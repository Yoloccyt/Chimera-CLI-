#!/usr/bin/env python3
"""重跑 Spec T1 门 ③：逐包 `cargo fmt -p <pkg> -- --check`（本机 `--all` 报 os error 206）。

为什么现在重跑：我在本波后段改过 3 个 chimera-tui 源文件的注释，也新增/移动过文件；
"44/44 全绿"是当时时点的结论，不能凭它声称现在仍成立。
退出码：0 全绿 / 1 有包不干净 / 2 不可判定（无法枚举包）。
"""
import json
import os
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

meta = subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps"],
                      capture_output=True, text=True, encoding="utf-8", errors="replace")
if meta.returncode != 0:
    print("[UNKNOWN] cargo metadata 失败：", (meta.stderr or "")[:200])
    sys.exit(2)

pkgs = sorted({p["name"] for p in json.loads(meta.stdout)["packages"]})
print(f"workspace 包数 = {len(pkgs)}")

bad, unknown = [], []
for i, p in enumerate(pkgs, 1):
    r = subprocess.run(["cargo", "fmt", "-p", p, "--", "--check"],
                       capture_output=True, text=True, encoding="utf-8", errors="replace")
    out = ((r.stdout or "") + (r.stderr or "")).strip()
    if r.returncode == 0:
        continue
    if "no such command" in out or "cannot find" in out.lower():
        unknown.append((p, out.splitlines()[:1]))
    else:
        bad.append((p, len(out.splitlines())))
    print(f"  [{i}/{len(pkgs)}] {p}: rc={r.returncode}")

print(f"\n不干净包数 = {len(bad)}；不可判定 = {len(unknown)}")
for p, n in bad:
    print(f"    {p}: diff {n} 行")
for p, o in unknown:
    print(f"    [UNKNOWN] {p}: {o}")

if unknown:
    print("[UNKNOWN] 有包未能判定，不视为通过")
    sys.exit(2)
sys.exit(1 if bad else 0)
