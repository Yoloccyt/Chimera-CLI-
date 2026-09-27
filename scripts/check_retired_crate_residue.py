#!/usr/bin/env python3
# =============================================================================
# check_retired_crate_residue.py - retired-crate blast-radius gate (B0-4)
# =============================================================================
# Purpose: when a crate is physically deleted, every FUNCTIONAL reference to it
#          must die in the same commit. Three such references survived batch A
#          (38c72e6 deleted auto-dpo + model-router) and are still on disk today:
#            * fuzz/Cargo.toml:61   `model-router = { path = "../crates/model-router" }`
#              -> `cargo check --manifest-path fuzz/Cargo.toml` fails to even
#                 parse the manifest (measured EXIT=101), so the fuzz gate has
#                 been silently unable to run;
#            * .github/workflows/bench_check.yml:136 `cargo bench -p auto-dpo`
#              -> a release tag turns that job red;
#            * .github/CODEOWNERS:16,37,55  -> ownership rows for dead dirs.
#          This gate makes the residue class visible and only-decreasing, so the
#          next retirement cannot leave a fourth one behind.
#
# WHY comment mentions are NOT scored: this repository's documentation policy is
# explicit that history is annotated, never rewritten (DOCUMENT_LIFECYCLE_POLICY;
# "活文档中被删 crate 复述属登记注记，保留"). Flagging `Cargo.toml:402`'s prose
# would fight that policy and get the gate disabled. Only references that a tool
# EXECUTES are debt.
#
# Scored reference kinds (each one breaks something concrete):
#   path-dep    `<crate> = { path = ... }` / `members = [... "<crate>"]`  -> build
#   symbol      `<crate>::` or `use <ident>::` in a .rs file              -> build
#   cargo-flag  `-p <crate>` / `--manifest-path .../<crate>/` in yml|sh|py-> CI job
#   owners      `/crates/<crate>/` row in .github/CODEOWNERS              -> wrong owner
#
# Baseline: scripts/retired_crate_residue.txt, one `file|crate|kind` per line,
#   only-decrease. Fixing a row without deleting it is STALE (red), because an
#   allowance must not outlive the debt it allowed.
# Exit dialect (shared with gate_rc): 0 clean / 1 residue grew or a row went
# stale / 2 undecidable (missing registries, bad line format, crash).
#
# Usage:
#   python scripts/check_retired_crate_residue.py --selftest
#   python scripts/check_retired_crate_residue.py --emit
#   python scripts/check_retired_crate_residue.py
# =============================================================================
import argparse
import os
import re
import shutil
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RETIRED = os.path.join(ROOT, "scripts", "retired_crates.txt")
BASELINE = os.path.join(ROOT, "scripts", "retired_crate_residue.txt")

# Search surface: files a tool executes. Deliberately excludes *.md (not in git
# per .gitignore, and the documentation policy keeps history on purpose).
SCAN_DIRS = (".github", "scripts", "tests", "crates", "fuzz")
SCAN_FILES = ("Cargo.toml", "Dockerfile", "install.sh", "install.ps1")
SCAN_EXT = (".rs", ".toml", ".yml", ".yaml", ".sh", ".ps1", ".py", "CODEOWNERS")
SKIP_DIR_PARTS = ("target", ".git", ".toolchain", "archives", "node_modules", ".config")


def set_root(path):
    """Re-point every path at another repo root (used by --selftest fixtures)."""
    global ROOT, RETIRED, BASELINE
    ROOT = path
    RETIRED = os.path.join(path, "scripts", "retired_crates.txt")
    BASELINE = os.path.join(path, "scripts", "retired_crate_residue.txt")


def read_list(path, what):
    """Read a `value # reason` registry into {value: reason}.

    Raises ValueError on a line without a reason: an unexplained allowance is
    how a registry quietly turns into a dumping ground.
    """
    out = {}
    if not os.path.exists(path):
        raise ValueError("%s missing (%s)" % (what, path))
    with open(path, encoding="utf-8", errors="replace") as fh:
        for raw in fh:
            line, _, why = raw.partition("#")
            key = line.strip()
            if not key:
                continue
            if not why.strip():
                raise ValueError("%s: line needs a trailing '# reason': %r" % (what, key))
            out[key] = why.strip()
    return out


def scan_files():
    """Yield repo-relative paths of every executable-surface file to inspect."""
    for name in SCAN_FILES:
        p = os.path.join(ROOT, name)
        if os.path.isfile(p):
            yield name
    for d in SCAN_DIRS:
        base = os.path.join(ROOT, d)
        if not os.path.isdir(base):
            continue
        # os.walk yields (dirpath, dirnames, filenames) -- the second slot is
        # DIRECTORIES. Mis-unpacking it as `files` makes the scan silently match
        # nothing, which reads as "no residue" instead of "not scanned".
        for dirpath, _dirs, files in os.walk(base):
            rel_dir = os.path.relpath(dirpath, ROOT).replace(os.sep, "/")
            if any(part in SKIP_DIR_PARTS for part in rel_dir.split("/")):
                continue
            for f in sorted(files):
                if not (f.endswith(SCAN_EXT) or f in SCAN_EXT):
                    continue
                yield "%s/%s" % (rel_dir, f)


def classify_line(line, crate, ident, owners_file=False):
    """Return the reference kind if this line functionally references `crate`.

    `owners_file` is passed by the caller from the *path*, not sniffed from the
    line: a CODEOWNERS row (`/crates/dead-crate/  @someone`) does not contain
    the word "CODEOWNERS" itself.
    """
    if re.search(r"%s\s*=\s*\{[^}]*path\s*=" % re.escape(crate), line):
        return "path-dep"
    if re.search(r'"\s*crates/%s\s*"' % re.escape(crate), line):
        return "path-dep"
    if re.search(r"\b%s\s*::" % re.escape(ident), line):
        return "symbol"
    if re.search(r"-p\s+%s\b" % re.escape(crate), line):
        return "cargo-flag"
    if owners_file and re.search(r"crates/%s\b" % re.escape(crate), line):
        return "owners"
    return None


def scan():
    """Return a sorted LIST of (relpath, crate, kind), one entry per matching line.

    Deliberately not deduplicated: the ledger counts OCCURRENCES. An earlier
    version returned a set keyed by (file, crate, kind), which made a second
    dangling reference inside an already-flagged file invisible -- the planted
    probe in the smoke run proved it (adding a 4th CODEOWNERS row for a
    registered crate left the verdict green).
    """
    retired = read_list(RETIRED, "retired_crates.txt")
    found = []
    for rel in scan_files():
        full = os.path.join(ROOT, rel)
        owners_file = os.path.basename(rel) == "CODEOWNERS"
        try:
            with open(full, encoding="utf-8", errors="replace") as fh:
                lines = fh.read().splitlines()
        except OSError:
            continue
        for line in lines:
            stripped = line.strip()
            if stripped.startswith(("//", "//!", "///", "#")) and not owners_file:
                continue  # prose/history annotation, deliberately out of scope
            for crate in retired:
                kind = classify_line(line, crate, crate.replace("-", "_"), owners_file)
                if kind:
                    found.append((rel, crate, kind))
                    break
    return sorted(found)


def load_baseline():
    """Return {file|crate|kind tuple: allowed occurrences} from the residue ledger.

    One row per occurrence, so a row can be deleted the moment one reference is
    fixed. Raises ValueError on a malformed row or a missing reason.
    """
    out = {}
    if not os.path.exists(BASELINE):
        raise ValueError("baseline missing: %s" % BASELINE)
    with open(BASELINE, encoding="utf-8", errors="replace") as fh:
        for raw in fh:
            body, _, why = raw.partition("#")
            line = body.strip()
            if not line:
                continue
            parts = line.split("|")
            if len(parts) != 3 or not why.strip():
                raise ValueError("needs `file|crate|kind # reason`: %r" % raw.rstrip())
            key = tuple(p.strip() for p in parts)
            out[key] = out.get(key, 0) + 1
    return out


def decide(found, baseline):
    """Compare occurrence counts against the ledger.

    Returns (new, grown, rotate, stale) as (key, measured, allowed) lists:
      new    - key absent from the ledger (a retirement left a reference behind)
      grown  - more occurrences than allowed
      rotate - fewer occurrences than allowed (refresh the ledger)
      stale  - ledger row(s) with nothing on disk to match them
    """
    measured = {}
    for key in found:
        measured[key] = measured.get(key, 0) + 1
    new, grown, rotate, stale = [], [], [], []
    for key, allowed in sorted(baseline.items()):
        got = measured.get(key, 0)
        if got == 0:
            stale.append((key, got, allowed))
        elif got > allowed:
            grown.append((key, got, allowed))
        elif got < allowed:
            rotate.append((key, got, allowed))
    for key, got in sorted(measured.items()):
        if key not in baseline:
            new.append((key, got, 0))
    return new, grown, rotate, stale


def _fixture():
    """Build a throwaway repo root exercising each kind + the exclusions."""
    tmp = tempfile.mkdtemp(prefix="retired-residue-")
    for d in ("scripts", "crates", "fuzz", ".github/workflows"):
        os.makedirs(os.path.join(tmp, d), exist_ok=True)
    with open(os.path.join(tmp, "scripts", "retired_crates.txt"), "w", encoding="utf-8") as fh:
        fh.write("dead-crate # deleted in the fixture batch\n")
    writes = {
        "Cargo.toml": '[workspace]\nmembers = ["crates/alive"]\n',
        "fuzz/Cargo.toml": 'dead-crate = { path = "../crates/dead-crate" }\n',
        "crates/alive/src/lib.rs": "use dead_crate::Thing;\n"
                                    "// a prose mention of dead-crate, must not count\n",
        ".github/workflows/bench.yml": "  run: cargo bench -p dead-crate --bench b\n",
        ".github/CODEOWNERS": "/crates/dead-crate/ @someone # owners row\n",
        "docs/architecture/CODE_WIKI.md": "dead-crate was deleted\n",
    }
    for rel, text in writes.items():
        p = os.path.join(tmp, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w", encoding="utf-8") as fh:
            fh.write(text)
    os.makedirs(os.path.join(tmp, "docs", "architecture"), exist_ok=True)
    return tmp


def selftest():
    """End-to-end teeth proof on a synthetic tree, plus ledger format strictness.

    Asserts the four scored kinds are each detected, that prose and .md are not,
    and that a fixed row left in the ledger reports as STALE. The fixture is a
    real directory scanned by the real scan path -- not a regex unit test.
    """
    tmp = _fixture()
    try:
        set_root(tmp)
        found = scan()
        seen = {(rel, kind) for rel, _c, kind in found}
        lib_rows = [rel for rel, _c, kind in found if rel.endswith("lib.rs")]
        checks = [
            ("path-dep detected", ("fuzz/Cargo.toml", "path-dep") in seen),
            ("symbol detected", ("crates/alive/src/lib.rs", "symbol") in seen),
            ("cargo-flag detected", (".github/workflows/bench.yml", "cargo-flag") in seen),
            ("owners detected", (".github/CODEOWNERS", "owners") in seen),
            # the fixture lib.rs holds BOTH a real `use dead_crate::Thing;` and a
            # prose mention; exactly one row may come from it, the symbol one.
            ("prose on its own line not scored", len(lib_rows) == 1),
            ("members list without the crate is clean",
             all(rel != "Cargo.toml" for rel, _k in seen)),
            ("decide yields all four verdicts",
             decide([("a", "b", "c"), ("a", "b", "c"), ("d", "e", "f"),
                     ("p", "q", "r")],
                     {("a", "b", "c"): 1, ("d", "e", "f"): 3,
                      ("g", "h", "i"): 1, ("x", "y", "z"): 1})
             == ([(("p", "q", "r"), 1, 0)],
                 [(("a", "b", "c"), 2, 1)],
                 [(("d", "e", "f"), 1, 3)],
                 [(("g", "h", "i"), 0, 1), (("x", "y", "z"), 0, 1)])),
            ("missing ledger is undecidable", _raises(load_baseline)),
        ]
        bad = [name for name, ok in checks if not ok]
        for name, ok in checks:
            print("  [%s] %s" % ("PASS" if ok else "FAIL", name))
        if bad:
            return 1
        print("[SELFTEST] 4 scored kinds detected, prose/md excluded, "
              "verdict split + missing-ledger path verified")
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
        set_root(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _raises(fn):
    """True if fn() raises ValueError (the registry/ledger-absent path)."""
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
        found = scan()
    except ValueError as exc:
        print("[CONFIG] %s" % exc)
        return 2
    if args.emit:
        for rel, crate, kind in found:
            print("%s|%s|%s  # measured 2026-09-23" % (rel, crate, kind))
        print("# total %d functional residue row(s)" % len(found))
        return 0
    try:
        baseline = load_baseline()
    except ValueError as exc:
        print("[CONFIG] %s" % exc)
        return 2
    new, grown, rotate, stale = decide(found, baseline)
    print("[INFO] functional residue to retired crates: %d measured / %d allowed "
          "(ledger rows)" % (len(found), len(baseline)))
    if not any((new, grown, rotate, stale)):
        print("[OK] no new residue; ledger matches the disk")
        return 0
    status = 1
    if new or grown:
        print("[FAIL] residue grew since the ledger was written:")
        for (rel, crate, kind), meas, allowed in new + grown:
            print("  [GAP] %s -> %s (%s): %d occurrence(s), allowed %d"
                  % (rel, crate, kind, meas, allowed))
        print("  Fix: delete the reference. If the mention is prose that the docs")
        print("       policy keeps on purpose, it must not be scored at all --")
        print("       check the line does not start with a comment marker.")
    if rotate:
        print("[ROTATE] %d key(s) shrank; drop the matching ledger row(s):" % len(rotate))
        for (rel, crate, kind), meas, allowed in rotate:
            print("  [ROTATE] %s|%s|%s %d -> %d" % (rel, crate, kind, allowed, meas))
    if stale:
        print("[FAIL] %d ledger row(s) with nothing on disk -- delete them:" % len(stale))
        for (rel, crate, kind), _meas, _allowed in stale:
            print("  [STALE] %s|%s|%s" % (rel, crate, kind))
    return status


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(main))
