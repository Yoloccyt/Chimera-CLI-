#!/usr/bin/env bash
# =============================================================================
# check_lock_await.sh - lock-guard-held-across-.await static gate (mission M3)
# =============================================================================
# Purpose: mechanically block the async concurrency debt class where a lock
#          guard (Mutex/RwLock/entry RefMut...) is still ALIVE at an `.await`
#          point inside the same async fn. Holding a guard across .await means
#          other tasks queue on the same lock -> tokio worker starvation ->
#          possible deadlock (project iron rule: no lock held across .await).
#
# WHY this gate exists (tower mission M3, roadmap direction 5 second half):
#   crates/faae-router/tests/lock_holding.rs (397 lines) is the ONLY runtime
#   regression detection for this debt class, and it covers faae-router only.
#   The known live violations in chimera-mas orchestrator / nexus-app-server
#   transport etc. had no mechanical guard. This gate scans crates/*/src
#   statically, workspace-wide, zero-cargo, seconds.
#
# Detection model (deliberately conservative - when in doubt, stay silent):
#   A `let <name> = ... .lock()/.read()/.write()/.entry()/.get_mut() ...`
#   binding marks <name> as a guard whose lifetime is tracked by brace depth.
#   A `.await` on a line where any guard is still alive (depth not yet closed)
#   is a finding. Explicitly NOT findings:
#     - acquire-only await   : `.lock().await` merely ACQUIRES the lock
#     - snapshot pattern     : guard taken inside `{ ... }`, block closes
#                              before the await (faae edsb.rs:296-310)
#     - spawn_blocking closure: closure body is sync; await is on JoinHandle
#     - explicit `drop(name)` or scope end releasing the guard first
#     - `#[cfg(test)]` modules (tests may stage pathological interleavings)
#
# Known live violations are FROZEN, not silently tolerated:
#   scripts/lock_await_freeze.txt lists `<path>:<guard-acquisition-line>`.
#   A finding matching a freeze entry is suppressed (entry marked live);
#   an unmatched finding FAILS; a stale entry (code moved on) FAILS too -
#   a freeze list that can rot is worse than no freeze list. Freeze is
#   shrink-only: fixing the code means deleting the line in the same PR.
#
# Usage:
#   bash scripts/check_lock_await.sh            # scan crates/*/src, judge
#   bash scripts/check_lock_await.sh --selftest # prove the gate has teeth
# Exit code: 0 = clean, 1 = finding/stale-entry, 2 = usage/environment error
#   (incl. missing freeze file or missing crates dir - never a silent pass;
#    pattern per scripts/audit_cmd_sync.py: missing target = "cannot decide")
# Encoding: all-ASCII (project script convention, Windows GBK consoles).
# =============================================================================
set -euo pipefail

# PATH may lack coreutils when launched from PowerShell/MSYS; harmless on Linux.
export PATH="/usr/bin:/bin:$PATH"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# Selftest redirects both roots at a fixture tree via these env overrides
# (resolved inside run_gate, not here - see run_gate):
#   CHECK_LOCK_AWAIT_ROOT     - scan root (default: repo root)
#   CHECK_LOCK_AWAIT_FREEZE   - freeze file (default: scripts/lock_await_freeze.txt)

# ---------------------------------------------------------------------------
# awk scanner: one file per invocation, findings on stdout as
#   <relpath>:<await_line>:guard `<name>` acquired <relpath>:<acq_line>
# deduped by acquisition line (first crossing await reported).
# POSIX awk only (no \b / \s / gensub): CI ubuntu runs mawk-compatible awk.
# ---------------------------------------------------------------------------
read -r -d '' AWK_SCANNER <<'AWK' || true
# lock tokens arrive as one |-joined string (portable across mawk/gawk).
BEGIN { nlock = split(locklist, locktok, "|") }

function nchars(s, re,   t) { t = s; return gsub(re, "", t) }

# Remove "//" comments and "..." string literals so braces/parens are real.
function strip_noise(s,   out, i, c, L, in_str) {
    out = ""; L = length(s); in_str = 0
    for (i = 1; i <= L; i++) {
        c = substr(s, i, 1)
        if (in_str) {
            if (c == "\\") { i++; continue }
            if (c == "\"") in_str = 0
            continue
        }
        if (c == "\"") { in_str = 1; continue }
        if (c == "/" && substr(s, i + 1, 1) == "/") break
        out = out c
    }
    return out
}

FNR == 1 {
    # repo-relative path of the current file (prefix passed via -v; the
    # index()/substr() dance avoids regex metacharacters in Windows paths).
    file = FILENAME
    if (index(file, prefix) == 1) file = substr(file, length(prefix) + 1)
    depth = 0; ng = 0; nsync = 0; in_test = 0; pend_name = ""
}

{
    raw = $0
    if (raw ~ /#\[cfg\(test\)\]/) in_test = 1
    code = strip_noise(raw)
    depth_before = depth
    openb  = nchars(code, "[{]")
    closeb = nchars(code, "[}]")

    # test-only code: count braces (scope integrity), skip guard logic
    if (in_test) { depth += openb - closeb; next }

    # --- 1. await while a guard is alive (acquire-only await excluded) ---
    if (code ~ /\.await([^A-Za-z0-9_]|$)/ \
        && code !~ /\.(lock|write|read|try_lock|try_write)[ \t]*\([ \t]*\)[ \t]*\.await/) {
        for (i = 1; i <= ng; i++) {
            if (gsync[i]) continue
            key = file ":" gline[i]
            if (key in seen) continue
            seen[key] = 1
            nfind++
            out[nfind] = file ":" FNR ":guard `" gname[i] "` acquired " file ":" gline[i]
        }
    }

    # --- 2. explicit drop(name) releases every guard with that name ---
    if (match(code, /(^|[^A-Za-z0-9_.])drop[ \t]*\([ \t]*[A-Za-z_][A-Za-z0-9_]*/)) {
        mstr = substr(code, RSTART, RLENGTH)
        match(mstr, /[A-Za-z_][A-Za-z0-9_]*[ \t]*$/)
        dropped = substr(mstr, RSTART, RLENGTH)
        gsub(/[ \t]+$/, "", dropped)
        j = 0
        for (i = 1; i <= ng; i++) {
            if (gname[i] == dropped) continue
            j++; gname[j] = gname[i]; gdepth[j] = gdepth[i]
            gsync[j] = gsync[i];      gline[j] = gline[i]
        }
        ng = j
    }

    # --- 3. spawn_blocking opens a sync context: guards inside it are safe ---
    if (index(code, "spawn_blocking")) { nsync++; sdepth[nsync] = depth_before }

    # --- 4. attribute a lock call to a pending let-binding -------------------
    # WHY position-aware: `let v = { let g = m.lock().unwrap(); *g };` must
    # bind the guard to `g` (block depth +1, dies at the same-line `}`), never
    # to `v` - attributing to the outer `v` would raise a false positive at
    # the next await below the block.
    # WHY carried-pending: `let g = map\n    .entry(k)\n    .or_insert(v);`
    # keeps the binding open across lines until the terminating `;`
    # (mlc-engine engine.rs migrate() is the real-world shape). pend_name
    # survives across lines, exactly like check_await_across_guard.py.
    lock_pos = 0
    for (t = 1; t <= nlock; t++) {
        p = index(code, locktok[t])
        if (p > 0 && (lock_pos == 0 || p < lock_pos)) lock_pos = p
    }
    if (lock_pos > 0) {
        semi_pos = index(code, ";")   # first ';' on the line (0 = none)
        pre  = substr(code, 1, lock_pos - 1)
        gdep = depth_before + nchars(pre, "[{]") - nchars(pre, "[}]")
        if (pend_name != "" && (semi_pos == 0 || semi_pos > lock_pos)) {
            ng++; gname[ng] = pend_name; gdepth[ng] = gdep
            gline[ng] = FNR;             gsync[ng] = (nsync > 0)
        } else {
            # same-line attribution: innermost let whose `=` precedes the lock
            rest = code; base = 0; best_eqend = 0; best_name = ""
            while (match(rest, /let[ \t]+(mut[ \t]+)?[A-Za-z_][A-Za-z0-9_]*[ \t]*=/)) {
                mstr  = substr(rest, RSTART, RLENGTH)
                eqend = base + RSTART + RLENGTH - 1
                match(mstr, /let[ \t]+(mut[ \t]+)?[A-Za-z_][A-Za-z0-9_]*/)
                nm = substr(mstr, RSTART, RLENGTH)
                sub(/^let[ \t]+/, "", nm); sub(/^mut[ \t]+/, "", nm)
                between = substr(code, eqend + 1, lock_pos - eqend - 1)
                if (eqend < lock_pos && index(between, ";") == 0) { best_eqend = eqend; best_name = nm }
                adv = RSTART + RLENGTH - 1
                base += adv
                rest = substr(rest, adv + 1)
            }
            if (best_eqend > 0) {
                ng++; gname[ng] = best_name; gdepth[ng] = gdep
                gline[ng] = FNR;             gsync[ng] = (nsync > 0)
            }
        }
    }

    # --- 4b. carry the pending let across lines (a ';' closes the statement) --
    last_eqend = 0; last_name = ""
    rest = code; base = 0
    while (match(rest, /let[ \t]+(mut[ \t]+)?[A-Za-z_][A-Za-z0-9_]*[ \t]*=/)) {
        mstr  = substr(rest, RSTART, RLENGTH)
        last_eqend = base + RSTART + RLENGTH - 1
        match(mstr, /let[ \t]+(mut[ \t]+)?[A-Za-z_][A-Za-z0-9_]*/)
        last_name = substr(mstr, RSTART, RLENGTH)
        sub(/^let[ \t]+/, "", last_name); sub(/^mut[ \t]+/, "", last_name)
        adv = RSTART + RLENGTH - 1
        base += adv
        rest = substr(rest, adv + 1)
    }
    last_semi = 0; tmp = code
    while ((sp = index(tmp, ";")) > 0) { last_semi += sp; tmp = substr(tmp, sp + 1) }
    if (last_eqend > 0 && (last_semi == 0 || last_semi < last_eqend)) pend_name = last_name
    else if (last_semi > 0) pend_name = ""

    # --- 5. close scopes: drop guards/sync contexts whose depth ended -------
    depth += openb - closeb
    j = 0
    for (i = 1; i <= ng; i++) {
        if (gdepth[i] > depth) continue
        j++; gname[j] = gname[i]; gdepth[j] = gdepth[i]
        gsync[j] = gsync[i];      gline[j] = gline[i]
    }
    ng = j
    while (nsync > 0 && depth <= sdepth[nsync]) nsync--
}

END { for (i = 1; i <= nfind; i++) print out[i] }
AWK

# Lock-acquisition tokens (DashMap entry/get_mut included: both yield guards).
# Field-tested list - matches scripts/check_await_across_guard.py (2026-09-10
# first-pass audit over the whole workspace produced no further token classes).
LOCK_TOKENS=(".lock(" ".write(" ".read(" ".try_lock(" ".try_write(" ".entry(" ".get_mut(")

# Run the scanner over every .rs under <scan-root>/crates/*/src.
# Prints deduped findings; returns 2 when the scan root does not exist.
# One awk process for all files (fork-per-file is minutes under Git Bash).
scan_tree() {
    local scan_root="$1"
    if [ ! -d "$scan_root/crates" ]; then
        echo "[FAIL] scan root has no crates/ directory: $scan_root" >&2
        return 2
    fi
    # Token list travels as ONE |-joined string; awk splits it in BEGIN
    # (-v 'arr[i]=v' is not portable across mawk/gawk).
    local locklist=""
    local t
    for t in "${LOCK_TOKENS[@]}"; do
        locklist="${locklist:+$locklist|}$t"
    done

    find "$scan_root/crates" -type d -name src \
        -exec find {} -type f -name '*.rs' \; 2>/dev/null | sort \
        | xargs -d '\n' -r awk -v "locklist=$locklist" \
                              -v "prefix=$scan_root/" "$AWK_SCANNER"
}

# Load freeze keys (<path>:<line>) from $1 into the global FREEZE_KEYS array.
load_freeze() {
    FREEZE_KEYS=()
    [ -f "$1" ] || return 1
    local line
    while IFS= read -r line; do
        case "$line" in
            ''|\#*) continue ;;
        esac
        line="${line%%#*}"           # strip trailing comment
        line="${line//[[:space:]]/}" # tolerate padding
        [ -n "$line" ] && FREEZE_KEYS+=("$line")
    done < "$1"
    return 0
}

# ---------------------------------------------------------------------------
# Gate: scan, then judge findings against the freeze list.
# Roots resolve HERE (not at script top) so the selftest can redirect them
# via CHECK_LOCK_AWAIT_ROOT / CHECK_LOCK_AWAIT_FREEZE env overrides.
# ---------------------------------------------------------------------------
run_gate() {
    local scan_root="${CHECK_LOCK_AWAIT_ROOT:-$root}"
    local freeze_path="${CHECK_LOCK_AWAIT_FREEZE:-scripts/lock_await_freeze.txt}"

    if [ ! -f "$freeze_path" ]; then
        echo "[FAIL] freeze file missing: $freeze_path" >&2
        echo "       (gate cannot decide - pattern per scripts/audit_cmd_sync.py)" >&2
        local raw f
        raw="$(scan_tree "$scan_root")" || return 2
        f="$(printf '%s\n' "$raw" | sed '/^$/d' | wc -l | tr -d ' ')"
        echo "       current findings ($f, freeze them only after manual review):" >&2
        printf '%s\n' "$raw" | sed '/^$/d' | sed 's/^/       /' >&2
        return 2
    fi

    local findings
    findings="$(scan_tree "$scan_root")" || return 2

    load_freeze "$freeze_path" || { echo "[FAIL] freeze file unreadable: $freeze_path" >&2; return 2; }

    # Partition findings into unfrozen vs frozen; mark freeze entries live.
    local -a unfrozen=()
    local live_list=""   # space-separated "path:line" of freeze keys seen live
    local acq k line
    while IFS= read -r line; do
        [ -z "$line" ] && continue
        acq="${line##* acquired }"
        k="$acq"
        if [[ " ${FREEZE_KEYS[*]-} " == *" $k "* ]]; then
            live_list="${live_list:+$live_list }$k"
        else
            unfrozen+=("$line")
        fi
    done <<< "$findings"

    # Stale freeze entries: registered, but no finding matches them anymore.
    local -a stale=()
    for k in "${FREEZE_KEYS[@]}"; do
        [[ " $live_list " == *" $k "* ]] || stale+=("$k")
    done

    if [ "${#unfrozen[@]}" -eq 0 ] && [ "${#stale[@]}" -eq 0 ]; then
        echo "[OK] crates/*/src scanned: no unfrozen lock-held-across-.await finding" \
             "(${#FREEZE_KEYS[@]} frozen exemption(s), all live)"
        return 0
    fi
    if [ "${#unfrozen[@]}" -gt 0 ]; then
        echo "[FAIL] ${#unfrozen[@]} unfrozen lock-held-across-.await finding(s):" >&2
        printf '  %s\n' "${unfrozen[@]}" >&2
        echo "Fix: release the guard before the await (block scope / drop(g) /" >&2
        echo "spawn_blocking). If the hold is a genuine DESIGN constraint, add the" >&2
        echo "acquisition line to $freeze_path with a reason (shrink-only ratchet)." >&2
    fi
    if [ "${#stale[@]}" -gt 0 ]; then
        echo "[FAIL] ${#stale[@]} stale freeze entry/entries (no current finding):" >&2
        printf '  %s\n' "${stale[@]}" >&2
        echo "The code moved on; delete the line(s) in the same PR." >&2
    fi
    return 1
}

# ---------------------------------------------------------------------------
# Selftest: fixture tree with positive + negative cases, then judge the judge.
# ---------------------------------------------------------------------------
selftest() {
    local fx
    fx="$(mktemp -d)"

    mkdir -p "$fx/crates/pos/src" "$fx/crates/neg/src" "$fx/scripts"

    # -- positive fixtures: every one MUST be reported ------------------------
    cat > "$fx/crates/pos/src/p1.rs" <<'RS'
async fn bad_plain(m: &std::sync::Mutex<u8>) {
    let g = m.lock().unwrap();
    do_io().await;
    drop(g);
}
RS
    cat > "$fx/crates/pos/src/p2.rs" <<'RS'
async fn bad_rwlock(m: &std::sync::RwLock<u8>) {
    let guard = m.write().unwrap();
    for _ in 0..3 {
        tick().await;
    }
}
RS
    cat > "$fx/crates/pos/src/p3.rs" <<'RS'
async fn bad_entry(map: &dashmap::DashMap<u8, u8>) {
    let mut e = map.entry(1);
    commit(&mut e).await;
}
RS
    # multi-line let: the binding stays open until the ';' two lines down
    # (mlc-engine engine.rs migrate() shape).
    cat > "$fx/crates/pos/src/p4.rs" <<'RS'
async fn bad_multiline(map: &dashmap::DashMap<u8, u8>) {
    let mut e = map
        .entry(2)
        .or_insert(0);
    commit(&mut e).await;
}
RS

    # -- negative fixtures: none may be reported ------------------------------
    # textbook snapshot pattern (faae-router edsb.rs:296-310): guard dies in
    # the block, the await below works on the cloned snapshot.
    cat > "$fx/crates/neg/src/n1.rs" <<'RS'
async fn good_snapshot(p: &Arc<RwLock<u8>>) {
    let snap = {
        let g = p.read().unwrap();
        *g
    };
    publish(snap).await;
}
RS
    # guard inside a spawn_blocking closure: the closure body is sync; the
    # outer .await is on the JoinHandle.
    cat > "$fx/crates/neg/src/n2.rs" <<'RS'
async fn good_blocking(m: Arc<std::sync::Mutex<u8>>) {
    let c = Arc::clone(&m);
    tokio::task::spawn_blocking(move || {
        let g = c.lock().unwrap();
        *g
    }).await.unwrap();
}
RS
    # acquire-only await + explicit release before the next await.
    cat > "$fx/crates/neg/src/n3.rs" <<'RS'
async fn good_acquire_then_release(m: &tokio::sync::Mutex<u8>) {
    let g = m.lock().await;
    let v = *g;
    drop(g);
    publish(v).await;
}
RS
    # value extracted out of the lock: the binding holds a copy, not a guard;
    # block scope releases the real guard on the same line.
    cat > "$fx/crates/neg/src/n4.rs" <<'RS'
async fn good_extracted(m: &std::sync::Mutex<u8>) {
    let v = { let g = m.lock().unwrap(); *g };
    publish(v).await;
}
RS
    # test-only code is allowed to stage pathological interleavings.
    cat > "$fx/crates/neg/src/n5.rs" <<'RS'
#[cfg(test)]
mod tests {
    async fn staged_deadlock(m: &std::sync::Mutex<u8>) {
        let g = m.lock().unwrap();
        do_io().await;
        drop(g);
    }
}
RS

    local fails=0

    # 1) detection: empty freeze, expect exactly the 3 positives.
    : > "$fx/scripts/lock_await_freeze.txt"
    local out rc
    out="$(CHECK_LOCK_AWAIT_ROOT="$fx" CHECK_LOCK_AWAIT_FREEZE="$fx/scripts/lock_await_freeze.txt" \
           run_gate 2>&1)" && rc=0 || rc=$?
    local npos
    npos="$(printf '%s\n' "$out" | grep -c 'guard `' || true)"
    if [ "$rc" -ne 1 ] || [ "$npos" -ne 4 ] \
       || ! printf '%s\n' "$out" | grep -q 'p1.rs:3:guard `g` acquired crates/pos/src/p1.rs:2' \
       || ! printf '%s\n' "$out" | grep -q 'p2.rs:4:guard `guard` acquired crates/pos/src/p2.rs:2' \
       || ! printf '%s\n' "$out" | grep -q 'p3.rs:3:guard `e` acquired crates/pos/src/p3.rs:2' \
       || ! printf '%s\n' "$out" | grep -q 'p4.rs:5:guard `e` acquired crates/pos/src/p4.rs:3' \
       || printf '%s\n' "$out" | grep -q 'neg/src/n[1-5]'; then
        echo "[SELFTEST] FAIL: expected exactly p1/p2/p3/p4 reported, got:" >&2
        printf '%s\n' "$out" >&2
        fails=1
    else
        echo "[SELFTEST] detection: 4/4 positive cases found, 5/5 negative cases silent"
    fi

    # 2) freeze suppression + stale detection: freeze p1 + a bogus entry.
    printf '%s\n' \
        'crates/pos/src/p1.rs:2 # DESIGN sync-std fixture' \
        'crates/pos/src/p1.rs:99 # bogus, code moved on' \
        > "$fx/scripts/lock_await_freeze.txt"
    out="$(CHECK_LOCK_AWAIT_ROOT="$fx" CHECK_LOCK_AWAIT_FREEZE="$fx/scripts/lock_await_freeze.txt" \
           run_gate 2>&1)" && rc=0 || rc=$?
    if [ "$rc" -ne 1 ] \
       || ! printf '%s\n' "$out" | grep -q 'crates/pos/src/p1.rs:99' \
       || printf '%s\n' "$out" | grep -q 'p1.rs:2:guard'; then
        echo "[SELFTEST] FAIL: freeze suppression/stale logic broken:" >&2
        printf '%s\n' "$out" >&2
        fails=1
    else
        echo "[SELFTEST] freeze: live entry suppresses, stale entry fails the gate"
    fi

    # 3) full freeze: all positives frozen -> clean pass, nothing stale.
    printf '%s\n' \
        'crates/pos/src/p1.rs:2 # DESIGN' \
        'crates/pos/src/p2.rs:2 # DESIGN' \
        'crates/pos/src/p3.rs:2 # DESIGN' \
        'crates/pos/src/p4.rs:3 # DESIGN' \
        > "$fx/scripts/lock_await_freeze.txt"
    out="$(CHECK_LOCK_AWAIT_ROOT="$fx" CHECK_LOCK_AWAIT_FREEZE="$fx/scripts/lock_await_freeze.txt" \
           run_gate 2>&1)" && rc=0 || rc=$?
    if [ "$rc" -ne 0 ] || ! printf '%s\n' "$out" | grep -q '\[OK\]'; then
        echo "[SELFTEST] FAIL: fully-frozen fixture should pass:" >&2
        printf '%s\n' "$out" >&2
        fails=1
    else
        echo "[SELFTEST] freeze: all exemptions live -> [OK]"
    fi

    # 4) missing freeze -> exit 2, never a silent pass.
    rm "$fx/scripts/lock_await_freeze.txt"
    out="$(CHECK_LOCK_AWAIT_ROOT="$fx" CHECK_LOCK_AWAIT_FREEZE="$fx/scripts/lock_await_freeze.txt" \
           run_gate 2>&1)" && rc=0 || rc=$?
    if [ "$rc" -ne 2 ]; then
        echo "[SELFTEST] FAIL: missing freeze must exit 2, got $rc:" >&2
        printf '%s\n' "$out" >&2
        fails=1
    else
        echo "[SELFTEST] missing freeze -> exit 2 (cannot decide)"
    fi

    # 5) missing scan root -> exit 2.
    out="$(CHECK_LOCK_AWAIT_ROOT="$fx/nope" CHECK_LOCK_AWAIT_FREEZE="$fx/scripts/x.txt" \
           run_gate 2>&1)" && rc=0 || rc=$?
    if [ "$rc" -ne 2 ]; then
        echo "[SELFTEST] FAIL: missing scan root must exit 2, got $rc" >&2
        fails=1
    else
        echo "[SELFTEST] missing scan root -> exit 2 (cannot decide)"
    fi

    rm -rf "$fx"
    if [ "$fails" -ne 0 ]; then
        echo "[SELFTEST] RESULT: FAIL" >&2
        return 1
    fi
    echo "[SELFTEST] RESULT: PASS"
    return 0
}

main() {
    case "${1:-}" in
        ""|--scan)
            run_gate
            ;;
        --selftest)
            selftest
            ;;
        -h|--help)
            sed -n '2,45p' "$0"
            ;;
        *)
            echo "usage: bash scripts/check_lock_await.sh [--selftest|--scan|--help]" >&2
            return 2
            ;;
    esac
}

main "$@"
