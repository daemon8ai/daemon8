mod config;

#[cfg(feature = "fastembed")]
mod fastembed_backend;

#[cfg(feature = "ollama")]
mod ollama_backend;

#[cfg(feature = "openai")]
mod openai_backend;

use std::sync::Arc;

pub use config::{EmbedConfig, EmbedProvider, EmbedProviderParseError};

#[cfg(feature = "fastembed")]
pub use fastembed_backend::FastembedEmbedder;

#[cfg(feature = "ollama")]
pub use ollama_backend::OllamaEmbedder;

#[cfg(feature = "openai")]
pub use openai_backend::OpenaiEmbedder;

#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>;

    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }

    fn dimensions(&self) -> usize;
    fn model_name(&self) -> &str;
}

pub struct NoopEmbedder;

#[async_trait::async_trait]
impl Embedder for NoopEmbedder {
    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(Vec::new())
    }

    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(vec![Vec::new(); texts.len()])
    }

    fn dimensions(&self) -> usize {
        0
    }

    fn model_name(&self) -> &str {
        "none"
    }
}

pub fn create_embedder(config: &EmbedConfig) -> anyhow::Result<Arc<dyn Embedder>> {
    match config.provider {
        EmbedProvider::None => {
            tracing::debug!("embedding disabled, using noop embedder");
            Ok(Arc::new(NoopEmbedder))
        }

        #[cfg(feature = "fastembed")]
        EmbedProvider::Fastembed => {
            tracing::info!(model = %config.model, "initializing fastembed embedder");
            let embedder = FastembedEmbedder::new(&config.model)?;
            Ok(Arc::new(embedder))
        }

        #[cfg(not(feature = "fastembed"))]
        EmbedProvider::Fastembed => {
            anyhow::bail!(
                "fastembed provider requested but the 'fastembed' feature is not enabled -- \
                 rebuild with `--features fastembed`"
            )
        }

        #[cfg(feature = "ollama")]
        EmbedProvider::Ollama => {
            tracing::info!(model = %config.model, "initializing ollama embedder");
            let embedder = OllamaEmbedder::new(
                &config.model,
                config.endpoint.as_deref(),
            );
            Ok(Arc::new(embedder))
        }

        #[cfg(not(feature = "ollama"))]
        EmbedProvider::Ollama => {
            anyhow::bail!(
                "ollama provider requested but the 'ollama' feature is not enabled -- \
                 rebuild with `--features ollama`"
            )
        }

        #[cfg(feature = "openai")]
        EmbedProvider::Openai => {
            let api_key = config
                .api_key
                .clone()
                .ok_or_else(|| anyhow::anyhow!("openai provider requires api_key"))?;

            tracing::info!(model = %config.model, "initializing openai embedder");
            let embedder = OpenaiEmbedder::new(
                &config.model,
                api_key,
                config.base_url.as_deref(),
            )?;
            Ok(Arc::new(embedder))
        }

        #[cfg(not(feature = "openai"))]
        EmbedProvider::Openai => {
            anyhow::bail!(
                "openai provider requested but the 'openai' feature is not enabled -- \
                 rebuild with `--features openai`"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_embed_returns_empty() {
        let embedder = NoopEmbedder;
        let result = embedder.embed("hello").await.unwrap();
        assert!(result.is_empty());
        assert_eq!(embedder.dimensions(), 0);
        assert_eq!(embedder.model_name(), "none");
    }

    #[tokio::test]
    async fn noop_embed_batch() {
        let embedder = NoopEmbedder;
        let results = embedder.embed_batch(&["a", "b", "c"]).await.unwrap();
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|v| v.is_empty()));
    }

    #[tokio::test]
    async fn create_noop_from_config() {
        let config = EmbedConfig::default();
        let embedder = create_embedder(&config).unwrap();
        assert_eq!(embedder.dimensions(), 0);
        assert_eq!(embedder.model_name(), "none");

        let result = embedder.embed("test").await.unwrap();
        assert!(result.is_empty());
    }

    #[cfg(not(feature = "fastembed"))]
    #[test]
    fn create_fastembed_without_feature_fails() {
        let config = EmbedConfig {
            provider: EmbedProvider::Fastembed,
            ..Default::default()
        };
        let Err(e) = create_embedder(&config) else {
            panic!("expected error for disabled fastembed feature");
        };
        assert!(e.to_string().contains("fastembed"));
    }

    #[cfg(not(feature = "ollama"))]
    #[test]
    fn create_ollama_without_feature_fails() {
        let config = EmbedConfig {
            provider: EmbedProvider::Ollama,
            ..Default::default()
        };
        let Err(e) = create_embedder(&config) else {
            panic!("expected error for disabled ollama feature");
        };
        assert!(e.to_string().contains("ollama"));
    }

    #[cfg(not(feature = "openai"))]
    #[test]
    fn create_openai_without_feature_fails() {
        let config = EmbedConfig {
            provider: EmbedProvider::Openai,
            ..Default::default()
        };
        let Err(e) = create_embedder(&config) else {
            panic!("expected error for disabled openai feature");
        };
        assert!(e.to_string().contains("openai"));
    }
}
