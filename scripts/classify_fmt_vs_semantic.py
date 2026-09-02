#!/usr/bin/env python3
"""严格判定：哪些 dirty Rust 文件的改动 100% 由 rustfmt 产生（无夹带语义改动）。

判据（比 `git diff -w` 强得多）：
    rustfmt(HEAD 版) == 工作区版   ->  纯格式（rustfmt 的 AST 保持契约 + 逐字节相等）
    否则                            ->  含语义改动
`git diff -w` 之所以不够：rustfmt 会把一行拆成多行，`-w` 只忽略既有空白，
拆行仍会被算作改动，于是"纯格式文件"也显示非零 diff，判据失效。
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(r"D:\Chimera CLI")
# cargo fmt 走 rustup 代理。⚠ PATH 顺序必须让 **toolchain bin 在前**：`.toolchain/cargo/bin`
# 里也有一个 rustfmt 代理，它在无参数从 stdin 读时失败，会让所有文件变成“无法判定”。
os.environ["PATH"] = (str(ROOT / ".toolchain" / "rustup" / "toolchains"
                          / "stable-x86_64-pc-windows-gnu" / "bin") + os.pathsep
                     + str(ROOT / ".toolchain" / "cargo" / "bin")
                     + os.pathsep + os.environ.get("PATH", ""))
RUSTFMT = shutil.which("rustfmt")
BOM = b"\xef\xbb\xbf"


def git(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True,
                          text=False)


def rustfmt(text: bytes) -> bytes | None:
    # rustfmt 无文件名时默认读 stdin（实测可用）；`--stdin-file-path` 在本版本是非法选项。
    # 比较前剔 BOM：BOM 增删属编码层差异而非语义差异（已在报告中单独说明）。
    p = subprocess.run(
        [RUSTFMT, "--edition", "2021", "--emit", "stdout"],
        input=text, capture_output=True, cwd=ROOT)
    if p.returncode != 0:
        # 把失败原因留印（只留首行，避免刷屏），便于区分“真的不能判”与“工具没找到”
        first = (p.stderr.decode("utf-8", "replace").splitlines() or [""])[0]
        print(f"[rustfmt-fail] {first}", file=sys.stderr)
        return None
    return p.stdout.replace(b"\r\n", b"\n").removeprefix(BOM)


def main() -> int:
    if not RUSTFMT:
        print("[FAIL] rustfmt not found on PATH")
        return 2

    out = git("status", "--porcelain", "-uall").stdout.decode("utf-8", "replace")
    tracked_rs, other = [], []
    for line in out.splitlines():
        if not line.strip():
            continue
        code, path = line[:2].strip(), line[3:].strip().strip('"')
        if path.endswith(".rs"):
            tracked_rs.append((code, path))
        else:
            other.append((code, path))

    pure_fmt, semantic, undecidable = [], [], []
    for code, path in tracked_rs:
        if code == "??":            # 新增文件无基线，不属"纯格式"
            semantic.append(path)
            continue
        head = git("show", f"HEAD:{path}")
        if head.returncode != 0:    # 删除等情形
            semantic.append(path)
            continue
        formatted = rustfmt(head.stdout.replace(b"\r\n", b"\n"))
        if formatted is None:
            undecidable.append(path)
            continue
        wp = ROOT / path
        if not wp.exists():          # 删除类变更（如 event_types.rs），不属“纯格式”
            semantic.append(path)
            continue
        work = wp.read_bytes().replace(b"\r\n", b"\n").removeprefix(BOM)
        (pure_fmt if formatted == work else semantic).append(path)

    # 落盘两份名单，供 R1-T2 提交批次直接引用（不在人嘴里重复一遍分类结果）。
    (ROOT / "tmp" / "batch_fmt_only.txt").write_text(
        "\n".join(sorted(pure_fmt)) + "\n", encoding="utf-8")
    (ROOT / "tmp" / "batch_semantic_rs.txt").write_text(
        "\n".join(sorted(semantic)) + "\n", encoding="utf-8")

    print(f".rs dirty 总数: {len(tracked_rs)}")
    print(f"  纯格式(rustfmt(HEAD)==工作区): {len(pure_fmt)}")
    print(f"  含语义改动:                    {len(semantic)}")
    print(f"  无法判定(rustfmt 失败等):      {len(undecidable)}")
    if undecidable:
        print("\n[需人工确认] rustfmt 无法处理的文件:")
        for p in undecidable[:20]:
            print(f"  {p}")
    print("\n含语义改动的 .rs（前 40）:")
    for p in semantic[:40]:
        print(f"  {p}")
    print(f"\n非 .rs 的脏项({len(other)}):")
    for code, p in other:
        print(f"  {code!r:>5} {p}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
