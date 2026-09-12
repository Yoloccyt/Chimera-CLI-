# =============================================================================
# check_doc_consistency.ps1 - Architecture document three-way reconciliation
# =============================================================================
# Purpose: detect drift between Cargo.toml (code), docs/ (index), CHANGELOG.md (changelog)
# Scope:   checks are SELF-REPORTED at the end of the run (categories / emitted check ids);
#          all assertions back to Cargo.toml as canonical truth
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
#                              - D3: index rows <-> disk files, both directions, with
#                                parsed-from-prose exemptions (merged ranges, "no
#                                standalone file", Released) - added 2026-08-31 after the
#                                ADR-161/166 numbering collision and the ADR-168 v1 phantom
#                                citation, neither of which any check could catch
#                              - D4: closed RK-P items must be written back to the exact
#                                source line they cite (ADR-168 decision 1)
#                              - D5: mojibake (UTF-8-as-GBK) damage scan against a frozen
#                                per-file baseline in scripts/mojibake_baseline.txt
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
# B1/B2 used to be silent on success, so the run reported fewer ids than checks actually
# executed and the hardcoded "14 checks" total could never be reconciled with observation.
# Every check must self-report on the green path too (see the derived footer count).
$b1Checked = 0
$b1Skipped = 0
foreach ($f in $b1Docs) {
    # 2026-08-07 适配: *.md 在 gitignore 策略下仅存于本地(bb471f9 移除跟踪),
    # CI checkout 必然缺失文档 —— 降级为 warn 而非阻断。
    if ($null -eq $f -or $f -eq '' -or $f -like '*not-found*') { $b1Skipped++; $report += '[B1-warn] nuxus rules file not discovered (gitignore *.md 策略,仅本地维护)'; continue }
    if (-not (Test-Path $f)) { $b1Skipped++; $report += '[B1-warn] missing document: ' + $f + ' (gitignore *.md 策略,CI 环境无此文档,跳过)'; continue }
    $content = Get-Content $f -Raw
    $hit = $false
    foreach ($t in $crateTokens) { if ($content.Contains($t)) { $hit = $true; break } }
    if (-not $hit) {
        $report += '[GAP-B1] ' + $f + ' does not contain current crate count (' + $nMembers + '), possibly stale'
        $status = 1
    } else {
        $b1Checked++
    }
}
$report += '[B1] ' + $b1Checked + '/' + $b1Docs.Count + ' docs carry current crate count (' + $nMembers + '), ' + $b1Skipped + ' skipped by gitignore *.md policy'

# B2. main docs contain current baseline string (DOCUMENT_LIFECYCLE_POLICY 6.4 trigger b)
$b2Docs = @('docs/architecture/CODE_WIKI.md', '.claude/CLAUDE.md', $nuxusRulesRel, 'CHANGELOG.md', 'docs/architecture/INDEX.md')
$baselineString = $currentVersion
$b2Checked = 0
$b2Skipped = 0
foreach ($f in $b2Docs) {
    # 2026-08-07 适配: 同 B1 —— gitignore *.md 策略下缺失文档降级为 warn。
    if ($null -eq $f -or $f -eq '' -or $f -like '*not-found*') { $b2Skipped++; $report += '[B2-warn] nuxus rules file not discovered (gitignore *.md 策略,仅本地维护)'; continue }
    if (-not (Test-Path $f)) { $b2Skipped++; $report += '[B2-warn] missing document: ' + $f + ' (gitignore *.md 策略,CI 环境无此文档,跳过)'; continue }
    $content = Get-Content $f -Raw
    if (-not $content.Contains($baselineString)) {
        $report += '[GAP-B2] ' + $f + ' does not contain baseline string (' + $baselineString + ')'
        $status = 1
    } else {
        $b2Checked++
    }
}
$report += '[B2] ' + $b2Checked + '/' + $b2Docs.Count + ' docs carry baseline string ' + $baselineString + ', ' + $b2Skipped + ' skipped by gitignore *.md policy'

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

# D3. adr_index.md table rows vs disk ADR files (bidirectional, with exemptions).
# WHY: ADR-161/166 numbering collision and the ADR-168 v1 phantom citation were both
#      caused by "index/ prose asserts something the disk does not contain". No existing
#      check covered that (D1 only counts, D2 only compares totals).
# Exemption semantics (parsed from the index row itself, never hardcoded lists):
#   - row declares "no standalone file" / "registered in <doc>" -> expected absent
#   - row status is Released / historical (numbers kept only in CHANGELOG) -> expected absent
# CJK literals are written as regex \uXXXX escapes to keep this file all-ASCII (L23-25).
$adrIdxPath = 'docs/architecture/adr_index.md'
$exemptPat  = '\u65e0\u72ec\u7acb\u7269\u7406\u6587\u4ef6|\u4e0d\u5355\u72ec\u5efa\u6863|\u767b\u8bb0\u4e8e|CHANGELOG'
$releasedPat = 'Released|\u5386\u53f2|\u5df2\u5e9f\u5f03'
$idxRowNums = @{}
if (Test-Path $adrIdxPath) {
    $adrIdxLines = @(Get-Content $adrIdxPath -Encoding UTF8)
    # Declared merged ranges (e.g. "ADR-001~006 + ADR-095~160") apply to BOTH directions.
    # First cut applied them only to the reverse direction, so ADR-001..005 - which the index
    # itself declares as file-less - were reported as GAP-D3. Ranges are parsed once here.
    $idxRangeCovered = @{}
    foreach ($line in $adrIdxLines) {
        foreach ($r in [regex]::Matches($line, 'ADR-(\d{3})\s*[~-]\s*(\d{3})')) {
            $lo = [int]$r.Groups[1].Value; $hi = [int]$r.Groups[2].Value
            for ($k = $lo; $k -le $hi; $k++) { $idxRangeCovered[$k] = $true }
        }
    }
    foreach ($line in $adrIdxLines) {
        $m = [regex]::Match($line, '^\|\s*ADR-(\d{3})\s*\|(.*)$')
        if ($m.Success) {
            $n = [int]$m.Groups[1].Value
            $body = $m.Groups[2].Value
            $ex = ([regex]::IsMatch($body, $exemptPat) -or [regex]::IsMatch($body, $releasedPat) -or $idxRangeCovered.ContainsKey($n))
            # keep the strongest exemption signal if a number appears twice
            if (-not $idxRowNums.ContainsKey($n) -or $ex) { $idxRowNums[$n] = $ex }
        }
    }
    # merged-range files cover every number inside them: ADR-135-144-*.md => 135..144
    $diskNums = @{}
    foreach ($f in $adrFiles) {
        $mm = [regex]::Match($f.BaseName, '^ADR-(\d{3})-(\d{3})(?:-rev\d+)?')
        if ($mm.Success) {
            $lo = [int]$mm.Groups[1].Value; $hi = [int]$mm.Groups[2].Value
            for ($k = $lo; $k -le $hi; $k++) { $diskNums[$k] = $f.Name }
        } else {
            $ms = [regex]::Match($f.BaseName, '^ADR-(\d{3})')
            if ($ms.Success) { $diskNums[[int]$ms.Groups[1].Value] = $f.Name }
        }
    }
    $d3Broken = @()
    foreach ($n in ($idxRowNums.Keys | Sort-Object)) {
        if (-not $diskNums.ContainsKey($n) -and -not $idxRowNums[$n]) {
            $d3Broken += ('ADR-{0:D3}' -f $n)
        }
    }
    if ($d3Broken.Count -gt 0) {
        $report += '[GAP-D3] adr_index.md rows point to ADRs with no file on disk and no' +
                   ' self-declared exemption (' + $d3Broken.Count + '): ' + (($d3Broken | Select-Object -First 12) -join ' ')
        $status = 1
    } else {
        # ratchet on the reverse direction (disk -> index). Merged segments are registered as
        # one range row in the index, so per-number rows are legitimately absent there; we freeze
        # the current count instead of pretending it is zero, and only allow it to shrink.
        $d3Unrowed = @()
        foreach ($n in ($diskNums.Keys | Sort-Object)) {
            if (-not $idxRowNums.ContainsKey($n) -and -not $idxRangeCovered.ContainsKey($n)) { $d3Unrowed += $n }
        }
        $d3Cap = 0   # ratchet: frozen 2026-08-31 at measured 0 after range-row expansion; may only shrink
        if ($d3Unrowed.Count -gt $d3Cap) {
            $report += '[GAP-D3] ' + $d3Unrowed.Count + ' disk ADR numbers are neither row-indexed nor inside' +
                       ' a declared merged range (cap ' + $d3Cap + '): ' + (($d3Unrowed | ForEach-Object { 'ADR-{0:D3}' -f $_ } | Select-Object -First 12) -join ' ')
            $status = 1
        } else {
            $report += '[D3] adr_index.md rows <-> disk files reconcile (' + $idxRowNums.Count + ' rows,' +
                       ' ' + $diskNums.Count + ' numbers on disk, unrowed ' + $d3Unrowed.Count + ' <= cap ' + $d3Cap + ')'
        }
    }
} else {
    # missing index is a real gap for a D-category check: never degrade to silent skip.
    $report += '[GAP-D3] missing file: ' + $adrIdxPath + ' (cannot verify ADR index <-> disk)'
    $status = 1
}

# D4. closed-item write-back (ADR-168 decision 1, landed as D4 because D3-D6 were free).
# WHY: a deviation recorded in a closure report was closed in RK-P + ADR-157, but the
#      original report line still showed the pre-fix "warning, buffer consumed" state, and
#      nothing went red. Closed conclusions must be reachable in one hop from where the
#      number first appears.
# Rule: every RK-P row whose status says "closed" must carry a `file.md:LINE` source, and
#      that line must mention "closed" or the closing ADR id. Missing source file = not
#      determinable = GAP (no silent skip).
$closedPat = '\u5df2\u5173\u95ed'
$rkPath = 'docs/governance/RK-P_risk_register.md'
if (Test-Path $rkPath) {
    $d4Bad = @(); $d4Checked = 0
    foreach ($line in (Get-Content $rkPath -Encoding UTF8)) {
        if ($line -notmatch '^\| RK-P') { continue }
        if (-not [regex]::IsMatch($line, $closedPat)) { continue }
        $sm = [regex]::Match($line, '([A-Za-z0-9_\-./]+\.md):(\d+)')
        if (-not $sm.Success) { $d4Bad += (($line -split '\|')[1].Trim() + ' <no file:line source>'); continue }
        $src = $sm.Groups[1].Value; $ln = [int]$sm.Groups[2].Value
        $d4Checked++
        $cand = @("docs/reports/$src", "docs/architecture/$src", "docs/governance/$src", $src)
        $hit = $cand | Where-Object { Test-Path $_ } | Select-Object -First 1
        if (-not $hit) { $d4Bad += ($src + ':' + $ln + ' <source file missing>'); continue }
        $srcLines = @(Get-Content $hit -Encoding UTF8)
        if ($ln -gt $srcLines.Count) { $d4Bad += ($hit + ':' + $ln + ' <line beyond EOF>'); continue }
        $targetLine = $srcLines[$ln - 1]
        # Take ADR refs from the STATUS cell only. Using the whole row is a toothless check:
        # the RK-P23 row mentions ADR-155 (the *registering* ADR) first, and the source line
        # already contains that string, so a never-written-back line would pass. Verified while
        # writing this - the failure mode this whole gate exists to catch.
        $cells = @($line -split '\|' | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne '' })
        $statusCell = if ($cells.Count -ge 2) { $cells[$cells.Count - 1] } else { '' }
        $closingAdrs = @([regex]::Matches($statusCell, 'ADR-\d{3}') | ForEach-Object { $_.Value })
        $okLine = [regex]::IsMatch($targetLine, $closedPat)
        if (-not $okLine) {
            foreach ($a in $closingAdrs) { if ($targetLine.Contains($a)) { $okLine = $true; break } }
        }
        if (-not $okLine) { $d4Bad += ($src + ':' + $ln + ' <closed in register, not written back>') }
    }
    if ($d4Bad.Count -gt 0) {
        $report += '[GAP-D4] closed RK-P items whose source line lacks the write-back (' + $d4Bad.Count + '): ' + (($d4Bad | Select-Object -First 8) -join ' ; ')
        $status = 1
    } else {
        $report += '[D4] closed-item write-back holds (' + $d4Checked + ' closed RK-P rows verified against their source lines)'
    }
} else {
    $report += '[WARN-D4] ' + $rkPath + ' absent, write-back check skipped (register lives outside git per .gitignore *.md)'
}

# D5. mojibake debt scan (frozen per-file baseline, ratchet-only-shrink).
# WHY: 2026-08-31 sweep found CHANGELOG.md carries 94 lines damaged by a UTF-8-read-as-GBK
#      round trip (signature: rare CJK char immediately followed by '?', or U+FFFD). Other
#      core docs measure 0. Repair must be line-by-line against code/ADR, never by guessing
#      glyphs (RK-P40), so the debt is made visible and shrink-only rather than hidden.
# The detector class is written as \uXXXX escapes to keep this file all-ASCII.
$mojiPat = '[\u951b\u9359\u6fb6\u6d60\u95be\u7eeb\u7f01\u6d93\u6c33\u9428\u52ec\u69f8\u7487\u69d1]\?|\ufffd'
$mojiFiles = @(
    'CHANGELOG.md',
    'agents.md',
    '.claude/CLAUDE.md',
    'docs/architecture/adr_index.md',
    'docs/governance/RK-P_risk_register.md',
    'docs/reports/phaseR-wave1-closure.md',
    'docs/reports/phaseR-release-checklist-v2.28.0.md'
)
$mojiBase = @{}
$mojiBasePath = 'scripts/mojibake_baseline.txt'
if (Test-Path $mojiBasePath) {
    foreach ($line in (Get-Content $mojiBasePath -Encoding UTF8)) {
        if ($line -match '^\s*#' -or $line.Trim() -eq '') { continue }
        $parts = $line -split "`t"
        if ($parts.Count -ge 2 -and $parts[0] -match '^\d+$') { $mojiBase[$parts[1].Trim()] = [int]$parts[0] }
    }
}
$d5Bad = @(); $d5Notes = @(); $d5Scanned = 0
foreach ($f in $mojiFiles) {
    if (-not (Test-Path $f)) { continue }   # absent .md is already covered by B/C/D checks
    $d5Scanned++
    $cnt = 0
    foreach ($line in (Get-Content $f -Encoding UTF8)) {
        if ([regex]::IsMatch($line, $mojiPat)) { $cnt++ }
    }
    $cap = if ($mojiBase.ContainsKey($f)) { $mojiBase[$f] } else { 0 }
    if ($cnt -gt $cap) {
        $d5Bad += ($f + ' = ' + $cnt + ' (cap ' + $cap + ')')
    } elseif ($cnt -lt $cap) {
        $d5Notes += ($f + ' shrank to ' + $cnt + ', lower the baseline from ' + $cap)
    }
}
if ($d5Bad.Count -gt 0) {
    $report += '[GAP-D5] mojibake damage exceeds frozen baseline: ' + ($d5Bad -join ' ; ') + ' -- repair by re-verifying each line against code/ADR'
    $status = 1
} else {
    $report += '[D5] mojibake scan clean within baseline (' + $d5Scanned + ' docs scanned, caps from ' + $mojiBasePath + ')'
    foreach ($nt in $d5Notes) { $report += '[D5-INFO] ' + $nt }
}

# =============================================================================
# E. Policy compliance (CONVENTIONS.md declared subdirs + SoT file)
# =============================================================================

# E1. CONVENTIONS.md declared subdirs must exist
# Every check self-reports on the green path as well (same reason as B1/B2): a check that
# is silent when passing cannot be counted, so the footer total was unverifiable by design.
$requiredDirs = @(
    @{ Path = 'docs/architecture/audit'; Purpose = 'audit/governance signoff and review records' },
    @{ Path = 'docs/architecture/governance'; Purpose = 'governance/policy documents' },
    @{ Path = 'docs/architecture/_archive'; Purpose = '_archive/historical snapshots' },
    @{ Path = 'docs/architecture/_blueprints'; Purpose = '_blueprints/design blueprints (not yet implemented)' }
)
$e1Missing = 0
foreach ($d in $requiredDirs) {
    if (-not (Test-Path $d.Path)) {
        # 2026-08-07 适配: 文档目录仅存于本地(gitignore *.md 策略,远程无 md 文件即无目录),
        # 降级为 warn 而非阻断。
        $e1Missing++
        $report += '[E1-warn] CONVENTIONS.md declared subdir missing: ' + $d.Path + ' (' + $d.Purpose + '; gitignore *.md 策略,仅本地维护,跳过)'
    }
}
$report += '[E1] ' + ($requiredDirs.Count - $e1Missing) + '/' + $requiredDirs.Count + ' declared doc subdirs exist (' + $e1Missing + ' warned)'

# E2. SoT policy file existence
if (-not (Test-Path 'docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md')) {
    # 2026-08-07 适配: 同 E1 —— gitignore *.md 策略下仅本地维护,降级为 warn。
    $report += '[E2-warn] missing policy file: docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md (gitignore *.md 策略,仅本地维护,跳过)'
} else {
    $report += '[E2] SoT policy file present: docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md'
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
# Self-reported check count: DERIVED from the ids actually emitted, not a hardcoded total.
# The previous literal ('14 checks') was duplicated across agents.md / CLAUDE.md / ADR-166 /
# two Phase R reports, so every added check silently made six statements stale - the exact
# "declaration without a single source of truth" defect this project keeps registering.
# Consumers must quote "all PASS" and read the count from here, never hardcode it.
$emittedIds = @($report | ForEach-Object {
    if ($_ -match '^\[(?:GAP-|WARN-|ERROR-|)?([A-F]\d[a-z]?)(?:-INFO)?\]') { $Matches[1] }
} | Select-Object -Unique)
$catCount = (@($emittedIds | ForEach-Object { $_[0] } | Select-Object -Unique)).Count
if ($status -eq 0) {
    Write-Host ('[OK] three-way reconciliation all pass (' + $catCount + ' categories / ' + $emittedIds.Count + ' check ids, self-reported): canonical version=' + $currentVersion + ', ' + $nMembers + ' crates, baseline aligned')
} else {
    Write-Host ('[FAIL] three-way reconciliation found gaps (' + $catCount + ' categories / ' + $emittedIds.Count + ' check ids emitted), see [GAP-*] lines above, fix and rerun')
}
exit $status
