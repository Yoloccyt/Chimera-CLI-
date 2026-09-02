#!/usr/bin/env python3
"""D3 / D4 两条新门禁的阴性对照（含还原校验）。

不做负对照的门禁等于没写：本脚本对真实文件做**临时变异**，要求两条检查各自变红，
随后立即还原并用 sha256 逐字节确认还原无损（变异前也在磁盘留了 .ncbak 备份，
中途崩溃可人工恢复）。
"""
import hashlib
import io
import os
import shutil
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

IDX = "docs/architecture/adr_index.md"
CLO = "docs/reports/phase4-wave4-closure.md"
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
    """备份 → 应用 fn(text) → 写盘；返回原文 bytes。"""
    orig = open(path, "rb").read()
    shutil.copyfile(path, path + ".ncbak")
    text = orig.decode("utf-8")
    new = fn(text)
    assert new != text, f"mutation did not apply to {path}"
    io.open(path, "w", encoding="utf-8", newline="\n").write(new)
    return orig


def restore(path, orig_bytes):
    io.open(path, "wb").write(orig_bytes)
    bak = path + ".ncbak"
    if os.path.exists(bak):
        os.remove(bak)
    assert sha(path) == hashlib.sha256(orig_bytes).hexdigest(), f"restore mismatch for {path}"


print(f"基线：gate 应为绿")
rc, out = run_gate()
print(f"  基线 EXIT={rc}（期望 0）")
if rc != 0:
    fails.append("baseline not green")

# --- NC-1: D3 必须抓到"索引指向不存在的 ADR 文件" ---
print("\nNC-1 (D3): 往索引插入一行指向不存在文件的 ADR-777")
h0 = sha(IDX)
orig = mutate(IDX, lambda t: t.replace(
    "| ADR-161 |", "| ADR-777 | NC-only row, no physical file | - |\n| ADR-161 |", 1))
try:
    rc, out = run_gate()
    line = [l.strip() for l in out.splitlines() if "GAP-D3" in l]
    caught = rc != 0 and line and "ADR-777" in line[0]
    print(f"  EXIT={rc} 抓到={'是' if caught else '否'}")
    if line:
        print(f"  {line[0][:150]}")
    if not caught:
        fails.append("D3 did not catch bogus index row")
finally:
    restore(IDX, orig)
    print(f"  还原校验: {'OK' if sha(IDX) == h0 else 'MISMATCH'}")

# --- NC-2: D4 必须抓到"登记册说已关闭、但来源行没回写" ---
print("\nNC-2 (D4): 把 closure:58 的关闭回写撤掉，模拟未回写状态")
h1 = sha(CLO)
orig2 = mutate(CLO, lambda t: t.replace(
    " ⚠️ ADR-155 登记 + RK-P23 缓冲消耗 1 次 " + MARK + " ADR-157 双口径三条件判定已取代本单值口径；"
    "release 实测 combined 0.552→0.999，RK-P23 状态 ✅ 已关闭",
    " ⚠️ ADR-155 登记 + RK-P23 缓冲消耗 1 次", 1))
try:
    rc, out = run_gate()
    line = [l.strip() for l in out.splitlines() if "GAP-D4" in l]
    caught = rc != 0 and bool(line)
    print(f"  EXIT={rc} 抓到={'是' if caught else '否'}")
    if line:
        print(f"  {line[0][:170]}")
    if not caught:
        fails.append("D4 did not catch missing write-back")
finally:
    restore(CLO, orig2)
    print(f"  还原校验: {'OK' if sha(CLO) == h1 else 'MISMATCH'}")

# --- 还原后必须回到绿 ---
print("\n还原后复跑")
rc, out = run_gate()
print(f"  EXIT={rc}（期望 0）")
if rc != 0:
    fails.append("not green after restore")

print("\n[NC] " + ("ALL PASS" if not fails else "FAIL: " + "; ".join(fails)))
sys.exit(1 if fails else 0)
