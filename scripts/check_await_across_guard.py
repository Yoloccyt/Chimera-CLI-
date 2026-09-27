#!/usr/bin/env python3
# =============================================================================
# check_await_across_guard.py - "lock held across .await" gate (arch direction 2)
# =============================================================================
# Purpose: mechanically block the async concurrency debt class where a lock
#          guard (std/parking_lot/tokio Mutex, RwLock, DashMap RefMut) is still
#          ALIVE at an `.await` point inside the same async fn.
#
#          Holding a guard across .await => (1) other tasks queue on the same
#          lock, (2) tokio worker starvation, (3) possible deadlock.
#
# WHY a bespoke scanner instead of clippy::await_holding_lock:
#   clippy's lint covers std::sync/parking_lot guards but NOT `DashMap::RefMut`,
#   which is exactly what bit us in nexus-app-server (see below). It also cannot
#   express "this guard lives inside a spawn_blocking closure, so it is fine".
#
# FALSE-POSITIVE CLASSES THIS SCANNER MUST NOT REPORT (all three were real
# false positives in the first-pass audit of 2026-09-10):
#   F1. `.lock().await` / `.write().await` / `.read().await` - the await merely
#       ACQUIRES the lock; it does not hold it.
#   F2. guard acquired INSIDE a `spawn_blocking(...)` closure - the closure body
#       is synchronous, the guard drops when the closure returns, and the outer
#       `.await` is on the JoinHandle. This is the RECOMMENDED fix for sync IO
#       (rusqlite), not a defect.
#   F3. guard released by block scope (`{ let g = ...; }` then `.await` after the
#       closing brace) or by an explicit `drop(g)`.
#
# Allowlist: guards that MUST stay held across .await, or that the scanner
#   mis-attributes (a value taken OUT of a lock, e.g. `*guard` / `.take()`).
#   Keyed by the ACQUISITION line, so one entry covers every await under it.
#   A stale entry (one that no longer matches any finding) FAILS the gate -
#   an allowlist that can rot is worse than no allowlist. See
#   scripts/await_guard_allowlist.txt for the per-entry rationale.
#
# Cross-table parity (finding F20): the SAME 11 exemptions are registered twice
#   - here, and in scripts/lock_await_freeze.txt for the bash sibling gate
#   (check_lock_await.sh). Both files DECLARE the 1:1 invariant in their own
#   headers (freeze file line 8-9; gate_manifest G-35 note), but until this
#   check existed nothing ever VERIFIED it, so a one-line code shift required
#   two hand edits and forgetting one stayed invisible (measured 2026-09-20:
#   re-keying pipeline.rs 891->892 meant editing both files).
#   Parity compares KEYS ONLY: the two files use different class labels for the
#   same entry ("SCANNER" here vs "SCANNER-DEBT" there) and different wording,
#   which is fine; a divergent KEY SET is the failure mode.
#   This gate is already wired into CI (ci.yml) and the manifest (G-26/G-27),
#   so parity has teeth on both sides without touching any workflow.
#
# Encoding: all-ASCII (project script convention, avoids CJK locale issues in CI)
# Exit code: 0 = clean, 1 = gap found, 2 = usage error / parity undecidable
# Usage:
#   python scripts/check_await_across_guard.py
#   python scripts/check_await_across_guard.py --selftest
#   python scripts/check_await_across_guard.py --include-tests
# =============================================================================
import argparse
import os
import re
import shutil
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATES = os.path.join(ROOT, "crates")
ALLOWLIST = os.path.join(ROOT, "scripts", "await_guard_allowlist.txt")
# Bash sibling gate's copy of the same exemptions (see header, finding F20).
FREEZE_TABLE = os.path.join(ROOT, "scripts", "lock_await_freeze.txt")

# Guard acquisition: a `let` binding whose initializer takes a lock.
LET_BIND = re.compile(r"\blet\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=")
LOCK_CALL = (
    ".lock(",
    ".write(",
    ".read(",
    ".get_mut(",
    ".entry(",
    ".try_lock(",
    ".try_write(",
)
# An await that only ACQUIRES the lock (F1) - never a holding violation.
ACQUIRE_AWAIT = re.compile(r"\.(?:lock|write|read|try_lock|try_write)\(\s*\)\.await")
AWAIT = re.compile(r"\.await\b")
DROP_CALL = re.compile(r"\bdrop\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)\s*\)")
SYNC_CLOSURE = "spawn_blocking"
TEST_MARKER = re.compile(r"#\[cfg\(test\)\]")


def strip_noise(line: str) -> str:
    """Remove string literals and line comments so braces/parens are real."""
    out = []
    in_str = False
    i = 0
    while i < len(line):
        c = line[i]
        if in_str:
            if c == "\\":
                i += 2
                continue
            if c == '"':
                in_str = False
            i += 1
            continue
        if c == '"':
            in_str = True
            i += 1
            continue
        if c == "/" and i + 1 < len(line) and line[i + 1] == "/":
            break
        out.append(c)
        i += 1
    return "".join(out)


class Guard:
    __slots__ = ("name", "line", "depth", "in_sync_closure")

    def __init__(self, name, line, depth, in_sync_closure):
        self.name = name
        self.line = line
        self.depth = depth
        self.in_sync_closure = in_sync_closure


def scan_text(text, path):
    """Return list of (path, await_line, guard_name, guard_line)."""
    global INCLUDE_TESTS
    gaps = []
    lines = text.split("\n")
    depth = 0
    guards = []
    sync_depths = []  # depths at which a spawn_blocking( closure opened
    pending_let = None  # let-binding whose initializer is not yet terminated by ';'
    in_test = False

    for lineno, raw in enumerate(lines, 1):
        if TEST_MARKER.search(raw):
            in_test = True
        code = strip_noise(raw)
        depth_at_line = depth

        if in_test and not INCLUDE_TESTS:
            depth += code.count("{") - code.count("}")
            continue

        # --- 1. await while a guard is still alive (F1/F2 filtered) ---
        if AWAIT.search(code) and not ACQUIRE_AWAIT.search(code):
            for g in guards:
                if not g.in_sync_closure:
                    gaps.append((path, lineno, g.name, g.line))

        # --- 2. explicit drop(g) releases that guard (F3) ---
        m = DROP_CALL.search(code)
        if m:
            guards = [g for g in guards if g.name != m.group(1)]

        # --- 3. open a synchronous spawn_blocking context? (F2) ---
        if SYNC_CLOSURE in code:
            sync_depths.append(depth_at_line)

        # --- 4. attribute lock calls to the correct let-binding, in source order
        # WHY position-ordered: `let v = { let g = lock.lock()...; *g };` must
        # attribute .lock() to `g` (depth+1, released at the '}' on the same
        # line), never to `v`. A naive "first let on the line" mis-attributes it
        # and reports `v` as held across the following await (false positive).
        events = [(m.start(), 0, m.group(1)) for m in LET_BIND.finditer(code)]
        lock_pos = min([code.index(t) for t in LOCK_CALL if t in code], default=None)
        if lock_pos is not None:
            events.append((lock_pos, 1, None))
        for i, c in enumerate(code):
            if c == ";":
                events.append((i, 2, None))
        events.sort(key=lambda e: e[0])

        for pos, kind, name in events:
            if kind == 0:
                pending_let = name
            elif kind == 1:
                if pending_let:
                    inner = depth_at_line + code[:pos].count("{") - code[:pos].count("}")
                    guards.append(
                        Guard(pending_let, lineno, inner, bool(sync_depths))
                    )
            else:
                pending_let = None

        # --- 5. advance depth, then drop what the closed scopes released (F3) ---
        depth += code.count("{") - code.count("}")
        guards = [g for g in guards if depth >= g.depth]
        sync_depths = [d for d in sync_depths if depth > d]

    return gaps


# ---------------------------------------------------------------- rule 2
# Same-map reentrancy: a held DashMap ref + a mutation on the SAME map.
#
# WHY this second rule lives in this file: the gate is the repo's lock-discipline
# scanner and is ALREADY wired into CI (ci.yml + G-26/G-27), so extending it adds
# no unwired-gate debt. The defect itself is not await-related -- DashMap shard
# write locks are NON-REENTRANT, so calling remove/insert on the same map while a
# get/get_mut/iter ref is still alive self-deadlocks the caller. `cargo check`
# cannot see it (not a type error) and rule 1 cannot see it (no .await boundary).
#
# WHY typed evidence, not a name guess: the receiver must be declared as a
# DashMap (field/let type or DashMap::new()), otherwise BTreeMap/HashMap code
# that legally mutates while holding a ref would go red.
#
# WHY there is NO allowlist for rule 2: an entry in await_guard_allowlist.txt is
# compared 1:1 against scripts/lock_await_freeze.txt (finding F20), which is the
# bash sibling gate's table for HELD-ACROSS-AWAIT exemptions -- a class that gate
# does not detect at all. Registering here would either lie in the other table or
# make it stale-red. The correct resolution of a reentrancy finding is the fix
# (block scope / drop(g)), never an exemption, so this rule is zero-tolerance.
DASHMAP_DECL = re.compile(
    r"(?:^|[\s,(])([a-z_][a-z0-9_]*)\s*:\s*(?:mut\s+)?&?\s*(?:std::)?(?:sync::)?"
    r"(?:&?\s*)?(?:Arc<\s*)?&?\s*DashMap\s*<")
DASHMAP_NEW = re.compile(
    r"let\s+([a-z_][a-z0-9_]*)\s*(?::[^=]+)?=\s*(?:\s*Arc::new\s*\(\s*)?DashMap\s*::\s*new\s*\(")
# Group 1/2 = the binding that keeps the guard alive (an `if let Some(..)` arm or a
# `let`), group 3 = receiver, group 4 = acquisition method.
REENT_ACQUIRE = re.compile(
    r"(?:if\s+let\s+Some\s*\(\s*(?:ref\s+)?(?:mut\s+)?([a-z_][a-z0-9_]*)\s*\)\s*=\s*"
    r"|let\s+(?:ref\s+)?(?:mut\s+)?([a-z_][a-z0-9_]*)\s*=\s*)?"
    r"([a-z_][a-z0-9_]*)\s*\.\s*(get_mut|get|iter_mut|iter)\s*\(")
REENT_MUTATE = re.compile(
    r"([a-z_][a-z0-9_]*)\s*\.\s*(remove|insert|remove_if|retain|clear|entry)\s*\(")


def dashmap_receivers(text):
    """Receivers with typed DashMap evidence in this file."""
    return set(DASHMAP_DECL.findall(text)) | set(DASHMAP_NEW.findall(text))


def scan_reentrancy_text(text, path):
    """Return [(path, acquire_line, mutate_line, recv, acquire_method, mutate_method)]."""
    names = dashmap_receivers(text)
    if not names:
        return []
    code = [strip_noise(l) for l in text.split("\n")]
    out = []
    for i, line in enumerate(code):
        m = REENT_ACQUIRE.search(line)
        if not m or m.group(3) not in names:
            continue
        recv, binding = m.group(3), (m.group(1) or m.group(2))
        if not binding:
            # No name to track => the guard's lifetime cannot be modelled (e.g.
            # `let v = m.get(k).map(|x| *x)` releases at the semicolon). Skipping
            # keeps the rule precise instead of guessing.
            continue
        depth = line.count("{") - line.count("}")
        if depth <= 0:
            depth = 1  # the guard dies with this statement's scope at the latest
        j = i + 1
        while j < len(code) and depth > 0:
            d = DROP_CALL.search(code[j])
            if d and d.group(1) == binding:
                break  # F3 equivalent: guard explicitly released before the call
            d = REENT_MUTATE.search(code[j])
            if d and d.group(1) == recv:
                out.append((path, i + 1, j + 1, recv, m.group(4), d.group(2)))
                break
            depth += code[j].count("{") - code[j].count("}")
            j += 1
    return out


def load_key_table(path):
    """Parse a `<path>:<guard-acquisition-line>  # reason` exemption table.

    Returns (keys, malformed):
      keys      - set of (posix_relpath, lineno), or None if the file is absent
      malformed - non-comment data lines that did not yield a key

    WHY `malformed` is not swallowed: the pre-2026-09-21 parser `continue`d past
    any line it could not turn into a key. On a single-table suppression check
    that only loses an exemption (fail-closed: the finding then goes red). Under
    parity it is a false-green channel - a line dropped from either side can
    make two disagreeing tables look identical.
    """
    if not os.path.exists(path):
        return None, ["file not found"]
    keys = set()
    malformed = []
    with open(path, encoding="utf-8", errors="ignore") as fh:
        for raw in fh:
            # Drop the inline `# reason` BEFORE splitting on ":" so a rationale
            # that quotes another location cannot become the key.
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            if ":" not in line:
                malformed.append(line)
                continue
            entry_path, _, lineno = line.rpartition(":")
            try:
                keys.add((entry_path.strip().replace("\\", "/").lstrip("./"), int(lineno)))
            except ValueError:
                malformed.append(line)
    return keys, malformed


def load_allowlist():
    """Suppression set for this gate (missing file => no suppression)."""
    keys, _ = load_key_table(ALLOWLIST)
    return keys if keys is not None else set()


def rel(p):
    return os.path.relpath(p, ROOT).replace("\\", "/")


def table_parity(allow_path, freeze_path):
    """Compare the two await-exemption tables on their KEY sets (finding F20).

    Returns a dict:
      verdict  'ok'          both readable, key sets identical
               'gap'         key sets differ, or a data line failed to parse
               'undecidable' a table is missing (cannot assert parity either way)
      only_allow / only_freeze  sorted 'path:line' lists (empty on a clean run)
      detail   malformed / missing-file reasons
      counts   (n_allow, n_freeze)

    Reason text and class labels are deliberately NOT compared: the two files
    word the same entry differently ("SCANNER" vs "SCANNER-DEBT"), and forcing
    prose equality would buy nothing while making every re-key a two-file edit
    again - which is the cost this check exists to remove.
    """
    allow, bad_a = load_key_table(allow_path)
    freeze, bad_f = load_key_table(freeze_path)
    detail = ["%s: %s" % (rel(allow_path), b) for b in bad_a]
    detail += ["%s: %s" % (rel(freeze_path), b) for b in bad_f]
    if allow is None or freeze is None:
        return {"verdict": "undecidable", "only_allow": [], "only_freeze": [],
                "detail": detail, "counts": (len(allow or ()), len(freeze or ()))}
    if detail:
        return {"verdict": "gap", "only_allow": [], "only_freeze": [],
                "detail": detail, "counts": (len(allow), len(freeze))}
    only_a = sorted("%s:%d" % k for k in (allow - freeze))
    only_f = sorted("%s:%d" % k for k in (freeze - allow))
    return {"verdict": "ok" if not (only_a or only_f) else "gap",
            "only_allow": only_a, "only_freeze": only_f, "detail": [],
            "counts": (len(allow), len(freeze))}


def scan_tree():
    gaps = []
    for crate in sorted(os.listdir(CRATES)):
        src = os.path.join(CRATES, crate, "src")
        if not os.path.isdir(src):
            continue
        for dirpath, _, files in os.walk(src):
            for f in sorted(files):
                if not f.endswith(".rs"):
                    continue
                p = os.path.join(dirpath, f)
                with open(p, encoding="utf-8", errors="ignore") as fh:
                    gaps.extend(scan_text(fh.read(), os.path.relpath(p, ROOT)))
    return gaps


def scan_reentrancy_tree():
    """Rule 2 over the same file set (one read per file, shared with rule 1's scope)."""
    found = []
    for crate in sorted(os.listdir(CRATES)):
        src = os.path.join(CRATES, crate, "src")
        if not os.path.isdir(src):
            continue
        for dirpath, _, files in os.walk(src):
            for f in sorted(files):
                if not f.endswith(".rs"):
                    continue
                p = os.path.join(dirpath, f)
                with open(p, encoding="utf-8", errors="ignore") as fh:
                    # Forward slashes: the printed pointer must stay greppable
                    # on every host (same shape as the allowlist keys).
                    found.extend(scan_reentrancy_text(fh.read(), rel(p)))
    return found


# ---------------------------------------------------------------- self-test
SELFTEST_SRC = """
async fn bad(lock: &Mutex<u8>) {
    let g = lock.lock().unwrap();
    do_io().await;
    drop(g);
}

async fn good_block(lock: &Mutex<u8>) {
    let v = { let g = lock.lock().unwrap(); *g };
    do_io().await;
    let _ = v;
}

async fn good_spawn(lock: Arc<Mutex<u8>>) {
    let c = Arc::clone(&lock);
    tokio::task::spawn_blocking(move || {
        let g = c.lock().unwrap();
        *g
    }).await.unwrap();
}

async fn good_acquire(lock: &tokio::sync::Mutex<u8>) {
    let g = lock.lock().await;
    let _ = *g;
}
"""


def scanner_selftest():
    global INCLUDE_TESTS
    INCLUDE_TESTS = False
    gaps = scan_text(SELFTEST_SRC, "<selftest>")
    bad = [g for g in gaps if "bad" in str(g[1])]
    ok = True
    if len(gaps) != 1:
        print("[SELFTEST] expected exactly 1 violation, got %d: %s" % (len(gaps), gaps))
        ok = False
    elif gaps[0][1] != 4:
        print("[SELFTEST] violation should be reported at line 4, got %s" % (gaps[0],))
        ok = False
    if not ok:
        return 1
    print("[SELFTEST] all constructed cases classified correctly:")
    print("           1 violation (bad) / 3 false positives suppressed")
    print("           (block-scoped, spawn_blocking closure, acquire-only await)")
    return 0


REENT_SELFTEST_SRC = """
struct S { chains: Arc<DashMap<String, u32>> }

fn bad_if_let(m: &DashMap<String, u32>) {
    if let Some(mut v) = m.get_mut("k") {
        *v += 1;
        m.remove("k");
    }
}

fn bad_let_then_insert(m: &DashMap<String, u32>) {
    let g = m.get("k");
    m.insert("j", 1);
    drop(g);
}

fn good_explicit_drop(m: &DashMap<String, u32>) {
    let g = m.get("k");
    drop(g);
    m.remove("k");
}

fn good_block_scope(m: &DashMap<String, u32>) {
    {
        let mut g = m.get_mut("k");
        *g += 1;
    }
    m.remove("k");
}

fn good_receiver_not_dashmap(store: &BTreeMap<String, u32>) {
    if let Some(v) = store.get("k") {
        store.remove("k");
    }
}

fn good_no_acquisition(m: &DashMap<String, u32>) {
    let n = m.len();
    m.remove("k");
    let _ = n;
}
"""


def reentrancy_selftest():
    """Rule-2 fixtures: 2 planted positives + 4 shapes that MUST stay quiet.

    WHY the negatives are as important as the positives: the whole reason this
    class had no guard is that nothing observable distinguishes "0 findings" from
    "detector disabled". A green run is only meaningful while the two planted
    sites still light up, and the drop()/block-scope forms -- which are the
    RECOMMENDED fixes -- must not be punished into an incentive to delete them.
    """
    found = scan_reentrancy_text(REENT_SELFTEST_SRC, "<selftest>")
    pairs = {(f[1], f[2]) for f in found}  # (acquire_line, mutate_line)
    cases = [
        # Planted positives: the detector MUST light up on both guard shapes.
        ("planted-positive-if-let-arm", (5, 7) in pairs),
        ("planted-positive-let-binding", (12, 13) in pairs),
        # The four recommended / legal forms must stay quiet.
        ("explicit-drop-is-clean", not any(a == 18 for a, _ in pairs)),
        ("block-scope-is-clean", not any(a == 25 for a, _ in pairs)),
        ("non-dashmap-receiver-is-clean", not any(a == 32 for a, _ in pairs)),
        ("no-acquisition-is-clean", not any(a == 38 for a, _ in pairs)),
        ("no-ghost-findings", len(found) == 2),
    ]
    fails = 0
    for name, good in cases:
        fails += 0 if good else 1
        print("  [%s] reentry-%s" % ("ok" if good else "FAIL", name))
    if fails:
        print("         got: %s" % (found,))
    print("[SELFTEST] reentrancy fixtures: %d/%d passed"
          % (len(cases) - fails, len(cases)))
    return 1 if fails else 0


def parity_selftest():
    """Fixtures for the two-table parity check (finding F20).

    Two classes, in priority order:
      (a) injected drift must be reported   - otherwise parity is decoration;
      (b) the parser must not misread its own input - a `#` comment that happens
          to look like a key, or a reason that quotes another location, must not
          become data. Class (b) is the failure mode this repo has already been
          bitten by three times (check_gate_predicates_parity selftest-8/9/10).
    """
    base = tempfile.mkdtemp(prefix="awaitparity_")

    def table(name, text):
        p = os.path.join(base, name)
        with open(p, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(text)
        return p

    HEAD = "# gate header\n# Key: <path>:<GUARD ACQUISITION LINE>  (one entry covers all awaits)\n\n"
    A = HEAD + "crates/x/src/a.rs:10   # DESIGN holds the frame\n" \
               "crates/x/src/b.rs:20   # SCANNER copy out\n"
    B_SAME = HEAD + "crates/x/src/a.rs:10   # DESIGN wording differs on purpose\n" \
                    "crates/x/src/b.rs:20   # SCANNER-DEBT label too\n"
    COMMENT_KEYS = HEAD + "# crates/x/src/a.rs:99\n# re-keyed 10 -> 20 last week\n" \
                          "crates/x/src/a.rs:10   # DESIGN\n" \
                          "crates/x/src/b.rs:20   # SCANNER\n"
    REASON_QUOTES = HEAD + "crates/x/src/a.rs:10   # DESIGN was crates/x/src/a.rs:99 before\n" \
                           "crates/x/src/b.rs:20   # SCANNER\n"
    ALLOW_ONLY = A
    WITH_EXTRA = A + "crates/x/src/c.rs:30   # SCANNER extra\n"
    MALFORMED = A + "crates/x/src/d.rs:latest\n"
    NOFILE = os.path.join(base, "does-not-exist.txt")

    cases = [
        # (name, allow_path, freeze_path, expected verdict, extra predicate)
        ("clean-despite-different-prose",
         table("clean_a.txt", A), table("clean_b.txt", B_SAME), "ok",
         lambda r: r["counts"] == (2, 2)),
        ("comment-shaped-key-not-counted",
         table("cmt_a.txt", A), table("cmt_b.txt", COMMENT_KEYS), "ok",
         lambda r: r["counts"] == (2, 2)),
        ("key-inside-reason-not-picked-up",
         table("rsn_a.txt", A), table("rsn_b.txt", REASON_QUOTES), "ok",
         lambda r: not r["only_allow"] and not r["only_freeze"]),
        ("extra-entry-in-allowlist",
         table("d1_a.txt", WITH_EXTRA), table("d1_b.txt", ALLOW_ONLY), "gap",
         lambda r: r["only_allow"] == ["crates/x/src/c.rs:30"] and not r["only_freeze"]),
        ("extra-entry-in-freeze",
         table("d2_a.txt", ALLOW_ONLY), table("d2_b.txt", WITH_EXTRA), "gap",
         lambda r: r["only_freeze"] == ["crates/x/src/c.rs:30"] and not r["only_allow"]),
        ("unparseable-data-line-is-red",
         table("m_a.txt", A), table("m_b.txt", MALFORMED), "gap",
         lambda r: any("d.rs:latest" in d for d in r["detail"])),
        ("missing-table-is-undecidable-not-clean",
         table("g_a.txt", A), NOFILE, "undecidable",
         lambda r: r["counts"] == (2, 0)),
    ]

    fails = 0
    for name, pa, pb, want, extra in cases:
        res = table_parity(pa, pb)
        good = res["verdict"] == want and extra(res)
        fails += 0 if good else 1
        print("  [%s] parity-%s (verdict=%s want=%s allow=%d freeze=%d)"
              % ("PASS" if good else "FAIL", name, res["verdict"], want,
                 res["counts"][0], res["counts"][1]))
        if not good:
            print("         detail=%s only_allow=%s only_freeze=%s"
                  % (res["detail"], res["only_allow"], res["only_freeze"]))
    shutil.rmtree(base, ignore_errors=True)
    print("[SELFTEST] parity fixtures: %d/%d passed" % (len(cases) - fails, len(cases)))
    return 1 if fails else 0


def breadth_summary(per_key):
    """Fold "how many findings one exemption key absorbs" into one self-report line.

    WHY: an allowlist key is `<path>:<guard acquisition line>`, so a single entry
    covers EVERY await under that guard (finding F19 / open ask Q27). Before this,
    the gate only reported the total (e.g. 22 suppressed), which hides the shape
    that matters: 22 findings absorbed by 11 keys, widest key absorbing 3. That
    width is exactly what lets a newly added held-across-await slip in silently
    under an already-exempted lock. This is observation only - it changes no
    verdict, so the non-green set cannot move because of it.
    """
    keys = len(per_key)
    if keys == 0:
        return "0 finding(s) / 0 key(s), max 0"
    total = sum(per_key.values())
    # Ties break by key name so the line stays deterministic (assertable).
    (path, gline), hit = sorted(per_key.items(), key=lambda kv: (-kv[1], kv[0]))[0]
    return "%d finding(s) / %d key(s), max %d under %s:%d" % (total, keys, hit, path, gline)


def breadth_selftest():
    """The breadth line must be tested, incl. the F19/Q27 shape itself."""
    cases = [
        ("empty-table", {}, "0 finding(s) / 0 key(s), max 0"),
        ("one-key-one-finding", {("a.rs", 10): 1},
         "1 finding(s) / 1 key(s), max 1 under a.rs:10"),
        # The F19/Q27 shape: one exemption key absorbing two awaits. The gate is
        # green either way, so width>=2 is the only visible warning.
        ("one-key-absorbs-two-awaits", {("b.rs", 20): 2},
         "2 finding(s) / 1 key(s), max 2 under b.rs:20"),
        # Equal widths must resolve to the same key every run.
        ("tie-breaks-by-key-name", {("z.rs", 9): 3, ("a.rs", 1): 3},
         "6 finding(s) / 2 key(s), max 3 under a.rs:1"),
    ]
    fails = 0
    for name, counts, want in cases:
        got = breadth_summary(counts)
        good = got == want
        print("  [%s] breadth-%s (want=%r got=%r)"
              % ("ok" if good else "FAIL", name, want, got))
        if not good:
            fails += 1
    print("[SELFTEST] breadth fixtures: %d/%d passed"
          % (len(cases) - fails, len(cases)))
    return 1 if fails else 0


def selftest():
    ok = (scanner_selftest() == 0 and reentrancy_selftest() == 0
          and parity_selftest() == 0 and breadth_selftest() == 0)
    if not ok:
        print("[SELFTEST] RESULT: FAIL")
        return 1
    # Family names only: each family prints its own count above, so this line
    # cannot rot the way a hardcoded fixture count would.
    print("[SELFTEST] RESULT: PASS "
          "(scanner + reentrancy + parity + breadth fixtures)")
    return 0


INCLUDE_TESTS = False


def main():
    global INCLUDE_TESTS
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--include-tests", action="store_true")
    args = ap.parse_args()
    INCLUDE_TESTS = args.include_tests

    if args.selftest:
        return selftest()

    raw_gaps = scan_tree()
    reentry = scan_reentrancy_tree()
    allow = load_allowlist()
    used = set()

    def norm(p):
        return p.replace("\\", "/")

    gaps = []
    suppressed = 0
    per_key = {}
    for path, line, name, gline in raw_gaps:
        key = (norm(path), gline)
        if key in allow:
            used.add(key)
            suppressed += 1
            per_key[key] = per_key.get(key, 0) + 1
            continue
        gaps.append((path, line, name, gline))

    # Stale allowlist entries: the code moved on but the exemption did not.
    stale = sorted(allow - used)

    # F20: the bash sibling gate (check_lock_await.sh) registers the same
    # exemptions in its own file; verify the two key sets never diverge.
    parity = table_parity(ALLOWLIST, FREEZE_TABLE)

    # Printed unconditionally, i.e. on the red paths too (lesson F8: a gate whose
    # self-report only grows on the green path goes silent exactly when it needs
    # auditing). Observation only: no verdict reads this value.
    print("[INFO] exemption breadth: %s" % breadth_summary(per_key))
    # Rule 2 self-report, printed on every path (lesson F8). Zero tolerance: the
    # number here is the finding count, and there is no register to absorb it.
    print("[INFO] same-map reentrancy: %d site(s), zero tolerance / no register"
          % len(reentry))

    if (not gaps and not stale and not reentry
            and parity["verdict"] == "ok"):
        print("[OK] no lock guard held across .await (production code); "
              "%d finding(s) suppressed by allowlist, all still live" % suppressed)
        print("[OK] exemption tables in parity: %d key(s) shared by %s and %s"
              % (parity["counts"][0], rel(ALLOWLIST), rel(FREEZE_TABLE)))
        print("[OK] no DashMap guard held across a mutation of the same map")
        return 0

    if gaps:
        print("[FAIL] %d lock-held-across-await violation(s):" % len(gaps))
        for path, line, name, gline in gaps:
            print("  [GAP] %s:%d  guard `%s` acquired at line %d still held at .await"
                  % (path, line, name, gline))
        print("")
        print("Fix: release the guard before the await (block scope / drop(g) /")
        print("     move the sync work into spawn_blocking). See docs/reports/")
        print("     arch-refactor-directions-2026-09-10.md direction 2.")
    if reentry:
        print("[FAIL] %d same-map DashMap reentrancy site(s) (self-deadlock):"
              % len(reentry))
        for path, aline, mline, recv, amethod, mmeth in reentry:
            print("  [GAP] %s:%d  guard on `%s` (%s acquired at :%d) is still "
                  "held when `%s.%s()` runs"
                  % (path, mline, recv, amethod, aline, recv, mmeth))
        print("")
        print("Fix: end the guard's scope before touching the same map again --")
        print("     bind the inner result in a block, or drop(g) first. A correct")
        print("     in-repo example is csn-substitutor/src/lib.rs")
        print("     advance_degradation (block scope, then remove).")
        print("This rule has NO allowlist: the bash sibling gate does not detect")
        print("this class, so a key here would either misrepresent that table or")
        print("go stale. Fix it instead of exempting it.")
    if stale:
        print("[FAIL] %d stale allowlist entry/entries (no longer match any finding)"
              % len(stale))
        for p, n in stale:
            print("  [STALE] %s:%d" % (p, n))
        print("  Remove them from scripts/await_guard_allowlist.txt "
              "(the code moved on; the exemption must not outlive it).")
    if parity["verdict"] == "undecidable":
        print("[UNDECIDABLE] cannot assert exemption-table parity: %s"
              % "; ".join(parity["detail"]))
    elif parity["verdict"] == "gap":
        print("[FAIL] await-exemption tables out of parity "
              "(allowlist=%d, freeze=%d):" % parity["counts"])
        for k in parity["only_allow"]:
            print("  [PARITY] %s registered only in %s" % (k, rel(ALLOWLIST)))
        for k in parity["only_freeze"]:
            print("  [PARITY] %s registered only in %s" % (k, rel(FREEZE_TABLE)))
        for d in parity["detail"]:
            print("  [PARITY] unparseable data line: %s" % d)
        print("  Both files list the same exemptions for two different gates")
        print("  (check_await_across_guard.py / check_lock_await.sh). Add the")
        print("  missing line to the other table with the same rationale, or")
        print("  delete it from both once the hold is repaid.")

    if gaps or stale or reentry or parity["verdict"] == "gap":
        return 1
    return 2  # only the parity input was unreadable: nothing was decided


if __name__ == "__main__":
    import gate_rc  # entry-only: a crash must exit 2, never borrow 1 to fake "judged and red" (F32/F33)
    sys.exit(gate_rc.run(main))
