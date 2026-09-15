#!/usr/bin/env bash
# =============================================================================
# check_doc_drift.sh - doc count declarations vs code-measured counts (M3)
# =============================================================================
# Purpose: pin the three count metrics that historically drifted between
#          documents and code (pain evidence: "145 vs 144" NexusEvent variants,
#          EventTopic "9 categories" while the enum has 10, crate-count stale layer
#          maps that once turned CI red):
#
#            nexus_event_variants - crates/event-bus/src/types.rs pub enum NexusEvent
#            event_topic_variants - crates/event-bus/src/topic.rs pub enum EventTopic
#            workspace_crates     - Cargo.toml [workspace].members
#
# Model (aligned with scripts/audit_cmd_sync.py, ADR-167 decision 4):
#   * CODE is the authority. The three counts are measured from source on
#     every run. A code target that is missing or unparseable is "cannot
#     decide" -> exit 2, never a silent pass.
#   * LOCKED values live in scripts/doc_count_freeze.txt (single source of
#     truth). Code measurement != locked value -> exit 1 and a human runs
#     `--update-baseline` after reviewing WHY the count moved.
#   * DOC DECLARATIONS: a curated set of "current truth" anchors in committed
#     files is extracted and compared against the locked value. A mismatch is
#     a FAIL unless the exact (file | metric | declared) triple is registered
#     in the freeze file's drift registry -> then it is a visible WARN.
#     The registry is fail-closed both ways: an unregistered drift FAILS, and
#     a registry entry that no longer matches any finding FAILS as STALE
#     (fix the doc, delete the line in the same PR). An anchor that suddenly
#     matches nothing also FAILS - a gate whose anchor rusts away must not
#     look green.
#
# Usage:
#   bash scripts/check_doc_drift.sh                # full check
#   bash scripts/check_doc_drift.sh -v             # verbose (every anchor)
#   bash scripts/check_doc_drift.sh --update-baseline  # re-lock measured counts
#   bash scripts/check_doc_drift.sh --selftest     # prove the gate has teeth
# Exit code: 0 = pass (WARNs allowed), 1 = drift/stale/anchor-lost,
#            2 = usage/environment error (missing target/freeze)
# Encoding: all-ASCII (project script convention, Windows GBK consoles).
# =============================================================================
set -euo pipefail

# PATH may lack coreutils when launched from PowerShell/MSYS; harmless on Linux.
export PATH="/usr/bin:/bin:$PATH"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

METRICS=(nexus_event_variants event_topic_variants workspace_crates)

# ---------------------------------------------------------------------------
# Code measurement. Prints the count; exit 2 with a stderr note when the
# target file or its enum/members marker is missing.
# ---------------------------------------------------------------------------

# Count variants of `pub enum <name> {` (brace-balanced, top-level entries).
# $1 = file, $2 = enum name, $3 = ERE a variant line must match.
count_enum_variants() {
    local file="$1" enum="$2" vre="$3"
    [ -f "$file" ] || { echo "[FAIL] required file not found: $file" >&2; return 2; }
    awk -v marker="pub enum $enum {" -v vre="$vre" '
        index($0, marker) == 1 { inenum = 1; seen = 1; next }
        inenum && /^}/        { inenum = 0 }
        inenum && $0 ~ vre    { count++ }
        END {
            if (!seen) {
                print "[FAIL] enum marker not found: " marker > "/dev/stderr"
                exit 2
            }
            print count + 0
        }' "$file"
}

# Count "crates/ occurrences inside the [workspace] members block.
count_workspace_crates() {
    local file="$1"
    [ -f "$file" ] || { echo "[FAIL] required file not found: $file" >&2; return 2; }
    awk '
        /^members = \[/   { inm = 1; seen = 1; next }
        inm && /^\]/      { inm = 0 }
        inm               { n += gsub(/"crates\//, "\"crates/") }
        END {
            if (!seen) {
                print "[FAIL] [workspace] members block not found" > "/dev/stderr"
                exit 2
            }
            print n + 0
        }' "$file"
}

measure_code() {
    local scan_root="$1"
    MEASURED=()
    local v
    v="$(count_enum_variants "$scan_root/crates/event-bus/src/types.rs" \
            NexusEvent '^    [A-Z][A-Za-z0-9_]*[ ]*[{,(]')" || return 2
    MEASURED+=("$v")
    v="$(count_enum_variants "$scan_root/crates/event-bus/src/topic.rs" \
            EventTopic '^    [A-Z][A-Za-z0-9_]*,')" || return 2
    MEASURED+=("$v")
    v="$(count_workspace_crates "$scan_root/Cargo.toml")" || return 2
    MEASURED+=("$v")
}

# ---------------------------------------------------------------------------
# Freeze file: locked counts + drift registry.
#   metric = <n>                        (locked value, one per metric)
#   drift = <file> | <metric> | <n>     (registered doc drift, WARN-level)
# ---------------------------------------------------------------------------
load_freeze() {
    local freeze="$1"
    [ -f "$freeze" ] || { echo "[FAIL] freeze file missing: $freeze" >&2; return 2; }
    LOCKED=()                       # locked value per metric (index-aligned)
    REGISTRY=()                     # registered triples "file|metric|declared"
    local line k
    while IFS= read -r line; do
        case "$line" in
            ''|\#*) continue ;;
        esac
        local body="${line%%#*}"
        body="$(printf '%s' "$body" | sed 's/[[:space:]]*$//')"
        case "$body" in
            "drift = "*) REGISTRY+=("$(printf '%s' "${body#drift = }" | sed 's/[[:space:]]*|[[:space:]]*/|/g')") ;;
            *=*)
                k="$(printf '%s' "${body%%=*}" | tr -d '[:space:]')"
                LOCKED_KEYS+=("$k")
                LOCKED_VALS+=("$(printf '%s' "${body#*=}" | tr -d '[:space:]')")
                ;;
        esac
    done < "$freeze"
    # Index LOCKED by METRICS position for stable lookups.
    LOCKED=()
    local m found
    for m in "${METRICS[@]}"; do
        found=""
        for k in "${!LOCKED_KEYS[@]}"; do
            if [ "${LOCKED_KEYS[$k]}" = "$m" ]; then found="${LOCKED_VALS[$k]}"; fi
        done
        if [ -z "$found" ]; then
            echo "[FAIL] freeze file has no lock for metric: $m" >&2
            return 2
        fi
        LOCKED+=("$found")
    done
    return 0
}

# ---------------------------------------------------------------------------
# Doc declaration anchors: "current truth" spots that must carry the locked
# value. EREs are deliberately precise so version-history mentions of old
# counts (CHANGELOG-style "v2.27.0 had 144") do NOT match.
# ---------------------------------------------------------------------------
DECLS=(
    'agents.md|nexus_event_variants|定义 [0-9]+ 个 `NexusEvent` 变体'
    'crates/event-bus/src/topic.rs|nexus_event_variants|全部 [0-9]+ 个 NexusEvent 变体'
    'crates/event-bus/src/topic.rs|event_topic_variants|[0-9]+ 类 EventTopic'
    'crates/event-bus/src/topic.rs|event_topic_variants|全部 [0-9]+ 个 topic'
    'crates/event-bus/src/pattern_index.rs|event_topic_variants|[0-9]+ 类 EventTopic'
    'agents.md|workspace_crates|Workspace × \*\*[0-9]+ crates\*\*'
)

# ---------------------------------------------------------------------------
# Full check. Roots resolve here so the selftest can redirect them via
# CHECK_DOC_DRIFT_ROOT / CHECK_DOC_DRIFT_FREEZE.
# ---------------------------------------------------------------------------
run_check() {
    local verbose="${1:-}"
    local scan_root="${CHECK_DOC_DRIFT_ROOT:-$root}"
    local freeze="${CHECK_DOC_DRIFT_FREEZE:-scripts/doc_count_freeze.txt}"

    LOCKED_KEYS=(); LOCKED_VALS=(); REGISTRY=()
    load_freeze "$freeze" || return 2
    measure_code "$scan_root" || return 2

    local rc=0 i m

    # --- layer 1: code measurement vs locked baseline -----------------------
    for i in "${!METRICS[@]}"; do
        if [ "${MEASURED[$i]}" != "${LOCKED[$i]}" ]; then
            echo "[FAIL] code drift: ${METRICS[$i]} measured=${MEASURED[$i]}" \
                 "locked=${LOCKED[$i]}" >&2
            echo "       if the count move is deliberate: bash scripts/check_doc_drift.sh" \
                 "--update-baseline, then reconcile doc declarations" >&2
            rc=1
        fi
    done
    if [ "$rc" -ne 0 ]; then
        echo "[FAIL] gate cannot judge doc declarations against a moved baseline" >&2
        return 1
    fi
    [ -n "$verbose" ] && printf 'measured = locked: %s=%s %s=%s %s=%s\n' \
        "${METRICS[0]}" "${MEASURED[0]}" "${METRICS[1]}" "${MEASURED[1]}" \
        "${METRICS[2]}" "${MEASURED[2]}"

    # --- layer 2: doc declarations vs locked baseline -----------------------
    local -a findings=()    # unique "file|metric|declared" mismatches found in docs
    local -a fevidence=()   # first "file:line" evidence per finding (aligned)
    local file ere metric hits lineno text declared triple dup t
    for spec in "${DECLS[@]}"; do
        file="${spec%%|*}"; rest="${spec#*|}"
        metric="${rest%%|*}"; ere="${rest#*|}"
        if [ ! -f "$scan_root/$file" ]; then
            echo "[FAIL] declaration target missing: $file" >&2
            return 2
        fi
        hits="$(grep -Eno "$ere" "$scan_root/$file" || true)"
        if [ -z "$hits" ]; then
            echo "[FAIL] anchor lost: no match in $file for the $metric anchor" >&2
            echo "       (gate cannot verify - anchor rusting away is not green;" >&2
            echo "        see the DECLS table in scripts/check_doc_drift.sh)" >&2
            rc=1
            continue
        fi
        while IFS= read -r hit; do
            lineno="${hit%%:*}"
            text="${hit#*:}"
            declared="$(printf '%s' "$text" | grep -Eo '[0-9]+' | head -1)"
            i=0
            for m in "${METRICS[@]}"; do [ "$m" = "$metric" ] && break; i=$((i + 1)); done
            if [ "$declared" = "${LOCKED[$i]}" ]; then
                [ -n "$verbose" ] && echo "  [OK]   $file:$lineno declares $metric=$declared"
            else
                triple="$file|$metric|$declared"
                dup=0
                for t in "${findings[@]:-}"; do [ "$t" = "$triple" ] && dup=1; done
                if [ "$dup" -eq 0 ]; then
                    findings+=("$triple")
                    fevidence+=("$file:$lineno")
                fi
            fi
        done <<< "$hits"
    done

    # Verdicts on unique findings (one line of evidence per drift, not per
    # anchor occurrence - a file can hold several anchors for one metric).
    local fmetric fdeclared
    for i in "${!findings[@]}"; do
        triple="${findings[$i]}"
        fmetric="${triple#*|}"; fmetric="${fmetric%|*}"
        fdeclared="${triple##*|}"
        j=0
        for m in "${METRICS[@]}"; do [ "$m" = "$fmetric" ] && break; j=$((j + 1)); done
        if [[ " ${REGISTRY[*]-} " == *" $triple "* ]]; then
            echo "[WARN] registered drift: $fmetric declared=$fdeclared vs locked=${LOCKED[$j]}" \
                 "at ${fevidence[$i]} - fix the doc, then delete the registry line"
        else
            echo "[FAIL] unregistered drift: $fmetric declared=$fdeclared vs locked=${LOCKED[$j]}" \
                 "at ${fevidence[$i]}" >&2
            rc=1
        fi
    done

    # --- layer 3: registry entries that no longer match any finding ---------
    local entry matched
    for entry in "${REGISTRY[@]}"; do
        matched=0
        for triple in "${findings[@]:-}"; do
            [ "$triple" = "$entry" ] && matched=1
        done
        # An entry whose target file now declares the locked value (or whose
        # anchor vanished) has no matching finding -> stale -> fail.
        if [ "$matched" -eq 0 ]; then
            echo "[FAIL] stale registry entry (no current finding, doc fixed?):" \
                 "drift = $entry" >&2
            echo "       remove the line in the same PR that fixed the doc" >&2
            rc=1
        fi
    done

    if [ "$rc" -eq 0 ]; then
        local nwarn
        nwarn="${#findings[@]}"
        echo "[OK] doc counts in sync with code" \
             "(${METRICS[0]}=${MEASURED[0]}, ${METRICS[1]}=${MEASURED[1]}, ${METRICS[2]}=${MEASURED[2]};" \
             "$nwarn registered drift(s) under repayment)"
    fi
    return "$rc"
}

# ---------------------------------------------------------------------------
# --update-baseline: rewrite the locked counts from current measurement.
# Deliberately does NOT touch the drift registry - reconciling docs is human.
# ---------------------------------------------------------------------------
update_baseline() {
    local scan_root="${CHECK_DOC_DRIFT_ROOT:-$root}"
    local freeze="${CHECK_DOC_DRIFT_FREEZE:-scripts/doc_count_freeze.txt}"
    [ -f "$freeze" ] || { echo "[FAIL] freeze file missing: $freeze" >&2; return 2; }
    measure_code "$scan_root" || return 2
    local tmp="${freeze}.tmp.$$" i
    cp "$freeze" "$tmp"
    for i in "${!METRICS[@]}"; do
        sed -i "s/^${METRICS[$i]} = .*/${METRICS[$i]} = ${MEASURED[$i]}/" "$tmp"
    done
    mv "$tmp" "$freeze"
    printf '[OK] baseline re-locked from code: %s=%s %s=%s %s=%s\n' \
        "${METRICS[0]}" "${MEASURED[0]}" "${METRICS[1]}" "${MEASURED[1]}" \
        "${METRICS[2]}" "${MEASURED[2]}"
    echo "next: reconcile doc declarations (fix doc, or register drift in the freeze file)"
}

# ---------------------------------------------------------------------------
# Selftest: fixture repo exercising every verdict class.
# ---------------------------------------------------------------------------
selftest() {
    local fx
    fx="$(mktemp -d)"
    mkdir -p "$fx/crates/event-bus/src" "$fx/scripts"

    cat > "$fx/crates/event-bus/src/types.rs" <<'RS'
//! fixture types
pub enum NexusEvent {
    Alpha {
        x: u8,
    },
    Beta {
        y: u8,
    },
    Gamma,
}
RS
    cat > "$fx/crates/event-bus/src/topic.rs" <<'RS'
//! event topics - 2 类 EventTopic fixture
/// 事件主题 - 2 类分类覆盖全部 3 个 NexusEvent 变体(fixture)
pub enum EventTopic {
    Routing,
    Memory,
}
impl EventTopic {
    /// 返回全部 2 个 topic
    pub fn all() {}
}
RS
    cat > "$fx/crates/event-bus/src/pattern_index.rs" <<'RS'
//! - 与既有 `topic.rs`（2 类 EventTopic 订阅侧过滤）互补：PatternIndex
RS
    cat > "$fx/Cargo.toml" <<'TOML'
[workspace]
members = [
    "crates/alpha",
    "crates/beta",
    "crates/gamma",
]
TOML
    cat > "$fx/agents.md" <<'MD'
# fixture rules
- 技术栈 | Workspace × **3 crates**(fixture) |
- 当前: event-bus 定义 3 个 `NexusEvent` 变体(types.rs 单表)。
MD
    cat > "$fx/scripts/doc_count_freeze.txt" <<'FRZ'
# fixture freeze
nexus_event_variants = 3
event_topic_variants = 2
workspace_crates = 3
FRZ

    local fails=0 out rc

    # 1) everything green -> 0
    out="$(CHECK_DOC_DRIFT_ROOT="$fx" CHECK_DOC_DRIFT_FREEZE="$fx/scripts/doc_count_freeze.txt" \
           run_check)" && rc=0 || rc=$?
    if [ "$rc" -ne 0 ]; then
        echo "[SELFTEST] FAIL: all-green fixture must pass:" >&2; printf '%s\n' "$out" >&2; fails=1
    else
        echo "[SELFTEST] all-green fixture -> [OK] (rc=0)"
    fi

    # 2) unregistered drift -> 1 (nexus declared 9 in agents.md)
    sed -i 's/定义 3 个 `NexusEvent` 变体/定义 9 个 `NexusEvent` 变体/' "$fx/agents.md"
    out="$(CHECK_DOC_DRIFT_ROOT="$fx" CHECK_DOC_DRIFT_FREEZE="$fx/scripts/doc_count_freeze.txt" \
           run_check 2>&1)" && rc=0 || rc=$?
    if [ "$rc" -ne 1 ] || ! printf '%s\n' "$out" | grep -q 'unregistered drift'; then
        echo "[SELFTEST] FAIL: unregistered drift must fail:" >&2; printf '%s\n' "$out" >&2; fails=1
    else
        echo "[SELFTEST] unregistered drift -> rc=1 with evidence"
    fi

    # 3) registered drift -> 0 with WARN
    printf '%s\n' 'drift = agents.md | nexus_event_variants | 9' >> "$fx/scripts/doc_count_freeze.txt"
    out="$(CHECK_DOC_DRIFT_ROOT="$fx" CHECK_DOC_DRIFT_FREEZE="$fx/scripts/doc_count_freeze.txt" \
           run_check 2>&1)" && rc=0 || rc=$?
    if [ "$rc" -ne 0 ] || ! printf '%s\n' "$out" | grep -q 'registered drift'; then
        echo "[SELFTEST] FAIL: registered drift must warn and pass:" >&2; printf '%s\n' "$out" >&2; fails=1
    else
        echo "[SELFTEST] registered drift -> [WARN] (rc=0, visible debt)"
    fi

    # 4) doc fixed but registry line left behind -> stale -> 1
    sed -i 's/定义 9 个 `NexusEvent` 变体/定义 3 个 `NexusEvent` 变体/' "$fx/agents.md"
    out="$(CHECK_DOC_DRIFT_ROOT="$fx" CHECK_DOC_DRIFT_FREEZE="$fx/scripts/doc_count_freeze.txt" \
           run_check 2>&1)" && rc=0 || rc=$?
    if [ "$rc" -ne 1 ] || ! printf '%s\n' "$out" | grep -q 'stale registry entry'; then
        echo "[SELFTEST] FAIL: stale registry entry must fail:" >&2; printf '%s\n' "$out" >&2; fails=1
    else
        echo "[SELFTEST] stale registry entry -> rc=1 (forces same-PR cleanup)"
    fi
    sed -i '/^drift = agents.md/d' "$fx/scripts/doc_count_freeze.txt"

    # 5) code drift -> 1; --update-baseline re-locks; doc then mismatches -> 1
    sed -i 's/    Gamma,/    Gamma,\n    Delta,/' "$fx/crates/event-bus/src/types.rs"
    out="$(CHECK_DOC_DRIFT_ROOT="$fx" CHECK_DOC_DRIFT_FREEZE="$fx/scripts/doc_count_freeze.txt" \
           run_check 2>&1)" && rc=0 || rc=$?
    if [ "$rc" -ne 1 ] || ! printf '%s\n' "$out" | grep -q 'code drift'; then
        echo "[SELFTEST] FAIL: code drift must fail:" >&2; printf '%s\n' "$out" >&2; fails=1
    else
        echo "[SELFTEST] code drift (variants 3->4) -> rc=1, --update-baseline offered"
    fi
    out="$(CHECK_DOC_DRIFT_ROOT="$fx" CHECK_DOC_DRIFT_FREEZE="$fx/scripts/doc_count_freeze.txt" \
           update_baseline)" && rc=0 || rc=$?
    grep -q '^nexus_event_variants = 4$' "$fx/scripts/doc_count_freeze.txt" || rc=99
    if [ "$rc" -ne 0 ]; then
        echo "[SELFTEST] FAIL: --update-baseline must relock to 4:" >&2; printf '%s\n' "$out" >&2; fails=1
    else
        echo "[SELFTEST] --update-baseline re-locked nexus_event_variants=4"
    fi
    out="$(CHECK_DOC_DRIFT_ROOT="$fx" CHECK_DOC_DRIFT_FREEZE="$fx/scripts/doc_count_freeze.txt" \
           run_check 2>&1)" && rc=0 || rc=$?
    if [ "$rc" -ne 1 ] || ! printf '%s\n' "$out" | grep -q 'unregistered drift'; then
        echo "[SELFTEST] FAIL: moved baseline must re-expose doc drift:" >&2; printf '%s\n' "$out" >&2; fails=1
    else
        echo "[SELFTEST] re-locked baseline -> stale doc declaration fails again"
    fi

    # 6) missing code target -> 2 (never a silent pass)
    rm "$fx/crates/event-bus/src/types.rs"
    out="$(CHECK_DOC_DRIFT_ROOT="$fx" CHECK_DOC_DRIFT_FREEZE="$fx/scripts/doc_count_freeze.txt" \
           run_check 2>&1)" && rc=0 || rc=$?
    if [ "$rc" -ne 2 ]; then
        echo "[SELFTEST] FAIL: missing code target must exit 2, got $rc:" >&2; printf '%s\n' "$out" >&2; fails=1
    else
        echo "[SELFTEST] missing code target -> exit 2 (cannot decide)"
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
        "")        run_check "" ;;
        -v|--verbose) run_check "v" ;;
        --update-baseline) update_baseline ;;
        --selftest) selftest ;;
        -h|--help)
            sed -n '2,42p' "$0" ;;
        *)
            echo "usage: bash scripts/check_doc_drift.sh [-v|--update-baseline|--selftest|--help]" >&2
            return 2 ;;
    esac
}

main "$@"
