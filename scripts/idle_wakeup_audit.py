#!/usr/bin/env python3
"""空闲唤醒审计（R5-1，ADR-264）——能效代理指标③的静态扫描器。

扫描 crates/*/src 的轮询/睡眠调用点，按「测试 vs 生产」粗分类统计：
- interval/sleep 是空闲唤醒（能耗税）的静态来源；
- 勘察教训（ADR-262 §4）：裸 grep 计数被测试代码严重污染（top 候选全为
  测试等待循环）——本工具把「测试/生产」分类做进统计口径，产出可比较的
  基线（治理前后对比用）。

分类启发式（诚实边界，dev-only 工具不做完整语法分析）：
  以文件内最后一个**顶层** `#[cfg(test)]` / `mod tests` 出现处为界，
  其后计为测试区（本仓测试模块集中于文件尾部的惯例布局）；界前者为生产。
  跨界 mod 布局的文件会误分类——报告中同时给出总数与分区数，使用者可抽查。

用法（repo 根运行）：
  python scripts/idle_wakeup_audit.py            # 打印 markdown 基线
  python scripts/idle_wakeup_audit.py --json     # 机器可读
退出码恒 0（advisory 工具，不入门禁；门禁化属后续裁决）。
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATES = ROOT / "crates"

# 轮询/睡眠调用形态（含常见别名写法）
PATTERNS = {
    "interval": re.compile(r"tokio::time::interval\s*\(|interval\s*=\s*tokio::time::interval"),
    "tokio_sleep": re.compile(r"tokio::time::sleep\s*\("),
    "thread_sleep": re.compile(r"std::thread::sleep\s*\("),
}

TEST_BOUNDARY = re.compile(r"#\[\s*cfg\s*\(\s*test\s*\)\s*\]|^\s*mod\s+tests\b", re.M)


def classify_line_no(text: str) -> int:
    """返回测试区起始行号（1-based）：最后一个顶层 test 边界标记所在行。
    找不到则返回 len+1（全文视为生产）。"""
    last = None
    for m in TEST_BOUNDARY.finditer(text):
        # 仅认行首列 0 的顶层边界（缩进的子模块测试不算顶层分界）
        if m.start() == 0 or text.rfind("\n", 0, m.start()) == -1 or (
            text[: m.start()].rsplit("\n", 1)[-1].strip() == ""
        ):
            last = m.start()
    if last is None:
        return len(text.splitlines()) + 1
    return text[:last].count("\n") + 1


def scan_file(path: Path) -> dict:
    text = path.read_text(encoding="utf-8", errors="ignore")
    test_start = classify_line_no(text)
    out = {"prod": 0, "test": 0, "kinds": {}}
    for line_no, line in enumerate(text.splitlines(), 1):
        for kind, pat in PATTERNS.items():
            if pat.search(line):
                zone = "test" if line_no >= test_start else "prod"
                out[zone] += 1
                out["kinds"].setdefault(kind, {"prod": 0, "test": 0})[zone] += 1
                break  # 一行只计一次
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description="idle wakeup static audit (R5-1)")
    parser.add_argument("--json", action="store_true", help="machine-readable output")
    args = parser.parse_args()

    rows = []
    for crate_dir in sorted(CRATES.iterdir()) if CRATES.exists() else []:
        src = crate_dir / "src"
        if not src.is_dir():
            continue
        agg = {"crate": crate_dir.name, "prod": 0, "test": 0}
        for rs in src.rglob("*.rs"):
            r = scan_file(rs)
            agg["prod"] += r["prod"]
            agg["test"] += r["test"]
        if agg["prod"] or agg["test"]:
            rows.append(agg)

    total_prod = sum(r["prod"] for r in rows)
    total_test = sum(r["test"] for r in rows)

    if args.json:
        print(json.dumps({"total_prod": total_prod, "total_test": total_test,
                          "by_crate": rows}, indent=2))
        return 0

    print("# 空闲唤醒静态审计基线（R5-1,ADR-264）\n")
    print(f"- 生产区轮询/睡眠点：**{total_prod}**")
    print(f"- 测试区（不计入能耗面）：{total_test}")
    print("\n| crate | 生产 | 测试 |")
    print("|---|---|---|")
    for r in sorted(rows, key=lambda x: -x["prod"]):
        print(f"| {r['crate']} | {r['prod']} | {r['test']} |")
    print(
        "\n> 口径：生产区 = 最后一个顶层 test 边界之前（启发式,见 docstring）；\n"
        "> 基线用途：R5-3 TUI 空闲 Eco 等治理的前后对比;生产区长周期定时器\n"
        "> （60s/5s/1s）为正当周期任务,治理以 TUI tick 面为主（ADR-264 §3）。"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
