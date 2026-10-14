#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L0 `pub fn` surface ratchet (G-105, ADR-213 decision R4).

Why this gate exists
--------------------
G-91 (`check_contract_consumers.py`) is the only gate that inventories the L0
public surface, and its `DEF_RE` enumerates `pub struct|enum|trait|type`. Function
declarations are not in that character class, so all of nexus-contracts' `pub fn`
items sat outside every gate: nothing turned red when a new public helper was
added to the contract layer, and nothing turned red when one was deleted without
anyone noticing. Measured on 2026-10-13: 240 public type-level definitions (under
G-91, 161 of them exempted as having <2 consumers) versus a separately-counted
public function surface with no gate at all.

What it judges
--------------
Only-decrease ratchet over the PRODUCTION `pub fn` surface of L0:
  * NEW  - a function present in the source but absent from the baseline -> red.
  * STALE - a baseline row whose function is gone -> red (shrinking the public
    surface must be a deliberate review act, never a side effect of "I deleted a
    line anyway").
  * malformed baseline row -> exit 2 (undecidable, never a silent pass).
  * definition count below FLOOR -> exit 2. A parser that went blind would report
    zero violations exactly like a clean tree; G-91 learned this from a real
    flattening bug, so the floor ships with the gate rather than after it.

Surface semantics are SHARED with G-99 on purpose: the comment/string lexer and the
`#[cfg(test)]` tail mask are imported, not re-implemented. Two hand-written copies
of "what counts as production code" is the drift class this repository already
fights (see scripts/check_gate_predicates_parity.py for the .sh/.ps1 twin).

Modes / exit codes
------------------
  (default)   judge the real repo        0 = agree, 1 = NEW/STALE found
  --selftest  planted fixtures prove the ratchet has teeth in BOTH directions,
              and that it does not fire on prose or test-only code
                                       0 = all assertions held, 1 = not
  usage / malformed ledger / blind parser        2

Output is pure ASCII (project script convention; avoids Windows GBK console traps).
"""

import os
import re
import shutil
import sys
import tempfile
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
CRATE_DIR = os.path.join(ROOT, "crates", "nexus-contracts")
BASELINE = os.path.join(HERE, "l0_pubfn_freeze.txt")

# Below this many extracted definitions the parser is presumed blind, not clean.
# Set from the measured surface (256 +/- drift), rounded down hard on purpose:
# the floor must tolerate a legitimate shrink batch but refuse a collapse.
DEFS_FLOOR = 100

PUB_FN_RE = re.compile(r"^\s*pub\s+fn\s+([A-Za-z_]\w*)")
BASELINE_LINE_RE = re.compile(r"^\S+$")

sys.path.insert(0, HERE)
import check_l0_purity as purity  # noqa: E402  (shares lexer + tail mask)


def module_key(path, src_dir):
    """Crate-relative module path for a source file (`vector_math/util`, `lib`).

    Takes src_dir as an argument instead of closing over the real crate, so that
    --selftest can point the SAME extractor at a tempdir fixture; a fixture that
    exercised a crate-bound copy would not prove the production path works.
    """
    rel = os.path.relpath(path, src_dir).replace(os.sep, "/")
    if rel.endswith(".rs"):
        rel = rel[:-3]
    # `sparse/mod.rs` is the module `sparse`, not `sparse/mod`.
    if rel.endswith("/mod"):
        rel = rel[: -len("/mod")]
    return rel


def extract_defs(src_dir):
    """(module_path, fn_name) over the production surface of every src file."""
    defs = []
    for dirpath, _dirnames, filenames in os.walk(src_dir):
        for fn in sorted(filenames):
            if not fn.endswith(".rs"):
                continue
            path = os.path.join(dirpath, fn)
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                src = fh.read()
            code = purity.blank_rust(src)
            lines = code.splitlines()
            mask = purity.test_tail_mask(lines)
            mod = module_key(path, src_dir)
            for i, line in enumerate(lines):
                if mask[i]:
                    continue
                m = PUB_FN_RE.match(line)
                if m:
                    defs.append((mod, m.group(1)))
    return defs


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
            if not line or ":" not in line or not BASELINE_LINE_RE.match(line):
                malformed.append("%s:%d '%s'" % (os.path.basename(path), no, raw.strip()))
                continue
            rows.append(line)
    return rows, malformed


def evaluate(defs, baseline_rows):
    """Pure verdict -> (rc, output_lines, stats).

    Compared as MULTISETS, not sets: `(module, name)` is not unique within a module
    (two `impl` blocks in one file may each define `new`), and a set would register
    such a pair as one row -- adding a third `new` to that module would then look
    like nothing happened, which is the exact growth this ratchet exists to catch.
    """
    live = Counter(defs)
    listed = Counter()
    bad_key = []
    for row in baseline_rows:
        parts = row.rsplit("::", 1)
        if len(parts) != 2 or not parts[0] or not parts[1]:
            bad_key.append(row)
            continue
        listed[(parts[0], parts[1])] += 1
    stats = {"defs": sum(live.values()), "listed": sum(listed.values())}
    out = []
    fails = 0
    if bad_key:
        for k in bad_key:
            out.append("[TOOL-ERROR][key] baseline row is not MODULE::FN: %s" % k)
        return 2, out, stats
    for key in sorted(set(live) | set(listed)):
        extra = live[key] - listed[key]
        gone = listed[key] - live[key]
        if extra > 0:
            out.append("[GAP] NEW pub fn not in baseline (%d x): %s::%s"
                       % (extra, key[0], key[1]))
            fails += extra
        elif gone > 0:
            out.append("[STALE] baseline row(s) no longer defined (%d x, delete them "
                       "in the SAME commit): %s::%s" % (gone, key[0], key[1]))
            fails += gone
    if fails:
        out.append("L0 pub fn surface ratchet BROKEN: %d violation(s), live=%d "
                   "listed=%d" % (fails, stats["defs"], stats["listed"]))
        return 1, out, stats
    out.append("[OK] L0 pub fn surface matches baseline: live=%d listed=%d "
               "(only-decrease ratchet)" % (stats["defs"], stats["listed"]))
    return 0, out, stats


def run_real():
    src_dir = os.path.join(CRATE_DIR, "src")
    if not os.path.isdir(src_dir):
        print("[TOOL-ERROR] L0 source not found: %s" % src_dir)
        return 2
    defs = extract_defs(src_dir)
    if "--emit" in sys.argv:
        # Bootstrap path: the ledger does not exist yet on the very first run, so
        # emitting must happen BEFORE the "baseline missing" verdict -- otherwise
        # the one mode that creates the baseline can never be reached.
        out_path = os.path.join(ROOT, "tmp", "l0_pubfn_candidate.txt")
        os.makedirs(os.path.dirname(out_path), exist_ok=True)
        with open(out_path, "w", encoding="utf-8") as fh:
            for mod, name in sorted(defs):
                fh.write("%s::%s\n" % (mod, name))
        print("[EMIT] %d live definitions (multiset) written to %s"
              % (len(defs), os.path.relpath(out_path, ROOT)))
    if len(defs) < DEFS_FLOOR:
        # A parser that sees nothing must never be read as "surface is clean".
        print("[TOOL-ERROR] extracted only %d pub fn definitions (< floor %d) -- parser or "
              "surface is broken, refusing to judge" % (len(defs), DEFS_FLOOR))
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
    rc, out, _stats = evaluate(defs, rows)
    for line in out:
        print(line)
    return rc


# --- selftest ---------------------------------------------------------------

FN = 'pub fn %s(&self) -> u32 { 1 }\n'
CLEAN_SRC = ("pub mod m;\n"
             "/// A doc comment mentioning `pub fn ghost()` must not count.\n"
             "// a line comment with pub fn ghost2()\n"
             "pub fn alpha() -> u32 { 1 }\n"
             "pub(crate) fn internal_only() {}\n"
             "impl T {\n"
             "    pub fn beta() {}\n"
             "}\n"
             "#[cfg(test)]\n"
             "mod tests {\n"
             "    pub fn in_tail() {}\n"
             "}\n"
             "pub fn gamma() {}\n")
CLEAN_ROWS = ["m::alpha", "m::beta", "m::gamma"]


def _write(root, rel, text):
    path = os.path.join(root, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


def selftest():
    ok = True

    def add(label, cond):
        nonlocal ok
        if cond:
            print("[SELFTEST] %s" % label)
        else:
            ok = False
            print("[SELFTEST-FAIL] %s" % label)

    tmp = tempfile.mkdtemp(prefix="l0pubfn-")
    try:
        src = os.path.join(tmp, "src")
        os.makedirs(os.path.join(src, "m"))
        _write(src, "m/mod.rs", CLEAN_SRC)
        defs = extract_defs(src)
        want = sorted(tuple(r.rsplit("::", 1)) for r in CLEAN_ROWS)
        add("clean fixture: tail/doc/pub(crate) excluded, 3 defs",
            sorted(defs) == want)
        add("clean fixture + matching baseline -> rc 0",
            evaluate(defs, CLEAN_ROWS)[0] == 0)
        # Ratchet direction 1: growth must be caught (same module, genuinely new name).
        rc, out, _ = evaluate(defs + [("m", "delta")], CLEAN_ROWS)
        add("planted NEW pub fn is caught", rc == 1 and any("[GAP] NEW" in l for l in out))
        # Growth of an ALREADY-LISTED name (second `new` in one module) must also be
        # caught -- that is why the comparison is a multiset, not a set.
        rc, out, _ = evaluate(defs + [("m", "alpha")], CLEAN_ROWS)
        add("duplicate-name growth in one module is caught (multiset semantics)",
            rc == 1 and any("NEW pub fn" in l and "1 x" in l and "::alpha" in l
                            for l in out))
        # Ratchet direction 2: shrinking must NOT be silently laundered.
        rc, out, _ = evaluate([d for d in defs if d[1] != "beta"], CLEAN_ROWS)
        add("deleted baseline function reports STALE (not silence)",
            rc == 1 and any("[STALE]" in l for l in out))
        # Shrink done properly (delete the row too) -> green again.
        rc, _out, _ = evaluate([d for d in defs if d[1] != "beta"],
                               [r for r in CLEAN_ROWS if r != "m::beta"])
        add("paired deletion (row removed in same commit) is green", rc == 0)
        # Undecidable states must exit 2, never 1.
        rc, _o, _s = evaluate(defs, ["no_separator_here"])
        add("malformed baseline key -> rc 2 (undecidable)", rc == 2)
        # The floor: a parser that went blind would otherwise report a clean tree.
        add("DEFS_FLOOR is below the live surface and above zero",
            0 < DEFS_FLOOR <= len(extract_defs(os.path.join(CRATE_DIR, "src"))))
        # Baseline reading: comments and inline reasons are not rows.
        led = os.path.join(tmp, "led.txt")
        with open(led, "w", encoding="utf-8") as fh:
            fh.write("# header\n\nm::alpha  # consumed by bench\nm::beta\n"
                     "m::gamma\n")
        rows, malformed = read_baseline(led)
        add("ledger comments/reasons parsed as non-rows",
            rows == ["m::alpha", "m::beta", "m::gamma"]
            and not malformed)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print("[SELFTEST] %s" % ("ALL ASSERTIONS HELD" if ok else "FAILED"))
    return 0 if ok else 1


def main(argv):
    args = [a for a in argv[1:]]
    if any(a not in ("--selftest", "--emit") for a in args):
        sys.stderr.write("usage: check_l0_pubfn_ratchet.py [--selftest|--emit]\n")
        return 2
    if "--selftest" in args:
        return selftest()
    return run_real()


if __name__ == "__main__":
    import gate_rc  # crash -> exit 2, never a borrowed traceback exit 1

    sys.exit(gate_rc.run(lambda: main(sys.argv)))
