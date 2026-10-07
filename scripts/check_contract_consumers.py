#!/usr/bin/env python3
# =============================================================================
# check_contract_consumers.py - L0 contract ">=2 workspace consumers" gate (G-91)
#                               (v3.0 5.2 WI-A1 / ADR-199 7 Q2 / v3.3 2.1)
# =============================================================================
# Purpose: mechanically guard the rule "a nexus-contracts (L0) type must be
#   born with >= 2 consuming workspace crates". Ghost contracts that land
#   before wiring (precedent: recall_fusion contract, price_vector first run)
#   would otherwise silently advertise capability. Deficiencies are made
#   visible in an only-shrink baseline register instead of staying invisible.
#
# Dialect (what counts as a consumer of NAME):
#   direct  -- a workspace crate whose PRODUCTION scope (crates/<c>/**/src/*.rs,
#              `#[cfg(test)] mod` tails excluded -- the SAME rule as
#              check_dead_contract_apis.py, kept in lock-step on purpose)
#              references NAME through a nexus_contracts connection:
#                * path chains : nexus_contracts::a::b::NAME
#                * use stmts   : `use nexus_contracts...;` incl. brace lists
#                  (flattened across lines) and `pub use` re-export statements
#   hop-1   -- a hub crate X whose own `pub use nexus_contracts...;` statement
#              re-exports NAME, AND a crate D references both X's identifier
#              and NAME inside one statement (`use nexus_core::{Checkpoint};`,
#              `nexus_core::domain::Quest`). Measured 2026-10-07: 10 such hubs,
#              all explicit re-export lists; NO glob re-exports of
#              nexus_contracts exist in the tree (grep-verified).
#   Dedup is per CRATE: >= 2 distinct consumer crates are required.
#
# Known boundaries (disclosed, deliberate):
#   * Re-export chains deeper than one hop (contracts -> nexus-core ->
#     quest-engine -> ...) are NOT modelled; counts can undercount on deep
#     chains, which only makes a name look MORE gap-ish. That is the safe
#     direction for a ratchet (the register grows, never silently shrinks).
#   * Enumeration covers `pub struct/enum/trait/type` definitions only
#     (237 defs measured 2026-10-07); macro-generated defs are not modelled.
#   * test/bench/example scopes never count as consumers.
#
# Verdicts:
#   GAP    def with < 2 consumers and no baseline line -> 1
#          (register the name WITH a reason comment after human review; never
#           edit the baseline to silence a red without that review)
#   STALE  baseline line whose name now has >= 2 consumers (or is no longer a
#          def) -> 1 (delete the line in the batch that wires it)
#   malformed baseline line, or missing baseline / crates dir -> 2
#          (undecidable; never a pass)
#
# Baseline: scripts/contract_consumer_baseline.txt, one NAME per line, optional
#   `# reason`. Only-shrink. --emit prints the full baseline-format document
#   (header + current gaps); regeneration REQUIRES human diff review.
#
# Output is ASCII only (PowerShell 5.1 GBK console safety, project convention).
# Exit code: 0 = clean, 1 = gap/stale, 2 = usage/config error
# Usage:
#   python scripts/check_contract_consumers.py --selftest
#   python scripts/check_contract_consumers.py --emit
#   python scripts/check_contract_consumers.py
# =============================================================================
import argparse
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASELINE = ROOT / "scripts" / "contract_consumer_baseline.txt"
CONTRACTS_DIR_NAME = "nexus-contracts"

DEF_RE = re.compile(r"^\s*pub\s+(?:struct|enum|trait|type)\s+([A-Z][A-Za-z0-9_]*)", re.M)
CFG_TEST_RE = re.compile(r"^\s*#\[cfg\(test\)\]")
MODULE_DECL_RE = re.compile(r"^\s*(pub(\([^)]*\))?\s+)?mod\s+\w")
CHAIN_RE = re.compile(r"nexus_contracts(?:\s*::\s*[A-Za-z_]\w*)+")
USE_RE = re.compile(r"use\s+nexus_contracts[^;]*;")
EXPORT_RE = re.compile(r"pub\s+use\s+nexus_contracts[^;]*;")
TOK_RE = re.compile(r"[A-Za-z_]\w*")
BASELINE_LINE_RE = re.compile(r"^\S+(\s+#.*)?$")

BASELINE_HEADER = """\
# contract_consumer_baseline.txt -- only-shrink register (gate: scripts/check_contract_consumers.py)
# One line per nexus-contracts (L0) pub type/trait whose CONSUMER CRATE COUNT
# is < 2 in production scope (v3.0 5.2 WI-A1; first run 2026-10-07 = full
# exemption snapshot, per the approved acceptance).
#   * a def reaching >= 2 consumers -> delete its line in the wiring batch
#     (the gate reports STALE otherwise)
#   * a new line REQUIRES a reason comment after human review; never silence a
#     GAP by editing this file without that review
#   * consumer counting dialect + disclosed boundaries: see the gate header
# Format: NAME  [# reason]
"""


def read_production_text(path, flatten):
    """One file -> PRODUCTION scope text (test tails + comment lines removed).
    Rule kept identical to check_dead_contract_apis.py::count_production_uses:
    a `#[cfg(test)]` line only starts the excluded tail when the next code line
    declares a module. flatten=True collapses all whitespace (connection
    scanning across multi-line `use` statements); flatten=False keeps newlines
    (definition extraction needs the line anchors for `^pub` -- flattening them
    away was a self-inflicted vacuous-green, caught by --selftest first).
    """
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""
    kept = []
    in_test_tail = False
    lines = text.splitlines()
    for idx, line in enumerate(lines):
        if not in_test_tail and CFG_TEST_RE.match(line):
            for later in lines[idx + 1:]:
                s = later.strip()
                if not s or s.startswith("#") or s.startswith("///") or s.startswith("//!"):
                    continue
                in_test_tail = bool(MODULE_DECL_RE.match(later))
                break
            continue
        if in_test_tail:
            continue
        if line.lstrip().startswith("//"):
            continue
        kept.append(line)
    text = "\n".join(kept)
    return re.sub(r"\s+", " ", text) if flatten else text


def read_production_blob(path):
    """Flattened production text (consumer-connection scanning)."""
    return read_production_text(path, flatten=True)


def build_model(crates_root):
    """Scan the workspace once. Returns a model dict:

    defs        -- set of pub type/trait/alias names defined in nexus-contracts
    idents      -- {crate_name: rust_ident}
    tokens      -- {crate_name: set of names reached via direct connections}
    exports     -- {crate_name: set of names re-exported from nexus_contracts}
    blobs       -- {crate_name: flattened production text} (for hop queries)
    """
    model = {"defs": set(), "idents": {}, "tokens": {}, "exports": {}, "blobs": {}}
    contracts_src = crates_root / CONTRACTS_DIR_NAME / "src"
    if not contracts_src.is_dir():
        return None
    for p in contracts_src.rglob("*.rs"):
        text = read_production_text(p, flatten=False)
        for m in DEF_RE.finditer(text):
            model["defs"].add(m.group(1))
    for cd in sorted(crates_root.iterdir()):
        if not cd.is_dir() or cd.name == CONTRACTS_DIR_NAME:
            continue
        src = cd / "src"
        if not src.is_dir():
            continue
        name = cd.name
        model["idents"][name] = name.replace("-", "_")
        parts, toks, exports = [], set(), set()
        for p in src.rglob("*.rs"):
            blob = read_production_blob(p)
            parts.append(blob)
            for chain in CHAIN_RE.findall(blob):
                toks.update(seg.strip() for seg in chain.split("::")[1:] if seg.strip())
            for stmt in USE_RE.findall(blob):
                toks.update(TOK_RE.findall(stmt))
            for stmt in EXPORT_RE.findall(blob):
                exports.update(TOK_RE.findall(stmt))
        model["tokens"][name] = toks
        model["exports"][name] = exports
        model["blobs"][name] = " ".join(parts)
    return model


def consumer_crates(model, name, hop_cache):
    """Distinct consumer crates of `name` (direct connections + 1 re-export hop)."""
    consumers = {c for c, toks in model["tokens"].items() if name in toks}
    hubs = (h for h, ex in model["exports"].items() if name in ex)
    for hub in hubs:
        rest = [c for c in model["blobs"] if c != hub and c not in consumers]
        if not rest:
            continue
        key = (hub, name)
        rx = hop_cache.get(key)
        if rx is None:
            rx = re.compile(r"\b" + re.escape(model["idents"][hub]) + r"\b[^;]*\b"
                            + re.escape(name) + r"\b[^;]*;")
            hop_cache[key] = rx
        for c in rest:
            if rx.search(model["blobs"][c]):
                consumers.add(c)
    return consumers


def compute_gaps(model):
    """Sorted list of def names with < 2 consuming crates."""
    hop_cache = {}
    gaps = []
    for name in sorted(model["defs"]):
        if len(consumer_crates(model, name, hop_cache)) < 2:
            gaps.append(name)
    return gaps


def load_baseline(path):
    """Baseline names; malformed line or missing file -> SystemExit(2)."""
    if not os.path.exists(path):
        print("[CONFIG] baseline missing: %s" % path)
        sys.exit(2)
    entries = set()
    with open(path, encoding="utf-8-sig", errors="ignore") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            if not BASELINE_LINE_RE.match(line):
                print("[CONFIG] malformed baseline line (want `NAME [# reason]`): %r"
                      % raw.rstrip())
                sys.exit(2)
            entries.add(line)
    return entries


def judge(gaps, baseline):
    """Returns (exit_code, new_gaps, stale_entries)."""
    gap_set = set(gaps)
    new_gaps = sorted(gap_set - baseline)
    stale = sorted(baseline - gap_set)
    return (1 if (new_gaps or stale) else 0), new_gaps, stale


def selftest():
    """Fixture-driven self-test over REAL files through the REAL scan path
    (tempdir, no in-memory shortcuts). Runs before the real verdict in CI."""
    import tempfile

    def check(label, cond):
        print("  [%s] %s" % ("PASS" if cond else "FAIL", label))
        return bool(cond)

    ok = True
    with tempfile.TemporaryDirectory(prefix="g91_st_") as tmp:
        root = Path(tmp)
        crates = root / "crates"

        def put(rel, body):
            p = crates / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(body, encoding="utf-8")

        put("nexus-contracts/src/a.rs",
            "pub struct Alpha;\n"
            "pub enum Beta { X }\n"
            "pub trait Gamma {}\n"
            "#[cfg(test)]\nmod tests {\n    pub struct Hidden;\n"
            "    fn t() { let _ = Hidden; let _ = nexus_contracts::a::Beta::X; }\n}\n")
        put("consumer-two/src/lib.rs",
            "use nexus_contracts::a::{Alpha, Beta};\n"
            "fn f(_a: Alpha) { let _ = Beta::X; }\n")
        put("consumer-one/src/lib.rs",
            "fn f() { let _: Option<nexus_contracts::a::Gamma> = None; }\n")
        put("hub-crate/src/lib.rs", "pub use nexus_contracts::a::Alpha;\n")
        put("hub-consumer/src/lib.rs",
            "use hub_crate::Alpha;\nfn g(a: Alpha) { let _ = a; }\n")
        put("noise/src/lib.rs",
            "// use nexus_contracts::a::Gamma;  (comment must not count)\n"
            "fn f() {}\n"
            "#[cfg(test)]\nmod tests {\n"
            "    fn t() { let _ = nexus_contracts::a::Beta; }\n}\n")

        model = build_model(crates)
        ok &= check("model built", model is not None)
        if model is None:
            return 1
        ok &= check("defs enumerated (test-tail Hidden excluded)",
                    model["defs"] == {"Alpha", "Beta", "Gamma"})
        hop_cache = {}
        c_alpha = consumer_crates(model, "Alpha", hop_cache)
        c_beta = consumer_crates(model, "Beta", hop_cache)
        c_gamma = consumer_crates(model, "Gamma", hop_cache)
        ok &= check("Alpha direct + pub-use + hop-1 == 3 consumers",
                    c_alpha == {"consumer-two", "hub-crate", "hub-consumer"})
        ok &= check("Beta counts only the direct import (test tail excluded)",
                    c_beta == {"consumer-two"})
        ok &= check("Gamma comment line does not count",
                    c_gamma == {"consumer-one"})
        gaps = compute_gaps(model)
        ok &= check("<2-consumer gaps == [Beta, Gamma]", gaps == ["Beta", "Gamma"])

        # judge three-state ------------------------------------------------
        base_path = root / "baseline.txt"
        base_path.write_text("# header comment ok\nBeta\nGamma  # known debt\n",
                             encoding="utf-8")
        baseline = load_baseline(str(base_path))
        rc, new, stale = judge(gaps, baseline)
        ok &= check("registered full-exemption state -> rc=0",
                    rc == 0 and new == [] and stale == [])
        rc, new, stale = judge(gaps, {"Beta"})
        ok &= check("unregistered Gamma -> GAP red", rc == 1 and new == ["Gamma"])
        rc, new, stale = judge(gaps, {"Beta", "Gamma", "Alpha"})
        ok &= check("wired Alpha left in register -> STALE red",
                    rc == 1 and stale == ["Alpha"])

        # undecidable inputs must exit 2, never pass silently --------------
        bad = root / "bad_baseline.txt"
        bad.write_text("Beta\nnot a valid name!!${}\n", encoding="utf-8")
        for label, path in (("missing baseline", str(root / "nope.txt")),
                            ("malformed baseline line", str(bad))):
            try:
                load_baseline(path)
                ok &= check("%s -> SystemExit(2)" % label, False)
            except SystemExit as e:
                ok &= check("%s -> SystemExit(%s)" % (label, e.code), e.code == 2)

        # missing crates dir -> model is None (main must exit 2)
        ok &= check("missing contracts dir -> model None",
                    build_model(root / "no_crates") is None)

    print("  RESULT: %s" % ("PASS" if ok else "FAIL"))
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--emit", action="store_true",
                    help="print the full baseline-format document for the current gap set")
    args = ap.parse_args()
    if args.selftest:
        return selftest()

    crates_root = ROOT / "crates"
    model = build_model(crates_root)
    if model is None:
        print("[CONFIG] contracts crate not found under %s" % crates_root)
        return 2
    if len(model["defs"]) < 50:
        print("[CONFIG] suspiciously few defs (%d) - the definition detector may be "
              "blind; refusing a verdict (undecidable)" % len(model["defs"]))
        return 2
    gaps = compute_gaps(model)

    if args.emit:
        sys.stdout.write(BASELINE_HEADER)
        for name in gaps:
            print(name)
        return 0

    baseline = load_baseline(str(BASELINE))
    rc, new_gaps, stale = judge(gaps, baseline)
    print("[INFO] nexus-contracts defs: %d (baseline: %d; gaps<2: %d; >=2-consumer rule)"
          % (len(model["defs"]), len(baseline), len(gaps)))
    if rc == 0:
        print("[OK] every L0 contract def has >= 2 workspace consumers or is a "
              "registered (only-shrink) exemption")
        return 0
    for name in new_gaps:
        print("[FAIL] GAP: '%s' has < 2 workspace consumers and is not registered"
              % name)
    for name in stale:
        print("[FAIL] STALE: baseline entry '%s' is no longer a <2-consumer def"
              % name)
    print("  Exemption path: a def that really must ship early is registered in")
    print("  scripts/contract_consumer_baseline.txt WITH a reason comment after")
    print("  human review; a def reaching >= 2 consumers deletes its line in the")
    print("  same batch. Never edit the register to silence a red without that review.")
    return rc


if __name__ == "__main__":
    import gate_rc  # entry-only: a crash must exit 2, never borrow 1 to fake "judged and red" (F32/F33)
    sys.exit(gate_rc.run(main))
