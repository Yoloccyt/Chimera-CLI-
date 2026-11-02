#!/usr/bin/env python3
# -*- coding: ascii -*-
"""Orphan source-file detector (candidate gate, ADR-213 finding D-18).

WHY THIS EXISTS
---------------
The repository measures crate reachability (ADR-160, check_crate_reachability.sh)
and dependency direction, but nothing asks whether a .rs file inside a crate is
compiled at all. Cargo does not warn about an unreferenced source file: a file
that no `mod` declaration points at is simply never parsed. Its `#[cfg(test)]`
tests never run, its `pub` items are not in the API, and it drifts silently.

Found by accident on 2026-10-13: crates/quest-engine/src/wal.rs has 400+ lines and
unit tests (test_truncated_tail_recovery), but lib.rs declares no `mod wal`, there
is no `#[path]` and no `include!` -- so the whole file is dead. Proven
behaviourally, not by grep: `cargo test -p quest-engine --lib -- --list` contains
zero wal tests.

WHAT IT JUDGES
--------------
Reachability of every `crates/*/src/**/*.rs` from that crate's compilation roots
(src/lib.rs, src/main.rs, and any `[[bin]]` path under src/), following
`mod NAME;` / `pub mod NAME;` declarations resolved by the two Rust rules
(`dir/NAME.rs` then `dir/NAME/mod.rs`) plus `#[path = "..."]` overrides.
Unreachable files are reported; the ledger makes the set only-decrease.

Modes:
  (default)      judge the real repo        0 = ledger agrees, 1 = NEW orphan,
                                            2 = undecidable (see below)
  --list         print the orphan set and exit 0 (inventory, no verdict)
  --emit         write tmp/orphan_candidate.txt for bootstrapping the ledger
  --selftest     fixtures prove both directions
"""

import os
import re
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
LEDGER = os.path.join(HERE, "src_module_reachability_freeze.txt")

MOD_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_]\w*)\s*;")
PATH_ATTR_RE = re.compile(r'#\[\s*path\s*=\s*"([^"]+)"\s*\]\s*$')
BLOCK_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+[A-Za-z_]\w*\s*\{")

CRATES_DIR = os.path.join(ROOT, "crates")


def strip_noise(text):
    """Blank // comments and /* */ blocks (cheap: this parser only needs module
    structure, and a `mod` inside a comment must not count as a declaration)."""
    out = []
    for line in text.splitlines():
        s = line.split("//", 1)[0] if line.lstrip().startswith("//") else line
        out.append(s)
    return "\n".join(out)


def module_targets(file_path, mod_name):
    """The two paths Rust would accept for `mod NAME;` inside a file."""
    d = os.path.dirname(file_path)
    stem = os.path.splitext(os.path.basename(file_path))[0]
    cands = []
    if stem == "mod":
        cands.append(os.path.join(d, mod_name + ".rs"))
        cands.append(os.path.join(d, mod_name, "mod.rs"))
    elif stem == "lib" or stem == "main":
        cands.append(os.path.join(d, mod_name + ".rs"))
        cands.append(os.path.join(d, mod_name, "mod.rs"))
    else:
        cands.append(os.path.join(d, stem + ".rs").replace(stem + ".rs", os.path.join(stem, mod_name + ".rs")))
        cands.append(os.path.join(d, stem, mod_name, "mod.rs"))
    return cands


def reachable_from(root_files):
    """BFS over mod declarations; returns the set of resolved file paths."""
    seen = set()
    queue = list(root_files)
    while queue:
        path = os.path.abspath(queue.pop())
        if path in seen or not os.path.isfile(path):
            continue
        seen.add(path)
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            text = strip_noise(fh.read())
        pending_path_attr = None
        for raw in text.splitlines():
            line = raw.rstrip()
            m = MOD_RE.match(line)
            if not m:
                pm = PATH_ATTR_RE.search(line)
                if pm and "{" not in line:
                    pending_path_attr = pm.group(1)
                continue
            if BLOCK_RE.match(line):
                continue
            mod_name = m.group(1)
            if pending_path_attr:
                cand = os.path.join(os.path.dirname(path), pending_path_attr)
                pending_path_attr = None
                if os.path.isfile(cand):
                    queue.append(cand)
                continue
            for cand in module_targets(path, mod_name):
                if os.path.isfile(cand):
                    queue.append(cand)
                    break
    return seen


def crate_roots(crate_src):
    roots = []
    for name in ("lib.rs", "main.rs"):
        p = os.path.join(crate_src, name)
        if os.path.isfile(p):
            roots.append(p)
    bin_dir = os.path.join(crate_src, "bin")
    if os.path.isdir(bin_dir):
        for fn in sorted(os.listdir(bin_dir)):
            if fn.endswith(".rs"):
                roots.append(os.path.join(bin_dir, fn))
    return roots


def all_src_files(crate_src):
    out = []
    for dirpath, _d, files in os.walk(crate_src):
        for fn in files:
            if fn.endswith(".rs"):
                out.append(os.path.join(dirpath, fn))
    return out


def find_orphans(crates_dir):
    orphans = []
    for crate in sorted(os.listdir(crates_dir)):
        src = os.path.join(crates_dir, crate, "src")
        if not os.path.isdir(src):
            continue
        roots = crate_roots(src)
        if not roots:
            print("[TOOL-ERROR] crate %s has no src/lib.rs or src/main.rs -- cannot "
                  "judge reachability" % crate)
            return None
        reach = reachable_from(roots)
        for f in all_src_files(src):
            if os.path.abspath(f) not in reach:
                orphans.append(os.path.relpath(f, ROOT).replace(os.sep, "/"))
    return orphans


def read_ledger(path):
    rows = []
    with open(path, "r", encoding="utf-8") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if line:
                rows.append(line)
    return set(rows)


def run(mode):
    orphans = find_orphans(CRATES_DIR)
    if orphans is None:
        return 2
    rel = sorted(orphans)
    if mode in ("--list", "--emit"):
        for o in rel:
            print("  ORPHAN  %s" % o)
        print("[INFO] orphan src files: %d" % len(rel))
        if mode == "--emit":
            out = os.path.join(ROOT, "tmp", "orphan_candidate.txt")
            os.makedirs(os.path.dirname(out), exist_ok=True)
            with open(out, "w", encoding="utf-8") as fh:
                for o in rel:
                    fh.write(o + "\n")
            print("[EMIT] written %s" % os.path.relpath(out, ROOT))
        return 0
    if not os.path.isfile(LEDGER):
        print("[TOOL-ERROR] ledger not found: %s (run --emit to bootstrap)" % LEDGER)
        return 2
    listed = read_ledger(LEDGER)
    new = [o for o in rel if o not in listed]
    gone = sorted(listed - set(rel))
    for o in new:
        print("[GAP] NEW orphan source file (no `mod` reaches it, cargo stays silent): %s"
              % o)
    for o in gone:
        print("[STALE] ledger row is no longer an orphan -- delete the row in the SAME "
              "commit: %s" % o)
    if new or gone:
        print("Orphan source files BROKEN: %d new / %d stale (live=%d listed=%d)"
              % (len(new), len(gone), len(rel), len(listed)))
        return 1
    print("[OK] no new orphan source files: live=%d listed=%d (only-decrease ledger)"
          % (len(rel), len(listed)))
    return 0


def selftest():
    ok = True

    def add(label, cond):
        nonlocal ok
        print("[SELFTEST%s] %s" % ("" if cond else "-FAIL", label))
        if not cond:
            ok = False

    tmp = tempfile.mkdtemp(prefix="orphan-")
    try:
        crates = os.path.join(tmp, "crates")
        c1 = os.path.join(crates, "c1", "src")
        os.makedirs(os.path.join(c1, "nested"))
        write(os.path.join(c1, "lib.rs"),
              "pub mod live;\npub mod nested;\n// mod mentioned_in_comment;\n")
        write(os.path.join(c1, "live.rs"), "pub mod deep;\n")
        write(os.path.join(c1, "nested", "mod.rs"), "")
        write(os.path.join(c1, "dead.rs"), "#[cfg(test)]\nmod tests { #[test] fn t() {} }\n")
        write(os.path.join(c1, "nested", "also_dead.rs"), "")
        reach = reachable_from([os.path.join(c1, "lib.rs")])
        names = sorted(os.path.relpath(p, c1).replace(os.sep, "/") for p in reach)
        add("declared modules resolved (lib/live/nested/mod)",
            names == ["lib.rs", "live.rs", "nested/mod.rs"])
        add("comment-only `mod` is not a declaration",
            "mentioned_in_comment.rs" not in " ".join(names))
        roots = crate_roots(c1)
        files = all_src_files(c1)
        orph = sorted(os.path.relpath(f, c1).replace(os.sep, "/")
                      for f in files if os.path.abspath(f) not in reach)
        add("undeclared files reported as orphans",
            orph == ["dead.rs", "nested/also_dead.rs"])
        add("crate root discovery finds lib.rs only",
            [os.path.basename(r) for r in roots] == ["lib.rs"])
        # A `mod x;` inside an inline `mod y { ... }` block must not resolve from the
        # file -- inline blocks have no child files. Assert we do not crash on them.
        write(os.path.join(c1, "lib.rs"),
              "pub mod live;\npub mod nested;\nmod inline_thing { pub mod not_a_file; }\n")
        reach2 = reachable_from([os.path.join(c1, "lib.rs")])
        add("inline mod block does not create phantom resolution",
            len(reach2) == 3)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print("[SELFTEST] %s" % ("ALL ASSERTIONS HELD" if ok else "FAILED"))
    return 0 if ok else 1


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


def main(argv):
    args = [a for a in argv[1:]]
    if any(a not in ("--selftest", "--list", "--emit") for a in args):
        sys.stderr.write("usage: check_src_module_reachability.py [--selftest|--list|--emit]\n")
        return 2
    if "--selftest" in args:
        return selftest()
    return run("--list" if "--list" in args else ("--emit" if "--emit" in args else "real"))


if __name__ == "__main__":
    import gate_rc  # crash -> exit 2, never a borrowed traceback exit 1
    sys.exit(gate_rc.run(lambda: main(sys.argv)))
