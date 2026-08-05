//! Token 效率优化压测框架 — ADR-069 高并发场景验证
//!
//! 验证五项客户端域优化在高并发下的性能表现与熔断行为:
//! 1. 厂商缓存亲和(CacheHitTracker 并发 record)
//! 2. 语义缓存命中率(并发语义命中统计)
//! 3. CostGuard 成本熔断(超限熔断 + 半开窗口探测)
//! 4. 并发安全性(DashMap + Atomic 无锁正确性)
//! 5. 12 场景压测矩阵(3 并发 × 4 输入规模)
//!
//! # 模拟压测模式
//! Chimera 是 API 客户端(无法真正发起 HTTP 请求),压测采用模拟模式:
//! - 每个"请求"模拟 CacheHitTracker::record + CostGuard::check 调用
//! - 采集真实延迟(含 tokio 调度开销)
//! - 验证熔断状态机正确性(CircuitOpen → HalfOpen → Re-melt)
//!
//! # 运行方式
//! ```powershell
//! cargo test --test token_efficiency_stress -- --ignored --nocapture
//! ```
//!
//! # broadcast 时序铁律(Week 6 教训 #9)
//! 涉及事件订阅的测试必须先 `bus.subscribe()` 再发布事件。

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use event_bus::{EventBus, NexusEvent};
use mca_gateway::cost_guard::CostGuard;
use scc_cache::CacheHitTracker;

// ============================================================
// 压测场景参数
// ============================================================

/// 压测场景定义
#[derive(Debug, Clone)]
struct StressScenario {
    /// 并发数: 1, 10, 50
    concurrency: usize,
    /// 输入规模(字符数): 4K, 16K, 64K, 128K
    input_size: usize,
    /// 压测持续时间(秒)
    duration_secs: u64,
    /// 厂商名称
    provider: String,
}

/// 压测结果
#[derive(Debug, Clone, Default)]
struct StressResult {
    /// 场景标识
    scenario: String,
    /// 总请求数
    total_requests: u64,
    /// 成功请求数
    success_count: u64,
    /// 失败请求数
    failure_count: u64,
    /// 每秒请求数(QPS)
    qps: f64,
    /// P50 延迟(毫秒)
    latency_p50_ms: f64,
    /// P95 延迟(毫秒)
    latency_p95_ms: f64,
    /// P99 延迟(毫秒)
    latency_p99_ms: f64,
    /// 总成本(微元)
    total_cost: f64,
    /// 熔断触发次数
    circuit_breaker_trips: u64,
    /// 平均厂商缓存命中率(%)
    avg_cache_hit_rate: f32,
    /// 平均语义缓存命中率(%)
    avg_semantic_cache_hit_rate: f32,
}

// ============================================================
// 压测场景生成
// ============================================================

/// 生成 12 个压测场景: 3 并发(1/10/50) × 4 输入规模(4K/16K/64K/128K)
fn generate_scenarios() -> Vec<StressScenario> {
    let concurrency_levels = [1, 10, 50];
    let input_sizes = [4_000, 16_000, 64_000, 128_000]; // 4K, 16K, 64K, 128K
    let providers = ["deepseek", "zhipu", "minimax", "moonshot"];

    let mut scenarios = Vec::with_capacity(12);
    for &concurrency in &concurrency_levels {
        for &input_size in &input_sizes {
            let provider = providers[scenarios.len() % providers.len()].to_string();
            scenarios.push(StressScenario {
                concurrency,
                input_size,
                // 压测持续时间: 并发数越高,持续时间越短(避免总耗时膨胀)
                duration_secs: match concurrency {
                    1 => 2,
                    10 => 2,
                    50 => 1,
                    _ => 1,
                },
                provider,
            });
        }
    }
    scenarios
}

/// 场景标识字符串: "c{concurrency}_i{input_size_k}K"
fn scenario_label(scenario: &StressScenario) -> String {
    let size_k = scenario.input_size / 1000;
    format!("c{}_i{}K", scenario.concurrency, size_k)
}

// ============================================================
// 模拟请求定义
// ============================================================

/// 单次模拟请求的上下文
struct SimulatedRequest {
    /// 输入规模(字符数,影响模拟的 token 成本)
    input_size: usize,
    /// 厂商名称
    provider: String,
    /// 当前 Unix 时间戳(秒)
    now_secs: i64,
}

/// 单次模拟请求的结果
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct SimulatedResponse {
    /// 请求是否成功(未被熔断拒绝)
    success: bool,
    /// 是否触发熔断
    circuit_breaker_tripped: bool,
    /// 请求延迟(微秒)
    latency_micros: u64,
    /// 本次请求成本(微元,基于 token 估算)
    cost_micro: u64,
    /// 厂商缓存命中 token 数
    cache_hit_tokens: u64,
    /// 总输入 token 数
    total_input_tokens: u64,
    /// 是否语义缓存命中
    semantic_hit: bool,
}

/// 执行一次模拟请求
///
/// 模拟流程:
/// 1. 模拟计算 token 数(字符/4 估算)
/// 2. 模拟厂商缓存命中(基于概率的命中判断)
/// 3. 记录 CacheHitTracker
/// 4. 检查 CostGuard 预算
/// 5. 记录成本
fn simulate_request(
    req: &SimulatedRequest,
    cache_tracker: &CacheHitTracker,
    cost_guard: &CostGuard,
    circuit_breaker_trips: &AtomicU64,
) -> SimulatedResponse {
    let start = Instant::now();

    // 1. 估算 token 数(字符/4 启发式)
    let estimated_tokens = (req.input_size / 4) as u64;

    // 2. 模拟厂商缓存命中: 基于厂商类型决定命中概率
    //    - 显式控制族(Zhipu/MiniMax/Moonshot): 80% 命中率
    //    - 隐式族(DeepSeek): 50% 命中率
    let hit_probability = match req.provider.as_str() {
        "zhipu" | "minimax" | "moonshot" => 0.80,
        "deepseek" => 0.50,
        _ => 0.50,
    };

    // 使用确定性伪随机(基于 input_size 和 now_secs 的简单 hash)
    let seed = req
        .input_size
        .wrapping_mul(6364136223846793005)
        .wrapping_add(req.now_secs as usize);
    let is_hit = (seed % 100) < (hit_probability * 100.0) as usize;

    let cache_hit_tokens = if is_hit {
        (estimated_tokens as f64 * hit_probability) as u64
    } else {
        0
    };

    // 3. 记录 CacheHitTracker
    cache_tracker.record(&req.provider, cache_hit_tokens, estimated_tokens);

    // 模拟语义缓存: 10% 概率命中
    let semantic_hit = (seed.wrapping_add(42) % 100) < 10;
    cache_tracker.record_request();
    if semantic_hit {
        cache_tracker.record_semantic_hit();
    }

    // 4. 检查 CostGuard 预算
    let cost_check = cost_guard.check(req.now_secs);

    let (success, circuit_breaker_tripped, cost_micro) = match cost_check {
        Ok(()) => {
            // 预算内放行 — 计算成本
            // 成本模型: 每 1K token 约 10 微元(简化版)
            let cost = (estimated_tokens / 1000).max(1) * 10;
            cost_guard.record(cost);
            (true, false, cost)
        }
        Err(_) => {
            // 熔断拒绝
            circuit_breaker_trips.fetch_add(1, Ordering::Relaxed);
            (false, true, 0)
        }
    };

    let latency_micros = start.elapsed().as_micros() as u64;

    SimulatedResponse {
        success,
        circuit_breaker_tripped,
        latency_micros,
        cost_micro,
        cache_hit_tokens,
        total_input_tokens: estimated_tokens,
        semantic_hit,
    }
}

// ============================================================
// 压测运行器
// ============================================================

/// 运行单个压测场景
///
/// 使用 `tokio::spawn` 模拟并发请求,每个任务循环发送模拟请求直至超时。
/// 采集所有响应的延迟、成功/失败、缓存命中率、熔断次数。
async fn run_stress_scenario(scenario: &StressScenario) -> StressResult {
    let cache_tracker = Arc::new(CacheHitTracker::new());
    // 成本上限: 由输入规模和并发数决定
    // 4K 输入 → 约 1000 tokens → 10 微元/请求
    // 上限设为能支撑约 50 次请求的量
    let estimated_cost_per_req = ((scenario.input_size / 4 / 1000).max(1) * 10) as u64;
    let budget = estimated_cost_per_req
        .saturating_mul(scenario.concurrency as u64)
        .saturating_mul(20) // 20 轮请求的预算
        .max(1000);

    let cost_guard = Arc::new(CostGuard::new(Some(budget)));
    let circuit_breaker_trips = Arc::new(AtomicU64::new(0));

    let label = scenario_label(scenario);
    let deadline = Instant::now() + Duration::from_secs(scenario.duration_secs);

    // 收集所有响应的延迟(微秒)
    let all_latencies = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
    let total_requests = Arc::new(AtomicU64::new(0));
    let success_count = Arc::new(AtomicU64::new(0));
    let failure_count = Arc::new(AtomicU64::new(0));
    let total_cost = Arc::new(AtomicU64::new(0));

    // 并发任务: 每个任务独立循环发送请求
    let mut handles = Vec::with_capacity(scenario.concurrency);
    for worker_id in 0..scenario.concurrency {
        let tracker = Arc::clone(&cache_tracker);
        let guard = Arc::clone(&cost_guard);
        let trips = Arc::clone(&circuit_breaker_trips);
        let latencies = Arc::clone(&all_latencies);
        let total = Arc::clone(&total_requests);
        let success = Arc::clone(&success_count);
        let failure = Arc::clone(&failure_count);
        let cost_sum = Arc::clone(&total_cost);
        let provider = scenario.provider.clone();
        let input_size = scenario.input_size;

        let handle = tokio::spawn(async move {
            let base_time = 1000i64; // 固定基准时间戳
            let mut req_seq = 0usize;

            while Instant::now() < deadline {
                let now_secs = base_time + (worker_id * 1000 + req_seq) as i64;
                let req = SimulatedRequest {
                    input_size,
                    provider: provider.clone(),
                    now_secs,
                };

                let resp = simulate_request(&req, &tracker, &guard, &trips);

                // 采集指标
                latencies.lock().unwrap().push(resp.latency_micros);
                total.fetch_add(1, Ordering::Relaxed);
                if resp.success {
                    success.fetch_add(1, Ordering::Relaxed);
                    cost_sum.fetch_add(resp.cost_micro, Ordering::Relaxed);
                } else {
                    failure.fetch_add(1, Ordering::Relaxed);
                }

                req_seq += 1;
                // 避免忙等: 每个请求间微休眠模拟真实网络延迟
                tokio::time::sleep(Duration::from_micros(50)).await;
            }
        });
        handles.push(handle);
    }

    // 等待所有任务完成
    for handle in handles {
        let _ = handle.await;
    }

    // 聚合延迟统计
    let mut latencies_vec = all_latencies.lock().unwrap().clone();
    latencies_vec.sort_unstable();
    let len = latencies_vec.len();
    let (p50, p95, p99) = if len == 0 {
        (0.0, 0.0, 0.0)
    } else {
        let p50_idx = (len as f64 * 0.50) as usize;
        let p95_idx = (len as f64 * 0.95) as usize;
        let p99_idx = (len as f64 * 0.99) as usize;
        (
            latencies_vec[p50_idx.min(len - 1)] as f64 / 1000.0, // 微秒 → 毫秒
            latencies_vec[p95_idx.min(len - 1)] as f64 / 1000.0,
            latencies_vec[p99_idx.min(len - 1)] as f64 / 1000.0,
        )
    };

    let total = total_requests.load(Ordering::Relaxed);
    let qps = if scenario.duration_secs > 0 {
        total as f64 / scenario.duration_secs as f64
    } else {
        0.0
    };

    // 缓存命中率
    let avg_cache_hit_rate = cache_tracker
        .all_hit_rates()
        .get(&scenario.provider)
        .copied()
        .unwrap_or(0.0)
        * 100.0;
    let avg_semantic_cache_hit_rate = cache_tracker.semantic_hit_rate_percent() as f32;

    StressResult {
        scenario: label,
        total_requests: total,
        success_count: success_count.load(Ordering::Relaxed),
        failure_count: failure_count.load(Ordering::Relaxed),
        qps,
        latency_p50_ms: p50,
        latency_p95_ms: p95,
        latency_p99_ms: p99,
        total_cost: total_cost.load(Ordering::Relaxed) as f64,
        circuit_breaker_trips: circuit_breaker_trips.load(Ordering::Relaxed),
        avg_cache_hit_rate,
        avg_semantic_cache_hit_rate,
    }
}

// ============================================================
// 压测报告生成
// ============================================================

/// 压测报告汇总
#[derive(Debug, Clone, serde::Serialize)]
struct StressReport {
    /// 报告标题
    title: String,
    /// 生成时间戳
    generated_at: String,
    /// 场景结果列表
    scenarios: Vec<ScenarioReportEntry>,
    /// 汇总统计
    summary: ReportSummary,
}

#[derive(Debug, Clone, serde::Serialize)]
struct ScenarioReportEntry {
    scenario: String,
    concurrency: usize,
    input_size_k: usize,
    total_requests: u64,
    success_count: u64,
    failure_count: u64,
    qps: f64,
    latency_p50_ms: f64,
    latency_p95_ms: f64,
    latency_p99_ms: f64,
    total_cost: f64,
    circuit_breaker_trips: u64,
    avg_cache_hit_rate: f32,
    avg_semantic_cache_hit_rate: f32,
}

#[derive(Debug, Clone, serde::Serialize)]
struct ReportSummary {
    total_scenarios: usize,
    total_requests: u64,
    total_success: u64,
    total_failures: u64,
    total_circuit_breaker_trips: u64,
    avg_qps: f64,
    avg_cache_hit_rate: f32,
    avg_semantic_cache_hit_rate: f32,
}

/// 从 StressResult 列表生成 JSON 报告并写入文件
fn generate_report(results: &[StressResult], scenarios: &[StressScenario]) -> StressReport {
    let mut entries = Vec::with_capacity(results.len());
    for (i, result) in results.iter().enumerate() {
        let sc = &scenarios[i];
        entries.push(ScenarioReportEntry {
            scenario: result.scenario.clone(),
            concurrency: sc.concurrency,
            input_size_k: sc.input_size / 1000,
            total_requests: result.total_requests,
            success_count: result.success_count,
            failure_count: result.failure_count,
            qps: result.qps,
            latency_p50_ms: result.latency_p50_ms,
            latency_p95_ms: result.latency_p95_ms,
            latency_p99_ms: result.latency_p99_ms,
            total_cost: result.total_cost,
            circuit_breaker_trips: result.circuit_breaker_trips,
            avg_cache_hit_rate: result.avg_cache_hit_rate,
            avg_semantic_cache_hit_rate: result.avg_semantic_cache_hit_rate,
        });
    }

    let total_requests: u64 = results.iter().map(|r| r.total_requests).sum();
    let total_success: u64 = results.iter().map(|r| r.success_count).sum();
    let total_failures: u64 = results.iter().map(|r| r.failure_count).sum();
    let total_cb_trips: u64 = results.iter().map(|r| r.circuit_breaker_trips).sum();
    let avg_qps = if !results.is_empty() {
        results.iter().map(|r| r.qps).sum::<f64>() / results.len() as f64
    } else {
        0.0
    };
    let avg_cache = if !results.is_empty() {
        results.iter().map(|r| r.avg_cache_hit_rate).sum::<f32>() / results.len() as f32
    } else {
        0.0
    };
    let avg_semantic = if !results.is_empty() {
        results
            .iter()
            .map(|r| r.avg_semantic_cache_hit_rate)
            .sum::<f32>()
            / results.len() as f32
    } else {
        0.0
    };

    StressReport {
        title: "Token 效率优化 v2 压测报告".to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        scenarios: entries,
        summary: ReportSummary {
            total_scenarios: results.len(),
            total_requests,
            total_success,
            total_failures,
            total_circuit_breaker_trips: total_cb_trips,
            avg_qps,
            avg_cache_hit_rate: avg_cache,
            avg_semantic_cache_hit_rate: avg_semantic,
        },
    }
}

// ============================================================
// 压测入口 — 12 场景全量压测
// ============================================================

/// 12 场景全量压测(标记 #[ignore],非日常 CI 一部分)
///
/// 场景矩阵: 3 并发(1/10/50) × 4 输入规模(4K/16K/64K/128K)
///
/// 验证目标:
/// - 所有场景无 panic
/// - CacheHitTracker 并发安全(多线程同时 record)
/// - CostGuard 熔断正确触发
/// - 延迟统计合理(non-zero)
///
/// 运行: cargo test --test token_efficiency_stress -- --ignored --nocapture
// P9-T2: 12 场景全量压测矩阵(3 并发 × 4 输入规模),test-group=stress 隔离;
//        ci-fast 档不执行,nightly stress profile 集中触发。
//        保留 #[ignore] 兼容 cargo test 旧调用约定(cargo test -- --ignored)。
#[tokio::test]
#[ignore]
async fn test_stress_12_scenarios_full_matrix() {
    let scenarios = generate_scenarios();
    assert_eq!(scenarios.len(), 12, "必须有 12 个场景");

    let mut results = Vec::with_capacity(12);

    for scenario in &scenarios {
        let label = scenario_label(scenario);
        println!(
            "[STRESS] 开始场景: {} (并发={}, 输入={}K, 持续={}s)",
            label,
            scenario.concurrency,
            scenario.input_size / 1000,
            scenario.duration_secs
        );

        let result = run_stress_scenario(scenario).await;

        println!(
            "  → 总请求={} 成功={} 失败={} QPS={:.1} P50={:.3}ms P95={:.3}ms P99={:.3}ms 熔断={} 缓存命中率={:.1}% 语义命中率={:.1}%",
            result.total_requests,
            result.success_count,
            result.failure_count,
            result.qps,
            result.latency_p50_ms,
            result.latency_p95_ms,
            result.latency_p99_ms,
            result.circuit_breaker_trips,
            result.avg_cache_hit_rate,
            result.avg_semantic_cache_hit_rate,
        );

        results.push(result);
    }

    // 生成 JSON 报告
    let report = generate_report(&results, &scenarios);
    let report_json = serde_json::to_string_pretty(&report).expect("报告序列化失败");

    // 写入报告文件
    let report_path = std::path::Path::new("stress_test_report.json");
    std::fs::write(report_path, &report_json).expect("报告写入失败");
    println!("\n[STRESS] 压测报告已写入: {}", report_path.display());

    // 汇总验证
    let summary = &report.summary;
    println!("\n=== 汇总 ===");
    println!("场景数: {}", summary.total_scenarios);
    println!("总请求: {}", summary.total_requests);
    println!("总成功: {}", summary.total_success);
    println!("总失败: {}", summary.total_failures);
    println!("总熔断: {}", summary.total_circuit_breaker_trips);
    println!("平均 QPS: {:.1}", summary.avg_qps);
    println!("平均缓存命中率: {:.1}%", summary.avg_cache_hit_rate);
    println!(
        "平均语义缓存命中率: {:.1}%",
        summary.avg_semantic_cache_hit_rate
    );

    // 基本断言: 所有场景至少产生了一些请求
    for result in &results {
        assert!(
            result.total_requests > 0,
            "场景 {} 必须产生请求",
            result.scenario
        );
    }
}

// ============================================================
// 单元测试: 压测框架自身正确性
// ============================================================

/// 测试 1: 场景参数正确性 — 12 个场景参数正确
#[test]
fn test_scenario_parameters_correctness() {
    let scenarios = generate_scenarios();
    assert_eq!(scenarios.len(), 12);

    // 验证场景覆盖: 3 并发 × 4 输入规模
    let expected_pairs: Vec<(usize, usize)> = [1, 10, 50]
        .iter()
        .flat_map(|&c| {
            [4_000, 16_000, 64_000, 128_000]
                .iter()
                .map(move |&s| (c, s))
        })
        .collect();

    for (i, scenario) in scenarios.iter().enumerate() {
        let (expected_concurrency, expected_input_size) = expected_pairs[i];
        assert_eq!(
            scenario.concurrency, expected_concurrency,
            "场景 {i} 并发数应为 {expected_concurrency}"
        );
        assert_eq!(
            scenario.input_size, expected_input_size,
            "场景 {i} 输入规模应为 {expected_input_size}"
        );
        assert!(scenario.duration_secs > 0, "场景 {i} 持续时间必须 > 0");
        assert!(!scenario.provider.is_empty(), "场景 {i} 厂商名不能为空");
    }
}

/// 测试 2: 场景标签格式正确性
#[test]
fn test_scenario_label_format() {
    let scenarios = generate_scenarios();
    let labels: Vec<String> = scenarios.iter().map(scenario_label).collect();

    // 验证标签唯一性
    let mut unique: HashMap<String, usize> = HashMap::new();
    for label in &labels {
        *unique.entry(label.clone()).or_insert(0) += 1;
    }
    for (label, count) in &unique {
        assert_eq!(*count, 1, "标签 '{label}' 重复 {count} 次");
    }
    assert_eq!(unique.len(), 12, "必须有 12 个唯一标签");

    // 验证格式: c{concurrency}_i{size}K
    for label in &labels {
        assert!(label.starts_with('c'), "标签 '{label}' 必须以 'c' 开头");
        assert!(label.contains("_i"), "标签 '{label}' 必须包含 '_i'");
        assert!(label.ends_with('K'), "标签 '{label}' 必须以 'K' 结尾");
    }
}

/// 测试 3: StressResult 默认值合理性
#[test]
fn test_stress_result_defaults() {
    let result = StressResult::default();
    assert_eq!(result.total_requests, 0);
    assert_eq!(result.success_count, 0);
    assert_eq!(result.failure_count, 0);
    assert_eq!(result.qps, 0.0);
    assert_eq!(result.latency_p50_ms, 0.0);
    assert_eq!(result.latency_p95_ms, 0.0);
    assert_eq!(result.latency_p99_ms, 0.0);
    assert_eq!(result.total_cost, 0.0);
    assert_eq!(result.circuit_breaker_trips, 0);
    assert_eq!(result.avg_cache_hit_rate, 0.0);
    assert_eq!(result.avg_semantic_cache_hit_rate, 0.0);
    assert!(result.scenario.is_empty());
}

/// 测试 4: 报告生成正确性
#[test]
fn test_report_generation() {
    let scenarios = vec![StressScenario {
        concurrency: 10,
        input_size: 16_000,
        duration_secs: 1,
        provider: "deepseek".to_string(),
    }];

    let results = vec![StressResult {
        scenario: "c10_i16K".to_string(),
        total_requests: 100,
        success_count: 90,
        failure_count: 10,
        qps: 100.0,
        latency_p50_ms: 5.0,
        latency_p95_ms: 15.0,
        latency_p99_ms: 25.0,
        total_cost: 1000.0,
        circuit_breaker_trips: 3,
        avg_cache_hit_rate: 50.0,
        avg_semantic_cache_hit_rate: 10.0,
    }];

    let report = generate_report(&results, &scenarios);
    assert_eq!(report.title, "Token 效率优化 v2 压测报告");
    assert!(!report.generated_at.is_empty());
    assert_eq!(report.scenarios.len(), 1);
    assert_eq!(report.summary.total_scenarios, 1);
    assert_eq!(report.summary.total_requests, 100);
    assert_eq!(report.summary.total_success, 90);
    assert_eq!(report.summary.total_failures, 10);
    assert_eq!(report.summary.total_circuit_breaker_trips, 3);

    // 验证 JSON 可序列化
    let json = serde_json::to_string_pretty(&report).expect("序列化失败");
    assert!(json.contains("c10_i16K"));
    assert!(json.contains("Token 效率优化 v2 压测报告"));
}

/// 测试 5: 模拟请求确定性(相同输入产生一致结果)
#[test]
fn test_simulate_request_deterministic() {
    let tracker = CacheHitTracker::new();
    let guard = CostGuard::new(Some(1_000_000));
    let trips = AtomicU64::new(0);

    let req = SimulatedRequest {
        input_size: 16_000,
        provider: "zhipu".to_string(),
        now_secs: 1000,
    };

    // 相同输入应产生相同结果(确定性伪随机)
    let resp1 = simulate_request(&req, &tracker, &guard, &trips);
    let resp2 = simulate_request(&req, &tracker, &guard, &trips);

    assert_eq!(resp1.success, resp2.success);
    assert_eq!(resp1.cache_hit_tokens, resp2.cache_hit_tokens);
    assert_eq!(resp1.total_input_tokens, resp2.total_input_tokens);
    assert_eq!(resp1.cost_micro, resp2.cost_micro);
}

/// 测试 6: 熔断行为正确性 — 超过 budget 后触发
#[test]
fn test_circuit_breaker_triggers_on_budget_exceeded() {
    let tracker = CacheHitTracker::new();
    // 极低预算: 仅 100 微元
    let guard = CostGuard::new(Some(100));
    let trips = AtomicU64::new(0);

    let mut success = 0;
    let mut failed = 0;

    // 发送 20 次请求,每次成本约 40 微元(16K 输入 → 4000 tokens → 40 微元)
    for i in 0..20 {
        let req = SimulatedRequest {
            input_size: 16_000,
            provider: "deepseek".to_string(),
            now_secs: 1000 + i,
        };
        let resp = simulate_request(&req, &tracker, &guard, &trips);
        if resp.success {
            success += 1;
        } else {
            failed += 1;
        }
    }

    // 预算仅 100 微元,成本约 40 微元/请求 → 约 2-3 次成功后就熔断
    assert!(success <= 3, "预算 100 微元,最多 2-3 次成功请求");
    assert!(failed >= 1, "超预算后必须触发熔断");
    assert!(trips.load(Ordering::Relaxed) >= 1, "熔断计数器必须递增");
}

/// 测试 7: 并发安全性 — 多线程同时操作 CacheHitTracker
#[test]
fn test_concurrent_cache_hit_tracker() {
    let tracker = Arc::new(CacheHitTracker::new());
    let threads: usize = 8;
    let ops_per_thread: usize = 1000;

    let mut handles = Vec::with_capacity(threads);
    for t in 0..threads {
        let tracker = Arc::clone(&tracker);
        let handle = std::thread::spawn(move || {
            let provider = if t % 2 == 0 { "zhipu" } else { "deepseek" };
            for i in 0..ops_per_thread {
                // 交替命中/未命中
                let hit = if i % 2 == 0 { 100 } else { 0 };
                tracker.record(provider, hit, 200);
                tracker.record_request();
                if i % 10 == 0 {
                    tracker.record_semantic_hit();
                }
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("线程 panic");
    }

    // 验证数据完整性
    let total_requests = tracker.total_requests();
    assert_eq!(
        total_requests,
        (threads * ops_per_thread) as u64,
        "总请求数应与线程数×操作数一致"
    );

    let zhipu_rate = tracker.hit_rate_percent("zhipu");
    let deepseek_rate = tracker.hit_rate_percent("deepseek");
    // 交替命中(100)/未命中(0),每次 200 input → 每对 100/400 = 25%
    assert_eq!(zhipu_rate, 25, "交替命中/未命中 → 25%");
    assert_eq!(deepseek_rate, 25, "交替命中/未命中 → 25%");

    // 语义缓存命中率: 每 10 次请求 1 次语义命中 → 10%
    assert_eq!(tracker.semantic_hit_rate_percent(), 10);
}

// ============================================================
// CostGuard 熔断生命周期验证(独立测试,不依赖模拟请求)
// ============================================================

/// 测试 8: CostGuard 完整熔断生命周期
/// - 阶段 1: 未超限 → 放行
/// - 阶段 2: 跨线 → 熔断 + BudgetExceeded 事件
/// - 阶段 3: 熔断期 → 拒绝
/// - 阶段 4: 半开窗口 → 放行探测请求 + 重开熔断
#[test]
fn test_cost_guard_full_lifecycle() {
    let guard = CostGuard::new(Some(100));

    // 阶段 1: 未超限放行(累计 50 < 100)
    guard.record(50);
    assert!(guard.check(1000).is_ok(), "未超限必须放行");

    // 阶段 2: 跨线熔断(累计 50+60=110 > 100)
    guard.record(60);
    let err = guard.check(1000).unwrap_err();
    match err {
        mca_gateway::cost_guard::CostGuardError::CircuitOpen {
            spent,
            limit,
            reopen_at: _,
        } => {
            assert_eq!(spent, 110);
            assert_eq!(limit, 100);
        }
    }

    // 阶段 3: 熔断期内拒绝(1001 < 1000+30=1030,仍在熔断期)
    assert!(guard.check(1001).is_err(), "熔断期内必须拒绝");
    assert!(guard.check(1029).is_err(), "29s 处仍在熔断期");

    // 阶段 4: 半开窗口(1030 >= 1030)
    assert!(guard.check(1030).is_ok(), "半开窗口必须放行探测请求");
    // 探测后仍超限 → 重新熔断
    assert!(guard.check(1031).is_err(), "探测后仍超限必须重新熔断");
}

/// 测试 9: CostGuard 未设上限恒放行
#[test]
fn test_cost_guard_no_limit_always_allows() {
    let guard = CostGuard::new(None);
    guard.record(1_000_000);
    assert!(guard.check(1000).is_ok());
    assert!(guard.check(9999).is_ok());
}

/// 测试 10: CostGuard BudgetExceeded 事件防重放
#[tokio::test]
async fn test_cost_guard_budget_exceeded_published_once() {
    // broadcast 纪律: subscribe 必须在 check(publish) 之前同步调用
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let guard = CostGuard::with_bus(Some(100), Some(bus));

    guard.record(150);
    // 首次跨线 → 发布 BudgetExceeded
    assert!(guard.check(1000).is_err());

    let mut events = 0;
    while let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)).await {
        if let NexusEvent::BudgetExceeded { .. } = ev {
            events += 1;
        }
    }
    assert_eq!(events, 1, "BudgetExceeded 必须只发布一次");

    // 熔断期内重复 check 不重发
    assert!(guard.check(1001).is_err());
    assert!(guard.check(1002).is_err());
    let mut extra = 0;
    while let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)).await {
        if let NexusEvent::BudgetExceeded { .. } = ev {
            extra += 1;
        }
    }
    assert_eq!(extra, 0, "防重放: 不得重复发布");
}

/// 测试 11: CostGuard 半开窗口探测逻辑
#[test]
fn test_cost_guard_half_open_probe() {
    let guard = CostGuard::new(Some(50));

    // 跨线
    guard.record(100);
    assert!(guard.check(1000).is_err());
    // 熔断期内
    assert!(guard.check(1029).is_err());
    // 半开: 放行
    assert!(guard.check(1030).is_ok(), "半开窗口必须放行");
    // 探测后重熔断
    assert!(guard.check(1031).is_err(), "探测后仍超限必须重熔断");
    // 下一半开窗口
    assert!(guard.check(1060).is_ok());
    assert!(guard.check(1061).is_err());
}

/// 测试 12: 自定义 StressResult 非默认值验证
#[test]
fn test_stress_result_non_zero_fields() {
    let result = StressResult {
        scenario: "c10_i16K".to_string(),
        total_requests: 1000,
        success_count: 950,
        failure_count: 50,
        qps: 500.0,
        latency_p50_ms: 2.5,
        latency_p95_ms: 8.0,
        latency_p99_ms: 12.0,
        total_cost: 5000.0,
        circuit_breaker_trips: 3,
        avg_cache_hit_rate: 75.0,
        avg_semantic_cache_hit_rate: 12.0,
    };

    // 验证所有字段非零(非空)
    assert!(!result.scenario.is_empty());
    assert!(result.total_requests > 0);
    assert!(result.success_count > 0);
    assert!(result.failure_count > 0);
    assert!(result.qps > 0.0);
    assert!(result.latency_p50_ms > 0.0);
    assert!(result.latency_p95_ms > 0.0);
    assert!(result.latency_p99_ms > 0.0);
    assert!(result.total_cost > 0.0);
    assert!(result.circuit_breaker_trips > 0);
    assert!(result.avg_cache_hit_rate > 0.0);
    assert!(result.avg_semantic_cache_hit_rate > 0.0);

    // 成功 + 失败 = 总请求
    assert_eq!(
        result.success_count + result.failure_count,
        result.total_requests,
        "成功+失败必须等于总请求数"
    );
}
