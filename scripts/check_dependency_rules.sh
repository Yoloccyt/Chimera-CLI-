#!/usr/bin/env bash
# =============================================================================
# check_dependency_rules.sh - Architecture dependency-iron-law audit (P9-T10)
# =============================================================================
# Purpose: enforce the three dependency iron laws of the NEXUS-OMEGA 10-layer
#          architecture as a scriptable gate (ADR-054 decision 5).
#          Logical twin of scripts/check_dependency_rules.ps1 - keep both in
#          sync (same config tables, same checks A/B/C, same exit semantics).
# Scope:
#   A. Inner-ring boundary   - the 9 inner-ring crates (memory + reasoning +
#                              evolution ring) may only depend on the L0/L1 base
#                              {nexus-contracts, nexus-core, event-bus,
#                              model-router, mcp-mesh} plus the inner-ring
#                              whitelist itself.
#   B. Upward dependency     - L(N) -> L(N+1) is forbidden for all 38 crates;
#                              the 2-item ADR exception table is exempted
#                              (gqep-executor->qeep-protocol ADR-048,
#                               pvl-layer->seccore dynamic-blacklist feature).
#   C. Graph completeness    - every referenced workspace dependency must exist
#                              in the layer map; layer map covers 38/38 crates
#                              (static count + disk scan in normal mode).
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
        nexus-core|event-bus|model-router|mcp-mesh) echo 1 ;;
        nmc-encoder|hcw-window|mlc-engine) echo 2 ;;
        scc-cache|lsct-tiering|cmt-tiering) echo 3 ;;
        seccore|qeep-protocol|decay-engine) echo 4 ;;
        repo-wiki|gsoe-evolution|auto-dpo) echo 5 ;;
        # Phase 6 W0 层图订正 (2026-08-16, ADR-084): gea-activator 移 L9,
        # ssra-fusion 移 L7 — 与 crate 自述头及 AGENTS.md §2.1 对齐
        osa-coordinator|kvbsr-router|faae-router|sesa-router|omega-learner) echo 6 ;;
        pvl-layer|gqep-executor|mtpe-executor|csn-substitutor|ssra-fusion) echo 7 ;;
        parliament|acb-governor|decb-governor) echo 8 ;;
        quest-engine|efficiency-monitor|chimera-mas|gea-activator) echo 9 ;;
        chimera-cli|chimera-tui|chtc-bridge|mca-gateway) echo 10 ;;
        *) echo "" ;;
    esac
}

# All 38 layered crates (static completeness bound for check C2).
layered_crates="nexus-contracts nexus-core event-bus model-router mcp-mesh nmc-encoder hcw-window mlc-engine scc-cache lsct-tiering cmt-tiering seccore qeep-protocol decay-engine repo-wiki gsoe-evolution auto-dpo osa-coordinator kvbsr-router faae-router gea-activator sesa-router ssra-fusion omega-learner pvl-layer gqep-executor mtpe-executor csn-substitutor parliament acb-governor decb-governor quest-engine efficiency-monitor chimera-mas chimera-cli chimera-tui chtc-bridge mca-gateway"
expected_crates=38

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
# auto-dpo (L5, inner) -> model-router (L1) is a legal L5->L1 downward edge
# (RHI-CG judge routes via model-router); mcp-mesh (L1) likewise allowed.
is_inner_base() {
    case "$1" in
        nexus-contracts|nexus-core|event-bus|model-router|mcp-mesh) return 0 ;;
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
# (external crates such as tokio/serde plus the 38 internal crate entries).
# Used as the whitelist for check C1. Normal mode parses the real file;
# selftest mode keeps it empty so the mock `ghost-crate` stays undefined.
# WHY set -- / "$*": the sed pipeline emits one name per line; collapsing to a
# space-separated string makes is_declared_dep's word-boundary case match work.
declared_deps=""
if [ "$SELFTEST" = "0" ]; then
    local_declared="$(sed -n '/^\[workspace\.dependencies\]$/,/^\[/p' Cargo.toml \
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
    sed -n '/^\[dependencies\]$/,/^\[/p' "crates/$crate/Cargo.toml" \
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
    # C2: layer map must define the full 38-crate workspace (static bound).
    # Use bash builtin positional params instead of wc/tr (portable, no
    # dependency on coreutils on minimal MSYS PATH setups).
    local n
    set -- $layered_crates
    n=$#
    if [ "$n" -ne "$expected_crates" ]; then
        report+=("[GAP-C] layer map defines $n crates, expected $expected_crates (38/38 coverage)")
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
                report+=("[GAP-C] disk crate <$name> not registered in layer map (expect 38/38 coverage)")
                status=1
            fi
        done
        report+=("[C] disk crates scanned: $disk_count, layer map entries: $n (expect 38/38)")
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
    echo "[OK] dependency iron-law audit all pass (A inner-ring boundary / B upward deps / C completeness)"
else
    echo "[FAIL] dependency iron-law audit found gaps, see [GAP-*] lines above, fix and rerun"
fi
exit "$status"
