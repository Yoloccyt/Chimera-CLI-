#!/usr/bin/env python3
# =============================================================================
# check_subscribe_order.py - "no subscribe INSIDE a spawned task" gate
# (arch quality deep-review, plan Phase 3 item E-6, red line: §4.4 anti-pattern 3)
# =============================================================================
# Purpose: mechanically block the regression class where a raw `*.subscribe*()`
#          call sits INSIDE a tokio::spawn / spawn_local closure body. broadcast
#          channels deliver only what is sent AFTER a receiver exists, so a task
#          that subscribes after being spawned can silently miss the events that
#          triggered it (lost-wakeup). The repo convention: subscribe in the
#          synchronous caller, THEN spawn the task holding the receiver.
#
# Relationship to the builder API (measured 2026-09-25):
#   The TypeState builder (event-bus/src/subscriber.rs) makes the wrong ORDER
#   un-constructible at compile time for builder users -- but builder coverage is
#   per-call-site opt-in: raw `bus.subscribe()` callers exist in volume (hundreds
#   of sites), and nothing stops NEW code from spawning FIRST. This gate is the
#   ratchet for the RAW path; it is not a replacement for the builder.
#
# WHY a lexical scope scanner (and the history of this file):
#   A first attempt (2026-09-25, same plan) used a naive brace-slice heuristic
#   and produced 3/4 false positives on hand-checked sites (impl/closure slicing
#   leaked ACROSS function boundaries); per the repo's "a gate that cannot verify
#   its own readings is worse than no gate" principle it was DELETED, not merged.
#   This version instead reuses the architecture PROVEN in-repo by
#   check_await_across_guard.py / check_sqlite_blocking.py: string+comment
#   stripping, line-by-line depth advance, and a context DEPTH STACK (spawn
#   call sites push, scope exit pops) -- the exact "call inside region X"
#   shape, decided at statement level without cross-function leakage.
#
# FALSE-POSITIVE CLASSES THIS SCANNER MUST NOT REPORT:
#   O1. `subscribe` text inside strings/comments -- strip_noise removes them.
#   O2. spawn(handler(bus)) forwarding -- the callee's body lives OUTSIDE the
#       spawn block; no subscribe line is lexically inside. (The one lexically
#       ambiguous residual -- `spawn(f(&bus))` where f's def sits in the SAME
#       file between the spawn and the region close -- cannot exist: fn defs are
#       top-level and the spawn region closes at its own `}`.)
#   O3. spawn_blocking closures -- synchronous one-shot jobs; the red line is
#       about async tasks missing the broadcast wake-up, not blocking threads.
#   O4. `#[cfg(test)]` regions (sticky to EOF, same convention as the await
#       gate; --include-tests opts them back in).
#
# Allowlist: scripts/subscribe_order_allowlist.txt, one `<path>:<line> # reason`
#   per finding. --emit takes the current baseline; only-decrease; a STALE entry
#   fails the gate (an allowance must not outlive its debt).
#
# Exit dialect (gate_rc): 0 clean / 1 judged red / 2 undecidable.
# Encoding: all-ASCII (project script convention).
# Usage:
#   python scripts/check_subscribe_order.py             # the gate
#   python scripts/check_subscribe_order.py --emit      # baseline rows
#   python scripts/check_subscribe_order.py --selftest  # planted fixtures
#   python scripts/check_subscribe_order.py --include-tests
# =============================================================================
import argparse
import os
import re
import shutil
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATES = os.path.join(ROOT, "crates")
ALLOWLIST = os.path.join(ROOT, "scripts", "subscribe_order_allowlist.txt")

SUBSCRIBE = re.compile(r"\.subscribe(_critical|_filtered|_critical_events)?\s*\(")
# spawn sites whose ASYNC body must not contain a first-time subscribe.
# spawn_blocking is deliberately absent (class O3).
SPAWN_ASYNC = re.compile(
    r"\b(?:tokio::)?spawn(_local)?\s*\("
    r"|\bhandle\s*\.\s*spawn\s*\("
    r"|\bself\s*\.\s*handle\s*\.\s*spawn\s*\("
)
TEST_MARKER = re.compile(r"#\[cfg\(test\)\]")
INCLUDE_TESTS = False  # flipped by scan_tree(include_tests=...)


def strip_noise(line):
    """Remove string literals and line comments so braces/parens are real.

    Same contract as check_await_across_guard.strip_noise.
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
    """Findings: [(relpath, lineno)] where a subscribe call sits strictly
    inside an open async-spawn region (depth > the spawn call's line depth).
    """
    findings = []
    lines = text.split("\n")
    depth = 0
    spawn_depths = []  # line-start depths at which an async spawn( opened
    in_test = False

    for lineno, raw in enumerate(lines, 1):
        if TEST_MARKER.search(raw):
            in_test = True
        code = strip_noise(raw)
        depth_at_line = depth

        # O4: cfg(test) regions skipped unless --include-tests
        if in_test and not INCLUDE_TESTS:
            depth += code.count("{") - code.count("}")
            continue

        # O3: spawn_blocking must not be treated as an async spawn region.
        # A line matching both (rare) is checked for the blocking form first.
        if "spawn_blocking" in code:
            pass  # no push; the blocking closure is not the red-line shape
        elif SPAWN_ASYNC.search(code):
            spawn_depths.append(depth_at_line)

        # verdict: subscribe strictly inside an open spawn region, OR after the
        # spawn call on the SAME line (single-line `spawn(async move { subscribe })`
        # -- caught only by position, since the brace opens and closes inline).
        # A subscribe BEFORE spawn on one line stays legal (caller-side subscribe).
        if SUBSCRIBE.search(code):
            inside_region = any(depth_at_line > d for d in spawn_depths)
            same_line_after = False
            sm = SPAWN_ASYNC.search(code)
            if sm and "spawn_blocking" not in code:
                sub_m = SUBSCRIBE.search(code[sm.start():])
                same_line_after = sub_m is not None
            if inside_region or same_line_after:
                findings.append((relpath, lineno))

        # advance depth AFTER attribution (same as await gate)
        depth += code.count("{") - code.count("}")
        # release regions whose scope has been left
        spawn_depths = [d for d in spawn_depths if depth > d]

    return findings


def rel(path):
    return os.path.relpath(path, ROOT).replace("\\", "/")


def scan_tree(include_tests=False):
    global INCLUDE_TESTS
    INCLUDE_TESTS = include_tests
    findings = []
    scanned = 0
    for dirpath, dirs, files in os.walk(CRATES):
        dirs[:] = [d for d in dirs if d not in ("target", "__pycache__")]
        for f in sorted(files):
            if not f.endswith(".rs"):
                continue
            full = os.path.join(dirpath, f)
            with open(full, encoding="utf-8", errors="replace") as fh:
                text = fh.read()
            findings.extend(scan_text(text, rel(full)))
            scanned += 1
    return findings, scanned


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
    used = set()
    gap = []
    for key in findings:
        if key in allow:
            used.add(key)
        else:
            gap.append(key)
    return gap, sorted(allow - used)


# ------------------------------------------------------------------ selftest
def _fixture():
    base = tempfile.mkdtemp(prefix="suborder-gate-")
    src = os.path.join(base, "src")
    os.makedirs(src)
    body = '''
use event_bus::EventBus;

pub fn good_order(bus: EventBus) {                 // subscribe BEFORE spawn
    let mut rx = bus.subscribe();                  // clean
    tokio::spawn(async move { while rx.recv().await.is_ok() {} });
}

pub fn bad_in_spawn(bus: EventBus) {
    tokio::spawn(async move {
        let mut rx = bus.subscribe();              // MARK_INNER_GAP
        while rx.recv().await.is_ok() {}
    });
}

pub fn bad_one_liner(bus: EventBus) {
    tokio::spawn(async move { let _rx = bus.subscribe(); });  // MARK_ONELINE_GAP
}

pub fn ok_same_line_before(bus: EventBus) {
    let _rx = bus.subscribe(); tokio::spawn(async move {});   // MARK_SAME_LINE_OK
}

pub fn forwarding(bus: EventBus) {
    tokio::spawn(worker(bus));                     // body lives elsewhere: clean
}

async fn worker(bus: EventBus) {
    let mut rx = bus.subscribe();                  // MARK_OUTSIDE_OK
}

pub fn blocking_ok(bus: EventBus) {
    tokio::task::spawn_blocking(move || {
        let rx = bus.subscribe();                  // O3: not an async spawn
        drop(rx);
    });
}

pub fn noise(bus: EventBus) {
    tokio::spawn(async move {
        let _s = "bus.subscribe()";                // O1 string
        // let _r = bus.subscribe();               // O1 comment
    });
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn in_test_spawn_sub() {                 // O4
        let bus = event_bus::EventBus::new();
        tokio::spawn(async move {
            let _rx = bus.subscribe();             // MARK_TEST_GAP
        });
    }
}
'''
    with open(os.path.join(src, "lib.rs"), "w", encoding="utf-8") as fh:
        fh.write(body)
    return base, body


def selftest():
    global ROOT, CRATES, ALLOWLIST
    orig_root, orig_crates, orig_allow = ROOT, CRATES, ALLOWLIST
    base = None
    try:
        base, fixture_body = _fixture()
        lines = fixture_body.split("\n")

        def ln(marker):
            hits = [i + 1 for i, l in enumerate(lines) if marker in l]
            assert len(hits) == 1, "marker %r hits=%d" % (marker, len(hits))
            return hits[0]

        L_INNER = ln("MARK_INNER_GAP")
        L_ONELINE = ln("MARK_ONELINE_GAP")
        L_SAMEROW = ln("MARK_SAME_LINE_OK")
        L_OUTSIDE = ln("MARK_OUTSIDE_OK")
        L_TEST = ln("MARK_TEST_GAP")
        ROOT = base
        CRATES = base  # scan walks CRATES; fixture lives at base/src
        scripts = os.path.join(base, "scripts")
        os.makedirs(scripts, exist_ok=True)
        ALLOWLIST = os.path.join(scripts, "subscribe_order_allowlist.txt")

        cases = []
        findings, scanned = scan_tree()
        paths = {p for p, _ in findings}
        cases.append(("fixture scanned", scanned == 1))
        cases.append(("subscribe inside tokio::spawn -> GAP",
                      ("src/lib.rs", L_INNER) in findings))
        cases.append(("single-line spawn{subscribe} -> GAP (teeth probe)",
                      ("src/lib.rs", L_ONELINE) in findings))
        cases.append(("subscribe before spawn on same line -> clean",
                      ("src/lib.rs", L_SAMEROW) not in findings))
        cases.append(("subscribe before spawn -> clean",
                      not any(n == 4 for (_p, n) in findings)))
        cases.append(("forwarded callee body -> clean (O2)",
                      ("src/lib.rs", L_OUTSIDE) not in findings))
        cases.append(("string/comment inside spawn -> clean (O1)",
                      sum(1 for (_p, n) in findings if n in (34, 35)) == 0))
        cases.append(("spawn_blocking closure -> clean (O3)",
                      not any(n == 26 for (_p, n) in findings)))
        cases.append(("cfg(test) default-skipped (O4)",
                      ("src/lib.rs", L_TEST) not in findings))
        f_inc, _ = scan_tree(include_tests=True)
        cases.append(("--include-tests surfaces the test-region gap",
                      ("src/lib.rs", L_TEST) in f_inc))
        # allowlist mechanics + teeth
        with open(ALLOWLIST, "w", encoding="utf-8") as fh:
            for p, n in sorted(findings):
                fh.write("%s:%d # planted\n" % (p, n))
        gap, stale = decide(findings, load_allowlist())
        cases.append(("exact allowlist suppresses all", not gap and not stale))
        gap2, stale2 = decide(findings, {("src/lib.rs", 999)})
        cases.append(("unmatched entry: GAP stands + STALE red",
                      len(gap2) == len(findings) and len(stale2) == 1))
        os.remove(ALLOWLIST)
        try:
            load_allowlist()
            cases.append(("missing allowlist is loud", False))
        except ValueError:
            cases.append(("missing allowlist is loud", True))
        # two findings expected on the default path (inner + one-liner)
        cases.append(("exactly two production findings", len(findings) == 2))

        rc = 0
        for name, ok in cases:
            print("  [%s] %s" % ("PASS" if ok else "FAIL", name))
            if not ok:
                rc = 1
        print("[SELFTEST] %d/%d fixtures passed" % (len(cases) - rc, len(cases)))
        return rc
    finally:
        ROOT, CRATES, ALLOWLIST = orig_root, orig_crates, orig_allow
        if base:
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

    findings, scanned = scan_tree(include_tests=args.include_tests)
    if args.emit:
        print("# subscribe_order_allowlist.txt -- only-decrease baseline for")
        print("#   check_subscribe_order.py (red line: subscribe-before-spawn).")
        print("# Row = `<path>:<line>  # reason`. Take via:")
        print("#   python scripts/check_subscribe_order.py --emit")
        print("# New violations must move the subscribe OUT of the spawned task")
        print("# (subscribe in the sync caller, then spawn), not be registered.")
        for path, lineno in sorted(findings):
            print("%s:%d  # baseline 2026-09-25" % (path, lineno))
        return 0

    try:
        allow = load_allowlist()
    except ValueError as e:
        print("[UNDECIDABLE] %s" % e)
        return 2

    gap, stale = decide(findings, allow)
    print("[INFO] files scanned: %d; findings: %d; allowlist: %d"
          % (scanned, len(findings), len(allow)))
    if not gap and not stale:
        print("[OK] no raw subscribe inside an async spawn beyond the baseline")
        return 0
    if gap:
        print("[FAIL] %d subscribe-inside-spawn site(s) (red line: subscribe "
              "BEFORE spawn; broadcast never replays history):" % len(gap))
        for path, lineno in sorted(gap):
            print("  [GAP] %s:%d" % (path, lineno))
    if stale:
        print("[FAIL] %d stale allowlist row(s) -- delete them:" % len(stale))
        for path, lineno in stale:
            print("  [STALE] %s:%d" % (path, lineno))
    return 1


if __name__ == "__main__":
    import gate_rc  # crash exits 2, never borrows 1 (F32/F33 discipline)
    sys.exit(gate_rc.run(main))
