#!/usr/bin/env python3
# =============================================================================
# check_outer_inner_ratchet.py - outer->inner direct-edge ratchet (ADR-187 / Q3 §4.2)
# =============================================================================
# Purpose: give mechanical teeth to the half of ADR-054 that had NO gate:
#   "外环 crate 只能经 event-bus 依赖内环公开接口" (agents.md 3.4.5).
#
# WHY THIS IS NOT A REVIVAL OF Check A (the discipline ADR-187 retired):
#   * Check A policed the INNER->base/outer direction and had zero discriminating
#     power (it collapsed into Check B). Retired by ADR-187.
#   * THIS gate polices the OPPOSITE, currently-ungated direction: an OUTER crate
#     reaching INTO an inner-ring crate via a DIRECT production edge instead of
#     event-bus. Those edges are DOWNWARD, so Check B (upward-only) never sees
#     them -- a genuine detection gap, hence "net-increase in gate strength".
#   * It is an ONLY-DECREASE ratchet frozen at the CURRENT count, NOT a
#     "must be zero" rule. Existing outer->inner edges are grandfathered as
#     visible debt; the ring partition stays a NOT-ENFORCED design goal (ADR-187);
#     this gate only forbids GROWTH and rewards shrinkage. Compatible by design.
#
# Baseline: scripts/outer_inner_ratchet_baseline.txt, one `<crate>|<inner-dep>`
#   per line. Regenerate with --emit after review.
#   - edge present in tree but missing from baseline -> FAIL (growth)
#   - baseline line whose edge is gone               -> FAIL (stale; delete it)
#
# Composition-root exemption: `chimera-cli` (the binary/composition root) and
#   `nexus-app-server` (host protocol facade) instantiate lower layers as their
#   JOB; blocking their downward edges would forbid wire-up itself (same scoping
#   rationale as Check E). They are NOT counted.
#
# Inner-ring set: the 8 surviving crates of the v2.20 design ring
#   (agents.md 3.4.5; `auto-dpo` removed). Kept inline + commented so this gate
#   does not re-couple to the retired is_inner_ring() in check_dependency_rules.
#
# Encoding: all-ASCII. Exit: 0 clean, 1 gap, 2 usage/config.
# Usage:
#   python scripts/check_outer_inner_ratchet.py --selftest
#   python scripts/check_outer_inner_ratchet.py --emit
#   python scripts/check_outer_inner_ratchet.py
# =============================================================================
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# Single manifest-parsing dialect, shared with check_dep_edge_freeze.py / E-a.
from check_declared_dep_usage import (  # noqa: E402
    CRATES,
    DEP_SECTION,
    crates_index,
    parse_declared,
)

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASELINE = os.path.join(ROOT, "scripts", "outer_inner_ratchet_baseline.txt")

# v2.20 inner ring (memory + reasoning + evolution), auto-dpo removed -> 8 crates.
INNER_RING = {
    "mlc-engine", "hcw-window", "nmc-encoder",
    "quest-engine", "parliament", "gea-activator",
    "gsoe-evolution", "repo-wiki",
}
# Crates whose job is to instantiate lower layers - exempt from the ratchet.
ASSEMBLY_ROOTS = {"chimera-cli", "nexus-app-server"}


def scan_outer_inner(index):
    """Return {(crate, inner_dep)} for production [dependencies] edges that go
    from an OUTER, non-assembly-root crate INTO the inner ring."""
    edges = set()
    for cdir in sorted(index):
        if cdir in INNER_RING or cdir in ASSEMBLY_ROOTS:
            continue
        mp = os.path.join(CRATES, cdir, "Cargo.toml")
        if not os.path.exists(mp):
            continue
        for dep, section in parse_declared(mp).items():
            if section != DEP_SECTION:
                continue
            if dep in INNER_RING and dep in index.values():
                edges.add((cdir, dep))
    return edges


def load_baseline():
    entries = set()
    if not os.path.exists(BASELINE):
        print("[CONFIG] baseline missing: %s" % BASELINE)
        sys.exit(2)
    with open(BASELINE, encoding="utf-8-sig", errors="ignore") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            parts = [p.strip() for p in line.split("|")]
            if len(parts) != 2 or not all(parts):
                print("[CONFIG] malformed baseline line: %r" % raw.rstrip())
                sys.exit(2)
            entries.add((parts[0], parts[1]))
    return entries


# ---------------------------------------------------------------- self-test
def selftest():
    ok = True
    # adjudication: (baseline, edges, expect_new, expect_stale, why)
    cases = [
        ({("faae-router", "gsoe-evolution")},
         {("faae-router", "gsoe-evolution")}, 0, 0, "edge frozen in baseline -> clean"),
        ({("faae-router", "gsoe-evolution")},
         {("faae-router", "gsoe-evolution"), ("kvbsr-router", "mlc-engine")},
         1, 0, "new outer->inner edge -> GAP (growth)"),
        ({("faae-router", "gsoe-evolution"), ("sesa-router", "repo-wiki")},
         {("faae-router", "gsoe-evolution")}, 0, 1, "baseline edge event-ized -> STALE"),
        (set(), set(), 0, 0, "empty everything -> clean"),
    ]
    for baseline, edges, e_new, e_stale, why in cases:
        new = edges - baseline
        stale = baseline - edges
        if len(new) != e_new or len(stale) != e_stale:
            print("[SELFTEST] case FAILED (%s): new=%s stale=%s" % (why, len(new), len(stale)))
            ok = False
    # structural guard: inner-ring + assembly-root sets must be non-empty and
    # disjoint, else the gate could silently judge nothing.
    if not INNER_RING or not ASSEMBLY_ROOTS or (INNER_RING & ASSEMBLY_ROOTS):
        print("[SELFTEST] INNER_RING/ASSEMBLY_ROOTS misconfigured")
        ok = False
    if not ok:
        return 1
    print("[SELFTEST] outer->inner ratchet detectors hold:")
    print("           4 adjudication cases + set-disjointness guard")
    return 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--emit", action="store_true",
                    help="print the current outer->inner edge set in baseline format")
    args = ap.parse_args()

    if args.selftest:
        return selftest()

    index = crates_index()
    if len(index) < 2:
        print("[CONFIG] crate index empty/too small (%d) - wrong working dir?" % len(index))
        return 2

    edges = scan_outer_inner(index)

    if args.emit:
        for cdir, dep in sorted(edges):
            print("%s|%s" % (cdir, dep))
        return 0

    baseline = load_baseline()
    new_edges = edges - baseline
    stale = baseline - edges

    print("[INFO] outer->inner direct edges: %d (baseline: %d; ratchet: only-decrease)"
          % (len(edges), len(baseline)))

    if not new_edges and not stale:
        print("[OK] outer->inner ratchet clean: no growth, no stale entry "
              "(ADR-187 teeth on the ungated ADR-054 half)")
        return 0

    status = 1
    if new_edges:
        print("[FAIL] %d NEW outer->inner direct edge(s) not in the ratchet baseline:"
              % len(new_edges))
        for cdir, dep in sorted(new_edges):
            print("  [NEW] %s -> %s" % (cdir, dep))
        print("  Route cross-ring data via event-bus (ADR-054 d4 / agents.md 3.4.5),")
        print("  or register a reviewed freeze line in %s WITH a reason (shrink-only)."
              % os.path.relpath(BASELINE, ROOT))
    if stale:
        print("[FAIL] %d stale baseline line(s) (edge no longer exists):" % len(stale))
        for cdir, dep in sorted(stale):
            print("  [STALE] %s|%s" % (cdir, dep))
        print("  Delete them - a freeze entry must not outlive the edge it froze.")
    return status


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(main))
