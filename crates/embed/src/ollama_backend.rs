use std::sync::OnceLock;

use anyhow::Context;

use crate::Embedder;

pub struct OllamaEmbedder {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    dimensions: OnceLock<usize>,
}

impl OllamaEmbedder {
    pub fn new(model: &str, endpoint: Option<&str>) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint: endpoint.unwrap_or("http://localhost:11434").to_owned(),
            model: model.to_owned(),
            dimensions: OnceLock::new(),
        }
    }
}

#[derive(serde::Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [&'a str],
}

#[derive(serde::Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[async_trait::async_trait]
impl Embedder for OllamaEmbedder {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let texts = [text];
        let mut results = self.embed_batch(&texts).await?;
        results
            .pop()
            .ok_or_else(|| anyhow::anyhow!("ollama returned no embeddings"))
    }

    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        let url = format!("{}/api/embed", self.endpoint);

        let body = EmbedRequest {
            model: &self.model,
            input: texts,
        };

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("ollama embed request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("ollama returned {status}: {body}");
        }

        let parsed: EmbedResponse = resp.json().await.context("failed to parse ollama response")?;

        if let Some(first) = parsed.embeddings.first() {
            let _ = self.dimensions.set(first.len());
        }

        Ok(parsed.embeddings)
    }

    fn dimensions(&self) -> usize {
        // Returns cached dimensions from the first successful embed call,
        // or the well-known default for nomic-embed-text
        *self.dimensions.get().unwrap_or(&768)
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction() {
        let embedder = OllamaEmbedder::new("nomic-embed-text", None);
        assert_eq!(embedder.model_name(), "nomic-embed-text");
        assert_eq!(embedder.endpoint, "http://localhost:11434");
        assert_eq!(embedder.dimensions(), 768);
    }

    #[test]
    fn custom_endpoint() {
        let embedder = OllamaEmbedder::new("mxbai-embed-large", Some("http://gpu-box:11434"));
        assert_eq!(embedder.endpoint, "http://gpu-box:11434");
        assert_eq!(embedder.model_name(), "mxbai-embed-large");
    }
}
