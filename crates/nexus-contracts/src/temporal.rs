//! 时间元数据 — P3 时间感知记忆扩展
//!
//! 对应架构层: L0 Contracts（新建）
//! 对应 ADR: ADR-033
//! 对应阶段: P3-W9~12 HCW-Sparse v2.0 + Temporal 全链
//!
//! # 设计决策(WHY)
//!
//! - **类型定义在 L0**: `TemporalMeta` / `TransitionType` 需被 L2 `mlc-engine`（四级记忆条目扩展）
//!   与 L2 `hcw-window`（分层上下文窗口）共同消费，定义在 L0 避免跨层依赖
//!
//! - **三态枚举**: `TransitionType` 区分 Current（当前有效）/ Historical（历史归档）/ Transition（迁移中），
//!   对应三重悖论中"记忆悖论"的解决方案——记忆策略随任务阶段自适应
//!
//! - **时间区间 + 置信度**: `valid_from` / `valid_until` 定义记忆有效期，
//!   `confidence` 标记记忆可信度，HCW-Sparse v2.0 据此做时间感知召回
//!
//! # 完整实现时机
//!
//! 当前文件仅定义**类型骨架**（P2-W5.1），完整时间感知逻辑在 P3-W11 落地:
//! - P3-W11: mlc-engine 四级条目扩展 TemporalMeta 全链 + 矛盾检测挂点(D12)

use serde::{Deserialize, Serialize};

/// 时间状态类型 — 记忆条目的三态分类
///
/// WHY: 三重悖论"记忆悖论"的核心问题——静态稀疏掩码无法区分新旧事实的时间有效性。
/// `TransitionType` 为记忆条目打上时间状态标签，使 HCW-Sparse v2.0 能在任务阶段切换时
/// 区分"当前有效"、"历史归档"、"迁移中"三类记忆，避免幽灵记忆
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransitionType {
    /// 当前有效 — 记忆在当前任务阶段有效，可被召回
    ///
    /// WHY: `valid_from` ≤ now < `valid_until`（若 `valid_until` 为 None 则无限期）
    Current,
    /// 历史归档 — 记忆已过期，降级到冷存储，仅作为历史参考
    ///
    /// WHY: now ≥ `valid_until`（若有），记忆不再影响当前决策，但保留谱系完整性
    Historical,
    /// 迁移中 — 记忆正在从一个阶段迁移到另一个阶段（如 Hot→Warm 降级）
    ///
    /// WHY: INV-8 归档单调性约束——归档沿 Hot→Warm→Cold→Ice 单向降级，
    /// `Transition` 状态标记迁移过程，迁移完成后转为 `Historical` 或 `Current`
    Transition,
}

impl TransitionType {
    /// 返回状态名称(用于事件 payload 与日志)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Current => "Current",
            Self::Historical => "Historical",
            Self::Transition => "Transition",
        }
    }

    /// 判断是否为当前有效状态
    ///
    /// WHY: HCW-Sparse v2.0 召回时优先选择 `Current` 状态的记忆
    pub fn is_current(&self) -> bool {
        matches!(self, Self::Current)
    }

    /// 判断是否为归档状态(历史或迁移中)
    ///
    /// WHY: INV-8 归档单调性检查——归档状态的记忆不可逆向升级为 Current
    pub fn is_archived(&self) -> bool {
        matches!(self, Self::Historical | Self::Transition)
    }
}

/// 时间元数据 — 记忆条目的时间有效性信息
///
/// WHY: 三重悖论"记忆悖论"的解决方案——为每条记忆附加时间区间与置信度，
/// 使 HCW-Sparse v2.0 能做时间感知召回（优先召回当前有效、高置信度的记忆）
///
/// # 字段说明
///
/// - `valid_from`: 记忆生效时间(UTC 时间戳，秒)
/// - `valid_until`: 记忆失效时间(UTC 时间戳，秒；None 表示永久有效)
/// - `transition_type`: 时间状态(Current/Historical/Transition)
/// - `confidence`: 置信度 [0.0, 1.0]，1.0 表示完全可信
///
/// # INV-8 归档单调性
///
/// `transition_type` 的状态转换遵循单向降级:
/// - `Current` → `Historical` (正常过期)
/// - `Current` → `Transition` → `Historical` (主动归档)
/// - `Historical` → `Current` 禁止(逆向升级违反 INV-8)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemporalMeta {
    /// 记忆生效时间(UTC 时间戳，秒)
    pub valid_from: i64,
    /// 记忆失效时间(UTC 时间戳，秒；None 表示永久有效)
    ///
    /// WHY: `None` 用于永久记忆(如架构红线、核心术语)，
    /// 这些记忆不应因时间过期而被降级
    pub valid_until: Option<i64>,
    /// 时间状态(Current/Historical/Transition)
    pub transition_type: TransitionType,
    /// 置信度 [0.0, 1.0]
    ///
    /// WHY: 置信度来源: GSOE 在线进化反馈 / Auto-DPO 偏好对验证 / 人工标注。
    /// HCW-Sparse v2.0 召回时按置信度排序，低置信度记忆优先被稀疏化
    pub confidence: f32,
}

impl TemporalMeta {
    /// 创建新的时间元数据(默认 Current 状态)
    ///
    /// # 参数
    ///
    /// - `valid_from`: 生效时间(UTC 秒)
    /// - `confidence`: 置信度 [0.0, 1.0]
    pub fn new(valid_from: i64, confidence: f32) -> Self {
        Self {
            valid_from,
            valid_until: None,
            transition_type: TransitionType::Current,
            confidence,
        }
    }

    /// 创建带有效期的时间元数据
    ///
    /// # 参数
    ///
    /// - `valid_from`: 生效时间(UTC 秒)
    /// - `valid_until`: 失效时间(UTC 秒)
    /// - `confidence`: 置信度 [0.0, 1.0]
    pub fn with_expiry(valid_from: i64, valid_until: i64, confidence: f32) -> Self {
        Self {
            valid_from,
            valid_until: Some(valid_until),
            transition_type: TransitionType::Current,
            confidence,
        }
    }

    /// 判断在指定时间点是否有效
    ///
    /// WHY: HCW-Sparse v2.0 召回时检查记忆的时间有效性，
    /// 过期记忆(valid_until < now)应已转为 Historical 状态
    pub fn is_valid_at(&self, now: i64) -> bool {
        if now < self.valid_from {
            return false;
        }
        match self.valid_until {
            Some(until) => now < until,
            None => true, // 永久有效
        }
    }

    /// 判断是否为永久有效(无 valid_until)
    ///
    /// WHY: 永久有效的记忆(如架构红线)不应因时间过期被稀疏化
    pub fn is_permanent(&self) -> bool {
        self.valid_until.is_none()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transition_type_as_str() {
        assert_eq!(TransitionType::Current.as_str(), "Current");
        assert_eq!(TransitionType::Historical.as_str(), "Historical");
        assert_eq!(TransitionType::Transition.as_str(), "Transition");
    }

    #[test]
    fn test_transition_type_flags() {
        assert!(TransitionType::Current.is_current());
        assert!(!TransitionType::Current.is_archived());

        assert!(!TransitionType::Historical.is_current());
        assert!(TransitionType::Historical.is_archived());

        assert!(!TransitionType::Transition.is_current());
        assert!(TransitionType::Transition.is_archived());
    }

    #[test]
    fn test_temporal_meta_new() {
        let meta = TemporalMeta::new(1000, 0.9);
        assert_eq!(meta.valid_from, 1000);
        assert_eq!(meta.valid_until, None);
        assert_eq!(meta.transition_type, TransitionType::Current);
        assert!((meta.confidence - 0.9).abs() < 1e-6);
        assert!(meta.is_permanent());
    }

    #[test]
    fn test_temporal_meta_with_expiry() {
        let meta = TemporalMeta::with_expiry(1000, 2000, 0.8);
        assert_eq!(meta.valid_from, 1000);
        assert_eq!(meta.valid_until, Some(2000));
        assert!(!meta.is_permanent());
    }

    #[test]
    fn test_is_valid_at() {
        let meta = TemporalMeta::with_expiry(1000, 2000, 0.8);

        // 生效前无效
        assert!(!meta.is_valid_at(999));

        // 有效期内有效
        assert!(meta.is_valid_at(1000));
        assert!(meta.is_valid_at(1500));
        assert!(meta.is_valid_at(1999));

        // 失效时间点无效(边界: valid_until 为 exclusive)
        assert!(!meta.is_valid_at(2000));
    }

    #[test]
    fn test_permanent_meta_always_valid() {
        let meta = TemporalMeta::new(1000, 1.0);
        assert!(meta.is_valid_at(1000));
        assert!(meta.is_valid_at(9999999999)); // 远未来仍有效
    }

    #[test]
    fn test_serde_roundtrip() {
        let meta = TemporalMeta::with_expiry(1000, 2000, 0.85);
        let json = serde_json::to_string(&meta).expect("序列化失败");
        let restored: TemporalMeta = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(meta, restored);
    }

    #[test]
    fn test_transition_type_serde_roundtrip() {
        for tt in [
            TransitionType::Current,
            TransitionType::Historical,
            TransitionType::Transition,
        ] {
            let json = serde_json::to_string(&tt).expect("序列化失败");
            let restored: TransitionType = serde_json::from_str(&json).expect("反序列化失败");
            assert_eq!(tt, restored);
        }
    }
}
