#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L0 root re-export surface ratchet (G-107, ADR-213 decision R5).

Why this gate exists
--------------------
W3 (ADR-212 decision 5) moved the 53 leaf modules of nexus-contracts into 10
groups but kept the crate root facade byte-identical by re-exporting every leaf
from lib.rs. That means every leaf is reachable by TWO public paths today:
`nexus_contracts::<leaf>` (what the 34 consumer crates use) and
`nexus_contracts::<group>::<leaf>` (the cohesive one). Freezing the existing
double paths is fine; letting the set GROW is not -- each new leaf published at
the root is a permanent second path that can never be removed later without
breaking consumers, which is exactly how the surface became ungoverned once.

What it judges
--------------
Only-decrease ratchet over `pub use <group>::<leaf>;` declarations in
crates/nexus-contracts/src/lib.rs:
  * NEW   - a root re-export present in lib.rs but absent from the ledger -> red.
    New modules should be reachable through their group path only.
  * STALE - a ledger row whose declaration is gone -> red (collapsing a public
    path is a review act; it must not be a side effect of touching lib.rs).
  * malformed ledger row / parser below FLOOR -> exit 2 (undecidable, never a
    silent pass; the floor exists because a blind regex reports exactly the same
    thing a clean tree does).

Doc comments and string literals are blanked first via the SAME lexer as G-99
(imported, not copied -- two hand-written notions of "what is code" is the drift
class check_gate_predicates_parity.py exists to fight).

Modes / exit codes
------------------
  (default)  judge the real repo     0 = agree, 1 = NEW/STALE found
  --selftest planted fixtures        0 = all assertions held, 1 = not
  --emit     dump live rows for bootstrapping the ledger
  usage / malformed / blind parser   2

Output is pure ASCII (project script convention; avoids Windows GBK console traps).
"""

import os
import re
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
LIB_RS = os.path.join(ROOT, "crates", "nexus-contracts", "src", "lib.rs")
LEDGER = os.path.join(HERE, "l0_root_reexport_freeze.txt")

# Measured surface is 53; the floor tolerates a legitimate shrink batch but
# refuses a collapse (a parser that matched nothing would report 0 rows).
DEFS_FLOOR = 20

# `pub use group::leaf;` -- must end right after the second path segment, so that
# the value-level re-exports (`pub use crate::leaf::{A, B};`) are NOT in this class.
ROOT_REEXPORT_RE = re.compile(r"^\s*pub\s+use\s+([a-z_][a-z0-9_]*)::([a-z_][a-z0-9_]*)\s*;")
sys.path.insert(0, HERE)
import check_l0_purity as purity  # noqa: E402  (shares the comment/string lexer)


def extract_rows(text):
    """[(group, leaf)] of root re-export declarations, code surface only."""
    code = purity.blank_rust(text)
    rows = []
    for line in code.splitlines():
        m = ROOT_REEXPORT_RE.match(line)
        if m:
            rows.append((m.group(1), m.group(2)))
    return rows


def read_ledger(path):
    rows, malformed = [], []
    with open(path, "r", encoding="utf-8") as fh:
        for no, raw in enumerate(fh, 1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            if "#" in line:
                line = line.split("#", 1)[0].strip()
            parts = line.split("::")
            if len(parts) != 2 or not all(parts):
                malformed.append("%s:%d '%s'" % (os.path.basename(path), no, raw.strip()))
                continue
            rows.append((parts[0], parts[1]))
    return rows, malformed


def evaluate(live, listed):
    """Pure verdict -> (rc, lines). Both sides are lists of (group, leaf)."""
    live_set = set(live)
    listed_set = set(listed)
    out = []
    fails = 0
    for group, leaf in sorted(live_set - listed_set):
        out.append("[GAP] NEW root re-export not in ledger: nexus_contracts::%s "
                   "(published by `pub use %s::%s;`) -- a new leaf should be reachable "
                   "through its group path only" % (leaf, group, leaf))
        fails += 1
    for group, leaf in sorted(listed_set - live_set):
        out.append("[STALE] ledger row no longer declared (delete it in the SAME "
                   "commit): nexus_contracts::%s" % leaf)
        fails += 1
    if fails:
        out.append("L0 root re-export ratchet BROKEN: %d violation(s), live=%d "
                   "listed=%d" % (fails, len(live_set), len(listed_set)))
        return 1, out
    out.append("[OK] L0 root re-export surface matches ledger: live=%d listed=%d "
               "(only-decrease ratchet)" % (len(live_set), len(listed_set)))
    return 0, out


def run_real():
    if not os.path.isfile(LIB_RS):
        print("[TOOL-ERROR] L0 lib.rs not found: %s" % LIB_RS)
        return 2
    with open(LIB_RS, "r", encoding="utf-8", errors="replace") as fh:
        live = extract_rows(fh.read())
    if "--emit" in sys.argv:
        out_path = os.path.join(ROOT, "tmp", "l0_root_surface_candidate.txt")
        os.makedirs(os.path.dirname(out_path), exist_ok=True)
        with open(out_path, "w", encoding="utf-8") as fh:
            for group, leaf in sorted(set(live)):
                fh.write("%s::%s\n" % (group, leaf))
        print("[EMIT] %d live root re-exports written to %s"
              % (len(set(live)), os.path.relpath(out_path, ROOT)))
    if len(live) < DEFS_FLOOR:
        print("[TOOL-ERROR] extracted only %d root re-exports (< floor %d) -- parser "
              "or surface is broken, refusing to judge" % (len(live), DEFS_FLOOR))
        return 2
    if not os.path.isfile(LEDGER):
        print("[TOOL-ERROR] ledger not found: %s" % LEDGER)
        return 2
    listed, malformed = read_ledger(LEDGER)
    if malformed:
        for m in malformed:
            print("[TOOL-ERROR][malformed] %s" % m)
        print("[TOOL-ERROR] malformed ledger rows -> undecidable, exit 2")
        return 2
    rc, out = evaluate(live, listed)
    for line in out:
        print(line)
    return rc


# --- selftest ---------------------------------------------------------------

CLEAN_LIB = (
    "//! Docs may MENTION `pub use ghost::leaf;` without publishing it.\n"
    "pub mod sparse;\n"
    "pub use sparse::masks;\n"
    "pub use sparse::omni_masks;\n"
    "pub use core_domain::domain;\n"
    "pub use crate::masks::{SparseMask, OmniSparseMasks};\n"  # value level, not this class
    "pub const X: &str = \"pub use fake::thing;\";\n"          # string literal
)
CLEAN_ROWS = [("sparse", "masks"), ("sparse", "omni_masks"), ("core_domain", "domain")]


def selftest():
    ok = True

    def add(label, cond):
        nonlocal ok
        if cond:
            print("[SELFTEST] %s" % label)
        else:
            ok = False
            print("[SELFTEST-FAIL] %s" % label)

    live = extract_rows(CLEAN_LIB)
    add("doc comment / string literal / value-level re-export not counted",
        sorted(live) == sorted(CLEAN_ROWS))
    add("matching ledger -> rc 0", evaluate(live, CLEAN_ROWS)[0] == 0)
    rc, out = evaluate(live + [("safety", "brand_new")], CLEAN_ROWS)
    add("planted NEW root re-export is caught", rc == 1 and "[GAP] NEW" in out[0])
    rc, out = evaluate([r for r in live if r[1] != "masks"], CLEAN_ROWS)
    add("removed declaration reports STALE (not silence)",
        rc == 1 and any("[STALE]" in l for l in out))
    rc, _o = evaluate([r for r in live if r[1] != "masks"],
                      [r for r in CLEAN_ROWS if r[1] != "masks"])
    add("paired deletion (row removed in same commit) is green", rc == 0)
    fd, tmp = tempfile.mkstemp(suffix=".txt")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write("# header\n\nsparse::masks  # consumed by L6\nbad_line_no_sep\n")
        _rows, malformed = read_ledger(tmp)
        add("malformed ledger row is flagged", len(malformed) == 1)
    finally:
        os.remove(tmp)
    with open(LIB_RS, "r", encoding="utf-8", errors="replace") as fh:
        real_live = extract_rows(fh.read())
    add("DEFS_FLOOR is below the live surface and above zero",
        0 < DEFS_FLOOR <= len(real_live))
    add("real ledger agrees with the real tree", evaluate(
        real_live, read_ledger(LEDGER)[0])[0] == 0 if os.path.isfile(LEDGER) else False)
    print("[SELFTEST] %s" % ("ALL ASSERTIONS HELD" if ok else "FAILED"))
    return 0 if ok else 1


def main(argv):
    args = [a for a in argv[1:]]
    if any(a not in ("--selftest", "--emit") for a in args):
        sys.stderr.write("usage: check_l0_root_surface.py [--selftest|--emit]\n")
        return 2
    if "--selftest" in args:
        return selftest()
    return run_real()


if __name__ == "__main__":
    import gate_rc  # crash -> exit 2, never a borrowed traceback exit 1

    sys.exit(gate_rc.run(lambda: main(sys.argv)))
