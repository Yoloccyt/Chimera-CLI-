#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L0 sub-layer direction gate (G-125, ADR-222 D1/D3).

Why this gate exists
--------------------
ADR-222 splits L0 into two sub-layers over the SAME machine layer numbers:
  0A = L0-A pure contracts   (nexus-contracts + 6 sibling crates, later waves)
  0B = L0-B core foundation  (nexus-core, event-bus, nexus-telemetry, later wave)
Renumbering L2-L10 was rejected because 116+ historical ADRs cite "L4 seccore"
etc.; falsifying that evidence chain costs more than the vocabulary buys.

Two directions are red:
  PURITY-LEAK  an L0-A crate declares a dependency on an L0-B crate
  UPWARD       an L0-B crate declares a dependency on a crate at layer >= 2

Undecidable (rc=2), never a silent pass:
  * source file missing or an anchor not found
  * $subLayerMap carries a duplicate key (PowerShell refuses to parse the whole
    table; a dict build would collapse the repeat and certify a dead script)
  * .sh and .ps1 sub-layer tables disagree (two sources, one truth)
  * sub-layer table does not cover exactly the layer-0/1 crate set
  * dependency parse below the floor (a blind parser reports "clean" exactly
    like a compliant tree -- the failure shape this repo keeps paying for)
  * tomllib unavailable (needs Python 3.11+)

Axis separation: G-45/G-46 own layer-map parity; this gate owns DIRECTION only.
It reads layer numbers from the .sh twin (the CI-called one) and never restates
them as constants.

Modes / exit codes
   (default)   judge the real repo   0 = clean, 1 = PURITY-LEAK / UPWARD
   --selftest  planted fixtures      0 = all assertions held, 1 = not
   usage / undecidable               2
Output is pure ASCII (project script convention).
"""

import os
import re
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

sys.path.insert(0, HERE)
import gate_rc  # noqa: E402  (crash -> rc 2, never borrow 1)
import l0_roster  # noqa: E402  (single source for the L0-A roster, ADR-222 D2)

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - guarded at every entry point
    tomllib = None

SH_FILE = os.path.join(ROOT, "scripts", "check_dependency_rules.sh")
PS1_FILE = os.path.join(ROOT, "scripts", "check_dependency_rules.ps1")
CRATES_DIR = os.path.join(ROOT, "crates")

L0A = "0A"
L0B = "0B"

# Measured 2026-10-19: L0-A=1, L0-B=2, internal L0 edges=3 (nexus-core->event-bus,
# nexus-core->nexus-contracts, event-bus->nexus-contracts). Floors are deliberately
# low: they exist to catch a blind parser, not to pin the expected size.
CRATE_FLOOR = 2
EDGE_FLOOR = 1

SH_LAYER_RE = re.compile(r"layer_of\(\)\s*\{\s*case\s+\"\$1\"\s+in(.*?)esac", re.S)
SH_SUB_RE = re.compile(r"sublayer_of\(\)\s*\{\s*case\s+\"\$1\"\s+in(.*?)esac", re.S)
SH_ARM_RE = re.compile(r"\s*([a-z0-9|-]+)\)\s*echo\s+(\d+)\s*;;")
SH_SUB_ARM_RE = re.compile(r"\s*([a-z0-9|-]+)\)\s*echo\s+\"(0[AB])\"\s*;;")
PS1_SUB_RE = re.compile(r"\$subLayerMap\s*=\s*@\{(.*?)\n\}", re.S)
PS1_PAIR_RE = re.compile(r"'([a-z0-9-]+)'\s*=\s*'(0[AB])'")


def read_text(path):
    """Read a source file with CRLF normalized (the worktree is CRLF on Windows)."""
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        return fh.read().replace("\r\n", "\n")


def _strip_comment(line):
    return line.split("#", 1)[0]


def parse_sh_layer(text):
    """crate -> numeric layer from `layer_of() { case "$1" in ... esac }`."""
    m = SH_LAYER_RE.search(text)
    if not m:
        return None
    out = {}
    for line in m.group(1).splitlines():
        mm = SH_ARM_RE.match(_strip_comment(line))
        if mm:
            for name in mm.group(1).split("|"):
                out[name.strip()] = int(mm.group(2))
    return out


def parse_sh_sublayer(text):
    """crate -> '0A'/'0B' from `sublayer_of() { case "$1" in ... esac }`."""
    m = SH_SUB_RE.search(text)
    if not m:
        return None
    out = {}
    for line in m.group(1).splitlines():
        mm = SH_SUB_ARM_RE.match(_strip_comment(line))
        if mm:
            for name in mm.group(1).split("|"):
                out[name.strip()] = mm.group(2)
    return out


def parse_ps1_sublayer(text):
    """crate -> '0A'/'0B' from `$subLayerMap = @{ 'name' = '0A'; ... }`."""
    m = PS1_SUB_RE.search(text)
    if not m:
        return None
    out = {}
    for line in m.group(1).splitlines():
        line = _strip_comment(line).strip()
        if not line:
            continue
        for name, sub in PS1_PAIR_RE.findall(line):
            out[name] = sub
    return out


def ps1_duplicate_keys(text):
    """Names written twice into `$subLayerMap`, sorted (empty when clean).

    PowerShell refuses to parse a hash literal carrying a duplicate key, so a
    table in that shape is not something this gate may reason about -- yet
    building a dict collapses the repeat silently. Measured 2026-10-20: exactly
    this shape made the local iron-law script a ParserError while every gate
    stayed green, because CI runs the .sh twin and the parity parser was dict-
    shaped. Axis note: G-45/G-46 owns the numeric $layerMap duplicates; this
    one owns $subLayerMap, the table this gate actually reads.
    """
    m = PS1_SUB_RE.search(text)
    if not m:
        return []
    seen = set()
    dups = set()
    for line in m.group(1).splitlines():
        for name, _sub in PS1_PAIR_RE.findall(_strip_comment(line)):
            if name in seen:
                dups.add(name)
            seen.add(name)
    return sorted(dups)


def validate_membership(layers, sh_sub, ps1_sub, roster=None):
    """Cross-source checks that are undecidable (rc 2), not verdicts (rc 1)."""
    errs = []
    if sh_sub != ps1_sub:
        diff_sh = {k: v for k, v in sh_sub.items() if ps1_sub.get(k) != v}
        diff_ps = {k: v for k, v in ps1_sub.items() if sh_sub.get(k) != v}
        errs.append("[SUBLayer-DRIFT] .sh vs .ps1 differ: sh_only=%r ps1_only=%r"
                    % (diff_sh, diff_ps))
    expected = {c for c, n in layers.items() if n in (0, 1)}
    got = set(sh_sub)
    if expected != got:
        errs.append("[MAP] layer-0/1 crates != sub-layer keys: missing=%r extra=%r"
                    % (sorted(expected - got), sorted(got - expected)))
    bad = sorted({v for v in sh_sub.values() if v not in (L0A, L0B)})
    if bad:
        errs.append("[MAP] unknown sub-layer value(s): %r" % bad)
    if roster is not None:
        # ADR-222 D2: the five L0-A judges read scripts/l0_roster.py, so a crate
        # added to the layer map but not to the roster (or the reverse) would
        # silently shrink their jurisdiction. Refuse to judge instead.
        zero_a = tuple(sorted(c for c, s in sh_sub.items() if s == L0A))
        if zero_a != tuple(sorted(roster)):
            errs.append("[ROSTER] sub-layer 0A set %r != l0_roster.L0A_DIR_NAMES %r"
                        % (list(zero_a), list(sorted(roster))))
    return errs


def crate_deps(crate, layers, crates_root):
    """Workspace-internal dependencies declared by one crate (dev-deps excluded)."""
    manifest = os.path.join(crates_root, crate, "Cargo.toml")
    with open(manifest, "rb") as fh:
        data = tomllib.load(fh)
    names = set()
    blocks = [data.get("dependencies", {}), data.get("build-dependencies", {})]
    for target in data.get("target", {}).values():
        if isinstance(target, dict):
            blocks.append(target.get("dependencies", {}))
    for block in blocks:
        for name in block:
            if name in layers:
                names.add(name)
    return names


def build_facts(sh_text, ps1_text, crates_root, roster=None):
    """Pure assembly step; every failure becomes an `undecidable` entry."""
    dups = ps1_duplicate_keys(ps1_text)
    if dups:
        return {"undecidable": [
            "[TOOL-ERROR] $subLayerMap has duplicate key(s) '%s' -- PowerShell "
            "will not parse that table, so the sub-layer truth is undecidable "
            "(not clean)" % "', '".join(dups)]}
    layers = parse_sh_layer(sh_text)
    sh_sub = parse_sh_sublayer(sh_text)
    ps1_sub = parse_ps1_sublayer(ps1_text)
    und = []
    for label, value in (("layer_of()", layers),
                         ("sublayer_of()", sh_sub),
                         ("$subLayerMap", ps1_sub)):
        if value is None:
            und.append("[TOOL-ERROR] %s not found or unparseable" % label)
    if und:
        return {"undecidable": und}
    und = validate_membership(layers, sh_sub, ps1_sub, roster)
    if und:
        return {"undecidable": und}
    l0a = {c for c, s in sh_sub.items() if s == L0A}
    l0b = {c for c, s in sh_sub.items() if s == L0B}
    try:
        edges = {c: crate_deps(c, layers, crates_root) for c in sorted(l0a | l0b)}
    except Exception as exc:
        return {"undecidable": ["[TOOL-ERROR] Cargo.toml parse failed: %s: %s"
                                % (type(exc).__name__, exc)]}
    return {"layers": layers, "l0a": l0a, "l0b": l0b, "edges": edges,
            "undecidable": []}


def evaluate(facts):
    """(rc, messages) from assembled facts; no IO, so fixtures reuse it verbatim."""
    if facts.get("undecidable"):
        return 2, list(facts["undecidable"])
    l0a, l0b = facts["l0a"], facts["l0b"]
    if len(l0a) + len(l0b) < CRATE_FLOOR:
        return 2, ["[TOOL-ERROR] L0 membership below floor: %d crate(s)"
                   % (len(l0a) + len(l0b))]
    edges = facts["edges"]
    if sum(len(v) for v in edges.values()) < EDGE_FLOOR:
        return 2, ["[TOOL-ERROR] internal L0 edge set below floor; "
                   "dependency parser is blind"]
    errs = []
    layers = facts["layers"]
    for crate in sorted(edges):
        deps = edges[crate]
        if crate in l0a:
            for dep in sorted(d for d in deps if d in l0b):
                errs.append("[PURITY-LEAK] L0-A %s -> L0-B %s" % (crate, dep))
        elif crate in l0b:
            for dep in sorted(d for d in deps if layers.get(d, 0) >= 2):
                errs.append("[UPWARD] L0-B %s -> layer %d %s"
                            % (crate, layers[dep], dep))
    return (1 if errs else 0), errs


def run_real():
    if tomllib is None:
        print("[UNDECIDABLE] tomllib unavailable (needs Python 3.11+)")
        return 2
    facts = build_facts(read_text(SH_FILE), read_text(PS1_FILE), CRATES_DIR,
                        l0_roster.L0A_DIR_NAMES)
    rc, msgs = evaluate(facts)
    for msg in msgs:
        print(msg)
    if rc == 0:
        edges = sum(len(v) for v in facts["edges"].values())
        print("[OK] L0 sub-layer direction holds: 0A=%d crate(s), 0B=%d crate(s), "
              "internal edges=%d"
              % (len(facts["l0a"]), len(facts["l0b"]), edges))
    return rc


def _write_crate(root, name, deps):
    """Minimal Cargo.toml fixture; dependency names double as crate names."""
    path = os.path.join(root, name)
    os.makedirs(os.path.join(path, "src"), exist_ok=True)
    lines = ['[package]', 'name = "%s"' % name, 'version = "0.1.0"', "",
             "[dependencies]"]
    for dep in deps:
        lines.append('%s = { path = "../%s" }' % (dep, dep))
    with open(os.path.join(path, "Cargo.toml"), "w", encoding="utf-8",
              newline="\n") as fh:
        fh.write("\n".join(lines) + "\n")


def _fixture_sh(layers, sub):
    """Render a synthetic layer_of/sublayer_of pair for tempdir fixtures."""
    by_layer = {}
    for crate, number in layers.items():
        by_layer.setdefault(number, []).append(crate)
    layer_arms = "\n".join("        %s) echo %d ;;" % ("|".join(sorted(names)), number)
                            for number, names in sorted(by_layer.items()))
    by_sub = {}
    for crate, value in sub.items():
        by_sub.setdefault(value, []).append(crate)
    sub_arms = "\n".join("        %s) echo \"%s\" ;;"
                         % ("|".join(sorted(names)), value)
                         for value, names in sorted(by_sub.items()))
    return ("layer_of() {\n    case \"$1\" in\n%s\n        *) echo \"\" ;;\n"
            "    esac\n}\n\nsublayer_of() {\n    case \"$1\" in\n%s\n"
            "        *) echo \"\" ;;\n    esac\n}\n" % (layer_arms, sub_arms))


def _fixture_ps1(sub):
    """Render a synthetic $subLayerMap for tempdir fixtures."""
    by_sub = {}
    for crate, value in sub.items():
        by_sub.setdefault(value, []).append(crate)
    rows = "\n".join("    %s" % "; ".join("'%s' = '%s'" % (name, value)
                                         for name in sorted(names))
                     for value, names in sorted(by_sub.items()))
    return "$subLayerMap = @{\n%s\n}\n" % rows


def selftest():
    n = 0
    bad = 0

    def check(label, got, want):
        nonlocal n, bad
        n += 1
        ok = got == want
        if not ok:
            bad += 1
        print("  [%s] %s got=%r want=%r"
              % ("PASS" if ok else "FAIL", label, got, want))

    if tomllib is None:
        print("  [FAIL] tomllib unavailable (needs Python 3.11+)")
        return 1

    # --- real sources: the three parsers must agree on the live tree ----------
    sh_text = read_text(SH_FILE)
    ps1_text = read_text(PS1_FILE)
    layers = parse_sh_layer(sh_text)
    sh_sub = parse_sh_sublayer(sh_text)
    ps1_sub = parse_ps1_sublayer(ps1_text)
    check("real layer_of(): nexus-contracts=0",
          (layers or {}).get("nexus-contracts"), 0)
    check("real layer_of(): nexus-core=event-bus=1",
          ((layers or {}).get("nexus-core"), (layers or {}).get("event-bus")),
          (1, 1))
    check("real sublayer_of(): nexus-contracts=0A",
          (sh_sub or {}).get("nexus-contracts"), "0A")
    check("real sublayer_of(): nexus-core/event-bus=0B",
          ((sh_sub or {}).get("nexus-core"), (sh_sub or {}).get("event-bus")),
          ("0B", "0B"))
    check("real .sh and .ps1 sub-layer tables agree", sh_sub, ps1_sub)

    # --- judgement fixtures (same evaluate() as production) -------------------
    clean = {"layers": {"a": 0, "b": 1}, "l0a": {"a"}, "l0b": {"b"},
             "edges": {"a": set(), "b": {"a"}}, "undecidable": []}
    check("clean layering passes", evaluate(clean)[0], 0)

    leak = {"layers": {"a": 0, "b": 1}, "l0a": {"a"}, "l0b": {"b"},
            "edges": {"a": {"b"}, "b": set()}, "undecidable": []}
    rc, msgs = evaluate(leak)
    check("L0-A -> L0-B is red", rc, 1)
    check("leak message names both crates",
          any("a" in m and "b" in m for m in msgs), True)

    up = {"layers": {"a": 0, "b": 1, "l5": 5}, "l0a": {"a"}, "l0b": {"b"},
          "edges": {"a": set(), "b": {"l5"}}, "undecidable": []}
    rc, msgs = evaluate(up)
    check("L0-B -> layer 5 is red", rc, 1)
    check("upward message names the layer", any("layer 5" in m for m in msgs), True)

    blind = {"layers": {"a": 0, "b": 1}, "l0a": {"a"}, "l0b": {"b"},
             "edges": {"a": set(), "b": set()}, "undecidable": []}
    check("empty edge set is undecidable, not clean", evaluate(blind)[0], 2)

    tiny = {"layers": {"a": 0}, "l0a": {"a"}, "l0b": set(),
            "edges": {"a": set()}, "undecidable": []}
    check("membership below floor is undecidable", evaluate(tiny)[0], 2)

    # --- membership cross-checks (rc 2, never rc 1) ---------------------------
    check("sh/ps1 drift is undecidable",
          bool(validate_membership({"a": 0}, {"a": "0A"}, {"a": "0B"})), True)
    check("coverage gap is undecidable",
          bool(validate_membership({"a": 0, "c": 0}, {"a": "0A"}, {"a": "0A"})),
          True)
    check("unknown sub-layer value is undecidable",
          bool(validate_membership({"a": 0}, {"a": "0C"}, {"a": "0C"})), True)
    check("roster out of sync with the 0A set is undecidable (ADR-222 D2)",
          bool(validate_membership({"a": 0, "b": 0}, {"a": "0A", "b": "0A"},
                                   {"a": "0A", "b": "0A"}, roster=("a",))), True)
    check("roster in sync is not an error",
          validate_membership({"a": 0}, {"a": "0A"}, {"a": "0A"}, roster=("a",)),
          [])

    # --- assembly path: fixtures go through build_facts, not a test-only copy --
    with tempfile.TemporaryDirectory() as tmp:
        _write_crate(tmp, "a", [])
        _write_crate(tmp, "b", ["a"])
        facts = build_facts(_fixture_sh({"a": 0, "b": 1}, {"a": "0A", "b": "0B"}),
                            _fixture_ps1({"a": "0A", "b": "0B"}), tmp, roster=("a",))
        check("build_facts parses manifests instead of restating deps",
              facts.get("edges", {}).get("b"), {"a"})
        check("fixture tree passes through the production evaluator",
              evaluate(facts)[0], 0)

        _write_crate(tmp, "a", ["b"])
        leak_facts = build_facts(
            _fixture_sh({"a": 0, "b": 1}, {"a": "0A", "b": "0B"}),
            _fixture_ps1({"a": "0A", "b": "0B"}), tmp, roster=("a",))
        check("fixture A->B manifests turn red", evaluate(leak_facts)[0], 1)

    # --- a duplicate hashtable key is undecidable, never silently collapsed ---
    dup_ps1 = "$subLayerMap = @{\n    'a' = '0A'; 'b' = '0B'; 'a' = '0A'\n}\n"
    dup_facts = build_facts(_fixture_sh({"a": 0, "b": 1}, {"a": "0A", "b": "0B"}),
                            dup_ps1, ".", roster=("a",))
    check("ps1 duplicate $subLayerMap key is undecidable and names the key",
          dup_facts.get("undecidable") and "'a'" in dup_facts["undecidable"][0],
          True)
    check("ps1 duplicate key refuses before any edge scan (no fake clean)",
          evaluate(dup_facts)[0], 2)

    # --- the real tree must be green -----------------------------------------
    check("real tree rc", run_real(), 0)

    print("  RESULT: %s (%d assertions)" % ("PASS" if not bad else "FAIL", n))
    return 1 if bad else 0


def main(argv):
    if argv == ["--selftest"]:
        return selftest()
    if argv:
        print("usage: check_l0_sublayer.py [--selftest]")
        return 2
    return run_real()


if __name__ == "__main__":
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
