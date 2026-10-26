// @ts-check
//
// Native window list / activate / close over D-Bus. The agent uses this on
// Wayland, where it cannot enumerate windows itself.

import Meta from 'gi://Meta';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

export const SHELL_DBUS_NAME = 'org.hyusk.Shell';
export const SHELL_DBUS_PATH = '/org/hyusk/Shell';
export const SHELL_INTERFACE = `
<node>
  <interface name="org.hyusk.Shell">
    <method name="ListWindows">
      <arg type="s" direction="out"/>
    </method>
    <method name="ActiveWindow">
      <arg type="s" direction="out"/>
    </method>
    <method name="ActivateWindow">
      <arg type="s" direction="in"/>
      <arg type="s" direction="out"/>
    </method>
    <method name="CloseWindow">
      <arg type="s" direction="in"/>
      <arg type="s" direction="out"/>
    </method>
  </interface>
</node>`;

/**
 * @typedef {object} WindowInfo
 * @property {number} id
 * @property {string} title
 * @property {string} app
 * @property {number|null} pid
 * @property {{x: number, y: number, width: number, height: number}|null} bounds
 * @property {boolean} minimized
 * @property {boolean} active
 * @property {number|null} workspace
 * @property {boolean} on_current_workspace
 * @property {number} monitor
 * @property {boolean} fullscreen
 */

export class WindowControl {
    /** @returns {string} */
    listWindows() {
        return JSON.stringify(this._normalWindows().map(win => this._windowInfo(win)));
    }

    /** @returns {string} */
    activeWindow() {
        const win = global.display.focus_window;
        if (!win)
            return JSON.stringify({});

        return JSON.stringify(this._windowInfo(win));
    }

    /** @param {string} query */
    activateWindow(query) {
        const match = this._matchWindow(query);
        if (!match)
            return JSON.stringify({ ok: false, error: `no window matching '${query}'` });

        try {
            if (Main.overview.visible)
                Main.overview.hide();
            if (match.minimized)
                match.unminimize();
            Main.activateWindow(match, global.get_current_time());
        } catch (error) {
            return JSON.stringify({ ok: false, error: `could not activate window: ${error.message}` });
        }

        return JSON.stringify({
            ok: true,
            id: match.get_id(),
            title: this._title(match),
            app: this._app(match),
        });
    }

    /** @param {string} query */
    closeWindow(query) {
        const match = this._matchWindow(query);
        if (!match)
            return JSON.stringify({ ok: false, error: `no window matching '${query}'` });

        const title = this._title(match);
        const app = this._app(match);
        match.delete(global.get_current_time());

        return JSON.stringify({ ok: true, id: match.get_id(), title, app });
    }

    /** @returns {any[]} */
    _normalWindows() {
        return global
            .get_window_actors()
            .map(/** @param {any} actor */ actor => actor.meta_window)
            .filter(/** @param {any} win */ win => {
                if (!win)
                    return false;

                try {
                    // File pickers, permission prompts, and other dialogs
                    // need to be targetable just like ordinary app windows.
                    return [
                        Meta.WindowType.NORMAL,
                        Meta.WindowType.DIALOG,
                        Meta.WindowType.MODAL_DIALOG,
                        Meta.WindowType.UTILITY,
                    ].includes(win.get_window_type());
                } catch (_error) {
                    return false;
                }
            });
    }

    /**
     * @param {any} win
     * @returns {WindowInfo}
     */
    _windowInfo(win) {
        /** @type {WindowInfo['bounds']} */
        let bounds = null;
        try {
            const rect = win.get_frame_rect();
            bounds = { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
        } catch (_error) { /* Some transient windows have no frame yet. */ }

        return {
            id: win.get_id(),
            title: this._title(win),
            app: this._app(win),
            pid: win.get_pid?.() ?? null,
            bounds,
            minimized: win.minimized ?? false,
            active: this._focused(win),
            workspace: win.get_workspace()?.index() ?? null,
            on_current_workspace: win.get_workspace()?.index() ===
                global.workspace_manager.get_active_workspace_index(),
            monitor: win.get_monitor(),
            fullscreen: win.is_fullscreen(),
        };
    }

    /**
     * @param {string} query
     * @returns {any|null}
     */
    _matchWindow(query) {
        const needle = String(query ?? '').toLowerCase().trim();
        const windows = this._normalWindows();

        if (!needle)
            return global.display.focus_window ?? windows[0] ?? null;

        // Stable IDs from list/active avoid selecting the wrong window when
        // several windows share an app name or title.
        if (/^id:\d+$/.test(needle)) {
            const id = Number(needle.slice(3));
            return windows.find(win => win.get_id() === id) ?? null;
        }

        // Prefer an exact application-name match, then app substring, then
        // window-title substring.
        return (
            windows.find(win => this._app(win).toLowerCase() === needle) ??
            windows.find(win => this._app(win).toLowerCase().includes(needle)) ??
            windows.find(win => this._title(win).toLowerCase().includes(needle)) ??
            null
        );
    }

    /** @param {any} win */
    _title(win) {
        try {
            return win.get_title() ?? '';
        } catch (_error) {
            return '';
        }
    }

    /** @param {any} win */
    _app(win) {
        try {
            return win.get_wm_class() ?? '';
        } catch (_error) {
            return '';
        }
    }

    /** @param {any} win */
    _focused(win) {
        try {
            return win.has_focus();
        } catch (_error) {
            return false;
        }
    }
}
