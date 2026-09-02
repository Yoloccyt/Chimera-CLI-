#!/usr/bin/env python3
"""manifest 负对照（v3）：统一按 LF 读写，避免 newline 双重翻译。

上一版把行用 "\\r\\n" join 后又以 newline="\\r\\n" 写盘 → 每个换行变成 "\\r\\r\\n"，
tomllib 报 "invalid character '\\r'"，表现为 R 崩溃。那是**对照脚本自己的 bug**。
本轮同时验证：R 崩溃时必须退 2 并打印原因（不得伪装成判据失败退 1）。
"""
import hashlib
import io
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

M = "scripts/gate_manifest.toml"
R = "scripts/run_gate_manifest.py"


def sha(p):
    return hashlib.sha256(open(p, "rb").read()).hexdigest()


def run(*args):
    r = subprocess.run([sys.executable, R, *args], capture_output=True,
                       text=True, encoding="utf-8", errors="replace")
    return r.returncode, (r.stdout or "") + (r.stderr or "")


def lines_of():
    """按通用换行读入，返回不含行尾符的行列表（写回统一用 LF）。"""
    return io.open(M, encoding="utf-8").read().splitlines()


def write(ls):
    io.open(M, "w", encoding="utf-8", newline="\n").write("\n".join(ls) + "\n")


def find(pred, what, ls):
    for i, l in enumerate(ls):
        if pred(l):
            return i
    raise AssertionError(f"锚点未命中：{what}")


ORIG = open(M, "rb").read()
H0 = sha(M)
fails = []


def expect(label, cond, detail=""):
    print(f"  [{'PASS' if cond else 'FAIL'}] {label}" + (f" :: {detail[:96]}" if detail else ""))
    if not cond:
        fails.append(label)


try:
    # NC-0（工具健壮性）：写入语法坏掉的 manifest → 必须 [TOOL-ERROR]/[MANIFEST-INVALID] + 退 2
    ls = lines_of()
    ls[find(lambda l: l.strip().startswith("[[gate"), "首个 [[gate]]", ls)] = "[[gate"
    write(ls)
    rc, out = run()
    expect("NC-0 清单损坏 → 退 2 且明说不可判定", rc == 2 and ("TOOL-ERROR" in out or "MANIFEST-INVALID" in out), f"rc={rc}")

    # NC-A：缺 spec_ref → 清单失真，退 2
    open(M, "wb").write(ORIG)
    ls = lines_of()
    ls[find(lambda l: l.strip() == 'spec_ref = "R1-T4 / T5"', "G-01 spec_ref", ls)] = "# removed for NC-A"
    write(ls)
    rc, out = run()
    expect("NC-A 缺 spec_ref → 退 2 并报清单失真", rc == 2 and "MANIFEST-INVALID" in out and "spec_ref" in out, f"rc={rc}")

    # NC-B：期望值篡改成不可能 → MISMATCH，退 1
    open(M, "wb").write(ORIG)
    ls = lines_of()
    j = find(lambda l: l.strip() == 'id = "G-06"', "G-06", ls)
    k = next(x for x in range(j, j + 8) if ls[x].strip().startswith("expect ="))
    ls[k] = "expect = 7"
    write(ls)
    rc, out = run()
    expect("NC-B 期望被篡改 → MISMATCH 且退 1", rc == 1 and "MISMATCH" in out, f"rc={rc}")

    # NC-C：heavy 磁盘门槛不足 → UNKNOWN 且退 2，且不执行 cargo
    open(M, "wb").write(ORIG)
    ls = lines_of()
    j = find(lambda l: l.strip() == 'id = "G-12"', "G-12", ls)
    k = next(x for x in range(j, j + 9) if ls[x].strip().startswith("disk_gb ="))
    ls[k] = "disk_gb = 900"
    write(ls)
    rc, out = run("--with-heavy")
    no_exec = "G-12] OK" not in out and "G-12] MISMATCH" not in out
    expect("NC-C 磁盘不足 → UNKNOWN 退 2 且未执行 cargo", rc == 2 and "UNKNOWN" in out and no_exec, f"rc={rc}")

    # NC-D：命令不存在 → FAIL 退 1（不静默通过）
    open(M, "wb").write(ORIG)
    ls = lines_of()
    j = find(lambda l: l.strip() == 'id = "G-06"', "G-06", ls)
    k = next(x for x in range(j, j + 8) if ls[x].strip().startswith("cmd ="))
    ls[k] = 'cmd = "definitely-not-a-real-binary-xyz --help"'
    write(ls)
    rc, out = run()
    expect("NC-D 命令不存在 → FAIL 且退 1", rc == 1 and "G-06] FAIL" in out, f"rc={rc}")
finally:
    open(M, "wb").write(ORIG)
    expect("manifest 按 sha256 完整还原", sha(M) == H0)

rc, _ = run()
expect("还原后 light 基线 rc=0", rc == 0, f"rc={rc}")
print("\n[NC] " + ("ALL PASS" if not fails else "FAIL: " + "; ".join(fails)))
sys.exit(1 if fails else 0)
