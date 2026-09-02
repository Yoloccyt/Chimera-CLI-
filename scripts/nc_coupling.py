#!/usr/bin/env python3
"""audit_commit_coupling.py 的负对照 + 干净退出码复测。

NC 设计：往一个**已跟踪**的执行体（scripts/check_slash_vocab.sh）末尾加一行注释，
内容是引用一个未跟踪文件（scripts/perf_redlines.toml）。若工具正确，该条必须从
"BOTH/无" 变成 REF_UNTRACKED 且退出码为 1。随后按 sha256 严格还原。
"""
import hashlib
import io
import os
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

VICTIM = "scripts/check_slash_vocab.sh"
NEEDLE = "scripts/perf_redlines.toml"
MARKER = "# coupling-nc: see also scripts/perf_redlines.toml"


def sha(p):
    return hashlib.sha256(open(p, "rb").read()).hexdigest()


def scan():
    r = subprocess.run([sys.executable, "scripts/audit_commit_coupling.py"],
                       capture_output=True, text=True, encoding="utf-8", errors="replace",
                       env=dict(os.environ, PYTHONIOENCODING="utf-8"))
    return r.returncode, (r.stdout or "") + (r.stderr or "")


def hard_line_present(out, holder):
    return any(holder in l and "REF_UNTRACKED" in l for l in out.splitlines())


assert os.path.exists(VICTIM), f"{VICTIM} 不存在，NC 前提不成立"
orig = open(VICTIM, "rb").read()
h0 = sha(VICTIM)
fails = []

rc0, out0 = scan()
base_ok = rc0 == 1 and hard_line_present(out0, "scripts/check_doc_consistency.ps1")
print(f"[基线] rc={rc0}  已抓到 C-3(ps1->mojibake_baseline)={base_ok}")
if not base_ok:
    fails.append("baseline: C-3 未被抓到")
if hard_line_present(out0, VICTIM):
    fails.append(f"基线不该出现 {VICTIM} 的硬耦合")

try:
    io.open(VICTIM, "wb").write(orig + ("\n" + MARKER + "\n").encode("utf-8"))
    rc1, out1 = scan()
    caught = rc1 == 1 and hard_line_present(out1, VICTIM)
    print(f"[NC] 注入已跟踪文件引用未跟踪文件 → rc={rc1} 抓到={caught}")
    if not caught:
        fails.append("NC: 新引入的耦合没被抓到（工具无牙齿）")
finally:
    io.open(VICTIM, "wb").write(orig)
    restored = sha(VICTIM) == h0
    print(f"[还原] sha256 一致 = {restored}")
    if not restored:
        fails.append("restore mismatch")

rc2, _ = scan()
print(f"[还原后] rc={rc2}（应与基线一致 = {rc0}）")
if rc2 != rc0:
    fails.append("还原后退出码变化")

print("\n[NC] " + ("ALL PASS" if not fails else "FAIL: " + "; ".join(fails)))
sys.exit(1 if fails else 0)
