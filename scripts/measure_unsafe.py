#!/usr/bin/env python3
"""重测 R1-T2 台账所需的 unsafe / forbid 覆盖数字（不凭记忆搬数）。

口径与 `scripts/check_dependency_rules.sh` 的检查 B 保持一致：
  * `forbid_covered`：crate 顶层（非 tests/、非 examples/、非 benches/）出现
    `#![forbid(unsafe_code)]` 的 crate 数
  * `unsafe_sites`：`crates/*/src` 下 `unsafe` 关键字作为块/impl/fn 起点的出现次数
  * `unsafe_crates`：含 unsafe 站点的 crate 数
另外单列 dev/tests 目录的 unsafe（测试允许，不计入红线）。
"""
import io
import os
import re
from collections import defaultdict

FORBID = re.compile(r"#!\[[^\]]*forbid\s*\(\s*unsafe_code\s*\)")
UNSAFE = re.compile(r"\bunsafe\b")

crates = sorted(d for d in os.listdir("crates") if os.path.isdir(os.path.join("crates", d)))
covered, missing = [], []
sites = defaultdict(int)

for c in crates:
    root = os.path.join("crates", c, "src")
    if not os.path.isdir(root):
        missing.append(c)
        continue
    has_forbid = False
    for r, dirs, files in os.walk(root):
        for fn in files:
            if not fn.endswith(".rs"):
                continue
            p = os.path.join(r, fn)
            t = io.open(p, encoding="utf-8", errors="replace").read()
            if FORBID.search(t):
                has_forbid = True
            n = len(UNSAFE.findall(t))
            if n:
                sites[c] += n
    if has_forbid:
        covered.append(c)
    else:
        missing.append(c)

total = sum(sites.values())
print(f"crate 总数（crates/ 目录）        = {len(crates)}")
print(f"顶层含 forbid(unsafe_code) 的 crate = {len(covered)}")
print(f"缺 forbid 的 crate                = {len(missing)} {missing if missing else ''}")
print(f"src 下 unsafe 关键字站点总数       = {total}")
print(f"含 unsafe 站点的 crate 数          = {len(sites)}")
for c, n in sorted(sites.items(), key=lambda kv: -kv[1]):
    print(f"    {c:22} {n}")

# tests/ 目录（dev-only，不计红线）
dev = 0
for c in crates:
    for sub in ("tests", "benches", "examples"):
        d = os.path.join("crates", c, sub)
        if os.path.isdir(d):
            for r, _, files in os.walk(d):
                for fn in files:
                    if fn.endswith(".rs"):
                        dev += len(UNSAFE.findall(
                            io.open(os.path.join(r, fn), encoding="utf-8", errors="replace").read()))
print(f"（对照）tests/benches/examples 下 unsafe = {dev}，按红线口径不计入")
