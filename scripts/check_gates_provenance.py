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
    0  both directions clean
    1  at least one direction found an unaccounted script / missing script
    2  the exemption list or the manifest could not be read

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

SCRIPT_TOKEN = re.compile(r"scripts/([A-Za-z0-9_.\-]+\.(?:py|sh|ps1))")
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
        for path in sorted(scripts_dir.glob(f"*.{ext}")):
            try:
                text = path.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            if "selftest" not in text:
                continue
            rel = f"scripts/{path.name}"
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

    if problems:
        for p in problems:
            print(f"FAIL  {p}")
        print("SELFTEST: FAILED")
        return 1
    print("SELFTEST: PASSED (direction A + direction B, positive + negative)")
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
    except FileNotFoundError as exc:
        print(f"FAIL  {exc}")
        return 2
    except (ValueError, tomllib.TOMLDecodeError) as exc:
        print(f"FAIL  cannot parse {manifest} or {exempt_path}: {exc}")
        return 2

    mtext = manifest.read_text(encoding="utf-8", errors="replace")
    findings = check_direction_a(manifest, scripts_dir)
    b_findings, exempted = check_direction_b(scripts_dir, mtext, exemptions)
    findings += b_findings

    if args.json:
        print(
            json.dumps(
                {
                    "gates": len(load_manifest(manifest)),
                    "exempted": exempted,
                    "findings": findings,
                },
                indent=2,
            )
        )
    else:
        gates = load_manifest(manifest)
        print("== gate provenance, both directions (ADR-198 §5 #5) ==")
        print(f"  manifest gates: {len(gates)}")
        print(f"  exemptions:     {len(exemptions)} (each must carry a reason)")
        for e in exempted:
            print(f"    EXEMPT  {e['script']}  # {e['reason'][:60]}")
        print()
        if findings:
            for f in findings:
                print(f"  FINDING  [{f['direction']}] {f['script']}  -- {f['reason']}")
        else:
            print("  OK  direction A: every script the manifest names exists on disk")
            print("  OK  direction B: every self-testing script is registered or exempted")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
