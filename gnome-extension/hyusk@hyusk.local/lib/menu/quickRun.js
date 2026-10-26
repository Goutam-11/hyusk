// @ts-check
//
// The searchable "Run workflow" submenu.

import St from 'gi://St';
import Pango from 'gi://Pango';

import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

import { namedWorkflows, readWorkflowConfig, writeControl } from '../runtime.js';

export class QuickRunMenu {
    /**
     * @param {any} parent
     */
    constructor(parent) {
        this.item = new PopupMenu.PopupSubMenuMenuItem('Run workflow');
        parent.addMenuItem(this.item);
        this.item.menu.actor.set_style('max-height: 360px;');

        /** @type {string} */
        this._key = '';
        /** @type {{item: any, searchable: string}[]} */
        this._rows = [];

        this.refresh();
    }

    /** Rebuild the list when `commands.json` changes. */
    refresh() {
        const config = readWorkflowConfig();
        const workflows = namedWorkflows(config);
        const key = JSON.stringify(workflows.map(workflow => [
            workflow.name,
            Array.isArray(workflow.phrases) ? workflow.phrases : [],
        ]));
        if (key === this._key)
            return;

        this._key = key;
        this._rows = [];
        this.item.menu.removeAll();

        const searchRow = new PopupMenu.PopupBaseMenuItem({
            reactive: false,
            can_focus: false,
            style_class: 'popup-menu-item hyusk-flat-item',
        });
        const search = new St.Entry({
            hint_text: 'Find a workflow…',
            can_focus: true,
            x_expand: true,
            style_class: 'hyusk-search',
        });
        searchRow.add_child(search);
        this.item.menu.addMenuItem(searchRow);

        for (const workflow of workflows) {
            const label = workflow.name.trim();
            const item = this.item.menu.addAction(
                label,
                () => writeControl('run_workflow', null, null, label)
            );
            item.label.clutter_text.ellipsize = Pango.EllipsizeMode.END;
            this._rows.push({
                item,
                searchable: [label, ...(Array.isArray(workflow.phrases) ? workflow.phrases : [])]
                    .join(' ').toLowerCase(),
            });
        }

        const empty = new PopupMenu.PopupMenuItem(
            workflows.length ? 'No matching workflows' : 'No workflows yet — create one in the app',
            { reactive: false }
        );
        empty.visible = !this._rows.length;
        this.item.menu.addMenuItem(empty);

        search.clutter_text.connect('text-changed', () => {
            const query = search.get_text().trim().toLowerCase();
            for (const row of this._rows)
                row.item.visible = !query || row.searchable.includes(query);
            empty.visible = !this._rows.some(row => row.item.visible);
        });
    }
}
