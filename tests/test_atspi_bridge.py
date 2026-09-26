"""Regression tests for stable AT-SPI paths and safe action selection."""

import sys
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
        self.assertIsNone(bridge.select_click_action(["activate", "jump"]))

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

    def test_tree_reports_when_node_budget_omits_children(self):
        root = FakeAccessible((FakeAccessible(), FakeAccessible()))
        with patch.object(bridge, "desktop", return_value=root), patch.object(
            bridge, "describe", return_value={"role": "frame", "name": "test"}
        ):
            result = bridge.handle_tree({"max_nodes": 2})
        self.assertTrue(result["truncated"])
        self.assertEqual(len(result["nodes"]), 2)

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
