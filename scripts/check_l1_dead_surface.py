#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L1/L0-B zero-assembly public surface registry gate (L1-B-4).

Why this gate exists
--------------------
ADR-223 D1 made `event-bus` / `nexus-core` / `nexus-compute` / `nexus-telemetry`
the L0-B core-foundation face.  Three public surfaces of `event-bus` turned out to
carry ZERO production assembly: `telemetry` (Telemetry / TurnSpan /
LatencyHistogram / SpanStatus), the `shadow_diff` recorder family and
`formal::CausalConsistencyChecker`.  They survived for a long time because "a
public item exists" is not a claim anybody can falsify -- nothing in the build
ever says "this thing is wired".

L1-B-1..L1-B-3 deleted them.  This gate is why the NEXT one cannot hide: it
re-derives the zero-assembly surface of the four L0-B crates from the code and
reconciles it, in BOTH directions, with `scripts/l1_dead_surface_freeze.txt`.
  * NEW    - a zero-assembly surface that is not registered -> red.
  * STALE  - a registered surface that no longer exists -> red (drop the row).
  * equal  -> green.

Judging rule
------------
A public item is a ZERO-ASSEMBLY surface when nothing in production code keeps it
alive.  "Alive" is decided by a reference graph, not by a grep of one file:

  * only `crates/*/src/**` counts as production code -- `tests/`, `benches/` and
    `examples/` are excluded by construction (they live outside `src/`);
  * comments and string literals are blanked with the SAME lexer G-99/G-105 use
    (`check_l0_purity.blank_rust`) and `#[cfg(test)]` tails are masked with
    `check_l0_purity.test_tail_mask` -- imported, never copied, so the twins
    cannot drift;
  * a `pub use` line is a RE-DECLARATION of a public path, never a consumption
    -> re-export lines never make an item live (otherwise every retired type
    would look alive through the crate-root re-export block);
  * an item is LIVE when a live anchor references it.  A live anchor is any
    production block that is not itself one of the scanned public items (another
    crate's code, a private fn, a `use` statement).  Liveness then propagates
    along the intra-crate reference edges, so a type that is only produced by an
    already-dead constructor is itself dead.

Granularity note: the unit is the public ITEM, not the module -- `event-bus`
already learned that "the module is used" != "every type in it is constructed"
(its crate doc says so explicitly, and the L1-B wave acted on it).

Modes / exit codes
------------------
  (default)   judge the real repo    0 = agree, 1 = NEW/STALE, 2 = undecidable
  --selftest  fixtures prove both directions through the production judge
  --emit      print the measured ledger for the live tree (write it deliberately)

Output is pure ASCII (project script convention).
"""

import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)

import check_l0_purity as purity  # noqa: E402  (shares lexer + test-tail mask)

BASELINE = os.path.join(HERE, "l1_dead_surface_freeze.txt")

# L0-B core-foundation crates (ADR-222 D1 + ADR-309 W4-1: nexus-compute extracted
# from nexus-core). Same roster as G-131 / G-135 -- one fact, one roster.
TARGET_CRATES = ("event-bus", "nexus-core", "nexus-compute", "nexus-telemetry")

# Below this many scanned public items the scanner is presumed blind, not clean.
# A blind parser reports exactly like a clean one, so refuse rather than pass.
ITEM_FLOOR = 40

# A ledger row is one token: <crate>::<module>::<name>.
ROW_RE = re.compile(r"^[^\s:]+::[^\s:]+::[^\s:]+$")

IDENT_RE = re.compile(r"[A-Za-z_]\w*")

# A public item declaration at column 0. `pub(crate)` is excluded on purpose:
# it is not part of the public surface this gate judges.
PUB_ITEM_RE = re.compile(
    r"^pub\s+(?:async\s+|const\s+|unsafe\s+|extern\s+\"[^\"]*\"\s+)*"
    r"(fn|struct|enum|trait|union|type|const|static|mod)\s+([A-Za-z_]\w*)")

# Block boundary: a top-level item (column 0). rustfmt indentation is what makes
# "column 0" a reliable marker; the repo enforces `cargo fmt` on every crate.
ITEM_BLOCK_RE = re.compile(
    r"^(?:pub\s*(?:\([^)]*\)\s*)?)?"
    r"(?:(?:async|const|unsafe|extern\s+\"[^\"]*\")\s+)*"
    r"(?:fn|struct|enum|trait|union|type|const|static|mod)\s+([A-Za-z_]\w*)")
IMPL_BLOCK_RE = re.compile(r"^impl\b")
USE_BLOCK_RE = re.compile(r"^(?:pub\s+)?use\b")
PUB_USE_RE = re.compile(r"^pub\s+use\b")


class LedgerError(ValueError):
    pass


def module_key(path, src_dir):
    """Crate-relative module path (`bus`, `compute/bridge`; `lib` for lib.rs)."""
    rel = os.path.relpath(path, src_dir).replace(os.sep, "/")
    if rel.endswith("/mod.rs"):
        rel = rel[: -len("/mod.rs")]
    elif rel.endswith(".rs"):
        rel = rel[:-3]
    return rel


def _impl_owner(line):
    """Owner name of an `impl` block: the target type, not the trait."""
    head = line[len("impl"):]
    head = re.sub(r"^\s*<[^>]*>", "", head)
    if " for " in head:
        tail = head.rsplit(" for ", 1)[1]
        m = re.match(r"\s*([A-Za-z_]\w*)", tail)
        return m.group(1) if m else None
    m = re.match(r"\s*([A-Za-z_]\w*)", head)
    return m.group(1) if m else None


def _block_owner(line):
    """Owner of a top-level block, or None for a `use` block."""
    if IMPL_BLOCK_RE.match(line):
        return _impl_owner(line)
    if USE_BLOCK_RE.match(line):
        return None
    m = ITEM_BLOCK_RE.match(line)
    if m:
        return m.group(1)
    return None


def _files_under(src_dir):
    """Raw walk: every `.rs` under `src_dir`, test modules included."""
    out = []
    for dirpath, _dirs, names in os.walk(src_dir):
        for name in sorted(names):
            if name.endswith(".rs"):
                out.append(os.path.join(dirpath, name))
    return sorted(out)


_EXTERN_MOD_RE = re.compile(r"^\s*(?:pub\s+)?mod\s+([A-Za-z_]\w*)\s*;\s*$")


def _test_module_files(src_dir):
    """Files reached through `#[cfg(test)] mod X;` (EXTERNAL test modules).

    WHY: `purity.test_tail_mask` only masks the INLINE tail form
    (`#[cfg(test)] mod tests { ... }`).  An external test module
    (`#[cfg(test)] mod tests;`) points at a sibling FILE or DIRECTORY that lives
    INSIDE `src/`, so a plain walk scans it as production code -- and then every
    helper that only tests construct looks assembled.  Excluding exactly the
    modules declared `#[cfg(test)]` keeps the judgement honest without
    special-casing a filename (`tests.rs` may be a production name elsewhere).
    """
    out = []
    for path in _files_under(src_dir):
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                lines = fh.read().splitlines()
        except OSError:
            continue
        armed = False
        for line in lines:
            stripped = line.strip()
            if not armed:
                if stripped == "#[cfg(test)]":
                    armed = True
                continue
            if not stripped or stripped.startswith("//"):
                continue
            if stripped.startswith("#["):  # further attributes stay on the item
                continue
            m = _EXTERN_MOD_RE.match(stripped)
            if m:
                base = os.path.join(os.path.dirname(path), m.group(1))
                if os.path.isdir(base):
                    out.extend(_files_under(base))
                elif os.path.isfile(base + ".rs"):
                    out.append(base + ".rs")
            armed = False
    return set(os.path.normcase(os.path.abspath(p)) for p in out)


def _prod_files_under(src_dir):
    """Production-side files = raw walk minus `#[cfg(test)] mod X;` targets."""
    skip = _test_module_files(src_dir)
    return [p for p in _files_under(src_dir)
            if os.path.normcase(os.path.abspath(p)) not in skip]


def _blocks_of(lines, mask):
    """[(owner_or_None, [tokens...])] per top-level block of one blanked file."""
    blocks = []
    current = None
    for i, line in enumerate(lines):
        if mask[i]:
            continue
        if line and not line[0].isspace() and (
            ITEM_BLOCK_RE.match(line) or IMPL_BLOCK_RE.match(line)
            or USE_BLOCK_RE.match(line)):
            current = [_block_owner(line), []]
            blocks.append(current)
            if PUB_USE_RE.match(line):
                # A re-export declares a public path; it consumes nothing.
                current[1].append(())
                continue
        if current is None:
            continue
        if PUB_USE_RE.match(line):
            current[1].append(())
            continue
        current[1].append(tuple(IDENT_RE.findall(line)))
    return blocks


def scan_crate(root, crate, want_items=True, name_filter=None):
    """(items, blocks) for one crate.

    items: {key: (crate, module, name, kind)} with key = <crate>::<module>::<name>
    blocks: [(owner, refs)] where refs is a set of referenced identifiers.

    `name_filter` is a cheap pre-filter for the crates that are NOT judged: a
    file that does not even mention a candidate name cannot keep any candidate
    alive, so it is skipped before the (character-by-character) lexer runs.
    """
    src_dir = os.path.join(root, "crates", crate, "src")
    if not os.path.isdir(src_dir):
        if want_items:
            raise LedgerError("missing crate src dir: %s" % src_dir)
        return {}, []
    items = {}
    blocks = []
    for path in _prod_files_under(src_dir):
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            src = fh.read()
        if name_filter is not None and not name_filter.search(src):
            continue
        mod = module_key(path, src_dir)
        code = purity.blank_rust(src)
        lines = code.splitlines()
        mask = purity.test_tail_mask(lines)
        if want_items:
            for i, line in enumerate(lines):
                if mask[i]:
                    continue
                m = PUB_ITEM_RE.match(line)
                if not m:
                    continue
                kind, name = m.group(1), m.group(2)
                key = "%s::%s::%s" % (crate, mod, name)
                items[key] = (crate, mod, name, kind)
        for owner, ref_lists in _blocks_of(lines, mask):
            refs = set()
            for toks in ref_lists:
                refs.update(toks)
            blocks.append((crate, mod, owner, refs))
    return items, blocks


def _resolve(name, ref_crate, by_name, crates=TARGET_CRATES):
    """Keys that a reference to `name` could mean.

    Inside a target crate its own item wins; from outside, every candidate is
    marked live (the conservative direction: a wrong "alive" only under-reports).
    """
    cands = by_name.get(name, ())
    if ref_crate in crates:
        own = tuple(k for k in cands if k.split("::")[0] == ref_crate)
        if own:
            return own
    return cands


def measure(root, crates=TARGET_CRATES):
    """(dead_keys, stats) over the live tree."""
    items = {}
    blocks = []
    crates_dir = os.path.join(root, "crates")
    roster = sorted(os.listdir(crates_dir)) if os.path.isdir(crates_dir) else []

    # Phase 1: the judged crates -- their public items define the candidate set.
    for crate in crates:
        crate_items, crate_blocks = scan_crate(root, crate)
        items.update(crate_items)
        blocks.extend(crate_blocks)

    # Phase 2: every other crate is scanned for REFERENCES only. A file that
    # does not mention a candidate name cannot keep any candidate alive, so it
    # is skipped before the lexer runs (this is the difference between a
    # 13-second gate and a 1-second one).
    names = sorted({name for (_c, _m, name, _k) in items.values()},
                   key=lambda n: (-len(n), n))
    prefilter = re.compile(r"\b(?:%s)\b" % "|".join(re.escape(n) for n in names)) \
        if names else None
    for crate in roster:
        if crate in crates:
            continue
        src_dir = os.path.join(crates_dir, crate, "src")
        if not os.path.isdir(src_dir):
            continue
        _items, crate_blocks = scan_crate(root, crate, want_items=False,
                                          name_filter=prefilter)
        blocks.extend(crate_blocks)

    by_name = {}
    for key, (_crate, _mod, name, _kind) in items.items():
        by_name.setdefault(name, []).append(key)

    # A live anchor is a production block that is not itself a scanned public
    # item of its own crate: another crate's code, a private fn, a `use`.
    edges = {}
    live = set()
    for crate, mod, owner, refs in blocks:
        own_key = None
        if owner is not None:
            own_key = "%s::%s::%s" % (crate, mod, owner)
            if own_key not in items:
                own_key = None
        if own_key is None:
            for name in refs:
                for key in _resolve(name, crate, by_name, crates):
                    live.add(key)
        else:
            edges.setdefault(own_key, set()).update(refs)

    # Propagate liveness: a live item keeps alive what it references.
    changed = True
    while changed:
        changed = False
        for owner_key, refs in edges.items():
            if owner_key not in live:
                continue
            crate = owner_key.split("::")[0]
            for name in refs:
                for key in _resolve(name, crate, by_name, crates):
                    if key not in live:
                        live.add(key)
                        changed = True

    # A module is a namespace, not a surface: it is assembled as soon as ANY
    # item inside it is assembled. Without this rule every module whose items
    # are reached through the crate-root re-export (`event_bus::EventBus`) would
    # be reported dead -- a false positive about ADDRESSABILITY, not assembly.
    mod_items = [k for k in items if items[k][3] == "mod"]
    changed = True
    while changed:
        changed = False
        for key in mod_items:
            if key in live:
                continue
            crate, parent, name, _kind = items[key]
            child = name if parent == "lib" else "%s/%s" % (parent, name)
            if any(items[k][0] == crate and items[k][1] == child and k in live
                   for k in items):
                live.add(key)
                changed = True

    dead = sorted(k for k in items if k not in live)
    stats = {"items": len(items), "live": len(live), "dead": len(dead)}
    return dead, stats


def parse_ledger(path):
    """(rows, malformed) from the freeze file; comment/blank lines are not rows."""
    rows = []
    malformed = []
    with open(path, "r", encoding="utf-8") as fh:
        for no, raw in enumerate(fh, 1):
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            if not ROW_RE.match(line):
                malformed.append((no, line))
                continue
            rows.append(line)
    return rows, malformed


def judge(root, ledger_path, crates=TARGET_CRATES, item_floor=ITEM_FLOOR):
    """(rc, report) over the live tree or a fixture tree."""
    try:
        dead, stats = measure(root, crates)
    except LedgerError as exc:
        return 2, ["[UNDECIDABLE] %s" % exc]
    except Exception as exc:  # noqa: BLE001 - any scan failure is undecidable
        return 2, ["[UNDECIDABLE] scan failed: %s" % exc]

    if stats["items"] < item_floor:
        return 2, ["[UNDECIDABLE] blind scan: %d public item(s) < floor %d"
                   % (stats["items"], item_floor)]
    if not os.path.isfile(ledger_path):
        return 2, ["[UNDECIDABLE] missing ledger %s; run --emit" % ledger_path]
    listed, malformed = parse_ledger(ledger_path)
    if malformed:
        for no, line in malformed[:10]:
            return 2, ["[MALFORMED] %s:%d: %s"
                       % (os.path.basename(ledger_path), no, line)]
    dup = sorted({k for k in listed if listed.count(k) > 1})
    if dup:
        return 2, ["[MALFORMED] duplicate row(s): %s" % ", ".join(dup)]

    new = sorted(set(dead) - set(listed))
    stale = sorted(set(listed) - set(dead))
    report = []
    for key in new:
        report.append("[NEW] %s (zero-assembly surface not registered; wire it up "
                      "or register it with a reason)" % key)
    for key in stale:
        report.append("[STALE] %s (registered but no longer zero-assembly; "
                      "delete the row in the same commit)" % key)
    if report:
        return 1, report
    return 0, ["[OK] L1 zero-assembly surface matches ledger: items=%d live=%d "
               "dead=%d rows=%d" % (stats["items"], stats["live"], stats["dead"],
                                    len(listed))]


def emit(root, crates=TARGET_CRATES):
    dead, stats = measure(root, crates)
    sys.stdout.write(
        "# L1/L0-B zero-assembly public surface ledger (L1-B-4). Rows are\n"
        "# <crate>::<module>::<name> for every public item of the four L0-B\n"
        "# crates that no production code keeps alive. Only-decrease key set:\n"
        "# a NEW row means a new unwired public surface appeared; a STALE row\n"
        "# means a surface was wired up or deleted -- drop the row.\n"
        "# Generated by --emit (items=%d live=%d dead=%d).\n"
        % (stats["items"], stats["live"], stats["dead"]))
    for key in dead:
        sys.stdout.write(key + "\n")
    return 0


def _write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="ascii", newline="\n") as fh:
        fh.write(text)


def selftest():
    """Fixtures prove both directions through the production judge."""
    n = 0
    bad = 0

    def check(label, got, want):
        nonlocal n, bad
        n += 1
        if got != want:
            bad += 1
            print("  [FAIL] %s got=%r want=%r" % (label, got, want))
        else:
            print("  [PASS] %s -> %r" % (label, got))

    tmp = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_l1_dead_selftest")
    if os.path.isdir(tmp):
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)
    try:
        # crate `aaa` carries one live and one dead public surface; crate `bbb`
        # is the production consumer that keeps `Live` alive.
        _write(os.path.join(tmp, "crates", "aaa", "src", "lib.rs"),
               "pub mod live;\npub mod dead;\n")
        _write(os.path.join(tmp, "crates", "aaa", "src", "live.rs"),
               "pub struct Live;\n")
        _write(os.path.join(tmp, "crates", "aaa", "src", "dead.rs"),
               "pub struct Dead;\npub struct DeadChild;\n"
               "impl DeadChild { pub fn make() -> DeadChild { DeadChild } }\n")
        _write(os.path.join(tmp, "crates", "bbb", "src", "main.rs"),
               "use aaa::live::Live;\nfn main() { let _ = Live; }\n")

        crates = ("aaa",)
        dead, stats = measure(tmp, crates)
        check("only the unwired surface is reported",
              dead, ["aaa::dead::Dead", "aaa::dead::DeadChild", "aaa::lib::dead"])
        check("the consumed surface is not reported",
              any("live::Live" in k for k in dead), False)
        check("scanner saw every public item", stats["items"], 5)

        ledger = os.path.join(tmp, "freeze.txt")
        _write(ledger, "\n".join(dead) + "\n")
        rc, _lines = judge(tmp, ledger, crates=crates, item_floor=1)
        check("clean fixture is green", rc, 0)

        short = os.path.join(tmp, "short.txt")
        _write(short, "aaa::dead::Dead\n")
        rc, lines = judge(tmp, short, crates=crates, item_floor=1)
        check("unregistered surface is NEW/red", rc, 1)
        check("NEW names the surface",
              any("aaa::dead::DeadChild" in ln for ln in lines), True)

        extra = os.path.join(tmp, "extra.txt")
        _write(extra, "\n".join(dead + ["aaa::live::Live"]) + "\n")
        rc, lines = judge(tmp, extra, crates=crates, item_floor=1)
        check("registered-but-gone surface is STALE/red", rc, 1)
        check("STALE names the surface",
              any("aaa::live::Live" in ln for ln in lines), True)

        dup = os.path.join(tmp, "dup.txt")
        _write(dup, "aaa::dead::Dead\naaa::dead::Dead\n")
        rc, _lines = judge(tmp, dup, crates=crates, item_floor=1)
        check("duplicate row is undecidable", rc, 2)

        malformed = os.path.join(tmp, "malformed.txt")
        _write(malformed, "aaa dead Dead\n")
        rc, _lines = judge(tmp, malformed, crates=crates, item_floor=1)
        check("malformed row is undecidable", rc, 2)

        missing = os.path.join(tmp, "missing.txt")
        rc, _lines = judge(tmp, missing, crates=crates, item_floor=1)
        check("missing ledger is undecidable", rc, 2)

        rc, _lines = judge(tmp, ledger, crates=crates, item_floor=1000)
        check("blind scan is undecidable", rc, 2)

        # A re-export must NOT keep a dead surface alive.
        _write(os.path.join(tmp, "crates", "aaa", "src", "lib.rs"),
               "pub mod live;\npub mod dead;\npub use dead::Dead;\n")
        dead2, _stats = measure(tmp, crates)
        check("pub use re-export does not make a surface live",
              dead2, ["aaa::dead::Dead", "aaa::dead::DeadChild", "aaa::lib::dead"])

        # A `use` in production code DOES keep a surface alive.
        _write(os.path.join(tmp, "crates", "bbb", "src", "main.rs"),
               "use aaa::dead::Dead;\nuse aaa::live::Live;\n"
               "fn main() { let _ = Dead; let _ = Live; }\n")
        dead3, _stats = measure(tmp, crates)
        check("production use makes the surface live",
              dead3, ["aaa::dead::DeadChild"])

        # cfg(test) tails are not production code.
        _write(os.path.join(tmp, "crates", "bbb", "src", "main.rs"),
               "use aaa::live::Live;\nfn main() { let _ = Live; }\n")
        _write(os.path.join(tmp, "crates", "aaa", "src", "dead.rs"),
               "pub struct Dead;\npub struct DeadChild;\n"
               "impl DeadChild { pub fn make() -> DeadChild { DeadChild } }\n"
               "#[cfg(test)]\nmod tests {\n    use super::*;\n"
               "    #[test]\n    fn t() { let _ = Dead; }\n}\n")
        dead4, _stats = measure(tmp, crates)
        check("cfg(test) tail does not make a surface live",
              dead4, ["aaa::dead::Dead", "aaa::dead::DeadChild", "aaa::lib::dead"])
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)

    print("  RESULT: %s (%d assertions)" % ("PASS" if not bad else "FAIL", n))
    return 1 if bad else 0


def main(argv):
    if argv == ["--selftest"]:
        return selftest()
    if argv == ["--emit"]:
        return emit(ROOT)
    if argv:
        print("usage: check_l1_dead_surface.py [--selftest|--emit]")
        return 2
    rc, report = judge(ROOT, BASELINE)
    for line in report:
        print(line)
    if rc == 2:
        print("[UNDECIDABLE] L1 zero-assembly surface gate cannot decide")
    elif rc == 1:
        print("[FAIL] L1 zero-assembly surface ledger drifted "
              "(see [NEW]/[STALE] above)")
    return rc


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
