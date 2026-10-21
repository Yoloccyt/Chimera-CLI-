# -*- coding: ascii -*-
"""L0-A crate roster -- the single source of truth for "which crates are pure
contract crates" after ADR-222 D2 (the 1+6 split).

WHY a shared module instead of a tuple per gate: five gates judge the L0-A
surface (G-99 purity, G-105 pub fn, G-115 extensibility class, G-119 logic
ledger, G-91/G-123 contract consumers). With a tuple in each file, every split
batch would need five synchronized edits, and forgetting one does NOT fail --
it silently shrinks that gate's jurisdiction (the def scan drops the moved
modules, their ledger rows turn into bogus [STALE], and the gate still prints
"clean"). Measured live on 2026-10-20: after `core_domain` moved out, a
single-crate def scan reported defs 236 -> 213.

This module is DATA, not a judge. The judge is G-125 (`check_l0_sublayer.py`),
which cross-checks this roster against the `.sh`/`.ps1` sub-layer tables and
exits 2 on divergence -- so an out-of-sync roster is undecidable, never green.

Order matters only in that the first entry is the umbrella/facade crate.
"""

# Pure contract crates (machine layer 0, sub-layer 0A).
L0A_DIR_NAMES = (
    "nexus-contracts",   # umbrella + facade (spec, vector_math, protocol, testutil)
    "nexus-contracts-core",   # core_domain
    "nexus-contracts-sparse",   # sparse
    "nexus-contracts-safety",
    "nexus-contracts-seams",
    "nexus-contracts-mca",
    "nexus-contracts-memory",
)

# Crate names of the same set, in Rust identifier form (lib name = dashes ->
# underscores). Used by judges that match paths/identifiers rather than dirs.
def rust_ident(crate_name):
    return crate_name.replace("-", "_")
