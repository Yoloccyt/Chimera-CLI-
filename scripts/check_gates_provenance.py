#!/usr/bin/env python3
"""check_gates_provenance.py -- bidirectional gate provenance audit.

Why this script exists (ADR-198 §5 #5, registered as a ghost gate):
    `ADR-198 §5 #5` records `check_gates_provenance.py` as a gate that is
    "not on disk". The audit that recorded it also stated the script was
    "mentioned in AGENTS.md" -- but AGENTS.md has zero occurrences of the
    name. So the record of the ghost gate was itself an unsourced claim.
    This file is the deliverable; its own registration is left to a later
    batch per the ADR-197 D5 / ADR-198 discipline (script first, gate
    registration only once a spec_ref and a committee decision exist).

    Two distinct ghost-gate shapes have already been observed, and they
    point in opposite directions:
      A. manifest -> disk: a gate is registered whose script does not
         exist ("phantom registration" -- ADR-197 D5 warned that an
         unwired entry shows up as PARTIAL/DECORATIVE in
         check_gated_wiring_substance.py).
      C. disk -> registry: the script is neither a gate nor exempt and has
         no lifecycle row in tool_registry.toml. Without C, "still needed"
         and "nobody noticed it died" look identical for every non-gate
         script -- direction B only inspects files containing `selftest`.
      B. disk -> manifest: a real, self-testing script is neither
         registered nor exempted. ADR-198 §3.4 recorded the concrete
         instance -- old text claimed `G-60 = check_ps1_bom_gate.py`
         while `gate_manifest.toml` never contained G-60, even though the
         script was real and already wired into CI. Direction A cannot
         see this shape; only direction B can.

An untrustworthy criterion is worse than no criterion, so both directions
are checked and the exemption list is explicit (one reason per script, no
wildcards): a script that is neither registered nor exempted is a finding,
not a silent pass.

Semantics:
    0  all three directions clean
    1  at least one direction found an unaccounted / missing script
    2  the exemption list, the manifest or the registry could not be read

Usage:
    python scripts/check_gates_provenance.py [--root PATH] [--json] [--selftest]
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path

SCRIPT_TOKEN = re.compile(r"scripts/([A-Za-z0-9_.\-/]+\.(?:py|sh|ps1))")
EXEMPT_LINE = re.compile(r"^\s*([^=\s]+)\s*=\s*(\S.*?)\s*$")


def load_manifest(path: Path) -> list[dict]:
    """Return the [[gate]] blocks. tomllib keeps this honest instead of
    regex-parsing TOML by hand."""
    with path.open("rb") as fh:
        data = tomllib.load(fh)
    gates = data.get("gate", [])
    if not isinstance(gates, list):
        raise ValueError("gate is not an array of tables")
    return gates


def manifest_script_stems(manifest_text: str) -> set[str]:
    """Stems of every script the manifest names anywhere (cmd / note /
    ci_exempt). Extension is dropped on purpose: `check_perf_redlines.ps1`
    is the Windows twin of a gate registered as `check_perf_redlines.sh`,
    and treating them as two different scripts would demand an exemption
    for every twin in the tree."""
    return {Path(m).stem for m in SCRIPT_TOKEN.findall(manifest_text)}


def check_direction_a(manifest: Path, scripts_dir: Path) -> list[dict]:
    """manifest -> disk: every script the manifest names must exist."""
    text = manifest.read_text(encoding="utf-8", errors="replace")
    findings = []
    for token in sorted(set(SCRIPT_TOKEN.findall(text))):
        target = scripts_dir / token
        if not target.is_file():
            findings.append(
                {
                    "direction": "A manifest->disk",
                    "script": f"scripts/{token}",
                    "reason": "registered in gate_manifest.toml but not on disk",
                }
            )
    return findings


def check_direction_b(
    scripts_dir: Path, manifest_text: str, exemptions: dict[str, str]
) -> tuple[list[dict], list[dict]]:
    """disk -> manifest: every self-testing script must be registered or
    explicitly exempted. Returns (findings, exempted_hits)."""
    stems = manifest_script_stems(manifest_text)
    findings: list[dict] = []
    exempted_hits: list[dict] = []

    for ext in ("py", "sh", "ps1"):
        for path in sorted(scripts_dir.rglob(f"*.{ext}")):
            try:
                text = path.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            if "selftest" not in text:
                continue
            rel = f"scripts/{path.relative_to(scripts_dir).as_posix()}"
            if path.stem in stems:
                continue
            if rel in exemptions:
                exempted_hits.append({"script": rel, "reason": exemptions[rel]})
                continue
            findings.append(
                {
                    "direction": "B disk->manifest",
                    "script": rel,
                    "reason": "self-testing script is neither registered nor exempted",
                }
            )
    return findings, exempted_hits


def load_exemptions(path: Path) -> dict[str, str]:
    """Parse `script = reason` lines. A line without a reason is an error,
    not a silently accepted entry -- an exemption nobody justified is
    exactly the drift this script exists to catch."""
    exemptions: dict[str, str] = {}
    if not path.is_file():
        raise FileNotFoundError(f"exemption file missing: {path}")
    for lineno, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        m = EXEMPT_LINE.match(line)
        if not m:
            raise ValueError(f"{path}:{lineno}: malformed exemption line: {raw!r}")
        exemptions[m.group(1)] = m.group(2)
    return exemptions

REGISTRY_STATUS = ("active", "diagnostic", "evidence", "retired")


def load_registry(path: Path) -> dict[str, str]:
    """Parse tool_registry.toml into {script: status}.

    Direction C's third accounting source (ADR-228 §5). A missing file is an
    error, not an empty pass: "nobody registered anything" must never look the
    same as "everything is registered".
    """
    if not path.is_file():
        raise FileNotFoundError(f"tool registry missing: {path}")
    with path.open("rb") as fh:
        doc = tomllib.load(fh)
    rows = doc.get("tool", [])
    if not isinstance(rows, list):
        raise ValueError(f"{path}: tool is not an array of tables")
    out: dict[str, str] = {}
    for i, row in enumerate(rows, 1):
        if not isinstance(row, dict):
            raise ValueError(f"{path}: row {i} is not a table")
        script = str(row.get("script", "")).strip()
        status = str(row.get("status", "")).strip()
        if not script.startswith("scripts/"):
            raise ValueError(f"{path}: row {i}: script must live under scripts/")
        if status not in REGISTRY_STATUS:
            raise ValueError(f"{path}: row {i}: status {status!r} unknown")
        if script in out:
            raise ValueError(f"{path}: row {i}: duplicate script {script}")
        out[script] = status
    return out


def check_direction_c(
    scripts_dir: Path,
    manifest_text: str,
    exemptions: dict[str, str],
    registry: dict[str, str],
) -> tuple[list[dict], int]:
    """disk -> registry: every script on disk must have one disposition.

    Direction B only sees scripts carrying a `selftest` literal. That leaves
    most of the tree invisible: a 200-line helper with no selftest is neither a
    gate nor a finding, so "does this still belong here?" has no answer.
    Direction C demands exactly one of three: a registered gate, an explicit
    exemption, or a registry row that is not `retired`.

    `retired` deliberately does NOT account for a file that still exists: a
    retired row is history, and history must never keep a live file green.
    Returns (findings, scripts_scanned) -- the sample size is part of the
    evidence, not a footnote.
    """
    stems = manifest_script_stems(manifest_text)
    findings: list[dict] = []
    scanned = 0
    for ext in ("py", "sh", "ps1"):
        for path in sorted(scripts_dir.rglob(f"*.{ext}")):
            scanned += 1
            rel = f"scripts/{path.relative_to(scripts_dir).as_posix()}"
            if path.stem in stems or rel in exemptions:
                continue
            status = registry.get(rel)
            if status and status != "retired":
                continue
            reason = ("registry row is retired but the file still exists"
                      if status == "retired" else
                      "not in gate_manifest.toml, the exemptions, or tool_registry.toml")
            findings.append(
                {"direction": "C disk->registry", "script": rel, "reason": reason}
            )
    return findings, scanned


def selftest() -> int:
    """Four fixtures, both directions, plus the negative direction:
    a script that IS registered must not be reported."""
    import tempfile

    problems = []
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        scripts = root / "scripts"
        scripts.mkdir()
        manifest = root / "gate_manifest.toml"
        manifest.write_text(
            '[[gate]]\nid = "G-01"\ncmd = "{python} scripts/registered.py"\n',
            encoding="utf-8",
        )
        (scripts / "registered.py").write_text("x = 1  # selftest\n", encoding="utf-8")
        (scripts / "orphan.py").write_text("y = 2  # selftest\n", encoding="utf-8")
        (scripts / "plain.py").write_text("z = 3\n", encoding="utf-8")

        empty_exempt = root / "exemptions.txt"
        empty_exempt.write_text("# none\n", encoding="utf-8")

        mtext = manifest.read_text(encoding="utf-8")

        # Direction A: a registered-but-absent script must be caught.
        (manifest).write_text(
            '[[gate]]\nid = "G-01"\ncmd = "{python} scripts/registered.py"\n'
            '[[gate]]\nid = "G-02"\ncmd = "{python} scripts/phantom.py"\n',
            encoding="utf-8",
        )
        a_findings = check_direction_a(manifest, scripts)
        if not any("phantom.py" in f["script"] for f in a_findings):
            problems.append("selftest: direction A missed a registered-but-absent script")
        manifest.write_text(
            '[[gate]]\nid = "G-01"\ncmd = "{python} scripts/registered.py"\n',
            encoding="utf-8",
        )
        mtext = manifest.read_text(encoding="utf-8")

        # Direction B: orphan self-testing script must be caught; the
        # registered one and the non-selftest one must NOT be.
        b_findings, _ = check_direction_b(scripts, mtext, {})
        if not any("orphan.py" in f["script"] for f in b_findings):
            problems.append("selftest: direction B missed an unregistered selftest script")
        if any("registered.py" in f["script"] for f in b_findings):
            problems.append("selftest: direction B flagged a registered script (false positive)")
        if any("plain.py" in f["script"] for f in b_findings):
            problems.append("selftest: direction B flagged a script with no selftest")

        # Exemption must silence the finding, and only for the named script.
        b_findings2, exempted = check_direction_b(
            scripts, mtext, {"scripts/orphan.py": "reason"}
        )
        if b_findings2:
            problems.append(f"selftest: exemption did not silence the finding {b_findings2}")
        if len(exempted) != 1:
            problems.append("selftest: exemption bookkeeping wrong")

        nested = scripts / "nested"
        nested.mkdir()
        (nested / "deep_orphan.py").write_text("q = 4  # selftest\n",
                                             encoding="utf-8")
        deep_findings, _ = check_direction_b(scripts, mtext, {})
        if not any(f["script"] == "scripts/nested/deep_orphan.py"
                       for f in deep_findings):
            problems.append("selftest: direction B missed a nested script")
        (nested / "registered.py").write_text("r = 5  # selftest\n",
                                                 encoding="utf-8")
        manifest.write_text(
            '[[gate]]\nid = "G-01"\ncmd = "{python} scripts/registered.py"\n'
            '[[gate]]\nid = "G-03"\ncmd = "{python} scripts/nested/registered.py"\n',
            encoding="utf-8",
        )
        if check_direction_a(manifest, scripts):
            problems.append("selftest: direction A rejected nested path")
        deep2, _ = check_direction_b(
            scripts, manifest.read_text(encoding="utf-8"), {})
        if any("nested/registered.py" in f["script"] for f in deep2):
            problems.append("selftest: nested registered flagged")
        # Direction C: every file on disk needs one disposition -- including the
        # ones direction B cannot see (no `selftest` literal anywhere).
        c_all, scanned = check_direction_c(scripts, mtext, {}, {})
        if scanned != len(list(scripts.rglob("*.py"))):
            problems.append(f"selftest: direction C scanned {scanned}, tree has more")
        if not any(f["script"] == "scripts/plain.py" for f in c_all):
            problems.append("selftest: direction C ignored a non-selftest script")
        if not any(f["script"] == "scripts/orphan.py" for f in c_all):
            problems.append("selftest: direction C ignored a selftest orphan")
        c_reg, _ = check_direction_c(scripts, mtext, {}, {"scripts/plain.py": "evidence"})
        if any(f["script"] == "scripts/plain.py" for f in c_reg):
            problems.append("selftest: registry row did not account for a script")
        c_ret, _ = check_direction_c(scripts, mtext, {}, {"scripts/plain.py": "retired"})
        if not any(f["script"] == "scripts/plain.py" for f in c_ret):
            problems.append("selftest: retired row wrongly kept a live file green")
        reg = root / "tool_registry.toml"
        reg.write_text('[[tool]]\nscript = "scripts/plain.py"\nstatus = "evidence"\n',
                       encoding="utf-8")
        if load_registry(reg) != {"scripts/plain.py": "evidence"}:
            problems.append("selftest: load_registry did not round-trip")
        reg.write_text('[[tool]]\nscript = "scripts/plain.py"\nstatus = "zombie"\n',
                       encoding="utf-8")
        try:
            load_registry(reg)
            problems.append("selftest: load_registry accepted an unknown status")
        except ValueError:
            pass
    if problems:
        for p in problems:
            print(f"FAIL  {p}")
        print("SELFTEST: FAILED")
        return 1
    print("SELFTEST: PASSED (directions A + B + C, positive + negative)")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default=".", help="repository root")
    parser.add_argument("--json", action="store_true", help="machine-readable output")
    parser.add_argument("--selftest", action="store_true", help="verify the detector itself")
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()

    root = Path(args.root)
    scripts_dir = root / "scripts"
    manifest = scripts_dir / "gate_manifest.toml"
    exempt_path = scripts_dir / "gate_provenance_exemptions.txt"

    try:
        load_manifest(manifest)  # fail fast on malformed TOML
        exemptions = load_exemptions(exempt_path)
        registry = load_registry(scripts_dir / "tool_registry.toml")
    except FileNotFoundError as exc:
        print(f"FAIL  {exc}")
        return 2
    except (ValueError, tomllib.TOMLDecodeError) as exc:
        print(f"FAIL  cannot parse {manifest}, {exempt_path} or tool_registry.toml: {exc}")
        return 2

    mtext = manifest.read_text(encoding="utf-8", errors="replace")
    findings = check_direction_a(manifest, scripts_dir)
    b_findings, exempted = check_direction_b(scripts_dir, mtext, exemptions)
    c_findings, scanned = check_direction_c(scripts_dir, mtext, exemptions, registry)
    findings += b_findings + c_findings

    if args.json:
        print(
            json.dumps(
                {
                    "gates": len(load_manifest(manifest)),
                    "exempted": exempted,
                    "registry": len(registry),
                    "scripts_scanned": scanned,
                    "unaccounted": len(c_findings),
                    "findings": findings,
                },
                indent=2,
            )
        )
    else:
        gates = load_manifest(manifest)
        print("== gate provenance, three directions (ADR-198 §5 #5, ADR-228 §5) ==")
        print(f"  manifest gates: {len(gates)}")
        print(f"  exemptions:     {len(exemptions)} (each must carry a reason)")
        print(f"  registry rows:  {len(registry)} (tool_registry.toml, ADR-228 §5)")
        print(f"  scripts scanned: {scanned}")
        for e in exempted:
            print(f"    EXEMPT  {e['script']}  # {e['reason'][:60]}")
        print()
        if findings:
            for f in findings:
                print(f"  FINDING  [{f['direction']}] {f['script']}  -- {f['reason']}")
        else:
            print("  OK  direction A: every script the manifest names exists on disk")
            print("  OK  direction B: every self-testing script is registered or exempted")
            print(f"  OK  direction C: all {scanned} scripts have a disposition "
                  f"(manifest / exemption / registry)")
    return 1 if findings else 0


if __name__ == "__main__":
    import gate_rc  # crash exits 2, never borrows 1 (F32/F33)
    sys.exit(gate_rc.run(main))
