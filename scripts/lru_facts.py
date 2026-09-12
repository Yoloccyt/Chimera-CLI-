#!/usr/bin/env python3
"""从 cargo metadata 取 lru 的版本约束与可用升级空间（为 ADR-167 的 A/B 裁决供事实）。"""
import io
import json
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

meta = subprocess.run(["cargo", "metadata", "--format-version", "1",
                       "--filter-platform", "x86_64-pc-windows-gnu"],
                      capture_output=True, text=True, encoding="utf-8", errors="replace")
if meta.returncode != 0:
    print("cargo metadata 失败:", (meta.stderr or "")[:300])
    sys.exit(1)
d = json.loads(meta.stdout)

print("=== 包与约束 ===")
for p in d["packages"]:
    if p["name"] in ("ratatui", "lru", "crossterm", "compact_str"):
        lru_req = [(x["name"], x["req"], x.get("optional"), x.get("kind"))
                   for x in p["dependencies"] if x["name"] == "lru"]
        print(f"  {p['name']:12} v{p['version']:8} 依赖 lru: {lru_req or '—'}")

print("\n=== 解析后的锁里 lru 出现在哪些节点 ===")
for pkg in d["packages"]:
    for dep in pkg["dependencies"]:
        if dep["name"] == "lru":
            print(f"  {pkg['name']} {pkg['version']} -> lru {dep['req']} (optional={dep.get('optional')}, kind={dep.get('kind')})")

# workspace 里是否有直接依赖
print("\n=== 本仓库是否直接依赖 lru ===")
ws = {p["name"] for p in d["packages"] if str(p["manifest_path"]).startswith(str(d["workspace_root"]))}
direct = [n for n in ws for p in d["packages"] if p["name"] == n
          for dep in p["dependencies"] if dep["name"] == "lru"]
print("  直接依赖 lru 的本地 crate:", sorted(set(direct)) or "无（纯传递依赖）")
