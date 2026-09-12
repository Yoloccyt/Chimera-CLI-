#!/usr/bin/env python3
# 复测 Spec §7 数字来源台账里各量的当前值。
# 每个数都附测量方式，避免引用无人可复算的数字。
# 昂贵的 target/ 全量遍历已刻意省略：本日早期已测得 110.60 GB（debug 103.42 / release 6.85），重扫不产生新信息。
import io
import os
import re
import subprocess
import sys
import tomllib

sys.stdout.reconfigure(encoding="utf-8", errors="replace")


def run(argv):
    r = subprocess.run(argv, capture_output=True, text=True, encoding="utf-8", errors="replace")
    return (r.stdout or "") + (r.stderr or "")


print("=== 1) 孤岛 LOC（台账行要求本阶段重跑锁定）===")
out = run(["bash", "scripts/check_crate_reachability.sh"])
for pat in (r"members: \d+, production reachable[^_]*", r"frozen unreachable src lines:.*"):
    m = re.search(pat, out)
    print("  " + (m.group(0).strip() if m else "(未匹配: " + pat[:28] + ")"))

print("\n=== 2) ignored 台账行的各口径 ===")
t = io.open("scripts/ignored_test_inventory_freeze.txt", encoding="utf-8").read()
entries = [l for l in t.splitlines() if l.strip() and not l.strip().startswith("#")]
tags = {}
for l in entries:
    for tag in ("slo-blocking", "slo-daily", "unignore-target", "manual-only"):
        if tag in l:
            tags[tag] = tags.get(tag, 0) + 1
print(f"  登记表条目 = {len(entries)}  分布 = {tags}")
r1 = re.search(r"max_unignore_pending\s*=\s*(\d+)", t)
print(f"  unignore 棘轮 = {r1.group(1) if r1 else '?'}")
with open("scripts/perf_thresholds.toml", "rb") as f:
    doc = tomllib.load(f)
rc = doc.get("ratchet", {})
print(f"  calibration_pending = {len(rc.get('calibration_pending', []))} / 上限 = {rc.get('max_calibration_pending')}")

print("\n=== 3) bench 三态与红线计数 ===")
inv = io.open("scripts/bench_inventory_freeze.txt", encoding="utf-8").read()
dev_only = [l for l in inv.splitlines() if l.strip() and not l.strip().startswith("#")]
txt = io.open("scripts/perf_redlines.toml", encoding="utf-8").read()
print(f"  dev-only 冻结条目 = {len(dev_only)}")
print(f"  perf_redlines.toml 内 run_bench 出现次数 = {txt.count('run_bench')}")
print(f"  阈值键数 = {len(doc.get('thresholds_ns', {}))} / 关键词表 = {len(doc.get('path_keywords', {}))}")

print("\n=== 4) 静态测试数口径 ===")
o2 = run([sys.executable, "tmp/count_test_attrs.py"])
for l in o2.splitlines():
    if "全部测试属性宏合计" in l or l.strip().startswith("仅 #"):
        print("  " + l.strip())

print("\n=== 5) 体积与磁盘 ===")
for p in ("target/release/chimera.exe", "target/release/chimera"):
    if os.path.exists(p):
        print(f"  {p} = {os.path.getsize(p) / 1024 ** 2:.2f} MB")
o3 = run(["powershell", "-NoProfile", "-Command",
          "[math]::Round((Get-PSDrive D).Free/1GB,1)"])
print("  D 盘空闲(GB) = " + (o3.strip().splitlines()[-1] if o3.strip() else "?"))
print("  target/ 体积 = 沿用本日早期实测 110.60 GB（台账当时标注为未测，现已测）")
