#!/bin/sh
# A minimal external tool server for the T2 host tests.
#
# Usage: external_server.sh <mode> [log-file] [state-file] [pid-file]
#
# It speaks the T2 host protocol: newline-delimited JSON-RPC 2.0 on stdin and
# stdout, an `initialize` handshake carrying one tool called `echo`, and
# `tools/call` returning MCP-shaped text content. The modes make the failure
# paths reproducible:
#
#   happy       echo the text argument back
#   secret      return a secret-shaped value the host must redact
#   crash       exit on the first call
#   crash-once  exit on the first call, then serve normally
#   idle-once   exit just after the first handshake, then serve normally
#   timeout     sleep on a call so the host's deadline fires
#   oversized   return a 40,000-character text
#
# The optional files let a test confirm that a server saw (or did not see) a
# call, how many times it restarted, and which process the host shut down.
# This is test support, not an installed program.

set -u

mode="${1:-happy}"
log="${2:-}"
state="${3:-}"
pid_file="${4:-}"

if [ -n "$pid_file" ]; then
    printf '%s\n' "$$" > "$pid_file"
fi

log_line() {
    if [ -n "$log" ]; then
        printf '%s\n' "$1" >> "$log"
    fi
}

field() {
    # The first occurrence of a JSON string field in the request line. The
    # fixture's tests use values without escapes, so this stays readable.
    printf '%s' "$1" | sed -n "s/.*\"$2\":\"\\([^\"]*\\)\".*/\\1/p"
}

respond() {
    printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$1" "$2"
}

while IFS= read -r line; do
    log_line "$line"
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    if [ -z "$id" ]; then
        continue
    fi
    case "$line" in
        *'"method":"initialize"'*)
            respond "$id" '{"protocolVersion":"1.0","server":{"name":"fixture","version":"1.2.3"},"tools":[{"name":"echo","description":"Echo the text argument back from the fixture server.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}}]}'
            if [ "$mode" = "idle-once" ]; then
                if [ -n "$state" ] && [ -f "$state" ]; then
                    :
                else
                    if [ -n "$state" ]; then
                        : > "$state"
                    fi
                    exit 0
                fi
            fi
            ;;
        *'"method":"tools/call"'*)
            case "$mode" in
                crash)
                    exit 3
                    ;;
                crash-once)
                    if [ -n "$state" ] && [ -f "$state" ]; then
                        text=$(field "$line" text)
                        respond "$id" "{\"content\":[{\"type\":\"text\",\"text\":\"echo: $text\"}]}"
                    else
                        if [ -n "$state" ]; then
                            : > "$state"
                        fi
                        exit 3
                    fi
                    ;;
                secret)
                    respond "$id" '{"content":[{"type":"text","text":"OPENROUTER_API_KEY=sk-or-v1-0123456789abcdef"}]}'
                    ;;
                timeout)
                    sleep 30
                    ;;
                oversized)
                    big=$(head -c 40000 /dev/zero | tr '\0' 'x')
                    respond "$id" "{\"content\":[{\"type\":\"text\",\"text\":\"$big\"}]}"
                    ;;
                *)
                    text=$(field "$line" text)
                    respond "$id" "{\"content\":[{\"type\":\"text\",\"text\":\"echo: $text\"}]}"
                    ;;
            esac
            ;;
    esac
done
