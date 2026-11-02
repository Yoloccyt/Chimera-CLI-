#!/usr/bin/env python3
"""check_compute_offload_reachability.py -- offload-entry reachability baseline is a machine-checked fact (G-87).

Why this script exists (round-3 audit F-U1; plan "caliber ruling"):
    "How many production-reachable parallel-offload chains does the CPU plane
    have?" was recorded as five mutually inconsistent numbers across four
    sources (ADR-196 note: 5; ADR-199 baseline: 7; dead-contract ledger:
    uses=8; external static audit: 2-4; C0 closeout: 0). The disagreement is a
    CALIBER dispute (crate-API liveness vs composition-root wiring), and no
    mechanism forced the two apart. This gate ends it the way the repo ends
    every such dispute: freeze the machine-computable facts, keep the semantic
    verdict as a human-evidenced field, FAIL on drift.

Verdict caliber (composition root wins):
    WIRED_COMPOSITION        -- chimera-cli / nexus-app-server src wires the chain
    CRATE_API_LIVE           -- non-test callers exist, no composition root
    PUB_API_DIAGNOSTIC_ONLY  -- only diagnostic harness callers (sentinel eval)
    DEAD                     -- no non-test callers at all (bench-only included)
    UNVERIFIED               -- fresh bless placeholder, must be adjudicated

Three mechanical checks (no regex symbol table is pretended anywhere -- the
v1 lesson: bare fn names collide across crates, e.g. score_parallel is defined
by BOTH faae and hcw; gsoe select_elite has a private same-name twin in
engine.rs, recorded in ADR-196 §1):
    1. call-site set drift   -- every `.spawn_compute_batch(` site in production
       scope maps to a frozen entry and vice versa (NEW_ENTRY / ENTRY_GONE).
    2. anchor existence      -- each frozen `path::symbol` evidence anchor still
       DEFINES that symbol (rename/delete => ANCHOR_STALE, baseline must not rot).
    3. dead-chain resurrection -- for DEAD entries the public api name is searched
       for qualified/imported call sites outside its defining files; any hit =>
       DEAD_PROMOTED (silent re-wiring needs human adjudication + bless).
    A chain whose verdict is non-DEAD with empty fingerprints would be
    self-contradictory; verdict-vs-evidence coherence is checked too.

Scanning rules are shared with check_dead_contract_apis.py on purpose:
    * only `crates/*/src/**` is production scope;
    * `#[cfg(test)]` opens the excluded tail ONLY before a `mod` declaration
      (attribute-on-item must not blind the scanner);
    * declaration lines never count as call sites;
    * `Self::name(` is attributed to the calling file's own same-name fn when
      that file defines it (twin-symbol exclusion).

Semantics:
    0  reality == frozen baseline
    1  drift (new/gone entry, stale anchor, silent promotion, malformed freeze)
    2  inputs unreadable (undecidable, never a silent pass)

Usage:
    python scripts/check_compute_offload_reachability.py [--root PATH]
        [--freeze PATH] [--json] [--selftest] [--update-baseline]
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path, PurePosixPath

CALL_SITE_RE = re.compile(r"\.spawn_compute_batch\(")
FN_DEF_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+|async\s+|unsafe\s+)*fn\s+(\w+)")
CFG_TEST_RE = re.compile(r"^\s*#\[cfg\(test\)\]")
MODULE_DECL_RE = re.compile(r"^\s*(pub(?:\([^)]*\))?\s+)?mod\s+\w")
ENTRY_SEP = "::"

COMPOSITION_ROOTS = {"chimera-cli", "nexus-app-server"}
VERDICTS = {
    "WIRED_COMPOSITION", "CRATE_API_LIVE", "PUB_API_DIAGNOSTIC_ONLY",
    "DEAD", "UNVERIFIED", "[Undecidable]",
}


def _next_code_line_is_module(lines: list[str], cfg_idx: int) -> bool:
    for later in lines[cfg_idx + 1:]:
        stripped = later.strip()
        if not stripped or stripped.startswith("#") or stripped.startswith("///") \
                or stripped.startswith("//!"):
            continue
        return bool(MODULE_DECL_RE.match(later))
    return False


def production_lines(path: Path) -> list[tuple[int, str]]:
    """File content, production scope only (test-tail rule shared with ledger)."""
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    out: list[tuple[int, str]] = []
    in_test_tail = False
    for idx, line in enumerate(lines):
        if not in_test_tail and CFG_TEST_RE.match(line):
            if _next_code_line_is_module(lines, idx):
                in_test_tail = True
            continue
        if in_test_tail:
            continue
        out.append((idx, line))
    return out


def iter_prod_files(crates_root: Path):
    for path in sorted(crates_root.rglob("*.rs")):
        try:
            rel = path.relative_to(crates_root)
        except ValueError:
            continue
        if "target" in path.parts:
            continue
        parts = rel.parts
        if len(parts) < 3 or parts[1] != "src":
            continue
        yield PurePosixPath(rel.as_posix()), path


def enclosing_fn(lines: list[tuple[int, str]], hit_pos: int) -> str | None:
    for i in range(hit_pos, -1, -1):
        m = FN_DEF_RE.match(lines[i][1])
        if m:
            return m.group(1)
    return None


def scan_call_sites(crates_root: Path) -> dict[str, dict]:
    """key = crate|src-relative::enclosing-fn for every `.spawn_compute_batch(` site."""
    out: dict[str, dict] = {}
    for rel, path in iter_prod_files(crates_root):
        lines = production_lines(path)
        for pos, (_idx, line) in enumerate(lines):
            if not CALL_SITE_RE.search(line):
                continue
            fn = enclosing_fn(lines, pos)
            if not fn:
                continue
            crate = rel.parts[0]
            in_crate = PurePosixPath(*rel.parts[1:]).as_posix()
            # key uses `/` between crate and src-path (no '|' -- pipe is the freeze
            # field separator; v1 lesson: a pipe inside keys broke the line schema)
            out[f"{crate}/{in_crate}{ENTRY_SEP}{fn}"] = {
                "crate": crate, "file": rel.as_posix(), "fn": fn,
            }
    return out


def _defines_symbol(path: Path, symbol: str) -> bool:
    pat = re.compile(rf"\bfn\s+{re.escape(symbol)}\b|\bstruct\s+{re.escape(symbol)}\b|\bimpl.*\b{re.escape(symbol)}\b")
    for _idx, line in production_lines(path):
        if pat.search(line):
            return True
    return False


def anchor_exists(crates_root: Path, anchor: str) -> bool:
    """anchor = 'some/dir/path.rs::symbol' (crate-root-relative like faae-router/src/router.rs::route)."""
    if ENTRY_SEP not in anchor:
        return False
    file_part, symbol = anchor.rsplit(ENTRY_SEP, 1)
    path = crates_root / file_part
    if not path.is_file():
        return False
    return _defines_symbol(path, symbol)


def _file_imports_api(text_lines: list[tuple[int, str]], pathseg: str, api: str) -> bool:
    use_re = re.compile(rf"\buse\s+[^;]*\b{re.escape(pathseg)}::[^;]*\b{re.escape(api)}\b")
    return any(use_re.search(line) for _i, line in text_lines)


def api_callers(crates_root: Path, api: str, defining_files: set[str]) -> list[str]:
    """Production files (outside defining files) with a call to `api` that is
    either module-qualified (`seg::api(`), imported-then-called (`use ... seg::api;`
    + `api(`), or `Self::`-free bare call in a file that does not define api itself.
    Twin-symbol exclusion mirrors the ledger's lessons."""
    call_re = re.compile(rf"(?<!fn )(?<!pub )(?<!async )(?<!unsafe )\b{re.escape(api)}\(")
    hits: set[str] = set()
    for rel, path in iter_prod_files(crates_root):
        rel_posix = rel.as_posix()
        if rel_posix in defining_files:
            continue
        lines = production_lines(path)
        defines_here = _defines_symbol(path, api)
        for _idx, line in lines:
            if not call_re.search(line):
                continue
            if re.search(rf"Self::{re.escape(api)}\(", line) and defines_here:
                continue
            # bare `api(` in a file that defines its own same-name fn is the twin trap
            if defines_here and not re.search(rf"::\s*{re.escape(api)}\(", line):
                continue
            hits.add(rel_posix)
    return sorted(hits)


def load_freeze(freeze_path: Path) -> dict[str, dict]:
    """row key = crate/src-path::fn; line = 4 pipe-fields key|verdict|anchors|api."""
    rows: dict[str, dict] = {}
    for raw in freeze_path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split("|")
        if len(parts) != 4:
            raise ValueError(f"malformed freeze line (want 4 '|'-fields): {raw!r}")
        key, verdict, anchors, api = parts
        if verdict not in VERDICTS:
            raise ValueError(f"unknown verdict {verdict!r} in: {raw!r}")
        rows[key] = {
            "verdict": verdict,
            "anchors": [a for a in anchors.split(";") if a],
            "api": api,
        }
    return rows


def evaluate(crates_root: Path, freeze: dict[str, dict]) -> list[dict]:
    findings: list[dict] = []
    live = scan_call_sites(crates_root)

    for key, entry in sorted(live.items()):
        if key not in freeze:
            findings.append({"kind": "NEW_ENTRY", "key": key,
                             "detail": "offload call site not in freeze -- adjudicate verdict + anchors, then --update-baseline"})
            continue
        row = freeze[key]
        # 2. anchors must still exist where the crate allows them to
        for anchor in row["anchors"]:
            if not anchor_exists(crates_root, anchor):
                findings.append({"kind": "ANCHOR_STALE", "key": key,
                                 "detail": f"anchor no longer defines the symbol: {anchor}"})
        # verdict vs evidence coherence
        if row["verdict"] in ("CRATE_API_LIVE", "PUB_API_DIAGNOSTIC_ONLY", "WIRED_COMPOSITION") and not row["anchors"]:
            findings.append({"kind": "VERDICT_EVIDENCE_GAP", "key": key,
                             "detail": f"verdict={row['verdict']} but zero anchors recorded"})
        if row["verdict"] == "WIRED_COMPOSITION":
            if not any(a.split("/")[0] in COMPOSITION_ROOTS for a in row["anchors"]):
                findings.append({"kind": "VERDICT_EVIDENCE_GAP", "key": key,
                                 "detail": "WIRED_COMPOSITION requires an anchor under a composition-root crate src/"})

    for key in sorted(freeze):
        if key not in live:
            findings.append({"kind": "ENTRY_GONE", "key": key,
                             "detail": "frozen entry has no `.spawn_compute_batch(` site in production scope anymore"})

    # 3. dead-chain resurrection (only rows with an api name; non-DEAD rows are
    #    coherence-checked by the ledger gate, not here)
    for key, row in sorted(freeze.items()):
        if row["verdict"] != "DEAD" or not row["api"]:
            continue
        entry_file = key.rsplit(ENTRY_SEP, 1)[0]  # crate/src/path.rs (== rel_posix)
        # only the entry's own file is excluded; files that merely DEFINE the api
        # stay in scope -- their bare same-name calls are dropped line-by-line
        # (twin rule) but qualified calls at the dead chain must still count (t9)
        defining = {entry_file}
        callers = api_callers(crates_root, row["api"], defining)
        if callers:
            findings.append({"kind": "DEAD_PROMOTED", "key": key,
                             "detail": f"verdict=DEAD but api {row['api']!r} now has non-test callers: {callers}"})
    return findings


def bless(crates_root: Path, freeze_path: Path) -> int:
    """Rewrite keys/anchors-preserving update: keeps existing verdict/anchors/api
    rows, adds NEW entries as UNVERIFIED, drops gone entries with a notice."""
    rows = load_freeze(freeze_path) if freeze_path.exists() else {}
    live = scan_call_sites(crates_root)
    out = [
        "# G-87 baseline: parallel-offload entry reachability (drift-detector freeze).",
        "# format: crate|src-path::enclosing-fn|verdict|anchors(';' path::sym list)|api(public chain head for DEAD resurrection check)",
        "# verdict caliber = COMPOSITION ROOT (chimera-cli / nexus-app-server) -- see script header.",
        "# Machine facts: call-site set, anchor existence, dead-chain resurrection. Human evidence: verdicts/anchors.",
        "# Ratchet: reality != freeze -> FAIL. Bless via --update-baseline THEN adjudicate every UNVERIFIED row.",
    ]
    for key in sorted(live):
        old = rows.get(key)
        if old:
            out.append(f"{key}|{old['verdict']}|{';'.join(old['anchors'])}|{old['api']}")
        else:
            out.append(f"{key}|UNVERIFIED||")
    if freeze_path.exists():
        for key in rows:
            if key not in live:
                out.append(f"# REMOVED(gone): {key}")
    freeze_path.write_text("\n".join(out) + "\n", encoding="utf-8")
    print(f"BLESSED {len(live)} entries (kept {sum(1 for k in live if k in rows)} adjudicated) -> {freeze_path}")
    return 0


def selftest(tmp_root: Path) -> int:
    """All three checks locked in both directions, plus twin-symbol exclusion."""
    def write(rel: str, body: str) -> None:
        p = tmp_root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(body, encoding="utf-8")

    entry = (
        "use nexus_core::compute::{bridge, TaskKind};\n"
        "pub fn offload_inner(n: usize) -> usize { n }\n"
        "fn run_offload(items: Vec<u32>) -> usize {\n"
        "    let _ = bridge().spawn_compute_batch(TaskKind::Generic, items.into_iter().map(|x| move || x));\n"
        "    1\n"
        "}\n"
    )
    write("alpha/src/parallel.rs", entry)
    write("alpha/src/engine.rs", "pub fn public_api() {} \npub fn parallel_batch() {}\n")
    write("beta/src/lib.rs", "pub fn caller() { let _ = alpha::parallel::run_offload(1); }\n")

    problems: list[str] = []
    live = scan_call_sites(tmp_root)
    key = "alpha/src/parallel.rs::run_offload"
    if key not in live:
        problems.append(f"t1 call site not discovered: {sorted(live)}")
        print("SELFTEST: FAILED\n" + "\n".join(f"FAIL  {p}" for p in problems))
        return 1

    freeze_ok = {key: {"verdict": "CRATE_API_LIVE",
                       "anchors": ["beta/src/lib.rs::caller"], "api": "run_offload"}}
    if evaluate(tmp_root, freeze_ok):
        problems.append(f"t2 coherent baseline must be green: {evaluate(tmp_root, freeze_ok)}")

    # new call site appears -> NEW_ENTRY
    write("gamma/src/lib.rs", "use nexus_core::compute::{bridge, TaskKind};\nfn g() { let _ = bridge().spawn_compute_batch(TaskKind::Generic, Vec::<fn() -> u32>::new()); }\n")
    if not any(f["kind"] == "NEW_ENTRY" for f in evaluate(tmp_root, freeze_ok)):
        problems.append("t3 new call site must be caught")
    (tmp_root / "gamma").rename(tmp_root / "gamma_removed")
    write("gamma_removed_note.txt", "")

    # stale anchor
    freeze_stale = {key: {"verdict": "CRATE_API_LIVE",
                          "anchors": ["beta/src/lib.rs::no_such_fn"], "api": "run_offload"}}
    if not any(f["kind"] == "ANCHOR_STALE" for f in evaluate(tmp_root, freeze_stale)):
        problems.append("t4 stale anchor must be caught")

    # verdict-without-evidence + composition without composition-root anchor
    bad_v = {key: {"verdict": "WIRED_COMPOSITION", "anchors": ["beta/src/lib.rs::caller"], "api": ""}}
    if not any(f["kind"] == "VERDICT_EVIDENCE_GAP" for f in evaluate(tmp_root, bad_v)):
        problems.append("t5 WIRED_COMPOSITION without composition-root anchor must be caught")

    # dead resurrection: DEAD row whose api gains a qualified caller
    # dead resurrection: DEAD rows whose api gains a qualified caller
    write("alpha/src/engine.rs",
          "use nexus_core::compute::{bridge, TaskKind};\n"
          "pub fn parallel_batch() -> usize { let _ = bridge().spawn_compute_batch(TaskKind::Generic, Vec::<fn() -> u32>::new()); 2 }\n")
    live2 = scan_call_sites(tmp_root)
    dkey = "alpha/src/engine.rs::parallel_batch"
    dead_freeze = {dkey: {"verdict": "DEAD", "anchors": [], "api": "parallel_batch"}}
    if any(f["kind"] == "DEAD_PROMOTED" for f in evaluate(tmp_root, dead_freeze)):
        problems.append("t6 no caller yet -> DEAD must stay green")
    write("beta/src/lib.rs", "pub fn c2() { let _ = alpha::engine::parallel_batch(); }\n")
    if not any(f["kind"] == "DEAD_PROMOTED" for f in evaluate(tmp_root, dead_freeze)):
        problems.append("t7 re-wired dead chain must be caught (silent resurrection)")

    # twin-symbol exclusion: beta defines its own parallel_batch and calls it bare
    write("beta/src/lib.rs",
          "pub fn parallel_batch() -> usize { 3 }\n"
          "pub fn c3() { let _ = parallel_batch(); }\n")
    promo = [f for f in evaluate(tmp_root, dead_freeze) if f["kind"] == "DEAD_PROMOTED"]
    if promo:
        problems.append(f"t8 bare Self twin call must not resurrect: {promo}")
    # qualified call from the twin file still counts (not hidden by the exclusion)
    write("beta/src/lib.rs",
          "pub fn parallel_batch() -> usize { 4 }\n"
          "pub fn c4() { let _ = alpha::engine::parallel_batch(); let _ = parallel_batch(); }\n")
    if not any(f["kind"] == "DEAD_PROMOTED" for f in evaluate(tmp_root, dead_freeze)):
        problems.append("t9 qualified call from twin-defining file must still count")

    if problems:
        for p in problems:
            print(f"FAIL  {p}")
        print("SELFTEST: FAILED")
        return 1
    print("SELFTEST: PASSED (call-site drift + anchor staleness + resurrection, both directions)")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default="crates")
    parser.add_argument("--freeze", default="scripts/compute_offload_reachability_freeze.txt")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--update-baseline", action="store_true")
    args = parser.parse_args(argv)

    if args.selftest:
        import tempfile
        with tempfile.TemporaryDirectory(prefix="g87_st_") as td:
            return selftest(Path(td))

    crates_root = Path(args.root)
    if not crates_root.is_dir():
        print(f"FAIL  crates root not found: {crates_root}")
        return 2
    freeze_path = Path(args.freeze)

    if args.update_baseline:
        return bless(crates_root, freeze_path)

    try:
        freeze = load_freeze(freeze_path)
    except (OSError, ValueError) as exc:
        print(f"FAIL  cannot load freeze {freeze_path}: {exc}")
        return 2

    findings = evaluate(crates_root, freeze)
    if args.json:
        print(json.dumps({"freeze": str(freeze_path), "fail": findings}, indent=2))
    else:
        print("== parallel-offload reachability baseline (G-87, round-3 F-U1 caliber ruling) ==")
        live = scan_call_sites(crates_root)
        for key in sorted(live):
            verdict = freeze.get(key, {}).get("verdict", "UNREGISTERED")
            print(f"  {'OK  ' if key in freeze else '??  '}{key:<62} verdict={verdict}")
        for f in findings:
            print(f"  [{f['kind']}] {f['key']} -- {f['detail']}")
        if findings:
            print("\n  Reality drifted from the frozen baseline. Adjudicate each line against\n"
                  "  the composition-root caliber (C0 closeout), update verdicts/anchors, then\n"
                  "  --update-baseline. Silent promotion/demotion of offload reachability is\n"
                  "  exactly what ADR-195 G-A's supporting argument must never be re-argued from.")
            return 1
        print(f"\n  RESULT: OK ({len(live)} entries, anchors live, dead chains still dead)")
    return 1 if findings else 0


if __name__ == "__main__":
    import gate_rc  # crash exits 2, never borrows 1 (F32/F33)
    sys.exit(gate_rc.run(main))
