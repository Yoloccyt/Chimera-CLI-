#!/usr/bin/env python3
# -*- coding: ascii -*-
"""Gate-predicate parity (Q15 / ADR-186 companion, 2026-09-21).

Purpose
-------
`check_layer_map_parity.py` (C10) cross-checks the DATA TABLES of the two
hand-maintained iron-law implementations:
  scripts/check_dependency_rules.sh   (the one CI actually runs: ci.yml:89,
                                        gate_manifest.toml G-08)
  scripts/check_dependency_rules.ps1  (local twin, NO CI/gate_manifest caller)

It cannot see PREDICATE drift. Observed on 2026-09-18 (docs/reports/
Q3-adjudication-inner-ring-gate-2026-09-21.md section 3): a change labelled
"FIXED 2026-09-18, ADR-XXX" rewrote .ps1's Check A predicate to
`$depLayer -gt $crateLayer -and $depLayer -ge 2` (which makes Check A a strict
subset of Check B, i.e. logically dead) while .sh kept the original
no-layer-comparison fall-through. Consequence: the two twins reach OPPOSITE
verdicts on gsoe-evolution(L5) -> decay-engine(L4), the "both twins EXIT=0"
claim became half-true/half-false, and the CI-visible behaviour never changed
because the edit landed on the non-executing side. `ADR-XXX` is a placeholder
number absent from docs/architecture/adr_index.md.

This gate cross-checks the RULES, not the tables:
  P1  Check roster            -- which Checks each twin implements
  P2  GAP emission arity      -- how many `[GAP-X]` sites per tag
  P3  Check predicate class   -- `layer-guarded` vs `fallthrough`
  P4  Check skip-guard count  -- how many exclusions precede the report
  P5  selftest expectation    -- per-tag expected GAP counts

Building it surfaced three false-positive classes of its own, each now pinned by
a selftest assertion -- a heuristic gate that cannot prove its own readings is
worse than no gate, because it launders a fabricated "drift" into a decision:
  * comment prose containing "layer" plus a `->` arrow in a report literal made
    the un-guarded .sh Check A look layer-guarded (selftest-8);
  * the LAST Check (D) inherited the selftest scaffolding after its own
    function close (selftest-9);
  * a report literal ("layer map entry <$crate> has no layer") counted as a
    predicate (selftest-10).

Known divergences are recorded in scripts/gate_predicate_baseline.txt as VISIBLE
debt (shrink-only ratchet, same discipline as dep_edge_freeze.txt): the gate is
green at adoption but turns red on any NEW divergence, and red on a STALE
baseline entry (a divergence silently fixed without deleting its line).
Deliberately NOT registered: P3|Check A -- that divergence is what Q3 must
adjudicate, so its redness IS the alarm (same reasoning as the 18 unregistered
NEW edges in dep_edge_freeze.txt).

Modes / exit codes
------------------
  (default)   parity on real repo files     0 = hold, 1 = drift
  --selftest  fixture injection             0 = all detectors have teeth, 1 = not
  --emit      print current findings as baseline lines (then hand-review)
  --explain   print each twin's per-Check evidence (skips / gaps / predicate line)
  usage error                                 2

Output is pure ASCII (project script convention, Windows GBK consoles).
"""

import re
import sys

SH = "scripts/check_dependency_rules.sh"
PS1 = "scripts/check_dependency_rules.ps1"
BASELINE = "scripts/gate_predicate_baseline.txt"

CHECK_HDR = re.compile(r"^[\s#]*-+\s*Check\s+([A-Z])\s*:", re.M)
GAP_EMIT = re.compile(r"\[GAP-([A-Z])\]")
SELFTEST_EXPECT = re.compile(r"expected\s+(\d+)\s+GAP-([A-Z])\s+line")
SKIP_GUARD = re.compile(r"\bcontinue\b")
# Report literals are OUTPUT, never a guard: the words "layer map" inside
# "[GAP-C] layer map entry <$crate> has no layer" once made Check C look
# layer-predicated on both sides (agreeing by accident, and showing an auditor a
# string literal as "evidence"). Dropping append-to-report lines is the
# dialect-neutral way to keep predicates and prose apart.
REPORT_LINE = re.compile(r"\breport\s*\+?=\s*[\(\"']|\breport\s*\+=")
# A Check is "layer-predicated" iff some *statement* line relationally compares a
# layer-derived value (dep-vs-crate, as in .ps1 Check A, or layer-vs-bound as in
# Check E). Token-anywhere matching was tried first and produced a FALSE NEGATIVE
# on the very drift this gate exists to catch: the .sh Check A comment "(dep with
# a layer)" supplied the crate-side token and the report literal "$crate -> $dep"
# supplied a ">" via the arrow, so both twins misread as layer-predicated.
# Hence: operands must be *adjacent* to the operator, and `->` is an arrow, not a
# comparison.
LAYER_TOKEN = re.compile(
    r"dep_layer|depLayer|crate_layer|crateLayer|\blayer\b"
    r"|layerMap\[\s*\$(?:dep|crate)\b"
    r"|\$\(layer_of\s+\"\$(?:dep|crate)\"\)")
REL_OP = (r"(?<!\w)(?:-gt|-lt|-ge|-le|-eq|-ne)(?!\w)"
          r"|>=|<=|==|(?<![<>=!\-\w])>(?!=)|(?<![<>=!])<(?!=)")
REL_OP_RE = re.compile(REL_OP)
# How far (chars) from an operator to look for a layer token as its operand.
OPERAND_WINDOW = 26
# A Check body never outlives the function that holds it. Without this cut the
# LAST Check (D) extends to end-of-file and inherits the selftest section's own
# layer comparisons, which read as a predicate class it does not have.
FUNC_END = re.compile(r"(?m)^(?:\}|# =)")


def cut_to_func_end(block):
    m = FUNC_END.search(block)
    return block[:m.start()] if m else block


def strip_comments(block):
    """Drop comment-only lines and trailing inline comments (statement text only)."""
    out = []
    for line in block.splitlines():
        if line.lstrip().startswith("#"):
            continue
        # An inline comment must be preceded by whitespace; this keeps report
        # literals intact (they never contain " #").
        line = re.sub(r"\s+#(?![!]).*$", "", line)
        out.append(line)
    return "\n".join(out)


def layer_predicate_line(code):
    """Return the first statement line that relationally compares a layer value.

    Returning the line (not a bare bool) lets `--explain` show a human the
    evidence behind a P3 finding, so adjudicating drift never means re-reading
    both twins from scratch.
    """
    for m in REL_OP_RE.finditer(code):
        left = code[:m.start()].rstrip(' "\t')[-OPERAND_WINDOW:]
        right = code[m.end():].lstrip(' "\t')[:OPERAND_WINDOW]
        if LAYER_TOKEN.search(left) or LAYER_TOKEN.search(right):
            start = code.rfind("\n", 0, m.start()) + 1
            end = code.find("\n", m.end())
            return code[start:len(code) if end < 0 else end].strip()
    return None


def parse(text, dialect):
    """Return {check: {'pline':str|None,'skips':int,'gaps':{tag:count}}, ...}."""
    checks = {}
    for m in CHECK_HDR.finditer(text):
        checks[m.group(1)] = m.start()
    out = {}
    order = sorted(checks, key=lambda k: checks[k])
    for i, name in enumerate(order):
        start = checks[name]
        stop = checks[order[i + 1]] if i + 1 < len(order) else len(text)
        block = text[start:stop]
        code = strip_comments(cut_to_func_end(block))
        guards = "\n".join(ln for ln in code.splitlines()
                           if not REPORT_LINE.search(ln))
        gaps = {}
        for tag in GAP_EMIT.findall(code):
            gaps[tag] = gaps.get(tag, 0) + 1
        # `|| continue` / `{ continue }` style exclusion guards only; the loop
        # header `|| continue` in sh counts too (it is an exclusion).
        skips = len([ln for ln in code.splitlines() if SKIP_GUARD.search(ln)])
        out[name] = {"pline": layer_predicate_line(guards), "skips": skips,
                     "gaps": gaps, "dialect": dialect}
    return out


def parse_selftest_expectations(text):
    """Return {tag: count} of the selftest negative-control expectations."""
    exp = {}
    for m in SELFTEST_EXPECT.finditer(text):
        exp[m.group(2)] = int(m.group(1))
    return exp


def load_baseline(root):
    """Return set('ID|detail') of registered known divergences."""
    known = set()
    try:
        fh = open(root + "/" + BASELINE, encoding="utf-8-sig")
    except OSError:
        return known
    with fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if line:
                known.add(line)
    return known


def adjudicate(sh, ps, sh_exp, ps_exp, baseline):
    """Return (unregistered_findings, stale) as sorted 'ID|detail' lists.

    Both sides must compare against the RAW divergence set: subtracting the
    baseline first would make every registered line look stale (it is, by
    construction, absent from the suppressed list) -- the bug this function had
    on its first real run, caught because the gate then contradicted itself by
    reporting a line as both suppressed and stale.
    """
    raw = []

    def emit(fid, detail):
        raw.append("%s|%s" % (fid, detail))

    # P1 roster
    only_sh = sorted(set(sh) - set(ps))
    only_ps = sorted(set(ps) - set(sh))
    if only_sh:
        emit("P1", "Check present only in .sh: %s" % ",".join(only_sh))
    if only_ps:
        emit("P1", "Check present only in .ps1: %s" % ",".join(only_ps))

    # P2 GAP emission arity per tag (summed across Checks)
    for tag in sorted({t for c in list(sh.values()) + list(ps.values())
                       for t in c["gaps"]}):
        n_s = sum(c["gaps"].get(tag, 0) for c in sh.values())
        n_p = sum(c["gaps"].get(tag, 0) for c in ps.values())
        if n_s != n_p:
            emit("P2", "GAP-%s emit sites .sh=%d .ps1=%d" % (tag, n_s, n_p))

    # P3 predicate class + P4 skip-guard arity, per shared Check
    for name in sorted(set(sh) & set(ps)):
        if bool(sh[name]["pline"]) != bool(ps[name]["pline"]):
            emit("P3", "Check %s predicate class .sh=%s .ps1=%s"
                 % (name, "layer-guarded" if sh[name]["pline"] else "fallthrough",
                    "layer-guarded" if ps[name]["pline"] else "fallthrough"))
        if sh[name]["skips"] != ps[name]["skips"]:
            emit("P4", "Check %s skip-guards .sh=%d .ps1=%d"
                 % (name, sh[name]["skips"], ps[name]["skips"]))

    # P5 selftest negative-control expectations
    for tag in sorted(set(sh_exp) | set(ps_exp)):
        if sh_exp.get(tag) != ps_exp.get(tag):
            emit("P5", "selftest expected GAP-%s .sh=%s .ps1=%s"
                 % (tag, sh_exp.get(tag), ps_exp.get(tag)))

    current = set(raw)
    return sorted(current - baseline), sorted(baseline - current)


# ---------------------------------------------------------------------------
# selftest: fixture injection proving each detector has teeth
# ---------------------------------------------------------------------------

SH_CLEAN = '''
    # --- Check A: inner-ring boundary ---
    # Only internal edges (dep with a layer) are audited; the word "layer" here
    # and the "$crate -> $dep" arrow below are exactly the tokens that once
    # fooled the classifier into reporting both twins as layer-predicated.
    for crate in $layered_crates; do
        is_inner_ring "$crate" || continue
        for dep in $(deps_of "$crate"); do
            [ -n "$(layer_of "$dep")" ] || continue
            if is_inner_base "$dep"; then continue; fi
            report+=("[GAP-A] $crate -> $dep boundary")
            status=1
        done
    done
    # --- Check B: upward dependency ---
    for crate in $(all_crates); do
        [ -n "$layer" ] || continue
        for dep in $(deps_of "$crate"); do
            if [ "$dep_layer" -gt "$layer" ]; then
                report+=("[GAP-B] $crate -> $dep upward")
                status=1
            fi
        done
    done
    [ "$gb" -eq 2 ] || { echo "expected 2 GAP-B lines"; }
    [ "$ga" -eq 1 ] || { echo "expected 1 GAP-A lines"; }
'''

PS_CLEAN = '''
    # --- Check A: inner-ring boundary ---
    foreach ($crate in $innerRing.Keys) {
        if ($null -eq $dep) { continue }
        if (-not $layerMap.ContainsKey($dep)) { continue }
        if ($innerBase.ContainsKey($dep)) { continue }
        $script:report += "[GAP-A] $crate -> $dep boundary"
    }
    # --- Check B: upward dependency ---
    foreach ($crate in $all) {
        if (-not $layer) { continue }
        if ($depLayer -gt $layer) {
            $script:report += "[GAP-B] $crate -> $dep upward"
        }
    }
    if ($gapBLines.Count -ne 2) { "[SELFTEST] expected 2 GAP-B lines" }
    if ($gapALines.Count -ne 1) { "[SELFTEST] expected 1 GAP-A lines" }
'''


def selftest():
    fails = 0

    def expect(name, cond):
        nonlocal fails
        print("  [%s] %s" % ("PASS" if cond else "FAIL", name))
        if not cond:
            fails += 1

    sh0 = parse(SH_CLEAN, "sh")
    ps0 = parse(PS_CLEAN, "ps1")
    e_sh = parse_selftest_expectations(SH_CLEAN)
    e_ps = parse_selftest_expectations(PS_CLEAN)

    expect("selftest-1 clean fixture green",
           adjudicate(sh0, ps0, e_sh, e_ps, set())[0] == [])
    # P1: a Check existing on one side only must be caught (Check A dropped from ps1)
    expect("selftest-2 P1 missing Check caught",
           adjudicate(sh0, parse(PS_CLEAN.replace(
               "# --- Check A: inner-ring boundary ---", "# --- Check Z: unrelated ---"),
               "ps1"), e_sh, e_ps, set())[0] != [])
    # P3: the real-world class. ps1 Check A gains a layer comparison -> drift
    guarded_ps = PS_CLEAN.replace(
        '        if ($innerBase.ContainsKey($dep)) { continue }\n',
        '        if ($innerBase.ContainsKey($dep)) { continue }\n'
        '        if ($depLayer -gt $crateLayer -and $depLayer -ge 2) {\n')
    expect("selftest-3 P3 predicate class flip caught",
           adjudicate(sh0, parse(guarded_ps + "\n            }\n", "ps1"),
                      e_sh, e_ps, set())[0] != [])
    # P2: one extra GAP-C site on one side must be caught
    expect("selftest-4 P2 emit-arity drift caught",
           adjudicate(sh0, parse(PS_CLEAN + '\n# --- Check C: x ---\n'
                                '$script:report += "[GAP-C] a"\n'
                                '$script:report += "[GAP-C] b"\n', "ps1"),
                      e_sh, e_ps, set())[0] != [])
    # P5: selftest expectation changed on one side (a silently weakened control)
    expect("selftest-5 P5 expectation drift caught",
           adjudicate(sh0, ps0, e_sh, {"A": 1, "B": 3}, set())[0] != [])
    # baseline must actually suppress a known finding -- AND must not then call
    # that same live line stale (the two lists compare against the raw divergence
    # set, not against each other; this pairing is what a first draft got wrong).
    known = "P5|selftest expected GAP-B .sh=2 .ps1=3"
    got = adjudicate(sh0, ps0, e_sh, {"A": 1, "B": 3}, {known})
    expect("selftest-6 registered baseline suppressed, not stale",
           got[0] == [] and got[1] == [])
    # ...and a baseline entry with no current finding must surface as STALE
    expect("selftest-7 stale baseline reported",
           adjudicate(sh0, ps0, e_sh, e_ps, {"P9|nothing matches this"})[1] != [])
    # Regression for this gate's OWN first bug: comment prose ("...with a layer")
    # and the "$crate -> $dep" arrow inside a report literal must NOT make a
    # Check look layer-predicated, while a real comparison must.
    expect("selftest-8 classifier ignores comment/arrow tokens",
           sh0["A"]["pline"] is None and sh0["B"]["pline"] is not None)
    # selftest-9: the last Check must not inherit the selftest section that
    # follows its own function close (an uncut block reads a stray comparison
    # as belonging to it -- real risk here: Check D is the last block in both
    # twins and everything after the function body is test scaffolding).
    tail = (PS_CLEAN + "\n    # --- Check D: bound ---\n"
            "    if ($count -gt $limit) { 'nope' }\n}\n"
            "if ($count -gt $layer) { 'selftest scaffolding' }\n")
    expect("selftest-9 block cut at function close",
           parse(tail, "ps1")["D"]["pline"] is None)
    # selftest-10: a report LITERAL is output, not a guard. The real line below
    # once made Check C look layer-predicated on both sides by coincidence.
    literal = ('    # --- Check Q: completeness ---\n'
               '    report+=("[GAP-Q] layer map entry <$crate> has no layer'
               ' (internal config error)")\n')
    expect("selftest-10 report literal is not a predicate",
           parse(literal, "sh")["Q"]["pline"] is None
           and parse(literal, "sh")["Q"]["gaps"] == {"Q": 1})
    print("  RESULT: %s" % ("PASS (all 10 detectors hold)" if fails == 0
                            else "FAIL (%d/10 violated)" % fails))
    return 0 if fails == 0 else 1


def explain(sh, ps):
    """Print the predicate evidence line each twin has per Check."""
    for name in sorted(set(sh) | set(ps)):
        for label, side in ((".sh", sh), (".ps1", ps)):
            info = side.get(name)
            if info is None:
                print("  Check %s %-4s : <absent>" % (name, label))
                continue
            # Scrub non-ASCII: source lines may embed CJK, gate output may not.
            evidence = re.sub(r"[^\x20-\x7e]", "?",
                              (info["pline"] or "no layer comparison")[:78])
            print("  Check %s %-4s : skips=%d gaps=%s predicate=%s"
                  % (name, label, info["skips"],
                     ",".join("%s x%d" % kv for kv in sorted(info["gaps"].items())) or "-",
                     evidence))


def main(argv):
    if "--selftest" in argv:
        return selftest()
    if any(a not in ("", "--emit", "--explain") for a in argv[1:]):
        print("usage: check_gate_predicates_parity.py [--selftest|--emit|--explain]",
              file=sys.stderr)
        return 2
    try:
        with open(SH, encoding="utf-8-sig") as fh:
            sh_text = fh.read()
        with open(PS1, encoding="utf-8-sig") as fh:
            ps_text = fh.read()
    except OSError as exc:
        print("[FAIL] cannot read twin scripts: %s" % exc)
        return 1
    sh = parse(sh_text, "sh")
    ps = parse(ps_text, "ps1")
    if not sh or not ps:
        # A twin whose Check headers we cannot see is NOT a pass: the gate would
        # be vacuous. Fail loud rather than judge nothing.
        print("[FAIL] Check headers not found (sh=%d ps1=%d) - gate cannot judge"
              % (len(sh), len(ps)))
        return 1
    findings, stale = adjudicate(sh, ps,
                                 parse_selftest_expectations(sh_text),
                                 parse_selftest_expectations(ps_text),
                                 load_baseline("."))
    if "--explain" in argv:
        explain(sh, ps)
        for f in findings:
            print("[PREDICATE-DRIFT] %s" % f)
        for s in stale:
            print("[STALE] registered divergence no longer real: %s" % s)
        return 1 if (findings or stale) else 0
    if "--emit" in argv:
        for f in findings:
            print(f)
        return 0
    for f in findings:
        print("[PREDICATE-DRIFT] %s" % f)
    for s in stale:
        print("[STALE] registered divergence no longer real: %s" % s)
    if findings or stale:
        print("")
        print("The two iron-law implementations disagree on RULES, not just data.")
        print("Fix: make both twins express the same predicate in the same commit,")
        print("     or register the divergence in %s with a reason (shrink-only)."
              % BASELINE)
        print("     A judgement change on the non-executing side (.ps1) is NOT a fix:")
        print("     CI runs .sh only (ci.yml:89 / gate_manifest G-08).")
        print("     Ref: docs/reports/Q3-adjudication-inner-ring-gate-2026-09-21.md")
        return 1
    print("[OK] gate predicates hold: Checks=%s, same class/arity/expectations"
          % ",".join(sorted(set(sh) & set(ps))))
    return 0


if __name__ == "__main__":
    # ASCII-only file (see the coding cookie above).
    # gate_rc: a crash must exit 2, never borrow 1 as "judged red" (F32/F33).
    # This gate ships with expect=1, so a bare crash used to read as "as expected".
    import gate_rc
    sys.exit(gate_rc.run(lambda: main(sys.argv)))
