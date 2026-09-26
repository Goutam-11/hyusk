use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use enigo::{
    Axis, Button as EnigoButton, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::sleep;

use super::{Tool, ToolResult};

const MAX_TEXT_BYTES: usize = 100_000;
const MAX_CLICKS: u32 = 20;
const MAX_SCROLL: i32 = 10_000;
const MAX_WAIT_MS: u64 = 30_000;
const MAX_COMBO_KEYS: usize = 12;

const SCREENSHOT_PREFIX: &str = "hyusk-screenshot-";
const DEFAULT_SCREENSHOT_RETENTION_SECS: u64 = 900;
const DEFAULT_SCREENSHOT_MAX_FILES: usize = 30;

/// Desktop/GUI control tool.
///
/// This is a structured alternative to reaching for `shell` when the user
/// wants the agent to interact with the visible desktop. On Linux it prefers
/// the XDG RemoteDesktop portal on Wayland so GNOME and other compositors can
/// accept input after a one-time user approval. It falls back to the native
/// `enigo` backend for X11 and compositors with virtual-input protocols.
pub struct ComputerTool {
    #[cfg(target_os = "linux")]
    portal: tokio::sync::Mutex<PortalState>,
}

impl ComputerTool {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            portal: tokio::sync::Mutex::new(PortalState::default()),
        }
    }
}

impl Default for ComputerTool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct PortalState {
    input: Option<crate::tools::portal::PortalInput>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ComputerAction {
    Status,

    Screens,

    Cursor,

    FindText {
        text: String,
        #[serde(default)]
        path: Option<PathBuf>,
        #[serde(default)]
        region: Option<Region>,
        #[serde(default)]
        include_cursor: bool,
    },

    ClickText {
        text: String,
        #[serde(default)]
        button: ButtonName,
        #[serde(default)]
        path: Option<PathBuf>,
        #[serde(default)]
        region: Option<Region>,
        #[serde(default)]
        include_cursor: bool,
    },

    Batch {
        steps: Vec<ComputerAction>,
    },

    ClipboardSet {
        text: String,
    },

    ClipboardGet,

    ClipboardClear,

    OpenUrl {
        url: String,
    },

    FocusWindow {
        title: String,
    },

    Screenshot {
        #[serde(default)]
        region: Option<Region>,
        #[serde(default)]
        include_cursor: bool,
        #[serde(default = "default_true")]
        inline_image: bool,
    },

    AppState {
        #[serde(default)]
        region: Option<Region>,
        #[serde(default)]
        include_cursor: bool,
        #[serde(default = "default_true")]
        inline_image: bool,
        #[serde(default = "default_app_state_depth")]
        max_depth: u8,
        #[serde(default = "default_app_state_nodes")]
        max_nodes: usize,
    },

    Ocr {
        #[serde(default)]
        path: Option<PathBuf>,
        #[serde(default)]
        region: Option<Region>,
        #[serde(default)]
        include_cursor: bool,
    },

    MouseMove {
        x: i32,
        y: i32,
        #[serde(default)]
        relative: bool,
    },

    MouseClick {
        #[serde(default)]
        button: ButtonName,
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
        #[serde(default = "default_one")]
        count: u32,
    },

    MouseDrag {
        from: Point,
        to: Point,
        #[serde(default)]
        button: ButtonName,
    },

    Scroll {
        #[serde(default)]
        axis: ScrollAxis,
        amount: i32,
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
    },

    TypeText {
        text: String,
    },

    KeyPress {
        key: String,
        #[serde(default = "default_one")]
        count: u32,
    },

    KeyCombo {
        keys: Vec<String>,
        #[serde(default = "default_one")]
        count: u32,
    },

    Wait {
        #[serde(default = "default_wait_ms")]
        milliseconds: u64,
    },
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Point {
    x: i32,
    y: i32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Region {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl Region {
    fn validate(self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            bail!("Screenshot region width and height must be greater than zero.");
        }

        Ok(())
    }

    fn grim_arg(self) -> String {
        format!("{},{} {}x{}", self.x, self.y, self.width, self.height)
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ButtonName {
    #[default]
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl ButtonName {
    fn as_enigo(self) -> EnigoButton {
        match self {
            Self::Left => EnigoButton::Left,
            Self::Right => EnigoButton::Right,
            Self::Middle => EnigoButton::Middle,
            Self::Back => EnigoButton::Back,
            Self::Forward => EnigoButton::Forward,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
            Self::Back => "back",
            Self::Forward => "forward",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ScrollAxis {
    #[default]
    Vertical,
    Horizontal,
}

impl ScrollAxis {
    fn as_enigo(self) -> Axis {
        match self {
            Self::Vertical => Axis::Vertical,
            Self::Horizontal => Axis::Horizontal,
        }
    }
}

fn default_one() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

fn model_vision_enabled() -> bool {
    env::var("MODEL_VISION")
        .map(|value| {
            !matches!(
                value.to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
        .unwrap_or(true)
}

fn default_wait_ms() -> u64 {
    1_000
}

fn session_kind() -> &'static str {
    match env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") => "wayland",
        Ok("x11") => "x11",
        _ if env::var_os("WAYLAND_DISPLAY").is_some() => "wayland",
        _ if env::var_os("DISPLAY").is_some() => "x11",
        _ => "unknown",
    }
}

fn screenshot_backends() -> &'static [&'static str] {
    match session_kind() {
        "wayland" => &["grim", "GNOME Shell D-Bus", "desktop portal"],
        "x11" => &["ImageMagick import", "scrot"],
        _ if cfg!(target_os = "macos") => &["screencapture"],
        _ => &[],
    }
}

fn screenshot_backend() -> String {
    let backends = screenshot_backends();

    if backends.is_empty() {
        "unsupported".to_string()
    } else {
        backends.join(", ")
    }
}

#[cfg(target_os = "linux")]
fn should_use_portal() -> bool {
    session_kind() == "wayland"
}

#[cfg(not(target_os = "linux"))]
fn should_use_portal() -> bool {
    false
}

fn enigo_status() -> String {
    match Enigo::new(&Settings::default()) {
        Ok(enigo) => {
            let mut lines = Vec::new();

            match enigo.main_display() {
                Ok((width, height)) => lines.push(format!("display: {width}x{height}")),
                Err(error) => lines.push(format!("display: unavailable ({error})")),
            }

            match enigo.location() {
                Ok((x, y)) => lines.push(format!("pointer: {x},{y}")),
                Err(error) => lines.push(format!("pointer: unavailable ({error})")),
            }

            match ensure_input_backend_supported(&enigo) {
                Ok(()) => lines.push("available".to_string()),
                Err(error) => lines.push(format!("unavailable ({error})")),
            }

            lines.join(", ")
        }

        Err(error) => format!("unavailable ({error})"),
    }
}

fn screenshot_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    env::temp_dir().join(format!(
        "{SCREENSHOT_PREFIX}{}-{nanos}.png",
        std::process::id()
    ))
}

/// Remove old temporary screenshots.
///
/// Files older than `HYUSK_SCREENSHOT_RETENTION_SECS` are deleted, and the
/// number of retained files is capped at `HYUSK_SCREENSHOT_MAX_FILES`.
pub fn cleanup_screenshots() {
    let directory = env::temp_dir();

    let Ok(entries) = std::fs::read_dir(&directory) else {
        return;
    };

    let retention = Duration::from_secs(
        env::var("HYUSK_SCREENSHOT_RETENTION_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_SCREENSHOT_RETENTION_SECS),
    );

    let max_files = env::var("HYUSK_SCREENSHOT_MAX_FILES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_SCREENSHOT_MAX_FILES);

    let now = SystemTime::now();
    let mut retained = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();

        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        if !name.starts_with(SCREENSHOT_PREFIX) {
            continue;
        }

        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH);

        let expired = now
            .duration_since(modified)
            .map(|age| age > retention)
            .unwrap_or(false);

        if expired {
            let _ = std::fs::remove_file(&path);
        } else {
            retained.push((path, modified));
        }
    }

    if retained.len() > max_files {
        retained.sort_by_key(|(_, modified)| *modified);

        let remove_count = retained.len() - max_files;

        for (path, _) in retained.into_iter().take(remove_count) {
            let _ = std::fs::remove_file(path);
        }
    }
}

async fn capture_screenshot(
    region: Option<Region>,
    include_cursor: bool,
) -> Result<(PathBuf, &'static str)> {
    if let Some(region) = region {
        region.validate()?;
    }

    cleanup_screenshots();

    let mut errors = Vec::new();

    for backend in screenshot_backends() {
        let path = screenshot_path();

        let result = match *backend {
            "grim" => capture_with_grim(&path, region, include_cursor).await,
            "GNOME Shell D-Bus" => capture_with_gnome_shell(&path, region, include_cursor).await,
            "desktop portal" => capture_with_portal(&path, region).await,
            "ImageMagick import" => capture_with_import(&path, region).await,
            "scrot" => capture_with_scrot(&path, region).await,
            "screencapture" => capture_with_screencapture(&path, region).await,
            other => Err(anyhow!("Unknown screenshot backend '{other}'.")),
        };

        match result {
            Ok(()) if path.exists() => return Ok((path, backend)),

            Ok(()) => errors.push(format!(
                "{backend}: command reported success but no file was created"
            )),

            Err(error) => errors.push(format!("{backend}: {error}")),
        }
    }

    if errors.is_empty() {
        bail!("Screenshot capture is not implemented for this display backend.");
    }

    bail!("Screenshot capture failed. {}", errors.join("; "));
}

async fn capture_with_grim(
    path: &Path,
    region: Option<Region>,
    include_cursor: bool,
) -> Result<()> {
    let mut command = Command::new("grim");

    if include_cursor {
        command.arg("-c");
    }

    if let Some(region) = region {
        command.arg("-g").arg(region.grim_arg());
    }

    command.arg(path);

    let output = command
        .output()
        .await
        .context("Failed to run 'grim'. Is grim installed?")?;

    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }

    Ok(())
}

async fn capture_with_gnome_shell(
    path: &Path,
    region: Option<Region>,
    include_cursor: bool,
) -> Result<()> {
    let path = path
        .to_str()
        .ok_or_else(|| anyhow!("Screenshot path is not valid UTF-8."))?;

    let include_cursor = if include_cursor { "true" } else { "false" };

    let mut command = Command::new("gdbus");

    command.args([
        "call",
        "--session",
        "--dest",
        "org.gnome.Shell.Screenshot",
        "--object-path",
        "/org/gnome/Shell/Screenshot",
        "--method",
    ]);

    match region {
        Some(region) => {
            command
                .arg("org.gnome.Shell.Screenshot.ScreenshotArea")
                .arg(region.x.to_string())
                .arg(region.y.to_string())
                .arg(region.width.to_string())
                .arg(region.height.to_string())
                .arg("false")
                .arg(path);
        }

        None => {
            command
                .arg("org.gnome.Shell.Screenshot.Screenshot")
                .arg(include_cursor)
                .arg("false")
                .arg(path);
        }
    }

    let output = command
        .output()
        .await
        .context("Failed to run 'gdbus'. Is it installed?")?;

    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    if !stdout.contains("true") {
        bail!("GNOME Shell did not report success: {}", stdout.trim());
    }

    Ok(())
}

#[cfg(target_os = "linux")]
async fn capture_with_portal(target: &Path, region: Option<Region>) -> Result<()> {
    if region.is_some() {
        bail!(
            "The Screenshot portal does not support programmatic region capture; \
             use grim, ImageMagick import, or scrot for region screenshots"
        );
    }

    let uri = crate::tools::portal::screenshot_uri().await?;
    let source = crate::tools::portal::uri_to_path(&uri)?;

    tokio::fs::copy(&source, target).await.with_context(|| {
        format!(
            "Failed to copy portal screenshot from {} to {}",
            source.display(),
            target.display()
        )
    })?;

    Ok(())
}

async fn capture_with_import(path: &Path, region: Option<Region>) -> Result<()> {
    let mut command = Command::new("import");

    command.arg("-window").arg("root");

    if let Some(region) = region {
        command.arg("-crop").arg(format!(
            "{}x{}+{}+{}",
            region.width, region.height, region.x, region.y
        ));
    }

    command.arg(path);

    let output = command
        .output()
        .await
        .context("Failed to run ImageMagick 'import'. Is ImageMagick installed?")?;

    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }

    Ok(())
}

async fn capture_with_scrot(path: &Path, region: Option<Region>) -> Result<()> {
    let mut command = Command::new("scrot");

    if let Some(region) = region {
        command.arg("-a").arg(format!(
            "{},{},{},{}",
            region.x, region.y, region.width, region.height
        ));
    }

    command.arg("-o").arg(path);

    let output = command
        .output()
        .await
        .context("Failed to run 'scrot'. Is scrot installed?")?;

    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }

    Ok(())
}

async fn capture_with_screencapture(path: &Path, region: Option<Region>) -> Result<()> {
    let mut command = Command::new("screencapture");

    command.arg("-x");

    if let Some(region) = region {
        command.arg("-R").arg(format!(
            "{},{},{},{}",
            region.x, region.y, region.width, region.height
        ));
    }

    command.arg(path);

    let output = command
        .output()
        .await
        .context("Failed to run 'screencapture'.")?;

    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }

    Ok(())
}

fn run_with_enigo<T>(action: impl FnOnce(&mut Enigo) -> Result<T>) -> Result<T> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|error| anyhow!("Failed to connect to the desktop input backend: {error}"))?;

    ensure_input_backend_supported(&enigo)?;

    action(&mut enigo)
}

/// Reject the XWayland fallback on a Wayland session.
///
/// `enigo` tries Wayland first and then falls back to X11. On compositors that
/// do not expose the virtual keyboard/pointer protocols, that fallback creates
/// an XWayland connection whose XTEST events are not delivered to native
/// Wayland applications. The input call returns success even though nothing
/// happened. Detect that case through the Wayland-specific pointer query: the
/// native Wayland backend cannot report a pointer location, so a successful
/// location query on a Wayland session means the fallback is in use.
fn ensure_input_backend_supported(enigo: &Enigo) -> Result<()> {
    if session_kind() == "wayland" && enigo.location().is_ok() {
        bail!(
            "This Wayland session does not expose virtual input protocols. \
             The input backend fell back to XWayland, which does not deliver \
             synthetic input to native Wayland applications. Use an X11 \
             session, a compositor with virtual keyboard/pointer support, or \
             the desktop RemoteDesktop/libei portal."
        );
    }

    Ok(())
}

#[cfg(target_os = "linux")]
impl ComputerTool {
    async fn ensure_portal<'a>(
        &self,
        state: &'a mut PortalState,
    ) -> Result<&'a mut crate::tools::portal::PortalInput> {
        if let Some(error) = state.error.clone() {
            bail!("Desktop portal input is unavailable: {error}");
        }

        if state.input.is_none() {
            let attempt = tokio::time::timeout(
                Duration::from_secs(120),
                crate::tools::portal::PortalInput::connect(),
            )
            .await;

            match attempt {
                Ok(Ok(input)) => state.input = Some(input),

                Ok(Err(error)) => {
                    let message = error.to_string();
                    state.error = Some(message.clone());
                    bail!("{message}");
                }

                Err(_) => {
                    let message = "Timed out waiting for desktop portal approval".to_string();
                    state.error = Some(message.clone());
                    bail!("{message}");
                }
            }
        }

        Ok(state
            .input
            .as_mut()
            .expect("portal input was initialized above"))
    }

    async fn portal_mouse_move(&self, x: i32, y: i32, relative: bool) -> Result<ToolResult> {
        if !relative && (x < 0 || y < 0) {
            return Ok(ToolResult::failure(
                "Absolute mouse coordinates must be non-negative.",
            ));
        }

        let mut state = self.portal.lock().await;
        let input = self.ensure_portal(&mut state).await?;

        if relative {
            input.move_relative(x as f64, y as f64).await?;
        } else {
            seed_portal_position(input);
            input.move_absolute(x as f64, y as f64).await?;
        }

        Ok(ToolResult::success(format!(
            "Mouse moved {} ({x}, {y}).",
            if relative { "by" } else { "to" }
        )))
    }

    async fn portal_mouse_click(
        &self,
        button: ButtonName,
        x: Option<i32>,
        y: Option<i32>,
        count: u32,
    ) -> Result<ToolResult> {
        if let Err(message) = validate_count(count) {
            return Ok(ToolResult::failure(message));
        }

        for coordinate in [x, y].into_iter().flatten() {
            if coordinate < 0 {
                return Ok(ToolResult::failure(
                    "Absolute mouse coordinates must be non-negative.",
                ));
            }
        }

        let mut state = self.portal.lock().await;
        let input = self.ensure_portal(&mut state).await?;

        if let (Some(x), Some(y)) = (x, y) {
            seed_portal_position(input);
            input.move_absolute(x as f64, y as f64).await?;
            portal_settle(40).await;
        }

        let code = portal_button_code(button);

        for index in 0..count {
            if index > 0 {
                portal_settle(120).await;
            }

            input.button(code, true).await?;
            portal_settle(30).await;
            input.button(code, false).await?;
        }

        Ok(ToolResult::success(format!(
            "Clicked {} button {count} time(s){}.",
            button.as_str(),
            position_suffix(x, y)
        )))
    }

    async fn portal_mouse_drag(
        &self,
        from: Point,
        to: Point,
        button: ButtonName,
    ) -> Result<ToolResult> {
        if from.x < 0 || from.y < 0 || to.x < 0 || to.y < 0 {
            return Ok(ToolResult::failure(
                "Absolute drag coordinates must be non-negative.",
            ));
        }

        let mut state = self.portal.lock().await;
        let input = self.ensure_portal(&mut state).await?;

        seed_portal_position(input);
        input.move_absolute(from.x as f64, from.y as f64).await?;
        portal_settle(40).await;

        let code = portal_button_code(button);

        input.button(code, true).await?;
        portal_settle(30).await;

        // Drag through intermediate points so Wayland compositors register
        // the motion instead of treating it as a teleport.
        let steps = 12;
        let step_x = (to.x - from.x) as f64 / steps as f64;
        let step_y = (to.y - from.y) as f64 / steps as f64;

        for step in 1..steps {
            input
                .move_absolute(
                    from.x as f64 + step_x * step as f64,
                    from.y as f64 + step_y * step as f64,
                )
                .await?;
            portal_settle(12).await;
        }

        let movement = input.move_absolute(to.x as f64, to.y as f64).await;
        let release = input.button(code, false).await;

        movement?;
        release?;

        Ok(ToolResult::success(format!(
            "Dragged {} button from ({}, {}) to ({}, {}).",
            button.as_str(),
            from.x,
            from.y,
            to.x,
            to.y
        )))
    }

    async fn portal_scroll(
        &self,
        axis: ScrollAxis,
        amount: i32,
        x: Option<i32>,
        y: Option<i32>,
    ) -> Result<ToolResult> {
        if amount == 0 {
            return Ok(ToolResult::failure("Scroll amount must not be zero."));
        }

        if amount.abs() > MAX_SCROLL {
            return Ok(ToolResult::failure(format!(
                "Scroll amount must be between -{MAX_SCROLL} and {MAX_SCROLL}."
            )));
        }

        for coordinate in [x, y].into_iter().flatten() {
            if coordinate < 0 {
                return Ok(ToolResult::failure(
                    "Absolute mouse coordinates must be non-negative.",
                ));
            }
        }

        let mut state = self.portal.lock().await;
        let input = self.ensure_portal(&mut state).await?;

        if let (Some(x), Some(y)) = (x, y) {
            seed_portal_position(input);
            input.move_absolute(x as f64, y as f64).await?;
            portal_settle(40).await;
        }

        let axis = match axis {
            ScrollAxis::Vertical => ashpd::desktop::remote_desktop::Axis::Vertical,
            ScrollAxis::Horizontal => ashpd::desktop::remote_desktop::Axis::Horizontal,
        };

        input.scroll(axis, amount).await?;

        Ok(ToolResult::success(format!(
            "Scrolled {} by {amount}.",
            match axis {
                ashpd::desktop::remote_desktop::Axis::Vertical => "vertically",
                ashpd::desktop::remote_desktop::Axis::Horizontal => "horizontally",
            }
        )))
    }

    async fn portal_type_text(&self, text: &str) -> Result<ToolResult> {
        if text.contains('\0') {
            return Ok(ToolResult::failure("Text must not contain NUL bytes."));
        }

        if text.len() > MAX_TEXT_BYTES {
            return Ok(ToolResult::failure(format!(
                "Text exceeds the maximum of {MAX_TEXT_BYTES} bytes."
            )));
        }

        if text.is_empty() {
            return Ok(ToolResult::success("No text to type."));
        }

        let mut state = self.portal.lock().await;
        let input = self.ensure_portal(&mut state).await?;

        input.type_text(text).await?;

        Ok(ToolResult::success(format!(
            "Typed {} byte(s) of text.",
            text.len()
        )))
    }

    async fn portal_key_press(&self, key: &str, count: u32) -> Result<ToolResult> {
        if let Err(message) = validate_count(count) {
            return Ok(ToolResult::failure(message));
        }

        let evdev = crate::tools::portal::evdev_key(key);

        let keysym = if evdev.is_some() {
            None
        } else {
            match parse_keysym(key) {
                Ok(keysym) => Some(keysym),
                Err(message) => return Ok(ToolResult::failure(message)),
            }
        };

        let mut state = self.portal.lock().await;
        let input = self.ensure_portal(&mut state).await?;

        for _ in 0..count {
            match evdev {
                Some((keycode, shift)) => input.keycode_click(keycode, shift).await?,
                None => {
                    input
                        .key_click(keysym.expect("keysym checked above"))
                        .await?
                }
            }
        }

        Ok(ToolResult::success(format!(
            "Pressed key '{key}' {count} time(s)."
        )))
    }

    async fn portal_key_combo(&self, keys: &[String], count: u32) -> Result<ToolResult> {
        if keys.is_empty() {
            return Ok(ToolResult::failure("Key combo must not be empty."));
        }

        if keys.len() > MAX_COMBO_KEYS {
            return Ok(ToolResult::failure(format!(
                "Key combo may contain at most {MAX_COMBO_KEYS} keys."
            )));
        }

        if let Err(message) = validate_count(count) {
            return Ok(ToolResult::failure(message));
        }

        let mut evdev_keys = Vec::with_capacity(keys.len());
        let mut all_evdev = true;

        for key in keys {
            match crate::tools::portal::evdev_key(key) {
                Some(value) => evdev_keys.push(value),
                None => {
                    all_evdev = false;
                    break;
                }
            }
        }

        let mut keysyms = Vec::new();

        if !all_evdev {
            for key in keys {
                match parse_keysym(key) {
                    Ok(keysym) => keysyms.push(keysym),
                    Err(message) => return Ok(ToolResult::failure(message)),
                }
            }
        }

        let mut state = self.portal.lock().await;
        let input = self.ensure_portal(&mut state).await?;

        for _ in 0..count {
            if all_evdev {
                input.evdev_combo(&evdev_keys).await?;
            } else {
                input.key_combo(&keysyms).await?;
            }
        }

        Ok(ToolResult::success(format!(
            "Pressed combo '{}' {count} time(s).",
            keys.join("+")
        )))
    }
}

/*
 * The compositor processes portal input notifications asynchronously. A
 * button press sent in the same breath as an absolute move regularly lands
 * on the *old* pointer position (or is dropped), which is the classic
 * "the agent clicked but nothing happened" symptom. Short settle gaps make
 * clicks land reliably while keeping the flow fast.
 */
#[cfg(target_os = "linux")]
async fn portal_settle(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

#[cfg(target_os = "linux")]
fn seed_portal_position(input: &mut crate::tools::portal::PortalInput) {
    if input.has_position() {
        return;
    }

    if let Ok(enigo) = Enigo::new(&Settings::default()) {
        if let Ok((x, y)) = enigo.location() {
            input.set_position_hint(x, y);
        }
    }
}

#[cfg(target_os = "linux")]
fn portal_button_code(button: ButtonName) -> i32 {
    match button {
        ButtonName::Left => 0x110,
        ButtonName::Right => 0x111,
        ButtonName::Middle => 0x112,
        ButtonName::Forward => 0x115,
        ButtonName::Back => 0x116,
    }
}

#[cfg(target_os = "linux")]
fn parse_keysym(name: &str) -> Result<i32, String> {
    use xkbcommon::xkb::{keysym_from_name, utf32_to_keysym, Keysym, KEYSYM_CASE_INSENSITIVE};

    let trimmed = name.trim();

    if trimmed.is_empty() {
        return Err("Key name must not be empty.".to_string());
    }

    let lowered = trimmed.to_ascii_lowercase();

    let xkb_name = match lowered.as_str() {
        "alt" | "option" => "Alt_L",
        "backspace" => "BackSpace",
        "capslock" | "caps_lock" => "Caps_Lock",
        "control" | "ctrl" => "Control_L",
        "delete" | "del" => "Delete",
        "down" | "down_arrow" | "arrowdown" | "arrow_down" => "Down",
        "end" => "End",
        "enter" | "return" => "Return",
        "escape" | "esc" => "Escape",
        "home" => "Home",
        "insert" | "ins" => "Insert",
        "left" | "left_arrow" | "arrowleft" | "arrow_left" => "Left",
        "meta" | "super" | "windows" | "win" | "cmd" | "command" => "Super_L",
        "pagedown" | "page_down" | "pgdn" => "Page_Down",
        "pageup" | "page_up" | "pgup" => "Page_Up",
        "printscreen" | "print_screen" | "printscr" => "Print",
        "right" | "right_arrow" | "arrowright" | "arrow_right" => "Right",
        "shift" => "Shift_L",
        "space" | "spacebar" => "space",
        "tab" => "Tab",
        "up" | "up_arrow" | "arrowup" | "arrow_up" => "Up",
        _ => "",
    };

    if !xkb_name.is_empty() {
        let keysym = keysym_from_name(xkb_name, KEYSYM_CASE_INSENSITIVE);

        if keysym != Keysym::NoSymbol {
            return Ok(keysym.raw() as i32);
        }

        return Err(format!("Unsupported key '{name}'."));
    }

    if let Some(number) = lowered.strip_prefix('f') {
        if let Ok(number) = number.parse::<u8>() {
            if (1..=12).contains(&number) {
                let keysym = keysym_from_name(&format!("F{number}"), KEYSYM_CASE_INSENSITIVE);

                if keysym != Keysym::NoSymbol {
                    return Ok(keysym.raw() as i32);
                }
            }
        }
    }

    if let Some(number) = lowered.strip_prefix("numpad") {
        if let Ok(number) = number.parse::<u8>() {
            if number <= 9 {
                let keysym = keysym_from_name(&format!("KP_{number}"), KEYSYM_CASE_INSENSITIVE);

                if keysym != Keysym::NoSymbol {
                    return Ok(keysym.raw() as i32);
                }
            }
        }
    }

    let mut characters = trimmed.chars();

    if let (Some(character), None) = (characters.next(), characters.next()) {
        let keysym = utf32_to_keysym(character as u32);

        if keysym != Keysym::NoSymbol {
            return Ok(keysym.raw() as i32);
        }
    }

    Err(format!(
        "Unsupported key '{name}'. Use a single character or one of the named keys."
    ))
}

fn parse_key(name: &str) -> Result<Key, String> {
    let trimmed = name.trim();

    if trimmed.is_empty() {
        return Err("Key name must not be empty.".to_string());
    }

    let lowered = trimmed.to_ascii_lowercase();

    let key = match lowered.as_str() {
        "alt" | "option" => Key::Alt,
        "backspace" => Key::Backspace,
        "capslock" | "caps_lock" => Key::CapsLock,
        "control" | "ctrl" => Key::Control,
        "delete" | "del" => Key::Delete,
        "down" | "down_arrow" | "arrowdown" | "arrow_down" => Key::DownArrow,
        "end" => Key::End,
        "enter" | "return" => Key::Return,
        "escape" | "esc" => Key::Escape,
        "home" => Key::Home,
        "insert" | "ins" => Key::Insert,
        "left" | "left_arrow" | "arrowleft" | "arrow_left" => Key::LeftArrow,
        "meta" | "super" | "windows" | "win" | "cmd" | "command" => Key::Meta,
        "pagedown" | "page_down" | "pgdn" => Key::PageDown,
        "pageup" | "page_up" | "pgup" => Key::PageUp,
        "printscreen" | "print_screen" | "printscr" => Key::PrintScr,
        "right" | "right_arrow" | "arrowright" | "arrow_right" => Key::RightArrow,
        "shift" => Key::Shift,
        "space" | "spacebar" => Key::Space,
        "tab" => Key::Tab,
        "up" | "up_arrow" | "arrowup" | "arrow_up" => Key::UpArrow,
        _ => {
            if let Some(number) = lowered.strip_prefix('f') {
                if let Ok(number) = number.parse::<u8>() {
                    return match number {
                        1 => Ok(Key::F1),
                        2 => Ok(Key::F2),
                        3 => Ok(Key::F3),
                        4 => Ok(Key::F4),
                        5 => Ok(Key::F5),
                        6 => Ok(Key::F6),
                        7 => Ok(Key::F7),
                        8 => Ok(Key::F8),
                        9 => Ok(Key::F9),
                        10 => Ok(Key::F10),
                        11 => Ok(Key::F11),
                        12 => Ok(Key::F12),
                        other => Err(format!("F{other} is not a supported function key.")),
                    };
                }
            }

            if let Some(number) = lowered.strip_prefix("numpad") {
                if let Ok(number) = number.parse::<u8>() {
                    return match number {
                        0 => Ok(Key::Numpad0),
                        1 => Ok(Key::Numpad1),
                        2 => Ok(Key::Numpad2),
                        3 => Ok(Key::Numpad3),
                        4 => Ok(Key::Numpad4),
                        5 => Ok(Key::Numpad5),
                        6 => Ok(Key::Numpad6),
                        7 => Ok(Key::Numpad7),
                        8 => Ok(Key::Numpad8),
                        9 => Ok(Key::Numpad9),
                        other => Err(format!("Numpad {other} is not supported.")),
                    };
                }
            }

            let mut characters = trimmed.chars();

            if let (Some(character), None) = (characters.next(), characters.next()) {
                return Ok(Key::Unicode(character));
            }

            return Err(format!(
                "Unsupported key '{name}'. Use a single character or one of the named keys."
            ));
        }
    };

    Ok(key)
}

fn is_modifier(key: Key) -> bool {
    matches!(
        key,
        Key::Alt
            | Key::Control
            | Key::LControl
            | Key::RControl
            | Key::Shift
            | Key::LShift
            | Key::RShift
            | Key::Meta
    )
}

fn validate_count(count: u32) -> Result<(), String> {
    if count == 0 {
        return Err("Count must be at least 1.".to_string());
    }

    if count > MAX_CLICKS {
        return Err(format!("Count exceeds the maximum of {MAX_CLICKS}."));
    }

    Ok(())
}

fn position_suffix(x: Option<i32>, y: Option<i32>) -> String {
    match (x, y) {
        (Some(x), Some(y)) => format!(" at ({x}, {y})"),
        _ => String::new(),
    }
}

async fn status() -> Result<ToolResult> {
    let session = session_kind();
    let mut lines = vec![format!("session: {session}")];

    if session == "wayland" {
        #[cfg(target_os = "linux")]
        {
            let portal = tokio::time::timeout(
                Duration::from_secs(3),
                crate::tools::portal::portal_version(),
            )
            .await;

            match portal {
                Ok(Ok(version)) => lines.push(format!(
                    "input injection: RemoteDesktop portal available (v{version}; user approval required on first use)"
                )),

                Ok(Err(error)) => lines.push(format!(
                    "input injection: RemoteDesktop portal unavailable ({error})"
                )),

                Err(_) => lines.push(
                    "input injection: RemoteDesktop portal did not respond".to_string(),
                ),
            }
        }

        lines.push(format!("enigo fallback: {}", enigo_status()));
    } else {
        lines.push(format!("input injection: {}", enigo_status()));
    }

    lines.push(format!("screenshot backends: {}", screenshot_backend()));
    lines.push("OCR backend: tesseract (optional)".to_string());
    lines.push(
        "note: the portal may ask for approval; individual actions can still be rejected."
            .to_string(),
    );

    Ok(ToolResult::success(lines.join("\n")))
}

async fn screens(tool: &ComputerTool) -> Result<ToolResult> {
    let mut lines = Vec::new();

    match Enigo::new(&Settings::default()) {
        Ok(enigo) => match enigo.main_display() {
            Ok((width, height)) => lines.push(format!("primary display: {width}x{height}")),
            Err(error) => lines.push(format!("primary display: unavailable ({error})")),
        },

        Err(error) => lines.push(format!("primary display: unavailable ({error})")),
    }

    #[cfg(target_os = "linux")]
    {
        let state = tool.portal.lock().await;

        if let Some(input) = state.input.as_ref() {
            if let Some((x, y)) = input.screen_position() {
                lines.push(format!("portal stream position: {x},{y}"));
            }

            if let Some((width, height)) = input.screen_size() {
                lines.push(format!("portal stream size: {width}x{height}"));
            }
        } else {
            lines.push(
                "portal stream: not started (approval happens on the first input action)"
                    .to_string(),
            );
        }
    }

    lines.push("coordinates are global pixels with the origin at the top-left".to_string());

    Ok(ToolResult::success(lines.join("\n")))
}

#[derive(Debug, Clone)]
struct OcrWord {
    text: String,
    center_x: i32,
    center_y: i32,
    confidence: f32,
}

async fn tesseract_words(path: &Path) -> Result<Vec<OcrWord>> {
    let output = Command::new("tesseract")
        .arg(path)
        .arg("stdout")
        .arg("tsv")
        .output()
        .await
        .context("Failed to run 'tesseract'. Is it installed and on PATH?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("tesseract failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut words = Vec::new();

    for line in stdout.lines().skip(1) {
        let columns: Vec<&str> = line.split('\t').collect();

        if columns.len() < 12 || columns[0] != "5" {
            continue;
        }

        let text = columns[11].trim();

        if text.is_empty() {
            continue;
        }

        let left: i32 = columns[6].parse().unwrap_or(0);
        let top: i32 = columns[7].parse().unwrap_or(0);
        let width: i32 = columns[8].parse().unwrap_or(0);
        let height: i32 = columns[9].parse().unwrap_or(0);
        let confidence: f32 = columns[10].parse().unwrap_or(0.0);

        if confidence < 30.0 {
            continue;
        }

        words.push(OcrWord {
            text: text.to_string(),
            center_x: left + width / 2,
            center_y: top + height / 2,
            confidence,
        });
    }

    Ok(words)
}

async fn resolve_ocr_path(
    path: Option<PathBuf>,
    region: Option<Region>,
    include_cursor: bool,
) -> Result<PathBuf> {
    match path {
        Some(path) => Ok(path),
        None => Ok(capture_screenshot(region, include_cursor).await?.0),
    }
}

async fn find_text(
    text: &str,
    path: Option<PathBuf>,
    region: Option<Region>,
    include_cursor: bool,
) -> Result<ToolResult> {
    let image = resolve_ocr_path(path, region, include_cursor).await?;
    let words = tesseract_words(&image).await?;
    let query = text.trim().to_lowercase();

    let matches: Vec<&OcrWord> = words
        .iter()
        .filter(|word| query.is_empty() || word.text.to_lowercase().contains(&query))
        .collect();

    if matches.is_empty() {
        return Ok(ToolResult::failure(format!(
            "No OCR text matching '{text}' in {}",
            image.display()
        )));
    }

    let mut lines = vec![format!(
        "Matches for '{text}' in {} ({} found):",
        image.display(),
        matches.len()
    )];

    for word in matches.iter().take(40) {
        lines.push(format!(
            "- '{}' center ({}, {}) confidence {:.1}",
            word.text, word.center_x, word.center_y, word.confidence
        ));
    }

    if matches.len() > 40 {
        lines.push(format!("... {} more", matches.len() - 40));
    }

    Ok(ToolResult::success(lines.join("\n")))
}

async fn cursor(tool: &ComputerTool) -> Result<ToolResult> {
    match Enigo::new(&Settings::default()) {
        Ok(enigo) => match enigo.location() {
            Ok((x, y)) => return Ok(ToolResult::success(format!("Pointer at ({x}, {y})."))),
            Err(error) => eprintln!("[Computer] Pointer query failed: {error}"),
        },

        Err(error) => eprintln!("[Computer] Pointer backend unavailable: {error}"),
    }

    #[cfg(target_os = "linux")]
    {
        let state = tool.portal.lock().await;

        if let Some(input) = state.input.as_ref() {
            if let Some((x, y)) = input.tracked_position() {
                return Ok(ToolResult::success(format!(
                    "Pointer tracked at ({x:.0}, {y:.0})."
                )));
            }
        }
    }

    Ok(ToolResult::failure(
        "Pointer position is not available on this backend.",
    ))
}

async fn run_command_stdin(program: &str, args: &[&str], input: &str) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to run '{program}'. Is it installed?"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.as_bytes())
            .await
            .context("Failed to write clipboard input")?;
    }

    let output = child
        .wait_with_output()
        .await
        .context("Failed waiting for clipboard command")?;

    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }

    Ok(())
}

async fn run_command_output(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .with_context(|| format!("Failed to run '{program}'. Is it installed?"))?;

    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn clipboard_set(text: &str) -> Result<ToolResult> {
    let result = match session_kind() {
        "wayland" => run_command_stdin("wl-copy", &[], text).await,

        "x11" => {
            match run_command_stdin("xclip", &["-selection", "clipboard", "-in"], text).await {
                Ok(()) => Ok(()),
                Err(_) => run_command_stdin("xsel", &["-ib"], text).await,
            }
        }

        _ if cfg!(target_os = "macos") => run_command_stdin("pbcopy", &[], text).await,

        _ if cfg!(target_os = "windows") => run_command_stdin("clip", &[], text).await,

        _ => Err(anyhow!(
            "Clipboard set is not implemented for this display backend"
        )),
    };

    match result {
        Ok(()) => Ok(ToolResult::success(format!(
            "Clipboard updated ({} bytes).",
            text.len()
        ))),

        Err(error) => Ok(ToolResult::failure(format!(
            "Could not set the clipboard: {error}"
        ))),
    }
}

async fn clipboard_get() -> Result<ToolResult> {
    let result = match session_kind() {
        "wayland" => run_command_output("wl-paste", &["--no-newline"]).await,

        "x11" => match run_command_output("xclip", &["-selection", "clipboard", "-o"]).await {
            Ok(text) => Ok(text),
            Err(_) => run_command_output("xsel", &["-ob"]).await,
        },

        _ if cfg!(target_os = "macos") => run_command_output("pbpaste", &[]).await,

        _ if cfg!(target_os = "windows") => {
            run_command_output("powershell", &["-NoProfile", "-Command", "Get-Clipboard"]).await
        }

        _ => Err(anyhow!(
            "Clipboard read is not implemented for this display backend"
        )),
    };

    match result {
        Ok(text) if text.is_empty() => Ok(ToolResult::success("Clipboard is empty.")),

        Ok(text) => Ok(ToolResult::success(format!("Clipboard:\n{text}"))),

        Err(error) => Ok(ToolResult::failure(format!(
            "Could not read the clipboard: {error}"
        ))),
    }
}

async fn clipboard_clear() -> Result<ToolResult> {
    let result = match session_kind() {
        "wayland" => run_command_output("wl-copy", &["--clear"])
            .await
            .map(|_| ()),

        "x11" => match run_command_stdin("xclip", &["-selection", "clipboard", "-in"], "").await {
            Ok(()) => Ok(()),
            Err(_) => run_command_stdin("xsel", &["-bc"], "").await,
        },

        _ if cfg!(target_os = "macos") => run_command_stdin("pbcopy", &[], "").await,

        _ if cfg!(target_os = "windows") => run_command_stdin("clip", &[], "").await,

        _ => Err(anyhow!(
            "Clipboard clear is not implemented for this display backend"
        )),
    };

    match result {
        Ok(()) => Ok(ToolResult::success("Clipboard cleared.")),

        Err(error) => Ok(ToolResult::failure(format!(
            "Could not clear the clipboard: {error}"
        ))),
    }
}

async fn open_url(url: &str) -> Result<ToolResult> {
    let trimmed = url.trim();

    if !(trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("file://"))
    {
        return Ok(ToolResult::failure(
            "URL must start with http://, https://, or file://.",
        ));
    }

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(trimmed);
        command
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", trimmed]);
        command
    };

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(trimmed);
        command
    };

    command.stdout(Stdio::null()).stderr(Stdio::null());

    match command.spawn() {
        Ok(child) => Ok(ToolResult::success(format!(
            "Opened {trimmed} in the default application (PID {:?}).",
            child.id()
        ))),

        Err(error) => Ok(ToolResult::failure(format!(
            "Could not open {trimmed}: {error}"
        ))),
    }
}

async fn focus_window(title: &str) -> Result<ToolResult> {
    let trimmed = title.trim();

    if trimmed.is_empty() {
        return Ok(ToolResult::failure("Window title must not be empty."));
    }

    let candidates: Vec<(&str, Vec<String>)> = vec![
        (
            "hyprctl",
            vec![
                "dispatch".to_string(),
                "focuswindow".to_string(),
                format!("title:{trimmed}"),
            ],
        ),
        ("wmctrl", vec!["-a".to_string(), trimmed.to_string()]),
        (
            "xdotool",
            vec![
                "search".to_string(),
                "--name".to_string(),
                trimmed.to_string(),
                "windowactivate".to_string(),
            ],
        ),
    ];

    let mut errors = Vec::new();

    for (program, args) in candidates {
        match Command::new(program).args(&args).output().await {
            Ok(output) if output.status.success() => {
                return Ok(ToolResult::success(format!(
                    "Focused a window matching '{trimmed}' with {program}."
                )));
            }

            Ok(output) => errors.push(format!(
                "{program}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),

            Err(error) => errors.push(format!("{program}: {error}")),
        }
    }

    Ok(ToolResult::failure(format!(
        "Could not focus a window matching '{trimmed}'. Tried: {}. GNOME Wayland does not expose a generic window-focus API.",
        errors.join("; ")
    )))
}

async fn screenshot(
    region: Option<Region>,
    include_cursor: bool,
    inline_image: bool,
) -> Result<ToolResult> {
    let (path, backend) = capture_screenshot(region, include_cursor).await?;
    let bytes = tokio::fs::metadata(&path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    let mut output = format!(
        "Screenshot saved to {} using {} ({} bytes).",
        path.display(),
        backend,
        bytes
    );

    /*
     * Attach the image automatically whenever the configured model supports
     * images. The runner converts this marker into a multimodal user message.
     * `inline_image: false` only affects text-only models.
     */
    if model_vision_enabled() {
        output.push_str(&format!("\nHYUSK_IMAGE:{}", path.display()));
        output.push_str(
            "\nThe image is attached to this conversation. Inspect it directly; \
             do not call `ocr` for a screenshot you can see.",
        );
    } else if inline_image {
        output.push_str(&format!("\nHYUSK_IMAGE:{}", path.display()));
    } else {
        output.push_str(
            "\nThe file is not embedded; use the `ocr` action or a local tool to inspect it.",
        );
    }

    Ok(ToolResult::success(output))
}

fn default_app_state_depth() -> u8 {
    5
}
fn default_app_state_nodes() -> usize {
    2000
}

async fn app_state(
    region: Option<Region>,
    include_cursor: bool,
    inline_image: bool,
    max_depth: u8,
    max_nodes: usize,
) -> Result<ToolResult> {
    let request = json!({"action":"app_state", "max_depth":max_depth, "max_nodes":max_nodes});
    let child_result = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/atspi_bridge.py"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let accessibility = match child_result {
        Ok(mut child) => match tokio::time::timeout(Duration::from_secs(60), async {
            let sent = if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(format!("{}\n", request).as_bytes())
                    .await
                    .is_ok()
            } else {
                false
            };
            (sent, child.wait_with_output().await)
        })
        .await
        {
            Ok((true, Ok(output))) if output.status.success() => {
                serde_json::from_slice(&output.stdout).unwrap_or_else(
                    |error| json!({"ok":false,"error":format!("Invalid AT-SPI response: {error}")}),
                )
            }
            Err(_) => json!({"ok":false,"error":"AT-SPI app state timed out after 60 seconds"}),
            _ => json!({"ok":false,"error":"AT-SPI bridge unavailable"}),
        },
        Err(error) => json!({"ok":false,"error":format!("AT-SPI bridge unavailable: {error}")}),
    };
    let screenshot_result = match tokio::time::timeout(
        Duration::from_secs(30),
        capture_screenshot(region, include_cursor),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(anyhow!("Screenshot capture timed out after 30 seconds")),
    };
    let screenshot = match screenshot_result {
        Ok((path, backend)) => {
            let bytes = tokio::fs::metadata(&path)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            json!({"ok":true,"path":path,"backend":backend,"bytes":bytes})
        }
        Err(error) => json!({"ok":false,"error":format!("{error:#}")}),
    };
    let snapshot = json!({
        "screenshot": screenshot,
        "active_window": {
            "application": accessibility["active"]["app_name"],
            "title": accessibility["active"]["name"],
            "path": accessibility["active"]["path"],
        },
        "accessibility": accessibility,
        "consistency": "sequential observations; screenshot is the full desktop, not cropped to the active window; UI changes between reads may make them differ; AT-SPI paths are valid only for this fresh tree snapshot",
        "fallback": "If the accessibility tree is unavailable or empty, inspect the screenshot (or OCR it when vision is unavailable).",
    });
    let mut output =
        serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| snapshot.to_string());
    if let Some(path) = snapshot["screenshot"]["path"].as_str() {
        if model_vision_enabled() || inline_image {
            output.push_str(&format!("\nHYUSK_IMAGE:{path}"));
        }
    }
    if snapshot["screenshot"]["ok"] != true && snapshot["accessibility"]["ok"] != true {
        return Ok(ToolResult::failure(output));
    }
    Ok(ToolResult::success(output))
}

async fn ocr(
    path: Option<PathBuf>,
    region: Option<Region>,
    include_cursor: bool,
) -> Result<ToolResult> {
    let path = match path {
        Some(path) => path,

        None => capture_screenshot(region, include_cursor).await?.0,
    };

    if !path.exists() {
        return Ok(ToolResult::failure(format!(
            "OCR input does not exist: {}",
            path.display()
        )));
    }

    let output = Command::new("tesseract")
        .arg(&path)
        .arg("stdout")
        .output()
        .await
        .context("Failed to run 'tesseract'. Is it installed and on PATH?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        return Ok(ToolResult::failure(format!(
            "tesseract failed: {}",
            stderr.trim()
        )));
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if text.is_empty() {
        Ok(ToolResult::success(format!(
            "OCR completed for {} but no text was recognized.",
            path.display()
        )))
    } else {
        Ok(ToolResult::success(format!(
            "OCR text from {}:\n{text}",
            path.display()
        )))
    }
}

fn mouse_move(x: i32, y: i32, relative: bool) -> Result<ToolResult> {
    if !relative && (x < 0 || y < 0) {
        return Ok(ToolResult::failure(
            "Absolute mouse coordinates must be non-negative.",
        ));
    }

    run_with_enigo(|enigo| {
        enigo.move_mouse(
            x,
            y,
            if relative {
                Coordinate::Rel
            } else {
                Coordinate::Abs
            },
        )?;

        Ok(ToolResult::success(format!(
            "Mouse moved {} ({x}, {y}).",
            if relative { "by" } else { "to" }
        )))
    })
}

fn mouse_click(
    button: ButtonName,
    x: Option<i32>,
    y: Option<i32>,
    count: u32,
) -> Result<ToolResult> {
    if let Err(message) = validate_count(count) {
        return Ok(ToolResult::failure(message));
    }

    for coordinate in [x, y].into_iter().flatten() {
        if coordinate < 0 {
            return Ok(ToolResult::failure(
                "Absolute mouse coordinates must be non-negative.",
            ));
        }
    }

    run_with_enigo(|enigo| {
        if let (Some(x), Some(y)) = (x, y) {
            enigo.move_mouse(x, y, Coordinate::Abs)?;
        }

        for _ in 0..count {
            enigo.button(button.as_enigo(), Direction::Click)?;
        }

        Ok(ToolResult::success(format!(
            "Clicked {} button {count} time(s){}.",
            button.as_str(),
            position_suffix(x, y)
        )))
    })
}

fn mouse_drag(from: Point, to: Point, button: ButtonName) -> Result<ToolResult> {
    if from.x < 0 || from.y < 0 || to.x < 0 || to.y < 0 {
        return Ok(ToolResult::failure(
            "Absolute drag coordinates must be non-negative.",
        ));
    }

    run_with_enigo(|enigo| {
        enigo.move_mouse(from.x, from.y, Coordinate::Abs)?;
        enigo.button(button.as_enigo(), Direction::Press)?;

        let movement = enigo.move_mouse(to.x, to.y, Coordinate::Abs);
        let release = enigo.button(button.as_enigo(), Direction::Release);

        // Always attempt to release the button, even if movement failed.
        movement?;
        release?;

        Ok(ToolResult::success(format!(
            "Dragged {} button from ({}, {}) to ({}, {}).",
            button.as_str(),
            from.x,
            from.y,
            to.x,
            to.y
        )))
    })
}

fn scroll(axis: ScrollAxis, amount: i32, x: Option<i32>, y: Option<i32>) -> Result<ToolResult> {
    if amount == 0 {
        return Ok(ToolResult::failure("Scroll amount must not be zero."));
    }

    if amount.abs() > MAX_SCROLL {
        return Ok(ToolResult::failure(format!(
            "Scroll amount must be between -{MAX_SCROLL} and {MAX_SCROLL}."
        )));
    }

    for coordinate in [x, y].into_iter().flatten() {
        if coordinate < 0 {
            return Ok(ToolResult::failure(
                "Absolute mouse coordinates must be non-negative.",
            ));
        }
    }

    run_with_enigo(|enigo| {
        if let (Some(x), Some(y)) = (x, y) {
            enigo.move_mouse(x, y, Coordinate::Abs)?;
        }

        enigo.scroll(amount, axis.as_enigo())?;

        Ok(ToolResult::success(format!(
            "Scrolled {} by {amount}.",
            match axis {
                ScrollAxis::Vertical => "vertically",
                ScrollAxis::Horizontal => "horizontally",
            }
        )))
    })
}

fn type_text(text: &str) -> Result<ToolResult> {
    if text.contains('\0') {
        return Ok(ToolResult::failure("Text must not contain NUL bytes."));
    }

    if text.len() > MAX_TEXT_BYTES {
        return Ok(ToolResult::failure(format!(
            "Text exceeds the maximum of {MAX_TEXT_BYTES} bytes."
        )));
    }

    if text.is_empty() {
        return Ok(ToolResult::success("No text to type."));
    }

    run_with_enigo(|enigo| {
        enigo.text(text)?;

        Ok(ToolResult::success(format!(
            "Typed {} byte(s) of text.",
            text.len()
        )))
    })
}

fn key_press(key: &str, count: u32) -> Result<ToolResult> {
    if let Err(message) = validate_count(count) {
        return Ok(ToolResult::failure(message));
    }

    let parsed = match parse_key(key) {
        Ok(key) => key,
        Err(message) => return Ok(ToolResult::failure(message)),
    };

    run_with_enigo(|enigo| {
        for _ in 0..count {
            enigo.key(parsed, Direction::Click)?;
        }

        Ok(ToolResult::success(format!(
            "Pressed key '{key}' {count} time(s)."
        )))
    })
}

fn key_combo(keys: &[String], count: u32) -> Result<ToolResult> {
    if keys.is_empty() {
        return Ok(ToolResult::failure("Key combo must not be empty."));
    }

    if keys.len() > MAX_COMBO_KEYS {
        return Ok(ToolResult::failure(format!(
            "Key combo may contain at most {MAX_COMBO_KEYS} keys."
        )));
    }

    if let Err(message) = validate_count(count) {
        return Ok(ToolResult::failure(message));
    }

    let mut parsed = Vec::with_capacity(keys.len());

    for key in keys {
        match parse_key(key) {
            Ok(key) => parsed.push(key),
            Err(message) => return Ok(ToolResult::failure(message)),
        }
    }

    let mut modifiers = Vec::new();
    let mut presses = Vec::new();

    for key in parsed {
        if is_modifier(key) {
            modifiers.push(key);
        } else {
            presses.push(key);
        }
    }

    if presses.is_empty() {
        return Ok(ToolResult::failure(
            "Key combo must include at least one non-modifier key.",
        ));
    }

    run_with_enigo(|enigo| {
        for _ in 0..count {
            for modifier in &modifiers {
                enigo.key(*modifier, Direction::Press)?;
            }

            for key in &presses {
                enigo.key(*key, Direction::Click)?;
            }

            for modifier in modifiers.iter().rev() {
                enigo.key(*modifier, Direction::Release)?;
            }
        }

        Ok(ToolResult::success(format!(
            "Pressed combo '{}' {count} time(s).",
            keys.join("+")
        )))
    })
}

async fn wait(milliseconds: u64) -> Result<ToolResult> {
    if milliseconds > MAX_WAIT_MS {
        return Ok(ToolResult::failure(format!(
            "Wait duration must not exceed {MAX_WAIT_MS} ms."
        )));
    }

    sleep(Duration::from_millis(milliseconds)).await;

    Ok(ToolResult::success(format!("Waited {milliseconds} ms.")))
}

#[async_trait]
impl Tool for ComputerTool {
    fn name(&self) -> &str {
        "computer"
    }

    fn description(&self) -> &str {
        "Control the local desktop: inspect the screen with screenshots/OCR, \
         combine a full-desktop screenshot with the active window's bounded \
         AT-SPI tree using `app_state`, locate labeled UI elements with find_text, \
         click them with click_text, \
         read the pointer with cursor, move and click the mouse, drag, scroll, \
         type text, press keys, and wait. Coordinates are global screen pixels \
         with the origin at the top-left. When MODEL_VISION=1, a `screenshot` is \
         attached to the conversation as an image; inspect it directly instead \
         of calling `ocr`. This tool can affect the user's real desktop; use it \
         only for actions the user asked for."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "status",
                        "screens",
                        "cursor",
                        "find_text",
                        "click_text",
                        "batch",
                        "clipboard_set",
                        "clipboard_get",
                        "clipboard_clear",
                        "open_url",
                        "focus_window",
                        "screenshot",
                        "app_state",
                        "ocr",
                        "mouse_move",
                        "mouse_click",
                        "mouse_drag",
                        "scroll",
                        "type_text",
                        "key_press",
                        "key_combo",
                        "wait"
                    ],
                    "description": "The desktop action to perform."
                },
                "x": {
                    "type": "integer",
                    "description": "Absolute X coordinate in screen pixels, or relative X offset when `relative` is true. Required for `mouse_move`; optional for `mouse_click` and `scroll`."
                },
                "y": {
                    "type": "integer",
                    "description": "Absolute Y coordinate in screen pixels, or relative Y offset when `relative` is true. Required for `mouse_move`; optional for `mouse_click` and `scroll`."
                },
                "relative": {
                    "type": "boolean",
                    "description": "For `mouse_move`, move by the given offset instead of to an absolute position. Defaults to false."
                },
                "button": {
                    "type": "string",
                    "enum": ["left", "right", "middle", "back", "forward"],
                    "description": "Mouse button. Defaults to left."
                },
                "count": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 20,
                    "description": "Number of clicks or key repetitions. Defaults to 1."
                },
                "from": {
                    "type": "object",
                    "description": "Start point for `mouse_drag`.",
                    "properties": {
                        "x": { "type": "integer" },
                        "y": { "type": "integer" }
                    },
                    "required": ["x", "y"]
                },
                "to": {
                    "type": "object",
                    "description": "End point for `mouse_drag`.",
                    "properties": {
                        "x": { "type": "integer" },
                        "y": { "type": "integer" }
                    },
                    "required": ["x", "y"]
                },
                "axis": {
                    "type": "string",
                    "enum": ["vertical", "horizontal"],
                    "description": "Scroll axis for `scroll`. Defaults to vertical."
                },
                "amount": {
                    "type": "integer",
                    "description": "Scroll direction and distance. Positive scrolls down/right, negative scrolls up/left. Required for `scroll`."
                },
                "text": {
                    "type": "string",
                    "description": "Text to type for `type_text`."
                },
                "key": {
                    "type": "string",
                    "description": "A single character or named key such as 'enter', 'escape', 'tab', 'ctrl', 'f1', or 'numpad7'. Required for `key_press`."
                },
                "keys": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Ordered key names for `key_combo`, for example [\"ctrl\", \"l\"] or [\"ctrl\", \"shift\", \"t\"]."
                },
                "region": {
                    "type": "object",
                    "description": "Optional screenshot region in global screen pixels.",
                    "properties": {
                        "x": { "type": "integer" },
                        "y": { "type": "integer" },
                        "width": { "type": "integer", "minimum": 1 },
                        "height": { "type": "integer", "minimum": 1 }
                    },
                    "required": ["x", "y", "width", "height"]
                },
                "include_cursor": {
                    "type": "boolean",
                    "description": "Include the cursor in screenshots when the backend supports it. Defaults to false."
                },
                "inline_image": {
                    "type": "boolean",
                    "description": "Only used when MODEL_VISION=0. When vision is enabled, screenshots are attached to the model automatically regardless of this flag."
                },
                "max_depth": { "type": "integer", "minimum": 1, "maximum": 12, "description": "Maximum active-window AT-SPI tree depth for `app_state` (default 5)." },
                "max_nodes": { "type": "integer", "minimum": 1, "maximum": 2000, "description": "Maximum active-window AT-SPI nodes for `app_state` (default 2000)." },
                "path": {
                    "type": "string",
                    "description": "Existing image path for `ocr`, `find_text`, or `click_text`. When omitted, a fresh screenshot is taken."
                },
                "steps": {
                    "type": "array",
                    "description": "Ordered computer actions for `batch`. Each entry is a normal computer action. At most 50 steps.",
                    "items": { "type": "object" }
                },
                "url": {
                    "type": "string",
                    "description": "URL to open with the desktop default browser for `open_url`."
                },
                "title": {
                    "type": "string",
                    "description": "Window title substring to focus for `focus_window`."
                },
                "milliseconds": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 30000,
                    "description": "Wait duration for `wait`. Defaults to 1000 ms."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let action: ComputerAction = serde_json::from_str(input).context(
            "Computer tool input must be a JSON object with an `action` field. See the tool schema for supported actions.",
        )?;

        self.run_action(action).await
    }
}

impl ComputerTool {
    async fn batch(&self, steps: Vec<ComputerAction>) -> Result<ToolResult> {
        if steps.is_empty() {
            return Ok(ToolResult::failure("Batch must contain at least one step."));
        }

        if steps.len() > 50 {
            return Ok(ToolResult::failure("Batch may contain at most 50 steps."));
        }

        let total = steps.len();
        let mut output = Vec::new();

        for (index, step) in steps.into_iter().enumerate() {
            let result = match Box::pin(self.run_action(step)).await {
                Ok(result) => result,
                Err(error) => {
                    output.push(format!(
                        "step {}/{}: execution error\n{}",
                        index + 1,
                        total,
                        error
                    ));
                    return Ok(ToolResult::failure(format!(
                        "Batch stopped after {} completed step(s); later steps were not run.\n{}",
                        index,
                        output.join("\n---\n")
                    )));
                }
            };
            let success = result.success;

            let body = if success {
                result.output.trim().to_string()
            } else {
                result
                    .error
                    .unwrap_or_else(|| "unknown error".to_string())
                    .trim()
                    .to_string()
            };

            output.push(format!(
                "step {}/{}: {}\n{}",
                index + 1,
                total,
                if success { "ok" } else { "failed" },
                body
            ));

            if !success {
                return Ok(ToolResult::failure(output.join("\n---\n")));
            }
        }

        Ok(ToolResult::success(output.join("\n---\n")))
    }

    async fn click_text(
        &self,
        text: &str,
        button: ButtonName,
        path: Option<PathBuf>,
        region: Option<Region>,
        include_cursor: bool,
    ) -> Result<ToolResult> {
        let image = resolve_ocr_path(path, region, include_cursor).await?;
        let words = tesseract_words(&image).await?;
        let query = text.trim().to_lowercase();

        let Some(word) = words
            .iter()
            .find(|word| query.is_empty() || word.text.to_lowercase().contains(&query))
        else {
            return Ok(ToolResult::failure(format!(
                "No OCR text matching '{text}' in {}",
                image.display()
            )));
        };

        let x = word.center_x;
        let y = word.center_y;

        if should_use_portal() {
            #[cfg(target_os = "linux")]
            {
                return self.portal_mouse_click(button, Some(x), Some(y), 1).await;
            }
        }

        mouse_click(button, Some(x), Some(y), 1)
    }

    async fn run_action(&self, action: ComputerAction) -> Result<ToolResult> {
        match action {
            ComputerAction::Status => status().await,

            ComputerAction::Screens => screens(self).await,

            ComputerAction::Cursor => cursor(self).await,

            ComputerAction::FindText {
                text,
                path,
                region,
                include_cursor,
            } => find_text(&text, path, region, include_cursor).await,

            ComputerAction::ClickText {
                text,
                button,
                path,
                region,
                include_cursor,
            } => {
                self.click_text(&text, button, path, region, include_cursor)
                    .await
            }

            ComputerAction::Batch { steps } => self.batch(steps).await,

            ComputerAction::ClipboardSet { text } => clipboard_set(&text).await,

            ComputerAction::ClipboardGet => clipboard_get().await,

            ComputerAction::ClipboardClear => clipboard_clear().await,

            ComputerAction::OpenUrl { url } => open_url(&url).await,

            ComputerAction::FocusWindow { title } => focus_window(&title).await,

            ComputerAction::Screenshot {
                region,
                include_cursor,
                inline_image,
            } => screenshot(region, include_cursor, inline_image).await,

            ComputerAction::AppState {
                region,
                include_cursor,
                inline_image,
                max_depth,
                max_nodes,
            } => {
                app_state(
                    region,
                    include_cursor,
                    inline_image,
                    max_depth.clamp(1, 12),
                    max_nodes.clamp(1, 2000),
                )
                .await
            }

            ComputerAction::Ocr {
                path,
                region,
                include_cursor,
            } => ocr(path, region, include_cursor).await,

            ComputerAction::MouseMove { x, y, relative } => {
                if should_use_portal() {
                    #[cfg(target_os = "linux")]
                    {
                        return self.portal_mouse_move(x, y, relative).await;
                    }
                }

                mouse_move(x, y, relative)
            }

            ComputerAction::MouseClick {
                button,
                x,
                y,
                count,
            } => {
                if should_use_portal() {
                    #[cfg(target_os = "linux")]
                    {
                        return self.portal_mouse_click(button, x, y, count).await;
                    }
                }

                mouse_click(button, x, y, count)
            }

            ComputerAction::MouseDrag { from, to, button } => {
                if should_use_portal() {
                    #[cfg(target_os = "linux")]
                    {
                        return self.portal_mouse_drag(from, to, button).await;
                    }
                }

                mouse_drag(from, to, button)
            }

            ComputerAction::Scroll { axis, amount, x, y } => {
                if should_use_portal() {
                    #[cfg(target_os = "linux")]
                    {
                        return self.portal_scroll(axis, amount, x, y).await;
                    }
                }

                scroll(axis, amount, x, y)
            }

            ComputerAction::TypeText { text } => {
                if should_use_portal() {
                    #[cfg(target_os = "linux")]
                    {
                        return self.portal_type_text(&text).await;
                    }
                }

                type_text(&text)
            }

            ComputerAction::KeyPress { key, count } => {
                if should_use_portal() {
                    #[cfg(target_os = "linux")]
                    {
                        return self.portal_key_press(&key, count).await;
                    }
                }

                key_press(&key, count)
            }

            ComputerAction::KeyCombo { keys, count } => {
                if should_use_portal() {
                    #[cfg(target_os = "linux")]
                    {
                        return self.portal_key_combo(&keys, count).await;
                    }
                }

                key_combo(&keys, count)
            }

            ComputerAction::Wait { milliseconds } => wait(milliseconds).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_keys_case_insensitively() {
        assert_eq!(parse_key("Enter").unwrap(), Key::Return);
        assert_eq!(parse_key("CTRL").unwrap(), Key::Control);
        assert_eq!(parse_key("escape").unwrap(), Key::Escape);
        assert_eq!(parse_key("f12").unwrap(), Key::F12);
        assert_eq!(parse_key("numpad7").unwrap(), Key::Numpad7);
    }

    #[test]
    fn parses_single_character_keys() {
        assert_eq!(parse_key("a").unwrap(), Key::Unicode('a'));
        assert_eq!(parse_key("?").unwrap(), Key::Unicode('?'));
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(parse_key("definitely_not_a_key").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn portal_parses_named_keysyms() {
        assert!(parse_keysym("enter").unwrap() > 0);
        assert!(parse_keysym("ctrl").unwrap() > 0);
        assert_eq!(
            parse_keysym("a").unwrap() as u32,
            xkbcommon::xkb::utf32_to_keysym('a' as u32).raw()
        );
    }

    #[test]
    fn region_formats_grim_geometry() {
        let region = Region {
            x: 1,
            y: 2,
            width: 300,
            height: 400,
        };

        assert_eq!(region.grim_arg(), "1,2 300x400");
    }

    #[test]
    fn schema_advertises_all_actions() {
        let schema = ComputerTool::new().input_schema();
        let actions = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");

        for expected in [
            "status",
            "screens",
            "cursor",
            "find_text",
            "click_text",
            "batch",
            "clipboard_set",
            "clipboard_get",
            "clipboard_clear",
            "open_url",
            "focus_window",
            "screenshot",
            "app_state",
            "ocr",
            "mouse_move",
            "mouse_click",
            "mouse_drag",
            "scroll",
            "type_text",
            "key_press",
            "key_combo",
            "wait",
        ] {
            assert!(
                actions.iter().any(|value| value.as_str() == Some(expected)),
                "missing action {expected}"
            );
        }
    }

    #[tokio::test]
    async fn rejects_unknown_action() {
        let tool = ComputerTool::new();

        assert!(tool
            .execute("{\"action\":\"definitely_not_an_action\"}")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn rejects_negative_absolute_mouse_move_without_touching_desktop() {
        let tool = ComputerTool::new();

        let result = tool
            .execute("{\"action\":\"mouse_move\",\"x\":-1,\"y\":0}")
            .await
            .expect("tool executes");

        assert!(!result.success);
    }

    #[tokio::test]
    async fn rejects_zero_scroll_amount() {
        let tool = ComputerTool::new();

        let result = tool
            .execute("{\"action\":\"scroll\",\"amount\":0}")
            .await
            .expect("tool executes");

        assert!(!result.success);
    }

    #[tokio::test]
    async fn rejects_empty_batch() {
        let tool = ComputerTool::new();

        let result = tool
            .execute("{\"action\":\"batch\",\"steps\":[]}")
            .await
            .expect("tool executes");

        assert!(!result.success);
    }

    #[tokio::test]
    async fn rejects_non_http_open_url() {
        let tool = ComputerTool::new();

        let result = tool
            .execute("{\"action\":\"open_url\",\"url\":\"javascript:alert(1)\"}")
            .await
            .expect("tool executes");

        assert!(!result.success);
    }

    #[tokio::test]
    async fn rejects_empty_focus_window_title() {
        let tool = ComputerTool::new();

        let result = tool
            .execute("{\"action\":\"focus_window\",\"title\":\"\"}")
            .await
            .expect("tool executes");

        assert!(!result.success);
    }

    #[tokio::test]
    async fn find_text_locates_rendered_word() {
        let convert_available = tokio::process::Command::new("convert")
            .arg("--version")
            .output()
            .await
            .is_ok();

        let tesseract_available = tokio::process::Command::new("tesseract")
            .arg("--version")
            .output()
            .await
            .is_ok();

        if !convert_available || !tesseract_available {
            eprintln!("skipping find_text test: ImageMagick/tesseract unavailable");
            return;
        }

        let path = std::env::temp_dir().join(format!("hyusk-find-text-{}.png", std::process::id()));

        let rendered = tokio::process::Command::new("convert")
            .args([
                "-size",
                "500x200",
                "xc:white",
                "-pointsize",
                "32",
                "-fill",
                "black",
                "-annotate",
                "+30+120",
                "Hello World",
                path.to_str().expect("utf-8 path"),
            ])
            .status()
            .await;

        if !matches!(rendered, Ok(status) if status.success()) {
            eprintln!("skipping find_text test: could not render fixture");
            return;
        }

        let result = find_text("Hello", Some(path.clone()), None, false)
            .await
            .expect("find_text executes");

        let _ = tokio::fs::remove_file(&path).await;

        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("center"), "{}", result.output);
    }

    #[tokio::test]
    async fn status_reports_input_capability() {
        let result = status().await.expect("status executes");

        assert!(result.success);
        assert!(result.output.contains("input injection:"));
    }
}
