#!/usr/bin/env python3
"""重跑 Spec T1 门 ③：逐包 `cargo fmt -p <pkg> -- --check`（本机 `--all` 报 os error 206）。

覆盖面 = 根 workspace 全部包 + 仓库内所有**隔离 workspace**（`[workspace]` 自立的目录，
如 `fuzz/`）——后者 `-p` 与 `--all` 都够不到，2026-09-24 前一直是零守门面。

为什么现在重跑：我在本波后段改过 3 个 chimera-tui 源文件的注释，也新增/移动过文件；
"44/44 全绿"是当时时点的结论，不能凭它声称现在仍成立。
退出码：0 全绿 / 1 有包不干净 / 2 不可判定（无法枚举包）。
"""
import json
import os
import re
import subprocess
import sys

import gate_rc  # F37：平铺脚本无 __main__ 可包 => 崩溃经 excepthook 退 2, 不借默认 1
gate_rc.install()

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

# 隔离 workspace:根 manifest 的 `--all` 与 `-p <pkg>` 都到不了它们——`fuzz/` 自成一个
# workspace,所以这一面曾长期零守门(HEAD 上它有 5 个 target 不 fmt-clean 而本门照样全绿)。
# 用「发现」而非硬编码清单:日后新增隔离 workspace 会被自动纳管,不会静默漏接。
isolated = []
for d in sorted(os.listdir(".")):
    manifest = os.path.join(d, "Cargo.toml")
    if d.startswith(".") or not os.path.isdir(d) or not os.path.isfile(manifest):
        continue
    with open(manifest, encoding="utf-8", errors="replace") as fh:
        if re.search(r"^\[workspace\]", fh.read(), re.M):
            isolated.append(manifest)
print(f"隔离 workspace 数 = {len(isolated)}: " + (", ".join(isolated) or "(无)"))

for m in isolated:
    r = subprocess.run(["cargo", "fmt", "--manifest-path", m, "--", "--check"],
                       capture_output=True, text=True, encoding="utf-8", errors="replace")
    out = ((r.stdout or "") + (r.stderr or "")).strip()
    if r.returncode == 0:
        continue
    if "no such command" in out or "cannot find" in out.lower():
        unknown.append((m + " [isolated]", out.splitlines()[:1]))
    else:
        bad.append((m + " [isolated]", len(out.splitlines())))
    print(f"  [iso] {m}: rc={r.returncode}")

print(f"\n不干净包数 = {len(bad)}；不可判定 = {len(unknown)}")
for p, n in bad:
    print(f"    {p}: diff {n} 行")
for p, o in unknown:
    print(f"    [UNKNOWN] {p}: {o}")

if unknown:
    print("[UNKNOWN] 有包未能判定，不视为通过")
    sys.exit(2)
sys.exit(1 if bad else 0)
