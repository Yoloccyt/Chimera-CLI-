#!/usr/bin/env python3
"""check_accel_exclusion.py -- accelerator / inference-stack exclusion is enforced.

Why this script exists (evidence, 2026-10-02 audit; ADR-195 D1 / V1):
    ADR-195 decided NOT to adopt GPU compute and laid down an acceptance
    criterion (V1): `Cargo.lock` must contain zero hits for the wgpu family,
    naga, ash, d3d12, candle, burn, ort. Until now that criterion was a
    one-off manual check, so it had no regression guard.

    The criterion is load-bearing because the exclusion is currently held by
    *feature non-activation* rather than by structure. Concretely,
    `crates/nmc-encoder/Cargo.toml` takes a direct production dependency
    `mel_spec = "0.4"` whose own manifest exposes three optional features:
        wgpu        -> dep:wgpu + dep:bytemuck + dep:pollster
        cuda        -> (feature only)
        ort-tensor  -> ort
    with `default = []`. So the project can acquire a GPU or an ONNX-Runtime
    stack -- with zero ADR, zero review-visible decision, and (for the wgpu
    path) an unresolved `forbid(unsafe_code)` x `bytemuck::derive(Pod)`
    coexistence question that ADR-195 D5 E1 explicitly flagged as having no
    precedent -- purely by appending a feature list on an existing line.
    `ort` additionally only supports MSVC, so pulling it in breaks the
    pinned `x86_64-pc-windows-gnu` toolchain (the same reason
    `crates/nmc-encoder/Cargo.toml` records for rejecting `ort` in the first
    place).

    This script turns that exclusion from a documented promise into an
    executable assertion. It does NOT change ADR-195's decision, and it does
    not inspect feature flags -- only resolved packages.

Scope note (deliberate):
    This is a *lockfile* assertion, not a feature-surface freeze. It cannot
    stop someone adding `features = ["wgpu"]` to a direct dependency; that
    would still show up here as a `wgpu` entry, so it is caught -- but
    freezing `default-features = false` on `mel_spec` is a separate decision
    and is NOT taken here (see the batch's unresolved item D-3).

Semantics:
    0  no forbidden package is present in the lockfile
    1  at least one forbidden package is present (or the lockfile is unreadable)

Usage:
    python scripts/check_accel_exclusion.py [--root PATH] [--json] [--selftest]
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

# ---------------------------------------------------------------------------
# Forbidden package set (ADR-195 D1 / V1).
#
# `exact` matches `name = "<pkg>"` verbatim. `families` matches any package
# whose name starts with "<prefix>-" -- the ecosystem ships members under the
# family name (`candle-core`, `ort-sys`), so matching the bare name alone
# would let the real package through.
#
# WHY read the lockfile text instead of the dependency graph: ADR-196 D5
# registers that `check_dependency_rules.sh` only parses `workspace = true`
# declarations and therefore does not intercept external version
# dependencies. A text assertion needs no dependency resolution and so runs
# in a zero-second light gate.
# ---------------------------------------------------------------------------
EXACT_FORBIDDEN = (
    "wgpu",
    "wgpu-core",
    "wgpu-hal",
    "wgpu-types",
    "naga",
    "ash",
    "d3d12",
    "burn",
)

# `candle` itself is not published; the ecosystem is `candle-*`.
# `ort` is published bare, and pulls `ort-sys` / `ort-sys-next`.
FORBIDDEN_FAMILIES = {
    "candle": "candle ecosystem (candle-core, candle-nn, ...)",
    "ort": "ONNX Runtime (ort, ort-sys, ort-sys-next)",
}

# A lockfile entry looks like:
#     [[package]]
#     name = "wgpu"
#     version = "0.20.1"
# Anchoring on `name = "..."` avoids matching the same substring appearing in
# a path, a source URL, or a comment.
NAME_LINE = re.compile(r'^\s*name\s*=\s*"([^"]+)"\s*$', re.MULTILINE)


def scan_lock(lock_text: str) -> list[dict]:
    """Return one row per forbidden rule that has at least one hit."""
    names = set(NAME_LINE.findall(lock_text))
    hits: list[dict] = []

    for pkg in EXACT_FORBIDDEN:
        if pkg in names:
            hits.append({"rule": pkg, "kind": "exact", "reason": "ADR-195 D1 excluded package"})

    for prefix, reason in sorted(FORBIDDEN_FAMILIES.items()):
        members = sorted(n for n in names if n == prefix or n.startswith(prefix + "-"))
        if members:
            hits.append(
                {
                    "rule": f"{prefix}-*",
                    "kind": "family",
                    "reason": reason,
                    "members": members,
                }
            )
    return hits


def evaluate(lock_text: str) -> tuple[list[dict], list[dict]]:
    """Split findings into (failures, all rules) for reporting."""
    hits = scan_lock(lock_text)
    return hits, hits


def selftest() -> int:
    """Four-way fixture check. Without a true-negative case a regex that
    matches nothing would pass every assertion, so both directions are locked:
    a forbidden package must be caught, and a near-miss must not be."""

    clean = (
        "[[package]]\n"
        'name = "nexus-core"\n'
        'version = "2.29.0-omega"\n'
        'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
        "\n"
        "[[package]]\n"
        # Contains "ort" and "candle" as substrings but is NOT a family member:
        # the rules must anchor at the start of the name, not match mid-string.
        'name = "stronghold-embedded-candler"\n'
        'version = "0.1.0"\n'
    )
    # Same name appearing only in a non-`name =` position must not count.
    in_comment_only = (
        "[[package]]\n"
        'name = "nexus-core"\n'
        'description = "we deliberately do not depend on wgpu or ort"\n'
    )
    with_ort = clean + '\n[[package]]\nname = "ort"\nversion = "2.0.0-rc.10"\n'
    with_candle = clean + '\n[[package]]\nname = "candle-core"\nversion = "0.8.1"\n'

    problems = []
    if scan_lock(clean):
        problems.append(f"selftest: clean lockfile produced false hits {scan_lock(clean)}")
    if scan_lock(in_comment_only):
        problems.append(
            f"selftest: substring in a non-name line counted {scan_lock(in_comment_only)}"
        )
    if not scan_lock(with_ort):
        problems.append("selftest: bare `ort` was NOT caught (exact/family rule broken)")
    candle_hits = scan_lock(with_candle)
    if not any(h["rule"] == "candle-*" and "candle-core" in h.get("members", []) for h in candle_hits):
        problems.append(f"selftest: `candle-core` was NOT caught as a family member {candle_hits}")

    if problems:
        for p in problems:
            print(f"FAIL  {p}")
        print("SELFTEST: FAILED")
        return 1
    print("SELFTEST: PASSED (true-positive + true-negative fixtures)")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default="Cargo.lock", help="path to Cargo.lock")
    parser.add_argument("--json", action="store_true", help="machine-readable output")
    parser.add_argument("--selftest", action="store_true", help="verify the detector itself")
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()

    lock = Path(args.root)
    try:
        text = lock.read_text(encoding="utf-8", errors="replace")
    except OSError as exc:
        print(f"FAIL  cannot read {lock}: {exc}")
        return 1

    hits, _ = evaluate(text)

    if args.json:
        print(json.dumps({"lockfile": str(lock), "fail": hits}, indent=2))
    else:
        print("== accelerator / inference-stack exclusion (ADR-195 D1, V1) ==")
        if hits:
            for h in hits:
                extra = f" members={h['members']}" if h.get("members") else ""
                print(f"  HIT   rule={h['rule']:<14} {h['reason']}{extra}")
            print(
                "\n  ADR-195 D1 forbids these. If a new entry appeared, either a direct\n"
                "  dependency gained a GPU / ONNX-Runtime feature, or a new dependency was\n"
                "  added. Reopening GPU compute requires superseding ADR-107/123/124/126\n"
                "  with new evidence, and its restart gates G-A/G-B/G-C must all pass."
            )
        else:
            names = len(NAME_LINE.findall(text))
            print(f"  OK    0 forbidden packages among {names} lockfile entries")
    return 1 if hits else 0


if __name__ == "__main__":
    sys.exit(main())
