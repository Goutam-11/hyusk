# Agent loop performance and interaction research

Reviewed on 2026-10-21 against the upstream source for [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness/tree/master/packages/core/agent-loop), [Pi agent core](https://github.com/earendil-works/pi/tree/main/packages/agent), and [OpenCode's session runner](https://github.com/anomalyco/opencode/tree/dev/packages/core/src/session/runner).

## Patterns Hyusk adopts

- Keep cancellation, tool results, and session writes within the same turn lifecycle. DeepSeek Harness persists accepted facts between steps, preserves delivered text on cancellation, and only overlaps tools declared parallel-safe; exclusive tools remain ordering barriers.
- Distinguish steering from follow-up. Pi's loop can accept steering while work is running and queue follow-up input for after the current work. Hyusk now represents scheduled input and model-requested voice continuation as distinct events, so an old follow-up cannot replace a newly spoken command.
- Treat each microphone interaction as a generation. Hyusk validates normal STT and Nova final completions against the active capture ID; model-requested follow-up listening is also tied to the finished turn ID.
- Detect stalled loops from repeated calls and results, while retaining bounded round and time limits. OpenCode's session processor also has a repeated-tool-call guard; neither approach makes an unchanged success result proof of progress.
- Keep request schemas and execution validation aligned. Pi validates tool arguments before execution; Hyusk's browser schema now describes required action fields and rejects empty identifiers/selectors or non-HTTP(S) navigation before CDP access.
- Workflow safety recognizes exact expansions of Hyusk's built-in app aliases, so the approval check does not mistake a resolved Flatpak/app command for an unknown executable.

These projects are references, not drop-in dependencies. Hyusk uses different providers, local Linux controls, Nova's live audio protocol, and a single desktop action lane. In particular, parallelizing desktop mutations would create focus races.

## Hyusk changes in this pass

- Runtime-generated internal follow-ups stay in an actor-local queue instead of being sent into the same bounded channel the actor must drain. Incoming user/stop events are checked first. Emergency stop clears stale internal and scheduled input.
- New voice/text input signals active turn, capture, and desktop-workflow cancellation before waiting for cleanup. Bounded cleanup waits run concurrently, reducing avoidable serial delay while still waiting before a replacement desktop interaction starts.
- Nova final events carry a capture ID. A canceled session can no longer hide a replacement interaction, overwrite its response card, or resume media for it.
- Deterministic fast commands now share the turn deadline and cancellation-aware tool runner. A command cannot start more actions after interruption; its journal keeps the in-flight result uncertain for safe resumption.
- Tool registry iteration is sorted, so tool specifications remain stable across launches and model requests.
- Provider streaming retries only transient HTTP/network failures before SSE begins. It does not replay after partial or malformed stream output. A plain request is used only after an explicit streaming-mode rejection (or image rejection), avoiding duplicate model charges on authentication/schema failures.
- Bedrock Mantle may close a valid Chat Completions stream after its terminal finish_reason without a [DONE] sentinel. Hyusk accepts EOF only after a finish reason; EOF earlier remains an error. Streaming stays enabled for Bedrock and other providers.
- The history budget is derived from the selected model's context window instead of a single global size, and history is compacted at tool boundaries inside a turn, not only at turn entry. A pending approval still blocks compaction because its result position is index-bound.
- Scheduled agent input no longer competes with a pending tool confirmation. While an approval is armed, a due scheduled task is queued instead of being handed to the turn as its answer, and it is released once the confirmation is resolved. An emergency stop disarms the approval so an unrelated later "yes" cannot execute the aborted action. A confirmation the user never answers also expires on a runtime timer: expiry records that nothing ran in the tool result, disarms the action, and releases the queue.
- Streamed replies are decoded into speakable sentences as they arrive. The decoder recovers the `text` field of a response envelope (or plain tool-call prose) incrementally, so the first sentence reaches speech and the UI event channel before generation finishes. A reply that was partially spoken is never replayed through a plain fallback request, and the final answer is not sent to speech a second time.
- Tool execution timing can be inspected with the existing HYUSK_TIMING=1 setting.

## Follow-up architecture work

1. Add tool execution classes (read-only, desktop-exclusive, independent background) as explicit metadata. Only then allow bounded parallel read-only work; keep focus-changing UI actions serial.
2. Read context windows from provider catalogs where they exist. The current budget derives a window from the model identifier, so a locally hosted model with a tuned context size still needs `HYUSK_AGENT_MAX_HISTORY_BYTES`.
3. Add local latency traces keyed by turn/tool/provider with durations and error categories, excluding user text, credentials, and page contents. Compare p50/p95 before changing timeouts or introducing caches.
4. Consider provider-neutral structured tool outcomes (ok, failed, timed out, cancelled, uncertain) in a versioned conversation format. Preserve the current OpenAI-compatible tool-result pairing at the provider boundary.
5. Move Nova intermediate UI deltas onto the same capture-ID validation path as its final result; final completions are now protected, but intermediate progress can still arrive while a newer capture is active.
6. Build a replay/evaluation set from synthetic desktop scenarios: one-step app launch, multi-step browser navigation, interrupted mutation with uncertain result, malformed tool arguments, stalled tool loop, and scheduled input arriving during a manual task.

Do not add tool parallelism or aggressive retries until the action has an explicit concurrency class or the request is known to be safe to replay.
