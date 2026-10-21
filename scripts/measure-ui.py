"""Launch the owned GUI smoke mode; sample its RSS. No synthetic UI input.

uv run --python '>=3.14' scripts/measure-ui.py
"""
import hashlib
import json
import platform
import argparse
import os
from datetime import datetime, timezone
from pathlib import Path
import subprocess
import tempfile
import time

root = Path(__file__).resolve().parents[1]
arguments = argparse.ArgumentParser()
arguments.add_argument('--output', default='evidence/ui-smoke-current.json')
args = arguments.parse_args()
binary = root / "target/release/dbide-ui-spike"
binary_hash = hashlib.sha256(binary.read_bytes()).hexdigest()
# Each measurement uses an empty task-owned store; never reads real profiles.
profile_directory = tempfile.TemporaryDirectory(prefix="dbide-ui-smoke-")
environment = dict(os.environ, DBIDE_PROFILE_STORE=str(Path(profile_directory.name) / "profiles.json"))
samples = []
for iteration in range(5):
    started = time.perf_counter()
    process = subprocess.Popen([str(binary), "--smoke-seconds=3"], cwd=root, env=environment,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    rss = []
    deadline = started + 30
    while process.poll() is None:
        if time.perf_counter() > deadline:
            process.kill()
            process.communicate()
            raise RuntimeError("owned UI smoke process exceeded 30 seconds")
        reading = subprocess.run(["ps", "-p", str(process.pid), "-o", "rss="],
                                 capture_output=True, text=True, check=False)
        if reading.stdout.strip().isdigit():
            rss.append(int(reading.stdout.strip()) * 1024)
        time.sleep(0.1)
    stdout, stderr = process.communicate()
    metrics = [json.loads(line) for line in stdout.splitlines() if line.startswith('{')]
    if process.returncode or not metrics:
        raise RuntimeError(f"UI smoke failed: {process.returncode}: {stderr[-2000:]}")
    samples.append({"iteration": iteration, "wall_ms_including_smoke_wait": (time.perf_counter()-started)*1000,
                    "peak_sampled_rss_bytes": max(rss, default=0), "rss_samples_bytes": rss,
                    "metrics": metrics[-1]})

if hashlib.sha256(binary.read_bytes()).hexdigest() != binary_hash:
    raise RuntimeError("UI binary changed during measurement; discard mixed-build samples")
profile_directory.cleanup()
report = {"schema_version": 2, "kind": "native_ui_smoke", "host": platform.platform(),
          "generated_at": datetime.now(timezone.utc).isoformat(), "build_profile": "release",
          "rustc": subprocess.check_output(['rustc', '--version'], cwd=root, text=True).strip(),
          "cargo_lock_sha256": hashlib.sha256((root/'Cargo.lock').read_bytes()).hexdigest(),
          "binary_sha256": binary_hash,
          "workspace_source_sha256_at_measurement": hashlib.sha256((root/'crates/dbide-ui-spike/src/main.rs').read_bytes()).hexdigest(),
          "samples": samples,
          "limitations": ["Five process restarts, OS/Metal caches not cleared; no cold-start guarantee.",
             "Root render instrumentation is not first paint, input latency, or frame latency.",
             "RSS samples cover only this process and can miss transient peaks/GPU allocation.",
             "Empty isolated profile store; no DB connected. No DataGrip comparative speedup claim.",
             "Workspace source/lock hashes describe measurement-time files, not proof of the binary's build inputs."]}
(root/args.output).write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n')
print(json.dumps({"runs":len(samples), "first_root_render_ms":[s['metrics']['first_root_render_ms'] for s in samples],
                  "peak_sampled_rss_mib":[round(s['peak_sampled_rss_bytes']/1048576, 1) for s in samples]}))
