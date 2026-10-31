//! Contract tests for external providers (T4).
//!
//! T2's local host protocol and T3's MCP provider are different wire
//! protocols, but the guarantees the host gives the user are the same. These
//! tests drive both fixture servers through one table, so a guarantee cannot
//! silently hold for one provider kind and not the other:
//!
//! - discovery yields conformant manifests in the `ext.<server>.<tool>`
//!   namespace, with the model specs generated from them;
//! - every tool asks for approval, and no turn grant or focus context can
//!   change that;
//! - malformed arguments are refused by the host, before the server sees a
//!   call;
//! - a valid call works;
//! - shutdown kills and reaps the children.
//!
//! The fixtures are the two scripts the T2 and T3 tests already use:
//! `tests/fixtures/external_server.sh` and `tests/fixtures/mcp_server.sh`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::tools::external::{ExternalHost, ServerKind, ServerSpec};
use crate::tools::manifest::Source;
use crate::tools::mcp::McpProvider;
use crate::tools::registry::ToolRegistry;
use crate::tools::safety::{policy_for, requires_approval, requires_focus_context};

/// One provider kind, boxed so the same contract runs against both.
enum Provider {
    Local(Arc<ExternalHost>),
    Mcp(Arc<McpProvider>),
}

impl Provider {
    fn label(&self) -> &'static str {
        match self {
            Provider::Local(_) => "local (T2)",
            Provider::Mcp(_) => "MCP (T3)",
        }
    }

    fn register(&self, registry: &mut ToolRegistry) {
        match self {
            Provider::Local(host) => host.register_into(registry),
            Provider::Mcp(provider) => provider.register_into(registry),
        }
    }

    async fn shutdown(&self) {
        match self {
            Provider::Local(host) => host.shutdown().await,
            Provider::Mcp(provider) => provider.shutdown().await,
        }
    }

    /// The fixture's own request log.
    fn request_log(&self, dir: &Path) -> PathBuf {
        match self {
            Provider::Local(_) => dir.join("calls.log"),
            Provider::Mcp(_) => dir.join("fixture.log"),
        }
    }

    fn pid_file(&self, dir: &Path) -> PathBuf {
        match self {
            Provider::Local(_) => dir.join("pid"),
            Provider::Mcp(_) => dir.join("fixture.pid"),
        }
    }
}

fn scratch(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "hyusk-contract-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("scratch directory");
    path
}

fn local_spec(dir: &Path) -> ServerSpec {
    ServerSpec {
        name: "fixture".to_string(),
        command: "sh".to_string(),
        args: vec![
            format!(
                "{}/tests/fixtures/external_server.sh",
                env!("CARGO_MANIFEST_DIR")
            ),
            "happy".to_string(),
            dir.join("calls.log").to_string_lossy().into_owned(),
            dir.join("state").to_string_lossy().into_owned(),
            dir.join("pid").to_string_lossy().into_owned(),
        ],
        timeout_ms: Some(2_000),
        startup_timeout_ms: Some(5_000),
        kind: ServerKind::Local,
        env: None,
        allow: None,
    }
}

fn mcp_config(dir: &Path) -> PathBuf {
    let path = dir.join("mcp.json");
    let server = serde_json::json!({
        "name": "fixture",
        "command": "sh",
        "args": [
            format!("{}/tests/fixtures/mcp_server.sh", env!("CARGO_MANIFEST_DIR")),
            "modern",
            "fixture",
            dir.join("fixture.pid").to_string_lossy(),
            dir.join("fixture.log").to_string_lossy(),
        ],
        "env": [],
        "timeout_ms": 2_000,
        "startup_timeout_ms": 5_000,
    });
    std::fs::write(
        &path,
        serde_json::json!({ "servers": [server] }).to_string(),
    )
    .expect("write MCP configuration");
    path
}

async fn providers(dir: &Path) -> Vec<Provider> {
    let local = ExternalHost::start(vec![local_spec(dir)]).await;
    assert_eq!(
        local.server_count(),
        1,
        "the local fixture did not start: {:?}",
        std::fs::read_to_string(dir.join("calls.log"))
    );

    let mcp = McpProvider::start_at(mcp_config(dir))
        .await
        .expect("the MCP fixture did not start");
    assert!(mcp.tool_count() > 0, "the MCP fixture declared no tools");

    vec![Provider::Local(Arc::new(local)), Provider::Mcp(mcp)]
}

async fn process_gone(pid: u32) -> bool {
    for _ in 0..100 {
        if !Path::new(&format!("/proc/{pid}")).exists() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn every_provider_kind_registers_namespaced_conformant_tools() {
    let dir = scratch("manifests");

    for provider in providers(&dir).await {
        let label = provider.label();
        let mut registry = ToolRegistry::new();
        provider.register(&mut registry);

        let manifests = registry.manifests();
        assert!(!manifests.is_empty(), "{label} registered no tools");
        for manifest in &manifests {
            assert!(
                manifest.name.starts_with("ext."),
                "{label} registered {} outside the external namespace",
                manifest.name
            );
            assert_eq!(
                manifest.source,
                Source::External,
                "{label} registered {} as a core tool",
                manifest.name
            );
            assert_eq!(
                manifest.conformance(),
                Ok(()),
                "{label} registered an unusable manifest for {}",
                manifest.name
            );
            assert!(
                !manifest.risk.is_empty(),
                "{label} registered {} with no risk class",
                manifest.name
            );
        }

        // The model specs are the validated manifests, generated by the
        // registry, not a second copy from the server.
        let specs = registry.specs();
        assert_eq!(specs.len(), manifests.len(), "{label} spec count");
        for (spec, manifest) in specs.iter().zip(manifests.iter()) {
            assert_eq!(spec["type"], "function");
            assert_eq!(spec["function"]["name"], manifest.name);
            assert_eq!(spec["function"]["description"], manifest.description);
            assert_eq!(spec["function"]["parameters"], manifest.schema.build());
        }

        provider.shutdown().await;
    }
}

#[tokio::test]
async fn every_provider_kind_asks_for_every_tool() {
    let dir = scratch("gate");

    for provider in providers(&dir).await {
        let label = provider.label();
        let mut registry = ToolRegistry::new();
        provider.register(&mut registry);

        for manifest in registry.manifests() {
            let name = manifest.name;
            assert!(
                policy_for(name).is_some(),
                "{label}: {name} has no policy row"
            );
            assert!(
                requires_approval(name, "{}"),
                "{label}: {name} does not ask"
            );
            assert!(
                requires_approval(name, r#"{"text":"hello"}"#),
                "{label}: {name} asks only for an empty call"
            );
            assert!(
                !requires_focus_context(name, r#"{"text":"hello"}"#),
                "{label}: {name} claims a focus-dependent input path"
            );
            assert_eq!(
                crate::tools::safety::grant_key(name, r#"{"text":"hello"}"#),
                None,
                "{label}: {name} can be covered by a turn grant"
            );
        }

        provider.shutdown().await;
    }
}

#[tokio::test]
async fn every_provider_kind_refuses_malformed_arguments_before_the_server() {
    let dir = scratch("arguments");

    for provider in providers(&dir).await {
        let label = provider.label();
        let mut registry = ToolRegistry::new();
        provider.register(&mut registry);
        let name = registry
            .manifests()
            .first()
            .expect("at least one tool")
            .name;
        let tool = registry.get(name).expect("the first tool is offered");

        for input in ["not json", r#"{"text":5}"#] {
            let result = tool.execute(input).await.expect("a refusal is a result");
            assert!(!result.success, "{label}: {name} accepted {input}");
        }

        let log = std::fs::read_to_string(provider.request_log(&dir)).unwrap_or_default();
        assert!(
            !log.contains("tools/call"),
            "{label}: the server saw a malformed call: {log}"
        );

        provider.shutdown().await;
    }
}

#[tokio::test]
async fn every_provider_kind_calls_a_tool() {
    let dir = scratch("call");

    for provider in providers(&dir).await {
        let label = provider.label();
        let mut registry = ToolRegistry::new();
        provider.register(&mut registry);
        let name = registry
            .manifests()
            .first()
            .expect("at least one tool")
            .name;
        let tool = registry.get(name).expect("the first tool is offered");

        let result = tool
            .execute(r#"{"text":"contract"}"#)
            .await
            .expect("a call returns a result");
        assert!(result.success, "{label}: {:?}", result.error);
        assert!(
            result.output.contains("contract"),
            "{label}: {}",
            result.output
        );

        provider.shutdown().await;
    }
}

#[tokio::test]
async fn every_provider_kind_reaps_its_children_on_shutdown() {
    let dir = scratch("shutdown");

    for provider in providers(&dir).await {
        let label = provider.label();
        // The fixtures write their pid before the handshake completes, so it
        // is present once the provider started.
        let pid: u32 = std::fs::read_to_string(provider.pid_file(&dir))
            .unwrap_or_else(|error| panic!("{label}: the fixture wrote no pid: {error}"))
            .trim()
            .parse()
            .expect("the fixture pid is a number");

        provider.shutdown().await;
        assert!(
            process_gone(pid).await,
            "{label}: the fixture process {pid} is still running"
        );
    }
}
