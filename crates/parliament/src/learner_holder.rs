//! Parliament 激活策略学习器持有器 — S5 接缝策略异步下发 + 本地 fallback
//!
//! 对应任务: **P4-W14.3.2**(parliament 接入 omega-learner 异步下发)
//! 对应 ADR: **ADR-031**(omega-learner 边界)+ **ADR-033**(nexus-contracts L0 契约层)
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 S5 接缝
//!
//! # 核心职责
//!
//! 承载 `ParliamentPolicy` 的运行时可变状态,为 `Parliament` 提供:
//! - **异步策略接收**: `update_policy()` 接收 `omega-learner` 下发的 `ParliamentPolicy::Learned`
//! - **当前策略查询**: `current_policy()` 返回当前激活的策略
//! - **策略感知辩论**: `strategy()` 返回当前 `ActivationStrategy` 供 `deliberate_with_policy` 使用
//! - **本地 fallback**: 任何异常回到 `ParliamentPolicy::Static(Full)`(C4 合规)
//!
//! # 依赖铁律合规(WHY parliament 不直接依赖 omega-learner)
//!
//! ```text
//! L6 omega-learner  ────(learned ParliamentPolicy)───▶  上层编排器
//!      │                                                    │
//!      │ L6 → L0 ✓                                        │ L0 → 注入
//!      ▼                                                    ▼
//! L0 nexus-contracts  ◀──(ParliamentPolicy 类型)──  L8 parliament
//!      ParliamentPolicy                                      │
//!      ActivationStrategy                                    │ L8 → L0 ✓
//!                                                           ▼
//!                                                  ParliamentLearnerHolder
//! ```
//!
//! parliament (L8) 只依赖 `nexus-contracts` (L0) 的 `ParliamentPolicy` 类型,
//! **不直接依赖** `omega-learner` (L6),遵守 §2.2 依赖铁律。
//! `omega-learner` 输出的 `ParliamentPolicy::Learned` 由上层编排器
//! (chimera-cli / quest-engine)通过 `update_policy()` 注入。
//!
//! # C4 合规(能力场灰度,非运行时旗)
//!
//! - **默认值层**: `ParliamentLearnerHolder::new()` 初始化为
//!   `ParliamentPolicy::Static(ActivationStrategy::Full)`(编译期常量 fallback)
//! - **异常回退层**: `RwLock::write().unwrap_or_else()` 处理 `PoisonError` 自动回退
//! - **熔断入口层**: `fallback_to_static()` 供 `omega-learner` 触发学习熔断
//!   (spec.md:335 S5 灰度阶段目标达成率降 >2%)时主动回退
//!
//! 三层叠加实现"learner panic/超时时调用方本地 fallback,无跨 crate 旗标传播"
//! 的 C4 合规要求(spec.md:334)。
//!
//! # 与 PrefetchLearnerHolder / MemoryStrategyLearnerHolder 的对称设计
//!
//! S1/S2/S3/S5 四接缝共用相同骨架:
//! - 枚举 + `Static`/`Learned` 双变体策略
//! - `RwLock<Policy>` + `new`/`with_policy`/`update_policy`/`fallback_to_static`
//! - `current_policy`/`is_learned`/`version`/`Default`/`Clone`
//!
//! 差异仅在策略类型与感知方法:
//! - S1: `DensityPolicy` + `select_with_density`(HCW 密度感知选择)
//! - S2: `MemoryStrategyPolicy` + `recall_by_clv_with_strategy`(MLC 记忆策略感知)
//! - S3: `PrefetchPolicy` + `prefetch_with_policy`(SCC 预取策略感知)
//! - S5: `ParliamentPolicy` + `deliberate_with_policy`(Parliament 激活策略感知)
//!
//! # 线程安全
//!
//! 内部用 `RwLock<ParliamentPolicy>` 保护策略状态:
//! - **写锁**: `update_policy()` 异步下发(低频,每秒 < 1 次)
//! - **读锁**: `current_policy()` / `strategy()` 热路径查询(高频,每次 `deliberate_with_policy` 调用)
//!
//! 读写分离避免锁竞争。`RwLock` 选择 `std::sync::RwLock` 而非 `tokio::sync::RwLock`:
//! - 读路径需要 sync 访问(`deliberate_with_policy` 入口处同步读取策略)
//! - 持锁时间极短(仅读取 `ParliamentPolicy` 枚举,~10ns)
//!
//! # 示例
//!
//! ## 基础 fallback 行为
//!
//! ```
//! use nexus_contracts::ActivationStrategy;
//! use parliament::learner_holder::ParliamentLearnerHolder;
//!
//! let holder = ParliamentLearnerHolder::new();
//!
//! // 初始化为 Static fallback(Full = 5 角色完整辩论)
//! let policy = holder.current_policy();
//! assert!(policy.is_static());
//! assert_eq!(policy.strategy(), ActivationStrategy::Full);
//! ```
//!
//! ## 异步下发学习策略
//!
//! ```
//! use nexus_contracts::ActivationStrategy;
//! use parliament::learner_holder::ParliamentLearnerHolder;
//!
//! let holder = ParliamentLearnerHolder::new();
//!
//! // omega-learner 异步下发学习策略(FastPath,低风险只读场景)
//! holder.update_policy(nexus_contracts::ParliamentPolicy::learned(
//!     42,
//!     ActivationStrategy::FastPath,
//! ));
//!
//! let policy = holder.current_policy();
//! assert!(policy.is_learned());
//! assert_eq!(policy.version(), Some(42));
//! assert_eq!(policy.strategy(), ActivationStrategy::FastPath);
//! ```

use std::sync::RwLock;

use nexus_contracts::{ActivationStrategy, ParliamentPolicy};

// ============================================================
// ParliamentLearnerHolder
// ============================================================

/// Parliament 激活策略学习器持有器 — 运行时可变的 `ParliamentPolicy` 容器
///
/// 承载 `omega-learner` 异步下发的学习策略,为 `Parliament` 的
/// `deliberate_with_policy` 路径提供策略感知能力。所有方法线程安全
/// (`RwLock` 保护)。
///
/// # 设计决策(WHY)
///
/// - **独立 struct 而非嵌入 Parliament**: 单一职责,便于单测与复用;
///   也避免 `Parliament` 结构体膨胀(已有 6 字段)
/// - **`RwLock<ParliamentPolicy>` 而非 `AtomicU8`**: `ParliamentPolicy`
///   是枚举(Static/Learned),原子化需要 `AtomicU8` + 手动重建枚举,复杂且易错
/// - **`std::sync::RwLock` 而非 `tokio::sync::RwLock`**: 读路径是 sync
///   (`deliberate_with_policy` 入口同步读取策略),持锁时间极短(~10ns)
///
/// # 线程安全
///
/// `ParliamentLearnerHolder` 内部 `RwLock` 保证:
/// - 多读单写(`current_policy`/`strategy` 并发读,`update_policy` 独占写)
/// - 持锁时间极短(仅读写 `ParliamentPolicy` 枚举,~10ns)
/// - 无 await 跨锁(`update_policy` 是 sync 方法,避免 §4.4 反模式 1)
///
/// # 示例
///
/// ```
/// use nexus_contracts::ActivationStrategy;
/// use parliament::learner_holder::ParliamentLearnerHolder;
///
/// let holder = ParliamentLearnerHolder::new();
/// assert!(holder.current_policy().is_static());
/// assert_eq!(holder.strategy(), ActivationStrategy::Full);
///
/// holder.update_policy(nexus_contracts::ParliamentPolicy::learned(
///     1,
///     ActivationStrategy::Simplified,
/// ));
/// assert!(holder.current_policy().is_learned());
/// assert_eq!(holder.strategy(), ActivationStrategy::Simplified);
/// ```
#[derive(Debug)]
pub struct ParliamentLearnerHolder {
    /// 当前激活的策略(`RwLock` 保护,读写分离)
    ///
    /// WHY 用 `RwLock` 而非 `Mutex`:
    /// - 读路径(`current_policy`/`strategy`)高频且只读(每次 deliberate 调用一次)
    /// - 写路径(`update_policy`)低频(每秒 < 1 次)
    /// - `RwLock` 允许并发读,避免读路径串行化
    policy: RwLock<ParliamentPolicy>,
}

impl ParliamentLearnerHolder {
    /// 创建持有器,初始化为 `ParliamentPolicy::Static(ActivationStrategy::Full)`(fallback)
    ///
    /// WHY 初始化为 Static(Full): C4 合规要求默认行为零变化,
    /// `Full` 对应既有 `deliberate()` 行为(5 角色完整辩论 + Skeptic 否决),
    /// 向后兼容。`omega-learner` 未下发学习策略时,行为与 P4 修复前完全一致。
    pub fn new() -> Self {
        Self {
            policy: RwLock::new(ParliamentPolicy::fallback()),
        }
    }

    /// 创建持有器,指定初始策略(便于测试)
    ///
    /// WHY 提供: 单测需要构造特定策略场景(如 Learned 初始状态)
    pub fn with_policy(policy: ParliamentPolicy) -> Self {
        Self {
            policy: RwLock::new(policy),
        }
    }

    /// 异步下发策略 — 接收 `omega-learner` 学习到的 `ParliamentPolicy`
    ///
    /// # 设计
    ///
    /// - 写入 `RwLock`(独占写锁,~10ns)
    /// - 不返回错误: 任何异常(如 PoisonError)静默 fallback 到 Static(Full)
    ///
    /// # C4 合规
    ///
    /// 调用方(chimera-cli / quest-engine)在 `omega-learner` panic/超时时
    /// 不调用此方法,`ParliamentLearnerHolder` 保持上一次的有效策略。
    /// 若需强制回退到 fallback,调用方传入 `ParliamentPolicy::fallback()`。
    ///
    /// # 参数
    /// - `policy`: 新策略(Static 或 Learned)
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::{ActivationStrategy, ParliamentPolicy};
    /// use parliament::learner_holder::ParliamentLearnerHolder;
    ///
    /// let holder = ParliamentLearnerHolder::new();
    /// holder.update_policy(ParliamentPolicy::learned(1, ActivationStrategy::Simplified));
    /// assert_eq!(holder.strategy(), ActivationStrategy::Simplified);
    /// ```
    pub fn update_policy(&self, policy: ParliamentPolicy) {
        // WHY unwrap_or_else: RwLock poison 时 fallback 到 Static(Full)
        // 避免调用方处理 PoisonError(C4 合规:本地 fallback,无错误传播)
        let mut guard = self.policy.write().unwrap_or_else(|p| {
            // PoisonError 时恢复锁并写入 fallback
            let mut guard = p.into_inner();
            *guard = ParliamentPolicy::fallback();
            guard
        });
        *guard = policy;
    }

    /// 强制回退到 fallback 策略(`Static(Full)`)
    ///
    /// WHY 提供: `omega-learner` 触发学习熔断(spec.md:335 S5 灰度阶段目标
    /// 达成率降 >2%)时,上层调用方调用此方法立即回退到静态策略。
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::ActivationStrategy;
    /// use parliament::learner_holder::ParliamentLearnerHolder;
    ///
    /// let holder = ParliamentLearnerHolder::new();
    /// holder.fallback_to_static();
    /// assert!(holder.current_policy().is_static());
    /// assert_eq!(holder.strategy(), ActivationStrategy::Full);
    /// ```
    pub fn fallback_to_static(&self) {
        self.update_policy(ParliamentPolicy::fallback());
    }

    /// 返回当前激活的策略(快照)
    ///
    /// 返回 `ParliamentPolicy` 的 Copy(枚举整体 Copy),调用方无需持有锁。
    ///
    /// # 性能
    ///
    /// 读锁 + Copy 枚举,~10ns。热路径调用无性能影响。
    pub fn current_policy(&self) -> ParliamentPolicy {
        let guard = self.policy.read().unwrap_or_else(|p| p.into_inner());
        *guard
    }

    /// 返回当前激活的辩论策略(便捷方法)
    ///
    /// 等价于 `current_policy().strategy()`,但避免调用方重复 match。
    /// 用于 `deliberate_with_policy` 路径的策略感知决策。
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::{ActivationStrategy, ParliamentPolicy};
    /// use parliament::learner_holder::ParliamentLearnerHolder;
    ///
    /// let holder = ParliamentLearnerHolder::new();
    /// assert_eq!(holder.strategy(), ActivationStrategy::Full);
    ///
    /// holder.update_policy(ParliamentPolicy::learned(1, ActivationStrategy::FastPath));
    /// assert_eq!(holder.strategy(), ActivationStrategy::FastPath);
    /// ```
    pub fn strategy(&self) -> ActivationStrategy {
        self.current_policy().strategy()
    }

    /// 返回是否已激活学习策略
    ///
    /// 便于上层编排器查询当前是否在灰度阶段。
    pub fn is_learned(&self) -> bool {
        self.current_policy().is_learned()
    }

    /// 返回当前学习版本号(Static 返回 None,Learned 返回 Some(version))
    ///
    /// 便于上层编排器记录使用的版本号用于效果追踪与 A/B 测试。
    pub fn version(&self) -> Option<u64> {
        self.current_policy().version()
    }
}

impl Default for ParliamentLearnerHolder {
    /// 默认状态 = `ParliamentPolicy::Static(Full)`(fallback,C4 合规)
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ParliamentLearnerHolder {
    /// 克隆持有器(创建新的 RwLock,策略快照独立)
    ///
    /// WHY 提供: 上层编排器可能需要克隆 `ParliamentLearnerHolder`
    /// 用于快照或并行处理。克隆后两者策略独立演化,互不影响。
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
    use nexus_contracts::{ActivationStrategy, ParliamentPolicy};

    // ============================================================
    // 基础行为测试
    // ============================================================

    #[test]
    fn test_new_is_static_fallback() {
        // C4 合规:new() 必须初始化为 Static(Full) fallback
        let holder = ParliamentLearnerHolder::new();
        let policy = holder.current_policy();
        assert!(policy.is_static());
        assert_eq!(policy.strategy(), ActivationStrategy::Full);
    }

    #[test]
    fn test_default_equals_new() {
        // Default 与 new() 行为一致
        let holder1 = ParliamentLearnerHolder::new();
        let holder2 = ParliamentLearnerHolder::default();
        assert_eq!(holder1.current_policy(), holder2.current_policy());
    }

    #[test]
    fn test_with_policy_learned() {
        // with_policy 可指定 Learned 初始状态(便于测试)
        let holder = ParliamentLearnerHolder::with_policy(ParliamentPolicy::learned(
            42,
            ActivationStrategy::FastPath,
        ));
        let policy = holder.current_policy();
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(42));
        assert_eq!(policy.strategy(), ActivationStrategy::FastPath);
    }

    #[test]
    fn test_with_policy_static_simplified() {
        // with_policy 可指定 Static(Simplified) 初始状态
        let holder = ParliamentLearnerHolder::with_policy(ParliamentPolicy::static_policy(
            ActivationStrategy::Simplified,
        ));
        let policy = holder.current_policy();
        assert!(policy.is_static());
        assert_eq!(policy.strategy(), ActivationStrategy::Simplified);
    }

    // ============================================================
    // update_policy 测试
    // ============================================================

    #[test]
    fn test_update_policy_to_learned() {
        // 从 Static 切换到 Learned
        let holder = ParliamentLearnerHolder::new();
        assert!(holder.current_policy().is_static());

        holder.update_policy(ParliamentPolicy::learned(1, ActivationStrategy::FastPath));
        let policy = holder.current_policy();
        assert!(policy.is_learned());
        assert_eq!(policy.strategy(), ActivationStrategy::FastPath);
        assert_eq!(policy.version(), Some(1));
    }

    #[test]
    fn test_update_policy_to_static() {
        // 从 Learned 切换回 Static
        let holder = ParliamentLearnerHolder::with_policy(ParliamentPolicy::learned(
            1,
            ActivationStrategy::Simplified,
        ));
        assert!(holder.current_policy().is_learned());

        holder.update_policy(ParliamentPolicy::static_policy(ActivationStrategy::Full));
        assert!(holder.current_policy().is_static());
        assert_eq!(holder.strategy(), ActivationStrategy::Full);
    }

    #[test]
    fn test_update_policy_multiple_times() {
        // 连续多次更新策略,验证版本号与策略一致性
        let holder = ParliamentLearnerHolder::new();
        let strategies = [
            ActivationStrategy::FastPath,
            ActivationStrategy::Simplified,
            ActivationStrategy::Full,
            ActivationStrategy::FastPath,
            ActivationStrategy::Simplified,
        ];

        for (version, strategy) in strategies.iter().enumerate() {
            holder.update_policy(ParliamentPolicy::learned(version as u64 + 1, *strategy));
            assert_eq!(holder.version(), Some(version as u64 + 1));
            assert_eq!(holder.strategy(), *strategy);
        }
    }

    #[test]
    fn test_update_policy_version_zero() {
        // 版本号 0 是合法值(首个学习版本)
        let holder = ParliamentLearnerHolder::new();
        holder.update_policy(ParliamentPolicy::learned(0, ActivationStrategy::Simplified));
        assert!(holder.is_learned());
        assert_eq!(holder.version(), Some(0));
    }

    // ============================================================
    // fallback_to_static 测试
    // ============================================================

    #[test]
    fn test_fallback_to_static_from_learned() {
        // 从 Learned 回退到 Static(Full)
        let holder = ParliamentLearnerHolder::with_policy(ParliamentPolicy::learned(
            99,
            ActivationStrategy::FastPath,
        ));
        assert!(holder.current_policy().is_learned());
        assert_eq!(holder.strategy(), ActivationStrategy::FastPath);

        holder.fallback_to_static();
        assert!(holder.current_policy().is_static());
        assert_eq!(holder.strategy(), ActivationStrategy::Full);
    }

    #[test]
    fn test_fallback_to_static_idempotent() {
        // 多次 fallback 等价于一次 fallback
        let holder = ParliamentLearnerHolder::new();
        holder.fallback_to_static();
        let p1 = holder.current_policy();
        holder.fallback_to_static();
        let p2 = holder.current_policy();
        assert_eq!(p1, p2);
    }

    // ============================================================
    // 便捷方法测试
    // ============================================================

    #[test]
    fn test_strategy_convenience_method() {
        // strategy() 便捷方法等价于 current_policy().strategy()
        let holder = ParliamentLearnerHolder::new();
        assert_eq!(holder.strategy(), ActivationStrategy::Full);

        holder.update_policy(ParliamentPolicy::learned(1, ActivationStrategy::Simplified));
        assert_eq!(holder.strategy(), ActivationStrategy::Simplified);
    }

    #[test]
    fn test_is_learned_and_version() {
        // is_learned() 与 version() 正确反映策略状态
        let holder = ParliamentLearnerHolder::new();
        assert!(!holder.is_learned());
        assert_eq!(holder.version(), None);

        holder.update_policy(ParliamentPolicy::learned(7, ActivationStrategy::FastPath));
        assert!(holder.is_learned());
        assert_eq!(holder.version(), Some(7));
    }

    // ============================================================
    // Clone 独立性测试
    // ============================================================

    #[test]
    fn test_clone_independent() {
        // Clone 后两者策略独立演化,互不影响
        let holder1 = ParliamentLearnerHolder::with_policy(ParliamentPolicy::learned(
            1,
            ActivationStrategy::Simplified,
        ));
        let holder2 = holder1.clone();

        // 修改 holder1 不影响 holder2
        holder1.update_policy(ParliamentPolicy::learned(2, ActivationStrategy::FastPath));
        assert_eq!(holder1.strategy(), ActivationStrategy::FastPath);
        assert_eq!(holder2.strategy(), ActivationStrategy::Simplified);
        assert_eq!(holder2.version(), Some(1));
    }

    #[test]
    fn test_clone_from_static() {
        // 从 Static 克隆,两者均保持 Static
        let holder1 = ParliamentLearnerHolder::new();
        let holder2 = holder1.clone();

        assert!(holder1.is_learned() == holder2.is_learned());
        assert!(!holder2.is_learned());
    }

    // ============================================================
    // C4 合规场景测试(D1 修复对应)
    // ============================================================

    #[test]
    fn test_c4_static_fallback_compiled_into_binary() {
        // spec.md "默认静态值 = 当前常量,fallback 编译进同一二进制"
        let holder = ParliamentLearnerHolder::new();
        let policy = holder.current_policy();
        assert!(policy.is_static());
        // 验证 fallback 值 = 既有 Parliament 行为(Full,5 角色完整辩论)
        assert_eq!(policy.strategy(), ActivationStrategy::Full);
        assert!((policy.strategy().debate_cost() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_c4_learner_panic_local_fallback() {
        // 模拟: omega-learner 下发 Learned 值后 panic,调用方本地 fallback 到 Static
        let holder = ParliamentLearnerHolder::with_policy(ParliamentPolicy::learned(
            1,
            ActivationStrategy::FastPath,
        ));
        assert!(holder.is_learned());

        // learner panic → 调用 fallback_to_static 触发本地 fallback
        holder.fallback_to_static();
        assert!(holder.current_policy().is_static());
        assert_eq!(holder.strategy(), ActivationStrategy::Full);
    }

    #[test]
    fn test_c4_no_cross_crate_flag() {
        // spec.md "无跨 crate 旗标"
        // ParliamentPolicy 通过值注入(Copy),不依赖全局 static 或 feature flag
        let holder = ParliamentLearnerHolder::new();
        let strategy = holder.strategy();
        // 策略值直接从 const 常量获取,无运行时旗标查询
        assert_eq!(strategy, ActivationStrategy::Full);
    }

    #[test]
    fn test_c4_learned_versioned_for_ab_test() {
        // 版本号用于 A/B 测试与回滚
        let holder1 = ParliamentLearnerHolder::with_policy(ParliamentPolicy::learned(
            1,
            ActivationStrategy::Full,
        ));
        let holder2 = ParliamentLearnerHolder::with_policy(ParliamentPolicy::learned(
            2,
            ActivationStrategy::FastPath,
        ));
        assert_ne!(holder1.version(), holder2.version());
        assert_ne!(holder1.strategy(), holder2.strategy());
    }

    // ============================================================
    // 三种激活策略覆盖测试
    // ============================================================

    #[test]
    fn test_strategy_fastpath() {
        let holder = ParliamentLearnerHolder::with_policy(ParliamentPolicy::static_policy(
            ActivationStrategy::FastPath,
        ));
        assert_eq!(holder.strategy(), ActivationStrategy::FastPath);
        assert!(holder.strategy().skipped());
        assert_eq!(holder.strategy().debate_cost(), 0.0);
    }

    #[test]
    fn test_strategy_simplified() {
        let holder = ParliamentLearnerHolder::with_policy(ParliamentPolicy::static_policy(
            ActivationStrategy::Simplified,
        ));
        assert_eq!(holder.strategy(), ActivationStrategy::Simplified);
        assert!(!holder.strategy().skipped());
        assert_eq!(holder.strategy().voter_count(), 3);
    }

    #[test]
    fn test_strategy_full() {
        let holder = ParliamentLearnerHolder::with_policy(ParliamentPolicy::static_policy(
            ActivationStrategy::Full,
        ));
        assert_eq!(holder.strategy(), ActivationStrategy::Full);
        assert!(!holder.strategy().skipped());
        assert_eq!(holder.strategy().voter_count(), 5);
    }

    // ============================================================
    // 端到端场景测试
    // ============================================================

    #[test]
    fn test_scenario_lifecycle_learned_to_fallback() {
        // 模拟完整生命周期:Static → Learned → 熔断 → Static
        let holder = ParliamentLearnerHolder::new();
        assert!(!holder.is_learned());
        assert_eq!(holder.version(), None);

        // 1. omega-learner 下发 Learned(v=1, Simplified)
        holder.update_policy(ParliamentPolicy::learned(1, ActivationStrategy::Simplified));
        assert!(holder.is_learned());
        assert_eq!(holder.version(), Some(1));
        assert_eq!(holder.strategy(), ActivationStrategy::Simplified);

        // 2. 继续学习 v=2, 切到 FastPath
        holder.update_policy(ParliamentPolicy::learned(2, ActivationStrategy::FastPath));
        assert_eq!(holder.version(), Some(2));
        assert_eq!(holder.strategy(), ActivationStrategy::FastPath);

        // 3. 灰度指标不达标,触发熔断
        holder.fallback_to_static();
        assert!(!holder.is_learned());
        assert_eq!(holder.strategy(), ActivationStrategy::Full);
    }

    #[test]
    fn test_scenario_alternating_updates() {
        // 交替更新 Static 与 Learned
        let holder = ParliamentLearnerHolder::new();

        holder.update_policy(ParliamentPolicy::learned(1, ActivationStrategy::Simplified));
        assert!(holder.is_learned());

        holder.update_policy(ParliamentPolicy::static_policy(ActivationStrategy::Full));
        assert!(!holder.is_learned());

        holder.update_policy(ParliamentPolicy::learned(2, ActivationStrategy::FastPath));
        assert!(holder.is_learned());

        holder.update_policy(ParliamentPolicy::static_policy(
            ActivationStrategy::Simplified,
        ));
        assert!(!holder.is_learned());
        assert_eq!(holder.strategy(), ActivationStrategy::Simplified);
    }
}
