#!/usr/bin/env python3
# -*- coding: ascii -*-
"""Derive the ADR number set declared by docs/architecture/adr_index.md (D-31).

WHY THIS EXISTS
---------------
`check_doc_consistency.{sh,ps1}` (D2) and `xdoc_precise.py` (the "stale ADR
declaration" rule) both read a HAND-COPIED integer out of one prose line of the
index ("ADR ... = 184 ..."). Measured 2026-10-17: expanding the index's own rows
gives 193, so the literal was 9 numbers short while every consumer stayed green
-- D2 only ever asks `declared >= disk`, which a large-enough stale constant
satisfies forever. A count that no consumer can falsify is not a control, it is
decorative text that launders trust.

This module makes the number a DERIVED quantity: single-number rows plus
interval rows expanded, de-duplicated. The index is then the only source of
truth about what it declares, and adding a row moves the number automatically.

WHAT IT DOES NOT DO
-------------------
It does not judge anything about ADRs. The verdict stays where it was (D2 of
check_doc_consistency, D3's index<->disk reconciliation, adr_index_sync). This
is a metric DERIVER: rc encodes "did I parse with confidence", never "is the
policy OK". Hence the provenance exemption row -- promoting it to a gate would
register an axis that D2/D3 already own.

EXIT CODES (gate_rc dialect)
-----------------------------
  0  parsed with confidence (floors met)
  1  reserved: never used here, so a caller can tell 1 (judged red elsewhere)
     apart from 2 (this tool could not decide)
  2  undecidable: missing file, zero rows, or below the row/scale floors
     (a silently truncated parse must never produce a plausible small number)

USAGE
-----
  adr_index_metrics.py --declared-total        # print one integer
  adr_index_metrics.py --report                # human-readable breakdown
  adr_index_metrics.py --selftest              # planted-positive fixtures
"""

import os
import re
import sys

# Row shapes actually present in the index (verified 2026-10-17): a first cell
# holding exactly one number, or one inclusive range `NNN~MMM`. Anything else
# (e.g. `ADR-SIMD-001`) is a non-numeric id and is intentionally NOT counted --
# the derived quantity is "numeric main numbers", matching how D1 counts disk.
SINGLE_ROW_RE = re.compile(r"^\|\s*ADR-(\d{3})\s*\|")
INTERVAL_ROW_RE = re.compile(r"^\|\s*ADR-(\d{3})\s*~\s*(\d{3})\s*\|")

# Scale floors. Without them a parser that quietly matched almost nothing would
# still print a plausible integer, and every consumer would call that "fine".
# Measured 2026-10-17: 123 single rows, 5 interval rows, 193 distinct numbers.
MIN_SINGLE_ROWS = 60
MIN_DISTINCT = 100


def parse_declared(text):
    """Return (singles, intervals, declared) parsed from index markdown text.

    singles    list of ints from one-number rows
    intervals  list of (lo, hi) from range rows
    declared   the distinct set of declared numeric ADR ids
    """
    singles = []
    intervals = []
    declared = set()
    for line in text.splitlines():
        m = SINGLE_ROW_RE.match(line)
        if m:
            n = int(m.group(1))
            singles.append(n)
            declared.add(n)
            continue
        m = INTERVAL_ROW_RE.match(line)
        if m:
            lo, hi = int(m.group(1)), int(m.group(2))
            if hi < lo:
                # Reversed range: refuse to invent numbers, but do not crash --
                # the floor guards below surface an underparse as exit 2.
                continue
            intervals.append((lo, hi))
            declared.update(range(lo, hi + 1))
    return singles, intervals, declared


def load_index(root=None):
    """Read the index file; return its text. Raises OSError when absent."""
    base = root or os.getcwd()
    path = os.path.join(base, "docs", "architecture", "adr_index.md")
    with open(path, encoding="utf-8") as fh:
        return fh.read()


def metrics(text):
    """Derive the metrics dict and the list of confidence problems (empty=ok)."""
    singles, intervals, declared = parse_declared(text)
    problems = []
    if not singles and not intervals:
        problems.append("zero index rows parsed (parser blindness or wrong file)")
    if len(singles) < MIN_SINGLE_ROWS:
        problems.append(
            "single rows=%d below floor %d" % (len(singles), MIN_SINGLE_ROWS))
    if len(declared) < MIN_DISTINCT:
        problems.append(
            "distinct numbers=%d below floor %d" % (len(declared), MIN_DISTINCT))
    return {
        "single_rows": len(singles),
        "interval_rows": len(intervals),
        "interval_span": sum(hi - lo + 1 for lo, hi in intervals),
        "declared_total": len(declared),
        "overlap": len(singles) + sum(hi - lo + 1 for lo, hi in intervals) - len(declared),
    }, problems


# ---------------------------------------------------------------------------
# selftest -- planted positives, because "0 findings" from an unproven parser
# is indistinguishable from a parser that reads nothing.
# ---------------------------------------------------------------------------

GOOD_TEXT = "\n".join([
    "| ADR-001 | a |",
    "| ADR-002 | b |",
    "| ADR-095~134 | merged |",
    "| ADR-100 | inside the interval above |",  # overlap: must dedupe
    "| ADR-SIMD-001 | non numeric, excluded |",
] + ["| ADR-%03d | pad%03d |" % (i, i) for i in range(200, 260)])


def _expect(cond, label):
    if not cond:
        print("[SELFTEST FAIL] %s" % label)
        return 1
    print("[selftest ok] %s" % label)
    return 0


def selftest():
    rc = 0
    m, problems = metrics(GOOD_TEXT)
    # GOOD_TEXT arithmetic: singles = 001,002,100 + 200..259 = 63;
    # interval 095~134 spans 40 numbers, of which 100 is already a single row
    # => distinct = 63 + 40 - 1 = 102, overlap = 1.
    rc |= _expect(m["single_rows"] == 63,
                  "single_rows=63, got %d" % m["single_rows"])
    rc |= _expect(m["interval_rows"] == 1,
                  "interval_rows=1, got %d" % m["interval_rows"])
    rc |= _expect(m["interval_span"] == 40,
                  "interval_span=40, got %d" % m["interval_span"])
    rc |= _expect(m["overlap"] == 1,
                  "overlap deduped to 1, got %d" % m["overlap"])
    rc |= _expect(m["declared_total"] == 102,
                  "declared_total=102, got %d" % m["declared_total"])
    rc |= _expect(problems == [], "healthy fixture reports no problem")

    # Negative control 1: a near-empty file must NOT yield a plausible number.
    _, p1 = metrics("| ADR-001 | a |\n| ADR-002 | b |\n")
    rc |= _expect(p1, "empty-ish index is undecidable, not a small green number")

    # Negative control 2: prose that only LOOKS like rows must not count.
    _, p2 = metrics("ADR-004 says MessagePack. See ADR-095~134 for the merged.\n")
    rc |= _expect(p2, "non-table prose contributes zero rows -> undecidable")

    # Negative control 3: a reversed interval must be rejected outright, and
    # must not change the set derived from the same good rows.
    _, good_intervals, good_declared = parse_declared(GOOD_TEXT)
    _, iv, decl = parse_declared("| ADR-134~095 | bad |\n" + GOOD_TEXT)
    rc |= _expect(len(iv) == len(good_intervals),
                  "reversed interval rejected, kept %d of %d"
                  % (len(iv), len(good_intervals)))
    rc |= _expect(decl == good_declared,
                  "reversed interval invents no numbers")

    # Negative control 4: a tilde without a second bound is not an interval.
    _, iv2, _ = parse_declared("| ADR-095~ | malformed |\n" + GOOD_TEXT)
    rc |= _expect(len(iv2) == len(good_intervals),
                  "malformed interval rejected, kept %d" % len(iv2))

    # Negative control 5: the non-numeric id (ADR-SIMD-001) must never enter the
    # set and must not corrupt it -- the set stays exactly the numeric ids.
    rc |= _expect(all(isinstance(x, int) for x in good_declared)
                  and (1 in good_declared) and (259 in good_declared)
                  and (0 not in good_declared) and (100 in good_declared),
                  "set holds only numeric ids; ADR-SIMD-001 excluded, "
                  "count=%d" % len(good_declared))
    print("SELFTEST: %s" % ("PASS" if rc == 0 else "FAIL"))
    return 0 if rc == 0 else 2


def main(argv):
    if "--selftest" in argv:
        return selftest()
    try:
        text = load_index()
    except OSError as exc:
        print("[UNDECIDABLE] cannot read docs/architecture/adr_index.md: %s" % exc)
        return 2
    m, problems = metrics(text)
    if problems:
        for p in problems:
            print("[UNDECIDABLE] %s" % p)
        return 2
    if "--declared-total" in argv:
        print(m["declared_total"])
        return 0
    print("ADR index declared set: single_rows=%d interval_rows=%d "
          "interval_span=%d overlap=%d distinct=%d"
          % (m["single_rows"], m["interval_rows"], m["interval_span"],
             m["overlap"], m["declared_total"]))
    return 0


if __name__ == "__main__":
    # Crash guard (F33/F37): this file is run as a program by check_doc_consistency.sh, so an
    # uncaught exception would exit 1 -- the same code as "judged red", turning "never decided"
    # into a verdict downstream. The guard lives in the gate_rc single point (see its WHY).
    # Import inside __main__ so importers (xdoc_precise.py) do not depend on gate_rc.
    import gate_rc
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
