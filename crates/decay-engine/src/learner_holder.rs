//! DecayEngine 衰减参数学习器持有器 — S6 接缝策略异步下发 + 本地 fallback
//!
//! 对应任务: **P4-W14.4.2**（decay-engine 接入 omega-learner 异步下发）
//! 对应 ADR: **ADR-031**（omega-learner 边界）+ **ADR-033**（nexus-contracts L0 契约层）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 S6 接缝
//!
//! # 核心职责
//!
//! 承载 `DecayPolicy` 的运行时可变状态，为 `DecayEngine` 提供：
//! - **异步策略接收**: `update_policy()` 接收 `omega-learner` 下发的 `DecayPolicy::Learned`
//! - **当前策略查询**: `current_policy()` 返回当前激活的策略
//! - **策略感知衰减**: `profile()` 返回当前 `DecayProfile` 供 `decay_with_policy` 使用
//! - **本地 fallback**: 任何异常回到 `DecayPolicy::Static(DecayProfile::Standard)`（C4 合规）
//!
//! # 依赖铁律合规（WHY decay-engine 不直接依赖 omega-learner）
//!
//! ```text
//! L6 omega-learner  ────(learned DecayPolicy)───▶  上层编排器
//!      │                                                    │
//!      │ L6 → L0 ✓                                        │ L0 → 注入
//!      ▼                                                    ▼
//! L0 nexus-contracts  ◀──(DecayPolicy 类型)──  L4 decay-engine
//!      DecayPolicy                                          │
//!      DecayProfile                                         │ L4 → L0 ✓
//!                                                           ▼
//!                                                  DecayLearnerHolder
//! ```
//!
//! decay-engine (L4) 只依赖 `nexus-contracts` (L0) 的 `DecayPolicy` 类型，
//! **不直接依赖** `omega-learner` (L6)，遵守 §2.2 依赖铁律。
//! `omega-learner` 输出的 `DecayPolicy::Learned` 由上层编排器
//! （chimera-cli / quest-engine）通过 `update_policy()` 注入。
//!
//! # C4 合规（能力场灰度，非运行时旗）
//!
//! - **默认值层**: `DecayLearnerHolder::new()` 初始化为
//!   `DecayPolicy::Static(DecayProfile::Standard)`（编译期常量 fallback）
//! - **异常回退层**: `RwLock::write().unwrap_or_else()` 处理 `PoisonError` 自动回退
//! - **熔断入口层**: `fallback_to_static()` 供 `omega-learner` 触发学习熔断
//!   （spec.md:335 S6 灰度阶段目标达成率降 >2%）时主动回退
//!
//! 三层叠加实现"learner panic/超时时调用方本地 fallback，无跨 crate 旗标传播"
//! 的 C4 合规要求（spec.md:334）。
//!
//! # 与 ParliamentLearnerHolder / PrefetchLearnerHolder 的对称设计
//!
//! S1/S2/S3/S5/S6 五接缝共用相同骨架：
//! - 枚举 + `Static`/`Learned` 双变体策略
//! - `RwLock<Policy>` + `new`/`with_policy`/`update_policy`/`fallback_to_static`
//! - `current_policy`/`is_learned`/`version`/`Default`/`Clone`
//!
//! 差异仅在策略类型与感知方法：
//! - S1: `DensityPolicy` + `select_with_density`（HCW 密度感知选择）
//! - S2: `MemoryStrategyPolicy` + `recall_by_clv_with_strategy`（MLC 记忆策略感知）
//! - S3: `PrefetchPolicy` + `prefetch_with_policy`（SCC 预取策略感知）
//! - S5: `ParliamentPolicy` + `deliberate_with_policy`（Parliament 激活策略感知）
//! - S6: `DecayPolicy` + `decay_with_policy`（DecayEngine 衰减档位感知）
//!
//! # 线程安全
//!
//! 内部用 `RwLock<DecayPolicy>` 保护策略状态：
//! - **写锁**: `update_policy()` 异步下发（低频，每秒 < 1 次）
//! - **读锁**: `current_policy()` / `profile()` 热路径查询（高频，每次 `decay_with_policy` 调用）
//!
//! 读写分离避免锁竞争。`RwLock` 选择 `std::sync::RwLock` 而非 `tokio::sync::RwLock`：
//! - 读路径需要 sync 访问（`decay_with_policy` 入口处同步读取策略）
//! - 持锁时间极短（仅读取 `DecayPolicy` 枚举，~10ns）
//!
//! # 示例
//!
//! ## 基础 fallback 行为
//!
//! ```
//! use nexus_contracts::DecayProfile;
//! use decay_engine::learner_holder::DecayLearnerHolder;
//!
//! let holder = DecayLearnerHolder::new();
//!
//! // 初始化为 Static fallback（Standard = 默认衰减参数）
//! let policy = holder.current_policy();
//! assert!(policy.is_static());
//! assert_eq!(policy.profile(), DecayProfile::Standard);
//! ```
//!
//! ## 异步下发学习策略
//!
//! ```
//! use nexus_contracts::DecayProfile;
//! use decay_engine::learner_holder::DecayLearnerHolder;
//!
//! let holder = DecayLearnerHolder::new();
//!
//! // omega-learner 异步下发学习策略（Strict，高风险写操作场景）
//! holder.update_policy(nexus_contracts::DecayPolicy::learned(
//!     42,
//!     DecayProfile::Strict,
//! ));
//!
//! let policy = holder.current_policy();
//! assert!(policy.is_learned());
//! assert_eq!(policy.version(), Some(42));
//! assert_eq!(policy.profile(), DecayProfile::Strict);
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use nexus_contracts::{DecayPolicy, DecayProfile};

// ============================================================
// DecayLearnerHolder
// ============================================================

/// DecayEngine 衰减参数学习器持有器 — 运行时可变的 `DecayPolicy` 容器
///
/// 承载 `omega-learner` 异步下发的学习策略，为 `DecayEngine` 的
/// `decay_with_policy` 路径提供策略感知能力。所有方法线程安全
/// （`RwLock` 保护）。
///
/// # 设计决策（WHY）
///
/// - **独立 struct 而非嵌入 DecayEngine**: 单一职责，便于单测与复用；
///   也避免 `DecayEngine` 结构体膨胀（已有 2 字段）
/// - **`RwLock<DecayPolicy>` 而非 `AtomicU8`**: `DecayPolicy`
///   是枚举（Static/Learned），原子化需要 `AtomicU8` + 手动重建枚举，复杂且易错
/// - **`std::sync::RwLock` 而非 `tokio::sync::RwLock`**: 读路径是 sync
///   （`decay_with_policy` 入口同步读取策略），持锁时间极短（~10ns）
///
/// # 线程安全
///
/// `DecayLearnerHolder` 内部 `RwLock` 保证：
/// - 多读单写（`current_policy`/`profile` 并发读，`update_policy` 独占写）
/// - 持锁时间极短（仅读写 `DecayPolicy` 枚举，~10ns）
/// - 无 await 跨锁（`update_policy` 是 sync 方法，避免 §4.4 反模式 1）
///
/// # 示例
///
/// ```
/// use nexus_contracts::DecayProfile;
/// use decay_engine::learner_holder::DecayLearnerHolder;
///
/// let holder = DecayLearnerHolder::new();
/// assert!(holder.current_policy().is_static());
/// assert_eq!(holder.profile(), DecayProfile::Standard);
///
/// holder.update_policy(nexus_contracts::DecayPolicy::learned(
///     1,
///     DecayProfile::Strict,
/// ));
/// assert!(holder.current_policy().is_learned());
/// assert_eq!(holder.profile(), DecayProfile::Strict);
/// ```
#[derive(Debug)]
pub struct DecayLearnerHolder {
    /// 当前激活的策略（`RwLock` 保护，读写分离）
    ///
    /// WHY 用 `RwLock` 而非 `Mutex`：
    /// - 读路径（`current_policy`/`profile`）高频且只读（每次 decay 调用一次）
    /// - 写路径（`update_policy`）低频（每秒 < 1 次）
    /// - `RwLock` 允许并发读，避免读路径串行化
    policy: RwLock<DecayPolicy>,

    /// P2-11: fallback 触发次数计数器(metrics 埋点)
    ///
    /// 三层 fallback 触发点均会递增:
    /// - 异常回退层: `update_policy()` 中 `PoisonError` 自动回退
    /// - 熔断入口层: `fallback_to_static()` 主动回退
    ///
    /// WHY `AtomicU64` 而非 `Mutex<u64>`:
    /// - fallback 可能在 panic 恢复路径触发,无锁更安全
    /// - 读路径(`fallback_count`)高频且无副作用,原子读取即可
    /// - 写路径原子 `fetch_add` 无需持锁,符合 §4.4 反模式 1(禁止持锁 .await)合规
    ///
    /// 调用方通过 `take_fallback_count()` 周期性取出增量并发布
    /// `DecayMetricsReported` 事件,用于监控 learner 健康度。
    fallback_count: AtomicU64,
}

impl DecayLearnerHolder {
    /// 创建持有器，初始化为 `DecayPolicy::Static(DecayProfile::Standard)`（fallback）
    ///
    /// WHY 初始化为 Static(Standard): C4 合规要求默认行为零变化，
    /// `Standard` 对应既有 `DecayConfig::default()` 行为
    /// （time_decay_rate=0.001, event_decay_penalty=0.1, freeze_threshold=0.05），
    /// 向后兼容。`omega-learner` 未下发学习策略时，行为与 P4 修复前完全一致。
    pub fn new() -> Self {
        Self {
            policy: RwLock::new(DecayPolicy::fallback()),
            fallback_count: AtomicU64::new(0),
        }
    }

    /// 创建持有器，指定初始策略（便于测试）
    ///
    /// WHY 提供：单测需要构造特定策略场景（如 Learned 初始状态）
    pub fn with_policy(policy: DecayPolicy) -> Self {
        Self {
            policy: RwLock::new(policy),
            fallback_count: AtomicU64::new(0),
        }
    }

    /// 异步下发策略 — 接收 `omega-learner` 学习到的 `DecayPolicy`
    ///
    /// # 设计
    ///
    /// - 写入 `RwLock`（独占写锁，~10ns）
    /// - 不返回错误：任何异常（如 PoisonError）静默 fallback 到 Static(Standard)
    ///
    /// # C4 合规
    ///
    /// 调用方（chimera-cli / quest-engine）在 `omega-learner` panic/超时时
    /// 不调用此方法，`DecayLearnerHolder` 保持上一次的有效策略。
    /// 若需强制回退到 fallback，调用方传入 `DecayPolicy::fallback()`。
    ///
    /// # 参数
    /// - `policy`: 新策略（Static 或 Learned）
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::{DecayPolicy, DecayProfile};
    /// use decay_engine::learner_holder::DecayLearnerHolder;
    ///
    /// let holder = DecayLearnerHolder::new();
    /// holder.update_policy(DecayPolicy::learned(1, DecayProfile::Strict));
    /// assert_eq!(holder.profile(), DecayProfile::Strict);
    /// ```
    pub fn update_policy(&self, policy: DecayPolicy) {
        // WHY unwrap_or_else: RwLock poison 时 fallback 到 Static(Standard)
        // 避免调用方处理 PoisonError（C4 合规：本地 fallback，无错误传播）
        // P2-11: PoisonError 路径属于"异常回退层" fallback,计数 +1
        let mut guard = self.policy.write().unwrap_or_else(|p| {
            // PoisonError 时恢复锁并写入 fallback
            let mut guard = p.into_inner();
            *guard = DecayPolicy::fallback();
            // 记录异常回退层 fallback(metrics 埋点)
            self.fallback_count.fetch_add(1, Ordering::Relaxed);
            guard
        });
        *guard = policy;
    }

    /// 强制回退到 fallback 策略（`Static(Standard)`）
    ///
    /// WHY 提供：`omega-learner` 触发学习熔断（spec.md:335 S6 灰度阶段目标
    /// 达成率降 >2%）时，上层调用方调用此方法立即回退到静态策略。
    ///
    /// # P2-11 计数说明
    ///
    /// 此方法对应"熔断入口层" fallback,计数 +1。
    /// 不复用 `update_policy()` 以避免双重计数(熔断 + PoisonError 路径)
    /// —— 本方法是 sync 正常路径,不会触发 PoisonError。
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::DecayProfile;
    /// use decay_engine::learner_holder::DecayLearnerHolder;
    ///
    /// let holder = DecayLearnerHolder::new();
    /// holder.fallback_to_static();
    /// assert!(holder.current_policy().is_static());
    /// assert_eq!(holder.profile(), DecayProfile::Standard);
    /// ```
    pub fn fallback_to_static(&self) {
        // 直接操作锁,不复用 update_policy(避免计数歧义)
        let mut guard = self.policy.write().unwrap_or_else(|p| {
            // 极端情况:熔断时锁已 poison,同时计数
            let mut guard = p.into_inner();
            *guard = DecayPolicy::fallback();
            self.fallback_count.fetch_add(1, Ordering::Relaxed);
            guard
        });
        *guard = DecayPolicy::fallback();
        // 记录熔断入口层 fallback(metrics 埋点)
        self.fallback_count.fetch_add(1, Ordering::Relaxed);
    }

    /// 返回累计 fallback 触发次数(P2-11 metrics 埋点)
    ///
    /// 计数范围:自创建以来所有 fallback 触发(含异常回退层 + 熔断入口层)
    ///
    /// # 性能
    ///
    /// `AtomicU64::load` 无锁,~1ns。可安全在热路径调用。
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::DecayPolicy;
    /// use nexus_contracts::DecayProfile;
    /// use decay_engine::learner_holder::DecayLearnerHolder;
    ///
    /// let holder = DecayLearnerHolder::new();
    /// assert_eq!(holder.fallback_count(), 0);
    ///
    /// holder.fallback_to_static();
    /// assert_eq!(holder.fallback_count(), 1);
    /// ```
    pub fn fallback_count(&self) -> u64 {
        self.fallback_count.load(Ordering::Relaxed)
    }

    /// 取出累计 fallback 次数并重置为 0(P2-11 metrics 埋点)
    ///
    /// 用于周期性上报场景:调用方定期 `take` 并发布 `DecayMetricsReported` 事件,
    /// 避免 fallback 高频时事件风暴(单次事件携带 delta 而非累计值)。
    ///
    /// # 原子语义
    ///
    /// `swap(0)` 是原子读-改-写操作,保证:
    /// - 并发 `take` 不会丢失计数
    /// - `take` 与 `fallback_to_static` 并发时计数准确
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::DecayPolicy;
    /// use nexus_contracts::DecayProfile;
    /// use decay_engine::learner_holder::DecayLearnerHolder;
    ///
    /// let holder =
    ///     DecayLearnerHolder::with_policy(DecayPolicy::learned(1, DecayProfile::Aggressive));
    ///
    /// holder.fallback_to_static();
    /// holder.fallback_to_static();
    ///
    /// let delta = holder.take_fallback_count();
    /// assert_eq!(delta, 2);
    /// assert_eq!(holder.fallback_count(), 0);
    /// ```
    pub fn take_fallback_count(&self) -> u64 {
        self.fallback_count.swap(0, Ordering::Relaxed)
    }

    /// 返回当前激活的策略（快照）
    ///
    /// 返回 `DecayPolicy` 的 Copy（枚举整体 Copy），调用方无需持有锁。
    ///
    /// # 性能
    ///
    /// 读锁 + Copy 枚举，~10ns。热路径调用无性能影响。
    pub fn current_policy(&self) -> DecayPolicy {
        let guard = self.policy.read().unwrap_or_else(|p| p.into_inner());
        *guard
    }

    /// 返回当前激活的衰减档位（便捷方法）
    ///
    /// 等价于 `current_policy().profile()`，但避免调用方重复 match。
    /// 用于 `decay_with_policy` 路径的策略感知决策。
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::{DecayPolicy, DecayProfile};
    /// use decay_engine::learner_holder::DecayLearnerHolder;
    ///
    /// let holder = DecayLearnerHolder::new();
    /// assert_eq!(holder.profile(), DecayProfile::Standard);
    ///
    /// holder.update_policy(DecayPolicy::learned(1, DecayProfile::Aggressive));
    /// assert_eq!(holder.profile(), DecayProfile::Aggressive);
    /// ```
    pub fn profile(&self) -> DecayProfile {
        self.current_policy().profile()
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

impl Default for DecayLearnerHolder {
    /// 默认状态 = `DecayPolicy::Static(Standard)`（fallback，C4 合规）
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for DecayLearnerHolder {
    /// 克隆持有器（创建新的 RwLock，策略快照独立）
    ///
    /// WHY 提供：上层编排器可能需要克隆 `DecayLearnerHolder`
    /// 用于快照或并行处理。克隆后两者策略独立演化，互不影响。
    ///
    /// # P2-11 计数器独立性
    ///
    /// 克隆时 `fallback_count` 重置为 0(独立计数):
    /// - 克隆产生的新 holder 是独立监控单元
    /// - 避免原 holder 的历史 fallback 计数污染新 holder 的 metrics
    /// - 调用方若需保留累计计数,应显式 `take_fallback_count()` 后合并
    fn clone(&self) -> Self {
        Self {
            policy: RwLock::new(self.current_policy()),
            fallback_count: AtomicU64::new(0),
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::{DecayPolicy, DecayProfile};

    // ============================================================
    // 基础行为测试
    // ============================================================

    #[test]
    fn test_new_is_static_fallback() {
        // C4 合规: new() 必须初始化为 Static(Standard) fallback
        let holder = DecayLearnerHolder::new();
        let policy = holder.current_policy();
        assert!(policy.is_static());
        assert_eq!(policy.profile(), DecayProfile::Standard);
    }

    #[test]
    fn test_default_equals_new() {
        // Default 与 new() 行为一致
        let holder1 = DecayLearnerHolder::new();
        let holder2 = DecayLearnerHolder::default();
        assert_eq!(holder1.current_policy(), holder2.current_policy());
    }

    #[test]
    fn test_with_policy_learned() {
        // with_policy 可指定 Learned 初始状态（便于测试）
        let holder =
            DecayLearnerHolder::with_policy(DecayPolicy::learned(42, DecayProfile::Aggressive));
        let policy = holder.current_policy();
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(42));
        assert_eq!(policy.profile(), DecayProfile::Aggressive);
    }

    #[test]
    fn test_with_policy_static_strict() {
        // with_policy 可指定 Static(Strict) 初始状态
        let holder =
            DecayLearnerHolder::with_policy(DecayPolicy::static_policy(DecayProfile::Strict));
        let policy = holder.current_policy();
        assert!(policy.is_static());
        assert_eq!(policy.profile(), DecayProfile::Strict);
    }

    // ============================================================
    // update_policy 测试
    // ============================================================

    #[test]
    fn test_update_policy_to_learned() {
        // 从 Static 切换到 Learned
        let holder = DecayLearnerHolder::new();
        assert!(holder.current_policy().is_static());

        holder.update_policy(DecayPolicy::learned(1, DecayProfile::Strict));
        let policy = holder.current_policy();
        assert!(policy.is_learned());
        assert_eq!(policy.profile(), DecayProfile::Strict);
        assert_eq!(policy.version(), Some(1));
    }

    #[test]
    fn test_update_policy_to_static() {
        // 从 Learned 切换回 Static
        let holder = DecayLearnerHolder::with_policy(DecayPolicy::learned(1, DecayProfile::Strict));
        assert!(holder.current_policy().is_learned());

        holder.update_policy(DecayPolicy::static_policy(DecayProfile::Standard));
        assert!(holder.current_policy().is_static());
        assert_eq!(holder.profile(), DecayProfile::Standard);
    }

    #[test]
    fn test_update_policy_multiple_times() {
        // 连续多次更新策略，验证版本号与策略一致性
        let holder = DecayLearnerHolder::new();
        let profiles = [
            DecayProfile::Lenient,
            DecayProfile::Standard,
            DecayProfile::Strict,
            DecayProfile::Aggressive,
            DecayProfile::Lenient,
        ];

        for (version, profile) in profiles.iter().enumerate() {
            holder.update_policy(DecayPolicy::learned(version as u64 + 1, *profile));
            assert_eq!(holder.version(), Some(version as u64 + 1));
            assert_eq!(holder.profile(), *profile);
        }
    }

    #[test]
    fn test_update_policy_version_zero() {
        // 版本号 0 是合法值（首个学习版本）
        let holder = DecayLearnerHolder::new();
        holder.update_policy(DecayPolicy::learned(0, DecayProfile::Strict));
        assert!(holder.is_learned());
        assert_eq!(holder.version(), Some(0));
    }

    // ============================================================
    // fallback_to_static 测试
    // ============================================================

    #[test]
    fn test_fallback_to_static_from_learned() {
        // 从 Learned 回退到 Static(Standard)
        let holder =
            DecayLearnerHolder::with_policy(DecayPolicy::learned(99, DecayProfile::Aggressive));
        assert!(holder.current_policy().is_learned());
        assert_eq!(holder.profile(), DecayProfile::Aggressive);

        holder.fallback_to_static();
        assert!(holder.current_policy().is_static());
        assert_eq!(holder.profile(), DecayProfile::Standard);
    }

    #[test]
    fn test_fallback_to_static_idempotent() {
        // 多次 fallback 等价于一次 fallback
        let holder = DecayLearnerHolder::new();
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
    fn test_profile_convenience_method() {
        // profile() 便捷方法等价于 current_policy().profile()
        let holder = DecayLearnerHolder::new();
        assert_eq!(holder.profile(), DecayProfile::Standard);

        holder.update_policy(DecayPolicy::learned(1, DecayProfile::Strict));
        assert_eq!(holder.profile(), DecayProfile::Strict);
    }

    #[test]
    fn test_is_learned_and_version() {
        // is_learned() 与 version() 正确反映策略状态
        let holder = DecayLearnerHolder::new();
        assert!(!holder.is_learned());
        assert_eq!(holder.version(), None);

        holder.update_policy(DecayPolicy::learned(7, DecayProfile::Strict));
        assert!(holder.is_learned());
        assert_eq!(holder.version(), Some(7));
    }

    // ============================================================
    // Clone 独立性测试
    // ============================================================

    #[test]
    fn test_clone_independent() {
        // Clone 后两者策略独立演化，互不影响
        let holder1 =
            DecayLearnerHolder::with_policy(DecayPolicy::learned(1, DecayProfile::Strict));
        let holder2 = holder1.clone();

        // 修改 holder1 不影响 holder2
        holder1.update_policy(DecayPolicy::learned(2, DecayProfile::Aggressive));
        assert_eq!(holder1.profile(), DecayProfile::Aggressive);
        assert_eq!(holder2.profile(), DecayProfile::Strict);
        assert_eq!(holder2.version(), Some(1));
    }

    #[test]
    fn test_clone_from_static() {
        // 从 Static 克隆，两者均保持 Static
        let holder1 = DecayLearnerHolder::new();
        let holder2 = holder1.clone();

        assert!(holder1.is_learned() == holder2.is_learned());
        assert!(!holder2.is_learned());
    }

    // ============================================================
    // C4 合规场景测试（D1 修复对应）
    // ============================================================

    #[test]
    fn test_c4_static_fallback_compiled_into_binary() {
        // spec.md "默认静态值 = 当前常量，fallback 编译进同一二进制"
        let holder = DecayLearnerHolder::new();
        let policy = holder.current_policy();
        assert!(policy.is_static());
        // 验证 fallback 值 = 既有 DecayEngine 行为（Standard，对应 DecayConfig::default()）
        assert_eq!(policy.profile(), DecayProfile::Standard);
        // 验证参数与 DecayConfig::default() 一致
        let profile = policy.profile();
        assert!((profile.time_decay_rate() - 0.001).abs() < 1e-6);
        assert!((profile.event_decay_penalty() - 0.1).abs() < 1e-6);
        assert!((profile.freeze_threshold() - 0.05).abs() < 1e-6);
    }

    #[test]
    fn test_c4_learner_panic_local_fallback() {
        // 模拟: omega-learner 下发 Learned 值后 panic，调用方本地 fallback 到 Static
        let holder =
            DecayLearnerHolder::with_policy(DecayPolicy::learned(1, DecayProfile::Aggressive));
        assert!(holder.is_learned());

        // learner panic → 调用 fallback_to_static 触发本地 fallback
        holder.fallback_to_static();
        assert!(holder.current_policy().is_static());
        assert_eq!(holder.profile(), DecayProfile::Standard);
    }

    #[test]
    fn test_c4_no_cross_crate_flag() {
        // spec.md "无跨 crate 旗标"
        // DecayPolicy 通过值注入（Copy），不依赖全局 static 或 feature flag
        let holder = DecayLearnerHolder::new();
        let profile = holder.profile();
        // 策略值直接从 const 常量获取，无运行时旗标查询
        assert_eq!(profile, DecayProfile::Standard);
    }

    #[test]
    fn test_c4_learned_versioned_for_ab_test() {
        // 版本号用于 A/B 测试与回滚
        let holder1 =
            DecayLearnerHolder::with_policy(DecayPolicy::learned(1, DecayProfile::Standard));
        let holder2 =
            DecayLearnerHolder::with_policy(DecayPolicy::learned(2, DecayProfile::Strict));
        assert_ne!(holder1.version(), holder2.version());
        assert_ne!(holder1.profile(), holder2.profile());
    }

    // ============================================================
    // 四种衰减档位覆盖测试
    // ============================================================

    #[test]
    fn test_profile_lenient() {
        let holder =
            DecayLearnerHolder::with_policy(DecayPolicy::static_policy(DecayProfile::Lenient));
        assert_eq!(holder.profile(), DecayProfile::Lenient);
        // Lenient: 慢衰减 + 低惩罚 + 低阈值
        assert!(holder.profile().time_decay_rate() < 0.001);
        assert!(holder.profile().event_decay_penalty() < 0.1);
        assert!(holder.profile().freeze_threshold() < 0.05);
    }

    #[test]
    fn test_profile_standard() {
        let holder =
            DecayLearnerHolder::with_policy(DecayPolicy::static_policy(DecayProfile::Standard));
        assert_eq!(holder.profile(), DecayProfile::Standard);
        // Standard: 默认值（与 DecayConfig::default() 一致）
        assert!((holder.profile().time_decay_rate() - 0.001).abs() < 1e-6);
        assert!((holder.profile().event_decay_penalty() - 0.1).abs() < 1e-6);
        assert!((holder.profile().freeze_threshold() - 0.05).abs() < 1e-6);
    }

    #[test]
    fn test_profile_strict() {
        let holder =
            DecayLearnerHolder::with_policy(DecayPolicy::static_policy(DecayProfile::Strict));
        assert_eq!(holder.profile(), DecayProfile::Strict);
        // Strict: 快衰减 + 高惩罚 + 高阈值
        assert!(holder.profile().time_decay_rate() > 0.001);
        assert!(holder.profile().event_decay_penalty() > 0.1);
        assert!(holder.profile().freeze_threshold() > 0.05);
    }

    #[test]
    fn test_profile_aggressive() {
        let holder =
            DecayLearnerHolder::with_policy(DecayPolicy::static_policy(DecayProfile::Aggressive));
        assert_eq!(holder.profile(), DecayProfile::Aggressive);
        // Aggressive: 极快衰减 + 最高惩罚 + 最高阈值
        assert!(holder.profile().time_decay_rate() > 0.005);
        assert!(holder.profile().event_decay_penalty() > 0.15);
        assert!(holder.profile().freeze_threshold() > 0.10);
    }

    // ============================================================
    // 端到端场景测试
    // ============================================================

    #[test]
    fn test_scenario_lifecycle_learned_to_fallback() {
        // 模拟完整生命周期: Static → Learned → 熔断 → Static
        let holder = DecayLearnerHolder::new();
        assert!(!holder.is_learned());
        assert_eq!(holder.version(), None);

        // 1. omega-learner 下发 Learned(v=1, Strict)
        holder.update_policy(DecayPolicy::learned(1, DecayProfile::Strict));
        assert!(holder.is_learned());
        assert_eq!(holder.version(), Some(1));
        assert_eq!(holder.profile(), DecayProfile::Strict);

        // 2. 继续学习 v=2, 切到 Aggressive
        holder.update_policy(DecayPolicy::learned(2, DecayProfile::Aggressive));
        assert_eq!(holder.version(), Some(2));
        assert_eq!(holder.profile(), DecayProfile::Aggressive);

        // 3. 灰度指标不达标，触发熔断
        holder.fallback_to_static();
        assert!(!holder.is_learned());
        assert_eq!(holder.profile(), DecayProfile::Standard);
    }

    #[test]
    fn test_scenario_alternating_updates() {
        // 交替更新 Static 与 Learned
        let holder = DecayLearnerHolder::new();

        holder.update_policy(DecayPolicy::learned(1, DecayProfile::Strict));
        assert!(holder.is_learned());

        holder.update_policy(DecayPolicy::static_policy(DecayProfile::Standard));
        assert!(!holder.is_learned());

        holder.update_policy(DecayPolicy::learned(2, DecayProfile::Aggressive));
        assert!(holder.is_learned());

        holder.update_policy(DecayPolicy::static_policy(DecayProfile::Lenient));
        assert!(!holder.is_learned());
        assert_eq!(holder.profile(), DecayProfile::Lenient);
    }

    // ============================================================
    // P2-11: fallback_count metrics 埋点测试
    // ============================================================
    // 对应任务: decay-engine learner_holder fallback 策略缺乏 metrics 埋点
    // 设计: 三层 fallback 触发点均计数,提供 fallback_count() / take_fallback_count()
    // WHY: 调用方/运维方需感知 fallback 何时发生、发生次数,以监控 learner 健康度

    #[test]
    fn test_fallback_count_initial_zero() {
        // 初始化时 fallback_count 必须为 0(未触发任何 fallback)
        let holder = DecayLearnerHolder::new();
        assert_eq!(holder.fallback_count(), 0);
    }

    #[test]
    fn test_fallback_count_increments_on_fallback_to_static() {
        // fallback_to_static() 触发熔断入口层 fallback,计数 +1
        let holder =
            DecayLearnerHolder::with_policy(DecayPolicy::learned(1, DecayProfile::Aggressive));
        assert_eq!(holder.fallback_count(), 0);

        holder.fallback_to_static();
        assert_eq!(holder.fallback_count(), 1);

        // 多次 fallback 累加
        holder.fallback_to_static();
        assert_eq!(holder.fallback_count(), 2);
    }

    #[test]
    fn test_fallback_count_no_increment_on_normal_update() {
        // 正常 update_policy(非 fallback 路径)不应增加 fallback_count
        let holder = DecayLearnerHolder::new();
        assert_eq!(holder.fallback_count(), 0);

        holder.update_policy(DecayPolicy::learned(1, DecayProfile::Strict));
        assert_eq!(holder.fallback_count(), 0, "正常 Learned 更新不应计数");

        holder.update_policy(DecayPolicy::learned(2, DecayProfile::Aggressive));
        assert_eq!(holder.fallback_count(), 0, "正常版本升级不应计数");

        holder.update_policy(DecayPolicy::static_policy(DecayProfile::Lenient));
        assert_eq!(holder.fallback_count(), 0, "正常 Static 更新不应计数");
    }

    #[test]
    fn test_take_fallback_count_resets_counter() {
        // take_fallback_count() 返回累计计数并重置为 0
        // 用于周期性上报场景:调用方定期 take 并发布 metrics 事件
        let holder =
            DecayLearnerHolder::with_policy(DecayPolicy::learned(1, DecayProfile::Aggressive));

        holder.fallback_to_static();
        holder.fallback_to_static();
        assert_eq!(holder.fallback_count(), 2);

        // take 返回 2 并重置
        let taken = holder.take_fallback_count();
        assert_eq!(taken, 2);
        assert_eq!(holder.fallback_count(), 0, "take 后计数应重置");

        // 再次 take 返回 0(无新增 fallback)
        let taken_again = holder.take_fallback_count();
        assert_eq!(taken_again, 0);
    }

    #[test]
    fn test_take_fallback_count_delta_after_period() {
        // 模拟周期性上报场景:两次 take 之间的增量为 delta
        let holder = DecayLearnerHolder::new();

        // 第一次周期:无 fallback
        let delta1 = holder.take_fallback_count();
        assert_eq!(delta1, 0);

        // 第二次周期:触发 3 次 fallback
        holder.fallback_to_static();
        holder.fallback_to_static();
        holder.fallback_to_static();
        let delta2 = holder.take_fallback_count();
        assert_eq!(delta2, 3, "第二次周期 delta 应为 3");

        // 第三次周期:无新增
        let delta3 = holder.take_fallback_count();
        assert_eq!(delta3, 0);
    }

    #[test]
    fn test_fallback_count_thread_safe_concurrent() {
        // 并发场景:多线程同时 fallback_to_static,计数必须准确
        // WHY AtomicU64:无锁并发安全,符合 §4.4 反模式 1(禁止持锁 .await)合规
        use std::sync::Arc;
        use std::thread;

        let holder = Arc::new(DecayLearnerHolder::new());
        let mut handles = vec![];

        for _ in 0..8 {
            let h = Arc::clone(&holder);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    h.fallback_to_static();
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // 8 线程 × 100 次 = 800 次 fallback
        assert_eq!(holder.fallback_count(), 800);
    }
}
