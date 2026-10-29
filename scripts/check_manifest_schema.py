#!/usr/bin/env python3
"""G-138: gate_manifest.toml schema gate (ADR-228 3.6).

The manifest is the single source of truth for 130+ gates, but until now its
only structural check was run_gate_manifest.load(): six required keys, kind
and duplicate id. The v2 fields added for govd had no guard, and a typo in
any of them would either be silently ignored or crash the scheduler mid-run.

Exit codes: 0 valid / 1 schema finding / 2 cannot read or parse.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "scripts" / "gate_manifest.toml"
TASKS = ROOT / "scripts" / "govd" / "tasks.toml"
REGISTRY = ROOT / "scripts" / "tool_registry.toml"

REQUIRED = ("id", "spec_ref", "cmd", "expect", "kind", "note")
KNOWN_KEYS = {
    "id", "spec_ref", "cmd", "expect", "kind", "note", "test_kind",
    "timeout_s", "retry", "mutating", "needs", "profiles", "owner",
    "disk_gb", "ci_exempt", "ci_advisory", "source",
}
PROFILES = ("pr", "nightly", "release", "local")
EXES = ("{python}", "bash", "pwsh", "cargo", "git")
SOURCES = ("gate", "rust", "external", "diagnostic")
ID_RE = re.compile(r"^G-\d+$")
TASK_ID_RE = re.compile(r"^(R|E|X|D)-\d+$")
PROFILE_KEYS = {"include_heavy", "jobs", "allow_mutating", "reuse_ok"}
EXEC_KEYS = {"whitelist", "default_timeout_s", "heavy_timeout_s", "env_retry",
             "mutating_prefixes"}
ROLE_KEYS = {"profiles"}
# Only the release lane may ever allow mutating tasks: pr/nightly/local are the
# lanes a contributor or an automated PR run uses, and `--allow-mutating` is their
# only opt-in. Keeping the rule here means the invariant cannot be flipped by a
# one-line edit to policy.toml that nobody reads.
MUTATING_ALLOWED_PROFILES = ("release",)


def _is_int(value) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def load_gates(path: Path) -> list[dict]:
    with path.open("rb") as fh:
        doc = tomllib.load(fh)
    gates = doc.get("gate", [])
    if not isinstance(gates, list) or not gates:
        raise ValueError("no [[gate]] entries in %s" % path)
    return gates


def load_tasks(path: Path) -> list[dict]:
    with path.open("rb") as fh:
        doc = tomllib.load(fh)
    tasks = doc.get("task", [])
    if not isinstance(tasks, list):
        raise ValueError("task must be an array of tables in %s" % path)
    return tasks

def load_policy(path: Path) -> dict:
    with path.open("rb") as fh:
        return tomllib.load(fh)


def validate_policy(doc):
    """Validate govd's policy.toml against the ADR-228 §3.3-§3.5 invariants.

    WHY a gate for the policy file: profiles, the executable whitelist and the
    reuse/mutating switches are exactly the permissions model, and until now
    nothing checked them. A typo (`alow_mutating`) or a `reuse_ok = true` under
    `[profiles.release]` would silently weaken the scheduler while every gate
    stayed green -- the Spec forbids reuse in a release run outright.
    """
    problems = []

    def bad(code, msg):
        problems.append((code, msg))

    profiles = doc.get("profiles", {})
    exec_row = doc.get("exec", {})
    roles = doc.get("roles", {})
    if not isinstance(profiles, dict) or not profiles:
        bad("POLICY_EMPTY", "no [profiles.*] tables")
        return problems
    for name in PROFILES:
        if name not in profiles:
            bad("POLICY_PROFILE", "profile %r is missing from policy.toml" % name)
    for name, row in sorted(profiles.items()):
        if name not in PROFILES:
            bad("POLICY_PROFILE", "unknown profile %r (known: %s)" % (name, list(PROFILES)))
            continue
        if not isinstance(row, dict):
            bad("POLICY_TYPE", "profile %r must be a table" % name)
            continue
        for key in sorted(set(row) - PROFILE_KEYS):
            bad("POLICY_UNKNOWN_KEY", "profiles.%s: unknown key %r" % (name, key))
        for key in ("include_heavy", "allow_mutating", "reuse_ok"):
            if key in row and not isinstance(row[key], bool):
                bad("POLICY_TYPE", "profiles.%s.%s must be a bool" % (name, key))
        if "jobs" in row and (not _is_int(row["jobs"]) or row["jobs"] < 1):
            bad("POLICY_JOBS", "profiles.%s.jobs must be an int >= 1" % name)
        if row.get("reuse_ok") is True:
            bad("POLICY_REUSE", "profiles.%s.reuse_ok must stay false: reuse is a "
                                "per-run CLI opt-in, and a release verdict may never "
                                "be inherited (ADR-228 §3.4)" % name)
        if row.get("allow_mutating") is True and name not in MUTATING_ALLOWED_PROFILES:
            bad("POLICY_MUTATING", "profiles.%s.allow_mutating is only allowed for %s"
                % (name, list(MUTATING_ALLOWED_PROFILES)))

    if not isinstance(exec_row, dict) or not exec_row:
        bad("POLICY_EXEC", "no [exec] table")
    else:
        for key in sorted(set(exec_row) - EXEC_KEYS):
            bad("POLICY_UNKNOWN_KEY", "exec: unknown key %r" % key)
        whitelist = exec_row.get("whitelist")
        if not isinstance(whitelist, list) or not whitelist:
            bad("POLICY_EXEC", "exec.whitelist must be a non-empty array")
        else:
            for exe in whitelist:
                if exe not in EXES:
                    bad("POLICY_EXEC", "exec.whitelist: %r is not a known executable" % exe)
        for key in ("default_timeout_s", "heavy_timeout_s"):
            if key not in exec_row or not _is_int(exec_row[key]) or exec_row[key] <= 0:
                bad("POLICY_TIMEOUT", "exec.%s must be a positive int" % key)
        if "env_retry" in exec_row and (not _is_int(exec_row["env_retry"])
                                        or not 0 <= exec_row["env_retry"] <= 3):
            bad("POLICY_RETRY", "exec.env_retry must be 0..3 (environment-only)")
        prefixes = exec_row.get("mutating_prefixes", [])
        if not isinstance(prefixes, list) or not all(isinstance(p, str) and p for p in prefixes):
            bad("POLICY_TYPE", "exec.mutating_prefixes must be a list of strings")

    if not isinstance(roles, dict) or not roles:
        bad("POLICY_ROLE", "no [roles.*] tables")
    else:
        for name, row in sorted(roles.items()):
            if not isinstance(row, dict):
                bad("POLICY_TYPE", "roles.%s must be a table" % name)
                continue
            for key in sorted(set(row) - ROLE_KEYS):
                bad("POLICY_UNKNOWN_KEY", "roles.%s: unknown key %r" % (name, key))
            allowed = row.get("profiles")
            if not isinstance(allowed, list) or not allowed:
                bad("POLICY_ROLE", "roles.%s.profiles must be a non-empty array" % name)
                continue
            for p in allowed:
                if p not in PROFILES:
                    bad("POLICY_ROLE", "roles.%s: unknown profile %r" % (name, p))
                elif p not in profiles:
                    bad("POLICY_ROLE", "roles.%s grants %r which has no [profiles.%s] table"
                        % (name, p, p))
    return problems


def validate(gates, *, family="gate"):
    """Return [(code, message)]; an empty list means the manifest is valid."""
    problems = []

    def bad(code, msg):
        problems.append((code, msg))

    if not isinstance(gates, list) or not gates:
        bad("EMPTY", "no entries")
        return problems
    seen = set()
    ids = set()
    for i, g in enumerate(gates, 1):
        if not isinstance(g, dict):
            bad("TYPE", "entry %d is not a table" % i)
            continue
        gid = str(g.get("id", "?"))
        for key in REQUIRED:
            if key not in g:
                bad("REQUIRED", "%s: missing %s" % (gid, key))
        for key in ("id", "spec_ref", "cmd", "kind", "note"):
            if key in g and (not isinstance(g[key], str) or not g[key].strip()):
                bad("REQUIRED", "%s: %s must be a non-empty string" % (gid, key))
        if isinstance(g.get("id"), str):
            ids.add(g["id"])
            pattern = TASK_ID_RE if family == "task" else ID_RE
            if not pattern.match(g["id"]):
                bad("ID", "%s: id must match %s" % (gid, pattern.pattern))
            if g["id"] in seen:
                bad("DUP", "%s: duplicate id" % gid)
            seen.add(g["id"])
        else:
            bad("ID", "entry %d: id must be a string" % i)
        if "expect" in g and (not _is_int(g["expect"]) or g["expect"] not in (0, 1, 2)):
            bad("EXPECT", "%s: expect must be 0, 1 or 2" % gid)
        if g.get("kind") not in ("light", "heavy"):
            bad("KIND", "%s: kind must be light|heavy" % gid)
        head = str(g.get("cmd", "")).split()[:1]
        if head and head[0] not in EXES:
            bad("EXEC", "%s: executable %r is not whitelisted" % (gid, head[0]))
        for key in sorted(set(g) - KNOWN_KEYS):
            bad("UNKNOWN_KEY", "%s: unknown key %r" % (gid, key))
        for key in ("ci_exempt", "ci_advisory", "test_kind", "owner"):
            if key in g and not isinstance(g[key], str):
                bad("TYPE", "%s: %s must be a string" % (gid, key))
        if "timeout_s" in g and (not _is_int(g["timeout_s"]) or g["timeout_s"] <= 0):
            bad("TIMEOUT", "%s: timeout_s must be a positive int" % gid)
        if "retry" in g and (not _is_int(g["retry"]) or not -1 <= g["retry"] <= 3):
            bad("RETRY", "%s: retry must be -1..3" % gid)
        if "mutating" in g and not isinstance(g["mutating"], bool):
            bad("MUTATING", "%s: mutating must be bool" % gid)
        if "source" in g and g["source"] not in SOURCES:
            bad("SOURCE", "%s: source must be one of %s" % (gid, list(SOURCES)))
        if "disk_gb" in g:
            v = g["disk_gb"]
            if isinstance(v, bool) or not isinstance(v, (int, float)) or v < 0:
                bad("DISK", "%s: disk_gb must be a number >= 0" % gid)

    if family == "task":
        for g in gates:
            if isinstance(g, dict) and not g.get("source"):
                bad("SOURCE", "%s: tasks.toml rows need a source" % g.get("id", "?"))
    for g in gates:
        if not isinstance(g, dict):
            continue
        gid = str(g.get("id", "?"))
        if "needs" in g:
            needs = g["needs"]
            if not isinstance(needs, list) or not all(isinstance(x, str) for x in needs):
                bad("NEEDS_TYPE", "%s: needs must be a list of strings" % gid)
            else:
                for n in needs:
                    if n not in ids:
                        bad("NEEDS", "%s: needs unknown task %r" % (gid, n))
        if "profiles" in g:
            profiles = g["profiles"]
            if not isinstance(profiles, list) or not all(isinstance(x, str) for x in profiles):
                bad("PROFILE_TYPE", "%s: profiles must be a list of strings" % gid)
            else:
                for p in profiles:
                    if p not in PROFILES:
                        bad("PROFILE", "%s: unknown profile %r" % (gid, p))
                if g.get("mutating") is True and any(p in ("pr", "nightly") for p in profiles):
                    bad("MUTATING_PROFILE",
                        "%s: mutating task must not be selected by pr/nightly" % gid)

    graph = {}
    for g in gates:
        if isinstance(g, dict) and isinstance(g.get("id"), str):
            graph[g["id"]] = [n for n in g.get("needs", [])
                              if isinstance(n, str) and n in ids]
    state = {}

    def visit(node, stack):
        if state.get(node) == 1:
            bad("CYCLE", "needs cycle: %s" % " -> ".join(stack + [node]))
            return
        if state.get(node) == 2:
            return
        state[node] = 1
        for nxt in graph.get(node, []):
            visit(nxt, stack + [node])
        state[node] = 2

    for node in graph:
        visit(node, [])
    return problems

def _gate(**kw):
    row = {"id": "G-100", "spec_ref": "x", "cmd": "{python} scripts/a.py",
           "expect": 0, "kind": "light", "note": "n", "test_kind": "cross_crate"}
    row.update(kw)
    return row


def selftest() -> int:
    """One positive fixture and one fixture per failure family (ADR-198 discipline)."""
    import tempfile

    checks = []

    def want(name, cond, detail=""):
        checks.append((name, bool(cond), str(detail)))

    def codes(rows, family="gate"):
        return {c for c, _ in validate(rows, family=family)}

    want("valid-clean", validate([_gate()]) == [])
    cases = [
        ("duplicate-id", [_gate(), _gate()], "DUP"),
        ("missing-note", [{k: v for k, v in _gate().items() if k != "note"}], "REQUIRED"),
        ("bad-expect", [_gate(expect=3)], "EXPECT"),
        ("bad-kind", [_gate(kind="slow")], "KIND"),
        ("bad-timeout", [_gate(timeout_s=0)], "TIMEOUT"),
        ("bad-retry", [_gate(retry=4)], "RETRY"),
        ("bad-mutating", [_gate(mutating="yes")], "MUTATING"),
        ("bad-profile", [_gate(profiles=["weekly"])], "PROFILE"),
        ("mutating-in-pr", [_gate(mutating=True, profiles=["pr"])], "MUTATING_PROFILE"),
        ("unknown-need", [_gate(needs=["G-999"])], "NEEDS"),
        ("cycle", [_gate(id="G-100", needs=["G-101"]),
                   _gate(id="G-101", needs=["G-100"])], "CYCLE"),
        ("bad-disk", [_gate(disk_gb=-1)], "DISK"),
        ("bad-exec", [_gate(cmd="python3 scripts/a.py")], "EXEC"),
        ("unknown-key", [_gate(ownerx="me")], "UNKNOWN_KEY"),
        ("bad-id", [_gate(id="X-1")], "ID"),
        ("empty", [], "EMPTY"),
        ("bool-expect", [_gate(expect=True)], "EXPECT"),
        ("bad-source", [_gate(source="bogus")], "SOURCE"),
    ]
    for name, rows, code in cases:
        want(name, code in codes(rows), codes(rows))
    want("task-needs-source", "SOURCE" in codes([_gate()], family="task"))

    def pcodes(doc):
        return {c for c, _ in validate_policy(doc)}

    good_policy = {
        "profiles": {p: {"include_heavy": p != "pr", "jobs": 2,
                        "allow_mutating": p == "release", "reuse_ok": False}
                     for p in PROFILES},
        "exec": {"whitelist": list(EXES), "default_timeout_s": 300,
                 "heavy_timeout_s": 1800, "env_retry": 1,
                 "mutating_prefixes": ["tmp/"]},
        "roles": {"local-dev": {"profiles": list(PROFILES)}, "ci": {"profiles": ["pr"]}},
    }
    want("policy-valid-clean", validate_policy(good_policy) == [])
    policy_cases = [
        ("policy-release-reuse",
         lambda d: d["profiles"].__setitem__("release", {**d["profiles"]["release"], "reuse_ok": True}),
         "POLICY_REUSE"),
        ("policy-mutating-in-pr",
         lambda d: d["profiles"].__setitem__("pr", {**d["profiles"]["pr"], "allow_mutating": True}),
         "POLICY_MUTATING"),
        ("policy-typo-key",
         lambda d: d["profiles"].__setitem__("local", {**d["profiles"]["local"], "alow_mutating": False}),
         "POLICY_UNKNOWN_KEY"),
        ("policy-empty-whitelist",
         lambda d: d["exec"].__setitem__("whitelist", []), "POLICY_EXEC"),
        ("policy-unknown-exe",
         lambda d: d["exec"].__setitem__("whitelist", ["curl"]), "POLICY_EXEC"),
        ("policy-missing-profile",
         lambda d: d["profiles"].pop("nightly", None), "POLICY_PROFILE"),
        ("policy-zero-timeout",
         lambda d: d["exec"].__setitem__("default_timeout_s", 0), "POLICY_TIMEOUT"),
        ("policy-jobs-zero",
         lambda d: d["profiles"].__setitem__("pr", {**d["profiles"]["pr"], "jobs": 0}),
         "POLICY_JOBS"),
        ("policy-role-unknown-profile",
         lambda d: d["roles"].__setitem__("bot", {"profiles": ["weekly"]}), "POLICY_ROLE"),
        ("policy-no-exec",
         lambda d: d.pop("exec", None), "POLICY_EXEC"),
    ]
    for name, mutate, code in policy_cases:
        import copy as _copy
        doc = _copy.deepcopy(good_policy)
        mutate(doc)
        want(name, code in pcodes(doc), pcodes(doc))

    with tempfile.TemporaryDirectory() as tmp:
        good = Path(tmp) / "good.toml"
        good.write_text('[[gate]]\nid = "G-100"\nspec_ref = "x"\n'
                        'cmd = "{python} scripts/a.py"\nexpect = 0\n'
                        'kind = "light"\nnote = "n"\n', encoding="utf-8")
        want("toml-roundtrip", len(load_gates(good)) == 1)
        bad = Path(tmp) / "bad.toml"
        bad.write_text("[[gate]\nid = 'G-100'\n", encoding="utf-8")
        try:
            load_gates(bad)
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
    p = argparse.ArgumentParser(description="G-138 gate manifest schema")
    p.add_argument("--root", default=str(ROOT))
    p.add_argument("--json", action="store_true")
    p.add_argument("--selftest", action="store_true")
    args = p.parse_args(argv)
    if args.selftest:
        return selftest()
    root = Path(args.root)
    try:
        gates = load_gates(root / "scripts" / "gate_manifest.toml")
        problems = validate(gates)
        tasks = []
        tasks_path = root / "scripts" / "govd" / "tasks.toml"
        if tasks_path.is_file():
            tasks = load_tasks(tasks_path)
            problems += [("TASK:" + c, m) for c, m in validate(tasks, family="task")]
        rows = []
        reg_path = root / "scripts" / "tool_registry.toml"
        if reg_path.is_file():
            sys.path.insert(0, str(root / "scripts"))
            from govd.manifest import load_registry, validate_registry
            rows = list(load_registry(reg_path))
            problems += [("REGISTRY", m) for m in validate_registry(rows)]
        policy_path = root / "scripts" / "govd" / "policy.toml"
        policy_profiles = 0
        if policy_path.is_file():
            policy_doc = load_policy(policy_path)
            policy_profiles = len(policy_doc.get("profiles", {}))
            problems += [("POLICY", m) for m in validate_policy(policy_doc)]
    except (OSError, ValueError, tomllib.TOMLDecodeError) as exc:
        print("FAIL  cannot read manifest: %s: %s" % (type(exc).__name__, exc))
        return 2
    light = sum(1 for g in gates if g.get("kind") == "light")
    heavy = sum(1 for g in gates if g.get("kind") == "heavy")
    print("manifest=%s gates=%d light=%d heavy=%d tasks=%d registry=%d policy_profiles=%d"
          % (root / "scripts" / "gate_manifest.toml", len(gates), light, heavy,
             len(tasks), len(rows), policy_profiles))
    if args.json:
        print(json.dumps([{"code": c, "message": m} for c, m in problems],
                         ensure_ascii=False, indent=2))
    else:
        for code, message in problems:
            print("  [%s] %s" % (code, message))
    if problems:
        print("[FAIL] %d schema finding(s)" % len(problems))
        return 1
    print("[OK] manifest schema valid (%d gates)" % len(gates))
    return 0


if __name__ == "__main__":
    import gate_rc

    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
