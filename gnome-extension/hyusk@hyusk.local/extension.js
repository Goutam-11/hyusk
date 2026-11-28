// @ts-check
//
// Hyusk GNOME Shell extension entry point. The extension has two independent
// jobs, composed here and nowhere else:
//   lib/indicator.js       the top-bar indicator, a client of the runtime files
//   lib/window-service.js  org.hyusk.Shell, the window service for the agent
//
// The indicator UI lives in ./lib: drawing.js, responseCard.js, menu/*. The
// window service is desktop control, not UI, and the agent reaches it over
// D-Bus only on a GNOME session.

import GLib from 'gi://GLib';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';

import { HyuskIndicator } from './lib/indicator.js';
import { HyuskWindowService } from './lib/window-service.js';

export default class HyuskExtension extends Extension {
    enable() {
        this._indicator = new HyuskIndicator(
            GLib.build_filenamev([this.path, 'hyusk_butterfly_mark.png'])
        );
        Main.panel.addToStatusArea('hyusk-indicator', this._indicator, 0, 'right');

        this._windowService = new HyuskWindowService();
        this._windowService.enable();
    }

    disable() {
        this._windowService?.disable();
        this._windowService = null;

        this._indicator?.destroy();
        this._indicator = null;
    }
}
