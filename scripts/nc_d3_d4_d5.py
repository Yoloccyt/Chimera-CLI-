#!/usr/bin/env python3
"""D3 / D4 / D5 三条新门禁的阴性对照（备份 → 变异 → 跑门 → 还原 → sha256 校验）。"""
import hashlib
import io
import os
import shutil
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

IDX = "docs/architecture/adr_index.md"
WAVE1 = "docs/reports/phaseR-wave1-closure.md"     # D5 注入对象（其 cap = 0）
WRITEBACK = "docs/reports/phase4-wave4-closure.md"  # D4 的关闭回写所在行（:58）
CLO = WRITEBACK
GATE = "scripts/check_doc_consistency.ps1"
MARK = "→ 已关闭（2026-08-31 ADR-168 回写）"
fails = []


def sha(p):
    return hashlib.sha256(open(p, "rb").read()).hexdigest()


def run_gate():
    r = subprocess.run(["pwsh", "-NoProfile", "-File", GATE],
                       capture_output=True, text=True, encoding="utf-8", errors="replace")
    return r.returncode, (r.stdout or "") + (r.stderr or "")


def mutate(path, fn):
    orig = open(path, "rb").read()
    shutil.copyfile(path, path + ".ncbak")
    new = fn(orig.decode("utf-8"))
    assert new != orig.decode("utf-8"), f"mutation no-op on {path}"
    io.open(path, "w", encoding="utf-8", newline="\n").write(new)
    return orig


def restore(path, orig):
    io.open(path, "wb").write(orig)
    bak = path + ".ncbak"
    if os.path.exists(bak):
        os.remove(bak)
    assert sha(path) == hashlib.sha256(orig).hexdigest(), f"RESTORE MISMATCH {path}"


def check(label, cond, detail=""):
    print(f"  [{('PASS' if cond else 'FAIL')}] {label}" + (f" -- {detail}" if detail else ""))
    if not cond:
        fails.append(label)


rc, out = run_gate()
print(f"基线: EXIT={rc}")
check("baseline green", rc == 0)

print("\nNC-1 (D3): 索引插入无文件、无豁免的 ADR-777")
h = sha(IDX)
orig = mutate(IDX, lambda t: t.replace("| ADR-161 |", "| ADR-777 | NC | - |\n| ADR-161 |", 1))
try:
    rc, out = run_gate()
    line = next((l.strip() for l in out.splitlines() if "GAP-D3" in l), "")
    check("D3 caught bogus row", rc != 0 and "ADR-777" in line, line[:120])
finally:
    restore(IDX, orig)
    check("D3 file restored", sha(IDX) == h)

print("\nNC-2 (D4): 撤掉 closure:58 的关闭回写")
h = sha(CLO)


def undo_writeback(t):
    return t.replace(
        " ⚠️ ADR-155 登记 + RK-P23 缓冲消耗 1 次 " + MARK
        + " ADR-157 双口径三条件判定已取代本单值口径；release 实测 combined 0.552→0.999，"
        "RK-P23 状态 ✅ 已关闭",
        " ⚠️ ADR-155 登记 + RK-P23 缓冲消耗 1 次", 1)


orig = mutate(CLO, undo_writeback)
try:
    rc, out = run_gate()
    line = next((l.strip() for l in out.splitlines() if "GAP-D4" in l), "")
    check("D4 caught missing write-back", rc != 0 and "closure.md:58" in line, line[:120])
finally:
    restore(CLO, orig)
    check("D4 file restored", sha(CLO) == h)

print("\nNC-3 (D5): 往基线为 0 的干净文档（wave1 报告）注入一行 mojibake 特征")
h = sha(WAVE1)
orig = mutate(WAVE1, lambda t: t.rstrip("\n") + "\n\n探针污染行示例：结尾锛?\n")
try:
    rc, out = run_gate()
    line = next((l.strip() for l in out.splitlines() if "GAP-D5" in l), "")
    check("D5 caught injected damage", rc != 0 and "phaseR-wave1-closure.md = 1" in line, line[:140])
finally:
    restore(WAVE1, orig)
    check("D5 file restored", sha(WAVE1) == h)

print('\nNC-4 (D5 棘轮方向): 把 CHANGELOG 的 cap 降到 0，它本有的伤必须被判红')
# CHANGELOG 基线为 94：把 cap 提高到 200 不应变红；反之把某文件 cap 降到 0 而它有伤应变红。
BASE = "scripts/mojibake_baseline.txt"
h = sha(BASE)
orig = mutate(BASE, lambda t: t.replace("94\tCHANGELOG.md", "0\tCHANGELOG.md", 1))
try:
    rc, out = run_gate()
    line = next((l.strip() for l in out.splitlines() if "GAP-D5" in l), "")
    check("D5 honors lowered cap", rc != 0 and "CHANGELOG.md" in line, line[:140])
finally:
    restore(BASE, orig)
    check("baseline file restored", sha(BASE) == h)

rc, out = run_gate()
print(f"\n全部还原后: EXIT={rc}")
check("green after restores", rc == 0)

print("\n[NC] " + ("ALL PASS" if not fails else "FAIL: " + "; ".join(fails)))
sys.exit(1 if fails else 0)
