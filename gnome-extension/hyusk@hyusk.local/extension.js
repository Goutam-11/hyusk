import GObject from 'gi://GObject';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import Meta from 'gi://Meta';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';

const SHELL_DBUS_NAME = 'org.hyusk.Shell';
const SHELL_DBUS_PATH = '/org/hyusk/Shell';
const SHELL_INTERFACE = `
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
  </interface>
</node>`;

const STATE_COLORS = {
    Hidden: [0x9a, 0xa0, 0xa6],
    Waking: [0x82, 0xd2, 0xff],
    Listening: [0x78, 0xe6, 0xaa],
    Thinking: [0xb4, 0xa0, 0xff],
    Working: [0xff, 0xaa, 0x6e],
    Speaking: [0xff, 0x82, 0xc8],
};

const STATE_LABELS = {
    Hidden: 'idle',
    Waking: 'waking',
    Listening: 'listening',
    Thinking: 'thinking',
    Working: 'working',
    Speaking: 'speaking',
};

function statePath() {
    return GLib.build_filenamev([GLib.get_user_runtime_dir(), 'hyusk-state']);
}

function lerp(current, target, amount) {
    return current + (target - current) * amount;
}

const HyuskDrawing = GObject.registerClass(
class HyuskDrawing extends St.DrawingArea {
    _init() {
        super._init({
            style_class: 'hyusk-indicator',
            reactive: false,
        });

        this.set_size(24, 24);

        this._state = 'Hidden';
        this._phase = 0;
        this._color = [...STATE_COLORS.Hidden];

        this.connect('repaint', () => this._draw());

        this._animationId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 33, () => {
            const speed = this._state === 'Working' ? 0.35
                : this._state === 'Waking' ? 0.30
                : this._state === 'Speaking' ? 0.22
                : 0.12;

            this._phase += speed;

            const target = STATE_COLORS[this._state] ?? STATE_COLORS.Hidden;
            for (let index = 0; index < 3; index++)
                this._color[index] = lerp(this._color[index], target[index], 0.12);

            this.queue_repaint();
            return GLib.SOURCE_CONTINUE;
        });
    }

    setState(state) {
        if (!STATE_COLORS[state] || state === this._state)
            return;

        this._state = state;
        this.accessible_name = `Hyusk: ${STATE_LABELS[state] ?? state}`;
    }

    _draw() {
        const context = this.get_context();
        const [width, height] = this.get_surface_size();

        const red = this._color[0] / 255;
        const green = this._color[1] / 255;
        const blue = this._color[2] / 255;

        context.save();

        const centerX = width / 2;
        const centerY = height / 2;

        const speaking = this._state === 'Speaking';
        const thinking = this._state === 'Thinking';
        const waking = this._state === 'Waking';

        const bounce = speaking ? Math.sin(this._phase) * 1.4 : 0;
        const tilt = thinking ? Math.sin(this._phase) * 0.18 : 0;
        const pulse = 1 + Math.sin(this._phase * (waking ? 2 : 1)) * (waking ? 0.12 : 0.05);
        const flap = 1 + Math.sin(this._phase) * (this._state === 'Working' ? 0.30 : 0.18);

        context.translate(centerX, centerY + bounce);
        context.rotate(tilt);
        context.scale(pulse, pulse);

        const alpha = this._state === 'Hidden' ? 0.55 : 1.0;

        // Upper wings.
        context.setSourceRGBA(red, green, blue, alpha);

        context.newPath();
        context.moveTo(0, -0.5);
        context.curveTo(-3 * flap, -10 * flap, -13, -10 * flap, -9.5, -1);
        context.curveTo(-14, 2, -6, 4, 0, 1);
        context.closePath();
        context.fill();

        context.newPath();
        context.moveTo(0, -0.5);
        context.curveTo(3 * flap, -10 * flap, 13, -10 * flap, 9.5, -1);
        context.curveTo(14, 2, 6, 4, 0, 1);
        context.closePath();
        context.fill();

        // Lower wings, slightly darker.
        context.setSourceRGBA(red * 0.78, green * 0.78, blue * 0.78, alpha);

        context.newPath();
        context.moveTo(0, 1);
        context.curveTo(-4, 5, -9, 8, -5.5, 3.5);
        context.curveTo(-6.5, 1.5, -3, 1, 0, 1.5);
        context.closePath();
        context.fill();

        context.newPath();
        context.moveTo(0, 1);
        context.curveTo(4, 5, 9, 8, 5.5, 3.5);
        context.curveTo(6.5, 1.5, 3, 1, 0, 1.5);
        context.closePath();
        context.fill();

        // Body.
        context.setSourceRGBA(1, 1, 1, alpha);
        context.newPath();
        context.arc(0, 0, 1.8, 0, 2 * Math.PI);
        context.fill();

        // Antennae.
        context.setSourceRGBA(red, green, blue, alpha);
        context.setLineWidth(0.9);

        context.newPath();
        context.moveTo(-0.8, -1.5);
        context.curveTo(-2.4, -4, -3.2, -5.5, -1.8, -6.5);
        context.stroke();

        context.newPath();
        context.moveTo(0.8, -1.5);
        context.curveTo(2.4, -4, 3.2, -5.5, 1.8, -6.5);
        context.stroke();

        context.restore();
        context.$dispose();
    }

    destroy() {
        if (this._animationId) {
            GLib.source_remove(this._animationId);
            this._animationId = 0;
        }

        super.destroy();
    }
});

const HyuskIndicator = GObject.registerClass(
class HyuskIndicator extends PanelMenu.Button {
    _init() {
        super._init(0.0, 'Hyusk', false);

        this._drawing = new HyuskDrawing();
        this.add_child(this._drawing);

        this._lastState = '';

        this._stateTimeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 500, () => {
            this._refresh();
            return GLib.SOURCE_CONTINUE;
        });

        this._refresh();
    }

    _refresh() {
        try {
            const [ok, contents] = GLib.file_get_contents(statePath());
            if (!ok)
                return;

            const state = new TextDecoder().decode(contents).trim();
            if (!state || state === this._lastState)
                return;

            this._lastState = state;
            this._drawing.setState(state);
        } catch (_error) {
            // The state file is optional; ignore transient read errors.
        }
    }

    destroy() {
        if (this._stateTimeout) {
            GLib.source_remove(this._stateTimeout);
            this._stateTimeout = 0;
        }

        this._drawing?.destroy();
        this._drawing = null;

        super.destroy();
    }
});

export default class HyuskIndicatorExtension extends Extension {
    enable() {
        this._indicator = new HyuskIndicator();
        Main.panel.addToStatusArea('hyusk-indicator', this._indicator, 0, 'right');

        this._exportShell();
    }

    disable() {
        this._indicator?.destroy();
        this._indicator = null;

        this._unexportShell();
    }

    _exportShell() {
        const impl = {
            ListWindows: () => this._listWindows(),
            ActiveWindow: () => this._activeWindow(),
            ActivateWindow: query => this._activateWindow(query),
        };

        this._dbus = Gio.DBusExportedObject.wrapJSObject(SHELL_INTERFACE, impl);
        this._dbus.export(Gio.DBus.session, SHELL_DBUS_PATH);

        this._nameId = Gio.bus_own_name(
            Gio.BusType.SESSION,
            SHELL_DBUS_NAME,
            Gio.BusNameOwnerFlags.NONE,
            null,
            null,
            null
        );
    }

    _unexportShell() {
        this._dbus?.unexport();
        this._dbus = null;

        if (this._nameId) {
            Gio.bus_unown_name(this._nameId);
            this._nameId = 0;
        }
    }

    _normalWindows() {
        return global
            .get_window_actors()
            .map(actor => actor.meta_window)
            .filter(win => {
                if (!win)
                    return false;

                try {
                    return win.get_window_type() === Meta.WindowType.NORMAL;
                } catch (_error) {
                    return false;
                }
            });
    }

    _listWindows() {
        const windows = this._normalWindows().map(win => ({
            title: this._title(win),
            app: this._app(win),
            active: this._focused(win),
        }));

        return JSON.stringify(windows);
    }

    _activeWindow() {
        const win = global.display.focus_window;

        if (!win)
            return JSON.stringify({});

        return JSON.stringify({
            title: this._title(win),
            app: this._app(win),
            active: true,
        });
    }

    _activateWindow(query) {
        const needle = String(query ?? '').toLowerCase().trim();

        if (!needle)
            return JSON.stringify({ ok: false, error: 'empty query' });

        const windows = this._normalWindows();

        // Prefer a window whose application name matches, then any title match.
        const match =
            windows.find(win => this._app(win).toLowerCase() === needle) ??
            windows.find(win => this._app(win).toLowerCase().includes(needle)) ??
            windows.find(win => this._title(win).toLowerCase().includes(needle));

        if (!match)
            return JSON.stringify({ ok: false, error: `no window matching '${query}'` });

        Main.activateWindow(match, global.get_current_time());

        return JSON.stringify({
            ok: true,
            title: this._title(match),
            app: this._app(match),
        });
    }

    _title(win) {
        try {
            return win.get_title() ?? '';
        } catch (_error) {
            return '';
        }
    }

    _app(win) {
        try {
            return win.get_wm_class() ?? '';
        } catch (_error) {
            return '';
        }
    }

    _focused(win) {
        try {
            return win.has_focus();
        } catch (_error) {
            return false;
        }
    }
}
