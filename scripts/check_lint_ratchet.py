#!/usr/bin/env python3
# =============================================================================
# check_lint_ratchet.py - workspace.lints opt-in ratchet gate
# (arch quality deep-review, plan Phase 3 item E-5, first increment)
# =============================================================================
# Purpose: make the E-5 `[workspace.lints]` governance REAL and SAFE to grow.
#   Root Cargo.toml declares clippy unwrap_used/expect_used = "warn"; the block
#   is INERT per crate until that crate adds `[lints] workspace = true`. With
#   40/41 crates holding >5 existing call sites (max 858, measured 2026-09-25),
#   switching everything at once would flood CI `-D warnings` builds -- so the
#   plan is opt-in per crate, and THIS gate is the ratchet that keeps the plan
#   honest:
#     V1  a crate registered in lint_ratchet_crates.txt must still carry
#         `[lints] workspace = true` (removing the opt-in silently disarms it)
#     V2  a registered crate must measure ZERO `.unwrap()` / `.expect(` across
#         ALL targets (src/tests/benches/examples) -- the lint applies to every
#         target, so a clean src with dirty tests would still flood warnings
#     V3  a crate that opted in but is NOT registered fails: the ledger is the
#         review surface (same "register or don't" discipline as
#         dep_edge_freeze), and the registered set only-grows
#   Exit dialect (gate_rc): 0 clean / 1 judged red / 2 undecidable.
#   Encoding: all-ASCII (project script convention).
#
# Why not clippy itself as the ratchet: warn-level lints exit 0; CI promotes
# with -D warnings only for the narrow dbg/todo/unimplemented set (see report
# S5.13). The registration+measure checks above have no clippy equivalent.
#
# Usage:
#   python scripts/check_lint_ratchet.py            # the gate
#   python scripts/check_lint_ratchet.py --emit     # print current opt-in set
#   python scripts/check_lint_ratchet.py --selftest # planted fixture workspace
# =============================================================================
import argparse
import os
import re
import shutil
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATES = os.path.join(ROOT, "crates")
LEDGER = os.path.join(ROOT, "scripts", "lint_ratchet_crates.txt")
OFFENDERS = re.compile(r"\.unwrap\(\)|\.expect\s*\(")
# cfg(test) AND compound forms -- cfg(all(test, feature = "..")) gates a real
# test module too (6th landmine, measured 2026-09-25 on chimera-tui/render.rs
# v3_engine_tests: 8 test offenders counted as production). The negative
# lookahead keeps cfg(not(test)) -- which is PRODUCTION code -- excluded.
# Line-level .*: balanced-paren alternation cannot count through nested () of
# all(..); a cfg attr is one line and comments/strings are stripped already.
# Known approximation: cfg(all(not(test), ..)) would be mis-read as test scope
# (grep of the real tree: zero occurrences).
CFG_TEST = re.compile(r"#\[cfg\((?!not\(test\)).*\btest\b.*\)\]")
LINTS_SECTION = re.compile(r"^\[lints\]\s*$", re.M)
LINTS_WORKSPACE = re.compile(r"^\[lints\][^[]*?^workspace\s*=\s*true", re.M | re.S)


def strip_line_noise(line):
    """Drop strings/line comments (lexical-gate family convention)."""
    out, _ = strip_line_noise_s(line, (False, 0))
    return out


def strip_line_noise_s(line, state):
    """Stateful variant: state = (in_string, raw_hashes) survives across lines
    so a multi-line raw string (r#"..."# JSON fixtures) cannot skew the brace
    counting line by line -- the exact failure a per-line strip has."""
    out = []
    in_str, raw = state
    i = 0
    while i < len(line):
        c = line[i]
        if raw:  # inside r#"..."# -- only the closing quote+hashes ends it
            if c == '"' and line[i:i + 1 + raw] == '"' + "#" * raw:
                i += 1 + raw
                raw = 0
            else:
                i += 1
            continue
        if in_str:
            if c == "\\":
                i += 2
                continue
            if c == '"':
                in_str = False
            i += 1
            continue
        if c == '"':
            in_str = True
            i += 1
            continue
        if c == "'":
            # char literal ('a', '\n', '\\', '"', '\\u{..}') vs lifetime ('de).
            # An unpaired apostrophe must NOT flip the string state -- 5th
            # structural landmine (measured 2026-09-25: trim_matches('"')
            # opened a fake string that survived to EOF, so the cfg(test)
            # window never opened and 14 test offenders counted as production).
            k, closed = i + 1, False
            while k < len(line) and k - i <= 10:
                ch = line[k]
                if ch == "\\":
                    k += 2
                    continue
                if ch == "'":
                    closed = True
                    break
                if ch in " \t(":
                    break
                k += 1
            if closed:
                i = k + 1
                continue
            i += 1  # lifetime / stray apostrophe: no state flip
            continue
        if c == "r" and i + 1 < len(line):
            j = i + 1
            hashes = 0
            while j < len(line) and line[j] == "#":
                j += 1
                hashes += 1
            if j < len(line) and line[j] == '"':
                if hashes:
                    raw = hashes
                    i = j + 1
                    continue
                # r"..." single-line form: treat as raw with 0 hashes by the same loop
                raw = 0
                in_str = True
                i = j + 1
                continue
        if c == "/" and i + 1 < len(line) and line[i + 1] == "/":
            break
        out.append(c)
        i += 1
    return "".join(out), (in_str, raw)


def count_offenders_file(path):
    """Per-file unwrap/expect count EXCLUDING #[cfg(test)] regions.

    The exclusion aligns the V2 measurement with the clippy reality: repo
    convention allows unwrap/expect in test modules (idiomatic), so a registered
    crate is "clean" iff its PRODUCTION code is. A file-level inner
    `#![allow(clippy::unwrap_used|expect_used)]` (the integration-test/bench
    idiom) exempts the whole file for the same reason. If an allow ever masks a
    production offender, `cargo clippy --all-targets` still catches it in
    review -- here we mirror the semantic, not re-implement the compiler.
    """
    total = 0
    depth = 0
    skip_until = None      # depth to return below to end the cfg(test) region
    pending = False        # cfg(test) attribute seen, opener not yet reached
    st = (False, 0)        # cross-line string state (multi-line raw strings)
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        # inner attributes are syntactically file-top items, and the file-top
        # region ends at the first line that is neither //!-doc nor an inner
        # attribute nor blank -- scanning that region is complete, not heuristic
        # (a long //! header block may push the allow past any fixed line window).
        head = []
        for line in fh:
            s = strip_line_noise(line).strip()
            if s == "" or s.startswith("//!") or s.startswith("#!["):
                head.append(s)
                continue
            break
        if any(h.startswith("#![allow(") and ("clippy::unwrap_used" in h
                                               or "clippy::expect_used" in h
                                               or "clippy::all" in h) for h in head):
            return 0  # file-level inner allow (test/bench unit)
        fh.seek(0)
        for raw in fh:
            code, st = strip_line_noise_s(raw, st)
            opens = code.count("{")
            closes = code.count("}")
            if skip_until is not None:
                depth += opens - closes
                if depth <= skip_until:
                    skip_until = None
                continue
            if CFG_TEST.search(code):
                pending = True
            elif pending and opens:
                # cfg(test) attribute attaches to the NEXT item; multi-line
                # signatures open the region only at the first '{' -- wait for
                # it instead of requiring mod/fn on the same line.
                skip_until = depth
                depth += opens - closes
                pending = False
                if depth > skip_until:
                    continue
                skip_until = None  # single-line item, already closed
                continue
            elif code.strip() and not code.strip().startswith("#"):
                pass  # pending stays until the item's brace opens
            # clippy does not lint doc comments (///-//!): counting their
            # unwrap/expect examples would diverge from the lint this mirrors.
            stripped = code.lstrip()
            if not stripped.startswith(("///", "//!")):
                total += len(OFFENDERS.findall(code))
            depth += opens - closes
    return total


def crate_dirs(crates=None):
    base = crates or CRATES
    if not os.path.isdir(base):
        return []
    return sorted(d for d in os.listdir(base)
                  if os.path.isdir(os.path.join(base, d, "src")))


def opted_in(manifest_path):
    """True iff the manifest has a [lints] section with workspace = true."""
    try:
        with open(manifest_path, encoding="utf-8", errors="replace") as fh:
            text = fh.read()
    except OSError:
        return False
    if not LINTS_SECTION.search(text):
        return False
    return bool(LINTS_WORKSPACE.search(text))


def count_offenders(crate_dir):
    """Production-code (cfg(test)-excluded) unwrap/expect count for one crate."""
    total = 0
    for sub in ("src", "tests", "benches", "examples"):
        d = os.path.join(crate_dir, sub)
        if not os.path.isdir(d):
            continue
        for dirpath, _dirs, files in os.walk(d):
            for f in files:
                if f.endswith(".rs"):
                    total += count_offenders_file(os.path.join(dirpath, f))
    return total


def load_ledger(path=None):
    p = path or LEDGER
    if not os.path.exists(p):
        raise ValueError("ledger missing: %s (take a baseline via --emit)" % p)
    rows = []
    with open(p, encoding="utf-8", errors="replace") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if line:
                rows.append(line)
    return set(rows)


def decide(crates_dir, ledger_path):
    """Return (v1_missing_lints, v2_dirty, v3_unregistered, opts)."""
    ledger = load_ledger(ledger_path)
    opts = {c for c in crate_dirs(crates_dir)
            if opted_in(os.path.join(crates_dir, c, "Cargo.toml"))}
    v1 = sorted(ledger - opts)                       # registered but disarmed
    v3 = sorted(opts - ledger)                       # opted in but unregistered
    v2 = []
    for c in sorted(ledger & opts):
        n = count_offenders(os.path.join(crates_dir, c))
        if n:
            v2.append((c, n))
    return v1, v2, v3, opts


# ------------------------------------------------------------------ selftest
def _fixture(base):
    """Workspace: clean+registered+opted (green); each violation shape once."""
    def mk(name, lints, offender_line, register=True):
        cdir = os.path.join(base, "crates", name)
        os.makedirs(os.path.join(cdir, "src"), exist_ok=True)
        with open(os.path.join(cdir, "Cargo.toml"), "w", encoding="utf-8") as fh:
            fh.write('[package]\nname = "%s"\nversion = "0.1.0"\n' % name)
            if lints:
                fh.write("\n[lints]\nworkspace = true\n")
        with open(os.path.join(cdir, "src", "lib.rs"), "w", encoding="utf-8") as fh:
            fh.write("pub fn f() -> u32 { 1 %s }\n" % (offender_line or ""))
        return name
    rows = []
    rows.append(mk("alpha", True, ""))                # clean + opted
    rows.append(mk("beta", True, "// allow"))        # clean + opted but UNREG (v3)
    rows.append(mk("gamma", False, ""))              # registered, NOT opted (v1)
    rows.append(mk("delta", True, "let _x = Some(1).unwrap();"))  # opted+reg+dirty (v2)
    scripts = os.path.join(base, "scripts")
    os.makedirs(scripts, exist_ok=True)
    ledger = os.path.join(scripts, "lint_ratchet_crates.txt")
    with open(ledger, "w", encoding="utf-8") as fh:
        fh.write("alpha # clean baseline opt-in\n"
                 "gamma # registered\n"
                 "delta # registered (dirty -> v2)\n")
    return ledger


def selftest():
    base = tempfile.mkdtemp(prefix="lintratchet-")
    try:
        ledger = _fixture(base)
        crates = os.path.join(base, "crates")
        v1, v2, v3, opts = decide(crates, ledger)
        cases = [
            ("opt-in detection", opts == {"alpha", "beta", "delta"}),
            ("V1 registered-but-disarmed caught", v1 == ["gamma"]),
            ("V2 opted-in-with-offenders caught", v2 == [("delta", 1)]),
            ("V3 opted-in-unregistered caught", v3 == ["beta"]),
            ("green case: ledger == opts, all clean",
             decide(*_green_workspace(base)) == ([], [], [], {"alpha"})),
        ]
        # cfg(test) exclusion fixture: offenders inside a #[cfg(test)] mod must
        # NOT count (V2 mirrors clippy's idiomatic test-module allowance).
        with open(os.path.join(os.path.join(base, "crates", "alpha", "src",
                                            "lib.rs")), "w", encoding="utf-8") as f:
            f.write('pub fn ok() -> u32 { 1 }\n\n#[cfg(test)]\nmod tests {\n'
                    '    #[test]\n    fn t() { let _ = Some(1).unwrap(); '
                    'let _ = "x".expect("y"); }\n}\n')
        _v1b, v2b, _v3b, _ob = decide(os.path.join(base, "crates"), ledger)
        cases.append(("cfg(test) offenders are exempt from V2",
                      all(c != "alpha" for c, _n in v2b)))
        with open(os.path.join(os.path.join(base, "crates", "alpha", "src",
                                            "lib.rs")), "a", encoding="utf-8") as f:
            f.write("pub fn bad() -> u32 { Some(1).unwrap() }\n")
        _v1c, v2c, _v3c, _oc = decide(os.path.join(base, "crates"), ledger)
        cases.append(("production offender still counts after exempt region",
                      any(c == "alpha" for c, _n in v2c)))
        # doc comments (/// //!): clippy does not lint them, so neither may V2
        with open(os.path.join(base, "crates", "alpha", "src", "lib.rs"), "w",
                  encoding="utf-8") as f:
            f.write('//! example: foo().unwrap()\n/// doc: bar().expect("x")\n'
                    'pub fn f() -> u32 { 1 }\n')
        _v1d0, v2d0, _v3d0, _o0 = decide(os.path.join(base, "crates"), ledger)
        cases.append(("doc-comment unwrap/expect is not an offender (clippy parity)",
                      all(c != "alpha" for c, _n in v2d0)))
        # multi-line raw strings must not skew brace counting (JSON fixtures
        # inside #[cfg(test)] stay exempt; a production offender after them
        # must still be counted)
        with open(os.path.join(base, "crates", "alpha", "src", "lib.rs"), "w",
                  encoding="utf-8") as f:
            f.write('#[cfg(test)]\nmod tests {\n    fn t() {\n'
                    '        let j = r#"{\n  "a": { "b": 1 }\n}"#;\n'
                    '        let _ = j.unwrap();\n    }\n}\n'
                    'pub fn ok() -> u32 { 1 }\n')
        _v1r, v2r, _v3r, _or = decide(os.path.join(base, "crates"), ledger)
        cases.append(("multi-line raw string keeps braces balanced (exempt region holds)",
                      all(c != "alpha" for c, _n in v2r)))
        # 5th structural landmine: a char literal containing a double quote
        # ('\"') used to open a fake string until EOF -- the cfg(test) window
        # never opened and every test-module offender counted as production
        # (measured on chimera-tui/actions/user_commands.rs, 2026-09-25).
        with open(os.path.join(base, "crates", "alpha", "src", "lib.rs"), "w",
                  encoding="utf-8") as f:
            f.write("pub fn t(s: &str) -> String { s.trim_matches('\"').to_string() }\n"
                    "#[cfg(test)]\nmod tests {\n    fn g() { let _ = Some(1).unwrap(); }\n}\n"
                    "pub fn u() { let _ = 'de_stays; Some(1).unwrap(); }\n")
        _v1c, v2c, _v3c, _oc = decide(os.path.join(base, "crates"), ledger)
        # alpha now has BOTH exempt-region offenders and one production offender
        # (the real-tree fix criterion: user_commands.rs counts 0 after the same
        # landmine was defused). n > 1 pre-fix: the fake string swallowed the
        # whole cfg(test) window and counted all 14 test offenders.
        cases.append(("char literal with a quote keeps states/regions sane",
                      any(c == "alpha" and n == 1 for c, n in v2c)))
        # 6th landmine: compound cfg(all(test, feature=..)) mod must be exempt
        # like plain cfg(test); cfg(not(test)) is production and must still count.
        with open(os.path.join(base, "crates", "alpha", "src", "lib.rs"), "w",
                  encoding="utf-8") as f:
            f.write('#[cfg(all(test, feature = "v3-engine"))]\nmod gated {\n'
                    '    fn g() { let _ = Some(1).unwrap(); }\n}\n'
                    '#[cfg(not(test))]\nmod prod {\n    fn p() { let _ = Some(1).unwrap(); }\n}\n')
        _v1f, v2f, _v3f, _of = decide(os.path.join(base, "crates"), ledger)
        cases.append(("compound cfg(all(test,..)) exempt while cfg(not(test)) counts",
                      any(c == "alpha" and n == 1 for c, n in v2f)))
        # file-level inner #![allow] exempts an integration-test/bench unit
        os.remove(os.path.join(base, "crates", "alpha", "src", "lib.rs"))
        with open(os.path.join(base, "crates", "alpha", "src", "bench_like.rs"),
                  "w", encoding="utf-8") as f:
            f.write('//! doc header\n#![allow(clippy::unwrap_used)]\n'
                    'fn t() { let _ = Some(1).unwrap(); }\n')
        _v1d, v2d, _v3d, _od = decide(os.path.join(base, "crates"), ledger)
        cases.append(("file-top inner allow exempts the unit",
                      all(c != "alpha" for c, _n in v2d)))
        os.remove(os.path.join(base, "crates", "alpha", "src", "bench_like.rs"))
        # regression guard: allow placed AFTER a long //! header block (>20 lines)
        with open(os.path.join(base, "crates", "alpha", "src", "long_head.rs"),
                  "w", encoding="utf-8") as f:
            f.write("//! h\n" * 30)
            f.write('#![allow(clippy::unwrap_used, clippy::expect_used)]\n'
                    'fn t() { let _ = Some(1).unwrap(); }\n')
        _v1e, v2e, _v3e, _oe = decide(os.path.join(base, "crates"), ledger)
        cases.append(("inner allow after a long //! header still exempts",
                      all(c != "alpha" for c, _n in v2e)))
        os.remove(os.path.join(base, "crates", "alpha", "src", "long_head.rs"))
        rc = 0
        for name, ok in cases:
            print("  [%s] %s" % ("PASS" if ok else "FAIL", name))
            if not ok:
                rc = 1
        print("[SELFTEST] %d/%d fixtures passed" % (len(cases) - rc, len(cases)))
        return rc
    finally:
        shutil.rmtree(base, ignore_errors=True)


def _green_workspace(base):
    g = os.path.join(base, "green")
    cdir = os.path.join(g, "crates", "alpha", "src")
    os.makedirs(cdir, exist_ok=True)
    with open(os.path.join(g, "crates", "alpha", "Cargo.toml"), "w",
              encoding="utf-8") as fh:
        fh.write('[package]\nname = "alpha"\nversion = "0.1.0"\n\n'
                 '[lints]\nworkspace = true\n')
    with open(os.path.join(cdir, "lib.rs"), "w", encoding="utf-8") as fh:
        fh.write("pub fn f() -> u32 { 1 }\n")
    scripts = os.path.join(g, "scripts")
    os.makedirs(scripts, exist_ok=True)
    ledger = os.path.join(scripts, "lint_ratchet_crates.txt")
    with open(ledger, "w", encoding="utf-8") as fh:
        fh.write("alpha # sole opt-in, clean\n")
    return os.path.join(g, "crates"), ledger


# ---------------------------------------------------------------------- main
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--emit", action="store_true")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()

    opts = {c for c in crate_dirs()
            if opted_in(os.path.join(CRATES, c, "Cargo.toml"))}
    if args.emit:
        print("# lint_ratchet_crates.txt -- crates carrying [lints] workspace=true")
        print("# Ratchet: only GROW. A row may be added only after")
        print("#   `cargo clippy -p <crate> --all-targets` is warning-free under")
        print("#   unwrap_used/expect_used (measure == 0). Removing an opt-in must")
        print("#   delete the row in the same commit (V1 otherwise fails).")
        for c in sorted(opts):
            print("%s  # opted in, measured clean at registration time" % c)
        return 0

    try:
        v1, v2, v3, opts_now = decide(CRATES, LEDGER)
    except ValueError as e:
        print("[UNDECIDABLE] %s" % e)
        return 2

    print("[INFO] crates scanned: %d; opted in: %d; ledger: %d"
          % (len(crate_dirs()), len(opts_now), len(load_ledger())))
    if not v1 and not v2 and not v3:
        print("[OK] lint ratchet holds: ledger == opt-ins, every registered "
              "crate measures 0 offenders")
        return 0
    for c in v1:
        print("  [V1] %s: registered but no [lints] workspace=true (opt-in "
              "disarmed silently)" % c)
    for c, n in v2:
        print("  [V2] %s: opted in yet %d unwrap/expect offender(s) across "
              "targets (lint would flood CI -D warnings)" % (c, n))
    for c in v3:
        print("  [V3] %s: opted in but NOT in the ledger (register it with "
              "the clippy-clean evidence, or drop the opt-in)" % c)
    return 1


if __name__ == "__main__":
    import gate_rc  # crash exits 2, never borrows 1 (F32/F33)
    sys.exit(gate_rc.run(main))
