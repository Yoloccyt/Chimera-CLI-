#!/usr/bin/env python3
# -*- coding: ascii -*-
"""Positional (MessagePack array) persistence field-order gate (G-111, ADR-213 R8.2/R9).

Why this gate exists
--------------------
ADR-213 decisions R8.2 and R9 froze the field order of every type persisted in rmp-serde
POSITIONAL (array) form and banned adding `default + skip` fields to such types -- and
neither rule had any machine judgement behind it. The ledger did not exist; no gate cited
them. That is the failure mode this repository keeps refusing: a rule whose violation is
invisible.

The violation is invisible in the strongest sense, and that is measured, not argued.
`crates/nexus-contracts/tests/wire_format_compat.rs` pins a three-row behaviour matrix.
Under array encoding:
  * a middle-INSERTED field -> loud `invalid length` error (survivable);
  * a TAIL-APPENDED field with `#[serde(default)]` -> still decodes (sanctioned growth);
  * TWO SAME-TYPED FIELDS SWAPPING DECLARATION ORDER -> `Ok`, values silently
    interchanged. No compile error, no test failure, no signal at the decode site.
R9 adds a second silent shape: if an earlier field is skippable, a later non-skipped value
occupies the skipped neighbour's slot -- loud when the neighbour types differ, silently
wrong when they match.

Named (map) encoding is immune to both, which is why this gate judges ONLY
`rmp_serde::to_vec(` sites and never `to_vec_named(`: freezing field order for map
encoding would manufacture false reds against a risk measurement says does not exist.

What it judges
--------------
Input surface: `crates/*/src/**/*.rs`. The risk is not L0-local -- the envelopes are
declared in L0 and written from L3/L9/L10. Comments and string literals are blanked and
`#[cfg(test)]` tails are masked by the SAME lexer G-99/G-105/G-107 use (imported, not
re-implemented), so prose and test fixtures never enter the surface.

  NEW                     array write point with neither a WRITE nor an EXEMPT row, and
                          not an orphan file -> red
  STALE                   a ledger row whose site no longer exists -> red
  ARG_DRIFT               the site now encodes a different expression than the row pins
                          -> red (the frozen order is claimed about the pinned value)
  ORDER_DRIFT             live field order is not the frozen order and the difference is
                          not a pure tail append -> red (the silent-swap case)
  UNREGISTERED_TAIL_APPEND  live = frozen + extra tail fields: allowed shape, but the row
                          must be re-emitted in the same commit -> red
  NO_DEFAULT_ON_APPEND    an appended field lacks `#[serde(default)]` -> red
  R9_SKIP_ON_ARRAY        a `default` + `skip_serializing_if` field appeared on a
                          positionally persisted type beyond the registered skip set -> red
  exit 2                  malformed row / unknown key / short evidence / duplicate site /
                          unresolved or ambiguous type / non-struct shape / scan surface
                          below floor / crash (gate_rc). Undecidable is never a pass.

Orphan linkage (not optional)
-----------------------------
Files listed in `src_module_reachability_freeze.txt` (G-109) are excluded from judgement
but counted on screen. The day such a file is wired up it leaves that ledger, so its write
point arrives here with no row -> NEW. Without the linkage, the wiring commit would open a
silent blind spot on exactly the file that had never been compiled.

Known limits (stated rather than hidden)
----------------------------------------
  * Shapes that are not named-field structs (tuple structs, enums, unions) have no field
    order to freeze -> exit 2, forcing a human decision. Enum VARIANT order is also
    positionally significant; judging it is out of scope here and is NOT claimed covered.
  * "This site's argument has this type" is asserted by the row and falsified
    mechanically only against the pinned `arg=` text. Rust type inference is out of scope;
    pinning the argument keeps the assertion reviewable and drift-visible.
  * `--emit` lists sites but cannot invent the type binding, so its rows carry `<REVIEW>`
    placeholders. They must never be pasted into the ledger unedited.

Modes / exit codes
------------------
  (default)  judge the real tree          0 clean / 1 violation / 2 undecidable
  --emit     candidate rows into tmp/     0 / 2
  --selftest plant every failure class above; prove the gate bites both ways
                                       0 held / 1 not held
  anything else                          2

Output is pure ASCII (project script convention; avoids Windows GBK console traps).
"""

import os
import re
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
LEDGER = os.path.join(HERE, "positional_field_order_freeze.txt")
ORPHAN_LEDGER = os.path.join(HERE, "src_module_reachability_freeze.txt")

# A parser that went blind reports "no violations" exactly like a clean tree, so the floor
# ships with the gate. Measured surface 2026-10-14: 799 files under crates/*/src.
SCANNED_FLOOR = 600

# Same discipline as G-99's FILE_EXEMPT: a waiver too short to review is not a waiver.
MIN_EVIDENCE_LEN = 24

sys.path.insert(0, HERE)
import check_l0_purity as purity  # noqa: E402  (shares lexer + cfg(test) tail mask)

ARRAY_SITE_RE = re.compile(r"\brmp_serde\s*::\s*to_vec\s*\(")
FN_DECL_RE = re.compile(r"(?<![\w.])fn\s+([A-Za-z_]\w*)\s*[<(]")
SITE_HINT = "to_vec"  # raw prefilter: every positional encode spells this substring
DECL_KIND_RE = re.compile(r"(?m)^[ \t]*pub(?:\([^)]*\))?[ \t]+"
                          r"(struct|enum|union)[ \t]+([A-Za-z_]\w*)")
# A field line is `name: Ty,`, optionally preceded by `pub` / `pub(crate)`.
FIELD_RE = re.compile(r"^(?:pub(?:\([^)]*\))?[ \t]+)?([A-Za-z_]\w*)[ \t]*:")
SERDE_ATTR_RE = re.compile(r"#\[serde\(")
DEFAULT_RE = re.compile(r"\bdefault\b")
SKIP_RE = re.compile(r"skip_serializing_if")
ROW_RE = re.compile(r"^(WRITE|EXEMPT)\s*\|(.*)$")
KV_RE = re.compile(r"^([a-z_]+)=(.*)$")
ROW_KEYS = {"WRITE": ("site", "arg", "type", "fields", "evidence"),
            "EXEMPT": ("site", "reason")}
ROW_OPTIONAL = {"WRITE": ("in", "skips"), "EXEMPT": ()}


def repo_rel(path):
    return os.path.relpath(path, ROOT).replace(os.sep, "/")


def iter_src_files(crates_root):
    """Every `crates/*/src/**/*.rs`, repo-relative, forward slashes."""
    out = []
    if not os.path.isdir(crates_root):
        return out
    for crate in sorted(os.listdir(crates_root)):
        src = os.path.join(crates_root, crate, "src")
        if not os.path.isdir(src):
            continue
        for dirpath, _dn, files in os.walk(src):
            for fn in sorted(files):
                if fn.endswith(".rs"):
                    out.append(repo_rel(os.path.join(dirpath, fn)))
    return out


def read_text(rel):
    try:
        with open(os.path.join(ROOT, rel.replace("/", os.sep)),
                  "r", encoding="utf-8", errors="replace") as fh:
            return fh.read()
    except (IOError, OSError):
        return None


def blanked_masked(rel):
    """(lines, tail_mask) of blanked production code, or (None, None) if unreadable."""
    raw = read_text(rel)
    if raw is None:
        return None, None
    lines = purity.blank_rust(raw).splitlines()
    return lines, purity.test_tail_mask(lines)


def enclosing_fn(lines, mask, site_idx):
    """Nearest unmasked `fn` at or above the site -- the stable part of the ledger key.

    Deliberately not a line number: W3's physical move of 53 modules shifted every L0
    line, and a line-anchored ledger would go wholesale red on a pure relocation.
    """
    for i in range(site_idx, -1, -1):
        if not mask[i]:
            m = FN_DECL_RE.search(lines[i])
            if m:
                return m.group(1)
    return "MOD_LEVEL"


def capture_arg(text, start):
    """First argument text after an open paren, balanced, stopping at a depth-1 comma."""
    depth = 1
    buf = []
    for ch in text[start:]:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
            if depth == 0:
                break
        elif ch == "," and depth == 1:
            break
        if depth >= 1:
            buf.append(ch)
    return re.sub(r"\s+", " ", "".join(buf)).strip()


def scan_sites(rels):
    """[{file, fn, site, arg}] over production positional write points."""
    sites = []
    for rel in rels:
        raw = read_text(rel)
        if raw is None or SITE_HINT not in raw:
            continue
        lines, mask = blanked_masked(rel)
        if lines is None:
            continue
        # Balance needs the tail of the line after the open paren, so re-join the file
        # body; the mask is consulted per start line, and a masked start is skipped.
        joined = "\n".join(lines)
        offset = 0
        for i, line in enumerate(lines):
            if not mask[i]:
                for m in ARRAY_SITE_RE.finditer(line):
                    arg = capture_arg(joined[offset + m.end():], 0)
                    sites.append({"file": rel, "fn": enclosing_fn(lines, mask, i),
                                  "arg": arg})
            offset += len(line) + 1
    return sites


def index_decls(rels, wanted):
    """name -> [rel,...] for the ledger-declared names only (bounded work)."""
    idx = {}
    if not wanted:
        return idx
    for rel in rels:
        raw = read_text(rel)
        if raw is None or "pub " not in raw:
            continue
        for kind, name in DECL_KIND_RE.findall(raw):
            if name in wanted:
                idx.setdefault(name, [])
                if rel not in idx[name]:
                    idx[name].append(rel)
    return idx


def struct_body_lines(lines, decl_idx):
    """Characters strictly inside a braced struct body, or None if not braced."""
    depth = 0
    started = False
    buf = []
    for i in range(decl_idx, len(lines)):
        for ch in lines[i]:
            if not started:
                if ch == "{":
                    started = True
                    depth = 1
                elif ch in ";)":
                    return None  # unit or tuple struct: no named field order
                continue
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
                if depth == 0:
                    return "".join(buf)
            buf.append(ch if depth == 1 else " ")
        if started:
            buf.append("\n")
    return None


def parse_fields(body):
    """(ordered names, skip-set, default-set) from already-blanked body text."""
    names = []
    skips = set()
    defaults = set()
    pending = []
    for raw in body.splitlines():
        s = raw.strip()
        if not s:
            continue
        if s.startswith("#"):
            pending.append(s)
            continue
        m = FIELD_RE.match(s)
        if not m:
            pending = []
            continue
        name = m.group(1)
        attr = " ".join(pending)
        if SERDE_ATTR_RE.search(attr):
            if DEFAULT_RE.search(attr):
                defaults.add(name)
            if SKIP_RE.search(attr):
                skips.add(name)
        names.append(name)
        pending = []
    return names, skips, defaults


def parse_struct_body(rel, type_name):
    """(names, skips, defaults) for a named-field struct; None if undecidable shape."""
    lines, mask = blanked_masked(rel)
    if lines is None:
        return None
    pat = re.compile(r"^[ \t]*pub(?:\([^)]*\))?[ \t]+struct[ \t]+"
                     + re.escape(type_name) + r"\b")
    for i, line in enumerate(lines):
        if mask[i]:
            continue
        if not pat.search(line):
            continue
        body = struct_body_lines(lines, i)
        if body is None:
            return None
        return parse_fields(body)
    return None


def site_key(s):
    return "%s::%s" % (s["file"], s["fn"])


def read_ledger(path):
    """(rows, malformed). Comment/blank lines are not rows."""
    if not os.path.isfile(path):
        return [], ["ledger not found: %s" % path]
    rows = []
    malformed = []
    with open(path, "r", encoding="utf-8") as fh:
        for no, raw in enumerate(fh, 1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            m = ROW_RE.match(line)
            if not m:
                malformed.append("%s:%d not a WRITE/EXEMPT row: '%s'"
                                 % (os.path.basename(path), no, line[:64]))
                continue
            tag, rest = m.group(1), m.group(2)
            kv = {}
            bad = False
            for part in rest.split("|"):
                part = part.strip()
                if not part:
                    continue
                km = KV_RE.match(part)
                if not km:
                    malformed.append("%s:%d not key=value: '%s'"
                                     % (os.path.basename(path), no, part[:40]))
                    bad = True
                    break
                if km.group(1) in kv:
                    malformed.append("%s:%d duplicate key %s"
                                     % (os.path.basename(path), no, km.group(1)))
                    bad = True
                    break
                kv[km.group(1)] = km.group(2).strip()
            if bad:
                continue
            missing = [k for k in ROW_KEYS[tag] if k not in kv or not kv[k]]
            unknown = sorted(k for k in kv if k not in ROW_KEYS[tag] + ROW_OPTIONAL[tag])
            if missing:
                malformed.append("%s:%d %s row missing required %s"
                                 % (os.path.basename(path), no, tag, ",".join(missing)))
                continue
            if unknown:
                malformed.append("%s:%d %s row has unknown key(s) %s"
                                 % (os.path.basename(path), no, tag, ",".join(unknown)))
                continue
            ev = kv["evidence"] if tag == "WRITE" else kv["reason"]
            if len(ev) < MIN_EVIDENCE_LEN:
                malformed.append("%s:%d %s evidence is %d chars, below the %d-char floor"
                                 % (os.path.basename(path), no, tag, len(ev),
                                    MIN_EVIDENCE_LEN))
                continue
            kv["_tag"] = tag
            rows.append(kv)
    seen = set()
    for r in rows:
        if r["site"] in seen:
            malformed.append("duplicate site key: %s" % r["site"])
        seen.add(r["site"])
    return rows, malformed


def read_orphans(path):
    out = set()
    if not os.path.isfile(path):
        return out
    with open(path, "r", encoding="utf-8") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if line:
                out.add(line.replace(os.sep, "/"))
    return out


def evaluate(sites, rows, orphans, decl_index):
    """Pure verdict -> (rc, out_lines, stats). rc 2 dominates 1 dominates 0."""
    write = dict((r["site"], r) for r in rows if r["_tag"] == "WRITE")
    exempt = dict((r["site"], r) for r in rows if r["_tag"] == "EXEMPT")
    out = []
    tool = []
    fails = 0
    orphan_sites = [s for s in sites if s["file"] in orphans]
    judged = [s for s in sites if s["file"] not in orphans]
    live_keys = set(site_key(s) for s in sites)
    stats = {"sites": len(sites), "judged": len(judged),
             "orphan_excluded": len(orphan_sites), "write_rows": len(write),
             "exempt_rows": len(exempt)}

    for s in judged:
        key = site_key(s)
        if key in exempt:
            continue
        if key not in write:
            out.append("[GAP] NEW positional write point without a ledger row: %s "
                       "(arg=%s) -- add WRITE, or EXEMPT with a >=%d-char reason, in the "
                       "same commit" % (key, s["arg"], MIN_EVIDENCE_LEN))
            fails += 1
            continue
        r = write[key]
        if r["arg"] != (s["arg"] or ""):
            out.append("[GAP] ARG_DRIFT: %s encodes '%s' but the row pins '%s'"
                       % (key, s["arg"], r["arg"]))
            fails += 1
        if r.get("in"):
            if r["in"] not in decl_index.get(r["type"], []):
                tool.append("[TOOL-ERROR] BAD_IN_ANCHOR: %s pins in=%s which does not "
                            "host type=%s" % (key, r["in"], r["type"]))
                continue
            decl_files = [r["in"]]
        else:
            decl_files = sorted(set(decl_index.get(r["type"], [])))
            if len(decl_files) > 1:
                tool.append("[TOOL-ERROR] AMBIGUOUS_TYPE: %s declares type=%s present in "
                            "%d files -- pin one with in=<file.rs>"
                            % (key, r["type"], len(decl_files)))
                continue
            if not decl_files:
                tool.append("[TOOL-ERROR] TYPE_NOT_RESOLVED: %s declares type=%s found in "
                            "no crates/*/src declaration" % (key, r["type"]))
                continue
        parsed = parse_struct_body(decl_files[0], r["type"])
        if parsed is None:
            tool.append("[TOOL-ERROR] UNSUPPORTED_SHAPE: %s type=%s is not a named-field "
                        "struct, so field order is undecidable -- a human must move it to "
                        "named encoding or remove the positional write point"
                        % (key, r["type"]))
            continue
        live, live_skips, live_defaults = parsed
        reg = [f for f in r["fields"].split(",") if f]
        reg_skips = set(f for f in r.get("skips", "").split(",") if f)
        if live == reg:
            new_skips = live_skips - reg_skips
            if new_skips:
                out.append("[GAP] R9_SKIP_ON_ARRAY: %s carries skippable field(s) %s not "
                           "in the row while persisted positionally at %s -- array "
                           "encoding puts a non-skipped value into the skipped "
                           "neighbour's slot; matching neighbour type => SILENT wrong "
                           "value" % (r["type"], ",".join(sorted(new_skips)), key))
                fails += 1
            continue
        if len(live) > len(reg) and live[:len(reg)] == reg:
            extra = live[len(reg):]
            out.append("[GAP] UNREGISTERED_TAIL_APPEND: %s grew by %s at %s -- tail append "
                       "is the sanctioned growth shape, but the frozen order must record "
                       "it (re-emit this row in the same commit)"
                       % (r["type"], ",".join(extra), key))
            fails += 1
            r9 = sorted(set(extra) & live_skips)
            if r9:
                out.append("[GAP] R9_SKIP_ON_ARRAY: %s appended skippable field(s) %s on a "
                           "positionally persisted type at %s -- forbidden by ADR-213 R9"
                           % (r["type"], ",".join(r9), key))
                fails += 1
            no_def = [f for f in extra if f not in live_defaults]
            if no_def:
                out.append("[GAP] NO_DEFAULT_ON_APPEND: %s appended %s without "
                           "#[serde(default)] at %s -- old payloads then fail to decode; "
                           "add the default or declare a breaking change"
                           % (r["type"], ",".join(no_def), key))
                fails += 1
            continue
        out.append("[FAIL] ORDER_DRIFT: %s live order [%s] != frozen [%s] at %s -- under "
                   "array encoding a same-typed swap decodes Ok with the values "
                   "INTERCHANGED (measured in wire_format_compat.rs); reordering here is "
                   "silent corruption of stored data, not a refactor"
                   % (r["type"], ",".join(live), ",".join(reg), key))
        fails += 1

    for key in sorted(set(write) - live_keys):
        out.append("[STALE] WRITE row has no live site: %s -- positional write points are "
                   "removed on purpose; delete the row in the same commit" % key)
        fails += 1
    for key in sorted(set(exempt) - live_keys):
        out.append("[STALE] EXEMPT row has no live site: %s -- a waiver for a vanished site "
                   "is a ledger lie" % key)
        fails += 1

    if fails:
        out.extend(tool)
        out.append("VERDICT: BROKEN %d violation(s) + %d undecidable; sites=%d judged=%d "
                   "sanctioned=%d waived=%d orphan-excluded=%d"
                   % (fails, len(tool), stats["sites"], stats["judged"],
                      stats["write_rows"], stats["exempt_rows"],
                      stats["orphan_excluded"]))
        return 1, out, stats
    if tool:
        out.extend(tool)
        out.append("VERDICT: UNDECIDABLE (%d) -> exit 2, never a silent pass" % len(tool))
        return 2, out, stats
    out.append("[OK] every positionally persisted type matches its frozen field order: "
               "sites=%d judged=%d sanctioned=%d waived=%d orphan-excluded=%d "
               "(only-decrease ledger)" % (stats["sites"], stats["judged"],
                                           stats["write_rows"], stats["exempt_rows"],
                                           stats["orphan_excluded"]))
    return 0, out, stats


def emit_candidates():
    rels = iter_src_files(os.path.join(ROOT, "crates"))
    sites = scan_sites(rels)
    orphans = read_orphans(ORPHAN_LEDGER)
    path = os.path.join(ROOT, "tmp", "positional_field_order_candidate.txt")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write("# candidates from the live tree (%d src files walked)\n" % len(rels))
        fh.write("# type=/fields=/skips= are NOT derivable from a call site alone and a "
                 "human must bind them. Do not paste these rows unedited.\n")
        for s in sorted(sites, key=site_key):
            if s["file"] in orphans:
                fh.write("# ORPHAN-EXCLUDED (G-109) %s arg=%s\n"
                         % (site_key(s), s["arg"]))
                continue
            fh.write("WRITE | site=%s | arg=%s | type=<REVIEW> | fields=<REVIEW> | "
                     "skips= | evidence=<REVIEW %d+ chars>\n"
                     % (site_key(s), s["arg"], MIN_EVIDENCE_LEN))
    print_line("[EMIT] %d site(s) listed -> %s" % (len(sites), repo_rel(path)))
    return 0


def run_real():
    rels = iter_src_files(os.path.join(ROOT, "crates"))
    if len(rels) < SCANNED_FLOOR:
        print_line("[TOOL-ERROR] walked only %d src files (< floor %d): scan surface "
                   "collapsed, refusing to judge" % (len(rels), SCANNED_FLOOR))
        return 2
    sites = scan_sites(rels)
    rows, malformed = read_ledger(LEDGER)
    if malformed:
        for m in malformed:
            print_line("[TOOL-ERROR][malformed] %s" % m)
        print_line("[TOOL-ERROR] malformed ledger rows -> undecidable, exit 2")
        return 2
    wanted = set(r["type"] for r in rows if r["_tag"] == "WRITE")
    rc, out, stats = evaluate(sites, rows, read_orphans(ORPHAN_LEDGER),
                              index_decls(rels, wanted))
    for line in out:
        print_line(line)
    if stats["write_rows"] == 0:
        print_line("[NOTE] zero sanctioned positional types: this gate is admission "
                   "control, not cleanup -- ADR-213 R8.2/R9 bind new sites only.")
    return rc


def print_line(text):
    print(text.encode("ascii", "replace").decode("ascii"))


# --- selftest ---------------------------------------------------------------

S_FILE = "crates/fake_crate/src/lib.rs"
ORPH_FILE = "crates/fake_crate/src/dead.rs"

# A frozen record persisted positionally, plus three decoys that must NOT count as sites:
# a doc comment, a string literal and a cfg(test) tail.
CLEAN_SRC = (
    '//! Doc comment mentioning rmp_serde::to_vec(&x) is not a site.\n'
    'pub struct WalEntry {\n'
    '    #[serde(default)]\n'
    '    pub ts: u64,\n'
    '    pub payload: Vec<u8>,\n'
    '    #[serde(default, skip_serializing_if = "Option::is_none")]\n'
    '    pub note: Option<u32>,\n'
    '}\n'
    'pub struct Tupleish(pub u8, pub u8);\n'
    'pub fn write(it: &WalEntry) -> Vec<u8> {\n'
    '    let _s = format!("inside a string rmp_serde::to_vec(&9u8)");\n'
    '    rmp_serde::to_vec(it).unwrap()\n'
    '}\n'
    '#[cfg(test)]\n'
    'mod tests {\n'
    '    fn in_tail() { let _ = rmp_serde::to_vec(&0u8); }\n'
    '}\n'
)
# ts/payload swapped, both same-typed neighbours: the silent corruption R8.2 freezes.
SWAPPED_SRC = CLEAN_SRC.replace(
    '    #[serde(default)]\n    pub ts: u64,\n    pub payload: Vec<u8>,\n',
    '    #[serde(default)]\n    pub payload: Vec<u8>,\n    pub ts: u64,\n')
R9_SRC = CLEAN_SRC.replace(
    '    pub note: Option<u32>,\n}',
    '    pub note: Option<u32>,\n'
    '    #[serde(default, skip_serializing_if = "Option::is_none")]\n'
    '    pub extra: Option<u32>,\n}')
APPEND_SRC = CLEAN_SRC.replace(
    '    pub note: Option<u32>,\n}',
    '    pub note: Option<u32>,\n    pub seq: u64,\n}')
# Same field names/order as the frozen row, but declared as a tuple struct.
TUPLE_SRC = CLEAN_SRC.replace(
    'pub struct WalEntry {\n    #[serde(default)]\n    pub ts: u64,\n'
    '    pub payload: Vec<u8>,\n    #[serde(default, skip_serializing_if = '
    '"Option::is_none")]\n    pub note: Option<u32>,\n}',
    'pub struct WalEntry(pub u64, pub Vec<u8>, pub Option<u32>);')

SITE = "%s::write" % S_FILE
EV = "class B short-lived WAL record per ADR-213 R8.1"


def clean_row(fields="ts,payload,note", skips="note", arg="it", type_name="WalEntry"):
    return ("WRITE | site=%s | arg=%s | type=%s | fields=%s | skips=%s | evidence=%s"
            % (SITE, arg, type_name, fields, skips, EV))


def exempt_row():
    return ("EXEMPT | site=%s | reason=in-memory popup hex preview, never persisted"
            % SITE)


def parse_rows(text_rows):
    """Read candidate rows through the SAME reader the real run uses."""
    tmpd = tempfile.mkdtemp(prefix="posorder-led-")
    try:
        path = os.path.join(tmpd, "l.txt")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("\n".join(text_rows) + "\n")
        return read_ledger(path)
    finally:
        shutil.rmtree(tmpd, ignore_errors=True)


def selftest():
    ok = True

    def add(label, cond):
        nonlocal ok
        if cond:
            print_line("[SELFTEST] %s" % label)
        else:
            ok = False
            print_line("[SELFTEST-FAIL] %s" % label)

    global ROOT
    real_root = ROOT
    tmp = tempfile.mkdtemp(prefix="posorder-")
    try:
        ROOT = tmp

        def mk(rel, text):
            path = os.path.join(tmp, rel.replace("/", os.sep))
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(text)

        crates = os.path.join(tmp, "crates")
        mk(S_FILE, CLEAN_SRC)
        mk(ORPH_FILE, "pub fn dead() { let _ = rmp_serde::to_vec(&0u8); }\n")
        rels = iter_src_files(crates)
        add("walk finds both fixture files", len(rels) == 2)
        sites = scan_sites(rels)
        # Two real sites (lib.rs production + dead.rs orphan); the three decoys above must
        # have produced none. dead.rs sorts first, so select by file rather than by index.
        add("both real sites found and all three decoys excluded",
            sorted(site_key(s) for s in sites) == sorted(
                [SITE, "%s::dead" % ORPH_FILE]))
        lib_sites = [s for s in sites if s["file"] == S_FILE]
        add("argument captured from blanked code",
            len(lib_sites) == 1 and lib_sites[0]["arg"] == "it")
        add("enclosing fn is the stable key",
            len(lib_sites) == 1 and lib_sites[0]["fn"] == "write")
        orphans = {ORPH_FILE}
        idx = index_decls(rels, {"WalEntry"})
        rows, mal = parse_rows([clean_row()])
        add("clean row parses without complaint", not mal and len(rows) == 1)
        rc, out, _ = evaluate(sites, rows, orphans, idx)
        add("clean fixture + matching frozen order -> rc 0", rc == 0)

        # 1. THE reason the gate exists: a same-typed swap must be loud.
        mk(S_FILE, SWAPPED_SRC)
        rc, out, _ = evaluate(scan_sites(rels), rows, orphans, index_decls(rels, {"WalEntry"}))
        add("planted same-typed field swap caught as ORDER_DRIFT",
            rc == 1 and any("ORDER_DRIFT" in l for l in out))
        mk(S_FILE, CLEAN_SRC)

        # 2. Unsanctioned write point.
        mk(S_FILE, CLEAN_SRC + "pub fn sneaky(e: &WalEntry) { let _ = rmp_serde::to_vec(e); }\n")
        rc, out, _ = evaluate(scan_sites(rels), rows, orphans, index_decls(rels, {"WalEntry"}))
        add("new unregistered positional write point caught",
            rc == 1 and any("NEW positional" in l for l in out))
        mk(S_FILE, CLEAN_SRC)

        # 3. R9, and its negation: the same code is fine once the row records the skip.
        mk(S_FILE, R9_SRC)
        rc, out, _ = evaluate(scan_sites(rels), rows, orphans, index_decls(rels, {"WalEntry"}))
        add("R9: skippable field on a positionally persisted type caught",
            rc == 1 and any("R9_SKIP_ON_ARRAY" in l for l in out))
        r9_rows, _m = parse_rows([clean_row(fields="ts,payload,note,extra",
                                            skips="note,extra")])
        rc, out, _ = evaluate(scan_sites(rels), r9_rows, orphans, index_decls(rels, {"WalEntry"}))
        add("identical code recorded in the row -> rc 0 (judges drift, not mere form)",
            rc == 0)

        mk(S_FILE, APPEND_SRC)
        rc, out, _ = evaluate(scan_sites(rels), rows, orphans, index_decls(rels, {"WalEntry"}))
        add("unrecorded tail append must be registered",
            rc == 1 and any("UNREGISTERED_TAIL_APPEND" in l for l in out))
        add("appended field lacking #[serde(default)] is named",
            any("NO_DEFAULT_ON_APPEND" in l for l in out))
        mk(S_FILE, CLEAN_SRC)

        # 4. Argument drift.
        r2, _m = parse_rows([clean_row(arg="stale_binding")])
        rc, out, _ = evaluate(sites, r2, orphans, idx)
        add("site encodes something other than the pinned arg -> caught",
            rc == 1 and any("ARG_DRIFT" in l for l in out))

        # 5. Undecidable states exit 2, never 1, never 0.
        add("missing required keys -> malformed",
            bool(parse_rows(["WRITE | site=a::b | arg=x | type=T"])[1]))
        add("unknown key -> malformed", bool(parse_rows([clean_row() + " | bogus=1"])[1]))
        add("evidence below the %d-char floor -> malformed" % MIN_EVIDENCE_LEN,
            bool(parse_rows([clean_row().replace(EV, "short")])[1]))
        add("duplicate site key -> malformed",
            bool(parse_rows([clean_row(), clean_row()])[1]))
        ghost, _m = parse_rows([clean_row(type_name="GhostType")])
        rc, out, _ = evaluate(sites, ghost, orphans, idx)
        add("row declaring a nonexistent type -> rc 2",
            rc == 2 and any("TYPE_NOT_RESOLVED" in l for l in out))
        mk(S_FILE, TUPLE_SRC)
        rc, out, _ = evaluate(scan_sites(rels), rows, orphans,
                              index_decls(rels, {"WalEntry"}))
        add("tuple struct (order undecidable) -> rc 2, forces a human decision",
            rc == 2 and any("UNSUPPORTED_SHAPE" in l for l in out))
        mk(S_FILE, CLEAN_SRC)
        idx_amb = dict(idx)
        idx_amb["WalEntry"] = [S_FILE, "crates/other/src/x.rs"]
        rc, out, _ = evaluate(sites, rows, orphans, idx_amb)
        add("type present in two files without an in= anchor -> rc 2",
            rc == 2 and any("AMBIGUOUS_TYPE" in l for l in out))

        # 6. The floor must sit under the REAL surface (the fixture tree is 2 files).
        n_real = len(iter_src_files(os.path.join(real_root, "crates")))
        add("SCANNED_FLOOR above zero and below the real walk (%d files)" % n_real,
            0 < SCANNED_FLOOR < n_real)

        # 7. Orphan linkage: wiring a G-109 file must not open a blind spot.
        rc, _o, st = evaluate(scan_sites(rels), rows, orphans, index_decls(rels, {"WalEntry"}))
        add("orphan-listed file's write point excluded but counted",
            rc == 0 and st["orphan_excluded"] == 1)
        rc, out, _ = evaluate(scan_sites(rels), rows, set(), index_decls(rels, {"WalEntry"}))
        add("removing only the G-109 row flips that site to NEW (linkage has teeth)",
            rc == 1 and any("NEW positional" in l and ORPH_FILE in l for l in out))

        # 8. Deletion without unregistering the row.
        os.remove(os.path.join(tmp, S_FILE.replace("/", os.sep)))
        rc, out, _ = evaluate(scan_sites(rels), rows, orphans, index_decls(rels, {"WalEntry"}))
        add("site deleted but row kept -> STALE", rc == 1 and any("[STALE]" in l
                                                                  for l in out))
        rc, _o, _s = evaluate(scan_sites(rels), [], orphans, {})
        add("paired deletion (row removed in same commit) -> rc 0", rc == 0)

        # 9. Exemption path, and that a waiver cannot outlive its site.
        mk(S_FILE, CLEAN_SRC)
        ex_rows, _m = parse_rows([exempt_row()])
        rc, _o, _s = evaluate(scan_sites(rels), ex_rows, orphans, {})
        add("EXEMPT row silences its site legitimately", rc == 0)
        rc, out, _ = evaluate([], ex_rows, orphans, {})
        add("waiver for a vanished site -> STALE (no ledger lies)",
            rc == 1 and any("[STALE] EXEMPT" in l for l in out))
    finally:
        ROOT = real_root
        shutil.rmtree(tmp, ignore_errors=True)
    print_line("[SELFTEST] %s" % ("ALL ASSERTIONS HELD" if ok else "FAILED"))
    return 0 if ok else 1


def main(argv):
    for a in argv[1:]:
        if a not in ("--selftest", "--emit"):
            sys.stderr.write("usage: check_positional_field_order_freeze.py "
                             "[--selftest|--emit]\n")
            return 2
    if "--selftest" in argv[1:]:
        return selftest()
    if "--emit" in argv[1:]:
        return emit_candidates()
    return run_real()


if __name__ == "__main__":
    import gate_rc  # crash -> exit 2, never a borrowed traceback exit 1

    sys.exit(gate_rc.run(lambda: main(sys.argv)))
