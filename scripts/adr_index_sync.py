#!/usr/bin/env python3
"""adr_index.md ↔ 磁盘 ADR 文件的双向核对（含"有意免档"豁免）。

背景（本会话实测）：初版探针把 ADR-158 报成坏链，查证后发现索引行**自述**
"不单独建档，登记于 phase5-wave5-closure.md T4，无独立物理文件" —— 与
ADR-086~094（登记于 v4.0 §2.5）同属**有意的免档模式**，不是缺陷。
探针若不解析豁免，就会稳定产出假阳，久而久之没人再看它的红。

判定口径：
  * 索引行含免档标记（"无独立物理文件" / "不单独建档" / "登记于"）→ 计入 exempt，不算坏链
  * **索引自身声明的合并段落**（行内/基线段里的 `ADR-095~160`、`ADR-007~025` 等）
    → **双向适用**：段内编号既不强求磁盘文件（第一代 ADR 只登索引不建档），
    也不要求逐号行（合并段以一行/一个合并文件登记）
  * 合并文件 `ADR-135-144-*.md` 展开为 135..144（含多版本 `-revN` 文件）
  * 索引有 + 磁盘无 + 无豁免 + 不在声明段内 → BROKEN LINK（真缺陷）
  * 磁盘有 + 索引无行且不在声明段内 → UNINDEXED（真缺陷）

双源同构义务（ADR-190 纪律）：本探针的判定口径与孪生实现
`check_doc_consistency.ps1` D3 逐字对齐（range 解析 + Released/历史 豁免 + 双向适用）。
前者缺 range 解析时会稳定报 96 条假阳（实测：同一状态下 D3 报 "unrowed 0" 绿，
本探针却报 26 BROKEN + 70 UNINDEXED 红），双源不一致时红会被当成真缺陷上报。
"""
import io
import os
import re
import sys

IDX_PATH = "docs/architecture/adr_index.md"
ADR_DIR = "docs/architecture"
# 历史/Released 编号：索引明文声明"仅在 CHANGELOG.md 汇总"，不要求磁盘文件
PROSE_EXEMPT = re.compile(r"无独立物理文件|不单独建档|仅在\s*CHANGELOG|登记于")
# 与 ps1 D3 $releasedPat 同列：已发布/历史/已废止的编号只存账不建档
RELEASED = re.compile(r"Released|\u5386\u53f2|\u5df2\u5e9f\u5f03")
# 索引声明的合并段落（`ADR-095~160` / `ADR-095-160` 两种写法），双向适用
RANGE_DECL = re.compile(r"ADR-(\d{3})\s*[~-]\s*(\d{3})")
MERGED = re.compile(r"^ADR-(\d{3})-(\d{3})(?:-rev\d+)?[^/]*\.md$")
SINGLE = re.compile(r"^ADR-(\d{3})(?:-\d{3})?-[^/]*\.md$")
# §2.6c (ADR-187): reject placeholder ADR numbers landing on disk. SINGLE/MERGED
# require \d{3}, so a file like `ADR-XXX-*.md` (an UN-numbered governance artifact,
# e.g. the 2026-09-18 "FIXED ... ADR-XXX" phantom) was invisible to this syncer and
# could be cited as a real ADR. PLACEHOLDER matches `ADR-` NOT followed by a digit.
PLACEHOLDER = re.compile(r"^ADR-(?!\d)")
# Shrink-only freeze of already-shipped, referenced-but-VOID placeholders. Each was
# superseded by a real ADR but its historical text is still cited (retiring the last
# reference is the trigger to delete it from disk AND from this set). A NEW placeholder
# not listed here fails the gate.
PLACEHOLDER_FREEZE = {
    # superseded by ADR-187 (inner-ring α′); still cited by ADR-188/adr_index/v3.0 as
    # the pre-adjudication evidence + the ADR-numbering-rule record. VOID, not a live ADR.
    "ADR-XXX-inner-ring-bug-fix.md",
}


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
    """返回 {编号: (行文本, 是否免档)}（逐号行）。"""
    rows = {}
    for line in idx_text.splitlines():
        m = re.match(r"\|\s*ADR-(\d{3})\s*\|(.*)$", line)
        if m:
            n = int(m.group(1))
            body = m.group(2)
            exempt = bool(PROSE_EXEMPT.search(body) or RELEASED.search(body))
            # 同一编号出现多处时保留最强豁免信号（与 ps1 D3 同策）
            if n not in rows or exempt:
                rows[n] = (line, exempt)
    return rows


def declared_ranges(idx_text):
    """索引自身声明的合并段落 → 段内编号集合（双向适用的豁免域）。

    WHY 只解析索引文本而不维护硬编号清单：本探针的设计原则是"豁免从索引
    自己的话里读，不得写死名单"（与 ps1 D3 同纪律）；新增合并段无需改脚本。
    """
    covered = set()
    for lo, hi in RANGE_DECL.findall(idx_text):
        a, b = int(lo), int(hi)
        if a <= b:
            covered.update(range(a, b + 1))
    return covered


def placeholder_files():
    r"""Return sorted list of on-disk files whose name is `ADR-` + non-digit (placeholder
    numbers the \d{3} parsers silently skip). Only .md under ADR_DIR; skips target/.git."""
    found = []
    for root, dirs, files in os.walk(ADR_DIR):
        if os.path.basename(root) in ("target", ".git"):
            continue
        for fn in files:
            if fn.endswith(".md") and PLACEHOLDER.match(fn):
                found.append(fn)
    return sorted(found)


def reconcile(rows, disk, ranges):
    """双向对账（纯函数，便于被夹具直测）→ (broken, exempt, unindexed)。"""
    broken = [
        n for n in rows
        if n not in disk and not rows[n][1] and n not in ranges
    ]
    exempt = sorted(
        n for n in rows
        if n not in disk and (rows[n][1] or n in ranges)
    )
    unindexed = sorted(n for n in disk if n not in rows and n not in ranges)
    return broken, exempt, unindexed


def selftest():
    """自证读数能力：每个判定分支一个植入用例（内存数据，不碰磁盘）。

    WHY 必须带夹具：本轮前该探针因缺 range 解析而稳定报 96 条假阳，而它的孪生
    实现 D3 同状态下报绿——这类“门解释不了自己读数”的红最危险，会被当
    成真缺陷上报或被整体忽略。夹具同时守住反向：声明段不得越界免放。
    """
    checks = []

    def expect(name, cond):
        checks.append((name, cond))

    row = lambda n, body="| 主题 | 决策 | 模块 |": f"| ADR-{n:03d} | {body}"

    # T1 真缺陷：索引有行、磁盘无文件、无任何豁免 -> BROKEN
    b, e, u = reconcile({500: (row(500), False)}, {}, set())
    expect("T1 broken 仍被抓住", b == [500] and not e and not u)

    # T2 行内自述豁免（原有能力不得回退）
    r = row(501, "| 主题 | 无独立物理文件，登于 v4.0 |")
    b, e, u = reconcile({501: (r, True)}, {}, set())
    expect("T2 行内自述豁免", not b and e == [501])

    # T3 新增能力：声明段内编号不强求磁盘文件（第一代 ADR 只登索引）
    b, e, u = reconcile({502: (row(502), False)}, {}, {502})
    expect("T3 声明段免 BROKEN", not b and e == [502])

    # T4 新增能力：合并段的成员不强求逐号行（否则 95..156 全报假阳）
    b, e, u = reconcile({}, {503: "ADR-503-x.md"}, {503})
    expect("T4 声明段免 UNINDEXED", not u and not b)

    # T5 真缺陷：磁盘有文件、无逐号行也不在任何声明段 -> UNINDEXED
    b, e, u = reconcile({}, {504: "ADR-504-x.md"}, set())
    expect("T5 unindexed 仍被抓住", u == [504])

    # T6 防过度放宽：段外编号不得被当作已覆盖
    b, e, u = reconcile({}, {111: "ADR-111-x.md"}, declared_ranges("本索引覆盖 ADR-100~110 段落"))
    expect("T6 段外不免放（边界 111 不属 100~110）", u == [111])

    # T7 两种写法（~ 与 -）均展开，且段内/段尾都算覆盖
    cov = declared_ranges("ADR-095~160 与 ADR-135-144 合并档")
    expect("T7 range 两种写法与端点", 95 in cov and 160 in cov and 135 in cov and 144 in cov)

    # T8 Released/历史行豁免（与 ps1 D3 $releasedPat 同列）
    rr = row(505, "| 主题 | Released 仅存 CHANGELOG |")
    expect("T8 Released 行免档", bool(PROSE_EXEMPT.search(rr) or RELEASED.search(rr)))

    # T9 range 声明不影响占位号拒绝（编号制度不变）
    expect("T9 占位号仍被拒", bool(PLACEHOLDER.match("ADR-XXX-new.md")) and
           not PLACEHOLDER.match("ADR-506-x.md"))

    failed = [name for name, ok in checks if not ok]
    for name, ok in checks:
        print(f"  [{'PASS' if ok else 'FAIL'}] {name}")
    if failed:
        print(f"RESULT: FAIL {failed} ({len(failed)} of {len(checks)} assertions broke)")
    else:
        print(f"RESULT: PASS (all {len(checks)} assertions held)")
    return 0 if not failed else 1


def main():
    idx = io.open(IDX_PATH, encoding="utf-8").read()
    rows = index_rows(idx)
    disk = disk_numbers()
    # 索引自身声明的合并段落：双向适用（与 ps1 D3 $idxRangeCovered 同构）
    ranges = declared_ranges(idx)

    broken, exempt, unindexed = reconcile(rows, disk, ranges)
    placeholders = placeholder_files()
    # new placeholders = any `ADR-XXX`-class file not explicitly frozen as VOID.
    new_placeholders = sorted(set(placeholders) - PLACEHOLDER_FREEZE)

    print(f"索引表格行={len(rows)}  磁盘主编号(展开后)={len(disk)}  索引声明段落覆盖编号={len(ranges)}")
    print(f"免档豁免（索引自述无独立文件或在声明段内）: {exempt or '无'}")
    print(f"BROKEN LINK（索引有行、磁盘无文件、无豁免且不在声明段内）: {broken or '无'}")
    print(f"UNINDEXED（磁盘有文件、无逐号行且不在声明段内）    : {unindexed or '无'}")
    print(f"PLACEHOLDER 冻结（已判 VOID、被引用的占位号）: "
          f"{sorted(set(placeholders) & PLACEHOLDER_FREEZE) or '无'}")
    if new_placeholders:
        print(f"REJECTED 占位 ADR 号落盘（须给真编号或归档，见 ADR-187 §2.6c）: {new_placeholders}")
    for n in (161, 166, 167, 168, 169):
        src = rows.get(n)
        print(f"  ADR-{n}: 索引={'有' if src else '无'}"
              f"{'(免档)' if src and src[1] else ''} 磁盘={'有' if n in disk else '无'}"
              f"{' -> ' + disk[n] if n in disk else ''}")
    return 1 if (broken or unindexed or new_placeholders) else 0


if __name__ == "__main__":
    import gate_rc  # 只在入口需要：崩溃必须退 2, 不得借 1 冒充"判过且红"（F32/F33）
    if "--selftest" in sys.argv:
        sys.exit(gate_rc.run(selftest))
    sys.exit(gate_rc.run(main))
