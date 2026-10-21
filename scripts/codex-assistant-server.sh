#!/usr/bin/env bash
# Owned loopback app-server only. Does not modify CODEX_HOME, global config, or auth.
set -euo pipefail
TASK_REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
exec uv run --offline --python '>=3.14' python - "${1:-status}" "$TASK_REPO_ROOT" <<'PY'
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import time
import tomllib
from urllib.parse import urlparse

ACTION = sys.argv[1]
REPO = Path(sys.argv[2])
STATE = REPO / "tmp" / "codex-ws"
TOKEN = STATE / "token"
WORKSPACE = STATE / "workspace"
PIDFILE = STATE / "assistant-server.json"
ENDPOINT = "ws://127.0.0.1:45457"
PORT = 45457
CODEX = shutil.which("codex")
MAX_CONFIG_BYTES = 4 * 1024 * 1024
DISABLED = (
    "shell_tool", "unified_exec", "apps", "browser_use", "browser_use_external",
    "computer_use", "image_generation", "view_image", "multi_agent", "multi_agent_v2",
    "code_mode", "code_mode_host", "skill_search", "tool_suggest", "hooks",
    "codex_hooks", "plugin_hooks", "plugins", "connectors", "enable_mcp_apps",
    "in_app_browser", "js_repl", "memory_tool", "memories", "shell_snapshot",
    "shell_snapshot_v2", "search_tool", "standalone_web_search", "request_permissions",
    "request_permissions_tool", "request_rule", "send_async_message",
    "send_message_to_user_async", "daemon_auto_start", "remote_control",
)
HOOK_EVENTS = (
    "Interrupt", "PermissionRequest", "PostCompact", "PostToolUse", "PreCompact",
    "PreToolUse", "SessionEnd", "SessionStart", "Stop", "SubagentStart",
    "SubagentStop", "UserPromptSubmit",
)


def fail(message):
    print(message, file=sys.stderr)
    raise SystemExit(1)


def ps_field(pid, field):
    result = subprocess.run(["ps", "-p", str(pid), "-o", field + "="],
                            capture_output=True, text=True, check=False)
    return result.stdout.strip() if result.returncode == 0 else ""


def load_state():
    if not PIDFILE.exists():
        return None
    if PIDFILE.is_symlink() or PIDFILE.stat().st_size > 65536:
        fail("Refusing unexpected launcher state file")
    try:
        state = json.loads(PIDFILE.read_text())
        if not isinstance(state.get("pid"), int) or state["pid"] <= 1:
            fail("Refusing invalid launcher PID state")
        return state
    except (ValueError, OSError):
        fail("Cannot read owned launcher state")


def owned_running(state):
    if not state:
        return False
    command = ps_field(state["pid"], "command")
    stamp = ps_field(state["pid"], "lstart")
    if not command:
        return False
    if (stamp != state.get("process_start") or command != state.get("process_command")
            or "app-server" not in command or ENDPOINT not in command
            or str(TOKEN) not in command):
        fail("PID belongs to a different process; refusing to control it")
    return True


def port_open():
    try:
        with socket.create_connection(("127.0.0.1", PORT), timeout=0.15):
            return True
    except OSError:
        return False


def private_directory(path):
    if path.is_symlink():
        fail("Refusing symlinked launcher directory")
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    if not path.is_dir() or path.stat().st_uid != os.getuid():
        fail("Launcher directory is not owned by the current user")
    path.chmod(0o700)


def atomic_private_json(path, value):
    temporary = path.with_suffix(".new")
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(value, stream)
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def metadata_overrides():
    # Parse TOML locally, emitting only configured IDs as boolean overrides.
    # auth.json and credential stores are never read or copied here.
    codex_home = Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex")))
    sources = {codex_home / "config.toml"}
    for ancestor in (WORKSPACE, *WORKSPACE.parents):
        sources.add(ancestor / ".codex" / "config.toml")
    mcp_ids, plugin_ids, hook_ids = set(), set(), set()

    def visit(value):
        if not isinstance(value, dict):
            return
        for key in ("model_instructions_file", "experimental_compact_prompt_file", "model_catalog_json"):
            if value.get(key):
                fail(f"Configured {key} may load unreviewed content; refusing owned assistant startup")
        if value.get("chatgpt_base_url"):
            url = urlparse(value["chatgpt_base_url"])
            if (url.scheme != "https" or url.hostname != "chatgpt.com"
                    or url.username or url.password or url.query or url.fragment
                    or url.port not in (None, 443)):
                fail("Nonofficial ChatGPT base URL configured; refusing assistant startup")
        providers = value.get("model_providers", {})
        if isinstance(providers, dict) and isinstance(providers.get("openai"), dict):
            if providers["openai"].get("base_url"):
                fail("Custom built-in OpenAI provider base URL configured; refusing assistant startup")
        for key, child in value.items():
            if key == "mcp_servers" and isinstance(child, dict):
                mcp_ids.update(child)
            elif key == "plugins" and isinstance(child, dict):
                plugin_ids.update(child)
            elif key == "hooks" and isinstance(child, dict):
                if isinstance(child.get("state"), dict):
                    hook_ids.update(child["state"])
            if isinstance(child, dict):
                visit(child)

    for source in sources:
        if source.exists():
            if not source.is_file() or source.stat().st_size > MAX_CONFIG_BYTES:
                fail("A configured TOML source is not a bounded regular file")
            try:
                visit(tomllib.loads(source.read_text()))
            except (OSError, ValueError):
                fail("Cannot parse configured TOML; refusing partial tool restrictions")
    args = []
    for table, ids in (("mcp_servers", mcp_ids), ("plugins", plugin_ids)):
        for identifier in sorted(ids):
            if "." in identifier or "=" in identifier or any(ord(c) < 32 for c in identifier):
                fail("Configured ID cannot be safely represented by this Codex CLI's dotted override parser")
            # CLI 0.160.1 splits dotted paths literally; TOML quotes become part
            # of the key and create an invalid extra MCP entry. Args bypass shell parsing.
            args.extend(["-c", f"{table}.{identifier}.enabled=false"])
    # Hook state IDs may contain dots/paths and cannot safely be represented by
    # this CLI parser. Disable the hook engines and clear every inline event;
    # never rewrite user hook-state entries or execute them to inspect them.
    return args, len(mcp_ids), len(plugin_ids), len(hook_ids)


def launch_args():
    if not CODEX:
        fail("codex executable not found")
    result = subprocess.run([CODEX, "app-server", "--help"], capture_output=True,
                            text=True, check=False)
    if result.returncode or "--ws-token-file" not in result.stdout or "--ws-auth" not in result.stdout:
        fail("Installed Codex does not support the required authenticated WebSocket flags")
    overrides, mcp_count, plugin_count, hook_count = metadata_overrides()
    args = [CODEX, "app-server", "--listen", ENDPOINT,
            "--ws-auth", "capability-token", "--ws-token-file", str(TOKEN)]
    for feature in DISABLED:
        args.extend(["-c", f"features.{feature}=false"])
    for config in (
        'approval_policy="never"', 'sandbox_mode="read-only"',
        'web_search="disabled"', 'memories.use_memories=false',
        'memories.generate_memories=false', 'project_doc_max_bytes=0',
        'features.skip_host_skill_discovery=true', 'notify=[]',
        'developer_instructions=""', 'history.persistence="none"',
        'instructions=""', 'compact_prompt=""', 'skills.include_instructions=false',
        'model_provider="openai"', 'forced_login_method="chatgpt"',
        'analytics.enabled=false', 'otel.log_user_prompt=false',
        'otel.log_agent_responses=false', 'otel.log_guardian_assessments=false',
        'otel.exporter="none"', 'otel.trace_exporter="none"',
        'otel.metrics_exporter="none"', 'include_apps_instructions=false',
    ):
        args.extend(["-c", config])
    for event in HOOK_EVENTS:
        args.extend(["-c", f"hooks.{event}=[]"])
    args.extend(overrides)
    return args, mcp_count, plugin_count, hook_count


def status(state):
    if not owned_running(state):
        print("Codex SQL assistant server: stopped")
        return 1
    print(f"Codex SQL assistant server: running (PID {state['pid']})")
    print(f"Endpoint: {ENDPOINT}")
    print(f"Capability file: {TOKEN} (value hidden)")
    print(f"Scratch workspace: {WORKSPACE}")
    print(f"Disabled configured entries: MCP={state['mcp_count']}, plugins={state['plugin_count']}; configured hook states={state['hook_count']} (hook engines disabled)")
    return 0


if ACTION not in {"start", "status", "stop", "check"}:
    fail("Usage: scripts/codex-assistant-server.sh start|status|stop|check")
if ACTION == "status":
    raise SystemExit(status(load_state()))
if ACTION == "check":
    _, mcp_count, plugin_count, hook_count = launch_args()
    print("Authenticated WebSocket flags supported; local restriction arguments prepared")
    print(f"Configured entry counts: MCP={mcp_count}, plugins={plugin_count}, hook states={hook_count}")
    print("No server, model turn, login, or global config change performed")
    raise SystemExit(0)

private_directory(STATE)
lock = STATE / "assistant-launch.lock"
try:
    lock.mkdir(mode=0o700)
except FileExistsError:
    fail("Another launcher operation is active; inspect the owned lock before retrying")
pending_process = None
try:
    state = load_state()
    if ACTION == "stop":
        if not owned_running(state):
            print("Codex SQL assistant server: already stopped")
            raise SystemExit(0)
        os.kill(state["pid"], signal.SIGTERM)
        for _ in range(40):
            if not ps_field(state["pid"], "command"):
                break
            time.sleep(0.05)
        else:
            # Never escalate or kill a reused PID automatically.
            fail("Owned server did not exit after SIGTERM; inspect before retrying")
        PIDFILE.unlink(missing_ok=True)
        print("Owned Codex SQL assistant server stopped")
        raise SystemExit(0)
    if owned_running(state):
        raise SystemExit(status(state))
    if port_open():
        fail("Loopback port 45457 is already in use; refusing to replace its listener")
    args, mcp_count, plugin_count, hook_count = launch_args()
    private_directory(WORKSPACE)
    if any(WORKSPACE.iterdir()):
        fail("Scratch workspace is not empty; refusing to remove or reuse its contents")
    # Rotate only the owned capability file. Never touch account/auth credentials.
    if TOKEN.is_symlink() or (TOKEN.exists() and not TOKEN.is_file()):
        fail("Refusing unexpected capability-file type")
    token_temporary = TOKEN.with_suffix(".new")
    token_fd = os.open(token_temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(token_fd, "w") as stream:
            stream.write(secrets.token_hex(32))
        os.replace(token_temporary, TOKEN)
    finally:
        token_temporary.unlink(missing_ok=True)
    # No raw app-server output is captured or printed; it may contain sensitive data.
    child_env = dict(os.environ)
    for key in ("OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_API_KEY", "CODEX_API_BASE"):
        child_env.pop(key, None)
    process = subprocess.Popen(args, cwd=WORKSPACE, env=child_env, stdin=subprocess.DEVNULL,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                               start_new_session=True, close_fds=True)
    pending_process = process
    for _ in range(100):
        if process.poll() is not None:
            fail("Owned app-server exited during startup; check CLI/config compatibility (raw output suppressed)")
        if port_open():
            break
        time.sleep(0.05)
    else:
        process.terminate()
        process.wait(timeout=3)
        fail("Owned app-server did not listen within five seconds")
    state = {"pid": process.pid, "process_start": ps_field(process.pid, "lstart"),
             "process_command": ps_field(process.pid, "command"),
             "endpoint": ENDPOINT, "mcp_count": mcp_count,
             "plugin_count": plugin_count, "hook_count": hook_count}
    if not state["process_start"] or str(TOKEN) not in state["process_command"]:
        process.terminate()
        fail("Cannot establish owned process identity")
    try:
        atomic_private_json(PIDFILE, state)
    except OSError:
        process.terminate()
        process.wait(timeout=3)
        fail("Cannot persist owned process identity; server stopped")
    pending_process = None  # Persisted scoped PID state owns it now.
    status(state)
finally:
    if pending_process is not None and pending_process.poll() is None:
        pending_process.terminate()
        try:
            pending_process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            print("New owned server did not terminate; inspect its exact PID before retrying", file=sys.stderr)
    lock.rmdir()
PY
