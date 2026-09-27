//! DPO 训练对 — 共识达成后的偏好优化数据(自 `debate.rs` 下沉,B5 模块内聚)

use crate::types::Consensus;
use crate::types::Opinion;
use uuid::Uuid;
// ============================================================
// DPO 训练对 — 共识达成后生成的偏好优化训练数据
// ============================================================

/// DPO 训练对 — 从辩论中提取的 chosen/rejected Opinion 对
///
/// WHY DPO(Direct Preference Optimization):共识达成时,赞成方与反对方
/// 的 Opinion 形成天然的好/坏决策对比,供 AutoDPO 进行偏好优化训练。
/// 经 ConsensusReached 事件的 `dpo_pair_id` 字段传递(不直接调用 AutoDPO,
/// 避免向上依赖 L5,符合 §2.2 依赖铁律)。
///
/// # 字段
/// - `chosen`:赞成立场(position=1.0)中置信度最高的 Opinion
/// - `rejected`:反对立场(position=0.0)中置信度最高的 Opinion
/// - `context`:quest_id + 决策哈希,供训练时还原决策上下文
#[derive(Debug, Clone, PartialEq)]
pub struct DpoPair {
    /// 训练对唯一 ID(UUIDv7,时间有序便于追溯)
    pub pair_id: String,
    /// 选择的 Opinion(赞成方最高置信度)
    pub chosen: Opinion,
    /// 拒绝的 Opinion(反对方最高置信度)
    pub rejected: Opinion,
    /// 决策上下文(quest_id:decision_hash)
    pub context: String,
    /// 关联的 Quest ID
    pub quest_id: String,
}

/// DPO 训练对生成器 — 从辩论 Opinion 中提取 chosen/rejected 对
///
/// WHY 无状态结构:DPO 对生成是纯函数操作,无需维护状态,
/// `DpoPairGenerator` 仅作为方法载体,线程安全(Send + Sync)。
///
/// # 生成规则
/// 1. 仅当 Consensus::Reached 时生成(无共识无对比价值)
/// 2. `chosen` = 赞成立场(position=1.0)中置信度最高的 Opinion
/// 3. `rejected` = 反对立场(position=0.0)中置信度最高的 Opinion
/// 4. 若无反对意见,返回 None(无对比,不生成)
pub struct DpoPairGenerator;

impl DpoPairGenerator {
    /// 创建新的 DPO 训练对生成器
    pub fn new() -> Self {
        Self
    }

    /// 从辩论 Opinion 与共识结果生成 DPO 训练对
    ///
    /// # 参数
    /// - `quest_id`:关联的 Quest ID
    /// - `opinions`:辩论产生的所有 Opinion
    /// - `consensus`:共识判定结果
    ///
    /// # 返回
    /// - `Some(DpoPair)`:存在赞成/反对对比,生成训练对
    /// - `None`:共识未达成,或无反对意见(无对比价值)
    pub fn generate(
        &self,
        quest_id: &str,
        opinions: &[Opinion],
        consensus: &Consensus,
    ) -> Option<DpoPair> {
        // 仅当共识达成时生成
        let decision_hash = match consensus {
            Consensus::Reached { decision_hash, .. } => decision_hash.as_str(),
            _ => return None,
        };

        // chosen = 赞成立场(position=1.0)中置信度最高的 Opinion
        let chosen = opinions.iter().filter(|o| o.is_approve()).max_by(|a, b| {
            a.confidence
                .partial_cmp(&b.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;

        // rejected = 反对立场(position=0.0)中置信度最高的 Opinion
        let rejected = opinions.iter().filter(|o| o.is_reject()).max_by(|a, b| {
            a.confidence
                .partial_cmp(&b.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;

        // 若无反对意见,返回 None(无对比价值)
        // (rejected 的 ? 已处理 None 情况)

        let pair_id = Uuid::now_v7().to_string();
        let context = format!("{quest_id}:{decision_hash}");

        Some(DpoPair {
            pair_id,
            chosen: chosen.clone(),
            rejected: rejected.clone(),
            context,
            quest_id: quest_id.to_string(),
        })
    }
}

impl Default for DpoPairGenerator {
    fn default() -> Self {
        Self::new()
    }
}
