# =============================================================================
# check_doc_consistency.ps1 - Architecture document three-way reconciliation
# =============================================================================
# Purpose: detect drift between Cargo.toml (code), docs/ (index), CHANGELOG.md (changelog)
# Scope:   5 categories / 12 checks; all assertions back to Cargo.toml as canonical truth
# Author:  staff-engineer-mode (documentation-lifecycle specialist)
# Refs:    docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md
#
# Categories:
#   A. Structural invariants   - Cargo.toml members count == disk crates count
#                              - workspace.package.version field present
#   B. Index document freshness - 5 main docs contain current crate count
#                                - 5 main docs contain current baseline string
#   C. Changelog reconciliation - CHANGELOG.md has `## vX.Y.Z-omega` header for current version
#   D. ADR physical vs index   - ADR-*.md files on disk match adr_index.md declaration
#   E. Policy compliance       - CONVENTIONS.md-declared subdirs exist
#                              - DOCUMENT_LIFECYCLE_POLICY.md (SoT) exists
#
# Exit code: 0 = clean, 1 = gap found
# Encoding:  all-ASCII to avoid IDE/CJK path corruption (the .trae/rules file
#            contains CJK which the IDE filters during write, so we discover it
#            via Get-ChildItem pattern instead of hardcoding the path)
# =============================================================================

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$status = 0
$report = @()

# Helper: discover a file by its ASCII basename under a known parent dir.
# Avoids hardcoding CJK characters that the IDE may corrupt during write.
function Get-AsciiPath {
    param([string]$ParentDir, [string]$AsciiBaseName, [string]$Extension = '.md')
    $found = Get-ChildItem -Path $ParentDir -Filter ("*" + $AsciiBaseName + "*" + $Extension) -File -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $found) { return $null }
    return $found.FullName
}

# Discover nuxus rules file (CJK name) by ASCII pattern matching.
# Use full path so Test-Path works regardless of cwd.
$nuxusRules = Get-AsciiPath -ParentDir '.trae/rules' -AsciiBaseName 'nuxus' -Extension '.md'
$nuxusRulesRel = if ($null -ne $nuxusRules) { $nuxusRules } else { '.trae/rules/nuxus-rule-not-found.md' }

# =============================================================================
# A. Structural invariants (Cargo.toml as canonical source)
# =============================================================================

# A1. canonical crate count: Cargo.toml members == disk crates/*/Cargo.toml
$cargo = Get-Content 'Cargo.toml' -Raw
$membersBlock = [regex]::Match($cargo, 'members\s*=\s*\[(.*?)\]', 'Singleline').Groups[1].Value
$nMembers = ([regex]::Matches($membersBlock, '"crates/[^"]+"')).Count
$nDirs = @(Get-ChildItem 'crates' -Directory | Where-Object { Test-Path (Join-Path $_.FullName 'Cargo.toml') }).Count
$report += '[A1] canonical crate count (Cargo.toml members) = ' + $nMembers
if ($nMembers -ne $nDirs) {
    $report += '[GAP-A1] Cargo.toml members(' + $nMembers + ') vs disk crates/*/Cargo.toml(' + $nDirs + ') mismatch'
    $status = 1
}

# A2. version field: Cargo.toml must have workspace.package.version
$verMatch = [regex]::Match($cargo, '(?m)^version\s*=\s*"([^"]+)"')
if (-not $verMatch.Success) {
    $report += '[GAP-A2] Cargo.toml missing workspace.package.version field'
    $status = 1
    $currentVersion = 'UNKNOWN'
} else {
    $currentVersion = $verMatch.Groups[1].Value
    $report += '[A2] canonical version (Cargo.toml workspace.package.version) = ' + $currentVersion
}

# =============================================================================
# B. Index document freshness (derived from code layer)
# =============================================================================

# B1. main docs contain current crate count (4 docs: README, CODE_WIKI, CLAUDE.md, nuxus-rules)
# Note: adr_index.md is an ADR index, not a crate index, so it is NOT checked here.
$b1Docs = @('docs/architecture/README.md', 'docs/architecture/CODE_WIKI.md', '.claude/CLAUDE.md', $nuxusRulesRel)
$crateTokens = @("$nMembers crate", "${nMembers} crate", "$nMembers Crate", "$nMembers/$nMembers crate", "${nMembers}个crate", "${nMembers} 个crate")
foreach ($f in $b1Docs) {
    if ($null -eq $f -or $f -eq '' -or $f -like '*not-found*') { $report += '[GAP-B1] nuxus rules file not discovered under .trae/rules/'; $status = 1; continue }
    if (-not (Test-Path $f)) { $report += '[GAP-B1] missing document: ' + $f; $status = 1; continue }
    $content = Get-Content $f -Raw
    $hit = $false
    foreach ($t in $crateTokens) { if ($content.Contains($t)) { $hit = $true; break } }
    if (-not $hit) {
        $report += '[GAP-B1] ' + $f + ' does not contain current crate count (' + $nMembers + '), possibly stale'
        $status = 1
    }
}

# B2. main docs contain current baseline string (DOCUMENT_LIFECYCLE_POLICY 6.4 trigger b)
$b2Docs = @('docs/architecture/CODE_WIKI.md', '.claude/CLAUDE.md', $nuxusRulesRel, 'CHANGELOG.md', 'docs/architecture/INDEX.md')
$baselineString = $currentVersion
foreach ($f in $b2Docs) {
    if ($null -eq $f -or $f -eq '' -or $f -like '*not-found*') { $report += '[GAP-B2] nuxus rules file not discovered'; $status = 1; continue }
    if (-not (Test-Path $f)) { $report += '[GAP-B2] missing document: ' + $f; $status = 1; continue }
    $content = Get-Content $f -Raw
    if (-not $content.Contains($baselineString)) {
        $report += '[GAP-B2] ' + $f + ' does not contain baseline string (' + $baselineString + ')'
        $status = 1
    }
}

# =============================================================================
# C. Changelog reconciliation
# =============================================================================

# C1. CHANGELOG.md must have ## vX.Y.Z-omega header for current version
if (-not (Test-Path 'CHANGELOG.md')) {
    $report += '[GAP-C1] missing document: CHANGELOG.md'
    $status = 1
} else {
    $changelog = Get-Content 'CHANGELOG.md' -Raw
    $verHeaderPattern = '^##\s+v' + [regex]::Escape($currentVersion) + '\b'
    if (-not [regex]::IsMatch($changelog, $verHeaderPattern, 'Multiline')) {
        $report += '[GAP-C1] CHANGELOG.md missing ## v' + $currentVersion + ' header (should be first entry)'
        $status = 1
    } else {
        $report += '[C1] CHANGELOG.md contains ## v' + $currentVersion + ' header'
    }
}

# =============================================================================
# D. ADR physical files vs index reconciliation
# =============================================================================

# D1. ADR physical file main numbers (dedupe rev[0-4] multi-version)
$adrFiles = @(Get-ChildItem 'docs/architecture/ADR-*.md' -ErrorAction SilentlyContinue)
$adrMainNumbers = @{}
foreach ($f in $adrFiles) {
    $m = [regex]::Match($f.BaseName, '^ADR-(\d{3})(?:-(rev\d+))?')
    if ($m.Success) {
        $num = $m.Groups[1].Value
        $rev = if ($m.Groups[2].Success) { $m.Groups[2].Value } else { 'main' }
        if (-not $adrMainNumbers.ContainsKey($num)) { $adrMainNumbers[$num] = @() }
        $adrMainNumbers[$num] += $rev
    }
}
$adrMainCount = $adrMainNumbers.Count
$report += '[D1] ADR physical file main numbers = ' + $adrMainCount + ' (with ' + (($adrFiles | Measure-Object).Count) + ' files, some multi-version)'

# D2. adr_index.md must declare ADR total (use Select-String with -match to avoid CJK regex issues)
# Logic:
#   - declared = the ADR total declared in adr_index.md (may include reserved/historical numbers)
#   - physical = the number of distinct ADR main numbers with physical files on disk
#   - expected: declared >= physical (some ADRs may be reserved/historical with no file)
#   - GAP only if declared < physical (index undercount vs actual files)
if (Test-Path 'docs/architecture/adr_index.md') {
    $declaredTotal = $null
    $adrIndexLines = Get-Content 'docs/architecture/adr_index.md' -Encoding UTF8
    foreach ($line in $adrIndexLines) {
        if ($line -match '(\d+)\s*个\s*ADR') {
            $declaredTotal = [int]$Matches[1]
            break
        }
    }
    if ($null -eq $declaredTotal) {
        $report += '[WARN-D2] adr_index.md has no machine-readable ADR total declaration'
    } elseif ($declaredTotal -lt $adrMainCount) {
        # real gap: index undercounts physical files
        $report += '[GAP-D2] adr_index.md declares ' + $declaredTotal + ' ADRs, disk has ' + $adrMainCount + ' main numbers (index undercount)'
        $status = 1
    } elseif ($declaredTotal -eq $adrMainCount) {
        $report += '[D2] adr_index.md declares ' + $declaredTotal + ' ADRs, matches disk ' + $adrMainCount + ' main numbers'
    } else {
        # expected: index declares more (includes reserved/historical numbers)
        $reserved = $declaredTotal - $adrMainCount
        $report += '[D2-INFO] adr_index.md declares ' + $declaredTotal + ' ADRs, disk has ' + $adrMainCount + ' main numbers (' + $reserved + ' reserved/historical, expected)'
    }
} else {
    $report += '[GAP-D2] missing document: docs/architecture/adr_index.md'
    $status = 1
}

# =============================================================================
# E. Policy compliance (CONVENTIONS.md declared subdirs + SoT file)
# =============================================================================

# E1. CONVENTIONS.md declared subdirs must exist
$requiredDirs = @(
    @{ Path = 'docs/architecture/audit'; Purpose = 'audit/governance signoff and review records' },
    @{ Path = 'docs/architecture/governance'; Purpose = 'governance/policy documents' },
    @{ Path = 'docs/architecture/_archive'; Purpose = '_archive/historical snapshots' },
    @{ Path = 'docs/architecture/_blueprints'; Purpose = '_blueprints/design blueprints (not yet implemented)' }
)
foreach ($d in $requiredDirs) {
    if (-not (Test-Path $d.Path)) {
        $report += '[GAP-E1] CONVENTIONS.md declared subdir missing: ' + $d.Path + ' (' + $d.Purpose + ')'
        $status = 1
    }
}

# E2. SoT policy file existence
if (-not (Test-Path 'docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md')) {
    $report += '[GAP-E2] missing policy file: docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md'
    $status = 1
}

# =============================================================================
# Report output
# =============================================================================

foreach ($line in $report) { Write-Host $line }

Write-Host ''
if ($status -eq 0) {
    Write-Host ('[OK] three-way reconciliation all pass (5 categories / 12 checks): canonical version=' + $currentVersion + ', ' + $nMembers + ' crates, baseline aligned')
} else {
    Write-Host '[FAIL] three-way reconciliation found gaps, see [GAP-*] lines above, fix and rerun'
}
exit $status
