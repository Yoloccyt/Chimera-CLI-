#!/usr/bin/env python3
# -*- coding: ascii -*-
"""Layer-map parity gate (C10, 2026-09-04).

Purpose
-------
The dependency iron-law audit exists as TWO hand-maintained implementations
(scripts/check_dependency_rules.sh for CI + scripts/check_dependency_rules.ps1
for local Windows). Each embeds its own crate->layer table (sh: layer_of() case
arms + layered_crates list; ps1: $layerMap hashtable). The 2026-08-29 incident
(.ps1 raised to 43 while the CI-called .sh stayed at 38 -> iron-law job red,
5 crates unchecked) proved the comment-level "DRIFT WARNING" is not enough.

This gate mechanically cross-checks FIVE structures against each other:
  1. .sh  layer_of() case dict          (crate -> layer)
  2. .sh  layered_crates list            (same key set as 1)
  3. .ps1 $layerMap hashtable            (crate -> layer, equal to 1)
  4. root Cargo.toml workspace.members   (ground truth crate roster)
  5. root Cargo.toml [workspace.dependencies] "# L<n" comment grouping
     (Check D, added 2026-09-24: the roster was policed but the dependency
     table's layer comments were read by nobody, so three entries -- mcp-mesh
     under L1, gea-activator + ssra-fusion under L6 -- had been lying silently)
plus the static bounds expected_crates (sh) / $expectedCrates (ps1) equal the
real member count. Any single-sided drift turns this gate red at PR time.

Modes / exit codes
------------------
  (default)   parity on real repo files      0 = hold, 1 = drift
  --selftest  fixture injection proves the gate has teeth
                                              0 = all assertions held, 1 = not
  usage error                                 2

Output is pure ASCII (project script convention, avoids Windows GBK console
pitfalls).
"""

import re
import sys

ROOT_MARK = "crates/"


# ---------------------------------------------------------------------------
# Parsers (regex on source text; no bash/pwsh execution needed)
# ---------------------------------------------------------------------------

def parse_sh(text):
    """Return (case_dict, layered_list, expected_int) from check_dependency_rules.sh."""
    m = re.search(r"layer_of\(\)\s*\{\s*case \"\$1\" in(.*?)esac", text, re.S)
    if not m:
        raise ValueError("sh: layer_of() case block not found")
    case = {}
    for line in m.group(1).splitlines():
        line = line.strip()
        mm = re.match(r"^([A-Za-z0-9_-]+(?:\|[A-Za-z0-9_-]+)*)\)\s*echo\s+(\d+)\s*;;$", line)
        if mm:
            for name in mm.group(1).split("|"):
                case[name] = int(mm.group(2))
    ml = re.search(r'^layered_crates="([^"]+)"', text, re.M)
    if not ml:
        raise ValueError("sh: layered_crates list not found")
    lst = ml.group(1).split()
    me = re.search(r"^expected_crates=(\d+)", text, re.M)
    if not me:
        raise ValueError("sh: expected_crates not found")
    return case, lst, int(me.group(1))


def parse_ps1(text):
    """Return (layermap_dict, expected_int) from check_dependency_rules.ps1."""
    m = re.search(r"\$layerMap\s*=\s*@\{(.*?)\n\}", text, re.S)
    if not m:
        raise ValueError("ps1: $layerMap block not found")
    pair = re.findall(r"'([A-Za-z0-9_-]+)'\s*=\s*(\d+)", m.group(1))
    return {k: int(v) for k, v in pair}, int(re.search(r"\$expectedCrates\s*=\s*(\d+)", text).group(1))


def parse_members(text):
    """Return sorted crate names from root Cargo.toml [workspace] members list.

    Skips comment lines (a leading '#' before the quoted path means commented out).
    """
    m = re.search(r"\[workspace\].*?members\s*=\s*\[(.*?)\n\]", text, re.S)
    if not m:
        raise ValueError("Cargo.toml: workspace members list not found")
    names = []
    for line in m.group(1).splitlines():
        stripped = line.strip()
        if stripped.startswith("#"):
            continue
        # One line may carry several quoted entries (real Cargo.toml style:
        # `"crates/nexus-core", "crates/event-bus", "crates/quest-engine",`),
        # so findall per line instead of a single ^...$ match.
        names.extend(re.findall(r'"crates/([A-Za-z0-9_-]+)"', stripped))
    return names


# ---------------------------------------------------------------------------
# Check D: [workspace.dependencies] "# L<n>" comment grouping vs the layer map
# ---------------------------------------------------------------------------

DEP_SECTION = "[workspace.dependencies]"
GROUP_RE = re.compile(r"^\s*#\s*L(\d+)\b")
ENTRY_RE = re.compile(r'^\s*([A-Za-z0-9_-]+)\s*=\s*\{\s*path\s*=\s*"crates/([A-Za-z0-9_-]+)"')
INLINE_RE = re.compile(r"#\s*L(\d+)\b")


def parse_dep_groups(text):
    """Return [(dep_key, path_crate, effective_layer, line_no)] inside [workspace.dependencies].

    A group header applies to the entries below it; an inline "# L<n>" on the entry line
    wins, because the manifest keeps some crates at their historical position and labels
    them where they sit (same convention the router-traits line already used).
    Extracting nothing raises: an empty scan surface must never be read as "groups agree".
    """
    rows = []
    inside = False
    cur = None
    for no, line in enumerate(text.splitlines(), 1):
        if line.startswith("["):
            inside = line.strip() == DEP_SECTION
            cur = None
            continue
        if not inside:
            continue
        g = GROUP_RE.match(line)
        if g:
            cur = int(g.group(1))
            continue
        e = ENTRY_RE.match(line)
        if not e:
            continue
        inline = INLINE_RE.search(line)
        rows.append((e.group(1), e.group(2), int(inline.group(1)) if inline else cur, no))
    if not rows:
        raise ValueError("Cargo.toml: %s scan extracted 0 path entries" % DEP_SECTION)
    return rows


def check_dep_groups(sh_case, rows):
    """Pure diff between the comment grouping and the authoritative layer map."""
    fails = []
    for key, path_crate, layer, no in rows:
        if key != path_crate:
            fails.append("Cargo.toml:%d dep key %s != path crate %s" % (no, key, path_crate))
            continue
        truth = sh_case.get(key)
        if truth is None:
            fails.append("Cargo.toml:%d dep %s absent from layer map" % (no, key))
        elif layer is None:
            fails.append("Cargo.toml:%d dep %s sits outside any '# L<n>' group" % (no, key))
        elif layer != truth:
            fails.append("Cargo.toml:%d dep %s grouping says L%d, layer map says L%d"
                         % (no, key, layer, truth))
    return fails


# ---------------------------------------------------------------------------
# Parity checks (pure function over the six inputs -> list of failure strings)
# ---------------------------------------------------------------------------

def run_checks(sh_case, sh_list, sh_exp, ps_map, ps_exp, members):
    fails = []
    case_keys = set(sh_case)
    list_keys = set(sh_list)
    ps_keys = set(ps_map)
    mem_keys = set(members)

    if len(sh_list) != len(list_keys):
        fails.append("sh: layered_crates has duplicate entries")
    if case_keys != list_keys:
        fails.append("sh: case dict vs layered_crates list differ: "
                     "case-only=%s list-only=%s" % (sorted(case_keys - list_keys),
                                                     sorted(list_keys - case_keys)))
    if sh_case != ps_map:
        drift = []
        for k in sorted(set(sh_case) | set(ps_map)):
            if sh_case.get(k) != ps_map.get(k):
                drift.append("%s(sh=%s,ps1=%s)" % (k, sh_case.get(k), ps_map.get(k)))
        fails.append("sh vs ps1 layer maps differ: %s" % "; ".join(drift[:12]))
    if mem_keys != case_keys or len(members) != len(mem_keys):
        fails.append("Cargo.toml members vs layer map differ: "
                     "members-only=%s map-only=%s" % (sorted(mem_keys - case_keys),
                                                       sorted(case_keys - mem_keys)))
    if sh_exp != len(mem_keys):
        fails.append("sh expected_crates=%d != real member count %d" % (sh_exp, len(mem_keys)))
    if ps_exp != len(mem_keys):
        fails.append("ps1 expectedCrates=%d != real member count %d" % (ps_exp, len(mem_keys)))
    return fails


# ---------------------------------------------------------------------------
# selftest: fixture injection proving the gate has teeth (perf_redlines.py pattern)
# ---------------------------------------------------------------------------

def selftest():
    # 42 synthetic crates + nexus-contracts = 43 members (clean fixture bound).
    base = {("crate-%02d" % i): (i % 11) for i in range(42)}
    base["nexus-contracts"] = 0
    members = sorted(base)
    ok_case, ok_list, ok_ps = dict(base), list(members), dict(base)
    failures = 0

    def expect(name, cond):
        nonlocal failures
        print("  [%s] %s" % ("PASS" if cond else "FAIL", name))
        if not cond:
            failures += 1

    # clean fixture must hold
    expect("selftest-1 clean fixture green",
           run_checks(ok_case, ok_list, 43, ok_ps, 43, members) == [])
    # ps1 single-sided layer change must be caught (the 08-29 drift class)
    bad_ps = dict(ok_ps); bad_ps["crate-05"] = 99
    expect("selftest-2 ps1 layer change caught",
           run_checks(ok_case, ok_list, 43, bad_ps, 43, members) != [])
    # sh list dropped crate (asymmetric maintenance) must be caught
    bad_list = [c for c in ok_list if c != "crate-07"]
    expect("selftest-3 sh list drop caught",
           run_checks(ok_case, bad_list, 43, ok_ps, 43, members) != [])
    # new member not in layer maps (44th crate without registration) must be caught
    bad_members = members + ["new-crate"]
    expect("selftest-4 unregistered member caught",
           run_checks(ok_case, ok_list, 43, ok_ps, 43, bad_members) != [])
    # stale static bound (count bumped wrongly) must be caught
    expect("selftest-5 stale expected_crates caught",
           run_checks(ok_case, ok_list, 42, ok_ps, 43, members) != [])
    # --- Check D: dependency-table "# L<n>" grouping vs the layer map ---------
    ok_rows = [(c, c, ok_case[c], 100 + i) for i, c in enumerate(members)]
    expect("selftest-6 clean dep grouping green",
           check_dep_groups(ok_case, ok_rows) == [])
    shifted = list(ok_rows)
    shifted[5] = (shifted[5][0], shifted[5][1], 3, shifted[5][3])
    expect("selftest-7 misplaced dep grouping caught",
           check_dep_groups(ok_case, shifted) != [])
    ungrouped = list(ok_rows)
    ungrouped[8] = (ungrouped[8][0], ungrouped[8][1], None, ungrouped[8][3])
    expect("selftest-8 dep outside any group caught",
           check_dep_groups(ok_case, ungrouped) != [])
    unknown = list(ok_rows)
    unknown.append(("ghost-crate", "ghost-crate", 1, 999))
    expect("selftest-9 dep absent from layer map caught",
           check_dep_groups(ok_case, unknown) != [])
    # parser level: an inline label must beat the group header it physically sits under
    mini = ('[workspace]\nmembers = ["crates/a"]\n'
            '[workspace.dependencies]\n'
            '# L1 Core\n'
            'a = { path = "crates/a" }  # L9 Quest\n')
    expect("selftest-10 inline label overrides group header",
           parse_dep_groups(mini) == [("a", "a", 9, 5)])
    # an empty scan surface must raise, never report "grouping agrees"
    try:
        parse_dep_groups('[package]\nname = "x"\n')
        empty_refused = False
    except ValueError:
        empty_refused = True
    expect("selftest-11 empty dep scan refused", empty_refused)
    total = 11
    verdict = "PASS (all %d assertions held)" % total if failures == 0 else "FAIL (%d/%d violated)" % (failures, total)
    print("  RESULT: %s" % verdict)
    return 0 if failures == 0 else 1


def main(argv):
    if "--selftest" in argv:
        return selftest()
    if any(a not in ("", "--paranoid") for a in argv[1:]):
        print("usage: check_layer_map_parity.py [--selftest]", file=sys.stderr)
        return 2
    try:
        with open("scripts/check_dependency_rules.sh", encoding="utf-8") as fh:
            sh_case, sh_list, sh_exp = parse_sh(fh.read())
        with open("scripts/check_dependency_rules.ps1", encoding="utf-8") as fh:
            ps_map, ps_exp = parse_ps1(fh.read())
        with open("Cargo.toml", encoding="utf-8-sig") as fh:
            manifest_text = fh.read()
        members = parse_members(manifest_text)
        dep_rows = parse_dep_groups(manifest_text)
    except Exception as exc:  # parse failure = gate cannot judge -> red, never silent
        print("[FAIL] parse error: %s" % exc)
        return 1
    fails = run_checks(sh_case, sh_list, sh_exp, ps_map, ps_exp, members)
    fails = fails + check_dep_groups(sh_case, dep_rows)
    for f in fails:
        print("[FAIL] %s" % f)
    if fails:
        print("Layer-map parity BROKEN (sh vs ps1 vs Cargo.toml). Fix every table")
        print("in the same commit; see DRIFT warnings in both scripts' headers.")
        return 1
    print("[OK] layer-map parity holds: sh case=%d sh list=%d ps1=%d members=%d "
          "dep groupings=%d (all equal)"
          % (len(sh_case), len(sh_list), len(ps_map), len(members), len(dep_rows)))
    return 0


if __name__ == "__main__":
    # ASCII-only file (see the coding cookie above).
    # gate_rc: a crash must exit 2, never borrow 1 as "judged red" (F32/F33).
    import gate_rc
    sys.exit(gate_rc.run(lambda: main(sys.argv)))
