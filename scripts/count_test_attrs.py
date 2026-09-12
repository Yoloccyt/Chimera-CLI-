#!/usr/bin/env python3
"""按"测试属性宏"分口径统计静态测试数，判定 AGENTS.md 的 11,433 静态口径是否仍成立。

动机：Spec §5 的红线"禁删既有测试"需要一个可复现的静态计数口径。上一版只数
`#[test]` 得 9,488，与文档的 11,433 差 1,945 —— 疑似漏了 `#[tokio::test]` / `#[rstest]`
等带属性的测试宏，而不是测试被删。必须分辨清楚，否则会把"口径差异"误报成"红线违反"。
"""
import io
import os
import re
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

PATS = {
    "#[test]": r"^\s*#\[test\]",
    "#[tokio::test]": r"#\[tokio::test\]",
    "#[tokio::test(...)": r"#\[tokio::test\(",
    "#[rstest]": r"#\[rstest(?=\]|\()",
    "#[test_case(..)]": r"#\[test_case\b",
    "#[quicktest]": r"#\[quicktest\]",
    "#[proptest]": r"#\[proptest\]",
}

tot = {k: 0 for k in PATS}
for base in ("crates", "tests", "fuzz"):
    if not os.path.isdir(base):
        continue
    for dp, dirs, fs in os.walk(base):
        dirs[:] = [d for d in dirs if d != "target"]
        for f in fs:
            if not f.endswith(".rs"):
                continue
            try:
                t = io.open(os.path.join(dp, f), encoding="utf-8", errors="replace").read()
            except OSError:
                continue
            for k, p in PATS.items():
                tot[k] += len(re.findall(p, t, re.M))

for k, v in tot.items():
    print(f"  {k:20} x{v}")
broad = sum(tot.values())
narrow = tot["#[test]"]
print("  " + "-" * 44)
print(f"  仅 #[test]            = {narrow}")
print(f"  全部测试属性宏合计     = {broad}")
print("  AGENTS.md 静态口径参照 = 11,433")
print()
if broad >= 11433 * 0.98:
    print("判定：静态计数与文档口径基本吻合（差异 <2%）→ '禁删既有测试' 未见违反迹象")
else:
    gap = 11433 - broad
    print(f"判定：仍差 {gap} 条，需继续分辨是“文档数字过期”还是“我的口径仍不全”"
          "（可能来源：根 tests/e2e 的 #[test] 在 mod 内、doctest、宏展开、或该 11,433 本身过期）")
