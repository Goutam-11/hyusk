// Ambient types for the modules GNOME Shell and GJS provide. They are
// deliberately permissive: the goal is editor completion and catching broken
// imports, not modelling the whole Shell API.

declare module 'gi://*' {
    const value: any;
    export default value;
}

declare module 'resource:///org/gnome/shell/ui/main.js' {
    export const panel: any;
    export const layoutManager: any;
    export const overview: any;
    export function notify(...args: any[]): void;
    export function activateWindow(...args: any[]): void;
}

declare module 'resource:///org/gnome/shell/ui/panelMenu.js' {
    export class Button {
        constructor(...args: any[]);
        [key: string]: any;
    }
}

declare module 'resource:///org/gnome/shell/ui/popupMenu.js' {
    export const PopupMenuItem: any;
    export const PopupBaseMenuItem: any;
    export const PopupSubMenuMenuItem: any;
    export const PopupSeparatorMenuItem: any;
    export const Ornament: any;
}

declare module 'resource:///org/gnome/shell/extensions/extension.js' {
    export class Extension {
        constructor(...args: any[]);
        path: string;
        [key: string]: any;
    }
}

// GJS globals that are not part of the standard library typings used here.
declare const global: any;
declare const console: any;

declare class TextDecoder {
    constructor(label?: string);
    decode(input?: any): string;
}

declare class TextEncoder {
    encode(input?: string): Uint8Array;
}
