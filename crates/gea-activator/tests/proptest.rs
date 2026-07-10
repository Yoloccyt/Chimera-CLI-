//! gea-activator 不变量属性测试 — 激活操作确定性(幂等性)
//!
//! 对应架构层:L7 Execution(注:activator 实现位于 L6 Router 范畴)
//! 对应创新点:GEA(Gated Expert Activation)
//!
//! # 测试目标
//! 验证相同 TaskProfile + 相同专家注册表 → 相同激活决策(确定性不变量):
//! 1. 两个独立但完全相同的 activator,激活相同任务,结果应相等
//! 2. activated / suppressed / top_gate_value 三字段完全一致
//!
//! # 设计决策
//! - 用 `tokio::runtime::Builder::new_current_thread()` 轻量 runtime:
//!   proptest 用例顺序执行,每用例一个 runtime,32 cases 开销可接受
//! - 两个独立 activator(独立 bus + 独立 registry)避免缓存共享
//! - block_on 同步执行 activate(),绕过 proptest! 不支持 async 的限制
//!
//! # 语法约束(§4.4 规则)
//! proptest 1.11+ 用 block-named 语法

#![forbid(unsafe_code)]

use std::sync::Arc;

use event_bus::EventBus;
use gea_activator::{ActivationResult, ExpertProfile, GeaActivator, GeaConfig, TaskProfile};
use proptest::prelude::*;

/// 生成 [0.0, 1.0] 范围的有限 f32(过滤 NaN/Inf)
fn prop_unit_f32() -> impl Strategy<Value = f32> {
    any::<f32>().prop_map(|v| {
        if v.is_nan() || v.is_infinite() {
            0.5
        } else {
            v.abs().rem_euclid(1.0)
        }
    })
}

/// 生成 64 维 [0.0, 1.0] 向量(与 ExpertProfile::expert_vector 维度一致)
fn prop_vector_64() -> impl Strategy<Value = Vec<f32>> {
    prop::collection::vec(prop_unit_f32(), 64)
}

proptest! {
    // WHY 32 cases:activate() 涉及 runtime 创建 + async 执行,32 cases 足够覆盖
    // 又不会拖慢测试(每 case < 5ms,总 < 200ms)
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn prop_activation_idempotent(
        complexity in prop_unit_f32(),
        task_type_idx in 0u8..3,
        risk_level in 0u8..=100,
        expert_vec in prop_vector_64(),
        priority in prop_unit_f32(),
    ) {
        let task_type = match task_type_idx {
            0 => "code-gen",
            1 => "refactor",
            _ => "test",
        };
        let task = Arc::new(TaskProfile::new(complexity, task_type, risk_level, vec![0.5; 64]));
        let expert = ExpertProfile::new(
            "e-1",
            expert_vec,
            priority,
            vec![task_type.into()],
        );

        // 构造两个独立但完全相同的 activator(独立 bus + 独立 registry)
        let build_activator = || -> GeaActivator {
            let bus = EventBus::new();
            let a = GeaActivator::new(GeaConfig::default(), bus).expect("default config 合法");
            a.register_expert(expert.clone());
            a
        };

        // current_thread runtime 轻量,每用例一个
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime 创建应成功");

        let r1: ActivationResult = rt
            .block_on(async {
                let a = build_activator();
                a.activate(&task).await
            })
            .expect("activate a1 应成功");
        let r2: ActivationResult = rt
            .block_on(async {
                let a = build_activator();
                a.activate(&task).await
            })
            .expect("activate a2 应成功");

        // === 不变量:相同输入 → 相同输出(确定性)===
        prop_assert_eq!(
            r1.activated, r2.activated,
            "activated 应确定性:相同输入产生相同激活列表"
        );
        prop_assert_eq!(
            r1.suppressed, r2.suppressed,
            "suppressed 应确定性:相同输入产生相同抑制列表"
        );
        prop_assert!(
            (r1.top_gate_value - r2.top_gate_value).abs() < 1e-5,
            "top_gate_value 应确定性: {} vs {}",
            r1.top_gate_value,
            r2.top_gate_value
        );
    }
}
