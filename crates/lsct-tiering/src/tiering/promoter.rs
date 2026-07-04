//! 升温器 — 逐级提升能力存储层级,HashSet 防级联
//!
//! 对应架构层:L3 Storage
//!
//! # 核心职责
//! - 执行升温操作(from → next_warmer(from)),严格逐级,禁止跨级跳跃
//! - HashSet 记录本周期已升温的能力,防止同一 tick 内重复升温(级联)
//! - 每个 tick 周期开始时由 coordinator 调用 reset() 清空集合
//!
//! # 级联防护(WHY)
//! 若不防护,一个 Ice 层能力在单次 tick 中可能被连续 promote 多次直达 Hot,
//! 造成 Hot 层瞬间涌入大量数据,触发 LRU 驱逐风暴。HashSet 确保每个能力
//! 在单个 tick 内最多升温一级,多 tick 周期逐步达到目标层级。

use std::collections::HashSet;

use cmt_tiering::Tier;

use crate::error::LsctError;
use crate::types::{next_warmer, tier_rank, TierAssignment};

/// 升温器 — 逐级提升层级,HashSet 防级联
///
/// # 线程安全
/// 本身非线程安全,由 `LsctCoordinator` 通过 `Mutex<LsctPromoter>` 保护。
/// coordinator 在锁内调用 promote(),锁外不持有引用,避免死锁。
pub struct LsctPromoter {
    /// 本周期已升温的能力 ID 集合
    promoted: HashSet<String>,
}

impl LsctPromoter {
    /// 创建空升温器
    pub fn new() -> Self {
        Self {
            promoted: HashSet::new(),
        }
    }

    /// 执行升温操作
    ///
    /// # 校验规则
    /// 1. `to` 必须是 `from` 的相邻更热层(`next_warmer(from) == Some(to)`)
    /// 2. `capability_id` 不能在本周期已升温(防级联)
    /// 3. `from != to`(由规则 1 隐式保证)
    ///
    /// # 返回
    /// 成功返回 `TierAssignment`(current=from, target=to),
    /// 并将 capability_id 记入 promoted 集合。
    ///
    /// # 错误
    /// - `InvalidTier`:跨级跳跃、方向错误或源目标相同
    /// - `InvalidTier`:能力已在本周期升温(级联防护)
    pub fn promote(
        &mut self,
        capability_id: &str,
        from: Tier,
        to: Tier,
    ) -> Result<TierAssignment, LsctError> {
        // 级联防护:同一 tick 内不允许重复升温
        if self.promoted.contains(capability_id) {
            return Err(LsctError::InvalidTier {
                reason: format!("能力 {capability_id} 已在本周期升温,拒绝级联(防 LRU 驱逐风暴)"),
            });
        }

        // 逐级校验:to 必须是 from 的相邻更热层
        match next_warmer(from) {
            Some(expected) if expected == to => {
                // 校验通过:to == next_warmer(from)
            }
            Some(other) => {
                return Err(LsctError::InvalidTier {
                    reason: format!(
                        "升温路径非法:from={from:?} → to={to:?},\
                         逐级要求 to={other:?}(next_warmer({from:?}))"
                    ),
                });
            }
            None => {
                return Err(LsctError::InvalidTier {
                    reason: format!("已是最热层({from:?}),无法继续升温"),
                });
            }
        }

        // 二次校验:确保 rank 递减(更热 = rank 更小)
        debug_assert!(
            tier_rank(to) < tier_rank(from),
            "升温不变量:tier_rank(to) 必须小于 tier_rank(from)"
        );

        self.promoted.insert(capability_id.to_string());

        Ok(TierAssignment {
            capability_id: capability_id.to_string(),
            current_tier: from,
            target_tier: to,
            reason: format!("promote: {from:?} → {to:?}"),
        })
    }

    /// 检查能力是否已在本周期升温
    pub fn is_promoted(&self, capability_id: &str) -> bool {
        self.promoted.contains(capability_id)
    }

    /// 获取本周期已升温的能力数量
    pub fn len(&self) -> usize {
        self.promoted.len()
    }

    /// 是否没有升温过任何能力
    pub fn is_empty(&self) -> bool {
        self.promoted.is_empty()
    }

    /// 重置已升温集合(每个 tick 周期开始时调用)
    ///
    /// WHY:新 tick 周期开始时,上一个周期的级联防护不再适用,
    /// 清空集合允许能力在新周期内再次升温一级。
    pub fn reset(&mut self) {
        self.promoted.clear();
    }
}

impl Default for LsctPromoter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_promote_warm_to_hot() {
        let mut promoter = LsctPromoter::new();
        let assignment = promoter.promote("cap-1", Tier::Warm, Tier::Hot).unwrap();
        assert_eq!(assignment.capability_id, "cap-1");
        assert_eq!(assignment.current_tier, Tier::Warm);
        assert_eq!(assignment.target_tier, Tier::Hot);
        assert!(promoter.is_promoted("cap-1"));
    }

    #[test]
    fn test_promote_cold_to_warm() {
        let mut promoter = LsctPromoter::new();
        let assignment = promoter.promote("cap-2", Tier::Cold, Tier::Warm).unwrap();
        assert_eq!(assignment.current_tier, Tier::Cold);
        assert_eq!(assignment.target_tier, Tier::Warm);
    }

    #[test]
    fn test_promote_ice_to_cold() {
        let mut promoter = LsctPromoter::new();
        let assignment = promoter.promote("cap-3", Tier::Ice, Tier::Cold).unwrap();
        assert_eq!(assignment.current_tier, Tier::Ice);
        assert_eq!(assignment.target_tier, Tier::Cold);
    }

    #[test]
    fn test_promote_cross_tier_rejected() {
        // 跨级升温:Ice → Hot(跳过 Cold/Warm),必须拒绝
        let mut promoter = LsctPromoter::new();
        let err = promoter.promote("cap-1", Tier::Ice, Tier::Hot).unwrap_err();
        assert!(matches!(err, LsctError::InvalidTier { .. }));
    }

    #[test]
    fn test_promote_wrong_direction_rejected() {
        // 方向错误:Hot → Warm(这是降温,不是升温),必须拒绝
        let mut promoter = LsctPromoter::new();
        let err = promoter
            .promote("cap-1", Tier::Hot, Tier::Warm)
            .unwrap_err();
        assert!(matches!(err, LsctError::InvalidTier { .. }));
    }

    #[test]
    fn test_promote_already_hottest_rejected() {
        // 已是最热层,无法继续升温
        let mut promoter = LsctPromoter::new();
        let err = promoter.promote("cap-1", Tier::Hot, Tier::Hot).unwrap_err();
        assert!(matches!(err, LsctError::InvalidTier { .. }));
    }

    #[test]
    fn test_promote_cascade_prevention() {
        // 级联防护:同一能力在 reset 前不能二次升温
        let mut promoter = LsctPromoter::new();
        promoter.promote("cap-1", Tier::Warm, Tier::Hot).unwrap();

        // 第二次升温同一能力(即使路径不同),必须拒绝
        let err = promoter
            .promote("cap-1", Tier::Warm, Tier::Hot)
            .unwrap_err();
        assert!(matches!(err, LsctError::InvalidTier { .. }));
    }

    #[test]
    fn test_promote_reset_clears_set() {
        let mut promoter = LsctPromoter::new();
        promoter.promote("cap-1", Tier::Warm, Tier::Hot).unwrap();
        assert!(promoter.is_promoted("cap-1"));
        assert_eq!(promoter.len(), 1);

        promoter.reset();
        assert!(!promoter.is_promoted("cap-1"));
        assert!(promoter.is_empty());

        // reset 后可以再次升温
        promoter.promote("cap-1", Tier::Warm, Tier::Hot).unwrap();
        assert!(promoter.is_promoted("cap-1"));
    }

    #[test]
    fn test_promote_different_capabilities_independent() {
        // 不同能力的升温互不影响
        let mut promoter = LsctPromoter::new();
        promoter.promote("cap-1", Tier::Warm, Tier::Hot).unwrap();
        promoter.promote("cap-2", Tier::Ice, Tier::Cold).unwrap();

        assert!(promoter.is_promoted("cap-1"));
        assert!(promoter.is_promoted("cap-2"));
        assert_eq!(promoter.len(), 2);
    }
}
