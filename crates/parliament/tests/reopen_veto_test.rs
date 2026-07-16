//! reopen_veto 覆议机制集成测试 — Phase IV N8
//!
//! 验证 Skeptic 否决覆议的三个不变量:
//! 1. 有效票据 + 超级多数 → 进入辩论(不返回 Vetoed)
//! 2. 票据失配 → 否决生效(返回 Vetoed)
//! 3. 有效票据但赞成率 < 覆议阈值(0.667)→ 返回 Rejected
//!
//! # 测试 3 的阈值区分设计(WHY)
//! 场景 task=4 + Fast + risk=0.4 + content="echo $(whoami)" 产生:
//! - Architect 反对(0.25,task>3)、Skeptic 弃权(0.30,0.3<risk≤0.5)
//! - Optimizer 赞成(0.20,Fast)、Librarian 赞成(0.15,task≤5)、Bard 赞成(0.10)
//! - 赞成率 = 0.45 / 0.70 ≈ 0.643 ∈ [0.6, 0.667)
//!
//! 常规阈值(0.6)下会 Reached,覆议阈值(0.667)下会 Rejected — 精确区分两阈值。

use event_bus::EventBus;
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use parliament::{Consensus, Parliament, ParliamentConfig, Proposal, VetoOverrideTicket};

/// 构造测试用 Quest(指定任务数与思考模式)
fn make_quest(task_count: usize, thinking_mode: ThinkingMode) -> Quest {
    let tasks: Vec<Task> = (0..task_count)
        .map(|i| Task {
            task_id: format!("t-{i}"),
            description: format!("任务 {i}"),
            status: TaskStatus::Pending,
            dependencies: vec![],
        })
        .collect();
    Quest {
        quest_id: "q-reopen".into(),
        title: "覆议测试 Quest".into(),
        tasks,
        thinking_mode,
        checkpoint_id: None,
        priority: 128,
    }
}

/// 构造默认配置的议会
fn make_parliament() -> Parliament {
    let config = ParliamentConfig::default();
    let bus = EventBus::new();
    Parliament::new(config, bus)
}

// ============================================================
// 不变量 1:有效票据 + 超级多数 → 进入辩论,不返回 Vetoed
// ============================================================

#[tokio::test]
async fn test_reopen_veto_with_valid_ticket_allows_debate() {
    // 命令注入提案(触发 Skeptic 规则否决)+ 有效匹配票据 → 覆盖 → 辩论
    // 低风险 + 少任务 + Fast → 全赞成 → 赞成率 1.0 ≥ 0.667 → 共识达成
    let parliament = make_parliament();
    let quest = make_quest(2, ThinkingMode::Fast);
    let proposal = Proposal::new("p-reopen-1", "q-reopen", "echo $(whoami)", 0.2);
    let ticket = VetoOverrideTicket::new(
        "p-reopen-1",
        "false positive: legitimate shell script",
        "admin:alice",
    )
    .unwrap();

    let consensus = parliament
        .reopen_veto(&quest, &proposal, &ticket)
        .await
        .unwrap();

    // 有效票据应覆盖否决,进入辩论;全赞成应达成共识(不返回 Vetoed)
    assert!(
        !consensus.is_vetoed(),
        "有效票据应覆盖否决并进入辩论,实际: {consensus:?}"
    );
    assert!(
        consensus.is_reached(),
        "全赞成(赞成率 1.0 ≥ 0.667)应达成共识,实际: {consensus:?}"
    );
}

// ============================================================
// 不变量 2:票据失配 → 否决生效(返回 Vetoed)
// ============================================================

#[tokio::test]
async fn test_reopen_veto_mismatched_ticket_returns_vetoed() {
    // 命令注入提案 + 票据 proposal_id 不匹配 → 覆盖无效 → 否决生效
    let parliament = make_parliament();
    let quest = make_quest(2, ThinkingMode::Fast);
    let proposal = Proposal::new("p-real", "q-reopen", "echo $(whoami)", 0.2);
    let ticket = VetoOverrideTicket::new("p-wrong", "legitimate override", "admin:alice").unwrap();

    let consensus = parliament
        .reopen_veto(&quest, &proposal, &ticket)
        .await
        .unwrap();

    // 票据 proposal_id 不匹配 → 覆盖无效 → Skeptic 否决生效
    assert!(
        consensus.is_vetoed(),
        "票据失配应导致否决生效,实际: {consensus:?}"
    );
}

// ============================================================
// 不变量 3:有效票据但赞成率 < 覆议阈值(0.667)→ 返回 Rejected
// ============================================================

#[tokio::test]
async fn test_reopen_veto_supermajority_required() {
    // 场景见文件头注释:赞成率 ≈ 0.643 ∈ [0.6, 0.667)
    // - 常规阈值 0.6:0.643 ≥ 0.6 → Reached
    // - 覆议阈值 0.667:0.643 < 0.667 → Rejected
    // 此测试验证覆议路径使用了更高的 0.667 阈值
    let parliament = make_parliament();
    let quest = make_quest(4, ThinkingMode::Fast);
    let proposal = Proposal::new("p-reopen-3", "q-reopen", "echo $(whoami)", 0.4);
    let ticket =
        VetoOverrideTicket::new("p-reopen-3", "false positive: legitimate", "admin:bob").unwrap();

    let consensus = parliament
        .reopen_veto(&quest, &proposal, &ticket)
        .await
        .unwrap();

    // 有效票据覆盖否决,但赞成率 0.643 < 0.667(覆议超级多数)→ Rejected
    assert!(
        !consensus.is_vetoed(),
        "有效票据覆盖后不应返回 Vetoed,实际: {consensus:?}"
    );
    assert!(
        consensus.is_rejected(),
        "赞成率 0.643 < 覆议阈值 0.667 应返回 Rejected,实际: {consensus:?}"
    );
    // 精确断言:应为 Rejected(非 Vetoed,因 Skeptic 辩论中弃权)
    assert!(
        matches!(consensus, Consensus::Rejected { .. }),
        "应为 Consensus::Rejected,实际: {consensus:?}"
    );
}
