#!/usr/bin/env python3
"""T2 批次清单的两条耦合核实（不靠断言，靠 git 事实）。

问题 1：HEAD 版的 check_perf_redlines.sh 是否已经引用 .py？
  → 若 HEAD 自包含，则"漏提交 .py"只在**提交动作之后**才致红，批次顺序就是防线。
问题 2：三个 workflow 各自引用哪些 scripts/ 路径，其中哪些尚未跟踪？
  → 未跟踪的被引用方 = 必须与之同 commit 的硬约束清单。
"""
import io
import os
import re
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")


def tracked(p):
    return subprocess.run(["git", "ls-files", "--error-unmatch", p],
                          capture_output=True).returncode == 0


print("=== 1. HEAD 版 vs 工作区版 check_perf_redlines.sh 的 .py 依赖 ===")
r = subprocess.run(["git", "show", "HEAD:scripts/check_perf_redlines.sh"], capture_output=True)
head = r.stdout.decode("utf-8", errors="replace")
work = io.open("scripts/check_perf_redlines.sh", encoding="utf-8", errors="replace").read()
print(f"  HEAD   : {len(head.splitlines())} 行, 提及 py 核 {head.count('check_perf_redlines.py')} 次")
print(f"  工作区 : {len(work.splitlines())} 行, 提及 py 核 {work.count('check_perf_redlines.py')} 次")
print(f"  py 核当前是否被跟踪: {tracked('scripts/check_perf_redlines.py')}")

print("\n=== 2. workflow -> scripts 依赖与被跟踪状态 ===")
for f in (".github/workflows/ci.yml", ".github/workflows/bench_check.yml",
          ".github/workflows/slo.yml", ".github/workflows/stress.yml"):
    if not os.path.exists(f):
        continue
    t = io.open(f, encoding="utf-8", errors="replace").read()
    refs = sorted(set(re.findall(r"scripts/[A-Za-z0-9_.\-]+", t)))
    print(f"  {f}")
    for rf in refs:
        tk = tracked(rf)
        mark = "OK      " if tk else "!!未跟踪!!"
        print(f"     {mark} {rf}")

print("\n=== 3. 脚本 -> 数据文件依赖（D5/棘轮类）===")
pairs = [
    ("scripts/check_doc_consistency.ps1", ["scripts/mojibake_baseline.txt"]),
    ("scripts/check_perf_redlines.py", ["scripts/perf_redlines.toml", "scripts/perf_thresholds.toml",
                                        "scripts/bench_inventory_freeze.txt",
                                        "scripts/ignored_test_inventory_freeze.txt"]),
    ("scripts/audit_phaseR_artifacts.py", []),
]
for script, deps in pairs:
    if not os.path.exists(script):
        print(f"  {script}: 不存在，跳过")
        continue
    txt = io.open(script, encoding="utf-8", errors="replace").read()
    print(f"  {script} (tracked={tracked(script)})")
    for d in deps:
        mentioned = os.path.basename(d) in txt
        print(f"     {'依赖成立' if mentioned else '未引用'} -> {d} (tracked={tracked(d)})")
