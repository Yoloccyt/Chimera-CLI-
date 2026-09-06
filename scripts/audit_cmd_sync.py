#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""cargo audit 口径三处一致性门禁（ADR-167 决策 4 的机械实现）。

背景：`audit.yml` 与各规则文档（`AGENTS.md §7.2`、`.claude/CLAUDE.md` §2 与 §5 第 8 项）
各自抄了一份 `cargo audit` 命令。P5-T5(2026-08-28) 把 CI 侧的 `--deny warnings` 改为
`--deny unmaintained --deny unsound` 时，两份文档没跟着改——照文档手跑的人会得出与 CI
不同的结论。这正是本仓反复出现的"两份互相抄、其中一份不生效"病灶，只是这次落在安全判据上。

用法:
    python scripts/audit_cmd_sync.py            # 比对；一致则退出 0
    python scripts/audit_cmd_sync.py -v         # 打印每处解析出的 deny/ignore

退出码:
    0 = 三处逐字等价
    1 = 存在不一致（打印差异）
    2 = 环境/用法错误——**包括目标文件不存在**。这一条是刻意的：
        `.md` 不入库（.gitignore:54-56），在 CI 的 checkout 里根本看不到文档，
        若此时静默"通过"，就等于造出一个永远为真的假门禁（本仓第 5 次同类病灶）。
        因此缺失文件一律判"不可判定"，只有本地发布前检查能给出真结论。

设计取舍：单一实现 + 单文件，不学 check_perf_redlines 那样做 .sh/.ps1 双包装——
本检查只在本地发布流程跑，没有第二个执行体，加包装只会制造新的漂移面。
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# audit.yml 是入库对象；两份文档是本地权威源（不入库，见模块 docstring）
TARGETS = (".github/workflows/audit.yml", "AGENTS.md", ".claude/CLAUDE.md")


def commands(path: Path) -> list[str]:
    """抽取该文件里的 `cargo audit` 命令（含 YAML 反斜杠 / PS 反引号续行）。

    跳过：纯注释行（描述的是历史变更）与 Markdown 表格行（叙述而非命令）。
    """
    out: list[str] = []
    lines = path.read_text(encoding="utf-8-sig").splitlines()
    i = 0
    while i < len(lines):
        raw = lines[i].strip()
        if not raw.lstrip("|").startswith("#") and "cargo audit" in raw \
                and not raw.startswith("|"):
            buf = raw
            while re.search(r"[\\`]$", buf) and i + 1 < len(lines):
                i += 1
                buf = re.sub(r"[\\`]$", "", buf).strip() + " " + lines[i].strip()
            if "--deny" in buf or "--ignore" in buf:
                out.append(re.sub(r"\s+", " ", buf))
        i += 1
    return out


def key(cmd: str) -> tuple[frozenset[str], frozenset[str]]:
    """从 `audit` 起连续消费 `--旗标 值`，遇第一个非选项 token 即停。

    WHY 要截断：文档常在命令后追加中文解释从句（如"…（与 audit.yml:61 逐字一致；
    已不用 `--deny warnings`）"），不截断就会把从句里的旧旗标当成当前态。
    """
    toks = cmd.split()
    if "audit" not in toks:
        return frozenset(), frozenset()
    seg: list[str] = []
    i = toks.index("audit") + 1
    while i < len(toks):
        flag = toks[i].strip("`").rstrip(",;.")
        if not flag.startswith("--"):
            break
        seg.append(flag)
        if i + 1 < len(toks):
            val = toks[i + 1].strip("`").rstrip(",;.")
            # 只吃旗标的值；`--开头 / 括号从句 / 注释 都留给下一轮判定
            if val and not val.startswith(("--", "（", "#", ")")):
                seg.append(val)
                i += 2
                continue
        i += 1
    joined = " ".join(seg)
    return (frozenset(re.findall(r"--deny\s+([a-z]+)", joined)),
            frozenset(re.findall(r"--ignore\s+(RUSTSEC-\d{4}-\d{4})", joined)))


def main(argv: list[str]) -> int:
    verbose = "-v" in argv
    results: dict[str, list[tuple[frozenset[str], frozenset[str]]]] = {}
    for rel in TARGETS:
        path = ROOT / rel
        if not path.exists():
            # 大小写不敏感兜底：仓库内该文件历史上出现过 agents.md / AGENTS.md 两种写法
            alt = ROOT / rel.lower()
            if alt.exists():
                path = alt
            else:
                print(f"[FAIL] required file not found: {rel} "
                      "(此检查不可判定，不得视为通过)", file=sys.stderr)
                return 2
        cmds = commands(path)
        if not cmds:
            print(f"[FAIL] no `cargo audit` command parsed from {rel} "
                  "(解析失败即为门禁失效，不得静默放过)", file=sys.stderr)
            return 1
        keys = [key(c) for c in cmds]
        results[rel] = keys
        if verbose:
            for c, (d, ig) in zip(cmds, keys):
                print(f"  {rel}: deny={sorted(d)} ignore={sorted(ig)}")

    # 每处文件的最后一条命令为该文件的当前态（文档 §2 在前、清单在后）
    current = {rel: keys[-1] for rel, keys in results.items()}
    baseline = current[TARGETS[0]]
    bad = [rel for rel in TARGETS[1:] if current[rel] != baseline]
    if bad:
        print("[FAIL] 与 audit.yml 口径不一致:", file=sys.stderr)
        print(f"  audit.yml          deny={sorted(baseline[0])} "
              f"ignore={sorted(baseline[1])}", file=sys.stderr)
        for rel in bad:
            d, ig = current[rel]
            print(f"  {rel} deny={sorted(d)} ignore={sorted(ig)}", file=sys.stderr)
        return 1
    print(f"[PASS] {len(TARGETS)} 处 cargo audit 口径逐字等价："
          f"deny={sorted(baseline[0])}, ignore={len(baseline[1])} 项")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
