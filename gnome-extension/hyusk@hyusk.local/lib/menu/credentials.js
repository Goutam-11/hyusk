// @ts-check
//
// API keys live in one submenu instead of three top-level entries, so the
// main menu stays about agent control. Keys go to the login keyring.

import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import St from 'gi://St';

import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

const PROVIDERS = [
    ['openai', 'OpenAI API key'],
    ['openrouter', 'OpenRouter API key'],
    ['bedrock', 'Amazon Bedrock API key'],
];

export class ApiKeysMenu {
    /** @param {any} parent */
    constructor(parent) {
        this.item = new PopupMenu.PopupSubMenuMenuItem('API keys');
        parent.addMenuItem(this.item);
        this.item.menu.actor.set_style('max-height: 420px;');

        for (const [provider, title] of PROVIDERS)
            this._addProvider(provider, title);
    }

    /**
     * @param {string} provider
     * @param {string} title
     */
    _addProvider(provider, title) {
        const row = new PopupMenu.PopupBaseMenuItem({
            reactive: false,
            can_focus: false,
            style_class: 'popup-menu-item hyusk-flat-item',
        });
        const column = new St.BoxLayout({ vertical: true, x_expand: true, spacing: 5 });

        column.add_child(new St.Label({
            text: title,
            style_class: 'hyusk-key-title',
            x_expand: true,
        }));

        const entryRow = new St.BoxLayout({ x_expand: true, spacing: 6 });
        const entry = new St.Entry({
            hint_text: 'Paste key',
            can_focus: true,
            x_expand: true,
            style_class: 'hyusk-search',
            style: 'min-width: 190px;',
        });
        entry.clutter_text.set_password_char('•');
        const save = new St.Button({
            child: new St.Label({ text: 'Save' }),
            reactive: true,
            can_focus: true,
            style_class: 'hyusk-key',
        });
        entryRow.add_child(entry);
        entryRow.add_child(save);
        column.add_child(entryRow);

        const status = new St.Label({
            text: 'Not changed',
            style_class: 'hyusk-key-status',
            x_expand: true,
        });
        column.add_child(status);

        const storeKey = () => {
            const key = entry.get_text().trim();
            if (!key) {
                status.text = 'Paste a key before saving';
                return;
            }

            status.text = 'Saving securely…';
            const process = Gio.Subprocess.new(
                ['secret-tool', 'store', `--label=Hyusk ${title}`, 'application', 'hyusk', 'provider', provider],
                Gio.SubprocessFlags.STDIN_PIPE | Gio.SubprocessFlags.STDERR_PIPE
            );
            process.communicate_utf8_async(`${key}\n`, null, (proc, result) => {
                try {
                    const [, , stderr] = proc.communicate_utf8_finish(result);
                    if (proc.get_successful()) {
                        entry.set_text('');
                        status.text = 'Saved securely · restarting Hyusk…';
                        GLib.spawn_command_line_async('systemctl --user restart hyusk.service');
                    } else {
                        status.text = (stderr || 'Could not save key').trim();
                    }
                } catch (_error) {
                    status.text = 'Could not save key';
                }
            });
        };

        save.connect('clicked', storeKey);
        entry.clutter_text.connect('activate', storeKey);
        row.add_child(column);
        this.item.menu.addMenuItem(row);
    }
}
