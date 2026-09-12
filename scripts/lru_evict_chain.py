#!/usr/bin/env python3
"""闭合"push 是否经 pop_lru 逐出"这一环，并核对 ratatui 实际调用的 LruCache 方法。

若 push 在容量满时内部调 pop_lru，则"ratatui 未调用 pop()"**不足以**支撑
RUSTSEC-2026-0253 不可达的结论 —— ADR-167 方案 A 的论证必须改写。
"""
import glob
import io
import os
import re
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

lru = glob.glob(".toolchain/cargo/registry/src/*/lru-0.12.5/src/lib.rs")[0]
lines = io.open(lru, encoding="utf-8", errors="replace").read().splitlines()

print("=== capturing_put 的容量/逐出分支 (L370-435) ===")
for n in range(369, 436):
    l = lines[n]
    if re.search(r"pop_lru|while |cap\b|detach|attach|unsafe|fn |=>", l):
        print(f"  L{n+1}: {l.strip()[:106]}")

print("\n=== pop_lru() 函数体 (L1189-1215) ===")
for n in range(1188, 1216):
    print(f"  L{n+1}: {lines[n][:104]}")

print("\n=== pop() 函数体（告警点名者, L1109-1135）===")
for n in range(1108, 1136):
    l = lines[n]
    if l.strip():
        print(f"  L{n+1}: {l[:104]}")

root = glob.glob(".toolchain/cargo/registry/src/*/ratatui-0.29.0/src/layout/layout.rs")[0]
r = io.open(root, encoding="utf-8", errors="replace").read()
print("\n=== ratatui layout.rs 里对 cache 的调用 ===")
for n, l in enumerate(r.splitlines(), 1):
    if re.search(r"\.(push|pop|pop_lru|get|get_mut|put|peek|cap|clear|iter)\s*\(", l) and "cache" in l.lower():
        print(f"  L{n}: {l.strip()[:110]}")
calls = sorted(set(re.findall(r"cache\.(\w+)\s*\(", r)))
print("  cache.<method> 集合:", calls)
print("  是否直接调 pop/pop_lru:", [c for c in calls if c in ("pop", "pop_lru")] or "否")
