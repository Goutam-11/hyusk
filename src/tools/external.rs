//! External tool host (T2).
//!
//! One process per configured server, speaking newline-delimited JSON-RPC 2.0
//! over stdio. The host spawns the server, performs one `initialize` handshake
//! that carries the server's name and its tools, registers each tool under
//! `ext.<server>.<tool>`, and forwards calls. The server's claims are data, not
//! policy: the host validates arguments against the declared schema before
//! sending them, truncates and redacts results, and the approval gate in
//! [`crate::tools::safety`] asks for every external call.
//!
//! Failure is contained. A server that will not start, crashes, or misses its
//! deadline is logged and skipped; the rest of the registry and the agent keep
//! running, and the next call starts a fresh process. Shutdown kills and reaps
//! every child, because a process spawned here would otherwise outlive the
//! agent.
//!
//! The protocol, in one paragraph (documented in `docs/tools.md`): the host
//! writes one JSON object per line. `initialize` carries
//! `{"protocolVersion":"1.0","client":{...}}` and the server answers with
//! `{"server":{"name","version"},"tools":[{"name","description","inputSchema"}]}`.
//! `tools/call` carries `{"name","arguments"}` and the server answers with
//! MCP-shaped `{"content":[{"type":"text","text":…}],"isError"?:bool}`.
//!
//! Configuration is one environment variable, [`ENV_VAR`]: either the path to
//! a JSON file or the JSON array itself. Servers are probed once at startup,
//! which is also where a broken one is skipped.

use std::{
    collections::HashSet,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
    sync::Mutex,
    time::timeout,
};

use super::manifest::{self, RiskClass, Schema, Source, ToolManifest, CORE_VERSION};
use super::registry::ToolRegistry;
use super::safety::EXTERNAL_PREFIX;
use super::{redact, Tool, ToolResult};

/// The handshake protocol version of the local (T2) server kind.
pub const PROTOCOL_VERSION: &str = "1.0";

/// The Model Context Protocol revision this client implements.
///
/// The 2026-07-28 revision is stateless: every request carries its version and
/// client metadata in `params._meta`, so there is no initialize handshake. The
/// newest initialize-based revision is used only to detect and serve a legacy
/// server (T3).
pub const MCP_PROTOCOL_VERSION: &str = "2026-07-28";
/// The newest initialize-based MCP revision, for a legacy server.
pub const MCP_LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";
/// The most `tools/list` pages one server may page through.
const MAX_LIST_PAGES: usize = 8;

/// The environment variable naming the server configuration.
pub const ENV_VAR: &str = "HYUSK_EXTERNAL_SERVERS";

/// The default per-call deadline.
pub const DEFAULT_CALL_TIMEOUT_MS: u64 = 10_000;
/// The shortest per-call deadline a configuration may ask for.
pub const MIN_CALL_TIMEOUT_MS: u64 = 100;
/// The longest per-call deadline a configuration may ask for.
pub const MAX_CALL_TIMEOUT_MS: u64 = 120_000;

/// The default deadline for the spawn and handshake.
pub const DEFAULT_STARTUP_TIMEOUT_MS: u64 = 5_000;
/// The shortest startup deadline a configuration may ask for.
pub const MIN_STARTUP_TIMEOUT_MS: u64 = 100;
/// The longest startup deadline a configuration may ask for.
pub const MAX_STARTUP_TIMEOUT_MS: u64 = 30_000;

/// The most result text one external call contributes to the context.
///
/// The same order as the `shell` tool's cap: a server's chatty output is a
/// cost in every later request, not a feature.
pub const MAX_RESULT_CHARS: usize = 20_000;

/// The largest argument object a call may carry.
pub const MAX_ARGUMENT_BYTES: usize = 64 * 1024;

/// The longest protocol line the host reads.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// The most servers one configuration may list.
pub const MAX_SERVERS: usize = 16;

/// The most tools one server may expose.
pub const MAX_TOOLS_PER_SERVER: usize = 64;

/// The longest namespaced tool name; model providers cap function names here.
pub const MAX_NAMESPACED_NAME: usize = 64;

/// The most nested objects or arrays the argument check walks.
const MAX_SCHEMA_DEPTH: usize = 8;

/// The largest server name or tool name segment.
const MAX_NAME_SEGMENT: usize = 32;

/// How a server speaks on its stdio pipes.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServerKind {
    /// The T2 host protocol: the tools arrive in the initialize result.
    #[default]
    Local,
    /// The Model Context Protocol (T3): `server/discover`, or a legacy
    /// `initialize` for a server that predates per-request metadata, then
    /// `tools/list`.
    Mcp,
}

/// One configured external server.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ServerSpec {
    /// The short name used in the `ext.<server>.<tool>` namespace.
    pub name: String,
    /// The program to start.
    pub command: String,
    /// The program's arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// The per-call deadline in milliseconds; clamped to the documented range.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// The spawn-and-handshake deadline in milliseconds; clamped too.
    #[serde(default)]
    pub startup_timeout_ms: Option<u64>,
    /// The wire protocol the server speaks.
    #[serde(default)]
    pub kind: ServerKind,
    /// The child's environment allowlist. `None` inherits the agent's
    /// environment (the T2 local default); `Some` clears it and passes `PATH`
    /// plus the named variables only (an MCP server, T3).
    #[serde(default)]
    pub env: Option<Vec<String>>,
    /// The tools this server may expose. `None` offers every discovered tool;
    /// `Some` offers only the named ones. A grant can only narrow the set -
    /// the host still asks for approval on every call.
    #[serde(default)]
    pub allow: Option<Vec<String>>,
}

impl ServerSpec {
    /// The per-call deadline this server is given.
    pub fn call_timeout(&self) -> Duration {
        Duration::from_millis(
            self.timeout_ms
                .unwrap_or(DEFAULT_CALL_TIMEOUT_MS)
                .clamp(MIN_CALL_TIMEOUT_MS, MAX_CALL_TIMEOUT_MS),
        )
    }

    /// The spawn-and-handshake deadline this server is given.
    pub fn startup_timeout(&self) -> Duration {
        Duration::from_millis(
            self.startup_timeout_ms
                .unwrap_or(DEFAULT_STARTUP_TIMEOUT_MS)
                .clamp(MIN_STARTUP_TIMEOUT_MS, MAX_STARTUP_TIMEOUT_MS),
        )
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.kind == ServerKind::Mcp {
            for name in self.env.iter().flatten() {
                if name.trim().is_empty() || name.contains('=') {
                    bail!(
                        "the server {:?} has an unusable environment name {name:?}",
                        self.name
                    );
                }
            }
            for name in self.allow.iter().flatten() {
                if !valid_tool_segment(name) {
                    bail!(
                        "the server {:?} allows {name:?}, which is not a tool name",
                        self.name
                    );
                }
            }
        }
        if !valid_segment(&self.name) {
            bail!(
                "the server name {:?} must be 1-{MAX_NAME_SEGMENT} letters, digits, '-' or '_' \
                 and start with a letter or digit",
                self.name
            );
        }
        if self.command.trim().is_empty() {
            bail!("the server {:?} names no command", self.name);
        }
        Ok(())
    }
}

/// Load the server list from [`ENV_VAR`]'s value: inline JSON or a file path.
pub fn load_specs(value: &str) -> Result<Vec<ServerSpec>> {
    let text = if value.trim_start().starts_with('[') {
        value.to_string()
    } else {
        std::fs::read_to_string(value)
            .with_context(|| format!("reading the external server config {value:?}"))?
    };
    let specs: Vec<ServerSpec> = serde_json::from_str(&text)
        .context("the external server config must be a JSON array of {name, command, args}")?;
    if specs.is_empty() {
        bail!("the external server config lists no servers");
    }
    if specs.len() > MAX_SERVERS {
        bail!("at most {MAX_SERVERS} external servers are supported");
    }
    let mut names = HashSet::new();
    for spec in &specs {
        spec.validate()?;
        if !names.insert(spec.name.as_str()) {
            bail!("two external servers are named {:?}", spec.name);
        }
    }
    Ok(specs)
}

/// Every started external server. Dropping it stops nothing by itself: a
/// process exit skips destructors, so the composition root calls
/// [`ExternalHost::shutdown`] before it exits.
pub struct ExternalHost {
    servers: Vec<Arc<ExternalServer>>,
}

impl ExternalHost {
    /// Probe and start each configured server. A server that fails is logged
    /// and left out; this never fails the agent.
    pub async fn start(specs: Vec<ServerSpec>) -> Self {
        let mut servers: Vec<Arc<ExternalServer>> = Vec::new();
        let mut names = HashSet::new();

        for spec in specs.into_iter().take(MAX_SERVERS) {
            if !names.insert(spec.name.clone()) {
                eprintln!("[External] Skipping a second server named {:?}", spec.name);
                continue;
            }
            match ExternalServer::start(spec).await {
                Ok(server) => servers.push(Arc::new(server)),
                Err(error) => {
                    let (message, _) = redact::redact(&format!("{error:#}"));
                    eprintln!("[External] Server unavailable: {message}");
                }
            }
        }

        Self { servers }
    }

    /// Start the servers named by [`ENV_VAR`], or `None` when there are none.
    ///
    /// A broken configuration or an empty server set is disabled, not fatal: a
    /// typo in an opt-in variable must not stop the assistant.
    pub async fn start_from_env() -> Option<Arc<Self>> {
        let value = std::env::var(ENV_VAR).ok()?;
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        let specs = match load_specs(value) {
            Ok(specs) => specs,
            Err(error) => {
                let (message, _) = redact::redact(&format!("{error:#}"));
                eprintln!("[External] Configuration ignored: {message}");
                return None;
            }
        };
        let host = Self::start(specs).await;
        if host.servers.is_empty() {
            return None;
        }
        Some(Arc::new(host))
    }

    /// Add every external tool to the registry under its namespaced name.
    ///
    /// A declaration the manifest checks refuse is skipped with a log line, so
    /// one malformed tool from a server does not cost the others.
    pub fn register_into(&self, registry: &mut ToolRegistry) {
        for server in &self.servers {
            for declaration in &server.tools {
                let tool = server.external_tool(declaration);
                if let Err(error) = tool.manifest.conformance() {
                    eprintln!(
                        "[External] {} was not registered: {error}",
                        declaration.namespaced
                    );
                    continue;
                }
                if registry.contains(tool.name()) {
                    eprintln!(
                        "[External] {} is already registered; skipping",
                        declaration.namespaced
                    );
                    continue;
                }
                registry.register(tool);
            }
        }
    }

    /// Stop one server and mark its tools unavailable (T3). A removed MCP
    /// server stops being offered on the next turn without a restart.
    pub async fn deactivate_server(&self, name: &str) -> bool {
        for server in &self.servers {
            if server.spec.name == name {
                server.deactivate().await;
                return true;
            }
        }
        false
    }

    /// The configured server names, in start order.
    pub fn server_names(&self) -> Vec<String> {
        self.servers
            .iter()
            .map(|server| server.spec.name.clone())
            .collect()
    }

    /// Kill and reap every child. Safe to call more than once.
    pub async fn shutdown(&self) {
        for server in &self.servers {
            server.shutdown().await;
        }
    }

    /// How many servers answered.
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    /// How many tools were declared by those servers.
    pub fn tool_count(&self) -> usize {
        self.servers.iter().map(|server| server.tools.len()).sum()
    }
}

/// One live server: its configuration, the handshake's tools, and its child.
struct ExternalServer {
    spec: ServerSpec,
    tools: Vec<ToolDeclaration>,
    runtime: Mutex<Option<Connection>>,
    /// False once the server leaves the configuration (T3). Its tools stay in
    /// the registry's manifest set but are no longer offered or callable.
    active: AtomicBool,
}

/// One tool a server declared during the handshake.
#[derive(Clone, Debug)]
struct ToolDeclaration {
    /// The server-side name, used in `tools/call`.
    name: String,
    /// The namespaced name the model sees.
    namespaced: String,
    /// The single-line, bounded description the model sees.
    description: String,
    /// The argument schema the server declared.
    schema: Value,
    /// The server version, normalised to `major.minor.patch`.
    version: String,
}

impl ExternalServer {
    async fn start(spec: ServerSpec) -> Result<Self> {
        spec.validate()?;
        let connection = Connection::start(&spec).await?;
        let handshake = connection.handshake.clone();

        if handshake.server != spec.name {
            // A server that names itself something else could impersonate
            // another server's namespace, so the host refuses it.
            bail!(
                "the server configured as {:?} named itself {:?}",
                spec.name,
                handshake.server
            );
        }

        let tools = declarations(&spec, &handshake);
        Ok(Self {
            spec,
            tools,
            runtime: Mutex::new(Some(connection)),
            active: AtomicBool::new(true),
        })
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    /// Mark the server gone and stop its child. Safe to call twice.
    async fn deactivate(&self) {
        self.active.store(false, Ordering::SeqCst);
        let mut guard = self.runtime.lock().await;
        if let Some(connection) = guard.take() {
            connection.terminate().await;
        }
    }

    fn external_tool(self: &Arc<Self>, declaration: &ToolDeclaration) -> ExternalTool {
        let manifest = ToolManifest {
            name: leak_str(declaration.namespaced.clone()),
            version: leak_str(declaration.version.clone()),
            description: leak_str(declaration.description.clone()),
            schema: Schema::Owned(leak_value(declaration.schema.clone())),
            risk: &[RiskClass::Execute],
            // The host does not let a server claim a desktop capability; the
            // D3/D6 tables stay the only place that grants one.
            capabilities: &[],
            source: Source::External,
        };
        ExternalTool {
            server: Arc::clone(self),
            tool_name: declaration.name.clone(),
            manifest,
        }
    }

    /// Call one tool, restarting the server first if it is not running.
    async fn call(&self, tool_name: &str, arguments: &Value) -> Result<String, String> {
        if !self.is_active() {
            return Err(format!(
                "the {:?} server is no longer in the configuration",
                self.spec.name
            ));
        }
        let mut guard = self.runtime.lock().await;

        // A process that exited since the last call is replaced, not reused.
        if guard.as_mut().is_some_and(Connection::exited) {
            if let Some(dead) = guard.take() {
                dead.terminate().await;
            }
        }

        if guard.is_none() {
            let connection = self.spawn().await?;
            if !connection.handshake.has_tool(tool_name) {
                let message = format!(
                    "the {:?} server restarted without the {tool_name:?} tool",
                    self.spec.name
                );
                connection.terminate().await;
                return Err(message);
            }
            *guard = Some(connection);
        }

        let outcome = {
            let connection = guard
                .as_mut()
                .expect("the connection was just ensured to exist");
            let id = connection.next_id();
            let request = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": connection.handshake.call_params(tool_name, arguments),
            });
            timeout(self.spec.call_timeout(), connection.request(id, request)).await
        };

        match outcome {
            Ok(Ok(result)) => {
                let (text, is_error) = result_text(&result);
                if is_error {
                    Err(text)
                } else {
                    Ok(text)
                }
            }
            Ok(Err(error)) => {
                self.terminate(&mut guard).await;
                Err(format!("the {:?} server failed: {error:#}", self.spec.name))
            }
            Err(_) => {
                self.terminate(&mut guard).await;
                Err(format!(
                    "the {:?} server did not answer within {} ms and was restarted",
                    self.spec.name,
                    self.spec.call_timeout().as_millis()
                ))
            }
        }
    }

    async fn spawn(&self) -> Result<Connection, String> {
        Connection::start(&self.spec)
            .await
            .map_err(|error| format!("the {:?} server could not start: {error:#}", self.spec.name))
    }

    async fn terminate(&self, guard: &mut Option<Connection>) {
        if let Some(connection) = guard.take() {
            connection.terminate().await;
        }
    }

    async fn shutdown(&self) {
        self.active.store(false, Ordering::SeqCst);
        let mut guard = self.runtime.lock().await;
        if let Some(connection) = guard.take() {
            connection.terminate().await;
        }
    }
}

/// One tool a server exposed, bound to its server.
struct ExternalTool {
    server: Arc<ExternalServer>,
    tool_name: String,
    manifest: ToolManifest,
}

#[async_trait]
impl Tool for ExternalTool {
    fn manifest(&self) -> ToolManifest {
        self.manifest
    }

    fn is_available(&self) -> bool {
        self.server.is_active()
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let arguments = match parse_arguments(input) {
            Ok(arguments) => arguments,
            Err(message) => {
                return Ok(ToolResult::failure(format!(
                    "Invalid arguments for {}: {message}",
                    self.name()
                )))
            }
        };

        if let Err(message) = validate_arguments(&self.manifest.schema.build(), &arguments) {
            return Ok(ToolResult::failure(format!(
                "Invalid arguments for {}: {message}",
                self.name()
            )));
        }

        match self.server.call(&self.tool_name, &arguments).await {
            Ok(text) => {
                let mut output = truncate_result(&text);
                let (redacted, replaced) = redact::redact(&output);
                output = redacted;
                if replaced > 0 {
                    output.push_str(&redact::note(replaced));
                }
                Ok(ToolResult::success(output))
            }
            Err(message) => Ok(ToolResult::failure(message)),
        }
    }
}

/// How the handshake described the server.
#[derive(Clone, Debug, Default)]
struct Handshake {
    server: String,
    version: String,
    tools: Vec<ServerTool>,
    protocol: WireProtocol,
}

impl Handshake {
    fn has_tool(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tool.name == name)
    }

    /// The `tools/call` parameters for this server's era (T3).
    fn call_params(&self, tool_name: &str, arguments: &Value) -> Value {
        match &self.protocol {
            WireProtocol::McpModern { version } => json!({
                "name": tool_name,
                "arguments": arguments,
                "_meta": modern_meta(version),
            }),
            _ => json!({ "name": tool_name, "arguments": arguments }),
        }
    }
}

/// Which wire protocol answered the handshake.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum WireProtocol {
    /// The T2 local protocol.
    #[default]
    Local,
    /// MCP 2026-07-28 and later: stateless, with per-request `_meta`.
    McpModern { version: String },
    /// MCP 2025-11-25 and earlier: an initialize handshake.
    McpLegacy { version: String },
}

/// One tool in the handshake.
#[derive(Clone, Debug)]
struct ServerTool {
    name: String,
    description: String,
    schema: Value,
}

/// One spawned server process and its protocol state.
struct Connection {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    handshake: Handshake,
}

/// A JSON-RPC failure that still carries the server's error code.
///
/// The MCP era probe needs the code: -32022 identifies a modern server that
/// speaks another revision, while any other error identifies a legacy server
/// that must be served with an initialize handshake (T3).
#[derive(Debug)]
enum RpcError {
    Transport(anyhow::Error),
    Json {
        code: i64,
        message: String,
        data: Value,
    },
}

impl RpcError {
    fn into_anyhow(self) -> anyhow::Error {
        match self {
            RpcError::Transport(error) => error,
            RpcError::Json {
                code,
                message,
                data,
            } => {
                let detail = if data.is_null() || data == Value::Object(Default::default()) {
                    String::new()
                } else {
                    format!(" ({data})")
                };
                anyhow!("the external server returned error {code}: {message}{detail}")
            }
        }
    }
}

/// The per-request metadata the 2026-07-28 MCP revision requires.
fn modern_meta(version: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientInfo": { "name": "hyusk", "version": CORE_VERSION },
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

/// What the MCP era probe learned.
enum Probe {
    Modern {
        version: String,
        server: String,
        server_version: String,
    },
    Legacy,
}

impl Connection {
    async fn start(spec: &ServerSpec) -> Result<Self> {
        let mut command = tokio::process::Command::new(&spec.command);
        command
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The server's diagnostics are not protocol; discarding them keeps
            // a chatty server from filling the caller's terminal.
            .stderr(Stdio::null())
            .kill_on_drop(true);

        // An MCP server gets only PATH and the variables the operator allowed
        // (T3). A local server keeps the T2 default of inheriting the agent's
        // environment.
        if let Some(allow) = &spec.env {
            command.env_clear();
            if let Some(path) = std::env::var_os("PATH") {
                command.env("PATH", path);
            }
            for name in allow {
                if let Some(value) = std::env::var_os(name) {
                    command.env(name, value);
                }
            }
        }

        let mut child = command.spawn().with_context(|| {
            format!(
                "starting the external server {:?} ({:?})",
                spec.name, spec.command
            )
        })?;
        let stdin = child
            .stdin
            .take()
            .context("the external server has no stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("the external server has no stdout")?;

        let mut connection = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 0,
            handshake: Handshake::default(),
        };
        connection.handshake = match spec.kind {
            ServerKind::Local => connection.initialize_local(spec.startup_timeout()).await?,
            ServerKind::Mcp => {
                connection
                    .initialize_mcp(&spec.name, spec.startup_timeout())
                    .await?
            }
        };
        Ok(connection)
    }

    fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    async fn initialize_local(&mut self, deadline: Duration) -> Result<Handshake> {
        let id = self.next_id();
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "client": { "name": "hyusk", "version": CORE_VERSION },
            },
        });
        let result = timeout(deadline, self.request(id, request))
            .await
            .map_err(|_| {
                anyhow!(
                    "the handshake did not finish within {} ms",
                    deadline.as_millis()
                )
            })??;
        parse_handshake(&result)
    }

    /// Detect the MCP era and discover the server's tools (T3).
    ///
    /// The 2026-07-28 revision is stateless: `server/discover` must answer. A
    /// legacy server answers that request with any other error, or not at all,
    /// and is then served with the initialize handshake it expects. A modern
    /// server that speaks only another revision is reported with its list; the
    /// late probe answer is skipped by request id if a timeout falls through.
    async fn initialize_mcp(&mut self, configured: &str, deadline: Duration) -> Result<Handshake> {
        match self.mcp_probe(deadline).await? {
            Probe::Modern {
                version,
                server,
                server_version,
            } => {
                let tools = self.list_tools_modern(&version, deadline).await?;
                Ok(Handshake {
                    server: if server.is_empty() {
                        configured.to_string()
                    } else {
                        server
                    },
                    version: server_version,
                    tools,
                    protocol: WireProtocol::McpModern { version },
                })
            }
            Probe::Legacy => {
                let (server, server_version) = self.initialize_mcp_legacy(deadline).await?;
                self.notify(json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized",
                }))
                .await?;
                let tools = self.list_tools_legacy(deadline).await?;
                Ok(Handshake {
                    server: if server.is_empty() {
                        configured.to_string()
                    } else {
                        server
                    },
                    version: server_version,
                    tools,
                    protocol: WireProtocol::McpLegacy {
                        version: MCP_LEGACY_PROTOCOL_VERSION.to_string(),
                    },
                })
            }
        }
    }

    async fn mcp_probe(&mut self, deadline: Duration) -> Result<Probe> {
        let id = self.next_id();
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "server/discover",
            "params": { "_meta": modern_meta(MCP_PROTOCOL_VERSION) },
        });
        let result = timeout(deadline, self.request_raw(id, request)).await;
        match result {
            Ok(Ok(result)) => {
                let supported: Vec<String> = result
                    .get("supportedVersions")
                    .and_then(Value::as_array)
                    .map(|versions| {
                        versions
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if !supported.is_empty()
                    && !supported
                        .iter()
                        .any(|version| version == MCP_PROTOCOL_VERSION)
                {
                    bail!(
                        "the MCP server supports {supported:?}; this client speaks {MCP_PROTOCOL_VERSION:?}"
                    );
                }
                let server = result
                    .pointer("/_meta/io.modelcontextprotocol/serverInfo/name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let server_version = result
                    .pointer("/_meta/io.modelcontextprotocol/serverInfo/version")
                    .and_then(Value::as_str)
                    .unwrap_or("0.0.0")
                    .to_string();
                Ok(Probe::Modern {
                    version: MCP_PROTOCOL_VERSION.to_string(),
                    server,
                    server_version,
                })
            }
            Ok(Err(RpcError::Json {
                code: -32022, data, ..
            })) => {
                let supported: Vec<String> = data
                    .get("supported")
                    .and_then(Value::as_array)
                    .map(|versions| {
                        versions
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                bail!("the MCP server rejected {MCP_PROTOCOL_VERSION:?}; it offers {supported:?}")
            }
            // A legacy server rejects the modern request, stays silent, or
            // closes the pipe; the initialize handshake is the fallback.
            Ok(Err(_)) | Err(_) => Ok(Probe::Legacy),
        }
    }

    async fn initialize_mcp_legacy(&mut self, deadline: Duration) -> Result<(String, String)> {
        let id = self.next_id();
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_LEGACY_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "hyusk", "version": CORE_VERSION },
            },
        });
        let result = timeout(deadline, self.request(id, request))
            .await
            .map_err(|_| {
                anyhow!(
                    "the MCP initialize did not finish within {} ms",
                    deadline.as_millis()
                )
            })??;
        let server = result
            .pointer("/serverInfo/name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let version = result
            .pointer("/serverInfo/version")
            .and_then(Value::as_str)
            .unwrap_or("0.0.0")
            .to_string();
        Ok((server, version))
    }

    async fn list_tools_modern(
        &mut self,
        version: &str,
        deadline: Duration,
    ) -> Result<Vec<ServerTool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;

        for _ in 0..MAX_LIST_PAGES {
            let id = self.next_id();
            let mut params = json!({ "_meta": modern_meta(version) });
            if let Some(cursor) = &cursor {
                params["cursor"] = Value::String(cursor.clone());
            }
            let request = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/list",
                "params": params,
            });
            let result = timeout(deadline, self.request(id, request))
                .await
                .map_err(|_| {
                    anyhow!(
                        "the MCP tools/list did not answer within {} ms",
                        deadline.as_millis()
                    )
                })??;
            collect_listed_tools(&result, &mut tools)?;
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if cursor.is_none() || tools.len() >= MAX_TOOLS_PER_SERVER {
                break;
            }
        }

        Ok(tools)
    }

    async fn list_tools_legacy(&mut self, deadline: Duration) -> Result<Vec<ServerTool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;

        for _ in 0..MAX_LIST_PAGES {
            let id = self.next_id();
            let mut params = json!({});
            if let Some(cursor) = &cursor {
                params["cursor"] = Value::String(cursor.clone());
            }
            let request = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/list",
                "params": params,
            });
            let result = timeout(deadline, self.request(id, request))
                .await
                .map_err(|_| {
                    anyhow!(
                        "the MCP tools/list did not answer within {} ms",
                        deadline.as_millis()
                    )
                })??;
            collect_listed_tools(&result, &mut tools)?;
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if cursor.is_none() || tools.len() >= MAX_TOOLS_PER_SERVER {
                break;
            }
        }

        Ok(tools)
    }

    async fn notify(&mut self, value: Value) -> Result<()> {
        let mut line = serde_json::to_vec(&value).context("serialising a protocol note")?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .context("writing to the external server")?;
        self.stdin
            .flush()
            .await
            .context("flushing to the external server")?;
        Ok(())
    }

    async fn request(&mut self, id: u64, value: Value) -> Result<Value> {
        self.request_raw(id, value)
            .await
            .map_err(RpcError::into_anyhow)
    }

    async fn request_raw(&mut self, id: u64, value: Value) -> Result<Value, RpcError> {
        let mut line =
            serde_json::to_vec(&value).map_err(|error| RpcError::Transport(error.into()))?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .map_err(|error| RpcError::Transport(error.into()))?;
        self.stdin
            .flush()
            .await
            .map_err(|error| RpcError::Transport(error.into()))?;
        self.read_response_raw(id).await
    }

    async fn read_response_raw(&mut self, id: u64) -> Result<Value, RpcError> {
        loop {
            let Some(line) = self.read_line().await.map_err(RpcError::Transport)? else {
                return Err(RpcError::Transport(anyhow!(
                    "the external server closed its output"
                )));
            };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                // A server that prints a banner or a log line is not speaking;
                // skip it rather than treating it as the answer.
                continue;
            };
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
                    .to_string();
                let data = error.get("data").cloned().unwrap_or(Value::Null);
                return Err(RpcError::Json {
                    code,
                    message,
                    data,
                });
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Read one bounded line, so a server cannot exhaust memory with a line
    /// that never ends.
    async fn read_line(&mut self) -> Result<Option<String>> {
        let mut line = Vec::new();
        loop {
            let available = self
                .stdout
                .fill_buf()
                .await
                .context("reading from the external server")?;
            if available.is_empty() {
                if line.is_empty() {
                    return Ok(None);
                }
                bail!("the external server ended a line early");
            }
            if let Some(end) = available.iter().position(|byte| *byte == b'\n') {
                line.extend_from_slice(&available[..end]);
                self.stdout.consume(end + 1);
                if line.len() > MAX_LINE_BYTES {
                    bail!("the external server sent a protocol line over {MAX_LINE_BYTES} bytes");
                }
                return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
            }
            line.extend_from_slice(available);
            let consumed = available.len();
            self.stdout.consume(consumed);
            if line.len() > MAX_LINE_BYTES {
                bail!("the external server sent a protocol line over {MAX_LINE_BYTES} bytes");
            }
        }
    }

    /// Kill and reap the child. Reaping matters: an unwaited child is a
    /// zombie that keeps its pid visible.
    async fn terminate(mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.start_kill();
        }
        let _ = self.child.wait().await;
    }
}

fn parse_handshake(result: &Value) -> Result<Handshake> {
    if let Some(version) = result.get("protocolVersion") {
        let version = version.as_str().unwrap_or("");
        if version != PROTOCOL_VERSION {
            bail!("the server offers protocol {version:?}, not {PROTOCOL_VERSION:?}");
        }
    }

    let server = result
        .get("server")
        .and_then(|server| server.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if server.trim().is_empty() {
        bail!("the handshake does not name the server");
    }

    let version = result
        .get("server")
        .and_then(|server| server.get("version"))
        .and_then(Value::as_str)
        .unwrap_or("0.0.0")
        .to_string();

    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .context("the handshake has no tools array")?;

    if tools.len() > MAX_TOOLS_PER_SERVER {
        bail!(
            "the server declares {} tools, over the {MAX_TOOLS_PER_SERVER} limit",
            tools.len()
        );
    }

    let mut seen = HashSet::new();
    let mut parsed = Vec::new();
    for tool in tools {
        let Some(parsed_tool) = parse_listed_tool(tool) else {
            continue;
        };
        if !seen.insert(parsed_tool.name.clone()) {
            // An unusable or duplicate name is one skipped tool, not a failed
            // server: the other tools still work.
            continue;
        }
        parsed.push(parsed_tool);
    }

    Ok(Handshake {
        server,
        version,
        tools: parsed,
        protocol: WireProtocol::Local,
    })
}

/// One `tools/list` tool, bounded and with a usable name.
fn parse_listed_tool(tool: &Value) -> Option<ServerTool> {
    let name = tool.get("name").and_then(Value::as_str)?;
    if !valid_tool_segment(name) {
        return None;
    }
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let schema = tool
        .get("inputSchema")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object"}));
    Some(ServerTool {
        name: name.to_string(),
        description: description.to_string(),
        schema,
    })
}

/// Append one `tools/list` page, dropping duplicates and the overflow.
fn collect_listed_tools(result: &Value, tools: &mut Vec<ServerTool>) -> Result<()> {
    let page = result
        .get("tools")
        .and_then(Value::as_array)
        .context("the MCP tools/list result has no tools array")?;
    for tool in page {
        if tools.len() >= MAX_TOOLS_PER_SERVER {
            break;
        }
        if let Some(parsed) = parse_listed_tool(tool) {
            if !tools.iter().any(|known| known.name == parsed.name) {
                tools.push(parsed);
            }
        }
    }
    Ok(())
}

fn declarations(spec: &ServerSpec, handshake: &Handshake) -> Vec<ToolDeclaration> {
    let version = if manifest::valid_version(&handshake.version) {
        handshake.version.clone()
    } else {
        // A server is free to call its version anything; the manifest shape
        // requires major.minor.patch, so an unknown version is `0.0.0`.
        "0.0.0".to_string()
    };
    let curated = spec.kind == ServerKind::Mcp;

    handshake
        .tools
        .iter()
        .filter_map(|tool| {
            if let Some(allow) = &spec.allow {
                if !allow.iter().any(|allowed| allowed == &tool.name) {
                    eprintln!(
                        "[External] Skipping {}.{}: it is not in the server's allow list",
                        spec.name, tool.name
                    );
                    return None;
                }
            }
            let namespaced = format!("{EXTERNAL_PREFIX}{}.{}", spec.name, tool.name);
            if namespaced.len() > MAX_NAMESPACED_NAME {
                eprintln!("[External] Skipping {namespaced}: the name is too long");
                return None;
            }
            Some(ToolDeclaration {
                name: tool.name.clone(),
                namespaced,
                description: if curated {
                    curate_description(&spec.name, &tool.name, &tool.schema)
                } else {
                    sanitise_description(&tool.description, &tool.name, &spec.name)
                },
                schema: tool.schema.clone(),
                version: version.clone(),
            })
        })
        .collect()
}

/// The model-facing description of an MCP tool (T3).
///
/// A server's own description is untrusted text, and its annotations are only
/// hints (the MCP specification says clients must not trust them from an
/// untrusted server), so the host writes the sentence from the tool name and
/// the schema it validated. The text always says whether the tool reads or
/// changes data, and it never promises that a call is free.
fn curate_description(server: &str, tool: &str, schema: &Value) -> String {
    let tokens: Vec<String> = tool
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();

    let read_verbs = [
        "list", "get", "read", "status", "diff", "log", "show", "search", "fetch", "find", "query",
        "describe", "inspect", "stat", "view",
    ];
    let write_verbs = [
        "delete",
        "remove",
        "write",
        "create",
        "move",
        "rename",
        "commit",
        "push",
        "exec",
        "run",
        "send",
        "update",
        "set",
        "add",
        "install",
        "uninstall",
        "truncate",
        "drop",
        "wipe",
        "reset",
        "replace",
        "upload",
    ];

    let reads = tokens
        .iter()
        .any(|token| read_verbs.contains(&token.as_str()));
    let writes = tokens
        .iter()
        .any(|token| write_verbs.contains(&token.as_str()));

    let mut text = if writes {
        format!(
            "Changes data: {tool} on the {server} MCP server. The host asks for approval              before every call."
        )
    } else if reads {
        format!(
            "Read-only {tool} on the {server} MCP server. Its description was written by              Hyusk; the server's own text is not shown."
        )
    } else {
        format!(
            "{tool} on the {server} MCP server. Its description was written by Hyusk              because a server's text is untrusted, so the host asks for approval before              every call."
        )
    };

    let arguments: Vec<String> = schema
        .pointer("/properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .keys()
                .take(8)
                .map(|name| format!("`{name}`"))
                .collect()
        })
        .unwrap_or_default();
    if !arguments.is_empty() {
        text.push_str(&format!(" Arguments: {}.", arguments.join(", ")));
    }
    text
}

/// Reduce a server's description to one bounded line.
fn sanitise_description(raw: &str, tool: &str, server: &str) -> String {
    let normalised = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalised.is_empty() {
        return format!("External tool {tool} from the {server} server.");
    }
    let mut characters: Vec<char> = normalised.chars().collect();
    if characters.len() > manifest::MAX_DESCRIPTION_CHARS {
        characters.truncate(manifest::MAX_DESCRIPTION_CHARS - 1);
        characters.push('…');
    }
    characters.into_iter().collect()
}

/// Parse and bound the model's arguments.
fn parse_arguments(input: &str) -> Result<Value, String> {
    let trimmed = input.trim();
    if trimmed.len() > MAX_ARGUMENT_BYTES {
        return Err(format!(
            "The arguments are over the {MAX_ARGUMENT_BYTES}-byte limit."
        ));
    }
    let value = if trimmed.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(trimmed)
            .map_err(|error| format!("The arguments are not valid JSON: {error}"))?
    };
    if !value.is_object() {
        return Err("The arguments must be a JSON object.".to_string());
    }
    Ok(value)
}

/// Check arguments against the server's declared schema, for the keywords that
/// decide whether a call is well formed.
///
/// This is a deliberately small validator: the host needs to refuse a call the
/// schema says is wrong before it reaches the server, not to implement every
/// JSON Schema keyword. An unknown type or a too-deep schema fails closed.
fn validate_arguments(schema: &Value, arguments: &Value) -> Result<(), String> {
    validate_value(schema, arguments, "$", 0)
}

fn validate_value(schema: &Value, value: &Value, path: &str, depth: usize) -> Result<(), String> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(format!("{path} nests too deeply to check"));
    }
    let Some(object) = schema.as_object() else {
        // A schema that is not an object cannot constrain the value; the
        // manifest check refuses such a tool before it registers.
        return Ok(());
    };

    if let Some(kind) = object.get("type") {
        let types: Vec<&str> = match kind {
            Value::String(kind) => vec![kind.as_str()],
            Value::Array(kinds) => kinds.iter().filter_map(Value::as_str).collect(),
            _ => return Err(format!("{path} declares an unusable type")),
        };
        if !types.iter().any(|kind| type_matches(kind, value)) {
            return Err(format!(
                "{path} must be {}, not {}",
                types.join(" or "),
                kind_name(value)
            ));
        }
    }

    if let Some(allowed) = object.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            return Err(format!("{path} is not one of the declared values"));
        }
    }

    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        if let Some(required) = object.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if value.get(name).is_none() {
                    return Err(format!("{path} is missing the required argument {name:?}"));
                }
            }
        }
        if let Some(map) = value.as_object() {
            for (key, item) in map {
                match properties.get(key) {
                    Some(property) => {
                        validate_value(property, item, &format!("{path}.{key}"), depth + 1)?
                    }
                    None if object.get("additionalProperties").and_then(Value::as_bool)
                        == Some(false) =>
                    {
                        return Err(format!("{path} has the unknown argument {key:?}"));
                    }
                    None => {}
                }
            }
        }
    }

    if let Some(items) = object.get("items") {
        if let Some(array) = value.as_array() {
            for (index, item) in array.iter().enumerate() {
                validate_value(items, item, &format!("{path}[{index}]"), depth + 1)?;
            }
        }
    }

    Ok(())
}

fn type_matches(kind: &str, value: &Value) -> bool {
    match kind {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        // An unknown declared type is a declaration the host cannot check, so
        // the call is refused rather than waved through.
        _ => false,
    }
}

fn kind_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The text an external call returns, and whether the server marked it failed.
fn result_text(result: &Value) -> (String, bool) {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut parts = Vec::new();
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for item in content {
            if item.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
        }
    }
    if parts.is_empty() {
        parts.push(serde_json::to_string(result).unwrap_or_else(|_| "{}".to_string()));
    }
    (parts.join("\n"), is_error)
}

/// Bound a result the way `shell` bounds a command's output.
fn truncate_result(text: &str) -> String {
    if text.chars().count() <= MAX_RESULT_CHARS {
        return text.to_string();
    }
    let truncated: String = text.chars().take(MAX_RESULT_CHARS).collect();
    format!("{truncated}\n... (output truncated)")
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_NAME_SEGMENT
        && value
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
}

fn valid_tool_segment(value: &str) -> bool {
    valid_segment(value) && value.len() <= MAX_NAME_SEGMENT + 16
}

/// The manifest fields are `&'static str` by design (T1), and a server's
/// declarations live for the process lifetime, so the host leaks one small
/// string per field once. The server set is fixed at startup, so this is
/// bounded by the configuration, not by call traffic.
fn leak_str(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn leak_value(value: Value) -> &'static Value {
    Box::leak(Box::new(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "hyusk-t2-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("scratch directory");
        path
    }

    fn spec(name: &str, mode: &str, dir: &Path, timeout_ms: u64) -> ServerSpec {
        ServerSpec {
            name: name.to_string(),
            command: "sh".to_string(),
            args: vec![
                format!(
                    "{}/tests/fixtures/external_server.sh",
                    env!("CARGO_MANIFEST_DIR")
                ),
                mode.to_string(),
                dir.join("calls.log").to_string_lossy().into_owned(),
                dir.join("state").to_string_lossy().into_owned(),
                dir.join("pid").to_string_lossy().into_owned(),
            ],
            timeout_ms: Some(timeout_ms),
            startup_timeout_ms: Some(5_000),
            kind: ServerKind::Local,
            env: None,
            allow: None,
        }
    }

    async fn host_with(specs: Vec<ServerSpec>) -> Arc<ExternalHost> {
        Arc::new(ExternalHost::start(specs).await)
    }

    fn registry_with(host: &ExternalHost) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        host.register_into(&mut registry);
        registry
    }

    fn read_pid(dir: &Path) -> Option<u32> {
        std::fs::read_to_string(dir.join("pid"))
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
    async fn a_fixture_server_exposes_a_tool_the_agent_can_call() {
        let dir = scratch("happy");
        let host = host_with(vec![spec("fixture", "happy", &dir, 2_000)]).await;
        assert_eq!(host.server_count(), 1);
        assert_eq!(host.tool_count(), 1);

        let registry = registry_with(&host);
        let manifest = registry
            .manifests()
            .into_iter()
            .find(|manifest| manifest.name == "ext.fixture.echo")
            .expect("the fixture tool is registered");
        assert_eq!(manifest.source, Source::External);
        assert_eq!(manifest.version, "1.2.3");
        assert!(manifest.description.starts_with("Echo the text argument"));

        // The model-facing spec comes from the handshake, not a second copy.
        let specs = registry.specs();
        let spec = specs
            .iter()
            .find(|spec| spec["function"]["name"] == "ext.fixture.echo")
            .expect("the fixture tool has a spec");
        assert_eq!(spec["function"]["parameters"]["required"][0], "text");

        // The registry set agrees with the gate: one always-ask namespace row.
        assert_eq!(registry.conformance(), Ok(()));

        let tool = registry
            .get("ext.fixture.echo")
            .expect("the fixture tool is callable");
        let result = tool
            .execute(r#"{"text":"hello"}"#)
            .await
            .expect("execution returns a result");
        assert!(result.success, "{:?}", result.error);
        assert_eq!(result.output, "echo: hello");

        // Every external call passes through the host approval policy.
        assert!(crate::tools::safety::requires_approval(
            "ext.fixture.echo",
            r#"{"text":"hello"}"#
        ));
        host.shutdown().await;
    }

    #[tokio::test]
    async fn a_crashing_server_does_not_take_down_the_agent() {
        let dir = scratch("crash");
        let host = host_with(vec![spec("fixture", "crash", &dir, 2_000)]).await;
        let registry = registry_with(&host);
        let tool = registry
            .get("ext.fixture.echo")
            .expect("the fixture tool is registered");

        // Two calls: the first crash must not poison the host, and the second
        // failure is still a result rather than an error or a panic.
        for _ in 0..2 {
            let result = tool
                .execute(r#"{"text":"hello"}"#)
                .await
                .expect("a crash is reported, not raised");
            assert!(!result.success);
            assert!(
                result
                    .error
                    .as_deref()
                    .unwrap_or("")
                    .contains("closed its output"),
                "{:?}",
                result.error
            );
        }

        // The rest of the registry keeps working alongside the broken server.
        let healthy = host_with(vec![spec("fixture", "happy", &dir, 2_000)]).await;
        let healthy_registry = registry_with(&healthy);
        let echo = healthy_registry
            .get("ext.fixture.echo")
            .expect("the healthy server's tool is registered");
        assert!(echo.execute(r#"{"text":"hi"}"#).await.unwrap().success);

        host.shutdown().await;
        healthy.shutdown().await;
    }

    #[tokio::test]
    async fn a_slow_server_times_out_and_is_restarted() {
        let dir = scratch("timeout");
        let host = host_with(vec![spec("fixture", "timeout", &dir, 300)]).await;
        let registry = registry_with(&host);
        let tool = registry.get("ext.fixture.echo").expect("registered");

        let started = std::time::Instant::now();
        let result = tool
            .execute(r#"{"text":"hello"}"#)
            .await
            .expect("a timeout is a result");
        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or("")
                .contains("did not answer within 300 ms"),
            "{:?}",
            result.error
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the call waited {:?}",
            started.elapsed()
        );
        host.shutdown().await;
    }

    #[tokio::test]
    async fn an_oversized_result_is_truncated() {
        let dir = scratch("oversized");
        let host = host_with(vec![spec("fixture", "oversized", &dir, 2_000)]).await;
        let registry = registry_with(&host);
        let tool = registry.get("ext.fixture.echo").expect("registered");

        let result = tool.execute(r#"{"text":"hello"}"#).await.unwrap();
        assert!(result.success, "{:?}", result.error);
        assert!(
            result.output.chars().count() <= MAX_RESULT_CHARS + 100,
            "the result is {} characters",
            result.output.chars().count()
        );
        assert!(result.output.contains("output truncated"));
        host.shutdown().await;
    }

    #[tokio::test]
    async fn arguments_are_validated_before_the_server_sees_them() {
        let dir = scratch("validate");
        let host = host_with(vec![spec("fixture", "happy", &dir, 2_000)]).await;
        let registry = registry_with(&host);
        let tool = registry.get("ext.fixture.echo").expect("registered");

        for input in [
            r#"{"text":5}"#,
            r#"{}"#,
            r#"{"text":"hi","extra":1}"#,
            "not json",
        ] {
            let result = tool.execute(input).await.expect("a refusal is a result");
            assert!(!result.success, "{input} must be refused");
            assert!(
                result
                    .error
                    .as_deref()
                    .unwrap_or("")
                    .contains("Invalid arguments"),
                "{input}: {:?}",
                result.error
            );
        }

        let log = std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default();
        assert!(
            !log.contains("tools/call"),
            "the server saw a call it should never have received: {log}"
        );
        host.shutdown().await;
    }

    #[tokio::test]
    async fn a_server_that_died_while_idle_is_restarted() {
        let dir = scratch("idle");
        let host = host_with(vec![spec("fixture", "idle-once", &dir, 2_000)]).await;
        let registry = registry_with(&host);
        let tool = registry.get("ext.fixture.echo").expect("registered");

        // The first fixture answers the handshake and exits, so the next call
        // must notice the dead child before it writes to a closed pipe.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let result = tool.execute(r#"{"text":"hello"}"#).await.unwrap();
        assert!(result.success, "{:?}", result.error);
        assert_eq!(result.output, "echo: hello");
        host.shutdown().await;
    }

    #[tokio::test]
    async fn a_crashed_server_is_restarted_on_the_next_call() {
        let dir = scratch("restart");
        let host = host_with(vec![spec("fixture", "crash-once", &dir, 2_000)]).await;
        let registry = registry_with(&host);
        let tool = registry.get("ext.fixture.echo").expect("registered");

        let first = tool.execute(r#"{"text":"hello"}"#).await.unwrap();
        assert!(!first.success, "{:?}", first.output);

        let second = tool.execute(r#"{"text":"hello"}"#).await.unwrap();
        assert!(second.success, "{:?}", second.error);
        assert_eq!(second.output, "echo: hello");
        host.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_leaves_no_orphan_process() {
        let dir = scratch("shutdown");
        let host = host_with(vec![spec("fixture", "happy", &dir, 2_000)]).await;
        let registry = registry_with(&host);
        let tool = registry.get("ext.fixture.echo").expect("registered");
        assert!(tool.execute(r#"{"text":"hello"}"#).await.unwrap().success);

        let pid = read_pid(&dir).expect("the fixture wrote its pid");
        host.shutdown().await;
        assert!(
            process_gone(pid).await,
            "the fixture process {pid} is still running"
        );
    }

    #[tokio::test]
    async fn a_server_that_will_not_start_is_skipped() {
        let dir = scratch("nostart");
        let mut broken = spec("broken", "happy", &dir, 1_000);
        broken.command = "/nonexistent/hyusk-t2-fixture".to_string();

        let host = host_with(vec![broken, spec("fixture", "happy", &dir, 2_000)]).await;
        assert_eq!(host.server_count(), 1);
        assert_eq!(host.tool_count(), 1);
        host.shutdown().await;
    }

    #[tokio::test]
    async fn a_server_that_names_itself_differently_is_skipped() {
        let dir = scratch("mismatch");
        let host = host_with(vec![spec("other", "happy", &dir, 2_000)]).await;
        assert_eq!(host.server_count(), 0);
        assert_eq!(host.tool_count(), 0);
        host.shutdown().await;
    }

    #[tokio::test]
    async fn a_secret_shaped_result_is_redacted() {
        let dir = scratch("secret");
        let host = host_with(vec![spec("fixture", "secret", &dir, 2_000)]).await;
        let registry = registry_with(&host);
        let tool = registry.get("ext.fixture.echo").expect("registered");

        let result = tool.execute(r#"{"text":"hello"}"#).await.unwrap();
        assert!(result.success, "{:?}", result.error);
        assert!(!result.output.contains("sk-or-v1-0123456789abcdef"));
        assert!(result.output.contains("[redacted]"));
        host.shutdown().await;
    }

    #[test]
    fn the_config_is_bounded_and_rejects_bad_names() {
        assert!(load_specs(r#"[{"name":"fixture","command":"sh"}]"#).is_ok());
        for config in [
            "[]",
            r#"[{"name":"bad name","command":"sh"}]"#,
            r#"[{"name":"ok","command":""}]"#,
            r#"[{"name":"ok","command":"sh"},{"name":"ok","command":"sh"}]"#,
        ] {
            assert!(load_specs(config).is_err(), "{config} must be refused");
        }
    }
}
