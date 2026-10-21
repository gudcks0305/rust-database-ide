# Native Codex assistance panel

`dbide-assistant-ui` is a GPUI component using the opt-in `dbide-assistant`
WebSocket client. It offers Generate SQL, Explain and Optimize with explicit
context review, a separate metadata probe, cancellation, transient streaming
text, final response text, copying, and an explicit SQL-draft event. The panel
contains no SQL execution or server-launch action and needs no OpenAI API key.
Protocol/authentication/isolation limits remain those in
[AI-ASSISTANT.md](AI-ASSISTANT.md).

## Host integration

The host initializes GPUI and `gpui_component`, then mounts:

```rust,ignore
let panel = cx.new(|cx| {
    AssistantPanel::new(
        captured_profile,   // Option<ConnectionProfile>, never a password/request
        captured_sql,       // String from explicit parent editor snapshot
        captured_schema,    // Option<String>, supplied snapshot only; no live lookup
        window,
        cx,
    )
});
```

`AssistantEvent::OpenSqlDraft { profile, sql }` is emitted only after the user
clicks **Open SQL draft for review** on a completed structured suggestion. The
host must create a dirty editor/console using that captured optional profile;
it must not run the SQL or substitute a newly selected connection. Schema and
SQL snapshots remain fixed for the panel's lifetime. Reopen to capture different
context. The panel never obtains credentials, runs a catalog query, reads query
history, follows profile selection changes, or reads user databases.

The host may call `panel.configure(config, window, cx)` with the re-exported
`AssistantConfig` to prefill an explicitly selected owned listener. Configuration
always forces disabled and cancels any active operation. Configuration and
panel construction do not open a socket or read the capability-token file.

## Explicit user flow

1. Enter the owned numeric loopback WebSocket endpoint, absolute capability-token
   **file path**, absolute isolated scratch directory, and optional model id.
   The initial endpoint is `ws://127.0.0.1:45457`; token/cwd/model start blank.
   Capability-token contents are never accepted as a credential field or shown
   in previews, errors or progress. Backend errors are sanitized. There is no
   OpenAI API key field.
2. Click **Enable Codex**. This changes local state only. Optional **Probe auth +
   model catalog** explicitly opens the owned WebSocket for sanitized account,
   available model ids and effective isolation metadata. It starts no model turn
   and sends no instruction, SQL or schema.
3. Choose Generate SQL, Explain or Optimize and enter an instruction. **Include
   SQL** and **Include schema** both start off, including when the host supplied
   context. Explain/Optimize require Include SQL. No implicit fallback includes
   excluded content.
4. Click **Preview selected payload**. This validates locally and displays the
   exact selected instruction/SQL/schema JSON, destination origin, individual
   byte counts, total serialized request bytes and ephemeral-thread indication.
   No socket/file read/model call occurs. Any input, task, inclusion flag or
   listener edit invalidates this reviewed request.
5. After reviewing, click **Send reviewed snapshot**. A dedicated background
   Tokio worker sends exactly the immutable snapshot. The model may use ChatGPT
   quota. No automatic resend or fallback model call is performed. Live deltas
   are untrusted transient text; the final completed response is authoritative.
6. **Copy response text** copies the completed raw response. **Open SQL draft for
   review** requires the structured `Suggestion.sql` field to contain at least
   one syntactically complete statement for the captured dialect (generic SQL
   when no profile was captured). Prose, Markdown fences and comment-only text
   are not promoted to SQL. Parser success is not proof of safety, correctness,
   permissions, compatibility, or performance. DDL/DML still require independent
   user review and a separate existing execution action.

**Cancel request**, Disable, reconfiguration, or dropping the panel cancels its
own token. A generation identifier prevents stale deltas/final responses from
publishing after cancellation or a newer operation. Server interruption is best
effort as documented by the backend. Cancel does not automatically resend.

The panel presents this remediation command when a listener is unavailable:

```sh
# Run explicitly from the repository root, outside the panel:
bash scripts/codex-assistant-server.sh start
```

The launcher reports the endpoint and its protected token-file/scratch paths.
The panel never runs this command, discovers existing desktop ports, starts a
server or modifies login/global configuration.

## Limits and checks

| Resource | Limit |
| --- | --- |
| Instruction | 64 KiB UTF-8; oversized edits rejected, never silently sent truncated |
| Captured SQL/schema | 256 KiB each; oversized initial snapshots omitted with status notice |
| Endpoint/token path/cwd input | 4,096 bytes each; no control characters |
| Optional model id | 256 bytes; backend validates content |
| Outbound request | Backend 1 MiB cap, checked during preview |
| Stream display and final assistant text | 1 MiB; UTF-8 boundaries preserved |
| Progress channel | 8 pending events; excess progress chunks may be omitted |
| Request timeout | 120 seconds, with backend cancellation/retirement behavior |
| SQL draft parse | 256 KiB, performed on worker rather than UI thread |

Final events are awaited rather than dropped when the progress channel fills.
Only local UI state is modified before explicit Probe/Send. Native rendering,
IME and accessibility need the host's normal GUI QA; pure-model tests do not
replace native interaction tests.

```sh
cargo test -p dbide-assistant-ui --lib
cargo clippy -p dbide-assistant-ui --all-targets -- -D warnings
cargo fmt -p dbide-assistant-ui -- --check
```

Tests use no live WebSocket, inference turn, user token or database. They cover
excluded-context non-disclosure, explicit inclusion/task requirements, offline
preview validation/limits, structured SQL parsing, stale generations, bounded
UTF-8 streaming, and cancellation before any connection/token-file access.
