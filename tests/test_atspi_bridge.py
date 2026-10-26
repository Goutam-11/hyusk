"""Regression tests for stable AT-SPI paths and safe action selection."""

import sys
import io
import json
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import atspi_bridge as bridge  # noqa: E402


class FakeAccessible:
    def __init__(self, children=()):
        self.children = children
        self.childCount = len(children)

    def getChildAtIndex(self, index):
        child = self.children[index]
        if child is None:
            raise RuntimeError("temporary AT-SPI child read failure")
        return child


class BridgeTests(unittest.TestCase):
    def test_failed_child_read_does_not_renumber_paths(self):
        root = FakeAccessible((FakeAccessible(), None, FakeAccessible()))
        self.assertEqual([index for index, _ in bridge.indexed_children_of(root)], [0, 2])

    def test_click_does_not_substitute_a_different_action(self):
        names = ["activate", "press", "toggle"]
        self.assertEqual(bridge.select_click_action(names), 1)
        self.assertEqual(bridge.select_click_action(names, "toggle"), 2)
        self.assertEqual(bridge.select_click_action(["activate", "jump"]), 0)

    def test_click_reports_an_action_that_returns_false(self):
        class FailedAction:
            nActions = 1

            def getActionName(self, _index):
                return "click"

            def doAction(self, _index):
                return False

        class Element:
            def queryAction(self):
                return FailedAction()

        with patch.object(bridge, "element_from_request", return_value=(Element(), [2, 4])):
            result = bridge.handle_click({"path": [2, 4]})
        self.assertFalse(result["ok"])
        self.assertIn("returned false", result["error"])

    def test_focus_reports_when_component_rejects_focus(self):
        class Component:
            def grabFocus(self):
                return False

        class Element:
            def queryComponent(self):
                return Component()

        with patch.object(bridge, "element_from_request", return_value=(Element(), [1])):
            result = bridge.handle_focus({"path": [1]})

        self.assertFalse(result["ok"])
        self.assertIn("grabFocus returned false", result["error"])

    def test_set_text_reports_when_editable_interface_rejects_update(self):
        class EditableText:
            def setTextContents(self, _value):
                return False

        class Element:
            def queryEditableText(self):
                return EditableText()

        with patch.object(bridge, "element_from_request", return_value=(Element(), [1])):
            result = bridge.handle_set_text({"path": [1], "text": "new value"})

        self.assertFalse(result["ok"])
        self.assertIn("setTextContents returned false", result["error"])

    def test_click_does_not_offer_coordinates_for_zero_height_bounds(self):
        class Element:
            def queryAction(self):
                return None

            def queryComponent(self):
                return object()

        with patch.object(bridge, "element_from_request", return_value=(Element(), [2])), patch.object(
            bridge, "bounds_of", return_value={"x": 20, "y": 30, "width": 40, "height": 0}
        ):
            result = bridge.handle_click({"path": [2]})

        self.assertFalse(result["ok"])
        self.assertNotIn("coordinate_fallback", result)

    def test_tree_reports_when_node_budget_omits_children(self):
        root = FakeAccessible((FakeAccessible(), FakeAccessible()))
        with patch.object(bridge, "desktop", return_value=root), patch.object(
            bridge, "describe", return_value={"role": "frame", "name": "test"}
        ):
            result = bridge.handle_tree({"max_nodes": 2})
        self.assertTrue(result["truncated"])
        self.assertEqual(len(result["nodes"]), 2)

    def test_find_caps_direct_bridge_limits(self):
        root = FakeAccessible(tuple(FakeAccessible() for _ in range(250)))
        with patch.object(bridge, "desktop", return_value=root), patch.object(
            bridge, "matches_query", return_value=True
        ), patch.object(bridge, "describe", return_value={"role": "button"}):
            result = bridge.handle_find({"limit": 1000000, "max_nodes": 1000000})

        self.assertEqual(len(result["matches"]), 200)

    def test_negative_element_index_does_not_select_last_match(self):
        with patch.object(bridge, "find_matches") as find:
            self.assertEqual(bridge.element_from_request({"index": -1}), (None, None))
        find.assert_not_called()

    def test_read_obeys_character_cap_even_for_a_long_single_line(self):
        root = FakeAccessible((FakeAccessible(),))
        with patch.object(bridge, "desktop", return_value=root), patch.object(
            bridge, "name_of", return_value=""
        ), patch.object(bridge, "text_of", return_value="x" * 400), patch.object(
            bridge, "role_name", return_value="label"
        ), patch.object(bridge, "states_of", return_value=[]):
            result = bridge.handle_read({"path": [0], "max_chars": 200})

        self.assertLessEqual(sum(map(len, result["lines"])), 200)
        self.assertTrue(result["truncated"])

    def test_non_object_json_does_not_terminate_bridge_loop(self):
        output = io.StringIO()
        with (
            patch.object(
                bridge.sys,
                "stdin",
                io.StringIO('[]\n{"action":[]}\n{"action":"windows"}\n'),
            ),
            patch.object(bridge.sys, "stdout", output),
            patch.dict(
                bridge.HANDLERS,
                {"windows": lambda _request: {"ok": True, "windows": []}},
            ),
        ):
            bridge.main()

        responses = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertEqual(responses[0]["error"], "request must be a JSON object")
        self.assertEqual(responses[1]["error"], "request action must be a string")
        self.assertTrue(responses[2]["ok"])

    def test_app_state_uses_active_window_path_and_bounded_tree(self):
        window = FakeAccessible((FakeAccessible(), FakeAccessible()))
        desktop = FakeAccessible((FakeAccessible((window,)),))
        active = {"app": 0, "app_name": "Editor", "window": 0, "name": "Draft", "path": [0, 0]}
        with patch.object(bridge, "desktop", return_value=desktop), patch.object(
            bridge, "active_context", return_value=active
        ), patch.object(bridge, "find_focused", return_value=None), patch.object(
            bridge, "describe", side_effect=lambda *_args: {"role": "frame", "name": "Draft"}
        ), patch.object(bridge, "environment_warnings", return_value=[]):
            result = bridge.handle_app_state({"max_nodes": 2})

        self.assertTrue(result["ok"])
        self.assertEqual(result["active"]["path"], [0, 0])
        self.assertEqual([node["path"] for node in result["nodes"]], [[0, 0], [0, 0, 0]])
        self.assertTrue(result["truncated"])

    def test_app_state_reports_no_active_window_for_screenshot_fallback(self):
        with patch.object(bridge, "active_context", return_value=None), patch.object(
            bridge, "environment_warnings", return_value=[]
        ):
            result = bridge.handle_app_state({})

        self.assertFalse(result["ok"])
        self.assertIsNone(result["active"])
        self.assertEqual(result["nodes"], [])


if __name__ == "__main__":
    unittest.main()
