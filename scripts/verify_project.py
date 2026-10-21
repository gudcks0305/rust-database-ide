"""Verify planning links, coverage IDs, and local static-evidence integrity."""

import hashlib
import json
from pathlib import Path
import re

root = Path(__file__).resolve().parents[1]
errors = []
markdown = [*sorted(root.glob("*.md")), *sorted((root / "docs").glob("*.md"))]
for path in markdown:
    content = path.read_text()
    for destination in re.findall(r"\]\(([^)]+)\)", content):
        if "://" in destination or destination.startswith("#"):
            continue
        target = destination.split("#", 1)[0]
        if target and not (path.parent / target).exists():
            errors.append(f"Broken link: {path.name}: {target}")
    for number, line in enumerate(content.splitlines(), 1):
        if line.rstrip() != line:
            errors.append(f"Trailing whitespace: {path.name}:{number}")

matrix = (root / "docs/FEATURE-MATRIX.md").read_text()
ids = re.findall(r"^\| (F\d+) \|", matrix, re.M)
expected = [f"F{i:02}" for i in range(1, 43)]
if ids != expected:
    errors.append("Feature IDs must be unique and ordered F01-F42")

index = json.loads((root / "evidence/index.json").read_text())
if len(index["entries"]) != 5:
    errors.append("Expected five recorded REA operations")
verified = 0
missing_raw = []
for entry in index["entries"]:
    path = root / entry["file"]
    if not path.exists():
        missing_raw.append(entry["file"])
        continue
    data = json.loads(path.read_text())
    if hashlib.sha256(path.read_bytes()).hexdigest() != entry["file_sha256"]:
        errors.append(f"Evidence file hash mismatch: {path.name}")
    if data["evidence_id"] != entry["evidence_id"] or data["operation"] != entry["operation"]:
        errors.append(f"Evidence identity mismatch: {path.name}")
    verified += 1

inventory = json.loads((root / "evidence/package-inventory.json").read_text())
archives = inventory["archives"]
if inventory["selected_jar_count"] != len(archives):
    errors.append("Archive count mismatch")
if inventory["selected_class_entries"] != sum(a["classes"] for a in archives):
    errors.append("Class-entry count mismatch")
facts = (root / "docs/REA-FINDINGS.md").read_text()
for entry in index["entries"]:
    if entry["evidence_id"] not in facts:
        errors.append(f"Missing Evidence ID in report: {entry['evidence_id']}")

print(json.dumps({"markdown_files": len(markdown), "feature_groups": len(ids),
                  "verified_raw_evidence_files": verified, "missing_local_raw": missing_raw,
                  "selected_jars": len(archives), "class_entries": inventory["selected_class_entries"],
                  "errors": errors}, ensure_ascii=False, indent=2))
if missing_raw:
    print("Local-only raw evidence absent; metadata checks passed but raw integrity was not verified.")
raise SystemExit(1 if errors else 0)
