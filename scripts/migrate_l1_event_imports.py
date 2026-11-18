#!/usr/bin/env python3
# -*- coding: ascii -*-
"""Migrate `use event_bus::<event symbol>` to `use nexus_events::<...>`.

Why this tool exists
--------------------
ADR-322 moved the event catalogue (`NexusEvent` + payloads + `EventTopic` +
registry) out of `event-bus` into its own crate `nexus-events`.  To keep the 37
downstream crates compiling with zero edits, `event-bus` carries a one-release
`compat-2x` block of pure path re-exports (`pub use nexus_events::{...}`).
v2.31.0-omega deletes that block, so every downstream import must be rewritten
FIRST.  This script is that rewrite, made mechanical and reviewable instead of
hand-edited across ~200 use statements.

Two modes, one judge
--------------------
  --check (default)   report only: pending statements, per-crate summary, first
                      20 detail lines.  Exit 1 when anything is pending, 0 when
                      the tree is already clean.
  --apply             rewrite the files.  NOT run by this wave; implemented so
                      the migration is one command when the compat window ends.

Symbol ownership is derived, never hard-coded: the set of names that belong to
`nexus-events` is re-read from `crates/nexus-events/src/**` on every run.  If
`NexusEvent` is not in that set the script refuses (exit 2) -- a blind scanner
would otherwise "migrate" nothing and still look green.

Rewrite rules (only `use` / `pub use` lines; full paths are untouched):
  * `use event_bus::X;`                     -> `use nexus_events::X;`
  * `use event_bus::{A, B, C};`             -> split by ownership into at most
    two lines, original order preserved inside each group, single-element
    braces dropped (`use a::B;`, not `use a::{B};`).
  * `use event_bus::X as Y;`                -> alias preserved verbatim.
  * a `self` item (`use event_bus::{self, X};`) is NEVER rewritten -- it means
    the module path itself is used, which is a human decision.  It is counted
    and reported separately.

Exit codes (shared repository dialect, see gate_rc.py):
  0 = no pending statement   1 = pending statements found   2 = undecidable
Unknown flags exit 2 with usage -- never silently fall back to "scan all".

Output is pure ASCII (project script convention).
"""

import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)

# Shared `#[cfg(test)] mod X;` walker -- imported, never copied, so the two
# gates cannot drift on what counts as production code.
import check_l1_dead_surface as l1dead  # noqa: E402

EVENTS_CRATE = "nexus-events"
CARRIER_CRATE = "event-bus"
EVENTS_TARGET = "nexus_events"
# Rust crate identifier of the carrier (the directory name has a hyphen).
CARRIER_TARGET = "event_bus"

# Below this many public names the extractor is presumed blind, not empty.
ITEM_FLOOR = 10

# A public type-like declaration.  Leading whitespace is ALLOWED on purpose:
# `NexusEvent` is assembled inside `macro_rules! define_nexus_event` and its
# `pub enum` line is indented into the macro body.  `pub(crate)` does not match
# (`pub` must be followed by whitespace), so crate-private items stay out.
PUB_TYPE_RE = re.compile(
    r"^[ \t]*pub[ \t]+(?:const[ \t]+|unsafe[ \t]+|extern[ \t]+\S+[ \t]+)*"
    r"(struct|enum|trait|union|type|const|static)[ \t]+([A-Za-z_]\w*)",
    re.MULTILINE)

# A public free function must start at column 0.  An indented `pub fn` is an
# inherent method (`pub fn severity(&self)`), which is not an importable name --
# keeping those in the set would invite a rewrite that cannot compile.
PUB_FN_RE = re.compile(
    r"^pub[ \t]+(?:async[ \t]+|const[ \t]+|unsafe[ \t]+|extern[ \t]+\S+[ \t]+)*"
    r"fn[ \t]+([A-Za-z_]\w*)",
    re.MULTILINE)

# `pub use ...;` possibly spanning lines (a rustfmt-wrapped brace group).
PUB_USE_RE = re.compile(r"^[ \t]*pub[ \t]+use[ \t]+([^;]*);", re.MULTILINE | re.DOTALL)

BLOCK_COMMENT_RE = re.compile(r"/\*.*?\*/", re.DOTALL)

# A downstream import of the carrier crate.  `[^;]*` also covers a brace group
# that rustfmt has wrapped over several lines; the repo is single-line today,
# but the parser must not silently skip a wrapped one.
USE_RE = re.compile(r"(?m)^([ \t]*)(pub[ \t]+)?use[ \t]+event_bus::([^;]*);")

USAGE = ("usage: migrate_l1_event_imports.py [--check | --apply] [--json]\n"
         "       migrate_l1_event_imports.py --selftest")


class ScanError(ValueError):
    pass


def _read(path):
    with open(path, "r", encoding="utf-8", errors="replace", newline="") as fh:
        return fh.read()


def _iter_rs(src_dir):
    for dirpath, _dirs, names in os.walk(src_dir):
        for name in sorted(names):
            if name.endswith(".rs"):
                yield os.path.join(dirpath, name)


def _prod_files(src_dir):
    """Every `.rs` under `src/` minus files reached by `#[cfg(test)] mod X;`."""
    skip = l1dead._test_module_files(src_dir)
    return [p for p in _iter_rs(src_dir)
            if os.path.normcase(os.path.abspath(p)) not in skip]


def split_top_level(body):
    """Split a brace-group body on commas that are not nested in `{...}`."""
    items = []
    depth = 0
    current = []
    for ch in body:
        if ch in "{([":
            depth += 1
        elif ch in "})]":
            depth -= 1
        if ch == "," and depth == 0:
            item = "".join(current).strip()
            if item:
                items.append(item)
            current = []
            continue
        current.append(ch)
    tail = "".join(current).strip()
    if tail:
        items.append(tail)
    return items


def _pub_use_names(body):
    """Names a `pub use` body re-exports (`self`, globs and paths skipped)."""
    names = []
    for item in split_top_level(body):
        text = item.strip()
        if not text:
            continue
        # `{A, B}` and `path::{A, B}` are both brace groups; recurse on the
        # braces so `A as B` inside them is split correctly.
        if text.endswith("}") and "{" in text:
            names.extend(_pub_use_names(text[text.rindex("{") + 1:-1]))
            continue
        if text == "self" or text == "*" or text.endswith("::*"):
            continue
        head, _sep, alias = text.partition(" as ")
        head = head.strip()
        alias = alias.strip()
        if not head:
            continue
        if head == "*" or head.endswith("::*"):
            continue
        names.append(alias or head.rsplit("::", 1)[-1])
    return names


def collect_symbols(root):
    """Public surface of `nexus-events` (declarations + re-exports)."""
    src_dir = os.path.join(root, "crates", EVENTS_CRATE, "src")
    if not os.path.isdir(src_dir):
        raise ScanError("missing crate src dir: %s" % src_dir)
    names = set()
    for path in _iter_rs(src_dir):
        text = BLOCK_COMMENT_RE.sub("", _read(path))
        for match in PUB_TYPE_RE.finditer(text):
            names.add(match.group(2))
        for match in PUB_FN_RE.finditer(text):
            names.add(match.group(1))
        for match in PUB_USE_RE.finditer(text):
            names.update(_pub_use_names(match.group(1)))
    return names


def _is_self_item(item):
    text = item.strip()
    return text == "self" or text.startswith("self as ")


def _is_migratable(item, symbols):
    """A bare name (optionally aliased) that nexus-events owns."""
    head = item.split(" as ")[0].strip()
    if not head or "::" in head:
        return False
    return head in symbols


def _render(indent, prefix, path, items):
    if len(items) == 1:
        return "%s%suse %s::%s;" % (indent, prefix, path, items[0])
    return "%s%suse %s::{%s};" % (indent, prefix, path, ", ".join(items))


def plan_use(indent, prefix, body, symbols):
    """Decide one import statement. Returns a dict:
    kind = "unchanged" | "rewrite" | "manual"; lines; migrated; kept."""
    text = body.strip()
    # A brace group is a single `{...}` whose commas belong to the group, not
    # to the statement: split its inner body, not the wrapper.
    if text.startswith("{") and text.endswith("}"):
        text = text[1:-1]
    items = split_top_level(text)
    if any(_is_self_item(i) for i in items):
        return {"kind": "manual", "lines": [], "migrated": [], "kept": items}
    migrated = [i for i in items if _is_migratable(i, symbols)]
    kept = [i for i in items if not _is_migratable(i, symbols)]
    if not migrated:
        return {"kind": "unchanged", "lines": [], "migrated": [], "kept": kept}
    lines = [_render(indent, prefix, EVENTS_TARGET, migrated)]
    if kept:
        lines.append(_render(indent, prefix, CARRIER_TARGET, kept))
    return {"kind": "rewrite", "lines": lines, "migrated": migrated, "kept": kept}


def rewrite_text(text, symbols, relpath):
    """(new_text, records) for one file; records are per changed statement."""
    records = []

    def replace(match):
        indent, prefix, body = match.group(1), match.group(2) or "", match.group(3)
        plan = plan_use(indent, prefix, body, symbols)
        if plan["kind"] == "unchanged":
            return match.group(0)
        line_no = text.count("\n", 0, match.start()) + 1
        record = {
            "file": relpath,
            "line": line_no,
            "kind": plan["kind"],
            "before": match.group(0),
            "after": plan["lines"],
            "migrated": plan["migrated"],
            "kept": plan["kept"],
        }
        records.append(record)
        if plan["kind"] == "manual":
            return match.group(0)
        sep = "\r\n" if "\r\n" in text else "\n"
        return sep.join(plan["lines"])

    return USE_RE.sub(replace, text), records


def scan_tree(root, symbols):
    """(records, per_crate) over every crate except the carrier and the source."""
    crates_dir = os.path.join(root, "crates")
    if not os.path.isdir(crates_dir):
        raise ScanError("missing crates dir: %s" % crates_dir)
    records = []
    per_crate = {}
    for crate in sorted(os.listdir(crates_dir)):
        if crate in (EVENTS_CRATE, CARRIER_CRATE):
            continue
        src_dir = os.path.join(crates_dir, crate, "src")
        if not os.path.isdir(src_dir):
            continue
        for path in _prod_files(src_dir):
            text = _read(path)
            if "event_bus::" not in text:
                continue
            rel = os.path.relpath(path, root).replace(os.sep, "/")
            _new, found = rewrite_text(text, symbols, rel)
            if not found:
                continue
            records.extend(found)
            row = per_crate.setdefault(crate, {"stmts": 0, "syms": 0, "manual": 0})
            for rec in found:
                if rec["kind"] == "rewrite":
                    row["stmts"] += 1
                    row["syms"] += len(rec["migrated"])
                else:
                    row["manual"] += 1
    return records, per_crate


def apply_tree(root, symbols):
    """Rewrite files in place. Returns the number of files touched."""
    crates_dir = os.path.join(root, "crates")
    touched = 0
    for crate in sorted(os.listdir(crates_dir)):
        if crate in (EVENTS_CRATE, CARRIER_CRATE):
            continue
        src_dir = os.path.join(crates_dir, crate, "src")
        if not os.path.isdir(src_dir):
            continue
        for path in _prod_files(src_dir):
            text = _read(path)
            if "event_bus::" not in text:
                continue
            rel = os.path.relpath(path, root).replace(os.sep, "/")
            new, found = rewrite_text(text, symbols, rel)
            if not any(rec["kind"] == "rewrite" for rec in found) or new == text:
                continue
            with open(path, "w", encoding="utf-8", newline="") as fh:
                fh.write(new)
            touched += 1
    return touched


def _summary(records, per_crate):
    rewrites = [r for r in records if r["kind"] == "rewrite"]
    manual = [r for r in records if r["kind"] == "manual"]
    return {
        "statements": len(rewrites),
        "symbols": sum(len(r["migrated"]) for r in rewrites),
        "crates": len(per_crate),
        "manual": len(manual),
        "rewrites": rewrites,
        "manual_records": manual,
        "per_crate": per_crate,
    }


def _print_report(symbols, result, limit=20):
    print("[SYMBOLS] %s public surface = %d name(s) from crates/%s/src"
          % (EVENTS_CRATE, len(symbols), EVENTS_CRATE))
    core = ["NexusEvent", "EventMetadata", "EventTopic"]
    print("[CORE] " + " ".join("%s=%s" % (n, "yes" if n in symbols else "no")
                               for n in core))
    print("[CHECK] %d use statement(s) pending across %d crate(s); "
          "%d symbol(s) migrate" % (result["statements"], result["crates"],
                                    result["symbols"]))
    rows = sorted(result["per_crate"].items(), key=lambda kv: (-kv[1]["stmts"], kv[0]))
    if rows:
        print("  crate                                  stmts  syms")
        for crate, row in rows:
            print("  %-38s %5d %5d" % (crate, row["stmts"], row["syms"]))
    if result["manual"]:
        print("[MANUAL] %d statement(s) refused (self group; human decision):"
              % result["manual"])
        for rec in result["manual_records"][:5]:
            print("  %s:%d: %s" % (rec["file"], rec["line"], rec["before"].strip()))
    if result["rewrites"]:
        print("[DETAIL] first %d of %d:"
              % (min(limit, result["statements"]), result["statements"]))
        for rec in result["rewrites"][:limit]:
            print("  %s:%d: %s" % (rec["file"], rec["line"], rec["before"].strip()))
            for line in rec["after"]:
                print("      -> %s" % line.strip())


def main(argv):
    if argv == ["--selftest"]:
        return selftest()
    mode = "check"
    want_json = False
    for arg in argv:
        if arg in ("--check", "--apply"):
            mode = arg[2:]
        elif arg == "--json":
            want_json = True
        else:
            print("unknown flag: %s" % arg)
            print(USAGE)
            return 2

    try:
        symbols = collect_symbols(ROOT)
        if "NexusEvent" not in symbols or len(symbols) < ITEM_FLOOR:
            print("[UNDECIDABLE] nexus-events surface looks blind: %d name(s), "
                  "NexusEvent=%s (floor %d)"
                  % (len(symbols), "yes" if "NexusEvent" in symbols else "no",
                     ITEM_FLOOR))
            return 2
        if mode == "apply":
            touched = apply_tree(ROOT, symbols)
            print("[APPLY] rewrote %d file(s); %d name(s) in the nexus-events surface"
                  % (touched, len(symbols)))
            return 0
        records, per_crate = scan_tree(ROOT, symbols)
    except ScanError as exc:
        print("[UNDECIDABLE] %s" % exc)
        return 2

    result = _summary(records, per_crate)
    if want_json:
        payload = {
            "mode": mode,
            "symbols": len(symbols),
            "core": {n: n in symbols for n in ("NexusEvent", "EventMetadata", "EventTopic")},
            "statements": result["statements"],
            "symbols_migrating": result["symbols"],
            "crates": result["crates"],
            "manual": result["manual"],
            "per_crate": {k: dict(v) for k, v in sorted(per_crate.items())},
            "records": records,
        }
        print(json.dumps(payload, ensure_ascii=True, indent=2))
    else:
        _print_report(symbols, result)
    rc = 1 if result["statements"] else 0
    if not want_json:
        if rc:
            print("[FAIL] compat-2x still carries %d migrated statement(s); run "
                  "--apply after the migration window closes" % result["statements"])
        else:
            print("[OK] no downstream import still reaches the event catalogue "
                  "through event_bus")
    return rc


def _write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="ascii", newline="\n") as fh:
        fh.write(text)


def selftest():
    """Fixtures prove both directions through the production rewrite path."""
    import gate_rc
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

    root = gate_rc.tmpdir("mig-l1-events-")
    _write(os.path.join(root, "crates", EVENTS_CRATE, "src", "lib.rs"),
           "pub mod payloads;\n"
           "macro_rules! define_nexus_event {\n"
           "    ($($variants:tt)*) => {\n"
           "        pub enum NexusEvent {\n"
           "            $($variants)*\n"
           "        }\n"
           "    };\n"
           "}\n"
           "pub enum EventTopic {\n    A,\n}\n"
           "pub struct Payload;\n")
    _write(os.path.join(root, "crates", EVENTS_CRATE, "src", "payloads", "mod.rs"),
           "pub use nexus_contracts::EventMetadata;\n"
           "pub struct ChatStatus;\n"
           "pub type Alias = u8;\n"
           "pub use other::{A as B, C};\n")

    lib = os.path.join(root, "crates", "aaa", "src", "lib.rs")
    _write(lib,
           "pub use event_bus::EventTopic;\n"
           "use event_bus::NexusEvent;\n"
           "use event_bus::NexusEvent as Ev;\n"
           "use event_bus::EventBus;\n"
           "use event_bus::{EventBus, NexusEvent, EventMetadata};\n"
           "use event_bus::{NexusEvent, VoteValue};\n"
           "use event_bus::{self, NexusEvent};\n"
           "use event_bus::logging::BusLogger;\n"
           "use event_bus::{logging::BusLogger, NexusEvent};\n"
           "\n"
           "#[cfg(test)]\n"
           "mod checks;\n")
    _write(os.path.join(root, "crates", "aaa", "src", "checks.rs"),
           "use event_bus::NexusEvent;\n")

    symbols = collect_symbols(root)
    check("NexusEvent is found inside the macro body", "NexusEvent" in symbols, True)
    check("indented macro declaration is scanned", "EventTopic" in symbols, True)
    check("re-exported single name is scanned", "EventMetadata" in symbols, True)
    check("aliased brace re-export yields the alias", "B" in symbols, True)
    check("crate-private carrier names stay out", "EventBus" in symbols, False)

    records, per_crate = scan_tree(root, symbols)
    result = _summary(records, per_crate)
    check("six statements migrate", result["statements"], 6)
    check("one self-group is refused", result["manual"], 1)
    check("only crate aaa is touched", sorted(per_crate), ["aaa"])

    new_text, _ = rewrite_text(_read(lib), symbols, "lib.rs")
    check("single import becomes nexus_events",
          "use nexus_events::NexusEvent;" in new_text, True)
    check("alias survives verbatim",
          "use nexus_events::NexusEvent as Ev;" in new_text, True)
    check("pub use keeps its prefix",
          "pub use nexus_events::EventTopic;" in new_text, True)
    check("mixed group keeps original order in each half",
          "use nexus_events::{NexusEvent, EventMetadata};\nuse event_bus::EventBus;"
          in new_text, True)
    check("single-element braces are dropped",
          "use event_bus::EventBus;" in new_text and "use event_bus::{EventBus};"
          not in new_text, True)
    check("unowned group stays untouched",
          "use event_bus::EventBus;\n" in new_text, True)
    check("self group is left byte-identical",
          "use event_bus::{self, NexusEvent};" in new_text, True)
    check("nested module path is left alone",
          "use event_bus::logging::BusLogger;" in new_text, True)
    check("nested path splits off the owned name only",
          "use nexus_events::NexusEvent;\nuse event_bus::logging::BusLogger;"
          in new_text, True)
    check("cfg(test) external module is not scanned",
          any(r["file"].endswith("checks.rs") for r in records), False)

    touched = apply_tree(root, symbols)
    check("apply rewrote exactly one file", touched, 1)
    check("apply is idempotent on a second pass",
          any(r["kind"] == "rewrite" for r in scan_tree(root, symbols)[0]), False)

    check("unknown flag exits 2", main(["--bogus"]), 2)

    print("  RESULT: %s (%d assertions)" % ("PASS" if not bad else "FAIL", n))
    return 1 if bad else 0


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
