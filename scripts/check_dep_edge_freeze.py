#!/usr/bin/env python3
# =============================================================================
# check_dep_edge_freeze.py - internal dependency-edge freeze gate (arch dir G')
# =============================================================================
# Purpose: mechanically stop GROWTH of the internal dependency-edge set.
#
# Background (docs/reports/arch-refactor-directions-v2-2026-09-12.md §1.1/§4 G):
#   140 internal assembly edges, 116 (82.9%) skip-level downward. The iron-law
#   gate (Check B) only forbids UPWARD edges - a new L10->L2 edge passes it as
#   easily as an L10->L9 edge, so layering has no force against edge growth.
#   The "layer-span budget" idea from the report needs layer-map data (which
#   the E-b deferral keeps scattered); an EDGE-SET FREEZE is simpler and
#   strictly stronger: ANY new edge needs an explicit registration with a
#   reason, removals shrink the baseline (ratchet, only-decrease).
#
# Relationship to check_declared_dep_usage.py (E-a gate):
#   E-a: every declared internal dep must be USED by src/ (no ghosts).
#   This gate: the SET of declared internal deps must not grow (no new edges).
#   They share the manifest parser via module import (single dialect, zero
#   code duplication).
#
# Baseline: scripts/dep_edge_freeze.txt, one `<crate-dir>|<dep>` per line.
#   - edge present in tree but missing from baseline -> FAIL (new edge)
#   - baseline line whose edge no longer exists      -> FAIL (stale; delete it)
#   Ratchet: only-decrease. A new edge must be registered WITH a reason in the
#   trailing comment AND be reviewed against the layer map by a human.
#
# Initial baseline: 2026-09-12, 138 edges (post E-a batch; Check D base 11).
#
# Encoding: all-ASCII (project script convention)
# Exit code: 0 = clean, 1 = gap found, 2 = usage/config error
# Usage:
#   python scripts/check_dep_edge_freeze.py --selftest
#   python scripts/check_dep_edge_freeze.py --emit   # print current edge set
#   python scripts/check_dep_edge_freeze.py
# =============================================================================
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# WHY import from the E-a gate: single manifest-parsing dialect. If the parser
# ever changes (new section shapes, workspace inheritance rules), both gates
# move together instead of drifting apart.
from check_declared_dep_usage import (  # noqa: E402
    CRATES,
    DEP_SECTION,
    crates_index,
    parse_declared,
)

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASELINE = os.path.join(ROOT, "scripts", "dep_edge_freeze.txt")


def scan_edges(index):
    """Return {(crate_dir, dep)} for every internal dep declared in
    `[dependencies]` (production assembly face - same scope as E-a / Check D)."""
    edges = set()
    for cdir, _pkg in sorted(index.items()):
        mp = os.path.join(CRATES, cdir, "Cargo.toml")
        if not os.path.exists(mp):
            continue
        for dep, section in parse_declared(mp).items():
            if section != DEP_SECTION:
                continue
            if dep not in index.values():
                continue  # external crate - out of scope
            edges.add((cdir, dep))
    return edges


def load_baseline():
    """Return set of (crate_dir, dep). Malformed lines fail loud (exit 2)."""
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
    # adjudication table: (baseline, edges, expect_new, expect_stale, why)
    # NOTE: edges are (crate, dep) TUPLES, matching scan_edges' output type.
    cases = [
        ({("a", "x")}, {("a", "x")}, 0, 0, "edge in baseline -> clean"),
        ({("a", "x")}, {("a", "x"), ("a", "y")}, 1, 0, "new edge absent from baseline -> GAP"),
        ({("a", "x"), ("a", "z")}, {("a", "x")}, 0, 1, "baseline edge removed from tree -> STALE"),
        (set(), set(), 0, 0, "empty everything -> clean"),
    ]
    for baseline, edges, e_new, e_stale, why in cases:
        new = edges - baseline
        stale = baseline - edges
        if len(new) != e_new or len(stale) != e_stale:
            print("[SELFTEST] case FAILED (%s): new=%s stale=%s" % (why, new, stale))
            ok = False
    if not ok:
        return 1
    print("[SELFTEST] all constructed cases classified correctly:")
    print("           4 adjudication cases (clean / new GAP / stale / empty)")
    return 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--emit", action="store_true",
                    help="print the current edge set in baseline format, then exit")
    args = ap.parse_args()

    index = crates_index()
    if len(index) < 2:
        print("[CONFIG] crate index is empty/too small (%d) — wrong working dir?" % len(index))
        return 2

    edges = scan_edges(index)

    if args.emit:
        for cdir, dep in sorted(edges):
            print("%s|%s" % (cdir, dep))
        return 0

    if args.selftest:
        return selftest()

    baseline = load_baseline()
    new_edges = edges - baseline
    stale = baseline - edges

    print("[INFO] internal assembly edges: %d (baseline: %d; ratchet: only-decrease)"
          % (len(edges), len(baseline)))

    if not new_edges and not stale:
        print("[OK] dependency-edge freeze clean: no edge added or removed without "
              "baseline registration")
        return 0

    status = 1
    if new_edges:
        print("[FAIL] %d NEW internal dependency edge(s) not in the freeze baseline:"
              % len(new_edges))
        for cdir, dep in sorted(new_edges):
            print("  [NEW] %s -> %s" % (cdir, dep))
        print("  An edge addition must be explicitly reviewed against the layer map:")
        print("  register it in scripts/dep_edge_freeze.txt WITH a reason comment,")
        print("  or remove the dependency. Prefer EventBus over new direct edges.")
    if stale:
        print("[FAIL] %d stale baseline line(s) (edge no longer exists):" % len(stale))
        for cdir, dep in sorted(stale):
            print("  [STALE] %s|%s" % (cdir, dep))
        print("  Delete them (a freeze entry must not outlive the edge it froze).")
    return status


if __name__ == "__main__":
    sys.exit(main())
