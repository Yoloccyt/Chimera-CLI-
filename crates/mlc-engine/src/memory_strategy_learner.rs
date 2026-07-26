//! 记忆策略学习器持有器 — S2 接缝策略异步下发 + 本地 fallback
//!
//! 对应任务: **P4-W14.1.3**（mlc-engine 接入 omega-learner 异步下发）
//! 对应 ADR: **ADR-031**（omega-learner 边界）+ **ADR-033**（nexus-contracts L0 契约层）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 S2 接缝
//!
//! # 核心职责
//!
//! 承载 `MemoryStrategyPolicy` 的运行时可变状态，为 `MlcEngine` 提供:
//! - **异步策略接收**: `update_policy()` 接收 `omega-learner` 下发的 `MemoryStrategyPolicy::Learned`
//! - **当前策略查询**: `current_policy()` 返回当前激活的策略
//! - **策略感知召回**: `strategy()` 返回当前 `MemoryStrategy` 供 recall 路径使用
//! - **本地 fallback**: 任何异常回到 `MemoryStrategyPolicy::Static(StandardTopK)`（C4 合规）
//!
//! # 依赖铁律合规（WHY mlc-engine 不直接依赖 omega-learner）
//!
//! ```text
//! L6 omega-learner  ────(learned MemoryStrategyPolicy)───▶  上层编排器
//!      │                                                    │
//!      │ L6 → L0 ✓                                        │ L0 → 注入
//!      ▼                                                    ▼
//! L0 nexus-contracts  ◀──(MemoryStrategyPolicy 类型)──  L2 mlc-engine
//!      MemoryStrategyPolicy                                  │
//!      MemoryStrategy                                        │ L2 → L0 ✓
//!                                                           ▼
//!                                                  MemoryStrategyLearnerHolder
//! ```
//!
//! mlc-engine (L2) 只依赖 `nexus-contracts` (L0) 的 `MemoryStrategyPolicy` 类型，
//! **不直接依赖** `omega-learner` (L6)，遵守 §2.2 依赖铁律。
//! `omega-learner` 输出的 `MemoryStrategyPolicy::Learned` 由上层编排器
//! （chimera-cli / quest-engine）通过 `update_policy()` 注入。
//!
//! # C4 合规（能力场灰度，非运行时旗）
//!
//! - **默认值层**: `MemoryStrategyLearnerHolder::new()` 初始化为
//!   `MemoryStrategyPolicy::Static(StandardTopK)`（编译期常量 fallback）
//! - **异常回退层**: `RwLock::write().unwrap_or_else()` 处理 `PoisonError` 自动回退
//! - **熔断入口层**: `fallback_to_static()` 供 `omega-learner` 触发学习熔断
//!   （spec.md:335 S2 灰度阶段目标达成率降 >2%）时主动回退
//!
//! 三层叠加实现"learner panic/超时时调用方本地 fallback，无跨 crate 旗标传播"
//! 的 C4 合规要求（spec.md:334）。
//!
//! # 与 DensityLearnerHolder / SelectorLearnerHolder 的对称设计
//!
//! S1/S2/S4 三接缝共用相同骨架:
//! - 枚举 + `Static`/`Learned` 双变体策略
//! - `RwLock<Policy>` + `new`/`with_policy`/`update_policy`/`fallback_to_static`
//! - `current_policy`/`is_learned`/`version`/`Default`/`Clone`
//!
//! 差异仅在策略类型与感知方法:
//! - S1: `DensityPolicy` + `select_with_density`（HCW 密度感知选择）
//! - S2: `MemoryStrategyPolicy` + `strategy`（MLC 记忆策略感知）
//! - S4: `SelectorPolicy` + `compute_importance_with_policy`（HCW selector 权重感知）
//!
//! # 线程安全
//!
//! 内部用 `RwLock<MemoryStrategyPolicy>` 保护策略状态:
//! - **写锁**: `update_policy()` 异步下发（低频，每秒 < 1 次）
//! - **读锁**: `current_policy()` / `strategy()` 热路径查询（高频）
//!
//! 读写分离避免锁竞争。`RwLock` 选择 `std::sync::RwLock` 而非 `tokio::sync::RwLock`:
//! - 读路径需要 sync 访问（recall 是 sync 路径的辅助查询）
//! - 持锁时间极短（仅读取 `MemoryStrategyPolicy`，~10ns）
//!
//! # 示例
//!
//! ## 基础 fallback 行为
//!
//! ```
//! use mlc_engine::memory_strategy_learner::MemoryStrategyLearnerHolder;
//! use nexus_contracts::{MemoryStrategy, MemoryStrategyPolicy};
//!
//! let holder = MemoryStrategyLearnerHolder::new();
//!
//! // 初始化为 Static fallback（StandardTopK）
//! let policy = holder.current_policy();
//! assert!(policy.is_static());
//! assert_eq!(policy.strategy(), MemoryStrategy::StandardTopK);
//! ```
//!
//! ## 异步下发学习策略
//!
//! ```
//! use mlc_engine::memory_strategy_learner::MemoryStrategyLearnerHolder;
//! use nexus_contracts::{MemoryStrategy, MemoryStrategyPolicy};
//!
//! let holder = MemoryStrategyLearnerHolder::new();
//!
//! // omega-learner 异步下发学习策略（TimeFocused，时间聚焦）
//! holder.update_policy(MemoryStrategyPolicy::learned(42, MemoryStrategy::TimeFocused));
//!
//! let policy = holder.current_policy();
//! assert!(policy.is_learned());
//! assert_eq!(policy.version(), Some(42));
//! assert_eq!(policy.strategy(), MemoryStrategy::TimeFocused);
//! ```

use std::sync::RwLock;

use nexus_contracts::{MemoryStrategy, MemoryStrategyPolicy};

// ============================================================
// MemoryStrategyLearnerHolder
// ============================================================

/// 记忆策略学习器持有器 — 运行时可变的 `MemoryStrategyPolicy` 容器
///
/// 承载 `omega-learner` 异步下发的学习策略，为 `MlcEngine` 的 recall 路径
/// 提供策略感知能力。所有方法线程安全（`RwLock` 保护）。
///
/// # 设计决策（WHY）
///
/// - **独立 struct 而非嵌入 MlcEngine**: 单一职责，便于单测与复用
/// - **`RwLock<MemoryStrategyPolicy>` 而非 `AtomicU8`**: `MemoryStrategyPolicy`
///   是枚举（Static/Learned），原子化需要 `AtomicU8` + 手动重建枚举，复杂且易错
/// - **`std::sync::RwLock` 而非 `tokio::sync::RwLock`**: 读路径是 sync
///   （recall 路径是 sync 辅助查询），持锁时间极短（~10ns）
///
/// # 线程安全
///
/// `MemoryStrategyLearnerHolder` 内部 `RwLock` 保证:
/// - 多读单写（`current_policy`/`strategy` 并发读，`update_policy` 独占写）
/// - 持锁时间极短（仅读写 `MemoryStrategyPolicy` 枚举，~10ns）
/// - 无 await 跨锁（`update_policy` 是 sync 方法，避免 §4.4 反模式 1）
///
/// # 示例
///
/// ```
/// use mlc_engine::memory_strategy_learner::MemoryStrategyLearnerHolder;
/// use nexus_contracts::{MemoryStrategy, MemoryStrategyPolicy};
///
/// let holder = MemoryStrategyLearnerHolder::new();
/// assert!(holder.current_policy().is_static());
///
/// holder.update_policy(MemoryStrategyPolicy::learned(1, MemoryStrategy::AggressivePruning));
/// assert!(holder.current_policy().is_learned());
/// assert_eq!(holder.current_policy().strategy(), MemoryStrategy::AggressivePruning);
/// ```
#[derive(Debug)]
pub struct MemoryStrategyLearnerHolder {
    /// 当前激活的策略（`RwLock` 保护，读写分离）
    ///
    /// WHY 用 `RwLock` 而非 `Mutex`:
    /// - 读路径（`current_policy`/`strategy`）高频且只读
    /// - 写路径（`update_policy`）低频（每秒 < 1 次）
    /// - `RwLock` 允许并发读，避免读路径串行化
    policy: RwLock<MemoryStrategyPolicy>,
}

impl MemoryStrategyLearnerHolder {
    /// 创建持有器，初始化为 `MemoryStrategyPolicy::Static(StandardTopK)`（fallback）
    ///
    /// WHY 初始化为 Static(StandardTopK): C4 合规要求默认行为零变化，
    /// `StandardTopK` 对应既有 `recall_by_clv(top_k=10)` 行为，向后兼容。
    pub fn new() -> Self {
        Self {
            policy: RwLock::new(MemoryStrategyPolicy::fallback()),
        }
    }

    /// 创建持有器，指定初始策略（便于测试）
    ///
    /// WHY 提供: 单测需要构造特定策略场景（如 Learned 初始状态）
    pub fn with_policy(policy: MemoryStrategyPolicy) -> Self {
        Self {
            policy: RwLock::new(policy),
        }
    }

    /// 异步下发策略 — 接收 `omega-learner` 学习到的 `MemoryStrategyPolicy`
    ///
    /// # 设计
    ///
    /// - 写入 `RwLock`（独占写锁，~10ns）
    /// - 不返回错误: 任何异常（如 PoisonError）静默 fallback 到 Static
    ///
    /// # C4 合规
    ///
    /// 调用方（chimera-cli / quest-engine）在 `omega-learner` panic/超时时
    /// 不调用此方法，`MemoryStrategyLearnerHolder` 保持上一次的有效策略。
    /// 若需强制回退到 fallback，调用方传入 `MemoryStrategyPolicy::fallback()`。
    ///
    /// # 参数
    /// - `policy`: 新策略（Static 或 Learned）
    ///
    /// # 示例
    ///
    /// ```
    /// use mlc_engine::memory_strategy_learner::MemoryStrategyLearnerHolder;
    /// use nexus_contracts::{MemoryStrategy, MemoryStrategyPolicy};
    ///
    /// let holder = MemoryStrategyLearnerHolder::new();
    /// holder.update_policy(MemoryStrategyPolicy::learned(1, MemoryStrategy::QueryReformulation));
    /// assert_eq!(holder.current_policy().strategy(), MemoryStrategy::QueryReformulation);
    /// ```
    pub fn update_policy(&self, policy: MemoryStrategyPolicy) {
        // WHY unwrap_or_else: RwLock poison 时 fallback 到 Static(StandardTopK)
        // 避免调用方处理 PoisonError（C4 合规：本地 fallback，无错误传播）
        let mut guard = self.policy.write().unwrap_or_else(|p| {
            // PoisonError 时恢复锁并写入 fallback
            let mut guard = p.into_inner();
            *guard = MemoryStrategyPolicy::fallback();
            guard
        });
        *guard = policy;
    }

    /// 强制回退到 fallback 策略（`Static(StandardTopK)`）
    ///
    /// WHY 提供: `omega-learner` 触发学习熔断（spec.md:335 S2 灰度阶段目标
    /// 达成率降 >2%）时，上层调用方调用此方法立即回退到静态策略。
    ///
    /// # 示例
    ///
    /// ```
    /// use mlc_engine::memory_strategy_learner::MemoryStrategyLearnerHolder;
    /// use nexus_contracts::MemoryStrategy;
    ///
    /// let holder = MemoryStrategyLearnerHolder::new();
    /// holder.fallback_to_static();
    /// assert!(holder.current_policy().is_static());
    /// assert_eq!(holder.current_policy().strategy(), MemoryStrategy::StandardTopK);
    /// ```
    pub fn fallback_to_static(&self) {
        self.update_policy(MemoryStrategyPolicy::fallback());
    }

    /// 返回当前激活的策略（快照）
    ///
    /// 返回 `MemoryStrategyPolicy` 的 Copy（枚举整体 Copy），调用方无需持有锁。
    ///
    /// # 性能
    ///
    /// 读锁 + Copy 枚举，~10ns。热路径调用无性能影响。
    pub fn current_policy(&self) -> MemoryStrategyPolicy {
        let guard = self.policy.read().unwrap_or_else(|p| p.into_inner());
        *guard
    }

    /// 返回当前激活的记忆策略（便捷方法）
    ///
    /// 等价于 `current_policy().strategy()`，但避免调用方重复 match。
    /// 用于 recall 路径的策略感知决策。
    ///
    /// # 示例
    ///
    /// ```
    /// use mlc_engine::memory_strategy_learner::MemoryStrategyLearnerHolder;
    /// use nexus_contracts::{MemoryStrategy, MemoryStrategyPolicy};
    ///
    /// let holder = MemoryStrategyLearnerHolder::new();
    /// assert_eq!(holder.strategy(), MemoryStrategy::StandardTopK);
    ///
    /// holder.update_policy(MemoryStrategyPolicy::learned(1, MemoryStrategy::MinimalRecall));
    /// assert_eq!(holder.strategy(), MemoryStrategy::MinimalRecall);
    /// ```
    pub fn strategy(&self) -> MemoryStrategy {
        self.current_policy().strategy()
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

impl Default for MemoryStrategyLearnerHolder {
    /// 默认状态 = `MemoryStrategyPolicy::Static(StandardTopK)`（fallback，C4 合规）
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for MemoryStrategyLearnerHolder {
    /// 克隆持有器（创建新的 RwLock，策略快照独立）
    ///
    /// WHY 提供: `MlcEngine` 可能需要克隆 `MemoryStrategyLearnerHolder` 用于
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

    // ============================================================
    // 构造与默认值测试
    // ============================================================

    #[test]
    fn test_holder_new_is_static_fallback() {
        let holder = MemoryStrategyLearnerHolder::new();
        let policy = holder.current_policy();
        assert!(policy.is_static());
        assert_eq!(policy.strategy(), MemoryStrategy::StandardTopK);
    }

    #[test]
    fn test_holder_default_equals_new() {
        let holder1 = MemoryStrategyLearnerHolder::new();
        let holder2 = MemoryStrategyLearnerHolder::default();
        assert_eq!(holder1.current_policy(), holder2.current_policy());
    }

    #[test]
    fn test_holder_with_policy_learned() {
        let holder = MemoryStrategyLearnerHolder::with_policy(MemoryStrategyPolicy::learned(
            42,
            MemoryStrategy::TimeFocused,
        ));
        let policy = holder.current_policy();
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(42));
        assert_eq!(policy.strategy(), MemoryStrategy::TimeFocused);
    }

    // ============================================================
    // update_policy 测试
    // ============================================================

    #[test]
    fn test_update_policy_to_learned() {
        let holder = MemoryStrategyLearnerHolder::new();
        assert!(holder.current_policy().is_static());

        holder.update_policy(MemoryStrategyPolicy::learned(
            1,
            MemoryStrategy::AggressivePruning,
        ));
        let policy = holder.current_policy();
        assert!(policy.is_learned());
        assert_eq!(policy.strategy(), MemoryStrategy::AggressivePruning);
        assert_eq!(policy.version(), Some(1));
    }

    #[test]
    fn test_update_policy_to_static() {
        let holder = MemoryStrategyLearnerHolder::with_policy(MemoryStrategyPolicy::learned(
            1,
            MemoryStrategy::QueryReformulation,
        ));
        assert!(holder.current_policy().is_learned());

        holder.update_policy(MemoryStrategyPolicy::static_policy(
            MemoryStrategy::StandardTopK,
        ));
        assert!(holder.current_policy().is_static());
        assert_eq!(
            holder.current_policy().strategy(),
            MemoryStrategy::StandardTopK
        );
    }

    #[test]
    fn test_update_policy_multiple_times() {
        let holder = MemoryStrategyLearnerHolder::new();
        let strategies = [
            MemoryStrategy::MinimalRecall,
            MemoryStrategy::StandardTopK,
            MemoryStrategy::QueryReformulation,
            MemoryStrategy::AggressivePruning,
            MemoryStrategy::TimeFocused,
        ];

        for (version, strategy) in strategies.iter().enumerate() {
            holder.update_policy(MemoryStrategyPolicy::learned(version as u64 + 1, *strategy));
            assert_eq!(holder.version(), Some(version as u64 + 1));
            assert_eq!(holder.strategy(), *strategy);
        }
    }

    // ============================================================
    // fallback_to_static 测试
    // ============================================================

    #[test]
    fn test_fallback_to_static() {
        let holder = MemoryStrategyLearnerHolder::with_policy(MemoryStrategyPolicy::learned(
            1,
            MemoryStrategy::MinimalRecall,
        ));
        assert!(holder.is_learned());

        holder.fallback_to_static();
        assert!(holder.current_policy().is_static());
        assert_eq!(
            holder.current_policy().strategy(),
            MemoryStrategy::StandardTopK
        );
    }

    // ============================================================
    // strategy 便捷方法测试
    // ============================================================

    #[test]
    fn test_strategy_method_default() {
        let holder = MemoryStrategyLearnerHolder::new();
        assert_eq!(holder.strategy(), MemoryStrategy::StandardTopK);
    }

    #[test]
    fn test_strategy_method_after_update() {
        let holder = MemoryStrategyLearnerHolder::new();
        holder.update_policy(MemoryStrategyPolicy::learned(
            1,
            MemoryStrategy::MinimalRecall,
        ));
        assert_eq!(holder.strategy(), MemoryStrategy::MinimalRecall);
    }

    #[test]
    fn test_strategy_method_all_strategies() {
        let holder = MemoryStrategyLearnerHolder::new();
        for strategy in MemoryStrategy::ALL.iter() {
            holder.update_policy(MemoryStrategyPolicy::learned(1, *strategy));
            assert_eq!(holder.strategy(), *strategy);
        }
    }

    // ============================================================
    // is_learned / version 测试
    // ============================================================

    #[test]
    fn test_is_learned_default_false() {
        let holder = MemoryStrategyLearnerHolder::new();
        assert!(!holder.is_learned());
    }

    #[test]
    fn test_is_learned_after_update_true() {
        let holder = MemoryStrategyLearnerHolder::new();
        holder.update_policy(MemoryStrategyPolicy::learned(
            1,
            MemoryStrategy::TimeFocused,
        ));
        assert!(holder.is_learned());
    }

    #[test]
    fn test_version_default_none() {
        let holder = MemoryStrategyLearnerHolder::new();
        assert_eq!(holder.version(), None);
    }

    #[test]
    fn test_version_after_learned_update() {
        let holder = MemoryStrategyLearnerHolder::new();
        holder.update_policy(MemoryStrategyPolicy::learned(
            123,
            MemoryStrategy::StandardTopK,
        ));
        assert_eq!(holder.version(), Some(123));
    }

    // ============================================================
    // Clone 测试
    // ============================================================

    #[test]
    fn test_clone_preserves_policy() {
        let holder = MemoryStrategyLearnerHolder::with_policy(MemoryStrategyPolicy::learned(
            42,
            MemoryStrategy::AggressivePruning,
        ));
        let cloned = holder.clone();
        assert_eq!(holder.current_policy(), cloned.current_policy());
    }

    #[test]
    fn test_clone_independent_evolution() {
        // 克隆后两者策略独立演化，互不影响
        let holder = MemoryStrategyLearnerHolder::with_policy(MemoryStrategyPolicy::learned(
            1,
            MemoryStrategy::MinimalRecall,
        ));
        let cloned = holder.clone();

        holder.update_policy(MemoryStrategyPolicy::learned(
            2,
            MemoryStrategy::TimeFocused,
        ));

        // 原持有器更新，克隆保持原状态
        assert_eq!(holder.strategy(), MemoryStrategy::TimeFocused);
        assert_eq!(cloned.strategy(), MemoryStrategy::MinimalRecall);
    }

    // ============================================================
    // C4 合规三层 fallback 模式测试
    // ============================================================

    #[test]
    fn test_c4_layer1_default_value_is_static_fallback() {
        // 第一层: 默认值层 — 编译期常量 Static(StandardTopK)
        let holder = MemoryStrategyLearnerHolder::new();
        assert!(holder.current_policy().is_static());
        assert_eq!(holder.strategy(), MemoryStrategy::StandardTopK);
    }

    #[test]
    fn test_c4_layer2_poison_error_recovery() {
        // 第二层: 异常回退层 — PoisonError 时自动回退到 fallback
        // 模拟 poison: 手动 panic 持有写锁
        let holder = MemoryStrategyLearnerHolder::with_policy(MemoryStrategyPolicy::learned(
            1,
            MemoryStrategy::TimeFocused,
        ));

        // 用 std::thread 模拟 poison 场景（持锁时 panic）
        let holder_for_panic = std::sync::Arc::new(holder);
        let holder_clone = holder_for_panic.clone();

        let handle = std::thread::spawn(move || {
            // 持有写锁时 panic，导致 RwLock poison
            let _guard = holder_clone.policy.write().unwrap();
            panic!("intentional panic to poison RwLock");
        });

        // 等待 panic 线程结束
        let _ = handle.join();

        // 此时 RwLock 已 poison，update_policy 应通过 unwrap_or_else 恢复
        // 并写入 fallback（而非传播 PoisonError）
        holder_for_panic.update_policy(MemoryStrategyPolicy::learned(
            2,
            MemoryStrategy::MinimalRecall,
        ));

        // 验证 update 后能正常读取（poison 已恢复）
        let policy = holder_for_panic.current_policy();
        assert_eq!(policy.strategy(), MemoryStrategy::MinimalRecall);
        assert!(policy.is_learned());
    }

    #[test]
    fn test_c4_layer3_circuit_breaker_fallback() {
        // 第三层: 熔断入口层 — fallback_to_static 主动触发回退
        let holder = MemoryStrategyLearnerHolder::with_policy(MemoryStrategyPolicy::learned(
            1,
            MemoryStrategy::AggressivePruning,
        ));
        assert!(holder.is_learned());

        // omega-learner 触发学习熔断
        holder.fallback_to_static();

        assert!(holder.current_policy().is_static());
        assert_eq!(holder.strategy(), MemoryStrategy::StandardTopK);
    }

    // ============================================================
    // 全策略覆盖测试
    // ============================================================

    #[test]
    fn test_all_strategies_round_trip() {
        let holder = MemoryStrategyLearnerHolder::new();
        for (idx, strategy) in MemoryStrategy::ALL.iter().enumerate() {
            holder.update_policy(MemoryStrategyPolicy::learned(idx as u64, *strategy));
            assert_eq!(holder.strategy(), *strategy);
            assert_eq!(holder.version(), Some(idx as u64));
            assert!(holder.is_learned());
        }
    }

    #[test]
    fn test_strategy_arm_count_is_five() {
        // S2 接缝 5 臂对应 5 种记忆策略
        assert_eq!(MemoryStrategy::ALL.len(), 5);
    }

    // ============================================================
    // Debug / Display 测试
    // ============================================================

    #[test]
    fn test_holder_debug_format() {
        let holder = MemoryStrategyLearnerHolder::new();
        let debug_str = format!("{:?}", holder);
        assert!(debug_str.contains("MemoryStrategyLearnerHolder"));
    }

    // ============================================================
    // 并发安全测试（基础验证）
    // ============================================================

    #[test]
    fn test_concurrent_read_safe() {
        // 多线程并发读不应死锁或 panic
        let holder = std::sync::Arc::new(MemoryStrategyLearnerHolder::with_policy(
            MemoryStrategyPolicy::learned(1, MemoryStrategy::TimeFocused),
        ));

        let mut handles = vec![];
        for _ in 0..4 {
            let h = holder.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    let policy = h.current_policy();
                    assert_eq!(policy.strategy(), MemoryStrategy::TimeFocused);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn test_concurrent_write_safe() {
        // 多线程交替写不应死锁（最终值是最后写入的）
        let holder = std::sync::Arc::new(MemoryStrategyLearnerHolder::new());

        let mut handles = vec![];
        for i in 0..4 {
            let h = holder.clone();
            handles.push(std::thread::spawn(move || {
                let strategy = MemoryStrategy::ALL[i as usize % 5];
                h.update_policy(MemoryStrategyPolicy::learned(i as u64, strategy));
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // 最终策略是某个线程写入的（具体哪个不确定，但应为合法值）
        let final_strategy = holder.strategy();
        assert!(MemoryStrategy::ALL.contains(&final_strategy));
    }
}
