//! MCP provider (T3).
//!
//! The Model Context Protocol is one provider kind on the T2 external host.
//! Configuration names the allowed servers (`~/.config/hyusk/mcp.json`), each
//! with its command, arguments, environment allowlist, and optional tool
//! grant. The host spawns each server over stdio, discovers its tools with
//! `server/discover` (2026-07-28) or the legacy `initialize` handshake, and
//! registers them as `ext.<server>.<tool>` under the one always-ask namespace
//! row.
//!
//! The trust boundary is the same as T2's, with two MCP-specific rules:
//!
//! - the model-facing description is written by the host from the tool name and
//!   schema, never forwarded from the server, and the server's annotations
//!   (`readOnlyHint`, `destructiveHint`) are ignored;
//! - a server only receives `PATH` and the environment names the operator
//!   listed, and only the tools its grant allows (when a grant is present) are
//!   offered.
//!
//! `HYUSK_MCP=0` disables the provider. A removed server stops being offered on
//! the next turn without a restart: a watcher rereads the file and deactivates
//! it, the registry omits unavailable tools from the model specs, and a call to
//! one fails closed. Adding a server still needs a restart, because its tools
//! have to be registered.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::external::{ExternalHost, ServerKind, ServerSpec, MAX_SERVERS};
use super::registry::ToolRegistry;

/// The environment variable that enables or disables the provider.
pub const ENV_VAR: &str = "HYUSK_MCP";
/// An optional override for the configuration path.
pub const CONFIG_ENV_VAR: &str = "HYUSK_MCP_CONFIG";
/// How often the configuration file is checked for a removed server.
pub const REFRESH_ENV_VAR: &str = "HYUSK_MCP_REFRESH_SECS";

/// The default refresh interval.
pub const DEFAULT_REFRESH_SECS: u64 = 30;
/// The shortest refresh interval a configuration may ask for.
pub const MIN_REFRESH_SECS: u64 = 5;
/// The longest refresh interval a configuration may ask for.
pub const MAX_REFRESH_SECS: u64 = 3600;

/// The configuration file: a `servers` array.
#[derive(Debug, Deserialize)]
struct McpConfig {
    #[serde(default)]
    servers: Vec<McpServer>,
}

/// One configured MCP server.
#[derive(Debug, Deserialize)]
struct McpServer {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    /// Environment names passed from the agent's environment; everything else
    /// is withheld from the child (except `PATH`).
    #[serde(default)]
    env: Vec<String>,
    /// When present, only these tool names are offered. The grant can only
    /// narrow the set; it never relaxes approval.
    #[serde(default)]
    allow: Option<Vec<String>>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    startup_timeout_ms: Option<u64>,
}

/// Whether the provider is enabled. Unset means enabled; `0`, `false`, `off`,
/// and `no` disable it, case-insensitively.
pub fn enabled_from(value: Option<&str>) -> bool {
    !matches!(
        value.unwrap_or("1").trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "off" | "no"
    )
}

/// Whether the provider is enabled in this process.
pub fn enabled() -> bool {
    enabled_from(std::env::var(ENV_VAR).ok().as_deref())
}

/// The configuration path: `HYUSK_MCP_CONFIG`, else
/// `$XDG_CONFIG_HOME/hyusk/mcp.json` (or `~/.config`).
pub fn config_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(CONFIG_ENV_VAR) {
        return Some(PathBuf::from(path));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("hyusk").join("mcp.json"))
}

/// One running MCP provider and the servers it started.
pub struct McpProvider {
    path: PathBuf,
    host: Arc<ExternalHost>,
}

impl McpProvider {
    /// Start the servers named by `path`, or `None` when the file is absent,
    /// empty, or no server answered. A configuration error is logged, not
    /// fatal: the rest of the assistant keeps working.
    pub async fn start_at(path: PathBuf) -> Option<Arc<Self>> {
        // Most installations never configure MCP: a missing file is the normal
        // "provider off" state, not an error to log.
        if !path.exists() {
            return None;
        }
        let specs = match load_specs(&path) {
            Ok(specs) => specs,
            Err(error) => {
                eprintln!("[MCP] Configuration not loaded: {error:#}");
                return None;
            }
        };
        if specs.is_empty() {
            return None;
        }

        let host = Arc::new(ExternalHost::start(specs).await);
        if host.server_count() == 0 {
            eprintln!("[MCP] No configured server answered");
            return None;
        }
        Some(Arc::new(Self { path, host }))
    }

    /// Register every discovered tool under its namespaced name.
    pub fn register_into(&self, registry: &mut ToolRegistry) {
        self.host.register_into(registry);
    }

    /// Reread the configuration and stop offering a server that was removed
    /// (T3). A missing or invalid file deactivates every server: the operator
    /// removed the provider, so the tools fail closed rather than linger.
    ///
    /// A newly added server is logged but not loaded; its tools need a
    /// registration that only the composition root can do.
    pub async fn refresh(&self) {
        match load_specs(&self.path) {
            Ok(specs) => {
                let wanted: HashSet<&str> = specs.iter().map(|spec| spec.name.as_str()).collect();
                for name in self.host.server_names() {
                    if !wanted.contains(name.as_str()) && self.host.deactivate_server(&name).await {
                        eprintln!("[MCP] Server {name:?} left the configuration; its tools stop");
                    }
                }
                for spec in &specs {
                    if !self.host.server_names().contains(&spec.name) {
                        eprintln!(
                            "[MCP] Server {:?} was added; restart Hyusk to load its tools",
                            spec.name
                        );
                    }
                }
            }
            Err(_) => {
                for name in self.host.server_names() {
                    self.host.deactivate_server(&name).await;
                }
            }
        }
    }

    /// Kill and reap every child.
    pub async fn shutdown(&self) {
        self.host.shutdown().await;
    }

    /// How many servers answered.
    pub fn server_count(&self) -> usize {
        self.host.server_count()
    }

    /// How many tools the servers declared.
    pub fn tool_count(&self) -> usize {
        self.host.tool_count()
    }
}

/// Watch the configuration and refresh when its content changes.
///
/// The task lives as long as the process; it polls a small file rather than
/// holding an inotify watch, which keeps it independent of the desktop session.
pub fn spawn_refresh_worker(provider: Arc<McpProvider>) {
    let seconds = std::env::var(REFRESH_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_REFRESH_SECS)
        .clamp(MIN_REFRESH_SECS, MAX_REFRESH_SECS);

    tokio::spawn(async move {
        let mut last = std::fs::read(&provider.path).ok();
        loop {
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            let current = std::fs::read(&provider.path).ok();
            if current != last {
                last = current;
                provider.refresh().await;
            }
        }
    });
}

/// Read and validate a configuration file into host specs.
fn load_specs(path: &Path) -> Result<Vec<ServerSpec>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading the MCP configuration {}", path.display()))?;
    let config: McpConfig = serde_json::from_str(&text)
        .context("the MCP configuration must be a JSON object with a servers array")?;
    if config.servers.len() > MAX_SERVERS {
        bail!(
            "the MCP configuration lists {} servers, over the {MAX_SERVERS} limit",
            config.servers.len()
        );
    }

    let mut names = HashSet::new();
    let mut specs = Vec::new();
    for server in config.servers {
        let spec = ServerSpec {
            name: server.name,
            command: server.command,
            args: server.args,
            timeout_ms: server.timeout_ms,
            startup_timeout_ms: server.startup_timeout_ms,
            kind: ServerKind::Mcp,
            env: Some(server.env),
            allow: server.allow,
        };
        spec.validate()?;
        if !names.insert(spec.name.clone()) {
            bail!("two MCP servers are named {:?}", spec.name);
        }
        specs.push(spec);
    }
    Ok(specs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::safety::requires_approval;
    use std::path::PathBuf;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "hyusk-t3-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("scratch directory");
        path
    }

    fn server_json(name: &str, mode: &str, dir: &Path) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "command": "sh",
            "args": [
                format!("{}/tests/fixtures/mcp_server.sh", env!("CARGO_MANIFEST_DIR")),
                mode,
                name,
                dir.join(format!("{name}.pid")).to_string_lossy(),
            ],
            "env": [],
            "timeout_ms": 2_000,
            "startup_timeout_ms": 5_000,
        })
    }

    fn write_config(path: &Path, servers: &[serde_json::Value]) {
        std::fs::write(path, serde_json::json!({ "servers": servers }).to_string())
            .expect("write MCP configuration");
    }

    fn read_pid(dir: &Path, name: &str) -> Option<u32> {
        std::fs::read_to_string(dir.join(format!("{name}.pid")))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    async fn process_gone(pid: u32) -> bool {
        for _ in 0..100 {
            if !Path::new(&format!("/proc/{pid}")).exists() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_configured_server_appears_with_curated_descriptions() {
        let dir = scratch("curated");
        let config = dir.join("mcp.json");
        write_config(&config, &[server_json("fixture", "modern", &dir)]);

        let provider = McpProvider::start_at(config).await.expect("provider");
        let mut registry = ToolRegistry::new();
        provider.register_into(&mut registry);

        let read = registry
            .get("ext.fixture.read_note")
            .expect("read_note is registered");
        let manifest = read.manifest();
        assert!(
            manifest.description.contains("Read-only"),
            "{}",
            manifest.description
        );
        let wipe = registry
            .get("ext.fixture.wipe_everything")
            .expect("wipe_everything is registered");
        assert!(
            wipe.manifest().description.contains("Changes data"),
            "{}",
            wipe.manifest().description
        );

        // The server's own text and its annotations are not shown or trusted.
        for tool in [&read, &wipe] {
            assert!(!tool.manifest().description.contains("IGNORE"));
            assert!(!tool.manifest().description.contains("read-only safe"));
        }

        let result = read
            .execute(r#"{"text":"hello"}"#)
            .await
            .expect("call succeeds");
        assert!(result.success, "{:?}", result.error);
        assert_eq!(result.output, "mcp: hello");

        // Every external call still goes through the host gate.
        assert!(requires_approval(
            "ext.fixture.wipe_everything",
            r#"{"text":"x"}"#
        ));
        provider.shutdown().await;
    }

    #[tokio::test]
    async fn a_legacy_server_is_discovered_through_initialize() {
        let dir = scratch("legacy");
        let config = dir.join("mcp.json");
        write_config(&config, &[server_json("legacy", "legacy", &dir)]);

        let provider = McpProvider::start_at(config).await.expect("provider");
        let mut registry = ToolRegistry::new();
        provider.register_into(&mut registry);

        let tool = registry
            .get("ext.legacy.read_note")
            .expect("the legacy tool is registered");
        assert!(tool.manifest().description.contains("Read-only"));
        let result = tool.execute(r#"{"text":"hi"}"#).await.unwrap();
        assert_eq!(result.output, "mcp: hi");
        provider.shutdown().await;
    }

    #[tokio::test]
    async fn removing_a_server_removes_its_tools_without_a_restart() {
        let dir = scratch("removal");
        let config = dir.join("mcp.json");
        write_config(
            &config,
            &[
                server_json("alpha", "modern", &dir),
                server_json("beta", "modern", &dir),
            ],
        );

        let provider = McpProvider::start_at(config.clone())
            .await
            .expect("provider");
        let mut registry = ToolRegistry::new();
        provider.register_into(&mut registry);
        assert!(registry.get("ext.alpha.read_note").is_some());
        assert!(registry.get("ext.beta.read_note").is_some());
        let beta_pid = read_pid(&dir, "beta").expect("beta wrote its pid");

        // The operator removes beta and the watcher refreshes.
        write_config(&config, &[server_json("alpha", "modern", &dir)]);
        provider.refresh().await;

        assert!(
            registry.get("ext.beta.read_note").is_none(),
            "the removed server's tool is still offered"
        );
        assert!(
            registry
                .specs()
                .iter()
                .all(|spec| spec["function"]["name"] != "ext.beta.read_note"),
            "the removed server's tool is still in the model specs"
        );
        assert!(registry.get("ext.alpha.read_note").is_some());
        assert!(process_gone(beta_pid).await, "beta {beta_pid} still runs");
        provider.shutdown().await;
    }

    #[tokio::test]
    async fn removing_the_configuration_removes_every_tool() {
        let dir = scratch("deleted");
        let config = dir.join("mcp.json");
        write_config(&config, &[server_json("alpha", "modern", &dir)]);

        let provider = McpProvider::start_at(config.clone())
            .await
            .expect("provider");
        let mut registry = ToolRegistry::new();
        provider.register_into(&mut registry);
        assert!(registry.get("ext.alpha.read_note").is_some());

        std::fs::remove_file(&config).expect("remove configuration");
        provider.refresh().await;
        assert!(registry.get("ext.alpha.read_note").is_none());
        provider.shutdown().await;
    }

    #[test]
    fn the_kill_switch_is_a_documented_value() {
        for value in [None, Some("1"), Some("true"), Some("on")] {
            assert!(enabled_from(value), "{value:?} enables the provider");
        }
        for value in [
            Some("0"),
            Some("false"),
            Some("off"),
            Some("no"),
            Some(" FALSE "),
        ] {
            assert!(!enabled_from(value), "{value:?} disables the provider");
        }
    }

    #[test]
    fn the_configuration_is_validated() {
        let dir = scratch("config");
        let config = dir.join("mcp.json");

        for servers in [
            // duplicate names
            vec![
                serde_json::json!({"name":"one","command":"sh"}),
                serde_json::json!({"name":"one","command":"sh"}),
            ],
            // a name that is not a namespace segment
            vec![serde_json::json!({"name":"bad name","command":"sh"})],
            // an allow entry that is not a tool name
            vec![serde_json::json!({"name":"one","command":"sh","allow":["bad name"]})],
            // an environment name with an assignment in it
            vec![serde_json::json!({"name":"one","command":"sh","env":["A=B"]})],
        ] {
            write_config(&config, &servers);
            assert!(load_specs(&config).is_err(), "{servers:?} must be refused");
        }

        // A minimal valid file yields one MCP spec with an empty allowlist.
        write_config(&config, &[serde_json::json!({"name":"one","command":"sh"})]);
        let specs = load_specs(&config).expect("valid configuration");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].kind, ServerKind::Mcp);
        assert_eq!(specs[0].env, Some(Vec::new()));
        assert_eq!(specs[0].allow, None);
    }

    #[tokio::test]
    async fn an_absent_configuration_registers_nothing() {
        let dir = scratch("absent");
        let config = dir.join("mcp.json");
        assert!(McpProvider::start_at(config).await.is_none());
    }

    #[tokio::test]
    async fn a_configuration_error_does_not_abort_startup() {
        let dir = scratch("invalid");
        let config = dir.join("mcp.json");
        std::fs::write(&config, "not json").expect("write invalid configuration");
        assert!(McpProvider::start_at(config).await.is_none());
    }

    #[tokio::test]
    async fn an_allow_list_narrows_the_offered_tools() {
        let dir = scratch("allow");
        let config = dir.join("mcp.json");
        let mut server = server_json("fixture", "modern", &dir);
        server["allow"] = serde_json::json!(["read_note"]);
        write_config(&config, &[server]);

        let provider = McpProvider::start_at(config).await.expect("provider");
        let mut registry = ToolRegistry::new();
        provider.register_into(&mut registry);
        assert!(registry.get("ext.fixture.read_note").is_some());
        assert!(
            registry.get("ext.fixture.wipe_everything").is_none(),
            "the grant must narrow the offered set"
        );
        provider.shutdown().await;
    }
}
