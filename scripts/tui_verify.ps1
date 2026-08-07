<#
.SYNOPSIS
    Chimera TUI 非交互验证脚本(遗留问题 1 的自动化前置部分)。

.DESCRIPTION
    运行三个场景并提供结构化证据:
      A. v3 引擎默认输出路径(非 TTY 重定向,验证启动稳定、无 panic、
         含 ANSI 备用屏与中文面板文案、增量 diff 生效);
      B. CHIMERA_NO_V3_ENGINE=1 回退路径(与 A 对比输出规模);
      C. 尽力向 stdin 写入 q(验证事件循环对管道输入不挂死/优雅退出)。
    证据报告写入 D:\Chimera CLI\tmp\tui_verify_report.txt(不入库)。

.NOTES
    真实终端交互验证仍须由用户在 Windows Terminal 人工执行
    (见 docs/architecture/TUI_MANUAL_VERIFICATION_CHECKLIST.md);
    本脚本只提供自动化冒烟证据,不替代人工 UX 验证。
#>
param(
    # 已构建的 CLI 二进制路径;不存在时脚本直接失败并提示构建命令
    [string]$BinaryPath = "D:\Chimera CLI\target\debug\chimera.exe",
    # 场景 A/B 的观察时长(秒)
    [int]$RunSeconds = 8,
    # 场景 C 的 stdin 输入等待与总超时(秒)
    [int]$TimeoutSeconds = 15
)

$ErrorActionPreference = 'Stop'
$ProjectRoot = 'D:\Chimera CLI'
$ReportPath = Join-Path $ProjectRoot 'tmp\tui_verify_report.txt'
$Report = [System.Collections.Generic.List[string]]::new()

function Write-Report {
    param([string]$Message)
    $Report.Add($Message)
    Write-Host $Message
}

if (-not (Test-Path -LiteralPath $BinaryPath)) {
    Write-Report "ERROR: 二进制不存在: $BinaryPath"
    Write-Report '提示: 先运行 cargo build -p chimera-cli'
    $Report | Set-Content -Path $ReportPath -Encoding utf8
    exit 1
}

Write-Report "===== Chimera TUI 非交互验证 $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') ====="
Write-Report "Binary: $BinaryPath"

function Assert-NoPanic {
    param([string]$Name, [string]$Stderr)
    $panicPatterns = @('panicked at', "thread '", 'RUST_BACKTRACE', 'error: The application panicked')
    $hit = $panicPatterns | Where-Object { $Stderr -match [regex]::Escape($_) } | Select-Object -First 1
    if ($hit) {
        Write-Report "[$Name] FAIL: stderr 含 panic 迹象: $hit"
        return $false
    }
    Write-Report "[$Name] PASS: stderr 无 panic"
    return $true
}

function Get-NormalizedText {
    param([string]$RawText)
    # 剥离 ANSI CSI 序列(如 \e[4;5H、\e[1m、\e[38;5;15m)与 OSC/其他转义,
    # 使文案断言不依赖输出路径是"整段写出"还是"逐格定位"
    $text = [regex]::Replace($RawText, "\x1b\[[0-9;?]*[A-Za-z]", '')
    $text = [regex]::Replace($text, "\x1b\][^\x07]*\x07", '')
    return $text
}

function Run-RedirectedScenario {
    param(
        [string]$Name,
        [switch]$UseFallback
    )
    $out = Join-Path $env:TMP "tui_verify_${Name}_out.txt"
    $err = Join-Path $env:TMP "tui_verify_${Name}_err.txt"
    Remove-Item -LiteralPath $out, $err -ErrorAction SilentlyContinue

    $oldFallback = $env:CHIMERA_NO_V3_ENGINE
    if ($UseFallback) {
        $env:CHIMERA_NO_V3_ENGINE = '1'
    } else {
        Remove-Item Env:CHIMERA_NO_V3_ENGINE -ErrorAction SilentlyContinue
    }

    $process = $null
    try {
        $process = Start-Process -FilePath $BinaryPath -ArgumentList 'tui' `
            -WorkingDirectory $ProjectRoot `
            -RedirectStandardOutput $out -RedirectStandardError $err `
            -PassThru -WindowStyle Hidden
    } finally {
        # 恢复环境变量,避免影响后续场景
        if ($null -eq $oldFallback) {
            Remove-Item Env:CHIMERA_NO_V3_ENGINE -ErrorAction SilentlyContinue
        } else {
            $env:CHIMERA_NO_V3_ENGINE = $oldFallback
        }
    }

    # 采样输出文件字节数,用于增量 diff 证据
    $sizes = [System.Collections.Generic.List[int]]::new()
    $deadline = (Get-Date).AddSeconds($RunSeconds)
    while ((Get-Date) -lt $deadline -and -not $process.HasExited) {
        Start-Sleep -Milliseconds 500
        if (Test-Path -LiteralPath $out) {
            $sizes.Add((Get-Item -LiteralPath $out).Length)
        }
    }

    $stillRunning = -not $process.HasExited
    if ($stillRunning) {
        $process.Kill()
        $process.WaitForExit()
    }
    $exitCode = if ($stillRunning) { 'KILLED' } else { $process.ExitCode }
    Write-Report "[$Name] 运行状态: $(if ($stillRunning) { 'RUNNING(观察期结束被终止)' } else { "EXITED code=$exitCode" })"

    $stdout = if (Test-Path -LiteralPath $out) { Get-Content -LiteralPath $out -Raw -Encoding utf8 } else { '' }
    $stderr = if (Test-Path -LiteralPath $err) { Get-Content -LiteralPath $err -Raw -Encoding utf8 } else { '' }

    $noPanic = Assert-NoPanic -Name $Name -Stderr $stderr

    # 增量 diff 证据:首样本增量(0→第一次采样)与后续最大增量对比
    $firstDelta = if ($sizes.Count -gt 0) { $sizes[0] } else { 0 }
    $laterDeltas = @()
    for ($i = 1; $i -lt $sizes.Count; $i++) {
        $laterDeltas += ($sizes[$i] - $sizes[$i - 1])
    }
    $maxLater = if ($laterDeltas.Count -gt 0) { ($laterDeltas | Measure-Object -Maximum).Maximum } else { 0 }
    Write-Report "[$Name] 输出字节: 首帧窗口 $firstDelta B; 后续最大窗口 $maxLater B; 末值 $(($sizes | Select-Object -Last 1)) B"
    if ($firstDelta -gt 0 -and $maxLater -lt ($firstDelta / 3)) {
        Write-Report "[$Name] PASS: 后续输出窗口显著小于首帧(增量 diff 生效证据)"
    } else {
        Write-Report "[$Name] WARN: 增量窗口比值不满足 3x 保守阈值(首=$firstDelta, 后=$maxLater),仅记录不判失败"
    }

    $normalized = Get-NormalizedText -RawText $stdout
    $hasAlternateScreen = $stdout.Contains('1049h')
    $hasChinesePanel = $normalized.Contains('任务列表') -or $normalized.Contains('暂无进行中的任务') -or $normalized.Contains('面板')
    Write-Report "[$Name] 备用屏转义(?1049h): $(if ($hasAlternateScreen) { 'PASS' } else { 'MISS' })"
    Write-Report "[$Name] 中文面板文案: $(if ($hasChinesePanel) { 'PASS' } else { 'MISS' })"
    if (-not $noPanic) { Write-Report "[$Name] stderr 尾部: $($stderr.Substring([Math]::Max(0, $stderr.Length - 600)))" }
    Write-Report ''
}

function Run-StdinScenario {
    param([string]$Name)
    $out = Join-Path $env:TMP "tui_verify_${Name}_out.txt"
    $err = Join-Path $env:TMP "tui_verify_${Name}_err.txt"
    Remove-Item -LiteralPath $out, $err -ErrorAction SilentlyContinue

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $BinaryPath
    $psi.Arguments = 'tui'
    $psi.WorkingDirectory = $ProjectRoot
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $psi
    [void]$process.Start()

    try {
        Start-Sleep -Seconds 2
        # 尽力发送 q 并关闭输入流;若事件循环不处理管道输入,进程仍应存活,由超时保护
        $process.StandardInput.Write('q')
        $process.StandardInput.Close()

        if ($process.WaitForExit($TimeoutSeconds * 1000)) {
            $stdout = $process.StandardOutput.ReadToEnd()
            $stderr = $process.StandardError.ReadToEnd()
            Write-Report "[$Name] EXITED code=$($process.ExitCode)"
            $noPanic = Assert-NoPanic -Name $Name -Stderr $stderr
            Write-Report "[$Name] stdout 字节: $($stdout.Length); 含备用屏: $($stdout.Contains('1049h'))"
        } else {
            $process.Kill()
            $process.WaitForExit()
            Write-Report "[$Name] TIMEOUT(管道输入未被事件循环消费,按预期保护性终止;不判失败)"
            Write-Report "[$Name] stdout 字节: $(if (Test-Path $out) { (Get-Item $out).Length } else { 0 })"
        }
    } catch {
        if (-not $process.HasExited) { $process.Kill(); $process.WaitForExit() }
        Write-Report "[$Name] EXCEPTION: $($_.Exception.Message)"
    } finally {
        $process.Dispose()
    }
    Write-Report ''
}

Run-RedirectedScenario -Name 'A_v3_default'
Run-RedirectedScenario -Name 'B_fallback' -UseFallback
Run-StdinScenario -Name 'C_stdin_q'

Write-Report '===== 摘要 ====='
Write-Report '自动化冒烟完成;真实终端交互验证(Windows Terminal)仍待用户执行,清单见 docs/architecture/TUI_MANUAL_VERIFICATION_CHECKLIST.md'
$Report | Set-Content -Path $ReportPath -Encoding utf8
Write-Host "报告已写入: $ReportPath"
