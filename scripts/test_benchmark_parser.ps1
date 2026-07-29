# P1-7 Parser Unit Test - ASCII only to avoid GBK encoding issues
# Tests the criterion output parsing logic from check_repo_wiki_benchmark.ps1
# Exit code: 0 = all pass / 1 = at least one fail

$ErrorActionPreference = 'Stop'

# Mock criterion output (multi-line format: name on one line, time: on next)
$mockOutput = @"
hnsw_10k_search_latency/knn_top5
                        time:   [48.820 us 49.163 us 49.559 us]
Found 9 outliers among 100 measurements (9.00%)
hnsw_10k_p95_search_latency/p95_latency
                        time:   [98.123 us 102.456 us 110.789 us]
Found 5 outliers among 100 measurements (5.00%)
single_thread_knn_latency/100
                        time:   [8.123 us 9.456 us 11.789 us]
Found 3 outliers among 100 measurements (3.00%)
single_thread_knn_latency/1000
                        time:   [88.123 us 95.456 us 105.789 us]
Found 7 outliers among 100 measurements (7.00%)
super_slow_benchmark/slow_case
                        time:   [15.000 ms 25.000 ms 35.000 ms]
Found 2 outliers among 100 measurements (2.00%)
"@

# Threshold table (matches check_repo_wiki_benchmark.ps1)
# WHY sorted by Name length descending: avoids short prefix regex match
# e.g. 'single_thread_knn_latency/100' would match 'single_thread_knn_latency/1000'
# via -match (contains semantics), giving wrong threshold (5ms instead of 20ms)
$Thresholds = @(
    @{ Name = 'hnsw_10k_p95_search_latency';     ThresholdMs = 20.0 }
    @{ Name = 'hnsw_10k_search_latency';          ThresholdMs = 10.0 }
    @{ Name = 'single_thread_knn_latency/1000';   ThresholdMs = 20.0 }
    @{ Name = 'single_thread_knn_latency/100';    ThresholdMs = 5.0 }
    @{ Name = 'super_slow_benchmark';             ThresholdMs = 10.0 }
) | Sort-Object { -($_.Name.Length) }

# Parsing logic (extracted from check_repo_wiki_benchmark.ps1)
$lines = $mockOutput -split "`n"
$currentBenchName = $null
$timePattern = 'time:\s+\[([\d.]+)\s+(\S+)\s+([\d.]+)\s+(\S+)\s+([\d.]+)\s+(\S+)\]'
$results = @()

for ($i = 0; $i -lt $lines.Count; $i++) {
    $line = $lines[$i].TrimEnd()
    if ([string]::IsNullOrWhiteSpace($line)) { continue }

    # Check if this is a benchmark name line (not indented, not a time/change/Found/etc line)
    $isNameLine = (-not $line.StartsWith(' ')) -and
                  (-not $line.StartsWith([char]9)) -and
                  ($line -notmatch 'time:') -and
                  ($line -notmatch 'change:') -and
                  ($line -notmatch 'Performance') -and
                  ($line -notmatch 'Found') -and
                  ($line -notmatch 'setting') -and
                  ($line -notmatch 'Benchmarking') -and
                  ($line -notmatch 'Gnuplot') -and
                  ($line -notmatch 'Running') -and
                  ($line -notmatch 'Compiling') -and
                  ($line -notmatch 'Finished') -and
                  ($line -notmatch '^\[')

    if ($isNameLine) {
        $currentBenchName = $line.Trim()
        continue
    }

    # Check if this is a time: line
    if ($line -match $timePattern -and $currentBenchName) {
        $benchName = $currentBenchName
        $meanStr = $Matches[3]
        $unit = $Matches[4]

        # Convert to milliseconds
        $meanMs = $null
        switch ($unit) {
            'ns'  { $meanMs = [double]$meanStr / 1000000.0 }
            { $_ -eq 'us' -or $_ -eq ([char]0xB5 + 's') } { $meanMs = [double]$meanStr / 1000.0 }
            'ms'  { $meanMs = [double]$meanStr }
            's'   { $meanMs = [double]$meanStr * 1000.0 }
            default { continue }
        }

        # Find matching threshold
        $matchedThreshold = $null
        foreach ($t in $Thresholds) {
            if ($benchName -match $t.Name) {
                $matchedThreshold = $t
                break
            }
        }
        if (-not $matchedThreshold) { continue }

        $passed = $meanMs -lt $matchedThreshold.ThresholdMs
        $results += [PSCustomObject]@{
            Name = $benchName
            MeanMs = $meanMs
            ThresholdMs = $matchedThreshold.ThresholdMs
            Passed = $passed
        }
    }
}

# ============================================================
# Assertions
# ============================================================

$testFailures = 0
Write-Host "=== P1-7 Parser Unit Test ==="
Write-Host ""

# Test 1: hnsw_10k_search_latency/knn_top5 - 49.163 us = 0.049163 ms < 10 ms (PASS)
$t = $results | Where-Object { $_.Name -eq 'hnsw_10k_search_latency/knn_top5' }
if ($t -and [Math]::Abs($t.MeanMs - 0.049163) -lt 0.0001 -and $t.Passed -eq $true) {
    Write-Host ("[PASS] Test 1: {0} = {1:F6} ms < {2} ms" -f $t.Name, $t.MeanMs, $t.ThresholdMs)
} else {
    Write-Host ("[FAIL] Test 1: expected 0.049163 ms Passed=true, got {0}" -f $(if ($t) {"$($t.MeanMs) ms Passed=$($t.Passed)"} else {"NOT FOUND"}))
    $testFailures++
}

# Test 2: hnsw_10k_p95_search_latency/p95_latency - 102.456 us = 0.102456 ms < 20 ms (PASS)
$t = $results | Where-Object { $_.Name -eq 'hnsw_10k_p95_search_latency/p95_latency' }
if ($t -and [Math]::Abs($t.MeanMs - 0.102456) -lt 0.0001 -and $t.Passed -eq $true) {
    Write-Host ("[PASS] Test 2: {0} = {1:F6} ms < {2} ms" -f $t.Name, $t.MeanMs, $t.ThresholdMs)
} else {
    Write-Host ("[FAIL] Test 2: expected 0.102456 ms Passed=true, got {0}" -f $(if ($t) {"$($t.MeanMs) ms Passed=$($t.Passed)"} else {"NOT FOUND"}))
    $testFailures++
}

# Test 3: single_thread_knn_latency/100 - 9.456 us = 0.009456 ms < 5 ms (PASS)
$t = $results | Where-Object { $_.Name -eq 'single_thread_knn_latency/100' }
if ($t -and [Math]::Abs($t.MeanMs - 0.009456) -lt 0.0001 -and $t.Passed -eq $true) {
    Write-Host ("[PASS] Test 3: {0} = {1:F6} ms < {2} ms" -f $t.Name, $t.MeanMs, $t.ThresholdMs)
} else {
    Write-Host ("[FAIL] Test 3: expected 0.009456 ms Passed=true, got {0}" -f $(if ($t) {"$($t.MeanMs) ms Passed=$($t.Passed)"} else {"NOT FOUND"}))
    $testFailures++
}

# Test 4: single_thread_knn_latency/1000 - 95.456 us = 0.095456 ms < 20 ms (PASS)
# CRITICAL: Must use 20ms threshold (not 5ms) - verifies sort fix for prefix matching
$t = $results | Where-Object { $_.Name -eq 'single_thread_knn_latency/1000' }
if ($t -and [Math]::Abs($t.MeanMs - 0.095456) -lt 0.0001 -and $t.Passed -eq $true -and $t.ThresholdMs -eq 20.0) {
    Write-Host ("[PASS] Test 4: {0} = {1:F6} ms < {2} ms (correct threshold)" -f $t.Name, $t.MeanMs, $t.ThresholdMs)
} else {
    $detail = if ($t) {"$($t.MeanMs) ms Passed=$($t.Passed) Threshold=$($t.ThresholdMs)"} else {"NOT FOUND"}
    Write-Host ("[FAIL] Test 4: expected 0.095456 ms Passed=true Threshold=20, got {0}" -f $detail)
    $testFailures++
}

# Test 5: super_slow_benchmark/slow_case - 25.000 ms >= 10 ms (FAIL - correctly detected)
$t = $results | Where-Object { $_.Name -eq 'super_slow_benchmark/slow_case' }
if ($t -and $t.MeanMs -eq 25.0 -and $t.Passed -eq $false) {
    Write-Host ("[PASS] Test 5: {0} = {1:F6} ms >= {2} ms (correctly detected threshold violation)" -f $t.Name, $t.MeanMs, $t.ThresholdMs)
} else {
    Write-Host ("[FAIL] Test 5: expected 25.000000 ms Passed=false, got {0}" -f $(if ($t) {"$($t.MeanMs) ms Passed=$($t.Passed)"} else {"NOT FOUND"}))
    $testFailures++
}

# Test 6: Verify all 5 benchmarks were parsed
if ($results.Count -eq 5) {
    Write-Host ("[PASS] Test 6: All 5 benchmarks parsed correctly")
} else {
    Write-Host ("[FAIL] Test 6: Expected 5 parsed results, got {0}" -f $results.Count)
    $testFailures++
}

# ============================================================
# Summary
# ============================================================

Write-Host ""
Write-Host "=== Summary ==="
Write-Host "Parsed results: $($results.Count)"
Write-Host "Test failures:  $testFailures"

if ($testFailures -eq 0) {
    Write-Host ""
    Write-Host "[PASS] All 6 tests passed - parsing logic is correct"
    exit 0
} else {
    Write-Host ""
    Write-Host "[FAIL] $testFailures test(s) failed"
    exit 1
}
