#!/usr/bin/env python3
# -*- coding: ascii -*-
"""ImmutableSurface <-> event-bus Critical mpsc parity gate (G-117, ADR-216).

Why this gate exists
--------------------
`nexus-contracts::spec::harness_spec::ImmutableSurface` enumerates the event
variants whose channel/severity semantics may never evolve. ADR-212 section 7.1
raised its Critical coverage 6 -> 17: the 16 mpsc-bypass variants pinned by
`event-bus/src/bus.rs::CRITICAL_MPSC_VARIANTS`, plus broadcast-only
`CheckpointSaved`. The bus side already carries FIVE locks (D-8 dual list,
count const, lane list, perf-redline prose lock...), but ALL of them live INSIDE
event-bus. Measured 2026-10-15: adding a new mpsc-Critical variant turns those
five red while L0's ImmutableSurface stays silent -- the spec can silently lose
the delivery guarantee for an event that advertises it. That is exactly the
"assurance promised in prose, judged by nothing" shape this repo builds gates
against.

Contract (both directions are red, rc=1):
  MISSING  an mpsc-bypass variant absent from the ImmutableSurface Critical set
  EXTRA    an ImmutableSurface Critical variant that is neither in the mpsc set
           nor in the registered broadcast-only allowlist -- any such addition
           must be deliberate, so it costs an allowlist row (with a reason)

Parsing locks (same-source on purpose)
--------------------------------------
  * bus side: blanked source of `fn is_critical_mpsc_event`, regex
    `NexusEvent::(\\w+)`; cross-checked against the `CRITICAL_MPSC_VARIANTS`
    const in the SAME file (mismatch = bus-internal inconsistency -> rc 1).
  * L0 side: blanked `pub enum ImmutableSurface` block, variant names matching
    `^(Critical)(\\w+)` per line; `RedlineCriticalEventMpsc` is excluded by
    construction (it does not start with `Critical`).
  * comment/string blanking is IMPORTED from check_l0_purity (the L0 gate
    family lexer), never re-implemented -- a second lexer would drift.

Undecidable states exit 2 (never a silent pass): unreadable input files, parse
below the floors (a blind parser reports "parity" exactly like a compliant
tree), malformed allowlist rows.

Modes / exit codes
   (default)   judge the real repo       0 = parity, 1 = MISSING/EXTRA/count-mismatch
   --selftest  planted fixtures in both directions + comment decoys
                                         0 = all assertions held, 1 = not
   usage / undecidable                    2
Output is pure ASCII (project script convention).
"""

import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

sys.path.insert(0, HERE)
import check_l0_purity as purity  # noqa: E402  (same blanking lexer as the gate family)

BUS_FILE = os.path.join(ROOT, "crates", "event-bus", "src", "bus.rs")
SURFACE_FILE = os.path.join(ROOT, "crates", "nexus-contracts", "src", "spec",
                            "harness_spec.rs")

# Refuse to judge below these: a blind parse must not be able to report "parity"
# (measured surface 2026-10-15: mpsc=16, ImmutableSurface Critical=17).
MPSC_FLOOR = 10
SURFACE_FLOOR = 10

# Broadcast-only Critical variants deliberately covered by ImmutableSurface.
# Row = (name, reason>=24 chars). ANY addition here is a deliberate act: the row
# must explain why an event that does NOT ride the mpsc bypass is still frozen.
BROADCAST_ONLY = [
    ("CheckpointSaved",
     "broadcast-only Critical frozen by ADR-212 section 7.1 (17 = 16 mpsc + 1); "
     "semantics may never evolve even though delivery is broadcast"),
]
MIN_REASON_LEN = 24

MPSC_COUNT_RE = re.compile(r"CRITICAL_MPSC_VARIANTS\s*:\s*usize\s*=\s*(\d+)")
SUFFIX_RE = re.compile(r"NexusEvent::(\w+)")
# re.MULTILINE is load-bearing: without it `^` only matches at string start and
# the real tree parses to ZERO surface variants (caught by the selftest, which
# is exactly why the fixture runs through these same functions).
CRITICAL_VARIANT_RE = re.compile(r"^[ \t]*Critical(\w+)[ \t]*,", re.MULTILINE)


def blanked(path):
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        # Normalize CRLF: the worktree is CRLF on Windows, and a line-anchored
        # `find("\n}\n")` misses every block end inside CRLF text.
        return purity.blank_rust(fh.read().replace("\r\n", "\n"))


def extract_mpsc(bus_src):
    """(set_of_names, declared_count_or_None) from `is_critical_mpsc_event`."""
    idx = bus_src.find("fn is_critical_mpsc_event")
    if idx == -1:
        return None, None
    tail = bus_src[idx:]
    end = tail.find("\n}\n")
    body = tail if end == -1 else tail[:end]
    names = set(SUFFIX_RE.findall(body))
    m = MPSC_COUNT_RE.search(bus_src)
    declared = int(m.group(1)) if m else None
    return names, declared


def extract_surface(surface_src):
    """set_of_event_names from the `pub enum ImmutableSurface` block."""
    idx = surface_src.find("pub enum ImmutableSurface")
    if idx == -1:
        return None
    tail = surface_src[idx:]
    end = tail.find("\n}\n")
    body = tail if end == -1 else tail[:end]
    return set(CRITICAL_VARIANT_RE.findall(body))


def validate_allowlist(rows):
    errs = []
    for name, reason in rows:
        if len(reason) < MIN_REASON_LEN:
            errs.append("allowlist row '%s': reason is %d chars, below the "
                        "%d-char evidence floor" % (name, len(reason), MIN_REASON_LEN))
    return errs


def compare(mpsc, surface, allowlist_names):
    """Pure verdict -> (ok, out_lines). Same function the selftest plants into."""
    out = []
    missing = sorted(mpsc - surface)
    extra = sorted(n for n in (surface - mpsc) if n not in allowlist_names)
    for n in missing:
        out.append("[MISSING] mpsc-bypass variant '%s' is not covered by "
                   "ImmutableSurface::Critical* -- a new Critical channel "
                   "guarantee silently left the never-evolve surface" % n)
    for n in extra:
        out.append("[EXTRA] ImmutableSurface covers '%s' but it is neither an "
                   "mpsc-bypass variant nor a registered broadcast-only row -- "
                   "add the row with a reason (same commit) or drop the variant" % n)
    return (not missing and not extra), out


def run_real():
    for path in (BUS_FILE, SURFACE_FILE):
        if not os.path.isfile(path):
            print("[TOOL-ERROR] input not found: %s" % path)
            return 2
    errs = validate_allowlist(BROADCAST_ONLY)
    for e in errs:
        print("[TOOL-ERROR][ledger] " + e)
    if errs:
        return 2

    mpsc, declared = extract_mpsc(blanked(BUS_FILE))
    surface = extract_surface(blanked(SURFACE_FILE))
    if mpsc is None or surface is None:
        print("[TOOL-ERROR] parse anchors not found (fn is_critical_mpsc_event / "
              "pub enum ImmutableSurface)")
        return 2
    if len(mpsc) < MPSC_FLOOR or len(surface) < SURFACE_FLOOR:
        print("[TOOL-ERROR] parse suspiciously small (mpsc=%d surface=%d, floors "
              "%d/%d): refusing to judge a blind parse"
              % (len(mpsc), len(surface), MPSC_FLOOR, SURFACE_FLOOR))
        return 2

    allow = {n for n, _r in BROADCAST_ONLY}
    ok, out = compare(mpsc, surface, allow)
    if declared is not None and declared != len(mpsc):
        out.append("[DRIFT] CRITICAL_MPSC_VARIANTS declares %d but the matches! "
                   "body carries %d variants -- bus-internal inconsistency"
                   % (declared, len(mpsc)))
        ok = False
    for line in out:
        print(line)
    if ok:
        print("[OK] ImmutableSurface <-> mpsc parity: mpsc=%d, Critical=%d "
              "(= mpsc + %s)" % (len(mpsc), len(surface),
                                 ",".join(sorted(allow)) or "-"))
        return 0
    print("[FAIL] ImmutableSurface <-> mpsc parity broken (%d finding(s))"
          % len(out))
    return 1


# --- selftest: planted fixtures in BOTH directions, on the same functions ----

BUS_FIXTURE = """\
fn is_critical_mpsc_event(event: &NexusEvent) -> bool {
    matches!(
        event,
        NexusEvent::Alpha { .. }
            // comment decoy: NexusEvent::GhostVariant { .. } must not count
            | NexusEvent::Beta { .. }
            | NexusEvent::Gamma { .. }
    )
}
const CRITICAL_MPSC_VARIANTS: usize = 3;
"""

SURFACE_FIXTURE = """\
#[non_exhaustive]
pub enum ImmutableSurface {
    RedlineCriticalEventMpsc,
    CriticalAlpha,
    CriticalBeta,
    CriticalGamma,
    CriticalBroadcastOne,
}
"""


def selftest():
    ok = True

    def add(label, cond):
        nonlocal ok
        if cond:
            print("[SELFTEST] %s" % label)
        else:
            ok = False
            print("[SELFTEST-FAIL] %s" % label)

    mpsc, declared = extract_mpsc(purity.blank_rust(BUS_FIXTURE.replace("\r\n", "\n")))
    surface = extract_surface(purity.blank_rust(SURFACE_FIXTURE.replace("\r\n", "\n")))
    add("bus parse: 3 arms found, count const 3",
        mpsc == {"Alpha", "Beta", "Gamma"} and declared == 3)
    add("comment decoy inside matches! body not counted", "GhostVariant" not in mpsc)
    add("surface parse: only Critical* variants, Redline excluded",
        surface == {"Alpha", "Beta", "Gamma", "BroadcastOne"})

    good, out = compare(mpsc, surface, {"BroadcastOne"})
    add("parity holds when surface = mpsc + allowlisted broadcast-only", good)

    # Direction 1: MISSING -- a new mpsc variant with no L0 coverage.
    ok2, out2 = compare(mpsc | {"Delta"}, surface, {"BroadcastOne"})
    add("MISSING fires when an mpsc variant left the surface",
        not ok2 and any("[MISSING]" in l and "'Delta'" in l for l in out2))

    # Direction 2: EXTRA -- surface covers an unknown variant without a row.
    ok3, out3 = compare(mpsc, surface | {"UnknownAdded"}, {"BroadcastOne"})
    add("EXTRA fires for an unregistered surface variant",
        not ok3 and any("[EXTRA]" in l and "'UnknownAdded'" in l for l in out3))

    # Allowlist semantics: registered name is not EXTRA; missing row still MISSING.
    add("allowlisted broadcast-only is not EXTRA",
        compare(mpsc, surface, {"BroadcastOne"})[0])
    # Ledger guards must fire (an always-false guard is noise).
    add("short allowlist reason rejected",
        bool(validate_allowlist([("X", "test")])))
    add("real allowlist passes its own guards", validate_allowlist(BROADCAST_ONLY) == [])
    # Floors must refuse a blind parse.
    add("floors refuse tiny parses",
        len(mpsc) < MPSC_FLOOR and len(surface) < SURFACE_FLOOR)

    # String decoy: a variant name inside a string literal must not count.
    STR_BUS = BUS_FIXTURE.replace(
        "NexusEvent::Gamma { .. }",
        'NexusEvent::Gamma { .. }\n    // | NexusEvent::FromString { .. }')
    m2, _d = extract_mpsc(purity.blank_rust(STR_BUS))
    add("second comment decoy not counted", "FromString" not in m2)

    print("[SELFTEST] %s" % ("ALL ASSERTIONS HELD" if ok else "FAILED"))
    return 0 if ok else 1


def main(argv):
    for a in argv[1:]:
        if a not in ("--selftest",):
            sys.stderr.write("usage: check_immutable_surface_parity.py [--selftest]\n")
            return 2
    if "--selftest" in argv[1:]:
        return selftest()
    return run_real()


if __name__ == "__main__":
    import gate_rc  # crash -> exit 2, never a borrowed traceback exit 1

    sys.exit(gate_rc.run(lambda: main(sys.argv)))
