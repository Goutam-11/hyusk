#!/usr/bin/env python3
"""AT-SPI2 accessibility bridge for hyusk_agent.

Reads one JSON object per line from stdin and writes one JSON object per line
to stdout. The Rust `accessibility` tool spawns this process once and reuses it.

Supported actions:
  {"action": "apps"}
  {"action": "active", "read": true}
  {"action": "tree", "app": 0, "max_depth": 5, "max_nodes": 2000}
  {"action": "find", "name": "Search", "role": "push button", "text": "...", "app": 0, "limit": 20}
  {"action": "click", "path": [1, 0, 2]}        # or name/role/text
  {"action": "focus", "path": [1, 0, 2]}        # or name/role/text
  {"action": "set_text", "path": [...], "text": "hello"}
  {"action": "get_text", "path": [...]}
  {"action": "read", "app": 0}                  # native screen text
  {"action": "windows"}
"""

import json
import subprocess
import sys
from collections import deque

try:
    import pyatspi
except Exception as error:  # pragma: no cover - environment dependent
    print(
        json.dumps({"ok": False, "error": f"pyatspi unavailable: {error}"}),
        flush=True,
    )
    sys.exit(1)

# Roles that represent a top-level window rather than a control.
WINDOW_ROLES = ("frame", "window", "dialog", "alert", "file chooser")

# Cached check for org.gnome.desktop.interface toolkit-accessibility.
_TOOLKIT_ACCESSIBILITY = None


def toolkit_accessibility_enabled():
    """Whether GTK toolkits are asked to expose their accessibility tree.

    When this is false most GTK apps register with AT-SPI but publish an empty
    tree, which is the single biggest source of "blind spots".
    """
    global _TOOLKIT_ACCESSIBILITY

    if _TOOLKIT_ACCESSIBILITY is None:
        try:
            result = subprocess.run(
                [
                    "gsettings",
                    "get",
                    "org.gnome.desktop.interface",
                    "toolkit-accessibility",
                ],
                capture_output=True,
                text=True,
                timeout=5,
            )
            _TOOLKIT_ACCESSIBILITY = result.stdout.strip().lower() == "true"
        except Exception:
            _TOOLKIT_ACCESSIBILITY = True  # cannot tell; do not nag

    return _TOOLKIT_ACCESSIBILITY


def environment_warnings():
    warnings = []

    if not toolkit_accessibility_enabled():
        warnings.append(
            "toolkit-accessibility is disabled, so GTK apps expose an empty "
            "tree. Enable it with 'gsettings set "
            "org.gnome.desktop.interface toolkit-accessibility true' and "
            "restart the apps (Chromium/Electron apps also need "
            "ACCESSIBILITY_ENABLED=1)."
        )

    return warnings


def safe(function, default=None):
    try:
        return function()
    except Exception:
        return default


def role_name(accessible):
    return safe(accessible.getRoleName, "") or ""


def name_of(accessible):
    return safe(lambda: accessible.name, "") or ""


def bounds_of(accessible):
    component = safe(accessible.queryComponent)
    if component is None:
        return None

    extents = safe(lambda: component.getExtents(pyatspi.DESKTOP_COORDS))
    if extents is None:
        return None

    return {
        "x": int(extents.x),
        "y": int(extents.y),
        "width": int(extents.width),
        "height": int(extents.height),
    }


def states_of(accessible):
    state = safe(accessible.getState)
    if state is None:
        return []

    names = []
    for name in (
        "SHOWING",
        "VISIBLE",
        "ENABLED",
        "FOCUSED",
        "SELECTED",
        "EDITABLE",
        "ACTIVE",
        "EXPANDED",
        "CHECKED",
    ):
        constant = getattr(pyatspi, f"STATE_{name}", None)
        if constant is not None and safe(lambda: state.contains(constant), False):
            names.append(name.lower())

    return names


def actions_of(accessible):
    action = safe(accessible.queryAction)
    if action is None:
        return []

    names = []
    for index in range(safe(lambda: action.nActions, 0) or 0):
        names.append(safe(lambda index=index: action.getActionName(index), "") or "")

    return names


def text_of(accessible, limit=120):
    text_interface = safe(accessible.queryText)
    if text_interface is None:
        return ""

    value = safe(lambda: text_interface.getText(0, -1), "") or ""

    if not value.strip():
        return ""

    value = " ".join(value.split())

    if len(value) > limit:
        value = value[:limit] + "..."

    return value


def describe(accessible, include_text=True):
    item = {
        "role": role_name(accessible),
        "name": name_of(accessible),
        "bounds": bounds_of(accessible),
        "states": states_of(accessible),
        "actions": actions_of(accessible),
    }

    if include_text:
        value = text_of(accessible)

        if value:
            item["text"] = value

    return item


def indexed_children_of(accessible, limit=None):
    count = safe(lambda: accessible.childCount, 0) or 0
    output = []

    for index in range(min(count, limit) if limit is not None else count):
        child = safe(lambda index=index: accessible.getChildAtIndex(index))
        if child is not None:
            output.append((index, child))

    return output


def children_of(accessible):
    return [child for _, child in indexed_children_of(accessible)]


def desktop():
    return pyatspi.Registry.getDesktop(0)


def windows_of(app):
    return [window for window in children_of(app) if role_name(window) in WINDOW_ROLES]


def is_active(states):
    lowered = [state.lower() for state in states]
    return "active" in lowered or "focused" in lowered


def find_focused(root, start_path, max_nodes=4000):
    """Return the first focused element under `root`, or None."""
    queue = deque([(root, start_path)])
    visited = 0

    while queue and visited < max_nodes:
        accessible, path = queue.popleft()
        visited += 1

        if "focused" in states_of(accessible):
            item = describe(accessible)
            item["path"] = path
            return item

        for index, child in indexed_children_of(accessible):
            queue.append((child, path + [index]))

    return None


def active_context():
    """The active (or focused) top-level window across all applications.

    Several windows can report themselves active (for example gnome-shell's
    stage and the real app window), so prefer the one that actually contains
    the focused element.
    """
    actives = []
    focused_only = None

    for app_index, app in indexed_children_of(desktop()):
        app_name = name_of(app) or f"app {app_index}"

        for window_index, window in indexed_children_of(app):
            if role_name(window) not in WINDOW_ROLES:
                continue
            states = states_of(window)
            lowered = [state.lower() for state in states]

            item = {
                "app": app_index,
                "app_name": app_name,
                "window": window_index,
                "name": name_of(window),
                "role": role_name(window),
                "bounds": bounds_of(window),
                "states": states,
                "path": [app_index, window_index],
            }

            if "active" in lowered:
                actives.append(item)
            elif focused_only is None and "focused" in lowered:
                focused_only = item

    if len(actives) == 1:
        return actives[0]

    for candidate in actives:
        resolved = resolve(candidate["path"])

        if resolved is not None and find_focused(resolved, candidate["path"], 1500):
            return candidate

    if actives:
        return actives[-1]

    return focused_only


def resolve(path):
    accessible = desktop()

    for index in path:
        accessible = safe(lambda index=index: accessible.getChildAtIndex(index))
        if accessible is None:
            return None

    return accessible


def matches_query(accessible, name_query, role_query, text_query):
    name = name_of(accessible).lower()
    role = role_name(accessible).lower()

    name_ok = (not name_query) or (name_query.lower() in name)
    role_ok = (not role_query) or (role_query.lower() in role)

    text_ok = True

    if text_query:
        text_ok = text_query.lower() in text_of(accessible, 600).lower()

    return name_ok and role_ok and text_ok


def find_matches(name_query, role_query, app_index, limit, max_nodes, text_query=""):
    root = desktop()
    start_path = []

    if app_index is not None:
        child = safe(lambda: root.getChildAtIndex(app_index))
        if child is None:
            return []
        root = child
        start_path = [app_index]

    queue = deque([(root, start_path, 0)])
    matches = []
    visited = 0

    while queue and len(matches) < limit and visited < max_nodes:
        accessible, path, depth = queue.popleft()
        visited += 1

        if depth > 0 and matches_query(accessible, name_query, role_query, text_query):
            item = describe(accessible)
            item["path"] = path
            matches.append(item)

        for index, child in indexed_children_of(accessible):
            queue.append((child, path + [index], depth + 1))

    return matches


def handle_apps(_request):
    output = []

    for index, child in indexed_children_of(desktop()):
        windows = windows_of(child)

        entry = {
            "index": index,
            **describe(child),
            "windows": [name_of(window) for window in windows if name_of(window)],
            "active": any(is_active(states_of(window)) for window in windows),
        }

        output.append(entry)

    response = {"ok": True, "apps": output}

    warnings = environment_warnings()

    if warnings:
        response["warnings"] = warnings

    return response


def handle_tree(request):
    app_index = request.get("app")
    max_depth = max(0, min(int(request.get("max_depth", 5)), 12))
    max_nodes = max(1, min(int(request.get("max_nodes", 2000)), 20000))

    root = desktop()
    start_path = []

    path = request.get("path")
    if isinstance(path, list) and path:
        root = resolve(path)
        if root is None:
            return {"ok": False, "error": f"accessibility path {path} not found"}
        start_path = path

    elif app_index is not None:
        child = safe(lambda: root.getChildAtIndex(app_index))
        if child is None:
            return {"ok": False, "error": f"app index {app_index} not found"}
        root = child
        start_path = [app_index]

    queue = deque([(root, start_path, 0)])
    nodes = []
    visited = 0
    truncated = False

    while queue and visited < max_nodes:
        accessible, path, depth = queue.popleft()
        visited += 1

        node = describe(accessible)
        node["path"] = path
        node["depth"] = depth
        node["children_count"] = safe(lambda: accessible.childCount, 0) or 0
        nodes.append(node)

        if depth >= max_depth:
            truncated |= node["children_count"] > 0
            continue

        remaining = max_nodes - visited - len(queue)
        truncated |= node["children_count"] > remaining
        if remaining <= 0:
            continue
        for index, child in indexed_children_of(accessible, remaining):
            queue.append((child, path + [index], depth + 1))

    return {"ok": True, "nodes": nodes, "truncated": truncated or bool(queue)}


def handle_find(request):
    return {
        "ok": True,
        "matches": find_matches(
            request.get("name", ""),
            request.get("role", ""),
            request.get("app"),
            int(request.get("limit", 20)),
            int(request.get("max_nodes", 5000)),
            request.get("text", ""),
        ),
    }


def element_from_request(request):
    path = request.get("path")

    if path:
        return resolve(path), path

    matches = find_matches(
        request.get("name", ""),
        request.get("role", ""),
        request.get("app"),
        int(request.get("index", 0)) + 1,
        5000,
        request.get("text", ""),
    )

    if len(matches) <= int(request.get("index", 0)):
        return None, None

    match = matches[int(request.get("index", 0))]
    return resolve(match["path"]), match["path"]


CLICK_EQUIVALENT_ACTIONS = ("click", "press", "toggle")


def select_click_action(names, requested=""):
    lowered = [name.lower() for name in names]
    if requested:
        return lowered.index(requested.lower()) if requested.lower() in lowered else None
    for preferred in CLICK_EQUIVALENT_ACTIONS:
        if preferred in lowered:
            return lowered.index(preferred)
    return None


def handle_click(request):
    accessible, path = element_from_request(request)

    if accessible is None:
        return {"ok": False, "error": "element not found"}

    action = safe(accessible.queryAction)
    if action is None:
        bounds = bounds_of(accessible)
        if bounds and bounds.get("width", 0) > 0:
            hint = (
                "element has no action interface; its screen bounds are "
                f"x={bounds['x']} y={bounds['y']} w={bounds['width']} "
                f"h={bounds['height']} - use the computer tool "
                "mouse_click at the center of that box"
            )
        else:
            hint = "element has no action interface and no screen bounds"

        return {"ok": False, "error": hint}

    count = safe(lambda: action.nActions, 0) or 0
    names = [safe(lambda index=index: action.getActionName(index), "") or "" for index in range(count)]
    requested_action = (request.get("action_name") or "").strip()
    chosen = select_click_action(names, requested_action)
    if chosen is None:
        if requested_action:
            return {"ok": False, "error": f"element has no action named {requested_action!r} (available: {names})"}
        return {
            "ok": False,
            "error": (
                f"element has no click/press/toggle action (available: {names}). "
                "Use action_name explicitly for other actions, or mouse_click at its bounds."
            ),
        }

    # doAction failures were silently swallowed before (they were passed to
    # safe(), which returns None on any exception), so the tool reported
    # success while nothing happened on screen. Surface real errors.
    try:
        result = action.doAction(chosen)
    except Exception as error:
        return {"ok": False, "error": f"click action failed: {error}"}
    if result is False:
        return {"ok": False, "error": f"AT-SPI action {names[chosen]!r} returned false; verify the app state"}

    return {
        "ok": True,
        "action": safe(lambda: action.getActionName(chosen), "") or "",
        "path": path,
        "element": describe(accessible),
    }


def handle_focus(request):
    accessible, path = element_from_request(request)

    if accessible is None:
        return {"ok": False, "error": "element not found"}

    component = safe(accessible.queryComponent)
    if component is None:
        return {"ok": False, "error": "element has no component interface"}

    try:
        component.grabFocus()
    except Exception as error:
        return {"ok": False, "error": f"grabFocus failed: {error}"}

    return {"ok": True, "path": path, "element": describe(accessible)}


def handle_set_text(request):
    if "text" not in request:
        return {"ok": False, "error": "missing 'text'"}

    accessible, path = element_from_request(request)

    if accessible is None:
        return {"ok": False, "error": "element not found"}

    editable = safe(accessible.queryEditableText)
    if editable is None:
        text_interface = safe(accessible.queryText)

        if text_interface is not None:
            return {
                "ok": False,
                "error": (
                    "element is read-only text; focus it and use the "
                    "computer tool type_text / key_press instead"
                ),
            }

        return {"ok": False, "error": "element is not editable"}

    try:
        editable.setTextContents(request["text"])
    except Exception as error:
        return {"ok": False, "error": f"setTextContents failed: {error}"}

    return {"ok": True, "path": path, "element": describe(accessible)}


def handle_get_text(request):
    accessible, path = element_from_request(request)

    if accessible is None:
        return {"ok": False, "error": "element not found"}

    text_interface = safe(accessible.queryText)
    if text_interface is None:
        return {"ok": False, "error": "element has no text interface"}

    value = safe(lambda: text_interface.getText(0, -1), "") or ""

    return {"ok": True, "path": path, "text": value}


def handle_read(request):
    """Dump the visible text content of an app (or element) natively.

    This is the fast, native alternative to a screenshot + OCR: it reads the
    text interface of every visible element under the target and returns
    labeled lines, so the model can "see" what is on screen without pixels.
    """
    app_index = request.get("app")
    path = request.get("path")
    max_chars = int(request.get("max_chars", 4000))
    max_nodes = int(request.get("max_nodes", 1500))

    root = desktop()
    start_path = []

    if path:
        resolved = resolve(path)

        if resolved is None:
            return {"ok": False, "error": f"path {path} not found"}

        accessible, path = resolved, path
        queue = deque([(accessible, path, 0)])
    else:
        if app_index is not None:
            child = safe(lambda: root.getChildAtIndex(app_index))

            if child is None:
                return {"ok": False, "error": f"app index {app_index} not found"}

            root = child
            start_path = [app_index]
        else:
            queue = deque()

            for index, child in indexed_children_of(root):
                queue.append((child, [index], 0))

        queue = deque([(root, start_path, 0)]) if start_path or app_index is not None else queue

    lines = []
    used = 0
    visited = 0

    while queue and visited < max_nodes and used < max_chars:
        accessible, path, depth = queue.popleft()
        visited += 1

        name = name_of(accessible)
        value = text_of(accessible, 400)
        role = role_name(accessible)
        states = states_of(accessible)

        if value and ("SHOWING" in [s.upper() for s in states] or not states):
            label = f"{role}" if not name else f"{role} '{name}'"
            lines.append(f"[{'>'.join(map(str, path))}] {label}: {value}")
            used += len(value) + 24
        elif name and role in ("frame", "window", "dialog", "alert"):
            lines.append(f"[{'>'.join(map(str, path))}] {role} '{name}'")
            used += len(name) + 24

        for index, child in indexed_children_of(accessible):
            queue.append((child, path + [index], depth + 1))

    return {"ok": True, "lines": lines, "visited": visited}


def handle_windows(request):
    """List top-level windows across all apps with their active state."""
    output = []

    for app_index, app in indexed_children_of(desktop()):
        app_name = name_of(app) or f"app {app_index}"

        for window_index, window in indexed_children_of(app):
            role = role_name(window)

            if role not in ("frame", "window", "dialog", "alert"):
                continue

            item = {
                "app": app_index,
                "window": window_index,
                "app_name": app_name,
                "name": name_of(window),
                "role": role,
                "states": states_of(window),
                "active": is_active(states_of(window)),
                "bounds": bounds_of(window),
                "path": [app_index, window_index],
            }

            output.append(item)

    return {"ok": True, "windows": output}


def handle_active(request):
    """Report where the user actually is: active app/window and focused element.

    This is the fastest way for the model to orient itself before acting, and
    it surfaces the "blind spot" cases (no accessible tree) explicitly instead
    of returning an empty list.
    """
    window = active_context()

    if window is None:
        response = {
            "ok": False,
            "error": (
                "no active window is exposed through accessibility. The "
                "focused app may not publish a tree (enable toolkit "
                "accessibility / ACCESSIBILITY_ENABLED, see warnings), or it "
                "is a native app that hides its widgets. Fall back to the "
                "computer tool: screenshot + vision or OCR."
            ),
        }

        warnings = environment_warnings()

        if warnings:
            response["warnings"] = warnings

        return response

    resolved = resolve(window["path"])
    focused = find_focused(resolved, window["path"]) if resolved is not None else None

    response = {"ok": True, "active": window, "focused": focused}

    if request.get("read"):
        read_request = {
            "action": "read",
            "path": window["path"],
            "max_chars": request.get("max_chars", 4000),
            "max_nodes": request.get("max_nodes", 1500),
        }

        response["lines"] = handle_read(read_request).get("lines", [])

    warnings = environment_warnings()

    if warnings:
        response["warnings"] = warnings

    return response


def handle_app_state(request):
    """Return active-window context and its bounded accessibility subtree."""
    window = active_context()
    warnings = environment_warnings()
    if window is None:
        response = {
            "ok": False,
            "error": "no active window is exposed through accessibility",
            "active": None,
            "focused": None,
            "nodes": [],
            "truncated": False,
        }
        if warnings:
            response["warnings"] = warnings
        return response

    accessible = resolve(window["path"])
    focused = find_focused(accessible, window["path"]) if accessible is not None else None
    # Walk from the active window path so returned paths remain usable by the
    # existing accessibility actions.
    tree_request = {
        "path": window["path"],
        "max_depth": request.get("max_depth", 5),
        "max_nodes": request.get("max_nodes", 2000),
    }
    tree = handle_tree(tree_request)
    response = {
        "ok": True,
        "active": window,
        "focused": focused,
        "nodes": tree.get("nodes", []),
        "truncated": tree.get("truncated", False),
    }
    if tree.get("ok") is not True:
        response["tree_error"] = tree.get("error", "active-window tree could not be read")
    if response["truncated"]:
        warnings.append("active-window accessibility tree is partial because a depth or node limit was reached")
    if warnings:
        response["warnings"] = warnings
    return response


HANDLERS = {
    "apps": handle_apps,
    "active": handle_active,
    "app_state": handle_app_state,
    "tree": handle_tree,
    "find": handle_find,
    "click": handle_click,
    "focus": handle_focus,
    "set_text": handle_set_text,
    "get_text": handle_get_text,
    "read": handle_read,
    "windows": handle_windows,
}


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue

        try:
            request = json.loads(line)
        except Exception as error:
            print(json.dumps({"ok": False, "error": f"invalid json: {error}"}), flush=True)
            continue

        handler = HANDLERS.get(request.get("action"))

        if handler is None:
            response = {"ok": False, "error": f"unknown action {request.get('action')!r}"}
        else:
            try:
                response = handler(request)
            except Exception as error:
                response = {"ok": False, "error": f"{type(error).__name__}: {error}"}

        print(json.dumps(response), flush=True)


if __name__ == "__main__":
    main()
