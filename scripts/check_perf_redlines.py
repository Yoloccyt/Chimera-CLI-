#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""性能红线与 bench 清单门禁 —— 单一逻辑实现 (ADR-159 / ADR-161 配套治理工具)。

用法: python scripts/check_perf_redlines.py <mode> [选项]

  lint        Part 1: 11 条红线的存在性静态检查 (文件 / fn / 阈值标记 = 32 子检查)
  inventory   Part 3: crates/*/benches/*.rs 三态清单门 (gated/registered/dev-only) + STALE
  invariants  数据表自证: 阈值键集对等、兜底债棘轮、CI 执行清单与 run_bench 表对等 (I7)
  thresholds  解析 target/criterion/**/new/estimates.json 并断言 36 项阈值
  compare     双跑比对 heredoc 输出(--legacy-out)与 py 核结果, 逐键必须一致
  slo         Part 2: 实跑 criterion bench 并对 80% 红线断言 (需 cargo bench)
  ignored     #[ignore] 测试归属登记门 (slo-blocking/slo-daily/unignore/manual-only)
  emit-ignored 打印尚未登记的 #[ignore] 测试行（供登记表增量维护）
  emit-slo-filter 从归属清单生成 slo-blocking 集的 nextest -E 过滤器表达式
  all         lint + inventory + invariants + ignored (= --static-only, CI 阻塞门用)
  --selftest  夹具注入: 证明门"有牙齿"(未登记必红 / 幽灵登记必红 / 干净集必绿)

选项:
  --criterion-dir PATH  criterion 根 (默认 target/criterion)
  --legacy-out PATH     compare 模式读入的 heredoc stdout 文件
  --json                thresholds 模式输出机器可读结果 (供双跑比对使用)
  --strict              thresholds 模式把关键词匹配歧义也算作失败

WHY 存在这个文件:
  2026-08-31 第四轮冗余审计 F-R4-01 发现: 本门禁原有 .ps1(429 行) + .sh(246 行)
  两份手写实现, 且 **CI 中零真实调用**(唯一命中是 bench_check.yml 的一句注释)。
  同一规则维护两份的漂移已有前科 (check_dependency_rules.sh 停在 38 crate 导致
  依赖铁律 job 实测 EXIT=1)。故本文件是唯一逻辑实现, .ps1/.sh 退化为薄包装,
  表体全部外置到 perf_redlines.toml / perf_thresholds.toml / bench_inventory_freeze.txt。

棘轮与防腐语义:
  - [STALE]  登记表里有、盘上没有 -> FAIL (清单自身要防腐)
  - [UNKNOWN] 盘上有、登记格里没有 -> FAIL (新增/忘登记的 bench 必被捕获)
  - [RATCHET] 兜底债 (calibration_pending) 只减不增

退出码: 0=通过 / 1=违反 / 2=用法或环境错误
输出全部 ASCII, 规避 Windows 控制台 GBK 陷阱 (项目脚本约定)。
"""

import json
import os
import re
import sys

try:
    import tomllib
except ImportError:  # pragma: no cover - 环境门槛, 非业务分支
    print("[FAIL] check_perf_redlines.py requires python >= 3.11 (tomllib)")
    sys.exit(2)

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, ".."))
REDLINES_PATH = os.path.join(HERE, "perf_redlines.toml")
THRESHOLDS_PATH = os.path.join(HERE, "perf_thresholds.toml")
BENCH_FREEZE_PATH = os.path.join(HERE, "bench_inventory_freeze.txt")
IGNORED_FREEZE_PATH = os.path.join(HERE, "ignored_test_inventory_freeze.txt")
# I7 的比对对象: CI 侧真实执行 bench 的清单所在 workflow
BENCH_WORKFLOW_PATH = os.path.join(ROOT, ".github", "workflows", "bench_check.yml")

# criterion 单位 -> 秒的乘数。三态微符号 (U+00B5 / U+03BC / ASCII u) 必须兼容:
# criterion 以 UTF-8 输出 "µs", 若宿主按 GBK 解码会得到 "μs"(U+03BC), 而老脚本
# 只认 "us" -> 落入 else 分支按 ms 处理, 造成 1000 倍误判 (check_perf_redlines.sh
# :178-182 的实测缺陷, 本表是其在 py 侧的归零点)。
UNIT_TO_SEC = {
    "ns": 1e-9,
    "us": 1e-6,
    "\u00b5s": 1e-6,  # µ MICRO SIGN
    "\u03bcs": 1e-6,  # μ GREEK SMALL LETTER MU
    "ms": 1e-3,
    "s": 1.0,
}


def load_toml(path):
    """读取 TOML 并容忍 UTF-8 BOM (同 check_crate_reachability.py 的口径)。"""
    if not os.path.exists(path):
        print(f"[FAIL] missing data file: {os.path.relpath(path, ROOT)}")
        sys.exit(2)
    with open(path, "rb") as fh:
        return tomllib.loads(fh.read().decode("utf-8-sig"))


def read_pipe_table(path):
    """解析 `crate|key|state|reason` 登记表; 返回 {(crate, key): (state, reason)}。

    行格式固定 4 字段 (reason 内允许再出现 '|', 原样保留); 缺文件返回空表,
    由各 mode 自行决定"清单不存在"是 FAIL 还是容忍。
    """
    rows = {}
    if not os.path.exists(path):
        return rows
    with open(path, encoding="utf-8-sig") as fh:
        for raw in fh:
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            parts = [p.strip() for p in line.split("|")]
            if len(parts) < 4:
                print(f"[FAIL] malformed row in {os.path.basename(path)}: {line}")
                print("       expected: crate|key|state|reason")
                sys.exit(2)
            state, reason = parts[2], "|".join(parts[3:])
            rows[(parts[0], parts[1])] = (state, reason)
    return rows


# --------------------------------------------------------------------------- 清单枚举
def bench_inventory():
    """枚举 crates/*/benches/*.rs -> [(crate, bench_stem, 相对路径)]。

    相对路径统一用 '/' 分隔, 与 perf_redlines.toml 的 file 字段可直接比对
    (PowerShell 侧原实现用 Substring + -replace '\\','/' 做同一件事)。
    """
    crates_dir = os.path.join(ROOT, "crates")
    out = []
    for crate in sorted(os.listdir(crates_dir)):
        benches = os.path.join(crates_dir, crate, "benches")
        if not os.path.isdir(benches):
            continue
        for dirpath, _dirs, files in os.walk(benches):
            for name in sorted(files):
                if not name.endswith(".rs"):
                    continue
                rel = os.path.relpath(os.path.join(dirpath, name), ROOT).replace(os.sep, "/")
                out.append((crate, name[:-3], rel))
    return out


def classify(inv, redlines, thresholds_doc, freeze):
    """把每个 bench 文件归入四态, 判定序与语义与退役中的 .ps1 Part 3 完全一致。

    gated      <- perf_redlines.toml 的 redline.file 或 slo.(crate,bench_file)
    registered <- perf_thresholds.toml 的 [[run_bench]] (即 CI 真实执行清单)
    dev-only   <- bench_inventory_freeze.txt 显式登记
    unknown    <- 以上皆非 -> FAIL
    """
    part1_files = {r["file"] for r in redlines.get("redline", [])}
    part2_pairs = {(s["crate"], s["bench_file"]) for s in redlines.get("slo", [])}
    reg_pairs = {(rb["crate"], rb["bench"]) for rb in thresholds_doc.get("run_bench", [])}

    states = []
    for crate, stem, rel in inv:
        if rel in part1_files or (crate, stem) in part2_pairs:
            state = "gated"
        elif (crate, stem) in reg_pairs:
            state = "registered"
        elif (crate, stem) in freeze:
            state = "dev-only"
        else:
            state = "unknown"
        states.append((crate, stem, rel, state))
    return states, part1_files, part2_pairs, reg_pairs


# --------------------------------------------------------------------------- Part 1
def mode_lint(redlines):
    """11 条红线 x (文件 / fn / 阈值标记)。阈值标记为空的 spec-only 红线记 WARN。"""
    print("=== Part 1: static red-line existence lint ===")
    fails = warns = executed = 0
    for rl in redlines.get("redline", []):
        path = os.path.join(ROOT, rl["file"])
        print(f"\n  [{rl['id']}] {rl['name']}")
        exists = os.path.isfile(path)
        executed += 1
        if not _check(f"{rl['id']}.1 file exists ({rl['file']})", exists):
            fails += 1
            print("         skip fn/threshold checks (file missing)")
            continue
        with open(path, encoding="utf-8-sig", errors="replace") as fh:
            content = fh.read()
        executed += 1
        if not _check(f"{rl['id']}.2 function 'fn {rl['func']}' exists",
                      f"fn {rl['func']}" in content):
            fails += 1
        if rl["threshold"]:
            executed += 1
            if not _check(f"{rl['id']}.3 threshold marker '{rl['threshold']}' exists",
                          rl["threshold"] in content):
                fails += 1
        else:
            warns += 1
            print(f"  [WARN] {rl['id']}.3 threshold marker: spec-only red line, "
                  "no in-code assertion")
    print(f"\n  summary: {executed - fails}/{executed} sub-checks passed, {warns} warn")
    return fails


def _check(name, ok):
    print(f"  [{'PASS' if ok else 'FAIL'}] {name}")
    return ok


# --------------------------------------------------------------------------- Part 3
def mode_inventory(redlines, thresholds_doc, freeze):
    """三态清单门 + 反向 STALE 检查 (登记表有、盘上没有 -> FAIL)。"""
    print("=== Part 3: bench inventory completeness gate ===")
    inv = bench_inventory()
    states, _p1, _p2, reg_pairs = classify(inv, redlines, thresholds_doc, freeze)
    counts = {"gated": 0, "registered": 0, "dev-only": 0, "unknown": 0}
    fails = 0
    on_disk = set()
    for crate, stem, _rel, state in states:
        counts[state] += 1
        on_disk.add((crate, stem))
        if state == "unknown":
            fails += 1
            print(f"  [FAIL] inventory unknown    {crate}/{stem} "
                  "- register it as run_bench or add to bench_inventory_freeze.txt")
        else:
            print(f"  [PASS] inventory {state:<10} {crate}/{stem}")

    # 反向防腐: registered / dev-only 条目指向已消失的文件时必红。
    # WHY: 只查"盘上有无登记"是单向的; 删 bench 不删登记会让清单虚胖
    #      (check_crate_reachability.py 的 [STALE] 同款语义)。
    for pair in sorted(reg_pairs - on_disk):
        fails += 1
        print(f"  [STALE] registered entry has no bench file: {pair[0]}/{pair[1]}")
    for pair in sorted(set(freeze) - on_disk):
        fails += 1
        print(f"  [STALE] dev-only entry has no bench file: {pair[0]}/{pair[1]}")

    print(f"\n  counts: total={len(states)} gated={counts['gated']} "
          f"registered={counts['registered']} dev-only={counts['dev-only']} "
          f"unknown={counts['unknown']}")
    if fails:
        print(f"  RESULT: FAIL ({fails} finding(s))")
    else:
        print("  RESULT: PASS (every bench explicitly guarded)")
    return fails


# --------------------------------------------------------------------------- 自证
def mode_invariants(redlines, thresholds_doc, freeze):
    """数据表自身的不变式 —— 把原先靠注释维持的"两表同步"变成机械断言。"""
    print("=== Data-table self-assertions ===")
    fails = 0
    thr = thresholds_doc.get("thresholds_ns", {})
    kw = thresholds_doc.get("path_keywords", {})
    ratchet = thresholds_doc.get("ratchet", {})

    # I1 阈值键集 == 关键词键集 (原实测 36/36)
    missing_kw = set(thr) - set(kw)
    missing_thr = set(kw) - set(thr)
    if _check(f"I1 thresholds_ns keys == path_keywords keys ({len(thr)}/{len(kw)})",
              not missing_kw and not missing_thr):
        pass
    else:
        fails += 1
        for k in sorted(missing_kw):
            print(f"         no path_keywords entry for threshold key: {k}")
        for k in sorted(missing_thr):
            print(f"         no thresholds_ns entry for keyword key: {k}")

    # I2 关键词互为子串的重叠对数只减不增 (F-R4-04 的棘轮化)
    overlaps = keyword_overlaps(kw)
    baseline = ratchet.get("max_keyword_overlaps")
    if baseline is None:
        fails += 1
        print(f"  [FAIL] I2 ratchet baseline missing; pin "
              f"max_keyword_overlaps = {len(overlaps)} in [ratchet]")
    elif not _check(f"I2 keyword substring overlaps {len(overlaps)} <= {baseline}",
                    len(overlaps) <= int(baseline)):
        fails += 1
        for a, b in overlaps:
            print(f"         overlap: {a} in {b}")

    # I3 registered 双向等价 (由 inventory 的 STALE 检查覆盖, 此处断言集合非空)
    reg_pairs = {(rb["crate"], rb["bench"]) for rb in thresholds_doc.get("run_bench", [])}
    if not _check(f"I3 run_bench list non-empty and pair-unique ({len(reg_pairs)})",
                  reg_pairs):
        fails += 1

    # I7 CI 执行清单 == [[run_bench]] 表 (把 bench_check.yml 的"人工同步注释"变成机器断言)
    ci_problems = ci_runbench_sync(BENCH_WORKFLOW_PATH, thresholds_doc.get("run_bench", []))
    if not _check(f"I7 bench_check.yml run_bench lines == [[run_bench]] table "
                  f"({len(reg_pairs)} pairs)", not ci_problems):
        fails += 1
        for p in ci_problems:
            print(f"         {p}")

    # I4 兜底债棘轮: 列表只减不增, 且列表内每项当前值必须仍是 100ms 兜底
    pending = set(ratchet.get("calibration_pending", []))
    cap = int(ratchet.get("max_calibration_pending", len(pending)))
    if not _check(f"I4a calibration_pending {len(pending)} <= {cap}", len(pending) <= cap):
        fails += 1
    stale_pending = {k for k in pending if thr.get(k) != 100_000_000}
    if not _check("I4b every calibration_pending entry still has the 100ms fallback value",
                  not stale_pending):
        fails += 1
        for k in sorted(stale_pending):
            print(f"         {k} was tightened but is still listed as pending debt "
                  "- remove it from calibration_pending")

    # I5 11 条红线 + 7 条 SLO 的规模下限 (防止误删表体把门禁掏空)
    n_rl = len(redlines.get("redline", []))
    n_slo = len(redlines.get("slo", []))
    if not _check(f"I5 redline/slo tables populated ({n_rl} rl / {n_slo} slo)",
                  n_rl >= 11 and n_slo >= 7):
        fails += 1
    if not _check(f"I6 dev-only registry populated ({len(freeze)})", len(freeze) >= 70):
        fails += 1
    verdict = f"FAIL ({fails} violation(s))" if fails else "PASS (all invariants hold)"
    print(f"\n  RESULT: {verdict}")
    return fails


# CI 侧执行清单行的形状: run_bench <alias> cargo bench -p <crate> --bench <bench> [filter...]
# 行尾允许额外 nextest/benchfilter 参数(实测 scheduler 行带 `scheduler_dequeue`)。
# re.M 不可省: 缺它则 `^` 只在字符串开头成立, 31 行只匹到第 1 行 —— 负对照 10b
# (CI 多出必红) 当场判红 got=0, 没让这条不变式静默变成假绿。
CI_RUNBENCH_RE = re.compile(r"^\s*run_bench\s+(\S+)\s+cargo bench -p (\S+) --bench (\S+)", re.M)


def ci_runbench_sync(workflow_path, run_bench_entries):
    """返回 I7 的分歧列表(空 = 对等)。

    WHY 需要它: 该同步此前只靠 workflow 里一行注释维持, 而那行注释指向的
    `check_perf_redlines.ps1 Part3 $registeredBench` 已随 T4 薄包装化消失(实测
    .ps1/.sh/.py 三处 0 次提及), 即"同步提示"本身已经失效却仍写着 —— 与 RK-P30
    "写了不跑"同族。机器断言后, 新增/改名 bench 只登记表而漏接 CI(或反之)立刻红。
    """
    if not os.path.exists(workflow_path):
        return [f"workflow not found: {os.path.relpath(workflow_path, ROOT)}"]
    with open(workflow_path, encoding="utf-8-sig") as fh:
        text = fh.read()
    ci = {(m.group(2), m.group(3)): m.group(1) for m in CI_RUNBENCH_RE.finditer(text)}
    table = {(e.get("crate"), e.get("bench")): e.get("alias") for e in run_bench_entries}
    problems = [f"run by CI but absent from [[run_bench]]: {k}" for k in sorted(set(ci) - set(table))]
    problems += [f"registered in [[run_bench]] but never run by CI: {k}" for k in sorted(set(table) - set(ci))]
    problems += [f"alias drift: {k} ci={ci[k]} table={table[k]}"
                 for k in sorted(set(ci) & set(table)) if ci[k] != table[k]]
    return problems


def keyword_overlaps(kw):
    """返回重叠键对列表: 两键的关键词集存在**任意方向**的真子串关系。

    WHY 双向: 子串关系的危险在于"路径同时匹配两键"(匹配循环 hit 即 break),
    与哪个键更长无关。单向判定会漏掉 (长键, 短键) 这一半序对 ——
    实测例: diff_engine_bench 的 "diff_incremental_5pct" 包含 writer_ansi
    的 "incremental_5pct", 只查短包长则整对漏检, 门禁自证变成假绿。
    """
    out = []
    items = sorted(kw.items())
    for i, (ka, vas) in enumerate(items):
        for kb, vbs in items[i + 1:]:
            hit = any(
                a != b and (a in b or b in a)
                for a in vas
                for b in vbs
            )
            if hit:
                out.append((ka, kb))
    return out


# --------------------------------------------------------------------------- thresholds
LEGACY_ASSERT_RE = re.compile(r"^\s+(\S+): ([\u2713\u2717]) max median = (.+) \(threshold")
LEGACY_WORST_RE = re.compile(r"^\s+worst group: (.+)$")


def collect_threshold_results(thresholds_doc, criterion_dir):
    """跑一遍关键词匹配, 返回 {key: {max_median_ns, worst, threshold_ns, passed}}。

    first-match-wins 与 heredoc 逐字等价(PATH_KEYWORDS 文档序 = TOML 写入序),
    这是 R1-T6 双跑比对能成立的前提。
    """
    thr = thresholds_doc.get("thresholds_ns", {})
    kw = thresholds_doc.get("path_keywords", {})
    results = {name: [] for name in thr}
    unmatched, ambiguous = [], []
    for estimates in sorted(_walk_estimates(criterion_dir)):
        rel = os.path.relpath(estimates, criterion_dir).replace(os.sep, "/")
        try:
            with open(estimates, encoding="utf-8") as fh:
                median_ns = json.load(fh)["median"]["point_estimate"]
        except (json.JSONDecodeError, KeyError) as exc:
            print(f"[WARN] failed to parse {estimates}: {exc}")
            continue
        hits = [name for name, kws in kw.items() if any(k in rel for k in kws)]
        if len(hits) > 1:
            ambiguous.append((rel, hits))
        if hits:
            results[hits[0]].append((rel, median_ns))  # first-match-wins (legacy 等价)
        else:
            unmatched.append((rel, median_ns))

    payload = {}
    for name, limit in thr.items():
        rows = results[name]
        if not rows:
            payload[name] = {"max_median_ns": None, "worst": None,
                             "threshold_ns": limit, "passed": None}
            continue
        worst_path, worst_ns = max(rows, key=lambda x: x[1])
        payload[name] = {"max_median_ns": round(worst_ns, 6), "worst": worst_path,
                         "threshold_ns": limit, "passed": worst_ns < limit}
    return payload, unmatched, ambiguous


def parse_legacy_threshold_log(path):
    """解析 heredoc 的 stdout: {key: (passed, worst_path)}。

    legacy 对"无 estimates"的键只打 ::warning:: 行而不进结果集, 故这里也不记。
    """
    res = {}
    with open(path, encoding="utf-8-sig", errors="replace") as fh:
        lines = fh.read().splitlines()
    for i, line in enumerate(lines):
        m = LEGACY_ASSERT_RE.match(line)
        if not m:
            continue
        worst = ""
        if i + 1 < len(lines):
            m2 = LEGACY_WORST_RE.match(lines[i + 1])
            if m2:
                worst = m2.group(1).strip()
        res[m.group(1)] = (m.group(2) == "\u2713", worst)
    return res


def mode_compare(thresholds_doc, criterion_dir, legacy_path):
    """双跑比对: heredoc 结果 vs py 核结果逐键必须一致 (R1-T6 切流前置门)。

    WHY 固化到 CI: 等价性只在本地证过一次就删 heredoc, 等于把双实现漂移风
    险重新埋回去(F-R4-01 的病根)。在删 heredoc 前让 CI 每日跑这个比对。
    """
    print("=== double-run comparison: heredoc vs py core ===")
    if not os.path.isfile(legacy_path):
        print(f"[FAIL] legacy log not found: {legacy_path}")
        return 1
    legacy = parse_legacy_threshold_log(legacy_path)
    payload, _unmatched, _ambiguous = collect_threshold_results(thresholds_doc, criterion_dir)
    fails = 0
    checked = 0
    fail_verdicts = 0
    for key, (l_passed, l_worst) in sorted(legacy.items()):
        pv = payload.get(key)
        if pv is None or pv["passed"] is None:
            fails += 1
            print(f"  [DIFF] {key}: legacy asserted but py has no estimates")
            continue
        checked += 1
        if not l_passed:
            fail_verdicts += 1
        same_verdict = l_passed == pv["passed"]
        same_worst = l_worst.replace("\\", "/") == (pv["worst"] or "").replace("\\", "/")
        if not (same_verdict and same_worst):
            fails += 1
            print(f"  [DIFF] {key}: legacy=(passed={l_passed}, {l_worst}) "
                  f"py=(passed={pv['passed']}, {pv['worst']})")
    print(f"\n  compared: {checked} key(s) with estimates on both sides; "
          f"breached keys exercised: {fail_verdicts}")
    if checked == 0:
        print("  [FAIL] nothing to compare - the proof would be vacuous "
              "(bench step produced no estimates?)")
        fails += 1
    if fails:
        print(f"  RESULT: FAIL ({fails} divergence(s)) - heredoc must NOT be removed yet")
        return 1
    print("  RESULT: PASS (py core is behaviour-identical to the heredoc)")
    return 0


def mode_thresholds(thresholds_doc, criterion_dir, as_json, strict):
    """解析 criterion estimates.json 并断言阈值 (bench_check.yml heredoc 的等价移植)。

    迁移期纪律: 默认保持与 heredoc **逐键等价**的 first-match-wins 语义, 以便
    R1-T6 的"双跑比对"能证明零行为变更; 匹配歧义只在 strict 模式下才致命。
    """
    thr = thresholds_doc.get("thresholds_ns", {})
    if not os.path.isdir(criterion_dir):
        print(f"[FAIL] criterion dir not found: {criterion_dir} "
              "(benchmarks may have failed)")
        return 1

    payload, unmatched, ambiguous = collect_threshold_results(thresholds_doc, criterion_dir)
    fails = 0
    print("=== Benchmark threshold assertions ===")
    for name, limit in thr.items():
        row = payload[name]
        if row["passed"] is None:
            print(f"  [SKIP] {name}: no estimates found")
            continue
        worst_ns, worst_path = row["max_median_ns"], row["worst"]
        passed = row["passed"]
        print(f"  [{'PASS' if passed else 'FAIL'}] {name}: max median = "
              f"{_fmt_ns(worst_ns)} (threshold < {_fmt_ns(limit)})")
        print(f"         worst group: {worst_path}")
        if not passed:
            fails += 1

    if unmatched:
        print(f"\n  {len(unmatched)} estimates path(s) matched no keyword (diagnostics):")
        for rel, median in unmatched[:20]:
            print(f"    {rel}: {_fmt_ns(median)}")
    if ambiguous:
        print(f"\n  [AMBIG] {len(ambiguous)} estimates path(s) matched >1 threshold key:")
        for rel, hits in ambiguous[:20]:
            print(f"    {rel} -> {hits}")
        if strict:
            fails += len(ambiguous)
            print("  strict mode: ambiguity is fatal")
        else:
            print("  non-fatal by design (migration must be behaviour-preserving); "
                  "fix the keyword sets instead of flipping this switch")

    if as_json:
        print("\n__JSON__")
        print(json.dumps(payload, sort_keys=True))
    return fails


def _walk_estimates(root_dir):
    """枚举 <root>/**/new/estimates.json，**按路径字典序**返回。

    WHY 显式排序：os.walk 的返回顺序跟文件系统目录项有关，跨机/跨文件系统不稳定。
    当同一阈值的两个 group 中位数**完全相等**时，`max()` 取先遇到的那个，
    worst path 就会随遍历顺序飘 —— 本会话实测到过（注入两个相同值后，
    heredoc 的 glob 与 py 核的 os.walk 选出不同 worst path，双跑报 divergence）。
    排序不修正与 heredoc 的平局差异（那需要改 heredoc，而它正待退役），
    但保证 **py 核自身可复现**，并把双跑分歧的可能收敛到“真实浮点完全相等”这一几乎不发生的场景。
    """
    found = []
    for dirpath, _dirs, files in os.walk(root_dir):
        if os.path.basename(dirpath) == "new" and "estimates.json" in files:
            found.append(os.path.join(dirpath, "estimates.json"))
    return sorted(found)


def _fmt_ns(ns):
    if ns < 1_000:
        return f"{ns:.0f} ns"
    if ns < 1_000_000:
        return f"{ns / 1_000:.2f} us"
    if ns < 1_000_000_000:
        return f"{ns / 1_000_000:.2f} ms"
    return f"{ns / 1_000_000_000:.2f} s"


# --------------------------------------------------------------------------- Part 2 (SLO)
def mode_slo(redlines):
    """实跑 criterion bench, 解析 'time: [lo est up unit]' 并对 80% 红线断言。

    与退役中的 .ps1 Part 2 语义一致, 但单位解析改走 UNIT_TO_SEC 表
    (消除 .sh 侧只认 'us' 的 1000 倍误判)。
    """
    import subprocess

    print("=== Part 2: SLO bench assertions (80% of SLO as CI redline) ===")
    fails = skipped = 0
    for slo in redlines.get("slo", []):
        print(f"\n  [SLO] {slo['name']} (target < {slo['slo_display']}, "
              f"redline {slo['redline_display']})")
        cmd = ["cargo", "bench", "--package", slo["crate"], "--bench",
               slo["bench_file"], "--", "--noplot", "--quick", slo["filter"]]
        proc = subprocess.run(cmd, capture_output=True, text=True,
                              encoding="utf-8", errors="replace", cwd=ROOT)
        out = (proc.stdout or "") + (proc.stderr or "")
        m = re.search(r"time:\s+\[([^\]]+)\]", out)
        if not m:
            skipped += 1
            print("    [SKIP] no criterion time output "
                  f"(cargo exit={proc.returncode}; bench may have failed to compile)")
            continue
        vals = m.group(1).split()
        if len(vals) < 4:
            skipped += 1
            print(f"    [SKIP] unparsable criterion line: {m.group(1)!r}")
            continue
        estimate, unit = float(vals[1]), vals[3]
        if unit not in UNIT_TO_SEC:
            skipped += 1
            print(f"    [SKIP] unknown unit {unit!r} - extend UNIT_TO_SEC deliberately")
            continue
        est_sec = estimate * UNIT_TO_SEC[unit]
        print(f"    measured: {estimate:g} {unit} = {est_sec:.9f} s")
        if est_sec <= slo["redline_sec"]:
            print("    [PASS] below redline")
        elif est_sec <= slo["slo_sec"]:
            print("    [PASS] above redline but within SLO")
        else:
            fails += 1
            print(f"    [FAIL] exceeds SLO ({slo['slo_display']})")
    print(f"\n  summary: failed={fails} skipped={skipped}")
    return fails


# --------------------------------------------------------------------------- ignored 归属门
IGN_ATTR_RE = re.compile(r"^\s*#\[ignore", re.M)
FN_RE = re.compile(r"\bfn\s+([A-Za-z_]\w*)")
# 登记表合法态集合: 写错 state 不能默默当作已登记 —— 否则一个 typo 就能把守护掉。
IGNORED_STATES = {"slo-blocking", "slo-daily", "unignore-target", "manual-only"}


def ignored_inventory():
    """枚举 (crate, 相对路径, 测试函数名) 三元组, 覆盖 src / tests / benches 三处。

    WHY 连 benches 也扫: criterion harness=false 下 `#[ignore]` 是惰性属性
    (不会被 cargo bench 尊重), 但"写了 ignore 却无人跑"的事实必须被登记而不是
    被遗忘 —— 本门要求这类条目显式写 manual-only。
    """
    out = []
    crates_dir = os.path.join(ROOT, "crates")
    for crate in sorted(os.listdir(crates_dir)):
        cdir = os.path.join(crates_dir, crate)
        if not os.path.isdir(cdir):
            continue
        for sub in ("src", "tests", "benches"):
            base = os.path.join(cdir, sub)
            if not os.path.isdir(base):
                continue
            for dirpath, _dirs, files in os.walk(base):
                for name in sorted(files):
                    if not name.endswith(".rs"):
                        continue
                    path = os.path.join(dirpath, name)
                    with open(path, encoding="utf-8-sig", errors="replace") as fh:
                        text = fh.read()
                    for attr in IGN_ATTR_RE.finditer(text):
                        tail = text[attr.end():]
                        fn = FN_RE.search(tail[:2000])
                        if not fn:
                            continue
                        rel = os.path.relpath(path, ROOT).replace(os.sep, "/")
                        out.append((crate, rel, fn.group(1)))
    return out


def mode_ignored(freeze_path=IGNORED_FREEZE_PATH):
    """每个 #[ignore] 测试都必须落在登记态里; 未登记 -> FAIL。

    登记粒度: 精确到 `crate|相对路径#函数名`, 或整文件 `crate|相对路径`
    (函数级条目优先, 覆盖文件级默认)。这是"不跑必须是被登记的决策"的兑现。
    """
    print("=== #[ignore] ownership gate ===")
    registry = read_pipe_table(freeze_path)
    items = ignored_inventory()
    if not registry:
        print(f"[FAIL] registry empty or missing: {os.path.basename(freeze_path)}")
        return 1
    fails = 0
    counts = {}
    for crate, rel, fn in items:
        exact, broad = (crate, f"{rel}#{fn}"), (crate, rel)
        state = None
        if exact in registry:
            state = registry[exact][0]
        elif broad in registry:
            state = registry[broad][0]
        counts[state or "unregistered"] = counts.get(state or "unregistered", 0) + 1
        if state is None:
            fails += 1
            print(f"  [FAIL] unregistered ignored test: {crate} {rel}::{fn}")
        elif state not in IGNORED_STATES:
            fails += 1
            print(f"  [FAIL] unknown state {state!r}: {crate} {rel}::{fn}")
    # 反向防腐: 登记条目指向已不存在的测试 (改名 / 删除测试须同步清单)
    present = {(c, f"{r}#{fn}") for c, r, fn in items} | {(c, r) for c, r, _ in items}
    for key in sorted(set(registry) - present):
        fails += 1
        print(f"  [STALE] registry entry has no matching test: {key[0]} {key[1]}")

    # 棘轮: unignore-target 是已判定但尚未执行的欠债, 只减不增。
    # WHY 需要这个: 登记"应解除"很容易, 真去解除很费事; 没有上限就变成新的
    # “写在清单里就算做过”。上限值记在清单头的 `# ratchet max_unignore_pending = N`。
    cap = read_ratchet_directive(freeze_path, "max_unignore_pending")
    pending = counts.get("unignore-target", 0)
    if cap is None:
        fails += 1
        print("  [FAIL] ratchet directive max_unignore_pending missing from registry head")
    elif not _check(f"ratchet unignore-target {pending} <= {cap}", pending <= int(cap)):
        fails += 1
    print("\n  state counts: " + ", ".join(f"{k}={v}" for k, v in sorted(counts.items())))
    print(f"  total ignored tests: {len(items)}")
    print(f"  RESULT: {'FAIL' if fails else 'PASS'}")
    return fails


def read_ratchet_directive(path, key):
    """从清单头部读 `# ratchet <key> = <value>` 指令; 无则返回 None。"""
    pat = re.compile(rf"^#\s*ratchet\s+{re.escape(key)}\s*=\s*(\d+)\s*$")
    if not os.path.exists(path):
        return None
    with open(path, encoding="utf-8-sig") as fh:
        for raw in fh:
            m = pat.match(raw.strip())
            if m:
                return int(m.group(1))
    return None


# --------------------------------------------------------------------------- selftest
def mode_selftest():
    """夹具注入 —— 证明门禁有牙齿, 而不是第二个"写了不跑"。

    三条断言: 未登记 bench 必红 / 幽灵登记必红 / 干净集必绿。
    全部在临时目录里做, 不触碰真实仓库文件。
    """
    import tempfile
    import shutil

    print("=== selftest: the gate must have teeth ===")
    fails = 0
    n_assert = 0
    tmp = tempfile.mkdtemp(prefix="perf_gate_selftest_")
    try:
        fake = os.path.join(tmp, "crates", "fakecrate", "benches")
        os.makedirs(fake)
        with open(os.path.join(fake, "clean_bench.rs"), "w", encoding="utf-8") as fh:
            fh.write("fn main() {}\n")
        inv = [("fakecrate", "clean_bench", "crates/fakecrate/benches/clean_bench.rs")]

        # 夹具构造器：一条 run_bench 清单 -> 一份自洽的阈值文档。
        # I5/I6 的规模下限用 skip_scale 绕过，否则每条断言都会被它们抵消。
        rl_gated = {"redline": [], "slo": [{"crate": "fakecrate",
                                            "bench_file": "clean_bench"}]}
        rl_none = {"redline": [], "slo": []}

        def mk_td(pairs):
            return {"thresholds_ns": {"k": 1000}, "path_keywords": {"k": ["k"]},
                    "ratchet": {"max_keyword_overlaps": 0, "calibration_pending": [],
                                "max_calibration_pending": 0},
                    "run_bench": [{"crate": c, "bench": b, "alias": b} for c, b in pairs]}

        def expect(name, got, want):
            nonlocal fails
            nonlocal n_assert
            n_assert += 1
            if _check(f"{name} (got={got} want={want})", got == want):
                return
            fails += 1

        # 断言 1：盘上有、登记无 -> unknown 必被识别
        st, _p1, _p2, _reg = classify(inv, rl_none, mk_td([]), {})
        expect("selftest-1 unregistered bench is classified unknown",
               st[0][3], "unknown")

        # 断言 2：登记了但盘上没有 -> STALE 必红
        rc = _capture(inventory_capturing, inv, rl_gated, mk_td([("fakecrate", "ghost")]),
                      {}, True)
        expect("selftest-2 ghost run_bench entry is fatal", rc, 1)

        # 断言 3：dev-only 登记指向已删文件 -> STALE 必红
        rc = _capture(inventory_capturing, inv, rl_gated, mk_td([]),
                      {("fakecrate", "gone"): ("dev-only", "stale row")}, True)
        expect("selftest-3 ghost dev-only row is fatal", rc, 1)

        # 断言 4：完全受守护的集 -> 必绿（防空转：红绿都得能出）
        rc = _capture(inventory_capturing, inv, rl_gated, mk_td([]), {}, True)
        expect("selftest-4 fully-guarded set is green", rc, 0)

        # 断言 5：关键词互为子串双向可检（路径串台的根源）
        expect("selftest-5a overlap short-in-long",
               len(keyword_overlaps({"a": ["mixed"], "b": ["128_mixed"]})), 1)
        expect("selftest-5b overlap long-in-short (reverse iteration order)",
               len(keyword_overlaps({"c": ["diff_incremental_5pct"],
                                     "d": ["incremental_5pct"]})), 1)
        expect("selftest-5c disjoint keywords are clean",
               len(keyword_overlaps({"a": ["wal_"], "b": ["dag/"]})), 0)

        # 断言 6：100ms 兜底已收紧却仍列债务 -> 必红
        doc = {"thresholds_ns": {"x": 5_000}, "path_keywords": {"x": ["x"]},
               "ratchet": {"max_keyword_overlaps": 0, "calibration_pending": ["x"],
                           "max_calibration_pending": 5}, "run_bench": []}
        rc = _capture(invariants_capturing, rl_scale_ok(), doc, {}, True)
        expect("selftest-6 calibrated-but-still-listed debt is fatal", rc, 1)

        # 断言 7：债务列表增长超上限 -> 必红
        doc2 = {"thresholds_ns": {"x": 100_000_000, "y": 100_000_000},
                "path_keywords": {"x": ["x"], "y": ["y"]},
                "ratchet": {"max_keyword_overlaps": 0,
                            "calibration_pending": ["x", "y"],
                            "max_calibration_pending": 1}, "run_bench": []}
        rc = _capture(invariants_capturing, rl_scale_ok(), doc2, {}, True)
        expect("selftest-7 ratchet growth is fatal", rc, 1)

        # 断言 8：阈值键集与关键词键集不等 -> 必红（新 bench 只建一张表的典型病灶）
        doc3 = {"thresholds_ns": {"x": 1000}, "path_keywords": {},
                "ratchet": {"max_keyword_overlaps": 0, "calibration_pending": [],
                            "max_calibration_pending": 0}, "run_bench": []}
        rc = _capture(invariants_capturing, rl_scale_ok(), doc3, {}, True)
        expect("selftest-8 key-set mismatch is fatal", rc, 1)

        # 断言 9：真实表必绿（防止自测只测红路径而把真表测挂）
        real_rl = load_toml(REDLINES_PATH)
        real_td = load_toml(THRESHOLDS_PATH)
        real_fz = read_pipe_table(BENCH_FREEZE_PATH)
        rc = _capture(lambda: mode_lint(real_rl),)
        expect("selftest-9a real lint is green", rc, 0)
        rc = _capture(lambda: mode_inventory(real_rl, real_td, real_fz))
        expect("selftest-9b real inventory is green", rc, 0)
        rc = _capture(lambda: mode_invariants(real_rl, real_td, real_fz))
        expect("selftest-9c real invariants are green", rc, 0)
        rc = _capture(lambda: mode_ignored())
        expect("selftest-9d real ignored-ownership gate is green", rc, 0)
        # 断言 10：I7 的五种形态（干净/CI 多出/表多出/alias 漂/文件缺失）
        # 前四种用临时 workflow 文件, 不碰真仓的 bench_check.yml。
        def wf_text(rows, tag):
            p = os.path.join(tmp, f"wf_{tag}.yml")
            with open(p, "w", encoding="utf-8") as fh:
                fh.write("".join(f"          run_bench {a} cargo bench -p {c} --bench {b}\n"
                                 for c, b, a in rows))
            return p

        rb1 = [{"crate": "c1", "bench": "b1", "alias": "a1"}]
        expect("selftest-10a I7 clean when CI list matches table",
               ci_runbench_sync(wf_text([("c1", "b1", "a1")], "clean"), rb1), [])
        expect("selftest-10b I7 fatal when a bench runs in CI unregistered",
               len(ci_runbench_sync(wf_text([("c1", "b1", "a1"), ("c2", "b2", "a2")],
                                            "extra_ci"), rb1)), 1)
        expect("selftest-10c I7 fatal when a registered bench is not run by CI",
               len(ci_runbench_sync(wf_text([], "empty"), rb1)), 1)
        expect("selftest-10d I7 fatal on alias drift",
               len(ci_runbench_sync(wf_text([("c1", "b1", "OTHER")], "alias"), rb1)), 1)
        expect("selftest-10e I7 fatal when the workflow file is missing",
               len(ci_runbench_sync(os.path.join(tmp, "nope.yml"), rb1)), 1)

        # 断言 11：真实 bench_check.yml 与真表对等（I7 在产环境必绿，与自测红路径互补）
        expect("selftest-11 real bench_check.yml is in sync with [[run_bench]]",
               ci_runbench_sync(BENCH_WORKFLOW_PATH, load_toml(THRESHOLDS_PATH)["run_bench"]), [])
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    # 断言条数自报：文档不得写死这个数字（写死 = 下一道“写了不跑”与跨文档漂移）
    verdict = (f"PASS (all {n_assert} assertions held)" if not fails
               else f"FAIL ({fails}/{n_assert} assertion(s) violated)")
    print(f"\n  RESULT: {verdict}")
    return fails


def rl_scale_ok():
    """自测夹具：满足 I5 规模下限的最小红线表。"""
    return {"redline": [{"id": f"RL-{i:02d}"} for i in range(1, 12)],
            "slo": [{"crate": "c", "bench_file": "b"} for _ in range(7)]}


def inventory_capturing(inv, rl, td, fz, skip_scale=False):
    """mode_inventory 的可注入版本（selftest 用），判定与真实模式同构。"""
    states, _p1, _p2, reg = classify(inv, rl, td, fz)
    on_disk = {(c, s) for c, s, _r, _st in states}
    bad = sum(1 for st in states if st[3] == "unknown")
    bad += len(set(reg) - on_disk) + len(set(fz) - on_disk)
    return 1 if bad else 0


def invariants_capturing(rl, td, fz, skip_scale=False):
    """mode_invariants 的可注入版本（selftest 用），逻辑与真实模式同构。"""
    thr, kw = td.get("thresholds_ns", {}), td.get("path_keywords", {})
    r = td.get("ratchet", {})
    bad = 0
    if set(thr) != set(kw):
        bad += 1
    if len(keyword_overlaps(kw)) > int(r.get("max_keyword_overlaps", -1)):
        bad += 1
    pending = set(r.get("calibration_pending", []))
    if len(pending) > int(r.get("max_calibration_pending", len(pending))):
        bad += 1
    if any(thr.get(k) != 100_000_000 for k in pending):
        bad += 1
    if not skip_scale:
        if len(rl.get("redline", [])) < 11 or len(rl.get("slo", [])) < 7:
            bad += 1
        if len(fz) < 70:
            bad += 1
    return 1 if bad else 0


def _capture(fn, *args):
    """静默执行 fn (selftest 不打扰真实输出)。"""
    import contextlib
    import io

    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        return fn(*args)


# --------------------------------------------------------------------------- main
VALUE_FLAGS = ("--criterion-dir", "--legacy-out")


def main(argv):
    flags = {a for a in argv if a.startswith("-")}
    # --help 必须显式返回，不能掉进默认模式：未支持时它会被当作“无模式”而跑完整套件
    # 并以非零退出（本会话实测踩到：调用者会误读为“检查失败”）。
    if {"--help", "-h"} & flags:
        print(__doc__)
        return 2
    if "--selftest" in flags:
        return mode_selftest()

    as_json = "--json" in flags
    strict = "--strict" in flags
    criterion_dir = os.path.join(ROOT, "target", "criterion")
    legacy_out = None
    # 位置参数 = 排除选项名与它们的值。之前只剔了选项名, 值会混进 modes,
    # 靠"取第一个"侥幸不出错; 现在是显式剔除, 不再依赖顺序。
    consumed = set()
    for opt in VALUE_FLAGS:
        if opt in argv:
            i = argv.index(opt)
            if i + 1 >= len(argv):
                print(f"[FAIL] {opt} needs a value")
                return 2
            if opt == "--criterion-dir":
                criterion_dir = argv[i + 1]
            else:
                legacy_out = argv[i + 1]
            consumed.update({i, i + 1})
    modes = [a for idx, a in enumerate(argv)
             if not a.startswith("-") and idx not in consumed]

    mode = modes[0] if modes else "all"
    redlines = load_toml(REDLINES_PATH)
    thresholds_doc = load_toml(THRESHOLDS_PATH)
    freeze = read_pipe_table(BENCH_FREEZE_PATH)

    if mode in ("all", "--static-only", "static"):
        # CI 阻塞门: 全部子检查均为静态(零 cargo 调用、秒级)。ignored 门一并计入:
        # 它防的是同一病灶 —— “写了不跑”从 bench 延伸到测试, 不该只登记不把关。
        return (mode_lint(redlines)
                + mode_inventory(redlines, thresholds_doc, freeze)
                + mode_invariants(redlines, thresholds_doc, freeze)
                + mode_ignored())
    if mode == "lint":
        return mode_lint(redlines)
    if mode == "inventory":
        return mode_inventory(redlines, thresholds_doc, freeze)
    if mode == "invariants":
        return mode_invariants(redlines, thresholds_doc, freeze)
    if mode == "thresholds":
        return mode_thresholds(thresholds_doc, criterion_dir, as_json, strict)
    if mode == "compare":
        if not legacy_out:
            print("[FAIL] compare mode needs --legacy-out <heredoc stdout file>")
            return 2
        return mode_compare(thresholds_doc, criterion_dir, legacy_out)
    if mode == "slo":
        return mode_slo(redlines)
    if mode == "ignored":
        return mode_ignored()
    if mode == "emit-ignored":
        # 只打印现有登记表缺的那些行（新/改名测试），存量的 state 不越推
        registry = read_pipe_table(IGNORED_FREEZE_PATH)
        missing = []
        for crate, rel, fn in ignored_inventory():
            if (crate, f"{rel}#{fn}") not in registry and (crate, rel) not in registry:
                missing.append(f"{crate}|{rel}#{fn}|UNSET|TODO register reason")
        print("\n".join(missing) if missing else "# registry already covers every ignored test")
        print(f"# unregistered: {len(missing)}")
        return 1 if missing else 0
    if mode == "emit-slo-filter":
        # 从归属清单生成 slo-blocking 集的 nextest -E 表达式，让 CI 工文件不再
        # 手写第二份真值源（写死列表 = 清单与 workflow 必漂）。
        #
        # 两个实测约束（nextest 0.9.143，2026-08-31）:
        #   ① `binary(^name$)` 与 `binary(^pkg::name$)` 都零匹配（锚定式跟显示名不同形），
        #      而 `binary(name)` 可匹配；同理只有 `test(<name>)` 不锚定时生效。
        #   ② 因此改用**测试粒度 + 不锚定**，并靠下面的子串碰撞自证保证精确性：
        #      若某 A1 测试名是其他 ignored 测试名的子串，一个算子会多选→必须拒绝生成。
        registry = read_pipe_table(IGNORED_FREEZE_PATH)
        targets = sorted(
            fn for (_crate, key), (state, _r) in registry.items() if state == "slo-blocking"
            for _crate2, fn in [key.split("#")]
        )
        if not targets:
            print("[FAIL] no slo-blocking entries in registry", file=sys.stderr)
            return 1
        all_names = sorted(
            fn for (_c, key), _v in registry.items() if "#" in key for fn in [key.split("#")[1]]
        )
        collisions = [
            (a, b) for a in targets for b in all_names if a != b and a in b
        ]
        if collisions:
            for a, b in collisions:
                print(f"[FAIL] slo-blocking name {a!r} is a substring of {b!r} "
                      "- `test()` filter is unanchored, so it would over-select", file=sys.stderr)
            return 1
        print(" | ".join(f"test({t})" for t in targets))
        print(f"# targets: {len(targets)}, collisions: 0", file=sys.stderr)
        return 0
    if mode == "emit-overlaps":
        print(f"max_keyword_overlaps = {len(keyword_overlaps(thresholds_doc['path_keywords']))}")
        return 0
    print(f"[FAIL] unknown mode: {mode}")
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
