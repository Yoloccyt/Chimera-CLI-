#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L0-A sibling assembly ratchet (G-129/G-130, ADR-223 D3).

Why this gate exists
--------------------
ADR-222 split the contract layer into one umbrella plus six sibling crates.
G-91 counts consumers of `nexus-contracts` *definitions*; it cannot see whether
a sibling crate is still assembled by the consumers it was split out for. After
the split a consumer could silently drop e.g. `nexus-contracts-sparse` while the
umbrella keeps re-exporting it, and every existing gate would stay green. This
is exactly the axis ADR-215 D-29 / ADR-221 D-29 deferred ("per-crate assembly
ratchet") and the trigger ADR-222 D2 created by making siblings first-class.

What it judges
--------------
Only-decrease key set over production assembly edges `<sibling> -> <consumer>`:
  * NEW    - an edge present in Cargo.toml but absent from the ledger -> red.
  * STALE  - a ledger edge no longer present -> red.
  * malformed or duplicate ledger row -> exit 2 (undecidable, never green).
  * roster not exactly the L0-A roster, or fewer than EDGE_FLOOR edges -> exit 2.
Dialect: production `[dependencies]` of every `crates/*/Cargo.toml`. Dev, build
and target tables are NOT assembly (they do not link the crate into the product;
dev-dep exceptions are the same boundary G-125 discloses). The umbrella is
recorded as a consumer (ADR-222 D4 facade), and a facade edge never hides the
external edges: each `<sibling> -> <consumer>` pair is its own key.

Modes / exit codes
------------------
  (default)   judge the real repo   0 = agree, 1 = NEW/STALE, 2 = undecidable
  --selftest  fixtures prove both directions through the production judge
  --emit      print the ledger for the live tree (write it deliberately)

Output is pure ASCII (project script convention).
"""

import os
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
BASELINE = os.path.join(HERE, "l0a_sibling_assembly.txt")

EDGE_FLOOR = 2
# WHY the floor is 2, not the live edge count: the floor only guards a BLIND scan
# (fixture tree / broken walker). Ratcheting the exact count is the ledger's job;
# a floor equal to today's count would turn a deliberate, reviewed shrink into
# "undecidable" instead of the STALE verdict that forces the review.

sys.path.insert(0, HERE)
import l0_roster  # noqa: E402

UMBRELLA = l0_roster.L0A_DIR_NAMES[0]
SIBLINGS = tuple(l0_roster.L0A_DIR_NAMES[1:])


class LedgerError(ValueError):
    pass


def _tomllib():
    try:
        import tomllib  # Python 3.11+
    except ImportError:
        return None
    return tomllib


def _read_manifest(path, tomllib):
    """Parse a Cargo.toml, tolerating a UTF-8 BOM.

    WHY: this repository ships several BOM-prefixed manifests (root Cargo.toml
    among them). `tomllib.load()` raises "Invalid statement (at line 1, column 1)"
    on a BOM, which would turn a normal scan into an undecidable gate for a
    reason that has nothing to do with assembly.
    """
    with open(path, "rb") as fh:
        raw = fh.read()
    if raw.startswith(b"\xef\xbb\xbf"):
        raw = raw[3:]
    return tomllib.loads(raw.decode("utf-8"))


def collect_edges(crates_root, siblings, umbrella):
    """Return {(sibling, consumer)} for production `[dependencies]` edges."""
    tomllib = _tomllib()
    if tomllib is None:
        raise LedgerError("tomllib missing (Python 3.11+ required)")
    edges = {}
    if not os.path.isdir(crates_root):
        raise LedgerError("crates root missing: %s" % crates_root)
    for name in sorted(os.listdir(crates_root)):
        manifest = os.path.join(crates_root, name, "Cargo.toml")
        if not os.path.isfile(manifest):
            continue
        data = _read_manifest(manifest, tomllib)
        deps = data.get("dependencies", {}) or {}
        for dep in deps:
            if dep in siblings and dep != name:
                edges[(dep, name)] = True
    return edges


def parse_ledger(path, siblings):
    if not os.path.isfile(path):
        raise LedgerError("ledger missing: %s" % path)
    seen = set()
    with open(path, "r", encoding="utf-8") as fh:
        for line_no, raw in enumerate(fh, 1):
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            parts = line.split("->")
            if len(parts) != 2:
                raise LedgerError("%s:%d malformed row (want '<sibling> -> <consumer>')"
                                  % (path, line_no))
            sibling, consumer = parts[0].strip(), parts[1].strip()
            if sibling not in siblings:
                raise LedgerError("%s:%d unknown sibling %r" % (path, line_no, sibling))
            if not consumer:
                raise LedgerError("%s:%d empty consumer" % (path, line_no))
            key = (sibling, consumer)
            if key in seen:
                raise LedgerError("%s:%d duplicate edge %s -> %s"
                                  % (path, line_no, sibling, consumer))
            seen.add(key)
    return seen


def roster_error(siblings):
    """None when the roster is exactly umbrella + six siblings, else the reason."""
    if len(siblings) + 1 != 7:
        return "L0-A roster must be umbrella + 6 siblings, got %d" % (len(siblings) + 1)
    return None


def judge(crates_root, ledger_path, siblings, umbrella, edge_floor=EDGE_FLOOR):
    """(rc, report) over the live tree or a fixture tree."""
    try:
        actual = set(collect_edges(crates_root, siblings, umbrella))
        ledger = parse_ledger(ledger_path, siblings)
    except LedgerError as exc:
        return 2, ["[UNDECIDABLE] %s" % exc]
    except Exception as exc:  # noqa: BLE001 - any scan failure is undecidable
        return 2, ["[UNDECIDABLE] scan failed: %s" % exc]

    if len(actual) < edge_floor:
        return 2, ["[UNDECIDABLE] blind scan: %d edge(s) < floor %d"
                   % (len(actual), edge_floor)]

    new = sorted(actual - ledger)
    stale = sorted(ledger - actual)
    report = []
    for sibling, consumer in new:
        report.append("[NEW] %s -> %s (assembled but not registered)" % (sibling, consumer))
    for sibling, consumer in stale:
        report.append("[STALE] %s -> %s (registered but no longer assembled)"
                      % (sibling, consumer))
    if report:
        return 1, report

    facade = sum(1 for (_sibling, consumer) in actual if consumer == umbrella)
    external = len(actual) - facade
    return 0, ["[OK] L0-A sibling assembly: siblings=%d edges=%d external=%d facade=%d"
               % (len(siblings), len(actual), external, facade)]


def emit(crates_root, siblings, umbrella):
    try:
        actual = collect_edges(crates_root, siblings, umbrella)
    except LedgerError as exc:
        print("[UNDECIDABLE] %s" % exc)
        return 2
    for sibling, consumer in sorted(actual):
        print("%s -> %s" % (sibling, consumer))
    return 0


def _write_manifest(path, name, deps):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    lines = ['[package]', 'name = "%s"' % name, 'version = "0.1.0"', '']
    for table, entries in deps:
        lines.append("[%s]" % table)
        for dep in entries:
            lines.append('%s = { path = "../%s" }' % (dep, dep))
        lines.append('')
    with open(path, "w", encoding="ascii", newline="\n") as fh:
        fh.write("\n".join(lines))


def selftest():
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

    siblings = ("a", "b")
    umbrella = "umbrella"
    with tempfile.TemporaryDirectory() as tmp:
        crates = os.path.join(tmp, "crates")
        _write_manifest(os.path.join(crates, "a", "Cargo.toml"), "a", [])
        _write_manifest(os.path.join(crates, "b", "Cargo.toml"), "b", [])
        _write_manifest(os.path.join(crates, "umbrella", "Cargo.toml"), "umbrella",
                        [("dependencies", ["a", "b"])])
        _write_manifest(os.path.join(crates, "c1", "Cargo.toml"), "c1",
                        [("dependencies", ["a"])])
        _write_manifest(os.path.join(crates, "c2", "Cargo.toml"), "c2",
                        [("dependencies", ["a", "b"])])
        # dev-only must NOT count as assembly
        _write_manifest(os.path.join(crates, "c3", "Cargo.toml"), "c3",
                        [("dev-dependencies", ["a"])])

        actual = set(collect_edges(crates, siblings, umbrella))
        check("dev-dependency is not an assembly edge", ("a", "c3") in actual, False)
        check("umbrella facade edge is recorded", ("a", "umbrella") in actual, True)

        clean = os.path.join(tmp, "clean.txt")
        with open(clean, "w", encoding="ascii", newline="\n") as fh:
            fh.write("a -> umbrella\na -> c1\na -> c2\nb -> umbrella\nb -> c2\n")

        rc, _ = judge(crates, clean, siblings, umbrella, edge_floor=1)
        check("clean fixture is green", rc, 0)

        new_ledger = os.path.join(tmp, "new.txt")
        with open(new_ledger, "w", encoding="ascii", newline="\n") as fh:
            fh.write("a -> umbrella\na -> c1\nb -> umbrella\nb -> c2\n")
        rc, lines = judge(crates, new_ledger, siblings, umbrella, edge_floor=1)
        check("missing edge is NEW/red", rc, 1)
        check("NEW names both sides", any("a -> c2" in ln for ln in lines), True)

        stale_ledger = os.path.join(tmp, "stale.txt")
        with open(stale_ledger, "w", encoding="ascii", newline="\n") as fh:
            fh.write("a -> umbrella\na -> c1\na -> c2\nb -> umbrella\nb -> c2\nb -> c1\n")
        rc, lines = judge(crates, stale_ledger, siblings, umbrella, edge_floor=1)
        check("extra ledger edge is STALE/red", rc, 1)
        check("STALE names both sides", any("b -> c1" in ln for ln in lines), True)

        dup = os.path.join(tmp, "dup.txt")
        with open(dup, "w", encoding="ascii", newline="\n") as fh:
            fh.write("a -> c1\na -> c1\n")
        rc, _ = judge(crates, dup, siblings, umbrella, edge_floor=1)
        check("duplicate ledger row is undecidable", rc, 2)

        malformed = os.path.join(tmp, "malformed.txt")
        with open(malformed, "w", encoding="ascii", newline="\n") as fh:
            fh.write("a => c1\n")
        rc, _ = judge(crates, malformed, siblings, umbrella, edge_floor=1)
        check("malformed row is undecidable", rc, 2)

        empty = os.path.join(tmp, "empty")
        os.makedirs(empty)
        rc, _ = judge(empty, clean, siblings, umbrella, edge_floor=1)
        check("blind scan (no crates) is undecidable", rc, 2)

        check("roster not seven L0-A crates is undecidable",
              bool(roster_error(("a",))), True)

    print("  RESULT: %s (%d assertions)" % ("PASS" if not bad else "FAIL", n))
    return 1 if bad else 0


def run_real():
    reason = roster_error(SIBLINGS)
    if reason:
        print("[UNDECIDABLE] %s" % reason)
        return 2
    rc, report = judge(os.path.join(ROOT, "crates"), BASELINE, SIBLINGS, UMBRELLA)
    for line in report:
        print(line)
    if rc == 2:
        print("[UNDECIDABLE] L0-A sibling assembly gate cannot decide")
    elif rc == 1:
        print("[FAIL] L0-A sibling assembly ratchet broken (see [NEW]/[STALE] above)")
    return rc


def main(argv):
    if argv == ["--selftest"]:
        return selftest()
    if argv == ["--emit"]:
        return emit(os.path.join(ROOT, "crates"), SIBLINGS, UMBRELLA)
    if argv:
        print("usage: check_l0a_sibling_assembly.py [--selftest|--emit]")
        return 2
    return run_real()


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
