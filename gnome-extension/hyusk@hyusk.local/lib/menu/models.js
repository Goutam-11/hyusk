// @ts-check
//
// Model picker: one searchable group per provider, with the active model
// marked by an ornament instead of a text suffix.

import St from 'gi://St';

import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

import { writeControl } from '../runtime.js';

/**
 * @typedef {{provider: string, fresh?: boolean, models?: {id: string, label?: string}[]}} Catalog
 */

export class ModelMenu {
    /** @param {any} parent */
    constructor(parent) {
        this.item = new PopupMenu.PopupSubMenuMenuItem('Model');
        parent.addMenuItem(this.item);
        this.item.menu.actor.set_style('max-height: 380px;');

        /** @type {string} */
        this._catalogKey = '';
        /** @type {string} */
        this._query = '';
        /** @type {{item: any, searchable: string}[]} */
        this._rows = [];
        /** @type {{heading: any, separator: any, rows: any[], empty?: any}[]} */
        this._groups = [];
        /** @type {any} */
        this._search = null;
        /** @type {any} */
        this._noResults = null;
    }

    /**
     * @param {Catalog[]} catalogs
     * @param {string} activeProvider
     * @param {string} activeModel
     */
    update(catalogs, activeProvider, activeModel) {
        const entries = Array.isArray(catalogs) ? catalogs : [];
        const key = JSON.stringify([entries, activeProvider, activeModel]);
        if (key === this._catalogKey) {
            this._applyFilter();
            return;
        }

        this._catalogKey = key;
        this._query = '';
        this._rows = [];
        this._groups = [];
        this.item.menu.removeAll();
        this._search = null;
        this._noResults = null;

        if (!entries.length) {
            this.item.menu.addMenuItem(
                new PopupMenu.PopupMenuItem('No model catalog available', { reactive: false })
            );
            return;
        }

        const hasModels = entries.some(catalog => Array.isArray(catalog.models) && catalog.models.length);
        if (hasModels) {
            const searchRow = new PopupMenu.PopupBaseMenuItem({
                reactive: false,
                can_focus: false,
                style_class: 'popup-menu-item hyusk-flat-item',
            });
            const search = new St.Entry({
                hint_text: 'Search models',
                can_focus: true,
                x_expand: true,
                style_class: 'hyusk-search',
            });
            this._search = search;
            search.clutter_text.connect('text-changed', () => {
                this._query = search.get_text();
                this._applyFilter();
            });
            searchRow.add_child(search);
            this.item.menu.addMenuItem(searchRow);
        }

        for (const catalog of entries) {
            const provider = String(catalog.provider ?? 'Provider');
            const headingRow = new PopupMenu.PopupBaseMenuItem({
                reactive: false,
                can_focus: false,
                style_class: 'popup-menu-item hyusk-flat-item',
            });
            headingRow.add_child(new St.Label({
                text: catalog.fresh === false ? `${provider} · cached` : provider,
                style_class: 'hyusk-section-label',
                x_expand: true,
            }));
            this.item.menu.addMenuItem(headingRow);

            /** @type {{heading: any, separator: any, rows: any[], empty?: any}} */
            const group = { heading: headingRow, separator: null, rows: [] };
            this._groups.push(group);

            const models = Array.isArray(catalog.models) ? catalog.models : [];
            for (const model of models) {
                const id = String(model.id ?? '');
                const label = String(model.label ?? id);
                const selected = provider === activeProvider && id === activeModel;
                const item = this.item.menu.addAction(
                    label,
                    () => writeControl('set_model', provider, id)
                );
                if (selected) {
                    item.setOrnament(PopupMenu.Ornament.CHECK);
                    item.label.add_style_class_name('hyusk-model-active');
                }
                group.rows.push(item);
                this._rows.push({ item, searchable: `${provider} ${id} ${label}`.toLowerCase() });
            }

            if (!models.length) {
                const empty = new PopupMenu.PopupMenuItem('No models found', { reactive: false });
                this.item.menu.addMenuItem(empty);
                group.empty = empty;
            }

            group.separator = new PopupMenu.PopupSeparatorMenuItem();
            this.item.menu.addMenuItem(group.separator);
        }

        this._noResults = new PopupMenu.PopupMenuItem('No models match your search.', { reactive: false });
        this._noResults.visible = false;
        this.item.menu.addMenuItem(this._noResults);
        this._applyFilter();
    }

    _applyFilter() {
        const query = String(this._query || '').trim().toLowerCase();
        let visible = 0;
        for (const row of this._rows) {
            const matches = !query || row.searchable.includes(query);
            row.item.visible = matches;
            if (matches)
                visible++;
        }

        for (const group of this._groups) {
            const hasVisible = group.rows.some(item => item.visible);
            group.heading.visible = hasVisible || !query;
            group.separator.visible = hasVisible || !query;
            if (group.empty)
                group.empty.visible = !query;
        }

        if (this._noResults)
            this._noResults.visible = Boolean(this._rows.length && !visible);
    }
}
