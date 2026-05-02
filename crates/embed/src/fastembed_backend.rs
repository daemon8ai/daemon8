use std::sync::{Arc, Mutex};

use anyhow::Context;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::Embedder;

pub struct FastembedEmbedder {
    // Arc so we can clone into spawn_blocking closures without borrowing &self
    model: Arc<Mutex<TextEmbedding>>,
    model_name: String,
    dimensions: usize,
}

impl FastembedEmbedder {
    pub fn new(model_name: &str) -> anyhow::Result<Self> {
        let embedding_model = resolve_model(model_name);
        let dimensions = model_dimensions(&embedding_model);

        let options = InitOptions::new(embedding_model).with_show_download_progress(true);
        let model =
            TextEmbedding::try_new(options).context("failed to initialize fastembed model")?;

        Ok(Self {
            model: Arc::new(Mutex::new(model)),
            model_name: model_name.to_owned(),
            dimensions,
        })
    }
}

#[async_trait::async_trait]
impl Embedder for FastembedEmbedder {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let input = text.to_owned();
        let model = Arc::clone(&self.model);

        let result = tokio::task::spawn_blocking(move || {
            let mut guard = model
                .lock()
                .map_err(|e| anyhow::anyhow!("mutex poisoned: {e}"))?;
            guard
                .embed(vec![input], None)
                .context("fastembed embed failed")
        })
        .await
        .context("fastembed task panicked")??;

        result
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("fastembed returned no embeddings"))
    }

    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        let owned: Vec<String> = texts.iter().map(|s| (*s).to_owned()).collect();
        let model = Arc::clone(&self.model);

        tokio::task::spawn_blocking(move || {
            let mut guard = model
                .lock()
                .map_err(|e| anyhow::anyhow!("mutex poisoned: {e}"))?;
            guard
                .embed(owned, None)
                .context("fastembed batch embed failed")
        })
        .await
        .context("fastembed batch task panicked")?
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}

fn resolve_model(name: &str) -> EmbeddingModel {
    match name {
        "BAAI/bge-small-en-v1.5" => EmbeddingModel::BGESmallENV15,
        "BAAI/bge-base-en-v1.5" => EmbeddingModel::BGEBaseENV15,
        "BAAI/bge-large-en-v1.5" => EmbeddingModel::BGELargeENV15,
        "sentence-transformers/all-MiniLM-L6-v2" => EmbeddingModel::AllMiniLML6V2,
        "sentence-transformers/all-MiniLM-L12-v2" => EmbeddingModel::AllMiniLML12V2,
        "nomic-ai/nomic-embed-text-v1" => EmbeddingModel::NomicEmbedTextV1,
        "nomic-ai/nomic-embed-text-v1.5" => EmbeddingModel::NomicEmbedTextV15,
        _ => {
            tracing::warn!(
                model = name,
                "unknown fastembed model, falling back to BGESmallENV15"
            );
            EmbeddingModel::BGESmallENV15
        }
    }
}

fn model_dimensions(model: &EmbeddingModel) -> usize {
    match model {
        EmbeddingModel::BGESmallENV15 => 384,
        EmbeddingModel::BGEBaseENV15 => 768,
        EmbeddingModel::BGELargeENV15 => 1024,
        EmbeddingModel::AllMiniLML6V2 => 384,
        EmbeddingModel::AllMiniLML12V2 => 384,
        EmbeddingModel::NomicEmbedTextV1 => 768,
        EmbeddingModel::NomicEmbedTextV15 => 768,
        _ => 384,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_known_models() {
        assert!(matches!(
            resolve_model("BAAI/bge-small-en-v1.5"),
            EmbeddingModel::BGESmallENV15
        ));
        assert!(matches!(
            resolve_model("sentence-transformers/all-MiniLM-L6-v2"),
            EmbeddingModel::AllMiniLML6V2
        ));
    }

    #[test]
    fn resolve_unknown_falls_back() {
        assert!(matches!(
            resolve_model("unknown-model"),
            EmbeddingModel::BGESmallENV15
        ));
    }

    #[test]
    fn dimensions_match_expected() {
        assert_eq!(model_dimensions(&EmbeddingModel::BGESmallENV15), 384);
        assert_eq!(model_dimensions(&EmbeddingModel::BGELargeENV15), 1024);
    }

    #[test]
    #[ignore] // requires ORT runtime download (~200MB)
    fn embed_text() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let embedder = FastembedEmbedder::new("BAAI/bge-small-en-v1.5").unwrap();
            let result = embedder.embed("hello world").await.unwrap();
            assert_eq!(result.len(), 384);
        });
    }
}
