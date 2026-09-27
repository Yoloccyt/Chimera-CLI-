//! CRATE-CONTRACT BEGIN
//! LAYER:    L6
//! ROLE:     L6 Router Traits — 路由层公共 trait 契约(星型耦合消除的抽象层)
//! BACKEND:  none
//! PRODUCERS: 0 -
//! CONSUMERS: 1 osa-coordinator
//! MATURITY: TRUE
//! CRATE-CONTRACT END
//! L6 Router Traits — 路由层公共 trait 契约(星型耦合消除的抽象层)
//!
//! ★ Insight: 将 osa-coordinator 的具体能力抽象为 trait,使未来路由消费方
//! 依赖抽象而非具体实现(依赖倒置 DIP)。本 crate 是纯 trait/类型契约层,
//! 不含运行时逻辑。
//!
//! # 内容
//! - `SparseMaskProvider` — OSA 稀疏掩码提供者接口(实现方:osa-coordinator)
//! - `RouterConfig` — 路由配置契约(router_id / priority_weight)
//! - `RouterId` — 路由组件身份枚举(事件元数据/日志归属用)
//!
//! # 收缩记录(2026-09-20,架构减法准则)
//! 原 `security` 模块(`ZeroOrphanGuarantee`/`OrphanDetector`)已删除——其设立
//! 前提("gqep-executor L7→qeep-protocol L4 跨层违规")被 P0 治理批次证据推翻
//! (依赖铁律允许向下边;ADR-048 例外已于 M12/wave-3c 收编为正常向下边,
//! 见 `scripts/crate_reachability_freeze.txt` M12 注记与 ADR-185)。该模块为
//! 零实现、零消费的纯死抽象,且与 qeep-protocol 具体结构 `OrphanDetector`
//! 构成无关联同名冲突。若未来出现真实的 trait 化消费需求(如 L7 可测试性
//! 改造),从 git 历史恢复并补齐双端接线即可。
//!
//! # 依赖方向
//! - 生产依赖:nexus-contracts (L0) ✓ 向下依赖允许(L6 → L0 恒合法,ADR-033)
//! - 实现方:osa-coordinator(SparseMaskProvider + RouterConfig)
//! - 消费方:待批次 C 决策(v2.31 前无消费方则评估整 crate 收缩)

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod config;
pub mod masks;

pub use config::{RouterConfig, RouterId};
pub use masks::{SparseMaskError, SparseMaskProvider};

// ============================================================
// 单元测试 — trait 契约行为与 mock 可用性(覆盖率门槛配套)
// ============================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use nexus_contracts::{
        FileId, MemoryId, OmniSparseMasks, OperationId, SparseMask, TaskId, ToolId,
    };
    use std::sync::{Arc, Mutex};

    /// 构造五维全空掩码(与 osa-coordinator fallback 同构)
    fn empty_masks() -> OmniSparseMasks {
        OmniSparseMasks::new(
            SparseMask::<ToolId>::empty(),
            SparseMask::<FileId>::empty(),
            SparseMask::<MemoryId>::empty(),
            SparseMask::<OperationId>::empty(),
            SparseMask::<TaskId>::empty(),
        )
    }

    /// MockProvider — 验证 trait 可被下游以泛型/mock 方式消费(可测试性收益实证)
    struct MockProvider {
        masks: OmniSparseMasks,
        updates: Arc<Mutex<u32>>,
    }

    impl SparseMaskProvider for MockProvider {
        fn tool_masks(&self) -> &OmniSparseMasks {
            &self.masks
        }

        fn update_masks_if_needed(&mut self) -> bool {
            // E-5 opt-in:poison-tolerant 取锁(仓内惯例)，不引入 unwrap_used/expect_used 告警
            *self.updates.lock().unwrap_or_else(|e| e.into_inner()) += 1;
            true
        }
    }

    /// 默认实现 MockConfig — 仅实现必填方法 router_id
    struct MockConfig;

    impl RouterConfig for MockConfig {
        fn router_id(&self) -> RouterId {
            RouterId::Kvbsr
        }
    }

    #[test]
    fn sparse_mask_provider_default_update_returns_false() {
        struct Static(MaskHolder);
        struct MaskHolder(OmniSparseMasks);
        impl SparseMaskProvider for Static {
            fn tool_masks(&self) -> &OmniSparseMasks {
                &self.0 .0
            }
            // update_masks_if_needed 不覆盖 → 走默认实现
        }
        let mut p = Static(MaskHolder(empty_masks()));
        assert!(
            !p.update_masks_if_needed(),
            "默认 update_masks_if_needed 必须返回 false(静态掩码语义)"
        );
    }

    #[test]
    fn sparse_mask_provider_object_safe_via_box() {
        // trait 对象安全性:Box<dyn SparseMaskProvider> 可构造即可消费
        let boxed: Box<dyn SparseMaskProvider> = Box::new(MockProvider {
            masks: empty_masks(),
            updates: Arc::new(Mutex::new(0)),
        });
        assert_eq!(boxed.tool_masks().routing.active_count(), 0);
    }

    #[test]
    fn router_config_defaults() {
        // 默认 priority_weight = 1.0(仅实现 router_id 的最小实现)
        assert_eq!(MockConfig.priority_weight(), 1.0);
    }

    #[test]
    fn router_id_as_str_covers_all_variants() {
        // 全变体穷举 match(新增变体不更新此测试即编译错误 — 防漂移锁)
        let cases = [
            (RouterId::OsCoordinator, "osa-coordinator"),
            (RouterId::Kvbsr, "kvbsr-router"),
            (RouterId::Faae, "faae-router"),
            (RouterId::Sesa, "sesa-router"),
            (RouterId::Gea, "gea-activator"),
            (RouterId::OmegaLearner, "omega-learner"),
            (RouterId::Custom("x".into()), "x"),
            (RouterId::Unknown, "unknown"),
        ];
        for (id, expect) in cases {
            assert_eq!(id.as_str(), expect, "RouterId::as_str 映射漂移");
        }
    }

    #[test]
    fn router_id_default_is_unknown() {
        assert_eq!(RouterId::default(), RouterId::Unknown);
    }

    #[test]
    fn sparse_mask_error_display_is_actionable() {
        // 错误信息可行动(ACI 准则:错误必须可读、可定位)
        let e1 = SparseMaskError::UpdateFailed("timeout".into());
        assert_eq!(e1.to_string(), "mask update failed: timeout");
        let e2 = SparseMaskError::EmptyMasks;
        assert_eq!(e2.to_string(), "masks are empty");
    }
}
