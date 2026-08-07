# =============================================================================
# check_doc_consistency.ps1 - Architecture document three-way reconciliation
# =============================================================================
# Purpose: detect drift between Cargo.toml (code), docs/ (index), CHANGELOG.md (changelog)
# Scope:   6 categories / 14 checks; all assertions back to Cargo.toml as canonical truth
# Author:  staff-engineer-mode (documentation-lifecycle specialist)
# Refs:    docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md
#
# Categories:
#   A. Structural invariants   - Cargo.toml members count == disk crates count
#                              - workspace.package.version field present
#   B. Index document freshness - 5 main docs contain current crate count
#                                - 5 main docs contain current baseline string
#   C. Changelog reconciliation - CHANGELOG.md has `## [vX.Y.Z-omega]` (bracket) or `## vX.Y.Z-omega` header for current version
#   D. ADR physical vs index   - ADR-*.md files on disk match adr_index.md declaration
#   E. Policy compliance       - CONVENTIONS.md-declared subdirs exist
#                              - DOCUMENT_LIFECYCLE_POLICY.md (SoT) exists
#   F. NexusEvent reconciliation - enum variant count in types.rs vs CODE_WIKI.md declaration
#                                 (PROBE P-1.4: closes the blind spot where variant count
#                                  drift escaped the previous 5-category scan)
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
    # 2026-08-07 适配: *.md 在 gitignore 策略下仅存于本地(bb471f9 移除跟踪),
    # CI checkout 必然缺失文档 —— 降级为 warn 而非阻断。
    if ($null -eq $f -or $f -eq '' -or $f -like '*not-found*') { $report += '[B1-warn] nuxus rules file not discovered (gitignore *.md 策略,仅本地维护)'; continue }
    if (-not (Test-Path $f)) { $report += '[B1-warn] missing document: ' + $f + ' (gitignore *.md 策略,CI 环境无此文档,跳过)'; continue }
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
    # 2026-08-07 适配: 同 B1 —— gitignore *.md 策略下缺失文档降级为 warn。
    if ($null -eq $f -or $f -eq '' -or $f -like '*not-found*') { $report += '[B2-warn] nuxus rules file not discovered (gitignore *.md 策略,仅本地维护)'; continue }
    if (-not (Test-Path $f)) { $report += '[B2-warn] missing document: ' + $f + ' (gitignore *.md 策略,CI 环境无此文档,跳过)'; continue }
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
# Dual-format compatible: `## [2.20.0-omega] - date` (CHANGELOG canonical bracket
# style) and `## v2.20.0-omega` (bare style); lookahead (?=\s|$) anchors the
# version token so `## [2.20.0-omega]` does not false-positive on prefix matches.
if (-not (Test-Path 'CHANGELOG.md')) {
    # 2026-08-07 适配: CHANGELOG.md 在 gitignore *.md 策略下仅存于本地(bb471f9 移除跟踪),
    # CI checkout 必然缺失 —— 降级为 warn 而非阻断。
    $report += '[C1-warn] missing document: CHANGELOG.md (gitignore *.md 策略,仅本地维护,跳过)'
} else {
    $changelog = Get-Content 'CHANGELOG.md' -Raw
    $verHeaderPattern = '^##\s+\[?v?' + [regex]::Escape($currentVersion) + '\]?(?=\s|$)'
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
        if ($line -match '(\d+)\s*\u4E2A\s*ADR') {
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
    # 2026-08-07 适配: 同 B/C —— gitignore *.md 策略下缺失文档降级为 warn。
    $report += '[D2-warn] missing document: docs/architecture/adr_index.md (gitignore *.md 策略,仅本地维护,跳过)'
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
        # 2026-08-07 适配: 文档目录仅存于本地(gitignore *.md 策略,远程无 md 文件即无目录),
        # 降级为 warn 而非阻断。
        $report += '[E1-warn] CONVENTIONS.md declared subdir missing: ' + $d.Path + ' (' + $d.Purpose + '; gitignore *.md 策略,仅本地维护,跳过)'
    }
}

# E2. SoT policy file existence
if (-not (Test-Path 'docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md')) {
    # 2026-08-07 适配: 同 E1 —— gitignore *.md 策略下仅本地维护,降级为 warn。
    $report += '[E2-warn] missing policy file: docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md (gitignore *.md 策略,仅本地维护,跳过)'
}

# =============================================================================
# F. NexusEvent variant count (types.rs enum vs CODE_WIKI declaration)
# =============================================================================
# F1. Parse NexusEvent enum body from types.rs via brace-balanced scan, then
#     extract top-level variant names. Variant-name heuristic: line starts with
#     optional whitespace + uppercase CamelCase identifier followed by '{', '(',
#     ',' or end-of-line. Doc comments ('///') and serde attrs ('#') never match.
# WHY brace-balanced: struct variants contain nested braces, so naive regex
#     '.*?\n}' would truncate the enum body prematurely.
# Encoding pitfall (fixed 2026-08-03): Get-Content defaults to ANSI (GBK) on
#     Windows PowerShell 5.1, which corrupts UTF-8 CJK sequences and swallows
#     newlines (observed 3/32 variants instead of 126). Must read as UTF-8.
#     Strip order matters: comments first, then strings - reversing it lets
#     stray quotes in doc comments pair across lines and delete code regions.
$eventTypes = 'crates/event-bus/src/types.rs'
if (Test-Path $eventTypes) {
    $ts = [System.IO.File]::ReadAllText((Resolve-Path $eventTypes), [System.Text.Encoding]::UTF8)
    # Normalize newlines (defensive against CRLF / lone CR)
    $ts = $ts -replace "`r`n", "`n"
    $ts = $ts -replace "`r", "`n"
    # Strip line comments (incl. trailing), then raw/normal strings, then chars
    $ts = [regex]::Replace($ts, '(?m)//.*$', '')
    $ts = [regex]::Replace($ts, 'r#*"[\s\S]*?"#*', '""')
    $ts = [regex]::Replace($ts, '"[^"\\\r\n]*(?:\\.[^"\\\r\n]*)*"', '""')
    $ts = [regex]::Replace($ts, "'(?![a-zA-Z_])[^'\\\r\n]'", "''")
    $enumStart = $ts.IndexOf('pub enum NexusEvent')
    if ($enumStart -ge 0) {
        $braceStart = $ts.IndexOf('{', $enumStart)
        $depth = 0
        $braceEnd = -1
        for ($i = $braceStart; $i -lt $ts.Length; $i++) {
            $ch = $ts[$i]
            if ($ch -eq '{') { $depth++ }
            elseif ($ch -eq '}') {
                $depth--
                if ($depth -eq 0) { $braceEnd = $i; break }
            }
        }
        if ($braceEnd -gt $braceStart) {
            $body = $ts.Substring($braceStart + 1, $braceEnd - $braceStart - 1)
            $variants = [regex]::Matches($body, '(?m)^\s*([A-Z][A-Za-z0-9_]*)\s*(?:\{|\,|$|\()') |
                ForEach-Object { $_.Groups[1].Value } | Select-Object -Unique
            $eventVariantCount = $variants.Count
            $report += '[F1] NexusEvent enum variants (types.rs) = ' + $eventVariantCount

            # F2. CODE_WIKI.md must declare a matching NexusEvent variant count.
            # GAP only when declared < measured (stale docs). INFO when declared
            # == measured. WARN when declared > measured (possible doc leading edge).
            $codeWiki = 'docs/architecture/CODE_WIKI.md'
            if (Test-Path $codeWiki) {
                $cw = Get-Content $codeWiki -Raw
                $declMatch = [regex]::Match($cw, '(\d+)\s*NexusEvent')
                if (-not $declMatch.Success) { $declMatch = [regex]::Match($cw, 'NexusEvent[^0-9]{0,40}(\d+)') }
                if ($declMatch.Success) {
                    $declaredVariants = [int]$declMatch.Groups[1].Value
                    if ($declaredVariants -lt $eventVariantCount) {
                        $report += '[GAP-F2] CODE_WIKI.md declares ' + $declaredVariants + ' NexusEvent variants, types.rs has ' + $eventVariantCount + ' (stale, must sync)'
                        $status = 1
                    } elseif ($declaredVariants -eq $eventVariantCount) {
                        $report += '[F2] CODE_WIKI.md declares ' + $declaredVariants + ' NexusEvent variants, matches types.rs'
                    } else {
                        $report += '[F2-INFO] CODE_WIKI.md declares ' + $declaredVariants + ' NexusEvent variants, types.rs has ' + $eventVariantCount + ' (docs ahead, expected during release prep)'
                    }
                } else {
                    $report += '[WARN-F2] CODE_WIKI.md has no machine-readable NexusEvent variant count'
                }
            } else {
                $report += '[GAP-F2] missing document: docs/architecture/CODE_WIKI.md'
                $status = 1
            }
        } else {
            $report += '[WARN-F1] NexusEvent enum body not parseable in ' + $eventTypes
        }
    } else {
        $report += '[GAP-F1] NexusEvent enum not found in ' + $eventTypes
        $status = 1
    }
} else {
    $report += '[GAP-F1] missing file: ' + $eventTypes
    $status = 1
}

# =============================================================================
# Report output
# =============================================================================

foreach ($line in $report) { Write-Host $line }

Write-Host ''
if ($status -eq 0) {
    Write-Host ('[OK] three-way reconciliation all pass (6 categories / 14 checks): canonical version=' + $currentVersion + ', ' + $nMembers + ' crates, baseline aligned')
} else {
    Write-Host '[FAIL] three-way reconciliation found gaps, see [GAP-*] lines above, fix and rerun'
}
exit $status
