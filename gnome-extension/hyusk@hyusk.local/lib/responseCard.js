// @ts-check
//
// The floating card under the panel: latest agent reply, approvals, timers.
// It pauses its auto-dismiss while the pointer or keyboard focus is inside it.

import GLib from 'gi://GLib';
import Clutter from 'gi://Clutter';
import Pango from 'gi://Pango';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import { applyStateClass, stateStatus } from './state.js';
import { animationsEnabled } from './motion.js';

const CARD_WIDTH = 360;
const AUTO_HIDE_MS = 14000;
const PREVIEW_CHARS = 360;
const FADE_IN_MS = 160;
const FADE_OUT_MS = 130;

export class HyuskResponseCard extends St.BoxLayout {
    /**
     * @param {any} anchor The panel button the card points at.
     * @param {(action: string) => void} onControl
     */
    constructor(anchor, onControl) {
        super({
            style_class: 'hyusk-display',
            vertical: false,
            reactive: true,
            can_focus: true,
            visible: false,
            opacity: 0,
        });

        this._anchor = anchor;
        this._onControl = onControl;
        this._text = '';
        this._expanded = false;
        this._persistent = false;
        this._hideTimeout = 0;
        this._timerTimeout = 0;
        this._payloadKey = '';
        this._hiding = false;

        this._accent = new St.Widget({
            style_class: 'hyusk-accent',
            x_expand: false,
            y_expand: true,
        });
        this.add_child(this._accent);

        this._column = new St.BoxLayout({
            style_class: 'hyusk-card-body',
            vertical: true,
            x_expand: true,
            spacing: 8,
        });
        this.add_child(this._column);

        const header = new St.BoxLayout({ x_expand: true, spacing: 8 });
        this._title = new St.Label({
            text: 'Hyusk',
            style_class: 'hyusk-card-title',
            x_expand: true,
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._statusLabel = new St.Label({
            text: stateStatus('Hidden'),
            style_class: 'hyusk-card-status',
            y_align: Clutter.ActorAlign.CENTER,
        });
        header.add_child(this._title);
        header.add_child(this._statusLabel);
        this._column.add_child(header);

        this._meta = new St.Label({ text: '', style_class: 'hyusk-card-meta', x_expand: true });
        this._meta.clutter_text.ellipsize = Pango.EllipsizeMode.END;
        this._column.add_child(this._meta);

        this._timerDisplay = new St.BoxLayout({
            x_align: Clutter.ActorAlign.CENTER,
            x_expand: true,
            spacing: 2,
        });
        this._timerMinutes = this._timerPart('hyusk-timer');
        this._timerColon = this._timerPart('hyusk-timer-colon');
        this._timerSeconds = this._timerPart('hyusk-timer');
        this._timerColon.text = ':';
        this._timerDisplay.add_child(this._timerMinutes);
        this._timerDisplay.add_child(this._timerColon);
        this._timerDisplay.add_child(this._timerSeconds);
        this._timerDisplay.visible = false;
        this._column.add_child(this._timerDisplay);

        this._body = new St.Label({
            text: '',
            style_class: 'hyusk-display-body',
            x_expand: true,
        });
        this._body.clutter_text.line_wrap = true;
        this._body.clutter_text.line_wrap_mode = Pango.WrapMode.WORD_CHAR;
        const bodyBox = new St.BoxLayout({ vertical: true, x_expand: true });
        bodyBox.add_child(this._body);
        this._bodyScroll = new St.ScrollView({
            style: 'max-height: 260px;',
            hscrollbar_policy: St.PolicyType.NEVER,
            vscrollbar_policy: St.PolicyType.AUTOMATIC,
            overlay_scrollbars: true,
        });
        this._bodyScroll.set_child(bodyBox);
        this._column.add_child(this._bodyScroll);

        this._approvalActions = new St.BoxLayout({ x_expand: true, spacing: 6 });
        this._approvalActions.add_child(this._button('Approve', () => this._onControl('approve'), 'hyusk-key-primary'));
        this._approvalActions.add_child(this._button('Deny', () => this._onControl('deny'), 'hyusk-key-ghost'));
        this._approvalActions.visible = false;
        this._column.add_child(this._approvalActions);

        this._listeningActions = new St.BoxLayout({ x_expand: true, spacing: 6 });
        this._listeningActions.add_child(this._button(
            'Stop listening', () => this._onControl('stop_listening'), 'hyusk-key-danger'
        ));
        this._listeningActions.visible = false;
        this._column.add_child(this._listeningActions);

        const actions = new St.BoxLayout({ x_expand: true, spacing: 6 });
        this._expandButton = this._button('More', () => this._toggleExpanded(), 'hyusk-key-ghost');
        actions.add_child(this._expandButton);
        actions.add_child(this._button('Copy', () => this._copy(), 'hyusk-key-ghost'));
        actions.add_child(this._button('Dismiss', () => this.dismiss(true), 'hyusk-key-ghost'));
        this._column.add_child(actions);

        this.connect('key-focus-in', () => this._cancelHide());
        this.connect('key-focus-out', () => this._armAutoHide());
        this.connect('enter-event', () => {
            this._cancelHide();
            return Clutter.EVENT_PROPAGATE;
        });
        this.connect('leave-event', () => {
            this._armAutoHide();
            return Clutter.EVENT_PROPAGATE;
        });
        this.connect('key-press-event', (_actor, event) => {
            if (event.get_key_symbol() === Clutter.KEY_Escape) {
                this.dismiss(true);
                return Clutter.EVENT_STOP;
            }
            return Clutter.EVENT_PROPAGATE;
        });
    }

    /**
     * @param {string} label
     * @param {() => void} callback
     * @param {string} [styleClass]
     */
    _button(label, callback, styleClass = '') {
        const button = new St.Button({
            child: new St.Label({ text: label }),
            reactive: true,
            can_focus: true,
            style_class: ['hyusk-key', styleClass].filter(Boolean).join(' '),
        });
        button.connect('clicked', callback);
        return button;
    }

    /** @param {string} styleClass */
    _timerPart(styleClass) {
        return new St.Label({ text: '00', style_class: styleClass });
    }

    /** @param {string} value */
    _setTimerText(value) {
        const parts = String(value).split(':');
        this._timerMinutes.text = parts[0] || '00';
        this._timerSeconds.text = parts[1] || '00';
    }

    /**
     * @param {{id?: unknown, kind?: string, text?: unknown, persistent?: boolean}} latest
     * @param {string} provider
     * @param {string} model
     * @param {boolean} awaitingReply
     * @param {{ends_at_ms?: number}|null} timer
     */
    showPayload(latest, provider, model, awaitingReply, timer) {
        const payloadKey = JSON.stringify([latest.id ?? null, latest.kind, latest.text, latest.persistent]);
        const changed = payloadKey !== this._payloadKey;
        this._payloadKey = payloadKey;
        if (changed) {
            this._cancelHide();
            this._expanded = false;
        }

        this._text = String(latest.text ?? '');
        const approval = latest.kind === 'approval';
        this._persistent = Boolean(latest.persistent) || approval;
        this._title.text = approval ? 'Approval needed' :
            String(latest.kind || '').startsWith('workflow') ? 'Workflow' : 'Hyusk';
        this._approvalActions.visible = approval;
        this._listeningActions.visible = Boolean(awaitingReply);
        this._meta.text = [provider, model, awaitingReply ? 'Listening for your reply…' : '']
            .filter(Boolean).join(' · ');
        this._timer = timer;
        this._renderTimer();
        this._renderText();

        if (changed && this._text) {
            if (this.visible)
                this._position();
            else
                this._show();
        }
        if (changed && this.visible && !this._persistent)
            this._armAutoHide();
    }

    /** @param {import('./state.js').HyuskState} state */
    setState(state) {
        applyStateClass(this, state);
        this._statusLabel.text = stateStatus(state);
    }

    _renderTimer() {
        if (this._timerTimeout) {
            GLib.source_remove(this._timerTimeout);
            this._timerTimeout = 0;
        }

        const clock = this._text?.match(/\b(\d{1,2}:\d{2})\b/);
        if (clock) {
            this._setTimerText(clock[1]);
            this._timerDisplay.visible = true;
            return;
        }

        if (!this._timer?.ends_at_ms) {
            this._timerDisplay.visible = false;
            return;
        }

        const tick = () => {
            const seconds = Math.max(0, Math.ceil((this._timer.ends_at_ms - Date.now()) / 1000));
            const minutes = Math.floor(seconds / 60);
            this._setTimerText(`${minutes}:${String(seconds % 60).padStart(2, '0')}`);
            this._timerDisplay.visible = true;
            return seconds > 0 ? GLib.SOURCE_CONTINUE : GLib.SOURCE_REMOVE;
        };
        tick();
        this._timerTimeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 1000, tick);
    }

    _show() {
        this.remove_all_transitions();
        this._hiding = false;
        this.show();
        this._position();

        if (!animationsEnabled()) {
            this.opacity = 255;
            return;
        }

        this.opacity = 0;
        this.ease({
            opacity: 255,
            duration: FADE_IN_MS,
            mode: Clutter.AnimationMode.EASE_OUT_QUAD,
        });

        // Re-clamp once the card has been allocated its real height.
        GLib.idle_add(GLib.PRIORITY_DEFAULT_IDLE, () => {
            if (this.visible)
                this._position();
            return GLib.SOURCE_REMOVE;
        });
    }

    _position() {
        const [x, y] = this._anchor.get_transformed_position();
        const anchorWidth = this._anchor.width || 24;
        const monitor = Main.layoutManager.currentMonitor ?? Main.layoutManager.primaryMonitor;

        /** @type {{x: number, y: number, width: number, height: number}} */
        let area = { x: monitor.x, y: monitor.y, width: monitor.width, height: monitor.height };
        try {
            const workArea = Main.layoutManager.getWorkAreaForMonitor(monitor.index);
            if (workArea)
                area = workArea;
        } catch (_error) {
            // Fall back to the full monitor area on older shells.
        }

        const width = this.width || CARD_WIDTH;
        const height = this.height || 160;
        const cardX = Math.max(area.x + 8,
            Math.min(x + anchorWidth - width, area.x + area.width - width - 8));
        const cardY = Math.max(area.y + 8,
            Math.min(y + this._anchor.height + 8, area.y + area.height - height - 8));
        this.set_position(Math.round(cardX), Math.round(cardY));
    }

    _renderText() {
        const limit = this._expanded ? this._text.length : PREVIEW_CHARS;
        const suffix = this._text.length > limit ? '…' : '';
        this._body.text = this._text.slice(0, limit) + suffix;
        this._expandButton.visible = this._text.length > PREVIEW_CHARS;
        this._expandButton.get_child().text = this._expanded ? 'Less' : 'More';
        this._position();
    }

    _toggleExpanded() {
        this._expanded = !this._expanded;
        this._renderText();
    }

    _copy() {
        try {
            St.Clipboard.get_default().set_text(St.ClipboardType.CLIPBOARD, this._text);
        } catch (_error) {
            // Clipboard ownership is best effort in the shell process.
        }
    }

    /** Forget the current payload so the next update is treated as new. */
    reset() {
        this._payloadKey = '';
    }

    /** @param {boolean} notify */
    dismiss(notify) {
        this._cancelHide();
        if (notify)
            this._onControl('dismiss_card');
        if (!this.visible || this._hiding)
            return;

        this.remove_all_transitions();
        if (!animationsEnabled()) {
            this.hide();
            this.opacity = 0;
            return;
        }

        this._hiding = true;
        this.ease({
            opacity: 0,
            duration: FADE_OUT_MS,
            mode: Clutter.AnimationMode.EASE_IN_QUAD,
            onComplete: () => {
                this._hiding = false;
                this.hide();
                this.opacity = 0;
            },
        });
    }

    _armAutoHide() {
        this._cancelHide();
        if (!this.visible || this._persistent)
            return;

        this._hideTimeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, AUTO_HIDE_MS, () => {
            this._hideTimeout = 0;
            this.dismiss(false);
            return GLib.SOURCE_REMOVE;
        });
    }

    _cancelHide() {
        if (this._hideTimeout) {
            GLib.source_remove(this._hideTimeout);
            this._hideTimeout = 0;
        }
    }

    destroy() {
        this._cancelHide();
        if (this._timerTimeout)
            GLib.source_remove(this._timerTimeout);
        super.destroy();
    }
}
