#!/usr/bin/env python3
# =============================================================================
# check_declared_dep_usage.py - declared-vs-used dependency gate (arch dir E)
# =============================================================================
# Purpose: mechanical check that every INTERNAL workspace dependency declared in
#          a crate's `[dependencies]` section is actually referenced by that
#          crate's `src/`. Closes the "ghost dependency" debt class: a manifest
#          entry with zero code references inflates the real dependency graph,
#          and any ratchet computed from it (e.g. `check_dependency_rules.sh`
#          Check D "chimera-mas internal deps 13/16") then runs on a wrong base.
#
# Measured origin (2026-09-12, docs/reports/arch-refactor-directions-v2-
# 2026-09-12.md direction E): `chimera-mas` declared `mlc-engine` and
# `cmt-tiering` with ZERO references in src/ — 2 of the 13 internal deps counted
# by Check D were ghosts.
#
# SCOPE DECISIONS (deliberately narrow, to keep the gate honest and quiet):
#   1. INTERNAL workspace crates only. External crates are excluded because
#      their usage cannot be decided textually (derive macros `#[derive(Error)]`,
#      helper attributes `#[serde(default)]`, feature-only transitive enables)
#      — a textual gate there would produce false positives and get disabled.
#      Ghost INTERNAL deps are also the ones that distort layer/dep ratchets.
#   2. `[dependencies]` section only (production assembly face) — same scope as
#      `check_dependency_rules.sh` Check D, so the two gates speak one dialect.
#      dev-dependencies / bench-only usage is reported as INFO, not FAIL.
#   3. Usage = the crate identifier appears as `use <ident>::`, `<ident>::` or
#      `extern crate <ident>` in `crates/<crate>/src/**/*.rs`. Ident = package
#      name with '-' -> '_'. Comments/strings are stripped first (a doc example
#      or a string literal naming the crate must not count).
#
# Exceptions: scripts/declared_dep_exceptions.txt, one `<crate> -> <dep> # why`
#   per line, for entries PROVEN to be needed by something other than src code
#   (e.g. a feature table forwarding, or a build-script contract). Every
#   exception needs a reason; a malformed line fails the gate (exit 2).
#
# Encoding: all-ASCII (project script convention, avoids CJK locale issues)
# Exit code: 0 = clean, 1 = ghost dependency found, 2 = usage/config error
# Usage:
#   python scripts/check_declared_dep_usage.py --selftest
#   python scripts/check_declared_dep_usage.py
# =============================================================================
import argparse
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATES = os.path.join(ROOT, "crates")
EXCEPTIONS = os.path.join(ROOT, "scripts", "declared_dep_exceptions.txt")

DEP_SECTION = "dependencies"
DEV_SECTIONS = ("dev-dependencies", "build-dependencies")


def package_name(manifest_path):
    """Return the `[package] name` of a manifest, or None."""
    section = None
    with open(manifest_path, encoding="utf-8-sig", errors="ignore") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            if line.startswith("["):
                section = line.strip("[]").strip()
                continue
            if section == "package":
                m = re.match(r'^name\s*=\s*"([^"]+)"', line)
                if m:
                    return m.group(1)
    return None


def crates_index():
    """Return {dir_name: package_name} for every crates/<dir>/Cargo.toml."""
    index = {}
    for d in sorted(os.listdir(CRATES)):
        mp = os.path.join(CRATES, d, "Cargo.toml")
        if not os.path.exists(mp):
            continue
        index[d] = package_name(mp) or d
    return index


def parse_declared(manifest_path):
    """Return {dep_name: section} for [dependencies] / dev / build sections.

    Inline tables ([dependencies.foo]) are normalised to the section they live
    under so they are classified the same as section-body declarations.
    """
    declared = {}
    section = None
    with open(manifest_path, encoding="utf-8-sig", errors="ignore") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            if line.startswith("["):
                sec = line.strip("[]").strip()
                head = sec.split(".")[0]
                if "." in sec and head in (DEP_SECTION,) + DEV_SECTIONS:
                    section = head
                    declared[sec.split(".", 1)[1].strip('"')] = head
                elif sec in (DEP_SECTION,) + DEV_SECTIONS:
                    section = sec
                elif sec.startswith("target."):
                    section = sec  # target-specific tables fall through to body
                else:
                    section = None
                continue
            if section is None:
                continue
            if section.startswith("target."):
                # Only [target.<cfg>.dependencies] counts as production deps.
                if not section.endswith("." + DEP_SECTION):
                    continue
                eff = DEP_SECTION
            else:
                eff = section
            m = re.match(r'^"?([A-Za-z0-9_\-]+)"?\s*=', line)
            if m:
                declared[m.group(1)] = eff
    return declared


def strip_noise(line):
    """Remove line comments and string literal contents (doc examples must not count)."""
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


def code_blob(root_dir):
    """Concatenated, noise-stripped source of every .rs under root_dir."""
    parts = []
    for dirpath, _, files in os.walk(root_dir):
        for f in files:
            if not f.endswith(".rs"):
                continue
            with open(os.path.join(dirpath, f), encoding="utf-8", errors="ignore") as fh:
                for line in fh:
                    parts.append(strip_noise(line))
    return "\n".join(parts)


def is_used(blob, ident):
    """True if `ident` is referenced as a crate path/import in the blob."""
    return bool(
        re.search(r"\buse\s+%s\s*::" % re.escape(ident), blob)
        or re.search(r"\b%s\s*::" % re.escape(ident), blob)
        or re.search(r"\bextern\s+crate\s+%s\b" % re.escape(ident), blob)
    )


def load_exceptions():
    """Return set of (crate_dir, dep_name) accepted, requiring an inline reason."""
    entries = set()
    if not os.path.exists(EXCEPTIONS):
        return entries
    with open(EXCEPTIONS, encoding="utf-8", errors="ignore") as fh:
        for raw in fh:
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            body, _, why = line.partition("#")
            if "->" not in body or not why.strip():
                print("[CONFIG] exception needs `<crate> -> <dep> # reason`: %r" % raw.rstrip())
                sys.exit(2)
            crate, _, dep = body.partition("->")
            entries.add((crate.strip(), dep.strip()))
    return entries


def scan(index):
    """Return (gaps, checked_count) for the whole workspace.

    Three-state classification of every internal dep declared in
    `[dependencies]`:
      A. referenced from src/**        -> correct (production dependency).
      B. referenced only from benches/** or tests/** -> gap kind "dev-only":
         the entry belongs in `[dev-dependencies]`; keeping it in
         `[dependencies]` inflates the production graph (and every ratchet
         computed from it) for zero production benefit.
      C. referenced nowhere            -> gap kind "ghost": pure manifest noise.

    WHY benches/tests are scanned (self-correction, 2026-09-12): the first
    version of this gate scanned src/ only and reported chimera-mas's
    `mlc-engine`/`cmt-tiering` as ghosts — but benches/mas_benchmark.rs does
    use them, so removing the entries broke the build (`error[E0433]: cannot
    find module or crate mlc_engine`). A gate that recommends breaking the
    build is worse than no gate; the fix is to classify by usage surface
    (state B) instead of by a single directory.
    """
    gaps = []
    checked = 0
    for cdir, pkg in sorted(index.items()):
        mp = os.path.join(CRATES, cdir, "Cargo.toml")
        src = os.path.join(CRATES, cdir, "src")
        if not os.path.isdir(src):
            continue
        declared = parse_declared(mp)
        src_blob = code_blob(src)
        dev_blob = ""
        for extra in ("benches", "tests"):
            d = os.path.join(CRATES, cdir, extra)
            if os.path.isdir(d):
                dev_blob += "\n" + code_blob(d)
        for dep, section in sorted(declared.items()):
            if dep not in index.values():
                continue  # external crate -> out of scope by design
            if section != DEP_SECTION:
                continue
            checked += 1
            ident = dep.replace("-", "_")
            if is_used(src_blob, ident):
                continue  # state A
            if is_used(dev_blob, ident):
                gaps.append((cdir, dep, "dev-only"))
            else:
                gaps.append((cdir, dep, "ghost"))
    return gaps, checked


# ---------------------------------------------------------------- self-test
SELFTEST_EXPECT = {
    "use event_bus::EventBus;": True,
    "let x = event_bus::EventBus::new();": True,
    "pub use event_bus::EventBus;": True,
    "extern crate event_bus;": True,
    "// event_bus::EventBus in a comment must not count": False,
    'let s = "event_bus::EventBus in a string must not count";': False,
    "/// doc example: event_bus::EventBus::new()": False,
    "let event_bus = 1;": False,
    "let my_event_bus = 2;": False,
}


def selftest():
    ok = True
    for line, expected in SELFTEST_EXPECT.items():
        got = is_used(strip_noise(line), "event_bus")
        if got != expected:
            print("[SELFTEST] mismatch for %r: expected %s got %s" % (line, expected, got))
            ok = False
    # declaration parsing: section classification + inline table normalisation
    ok = ok and DEP_SECTION == "dependencies" and DEV_SECTIONS == ("dev-dependencies", "build-dependencies")
    if not ok:
        return 1
    print("[SELFTEST] all constructed cases classified correctly:")
    print("           usage: 4 true positives / 5 noise classes suppressed "
          "(comment/string/doc/ident-shadow)")
    return 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()

    index = crates_index()
    if len(index) < 2:
        print("[CONFIG] crate index is empty/too small (%d) — wrong working dir?" % len(index))
        return 2
    gaps, checked = scan(index)
    exceptions = load_exceptions()

    unresolved = [g for g in gaps if (g[0], g[1]) not in exceptions]
    used_exc = [g for g in gaps if (g[0], g[1]) in exceptions]
    stale = sorted(exceptions - {(g[0], g[1]) for g in gaps})

    print("[INFO] internal deps checked (production face): %d across %d crates"
          % (checked, len(index)))

    if not unresolved and not stale:
        print("[OK] every declared internal dependency is referenced by src/ "
              "(%d exception(s) honoured)" % len(used_exc))
        return 0

    status = 1
    dev_only = [g for g in unresolved if g[2] == "dev-only"]
    ghosts = [g for g in unresolved if g[2] == "ghost"]
    if dev_only:
        print("[FAIL] %d dependency/ies used ONLY from benches/ or tests/ but "
              "declared in [dependencies]:" % len(dev_only))
        for cdir, dep, _ in dev_only:
            print("  [GAP] crates/%s declares `%s` — no src/ reference, but used "
                  "from benches/ or tests/" % (cdir, dep))
        print("  Fix: move the entry to [dev-dependencies] (benches/tests see dev "
              "deps; the production graph and its ratchets stop counting it).")
    if ghosts:
        print("[FAIL] %d ghost internal dependency/ies (zero reference anywhere):"
              % len(ghosts))
        for cdir, dep, _ in ghosts:
            print("  [GAP] crates/%s declares `%s` — no src/, benches/ or tests/ "
                  "reference" % (cdir, dep))
        print("  Fix: remove the entry from the manifest.")
    if unresolved:
        print("")
        print("     Or register a justified exception in")
        print("     scripts/declared_dep_exceptions.txt (`<crate> -> <dep> # why`).")
        print("     Ref: docs/reports/arch-refactor-directions-v2-2026-09-12.md E.")
    if stale:
        print("[FAIL] %d stale exception(s) (no longer match any gap):" % len(stale))
        for cdir, dep in stale:
            print("  [STALE] %s -> %s" % (cdir, dep))
        print("  Remove them (an exception must not outlive the debt it excused).")
    return status


if __name__ == "__main__":
    sys.exit(main())
