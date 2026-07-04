//! 降温器 — 逐级降低能力存储层级,HashSet 防级联
//!
//! 对应架构层:L3 Storage
//!
//! # 核心职责
//! - 执行降温操作(from → next_colder(from)),严格逐级,禁止跨级跳跃
//! - HashSet 记录本周期已降温的能力,防止同一 tick 内重复降温(级联)
//! - 每个 tick 周期开始时由 coordinator 调用 reset() 清空集合
//!
//! # 级联防护(WHY)
//! 与升温器对称:若不防护,一个 Hot 层能力在单次 tick 中可能被连续 demote
//! 多次直达 Ice,造成 Ice 层瞬间涌入大量归档数据,I/O 压力骤增。HashSet
//! 确保每个能力在单个 tick 内最多降温一级。

use std::collections::HashSet;

use cmt_tiering::Tier;

use crate::error::LsctError;
use crate::types::{next_colder, tier_rank, TierAssignment};

/// 降温器 — 逐级降低层级,HashSet 防级联
///
/// # 线程安全
/// 本身非线程安全,由 `LsctCoordinator` 通过 `Mutex<LsctDemoter>` 保护。
pub struct LsctDemoter {
    /// 本周期已降温的能力 ID 集合
    demoted: HashSet<String>,
}

impl LsctDemoter {
    /// 创建空降温器
    pub fn new() -> Self {
        Self {
            demoted: HashSet::new(),
        }
    }

    /// 执行降温操作
    ///
    /// # 校验规则
    /// 1. `to` 必须是 `from` 的相邻更冷层(`next_colder(from) == Some(to)`)
    /// 2. `capability_id` 不能在本周期已降温(防级联)
    /// 3. `from != to`(由规则 1 隐式保证)
    ///
    /// # 返回
    /// 成功返回 `TierAssignment`(current=from, target=to),
    /// 并将 capability_id 记入 demoted 集合。
    ///
    /// # 错误
    /// - `InvalidTier`:跨级跳跃、方向错误或源目标相同
    /// - `InvalidTier`:能力已在本周期降温(级联防护)
    pub fn demote(
        &mut self,
        capability_id: &str,
        from: Tier,
        to: Tier,
    ) -> Result<TierAssignment, LsctError> {
        // 级联防护:同一 tick 内不允许重复降温
        if self.demoted.contains(capability_id) {
            return Err(LsctError::InvalidTier {
                reason: format!("能力 {capability_id} 已在本周期降温,拒绝级联(防 I/O 压力骤增)"),
            });
        }

        // 逐级校验:to 必须是 from 的相邻更冷层
        match next_colder(from) {
            Some(expected) if expected == to => {
                // 校验通过:to == next_colder(from)
            }
            Some(other) => {
                return Err(LsctError::InvalidTier {
                    reason: format!(
                        "降温路径非法:from={from:?} → to={to:?},\
                         逐级要求 to={other:?}(next_colder({from:?}))"
                    ),
                });
            }
            None => {
                return Err(LsctError::InvalidTier {
                    reason: format!("已是最冷层({from:?}),无法继续降温"),
                });
            }
        }

        // 二次校验:确保 rank 递增(更冷 = rank 更大)
        debug_assert!(
            tier_rank(to) > tier_rank(from),
            "降温不变量:tier_rank(to) 必须大于 tier_rank(from)"
        );

        self.demoted.insert(capability_id.to_string());

        Ok(TierAssignment {
            capability_id: capability_id.to_string(),
            current_tier: from,
            target_tier: to,
            reason: format!("demote: {from:?} → {to:?}"),
        })
    }

    /// 检查能力是否已在本周期降温
    pub fn is_demoted(&self, capability_id: &str) -> bool {
        self.demoted.contains(capability_id)
    }

    /// 获取本周期已降温的能力数量
    pub fn len(&self) -> usize {
        self.demoted.len()
    }

    /// 是否没有降温过任何能力
    pub fn is_empty(&self) -> bool {
        self.demoted.is_empty()
    }

    /// 重置已降温集合(每个 tick 周期开始时调用)
    pub fn reset(&mut self) {
        self.demoted.clear();
    }
}

impl Default for LsctDemoter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_demote_hot_to_warm() {
        let mut demoter = LsctDemoter::new();
        let assignment = demoter.demote("cap-1", Tier::Hot, Tier::Warm).unwrap();
        assert_eq!(assignment.capability_id, "cap-1");
        assert_eq!(assignment.current_tier, Tier::Hot);
        assert_eq!(assignment.target_tier, Tier::Warm);
        assert!(demoter.is_demoted("cap-1"));
    }

    #[test]
    fn test_demote_warm_to_cold() {
        let mut demoter = LsctDemoter::new();
        let assignment = demoter.demote("cap-2", Tier::Warm, Tier::Cold).unwrap();
        assert_eq!(assignment.current_tier, Tier::Warm);
        assert_eq!(assignment.target_tier, Tier::Cold);
    }

    #[test]
    fn test_demote_cold_to_ice() {
        let mut demoter = LsctDemoter::new();
        let assignment = demoter.demote("cap-3", Tier::Cold, Tier::Ice).unwrap();
        assert_eq!(assignment.current_tier, Tier::Cold);
        assert_eq!(assignment.target_tier, Tier::Ice);
    }

    #[test]
    fn test_demote_cross_tier_rejected() {
        // 跨级降温:Hot → Ice(跳过 Warm/Cold),必须拒绝
        let mut demoter = LsctDemoter::new();
        let err = demoter.demote("cap-1", Tier::Hot, Tier::Ice).unwrap_err();
        assert!(matches!(err, LsctError::InvalidTier { .. }));
    }

    #[test]
    fn test_demote_wrong_direction_rejected() {
        // 方向错误:Warm → Hot(这是升温,不是降温),必须拒绝
        let mut demoter = LsctDemoter::new();
        let err = demoter.demote("cap-1", Tier::Warm, Tier::Hot).unwrap_err();
        assert!(matches!(err, LsctError::InvalidTier { .. }));
    }

    #[test]
    fn test_demote_already_coldest_rejected() {
        // 已是最冷层,无法继续降温
        let mut demoter = LsctDemoter::new();
        let err = demoter.demote("cap-1", Tier::Ice, Tier::Ice).unwrap_err();
        assert!(matches!(err, LsctError::InvalidTier { .. }));
    }

    #[test]
    fn test_demote_cascade_prevention() {
        let mut demoter = LsctDemoter::new();
        demoter.demote("cap-1", Tier::Hot, Tier::Warm).unwrap();

        let err = demoter.demote("cap-1", Tier::Hot, Tier::Warm).unwrap_err();
        assert!(matches!(err, LsctError::InvalidTier { .. }));
    }

    #[test]
    fn test_demote_reset_clears_set() {
        let mut demoter = LsctDemoter::new();
        demoter.demote("cap-1", Tier::Hot, Tier::Warm).unwrap();
        assert!(demoter.is_demoted("cap-1"));
        assert_eq!(demoter.len(), 1);

        demoter.reset();
        assert!(!demoter.is_demoted("cap-1"));
        assert!(demoter.is_empty());

        demoter.demote("cap-1", Tier::Hot, Tier::Warm).unwrap();
        assert!(demoter.is_demoted("cap-1"));
    }
}
