#!/usr/bin/env python3
"""G-deleted-crate: CODE_WIKI 3.12 registry vs git-deleted crates (bidirectional).

Why: the registry calls itself the "only place that lists physically deleted
crates" and instructs every other doc to reference it instead of restating the
list. On 2026-10-14 the section still claimed "exactly 3" while its OWN stated
criterion (git log --diff-filter=D on crates/*/Cargo.toml) measured 5 --
lsct-tiering and router-traits were deleted 2026-09-28, six days after the
section was written, and trigger T1/T6 never re-fed it. A hub doc that is
silently incomplete propagates the gap to every doc that obeys it.

Model (aligned with check_doc_drift.sh / audit_cmd_sync.py, ADR-167 decision 4):
  * CODE + git history is the authority.
  * Unregistered deletion            -> FAIL (a doc hub lies by omission)
  * Registered name not actually deleted -> FAIL (registry rot)
  * Section/table missing or unparseable -> exit 2 "cannot decide", never a
    silent pass.
  * --selftest plants both failure directions to prove the gate has teeth.

Usage:
  py -3 scripts/check_deleted_crate_registry.py            # full check
  py -3 scripts/check_deleted_crate_registry.py --selftest # prove teeth
  py -3 scripts/check_deleted_crate_registry.py -v         # list both sets
Exit: 0 pass | 1 drift | 2 usage/environment error
Encoding: ASCII-only output (Windows GBK consoles).
"""
import os
import re
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
WIKI = os.path.join("docs", "architecture", "CODE_WIKI.md")
SECTION_RE = re.compile(r"^#{2,4}\s*3\.12\b")
ROW_RE = re.compile(r"^\|\s*`([a-z0-9][a-z0-9-]*)`")


def git_deleted_crates(root):
    """Criterion declared by CODE_WIKI 3.12 itself."""
    try:
        r = subprocess.run(
            ["git", "log", "--diff-filter=D", "--name-only", "--format=",
             "--", "crates/*/Cargo.toml"],
            cwd=root, capture_output=True, text=True, encoding="utf-8",
            errors="replace", check=True)
    except (OSError, subprocess.CalledProcessError) as exc:
        print("[ENV-FAIL] git invocation failed: %s" % exc)
        return None
    names = set()
    for line in r.stdout.splitlines():
        m = re.match(r"^crates/([^/]+)/Cargo\.toml$", line.strip())
        if m:
            names.add(m.group(1))
    if not names:
        print("[ENV-FAIL] git returned an empty deletion set -- cannot decide")
        return None
    return names


def registered_crates(root):
    """First back-quoted cell of each table row inside section 3.12."""
    path = os.path.join(root, WIKI)
    if not os.path.isfile(path):
        print("[ENV-FAIL] required file not found: %s" % WIKI)
        return None
    with open(path, encoding="utf-8", errors="replace") as fh:
        lines = fh.read().splitlines()
    start = None
    for i, ln in enumerate(lines):
        if SECTION_RE.match(ln):
            start = i
            break
    if start is None:
        print("[ENV-FAIL] section '3.12' not found in %s -- gate has no anchor" % WIKI)
        return None
    names = []
    for ln in lines[start + 1:]:
        if re.match(r"^#{1,4}\s", ln):          # next heading ends the section
            break
        m = ROW_RE.match(ln)
        if m:
            names.append(m.group(1))
    if not names:
        print("[ENV-FAIL] section 3.12 parsed but yielded 0 rows -- anchor rusted")
        return None
    return names


def verdict(git_set, reg_list, verbose=False):
    reg_set = set(reg_list)
    rc = 0
    unreg = sorted(git_set - reg_set)
    bogus = sorted(reg_set - git_set)
    if verbose:
        print("[MEASURED] git-deleted=%s" % ", ".join(sorted(git_set)))
        print("[REGISTERED] 3.12 rows=%s" % ", ".join(sorted(reg_set)))
    print("[SCAN] git-deleted=%d registered=%d" % (len(git_set), len(reg_set)))
    for n in unreg:
        print("[FAIL] deleted in git but NOT registered in 3.12: %s" % n)
        rc = 1
    for n in bogus:
        print("[FAIL] registered in 3.12 but no git deletion found: %s" % n)
        rc = 1
    if rc == 0:
        print("[OK] 3.12 registry is bidirectionally complete (%d entries)" % len(reg_set))
    return rc


def selftest():
    """Plant both failure directions; a gate that cannot see them is decorative."""
    git_set = {"a-old", "b-gone", "c-live-ish"}
    cases = [
        ("aligned", git_set, ["a-old", "b-gone", "c-live-ish"], 0),
        ("unregistered deletion", git_set, ["a-old", "b-gone"], 1),
        ("registry rot (phantom row)", {"a-old"}, ["a-old", "never-existed"], 1),
    ]
    failed = 0
    for label, g, r, expect in cases:
        # capture stdout so the planted evidence does not look like a real finding
        keep = sys.stdout
        sys.stdout = __import__("io").StringIO()
        got = verdict(g, r)
        sys.stdout = keep
        ok = "OK" if got == expect else "FAIL"
        if got != expect:
            failed += 1
        print("[SELFTEST] %-28s expect_rc=%d got_rc=%d %s" % (label, expect, got, ok))
    if failed:
        print("[SELFTEST] RESULT: FAIL (%d case(s) misjudged)" % failed)
        return 1
    print("[SELFTEST] RESULT: PASS")
    return 0


def main():
    if "--selftest" in sys.argv:
        return selftest()
    verbose = "-v" in sys.argv
    git_set = git_deleted_crates(ROOT)
    if git_set is None:
        return 2
    reg_list = registered_crates(ROOT)
    if reg_list is None:
        return 2
    return verdict(git_set, reg_list, verbose)


if __name__ == "__main__":
    import gate_rc  # crash -> exit 2, never a borrowed traceback exit 1

    sys.exit(gate_rc.run(main))
