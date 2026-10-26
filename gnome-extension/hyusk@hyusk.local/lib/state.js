// @ts-check
//
// Agent states, their labels, and the colors documented in DESIGN.md.
// St's CSS parser has no custom properties, so the same hex values are written
// out per state in stylesheet.css. Keep both sides in sync.

/** @typedef {'Hidden'|'Waking'|'Listening'|'Thinking'|'Working'|'Speaking'} HyuskState */

/** @type {Record<HyuskState, number[]>} */
export const STATE_COLORS = {
    Hidden: [0x9a, 0xa0, 0xa6],
    Waking: [0x82, 0xd2, 0xff],
    Listening: [0x78, 0xe6, 0xaa],
    Thinking: [0xb4, 0xa0, 0xff],
    Working: [0xff, 0xaa, 0x6e],
    Speaking: [0xff, 0x82, 0xc8],
};

/** Short labels for the panel button. */
/** @type {Record<HyuskState, string>} */
export const STATE_LABELS = {
    Hidden: 'idle',
    Waking: 'waking',
    Listening: 'listening',
    Thinking: 'thinking',
    Working: 'working',
    Speaking: 'speaking',
};

/** Sentence-case labels for the menu header and response card. */
/** @type {Record<HyuskState, string>} */
export const STATE_STATUS = {
    Hidden: 'Ready',
    Waking: 'Waking up',
    Listening: 'Listening',
    Thinking: 'Thinking',
    Working: 'Working',
    Speaking: 'Speaking',
};

/** Every `hyusk-state-*` class, used to swap the active state cleanly. */
export const STATE_CLASSES = Object.keys(STATE_COLORS).map(
    state => stateClassName(/** @type {HyuskState} */ (state))
);

/** @param {HyuskState} state */
export function stateClassName(state) {
    return `hyusk-state-${state.toLowerCase()}`;
}

/**
 * Normalize a value from `hyusk-state` or `hyusk-status.json`.
 * @param {unknown} value
 * @returns {HyuskState}
 */
export function stateFromRuntime(value) {
    const normalized = String(value ?? '').toLowerCase();
    /** @type {Record<string, HyuskState>} */
    const states = {
        hidden: 'Hidden',
        idle: 'Hidden',
        waking: 'Waking',
        listening: 'Listening',
        thinking: 'Thinking',
        working: 'Working',
        speaking: 'Speaking',
    };
    return states[normalized] ?? 'Hidden';
}

/**
 * @param {HyuskState} state
 * @returns {number[]}
 */
export function stateColor(state) {
    return STATE_COLORS[state] ?? STATE_COLORS.Hidden;
}

/**
 * @param {HyuskState} state
 * @returns {string}
 */
export function stateStatus(state) {
    return STATE_STATUS[state] ?? STATE_STATUS.Hidden;
}

/**
 * Replace any previous state class on a shell actor.
 * @param {any} actor
 * @param {HyuskState} state
 */
export function applyStateClass(actor, state) {
    for (const name of STATE_CLASSES)
        actor.remove_style_class_name(name);
    actor.add_style_class_name(stateClassName(state));
}
