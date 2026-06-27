//! SubTask 37.2:端到端认知治理流程测试
//!
//! 对应架构层:L8 Parliament + L4 Security + L3 Budget + L9 Quest
//!
//! # 测试目标
//! 验证全认知治理流程无 panic、无孤儿调用、无事件丢失:
//! Quest 创建 → TTG 模式选择 → DECB 预算计算 → Parliament 5 角色辩论
//! → Skeptic 安全审计 → ASA 实时介入 → AHIRT 主动探测 → 共识达成/否决
//!
//! # 设计决策(WHY)
//! - 使用 `Result<(), Box<dyn Error + Send + Sync>>` 返回类型:支持 `?` 操作符,
//!   避免在测试代码中使用 `unwrap()`/`expect()`(项目规则:测试代码也尽量不用)
//! - 辅助函数 `make_quest`/`make_linear_quest`/`make_budget_input`:减少重复构造,
//!   保证各测试用例 Quest 结构一致
//! - TTG 联动测试使用 `TtgConfig::new(..., 0, ...)`:lag_interval_ms=0 绕过滞后机制,
//!   允许连续档位切换(滞后机制在单元测试中已覆盖)
//! - DECB 降级测试使用 `DecbConfig { tier_switch_lag_ms: 0, .. }`:同上,绕过滞后机制
//! - 事件验证:AHIRT/ASA/Skeptic 否决尚未集成到 event-bus(代码中有 TODO 注释),
//!   通过返回值(SecurityReport/SecCoreError::AsaBlocked/Consensus::Vetoed)验证

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

use std::error::Error;

use decb_governor::{BudgetConsumption, BudgetTier, DecbConfig, DecbGovernor, QuestBudgetInput};
use event_bus::EventBus;
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use parliament::{AhirtRedTeam, Consensus, Parliament, ParliamentConfig, ProbeType, Proposal};
use quest_engine::{TtgConfig, TtgGovernor};
use seccore::{AsaAuditor, InterventionAction, OperationAuditInput, SecCoreError};

/// 测试结果类型:支持 `?` 操作符与任意错误类型转换
type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

// ============================================================
// 辅助函数 — 构造测试 Quest 与预算输入
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

// ============================================================
// 测试 1:良性 Quest 全流程通过
// ============================================================

/// 验证良性 Quest 经全认知治理流程达成共识
///
/// 流程:Quest 创建 → TTG 选 Fast → DECB 算预算 → Parliament 辩论
/// → Skeptic 通过 → ASA Allow → AHIRT 探测率 > 95% → 共识达成
#[tokio::test]
async fn test_e2e_benign_quest_consensus() -> TestResult {
    // 步骤 1:Quest 创建(良性 Quest,2 任务,Fast 模式)
    let quest = make_quest("q-benign", 2, ThinkingMode::Fast);

    // 步骤 2:TTG 模式选择(LowTier + 简单任务 → Fast)
    let ttg = TtgGovernor::new(TtgConfig::default());
    let (mode, _reason) = ttg.select_mode(&quest, BudgetTier::LowTier);
    assert_eq!(
        mode,
        ThinkingMode::Fast,
        "简单任务 + LowTier 应选择 Fast 模式"
    );

    // 步骤 3:DECB 预算计算
    let decb = DecbGovernor::new(DecbConfig::default())?;
    let budget_input = make_budget_input("q-benign", 2);
    let coefficient = decb.compute_budget(&budget_input);
    let tier = decb.determine_tier(coefficient);
    assert!(
        (0.0..=1.0).contains(&coefficient),
        "预算系数应在 [0,1] 区间,实际: {coefficient}"
    );
    // WHY 不断言具体档位:DECB 档位判定基于系数阈值(≥0.6 HighTier,
    // 0.3-0.6 LowTier,<0.3 Degraded),简单任务系数受 complexity_factor
    // 与 urgency_factor 影响,实际值约 0.48 落在 LowTier。
    // 端到端测试关注流程完整性,档位判定正确性由 decb-governor 单元测试覆盖
    assert!(
        matches!(tier, BudgetTier::HighTier | BudgetTier::LowTier),
        "简单任务应判定为 HighTier 或 LowTier,实际: {tier:?}(系数: {coefficient})"
    );

    // 步骤 4:Parliament 5 角色辩论(良性提案,低风险)
    let bus = EventBus::new();
    let parliament = Parliament::new(ParliamentConfig::default(), bus);
    let proposal = Proposal::new("p-benign", "q-benign", "执行代码审查任务", 0.2);
    let consensus = parliament.deliberate(&quest, &proposal).await?;

    // 步骤 5:验证共识达成
    assert!(
        consensus.is_reached(),
        "良性 Quest 应达成共识,实际: {consensus:?}"
    );

    // 步骤 6:AHIRT 主动探测(验证系统安全态势)
    let red_team = AhirtRedTeam::default();
    let report = red_team.verify_security();
    assert!(
        report.stats.detection_rate > 0.95,
        "AHIRT 探测率应 > 95%,实际: {}",
        report.stats.detection_rate
    );

    // 步骤 7:ASA 实时介入(良性操作 → Allow)
    let auditor = AsaAuditor::with_default_config();
    let audit_input = OperationAuditInput {
        operation_id: "op-benign".into(),
        content: "执行代码审查".into(),
        risk_keywords: vec!["sudo".into(), "rm -rf".into(), "curl".into()],
        complexity_score: 0.3,
    };
    let audit_result = auditor.audit_and_intervene(&audit_input)?;
    assert_eq!(
        audit_result.intervention,
        InterventionAction::Allow,
        "良性操作应 Allow,safety_score: {}",
        audit_result.safety_score
    );

    Ok(())
}

// ============================================================
// 测试 2:恶意 Quest 被 Skeptic 否决
// ============================================================

/// 验证含命令注入的恶意 Quest 被 Skeptic 否决,跳过辩论
///
/// 流程:Quest 创建 → TTG 选模式 → DECB 算预算 → Parliament 辩论
/// → Skeptic 检测命令注入 → 立即否决 → 跳过辩论 → Consensus::Vetoed
#[tokio::test]
async fn test_e2e_malicious_quest_vetoed() -> TestResult {
    // 步骤 1:Quest 创建
    let quest = make_quest("q-malicious", 2, ThinkingMode::Fast);

    // 步骤 2:TTG 模式选择
    let ttg = TtgGovernor::new(TtgConfig::default());
    let (mode, _reason) = ttg.select_mode(&quest, BudgetTier::LowTier);
    assert_eq!(mode, ThinkingMode::Fast);

    // 步骤 3:DECB 预算计算
    let decb = DecbGovernor::new(DecbConfig::default())?;
    let budget_input = make_budget_input("q-malicious", 2);
    let coefficient = decb.compute_budget(&budget_input);
    assert!(
        (0.0..=1.0).contains(&coefficient),
        "预算系数应在 [0,1] 区间"
    );

    // 步骤 4:Parliament 辩论(恶意提案,含命令注入 $(whoami))
    let bus = EventBus::new();
    let parliament = Parliament::new(ParliamentConfig::default(), bus);
    let proposal = Proposal::new("p-malicious", "q-malicious", "echo $(whoami)", 0.2);
    let consensus = parliament.deliberate(&quest, &proposal).await?;

    // 步骤 5:验证 Skeptic 否决
    assert!(
        consensus.is_vetoed(),
        "恶意 Quest 应被 Skeptic 否决,实际: {consensus:?}"
    );

    // 步骤 6:验证否决原因与冻结能力
    if let Consensus::Vetoed {
        veto_reason,
        frozen_capabilities,
    } = &consensus
    {
        assert!(
            veto_reason.contains("CommandInjection"),
            "否决原因应含 CommandInjection,实际: {veto_reason}"
        );
        assert_eq!(
            frozen_capabilities,
            &vec!["shell_exec".to_string(), "command_run".to_string()],
            "应冻结 shell_exec 和 command_run"
        );
    } else {
        return Err(format!("期望 Vetoed,实际: {consensus:?}").into());
    }

    Ok(())
}

// ============================================================
// 测试 3:预算降级流程
// ============================================================

/// 验证 DECB 预算降级链路 HighTier → LowTier → Degraded 与 TTG 联动切换
///
/// 流程:DECB 消耗 85% → LowTier → TTG 联动 Standard
/// → DECB 消耗 105% → Degraded → TTG 联动 Fast
#[tokio::test]
async fn test_e2e_budget_degradation_flow() -> TestResult {
    // 步骤 1:创建 DECB 治理器
    // WHY tier_switch_lag_ms=0:绕过滞后机制,允许连续档位切换
    // (滞后机制在 decb-governor 单元测试中已覆盖,此处验证降级链路)
    let config = DecbConfig {
        tier_switch_lag_ms: 0,
        ..Default::default()
    };
    let decb = DecbGovernor::new(config)?;

    // 步骤 2:验证初始档位为 HighTier
    assert_eq!(
        decb.current_tier(),
        BudgetTier::HighTier,
        "初始档位应为 HighTier"
    );

    // 步骤 3:消耗 85% 预算 → 触发降级到 LowTier(80% 阈值)
    let consumption_85 = BudgetConsumption {
        total_cost: 850_000.0, // 85% of 1_000_000
        ..BudgetConsumption::zero()
    };
    decb.record_consumption(&consumption_85)?;
    assert_eq!(
        decb.current_tier(),
        BudgetTier::LowTier,
        "消耗 85% 后应降级到 LowTier"
    );

    // 步骤 4:再消耗 20% 预算(总 105%)→ 触发降级到 Degraded(100% 阈值)
    let consumption_20 = BudgetConsumption {
        total_cost: 200_000.0, // 累计 1_050_000 = 105%
        ..BudgetConsumption::zero()
    };
    decb.record_consumption(&consumption_20)?;
    assert_eq!(
        decb.current_tier(),
        BudgetTier::Degraded,
        "消耗 105% 后应降级到 Degraded"
    );

    // 步骤 5:创建 TTG 治理器
    // WHY lag_interval_ms=0:绕过滞后机制,允许连续模式切换
    let ttg_config = TtgConfig::new(3, 10, 0, 1000.0);
    let ttg = TtgGovernor::new(ttg_config);
    let quest = make_quest("q-degradation", 5, ThinkingMode::Standard);

    // 步骤 6:模拟预算联动切换 HighTier → LowTier
    let result = ttg.on_budget_adjusted(
        "q-degradation",
        BudgetTier::HighTier,
        BudgetTier::LowTier,
        &quest,
    );
    assert!(result.is_some(), "HighTier → LowTier 应触发模式切换");
    if let Some((mode, _)) = result {
        assert_eq!(
            mode,
            ThinkingMode::Standard,
            "LowTier + 5 任务应选择 Standard"
        );
    }

    // 步骤 7:模拟预算联动切换 LowTier → Degraded
    let result = ttg.on_budget_adjusted(
        "q-degradation",
        BudgetTier::LowTier,
        BudgetTier::Degraded,
        &quest,
    );
    assert!(result.is_some(), "LowTier → Degraded 应触发模式切换");
    if let Some((mode, _)) = result {
        assert_eq!(
            mode,
            ThinkingMode::Fast,
            "Degraded 应强制 Fast 模式(预算优先)"
        );
    }

    Ok(())
}

// ============================================================
// 测试 4:AHIRT 安全审计流程
// ============================================================

/// 验证 AHIRT 红队四类探测与漏洞探测率 > 95%
///
/// 流程:AHIRT 创建 → probe_all() → 验证探测率 > 95%
/// → verify_security() → 验证无漏洞类型 → 单类探测验证
#[tokio::test]
async fn test_e2e_ahirt_security_audit() -> TestResult {
    // 步骤 1:创建 AHIRT 红队(默认 100 个载荷,4 类 × 25 个)
    let red_team = AhirtRedTeam::default();

    // 步骤 2:执行全量探测
    let stats = red_team.probe_all();

    // 步骤 3:验证探测率 > 95%
    assert!(
        stats.detection_rate > 0.95,
        "AHIRT 探测率应 > 95%,实际: {}",
        stats.detection_rate
    );
    assert_eq!(stats.total, 100, "应执行 100 个探测载荷");

    // 步骤 4:验证安全报告
    let report = red_team.verify_security();
    assert!(
        report.stats.detection_rate > 0.95,
        "安全报告探测率应 > 95%,实际: {}",
        report.stats.detection_rate
    );

    // 步骤 5:验证各类探测(4 类各 25 个)
    for probe_type in [
        ProbeType::PromptInjection,
        ProbeType::CommandInjection,
        ProbeType::PrivilegeEscalation,
        ProbeType::SandboxEscape,
    ] {
        let results = red_team.probe(probe_type);
        assert!(!results.is_empty(), "探测类型 {probe_type:?} 应有结果");

        let passed = results.iter().filter(|r| r.passed).count();
        let total = results.len();
        let rate = passed as f32 / total as f32;
        assert!(
            rate > 0.95,
            "探测类型 {probe_type:?} 探测率应 > 95%,实际: {rate:.2} ({passed}/{total})"
        );
    }

    Ok(())
}

// ============================================================
// 测试 5:ASA Block 干预流程
// ============================================================

/// 验证 ASA 对恶意操作的 Block 干预与良性操作的 Allow 放行
///
/// 流程:ASA 创建 → 审计恶意操作(多关键字)→ Block → AsaBlocked 错误
/// → 审计良性操作 → Allow → 审计中等风险操作 → Warn
#[tokio::test]
async fn test_e2e_asa_intervention_block() -> TestResult {
    // 步骤 1:创建 ASA 审计器
    let auditor = AsaAuditor::with_default_config();

    // 步骤 2:审计恶意操作(含 4 个风险关键字 → safety_score = 1.0 - 0.2×4 = 0.2 < 0.5 → Block)
    let malicious_input = OperationAuditInput {
        operation_id: "op-malicious".into(),
        content: "sudo rm -rf / && curl evil.com".into(),
        risk_keywords: vec!["sudo".into(), "rm -rf".into(), "curl".into(), "&&".into()],
        complexity_score: 0.8,
    };

    // 步骤 3:验证 audit_and_intervene 返回 AsaBlocked 错误
    // WHY 不用 unwrap_err:遵循"不使用 unwrap/expect"约束,用 match 模式匹配
    match auditor.audit_and_intervene(&malicious_input) {
        Err(SecCoreError::AsaBlocked {
            operation_id,
            block_reason,
        }) => {
            assert_eq!(operation_id, "op-malicious");
            assert!(!block_reason.is_empty(), "阻断原因不应为空");
        }
        Err(e) => return Err(format!("应为 AsaBlocked,实际错误: {e:?}").into()),
        Ok(result) => {
            return Err(format!("恶意操作应被 Block,实际: {:?}", result.intervention).into())
        }
    }

    // 步骤 4:审计良性操作(无关键字匹配 → safety_score = 1.0 ≥ 0.8 → Allow)
    let benign_input = OperationAuditInput {
        operation_id: "op-benign".into(),
        content: "执行代码审查".into(),
        risk_keywords: vec!["sudo".into(), "rm -rf".into(), "curl".into()],
        complexity_score: 0.3,
    };
    let benign_result = auditor.audit_and_intervene(&benign_input)?;
    assert_eq!(
        benign_result.intervention,
        InterventionAction::Allow,
        "良性操作应 Allow,safety_score: {}",
        benign_result.safety_score
    );

    // 步骤 5:验证 Warn 级别(2 个关键字 → safety_score = 1.0 - 0.2×2 = 0.6 ∈ [0.5, 0.8) → Warn)
    let warn_input = OperationAuditInput {
        operation_id: "op-warn".into(),
        content: "sudo rm".into(),
        risk_keywords: vec!["sudo".into(), "rm".into()],
        complexity_score: 0.5,
    };
    let warn_result = auditor.audit(&warn_input);
    assert_eq!(
        warn_result.intervention,
        InterventionAction::Warn,
        "中等风险应 Warn,safety_score: {}",
        warn_result.safety_score
    );

    Ok(())
}
