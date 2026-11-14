#!/usr/bin/env python3
"""G-143 — ToolExecutor 声明表穿透静态门（ADR-295 D1 / R11 W1）

不变量：``crates/*/src`` 生产码中，任何同时覆写了 ``capability_of`` 与
``declared_tools`` 的 ``impl ToolExecutor for X`` 块，必须同时覆写
``declared_side_effects``。

WHY 这条规则：能力视图三方法（capability_of / declared_tools /
declared_side_effects）是装饰器/包装器的「转发契约面」。
declared_side_effects 是 R9 后加的方法——exec_plan.rs 三个装饰器、
ToolRuntime、ExecPolicyToolExecutor 都只转发前两个 ⇒ 配置驱动的批量
声明表在装饰器链被静默丢弃，MCP 并行封顶保持 0（ADR-295 背景③）。
「同时覆写前两者」是「本 impl 是能力视图转发面」的可靠签名——只覆写
execute 的最小实现、只覆写 capability_of 的探针夹具都不是，不误报。

豁免标记：impl 块内任意一行含 ``SIDE_EFFECT_FORWARDING_EXEMPT:``（同行
后跟理由）即可豁免——用于真值源不在 delegate 的合法形态。

退出码：0 = 干净；1 = 存在违例；2 = 扫描目录缺失 / 自证失败（致盲守卫，
绝不静默判绿）。

自证：``--selftest`` 对内嵌夹具断言门有牙（红被抓、绿放过、cfg(test)
豁免、显式豁免、泛型参数），任一不成立退 2。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# impl 头：允许泛型参数与 trait 的完整路径前缀（gqep_executor::ToolExecutor）
IMPL_RE = re.compile(
    r"impl\s*(?:<[^>]*>)?\s*(?:[\w:]+::)*ToolExecutor\s+for\s+(\w+)"
)
EXEMPT_MARK = "SIDE_EFFECT_FORWARDING_EXEMPT:"


def strip_cfg_test_regions(text: str) -> str:
    """把 #[cfg(test)] mod ... { } 区域替换为等长空白，避免测试夹具误报。

    处理形态：``#[cfg(test)]`` 行后跟 ``mod <name> { ... }``（含嵌套 brace）。
    保留行号与换行数（替换成空白而非删除），保证诊断行号仍指向原文件。
    """
    lines = text.splitlines(keepends=True)
    out: list[str] = []
    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        if "#[cfg(test)]" in line:
            # 找其后第一条 mod 声明行
            j = i + 1
            while j < n and not lines[j].lstrip().startswith("mod "):
                if lines[j].strip() and not lines[j].lstrip().startswith("#"):
                    break  # 非 mod 且非属性 ⇒ 不是 cfg(test) mod，停止
                j += 1
            if j < n and lines[j].lstrip().startswith("mod "):
                # 从 mod 行开始做 brace 配平，配平后整体抹白
                depth = 0
                seen_open = False
                k = j
                while k < n:
                    for ch in lines[k]:
                        if ch == "{":
                            depth += 1
                            seen_open = True
                        elif ch == "}":
                            depth -= 1
                    k += 1
                    if seen_open and depth == 0:
                        break
                for idx in range(i, k):
                    out.append(" " * len(lines[idx]))
                i = k
                continue
        out.append(line)
        i += 1
    return "".join(out)


def find_impl_blocks(text: str) -> list[tuple[str, int, str]]:
    """返回 (类型名, impl 块起始行号 1-based, 块全文)。brace 配平取整块。"""
    blocks: list[tuple[str, int, str]] = []
    for m in IMPL_RE.finditer(text):
        name = m.group(1)
        # impl 头的 `{`（泛型/where 子句里不该有裸 `{`，取 `for <Name>` 后首个）
        open_idx = text.find("{", m.end())
        if open_idx == -1:
            continue
        depth = 0
        end_idx = open_idx
        for idx in range(open_idx, len(text)):
            ch = text[idx]
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
                if depth == 0:
                    end_idx = idx
                    break
        body = text[m.start() : end_idx + 1]
        line_no = text.count("\n", 0, m.start()) + 1
        blocks.append((name, line_no, body))
    return blocks


def check_text(text: str) -> list[str]:
    """对一段生产码文本执行门规则，返回违例描述（空列表 = 干净）。"""
    violations: list[str] = []
    stripped = strip_cfg_test_regions(text)
    for name, line_no, body in find_impl_blocks(stripped):
        has_cap = "fn capability_of(" in body
        has_tools = "fn declared_tools(" in body
        has_table = "fn declared_side_effects(" in body
        if has_cap and has_tools and not has_table:
            if EXEMPT_MARK in body:
                continue
            violations.append(
                f"impl ToolExecutor for {name} (line {line_no}): "
                "覆写了 capability_of + declared_tools 但未覆写 declared_side_effects "
                "—— 配置驱动的批量声明表会在此被静默丢弃（ADR-295 D1）。"
                "补转发，或用 SIDE_EFFECT_FORWARDING_EXEMPT: <理由> 显式豁免"
            )
    return violations


def collect_source_files() -> list[Path]:
    crates = sorted((REPO_ROOT / "crates").glob("*/src"))
    files: list[Path] = []
    for src in crates:
        files.extend(sorted(src.rglob("*.rs")))
    return files


def run_gate() -> int:
    files = collect_source_files()
    if not files:
        print("G-143: crates/*/src 下未找到任何 .rs —— 致盲守卫，退 2", file=sys.stderr)
        return 2
    violations: list[str] = []
    scanned = 0
    for f in files:
        try:
            text = f.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            print(f"G-143: 无法读取 {f}: {exc}", file=sys.stderr)
            return 2
        scanned += 1
        for v in check_text(text):
            violations.append(f"{f}: {v}")
    if violations:
        print(f"G-143: {len(violations)} 处声明表穿透缺口：")
        for v in violations:
            print(f"  - {v}")
        return 1
    print(f"G-143: 干净（扫描 {scanned} 个 src 文件）")
    return 0


# ---------------------------------------------------------------- selftest

SELFTEST_CASES: list[tuple[str, str, bool]] = [
    # (用例名, 源码, 期望是否违例)
    (
        "green_full_forwarding",
        """
        #[async_trait]
        impl ToolExecutor for Good {
            async fn execute(&self, _t: &str, _a: &str) -> Result<String, String> { Ok(String::new()) }
            fn capability_of(&self, _t: &str) -> Option<SideEffectDecl> { None }
            fn declared_tools(&self) -> Vec<String> { Vec::new() }
            fn declared_side_effects(&self) -> Vec<(String, SideEffectDecl)> { Vec::new() }
        }
        """,
        False,
    ),
    (
        "red_missing_table_forwarding",
        """
        #[async_trait]
        impl ToolExecutor for Bad {
            async fn execute(&self, _t: &str, _a: &str) -> Result<String, String> { Ok(String::new()) }
            fn capability_of(&self, _t: &str) -> Option<SideEffectDecl> { None }
            fn declared_tools(&self) -> Vec<String> { Vec::new() }
        }
        """,
        True,
    ),
    (
        "exempt_marker_passes",
        """
        #[async_trait]
        impl ToolExecutor for Excused {
            async fn execute(&self, _t: &str, _a: &str) -> Result<String, String> { Ok(String::new()) }
            fn capability_of(&self, _t: &str) -> Option<SideEffectDecl> { None }
            fn declared_tools(&self) -> Vec<String> { Vec::new() }
            // SIDE_EFFECT_FORWARDING_EXEMPT: 真值源是配置视图，非 delegate
        }
        """,
        False,
    ),
    (
        "cfg_test_mod_ignored",
        """
        #[async_trait]
        impl ToolExecutor for Prod {
            async fn execute(&self, _t: &str, _a: &str) -> Result<String, String> { Ok(String::new()) }
            fn capability_of(&self, _t: &str) -> Option<SideEffectDecl> { None }
            fn declared_tools(&self) -> Vec<String> { Vec::new() }
            fn declared_side_effects(&self) -> Vec<(String, SideEffectDecl)> { Vec::new() }
        }

        #[cfg(test)]
        mod tests {
            use super::*;
            struct Fixture;
            #[async_trait]
            impl ToolExecutor for Fixture {
                async fn execute(&self, _t: &str, _a: &str) -> Result<String, String> { Ok(String::new()) }
                fn capability_of(&self, _t: &str) -> Option<SideEffectDecl> { None }
                fn declared_tools(&self) -> Vec<String> { Vec::new() }
            }
        }
        """,
        False,
    ),
    (
        "minimal_execute_only_passes",
        """
        #[async_trait]
        impl ToolExecutor for Minimal {
            async fn execute(&self, _t: &str, _a: &str) -> Result<String, String> { Ok(String::new()) }
        }
        """,
        False,
    ),
    (
        "generic_impl_red",
        """
        #[async_trait]
        impl<S> ToolExecutor for Wrapper<S> {
            async fn execute(&self, _t: &str, _a: &str) -> Result<String, String> { Ok(String::new()) }
            fn capability_of(&self, _t: &str) -> Option<SideEffectDecl> { None }
            fn declared_tools(&self) -> Vec<String> { Vec::new() }
        }
        """,
        True,
    ),
    (
        "qualified_trait_path_red",
        """
        #[async_trait]
        impl gqep_executor::ToolExecutor for Qualified {
            async fn execute(&self, _t: &str, _a: &str) -> Result<String, String> { Ok(String::new()) }
            fn capability_of(&self, _t: &str) -> Option<SideEffectDecl> { None }
            fn declared_tools(&self) -> Vec<String> { Vec::new() }
        }
        """,
        True,
    ),
]


def selftest() -> int:
    failures: list[str] = []
    for case_name, src, expect_violation in SELFTEST_CASES:
        got = check_text(src)
        if expect_violation and not got:
            failures.append(f"{case_name}: 期望红（门必须抓住缺口）却放行")
        if not expect_violation and got:
            failures.append(f"{case_name}: 期望绿却报违例: {got}")
    if failures:
        print("G-143 selftest 失败：", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        return 2
    print(f"G-143 selftest: {len(SELFTEST_CASES)} 夹具全过（红被抓/绿放过/cfg(test) 豁免/显式豁免/泛型/限定路径）")
    return 0


def main() -> int:
    if "--selftest" in sys.argv:
        return selftest()
    return run_gate()


if __name__ == "__main__":
    sys.exit(main())
