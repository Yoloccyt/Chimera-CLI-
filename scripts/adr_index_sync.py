#!/usr/bin/env python3
"""adr_index.md ↔ 磁盘 ADR 文件的双向核对（含"有意免档"豁免）。

背景（本会话实测）：初版探针把 ADR-158 报成坏链，查证后发现索引行**自述**
"不单独建档，登记于 phase5-wave5-closure.md T4，无独立物理文件" —— 与
ADR-086~094（登记于 v4.0 §2.5）同属**有意的免档模式**，不是缺陷。
探针若不解析豁免，就会稳定产出假阳，久而久之没人再看它的红。

判定口径：
  * 索引行含免档标记（"无独立物理文件" / "不单独建档" / "登记于"）→ 计入 exempt，不算坏链
  * 合并文件 `ADR-135-144-*.md` 展开为 135..144（含多版本 `-revN` 文件）
  * 索引有 + 磁盘无 + 无豁免 → BROKEN LINK（真缺陷）
  * 磁盘有 + 索引无行 → UNINDEXED（真缺陷）
"""
import io
import os
import re
import sys

IDX_PATH = "docs/architecture/adr_index.md"
ADR_DIR = "docs/architecture"
# 历史/Released 编号：索引明文声明"仅在 CHANGELOG.md 汇总"，不要求磁盘文件
PROSE_EXEMPT = re.compile(r"无独立物理文件|不单独建档|仅在\s*CHANGELOG|登记于")
MERGED = re.compile(r"^ADR-(\d{3})-(\d{3})(?:-rev\d+)?[^/]*\.md$")
SINGLE = re.compile(r"^ADR-(\d{3})(?:-\d{3})?-[^/]*\.md$")


def disk_numbers():
    """返回 {主编号: 文件名}，合并文件展开为区间。"""
    out = {}
    for root, dirs, files in os.walk(ADR_DIR):
        if os.path.basename(root) in ("target", ".git"):
            continue
        for fn in files:
            if not fn.endswith(".md"):
                continue
            m = MERGED.match(fn)
            if m:
                lo, hi = int(m.group(1)), int(m.group(2))
                for n in range(lo, hi + 1):
                    out.setdefault(n, fn)
                continue
            m = SINGLE.match(fn)
            if m:
                out.setdefault(int(m.group(1)), fn)
    return out


def index_rows(idx_text):
    """返回 {编号: (行文本, 是否免档)}。"""
    rows = {}
    for line in idx_text.splitlines():
        m = re.match(r"\|\s*ADR-(\d{3})\s*\|(.*)$", line)
        if m:
            n = int(m.group(1))
            rows.setdefault(n, (line, bool(PROSE_EXEMPT.search(m.group(2)))))
    return rows


def main():
    idx = io.open(IDX_PATH, encoding="utf-8").read()
    rows = index_rows(idx)
    disk = disk_numbers()

    broken = [n for n in rows if n not in disk and not rows[n][1]]
    exempt = sorted(n for n in rows if n not in disk and rows[n][1])
    unindexed = sorted(n for n in disk if n not in rows)

    print(f"索引表格行={len(rows)}  磁盘主编号(展开后)={len(disk)}")
    print(f"免档豁免（索引自述无独立文件）: {exempt or '无'}")
    print(f"BROKEN LINK（索引有行、磁盘无文件、无豁免）: {broken or '无'}")
    print(f"UNINDEXED（磁盘有文件、索引无行）        : {unindexed or '无'}")
    for n in (161, 166, 167, 168, 169):
        src = rows.get(n)
        print(f"  ADR-{n}: 索引={'有' if src else '无'}"
              f"{'(免档)' if src and src[1] else ''} 磁盘={'有' if n in disk else '无'}"
              f"{' -> ' + disk[n] if n in disk else ''}")
    return 1 if (broken or unindexed) else 0


if __name__ == "__main__":
    sys.exit(main())
