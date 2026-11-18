#!/usr/bin/env python3
# -*- coding: ascii -*-
"""L1 public-surface ratchet gate (L1-F-3).

Why this gate exists
--------------------
The L1 core-foundation face (`event-bus` / `nexus-core` / `nexus-compute` /
`nexus-telemetry` / `nexus-events`) already has a zero-assembly ledger
(G-161/G-162), but nothing pinned the SIZE of its public surface. A refactor
could add public items, features or error variants and every existing gate
would stay green: "a public item exists" is not a claim the build can falsify.
This gate turns the surface into a number and makes growth visible.

Judge (one ledger row per crate)
--------------------------------
    <crate> pub_items=<N> pub_fns=<N> features=<N> error_variants=<N>

  pub_items      `pub struct|enum|type|trait|union|const|static|fn` declarations
                 (`pub(crate)` is excluded: it is not public surface)
  pub_fns        the `pub fn` subset of pub_items (diagnostic only)
  features       keys under `[features]` in the crate's Cargo.toml (0 if absent)
  error_variants variants of the crate's `pub enum *Error` (and any
                 thiserror-derived `pub enum`), counted in the enum body

Only-decrease ratchet: an INCREASE means the public surface grew (red), a
DECREASE means the ledger must shrink in the same commit (also red, so the
number is never a stale guess). Equal -> green.

Scan face
---------
`crates/<crate>/src/**/*.rs` MINUS the files reached through
`#[cfg(test)] mod X;`. That exclusion is imported from
`check_l1_dead_surface._test_module_files` -- this is the fifth consumer of the
same rule, and a copy would drift.

Modes / exit codes
------------------
  (default) --check   judge the repo    0 = equal, 1 = drift, 2 = undecidable
  --emit               print the measured ledger (write it deliberately)
  --selftest           fixtures exercise the production judge
  --json               JSON instead of text (measurement, or verdict)

Output is pure ASCII (project script convention).
"""

import json
import os
import re
import sys
import tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)

import check_l0_purity as purity  # noqa: E402  (shared Rust lexer + test-tail mask)
import check_l1_dead_surface as dead_surface  # noqa: E402  (_test_module_files SSOT)

BASELINE = os.path.join(HERE, "l1_surface_freeze.txt")

# Same roster as G-131 / G-135 / G-161: one fact, one roster.
TARGET_CRATES = ("event-bus", "nexus-core", "nexus-compute",
                 "nexus-telemetry", "nexus-events")

# Below this many scanned public items the scanner is presumed blind, not clean.
# A blind parser reports exactly like a clean one, so refuse rather than pass.
SURFACE_FLOOR = 100

FIELDS = ("pub_items", "pub_fns", "features", "error_variants")

ROW_RE = re.compile(
    r"^(\S+)\s+pub_items=(\d+)\s+pub_fns=(\d+)\s+features=(\d+)"
    r"\s+error_variants=(\d+)$")

# A public item declaration at column 0. `pub(crate)` is excluded on purpose:
# it is not part of the surface this gate judges. `mod` is a namespace, not a
# surface item, so it is not counted either.
PUB_ITEM_RE = re.compile(
    r"^pub\s+(?:async\s+|const\s+|unsafe\s+|extern\s+\"[^\"]*\"\s+)*"
    r"(fn|struct|enum|trait|union|type|const|static)\s+([A-Za-z_]\w*)")

PUB_ENUM_RE = re.compile(r"^pub\s+enum\s+([A-Za-z_]\w*)")

VARIANT_START_RE = re.compile(r"^[A-Za-z_]\w*\s*(?:[({,]|$|=)")


class SurfaceError(ValueError):
    """Anything that makes the measurement undecidable (never a verdict)."""


def _bracket_delta(line):
    return (line.count("{") - line.count("}") + line.count("(")
            - line.count(")") + line.count("[") - line.count("]"))


def _is_error_enum(name, lines, index, mask):
    """True for `*Error` enums and for thiserror-derived enums with other names."""
    if name.endswith("Error"):
        return True
    j = index - 1
    steps = 0
    while j >= 0 and steps < 8:
        if mask[j]:
            j -= 1
            continue
        stripped = lines[j].strip()
        if not stripped or stripped.startswith("//"):
            j -= 1
        elif stripped.startswith("#["):
            if "derive" in stripped and re.search(r"\bError\b", stripped):
                return True
            j -= 1
        else:
            break
        steps += 1
    return False


def _count_variants(lines, decl_index):
    """Count the variants of the enum whose declaration is at `decl_index`.

    Counted at brace depth 1 (the enum body), one count per identifier that
    starts a variant. Attribute lines, doc lines (already blanked) and
    continuation lines of a multi-line variant are not variants.
    """
    depth = 0
    count = 0
    entered = False
    for line in lines[decl_index:]:
        stripped = line.strip()
        if not entered:
            if "{" in line:
                entered = True
                depth = _bracket_delta(line)
                if depth <= 0:
                    return count
            continue
        if depth == 1 and stripped and not stripped.startswith("#") \
                and VARIANT_START_RE.match(stripped):
            count += 1
        depth += _bracket_delta(line)
        if depth <= 0:
            break
    return count


def _feature_count(cargo_toml):
    try:
        with open(cargo_toml, "rb") as fh:
            doc = tomllib.load(fh)
    except OSError as exc:
        raise SurfaceError("cannot read %s: %s" % (cargo_toml, exc))
    except tomllib.TOMLDecodeError as exc:
        raise SurfaceError("malformed %s: %s" % (cargo_toml, exc))
    features = doc.get("features") or {}
    if not isinstance(features, dict):
        raise SurfaceError("[features] is not a table in %s" % cargo_toml)
    return len(features)


def measure_crate(root, crate):
    """(counts dict, scanned item count) for one crate."""
    crate_dir = os.path.join(root, "crates", crate)
    src_dir = os.path.join(crate_dir, "src")
    if not os.path.isdir(src_dir):
        raise SurfaceError("missing crate src dir: %s" % src_dir)

    skip = dead_surface._test_module_files(src_dir)
    counts = {"pub_items": 0, "pub_fns": 0,
              "features": _feature_count(os.path.join(crate_dir, "Cargo.toml")),
              "error_variants": 0}

    for path in dead_surface._files_under(src_dir):
        if os.path.normcase(os.path.abspath(path)) in skip:
            continue
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                src = fh.read()
        except OSError as exc:
            raise SurfaceError("cannot read %s: %s" % (path, exc))

        lines = purity.blank_rust(src).splitlines()
        mask = purity.test_tail_mask(lines)
        for i, line in enumerate(lines):
            if mask[i]:
                continue
            # An error enum is BOTH a public item and a variant source, so the
            # two counts are taken independently (no `continue` between them).
            enum_match = PUB_ENUM_RE.match(line)
            if enum_match and _is_error_enum(enum_match.group(1), lines, i, mask):
                counts["error_variants"] += _count_variants(lines, i)
            item = PUB_ITEM_RE.match(line)
            if item:
                counts["pub_items"] += 1
                if item.group(1) == "fn":
                    counts["pub_fns"] += 1
    return counts


def measure(root, crates=TARGET_CRATES):
    """({crate: counts}, total scanned public items)."""
    rows = {}
    total = 0
    for crate in crates:
        counts = measure_crate(root, crate)
        rows[crate] = counts
        total += counts["pub_items"]
    return rows, total


def parse_ledger(path):
    """({crate: counts}, malformed, duplicates)."""
    rows = {}
    malformed = []
    duplicates = []
    with open(path, "r", encoding="utf-8") as fh:
        for no, raw in enumerate(fh, 1):
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            match = ROW_RE.match(line)
            if not match:
                malformed.append((no, line))
                continue
            crate = match.group(1)
            if crate in rows:
                duplicates.append(crate)
                continue
            rows[crate] = {
                "pub_items": int(match.group(2)),
                "pub_fns": int(match.group(3)),
                "features": int(match.group(4)),
                "error_variants": int(match.group(5)),
            }
    return rows, malformed, duplicates


def judge(root, ledger_path, crates=TARGET_CRATES, floor=SURFACE_FLOOR):
    """(rc, report lines, measured rows or None)."""
    try:
        measured, total = measure(root, crates)
    except SurfaceError as exc:
        return 2, ["[UNDECIDABLE] %s" % exc], None
    except Exception as exc:  # noqa: BLE001 - any scan failure is undecidable
        return 2, ["[UNDECIDABLE] scan failed: %s" % exc], None

    if total < floor:
        return 2, ["[UNDECIDABLE] blind scan: %d public item(s) < floor %d"
                   % (total, floor)], None
    if not os.path.isfile(ledger_path):
        return 2, ["[UNDECIDABLE] missing ledger %s; run --emit" % ledger_path], None

    listed, malformed, duplicates = parse_ledger(ledger_path)
    if malformed:
        no, line = malformed[0]
        return 2, ["[MALFORMED] %s:%d: %s"
                   % (os.path.basename(ledger_path), no, line)], None
    if duplicates:
        return 2, ["[MALFORMED] duplicate row(s): %s"
                   % ", ".join(sorted(set(duplicates)))], None

    report = []
    unknown = sorted(set(listed) - set(crates))
    for crate in unknown:
        report.append("[STALE] %s has a ledger row but is not in the L1 roster; "
                      "delete the row in the same commit" % crate)
    for crate in crates:
        if crate not in listed:
            report.append("[MISSING] %s has no ledger row; run --emit and commit "
                          "the measured value" % crate)
            continue
        for field in FIELDS:
            old = listed[crate][field]
            new = measured[crate][field]
            if new > old:
                report.append("[UP] %s %s grew: %d -> %d (the L1 public surface "
                              "is a only-decrease ratchet)" % (crate, field, old, new))
            elif new < old:
                report.append("[DOWN] %s %s shrank: %d -> %d (update the ledger "
                              "in the same commit)" % (crate, field, old, new))
    if report:
        return 1, report, measured
    return 0, ["[OK] L1 public surface matches ledger: items=%d rows=%d"
               % (total, len(listed))], measured


def emit(root, crates=TARGET_CRATES):
    measured, total = measure(root, crates)
    out = sys.stdout
    out.write(
        "# L1 public-surface ledger (L1-F-3). One row per L1 core-foundation\n"
        "# crate; every count is an only-decrease ratchet.\n"
        "#\n"
        "# LEDGER FORMAT\n"
        "#   <crate> pub_items=<N> pub_fns=<N> features=<N> error_variants=<N>\n"
        "#\n"
        "# COUNTS\n"
        "#   pub_items      pub struct|enum|type|trait|union|const|static|fn\n"
        "#                  declarations in crates/<crate>/src/**/*.rs\n"
        "#                  (pub(crate) excluded; #[cfg(test)] mod X; files and\n"
        "#                  inline #[cfg(test)] mod tests { } tails excluded)\n"
        "#   pub_fns        the pub fn subset of pub_items (diagnostic)\n"
        "#   features       keys under [features] in crates/<crate>/Cargo.toml\n"
        "#   error_variants variants of pub enum *Error and thiserror-derived\n"
        "#                  pub enum bodies\n"
        "#\n"
        "# RATCHET\n"
        "#   An increase is a surface growth: red. A decrease means the ledger\n"
        "#   must shrink in the SAME commit: red until the row is updated.\n"
        "#\n"
        "# MEASURED\n"
        "#   command: py -3 scripts/check_l1_surface.py --emit\n"
        "#   date:    2026-11-18\n"
        "#   scanned: %d public item(s) across %d crate(s)\n"
        % (total, len(crates)))
    for crate in crates:
        counts = measured[crate]
        out.write("%s pub_items=%d pub_fns=%d features=%d error_variants=%d\n"
                  % (crate, counts["pub_items"], counts["pub_fns"],
                     counts["features"], counts["error_variants"]))
    return 0


def _write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="ascii", newline="\n") as fh:
        fh.write(text)


def selftest():
    """Fixtures exercise the production judge (no parallel implementation)."""
    import gate_rc

    state = {"n": 0, "bad": 0}

    def check(label, got, want):
        state["n"] += 1
        if got != want:
            state["bad"] += 1
            print("  [FAIL] %s got=%r want=%r" % (label, got, want))
        else:
            print("  [PASS] %s -> %r" % (label, got))

    tmp = gate_rc.tmpdir(prefix="l1-surface-")
    try:
        # Crate `aaa`: 4 public items (2 structs/enums + 1 fn + 1 enum), 1
        # feature, 1 error enum with 2 variants. `src/tests.rs` is reached only
        # through `#[cfg(test)] mod tests;` and carries 2 extra pub items that
        # MUST NOT count.
        _write(os.path.join(tmp, "crates", "aaa", "Cargo.toml"),
               "[package]\nname = \"aaa\"\nversion = \"0.1.0\"\n\n"
               "[features]\ndefault = []\nextra = []\n")
        _write(os.path.join(tmp, "crates", "aaa", "src", "lib.rs"),
               "pub struct Kept;\n"
               "pub fn kept() {}\n"
               "pub(crate) fn hidden() {}\n"
               "pub enum KeptError {\n    One,\n    Two,\n}\n"
               "pub enum Plain {\n    A,\n    B,\n    C,\n}\n"
               "#[cfg(test)]\nmod tests {\n    pub struct InlineTail;\n}\n"
               "#[cfg(test)]\nmod tests_external;\n")
        _write(os.path.join(tmp, "crates", "aaa", "src", "tests_external.rs"),
               "pub struct ExternalTestOnly;\npub fn helper() {}\n")
        # Second crate so the roster is non-trivial and features=0 is proved.
        _write(os.path.join(tmp, "crates", "bbb", "Cargo.toml"),
               "[package]\nname = \"bbb\"\nversion = \"0.1.0\"\n")
        _write(os.path.join(tmp, "crates", "bbb", "src", "lib.rs"),
               "pub trait Marker {}\n")

        crates = ("aaa", "bbb")
        rows, total = measure(tmp, crates)
        check("pub_items counts only public declarations", rows["aaa"]["pub_items"], 4)
        check("pub_fns is the pub fn subset", rows["aaa"]["pub_fns"], 1)
        check("features counts [features] keys", rows["aaa"]["features"], 2)
        check("features is 0 when the table is absent", rows["bbb"]["features"], 0)
        check("error_variants counts only *Error bodies",
              rows["aaa"]["error_variants"], 2)
        check("external cfg(test) module files are not surface", total, 5)

        ledger = os.path.join(tmp, "freeze.txt")
        _write(ledger, "# fixture\n"
                       "aaa pub_items=4 pub_fns=1 features=2 error_variants=2\n"
                       "bbb pub_items=1 pub_fns=0 features=0 error_variants=0\n")
        rc, lines, _rows = judge(tmp, ledger, crates=crates, floor=1)
        check("equal ledger is green", rc, 0)

        grown = os.path.join(tmp, "grown.txt")
        _write(grown, "aaa pub_items=4 pub_fns=1 features=2 error_variants=1\n"
                      "bbb pub_items=1 pub_fns=0 features=0 error_variants=0\n")
        rc, lines, _rows = judge(tmp, grown, crates=crates, floor=1)
        check("growth is red", rc, 1)
        check("growth names the field and both values",
              any("error_variants grew: 1 -> 2" in ln for ln in lines), True)

        shrunk = os.path.join(tmp, "shrunk.txt")
        _write(shrunk, "aaa pub_items=9 pub_fns=1 features=2 error_variants=2\n"
                       "bbb pub_items=1 pub_fns=0 features=0 error_variants=0\n")
        rc, lines, _rows = judge(tmp, shrunk, crates=crates, floor=1)
        check("shrink is red (ledger must follow)", rc, 1)
        check("shrink asks for the ledger update",
              any("shrank: 9 -> 4" in ln for ln in lines), True)

        missing = os.path.join(tmp, "missing.txt")
        _write(missing, "aaa pub_items=4 pub_fns=1 features=2 error_variants=2\n")
        rc, lines, _rows = judge(tmp, missing, crates=crates, floor=1)
        check("missing crate row is red", rc, 1)

        extra = os.path.join(tmp, "extra.txt")
        _write(extra, "aaa pub_items=4 pub_fns=1 features=2 error_variants=2\n"
                      "bbb pub_items=1 pub_fns=0 features=0 error_variants=0\n"
                      "zzz pub_items=0 pub_fns=0 features=0 error_variants=0\n")
        rc, _lines, _rows = judge(tmp, extra, crates=crates, floor=1)
        check("unknown crate row is stale/red", rc, 1)

        malformed = os.path.join(tmp, "malformed.txt")
        _write(malformed, "aaa pub_items=x pub_fns=1 features=2 error_variants=2\n")
        rc, _lines, _rows = judge(tmp, malformed, crates=crates, floor=1)
        check("malformed row is undecidable", rc, 2)

        dup = os.path.join(tmp, "dup.txt")
        _write(dup, "aaa pub_items=3 pub_fns=1 features=2 error_variants=2\n"
                    "aaa pub_items=3 pub_fns=1 features=2 error_variants=2\n")
        rc, _lines, _rows = judge(tmp, dup, crates=crates, floor=1)
        check("duplicate crate row is undecidable", rc, 2)

        rc, _lines, _rows = judge(tmp, ledger, crates=crates, floor=10 ** 6)
        check("blind scan is undecidable", rc, 2)

        nowhere = os.path.join(tmp, "nope.txt")
        rc, _lines, _rows = judge(tmp, nowhere, crates=crates, floor=1)
        check("missing ledger is undecidable", rc, 2)

        # A crate whose src dir vanished must not look clean.
        rc, _lines, _rows = judge(tmp, ledger, crates=("aaa", "ghost"), floor=1)
        check("missing crate src dir is undecidable", rc, 2)

        # `#[non_exhaustive]`-style attributes must not be counted as variants.
        _write(os.path.join(tmp, "crates", "aaa", "src", "lib.rs"),
               "pub enum KeptError {\n"
               "    #[error(\"a\")]\n    One(String),\n"
               "    #[error(\"b\")]\n    Two {\n        id: u64,\n    },\n}\n")
        rows, _total = measure(tmp, crates)
        check("attributes and multi-line variants count once each",
              rows["aaa"]["error_variants"], 2)
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)

    print("  RESULT: %s (%d assertions)"
          % ("PASS" if not state["bad"] else "FAIL", state["n"]))
    return 1 if state["bad"] else 0


def main(argv):
    mode = "check"
    as_json = False
    for arg in argv:
        if arg == "--emit":
            mode = "emit"
        elif arg == "--selftest":
            mode = "selftest"
        elif arg == "--check":
            mode = "check"
        elif arg == "--json":
            as_json = True
        else:
            print("usage: check_l1_surface.py [--check|--emit|--selftest] [--json]")
            return 2

    if mode == "selftest":
        return selftest()
    if mode == "emit":
        rows, total = measure(ROOT)
        if as_json:
            sys.stdout.write(json.dumps({"scanned": total, "crates": rows},
                                        indent=2, sort_keys=True) + "\n")
            return 0
        return emit(ROOT)

    rc, report, measured = judge(ROOT, BASELINE)
    if as_json:
        sys.stdout.write(json.dumps({"verdict": rc, "report": report,
                                     "measured": measured},
                                    indent=2, sort_keys=True) + "\n")
        return rc
    for line in report:
        print(line)
    if rc == 2:
        print("[UNDECIDABLE] L1 public-surface gate cannot decide")
    elif rc == 1:
        print("[FAIL] L1 public-surface ledger drifted (see [UP]/[DOWN]/[STALE] above)")
    return rc


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
