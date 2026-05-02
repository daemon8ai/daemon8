use anyhow::Context;

use crate::Embedder;

pub struct OpenaiEmbedder {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
    dimensions: usize,
}

impl OpenaiEmbedder {
    pub fn new(model: &str, api_key: String, base_url: Option<&str>) -> anyhow::Result<Self> {
        if api_key.is_empty() {
            anyhow::bail!("openai api_key is required");
        }

        let dimensions = model_dimensions(model);

        Ok(Self {
            client: reqwest::Client::new(),
            base_url: base_url.unwrap_or("https://api.openai.com/v1").to_owned(),
            model: model.to_owned(),
            api_key,
            dimensions,
        })
    }
}

#[derive(serde::Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [&'a str],
}

#[derive(serde::Deserialize)]
struct EmbedResponse {
    data: Vec<EmbeddingData>,
}

#[derive(serde::Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    #[allow(dead_code)]
    index: usize,
}

#[async_trait::async_trait]
impl Embedder for OpenaiEmbedder {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let texts = [text];
        let mut results = self.embed_batch(&texts).await?;
        results
            .pop()
            .ok_or_else(|| anyhow::anyhow!("openai returned no embeddings"))
    }

    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        let url = format!("{}/embeddings", self.base_url);

        let body = EmbedRequest {
            model: &self.model,
            input: texts,
        };

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("openai embed request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("openai returned {status}: {body}");
        }

        let parsed: EmbedResponse = resp
            .json()
            .await
            .context("failed to parse openai response")?;

        let mut embeddings: Vec<_> = parsed
            .data
            .into_iter()
            .map(|d| (d.index, d.embedding))
            .collect();

        embeddings.sort_by_key(|(idx, _)| *idx);

        Ok(embeddings.into_iter().map(|(_, e)| e).collect())
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

fn model_dimensions(model: &str) -> usize {
    match model {
        "text-embedding-3-small" => 1536,
        "text-embedding-3-large" => 3072,
        "text-embedding-ada-002" => 1536,
        _ => {
            tracing::debug!(model, "unknown openai model dimensions, defaulting to 1536");
            1536
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction() {
        let embedder =
            OpenaiEmbedder::new("text-embedding-3-small", "sk-test-key".into(), None).unwrap();
        assert_eq!(embedder.model_name(), "text-embedding-3-small");
        assert_eq!(embedder.dimensions(), 1536);
        assert_eq!(embedder.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn custom_base_url() {
        let embedder = OpenaiEmbedder::new(
            "text-embedding-3-large",
            "sk-test".into(),
            Some("https://my-proxy.example.com/v1"),
        )
        .unwrap();
        assert_eq!(embedder.base_url, "https://my-proxy.example.com/v1");
        assert_eq!(embedder.dimensions(), 3072);
    }

    #[test]
    fn empty_api_key_rejected() {
        let result = OpenaiEmbedder::new("text-embedding-3-small", String::new(), None);
        assert!(result.is_err());
    }

    #[test]
    fn known_model_dimensions() {
        assert_eq!(model_dimensions("text-embedding-3-small"), 1536);
        assert_eq!(model_dimensions("text-embedding-3-large"), 3072);
        assert_eq!(model_dimensions("text-embedding-ada-002"), 1536);
    }
}
