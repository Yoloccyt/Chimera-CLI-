#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L0 controlled-logic ledger (G-119 / G-120, ADR-218).

Why this gate exists
--------------------
ADR-033's purity wording was corrected from "zero logic" to "controlled pure
logic" (addendum 2026-10-09), and that addendum itself recorded that the word
"controlled" had NO machine judgement behind it. ADR-212 decision 3 closed half
of that gap by shipping G-99, but G-99 only judges (a) the dependency whitelist
and (b) forbidden symbols. It is blind to logic QUANTUM: adding a real algorithm
to L0 today keeps G-99 green, because an algorithm is not a forbidden symbol.

The registry that is supposed to bound "controlled logic" is
`ADR_033_EXCEPTION_MODULES` in nexus-contracts/src/lib.rs. It is cross-checked by
`tests/l0_exception_registry.rs`, but that test compares the registry against a
hand-maintained MIRROR of itself -- a self-consistency check, not a completeness
check. Nothing compares the registry against what the source actually does.
Measured on 2026-10-15: 6 modules carry real logic and are not registered
(sparse/masks, memory/quota, mca/affinity, mca/price_vector, seams/reward,
spec/formal_props), plus 4 more exposed by a second derivation pass.

What it judges
--------------
Completeness pairing, NOT a size ratchet. The key set is

    required = derived FN keys  U  derived TABLE keys
               U  one MOD row per registered module that has no derived key

and the ledger must be EXACTLY that set:
  * NEW    - a derived key with no ledger row -> red (this is what closes the gap:
             a new algorithm in L0 cannot land without someone signing a row).
  * STALE  - a row whose key is gone -> red (dropping logic must be deliberate).
  * CLASS_MISMATCH - class says trivial but a hard detector fired, or class says
             controlled-logic with no detector -> red. The ledger may not lie.
  * MECH   - the row's mechanism echo (mech/near/supp) disagrees with a fresh
             derivation -> red. This is the field that closes the hole a free-text
             `basis` left: the shipped ledger asserted "the LOOKUP veto suppresses"
             for three MOD rows and two of them were about functions with zero
             match arms, whose real cause is body length under D1/D3. A basis may
             now not narrate mechanism at all -- it is exit 2 when it does, because
             a reader cannot tell whether the prose or the field describes the tree.
  * CLAIM  - the header's machine-readable claim line disagrees with the rows or
             with the live parse -> red. A stale header is how
             l0_extensibility_class_freeze.txt:28 ended up asserting "ZERO
             #[non_exhaustive]" while 16 files carried the attribute.

Surface semantics are SHARED with G-99 and G-105 on purpose: the comment/string
lexer, the `#[cfg(test)]` tail mask and the module-key shape are imported, not
re-implemented. Two hand-written copies of "what counts as production code" is
the drift class this repository already fights.

Deliberate non-goals
--------------------
  * It does NOT judge "is this registered in lib.rs" as a violation, and it does
    not accept a `registered=` key in the ledger. lib.rs owns the registry (the
    script reads it live and prints REG=YES|NO); the ledger owns the class and
    the basis. That split is what keeps this from becoming a second truth source.
  * It does NOT re-judge clock/env reads: G-99 already exempts them by ruling
    (ADR-212 decision 3), and a second verdict there would conflict.

Modes / exit codes
------------------
  (default)  judge the real repo          0 = agree, 1 = NEW/STALE/CLASS/MECH/CLAIM
  --emit     write a candidate ledger under tmp/ (bootstrap; never overwrites)
  --write    rebuild the ledger in place, floors checked FIRST
  --selftest planted positive AND negative fixtures prove the detectors fire in
             both directions and stay silent on lookup accessors
  undecidable                             2 = missing/bad ledger, bad key, blind
             parser (floors), unresolvable registry name, a basis that narrates
             mechanism instead of citing it

Output is pure ASCII (project script convention; avoids Windows GBK console traps).
"""

import os
import re
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
# L0-A crate roster (ADR-222 D2); primary crate first. Siblings keep their
# original group directories, so the module keys stay byte-identical.
sys.path.insert(0, HERE)
import l0_roster  # noqa: E402  (single source for the L0-A roster; G-125 cross-checks it)
L0A_CRATES = l0_roster.L0A_DIR_NAMES
SRC_DIRS = [os.path.join(ROOT, "crates", name, "src") for name in L0A_CRATES]
CRATE_DIR = os.path.join(ROOT, "crates", L0A_CRATES[0])
SRC_DIR = SRC_DIRS[0]  # kept for fixtures / back-compat
LIB_RS = os.path.join(SRC_DIR, "lib.rs")
LEDGER = os.path.join(HERE, "l0_logic_ledger.txt")
CANDIDATE = os.path.join(ROOT, "tmp", "l0_logic_ledger_candidate.txt")

sys.path.insert(0, HERE)
import check_l0_purity as purity  # noqa: E402  (shares lexer + tail mask)
from check_l0_pubfn_ratchet import module_key  # noqa: E402  (shares key shape)

# --- floors: below these the parser is presumed blind, not clean --------------
# Set from the live measurement (64 files / 21058 src lines / ~60 derived fns),
# rounded DOWN hard so a legitimate shrink batch passes but a collapse does not.
FILES_FLOOR = 55
SRC_LINES_FLOOR = 18000
DERIVED_FN_FLOOR = 15
REG_RESOLVED_FLOOR = 12

# --- detectors ---------------------------------------------------------------
# A function is "substantive logic" if it trips ANY of D1..D6. Thresholds were
# calibrated against two rulings this gate must reproduce rather than contradict:
#   * lib.rs:499-500 -- "small accessors / enum as_str are trivial and not
#     registered", with vector.rs' "<=7 line micro helpers + 8 bodiless trait
#     declarations" as the named example. Measured live, vector.rs' largest
#     production body is 7 lines, so nothing in it may trip these detectors.
#   * ADR-213 D-7 -- SparseMask::select_top_k is in-domain controlled pure logic,
#     which D1/D2/D3/D5/D6 all catch independently.
D1 = (12, 2, 8)   # body lines >= 12 and (branches >= 2 or let/arith/cmp >= 8)
D2 = 5            # algorithm vocabulary hit and body lines >= 5
D3 = 25           # plainly long body
D5 = (9, 3)       # body lines >= 9 and comparisons >= 3
D6 = (2, 8)       # >= 2 `let` and body lines >= 8

# Closed algorithm vocabulary. Word-boundary anchored; prefix forms end in `_`.
VOCAB_RX = re.compile(r"""
    \b(?:
      select_nth_unstable\w* | sort\w* | binary_search | is_sorted
    | min_by | max_by | then_with | partial_cmp | position | fold | reduce
    | iter | iter_mut | into_iter | rev | windows | chunks | dedup | retain
    | drain | zip | enumerate | collect | newtype_vec
    | saturating_\w+ | checked_\w+ | wrapping_\w+ | rem_euclid
    | pow\w* | abs | clamp | sum | product | normalize\w* | hash\w*
    )\b
""", re.VERBOSE)

BR_RX = re.compile(r"\b(?:if|else|for|while|loop)\b")
LOOP_RX = re.compile(r"\b(?:for|while|loop)\b")
MATCH_RX = re.compile(r"\bmatch\b")
LET_RX = re.compile(r"\blet\b")
RET_RX = re.compile(r"\breturn\b")
CMP_RX = re.compile(r"(?:==|!=|<=|>=|&&|\|\|)")
ARITH_RX = re.compile(r"(?<![-<>=])[-+*/%](?![-=>])")
CALL_RX = re.compile(r"\b[A-Za-z_]\w*\s*\(")
ARROW_RX = re.compile(r"->")

FN_SIG_RE = re.compile(
    r"^[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:const\s+)?"
    r"fn\s+([A-Za-z_]\w*)\b")
# Module-level policy table axis (D7): `const NAME: [T; N]` with N >= TABLE_MIN.
# The `;` is matched with optional spaces because the live tree uses both the
# `[(RewardLayer, f32); 11]` and the `[Self; 4]` spellings.
# re.MULTILINE is load-bearing: the axis scans a joined block of unmasked lines,
# so without it `^` matches only the first line of the whole file and D7 goes
# silently blind -- a zero-hit detector reads as "no policy tables here", which is
# exactly the false green this gate exists to prevent. The selftest pins the count.
TABLE_RE = re.compile(
    r"^[ \t]*(?:pub\s+)?const\s+([A-Z][A-Z0-9_]*)\s*:\s*\[[^\]]*?\s*;\s*(\d+)\s*\]",
    re.MULTILINE)
# Measured live on 2026-10-16: L0 declares 18 `const [T; N]`; N>=8 selects exactly
# 3 (seams/reward ALL + LAYER_WEIGHTS, spec/harness_spec ALL_CONST). Lowering this
# would drag the ubiquitous `[Self; 4]` variant-enumeration idiom into the ledger
# and buy no coverage, because those modules' behaviour is already judged fn-wise.
TABLE_MIN = 8

REGISTRY_RE = re.compile(
    r"pub\s+const\s+ADR_033_EXCEPTION_MODULES\s*:\s*&\[&str\]\s*=\s*&\[(.*?)\]\s*;",
    re.DOTALL)
REG_ITEM_RE = re.compile(r"\"([A-Za-z_]\w*)\"")

CLASS_VOCAB = ("controlled-logic", "structural", "trivial", "pending",
               "env-ambient", "clock-read")
EXEMPT_VOCAB = ("NONE", "AMBIENT-INPUT", "IO-EFFECT", "SHARED-STATE")
ROW_KINDS = ("FN", "TABLE", "MOD")
# WHY these three fields exist. `detectors` says WHAT fired but not why a row has
# no detector, so the "why" was written as free text -- and measured 2026-10-16,
# that prose was wrong: all three shipped MOD rows asserted "the LOOKUP veto
# suppresses" their logic, while
#   memory/archive_monotonicity::assert_archive_monotonicity  bl=7  `=>`=0
#   memory/experience_card::normalize                         bl=7  `=>`=0
# trip no detector because of body length (D1 needs 12, D3 needs 25) and have no
# match arms to suppress at all. Only safety/errors::to_nexus_error (bl=36, `=>`=10,
# call=9) is genuinely LOOKUP-suppressed. mech/near/supp restate the cause from the
# same `detectors_of` + `is_lookup` the judge runs, so a mechanism claim is now
# machine diffed instead of believed.
MECH_VOCAB = ("DETECTED", "LOOKUP-SUPPRESSED", "BELOW-DETECTOR", "NO-BODY", "TABLE")
# The one-vote-veto wording is machine-owned now; repeating it in `basis` creates a
# second place to be wrong. A hit is undecidable (exit 2), not red -- a reader
# cannot tell whether the prose or the field describes the tree, and the row's
# whole purpose is to be reviewable.
BASIS_MECH_RX = re.compile(
    r"trips no detector|no detector|decision[ -]?table|match[ -]to[ -]variant"
    r"|lookup|veto|suppress|threshold|algorithm", re.IGNORECASE)
KNOWN_KEYS = ("fn", "table", "module", "detectors", "bl", "br", "ar",
              "mech", "near", "supp", "class", "basis", "exempt", "origin")


def metrics_of(body_lines):
    """Count the detector inputs over a function body (already blanked lines)."""
    text = "\n".join(body_lines)
    nonblank = [l for l in body_lines if l.strip()]
    # `->` would otherwise be read as minus-then-gt by ARITH_RX.
    arith = len(ARITH_RX.findall(ARROW_RX.sub(" ", text)))
    return {
        "bl": len(nonblank),
        "br": len(BR_RX.findall(text)),
        "loop": len(LOOP_RX.findall(text)),
        "mt": len(MATCH_RX.findall(text)),
        "ar": text.count("=>"),
        "let": len(LET_RX.findall(text)),
        "ret": len(RET_RX.findall(text)),
        "cmp": len(CMP_RX.findall(text)),
        "arith": arith,
        "call": len(CALL_RX.findall(text)),
        "vocab": len(VOCAB_RX.findall(text)),
    }


def is_lookup(m):
    """True = plain lookup table / accessor; the one-vote veto from lib.rs:499-500.

    Requires the ABSENCE of every computation signal, so it cannot swallow
    `DensityTier::from_rho` (a min_by nearest-tier search) even though that reads
    superficially like a table accessor -- that was a live misclassification in
    the pre-flight audit and is the reason CALL/ARITH/CMP/RET are all gated here.
    """
    return (m["ar"] >= 2 and m["br"] <= 1 and m["mt"] <= 1
            and m["let"] == 0 and m["arith"] == 0 and m["cmp"] == 0
            and m["ret"] == 0 and m["vocab"] == 0 and m["call"] <= m["ar"])


def detectors_of(m):
    hits = []
    if m["bl"] >= D1[0] and (m["br"] >= D1[1] or m["let"] + m["arith"] + m["cmp"] >= D1[2]):
        hits.append("D1")
    if m["vocab"] >= 1 and m["bl"] >= D2:
        hits.append("D2")
    if m["bl"] >= D3:
        hits.append("D3")
    if m["loop"] >= 1:
        hits.append("D4")
    if m["bl"] >= D5[0] and m["cmp"] >= D5[1]:
        hits.append("D5")
    if m["let"] >= D6[0] and m["bl"] >= D6[1]:
        hits.append("D6")
    return hits


# Every conjunct of every detector, as (detector, metric, getter, threshold). D1 is
# compound (body lines AND (branches OR computed mass)), so listing its conjuncts
# apart is the only way to answer "how close is this to firing" at all.
SHORTFALL_CANDS = (
    ("D1", "bl", lambda m: m["bl"], D1[0]),
    ("D1", "br", lambda m: m["br"], D1[1]),
    ("D1", "lac", lambda m: m["let"] + m["arith"] + m["cmp"], D1[2]),
    ("D2", "vocab", lambda m: m["vocab"], 1),
    ("D2", "bl", lambda m: m["bl"], D2),
    ("D3", "bl", lambda m: m["bl"], D3),
    ("D4", "loop", lambda m: m["loop"], 1),
    ("D5", "bl", lambda m: m["bl"], D5[0]),
    ("D5", "cmp", lambda m: m["cmp"], D5[1]),
    ("D6", "let", lambda m: m["let"], D6[0]),
    ("D6", "bl", lambda m: m["bl"], D6[1]),
)


def closest_shortfall(mod_fns):
    """`<fn>:<detector>.<metric>=<have>/<need>` for the threshold nearest to firing.

    Called only for a module whose every function trips nothing, which makes D3's
    body-line conjunct unmet by construction (bl >= D3 would have fired it), so the
    candidate list cannot be empty. The label is part of the sort key: fn+detector+
    metric is unique, which keeps the winner deterministic even though os.walk does
    not order directories.
    """
    cands = []
    for name, _hits, met in mod_fns:
        for det, metric, get, need in SHORTFALL_CANDS:
            have = get(met)
            if have < need:
                cands.append((have / float(need), have - need,
                              "%s:%s.%s=%d/%d" % (name, det, metric, have, need)))
    return max(cands)[2]


def mech_of(kind, hits, mod_fns):
    """(mech, near, supp) derived from the live parse -- never hand-written.

    `class` stays the human adjudication and `basis` the human citation; these three
    are the machine's own echo, and they are diffed field by field in judge() exactly
    like detectors/bl/br/ar. Precedence is deliberate: a module that has both a
    suppressed lookup and short bodies reports LOOKUP-SUPPRESSED, because that is the
    binding cause of its missing FN row.
    """
    if kind == "TABLE":
        return "TABLE", "-", "-"
    if hits:
        return "DETECTED", "-", "-"
    supp = sum(1 for _n, h, _m in mod_fns if h)
    if supp:
        return "LOOKUP-SUPPRESSED", "-", str(supp)
    if not mod_fns:
        return "NO-BODY", "-", "0"
    return "BELOW-DETECTOR", closest_shortfall(mod_fns), "0"


# A signature may wrap across lines. 13 was too small and produced FALSE NEGATIVES:
# `memory/token_evidence.rs:129` is a genuine 26-argument constructor whose opening
# brace sits ~28 lines below `pub fn new(`, so a short budget silently dropped real
# logic (the worst direction a completeness gate can fail in). Declarations are still
# cheap to reject because a `;` at depth 0 terminates the scan on its own line.
SIG_SCAN_BUDGET = 64
# Body span budget: the largest live L0 body measures 91 lines (harness_spec::validate);
# truncation returns the partial body, which under-flags rather than over-flags.
BODY_SCAN_BUDGET = 300


def body_open_index(lines, mask, start):
    """Index of the '{' that opens this fn's body, or None if it is a declaration.

    A trait method signature (`fn f(&self) -> T;`) has NO body. Latching onto the
    next unrelated '{' (an `impl` block, a struct, or the *next* method) fabricates
    a body out of foreign code: measured on 2026-10-16, L0 carries 28 such
    declarations, and `spec/formal_props.rs:196` produced a phantom 48-line
    `collect_results` sitting next to the genuine 46-line one -- which in turn made
    the dup_fns field report "two impl blocks" when the truth was a parse artifact.

    Terminator rule: a ';' at bracket depth 0. Depth matters because array types
    put a ';' inside brackets BEFORE the body brace -- `pub const fn all() ->
    [SeamId; 9] {` is a definition, and a naive "first ';' wins" test would drop it.
    """
    depth = 0
    for i in range(start, min(start + SIG_SCAN_BUDGET, len(lines))):
        if mask[i]:
            continue
        for ch in lines[i]:
            if ch in "([":
                depth += 1
            elif ch in ")]":
                depth -= 1
            elif ch == "{":
                if depth <= 0:
                    return i
            elif ch == ";" and depth <= 0:
                return None
    return None


def fn_bodies(lines, mask, start):
    """Body lines of the fn whose signature begins at `start`, or None.

    The signature may span lines; the body begins at the first depth-0 '{' that is
    not preceded by a depth-0 ';' (see body_open_index). Brace balancing runs over
    the blanked code so strings/comments cannot open a brace. A body we cannot close
    is returned empty, which biases toward NOT flagging -- the floors and the
    selftest are what make that safe.
    """
    opened = body_open_index(lines, mask, start)
    if opened is None:
        return None
    depth = 0
    body = []
    for i in range(opened, min(opened + BODY_SCAN_BUDGET, len(lines))):
        if mask[i]:
            continue
        line = lines[i]
        if i == opened:
            line = line[line.index("{"):]
        depth += line.count("{") - line.count("}")
        body.append(line)
        if depth <= 0:
            return body
    return body


def parse_file(path, src_dir):
    """(fn_keys, table_keys, stats) for one source file, production surface only."""
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        raw = fh.read()
    code = purity.blank_rust(raw)
    lines = code.splitlines()
    mask = purity.test_tail_mask(lines)
    mod = module_key(path, src_dir)
    fn_keys, table_keys, fns = [], [], []
    for i, line in enumerate(lines):
        if mask[i]:
            continue
        m = FN_SIG_RE.match(line)
        if not m:
            continue
        name = m.group(1)
        if name == "main":
            continue
        body = fn_bodies(lines, mask, i)
        if body is None:
            # A bodiless declaration (trait method sig, fn-pointer type). Not counted
            # as seen code: `fns_seen` must mean "bodies", or a 48-line phantom can
            # sit beside a 6-line real one and make dup_fns report legal Rust.
            continue
        met = metrics_of(body)
        hits = detectors_of(met)
        fns.append((mod, name, hits, met))
        if hits and not is_lookup(met):
            fn_keys.append((mod, name, hits, met))
    tbl_text = "\n".join(l for k, l in enumerate(lines) if not mask[k])
    for tm in TABLE_RE.finditer(tbl_text):
        try:
            n = int(tm.group(2))
        except ValueError:
            continue
        if n >= TABLE_MIN:
            table_keys.append((mod, tm.group(1), n))
    return fn_keys, table_keys, fns, len(lines)


def parse_tree(src_dir):
    """(fn_keys, table_keys, files, src_lines, all_fns) over the whole tree."""
    fn_keys, table_keys, all_fns, files, src_lines = [], [], [], 0, 0
    for dirpath, _dirnames, filenames in os.walk(src_dir):
        for fn in sorted(filenames):
            if not fn.endswith(".rs"):
                continue
            path = os.path.join(dirpath, fn)
            f_keys, t_keys, fns, nl = parse_file(path, src_dir)
            fn_keys.extend(f_keys)
            table_keys.extend(t_keys)
            all_fns.extend(fns)
            files += 1
            src_lines += nl
    return fn_keys, table_keys, all_fns, files, src_lines


def parse_tree_all(src_dirs):
    """Aggregate parse_tree over every L0-A crate (keys stay crate-relative)."""
    fn_keys, table_keys, all_fns, files, src_lines = [], [], [], 0, 0
    for src_dir in src_dirs:
        f, t, a, n, lines = parse_tree(src_dir)
        fn_keys.extend(f)
        table_keys.extend(t)
        all_fns.extend(a)
        files += n
        src_lines += lines
    return fn_keys, table_keys, all_fns, files, src_lines


def read_registry(path=LIB_RS):
    """The registered module names, read LIVE off lib.rs (never copied into the ledger).

    Count-free on purpose: the registry grew 17 -> 28 when ADR-218's P1-REG batch
    landed, and a selftest that asserted `== 17` went red on a perfectly healthy
    tree. The invariant is "every name resolves to exactly one module", not a
    number somebody measured once. Same reasoning keeps this docstring from
    quoting a count.
    """
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        text = fh.read()
    m = REGISTRY_RE.search(text)
    if not m:
        return None
    return sorted(set(REG_ITEM_RE.findall(m.group(1))))


def resolve_registry(names, src_dirs):
    """Map registry stems to module keys, printing every path actually opened.

    Bare-name resolution is a known false-red class in this repository (two
    different src/wal.rs exist repo-wide), so a stem with 0 or >1 hits is
    undecidable rather than guessed, and each hit is printed with its full path.
    Accepts one src dir (fixtures) or a list (production L0-A roster).
    """
    if isinstance(src_dirs, str):
        src_dirs = [src_dirs]
    out = {}
    notes = []
    for stem in names:
        hits = []
        for src_dir in src_dirs:
            for dirpath, _dirnames, filenames in os.walk(src_dir):
                for fn in sorted(filenames):
                    if fn == stem + ".rs":
                        hits.append((os.path.join(dirpath, fn), src_dir))
        if len(hits) != 1:
            notes.append("stem %r resolved to %d file(s): %s"
                         % (stem, len(hits),
                            "; ".join(h[0] for h in hits) if hits else "-"))
            continue
        out[stem] = module_key(hits[0][0], hits[0][1])
    return out, notes


def claim_of(fn_keys, table_keys, reg_mods, rows):
    """Machine-readable header claim; disagreement is CLAIM red.

    Accepts both row shapes (read_ledger's {"kind","key","fields"} wrappers and
    bare field dicts) so the emitted header and the judged header compute the same
    string -- if they diverged, --write would produce a ledger that is red on
    arrival and the first real judgement would look like a code defect.
    """
    classes = {}
    mechs = {}
    for r in rows:
        f = r.get("fields", r)
        c = f.get("class", "?")
        classes[c] = classes.get(c, 0) + 1
        k = f.get("mech", "?")
        mechs[k] = mechs.get(k, 0) + 1
    dist = "/".join("%s=%d" % (c, classes.get(c, 0)) for c in CLASS_VOCAB)
    mdist = "/".join("%s=%d" % (m, mechs.get(m, 0)) for m in MECH_VOCAB)
    # Two fns may share a name inside one module (two impl blocks, or an inherent
    # plus a trait impl) -- legal Rust, and the exact case where a set comparison
    # silently launders growth (G-105's multiset lesson). The ledger keeps one row
    # per key, so the COLLAPSE itself is pinned in the CLAIM: a third `new` moves
    # dup_fns and the header no longer describes the tree.
    distinct = len(set((m, n) for m, n, _h, _met in fn_keys))
    return ("derived_fns=%d dup_fns=%d tables=%d registered=%d rows=%d classes=%s "
            "mech=%s"
            % (len(fn_keys), len(fn_keys) - distinct, len(table_keys),
               len(reg_mods), len(rows), dist, mdist))


CLAIM_RE = re.compile(r"^#\s*CLAIM\s+(.*)$")


def required_keys(fn_keys, table_keys, reg_mods, all_fns):
    """The exact key set the ledger must equal (see docstring 'What it judges').

    `all_fns` is every production body in the tree, not only the derived ones: a MOD
    row's mechanism is precisely about the functions that did NOT become keys, so
    deriving it needs the whole module's shape.
    """
    by_mod = {}
    for mod, name, hits, met in all_fns:
        by_mod.setdefault(mod, []).append((name, hits, met))
    mods_with_keys = set(m for m, _n, _h, _met in fn_keys)
    mods_with_keys |= set(m for m, _t, _n in table_keys)
    req = {}
    for mod, name, hits, met in fn_keys:
        mech, near, supp = mech_of("FN", hits, by_mod.get(mod, []))
        req[("FN", "%s::%s" % (mod, name))] = {
            "detectors": "/".join(hits), "bl": str(met["bl"]),
            "br": str(met["br"]), "ar": str(met["ar"]),
            "mech": mech, "near": near, "supp": supp}
    for mod, name, n in table_keys:
        mech, near, supp = mech_of("TABLE", [], [])
        req[("TABLE", "%s::%s" % (mod, name))] = {
            "detectors": "D7", "bl": "0", "br": "0", "ar": str(n),
            "mech": mech, "near": near, "supp": supp}
    for stem, mod in sorted(reg_mods.items()):
        if mod not in mods_with_keys:
            mech, near, supp = mech_of("MOD", [], by_mod.get(mod, []))
            req[("MOD", mod)] = {"detectors": "NONE", "bl": "0", "br": "0",
                                 "ar": "0", "mech": mech, "near": near,
                                 "supp": supp, "origin": "registered"}
    return req


def read_ledger(path):
    """Return (rows, claim, malformed). Row shape: `KIND | k=v | k=v | ...`."""
    rows, malformed = [], []
    claim = None
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        for no, raw in enumerate(fh, 1):
            raw = raw.replace("\r\n", "\n")
            line = raw.rstrip("\n").strip()
            if not line:
                continue
            if line.startswith("#"):
                m = CLAIM_RE.match(line)
                if m:
                    claim = m.group(1).strip()
                continue
            parts = [p.strip() for p in line.split("|")]
            if len(parts) < 3 or parts[0] not in ROW_KINDS:
                malformed.append("%s:%d unknown row kind or too few fields: %r"
                                 % (os.path.basename(path), no, line[:70]))
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
                if k not in KNOWN_KEYS:
                    # `registered=` is the specific key this ledger must never
                    # carry: lib.rs is the only source for that fact.
                    malformed.append("%s:%d unknown key %r (second-truth-source guard)"
                                     % (os.path.basename(path), no, k))
                    bad = True
                    break
                fields[k] = v.strip()
            if bad:
                continue
            ident = fields.get("fn") or fields.get("table") or fields.get("module")
            if not ident:
                malformed.append("%s:%d row has no identity field" % (
                    os.path.basename(path), no))
                continue
            if ":" not in ident or "::" not in ident and fields.get("origin") != "registered":
                if "::" not in ident and fields.get("origin") != "registered":
                    malformed.append("%s:%d identity not MOD::NAME: %r"
                                     % (os.path.basename(path), no, ident))
                    continue
            rows.append({"kind": parts[0], "key": ident, "fields": fields})
    return rows, claim, malformed


def emit_row_line(kind, key, fields):
    if kind == "FN":
        head = "fn=%s" % key
    elif kind == "TABLE":
        head = "table=%s" % key
    else:
        head = "module=%s" % key
    order = ["detectors", "bl", "br", "ar", "mech", "near", "supp",
             "exempt", "class", "basis", "origin"]
    chunks = [kind, head]
    for k in order:
        if k in fields:
            chunks.append("%s=%s" % (k, fields[k]))
    return " | ".join(chunks)


def judge(fn_keys, table_keys, reg_mods, rows, claim, all_fns):
    req = required_keys(fn_keys, table_keys, reg_mods, all_fns)
    seen = {}
    out, fails = [], 0
    dup = []
    for r in rows:
        k = (r["kind"], r["key"])
        if k in seen:
            dup.append(r["key"])
        seen[k] = r["fields"]
    if dup:
        out.append("[TOOL-ERROR] duplicate ledger keys: %s" % ", ".join(sorted(set(dup))))
        return 2, out, {}
    for k in sorted(set(seen) - set(req), key=lambda x: x[1]):
        out.append("[STALE] %s row %r has no matching live construct -- drop it in the "
                   "SAME commit that removed the code" % (k[0], k[1]))
        fails += 1
    for k in sorted(set(req) - set(seen), key=lambda x: x[1]):
        d = req[k]
        out.append("[NEW] %s %r trips %s but has no ledger row -- L0 logic requires a "
                   "signed class+basis" % (k[0], k[1], d["detectors"]))
        fails += 1
    for k in sorted(set(req) & set(seen), key=lambda x: x[1]):
        got, want = seen[k], req[k]
        for field in ("detectors", "mech", "near", "supp"):
            if got.get(field, "") != want[field]:
                tag = "[STALE]" if field == "detectors" else "[MECH]"
                out.append("%s %s %r ledger says %s=%r but live derivation says %s=%r "
                           "(threshold drifted, code changed under the row, or the "
                           "mechanism was asserted rather than measured)"
                           % (tag, k[0], k[1], field, got.get(field, ""),
                              field, want[field]))
                fails += 1
        cls = got.get("class", "")
        det = want["detectors"]
        if cls not in CLASS_VOCAB:
            out.append("[TOOL-ERROR] %s %r class=%r outside the closed vocabulary %s"
                       % (k[0], k[1], cls, "/".join(CLASS_VOCAB)))
            return 2, out, {}
        if got.get("mech", "") not in MECH_VOCAB:
            out.append("[TOOL-ERROR] %s %r mech=%r outside the closed vocabulary %s -- "
                       "mech is machine-derived, so a value from outside it means the "
                       "row was hand-edited" % (k[0], k[1], got.get("mech", ""),
                                                "/".join(MECH_VOCAB)))
            return 2, out, {}
        if got.get("exempt", "NONE") not in EXEMPT_VOCAB:
            out.append("[TOOL-ERROR] %s %r exempt=%r outside the closed vocabulary"
                       % (k[0], k[1], got.get("exempt", "")))
            return 2, out, {}
        if det == "NONE" and cls == "controlled-logic" \
                and got.get("origin") != "registered":
            out.append("[CLASS_MISMATCH] %s %r claims controlled-logic but no detector "
                       "fires -- either the basis is wrong or the gate is blind"
                       % (k[0], k[1]))
            fails += 1
        # A MOD row is legal and expected on day one: a name in lib.rs' registry whose
        # module trips NO detector. The row + origin=registered is the machine-readable
        # form of "the registry asserts logic the derivation cannot see" -- which is why
        # required() is C U T U R and never C alone. WHY its cause is not taken at face
        # value: measured 2026-10-16 it differs per module (archive_monotonicity and
        # experience_card are BELOW-DETECTOR on body length, errors is genuinely
        # LOOKUP-SUPPRESSED) while all three rows' prose basis asserted one cause.
        if cls == "trivial" and det != "NONE" \
                and any(x in det.split("/") for x in ("D1", "D3", "D4", "D7")):
            out.append("[CLASS_MISMATCH] %s %r claims trivial but %s fired (a hard "
                       "detector: length, loop, branch mass or policy table)"
                       % (k[0], k[1], det))
            fails += 1
        # `trivial` means "no row needed", so a trivial row that ALSO has a hard
        # detector is self-contradictory; `structural` is the honest label for a
        # constant-data surface such as `[Self; 11]`, which carries no decision.
        if cls == "structural" and k[0] != "TABLE":
            out.append("[CLASS_MISMATCH] %s %r claims structural, which is reserved "
                       "for TABLE rows (constant data surface)" % (k[0], k[1]))
            fails += 1
        if len(got.get("basis", "")) < purity.MIN_EXEMPT_REASON_LEN:
            out.append("[TOOL-ERROR] %s %r basis shorter than %d chars -- a row without "
                       "a citation is a second place to lie"
                       % (k[0], k[1], purity.MIN_EXEMPT_REASON_LEN))
            return 2, out, {}
        narrated = BASIS_MECH_RX.search(got.get("basis", ""))
        if narrated:
            out.append("[TOOL-ERROR] %s %r basis narrates machine-owned mechanism "
                       "(%r) -- mech/near/supp carry the cause now, so the prose clause "
                       "is a second place to be wrong: delete it and keep the citation"
                       % (k[0], k[1], narrated.group(0)))
            return 2, out, {}
    stats = {"derived_fns": len(fn_keys), "tables": len(table_keys),
             "registered": len(reg_mods), "rows": len(rows)}
    want_claim = claim_of(fn_keys, table_keys, reg_mods, rows)
    if claim is None:
        out.append("[TOOL-ERROR] no '# CLAIM' header line -- the ledger may not be "
                   "un-verifiable about its own size: want '%s'" % want_claim)
        return 2, out, stats
    if claim != want_claim:
        out.append("[CLAIM] ledger header says '%s' but live reality is '%s'"
                   % (claim, want_claim))
        fails += 1
    if fails:
        out.append("L0 controlled-logic ledger BROKEN: %d violation(s) -- %s"
                   % (fails, want_claim))
        return 1, out, stats
    out.append("[OK] L0 controlled-logic ledger matches: %s" % want_claim)
    return 0, out, stats


def strip_mech_prose(basis):
    """Drop `;`-separated clauses that narrate mechanism; keep the citation.

    Returns (basis, stripped_count, refused). The refusal case is what makes this
    safe: when every clause narrates mechanism, deleting them all would turn a loud
    rc-2 into a silently short basis, so the row is left untouched and --write names
    it as [BASIS-KEEP] for a human to re-cite. Migration of the shipped ledger is
    therefore zero-hand-work exactly where it is honest and flagged where it is not.
    """
    if not BASIS_MECH_RX.search(basis):
        return basis, 0, False
    parts = [p.strip() for p in basis.split(";") if p.strip()]
    kept = [p for p in parts if not BASIS_MECH_RX.search(p)]
    joined = "; ".join(kept)
    if len(joined) < purity.MIN_EXEMPT_REASON_LEN:
        return basis, 0, True
    return joined, len(parts) - len(kept), False


def run_real(ledger_path=LEDGER):
    missing = [d for d in SRC_DIRS if not os.path.isdir(d)]
    if missing:
        print("[TOOL-ERROR] L0 source not found: %s" % ", ".join(missing))
        return 2
    fn_keys, table_keys, all_fns, files, src_lines = parse_tree_all(SRC_DIRS)
    names = read_registry()
    if names is None:
        print("[TOOL-ERROR] ADR_033_EXCEPTION_MODULES not found in %s -- the registry "
              "shape changed and this gate no longer knows what to read" % LIB_RS)
        return 2
    reg_mods, notes = resolve_registry(names, SRC_DIRS)
    emit_mode = "--emit" in sys.argv
    write_mode = "--write" in sys.argv
    # Floors BEFORE any write: a collapsed parse must never be able to launder a
    # baseline (G-115 order discipline -- emit-then-floor would let a blind parser
    # rewrite the ledger to match its own blindness).
    for label, got, floor in (("src files", files, FILES_FLOOR),
                              ("src lines", src_lines, SRC_LINES_FLOOR),
                              ("derived fns", len(fn_keys), DERIVED_FN_FLOOR),
                              ("registry modules resolved", len(reg_mods), REG_RESOLVED_FLOOR)):
        if got < floor:
            print("[TOOL-ERROR] %s == %d (below floor %d) -- parser or surface is "
                  "broken, refusing to judge and refusing to write" % (label, got, floor))
            return 2
    if notes:
        for n in notes:
            print("[TOOL-ERROR] registry name unresolvable: %s" % n)
        return 2
    if emit_mode or write_mode:
        # Both write paths need the committed ledger: --emit is the bootstrap that
        # carries human fields too, and rebuilding it from an absent ledger would
        # silently hand back a tree of class=pending rows for a ledger that already
        # exists -- the same laundering --write refuses to do.
        if not os.path.isfile(ledger_path):
            print("[TOOL-ERROR] %s not found -- %s needs the current ledger to carry "
                  "class/basis/exempt forward; refusing to rebuild human adjudication "
                  "out of nothing" % (ledger_path,
                                      "--write" if write_mode else "--emit"))
            return 2
        prev_rows, _prev_claim, prev_bad = read_ledger(ledger_path)
        if prev_bad:
            for m in prev_bad:
                print("[TOOL-ERROR][malformed] %s" % m)
            return 2
        human = {}
        for r in prev_rows:
            human[(r["kind"], r["key"])] = r["fields"]
        req = required_keys(fn_keys, table_keys, reg_mods, all_fns)
        out_rows = []
        stripped_total, keeps = 0, []
        for key in sorted(req, key=lambda x: (x[1], x[0])):
            kind, ident = key
            fields = dict(req[key])
            prev = human.get(key, {})
            fields["class"] = prev.get("class") or "pending"
            fields["exempt"] = prev.get("exempt") or "NONE"
            basis, stripped, refused = strip_mech_prose(
                prev.get("basis") or "TODO: cite the ADR or ruling")
            fields["basis"] = basis
            stripped_total += stripped
            if refused:
                keeps.append(ident)
            if kind == "MOD":
                fields["origin"] = "registered"
            out_rows.append({"kind": kind, "key": ident, "fields": fields})
        claim = claim_of(fn_keys, table_keys, reg_mods, out_rows)
        lines = [emit_row_line(r["kind"], r["key"], r["fields"]) for r in out_rows]
        target = ledger_path if write_mode else CANDIDATE
        if not write_mode:
            os.makedirs(os.path.dirname(CANDIDATE), exist_ok=True)
        with open(target, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(_header(claim) + "\n")
            for line in lines:
                fh.write(line + "\n")
        # Self-reported, never quoted in gate_manifest note text (ADR-218 D-44: a
        # count copied into prose rots before the code it describes does).
        print("[BASIS] stripped=%d clause(s) that narrated machine-owned mechanism"
              % stripped_total)
        for ident in keeps:
            print("[BASIS-KEEP] %s -- every clause narrates mechanism, so stripping "
                  "would drop the basis under %d chars: re-cite by hand"
                  % (ident, purity.MIN_EXEMPT_REASON_LEN))
        if write_mode:
            fresh = sum(1 for r in out_rows if r["fields"]["class"] == "pending")
            print("[WRITE] %d rows rebuilt at %s (%d left pending = keys with no "
                  "predecessor, %d basis(es) needing a hand citation)"
                  % (len(lines), target, fresh, len(keeps)))
        else:
            print("[EMIT] %d candidate rows at %s -- fill class/basis by hand, copy to "
                  "scripts/, then re-run (this file never overwrites the ledger)"
                  % (len(lines), os.path.relpath(target, ROOT)))
        return 0
    if not os.path.isfile(ledger_path):
        print("[TOOL-ERROR] ledger not found: %s" % ledger_path)
        return 2
    rows, claim, malformed = read_ledger(ledger_path)
    if malformed:
        for m in malformed:
            print("[TOOL-ERROR][malformed] %s" % m)
        print("[TOOL-ERROR] malformed ledger rows -> undecidable, exit 2")
        return 2
    rc, out, stats = judge(fn_keys, table_keys, reg_mods, rows, claim, all_fns)
    for line in out:
        print(line)
    print("[SCOPE] files=%d src_lines=%d fns_seen=%d derived_fns=%d tables=%d "
          "registered=%d resolved=%d" % (files, src_lines, len(all_fns),
                                         len(fn_keys), len(table_keys),
                                         len(names), len(reg_mods)))
    collapsed = len(fn_keys) - len(set((m, n) for m, n, _h, _met in fn_keys))
    if collapsed:
        print("[NOTE] %d derived fn(s) reuse a name inside their module (two impl "
              "blocks, or inherent plus trait impl -- legal Rust). One row per key by "
              "design; CLAIM pins the collapse as dup_fns so growth cannot hide."
              % collapsed)
    hit_mods = set(m for m, _n, _h, _met in fn_keys) | set(m for m, _t, _n in table_keys)
    for stem in sorted(reg_mods):
        flag = "REG=YES" if reg_mods[stem] in hit_mods else "REG=NO "
        print("  %s %s -> %s" % (flag, stem, reg_mods[stem]))
    # Advisory, deliberately not a verdict: flipping these into lib.rs' registry is a
    # crates/ edit, which ADR-218 defers to its own signed batch. Printing the number on
    # every run is what keeps the deferred gap from becoming invisible again -- the
    # silence is the failure mode this gate was built to end.
    gap = sorted(hit_mods - set(reg_mods.values()))
    print("[REGISTRY-GAP] %d module(s) carry derived logic but are absent from lib.rs "
          "ADR_033_EXCEPTION_MODULES: %s" % (len(gap), " ".join(gap) if gap else "-"))
    return rc


def _header(claim):
    return "\n".join([
        "# L0 controlled-logic ledger -- G-119 / ADR-218 (scripts/check_l0_logic_ledger.py)",
        "#",
        "# Every module in crates/nexus-contracts/src that carries real logic must have",
        "# exactly one row here. Key set is C U T U (registered modules with no derived key)",
        "# and the gate requires EXACT set equality in both directions.",
        "#",
        "#   FN     a function that trips D1..D6 after the LOOKUP suppression",
        "#   TABLE  a module-level `const NAME: [T; N]` policy table with N>=8 (D7)",
        "#   MOD    a name registered in lib.rs ADR_033_EXCEPTION_MODULES whose module",
        "#          trips no detector at all -- the registry asserts logic, so the row",
        "#          exists to make that assertion reviewable rather than invisible",
        "#",
        "# class:  %s" % "/".join(CLASS_VOCAB),
        "#         structural is reserved for TABLE rows holding constant data (an",
        "#         enumeration such as `[Self; 11]` carries no decision, so calling it",
        "#         controlled-logic would overstate it and calling it trivial would",
        "#         contradict it having a row at all).",
        "# exempt: %s (reuses G-99's IMPURITY_CATEGORIES; NONE for most rows)"
        % "/".join(EXEMPT_VOCAB),
        "# mech/near/supp: MACHINE ECHO -- why this row has the detectors it has.",
        "#         mech: %s" % "/".join(MECH_VOCAB),
        "#         near: for BELOW-DETECTOR, the closest threshold anywhere in the",
        "#               module as `<fn>:<D>.<metric>=<have>/<need>`; `-` elsewhere.",
        "#         supp: for LOOKUP-SUPPRESSED, how many of the module's functions trip",
        "#               a detector and are then vetoed as plain lookups.",
        "#         Diffed field by field against a fresh derivation every run, exactly",
        "#         like detectors/bl/br/ar -- so hand-editing one goes [MECH] red.",
        "# basis:  >= %d chars, and it must cite. A row without a citation is just"
        % purity.MIN_EXEMPT_REASON_LEN,
        "#         another place for the ledger to become a lie. It may NOT narrate",
        "#         mechanism (no detector / decision table / lookup / veto / suppress /",
        "#         threshold / algorithm ...): mech carries that now, and the shipped",
        "#         ledger proved such prose can be wrong. A hit is exit 2, not red --",
        "#         a reader cannot tell which of the two describes the tree.",
        "#",
        "# DO NOT add a `registered=` key. scripts/check_l0_purity.py and lib.rs own",
        "# those facts; duplicating them here creates a second truth source, which is",
        "# the failure class this repository keeps getting bitten by. The gate reads",
        "# the live registry itself and prints REG=YES|NO per name.",
        "#",
        "# OPENING A ROW is a deliberate act: adding a row with class=controlled-logic",
        "# requires a basis, and flipping class=trivial while a hard detector (D1/D3/D4/D7)",
        "# still fires is CLASS_MISMATCH red.",
        "#",
        "# Rebuild machine fields without losing adjudication: --write carries",
        "# class/basis/exempt forward by key, refreshes detectors/bl/br/ar, derives",
        "# mech/near/supp, and strips mechanism narration from basis -- naming on disk",
        "# any row whose basis cannot survive the strip, so migration is either honest",
        "# and automatic, or flagged and human.",
        "# CLAIM " + claim])


# --- selftest ---------------------------------------------------------------

POS_HEAVY = """
pub fn rank_desc(vals: &mut [u32]) -> u32 {
    let n = vals.len();
    if n == 0 {
        return 0;
    }
    vals.sort_unstable_by(|a, b| b.cmp(a));
    let mut total = 0;
    for i in 0..n {
        let v = vals[i];
        if v > 10 {
            total += v * 2;
        } else if v > 5 {
            total += v;
        } else {
            total = total.saturating_add(1);
        }
    }
    let half = n / 2;
    let mid = vals[half];
    total + mid as u32
}
"""

POS_TABLE = """
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind { A, B, C, D, E, F, G, H, I, J }

pub const POLICY: [(Kind, u8); 9] = [
    (Kind::A, 1),
    (Kind::B, 2),
];
"""

NEG_LOOKUPS = """
pub enum Tier { Rho1, Rho2, Rho5, Rho10 }

impl Tier {
    pub fn rho(&self) -> f64 {
        match self {
            Tier::Rho1 => 1.0,
            Tier::Rho2 => 2.0,
            Tier::Rho5 => 5.0,
            Tier::Rho10 => 10.0,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Rho1 => "rho1",
            Tier::Rho2 => "rho2",
            Tier::Rho5 => "rho5",
            Tier::Rho10 => "rho10",
        }
    }
    pub fn is_hot(&self) -> bool {
        matches!(self, Tier::Rho1 | Tier::Rho2)
    }
}

pub trait Store {
    type Meta;
    type Error;
    fn upsert(&mut self, id: &str, v: &[f32]) -> Result<(), Self::Error>;
    fn top_k(&self, q: &[f32], k: usize) -> Vec<u32>;
    fn remove(&mut self, id: &str) -> bool;
    fn len(&self) -> usize;
}
"""

# A match whose arms compute must NOT be suppressed as a lookup.
POS_BRANCHY = """
pub fn scale_up(mode: u8, x: f64) -> f64 {
    let base = x * 1.5;
    let mut acc = 0.0;
    match mode {
        0 => acc = base + x / 2.0,
        1 => acc = base * 2.0 - 1.0,
        2 => acc = (x - base).abs() + 3.0,
        _ => acc = x,
    }
    let half = acc / 2.0;
    if half > 1.0 && acc != 0.0 {
        return acc + half;
    }
    acc
}
"""

NEG_TESTS_TAIL = """
#[cfg(test)]
mod tests {
    pub fn helper_in_tail(vals: &mut [u32]) -> u32 {
        let mut total = 0;
        for i in 0..30 {
            let v = vals[i % vals.len()];
            if v > 10 {
                total += v * 2 + 7;
            } else if v > 5 {
                total += v;
            } else {
                total = total.saturating_add(1);
            }
            let half = total / 2;
            total = half + v;
        }
        total
    }
}
"""

NEG_DOC = """
/// Pseudo-code from a design doc must never count:
/// ```text
/// pub fn ghost_from_docs(x: u32) -> u32 {
///     let mut acc = 0;
///     for i in 0..x { acc += i * 2; }
///     acc
/// }
/// ```
pub fn real_and_short(x: u32) -> u32 {
    x + 1
}
"""

# Regression guard for the phantom-body bug: a bodiless trait declaration followed
# closely by an impl of the SAME method name used to latch onto the impl block's
# brace and invent a body out of foreign code (measured: spec/formal_props.rs:196
# produced bl=48 next to the genuine bl=46). The array-type case in the same file is
# the opposite guard -- `[SeamId; 9]` puts a ';' BEFORE the body brace, so a naive
# "first ';' means declaration" test would silently drop a real definition.
NEG_DECL_THEN_IMPL = """
pub trait Probe {
    /// doc for a bodiless declaration
    fn measured(&self) -> u32;
}

pub struct Holder;

impl Probe for Holder {
    fn measured(&self) -> u32 {
        let mut acc = 0;
        for i in 0..9 {
            acc += i;
        }
        acc
    }
}

pub enum SeamId { A, B, C, D, E, F, G, H, I }

impl SeamId {
    pub const fn all() -> [SeamId; 9] {
        [SeamId::A, SeamId::B]
    }
}
"""


def _fixture_tree(root, newline):
    src = os.path.join(root, "src")
    os.makedirs(os.path.join(src, "planted"), exist_ok=True)
    os.makedirs(os.path.join(src, "neg"), exist_ok=True)
    files = {
        "planted/heavy.rs": POS_HEAVY,
        "planted/table.rs": POS_TABLE,
        "planted/lookup_long.rs": POS_LOOKUP_LONG,
        "neg/lookups.rs": NEG_LOOKUPS,
        "neg/branchy.rs": POS_BRANCHY,
        "neg/tests_tail.rs": NEG_TESTS_TAIL,
        "neg/doc_only.rs": NEG_DOC,
        "neg/decl_then_impl.rs": NEG_DECL_THEN_IMPL,
        "neg/longsig.rs": POS_LONGSIG,
        "lib.rs": "pub mod planted;\npub mod neg;\n",
    }
    for rel, text in files.items():
        path = os.path.join(src, rel)
        with open(path, "w", encoding="utf-8", newline=newline) as fh:
            fh.write(text)
    return src


# The LOOKUP-suppressed shape, generated rather than hand-typed: 30 arms keep the
# body over D3's 25-line floor by construction, so the fixture cannot silently drop
# below it if someone edits an arm. This is the one shape whose detectors are NON-empty
# and which is_lookup still vetoes -- i.e. the only way to plant a MOD row whose mech is
# LOOKUP-SUPPRESSED instead of BELOW-DETECTOR.
POS_LOOKUP_LONG = (
    "pub enum Code { %s }\n\n" % ", ".join("V%d" % i for i in range(30))
    + "impl Code {\n"
      "    pub fn name(&self) -> &'static str {\n"
      "        match self {\n"
    + "".join('            Code::V%d => "v%02d",\n' % (i, i) for i in range(30))
    + "        }\n"
      "    }\n"
      "}\n"
)


# False-negative guard for the signature-scan budget: a wide constructor whose
# opening brace sits far below `pub fn new(` must still be measured. The live tree
# has exactly this shape at memory/token_evidence.rs:129 (26 arguments).
POS_LONGSIG = (
    "pub struct Wide { pub a: u32, pub total: u32 }\n\n"
    "impl Wide {\n"
    "    #[allow(clippy::too_many_arguments)]\n"
    "    pub fn build(\n"
    + "".join("        p%d: u32,\n" % i for i in range(24))
    + "    ) -> Self {\n"
      "        let mut acc = 0;\n"
      "        for i in 0..p3 {\n"
      "            acc += i;\n"
      "        }\n"
      "        Wide { a: acc, total: acc }\n"
      "    }\n"
      "}\n"
)


def selftest():
    ok = True
    n = 0

    def add(label, cond):
        nonlocal ok, n
        n += 1
        if cond:
            print("[SELFTEST] %s" % label)
        else:
            ok = False
            print("[SELFTEST-FAIL] %s" % label)

    tmp = tempfile.mkdtemp(prefix="l0logic-")
    try:
        src = _fixture_tree(tmp, "\n")
        fn_keys, table_keys, all_fns, _files, _lines = parse_tree(src)
        fk = set("%s::%s" % (m, name) for m, name, _h, _met in fn_keys)
        add("planted positive: 40-line sort/loop fn is derived",
            "planted/heavy::rank_desc" in fk)
        add("planted positive: branchy match-with-arithmetic is derived "
            "(LOOKUP suppression must not swallow computation in arms)",
            "neg/branchy::scale_up" in fk)
        add("planted positive: D7 table axis has teeth (const [K;9] enters via TABLE)",
            any(m == "planted/table" and name == "POLICY"
                for m, name, _n in table_keys))
        add("planted negative: lookup accessors + as_str + bodiless trait decls "
            "produce ZERO derived fns (strict equality, no 'or empty' escape)",
            "neg/lookups::rho" not in fk and "neg/lookups::as_str" not in fk
            and "neg/lookups::is_hot" not in fk and "neg/lookups::upsert" not in fk
            and len([1 for m, _n, _h, _met in fn_keys if m == "neg/lookups"]) == 0)
        add("planted negative: #[cfg(test)] tail cannot smuggle logic",
            "neg/tests_tail::helper_in_tail" not in fk)
        add("planted negative: fn inside a doc comment is not real code",
            "neg/doc_only::ghost_from_docs" not in fk
            and "neg/doc_only::real_and_short" not in fk)
        # Phantom-body guard (the defect that made dup_fns lie about formal_props).
        decl_seen = [x for x in all_fns if x[1] == "measured"]
        add("bodiless trait declaration yields exactly ONE entry, not decl+impl",
            len(decl_seen) == 1)
        add("the surviving `measured` is the genuine impl body (D4 fires, bl<=8)",
            len(decl_seen) == 1 and any("D4" in h for _m, _n, h, _met in decl_seen)
            and decl_seen[0][3]["bl"] <= 8)
        add("the genuine declaration-body pair still ENTERS the derived set "
            "(guard must not throw away real logic while killing phantoms)",
            "neg/decl_then_impl::measured" in fk)
        alls = [x for x in all_fns if x[1] == "all"]
        add("array-type signature `-> [T; 9] {` is NOT mistaken for a declaration",
            len(alls) == 1 and alls[0][3]["bl"] >= 1)
        # Budget guard: a 24-parameter wrapped signature must still be measured.
        wide = [x for x in all_fns if x[1] == "build"]
        add("24-line wrapped signature is measured, not dropped as a declaration",
            len(wide) == 1 and wide[0][3]["bl"] >= 5
            and any("D4" in h for _m, _n, h, _met in wide))
        add("fixture scope sanity: fns seen covers positive+negative bodies",
            len(all_fns) >= 6)
        add("VOCAB actually fires on select_nth_unstable/sort (D2 detector not dead)",
            any("D2" in h for _m, _n, h, _met in fn_keys))

        # Judge direction, on a hand-made key set derived from the fixture. FIX_REG
        # deliberately registers the two modules that produce NO FN key, so the MOD
        # branch is exercised twice with two different mechanisms (one suppressed by
        # the lookup veto, one simply below every detector).
        FIX_REG = {"lookup_stub": "planted/lookup_long", "neg_stub": "neg/lookups"}
        ID_FIELD = {"FN": "fn", "TABLE": "table", "MOD": "module"}
        rows_ok = []
        req = required_keys(fn_keys, table_keys, FIX_REG, all_fns)

        def row_for(kind, key, **over):
            """A row that matches the live parse, with exactly one injected defect."""
            fields = dict(req[(kind, key)])
            fields["class"] = "controlled-logic"
            fields["exempt"] = "NONE"
            fields["basis"] = "fixture basis for selftest only"
            fields[ID_FIELD[kind]] = key
            fields.update(over)
            return {"kind": kind, "key": key, "fields": fields}

        for (_kind, key), f in req.items():
            fields = dict(f)
            fields["class"] = "controlled-logic"
            fields["exempt"] = "NONE"
            fields["basis"] = "fixture basis for selftest only"
            fields[ID_FIELD[_kind]] = key
            rows_ok.append({"kind": _kind, "key": key, "fields": fields})
        claim = claim_of(fn_keys, table_keys, FIX_REG, rows_ok)
        rc, _out, _ = judge(fn_keys, table_keys, FIX_REG, rows_ok, claim, all_fns)
        add("fixture rows matching the live parse -> rc 0", rc == 0)
        # NEW: drop one row -> red.
        rc, out, _ = judge(fn_keys, table_keys, FIX_REG, rows_ok[:-1], claim, all_fns)
        add("missing row is caught as NEW (this is the gap the gate exists to close)",
            rc == 1 and any("[NEW]" in l for l in out))
        # STALE: extra row -> red.
        bogus = {"kind": "FN", "key": "planted/heavy::gone_now",
                 "fields": {"fn": "planted/heavy::gone_now", "detectors": "D1",
                            "mech": "DETECTED", "near": "-", "supp": "-",
                            "class": "controlled-logic", "exempt": "NONE",
                            "basis": "stale row fixture basis"}}
        rc, out, _ = judge(fn_keys, table_keys, FIX_REG, rows_ok + [bogus], claim,
                           all_fns)
        add("row whose construct vanished is caught as STALE",
            rc == 1 and any("[STALE]" in l for l in out))
        # CLASS_MISMATCH: trivial label on a hard detector.
        mutated = []
        for r in rows_ok:
            r2 = {"kind": r["kind"], "key": r["key"], "fields": dict(r["fields"])}
            mutated.append(r2)
        for r in mutated:
            if r["key"].endswith("rank_desc"):
                r["fields"]["class"] = "trivial"
        rc, out, _ = judge(fn_keys, table_keys, FIX_REG, mutated, claim, all_fns)
        add("class=trivial while D1/D3 fired is CLASS_MISMATCH (ledger cannot lie)",
            rc == 1 and any("[CLASS_MISMATCH]" in l for l in out))
        # CLAIM drift.
        rc, out, _ = judge(fn_keys, table_keys, FIX_REG, rows_ok,
                           "derived_fns=999 tables=0 registered=1 rows=999", all_fns)
        add("header CLAIM disagreeing with live parse is red",
            rc == 1 and any("[CLAIM]" in l for l in out))
        # Undecidable states must exit 2, never 1 -- and each assertion names the
        # message it expects, or a defect caught for the wrong reason reads as a pass.
        rd = ("FN", "planted/heavy::rank_desc")
        rc, out, _ = judge(fn_keys, table_keys, FIX_REG, [row_for(*rd, basis="x")],
                           claim, all_fns)
        add("basis shorter than MIN_EXEMPT_REASON_LEN -> rc 2, named as such",
            rc == 2 and any("basis shorter than" in l for l in out))
        rc, out, _ = judge(fn_keys, table_keys, FIX_REG,
                           [row_for(*rd, **{"class": "maybe"})], claim, all_fns)
        add("class outside the closed vocabulary -> rc 2, named as such",
            rc == 2 and any("class=" in l and "outside the closed vocabulary" in l
                            for l in out))
        rc, out, _ = judge(fn_keys, table_keys, FIX_REG,
                           [row_for(*rd, mech="GUESSED")], claim, all_fns)
        add("mech outside the closed vocabulary -> rc 2 (it is derived, not typed)",
            rc == 2 and any("mech=" in l and "hand-edited" in l for l in out))
        rc, out, _ = judge(
            fn_keys, table_keys, FIX_REG,
            [row_for(*rd, basis="its logic is match-to-variant decision tables "
                                "the LOOKUP veto suppresses per the old ledger")],
            claim, all_fns)
        add("a basis that narrates mechanism -> rc 2 (wording copied from the shipped "
            "ledger, so the ban is proven against the sentence it exists for)",
            rc == 2 and any("narrates machine-owned mechanism" in l for l in out))
        rc, out, _ = judge(fn_keys, table_keys, FIX_REG,
                           [row_for(*rd, mech="LOOKUP-SUPPRESSED")], claim, all_fns)
        add("mech asserted against a live BELOW/DETECTED reality is [MECH] red, not 2",
            rc == 1 and any(l.startswith("[MECH]") for l in out))
        mrow = ("MOD", "neg/lookups")
        rc, out, _ = judge(fn_keys, table_keys, FIX_REG,
                           [row_for(*mrow, near="made-up:D9.bl=99/100")], claim,
                           all_fns)
        add("a hand-written `near` is [MECH] red (the gap is measured, not claimed)",
            rc == 1 and any(l.startswith("[MECH]") and "near" in l for l in out))

        # Mechanism derivation itself: each MECH_VOCAB value needs its own fixture,
        # otherwise a constant return value would satisfy every assertion above.
        lk = [x for x in all_fns if x[0] == "planted/lookup_long"]
        add("planted lookup table trips a detector yet is_lookup vetoes it",
            len(lk) == 1 and bool(lk[0][2]) and is_lookup(lk[0][3]))
        add("the vetoed function produces no FN key (that is what suppression means)",
            "planted/lookup_long::name" not in fk)
        add("LOOKUP-SUPPRESSED: mech names the veto and counts it",
            req[("MOD", "planted/lookup_long")]["mech"] == "LOOKUP-SUPPRESSED"
            and req[("MOD", "planted/lookup_long")]["supp"] == "1")
        below = req[("MOD", "neg/lookups")]
        add("BELOW-DETECTOR: mech reports the closest threshold, no veto counted",
            below["mech"] == "BELOW-DETECTOR" and below["supp"] == "0"
            and below["near"].count(":") == 1 and below["near"].count("/") == 1)
        add("BELOW-DETECTOR near names a function of that module",
            below["near"].split(":")[0] in ("rho", "as_str", "is_hot"))
        add("DETECTED / TABLE are the row-kind echoes, never a human choice",
            req[rd]["mech"] == "DETECTED"
            and req[("TABLE", "planted/table::POLICY")]["mech"] == "TABLE")
        add("NO-BODY: a registered module with no production body says so",
            mech_of("MOD", [], [])[0] == "NO-BODY")
        add("mech_of precedence: a veto outranks short bodies in the same module",
            mech_of("MOD", [], [("tiny", [], {"bl": 1, "br": 0, "loop": 0, "mt": 0,
                                              "ar": 0, "let": 0, "ret": 0, "cmp": 0,
                                              "arith": 0, "call": 0, "vocab": 0}),
                                ("tab", ["D3"], {"bl": 30, "br": 0, "loop": 0,
                                                 "mt": 1, "ar": 30, "let": 0,
                                                 "ret": 0, "cmp": 0, "arith": 0,
                                                 "call": 0, "vocab": 0})])[0]
            == "LOOKUP-SUPPRESSED")
        add("CLAIM carries the mech distribution, not only the row counts",
            "LOOKUP-SUPPRESSED=1" in claim and "BELOW-DETECTOR=1" in claim
            and "TABLE=1" in claim)
        # Migration: automatic where it is honest, refused where it would erase the
        # citation. Both directions must be pinned or --write is a blind rewrite.
        kept_basis, stripped, refused = strip_mech_prose(
            "registered in lib.rs but trips no detector: its logic is match-to-variant "
            "decision tables the LOOKUP veto suppresses; ADR-218 records the blind axis")
        add("--write migration drops the narrating clause and keeps the citation",
            kept_basis == "ADR-218 records the blind axis" and stripped == 1
            and not refused)
        same_basis, stripped2, refused2 = strip_mech_prose(
            "trips no detector because the LOOKUP veto suppresses it")
        add("migration refuses to empty a basis that is nothing but mechanism",
            refused2 and stripped2 == 0 and same_basis.startswith("trips no detector"))
        add("a pure citation basis is untouched (the ban must not eat provenance)",
            strip_mech_prose("ADR-218 D-1 closes the pending-adjudication marker")
            == ("ADR-218 D-1 closes the pending-adjudication marker", 0, False))

        # CRLF must not change a single verdict (Windows worktree mixes 11/53).
        tmp2 = tempfile.mkdtemp(prefix="l0logic-crlf-")
        try:
            src2 = _fixture_tree(tmp2, "\r\n")
            fn2, tbl2, _a2, _f2, _l2 = parse_tree(src2)
            # Projected compare: sorting the raw tuples would try `<` on a dict
            # whenever two fns share a module and name, which is legal Rust.
            def proj(ks):
                return [(m, n, tuple(h)) for m, n, h, _met
                        in sorted(ks, key=lambda x: (x[0], x[1]))]
            add("CRLF fixture yields identical derived fns",
                proj(fn2) == proj(fn_keys))
            add("CRLF fixture yields identical D7 table axis",
                sorted(tbl2) == sorted(table_keys))
        finally:
            shutil.rmtree(tmp2, ignore_errors=True)

        # Ledger reader: `registered=` key must be malformed (second truth source).
        led = os.path.join(tmp, "led.txt")
        with open(led, "w", encoding="utf-8") as fh:
            fh.write("# CLAIM derived_fns=1 tables=0 registered=0 rows=1\n")
            fh.write("FN | fn=a/b::x | detectors=D1 | bl=9 | br=2 | ar=0 | "
                     "class=controlled-logic | basis=a basis long enough here | "
                     "registered=yes\n")
        rows, claim2, malformed = read_ledger(led)
        add("a `registered=` key in the ledger is malformed (second-truth-source guard)",
            bool(malformed) and rows == [])
        with open(led, "w", encoding="utf-8") as fh:
            fh.write("# CLAIM derived_fns=1 tables=0 registered=0 rows=1\n")
            fh.write("FN | fn=a/b::x | detectors=D1 | bl=9 | class=controlled-logic | "
                     "basis=a basis long enough here\n")
            fh.write("Mystery | fn=z\n")
        rows, claim2, malformed = read_ledger(led)
        add("unknown row kind is malformed rather than skipped",
            len(malformed) == 1 and len(rows) == 1 and claim2 is not None)

        # The floors must actually be able to fire, and be below the live tree.
        live_fn, live_tbl, live_all, live_files, live_lines = parse_tree_all(SRC_DIRS)
        names = read_registry()
        reg_mods, notes = resolve_registry(names, SRC_DIRS) if names else ({}, ["x"])
        add("live registry resolves uniquely (every name -> exactly one module)",
            names is not None and len(names) > 0
            and len(reg_mods) == len(names) and not notes)
        add("floors: 0 < DERIVED_FN_FLOOR <= live derived fns (%d)" % len(live_fn),
            0 < DERIVED_FN_FLOOR <= len(live_fn))
        add("floors: FILES_FLOOR <= live src files (%d)" % live_files,
            FILES_FLOOR <= live_files)
        add("floors: SRC_LINES_FLOOR <= live src lines (%d)" % live_lines,
            SRC_LINES_FLOOR <= live_lines)
        add("floors: REG_RESOLVED_FLOOR <= resolved registry names (%d)" % len(reg_mods),
            REG_RESOLVED_FLOOR <= len(reg_mods))
        # D7 is a whole-block regex scan, the one axis a missing re.MULTILINE can
        # silence without touching any fixture outcome -- so pin it against reality,
        # not only against the planted table.
        add("live tree yields at least one D7 policy table (axis not blind on reality)",
            len(live_tbl) >= 1)
        # Mechanism fields judged against REALITY, not only against fixtures. These are
        # structural: they never assert which mechanism a given module has (that would
        # freeze today's measurement into a spec, the exact defect class ADR-218 D-44
        # records), only that whatever the derivation says is self-consistent and
        # traceable to a function that exists.
        live_req = required_keys(live_fn, live_tbl, reg_mods, live_all)
        live_mods = dict((k, v) for k, v in live_req.items() if k[0] == "MOD")
        names_by_mod = {}
        for _mod, _nm, _h, _m in live_all:
            names_by_mod.setdefault(_mod, set()).add(_nm)
        add("live MOD rows exist and every one carries a mechanism in the closed set",
            len(live_mods) >= 1
            and all(v["mech"] in MECH_VOCAB for v in live_mods.values()))
        add("cross-field: BELOW-DETECTOR names a real gap and counts no veto",
            all(v["near"] != "-" and v["supp"] == "0"
                for v in live_mods.values() if v["mech"] == "BELOW-DETECTOR"))
        add("cross-field: a BELOW-DETECTOR gap names a function of THAT module",
            all(v["near"].split(":")[0] in names_by_mod.get(v_key[1], set())
                for v_key, v in live_mods.items()
                if v["mech"] == "BELOW-DETECTOR"))
        add("cross-field: LOOKUP-SUPPRESSED counts at least one vetoed function",
            all(int(v["supp"]) >= 1
                for v in live_mods.values() if v["mech"] == "LOOKUP-SUPPRESSED"))
        add("cross-field: a NO-BODY module really has no production body",
            all(not names_by_mod.get(k[1])
                for k, v in live_mods.items() if v["mech"] == "NO-BODY"))
        _dist = {}
        for _v in live_mods.values():
            _dist[_v["mech"]] = _dist.get(_v["mech"], 0) + 1
        print("[SELFTEST] live MOD-row mech distribution: %s"
              % "/".join("%s=%d" % (k, _dist[k]) for k in sorted(_dist)))
        # Multiplicity pin: two fns sharing a name in one module collapse to a single
        # ledger key, so the only thing standing between that collapse and a silent
        # pass is dup_fns living in the CLAIM header.
        first = fn_keys[0]
        dup_claim = claim_of(fn_keys + [first], table_keys, {}, rows_ok)
        add("a duplicate (module, fn) name is pinned as dup_fns=1",
            "dup_fns=1" in dup_claim)
        add("a fixture without name collisions pins dup_fns=0 (the field is not fixed)",
            "dup_fns=0" in claim_of(fn_keys, table_keys, {}, rows_ok))
        # Non-blindness on the REAL tree. This must not depend on the ledger
        # existing, so it judges the live parse against an EMPTY ledger: every
        # derived key then becomes NEW. A gate whose extractors silently returned
        # nothing would report green here, which is the false-green this assertion
        # exists to make impossible -- and why the count is checked against the
        # same floor the runtime guard uses.
        empty_claim = claim_of(live_fn, live_tbl, reg_mods, [])
        empty_rc, empty_out, _es = judge(live_fn, live_tbl, reg_mods, [], empty_claim,
                                         live_all)
        new_rows = sum(1 for l in empty_out if "[NEW]" in l)
        add("live tree vs empty ledger is red with NEW rows (extractors are not blind)",
            empty_rc == 1 and new_rows > 0)
        add("live tree yields at least DERIVED_FN_FLOOR NEW rows (%d >= %d)"
            % (new_rows, DERIVED_FN_FLOOR), new_rows >= DERIVED_FN_FLOOR)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print("[SELFTEST] %s (%d assertions)"
          % ("ALL ASSERTIONS HELD" if ok else "FAILED", n))
    return 0 if ok else 1


def main(argv):
    args = list(argv[1:])
    allowed = ("--selftest", "--emit", "--write")
    if any(a not in allowed for a in args):
        sys.stderr.write("usage: check_l0_logic_ledger.py [--selftest|--emit|--write]\n")
        return 2
    if "--selftest" in args:
        return selftest()
    return run_real()


if __name__ == "__main__":
    import gate_rc  # crash -> exit 2, never a borrowed traceback exit 1

    sys.exit(gate_rc.run(lambda: main(sys.argv)))
