// @ts-check
//
// Phone pairing: enable the laptop link once, then copy a short-lived code.

import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import Pango from 'gi://Pango';
import St from 'gi://St';

import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

import { pairingPath, readJson, writeControl } from '../runtime.js';

export class PhoneMenu {
    /**
     * @param {any} parent
     * @param {string} iconPath Path to the extension's brand icon, used to
     *   locate the sibling setup script.
     */
    constructor(parent, iconPath) {
        this.item = new PopupMenu.PopupSubMenuMenuItem('Connect phone');
        parent.addMenuItem(this.item);
        this._iconPath = iconPath;

        const statusRow = new PopupMenu.PopupBaseMenuItem({
            reactive: false,
            can_focus: false,
            style_class: 'popup-menu-item hyusk-flat-item',
        });
        this._status = new St.Label({
            text: 'Checking laptop link…',
            style_class: 'hyusk-phone-status',
            x_expand: true,
        });
        this._status.clutter_text.line_wrap = true;
        statusRow.add_child(this._status);
        this.item.menu.addMenuItem(statusRow);

        this._codeRow = new PopupMenu.PopupBaseMenuItem({
            reactive: false,
            can_focus: false,
            style_class: 'popup-menu-item hyusk-flat-item',
        });
        const codeBox = new St.BoxLayout({ x_expand: true, spacing: 6 });
        this._code = new St.Entry({
            hint_text: 'Pairing code',
            can_focus: true,
            x_expand: false,
            style_class: 'hyusk-code',
        });
        this._code.clutter_text.set_editable(false);
        this._code.clutter_text.ellipsize = Pango.EllipsizeMode.END;
        const copy = new St.Button({
            child: new St.Label({ text: 'Copy' }),
            reactive: true,
            can_focus: true,
            style_class: 'hyusk-key',
        });
        copy.connect('clicked', () => {
            const code = this._code.get_text();
            if (code)
                St.Clipboard.get_default().set_text(St.ClipboardType.CLIPBOARD, code);
        });
        codeBox.add_child(this._code);
        codeBox.add_child(copy);
        this._codeRow.add_child(codeBox);
        this.item.menu.addMenuItem(this._codeRow);

        this._setup = this.item.menu.addAction('Enable laptop link', () => this._enableLink());
        this.item.menu.addAction('Generate fresh code', () => writeControl('pair_phone'));

        this.item.menu.connect('open-state-changed', (_menu, open) => {
            if (open)
                this.refresh();
        });

        this.refresh();
    }

    refresh() {
        const pairing = readJson(pairingPath());
        const valid = pairing?.protocol === 'hyusk.link.v1' &&
            Number(pairing.expires_at) > Math.floor(Date.now() / 1000) &&
            typeof pairing.secret === 'string';

        this._codeRow.visible = valid;
        this._setup.visible = !pairing;

        if (!valid) {
            this._code.set_text('');
            this._status.text = pairing
                ? 'Code expired · generate a fresh one'
                : 'Link is off · enable the laptop link first';
            return;
        }

        const code = JSON.stringify(pairing);
        if (this._code.get_text() !== code)
            this._code.set_text(code);
        const minutes = Math.max(1, Math.ceil((Number(pairing.expires_at) * 1000 - Date.now()) / 60000));
        this._status.text = `One-time code · expires in ${minutes} min`;
    }

    _enableLink() {
        const script = GLib.build_filenamev([GLib.path_get_dirname(this._iconPath), 'setup-mobile-link.sh']);
        if (!GLib.file_test(script, GLib.FileTest.EXISTS)) {
            this._status.text = 'Install the latest Hyusk extension first';
            return;
        }

        this._status.text = 'Enabling secure laptop link…';
        let process;
        try {
            process = Gio.Subprocess.new(['/usr/bin/bash', script, '--enable'],
                Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_PIPE);
        } catch (error) {
            this._status.text = `Could not start setup: ${error.message}`;
            return;
        }

        process.communicate_utf8_async(null, null, (proc, result) => {
            try {
                const [, , stderr] = proc.communicate_utf8_finish(result);
                if (!proc.get_successful())
                    this._status.text = (stderr || 'Could not enable laptop link').trim();
                else
                    this._status.text = 'Link enabled · generating code…';
            } catch (_error) {
                this._status.text = 'Could not enable laptop link';
            }
        });
    }
}
