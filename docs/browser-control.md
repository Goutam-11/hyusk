# Browser control

Hyusk can control a dedicated Brave, Chromium, or Chrome profile through the local Chrome DevTools Protocol (CDP). It does not attach to your normal browser profile. Start the separate profile before using the `browser` tool:

```sh
scripts/start-browser-control.sh
```

The script detects Brave, Chromium, or Chrome, including Flatpak Brave (`com.brave.Browser`) and Chrome (`com.google.Chrome`). Set `HYUSK_BROWSER_BIN` to choose a native executable path, `HYUSK_BROWSER_PROFILE` to choose the dedicated profile directory, or `HYUSK_BROWSER_CDP_PORT` to choose the debugging port (default `9222`). For Flatpak, the default profile is stored under that app's own `~/.var/app/<application-id>/data/` directory. Other installs use `$XDG_DATA_HOME/hyusk/browser-control-profile`, or `~/.local/share/hyusk/browser-control-profile` when `XDG_DATA_HOME` is unset. This profile has its own cookies and sign-ins, so sign in there if a task requires it. The launcher reuses a responding configured endpoint; otherwise it starts `hyusk-browser-control.service` independently of the voice service and waits for readiness. It does not kill another browser. Without systemd, run it with `--worker` in a separate terminal.

The tool connects only to a loopback HTTP CDP endpoint and bypasses HTTP proxies. By default it uses `http://127.0.0.1:9222`. You can set `HYUSK_BROWSER_CDP_URL` to a root loopback URL; the launcher derives its port from that URL. If you set both `HYUSK_BROWSER_CDP_URL` and `HYUSK_BROWSER_CDP_PORT`, their ports must match. Returned WebSocket addresses must also be loopback and use the configured port. If the profile is not running, the tool returns a setup hint to run the script.

Actions are typed: `tabs`, `read`, `navigate`, `click`, `fill`, and `press_key`. Mutating actions return a fresh snapshot with page title, URL, bounded visible text, and a limited list of interactive controls with CSS selectors. Snapshots and selectors cover the top-level document only; CSS selectors do not cross iframe boundaries. Navigation accepts only absolute HTTP and HTTPS URLs. The tool does not accept page JavaScript from the model. Clicks use CDP mouse input after checking the target is enabled, visible, and not covered at its click point. Filling is limited to enabled, writable inputs, textareas, and editable regions. Supported key names are listed in the tool schema.

Recovery actions are `status`, `start`, and `new_tab`. Status checks the browser-level WebSocket, not just HTTP tab metadata. Start opens/reuses the managed browser and verifies the connection. New tab returns a fresh target ID without closing an unresponsive or unsaved tab. Never recover with broad `pkill`/`killall` commands. If a fresh tab also fails, use the desktop fallback or report the exact error instead of opening more tabs repeatedly.

Each CDP RPC has one bounded deadline across connection, send, and response (5 seconds for reads/health checks, 12 seconds for other commands). Event traffic cannot extend it. Failures include the CDP method, target, and phase; failed page calls are logged with elapsed time. A dispatched action followed by an observation failure is explicitly uncertain: inspect state rather than repeat a potentially completed click or submission.

If a provider explicitly rejects image input, the text-model loop retries once without images, keeps the assistant/tool message ordering, and stops attaching further screenshots for that selection. It can continue through DOM, accessibility, or OCR. Selecting a model again resets the capability assumption.

The implementation follows the CDP [DOM](https://chromedevtools.github.io/devtools-protocol/tot/DOM/), [Input](https://chromedevtools.github.io/devtools-protocol/tot/Input/), and [Runtime](https://chromedevtools.github.io/devtools-protocol/tot/Runtime/) domains. Chrome requires a non-default user-data directory for remote debugging in current releases; the launcher supplies its own profile directory. Firefox uses WebDriver BiDi rather than CDP and is not supported by this tool.
