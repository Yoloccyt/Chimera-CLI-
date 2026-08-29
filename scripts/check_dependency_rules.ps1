# =============================================================================
# check_dependency_rules.ps1 - Architecture dependency-iron-law audit (P9-T10)
# =============================================================================
# Purpose: enforce the three dependency iron laws of the NEXUS-OMEGA 10-layer
#          architecture as a scriptable gate (ADR-054 decision 5: keep the
#          single workspace + a dependency audit script as the constraint).
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
#                              in the layer map; layer map covers 39/39 crates
#                              (static count + disk scan in normal mode).
# Author:  staff-engineer-mode (architecture governance specialist)
# Refs:    ADR-054 decision 5 / ADR-048, .trae/rules/nuxus-rules.md section 2.2,
#          docs/architecture/CODE_WIKI.md section 2
# Exit code: 0 = clean, 1 = gap found
# Usage:
#   powershell -NoProfile -File scripts/check_dependency_rules.ps1
#   powershell -NoProfile -File scripts/check_dependency_rules.ps1 -SelfTest
# Encoding: all-ASCII to avoid IDE/CJK path corruption (project script convention)
# =============================================================================

param([switch]$SelfTest)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$status = 0
$report = @()

# =============================================================================
# Configuration tables (single source of truth; mirrored in
# scripts/check_dependency_rules.sh - keep both in sync)
# =============================================================================

# Layer map: workspace crate -> layer number (L0 = 0 ... L10 = 10).
# Source: workspace.dependencies comments in root Cargo.toml + P9-T1 layer
# conflict adjudication report section 2.5.
$layerMap = @{
    # L0 Contracts (ADR-033): zero-logic contract layer
    'nexus-contracts' = 0
    # L1 Core
    'nexus-core' = 1; 'event-bus' = 1; 'model-router' = 1; 'mcp-mesh' = 1
    # L2 Memory
    'nmc-encoder' = 2; 'hcw-window' = 2; 'mlc-engine' = 2
    # L3 Storage
    'scc-cache' = 3; 'lsct-tiering' = 3; 'cmt-tiering' = 3
    # P2-T2 (2026-08-24): session-store session persistence (L3, 40th crate)
    'session-store' = 3
    # L4 Security
    'seccore' = 4; 'qeep-protocol' = 4; 'decay-engine' = 4
    # L5 Knowledge
    'repo-wiki' = 5; 'gsoe-evolution' = 5; 'auto-dpo' = 5
    # L6 Router
    # Phase 6 W0 层图订正 (2026-08-16, ADR-084): gea-activator 移 L9 Quest,
    # ssra-fusion 移 L7 Execution — 与 crate 自述头及 AGENTS.md §2.1 对齐
    # (二者均零外部生产依赖方, 移动经 grep 核实无 Check-B 影响)
    'osa-coordinator' = 6; 'kvbsr-router' = 6; 'faae-router' = 6
    'sesa-router' = 6; 'omega-learner' = 6
    # L7 Execution
    'pvl-layer' = 7; 'gqep-executor' = 7; 'mtpe-executor' = 7
    'csn-substitutor' = 7; 'ssra-fusion' = 7
    # P3-T9 (2026-08-27): nexus-subagent typed SubAgent runtime (L7, 43rd crate)
    'nexus-subagent' = 7
    # L8 Parliament
    'parliament' = 8; 'acb-governor' = 8; 'decb-governor' = 8
    # L9 Quest
    'quest-engine' = 9; 'efficiency-monitor' = 9; 'chimera-mas' = 9
    'gea-activator' = 9
    # P3-T2 (2026-08-27): mas-sched peer scheduler control plane (L9, 41st crate)
    'mas-sched' = 9
    # P3-T3 (2026-08-27): nexus-hook lifecycle hook system (L9, 42nd crate)
    'nexus-hook' = 9
    # L10 Interface
    'chimera-cli' = 10; 'chimera-tui' = 10; 'chtc-bridge' = 10
    'mca-gateway' = 10
    # WI-01 (2026-08-22): nexus-app-server host facade (L10, 39th crate)
    'nexus-app-server' = 10
}

# Expected total crate count (workspace members). Static completeness bound.
$expectedCrates = 43

# Inner-ring whitelist: 9 crates (memory + reasoning + evolution ring).
# Three-ring reorganization target: inner ring talks via shared memory/direct
# calls; it must never reach into the L2+ business outer ring.
$innerRing = @{
    'mlc-engine' = $true; 'hcw-window' = $true; 'nmc-encoder' = $true
    'quest-engine' = $true; 'parliament' = $true; 'gea-activator' = $true
    'gsoe-evolution' = $true; 'auto-dpo' = $true; 'repo-wiki' = $true
}

# L0/L1 base deps that any inner-ring crate may use. L0/L1 is the Core
# infrastructure layer: ADR-054 decision 2 spirit allows the inner ring to
# depend on ALL L0/L1 crates (only the L2+ business outer ring is forbidden).
# auto-dpo (L5, inner) -> model-router (L1) is a legal L5->L1 downward edge
# (RHI-CG judge routes via model-router); mcp-mesh (L1) likewise allowed.
$innerBase = @{
    'nexus-contracts' = $true; 'nexus-core' = $true; 'event-bus' = $true
    'model-router' = $true; 'mcp-mesh' = $true
}

# ADR exception table: "from,to" -> reference. Exempted from check B.
# gqep-executor (L7) -> qeep-protocol (L4): ADR-048 accepted cross-layer
#   penetration (synchronous low-latency entangle call, <10us).
# pvl-layer (L7) -> seccore (L4): gated behind pvl-layer optional feature
#   `dynamic-blacklist` (seccore declared optional = true).
$adrExceptions = @{
    'gqep-executor,qeep-protocol' = 'ADR-048 cross-layer penetration (accepted tech debt)'
    'pvl-layer,seccore' = 'optional feature dynamic-blacklist (seccore optional = true)'
}

# =============================================================================
# Dependency resolution
# =============================================================================

function Get-WorkspaceDeps {
    # Extract the production workspace-internal references of a crate from its
    # [dependencies] section only ([dev-dependencies] excluded). Only references
    # of the form `name = { workspace = true }` are collected; plain version refs
    # (e.g. `futures = "0.3"`) are external and ignored. Note: the collected
    # names include external workspace deps (tokio, serde, ...) - they are
    # filtered out in the checks by consulting the layer map + the declared
    # workspace.dependencies whitelist.
    # Section slicing: from the `[dependencies]` header up to the next
    # `[`-prefixed header line. optional = true refs (e.g. seccore in
    # pvl-layer) still count as production dependencies.
    param([string]$CrateName)
    $path = Join-Path 'crates' (Join-Path $CrateName 'Cargo.toml')
    if (-not (Test-Path $path)) { return @() }
    # UTF-8 explicit read: Windows PowerShell 5.1 Get-Content defaults to
    # ANSI/GBK, which corrupts the CJK comments in Cargo.toml and can swallow
    # newlines. (Same pitfall fixed in check_doc_consistency.ps1 section F.)
    $lines = [System.IO.File]::ReadAllLines((Resolve-Path $path).Path, [System.Text.Encoding]::UTF8)
    $inDeps = $false
    $deps = @()
    foreach ($line in $lines) {
        $trimmed = $line.Trim()
        if ($trimmed.StartsWith('[')) {
            if ($trimmed -eq '[dependencies]') { $inDeps = $true; continue }
            if ($inDeps) { break }  # any other section header ends [dependencies]
        }
        if ($inDeps -and $trimmed -match '^([A-Za-z0-9_-]+)\s*=\s*\{\s*workspace\s*=\s*true') {
            $deps += $Matches[1]
        }
    }
    return $deps
}

function Get-DeclaredWorkspaceDeps {
    # Collect every dependency name declared in the root Cargo.toml
    # [workspace.dependencies] section (external crates such as tokio/serde
    # plus the 38 internal crate path entries). Used as the whitelist for
    # check C1: a `workspace = true` reference is legal if it is either an
    # internal crate (in the layer map) or declared here (external dep).
    $lines = [System.IO.File]::ReadAllLines((Resolve-Path 'Cargo.toml').Path, [System.Text.Encoding]::UTF8)
    $inDeps = $false
    $names = @{}
    foreach ($line in $lines) {
        $trimmed = $line.Trim()
        if ($trimmed.StartsWith('[')) {
            if ($trimmed -eq '[workspace.dependencies]') { $inDeps = $true; continue }
            if ($inDeps) { break }  # any other section header ends the block
        }
        if ($inDeps -and $trimmed -match '^([A-Za-z0-9_-]+)\s*=') {
            $names[$Matches[1]] = $true
        }
    }
    return $names
}

# =============================================================================
# Checks A / B / C against a dependency graph (shared by normal + selftest mode)
# =============================================================================

function Invoke-RuleChecks {
    # Runs the three dependency-iron-law checks against the given graph.
    # DepGraph: hashtable crate -> string[] of production workspace deps.
    # ScanDisk: $true in normal mode verifies 38/38 disk coverage (C3);
    #           $false in selftest mode (mock graph, no disk access).
    param(
        [hashtable]$DepGraph,
        [bool]$ScanDisk
    )

    # --- Check A: inner-ring boundary ---
    # Only internal edges (dep in the layer map) are audited; external workspace
    # deps (tokio, serde, ...) are legal for inner-ring crates and are left to
    # check C1 to validate their declaration.
    foreach ($crate in $innerRing.Keys) {
        foreach ($dep in @($DepGraph[$crate])) {
            if ($null -eq $dep) { continue }                     # empty graph slot
            if (-not $layerMap.ContainsKey($dep)) { continue }   # external dep, not an internal edge
            if ($innerBase.ContainsKey($dep) -or $innerRing.ContainsKey($dep)) { continue }
            $script:report += "[GAP-A] $crate -> $dep violates inner-ring boundary (inner-ring crates may only depend on L0/L1 base + inner-ring whitelist)"
            $script:status = 1
        }
    }

    # --- Check B: upward dependency L(N) -> L(N+1) ---
    foreach ($crate in $DepGraph.Keys) {
        if (-not $layerMap.ContainsKey($crate)) { continue }  # undefined crate -> handled by C
        $layer = $layerMap[$crate]
        foreach ($dep in @($DepGraph[$crate])) {
            if ($null -eq $dep) { continue }  # empty graph slot
            $key = "$crate,$dep"
            if ($adrExceptions.ContainsKey($key)) { continue }  # ADR-exempted edge
            if (-not $layerMap.ContainsKey($dep)) { continue }  # external dep -> no layer comparison
            $depLayer = $layerMap[$dep]
            if ($depLayer -gt $layer) {
                $script:report += "[GAP-B] $crate (L$layer) -> $dep (L$depLayer) upward dependency violation"
                $script:status = 1
            }
        }
    }

    # --- Check C: graph completeness ---
    # C1: every referenced workspace dep must be either an internal crate (in
    # the layer map) or declared in root [workspace.dependencies]; anything
    # else is an undefined dependency (typo / unregistered new crate).
    foreach ($crate in $DepGraph.Keys) {
        foreach ($dep in @($DepGraph[$crate])) {
            if ($null -eq $dep) { continue }  # empty graph slot
            if (-not $layerMap.ContainsKey($dep) -and -not $declaredDeps.ContainsKey($dep)) {
                $script:report += "[GAP-C] $crate references undefined dependency <$dep>"
                $script:status = 1
            }
        }
    }
    # C2: layer map must define the full 38-crate workspace (static bound).
    if ($layerMap.Count -ne $expectedCrates) {
        $script:report += "[GAP-C] layer map defines $($layerMap.Count) crates, expected $expectedCrates (38/38 coverage)"
        $script:status = 1
    }
    # C3: every crates/*/Cargo.toml on disk must be registered in the layer map
    #     (normal mode only; selftest uses a mock graph and no disk access).
    if ($ScanDisk) {
        $dirs = @(Get-ChildItem 'crates' -Directory | Where-Object { Test-Path (Join-Path $_.FullName 'Cargo.toml') })
        foreach ($d in $dirs) {
            if (-not $layerMap.ContainsKey($d.Name)) {
                $script:report += "[GAP-C] disk crate <$($d.Name)> not registered in layer map (expect 38/38 coverage)"
                $script:status = 1
            }
        }
        $script:report += "[C] disk crates scanned: $($dirs.Count), layer map entries: $($layerMap.Count) (expect 38/38)"

        # --- Check D: chimera-mas internal dependency bound (P3-T2, WI-29) ---
        # WI-29 strangler 目标: chimera-mas 内部 crate 依赖 ≤16（实测 13）;
        # 超限即拆（mas-sched 控制面已拆出,后续执行面继续瘦身）。
        $masInternal = @()
        if (Test-Path 'crates/chimera-mas/Cargo.toml') {
            $masLines = Get-Content 'crates/chimera-mas/Cargo.toml'
            $inDeps = $false
            foreach ($line in $masLines) {
                if ($line -match '^\[dependencies\]') { $inDeps = $true; continue }
                if ($line -match '^\[dev-dependencies\]') { $inDeps = $false }
                if ($inDeps -and $line -match '^([a-z0-9-]+)\s*=') {
                    $depName = $Matches[1]
                    if ($layerMap.ContainsKey($depName)) { $masInternal += $depName }
                }
            }
        }
        $masLimit = 16
        if ($masInternal.Count -gt $masLimit) {
            $script:report += "[GAP-D] chimera-mas internal deps $($masInternal.Count) > $masLimit (WI-29 bound, mas-sched split required)"
            $script:status = 1
        } else {
            $script:report += "[D] chimera-mas internal deps: $($masInternal.Count)/$masLimit (WI-29 <=16 bound)"
        }
    }
}

# =============================================================================
# Main flow
# =============================================================================

if ($SelfTest) {
    # -------------------------------------------------------------------------
    # SelfTest mode: run the shared checks against an embedded mock graph that
    # deliberately violates all three rules, then assert every constructed
    # violation was detected. No real Cargo.toml files are read (declaredDeps
    # stays empty, so the mock `ghost-crate` remains an undefined dependency).
    # Constructed violations:
    #   mlc-engine (inner, L2) -> scc-cache (L3 external) : GAP-A + GAP-B
    #   model-router (L1)      -> nmc-encoder (L2)        : GAP-B (upward)
    #   repo-wiki  (inner, L5) -> ghost-crate (undefined) : GAP-C
    # Legal edges that must NOT be reported:
    #   gqep-executor -> qeep-protocol (ADR-048),
    #   pvl-layer     -> seccore (feature dynamic-blacklist).
    # -------------------------------------------------------------------------
    $declaredDeps = @{}
    $mockGraph = @{
        'mlc-engine'      = @('nexus-core', 'event-bus', 'scc-cache')
        'repo-wiki'       = @('nexus-core', 'event-bus', 'ghost-crate')
        'model-router'    = @('nexus-core', 'nmc-encoder')
        'gqep-executor'   = @('nexus-core', 'event-bus', 'qeep-protocol')
        'pvl-layer'       = @('nexus-core', 'event-bus', 'seccore')
        'nexus-contracts' = @()
    }
    Invoke-RuleChecks -DepGraph $mockGraph -ScanDisk $false

    $gapALines = @($report | Where-Object { $_ -match '^\[GAP-A\]' })
    $gapBLines = @($report | Where-Object { $_ -match '^\[GAP-B\]' })
    $gapCLines = @($report | Where-Object { $_ -match '^\[GAP-C\]' })

    $selftestOk = $true
    if ($gapALines.Count -ne 1) { $selftestOk = $false; $report += "[SELFTEST] expected 1 GAP-A line (mlc-engine->scc-cache), got $($gapALines.Count)" }
    if ($gapBLines.Count -ne 2) { $selftestOk = $false; $report += "[SELFTEST] expected 2 GAP-B lines (mlc-engine->scc-cache, model-router->nmc-encoder), got $($gapBLines.Count)" }
    if ($gapCLines.Count -ne 1) { $selftestOk = $false; $report += "[SELFTEST] expected 1 GAP-C line (repo-wiki->ghost-crate), got $($gapCLines.Count)" }
    if (@($gapALines | Where-Object { $_ -like '*mlc-engine -> scc-cache*' }).Count -eq 0) { $selftestOk = $false; $report += '[SELFTEST] missing GAP-A for mlc-engine -> scc-cache' }
    if (@($gapBLines | Where-Object { $_ -like '*model-router (L1) -> nmc-encoder*' }).Count -eq 0) { $selftestOk = $false; $report += '[SELFTEST] missing GAP-B for model-router -> nmc-encoder' }
    if (@($gapCLines | Where-Object { $_ -like '*ghost-crate*' }).Count -eq 0) { $selftestOk = $false; $report += '[SELFTEST] missing GAP-C for undefined dep ghost-crate' }
    # ADR-exempted edges must never surface as gaps.
    if (@($report | Where-Object { $_ -match '^\[GAP-[ABC]\] (gqep-executor|pvl-layer)' }).Count -gt 0) {
        $selftestOk = $false
        $report += '[SELFTEST] ADR-exempted edge (gqep-executor->qeep-protocol / pvl-layer->seccore) wrongly reported'
    }

    if ($selftestOk) {
        $report += '[SELFTEST] all constructed violations detected'
        $status = 0
    } else {
        $report += '[SELFTEST] FAILED: not all constructed violations detected'
        $status = 1
    }
} else {
    # Normal mode: parse real crates/*/Cargo.toml files into a dependency graph
    # and collect the declared workspace.dependencies whitelist from root
    # Cargo.toml (used by check C1 to distinguish external deps from typos).
    $declaredDeps = Get-DeclaredWorkspaceDeps
    $depGraph = @{}
    $crates = @(Get-ChildItem 'crates' -Directory | Where-Object { Test-Path (Join-Path $_.FullName 'Cargo.toml') })
    foreach ($c in $crates) { $depGraph[$c.Name] = @(Get-WorkspaceDeps $c.Name) }
    $report += "[INFO] parsed $($crates.Count) crates from crates/*/Cargo.toml, declared workspace deps: $($declaredDeps.Count)"
    Invoke-RuleChecks -DepGraph $depGraph -ScanDisk $true
}

# =============================================================================
# Report output
# =============================================================================

foreach ($line in $report) { Write-Host $line }

Write-Host ''
if ($status -eq 0) {
    Write-Host '[OK] dependency iron-law audit all pass (A inner-ring boundary / B upward deps / C completeness)'
} else {
    Write-Host '[FAIL] dependency iron-law audit found gaps, see [GAP-*] lines above, fix and rerun'
}
exit $status
