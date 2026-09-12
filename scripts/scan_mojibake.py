#!/usr/bin/env python3
"""扫描 CHANGELOG 中疑似 mojibake（UTF-8 被按 GBK 读后回写）残留的行。

判据只用"几乎不会在正常中文技术文档里出现"的罕见字，避免把 `满`/`线` 这类
合法常用字误判成损伤（第一版把 `满` 放进判据就产生了假阳）。
`?` 紧跟这些字是 GBK 单字节截断的特征。
"""
import io
import re
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

TARGETS = ["CHANGELOG.md", "agents.md", ".claude/CLAUDE.md",
           "docs/reports/phaseR-wave1-closure.md", "docs/governance/RK-P_risk_register.md",
           "docs/architecture/adr_index.md"]

# 罕见字集：这些字在正常中文技术写作里几乎不单独出现，而在 GBK 误读产物里高频。
# 上一版把 `存` 放进集内，导致"存储/保存"满屏假阳——真损伤的硬特征不是罕见字本身，
# 而是**罕见字 + 紧跟 `?`**（UTF-8 尾字节被截断为不可表示字符）。
RARE = "锛鍙澶浠閾绫缁涓氳鐨勬槸璇槑"
PAT = re.compile("[" + RARE + "]\\?")
# 辅助信号：U+FFFD 替换字（解码失败的直接证据）
FFFD = re.compile("\ufffd")

for p in TARGETS:
    try:
        lines = io.open(p, encoding="utf-8").read().splitlines()
    except OSError as e:
        print(f"{p}: 读不到（{e}）")
        continue
    hits = [(n, l) for n, l in enumerate(lines, 1) if PAT.search(l) or FFFD.search(l)]
    nfffd = sum(l.count("\ufffd") for l in lines)
    print(f"{p}: 损伤行 {len(hits)} / 总 {len(lines)} 行, U+FFFD x{nfffd}")
    for n, l in hits[:4]:
        print(f"    L{n}: {l[:120]}")
