// @ts-check
//
// The top-bar button. It polls the agent's status files and forwards state to
// the drawing, the menu, and the response card.

import GObject from 'gi://GObject';
import GLib from 'gi://GLib';
import Clutter from 'gi://Clutter';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';

import { HyuskDrawing } from './drawing.js';
import { HyuskResponseCard } from './responseCard.js';
import { DeckMenu } from './menu/deck.js';
import { applyStateClass, STATE_LABELS, stateFromRuntime } from './state.js';
import {
    readJson,
    readText,
    statePath,
    statusPath,
    writeControl,
} from './runtime.js';

const POLL_INTERVAL_MS = 500;

export const HyuskIndicator = GObject.registerClass(
class HyuskIndicator extends PanelMenu.Button {
    /** @param {string} iconPath */
    _init(iconPath) {
        super._init(0.0, 'Hyusk', false);

        this.can_focus = true;
        this.add_style_class_name('hyusk-panel');

        this._lastState = '';
        this._lastRevision = -1;
        this._stateTimeoutId = 0;

        this._drawing = new HyuskDrawing(iconPath);
        const panel = new St.BoxLayout({
            y_align: Clutter.ActorAlign.CENTER,
            spacing: 4,
        });
        panel.add_child(this._drawing);
        this._panelState = new St.Label({
            text: '',
            style_class: 'hyusk-panel-state',
            y_align: Clutter.ActorAlign.CENTER,
        });
        panel.add_child(this._panelState);
        this.add_child(panel);

        this._deck = new DeckMenu(this.menu, iconPath);
        this._responseCard = new HyuskResponseCard(this, action => writeControl(action));
        Main.layoutManager.addTopChrome(this._responseCard);

        this._stateTimeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, POLL_INTERVAL_MS, () => {
            this._refresh();
            return GLib.SOURCE_CONTINUE;
        });

        this._refresh();
    }

    _refresh() {
        const status = readJson(statusPath());
        if (status && Number.isFinite(status.revision)) {
            if (status.revision !== this._lastRevision) {
                this._lastRevision = status.revision;
                this._applyStatus(status);
            }
            return;
        }

        // Compatibility with agents that have not yet adopted the JSON status file.
        const state = readText(statePath());
        if (!state)
            return;
        const normalized = stateFromRuntime(state);
        if (normalized === this._lastState)
            return;

        this._applyState(normalized);
    }

    /**
     * @param {import('./state.js').HyuskState} state
     * @param {string|null} [provider]
     */
    _applyState(state, provider = null) {
        this._lastState = state;
        applyStateClass(this, state);
        this._drawing.setState(state);
        this._deck.setState(state, provider);
        this._responseCard.setState(state);
        this._panelState.text = state === 'Hidden' ? '' : STATE_LABELS[state];
    }

    /** @param {any} status */
    _applyStatus(status) {
        const state = stateFromRuntime(status.state);
        this._applyState(state, status.active_provider ?? null);

        if (status.latest?.text) {
            this._responseCard.showPayload(
                status.latest,
                status.active_provider,
                status.active_model,
                status.awaiting_reply,
                status.timer
            );
        } else {
            this._responseCard.dismiss(false);
            this._responseCard.reset();
        }

        this._deck.updateModels(
            status.catalogs,
            status.active_provider,
            status.active_model
        );
        this._deck.refreshWorkflows();
    }

    destroy() {
        if (this._stateTimeoutId) {
            GLib.source_remove(this._stateTimeoutId);
            this._stateTimeoutId = 0;
        }

        this._responseCard?.destroy();
        this._responseCard = null;

        this._drawing?.destroy();
        this._drawing = null;

        super.destroy();
    }
});
