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

Manifest-driven execution surface (TC-C, 2026-11-19)
----------------------------------------------------
`ci.yml` no longer enumerates the zero-cargo gates step by step: one step runs
`scripts/run_gate_manifest.py --ci --jobs N`, and the manifest is what defines
which gates that covers. Judging direction B by "the script name is literally
present in the workflow text" would therefore report every gate as MISSING the
day that surface lands -- a false red, not a finding.

So direction B gains one extra source of wiring: a workflow that *runs* the
manifest driver outside a continue-on-error block covers the gates the pr
profile selects. The covered set is derived conservatively from the manifest
itself (light + no `profiles` + not `mutating`), mirroring govd/model.py::
Task.selected_by. Anything we cannot prove from the manifest alone (heavy,
profile-restricted, mutating) still requires literal wiring, so this cannot
hide a gate that the driver would not actually run.

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
    # Named (not executed) by the observation-period comments in slo.yml /
    # perf_redline.yml / stress.yml: it judges `continue-on-error` conversion
    # conditions from `gh run list`, so it is an ops/收口 checker rather than a
    # per-PR gate -- running it in CI would need a token scope the PR lane
    # deliberately does not hold.
    "scripts/check_coe_observability.py":
        "COE observation-period checker (gh run list); ops/toolchain lane, not a PR gate",
}

# The manifest driver (see module docstring). A workflow running it *outside* a
# continue-on-error block covers every gate the pr profile selects.
DRIVER = "scripts/run_gate_manifest.py"
COE_RE = re.compile(r"^\s*continue-on-error:\s*true\s*$")
RUN_LINE_RE = re.compile(r"(\brun:|\bbash |\bpwsh |\bpython3? |\bpy |\bsh )")


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


def driver_covered(gates) -> set[str]:
    """Scripts the manifest driver is guaranteed to run for the pr profile.

    Conservative on purpose: `light`, no `profiles` restriction and not
    `mutating` is exactly what govd's pr profile selects without extra flags
    (govd/model.py::Task.selected_by is the authority). A row we cannot prove
    from the manifest stays out, so its gate still needs literal wiring -- a
    fail-closed default rather than an optimistic one.
    """
    covered = set()
    for g in gates:
        if g.get("kind") != "light":
            continue
        if g.get("profiles") or g.get("mutating"):
            continue
        m = GATE_SCRIPT_RE.search(str(g.get("cmd", "")))
        if m:
            covered.add("scripts/" + m.group(1))
    return covered


def driver_runs_blocking(wf_dir: Path) -> bool:
    """True when a workflow *executes* the driver outside a COE block.

    `--selftest`-only lines do not count: the selftest proves the runner's
    classifier works, it does not run any gate. Counting it would let a workflow
    that never executes the manifest look fully wired.
    """
    try:
        files = sorted(wf_dir.glob("*.yml")) + sorted(wf_dir.glob("*.yaml"))
    except OSError:
        return False
    for path in files:
        try:
            lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            continue
        coe_indent = None
        for line in lines:
            if COE_RE.match(line):
                coe_indent = len(line) - len(line.lstrip())
                continue
            if coe_indent is not None:
                ind = len(line) - len(line.lstrip())
                if line.strip() and ind < coe_indent:
                    coe_indent = None
            if DRIVER in line and "--selftest" not in line and RUN_LINE_RE.search(line):
                if coe_indent is None:
                    return True
    return False


def evaluate(gates, steps, helpers=None, driver=False):
    """Pure parity computation; both directions plus the helper allowlist.

    `driver=True` adds the manifest-driven coverage described in the module
    docstring (direction B only -- direction A still judges the driver path
    itself against the manifest).
    """
    helpers = HELPERS if helpers is None else helpers
    ci_scripts = {s for _, _, s in steps}
    gate_scripts: dict[str, list[dict]] = {}
    for g in gates:
        m = GATE_SCRIPT_RE.search(str(g.get("cmd", "")))
        if m:
            gate_scripts.setdefault("scripts/" + m.group(1), []).append(g)
    gate_stems = {Path(s).stem for s in gate_scripts}
    ci_stems = {Path(s).stem for s in ci_scripts}
    covered_stems = {Path(s).stem for s in driver_covered(gates)} if driver else set()
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
        if Path(script).stem in covered_stems:
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
        "driver_covered": len(covered_stems),
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

        # ---- manifest-driven driver (TC-C): what it covers, and what it must
        # ---- NOT cover (heavy / mutating / profile-restricted stay literal).
        drv_gates = [
            {"id": "G-09", "cmd": "{python} scripts/run_gate_manifest.py --selftest",
             "kind": "light"},
            {"id": "G-10", "cmd": "{python} scripts/lone.py", "kind": "light"},
            {"id": "G-11", "cmd": "{python} scripts/hv.py", "kind": "heavy"},
            {"id": "G-12", "cmd": "{python} scripts/mut.py", "kind": "light",
             "mutating": True},
            {"id": "G-13", "cmd": "{python} scripts/night.py", "kind": "light",
             "profiles": ["nightly"]},
        ]
        drv_wf = root / "drv" / ".github" / "workflows"
        drv_wf.mkdir(parents=True)
        (drv_wf / "ci.yml").write_text(
            "jobs:\n  g:\n    steps:\n"
            "      - run: python3 scripts/run_gate_manifest.py --ci --jobs 4\n",
            encoding="utf-8")
        _, drv_steps = scan_workflows(drv_wf)
        drv_on = evaluate(drv_gates, drv_steps, {}, driver=True)
        drv_off = evaluate(drv_gates, drv_steps, {}, driver=False)
        want("driver-line-is-matched", drv_on["unmatched"] == [], drv_on["unmatched"])
        want("driver-covers-light-gate",
             all(x["gate"] != "G-10" for x in drv_on["gate_missing"]),
             drv_on["gate_missing"])
        want("driver-does-not-cover-heavy",
             any(x["gate"] == "G-11" for x in drv_on["gate_missing"]),
             drv_on["gate_missing"])
        want("driver-does-not-cover-mutating",
             any(x["gate"] == "G-12" for x in drv_on["gate_missing"]))
        want("driver-does-not-cover-profile-restricted",
             any(x["gate"] == "G-13" for x in drv_on["gate_missing"]))
        want("without-driver-flag-light-gate-is-missing",
             any(x["gate"] == "G-10" for x in drv_off["gate_missing"]),
             drv_off["gate_missing"])
        want("driver-detected-blocking", driver_runs_blocking(drv_wf) is True)

        # A driver under continue-on-error, or a `--selftest`-only line, is NOT
        # coverage: the former runs the gates non-blocking, the latter runs none.
        (drv_wf / "ci.yml").write_text(
            "jobs:\n  g:\n    continue-on-error: true\n    steps:\n"
            "      - run: python3 scripts/run_gate_manifest.py --ci\n",
            encoding="utf-8")
        want("driver-under-coe-is-not-coverage",
             driver_runs_blocking(drv_wf) is False)
        (drv_wf / "ci.yml").write_text(
            "jobs:\n  g:\n    steps:\n"
            "      - run: python3 scripts/run_gate_manifest.py --selftest\n",
            encoding="utf-8")
        want("driver-selftest-only-is-not-coverage",
             driver_runs_blocking(drv_wf) is False)
        (drv_wf / "ci.yml").write_text(
            "jobs:\n  g:\n    steps:\n      - run: echo hi\n", encoding="utf-8")
        want("no-driver-line-means-no-coverage",
             driver_runs_blocking(drv_wf) is False)

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
    result = evaluate(gates, steps,
                      driver=driver_runs_blocking(root / ".github" / "workflows"))
    if args.json:
        print(json.dumps(result, ensure_ascii=False, indent=2))
    else:
        print("workflows=%(workflows)d ci_refs=%(ci_refs)d ci_steps=%(ci_steps)d "
              "manifest_gates=%(manifest_gates)d driver_covered=%(driver_covered)d "
              "matched=%(matched)d "
              "allowlisted=%(allowlisted)d unmatched=%(unmatched)d "
              "gate_missing=%(gate_missing)d" % {
                  "workflows": result["workflows"], "ci_refs": result["ci_refs"],
                  "ci_steps": result["ci_steps"],
                  "manifest_gates": result["manifest_gates"],
                  "driver_covered": result["driver_covered"],
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
