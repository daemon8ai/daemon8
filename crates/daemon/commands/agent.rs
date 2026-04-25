use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use axum::body::Bytes;
use clap::Args;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_stream::StreamExt;

use daemon8_store::semantic;
use daemon8_types::Observation;

use crate::config;

const SSE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const QUEEN_PROPOSAL_WINDOW: Duration = Duration::from_millis(1200);
const MEMORY_HITS_LIMIT: usize = 6;

#[derive(Args, Clone, Debug)]
pub struct AgentArgs {
    #[arg(long, default_value = "worker")]
    pub role: String,

    #[arg(long, default_value = "gpt-5.4")]
    pub model: String,

    #[arg(long)]
    pub background: bool,

    #[arg(long)]
    pub prompt: Option<String>,

    #[arg(long)]
    pub daemon_url: Option<String>,

    #[arg(long)]
    pub project_root: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct PromptEnvelope {
    prompt_text: String,
    session_id: Option<String>,
    project_slug: Option<String>,
}

#[derive(Debug, Clone)]
struct ProposalEnvelope {
    proposal_text: String,
    session_id: Option<String>,
    source_role: Option<String>,
}

#[derive(Debug)]
struct PendingPrompt {
    prompt: PromptEnvelope,
    proposals: Vec<ProposalEnvelope>,
    deadline: tokio::time::Instant,
}

#[derive(Debug, Clone)]
enum StreamEvent {
    Prompt(PromptEnvelope),
    Proposal(ProposalEnvelope),
}

#[derive(Debug, Clone, Default)]
struct CompletionInput {
    prompt: String,
    proposals: Vec<ProposalEnvelope>,
}

struct AgentEventPayload<'a> {
    event: &'a str,
    role: &'a str,
    session_id: Option<&'a str>,
    project_slug: Option<&'a str>,
    prompt: &'a str,
    response: &'a str,
    proposals: &'a [ProposalEnvelope],
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessageResponse,
}

#[derive(Deserialize)]
struct ChatMessageResponse {
    content: String,
}

struct OpenAiCompatibleModel {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl OpenAiCompatibleModel {
    fn from_env(model: &str) -> Result<Self> {
        let api_key =
            env::var("OPENAI_API_KEY").context("OPENAI_API_KEY is required for agent inference")?;
        let base_url =
            env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            base_url,
            model: model.to_string(),
        })
    }

    async fn complete(&self, system: &str, user: &str) -> Result<String> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let request = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: system,
                },
                ChatMessage {
                    role: "user",
                    content: user,
                },
            ],
        };

        let response = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await?
            .error_for_status()?;

        let response: ChatResponse = response.json().await?;
        response
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content)
            .context("model returned no choices")
    }
}

pub async fn run_agent(args: AgentArgs) -> Result<()> {
    let daemon_url = resolve_daemon_url(args.daemon_url.as_deref())?;
    let project_root = args
        .project_root
        .clone()
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let db_path = config::resolve_db_path(
        config::load(None)
            .unwrap_or_default()
            .storage
            .path
            .as_deref(),
    );
    let conn = Connection::open(&db_path)
        .with_context(|| format!("failed to open {}", db_path.display()))?;
    semantic::ensure_schema(&conn)?;
    index_project_codebase(&conn, &project_root)?;

    if args.background {
        run_background(args, &daemon_url, &project_root, &conn).await
    } else {
        run_one_shot(args, &daemon_url, &project_root, &conn).await
    }
}

async fn run_one_shot(
    args: AgentArgs,
    daemon_url: &str,
    project_root: &Path,
    conn: &Connection,
) -> Result<()> {
    let prompt = match args.prompt {
        Some(ref prompt) => prompt.clone(),
        None => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            input.trim().to_string()
        }
    };
    if prompt.is_empty() {
        bail!("prompt is required in one-shot mode");
    }

    let model = OpenAiCompatibleModel::from_env(&args.model)?;
    let response = complete_with_memory(
        &model,
        &args.role,
        project_root,
        conn,
        CompletionInput {
            prompt: prompt.clone(),
            proposals: Vec::new(),
        },
    )
    .await?;
    emit_agent_event(
        daemon_url,
        AgentEventPayload {
            event: if args.role == "queen" {
                "agent.final_answer"
            } else {
                "agent.intent_proposal"
            },
            role: &args.role,
            session_id: None,
            project_slug: None,
            prompt: &prompt,
            response: &response,
            proposals: &[],
        },
    )
    .await?;
    println!("{response}");
    Ok(())
}

async fn run_background(
    args: AgentArgs,
    daemon_url: &str,
    project_root: &Path,
    conn: &Connection,
) -> Result<()> {
    let model = OpenAiCompatibleModel::from_env(&args.model)?;
    let client = reqwest::Client::new();
    let mut stream = ObservationStream::new(&client, daemon_url);
    let mut pending: BTreeMap<String, PendingPrompt> = BTreeMap::new();

    loop {
        flush_due_prompts(&args, daemon_url, project_root, conn, &model, &mut pending).await?;

        match tokio::time::timeout(SSE_POLL_INTERVAL, stream.next()).await {
            Ok(Ok(Some(observation))) => {
                if let Some(event) = decode_stream_event(observation)? {
                    handle_stream_event(
                        &args,
                        event,
                        &mut pending,
                        daemon_url,
                        project_root,
                        conn,
                        &model,
                    )
                    .await?;
                }
            }
            Ok(Ok(None)) => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Ok(Err(error)) => {
                eprintln!("[daemon8 agent] stream error: {error:#}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(_) => {}
        }
    }
}

async fn handle_stream_event(
    args: &AgentArgs,
    event: StreamEvent,
    pending: &mut BTreeMap<String, PendingPrompt>,
    daemon_url: &str,
    project_root: &Path,
    conn: &Connection,
    model: &OpenAiCompatibleModel,
) -> Result<()> {
    match (&args.role[..], event) {
        ("queen", StreamEvent::Prompt(prompt)) => {
            let key = pending_key(prompt.session_id.as_deref(), &prompt.prompt_text);
            pending.insert(
                key,
                PendingPrompt {
                    prompt,
                    proposals: Vec::new(),
                    deadline: tokio::time::Instant::now() + QUEEN_PROPOSAL_WINDOW,
                },
            );
        }
        ("queen", StreamEvent::Proposal(proposal)) => {
            if let Some(session_id) = proposal.session_id.as_deref() {
                for entry in pending.values_mut() {
                    if entry.prompt.session_id.as_deref() == Some(session_id) {
                        entry.proposals.push(proposal.clone());
                    }
                }
            }
        }
        (_, StreamEvent::Prompt(prompt)) => {
            let response = complete_with_memory(
                model,
                &args.role,
                project_root,
                conn,
                CompletionInput {
                    prompt: prompt.prompt_text.clone(),
                    proposals: Vec::new(),
                },
            )
            .await?;
            emit_agent_event(
                daemon_url,
                AgentEventPayload {
                    event: "agent.intent_proposal",
                    role: &args.role,
                    session_id: prompt.session_id.as_deref(),
                    project_slug: prompt.project_slug.as_deref(),
                    prompt: &prompt.prompt_text,
                    response: &response,
                    proposals: &[],
                },
            )
            .await?;
        }
        (_, StreamEvent::Proposal(_)) => {}
    }

    flush_due_prompts(args, daemon_url, project_root, conn, model, pending).await
}

async fn flush_due_prompts(
    args: &AgentArgs,
    daemon_url: &str,
    project_root: &Path,
    conn: &Connection,
    model: &OpenAiCompatibleModel,
    pending: &mut BTreeMap<String, PendingPrompt>,
) -> Result<()> {
    if args.role != "queen" {
        return Ok(());
    }

    let now = tokio::time::Instant::now();
    let ready_keys: Vec<String> = pending
        .iter()
        .filter(|(_, item)| item.deadline <= now)
        .map(|(key, _)| key.clone())
        .collect();

    for key in ready_keys {
        let Some(item) = pending.remove(&key) else {
            continue;
        };
        let response = complete_with_memory(
            model,
            &args.role,
            project_root,
            conn,
            CompletionInput {
                prompt: item.prompt.prompt_text.clone(),
                proposals: item.proposals.clone(),
            },
        )
        .await?;
        emit_agent_event(
            daemon_url,
            AgentEventPayload {
                event: "agent.final_answer",
                role: &args.role,
                session_id: item.prompt.session_id.as_deref(),
                project_slug: item.prompt.project_slug.as_deref(),
                prompt: &item.prompt.prompt_text,
                response: &response,
                proposals: &item.proposals,
            },
        )
        .await?;
    }

    Ok(())
}

async fn complete_with_memory(
    model: &OpenAiCompatibleModel,
    role: &str,
    project_root: &Path,
    conn: &Connection,
    input: CompletionInput,
) -> Result<String> {
    let memories = semantic::search(conn, &input.prompt, MEMORY_HITS_LIMIT)?;
    let mut context_pack = String::new();
    for hit in memories {
        context_pack.push_str(&format!(
            "[{}:{}#{} score={:.3}]\n{}\n\n",
            hit.source_kind, hit.source_key, hit.chunk_index, hit.score, hit.content
        ));
    }

    let mut proposals = String::new();
    if !input.proposals.is_empty() {
        for proposal in &input.proposals {
            let source_role = proposal.source_role.as_deref().unwrap_or("worker");
            proposals.push_str(&format!(
                "[proposal:{source_role}]\n{}\n\n",
                proposal.proposal_text
            ));
        }
    }

    let system = format!(
        "You are the daemon8 {role} agent.\n\
         Work statelessly. Use the provided context pack and current prompt only.\n\
         Project root: {root}\n\
         If you are queen, synthesize one final answer across any worker proposals.\n\
         If you are not queen, produce one focused implementation proposal.",
        root = project_root.display()
    );

    let user = if proposals.is_empty() {
        format!(
            "Context Pack:\n{context_pack}\nCurrent Prompt:\n{}",
            input.prompt
        )
    } else {
        format!(
            "Context Pack:\n{context_pack}\nWorker Proposals:\n{proposals}\nCurrent Prompt:\n{}",
            input.prompt
        )
    };

    model.complete(&system, &user).await
}

async fn emit_agent_event(daemon_url: &str, payload: AgentEventPayload<'_>) -> Result<()> {
    let app_name = format!("agent-{}", payload.role);
    let body = json!({
        "app": app_name,
        "kind": "custom",
        "channel": "agent",
        "severity": "info",
        "data": {
            "event": payload.event,
            "role": payload.role,
            "session_id": payload.session_id,
            "project_slug": payload.project_slug,
            "prompt_text": payload.prompt,
            "response_text": payload.response,
            "proposal_count": payload.proposals.len(),
            "proposals": payload.proposals.iter().map(|proposal| json!({
                "source_role": proposal.source_role,
                "response_text": proposal.proposal_text,
            })).collect::<Vec<_>>(),
            "timestamp": now_ns(),
        }
    });

    let url = format!("{}/ingest", daemon_url.trim_end_matches('/'));
    reqwest::Client::new()
        .post(url)
        .json(&body)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

fn index_project_codebase(conn: &Connection, root: &Path) -> Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).with_context(|| format!("read_dir {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            if should_skip(&path) {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !is_indexable_file(&path) {
                continue;
            }

            let content = fs::read_to_string(&path).unwrap_or_default();
            if content.trim().is_empty() {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let metadata = json!({
                "project_root": root.display().to_string(),
                "path": relative,
            });
            semantic::upsert_document(
                conn,
                "code",
                &relative,
                &content,
                &metadata.to_string(),
                now_ns(),
            )?;
        }
    }
    Ok(())
}

fn should_skip(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            matches!(
                name,
                ".git" | "node_modules" | "target" | ".aimind" | ".claude"
            )
        })
        .unwrap_or(false)
}

fn is_indexable_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("rs" | "ts" | "tsx" | "js" | "jsx" | "json" | "toml" | "md" | "yml" | "yaml")
    )
}

fn resolve_daemon_url(explicit: Option<&str>) -> Result<String> {
    if let Some(url) = explicit {
        return Ok(url.to_string());
    }
    let cfg = config::load(None).unwrap_or_default();
    Ok(format!("http://{}:{}", cfg.server.host, cfg.server.port))
}

fn pending_key(session_id: Option<&str>, prompt: &str) -> String {
    match session_id {
        Some(session_id) if !session_id.is_empty() => session_id.to_string(),
        _ => prompt.to_string(),
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn decode_stream_event(observation: Observation) -> Result<Option<StreamEvent>> {
    let data = observation.data;
    let event_name = data.get("event").and_then(Value::as_str);

    match event_name {
        Some("user.prompt_submitted") => Ok(prompt_from_data(&data).map(StreamEvent::Prompt)),
        Some("agent.intent_proposal") => Ok(proposal_from_data(&data).map(StreamEvent::Proposal)),
        _ => Ok(None),
    }
}

fn prompt_from_data(data: &Value) -> Option<PromptEnvelope> {
    let prompt_text = first_non_empty_string(data, &["prompt_text", "prompt"])?;
    Some(PromptEnvelope {
        prompt_text,
        session_id: string_field(data, "session_id"),
        project_slug: string_field(data, "project_slug"),
    })
}

fn proposal_from_data(data: &Value) -> Option<ProposalEnvelope> {
    let proposal_text = first_non_empty_string(data, &["response_text", "response"])?;
    Some(ProposalEnvelope {
        proposal_text,
        session_id: string_field(data, "session_id"),
        source_role: string_field(data, "role"),
    })
}

fn first_non_empty_string(data: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = data.get(*key).and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn string_field(data: &Value, key: &str) -> Option<String> {
    data.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

struct ObservationStream<'a> {
    client: &'a reqwest::Client,
    daemon_url: &'a str,
    last_event_id: Option<u64>,
    stream: Option<PinBoxStream>,
    buffer: String,
}

type PinBoxStream =
    std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

impl<'a> ObservationStream<'a> {
    fn new(client: &'a reqwest::Client, daemon_url: &'a str) -> Self {
        Self {
            client,
            daemon_url,
            last_event_id: None,
            stream: None,
            buffer: String::new(),
        }
    }

    async fn next(&mut self) -> Result<Option<Observation>> {
        loop {
            if self.stream.is_none() {
                self.stream = Some(self.open_stream().await?);
            }

            let mut stream = self.stream.take().context("missing stream state")?;
            let event = self.read_sse_event(&mut stream).await?;
            self.stream = Some(stream);

            match event {
                Some(frame) => {
                    if let Some(id) = frame.id {
                        self.last_event_id = Some(id);
                    }
                    if frame.event.as_deref() == Some("gap") {
                        continue;
                    }
                    if frame.data.trim().is_empty() {
                        continue;
                    }
                    let observation: Observation =
                        serde_json::from_str(&frame.data).context("parse SSE observation")?;
                    return Ok(Some(observation));
                }
                None => {
                    self.stream = None;
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
    }

    async fn open_stream(&self) -> Result<PinBoxStream> {
        let url = format!(
            "{}/api/stream?kinds=custom",
            self.daemon_url.trim_end_matches('/')
        );
        let mut request = self.client.get(url);
        if let Some(last_event_id) = self.last_event_id {
            request = request.header("Last-Event-ID", last_event_id.to_string());
        }
        let response = request.send().await?.error_for_status()?;
        Ok(Box::pin(response.bytes_stream()))
    }

    async fn read_sse_event(&mut self, stream: &mut PinBoxStream) -> Result<Option<SseFrame>> {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            self.buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(index) = self.buffer.find("\n\n") {
                let frame = self.buffer[..index].to_string();
                self.buffer.drain(..index + 2);
                let parsed = parse_sse_frame(&frame);
                if parsed.event.is_some() || parsed.id.is_some() || !parsed.data.is_empty() {
                    return Ok(Some(parsed));
                }
            }
        }

        self.buffer.clear();
        Ok(None)
    }
}

#[derive(Debug, Default)]
struct SseFrame {
    event: Option<String>,
    id: Option<u64>,
    data: String,
}

fn parse_sse_frame(frame: &str) -> SseFrame {
    let mut parsed = SseFrame::default();

    for raw_line in frame.lines() {
        let line = raw_line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("event:") {
            parsed.event = Some(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("id:") {
            parsed.id = rest.trim().parse().ok();
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            if !parsed.data.is_empty() {
                parsed.data.push('\n');
            }
            parsed.data.push_str(rest.trim_start());
        }
    }

    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sse_frame_reads_id_event_and_data() {
        let frame = parse_sse_frame("id: 42\nevent: gap\ndata: {\"ok\":true}\n");
        assert_eq!(frame.id, Some(42));
        assert_eq!(frame.event.as_deref(), Some("gap"));
        assert_eq!(frame.data, "{\"ok\":true}");
    }

    #[test]
    fn prompt_from_data_prefers_prompt_text() {
        let prompt = prompt_from_data(&json!({
            "prompt": "old",
            "prompt_text": "new",
            "session_id": "abc",
            "project_slug": "proj",
        }))
        .unwrap();

        assert_eq!(prompt.prompt_text, "new");
        assert_eq!(prompt.session_id.as_deref(), Some("abc"));
        assert_eq!(prompt.project_slug.as_deref(), Some("proj"));
    }

    #[test]
    fn proposal_from_data_reads_response_aliases() {
        let proposal = proposal_from_data(&json!({
            "response": "legacy",
            "role": "worker",
            "session_id": "abc",
        }))
        .unwrap();

        assert_eq!(proposal.proposal_text, "legacy");
        assert_eq!(proposal.source_role.as_deref(), Some("worker"));
        assert_eq!(proposal.session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn pending_key_prefers_session_id() {
        assert_eq!(pending_key(Some("session-1"), "hello"), "session-1");
        assert_eq!(pending_key(None, "hello"), "hello");
    }
}
