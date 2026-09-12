#!/usr/bin/env python3
# =============================================================================
# check_await_across_guard.py - "lock held across .await" gate (arch direction 2)
# =============================================================================
# Purpose: mechanically block the async concurrency debt class where a lock
#          guard (std/parking_lot/tokio Mutex, RwLock, DashMap RefMut) is still
#          ALIVE at an `.await` point inside the same async fn.
#
#          Holding a guard across .await => (1) other tasks queue on the same
#          lock, (2) tokio worker starvation, (3) possible deadlock.
#
# WHY a bespoke scanner instead of clippy::await_holding_lock:
#   clippy's lint covers std::sync/parking_lot guards but NOT `DashMap::RefMut`,
#   which is exactly what bit us in nexus-app-server (see below). It also cannot
#   express "this guard lives inside a spawn_blocking closure, so it is fine".
#
# FALSE-POSITIVE CLASSES THIS SCANNER MUST NOT REPORT (all three were real
# false positives in the first-pass audit of 2026-09-10):
#   F1. `.lock().await` / `.write().await` / `.read().await` - the await merely
#       ACQUIRES the lock; it does not hold it.
#   F2. guard acquired INSIDE a `spawn_blocking(...)` closure - the closure body
#       is synchronous, the guard drops when the closure returns, and the outer
#       `.await` is on the JoinHandle. This is the RECOMMENDED fix for sync IO
#       (rusqlite), not a defect.
#   F3. guard released by block scope (`{ let g = ...; }` then `.await` after the
#       closing brace) or by an explicit `drop(g)`.
#
# Allowlist: guards that MUST stay held across .await, or that the scanner
#   mis-attributes (a value taken OUT of a lock, e.g. `*guard` / `.take()`).
#   Keyed by the ACQUISITION line, so one entry covers every await under it.
#   A stale entry (one that no longer matches any finding) FAILS the gate -
#   an allowlist that can rot is worse than no allowlist. See
#   scripts/await_guard_allowlist.txt for the per-entry rationale.
#
# Encoding: all-ASCII (project script convention, avoids CJK locale issues in CI)
# Exit code: 0 = clean, 1 = gap found, 2 = usage error
# Usage:
#   python scripts/check_await_across_guard.py
#   python scripts/check_await_across_guard.py --selftest
#   python scripts/check_await_across_guard.py --include-tests
# =============================================================================
import argparse
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATES = os.path.join(ROOT, "crates")
ALLOWLIST = os.path.join(ROOT, "scripts", "await_guard_allowlist.txt")

# Guard acquisition: a `let` binding whose initializer takes a lock.
LET_BIND = re.compile(r"\blet\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=")
LOCK_CALL = (
    ".lock(",
    ".write(",
    ".read(",
    ".get_mut(",
    ".entry(",
    ".try_lock(",
    ".try_write(",
)
# An await that only ACQUIRES the lock (F1) - never a holding violation.
ACQUIRE_AWAIT = re.compile(r"\.(?:lock|write|read|try_lock|try_write)\(\s*\)\.await")
AWAIT = re.compile(r"\.await\b")
DROP_CALL = re.compile(r"\bdrop\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)\s*\)")
SYNC_CLOSURE = "spawn_blocking"
TEST_MARKER = re.compile(r"#\[cfg\(test\)\]")


def strip_noise(line: str) -> str:
    """Remove string literals and line comments so braces/parens are real."""
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


class Guard:
    __slots__ = ("name", "line", "depth", "in_sync_closure")

    def __init__(self, name, line, depth, in_sync_closure):
        self.name = name
        self.line = line
        self.depth = depth
        self.in_sync_closure = in_sync_closure


def scan_text(text, path):
    """Return list of (path, await_line, guard_name, guard_line)."""
    global INCLUDE_TESTS
    gaps = []
    lines = text.split("\n")
    depth = 0
    guards = []
    sync_depths = []  # depths at which a spawn_blocking( closure opened
    pending_let = None  # let-binding whose initializer is not yet terminated by ';'
    in_test = False

    for lineno, raw in enumerate(lines, 1):
        if TEST_MARKER.search(raw):
            in_test = True
        code = strip_noise(raw)
        depth_at_line = depth

        if in_test and not INCLUDE_TESTS:
            depth += code.count("{") - code.count("}")
            continue

        # --- 1. await while a guard is still alive (F1/F2 filtered) ---
        if AWAIT.search(code) and not ACQUIRE_AWAIT.search(code):
            for g in guards:
                if not g.in_sync_closure:
                    gaps.append((path, lineno, g.name, g.line))

        # --- 2. explicit drop(g) releases that guard (F3) ---
        m = DROP_CALL.search(code)
        if m:
            guards = [g for g in guards if g.name != m.group(1)]

        # --- 3. open a synchronous spawn_blocking context? (F2) ---
        if SYNC_CLOSURE in code:
            sync_depths.append(depth_at_line)

        # --- 4. attribute lock calls to the correct let-binding, in source order
        # WHY position-ordered: `let v = { let g = lock.lock()...; *g };` must
        # attribute .lock() to `g` (depth+1, released at the '}' on the same
        # line), never to `v`. A naive "first let on the line" mis-attributes it
        # and reports `v` as held across the following await (false positive).
        events = [(m.start(), 0, m.group(1)) for m in LET_BIND.finditer(code)]
        lock_pos = min([code.index(t) for t in LOCK_CALL if t in code], default=None)
        if lock_pos is not None:
            events.append((lock_pos, 1, None))
        for i, c in enumerate(code):
            if c == ";":
                events.append((i, 2, None))
        events.sort(key=lambda e: e[0])

        for pos, kind, name in events:
            if kind == 0:
                pending_let = name
            elif kind == 1:
                if pending_let:
                    inner = depth_at_line + code[:pos].count("{") - code[:pos].count("}")
                    guards.append(
                        Guard(pending_let, lineno, inner, bool(sync_depths))
                    )
            else:
                pending_let = None

        # --- 5. advance depth, then drop what the closed scopes released (F3) ---
        depth += code.count("{") - code.count("}")
        guards = [g for g in guards if depth >= g.depth]
        sync_depths = [d for d in sync_depths if depth > d]

    return gaps


def load_allowlist():
    """Return set of (posix_relpath, line) entries explicitly accepted."""
    entries = set()
    if not os.path.exists(ALLOWLIST):
        return entries
    with open(ALLOWLIST, encoding="utf-8", errors="ignore") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            if ":" not in line:
                continue
            path, _, lineno = line.rpartition(":")
            try:
                entries.add((path.strip().replace("\\", "/").lstrip("./"), int(lineno)))
            except ValueError:
                continue
    return entries


def scan_tree():
    gaps = []
    for crate in sorted(os.listdir(CRATES)):
        src = os.path.join(CRATES, crate, "src")
        if not os.path.isdir(src):
            continue
        for dirpath, _, files in os.walk(src):
            for f in sorted(files):
                if not f.endswith(".rs"):
                    continue
                p = os.path.join(dirpath, f)
                with open(p, encoding="utf-8", errors="ignore") as fh:
                    gaps.extend(scan_text(fh.read(), os.path.relpath(p, ROOT)))
    return gaps


# ---------------------------------------------------------------- self-test
SELFTEST_SRC = """
async fn bad(lock: &Mutex<u8>) {
    let g = lock.lock().unwrap();
    do_io().await;
    drop(g);
}

async fn good_block(lock: &Mutex<u8>) {
    let v = { let g = lock.lock().unwrap(); *g };
    do_io().await;
    let _ = v;
}

async fn good_spawn(lock: Arc<Mutex<u8>>) {
    let c = Arc::clone(&lock);
    tokio::task::spawn_blocking(move || {
        let g = c.lock().unwrap();
        *g
    }).await.unwrap();
}

async fn good_acquire(lock: &tokio::sync::Mutex<u8>) {
    let g = lock.lock().await;
    let _ = *g;
}
"""


def selftest():
    global INCLUDE_TESTS
    INCLUDE_TESTS = False
    gaps = scan_text(SELFTEST_SRC, "<selftest>")
    bad = [g for g in gaps if "bad" in str(g[1])]
    ok = True
    if len(gaps) != 1:
        print("[SELFTEST] expected exactly 1 violation, got %d: %s" % (len(gaps), gaps))
        ok = False
    elif gaps[0][1] != 4:
        print("[SELFTEST] violation should be reported at line 4, got %s" % (gaps[0],))
        ok = False
    if not ok:
        return 1
    print("[SELFTEST] all constructed cases classified correctly:")
    print("           1 violation (bad) / 3 false positives suppressed")
    print("           (block-scoped, spawn_blocking closure, acquire-only await)")
    return 0


INCLUDE_TESTS = False


def main():
    global INCLUDE_TESTS
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--include-tests", action="store_true")
    args = ap.parse_args()
    INCLUDE_TESTS = args.include_tests

    if args.selftest:
        return selftest()

    raw_gaps = scan_tree()
    allow = load_allowlist()
    used = set()

    def norm(p):
        return p.replace("\\", "/")

    gaps = []
    suppressed = 0
    for path, line, name, gline in raw_gaps:
        key = (norm(path), gline)
        if key in allow:
            used.add(key)
            suppressed += 1
            continue
        gaps.append((path, line, name, gline))

    # Stale allowlist entries: the code moved on but the exemption did not.
    stale = sorted(allow - used)

    if not gaps and not stale:
        print("[OK] no lock guard held across .await (production code); "
              "%d finding(s) suppressed by allowlist, all still live" % suppressed)
        return 0

    status = 1
    if gaps:
        print("[FAIL] %d lock-held-across-await violation(s):" % len(gaps))
        for path, line, name, gline in gaps:
            print("  [GAP] %s:%d  guard `%s` acquired at line %d still held at .await"
                  % (path, line, name, gline))
        print("")
        print("Fix: release the guard before the await (block scope / drop(g) /")
        print("     move the sync work into spawn_blocking). See docs/reports/")
        print("     arch-refactor-directions-2026-09-10.md direction 2.")
    if stale:
        status = 1
        print("[FAIL] %d stale allowlist entry/entries (no longer match any finding)"
              % len(stale))
        for p, n in stale:
            print("  [STALE] %s:%d" % (p, n))
        print("  Remove them from scripts/await_guard_allowlist.txt "
              "(the code moved on; the exemption must not outlive it).")
    return status
    print("")
    print("Fix: release the guard before the await (block scope / drop(g) /")
    print("     move the sync work into spawn_blocking). See docs/reports/")
    print("     arch-refactor-directions-2026-09-10.md direction 2.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
