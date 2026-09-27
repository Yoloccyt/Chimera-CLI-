#!/usr/bin/env python3
# =============================================================================
# check_doc_pointer_integrity.py - dead `path:line` pointer gate (B0-5)
# =============================================================================
# Purpose: the files that steer work (agents.md, the two rules copies, CLAUDE.md,
#          CODE_WIKI.md) are full of `crates/x/src/y.rs:NN` anchors. When a crate
#          is deleted or a file moves, the anchor outlives its target and the next
#          reader -- human or agent -- opens a file that is not there. Batch A
#          (38c72e6) deleted `model-router`; one such anchor is still on disk.
#
# WHAT IS JUDGED, and why nothing else is:
#   anchored  `crates/foo/src/bar.rs:12`   -> must resolve, line must be in range
#   shorthand `mca-gateway/src/x.rs:131`   -> bridged to crates/..., judged if the
#                                             suffix matches exactly one file
#   bare      `domain.rs:37`               -> ADVISORY ONLY. This repo has several
#                                             `src/types.rs`, two `src/wal.rs`; a
#                                             bare name cannot be judged without
#                                             guessing, and a wrong guess would
#                                             print a red that is not true.
#
# WHY SYMBOL ADJACENCY IS NOT IN THIS GATE (measured, do not re-litigate):
#   An earlier draft also asserted "the symbol named next to the anchor sits at
#   that line". On the 6 agent-facing docs it produced 38 OK / 9 DRIFT / 6 ABSENT,
#   then every sampled failure turned out to be prose, not rot:
#     * `bus.rs:48` cites the `CRITICAL_MPSC_VARIANTS` const block while naming
#       `is_critical_mpsc_event`, which the block's own doc comment references;
#     * a table cell whose subject is `SparseMask` also mentions `OmniSparseMasks`,
#       which lives in a *different* file -- pairing cannot tell cross-reference
#       from claim without understanding the sentence.
#   A gate that reds on 28% of its own sample gets disabled inside a month, which
#   is worse than no gate. So: existence and range only -- those are decidable.
#
# Baseline: scripts/doc_pointer_residue.txt, rows `doc|pointer|kind` + reason,
#   only-decrease. A row whose pointer has been repaired is STALE (red): an
#   allowance must not outlive the debt it allowed.
# Exit dialect (shared with gate_rc): 0 clean / 1 residue grew or went stale /
#   2 undecidable (missing registry, row without a reason, crash).
#
# Usage:
#   py -3 scripts/check_doc_pointer_integrity.py --selftest
#   py -3 scripts/check_doc_pointer_integrity.py --emit
#   py -3 scripts/check_doc_pointer_integrity.py
# =============================================================================
import argparse
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SURFACE = os.path.join(ROOT, "scripts", "doc_pointer_surface.txt")
BASELINE = os.path.join(ROOT, "scripts", "doc_pointer_residue.txt")

EXT = r"(?:rs|py|sh|ps1|toml|yml|yaml|md|json|txt|lock)"
# A pointer: something that looks like a path, then `:` or fullwidth colon, then
# a line number. Paths without a slash are kept but only counted as advisory.
PTR_RE = re.compile(
    r"([A-Za-z0-9_\-./\u4e00-\u9fff]*[A-Za-z0-9_\-.]+\.(?:%s))[:：](\d{1,4})" % EXT)

JUDGE_DIRS = ("crates", "scripts", "docs", "tests", ".github", "fuzz",
              "examples", ".claude", "archives")
SKIP_PARTS = ("target", ".git", ".toolchain", "node_modules", "tmp", "tmp_podman")

index = {}


def set_root(path):
    """Re-point every path at another repo root (used by --selftest fixtures)."""
    global ROOT, SURFACE, BASELINE, index
    ROOT = path
    SURFACE = os.path.join(path, "scripts", "doc_pointer_surface.txt")
    BASELINE = os.path.join(path, "scripts", "doc_pointer_residue.txt")
    index = {}


def read_list(path, what):
    """Read a `value # reason` registry into {value: reason}.

    Raises ValueError on a line without a reason: an unexplained allowance is how
    a registry quietly becomes a dumping ground.
    """
    out = {}
    if not os.path.exists(path):
        raise ValueError("%s missing (%s)" % (what, path))
    with open(path, encoding="utf-8", errors="replace") as fh:
        for raw in fh:
            line, _, why = raw.partition("#")
            key = line.strip()
            if not key:
                continue
            if not why.strip():
                raise ValueError("%s: line needs a trailing '# reason': %r" % (what, key))
            out[key] = why.strip()
    return out


def build_index():
    """basename -> [repo-relative paths], so shorthand anchors can be bridged."""
    if index:
        return
    for top in JUDGE_DIRS:
        base = os.path.join(ROOT, top)
        if not os.path.isdir(base):
            continue
        for dirpath, dirs, files in os.walk(base):
            rel_dir = os.path.relpath(dirpath, ROOT).replace(os.sep, "/")
            dirs[:] = [d for d in dirs if d not in SKIP_PARTS]
            if any(part in SKIP_PARTS for part in rel_dir.split("/")):
                continue
            for fn in files:
                rel = "%s/%s" % (rel_dir, fn) if rel_dir != "." else fn
                index.setdefault(fn, []).append(rel.replace(os.sep, "/"))


def resolve(path):
    """Return (state, [targets]).

    state: 'rooted' (exists relative to root) | 'shorthand' (exactly one file ends
    with this path) | 'bare' (no slash -- unjudgeable) | 'multi' (ambiguous) |
    'dead' (nothing could be this file).
    """
    build_index()
    if os.path.isfile(os.path.join(ROOT, path)):
        return "rooted", [path]
    if "/" not in path:
        return "bare", []
    hits = [rel for rel in index.get(path.rsplit("/", 1)[-1], [])
            if rel.endswith("/" + path)]
    if len(hits) == 1:
        return "shorthand", hits
    if len(hits) > 1:
        return "multi", hits
    return "dead", []


def nlines(rel):
    try:
        with open(os.path.join(ROOT, rel), encoding="utf-8", errors="replace") as fh:
            return sum(1 for _ in fh)
    except OSError:
        return None


def scan_doc(rel_doc):
    """Return (findings, advisory_count) for one document.

    A missing document raises: "nothing to check because the file is gone" is not
    clean, it is undecidable -- and the *.md files here are untracked, so they CAN
    vanish while the register still lists them.
    """
    findings, advisory = [], 0
    doc = os.path.join(ROOT, rel_doc)
    if not os.path.isfile(doc):
        raise ValueError("surface doc missing: %s" % rel_doc)
    with open(doc, encoding="utf-8-sig", errors="replace") as fh:
        for ln, text in enumerate(fh, 1):
            for path, line in PTR_RE.findall(text):
                state, targets = resolve(path)
                if state in ("bare", "multi"):
                    advisory += 1
                    continue
                ptr = "%s:%s" % (path, line)
                if state == "dead":
                    findings.append("%s|%s|dead-file" % (rel_doc, ptr))
                    continue
                n = nlines(targets[0])
                if n is not None and int(line) > n:
                    findings.append("%s|%s|line-beyond(%d)" % (rel_doc, ptr, n))
    return findings, advisory


TBL_SEP = re.compile(r"^\|[\s:|-]+\|?\s*$")


def _cols(line):
    """Unescaped pipe count of a table row.

    GFM allows a literal `|` inside a cell ONLY as `\\|`, and that escaping works
    even inside a code span -- so counting unescaped pipes is the column count.
    (This is the miscount that made an earlier probe read a 6-column row as 7.)
    """
    return len(re.findall(r"(?<!\\)\|", line))


def scan_tables(rel_doc):
    """Rows whose column count disagrees with their own table header.

    WHY zero tolerance (no register row, unlike the pointer findings): a broken
    row is always wrong and costs one character to fix, while a line-keyed
    allowance here would drift the moment anyone edits the document above it --
    turning one real defect into a permanent false red. That drift is exactly
    what scripts/retired_crate_residue.txt had to be designed around.

    WHY it matters: the truncation is invisible in raw text and only shows when
    the table renders, and these seven files are the instruction surface humans
    and agents read before touching the code.

    KNOWN BOUNDARY: only triple-backtick fences are recognised. The tilde fence
    dialect measures 0 occurrences across the seven surface docs (2026-09-24), so
    handling it would be code for a case that cannot happen here; if a `~~~`
    block is ever introduced, a `|---|` inside it would read as a table header.
    """
    doc = os.path.join(ROOT, rel_doc)
    if not os.path.isfile(doc):
        raise ValueError("surface doc missing: %s" % rel_doc)
    with open(doc, encoding="utf-8-sig", errors="replace") as fh:
        lines = fh.read().split("\n")
    out, in_fence, i = [], False, 0
    while i < len(lines):
        if lines[i].lstrip().startswith("```"):
            # a `|---|` inside a fenced block (ASCII tables in docs are common)
            # is not a table header -- judging it would invent findings.
            in_fence = not in_fence
            i += 1
            continue
        if in_fence:
            i += 1
            continue
        if (lines[i].startswith("|") and i + 1 < len(lines)
                and "-" in lines[i + 1] and TBL_SEP.match(lines[i + 1])):
            want = _cols(lines[i])
            j = i + 2
            while j < len(lines) and lines[j].startswith("|"):
                got = _cols(lines[j])
                if got != want:
                    out.append("%s:%d:cols(%d!=%d)" % (rel_doc, j + 1, got, want))
                j += 1
            i = j
            continue
        i += 1
    return out


def check():
    """Compare measured pointers against the only-decrease register."""
    surface = sorted(read_list(SURFACE, "surface registry"))
    baseline = read_list(BASELINE, "residue register")
    measured, table_bad, advisory_total = [], [], 0
    for rel in surface:
        f, adv = scan_doc(rel)
        measured.extend(f)
        advisory_total += adv
        table_bad.extend(scan_tables(rel))
    got = set(measured)
    new = sorted(got - set(baseline))
    stale = sorted(k for k in baseline if k not in got)
    print("[SCAN] %d docs | %d dead anchors measured | %d registered | "
          "%d new | %d stale | %d advisory (bare/ambiguous, deliberately unjudged)"
          % (len(surface), len(measured), len(baseline), len(new), len(stale),
             advisory_total))
    print("[SCAN] %d malformed table row(s) (zero tolerance, no register)"
          % len(table_bad))
    for row in new:
        print("[GAP-P] NEW dead anchor: %s" % row)
    for row in stale:
        print("[STALE] registered row no longer measured (repair the register): %s"
              % row)
    for row in table_bad:
        print("[GAP-T] table row column count != header: %s" % row)
    if new or stale or table_bad:
        print("[FAIL] %d new, %d stale, %d malformed table row(s)"
              % (len(new), len(stale), len(table_bad)))
        return 1
    print("[OK] every anchored pointer in the surface resolves (register: %d)"
          % len(baseline))
    return 0


def emit():
    surface = sorted(read_list(SURFACE, "surface registry"))
    rows = []
    for rel in surface:
        rows.extend(scan_doc(rel)[0])
    if not rows:
        print("# no unresolvable anchors measured on %s" % ", ".join(surface))
        return 0
    for row in sorted(set(rows)):
        print("%s  # measured %s via --emit" % (row, "scan"))
    return 0


# ---------------------------------------------------------------------------
# selftest: fixtures must flip the verdict, or the gate has no teeth.
# ---------------------------------------------------------------------------
def _write(path, text):
    d = os.path.dirname(path)
    if d and not os.path.isdir(d):
            os.makedirs(d)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(text)


def selftest():
    real_root, real_surface, real_baseline = ROOT, SURFACE, BASELINE
    tmp = os.path.join(real_root, "tmp", "doc_pointer_selftest")
    import shutil
    shutil.rmtree(tmp, ignore_errors=True)
    ok = fail = 0

    def expect(name, want, fn):
        nonlocal ok, fail
        try:
            got = fn()
        except ValueError as exc:
            got = "raised"  # the message varies; the *fact* of refusing is the verdict
        if got == want:
            ok += 1
            print("  [PASS] %s" % name)
        else:
            fail += 1
            print("  [FAIL] %s: want %r got %r" % (name, want, got))

    for crate in ("demo", "other"):
        _write(os.path.join(tmp, "crates", crate, "src", "lib.rs"),
               "\n".join("//! line %d" % i for i in range(1, 21)))
    _write(os.path.join(tmp, "scripts", "tool.py"), "print(1)\n")

    docs = {
        "good-rooted.md": "见 `crates/demo/src/lib.rs:5` 与 `scripts/tool.py:1`。\n",
        "good-shorthand.md": "`demo/src/lib.rs:7` 是 crate 相对简写。\n",
        "dead-file.md": "`crates/gone/src/x.rs:3` 指向已删 crate。\n",
        "line-beyond.md": "`crates/demo/src/lib.rs:999` 越界。\n",
        "bare-name.md": "`lib.rs:9` 裸名，不可判。\n",
        "ambiguous.md": "`src/lib.rs:4` 两个候选，不可判。\n",
        # table-integrity fixtures: one clean, one planted defect, two negative
        # controls (the escaping and the fence -- both must NOT be flagged).
        "table-good.md": "| x | y |\n|---|---|\n| a | b |\n",
        "table-short-row.md": "| x | y |\n|---|---|\n| a | b\n",
        "table-escaped.md": "| x | y |\n|---|---|\n| `grep -c p \\| wc -l` | b |\n",
        "table-fenced.md": ("text\n```\n| x | y |\n|---|---|\n| a | b\n```\nend\n"),
    }
    for name, body in docs.items():
        _write(os.path.join(tmp, "docs", name), body)

    def doc_case(stem):
        set_root(tmp)
        return scan_doc("docs/%s.md" % stem)

    set_root(real_root)
    try:
        expect("rooted anchor -> no finding", ([], 0),
               lambda: doc_case("good-rooted"))
        expect("crate-relative shorthand -> no finding (no false red)", ([], 0),
               lambda: doc_case("good-shorthand"))
        expect("PLANTED dead crate anchor -> red",
               (["docs/dead-file.md|crates/gone/src/x.rs:3|dead-file"], 0),
               lambda: doc_case("dead-file"))
        expect("PLANTED line beyond EOF -> red",
               (["docs/line-beyond.md|crates/demo/src/lib.rs:999|line-beyond(20)"], 0),
               lambda: doc_case("line-beyond"))
        expect("NEGATIVE CONTROL bare filename -> advisory, never a finding",
               ([], 1), lambda: doc_case("bare-name"))
        expect("NEGATIVE CONTROL ambiguous shorthand -> advisory",
               ([], 1), lambda: doc_case("ambiguous"))
        # a typo'd/untracked surface row must not read as "clean":
        expect("missing surface doc -> undecidable, never a false green", "raised",
               lambda: _expect_raise(lambda: doc_case("typo-stem")))

        def table_case(stem):
            set_root(tmp)
            return scan_tables("docs/%s.md" % stem)

        expect("clean table -> no finding", [],
               lambda: table_case("table-good"))
        expect("PLANTED row missing its trailing pipe -> red",
               ["docs/table-short-row.md:3:cols(2!=3)"],
               lambda: table_case("table-short-row"))
        expect("NEGATIVE CONTROL escaped pipe inside a cell -> not flagged",
               [], lambda: table_case("table-escaped"))
        expect("NEGATIVE CONTROL `|---|` inside a fenced block -> not a table",
               [], lambda: table_case("table-fenced"))
        expect("missing surface doc (tables) -> undecidable", "raised",
               lambda: _expect_raise(lambda: table_case("typo-stem")))

        # verdict plumbing: registered -> green; repaired but registered -> STALE
        _write(os.path.join(tmp, "scripts", "doc_pointer_surface.txt"),
               "docs/dead-file.md  # fixture\n")
        _write(os.path.join(tmp, "scripts", "doc_pointer_residue.txt"),
               "docs/dead-file.md|crates/gone/src/x.rs:3|dead-file  # allowed\n")
        set_root(tmp)
        import io
        from contextlib import redirect_stdout

        def quiet(fn):
            buf = io.StringIO()
            with redirect_stdout(buf):
                rc = fn()
            return rc

        expect("registered residue -> 0", 0, lambda: quiet(check))
        _write(os.path.join(tmp, "scripts", "doc_pointer_residue.txt"),
               "docs/dead-file.md|crates/gone/src/other.rs:3|dead-file  # stale row\n")
        expect("repaired pointer but row kept -> STALE = 1", 1, lambda: quiet(check))
        _write(os.path.join(tmp, "scripts", "doc_pointer_residue.txt"),
               "docs/dead-file.md|crates/gone/src/x.rs:3|dead-file\n")
        expect("row without a reason -> undecidable", "raised",
               lambda: _expect_raise(check))
        _write(os.path.join(tmp, "scripts", "doc_pointer_residue.txt"), "# empty\n")
        expect("unregistered new residue -> 1", 1, lambda: quiet(check))
        expect("missing registry -> undecidable", "raised",
               lambda: _expect_raise(lambda: read_list(
                   os.path.join(tmp, "scripts", "nope.txt"), "x")))
    finally:
        set_root(real_root)
    print("\nselftest: %d passed, %d failed" % (ok, fail))
    return 0 if fail == 0 else 1


def _expect_raise(fn):
    try:
        fn()
    except ValueError:
        return "raised"
    return "no-raise"


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--emit", action="store_true")
    ap.add_argument("--set-root", default=None)
    a = ap.parse_args()
    if a.set_root:
        set_root(a.set_root)
    import gate_rc  # a crash must read 2 (undecidable), never 1 (judged red)
    chosen = selftest if a.selftest else (emit if a.emit else check)
    sys.exit(gate_rc.run(chosen))
