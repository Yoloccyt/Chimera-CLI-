#!/usr/bin/env python3
"""check_sync_offload_guard.py -- synchronous batch offload keeps its worker-yield guard (G-88).

Why this script exists (round-3 audit decision 3 + F-U1 wiring risk):
    `spawn_compute_batch` is synchronous: inside a tokio `async fn` it occupies
    a worker for the whole rayon window. The P1/N-2 batches fixed the two live
    async call paths (faae route, osa single-profile batch) and the hcw window
    path with the SAME pattern: `Handle::try_current()` runtime-flavor branch ->
    multi-thread runs `tokio::task::block_in_place(...)`, current_thread (and
    non-tokio executors) call directly. Decision 3 of this round upgrades that
    pattern from "how the code happens to be written" to a FORCED mode for every
    synchronous-offload chain -- which needs a gate, because today a silent
    removal of one line (`block_in_place(` -> direct call) or a new `async fn`
    around a "NO_ASYNC_BOUNDARY" entry would go completely unnoticed.

Known limitation registered ON PURPOSE (mirrors the in-code comments at
osa-coordinator/src/coordinator.rs:540-548 and faae-router/src/parallel.rs:268-276):
    the flavor check is NOT the real guard -- tokio actually gates on its private
    `allow_block_in_place` flag, so `block_in_place` inside a `LocalSet` still
    panics even on a MultiThread handle. No static rule can decide LocalSet
    context, so this gate asserts the guard text EXISTS where the baseline says
    it must; LocalSet-safety remains a runtime-review concern, stated here so
    the gate is never mistaken for full coverage.

Checks (all text-level, deterministic, light):
    1. ANCHOR_STALE        -- the frozen `path::fn` anchor no longer defines the fn.
    2. GUARD_MISSING       -- a GUARDED row's file lost `block_in_place(` or `try_current`.
    3. ASYNC_APPEARED      -- a NO_ASYNC_BOUNDARY row's file gained `async fn`
                              (the chain head must be re-adjudicated to GUARDED).
    4. FILE_UNACCOUNTED    -- a production file now calls `.spawn_compute_batch(`
                              but no frozen anchor lives in it (new chain needs review;
                              pair with G-87 which catches the call-site dimension).

Line schema (2 pipe fields; keys must never contain '|', learned from G-87 v1):
    crate/src/path.rs::fn_name|STATE

Semantics: 0 baseline == reality / 1 any drift / 2 inputs unreadable (never a silent pass).
Usage: python scripts/check_sync_offload_guard.py [--root crates] [--freeze PATH]
       [--json] [--selftest]
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ENTRY_SEP = "::"
STATES = {"GUARDED", "NO_ASYNC_BOUNDARY"}
GUARD_RE = re.compile(r"\bblock_in_place\s*\(")
FLAVOR_RE = re.compile(r"\btry_current\b")
ASYNC_FN_RE = re.compile(r"\basync\s+fn\b")
CALL_SITE_RE = re.compile(r"\.spawn_compute_batch\(")


def load_freeze(path: Path) -> dict[str, str]:
    rows: dict[str, str] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split("|")
        if len(parts) != 2 or ENTRY_SEP not in parts[0]:
            raise ValueError(f"malformed freeze line (want 'crate/src/f.rs::fn|STATE'): {raw!r}")
        key, state = parts
        if state not in STATES:
            raise ValueError(f"unknown state {state!r} in: {raw!r}")
        if "/" not in key.split(ENTRY_SEP)[0]:
            raise ValueError(f"key must be crate/src/path.rs::fn: {raw!r}")
        rows[key] = state
    return rows


CFG_TEST_RE = re.compile(r"^\s*#\[cfg\(test\)\]")
MODULE_DECL_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w")


def split_key(key: str) -> tuple[str, str]:
    file_part, fn = key.rsplit(ENTRY_SEP, 1)
    return file_part, fn


def _next_line_is_module(lines: list[str], cfg_idx: int) -> bool:
    for later in lines[cfg_idx + 1:]:
        stripped = later.strip()
        if not stripped or stripped.startswith("#") or stripped.startswith("///") \
                or stripped.startswith("//!"):
            continue
        return bool(MODULE_DECL_RE.match(later))
    return False


def _prod_lines(path: Path) -> list[tuple[int, str]]:
    """Production-scope lines only -- SAME test-tail rule as G-87/ledger, so
    a `#[cfg(test)] mod` call inside the definition host (bridge.rs) never
    counts as a chain that needs an anchor."""
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    out: list[tuple[int, str]] = []
    in_test_tail = False
    for idx, line in enumerate(lines):
        if not in_test_tail and CFG_TEST_RE.match(line):
            if _next_line_is_module(lines, idx):
                in_test_tail = True
            continue
        if in_test_tail:
            continue
        out.append((idx, line))
    return out


def evaluate(crates_root: Path, freeze: dict[str, str]) -> list[dict]:
    findings: list[dict] = []
    anchored_files: set[str] = set()

    for key, state in sorted(freeze.items()):
        file_part, fn = split_key(key)
        anchored_files.add(file_part)
        p = crates_root / file_part
        try:
            text = p.read_text(encoding="utf-8", errors="replace")
        except OSError:
            findings.append({"kind": "ANCHOR_STALE", "key": key, "detail": f"file missing: {file_part}"})
            continue
        if not re.search(rf"\bfn\s+{re.escape(fn)}\b", text):
            findings.append({"kind": "ANCHOR_STALE", "key": key,
                             "detail": f"fn {fn} not defined in {file_part} anymore"})
        if state == "GUARDED":
            if not GUARD_RE.search(text):
                findings.append({"kind": "GUARD_MISSING", "key": key,
                                 "detail": f"{file_part} lost block_in_place( -- forced worker-yield mode broken"})
            if not FLAVOR_RE.search(text):
                findings.append({"kind": "GUARD_MISSING", "key": key,
                                 "detail": f"{file_part} lost try_current flavor branch (current_thread would panic"})
        elif state == "NO_ASYNC_BOUNDARY":
            if ASYNC_FN_RE.search(text):
                findings.append({"kind": "ASYNC_APPEARED", "key": key,
                                 "detail": f"{file_part} gained async fn -- re-adjudicate to GUARDED"})

    # 4. every production call-site file must be accounted by some frozen anchor.
    # Comment/doc lines are skipped first -- the ledger's own lesson: bridge.rs and
    # mod.rs carry `bridge().spawn_compute_batch(` inside doc examples, and a text
    # scan without comment filtering would flag the definition host itself.
    for path in sorted(crates_root.rglob("*.rs")):
        try:
            rel = path.relative_to(crates_root).as_posix()
        except ValueError:
            continue
        parts = Path(rel).parts
        if "target" in path.parts or len(parts) < 3 or parts[1] != "src":
            continue
        if rel in anchored_files:
            continue
        code_lines = [
            l for _i, l in _prod_lines(path) if not l.lstrip().startswith(("//", "/*"))
        ]
        if any(CALL_SITE_RE.search(l) for l in code_lines):
            findings.append({"kind": "FILE_UNACCOUNTED", "key": rel,
                             "detail": "calls spawn_compute_batch but no G-88 anchor row lives in it"})
    return findings


def selftest(tmp_root: Path) -> int:
    def write(rel: str, body: str) -> None:
        p = tmp_root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(body, encoding="utf-8")

    guarded = (
        "pub fn offload_head(items: usize) -> usize {\n"
        "    match tokio::runtime::Handle::try_current() { _h => {}, }\n"
        "    tokio::task::block_in_place(|| items)\n"
        "}\n"
        "fn h() { let _ = bridge().spawn_compute_batch(x); }\n"
    )
    write("alpha/src/parallel.rs", guarded)
    write("beta/src/lib.rs", "pub fn plain(n: usize) -> usize { let _ = bridge().spawn_compute_batch(n); n }\n")

    problems: list[str] = []
    ok = {
        "alpha/src/parallel.rs::offload_head": "GUARDED",
        "beta/src/lib.rs::plain": "NO_ASYNC_BOUNDARY",
    }
    if evaluate(tmp_root, ok):
        problems.append(f"t1 coherent baseline must be green: {evaluate(tmp_root, ok)}")

    write("alpha/src/parallel.rs", guarded.replace("tokio::task::block_in_place(|| items)", "items"))
    if not any(f["kind"] == "GUARD_MISSING" for f in evaluate(tmp_root, ok)):
        problems.append("t2 removed block_in_place must be caught")
    write("alpha/src/parallel.rs", guarded)

    write("beta/src/lib.rs", "pub async fn plain(n: usize) -> usize { n }\nfn h2() { let _ = bridge().spawn_compute_batch(1); }\n")
    if not any(f["kind"] == "ASYNC_APPEARED" for f in evaluate(tmp_root, ok)):
        problems.append("t3 async appearing around NO_ASYNC row must be caught")
    write("beta/src/lib.rs", "pub fn plain(n: usize) -> usize { let _ = bridge().spawn_compute_batch(n); n }\n")

    write("alpha/src/parallel.rs", guarded.replace("fn offload_head", "fn renamed_head"))
    if not any(f["kind"] == "ANCHOR_STALE" for f in evaluate(tmp_root, ok)):
        problems.append("t4 renamed anchor must be caught")
    write("alpha/src/parallel.rs", guarded)

    write("gamma/src/lib.rs", "fn g() { let _ = bridge().spawn_compute_batch(2); }\n")
    if not any(f["kind"] == "FILE_UNACCOUNTED" for f in evaluate(tmp_root, ok)):
        problems.append("t5 new unaccounted call-site file must be caught")
    (tmp_root / "gamma").rename(tmp_root / "zz_gamma_removed")

    if problems:
        for p in problems:
            print(f"FAIL  {p}")
        print("SELFTEST: FAILED")
        return 1
    print("SELFTEST: PASSED (guard/anchor/async-boundary/accounting, both directions)")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default="crates")
    parser.add_argument("--freeze", default="scripts/sync_offload_guard_freeze.txt")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args(argv)

    if args.selftest:
        import tempfile
        with tempfile.TemporaryDirectory(prefix="g88_st_") as td:
            return selftest(Path(td))

    crates_root = Path(args.root)
    if not crates_root.is_dir():
        print(f"FAIL  crates root not found: {crates_root}")
        return 2
    try:
        freeze = load_freeze(Path(args.freeze))
    except (OSError, ValueError) as exc:
        print(f"FAIL  cannot load freeze: {exc}")
        return 2

    findings = evaluate(crates_root, freeze)
    if args.json:
        print(json.dumps({"fail": findings}, indent=2))
    else:
        print("== sync-offload worker-yield guard (G-88, forced mode of decision 3) ==")
        for key, state in sorted(freeze.items()):
            bad = {f["kind"] for f in findings if f["key"] == key}
            mark = "OK  " if not bad else f"{','.join(sorted(bad))[:12]}"
            print(f"  {mark} {key:<70} state={state}")
        for f in findings:
            if f["key"] not in freeze:
                print(f"  [{f['kind']}] {f['key']} -- {f['detail']}")
        if findings:
            print("\n  The forced sync-offload guard mode broke somewhere. Restore the flavor\n"
                  "  branch (block_in_place + try_current, see faae parallel.rs doc) or\n"
                  "  re-adjudicate the row knowingly -- never let an async chain call the\n"
                  "  sync batch off a worker. LocalSet caveat: script header.")
            return 1
        print(f"\n  RESULT: OK ({len(freeze)} anchors live)")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
