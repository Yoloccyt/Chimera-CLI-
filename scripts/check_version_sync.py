#!/usr/bin/env python3
# -*- coding: ascii -*-
"""Version single-source-of-truth sync gate (G-158, W3 toolchain batch).

Why this gate exists
--------------------
`workspace.package.version` is the declared single authority for the crate
version, but three derived surfaces drift independently in practice
(measured 2026-11-14, all on this branch):
  * Dockerfile `ARG VERSION=...` had a live-gate blind spot: the only pin was
    audit_phaseR_spec.py's stale 2.28.0 prefix, masked by G-10 expect=1.
  * The `--version` acceptance regex exists in FIVE hand-written copies
    (release.yml docker job, test-install-scripts.yml, install.sh,
    install.ps1, test_version_verification.ps1) and had already diverged:
    test-install-scripts.yml was missing the `chimela` compat alias while its
    comment claimed parity with release.yml.
The existing doc_consistency version checks target *.md files that are
gitignored, so on CI they degrade to warn -- this gate reads only files that
are actually tracked, so it is CI-effective.

What it judges
--------------
  * Dockerfile ARG VERSION (default value) == Cargo.toml workspace version
  * every tracked --version regex copy carries the SAME alternation set
    (order-independent; the set must be non-empty)
  * surfaces missing entirely -> exit 2 (cannot judge what does not exist)

Modes / exit codes
------------------
  (default)  judge the real repo     0 = all surfaces in sync
  --selftest fixture tree proves both directions
  usage / missing surface            2

Output is pure ASCII (project script convention).
"""

import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

CARGO_TOML = os.path.join(ROOT, "Cargo.toml")
DOCKERFILE = os.path.join(ROOT, "Dockerfile")
# Every tracked copy of the --version acceptance regex. test-install-scripts.yml
# is the historical divergence site (missing chimela while claiming parity).
REGEX_SITES = [
    ".github/workflows/release.yml",
    ".github/workflows/test-install-scripts.yml",
    "install.sh",
    "install.ps1",
    "test_version_verification.ps1",
]

WS_VERSION_RE = re.compile(r"\[workspace\.package\]([^\[]*)", re.S)
VERSION_LINE_RE = re.compile(r'(?m)^version\s*=\s*"([^"]+)"')
DOCKER_ARG_RE = re.compile(r"ARG\s+VERSION\s*=?\s*(\S+)")
# Alternation at the start of an anchored version regex. Two dialects coexist
# by design: grep -E uses [0-9], PowerShell uses \d -- same semantics, both are
# accepted and compared as the SAME alternation set.
ALTERNATION_RE = re.compile(r"\^\(([\w-]+(?:\|[\w-]+)*)\)\s+(?:\[0-9\]|\\d)")


def workspace_version(cargo_text):
    sec = WS_VERSION_RE.search(cargo_text)
    if not sec:
        return None
    m = VERSION_LINE_RE.search(sec.group(1))
    return m.group(1) if m else None


def dockerfile_version(docker_text):
    m = DOCKER_ARG_RE.search(docker_text)
    return m.group(1) if m else None


def regex_alternation(text):
    """The --version regex alternation set in one file (frozenset of names)."""
    found = set()
    for m in ALTERNATION_RE.finditer(text):
        found.add(frozenset(m.group(1).split("|")))
    return found


def evaluate(cargo_text, docker_text, site_texts):
    """Pure verdict -> (rc, out_lines)."""
    out = []
    fails = 0
    ws = workspace_version(cargo_text)
    if not ws:
        out.append("[TOOL-ERROR] workspace.package.version not parseable")
        return 2, out
    dk = dockerfile_version(docker_text)
    if not dk:
        out.append("[TOOL-ERROR] Dockerfile ARG VERSION not found")
        return 2, out
    if dk != ws:
        out.append("[FAIL] Dockerfile ARG VERSION=%s != workspace=%s" % (dk, ws))
        fails += 1
    else:
        out.append("[OK] Dockerfile ARG VERSION == workspace (%s)" % ws)

    sets = {}
    for name, text in site_texts.items():
        alts = regex_alternation(text)
        if not alts:
            out.append("[TOOL-ERROR] no --version regex found in %s" % name)
            return 2, out
        if len(alts) > 1:
            out.append("[FAIL] %s carries %d DISTINCT --version regexes: %s"
                       % (name, len(alts), sorted("|".join(sorted(a)) for a in alts)))
            fails += 1
            continue
        sets[name] = next(iter(alts))
    ref_name = REGEX_SITES[0]
    ref = sets.get(ref_name)
    for name, s in sorted(sets.items()):
        if s != ref:
            out.append("[FAIL] regex alternation drift: %s = {%s} != %s = {%s}"
                       % (name, ",".join(sorted(s)), ref_name, ",".join(sorted(ref))))
            fails += 1
        else:
            out.append("[OK] %s alternation = {%s}" % (name, ",".join(sorted(s))))
    if fails:
        out.append("version sync BROKEN: %d violation(s)" % fails)
        return 1, out
    out.append("[OK] version single-source-of-truth holds across %d surfaces"
               % (2 + len(sets)))
    return 0, out


def run_real():
    texts = {}
    try:
        with open(CARGO_TOML, "r", encoding="utf-8") as fh:
            cargo_text = fh.read()
    except OSError as exc:
        print("[TOOL-ERROR] cannot read Cargo.toml: %s" % exc)
        return 2
    try:
        with open(DOCKERFILE, "r", encoding="utf-8") as fh:
            docker_text = fh.read()
    except OSError as exc:
        print("[TOOL-ERROR] cannot read Dockerfile: %s" % exc)
        return 2
    for site in REGEX_SITES:
        path = os.path.join(ROOT, site)
        if not os.path.isfile(path):
            print("[TOOL-ERROR] regex site missing: %s" % site)
            return 2
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            texts[site] = fh.read()
    rc, out = evaluate(cargo_text, docker_text, texts)
    for line in out:
        print(line)
    return rc


# --- selftest ---------------------------------------------------------------

GOOD_CARGO = '[workspace]\nmembers = []\n[workspace.package]\nversion = "2.29.0-omega"\n'
GOOD_DOCKER = "FROM x\nARG VERSION=2.29.0-omega\n"
GOOD_RE = 'grep -Eq \'^(aether|chimera|chimela) [0-9]+\\.[0-9]+\\.[0-9]+\'\n'
BAD_RE = 'grep -Eq \'^(aether|chimera) [0-9]+\\.[0-9]+\\.[0-9]+\'\n'


def selftest():
    ok = True

    def add(label, cond):
        nonlocal ok
        if cond:
            print("[SELFTEST] %s" % label)
        else:
            ok = False
            print("[SELFTEST-FAIL] %s" % label)

    sites = {name: GOOD_RE for name in REGEX_SITES}
    rc, _out = evaluate(GOOD_CARGO, GOOD_DOCKER, dict(sites))
    add("all surfaces in sync -> rc 0", rc == 0)

    rc, out = evaluate(GOOD_CARGO, "ARG VERSION=2.28.0-omega\n", dict(sites))
    add("Dockerfile version drift -> rc 1", rc == 1
        and any("ARG VERSION" in l for l in out))

    drifted = dict(sites)
    drifted[".github/workflows/test-install-scripts.yml"] = BAD_RE
    rc, out = evaluate(GOOD_CARGO, GOOD_DOCKER, drifted)
    add("regex alternation drift (missing alias) -> rc 1", rc == 1
        and any("alternation drift" in l for l in out))

    rc, _out = evaluate(GOOD_CARGO, GOOD_DOCKER, {"install.sh": "nothing here\n"})
    add("regex site with no pattern -> rc 2 (undecidable)", rc == 2)

    add("ALTERNATION_RE extracts the chimela alias",
        regex_alternation(GOOD_RE) == {frozenset({"aether", "chimera", "chimela"})})
    add("workspace_version parses the package section",
        workspace_version(GOOD_CARGO) == "2.29.0-omega")
    print("[SELFTEST] %s" % ("ALL ASSERTIONS HELD" if ok else "FAILED"))
    return 0 if ok else 1


def main(argv):
    args = [a for a in argv[1:]]
    if any(a not in ("--selftest",) for a in args):
        sys.stderr.write("usage: check_version_sync.py [--selftest]\n")
        return 2
    if "--selftest" in args:
        return selftest()
    return run_real()


if __name__ == "__main__":
    sys.path.insert(0, HERE)
    import gate_rc  # noqa: E402  (crash -> exit 2, never a borrowed traceback exit 1)

    sys.exit(gate_rc.run(lambda: main(sys.argv)))
