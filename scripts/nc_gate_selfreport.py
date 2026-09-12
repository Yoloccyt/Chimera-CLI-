#!/usr/bin/env python3
"""audit_gate_selfreport.py 的负对照：三条判据必须各自能独立变红。

用 --from-log 模式对**同一份真实门禁输出**做两种破坏：
  T1 篡改自报 id 数（14 → 99）→ 必须 MISMATCH(1)
  T2 截断日志但保留自报行（模拟中途被杀/未刷盘）→ 必须 MISMATCH(1)
  T3 下限抬到高于实算（模拟"有检查被静默去掉"）→ 必须 MISMATCH(1)
  T4 无自报行 → 必须"不可判定"(2)，不得当作通过
  基线 原始日志 → 必须 OK(0)
"""
import io
import os
import re
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

TOOL = "scripts/audit_gate_selfreport.py"
LOGDIR = "tmp/gate_selfreport"
os.makedirs(LOGDIR, exist_ok=True)

# 取一份真实门禁输出作底本
base_log = os.path.join(LOGDIR, "nc_base.log")
subprocess.run(["pwsh", "-NoProfile", "-File", "scripts/check_doc_consistency.ps1"],
               stdout=open(base_log, "wb"), stderr=subprocess.STDOUT)
text = io.open(base_log, encoding="utf-8", errors="replace").read()
m = re.search(r"\((\d+) categories / (\d+) check ids", text)
assert m, "底本没有自报行，NC 前提不成立"
real_ids = int(m.group(2))


def run(path, floor):
    r = subprocess.run([sys.executable, TOOL, "--from-log", path, "nc", str(floor)],
                       capture_output=True, text=True, encoding="utf-8", errors="replace")
    return r.returncode, (r.stdout or "") + (r.stderr or "")


def write(name, body):
    p = os.path.join(LOGDIR, name)
    io.open(p, "w", encoding="utf-8", newline="\n").write(body)
    return p


fails = []


def chk(label, cond, detail=""):
    print(f"  [{'PASS' if cond else 'FAIL'}] {label}" + (f" :: {detail[:100]}" if detail else ""))
    if not cond:
        fails.append(label)


rc0, out0 = run(base_log, real_ids)
chk("基线：真实门禁输出 + 下限=实算 → 0", rc0 == 0, f"rc={rc0}")

p1 = write("nc_t1.log", text.replace(f"{real_ids} check ids", "99 check ids"))
rc1, _ = run(p1, real_ids)
chk("T1 自报被篡改为 99 → 1", rc1 == 1, f"rc={rc1}")

drop = re.compile(r"^\[(D3|D4|D5|E1|E2|F1|F2|B1|B2)\]")
trunc = "\n".join(l for l in text.splitlines() if not drop.match(l.strip()))
p2 = write("nc_t2.log", trunc)
rc2, out2 = run(p2, real_ids)
chk("T2 截断日志（保留自报行）→ 1 且指出自报≠实算", rc2 == 1 and "实算不符" in out2, f"rc={rc2}")

rc3, out3 = run(base_log, real_ids + 5)
chk("T3 下限高于实算（有检查被去掉）→ 1 且指出未达下限", rc3 == 1 and "< 下限" in out3, f"rc={rc3}")

body = "\n".join(l for l in text.splitlines() if "categories /" not in l)
p4 = write("nc_t4.log", body)
rc4, out4 = run(p4, real_ids)
chk("T4 无自报行 → 2（不可判定，不当通过）", rc4 == 2 and "不可判定" in out4, f"rc={rc4}")

for f in (base_log, p1, p2, p4):
    try:
        os.remove(f)
    except OSError:
        pass

print("\n[NC] " + ("ALL PASS —— 三条判据各自可独立变红，且缺自报行判不可判定"
                  if not fails else "FAIL: " + "; ".join(fails)))
sys.exit(1 if fails else 0)
