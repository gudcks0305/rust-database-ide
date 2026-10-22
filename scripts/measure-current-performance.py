"""Current release GUI/CLI measurements over generated, owned local fixtures.
Run with uv run --python '>=3.14' scripts/measure-current-performance.py.
No synthetic UI input; callback timings are explicitly not paint/input latency.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import statistics
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument(
    "--binary",
    type=Path,
    default=ROOT / "outputs/Rust Database IDE.app/Contents/MacOS/dbide-ui-spike",
)
parser.add_argument("--cli", type=Path, default=ROOT / "target/release/dbide")
parser.add_argument(
    "--output", type=Path, default=ROOT / "evidence/performance-2026-10-22-runtime.json"
)
parser.add_argument("--starts", type=int, default=10)
parser.add_argument("--idle-seconds", type=int, default=65)
args = parser.parse_args()
if not 1 <= args.starts <= 30 or not 10 <= args.idle_seconds <= 120:
    parser.error("starts1..30 and idle-seconds10..120 required")


def command(*argv):
    return subprocess.check_output(argv, cwd=ROOT, text=True).strip()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def summary(values):
    ordered = sorted(values)
    return {
        "count": len(values),
        "median": statistics.median(values),
        "p95_nearest_rank": ordered[math.ceil(0.95 * len(ordered)) - 1],
        "min": ordered[0],
        "max": ordered[-1],
    }


def cpu_seconds(text):
    fields = text.split(":")
    result = 0.0
    for field in fields:
        result = result * 60 + float(field)
    return result


report = {
    "schema_version": 1,
    "observed_at": datetime.now(timezone.utc).isoformat(),
    "commit": command("git", "rev-parse", "HEAD"),
    "build": "release packaged UI / release CLI",
    "ui_binary_sha256": digest(args.binary),
    "cli_binary_sha256": digest(args.cli),
    "cargo_lock_sha256": digest(ROOT / "Cargo.lock"),
    "host": {
        "os": platform.platform(),
        "model": command("sysctl", "-n", "hw.model"),
        "cpu": command("sysctl", "-n", "machdep.cpu.brand_string"),
        "logical_cpus": int(command("sysctl", "-n", "hw.logicalcpu")),
        "memory_bytes": int(command("sysctl", "-n", "hw.memsize")),
        "uptime_before": command("uptime"),
        "power": command("pmset", "-g", "batt"),
    },
    "startup": {},
    "idle": {},
    "queries": {},
    "limitations": [
        "Process restarts with OS/Metal/file caches retained; not cold boot. Other desktop apps remain running.",
        "first_root_render_ms is main entry to first root render callback, not first paint or restored-workspace readiness.",
        "RSS/time samples cover only the UI process at approximately250ms; no GPU/WindowServer/helper allocation or transient peak guarantee.",
        "Idle CPU is sampled process CPU-time delta / sampled wall interval;100percent means one CPU core.",
        "CLI wall includes process launch/config/connect/query/decode/JSON/stdout; report elapsed is adapter-reported query time, not server-only or first-row paint.",
        "No DataGrip comparison, key-to-paint, physical IME, or scroll FPS claim.",
    ],
}


def persist():
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_suffix(".tmp")
    temporary.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    temporary.replace(args.output)


with tempfile.TemporaryDirectory(prefix="dbide-measure-current-") as temporary:
    tmp = Path(temporary).resolve()
    pinned = tmp / "dbide-ui-measured"
    shutil.copy2(args.binary, pinned)
    cli = tmp / "dbide-measured"
    shutil.copy2(args.cli, cli)
    config = tmp / "workspace-config/profiles.json"
    config.parent.mkdir()
    workspace = tmp / "generated-workspace"
    workspace.mkdir()

    def setup(*arguments):
        proc = subprocess.run(
            [str(cli), "--config", str(config), *map(str, arguments)],
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=30,
        )
        if proc.returncode:
            raise RuntimeError(proc.stderr[-2000:] + proc.stdout[-1000:])
        return proc.stdout

    setup(
        "profiles",
        "add",
        "--id",
        "benchmark-sqlite",
        "--name",
        "Generated benchmark",
        "--engine",
        "sqlite",
        "--sqlite-path",
        tmp / "fixture.sqlite",
        "--sqlite-create",
    )
    setup(
        "query",
        "--profile",
        "benchmark-sqlite",
        "--sql",
        "CREATE TABLE rows_data(id INTEGER PRIMARY KEY,name TEXT,unicode_text TEXT,amount REAL,nullable_text TEXT,payload TEXT); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000) INSERT INTO rows_data SELECT x,printf('row-%04d',x),'한글 benchmark',x*1.25,CASE WHEN x%7=0 THEN NULL ELSE 'bounded result' END,'{\"enabled\":true}' FROM n;",
        "--format",
        "json",
    )
    setup("profiles", "edit", "benchmark-sqlite", "--sqlite-create", "false")
    setup("workspace", "init", "--root", workspace, "--name", "Generated500files8tabs")
    bound = json.loads(
        setup(
            "workspace",
            "bind",
            "--root",
            workspace,
            "--profile",
            "benchmark-sqlite",
            "--expect-revision",
            "0",
        )
    )
    manifest = json.loads(setup("workspace", "show", "--root", workspace))["manifest"]
    docs = []
    for i in range(500):
        sql = "SELECT id,name FROM rows_data LIMIT 10;\n"
        if i < 8:
            sql += "-- " + ("generated SQL buffer " * 400) + "\n"
        path = workspace / f"query-{i:03}.sql"
        path.write_text(sql)
        if i < 8:
            docs.append(
                {
                    "id": i + 1,
                    "title": path.name,
                    "sql": sql,
                    "saved_sql": sql,
                    "revision": 0,
                    "path": str(path),
                    "profile_id": "benchmark-sqlite",
                    "execution_target": manifest["default_target"],
                    "project_root": str(workspace),
                }
            )
    (config.parent / "workspace.json").write_text(
        json.dumps({"schema_version": 3, "documents": docs, "active_document_id": 1})
    )
    (config.parent / "editor-preferences.json").write_text('{"enabled":true}')
    scenarios = {
        "empty": {"config": tmp / "empty/profiles.json"},
        "workspace_500_files_8_tabs": {"config": config, "workspace": workspace},
    }
    base_env = {k: v for k, v in os.environ.items() if not k.startswith("DBIDE_")}

    def ui_run(name, seconds):
        options = scenarios[name]
        env = dict(base_env, DBIDE_PROFILE_STORE=str(options["config"]))
        if "workspace" in options:
            env["DBIDE_WORKSPACE"] = str(options["workspace"])
        began = time.perf_counter()
        samples = []
        proc = subprocess.Popen(
            [str(pinned), f"--smoke-seconds={seconds}"],
            cwd=ROOT,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            while proc.poll() is None:
                elapsed = time.perf_counter() - began
                if elapsed > seconds + 30:
                    raise RuntimeError("UI process exceeded bounded deadline")
                data = subprocess.run(
                    ["ps", "-p", str(proc.pid), "-o", "rss=,time=,pcpu="],
                    capture_output=True,
                    text=True,
                ).stdout.split()
                if len(data) == 3:
                    samples.append(
                        {
                            "elapsed_s": elapsed,
                            "rss_bytes": int(data[0]) * 1024,
                            "cpu_s": cpu_seconds(data[1]),
                            "ps_percent_cpu": float(data[2]),
                        }
                    )
                time.sleep(0.25)
            stdout, stderr = proc.communicate(timeout=5)
            metrics = [
                json.loads(line) for line in stdout.splitlines() if line.startswith("{")
            ]
            if proc.returncode or not metrics:
                raise RuntimeError(f"UI failed:{proc.returncode}:{stderr[-2000:]}")
            return {
                "wall_ms_including_smoke_wait": (time.perf_counter() - began) * 1000,
                "peak_sampled_rss_bytes": max(s["rss_bytes"] for s in samples),
                "samples": samples,
                "metrics": metrics[-1],
            }
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.communicate()

    for name in scenarios:
        runs = []
        for index in range(args.starts):
            runs.append(ui_run(name, 3))
            print(f"startup {name} {index + 1}/{args.starts}", flush=True)
        report["startup"][name] = {
            "samples": runs,
            "first_root_render_ms": summary(
                [r["metrics"]["first_root_render_ms"] for r in runs]
            ),
            "peak_sampled_rss_mib": summary(
                [r["peak_sampled_rss_bytes"] / 1048576 for r in runs]
            ),
        }
        persist()
    for name in scenarios:
        print(f"idle {name} for {args.idle_seconds}s", flush=True)
        run = ui_run(name, args.idle_seconds)
        steady = [s for s in run["samples"] if s["elapsed_s"] >= 5]
        report["idle"][name] = {
            "raw": run,
            "settling_seconds": 5,
            "steady_sampled_wall_s": steady[-1]["elapsed_s"] - steady[0]["elapsed_s"],
            "steady_cpu_percent_one_core": 100
            * (steady[-1]["cpu_s"] - steady[0]["cpu_s"])
            / (steady[-1]["elapsed_s"] - steady[0]["elapsed_s"]),
            "steady_rss_mib": summary([s["rss_bytes"] / 1048576 for s in steady]),
            "rss_end_minus_start_mib": (
                steady[-1]["rss_bytes"] - steady[0]["rss_bytes"]
            )
            / 1048576,
        }
        persist()
    for rows in [1, 1000]:
        samples = []
        for iteration in range(33):
            began = time.perf_counter()
            output = setup(
                "query",
                "--profile",
                "benchmark-sqlite",
                "--sql",
                f"SELECT * FROM rows_data ORDER BY id LIMIT {rows};",
                "--read-only",
                "--format",
                "json",
            )
            elapsed = (time.perf_counter() - began) * 1000
            result = json.loads(output)
            count = sum("Row" in event for event in result["events"])
            assert (
                count == rows
                and result["outcome"] == "Completed"
                and not result["truncated"]
            )
            if iteration >= 3:
                samples.append(
                    {
                        "wall_ms": elapsed,
                        "reported_query_ms": result["elapsed_micros"] / 1000,
                        "rows": count,
                        "retained_bytes": result["retained_bytes"],
                        "json_stdout_bytes": len(output.encode()),
                    }
                )
        report["queries"][f"sqlite_{rows}_rows"] = {
            "warmups": 3,
            "samples": samples,
            "wall_ms": summary([s["wall_ms"] for s in samples]),
            "reported_query_ms": summary([s["reported_query_ms"] for s in samples]),
        }
        print(f"query sqlite {rows}rows finished", flush=True)
        persist()
    assert (
        digest(pinned) == report["ui_binary_sha256"]
        and digest(cli) == report["cli_binary_sha256"]
    )
report["host"]["uptime_after"] = command("uptime")
report["completed_at"] = datetime.now(timezone.utc).isoformat()
persist()
print(
    json.dumps(
        {
            "output": str(args.output),
            "startup": {
                k: v["first_root_render_ms"] for k, v in report["startup"].items()
            },
            "idle": {
                k: {
                    "cpu_percent": v["steady_cpu_percent_one_core"],
                    "rss": v["steady_rss_mib"],
                }
                for k, v in report["idle"].items()
            },
            "queries": {k: v["wall_ms"] for k, v in report["queries"].items()},
        },
        ensure_ascii=False,
    ),
    flush=True,
)
