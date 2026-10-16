# Research: carrying tool calls, thinking and attachments across harnesses

Date: 2026-10-16. Read-only research: source code and docs only. No harness was run and no
session store was opened.

Credit: the conversion design described here is taken from
[session-migrate](https://github.com/xhluca/session-migrate) by Xing Han Lu (xhluca), MIT
licence. DoppelClanker reimplements the ideas in Rust and includes none of its code.

## The gap in DoppelClanker today

`hsconv` converts only text between harnesses. Measured on one real OpenCode session:
307 text parts converted, 367 tool parts skipped.

| Content | from-claude | from-opencode | to-claude | to-opencode |
|---|---|---|---|---|
| Prompts and answers (text) | kept | kept | kept | kept |
| Tool calls and results | dropped | dropped | not carried | not carried |
| Thinking / reasoning | dropped | dropped | dropped | kept if present |
| Images / attachments | dropped | not handled | no | no |
| Compaction summaries | dropped | not handled | no | no |

## Existing tool that already solves this: xhluca/session-migrate

- MIT licence, Python, 135 stars, last push 2026-10-09.
- 18 harnesses: Claude Code, Codex, Pi, omp, OpenCode, Copilot, Cursor and others.
- The old Python daemon already depended on it (`smigrate`), before the Rust port dropped it.

### How it works

Each harness has a `parse()` into a shared `Session` of `Event`s and a `serialize()` out of it
(`src/session_migrate/model.py`). Each event has one of these kinds:

`MESSAGE`, `TOOL_CALL`, `TOOL_RESULT`, `THINKING`, `COMPACTION`, `CONTEXT`, `OPAQUE`

An event carries `role`, `text`, `tool_name`, `tool_call_id` and a `payload`
(for example tool `input`, `is_error`). Calls and results are linked by `tool_call_id`.

Native shapes, as each adapter reads and writes them:

| Harness | Tool call | Tool result | Thinking |
|---|---|---|---|
| Claude | assistant `tool_use` block `{id, name, input}` | user `tool_result` block `{tool_use_id, content, is_error}` | `thinking` / `redacted_thinking` with `signature` |
| Pi / omp | assistant `toolCall` block `{id, name, arguments}` | message `role: toolResult` with `toolCallId`, `isError` | `thinking` block with `thinkingSignature` |
| OpenCode | one `tool` part `{callID, tool, state: {status, input, output, attachments}}` that holds call and result together | (same part; `status` completed or error) | `reasoning` part |

### Rules it applies (worth copying)

1. **Pair every result with a call.** A result with no matching call, or a duplicate result,
   is dropped and counted. Missing call ids are generated. Claude rejects unpaired results.
2. **OpenCode splits in two.** One `tool` part becomes a `TOOL_CALL` plus a `TOOL_RESULT`.
   A tool still running (status not completed or error) becomes `OPAQUE` and is not carried.
3. **Count every drop.** Each omission goes into a content-free manifest (a counter by reason),
   so a lossy conversion is visible, not silent.
4. **Never move signed or private thinking.** Signatures are provider- and model-bound.
5. **Never modify the source session.**

### Thinking: what is possible

From their `docs/pi-thinking-traces.md`: Pi keeps a `thinkingSignature` and replays it only
for the same provider, API and model. For a different model it removes the signature and turns
non-redacted thinking into ordinary text; redacted thinking is dropped.

So:

- **Same harness, same model:** thinking can be kept exactly, signature included.
- **Different harness or model:** thinking can be carried as visible text only.
- **Into Claude:** never as a thinking block. Claude rejects foreign reasoning. It can only go
  in as plain assistant text, or be dropped.

## Decision: carry thinking across models as marked text

Across harnesses or models, non-redacted thinking is kept as plain text inside the assistant
message, before the answer, wrapped so the next model can tell it apart:

```
<prior_reasoning harness="omp" model="claude-opus">
...original thinking text...
</prior_reasoning>
```

- A neutral tag, not `<thinking>`, so the next model reads it as an earlier model's notes.
- The signature is always dropped; redacted thinking has no text and stays dropped.
- A converter must recognise its own `<prior_reasoning>` block on a round trip and keep it
  as is: never wrap it twice, never turn it back into a signed thinking block.
- Costs context on every resume, so it is a per-space setting: keep wrapped, or drop.

## Two options for DoppelClanker

### A. Call session-migrate for cross-harness conversion

Fast, 18 harnesses. Costs: a Python runtime dependency again, which the Rust port removed by
decision, and pinned harness versions it validates against.

### B. Port the event model into hsconv (Rust)

Extend the omp journal, our canonical form, to keep `toolCall` blocks, `toolResult` messages,
`thinking` blocks and images, which omp already supports natively. Then each converter maps to
and from those shapes using the rules above, with a drop manifest.

Recommendation: **B**, using session-migrate as the reference. Read its adapters and tests
for the exact native shapes; write our own Rust code. The MIT licence also allows copying
with attribution if a piece is easier to take as is.

## For the team server

Store the **original session file unchanged**. A same-harness handoff (Claude to Claude,
omp to omp) is then exact: tool calls, thinking with signatures, attachments. The converted
journal is only needed when the receiving harness differs.
