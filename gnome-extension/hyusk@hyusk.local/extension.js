import GObject from 'gi://GObject';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import Meta from 'gi://Meta';
import Clutter from 'gi://Clutter';
import Pango from 'gi://Pango';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as ModalDialog from 'resource:///org/gnome/shell/ui/modalDialog.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';

const SHELL_DBUS_NAME = 'org.hyusk.Shell';
const SHELL_DBUS_PATH = '/org/hyusk/Shell';
const SHELL_INTERFACE = `
<node>
  <interface name="org.hyusk.Shell">
    <method name="ListWindows">
      <arg type="s" direction="out"/>
    </method>
    <method name="ActiveWindow">
      <arg type="s" direction="out"/>
    </method>
    <method name="ActivateWindow">
      <arg type="s" direction="in"/>
      <arg type="s" direction="out"/>
    </method>
    <method name="CloseWindow">
      <arg type="s" direction="in"/>
      <arg type="s" direction="out"/>
    </method>
  </interface>
</node>`;

const STATE_COLORS = {
    Hidden: [0x9a, 0xa0, 0xa6],
    Waking: [0x82, 0xd2, 0xff],
    Listening: [0x78, 0xe6, 0xaa],
    Thinking: [0xb4, 0xa0, 0xff],
    Working: [0xff, 0xaa, 0x6e],
    Speaking: [0xff, 0x82, 0xc8],
};

const STATE_LABELS = {
    Hidden: 'idle',
    Waking: 'waking',
    Listening: 'listening',
    Thinking: 'thinking',
    Working: 'working',
    Speaking: 'speaking',
};

function statePath() {
    return GLib.build_filenamev([GLib.get_user_runtime_dir(), 'hyusk-state']);
}

function statusPath() {
    return GLib.build_filenamev([GLib.get_user_runtime_dir(), 'hyusk-status.json']);
}

function controlPath() {
    return GLib.build_filenamev([GLib.get_user_runtime_dir(), 'hyusk-control.json']);
}

function commandsConfigPath() {
    return GLib.build_filenamev([GLib.get_user_config_dir(), 'hyusk', 'commands.json']);
}

function readJson(path) {
    try {
        const [ok, contents] = GLib.file_get_contents(path);
        if (!ok)
            return null;

        return JSON.parse(new TextDecoder().decode(contents));
    } catch (_error) {
        // Runtime files are replaced atomically, but ignore a missing or partial file.
        return null;
    }
}

function stateFromRuntime(state) {
    const value = String(state ?? '').toLowerCase();
    return {
        hidden: 'Hidden', idle: 'Hidden', waking: 'Waking', listening: 'Listening',
        thinking: 'Thinking', working: 'Working', speaking: 'Speaking',
    }[value] ?? 'Hidden';
}

function lerp(current, target, amount) {
    return current + (target - current) * amount;
}

const HyuskDrawing = GObject.registerClass(
class HyuskDrawing extends St.DrawingArea {
    _init() {
        super._init({
            style_class: 'hyusk-indicator',
            reactive: false,
        });

        this.set_size(24, 24);

        this._state = 'Hidden';
        this._phase = 0;
        this._color = [...STATE_COLORS.Hidden];

        this.connect('repaint', () => this._draw());

        this._animationId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 33, () => {
            const speed = this._state === 'Working' ? 0.35
                : this._state === 'Waking' ? 0.30
                : this._state === 'Speaking' ? 0.22
                : 0.12;

            this._phase += speed;

            const target = STATE_COLORS[this._state] ?? STATE_COLORS.Hidden;
            for (let index = 0; index < 3; index++)
                this._color[index] = lerp(this._color[index], target[index], 0.12);

            this.queue_repaint();
            return GLib.SOURCE_CONTINUE;
        });
    }

    setState(state) {
        if (!STATE_COLORS[state] || state === this._state)
            return;

        this._state = state;
        this.accessible_name = `Hyusk: ${STATE_LABELS[state] ?? state}`;
    }

    _draw() {
        const context = this.get_context();
        const [width, height] = this.get_surface_size();

        const red = this._color[0] / 255;
        const green = this._color[1] / 255;
        const blue = this._color[2] / 255;

        context.save();

        const centerX = width / 2;
        const centerY = height / 2;

        const speaking = this._state === 'Speaking';
        const thinking = this._state === 'Thinking';
        const waking = this._state === 'Waking';

        const bounce = speaking ? Math.sin(this._phase) * 1.4 : 0;
        const tilt = thinking ? Math.sin(this._phase) * 0.18 : 0;
        const pulse = 1 + Math.sin(this._phase * (waking ? 2 : 1)) * (waking ? 0.12 : 0.05);
        const flap = 1 + Math.sin(this._phase) * (this._state === 'Working' ? 0.30 : 0.18);

        context.translate(centerX, centerY + bounce);
        context.rotate(tilt);
        context.scale(pulse, pulse);

        const alpha = this._state === 'Hidden' ? 0.55 : 1.0;

        // Upper wings.
        context.setSourceRGBA(red, green, blue, alpha);

        context.newPath();
        context.moveTo(0, -0.5);
        context.curveTo(-3 * flap, -10 * flap, -13, -10 * flap, -9.5, -1);
        context.curveTo(-14, 2, -6, 4, 0, 1);
        context.closePath();
        context.fill();

        context.newPath();
        context.moveTo(0, -0.5);
        context.curveTo(3 * flap, -10 * flap, 13, -10 * flap, 9.5, -1);
        context.curveTo(14, 2, 6, 4, 0, 1);
        context.closePath();
        context.fill();

        // Lower wings, slightly darker.
        context.setSourceRGBA(red * 0.78, green * 0.78, blue * 0.78, alpha);

        context.newPath();
        context.moveTo(0, 1);
        context.curveTo(-4, 5, -9, 8, -5.5, 3.5);
        context.curveTo(-6.5, 1.5, -3, 1, 0, 1.5);
        context.closePath();
        context.fill();

        context.newPath();
        context.moveTo(0, 1);
        context.curveTo(4, 5, 9, 8, 5.5, 3.5);
        context.curveTo(6.5, 1.5, 3, 1, 0, 1.5);
        context.closePath();
        context.fill();

        // Body.
        context.setSourceRGBA(1, 1, 1, alpha);
        context.newPath();
        context.arc(0, 0, 1.8, 0, 2 * Math.PI);
        context.fill();

        // Antennae.
        context.setSourceRGBA(red, green, blue, alpha);
        context.setLineWidth(0.9);

        context.newPath();
        context.moveTo(-0.8, -1.5);
        context.curveTo(-2.4, -4, -3.2, -5.5, -1.8, -6.5);
        context.stroke();

        context.newPath();
        context.moveTo(0.8, -1.5);
        context.curveTo(2.4, -4, 3.2, -5.5, 1.8, -6.5);
        context.stroke();

        context.restore();
        context.$dispose();
    }

    destroy() {
        if (this._animationId) {
            GLib.source_remove(this._animationId);
            this._animationId = 0;
        }

        super.destroy();
    }
});

const HyuskResponseCard = GObject.registerClass(
class HyuskResponseCard extends St.BoxLayout {
    _init(anchor, onControl) {
        super._init({
            style: 'background-color: rgba(32, 35, 42, 0.98); border-radius: 12px; ' +
                'padding: 12px; spacing: 8px; width: 330px;',
            vertical: true,
            reactive: true,
            can_focus: true,
            visible: false,
        });

        this._anchor = anchor;
        this._onControl = onControl;
        this._text = '';
        this._expanded = false;
        this._hideTimeout = 0;
        this._timerTimeout = 0;

        const header = new St.BoxLayout({ x_expand: true });
        this._title = new St.Label({
            text: 'Hyusk',
            style: 'font-weight: 700; font-size: 13px;',
            x_expand: true,
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._meta = new St.Label({
            text: '',
            style: 'color: rgba(255, 255, 255, 0.72); font-size: 11px;',
            y_align: Clutter.ActorAlign.CENTER,
        });
        header.add_child(this._title);
        header.add_child(this._meta);
        this.add_child(header);

        this._timerDisplay = new St.BoxLayout({ x_align: Clutter.ActorAlign.CENTER, x_expand: true, style: 'spacing: 5px;' });
        this._timerMinutes = this._timerPart();
        this._timerColon = this._timerPart();
        this._timerSeconds = this._timerPart();
        this._timerColon.text = ':';
        this._timerDisplay.add_child(this._timerMinutes);
        this._timerDisplay.add_child(this._timerColon);
        this._timerDisplay.add_child(this._timerSeconds);
        this._timerDisplay.visible = false;
        this.add_child(this._timerDisplay);

        this._body = new St.Label({
            text: '',
            style: 'color: rgba(255, 255, 255, 0.94); font-size: 12px; line-height: 1.35;',
            x_expand: true,
        });
        this._body.clutter_text.line_wrap = true;
        this._body.clutter_text.line_wrap_mode = Pango.WrapMode.WORD_CHAR;
        this.add_child(this._body);

        this._approvalActions = new St.BoxLayout({ x_expand: true, style: 'spacing: 6px;' });
        this._approvalActions.add_child(this._button('Yes', () => this._onControl('approve')));
        this._approvalActions.add_child(this._button('No', () => this._onControl('deny')));
        this._approvalActions.visible = false;
        this.add_child(this._approvalActions);

        this._listeningActions = new St.BoxLayout({ x_expand: true, style: 'spacing: 6px;' });
        this._listeningActions.add_child(this._button('Stop listening', () => this._onControl('stop_listening')));
        this._listeningActions.visible = false;
        this.add_child(this._listeningActions);

        const actions = new St.BoxLayout({ x_expand: true, style: 'spacing: 6px;' });
        this._expandButton = this._button('More', () => this._toggleExpanded());
        actions.add_child(this._expandButton);
        actions.add_child(this._button('Copy', () => this._copy()));
        actions.add_child(this._button('Dismiss', () => this.dismiss(true)));
        this.add_child(actions);

        this.connect('key-focus-in', () => this._cancelHide());
        this.connect('key-press-event', (_actor, event) => {
            if (event.get_key_symbol() === Clutter.KEY_Escape) {
                this.dismiss(true);
                return Clutter.EVENT_STOP;
            }
            return Clutter.EVENT_PROPAGATE;
        });
    }

    _button(label, callback) {
        const button = new St.Button({
            child: new St.Label({ text: label }),
            reactive: true,
            can_focus: true,
            style: 'padding: 5px 8px; border-radius: 7px; background-color: rgba(255, 255, 255, 0.10);',
        });
        button.connect('clicked', callback);
        return button;
    }

    _timerPart() {
        return new St.Label({ text: '00', style: 'font-size: 34px; font-weight: 800; color: #ffffff;' });
    }

    _setTimerText(value) {
        const parts = String(value).split(':');
        this._timerMinutes.text = parts[0] || '00';
        this._timerSeconds.text = parts[1] || '00';
    }

    showPayload(latest, provider, model, awaitingReply, timer) {
        this._cancelHide();
        this._text = String(latest.text ?? '');
        this._expanded = false;
        const approval = latest.kind === 'approval';
        this._title.text = approval ? 'Approval needed' : 'Hyusk';
        this._approvalActions.visible = approval;
        this._listeningActions.visible = Boolean(awaitingReply);
        this._meta.text = [provider, model, awaitingReply ? 'Listening for your reply…' : '']
            .filter(Boolean).join(' · ');
        this._timer = timer;
        this._renderTimer();
        this._renderText();
        this.visible = Boolean(this._text);
        this._position();

        if (this.visible && !latest.persistent) {
            this._hideTimeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 12000, () => {
                this.dismiss(false);
                return GLib.SOURCE_REMOVE;
            });
        }
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

    _position() {
        const [x, y] = this._anchor.get_transformed_position();
        const anchorWidth = this._anchor.width || 24;
        const monitor = Main.layoutManager.primaryMonitor;
        const width = this.width || 330;
        const cardX = Math.max(monitor.x + 8, Math.min(x + anchorWidth - width, monitor.x + monitor.width - width - 8));
        this.set_position(Math.round(cardX), Math.round(y + this._anchor.height + 8));
    }

    _renderText() {
        const limit = this._expanded ? this._text.length : 360;
        const suffix = this._text.length > limit ? '…' : '';
        this._body.text = this._text.slice(0, limit) + suffix;
        this._expandButton.visible = this._text.length > 360;
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

    dismiss(notify) {
        this._cancelHide();
        this.hide();
        if (notify)
            this._onControl('dismiss_card');
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
});

const HyuskIndicator = GObject.registerClass(
class HyuskIndicator extends PanelMenu.Button {
    _init() {
        super._init(0.0, 'Hyusk', false);
        this.reactive = true;
        this.can_focus = true;
        this.connect('button-press-event', () => {
            this.menu.toggle();
            return Clutter.EVENT_STOP;
        });

        this._drawing = new HyuskDrawing();
        this.add_child(this._drawing);
        this._lastState = '';
        this._lastRevision = -1;
        this._controlRevision = Date.now();
        this._catalogKey = '';
        this._workflowKey = '';
        this._workflowDialog = null;
        this._modelQuery = '';
        this._modelRows = [];
        this._modelGroups = [];
        this._modelSearchEntry = null;
        this._modelNoResults = null;

        this._responseCard = new HyuskResponseCard(this, action => this._writeControl(action));
        Main.layoutManager.addTopChrome(this._responseCard);

        this._modelMenu = new PopupMenu.PopupSubMenuMenuItem('Model');
        this.menu.addMenuItem(this._modelMenu);
        this._modelMenu.menu.addMenuItem(new PopupMenu.PopupMenuItem('Waiting for model catalog', { reactive: false }));
        this._workflowMenu = new PopupMenu.PopupMenuItem('Workflows');
        this._workflowMenu.connect('activate', () => {
            GLib.idle_add(GLib.PRIORITY_DEFAULT_IDLE, () => {
                this._launchWorkflowsApp();
                return GLib.SOURCE_REMOVE;
            });
        });
        this.menu.addMenuItem(this._workflowMenu);
        this._refreshWorkflowLabel();
        this.menu.addAction('Refresh models', () => this._writeControl('refresh_models'));
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        this.menu.addMenuItem(this._credentialMenu('openai', 'OpenAI API key'));
        this.menu.addMenuItem(this._credentialMenu('openrouter', 'OpenRouter API key'));
        this.menu.addAction('Stop Hyusk', () => {
            try {
                GLib.file_set_contents(
                    GLib.build_filenamev([GLib.get_user_runtime_dir(), 'hyusk-stop']),
                    'stop\n'
                );
            } catch (_error) {
                // The process may already have stopped; there is nothing to do.
            }
        });

        this._stateTimeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 500, () => {
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
        try {
            const [ok, contents] = GLib.file_get_contents(statePath());
            if (!ok)
                return;

            const state = new TextDecoder().decode(contents).trim();
            if (!state || state === this._lastState)
                return;

            this._lastState = state;
            this._drawing.setState(state);
        } catch (_error) {
            // The state file is optional; ignore transient read errors.
        }
    }

    _applyStatus(status) {
        const state = stateFromRuntime(status.state);
        this._lastState = state;
        this._drawing.setState(state);

        if (status.latest?.text)
            this._responseCard.showPayload(
                status.latest,
                status.active_provider,
                status.active_model,
                status.awaiting_reply,
                status.timer
            );

        this._populateModels(status.catalogs, status.active_provider, status.active_model);
        this._refreshWorkflowLabel();
    }

    _readWorkflowConfig() {
        const config = readJson(commandsConfigPath());
        if (!config || typeof config !== 'object')
            return { apps: {}, sites: {}, workflows: [] };

        if (!Array.isArray(config.workflows))
            config.workflows = [];
        return config;
    }

    _refreshWorkflowLabel() {
        if (!this._workflowMenu)
            return;

        const config = this._readWorkflowConfig();
        const count = config.workflows.filter(workflow => workflow && typeof workflow === 'object').length;
        this._workflowMenu.label.text = count ? `Workflows (${count})` : 'Workflows';
        this._workflowKey = JSON.stringify(config.workflows);
    }

    _launchWorkflowsApp() {
        const installed = GLib.find_program_in_path('hyusk-workflows');
        if (installed) {
            try {
                Gio.Subprocess.new([installed], Gio.SubprocessFlags.NONE);
                return;
            } catch (error) {
                console.error(`[Hyusk] Could not launch installed workflow app: ${error}`);
            }
        }

        // GNOME Shell may inherit a login PATH without ~/.local/bin. Try the
        // location used by install-workflow-app.sh explicitly before falling
        // back to development builds.
        const userBinary = GLib.build_filenamev([
            GLib.get_home_dir(), '.local', 'bin', 'hyusk-workflows',
        ]);
        if (GLib.file_test(userBinary, GLib.FileTest.IS_EXECUTABLE)) {
            try {
                Gio.Subprocess.new([userBinary], Gio.SubprocessFlags.NONE);
                return;
            } catch (error) {
                console.error(`[Hyusk] Could not launch ${userBinary}: ${error}`);
            }
        }

        const roots = [];
        const configuredRoot = GLib.getenv('HYUSK_PROJECT_DIR');
        if (configuredRoot)
            roots.push(configuredRoot);
        roots.push(GLib.get_current_dir());

        for (const root of roots) {
            const binary = GLib.build_filenamev([root, 'target', 'release', 'hyusk-workflows']);
            if (!GLib.file_test(binary, GLib.FileTest.IS_EXECUTABLE))
                continue;

            try {
                Gio.Subprocess.new([binary], Gio.SubprocessFlags.NONE);
                return;
            } catch (error) {
                console.error(`[Hyusk] Could not launch workflow app at ${binary}: ${error}`);
            }
        }

        const projectRoot = roots.find(root => GLib.file_test(
            GLib.build_filenamev([root, 'Cargo.toml']), GLib.FileTest.EXISTS));
        if (projectRoot) {
            try {
                const launcher = new Gio.SubprocessLauncher({ flags: Gio.SubprocessFlags.NONE });
                launcher.set_cwd(projectRoot);
                launcher.spawnv(['cargo', 'run', '--quiet', '--bin', 'hyusk-workflows']);
                return;
            } catch (error) {
                console.error(`[Hyusk] Could not launch workflow app with cargo: ${error}`);
            }
        }

        Main.notify('Hyusk Workflows', 'Install or build the hyusk-workflows app first.');
    }

    _openWorkflowDialog() {
        if (this._workflowDialog) {
            this._workflowDialog.open(global.get_current_time());
            return;
        }

        const dialog = new ModalDialog.ModalDialog({
            destroyOnClose: true,
            styleClass: 'hyusk-workflow-dialog',
        });
        this._workflowDialog = dialog;

        let config = this._readWorkflowConfig();
        let selectedIndex = null;
        let draft = null;
        let deleteArmed = false;
        const workflowRows = [];

        const content = new St.BoxLayout({
            vertical: true,
            style: 'width: 760px; spacing: 10px;',
        });
        dialog.contentLayout.add_child(content);

        const title = new St.Label({
            text: 'Workflows',
            style: 'font-size: 18px; font-weight: 700;',
        });
        const description = new St.Label({
            text: 'Create a shortcut that runs deterministic commands in order. Say a trigger phrase to use it.',
            style: 'color: rgba(255, 255, 255, 0.72); font-size: 12px;',
            x_expand: true,
        });
        description.clutter_text.line_wrap = true;
        content.add_child(title);
        content.add_child(description);

        const search = new St.Entry({
            hint_text: 'Search workflows',
            can_focus: true,
            x_expand: true,
        });
        content.add_child(search);

        const listScroll = new St.ScrollView({ style: 'height: 180px;' });
        listScroll.set_policy(St.PolicyType.NEVER, St.PolicyType.AUTOMATIC);
        const list = new St.BoxLayout({ vertical: true, x_expand: true });
        listScroll.set_child(list);
        content.add_child(listScroll);
        const noResults = new St.Label({
            text: 'No workflows yet — choose New workflow.',
            style: 'padding: 10px; color: rgba(255, 255, 255, 0.72);',
            x_expand: true,
        });
        list.add_child(noResults);

        const editor = new St.BoxLayout({
            vertical: true,
            x_expand: true,
            style: 'spacing: 6px; padding-top: 4px;',
        });
        content.add_child(editor);

        const editorTitle = new St.Label({
            text: 'Select a workflow or create a new one',
            style: 'font-weight: 700;',
        });
        const nameEntry = new St.Entry({ hint_text: 'Workflow name', can_focus: true, x_expand: true });
        const phraseEntry = new St.Entry({ hint_text: 'Trigger phrases, comma-separated', can_focus: true, x_expand: true });
        const stepsEntry = new St.Entry({
            hint_text: 'Steps separated by semicolons, e.g. open code; set timer for 25 minutes',
            can_focus: true,
            x_expand: true,
        });
        const feedback = new St.Label({
            text: '',
            style: 'color: rgba(255, 170, 110, 0.95); font-size: 11px;',
            x_expand: true,
        });
        feedback.visible = false;
        const field = (label, entry) => {
            const row = new St.BoxLayout({ x_expand: true, style: 'spacing: 8px;' });
            row.add_child(new St.Label({ text: label, style: 'width: 72px;' }));
            row.add_child(entry);
            return row;
        };
        editor.add_child(editorTitle);
        editor.add_child(field('Name', nameEntry));
        editor.add_child(field('Triggers', phraseEntry));
        editor.add_child(field('Steps', stepsEntry));
        editor.add_child(feedback);

        const controls = new St.BoxLayout({ x_expand: true, style: 'spacing: 6px; padding-top: 4px;' });
        const control = (label, callback, style) => {
            const button = new St.Button({
                child: new St.Label({ text: label }),
                reactive: true,
                can_focus: true,
                style: `padding: 6px 10px; border-radius: 7px; ${style || 'background-color: rgba(255,255,255,0.10);'}`,
            });
            button.connect('clicked', callback);
            return button;
        };
        const newButton = control('New workflow', () => {
            const names = new Set(config.workflows
                .filter(workflow => workflow && typeof workflow === 'object')
                .map(workflow => String(workflow.name || '').toLowerCase()));
            let name = 'New workflow';
            let suffix = 2;
            while (names.has(name.toLowerCase()))
                name = `New workflow ${suffix++}`;
            draft = { name, phrases: [name], steps: [] };
            selectedIndex = -1;
            deleteArmed = false;
            editorTitle.text = 'New workflow';
            nameEntry.set_text(name);
            phraseEntry.set_text(name);
            stepsEntry.set_text('');
            setFeedback('');
            saveButton.get_child().text = 'Create';
            saveButton.visible = true;
            runButton.visible = false;
            deleteButton.get_child().text = 'Cancel';
            deleteButton.visible = true;
            renderList();
            nameEntry.grab_key_focus();
        }, 'background-color: rgba(130, 170, 255, 0.28);');
        const saveButton = control('Create', () => saveEditor(), 'background-color: rgba(80, 190, 130, 0.35);');
        const runButton = control('Run', () => {
            if (selectedIndex === null || selectedIndex < 0 || !config.workflows[selectedIndex])
                return;
            this._writeControl('run_workflow', null, null, config.workflows[selectedIndex].name);
        }, 'background-color: rgba(130, 170, 255, 0.28);');
        const deleteButton = control('Delete', () => {
            if (selectedIndex === -1) {
                draft = null;
                selectedIndex = null;
                renderList();
                clearEditor();
                return;
            }
            if (selectedIndex === null || !config.workflows[selectedIndex])
                return;
            if (!deleteArmed) {
                deleteArmed = true;
                deleteButton.get_child().text = 'Delete?';
                setFeedback('Select Delete? again to remove this workflow.');
                return;
            }
            const updated = this._readWorkflowConfig();
            updated.workflows.splice(selectedIndex, 1);
            if (!this._saveWorkflowConfig(updated)) {
                setFeedback('Could not delete workflow. Check config directory permissions.');
                return;
            }
            config = updated;
            selectedIndex = null;
            deleteArmed = false;
            deleteButton.get_child().text = 'Delete';
            renderList();
            clearEditor();
        }, 'background-color: rgba(220, 80, 80, 0.30);');
        controls.add_child(newButton);
        controls.add_child(saveButton);
        controls.add_child(runButton);
        controls.add_child(deleteButton);
        editor.add_child(controls);

        const setFeedback = message => {
            feedback.text = message;
            feedback.visible = Boolean(message);
        };
        const clearEditor = () => {
            editorTitle.text = 'Select a workflow or create a new one';
            nameEntry.set_text('');
            phraseEntry.set_text('');
            stepsEntry.set_text('');
            setFeedback('');
            saveButton.get_child().text = 'Create';
            saveButton.visible = false;
            runButton.visible = false;
            deleteButton.visible = false;
        };
        const selectWorkflow = index => {
            draft = null;
            selectedIndex = index;
            deleteArmed = false;
            deleteButton.get_child().text = 'Delete';
            const workflow = config.workflows[index];
            if (!workflow)
                return clearEditor();
            editorTitle.text = String(workflow.name || `Workflow ${index + 1}`);
            nameEntry.set_text(String(workflow.name || ''));
            phraseEntry.set_text(Array.isArray(workflow.phrases) ? workflow.phrases.join(', ') : '');
            stepsEntry.set_text(Array.isArray(workflow.steps) ? workflow.steps.join('; ') : '');
            saveButton.get_child().text = 'Save';
            saveButton.visible = true;
            runButton.visible = true;
            deleteButton.visible = true;
            setFeedback('');
        };
        const renderList = () => {
            for (const row of workflowRows)
                row.destroy();
            workflowRows.length = 0;
            if (draft) {
                const draftRow = new St.Button({
                    child: new St.Label({ text: `${draft.name} (draft)` }),
                    reactive: true,
                    can_focus: true,
                    x_expand: true,
                    style: 'padding: 6px; background-color: rgba(130, 170, 255, 0.22);',
                });
                draftRow.connect('clicked', () => {
                    selectedIndex = -1;
                    nameEntry.grab_key_focus();
                });
                list.add_child(draftRow);
                workflowRows.push(draftRow);
            }
            config.workflows.forEach((workflow, index) => {
                if (!workflow || typeof workflow !== 'object')
                    return;
                const row = new St.Button({
                    child: new St.Label({
                        text: String(workflow.name || `Workflow ${index + 1}`),
                        x_expand: true,
                        x_align: Clutter.ActorAlign.START,
                    }),
                    reactive: true,
                    can_focus: true,
                    x_expand: true,
                    style: 'padding: 6px; background-color: rgba(255,255,255,0.08);',
                });
                row.connect('clicked', () => selectWorkflow(index));
                list.add_child(row);
                workflowRows.push(row);
            });
            list.set_child_above_sibling(noResults, null);
            filterList();
        };
        const filterList = () => {
            const query = search.get_text().trim().toLowerCase();
            let index = 0;
            if (draft) {
                workflowRows[0].visible = !query || draft.name.toLowerCase().includes(query);
                index = 1;
            }
            config.workflows.forEach(workflow => {
                if (!workflow || typeof workflow !== 'object')
                    return;
                const text = [workflow.name, ...(Array.isArray(workflow.phrases) ? workflow.phrases : []), ...(Array.isArray(workflow.steps) ? workflow.steps : [])]
                    .join(' ').toLowerCase();
                workflowRows[index++].visible = !query || text.includes(query);
            });
            const hasVisibleWorkflow = workflowRows.some(row => row.visible);
            noResults.text = config.workflows.length || draft
                ? 'No workflows match your search.'
                : 'No workflows yet — choose New workflow.';
            noResults.visible = !hasVisibleWorkflow;
        };
        const saveEditor = () => {
            const entryName = nameEntry.get_text().trim();
            const entryPhrases = phraseEntry.get_text().split(',').map(value => value.trim()).filter(Boolean);
            const entrySteps = stepsEntry.get_text().split(';').map(value => value.trim()).filter(Boolean);
            if (!entryName)
                return setFeedback('Add a name for this workflow.');
            if (!entrySteps.length)
                return setFeedback('Add at least one step, separated with semicolons.');
            const normalized = [entryName, ...entryPhrases].map(value => value.toLowerCase());
            const collision = config.workflows.some((workflow, index) => {
                if (!workflow || typeof workflow !== 'object' || (draft === null && index === selectedIndex))
                    return false;
                const names = [workflow.name, ...(Array.isArray(workflow.phrases) ? workflow.phrases : [])]
                    .map(value => String(value || '').trim().toLowerCase());
                return normalized.some(value => names.includes(value));
            });
            if (collision)
                return setFeedback('That name or trigger is already used by another workflow.');
            const entry = { name: entryName, phrases: entryPhrases.length ? entryPhrases : [entryName], steps: entrySteps };
            const updated = this._readWorkflowConfig();
            if (draft)
                updated.workflows.push(entry);
            else if (selectedIndex !== null && updated.workflows[selectedIndex])
                Object.assign(updated.workflows[selectedIndex], entry);
            else
                return setFeedback('Select a workflow or choose New workflow.');
            if (!this._saveWorkflowConfig(updated))
                return setFeedback('Could not save workflow. Check config directory permissions.');
            config = updated;
            selectedIndex = draft ? updated.workflows.length - 1 : selectedIndex;
            draft = null;
            deleteArmed = false;
            deleteButton.get_child().text = 'Delete';
            selectWorkflow(selectedIndex);
            renderList();
            this._refreshWorkflowLabel();
        };

        search.clutter_text.connect('text-changed', filterList);
        clearEditor();
        renderList();
        dialog.setButtons([{
            label: 'Close',
            action: () => dialog.close(global.get_current_time()),
            key: Clutter.KEY_Escape,
        }]);
        dialog.connect('closed', () => {
            this._workflowDialog = null;
            this._workflowKey = '';
            this._refreshWorkflowLabel();
        });
        dialog.setInitialKeyFocus(search);
        dialog.open(global.get_current_time());
    }

    _saveWorkflowConfig(config) {
        try {
            const directory = Gio.File.new_for_path(GLib.path_get_dirname(commandsConfigPath()));
            directory.make_directory_with_parents(null);
        } catch (_error) {
            // Directory may already exist.
        }

        try {
            const file = Gio.File.new_for_path(commandsConfigPath());
            const bytes = new TextEncoder().encode(JSON.stringify(config, null, 2) + '\n');
            file.replace_contents(bytes, null, false, Gio.FileCreateFlags.REPLACE_DESTINATION, null);
            this._workflowKey = '';
            return true;
        } catch (error) {
            // Keep the existing config intact if the write fails.
            console.error(`[Hyusk] Could not save workflows: ${error}`);
            return false;
        }
    }

    _populateModels(catalogs, activeProvider, activeModel) {
        const entries = Array.isArray(catalogs) ? catalogs : [];
        const key = JSON.stringify([entries, activeProvider, activeModel]);
        if (key === this._catalogKey) {
            this._applyModelFilter();
            return;
        }

        this._catalogKey = key;
        this._modelQuery = '';
        this._modelRows = [];
        this._modelGroups = [];
        this._modelMenu.menu.removeAll();
        this._modelSearchEntry = null;
        this._modelNoResults = null;
        if (!entries.length) {
            this._modelMenu.menu.addMenuItem(new PopupMenu.PopupMenuItem('No model catalog available', { reactive: false }));
            return;
        }

        const hasModels = entries.some(catalog => Array.isArray(catalog.models) && catalog.models.length);
        if (hasModels) {
            const searchRow = new PopupMenu.PopupBaseMenuItem({ reactive: true, can_focus: false });
            const search = new St.Entry({
                hint_text: 'Search models',
                can_focus: true,
                x_expand: true,
                style: 'min-width: 260px; margin: 2px 0;',
            });
            this._modelSearchEntry = search;
            search.clutter_text.connect('text-changed', () => {
                this._modelQuery = search.get_text();
                this._applyModelFilter();
            });
            searchRow.add_child(search);
            this._modelMenu.menu.addMenuItem(searchRow);
        }

        for (const catalog of entries) {
            const provider = String(catalog.provider ?? 'Provider');
            const heading = new PopupMenu.PopupMenuItem(
                catalog.fresh === false ? `${provider} (cached)` : provider,
                { reactive: false }
            );
            heading.label.style = 'font-weight: 700; color: rgba(255, 255, 255, 0.72);';
            this._modelMenu.menu.addMenuItem(heading);

            const models = Array.isArray(catalog.models) ? catalog.models : [];
            const group = { heading, separator: null, rows: [] };
            this._modelGroups.push(group);
            for (const model of models) {
                const id = String(model.id ?? '');
                const label = String(model.label ?? id);
                const selected = provider === activeProvider && id === activeModel;
                const item = this._modelMenu.menu.addAction(
                    selected ? `${label} (active)` : label,
                    () => this._writeControl('set_model', provider, id)
                );
                group.rows.push(item);
                this._modelRows.push({
                    item,
                    searchable: `${provider} ${id} ${label}`.toLowerCase(),
                });
            }
            if (!models.length) {
                const empty = new PopupMenu.PopupMenuItem('No models found', { reactive: false });
                empty.visible = true;
                this._modelMenu.menu.addMenuItem(empty);
                group.empty = empty;
            }
            group.separator = new PopupMenu.PopupSeparatorMenuItem();
            this._modelMenu.menu.addMenuItem(group.separator);
        }

        this._modelNoResults = new PopupMenu.PopupMenuItem('No models match your search.', { reactive: false });
        this._modelNoResults.visible = false;
        this._modelMenu.menu.addMenuItem(this._modelNoResults);
        this._applyModelFilter();
    }

    _applyModelFilter() {
        const query = String(this._modelQuery || '').trim().toLowerCase();
        let visible = 0;
        for (const row of this._modelRows) {
            const matches = !query || row.searchable.includes(query);
            row.item.visible = matches;
            if (matches)
                visible++;
        }
        for (const group of this._modelGroups) {
            const hasVisible = group.rows.some(item => item.visible);
            group.heading.visible = hasVisible || !query;
            group.separator.visible = hasVisible || !query;
            if (group.empty)
                group.empty.visible = !query;
        }
        if (this._modelNoResults)
            this._modelNoResults.visible = Boolean(this._modelRows.length && !visible);
    }

    _credentialMenu(provider, title) {
        const item = new PopupMenu.PopupSubMenuMenuItem(title);
        const row = new PopupMenu.PopupBaseMenuItem({ reactive: false, can_focus: false });
        const entry = new St.Entry({ hint_text: 'Paste key', can_focus: true, x_expand: true });
        const save = new St.Button({ child: new St.Label({ text: 'Save' }), reactive: true, can_focus: true,
            style: 'padding: 5px 8px; margin-left: 6px; border-radius: 7px; background-color: rgba(255,255,255,0.12);' });
        save.connect('clicked', () => {
            const key = entry.get_text().trim();
            if (!key) return;
            const process = Gio.Subprocess.new(
                ['secret-tool', 'store', `--label=Hyusk ${title}`, 'hyusk', 'provider', provider],
                Gio.SubprocessFlags.STDIN_PIPE | Gio.SubprocessFlags.STDERR_PIPE
            );
            process.communicate_utf8_async(`${key}\n`, null, (proc, result) => {
                try {
                    const [, , stderr] = proc.communicate_utf8_finish(result);
                    if (proc.get_successful()) {
                        entry.set_text('Saved — restarting Hyusk');
                        GLib.spawn_command_line_async('systemctl --user restart hyusk.service');
                    } else {
                        entry.set_text(stderr || 'Could not save key');
                    }
                } catch (_error) { entry.set_text('Could not save key'); }
            });
        });
        row.add_child(entry);
        row.add_child(save);
        item.menu.addMenuItem(row);
        return item;
    }

    _writeControl(action, provider = null, model = null, text = null) {
        this._controlRevision = Math.max(Date.now(), this._controlRevision + 1);
        const payload = { revision: this._controlRevision, action };
        if (provider)
            payload.provider = provider;
        if (model)
            payload.model = model;
        if (text)
            payload.text = text;

        try {
            const file = Gio.File.new_for_path(controlPath());
            const bytes = new TextEncoder().encode(JSON.stringify(payload));
            file.replace_contents(
                bytes,
                null,
                false,
                Gio.FileCreateFlags.REPLACE_DESTINATION,
                null
            );
        } catch (_error) {
            // The agent may not be running; controls are intentionally best effort.
        }
    }

    destroy() {
        if (this._stateTimeout) {
            GLib.source_remove(this._stateTimeout);
            this._stateTimeout = 0;
        }

        this._workflowDialog?.close();
        this._workflowDialog = null;

        this._drawing?.destroy();
        this._drawing = null;

        this._responseCard?.destroy();
        this._responseCard = null;

        super.destroy();
    }
});

export default class HyuskIndicatorExtension extends Extension {
    enable() {
        this._indicator = new HyuskIndicator();
        Main.panel.addToStatusArea('hyusk-indicator', this._indicator, 0, 'right');

        this._exportShell();
    }

    disable() {
        this._indicator?.destroy();
        this._indicator = null;

        this._unexportShell();
    }

    _exportShell() {
        const impl = {
            ListWindows: () => this._listWindows(),
            ActiveWindow: () => this._activeWindow(),
            ActivateWindow: query => this._activateWindow(query),
            CloseWindow: query => this._closeWindow(query),
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

    _unexportShell() {
        this._dbus?.unexport();
        this._dbus = null;

        if (this._nameId) {
            Gio.bus_unown_name(this._nameId);
            this._nameId = 0;
        }
    }

    _normalWindows() {
        return global
            .get_window_actors()
            .map(actor => actor.meta_window)
            .filter(win => {
                if (!win)
                    return false;

                try {
                    return win.get_window_type() === Meta.WindowType.NORMAL;
                } catch (_error) {
                    return false;
                }
            });
    }

    _listWindows() {
        const windows = this._normalWindows().map(win => ({
            title: this._title(win),
            app: this._app(win),
            active: this._focused(win),
        }));

        return JSON.stringify(windows);
    }

    _activeWindow() {
        const win = global.display.focus_window;

        if (!win)
            return JSON.stringify({});

        return JSON.stringify({
            title: this._title(win),
            app: this._app(win),
            active: true,
        });
    }

    _matchWindow(query) {
        const needle = String(query ?? '').toLowerCase().trim();
        const windows = this._normalWindows();

        if (!needle)
            return global.display.focus_window ?? windows[0] ?? null;

        // Prefer an exact application-name match, then app substring, then
        // window-title substring.
        return (
            windows.find(win => this._app(win).toLowerCase() === needle) ??
            windows.find(win => this._app(win).toLowerCase().includes(needle)) ??
            windows.find(win => this._title(win).toLowerCase().includes(needle)) ??
            null
        );
    }

    _activateWindow(query) {
        const match = this._matchWindow(query);

        if (!match)
            return JSON.stringify({ ok: false, error: `no window matching '${query}'` });

        Main.activateWindow(match, global.get_current_time());

        return JSON.stringify({
            ok: true,
            title: this._title(match),
            app: this._app(match),
        });
    }

    _closeWindow(query) {
        const match = this._matchWindow(query);

        if (!match)
            return JSON.stringify({ ok: false, error: `no window matching '${query}'` });

        const title = this._title(match);
        const app = this._app(match);

        match.delete(global.get_current_time());

        return JSON.stringify({ ok: true, title, app });
    }

    _title(win) {
        try {
            return win.get_title() ?? '';
        } catch (_error) {
            return '';
        }
    }

    _app(win) {
        try {
            return win.get_wm_class() ?? '';
        } catch (_error) {
            return '';
        }
    }

    _focused(win) {
        try {
            return win.has_focus();
        } catch (_error) {
            return false;
        }
    }
}
