"""Fresh-process release CLI query timings on fixed project-owned loopback fixtures.
Requires --owned-loopback; never accepts an arbitrary endpoint/config/password.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import statistics
import shutil
import subprocess
import tempfile
import time

root = Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser()
p.add_argument("--owned-loopback", action="store_true", required=True)
args = p.parse_args()
cli = root / "target/release/dbide"
engines = [
    ("postgres", "dbide-m0-pg", "rust-database-ide-m0", "5432/tcp", 55439, "postgres"),
    ("mysql", "dbide-mysql", "rust-database-ide-mysql", "3306/tcp", 55441, "root"),
]
for engine, name, owner, container_port, port, user in engines:
    state = json.loads(subprocess.check_output(["docker", "inspect", name], text=True))[
        0
    ]
    assert state["Config"]["Labels"].get("owner") == owner and state["State"]["Running"]
    assert any(
        v["HostIp"] == "127.0.0.1" and v["HostPort"] == str(port)
        for v in state["NetworkSettings"]["Ports"][container_port]
    )


def summary(values):
    ordered = sorted(values)
    return {
        "median": statistics.median(values),
        "p95_nearest_rank": ordered[math.ceil(0.95 * len(ordered)) - 1],
        "min": ordered[0],
        "max": ordered[-1],
    }


report = {
    "schema_version": 1,
    "observed_at": datetime.now(timezone.utc).isoformat(),
    "commit": subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=root, text=True
    ).strip(),
    "cli_binary_sha256": hashlib.sha256(cli.read_bytes()).hexdigest(),
    "mode": "release CLI, fresh process and connection per request",
    "warmups": 3,
    "samples_per_case": 30,
    "cases": {},
    "limits": [
        "Generated SELECT data only on owner-label/loopback-port verified containers.",
        "Wall includes process/config/auth/connect/query/serialization/stdout; adapter query elapsed is not server-only time.",
        "Warm DB/OS caches; no DataGrip comparison or remote-network extrapolation.",
    ],
}
with tempfile.TemporaryDirectory(prefix="dbide-query-measure-") as temporary:
    measured_cli = Path(temporary) / "dbide-measured"
    shutil.copy2(cli, measured_cli)
    config = Path(temporary) / "profiles.json"
    env = {k: v for k, v in os.environ.items() if not k.startswith("DBIDE_")}
    env["DBIDE_BENCH_PASSWORD"] = "dbide_fixture_only"

    def invoke(*arguments):
        return subprocess.run(
            [str(measured_cli), "--config", str(config), *map(str, arguments)],
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
            timeout=35,
        )

    for engine, name, owner, container_port, port, user in engines:
        created = invoke(
            "profiles",
            "add",
            "--id",
            engine,
            "--name",
            f"Owned {engine} benchmark",
            "--engine",
            engine,
            "--host",
            "127.0.0.1",
            "--port",
            port,
            "--database",
            "dbide_fixture",
            "--username",
            user,
            "--tls",
            "disable",
        )
        assert created.returncode == 0, created.stderr
        version = invoke(
            "query",
            "--profile",
            engine,
            "--sql",
            "SELECT version();",
            "--password-env",
            "DBIDE_BENCH_PASSWORD",
            "--read-only",
            "--format",
            "json",
        )
        assert version.returncode == 0, version.stderr
        version = next(
            e["Row"]["values"][0]
            for e in json.loads(version.stdout)["events"]
            if "Row" in e
        )
        for rows in (1, 1000):
            sql = (
                f"SELECT i::bigint AS id, 'row-'||i AS name, (i*1.25)::numeric(12,2) AS amount, CASE WHEN i%7=0 THEN NULL ELSE '한글' END AS note FROM generate_series(1,{rows}) AS i;"
                if engine == "postgres"
                else f"WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<{rows}) SELECT i AS id, CONCAT('row-',i) AS name, CAST(i*1.25 AS DECIMAL(12,2)) AS amount, IF(i%7=0,NULL,'한글') AS note FROM n;"
            )
            samples = []
            for iteration in range(33):
                began = time.perf_counter()
                out = invoke(
                    "query",
                    "--profile",
                    engine,
                    "--sql",
                    sql,
                    "--password-env",
                    "DBIDE_BENCH_PASSWORD",
                    "--read-only",
                    "--format",
                    "json",
                )
                elapsed = (time.perf_counter() - began) * 1000
                assert out.returncode == 0, out.stderr
                result = json.loads(out.stdout)
                count = sum("Row" in e for e in result["events"])
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
                            "json_stdout_bytes": len(out.stdout.encode()),
                        }
                    )
            report["cases"][f"{engine}_{rows}_rows"] = {
                "server_version": version,
                "columns": 4,
                "samples": samples,
                "wall_ms": summary([s["wall_ms"] for s in samples]),
                "reported_query_ms": summary([s["reported_query_ms"] for s in samples]),
            }
            print(
                f"{engine} {rows} rows: {report['cases'][f'{engine}_{rows}_rows']['wall_ms']}",
                flush=True,
            )
    assert (
        hashlib.sha256(measured_cli.read_bytes()).hexdigest()
        == report["cli_binary_sha256"]
    )
report["completed_at"] = datetime.now(timezone.utc).isoformat()
(root / "evidence/performance-2026-10-22-network.json").write_text(
    json.dumps(report, ensure_ascii=False, indent=2) + "\n"
)
