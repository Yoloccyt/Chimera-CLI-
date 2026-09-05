#!/usr/bin/env bash
# =============================================================================
# check_dependency_rules.sh - Architecture dependency-iron-law audit (P9-T10)
# =============================================================================
# Purpose: enforce the three dependency iron laws of the NEXUS-OMEGA 10-layer
#          architecture as a scriptable gate (ADR-054 decision 5).
#          Logical twin of scripts/check_dependency_rules.ps1 - keep both in
#          sync (same config tables, same checks A/B/C, same exit semantics).
#
# DRIFT WARNING (2026-08-29 audit): the two files each hand-maintain a copy of
#   the layer map, and they DID drift -- .ps1 was raised to 43 crates while this
#   .sh stayed at 38, silently turning the CI iron-law job red for five crates
#   (nexus-app-server / session-store / mas-sched / nexus-hook / nexus-subagent).
#   CI runs THIS .sh (ci.yml), so a .ps1-only update is NOT a fix.
#   When adding a crate: update layer_of + layered_crates + expected_crates here
#   AND $layerMap + $expectedCrates in .ps1 in the same commit.
#   C10 (2026-09-04): drift is now MECHANICALLY blocked -- ci.yml check job runs
#   scripts/check_layer_map_parity.py which cross-checks this file's case dict +
#   layered_crates list, the .ps1 $layerMap, and Cargo.toml workspace.members
#   (four-way lock; any single-sided drift fails the gate at PR time).
# Scope:
#   A. Inner-ring boundary   - the 9 inner-ring crates (memory + reasoning +
#                              evolution ring) may only depend on the L0/L1 base
#                              {nexus-contracts, nexus-core, event-bus,
#                              model-router} plus the inner-ring whitelist itself.
#   B. Upward dependency     - L(N) -> L(N+1) is forbidden for every layered
#                              crate (see expected_crates); the 2-item ADR
#                              exception table is exempted
#                              (gqep-executor->qeep-protocol ADR-048,
#                               pvl-layer->seccore dynamic-blacklist feature).
#   C. Graph completeness    - every referenced workspace dependency must exist
#                              in the layer map; layer map must cover the whole
#                              workspace (static count + disk scan).
#   D. chimera-mas dep bound - internal crate deps <= 16 (WI-29 strangler).
# Author:  staff-engineer-mode (architecture governance specialist)
# Refs:    ADR-054 decision 5 / ADR-048, .trae/rules/nuxus-rules.md section 2.2
# Exit code: 0 = clean, 1 = gap found
# Usage:
#   bash scripts/check_dependency_rules.sh
#   bash scripts/check_dependency_rules.sh --selftest
# Encoding: all-ASCII to avoid CJK path/locale issues (project script convention)
# =============================================================================
set -euo pipefail

# Ensure coreutils are reachable even when the parent process (e.g. PowerShell
# on Windows/MSYS) passes a PATH without /usr/bin. Harmless no-op on Linux/macOS.
export PATH="/usr/bin:/bin:$PATH"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

status=0
report=()

SELFTEST=0
case "${1:-}" in
    --selftest) SELFTEST=1 ;;
    "") ;;
    *) echo "usage: $0 [--selftest]" >&2; exit 2 ;;
esac

# =============================================================================
# Configuration tables (single source of truth; mirrored in .ps1)
# =============================================================================

# Layer map: workspace crate -> layer number (L0 = 0 ... L10 = 10).
# Source: workspace.dependencies comments in root Cargo.toml + P9-T1 layer
# conflict adjudication report section 2.5.
layer_of() {
    case "$1" in
        nexus-contracts) echo 0 ;;
        nexus-core|event-bus|model-router) echo 1 ;;
        nmc-encoder|hcw-window|mlc-engine) echo 2 ;;
        # P2-T2 (2026-08-24): session-store session persistence (L3, 40th crate)
        scc-cache|lsct-tiering|cmt-tiering|session-store) echo 3 ;;
        seccore|qeep-protocol|decay-engine) echo 4 ;;
        repo-wiki|gsoe-evolution|auto-dpo) echo 5 ;;
        # Phase 6 W0 层图订正 (2026-08-16, ADR-084): gea-activator 移 L9,
        # ssra-fusion 移 L7 — 与 crate 自述头及 AGENTS.md §2.1 对齐
        osa-coordinator|kvbsr-router|faae-router|sesa-router|omega-learner) echo 6 ;;
        # P3-T9 (2026-08-27): nexus-subagent typed SubAgent runtime (L7, 43rd crate)
        pvl-layer|gqep-executor|mtpe-executor|csn-substitutor|ssra-fusion|nexus-subagent) echo 7 ;;
        parliament|acb-governor|decb-governor) echo 8 ;;
        # P3-T2/T3 (2026-08-27): mas-sched peer scheduler + nexus-hook
        # lifecycle hooks (L9, 41st/42nd crates)
        quest-engine|efficiency-monitor|chimera-mas|gea-activator|mas-sched|nexus-hook) echo 9 ;;
        # WI-01 (2026-08-22): nexus-app-server host facade (L10, 39th crate)
        # mcp-mesh 2026-09-02 T10: 对齐文档 L10 归属(原脚本误置 L1)
        chimera-cli|chimera-tui|chtc-bridge|mca-gateway|nexus-app-server|mcp-mesh) echo 10 ;;
        *) echo "" ;;
    esac
}

# All layered crates (static completeness bound for check C2).
# Authority for layer numbers: $layerMap in check_dependency_rules.ps1 -- both
# lists must stay identical (see DRIFT WARNING in the header).
layered_crates="nexus-contracts nexus-core event-bus model-router mcp-mesh nmc-encoder hcw-window mlc-engine scc-cache lsct-tiering cmt-tiering session-store seccore qeep-protocol decay-engine repo-wiki gsoe-evolution auto-dpo osa-coordinator kvbsr-router faae-router gea-activator sesa-router ssra-fusion omega-learner pvl-layer gqep-executor mtpe-executor csn-substitutor parliament acb-governor decb-governor quest-engine efficiency-monitor chimera-mas mas-sched nexus-hook chimera-cli chimera-tui chtc-bridge mca-gateway nexus-app-server nexus-subagent"
expected_crates=43

# Inner-ring whitelist: 9 crates (memory + reasoning + evolution ring).
# Three-ring reorganization target: inner ring talks via shared memory/direct
# calls; it must never reach into the L2+ business outer ring.
is_inner_ring() {
    case "$1" in
        mlc-engine|hcw-window|nmc-encoder|quest-engine|parliament|gea-activator|gsoe-evolution|auto-dpo|repo-wiki) return 0 ;;
        *) return 1 ;;
    esac
}

# L0/L1 base deps allowed for inner-ring crates. L0/L1 is the Core
# infrastructure layer; ADR-054 decision 2 spirit allows the inner ring to
# depend on ALL L0/L1 crates (only the L2+ business outer ring is forbidden).
# auto-dpo (L5, inner) -> model-router (L1): legal L5->L1 downward edge AND an
# ADR-171 T9 accepted pseudo-reachable production edge (ModelRouterJudgeClient).
# ADR-172 (Accepted 2026-09-03) retired model-router's "cross-model routing
# contract" status -- mca-gateway is the ONLY live LLM channel. The edge is KEPT
# (ADR-160 visible-debt posture: crate frozen, not deleted); model-router stays
# in this whitelist ONLY so the frozen edge stays green, NOT as a live-channel
# grant -- new LLM consumers MUST anchor on mca-gateway (ADR-172 decision 2).
# mcp-mesh is L10 (T10 realignment) so it is no longer an L0/L1 base dep.
is_inner_base() {
    case "$1" in
        nexus-contracts|nexus-core|event-bus|model-router) return 0 ;;
        *) return 1 ;;
    esac
}

# ADR exception table: exempted from check B.
is_adr_exception() {
    case "$1,$2" in
        gqep-executor,qeep-protocol|pvl-layer,seccore) return 0 ;;
        *) return 1 ;;
    esac
}

# Declared workspace deps in root Cargo.toml [workspace.dependencies]
# (external crates such as tokio/serde plus the 43 internal crate entries).
# Used as the whitelist for check C1. Normal mode parses the real file;
# selftest mode keeps it empty so the mock `ghost-crate` stays undefined.
# WHY set -- / "$*": the sed pipeline emits one name per line; collapsing to a
# space-separated string makes is_declared_dep's word-boundary case match work.
declared_deps=""
if [ "$SELFTEST" = "0" ]; then
    # WHY `tr -d '\r'`: the range START pattern is anchored with `$`. Root Cargo.toml
    # is CRLF on a core.autocrlf=true checkout (and the file even carries a UTF-8 BOM),
    # so `/^\[workspace\.dependencies\]$/` matches nothing -> declared_deps comes out EMPTY
    # and Check C1 emits FALSE [GAP-C] for perfectly declared deps (observed 2026-08-31:
    # 4 bogus gaps on nexus-contracts' serde/chrono/uuid/thiserror, all declared at root).
    # Worse, the per-crate read below degrades the opposite way: an empty dep list silently
    # disables the check. Either way the verdict would depend on which file happens to be
    # CRLF -- a gate must not be that fragile. Normalising line endings makes local == CI.
    local_declared="$(tr -d '\r' < Cargo.toml \
        | sed -n '/^\[workspace\.dependencies\]$/,/^\[/p' \
        | grep -v '^\[' \
        | grep -E '^[A-Za-z0-9_-]+[[:space:]]*=' \
        | sed -E 's/^([A-Za-z0-9_-]+).*/\1/' || true)"
    set -- $local_declared
    declared_deps="$*"
fi

is_declared_dep() {
    case " $declared_deps " in
        *" $1 "*) return 0 ;;
        *) return 1 ;;
    esac
}

# =============================================================================
# Dependency resolution
# =============================================================================

# Normal mode: extract production workspace deps from the [dependencies] section
# (dev-dependencies excluded). Slices from the `[dependencies]` header to the
# next `[`-prefixed header, strips header lines, then keeps only
# `name = { workspace = true }` refs (plain version refs are external).
workspace_deps_of() {
    local crate="$1"
    # WHY `tr -d '\r'`: same CRLF fragility as above -- without it a CRLF manifest
    # yields an EMPTY dep list, so the crate is quietly never checked (false green,
    # strictly worse than a false red).
    tr -d '\r' < "crates/$crate/Cargo.toml" \
        | sed -n '/^\[dependencies\]$/,/^\[/p' \
        | grep -v '^\[' \
        | grep -E '^[A-Za-z0-9_-]+[[:space:]]*=[[:space:]]*\{[[:space:]]*workspace[[:space:]]*=[[:space:]]*true' \
        | sed -E 's/^([A-Za-z0-9_-]+).*/\1/' || true
}

# SelfTest mode: embedded mock graph with deliberately constructed violations.
mock_deps_of() {
    case "$1" in
        mlc-engine) echo "nexus-core event-bus scc-cache" ;;        # GAP-A + GAP-B
        repo-wiki) echo "nexus-core event-bus ghost-crate" ;;       # GAP-C (undefined dep)
        model-router) echo "nexus-core nmc-encoder" ;;              # GAP-B (L1 -> L2)
        gqep-executor) echo "nexus-core event-bus qeep-protocol" ;; # legal (ADR-048)
        pvl-layer) echo "nexus-core event-bus seccore" ;;           # legal (feature-gated)
        nexus-contracts) echo "" ;;
        *) echo "" ;;
    esac
}

# Unified dep source: real files (normal) or mock graph (selftest).
deps_of() {
    if [ "$SELFTEST" = "1" ]; then
        mock_deps_of "$1"
    else
        workspace_deps_of "$1"
    fi
}

# Crate universe: real crates/* dirs (normal) or mock crate list (selftest).
all_crates() {
    if [ "$SELFTEST" = "1" ]; then
        echo "mlc-engine repo-wiki model-router gqep-executor pvl-layer nexus-contracts"
    else
        for d in crates/*/; do
            [ -f "$d/Cargo.toml" ] || continue
            basename "$d"
        done
    fi
}

# =============================================================================
# Checks A / B / C (shared by normal + selftest mode)
# =============================================================================
run_checks() {
    local scan_disk="$1"   # "yes" normal mode (disk coverage), "no" selftest
    local crate dep layer dep_layer

    # --- Check A: inner-ring boundary ---
    # Only internal edges (dep with a layer) are audited; external workspace
    # deps (tokio, serde, ...) are legal for inner-ring crates and are left to
    # check C1 to validate their declaration.
    for crate in $layered_crates; do
        is_inner_ring "$crate" || continue
        for dep in $(deps_of "$crate"); do
            [ -n "$(layer_of "$dep")" ] || continue   # external dep, not an internal edge
            if is_inner_base "$dep" || is_inner_ring "$dep"; then continue; fi
            report+=("[GAP-A] $crate -> $dep violates inner-ring boundary (inner-ring crates may only depend on L0/L1 base + inner-ring whitelist)")
            status=1
        done
    done

    # --- Check B: upward dependency L(N) -> L(N+1) ---
    for crate in $(all_crates); do
        layer="$(layer_of "$crate")"
        [ -n "$layer" ] || continue   # undefined crate -> handled by C
        for dep in $(deps_of "$crate"); do
            is_adr_exception "$crate" "$dep" && continue
            dep_layer="$(layer_of "$dep")"
            [ -n "$dep_layer" ] || continue   # undefined dep -> handled by C
            if [ "$dep_layer" -gt "$layer" ]; then
                report+=("[GAP-B] $crate (L$layer) -> $dep (L$dep_layer) upward dependency violation")
                status=1
            fi
        done
    done

    # --- Check C: graph completeness ---
    # C1: every referenced workspace dep must be either an internal crate (with
    # a layer) or declared in root [workspace.dependencies]; anything else is an
    # undefined dependency (typo / unregistered new crate).
    for crate in $(all_crates); do
        for dep in $(deps_of "$crate"); do
            if [ -z "$(layer_of "$dep")" ] && ! is_declared_dep "$dep"; then
                report+=("[GAP-C] $crate references undefined dependency <$dep>")
                status=1
            fi
        done
    done
    # C2: layer map must define the full workspace (static bound).
    # Use bash builtin positional params instead of wc/tr (portable, no
    # dependency on coreutils on minimal MSYS PATH setups).
    local n
    set -- $layered_crates
    n=$#
    if [ "$n" -ne "$expected_crates" ]; then
        report+=("[GAP-C] layer map defines $n crates, expected $expected_crates (full workspace coverage)")
        status=1
    fi
    for crate in $layered_crates; do
        if [ -z "$(layer_of "$crate")" ]; then
            report+=("[GAP-C] layer map entry <$crate> has no layer (internal config error)")
            status=1
        fi
    done
    # C3: every crates/*/Cargo.toml on disk must be registered in the layer map
    #     (normal mode only; selftest uses a mock graph and no disk access).
    if [ "$scan_disk" = "yes" ]; then
        local disk_count=0
        for d in crates/*/; do
            [ -f "$d/Cargo.toml" ] || continue
            local name
            name="$(basename "$d")"
            disk_count=$((disk_count + 1))
            if [ -z "$(layer_of "$name")" ]; then
                report+=("[GAP-C] disk crate <$name> not registered in layer map (expect $expected_crates/$expected_crates coverage)")
                status=1
            fi
        done
        report+=("[C] disk crates scanned: $disk_count, layer map entries: $n (expect $expected_crates/$expected_crates)")
    fi

    # --- Check D: chimera-mas internal dependency bound (P3-T2, WI-29) ---
    # WI-29 strangler target: chimera-mas internal crate deps <= 16 (measured
    # 13). The mas-sched control plane was already split out; further growth
    # means the execution plane must keep splitting. Mirror of .ps1 Check D.
    # Normal mode only, so the selftest GAP-A/B/C count assertions stay valid.
    if [ "$scan_disk" = "yes" ] && [ -f "crates/chimera-mas/Cargo.toml" ]; then
        local mas_internal=0
        local mas_limit=16
        for dep in $(workspace_deps_of chimera-mas); do
            if [ -n "$(layer_of "$dep")" ]; then
                mas_internal=$((mas_internal + 1))
            fi
        done
        if [ "$mas_internal" -gt "$mas_limit" ]; then
            report+=("[GAP-D] chimera-mas internal deps $mas_internal > $mas_limit (WI-29 bound, split required)")
            status=1
        else
            report+=("[D] chimera-mas internal deps: $mas_internal/$mas_limit (WI-29 <=16 bound)")
        fi
    fi
}

# =============================================================================
# Main flow
# =============================================================================

if [ "$SELFTEST" = "1" ]; then
    # SelfTest mode: run the shared checks against the embedded mock graph.
    # Expected detection (see mock_deps_of comments):
    #   GAP-A x1 (mlc-engine->scc-cache; repo-wiki->ghost-crate is an external
    #             ref with no layer, so check A skips it and C1 reports it)
    #   GAP-B x2 (mlc-engine->scc-cache, model-router->nmc-encoder)
    #   GAP-C x1 (repo-wiki->ghost-crate)
    run_checks "no"

    ga="$(printf '%s\n' "${report[@]}" | grep -c '^\[GAP-A\]' || true)"
    gb="$(printf '%s\n' "${report[@]}" | grep -c '^\[GAP-B\]' || true)"
    gc="$(printf '%s\n' "${report[@]}" | grep -c '^\[GAP-C\]' || true)"
    ok=1
    [ "$ga" -eq 1 ] || { ok=0; report+=("[SELFTEST] expected 1 GAP-A line (mlc-engine->scc-cache), got $ga"); }
    [ "$gb" -eq 2 ] || { ok=0; report+=("[SELFTEST] expected 2 GAP-B lines (mlc-engine->scc-cache, model-router->nmc-encoder), got $gb"); }
    [ "$gc" -eq 1 ] || { ok=0; report+=("[SELFTEST] expected 1 GAP-C line (repo-wiki->ghost-crate), got $gc"); }
    printf '%s\n' "${report[@]}" | grep -q '\[GAP-A\] mlc-engine -> scc-cache' || { ok=0; report+=('[SELFTEST] missing GAP-A for mlc-engine -> scc-cache'); }
    printf '%s\n' "${report[@]}" | grep -q '\[GAP-B\] model-router (L1) -> nmc-encoder' || { ok=0; report+=('[SELFTEST] missing GAP-B for model-router -> nmc-encoder'); }
    printf '%s\n' "${report[@]}" | grep -q 'undefined dependency <ghost-crate>' || { ok=0; report+=('[SELFTEST] missing GAP-C for undefined dep ghost-crate'); }
    # ADR-exempted edges must never surface as gaps.
    if printf '%s\n' "${report[@]}" | grep -Eq '^\[GAP-[ABC]\] (gqep-executor|pvl-layer)'; then
        ok=0
        report+=('[SELFTEST] ADR-exempted edge (gqep-executor->qeep-protocol / pvl-layer->seccore) wrongly reported')
    fi

    if [ "$ok" -eq 1 ]; then
        report+=('[SELFTEST] all constructed violations detected')
        status=0
    else
        report+=('[SELFTEST] FAILED: not all constructed violations detected')
        status=1
    fi
else
    # Normal mode: parse real crates/*/Cargo.toml files into a dependency graph.
    run_checks "yes"
fi

# =============================================================================
# Report output
# =============================================================================

for line in "${report[@]}"; do
    echo "$line"
done

echo ""
if [ "$status" -eq 0 ]; then
    echo "[OK] dependency iron-law audit all pass (A inner-ring / B upward deps / C completeness / D mas dep bound)"
else
    echo "[FAIL] dependency iron-law audit found gaps, see [GAP-*] lines above, fix and rerun"
fi
exit "$status"
