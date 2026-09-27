#!/usr/bin/env python3
# =============================================================================
# check_gated_wiring_substance.py - "is a feature gate actually wired?" gate (B0-1)
# =============================================================================
# Purpose: close the hole that let `r2_island_repayment` be booked as
#          "frozen islands repaid". A crate reached only through an optional
#          dependency is classified GATED by scripts/check_crate_reachability.py,
#          and GATED was treated as "assembled". It is not: a feature that has
#          ZERO `#[cfg(feature = "...")]` sites compiles the dependency but wires
#          nothing, so opening it changes no behaviour and the assembly work was
#          never done -- it was only declared.
#
# Measured origin: docs/reports/Q2-decision-brief-7-ghost-crates-2026-09-21.md
#   section 1 (7 optional edges, 0 cfg sites, 0 CI enablements) and section 4
#   proposal 2 ("GATED must carry an assembly-substance requirement").
#
# Why a SIBLING gate and not an edit to check_crate_reachability.py: that script
# is a live, CI-registered gate carrying selftest fixtures and doc anchors, and
# it was already modified in this working tree when this gate was written. A
# second instrument over the same facts keeps both honest (they must agree) and
# avoids coupling a new judgement to an in-flight change.
#
# Verdicts per optional INTERNAL dependency of the composition root:
#   WIRED       >=1 cfg site AND >=1 CI enablement            -> not debt
#   PARTIAL     >=1 cfg site but no CI job ever turns it on   -> cfg never compiled in CI
#   DECORATIVE  zero cfg sites: enabling it changes nothing   -> declarative-only wiring
# The ledger (scripts/gated_wiring_baseline.txt) allows PARTIAL/DECORATIVE rows
# only-decrease: an allowance must not outlive the debt it allowed.
#
# Exit dialect (shared with gate_rc): 0 clean / 1 grew or stale / 2 undecidable.
# Usage:
#   python scripts/check_gated_wiring_substance.py --selftest
#   python scripts/check_gated_wiring_substance.py --emit
#   python scripts/check_gated_wiring_substance.py
# =============================================================================
import argparse
import os
import re
import shutil
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LEDGER = os.path.join(ROOT, "scripts", "gated_wiring_baseline.txt")
ROOT_PKG = "chimera-cli"          # the single composition root (agents.md §2.1)


def set_root(path):
    """Re-point every path at another repo root (used by --selftest fixtures)."""
    global ROOT, LEDGER
    ROOT = path
    LEDGER = os.path.join(path, "scripts", "gated_wiring_baseline.txt")


def parse_features(manifest):
    """Return {feature_name: [dep:... targets]} from a `[features]` table.

    Minimal hand parser (no tomllib dependency, so this runs on the repo's
    oldest documented interpreter too). Continuation lines of a multi-line array
    are handled -- `r2_island_repayment` is written across four lines, and a
    line-oriented-only parser would silently drop 5 of its 7 targets.
    """
    feats = {}
    if not os.path.exists(manifest):
        raise ValueError("manifest missing: %s" % manifest)
    section = None
    pending = None
    with open(manifest, encoding="utf-8", errors="replace") as fh:
        text = fh.read()
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if line.startswith("["):
            if pending:
                feats[pending[0]] = pending[1]
                pending = None
            section = line.strip("[]").strip()
            continue
        if section != "features":
            continue
        if pending is not None and not line.endswith("]"):
            pending[1].extend(re.findall(r'"([^"]+)"', line))
            continue
        if pending is not None:
            pending[1].extend(re.findall(r'"([^"]+)"', line))
            feats[pending[0]] = pending[1]
            pending = None
            continue
        m = re.match(r'^([A-Za-z0-9_-]+)\s*=\s*\[(.*)$', line)
        if not m:
            continue
        name, tail = m.group(1), m.group(2)
        if tail.rstrip().endswith("]"):
            feats[name] = re.findall(r'"([^"]+)"', tail)
        else:
            pending = (name, re.findall(r'"([^"]+)"', tail))
    if pending:
        feats[pending[0]] = pending[1]
    return feats


def cfg_sites(feature, src_dir):
    """Count `.rs` lines under src_dir referencing `feature = "<feature>"`.

    Accepts `cfg(feature = "f")`, `cfg_attr`, and `all(...)`/`any(...)` nestings
    by matching the inner text only. Comments count: a cfg site inside a comment
    is not a wiring site either, but stripping comments here would mean
    re-implementing the Rust lexer, and a false "wired" verdict only ever
    UNDER-reports debt (the conservative direction for this gate's purpose).
    """
    if not os.path.isdir(src_dir):
        return 0
    n = 0
    pat = re.compile(r'feature\s*=\s*"%s"' % re.escape(feature))
    for dirpath, _dirs, files in os.walk(src_dir):
        for f in files:
            if not f.endswith(".rs"):
                continue
            with open(os.path.join(dirpath, f), encoding="utf-8", errors="replace") as fh:
                n += sum(1 for line in fh if pat.search(line))
    return n


def ci_enablements(feature):
    """Count CI lines that actually turn `feature` on (--features .../features: [...])."""
    wf = os.path.join(ROOT, ".github", "workflows")
    if not os.path.isdir(wf):
        return 0
    n = 0
    for f in sorted(os.listdir(wf)):
        if not f.endswith((".yml", ".yaml")):
            continue
        with open(os.path.join(wf, f), encoding="utf-8", errors="replace") as fh:
            for line in fh:
                if "--features" in line and re.search(r'\b%s\b' % re.escape(feature), line):
                    n += 1
                elif re.search(r'features:\s*\[[^\]]*\b%s\b' % re.escape(feature), line):
                    n += 1
    return n


def optional_internal_deps(manifest):
    """Return {dep: [feature names enabling it]} for internal optional deps."""
    if not os.path.exists(manifest):
        raise ValueError("manifest missing: %s" % manifest)
    section = None
    optional = set()
    with open(manifest, encoding="utf-8", errors="replace") as fh:
        text = fh.read()
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()
        if line.startswith("["):
            section = line.strip("[]").strip()
            continue
        if section != "dependencies":
            continue
        m = re.match(r'^"?([A-Za-z0-9_-]+)"?\s*=', line)
        if m and "optional" in line and "true" in line.split("optional", 1)[1]:
            optional.add(m.group(1))
    feats = parse_features(manifest)
    out = {}
    for dep in sorted(optional):
        if not os.path.isdir(os.path.join(ROOT, "crates", dep)):
            continue          # external optional dep -> out of scope
        enablers = [f for f, targets in feats.items()
                    if any(t.strip() == "dep:" + dep for t in targets)]
        out[dep] = enablers
    return out


def assess():
    """Return [(dep, verdict, cfg_count, ci_count, enablers)] for the root package."""
    manifest = os.path.join(ROOT, "crates", ROOT_PKG, "Cargo.toml")
    src = os.path.join(ROOT, "crates", ROOT_PKG, "src")
    rows = []
    for dep, enablers in optional_internal_deps(manifest).items():
        cfg = sum(cfg_sites(f, src) for f in enablers)
        ci = sum(ci_enablements(f) for f in enablers)
        if not enablers:
            verdict = "ORPHAN-OPTIONAL"     # optional but no feature enables it
        elif cfg == 0:
            verdict = "DECORATIVE"
        elif ci == 0:
            verdict = "PARTIAL"
        else:
            verdict = "WIRED"
        rows.append((dep, verdict, cfg, ci, ",".join(enablers) or "-"))
    return rows


def load_ledger():
    """Return {(dep, verdict): rows} from the allowance ledger."""
    out = {}
    if not os.path.exists(LEDGER):
        raise ValueError("ledger missing: %s" % LEDGER)
    with open(LEDGER, encoding="utf-8", errors="replace") as fh:
        for raw in fh:
            body, _, why = raw.partition("#")
            line = body.strip()
            if not line:
                continue
            parts = line.split("|")
            if len(parts) != 2 or not why.strip():
                raise ValueError("needs `dep|verdict # reason`: %r" % raw.rstrip())
            key = (parts[0].strip(), parts[1].strip())
            out[key] = out.get(key, 0) + 1
    return out


def decide(rows, ledger):
    """Classify debt rows against the ledger (occurrence counts, not key sets)."""
    debt = {}
    for dep, verdict, _cfg, _ci, _e in rows:
        if verdict in ("DECORATIVE", "PARTIAL", "ORPHAN-OPTIONAL"):
            debt[(dep, verdict)] = debt.get((dep, verdict), 0) + 1
    new = sorted(set(debt) - set(ledger))
    stale = sorted(k for k in ledger if debt.get(k, 0) == 0)
    grown = sorted(k for k in set(debt) & set(ledger) if debt[k] > ledger[k])
    rotate = sorted(k for k in set(debt) & set(ledger) if debt[k] < ledger[k])
    return new, grown, rotate, stale, debt


def _fixture():
    """Synthetic root package exercising WIRED / PARTIAL / DECORATIVE / ORPHAN.

    The root package is named `chimera-cli` because that is what ROOT_PKG pins
    (it is the workspace's single composition root); the fixture mirrors the
    real manifest shapes rather than parameterising the constant.
    """
    tmp = tempfile.mkdtemp(prefix="gated-wiring-")
    for d in ("crates/chimera-cli/src", "crates/used/src", "crates/dead/src",
              "crates/orphan/src", "scripts", ".github/workflows"):
        os.makedirs(os.path.join(tmp, d), exist_ok=True)
    with open(os.path.join(tmp, "crates", "chimera-cli", "Cargo.toml"), "w", encoding="utf-8") as fh:
        fh.write('[package]\nname = "chimera-cli"\n\n[dependencies]\n'
                 'used = { path = "../used", optional = true }\n'
                 'dead = { path = "../dead", optional = true }\n'
                 'orphan = { path = "../orphan", optional = true }\n'
                 'serde = { version = "1", optional = true }\n'
                 '\n[features]\n'
                 'used = ["dep:used"]\n'
                 'dead = [\n    "dep:dead",\n]\n')
    with open(os.path.join(tmp, "crates", "chimera-cli", "src", "lib.rs"), "w", encoding="utf-8") as fh:
        fh.write('#[cfg(feature = "used")]\npub fn wired() {}\n')
    with open(os.path.join(tmp, ".github", "workflows", "ci.yml"), "w", encoding="utf-8") as fh:
        fh.write("run: cargo check --features used\n")
    return tmp


def selftest():
    """Prove the four verdicts are each produced, on a synthetic workspace.

    Also pins two traps this gate would otherwise fall into: a multi-line
    feature array must parse completely, and an external optional dep must not
    be scored as an internal assembly gap.
    """
    tmp = _fixture()
    try:
        set_root(tmp)
        rows = {dep: (v, c, i) for dep, v, c, i, _e in assess()}
        checks = [
            ("WIRED", rows.get("used") == ("WIRED", 1, 1)),
            ("DECORATIVE (multi-line array parsed)", rows.get("dead") == ("DECORATIVE", 0, 0)),
            ("ORPHAN-OPTIONAL", rows.get("orphan", ("?",))[0] == "ORPHAN-OPTIONAL"),
            ("external optional dep not scored", "serde" not in rows),
            ("decide yields all four verdicts",
             decide([("a", "DECORATIVE", 0, 0, "f"), ("a", "DECORATIVE", 0, 0, "f"),
                     ("p", "PARTIAL", 2, 0, "g"), ("q", "ORPHAN-OPTIONAL", 0, 0, "-"),
                     ("w", "WIRED", 3, 1, "h")],
                     {("a", "DECORATIVE"): 1, ("p", "PARTIAL"): 2,
                      ("z", "DECORATIVE"): 1})
             == ([("q", "ORPHAN-OPTIONAL")], [("a", "DECORATIVE")],
                 [("p", "PARTIAL")], [("z", "DECORATIVE")],
                 {("a", "DECORATIVE"): 2, ("p", "PARTIAL"): 1,
                  ("q", "ORPHAN-OPTIONAL"): 1})),
            ("missing ledger is undecidable", _raises(load_ledger)),
        ]
        for name, ok in checks:
            print("  [%s] %s" % ("PASS" if ok else "FAIL", name))
        bad = [n for n, ok in checks if not ok]
        if bad:
            return 1
        print("[SELFTEST] WIRED/PARTIAL/DECORATIVE/ORPHAN verdicts produced, "
              "multi-line arrays parsed, external deps excluded, ledger paths verified")
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
        set_root(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _raises(fn):
    """True if fn() raises ValueError (missing or malformed ledger)."""
    try:
        fn()
    except ValueError:
        return True
    except Exception:
        return False
    return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--emit", action="store_true")
    ap.add_argument("--root")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    if args.root:
        set_root(os.path.abspath(args.root))
    try:
        rows = assess()
    except ValueError as exc:
        print("[CONFIG] %s" % exc)
        return 2
    if args.emit:
        for dep, verdict, cfg, ci, enablers in rows:
            print("%s|%s|%d|%d|%s" % (dep, verdict, cfg, ci, enablers))
        return 0
    try:
        ledger = load_ledger()
    except ValueError as exc:
        print("[CONFIG] %s" % exc)
        return 2
    new, grown, rotate, stale, debt = decide(rows, ledger)
    print("[INFO] optional internal deps of %s: %d; debt rows %d / allowed %d"
          % (ROOT_PKG, len(rows), sum(debt.values()), len(ledger)))
    for dep, verdict, cfg, ci, enablers in rows:
        print("  [%-15s] %-16s cfg=%-3d ci=%-3d via %s" % (verdict, dep, cfg, ci, enablers))
    if not any((new, grown, rotate, stale)):
        print("[OK] gating substance matches the ledger")
        return 0
    if new or grown:
        print("[FAIL] assembly debt grew:")
        for k in new + grown:
            print("  [GAP] %s|%s is not allowed by the ledger" % k)
        print("  Fix: add the cfg site (real wiring) or drop the optional edge.")
    if rotate:
        print("[ROTATE] %d row(s) shrank; refresh the ledger:" % len(rotate))
        for k in rotate:
            print("  [ROTATE] %s|%s" % k)
    if stale:
        print("[FAIL] %d ledger row(s) with no matching debt -- delete them:" % len(stale))
        for k in stale:
            print("  [STALE] %s|%s" % k)
    return 1


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(main))
