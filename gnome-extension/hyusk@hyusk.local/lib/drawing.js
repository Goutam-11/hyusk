// @ts-check
//
// The panel butterfly: a tinted PNG with a Cairo-drawn state aura. Colors are
// interpolated between states so a change of state reads as a transition
// rather than a jump.

import GObject from 'gi://GObject';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import Clutter from 'gi://Clutter';
import Cogl from 'gi://Cogl';
import St from 'gi://St';

import { STATE_COLORS, stateColor, stateStatus } from './state.js';

/**
 * @param {number} current
 * @param {number} target
 * @param {number} amount
 */
function lerp(current, target, amount) {
    return current + (target - current) * amount;
}

/** @param {number[]} rgb */
function clutterColor(rgb) {
    const [red, green, blue] = rgb.map(value => value / 255);
    if (Clutter.Color)
        return Clutter.Color.new(rgb[0], rgb[1], rgb[2], 255);

    const color = new Cogl.Color();
    color.init_from_4f(red, green, blue, 1.0);
    return color;
}

/** Per-state animation speed, in phase units per 50 ms tick. */
const STATE_SPEED = {
    Working: 0.22,
    Waking: 0.18,
    Speaking: 0.15,
    Listening: 0.09,
    Thinking: 0.09,
};

export const HyuskDrawing = GObject.registerClass(
class HyuskDrawing extends St.Widget {
    /** @param {string} iconPath */
    _init(iconPath) {
        super._init({
            reactive: false,
            layout_manager: new Clutter.BinLayout(),
        });

        this.set_size(28, 28);
        /** @type {import('./state.js').HyuskState} */
        this._state = 'Hidden';
        this._phase = 0;
        this._color = [...STATE_COLORS.Hidden];
        this._animationsEnabled = true;
        this._animationId = 0;

        this._settings = new Gio.Settings({ schema_id: 'org.gnome.desktop.interface' });
        this._animationsEnabled = this._settings.get_boolean('enable-animations');
        this._settingsChangedId = this._settings.connect('changed::enable-animations', () => {
            this._animationsEnabled = this._settings.get_boolean('enable-animations');
            if (!this._animationsEnabled)
                this._color = [...stateColor(this._state)];
            this._syncAnimation();
            this._applyMarkTint();
            this._aura.queue_repaint();
        });

        this._aura = new St.DrawingArea({
            reactive: false,
            x_expand: true,
            y_expand: true,
        });
        this._aura.connect('repaint', () => this._drawAura());
        this.add_child(this._aura);

        this._mark = new St.Icon({
            gicon: new Gio.FileIcon({ file: Gio.File.new_for_path(iconPath) }),
            icon_size: 22,
            reactive: false,
            x_align: Clutter.ActorAlign.CENTER,
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._colorize = new Clutter.ColorizeEffect();
        this._mark.add_effect_with_name('hyusk-state-tint', this._colorize);
        this._applyMarkTint();
        this.add_child(this._mark);
        this._mark.opacity = 190;
        this.accessible_name = 'Hyusk: idle';
    }

    /** @param {import('./state.js').HyuskState} state */
    setState(state) {
        if (!STATE_COLORS[state] || state === this._state)
            return;

        this._state = state;
        this.accessible_name = `Hyusk: ${stateStatus(state).toLowerCase()}`;
        this._mark.opacity = state === 'Hidden' ? 190 : 255;

        if (!this._animationsEnabled || state === 'Hidden')
            this._color = [...stateColor(state)];

        this._syncAnimation();
        this._applyMarkTint();
        this._aura.queue_repaint();
    }

    _applyMarkTint() {
        this._colorize.set_tint(clutterColor(this._color));
    }

    _settleButterfly() {
        this._mark.translation_x = 0;
        this._mark.translation_y = 0;
        this._mark.rotation_angle_z = 0;
        this._mark.scale_x = 1;
        this._mark.scale_y = 1;
    }

    _syncAnimation() {
        const shouldAnimate = this._animationsEnabled && this._state !== 'Hidden';
        if (shouldAnimate && !this._animationId) {
            this._animationId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 50, () => {
                if (this._state === 'Hidden' || !this._animationsEnabled) {
                    this._animationId = 0;
                    return GLib.SOURCE_REMOVE;
                }

                this._phase += STATE_SPEED[this._state] ?? 0.09;
                const target = stateColor(this._state);
                for (let index = 0; index < 3; index++)
                    this._color[index] = lerp(this._color[index], target[index], 0.18);

                this._applyMarkTint();
                const flight = this._state === 'Listening' || this._state === 'Waking' ? 1 : 0.65;
                this._mark.translation_x = Math.sin(this._phase) * 1.7 * flight;
                this._mark.translation_y = Math.sin(this._phase * 2) * 1.1 * flight;
                this._mark.rotation_angle_z = Math.sin(this._phase) * 5 * flight;
                const flutter = Math.sin(this._phase * 2) * 0.035 * flight;
                this._mark.scale_x = 1 + flutter;
                this._mark.scale_y = 1 - flutter * 0.55;
                this._aura.queue_repaint();
                return GLib.SOURCE_CONTINUE;
            });
        } else if (!shouldAnimate && this._animationId) {
            GLib.source_remove(this._animationId);
            this._animationId = 0;
            this._settleButterfly();
        }
    }

    _drawAura() {
        const context = this._aura.get_context();
        const [width, height] = this._aura.get_surface_size();

        const red = this._color[0] / 255;
        const green = this._color[1] / 255;
        const blue = this._color[2] / 255;

        context.save();

        const centerX = width / 2;
        const centerY = height / 2;
        const pulse = this._animationsEnabled && this._state !== 'Hidden'
            ? 1 + Math.sin(this._phase) * (this._state === 'Waking' ? 0.16 : 0.09)
            : 1;
        const alpha = this._state === 'Hidden' ? 0.16 : 0.3;

        context.translate(centerX, centerY);
        context.scale(pulse, pulse);
        for (let layer = 5; layer >= 1; layer--) {
            const glowAlpha = (alpha / layer) * (1 - layer / 8);
            context.setSourceRGBA(red, green, blue, glowAlpha);
            context.arc(0, 0, 2 + layer * 2.2, 0, 2 * Math.PI);
            context.fill();
        }

        context.restore();
        context.$dispose();
    }

    destroy() {
        if (this._animationId) {
            GLib.source_remove(this._animationId);
            this._animationId = 0;
        }

        if (this._settingsChangedId) {
            this._settings.disconnect(this._settingsChangedId);
            this._settingsChangedId = 0;
        }

        super.destroy();
    }
});
