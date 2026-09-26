use super::{auth, protocol::JsonRpcResponse, LinkChannels};
use crate::types::HyuskEvent;
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use rustls::{
    pki_types::{
        CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, PrivateSec1KeyDer,
    },
    ServerConfig,
};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap, fs, io, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration,
};
use thiserror::Error;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, Mutex, Semaphore},
    task::JoinHandle,
    time::timeout,
};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::{
    accept_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
};

const MAX_MESSAGE: usize = 64 * 1024;
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum LinkServerError {
    #[error("invalid link configuration: {0}")]
    Config(String),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("TLS configuration error: {0}")]
    Tls(String),
    #[error("authentication error: {0}")]
    Auth(#[from] auth::AuthError),
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("device action timed out before the phone reported a result")]
    InvocationTimedOut,
    #[error("device disconnected before it reported the action result")]
    InvocationDisconnected,
}

#[derive(Debug, Clone)]
pub struct LinkServerConfig {
    pub bind_addr: SocketAddr,
    pub advertise_host: String,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub state_path: PathBuf,
    pub max_clients: usize,
}

impl LinkServerConfig {
    /// TLS is mandatory.  There is deliberately no plaintext fallback for a
    /// service that accepts a credential and can invoke laptop actions.
    pub fn from_env() -> Result<Self, LinkServerError> {
        let bind_addr = std::env::var("HYUSK_LINK_BIND")
            .unwrap_or_else(|_| "127.0.0.1:4488".into())
            .parse()
            .map_err(|e| LinkServerError::Config(format!("HYUSK_LINK_BIND: {e}")))?;
        let cert_path = std::env::var_os("HYUSK_LINK_TLS_CERT")
            .ok_or_else(|| LinkServerError::Config("HYUSK_LINK_TLS_CERT is required".into()))?
            .into();
        let key_path = std::env::var_os("HYUSK_LINK_TLS_KEY")
            .ok_or_else(|| LinkServerError::Config("HYUSK_LINK_TLS_KEY is required".into()))?
            .into();
        Ok(Self {
            bind_addr,
            advertise_host: std::env::var("HYUSK_LINK_ADVERTISE_HOST")
                .unwrap_or_else(|_| "127.0.0.1".into()),
            cert_path,
            key_path,
            state_path: auth::DeviceStore::default_path(),
            max_clients: std::env::var("HYUSK_LINK_MAX_CLIENTS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4)
                .clamp(1, 32),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LinkInvocation {
    pub invocation_id: String,
    pub device_id: String,
    pub action: String,
    pub arguments: Value,
    pub risk: String,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone)]
struct ClientMap(Arc<Mutex<HashMap<String, mpsc::Sender<OutboundFrame>>>>);

/// Results are routed by the random invocation id rather than through the
/// application event loop.  That keeps a tool call tied to the phone action
/// that it actually requested, even while other phone events are arriving.
#[derive(Debug, Clone, Default)]
struct PendingInvocations(Arc<Mutex<HashMap<String, PendingInvocation>>>);

#[derive(Debug)]
struct PendingInvocation {
    device_id: String,
    result: oneshot::Sender<super::InvocationResult>,
}

impl PendingInvocations {
    async fn insert(
        &self,
        invocation_id: String,
        device_id: String,
        result: oneshot::Sender<super::InvocationResult>,
    ) {
        self.0
            .lock()
            .await
            .insert(invocation_id, PendingInvocation { device_id, result });
    }

    async fn remove(&self, invocation_id: &str) {
        self.0.lock().await.remove(invocation_id);
    }

    /// Returns false for late, duplicate, or cross-device results.  Those
    /// results are harmless and must never complete another phone's action.
    async fn resolve(&self, device_id: &str, value: super::InvocationResult) -> bool {
        let pending = self.0.lock().await.remove(&value.invocation_id);
        match pending {
            Some(pending) if pending.device_id == device_id => pending.result.send(value).is_ok(),
            Some(pending) => {
                self.0.lock().await.insert(value.invocation_id, pending);
                false
            }
            None => false,
        }
    }

    async fn disconnect_device(&self, device_id: &str) {
        self.0
            .lock()
            .await
            .retain(|_, pending| pending.device_id != device_id);
    }
}

#[derive(Debug, Clone, Serialize)]
struct OutboundFrame {
    jsonrpc: &'static str,
    method: String,
    params: Value,
}

impl From<super::OutboundNotification> for OutboundFrame {
    fn from(value: super::OutboundNotification) -> Self {
        Self {
            jsonrpc: super::JSONRPC,
            method: value.method,
            params: value.params,
        }
    }
}

/// Handle for a running laptop link server.
pub struct LinkServer {
    local_addr: SocketAddr,
    pairing_url: String,
    tls_fingerprint: String,
    pairing: Arc<std::sync::RwLock<auth::PairingState>>,
    devices: Arc<auth::DeviceStore>,
    clients: ClientMap,
    pending: PendingInvocations,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for LinkServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkServer")
            .field("local_addr", &self.local_addr)
            .finish()
    }
}

impl LinkServer {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
    pub fn pairing_payload(&self) -> super::PairingPayload {
        let pairing = self.pairing.read().expect("pairing lock poisoned");
        super::PairingPayload {
            protocol: super::PROTOCOL.into(),
            url: self.pairing_url.clone(),
            tls_fingerprint: self.tls_fingerprint.clone(),
            secret: pairing.secret(),
            expires_at: pairing.expires_at(),
        }
    }

    pub fn refresh_pairing(&self) -> super::PairingPayload {
        *self.pairing.write().expect("pairing lock poisoned") = auth::PairingState::new();
        self.pairing_payload()
    }
    pub async fn devices(&self) -> Vec<auth::DeviceInfo> {
        self.devices.list().await
    }

    /// Send a structured action request to one authenticated phone.  The
    /// phone replies on the normal JSON-RPC stream with `agent.invoke.result`.
    pub async fn invoke(
        &self,
        device_id: &str,
        action: impl Into<String>,
        arguments: Value,
        risk: impl Into<String>,
        timeout_ms: u64,
    ) -> Result<super::InvocationResult, LinkServerError> {
        let invocation_id = format!("inv-{}", auth::encode_secret(&auth::random_secret()[..12]));
        let action = action.into();
        let risk = risk.into();
        let timeout_ms = timeout_ms.clamp(1_000, 60_000);
        let clients = self.clients.0.lock().await;
        let sender = clients
            .get(device_id)
            .ok_or_else(|| LinkServerError::Config("device is not connected".into()))?
            .clone();
        drop(clients);
        let (result_tx, result_rx) = oneshot::channel();
        self.pending
            .insert(invocation_id.clone(), device_id.to_string(), result_tx)
            .await;
        let frame = OutboundFrame {
            jsonrpc: super::JSONRPC,
            method: super::METHOD_INVOKE.into(),
            params: serde_json::to_value(LinkInvocation {
                invocation_id: invocation_id.clone(),
                device_id: device_id.into(),
                action,
                arguments,
                risk,
                timeout_ms,
            })
            .unwrap(),
        };
        if let Err(error) = timeout(SEND_TIMEOUT, sender.send(frame))
            .await
            .map_err(|_| LinkServerError::Config("client send timed out".into()))
            .and_then(|result| {
                result.map_err(|_| LinkServerError::Config("client disconnected".into()))
            })
        {
            self.pending.remove(&invocation_id).await;
            return Err(error);
        }

        match timeout(Duration::from_millis(timeout_ms), result_rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => Err(LinkServerError::InvocationDisconnected),
            Err(_) => {
                self.pending.remove(&invocation_id).await;
                Err(LinkServerError::InvocationTimedOut)
            }
        }
    }

    pub async fn shutdown(mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

/// Start a TLS WebSocket server.  Certificate and private-key paths come from
/// `HYUSK_LINK_TLS_CERT` and `HYUSK_LINK_TLS_KEY` (or an explicit config).
pub async fn start_link_server(channels: LinkChannels) -> Result<LinkServer, LinkServerError> {
    start_link_server_with_config(channels, LinkServerConfig::from_env()?).await
}

pub async fn start_link_server_with_config(
    channels: LinkChannels,
    config: LinkServerConfig,
) -> Result<LinkServer, LinkServerError> {
    let (tls, tls_fingerprint) = load_tls_config(&config.cert_path, &config.key_path)?;
    let listener = TcpListener::bind(config.bind_addr).await?;
    let local_addr = listener.local_addr()?;
    let devices = Arc::new(auth::DeviceStore::load(config.state_path.clone())?);
    let pairing = Arc::new(std::sync::RwLock::new(auth::PairingState::new()));
    let clients = ClientMap(Arc::new(Mutex::new(HashMap::new())));
    let pending = PendingInvocations::default();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
    let acceptor = TlsAcceptor::from(tls);
    let semaphore = Arc::new(Semaphore::new(config.max_clients));
    let task_clients = clients.clone();
    let task_pending = pending.clone();
    let task_devices = devices.clone();
    let task_pairing = pairing.clone();
    let task_events = channels.events.clone();
    let task_commands = channels.commands.clone();
    let mut outbound = channels.outbound.subscribe();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                event = outbound.recv() => {
                    if let Ok(notification) = event {
                        let frame: OutboundFrame = notification.into();
                        let senders: Vec<_> = task_clients.0.lock().await.values().cloned().collect();
                        for sender in senders { let _ = timeout(SEND_TIMEOUT, sender.send(frame.clone())).await; }
                    }
                }
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    let Ok(permit) = semaphore.clone().try_acquire_owned() else { continue };
                    let acceptor = acceptor.clone(); let clients = task_clients.clone(); let pending = task_pending.clone(); let devices = task_devices.clone(); let pairing = task_pairing.clone(); let events = task_events.clone(); let commands = task_commands.clone();
                    tokio::spawn(async move { let _permit = permit; if let Ok(Ok(stream)) = timeout(AUTH_TIMEOUT, acceptor.accept(stream)).await { let _ = connection(stream, clients, pending, devices, pairing, events, commands).await; } });
                }
            }
        }
    });
    Ok(LinkServer {
        local_addr,
        pairing_url: format!("wss://{}:{}/link", config.advertise_host, local_addr.port()),
        tls_fingerprint,
        pairing,
        devices,
        clients,
        pending,
        shutdown: Some(shutdown_tx),
        task: Some(task),
    })
}

async fn connection(
    stream: tokio_rustls::server::TlsStream<TcpStream>,
    clients: ClientMap,
    pending: PendingInvocations,
    devices: Arc<auth::DeviceStore>,
    pairing: Arc<std::sync::RwLock<auth::PairingState>>,
    events: mpsc::Sender<HyuskEvent>,
    commands: mpsc::Sender<super::LinkCommand>,
) -> Result<(), LinkServerError> {
    let ws_config = WebSocketConfig {
        max_message_size: Some(MAX_MESSAGE),
        max_frame_size: Some(MAX_MESSAGE),
        ..Default::default()
    };
    let mut socket = accept_async_with_config(stream, Some(ws_config)).await?;
    let challenge = auth::random_secret();
    let challenge_text = auth::encode_secret(&challenge);
    send_frame(
        &mut socket,
        OutboundFrame {
            jsonrpc: super::JSONRPC,
            method: super::METHOD_CHALLENGE.into(),
            params: serde_json::to_value(super::AuthChallenge {
                protocol: super::PROTOCOL.into(),
                challenge: challenge_text.clone(),
                expires_at: auth::unix_now() + AUTH_TIMEOUT.as_secs(),
            })
            .unwrap(),
        },
    )
    .await?;
    let Some(Ok(Message::Text(hello_text))) =
        timeout(AUTH_TIMEOUT, socket.next()).await.ok().flatten()
    else {
        return Err(LinkServerError::Config("hello timeout".into()));
    };
    if hello_text.len() > MAX_MESSAGE {
        return Err(LinkServerError::Config("frame too large".into()));
    }
    let hello_request: super::JsonRpcRequest = serde_json::from_str(&hello_text)
        .map_err(|e| LinkServerError::Config(format!("invalid hello: {e}")))?;
    if hello_request.method != super::METHOD_HELLO {
        return Err(LinkServerError::Config("device.hello required".into()));
    }
    let hello: super::DeviceHello = hello_request
        .parse_params()
        .map_err(|e| LinkServerError::Config(e.message))?;
    if hello.protocol != super::PROTOCOL || hello.challenge != challenge_text {
        return Err(LinkServerError::Config(
            "protocol/challenge mismatch".into(),
        ));
    }
    let device_secret = auth::decode_secret(&hello.device_secret)?;
    if !auth::verify_proof(&device_secret, &challenge, &hello.device_id, &hello.proof) {
        return Err(LinkServerError::Config("invalid challenge proof".into()));
    }
    let current_pairing = pairing.read().expect("pairing lock poisoned").clone();
    devices
        .authenticate(
            &hello.device_id,
            &device_secret,
            hello.pairing_secret.as_deref(),
            &current_pairing,
        )
        .await?;
    let (sender, mut receiver) = mpsc::channel::<OutboundFrame>(16);
    let mut connected = clients.0.lock().await;
    if connected.contains_key(&hello.device_id) {
        return Err(LinkServerError::Config("device already connected".into()));
    }
    connected.insert(hello.device_id.clone(), sender);
    drop(connected);
    send_response(
        &mut socket,
        JsonRpcResponse::result(
            hello_request.id,
            json!({"ok": true, "protocol": super::PROTOCOL}),
        ),
    )
    .await?;
    let device_id = hello.device_id;
    let mut sequence = 0u64;
    loop {
        tokio::select! {
            frame = receiver.recv() => { let Some(frame) = frame else { break }; send_frame(&mut socket, frame).await?; }
            incoming = timeout(IDLE_TIMEOUT, socket.next()) => {
                let incoming = incoming.map_err(|_| LinkServerError::Config("idle timeout".into()))?;
                let Some(message) = incoming else { break };
                let message = message?;
                let Message::Text(text) = message else { continue };
                if text.len() > MAX_MESSAGE { return Err(LinkServerError::Config("frame too large".into())); }
                let request: super::JsonRpcRequest = match serde_json::from_str(&text) { Ok(r) => r, Err(e) => { send_response(&mut socket, JsonRpcResponse::error(Value::Null, super::JsonRpcError::parse(e.to_string()))).await?; continue; } };
                if request.jsonrpc != super::JSONRPC { send_response(&mut socket, JsonRpcResponse::error(request.id, super::JsonRpcError::invalid_request("jsonrpc must be 2.0"))).await?; continue; }
                let Some(request_auth) = request.auth.as_ref() else { send_response(&mut socket, JsonRpcResponse::error(request.id, super::JsonRpcError::unauthorized("request authentication required"))).await?; continue; };
                if request_auth.sequence <= sequence || !verify_request_mac(&request, &device_secret) { send_response(&mut socket, JsonRpcResponse::error(request.id, super::JsonRpcError::unauthorized("invalid or replayed request"))).await?; continue; }
                sequence = request_auth.sequence;
                let response = handle_request(request, &device_id, &pending, &events, &commands).await;
                send_response(&mut socket, response).await?;
            }
        }
    }
    clients.0.lock().await.remove(&device_id);
    pending.disconnect_device(&device_id).await;
    Ok(())
}

async fn handle_request(
    request: super::JsonRpcRequest,
    device_id: &str,
    pending: &PendingInvocations,
    events: &mpsc::Sender<HyuskEvent>,
    commands: &mpsc::Sender<super::LinkCommand>,
) -> super::JsonRpcResponse {
    let id = request.id.clone();
    let result: Result<Value, super::JsonRpcError> = match request.method.as_str() {
        super::METHOD_SUBMIT => {
            let value = match request.parse_params::<super::TurnSubmitRequest>() {
                Ok(value) => value,
                Err(error) => return super::JsonRpcResponse::error(id, error),
            };
            let send = timeout(SEND_TIMEOUT, events.send(HyuskEvent::UserInput(value.text))).await;
            if send.is_err() || send.unwrap().is_err() {
                Err(super::JsonRpcError::unavailable(
                    "event channel unavailable",
                ))
            } else {
                Ok(json!({"accepted": true, "turn_id": value.turn_id}))
            }
        }
        super::METHOD_CANCEL => {
            let value = match request.parse_params::<super::TurnCancelRequest>() {
                Ok(value) => value,
                Err(error) => return super::JsonRpcResponse::error(id, error),
            };
            let send = timeout(SEND_TIMEOUT, events.send(HyuskEvent::StopRequested)).await;
            if send.is_err() || send.unwrap().is_err() {
                Err(super::JsonRpcError::unavailable(
                    "event channel unavailable",
                ))
            } else {
                Ok(json!({"accepted": true, "turn_id": value.turn_id}))
            }
        }
        super::METHOD_INVOKE | super::METHOD_INVOKE_LEGACY => {
            send_command(
                request
                    .parse_params::<super::InvokeRequest>()
                    .map(super::LinkCommand::Invoke),
                commands,
            )
            .await
        }
        super::METHOD_INVOKE_RESULT => {
            let value = match request.parse_params::<super::InvocationResult>() {
                Ok(value) => value,
                Err(error) => return super::JsonRpcResponse::error(id, error),
            };
            let resolved = pending.resolve(device_id, value.clone()).await;
            // Status reporting must never make a completed phone action look
            // unacknowledged to the phone.  The result has already been
            // correlated above; this is best-effort UI telemetry only.
            let _ = commands.try_send(super::LinkCommand::InvocationResult(value));
            Ok(json!({"accepted": true, "resolved": resolved}))
        }
        super::METHOD_APPROVAL | super::METHOD_APPROVAL_LEGACY => {
            send_command(
                request
                    .parse_params::<super::ApprovalResponse>()
                    .map(super::LinkCommand::Approval),
                commands,
            )
            .await
        }
        super::METHOD_MEMORY_SYNC => {
            send_command(
                request.parse_params().map(super::LinkCommand::MemorySync),
                commands,
            )
            .await
        }
        super::METHOD_WORKFLOW_SYNC => {
            send_command(
                request.parse_params().map(super::LinkCommand::WorkflowSync),
                commands,
            )
            .await
        }
        super::METHOD_PING => Ok(json!({"pong": true})),
        super::METHOD_SUBSCRIBE => Ok(json!({"subscribed": true})),
        _ => Err(super::JsonRpcError::method_not_found()),
    };
    match result {
        Ok(value) => super::JsonRpcResponse::result(id, value),
        Err(error) => super::JsonRpcResponse::error(id, error),
    }
}

async fn send_command(
    command: Result<super::LinkCommand, super::JsonRpcError>,
    commands: &mpsc::Sender<super::LinkCommand>,
) -> Result<Value, super::JsonRpcError> {
    let command = command?;
    timeout(SEND_TIMEOUT, commands.send(command))
        .await
        .map_err(|_| super::JsonRpcError::unavailable("command channel unavailable"))?
        .map_err(|_| super::JsonRpcError::unavailable("command channel unavailable"))?;
    Ok(json!({"accepted": true}))
}

fn verify_request_mac(request: &super::JsonRpcRequest, secret: &[u8]) -> bool {
    let Some(auth) = request.auth.as_ref() else {
        return false;
    };
    let expected = auth::request_mac(secret, &request.signing_bytes(auth.sequence));
    auth::constant_time_str_eq(&expected, &auth.mac)
}

async fn send_response<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    response: super::JsonRpcResponse,
) -> Result<(), LinkServerError> {
    send_text(socket, serde_json::to_string(&response).unwrap()).await
}
async fn send_frame<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    frame: OutboundFrame,
) -> Result<(), LinkServerError> {
    send_text(socket, serde_json::to_string(&frame).unwrap()).await
}
async fn send_text<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    text: String,
) -> Result<(), LinkServerError> {
    timeout(SEND_TIMEOUT, socket.send(Message::Text(text)))
        .await
        .map_err(|_| LinkServerError::Config("write timeout".into()))??;
    Ok(())
}

fn load_tls_config(
    cert_path: &PathBuf,
    key_path: &PathBuf,
) -> Result<(Arc<ServerConfig>, String), LinkServerError> {
    let certs = pem_blocks(&fs::read(cert_path)?, "CERTIFICATE")
        .into_iter()
        .map(CertificateDer::from)
        .collect::<Vec<_>>();
    if certs.is_empty() {
        return Err(LinkServerError::Tls("certificate file is empty".into()));
    }
    let key_bytes = fs::read(key_path)?;
    let key = if let Some(bytes) = pem_blocks(&key_bytes, "PRIVATE KEY").into_iter().next() {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(bytes))
    } else if let Some(bytes) = pem_blocks(&key_bytes, "RSA PRIVATE KEY").into_iter().next() {
        PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(bytes))
    } else if let Some(bytes) = pem_blocks(&key_bytes, "EC PRIVATE KEY").into_iter().next() {
        PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(bytes))
    } else {
        return Err(LinkServerError::Tls(
            "private key file is empty or unsupported".into(),
        ));
    };
    let fingerprint = certs
        .first()
        .map(|cert| {
            let digest = Sha256::digest(cert.as_ref());
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(":")
        })
        .unwrap_or_default();
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| LinkServerError::Tls(e.to_string()))?;
    Ok((Arc::new(config), fingerprint))
}

fn pem_blocks(data: &[u8], label: &str) -> Vec<Vec<u8>> {
    let text = String::from_utf8_lossy(data);
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    text.split(&begin)
        .skip(1)
        .filter_map(|part| part.split(&end).next())
        .filter_map(|body| {
            STANDARD
                .decode(
                    body.lines()
                        .filter(|line| !line.contains(':'))
                        .collect::<String>(),
                )
                .ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_requires_tls_material() {
        std::env::remove_var("HYUSK_LINK_TLS_CERT");
        std::env::remove_var("HYUSK_LINK_TLS_KEY");
        assert!(LinkServerConfig::from_env().is_err());
    }

    #[tokio::test]
    async fn pending_result_is_correlated_to_the_originating_device_once() {
        let pending = PendingInvocations::default();
        let (sender, receiver) = oneshot::channel();
        pending
            .insert("inv-1".into(), "phone-a".into(), sender)
            .await;
        let result = super::super::InvocationResult {
            invocation_id: "inv-1".into(),
            success: true,
            output: json!({"opened": true}),
            error: None,
        };

        assert!(!pending.resolve("phone-b", result.clone()).await);
        assert!(pending.resolve("phone-a", result.clone()).await);
        assert_eq!(receiver.await.unwrap(), result);
        assert!(!pending.resolve("phone-a", result).await);
    }
}
