"""Read package metadata only; never execute DataGrip or inspect user profiles.

Run: uv run --python '>=3.14' scripts/inventory_datagrip.py APP_PATH OUTPUT_JSON
"""

import hashlib
import json
from collections import Counter
from pathlib import Path
import re
import sys
import zipfile
import xml.etree.ElementTree as ET


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


app = Path(sys.argv[1]).resolve()
out = Path(sys.argv[2])
contents = app / "Contents"
product_path = contents / "Resources/product-info.json"
product = json.loads(product_path.read_text())
relevant = re.compile(r"database|datagrip|datagrid|^intellij\.sql")
modules = [m for m in product["layout"] if relevant.search(m["name"])]
selected_paths = sorted({p for m in modules for p in m.get("classPath", [])})
concepts = {
    "session_transaction": r"Session|Transaction|Commit|Rollback",
    "execution_cancel": r"Cancel|QueryRunner|StatementExecutor|DataProducer",
    "introspection": r"Introspect|Metadata|SchemaCache",
    "editing_grid": r"Grid|Mutation|RowIdentity",
    "semantic_refactor": r"Resolve|Completion|Refactor|Rename",
    "schema_diff": r"Migration|SchemaDiff|DdlGenerator",
}
archives = []
for relative in selected_paths:
    path = contents / relative
    if not path.is_file() or path.suffix != ".jar":
        continue
    item = {"path": relative, "bytes": path.stat().st_size, "sha256": digest(path)}
    with zipfile.ZipFile(path) as archive:
        names = archive.namelist()
        classes = [n for n in names if n.endswith(".class")]
        item["entries"] = len(names)
        item["classes"] = len(classes)
        item["class_packages"] = dict(Counter(n.rsplit("/", 1)[0] for n in classes).most_common(12))
        item["concept_candidates"] = {}
        for key, pattern in concepts.items():
            matched = [n for n in classes if re.search(pattern, n.rsplit("/", 1)[-1])]
            if matched:
                item["concept_candidates"][key] = {"count": len(matched), "sample": matched[:8]}
        descriptors = []
        for name in names:
            if not (name.startswith("META-INF/") and name.endswith(".xml")):
                continue
            if archive.getinfo(name).file_size > 2_000_000:
                continue
            try:
                root = ET.fromstring(archive.read(name))
            except ET.ParseError:
                continue
            if root.tag not in ("idea-plugin", "module"):
                continue
            descriptors.append({
                "path": name, "root": root.tag,
                "id": root.findtext("id"), "name": root.findtext("name"),
                "depends": [e.text for e in root.findall("depends")],
                "extensions": len(root.findall(".//extensions/*")),
                "actions": len(root.findall(".//action")),
                "module_names": [e.get("name") for e in root.findall(".//content/module")],
            })
        item["descriptors"] = descriptors
    archives.append(item)

all_jars = list(contents.rglob("*.jar"))
result = {
    "evidence_type": "ordinary-shell-python-static-inventory; not REA Evidence",
    "version": product["version"], "build": product["buildNumber"],
    "product_info_sha256": digest(product_path),
    "all_jar_count": len(all_jars), "all_jar_bytes": sum(p.stat().st_size for p in all_jars),
    "selection": relevant.pattern, "selected_layout_entries": len(modules),
    "selected_jar_count": len(archives), "selected_class_entries": sum(a["classes"] for a in archives),
    "modules": modules, "archives": archives,
    "limitations": [
        "Class names and descriptor declarations are candidate boundaries, not executed behavior.",
        "No Java bytecode bodies were decompiled; no runtime or performance measurements.",
        "Selection is name-based, not complete transitive dependency analysis.",
        "Class count counts archive entries, not unique logical classes or features.",
    ],
}
out.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
print(json.dumps({k: v for k, v in result.items() if k not in ("archives", "modules")}, ensure_ascii=False))
