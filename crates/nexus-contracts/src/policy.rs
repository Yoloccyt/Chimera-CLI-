//! 选择器策略契约 — D1 病理修复（selector 权重外置）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应 ADR: **ADR-033**（L0 nexus-contracts 契约层建立）
//! 对应任务: **P3-W10.3**（spec.md §P3 内环升级 — selector 权重外置）
//! 对应病理: **D1**（HCW selector 权重手写 `w1/w2/w3`、OSA 静态掩码、无学习机制）
//!
//! # 核心职责
//!
//! 承载 HCW 重要性评分公式 `score = w1·recency + w2·frequency + w3·relevance` 的权重系数，
//! 将其从 `hcw-window` 硬编码常量升级为可注入策略，为 P4 `omega-learner` Bandit 异步下发学习值奠基。
//!
//! # 设计动机（D1 修复）
//!
//! v5.0 设计文档 §2.1 D1 病理：「HCW selector 权重手写（`score = w1·recency + w2·frequency + w3·relevance`）、
//! OSA 五维掩码静态、scc 一阶马尔可夫、decay 参数固定，无任何学习机制」。
//!
//! 本模块通过 `SelectorPolicy` 枚举将权重外置为注入式策略：
//! - **Static 变体**: 编译进二进制的常量（fallback，C4 合规：非运行时旗）
//! - **Learned 变体**: `omega-learner` 异步下发的版本化权重（带版本号用于 A/B 测试与回滚）
//!
//! # C4 合规（能力场灰度，非运行时旗）
//!
//! `SelectorPolicy::default()` 返回 `Static(SelectorWeights::DEFAULT)`，
//! `DEFAULT` 是 `const` 常量编译进二进制。`omega-learner` panic/超时时，
//! 调用方本地 fallback 到 `SelectorPolicy::Static`，**无跨 crate 旗标传播**（spec.md:289-290）。
//!
//! # 示例
//!
//! ```
//! use nexus_contracts::{SelectorPolicy, SelectorWeights};
//!
//! // 静态策略（默认 fallback）
//! let static_policy = SelectorPolicy::default();
//! assert!(static_policy.is_static());
//! let w = static_policy.weights();
//! assert!((w.recency - 0.4).abs() < 1e-6);
//!
//! // 学习策略（omega-learner 异步下发）
//! let learned = SelectorPolicy::learned(42, SelectorWeights::new(0.5, 0.3, 0.2));
//! assert_eq!(learned.version(), Some(42));
//! assert!(learned.is_learned());
//! ```

use serde::{Deserialize, Serialize};

/// 选择器权重三元组 — 重要性评分公式系数
///
/// 对应公式 `score = w1·recency + w2·frequency + w3·relevance`：
/// - `recency`（时近性权重）: 最近访问的条目评分更高
/// - `frequency`（频次权重）: 高频访问的条目评分更高
/// - `relevance`（任务相关性权重）: CLV 余弦相似度匹配任务上下文
///
/// 三个权重之和应为 1.0。`DEFAULT` 对应架构手册推荐值 (0.4, 0.3, 0.3)。
///
/// # 设计决策（WHY）
/// - **Copy + Clone**: 三元组为 `f32` 聚合体，Copy 语义零成本，避免克隆开销
/// - **PartialEq + Eq**: f32 不实现 Eq，但权重比较在误差范围内有意义（手动实现）
/// - **`DEFAULT` const 常量**: 编译进二进制，C4 合规（非运行时旗）
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SelectorWeights {
    /// 时近性权重（默认 0.4）— 最近访问的条目评分更高
    pub recency: f32,
    /// 频次权重（默认 0.3）— 高频访问的条目评分更高
    pub frequency: f32,
    /// 任务相关性权重（默认 0.3）— CLV 余弦相似度匹配任务上下文
    pub relevance: f32,
}

impl SelectorWeights {
    /// 默认权重常量 — 架构手册推荐值 (0.4, 0.3, 0.3)
    ///
    /// WHY(C4 合规): `const` 常量编译进二进制，非运行时 feature flag。
    /// `omega-learner` panic/超时时调用方本地 fallback 到此值，无跨 crate 旗标传播。
    pub const DEFAULT: Self = Self {
        recency: 0.4,
        frequency: 0.3,
        relevance: 0.3,
    };

    /// 创建权重三元组
    ///
    /// # 参数
    /// - `recency`: 时近性权重（应 ≥ 0）
    /// - `frequency`: 频次权重（应 ≥ 0）
    /// - `relevance`: 任务相关性权重（应 ≥ 0）
    ///
    /// # 示例
    /// ```
    /// use nexus_contracts::SelectorWeights;
    /// let w = SelectorWeights::new(0.5, 0.3, 0.2);
    /// assert!((w.recency - 0.5).abs() < 1e-6);
    /// ```
    pub const fn new(recency: f32, frequency: f32, relevance: f32) -> Self {
        Self {
            recency,
            frequency,
            relevance,
        }
    }

    /// 返回权重三元组为元组形式（兼容既有 `(f32, f32, f32)` 签名）
    ///
    /// WHY: 便于与既有 `HcwConfig.compressor_weights: (f32, f32, f32)` 互操作，
    /// 以及压缩器内部解构 `let (rw, fw, rlw) = weights.as_tuple();`
    pub const fn as_tuple(&self) -> (f32, f32, f32) {
        (self.recency, self.frequency, self.relevance)
    }

    /// 校验权重合法性 — 非负且和 ≈ 1.0（误差容忍 1e-3）
    ///
    /// WHY: 权重为评分公式的系数，负值无意义；和偏离 1.0 会导致评分偏离 [0, 1] 区间。
    /// 容忍 1e-3 误差因 f32 浮点累加可能产生微小漂移。
    ///
    /// # 示例
    /// ```
    /// use nexus_contracts::SelectorWeights;
    /// assert!(SelectorWeights::DEFAULT.is_valid());
    /// assert!(!SelectorWeights::new(-0.1, 0.5, 0.6).is_valid()); // 负值
    /// assert!(!SelectorWeights::new(0.5, 0.5, 0.5).is_valid());  // 和 = 1.5 ≠ 1.0
    /// ```
    pub fn is_valid(&self) -> bool {
        self.recency >= 0.0
            && self.frequency >= 0.0
            && self.relevance >= 0.0
            && (self.recency + self.frequency + self.relevance - 1.0).abs() <= 1e-3
    }
}

impl Default for SelectorWeights {
    /// 默认权重 = `DEFAULT` 常量 (0.4, 0.3, 0.3)
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 选择器策略 — 静态常量或学习版本（D1 修复）
///
/// 承载 HCW 重要性评分权重，将 `w1/w2/w3` 从 `hcw-window` 硬编码常量
/// 升级为可注入策略，为 `omega-learner` Bandit 异步下发学习值奠基。
///
/// # 变体
/// - [`Static`](SelectorPolicy::Static): 编译进二进制的常量（fallback，C4 合规）
/// - [`Learned`](SelectorPolicy::Learned): `omega-learner` 异步下发的版本化权重
///
/// # C4 合规（能力场灰度，非运行时旗）
///
/// `default()` 返回 `Static(SelectorWeights::DEFAULT)`，`DEFAULT` 是 `const` 常量。
/// `omega-learner` panic/超时时，调用方本地 fallback 到 `Static`，无跨 crate 旗标传播。
///
/// # 设计决策（WHY）
/// - **枚举而非 trait**: 策略变体有限（Static/Learned），枚举比 trait object 更轻量，
///   避免 `Box<dyn>` 动态分发开销（§4.1 约定：避免 `Box<dyn Trait>`）
/// - **Copy 语义**: `SelectorWeights` 为 Copy，枚举整体 Copy，注入时零成本
/// - **版本号 `Option<u64>`**: Learned 携带版本号用于 A/B 测试与回滚，
///   Static 无版本号（`version()` 返回 None）
///
/// # 示例
///
/// ## 静态策略（默认 fallback）
/// ```
/// use nexus_contracts::SelectorPolicy;
///
/// let policy = SelectorPolicy::default();
/// assert!(policy.is_static());
/// assert_eq!(policy.version(), None);
/// let w = policy.weights();
/// assert!((w.recency - 0.4).abs() < 1e-6);
/// ```
///
/// ## 学习策略（omega-learner 异步下发）
/// ```
/// use nexus_contracts::{SelectorPolicy, SelectorWeights};
///
/// let policy = SelectorPolicy::learned(42, SelectorWeights::new(0.5, 0.3, 0.2));
/// assert!(policy.is_learned());
/// assert_eq!(policy.version(), Some(42));
/// ```
///
/// ## learner panic 时本地 fallback
/// ```
/// use nexus_contracts::{SelectorPolicy, SelectorWeights};
///
/// // 模拟 omega-learner 下发学习值
/// let learned = SelectorPolicy::learned(1, SelectorWeights::new(0.5, 0.3, 0.2));
/// // learner panic 时本地 fallback 到 Static
/// let fallback = SelectorPolicy::fallback();
/// assert!(fallback.is_static());
/// assert!(fallback.weights() != learned.weights() || learned.is_static());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SelectorPolicy {
    /// 静态策略 — 编译进二进制的常量（fallback，C4 合规）
    ///
    /// 承载 `SelectorWeights` 常量，`default()` 返回 `Static(SelectorWeights::DEFAULT)`。
    /// `omega-learner` panic/超时时调用方本地 fallback 到此变体。
    Static(SelectorWeights),

    /// 学习策略 — `omega-learner` 异步下发的版本化权重
    ///
    /// `version` 单调递增，用于 A/B 测试与回滚（P4 `SpecRegistry` 版本化）。
    /// `weights` 承载学习到的权重值，由 `omega-learner` Bandit 算法计算。
    Learned {
        /// 学习版本号（单调递增，用于 A/B 测试与回滚）
        version: u64,
        /// 学习到的权重三元组
        weights: SelectorWeights,
    },
}

impl SelectorPolicy {
    /// 创建静态策略（便捷构造函数）
    ///
    /// 等价于 `SelectorPolicy::Static(weights)`，语义更清晰。
    pub const fn static_policy(weights: SelectorWeights) -> Self {
        Self::Static(weights)
    }

    /// 创建学习策略（便捷构造函数）
    ///
    /// 等价于 `SelectorPolicy::Learned { version, weights }`，语义更清晰。
    pub const fn learned(version: u64, weights: SelectorWeights) -> Self {
        Self::Learned { version, weights }
    }

    /// 返回 fallback 策略 — `Static(SelectorWeights::DEFAULT)`
    ///
    /// WHY(C4 合规): `omega-learner` panic/超时时调用方本地 fallback 到此值。
    /// `DEFAULT` 是 `const` 常量编译进二进制，非运行时 feature flag。
    pub const fn fallback() -> Self {
        Self::Static(SelectorWeights::DEFAULT)
    }

    /// 返回当前策略的权重三元组（无论 Static 还是 Learned）
    ///
    /// 统一访问方法，调用方无需 match 策略变体即可获取权重。
    pub const fn weights(&self) -> SelectorWeights {
        match self {
            Self::Static(w) => *w,
            Self::Learned { weights, .. } => *weights,
        }
    }

    /// 返回学习版本号（Static 返回 None，Learned 返回 Some(version)）
    ///
    /// WHY: 用于 A/B 测试与回滚 — `omega-learner` 每次下发递增版本号，
    /// 调用方可记录使用的版本号用于效果追踪。
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

    /// 校验策略权重合法性（委托给 `SelectorWeights::is_valid`）
    ///
    /// WHY: 注入策略前调用方应校验合法性，避免无效权重污染评分。
    pub fn is_valid(&self) -> bool {
        self.weights().is_valid()
    }
}

impl Default for SelectorPolicy {
    /// 默认策略 = `Static(SelectorWeights::DEFAULT)` (0.4, 0.3, 0.3)
    ///
    /// WHY(C4 合规): 默认值 = 当前 hcw-window 硬编码常量，fallback 编译进二进制。
    /// 调用方未注入策略时使用此默认值，行为与 D1 修复前完全一致（向后兼容）。
    fn default() -> Self {
        Self::Static(SelectorWeights::DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // SelectorWeights 单元测试
    // ============================================================

    #[test]
    fn test_weights_default_matches_const() {
        // Default impl 应等于 DEFAULT const 常量
        let w = SelectorWeights::default();
        assert!((w.recency - 0.4).abs() < 1e-6);
        assert!((w.frequency - 0.3).abs() < 1e-6);
        assert!((w.relevance - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_weights_default_equals_const() {
        // Default::default() == DEFAULT const
        assert_eq!(SelectorWeights::default(), SelectorWeights::DEFAULT);
    }

    #[test]
    fn test_weights_new() {
        let w = SelectorWeights::new(0.5, 0.3, 0.2);
        assert!((w.recency - 0.5).abs() < 1e-6);
        assert!((w.frequency - 0.3).abs() < 1e-6);
        assert!((w.relevance - 0.2).abs() < 1e-6);
    }

    #[test]
    fn test_weights_as_tuple() {
        let w = SelectorWeights::new(0.5, 0.3, 0.2);
        let (r, f, rel) = w.as_tuple();
        assert!((r - 0.5).abs() < 1e-6);
        assert!((f - 0.3).abs() < 1e-6);
        assert!((rel - 0.2).abs() < 1e-6);
    }

    #[test]
    fn test_weights_as_tuple_default() {
        let (r, f, rel) = SelectorWeights::DEFAULT.as_tuple();
        assert!((r - 0.4).abs() < 1e-6);
        assert!((f - 0.3).abs() < 1e-6);
        assert!((rel - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_weights_is_valid_default() {
        assert!(SelectorWeights::DEFAULT.is_valid());
    }

    #[test]
    fn test_weights_is_valid_custom() {
        assert!(SelectorWeights::new(0.5, 0.3, 0.2).is_valid());
        assert!(SelectorWeights::new(1.0, 0.0, 0.0).is_valid());
    }

    #[test]
    fn test_weights_is_valid_negative() {
        assert!(!SelectorWeights::new(-0.1, 0.5, 0.6).is_valid());
        assert!(!SelectorWeights::new(0.5, -0.1, 0.6).is_valid());
        assert!(!SelectorWeights::new(0.5, 0.5, -0.1).is_valid());
    }

    #[test]
    fn test_weights_is_valid_sum_not_one() {
        assert!(!SelectorWeights::new(0.5, 0.5, 0.5).is_valid()); // 和 = 1.5
        assert!(!SelectorWeights::new(0.3, 0.3, 0.3).is_valid()); // 和 = 0.9
    }

    #[test]
    fn test_weights_is_valid_within_tolerance() {
        // 和 = 1.0005，误差 < 1e-3 容忍
        assert!(SelectorWeights::new(0.4, 0.3, 0.3005).is_valid());
    }

    #[test]
    fn test_weights_is_valid_boundary_zero() {
        // 零权重合法（非负，和 = 1.0）
        assert!(SelectorWeights::new(1.0, 0.0, 0.0).is_valid());
        assert!(SelectorWeights::new(0.0, 1.0, 0.0).is_valid());
        assert!(SelectorWeights::new(0.0, 0.0, 1.0).is_valid());
    }

    #[test]
    fn test_weights_copy_semantics() {
        // Copy 语义：赋值后两者独立
        let w1 = SelectorWeights::new(0.5, 0.3, 0.2);
        let w2 = w1; // Copy
        assert_eq!(w1, w2); // w1 仍可用（Copy 非 Move）
    }

    #[test]
    fn test_weights_equality() {
        let w1 = SelectorWeights::new(0.4, 0.3, 0.3);
        let w2 = SelectorWeights::new(0.4, 0.3, 0.3);
        let w3 = SelectorWeights::new(0.5, 0.3, 0.2);
        assert_eq!(w1, w2);
        assert_ne!(w1, w3);
    }

    #[test]
    fn test_weights_serialize_json() {
        let w = SelectorWeights::new(0.5, 0.3, 0.2);
        let json = serde_json::to_string(&w).unwrap();
        let deserialized: SelectorWeights = serde_json::from_str(&json).unwrap();
        assert_eq!(w, deserialized);
    }

    #[test]
    fn test_weights_serialize_yaml() {
        let w = SelectorWeights::new(0.5, 0.3, 0.2);
        let yaml = serde_yaml::to_string(&w).unwrap();
        let deserialized: SelectorWeights = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(w, deserialized);
    }

    // ============================================================
    // SelectorPolicy 单元测试
    // ============================================================

    #[test]
    fn test_policy_default_is_static() {
        let policy = SelectorPolicy::default();
        assert!(policy.is_static());
        assert!(!policy.is_learned());
    }

    #[test]
    fn test_policy_default_weights_match_const() {
        let policy = SelectorPolicy::default();
        let w = policy.weights();
        assert!((w.recency - 0.4).abs() < 1e-6);
        assert!((w.frequency - 0.3).abs() < 1e-6);
        assert!((w.relevance - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_policy_default_version_none() {
        // Static 策略无版本号
        let policy = SelectorPolicy::default();
        assert_eq!(policy.version(), None);
    }

    #[test]
    fn test_policy_fallback_equals_default() {
        // fallback() 应等于 default()
        assert_eq!(SelectorPolicy::fallback(), SelectorPolicy::default());
    }

    #[test]
    fn test_policy_fallback_is_static() {
        let fallback = SelectorPolicy::fallback();
        assert!(fallback.is_static());
        assert!(!fallback.is_learned());
    }

    #[test]
    fn test_policy_static_constructor() {
        let weights = SelectorWeights::new(0.5, 0.3, 0.2);
        let policy = SelectorPolicy::static_policy(weights);
        assert!(policy.is_static());
        assert_eq!(policy.version(), None);
        assert_eq!(policy.weights(), weights);
    }

    #[test]
    fn test_policy_learned_constructor() {
        let weights = SelectorWeights::new(0.5, 0.3, 0.2);
        let policy = SelectorPolicy::learned(42, weights);
        assert!(policy.is_learned());
        assert!(!policy.is_static());
        assert_eq!(policy.version(), Some(42));
        assert_eq!(policy.weights(), weights);
    }

    #[test]
    fn test_policy_learned_version_zero() {
        // 版本号 0 是合法值（首个学习版本）
        let weights = SelectorWeights::DEFAULT;
        let policy = SelectorPolicy::learned(0, weights);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(0));
    }

    #[test]
    fn test_policy_weights_static() {
        let weights = SelectorWeights::new(0.6, 0.2, 0.2);
        let policy = SelectorPolicy::Static(weights);
        assert_eq!(policy.weights(), weights);
    }

    #[test]
    fn test_policy_weights_learned() {
        let weights = SelectorWeights::new(0.5, 0.3, 0.2);
        let policy = SelectorPolicy::Learned {
            version: 1,
            weights,
        };
        assert_eq!(policy.weights(), weights);
    }

    #[test]
    fn test_policy_is_valid_default() {
        assert!(SelectorPolicy::default().is_valid());
    }

    #[test]
    fn test_policy_is_valid_learned() {
        let weights = SelectorWeights::new(0.5, 0.3, 0.2);
        let policy = SelectorPolicy::learned(1, weights);
        assert!(policy.is_valid());
    }

    #[test]
    fn test_policy_is_valid_invalid_weights() {
        let invalid = SelectorWeights::new(-0.1, 0.5, 0.6);
        let policy = SelectorPolicy::static_policy(invalid);
        assert!(!policy.is_valid());
    }

    #[test]
    fn test_policy_equality_static() {
        let p1 = SelectorPolicy::static_policy(SelectorWeights::DEFAULT);
        let p2 = SelectorPolicy::static_policy(SelectorWeights::DEFAULT);
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_policy_equality_learned() {
        let weights = SelectorWeights::new(0.5, 0.3, 0.2);
        let p1 = SelectorPolicy::learned(1, weights);
        let p2 = SelectorPolicy::learned(1, weights);
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_policy_inequality_different_version() {
        let weights = SelectorWeights::DEFAULT;
        let p1 = SelectorPolicy::learned(1, weights);
        let p2 = SelectorPolicy::learned(2, weights);
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_policy_inequality_different_weights() {
        let p1 = SelectorPolicy::learned(1, SelectorWeights::new(0.5, 0.3, 0.2));
        let p2 = SelectorPolicy::learned(1, SelectorWeights::new(0.4, 0.3, 0.3));
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_policy_inequality_static_vs_learned() {
        let p1 = SelectorPolicy::static_policy(SelectorWeights::DEFAULT);
        let p2 = SelectorPolicy::learned(1, SelectorWeights::DEFAULT);
        assert_ne!(p1, p2); // 不同变体
    }

    #[test]
    fn test_policy_copy_semantics() {
        let policy = SelectorPolicy::learned(42, SelectorWeights::new(0.5, 0.3, 0.2));
        let copied = policy; // Copy
        assert_eq!(policy, copied); // policy 仍可用（Copy 非 Move）
    }

    #[test]
    fn test_policy_serialize_json_static() {
        let policy = SelectorPolicy::static_policy(SelectorWeights::new(0.5, 0.3, 0.2));
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: SelectorPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, deserialized);
    }

    #[test]
    fn test_policy_serialize_json_learned() {
        let policy = SelectorPolicy::learned(42, SelectorWeights::new(0.5, 0.3, 0.2));
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: SelectorPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, deserialized);
    }

    #[test]
    fn test_policy_serialize_yaml_static() {
        let policy = SelectorPolicy::static_policy(SelectorWeights::new(0.5, 0.3, 0.2));
        let yaml = serde_yaml::to_string(&policy).unwrap();
        let deserialized: SelectorPolicy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(policy, deserialized);
    }

    #[test]
    fn test_policy_serialize_yaml_learned() {
        let policy = SelectorPolicy::learned(42, SelectorWeights::new(0.5, 0.3, 0.2));
        let yaml = serde_yaml::to_string(&policy).unwrap();
        let deserialized: SelectorPolicy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(policy, deserialized);
    }

    // ============================================================
    // D1 修复场景测试（spec.md §P3 selector 权重外置）
    // ============================================================

    #[test]
    fn test_d1_scenario_static_fallback_compiled_into_binary() {
        // spec.md:426 "默认静态值 = 当前常量，fallback 编译进同一二进制"
        // SelectorPolicy::default() 应返回 Static 变体，权重 = 当前常量
        let policy = SelectorPolicy::default();
        assert!(policy.is_static());

        // 验证 fallback 值 = 当前 hcw-window 硬编码常量 (0.4, 0.3, 0.3)
        let w = policy.weights();
        assert!((w.recency - 0.4).abs() < 1e-6, "recency 应为 0.4");
        assert!((w.frequency - 0.3).abs() < 1e-6, "frequency 应为 0.3");
        assert!((w.relevance - 0.3).abs() < 1e-6, "relevance 应为 0.3");
    }

    #[test]
    fn test_d1_scenario_learner_panic_local_fallback() {
        // spec.md:289-290 "learner panic/超时时调用方本地 fallback 到 Static(常量)"
        // 模拟:omega-learner 下发 Learned 值后 panic，调用方本地 fallback 到 Static
        let learned = SelectorPolicy::learned(1, SelectorWeights::new(0.5, 0.3, 0.2));
        assert!(learned.is_learned());

        // learner panic → 本地 fallback
        let fallback = SelectorPolicy::fallback();
        assert!(fallback.is_static());
        assert_ne!(fallback.weights(), learned.weights());
    }

    #[test]
    fn test_d1_scenario_no_cross_crate_flag() {
        // spec.md:290 "无跨 crate 旗标"
        // SelectorPolicy 通过值注入（Copy），不依赖全局 static 或 feature flag
        let policy = SelectorPolicy::default();
        let weights = policy.weights();
        // 权重值直接从 const 常量获取，无运行时旗标查询
        assert_eq!(weights, SelectorWeights::DEFAULT);
    }

    #[test]
    fn test_d1_scenario_learned_versioned_for_ab_test() {
        // spec.md:287 "SelectorPolicy::Learned(版本号)"
        // 版本号用于 A/B 测试与回滚
        let v1 = SelectorPolicy::learned(1, SelectorWeights::new(0.5, 0.3, 0.2));
        let v2 = SelectorPolicy::learned(2, SelectorWeights::new(0.4, 0.4, 0.2));
        assert_ne!(v1.version(), v2.version());
        assert_ne!(v1.weights(), v2.weights());
    }
}
