//! OSA memory 维度自适应记忆策略契约 — S2 桥接层（L0 契约）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应 ADR: **ADR-033**（L0 nexus-contracts 契约层建立）
//! 对应任务: **Task 2**（OSA memory 维度集成 omega-learner S2）
//! 对应三重悖论: **记忆悖论修复**（OSA memory 维度通过 trait 调用 S2，自适应选择记忆策略）
//!
//! # 核心职责
//!
//! 定义 OSA memory 维度与 omega-learner S2 接缝之间的桥接契约：
//! - `MemoryTaskPhase`: 任务阶段枚举（Initial/Stuck/LongRun）
//! - `MemoryStrategyProvider`: 记忆策略提供者 trait
//!
//! WHY 独立模块: `strategy.rs` 承载 S2 接缝的完整契约（`MemoryStrategy` +
//! `MemoryStrategyPolicy`，供 mlc-engine 消费），本模块专注"OSA 集成"的
//! 桥接契约（phase + provider trait，供 OSA 消费），职责正交不混淆。
//!
//! # 依赖铁律合规（§2.2）
//!
//! `osa-coordinator` (L6) 不直接依赖 `omega-learner` (L6)（同层依赖虽允许，
//! 但会产生紧耦合）。通过 L0 trait 解耦：
//! - `omega-learner` 在 L6 实现 `MemoryStrategyProvider` trait
//! - `osa-coordinator` 在 L6 通过 `Arc<dyn MemoryStrategyProvider>` 调用
//! - trait 定义在 L0，两端各自依赖 L0，无 L6→L6 直接依赖
//!
//! 此模式与 `VectorStore` trait（L0 契约，L6 OSA 通过 `Arc<dyn>` 调用实现者）一致。
//!
//! # 三重悖论"记忆悖论"修复路径
//!
//! 三重悖论病理：OSA memory 维度直接复用 routing 的 Top-K 策略（8/16/24/32），
//! 未实现自适应记忆策略，固定 top-k 召回在任务阶段切换时会产生"幽灵记忆"
//! （新旧事实共存无法区分时间有效性）。
//!
//! 修复路径：OSA memory 维度通过 `MemoryStrategyProvider` 调用 omega-learner S2，
//! 根据任务阶段（Initial/Stuck/LongRun）自适应选择记忆策略，再用策略的
//! `k_multiplier()` 调整基础 Top-K，实现记忆策略随任务阶段自适应。

use serde::{Deserialize, Serialize};

// 复用 strategy.rs 的 MemoryStrategy（S2 接缝 5 种记忆策略枚举）
// WHY 复用而非重定义: strategy.rs 已定义完整的 MemoryStrategy（含 k_multiplier
// / default_top_k / similarity_threshold 等方法），重定义会导致类型分裂
use crate::strategy::MemoryStrategy;

// ============================================================
// 任务阶段枚举
// ============================================================

/// 任务阶段 — OSA memory 维度自适应记忆策略的输入信号
///
/// 与 `omega-learner::s2_memory::TaskPhase` 语义对齐（Initial/Stuck/LongRun），
/// 但定义在 L0 避免 OSA 直接依赖 omega-learner (L6)。
///
/// WHY 独立枚举而非复用 omega-learner 的 TaskPhase:
/// - L0 契约层禁止依赖任何 workspace crate（ADR-033）
/// - omega-learner 的 `TaskPhase` 携带 one-hot 编码等 LinUCB 特定逻辑，
///   OSA 不需要这些细节，只需阶段语义
/// - 两端通过 `From` 转换保持映射一致性（适配器负责转换）
///
/// # 三重悖论修复的阶段→策略映射（启发式先验）
///
/// - `Initial` → 偏向 `MinimalRecall`（快速响应，减少噪声）
/// - `Stuck` → 偏向 `QueryReformulation`（多角度查询突破卡壳）
/// - `LongRun` → 偏向 `AggressivePruning`（长跑抑制噪声累积）
///
/// 注意：实际策略由 `MemoryStrategyProvider` 实现者决定（可基于 LinUCB 学习结果），
/// 上述映射仅为启发式先验，非硬编码绑定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum MemoryTaskPhase {
    /// 初始阶段 — 任务刚开始，上下文稀疏，偏向 MinimalRecall
    ///
    /// 特征：DAG 深度浅（0-2），任务尚未展开，快速响应优先
    Initial = 0,

    /// 卡壳阶段 — 任务遇到障碍，需要多角度信息突破
    ///
    /// 特征：连续失败或无进展，需要 QueryReformulation 多查询融合
    Stuck = 1,

    /// 长跑阶段 — 任务持续较久，上下文累积，偏向 AggressivePruning
    ///
    /// 特征：DAG 深度深（5+），记忆条目多，需剪枝抑制噪声
    LongRun = 2,
}

impl MemoryTaskPhase {
    /// 返回所有任务阶段（按枚举值升序）
    pub const ALL: [Self; 3] = [Self::Initial, Self::Stuck, Self::LongRun];

    /// 返回任务阶段简称（用于日志/调试）
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Stuck => "stuck",
            Self::LongRun => "long-run",
        }
    }

    /// 返回任务阶段推荐的默认策略（启发式先验，供 fallback 参考）
    ///
    /// WHY 提供: 此映射对应三重悖论修复路径的阶段→策略启发式先验，
    /// `MemoryStrategyProvider` 实现者无学习模型时可用作"合理默认"。
    /// omega-learner S2 的 `TaskPhase::default_strategy()` 提供相同映射，
    /// 适配器实现时应委托给 S2 学习器而非直接调用此方法。
    pub const fn default_strategy(self) -> MemoryStrategy {
        match self {
            Self::Initial => MemoryStrategy::MinimalRecall,
            Self::Stuck => MemoryStrategy::QueryReformulation,
            Self::LongRun => MemoryStrategy::AggressivePruning,
        }
    }
}

impl Default for MemoryTaskPhase {
    /// 默认阶段 = `Initial`（新任务起始阶段）
    ///
    /// WHY(C4 合规): `None` fallback 时 OSA 用此默认阶段查询策略，
    /// `Initial` 对应 `MinimalRecall`（k_multiplier=0.5），保守召回。
    fn default() -> Self {
        Self::Initial
    }
}

impl std::fmt::Display for MemoryTaskPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

// ============================================================
// 记忆策略提供者 trait
// ============================================================

/// 记忆策略提供者 trait — OSA memory 维度调用 omega-learner S2 的桥接接口
///
/// omega-learner S2 在 L6 实现此 trait（通常通过适配器结构体，因 `S2Learner::select`
/// 需要 `&mut self` 与完整 `S2Context`，与 trait 的 `&self` + 仅 phase 签名不兼容），
/// OSA 在 L6 通过 `Arc<dyn MemoryStrategyProvider>` 调用。
///
/// # 依赖铁律合规（§2.2）
///
/// `osa-coordinator` (L6) 不直接依赖 `omega-learner` (L6)，通过 L0 trait 解耦：
/// - trait 定义在 L0（本模块），两端各自依赖 L0
/// - OSA 持有 `Option<Arc<dyn MemoryStrategyProvider>>`，`None` 时 fallback 到
///   `StandardTopK`（k_multiplier=1.0，当前行为，向后兼容）
/// - 此模式与 `VectorStore` trait（L0 契约）一致
///
/// # C4 合规（能力场灰度，非运行时旗）
///
/// OSA 未注入 provider 时（`None`），memory 维度使用 `StandardTopK`（编译进二进制的
/// const 常量），行为与 S2 集成前完全一致。provider 注入是构造时一次性操作，
/// 非运行时 feature flag 查询。
///
/// # 线程安全
///
/// `Send + Sync` 约束使 OSA 可跨 async 任务共享 provider（`Arc<dyn>` 要求）。
/// 实现者内部如需可变状态（如 `S2Learner` 的 `&mut self`），用 `Mutex` 包裹。
pub trait MemoryStrategyProvider: Send + Sync {
    /// 根据任务阶段选择记忆策略
    ///
    /// # 参数
    /// - `phase`: 当前任务阶段（Initial/Stuck/LongRun）
    ///
    /// # 返回
    /// 5 种记忆策略之一（`MemoryStrategy`），调用方用 `k_multiplier()` 调整基础 Top-K
    ///
    /// # 实现建议
    ///
    /// - **有学习模型时**: 调用 LinUCB `select`（用合理默认 context），利用学习结果
    /// - **无学习模型时**: 返回 `phase.default_strategy()`（启发式先验）
    /// - **panic/错误时**: 调用方本地 fallback 到 `StandardTopK`（C4 合规）
    fn select_strategy(&self, phase: MemoryTaskPhase) -> MemoryStrategy;
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // ============================================================
    // MemoryTaskPhase 测试
    // ============================================================

    #[test]
    fn test_phase_all_returns_three() {
        let all = MemoryTaskPhase::ALL;
        assert_eq!(all.len(), 3);
        assert!(all.contains(&MemoryTaskPhase::Initial));
        assert!(all.contains(&MemoryTaskPhase::Stuck));
        assert!(all.contains(&MemoryTaskPhase::LongRun));
    }

    #[test]
    fn test_phase_short_name() {
        assert_eq!(MemoryTaskPhase::Initial.short_name(), "initial");
        assert_eq!(MemoryTaskPhase::Stuck.short_name(), "stuck");
        assert_eq!(MemoryTaskPhase::LongRun.short_name(), "long-run");
    }

    #[test]
    fn test_phase_display() {
        assert_eq!(format!("{}", MemoryTaskPhase::Initial), "initial");
        assert_eq!(format!("{}", MemoryTaskPhase::Stuck), "stuck");
        assert_eq!(format!("{}", MemoryTaskPhase::LongRun), "long-run");
    }

    #[test]
    fn test_phase_default_is_initial() {
        assert_eq!(MemoryTaskPhase::default(), MemoryTaskPhase::Initial);
    }

    #[test]
    fn test_phase_default_strategy_mapping() {
        // 验证三重悖论修复路径的阶段→策略启发式先验
        assert_eq!(
            MemoryTaskPhase::Initial.default_strategy(),
            MemoryStrategy::MinimalRecall
        );
        assert_eq!(
            MemoryTaskPhase::Stuck.default_strategy(),
            MemoryStrategy::QueryReformulation
        );
        assert_eq!(
            MemoryTaskPhase::LongRun.default_strategy(),
            MemoryStrategy::AggressivePruning
        );
    }

    #[test]
    fn test_phase_copy_semantics() {
        let p1 = MemoryTaskPhase::Stuck;
        let p2 = p1; // Copy
        assert_eq!(p1, p2); // p1 仍可用（Copy 非 Move）
    }

    #[test]
    fn test_phase_equality() {
        assert_eq!(MemoryTaskPhase::Initial, MemoryTaskPhase::Initial);
        assert_ne!(MemoryTaskPhase::Initial, MemoryTaskPhase::Stuck);
        assert_ne!(MemoryTaskPhase::Stuck, MemoryTaskPhase::LongRun);
    }

    #[test]
    fn test_phase_serialize_json() {
        let phase = MemoryTaskPhase::LongRun;
        let json = serde_json::to_string(&phase).unwrap();
        let deserialized: MemoryTaskPhase = serde_json::from_str(&json).unwrap();
        assert_eq!(phase, deserialized);
    }

    // ============================================================
    // MemoryStrategyProvider trait 测试（用 mock 实现验证契约）
    // ============================================================

    /// Mock 实现：直接返回 phase 的 default_strategy（启发式先验）
    struct MockHeuristicProvider;

    impl MemoryStrategyProvider for MockHeuristicProvider {
        fn select_strategy(&self, phase: MemoryTaskPhase) -> MemoryStrategy {
            phase.default_strategy()
        }
    }

    /// Mock 实现：始终返回 StandardTopK（模拟 fallback）
    struct MockFallbackProvider;

    impl MemoryStrategyProvider for MockFallbackProvider {
        fn select_strategy(&self, _phase: MemoryTaskPhase) -> MemoryStrategy {
            MemoryStrategy::StandardTopK
        }
    }

    #[test]
    fn test_provider_heuristic_returns_phase_default() {
        let provider = MockHeuristicProvider;
        assert_eq!(
            provider.select_strategy(MemoryTaskPhase::Initial),
            MemoryStrategy::MinimalRecall
        );
        assert_eq!(
            provider.select_strategy(MemoryTaskPhase::Stuck),
            MemoryStrategy::QueryReformulation
        );
        assert_eq!(
            provider.select_strategy(MemoryTaskPhase::LongRun),
            MemoryStrategy::AggressivePruning
        );
    }

    #[test]
    fn test_provider_fallback_returns_standard() {
        let provider = MockFallbackProvider;
        for phase in MemoryTaskPhase::ALL {
            assert_eq!(
                provider.select_strategy(phase),
                MemoryStrategy::StandardTopK
            );
        }
    }

    #[test]
    fn test_provider_send_sync() {
        // 验证 trait object 满足 Send + Sync（Arc<dyn> 要求）
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MockHeuristicProvider>();
        assert_send_sync::<MockFallbackProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        // 验证可作为 Arc<dyn MemoryStrategyProvider> 使用
        let provider: Arc<dyn MemoryStrategyProvider> = Arc::new(MockHeuristicProvider);
        let strategy = provider.select_strategy(MemoryTaskPhase::LongRun);
        assert_eq!(strategy, MemoryStrategy::AggressivePruning);
    }
}
