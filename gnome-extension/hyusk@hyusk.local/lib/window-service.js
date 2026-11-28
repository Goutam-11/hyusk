// @ts-check
//
// The shell-side window service: the agent's GNOME desktop adapter calls
// org.hyusk.Shell to list, focus, and close windows, because Wayland does not
// let the agent enumerate them itself. This is desktop control, not UI, and it
// is independent of the top-bar indicator in ./indicator.js.
//
// `windows.js` does the Meta work; this module owns only the D-Bus name and
// object lifecycle. Nothing here draws, polls, or reads the runtime files.

import Gio from 'gi://Gio';

import {
    SHELL_DBUS_NAME,
    SHELL_DBUS_PATH,
    SHELL_INTERFACE,
    WindowControl,
} from './windows.js';

export class HyuskWindowService {
    constructor() {
        this._dbus = null;
        this._nameId = 0;
        this._control = new WindowControl();
    }

    /** Export the object and take the well-known session-bus name. */
    enable() {
        if (this._dbus)
            return;

        const control = this._control;
        const impl = {
            ListWindows: () => control.listWindows(),
            ActiveWindow: () => control.activeWindow(),
            ActivateWindow: (/** @type {string} */ query) => control.activateWindow(query),
            CloseWindow: (/** @type {string} */ query) => control.closeWindow(query),
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

    /** Drop the name and unexport the object. Safe to call twice. */
    disable() {
        this._dbus?.unexport();
        this._dbus = null;

        if (this._nameId) {
            Gio.bus_unown_name(this._nameId);
            this._nameId = 0;
        }
    }
}
