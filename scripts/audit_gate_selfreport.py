#!/usr/bin/env python3
"""把 read_dc_log.py 升级为自给自足的常驻判据 audit_gate_selfreport.py。

为什么必须自跑：原版读的是 `tmp/sr_ps.log` / `tmp/sr_sh.log` —— 那是某次手工执行的中间产物。
作为"每轮都要跑"的判据，依赖上一次留下的日志意味着它会随日志过期而给出无意义结果
（甚至可能拿到旧绿证而报 OK）。常驻判据必须自己产生输入。

判据三条件（原版就有，保留）：
  1. 必须出现 `[OK] ... (N categories / M check ids, self-reported)`；
  2. 自报的 N/M 必须等于**日志里实际出现的 distinct 检查 id 数**（抓中途截断）；
  3. 实算 id 数不得低于下限（抓"某条检查被静默去掉"）。
退出码：0 全部满足 / 1 不满足 / 2 不可判定（门禁跑不起来或无自报行）。
"""
import io
import os
import re
import shutil
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(ROOT)
LOGDIR = os.path.join("tmp", "gate_selfreport")
os.makedirs(LOGDIR, exist_ok=True)

OK_RE = re.compile(r"\((\d+) categories / (\d+) check ids, self-reported\)")
ID_RE = re.compile(r"^\[(?:GAP-|WARN-|ERROR-)?([A-F]\d[a-z]?)(?:-INFO)?\]")

# 下限取自本波实测；只允许因"新增检查"而上调，不允许下跌（下跌=有检查被去掉或没跑）
TARGETS = [
    ("ps1", ["pwsh", "-NoProfile", "-File", "scripts/check_doc_consistency.ps1"], 14),
    ("sh", ["bash", "scripts/check_doc_consistency.sh"], 7),
]

rc_all = 0


def judge(label, text, rc, floor):
    """对一份门禁输出套用三条判据；返回退出码贡献（0/1/2）。"""
    ids, m = set(), None
    for line in text.splitlines():
        g = ID_RE.match(line.strip())
        if g:
            ids.add(g.group(1))
        mm = OK_RE.search(line)
        if mm:
            m = mm
    if m is None:
        print(f"{label}: 不可判定 —— 没有自报汇总行（rc={rc}）")
        return 2
    rep_cat, rep_id = int(m.group(1)), int(m.group(2))
    act_cat = len({i[0] for i in ids})
    gaps = [l.strip() for l in text.splitlines() if "[GAP-" in l]
    inconsistent = (rep_id != len(ids) or rep_cat != act_cat)
    below = len(ids) < floor
    ok = (rc == 0) and not inconsistent and not below and not gaps
    print(f"{label}: 自报 {rep_cat}类/{rep_id}id | 实算 {act_cat}类/{len(ids)}id | "
          f"GAP {len(gaps)} | 下限 {floor} | rc={rc} -> {'OK' if ok else 'MISMATCH'}")
    if inconsistent:
        print("    原因：自报与实算不符（可能是中途截断/输出未刷盘/自报行被改写）")
    if below:
        print(f"    原因：实算 id {len(ids)} < 下限 {floor}（有检查被去掉或未执行）")
    for g in gaps[:3]:
        print("    GAP:", g[:120])
    if ok:
        return 0
    return 1 if rc in (0, 1) else 1


# --from-log：只判已有日志（不重跑门禁）。它让本工具的三条判据能被负对照直接检验，
# 否则要构造“自报与实算不符”就得真去干扰正在执行的门禁。
if "--from-log" in sys.argv:
    _, path, label, floor = sys.argv[1:5]
    code = judge(label or os.path.basename(path),
                 io.open(path, encoding="utf-8", errors="replace").read(), 0, int(floor))
    print("[OK] 符合期望" if code == 0 else f"结论：rc={code}")
    sys.exit(code)

for label, cmd, floor in TARGETS:
    exe = shutil.which(cmd[0])
    if not exe:
        print(f"{label}: 不可判定 —— 找不到可执行 {cmd[0]}（不视为通过）")
        rc_all = 2
        continue
    log = os.path.join(LOGDIR, f"selfreport_{label}.log")
    with open(log, "wb") as lf:
        rc = subprocess.run(cmd, stdout=lf, stderr=subprocess.STDOUT).returncode
    text = io.open(log, encoding="utf-8", errors="replace").read()
    contrib = judge(label, text, rc, floor)
    if contrib and rc_all == 0:
        rc_all = contrib if contrib != 2 else 2
    if contrib == 2 and rc_all != 2:
        rc_all = 2

print("[OK] 两执行体的自报计数与实算一致且达下限" if rc_all == 0 else f"结论：rc={rc_all}")
sys.exit(rc_all)
