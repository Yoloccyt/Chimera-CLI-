#!/usr/bin/env python3
"""Cargo.lock 里 lru 的全部版本与引用者（为 ADR-167 的豁免范围定界）。

关键问题：RUSTSEC 说 lru 的 patched 版本是 >=0.18.2，而 ratatui 钉 ^0.12.0；
若锁里还有第二个 lru（如 tracing-log 的 optional ^0.7.7 被启用），则"豁免"必须
覆盖**多个**版本，ADR 的措辞与 audit 的 ignore 写法都要跟着变。
"""
import io
import re
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

t = io.open("Cargo.lock", encoding="utf-8", errors="replace").read()
blocks = re.split(r"\n(?=\[\[package\]\])", t)

pkgs = {}
for b in blocks:
    nm = re.search(r'^name = "([^"]+)"', b, re.M)
    vr = re.search(r'^version = "([^"]+)"', b, re.M)
    if nm and vr:
        pkgs.setdefault(nm.group(1), []).append((vr.group(1), b))

print("=== 锁里的 lru 版本 ===")
lru_versions = [v for v, _b in pkgs.get("lru", [])]
for v in lru_versions:
    print(f"  lru v{v}")
if not lru_versions:
    print("  (锁里没有 lru？)")

print("\n=== 谁引用哪个 lru（按 lock 的 'lru <ver>' 依赖串）===")
for name, entries in sorted(pkgs.items()):
    for ver, b in entries:
        refs = set(re.findall(r'"lru ([0-9][0-9.]*)"', b))
        if refs:
            print(f"  {name} v{ver} -> lru {sorted(refs)}")

print("\n=== 结论要素 ===")
print(f"  锁内 lru 版本集合: {sorted(set(lru_versions))}")
print(f"  0.12.5 是否为唯一在用版本: {sorted(set(lru_versions)) == ['0.12.5']}")
adv = {"0.12.5": "受影响（patched >=0.18.2）", "0.7.10": "同样早于 0.18.2，若在用亦受影响"}
for v in sorted(set(lru_versions)):
    print(f"  lru {v}: {adv.get(v, '未列入本告警口径，需核对 advisory 区间')}")
