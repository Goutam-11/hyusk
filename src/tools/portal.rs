//! GNOME/Wayland input and screenshot support through the XDG desktop portals.
//!
//! The compositor does not expose virtual keyboard/pointer protocols to
//! ordinary clients, so `enigo` can fall back to XWayland and silently ignore
//! synthetic input. The supported path is
//! `org.freedesktop.portal.RemoteDesktop`, which creates one user-approved
//! remote-control session and then accepts direct `Notify*` calls for pointer
//! and keyboard events.
//!
//! The screenshot portal is used as the final fallback for screen capture.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use ashpd::desktop::{
    remote_desktop::{
        Axis, DeviceType, KeyState, NotifyKeyboardKeycodeOptions, NotifyKeyboardKeysymOptions,
        NotifyPointerAxisDiscreteOptions, NotifyPointerButtonOptions,
        NotifyPointerMotionAbsoluteOptions, NotifyPointerMotionOptions, RemoteDesktop,
        SelectDevicesOptions, StartOptions,
    },
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
    screenshot::Screenshot,
    CreateSessionOptions, PersistMode, Session,
};
use enumflags2::BitFlags;

/// A live, user-approved RemoteDesktop portal session.
pub struct PortalInput {
    proxy: RemoteDesktop,
    session: Session<RemoteDesktop>,
    keyboard: bool,
    pointer: bool,
    position: Option<(f64, f64)>,
    stream_id: Option<u32>,
    stream_position: Option<(i32, i32)>,
    stream_size: Option<(i32, i32)>,
}

impl PortalInput {
    /// Create and start a RemoteDesktop session.
    ///
    /// The first call causes the desktop portal to show a permission dialog.
    /// The approval is persisted for the lifetime of the application through
    /// [`PersistMode::Application`].
    pub async fn connect() -> Result<Self> {
        let proxy = RemoteDesktop::new()
            .await
            .context("The RemoteDesktop portal is not available")?;

        let session = proxy
            .create_session(CreateSessionOptions::default())
            .await
            .context("Failed to create a RemoteDesktop portal session")?;

        proxy
            .select_devices(
                &session,
                SelectDevicesOptions::default()
                    .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                    .set_persist_mode(PersistMode::Application),
            )
            .await
            .context("Failed to select keyboard/pointer devices in the portal session")?;

        /*
         * Combine the RemoteDesktop session with a monitor ScreenCast so
         * absolute pointer notifications have the PipeWire stream they need.
         */
        let screencast = Screencast::new()
            .await
            .context("The ScreenCast portal is not available")?;

        screencast
            .select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_sources(BitFlags::from(SourceType::Monitor))
                    .set_cursor_mode(CursorMode::Embedded)
                    .set_multiple(false)
                    .set_persist_mode(PersistMode::DoNot),
            )
            .await
            .context("Failed to request a monitor stream in the portal session")?;

        let response = proxy
            .start(&session, None, StartOptions::default())
            .await
            .context("Failed to start the RemoteDesktop portal session")?
            .response()
            .context("The RemoteDesktop portal request was denied or cancelled by the user")?;

        let devices = response.devices();

        let keyboard = devices.contains(DeviceType::Keyboard);
        let pointer = devices.contains(DeviceType::Pointer);

        if !keyboard && !pointer {
            bail!("The portal session did not grant keyboard or pointer access");
        }

        let stream = response.streams().first();

        let stream_id = stream.map(|stream| stream.pipe_wire_node_id());
        let stream_position = stream.and_then(|stream| stream.position());
        let stream_size = stream.and_then(|stream| stream.size());

        println!(
            "[Portal] RemoteDesktop session approved (keyboard={keyboard}, pointer={pointer}, stream={stream_id:?})"
        );

        Ok(Self {
            proxy,
            session,
            keyboard,
            pointer,
            position: None,
            stream_id,
            stream_position,
            stream_size,
        })
    }

    /// Whether a pointer position hint is available for absolute moves.
    pub fn has_position(&self) -> bool {
        self.position.is_some()
    }

    /// Seed the pointer position so absolute moves can be translated into
    /// relative portal notifications.
    pub fn set_position_hint(&mut self, x: i32, y: i32) {
        self.position = Some((x as f64, y as f64));
    }

    pub fn screen_size(&self) -> Option<(i32, i32)> {
        self.stream_size
    }

    pub fn screen_position(&self) -> Option<(i32, i32)> {
        self.stream_position
    }

    pub fn tracked_position(&self) -> Option<(f64, f64)> {
        self.position
    }

    /// Move to an absolute position.
    ///
    /// When the session has a monitor stream, the portal's absolute
    /// notification is used directly. Otherwise the move is translated into a
    /// relative delta using the tracked position.
    pub async fn move_absolute(&mut self, x: f64, y: f64) -> Result<()> {
        self.require_pointer()?;

        if let Some(stream_id) = self.stream_id {
            let (offset_x, offset_y) = self.stream_position.unwrap_or((0, 0));

            self.proxy
                .notify_pointer_motion_absolute(
                    &self.session,
                    stream_id,
                    x - offset_x as f64,
                    y - offset_y as f64,
                    NotifyPointerMotionAbsoluteOptions::default(),
                )
                .await
                .map_err(portal_error)?;

            self.position = Some((x, y));
            return Ok(());
        }

        if let Some((current_x, current_y)) = self.position {
            self.move_relative(x - current_x, y - current_y).await?;
            self.position = Some((x, y));
            return Ok(());
        }

        bail!(
            "The portal session has no monitor stream and no pointer position hint; absolute moves are unavailable"
        );
    }

    pub async fn move_relative(&mut self, dx: f64, dy: f64) -> Result<()> {
        self.require_pointer()?;

        self.proxy
            .notify_pointer_motion(&self.session, dx, dy, NotifyPointerMotionOptions::default())
            .await
            .map_err(portal_error)?;

        if let Some((x, y)) = self.position {
            self.position = Some((x + dx, y + dy));
        }

        Ok(())
    }

    pub async fn button(&mut self, code: i32, pressed: bool) -> Result<()> {
        self.require_pointer()?;

        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };

        self.proxy
            .notify_pointer_button(
                &self.session,
                code,
                state,
                NotifyPointerButtonOptions::default(),
            )
            .await
            .map_err(portal_error)
    }

    pub async fn scroll(&mut self, axis: Axis, steps: i32) -> Result<()> {
        self.require_pointer()?;

        self.proxy
            .notify_pointer_axis_discrete(
                &self.session,
                axis,
                steps,
                NotifyPointerAxisDiscreteOptions::default(),
            )
            .await
            .map_err(portal_error)
    }

    pub async fn keysym(&mut self, keysym: i32, pressed: bool) -> Result<()> {
        self.require_keyboard()?;

        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };

        self.proxy
            .notify_keyboard_keysym(
                &self.session,
                keysym,
                state,
                NotifyKeyboardKeysymOptions::default(),
            )
            .await
            .map_err(portal_error)
    }

    pub async fn keycode(&mut self, keycode: i32, pressed: bool) -> Result<()> {
        self.require_keyboard()?;

        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };

        self.proxy
            .notify_keyboard_keycode(
                &self.session,
                keycode,
                state,
                NotifyKeyboardKeycodeOptions::default(),
            )
            .await
            .map_err(portal_error)
    }

    /// Press and release an evdev keycode, holding Shift when requested.
    pub async fn keycode_click(&mut self, keycode: i32, shift: bool) -> Result<()> {
        if shift {
            self.keycode(KEY_LEFTSHIFT, true).await?;
        }

        self.keycode(keycode, true).await?;
        self.keycode(keycode, false).await?;

        if shift {
            self.keycode(KEY_LEFTSHIFT, false).await?;
        }

        Ok(())
    }

    pub async fn key_click(&mut self, keysym: i32) -> Result<()> {
        self.keysym(keysym, true).await?;
        self.keysym(keysym, false).await
    }

    pub async fn type_text(&mut self, text: &str) -> Result<()> {
        self.require_keyboard()?;

        if let Some(plan) = evdev_text(text) {
            for (keycode, shift) in plan {
                self.keycode_click(keycode, shift).await?;
            }

            return Ok(());
        }

        for character in text.chars() {
            let keysym = xkbcommon::xkb::utf32_to_keysym(character as u32).raw() as i32;

            if keysym == 0 {
                continue;
            }

            self.key_click(keysym).await?;
        }

        Ok(())
    }

    /// Press a sequence of evdev keys as a combo (modifiers held).
    pub async fn evdev_combo(&mut self, keys: &[(i32, bool)]) -> Result<()> {
        self.require_keyboard()?;

        let mut modifiers = Vec::new();
        let mut presses = Vec::new();

        for (keycode, shift) in keys {
            if is_modifier_keycode(*keycode) {
                modifiers.push(*keycode);
            } else {
                presses.push((*keycode, *shift));
            }
        }

        if presses.is_empty() {
            bail!("Key combo must include at least one non-modifier key");
        }

        for modifier in &modifiers {
            self.keycode(*modifier, true).await?;
        }

        for (keycode, shift) in &presses {
            self.keycode_click(*keycode, *shift).await?;
        }

        for modifier in modifiers.iter().rev() {
            self.keycode(*modifier, false).await?;
        }

        Ok(())
    }

    pub async fn key_combo(&mut self, keysyms: &[i32]) -> Result<()> {
        self.require_keyboard()?;

        let mut modifiers = Vec::new();
        let mut presses = Vec::new();

        for keysym in keysyms {
            if is_modifier_keysym(*keysym) {
                modifiers.push(*keysym);
            } else {
                presses.push(*keysym);
            }
        }

        if presses.is_empty() {
            bail!("Key combo must include at least one non-modifier key");
        }

        for modifier in &modifiers {
            self.keysym(*modifier, true).await?;
        }

        for key in &presses {
            self.key_click(*key).await?;
        }

        for modifier in modifiers.iter().rev() {
            self.keysym(*modifier, false).await?;
        }

        Ok(())
    }

    fn require_keyboard(&self) -> Result<()> {
        if !self.keyboard {
            bail!("The portal session did not grant keyboard access");
        }

        Ok(())
    }

    fn require_pointer(&self) -> Result<()> {
        if !self.pointer {
            bail!("The portal session did not grant pointer access");
        }

        Ok(())
    }
}

/// Query the RemoteDesktop portal version without opening a session or
/// prompting the user.
pub async fn portal_version() -> Result<u32> {
    let proxy = RemoteDesktop::new()
        .await
        .context("The RemoteDesktop portal is not available")?;

    Ok(proxy.version())
}

/// Take a full-screen screenshot through the desktop portal.
///
/// The portal may show a permission dialog. The returned value is a `file://`
/// URI that can be converted with [`uri_to_path`].
pub async fn screenshot_uri() -> Result<String> {
    let response = Screenshot::request()
        .interactive(false)
        .modal(false)
        .send()
        .await
        .context("Failed to call the Screenshot portal")?
        .response()
        .context("The Screenshot portal request was denied or cancelled")?;

    Ok(response.uri().as_str().to_string())
}

/// Convert a `file://` URI returned by the portal into a local path.
pub fn uri_to_path(uri: &str) -> Result<PathBuf> {
    let path = uri.strip_prefix("file://").unwrap_or(uri);

    if path.is_empty() {
        bail!("The portal returned an empty screenshot URI");
    }

    Ok(PathBuf::from(percent_decode(path)))
}

fn portal_error(error: ashpd::Error) -> anyhow::Error {
    anyhow!("Desktop portal error: {error}")
}

fn is_modifier_keysym(keysym: i32) -> bool {
    matches!(
        keysym as u32,
        0xffe1..=0xffec // Shift, Control, Caps Lock, Meta, Alt, Super, Hyper
            | 0xfe03 // ISO_Level3_Shift
            | 0xff7e..=0xff7f // Mode_switch, Num_Lock
    )
}

const KEY_LEFTSHIFT: i32 = 42;
const KEY_LEFTCTRL: i32 = 29;
const KEY_LEFTALT: i32 = 56;
const KEY_LEFTMETA: i32 = 125;
const KEY_RIGHTCTRL: i32 = 97;
const KEY_RIGHTALT: i32 = 100;
const KEY_RIGHTMETA: i32 = 126;
const KEY_CAPSLOCK: i32 = 58;

fn is_modifier_keycode(keycode: i32) -> bool {
    matches!(
        keycode,
        KEY_LEFTSHIFT
            | 54 // right shift
            | KEY_LEFTCTRL
            | KEY_RIGHTCTRL
            | KEY_LEFTALT
            | KEY_RIGHTALT
            | KEY_LEFTMETA
            | KEY_RIGHTMETA
            | KEY_CAPSLOCK
    )
}

/// Translate a friendly key name to (evdev keycode, shift needed).
pub fn evdev_key(name: &str) -> Option<(i32, bool)> {
    let lowered = name.trim().to_ascii_lowercase();

    let value = match lowered.as_str() {
        "enter" | "return" => (28, false),
        "escape" | "esc" => (1, false),
        "backspace" => (14, false),
        "tab" => (15, false),
        "space" | "spacebar" => (57, false),
        "delete" | "del" => (111, false),
        "home" => (102, false),
        "end" => (107, false),
        "pageup" | "page_up" | "pgup" => (104, false),
        "pagedown" | "page_down" | "pgdn" => (109, false),
        "insert" | "ins" => (110, false),
        "left" | "left_arrow" | "arrowleft" | "arrow_left" => (105, false),
        "right" | "right_arrow" | "arrowright" | "arrow_right" => (106, false),
        "up" | "up_arrow" | "arrowup" | "arrow_up" => (103, false),
        "down" | "down_arrow" | "arrowdown" | "arrow_down" => (108, false),
        "shift" => (KEY_LEFTSHIFT, false),
        "ctrl" | "control" => (KEY_LEFTCTRL, false),
        "alt" | "option" => (KEY_LEFTALT, false),
        "meta" | "super" | "windows" | "win" | "cmd" | "command" => (KEY_LEFTMETA, false),
        "capslock" | "caps_lock" => (KEY_CAPSLOCK, false),
        "f1" => (59, false),
        "f2" => (60, false),
        "f3" => (61, false),
        "f4" => (62, false),
        "f5" => (63, false),
        "f6" => (64, false),
        "f7" => (65, false),
        "f8" => (66, false),
        "f9" => (67, false),
        "f10" => (68, false),
        "f11" => (87, false),
        "f12" => (88, false),
        "numpad0" => (82, false),
        "numpad1" => (79, false),
        "numpad2" => (80, false),
        "numpad3" => (81, false),
        "numpad4" => (75, false),
        "numpad5" => (76, false),
        "numpad6" => (77, false),
        "numpad7" => (71, false),
        "numpad8" => (72, false),
        "numpad9" => (73, false),
        _ => {
            let mut chars = lowered.chars();

            if let (Some(character), None) = (chars.next(), chars.next()) {
                return evdev_for_char(character);
            }

            return None;
        }
    };

    Some(value)
}

fn evdev_text(text: &str) -> Option<Vec<(i32, bool)>> {
    let mut plan = Vec::with_capacity(text.len());

    for character in text.chars() {
        plan.push(evdev_for_char(character)?);
    }

    Some(plan)
}

fn evdev_for_char(character: char) -> Option<(i32, bool)> {
    let value = match character {
        'a'..='z' => (letter_code(character), false),
        'A'..='Z' => (letter_code(character.to_ascii_lowercase()), true),
        '0' => (11, false),
        '1'..='9' => (character as i32 - '1' as i32 + 2, false),
        ' ' => (57, false),
        '\n' => (28, false),
        '\t' => (15, false),
        '-' => (12, false),
        '_' => (12, true),
        '=' => (13, false),
        '+' => (13, true),
        '[' => (26, false),
        '{' => (26, true),
        ']' => (27, false),
        '}' => (27, true),
        ';' => (39, false),
        ':' => (39, true),
        '\'' => (40, false),
        '"' => (40, true),
        '`' => (41, false),
        '~' => (41, true),
        '\\' => (43, false),
        '|' => (43, true),
        ',' => (51, false),
        '<' => (51, true),
        '.' => (52, false),
        '>' => (52, true),
        '/' => (53, false),
        '?' => (53, true),
        '!' => (2, true),
        '@' => (3, true),
        '#' => (4, true),
        '$' => (5, true),
        '%' => (6, true),
        '^' => (7, true),
        '&' => (8, true),
        '*' => (9, true),
        '(' => (10, true),
        ')' => (11, true),
        _ => return None,
    };

    Some(value)
}

fn letter_code(character: char) -> i32 {
    match character {
        'a' => 30,
        'b' => 48,
        'c' => 46,
        'd' => 32,
        'e' => 18,
        'f' => 33,
        'g' => 34,
        'h' => 35,
        'i' => 23,
        'j' => 36,
        'k' => 37,
        'l' => 38,
        'm' => 50,
        'n' => 49,
        'o' => 24,
        'p' => 25,
        'q' => 16,
        'r' => 19,
        's' => 31,
        't' => 20,
        'u' => 22,
        'v' => 47,
        'w' => 17,
        'x' => 45,
        'y' => 21,
        'z' => 44,
        _ => 0,
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                output.push(high * 16 + low);
                index += 3;
                continue;
            }
        }

        output.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&output).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_percent_encoded_file_uri() {
        let path = uri_to_path("file:///home/user/My%20Pictures/shot.png").unwrap();

        assert_eq!(path, PathBuf::from("/home/user/My Pictures/shot.png"));
    }

    #[test]
    fn rejects_empty_uri() {
        assert!(uri_to_path("file://").is_err());
    }

    #[test]
    fn maps_ascii_text_to_evdev() {
        let plan = evdev_text("goat by diljit").expect("ascii plan");

        assert_eq!(plan.len(), "goat by diljit".len());
    }

    #[test]
    fn maps_named_keys_to_evdev() {
        assert_eq!(evdev_key("enter"), Some((28, false)));
        assert_eq!(evdev_key("ctrl"), Some((29, false)));
        assert_eq!(evdev_key("f12"), Some((88, false)));
    }

    #[tokio::test]
    #[ignore = "opens a desktop portal permission dialog"]
    async fn portal_relative_move_smoke() {
        let mut input = PortalInput::connect().await.expect("portal session");

        input.move_relative(1.0, 0.0).await.expect("move right");
        input.move_relative(-1.0, 0.0).await.expect("move back");
    }

    #[tokio::test]
    #[ignore = "opens a desktop portal permission dialog and moves the pointer"]
    async fn portal_absolute_move_smoke() {
        let mut input = PortalInput::connect().await.expect("portal session");

        let (width, height) = input.screen_size().unwrap_or((1920, 1080));

        input
            .move_absolute(120.0, 120.0)
            .await
            .expect("move absolute top-left");

        input
            .move_absolute((width / 2) as f64, (height / 2) as f64)
            .await
            .expect("move absolute center");
    }

    #[tokio::test]
    #[ignore = "opens a desktop portal permission dialog and sends a harmless modifier"]
    async fn portal_keyboard_smoke() {
        let mut input = PortalInput::connect().await.expect("portal session");

        input
            .keycode_click(KEY_LEFTSHIFT, false)
            .await
            .expect("send shift keycode");
    }

    #[tokio::test]
    #[ignore = "opens a desktop portal screenshot permission dialog"]
    async fn portal_screenshot_smoke() {
        let uri = screenshot_uri().await.expect("screenshot");
        let path = uri_to_path(&uri).expect("screenshot path");

        assert!(
            path.exists(),
            "screenshot file should exist: {}",
            path.display()
        );
    }
}
