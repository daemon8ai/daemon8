use serde::{Deserialize, Serialize};

fn default_model() -> String {
    "BAAI/bge-small-en-v1.5".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmbedConfig {
    #[serde(default)]
    pub provider: EmbedProvider,

    #[serde(default = "default_model")]
    pub model: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbedProvider {
    #[default]
    None,
    Fastembed,
    Ollama,
    Openai,
}

impl std::fmt::Display for EmbedProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Fastembed => write!(f, "fastembed"),
            Self::Ollama => write!(f, "ollama"),
            Self::Openai => write!(f, "openai"),
        }
    }
}

impl std::str::FromStr for EmbedProvider {
    type Err = EmbedProviderParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(Self::None),
            "fastembed" => Ok(Self::Fastembed),
            "ollama" => Ok(Self::Ollama),
            "openai" => Ok(Self::Openai),
            _ => Err(EmbedProviderParseError(s.to_owned())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown embed provider: {0} (expected: none, fastembed, ollama, openai)")]
pub struct EmbedProviderParseError(String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip() {
        let config = EmbedConfig {
            provider: EmbedProvider::Ollama,
            model: "nomic-embed-text".into(),
            endpoint: Some("http://localhost:11434".into()),
            api_key: None,
            base_url: None,
        };

        let json = serde_json::to_string(&config).unwrap();
        let parsed: EmbedConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.provider, EmbedProvider::Ollama);
        assert_eq!(parsed.model, "nomic-embed-text");
        assert_eq!(parsed.endpoint.as_deref(), Some("http://localhost:11434"));
        assert!(parsed.api_key.is_none());
        assert!(parsed.base_url.is_none());
    }

    #[test]
    fn config_defaults() {
        let config: EmbedConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.provider, EmbedProvider::None);
        assert_eq!(config.model, "BAAI/bge-small-en-v1.5");
        assert!(config.endpoint.is_none());
    }

    #[test]
    fn provider_display_parse_roundtrip() {
        for provider in [
            EmbedProvider::None,
            EmbedProvider::Fastembed,
            EmbedProvider::Ollama,
            EmbedProvider::Openai,
        ] {
            let s = provider.to_string();
            let parsed: EmbedProvider = s.parse().unwrap();
            assert_eq!(parsed, provider);
        }
    }

    #[test]
    fn provider_parse_unknown() {
        let result = "unknown".parse::<EmbedProvider>();
        assert!(result.is_err());
    }

    #[test]
    fn provider_serde_roundtrip() {
        let json = serde_json::to_string(&EmbedProvider::Fastembed).unwrap();
        assert_eq!(json, "\"fastembed\"");
        let parsed: EmbedProvider = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, EmbedProvider::Fastembed);
    }

    #[test]
    fn api_key_not_serialized_when_none() {
        let config = EmbedConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("api_key"));
    }
}
