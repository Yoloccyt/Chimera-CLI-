#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L0 zero-consumer contract ledger (G-123 / G-124, ADR-220 decision 1).

Why this gate exists
--------------------
G-91 (scripts/check_contract_consumers.py) enforces "every nexus-contracts pub
def has >= 2 consuming workspace crates, or is registered in an only-shrink
baseline". Measured on 2026-10-16 it prints:

    [INFO] nexus-contracts defs: 236 (baseline: 161; gaps<2: 161; >=2-consumer rule)
    [OK]

`gaps<2` EQUALS `baseline`, i.e. every single deficiency is already exempted.
G-91's `judge` computes `stale = baseline - gap_set`, so dropping or renaming a
def is caught -- but a NEWLY added zero-consumer public type is absorbed silently:
add the def, add one baseline line, and both gates stay green forever. Of those
exemptions, 75 defs have **zero** production consumers. G-91's own header says it
plainly: "Deficiencies are made visible in an only-shrink baseline register" --
visibility was the goal, and a register that has reached 100% coverage of the
deficiency set no longer discriminates.

A scalar ratchet (`zero_max=75`) was rejected: renaming a type moves one name out
and one in, the net stays 75, and the gate stays green while the ledger content
changes completely. G-115 and G-119 both only gained teeth when they moved from
"count" to "exact key set", so this gate uses the same form:

  * the ledger is **machine-derived** (never hand-edited) and must equal the live
    zero-consumer set exactly -- NEW/STALE/CLAIM;
  * `declared_in=` is recomputed by a second pass that is forced to agree with
    `check_contract_consumers`'s own enumeration, so it cannot become a shadow
    fact;
  * the consumer count comes from `cc.consumer_crates`, never a re-implementation,
    and `zero U one == cc.compute_gaps(...)` is asserted so a dialect split
    between the two gates is undecidable rather than silently tolerated.

Deliberate non-goals
--------------------
  * It does NOT own the exemption register. Humans edit
    `contract_consumer_baseline.txt`; this file is derived from it and from the
    scan. The gate never writes the baseline.
  * It does NOT floor the row count. `zero` is the DEBT measure: wiring up types
    drives it toward 0, which is the desired direction. A minimum there would make
    the gate's green depend on the defect persisting -- the exact pathology this
    repository has recorded. Floors here guard only the SCAN surface (defs).
  * It does NOT require a reason comment for newly zero-consumer defs on day one.
    All 75 current lines are bare names, so that rule would be red at birth; the
    debt is printed as an advisory instead.

Modes / exit codes
------------------
  (default)   judge the real repo      0 = agrees, 1 = NEW/STALE/LOC/CLAIM/UNREGISTERED
  --emit      write a candidate ledger under tmp/ (never overwrites)
  --write     rebuild the ledger in place, floors checked FIRST
  --selftest  planted positives AND negatives, through the production functions
  undecidable 2 = missing ledger, malformed row, duplicate key, blind scan
              (floor), dialect split, enumeration disagreement, missing CLAIM

Output is pure ASCII (project script convention; avoids Windows GBK console traps).
"""

import contextlib
import io
import os
import re
import sys
import tempfile
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = Path(HERE).parent
LEDGER = os.path.join(HERE, "contract_zero_consumer.txt")
CANDIDATE = str(ROOT / "tmp" / "contract_zero_consumer_candidate.txt")
BASELINE = os.path.join(HERE, "contract_consumer_baseline.txt")

sys.path.insert(0, HERE)
import check_contract_consumers as cc  # noqa: E402  (the ONLY consumer dialect)
from check_l0_pubfn_ratchet import module_key  # noqa: E402  (shares the key shape)

# Scan-surface floor. `check_contract_consumers.main()` carries its own inline 50
# for the same purpose; it is not exported, so this is a mirrored constant. The two
# do not need to stay equal and neither can make the other red: each guards its own
# scan, and both are far below the live def count, so a real collapse trips both.
DEFS_FLOOR = 50

NAME_RE = re.compile(r"^[A-Z][A-Za-z0-9_]*$")
CLAIM_RE = re.compile(r"^#\s*CLAIM\s+(.*)$")
ROW_FIELDS = ("consumers", "declared_in")
LOC_JOIN = "+"


def claim_text(defs_n, zero_n, one_n, ge2_n, gap_n, baseline_n, rows_n):
    """Header claim. `sum` is computed here, so a header cannot claim a false total."""
    total = zero_n + one_n + ge2_n
    return ("defs=%d zero=%d one=%d ge2=%d gap=%d baseline=%d rows=%d sum=%s"
            % (defs_n, zero_n, one_n, ge2_n, gap_n, baseline_n, rows_n,
               "OK" if total == defs_n else "FAIL"))


def floor_error(defs_n):
    """None when the scan is big enough to be trusted, else the diagnosis.

    Split out of run_real so --selftest can prove the guard fires: a floor that is
    never observed firing is a floor that does not exist (G-121 self-reported this
    exact defect class -- WORKFLOW_FLOOR declared but never wired into the judge).
    """
    if defs_n < DEFS_FLOOR:
        return ("defs == %d (below floor %d) -- the definition detector is likely "
                "blind; refusing to judge and refusing to write" % (defs_n, DEFS_FLOOR))
    return None


def scan(crates_root):
    """(facts, error). facts holds every number the ledger and the CLAIM are made of.

    Deliberately free of the floor guard: a data-gathering function that refuses on
    small input cannot be unit-tested on a small input, and the guard would then be
    proven only by itself. run_real applies floor_error() to whatever scan returns,
    and --selftest asserts that wiring by driving run_real over a 5-def fixture.
    """
    model = cc.build_model(Path(crates_root))
    if model is None:
        return None, "contracts crate not found under %s" % crates_root
    src_root = Path(crates_root)
    if not (src_root / cc.L0A_DIR_NAMES[0] / "src").is_dir():
        return None, ("contracts crate %s not found under %s"
                      % (cc.L0A_DIR_NAMES[0], crates_root))
    locs = {}
    # ADR-222 D2: the second pass must walk the same L0-A crate set that
    # cc.build_model() enumerates defs from, or the `set(locs) == defs` lock
    # below would fire on every split batch (a moved group would read as a def
    # that vanished). Module keys stay crate-relative, so `declared_in` is
    # byte-stable across the split. Missing siblings are tolerated for fixtures;
    # G-99 owns the real-tree disk-existence guard.
    for crate_name in cc.L0A_DIR_NAMES:
        src = src_root / crate_name / "src"
        if not src.is_dir():
            continue
        for path in sorted(src.rglob("*.rs")):
            text = cc.read_production_text(path, flatten=False)
            for m in cc.DEF_RE.finditer(text):
                locs.setdefault(m.group(1), []).append(module_key(str(path), str(src)))
    for keys in locs.values():
        keys.sort()
    if set(locs) != set(model["defs"]):
        only_cc = sorted(set(model["defs"]) - set(locs))
        only_here = sorted(set(locs) - set(model["defs"]))
        return None, ("second pass disagrees with check_contract_consumers' own "
                      "enumeration (cc-only=%s, this-only=%s) -- declared_in would be "
                      "an unverified shadow fact"
                      % (",".join(only_cc) or "-", ",".join(only_here) or "-"))
    hop_cache = {}
    buckets = {0: [], 1: [], 2: []}
    for name in sorted(model["defs"]):
        n = len(cc.consumer_crates(model, name, hop_cache))
        buckets[0 if n == 0 else 1 if n == 1 else 2].append(name)
    gaps = cc.compute_gaps(model)
    facts = {
        "defs": sorted(model["defs"]), "zero": buckets[0], "one": buckets[1],
        "ge2": buckets[2], "gaps": gaps, "locs": locs, "defs_n": len(model["defs"]),
    }
    return facts, None


def bare_names(baseline_path):
    """Baseline names registered WITHOUT a `# reason` -- advisory only.

    Read separately from cc.load_baseline because that helper deliberately strips
    comments to return a name set; the reason axis is a different question, and it
    must never influence a verdict (it is debt visibility, not a judgement).
    """
    out = set()
    try:
        with open(baseline_path, encoding="utf-8-sig", errors="ignore") as fh:
            for raw in fh:
                stripped = raw.strip()
                if not stripped or stripped.startswith("#"):
                    continue
                if "#" not in stripped:
                    out.add(stripped.split()[0])
    except OSError:
        return set()
    return out


def read_ledger(path):
    """(rows, claim, malformed). Row shape: `NAME | consumers=0 | declared_in=k`."""
    rows, malformed, claim = [], [], None
    with open(path, encoding="utf-8", errors="replace") as fh:
        for no, raw in enumerate(fh, 1):
            line = raw.replace("\r\n", "\n").strip()
            if not line:
                continue
            if line.startswith("#"):
                m = CLAIM_RE.match(line)
                if m:
                    claim = m.group(1).strip()
                continue
            parts = [p.strip() for p in line.split("|")]
            name = parts[0]
            if not NAME_RE.match(name):
                malformed.append("%s:%d row name is not a L0 def name: %r"
                                 % (os.path.basename(path), no, name[:40]))
                continue
            fields = {}
            bad = False
            for chunk in parts[1:]:
                if "=" not in chunk:
                    malformed.append("%s:%d field without '=': %r"
                                     % (os.path.basename(path), no, chunk[:40]))
                    bad = True
                    break
                k, v = chunk.split("=", 1)
                k = k.strip()
                if k not in ROW_FIELDS:
                    malformed.append("%s:%d unknown key %r (this ledger is derived, "
                                     "not annotated)" % (os.path.basename(path), no, k))
                    bad = True
                    break
                fields[k] = v.strip()
            if bad:
                continue
            if fields.get("consumers") != "0":
                malformed.append("%s:%d consumers=%r but only zero-consumer defs are "
                                 "listed here (>=1 is G-91's business, and a count "
                                 "here would go stale by construction)"
                                 % (os.path.basename(path), no, fields.get("consumers")))
                continue
            rows.append({"name": name, "declared_in": fields.get("declared_in", "")})
    return rows, claim, malformed


def judge(facts, rows, claim, baseline):
    out, fails = [], 0
    seen, dups = {}, []
    for r in rows:
        if r["name"] in seen:
            dups.append(r["name"])
        seen[r["name"]] = r["declared_in"]
    if dups:
        out.append("[TOOL-ERROR] duplicate ledger names: %s" % ", ".join(sorted(set(dups))))
        return 2, out
    # Dialect lock. If this gate and G-91 disagree about what a consumer is, no
    # verdict from either is meaningful -- undecidable, not red.
    if set(facts["zero"]) | set(facts["one"]) != set(facts["gaps"]):
        out.append("[DIALECT] zero-consumer U one-consumer != check_contract_consumers"
                   "'s own <2 gap set (%d+%d vs %d) -- the two gates no longer share a "
                   "consumer definition"
                   % (len(facts["zero"]), len(facts["one"]), len(facts["gaps"])))
        return 2, out
    want = dict((n, LOC_JOIN.join(facts["locs"][n])) for n in facts["zero"])
    for name in sorted(set(seen) - set(want)):
        out.append("[STALE] %r is no longer zero-consumer -- it gained a production "
                   "consumer; delete its line in the SAME batch that wired it" % name)
        fails += 1
    for name in sorted(set(want) - set(seen)):
        out.append("[NEW] %r has 0 production consumers and no ledger line -- it is "
                   "invisible to this gate (G-91 would absorb it into its exemption "
                   "register silently)" % name)
        fails += 1
    for name in sorted(set(want) & set(seen)):
        if seen[name] != want[name]:
            out.append("[LOC] %r declared_in=%r but live enumeration says %r -- the type "
                       "moved file, or the ledger was hand-edited"
                       % (name, seen[name], want[name]))
            fails += 1
    unregistered = sorted(set(facts["zero"]) - baseline)
    for name in unregistered:
        out.append("[UNREGISTERED] %r is zero-consumer but absent from G-91's baseline "
                   "-- the exemption register and this ledger disagree" % name)
    if unregistered:
        fails += len(unregistered)
    want_claim = claim_text(facts["defs_n"], len(facts["zero"]), len(facts["one"]),
                            len(facts["ge2"]), len(facts["gaps"]), len(baseline),
                            len(rows))
    if claim is None:
        out.append("[TOOL-ERROR] no '# CLAIM' header -- a derived ledger that does not "
                   "describe its own inputs cannot be audited: want '%s'" % want_claim)
        return 2, out
    if claim != want_claim:
        out.append("[CLAIM] ledger header says '%s' but live reality is '%s'"
                   % (claim, want_claim))
        fails += 1
    if fails:
        out.append("L0 zero-consumer ledger BROKEN: %d violation(s) -- %s"
                   % (fails, want_claim))
        return 1, out
    out.append("[OK] zero-consumer ledger matches the live scan: %s" % want_claim)
    return 0, out


def render(facts, baseline_n):
    claim = claim_text(facts["defs_n"], len(facts["zero"]), len(facts["one"]),
                       len(facts["ge2"]), len(facts["gaps"]), baseline_n,
                       len(facts["zero"]))
    lines = [_header(claim)]
    for name in facts["zero"]:
        lines.append("%s | consumers=0 | declared_in=%s"
                     % (name, LOC_JOIN.join(facts["locs"][name])))
    return "\n".join(lines) + "\n"


def _header(claim):
    return "\n".join([
        "# L0 zero-consumer contract ledger -- G-123 / ADR-220",
        "# (scripts/check_contract_zero_consumers.py)",
        "#",
        "# MACHINE-DERIVED. DO NOT HAND-EDIT. Regenerate with",
        "#     py -3 scripts/check_contract_zero_consumers.py --write",
        "# every field is recomputed from the scan on every run and diffed, so a",
        "# hand edit does not express an adjudication -- it just goes red.",
        "#",
        "# One line per nexus-contracts pub def with ZERO production consumer crates",
        "# under G-91's own counting dialect (check_contract_consumers.py imports:",
        "# build_model / consumer_crates / compute_gaps / DEF_RE / read_production_text).",
        "# Defs with exactly 1 consumer are NOT listed: they are registered in",
        "# scripts/contract_consumer_baseline.txt (human-owned) and judged by G-91.",
        "#",
        "#   NAME           as enumerated by cc.DEF_RE over the production surface",
        "#   consumers=0    fixed by construction; any other value is malformed",
        "#   declared_in    module key(s) relative to nexus-contracts/src, joined by",
        "#                  '+' when the same name is defined in more than one file",
        "#",
        "# Why exact-set and not a scalar ratchet: renaming a type moves one name out",
        "# and one in, so `zero_max=NN` stays green across a complete change of content.",
        "# Why there is no minimum: `zero` is the debt -- wiring types up drives it to",
        "# 0, and a floor there would make this gate's green depend on the defect.",
        "#",
        "# CLAIM " + claim,
    ])


def run_real(write=False, emit=False, ledger_path=LEDGER, crates_root=None):
    crates_root = Path(crates_root) if crates_root else ROOT / "crates"
    facts, err = scan(crates_root)
    if facts is None:
        print("[TOOL-ERROR] %s" % err)
        return 2
    # Floors BEFORE any read of the ledger or write of a new one: a collapsed scan
    # must never be able to launder a baseline (G-115/G-119 order discipline).
    blocked = floor_error(facts["defs_n"])
    if blocked:
        print("[TOOL-ERROR] %s" % blocked)
        return 2
    if emit or write:
        baseline = cc.load_baseline(BASELINE)
        target = ledger_path if write else CANDIDATE
        if not write:
            os.makedirs(os.path.dirname(CANDIDATE), exist_ok=True)
        with open(target, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(render(facts, len(baseline)))
        print("[DERIVED] %s rows=%d (machine-derived; hand-edit forbidden)"
              % (os.path.relpath(target, ROOT), len(facts["zero"])))
        return 0
    if not os.path.isfile(ledger_path):
        print("[TOOL-ERROR] ledger not found: %s -- generate it with --write"
              % ledger_path)
        return 2
    rows, claim, malformed = read_ledger(ledger_path)
    if malformed:
        for m in malformed:
            print("[TOOL-ERROR][malformed] %s" % m)
        print("[TOOL-ERROR] malformed ledger rows -> undecidable, exit 2")
        return 2
    baseline = cc.load_baseline(BASELINE)
    rc, out = judge(facts, rows, claim, baseline)
    print("[BUCKETS] defs=%d zero=%d one=%d ge2=%d sum=%s"
          % (facts["defs_n"], len(facts["zero"]), len(facts["one"]),
             len(facts["ge2"]),
             "OK" if len(facts["zero"]) + len(facts["one"]) + len(facts["ge2"])
             == facts["defs_n"] else "FAIL"))
    by_mod = {}
    for name in facts["zero"]:
        for key in facts["locs"][name]:
            by_mod[key] = by_mod.get(key, 0) + 1
    print("[BUCKETS] zero_by_module=%s"
          % ",".join("%s:%d" % (k, by_mod[k]) for k in sorted(by_mod)))
    bare = sorted(set(facts["zero"]) & bare_names(BASELINE))
    print("[REGISTER-DEBT] bare=%d -- advisory only (not counted in any verdict): "
          "these zero-consumer exemptions carry no reason comment in "
          "scripts/contract_consumer_baseline.txt" % len(bare))
    for line in out:
        print(line)
    return rc


def selftest():
    ok, n = True, 0

    def add(label, cond):
        nonlocal ok, n
        n += 1
        print("[%s] %s" % ("SELFTEST" if cond else "SELFTEST-FAIL", label))
        if not cond:
            ok = False

    # Same module object, not a re-imported copy: two module objects would mean two
    # hop caches and, worse, two independent readings of "what is a consumer".
    add("check_contract_consumers is the shared module object (no dialect copy)",
        cc is sys.modules["check_contract_consumers"])

    with tempfile.TemporaryDirectory(prefix="g123_") as tmp:
        crates = Path(tmp) / "crates"

        def put(rel, body):
            p = crates / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(body, encoding="utf-8")

        put("nexus-contracts/src/a.rs",
            "pub struct Alpha;\npub enum Beta { X }\npub trait Gamma {}\n"
            "pub struct Dup;\n"
            "#[cfg(test)]\nmod tests {\n    pub struct Hidden;\n}\n")
        put("nexus-contracts/src/b/nested.rs", "pub struct Solo;\npub struct Dup;\n")
        put("consumer-two/src/lib.rs",
            "use nexus_contracts::a::{Alpha, Beta};\nfn f(a: Alpha) { let _ = (a, Beta::X); }\n")
        put("consumer-three/src/lib.rs",
            "fn f() { let _ = nexus_contracts::a::Alpha; }\n")
        put("consumer-four/src/lib.rs",
            "fn f() { let _ = nexus_contracts::a::Beta; }\n")
        put("consumer-one/src/lib.rs",
            "fn f() { let _ = nexus_contracts::b::nested::Solo; }\n")
        put("noise/src/lib.rs",
            "// use nexus_contracts::a::Gamma; must not count\nfn f() {}\n")

        facts, err = scan(crates)
        add("scan on fixture succeeds", facts is not None and not err)
        if facts is None:
            print("[SELFTEST] FAILED (scan blind)")
            return 1
        add("second pass agrees with cc's enumeration (Hidden excluded by the tail mask)",
            set(facts["locs"]) == set(facts["defs"]))
        add("buckets exact: zero=[Dup,Gamma] one=[Solo] ge2=[Alpha,Beta]",
            facts["zero"] == ["Dup", "Gamma"] and facts["one"] == ["Solo"]
            and facts["ge2"] == ["Alpha", "Beta"])
        add("sum closes: zero+one+ge2 == defs",
            len(facts["zero"]) + len(facts["one"]) + len(facts["ge2"]) == facts["defs_n"])
        add("a name defined in two files is joined, not silently collapsed",
            facts["locs"]["Dup"] == ["a", "b/nested"])
        add("dialect lock holds on honest input (guard is not vacuously red)",
            set(facts["zero"]) | set(facts["one"]) == set(facts["gaps"]))

        base = Path(tmp) / "baseline.txt"
        base.write_text("# header\nAlpha\nDup\nGamma\nSolo\n", encoding="utf-8")
        baseline = cc.load_baseline(str(base))
        # One claim literal, not four: the fixture's header line is asserted against
        # the scan in several shapes, and copying it around is how a fixture ends up
        # testing a number nobody recomputes (same reason the gates stopped quoting
        # their own counts in note text).
        CLAIM_LINE = "# CLAIM defs=5 zero=2 one=1 ge2=2 gap=3 baseline=4 rows=2 sum=OK\n"
        ROW_DUP = "Dup | consumers=0 | declared_in=a+b/nested\n"
        ROW_GAMMA = "Gamma | consumers=0 | declared_in=a\n"
        GOOD = CLAIM_LINE + ROW_DUP + ROW_GAMMA
        ONE_ROW = CLAIM_LINE + ROW_DUP
        led = Path(tmp) / "ledger.txt"
        led.write_text(GOOD, encoding="utf-8")
        rows, claim, malformed = read_ledger(str(led))
        rc, _out = judge(facts, rows, claim, baseline)
        add("derived ledger matching the scan -> rc 0", rc == 0 and not malformed)

        def judge_with(text=None):
            nonlocal rows, claim
            if text is not None:
                led.write_text(text, encoding="utf-8")
                rows, claim, malformed = read_ledger(str(led))
                if malformed:
                    return 2, ["malformed"]
            return judge(facts, rows, claim, baseline)

        rc, out = judge_with(ONE_ROW)
        add("a zero-consumer def with no line is [NEW] red",
            rc == 1 and any("[NEW]" in l for l in out))
        extra = GOOD + "Alpha | consumers=0 | declared_in=a\n"
        rc, out = judge_with(extra)
        add("a line for a def that is no longer zero-consumer is [STALE] red",
            rc == 1 and any("[STALE]" in l for l in out))
        bad_loc = CLAIM_LINE + "Dup | consumers=0 | declared_in=a\n" + ROW_GAMMA
        rc, out = judge_with(bad_loc)
        add("a hand-edited declared_in is [LOC] red (the column is verified, not decoration)",
            rc == 1 and any("[LOC]" in l for l in out))
        bad_claim = ONE_ROW + "Gamma | consumers=0 | declared_in=a\n"
        bad_claim = bad_claim.replace("rows=2", "rows=99")
        rc, out = judge_with(bad_claim)
        add("header CLAIM disagreeing with the scan is [CLAIM] red",
            rc == 1 and any("[CLAIM]" in l for l in out))
        for label, line in (
            ("consumers!=0", "Dup | consumers=1 | declared_in=a"),
            ("unknown key", "Dup | consumers=0 | weight=3"),
            ("bad name", "dup | consumers=0 | declared_in=a"),
            ("field without '='", "Dup | consumers=0 | declared_in a"),
        ):
            text = (CLAIM_LINE + line + "\n"
                    + "Gamma | consumers=0 | declared_in=a\n")
            led.write_text(text, encoding="utf-8")
            _r, _c, malformed = read_ledger(str(led))
            add("%s is malformed -> rc 2 (never skipped silently)" % label,
                len(malformed) == 1)
        dup_row = "Dup | consumers=0 | declared_in=a\n"
        rc, out = judge_with(CLAIM_LINE + dup_row + dup_row)
        add("duplicate rows are undecidable -> rc 2",
            rc == 2 and any("duplicate" in l for l in out))
        rc, out = judge_with(ROW_DUP + ROW_GAMMA)
        add("a ledger with no CLAIM header is undecidable -> rc 2",
            rc == 2 and any("no '# CLAIM'" in l for l in out))
        judge_with(GOOD)  # reset rows/claim: the two tests above deliberately broke them
        rc, out = judge(facts, rows, claim, {"Alpha", "Solo"})
        add("a zero-consumer def missing from G-91's baseline is [UNREGISTERED] red",
            rc == 1 and any("[UNREGISTERED]" in l for l in out))
        add("advisory reader sees the bare (comment-less) baseline lines",
            bare_names(str(base)) == {"Alpha", "Dup", "Gamma", "Solo"} and
            len(set(facts["zero"]) & bare_names(str(base))) == 2)
        base2 = Path(tmp) / "baseline2.txt"
        base2.write_text("Dup  # wired later\nGamma  # pure contract, awaiting seam\n",
                         encoding="utf-8")
        add("reason-bearing baseline lines are NOT counted as debt",
            bare_names(str(base2)) == set())

        # Counterfactual: the dialect guard must be able to fire. On the real tree it
        # never does, so without this injection it is an unwired guard -- the exact
        # defect G-121 self-reported (a floor declared but never read by the judge).
        # Patching alone is not enough: facts.gaps is computed inside scan(), so the
        # scan has to run again under the patched function or the test proves nothing.
        led.write_text(GOOD, encoding="utf-8")
        rows, claim, _m = read_ledger(str(led))
        honest = cc.compute_gaps
        try:
            cc.compute_gaps = lambda model: ["Solo"]
            poisoned, _perr = scan(crates)
            rc, out = judge(poisoned, rows, claim, baseline)
            add("injected <2 gap set trips [DIALECT] -> rc 2 (guard has teeth)",
                poisoned is not None and rc == 2
                and any("[DIALECT]" in l for l in out))
        finally:
            cc.compute_gaps = honest
        facts_again, _ = scan(crates)
        rc, _out = judge(facts_again, rows, claim, baseline)
        add("after restore the same ledger judges rc 0 (guard is not vacuously red)",
            rc == 0 and cc.compute_gaps is honest)
        add("scan is deterministic across runs (the ledger is reproducible, not lucky)",
            (facts_again["zero"], facts_again["one"], facts_again["ge2"])
            == (facts["zero"], facts["one"], facts["ge2"]))
        rc, out = judge_with(ONE_ROW)
        add("after the injection round-trip a dropped line still reads [NEW] red",
            rc == 1 and any("[NEW]" in l for l in out))

        # The floor must be proven WIRED, not just defined: G-121 recorded a floor
        # that was declared and never read by the judge, which is a floor that cannot
        # fire. Driving run_real over the 5-def fixture is that proof, and the message
        # is checked so a refusal for some other reason cannot pass as the floor.
        _buf = io.StringIO()
        with contextlib.redirect_stdout(_buf):
            blocked_rc = run_real(crates_root=crates)
        add("run_real refuses the 5-def fixture with rc 2 (floor is wired, not decorative)",
            blocked_rc == 2 and "below floor" in _buf.getvalue())
        add("the refusal comes before any ledger read or write",
            "DERIVED" not in _buf.getvalue())

    live_facts, live_err = scan(ROOT / "crates")
    add("live scan succeeds", live_facts is not None)
    if live_facts is not None:
        # Both directions in ONE assertion whose label matches what it checks
        # (G-119's precedent). A bare `0 < DEFS_FLOOR` would be an assertion no
        # input can flip, and a label claiming more than the condition proves is
        # the failure class this repository has recorded twice in its own tests.
        add("floors: 0 < DEFS_FLOOR <= live defs (%d)" % live_facts["defs_n"],
            0 < DEFS_FLOOR <= live_facts["defs_n"])
        add("live sum closes and the dialect lock holds on reality",
            len(live_facts["zero"]) + len(live_facts["one"]) + len(live_facts["ge2"])
            == live_facts["defs_n"]
            and set(live_facts["zero"]) | set(live_facts["one"]) == set(live_facts["gaps"]))
        add("live zero-consumer set is a subset of G-91's register (no new hole today)",
            set(live_facts["zero"]) <= cc.load_baseline(BASELINE))
    rc = run_real(ledger_path=LEDGER)
    add("this gate on the real tree must be rc 0 (got %d)" % rc, rc == 0)
    print("[SELFTEST] %s (%d assertions)"
          % ("ALL ASSERTIONS HELD" if ok else "FAILED", n))
    return 0 if ok else 1


def main(argv):
    args = [a for a in argv[1:]]
    allowed = ("--selftest", "--emit", "--write")
    if any(a not in allowed for a in args):
        sys.stderr.write("usage: check_contract_zero_consumers.py "
                         "[--selftest|--emit|--write]\n")
        return 2
    if "--selftest" in args:
        return selftest()
    return run_real(write="--write" in args, emit="--emit" in args)


if __name__ == "__main__":
    import gate_rc  # entry-only: a crash must exit 2, never borrow 1 to fake "judged and red"

    sys.exit(gate_rc.run(lambda: main(sys.argv)))
