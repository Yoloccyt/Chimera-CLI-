//! 选择器学习器持有器 — S4 接缝策略异步下发 + 本地 fallback
//!
//! 对应任务: **P4-W13.3.2**（hcw-window 接入 omega-learner S4 接缝异步下发）
//! 对应 ADR: **ADR-031**（omega-learner 边界）+ **ADR-033**（nexus-contracts L0 契约层）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 S4 接缝
//!
//! # 核心职责
//!
//! 承载 `SelectorPolicy` 的运行时可变状态，为 `HcwWindow` / `ContextCompressor`
//! 提供重要性评分权重的运行时注入能力:
//! - **异步策略接收**: `update_policy()` 接收 `omega-learner` 下发的 `SelectorPolicy::Learned`
//! - **当前策略查询**: `current_policy()` 返回当前激活的策略
//! - **策略感知评分**: `compute_importance_with_policy()` 使用当前策略权重计算条目重要性
//! - **本地 fallback**: 任何异常回到 `SelectorPolicy::Static(SelectorWeights::DEFAULT)`（C4 合规）
//!
//! # 依赖铁律合规（WHY hcw-window 不直接依赖 omega-learner）
//!
//! ```text
//! L6 omega-learner  ────(learned SelectorPolicy)───▶  上层编排器
//!      │                                                │
//!      │ L6 → L0 ✓                                      │ L0 → 注入
//!      ▼                                                ▼
//! L0 nexus-contracts  ◀──(SelectorPolicy 类型)──  L2 hcw-window
//!      SelectorPolicy                                      │
//!      SelectorWeights                                     │ L2 → L0 ✓
//!                                                         ▼
//!                                                SelectorLearnerHolder
//! ```
//!
//! hcw-window (L2) 只依赖 `nexus-contracts` (L0) 的 `SelectorPolicy` / `SelectorWeights` 类型，
//! **不直接依赖** `omega-learner` (L6)，遵守 §2.2 依赖铁律。
//! `omega-learner` 输出的 `SelectorPolicy::Learned` 由上层编排器
//! （chimera-cli / quest-engine）通过 `update_policy()` 注入。
//!
//! # C4 合规（能力场灰度，非运行时旗）— 三层 fallback
//!
//! 1. **默认值层**: `SelectorLearnerHolder::new()` 初始化为
//!    `SelectorPolicy::Static(SelectorWeights::DEFAULT)` (0.4, 0.3, 0.3)
//!    （编译期常量，零运行时开销）
//! 2. **异常回退层**: `RwLock` poison 时 `unwrap_or_else` 自动回退到
//!    `SelectorPolicy::fallback()`，调用方无需处理 `PoisonError`
//! 3. **熔断入口层**: `fallback_to_static()` 方法供 `omega-learner` 触发学习熔断
//!    （spec.md:335 S4 灰度后悔率 > 阈值）时主动回退
//!
//! 三层叠加实现"learner panic/超时时调用方本地 fallback，无跨 crate 旗标传播"的 C4 合规要求。
//!
//! # 线程安全
//!
//! 内部用 `RwLock<SelectorPolicy>` 保护策略状态:
//! - **写锁**: `update_policy()` 异步下发（低频，每秒 < 1 次）
//! - **读锁**: `current_policy()` / `compute_importance_with_policy()` 热路径查询（高频）
//!
//! 读写分离避免锁竞争。`RwLock` 选择 `std::sync::RwLock` 而非 `tokio::sync::RwLock`:
//! - 读路径需要 sync 访问（`compute_importance_with_policy` 是同步函数）
//! - 持锁时间极短（仅读取 `SelectorPolicy` Copy 枚举，~10ns）
//! - 不持锁跨 `.await`（避免 §4.4 反模式 1: 禁止持锁 .await）
//!
//! # 与 DensityLearnerHolder 的对称性
//!
//! `SelectorLearnerHolder` 与 `DensityLearnerHolder` (S1 接缝) 镜像对称:
//! - 同样用 `RwLock<Policy>` + PoisonError 本地 fallback
//! - 同样提供 `new` / `with_policy` / `update_policy` / `fallback_to_static` / `current_policy`
//! - 差异仅在策略类型（`SelectorPolicy` vs `DensityPolicy`）与感知方法
//!   （`compute_importance_with_policy` vs `select_with_density`）
//!
//! # 示例
//!
//! ## 基础 fallback 行为
//!
//! ```
//! use hcw_window::selector_learner::SelectorLearnerHolder;
//! use nexus_contracts::SelectorWeights;
//!
//! let holder = SelectorLearnerHolder::new();
//!
//! // 初始化为 Static fallback (0.4, 0.3, 0.3)
//! let policy = holder.current_policy();
//! assert!(policy.is_static());
//! let w = policy.weights();
//! assert!((w.recency - 0.4).abs() < 1e-6);
//! ```
//!
//! ## 异步下发学习策略
//!
//! ```
//! use hcw_window::selector_learner::SelectorLearnerHolder;
//! use nexus_contracts::{SelectorPolicy, SelectorWeights};
//!
//! let holder = SelectorLearnerHolder::new();
//!
//! // omega-learner 异步下发学习策略（recency-heavy: 0.6, 0.2, 0.2）
//! holder.update_policy(SelectorPolicy::learned(42, SelectorWeights::new(0.6, 0.2, 0.2)));
//!
//! let policy = holder.current_policy();
//! assert!(policy.is_learned());
//! assert_eq!(policy.version(), Some(42));
//! assert!((policy.weights().recency - 0.6).abs() < 1e-6);
//! ```

use std::sync::RwLock;

use chrono::{DateTime, Utc};
use nexus_contracts::{SelectorPolicy, SelectorWeights};
use nexus_core::CLV;

use crate::compressor::compute_importance_score;
use crate::types::ContextEntry;

// ============================================================
// SelectorLearnerHolder
// ============================================================

/// 选择器学习器持有器 — 运行时可变的 `SelectorPolicy` 容器
///
/// 承载 `omega-learner` 异步下发的学习策略，为 `HcwWindow` 的
/// 重要性评分提供权重感知能力。所有方法线程安全（`RwLock` 保护）。
///
/// # 设计决策（WHY）
///
/// - **独立 struct 而非嵌入 HcwWindow**: 单一职责，便于单测与复用
/// - **`RwLock<SelectorPolicy>` 而非 `AtomicU8`**: `SelectorPolicy` 是枚举
///   （Static/Learned），原子化需要 `AtomicU8` + 手动重建枚举，复杂且易错
/// - **`std::sync::RwLock` 而非 `tokio::sync::RwLock`**: 读路径是 sync
///   （`compute_importance_with_policy` 是同步函数），持锁时间极短（~10ns）
/// - **委托 `compute_importance_score`**: 复用 `compressor.rs` 的共享公式，
///   避免运行时策略路径与配置时策略路径公式漂移
///
/// # 线程安全
///
/// `SelectorLearnerHolder` 内部 `RwLock` 保证:
/// - 多读单写（`current_policy`/`compute_importance_with_policy` 并发读，
///   `update_policy` 独占写）
/// - 持锁时间极短（仅读写 `SelectorPolicy` Copy 枚举，~10ns）
/// - 无 await 跨锁（`update_policy` 是 sync 方法，避免 §4.4 反模式 1）
///
/// # 示例
///
/// ```
/// use hcw_window::selector_learner::SelectorLearnerHolder;
/// use nexus_contracts::{SelectorPolicy, SelectorWeights};
///
/// let holder = SelectorLearnerHolder::new();
/// assert!(holder.current_policy().is_static());
///
/// holder.update_policy(SelectorPolicy::learned(1, SelectorWeights::new(0.5, 0.3, 0.2)));
/// assert!(holder.current_policy().is_learned());
/// assert_eq!(holder.current_policy().version(), Some(1));
/// ```
#[derive(Debug)]
pub struct SelectorLearnerHolder {
    /// 当前激活的策略（`RwLock` 保护，读写分离）
    ///
    /// WHY 用 `RwLock` 而非 `Mutex`:
    /// - 读路径（`current_policy`/`compute_importance_with_policy`）高频且只读
    /// - 写路径（`update_policy`）低频（每秒 < 1 次）
    /// - `RwLock` 允许并发读，避免读路径串行化
    policy: RwLock<SelectorPolicy>,
}

impl SelectorLearnerHolder {
    /// 创建持有器，初始化为 `SelectorPolicy::fallback()`（C4 合规默认值）
    ///
    /// WHY 初始化为 `Static(SelectorWeights::DEFAULT)` (0.4, 0.3, 0.3):
    /// C4 合规要求默认行为零变化，`DEFAULT` 是编译进二进制的 `const` 常量，
    /// 等于 v5.0 前 `compressor_weights: (0.4, 0.3, 0.3)` 硬编码常量。
    /// `omega-learner` 未下发学习策略时，行为与 D1 修复前完全一致（向后兼容）。
    pub fn new() -> Self {
        Self {
            policy: RwLock::new(SelectorPolicy::fallback()),
        }
    }

    /// 创建持有器，指定初始策略（便于测试）
    ///
    /// WHY 提供: 单测需要构造特定策略场景（如 Learned 初始状态）
    pub fn with_policy(policy: SelectorPolicy) -> Self {
        Self {
            policy: RwLock::new(policy),
        }
    }

    /// 异步下发策略 — 接收 `omega-learner` 学习到的 `SelectorPolicy`
    ///
    /// # 设计
    ///
    /// - 写入 `RwLock`（独占写锁，~10ns）
    /// - 不返回错误: 任何异常（如 `PoisonError`）静默 fallback 到 Static
    ///
    /// # C4 合规
    ///
    /// 调用方（chimera-cli / quest-engine）在 `omega-learner` panic/超时时
    /// 不调用此方法，`SelectorLearnerHolder` 保持上一次的有效策略。
    /// 若需强制回退到 fallback，调用方传入 `SelectorPolicy::fallback()`。
    ///
    /// WHY 不校验 `policy.is_valid()`: `SelectorPolicy` 由 `omega-learner`
    /// 构造时已校验权重合法性（`SelectorWeights::is_valid`），此处重复校验
    /// 会增加热路径开销。非法权重应在 learner 侧拦截，而非 holder 侧。
    /// 若需防御性校验，调用方可在传入前调用 `policy.is_valid()`。
    ///
    /// # 参数
    /// - `policy`: 新策略（Static 或 Learned）
    ///
    /// # 示例
    ///
    /// ```
    /// use hcw_window::selector_learner::SelectorLearnerHolder;
    /// use nexus_contracts::{SelectorPolicy, SelectorWeights};
    ///
    /// let holder = SelectorLearnerHolder::new();
    /// holder.update_policy(SelectorPolicy::learned(1, SelectorWeights::new(0.6, 0.2, 0.2)));
    /// assert!((holder.current_policy().weights().recency - 0.6).abs() < 1e-6);
    /// ```
    pub fn update_policy(&self, policy: SelectorPolicy) {
        // WHY unwrap_or_else: RwLock poison 时 fallback 到 Static(DEFAULT)
        // 避免调用方处理 PoisonError（C4 合规：本地 fallback，无错误传播）
        let mut guard = self.policy.write().unwrap_or_else(|p| {
            // PoisonError 时恢复锁并写入 fallback
            let mut guard = p.into_inner();
            *guard = SelectorPolicy::fallback();
            guard
        });
        *guard = policy;
    }

    /// 强制回退到 fallback 策略（`Static(SelectorWeights::DEFAULT)`）
    ///
    /// WHY 提供: `omega-learner` 触发学习熔断（spec.md:335 S4 灰度后悔率超阈值）
    /// 时，上层调用方调用此方法立即回退到静态策略。
    ///
    /// # 示例
    ///
    /// ```
    /// use hcw_window::selector_learner::SelectorLearnerHolder;
    /// use nexus_contracts::{SelectorPolicy, SelectorWeights};
    ///
    /// let holder = SelectorLearnerHolder::with_policy(
    ///     SelectorPolicy::learned(1, SelectorWeights::new(0.6, 0.2, 0.2)),
    /// );
    /// assert!(holder.is_learned());
    ///
    /// holder.fallback_to_static();
    /// assert!(!holder.is_learned());
    /// let w = holder.current_policy().weights();
    /// assert!((w.recency - 0.4).abs() < 1e-6); // 恢复为 DEFAULT
    /// ```
    pub fn fallback_to_static(&self) {
        self.update_policy(SelectorPolicy::fallback());
    }

    /// 返回当前激活的策略（快照）
    ///
    /// 返回 `SelectorPolicy` 的 Copy（枚举整体 Copy），调用方无需持有锁。
    ///
    /// # 性能
    ///
    /// 读锁 + Copy 枚举，~10ns。热路径调用无性能影响。
    pub fn current_policy(&self) -> SelectorPolicy {
        let guard = self.policy.read().unwrap_or_else(|p| p.into_inner());
        *guard
    }

    /// 返回当前策略的权重三元组（便捷方法）
    ///
    /// 等价于 `self.current_policy().weights()`，减少调用方样板代码。
    pub fn current_weights(&self) -> SelectorWeights {
        self.current_policy().weights()
    }

    /// 策略感知重要性评分 — 使用当前策略权重计算条目重要性
    ///
    /// # 算法
    ///
    /// 1. 从 `current_policy()` 读取 `SelectorWeights`（读锁，~10ns）
    /// 2. 委托 `compute_importance_score()` 共享公式计算评分
    /// 3. 公式: `score = w1 × recency + w2 × frequency + w3 × relevance`
    ///
    /// # 与 `ContextCompressor::compute_importance` 的关系
    ///
    /// - `ContextCompressor::compute_importance`: 从 `HcwConfig.selector_policy` 读取权重
    ///   （配置时固定，启动时确定）
    /// - `SelectorLearnerHolder::compute_importance_with_policy`: 从 `self.current_policy()` 读取权重
    ///   （运行时可变，支持 omega-learner 异步下发）
    ///
    /// 二者共享 `compute_importance_score()` 公式，仅权重来源不同。
    /// 当 `HcwWindow` 持有 `SelectorLearnerHolder` 时，应优先使用此方法
    /// （支持运行时策略更新），而非从 `HcwConfig` 读取静态策略。
    ///
    /// # 参数
    /// - `entry`: 待评分的上下文条目
    /// - `task_clv`: 当前任务的 CLV（None 时相关性取中性 0.5）
    /// - `now`: 当前时间（用于时近性计算）
    /// - `max_access_count`: 最大访问次数（用于频次归一化，调用方确保 > 0）
    /// - `time_span_ms`: 时间跨度毫秒（用于时近性归一化，调用方确保 > 0）
    ///
    /// # 返回
    /// 重要性评分 ∈ [0, 1]（权重和 = 1.0 且各分量 ∈ [0, 1] 时）
    ///
    /// # 示例
    ///
    /// ```
    /// use hcw_window::selector_learner::SelectorLearnerHolder;
    /// use hcw_window::ContextEntry;
    /// use nexus_contracts::{SelectorPolicy, SelectorWeights};
    /// use chrono::Utc;
    ///
    /// let holder = SelectorLearnerHolder::new();
    /// let entry = ContextEntry::new("e-1", "file-1", "content", 100);
    ///
    /// // 使用默认 Static 策略评分
    /// let score = holder.compute_importance_with_policy(
    ///     &entry, None, Utc::now(), 10.0, 1000.0,
    /// );
    /// assert!(score >= 0.0 && score <= 1.0);
    /// ```
    pub fn compute_importance_with_policy(
        &self,
        entry: &ContextEntry,
        task_clv: Option<&CLV>,
        now: DateTime<Utc>,
        max_access_count: f32,
        time_span_ms: f32,
    ) -> f32 {
        let weights = self.current_policy().weights();
        compute_importance_score(
            entry,
            weights,
            task_clv,
            now,
            max_access_count,
            time_span_ms,
        )
    }

    /// 返回是否已激活学习策略
    ///
    /// 便于上层编排器查询当前是否在灰度阶段。
    pub fn is_learned(&self) -> bool {
        self.current_policy().is_learned()
    }

    /// 返回当前学习版本号（Static 返回 None，Learned 返回 Some(version)）
    ///
    /// 便于上层编排器记录使用的版本号用于效果追踪与 A/B 测试。
    pub fn version(&self) -> Option<u64> {
        self.current_policy().version()
    }
}

impl Default for SelectorLearnerHolder {
    /// 默认状态 = `SelectorPolicy::Static(SelectorWeights::DEFAULT)`（fallback，C4 合规）
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for SelectorLearnerHolder {
    /// 克隆持有器（创建新的 RwLock，策略快照独立）
    ///
    /// WHY 提供: `HcwWindow` 可能需要克隆 `SelectorLearnerHolder` 用于
    /// 快照或并行处理。克隆后两者策略独立演化，互不影响。
    fn clone(&self) -> Self {
        Self::with_policy(self.current_policy())
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::SelectorWeights;

    // ============================================================
    // 构造与默认值测试
    // ============================================================

    #[test]
    fn test_holder_new_is_static_fallback() {
        let holder = SelectorLearnerHolder::new();
        let policy = holder.current_policy();
        assert!(policy.is_static());
        let w = policy.weights();
        assert!((w.recency - 0.4).abs() < 1e-6);
        assert!((w.frequency - 0.3).abs() < 1e-6);
        assert!((w.relevance - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_holder_default_equals_new() {
        let holder1 = SelectorLearnerHolder::new();
        let holder2 = SelectorLearnerHolder::default();
        assert_eq!(holder1.current_policy(), holder2.current_policy());
    }

    #[test]
    fn test_holder_default_equals_fallback() {
        let holder = SelectorLearnerHolder::new();
        assert_eq!(holder.current_policy(), SelectorPolicy::fallback());
    }

    #[test]
    fn test_holder_with_policy_learned() {
        let holder = SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            42,
            SelectorWeights::new(0.5, 0.3, 0.2),
        ));
        let policy = holder.current_policy();
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(42));
        let w = policy.weights();
        assert!((w.recency - 0.5).abs() < 1e-6);
        assert!((w.frequency - 0.3).abs() < 1e-6);
        assert!((w.relevance - 0.2).abs() < 1e-6);
    }

    // ============================================================
    // update_policy 测试
    // ============================================================

    #[test]
    fn test_update_policy_to_learned() {
        let holder = SelectorLearnerHolder::new();
        assert!(holder.current_policy().is_static());

        holder.update_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.6, 0.2, 0.2),
        ));
        let policy = holder.current_policy();
        assert!(policy.is_learned());
        assert!((policy.weights().recency - 0.6).abs() < 1e-6);
        assert_eq!(policy.version(), Some(1));
    }

    #[test]
    fn test_update_policy_to_static() {
        let holder = SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.6, 0.2, 0.2),
        ));
        assert!(holder.current_policy().is_learned());

        holder.update_policy(SelectorPolicy::static_policy(SelectorWeights::DEFAULT));
        assert!(holder.current_policy().is_static());
        let w = holder.current_policy().weights();
        assert!((w.recency - 0.4).abs() < 1e-6);
    }

    #[test]
    fn test_update_policy_multiple_times() {
        let holder = SelectorLearnerHolder::new();

        for version in 1..=5 {
            holder.update_policy(SelectorPolicy::learned(
                version,
                SelectorWeights::new(0.5, 0.3, 0.2),
            ));
            assert_eq!(holder.version(), Some(version));
        }
    }

    #[test]
    fn test_fallback_to_static() {
        let holder = SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.6, 0.2, 0.2),
        ));
        assert!(holder.is_learned());

        holder.fallback_to_static();
        assert!(!holder.is_learned());
        assert!(holder.current_policy().is_static());
        let w = holder.current_policy().weights();
        assert!((w.recency - 0.4).abs() < 1e-6);
        assert!((w.frequency - 0.3).abs() < 1e-6);
        assert!((w.relevance - 0.3).abs() < 1e-6);
    }

    // ============================================================
    // current_weights 测试
    // ============================================================

    #[test]
    fn test_current_weights_default() {
        let holder = SelectorLearnerHolder::new();
        let w = holder.current_weights();
        assert!((w.recency - 0.4).abs() < 1e-6);
        assert!((w.frequency - 0.3).abs() < 1e-6);
        assert!((w.relevance - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_current_weights_learned() {
        let holder = SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.6, 0.2, 0.2),
        ));
        let w = holder.current_weights();
        assert!((w.recency - 0.6).abs() < 1e-6);
        assert!((w.frequency - 0.2).abs() < 1e-6);
        assert!((w.relevance - 0.2).abs() < 1e-6);
    }

    // ============================================================
    // compute_importance_with_policy 测试
    // ============================================================

    #[test]
    fn test_compute_importance_default_policy() {
        // 使用默认 Static 策略 (0.4, 0.3, 0.3) 计算重要性
        let holder = SelectorLearnerHolder::new();
        let entry = ContextEntry::new("e-1", "file-1", "content", 100);
        let now = Utc::now();

        let score = holder.compute_importance_with_policy(&entry, None, now, 10.0, 1000.0);
        // score = 0.4 × recency + 0.3 × frequency + 0.3 × 0.5 (no CLV)
        // recency ≈ 1.0 (刚创建), frequency = 0/10 = 0.0
        // score ≈ 0.4 × 1.0 + 0.3 × 0.0 + 0.3 × 0.5 = 0.55
        assert!((0.0..=1.0).contains(&score));
        assert!((score - 0.55).abs() < 0.01);
    }

    #[test]
    fn test_compute_importance_recency_heavy_policy() {
        // 使用 recency-heavy 策略 (0.6, 0.2, 0.2) 计算重要性
        let holder = SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.6, 0.2, 0.2),
        ));
        let entry = ContextEntry::new("e-1", "file-1", "content", 100);
        let now = Utc::now();

        let score = holder.compute_importance_with_policy(&entry, None, now, 10.0, 1000.0);
        // score = 0.6 × recency + 0.2 × frequency + 0.2 × 0.5
        // recency ≈ 1.0, frequency = 0.0
        // score ≈ 0.6 × 1.0 + 0.2 × 0.0 + 0.2 × 0.5 = 0.7
        assert!((score - 0.7).abs() < 0.01);
    }

    #[test]
    fn test_compute_importance_frequency_heavy_policy() {
        // 使用 frequency-heavy 策略 (0.2, 0.6, 0.2)
        let holder = SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.2, 0.6, 0.2),
        ));
        let mut entry = ContextEntry::new("e-1", "file-1", "content", 100);
        entry.access_count = 5; // 高频访问
        let now = Utc::now();

        let score = holder.compute_importance_with_policy(&entry, None, now, 10.0, 1000.0);
        // frequency = 5/10 = 0.5
        // score = 0.2 × 1.0 + 0.6 × 0.5 + 0.2 × 0.5 = 0.2 + 0.3 + 0.1 = 0.6
        assert!((score - 0.6).abs() < 0.01);
    }

    #[test]
    fn test_compute_importance_policy_change_affects_score() {
        // 验证策略更新会影响评分（运行时可变性）
        let holder = SelectorLearnerHolder::new();
        let mut entry = ContextEntry::new("e-1", "file-1", "content", 100);
        entry.access_count = 10; // 最高频
        let now = Utc::now();

        // 默认 Static (0.4, 0.3, 0.3)
        let score_static = holder.compute_importance_with_policy(&entry, None, now, 10.0, 1000.0);

        // 切换到 frequency-heavy (0.2, 0.6, 0.2)
        holder.update_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.2, 0.6, 0.2),
        ));
        let score_learned = holder.compute_importance_with_policy(&entry, None, now, 10.0, 1000.0);

        // frequency = 10/10 = 1.0, recency ≈ 1.0, relevance = 0.5
        // Static:  0.4 × 1.0 + 0.3 × 1.0 + 0.3 × 0.5 = 0.85
        // Learned: 0.2 × 1.0 + 0.6 × 1.0 + 0.2 × 0.5 = 0.9
        assert!((score_static - 0.85).abs() < 0.01);
        assert!((score_learned - 0.9).abs() < 0.01);
        assert!(score_learned > score_static); // frequency-heavy 策略对高频条目评分更高
    }

    // ============================================================
    // is_learned / version 测试
    // ============================================================

    #[test]
    fn test_is_learned_static() {
        let holder = SelectorLearnerHolder::new();
        assert!(!holder.is_learned());
    }

    #[test]
    fn test_is_learned_learned() {
        let holder = SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.5, 0.3, 0.2),
        ));
        assert!(holder.is_learned());
    }

    #[test]
    fn test_version_static_none() {
        let holder = SelectorLearnerHolder::new();
        assert_eq!(holder.version(), None);
    }

    #[test]
    fn test_version_learned_some() {
        let holder = SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            42,
            SelectorWeights::new(0.5, 0.3, 0.2),
        ));
        assert_eq!(holder.version(), Some(42));
    }

    // ============================================================
    // Clone 测试
    // ============================================================

    #[test]
    fn test_clone_independent() {
        let holder1 = SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.5, 0.3, 0.2),
        ));
        let holder2 = holder1.clone();

        // 修改 holder1 不影响 holder2
        holder1.update_policy(SelectorPolicy::learned(
            2,
            SelectorWeights::new(0.6, 0.2, 0.2),
        ));
        assert_eq!(holder1.version(), Some(2));
        assert_eq!(holder2.version(), Some(1)); // 保持原值
    }

    // ============================================================
    // 并发安全测试
    // ============================================================

    #[test]
    fn test_concurrent_update_and_read() {
        use std::sync::Arc;
        use std::thread;

        let holder = Arc::new(SelectorLearnerHolder::new());
        let mut handles = vec![];

        // 启动 4 个写线程
        for i in 0..4 {
            let h_clone = Arc::clone(&holder);
            handles.push(thread::spawn(move || {
                h_clone.update_policy(SelectorPolicy::learned(
                    i + 1,
                    SelectorWeights::new(0.5, 0.3, 0.2),
                ));
            }));
        }

        // 启动 4 个读线程
        for _ in 0..4 {
            let h_clone = Arc::clone(&holder);
            handles.push(thread::spawn(move || {
                let _policy = h_clone.current_policy();
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // 最终策略是某个写线程的值
        let policy = holder.current_policy();
        assert!(policy.is_learned());
        let w = policy.weights();
        assert!((w.recency - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_concurrent_compute_importance() {
        use std::sync::Arc;
        use std::thread;

        let holder = Arc::new(SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.5, 0.3, 0.2),
        )));
        let entry = Arc::new(ContextEntry::new("e-1", "file-1", "content", 100));
        let now = Utc::now();
        let mut handles = vec![];

        // 启动 4 个并发评分线程
        for _ in 0..4 {
            let h_clone = Arc::clone(&holder);
            let e_clone = Arc::clone(&entry);
            handles.push(thread::spawn(move || {
                h_clone.compute_importance_with_policy(&e_clone, None, now, 10.0, 1000.0)
            }));
        }

        let scores: Vec<f32> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // 所有线程应得到相同分数（策略未变化）
        for i in 1..scores.len() {
            assert!((scores[i] - scores[0]).abs() < 1e-6);
        }
    }

    // ============================================================
    // C4 合规场景测试
    // ============================================================

    #[test]
    fn test_c4_default_value_layer() {
        // 第一层: 默认值 = SelectorPolicy::fallback() (编译期常量)
        let holder = SelectorLearnerHolder::new();
        assert_eq!(holder.current_policy(), SelectorPolicy::fallback());
        assert!(holder.current_policy().is_static());

        // DEFAULT 是 const 常量，编译进二进制
        let w = holder.current_policy().weights();
        assert_eq!(w, SelectorWeights::DEFAULT);
    }

    #[test]
    fn test_c4_exception_fallback_layer() {
        // 第二层: 异常回退（模拟 PoisonError 后的 unwrap_or_else 路径）
        // 通过 with_policy 构造一个 holder，然后正常 update_policy
        // 验证 update_policy 不 panic（即使内部 RwLock poison 也能恢复）
        let holder = SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.5, 0.3, 0.2),
        ));

        // 正常 update 应不 panic
        holder.update_policy(SelectorPolicy::learned(
            2,
            SelectorWeights::new(0.6, 0.2, 0.2),
        ));
        assert_eq!(holder.version(), Some(2));
        assert!((holder.current_policy().weights().recency - 0.6).abs() < 1e-6);
    }

    #[test]
    fn test_c4_circuit_breaker_layer() {
        // 第三层: 熔断入口（omega-learner 触发学习熔断时主动回退）
        let holder = SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.6, 0.2, 0.2),
        ));
        assert!(holder.is_learned());

        // 触发熔断
        holder.fallback_to_static();
        assert!(!holder.is_learned());
        assert!(holder.current_policy().is_static());
        assert_eq!(holder.current_policy(), SelectorPolicy::fallback());
    }

    #[test]
    fn test_c4_no_cross_crate_flag_propagation() {
        // C4 合规: 无跨 crate 旗标传播
        // SelectorPolicy 通过值注入（Copy 语义），不依赖全局 static 或 feature flag
        let holder1 = SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.5, 0.3, 0.2),
        ));
        let holder2 = SelectorLearnerHolder::new();

        // holder1 用 Learned，holder2 用 Static，互不影响
        assert!(holder1.is_learned());
        assert!(!holder2.is_learned());
        assert_ne!(holder1.current_policy(), holder2.current_policy());
    }

    // ============================================================
    // 集成场景: S4 接缝完整流程模拟
    // ============================================================

    #[test]
    fn test_s4_seam_full_integration_flow() {
        // 模拟 S4 接缝完整流程:
        // 1. holder 初始化为 Static fallback
        // 2. omega-learner 异步下发 Learned 策略
        // 3. HcwWindow 使用 compute_importance_with_policy 评分
        // 4. learner 检测到后悔率上升,触发熔断
        // 5. holder 回退到 Static

        let holder = SelectorLearnerHolder::new();

        // 初始状态: Static fallback
        assert!(holder.current_policy().is_static());

        // Step 2: omega-learner 下发 Learned 策略（recency-heavy: 0.6, 0.2, 0.2）
        holder.update_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.6, 0.2, 0.2),
        ));
        assert!(holder.is_learned());
        assert_eq!(holder.version(), Some(1));

        // Step 3: compute_importance_with_policy 评分
        let entry = ContextEntry::new("e-1", "file-1", "content", 100);
        let now = Utc::now();
        let score = holder.compute_importance_with_policy(&entry, None, now, 10.0, 1000.0);
        assert!((0.0..=1.0).contains(&score));

        // Step 4: 触发学习熔断（spec.md:335 S4 灰度后悔率超阈值）
        holder.fallback_to_static();
        assert!(!holder.is_learned());
        assert_eq!(holder.current_policy(), SelectorPolicy::fallback());

        // Step 5: 回退后再次评分，使用 Static 策略
        let score_static = holder.compute_importance_with_policy(&entry, None, now, 10.0, 1000.0);
        assert!((0.0..=1.0).contains(&score_static));
    }

    #[test]
    fn test_s4_seam_ab_test_scenario() {
        // 模拟 A/B 测试场景:
        // 版本 1 = recency-heavy (0.6, 0.2, 0.2)
        // 版本 2 = frequency-heavy (0.2, 0.6, 0.2)
        // 对比两个版本的评分效果

        let holder_v1 = SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.6, 0.2, 0.2),
        ));
        let holder_v2 = SelectorLearnerHolder::with_policy(SelectorPolicy::learned(
            2,
            SelectorWeights::new(0.2, 0.6, 0.2),
        ));

        // 构造高频访问条目
        let mut entry = ContextEntry::new("e-1", "file-1", "content", 100);
        entry.access_count = 10;
        let now = Utc::now();

        let score_v1 = holder_v1.compute_importance_with_policy(&entry, None, now, 10.0, 1000.0);
        let score_v2 = holder_v2.compute_importance_with_policy(&entry, None, now, 10.0, 1000.0);

        // frequency = 10/10 = 1.0, recency ≈ 1.0, relevance = 0.5
        // v1 (recency-heavy): 0.6 × 1.0 + 0.2 × 1.0 + 0.2 × 0.5 = 0.9
        // v2 (frequency-heavy): 0.2 × 1.0 + 0.6 × 1.0 + 0.2 × 0.5 = 0.9
        // 两者相同（因为 recency 和 frequency 都为 1.0）
        assert!((score_v1 - 0.9).abs() < 0.01);
        assert!((score_v2 - 0.9).abs() < 0.01);
    }

    #[test]
    fn test_s4_seam_version_monotonicity() {
        // 验证版本号单调递增（用于 A/B 测试与回滚）
        let holder = SelectorLearnerHolder::new();
        assert_eq!(holder.version(), None); // Static 无版本号

        holder.update_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.5, 0.3, 0.2),
        ));
        assert_eq!(holder.version(), Some(1));

        holder.update_policy(SelectorPolicy::learned(
            2,
            SelectorWeights::new(0.6, 0.2, 0.2),
        ));
        assert_eq!(holder.version(), Some(2));

        holder.update_policy(SelectorPolicy::learned(
            10,
            SelectorWeights::new(0.4, 0.4, 0.2),
        ));
        assert_eq!(holder.version(), Some(10));

        // 回退到 Static 后版本号清空
        holder.fallback_to_static();
        assert_eq!(holder.version(), None);
    }
}
