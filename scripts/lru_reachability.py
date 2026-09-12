#!/usr/bin/env python3
"""判定 RUSTSEC-2026-0253 的影响面是否经 ratatui 实际用到的 API 可达。

告警标题只点名 `LruCache::pop()`（panic 安全缺失 → use-after-free）。
但 ratatui 用的是 `push()`（0.12 的插入 API），而 **push 在容量满时内部逐出**。
所以"没调 pop"这句是否等于"不可达"，取决于逐出是否复用 pop 的那段 unsafe 代码。
本脚本读 lru 0.12.5 源码把这件事定死。
"""
import glob
import io
import os
import re
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

roots = glob.glob(".toolchain/cargo/registry/src/*/lru-0.12.5")
if not roots:
    print("未找到 lru-0.12.5 源码")
    sys.exit(1)
src_dir = os.path.join(roots[0], "src")
files = [f for f in sorted(os.listdir(src_dir)) if f.endswith(".rs")]
print("lru-0.12.5/src:", files)

text = {}
for f in files:
    text[f] = io.open(os.path.join(src_dir, f), encoding="utf-8", errors="replace").read()

whole = "\n".join(text.values())
unsafe_count = len(re.findall(r"\bunsafe\b", whole))
print(f"\nunsafe 出现次数 = {unsafe_count}")

print("\n=== 关键函数定义位置 ===")
for f, t in text.items():
    for n, line in enumerate(t.splitlines(), 1):
        if re.search(r"pub fn (push|pop|peek|get|get_mut)\b|fn pop_lru\b|fn detach\b|fn unsafe_move_to_", line):
            print(f"  {f}:L{n}: {line.strip()[:100]}")

# 逐出路径：push 里调了什么
print("\n=== push() 函数体（看它是否走 pop_lru/detach）===")
for f, t in text.items():
    lines = t.splitlines()
    for n, line in enumerate(lines):
        if re.search(r"pub fn push\b", line):
            body = lines[n:n + 26]
            print(f"  --- {f}:L{n+1}")
            for k, l in enumerate(body):
                print(f"     {n+1+k}: {l[:104]}")
            break

print("\n=== pop_lru() / detach() 是否含 unsafe ===")
for f, t in text.items():
    lines = t.splitlines()
    for n, line in enumerate(lines):
        if re.search(r"fn (pop_lru|detach)\b", line):
            seg = lines[n:n + 12]
            has_unsafe = any("unsafe" in s for s in seg)
            print(f"  {f}:L{n+1} {line.strip()[:70]}  这段含 unsafe = {has_unsafe}")
            for l in seg:
                if "unsafe" in l or "ptr" in l or "pop" in l:
                    print(f"        | {l.strip()[:96]}")
