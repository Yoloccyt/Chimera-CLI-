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
from pathlib import Path

# ---------------------------------------------------------------------------
# Contract ledger
#
# `pattern` matches a *use site* (not the declaration). Declarations are written
# as `pub fn <name>` / `    Saturated,`, so requiring an opening paren / a
# `Err(`-style value position keeps definitions out of the count.
# ---------------------------------------------------------------------------
CONTRACTS = [
    {
        "name": "ComputeBridge::spawn_compute",
        # call sites look like `bridge().spawn_compute(` or `b.spawn_compute(`
        "pattern": r"\.spawn_compute\(",
        "status": "DEFERRED",
        "note": "single-task offload still has no production caller (audit S-2); "
                "decide in ADR-197 follow-up: wire a real caller or drop the API",
    },
    {
        "name": "ComputeBridge::spawn_compute_batch",
        "pattern": r"\.spawn_compute_batch\(",
        "status": "WIRED",
        "note": "batch offload points (P1-T14 / WI-34)",
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
        "pattern": r"UtilizationSampler::(?:new|with_probes)\s*\(",
        "status": "DEFERRED",
        "note": "probe reaches production via spawn_saturation_reporter; sampler "
                "periodic wiring is Phase A5 follow-up",
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
    """Scan every production .rs file and tally use sites per contract."""
    compiled = [(c, re.compile(c["pattern"])) for c in CONTRACTS]
    rows = [{"name": c["name"], "status": c["status"], "note": c["note"], "uses": 0}
            for c, _ in compiled]
    if not crates_root.is_dir():
        return rows
    for path in crates_root.rglob("*.rs"):
        try:
            if "target" in path.parts:
                continue
            if not is_production_file(path, crates_root):
                continue
        except ValueError:
            continue
        for idx, (_, regex) in enumerate(compiled):
            rows[idx]["uses"] += count_production_uses(path, regex)
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
        dead = root / "crates" / "dead-crate" / "src"
        dead.mkdir(parents=True)
        (dead / "quiet.rs").write_text("// no contract use here\n", encoding="utf-8")

        rows = scan(root / "crates")
        by_name = {r["name"]: r["uses"] for r in rows}

        problems = []
        # true positive: wired patterns present in production scope
        if by_name.get("ComputeBridge::spawn_compute", 0) != 1:
            problems.append("selftest: spawn_compute use not detected (regex too strict?)")
        if by_name.get("ComputeBridge::spawn_compute_batch", 0) != 1:
            problems.append("selftest: spawn_compute_batch use not detected")
        if by_name.get("ComputeError::Saturated", 0) != 1:
            problems.append("selftest: Saturated construction not detected")
        # true negative: use inside a `#[cfg(test)] mod tests` tail must NOT count
        if by_name.get("ComputeBridge::update_threshold", -1) != 1:
            problems.append(
                "selftest: attribute-on-item hid real production use, or test tail leaked "
                f"(update_threshold={by_name.get('ComputeBridge::update_threshold')}, expect 1)"
            )
        # dead file contributes nothing; docs/comments must not add hits
        if sum(by_name.values()) != 4:
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
            print(f"  {verdict:<8} {row['status']:<9} uses={row['uses']:<4} "
                  f"{row['name']}  # {row['note']}")
        print()
        for row in promotions:
            print(f"NOTE  '{row['name']}' is DEFERRED but now has {row['uses']} "
                  f"production use(s) -> promote it to WIRED in CONTRACTS.")
        for row in fails:
            print(f"FAIL  '{row['name']}' is WIRED but has 0 production use(s): "
                  f"contract advertised, behaviour unreachable (regression guard).")
        total = len(rows)
        print(f"\nRESULT: {'FAIL' if fails else 'OK'} "
              f"({total - len(fails)}/{total} contracts satisfied)")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
