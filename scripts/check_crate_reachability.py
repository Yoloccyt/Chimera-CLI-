#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""crate 生产可达性棘轮门禁 (ADR-160)。

用法: python scripts/check_crate_reachability.py [--selftest]

判据: 从组合根 `chimera-cli`(唯一发布 bin)出发, 沿各 crate `[dependencies]` 中
指向 workspace 内部成员的**非 optional** 边做 BFS。dev-dependencies /
build-dependencies 一律不参与——测试能引用一个 crate 不代表产品装配了它, 这正是
本门禁存在的理由(2026-08-29 冗余审计: 43 个 crate 仅 28 个生产可达, 15 个孤岛合计
50,604 src LOC, 而同期 `cargo test --workspace` 全绿)。

棘轮语义: 债务只减不增。
  - 新增不可达且未登记 -> [GAP-R] FAIL
  - 登记条目已转正     -> [ROTATE] 提示删条目, PASS(不阻塞还债者)
  - 条目对应 crate 消失 -> [STALE] FAIL(清单自身也要防腐)

退出码: 0=通过 / 1=违反棘轮 / 2=用法或环境错误
输出全部 ASCII, 规避 Windows 控制台 GBK 陷阱(项目脚本约定)。
"""

import os
import sys

try:
    import tomllib
except ImportError:  # pragma: no cover - 环境门槛, 非业务分支
    print("[FAIL] check_crate_reachability.py requires python >= 3.11 (tomllib)")
    sys.exit(2)

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, ".."))
FREEZE_PATH = os.path.join(HERE, "crate_reachability_freeze.txt")

# 组合根 = 唯一发布二进制。chimera-tui / chimera-mas 等由其装配, 无需单列豁免。
ENTRY_POINTS = ("chimera-cli",)


def read_freeze(path):
    """解析冻结清单: 一行一个 crate 名, `#` 起注释(应带 ADR 号或审计出处)。"""
    entries = {}
    if not os.path.exists(path):
        return entries
    with open(path, encoding="utf-8") as fh:
        for raw in fh:
            head, sep, note = raw.partition("#")
            name = head.strip()
            if name:
                entries[name] = (note.strip() if sep else "") or "UNLISTED-REASON"
    return entries


def load_toml(path):
    """读取 TOML 并容忍 UTF-8 BOM。

    WHY: 仓库内 6 个入库 .toml 带 BOM(根 Cargo.toml、audit.toml、chimera-tui/
    gqep-executor/mcp-mesh/nexus-app-server 的 Cargo.toml)。cargo 容忍 BOM,
    tomllib 则在 line 1 col 1 直接抛 TOMLDecodeError —— 门禁必须比被检对象更宽容,
    否则换个 manifest 就瞎了。
    """
    with open(path, "rb") as fh:
        raw = fh.read()
    return tomllib.loads(raw.decode("utf-8-sig"))


def workspace_members(root):
    """根 Cargo.toml 的 workspace.members -> {crate 目录名}。

    WHY 以 members 而非 `ls crates/` 为权威源: members 才是 Cargo 真正编译的集合。
    """
    data = load_toml(os.path.join(root, "Cargo.toml"))
    paths = data.get("workspace", {}).get("members", [])
    return {os.path.basename(str(p).rstrip("/\\")) for p in paths}


def manifest_deps(manifest_path):
    """返回 (生产非 optional 依赖, 生产 optional 依赖)。

    段落语义直接取 TOML 结构, 不做正则猜测: dependencies 与
    target.*.dependencies 算生产边, dev/build-dependencies 一律排除。
    """
    data = load_toml(manifest_path)
    buckets = []
    if isinstance(data.get("dependencies"), dict):
        buckets.append(data["dependencies"])
    for target in data.get("target", {}).values():
        if isinstance(target, dict) and isinstance(target.get("dependencies"), dict):
            buckets.append(target["dependencies"])
    always, optional = set(), set()
    for bucket in buckets:
        for name, spec in bucket.items():
            is_opt = isinstance(spec, dict) and bool(spec.get("optional"))
            (optional if is_opt else always).add(name)
    return always, optional


def build_graph(members, root):
    """{crate: (内部生产依赖, 内部 optional 依赖)}; 缺 manifest 时记 0 出边并告警。"""
    graph = {}
    for crate in sorted(members):
        manifest = os.path.join(root, "crates", crate, "Cargo.toml")
        if not os.path.exists(manifest):
            print(f"[GAP-R] workspace member <{crate}> has no crates/{crate}/Cargo.toml")
            graph[crate] = (set(), set())
            continue
        always, optional = manifest_deps(manifest)
        graph[crate] = ({d for d in always if d in members},
                        {d for d in optional if d in members})
    return graph


def reachable(graph, entry_points):
    """沿非 optional 生产边 BFS。"""
    seen, stack = set(), list(entry_points)
    while stack:
        cur = stack.pop()
        if cur in seen:
            continue
        seen.add(cur)
        stack.extend(graph.get(cur, (set(), set()))[0])
    return seen


def src_loc(crate, root):
    """crate 的 src/*.rs 行数(把债务量一并暴露到 CI 日志)。"""
    total = 0
    src = os.path.join(root, "crates", crate, "src")
    for dirpath, _dirs, files in os.walk(src):
        for name in files:
            if name.endswith(".rs"):
                with open(os.path.join(dirpath, name), encoding="utf-8", errors="replace") as fh:
                    total += sum(1 for _ in fh)
    return total


def evaluate(members, graph, freeze, entry_points):
    """棘轮裁决(纯函数, 便于 selftest 用 mock 图驱动)。

    返回 (islands, gated_only, reach, lines, status)。
    """
    reach = reachable(graph, entry_points)
    entry = set(entry_points)
    # 仅被 optional 边引用的成员 = 条件装配(只有开对应 feature 才进二进制)。
    # "改 feature 门控"是文档允许的还债路径之一, 因此这类成员判为 GATED,
    # 不再计入孤岛(否则门禁与自己的修复指引自相矛盾)。
    opt_referenced = {d for _c, (_a, opt) in graph.items() for d in opt}
    gated_only = sorted(opt_referenced - reach - entry)
    islands = sorted(members - reach - entry - set(gated_only))

    lines, status = [], 0
    for crate in islands:
        if crate in freeze:
            continue
        lines.extend([
            f"[GAP-R] crate <{crate}> is unreachable from the production dependency graph "
            f"of {'/'.join(entry_points)} ({src_loc(crate, ROOT)} src lines)",
            "        it is not listed in scripts/crate_reachability_freeze.txt.",
            "        Pick one:",
            "          1) wire it into the composition root "
            "(crates/chimera-cli/Cargo.toml [dependencies]);",
            "          2) gate it behind a cargo feature: declare it `optional = true` in the "
            "consumer manifest and list it under that feature (precedent: ADR-065 decision 6 "
            "+ the `--features mca` job in ci.yml);",
            "          3) add it to scripts/crate_reachability_freeze.txt with an ADR number "
            "recording why it is intentionally unwired (shadow-first / migration in progress).",
        ])
        status = 1
    for crate in sorted(freeze):
        if crate not in members:
            lines.append(f"[STALE] freeze entry <{crate}> is no longer a workspace member "
                         f"- delete the line")
            status = 1
        elif crate not in islands:
            lines.append(f"[ROTATE] crate <{crate}> is now reachable - debt repaid, "
                         f"delete its freeze entry")
    return islands, gated_only, reach, lines, status


def selftest():
    """内存 mock 图驱动棘轮语义, 不读仓库真实 manifest。"""
    cli, mid, isl = "cli", "mid", "isl"
    entry = (cli,)
    graph = {cli: ({mid}, set()), mid: (set(), set()), isl: ({mid}, set())}
    members = {cli, mid, isl}

    wired = {cli: ({mid, isl}, set()), mid: (set(), set()), isl: (set(), set())}
    feature_only = {cli: (set(), {"opt"}), "opt": (set(), set())}

    cases = []
    _i, _g, _r, lines, st = evaluate(members, graph, {}, entry)
    cases.append(("uncovered island fails", st == 1 and any("[GAP-R]" in l for l in lines)))
    cases.append(("fix guidance lists all three paths",
                  all(any(hint in l for l in lines) for hint in
                      ("composition root", "optional = true", "crate_reachability_freeze.txt"))))

    _i, _g, _r, lines, st = evaluate(members, graph, {isl: "ADR-000"}, entry)
    cases.append(("frozen island passes", st == 0 and not any("[GAP-R]" in l for l in lines)))

    _i, _g, _r, lines, st = evaluate(members, graph, {isl: "ADR-000", "ghost": "ADR-001"}, entry)
    cases.append(("stale freeze entry fails", st == 1 and any("[STALE]" in l for l in lines)))

    _i, _g, _r, lines, st = evaluate(members, wired, {isl: "ADR-000"}, entry)
    cases.append(("repaid debt warns without blocking",
                  st == 0 and any("[ROTATE]" in l for l in lines)))

    _i, gated, _r, _lines, st = evaluate({cli, "opt"}, feature_only, {}, entry)
    cases.append(("feature-gated crate is not an island", st == 0 and "opt" in gated))

    failed = [name for name, passed in cases if not passed]
    for name, passed in cases:
        print(f"[SELFTEST] {'ok  ' if passed else 'FAIL'} {name}")
    print("[SELFTEST] %s" % ("all ratchet semantics verified" if not failed
                             else f"FAILED: {', '.join(failed)}"))
    return 0 if not failed else 1


def main(argv):
    if "--selftest" in argv:
        return selftest()
    if argv:
        print("usage: check_crate_reachability.py [--selftest]")
        return 2

    members = workspace_members(ROOT)
    graph = build_graph(members, ROOT)
    freeze = read_freeze(FREEZE_PATH)
    islands, gated, reach, lines, status = evaluate(members, graph, freeze, ENTRY_POINTS)

    # LOC 只算一次(43 个 crate 全量遍历), 避免同一 crate 反复走磁盘
    loc = {c: src_loc(c, ROOT) for c in sorted(members)}
    total_loc = sum(loc.values())
    island_loc = sum(loc[c] for c in islands)
    pct = (island_loc * 100.0 / total_loc) if total_loc else 0.0

    print(f"[R] workspace members: {len(members)}, production reachable from "
          f"{'/'.join(ENTRY_POINTS)}: {len(reach)}, unreachable: {len(islands)}")
    print(f"[R] frozen unreachable src lines: {island_loc}/{total_loc} ({pct:.1f}%)")
    for crate in islands:
        print(f"[FREEZE] {crate:<20} src={loc[crate]:<7} {freeze.get(crate, 'UNLISTED')}")
    if gated:
        print(f"[GATED] feature-gated only (compiled by --features, absent from default "
              f"binary): {', '.join(gated)}")
    for line in lines:
        print(line)
    print("")
    if status == 0:
        print("[OK] crate reachability ratchet holds "
              f"(reachable={len(reach)} frozen={len(islands)} new_gaps=0)")
    else:
        print("[FAIL] crate reachability ratchet violated, see [GAP-R]/[STALE] above")
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
