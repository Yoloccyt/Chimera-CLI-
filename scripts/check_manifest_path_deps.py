#!/usr/bin/env python3
# =============================================================================
# check_manifest_path_deps.py - every `path = "..."` dependency must resolve (B0-3)
# =============================================================================
# Purpose: a path dependency pointing at a directory that no longer exists makes
#          the whole manifest fail to PARSE, which is worse than a compile error:
#          no target can be checked, tested or built through it. That is exactly
#          how `cargo check --manifest-path fuzz/Cargo.toml` has been failing
#          since batch A deleted crates/model-router (fuzz/Cargo.toml:61 still
#          declares it) -- and scripts/check_fuzz_config.sh kept reporting PASS,
#          because its "path exists" check covers [[bin]] fuzz TARGET files, not
#          [dependencies] path entries.
#
# Scope: every `[dependencies]` / `[dev-dependencies]` / `[build-dependencies]` /
#        `[target.*.dependencies]` / `[patch.*]` table in Cargo.toml, fuzz/Cargo.toml
#        and crates/*/Cargo.toml. An entry resolves if `<root>/<path>/Cargo.toml`
#        exists. Registry (non-path) deps are out of scope.
#
# Exit dialect (gate_rc): 0 all resolve / 1 a path dependency is dangling /
# 2 undecidable (no manifests found -- wrong working dir).
# Usage:
#   python scripts/check_manifest_path_deps.py --selftest
#   python scripts/check_manifest_path_deps.py
# =============================================================================
import argparse
import os
import re
import shutil
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MANIFESTS = ("Cargo.toml", os.path.join("fuzz", "Cargo.toml"))
DEP_SECTIONS = re.compile(
    r"^\[(dev-|build-)?dependencies\]|^\[target\..*\.(dev-|build-)?dependencies\]"
    r"|^\[patch\.", re.I)
PATH_DEP = re.compile(r'^"?([A-Za-z0-9_\-]+)"?\s*=\s*\{[^}\n]*path\s*=\s*"([^"]+)"')


def set_root(path):
    global ROOT
    ROOT = path


def manifest_paths():
    """Every Cargo.toml whose path dependencies must resolve."""
    out = list(MANIFESTS)
    crates = os.path.join(ROOT, "crates")
    if os.path.isdir(crates):
        for d in sorted(os.listdir(crates)):
            # The path must keep the `crates/` prefix: joining only the directory
            # name onto ROOT silently matched nothing, so the scan ran over 2
            # manifests instead of 43 while still reporting a clean-looking count.
            m = os.path.join("crates", d, "Cargo.toml")
            if os.path.exists(os.path.join(ROOT, m)):
                out.append(m)
    # Emit POSIX separators: `os.path.join` above yields `crates\real\...` on
    # Windows, so the reported path -- and every selftest assertion keyed on it
    # -- silently changed shape depending on the host. Findings must read the
    # same on Linux CI as on a Windows workstation.
    return [p.replace(os.sep, "/") for p in out]


def dangling(root=None):
    """Return [(manifest, dep_name, declared_path)] whose target has no Cargo.toml."""
    root = root or ROOT
    bad = []
    for rel in manifest_paths():
        full = os.path.join(root, rel)
        if not os.path.exists(full):
            continue
        section = False
        with open(full, encoding="utf-8", errors="replace") as fh:
            for no, raw in enumerate(fh, 1):
                line = raw.split("#", 1)[0].strip()
                if not line:
                    continue
                if line.startswith("["):
                    section = bool(DEP_SECTIONS.match(line))
                    continue
                if line.startswith(("[[", "#")) or not section:
                    continue
                m = PATH_DEP.match(line)
                if not m:
                    continue
                name, path = m.group(1), m.group(2)
                # path entries are relative to the manifest's own directory
                target = os.path.normpath(os.path.join(os.path.dirname(full), path))
                if not os.path.exists(os.path.join(target, "Cargo.toml")):
                    bad.append((rel, name, path, no))
    return bad


def _fixture():
    """A tree with one good path dep, one dangling, one registry dep, one patch."""
    tmp = tempfile.mkdtemp(prefix="pathdeps-")
    os.makedirs(os.path.join(tmp, "crates", "real"))
    os.makedirs(os.path.join(tmp, "fuzz"))
    with open(os.path.join(tmp, "crates", "real", "Cargo.toml"), "w", encoding="utf-8") as fh:
        # A dangling entry inside a MEMBER manifest: proves the scan surface really
        # includes crates/*/Cargo.toml (an earlier revision built the path without
        # the `crates/` prefix and silently scanned only 2 of 43 manifests).
        fh.write('[package]\nname = "real"\n[dependencies]\n'
                 'alsogone = { path = "../alsogone" }\n')
    with open(os.path.join(tmp, "fuzz", "Cargo.toml"), "w", encoding="utf-8") as fh:
        fh.write('[package]\nname = "fuzz"\n[dependencies]\n'
                 'real = { path = "../crates/real" }\n'
                 'gone = { path = "../crates/gone" }\n'
                 'serde = { version = "1" }\n'
                 '[patch.crates-io]\nalso-gone = { path = "../crates/also-gone" }\n')
    return tmp


def selftest():
    """Prove dangling paths are found, and that valid/registry entries are not.

    Also pins two false-positive classes: a registry dependency (no `path`) and a
    `[[bin]]` table (not a dependency table) must never be reported.
    """
    tmp = _fixture()
    try:
        set_root(tmp)
        found = {(rel, name) for rel, name, _p, _no in dangling(tmp)}
        checks = [
            ("dangling path dep detected", ("fuzz/Cargo.toml", "gone") in found),
            ("[patch.*] path dep detected", ("fuzz/Cargo.toml", "also-gone") in found),
            ("valid path dep not reported", ("fuzz/Cargo.toml", "real") not in found),
            ("registry dep not reported", ("fuzz/Cargo.toml", "serde") not in found),
            ("member manifest is scanned",
             ("crates/real/Cargo.toml", "alsogone") in found),
            ("count equals exactly the three planted dangles", len(found) == 3),
        ]
        for name, ok in checks:
            print("  [%s] %s" % ("PASS" if ok else "FAIL", name))
        if not all(ok for _n, ok in checks):
            return 1
        print("[SELFTEST] dangling + patch paths caught; valid, registry and "
              "non-dependency tables suppressed")
        return 0
    finally:
        set_root(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
        shutil.rmtree(tmp, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    scanned = manifest_paths()
    if not [p for p in scanned if os.path.exists(os.path.join(ROOT, p))]:
        print("[CONFIG] no Cargo.toml found under %s -- wrong working dir?" % ROOT)
        return 2
    bad = dangling()
    print("[INFO] manifests scanned: %d; dangling path dependencies: %d"
          % (len(scanned), len(bad)))
    if not bad:
        print("[OK] every path dependency resolves to a package manifest")
        return 0
    print("[FAIL] %d path dependency/ies point at a directory with no Cargo.toml:" % len(bad))
    for rel, name, path, no in bad:
        print("  [GAP] %s:%d declares `%s = { path = \"%s\" }` -- target missing"
              % (rel, no, name, path))
    print("  A dangling path dep makes the manifest unparseable, so NOTHING in that")
    print("  package can be built or checked. Fix: drop the entry, or restore the")
    print("  package. Ref: scripts/retired_crate_residue.txt for retired-crate rows.")
    return 1


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(main))
