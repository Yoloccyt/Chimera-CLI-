#!/usr/bin/env python3
# =============================================================================
# crate_contract.py - per-crate contract header: generator + checker (B0-5)
# =============================================================================
# Purpose: give every crate ONE machine-checked declaration of what it is, so
#          the two claims that this repository's own audits kept getting wrong
#          stop being prose:
#            * "已实现 / 零 Stub"  -> MATURITY, derived from the crate's own
#              self-declaration labels (shared with check_impl_maturity_claims)
#            * "已装配"             -> CONSUMERS, derived from the NON-optional
#              production edges in the workspace manifests (the same face that
#              scripts/check_crate_reachability.py walks)
#          LAYER comes from layer_of() in scripts/check_dependency_rules.sh --
#          the single executable authority -- so a header that disagrees with the
#          gate is a real finding, not a second opinion. (Known live divergence
#          this surfaces: csn-substitutor is L7 in layer_of() but L10 in docs.)
#
# Block layout, written as Rust INNER DOC COMMENTS so it cannot change codegen:
#     //! CRATE-CONTRACT BEGIN            <- structural marker (idempotency key)
#     //! LAYER:     L7
#     //! ROLE:      <one line, harvested from the crate's own leading //! text>
#     //! BACKEND:   sqlite | fs | memory | none
#     //! PRODUCERS  <NexusEvent variants constructed in src/>
#     //! CONSUMERS  <count> <first five, alphabetical>
#     //! MATURITY   TRUE | MOCK-ONLY | PSEUDO | DEFERRED | ...
#     //! CRATE-CONTRACT END
#
# ROLE is harvested, never invented: 41/41 crates already open lib.rs with a
# `//!` self-description (measured 2026-09-23), so the header quotes the crate's
# own words instead of mine.
#
# Modes:
#   --emit            print the derived block for every crate (no writes)
#   --install         write/refresh the block in every crates/*/src/lib.rs
#   --check           the gate: verify presence + that derived fields match disk
#   --selftest        fixture tree + planted bad headers must all be caught
# Exit dialect (gate_rc): 0 clean / 1 judged red / 2 undecidable.
# =============================================================================
import argparse
import os
import re
import shutil
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import check_declared_dep_usage as ddu        # manifest parsing, one dialect
import check_impl_maturity_claims as maturity  # maturity labels, one vocabulary
import check_l0_purity as l0pure               # blank_rust + test_tail_mask (G-99 lexer family)

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LAYER_SCRIPT = os.path.join(ROOT, "scripts", "check_dependency_rules.sh")
BEGIN = "CRATE-CONTRACT BEGIN"
END = "CRATE-CONTRACT END"
KEYS = ("LAYER", "ROLE", "BACKEND", "PRODUCERS", "CONSUMERS", "MATURITY")
MAX_LIST = 5


def set_root(path):
    """Re-point at another repo root (selftest fixtures)."""
    global ROOT, LAYER_SCRIPT, ddu
    ROOT = path
    LAYER_SCRIPT = os.path.join(path, "scripts", "check_dependency_rules.sh")
    ddu.ROOT = path
    ddu.CRATES = os.path.join(path, "crates")
    maturity.set_root(path)


# ----------------------------------------------------------------- derivation
def layer_map(script=None):
    """Parse `layer_of()` out of the dependency gate into {crate: int}.

    Only the case arms are read; comments inside the case block are skipped, and
    an arm listing several crates maps each of them. Returns {} if the script or
    the function is missing -- callers must treat that as undecidable, never as
    "no crates are layered".
    """
    script = script or LAYER_SCRIPT
    if not os.path.exists(script):
        return {}
    with open(script, encoding="utf-8", errors="replace") as fh:
        text = fh.read()
    m = re.search(r"layer_of\(\)\s*\{.*?case[^{]*?in(.*?)\besac\b", text, re.S)
    if not m:
        return {}
    out = {}
    for line in m.group(1).splitlines():
        line = line.split("#", 1)[0].strip()
        arm = re.match(r'^([A-Za-z0-9_| -]+)\)\s*echo\s+(\d+)\s*;;', line)
        if not arm:
            continue
        for crate in arm.group(1).split("|"):
            crate = crate.strip()
            if crate and crate != "*":
                out[crate] = int(arm.group(2))
    return out


BOM = b"\xef\xbb\xbf"


def first_role_lines(lib_path):
    """Return the crate's own leading `//!` lines (blank-stripped).

    Read as `utf-8-sig`: one of the 41 crates' lib.rs carries a UTF-8 BOM
    (nexus-subagent, measured by byte comparison 2026-09-23), and with plain
    `utf-8` the BOM stays glued to line 1, so `//!` no longer starts the line and
    the harvest silently returned nothing -> ROLE "(untitled)".

    The installed CRATE-CONTRACT block is skipped **as a region**: the block sits
    at line 1 (inner doc comments must precede every item, see `insert_pos`), so
    without this its own `//! LAYER: L0` line was harvested as the ROLE -- 41/41
    self-referential values, invisible to `--check` because the checker calls the
    same function.
    """
    out = []
    if not os.path.exists(lib_path):
        return out
    in_block = False
    with open(lib_path, encoding="utf-8-sig", errors="replace") as fh:
        for line in fh:
            s = line.rstrip("\n")
            if not in_block and BEGIN in s:
                in_block = True
                continue
            if in_block:
                if END in s:
                    in_block = False
                continue
            if s.startswith("//!"):
                out.append(s[3:].strip())
            elif s.strip():
                break
    return out


def harvest_role(lib_path):
    """Pick the crate's own one-line role from its leading `//!` block.

    Skips lines that are only a layer tag, a heading, or a marker of this very
    block -- those describe placement or tooling, not the job.
    """
    for raw in first_role_lines(lib_path):
        t = raw.strip()
        if not t or BEGIN in t or END in t:
            continue
        if re.match(r"^(对应架构层|架构层归属|该 crate|#|-\s*|>)", t):
            continue
        if "CRATE-CONTRACT" in t:
            continue
        return t
    return ""


def derive_backend(crate_dir, index):
    """Classify the persistence face actually used in src/.

    sqlite     -> depends on rusqlite (bundled SQLite is the only such dep here)
    fs         -> writes/reads files via std::fs even without a DB
    memory     -> holds state in DashMap/HashMap only
    none       -> neither (pure logic / pure types)
    Precedence is sqlite > fs > memory, because a crate that does have a DB is
    not honestly described by whatever in-memory cache it also carries.
    """
    src = os.path.join(ROOT, "crates", crate_dir, "src")
    if not os.path.isdir(src):
        return "none"
    deps = ddu.parse_declared(os.path.join(ROOT, "crates", crate_dir, "Cargo.toml"))
    blob = ddu.code_blob(src)
    if "rusqlite" in deps:
        return "sqlite"
    if re.search(r"\bstd::fs\b|fs::(write|read|create|remove_file|rename)|OpenOptions::new", blob):
        return "fs"
    if re.search(r"\bDashMap\b|\bHashMap<|\bVec<[A-Z]", blob):
        return "memory"
    return "none"


def derive_producers(crate_dir):
    """List NexusEvent variants CONSTRUCTED (not matched) in this crate's src.

    Production semantics only (D3/M03: the header field promises "constructed
    in src/", so pattern positions, prose and test fixtures must not count):
      * comments and string literals are blanked first (check_l0_purity's
        blank_rust -- the same lexer the G-99 family judges by), so doc
        examples like `/// matches!(e, NexusEvent::QuestCreated {..})` cannot
        register as constructions;
      * `#[cfg(test)] mod ...` tails are masked (check_l0_purity's
        test_tail_mask) -- a fixture constructing an event proves nothing
        about production;
      * a line containing `=>` is dropped wholesale (single-line match arms);
      * multi-line `matches!(...)` spans are masked by paren tracking, which
        covers both the single-line form and the wrapped form where the
        pattern sits alone on a later line (chimera-tui panels shape);
      * `let`-pattern scrutinees are dropped positionally: the occurrence must
        not sit between a `(if|while)? let` keyword and the `=` that separates
        pattern from expression. This keeps `let ev = NexusEvent::X {..}`
        (construction, after the `=`) and `if let Err(e) = bus.publish(
        NexusEvent::X {..})` (construction inside the expression) while
        dropping `if let NexusEvent::X {..} = ev` and plain `let
        NexusEvent::X {..} = ev` destructure patterns;
      * a variant line is a multi-line match arm when an arm arrow `=>`
        appears at group depth 0 before any top-level `;` (statement-aware
        scan from the occurrence's own opening brace/paren). Depth awareness
        is what keeps both directions honest: nested arrows/semicolons
        (closures, inner matches inside the literal) cannot decide it, and a
        finished construction statement (`};`) ends the scan before a LATER
        match's arrows can mis-attach -- the parliament debate/mod.rs
        `certify_if_reviewed` shape (construction at :228, unrelated
        `Ok(()) => true` at :235) must stay counted. Or-pattern arms are the
        same scan with each alternative's braces opening and closing at
        depth 0 until the shared arrow (event-bus membrane.rs carries one
        fourteen-alternative arm; ARM_SCAN_LINES sits above it with headroom).
    What stays counted: a constructor passed as a value (rare; the honest
    residual of a line-based pass). What no longer happens: prose, tests and
    pattern positions inflating PRODUCERS.
    """
    src = os.path.join(ROOT, "crates", crate_dir, "src")
    if not os.path.isdir(src):
        return []
    out = set()
    for dirpath, _d, files in os.walk(src):
        for f in sorted(files):
            if not f.endswith(".rs"):
                continue
            path = os.path.join(dirpath, f)
            if _declared_cfg_test_module(path):
                continue
            with open(path, encoding="utf-8", errors="replace") as fh:
                code = l0pure.blank_rust(fh.read())
            lines = code.splitlines()
            tail = l0pure.test_tail_mask(lines)
            mspan = _matches_span_mask(lines)
            for i, line in enumerate(lines):
                if "NexusEvent::" not in line or tail[i] or mspan[i] or "=>" in line:
                    continue
                for m in EVENT_OCC.finditer(line):
                    if _in_let_pattern(line, m.start()):
                        continue
                    if _arm_arrow_after(lines, i, m.end() - 1, ARM_SCAN_LINES):
                        continue
                    out.add(m.group(1))
    return sorted(out)


# An occurrence is a construction only when it introduces the value: `X {` or
# `X(`. Pattern positions reuse the same two shapes, so the regex alone cannot
# tell them apart -- position context does.
EVENT_OCC = re.compile(r"NexusEvent::([A-Z]\w*)\s*[({]")
# `let` / `if let` / `while let` introduce a PATTERN before the `=` separator.
LET_ANCHOR = re.compile(r"\b(?:if\s+|while\s+)?let\b")
# The pattern/expression separator: a single `=` that is not part of ==, =>,
# <=, >=, !=. Those operators never separate a let-pattern from its scrutinee.
EQ_SEP = re.compile(r"(?<![!=<>])=(?!=|>)")
# Arm-scan line budget: the in-tree record is one fourteen-alternative arm
# (event-bus membrane.rs, arrow at +14); 32 leaves headroom. An arm longer
# than the budget would count as produced (over-report), so grow it if a
# wider arm ever lands -- the selftest fixture pins today's shape.
ARM_SCAN_LINES = 32


def _matches_span_mask(lines):
    """True for every line inside a `matches!(...)` macro span.

    Everything inside the macro's parentheses is scrutinee-plus-patterns, so
    no construction can hide there. Paren depth is tracked across lines on
    code already blanked by blank_rust, where parentheses are real syntax.
    """
    mask = [False] * len(lines)
    depth = 0
    for i, line in enumerate(lines):
        inside_at_start = depth > 0
        opened_here = False
        j = 0
        while j < len(line):
            if depth == 0 and line.startswith("matches!(", j):
                depth = 1
                opened_here = True
                j += len("matches!(")
                continue
            if depth > 0:
                if line[j] == "(":
                    depth += 1
                elif line[j] == ")":
                    depth -= 1
                j += 1
                continue
            j += 1
        mask[i] = inside_at_start or opened_here or depth > 0
    return mask


def _in_let_pattern(line, pos):
    """True when the occurrence at `pos` sits in a let-PATTERN (pre-`=`)."""
    m = LET_ANCHOR.search(line)
    if m is None or m.start() >= pos:
        return False
    sep = EQ_SEP.search(line, m.end())
    return sep is None or sep.start() > pos


def _arm_arrow_after(lines, i, col, max_lines):
    """True when the occurrence opening at lines[i][col] belongs to a match
    arm: an `=>` shows up at group depth 0 before a top-level `;`, or on the
    far side of the wrapping group the occurrence sits in.

    `col` points at the occurrence's own `{`/`(` (EVENT_OCC guarantees one),
    so the scan starts inside the literal it must judge. Only depth-0 arrows
    and semicolons decide -- arrows inside the literal's nested closures or
    inner matches are inert, which is exactly the discrimination a fixed line
    window cannot make. Going NEGATIVE means an enclosing group the scan never
    saw open has closed (the `Ok(NexusEvent::X {..}) => {` subscriber shape:
    the `(` opens before the occurrence); whether it is an arm is then decided
    by what follows on that line -- `}) => {` is an arm, `});` is a finished
    construction statement, a bare `}` (fn-tail construction) is not. A
    line-leading `=>` on the next non-blank line counts too (pathological
    formatting only; rustfmt never emits it, but a missed arrow is the
    over-report direction this gate exists to close).
    """
    depth = 0
    for j in range(i, min(len(lines), i + max_lines)):
        line = lines[j]
        k = col if j == i else 0
        while k < len(line):
            ch = line[k]
            if ch in "{(":
                depth += 1
            elif ch in "})":
                depth -= 1
                if depth < 0:
                    rest = line[k + 1:]
                    if "=>" in rest:
                        return True
                    if rest.strip():
                        return False
                    # blank remainder: only a line-leading arrow (next
                    # non-blank line) can still make this an arm
                    for ln in lines[j + 1:j + 3]:
                        if not ln.strip():
                            continue
                        return ln.strip().startswith("=>")
                    return False
            elif depth == 0:
                if ch == ";":
                    return False
                if ch == "=" and line.startswith("=>", k):
                    return True
            k += 1
    return False


def _declared_cfg_test_module(path):
    """True when the WHOLE file is a test module: its parent declares
    `#[cfg(test)] mod <stem>;`, so the gating attribute lives in another file
    and test_tail_mask (same-file) cannot see it.

    Measured shapes: chimera-tui src/app/tests.rs and parliament
    src/debate/tests.rs (both gated in their mod.rs). The stem guard keeps the
    parent-file scan off ordinary module files; the declaration must still
    carry the attribute, so pub test-utility modules (nexus-contracts
    testutil/test_scale.rs, compiled unconditionally on purpose) stay in the
    production face.
    """
    stem = os.path.splitext(os.path.basename(path))[0]
    if stem != "tests" and not stem.startswith("test_"):
        return False
    d = os.path.dirname(path)
    # src/app/tests.rs -> src/app/mod.rs | src/app.rs ; src/tests.rs (a child
    # of the crate root) -> src/lib.rs. All three spellings are legal Rust 2018
    # parents; probing costs one stat each and a miss is just "not a test file".
    parents = [os.path.join(d, "mod.rs"),
               os.path.join(d, "lib.rs"),
               os.path.join(os.path.dirname(d), os.path.basename(d) + ".rs")]
    decl = re.compile(r"#\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*(?:#[^\n]*\n\s*)*"
                      r"(?:pub\s*\([^)]*\)\s+)?(?:pub\s+)?mod\s+"
                      + re.escape(stem) + r"\s*;")
    for parent in parents:
        if not os.path.exists(parent):
            continue
        try:
            with open(parent, encoding="utf-8", errors="replace") as fh:
                if decl.search(fh.read()):
                    return True
        except OSError:
            continue
    return False


def _table_kind(line):
    """Classify a TOML table header as an assembly-face ('normal') dependency
    table, a non-assembly table ('dev'/'build'), or a table the CONSUMERS face
    deliberately excludes (None: workspace/registry/target-scoped).
    """
    m = re.match(r"^\s*\[([^\]]+)\]\s*$", line)
    if not m:
        return None
    name = m.group(1).strip().strip('"')
    if name == "dependencies":
        return "normal"
    if name in ("dev-dependencies", "build-dependencies"):
        return "dev"
    # workspace.dependencies is the version pinning table, not an edge;
    # target.*.dependencies is platform-scoped (see check_contract_consumers_parity
    # -- it reports those separately instead of silently dropping them).
    return None


def _dep_rows(body):
    """Yield (table_kind, dep_name, value_text) rows, joining multi-line values.

    WHY this exists instead of one-line regexes: a row can spread over several
    lines (this repo does it for feature arrays), so `optional = true` is not
    always on the dep's own line, and the same package can be declared twice
    (production + dev). Both cases made a line-wise parser disagree with cargo.
    """
    kind = None
    lines = body.splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.strip()
        if stripped.startswith("["):
            kind = _table_kind(stripped)
            i += 1
            continue
        m = re.match(r'^"?([A-Za-z0-9_.+-]+)"?\s*=\s*(.*)$', stripped)
        if not m or kind is None:
            i += 1
            continue
        name, value = m.group(1), m.group(2)
        # accumulate until braces/brackets balance
        depth = value.count("{") + value.count("[") - value.count("}") - value.count("]")
        j = i + 1
        while depth > 0 and j < len(lines):
            cont = lines[j]
            value += " " + cont.strip()
            depth += cont.count("{") + cont.count("[") - cont.count("}") - cont.count("]")
            j += 1
        yield kind, name, value
        i = j


def declared_production(manifest):
    """Return {dep_name: True} for deps with >=1 non-optional [dependencies] row.

    Cargo's own rule: an edge is in the assembly graph if ANY normal-kind row
    declares it non-optionally -- a second `[dev-dependencies]` row for the same
    package does not remove it. Keyed by manifest name; `package = "..."` renames
    are mapped too so both spellings resolve.
    """
    try:
        with open(manifest, encoding="utf-8-sig", errors="replace") as fh:
            body = fh.read()
    except OSError:
        return {}
    out = {}
    for kind, name, value in _dep_rows(body):
        if kind != "normal":
            continue
        if re.search(r"\boptional\s*=\s*true", value):
            continue
        out[name] = True
        m = re.search(r'\bpackage\s*=\s*"([^"]+)"', value)
        if m:
            out[m.group(1)] = True
    return out


def derive_consumers(crate_dir, index):
    """Crates listing `crate_dir` in a NON-optional [dependencies] section."""
    pkg = index.get(crate_dir)
    if pkg is None:
        return []
    out = []
    for other, opkg in sorted(index.items()):
        if other == crate_dir:
            continue
        manifest = os.path.join(ROOT, "crates", other, "Cargo.toml")
        if not os.path.exists(manifest):
            continue
        # optional edges are the GATED face, not the assembly face; a dual
        # production+dev declaration still counts (see declared_production)
        if declared_production(manifest).get(pkg):
            out.append(other)
    return out


def derive_maturity(crate_dir):
    """Maturity labels this crate self-declares, else TRUE."""
    hits = maturity.scan_crate(crate_dir)
    labels = sorted({label for (_c, label) in hits})
    return "|".join(labels) if labels else "TRUE"


def derive(crate_dir, layers, index):
    """Build every derived field for one crate."""
    lib = os.path.join(ROOT, "crates", crate_dir, "src", "lib.rs")
    layer = layers.get(crate_dir)
    cons = derive_consumers(crate_dir, index)
    prod = derive_producers(crate_dir)
    return {
        "LAYER": "L%d" % layer if layer is not None else "UNKNOWN",
        "ROLE": harvest_role(lib) or "(untitled)",
        "BACKEND": derive_backend(crate_dir, index),
        "PRODUCERS": "%d %s" % (len(prod), ",".join(prod[:MAX_LIST]) +
                                ("…" if len(prod) > MAX_LIST else "")) if prod else "0 -",
        "CONSUMERS": "%d %s" % (len(cons), ",".join(cons[:MAX_LIST]) +
                                 ("…" if len(cons) > MAX_LIST else "")) if cons else "0 -",
        "MATURITY": derive_maturity(crate_dir),
    }


def render(fields):
    """Render the header block, keys aligned, as Rust inner doc comments."""
    w = max(len(k) for k in KEYS)
    lines = ["//! %s" % BEGIN]
    for k in KEYS:
        lines.append("//! %-*s %s" % (w, k + ":", fields[k]))
    lines.append("//! %s" % END)
    return lines


# -------------------------------------------------------------------- install
def insert_pos(lines):
    """Always 0: the block goes at the very top of lib.rs.

    WHY not "find the first item and insert before it" (the first revision did
    exactly that, and it broke the build): the skip set matched any line starting
    with `//`, which also swallows OUTER doc comments (`/// ...`), so on
    nexus-contracts the scan walked past a run of `///` + `pub mod` pairs and put
    eight INNER doc comments (`//!`) below the first item -> `error[E0753]`,
    8 per crate, invisible to a text diff and to `rustfmt` in this environment.
    Inner doc comments are inner attributes; at index 0 they precede every item
    by construction, so placement cannot be wrong. A BOM (if present) is re-added
    outside this position, so it stays the first three bytes of the file.
    """
    return 0


def install(crate_dir, fields):
    """Write/refresh the block in one lib.rs. Returns 'installed'/'refreshed'.

    Bytes are read and written verbatim (no newline translation, no BOM), which
    is the only safe mode in a tree whose files mix CRLF and LF.
    """
    lib = os.path.join(ROOT, "crates", crate_dir, "src", "lib.rs")
    if not os.path.exists(lib):
        raise ValueError("no lib.rs for %s" % crate_dir)
    with open(lib, "rb") as fh:
        raw = fh.read()
    # A BOM must stay the first three bytes of the file: it is stripped before
    # the text is manipulated and re-prepared on write, otherwise inserting a
    # block "before line 1" would leave the BOM stranded mid-file (illegal char).
    bom = raw.startswith(BOM)
    text = (raw[len(BOM):] if bom else raw).decode("utf-8")
    eol = "\r\n" if "\r\n" in text else "\n"
    lines = text.split(eol)
    b = [i for i, l in enumerate(lines) if BEGIN in l]
    e = [i for i, l in enumerate(lines) if END in l]
    if len(b) > 1 or len(e) > 1 or len(b) != len(e):
        # A half-present or duplicated block means a human edited it; silently
        # overwriting that would destroy their work, so abort loudly instead.
        raise ValueError("%s: unbalanced contract markers (%d begin/%d end)"
                         % (crate_dir, len(b), len(e)))
    state = "refreshed" if b else "installed"
    if b:
        keep = [l for i, l in enumerate(lines) if not any(x <= i <= y for x, y in zip(b, e))]
        lines = keep
    at = insert_pos(lines)
    lines = lines[:at] + render(fields) + lines[at:]
    blob = eol.join(lines).encode("utf-8")
    with open(lib, "wb") as fh:
        fh.write((BOM if bom else b"") + blob)
    return state


# --------------------------------------------------------------------- check
def parse_block(crate_dir):
    """Return (fields|None, problem|None) for one crate's installed block."""
    lib = os.path.join(ROOT, "crates", crate_dir, "src", "lib.rs")
    if not os.path.exists(lib):
        return None, "no lib.rs"
    with open(lib, encoding="utf-8-sig", errors="replace") as fh:
        lines = fh.read().splitlines()
    body = [l for l in lines if BEGIN in l or END in l or
            re.match(r"^//!\s+[A-Z]+\s*:", l)]
    if sum(1 for l in lines if BEGIN in l) == 0:
        return None, "missing contract block"
    fields = {}
    for l in body:
        m = re.match(r"^//!\s+([A-Z]+)\s*:\s*(.*)$", l)
        if m:
            fields[m.group(1)] = m.group(2).strip()
    missing = [k for k in KEYS if k not in fields]
    if missing:
        return None, "missing key(s): %s" % ",".join(missing)
    return fields, None


def check():
    """Verify every crate's block against freshly derived facts."""
    index = ddu.crates_index()
    layers = layer_map()
    if not layers:
        print("[CONFIG] layer_of() not found in %s -- cannot judge LAYER" % LAYER_SCRIPT)
        return 2
    problems = []
    for crate_dir in sorted(index):
        want = derive(crate_dir, layers, index)
        got, why = parse_block(crate_dir)
        if got is None:
            problems.append((crate_dir, why))
            continue
        for k in KEYS:
            if k == "ROLE":
                if not got[k] or got[k] == "(untitled)":
                    problems.append((crate_dir, "ROLE is empty"))
                elif re.match(r"^[A-Z]{3,}\s*:", got[k]):
                    # A ROLE that reads like `LAYER: L0` means the harvest walked
                    # into this very block. `--check` must be able to say so,
                    # because it cannot otherwise disagree with its own source.
                    problems.append(
                        (crate_dir,
                         "ROLE is a contract field, not the crate's own role: %r"
                         % got[k][:40]))
                continue
            if got[k] != want[k]:
                problems.append((crate_dir, "%s says %r, disk says %r" % (k, got[k], want[k])))
    print("[INFO] crates checked: %d; contract blocks valid: %d"
          % (len(index), len(index) - len(problems)))
    if not problems:
        print("[OK] every crate declares a contract and its derived fields match the disk")
        return 0
    print("[FAIL] %d contract problem(s):" % len(problems))
    for crate_dir, why in problems:
        print("  [GAP] %-20s %s" % (crate_dir, why))
    print("  Fix: python scripts/crate_contract.py --install   (regenerates derived rows)")
    return 1


# ------------------------------------------------------------------- selftest
def _fixture():
    """Synthetic workspace: one clean crate + four planted defects."""
    tmp = tempfile.mkdtemp(prefix="contract-")
    for d in ("scripts", "crates/alpha/src", "crates/beta/src", "crates/gamma/src"):
        os.makedirs(os.path.join(tmp, d), exist_ok=True)
    with open(os.path.join(tmp, "scripts", "check_dependency_rules.sh"), "w", encoding="utf-8") as fh:
        fh.write('layer_of() {\n    case "$1" in\n'
                 '        alpha|beta) echo 2 ;;\n'
                 '        # comment arm\n        gamma) echo 3 ;;\n'
                 '        *) echo "" ;;\n    esac\n}\n')
    for name in ("alpha", "beta", "gamma"):
        with open(os.path.join(tmp, "crates", name, "Cargo.toml"), "w", encoding="utf-8") as fh:
            fh.write('[package]\nname = "%s"\n\n[dependencies]\n' % name)
        with open(os.path.join(tmp, "crates", name, "src", "lib.rs"), "w", encoding="utf-8") as fh:
            fh.write("//! %s -- the crate's own role sentence\n\n#![forbid(unsafe_code)]\n"
                     "/// outer doc: the shape that fooled the old placement rule\n"
                     "pub fn f() {}\n" % name)
    # beta: gamma depends on it non-optionally -> consumer, so beta's header must list it
    with open(os.path.join(tmp, "crates", "gamma", "Cargo.toml"), "a", encoding="utf-8") as fh:
        fh.write('beta = { path = "../beta" }\n')
    # gamma additionally reproduces the two real-tree byte shapes that broke an
    # earlier revision: a UTF-8 BOM (stranded mid-file if insertion ignores it)
    # and CRLF line endings (must not be normalised to LF).
    gp = os.path.join(tmp, "crates", "gamma", "src", "lib.rs")
    with open(gp, "rb") as fh:
        body = fh.read()
    with open(gp, "wb") as fh:
        fh.write(BOM + body.replace(b"\n", b"\r\n"))
    # alpha carries the producer-derivation fixtures (C-03/M03): every shape
    # here is one the old line loop mis-counted on the real tree. Only
    # RealConstructed and ExprConstruct may survive derive_producers.
    with open(os.path.join(tmp, "crates", "alpha", "src", "producers_fixture.rs"),
              "w", encoding="utf-8") as fh:
        fh.write(PRODUCER_FIXTURE)
    # file-level test module: gated by `#[cfg(test)] mod tests;` in lib.rs, so
    # neither test_tail_mask (same-file attribute) nor the arm rules can see
    # the gate -- _declared_cfg_test_module must (chimera-tui app/tests.rs
    # shape; CacheMiss was over-reported through exactly this hole).
    with open(os.path.join(tmp, "crates", "alpha", "src", "tests.rs"),
              "w", encoding="utf-8") as fh:
        fh.write("pub fn fixture() -> NexusEvent {\n"
                 "    NexusEvent::FileTailConstructed { id: 4 }\n}\n")
    with open(os.path.join(tmp, "crates", "alpha", "src", "lib.rs"),
              "a", encoding="utf-8") as fh:
        fh.write("\n#[cfg(test)]\nmod tests;\n")
    return tmp


# Every line mirrors a measured real-tree shape (2026-10-25):
#   - comment/string prose mentioning variants (event-bus bus.rs doc lines)
#   - single-line and wrapped matches! (efficiency-monitor monitor.rs:51,
#     chimera-tui panels event_stream.rs/log.rs)
#   - if-let scrutinee (mcp-mesh mesh.rs:783) and plain let-destructure
#     (efficiency-monitor recall_collector.rs:125)
#   - construction AFTER the `=` of an if-let (event-bus token_ledger.rs:139
#     shape: `if let Err(e) = bus.publish(NexusEvent::X {..})`) -- must COUNT
#   - multi-line match arm with the arrow at +3 (parliament strategy_cap.rs)
#     and at +5 (osa six_dimension.rs / csn-substitutor lib.rs -- why
#     ARM_WINDOW is 8, not the plan-drafted 3)
#   - or-pattern chain (chimera-tui event_stream.rs severity bands) and a
#     14-alternative chain with the arrow at +14 (event-bus membrane.rs:344
#     category arm -- why the chain walk exists, no fixed window covers it)
#   - cfg(test) tail construction (omega-learner s9_integration.rs:271) and a
#     whole-file test module gated from the parent (chimera-tui app/tests.rs)
#   - a construction whose NEXT statement is a match with arm arrows 3 lines
#     below (parliament debate/mod.rs certify_if_reviewed shape) -- must
#     COUNT; only a depth-aware arm scan can tell it from a real arm
#   - the Ok(...)-wrapped pair: `Ok(NexusEvent::X {..}) => {}` is an arm
#     (strategy_cap.rs:382 / handshake.rs:88 subscriber shape), while
#     `Ok(NexusEvent::Y {..});` is a real construction -- same wrapping,
#     opposite verdicts, decided by what follows the closing paren
PRODUCER_FIXTURE = """
fn prod(ev: &NexusEvent, bus: &mut Bus) {
    let _a = NexusEvent::RealConstructed { id: 1 };
    // prose mention: NexusEvent::CommentMentioned { .. } must not count
    let _s = "NexusEvent::StringMentioned { .. }";
    let _m1 = matches!(ev, NexusEvent::MatchesSingle { .. });
    let _m2 = matches!(
        ev,
        NexusEvent::MatchesWrapped { .. }
    );
    let (_m1, _m2) = (_m1, _m2);
    if let NexusEvent::IfLetScrutinee { .. } = ev {}
    let NexusEvent::LetDestructured { .. } = ev;
    match ev {
        NexusEvent::MultilineArm {
            a,
            ..
        } => {}
        NexusEvent::FiveGapArm {
            f1,
            f2,
            f3,
            f4,
        } => {}
        NexusEvent::OrPatternA { .. }
        | NexusEvent::OrPatternB { .. } => 2,
        NexusEvent::ChainStart { .. }
        | NexusEvent::ChainMid1 { .. }
        | NexusEvent::ChainMid2 { .. }
        | NexusEvent::ChainMid3 { .. }
        | NexusEvent::ChainMid4 { .. }
        | NexusEvent::ChainMid5 { .. }
        | NexusEvent::ChainMid6 { .. }
        | NexusEvent::ChainMid7 { .. }
        | NexusEvent::ChainMid8 { .. }
        | NexusEvent::ChainMid9 { .. }
        | NexusEvent::ChainMid10 { .. }
        | NexusEvent::ChainMid11 { .. }
        | NexusEvent::ChainMid12 { .. }
        | NexusEvent::ChainEnd { .. } => 3,
        _ => {}
    }
    if let Err(_e) = bus.publish(NexusEvent::ExprConstruct { id: 2 }) {}
    let _wrapped: Result<NexusEvent, ()> = Ok(NexusEvent::OkWrappedReal {
        id: 5,
    });
    match bus.recv().await {
        Ok(NexusEvent::OkWrappedArm {
            id,
            ..
        }) => {}
        _ => {}
    }
    let nearby = NexusEvent::NearbyMatchCounted {
        id: 9,
    };
    match bus.send(nearby) {
        Ok(()) => true,
        Err(_e) => false,
    }
}

#[cfg(test)]
mod producer_tests {
    #[test]
    fn tail_only() {
        let _t = NexusEvent::TailConstructed { id: 3 };
    }
}
"""


def selftest():
    """Install into a fixture, then prove each planted defect is caught.

    The planted set is the point: a checker that never reports a problem on
    doctored input has no demonstrated ability to see one.
    """
    tmp = _fixture()
    orig_root, orig_script = ROOT, LAYER_SCRIPT
    try:
        set_root(tmp)
        index = ddu.crates_index()
        layers = layer_map()
        caught = []
        # 0. layer parsing must skip comment lines and expand `|` arms
        caught.append(("layer_of parsed incl. comment-arm skip",
                       layers == {"alpha": 2, "beta": 2, "gamma": 3}))
        for c in sorted(index):
            install(c, derive(c, layers, index))
        # Consumers must come from the manifest edge, and this assertion has to
        # run BEFORE the fixture is deliberately corrupted below -- parse_block()
        # refuses to return partial fields once a key is missing, so checking
        # afterwards measures nothing (that ordering mistake failed once already).
        got, _why = parse_block("beta")
        caught.append(("consumer derived, not guessed",
                       bool(got) and got["CONSUMERS"].startswith("1 gamma")))
        # ROLE must be the crate's OWN sentence. The block now sits at line 1, so
        # a harvest that does not skip the block returns its `LAYER:` row and
        # every crate reads the same garbage -- and `check()` cannot see it
        # because it calls the same function. Hence: assert the text, then prove
        # the checker rejects the field-shaped value.
        caught.append(("ROLE is the crate's own sentence, not a block field",
                       bool(got) and got["ROLE"] == "beta -- the crate's own role sentence"))
        # C-03/M03: the producer face must see constructions, not pattern
        # positions / prose / test tails. The fixture plants the shapes that
        # must vanish and the three that must survive; set equality pins both
        # sides.
        caught.append(("PRODUCERS counts only real constructions",
                       derive_producers("alpha") ==
                       sorted({"RealConstructed", "ExprConstruct",
                               "NearbyMatchCounted", "OkWrappedReal"})))
        caught.append(("clean fixture passes", check() == 0))
        _mutate(tmp, "beta", "ROLE", "LAYER:    L2")
        caught.append(("ROLE that echoes a block field is caught", check() == 1))
        install("beta", derive("beta", layer_map(), ddu.crates_index()))
        caught.append(("fixture clean again after ROLE restore", check() == 0))
        with open(os.path.join(tmp, "crates", "alpha", "src", "lib.rs"), "rb") as fh:
            first = fh.read().decode("utf-8-sig").splitlines()[0]
        caught.append(("block sits at file top (E0753 guard)", BEGIN in first))
        # Byte fidelity: gamma was written with a BOM + CRLF on purpose (see
        # _fixture). An insertion that ignores either would silently corrupt the
        # file, and neither shows up in a text-level diff.
        with open(os.path.join(tmp, "crates", "gamma", "src", "lib.rs"), "rb") as fh:
            gb = fh.read()
        caught.append(("BOM stays first, CRLF not normalised",
                       bool(gb.startswith(BOM))
                       and gb.count(b"\n") == gb.count(b"\r\n")))
        # 1. derived field now disagrees with the header (stale LAYER)
        _mutate(tmp, "alpha", "LAYER", "L9")
        caught.append(("stale LAYER caught", check() == 1))
        _mutate(tmp, "alpha", "LAYER", "L2")
        # 2. a key removed entirely
        _mutate(tmp, "beta", "BACKEND", None)
        caught.append(("missing key caught", check() == 1))
        # 3. a crate with no block at all
        _strip_block(tmp, "gamma")
        caught.append(("absent block caught", check() == 1))
        rc = 0
        for name, ok in caught:
            print("  [%s] %s" % ("PASS" if ok else "FAIL", name))
            if not ok:
                rc = 1
        # idempotency: installing twice must not duplicate the block
        if rc == 0:
            set_root(tmp)
            for c in sorted(ddu.crates_index()):
                install(c, derive(c, layer_map(), ddu.crates_index()))
            dup = sum(1 for c in ddu.crates_index()
                      if c != "gamma" and _count(tmp, c, BEGIN) != 1)
            print("  [%s] re-install is idempotent" % ("PASS" if not dup else "FAIL"))
            rc = 1 if dup else rc
        if rc == 0:
            print("[SELFTEST] install + 5 planted checker failures + idempotency verified")
        return rc
    finally:
        set_root(orig_root)
        globals()["LAYER_SCRIPT"] = orig_script
        shutil.rmtree(tmp, ignore_errors=True)


def _path(tmp, crate):
    return os.path.join(tmp, "crates", crate, "src", "lib.rs")


def _mutate(tmp, crate, key, value):
    """Set or delete one header row in the fixture's installed block."""
    p = _path(tmp, crate)
    with open(p, encoding="utf-8") as fh:
        lines = fh.read().splitlines()
    out = []
    for l in lines:
        m = re.match(r"^//!\s+(%s)\s*:" % key, l)
        if m:
            if value is None:
                continue
            out.append("//! %-9s %s" % (key + ":", value))
        else:
            out.append(l)
    with open(p, "w", encoding="utf-8") as fh:
        fh.write("\n".join(out) + "\n")


def _strip_block(tmp, crate):
    p = _path(tmp, crate)
    with open(p, encoding="utf-8") as fh:
        lines = fh.read().splitlines()
    keep, skip = [], False
    for l in lines:
        if BEGIN in l:
            skip = True
        if not skip:
            keep.append(l)
        if END in l:
            skip = False
    with open(p, "w", encoding="utf-8") as fh:
        fh.write("\n".join(keep) + "\n")


def _count(tmp, crate, marker):
    with open(_path(tmp, crate), encoding="utf-8") as fh:
        return sum(1 for l in fh if marker in l)


# ---------------------------------------------------------------------- main
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--emit", action="store_true")
    ap.add_argument("--install", action="store_true")
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    index = ddu.crates_index()
    layers = layer_map()
    if not layers:
        print("[CONFIG] layer_of() not found in %s" % LAYER_SCRIPT)
        return 2
    if args.emit:
        for c in sorted(index):
            print("### %s" % c)
            print("\n".join(render(derive(c, layers, index))))
        return 0
    if args.install:
        done = {"installed": 0, "refreshed": 0}
        for c in sorted(index):
            done[install(c, derive(c, layers, index))] += 1
        print("[OK] contract blocks: %d written fresh, %d refreshed (of %d crates)"
              % (done["installed"], done["refreshed"], len(index)))
        return 0
    return check()


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(main))
