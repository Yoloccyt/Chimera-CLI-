#!/usr/bin/env python3
"""数准 REGISTERED_FOCUS_ORDER 长度，并定位"25 vs 27"注释矛盾谁对谁错。

上一版正则被 `pub const X: &[PanelId] = &[` 里的 `&[` 挡住，误报"未找到"。
"""
import io
import re
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

src = io.open("crates/chimera-tui/src/types.rs", encoding="utf-8", errors="replace").read()
i = src.find("pub const REGISTERED_FOCUS_ORDER")
assert i > 0, "未找到 REGISTERED_FOCUS_ORDER"
j = src.find("];", i)
body = src[i:j]
items = re.findall(r"PanelId::([A-Za-z0-9]+)", body)
print(f"REGISTERED_FOCUS_ORDER 长度 = {len(items)}")

# 枚举全集
k = src.find("enum PanelId")
seg = src[k:src.find("\n}", k)]
variants = re.findall(r"^\s{4}([A-Z][A-Za-z0-9]*)\s*,\s*$", seg, re.M)
print(f"PanelId 枚举变体数     = {len(variants)}")
unreg = [v for v in variants if v not in items]
print(f"未注册变体             = {len(unreg)}: {unreg}")

print("\n=== 代码注释里的面板数断言（找矛盾）===")
pat = re.compile(r"(2[0-9])\s*面板")
for f in ("src/types.rs", "src/app/mod.rs", "src/app/tests.rs", "tests/integration.rs"):
    p = "crates/chimera-tui/" + f
    for n, l in enumerate(io.open(p, encoding="utf-8", errors="replace").read().splitlines(), 1):
        for m in pat.finditer(l):
            print(f"  {f}:L{n}: {m.group(0)!r} :: {l.strip()[:104]}")

print("\n结论口径：焦点环面板数 = REGISTERED_FOCUS_ORDER.len()，枚举变体数 = 上值；")
print("写文档时必须指明是哪一个，且以 `types.rs` 实测为准。")
