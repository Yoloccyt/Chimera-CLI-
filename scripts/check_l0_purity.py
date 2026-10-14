#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L0 purity gate (G-99, 2026-10-12).

Purpose
-------
ADR-033 characterizes L0 `nexus-contracts` as "pure types + controlled pure
logic": no IO / no async / no network / no randomness / no external business
dependency (serde-derive only). Its 2026-10-09 addendum honestly registered a
residual gap: the "controlled" half had NO machine gate -- function purity was
policed only by WI-G1 human audit + unit tests. `check_perf_redlines.py` I1-I8c
do not cover function purity, and `check_dependency_rules.ps1` Check B only
catches upward *crate* dependencies, not impure symbols reached through an
allowed dependency or std.

This gate mechanically enforces the two hard, deterministic faces of ADR-033:

  1. Dependency allowlist  - every entry in [dependencies] of
     crates/nexus-contracts/Cargo.toml must be in {serde, chrono, uuid,
     thiserror}. Anything else (tokio / rand / regex / reqwest / wasmtime /
     rusqlite / ...) is a hard violation: L0 may not even *see* an impure crate.
     (dev-dependencies are test-only and are NOT policed here, matching ADR-033.)

  2. Impure-symbol scan     - for each crates/nexus-contracts/src/**/*.rs,
     comments (line + nestable block + doc) and string literals (normal + raw)
     are blanked by a small lexer, THEN the residual code is scanned for
     forbidden constructs (unsafe / async / .await / tokio / std::fs / std::net
     / std::io / std::process / TcpStream / rand:: / getrandom / println! ...).

The lexer is the whole point. A naive substring scan would false-positive on
`forbid(unsafe_code)`, doc-comments that *mention* "tokio::spawn" / "async", the
redline-enum string "redline-subscribe-before-spawn", and the `base_url`
"https://..." data field -- exactly the "confirm the counting scope before
judging" trap the ADR-033 addendum warns about. Blanking comments + strings
first makes the gate judge the *executable* surface only.

Modes / exit codes
------------------
  (default)   scan the real repo            0 = pure, 1 = impurity found
  --selftest  fixture injection proves the gate has teeth AND does not
              false-positive on comment/string mentions
                                            0 = all assertions held, 1 = not
  usage error                                 2

Output is pure ASCII (project script convention; avoids Windows GBK console
pitfalls). Time (`Utc::now`) is intentionally NOT forbidden: ADR-033's redline
list is IO/async/network/randomness, and chrono timestamp constructors are an
accepted "base constructor". ADR-212 decision 3 recorded that ruling and never
discussed the process environment; ADR-213 section 4 closes that by naming a
fifth class -- AMBIENT-INPUT (std::env::var/vars/var_os reads, set_var/
remove_var/set_current_dir writes). Unlike IO it has no visible side effect,
which is exactly why it is easy to miss and why a CI that *sets* those variables
can change a function's meaning without any code diff.

  3. Exemption ledger       - a file may be waived for ONE category (FILE_EXEMPT:
     path x category x evidence). Three properties are enforced: the category must
     sit in a closed vocabulary, the evidence must clear a length floor, and a row
     whose token has left the PRODUCTION surface is STALE (red) -- an exemption
     that outlives its reason is how ledgers rot. "Production surface" excludes
     `#[cfg(test)] mod` blocks, matching judgment 1's dev-dependency policy
     (test-only code is not policed); the tail is brace-closed, because an
     open-ended tail would let the first test module in a file disarm the rest
     of that file (the bug that made this gate's teeth partial).
"""

import os
import re
import sys

# --- configuration -----------------------------------------------------------

ALLOW_DEPS = {"serde", "chrono", "uuid", "thiserror"}

# (rule-name, category, regex) applied to the comment/string-blanked source.
# CATEGORIES are the impurity classes ADR-033 names; a file may be exempted only
# for a category it is listed under (see FILE_EXEMPT), so "exempt this file" can
# never silently cover a NEW class of impurity introduced later in the same file.
FORBIDDEN = [
    ("unsafe-block", "IO-EFFECT", r"\bunsafe\b"),
    ("async", "IO-EFFECT", r"\basync\b"),
    ("await", "IO-EFFECT", r"\.await\b"),
    ("tokio", "IO-EFFECT", r"\btokio\b"),
    ("std-fs", "IO-EFFECT", r"\bstd::fs\b"),
    ("std-net", "IO-EFFECT", r"\bstd::net\b"),
    ("std-io", "IO-EFFECT", r"\bstd::io\b"),
    ("std-process", "IO-EFFECT", r"\bstd::process\b"),
    ("net-socket", "IO-EFFECT", r"\bTcpStream\b|\bUdpSocket\b"),
    ("random", "IO-EFFECT", r"\brand::|\bgetrandom\b|thread_rng"),
    ("stdout-write", "IO-EFFECT", r"\bprintln!|\bprint!|\beprintln!|\beprint!|\bdbg!"),
    # ADR-213 section 4 -- fifth impurity class. An ambient input is a read whose
    # result is decided by the process environment rather than by the arguments;
    # unlike IO it has no visible side effect, which is exactly why it is easy to
    # miss and why CI (which SETS these variables) can change a function's meaning
    # without any code diff. ADR-212 decision 3 exempted time (Utc::now) on purpose
    # and never discussed env; this closes that hole instead of leaving it implied.
    ("env-read", "AMBIENT-INPUT", r"\bstd::env::(?:var|vars|var_os)\b"),
    ("env-write", "AMBIENT-INPUT",
     r"\bstd::env::(?:set_var|remove_var|set_current_dir|set_experimental)\b"),
]

FORBIDDEN_RE = [(name, cat, re.compile(rx)) for name, cat, rx in FORBIDDEN]

# Closed vocabulary of exemption categories. An unknown category is a ledger
# error, not a finding: the gate cannot judge -> exit 2 (never borrow a 1 that
# would read as "checked and red"). Mirrors check_write_sensitive_instrumentation
# .py's EXEMPT_CATEGORIES discipline (G-92).
IMPURITY_CATEGORIES = ("AMBIENT-INPUT", "IO-EFFECT")

# Minimum reason length in characters. A one-word reason ("test") is not evidence;
# the sibling gate enforces the same rule for the same reason.
MIN_EXEMPT_REASON_LEN = 24

# (crate-relative path, category, reason). Read relative to crates/nexus-contracts/.
# Both entries are the control-plane/test-plane scale readers: CODE_WIKI section 3.0
# already self-declares perf_scale/perf_scale_ms as non-pure and "production must not
# depend on them"; test_scale reads CHIMERA_TEST_TIMEOUT_SCALE which CI injects to
# shrink hard-coded waits. They stay public because benches and 9 test files across
# 8 crates consume them (ADR-213 decision R2 rejected moving them out of L0).
FILE_EXEMPT = [
    ("src/vector_math/util.rs", "AMBIENT-INPUT",
     "perf_scale()/perf_scale_ms() read CHIMERA_PERF_SCALE; control-plane only, "
     "CODE_WIKI 3.0 forbids production dependence"),
    ("src/testutil/test_scale.rs", "AMBIENT-INPUT",
     "reads CHIMERA_TEST_TIMEOUT_SCALE for test waits only; ADR-033 exception "
     "registry lists test_scale"),
]

# Test-tail exclusion: a `#[cfg(test)]` line starts an excluded tail only when the
# next code line declares a module. The START condition is byte-identical to
# check_contract_consumers.py::read_production_text and
# check_dead_contract_apis.py::count_production_uses, so "production surface" means
# the same thing in all three gates (they deliberately do NOT import each other: a
# bug in one gate must not change another gate's verdict).
# The END is NOT shared: those two gates only need to drop definitions, so an
# open-ended tail was harmless there. A purity judge cannot afford that -- with an
# open-ended tail the FIRST `#[cfg(test)] mod` in vector_math/util.rs (line 93)
# masked the rest of the file, including the production `perf_scale()` at line 470,
# which is exactly the finding this gate exists to catch. The tail therefore closes
# when the block that opened it closes (brace depth returns to the level the
# attribute was seen at). Braces inside comments/strings are already blanked away.
CFG_TEST_RE = re.compile(r"^\s*#\[cfg\(test\)\]")
MODULE_DECL_RE = re.compile(r"^\s*(pub(\([^)]*\))?\s+)?mod\s+\w")
SKIP_IN_LOOKAHEAD = ("#", "//", "/*", "*")

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATE_DIR = os.path.join(ROOT, "crates", "nexus-contracts")
CARGO_TOML = os.path.join(CRATE_DIR, "Cargo.toml")
SRC_DIR = os.path.join(CRATE_DIR, "src")


# --- Rust comment/string blanking lexer --------------------------------------

def blank_rust(src):
    """Return `src` with comments and string/char-ish literals replaced by
    spaces, preserving every newline so byte offsets -> line numbers still map.

    Handles: line comments (//...), nestable block comments (/* ... */), raw
    strings (r"..." / r#"..."# / r##"..."##), normal strings ("..." with escapes).
    Char literals and lifetimes are left in CODE (harmless: they never contain
    forbidden path tokens), which keeps this lexer small and robust.
    """
    out = []
    i = 0
    n = len(src)
    while i < n:
        c = src[i]

        # raw string: r / r# / r## ... then '"'
        if c == "r":
            j = i + 1
            hashes = 0
            while j < n and src[j] == "#":
                hashes += 1
                j += 1
            if hashes <= 8 and j < n and src[j] == '"':
                closer = '"' + "#" * hashes
                start_content = j + 1
                end = src.find(closer, start_content)
                if end != -1:
                    seg = src[i:end + len(closer)]
                    out.append(_blank_keep_nl(seg))
                    i = end + len(closer)
                    continue
            # else: identifier starting with 'r' -> fall through to CODE path

        # line comment
        if c == "/" and i + 1 < n and src[i + 1] == "/":
            while i < n and src[i] != "\n":
                out.append(" ")
                i += 1
            continue

        # block comment (nestable)
        if c == "/" and i + 1 < n and src[i + 1] == "*":
            out.append("  ")
            i += 2
            depth = 1
            while i < n and depth > 0:
                if src[i] == "/" and i + 1 < n and src[i + 1] == "*":
                    depth += 1
                    out.append("  ")
                    i += 2
                elif src[i] == "*" and i + 1 < n and src[i + 1] == "/":
                    depth -= 1
                    out.append("  ")
                    i += 2
                else:
                    out.append("\n" if src[i] == "\n" else " ")
                    i += 1
            continue

        # string literal
        if c == '"':
            out.append(" ")
            i += 1
            while i < n and src[i] != '"':
                if src[i] == "\\":
                    out.append(" ")
                    i += 1
                    if i < n:
                        out.append("\n" if src[i] == "\n" else " ")
                        i += 1
                else:
                    out.append("\n" if src[i] == "\n" else " ")
                    i += 1
            if i < n:  # closing quote
                out.append(" ")
                i += 1
            continue

        out.append(c)
        i += 1
    return "".join(out)


def _blank_keep_nl(s):
    return "".join("\n" if ch == "\n" else " " for ch in s)



# --- real scan ---------------------------------------------------------------

def scan_dependencies():
    """Return list of violation strings for [dependencies] allowlist breach."""
    violations = []
    if not os.path.isfile(CARGO_TOML):
        return ["Cargo.toml not found at " + CARGO_TOML]
    section = ""
    with open(CARGO_TOML, "r", encoding="utf-8") as fh:
        for raw in fh:
            line = raw.strip()
            if line.startswith("[") and line.endswith("]"):
                section = line[1:-1].strip()
                continue
            if section == "dependencies":
                m = re.match(r"^([A-Za-z0-9_.\-]+)\s*=", line)
                if not m:
                    continue
                dep = m.group(1)
                if dep not in ALLOW_DEPS:
                    violations.append(
                        "Cargo.toml [dependencies]: disallowed dep '%s' "
                        "(allowlist: %s)" % (dep, ", ".join(sorted(ALLOW_DEPS)))
                    )
    return violations


def iter_src_files():
    files = []
    for dirpath, _dirnames, filenames in os.walk(SRC_DIR):
        for fn in filenames:
            if fn.endswith(".rs"):
                files.append(os.path.join(dirpath, fn))
    return sorted(files)


EXEMPT_SET = {(rel, cat) for rel, cat, _reason in FILE_EXEMPT}


def validate_ledger(exempt=None):
    """Ledger self-consistency. Errors here mean the gate cannot judge -> rc 2.

    Parameterised so --selftest can feed it a deliberately broken ledger: the two
    guard rails (closed category set, reason-length floor) have to be proven to
    FIRE, otherwise they are decoration and a malformed row would be ignored.
    """
    rows = FILE_EXEMPT if exempt is None else exempt
    errs = []
    seen = set()
    for rel, cat, reason in rows:
        if cat not in IMPURITY_CATEGORIES:
            errs.append("FILE_EXEMPT %s: unknown category '%s' (allowed: %s)"
                        % (rel, cat, "/".join(IMPURITY_CATEGORIES)))
        if len(reason) < MIN_EXEMPT_REASON_LEN:
            errs.append("FILE_EXEMPT %s/%s: reason is %d chars, below the %d-char "
                        "evidence floor" % (rel, cat, len(reason), MIN_EXEMPT_REASON_LEN))
        key = (rel, cat)
        if key in seen:
            errs.append("FILE_EXEMPT: duplicate key %s/%s" % (rel, cat))
        seen.add(key)
    return errs


def stale_entries(exempt_set, seen_pairs):
    """Exemption rows whose category token has left the file -- must be deleted."""
    return [(rel, cat) for rel, cat in exempt_set if (rel, cat) not in seen_pairs]


def test_tail_mask(code_lines):
    """True = line belongs to a `#[cfg(test)] mod ...` block (not production surface).

    Depth-tracked so the tail closes with its block; see the CFG_TEST_RE comment
    for why an open-ended tail would disarm this particular gate.
    """
    mask = [False] * len(code_lines)
    depth = 0
    i = 0
    n = len(code_lines)
    while i < n:
        line = code_lines[i]
        if CFG_TEST_RE.match(line):
            base = depth
            j = i + 1
            while j < n:
                s = code_lines[j].strip()
                if not s or s.startswith(SKIP_IN_LOOKAHEAD):
                    j += 1
                    continue
                break
            if j < n and MODULE_DECL_RE.match(code_lines[j]):
                # Tail covers the attribute line through the line closing the block.
                # `opened` guards the attribute line itself: it carries no brace, so
                # "depth <= base" is already true on it and would end the tail at once.
                k = i
                opened = False
                while k < n:
                    mask[k] = True
                    depth += code_lines[k].count("{") - code_lines[k].count("}")
                    if depth > base:
                        opened = True
                    k += 1
                    if opened and depth <= base:
                        break
                i = k
                continue
            # cfg(test) on a non-mod item (a bare test fn): only the attribute line
            # is dropped, the item body stays in the production scan. Over-scan here
            # is deliberate -- a judge must err toward looking, not toward ignoring.
            mask[i] = True
            depth += line.count("{") - line.count("}")
            i += 1
            continue
        depth += line.count("{") - line.count("}")
        i += 1
    return mask


def scan_text(rel, src, exempt_set):
    """Pure one-file scan: (hits, exempted). Shared by the real scan and --selftest,
    so a fixture exercises the SAME rules the repo is judged by (a fixture that only
    calls a test-only copy of the logic proves nothing about production teeth)."""
    code = blank_rust(src)
    code_lines = code.splitlines()
    src_lines = src.splitlines()
    tail = test_tail_mask(code_lines)
    hits = []
    exempted = []
    for i, cl in enumerate(code_lines):
        if tail[i]:
            continue
        for name, cat, rx in FORBIDDEN_RE:
            if not rx.search(cl):
                continue
            snippet = src_lines[i].strip() if i < len(src_lines) else ""
            entry = (rel, i + 1, name, cat, snippet)
            (exempted if (rel, cat) in exempt_set else hits).append(entry)
    return hits, exempted


def scan_symbols():
    """Return (hits, exempted, stale) over the real crate source."""
    hits = []
    exempted = []
    seen_pairs = set()
    for path in iter_src_files():
        with open(path, "r", encoding="utf-8") as fh:
            src = fh.read()
        rel = os.path.relpath(path, CRATE_DIR).replace(os.sep, "/")
        file_hits, file_exempt = scan_text(rel, src, EXEMPT_SET)
        hits.extend(file_hits)
        exempted.extend(file_exempt)
        for _r, _l, _n, cat, _s in file_exempt:
            seen_pairs.add((rel, cat))
    return hits, exempted, stale_entries(EXEMPT_SET, seen_pairs)


def run_real():
    ledger_errors = validate_ledger()
    for e in ledger_errors:
        print("[TOOL-ERROR][ledger] " + e)
    if ledger_errors:
        # Undecidable, not "clean": a broken ledger must never read as a pass.
        print("[TOOL-ERROR] L0 purity gate: exemption ledger invalid (%d error(s))"
              % len(ledger_errors))
        return 2
    dep_violations = scan_dependencies()
    sym_hits, exempted, stale = scan_symbols()
    for v in dep_violations:
        print("[FAIL][dep] " + v)
    for rel, ln, name, _cat, snippet in sym_hits:
        print("[FAIL][sym] %s:%d %s -> %s" % (rel, ln, name, snippet))
    for rel, cat in sorted(stale):
        print("[FAIL][stale] FILE_EXEMPT %s/%s is no longer needed (the token is "
              "gone) -- delete the line in the same commit" % (rel, cat))
    total = len(dep_violations) + len(sym_hits) + len(stale)
    if total == 0:
        cats = "/".join(sorted({cat for _rel, cat in EXEMPT_SET})) or "-"
        print("[OK] L0 purity gate: %d src files scanned (comments+strings blanked, "
              "cfg(test) tails excluded), [dependencies] subset of {%s}; "
              "0 impurity, %d exempted under %s" %
              (len(iter_src_files()), ", ".join(sorted(ALLOW_DEPS)),
               len(exempted), cats))
        return 0
    print("[FAIL] L0 purity gate: %d violation(s) (dep allowlist: %d, symbol: %d, "
          "stale exemption: %d)" % (total, len(dep_violations), len(sym_hits), len(stale)))
    return 1


# --- selftest (proves teeth + no false-positive) ------------------------------

# A clean fixture MUST yield zero hits: forbidden words appear ONLY inside
# comments / doc-comments / string literals (mirrors the real base_url +
# redline-enum + doc-comment situation), plus forbid(unsafe_code) attribute.
CLEAN = (
    '#![forbid(unsafe_code)]\n'
    '//! mentions tokio::spawn and async fn in docs, and a redline string.\n'
    '// std::fs is only discussed here, not called.\n'
    'pub struct Spec {\n'
    '    pub base_url: String,\n'
    '}\n'
    'impl Spec {\n'
    '    pub fn new() -> Self {\n'
    '        Self { base_url: "https://example.com/api//v4".to_string() }\n'
    '    }\n'
    '    pub const REDLINE: &str = "redline-subscribe-before-spawn";\n'
    '}\n'
    'trait Runner {\n'
    '    fn run(&self) -> Result<(), ()>;\n'  # plain code, no forbidden token
    '}\n'
)

# Dirty fixture: each line injects one distinct forbidden construct in CODE.
DIRTY = [
    ("unsafe-block", "fn f() { unsafe { core::ptr::null::<u8>(); } }"),
    ("async", "pub async fn g() {}"),
    ("await", "async fn h() { work().await; }"),
    ("tokio", "fn i() { tokio::spawn(async {}); }"),
    ("std-fs", "fn j() { let _ = std::fs::read(\"x\"); }"),
    ("std-net", "fn k() { let _ = std::net::TcpListener::bind(\"1\"); }"),
    ("std-io", "fn l() { let _ = std::io::stdout(); }"),
    ("std-process", "fn m() { let _ = std::process::id(); }"),
    ("net-socket", "fn n() { let _ = TcpStream::connect(\"x\"); }"),
    ("random", "fn o() { let _ = rand::thread_rng(); }"),
    ("stdout-write", "fn p() { println!(\"hi\"); }"),
    # ADR-213 section 4: the fifth class. Without these two lines the gate would keep
    # reporting "0 impurity" while a helper's meaning changed with the CI environment.
    ("env-read", "fn q() { let _ = std::env::var(\"CHIMERA_PERF_SCALE\"); }"),
    ("env-write", "fn r() { std::env::set_var(\"A\", \"b\"); }"),
]


def detect_in(text):
    """Run the SAME scanner the real crate is judged by (no exemptions applied).

    Deliberately not a test-only copy of the logic: a fixture calling its own
    mini-scanner could stay green while the production scan was broken.
    """
    hits, _exempted = scan_text("fixture.rs", text, set())
    return [h[2] for h in hits]


def run_selftest():
    ok = True
    clean_found = detect_in(CLEAN)
    if clean_found:
        ok = False
        print("[SELFTEST-FAIL] clean fixture (comments/strings only) wrongly "
              "flagged: %s" % ", ".join(clean_found))
    else:
        print("[SELFTEST] clean fixture: 0 false positives (comments/strings "
              "blanked correctly)")

    for expected, src in DIRTY:
        found = detect_in(src)
        if expected not in found:
            ok = False
            print("[SELFTEST-FAIL] dirty line for '%s' not detected: %s"
                  % (expected, src))
        else:
            print("[SELFTEST] detected '%s' as expected" % expected)

    # --- ADR-213: the exemption ledger must be a NARROW gate, not an off switch.
    def check(label, cond):
        nonlocal ok
        if cond:
            print("[SELFTEST] %s" % label)
        else:
            ok = False
            print("[SELFTEST-FAIL] %s" % label)

    ENV_SRC = 'fn q() { let _ = std::env::var("S"); }\n'
    h, e = scan_text("a.rs", ENV_SRC, {("a.rs", "AMBIENT-INPUT")})
    check("ledger exempts the listed category (0 hits, 1 exempted)",
          not h and len(e) == 1)
    # The same file, now also printing: an AMBIENT-INPUT exemption must NOT cover it.
    MIXED = ENV_SRC + 'fn p() { println!("x"); }\n'
    h, e = scan_text("a.rs", MIXED, {("a.rs", "AMBIENT-INPUT")})
    check("exemption cannot blanket a different category in the same file",
          len(h) == 1 and h[0][2] == "stdout-write" and len(e) == 1)
    # Ghost exemption: ledger row whose token has gone must be reported stale.
    check("ghost exemption is stale (token no longer present)",
          stale_entries({("a.rs", "AMBIENT-INPUT")}, set()) == [("a.rs", "AMBIENT-INPUT")])
    check("live exemption is not stale",
          stale_entries({("a.rs", "AMBIENT-INPUT")}, {("a.rs", "AMBIENT-INPUT")}) == [])
    # The lexer must not over-fire on env either: prose and string data mentioning
    # std::env::var are the exact case a naive substring scan would misjudge.
    CLEAN_ENV = ('//! documents std::env::var here, does not call it\n'
                 'pub const DOC: &str = "std::env::var in a string literal";\n')
    check("env token inside comment/string is not flagged",
          not scan_text("a.rs", CLEAN_ENV, set())[0])
    # cfg(test) tail rule, pinned with the two-condition semantics.
    TAIL = ('#[cfg(test)]\nmod t {\n'
            '    fn x() { println!("y"); }\n}\n')
    check("cfg(test) mod tail excluded from production surface",
          not scan_text("a.rs", TAIL, set())[0])
    NOTAIL = '#[cfg(test)]\nfn x() { println!("y"); }\n'
    check("cfg(test) on a non-mod line does NOT start a tail",
          len(scan_text("a.rs", NOTAIL, set())[0]) == 1)
    # The tail must CLOSE. Without this the first `#[cfg(test)] mod` in a file disarms
    # the rest of it -- vector_math/util.rs has its first test mod at line 93 and the
    # production perf_scale() at 470, which is the case that made this a real bug.
    CLOSED = ('#[cfg(test)]\nmod t {\n    fn x() { println!("y"); }\n}\n'
              'pub fn prod() { println!("z"); }\n')
    check("tail closes: production item after a test mod is still scanned",
          len(scan_text("a.rs", CLOSED, set())[0]) == 1)
    NESTED = ('pub fn holder() {\n    #[cfg(test)]\n    mod inner {\n'
              '        fn a() { println!("t"); }\n    }\n'
              '    println!("p");\n}\n')
    hits_nested = scan_text("a.rs", NESTED, set())[0]
    check("tail closes inside a nested block too (only the outer item flags)",
          len(hits_nested) == 1 and hits_nested[0][1] == 6)
    # Ledger validation guards must actually fire (an always-false guard is noise).
    check("unknown category rejected",
          bool(validate_ledger([("a.rs", "NOT-A-THING", "x" * 40)])))
    check("short reason rejected",
          bool(validate_ledger([("a.rs", "AMBIENT-INPUT", "test")])))
    check("duplicate key rejected",
          len(validate_ledger([("a.rs", "AMBIENT-INPUT", "x" * 40),
                               ("a.rs", "AMBIENT-INPUT", "y" * 40)])) == 1)
    check("real ledger passes its own guards",
          validate_ledger() == [])

    # dependency allowlist teeth: a synthetic Cargo.toml fragment with tokio.
    bad = "serde = { workspace = true }\ntokio = { workspace = true }\n"
    disallowed = [ln.split("=")[0].strip() for ln in bad.splitlines()
                  if re.match(r"^[A-Za-z0-9_.\-]+\s*=", ln.strip())
                  and ln.split("=")[0].strip() not in ALLOW_DEPS]
    if "tokio" not in disallowed:
        ok = False
        print("[SELFTEST-FAIL] dependency allowlist did not flag 'tokio'")
    else:
        print("[SELFTEST] dependency allowlist flags 'tokio' as expected")

    print("[SELFTEST] %s" % ("ALL ASSERTIONS HELD" if ok else "FAILED"))
    return 0 if ok else 1


def main(argv):
    args = [a for a in argv[1:]]
    if any(a not in ("--selftest",) for a in args):
        sys.stderr.write("usage: check_l0_purity.py [--selftest]\n")
        return 2
    if "--selftest" in args:
        return run_selftest()
    return run_real()


if __name__ == "__main__":
    # F37 entry contract: a crash must exit 2 (undecidable), never borrow a
    # traceback exit-1 to fake "checked and red". Follows the gate family's
    # gate_rc.run precedent (check_topk_registry et al.).
    import gate_rc

    sys.exit(gate_rc.run(lambda: main(sys.argv)))
