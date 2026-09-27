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
#   A. (RETIRED by ADR-187, 2026-09-24) The former "inner-ring boundary" check
#                              is removed from BOTH twins. Root cause (see
#                              docs/reports/Q3-adjudication-inner-ring-gate-2026-09-21.md):
#                              the inner/outer ring partition (ADR-054 d4) was
#                              never implemented (CoreLoopEvent absent), and after
#                              the 2026-09-18 .ps1 "fix" (phantom ADR-XXX) Check A
#                              became a strict subset of Check B (zero unique
#                              detections). The ten-layer law (agents.md 2.2)
#                              permits L(N)->L(N-1) downward deps, so gsoe(L5)->
#                              decay(L4) is lawful; Check A vetoed it with an
#                              unimplemented goal. Reopen only when CoreLoopEvent
#                              lands in event-bus/types.rs + inner-ring latency
#                              criterion baseline + a real shared-state container
#                              (see ADR-187).
#   B. Upward dependency     - L(N) -> L(N+1) is forbidden for every layered
#                              crate (see expected_crates); the 1-item ADR
#                              exception table is exempted
#                              (pvl-layer->seccore dynamic-blacklist feature).
#                              The former gqep-executor->qeep-protocol ADR-048
#                              entry was retired in wave 3c (M12, ADR-185 D4):
#                              L7->L4 is a legal DOWNWARD edge that Check B
#                              never flags, so the exemption was a no-op pad.
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
        nexus-core|event-bus) echo 1 ;;
        nmc-encoder|hcw-window|mlc-engine) echo 2 ;;
        # P2-T2 (2026-08-24): session-store session persistence (L3, 40th crate)
        scc-cache|lsct-tiering|cmt-tiering|session-store) echo 3 ;;
        seccore|qeep-protocol|decay-engine) echo 4 ;;
        repo-wiki|gsoe-evolution) echo 5 ;;
        # Phase 6 W0 层图订正 (2026-08-16, ADR-084): gea-activator 移 L9,
        # ssra-fusion 移 L7 — 与 crate 自述头及 AGENTS.md §2.1 对齐
        osa-coordinator|kvbsr-router|faae-router|sesa-router|omega-learner|router-traits) echo 6 ;;
        # P3-T9 (2026-08-27): nexus-subagent typed SubAgent runtime (L7, 43rd crate)
        pvl-layer|gqep-executor|mtpe-executor|ssra-fusion|nexus-subagent) echo 7 ;;
        # ADR-182 (2026-09-16, M10): acb-governor 退役删除, L8 余 parliament/decb-governor
        parliament|decb-governor) echo 8 ;;
        # P3-T2/T3 (2026-08-27): mas-sched peer scheduler + nexus-hook
        # lifecycle hooks (L9, 41st/42nd crates)
        quest-engine|efficiency-monitor|chimera-mas|gea-activator|mas-sched|nexus-hook) echo 9 ;;
        # WI-01 (2026-08-22): nexus-app-server host facade (L10, 39th crate)
        # mcp-mesh 2026-09-02 T10: 对齐文档 L10 归属(原脚本误置 L1)
        # csn-substitutor 2026-09-24 B5: 对齐文档 L10 归属(原脚本误置 L7;方案 §2.4 裁决,
        #   与 CODE_WIKI 层图/孤岛表/E08 映射/根 manifest 行内标注四处一致。
        #   方向上惰性: 其内部依赖只有 nexus-contracts(L0)/event-bus(L1)/nexus-core(L1),
        #   改层不翻转任何边 ⇒ 纯账面收口)
        chimera-cli|chimera-tui|chtc-bridge|mca-gateway|nexus-app-server|mcp-mesh|csn-substitutor) echo 10 ;;
        *) echo "" ;;
    esac
}

# All layered crates (static completeness bound for check C2).
# Authority for layer numbers: $layerMap in check_dependency_rules.ps1 -- both
# lists must stay identical (see DRIFT WARNING in the header).
layered_crates="nexus-contracts nexus-core event-bus mcp-mesh nmc-encoder hcw-window mlc-engine scc-cache lsct-tiering cmt-tiering session-store seccore qeep-protocol decay-engine repo-wiki gsoe-evolution osa-coordinator kvbsr-router faae-router gea-activator sesa-router ssra-fusion omega-learner router-traits pvl-layer gqep-executor mtpe-executor csn-substitutor parliament decb-governor quest-engine efficiency-monitor chimera-mas mas-sched nexus-hook chimera-cli chimera-tui chtc-bridge mca-gateway nexus-app-server nexus-subagent"
# ADR-182 (2026-09-16, M10): acb-governor 退役,43 -> 42
# 架构减法批次 (2026-09-20): +router-traits -> 43
# B0-6 (2026-09-20): auto-dpo / model-router 两个"历史层号条目"出表 —— 二者磁盘
#   目录已删，留在生产层图里只让 check_layer_map_parity 判 map-only 漂移。
#   原先"保留以支撑 selftest mock 基线"的理由经实测不成立：mock 图借用的
#   model-router 已换成同为 L1 的现存 event-bus（见 mock_deps_of 注释），
#   check_layer_map_parity.py 的 selftest 夹具本就用合成名 crate-NN。
# 口径：此值必须等于 `Cargo.toml` workspace.members 实测数，勿硬编码追数。
expected_crates=41

# Inner-ring whitelist (is_inner_ring) and L0/L1 base (is_inner_base) helpers
# were removed together with Check A by ADR-187 (2026-09-24); they were used
# ONLY by the retired inner-ring boundary check. The three-ring reorganization
# intent stays documented as a NOT-ENFORCED design goal in agents.md 2.2/3.4.5,
# not as a machine gate. KEEP IN SYNC with check_dependency_rules.ps1 ($innerRing
# / $innerBase removed there in the same commit).

# ADR exception table: exempted from check B.
# WHY 只剩 1 条:gqep-executor(L7)->qeep-protocol(L4) 条目已于 wave 3c 收编移除
# (M12/ADR-185 D4)——Check B 只 flag 向上依赖(dep_layer > layer),该向下边无豁免
# 也天然合法,例外名义只剩 no-op 安全垫,挂着反而误导后人以为该边仍异常。
# 移除后双门仍绿 = 收编的可执行证据(先例:P9-T6 复审"外环内部 L7->L4 合法")。
is_adr_exception() {
    case "$1,$2" in
        pvl-layer,seccore) return 0 ;;
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
    # 层图条目必须对应真实目录：条目比代码活得久时（crate 已退役但层号未清理），
    # 这里会直接对不存在的 manifest 报错。跳过而非静默产出空列表，让 C3 反向
    # 检查去显式暴露它。
    [ -f "crates/$crate/Cargo.toml" ] || return 0
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
        mlc-engine) echo "nexus-core event-bus scc-cache" ;;        # GAP-B (upward; Check A retired by ADR-187)
        repo-wiki) echo "nexus-core event-bus ghost-crate" ;;       # GAP-C (undefined dep)
        # GAP-B 夹具必须借用**现存** crate：曾用 model-router(L1) 承载此形状，
        # 该 crate 已随 ADR-172 物理删除，借用已删 crate 会把层图生产表钉住
        # （删条目即打断负控），故改用同样 L1 的 event-bus。
        event-bus) echo "nexus-core hcw-window" ;;                  # GAP-B (L1 -> L2)
        gqep-executor) echo "nexus-core event-bus qeep-protocol" ;; # legal downward L7->L4 (no exemption needed since ADR-048 wave-3c retirement, ADR-185 D4)
        pvl-layer) echo "nexus-core event-bus seccore" ;;           # legal (feature-gated)
        nexus-contracts) echo "" ;;
        chimera-tui) echo "nexus-contracts event-bus pvl-layer" ;;  # GAP-E (L10 -> L7)
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
        echo "mlc-engine repo-wiki event-bus gqep-executor pvl-layer nexus-contracts chimera-tui"
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

    # --- Check A: RETIRED by ADR-187 (2026-09-24) ---
    # The inner-ring boundary check was deleted from both twins here. Its
    # detection surface collapsed into Check B (strict-subset after the 2026-09-18
    # .ps1 "fix", zero unique detections) and it vetoed the lawful downward
    # gsoe(L5)->decay(L4) edge using the never-implemented ring partition.
    # Machine-proof of "green via removal, not relaxation": the selftest fixture
    # mlc-engine(L2)->scc-cache(L3) is an UPWARD edge that Check B still flags,
    # so deleting Check A loses no detectable violation class. Reopen conditions
    # live in ADR-187; see gate-predicate parity gate (G-38/G-39) going expect=0.

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

    # --- Check E: chimera-tui outbound boundary (PS-2 2.2 enforcement) ---
    # Mirror of .ps1 Check E. Scope = chimera-tui ONLY: other L10 crates are
    # composition roots / host facades whose job is to instantiate lower layers
    # (e.g. nexus-app-server -> nexus-subagent L7), so blocking their downward
    # edges would forbid wire-up itself.
    for crate in chimera-tui; do
        layer="$(layer_of "$crate")"
        [ -n "$layer" ] || continue
        for dep in $(deps_of "$crate"); do
            is_adr_exception "$crate" "$dep" && continue
            dep_layer="$(layer_of "$dep")"
            [ -n "$dep_layer" ] || continue
            if [ "$dep_layer" -ge 2 ] && [ "$dep_layer" -le 9 ]; then
                report+=("[GAP-E] $crate (L10) -> $dep (L$dep_layer) outbound violation; route cross-layer data via EventBus (2.2)")
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
    # 11 as of 2026-09-12, was 13). The mas-sched control plane was already
    # split out; further growth means the execution plane must keep splitting.
    # 13 -> 11 (arch direction E batch): `mlc-engine` and `cmt-tiering` were
    # ghost dependencies (declared, zero reference in src/) and were removed,
    # so this ratchet now runs on the real base. Guarded by
    # scripts/check_declared_dep_usage.py (G-30/G-31) against re-introduction.
    # Mirror of .ps1 Check D.
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
    #   GAP-A x0 (Check A retired by ADR-187; mlc-engine->scc-cache is now caught
    #             by GAP-B alone -- the loss-0 machine proof)
    #   GAP-B x2 (mlc-engine->scc-cache, event-bus->hcw-window)
    #   GAP-C x1 (repo-wiki->ghost-crate)
    #   GAP-E x1 (chimera-tui L10 -> pvl-layer L7; PS-2 2.2)
    run_checks "no"

    ga="$(printf '%s\n' "${report[@]}" | grep -c '^\[GAP-A\]' || true)"
    gb="$(printf '%s\n' "${report[@]}" | grep -c '^\[GAP-B\]' || true)"
    gc="$(printf '%s\n' "${report[@]}" | grep -c '^\[GAP-C\]' || true)"
    ok=1
    # Check A retired (ADR-187): assert GAP-A is EMPTY (proves the check is gone)
    # while GAP-B still catches mlc-engine->scc-cache + event-bus->hcw-window (2).
    [ "$ga" -eq 0 ] || { ok=0; report+=("[SELFTEST] expected 0 GAP-A line (Check A retired by ADR-187), got $ga"); }
    [ "$gb" -eq 2 ] || { ok=0; report+=("[SELFTEST] expected 2 GAP-B lines (mlc-engine->scc-cache, event-bus->hcw-window), got $gb"); }
    [ "$gc" -eq 1 ] || { ok=0; report+=("[SELFTEST] expected 1 GAP-C line (repo-wiki->ghost-crate), got $gc"); }
    ge="$(printf '%s\n' "${report[@]}" | grep -c '^\[GAP-E\]' || true)"
    [ "$ge" -eq 1 ] || { ok=0; report+=("[SELFTEST] expected 1 GAP-E line (chimera-tui->pvl-layer), got $ge"); }
    printf '%s\n' "${report[@]}" | grep -q '\[GAP-B\] mlc-engine (L2) -> scc-cache' || { ok=0; report+=('[SELFTEST] missing GAP-B for mlc-engine -> scc-cache'); }
    printf '%s\n' "${report[@]}" | grep -q '\[GAP-B\] event-bus (L1) -> hcw-window' || { ok=0; report+=('[SELFTEST] missing GAP-B for event-bus -> hcw-window'); }
    printf '%s\n' "${report[@]}" | grep -q 'undefined dependency <ghost-crate>' || { ok=0; report+=('[SELFTEST] missing GAP-C for undefined dep ghost-crate'); }
    printf '%s\n' "${report[@]}" | grep -q '\[GAP-E\] chimera-tui (L10) -> pvl-layer' || { ok=0; report+=('[SELFTEST] missing GAP-E for chimera-tui -> pvl-layer'); }
    # 合法/例外边不得报 gap:gqep-executor->qeep-protocol 为向下边(ADR-048 已于
    # wave 3c 收编,无豁免天然合法,ADR-185 D4);pvl-layer->seccore 为 ADR 例外
    # (feature-gated)。两条边在 mock 图中均存在,任何一条冒出 GAP 都是回归。
    if printf '%s\n' "${report[@]}" | grep -Eq '^\[GAP-[ABC]\] (gqep-executor|pvl-layer)'; then
        ok=0
        report+=('[SELFTEST] legal/ADR-exempted edge (gqep-executor->qeep-protocol downward / pvl-layer->seccore) wrongly reported')
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
    echo "[OK] dependency iron-law audit all pass (B upward deps / C completeness / D mas dep bound / E L10 outbound; A inner-ring retired by ADR-187)"
else
    echo "[FAIL] dependency iron-law audit found gaps, see [GAP-*] lines above, fix and rerun"
fi
exit "$status"
