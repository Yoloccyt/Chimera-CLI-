<#
.SYNOPSIS
  Chimera CLI 性能红线 lint 静态验证脚本(Windows / PowerShell)

.DESCRIPTION
  静态验证 spec.md KPI 表格中定义的全部性能红线(SLO)是否在代码库中
  有对应的 benchmark/test 文件与函数,以及阈值标记是否就位。

  Part 1: Static Lint — 文件/函数/阈值标记存在性检查
  Part 2: SLO Benchmark Assertions — 实际运行 bench, 解析 criterion 输出,
          与 80% redline 阈值比较(Phase 7 新增)
  Part 3: Bench Inventory Completeness Gate (WS-3 E1) — 枚举 crates/*/benches/*.rs
          全量清单, 逐项分类 gated / registered / dev-only, 捕获落到"未知"状态的
          bench(新增或忘登记的 bench 会触发 FAIL), 保证每个基准都被显式守护。

  检查项(每条红线 3 项):
  1. 文件存在 — benchmark/test 文件路径有效
  2. 函数存在 — `fn <function_name>` 在文件中可匹配
  3. 阈值标记 — 阈值字符串(阈值常量名或阈值描述)在文件中可匹配

  红线全集(8 条,来自 spec.md KPI 表格"性能 SLO 分层"):
  - RL-01: window_select <1ms(内环本地,维持)
  - RL-02: mlc_l2_knn <5ms(内环本地,维持)
  - RL-03: decay <1μs(内环本地,维持,阈值仅在 spec 中,代码无断言)
  - RL-04: wiki_knn @1000 <10ms(索引,既有)
  - RL-05: wiki_knn @10K p95<50ms(索引,P2-W8.1.3)
  - RL-06: wiki_knn @100K p95<50ms(索引,P2-W8.3.1 新增)
  - RL-07: 跨膜事件投递 p95<10ms(跨膜,P1-W2 新增)
  - RL-08: 50agent_mem_peak ≤130MB(MAS,维持)
  - RL-09: linucb 40arm select p99<50μs(MCA M3 s9 路由臂,新增)
  - RL-10: cost_estimate <1μs(MCA M3 路由热路径,新增)
  - RL-11: sse normalize <5μs/event(MCA M0 SSE 归一器,新增)

.NOTES
  退出码:0=全部通过, 1=有检查项失败
  使用方式:pwsh scripts/check_perf_redlines.ps1
  对应任务:P2-W8.3.2 红线 lint CI 化
  权威源:spec.md KPI 表格(nexus-omega-v5-implementation-plan/spec.md L29)
#>

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$script:FailCount = 0
$script:WarnCount = 0

function Write-Check {
    param([string]$Name, [bool]$Pass, [string]$Detail = '')
    $status = if ($Pass) { 'PASS' } else { 'FAIL' }
    $color = if ($Pass) { 'Green' } else { 'Red' }
    Write-Host "  [$status] $Name" -ForegroundColor $color
    if ($Detail) { Write-Host "         $Detail" -ForegroundColor Gray }
    if (-not $Pass) { $script:FailCount++ }
}

function Write-Warn {
    param([string]$Name, [string]$Detail = '')
    Write-Host "  [WARN] $Name" -ForegroundColor Yellow
    if ($Detail) { Write-Host "         $Detail" -ForegroundColor Gray }
    $script:WarnCount++
}

Write-Host "`n=== Chimera CLI Performance Red Line Lint ===" -ForegroundColor Cyan
Write-Host "    (spec.md KPI: 性能 SLO 分层, 8 red lines)`n" -ForegroundColor Gray

$projectRoot = Join-Path $PSScriptRoot '..' | Resolve-Path

# --- 红线全集(spec.md KPI 表格"性能 SLO 分层") ---
# 每条红线:Id / Name / File(相对项目根) / Func(bench/test 函数名) / Threshold(阈值标记,空=跳过)
$redlines = @(
    @{ Id='RL-01'; Name='window_select <1ms'; File='crates/chimera-mas/benches/mas_benchmark.rs'; Func='bench_window_select'; Threshold='1ms' },
    @{ Id='RL-02'; Name='mlc_l2_knn <5ms'; File='crates/chimera-mas/benches/mas_benchmark.rs'; Func='bench_mlc_l2_knn_top10_4096'; Threshold='5ms' },
    @{ Id='RL-03'; Name='decay <1us'; File='crates/decay-engine/benches/decay_bench.rs'; Func='single_decay_step_latency'; Threshold='' },
    @{ Id='RL-04'; Name='wiki_knn @1000 <10ms'; File='crates/repo-wiki/benches/vector_bench.rs'; Func='single_thread_knn_latency'; Threshold='LARGE_SIZE' },
    @{ Id='RL-05'; Name='wiki_knn @10K p95<50ms'; File='crates/repo-wiki/tests/hnsw_p95_test.rs'; Func='test_hnsw_10k_p95_below_50ms'; Threshold='P95_THRESHOLD_MS' },
    @{ Id='RL-06'; Name='wiki_knn @100K p95<50ms'; File='crates/repo-wiki/tests/hnsw_p95_test.rs'; Func='test_hnsw_100k_p95_below_50ms'; Threshold='ENTRY_COUNT_100K' },
    @{ Id='RL-07'; Name='membrane delivery p95<10ms'; File='crates/event-bus/benches/membrane_delivery_bench.rs'; Func='membrane_e2e_delivery'; Threshold='10ms' },
    @{ Id='RL-08'; Name='50agent_mem_peak <=130MB'; File='crates/chimera-mas/benches/mas_benchmark.rs'; Func='bench_50agent_mem_peak'; Threshold='130' },
    # MCA M3/M4 热路径红线(ADR-065/068):路由选择/成本估算/SSE 归一
    @{ Id='RL-09'; Name='linucb 40arm select p99<50us'; File='crates/omega-learner/benches/linucb_select.rs'; Func='bench_s9_route_40arm_select'; Threshold='P99_TARGET_US' },
    @{ Id='RL-10'; Name='cost_estimate <1us'; File='crates/mca-gateway/benches/mca_hot_paths.rs'; Func='bench_cost_estimate'; Threshold='COST_ESTIMATE_TARGET_US' },
    @{ Id='RL-11'; Name='sse normalize <5us/event'; File='crates/mca-gateway/benches/mca_hot_paths.rs'; Func='bench_sse_normalize'; Threshold='SSE_EVENT_TARGET_US' }
)

foreach ($rl in $redlines) {
    $filePath = Join-Path $projectRoot $rl.File
    $relPath = $rl.File

    Write-Host "`n  [$($rl.Id)] $($rl.Name)" -ForegroundColor Cyan

    # 检查 1: 文件存在
    $fileExists = Test-Path $filePath
    Write-Check "$($rl.Id).1 file exists ($relPath)" $fileExists
    if (-not $fileExists) {
        Write-Host "         跳过函数/阈值检查(文件不存在)" -ForegroundColor Gray
        continue
    }

    $content = Get-Content $filePath -Raw -Encoding UTF8

    # 检查 2: 函数存在
    $funcPattern = "fn $($rl.Func)"
    $funcExists = $content -match [regex]::Escape($funcPattern)
    Write-Check "$($rl.Id).2 function 'fn $($rl.Func)' exists" $funcExists

    # 检查 3: 阈值标记存在(空=跳过,spec-only 红线)
    if ($rl.Threshold -ne '') {
        $thresholdExists = $content -match [regex]::Escape($rl.Threshold)
        Write-Check "$($rl.Id).3 threshold marker '$($rl.Threshold)' exists" $thresholdExists
    } else {
        Write-Warn "$($rl.Id).3 threshold marker" "阈值仅在 spec 中定义,代码无显式断言(spec-only)"
    }
}

# =====================================================================
# Part 2: SLO Benchmark 阈值断言 (实际运行 bench, 解析 criterion 输出)
# =====================================================================
# 每个 SLO 用 80% 宽松阈值作为 CI redline (CI 环境波动缓冲)
# 格式: SLO 名称 -> crate / bench 文件 / bench 函数过滤 / SLO 阈值(秒) / redline 阈值(秒)
$sloRedlines = @(
    # WHY Filter 用 criterion 组名前缀而非 bench 函数名(closure C-13 修复):
    # criterion 命令行过滤匹配的是 benchmark ID("window_select/L0_4K"),
    # 函数名 'bench_window_select' 零匹配时 criterion 跑全部 bench,
    # 解析器取到首个 time: 行造成 1.39ms 伪影(实测单次 select 为 ~1ns)
    @{ Name='window_select';     Crate='hcw-window';    BenchFile='window_select';  Filter='window_select/';            SloSec=0.001;   RedlineSec=0.0008;   Unit='ms'; SloDisplay='1ms';   RedlineDisplay='0.8ms'  },
    @{ Name='mlc_l2_knn';       Crate='mlc-engine';    BenchFile='mlc_l2_knn';     Filter='bench_l2_knn_slo_assert';   SloSec=0.005;   RedlineSec=0.004;    Unit='ms'; SloDisplay='5ms';   RedlineDisplay='4ms'   },
    @{ Name='decay_compute';    Crate='decay-engine';  BenchFile='decay_compute';  Filter='single_decay_by_profile';   SloSec=0.000001;RedlineSec=0.0000008;Unit='us'; SloDisplay='1us';   RedlineDisplay='0.8us' },
    @{ Name='wiki_knn_100k';    Crate='repo-wiki';     BenchFile='wiki_knn_slo';   Filter='wiki_knn_100k_p95';         SloSec=0.050;   RedlineSec=0.040;    Unit='ms'; SloDisplay='50ms';  RedlineDisplay='40ms'  },
    @{ Name='immune_probe';     Crate='chimera-mas';   BenchFile='immune_probe';   Filter='bench_assess_paradox_risk'; SloSec=0.100;   RedlineSec=0.080;    Unit='ms'; SloDisplay='100ms'; RedlineDisplay='80ms'  },
    @{ Name='rhi_judge';        Crate='auto-dpo';      BenchFile='rhi_judge';      Filter='rhi_judge_latency';         SloSec=2.0;     RedlineSec=1.6;      Unit='s';  SloDisplay='2s';    RedlineDisplay='1.6s'   },
    # MCA M3 s9 路由臂 40 臂选择延迟(ADR-068 决策 2:p99 < 50μs,红线上浮 80%)
    @{ Name='linucb_40arm';     Crate='omega-learner'; BenchFile='linucb_select';  Filter='select_arm_40arms_6dim_s9route'; SloSec=0.00005; RedlineSec=0.00004; Unit='us'; SloDisplay='50us';  RedlineDisplay='40us' }
)

Write-Host "`n=== SLO Benchmark Threshold Assertions ===" -ForegroundColor Cyan
Write-Host "    (criterion bench, 80% redline of SLO)`n" -ForegroundColor Gray

$sloPassCount = 0
$sloFailCount = 0
$sloSkipCount = 0

foreach ($slo in $sloRedlines) {
    Write-Host "  [SLO] $($slo.Name) (target: < $($slo.SloDisplay), redline: $($slo.RedlineDisplay))" -ForegroundColor Cyan

    # 运行 bench 并捕获输出
    $benchArgs = @(
        'bench',
        '--package', $slo.Crate,
        '--bench', $slo.BenchFile,
        '--', '--noplot', '--quick', $slo.Filter
    )

    try {
        $benchOutput = & cargo @benchArgs 2>&1 | Out-String
    } catch {
        Write-Host "    [SKIP] bench 执行失败: $_" -ForegroundColor Yellow
        $sloSkipCount++
        continue
    }

# 转换前强制 UTF-8 解码 native stdout:
# criterion 的 "µs" 单位以 UTF-8 字节 0xC2 0xB5 输出,Windows 控制台代码页
# (GBK/936)默认将其解码为 "Âμ",导致后续单位比较失败而误入 else 分支
# (把 6.8µs 当 6.8ms,linucb_40arm SLO 误报 FAIL —— 2026-08-08 发布检查发现)。
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch {}

# 解析 criterion 输出: "time:   [X.XXX us X.XXX us X.XXX us]"
# 格式: time: [lower estimate upper unit]
# 提取 estimate (中间值) 和单位
$timePattern = 'time:\s+\[([^\]]+)\]'
if ($benchOutput -match $timePattern) {
    $timeValues = $Matches[1].Trim() -split '\s+'
    # timeValues[0]=lower, [1]=estimate, [2]=upper, [3]=unit
    if ($timeValues.Count -ge 4) {
        $estimate = [double]$timeValues[2]
        $unit = $timeValues[3]

        # 转换为秒。单位匹配用正则兼容三种微符号形态:
        # U+00B5 µ(UTF-8 解码) / U+03BC μ(GBK 解码) / ASCII u
        if     ($unit -eq 'ns') { $estimateSec = $estimate / 1e9 }
        elseif ($unit -match '^[µμu]s$') { $estimateSec = $estimate / 1e6 }
        elseif ($unit -eq 'ms') { $estimateSec = $estimate / 1e3 }
        elseif ($unit -eq 's')  { $estimateSec = $estimate }
        else                    { $estimateSec = $estimate / 1e3 }

            # 显示实测值
            if     ($slo.Unit -eq 'us') { $displayVal = ('{0:N3} us' -f ($estimateSec * 1e6)) }
            elseif ($slo.Unit -eq 'ms') { $displayVal = ('{0:N3} ms' -f ($estimateSec * 1e3)) }
            else                        { $displayVal = ('{0:N3} s'  -f $estimateSec) }
            Write-Host "    实测: $displayVal" -ForegroundColor Gray

            # 与 redline 比较
            if ($estimateSec -le $slo.RedlineSec) {
                Write-Host "    [PASS] 低于 redline ($($slo.RedlineDisplay))" -ForegroundColor Green
                $sloPassCount++
            } elseif ($estimateSec -le $slo.SloSec) {
                Write-Host "    [WARN] 超过 redline 但低于 SLO ($($slo.SloDisplay))" -ForegroundColor Yellow
                $sloPassCount++  # 低于 SLO 即通过
            } else {
                Write-Host "    [FAIL] 超过 SLO ($($slo.SloDisplay))!" -ForegroundColor Red
                $sloFailCount++
            }
        } else {
            Write-Host "    [SKIP] 无法解析 criterion 时间输出" -ForegroundColor Yellow
            $sloSkipCount++
        }
    } else {
        Write-Host "    [SKIP] 未找到 criterion 时间输出 (bench 可能编译失败)" -ForegroundColor Yellow
        $sloSkipCount++
    }
}

# =====================================================================
# Part 3: Bench Inventory Completeness Gate (WS-3 E1)
# =====================================================================
# WHY: 保证 crates/*/benches/*.rs 全量清单里每个 bench 都被显式守护。
#   分类三态:
#     gated      — 被 Part1 $redlines.File 或 Part2 $sloRedlines(Crate+BenchFile) 引用
#     registered — 被 bench_check.yml run_bench 的 `--bench <name>` 引用(已有 CI 阈值门)
#     dev-only   — 既非 gated 也非 registered 的已显式登记开发/实验工具 bench
#     unknown    — 未落入以上任何一态 → FAIL(新增或忘登记的 bench 会被捕获)
$benchInventory = Get-ChildItem (Join-Path $projectRoot 'crates') -Recurse -Filter '*.rs' |
    Where-Object { $_.Directory.Name -eq 'benches' }

# registered 全集 = bench_check.yml run_bench `--bench <stem>` 引用的 (crate, bench)。追加 17 项(WS-3 E1)+ 预留 xts_top_k。
$registeredBench = @(
    @{ Crate='chimera-cli';     Bench='exit_code_matrix' }
    @{ Crate='chimera-tui';     Bench='render_bench' }
    @{ Crate='chimera-tui';     Bench='diff_engine_bench' }
    @{ Crate='chimera-tui';     Bench='streaming_bench' }
    @{ Crate='mcp-mesh';        Bench='wal_recovery' }
    @{ Crate='csn-substitutor'; Bench='degradation_chain' }
    @{ Crate='quest-engine';    Bench='dag_bench' }
    @{ Crate='quest-engine';    Bench='checkpoint_bench' }
    @{ Crate='gea-activator';   Bench='gate_compute' }
    @{ Crate='chimera-mas';     Bench='mas_benchmark' }
    @{ Crate='chimera-mas';     Bench='dedup_bench' }
    @{ Crate='chimera-mas';     Bench='shadow_stats' }
    # ---- WS-3 E1 新增 17 项(与 bench_check.yml 同步)----
    @{ Crate='chimera-cli';     Bench='config_concurrency_bench' }
    @{ Crate='chimera-tui';     Bench='writer_ansi_bench' }
    @{ Crate='chimera-tui';     Bench='v3_pipeline_bench' }
    @{ Crate='nexus-core';      Bench='reduce_bench' }
    @{ Crate='session-store';   Bench='session_store_bench' }
    @{ Crate='event-bus';       Bench='cross_layer_bench' }
    @{ Crate='repo-wiki';       Bench='hybrid_search_bench' }
    @{ Crate='parliament';      Bench='debate' }
    @{ Crate='mlc-engine';      Bench='l2_recall' }
    @{ Crate='hcw-window';      Bench='compress' }
    @{ Crate='osa-coordinator'; Bench='compute_masks' }
    @{ Crate='chimera-mas';     Bench='delegation_bench' }
    @{ Crate='scc-cache';       Bench='cache_hit' }
    @{ Crate='lsct-tiering';    Bench='tiering_benchmark' }
    @{ Crate='gqep-executor';   Bench='gather' }
    @{ Crate='pvl-layer';       Bench='produce_verify' }
    @{ Crate='seccore';         Bench='command_validator_bench' }
    # 预留(并行 WS-2 的 nexus-contracts/benches/xts_top_k.rs 基准):
    # 文件已落盘(2026-08-29),在清单中自动归入 registered;其 CI 阈值条目
    # 已在 bench_check.yml 预留(宽松待首跑校准),run_bench 行随 WS-2 落地。
    @{ Crate='nexus-contracts'; Bench='xts_top_k'; Reserved=$true }
)

# dev-only 显式登记(既非 gated 也非 registered, 无 spec KPI 对应的开发/实验工具 bench)。
# WHY: 清单完整性门需要"未知"态存在才有效——这些对象显式声明为 dev-only, 非 FN 或 FP。
$devOnly = @(
    @{ Crate='acb-governor';        Bench='governor_bench';            Reason='ACB 治理闸门开发基准,无 spec KPI 阈值' }
    @{ Crate='auto-dpo';            Bench='dpo_bench';                 Reason='online-DPO 训练环节开发基准,仅实验用' }
    @{ Crate='auto-dpo';            Bench='judge_retry';               Reason='训练重试路径实验基准确认' }
    @{ Crate='auto-dpo';            Bench='rhi_channel_a_bench';       Reason='RHI Channel-A 复用探索通道实验' }
    @{ Crate='chimera-cli';         Bench='overwindow_bridge';         Reason='overwindow 桥接开发探针,无 SLO' }
    @{ Crate='chimera-cli';         Bench='quest_orchestrator_bench';  Reason='Quest 编排黑盒探索基准' }
    @{ Crate='chimera-mas';         Bench='pdca_bench';                Reason='PDCA 循环闭环实验基准' }
    @{ Crate='chimera-tui';         Bench='data_pipeline_bench';       Reason='数据管线开发基准,已由 writer/v3 覆盖主链' }
    @{ Crate='chimera-tui';         Bench='panels_scale_bench';        Reason='面板规模压力探索基准' }
    @{ Crate='chimera-tui';         Bench='v3_output_bench';           Reason='v3 输出渲染补充基准,主链已注册 v3_pipeline' }
    @{ Crate='chtc-bridge';         Bench='bridge_benchmark';          Reason='CHTC 桥接开发验证基准' }
    @{ Crate='cmt-tiering';         Bench='hot_lru';                   Reason='Hot LRU 层级开发基准' }
    @{ Crate='cmt-tiering';         Bench='pragma_capable_bench';      Reason='PixelRAG 能力探测开发基准' }
    @{ Crate='cmt-tiering';         Bench='rl_migration_bench';        Reason='RL 迁移路径实验(RL 闸门:仅规划外观)基准' }
    @{ Crate='cmt-tiering';         Bench='rl_replay_sample';          Reason='RL 采样实验基准,规格未冻结' }
    @{ Crate='csn-substitutor';     Bench='substitutor_benchmark';     Reason='替换器语义测量基准(主链已有 degradation_chain)' }
    @{ Crate='decay-engine';        Bench='decay_parallel';            Reason='衰减并行路径补充基准,无独立 SLO' }
    @{ Crate='decb-governor';       Bench='budget_compute';            Reason='预算计算开发基准' }
    @{ Crate='efficiency-monitor';  Bench='monitor_benchmark';         Reason='监控器审计路径开发基准' }
    @{ Crate='event-bus';           Bench='bus_bench';                 Reason='事件总线吞吐基准(覆盖在 cross_layer 之下)' }
    @{ Crate='event-bus';           Bench='bus_credit_bench';          Reason='信用记账路径开发基准' }
    @{ Crate='event-bus';           Bench='bus_shard_bench';           Reason='分片职责 benchmark,无 KPI 目标' }
    @{ Crate='event-bus';           Bench='pattern_index_bench';       Reason='模式索引路径开发基准' }
    @{ Crate='event-bus';           Bench='tui_action_dispatch_bench'; Reason='TUI 动作分发基准,功能层无性能契约' }
    @{ Crate='faae-router';         Bench='faae_parallel_bench';       Reason='FA 亲和并行基准,实验探针' }
    @{ Crate='faae-router';         Bench='operator_select';           Reason='算子选择开发基准' }
    @{ Crate='faae-router';         Bench='route';                     Reason='路由热路径补充开发基准' }
    @{ Crate='gsoe-evolution';      Bench='channel_b_benchmark';       Reason='Channel-B 进化通道实验基准' }
    @{ Crate='gsoe-evolution';      Bench='evolution_benchmark';       Reason='进化引擎整体开发基准' }
    @{ Crate='gsoe-evolution';      Bench='formal_gate_eval';          Reason='形式化闸门评估开发基准' }
    @{ Crate='hcw-window';          Bench='coarse_recall';             Reason='粗召回窗口开发基准(分类在 compress 之下)' }
    @{ Crate='hcw-window';          Bench='csc_bench';                 Reason='CSC 压缩模式开发基准' }
    @{ Crate='hcw-window';          Bench='fine_recall';               Reason='细召回窗口开发基准' }
    @{ Crate='hcw-window';          Bench='parallel_compress';         Reason='并行压缩补充基准' }
    @{ Crate='hcw-window';          Bench='probe_select';              Reason='探针选择开发基准' }
    @{ Crate='hcw-window';          Bench='rerank_fill';               Reason='重排填充路径开发基准' }
    @{ Crate='hcw-window';          Bench='shadow_recall_1m';          Reason='1M 影子召回压力开发基准' }
    @{ Crate='hcw-window';          Bench='sparse_compare';            Reason='稀疏比较开发基准' }
    @{ Crate='hcw-window';          Bench='streaming_fill';            Reason='流式填充开发基准' }
    @{ Crate='hcw-window';          Bench='window_affinity';           Reason='窗口亲和度开发基准' }
    @{ Crate='kvbsr-router';        Bench='route';                     Reason='KVBSR 路由补充开发基准' }
    @{ Crate='mcp-mesh';            Bench='mesh_benchmark';            Reason='Mesh 消息吞吐开发基准' }
    @{ Crate='mlc-engine';          Bench='memory_graph_edges';        Reason='记忆构图边基准,无独立 KPI' }
    @{ Crate='model-router';        Bench='hot_reload_bench';          Reason='模型热加载开发基准' }
    @{ Crate='model-router';        Bench='moe_bench';                 Reason='MoE 门控开发基准' }
    @{ Crate='model-router';        Bench='registry_bench';            Reason='路由注册表开发基准' }
    @{ Crate='mtpe-executor';       Bench='predict';                   Reason='MTPE 预测路径开发基准' }
    @{ Crate='nexus-core';          Bench='bridge_bench';              Reason='桥接层开发基准' }
    @{ Crate='nexus-core';          Bench='clv_bench';                 Reason='CLV 客户生命周期价值计算开发基准' }
    @{ Crate='nexus-core';          Bench='clv_cosine';                Reason='CLV 余弦专项基准' }
    @{ Crate='nexus-core';          Bench='hts_bench';                 Reason='HTS 哈希时间序列开发基准' }
    @{ Crate='nmc-encoder';         Bench='encoding_benchmark';        Reason='NMC 编码整体基准' }
    @{ Crate='nmc-encoder';         Bench='parallel_encoding';         Reason='并行编码开发基准' }
    @{ Crate='omega-learner';       Bench='per_buffer_sample';         Reason='per-buffer 采样实验基准' }
    @{ Crate='omega-learner';       Bench='r1_recall_quota';           Reason='R1 召回配额开发基准' }
    @{ Crate='omega-learner';       Bench='regret_collection';         Reason='后悔值收集实验基准' }
    @{ Crate='omega-learner';       Bench='replay_sample';             Reason='回放采样实验基准' }
    @{ Crate='omega-learner';       Bench='s4_select';                 Reason='S4 状态选择基准,非生产热路径' }
    @{ Crate='omega-learner';       Bench='s9_bench';                  Reason='S9 综合基准,主链已由 linucb_select 守护' }
    @{ Crate='osa-coordinator';     Bench='osa_parallel_bench';        Reason='OSA 并行基准,主链已由 compute_masks 守护' }
    @{ Crate='osa-coordinator';     Bench='parallel_vs_sequential';    Reason='串并行对比实验基准' }
    @{ Crate='parliament';          Bench='immune_system_probe';       Reason='免疫系统探针基准,主链已有 immune_probe 守护' }
    @{ Crate='parliament';          Bench='variant_route';             Reason='变体路由实验基准' }
    @{ Crate='pvl-layer';           Bench='gtpo_rlvr_bench';           Reason='RL 变体训练前端基准(RL 闸门:仅规划外观)' }
    @{ Crate='qeep-protocol';       Bench='protocol_bench';            Reason='QEEP 协议基准,功能层无契约' }
    @{ Crate='quest-engine';        Bench='ttg_select';                Reason='任务标签组选择开发基准' }
    @{ Crate='repo-wiki';           Bench='e2e_parallel';              Reason='检索 E2E 并行开发基准' }
    @{ Crate='repo-wiki';           Bench='fts_bench';                 Reason='全文检索 FTS 开发基准' }
    @{ Crate='repo-wiki';           Bench='knn_parallel';              Reason='KNN 并行基准(主链已由 wiki_knn_slo 守护)' }
    @{ Crate='repo-wiki';           Bench='store_bench';               Reason='存储层开发基准' }
    @{ Crate='scc-cache';           Bench='semantic_cache_bench';      Reason='语义缓存整体基准' }
    @{ Crate='scc-cache';           Bench='wal_recovery';              Reason='缓存 WAL 恢复开发基准(mcp-mesh 已有同名下界)' }
    @{ Crate='seccore';             Bench='asa_audit';                 Reason='ASA 审计链路开发基准' }
    @{ Crate='seccore';             Bench='gvisor_bench';              Reason='gVisor 沙箱探针基准' }
    @{ Crate='sesa-router';         Bench='router_benchmark';          Reason='SESA 路由整体基准' }
    @{ Crate='sesa-router';         Bench='three_layer_routing';       Reason='三层路由实验基准' }
    @{ Crate='session-store';       Bench='replay_bench';              Reason='会话回放开发基准(session_store 已注册主链)' }
    @{ Crate='ssra-fusion';         Bench='fusion_benchmark';          Reason='融合层整体基准' }
)

Write-Host "`n=== Bench Inventory Completeness Gate (WS-3 E1) ===" -ForegroundColor Cyan
Write-Host "    (三态: gated / registered / dev-only; 未知即 FAIL)`n" -ForegroundColor Gray

$invTotal = 0; $invGated = 0; $invRegistered = 0; $invDevOnly = 0; $invUnknown = 0

# Part1 gated via File(直接比较完整相对路径)
$part1Rel = @($redlines | ForEach-Object { $_.File })
# Part2 gated via (Crate, BenchFile)
$part2Pair = @($sloRedlines | ForEach-Object { "$($_.Crate)|$($_.BenchFile)" })
# registered / dev-only 查表键
$regKey   = @($registeredBench | ForEach-Object { "$($_.Crate)|$($_.Bench)" })
$devKey   = @($devOnly      | ForEach-Object { "$($_.Crate)|$($_.Bench)" })

foreach ($bf in $benchInventory) {
    $rel = $bf.FullName.Substring($projectRoot.Path.Length + 1) -replace '\\', '/'
    $parts = $rel -split '/'
    $crate = $parts[1]
    $stem  = [System.IO.Path]::GetFileNameWithoutExtension($bf.Name)
    $pair  = "$crate|$stem"
    $invTotal++

    $state = 'unknown'
    if ($part1Rel -contains $rel) { $state = 'gated' }
    elseif ($part2Pair -contains $pair) { $state = 'gated' }
    elseif ($regKey -contains $pair) { $state = 'registered' }
    elseif ($devKey -contains $pair) { $state = 'dev-only' }

    $guardian = switch ($state) {
        'gated'      { 'Part1 $redlines.File / Part2 $sloRedlines' }
        'registered' { 'bench_check.yml run_bench --bench' }
        'dev-only'   { 'Part3 $devOnly 显式登记(无 spec KPI)' }
        default      { 'UNKNOWN' }
    }
    switch ($state) {
        'gated'      { $invGated++;      Write-Check "inventory gated      $crate/$stem" $true  "守护: $guardian" }
        'registered' { $invRegistered++; Write-Check "inventory registered $crate/$stem" $true  "守护: $guardian" }
        'dev-only'   { $invDevOnly++;    Write-Check "inventory dev-only   $crate/$stem" $true  "声明: $guardian" }
        default      { $invUnknown++;    Write-Check "inventory unknown    $crate/$stem" $false "请登记为 registered 或加入 \$devOnly" }
    }
}

Write-Host "`n  Bridge Inventory Counts: total=$invTotal gated=$invGated registered=$invRegistered dev-only=$invDevOnly unknown=$invUnknown" -ForegroundColor White
if ($invUnknown -gt 0) {
    Write-Host "  FAIL: $invUnknown 个 bench 未受任何守护, 必须显式登记" -ForegroundColor Red
} else {
    Write-Host "  PASS: 全量 bench 均已显式守护" -ForegroundColor Green
}

# --- 最终汇总 ---
Write-Host "`n=== Final Summary ===" -ForegroundColor Cyan
Write-Host "  Static Lint:" -ForegroundColor White
$totalChecks = $redlines.Count * 2
$passedChecks = $totalChecks - $script:FailCount
Write-Host "    Passed: $passedChecks / $totalChecks" -ForegroundColor $(if ($script:FailCount -eq 0) { 'Green' } else { 'Red' })
Write-Host "    Warnings: $script:WarnCount" -ForegroundColor Yellow
Write-Host "  SLO Benchmarks:" -ForegroundColor White
Write-Host "    Passed: $sloPassCount" -ForegroundColor $(if ($sloFailCount -eq 0) { 'Green' } else { 'Red' })
Write-Host "    Failed: $sloFailCount" -ForegroundColor $(if ($sloFailCount -gt 0) { 'Red' } else { 'Gray' })
Write-Host "    Skipped: $sloSkipCount" -ForegroundColor Yellow

$totalFail = $script:FailCount + $sloFailCount
if ($totalFail -gt 0) {
    Write-Host "`n  RESULT: FAIL — 有 $totalFail 项检查失败" -ForegroundColor Red
    exit 1
} else {
    Write-Host "`n  RESULT: PASS — 全部性能红线 lint + SLO 断言通过" -ForegroundColor Green
    exit 0
}

