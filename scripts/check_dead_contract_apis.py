#!/usr/bin/env python3
"""check_dead_contract_apis.py -- contract APIs must have production callers (ADR-197 A6).

Why this script exists (evidence, 2026-10-01 audit `cpu-hetero-four-dimension-audit`):
    `cargo test -p nexus-core` was fully green (235 tests) and
    `cargo clippy -p nexus-core --all-targets` emitted zero warnings, while
    three advertised contracts had no production caller at all:
      * `ComputeBridge::update_threshold`  -- only definition + in-file test
      * `ComputeBridge::spawn_compute`     -- only definition + in-file test
      * `ComputeError::Saturated`          -- never constructed
    Public items are exempt from `dead_code`, so neither the compiler nor the
    test suite can see this class of defect. This script is the missing lock.

Semantics:
    WIRED     -- must keep >= 1 production call site; regression to 0 is a FAIL.
    DEFERRED  -- known-not-wired yet (ADR pointer recorded); reported but not a
                 FAIL. When it becomes wired, the script says "promote to WIRED"
                 so the ledger converges instead of silently rotting.

Output is ASCII only (PowerShell 5.1 GBK console safety, project convention).

Exit codes:
    0  every WIRED contract has production callers
    1  at least one WIRED contract regressed to zero (or selftest failed)

Usage:
    python scripts/check_dead_contract_apis.py [--root PATH] [--json] [--selftest]
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path, PurePosixPath

# ---------------------------------------------------------------------------
# Contract ledger
#
# `pattern` matches a *use site* (not the declaration). Declarations are written
# as `pub fn <name>` / `    Saturated,`, so requiring an opening paren / a
# `Err(`-style value position keeps definitions out of the count.
#
# Optional per-row fields (see `scan` for the semantics):
#   `exclude_files`       -- own-module files whose hits move to `self_uses`
#   `self_uses_baseline`  -- expected `self_uses` (permanent artifacts), default 0
# ---------------------------------------------------------------------------
CONTRACTS = [
    {
        "name": "ComputeBridge::spawn_compute_batch",
        "pattern": r"\.spawn_compute_batch\(",
        "status": "WIRED",
        "note": "batch offload points (P1-T14 / WI-34); sole ComputeBridge offload "
                "entry since ADR-199 candidate-B retired spawn_compute (2026-10-03)",
    },
    {
        "name": "ComputeError::Saturated",
        # value position only: `Err(ComputeError::Saturated)` / `== ComputeError::Saturated`
        "pattern": r"ComputeError::Saturated\s*[\)\],;]",
        "status": "WIRED",  # ADR-197 A1: constructed in strict-pool degradation path
        "note": "constructed when pool saturated under strict mode",
    },
    {
        "name": "ComputeBridge::update_threshold",
        "pattern": r"\.update_threshold\(",
        "status": "WIRED",  # ADR-198 B1:由 calibration::apply_pending 在 apply 模式下写表
        "note": "SPRT calibration driver landed (ADR-198); default off/observe means "
                "it is not invoked unless CHIMERA_COMPUTE_CALIBRATION=apply",
    },
    {
        "name": "UtilizationSampler (double-probe)",
        # WHY 无 `\\s*\\(` 强制：生产调用是**无括号的函数路径引用**
        #   （`SAMPLER.get_or_init(UtilizationSampler::default)`，
        #   crates/nexus-core/src/compute/telemetry.rs），而非 `::default()` 调用。
        #   要求括号会让本判据对已落地的 A5 接线报 uses=0 —— 即脚本自身构成它要防的
        #   假绿（审计 finding：判据漏配型）。`\\b` 而非 `\\s*\\(` 既覆盖三种形态
        #   （`::new(` / `::with_probes(` / `::default` 裸路径），又排除
        #   `::defaults()` 之类的近似标识符。
        "pattern": r"UtilizationSampler::(?:new|with_probes|default)\b",
        "status": "WIRED",  # ADR-197 A5 已落地：spawn_saturation_reporter 周期采样
        "note": "probe reaches production via spawn_saturation_reporter (ADR-197 A5); "
                "sampler is constructed as a bare fn path, not a call -- see pattern note",
    },
    {
        "name": "faae::parallel::score_experts_batch",
        # WHY 宽匹配 + `exclude_files`（而不是 `::`-锚定）:
        # `::` 锚定只把"自身声明 + 同胞 wrapper 的两处调用"挡在计数外,却**连同
        # 模块内部的真实改接线一起看不见** —— 那是正则形状的偶然副作用,不是记账
        # 口径(旧版把这条洞写在 note 里"记录在案",实际后果是行永远报 uses=0,
        # 哪怕路径已在生产里被重新接线)。改用「宽匹配看得见 + exclude_files 把它
        # 移出 uses + 同文件命中转入 self_uses 单独记账」:跨模块口径由字段机械
        # 保证,模块内部的增量则变成**有基线的可见数字**。
        # `(?!_offloaded)` 排除同胞 wrapper 的名字(它有自己的行)。
        "pattern": r"\bscore_experts_batch(?!_offloaded)\(",
        # 契约自有模块文件:其命中不计入 uses(跨模块口径),改记入 self_uses。
        "exclude_files": ["faae-router/src/parallel.rs"],
        # self_uses 基线 = 1 处声明 + 2 处 wrapper 内部调用(offloaded 的两分支)。
        "self_uses_baseline": 3,
        "status": "DEFERRED",
        "note": "ScoreInput-based batch: production-dead since the precomputed-Arc path "
                "(P3). Retained ONLY as the pre-existing pub contract (API stability) and "
                "as the reference side of the two-path equivalence tests; the bench no "
                "longer calls it, so it is not a perf proxy. Production hot path is "
                "score_experts_scoring_batch*. uses = cross-module only (exclude_files); "
                "same-module hits are tallied in self_uses. "
                "[T2.4] bench-only 等接线; 接线三要件见 faae-router/src/parallel.rs 模块头; "
                "可达性权威源=G-87 行 faae-router/src/parallel.rs::score_parallel verdict=DEAD.",
    },
    {
        "name": "faae::parallel::score_experts_batch_offloaded",
        # 同上一行:宽匹配看得见同文件调用,exclude_files 把它移出 uses。
        "pattern": r"\bscore_experts_batch_offloaded\(",
        "exclude_files": ["faae-router/src/parallel.rs"],
        "self_uses_baseline": 1,  # 仅 1 处声明
        "status": "DEFERRED",
        "note": "RuntimeFlavor wrapper over the ScoreInput path; also production-dead "
                "since P3. Its multi-thread block_in_place branch is therefore no longer "
                "covered by any test that exercises a multi-thread runtime. "
                "uses = cross-module only (exclude_files). "
                "[T2.4] bench-only 等接线; 接线前须先补回多线程 block_in_place 路径覆盖 "
                "(G-88 保护); 可达性权威源=G-87 行 faae-router/src/parallel.rs::score_parallel verdict=DEAD.",
    },
    {
        "name": "EventBus::shadow_stats",
        # call sites `bus.shadow_stats()` / `self.shadow_stats()`; the
        # `pub fn shadow_stats(` declaration carries no leading dot -> not matched.
        "pattern": r"\.shadow_stats\(",
        "status": "WIRED",
        # Third-round T1.1 closed the loop: `spawn_throughput_reporter` (bus.rs, production
        # scope) now reads `shadow_stats().merge_closed_total` on a window cadence to
        # surface zero-subscriber merge drops -> promoted DEFERRED->WIRED (ratchet: a future
        # removal of that consumer regresses uses to 0 and FAILs, which is correct).
        "note": "shard shed/merged + merge_closed_total distribution. Previously DEFERRED "
                "(four-dim audit S-8 / reconciliation D-1); third-round T1.1 wired the "
                "reporter consumer, so it now has production uses. Loop-closure tracked in "
                "docs/reports/cpu-plane-phase0-c0-2026-10-02.md.",
    },
    {
        "name": "attribution::spawn_attribution_tap",
        # 调用为裸函数路径(无前导点),须带模块路径锚定,避免命中声明行。
        "pattern": r"event_bus::attribution::spawn_attribution_tap\(",
        "status": "WIRED",  # ADR-200 D1:experience_loop 组合根启动(生产常驻 tap)
        "note": "Subscription-side causal attribution tap (ADR-200 D1) started by the "
                "chimera-cli composition root (W4-2 batch B). Regression of this "
                "cross-module call site to zero is a FAIL on the next run (ratchet).",
    },
    {
        "name": "DecayEngine::parallel_batch",
        # `pub fn parallel_batch` decl has no leading dot; callers are bench-only.
        "pattern": r"\.parallel_batch\(",
        "status": "DEFERRED",
        "note": "public batch-decay offload entry is bench-only -- no production caller "
                "(C1/C2 scale-bounding matrix). Plan D1 demote disposition: do not "
                "optimize its parallel/failure semantics until wired to real traffic "
                "(negative-yield redline: don't invest in unreachable points). "
                "[T2.4] bench-only 等接线; 接线三要件见 decay-engine/src/parallel.rs 模块头; "
                "可达性权威源=G-87 行 decay-engine/src/parallel.rs::decay_batch_parallel verdict=DEAD.",
    },
    {
        "name": "NmcEncoder::perceive_batch",
        # lib.rs `pub use parallel::perceive_batch` is a re-export (no call parens); no `.perceive_batch(` anywhere.
        "pattern": r"\.perceive_batch\(",
        "status": "DEFERRED",
        "note": "public batch-encode entry: re-exported but zero call sites including "
                "production (C1/C2 matrix). Plan D1 demote as bench-only; distinct from "
                "single-item perceive. Gsoe/Wiki/Osa batch points are NOT listed here -- "
                "their names collide with same-named symbols (private sort select_elite; "
                "inherent search_batch/top_k_batch) and would yield misleading uses, so "
                "they are tracked in the C1/C2 report instead (dual-channel negative). "
                "[T2.4] bench-only 等接线; 接线三要件见 nmc-encoder/src/parallel.rs 模块头; "
                "可达性权威源=G-87 行 nmc-encoder/src/parallel.rs::perceive_parallel verdict=DEAD.",
    },
    {
        "name": "faae::parallel::dispatch_plan",
        # 词边界挡住同名近邻符号（chimera-tui 的 `apply_dispatch_plan`，下划线是 \w，
        # `\bdispatch_plan` 在其内部不成立）；mod tests 尾与注释行按脚本规则排除。
        "pattern": r"\bdispatch_plan\(",
        # 自身模块文件：命中转入 self_uses（定义 1 + test 探针 1 + wrapper 2）。
        "exclude_files": ["faae-router/src/parallel.rs"],
        "self_uses_baseline": 4,
        "status": "DEFERRED",
        "note": "Routing decision point, pub widened for test observability (P5, "
                "2026-10-03). Production consumers are same-module wrappers only "
                "(score_experts_scoring_batch = live via route(); score_experts_batch = "
                "dead since P3); cross-module callers are tests only "
                "(tests/env_no_parallel.rs, not production scope). Registered to "
                "close the ADR-197 A6 gap left open in cpu-plane-p0p1-progress SS24.",
    },
]

# `#[cfg(test)]` marks the start of a file's test tail **only when it precedes a
# module declaration**. Attribute-on-item forms (`#[cfg(test)] fn test_bridge()`)
# must NOT blind the scanner -- that mistake hides real production code that
# appears later in the same file (found by the first real run of this script).
CFG_TEST_RE = re.compile(r"^\s*#\[cfg\(test\)\]")
MODULE_DECL_RE = re.compile(r"^\s*(pub(\([^)]*\))?\s+)?mod\s+\w")


def is_production_file(path: Path, crates_root: Path) -> bool:
    """Only `crates/*/src/**` counts as production code.

    Excluded: tests/, benches/, examples/ (dev-only targets), target/.
    """
    rel = path.relative_to(crates_root)
    parts = rel.parts
    if len(parts) < 3:
        return False
    if parts[1] != "src":
        return False
    return True


def count_production_uses(path: Path, regex: re.Pattern) -> int:
    """Count use sites in production scope of one file.

    Test-scope exclusion rule: a `#[cfg(test)]` line only starts the excluded
    tail when the following non-blank, non-attribute line is a `mod` declaration
    (the conventional test module). Two consequences are intentional:
      * `#[cfg(test)] fn helper()` keeps the rest of the file visible;
      * code placed after a test module is still skipped, which can only make a
        WIRED item look *deader*, never the reverse -- the ratchet stays safe.

    Doc comments and `//` comments are skipped: markdown link syntax
    `([`Path`](module::Item))` ends with `)` and would otherwise be mis-read as
    a value-position use of the same identifier.
    """
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return 0
    lines = text.splitlines()
    hits = 0
    in_test_tail = False
    for idx, line in enumerate(lines):
        if not in_test_tail and CFG_TEST_RE.match(line):
            if _next_code_line_is_module(lines, idx):
                in_test_tail = True
            continue
        if in_test_tail:
            continue
        stripped = line.lstrip()
        if stripped.startswith("//"):
            continue
        if regex.search(line):
            hits += 1
    return hits


def _next_code_line_is_module(lines: list[str], cfg_idx: int) -> bool:
    """True when the line after a `#[cfg(test)]` attribute (skipping other
    attributes, doc comments and blanks) declares a module."""
    for later in lines[cfg_idx + 1:]:
        stripped = later.strip()
        if not stripped or stripped.startswith("#") or stripped.startswith("///") \
                or stripped.startswith("//!"):
            continue
        return bool(MODULE_DECL_RE.match(later))
    return False


def scan(crates_root: Path) -> list[dict]:
    """Scan every production .rs file and tally use sites per contract.

    Per-row caller-file scoping (optional fields, both backwards compatible):
      * `exclude_files`  -- the contract's own module file(s), given as
        `crates_root`-relative posix paths. Hits there do **not** count towards
        `uses`; they are tallied into `self_uses` instead. Rationale: a free
        function's declaration and the calls made by its own sibling wrapper live
        in that file, so "cross-module callers only" cannot be expressed by the
        regex alone -- any anchor that hides those artifacts also hides a genuine
        intra-module re-wire. Splitting the two numbers keeps the ledger verdict
        driven by cross-module wiring while making the intra-module delta visible
        instead of invisible.
      * `self_uses_baseline` -- how many `self_uses` hits are expected (the
        permanent artifacts). Default 0. A larger `self_uses` means someone added
        a same-module call site, which the report surfaces as a NOTE.

    Rows without these fields behave exactly as before.
    """
    compiled = []
    for c in CONTRACTS:
        exclude = {PurePosixPath(e).as_posix() for e in c.get("exclude_files", ())}
        compiled.append((c, re.compile(c["pattern"]), exclude))
    rows = [{"name": c["name"], "status": c["status"], "note": c["note"], "uses": 0,
             "self_uses": 0,
             "self_uses_baseline": c.get("self_uses_baseline", 0),
             "exclude_files": sorted(exclude)}
            for c, _, exclude in compiled]
    if not crates_root.is_dir():
        return rows
    for path in crates_root.rglob("*.rs"):
        try:
            if "target" in path.parts:
                continue
            if not is_production_file(path, crates_root):
                continue
            rel = PurePosixPath(path.relative_to(crates_root).as_posix())
        except ValueError:
            continue
        for idx, (_, regex, exclude) in enumerate(compiled):
            key = "self_uses" if rel.as_posix() in exclude else "uses"
            rows[idx][key] += count_production_uses(path, regex)
    return rows


def evaluate(rows: list[dict]) -> tuple[list[str], list[str], list[str]]:
    """Split rows into fails / promotions / ok."""
    fails, promotions, ok = [], [], []
    for row in rows:
        if row["status"] == "WIRED" and row["uses"] == 0:
            fails.append(row)
        elif row["status"] == "DEFERRED" and row["uses"] > 0:
            promotions.append(row)
        else:
            ok.append(row)
    return fails, promotions, ok


def selftest() -> int:
    """Two-way fixture check: the detector must catch a false green.

    Without this, a broken regex (matching nothing) would report "0 uses" for
    every contract and -- because DEFERRED rows never fail -- the script could
    sit green forever while proving nothing.
    """
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        src = root / "crates" / "demo-crate" / "src"
        src.mkdir(parents=True)
        (src / "wired.rs").write_text(
            "//! Doc link must not count: ([`ComputeError::Saturated`]) and "
            "(mod::ComputeError::Saturated) trailing paren is a link, not a use.\n"
            "fn main() {\n"
            "    let _ = bridge().spawn_compute_batch(kind, items);\n"
            "    let _ = b.spawn_compute(kind, || 1u32);\n"
            "    return Err(ComputeError::Saturated);\n"
            "    // commented out: b.spawn_compute(kind, f);\n"
            "    #[cfg(test)]\n"
            "    #[allow(dead_code)]\n"
            "    mod tests {\n"
            "        fn t() { let _ = bridge().update_threshold(k, 1, 1, s); }\n"
            "    }\n"
            "}\n",
            encoding="utf-8",
        )
        # attribute-on-item must NOT hide later production code
        (src / "attr_on_item.rs").write_text(
            "#[cfg(test)]\nfn test_only_helper() {}\n"
            "fn real() { let _ = bridge().update_threshold(kind, 1, 1, src); }\n",
            encoding="utf-8",
        )
        # UtilizationSampler is constructed as a BARE fn path (no call parens) in
        # production -- the exact form that made the old `\\s*\\(` pattern report 0.
        # This fixture is the regression lock for that finding: it must count as a
        # production use. Its `#[cfg(test)] mod tests` twin below must not.
        (src / "sampler_bare_path.rs").write_text(
            "fn reporter() {\n"
            "    let sampler = SAMPLER.get_or_init(UtilizationSampler::default);\n"
            "    // lookalike must NOT count: UtilizationSampler::defaults()\n"
            "    let _ = helper(UtilizationSampler::new());\n"
            "    #[cfg(test)]\n"
            "    mod tests {\n"
            "        fn t() { let _ = SAMPLER.get_or_init(UtilizationSampler::default); }\n"
            "    }\n"
            "}\n",
            encoding="utf-8",
        )
        dead = root / "crates" / "dead-crate" / "src"
        dead.mkdir(parents=True)
        (dead / "quiet.rs").write_text("// no contract use here\n", encoding="utf-8")

        # ── `exclude_files` fixture ──────────────────────────────────────
        # Mirrors the real ledger layout: the faae rows scope themselves with
        # `exclude_files = ["faae-router/src/parallel.rs"]`, so the fixture uses the
        # SAME path -- it therefore locks the real rows' semantics, not a toy.
        #   * owner file  -- declaration + 2 sibling-wrapper calls (permanent
        #     artifacts, == baseline) + 1 NEW same-module re-wire (the case that
        #     the old `::`-anchored pattern reported as *nothing at all*), plus a
        #     `#[cfg(test)] mod tests` tail whose call must stay excluded.
        #   * caller file -- a genuine cross-module call: must count towards uses.
        owner = root / "crates" / "faae-router" / "src"
        owner.mkdir(parents=True)
        (owner / "parallel.rs").write_text(
            "//! Owner module: declaration + self-calls are not cross-module wiring.\n"
            "pub fn score_experts_batch_offloaded(clv: &[f32]) {\n"
            "    let _ = clv;\n"
            "}\n"
            "pub fn score_experts_batch(clv: &[f32]) {\n"
            "    // artifact 1+2: the sibling wrapper's two branches\n"
            "    score_experts_batch(clv);\n"
            "    score_experts_batch(clv);\n"
            "    // artifact 3: a new intra-module re-wire -- used to be invisible\n"
            "    score_experts_batch(clv);\n"
            "    // the test tail goes last on purpose: everything after a `#[cfg(test)] mod`\n"
            "    // is skipped by design, so a fixture item placed there would be a fixture bug\n"
            "    #[cfg(test)]\n"
            "    mod tests {\n"
            "        fn t() { score_experts_batch(&[]); }\n"
            "    }\n"
            "}\n",
            encoding="utf-8",
        )
        (owner / "caller.rs").write_text(
            "fn r() { faae_router::parallel::score_experts_batch(&[]); }\n",
            encoding="utf-8",
        )

        rows = scan(root / "crates")
        by_name = {r["name"]: r["uses"] for r in rows}
        by_self = {r["name"]: r["self_uses"] for r in rows}

        problems = []
        # true positive: wired patterns present in production scope
        if by_name.get("ComputeBridge::spawn_compute_batch", 0) != 1:
            problems.append("selftest: spawn_compute_batch use not detected")
        if by_name.get("ComputeError::Saturated", 0) != 1:
            problems.append("selftest: Saturated construction not detected")
        # true positive: the BARE fn path form must be counted, and the `#[cfg(test)]`
        # twin must be excluded -> exactly 2 (bare path + `::new()` call), not 3.
        if by_name.get("UtilizationSampler (double-probe)", 0) != 2:
            problems.append(
                "selftest: UtilizationSampler bare-path form not detected, or test tail "
                f"leaked (uses={by_name.get('UtilizationSampler (double-probe)')}, expect 2)"
            )
        # true negative: use inside a `#[cfg(test)] mod tests` tail must NOT count
        if by_name.get("ComputeBridge::update_threshold", -1) != 1:
            problems.append(
                "selftest: attribute-on-item hid real production use, or test tail leaked "
                f"(update_threshold={by_name.get('ComputeBridge::update_threshold')}, expect 1)"
            )
        # `exclude_files`: the owner file's 4 hits must NOT inflate `uses` (only the
        # cross-module call in caller.rs does), and must be tallied as self_uses.
        if by_name.get("faae::parallel::score_experts_batch", -1) != 1:
            problems.append(
                "selftest: exclude_files did not scope the owner file "
                f"(uses={by_name.get('faae::parallel::score_experts_batch')}, expect 1: "
                "cross-module caller only)"
            )
        if by_self.get("faae::parallel::score_experts_batch", -1) != 4:
            problems.append(
                "selftest: intra-module re-wire still invisible "
                f"(self_uses={by_self.get('faae::parallel::score_experts_batch')}, expect 4 "
                "= 1 declaration + 3 self-calls, test tail excluded)"
            )
        if by_name.get("faae::parallel::score_experts_batch_offloaded", -1) != 0:
            problems.append("selftest: exclude_files leaked the offloaded row's declaration")
        if by_self.get("faae::parallel::score_experts_batch_offloaded", -1) != 1:
            problems.append(
                "selftest: offloaded owner-file declaration not tallied "
                f"(self_uses={by_self.get('faae::parallel::score_experts_batch_offloaded')}, "
                "expect 1)"
            )
        # the recorded baseline must equal the permanent-artifact count, so that an
        # added call site shows up as a visible delta instead of being absorbed.
        # row 1's fixture deliberately carries ONE intra-module re-wire beyond the
        # baseline -> the delta must be exactly 1 (this is the case the old
        # `::`-anchored pattern reported as nothing at all).
        base1 = next((r["self_uses_baseline"] for r in rows
                      if r["name"] == "faae::parallel::score_experts_batch"), None)
        if base1 is None or by_self.get("faae::parallel::score_experts_batch", 0) != base1 + 1:
            problems.append(
                "selftest: intra-module re-wire is not a visible delta "
                f"(self_uses={by_self.get('faae::parallel::score_experts_batch')}, "
                f"baseline={base1}, expect baseline+1 -> the NOTE would never fire)"
            )
        # row 2's fixture has no extra call site -> self_uses must sit ON the baseline
        # (no false NOTE on an unchanged row)
        base2 = next((r["self_uses_baseline"] for r in rows
                      if r["name"] == "faae::parallel::score_experts_batch_offloaded"), None)
        if base2 is None or by_self.get("faae::parallel::score_experts_batch_offloaded") != base2:
            problems.append(
                "selftest: self_uses_baseline is not the artifact count "
                f"(self_uses={by_self.get('faae::parallel::score_experts_batch_offloaded')}, "
                f"baseline={base2}, expect equal -> unchanged rows must not raise a NOTE)"
            )
        # optionality: rows without the new fields must be untouched
        if any(r["self_uses"] for r in rows if not r["exclude_files"]):
            problems.append("selftest: rows without exclude_files must not be self-scoped")
        # dead file contributes nothing; docs/comments must not add hits
        # 5 from the legacy fixtures + 1 from the exclude_files caller fixture
        # (spawn_compute retired by ADR-199 candidate-B -> its fixture hit matches no pattern)
        if sum(by_name.values()) != 6:
            problems.append(f"selftest: unexpected total count {by_name}")

    if problems:
        for p in problems:
            print(f"FAIL  {p}")
        print("SELFTEST: FAILED")
        return 1
    print("SELFTEST: PASSED (true-positive + true-negative fixtures)")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default="crates", help="crates/ directory")
    parser.add_argument("--json", action="store_true", help="machine-readable output")
    parser.add_argument("--selftest", action="store_true", help="verify the detector itself")
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()

    rows = scan(Path(args.root))
    fails, promotions, _ok = evaluate(rows)

    if args.json:
        print(json.dumps({"rows": rows, "fail": [r["name"] for r in fails],
                          "promote": [r["name"] for r in promotions]}, indent=2))
    else:
        print("== contract API production-caller ledger (ADR-197 A6) ==")
        for row in rows:
            verdict = "OK"
            if row in fails:
                verdict = "FAIL"
            elif row in promotions:
                verdict = "PROMOTE"
            scoped = ""
            if row["exclude_files"]:
                scoped = (f" [self_uses={row['self_uses']}"
                          f" baseline={row['self_uses_baseline']}]")
            print(f"  {verdict:<8} {row['status']:<9} uses={row['uses']:<4} "
                  f"{row['name']}  # {row['note']}{scoped}")
        print()
        for row in promotions:
            print(f"NOTE  '{row['name']}' is DEFERRED but now has {row['uses']} "
                  f"production use(s) -> promote it to WIRED in CONTRACTS.")
        for row in rows:
            if row["self_uses"] > row["self_uses_baseline"]:
                print(f"NOTE  '{row['name']}' has {row['self_uses']} same-module use(s) in "
                      f"its excluded owner file(s) {row['exclude_files']} "
                      f"(baseline {row['self_uses_baseline']}) -> an intra-module call site "
                      f"appeared and used to be invisible. If it is real wiring, promote the "
                      f"row to WIRED; if it is another self-reference, raise the baseline.")
        for row in fails:
            print(f"FAIL  '{row['name']}' is WIRED but has 0 production use(s): "
                  f"contract advertised, behaviour unreachable (regression guard).")
        total = len(rows)
        print(f"\nRESULT: {'FAIL' if fails else 'OK'} "
              f"({total - len(fails)}/{total} contracts satisfied)")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
