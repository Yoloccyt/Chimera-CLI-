#!/usr/bin/env python3
"""G-137: CI workflow steps vs gate_manifest.toml parity (ADR-228 §3.6).

Before this gate existed the repository had two execution surfaces: 136 gates
in gate_manifest.toml and 108 hand-written `python3 scripts/*.py` steps in
ci.yml. Nothing compared them, so a gate could silently stop running in CI and
a CI-only script could bypass the manifest (PYG-F01).

Directions:
  A. CI -> manifest: every scripts/*.{py,sh,ps1} named by a workflow must be
     either a registered gate script (exact path or same-stem twin, because
     .sh/.ps1 are platform twins) or an explicit non-gate helper with a reason.
  B. manifest -> CI: every non-ci_exempt gate whose cmd names a script must
     have that script (or its twin) named by a workflow. A gate that only runs
     on someone's laptop is not a gate.

Exit codes: 0 clean / 1 parity finding / 2 cannot read or parse inputs.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT_RE = re.compile(r"scripts/[A-Za-z0-9_.\-/]+\.(?:py|sh|ps1)")
GATE_SCRIPT_RE = re.compile(r"scripts/([A-Za-z0-9_.\-/]+\.(?:py|sh|ps1))")

# A CI script that is deliberately not a gate must carry a reason here; an
# unexplained extra step is a finding, not a silent pass (ADR-198 §3.4).
HELPERS = {
    "scripts/audit_fnlen.py": "invoked by scripts/test_fnlen_smoke.sh (G-48); helper, not a gate",
}


def load_manifest(path: Path) -> list[dict]:
    with path.open("rb") as fh:
        doc = tomllib.load(fh)
    gates = doc.get("gate", [])
    if not isinstance(gates, list) or not gates:
        raise ValueError("no [[gate]] entries in %s" % path)
    return gates

def scan_workflows(wf_dir: Path):
    """Return (files, steps); a step is (workflow, line, script path)."""
    files = sorted(wf_dir.glob("*.yml")) + sorted(wf_dir.glob("*.yaml"))
    steps = []
    for path in files:
        text = path.read_text(encoding="utf-8", errors="replace")
        for m in SCRIPT_RE.finditer(text):
            line = text.count("\n", 0, m.start()) + 1
            steps.append((path.name, line, m.group(0)))
    return files, steps


def evaluate(gates, steps, helpers=None):
    """Pure parity computation; both directions plus the helper allowlist."""
    helpers = HELPERS if helpers is None else helpers
    ci_scripts = {s for _, _, s in steps}
    gate_scripts: dict[str, list[dict]] = {}
    for g in gates:
        m = GATE_SCRIPT_RE.search(str(g.get("cmd", "")))
        if m:
            gate_scripts.setdefault("scripts/" + m.group(1), []).append(g)
    gate_stems = {Path(s).stem for s in gate_scripts}
    ci_stems = {Path(s).stem for s in ci_scripts}
    matched = sorted(s for s in ci_scripts
                     if s in gate_scripts or Path(s).stem in gate_stems)
    allowlisted = sorted(s for s in ci_scripts
                         if s not in matched and s in helpers)
    unmatched = sorted(s for s in ci_scripts
                       if s not in matched and s not in helpers)
    missing = []
    for script, rows in sorted(gate_scripts.items()):
        if script in ci_scripts or Path(script).stem in ci_stems:
            continue
        for g in rows:
            if g.get("ci_exempt"):
                continue
            missing.append({"gate": g.get("id"), "script": script})
    return {
        "workflows": len({w for w, _, _ in steps}),
        "ci_refs": len(steps),
        "ci_steps": len(ci_scripts),
        "manifest_gates": len(gates),
        "matched": matched,
        "allowlisted": allowlisted,
        "unmatched": unmatched,
        "gate_missing": missing,
    }

def selftest() -> int:
    """Positive + negative fixtures for both directions, no production files."""
    import tempfile

    checks = []

    def want(name, cond, detail=""):
        checks.append((name, bool(cond), str(detail)))

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        wf = root / ".github" / "workflows"
        wf.mkdir(parents=True)
        gates = [
            {"id": "G-01", "cmd": "{python} scripts/a.py"},
            {"id": "G-02", "cmd": "{python} scripts/b.py", "ci_exempt": "local only"},
            {"id": "G-03", "cmd": "bash scripts/c.sh"},
            {"id": "G-04", "cmd": "cargo fmt --all -- --check"},
        ]
        (wf / "ci.yml").write_text(
            "steps:\n  - run: python3 scripts/a.py\n"
            "  - run: bash scripts/c.py\n"
            "  - run: python3 scripts/helper.py\n",
            encoding="utf-8")
        files, steps = scan_workflows(wf)
        res = evaluate(gates, steps, {"scripts/helper.py": "helper"})
        want("workflows", res["workflows"] == 1, res["workflows"])
        want("ci_steps", res["ci_steps"] == 3, res["ci_steps"])
        want("exact-match", "scripts/a.py" in res["matched"])
        want("twin-match", "scripts/c.py" in res["matched"], res["matched"])
        want("helper-allowlisted", res["allowlisted"] == ["scripts/helper.py"],
             res["allowlisted"])
        want("clean-unmatched", res["unmatched"] == [], res["unmatched"])
        want("clean-gate-missing", res["gate_missing"] == [], res["gate_missing"])
        want("empty-cmd-not-missing", all(x["gate"] != "G-04" for x in res["gate_missing"]))

        (wf / "ci.yml").write_text(
            "steps:\n  - run: python3 scripts/orphan.py\n", encoding="utf-8")
        _, steps2 = scan_workflows(wf)
        res2 = evaluate(gates, steps2, {"scripts/helper.py": "helper"})
        want("orphan-unmatched", res2["unmatched"] == ["scripts/orphan.py"],
             res2["unmatched"])
        want("missing-a", any(x["gate"] == "G-01" for x in res2["gate_missing"]),
             res2["gate_missing"])
        want("missing-c", any(x["gate"] == "G-03" for x in res2["gate_missing"]))
        want("exempt-not-missing",
             all(x["gate"] != "G-02" for x in res2["gate_missing"]))

        good = root / "good.toml"
        good.write_text('[[gate]]\nid = "G-01"\ncmd = "{python} scripts/a.py"\n',
                        encoding="utf-8")
        want("manifest-roundtrip", len(load_manifest(good)) == 1)
        bad = root / "bad.toml"
        bad.write_text("[[gate]\nid = 'G-01'\n", encoding="utf-8")
        try:
            load_manifest(bad)
            want("malformed-raises", False)
        except tomllib.TOMLDecodeError:
            want("malformed-raises", True)

    failed = [c for c in checks if not c[1]]
    for name, _, detail in failed:
        print("FAIL  %s %s" % (name, detail))
    if failed:
        print("SELFTEST: FAILED (%d/%d)" % (len(failed), len(checks)))
        return 1
    print("SELFTEST: ALL ASSERTIONS HELD (%d assertions)" % len(checks))
    return 0


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description="G-137 CI/manifest parity")
    p.add_argument("--root", default=str(ROOT))
    p.add_argument("--json", action="store_true")
    p.add_argument("--selftest", action="store_true")
    args = p.parse_args(argv)
    if args.selftest:
        return selftest()
    root = Path(args.root)
    try:
        gates = load_manifest(root / "scripts" / "gate_manifest.toml")
        files, steps = scan_workflows(root / ".github" / "workflows")
    except (OSError, ValueError, tomllib.TOMLDecodeError) as exc:
        print("FAIL  cannot read inputs: %s: %s" % (type(exc).__name__, exc))
        return 2
    if not files:
        print("FAIL  no workflow files under %s" % (root / ".github" / "workflows"))
        return 2
    result = evaluate(gates, steps)
    if args.json:
        print(json.dumps(result, ensure_ascii=False, indent=2))
    else:
        print("workflows=%(workflows)d ci_refs=%(ci_refs)d ci_steps=%(ci_steps)d "
              "manifest_gates=%(manifest_gates)d matched=%(matched)d "
              "allowlisted=%(allowlisted)d unmatched=%(unmatched)d "
              "gate_missing=%(gate_missing)d" % {
                  "workflows": result["workflows"], "ci_refs": result["ci_refs"],
                  "ci_steps": result["ci_steps"],
                  "manifest_gates": result["manifest_gates"],
                  "matched": len(result["matched"]),
                  "allowlisted": len(result["allowlisted"]),
                  "unmatched": len(result["unmatched"]),
                  "gate_missing": len(result["gate_missing"])})
        for s in result["unmatched"]:
            print("  [UNMATCHED] %s (not a gate and not an allowlisted helper)" % s)
        for row in result["gate_missing"]:
            print("  [GATE-MISSING] %s -> %s (non-exempt gate absent from CI)"
                  % (row["gate"], row["script"]))
    bad = len(result["unmatched"]) + len(result["gate_missing"])
    print("[OK] CI and manifest agree" if not bad
          else "[FAIL] %d parity finding(s)" % bad)
    return 0 if not bad else 1


if __name__ == "__main__":
    import gate_rc

    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
