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
use nexus_core::{MultimodalInput, UserIntent};
use parliament::{Consensus, Parliament, ParliamentConfig, Proposal};
use uuid::Uuid;

use crate::call_budget::{call_with_budget, CallBudgetError, DEFAULT_CALL_BUDGET};
use crate::composition::AppContext;
use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::output;
use crate::permission::PermissionCtx;
use tokio_util::sync::CancellationToken;

/// M2 预算包装错误 → ChimeraCliError 映射(parliament 版)
///
/// WHY 沿用 EngineError:与 quest 分解失败的既有映射同语义(退出码 3,
/// 全局 ADR-060 矩阵)——预算耗尽属引擎/预算类故障,不引入新错误面
/// (对外 CLI 行为无感)。
fn quest_budget_error(e: CallBudgetError) -> ChimeraCliError {
    ChimeraCliError::EngineError(format!("Quest 分解失败: {e}"))
}

/// 执行 parliament 审议命令 — 兼容层 thin wrapper（M4-P1）
///
/// 独立调用场景（库调用方/既有测试）经组合根装配 ephemeral AppContext。
/// dispatch 主链直接调 [`execute_with_ctx`] 共享 dispatch 级 AppContext。
///
/// `proposal` 为待审议的决策描述文本,`config` 提供引擎配置,
/// `json` flag 控制输出格式,`perm` 预留供未来权限检查。
pub async fn execute(
    proposal: &str,
    config: &ChimeraConfig,
    json: bool,
    _perm: &PermissionCtx,
) -> Result<()> {
    let ctx = crate::composition::build(config)?;
    execute_with_ctx(&ctx, proposal, json, _perm).await
}

/// parliament 审议主体 — 从组合根 AppContext 取共享 bus + engine（M4-P1）
pub async fn execute_with_ctx(
    ctx: &AppContext,
    proposal: &str,
    json: bool,
    _perm: &PermissionCtx,
) -> Result<()> {
    tracing::info!(proposal = %proposal, "议会审议提案");

    // 1. 共享 bus/engine 来自组合根（C12）；Parliament 与 QuestEngine
    //    各持 bus 引用（EventBus Clone = Arc 廉价共享）。
    //    WHY bus 引用:M2 的 call_with_budget 还需持 bus 发布
    //    OperationTimedOut 事件(本层超时兜底)。
    let bus = &ctx.bus;
    let engine = &ctx.engine;
    let parliament = Parliament::new(ParliamentConfig::default(), bus.clone());

    // 2. 将 proposal 封装为 UserIntent,经 QuestEngine 分解为 Quest
    //    WHY 先分解:Parliament::deliberate 需要 &Quest 上下文(任务数、思考模式)
    //    M2:create_quest 经 call_with_budget 注入本层超时(120s,对齐
    //    mca-gateway per-endpoint 口径)+ CancellationToken;超时发
    //    OperationTimedOut 事件,预算错误沿用 quest_budget_error 的
    //    EngineError 映射。token 为新建未触发令牌,预留取消链路接线。
    let intent = UserIntent {
        intent_id: format!("intent-{}", Uuid::now_v7()),
        raw_text: proposal.to_string(),
        multimodal_inputs: vec![MultimodalInput::Text(proposal.to_string())],
        risk_level: 0,
    };
    let token = CancellationToken::new();
    let quest = call_with_budget(
        engine.create_quest(intent),
        DEFAULT_CALL_BUDGET,
        &token,
        "parliament.create_quest",
        bus,
    )
    .await
    .map_err(quest_budget_error)? // 外层:CallBudgetError(M2 预算包装)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call_budget::CallBudgetError;

    // ---- M2 复核增量:call_with_budget 接入 parliament 的映射验证(TDD:先写验证) ----

    #[test]
    fn parliament_budget_error_uses_engine_error_mapping() {
        // 沿用既有"Quest 分解失败"EngineError 映射(退出码 3,全局矩阵),不新增错误面
        let err = quest_budget_error(CallBudgetError::Timeout {
            operation_id: "parliament.create_quest".into(),
            budget_ms: 120_000,
        });
        assert_eq!(err.kind(), "EngineError");
        let msg = err.message();
        assert!(
            msg.starts_with("Quest 分解失败: "),
            "消息应沿用既有分解失败前缀,实际: {msg}"
        );
        assert_eq!(err.exit_code_value(), 3);
    }

    #[tokio::test]
    async fn parliament_create_quest_normal_path_with_budget() {
        // 包装后的正常路径回归(行为无感):端到端 execute 仍 Ok
        let config = ChimeraConfig::default();
        let perm = PermissionCtx::default();
        execute("是否引入缓存层,请审议", &config, false, &perm)
            .await
            .expect("parliament 正常路径应成功");
    }

    /// M4-P1：parliament 经 execute_with_ctx 走组合根共享 bus ——
    /// QuestCreated 与审议事件对外部订阅者可见（此前命令自建私有 bus，事件零可见）。
    /// 此测试先行失败（红）：execute_with_ctx 尚不存在。
    #[tokio::test]
    async fn parliament_events_visible_on_shared_context_bus() {
        use std::time::Duration;
        let ctx = crate::composition::build(&ChimeraConfig::default()).expect("装配应成功");
        // §4.4 反模式 3：先 subscribe 再驱动命令，否则事件静默丢失
        let mut rx = ctx.bus.subscribe();
        let perm = PermissionCtx::default();
        execute_with_ctx(&ctx, "是否引入缓存层,请审议", false, &perm)
            .await
            .expect("parliament 正常路径应成功");
        let mut saw_quest_created = false;
        for _ in 0..40 {
            match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
                Ok(Ok(event_bus::NexusEvent::QuestCreated { .. })) => {
                    saw_quest_created = true;
                    break;
                }
                Ok(Ok(_)) => continue,
                _ => break,
            }
        }
        assert!(
            saw_quest_created,
            "QuestCreated 应在组合根共享 bus 上可见（命令不再自建私有 bus）"
        );
    }
}
