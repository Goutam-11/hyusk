use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL: &str = "hyusk.link.v1";
pub const JSONRPC: &str = "2.0";

pub const METHOD_HELLO: &str = "device.hello";
pub const METHOD_SUBMIT: &str = "turn.submit";
pub const METHOD_CANCEL: &str = "turn.cancel";
pub const METHOD_INVOKE: &str = "device.invoke";
pub const METHOD_INVOKE_RESULT: &str = "device.invoke.result";
pub const METHOD_APPROVAL: &str = "approval.resolve";
pub const METHOD_INVOKE_LEGACY: &str = "agent.invoke";
pub const METHOD_APPROVAL_LEGACY: &str = "approval.respond";
pub const METHOD_MEMORY_SYNC: &str = "memory.sync";
pub const METHOD_WORKFLOW_SYNC: &str = "workflow.sync";
pub const METHOD_PING: &str = "device.ping";
pub const METHOD_SUBSCRIBE: &str = "event.subscribe";
pub const METHOD_CHALLENGE: &str = "auth.challenge";
pub const METHOD_EVENT: &str = "event";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub auth: Option<RequestAuth>,
}

impl JsonRpcRequest {
    #[cfg(test)]
    pub fn new(id: impl Into<Value>, method: impl Into<String>, params: impl Serialize) -> Self {
        Self {
            jsonrpc: JSONRPC.to_string(),
            id: id.into(),
            method: method.into(),
            params: serde_json::to_value(params).unwrap_or(Value::Null),
            auth: None,
        }
    }

    pub fn parse_params<T: DeserializeOwned>(&self) -> Result<T, JsonRpcError> {
        serde_json::from_value(self.params.clone())
            .map_err(|error| JsonRpcError::invalid_params(error.to_string()))
    }

    /// Stable bytes covered by the per-request MAC.  `auth` is deliberately
    /// excluded so a client cannot authenticate one request as another.
    pub fn signing_bytes(&self, sequence: u64) -> Vec<u8> {
        serde_json::to_vec(&(JSONRPC, &self.id, &self.method, &self.params, sequence))
            .expect("JSON-RPC request fields are serializable")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestAuth {
    pub sequence: u64,
    pub mac: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn result(id: Value, value: impl Serialize) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            id,
            result: serde_json::to_value(value).ok(),
            error: None,
        }
    }

    pub fn error(id: Value, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    pub fn parse(message: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: message.into(),
            data: None,
        }
    }
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: message.into(),
            data: None,
        }
    }
    pub fn method_not_found() -> Self {
        Self {
            code: -32601,
            message: "method not found".into(),
            data: None,
        }
    }
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: None,
        }
    }
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            code: -32001,
            message: message.into(),
            data: None,
        }
    }
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: -32002,
            message: message.into(),
            data: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DeviceCapabilities {
    #[serde(default)]
    pub audio_input: bool,
    #[serde(default)]
    pub notifications: bool,
    #[serde(default)]
    pub approvals: bool,
    #[serde(default)]
    pub memory_sync: bool,
    #[serde(default)]
    pub workflow_sync: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceHello {
    pub protocol: String,
    pub device_id: String,
    pub device_name: String,
    #[serde(default)]
    pub capabilities: DeviceCapabilities,
    pub challenge: String,
    pub proof: String,
    /// Sent only during pairing.  A random per-device secret is retained by
    /// the phone and used for subsequent challenge responses.
    #[serde(default)]
    pub pairing_secret: Option<String>,
    pub device_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnSubmitRequest {
    pub turn_id: String,
    pub text: String,
    #[serde(default)]
    pub workflow: Option<String>,
    #[serde(default)]
    pub source_device: Option<String>,
    #[serde(default)]
    pub target_device: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnCancelRequest {
    pub turn_id: String,
    #[serde(default)]
    pub source_device: Option<String>,
    #[serde(default)]
    pub target_device: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvokeRequest {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default)]
    pub risk: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvocationResult {
    pub invocation_id: String,
    pub success: bool,
    #[serde(default)]
    pub output: Value,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApprovalResponse {
    pub request_id: String,
    pub approved: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MemorySyncRequest {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub items: Vec<MemoryItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryItem {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WorkflowSyncRequest {
    #[serde(default)]
    pub revision: Option<u64>,
    #[serde(default)]
    pub workflows: Vec<WorkflowItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowItem {
    pub id: String,
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthChallenge {
    pub protocol: String,
    pub challenge: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairingPayload {
    pub protocol: String,
    pub url: String,
    pub tls_fingerprint: String,
    pub secret: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutboundNotification {
    pub method: String,
    pub params: Value,
}

impl OutboundNotification {
    pub fn event(params: impl Serialize) -> Self {
        Self {
            method: METHOD_EVENT.into(),
            params: serde_json::to_value(params).unwrap_or(Value::Null),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_signing_excludes_auth_and_round_trips() {
        let mut request = JsonRpcRequest::new(
            7,
            METHOD_SUBMIT,
            TurnSubmitRequest {
                turn_id: "turn-1".into(),
                text: "hello".into(),
                workflow: None,
                source_device: None,
                target_device: None,
                timeout_ms: None,
                idempotency_key: None,
            },
        );
        let before = request.signing_bytes(1);
        request.auth = Some(RequestAuth {
            sequence: 1,
            mac: "different".into(),
        });
        assert_eq!(before, request.signing_bytes(1));
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: JsonRpcRequest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.method, METHOD_SUBMIT);
    }

    #[test]
    fn protocol_types_use_jsonrpc_two() {
        let response = JsonRpcResponse::result(Value::from(1), serde_json::json!({"ok": true}));
        assert_eq!(response.jsonrpc, JSONRPC);
        assert_eq!(response.result.unwrap()["ok"], true);
    }
}
