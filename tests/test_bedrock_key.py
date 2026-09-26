"""The Bedrock smoke test must never pass on catalog access alone."""

import importlib.util
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "test-bedrock-key.py"
SPEC = importlib.util.spec_from_file_location("test_bedrock_key_script", SCRIPT)
bedrock = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bedrock)


class BedrockSmokeTests(unittest.TestCase):
    def test_mantle_model_ids_only(self):
        self.assertEqual(bedrock.choose_model({"openai.gpt-oss-20b"}), "openai.gpt-oss-20b")
        self.assertIsNone(bedrock.choose_model({"openai.gpt-oss-20b-1:0"}))

    def test_choices_without_visible_text_are_not_success(self):
        self.assertFalse(bedrock.has_visible_answer({"choices": [{"message": {"content": None}}]}))
        self.assertFalse(bedrock.has_visible_answer({"choices": [{"message": {"content": " "}}]}))
        self.assertFalse(bedrock.has_visible_answer({"choices": [{"message": {"content": "OK"}, "finish_reason": "length"}]}))
        self.assertTrue(bedrock.has_visible_answer({"choices": [{"message": {"content": "OK"}}]}))

    def test_catalog_without_supported_model_fails(self):
        with patch.dict("os.environ", {"AWS_API_KEY": "test-only"}, clear=True), patch.object(
            bedrock, "request", return_value={"data": [{"id": "unrelated.model"}]}
        ) as request:
            self.assertEqual(bedrock.main(), 3)
            request.assert_called_once_with("/models", "test-only")

    def test_empty_chat_response_fails(self):
        with patch.dict("os.environ", {"AWS_API_KEY": "test-only"}, clear=True), patch.object(
            bedrock,
            "request",
            side_effect=[
                {"data": [{"id": "openai.gpt-oss-20b"}]},
                {"choices": [{"message": {"content": None}}]},
            ],
        ):
            self.assertEqual(bedrock.main(), 4)

    def test_second_key_uses_separate_response_file(self):
        with patch.dict("os.environ", {"AWS_API2_KEY": "test-only"}, clear=True), patch.object(
            bedrock, "request", side_effect=[
                {"data": [{"id": "openai.gpt-oss-20b"}]},
                {"choices": [{"message": {"content": "OK"}}]},
            ],
        ) as request:
            self.assertEqual(bedrock.main(), 0)
            self.assertEqual(request.call_args.args[-1], bedrock.API2_RESPONSE_FILE)

    def test_two_keys_fail_closed(self):
        with patch.dict("os.environ", {"AWS_API_KEY": "one", "AWS_API2_KEY": "two"}, clear=True):
            self.assertEqual(bedrock.main(), 2)

    def test_response_file_is_private_and_redacts_literal_key(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "response.json"
            bedrock.save_chat_response(b'{"content":"test-only OK"}', "test-only", str(target))
            self.assertEqual(target.stat().st_mode & 0o777, 0o600)
            self.assertEqual(target.read_text(), '{"content":"[REDACTED] OK"}')
            with self.assertRaises(SystemExit) as failure:
                bedrock.save_chat_response(b"overwrite", "test-only", str(target))
            self.assertEqual(failure.exception.code, 9)


if __name__ == "__main__":
    unittest.main()
