#!/usr/bin/env python3
"""audit_spec_coverage 的负对照：喂一份含"不存在的任务/交付物"的 Spec，必须报盲区。

没有这一步，新检查器就只是"跑出来绿"而没人知道它对不对 —— 而它检查的正是我自己的审计器。
"""
import io
import os
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

SYN = "tmp/nc_spec_bogus.md"
io.open(SYN, "w", encoding="utf-8", newline="\n").write(
    "# 合成 Spec（负对照用）\n\n"
    "#### R1-T77 一个仓库里不存在对应判据的任务\n"
    "- 交付物：`scripts/definitely-missing-artifact-xyz.py`\n\n"
    "| RK-P90 | 一个未入册的风险 | 缓解 | 触发 | 状态 | 备注 |\n"
    "本文件刻意不包含任何真实任务，用来验证反查会报盲区。\n"
)

env = dict(os.environ, CHIMERA_SPEC_PATH=os.path.abspath(SYN), PYTHONIOENCODING="utf-8")
r = subprocess.run([sys.executable, "scripts/audit_spec_coverage.py"],
                   capture_output=True, text=True, encoding="utf-8", errors="replace", env=env)
out = (r.stdout or "") + (r.stderr or "")
print(f"rc={r.returncode}")
for l in out.splitlines():
    if "GAP" in l or l.strip().startswith("-") or "MISS" in l or "R1-T77" in l or "RK-P90" in l:
        print("   ", l.strip()[:126])

checks = []
checks.append(("退出码为 1（报出盲区）", r.returncode == 1))
checks.append(("抓到虚构任务 R1-T77", "R1-T77" in out))
checks.append(("抓到虚构交付物", "definitely-missing-artifact-xyz" in out))
checks.append(("抓到未入册风险 RK-P90", "RK-P90" in out))
os.remove(SYN)

print()
for label, ok in checks:
    print(f"  [{'PASS' if ok else 'FAIL'}] {label}")
bad = [l for l, ok in checks if not ok]
print("\n[NC] " + ("ALL PASS" if not bad else "FAIL: " + "; ".join(bad)))
sys.exit(1 if bad else 0)
