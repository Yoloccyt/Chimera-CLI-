#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L0 `pub const`/`pub static` surface ratchet (G-150, W2 new-gate batch).

Why this gate exists
--------------------
G-105 (pub fn surface) and G-115 (type extensibility class) leave the L0-A
`pub const`/`pub static` name surface ungated: policy tables, dimension
constants (e.g. `CLV::DIMENSION` analogues in L0), and wire-version constants
could grow or vanish with no gate turning red. G-119's D7 detector only catches
`const [T; N>=8]` TABLES, not scalar/version constants. This ratchet closes the
third surface of the same "public name" family.

What it judges
--------------
Only-decrease ratchet over the PRODUCTION `pub const`/`pub static` surface of
L0-A, keyed (module, NAME) as a MULTISET (same rationale as G-105: duplicate
names across impl blocks must register as growth):
  * NEW   - present in source, absent from baseline -> red
  * STALE - baseline row no longer defined -> red (shrink is a review act)
  * malformed baseline key -> exit 2
  * extracted surface below CONST_FLOOR -> exit 2 (blind-parser guard)

Surface semantics SHARED with G-99/G-105 on purpose: the comment/string lexer
and the #[cfg(test)] tail mask are imported from check_l0_purity, not
re-implemented.

Modes / exit codes
------------------
  (default)  judge the real repo     0 = agree, 1 = NEW/STALE
  --emit     write candidate ledger to tmp/l0_const_candidate.txt (bootstrap)
  --selftest planted fixtures prove teeth in both directions
  usage / malformed ledger / blind parser   2

Output is pure ASCII (project script convention).
"""

import os
import re
import shutil
import sys
import tempfile
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
import l0_roster  # noqa: E402  (single source for the L0-A roster; G-125 cross-checks it)
L0A_CRATES = l0_roster.L0A_DIR_NAMES
SRC_DIRS = [os.path.join(ROOT, "crates", name, "src") for name in L0A_CRATES]
BASELINE = os.path.join(HERE, "l0_const_freeze.txt")

# Floor set far below the live surface (measured at bootstrap, W2 2026-11-15);
# exists to catch a blind parser, not to pin the size.
CONST_FLOOR = 20

CONST_RE = re.compile(r"^\s*pub\s+(?:const|static)\s+([A-Z][A-Z0-9_]*)\b")
BASELINE_LINE_RE = re.compile(r"^\S+$")

sys.path.insert(0, HERE)
import check_l0_purity as purity  # noqa: E402  (shares lexer + tail mask)


def module_key(path, src_dir):
    """Crate-relative module path (same semantics as G-105)."""
    rel = os.path.relpath(path, src_dir).replace(os.sep, "/")
    if rel.endswith(".rs"):
        rel = rel[:-3]
    if rel.endswith("/mod"):
        rel = rel[: -len("/mod")]
    return rel


def extract_defs(src_dir):
    """(module_path, CONST_NAME) over the production surface of every src file."""
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
                m = CONST_RE.match(line)
                if m:
                    defs.append((mod, m.group(1)))
    return defs


def extract_all(src_dirs):
    defs = []
    for src_dir in src_dirs:
        defs.extend(extract_defs(src_dir))
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
            if not line or "::" not in line or not BASELINE_LINE_RE.match(line):
                malformed.append("%s:%d '%s'" % (os.path.basename(path), no, raw.strip()))
                continue
            rows.append(line)
    return rows, malformed


def evaluate(defs, baseline_rows):
    """Pure verdict -> (rc, output_lines, stats). Multiset comparison."""
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
            out.append("[TOOL-ERROR][key] baseline row is not MODULE::NAME: %s" % k)
        return 2, out, stats
    for key in sorted(set(live) | set(listed)):
        extra = live[key] - listed[key]
        gone = listed[key] - live[key]
        if extra > 0:
            out.append("[GAP] NEW pub const/static not in baseline (%d x): %s::%s"
                       % (extra, key[0], key[1]))
            fails += extra
        elif gone > 0:
            out.append("[STALE] baseline row(s) no longer defined (%d x, delete in "
                       "the SAME commit): %s::%s" % (gone, key[0], key[1]))
            fails += gone
    if fails:
        out.append("L0 const/static surface ratchet BROKEN: %d violation(s), "
                   "live=%d listed=%d" % (fails, stats["defs"], stats["listed"]))
        return 1, out, stats
    out.append("[OK] L0 const/static surface matches baseline: live=%d listed=%d "
               "(only-decrease ratchet)" % (stats["defs"], stats["listed"]))
    return 0, out, stats


def run_real():
    missing = [d for d in SRC_DIRS if not os.path.isdir(d)]
    if missing:
        print("[TOOL-ERROR] L0 source not found: %s" % ", ".join(missing))
        return 2
    defs = extract_all(SRC_DIRS)
    if "--emit" in sys.argv:
        out_path = os.path.join(ROOT, "tmp", "l0_const_candidate.txt")
        os.makedirs(os.path.dirname(out_path), exist_ok=True)
        with open(out_path, "w", encoding="utf-8") as fh:
            for mod, name in sorted(defs):
                fh.write("%s::%s\n" % (mod, name))
        print("[EMIT] %d live definitions (multiset) written to %s"
              % (len(defs), os.path.relpath(out_path, ROOT)))
    if len(defs) < CONST_FLOOR:
        print("[TOOL-ERROR] extracted only %d const/static defs (< floor %d) -- "
              "parser or surface is broken, refusing to judge" % (len(defs), CONST_FLOOR))
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

CLEAN_SRC = ("pub mod m;\n"
             "/// A doc comment mentioning `pub const GHOST` must not count.\n"
             "// a line comment with pub const GHOST2\n"
             "pub const MAX_DEPTH: u32 = 5;\n"
             "pub static REGISTRY: usize = 0;\n"
             "pub(crate) const INTERNAL: u32 = 1;\n"
             "const private_const: u32 = 2;\n"
             "impl T {\n"
             "    pub const ASSOC: u8 = 1;\n"
             "}\n"
             "#[cfg(test)]\n"
             "mod tests {\n"
             "    pub const IN_TAIL: u8 = 9;\n"
             "}\n"
             "pub const VERSION_TAG: &str = \"v1\";\n")
CLEAN_ROWS = ["m::MAX_DEPTH", "m::REGISTRY", "m::ASSOC", "m::VERSION_TAG"]


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

    tmp = tempfile.mkdtemp(prefix="l0const-")
    try:
        src = os.path.join(tmp, "src")
        os.makedirs(os.path.join(src, "m"))
        _write(src, "m/mod.rs", CLEAN_SRC)
        defs = extract_defs(src)
        want = sorted(tuple(r.rsplit("::", 1)) for r in CLEAN_ROWS)
        add("clean fixture: tail/doc/non-pub/private excluded, 4 defs",
            sorted(defs) == want)
        add("clean fixture + matching baseline -> rc 0",
            evaluate(defs, CLEAN_ROWS)[0] == 0)
        rc, out, _ = evaluate(defs + [("m", "EXTRA")], CLEAN_ROWS)
        add("planted NEW const is caught", rc == 1 and any("[GAP] NEW" in l for l in out))
        rc, out, _ = evaluate(defs + [("m", "MAX_DEPTH")], CLEAN_ROWS)
        add("duplicate-name growth caught (multiset semantics)",
            rc == 1 and any("NEW pub const" in l and "::MAX_DEPTH" in l for l in out))
        rc, out, _ = evaluate([d for d in defs if d[1] != "REGISTRY"], CLEAN_ROWS)
        add("deleted baseline const reports STALE (not silence)",
            rc == 1 and any("[STALE]" in l for l in out))
        rc, _o, _ = evaluate([d for d in defs if d[1] != "REGISTRY"],
                             [r for r in CLEAN_ROWS if r != "m::REGISTRY"])
        add("paired deletion (row removed in same commit) is green", rc == 0)
        rc, _o, _s = evaluate(defs, ["no_separator_here"])
        add("malformed baseline key -> rc 2 (undecidable)", rc == 2)
        add("CONST_FLOOR is below the live surface and above zero",
            0 < CONST_FLOOR <= len(extract_all(SRC_DIRS)))
        led = os.path.join(tmp, "led.txt")
        with open(led, "w", encoding="utf-8") as fh:
            fh.write("# header\n\nm::MAX_DEPTH  # policy table\nm::REGISTRY\n"
                     "m::ASSOC\nm::VERSION_TAG\n")
        rows, malformed = read_baseline(led)
        add("ledger comments/reasons parsed as non-rows",
            rows == ["m::MAX_DEPTH", "m::REGISTRY", "m::ASSOC", "m::VERSION_TAG"]
            and not malformed)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print("[SELFTEST] %s" % ("ALL ASSERTIONS HELD" if ok else "FAILED"))
    return 0 if ok else 1


def main(argv):
    args = [a for a in argv[1:]]
    if any(a not in ("--selftest", "--emit") for a in args):
        sys.stderr.write("usage: check_l0_const_ratchet.py [--selftest|--emit]\n")
        return 2
    if "--selftest" in args:
        return selftest()
    return run_real()


if __name__ == "__main__":
    import gate_rc  # crash -> exit 2, never a borrowed traceback exit 1

    sys.exit(gate_rc.run(lambda: main(sys.argv)))
