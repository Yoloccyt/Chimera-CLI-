#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L0-B external dependency surface ratchet (G-131/G-132, ADR-223 D1).

Why this gate exists
--------------------
ADR-222 D1 put `nexus-core` / `event-bus` / `nexus-telemetry` into L0-B and D3
deliberately gave them a *wider* gate than L0-A's purity rule. R9 then shipped
event-bus sharding (`without_sharding`, `request_drain`, +370 lines in bus.rs)
without any machine statement about the L0-B dependency surface; R8-1 had
deferred a freeze gate to avoid gate inflation. This gate locks only the
external dependency rows (roughly ten lines of ledger), not the 281 public
functions of event-bus, so the cost stays proportional to the drift risk that
motivated it.

What it judges
--------------
Only-decrease key set over external dependency edges `<l0b-crate>::<dep>`:
  * NEW    - a dependency present in Cargo.toml but absent from the ledger -> red.
  * STALE  - a ledger row no longer present -> red.
  * malformed / duplicate row, unknown L0-B crate -> exit 2.
  * a missing L0-B crate, or fewer than EDGE_FLOOR external rows -> exit 2.
Dialect: `[dependencies]` + `[build-dependencies]` + target-specific
`dependencies` / `build-dependencies` are counted (they all enter the product).
`[dev-dependencies]` is excluded, matching G-125's disclosed boundary. Workspace
internal edges are NOT counted here: direction is owned by G-125, and counting
them twice would create two judges for one fact.

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
BASELINE = os.path.join(HERE, "l0b_dep_freeze.txt")

L0B = ("nexus-core", "event-bus", "nexus-telemetry")
EDGE_FLOOR = 5


class LedgerError(ValueError):
    pass


def _tomllib():
    try:
        import tomllib  # Python 3.11+
    except ImportError:
        return None
    return tomllib


def _load_manifest(path, tomllib):
    """Parse a Cargo.toml, tolerating a UTF-8 BOM (root manifest has one)."""
    with open(path, "rb") as fh:
        raw = fh.read()
    if raw.startswith(b"\xef\xbb\xbf"):
        raw = raw[3:]
    return tomllib.loads(raw.decode("utf-8"))


def workspace_names(root, tomllib):
    """Package names of every workspace member (for the internal-edge filter)."""
    manifest = os.path.join(root, "Cargo.toml")
    data = _load_manifest(manifest, tomllib)
    members = data.get("workspace", {}).get("members", []) or []
    names = set()
    for member in members:
        path = os.path.join(root, member, "Cargo.toml")
        if not os.path.isfile(path):
            continue
        try:
            member_data = _load_manifest(path, tomllib)
        except Exception:  # noqa: BLE001 - an unreadable member is not our axis
            continue
        name = member_data.get("package", {}).get("name")
        if name:
            names.add(name)
    return names


def collect_edges(crates_root, l0b, internal):
    """({edge: True}, seen_crates) for external dependencies of the L0-B crates."""
    tomllib = _tomllib()
    if tomllib is None:
        raise LedgerError("tomllib missing (Python 3.11+ required)")
    edges = {}
    seen = set()
    for crate in l0b:
        path = os.path.join(crates_root, crate, "Cargo.toml")
        if not os.path.isfile(path):
            continue
        seen.add(crate)
        data = _load_manifest(path, tomllib)
        tables = [data.get("dependencies", {}), data.get("build-dependencies", {})]
        for _cfg, target in (data.get("target", {}) or {}).items():
            if not isinstance(target, dict):
                continue
            tables.append(target.get("dependencies", {}))
            tables.append(target.get("build-dependencies", {}))
        for table in tables:
            for dep in (table or {}):
                if dep in internal:
                    continue
                edges["%s::%s" % (crate, dep)] = True
    return edges, seen


def parse_ledger(path, l0b):
    if not os.path.isfile(path):
        raise LedgerError("ledger missing: %s" % path)
    seen = set()
    with open(path, "r", encoding="utf-8") as fh:
        for line_no, raw in enumerate(fh, 1):
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            parts = line.split("::")
            if len(parts) != 2:
                raise LedgerError("%s:%d malformed row (want '<crate>::<dep>')"
                                  % (path, line_no))
            crate, dep = parts[0].strip(), parts[1].strip()
            if crate not in l0b:
                raise LedgerError("%s:%d unknown L0-B crate %r" % (path, line_no, crate))
            if not dep:
                raise LedgerError("%s:%d empty dependency name" % (path, line_no))
            key = "%s::%s" % (crate, dep)
            if key in seen:
                raise LedgerError("%s:%d duplicate row %s" % (path, line_no, key))
            seen.add(key)
    return seen


def judge(crates_root, ledger_path, internal, l0b=L0B, edge_floor=EDGE_FLOOR):
    """(rc, report) over the live tree or a fixture tree."""
    if len(l0b) != 3:
        return 2, ["[UNDECIDABLE] L0-B roster must be exactly three crates, got %d"
                   % len(l0b)]
    try:
        actual, seen = collect_edges(crates_root, l0b, internal)
        ledger = parse_ledger(ledger_path, l0b)
    except LedgerError as exc:
        return 2, ["[UNDECIDABLE] %s" % exc]
    except Exception as exc:  # noqa: BLE001 - any scan failure is undecidable
        return 2, ["[UNDECIDABLE] scan failed: %s" % exc]

    missing = [crate for crate in l0b if crate not in seen]
    if missing:
        return 2, ["[UNDECIDABLE] L0-B crate(s) not scanned: %s" % ", ".join(missing)]
    if len(actual) < edge_floor:
        return 2, ["[UNDECIDABLE] blind scan: %d external row(s) < floor %d"
                   % (len(actual), edge_floor)]

    new = sorted(set(actual) - ledger)
    stale = sorted(ledger - set(actual))
    report = []
    for key in new:
        report.append("[NEW] %s (declared but not registered; new external dependency "
                      "requires an ADR)" % key)
    for key in stale:
        report.append("[STALE] %s (registered but no longer declared)" % key)
    if report:
        return 1, report

    return 0, ["[OK] L0-B dependency surface: crates=%d external_deps=%d"
               % (len(l0b), len(actual))]


def emit(crates_root, root, l0b=L0B):
    tomllib = _tomllib()
    if tomllib is None:
        print("[UNDECIDABLE] tomllib missing")
        return 2
    try:
        internal = workspace_names(root, tomllib)
        actual, _seen = collect_edges(crates_root, l0b, internal)
    except LedgerError as exc:
        print("[UNDECIDABLE] %s" % exc)
        return 2
    for key in sorted(actual):
        print(key)
    return 0


def _write_manifest(path, name, deps=None, dev=None, build=None, target=None):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    lines = ['[package]', 'name = "%s"' % name, 'version = "0.1.0"', '']
    for dep in deps or []:
        lines.append('[dependencies]')
        break
    for dep in deps or []:
        lines.append('%s = "1"' % dep)
    if deps:
        lines.append('')
    for dep in dev or []:
        lines.append('[dev-dependencies]')
        break
    for dep in dev or []:
        lines.append('%s = "1"' % dep)
    if dev:
        lines.append('')
    for dep in build or []:
        lines.append('[build-dependencies]')
        break
    for dep in build or []:
        lines.append('%s = "1"' % dep)
    if build:
        lines.append('')
    if target:
        lines.append("[target.'cfg(unix)'.dependencies]")
        for dep in target:
            lines.append('%s = "1"' % dep)
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

    l0b = ("core", "bus", "tel")
    internal = {"core", "tel", "other"}
    with tempfile.TemporaryDirectory() as tmp:
        crates = os.path.join(tmp, "crates")
        _write_manifest(os.path.join(crates, "core", "Cargo.toml"), "core",
                        deps=["serde", "ndarray"], dev=["proptest"],
                        build=["cc"], target=["libc"])
        _write_manifest(os.path.join(crates, "bus", "Cargo.toml"), "bus",
                        deps=["tokio", "serde", "core"], dev=["proptest"])
        _write_manifest(os.path.join(crates, "tel", "Cargo.toml"), "tel",
                        deps=["tokio", "tracing"])

        actual, seen = collect_edges(crates, l0b, internal)
        check("dev-dependency is excluded", "core::proptest" in actual, False)
        check("internal workspace edge is excluded", "bus::core" in actual, False)
        check("build-dependency is counted", "core::cc" in actual, True)
        check("target-specific dependency is counted", "core::libc" in actual, True)
        check("all L0-B crates scanned", set(seen), set(l0b))

        clean = os.path.join(tmp, "clean.txt")
        rows = sorted(actual)
        with open(clean, "w", encoding="ascii", newline="\n") as fh:
            fh.write("\n".join(rows) + "\n")

        rc, _ = judge(crates, clean, internal, l0b=l0b, edge_floor=1)
        check("clean fixture is green", rc, 0)

        new_ledger = os.path.join(tmp, "new.txt")
        with open(new_ledger, "w", encoding="ascii", newline="\n") as fh:
            fh.write("\n".join(rows[:-1]) + "\n")
        rc, lines = judge(crates, new_ledger, internal, l0b=l0b, edge_floor=1)
        check("missing row is NEW/red", rc, 1)
        check("NEW names the dependency", any(rows[-1] in ln for ln in lines), True)

        stale_ledger = os.path.join(tmp, "stale.txt")
        with open(stale_ledger, "w", encoding="ascii", newline="\n") as fh:
            fh.write("\n".join(rows) + "\ncore::serde_json\n")
        rc, lines = judge(crates, stale_ledger, internal, l0b=l0b, edge_floor=1)
        check("extra row is STALE/red", rc, 1)
        check("STALE names the dependency", any("core::serde_json" in ln for ln in lines), True)

        dup = os.path.join(tmp, "dup.txt")
        with open(dup, "w", encoding="ascii", newline="\n") as fh:
            fh.write("core::serde\ncore::serde\n")
        rc, _ = judge(crates, dup, internal, l0b=l0b, edge_floor=1)
        check("duplicate row is undecidable", rc, 2)

        malformed = os.path.join(tmp, "malformed.txt")
        with open(malformed, "w", encoding="ascii", newline="\n") as fh:
            fh.write("core = serde\n")
        rc, _ = judge(crates, malformed, internal, l0b=l0b, edge_floor=1)
        check("malformed row is undecidable", rc, 2)

        empty = os.path.join(tmp, "empty")
        os.makedirs(empty)
        rc, _ = judge(empty, clean, internal, l0b=l0b, edge_floor=1)
        check("missing L0-B crate is undecidable", rc, 2)

    print("  RESULT: %s (%d assertions)" % ("PASS" if not bad else "FAIL", n))
    return 1 if bad else 0


def run_real():
    tomllib = _tomllib()
    if tomllib is None:
        print("[UNDECIDABLE] tomllib missing (Python 3.11+ required)")
        return 2
    try:
        internal = workspace_names(ROOT, tomllib)
    except Exception as exc:  # noqa: BLE001
        print("[UNDECIDABLE] workspace member scan failed: %s" % exc)
        return 2
    rc, report = judge(os.path.join(ROOT, "crates"), BASELINE, internal)
    for line in report:
        print(line)
    if rc == 2:
        print("[UNDECIDABLE] L0-B dependency surface gate cannot decide")
    elif rc == 1:
        print("[FAIL] L0-B dependency surface ratchet broken (see [NEW]/[STALE] above)")
    return rc


def main(argv):
    if argv == ["--selftest"]:
        return selftest()
    if argv == ["--emit"]:
        return emit(os.path.join(ROOT, "crates"), ROOT)
    if argv:
        print("usage: check_l0b_dep_surface.py [--selftest|--emit]")
        return 2
    return run_real()


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
