#!/usr/bin/env python3
"""负对照：证明 runner 现在能抓到"死判据"（引用不存在脚本的条目），且 light 模式也报。

这是本轮真实踩到的坑：我登记 G-19 时忘了提升脚本，而 kind=heavy 让它在 light 模式下
永不执行 —— 一条永远轮不到、也不会报错的判据，比没有判据更糟（它会让人以为已覆盖）。
"""
import hashlib
import io
import os
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

M = "scripts/gate_manifest.toml"
orig = open(M, "rb").read()
h0 = hashlib.sha256(orig).hexdigest()


def run(*args):
    r = subprocess.run([sys.executable, "scripts/run_gate_manifest.py", *args],
                       capture_output=True, text=True, encoding="utf-8", errors="replace",
                       env=dict(os.environ, PYTHONIOENCODING="utf-8"))
    return r.returncode, (r.stdout or "") + (r.stderr or "")


fails = []

rc0, out0 = run()
base_ok = rc0 == 0 and "MANIFEST-INVALID" not in out0
print(f"  [{'PASS' if base_ok else 'FAIL'}] 基线：当前清单无死判据，rc={rc0}")
if not base_ok:
    print("    ", out0.strip().splitlines()[-3:])
    fails.append("baseline")

try:
    io.open(M, "wb").write(orig + b"\n[[gate]]\nid = \"G-XX\"\nspec_ref = \"NC\"\n"
                               b"cmd = \"{python} scripts/never_exists_nc_xyz.py\"\n"
                               b"expect = 0\nkind = \"heavy\"\nnote = \"NC\"\n")
    rc1, out1 = run()
    caught = rc1 == 2 and "G-XX" in out1 and "never_exists_nc_xyz.py" in out1
    print(f"  [{'PASS' if caught else 'FAIL'}] 注入死判据（kind=heavy，light 不执行）→ rc={rc1} 被抓={caught}")
    if not caught:
        fails.append("dead-gate not caught")
        print("    ", out1.strip().splitlines()[-4:])
finally:
    open(M, "wb").write(orig)
    restored = hashlib.sha256(open(M, "rb").read()).hexdigest() == h0
    print(f"  [{'PASS' if restored else 'FAIL'}] manifest 按 sha256 还原")
    if not restored:
        fails.append("restore")

rc2, _ = run()
if rc2 != rc0:
    fails.append("post-restore rc changed")
print(f"  [{'PASS' if rc2 == rc0 else 'FAIL'}] 还原后 rc 回到基线（{rc2}）")

print("\n[NC] " + ("ALL PASS" if not fails else "FAIL: " + "; ".join(fails)))
sys.exit(1 if fails else 0)
