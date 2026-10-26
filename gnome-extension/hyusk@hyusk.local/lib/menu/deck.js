// @ts-check
//
// The indicator menu: a state header, transport keys, then grouped sections
// for workspace, model, device, and settings.

import Gio from 'gi://Gio';
import St from 'gi://St';
import Clutter from 'gi://Clutter';

import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

import { applyStateClass, stateStatus } from '../state.js';
import { launchAutomationBrowser, launchWorkflowsApp } from '../launcher.js';
import { namedWorkflows, readWorkflowConfig, writeControl } from '../runtime.js';
import { ApiKeysMenu } from './credentials.js';
import { ModelMenu } from './models.js';
import { PhoneMenu } from './phone.js';
import { QuickRunMenu } from './quickRun.js';

export class DeckMenu {
    /**
     * @param {any} menu The `PopupMenu.PopupMenu` owned by the panel button.
     * @param {string} iconPath
     */
    constructor(menu, iconPath) {
        this._menu = menu;
        this._iconPath = iconPath;
        this._menu.actor.add_style_class_name('hyusk-menu');

        this._buildHeader();
        this._buildTransport();

        this._section('Workspace');
        this._quickRun = new QuickRunMenu(this._menu);
        this._workflowItem = this._menu.addAction('Workflows', () => launchWorkflowsApp());

        this._section('Model');
        this._models = new ModelMenu(this._menu);
        this._menu.addAction('Refresh models', () => writeControl('refresh_models'));

        this._section('Device');
        new PhoneMenu(this._menu, iconPath);
        this._menu.addAction('Open automation browser', () => launchAutomationBrowser(this._iconPath));

        this._section('Settings');
        new ApiKeysMenu(this._menu);

        this._menu.connect('open-state-changed', (_menu, open) => {
            if (open)
                this.refreshWorkflows();
        });
    }

    _buildHeader() {
        const header = new PopupMenu.PopupBaseMenuItem({
            reactive: false,
            can_focus: false,
            style_class: 'popup-menu-item hyusk-flat-item',
        });
        const box = new St.BoxLayout({ style_class: 'hyusk-deck-heading', x_expand: true });
        box.add_child(new St.Icon({
            gicon: new Gio.FileIcon({ file: Gio.File.new_for_path(this._iconPath) }),
            icon_size: 32,
            y_align: Clutter.ActorAlign.CENTER,
        }));

        const text = new St.BoxLayout({
            style_class: 'hyusk-deck-text',
            vertical: true,
            x_expand: true,
            spacing: 1,
        });
        text.add_child(new St.Label({ text: 'Hyusk', style_class: 'hyusk-deck-title' }));

        const stateRow = new St.BoxLayout({ y_align: Clutter.ActorAlign.CENTER });
        this._dot = new St.Widget({ style_class: 'hyusk-state-dot' });
        this._stateLabel = new St.Label({ text: stateStatus('Hidden'), style_class: 'hyusk-deck-state' });
        stateRow.add_child(this._dot);
        stateRow.add_child(this._stateLabel);
        text.add_child(stateRow);

        box.add_child(text);
        header.add_child(box);
        this._menu.addMenuItem(header);
    }

    _buildTransport() {
        const transport = new PopupMenu.PopupBaseMenuItem({
            reactive: false,
            can_focus: false,
            style_class: 'popup-menu-item hyusk-flat-item',
        });
        const keys = new St.BoxLayout({
            style_class: 'hyusk-transport',
            x_expand: true,
            spacing: 8,
        });

        const listen = new St.Button({
            child: new St.Label({ text: 'Listen' }),
            reactive: true,
            can_focus: true,
            x_expand: true,
            style_class: 'hyusk-key hyusk-key-primary',
        });
        listen.connect('clicked', () => writeControl('start_listening'));

        const stop = new St.Button({
            child: new St.Label({ text: 'Stop' }),
            reactive: true,
            can_focus: true,
            x_expand: true,
            style_class: 'hyusk-key hyusk-key-danger',
        });
        stop.connect('clicked', () => writeControl('stop_task'));

        keys.add_child(listen);
        keys.add_child(stop);
        transport.add_child(keys);
        this._menu.addMenuItem(transport);
    }

    /** @param {string} text */
    _section(text) {
        const row = new PopupMenu.PopupBaseMenuItem({
            reactive: false,
            can_focus: false,
            style_class: 'popup-menu-item hyusk-flat-item',
        });
        row.add_child(new St.Label({ text, style_class: 'hyusk-section-label', x_expand: true }));
        this._menu.addMenuItem(row);
    }

    /**
     * @param {import('../state.js').HyuskState} state
     * @param {string|null} [provider]
     */
    setState(state, provider = null) {
        applyStateClass(this._menu.actor, state);
        this._stateLabel.text = provider
            ? `${stateStatus(state)} · ${provider}`
            : stateStatus(state);
    }

    /**
     * @param {import('./models.js').Catalog[]} catalogs
     * @param {string} provider
     * @param {string} model
     */
    updateModels(catalogs, provider, model) {
        this._models.update(catalogs, provider, model);
    }

    refreshWorkflows() {
        this._quickRun.refresh();
        const count = namedWorkflows(readWorkflowConfig()).length;
        this._workflowItem.label.text = count ? `Workflows (${count})` : 'Workflows';
    }
}
