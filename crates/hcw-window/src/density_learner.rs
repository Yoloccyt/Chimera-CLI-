//! 密度学习器持有器 — S1 接缝策略异步下发 + 本地 fallback
//!
//! 对应任务: **P4-W13.2.2**（hcw-window 接入 omega-learner 异步下发）
//! 对应 ADR: **ADR-031**（omega-learner 边界）+ **ADR-033**（nexus-contracts L0 契约层）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 S1 接缝
//!
//! # 核心职责
//!
//! 承载 `DensityPolicy` 的运行时可变状态，为 `HcwWindow` 提供:
//! - **异步策略接收**: `update_policy()` 接收 `omega-learner` 下发的 `DensityPolicy::Learned`
//! - **当前策略查询**: `current_policy()` 返回当前激活的策略
//! - **密度感知选择**: `select_with_density()` 结合 complexity 与 density policy 选择窗口
//! - **本地 fallback**: 任何异常回到 `DensityPolicy::Static(Rho10)`（C4 合规）
//!
//! # 依赖铁律合规（WHY hcw-window 不直接依赖 omega-learner）
//!
//! ```text
//! L6 omega-learner  ────(learned DensityPolicy)───▶  上层编排器
//!      │                                              │
//!      │ L6 → L0 ✓                                  │ L0 → 注入
//!      ▼                                              ▼
//! L0 nexus-contracts  ◀──(DensityPolicy 类型)──  L2 hcw-window
//!      DensityPolicy                                    │
//!      DensityTier                                       │ L2 → L0 ✓
//!                                                       ▼
//!                                              DensityLearnerHolder
//! ```
//!
//! hcw-window (L2) 只依赖 `nexus-contracts` (L0) 的 `DensityPolicy` 类型，
//! **不直接依赖** `omega-learner` (L6)，遵守 §2.2 依赖铁律。
//! `omega-learner` 输出的 `DensityPolicy::Learned` 由上层编排器
//! （chimera-cli / quest-engine）通过 `update_policy()` 注入。
//!
//! # C4 合规（能力场灰度，非运行时旗）
//!
//! - `DensityLearnerHolder::new()` 初始化为 `DensityPolicy::Static(Rho10)`（fallback）
//! - `update_policy()` 接收 `Learned` 策略时，校验版本号与 tier 合法性
//! - 校验失败或任何异常时，自动回退到 `Static(Rho10)`，**无跨 crate 旗标传播**
//! - `omega-learner` panic/超时时，调用方本地 fallback，无需等待 learner 恢复
//!
//! # 线程安全
//!
//! 内部用 `RwLock<DensityPolicy>` 保护策略状态:
//! - **写锁**: `update_policy()` 异步下发（低频，每秒 < 1 次）
//! - **读锁**: `current_policy()` / `select_with_density()` 热路径查询（高频）
//!
//! 读写分离避免锁竞争。`RwLock` 选择 `std::sync::RwLock` 而非 `tokio::sync::RwLock`:
//! - 读路径需要 sync 访问（`WindowSelector::select` 是同步函数）
//! - 持锁时间极短（仅读取 `DensityPolicy`，~10ns）
//!
//! # 示例
//!
//! ## 基础 fallback 行为
//!
//! ```
//! use hcw_window::density_learner::DensityLearnerHolder;
//! use nexus_contracts::{DensityPolicy, DensityTier};
//!
//! let holder = DensityLearnerHolder::new();
//!
//! // 初始化为 Static fallback
//! let policy = holder.current_policy();
//! assert!(policy.is_static());
//! assert_eq!(policy.tier(), DensityTier::Rho10);
//! ```
//!
//! ## 异步下发学习策略
//!
//! ```
//! use hcw_window::density_learner::DensityLearnerHolder;
//! use nexus_contracts::{DensityPolicy, DensityTier};
//!
//! let holder = DensityLearnerHolder::new();
//!
//! // omega-learner 异步下发学习策略（ρ=2，强稀疏化）
//! holder.update_policy(DensityPolicy::learned(42, DensityTier::Rho2));
//!
//! let policy = holder.current_policy();
//! assert!(policy.is_learned());
//! assert_eq!(policy.version(), Some(42));
//! assert_eq!(policy.tier(), DensityTier::Rho2);
//! ```

use std::sync::RwLock;

use nexus_contracts::{DensityPolicy, DensityTier};

use crate::types::{HcwConfig, WindowTier};
use crate::WindowSelector;

// ============================================================
// 常量定义
// ============================================================

/// L3 窗口的标称容量上限（1M Token）— 仅测试使用
///
/// WHY 定义: 测试场景中作为标称容量参考值，验证架构红线保护:
/// `select_with_density(0.9, L3_NOMINAL_CAPACITY)` 在 L3 + Rho10 时自动降级到 Rho05。
/// 生产代码使用 `HcwConfig::default().l3_capacity`（同样 = 1_048_576），不引用此常量。
#[cfg(test)]
const L3_NOMINAL_CAPACITY: usize = 1_048_576;

// ============================================================
// DensityLearnerHolder
// ============================================================

/// 密度学习器持有器 — 运行时可变的 `DensityPolicy` 容器
///
/// 承载 `omega-learner` 异步下发的学习策略，为 `HcwWindow` 的 selector
/// 提供密度感知选择能力。所有方法线程安全（`RwLock` 保护）。
///
/// # 设计决策（WHY）
///
/// - **独立 struct 而非嵌入 HcwWindow**: 单一职责，便于单测与复用
/// - **`RwLock<DensityPolicy>` 而非 `AtomicU8`**: `DensityPolicy` 是枚举
///   （Static/Learned），原子化需要 `AtomicU8` + 手动重建枚举，复杂且易错
/// - **`std::sync::RwLock` 而非 `tokio::sync::RwLock`**: 读路径是 sync
///   （`WindowSelector::select` 是同步函数），持锁时间极短（~10ns）
///
/// # 线程安全
///
/// `DensityLearnerHolder` 内部 `RwLock` 保证:
/// - 多读单写（`current_policy` 并发读，`update_policy` 独占写）
/// - 持锁时间极短（仅读写 `DensityPolicy` 枚举，~10ns）
/// - 无 await 跨锁（`update_policy` 是 sync 方法，避免 §4.4 反模式 1）
///
/// # 示例
///
/// ```
/// use hcw_window::density_learner::DensityLearnerHolder;
/// use nexus_contracts::{DensityPolicy, DensityTier};
///
/// let holder = DensityLearnerHolder::new();
/// assert!(holder.current_policy().is_static());
///
/// holder.update_policy(DensityPolicy::learned(1, DensityTier::Rho5));
/// assert!(holder.current_policy().is_learned());
/// assert_eq!(holder.current_policy().tier(), DensityTier::Rho5);
/// ```
#[derive(Debug)]
pub struct DensityLearnerHolder {
    /// 当前激活的策略（`RwLock` 保护，读写分离）
    ///
    /// WHY 用 `RwLock` 而非 `Mutex`:
    /// - 读路径（`current_policy`/`select_with_density`）高频且只读
    /// - 写路径（`update_policy`）低频（每秒 < 1 次）
    /// - `RwLock` 允许并发读，避免读路径串行化
    policy: RwLock<DensityPolicy>,
}

impl DensityLearnerHolder {
    /// 创建持有器，初始化为 `DensityPolicy::Static(Rho10)`（fallback）
    ///
    /// WHY 初始化为 Static: C4 合规要求默认行为零变化，
    /// `Rho10`（无稀疏化）等于 v5.0 前的硬编码行为。
    pub fn new() -> Self {
        Self {
            policy: RwLock::new(DensityPolicy::fallback()),
        }
    }

    /// 创建持有器，指定初始策略（便于测试）
    ///
    /// WHY 提供: 单测需要构造特定策略场景（如 Learned 初始状态）
    pub fn with_policy(policy: DensityPolicy) -> Self {
        Self {
            policy: RwLock::new(policy),
        }
    }

    /// 异步下发策略 — 接收 `omega-learner` 学习到的 `DensityPolicy`
    ///
    /// # 设计
    ///
    /// - 校验 `policy.is_valid()`（始终 true，DensityTier 枚举保证）
    /// - 写入 `RwLock`（独占写锁，~10ns）
    /// - 不返回错误: 任何异常（如 PoisonError）静默 fallback 到 Static
    ///
    /// # C4 合规
    ///
    /// 调用方（chimera-cli / quest-engine）在 `omega-learner` panic/超时时
    /// 不调用此方法，`DensityLearnerHolder` 保持上一次的有效策略。
    /// 若需强制回退到 fallback，调用方传入 `DensityPolicy::fallback()`。
    ///
    /// # 参数
    /// - `policy`: 新策略（Static 或 Learned）
    ///
    /// # 示例
    ///
    /// ```
    /// use hcw_window::density_learner::DensityLearnerHolder;
    /// use nexus_contracts::{DensityPolicy, DensityTier};
    ///
    /// let holder = DensityLearnerHolder::new();
    /// holder.update_policy(DensityPolicy::learned(1, DensityTier::Rho2));
    /// assert_eq!(holder.current_policy().tier(), DensityTier::Rho2);
    /// ```
    pub fn update_policy(&self, policy: DensityPolicy) {
        // WHY unwrap_or_default: RwLock poison 时 fallback 到 Static(Rho10)
        // 避免调用方处理 PoisonError（C4 合规：本地 fallback，无错误传播）
        let mut guard = self.policy.write().unwrap_or_else(|p| {
            // PoisonError 时恢复锁并写入 fallback
            let mut guard = p.into_inner();
            *guard = DensityPolicy::fallback();
            guard
        });
        *guard = policy;
    }

    /// 强制回退到 fallback 策略（`Static(Rho10)`）
    ///
    /// WHY 提供: `omega-learner` 触发学习熔断（spec.md:335 S1 灰度成功率降 >2%）
    /// 时，上层调用方调用此方法立即回退到静态策略。
    ///
    /// # 示例
    ///
    /// ```
    /// use hcw_window::density_learner::DensityLearnerHolder;
    /// use nexus_contracts::DensityTier;
    ///
    /// let holder = DensityLearnerHolder::new();
    /// holder.fallback_to_static();
    /// assert!(holder.current_policy().is_static());
    /// assert_eq!(holder.current_policy().tier(), DensityTier::Rho10);
    /// ```
    pub fn fallback_to_static(&self) {
        self.update_policy(DensityPolicy::fallback());
    }

    /// 返回当前激活的策略（快照）
    ///
    /// 返回 `DensityPolicy` 的 Copy（枚举整体 Copy），调用方无需持有锁。
    ///
    /// # 性能
    ///
    /// 读锁 + Copy 枚举，~10ns。热路径调用无性能影响。
    pub fn current_policy(&self) -> DensityPolicy {
        let guard = self.policy.read().unwrap_or_else(|p| p.into_inner());
        *guard
    }

    /// 密度感知选择 — 结合 complexity 与 density policy 选择窗口
    ///
    /// # 算法
    ///
    /// 1. 用 `WindowSelector::select(complexity)` 选择 `WindowTier`（L0/L1/L2/L3）
    /// 2. 从 `current_policy()` 获取 `DensityTier`（ρ 值）
    /// 3. 校验 tier 与 WindowTier 的兼容性:
    ///    - L3 + Rho10 → 自动降级到 Rho05（避免 1M 暴力加载，架构红线）
    ///    - 其他组合 → 保持原 tier
    /// 4. 计算 `actual_load = tier.actual_load(nominal_capacity)`
    ///
    /// # 架构红线保护
    ///
    /// L3 窗口的标称容量是 1M Token，若 tier=Rho10（100% 加载），
    /// 实际加载量 = 1M，违反"禁止 1M 暴力加载"红线。
    /// 本方法检测到此组合时，**自动降级到 Rho05**（5% 加载 = 50K Token）。
    ///
    /// # 参数
    /// - `complexity`: 任务复杂度 ∈ [0, 1]（用于 WindowTier 选择）
    /// - `nominal_capacity`: 标称容量（用于 actual_load 计算）
    ///
    /// # 返回
    /// `(WindowTier, DensityTier, actual_load)`:
    /// - `WindowTier`: 复杂度选择的窗口层级
    /// - `DensityTier`: 校验后的密度档位（可能因架构红线降级）
    /// - `actual_load`: 实际加载容量 = `tier.actual_load(nominal_capacity)`
    ///
    /// # 示例
    ///
    /// ```
    /// use hcw_window::density_learner::DensityLearnerHolder;
    /// use hcw_window::WindowTier;
    /// use nexus_contracts::{DensityPolicy, DensityTier};
    ///
    /// let holder = DensityLearnerHolder::new();
    ///
    /// // L2 窗口 + Rho10（默认 Static），实际加载 = 128K
    /// let (tier, density, load) = holder.select_with_density(0.6, 131072);
    /// assert_eq!(tier, WindowTier::L2);
    /// assert_eq!(density, DensityTier::Rho10);
    /// assert_eq!(load, 131072);
    /// ```
    pub fn select_with_density(
        &self,
        complexity: f32,
        nominal_capacity: usize,
    ) -> (WindowTier, DensityTier, usize) {
        let tier = WindowSelector::select(complexity);
        let mut density = self.current_policy().tier();

        // 架构红线保护: L3 + Rho10 会触发 1M 暴力加载
        // 自动降级到 Rho05（5% 加载 = 50K，远低于 128K 实际加载上限）
        if matches!(tier, WindowTier::L3) && density == DensityTier::Rho10 {
            density = DensityTier::Rho05;
        }

        let actual_load = density.actual_load(nominal_capacity);
        (tier, density, actual_load)
    }

    /// 密度感知选择（带配置） — 使用 `HcwConfig` 的标称容量
    ///
    /// 便捷方法，自动从 `HcwConfig` 获取 `WindowTier` 对应的标称容量。
    ///
    /// # 参数
    /// - `complexity`: 任务复杂度 ∈ [0, 1]
    /// - `config`: HCW 配置（提供各级窗口标称容量）
    ///
    /// # 返回
    /// `(WindowTier, DensityTier, actual_load)`:
    /// - `actual_load` 基于 `tier.capacity(config)` 计算
    pub fn select_with_density_config(
        &self,
        complexity: f32,
        config: &HcwConfig,
    ) -> (WindowTier, DensityTier, usize) {
        let window_tier = WindowSelector::select(complexity);
        let nominal_capacity = window_tier.capacity(config);
        self.select_with_density(complexity, nominal_capacity)
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

impl Default for DensityLearnerHolder {
    /// 默认状态 = `DensityPolicy::Static(Rho10)`（fallback，C4 合规）
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for DensityLearnerHolder {
    /// 克隆持有器（创建新的 RwLock，策略快照独立）
    ///
    /// WHY 提供: `HcwWindow` 可能需要克隆 `DensityLearnerHolder` 用于
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
    use crate::types::HcwConfig;

    // ============================================================
    // 构造与默认值测试
    // ============================================================

    #[test]
    fn test_holder_new_is_static_fallback() {
        let holder = DensityLearnerHolder::new();
        let policy = holder.current_policy();
        assert!(policy.is_static());
        assert_eq!(policy.tier(), DensityTier::Rho10);
    }

    #[test]
    fn test_holder_default_equals_new() {
        let holder1 = DensityLearnerHolder::new();
        let holder2 = DensityLearnerHolder::default();
        assert_eq!(holder1.current_policy(), holder2.current_policy());
    }

    #[test]
    fn test_holder_with_policy_learned() {
        let holder =
            DensityLearnerHolder::with_policy(DensityPolicy::learned(42, DensityTier::Rho2));
        let policy = holder.current_policy();
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(42));
        assert_eq!(policy.tier(), DensityTier::Rho2);
    }

    // ============================================================
    // update_policy 测试
    // ============================================================

    #[test]
    fn test_update_policy_to_learned() {
        let holder = DensityLearnerHolder::new();
        assert!(holder.current_policy().is_static());

        holder.update_policy(DensityPolicy::learned(1, DensityTier::Rho5));
        let policy = holder.current_policy();
        assert!(policy.is_learned());
        assert_eq!(policy.tier(), DensityTier::Rho5);
        assert_eq!(policy.version(), Some(1));
    }

    #[test]
    fn test_update_policy_to_static() {
        let holder =
            DensityLearnerHolder::with_policy(DensityPolicy::learned(1, DensityTier::Rho2));
        assert!(holder.current_policy().is_learned());

        holder.update_policy(DensityPolicy::static_policy(DensityTier::Rho10));
        assert!(holder.current_policy().is_static());
        assert_eq!(holder.current_policy().tier(), DensityTier::Rho10);
    }

    #[test]
    fn test_update_policy_multiple_times() {
        let holder = DensityLearnerHolder::new();

        for version in 1..=5 {
            holder.update_policy(DensityPolicy::learned(version, DensityTier::Rho2));
            assert_eq!(holder.version(), Some(version));
        }
    }

    #[test]
    fn test_fallback_to_static() {
        let holder =
            DensityLearnerHolder::with_policy(DensityPolicy::learned(1, DensityTier::Rho05));
        assert!(holder.is_learned());

        holder.fallback_to_static();
        assert!(holder.current_policy().is_static());
        assert_eq!(holder.current_policy().tier(), DensityTier::Rho10);
    }

    // ============================================================
    // select_with_density 测试
    // ============================================================

    #[test]
    fn test_select_with_density_default_static_rho10() {
        let holder = DensityLearnerHolder::new();
        // complexity 0.6 → L2 窗口，默认 Rho10
        let (tier, density, load) = holder.select_with_density(0.6, 131072);
        assert_eq!(tier, WindowTier::L2);
        assert_eq!(density, DensityTier::Rho10);
        assert_eq!(load, 131072); // 100% 加载
    }

    #[test]
    fn test_select_with_density_learned_rho2() {
        let holder =
            DensityLearnerHolder::with_policy(DensityPolicy::learned(1, DensityTier::Rho2));
        // complexity 0.6 → L2 窗口，学习策略 Rho2（20% 加载）
        let (tier, density, load) = holder.select_with_density(0.6, 131072);
        assert_eq!(tier, WindowTier::L2);
        assert_eq!(density, DensityTier::Rho2);
        assert_eq!(load, 26214); // 131072 × 0.2 = 26214.4 → 26214
    }

    #[test]
    fn test_select_with_density_l0_window() {
        let holder = DensityLearnerHolder::new();
        let (tier, density, load) = holder.select_with_density(0.1, 4096);
        assert_eq!(tier, WindowTier::L0);
        assert_eq!(density, DensityTier::Rho10);
        assert_eq!(load, 4096);
    }

    #[test]
    fn test_select_with_density_l1_window() {
        let holder = DensityLearnerHolder::new();
        let (tier, _, _) = holder.select_with_density(0.3, 32768);
        assert_eq!(tier, WindowTier::L1);
    }

    #[test]
    fn test_select_with_density_l3_window() {
        // L3 窗口 + 学习策略 Rho05（5% 加载）
        let holder =
            DensityLearnerHolder::with_policy(DensityPolicy::learned(1, DensityTier::Rho05));
        let (tier, density, load) = holder.select_with_density(0.9, 1_048_576);
        assert_eq!(tier, WindowTier::L3);
        assert_eq!(density, DensityTier::Rho05);
        assert_eq!(load, 52_428); // 1_048_576 × 0.05
    }

    // ============================================================
    // 架构红线保护测试（L3 + Rho10 自动降级）
    // ============================================================

    #[test]
    fn test_select_with_density_l3_rho10_architecture_guard() {
        // 架构红线: L3 + Rho10 = 1M 暴力加载，必须自动降级到 Rho05
        let holder = DensityLearnerHolder::new(); // 默认 Rho10
        let (tier, density, load) = holder.select_with_density(0.9, 1_048_576);
        assert_eq!(tier, WindowTier::L3);
        // Rho10 应被降级为 Rho05（5% 加载 = 52428，远低于 128K 上限）
        assert_eq!(density, DensityTier::Rho05);
        assert_eq!(load, 52_428);
    }

    #[test]
    fn test_select_with_density_l3_rho2_no_downgrade() {
        // L3 + Rho2（20% 加载 = 200K）不应触发降级（仅 Rho10 触发）
        let holder =
            DensityLearnerHolder::with_policy(DensityPolicy::learned(1, DensityTier::Rho2));
        let (tier, density, load) = holder.select_with_density(0.9, 1_048_576);
        assert_eq!(tier, WindowTier::L3);
        assert_eq!(density, DensityTier::Rho2);
        assert_eq!(load, 209_715); // 1_048_576 × 0.2
    }

    #[test]
    fn test_select_with_density_l3_rho5_no_downgrade() {
        let holder =
            DensityLearnerHolder::with_policy(DensityPolicy::learned(1, DensityTier::Rho5));
        let (tier, density, load) = holder.select_with_density(0.9, 1_048_576);
        assert_eq!(tier, WindowTier::L3);
        assert_eq!(density, DensityTier::Rho5);
        assert_eq!(load, 524_288);
    }

    #[test]
    fn test_select_with_density_l2_rho10_no_downgrade() {
        // L2 + Rho10 不触发降级（L2 容量 128K，全加载合理）
        let holder = DensityLearnerHolder::new();
        let (tier, density, load) = holder.select_with_density(0.6, 131072);
        assert_eq!(tier, WindowTier::L2);
        assert_eq!(density, DensityTier::Rho10);
        assert_eq!(load, 131072);
    }

    // ============================================================
    // select_with_density_config 测试
    // ============================================================

    #[test]
    fn test_select_with_density_config_default() {
        let holder = DensityLearnerHolder::new();
        let config = HcwConfig::default();
        let (tier, density, load) = holder.select_with_density_config(0.6, &config);
        assert_eq!(tier, WindowTier::L2);
        assert_eq!(density, DensityTier::Rho10);
        assert_eq!(load, 131072); // L2 默认容量
    }

    #[test]
    fn test_select_with_density_config_l3_architecture_guard() {
        // L3 + Rho10 自动降级测试（带 config）
        let holder = DensityLearnerHolder::new();
        let config = HcwConfig::default();
        let (tier, density, load) = holder.select_with_density_config(0.9, &config);
        assert_eq!(tier, WindowTier::L3);
        assert_eq!(density, DensityTier::Rho05);
        // L3 默认容量 1_048_576，Rho05 加载 = 52428
        assert_eq!(load, 52_428);
    }

    // ============================================================
    // is_learned / version 测试
    // ============================================================

    #[test]
    fn test_is_learned_static() {
        let holder = DensityLearnerHolder::new();
        assert!(!holder.is_learned());
    }

    #[test]
    fn test_is_learned_learned() {
        let holder =
            DensityLearnerHolder::with_policy(DensityPolicy::learned(1, DensityTier::Rho2));
        assert!(holder.is_learned());
    }

    #[test]
    fn test_version_static_none() {
        let holder = DensityLearnerHolder::new();
        assert_eq!(holder.version(), None);
    }

    #[test]
    fn test_version_learned_some() {
        let holder =
            DensityLearnerHolder::with_policy(DensityPolicy::learned(42, DensityTier::Rho5));
        assert_eq!(holder.version(), Some(42));
    }

    // ============================================================
    // Clone 测试
    // ============================================================

    #[test]
    fn test_clone_independent() {
        let holder1 =
            DensityLearnerHolder::with_policy(DensityPolicy::learned(1, DensityTier::Rho2));
        let holder2 = holder1.clone();

        // 修改 holder1 不影响 holder2
        holder1.update_policy(DensityPolicy::learned(2, DensityTier::Rho10));
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

        let holder = Arc::new(DensityLearnerHolder::new());
        let mut handles = vec![];

        // 启动 4 个写线程
        for i in 0..4 {
            let h_clone = Arc::clone(&holder);
            handles.push(thread::spawn(move || {
                h_clone.update_policy(DensityPolicy::learned(i + 1, DensityTier::Rho2));
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
        assert_eq!(policy.tier(), DensityTier::Rho2);
    }

    // ============================================================
    // 边界场景测试
    // ============================================================

    #[test]
    fn test_select_with_density_zero_capacity() {
        let holder = DensityLearnerHolder::new();
        let (tier, density, load) = holder.select_with_density(0.5, 0);
        assert_eq!(tier, WindowTier::L2);
        assert_eq!(density, DensityTier::Rho10);
        assert_eq!(load, 0); // nominal=0 → actual=0
    }

    #[test]
    fn test_select_with_density_nan_complexity() {
        // NaN 复杂度归为 L0（WindowSelector::select 的行为）
        let holder = DensityLearnerHolder::new();
        let (tier, _, _) = holder.select_with_density(f32::NAN, 4096);
        assert_eq!(tier, WindowTier::L0);
    }

    #[test]
    fn test_select_with_density_extreme_complexity() {
        let holder = DensityLearnerHolder::new();

        // complexity < 0 → L0
        let (tier, _, _) = holder.select_with_density(-0.1, 4096);
        assert_eq!(tier, WindowTier::L0);

        // complexity > 1 → L3（触发架构红线降级）
        let (tier, density, _) = holder.select_with_density(1.5, 1_048_576);
        assert_eq!(tier, WindowTier::L3);
        assert_eq!(density, DensityTier::Rho05); // 降级
    }

    // ============================================================
    // 集成场景: S1 接缝完整流程模拟
    // ============================================================

    #[test]
    fn test_s1_seam_full_integration_flow() {
        // 模拟 S1 接缝完整流程:
        // 1. holder 初始化为 Static fallback
        // 2. omega-learner 异步下发 Learned 策略
        // 3. HcwWindow 使用 select_with_density 选择窗口
        // 4. learner 检测到成功率下降,触发熔断
        // 5. holder 回退到 Static

        let holder = DensityLearnerHolder::new();

        // 初始状态: Static fallback
        assert!(holder.current_policy().is_static());

        // Step 2: omega-learner 下发 Learned 策略（ρ=2，强稀疏化）
        holder.update_policy(DensityPolicy::learned(1, DensityTier::Rho2));
        assert!(holder.is_learned());
        assert_eq!(holder.version(), Some(1));

        // Step 3: select_with_density 选择窗口
        let (tier, density, load) = holder.select_with_density(0.6, 131072);
        assert_eq!(tier, WindowTier::L2);
        assert_eq!(density, DensityTier::Rho2);
        assert_eq!(load, 26214); // 131072 × 0.2

        // Step 4: 触发学习熔断（spec.md:335 S1 灰度成功率降 >2%）
        holder.fallback_to_static();
        assert!(!holder.is_learned());
        assert_eq!(holder.current_policy().tier(), DensityTier::Rho10);

        // Step 5: 回退后再次选择，使用 Static 策略
        let (tier, density, load) = holder.select_with_density(0.6, 131072);
        assert_eq!(tier, WindowTier::L2);
        assert_eq!(density, DensityTier::Rho10);
        assert_eq!(load, 131072); // 100% 加载
    }

    #[test]
    fn test_s1_seam_architecture_redline_protection() {
        // 验证架构红线保护:
        // L3 + Rho10（1M 暴力加载）必须自动降级到 Rho05

        let holder = DensityLearnerHolder::new(); // Static Rho10

        // 复杂度 0.9 → L3 窗口
        let (tier, density, load) = holder.select_with_density(0.9, L3_NOMINAL_CAPACITY);
        assert_eq!(tier, WindowTier::L3);
        // Rho10 被降级为 Rho05
        assert_eq!(density, DensityTier::Rho05);
        // 实际加载 = 1M × 0.05 = 52428（远低于 128K 上限）
        assert_eq!(load, 52_428);
        assert!(
            load < 131_072,
            "actual_load must < 128K (architecture red line)"
        );
    }

    #[test]
    fn test_s1_seam_ab_test_scenario() {
        // 模拟 A/B 测试场景:
        // 版本 1 = Rho2，版本 2 = Rho5，对比两个版本的策略效果

        let holder_v1 =
            DensityLearnerHolder::with_policy(DensityPolicy::learned(1, DensityTier::Rho2));
        let holder_v2 =
            DensityLearnerHolder::with_policy(DensityPolicy::learned(2, DensityTier::Rho5));

        // 相同 complexity 与 nominal_capacity，对比 actual_load
        let (_, _, load_v1) = holder_v1.select_with_density(0.6, 131072);
        let (_, _, load_v2) = holder_v2.select_with_density(0.6, 131072);

        // Rho2 加载 20% = 26214, Rho5 加载 50% = 65536
        assert_eq!(load_v1, 26214);
        assert_eq!(load_v2, 65536);
        assert!(load_v1 < load_v2);
    }
}
