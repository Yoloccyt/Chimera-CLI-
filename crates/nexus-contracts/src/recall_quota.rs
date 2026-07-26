//! 召回配额策略契约 — S7 接缝（omega-learner r1_recall_quota，P4-W16.2.2）
//!
//! 对应架构层: **L0 Contracts**（nexus-core 之下，跨层共享类型）
//! 对应 ADR: **ADR-043**（R1 影子模式设计，决策 1 新增 SeamId::S7RecallQuota）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.5（离线 RL 两接缝）
//! 对应任务: **P4-W16.2.2**（R1 召回配额 CQL/IQL 算法契约层）
//!
//! # 核心职责
//!
//! 承载 R1 离线 RL 接缝（S7RecallQuota）的召回配额枚举与策略载体类型，
//! 供 L6 omega-learner（r1_recall_quota 模块消费 `RecallQuota`/`RecallQuotaPolicy`）
//! 与 L9 编排器（chimera-cli / quest-engine 查询 `RecallQuotaPolicy::Learned`）共享。
//!
//! # C4 合规（与 S1-S6 一致）
//!
//! - `RecallQuotaPolicy::Static(k)` 为默认 fallback，编排器本地可用，无跨 crate 旗标
//! - `RecallQuotaPolicy::Learned { version, quota }` 由 omega-learner 通过
//!   `CapabilityToken::Provisional → Authorized` 灰度授权后注入（ADR-037 + ADR-043）
//! - 影子模式期间（Provisional），编排器查询 token 未授权 → fallback 到 `Static(K10)`
//!
//! # 五档 k 值设计
//!
//! | 变体 | k 值 | 语义 | 对应记忆策略 |
//! |------|------|------|------------|
//! | K5 | 5 | 极简召回 | MinimalRecall |
//! | K10 | 10 | 标准召回 | StandardTopK |
//! | K20 | 20 | 增强召回 | TimeFocused |
//! | K50 | 50 | 激进召回 | QueryReformulation |
//! | K100 | 100 | 全量召回 | AggressivePruning（避免剪枝过头） |
//!
//! WHY 对数间隔（5→10→20→50→100）：
//! - 线性间隔（如 20/40/60/80/100）在低端过密（5 vs 10 差异显著），高端过疏
//! - 对数间隔使 CQL/IQL 在 k 值空间上的探索-利用平衡更均匀
//! - 与 `MemoryStrategy` 5 臂对齐，便于跨接缝对照分析
//!
//! # R2 冻结声明（ADR-042）
//!
//! 本契约仅承载 R1（召回配额 CQL/IQL）路径类型，**不涉及 R2（GSOE×AutoDPO 约束 RL）**。
//! R2 路径在 FormalVerifier 落地前无条件冻结（ADR-042），本文件无需 R2 冻结声明。

use serde::{Deserialize, Serialize};

// ============================================================
// RecallQuota 枚举
// ============================================================

/// 召回配额档位 — R1 离线 RL 接缝的离散动作空间（5 臂）
///
/// 对应 `omega_learner::r1_recall_quota::RecallQuotaLearner` 的动作集，
/// CQL/IQL 算法从 5 档 k 值中选择最优配额。
///
/// # 设计决策（WHY）
///
/// - **枚举而非结构体**: 5 档 k 值有限且固定，枚举提供编译期穷尽性检查
/// - **`#[repr(u8)]`**: 与 `SeamId` 一致，1 字节内存占用，便于序列化
/// - **对数间隔 k 值**: 见模块级文档"五档 k 值设计"
///
/// # 示例
///
/// ```
/// use nexus_contracts::RecallQuota;
///
/// let q = RecallQuota::K10;
/// assert_eq!(q.k_value(), 10);
/// assert_eq!(q.index(), 1);
/// assert_eq!(RecallQuota::from_index(1), Some(RecallQuota::K10));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum RecallQuota {
    /// K=5：极简召回（MinimalRecall 先验）
    K5 = 0,
    /// K=10：标准召回（StandardTopK 先验，默认 fallback）
    K10 = 1,
    /// K=20：增强召回（TimeFocused 先验）
    K20 = 2,
    /// K=50：激进召回（QueryReformulation 先验）
    K50 = 3,
    /// K=100：全量召回（AggressivePruning 先验，避免剪枝过头）
    K100 = 4,
}

impl RecallQuota {
    /// 返回配额的 k 值（用于实际召回条目数）
    pub const fn k_value(self) -> u32 {
        match self {
            Self::K5 => 5,
            Self::K10 => 10,
            Self::K20 => 20,
            Self::K50 => 50,
            Self::K100 => 100,
        }
    }

    /// 返回配额在动作空间中的索引（0..5，用于 CQL/IQL 的 θ_a 索引）
    pub const fn index(self) -> usize {
        self as usize
    }

    /// 从索引反查配额（用于 CQL/IQL 选择动作后映射回配额）
    ///
    /// # 参数
    /// - `index`: 动作索引，必须在 0..5 范围内
    ///
    /// # 返回
    /// - `Some(RecallQuota)`: 索引有效
    /// - `None`: 索引越界（CQL/IQL 输出异常时 fallback 到 K10）
    pub const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::K5),
            1 => Some(Self::K10),
            2 => Some(Self::K20),
            3 => Some(Self::K50),
            4 => Some(Self::K100),
            _ => None,
        }
    }

    /// 返回配额简称（用于日志/调试）
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::K5 => "k5",
            Self::K10 => "k10",
            Self::K20 => "k20",
            Self::K50 => "k50",
            Self::K100 => "k100",
        }
    }

    /// 返回所有 5 档配额（按 k 值升序，用于遍历初始化）
    pub const fn all() -> [Self; 5] {
        [Self::K5, Self::K10, Self::K20, Self::K50, Self::K100]
    }

    /// 默认 fallback 配额（C4 合规第三层：编排器本地可用）
    ///
    /// WHY K10: 与 `MemoryStrategy::StandardTopK` 对齐，标准 TopK 召回
    /// 是生产环境最常用的默认配额，平衡召回率与延迟。
    pub const DEFAULT_FALLBACK: Self = Self::K10;
}

impl Default for RecallQuota {
    /// 默认配额为 K10（与 `DEFAULT_FALLBACK` 一致）
    fn default() -> Self {
        Self::DEFAULT_FALLBACK
    }
}

impl std::fmt::Display for RecallQuota {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

// ============================================================
// RecallQuotaPolicy 枚举
// ============================================================

/// 召回配额策略 — C4 合规灰度授权载体（与 `MemoryStrategyPolicy` 对称）
///
/// 承载 R1 离线 RL 学习结果，由 omega-learner 注入到编排器
/// （chimera-cli / quest-engine），编排器通过 `CapabilityToken` 查询授权状态。
///
/// # 两态设计（与 `MemoryStrategyPolicy` / `DecayPolicy` 等一致）
///
/// - `Static(k)`: 静态 fallback，编排器本地可用，无需外部授权
/// - `Learned { version, quota }`: 学习结果，需 `CapabilityToken::Authorized` 才生效
///
/// # C4 合规三层 fallback
///
/// 1. **默认值层**: `RecallQuotaPolicy::default()` = `Static(K10)`
/// 2. **异常回退层**: `CapabilityToken::Cooldown` → 编排器 fallback 到 `Static(K10)`
/// 3. **熔断入口层**: `CapabilityToken::Frozen` → `fallback_to_static()` 不受 token 约束
///
/// # 示例
///
/// ```
/// use nexus_contracts::{RecallQuota, RecallQuotaPolicy};
///
/// // 1. 默认 fallback
/// let fallback = RecallQuotaPolicy::default();
/// assert!(matches!(fallback, RecallQuotaPolicy::Static(RecallQuota::K10)));
///
/// // 2. R1 训练结果（影子模式期间未授权，编排器 fallback）
/// let learned = RecallQuotaPolicy::Learned { version: 1, quota: RecallQuota::K20 };
/// assert!(learned.is_learned());
/// assert_eq!(learned.version(), Some(1));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecallQuotaPolicy {
    /// 静态 fallback — 编排器本地可用（C4 合规第一层）
    Static(RecallQuota),

    /// 学习结果 — 需 `CapabilityToken::Authorized` 才生效（C4 合规第二层）
    Learned {
        /// 策略版本号（与 `CapabilityToken::bound_policy_version` 对齐，便于回溯）
        version: u64,
        /// 召回配额
        quota: RecallQuota,
    },
}

impl RecallQuotaPolicy {
    /// 是否为 Learned 策略（编排器查询 token 后决定是否生效）
    pub fn is_learned(&self) -> bool {
        matches!(self, Self::Learned { .. })
    }

    /// 是否为 Static fallback
    pub fn is_static(&self) -> bool {
        matches!(self, Self::Static(_))
    }

    /// 返回策略版本号（Learned 返回 Some，Static 返回 None）
    pub fn version(&self) -> Option<u64> {
        match self {
            Self::Static(_) => None,
            Self::Learned { version, .. } => Some(*version),
        }
    }

    /// 返回当前生效的召回配额（无论 Static 或 Learned）
    ///
    /// WHY 提供: 编排器在 fallback 场景下需要知道实际生效的 k 值
    pub fn quota(&self) -> RecallQuota {
        match self {
            Self::Static(q) | Self::Learned { quota: q, .. } => *q,
        }
    }

    /// 返回 fallback 到 Static(K10) 的策略（C4 合规第三层熔断入口）
    ///
    /// WHY 方法而非关联函数: 与 `MemoryStrategyPolicy::fallback_to_static()` 模式一致
    pub fn fallback_to_static(self) -> Self {
        Self::Static(RecallQuota::DEFAULT_FALLBACK)
    }
}

impl Default for RecallQuotaPolicy {
    /// 默认策略为 Static(K10)（C4 合规第一层默认值）
    fn default() -> Self {
        Self::Static(RecallQuota::DEFAULT_FALLBACK)
    }
}

impl std::fmt::Display for RecallQuotaPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static(q) => write!(f, "Static({})", q),
            Self::Learned { version, quota } => write!(f, "Learned(v{}, {})", version, quota),
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ----- RecallQuota 枚举测试 -----

    #[test]
    fn test_recall_quota_k_values() {
        assert_eq!(RecallQuota::K5.k_value(), 5);
        assert_eq!(RecallQuota::K10.k_value(), 10);
        assert_eq!(RecallQuota::K20.k_value(), 20);
        assert_eq!(RecallQuota::K50.k_value(), 50);
        assert_eq!(RecallQuota::K100.k_value(), 100);
    }

    #[test]
    fn test_recall_quota_index_round_trip() {
        // 索引往返映射必须一致
        for q in RecallQuota::all() {
            let idx = q.index();
            assert_eq!(RecallQuota::from_index(idx), Some(q));
        }
    }

    #[test]
    fn test_recall_quota_index_range() {
        assert_eq!(RecallQuota::from_index(0), Some(RecallQuota::K5));
        assert_eq!(RecallQuota::from_index(4), Some(RecallQuota::K100));
        assert_eq!(RecallQuota::from_index(5), None);
        assert_eq!(RecallQuota::from_index(usize::MAX), None);
    }

    #[test]
    fn test_recall_quota_short_name() {
        assert_eq!(RecallQuota::K5.short_name(), "k5");
        assert_eq!(RecallQuota::K10.short_name(), "k10");
        assert_eq!(RecallQuota::K20.short_name(), "k20");
        assert_eq!(RecallQuota::K50.short_name(), "k50");
        assert_eq!(RecallQuota::K100.short_name(), "k100");
    }

    #[test]
    fn test_recall_quota_all_returns_five() {
        let all = RecallQuota::all();
        assert_eq!(all.len(), 5);
        assert_eq!(all[0], RecallQuota::K5);
        assert_eq!(all[4], RecallQuota::K100);
    }

    #[test]
    fn test_recall_quota_all_unique() {
        let all = RecallQuota::all();
        let mut seen = std::collections::HashSet::new();
        for q in all.iter() {
            assert!(seen.insert(q.k_value()), "duplicate k value");
        }
    }

    #[test]
    fn test_recall_quota_default_is_k10() {
        assert_eq!(RecallQuota::default(), RecallQuota::K10);
        assert_eq!(RecallQuota::DEFAULT_FALLBACK, RecallQuota::K10);
    }

    #[test]
    fn test_recall_quota_display() {
        assert_eq!(format!("{}", RecallQuota::K5), "k5");
        assert_eq!(format!("{}", RecallQuota::K100), "k100");
    }

    #[test]
    fn test_recall_quota_repr_u8() {
        // 验证 #[repr(u8)]: 内存中占 1 字节
        assert_eq!(std::mem::size_of::<RecallQuota>(), 1);
    }

    #[test]
    fn test_recall_quota_serde_json() {
        let q = RecallQuota::K20;
        let json = serde_json::to_string(&q).unwrap();
        let deserialized: RecallQuota = serde_json::from_str(&json).unwrap();
        assert_eq!(q, deserialized);
    }

    // ----- RecallQuotaPolicy 枚举测试 -----

    #[test]
    fn test_policy_default_is_static_k10() {
        let p = RecallQuotaPolicy::default();
        assert!(p.is_static());
        assert!(!p.is_learned());
        assert_eq!(p.quota(), RecallQuota::K10);
        assert_eq!(p.version(), None);
    }

    #[test]
    fn test_policy_learned_fields() {
        let p = RecallQuotaPolicy::Learned {
            version: 42,
            quota: RecallQuota::K50,
        };
        assert!(p.is_learned());
        assert!(!p.is_static());
        assert_eq!(p.version(), Some(42));
        assert_eq!(p.quota(), RecallQuota::K50);
    }

    #[test]
    fn test_policy_fallback_to_static() {
        let learned = RecallQuotaPolicy::Learned {
            version: 5,
            quota: RecallQuota::K100,
        };
        let fallback = learned.fallback_to_static();
        assert!(fallback.is_static());
        assert_eq!(fallback.quota(), RecallQuota::K10);
        assert_eq!(fallback.version(), None);
    }

    #[test]
    fn test_policy_display() {
        let s = RecallQuotaPolicy::Static(RecallQuota::K20);
        assert_eq!(format!("{}", s), "Static(k20)");

        let l = RecallQuotaPolicy::Learned {
            version: 3,
            quota: RecallQuota::K50,
        };
        assert_eq!(format!("{}", l), "Learned(v3, k50)");
    }

    #[test]
    fn test_policy_serde_round_trip() {
        let cases = vec![
            RecallQuotaPolicy::Static(RecallQuota::K5),
            RecallQuotaPolicy::Learned {
                version: 99,
                quota: RecallQuota::K100,
            },
        ];
        for p in cases {
            let json = serde_json::to_string(&p).unwrap();
            let de: RecallQuotaPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(p, de);
        }
    }
}
