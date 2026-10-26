pub mod bedrock_sonic;
pub mod codex;
pub mod openrouter;

use std::sync::{Arc, RwLock};

use anyhow::{anyhow, Result};

use self::openrouter::OpenRouterClient;

/// Build an HTTP client for an API provider from a key.
///
/// Returns `None` for providers that are not HTTP model APIs (Nova Sonic and
/// Codex authenticate differently), so callers can report an unusable choice
/// instead of silently substituting a base URL.
pub fn client_for_provider(provider: &str, key: impl Into<String>) -> Option<OpenRouterClient> {
    let key = key.into();
    if key.trim().is_empty() {
        return None;
    }
    let base_url = match provider {
        "openrouter" => std::env::var("OPENROUTER_BASE_URL")
            .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_string()),
        "openai" => "https://api.openai.com/v1".to_string(),
        "bedrock" => {
            let region =
                std::env::var("BEDROCK_REGION").unwrap_or_else(|_| "us-east-1".to_string());
            format!("https://bedrock-mantle.{region}.api.aws/v1")
        }
        _ => return None,
    };
    Some(OpenRouterClient::new(key, base_url))
}

/// Resolve a provider's key (keyring first, then the environment) and build
/// its client. A key saved from the indicator therefore works without a
/// service restart, and an environment key keeps working when the keyring has
/// no entry yet.
pub async fn configured_client(provider: &str) -> Option<OpenRouterClient> {
    let key = crate::credentials::lookup(provider)
        .await
        .or_else(|| env_key(provider))
        .filter(|key| !key.trim().is_empty())?;
    client_for_provider(provider, key)
}

fn env_key(provider: &str) -> Option<String> {
    match provider {
        "openrouter" => std::env::var("OPENROUTER_API_KEY").ok(),
        "openai" => std::env::var("OPENAI_API_KEY").ok(),
        "bedrock" => ["BEDROCK_API_KEY", "AWS_BEARER_TOKEN_BEDROCK", "AWS_API_KEY"]
            .into_iter()
            .find_map(|name| std::env::var(name).ok()),
        _ => None,
    }
}

/// Provider/model pair used by delegated research work. The state is shared
/// with the main agent so newly delegated work follows its current selection.
#[derive(Clone)]
pub struct SharedActiveModel(Arc<RwLock<ActiveModelSnapshot>>);

#[derive(Clone)]
pub struct ActiveModelSnapshot {
    pub provider: String,
    pub model: String,
    pub client: OpenRouterClient,
}

impl SharedActiveModel {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        client: OpenRouterClient,
    ) -> Self {
        Self(Arc::new(RwLock::new(ActiveModelSnapshot {
            provider: provider.into(),
            model: model.into(),
            client,
        })))
    }

    /// Change the provider and model used for future delegated research calls.
    pub fn select(
        &self,
        provider: impl Into<String>,
        model: impl Into<String>,
        client: OpenRouterClient,
    ) -> Result<()> {
        let mut active = self
            .0
            .write()
            .map_err(|_| anyhow!("active model state is unavailable"))?;
        *active = ActiveModelSnapshot {
            provider: provider.into(),
            model: model.into(),
            client,
        };
        Ok(())
    }

    /// Return a consistent snapshot for a newly delegated task.
    pub fn snapshot(&self) -> Result<ActiveModelSnapshot> {
        self.0
            .read()
            .map(|active| active.clone())
            .map_err(|_| anyhow!("active model state is unavailable"))
    }
}

#[cfg(test)]
mod tests {
    use super::{client_for_provider, openrouter::OpenRouterClient, SharedActiveModel};

    #[test]
    fn api_clients_reject_providers_that_do_not_use_http_keys() {
        assert!(client_for_provider("openrouter", "key").is_some());
        assert!(client_for_provider("openai", "key").is_some());
        assert!(client_for_provider("bedrock", "key").is_some());
        assert!(client_for_provider("codex", "key").is_none());
        assert!(client_for_provider("bedrock-sonic", "key").is_none());
        assert!(client_for_provider("openrouter", "   ").is_none());
    }

    #[test]
    fn selection_updates_future_research_snapshots() {
        let first = OpenRouterClient::new("first".into(), "https://example.com/v1".into());
        let second = OpenRouterClient::new("second".into(), "https://example.org/v1".into());
        let active = SharedActiveModel::new("openrouter", "model-a", first);
        let before = active.snapshot().expect("first snapshot");
        active
            .select("bedrock", "model-b", second)
            .expect("switch model");
        let after = active.snapshot().expect("second snapshot");
        assert_eq!(
            (before.provider.as_str(), before.model.as_str()),
            ("openrouter", "model-a")
        );
        assert_eq!(
            (after.provider.as_str(), after.model.as_str()),
            ("bedrock", "model-b")
        );
    }
}
