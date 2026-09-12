#!/usr/bin/env python3
"""从 cargo audit --json 提取"受影响版本 / 官方修补版本"，用于确定升级目标。

不做主观判断：patched 区间决定能否用 semver 兼容的小版本修掉，
还是需要跳大版本（那就不是 `cargo update` 能解决的事）。
"""
import json
from pathlib import Path

d = json.loads(Path("tmp/audit.json").read_text(encoding="utf-8"))
vulns = d.get("vulnerabilities", {}).get("list", [])
warns = d.get("warnings", {}).get("list", [])

for kind, items in (("vulnerability", vulns), ("warning", warns)):
    for v in items:
        pkg = v.get("package", {})
        adv = v.get("advisory", {})
        vers = v.get("versions", {})
        print(f"[{kind}] {pkg.get('name')} {pkg.get('version')}"
              f"  id={adv.get('id', '-')}  ref={adv.get('reference', '')}")
        print(f"    title: {adv.get('title', '')}")
        print(f"    patched={vers.get('patched')}  unaffected={vers.get('unaffected')}"
              f"  kind={v.get('kind')}")
print(f"\ntotals: vulnerabilities={len(vulns)} warnings={len(warns)}")
