// @ts-check
//
// Animation policy in one place: everything that moves asks this module first
// so the extension follows the desktop's reduced-motion setting.

import St from 'gi://St';

/** @returns {boolean} */
export function animationsEnabled() {
    try {
        // St.Settings is the Shell's own settings object, so it always matches
        // the animations setting used by the rest of the desktop.
        return St.Settings.get().enable_animations !== false;
    } catch (_error) {
        return true;
    }
}
