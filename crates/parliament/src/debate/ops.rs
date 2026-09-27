//! 审议侧算子 — 共识标签映射与单角色 Opinion 生成(自 `debate.rs` 下沉,B5 模块内聚)

use crate::types::Consensus;
use crate::types::Opinion;
use crate::types::Proposal;
use crate::types::Role;
use nexus_core::Quest;
use nexus_core::ThinkingMode;
/// 共识结果→审议结果标签(DebateCompleted 事件的 outcome 字段)
///
/// WHY 独立函数:标签取值("Reached"/"Rejected"/"Vetoed")是事件契约的
/// 一部分,集中定义避免各调用点字符串漂移。
pub(super) fn consensus_outcome_label(consensus: &Consensus) -> &'static str {
    match consensus {
        Consensus::Reached { .. } => "Reached",
        Consensus::Rejected { .. } => "Rejected",
        Consensus::Vetoed { .. } => "Vetoed",
    }
}

/// 生成单个角色的 Opinion(占位实现)
///
/// WHY 占位实现:Week 5 阶段 NMC 未接入,无法调用真实模型推理。
/// 基于 Quest 特征(任务数、思考模式)与 Proposal 特征(risk_level)
/// 的规则化生成,模拟 5 角色的差异化决策倾向。
///
/// Week 6 NMC 接入后,此函数替换为模型推理调用。
///
/// # 各角色决策规则(占位)
/// - **Architect**:任务数少(≤3)→ 赞成(架构简单),多 → 反对(复杂度高)
/// - **Skeptic**:risk_level > 0.5 → 反对(风险厌恶),0.3-0.5 → 弃权,< 0.3 → 赞成
///   若跨厂商辩论启用且 cross_vendor_enforced=true,置信度 +0.05(异厂商通道独立性更强)
/// - **Optimizer**:Fast 模式 → 赞成(快速),Standard → 弃权,Deep → 反对(慢)
/// - **Librarian**:任务数 ≤ 5 → 赞成(有先例),> 5 → 弃权(无先例)
/// - **Bard**:总是赞成(创意发散,鼓励尝试)
///
/// # 参数
/// - `cross_vendor`:跨厂商辩论角色分配（可选，Skeptic 角色使用异厂商通道时置信度提升）
pub(super) async fn generate_opinion(
    role: Role,
    quest: &Quest,
    proposal: &Proposal,
    cross_vendor: Option<&crate::cross_vendor::CrossVendorAssignment>,
) -> Opinion {
    // 模拟异步 Opinion 生成(Week 6 接入真实模型后替换)
    // WHY yield:让出调度,允许 FuturesUnordered 并发处理其他角色
    tokio::task::yield_now().await;

    let task_count = quest.tasks.len();
    let risk = proposal.risk_level;

    // MCA P2-1:判断 Skeptic 是否使用异厂商通道（跨厂商辩论启用且强制去相关）
    let skeptic_cross_vendor = cross_vendor
        .filter(|cv| cv.cross_vendor_enforced)
        .map(|cv| cv.skeptic_provider.as_str().to_string());

    match role {
        Role::Architect => {
            // 架构师:任务数少 → 赞成,多 → 反对
            if task_count <= 3 {
                Opinion::new(
                    Role::Architect,
                    1.0,
                    0.85,
                    format!("架构简单({task_count} 任务),赞成"),
                )
            } else {
                Opinion::new(
                    Role::Architect,
                    0.0,
                    0.80,
                    format!("架构复杂({task_count} 任务),反对"),
                )
            }
        }
        Role::Skeptic => {
            // 怀疑者:风险厌恶,red team 视角
            // 若跨厂商辩论启用且强制去相关,使用异厂商通道的 Skeptic 置信度更高
            let base_confidence = if skeptic_cross_vendor.is_some() {
                0.98 // 异厂商通道独立性更强,置信度更高
            } else {
                0.95
            };

            // 在 rationale 中注明跨厂商状态（如果启用）
            let cross_vendor_note = match &skeptic_cross_vendor {
                Some(provider) => format!("[跨厂商:Skeptic 来自 {provider},与 Producer 异厂商] "),
                None => String::new(),
            };

            if risk > 0.5 {
                Opinion::new(
                    Role::Skeptic,
                    0.0,
                    base_confidence,
                    format!("{cross_vendor_note}高风险(risk={risk:.2}),否决"),
                )
            } else if risk > 0.3 {
                Opinion::new(
                    Role::Skeptic,
                    0.5,
                    (base_confidence - 0.25).max(0.5), // 弃权时置信度适中
                    format!("{cross_vendor_note}中风险(risk={risk:.2}),弃权"),
                )
            } else {
                Opinion::new(
                    Role::Skeptic,
                    1.0,
                    base_confidence - 0.03, // 赞成时信心略低于否决
                    format!("{cross_vendor_note}低风险(risk={risk:.2}),赞成"),
                )
            }
        }
        Role::Optimizer => {
            // 优化者:关注执行效率
            match quest.thinking_mode {
                ThinkingMode::Fast => {
                    Opinion::new(Role::Optimizer, 1.0, 0.85, "Fast 模式,性能优先,赞成")
                }
                ThinkingMode::Standard => {
                    Opinion::new(Role::Optimizer, 0.5, 0.70, "Standard 模式,性能中等,弃权")
                }
                ThinkingMode::Deep => {
                    Opinion::new(Role::Optimizer, 0.0, 0.80, "Deep 模式,性能开销大,反对")
                }
            }
        }
        Role::Librarian => {
            // 图书馆员:任务数少 → 有先例 → 赞成
            if task_count <= 5 {
                Opinion::new(
                    Role::Librarian,
                    1.0,
                    0.75,
                    format!("任务数 {task_count},有历史先例,赞成"),
                )
            } else {
                Opinion::new(
                    Role::Librarian,
                    0.5,
                    0.60,
                    format!("任务数 {task_count},无充分先例,弃权"),
                )
            }
        }
        Role::Bard => {
            // 吟游诗人:创意发散,总是赞成
            Opinion::new(Role::Bard, 1.0, 0.65, "创意方案,鼓励尝试,赞成")
        }
    }
}
