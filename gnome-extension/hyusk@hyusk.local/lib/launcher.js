// @ts-check
//
// Launching companion Hyusk apps from the Shell menu.

import GLib from 'gi://GLib';
import Gio from 'gi://Gio';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

/**
 * Try an executable path and report whether it started.
 * @param {string[]} argv
 * @param {string} [cwd]
 * @returns {boolean}
 */
function spawn(argv, cwd) {
    try {
        if (cwd) {
            const launcher = new Gio.SubprocessLauncher({ flags: Gio.SubprocessFlags.NONE });
            launcher.set_cwd(cwd);
            launcher.spawnv(argv);
        } else {
            Gio.Subprocess.new(argv, Gio.SubprocessFlags.NONE);
        }
        return true;
    } catch (error) {
        console.error(`[Hyusk] Could not launch ${argv.join(' ')}: ${error}`);
        return false;
    }
}

/**
 * Open the native workflow studio, preferring the installed binary over the
 * development build.
 */
export function launchWorkflowsApp() {
    const installed = GLib.find_program_in_path('hyusk-workflows');
    if (installed && spawn([installed]))
        return;

    // GNOME Shell may inherit a login PATH without ~/.local/bin. Try the
    // location used by install-workflow-app.sh explicitly before falling
    // back to development builds.
    const userBinary = GLib.build_filenamev([GLib.get_home_dir(), '.local', 'bin', 'hyusk-workflows']);
    if (GLib.file_test(userBinary, GLib.FileTest.IS_EXECUTABLE) && spawn([userBinary]))
        return;

    const roots = [];
    const configuredRoot = GLib.getenv('HYUSK_PROJECT_DIR');
    if (configuredRoot)
        roots.push(configuredRoot);
    roots.push(GLib.get_current_dir());

    for (const root of roots) {
        const binary = GLib.build_filenamev([root, 'target', 'release', 'hyusk-workflows']);
        if (GLib.file_test(binary, GLib.FileTest.IS_EXECUTABLE) && spawn([binary]))
            return;
    }

    const projectRoot = roots.find(root => GLib.file_test(
        GLib.build_filenamev([root, 'Cargo.toml']), GLib.FileTest.EXISTS));
    if (projectRoot && spawn(['cargo', 'run', '--quiet', '--bin', 'hyusk-workflows'], projectRoot))
        return;

    Main.notify('Hyusk Workflows', 'Install or build the hyusk-workflows app first.');
}

/**
 * Start the dedicated automation browser profile.
 * @param {string} iconPath Path to the extension's brand icon, used to locate
 *   the sibling launch script.
 */
export function launchAutomationBrowser(iconPath) {
    const script = GLib.build_filenamev([GLib.path_get_dirname(iconPath), 'start-browser-control.sh']);
    if (!GLib.file_test(script, GLib.FileTest.EXISTS)) {
        Main.notify('Hyusk', 'The automation browser script is missing. Reinstall the extension.');
        return;
    }

    spawn(['/usr/bin/bash', script]);
}
