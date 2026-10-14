#!/usr/bin/env python3
# =============================================================================
# check_nexus_core_dep_freeze.py - nexus-core production dependency freeze gate
#                                 (G-90, v3.0 5.2 WI-F1 / v3.1 5-A1)
# =============================================================================
# Purpose: mechanically freeze the PRODUCTION [dependencies] set of the L1 hub
#          crate `nexus-core` (two-way ratchet: no new dep, no stale entry).
#
# Why this crate: nexus-core is the smallest-dependency hub of the ten-layer
#   architecture; its production dep face is the factual base of the
#   minimal-dependency argument. Growth there must be a reviewed decision,
#   never an accident of a manifest edit.
#
# Why a bespoke parser (NOT check_declared_dep_usage.parse_declared):
#   parse_declared returns a DICT {dep: section}, so a later section silently
#   OVERWRITES an earlier same-name declaration. Measured on this repo
#   (2026-10-06): nexus-core declares serde_json in [dependencies] AND in
#   [dev-dependencies]; parse_declared reports prod=14 -- the real production
#   declaration is eaten by the dev one. This gate freezes ALL production deps,
#   so a lost name would manufacture a fake STALE red. The same dict overwrite
#   undercounts for the E-a gate too; fixing E-a is out of scope here (known
#   boundary, deliberately not touched).
#
# Five verdicts (judge):
#   NEW                prod dep absent from baseline -> 1 (register WITH a
#                      reason comment after human review; never edit the
#                      baseline to silence a red without that review)
#   STALE              baseline entry with no prod declaration -> 1 (delete the
#                      line in the same batch that removes the dependency)
#   NON_WORKSPACE_DECL prod dep not declared `{ workspace = true }` -> 1
#                      (closes the ADR-196 D5 gap: grep-style workspace checks
#                      only see workspace=true lines, so `foo = "1.0"` slips by)
#   UNSUPPORTED_FORM   manifest shape this lightweight parser does not model
#                      ([dependencies.x] subtables, target.*.dependencies,
#                      dotted/quoted keys, multi-line tables) -> fail-closed 1
#   DUPLICATE_DECL     same dep declared twice in the section -> 1
#   Manifest or baseline missing/malformed -> 2 (undecidable; never a pass)
#
# Scope: [dependencies] ONLY. dev-dependencies / build-dependencies are not
#   frozen (test/bench deps do not enter the production graph).
#
# Baseline: scripts/nexus_core_dep_freeze.txt, one name per line. --emit prints
#   the current set for (re)generation; regeneration REQUIRES human diff review
#   (a bulk overwrite would launder unreviewed growth - same discipline as
#   dep_edge_freeze.txt).
#
# Encoding: all-ASCII (project script convention)
# Exit code: 0 = clean, 1 = gap found, 2 = usage/config error
# Usage:
#   python scripts/check_nexus_core_dep_freeze.py --selftest
#   python scripts/check_nexus_core_dep_freeze.py --emit
#   python scripts/check_nexus_core_dep_freeze.py
# =============================================================================
import argparse
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MANIFEST = os.path.join(ROOT, "crates", "nexus-core", "Cargo.toml")
BASELINE = os.path.join(ROOT, "scripts", "nexus_core_dep_freeze.txt")

NAME_RE = re.compile(r"^([A-Za-z0-9_\-]+)\s*=\s*(.*)$")
BASELINE_NAME_RE = re.compile(r"^[A-Za-z0-9_\-]+$")


def parse_prod_deps(manifest_path):
    """Return [(name, form)] for the production [dependencies] section.

    form is "workspace" (declared `{ workspace = true }`), "foreign" (any other
    value form -- a locally pinned version), or "unsupported" (a manifest shape
    this parser refuses to guess about; see header). The list preserves order
    AND duplicates so judge() can see both -- a dict here would re-introduce
    exactly the parse_declared overwrite pitfall this gate exists to avoid.
    """
    deps = []
    in_prod = False
    with open(manifest_path, encoding="utf-8-sig", errors="ignore") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            if line.startswith("["):
                head = line.strip("[]").strip()
                in_prod = head == "dependencies"
                if not in_prod and (head.startswith("dependencies.")
                                    or (head.startswith("target.")
                                        and head.endswith(".dependencies"))):
                    # fail-closed: a shape that CAN carry production deps but
                    # that this parser does not model
                    deps.append((head, "unsupported"))
                continue
            if not in_prod:
                continue
            m = NAME_RE.match(line)
            if not m:
                # dotted keys (`foo.workspace = true`), quoted names and
                # multi-line table tails would otherwise become silent
                # freeze bypasses -- flag them instead of skipping
                deps.append((line, "unsupported"))
                continue
            name, value = m.group(1), m.group(2)
            ws = "workspace=true" in value.replace(" ", "")
            deps.append((name, "workspace" if ws else "foreign"))
    return deps


def load_baseline(path):
    """Return set of frozen dep names. Malformed lines fail loud (exit 2)."""
    entries = set()
    if not os.path.exists(path):
        print("[CONFIG] baseline missing: %s" % path)
        sys.exit(2)
    with open(path, encoding="utf-8-sig", errors="ignore") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            if not BASELINE_NAME_RE.match(line):
                print("[CONFIG] malformed baseline line (want one dep name): %r"
                      % raw.rstrip())
                sys.exit(2)
            entries.add(line)
    return entries


def judge(prod_deps, baseline):
    """Adjudicate parsed prod deps against the baseline set.

    Returns (exit_code, findings); findings are (kind, detail) pairs.
    """
    findings = []
    for name, form in prod_deps:
        if form == "unsupported":
            findings.append(("UNSUPPORTED_FORM",
                             "manifest form not modeled by the freeze parser "
                             "(fail-closed): %s" % name))
    counts = {}
    for name, form in prod_deps:
        if form != "unsupported":
            counts[name] = counts.get(name, 0) + 1
    for name in sorted(n for n, c in counts.items() if c > 1):
        findings.append(("DUPLICATE_DECL",
                         "`%s` declared %d times in [dependencies]"
                         % (name, counts[name])))
    prod = set(n for n, f in prod_deps if f != "unsupported")
    for name in sorted(prod - baseline):
        findings.append(("NEW",
                         "production dependency `%s` is not in the freeze "
                         "baseline" % name))
    for name in sorted(baseline - prod):
        findings.append(("STALE",
                         "baseline entry `%s` no longer declared in "
                         "[dependencies]" % name))
    for name, form in prod_deps:
        if form == "foreign":
            findings.append(("NON_WORKSPACE_DECL",
                             "`%s` is not declared `{ workspace = true }` "
                             "(externally pinned version)" % name))
    return (1 if findings else 0), findings


def selftest():
    """Fixture-driven self-test over REAL files through the REAL parse path
    (tempdir, no in-memory shortcuts). CI runs this before the real verdict."""
    import tempfile

    def run_case(label, manifest_body, baseline, want_rc, want_kind):
        path = os.path.join(tmpdir, "Cargo.toml")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(manifest_body)
        rc, findings = judge(parse_prod_deps(path), baseline)
        kinds = sorted(k for k, _ in findings)
        bad = rc != want_rc or (want_kind is not None and want_kind not in kinds)
        print("  [%s] %s -> rc=%d kinds=%s" % ("FAIL" if bad else "PASS", label, rc, kinds))
        return not bad

    ok = True
    with tempfile.TemporaryDirectory(prefix="g90_st_") as tmpdir:
        base = {"serde", "tokio"}
        # fixture 1: clean manifest matching the baseline -> green
        ok &= run_case("clean manifest matches baseline",
                       "[dependencies]\n"
                       "serde = { workspace = true }\n"
                       "tokio = { workspace = true }\n",
                       base, 0, None)
        # fixture 2 (the ask-named one): injected dep absent from baseline -> NEW
        ok &= run_case("injected dep not in baseline",
                       "[dependencies]\n"
                       "serde = { workspace = true }\n"
                       "tokio = { workspace = true }\n"
                       "fake-dep = { workspace = true }\n",
                       base, 1, "NEW")
        # fixture 3: externally pinned version -> NON_WORKSPACE_DECL
        ok &= run_case("externally pinned version",
                       "[dependencies]\n"
                       "serde = { workspace = true }\n"
                       "tokio = { workspace = true }\n"
                       "fake-dep = \"1.0\"\n",
                       base, 1, "NON_WORKSPACE_DECL")
        # fixture 4: baseline entry outlives the declaration -> STALE
        ok &= run_case("baseline entry no longer declared",
                       "[dependencies]\n"
                       "serde = { workspace = true }\n",
                       base, 1, "STALE")
        # fixture 5: same key twice in the section -> DUPLICATE_DECL
        ok &= run_case("duplicate declaration",
                       "[dependencies]\n"
                       "serde = { workspace = true }\n"
                       "serde = { workspace = true }\n"
                       "tokio = { workspace = true }\n",
                       base, 1, "DUPLICATE_DECL")
        # fixture 6: shapes the parser refuses to model -> fail-closed
        ok &= run_case("subtable header [dependencies.x]",
                       "[dependencies]\n"
                       "serde = { workspace = true }\n"
                       "[dependencies.fake-dep]\n"
                       "version = \"1.0\"\n",
                       base, 1, "UNSUPPORTED_FORM")
        ok &= run_case("dotted-key declaration",
                       "[dependencies]\n"
                       "serde = { workspace = true }\n"
                       "fake-dep.workspace = true\n",
                       base, 1, "UNSUPPORTED_FORM")
        ok &= run_case("target-specific deps table",
                       "[dependencies]\n"
                       "serde = { workspace = true }\n"
                       "[target.'cfg(unix)'.dependencies]\n"
                       "fake-dep = { workspace = true }\n",
                       base, 1, "UNSUPPORTED_FORM")
        # fixture 7: undecidable inputs must exit 2, never pass silently
        bad_base = os.path.join(tmpdir, "bad_baseline.txt")
        with open(bad_base, "w", encoding="utf-8") as fh:
            fh.write("serde\nnot a valid name!!\n")
        for label, path in (("missing baseline",
                             os.path.join(tmpdir, "no_such_baseline.txt")),
                            ("malformed baseline line", bad_base)):
            try:
                load_baseline(path)
                print("  [FAIL] %s -> no SystemExit(2)" % label)
                ok = False
            except SystemExit as e:
                good = e.code == 2
                print("  [%s] %s -> SystemExit(%s)"
                      % ("PASS" if good else "FAIL", label, e.code))
                ok = ok and good
    print("  RESULT: %s" % ("PASS" if ok else "FAIL"))
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--emit", action="store_true",
                    help="print the current production dep set in baseline format, then exit")
    args = ap.parse_args()
    if args.selftest:
        return selftest()

    if not os.path.exists(MANIFEST):
        print("[CONFIG] manifest missing: %s" % MANIFEST)
        return 2
    prod = parse_prod_deps(MANIFEST)

    if args.emit:
        for name in sorted(n for n, f in prod if f != "unsupported"):
            print(name)
        return 0

    baseline = load_baseline(BASELINE)
    rc, findings = judge(prod, baseline)
    prod_names = set(n for n, f in prod if f != "unsupported")
    print("[INFO] nexus-core production deps: %d (baseline: %d; two-way ratchet)"
          % (len(prod_names), len(baseline)))
    if rc == 0:
        print("[OK] nexus-core dependency freeze clean: no dep added or removed "
              "without baseline registration; all declarations workspace-inherited")
        return 0
    by_kind = {}
    for kind, detail in findings:
        by_kind.setdefault(kind, []).append(detail)
    for kind in sorted(by_kind):
        print("[FAIL] %s: %d" % (kind, len(by_kind[kind])))
        for detail in by_kind[kind]:
            print("  [%s] %s" % (kind, detail))
    print("  Exemption path: a NEW dep must be reviewed for minimality and layer")
    print("  direction, then registered in scripts/nexus_core_dep_freeze.txt WITH a")
    print("  reason comment; never edit the baseline to silence a red without that")
    print("  review. A removed dep deletes its baseline line in the same batch.")
    print("  NON_WORKSPACE_DECL: convert the declaration to `{ workspace = true }`.")
    print("  UNSUPPORTED_FORM: reshape to the current manifest form, or extend the")
    print("  parser deliberately (fail-closed by design).")
    return rc


if __name__ == "__main__":
    import gate_rc  # entry-only: a crash must exit 2, never borrow 1 to fake "judged and red" (F32/F33)
    sys.exit(gate_rc.run(main))
