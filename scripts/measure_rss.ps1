# measure_rss.ps1 - peak RSS sampler for fixed scenarios (R12 D0-3 / memory gate)
#
# Why a local script: Windows has no /usr/bin/time -v and Task Manager is not scriptable.
# PeakWorkingSet64 is the process-lifetime peak; polling and taking max yields the peak.
#
# Usage:
#   pwsh scripts/measure_rss.ps1 -Name plan64 -Executable "target/debug/chimera.exe" -Arguments "exec --plan tmp/plan64.json --plan-readonly"
#   pwsh scripts/measure_rss.ps1 -Name tui10m -Executable "target/debug/chimera.exe" -Arguments "tui" -TimeoutSec 600
#
# Output: tmp/rss_<Name>.json plus a console summary. Three runs are used to judge reproducibility.

param(
    [Parameter(Mandatory = $true)][string]$Name,
    [Parameter(Mandatory = $true)][string]$Executable,
    [string]$Arguments = "",
    [int]$TimeoutSec = 600,
    [string]$OutDir = "tmp"
)

$ErrorActionPreference = "Stop"
if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Force -Path $OutDir | Out-Null }

$sw = [System.Diagnostics.Stopwatch]::StartNew()
$proc = Start-Process -FilePath $Executable -ArgumentList $Arguments -PassThru -WindowStyle Hidden
$peak = 0L
$samples = 0

while (-not $proc.HasExited -and $sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
    try {
        $proc.Refresh()
        if ($proc.PeakWorkingSet64 -gt $peak) { $peak = $proc.PeakWorkingSet64 }
        $samples++
    } catch {
        # Refresh can throw at process exit; treat it as exit.
    }
    Start-Sleep -Milliseconds 100
}

$status = if ($proc.HasExited) { "exited" } else { "timeout" }
if (-not $proc.HasExited) {
    try { $proc.Kill() } catch { }
    try { $proc.WaitForExit(5000) | Out-Null } catch { }
}
$exitCode = $null
try { $exitCode = $proc.ExitCode } catch { }
$sw.Stop()

$result = [ordered]@{
    name              = $Name
    executable        = $Executable
    arguments         = $Arguments
    peak_rss_bytes    = $peak
    peak_rss_mb       = [Math]::Round($peak / 1MB, 2)
    duration_s        = [Math]::Round($sw.Elapsed.TotalSeconds, 3)
    poll_samples      = $samples
    exit_code         = $exitCode
    status            = $status
    captured_at_local = (Get-Date).ToString("o")
}

$safeName = $Name -replace '[^A-Za-z0-9_.-]', '_'
$outFile = Join-Path $OutDir ("rss_" + $safeName + ".json")
$result | ConvertTo-Json -Depth 4 | Set-Content -Path $outFile -Encoding UTF8
Write-Output ("RSS {0}: peak={1} MB status={2} exit={3} duration={4}s -> {5}" -f `
        $Name, $result.peak_rss_mb, $status, $exitCode, $result.duration_s, $outFile)
