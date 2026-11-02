#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L0-B public API signature leak gate (G-135/G-136, ADR-225 D6).

Why this gate exists
--------------------
ADR-222 D1 put nexus-core / event-bus / nexus-telemetry into L0-B and stated
that L0-B is *not* a pure contract layer: it may carry tokio / tracing /
rayon. ADR-222 appendix 9 deliberately refused a dependency freeze gate there,
and ADR-223 later shipped G-131 to lock the external *dependency rows* -- but
its own note discloses the remaining hole: "L0-B public fn surface is not in
this gate's jurisdiction". R1 register M37 recorded "281 pub fn, 10 tokio
exposures, register only, no code change", i.e. the third-party-backend-type
leak axis was accepted as a known hole with no machine statement at all.

R2 measurement widened the hole: counting only return positions (what M37 did)
misses the single most coupled site in the repo, event-bus `shard.rs`
`spawn_workers(...)`, which takes `tokio::sync::broadcast::Sender<NexusEvent>`
and `&tokio::runtime::Handle` as *parameters*, forcing every caller to own a
runtime and hand over a handle. So this gate must judge return, parameter and
generic-bound positions, not just returns.

What it judges
--------------
Key set `<crate>::<module>::<fn-or-type>::<extern_crate>::<pos>` with pos in
{ret, arg, bound, implfrom}, over the production surface of the three L0-B crates.
Rows are a MULTISET, not a set: two distinct leaks that share a key (e.g. the two
`impl From<rmp_serde::{encode,decode}::Error> for EventBusError`) take two rows, so
a third such leak is still caught as NEW. Unique-row enforcement would defeat that.
  * NEW    - a leak site that appeared but is not in the ledger -> red.
  * STALE  - a ledger row no longer observed (someone fixed a leak and must
             delete the row in the same commit) -> red.
  * malformed row shape, or a pos token outside POSITIONS -> exit 2 (undecidable).
  * fewer than SIGN_FLOOR scanned pub fn signatures -> exit 2 (a blind parser
    reports exactly like a clean one, so refuse rather than pass).
A row naming a crate outside L0-B needs no separate guard: the scanner can never
observe it, so it surfaces as STALE red anyway.
Dependency vocabulary is taken from scripts/l0b_dep_freeze.txt (the measured
G-131 ledger), NOT from any prose whitelist: ADR-225 D6 exists precisely
because the G-125 note's three-value set intersects the measured 27-row
surface in only 16 rows, so a note-derived bound would plant ~11 false reds.
Workspace-internal crates are out of scope here: direction is owned by G-125.

Judged positions (4 classes; ADR-225 D6 + the T-6 owner ruling of 2026-10-26):
  ret      - return type of a `pub fn`
  arg      - parameter type of a `pub fn` (the strongest coupling: the caller
             must own a runtime / hand over a handle)
  bound    - generic parameter list on the fn name, and the `where` clause
  implfrom - `impl From<External> for LocalError`, visible to all downstream
             callers and reachable implicitly through `?`

Modes / exit codes
------------------
  (default)   judge the real repo            0 = agree, 1 = NEW/STALE, 2 = undecidable
  --selftest  fixtures prove both directions through the production judge
                                               0 = all assertions held, 1 = not
  --emit      print the ledger for the live tree (write it deliberately)

Lexer discipline: comments and string/char literals are blanked, and the
cfg(test) tail is masked, by importing the SAME helpers G-99/G-105 use -- an
import, not a copy, so the twins cannot drift.

Output is pure ASCII (project script convention; avoids Windows GBK console traps).
"""

import os
import re
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)

import check_l0_purity as purity  # noqa: E402  (shares lexer + test-tail mask)

L0B = ("nexus-core", "event-bus", "nexus-telemetry")
DEP_LEDGER = os.path.join(HERE, "l0b_dep_freeze.txt")
BASELINE = os.path.join(HERE, "l0b_pub_signature_freeze.txt")

# Below this many scanned `pub fn` signatures the parser is presumed blind, not
# clean. Set from the measured L0-B surface and rounded down hard on purpose.
SIGN_FLOOR = 120

POSITIONS = ("ret", "arg", "bound", "implfrom")

# `impl From<Ext> for Local` (and the generic form `impl<T> From<Ext> for Local`).
# Counted as a public-surface leak (owner ruling 2026-10-26, ADR-225 T-6): a trait
# impl is visible to every downstream crate, and the `?` operator calls it
# implicitly, so an external error type reaching a public boundary through
# `From` is the same exposure class as one in a return position -- except the
# caller often never writes the type name, which is exactly why a
# signature-only scan would miss it.
IMPL_FROM_RE = re.compile(
    r"^\s*impl\s*(?:<[^>]*>\s*)?From\s*<\s*([A-Za-z_]\w*)", re.M)
IMPL_FOR_TARGET_RE = re.compile(r"\bfor\s+([A-Za-z_]\w*)")

# `pub fn`, `pub async fn`, `pub const fn`, `pub unsafe fn`; optional leading
# generic list on the name (`pub fn foo<T>(`).
PUB_FN_RE = re.compile(
    r"^\s*pub\s+(?:const\s+|async\s+|unsafe\s+)*fn\s+(?:<[^>]*>\s*)?([A-Za-z_]\w*)"
)
# A ledger row is one token: crate::module::fn::dep::pos (module may contain '/').
BASELINE_LINE_RE = re.compile(r"^[^\s:]+(?:::[^\s:]+){4}$")
# Any identifier inside a signature region. Both spellings of a leak must count:
# the qualified `tokio::runtime::Handle` (path head) AND the bare `JoinHandle<()>
# ` brought in by `use tokio::task::JoinHandle;`. An earlier cut matched only
# `ident ::` heads and therefore reported a clean surface for subscriber.rs:166
# /:267 and attribution.rs:282 -- a gate that under-sees blesses the very hole
# it was built to close.
IDENT_RE = re.compile(r"\b[A-Za-z_]\w*\b")
# Top-level `use` statements, used to resolve imported short prefixes such as
# `use tokio::sync::mpsc;` making a bare `mpsc::Sender<..>` a tokio exposure.
USE_RE = re.compile(r"^\s*pub(?:\([^)]*\))?\s+use\s+|^\s*use\s+")


class LedgerError(ValueError):
    pass


def rust_ident(crate_name):
    """Cargo crate name -> Rust path segment (`rmp-serde` -> `rmp_serde`)."""
    return crate_name.replace("-", "_")


def module_key(path, src_dir):
    """Crate-relative module path (`shard`, `compute/bridge`)."""
    rel = os.path.relpath(path, src_dir).replace(os.sep, "/")
    if rel.endswith(".rs"):
        rel = rel[:-3]
    if rel.endswith("/mod"):
        rel = rel[: -len("/mod")]
    return rel


def dep_vocabulary():
    """{crate: set(rust idents)} from the G-131 measured ledger."""
    if not os.path.isfile(DEP_LEDGER):
        raise LedgerError("missing dependency ledger: %s" % DEP_LEDGER)
    vocab = {}
    with open(DEP_LEDGER, "r", encoding="utf-8") as fh:
        for raw in fh:
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            if "::" not in line:
                raise LedgerError("malformed dep ledger row: %s" % line)
            crate, dep = line.split("::", 1)
            vocab.setdefault(crate, set()).add(rust_ident(dep))
    return vocab


def _use_bindings(line):
    """Root-crate idents made visible by one `use` line.

    `use tokio::sync::mpsc;`              -> {mpsc: tokio}
    `use tokio::{sync::mpsc, task::JoinHandle};`
                                          -> {mpsc: tokio, JoinHandle: tokio}
    `use tokio::sync::{self as s};`       -> {s: tokio}
    `use crate::x::Y;` / `use std::..`    -> {} (not external)

    The binding name is the LAST path segment of each item (or its `as` alias).
    R2 note: an earlier cut of this helper required a bare identifier inside the
    brace group, which silently dropped `task::JoinHandle` and therefore
    under-reported three real return-position leaks (subscriber.rs:166/:267 and
    attribution.rs:282). A gate that under-sees blesses the hole it was built
    to close, so the nested-brace form is a fixture in --selftest.
    """
    m = re.match(
        r"^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+([A-Za-z_]\w*)\s*::\s*(.+?)\s*;?\s*$", line)
    if not m:
        return {}
    root, rest = m.group(1), m.group(2)
    if root in ("crate", "self", "super", "std", "core", "alloc"):
        return {}
    out = {}
    brace = re.search(r"\{([^}]*)\}", rest)
    items = [brace.group(1)] if brace else [rest]
    for chunk in items:
        for item in chunk.split(","):
            item = item.strip()
            if not item or item == "self":
                continue
            if " as " in item:
                alias = item.split(" as ", 1)[1].strip()
            else:
                alias = item.split("::")[-1].strip()
            if alias and alias != "self" and re.match(r"^[A-Za-z_]\w*$", alias):
                out[alias] = root
    return out


def _signature_span(lines, start):
    """Join the signature text of a `pub fn` starting at `start`.

    Stops at the body-opening `{` at paren depth 0, or at `;` for a trait
    method without a default body. Bounded so a malformed input cannot make
    this loop swallow the file.
    """
    depth = 0
    seen_paren = False
    chunks = []
    for i in range(start, min(start + 16, len(lines))):
        line = lines[i]
        chunks.append(line)
        for ch in line:
            if ch == "(":
                depth += 1
                seen_paren = True
            elif ch == ")":
                depth -= 1
            elif ch == "{" and depth == 0 and seen_paren:
                return " ".join(c.strip() for c in chunks), i
            elif ch == ";" and depth == 0 and seen_paren:
                return " ".join(c.strip() for c in chunks), i
    return " ".join(c.strip() for c in chunks), min(start + 15, len(lines) - 1)


def _split_regions(sig):
    """Return (params, ret, bound) text for one signature line-join.

    A generic parameter list on the fn name (`pub fn f<T: tokio::io::AsyncRead>`)
    is part of the public surface too: it constrains what a caller may pass and
    therefore leaks the crate named in it. It is folded into the `bound` region.
    """
    open_paren = sig.find("(")
    if open_paren < 0:
        return "", "", ""
    head = sig[:open_paren]          # `pub fn f<...>` -- generic list lives here
    depth = 0
    close = -1
    for i in range(open_paren, len(sig)):
        if sig[i] == "(":
            depth += 1
        elif sig[i] == ")":
            depth -= 1
            if depth == 0:
                close = i
                break
    if close < 0:
        params = sig[open_paren + 1:]
        tail = ""
    else:
        params = sig[open_paren + 1:close]
        tail = sig[close + 1:]
    # Strip a trailing body `{ ...` marker if the span ended on the brace.
    brace = tail.find("{")
    if brace >= 0:
        tail = tail[:brace]
    where = re.search(r"\bwhere\b", tail)
    bound = head[head.find("<"):] if "<" in head else ""
    if where:
        bound = (bound + " " + tail[where.start():]).strip()
        tail = tail[:where.start()]
    else:
        bound = bound.strip()
    arrow = re.search(r"->", tail)
    ret = tail[arrow.end():] if arrow else ""
    return params, ret, bound


def scan_signatures(src_dir, crate, vocab):
    """[(module, fn, dep, pos)] over the production surface of one crate."""
    found = []
    allowed = vocab.get(crate, set())
    for dirpath, _dirnames, filenames in os.walk(src_dir):
        for fn in sorted(filenames):
            if not fn.endswith(".rs"):
                continue
            path = os.path.join(dirpath, fn)
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                src = fh.read()
            code = purity.blank_rust(src)
            lines = code.splitlines()
            mask = purity.test_tail_mask(lines)
            mod = module_key(path, src_dir)
            # Prefixes bound by file-level `use` lines, mapped to their root crate.
            use_map = {}
            for line in lines:
                if USE_RE.match(line):
                    use_map.update(_use_bindings(line))
            for i, line in enumerate(lines):
                if mask[i]:
                    continue
                m = PUB_FN_RE.match(line)
                if not m:
                    continue
                name = m.group(1)
                sig, _end = _signature_span(lines, i)
                params, ret, bound = _split_regions(sig)
                for pos, region in (("arg", params), ("ret", ret), ("bound", bound)):
                    hits = set()
                    for tok in IDENT_RE.findall(region):
                        # Qualified head (`tokio::..`) or an imported bare name
                        # (`JoinHandle` bound by `use tokio::task::JoinHandle;`).
                        if tok in allowed:
                            hits.add(tok)
                        else:
                            root = use_map.get(tok)
                            if root and root in allowed:
                                hits.add(root)
                    for dep in sorted(hits):
                        found.append((mod, name, dep, pos))
            # `impl From<External> for LocalError`: attributed to the local type
            # name (there is no fn name to key on), position token `implfrom`.
            for i, line in enumerate(lines):
                if mask[i]:
                    continue
                m = IMPL_FROM_RE.match(line)
                if not m:
                    continue
                head = m.group(1)
                dep = head if head in allowed else use_map.get(head)
                if not dep or dep not in allowed:
                    continue
                tgt = IMPL_FOR_TARGET_RE.search(line)
                site = tgt.group(1) if tgt else "?"
                found.append((mod, site, dep, "implfrom"))
    return found


def scan_all(root_dir, vocab, crates=L0B):
    """Aggregate leak sites for every given crate, plus how many pub fn were seen."""
    sites = []
    sig_count = 0
    seen_crates = set()
    for crate in crates:
        src_dir = os.path.join(root_dir, "crates", crate, "src")
        if not os.path.isdir(src_dir):
            raise LedgerError("missing L0-B crate src dir: %s" % src_dir)
        seen_crates.add(crate)
        for dirpath, _dn, filenames in os.walk(src_dir):
            for fn in sorted(filenames):
                if not fn.endswith(".rs"):
                    continue
                path = os.path.join(dirpath, fn)
                with open(path, "r", encoding="utf-8", errors="replace") as fh:
                    src = fh.read()
                code = purity.blank_rust(src)
                lines = code.splitlines()
                mask = purity.test_tail_mask(lines)
                for i, line in enumerate(lines):
                    if not mask[i] and PUB_FN_RE.match(line):
                        sig_count += 1
        for s in scan_signatures(src_dir, crate, vocab):
            sites.append((crate,) + s)
    return sites, sig_count, seen_crates


def key_of(site):
    crate, mod, fn, dep, pos = site
    return "%s::%s::%s::%s::%s" % (crate, mod, fn, dep, pos)


def read_baseline(path):
    """Return (Counter of keys, malformed rows). Comment/blank lines are not rows."""
    rows = []
    malformed = []
    with open(path, "r", encoding="utf-8") as fh:
        for no, raw in enumerate(fh, 1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            if not BASELINE_LINE_RE.match(line):
                malformed.append((no, line))
                continue
            pos = line.split("::")[-1]
            if pos not in POSITIONS:
                malformed.append((no, line))
                continue
            rows.append(line)
    return rows, malformed


def evaluate(measured, listed):
    """Return (new, stale) as sorted key lists over multisets."""
    from collections import Counter
    mc = Counter(measured)
    lc = Counter(listed)
    new = sorted(k for k in mc if mc[k] > lc.get(k, 0))
    stale = sorted(k for k in lc if lc[k] > mc.get(k, 0))
    return new, stale


def run_real():
    vocab = dep_vocabulary()
    sites, sig_count, _seen = scan_all(ROOT, vocab)
    measured = [key_of(s) for s in sites]
    if sig_count < SIGN_FLOOR:
        sys.stdout.write(
            "[UNDECIDABLE] only %d pub fn signatures scanned (< %d floor): "
            "parser presumed blind, not clean\n" % (sig_count, SIGN_FLOOR))
        return 2
    if not os.path.isfile(BASELINE):
        sys.stdout.write("[UNDECIDABLE] missing ledger %s; run --emit\n" % BASELINE)
        return 2
    listed, malformed = read_baseline(BASELINE)
    if malformed:
        for no, line in malformed[:10]:
            sys.stdout.write("[MALFORMED] %s:%d: %s\n" % (
                os.path.basename(BASELINE), no, line))
        return 2
    new, stale = evaluate(measured, listed)
    for k in new:
        sys.stdout.write("[NEW] leaked signature site not in ledger: %s\n" % k)
    for k in stale:
        sys.stdout.write("[STALE] ledger row no longer observed: %s\n" % k)
    if new or stale:
        sys.stdout.write(
            "\n[FAIL] %d NEW / %d STALE against %d observed sites, %d rows listed\n"
            % (len(new), len(stale), len(measured), len(listed)))
        sys.stdout.write(
            "FIX: a NEW row means a third-party backend type entered a public L0-B "
            "signature; encapsulate it (newtype/DTO) or, if the exposure is decided, "
            "add the row with a reason comment AND record it in an ADR. "
            "A STALE row means a leak was fixed -- delete the row in the same commit "
            "(an exemption must not outlive the code it excuses).\n")
        return 1
    sys.stdout.write(
        "[OK] L0-B public signature leak surface matches ledger: "
        "sites=%d rows=%d scanned_pub_fn=%d\n" % (
            len(measured), len(listed), sig_count))
    return 0


def run_emit():
    vocab = dep_vocabulary()
    sites, sig_count, _seen = scan_all(ROOT, vocab)
    sys.stdout.write(
        "# L0-B public API signature leak ledger (G-135, ADR-225 D6). Only-"
        "decrease key set of <crate>::<module>::<fn>::<extern_crate>::<pos> "
        "with pos in ret|arg|bound, over the production surface of nexus-core / "
        "event-bus / nexus-telemetry. Comments and strings are blanked and the "
        "cfg(test) tail is masked with the SAME lexer G-99/G-105 use (imported, "
        "not copied). The extern-crate vocabulary is taken from "
        "l0b_dep_freeze.txt, never from prose. Generated by --emit "
        "(scanned_pub_fn=%d).\n" % sig_count)
    for k in sorted(key_of(s) for s in sites):
        sys.stdout.write(k + "\n")
    return 0


# --------------------------------------------------------------------------
# selftest fixtures (tempdir, judged by the production path)
# --------------------------------------------------------------------------

_FIXTURE_CRATE = "event-bus"


def _write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


def _fixture_src():
    # doc comment and string mention tokio -> must NOT count; pub(crate) fn ->
    # must NOT count; cfg(test) tail -> must NOT count; a private fn -> must NOT
    # count. One arg leak, one ret leak, one bound leak, and one ret leak that
    # is only visible through a NESTED-BRACE use binding (regression fixture).
    return '''//! crate doc
use tokio::{sync::mpsc, task::JoinHandle};

pub struct Holder;

impl Holder {
    /// Leaks `tokio` in prose only: tokio::runtime::Handle is not code.
    pub fn documented() -> u32 {
        let s = "tokio::sync::broadcast::Sender";
        let _ = s;
        1
    }
    pub(crate) fn internal(_x: tokio::sync::broadcast::Sender<u8>) {}
    fn private(_h: tokio::runtime::Handle) {}
    pub fn leaks_arg(_s: mpsc::Sender<u8>) -> u32 { 1 }
    pub fn leaks_ret() -> tokio::runtime::Handle { unreachable!() }
    pub fn leaks_bound<T: tokio::io::AsyncRead>(_t: T) {}
    pub fn leaks_via_brace_use() -> JoinHandle<()> { unreachable!() }
    pub fn clean(_x: u32) -> Vec<u32> { Vec::new() }
}

// `impl From<External> for Local` must count too (owner ruling, ADR-225 T-6):
// the trait impl is downstream-visible and `?` reaches it without the caller
// ever naming the type, which a signature-only scan would miss.
impl From<tokio::task::JoinError> for Holder {
    fn from(_e: tokio::task::JoinError) -> Self { Holder }
}
// A non-external From impl must NOT count (crate-internal error chaining).
impl From<InnerErr> for Holder {
    fn from(_e: InnerErr) -> Self { Holder }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn test_only(_h: tokio::runtime::Handle) {}
}
'''


def run_selftest():
    vocab = {_FIXTURE_CRATE: {"tokio", "serde"}}
    rc = 0
    tmp = tempfile.mkdtemp(prefix="g135_")
    try:
        src_dir = os.path.join(tmp, "crates", _FIXTURE_CRATE, "src")
        _write(os.path.join(src_dir, "lib.rs"), _fixture_src())
        sites, sig_count, _seen = scan_all(tmp, vocab, crates=(_FIXTURE_CRATE,))
        keys = sorted(key_of(s) for s in sites)
        expected = sorted([
            "%s::lib::leaks_arg::tokio::arg" % _FIXTURE_CRATE,
            "%s::lib::leaks_ret::tokio::ret" % _FIXTURE_CRATE,
            "%s::lib::leaks_bound::tokio::bound" % _FIXTURE_CRATE,
            "%s::lib::leaks_via_brace_use::tokio::ret" % _FIXTURE_CRATE,
            "%s::lib::Holder::tokio::implfrom" % _FIXTURE_CRATE,
        ])
        checks = [
            ("CLEAN fixture matches exactly the five planted sites",
             keys == expected),
            ("doc-comment/string mention not counted",
             not any("documented" in k for k in keys)),
            ("pub(crate) not counted",
             not any("internal" in k for k in keys)),
            ("private fn not counted",
             not any("::private::" in k for k in keys)),
            ("cfg(test) tail not counted",
             not any("test_only" in k for k in keys)),
            ("use-alias prefix resolved to its root crate",
             "%s::lib::leaks_arg::tokio::arg" % _FIXTURE_CRATE in keys),
            ("nested-brace use binding resolved (JoinHandle -> tokio)",
             "%s::lib::leaks_via_brace_use::tokio::ret" % _FIXTURE_CRATE in keys),
            ("impl From<External> counted as implfrom",
             "%s::lib::Holder::tokio::implfrom" % _FIXTURE_CRATE in keys),
            ("impl From<local type> NOT counted (no spurious row)",
             sum(1 for k in keys if k.endswith("::implfrom")) == 1),
            ("production judge: clean ledger -> 0",
             evaluate(keys, expected) == ([], [])),
            ("production judge: planted NEW -> red",
             evaluate(keys + [keys[0]], expected)[0] == [keys[0]]),
            ("production judge: un-deleted row after a fix -> STALE red",
             evaluate([], expected)[1] == expected),
            ("production judge: leak and row removed together -> green",
             evaluate([], []) == ([], [])),
            ("scanner saw the pub fn surface (not blind)",
             sig_count >= 5),
        ]
        # malformed ledger must be refused, not silently skipped
        bad = os.path.join(tmp, "bad.txt")
        _write(bad, "%s::lib::f::tokio::boguspos\nnonsense\n" % _FIXTURE_CRATE)
        _rows, malformed = read_baseline(bad)
        checks.append(("malformed row / unknown pos -> reported", len(malformed) == 2))
        # a ledger comment line is not a row
        ok_ledger = os.path.join(tmp, "ok.txt")
        _write(ok_ledger, "# comment\n\n%s\n" % keys[0])
        rows, mal = read_baseline(ok_ledger)
        checks.append(("comment/blank lines are not rows", rows == [keys[0]] and not mal))

        for label, held in checks:
            sys.stdout.write("  [%s] %s\n" % ("PASS" if held else "FAIL", label))
            if not held:
                rc = 1
                sys.stdout.write("       observed: %r\n" % (keys,))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    sys.stdout.write("SELFTEST: %s (%d assertions)\n" % (
        "PASSED" if rc == 0 else "FAILED", len(checks)))
    return rc


def main(argv):
    """ argv excludes the program name (same convention as the sibling gates). """
    if argv == ["--selftest"]:
        return run_selftest()
    if argv == ["--emit"]:
        return run_emit()
    if argv == ["-h"] or argv == ["--help"]:
        sys.stdout.write(__doc__)
        return 0
    if argv:
        sys.stdout.write("usage: scripts/check_pub_api_leak.py [--selftest|--emit]\n")
        return 2
    return run_real()


if __name__ == "__main__":
    # G-44 / F37: a gate executed from a list must map an unexpected crash to
    # exit 2 (undecidable) instead of a traceback -- otherwise "the parser broke"
    # is read by CI as the same thing as "the repo is clean". Reuse the shared
    # runner rather than hand-rolling a try/except, so all gates fail alike.
    import gate_rc
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
