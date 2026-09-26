pub mod bedrock_sonic;
pub mod codex;
pub mod openrouter;

use std::sync::{Arc, RwLock};

use anyhow::{anyhow, Result};

use self::openrouter::OpenRouterClient;

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
    use super::{openrouter::OpenRouterClient, SharedActiveModel};

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
