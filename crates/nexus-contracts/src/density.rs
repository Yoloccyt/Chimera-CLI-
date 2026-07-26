//! 密度档位策略 — S1 接缝（DDR/HCW 密度档位）契约
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应 ADR: **ADR-033**（L0 nexus-contracts 契约层建立）
//! 对应任务: **P4-W13.2**（S1 接缝 — DDR/HCW 密度档位）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 六接缝表 S1
//!
//! # 核心职责
//!
//! 承载 HCW 窗口选择的密度档位参数 ρ∈{0.5, 2, 5, 10}，将其从
//! `hcw-window` 硬编码常量（8× 稀疏化）升级为可注入策略，
//! 为 P4 `omega-learner` Bandit 异步下发学习值奠基。
//!
//! # ρ 语义（决策记录）
//!
//! ρ 是**密度系数**，决定实际加载量占标称容量的比例：
//!
//! ```text
//! actual_load = nominal_capacity × (ρ / 10.0)
//! ```
//!
//! | ρ 值 | 加载比例 | 语义 | 延迟 | 召回率 |
//! |------|---------|------|------|--------|
//! | 0.5  | 5%      | 极致稀疏化 | 最低 | 最低 |
//! | 2    | 20%     | 强稀疏化 | 低 | 低 |
//! | 5    | 50%     | 标准稀疏化 | 中 | 中 |
//! | 10   | 100%    | 无稀疏化 | 最高 | 最高 |
//!
//! ## 与"密度降档"语义对齐
//!
//! spec.md:282/518 提到"连续两版不达标触发密度降档复盘"——
//! 密度降档 = 从高 ρ 降到低 ρ = 降低加载量 = 降低延迟但可能降低召回率。
//! Bandit 奖励 = 成功率 − 延迟惩罚，延迟惩罚会抑制 bandit 总选最大密度（ρ=10）。
//!
//! # C4 合规（能力场灰度，非运行时旗）
//!
//! `DensityPolicy::default()` 返回 `Static(DensityTier::Rho10)`（无稀疏化，向后兼容），
//! `Rho10` 是 `const` 常量编译进二进制。`omega-learner` panic/超时时，
//! 调用方本地 fallback 到 `DensityPolicy::Static`，**无跨 crate 旗标传播**（spec.md:334）。
//!
//! # 设计决策（WHY）
//!
//! - **枚举而非 trait**: 策略变体有限（Static/Learned），枚举比 trait object 更轻量，
//!   避免 `Box<dyn>` 动态分发开销（§4.1 约定：避免 `Box<dyn Trait>`）
//! - **Copy 语义**: `DensityTier` 为 Copy，枚举整体 Copy，注入时零成本
//! - **版本号 `Option<u64>`**: Learned 携带版本号用于 A/B 测试与回滚，
//!   Static 无版本号（`version()` 返回 None）
//! - **与 SelectorPolicy 对称设计**: 复用相同模式降低学习成本，便于 P4-W14.5 能力场统一治理
//!
//! # 示例
//!
//! ## 静态策略（默认 fallback，无稀疏化）
//!
//! ```
//! use nexus_contracts::{DensityPolicy, DensityTier};
//!
//! let policy = DensityPolicy::default();
//! assert!(policy.is_static());
//! assert_eq!(policy.tier(), DensityTier::Rho10);
//! assert_eq!(policy.version(), None);
//! ```
//!
//! ## 学习策略（omega-learner 异步下发）
//!
//! ```
//! use nexus_contracts::{DensityPolicy, DensityTier};
//!
//! // 学习到 ρ=2（强稀疏化，20% 加载）
//! let learned = DensityPolicy::learned(42, DensityTier::Rho2);
//! assert!(learned.is_learned());
//! assert_eq!(learned.version(), Some(42));
//! assert_eq!(learned.tier(), DensityTier::Rho2);
//! assert!((learned.load_ratio() - 0.2).abs() < 1e-6);
//! ```
//!
//! ## learner panic 时本地 fallback
//!
//! ```
//! use nexus_contracts::{DensityPolicy, DensityTier};
//!
//! let learned = DensityPolicy::learned(1, DensityTier::Rho05);
//! let fallback = DensityPolicy::fallback();
//! assert!(fallback.is_static());
//! assert_eq!(fallback.tier(), DensityTier::Rho10);
//! ```

use serde::{Deserialize, Serialize};

/// 密度档位 — ρ∈{0.5, 2, 5, 10} 四档
///
/// 决定 HCW 窗口实际加载量占标称容量的比例：
/// `actual_load = nominal_capacity × (ρ / 10.0)`
///
/// # 设计决策（WHY）
/// - **枚举而非 f32**: 4 个固定档位避免 bandit 探索到无效值（如 ρ=0 或 ρ=∞）
/// - **Copy + Clone**: 枚举为单元变体，Copy 语义零成本
/// - **`Rho10` 默认**: 对应无稀疏化（100% 加载），向后兼容既有 L0/L1/L2 行为
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum DensityTier {
    /// ρ=0.5 — 极致稀疏化（5% 加载，最低延迟，最低召回率）
    ///
    /// WHY 极致稀疏化: 适用超大规模任务（1M+ token）或内存压力极高场景，
    /// 主动牺牲召回率换取延迟与内存占用。
    Rho05 = 1,

    /// ρ=2 — 强稀疏化（20% 加载，低延迟，低召回率）
    ///
    /// WHY 强稀疏化: 适用于内存压力较高、任务复杂度中等的场景，
    /// 平衡延迟与召回率。
    Rho2 = 2,

    /// ρ=5 — 标准稀疏化（50% 加载，中等延迟，中等召回率）
    ///
    /// WHY 标准稀疏化: 适用于常规复杂任务，是大多数场景的合理默认。
    Rho5 = 3,

    /// ρ=10 — 无稀疏化（100% 加载，最高延迟，最高召回率）
    ///
    /// WHY 无稀疏化: 向后兼容默认行为，适用于 L0/L1/L2 层级或低复杂度任务。
    /// 也是 `DensityPolicy::default()` 的 fallback 值。
    Rho10 = 4,
}

impl DensityTier {
    /// 所有密度档位（按 ρ 升序，便于遍历初始化 LinUCB 臂集）
    pub const ALL: [Self; 4] = [Self::Rho05, Self::Rho2, Self::Rho5, Self::Rho10];

    /// 返回 ρ 数值（f64，便于数学计算）
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::DensityTier;
    ///
    /// assert!((DensityTier::Rho05.rho() - 0.5).abs() < 1e-6);
    /// assert!((DensityTier::Rho10.rho() - 10.0).abs() < 1e-6);
    /// ```
    pub const fn rho(self) -> f64 {
        match self {
            Self::Rho05 => 0.5,
            Self::Rho2 => 2.0,
            Self::Rho5 => 5.0,
            Self::Rho10 => 10.0,
        }
    }

    /// 返回加载比例（actual_load / nominal_capacity）
    ///
    /// 公式: `load_ratio = ρ / 10.0`，范围 (0.05, 1.0]
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::DensityTier;
    ///
    /// assert!((DensityTier::Rho05.load_ratio() - 0.05).abs() < 1e-6);
    /// assert!((DensityTier::Rho2.load_ratio() - 0.2).abs() < 1e-6);
    /// assert!((DensityTier::Rho5.load_ratio() - 0.5).abs() < 1e-6);
    /// assert!((DensityTier::Rho10.load_ratio() - 1.0).abs() < 1e-6);
    /// ```
    pub const fn load_ratio(self) -> f64 {
        // ρ / 10.0，但 const fn 不支持浮点除法直接返回 f64，
        // 用 match 显式返回避免编译期浮点运算限制
        match self {
            Self::Rho05 => 0.05,
            Self::Rho2 => 0.2,
            Self::Rho5 => 0.5,
            Self::Rho10 => 1.0,
        }
    }

    /// 返回稀疏化倍率（nominal_capacity / actual_load）
    ///
    /// 公式: `sparse_ratio = 10.0 / ρ`，范围 [1.0, 20.0]
    ///
    /// WHY 提供: 与 spec.md §4.2 "8× 稀疏化"对齐，
    /// 便于与既有 HCW L3 稀疏化语义互操作。
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::DensityTier;
    ///
    /// assert!((DensityTier::Rho05.sparse_ratio() - 20.0).abs() < 1e-6);
    /// assert!((DensityTier::Rho10.sparse_ratio() - 1.0).abs() < 1e-6);
    /// ```
    pub const fn sparse_ratio(self) -> f64 {
        match self {
            Self::Rho05 => 20.0,
            Self::Rho2 => 5.0,
            Self::Rho5 => 2.0,
            Self::Rho10 => 1.0,
        }
    }

    /// 返回档位简称（用于日志/调试）
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::Rho05 => "ρ=0.5",
            Self::Rho2 => "ρ=2",
            Self::Rho5 => "ρ=5",
            Self::Rho10 => "ρ=10",
        }
    }

    /// 返回档位全称（用于文档/UI）
    pub const fn full_name(self) -> &'static str {
        match self {
            Self::Rho05 => "极致稀疏化 (5% 加载)",
            Self::Rho2 => "强稀疏化 (20% 加载)",
            Self::Rho5 => "标准稀疏化 (50% 加载)",
            Self::Rho10 => "无稀疏化 (100% 加载)",
        }
    }

    /// 根据 ρ 数值查找最近的档位（用于 learner 连续值→离散档位映射）
    ///
    /// WHY 提供: LinUCB 选择臂后返回 ArmIndex，需要映射回 DensityTier。
    /// 本方法提供从 ρ 数值到档位的反向查找（取最近邻）。
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::DensityTier;
    ///
    /// assert_eq!(DensityTier::from_rho(0.6), DensityTier::Rho05);
    /// assert_eq!(DensityTier::from_rho(3.0), DensityTier::Rho2);
    /// assert_eq!(DensityTier::from_rho(7.0), DensityTier::Rho5);
    /// assert_eq!(DensityTier::from_rho(15.0), DensityTier::Rho10);
    /// ```
    pub fn from_rho(rho: f64) -> Self {
        // 取与 ρ 数值最接近的档位（绝对值最小）
        Self::ALL
            .iter()
            .copied()
            .min_by(|a, b| {
                let dist_a = (a.rho() - rho).abs();
                let dist_b = (b.rho() - rho).abs();
                dist_a
                    .partial_cmp(&dist_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(Self::Rho10) // 兜底:空档位时返回 Rho10（无稀疏化）
    }

    /// 计算实际加载容量
    ///
    /// 公式: `actual_load = nominal_capacity × load_ratio`
    ///
    /// # 边界处理
    /// - `nominal_capacity = 0`: 返回 0（无意义但避免 NaN）
    /// - 结果向下取整（usize 加载量必须整数）
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::DensityTier;
    ///
    /// assert_eq!(DensityTier::Rho2.actual_load(1000), 200);
    /// assert_eq!(DensityTier::Rho10.actual_load(1000), 1000);
    /// assert_eq!(DensityTier::Rho05.actual_load(1000), 50);
    /// ```
    pub fn actual_load(self, nominal_capacity: usize) -> usize {
        if nominal_capacity == 0 {
            return 0;
        }
        let ratio = self.load_ratio();
        let actual = (nominal_capacity as f64) * ratio;
        // 向下取整，避免超出标称容量
        actual as usize
    }
}

impl Default for DensityTier {
    /// 默认档位 = `Rho10`（无稀疏化，向后兼容）
    ///
    /// WHY(C4 合规): 默认值 = 当前 HCW L0/L1/L2 行为（无稀疏化），
    /// 调用方未注入策略时行为零变化。
    fn default() -> Self {
        Self::Rho10
    }
}

impl std::fmt::Display for DensityTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

/// 密度策略 — 静态常量或学习版本（S1 接缝）
///
/// 承载 HCW 窗口密度档位，将 ρ 从 `hcw-window` 硬编码常量
/// 升级为可注入策略，为 `omega-learner` Bandit 异步下发学习值奠基。
///
/// # 变体
/// - [`Static`](DensityPolicy::Static): 编译进二进制的常量（fallback，C4 合规）
/// - [`Learned`](DensityPolicy::Learned): `omega-learner` 异步下发的版本化档位
///
/// # C4 合规（能力场灰度，非运行时旗）
///
/// `default()` 返回 `Static(DensityTier::Rho10)`，`Rho10` 是 `const` 常量。
/// `omega-learner` panic/超时时，调用方本地 fallback 到 `Static`，无跨 crate 旗标传播。
///
/// # 设计决策（WHY）
/// - **枚举而非 trait**: 策略变体有限（Static/Learned），枚举比 trait object 更轻量
/// - **Copy 语义**: `DensityTier` 为 Copy，枚举整体 Copy，注入时零成本
/// - **版本号 `Option<u64>`**: Learned 携带版本号用于 A/B 测试与回滚
///
/// # 示例
///
/// ## 静态策略（默认 fallback）
/// ```
/// use nexus_contracts::DensityPolicy;
///
/// let policy = DensityPolicy::default();
/// assert!(policy.is_static());
/// assert_eq!(policy.version(), None);
/// ```
///
/// ## 学习策略（omega-learner 异步下发）
/// ```
/// use nexus_contracts::{DensityPolicy, DensityTier};
///
/// let policy = DensityPolicy::learned(42, DensityTier::Rho2);
/// assert!(policy.is_learned());
/// assert_eq!(policy.version(), Some(42));
/// ```
///
/// ## learner panic 时本地 fallback
/// ```
/// use nexus_contracts::DensityPolicy;
///
/// let fallback = DensityPolicy::fallback();
/// assert!(fallback.is_static());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DensityPolicy {
    /// 静态策略 — 编译进二进制的常量（fallback，C4 合规）
    ///
    /// 承载 `DensityTier` 常量，`default()` 返回 `Static(DensityTier::Rho10)`。
    /// `omega-learner` panic/超时时调用方本地 fallback 到此变体。
    Static(DensityTier),

    /// 学习策略 — `omega-learner` 异步下发的版本化档位
    ///
    /// `version` 单调递增，用于 A/B 测试与回滚（P4 `SpecRegistry` 版本化）。
    /// `tier` 承载学习到的密度档位，由 `omega-learner` Bandit 算法计算。
    Learned {
        /// 学习版本号（单调递增，用于 A/B 测试与回滚）
        version: u64,
        /// 学习到的密度档位
        tier: DensityTier,
    },
}

impl DensityPolicy {
    /// 创建静态策略（便捷构造函数）
    pub const fn static_policy(tier: DensityTier) -> Self {
        Self::Static(tier)
    }

    /// 创建学习策略（便捷构造函数）
    pub const fn learned(version: u64, tier: DensityTier) -> Self {
        Self::Learned { version, tier }
    }

    /// 返回 fallback 策略 — `Static(DensityTier::Rho10)`
    ///
    /// WHY(C4 合规): `omega-learner` panic/超时时调用方本地 fallback 到此值。
    /// `Rho10` 是 `const` 常量编译进二进制，非运行时 feature flag。
    pub const fn fallback() -> Self {
        Self::Static(DensityTier::Rho10)
    }

    /// 返回当前策略的密度档位（无论 Static 还是 Learned）
    ///
    /// 统一访问方法，调用方无需 match 策略变体即可获取档位。
    pub const fn tier(&self) -> DensityTier {
        match self {
            Self::Static(tier) => *tier,
            Self::Learned { tier, .. } => *tier,
        }
    }

    /// 返回加载比例（actual_load / nominal_capacity）
    ///
    /// 便捷方法，委托给 `DensityTier::load_ratio`。
    pub const fn load_ratio(&self) -> f64 {
        self.tier().load_ratio()
    }

    /// 计算实际加载容量（便捷方法）
    ///
    /// 委托给 `DensityTier::actual_load`。
    pub fn actual_load(&self, nominal_capacity: usize) -> usize {
        self.tier().actual_load(nominal_capacity)
    }

    /// 返回学习版本号（Static 返回 None，Learned 返回 Some(version)）
    pub fn version(&self) -> Option<u64> {
        match self {
            Self::Static(_) => None,
            Self::Learned { version, .. } => Some(*version),
        }
    }

    /// 是否为学习策略
    pub const fn is_learned(&self) -> bool {
        matches!(self, Self::Learned { .. })
    }

    /// 是否为静态策略
    pub const fn is_static(&self) -> bool {
        matches!(self, Self::Static(_))
    }

    /// 校验策略合法性（始终返回 true，DensityTier 枚举保证合法性）
    ///
    /// WHY 提供: 与 `SelectorPolicy::is_valid()` API 对称，
    /// 便于 P4-W14.5 能力场统一校验。
    pub const fn is_valid(&self) -> bool {
        true
    }
}

impl Default for DensityPolicy {
    /// 默认策略 = `Static(DensityTier::Rho10)`（无稀疏化，向后兼容）
    ///
    /// WHY(C4 合规): 默认值 = 当前 HCW L0/L1/L2 行为（无稀疏化），
    /// 调用方未注入策略时行为零变化（向后兼容）。
    fn default() -> Self {
        Self::Static(DensityTier::Rho10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // DensityTier 单元测试
    // ============================================================

    #[test]
    fn test_tier_default_is_rho10() {
        // Default 应等于 Rho10（无稀疏化，向后兼容）
        assert_eq!(DensityTier::default(), DensityTier::Rho10);
    }

    #[test]
    fn test_tier_rho_values() {
        assert!((DensityTier::Rho05.rho() - 0.5).abs() < 1e-6);
        assert!((DensityTier::Rho2.rho() - 2.0).abs() < 1e-6);
        assert!((DensityTier::Rho5.rho() - 5.0).abs() < 1e-6);
        assert!((DensityTier::Rho10.rho() - 10.0).abs() < 1e-6);
    }

    #[test]
    fn test_tier_load_ratio() {
        assert!((DensityTier::Rho05.load_ratio() - 0.05).abs() < 1e-6);
        assert!((DensityTier::Rho2.load_ratio() - 0.2).abs() < 1e-6);
        assert!((DensityTier::Rho5.load_ratio() - 0.5).abs() < 1e-6);
        assert!((DensityTier::Rho10.load_ratio() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_tier_sparse_ratio() {
        assert!((DensityTier::Rho05.sparse_ratio() - 20.0).abs() < 1e-6);
        assert!((DensityTier::Rho2.sparse_ratio() - 5.0).abs() < 1e-6);
        assert!((DensityTier::Rho5.sparse_ratio() - 2.0).abs() < 1e-6);
        assert!((DensityTier::Rho10.sparse_ratio() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_tier_short_name() {
        assert_eq!(DensityTier::Rho05.short_name(), "ρ=0.5");
        assert_eq!(DensityTier::Rho2.short_name(), "ρ=2");
        assert_eq!(DensityTier::Rho5.short_name(), "ρ=5");
        assert_eq!(DensityTier::Rho10.short_name(), "ρ=10");
    }

    #[test]
    fn test_tier_full_name() {
        assert_eq!(DensityTier::Rho05.full_name(), "极致稀疏化 (5% 加载)");
        assert_eq!(DensityTier::Rho10.full_name(), "无稀疏化 (100% 加载)");
    }

    #[test]
    fn test_tier_from_rho() {
        assert_eq!(DensityTier::from_rho(0.6), DensityTier::Rho05);
        assert_eq!(DensityTier::from_rho(1.0), DensityTier::Rho05); // 0.5 比 2 更近
        assert_eq!(DensityTier::from_rho(1.5), DensityTier::Rho2); // 等距取后者
        assert_eq!(DensityTier::from_rho(3.0), DensityTier::Rho2);
        assert_eq!(DensityTier::from_rho(4.0), DensityTier::Rho5);
        assert_eq!(DensityTier::from_rho(7.0), DensityTier::Rho5);
        assert_eq!(DensityTier::from_rho(8.0), DensityTier::Rho10);
        assert_eq!(DensityTier::from_rho(15.0), DensityTier::Rho10);
    }

    #[test]
    fn test_tier_actual_load() {
        assert_eq!(DensityTier::Rho05.actual_load(1000), 50);
        assert_eq!(DensityTier::Rho2.actual_load(1000), 200);
        assert_eq!(DensityTier::Rho5.actual_load(1000), 500);
        assert_eq!(DensityTier::Rho10.actual_load(1000), 1000);
    }

    #[test]
    fn test_tier_actual_load_zero_capacity() {
        // 边界: nominal=0 返回 0
        assert_eq!(DensityTier::Rho10.actual_load(0), 0);
        assert_eq!(DensityTier::Rho05.actual_load(0), 0);
    }

    #[test]
    fn test_tier_actual_load_l3_1m_equivalent() {
        // L3 1M 等效场景验证:
        // - 标称容量 1_048_576 (1M Token)
        // - ρ=0.5: 实际加载 52428 (5%, 接近 50K)
        // - ρ=2: 实际加载 209715 (20%, 接近 200K)
        // - ρ=5: 实际加载 524288 (50%, 512K)
        // - ρ=10: 实际加载 1048576 (100%, 1M, 违反架构红线需 hard_cap)
        let nominal = 1_048_576;
        assert_eq!(DensityTier::Rho05.actual_load(nominal), 52_428);
        assert_eq!(DensityTier::Rho2.actual_load(nominal), 209_715);
        assert_eq!(DensityTier::Rho5.actual_load(nominal), 524_288);
        assert_eq!(DensityTier::Rho10.actual_load(nominal), 1_048_576);
    }

    #[test]
    fn test_tier_all_returns_four() {
        let all = DensityTier::ALL;
        assert_eq!(all.len(), 4);
        assert!(all.contains(&DensityTier::Rho05));
        assert!(all.contains(&DensityTier::Rho10));
    }

    #[test]
    fn test_tier_copy_semantics() {
        let tier = DensityTier::Rho5;
        let copied = tier; // Copy
        assert_eq!(tier, copied);
    }

    #[test]
    fn test_tier_equality() {
        assert_eq!(DensityTier::Rho05, DensityTier::Rho05);
        assert_ne!(DensityTier::Rho05, DensityTier::Rho10);
    }

    #[test]
    fn test_tier_display() {
        assert_eq!(format!("{}", DensityTier::Rho05), "ρ=0.5");
        assert_eq!(format!("{}", DensityTier::Rho10), "ρ=10");
    }

    #[test]
    fn test_tier_serialize_json() {
        let tier = DensityTier::Rho2;
        let json = serde_json::to_string(&tier).unwrap();
        let deserialized: DensityTier = serde_json::from_str(&json).unwrap();
        assert_eq!(tier, deserialized);
    }

    #[test]
    fn test_tier_repr_u8() {
        // 验证 #[repr(u8)]: 内存中占 1 字节
        assert_eq!(std::mem::size_of::<DensityTier>(), 1);
    }

    // ============================================================
    // DensityPolicy 单元测试
    // ============================================================

    #[test]
    fn test_policy_default_is_static() {
        let policy = DensityPolicy::default();
        assert!(policy.is_static());
        assert!(!policy.is_learned());
    }

    #[test]
    fn test_policy_default_tier_is_rho10() {
        let policy = DensityPolicy::default();
        assert_eq!(policy.tier(), DensityTier::Rho10);
    }

    #[test]
    fn test_policy_default_version_none() {
        let policy = DensityPolicy::default();
        assert_eq!(policy.version(), None);
    }

    #[test]
    fn test_policy_fallback_equals_default() {
        assert_eq!(DensityPolicy::fallback(), DensityPolicy::default());
    }

    #[test]
    fn test_policy_fallback_is_static() {
        let fallback = DensityPolicy::fallback();
        assert!(fallback.is_static());
        assert!(!fallback.is_learned());
    }

    #[test]
    fn test_policy_static_constructor() {
        let policy = DensityPolicy::static_policy(DensityTier::Rho2);
        assert!(policy.is_static());
        assert_eq!(policy.version(), None);
        assert_eq!(policy.tier(), DensityTier::Rho2);
    }

    #[test]
    fn test_policy_learned_constructor() {
        let policy = DensityPolicy::learned(42, DensityTier::Rho5);
        assert!(policy.is_learned());
        assert!(!policy.is_static());
        assert_eq!(policy.version(), Some(42));
        assert_eq!(policy.tier(), DensityTier::Rho5);
    }

    #[test]
    fn test_policy_learned_version_zero() {
        // 版本号 0 是合法值（首个学习版本）
        let policy = DensityPolicy::learned(0, DensityTier::Rho10);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(0));
    }

    #[test]
    fn test_policy_tier_static() {
        let policy = DensityPolicy::Static(DensityTier::Rho05);
        assert_eq!(policy.tier(), DensityTier::Rho05);
    }

    #[test]
    fn test_policy_tier_learned() {
        let policy = DensityPolicy::Learned {
            version: 1,
            tier: DensityTier::Rho2,
        };
        assert_eq!(policy.tier(), DensityTier::Rho2);
    }

    #[test]
    fn test_policy_load_ratio() {
        let policy = DensityPolicy::static_policy(DensityTier::Rho2);
        assert!((policy.load_ratio() - 0.2).abs() < 1e-6);
    }

    #[test]
    fn test_policy_actual_load() {
        let policy = DensityPolicy::static_policy(DensityTier::Rho5);
        assert_eq!(policy.actual_load(1000), 500);
    }

    #[test]
    fn test_policy_is_valid_default() {
        assert!(DensityPolicy::default().is_valid());
    }

    #[test]
    fn test_policy_is_valid_learned() {
        let policy = DensityPolicy::learned(1, DensityTier::Rho05);
        assert!(policy.is_valid());
    }

    #[test]
    fn test_policy_equality_static() {
        let p1 = DensityPolicy::static_policy(DensityTier::Rho10);
        let p2 = DensityPolicy::static_policy(DensityTier::Rho10);
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_policy_equality_learned() {
        let p1 = DensityPolicy::learned(1, DensityTier::Rho2);
        let p2 = DensityPolicy::learned(1, DensityTier::Rho2);
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_policy_inequality_different_version() {
        let p1 = DensityPolicy::learned(1, DensityTier::Rho10);
        let p2 = DensityPolicy::learned(2, DensityTier::Rho10);
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_policy_inequality_different_tier() {
        let p1 = DensityPolicy::learned(1, DensityTier::Rho2);
        let p2 = DensityPolicy::learned(1, DensityTier::Rho5);
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_policy_serialize_json() {
        let policy = DensityPolicy::learned(42, DensityTier::Rho5);
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: DensityPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, deserialized);
    }

    #[test]
    fn test_policy_static_serialize_json() {
        let policy = DensityPolicy::static_policy(DensityTier::Rho05);
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: DensityPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, deserialized);
    }
}
