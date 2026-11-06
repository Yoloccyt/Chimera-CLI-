#!/usr/bin/env python3
"""G-141/G-142: lexical sanity for .github/workflows/*.yml.

WHY THIS GATE EXISTS. On 2026-10-29 a step was added as

    - name: Format check (isolated workspace: fuzz)

A plain YAML scalar may not contain ": ", so that line is a mapping inside a
mapping value and the file does not parse. GitHub rejects the whole workflow at
registration, which does not fail one job -- it drops every job in ci.yml,
including the actionlint job that would otherwise have caught it. No existing
local check noticed: `check_ci_manifest_parity.py` greps for script paths, and
`zizmor` prints "failed to parse input" yet still exits 0 (a green tool that read
nothing). So the class needed a cheap, stdlib, deterministic guard.

Rules (deliberately narrow -- this is a lexical tripwire, not a YAML parser):
  L1  plain-scalar value containing ": " outside a trailing comment
  L2  unbalanced quotes on a key/value line

Dialect: 0 clean, 1 findings, 2 undecidable (missing/unreadable input).
"""
from __future__ import annotations

import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gate_rc  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WORKFLOWS = os.path.join(ROOT, ".github", "workflows")

KV_RE = re.compile(r"^(?P<indent>\s*)(?:-\s+)?(?P<key>[A-Za-z0-9_][A-Za-z0-9_-]*):(?P<sep>[ \t]+)(?P<val>.*?)(?P<eol>[ \t]*)$")
BLOCK_START = ("|", ">", "&", "*", "{", "[", "'", '"', "!", "?")
PROBE_MIN = 40  # fewer parsed lines means the walk saw almost nothing: blind, not clean

QUOTES = ("'", '"')


def _strip_comment(val):
    """Drop a trailing `# ...` comment, but only when the '#' starts a token."""
    for i, ch in enumerate(val):
        if ch != "#":
            continue
        if i == 0 or val[i - 1] in " \t":
            return val[:i].rstrip()
    return val


def _unbalanced(text):
    for q in QUOTES:
        # a quote preceded by a backslash is escaped in YAML double-quoted style
        n = len(re.findall(r"(?<!\\)" + re.escape(q), text))
        if n % 2:
            return True
    return False


def check_lines(path, lines):
    """Return [(lineno, rule, excerpt)] for the lexical rules above."""
    findings = []
    for no, raw in enumerate(lines, 1):
        line = raw.rstrip("\n")
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        m = KV_RE.match(line)
        if not m:
            continue
        val = _strip_comment(m.group("val"))
        if not val or val[0] in BLOCK_START:
            if val and _unbalanced(val):
                findings.append((no, "L2-unbalanced-quote", stripped[:110]))
            continue
        if _unbalanced(val):
            findings.append((no, "L2-unbalanced-quote", stripped[:110]))
            continue
        if ": " in val:
            findings.append((no, "L1-plain-scalar-colon", stripped[:110]))
    return findings


def scan(workflow_dir):
    """(findings, files, lines) -- callers must refuse to judge on a blind walk."""
    findings, files, total = [], 0, 0
    for name in sorted(os.listdir(workflow_dir)):
        if not name.endswith((".yml", ".yaml")):
            continue
        path = os.path.join(workflow_dir, name)
        try:
            with open(path, encoding="utf-8", errors="replace") as fh:
                lines = fh.readlines()
        except OSError as exc:                      # unreadable input is not clean
            raise ValueError("cannot read %s: %s" % (path, exc))
        files += 1
        total += len(lines)
        for no, rule, excerpt in check_lines(path, lines):
            findings.append((os.path.join(".github/workflows", name), no, rule, excerpt))
    return findings, files, total


def selftest():
    """Planted positive + planted negatives. The positive is the real 2026-10-29 line."""
    cases = [
        ("L1 catches the exact step name that broke ci.yml",
         ["      - name: Format check (isolated workspace: fuzz)"], True),
        ("quoted value is fine",
         ['      - name: "Format check (isolated workspace: fuzz)"'], False),
        ("block scalar is fine",
         ["      - run: |", "          echo 'note: block scalars are opaque'"], False),
        ("colon inside a trailing comment is fine",
         ["      - uses: actions/checkout@v4  # zizmor: ignore[cache-poisoning]"], False),
        ("colon inside a quoted comment-ish value is fine",
         ['      - name: "ci: gate suite"'], False),
        ("plain URL value is fine (no space after colon)",
         ["      - uses: actions/upload-artifact@v4"], False),
        ("unbalanced quote is caught",
         ['      - name: "broken quote value'], True),
        ("mapping key with empty value is fine",
         ["jobs:", "  gate:", "    runs-on: ubuntu-latest"], False),
    ]
    failed = 0
    for name, lines, want_hit in cases:
        hits = check_lines("inline.yml", lines)
        got = bool(hits)
        if got != want_hit:
            failed += 1
            print("FAIL  %s -- got %s want %s (%r)" % (name, got, want_hit, hits))
        else:
            print("ok    %s" % name)
    # an unreadable/missing input must come back undecidable, never "clean"
    missing = main(["--root", os.path.join(os.getcwd(), "no-such-workflow-dir")])
    if missing != 2:
        failed += 1
        print("FAIL  missing directory is undecidable -- got %d want 2" % missing)
    else:
        print("ok    missing directory is undecidable, not clean")
    # teeth on production shape: the real ci.yml regression, as an actual file.
    # gate_rc.tmpdir() so the fixture cannot leak a directory (batch doctrine).
    bad = gate_rc.tmpdir("yamlgate-")
    with open(os.path.join(bad, "ci.yml"), "w", encoding="utf-8", newline="\n") as fh:
        fh.write("jobs:\n  g:\n    runs-on: ubuntu-latest\n    steps:\n"
                 "      - name: Format check (isolated workspace: fuzz)\n"
                 "      - run: cargo fmt --all -- --check\n"
                 + "".join("      # padding %d\n" % i for i in range(40)))
    rc_bad = main(["--root", bad])
    if rc_bad != 1:
        failed += 1
        print("FAIL  bad workflow file must be a finding -- got %d want 1" % rc_bad)
    else:
        print("ok    bad workflow file is reported, not just inline lines")
    print("SELFTEST: %s (%d/%d cases)" % ("FAILED" if failed else "ALL ASSERTIONS HELD",
                                          failed, len(cases) + 2))
    return 1 if failed else 0


def main(argv=None):
    p = argparse.ArgumentParser(description="G-141 workflow YAML lexical guard")
    p.add_argument("--root", default=WORKFLOWS, help="directory holding the workflow files")
    p.add_argument("--selftest", action="store_true")
    args = p.parse_args(argv)
    if args.selftest:
        return selftest()
    if not os.path.isdir(args.root):
        print("[UNDECIDABLE] workflow directory missing: %s" % args.root)
        return 2
    try:
        findings, files, total = scan(args.root)
    except ValueError as exc:
        print("[UNDECIDABLE] %s" % exc)
        return 2
    print("scanned=%d files lines=%d findings=%d" % (files, total, len(findings)))
    if total < PROBE_MIN:
        print("[UNDECIDABLE] only %d lines parsed from %d file(s) -- refusing to "
              "report clean on a blind walk" % (total, files))
        return 2
    for path, no, rule, excerpt in findings:
        print("  [%s] %s:%d: %s" % (rule, path, no, excerpt))
    if findings:
        print("[FAIL] %d lexical finding(s); GitHub would reject the file at "
              "registration and silently drop every job in it" % len(findings))
        return 1
    print("[OK] workflow files lexically sound (%d files)" % files)
    return 0


if __name__ == "__main__":
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
