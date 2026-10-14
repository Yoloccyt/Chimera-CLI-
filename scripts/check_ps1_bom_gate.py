#!/usr/bin/env python3
"""check_ps1_bom_gate.py -- UTF-8 BOM gate for Windows PowerShell scripts (ADR-198).

Why this gate exists (measured 2026-10-02):
    `scripts/check_dependency_rules.ps1` ran green under pwsh 7 (`rc=0`) but Windows
    PowerShell 5.1 failed with `386:1 unexpected token '}'`. The file had no UTF-8
    BOM (first bytes `23 20 3D`) and carried 1034 non-ASCII bytes, so 5.1 decoded it
    as ANSI (GBK): multi-byte sequences swallowed quote/backtick characters and
    shifted the tokenizer. A repo-wide scan showed 21 `.ps1` files without BOM and
    **11 of them unparseable under 5.1 (55 parse errors total)**.
    CI and scripts/gate_manifest.toml invoke `.ps1` via `pwsh` (UTF-8 by default),
    so nothing on the CI path noticed: the only 5.1 consumer was the root e2e test
    `tests/dependency_rules_inner_ring_test.rs`. That means the "Windows authoritative
    executor" claim in ADR-166/175 was unenforced, and the gate that ADR-187 text
    credits for this exact issue (`check_ps1_bom_gate.py`) did not exist on disk.
    This file lands it (gate id G-85).

Rule:
    A `.ps1` containing non-ASCII bytes MUST carry a UTF-8 BOM.
    Rationale: 5.1 needs the BOM to pick UTF-8 over the ANSI codepage; pure-ASCII
    files decode identically either way, so we do not force a diff on them.
    Pure-ASCII-without-BOM files are reported as INFO so the ledger stays visible.

Output is ASCII only (project convention: PowerShell 5.1 GBK console safety).

Usage:
    python scripts/check_ps1_bom_gate.py [--root DIR] [--json] [--selftest]

Exit codes:
    0  no violations
    1  at least one violation (non-ASCII without BOM)
    2  undecidable (root missing / nothing scanned)
"""

from __future__ import annotations

import argparse
import json
import sys
import tempfile
from pathlib import Path

BOM = b"\xef\xbb\xbf"


def inspect_bytes(data: bytes) -> tuple[bool, int]:
    """Return (has_bom, non_ascii_byte_count) for a file's raw bytes."""
    has_bom = data.startswith(BOM)
    non_ascii = sum(1 for b in data if b > 0x7F)
    return has_bom, non_ascii


def scan_dir(root: Path) -> tuple[list[dict], list[dict]]:
    """Scan *.ps1 under root.

    Returns (violations, infos). violations = non-ASCII without BOM (must be fixed);
    infos = pure-ASCII without BOM (allowed, listed so the count cannot silently drift).
    """
    violations: list[dict] = []
    infos: list[dict] = []
    for path in sorted(root.glob("*.ps1")):
        try:
            data = path.read_bytes()
        except OSError as exc:  # unreadable file is a violation, not a skip
            violations.append({"file": path.name, "error": str(exc), "non_ascii": -1})
            continue
        has_bom, non_ascii = inspect_bytes(data)
        if non_ascii > 0 and not has_bom:
            violations.append(
                {
                    "file": path.name,
                    "non_ascii": non_ascii,
                    "reason": "non-ASCII content without UTF-8 BOM",
                }
            )
        elif non_ascii == 0 and not has_bom:
            infos.append({"file": path.name, "non_ascii": 0})
    return violations, infos


def selftest() -> int:
    """Two-way fixtures: the detector must catch a false green.

    Without this, a scanner that always returns "no violations" (e.g. a broken glob
    or an inverted condition) would look perfectly healthy -- the exact failure mode
    this gate was written to prevent.
    """
    with tempfile.TemporaryDirectory() as tmp:
        d = Path(tmp)
        # fixture 1: non-ASCII without BOM -> must be flagged
        (d / "bad_ascii_no_bom.ps1").write_bytes("# comment \u4e2d\u6587\nWrite-Host 1\n".encode("utf-8"))
        # fixture 2: non-ASCII with BOM -> must pass
        (d / "good_ascii_bom.ps1").write_bytes(BOM + "# comment \u4e2d\u6587\n".encode("utf-8"))
        # fixture 3: pure ASCII without BOM -> allowed, INFO only
        (d / "ok_plain_ascii.ps1").write_bytes(b"# plain ascii\nWrite-Host 1\n")
        # fixture 4: empty file -> must not crash, not a violation
        (d / "empty.ps1").write_bytes(b"")

        violations, infos = scan_dir(d)
        flagged = {v["file"] for v in violations}
        info_files = {i["file"] for i in infos}

        problems = []
        if "bad_ascii_no_bom.ps1" not in flagged:
            problems.append("non-ASCII without BOM was NOT flagged (gate has no teeth)")
        if "good_ascii_bom.ps1" in flagged:
            problems.append("non-ASCII with BOM was wrongly flagged")
        if "ok_plain_ascii.ps1" in flagged:
            problems.append("pure-ASCII without BOM must be INFO, not a violation")
        if "ok_plain_ascii.ps1" not in info_files:
            problems.append("pure-ASCII without BOM missing from INFO ledger")
        if "empty.ps1" in flagged:
            problems.append("empty file must not be a violation")
        if len(flagged) != 1:
            problems.append(f"expected exactly 1 violation, got {sorted(flagged)}")

        if problems:
            for p in problems:
                print(f"FAIL  {p}")
            print("SELFTEST: FAILED")
            return 1
    print("SELFTEST: PASSED (4 fixtures: flag / allow / info / empty)")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default="scripts", help="directory holding *.ps1")
    parser.add_argument("--json", action="store_true", help="machine-readable output")
    parser.add_argument("--selftest", action="store_true", help="verify the detector itself")
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()

    root = Path(args.root)
    if not root.is_dir():
        print(f"[UNDECIDABLE] root not found: {root}")
        return 2

    violations, infos = scan_dir(root)
    total = len(list(root.glob("*.ps1")))
    if total == 0:
        print(f"[UNDECIDABLE] no .ps1 files under {root}")
        return 2

    if args.json:
        print(json.dumps(
            {"scanned": total, "violations": violations, "info_no_bom_ascii_only": infos},
            indent=2,
        ))
    else:
        print("== PowerShell UTF-8 BOM gate (ADR-198) ==")
        print(f"  scanned .ps1: {total}")
        for v in violations:
            print(f"  FAIL    {v['file']}: {v['reason']} (non_ascii={v['non_ascii']})")
        for i in infos:
            print(f"  INFO    {i['file']}: pure ASCII without BOM (allowed)")
        if violations:
            print(
                "\n  FIX: prepend UTF-8 BOM (EF BB BF) byte-wise; do not re-encode the "
                "body. Windows PowerShell 5.1 decodes BOM-less files via the ANSI "
                "codepage, which corrupts non-ASCII comments and shifts the parser."
            )
        print(
            f"\nRESULT: {'FAIL' if violations else 'OK'} "
            f"({total - len(violations)}/{total} files satisfy the rule)"
        )
    return 1 if violations else 0


if __name__ == "__main__":
    import gate_rc  # crash exits 2, never borrows 1 (F32/F33)
    sys.exit(gate_rc.run(main))
