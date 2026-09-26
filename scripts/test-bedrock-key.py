#!/usr/bin/python3
"""Verify a Bedrock Mantle model actually returns text using a Blind-injected key.

Exit 0 means a non-empty chat reply was received. The chat response body is
saved privately for the user; the key and request headers are never saved.
"""

import json
import os
import sys
import urllib.error
import urllib.request


BASE = "https://bedrock-mantle.us-east-1.api.aws/v1"
RESPONSE_FILE = "/home/goutamsharma/.local/share/hyusk/bedrock-response.json"
API2_RESPONSE_FILE = "/home/goutamsharma/.local/share/hyusk/bedrock-response-api2.json"
MAX_RESPONSE_BYTES = 1_000_000


def save_chat_response(raw, key, path=RESPONSE_FILE):
    if len(raw) > MAX_RESPONSE_BYTES:
        raise SystemExit(9)
    # The provider should not echo credentials, but redact a literal echo if it does.
    content = raw.decode("utf-8", errors="replace").replace(key, "[REDACTED]")
    try:
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
        descriptor = os.open(path, flags, 0o600)
    except OSError:
        raise SystemExit(9) from None
    with os.fdopen(descriptor, "w", encoding="utf-8") as output:
        output.write(content)


def request(path, key, body=None, response_file=RESPONSE_FILE):
    data = json.dumps(body).encode() if body is not None else None
    method = "POST" if data is not None else "GET"
    headers = {"Authorization": f"Bearer {key}", "Accept": "application/json"}
    if data is not None:
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=25) as response:
            raw = response.read(MAX_RESPONSE_BYTES + 1) if path == "/chat/completions" else response.read()
            if path == "/chat/completions":
                save_chat_response(raw, key, response_file)
            return json.loads(raw)
    except urllib.error.HTTPError as error:
        if path == "/chat/completions":
            save_chat_response(error.read(MAX_RESPONSE_BYTES + 1), key, response_file)
        if error.code in (401, 403):
            raise SystemExit(5) from None  # key or account/model access denied
        if error.code in (400, 404, 422):
            raise SystemExit(6) from None  # endpoint/model/request rejected
        raise SystemExit(7) from None
    except (urllib.error.URLError, TimeoutError, ValueError):
        raise SystemExit(8) from None


def choose_model(ids):
    # Mantle model IDs differ from the bedrock-runtime foundation-model IDs.
    return next((model for model in (
        "openai.gpt-oss-20b", "openai.gpt-oss-120b"
    ) if model in ids), None)


def has_visible_answer(result):
    choices = result.get("choices") if isinstance(result, dict) else None
    if not isinstance(choices, list) or not choices or not isinstance(choices[0], dict):
        return False
    choice = choices[0]
    if choice.get("finish_reason") == "length":
        return False
    message = choice.get("message")
    content = message.get("content") if isinstance(message, dict) else None
    if isinstance(content, str):
        return bool(content.strip())
    if isinstance(content, list):
        return any(isinstance(part, dict) and isinstance(part.get("text"), str)
                   and part["text"].strip() for part in content)
    return False


def main():
    keys = [
        (os.environ.get(alias, "").strip(), path)
        for alias, path in (("AWS_API_KEY", RESPONSE_FILE), ("AWS_API2_KEY", API2_RESPONSE_FILE))
        if os.environ.get(alias, "").strip()
    ]
    if len(keys) != 1:
        return 2
    key, response_file = keys[0]
    catalog = request("/models", key)
    models = catalog.get("data", []) if isinstance(catalog, dict) else []
    ids = {item.get("id") for item in models if isinstance(item, dict)} if isinstance(models, list) else set()
    model = choose_model(ids)
    if model is None:
        return 3
    result = request("/chat/completions", key, {
        "model": model,
        "messages": [{"role": "user", "content": "Reply with OK."}],
        "max_tokens": 512,
    }, response_file)
    return 0 if has_visible_answer(result) else 4


if __name__ == "__main__":
    sys.exit(main())
