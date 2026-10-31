use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{net::IpAddr, time::Duration};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::{Tool, ToolResult};
use crate::tools::manifest::{Capability, RiskClass, Source, ToolManifest, CORE_VERSION};

const DEFAULT_CDP_URL: &str = "http://127.0.0.1:9222";
const CDP_TIMEOUT: Duration = Duration::from_secs(12);
const READ_TIMEOUT: Duration = Duration::from_secs(5);

pub struct BrowserTool {
    client: Client,
    endpoint: Url,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum BrowserAction {
    Status,
    Start,
    NewTab,
    Tabs,
    Read {
        target_id: String,
    },
    Navigate {
        target_id: String,
        url: String,
    },
    Click {
        target_id: String,
        selector: String,
    },
    Fill {
        target_id: String,
        selector: String,
        text: String,
    },
    PressKey {
        target_id: String,
        key: String,
    },
}

#[derive(Debug, Deserialize)]
struct Target {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    title: String,
    url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    websocket: Option<String>,
}

impl BrowserTool {
    pub fn new() -> Result<Self> {
        let raw_endpoint =
            std::env::var("HYUSK_BROWSER_CDP_URL").unwrap_or_else(|_| DEFAULT_CDP_URL.to_owned());
        let endpoint = Url::parse(&raw_endpoint).context("HYUSK_BROWSER_CDP_URL must be a URL")?;
        if !is_loopback_url(&endpoint) || endpoint.scheme() != "http" {
            bail!(
                "Browser CDP endpoint must be an http URL on loopback (127.0.0.1, ::1, or localhost)."
            );
        }
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .build()
            .context("Could not initialize browser CDP client")?;
        Ok(Self { client, endpoint })
    }

    async fn targets(&self) -> Result<Vec<Target>> {
        let url = self.endpoint.join("/json/list")?;
        self.client
            .get(url)
            .send()
            .await
            .context(
                "No dedicated browser connection. Start it with scripts/start-browser-control.sh.",
            )?
            .error_for_status()
            .context("Browser CDP returned an error")?
            .json()
            .await
            .context("Browser CDP returned an invalid target list")
    }

    async fn target(&self, id: &str) -> Result<Target> {
        self.targets()
            .await?
            .into_iter()
            .find(|target| target.id == id && target.kind == "page")
            .ok_or_else(|| {
                anyhow!(
                    "No browser tab found with target_id '{id}'. Call tabs to refresh the list."
                )
            })
    }

    async fn status(&self) -> Result<String> {
        let version: Value = self.client.get(self.endpoint.join("/json/version")?)
            .send().await.context("Automation browser is disconnected; use browser start. Do not kill browser processes.")?
            .error_for_status()?.json().await?;
        let ws = version["webSocketDebuggerUrl"]
            .as_str()
            .context("CDP browser WebSocket is missing")?;
        self.call_ws(ws, "Browser.getVersion", json!({})).await?;
        let targets = self.targets().await?;
        Ok(json!({"connected":true,"browser":version["Browser"],"tabs":page_target_count(&targets),
            "recovery":"If one page is unresponsive, use new_tab and continue there. Existing tabs are not closed."}).to_string())
    }

    async fn start(&self) -> Result<String> {
        if let Ok(status) = self.status().await {
            return Ok(status);
        }
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts/start-browser-control.sh");
        let output = timeout(
            Duration::from_secs(25),
            tokio::process::Command::new("bash")
                .arg(script)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("Automation-browser startup timed out")??;
        if !output.status.success() {
            bail!(
                "Automation browser could not start: {}",
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(600)
                    .collect::<String>()
            );
        }
        self.status().await
    }

    async fn new_tab(&self) -> Result<String> {
        let target: Target = self
            .client
            .put(self.endpoint.join("/json/new?about%3Ablank")?)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(
            json!({"target_id":target.id,"url":target.url,"title":target.title,
            "note":"New blank tab created. Use this target_id; previous tabs were left open."})
            .to_string(),
        )
    }

    async fn call(&self, target: &Target, method: &str, params: Value) -> Result<Value> {
        let ws_url = target
            .websocket
            .as_deref()
            .ok_or_else(|| anyhow!("This browser tab does not expose a debugging WebSocket"))?;
        let started = std::time::Instant::now();
        let result = self.call_ws(ws_url, method, params).await;
        if let Err(error) = &result {
            eprintln!(
                "[Browser] {method} target={} failed after {} ms: {error:#}",
                target.id,
                started.elapsed().as_millis()
            );
        }
        result.with_context(|| format!("CDP {method} for target {} failed; do not repeat a possibly dispatched action. Use status/read to inspect, or new_tab for an unresponsive page", target.id))
    }

    async fn call_ws(&self, ws_url: &str, method: &str, params: Value) -> Result<Value> {
        validate_websocket_url(ws_url, &self.endpoint)?;
        let budget = if method == "Runtime.evaluate" || method == "Browser.getVersion" {
            READ_TIMEOUT
        } else {
            CDP_TIMEOUT
        };
        let deadline = tokio::time::Instant::now() + budget;
        let (mut socket, _) = tokio::time::timeout_at(deadline, connect_async(ws_url))
            .await
            .with_context(|| format!("CDP {method}: connection deadline exceeded"))??;
        tokio::time::timeout_at(
            deadline,
            socket.send(Message::Text(
                json!({"id": 1, "method": method, "params": params}).to_string(),
            )),
        )
        .await
        .with_context(|| {
            format!("CDP {method}: send deadline exceeded; action outcome uncertain")
        })??;
        let response = tokio::time::timeout_at(deadline, async {
            while let Some(message) = socket.next().await {
                let message = message?;
                if let Message::Text(text) = message {
                    let value: Value = serde_json::from_str(&text)?;
                    if let Some(result) = cdp_result(&value, 1)? {
                        return Ok::<Value, anyhow::Error>(result);
                    }
                }
            }
            bail!("Browser debugging connection closed before replying")
        })
        .await
        .with_context(|| {
            format!(
                "CDP {method}: no response within {} seconds; action outcome uncertain",
                budget.as_secs()
            )
        })??;
        let _ = timeout(Duration::from_millis(100), socket.close(None)).await;
        Ok(response)
    }

    async fn snapshot(&self, target: &Target) -> Result<String> {
        let value = self.evaluate(target, SNAPSHOT_SCRIPT).await?;
        if value.is_null() {
            bail!("Browser returned no page snapshot.");
        }
        Ok(serde_json::to_string_pretty(&value)?)
    }

    async fn snapshot_after_mutation(&self, target: &Target, action: &str) -> ToolResult {
        match self.snapshot(target).await {
            Ok(snapshot) => ToolResult::success(snapshot),
            Err(error) => mutation_observation_failure(action, &error),
        }
    }

    async fn navigate(&self, target: &Target, url: &str) -> Result<()> {
        let parsed = navigation_url(url)?;
        let result = self
            .call(target, "Page.navigate", json!({"url": parsed.as_str()}))
            .await?;
        if let Some(error) = result.get("errorText").and_then(Value::as_str) {
            bail!("Browser navigation failed: {error}");
        }
        Ok(())
    }

    async fn click(&self, target: &Target, selector: &str) -> Result<()> {
        let expr = format!(
            "(() => {{ const e=document.querySelector({}); if(!e) return {{error:'missing'}}; if(e.matches(':disabled') || e.getAttribute('aria-disabled')==='true') return {{error:'disabled'}}; e.scrollIntoView({{block:'center',inline:'center'}}); const r=e.getBoundingClientRect(); const x=r.left+r.width/2,y=r.top+r.height/2; if(r.width<=0 || r.height<=0 || x<0 || y<0 || x>=innerWidth || y>=innerHeight) return {{error:'not_visible'}}; const hit=document.elementFromPoint(x,y); if(!hit || (hit!==e && !e.contains(hit))) return {{error:'occluded'}}; return {{x,y,width:r.width,height:r.height}}; }})()",
            js_string(selector)
        );
        let bounds = self.evaluate(target, &expr).await?;
        if let Some(error) = bounds.get("error").and_then(Value::as_str) {
            match error {
                "missing" => bail!("No element matches selector '{selector}'."),
                "disabled" => bail!("Element matching '{selector}' is disabled."),
                "occluded" => bail!("Element matching '{selector}' is covered by another element."),
                _ => bail!("Element matching '{selector}' is not visible in the page viewport."),
            }
        }
        let x = bounds
            .get("x")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow!("Could not locate element matching '{selector}'."))?;
        let y = bounds
            .get("y")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow!("No element matches selector '{selector}'."))?;
        if bounds.get("width").and_then(Value::as_f64).unwrap_or(0.0) <= 0.0
            || bounds.get("height").and_then(Value::as_f64).unwrap_or(0.0) <= 0.0
        {
            bail!("Element matching '{selector}' is not visible.");
        }
        self.call(
            target,
            "Input.dispatchMouseEvent",
            json!({"type":"mouseMoved","x":x,"y":y}),
        )
        .await?;
        if let Err(press_error) = self
            .call(
                target,
                "Input.dispatchMouseEvent",
                json!({"type":"mousePressed","x":x,"y":y,"button":"left","clickCount":1}),
            )
            .await
        {
            // A timed out press may have reached Chrome even though its reply was lost.
            // Release the mouse button as cleanup, but never repeat the press.
            let released = self
                .call(
                    target,
                    "Input.dispatchMouseEvent",
                    json!({"type":"mouseReleased","x":x,"y":y,"button":"left","clickCount":1}),
                )
                .await
                .is_ok();
            if released {
                bail!(
                    "Mouse press result was ambiguous ({press_error:#}); a release was sent to clear button state. The click may have occurred."
                );
            }
            bail!(
                "Mouse press result was ambiguous ({press_error:#}), and mouse release could not be confirmed. The click may have occurred."
            );
        }
        self.call(
            target,
            "Input.dispatchMouseEvent",
            json!({"type":"mouseReleased","x":x,"y":y,"button":"left","clickCount":1}),
        )
        .await?;
        Ok(())
    }

    async fn fill(&self, target: &Target, selector: &str, text: &str) -> Result<()> {
        let expr = fill_preparation_script(selector);
        let state = self
            .evaluate(target, &expr)
            .await?
            .as_str()
            .unwrap_or("")
            .to_owned();
        if state == "missing" {
            bail!("No element matches selector '{selector}'.");
        }
        if state == "disabled" {
            bail!("Element matching '{selector}' is disabled.");
        }
        if state == "readonly" {
            bail!("Element matching '{selector}' is read-only.");
        }
        if state == "hidden" {
            bail!("Element matching '{selector}' is not visible.");
        }
        if state != "ready" {
            bail!("Element matching '{selector}' is not a visible text input, textarea, or editable region.");
        }
        // Preparation only selects the current value. If insertion times out,
        // the original contents remain intact unless the browser accepted the
        // insertion, so a lost CDP reply cannot silently clear the field.
        self.call(target, "Input.insertText", json!({"text":text}))
            .await?;
        Ok(())
    }

    async fn evaluate(&self, target: &Target, expression: &str) -> Result<Value> {
        let result = self
            .call(
                target,
                "Runtime.evaluate",
                json!({"expression":expression,"returnByValue":true,"timeout":4000}),
            )
            .await?;
        runtime_value(result)
    }
}

fn fill_preparation_script(selector: &str) -> String {
    format!(
        "(() => {{ const e=document.querySelector({}); if(!e) return 'missing'; const textInputTypes=['text','search','url','tel','email','password','number']; const supported=e instanceof HTMLTextAreaElement || (e instanceof HTMLInputElement && textInputTypes.includes(e.type)) || e.isContentEditable; if(!supported) return 'unsupported'; if(e.matches(':disabled') || e.getAttribute('aria-disabled')==='true') return 'disabled'; if(('readOnly' in e) && e.readOnly) return 'readonly'; const style=getComputedStyle(e); if(!e.getClientRects().length || style.visibility==='hidden' || style.display==='none') return 'hidden'; e.focus(); if(e.isContentEditable) {{ const range=document.createRange(); range.selectNodeContents(e); const selection=getSelection(); selection.removeAllRanges(); selection.addRange(range); }} else e.select(); return 'ready'; }})()",
        js_string(selector)
    )
}

fn runtime_value(result: Value) -> Result<Value> {
    if let Some(error) = result.get("exceptionDetails") {
        bail!(
            "Browser page evaluation failed: {}",
            error
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("page exception")
        );
    }
    Ok(result
        .pointer("/result/value")
        .cloned()
        .unwrap_or(Value::Null))
}

fn page_target_count(targets: &[Target]) -> usize {
    targets
        .iter()
        .filter(|target| target.kind == "page")
        .count()
}

fn is_loopback_url(url: &Url) -> bool {
    match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        None => false,
    }
}

fn cdp_result(response: &Value, expected_id: u64) -> Result<Option<Value>> {
    if response.get("id").and_then(Value::as_u64) != Some(expected_id) {
        return Ok(None);
    }
    if let Some(error) = response.get("error") {
        bail!(
            "Browser command failed: {}",
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown CDP error")
        );
    }
    Ok(Some(response.get("result").cloned().unwrap_or(Value::Null)))
}

fn navigation_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).context("Navigation URL must be absolute")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("Browser navigation supports only absolute http:// and https:// URLs.");
    }
    Ok(url)
}

fn validate_websocket_url(raw: &str, endpoint: &Url) -> Result<()> {
    let ws = Url::parse(raw).context("Browser returned an invalid WebSocket URL")?;
    let same_port = ws.port_or_known_default() == endpoint.port_or_known_default();
    if ws.scheme() != "ws" || !is_loopback_url(&ws) || !same_port {
        bail!("Browser returned a WebSocket outside the configured loopback CDP endpoint.");
    }
    Ok(())
}

fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
}

const SNAPSHOT_SCRIPT: &str = r#"(() => {
  const limit = (s, n) => (s || '').replace(/\s+/g, ' ').trim().slice(0, n);
  const selector = (e) => {
    if (e.id) { const s = '#' + CSS.escape(e.id); if (document.querySelectorAll(s).length === 1) return s; }
    const parts = []; let n = e;
    while (n && n.nodeType === 1 && n !== document.documentElement) {
      let p = n.tagName.toLowerCase();
      if (n.parentElement) { const same = [...n.parentElement.children].filter(x => x.tagName === n.tagName); if (same.length > 1) p += ':nth-of-type(' + (same.indexOf(n) + 1) + ')'; }
      parts.unshift(p); n = n.parentElement;
      const s = parts.join(' > '); if (document.querySelectorAll(s).length === 1) return s;
    }
    return parts.join(' > ');
  };
  const controls = [...document.querySelectorAll('a,button,input,textarea,select,[role=button],[contenteditable=true]')].slice(0, 80).map(e => ({selector: selector(e), tag:e.tagName.toLowerCase(), type:e.type || null, text:limit(e.innerText || e.getAttribute('aria-label') || e.value || e.placeholder, 180), disabled:!!e.disabled}));
  return {title:document.title, url:location.href, text:limit(document.body?.innerText, 12000), controls};
})()"#;

#[async_trait]
impl Tool for BrowserTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "browser",
            version: CORE_VERSION,
            description: "Control the separate Hyusk browser profile over local CDP. Use status to diagnose, start to open/reuse the managed automation browser, and new_tab to recover from an unresponsive page without closing existing tabs. Read DOM, navigate, click, fill, and press keys. Never kill browser processes with shell commands.",
            schema: browser_schema,
            risk: &[RiskClass::Read, RiskClass::Mutate, RiskClass::Input],
            capabilities: &[Capability::Input],
            source: Source::Core,
        }
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let action: BrowserAction =
            serde_json::from_str(input).context("Browser action must be valid JSON")?;
        let result = match action {
            BrowserAction::Status => self.status().await?,
            BrowserAction::Start => self.start().await?,
            BrowserAction::NewTab => self.new_tab().await?,
            BrowserAction::Tabs => {
                let tabs: Vec<Value> = self
                    .targets()
                    .await?
                    .into_iter()
                    .filter(|t| t.kind == "page")
                    .map(|t| json!({"target_id":t.id,"title":t.title,"url":t.url}))
                    .collect();
                serde_json::to_string_pretty(&tabs)?
            }
            BrowserAction::Navigate { target_id, url } => {
                navigation_url(&url)?;
                require_non_empty("target_id", &target_id)?;
                let target = self.target(&target_id).await?;
                self.navigate(&target, &url).await?;
                tokio::time::sleep(Duration::from_millis(350)).await;
                return Ok(self.snapshot_after_mutation(&target, "Navigation").await);
            }
            BrowserAction::Read { target_id } => {
                require_non_empty("target_id", &target_id)?;
                let t = self.target(&target_id).await?;
                self.snapshot(&t).await?
            }
            BrowserAction::Click {
                target_id,
                selector,
            } => {
                require_non_empty("target_id", &target_id)?;
                require_non_empty("selector", &selector)?;
                let t = self.target(&target_id).await?;
                self.click(&t, &selector).await?;
                return Ok(self.snapshot_after_mutation(&t, "Click").await);
            }
            BrowserAction::Fill {
                target_id,
                selector,
                text,
            } => {
                require_non_empty("target_id", &target_id)?;
                require_non_empty("selector", &selector)?;
                let t = self.target(&target_id).await?;
                self.fill(&t, &selector, &text).await?;
                return Ok(self.snapshot_after_mutation(&t, "Fill").await);
            }
            BrowserAction::PressKey { target_id, key } => {
                require_non_empty("target_id", &target_id)?;
                let t = self.target(&target_id).await?;
                press_key(self, &t, &key).await?;
                return Ok(self.snapshot_after_mutation(&t, "Key press").await);
            }
        };
        Ok(ToolResult::success(result))
    }
}

fn browser_schema() -> Value {
    json!({"type":"object","properties":{
        "action":{"type":"string","enum":["status","start","new_tab","tabs","read","navigate","click","fill","press_key"]},
        "target_id":{"type":"string","minLength":1,"description":"Non-empty ID returned by tabs or new_tab; required for read, navigate, click, fill and press_key."},
        "url":{"type":"string","format":"uri","minLength":1,"description":"Absolute http:// or https:// URL for navigate."},
        "selector":{"type":"string","minLength":1,"description":"Non-empty CSS selector for click or fill."},
        "text":{"type":"string","description":"Text to place into an input, textarea, or editable region."},
        "key":{"type":"string","enum":["Enter","Tab","Escape","Backspace","Delete","ArrowUp","ArrowDown","ArrowLeft","ArrowRight","Home","End","PageUp","PageDown","Ctrl+A"]}
    },"required":["action"]})
}

fn require_non_empty(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("Browser {field} must not be empty.");
    }
    Ok(())
}

async fn press_key(tool: &BrowserTool, target: &Target, key: &str) -> Result<()> {
    let (key, code, vk) = match key {
        "Enter" => ("Enter", "Enter", 13),
        "Tab" => ("Tab", "Tab", 9),
        "Escape" => ("Escape", "Escape", 27),
        "Backspace" => ("Backspace", "Backspace", 8),
        "Delete" => ("Delete", "Delete", 46),
        "ArrowUp" => ("ArrowUp", "ArrowUp", 38),
        "ArrowDown" => ("ArrowDown", "ArrowDown", 40),
        "ArrowLeft" => ("ArrowLeft", "ArrowLeft", 37),
        "ArrowRight" => ("ArrowRight", "ArrowRight", 39),
        "Home" => ("Home", "Home", 36),
        "End" => ("End", "End", 35),
        "PageUp" => ("PageUp", "PageUp", 33),
        "PageDown" => ("PageDown", "PageDown", 34),
        "Ctrl+A" => return ctrl_a(tool, target).await,
        _ => bail!("Unsupported key. Use one of the documented key names."),
    };
    tool.call(
        target,
        "Input.dispatchKeyEvent",
        json!({"type":"keyDown","key":key,"code":code,"windowsVirtualKeyCode":vk}),
    )
    .await?;
    tool.call(
        target,
        "Input.dispatchKeyEvent",
        json!({"type":"keyUp","key":key,"code":code,"windowsVirtualKeyCode":vk}),
    )
    .await?;
    Ok(())
}

async fn ctrl_a(tool: &BrowserTool, target: &Target) -> Result<()> {
    tool.call(target,"Input.dispatchKeyEvent",json!({"type":"keyDown","key":"Control","code":"ControlLeft","windowsVirtualKeyCode":17,"modifiers":2})).await?;
    tool.call(
        target,
        "Input.dispatchKeyEvent",
        json!({"type":"keyDown","key":"a","code":"KeyA","windowsVirtualKeyCode":65,"modifiers":2}),
    )
    .await?;
    tool.call(
        target,
        "Input.dispatchKeyEvent",
        json!({"type":"keyUp","key":"a","code":"KeyA","windowsVirtualKeyCode":65,"modifiers":2}),
    )
    .await?;
    tool.call(
        target,
        "Input.dispatchKeyEvent",
        json!({"type":"keyUp","key":"Control","code":"ControlLeft","windowsVirtualKeyCode":17}),
    )
    .await?;
    Ok(())
}

fn mutation_observation_failure(action: &str, error: &anyhow::Error) -> ToolResult {
    let detail: String = error.to_string().chars().take(500).collect();
    ToolResult::failure(format!(
        "{action} was dispatched, but the follow-up page read failed: {detail}. The page may already have changed. Do not repeat this action; call browser read with the same target_id to check the current state. This does not confirm task completion."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn endpoint_must_be_loopback_http() {
        assert!(is_loopback_url(
            &Url::parse("http://127.0.0.1:9222").unwrap()
        ));
        assert!(is_loopback_url(&Url::parse("http://[::1]:9222").unwrap()));
        assert!(!is_loopback_url(
            &Url::parse("http://192.168.1.4:9222").unwrap()
        ));
        let endpoint = Url::parse("http://127.0.0.1:9222").unwrap();
        assert!(validate_websocket_url("ws://127.0.0.1:9222/devtools/page/1", &endpoint).is_ok());
        assert!(
            validate_websocket_url("ws://example.com:9222/devtools/page/1", &endpoint).is_err()
        );
        assert!(validate_websocket_url("ws://127.0.0.1:9223/devtools/page/1", &endpoint).is_err());
    }

    #[test]
    fn status_counts_only_page_tabs() {
        let targets = vec![
            Target {
                id: "page".into(),
                kind: "page".into(),
                title: String::new(),
                url: String::new(),
                websocket: None,
            },
            Target {
                id: "worker".into(),
                kind: "service_worker".into(),
                title: String::new(),
                url: String::new(),
                websocket: None,
            },
        ];
        assert_eq!(page_target_count(&targets), 1);
    }

    #[test]
    fn action_parser_requires_typed_fields() {
        assert!(serde_json::from_str::<BrowserAction>(
            r##"{"action":"fill","target_id":"t","selector":"#q","text":"hello"}"##
        )
        .is_ok());
        assert!(serde_json::from_str::<BrowserAction>(
            r#"{"action":"navigate","target_id":"t","url":"javascript:alert(1)"}"#
        )
        .is_ok());
        assert!(
            serde_json::from_str::<BrowserAction>(r##"{"action":"click","selector":"#q"}"##)
                .is_err()
        );
        assert!(navigation_url("https://example.com").is_ok());
        assert!(navigation_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn advertised_actions_match_decodable_action_samples() {
        let schema = browser_schema();
        let advertised = schema["properties"]["action"]["enum"].as_array().unwrap();
        let samples = [
            json!({"action":"status"}),
            json!({"action":"start"}),
            json!({"action":"new_tab"}),
            json!({"action":"tabs"}),
            json!({"action":"read","target_id":"tab-1"}),
            json!({"action":"navigate","target_id":"tab-1","url":"https://example.com"}),
            json!({"action":"click","target_id":"tab-1","selector":"#go"}),
            json!({"action":"fill","target_id":"tab-1","selector":"#query","text":"hello"}),
            json!({"action":"press_key","target_id":"tab-1","key":"Enter"}),
        ];

        for sample in samples {
            let action_name = sample["action"].as_str().unwrap();
            assert!(advertised
                .iter()
                .any(|value| value.as_str() == Some(action_name)));
            assert!(serde_json::from_value::<BrowserAction>(sample).is_ok());
        }
    }

    #[tokio::test]
    async fn missing_or_empty_required_arguments_fail_before_cdp_io() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let tool = BrowserTool {
            client: Client::new(),
            endpoint: Url::parse(&format!("http://{address}"))?,
        };
        let invalid = [
            json!({"action":"read"}),
            json!({"action":"navigate","target_id":"tab-1"}),
            json!({"action":"click","target_id":"tab-1"}),
            json!({"action":"fill","target_id":"tab-1","selector":"#query"}),
            json!({"action":"press_key","key":"Enter"}),
            json!({"action":"read","target_id":"  "}),
            json!({"action":"click","target_id":"tab-1","selector":"  "}),
            json!({"action":"navigate","target_id":"tab-1","url":"javascript:alert(1)"}),
        ];

        for input in invalid {
            assert!(tool.execute(&input.to_string()).await.is_err(), "{input}");
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn selectors_and_text_are_embedded_as_json_strings() {
        assert_eq!(js_string("#a'\\\n"), "\"#a'\\\\\\n\"");
    }

    #[test]
    fn fill_preparation_selects_without_clearing_and_rejects_non_text_inputs() {
        let script = fill_preparation_script("#query");
        assert!(script.contains("e.select()"));
        assert!(script.contains("selection.addRange(range)"));
        assert!(script.contains("textInputTypes.includes(e.type)"));
        assert!(script.contains("return 'hidden'"));
        assert!(!script.contains("set.call(e,''"));
        assert!(!script.contains("e.textContent=''"));
    }

    #[test]
    fn cdp_response_parser_ignores_events_and_reports_protocol_errors() {
        let event = json!({"method":"Page.loadEventFired","params":{}});
        assert!(cdp_result(&event, 1).unwrap().is_none());

        let other_id = json!({"id":2,"result":{"value":"ignored"}});
        assert!(cdp_result(&other_id, 1).unwrap().is_none());

        let response = json!({"id":1,"result":{"value":"ok"}});
        assert_eq!(
            cdp_result(&response, 1).unwrap(),
            Some(json!({"value":"ok"}))
        );

        let error = json!({"id":1,"error":{"code":-32601,"message":"method not found"}});
        assert!(cdp_result(&error, 1)
            .unwrap_err()
            .to_string()
            .contains("method not found"));
    }

    #[test]
    fn runtime_exceptions_are_not_reported_as_empty_snapshots() {
        let result = json!({"exceptionDetails":{"text":"Uncaught TypeError"}});
        assert!(runtime_value(result)
            .unwrap_err()
            .to_string()
            .contains("Uncaught TypeError"));
        assert!(runtime_value(json!({})).unwrap().is_null());
    }

    #[test]
    fn mutation_observation_failure_warns_against_repeating_the_action() {
        let error = anyhow::anyhow!("snapshot unavailable");
        let result = mutation_observation_failure("Click", &error);
        assert!(!result.success);
        let message = result.error.unwrap();
        assert!(message.contains("Click was dispatched"));
        assert!(message.contains("may already have changed"));
        assert!(message.contains("Do not repeat this action"));
        assert!(message.contains("browser read"));
        assert!(message.contains("does not confirm task completion"));
    }

    #[tokio::test]
    async fn cdp_rpc_correlates_events_and_preserves_method_errors() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let request: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(request["method"], "Runtime.evaluate");
            ws.send(Message::Text(
                json!({"method":"Page.frameNavigated","params":{}}).to_string(),
            ))
            .await
            .unwrap();
            ws.send(Message::Text(json!({"id":99,"result":{}}).to_string()))
                .await
                .unwrap();
            ws.send(Message::Text(
                json!({"id":1,"result":{"ok":true}}).to_string(),
            ))
            .await
            .unwrap();
            let _ = ws.next().await;
        });
        let tool = BrowserTool {
            client: Client::new(),
            endpoint: Url::parse(&format!("http://{address}"))?,
        };
        assert_eq!(
            tool.call_ws(
                &format!("ws://{address}/page"),
                "Runtime.evaluate",
                json!({"expression":"1+1"})
            )
            .await?,
            json!({"ok":true})
        );
        server.await?;
        Ok(())
    }

    #[tokio::test]
    async fn unanswered_cdp_rpc_has_one_bounded_deadline_even_with_events() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let _ = ws.next().await;
            loop {
                if ws
                    .send(Message::Text(
                        json!({"method":"Page.frameNavigated","params":{}}).to_string(),
                    ))
                    .await
                    .is_err()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        });
        let tool = BrowserTool {
            client: Client::new(),
            endpoint: Url::parse(&format!("http://{address}"))?,
        };
        let started = std::time::Instant::now();
        let result = tool
            .call_ws(
                &format!("ws://{address}/test"),
                "Runtime.evaluate",
                json!({"expression":"1+1"}),
            )
            .await;
        server.abort();
        let _ = server.await;
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains("Runtime.evaluate: no response within 5 seconds"));
        assert!(started.elapsed() < Duration::from_secs(10));
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires the dedicated local browser profile and HYUSK_BROWSER_LIVE_TEST=1"]
    async fn live_cdp_reads_fills_and_clicks_only_a_new_test_tab() -> Result<()> {
        if std::env::var("HYUSK_BROWSER_LIVE_TEST").as_deref() != Ok("1") {
            bail!("Set HYUSK_BROWSER_LIVE_TEST=1 to enable the live CDP smoke test.");
        }

        let tool = BrowserTool::new()?;
        invoke(&tool, json!({"action":"status"})).await?;
        invoke(&tool, json!({"action":"start"})).await?;
        const FIXTURE: &str = "<!doctype html><html><head><title>Hyusk CDP smoke</title></head><body><label>Name <input id=\"name\"></label><button id=\"apply\" onclick=\"document.querySelector('#result').textContent='saved: '+document.querySelector('#name').value\">Apply</button><p id=\"result\">waiting</p></body></html>";
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let fixture_url = format!("http://{}/", listener.local_addr()?);
        let fixture_server = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    FIXTURE.len(),
                    FIXTURE
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });

        let result = async {
            let created: Value =
                serde_json::from_str(&invoke(&tool, json!({"action":"new_tab"})).await?)?;
            let target_id = created["target_id"]
                .as_str()
                .context("new_tab target ID")?
                .to_string();
            let close_url = tool.endpoint.join(&format!("/json/close/{target_id}"))?;

            let exercise = async {
                let navigate = json!({"action":"navigate","target_id":target_id,"url":fixture_url});
                invoke(&tool, navigate).await?;

                let mut loaded = false;
                for _ in 0..20 {
                    let read =
                        invoke(&tool, json!({"action":"read","target_id":target_id})).await?;
                    if read.contains("Hyusk CDP smoke") {
                        loaded = true;
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
                if !loaded {
                    bail!("The local fixture did not load in the new test tab.");
                }

                invoke(
                    &tool,
                    json!({"action":"fill","target_id":target_id,"selector":"#name","text":"Ada"}),
                )
                .await?;
                invoke(
                    &tool,
                    json!({"action":"click","target_id":target_id,"selector":"#apply"}),
                )
                .await?;

                let snapshot =
                    invoke(&tool, json!({"action":"read","target_id":target_id})).await?;
                if !snapshot.contains("saved: Ada") {
                    bail!("The fixture did not reflect the filled value after the button click.");
                }

                let tabs: Vec<Value> =
                    serde_json::from_str(&invoke(&tool, json!({"action":"tabs"})).await?)?;
                let found = tabs.iter().any(|tab| {
                    tab.get("target_id").and_then(Value::as_str) == Some(target_id.as_str())
                        && tab.get("title").and_then(Value::as_str) == Some("Hyusk CDP smoke")
                });
                if !found {
                    bail!("The tabs action did not return metadata for the created test tab.");
                }
                // Reuse the same target repeatedly, not just a one-shot blank
                // page; verify startup is idempotent with the test tab open.
                invoke(&tool, json!({"action":"start"})).await?;
                for _ in 0..3 {
                    invoke(
                        &tool,
                        json!({"action":"navigate","target_id":target_id,"url":fixture_url}),
                    )
                    .await?;
                    invoke(&tool, json!({"action":"read","target_id":target_id})).await?;
                }
                Ok::<(), anyhow::Error>(())
            };

            // The test owns only this target; never enumerate-and-close existing tabs.
            let exercise = exercise.await;
            let close = tool
                .client
                .get(close_url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status);
            exercise?;
            close?;
            Ok::<(), anyhow::Error>(())
        }
        .await;

        fixture_server.abort();
        let _ = fixture_server.await;
        result
    }

    async fn invoke(tool: &BrowserTool, input: Value) -> Result<String> {
        let result = tool.execute(&input.to_string()).await?;
        if !result.success {
            bail!(
                "Browser action failed: {}",
                result.error.as_deref().unwrap_or("unknown error")
            );
        }
        Ok(result.output)
    }
}
