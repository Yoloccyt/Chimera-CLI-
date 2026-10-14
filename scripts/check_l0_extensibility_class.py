#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L0 public-type extensibility class ledger (G-115, ADR-214 decision 3 / D-22).

Why this gate exists
--------------------
Measured on 2026-10-14: nexus-contracts declares 103 `pub enum` and 125 `pub struct`
and carries **zero** `#[non_exhaustive]`. So every public L0 type is CLOSED by default,
which means any future variant or field is a source-level breaking change for the 30+
consumer crates -- and that is exactly what the last two waves kept hitting (61 external
struct literals on ExperienceCard; an E0063 that only appeared once `--all-targets` ran).

Marking types `#[non_exhaustive]` is NOT unconditionally good, and this repository already
relies on the opposite mechanism where it matters: `crates/event-bus/src/registry.rs`
expands FIVE wildcard-free matches so that a new `NexusEvent` variant without a severity
fails to compile. Exhaustive matching is a safety feature for verdict/failure/severity-like
enums; it is friction for display/format/strategy-like ones.

Therefore the decision is per type, and a per-type decision that lives only in prose rots.
This gate turns it into a machine-checked contract:

  * every public L0 enum/struct has exactly one ledger row declaring its class;
  * class=open  <=> the type carries `#[non_exhaustive]` (and must carry a reviewable reason);
  * class=closed <=> the type carries no such attribute;
  * adding an attribute without flipping the row, or flipping the row without adding the
    attribute, is red -- so opening a type is always a two-line deliberate act in one commit.

The ledger is only-decrease in row COUNT for a given class in the sense that STALE rows and
NEW types both red out; the policy direction (closed -> open) is allowed but costs a reason.

What it judges
--------------
Input surface: `crates/nexus-contracts/src/**/*.rs`, blanked by the SAME lexer as G-99/G-105
and with `#[cfg(test)]` tails masked (imported, not re-implemented). Test-only declarations
never enter the class ledger.

  NEW            a public type with no row -> red (declare its class deliberately)
  STALE          a row whose type is gone -> red (rename/delete must drop the row)
  CLASS_MISMATCH row says open but no attribute, or closed but attribute present -> red
  OPEN_NO_REASON class=open without a reason of >= MIN_REASON_LEN chars -> malformed (2)
  malformed row / unknown key / duplicate type key / parser blindness (extracted types
  below TYPES_FLOOR) / unreadable crate root -> exit 2. Undecidable is never a pass.

Known limits (stated rather than hidden)
----------------------------------------
  * Attribute association is textual: a declaration's attribute block is the contiguous run
    of lines above it that start with `#`, `]` or are blank-after-blanking. An `#[serde(...)]`
    style attribute split across lines is folded by joining that run; exotic spacing could
    mis-associate, which would surface as CLASS_MISMATCH (a review prompt), never as silence.
  * The gate judges presence of the attribute, not whether the chosen class is CORRECT.
    Choosing open vs closed is a design decision; this gate makes it explicit and reversible,
    it does not make it for the reviewer. Hence the reason field is mandatory for `open`.
  * `#[non_exhaustive]` on an enum does not restrict variant construction, and does not
    affect same-crate exhaustive matching (the attribute is cross-crate only). Both are
    intended here: the ledger tracks the downstream contract, not in-crate ergonomics.

Modes / exit codes
------------------
  (default)   judge the real repo        0 = agree, 1 = NEW/STALE/CLASS_MISMATCH
  --emit      dump the candidate ledger (class reflects the live attribute state, so the
              emitted baseline is self-consistent and a first real run is green)
                                       0 = written, 2 = failure
  --selftest  planted fixtures prove every judgement bites, in both directions
                                       0 = all held, 1 = not
  usage / undecidable                   2

Output is pure ASCII (project script convention; avoids Windows GBK console traps).
"""

import os
import re
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
CRATE_DIR = os.path.join(ROOT, "crates", "nexus-contracts")
LEDGER = os.path.join(HERE, "l0_extensibility_class_freeze.txt")

# Refuse to judge if fewer public types than this are extracted: a blind parser reports
# "no violations" exactly like a compliant tree. Measured surface 2026-10-14: 228 types.
TYPES_FLOOR = 150

# A row that opens a type must say why, at reviewable length. Same floor as G-99/G-111.
MIN_REASON_LEN = 24

sys.path.insert(0, HERE)
import check_l0_purity as purity  # noqa: E402  (shares lexer + cfg(test) tail mask)
from check_l0_pubfn_ratchet import module_key  # noqa: E402 (shares the module-path rule)

TYPE_DECL_RE = re.compile(r"^[ \t]*pub[ \t]+(enum|struct)[ \t]+([A-Za-z_]\w*)")
ROW_RE = re.compile(r"^(ENUM|STRUCT)\s*\|(.*)$")
KV_RE = re.compile(r"^([a-z_]+)=(.*)$")
ROW_KEYS = ("type", "class")


def repo_rel(path):
    return os.path.relpath(path, ROOT).replace(os.sep, "/")


def iter_src_files(src_dir):
    out = []
    if not os.path.isdir(src_dir):
        return out
    for dirpath, _dn, files in os.walk(src_dir):
        for fn in sorted(files):
            if fn.endswith(".rs"):
                out.append(os.path.join(dirpath, fn))
    return out


def attr_block(lines, idx):
    """Attribute text governing the declaration at `idx` (its own line plus the
    contiguous attribute run immediately above it)."""
    buf = [lines[idx]]
    j = idx - 1
    while j >= 0:
        s = lines[j].strip()
        if s.startswith("#") or s.startswith("]") or s == "":
            buf.insert(0, lines[j])
            if s == "" and j < idx - 1:
                # a blank line between an attribute run and unrelated code ends the run
                # only when we already collected at least one attribute line
                if any(x.strip().startswith("#") for x in buf[:-1]):
                    break
            j -= 1
            continue
        break
    return " ".join(buf)


def extract_types(src_dir):
    """[(kind, module_path, name, is_non_exhaustive)] over the production surface."""
    rows = []
    for path in iter_src_files(src_dir):
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                src = fh.read()
        except (IOError, OSError):
            continue
        lines = purity.blank_rust(src).splitlines()
        mask = purity.test_tail_mask(lines)
        mod = module_key(path, src_dir)
        for i, line in enumerate(lines):
            if mask[i]:
                continue
            m = TYPE_DECL_RE.match(line)
            if not m:
                continue
            kind, name = m.group(1), m.group(2)
            ne = "non_exhaustive" in attr_block(lines, i)
            rows.append((kind, mod, name, ne))
    return rows


def type_key(kind, mod, name):
    return "%s|%s::%s" % (kind.upper(), mod, name)


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
                malformed.append("%s:%d not an ENUM/STRUCT row: '%s'"
                                 % (os.path.basename(path), no, line[:64]))
                continue
            kind, rest = m.group(1), m.group(2)
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
            missing = [k for k in ROW_KEYS if k not in kv or not kv[k]]
            unknown = sorted(k for k in kv if k not in ROW_KEYS + ("reason",))
            if missing:
                malformed.append("%s:%d row missing required %s"
                                 % (os.path.basename(path), no, ",".join(missing)))
                continue
            if unknown:
                malformed.append("%s:%d row has unknown key(s) %s"
                                 % (os.path.basename(path), no, ",".join(unknown)))
                continue
            if kv["class"] not in ("open", "closed"):
                malformed.append("%s:%d class must be open|closed, got '%s'"
                                 % (os.path.basename(path), no, kv["class"]))
                continue
            if kv["class"] == "open" and len(kv.get("reason", "")) < MIN_REASON_LEN:
                malformed.append("%s:%d class=open needs a reason of >= %d chars (got %d)"
                                 % (os.path.basename(path), no, MIN_REASON_LEN,
                                    len(kv.get("reason", ""))))
                continue
            kv["_kind"] = kind
            rows.append(kv)
    seen = {}
    for r in rows:
        k = (r["_kind"], r["type"])
        if k in seen:
            malformed.append("duplicate type row: %s %s" % (k[0], k[1]))
        seen[k] = True
    return rows, malformed


def evaluate(types, rows):
    """Pure verdict -> (rc, out_lines, stats). Keys are type_key() strings throughout."""
    listed = dict(("%s|%s" % (r["_kind"], r["type"]), r) for r in rows)
    live = dict((type_key(kind, mod, name), ne) for (kind, mod, name, ne) in types)
    out = []
    fails = 0
    opens = sum(1 for r in rows if r["class"] == "open")
    stats = {"types": len(live), "listed": len(listed), "open": opens,
             "closed": len(listed) - opens}

    for key in sorted(live):
        ne = live[key]
        r = listed.get(key)
        if r is None:
            out.append("[GAP] NEW public type with no extensibility class row: %s "
                       "(#[non_exhaustive] present=%s) -- declare its class in the ledger "
                       "as open or closed, deliberately" % (key, str(ne)))
            fails += 1
            continue
        want_open = (r["class"] == "open")
        if want_open != ne:
            if want_open:
                out.append("[GAP] CLASS_MISMATCH: %s is declared open but carries no "
                           "#[non_exhaustive] -- opening a type and adding the attribute are "
                           "ONE deliberate act (same commit)" % key)
            else:
                out.append("[GAP] CLASS_MISMATCH: %s is declared closed yet carries "
                           "#[non_exhaustive] -- closing a type silently removes downstream "
                           "compile-forced handling; flip the row (with a reason) or drop the "
                           "attribute" % key)
            fails += 1

    for key in sorted(listed):
        if key not in live:
            out.append("[STALE] class row for a type that no longer exists at that path: "
                       "%s -- renames/moves must re-key the row in the same commit" % key)
            fails += 1

    if fails:
        out.append("L0 extensibility class ledger BROKEN: %d violation(s); types=%d "
                   "listed=%d open=%d closed=%d"
                   % (fails, stats["types"], stats["listed"], stats["open"],
                      stats["closed"]))
        return 1, out, stats
    out.append("[OK] every public L0 type declares exactly one extensibility class and the "
               "attribute matches it: types=%d listed=%d open=%d closed=%d"
               % (stats["types"], stats["listed"], stats["open"], stats["closed"]))
    return 0, out, stats


def print_line(text):
    print(text.encode("ascii", "replace").decode("ascii"))


def emit_and_judge(types):
    """--emit writes a self-consistent baseline ONLY after the floor guard has passed."""
    out_path = os.path.join(ROOT, "tmp", "l0_extensibility_class_candidate.txt")
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as fh:
        fh.write("# emitted from the live tree; class mirrors the CURRENT attribute state,\n")
        fh.write("# so every emitted row agrees with the tree by construction. Review before\n")
        fh.write("# promoting to the real ledger: opening a type is a decision, not a default.\n")
        for kind, mod, name, ne in sorted(types):
            fh.write("%s | type=%s::%s | class=%s\n"
                     % (kind.upper(), mod, name, "open" if ne else "closed"))
    print_line("[EMIT] %d type(s) -> %s" % (len(types), repo_rel(out_path)))
    return 0


def run_real():
    src_dir = os.path.join(CRATE_DIR, "src")
    if not os.path.isdir(src_dir):
        print_line("[TOOL-ERROR] L0 source not found: %s" % src_dir)
        return 2
    types = extract_types(src_dir)
    # The floor guard runs BEFORE emit on purpose: a blind parser must not be able to
    # write a "clean" baseline, which would launder its own blindness into the ledger.
    if len(types) < TYPES_FLOOR:
        print_line("[TOOL-ERROR] extracted only %d public types (< floor %d): parser or "
                   "surface is broken, refusing to judge or to emit"
                   % (len(types), TYPES_FLOOR))
        return 2
    if "--emit" in sys.argv[1:]:
        return emit_and_judge(types)
    rows, malformed = read_ledger(LEDGER)
    if malformed:
        for m in malformed:
            print_line("[TOOL-ERROR][malformed] %s" % m)
        print_line("[TOOL-ERROR] malformed class rows -> undecidable, exit 2")
        return 2
    rc, out, _stats = evaluate(types, rows)
    for line in out:
        print_line(line)
    return rc


# --- selftest ---------------------------------------------------------------

CLEAN_SRC = '''\
#![forbid(unsafe_code)]
pub mod m;
/// doc comment mentioning #[non_exhaustive] must not count as an attribute
pub enum ClosedKind {
    A,
    B,
}
#[non_exhaustive]
pub enum OpenKind {
    A,
}
#[derive(Debug, Clone)]
pub enum DerivedClosed {
    A,
}
pub struct Plain {
    pub x: u8,
}
#[non_exhaustive]
pub struct OpenStruct {
    pub x: u8,
}
#[cfg(test)]
mod tests {
    pub enum TailOnly {
        A,
    }
}
'''


def parse_rows(text_rows):
    tmpd = tempfile.mkdtemp(prefix="l0ext-led-")
    try:
        p = os.path.join(tmpd, "l.txt")
        with open(p, "w", encoding="utf-8") as fh:
            fh.write("\n".join(text_rows) + "\n")
        return read_ledger(p)
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

    tmp = tempfile.mkdtemp(prefix="l0ext-")
    try:
        src = os.path.join(tmp, "src")
        os.makedirs(src)
        with open(os.path.join(src, "lib.rs"), "w", encoding="utf-8") as fh:
            fh.write(CLEAN_SRC)
        os.makedirs(os.path.join(src, "m"))
        with open(os.path.join(src, "m", "mod.rs"), "w", encoding="utf-8") as fh:
            fh.write("pub enum InSub { A }\n")
        types = extract_types(src)
        keys = set(type_key(k, mo, n) for (k, mo, n, _ne) in types)
        # Strict set equality on purpose. An earlier draft of this assertion ended with
        # `or len(types) == 6`, which stayed GREEN while every key was wrong (lib.rs is the
        # module `lib`, not `m`) -- a tautological guard is a silent guard.
        add("production surface = exactly the 6 expected keys (crate root is module `lib`)",
            keys == {"ENUM|lib::ClosedKind", "ENUM|lib::OpenKind",
                     "ENUM|lib::DerivedClosed", "STRUCT|lib::Plain",
                     "STRUCT|lib::OpenStruct", "ENUM|m::InSub"})
        add("cfg(test) tail is masked (TailOnly absent)",
            not any("TailOnly" in k for k in keys))
        ne = dict((type_key(k, mo, n), v) for (k, mo, n, v) in types)
        add("attribute detected exactly on the two declared-open types",
            ne["ENUM|lib::OpenKind"] and ne["STRUCT|lib::OpenStruct"]
            and not ne["ENUM|lib::ClosedKind"] and not ne["ENUM|lib::DerivedClosed"])
        add("doc-comment mention does NOT count as an attribute",
            not ne["ENUM|lib::ClosedKind"])
        good = [
            "ENUM | type=lib::ClosedKind | class=closed",
            "ENUM | type=lib::OpenKind | class=open | reason=display-only format vocabulary, "
            "adding a variant must never break a downstream match",
            "ENUM | type=lib::DerivedClosed | class=closed",
            "STRUCT | type=lib::Plain | class=closed",
            "STRUCT | type=lib::OpenStruct | class=open | reason=blob carrier whose fields "
            "grow every release; consumers must not build it by literal",
            "ENUM | type=m::InSub | class=closed",
        ]
        rows, mal = parse_rows(good)
        add("consistent fixture parses cleanly", not mal and len(rows) == 6)
        rc, _o, st = evaluate(types, rows)
        add("class matches attribute everywhere -> rc 0", rc == 0)
        add("stats count open and closed separately",
            st["open"] == 2 and st["closed"] == 4)

        # 1. Opening a type without flipping the row must be red.
        flipped = [(k, mo, n, True if (n == "ClosedKind") else v)
                   for (k, mo, n, v) in types]
        rc, out, _ = evaluate(flipped, rows)
        add("planted #[non_exhaustive] on a closed row is caught",
            rc == 1 and any("CLASS_MISMATCH" in l and "ClosedKind" in l for l in out))

        # 2. Flipping the row without the attribute must be red (the other direction).
        rows2, _m = parse_rows([r.replace("class=closed", "class=open | reason=long enough "
                                                          "reviewable reason for opening a type here")
                                if "ClosedKind" in r and r.startswith("ENUM") else r
                                for r in good])
        rc, out, _ = evaluate(types, rows2)
        add("row flipped to open without the attribute is caught",
            rc == 1 and any("one deliberate act" in l or "deliberate act" in l for l in out))

        # 3. New type with no row.
        rc, out, _ = evaluate(types + [("enum", "m", "Fresh", False)], rows)
        add("public type with no class row is caught",
            rc == 1 and any("[GAP] NEW public type" in l for l in out))

        # 4. Row whose type moved/ vanished.
        rc, out, _ = evaluate([t for t in types if t[2] != "Plain"], rows)
        add("renamed/deleted type keeping its row reports STALE",
            rc == 1 and any("[STALE]" in l for l in out))

        # 5. Both fixed together -> green again. Rows go through the SAME reader as the
        # real run (handing evaluate raw ledger strings is a TypeError, not a verdict).
        paired, _m5 = parse_rows([r for r in good if "lib::Plain" not in r])
        rc, _o, _s = evaluate([t for t in types if t[2] != "Plain"], paired)
        add("paired deletion (row dropped in same commit) -> rc 0", rc == 0)

        # 6. Undecidable states -> exit 2 via the reader, never a silent pass.
        add("open without a reviewable reason is malformed",
            bool(parse_rows(["ENUM | type=a::B | class=open | reason=too short"])[1]))
        add("class value outside open|closed is malformed",
            bool(parse_rows(["ENUM | type=a::B | class=maybe"])[1]))
        add("unknown key is malformed",
            bool(parse_rows(["ENUM | type=a::B | class=closed | note=x"])[1]))
        add("duplicate type row is malformed",
            bool(parse_rows(["ENUM | type=a::B | class=closed",
                             "ENUM | type=a::B | class=closed"])[1]))

        # 7. Parser-blindness floor must sit under the real surface.
        real = extract_types(os.path.join(CRATE_DIR, "src"))
        add("TYPES_FLOOR above zero and below the real surface (%d types)" % len(real),
            0 < TYPES_FLOOR <= len(real))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print_line("[SELFTEST] %s" % ("ALL ASSERTIONS HELD" if ok else "FAILED"))
    return 0 if ok else 1


def main(argv):
    for a in argv[1:]:
        if a not in ("--selftest", "--emit"):
            sys.stderr.write("usage: check_l0_extensibility_class.py [--selftest|--emit]\n")
            return 2
    if "--selftest" in argv[1:]:
        return selftest()
    return run_real()


if __name__ == "__main__":
    import gate_rc  # crash -> exit 2, never a borrowed traceback exit 1

    sys.exit(gate_rc.run(lambda: main(sys.argv)))
