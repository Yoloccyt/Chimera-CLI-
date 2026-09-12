#!/usr/bin/env python3
# =============================================================================
# check_composition_root.py - composition-root assembly gate (arch dir B-c)
# =============================================================================
# Purpose: mechanically block NEW bypasses of the canonical composition root.
#          chimera-cli/src/composition.rs is the ONLY sanctioned place to
#          assemble infrastructure (EventBus / QuestEngine / McaGateway /
#          AppServer). History (measured 2026-09-12, see
#          docs/reports/arch-refactor-directions-v2-2026-09-12.md section 3):
#          9 production EventBus::new() sites - 8 command paths
#          (chat/run/exec/quest/parliament/agent/tui/doctor) each build their
#          own ephemeral bus+engine, so the Critical-delivery guarantee held on
#          only 1 of 9 paths (fixed at the bus level by B-a fallback sink, but
#          the STRUCTURAL debt - N invisible buses per process - remains).
#
# Scope: crates/chimera-cli/src only. Other L10 crates (chimera-tui etc.) are
#        separate hosts with their own wiring story; widening scope is a
#        follow-up decision, not smuggled in here.
#
# Rules:
#   R1. composition.rs may call the assembly constructors freely (canonical).
#   R2. Any OTHER production-segment call site is a GAP unless registered in
#       scripts/composition_root_baseline.txt as `path count` with
#       count EXACTLY equal to the measured violations in that file.
#       - count > measured  -> stale exemption (debt already moved) -> FAIL
#       - count < measured  -> new bypass appeared -> FAIL
#       - file deleted      -> stale entry -> FAIL
#       (Ratchet: only-decrease; an exemption must not outlive the debt,
#        same discipline as await_guard_allowlist.txt staleness check.)
#   R3. Lines inside #[cfg(test)] segments and comments never count.
#
# Encoding: all-ASCII (project script convention, avoids CJK locale issues)
# Exit code: 0 = clean, 1 = gap found, 2 = usage error
# Usage:
#   python scripts/check_composition_root.py --selftest
#   python scripts/check_composition_root.py
# =============================================================================
import argparse
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCAN_ROOT = os.path.join(ROOT, "crates", "chimera-cli", "src")
BASELINE = os.path.join(ROOT, "scripts", "composition_root_baseline.txt")

# Assembly constructors: EventBus / QuestEngine / McaGateway / AppServer
# `new` or any `with_*` associated fn (with_capacity/with_logger/with_backend/
# with_checkpoints/with_critical_fallback ...). Plain method CALLS on values
# (`bus.publish(...)`) are NOT assembly and never match (require `Type::`).
ASSEMBLY_CALL = re.compile(
    r"\b(EventBus|QuestEngine|McaGateway|AppServer)::(?:new|with_[A-Za-z_]*)\s*\("
)
TEST_MARKER = re.compile(r"^\s*#\[cfg\(test\)\]")
CANONICAL_REL = "crates/chimera-cli/src/composition.rs"


def strip_noise(line):
    """Remove line comments and string literal contents so only real code stays.

    WHY: doc comments embed example code (`/// let bus = EventBus::new()`) and
    veto reasons embed type names in strings; both must never count.
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
    """Return [(relpath, lineno, matched_call)] for the production segment.

    Test-segment simplification (SAME simplification as
    check_await_across_guard.py, kept for gate-consistency): once a
    `#[cfg(test)]` attribute is seen, the rest of the file is treated as test
    code. In this crate every `#[cfg(test)]` heads the single tests module at
    the file tail; if that invariant ever breaks the gate re-classifies by
    failing loud on review, not silently.
    """
    hits = []
    in_test = False
    for lineno, raw in enumerate(text.split("\n"), 1):
        if TEST_MARKER.match(raw):
            in_test = True
            continue
        if in_test:
            continue
        code = strip_noise(raw)
        m = ASSEMBLY_CALL.search(code)
        if m:
            hits.append((relpath, lineno, m.group(0)))
    return hits


def scan_tree():
    hits = []
    for dirpath, _, files in os.walk(SCAN_ROOT):
        for f in sorted(files):
            if not f.endswith(".rs"):
                continue
            p = os.path.join(dirpath, f)
            rel = os.path.relpath(p, ROOT).replace("\\", "/")
            with open(p, encoding="utf-8", errors="ignore") as fh:
                hits.extend(scan_text(fh.read(), rel))
    return hits


def load_baseline():
    """Return {posix_relpath: max_count}. Malformed lines fail loud (exit 2)."""
    caps = {}
    if not os.path.exists(BASELINE):
        return caps
    with open(BASELINE, encoding="utf-8", errors="ignore") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            parts = line.split()
            if len(parts) != 2 or not parts[1].isdigit():
                print("[CONFIG] malformed baseline line: %r" % raw.rstrip())
                sys.exit(2)
            caps[parts[0]] = int(parts[1])
    return caps


def adjudicate(hits, caps):
    """Split hits into (gaps, stale, allowed).

    gaps   - production call sites beyond the canonical file whose per-file
             count exceeds (or has no) baseline entry.
    stale  - baseline entries that no longer match reality: file gone, or
             registered cap != measured count (ratchet must be exact).
    allowed- call sites covered by an exactly-matching baseline entry.
    """
    per_file = {}
    canonical_hits = 0
    for rel, lineno, call in hits:
        if rel == CANONICAL_REL:
            canonical_hits += 1
            continue
        per_file.setdefault(rel, []).append((lineno, call))

    gaps, allowed = [], []
    for rel in sorted(set(per_file) | set(caps)):
        measured = len(per_file.get(rel, []))
        if rel not in caps:
            if measured:
                gaps.extend((rel, ln, call, "no baseline entry") for ln, call in per_file[rel])
            continue
        cap = caps[rel]
        if not os.path.exists(os.path.join(ROOT, rel)):
            stale_entry = (rel, cap, "file no longer exists")
        elif cap < measured:
            stale_entry = (rel, cap, "measured %d > cap %d (new bypass)" % (measured, cap))
        elif cap > measured:
            stale_entry = (rel, cap, "measured %d < cap %d (debt already moved)" % (measured, cap))
        else:
            stale_entry = None
        if stale_entry:
            gaps.append((rel, 0, "BASELINE", stale_entry[2]))
        else:
            allowed.extend((rel, ln, call) for ln, call in per_file[rel])
    return gaps, allowed, canonical_hits


# ---------------------------------------------------------------- self-test
SELFTEST_SRC = """
use event_bus::EventBus;

/// doc example must not count: let bus = EventBus::new();
fn prod_ok() {
    let bus = EventBus::new(); // canonical-style call OUTSIDE composition -> GAP
    let _ = bus;
}

fn vetoes() {
    // comment mention: EventBus::new() must not count
    let reason = "EventBus::new() in a string must not count";
    let _ = reason;
}

#[cfg(test)]
mod tests {
    fn t() {
        let bus = EventBus::new(); // test segment -> suppressed
        let _ = bus;
    }
}
"""

BASELINE_SELFTEST_CASES = [
    # (caps, files_present, measured_per_file, expect_gaps, expect_allowed, why)
    ({"a.rs": 1}, {"a.rs"}, {"a.rs": 1}, 0, 1, "cap==measured -> allowed"),
    ({"a.rs": 2}, {"a.rs"}, {"a.rs": 1}, 1, 0, "cap>measured -> stale (debt moved)"),
    ({"a.rs": 0}, {"a.rs"}, {"a.rs": 1}, 1, 0, "cap<measured -> gap (new bypass)"),
    ({"a.rs": 1}, {}, {"a.rs": 0}, 1, 0, "file gone -> stale entry"),
    ({}, {"a.rs"}, {"a.rs": 1}, 1, 0, "no entry -> gap"),
]


def selftest():
    ok = True
    hits = scan_text(SELFTEST_SRC, "crates/chimera-cli/src/selftest.rs")
    if len(hits) != 1 or hits[0][1] != 6:
        print("[SELFTEST] scan classification wrong, expected exactly 1 hit at "
              "line 6, got: %s" % hits)
        ok = False
    # baseline adjudication table
    for i, (caps, present, measured, e_gaps, e_allow, why) in enumerate(
            BASELINE_SELFTEST_CASES, 1):
        hits2 = []
        for rel, n in measured.items():
            hits2.extend((rel, 10 + j, "EventBus::new()") for j in range(n))
        # emulate os.path.exists check
        real_exists = os.path.exists
        os.path.exists = lambda p, _present=present: relpath_in(p, _present)
        try:
            gaps, allowed, _ = adjudicate(hits2, dict(caps))
        finally:
            os.path.exists = real_exists
        if len(gaps) != e_gaps or len(allowed) != e_allow:
            print("[SELFTEST] baseline case %d FAILED (%s): gaps=%s allowed=%s"
                  % (i, why, gaps, allowed))
            ok = False
    if not ok:
        return 1
    print("[SELFTEST] all constructed cases classified correctly:")
    print("           scan: 1 production GAP / doc+comment+test-segment suppressed")
    print("           baseline: 5 adjudication cases (exact ratchet, stale, new, gone, absent)")
    return 0


def relpath_in(p, present):
    """os.path.exists shim for selftest: only selftest-declared files exist."""
    norm = str(p).replace("\\", "/")
    for rel in present:
        if norm.endswith(rel):
            return True
    return False  # everything else (incl. real files) judged by fixture only


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()

    hits = scan_tree()
    caps = load_baseline()
    gaps, allowed, canonical = adjudicate(hits, caps)

    print("[INFO] canonical assembly points in composition.rs: %d" % canonical)
    print("[INFO] baseline-covered bypasses: %d (ratchet: exact-count, only-decrease)"
          % len(allowed))

    if not gaps:
        print("[OK] composition-root gate clean: no unregistered infrastructure "
              "assembly outside composition.rs")
        return 0

    print("[FAIL] %d composition-root violation(s):" % len(gaps))
    for rel, lineno, call, why in gaps:
        if lineno:
            print("  [GAP] %s:%d  %s" % (rel, lineno, call))
        else:
            print("  [BASELINE] %s  %s" % (rel, why))
    print("")
    print("Fix: assemble in crates/chimera-cli/src/composition.rs (canonical)")
    print("     and consume via AppContext, or migrate the call there (B-b).")
    print("     Known-debt registration: scripts/composition_root_baseline.txt")
    print("     `<posix-relpath> <exact-count>`; cap must EQUAL measured count")
    print("     (only-decrease ratchet; exemptions must not outlive the debt).")
    print("     Ref: docs/reports/arch-refactor-directions-v2-2026-09-12.md B-c.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
