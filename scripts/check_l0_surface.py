#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L0 surface gate single entry (L0-D, ADR-310).

Why this dispatcher exists
--------------------------
The ten check_l0_* gates each carry their own ADR-registered adjudication
(G-99/G-105/G-107/G-115/G-119/G-125/G-126/G-152...), exit-code discipline
(0 = verdict green / 1 = verdict red / 2 = undecidable), and --selftest teeth.
What was missing was a single discoverable entry point: every L0-boundary
question had to know which of the ten scripts owns it, and ci.yml carried ~20
near-identical invocation lines that drift independently.

This module is a THIN dispatcher, not a merger:
  * it owns NO adjudication logic -- each subcommand delegates to the existing
    script unchanged, in the same process tree, with arguments passed through;
  * verdict passthrough is exact (rc 0/1/2 from the gate is returned as-is) --
    rewriting `rc != 0 -> 1` is the H7 anti-pattern this repo's tooling-health
    gate exists to kill, so the gate's own code is always the final word;
  * `all` runs the ten gates in a fixed order, continues after failures
    (no-fail-fast, the same discipline as `cargo test --no-fail-fast`), and
    returns 2 if any gate was undecidable, else 1 if any was red, else 0.

Undecidable-dominates-red in `all` mode: a missing script or interpreter error
means the surface was not fully judged, which must never masquerade as a
verdict either way.

Freeze files stay per-script (scripts/l0_*_freeze*.txt): merging them into one
toml would force every gate to change its BASELINE path in the same commit and
would blind the --selftest fixtures that plant defects against those paths.
That merge is deferred with this rationale recorded (ADR-310).

Usage
-----
  check_l0_surface.py <sub> [extra args...]   run one gate, args passed through
  check_l0_surface.py all                     run all ten (fixed order, no-fail-fast)
  check_l0_surface.py --help                  list subcommands

Exit codes: 0 = green, 1 = verdict red (or any red in `all`), 2 = usage error /
undecidable (missing script, --help is 0). Output is pure ASCII (project
script convention; avoids Windows GBK console pitfalls).
"""

import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))

# subcommand -> (script, one-line what-it-judges). Order of ALL_ORDER is the
# `all` execution order: purity first (the widest scanner), ratchets last.
GATES = {
    "purity": ("check_l0_purity.py", "G-99 dependency allowlist + impure-symbol scan"),
    "logic-ledger": ("check_l0_logic_ledger.py", "G-119 function-granularity controlled-logic ledger"),
    "sublayer": ("check_l0_sublayer.py", "G-125/G-126 L0-A/L0-B direction (PURITY-LEAK/UPWARD)"),
    "assembly": ("check_l0a_sibling_assembly.py", "L0-A sibling assembly parity"),
    "dep-surface": ("check_l0b_dep_surface.py", "L0-B dependency surface"),
    "root-surface": ("check_l0_root_surface.py", "G-107 umbrella root re-export ratchet"),
    "sibling-surface": ("check_l0_sibling_surface.py", "G-152 sibling root re-export parity"),
    "const-ratchet": ("check_l0_const_ratchet.py", "L0 const/static surface ratchet"),
    "pubfn-ratchet": ("check_l0_pubfn_ratchet.py", "G-105 pub fn surface ratchet"),
    "extensibility": ("check_l0_extensibility_class.py", "G-115 per-type extensibility class ledger"),
}

ALL_ORDER = [
    "purity",
    "logic-ledger",
    "sublayer",
    "assembly",
    "dep-surface",
    "root-surface",
    "sibling-surface",
    "const-ratchet",
    "pubfn-ratchet",
    "extensibility",
]


def usage(out):
    out.write("L0 surface gate single entry (delegates to check_l0_* scripts)\n")
    out.write("usage: check_l0_surface.py <sub> [extra args...] | all | --help\n")
    for name in ALL_ORDER:
        script, what = GATES[name]
        out.write("  %-16s %s (%s)\n" % (name, what, script))
    out.write("  all              run all ten, fixed order, no-fail-fast\n")


def run_gate(script, extra):
    """Delegate to one gate script; its rc IS the verdict (exact passthrough)."""
    path = os.path.join(HERE, script)
    if not os.path.isfile(path):
        sys.stderr.write("[ENV] gate script missing: %s\n" % path)
        return 2
    try:
        proc = subprocess.run([sys.executable, path] + list(extra))
    except OSError as exc:
        sys.stderr.write("[ENV] cannot launch %s: %s\n" % (script, exc))
        return 2
    return proc.returncode


def run_all():
    undecidable = 0
    red = 0
    for name in ALL_ORDER:
        script, _ = GATES[name]
        rc = run_gate(script, [])
        if rc == 2:
            undecidable += 1
        elif rc != 0:
            red += 1
        sys.stdout.write("[%-16s] %s\n" % (name, "OK" if rc == 0 else "rc=%d" % rc))
    sys.stdout.write(
        "[SUMMARY] gates=%d ok=%d red=%d undecidable=%d\n"
        % (len(ALL_ORDER), len(ALL_ORDER) - red - undecidable, red, undecidable)
    )
    # undecidable dominates red: an unjudged surface is never a verdict
    if undecidable:
        return 2
    if red:
        return 1
    return 0


def main(argv):
    if not argv:
        usage(sys.stderr)
        return 2
    sub = argv[0]
    if sub in ("-h", "--help"):
        usage(sys.stdout)
        return 0
    if sub == "all":
        if len(argv) > 1:
            sys.stderr.write("usage: `all` takes no extra args\n")
            return 2
        return run_all()
    entry = GATES.get(sub)
    if entry is None:
        sys.stderr.write("[USAGE] unknown subcommand: %s\n" % sub)
        usage(sys.stderr)
        return 2
    return run_gate(entry[0], argv[1:])


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
