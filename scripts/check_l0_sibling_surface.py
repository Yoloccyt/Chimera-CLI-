#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L0-A sibling crate-root re-export surface gate (G-152, W2 new-gate batch).

Why this gate exists
--------------------
G-107 freezes the UMBRELLA's root `pub use <group>::<leaf>;` surface, but the 6
sibling crates' own lib.rs re-export lines have no gate: after ADR-222 made the
siblings first-class assembly units, their crate-root surface (e.g.
`pub use memory::quota;`, the cross-sibling shims
`pub use nexus_contracts_core::core_domain;`) could grow or silently vanish.
A vanished shim would break the zero-semantic-change `crate::<leaf>` path promise
without any existing gate turning red.

What it judges
--------------
Exact-key-set parity between the live single-line `pub use <path>;` statements
in each sibling's src/lib.rs and the ledger l0_sibling_surface_freeze.txt:
  * NEW   - live re-export absent from ledger -> red
  * STALE - ledger row with no live re-export -> red (shrink is a review act)
  * fewer than SURFACE_FLOOR total live lines -> exit 2 (blind-parser guard)

Scope (deliberate): only single-line `pub use <path>;` statements (no braces).
Multi-line `pub use x::{a, b};` item lists live under G-105/G-115 jurisdiction
(function/type surfaces); this gate owns the MODULE-re-export spine.

Modes / exit codes
------------------
  (default)  judge the real repo     0 = agree, 1 = NEW/STALE
  --emit     write candidate ledger to tmp/l0_sibling_surface_candidate.txt
  --selftest planted fixtures prove teeth in both directions
  usage / malformed ledger / blind parser   2

Output is pure ASCII (project script convention).
"""

import os
import re
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
import l0_roster  # noqa: E402  (single source for the L0-A roster; G-125 cross-checks it)
L0A_CRATES = list(l0_roster.L0A_DIR_NAMES)
# G-107 already owns the umbrella root surface; this gate owns the 6 siblings.
SIBLING_CRATES = [c for c in L0A_CRATES if c != "nexus-contracts"]
BASELINE = os.path.join(HERE, "l0_sibling_surface_freeze.txt")

# Floor far below the live surface; exists to catch a blind parser.
SURFACE_FLOOR = 10

# Single-line plain re-export: `pub use <path>;` (no braces on the line).
REEXPORT_RE = re.compile(r"^\s*pub\s+use\s+([A-Za-z_][\w:]*)\s*;\s*$")
BASELINE_LINE_RE = re.compile(r"^[\w-]+\|[A-Za-z_:][\w:]*$")


def lib_path(crate):
    return os.path.join(ROOT, "crates", crate, "src", "lib.rs")


def extract_surface(crate):
    """Every single-line `pub use <path>;` in the sibling's lib.rs, as
    "<crate>|<path>" keys. Comments/strings are stripped with the shared
    G-99 lexer so doc examples do not count."""
    sys.path.insert(0, HERE)
    import check_l0_purity as purity  # noqa: E402  (shared lexer)
    with open(lib_path(crate), "r", encoding="utf-8", errors="replace") as fh:
        code = purity.blank_rust(fh.read())
    keys = []
    for line in code.splitlines():
        m = REEXPORT_RE.match(line)
        if m:
            keys.append(crate + "|" + m.group(1))
    return keys


def extract_all():
    keys = []
    for crate in SIBLING_CRATES:
        keys.extend(extract_surface(crate))
    return keys


def read_baseline(path):
    """Return (rows, malformed). Comment/blank lines are not rows."""
    rows = []
    malformed = []
    with open(path, "r", encoding="utf-8") as fh:
        for no, raw in enumerate(fh, 1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            if "#" in line:
                line = line.split("#", 1)[0].strip()
            if not line or not BASELINE_LINE_RE.match(line):
                malformed.append("%s:%d '%s'" % (os.path.basename(path), no, raw.strip()))
                continue
            rows.append(line)
    return rows, malformed


def evaluate(keys, baseline_rows):
    live = {}
    for k in keys:
        live[k] = live.get(k, 0) + 1
    listed = {}
    bad = []
    for row in baseline_rows:
        if "|" not in row:
            bad.append(row)
            continue
        listed[row] = listed.get(row, 0) + 1
    stats = {"live": sum(live.values()), "listed": sum(listed.values())}
    out = []
    fails = 0
    if bad:
        for k in bad:
            out.append("[TOOL-ERROR][key] baseline row is not CRATE|PATH: %s" % k)
        return 2, out, stats
    for key in sorted(set(live) | set(listed)):
        extra = live.get(key, 0) - listed.get(key, 0)
        gone = listed.get(key, 0) - live.get(key, 0)
        if extra > 0:
            out.append("[GAP] NEW sibling re-export not in ledger (%d x): %s"
                       % (extra, key))
            fails += extra
        elif gone > 0:
            out.append("[STALE] ledger row(s) with no live re-export (%d x, delete "
                       "in the SAME commit): %s" % (gone, key))
            fails += gone
    if fails:
        out.append("L0 sibling surface BROKEN: %d violation(s), live=%d listed=%d"
                   % (fails, stats["live"], stats["listed"]))
        return 1, out, stats
    out.append("[OK] L0 sibling re-export surface matches ledger: live=%d "
               "listed=%d (exact-key parity)" % (stats["live"], stats["listed"]))
    return 0, out, stats


def run_real():
    missing = [c for c in SIBLING_CRATES if not os.path.isfile(lib_path(c))]
    if missing:
        print("[TOOL-ERROR] sibling lib.rs not found: %s" % ", ".join(missing))
        return 2
    keys = extract_all()
    if "--emit" in sys.argv:
        out_path = os.path.join(ROOT, "tmp", "l0_sibling_surface_candidate.txt")
        os.makedirs(os.path.dirname(out_path), exist_ok=True)
        with open(out_path, "w", encoding="utf-8") as fh:
            for k in sorted(keys):
                fh.write(k + "\n")
        print("[EMIT] %d live re-export keys written to %s"
              % (len(keys), os.path.relpath(out_path, ROOT)))
    if len(keys) < SURFACE_FLOOR:
        print("[TOOL-ERROR] extracted only %d re-export lines (< floor %d) -- "
              "parser or surface is broken, refusing to judge" % (len(keys), SURFACE_FLOOR))
        return 2
    if not os.path.isfile(BASELINE):
        print("[TOOL-ERROR] baseline not found: %s" % BASELINE)
        return 2
    rows, malformed = read_baseline(BASELINE)
    if malformed:
        for m in malformed:
            print("[TOOL-ERROR][malformed] %s" % m)
        print("[TOOL-ERROR] malformed baseline rows -> undecidable, exit 2")
        return 2
    rc, out, _stats = evaluate(keys, rows)
    for line in out:
        print(line)
    return rc


# --- selftest ---------------------------------------------------------------

CLEAN_LIB = ("//! doc line mentioning `pub use fake::thing;` must not count\n"
             "pub mod memory;\n"
             "pub use memory::quota;\n"
             "pub use memory::experience_card;\n"
             "// a comment with pub use comment::only;\n"
             "use std::fmt;\n"
             "pub use nexus_contracts_core::core_domain;\n"
             "pub use nexus_contracts_core::graph_identity;\n"
             "#[cfg(test)]\n"
             "mod tests {\n"
             "    pub use super::*;\n"
             "}\n"
             "pub fn not_a_reexport() {}\n")


def selftest():
    ok = True

    def add(label, cond):
        nonlocal ok
        if cond:
            print("[SELFTEST] %s" % label)
        else:
            ok = False
            print("[SELFTEST-FAIL] %s" % label)

    tmp = tempfile.mkdtemp(prefix="l0sib-")
    try:
        # extract_surface is crate-bound through lib_path(); exercise the pure
        # pieces (regex discipline, evaluate) plus a file-level fixture by
        # temporarily pointing the extractor at the fixture through monkey-free
        # reimplementation of the line scan (kept identical on purpose).
        import check_l0_purity as purity
        code = purity.blank_rust(CLEAN_LIB)
        got = []
        for line in code.splitlines():
            m = REEXPORT_RE.match(line)
            if m:
                got.append("fixture-crate|" + m.group(1))
        want = ["fixture-crate|memory::quota",
                "fixture-crate|memory::experience_card",
                "fixture-crate|nexus_contracts_core::core_domain",
                "fixture-crate|nexus_contracts_core::graph_identity"]
        add("fixture: doc/comment/plain-use/fn/cfg(test) lines excluded, 4 keys",
            got == want)
        add("fixture + matching ledger -> rc 0", evaluate(got, want)[0] == 0)
        rc, out, _ = evaluate(got + ["fixture-crate|memory::new_leaf"], want)
        add("planted NEW re-export is caught",
            rc == 1 and any("[GAP] NEW" in l for l in out))
        rc, out, _ = evaluate([k for k in got if "graph_identity" not in k], want)
        add("vanished re-export reports STALE (not silence)",
            rc == 1 and any("[STALE]" in l for l in out))
        rc, _o, _ = evaluate([k for k in got if "graph_identity" not in k],
                             [k for k in want if "graph_identity" not in k])
        add("paired deletion (row removed in same commit) is green", rc == 0)
        rc, _o, _s = evaluate(got, ["no-pipe-here"])
        add("malformed baseline key -> rc 2 (undecidable)", rc == 2)
        add("SURFACE_FLOOR is below the live surface and above zero",
            0 < SURFACE_FLOOR <= len(extract_all()))
        led = os.path.join(tmp, "led.txt")
        with open(led, "w", encoding="utf-8") as fh:
            fh.write("# header\n\nfixture-crate|memory::quota  # shim\n"
                     "fixture-crate|memory::experience_card\n")
        rows, malformed = read_baseline(led)
        add("ledger comments/reasons parsed as non-rows",
            rows == ["fixture-crate|memory::quota",
                     "fixture-crate|memory::experience_card"] and not malformed)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print("[SELFTEST] %s" % ("ALL ASSERTIONS HELD" if ok else "FAILED"))
    return 0 if ok else 1


def main(argv):
    args = [a for a in argv[1:]]
    if any(a not in ("--selftest", "--emit") for a in args):
        sys.stderr.write("usage: check_l0_sibling_surface.py [--selftest|--emit]\n")
        return 2
    if "--selftest" in args:
        return selftest()
    return run_real()


if __name__ == "__main__":
    import gate_rc  # crash -> exit 2, never a borrowed traceback exit 1

    sys.exit(gate_rc.run(lambda: main(sys.argv)))
