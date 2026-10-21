#!/usr/bin/env python3
# -*- coding: ascii -*-
"""PowerShell syntax gate (G-127, closes ADR-222 R8-9).

Why this gate exists
--------------------
The repository keeps `scripts/check_dependency_rules.ps1` as the local twin of the
CI-run `.sh`. On 2026-10-20 that file carried a duplicate hashtable key: PowerShell
refused to parse the whole script (`hash literal cannot contain duplicate key`)
while every gate stayed green, because the parity parser (`check_layer_map_parity`)
builds a dict from `findall` pairs -- a duplicate is silently collapsed, and CI only
runs the `.sh` sibling. The mechanical shape (duplicate keys) is now judged by G-45/46
and G-125; this gate closes the remaining hole: **no executor ever parsed the .ps1
as PowerShell**, so any other PowerShell-only syntax error would also go unnoticed.

Judge (three states, never a silent pass)
-----------------------------------------
  0  every target parses with zero diagnostics
  1  at least one target produced parse diagnostics (line:col: message)
  2  no PowerShell host / host timeout / bad usage -> undecidable

Axis separation: G-45/G-46 own the *content* parity of the layer tables, G-125 owns
the sub-layer direction. This gate owns only "can the shipped .ps1 be parsed at all".

Modes / exit codes
------------------
  (default)   judge the real repo      0 = clean, 1 = diagnostics, 2 = undecidable
  --selftest  planted fixtures must be caught in both directions
                                       0 = all assertions held, 1 = not, 2 = no host

Output is pure ASCII (project script convention; avoids Windows GBK console traps).
"""
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

# Relative to ROOT. Kept as a tuple so adding a target is a one-line change.
PS1_TARGETS = ("scripts/check_dependency_rules.ps1",)

# PowerShell parses the file and prints one `line:col: message` per diagnostic.
# The target path travels through the environment so no shell quoting is involved.
# WHY explicit UTF-8 at both ends (measured 2026-10-21): under a redirected pipe
# both Windows PowerShell 5.1 and pwsh emit diagnostics in the console code page
# (GBK on this host). Decoding with the Python locale codec raised
# UnicodeDecodeError inside the reader thread, so judge() saw empty output and
# the "diagnostic names a line" selftest assertion went red (reproduced 10/10).
# The command therefore pins console/pipeline output to UTF-8, and the Python
# side decodes with encoding="utf-8" + errors="replace": errors="replace" never
# drops a whole line when a host still emits non-UTF-8 bytes (degraded text
# beats reading "no diagnostics" out of a decode failure).
PS_PARSE_COMMAND = (
    "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;"
    "$OutputEncoding=[System.Text.Encoding]::UTF8;"
    "$t=$null;$e=$null;"
    "[void][System.Management.Automation.Language.Parser]::ParseFile("
    "$env:CHIMERA_PS1_TARGET,[ref]$t,[ref]$e);"
    "if($e.Count -gt 0){"
    "foreach($x in $e){Write-Output (\"{0}:{1}: {2}\" -f "
    "$x.Extent.StartLineNumber,$x.Extent.StartColumnNumber,$x.Message)};"
    "exit 1};exit 0"
)

HOST_TIMEOUT_SECS = 120


def find_host():
    """First available PowerShell host, or None.

    `pwsh` first (what CI and the repo's own local commands use); `powershell`
    (Windows PowerShell 5.1) is accepted as a fallback because it exposes the same
    Parser API -- the judgement is identical, only the host differs.
    """
    for name in ("pwsh", "powershell"):
        found = shutil.which(name)
        if found:
            return found
    return None


def run_host(path):
    """(returncode, combined output) from the PowerShell parser.

    OSError / timeout become an undecidable marker instead of raising: an
    environment failure must not masquerade as a red verdict (gate_rc H7).
    """
    host = find_host()
    if not host:
        return 127, "no PowerShell host (pwsh/powershell) on PATH"
    env = dict(os.environ)
    env["CHIMERA_PS1_TARGET"] = os.path.abspath(path)
    try:
        proc = subprocess.run(
            [host, "-NoProfile", "-NonInteractive", "-Command", PS_PARSE_COMMAND],
            capture_output=True, text=True, encoding="utf-8", errors="replace",
            env=env, timeout=HOST_TIMEOUT_SECS)
    except subprocess.TimeoutExpired:
        return 124, "PowerShell host timed out after %ds" % HOST_TIMEOUT_SECS
    except OSError as exc:
        return 126, "PowerShell host failed to start: %s" % exc
    return proc.returncode, (proc.stdout or "") + (proc.stderr or "")


def judge(path, runner=None):
    """(rc, lines) for one .ps1 path. `runner` is the seam used by --selftest."""
    runner = runner or run_host
    code, output = runner(path)
    lines = [ln for ln in output.splitlines() if ln.strip()]
    if code == 0:
        return 0, []
    if code == 1:
        return 1, lines or ["[FAIL] parser reported diagnostics without text"]
    return 2, ["[UNDECIDABLE] host rc=%s: %s" % (code, lines[0] if lines else "(no output)")]


def judge_targets(targets, runner=None):
    """(rc, report) over several files; first undecidable wins, all diagnostics kept."""
    report = []
    for rel in targets:
        path = os.path.join(ROOT, rel)
        if not os.path.isfile(path):
            report.append("[FAIL] %s: target file is missing" % rel)
            return 1, report
        rc, lines = judge(path, runner=runner)
        if rc == 2:
            report.append("%s: %s" % (rel, lines[0] if lines else "[UNDECIDABLE]"))
            return 2, report
        for line in lines:
            report.append("%s: %s" % (rel, line))
    if report:
        return 1, report
    return 0, ["[OK] PowerShell syntax parses: %d file(s)" % len(targets)]


def selftest():
    """Every judgement must bite in both directions, through the production judge."""
    n = 0
    bad = 0

    def check(label, got, want):
        nonlocal n, bad
        n += 1
        if got != want:
            bad += 1
            print("  [FAIL] %s got=%r want=%r" % (label, got, want))
        else:
            print("  [PASS] %s -> %r" % (label, got))

    if not find_host():
        print("  [UNDECIDABLE] --selftest needs a PowerShell host on PATH")
        return 2

    with tempfile.TemporaryDirectory() as tmp:
        clean = os.path.join(tmp, "clean.ps1")
        with open(clean, "w", encoding="ascii", newline="\n") as fh:
            fh.write("$value = 1\nWrite-Output $value\n")

        dupkey = os.path.join(tmp, "dupkey.ps1")
        with open(dupkey, "w", encoding="ascii", newline="\n") as fh:
            fh.write("$map = @{\n    'same' = 1; 'same' = 2\n}\n")

        broken = os.path.join(tmp, "broken.ps1")
        with open(broken, "w", encoding="ascii", newline="\n") as fh:
            fh.write("if ($true) {\n    Write-Output 1\n")

        rc, _ = judge(clean)
        check("clean fixture parses", rc, 0)

        rc, lines = judge(dupkey)
        check("duplicate hashtable key is red", rc, 1)
        check("duplicate key diagnostic is non-empty", bool(lines), True)

        rc, lines = judge(broken)
        check("missing closing brace is red", rc, 1)
        check("missing brace diagnostic names a line", bool(lines) and ":" in lines[0], True)

        # Environment failures must be undecidable, never a red verdict.
        rc, _ = judge(clean, runner=lambda _p: (127, "no PowerShell host"))
        check("missing host is undecidable", rc, 2)
        rc, _ = judge(clean, runner=lambda _p: (3, "unexpected host failure"))
        check("unexpected host rc is undecidable", rc, 2)
        rc, _ = judge(clean, runner=lambda _p: (124, "host timed out"))
        check("host timeout is undecidable", rc, 2)

        # The real tree must be green through the same multi-target path.
        rc, report = judge_targets(PS1_TARGETS)
        check("real tree rc", rc, 0)
        check("real tree reports a parse line", report[0].startswith("[OK]"), True)

    print("  RESULT: %s (%d assertions)" % ("PASS" if not bad else "FAIL", n))
    return 1 if bad else 0


def main(argv):
    if argv == ["--selftest"]:
        return selftest()
    if argv:
        print("usage: check_ps1_syntax.py [--selftest]")
        return 2

    rc, report = judge_targets(PS1_TARGETS)
    for line in report:
        print(line)
    if rc == 2:
        return 2
    if rc == 1:
        print("[FAIL] PowerShell syntax gate: parse diagnostics above")
        return 1
    return 0


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
