#!/usr/bin/env python3
# =============================================================================
# check_sqlite_blocking.py - "rusqlite call inside async fn outside spawn_blocking"
# gate (arch quality deep-review, plan Phase 3 item P-3)
# =============================================================================
# Purpose: mechanically block the async concurrency debt class where a rusqlite
#          call (Connection::open*, .execute*/.query*/.prepare*) runs DIRECTLY
#          in an `async fn` body instead of inside a `spawn_blocking` closure.
#          A blocking SQLite call on a tokio worker thread starves the runtime:
#          other tasks on the same worker cannot make progress. The repo's own
#          convention (agents.md red line) is: rusqlite work goes through
#          spawn_blocking; this gate gives that red line teeth.
#
# WHY a lexical scope scanner (and not syn, and not the rejected 2026-09 heuristic):
#   The rejected first pass used a naive "big-brace depth after fn start"
#   heuristic whose readings it could not itself verify (3/4 false positives on
#   hand-checked fixtures; see arch-quality-deep-review-2026-09-25.md S6/P-3).
#   This scanner instead REUSES the architecture of check_await_across_guard.py:
#   the same strip_noise (strings + comments removed), the same line-by-line
#   depth advance with post-line scope release, and the same spawn_blocking
#   depth stack. That gate has run in CI for weeks with a measured, self-testing
#   detector over a planted-fixture tree -- proven in-repo precedent for exactly
#   this "call inside region X but not inside context Y" shape.
#
# FALSE-POSITIVE CLASSES THIS SCANNER MUST NOT REPORT (mirrors the await gate):
#   S1. call INSIDE a spawn_blocking closure (or the repo's pool.rs async
#       wrappers, which are spawn_blocking underneath) -- recommended
#       pattern, legal.
#   S2. call in a NON-async fn (sync fns may block; red line is async only).
#   S3. call inside a `#[cfg(test)]` region (dev-only; use --include-tests to
#       see them, verdict does not count them by default).
#   S4. call text inside strings/comments -- strip_noise removes them first.
#
# Allowlist: scripts/sqlite_blocking_allowlist.txt, one `<path>:<line>` per
#   finding with a trailing "# reason". Baseline policy per plan P-3: the first
#   run takes --emit current values as the allowlist (register-then-shrink,
#   only-decrease). A STALE entry (no longer matching any finding) FAILS the
#   gate -- an allowance must not outlive its debt (same discipline as
#   await_guard_allowlist.txt and check_retired_crate_residue.txt).
#
# Exit dialect (gate_rc): 0 clean / 1 judged red / 2 undecidable.
# Encoding: all-ASCII (project script convention, avoids CJK locale issues).
# Usage:
#   python scripts/check_sqlite_blocking.py             # the gate
#   python scripts/check_sqlite_blocking.py --emit      # print allowlist data
#   python scripts/check_sqlite_blocking.py --selftest  # planted fixtures tree
# =============================================================================
import argparse
import os
import re
import shutil
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATES = os.path.join(ROOT, "crates")
ALLOWLIST = os.path.join(ROOT, "scripts", "sqlite_blocking_allowlist.txt")

ASYNC_FN = re.compile(r"\basync\s+fn\s+[A-Za-z_]")
# Offload helpers: callables whose sync closure argument runs OUTSIDE the async
# task (tokio standard plus this repo's own pool wrappers, e.g. cmt-tiering's
# ConnectionPool::with_read_async/with_write_async -- pool.rs:153/159 -- which
# are spawn_blocking under their public names). Opening a legal context here
# mirrors the await gate's SYNC_CLOSURE handling; the closure body must be
# sync (a bare async closure would defeat the wrapper and is out of scope).
OFFLOAD_HELPERS = (
    "spawn_blocking",
    "with_read_async",
    "with_write_async",
    "with_any_read_async",
    "with_read_conn",
)
DB_CALL = re.compile(
    r"Connection::(?:open|open_in_memory|open_with_flags|open_in_memory_with_flags)"
    r"|\.execute(?:_batch|[_a-z]*)\s*\("
    r"|\.query(?:_row|_map|_and_then|[_a-z]*)?\s*\("
    r"|\.prepare(?:[_a-z]*)?\s*\("
)
TEST_MARKER = re.compile(r"#\[cfg\(test\)\]")
INCLUDE_TESTS = False  # flipped by scan_tree(include_tests=...)


def strip_noise(line):
    """Remove string literals and line comments so braces/parens are real.

    Same contract as check_await_across_guard.strip_noise (shared discipline,
    duplicated deliberately: gates must not import each other's internals).
    """
    out = []
    in_str = False
    i = 0
    while i < len(line):
        c = line[i]
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
        if c == "/" and i + 1 < len(line) and line[i + 1] == "/":
            break
        out.append(c)
        i += 1
    return "".join(out)


def scan_text(text, relpath):
    """Return findings: [(relpath, lineno)] for DB calls in async fn outside
    spawn_blocking.

    Region model: once `async fn` is seen, the async region spans from that
    line until the depth returns below the depth at the fn's first `{` (the
    body). spawn_blocking opens a legal context whose lifetime is the nested
    scope it appears in (depth stack, same as the await gate's sync_depths).
    """
    findings = []
    lines = text.split("\n")
    depth = 0
    async_body_depth = None   # depth inside current async fn body, None = outside
    async_regions_seen = 0
    sync_depths = []          # depths at which spawn_blocking closures opened
    in_test = False

    for lineno, raw in enumerate(lines, 1):
        if TEST_MARKER.search(raw):
            in_test = True
        code = strip_noise(raw)
        depth_at_line = depth

        # --- S3: cfg(test) regions are dev-only; skipped unless --include-tests
        # (sticky to end of file, same convention as check_await_across_guard)
        if in_test and not INCLUDE_TESTS:
            depth += code.count("{") - code.count("}")
            continue

        # --- enter async fn? (multi-line signatures: first '{' wins) ---
        if async_body_depth is None and ASYNC_FN.search(code):
            # body starts at the first '{' on/after this line; approximate the
            # await-gate convention: region ends when depth drops below the
            # depth at the '{'. We open the region optimistically at
            # depth_at_line+1 and close it when depth < that value.
            async_regions_seen += 1
            async_body_depth = depth_at_line + 1

        # --- offload legal context opens at this line's scope (S1) ---
        if any(h in code for h in OFFLOAD_HELPERS):
            sync_depths.append(depth_at_line)

        # --- verdict: DB call while inside async body and NOT in any legal ctx
        # (S3 skipping is handled solely by the in_test `continue` above -- a
        # second `not in_test` here would make --include-tests a no-op, which
        # the fixture for exactly that flag now guards against.)
        if (async_body_depth is not None and DB_CALL.search(code)
                and not sync_depths):
            findings.append((relpath, lineno))

        # --- advance depth AFTER attribution (same as await gate) ---
        depth += code.count("{") - code.count("}")

        # --- close async region when its body scope has been left ---
        if async_body_depth is not None and depth < async_body_depth:
            async_body_depth = None
        # --- close spawn_blocking contexts whose scope has been left ---
        sync_depths = [d for d in sync_depths if depth > d]

    return findings, async_regions_seen


def sqlite_crates():
    """Crate dirs whose Cargo.toml declares rusqlite (authoritative, dynamic)."""
    out = []
    if not os.path.isdir(CRATES):
        return out
    for name in sorted(os.listdir(CRATES)):
        manifest = os.path.join(CRATES, name, "Cargo.toml")
        if os.path.isfile(manifest):
            try:
                with open(manifest, encoding="utf-8", errors="replace") as fh:
                    if re.search(r"^\s*rusqlite\b", fh.read(), re.M):
                        out.append(name)
            except OSError:
                pass
    return out


def rel(path):
    return os.path.relpath(path, ROOT).replace("\\", "/")


def scan_tree(include_tests=False):
    global INCLUDE_TESTS
    INCLUDE_TESTS = include_tests
    findings = []
    scanned = 0
    crates = sqlite_crates()
    for crate in crates:
        src = os.path.join(CRATES, crate, "src")
        for dirpath, dirs, files in os.walk(src):
            dirs[:] = [d for d in dirs if d != "target"]
            for f in sorted(files):
                if not f.endswith(".rs"):
                    continue
                full = os.path.join(dirpath, f)
                with open(full, encoding="utf-8", errors="replace") as fh:
                    text = fh.read()
                got, _regions = scan_text(text, rel(full))
                findings.extend(got)
                scanned += 1
    return findings, scanned, crates


def load_allowlist(path=None):
    p = path or ALLOWLIST
    allow = set()
    if not os.path.exists(p):
        raise ValueError("allowlist missing: %s (take a baseline via --emit)" % p)
    with open(p, encoding="utf-8", errors="replace") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            m = re.match(r"^(.+):(\d+)$", line)
            if not m:
                raise ValueError("needs `path:line # reason`: %r" % raw.rstrip())
            allow.add((m.group(1), int(m.group(2))))
    return allow


def decide(findings, allow):
    """Compare findings against the allowlist. Returns (gap, stale)."""
    used = set()
    gap = []
    for path, lineno in findings:
        key = (path, lineno)
        if key in allow:
            used.add(key)
        else:
            gap.append(key)
    stale = sorted(allow - used)
    return gap, stale


# ------------------------------------------------------------------ selftest
def _fixture_tree(base):
    """Synthetic crates dir exercising every class S1-S4 + clean/dirty keys.

    Line numbers are computed from the source itself (marker scan) so the
    planted expectations can never drift from the fixture text.
    """
    crate = os.path.join(base, "crates", "fixture-store")
    src = os.path.join(crate, "src")
    os.makedirs(src)
    with open(os.path.join(crate, "Cargo.toml"), "w", encoding="utf-8") as fh:
        fh.write('[package]\nname = "fixture-store"\n\n'
                 '[dependencies]\nrusqlite = "0.37"\n')
    # line numbers matter: 1 blank, blocks separated deliberately below.
    body = '''
use rusqlite::Connection;

pub async fn bare_open() {          // L4: async fn
    let _c = Connection::open("x.db").unwrap();   // L5 GAP (S: async, outside)
}

pub fn sync_query() {               // L8: NOT async -> clean
    let _c = Connection::open_in_memory().unwrap();
    let _v = _c.query_row("SELECT 1", [], |_| Ok(1));   // clean (sync fn)
}

pub async fn offthread() {          // L13
    let _c = tokio::task::spawn_blocking(|| {           // L14: legal context
        let c = Connection::open("y.db").unwrap();      // L15 clean (S1)
        c.execute_batch("VACUUM").unwrap()              // L16 clean (S1)
    })
    .await;                                             // L18 await outside ctx
}

pub async fn string_noise() {       // L21
    let _s = "Connection::open(z.db) and .query(x)";    // L22 clean (S4)
    // Connection::open(w.db) in a comment .query_row   // L23 clean (S4)
}

pub async fn tail_gap() {           // placed BEFORE cfg(test): the in_test
    let c = rusqlite::Connection::open_in_memory().unwrap();
    c.execute("CREATE TABLE t(a)", []).unwrap();        // MARK_TAIL_GAP
}

pub async fn wrapper_offload() {     // S1': repo's own pool async wrappers
    let _r = pool
        .with_write_async(move |conn| {                  // MARK_WRAP_OPEN
            conn.execute("DELETE FROM t", []).unwrap()   // MARK_WRAP_INNER
        })
        .await;
}

#[cfg(test)]
mod tests {                         // L26
    #[tokio::test]
    async fn in_tests_query_row() { // S3: async ON PURPOSE -- without the
        let c = rusqlite::Connection::open_in_memory().unwrap(); // MARK_TEST_OPEN
        let _ = c.query_row("SELECT 1", [], |_| Ok(1)); // MARK_TEST_QUERY
    }
}
'''
    with open(os.path.join(src, "lib.rs"), "w", encoding="utf-8") as fh:
        fh.write(body)
    return crate, body


def selftest():
    global ROOT, CRATES, ALLOWLIST
    base = tempfile.mkdtemp(prefix="sqlite-gate-")
    orig_root, orig_crates, orig_allow = ROOT, CRATES, ALLOWLIST
    try:
        fixture_crates = os.path.join(base, "crates")
        _crate, fixture_body = _fixture_tree(base)  # in place at base/crates/*
        lines = fixture_body.split("\n")

        def ln(marker):
            """1-based line number of a planted marker inside the fixture."""
            hits = [i + 1 for i, l in enumerate(lines) if marker in l]
            assert len(hits) == 1, "marker %r hits=%d" % (marker, len(hits))
            return hits[0]

        L_BARE = ln("L5 GAP")
        L_SYNC1, L_SYNC2 = ln("// L8: NOT async"), ln("_v = _c.query_row")
        L_SB1, L_SB2 = ln('Connection::open("y.db")'), ln("execute_batch")
        L_STR = ln("z.db")
        L_TEST = ln("MARK_TEST_QUERY")
        L_TAIL = ln("MARK_TAIL_GAP")
        L_WRAP = ln("MARK_WRAP_INNER")
        ROOT = base
        CRATES = fixture_crates
        ALLOWLIST = os.path.join(base, "scripts", "sqlite_blocking_allowlist.txt")
        os.makedirs(os.path.dirname(ALLOWLIST), exist_ok=True)

        cases = []
        findings, scanned, crates = scan_tree()
        keys = sorted(findings)
        cases.append(("scope: only rusqlite crates scanned",
                      crates == ["fixture-store"] and scanned == 1))
        # planted GAPs: async bare open, async tail execute. planted cleans:
        # sync fn queries (S2), spawn_blocking body (S1), strings/comments (S4),
        # cfg(test) body (S3). Line numbers resolved from markers above.
        cases.append(("async bare Connection::open -> GAP",
                      ("crates/fixture-store/src/lib.rs", L_BARE) in keys))
        cases.append(("async execute -> GAP",
                      ("crates/fixture-store/src/lib.rs", L_TAIL) in keys))
        for target, why in ((L_SYNC1, "S2 sync fn"), (L_SYNC2, "S2 sync fn"),
                            (L_SB1, "S1 spawn_blocking"),
                            (L_SB2, "S1 spawn_blocking"),
                            (L_WRAP, "S1' with_write_async wrapper"),
                            (L_STR, "S4 string"), (L_TEST, "S3 cfg(test)")):
            cases.append(("no false positive at L%d (%s)" % (target, why),
                          ("crates/fixture-store/src/lib.rs", target)
                          not in keys))
        # allowlist mechanics: live keys suppress; unknown key = GAP stands;
        # stale key (no matching finding) fails. Register ALL findings so the
        # fixture cannot drift from the scanner's output (L27 lesson: every DB
        # line inside an async fn is its own finding).
        with open(ALLOWLIST, "w", encoding="utf-8") as fh:
            for p, n in sorted(findings):
                fh.write("%s:%d # planted ok\n" % (p, n))
        gap, stale = decide(findings, load_allowlist())
        cases.append(("exact allowlist suppresses all", not gap and not stale))
        gap2, stale2 = decide(findings, {(findings[0][0], 999)})
        cases.append(("unmatched allowlist keeps GAP",
                      len(gap2) == len(findings) and len(stale2) == 1))
        # empty crates dir on a fresh root is undecidable upstream (main exits 2
        # only for missing allowlist); missing allowlist -> ValueError (gate 2).
        os.remove(ALLOWLIST)
        try:
            load_allowlist()
            cases.append(("missing allowlist is loud", False))
        except ValueError:
            cases.append(("missing allowlist is loud", True))
        # include-tests flips S3 into a finding
        f_inc, _s, _c = scan_tree(include_tests=True)
        with open(ALLOWLIST, "w", encoding="utf-8") as fh:
            for p, n in sorted(set(findings) | set(f_inc)):
                fh.write("%s:%d # a\n" % (p, n))
        cases.append(("--include-tests surfaces the cfg(test) call",
                      ("crates/fixture-store/src/lib.rs", L_TEST) in f_inc
                      and ("crates/fixture-store/src/lib.rs", L_TEST)
                      not in findings))

        rc = 0
        for name, ok in cases:
            print("  [%s] %s" % ("PASS" if ok else "FAIL", name))
            if not ok:
                rc = 1
        print("[SELFTEST] %d/%d fixtures passed" % (len(cases) - rc, len(cases)))
        return rc
    finally:
        ROOT, CRATES, ALLOWLIST = orig_root, orig_crates, orig_allow
        shutil.rmtree(base, ignore_errors=True)


# ---------------------------------------------------------------------- main
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--emit", action="store_true")
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--include-tests", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()

    findings, scanned, crates = scan_tree(include_tests=args.include_tests)
    if not crates:
        print("[CONFIG] no crate declares rusqlite -- nothing to judge")
        return 2

    if args.emit:
        print("# sqlite_blocking_allowlist.txt -- only-decrease baseline "
              "for check_sqlite_blocking.py")
        print("# Row = `<path>:<line>  # reason`. Take via:")
        print("#   python scripts/check_sqlite_blocking.py --emit")
        print("# Register each with a real justification; new DB-in-async "
              "calls must go through spawn_blocking instead.")
        for path, lineno in sorted(findings):
            print("%s:%d  # baseline %s" % (path, lineno, "2026-09-25"))
        return 0

    try:
        allow = load_allowlist()
    except ValueError as e:
        print("[UNDECIDABLE] %s" % e)
        return 2

    gap, stale = decide(findings, allow)
    print("[INFO] rusqlite crates: %d; files scanned: %d; "
          "findings: %d; allowlist: %d"
          % (len(crates), scanned, len(findings), len(allow)))
    if not gap and not stale:
        print("[OK] no DB call in async fn outside spawn_blocking beyond the "
              "registered baseline")
        return 0
    if gap:
        print("[FAIL] %d new DB-call-in-async site(s) (red line: wrap in "
              "spawn_blocking):" % len(gap))
        for path, lineno in sorted(gap):
            print("  [GAP] %s:%d" % (path, lineno))
    if stale:
        print("[FAIL] %d stale allowlist row(s) -- delete them:" % len(stale))
        for path, lineno in stale:
            print("  [STALE] %s:%d" % (path, lineno))
    return 1


if __name__ == "__main__":
    import gate_rc  # crash must exit 2, never borrow 1 (F32/F33 discipline)
    sys.exit(gate_rc.run(main))
