//! `chimera parliament <proposal>` — 议会审议,真实接入 L8 Parliament crate
//!
//! v2.9.0-omega Task 1.4:替换 NotImplemented 占位,真实调用 Parliament::deliberate。
//!
//! # 流程
//! 1. 构造进程内 EventBus + QuestEngine + Parliament(共享 EventBus)
//! 2. 将 proposal 文本封装为 UserIntent,经 QuestEngine::create_quest 分解为 Quest
//!    (对齐 §5.2 数据流:用户输入 → Quest 分解 → Parliament 审议)
//! 3. 从 Quest 构建 Proposal(proposal_id / quest_id / content / risk_level)
//! 4. 调用 Parliament::deliberate(&quest, &proposal) 执行 5 角色对抗性审议
//! 5. 输出审议过程(角色辩论 + 投票 + SkepticVeto)与共识结果
//!
//! # 设计决策(WHY)
//! - **先 QuestEngine 分解再 Parliament 审议**:对齐架构数据流(§5.2),
//!   Parliament 需要 Quest 上下文(任务数、思考模式)来评估提案复杂度。
//!   直接用 proposal 文本构造 dummy Quest 会丢失分解信息,审议质量低。
//! - **进程内 ephemeral 引擎**:与 `chimera run` / `quest` 一致,不持久化。
//!   Parliament 审议结果不跨进程保留,适合"快速审议看共识"场景。
//! - **审议过程输出到 stderr,共识结果到 stdout**:WHY 分流 — 审议过程是诊断信息
//!   (人类可读),共识结果是数据(stdout 便于 `jq` 消费)。`--json` 时两者均走 JSON envelope。
//!
//! v2.9.0-omega Task 1.7:接受 `json` flag(共识结果 envelope 在本命令输出)

use anyhow::Result;
use event_bus::EventBus;
use nexus_core::{MultimodalInput, UserIntent};
use parliament::{Consensus, Parliament, ParliamentConfig, Proposal};
use quest_engine::QuestEngine;
use uuid::Uuid;

use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::output;
use crate::permission::PermissionCtx;

/// 执行 parliament 审议命令 — 真实接入 Parliament API
///
/// `proposal` 为待审议的决策描述文本,`config` 提供引擎配置,
/// `json` flag 控制输出格式,`perm` 预留供未来权限检查。
pub async fn execute(
    proposal: &str,
    _config: &ChimeraConfig,
    json: bool,
    _perm: &PermissionCtx,
) -> Result<()> {
    tracing::info!(proposal = %proposal, "议会审议提案");

    // 1. 构造进程内 ephemeral 引擎(EventBus 共享,QE + Parliament 各持一份 clone)
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus.clone());
    let parliament = Parliament::new(ParliamentConfig::default(), bus);

    // 2. 将 proposal 封装为 UserIntent,经 QuestEngine 分解为 Quest
    //    WHY 先分解:Parliament::deliberate 需要 &Quest 上下文(任务数、思考模式)
    let intent = UserIntent {
        intent_id: format!("intent-{}", Uuid::now_v7()),
        raw_text: proposal.to_string(),
        multimodal_inputs: vec![MultimodalInput::Text(proposal.to_string())],
        risk_level: 0,
    };
    let quest = engine
        .create_quest(intent)
        .await
        .map_err(|e| ChimeraCliError::EngineError(format!("Quest 分解失败: {e}")))?;

    // 3. 从 Quest 构建 Proposal(UUIDv7 时间有序,关联 quest_id)
    let proposal_obj = Proposal::new(
        format!("proposal-{}", Uuid::now_v7()),
        &quest.quest_id,
        proposal,
        0.0, // risk_level=0(低风险),CLI 提案默认低风险,真实风险由 Skeptic 检测
    );

    // 4. 真实 L8 审议:5 角色对抗性辩论 + Skeptic 否决 + 加权投票
    //    deliberate 内部发布 DebateStarted / VoteCast / ConsensusReached 事件
    let consensus = parliament
        .deliberate(&quest, &proposal_obj)
        .await
        .map_err(|e| ChimeraCliError::EngineError(format!("议会审议失败: {e}")))?;

    // 5. 输出
    if json {
        // JSON 模式:输出共识结果 + Quest 上下文 envelope
        let payload = serde_json::json!({
            "quest_id": quest.quest_id,
            "quest_title": quest.title,
            "task_count": quest.tasks.len(),
            "thinking_mode": format!("{:?}", quest.thinking_mode),
            "proposal_id": proposal_obj.proposal_id,
            "consensus": consensus,
        });
        output::print_json(&payload)?;
    } else {
        // 人类可读模式:审议上下文到 stderr,共识结果到 stdout
        eprintln!("=== 议会审议 ===");
        eprintln!(
            "Quest: {} ({}, {} 任务, {:?})",
            quest.quest_id,
            quest.title,
            quest.tasks.len(),
            quest.thinking_mode
        );
        eprintln!("提案: {}", proposal_obj.content);
        eprintln!("风险等级: {:.2}", proposal_obj.risk_level);
        eprintln!("--- 审议结果 ---");
        print_consensus_human(&consensus);
    }

    Ok(())
}

/// 人类可读模式输出共识结果(SubTask 1.4.2)
///
/// 三种共识结果格式化:
/// - `Reached`:决议哈希 + 可选 DPO 训练对 ID
/// - `Rejected`:拒绝原因
/// - `Vetoed`:Skeptic 否决原因 + 冻结能力列表
fn print_consensus_human(consensus: &Consensus) {
    match consensus {
        Consensus::Reached {
            decision_hash,
            dpo_pair_id,
        } => {
            output::print_success("共识达成 ✓");
            println!("决议哈希: {decision_hash}");
            if let Some(pair_id) = dpo_pair_id {
                println!("DPO 训练对: {pair_id}");
            }
        }
        Consensus::Rejected { reason } => {
            output::print_warning("提案被拒绝 ⚠");
            println!("拒绝原因: {reason}");
        }
        Consensus::Vetoed {
            veto_reason,
            frozen_capabilities,
        } => {
            output::print_error("Skeptic 否决 ✗(红队防线触发)");
            println!("否决原因: {veto_reason}");
            if !frozen_capabilities.is_empty() {
                println!("冻结能力: {}", frozen_capabilities.join(", "));
            }
        }
    }
}
