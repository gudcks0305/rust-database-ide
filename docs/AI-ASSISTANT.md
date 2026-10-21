# Optional Codex SQL assistant (F41 subset)

`dbide-assistant` is a UI-independent, opt-in client for an explicitly configured
**local Codex app-server WebSocket**. It does not use OpenAI API keys or the
Responses HTTP API, discover desktop ports, log in, read credentials, launch a
server, or expose any SQL execution/apply method. The caller owns server startup,
authentication, the empty scratch directory, and shutdown.

## Contract and evidence

Protocol reference: [official OpenAI Codex App Server documentation](https://developers.openai.com/codex/app-server).
Local schema checked on 2026-10-21 using `codex-cli 0.160.1`:

```sh
codex --version
codex app-server generate-json-schema --out tmp/codex-schema
```

Messages omit `jsonrpc`, with one JSON object per WebSocket text frame. The
client sends `initialize`, waits for its result, sends `initialized`, confirms
`account/read` returns the existing ChatGPT login, creates a new ephemeral
`thread/start`, then calls `turn/start`. It never resumes an
existing chat. Local schema values are `sandbox: "read-only"` on thread start
and `sandboxPolicy: {"type":"readOnly","networkAccess":false}` on turn start.
Approval policy is `untrusted`, reviewer is `user`; this client never approves.
The current online reference describes some fields absent from 0.160.1's local
schema, so code follows the locally generated schema for this transport.

Agent-message deltas are transient progress. Completed `agentMessage` items are
authoritative: commentary is omitted, final-answer items are preferred when
present, and legacy messages without phase remain supported. Only the matching
thread and turn may complete a request. A failed or interrupted turn returns an
error instead of a partial suggestion. A JSON model answer can populate `sql`
and `explanation`; other text stays visible as untrusted explanation/raw text.
There is no automatic copying, applying, query execution, or claim that generated
SQL is correct.

Successful suggestions carry optional completion metadata populated only after
the matching terminal event: opaque thread/turn IDs and the model resolved by
the server's thread/start response. It does not contain reasoning or credentials.

WebSocket app-server transport is experimental and unsupported according to the
official documentation. This is an integration component, not a claim of Codex
Desktop's internal IPC compatibility or a stable production protocol.

## Integration

Construct `AssistantConfig::codex("ws://127.0.0.1:PORT", absolute_scratch_dir)`.
It starts disabled. Set `enabled` only after the user opts in. Model is optional:
`None` uses the authenticated server's configured model; an explicit identifier
must come from the caller's selection. Availability and account usage limits
remain server-owned. No API billing or subscription-entitlement assumption is
made by the client.

An optional `capability_token_file` supplies the owned listener's local bearer
capability. This is **not an OpenAI API key**. On Unix, the client requires a
regular 0600 file, rejects symlinks, reads at most 4097 bytes, and rejects empty,
non-ASCII-graphic, or >4096-byte content. The temporary token buffer is zeroized
and the Authorization header is marked sensitive. On other platforms token-file
loading is currently unsupported. No token value enters configuration Debug,
request JSON, preview, or errors. Use a protected listener started with the
CLI's `--ws-auth capability-token --ws-token-file /absolute/path` flags.

Call `build_preview(task, &context)` before sending, show the selected instruction,
SQL, and schema locally, then call `generate(task, &context, &cancel)` on explicit
user action. `generate_with_progress` additionally reports untrusted text deltas.
The preview includes destination origin and byte counts, with content omitted.
Its request-byte count uses a placeholder thread ID because the real thread ID
does not exist yet; it is an estimate, not the eventual wire size.

Supported tasks: `GenerateSql`, `Explain`, and `Optimize`. Explain/Optimize require
nonempty selected SQL. Context has only an instruction, optional SQL, and optional
schema. The library does not access a connection profile, password, database,
result rows, query history, project files, or cached schema to assemble context.
The caller must exclude sensitive literals from the explicitly selected text.
This is a scope boundary, not an automatic SQL/DDL secret scrubber.

## Server isolation boundary

Only numeric loopback `ws://` endpoints with an explicit port are accepted. There
is no arbitrary remote endpoint, DNS name, URL credential, query-string token,
redirect following, or plaintext production-network API connection.

**The configured server must be a trusted, restricted, caller-owned process.**
An empty scratch cwd and read-only sandbox alone do not prevent an agent from
reading other files in Codex 0.160.1. The launcher must independently disable
shell and other runtime tools, MCP servers, plugins/apps, skills, and inherited
workspace instructions/config as appropriate. The library's explicit context
construction is verified; a guarantee that the provider sees only that selected
context depends on the launcher's effective configuration and remains a separate
gate. Prompts that prohibit tools are not a security boundary.

Every server-initiated request is refused with a JSON-RPC error, including
approval, tool, and user-input requests. Tool activity reported as an item causes
failure and a best-effort turn interruption. That detection cannot undo a tool
that already ran on an incorrectly configured server. The client has no method
to execute a tool or database operation itself.

## Limits and cancellation

- Selected instruction: 64 KiB; SQL and schema: 256 KiB each, measured as UTF-8 bytes.
- Serialized outbound message, inbound frame/message, and accumulated text: 1 MiB.
- Aggregate inbound protocol text: 4 MiB per request, including metadata.
- Default timeout: 60 seconds per connect/turn phase; maximum configurable timeout: 300 seconds.
- No retries. RPC rejection returns a static login/model/usage-limits hint without
  echoing server error text or model content. Transport errors are also static.
- Cancellation/timeout attempts `turn/interrupt` only for the known thread/turn;
  the write has a 100 ms bound. It does not await terminal acknowledgement and
  does not claim upstream inference or quota consumption stopped. Dropping the
  future closes local I/O; callers needing guaranteed shutdown must own and stop
  their app-server process.

`Suggestion` and selected context Debug are content-redacted. Raw response text
is intentionally available to the caller for review, so callers must avoid
logging it or interpreting it as trusted UI markup.

## Verification

```sh
cargo test -p dbide-assistant
cargo clippy -p dbide-assistant --all-targets -- -D warnings
```

Tests use synthetic loopback WebSocket servers only: initialize ordering,
ephemeral thread policy, selected schema/context scope, authoritative final
items versus deltas, early notifications, RPC/malformed/failed/empty responses,
response-size enforcement, disabled/pre-cancelled no-connect behavior, timeout,
in-flight cancellation, approval refusal, tool-activity rejection, and private
capability-file/header handling. No paid model call, real credential lookup,
Keychain operation, database connection, or existing Codex chat operation is part
of this suite. A passing mock suite establishes local protocol behavior; it does
not establish live model availability, effective launcher isolation, or an
end-to-end UI integration.

## Owned local launcher

From the repository root:

```sh
bash scripts/codex-assistant-server.sh check
bash scripts/codex-assistant-server.sh start
bash scripts/codex-assistant-server.sh status
bash scripts/codex-assistant-server.sh stop
```

The launcher fixes the address to `ws://127.0.0.1:45457`. Its local files are
`tmp/codex-ws/token` (random 64-character capability, 0600),
`tmp/codex-ws/workspace/` (empty, 0700), and
`tmp/codex-ws/assistant-server.json` (owned process identity, 0600). Start refuses
a nonempty scratch directory or an already occupied port. It uses an operation
lock, and a second start reuses only the recorded owned process. Stop checks PID,
process creation time, and full command including this endpoint and capability
path before SIGTERM. It never uses `killall`/`pkill` or controls unrelated Codex
processes. If PID identity differs or shutdown does not finish, it refuses to
escalate. Capability rotation uses a new private file plus atomic replacement.

The script uses `uv run --offline --python '>=3.14'` and the installed `codex`.
It preserves the current `CODEX_HOME` environment and its existing authentication;
it never copies `auth.json`, API keys, OAuth tokens, or Keychain material. It
writes no global configuration or existing chat. Only the local capability is
created. Normal launcher output contains paths, PID, and entry counts; server
stdout/stderr are discarded instead of capturing raw content. Codex itself may
maintain its own local runtime logs under its existing home.

Runtime CLI overrides include:

- `approval_policy="never"`, `sandbox_mode="read-only"`, and disabled web search.
- Shell/unified exec, apps/connectors, browser/computer use, image generation/view,
  both multi-agent versions, Code Mode/host, JS REPL, skill search, tool suggestion,
  host skill discovery, shell snapshots, and remote control disabled.
- Hook engines `hooks`, `codex_hooks`, and `plugin_hooks` disabled, every inline hook
  event cleared, and `notify=[]`. Hook state IDs are counted without executing or
  rewriting them; engine flags suppress them even when IDs contain dots/paths.
- Memory injection/generation disabled and `project_doc_max_bytes=0`.
- `skills.include_instructions=false`; inherited system, developer, and compact
  prompt text cleared. Any configured `model_instructions_file`,
  `experimental_compact_prompt_file`, or `model_catalog_json` causes startup to
  fail before those files are read, including overrides found in parsed profiles.
- Account-specific MCP/plugin IDs read locally from config TOML (including profiles
  and ancestor project layers), with each server/plugin explicitly disabled.
  Config source files are bounded to 4 MiB. No commands, environment values, auth
  data, or raw configuration are printed.
- Analytics and OTEL exporters/prompt/response logging disabled for this process,
  legacy command history persistence disabled, and inherited developer instructions
  cleared. No global settings are changed.
- Built-in `model_provider="openai"` and `forced_login_method="chatgpt"` selected.
  The child environment drops OpenAI/Codex API-key/base-URL override variables
  without reading or printing their values. Custom built-in OpenAI base URLs and
  nonofficial ChatGPT base URLs cause startup failure. Each generation separately
  checks ChatGPT account type before creating its thread; an API-key account is
  rejected rather than silently used as a fallback.

The [official configuration reference](https://developers.openai.com/codex/config-reference)
establishes the shell-tool, document-byte, memory, and web-search settings. Other
feature switches were checked against the installed 0.160.1 config schema and
accepted by an actual launch. CLI 0.160.1 splits dotted override keys literally;
TOML quotes embedded in an ID become literal key characters. The launcher passes
unquoted path segments as individual process arguments, and refuses MCP/plugin
IDs containing dots, `=`, or control characters rather than silently failing to
disable them. Managed or future configuration layers may still require separate
review. Named permission profiles do not compose with the loaded global sandbox
mode and are not used as proof of restricted filesystem roots.

These settings reduce exposure; they do not turn the local Codex agent into a
provably tool-free model endpoint. In this version an `apply_patch` builtin can
remain advertised, with writes blocked by the read-only policy. The client still
refuses every server request and aborts reported tool activity. The selected-
context-only security guarantee remains **unverified**. A trusted owner must
review effective launch configuration before sending sensitive SQL or schema.

## Metadata-only live probe

After starting the owned listener:

```sh
cargo run -p dbide-assistant --example probe_codex -- \
  ws://127.0.0.1:45457 \
  "$PWD/tmp/codex-ws/workspace" \
  "$PWD/tmp/codex-ws/token"
```

`probe_metadata` sends only initialize/initialized, `account/read` with
`refreshToken=false`, `model/list`, and `config/read` with `includeLayers=false`.
It does not create a thread or start a model turn. The returned projection omits
account emails, credentials, raw config, and conversation content.

Verified on 2026-10-21 with installed `codex-cli 0.160.1`:

- Authenticated ChatGPT account detected without copying credentials.
- Authenticated local capability handshake and full model page returned.
- Shell tool disabled; all three hook engines disabled.
- Six configured MCP servers, zero enabled; fifteen configured plugins, zero enabled.
- Project instruction bytes zero; memory injection and generation disabled.
- Skills instruction injection disabled; inherited instruction strings empty;
  custom instruction/catalog files absent; built-in OpenAI and ChatGPT-only login
  requirements confirmed in effective configuration.
- Owned start/status/stop lifecycle completed successfully.

The metadata-only probe verifies configuration and transport/authentication. It
does not establish model inference, universal account entitlement, actual
tool-free behavior, or end-to-end UI assistance.

## Explicit synthetic live smoke

After integration-owner review, one authorized benign turn was run on 2026-10-21:

```sh
cargo run -p dbide-assistant --example smoke_codex -- \
  --confirm-live-turn ws://127.0.0.1:45457 \
  "$PWD/tmp/codex-ws/workspace" "$PWD/tmp/codex-ws/token" gpt-6.1-sol
```

The example requires the explicit live-turn flag, explicit paths/model, existing
ChatGPT account type, and passing restriction metadata before starting inference.
It sends only a synthetic instruction and `SELECT 1 AS result;`, without schema,
files, history, database credentials, or an actual database. The resolved model
was `gpt-6.1-sol`; the complete client generation call took **4,693 ms** through
the matching `turn/completed` event and returned
the exact synthetic SQL plus a 43-byte explanation. No tool activity or approval
request was observed. SQL was not executed. The example prints only this fixture
result and terminal summary, omitting raw model text, reasoning, and capability.

This establishes one successful authenticated ChatGPT-route inference on the
owned local listener. It does not establish response quality for real queries,
universal model availability, a quota amount/cost, hard selected-context isolation,
or an end-to-end graphical flow. This opt-in example is never run by normal tests.
