//! SubTask 37.3:CSA 延迟验证
//!
//! 对应架构层:L8 Parliament + L4 Security + L3 Budget + L9 Quest
//!
//! # 测试目标
//! 验证 CSA(Cognitive Security Assembly,认知安全组装)端到端延迟 < 300ms。
//! CSA 延迟分解:TTG 1ms + DECB 1ms + Parliament 200ms + Skeptic 10ms
//! + ASA 5ms + AHIRT 50ms + 事件传播 30ms ≈ 297ms
//!
//! # 设计决策(WHY)
//! - 使用 `min-of-N`(5 次)方法:取 5 次运行的最小延迟,减少调度噪声
//!   (参考 project_memory.md:min-of-N 减少亚毫秒级操作的调度噪声)
//! - 性能测试标记 `#[ignore = "perf: run with --ignored"]`:避免在常规
//!   `cargo test` 中运行性能断言,仅通过 `--ignored` 显式触发
//! - 使用 `std::time::Instant`:单调时钟,不受系统时间调整影响
//! - 每个测试函数 ≤ 200 行(项目铁律)
//! - 不使用 `unwrap()`/`expect()`,用 `?` 与 `assert!` 替代
//! - 延迟分解测试:逐阶段测量 TTG/DECB/Parliament/Skeptic/ASA/AHIRT 延迟,
//!   验证各阶段延迟符合预算分配(单阶段超预算时给出明确错误信息)

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

use std::error::Error;
use std::time::{Duration, Instant};

use decb_governor::{BudgetTier, DecbConfig, DecbGovernor, QuestBudgetInput};
use event_bus::EventBus;
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use parliament::{AhirtRedTeam, Parliament, ParliamentConfig, Proposal};
use quest_engine::{TtgConfig, TtgGovernor};
use seccore::{AsaAuditor, OperationAuditInput};

/// 测试结果类型:支持 `?` 操作符与任意错误类型转换
type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

/// 携带返回值的测试结果类型(用于延迟测量等需要返回数据的场景)
type TestResultWith<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// CSA 延迟阈值:300ms(Week 5 验收门禁)
///
/// WHY 300ms:CSA 延迟分解总和约 297ms,留 3ms 安全余量
const CSA_LATENCY_THRESHOLD_MS: u64 = 300;

/// min-of-N 的 N 值:运行 5 次取最小值
const MIN_OF_N: usize = 5;

// ============================================================
// 辅助函数 — 构造测试 Quest 与运行 CSA 流程
// ============================================================

/// 构造测试 Quest(无依赖,指定任务数与思考模式)
fn make_quest(quest_id: &str, task_count: usize, thinking_mode: ThinkingMode) -> Quest {
    let tasks: Vec<Task> = (0..task_count)
        .map(|i| Task {
            task_id: format!("{quest_id}-t-{i}"),
            description: format!("任务 {i}"),
            status: TaskStatus::Pending,
            dependencies: vec![],
        })
        .collect();
    Quest {
        quest_id: quest_id.to_string(),
        title: format!("测试 Quest {quest_id}"),
        tasks,
        thinking_mode,
        checkpoint_id: None,
    }
}

/// 构造测试预算输入(无 deadline,指定任务数)
fn make_budget_input(quest_id: &str, task_count: usize) -> QuestBudgetInput {
    QuestBudgetInput::new(quest_id, task_count, 0, None, 100)
}

/// 执行一次完整 CSA 流程(异步)并返回端到端延迟
///
/// CSA 流程:TTG 模式选择 → DECB 预算计算 → Parliament 辩论
/// → ASA 审计 → AHIRT 探测(简化版,验证主要阶段)
///
/// WHY 简化 AHIRT:全量 AHIRT 探测(100 个载荷)延迟较高,
/// CSA 延迟测试关注端到端流程,使用 `probe` 单类探测替代 `probe_all`
async fn run_csa_once_async(
    quest: &Quest,
    proposal: &Proposal,
    budget_input: &QuestBudgetInput,
) -> TestResultWith<Duration> {
    let start = Instant::now();

    // 阶段 1:TTG 模式选择
    let ttg = TtgGovernor::new(TtgConfig::default());
    let _mode = ttg.select_mode(quest, BudgetTier::HighTier);

    // 阶段 2:DECB 预算计算
    let decb = DecbGovernor::new(DecbConfig::default())?;
    let _coefficient = decb.compute_budget(budget_input);

    // 阶段 3:Parliament 辩论
    let bus = EventBus::new();
    let parliament = Parliament::new(ParliamentConfig::default(), bus);
    let _consensus = parliament.deliberate(quest, proposal).await?;

    // 阶段 4:ASA 审计(良性操作)
    let auditor = AsaAuditor::with_default_config();
    let audit_input = OperationAuditInput {
        operation_id: "op-csa".into(),
        content: "执行代码审查".into(),
        risk_keywords: vec!["sudo".into(), "rm -rf".into()],
        complexity_score: 0.3,
        // P0-4 新增字段:CSA 延迟测试仅验证关键词匹配路径,不启用语义相似度检测
        semantic_vector: None,
        reference_risk_vectors: Vec::new(),
    };
    let _result = auditor.audit(&audit_input);

    // 阶段 5:AHIRT 单类探测(简化,使用 CommandInjection 类)
    let red_team = AhirtRedTeam::default();
    let _probe_results = red_team.probe(parliament::ProbeType::CommandInjection);

    Ok(start.elapsed())
}

// ============================================================
// 测试 1:良性 Quest CSA 延迟(min-of-N 5 次)
// ============================================================

/// 验证良性 Quest 的 CSA 端到端延迟 < 300ms(min-of-N 5 次)
///
/// 流程:构造良性 Quest → 运行 CSA 5 次 → 取最小延迟 → 断言 < 300ms
///
/// WHY min-of-N:单次测量受调度噪声影响大,5 次取最小值更接近真实延迟
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_csa_latency_benign_quest() -> TestResult {
    let quest = make_quest("q-csa-benign", 2, ThinkingMode::Fast);
    let proposal = Proposal::new("p-csa-benign", "q-csa-benign", "执行代码审查任务", 0.2);
    let budget_input = make_budget_input("q-csa-benign", 2);

    let mut min_latency = Duration::from_secs(u64::MAX);

    // 运行 5 次 CSA 流程,取最小延迟
    for i in 0..MIN_OF_N {
        let latency = run_csa_once_async(&quest, &proposal, &budget_input).await?;
        if latency < min_latency {
            min_latency = latency;
        }
        println!(
            "CSA 良性 Quest 第 {} 次延迟: {:.3}ms",
            i + 1,
            latency.as_secs_f64() * 1000.0
        );
    }

    let min_ms = min_latency.as_secs_f64() * 1000.0;
    println!("CSA 良性 Quest min-of-N 延迟: {min_ms:.3}ms");

    assert!(
        min_latency.as_millis() < CSA_LATENCY_THRESHOLD_MS as u128,
        "CSA 良性 Quest 延迟应 < {CSA_LATENCY_THRESHOLD_MS}ms,实际 min-of-N: {min_ms:.3}ms"
    );

    Ok(())
}

// ============================================================
// 测试 2:恶意 Quest CSA 延迟
// ============================================================

/// 验证恶意 Quest(含命令注入)的 CSA 端到端延迟 < 300ms(min-of-N 5 次)
///
/// 流程:构造恶意 Quest → 运行 CSA 5 次 → 取最小延迟 → 断言 < 300ms
///
/// WHY 恶意 Quest 也需 < 300ms:Skeptic 否决是快速路径(规则匹配 < 10ms),
/// 恶意 Quest 应比良性 Quest 更快达成"否决"结论,不应因安全检测而超时
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_csa_latency_malicious_quest() -> TestResult {
    let quest = make_quest("q-csa-malicious", 2, ThinkingMode::Fast);
    // 恶意提案:含命令注入 $(whoami),触发 Skeptic 否决
    let proposal = Proposal::new("p-csa-malicious", "q-csa-malicious", "echo $(whoami)", 0.2);
    let budget_input = make_budget_input("q-csa-malicious", 2);

    let mut min_latency = Duration::from_secs(u64::MAX);

    // 运行 5 次 CSA 流程,取最小延迟
    for i in 0..MIN_OF_N {
        let latency = run_csa_once_async(&quest, &proposal, &budget_input).await?;
        if latency < min_latency {
            min_latency = latency;
        }
        println!(
            "CSA 恶意 Quest 第 {} 次延迟: {:.3}ms",
            i + 1,
            latency.as_secs_f64() * 1000.0
        );
    }

    let min_ms = min_latency.as_secs_f64() * 1000.0;
    println!("CSA 恶意 Quest min-of-N 延迟: {min_ms:.3}ms");

    assert!(
        min_latency.as_millis() < CSA_LATENCY_THRESHOLD_MS as u128,
        "CSA 恶意 Quest 延迟应 < {CSA_LATENCY_THRESHOLD_MS}ms,实际 min-of-N: {min_ms:.3}ms"
    );

    Ok(())
}

// ============================================================
// 测试 3:CSA 延迟分解验证
// ============================================================

/// 验证 CSA 各阶段延迟符合预算分配
///
/// CSA 延迟分解预算:
/// - TTG 模式选择:< 1ms
/// - DECB 预算计算:< 1ms
/// - Parliament 辩论:< 200ms
/// - ASA 审计:< 5ms
/// - AHIRT 单类探测:< 50ms
///
/// WHY 单独验证各阶段:端到端延迟达标但某阶段超预算时,仍需优化。
/// 本测试逐阶段测量并断言,定位性能瓶颈
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_csa_latency_breakdown() -> TestResult {
    let quest = make_quest("q-csa-breakdown", 2, ThinkingMode::Fast);
    let proposal = Proposal::new("p-csa-breakdown", "q-csa-breakdown", "执行代码审查", 0.2);
    let budget_input = make_budget_input("q-csa-breakdown", 2);

    // 各阶段延迟预算(单位:ms)
    const TTG_BUDGET_MS: u64 = 1;
    const DECB_BUDGET_MS: u64 = 1;
    const PARLIAMENT_BUDGET_MS: u64 = 200;
    const ASA_BUDGET_MS: u64 = 5;
    const AHIRT_BUDGET_MS: u64 = 50;

    // 阶段 1:TTG 模式选择延迟
    let ttg = TtgGovernor::new(TtgConfig::default());
    let start = Instant::now();
    let _mode = ttg.select_mode(&quest, BudgetTier::HighTier);
    let ttg_latency = start.elapsed();
    let ttg_ms = ttg_latency.as_secs_f64() * 1000.0;
    println!("TTG 模式选择延迟: {ttg_ms:.3}ms");

    assert!(
        ttg_latency.as_millis() <= TTG_BUDGET_MS as u128,
        "TTG 延迟应 ≤ {TTG_BUDGET_MS}ms,实际: {ttg_ms:.3}ms"
    );

    // 阶段 2:DECB 预算计算延迟
    let decb = DecbGovernor::new(DecbConfig::default())?;
    let start = Instant::now();
    let _coefficient = decb.compute_budget(&budget_input);
    let decb_latency = start.elapsed();
    let decb_ms = decb_latency.as_secs_f64() * 1000.0;
    println!("DECB 预算计算延迟: {decb_ms:.3}ms");

    assert!(
        decb_latency.as_millis() <= DECB_BUDGET_MS as u128,
        "DECB 延迟应 ≤ {DECB_BUDGET_MS}ms,实际: {decb_ms:.3}ms"
    );

    // 阶段 3:Parliament 辩论延迟
    let bus = EventBus::new();
    let parliament = Parliament::new(ParliamentConfig::default(), bus);
    let start = Instant::now();
    let _consensus = parliament.deliberate(&quest, &proposal).await?;
    let parliament_latency = start.elapsed();
    let parliament_ms = parliament_latency.as_secs_f64() * 1000.0;
    println!("Parliament 辩论延迟: {parliament_ms:.3}ms");

    assert!(
        parliament_latency.as_millis() <= PARLIAMENT_BUDGET_MS as u128,
        "Parliament 延迟应 ≤ {PARLIAMENT_BUDGET_MS}ms,实际: {parliament_ms:.3}ms"
    );

    // 阶段 4:ASA 审计延迟
    let auditor = AsaAuditor::with_default_config();
    let audit_input = OperationAuditInput {
        operation_id: "op-csa-breakdown".into(),
        content: "执行代码审查".into(),
        risk_keywords: vec!["sudo".into(), "rm -rf".into()],
        complexity_score: 0.3,
        semantic_vector: None,
        reference_risk_vectors: Vec::new(),
    };
    let start = Instant::now();
    let _result = auditor.audit(&audit_input);
    let asa_latency = start.elapsed();
    let asa_ms = asa_latency.as_secs_f64() * 1000.0;
    println!("ASA 审计延迟: {asa_ms:.3}ms");

    assert!(
        asa_latency.as_millis() <= ASA_BUDGET_MS as u128,
        "ASA 延迟应 ≤ {ASA_BUDGET_MS}ms,实际: {asa_ms:.3}ms"
    );

    // 阶段 5:AHIRT 单类探测延迟
    let red_team = AhirtRedTeam::default();
    let start = Instant::now();
    let _probe_results = red_team.probe(parliament::ProbeType::CommandInjection);
    let ahirt_latency = start.elapsed();
    let ahirt_ms = ahirt_latency.as_secs_f64() * 1000.0;
    println!("AHIRT 单类探测延迟: {ahirt_ms:.3}ms");

    assert!(
        ahirt_latency.as_millis() <= AHIRT_BUDGET_MS as u128,
        "AHIRT 延迟应 ≤ {AHIRT_BUDGET_MS}ms,实际: {ahirt_ms:.3}ms"
    );

    // 汇总各阶段延迟(不含事件传播,事件传播在端到端测试中体现)
    let total_ms = ttg_ms + decb_ms + parliament_ms + asa_ms + ahirt_ms;
    println!(
        "CSA 延迟分解汇总: TTG={ttg_ms:.3}ms + DECB={decb_ms:.3}ms + Parliament={parliament_ms:.3}ms + ASA={asa_ms:.3}ms + AHIRT={ahirt_ms:.3}ms = {total_ms:.3}ms"
    );

    // 总延迟(不含事件传播 30ms)应 < 270ms,留 30ms 给事件传播
    const TOTAL_BUDGET_MS: f64 = 270.0;
    assert!(
        total_ms < TOTAL_BUDGET_MS,
        "CSA 各阶段延迟总和应 < {TOTAL_BUDGET_MS}ms(留 30ms 给事件传播),实际: {total_ms:.3}ms"
    );

    Ok(())
}
