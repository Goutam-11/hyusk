// @ts-check
//
// The extension talks to the Hyusk agent through small JSON files in the
// runtime directory. Nothing here touches Shell widgets.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

/** @typedef {{name: string, phrases: string[], steps: string[]}} Workflow */
/** @typedef {{apps?: Record<string, unknown>, sites?: Record<string, unknown>, workflows: Workflow[]}} CommandConfig */

export function statePath() {
    return GLib.build_filenamev([GLib.get_user_runtime_dir(), 'hyusk-state']);
}

export function statusPath() {
    return GLib.build_filenamev([GLib.get_user_runtime_dir(), 'hyusk-status.json']);
}

export function controlPath() {
    return GLib.build_filenamev([GLib.get_user_runtime_dir(), 'hyusk-control.json']);
}

export function pairingPath() {
    return GLib.build_filenamev([GLib.get_user_runtime_dir(), 'hyusk-pairing.json']);
}

export function commandsConfigPath() {
    return GLib.build_filenamev([GLib.get_user_config_dir(), 'hyusk', 'commands.json']);
}

/**
 * Read and parse a JSON file, returning null when it is missing or partial.
 * @param {string} path
 * @returns {any}
 */
export function readJson(path) {
    try {
        const [ok, contents] = GLib.file_get_contents(path);
        if (!ok)
            return null;

        return JSON.parse(new TextDecoder().decode(contents));
    } catch (_error) {
        // Runtime files are replaced atomically, but ignore a missing file.
        return null;
    }
}

/**
 * Read plain text, returning null when it is unavailable.
 * @param {string} path
 * @returns {string|null}
 */
export function readText(path) {
    try {
        const [ok, contents] = GLib.file_get_contents(path);
        return ok ? new TextDecoder().decode(contents).trim() : null;
    } catch (_error) {
        return null;
    }
}

let controlRevision = Date.now();

/**
 * Ask the running agent to do something. Best effort: the agent may be down.
 * @param {string} action
 * @param {string|null} [provider]
 * @param {string|null} [model]
 * @param {string|null} [text]
 */
export function writeControl(action, provider = null, model = null, text = null) {
    controlRevision = Math.max(Date.now(), controlRevision + 1);
    /** @type {Record<string, unknown>} */
    const payload = { revision: controlRevision, action };
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

/**
 * Read `commands.json`, always returning a usable workflows array.
 * @returns {CommandConfig}
 */
export function readWorkflowConfig() {
    const config = readJson(commandsConfigPath());
    if (!config || typeof config !== 'object')
        return { apps: {}, sites: {}, workflows: [] };

    if (!Array.isArray(config.workflows))
        config.workflows = [];
    return config;
}

/**
 * Persist `commands.json`, keeping the existing file when the write fails.
 * @param {CommandConfig} config
 * @returns {boolean}
 */
export function saveWorkflowConfig(config) {
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
        return true;
    } catch (error) {
        console.error(`[Hyusk] Could not save workflows: ${error}`);
        return false;
    }
}

/**
 * Workflows that have a usable name.
 * @param {CommandConfig} config
 * @returns {Workflow[]}
 */
export function namedWorkflows(config) {
    return config.workflows.filter(
        workflow => workflow && typeof workflow === 'object' &&
            typeof workflow.name === 'string' && workflow.name.trim()
    );
}
