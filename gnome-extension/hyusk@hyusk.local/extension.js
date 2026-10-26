// @ts-check
//
// Hyusk GNOME Shell extension entry point. The UI lives in ./lib:
//   lib/indicator.js    panel button and status polling
//   lib/drawing.js      animated butterfly and state aura
//   lib/responseCard.js floating reply / approval card
//   lib/menu/*          indicator menu and its submenus
//   lib/windows.js      D-Bus window control for the agent
//   lib/runtime.js      runtime and config file access

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';

import { HyuskIndicator } from './lib/indicator.js';
import {
    SHELL_DBUS_NAME,
    SHELL_DBUS_PATH,
    SHELL_INTERFACE,
    WindowControl,
} from './lib/windows.js';

export default class HyuskIndicatorExtension extends Extension {
    enable() {
        this._indicator = new HyuskIndicator(
            GLib.build_filenamev([this.path, 'hyusk_butterfly_mark.png'])
        );
        Main.panel.addToStatusArea('hyusk-indicator', this._indicator, 0, 'right');

        this._exportShell();
    }

    disable() {
        this._indicator?.destroy();
        this._indicator = null;

        this._unexportShell();
    }

    _exportShell() {
        const service = new WindowControl();
        const impl = {
            ListWindows: () => service.listWindows(),
            ActiveWindow: () => service.activeWindow(),
            ActivateWindow: (/** @type {string} */ query) => service.activateWindow(query),
            CloseWindow: (/** @type {string} */ query) => service.closeWindow(query),
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
}
