#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L1 error-model convention gate (L1-F-4).

Why this gate exists
--------------------
Project rule (AGENTS.md 4.1 / nuxus rules 0): library crates use `thiserror`,
application layers use `anyhow`. Until now that was enforced by code review
only, and review cannot see the next `use anyhow::Result` added to a library
crate. This gate is the machine form of the rule for the L1 core-foundation
face (`event-bus` / `nexus-core` / `nexus-compute` / `nexus-telemetry` /
`nexus-events`).

Families
--------
  E1  no `anyhow` in L1 `src/**` (comments and string literals are blanked with
      the shared Rust lexer, so prose about anyhow is not a hit). RED.
  E2  every `pub enum *Error` / `pub struct *Error` carries `#[non_exhaustive]`,
      unless registered in `scripts/l1_error_model_exempt.txt` with a reason.
      RED. A stale exemption (the type is gone) is red too, so the ledger
      cannot rot.
  E3  a `Display` rendering of an error used INSIDE a control-flow decision
      (`if` / `match` / `==` on the same line) -- `err.to_string()`,
      `format!("{}", err)`. WARN only: deciding whether the rendered text
      really drives the branch needs type information, so this family reports
      and never blocks. Exit code is unaffected.

Scan face
---------
`crates/<crate>/src/**/*.rs` minus files reached through `#[cfg(test)] mod X;`
(imported from `check_l1_dead_surface._test_module_files`, the single source of
that rule), and minus inline `#[cfg(test)] mod tests { ... }` tails.

Modes / exit codes
------------------
  (default)   E1 or E2 hit -> 1; only E3 warnings -> 0; undecidable -> 2
  --json      machine-readable findings
  --selftest  fixtures prove every direction through the production judge

Output is pure ASCII (project script convention).
"""

import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)

import check_l0_purity as purity  # noqa: E402  (shared Rust lexer + test-tail mask)
import check_l1_dead_surface as dead_surface  # noqa: E402  (_test_module_files SSOT)

EXEMPT = os.path.join(HERE, "l1_error_model_exempt.txt")

TARGET_CRATES = ("event-bus", "nexus-core", "nexus-compute",
                 "nexus-telemetry", "nexus-events")

# Below this many public error types the scan is presumed blind, not clean.
ERROR_TYPE_FLOOR = 3

ANYHOW_RE = re.compile(r"\banyhow\b")

ERROR_DECL_RE = re.compile(r"^pub\s+(?:enum|struct)\s+(\w*Error)\b")

NON_EXHAUSTIVE_RE = re.compile(r"^#\[\s*non_exhaustive\s*\]$")

# E3: a Display rendering sitting on the same line as a branch/comparison.
E3_LEADING_RE = re.compile(
    r"(?:\bif\b|\bmatch\b|==).*(?:\berr\w*\.to_string\(\)"
    r"|format!\(\s*\"\{\}\"\s*,\s*err)")
E3_TRAILING_RE = re.compile(
    r"(?:\berr\w*\.to_string\(\)|format!\(\s*\"\{\}\"\s*,\s*err).*==")

ROW_RE = re.compile(r"^([^\s:]+::[^\s:]+::[^\s:]+)$")


class ModelError(ValueError):
    """Anything that makes the judgement undecidable (never a verdict)."""


def _prod_files(root, crate):
    """(src_dir, production files) for one crate."""
    src_dir = os.path.join(root, "crates", crate, "src")
    if not os.path.isdir(src_dir):
        raise ModelError("missing crate src dir: %s" % src_dir)
    skip = dead_surface._test_module_files(src_dir)
    return src_dir, [p for p in dead_surface._files_under(src_dir)
                     if os.path.normcase(os.path.abspath(p)) not in skip]


def _read(path):
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            return fh.read()
    except OSError as exc:
        raise ModelError("cannot read %s: %s" % (path, exc))


def _has_non_exhaustive(lines, mask, index):
    """True when `#[non_exhaustive]` is in the attribute block above `index`."""
    j = index - 1
    while j >= 0:
        if mask[j]:
            j -= 1
            continue
        stripped = lines[j].strip()
        if not stripped:
            j -= 1
            continue
        if stripped.startswith("#["):
            if NON_EXHAUSTIVE_RE.match(stripped):
                return True
            j -= 1
            continue
        break
    return False


def scan_crate(root, crate, exempt):
    """(findings, warnings, error_type_count) for one crate."""
    src_dir, files = _prod_files(root, crate)
    findings = []
    warnings = []
    seen_types = set()

    for path in files:
        rel = os.path.relpath(path, root).replace(os.sep, "/")
        src = _read(path)
        lines = purity.blank_rust(src).splitlines()
        mask = purity.test_tail_mask(lines)

        for i, line in enumerate(lines):
            if mask[i]:
                continue
            if ANYHOW_RE.search(line):
                findings.append(
                    "E1 %s:%d anyhow reference in L1 library code "
                    "(library errors must be thiserror)" % (rel, i + 1))
            decl = ERROR_DECL_RE.match(line)
            if not decl:
                continue
            name = decl.group(1)
            module = dead_surface.module_key(path, src_dir)
            key = "%s::%s::%s" % (crate, module, name)
            seen_types.add(key)
            if _has_non_exhaustive(lines, mask, i) or key in exempt:
                continue
            findings.append(
                "E2 %s:%d %s lacks #[non_exhaustive] (add it, or register "
                "'%s' in scripts/l1_error_model_exempt.txt with a reason)"
                % (rel, i + 1, name, key))

        # E3 reads the RAW lines: the pattern lives inside string literals and
        # `blank_rust` would erase exactly the evidence.
        for i, line in enumerate(src.splitlines()):
            code = line.split("//", 1)[0]
            if E3_LEADING_RE.search(code) or E3_TRAILING_RE.search(code):
                warnings.append(
                    "E3 %s:%d error Display used inside a control-flow branch "
                    "(fragile: rendered text is not a stable discriminator)"
                    % (rel, i + 1))
    return findings, warnings, len(seen_types)


def parse_exempt(path):
    """(set of keys, malformed lines) from the exemption ledger."""
    keys = set()
    malformed = []
    with open(path, "r", encoding="utf-8") as fh:
        for no, raw in enumerate(fh, 1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            if "#" not in line:
                malformed.append((no, "missing '# reason'"))
                continue
            key, reason = line.split("#", 1)
            key = key.strip()
            if not ROW_RE.match(key):
                malformed.append((no, "malformed key %r" % key))
                continue
            if not reason.strip():
                malformed.append((no, "empty exemption reason"))
                continue
            keys.add(key)
    return keys, malformed


def judge(root, exempt_path, crates=TARGET_CRATES, floor=ERROR_TYPE_FLOOR):
    """(rc, findings, warnings, error_type_count)."""
    try:
        exempt = set()
        if os.path.isfile(exempt_path):
            exempt, malformed = parse_exempt(exempt_path)
            if malformed:
                no, why = malformed[0]
                return 2, ["[MALFORMED] %s:%d: %s"
                           % (os.path.basename(exempt_path), no, why)], [], 0
        findings = []
        warnings = []
        total_types = 0
        for crate in crates:
            crate_findings, crate_warnings, count = scan_crate(root, crate, exempt)
            findings.extend(crate_findings)
            warnings.extend(crate_warnings)
            total_types += count
    except ModelError as exc:
        return 2, ["[UNDECIDABLE] %s" % exc], [], 0
    except Exception as exc:  # noqa: BLE001 - any scan failure is undecidable
        return 2, ["[UNDECIDABLE] scan failed: %s" % exc], [], 0

    if total_types < floor:
        return 2, ["[UNDECIDABLE] blind scan: %d error type(s) < floor %d"
                   % (total_types, floor)], [], total_types

    for key in sorted(exempt - _live_keys(root, crates)):
        findings.append("E2-EXEMPT %s is registered but no such error type exists; "
                        "delete the row in the same commit" % key)
    return (1 if findings else 0), findings, warnings, total_types


def _live_keys(root, crates):
    """Every `crate::module::*Error` that exists in the production face."""
    live = set()
    for crate in crates:
        src_dir, files = _prod_files(root, crate)
        for path in files:
            src = _read(path)
            lines = purity.blank_rust(src).splitlines()
            mask = purity.test_tail_mask(lines)
            module = dead_surface.module_key(path, src_dir)
            for i, line in enumerate(lines):
                if mask[i]:
                    continue
                decl = ERROR_DECL_RE.match(line)
                if decl:
                    live.add("%s::%s::%s" % (crate, module, decl.group(1)))
    return live


def _write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="ascii", newline="\n") as fh:
        fh.write(text)


def selftest():
    """Fixtures prove E1/E2 both directions and the E3 warn-only semantics."""
    import gate_rc

    state = {"n": 0, "bad": 0}

    def check(label, got, want):
        state["n"] += 1
        if got != want:
            state["bad"] += 1
            print("  [FAIL] %s got=%r want=%r" % (label, got, want))
        else:
            print("  [PASS] %s -> %r" % (label, got))

    tmp = gate_rc.tmpdir(prefix="l1-error-")
    try:
        crates = ("aaa",)
        lib = os.path.join(tmp, "crates", "aaa", "src", "lib.rs")
        clean = ("pub enum CleanError {\n    One,\n}\n"
                 "/// prose about anyhow is not a hit\n"
                 "pub enum AlsoFine {\n    A,\n}\n"
                 "pub struct ThirdError;\n")
        _write(lib, clean)
        ledger = os.path.join(tmp, "exempt.txt")
        rc, findings, _warnings, _types = judge(tmp, ledger, crates=crates, floor=1)
        check("a missing exemption ledger is not undecidable", rc, 1)

        _write(ledger, "# fixture ledger\n")
        rc, findings, _warnings, _types = judge(tmp, ledger, crates=crates, floor=1)
        check("missing #[non_exhaustive] on *Error is red", rc, 1)
        check("E2 names the type and the exemption key",
              any("E2 " in f and "aaa::lib::CleanError" in f for f in findings), True)
        check("`pub struct *Error` is checked too",
              any("ThirdError" in f for f in findings), True)

        _write(lib, "pub enum CleanError {\n    One,\n}\n"
                    "/// prose about anyhow is not a hit\n"
                    "pub enum AlsoFine {\n    A,\n}\n"
                    "pub struct ThirdError;\n"
                    "use anyhow::Result;\n")
        rc, findings, _warnings, _types = judge(tmp, ledger, crates=crates, floor=1)
        check("an anyhow import is red", rc, 1)
        check("E1 points at the anyhow line",
              any("E1 " in f and "anyhow" in f for f in findings), True)

        _write(lib, "#[derive(Debug)]\n#[non_exhaustive]\npub enum CleanError {\n"
                    "    One,\n}\n"
                    "#[non_exhaustive]\npub struct ThirdError;\n"
                    "pub enum AlsoFine {\n    A,\n}\n")
        _write(ledger, "# no exemptions needed\n")
        rc, findings, _warnings, _types = judge(tmp, ledger, crates=crates, floor=1)
        check("#[non_exhaustive] on every *Error is green", rc, 0)

        _write(ledger, "aaa::lib::CleanError  # accepted for this fixture\n")
        _write(lib, "pub enum CleanError {\n    One,\n}\n"
                    "pub enum AlsoFine {\n    A,\n}\n"
                    "pub struct ThirdError;\n")
        rc, findings, _warnings, _types = judge(tmp, ledger, crates=crates, floor=1)
        check("an exemption is green for the named type", rc, 1)
        check("other *Error types stay red",
              any("ThirdError" in f for f in findings), True)

        _write(ledger, "aaa::lib::GoneError  # registered but the type is gone\n")
        _write(lib, "pub enum CleanError {\n    One,\n}\n"
                    "pub enum AlsoFine {\n    A,\n}\n"
                    "pub struct ThirdError;\n")
        rc, findings, _warnings, _types = judge(tmp, ledger, crates=crates, floor=1)
        check("a stale exemption is red", rc, 1)
        check("the stale row is named",
              any("GoneError" in f for f in findings), True)

        _write(ledger, "aaa::lib::CleanError\n")
        rc, _findings, _warnings, _types = judge(tmp, ledger, crates=crates, floor=1)
        check("an exemption without a reason is undecidable", rc, 2)

        _write(ledger, "")
        _write(lib, "#[non_exhaustive]\npub enum CleanError {\n    One,\n}\n"
                    "#[non_exhaustive]\npub enum AlsoFine {\n    A,\n}\n"
                    "#[non_exhaustive]\npub struct ThirdError;\n"
                    "fn check(err: ThirdError) -> bool {\n"
                    "    if err.to_string() == \"boom\" {\n        return true;\n    }\n"
                    "    false\n}\n"
                    "// if err.to_string() == \"boom\" { } -- comment, not a hit\n")
        rc, findings, warnings, _types = judge(tmp, ledger, crates=crates, floor=1)
        check("E3 alone leaves the gate green", rc, 0)
        check("E3 is reported as a warning",
              any("E3 " in w for w in warnings), True)
        check("E3 does not fire on comments",
              len([w for w in warnings if "comment" in w]), 0)

        # `#[cfg(test)] mod X;` external test modules are not production code.
        _write(os.path.join(tmp, "crates", "aaa", "src", "lib.rs"),
               "#[non_exhaustive]\npub enum CleanError {\n    One,\n}\n"
               "#[non_exhaustive]\npub enum AlsoFine {\n    A,\n}\n"
               "#[non_exhaustive]\npub struct ThirdError;\n"
               "#[cfg(test)]\nmod fixture_tests;\n")
        _write(os.path.join(tmp, "crates", "aaa", "src", "fixture_tests.rs"),
               "use anyhow::Result;\npub enum TestOnlyError {\n    X,\n}\n")
        rc, findings, _warnings, _types = judge(tmp, ledger, crates=crates, floor=1)
        check("cfg(test) external modules are out of scope", rc, 0)

        rc, _findings, _warnings, _types = judge(tmp, ledger, crates=crates, floor=10 ** 6)
        check("blind scan is undecidable", rc, 2)

        rc, _findings, _warnings, _types = judge(tmp, ledger, crates=("ghost",), floor=1)
        check("missing crate src dir is undecidable", rc, 2)
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)

    print("  RESULT: %s (%d assertions)"
          % ("PASS" if not state["bad"] else "FAIL", state["n"]))
    return 1 if state["bad"] else 0


def main(argv):
    as_json = False
    for arg in argv:
        if arg == "--selftest":
            return selftest()
        if arg == "--json":
            as_json = True
        else:
            print("usage: check_l1_error_model.py [--json|--selftest]")
            return 2

    rc, findings, warnings, types = judge(ROOT, EXEMPT)
    if as_json:
        sys.stdout.write(json.dumps({"verdict": rc, "findings": findings,
                                     "warnings": warnings,
                                     "error_types": types},
                                    indent=2, sort_keys=True) + "\n")
        return rc

    for line in warnings:
        print("[WARN] " + line)
    for line in findings:
        print("[FAIL] " + line)
    if rc == 2:
        print("[UNDECIDABLE] L1 error-model gate cannot decide")
    elif rc == 1:
        print("[FAIL] L1 error model violated (%d finding(s), %d warning(s))"
              % (len(findings), len(warnings)))
    else:
        print("[OK] L1 error model holds: %d error type(s), %d warning(s)"
              % (types, len(warnings)))
    return rc


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
