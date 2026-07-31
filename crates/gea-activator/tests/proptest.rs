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

// ============================================================
// resolve_conflicts 早停等价性(L9 优化第二轮)
// ============================================================

use std::collections::{HashMap, HashSet};

use gea_activator::{resolve_conflicts, Candidate, ExpertId};

/// 朴素全扫描参考实现 — 复现早停前的旧算法(全贪心 + select_top_k 裁剪)
///
/// 用于验证 `resolve_conflicts` 的早停优化与全扫描行为等价:
/// activated 序列逐元素一致,suppressed 集合成员一致。
fn reference_full_scan(
    candidates: Vec<Candidate>,
    profiles: &HashMap<ExpertId, ExpertProfile>,
    config: &GeaConfig,
) -> (Vec<String>, HashSet<String>) {
    // 1. 按综合评分(gate × priority)降序
    let mut scored: Vec<(ExpertId, f32)> = candidates
        .into_iter()
        .map(|(id, gate)| {
            let comp = gate * profiles[&id].priority;
            (id, comp)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // 2. 全贪心(无早停):每候选检查 vs 所有已激活(可能 >top_k)
    let mut activated: Vec<(ExpertId, f32)> = Vec::new();
    let mut suppressed: HashSet<String> = HashSet::new();
    for (id, comp) in scored {
        let vec = &profiles[&id].expert_vector;
        let conflict = activated.iter().any(|(aid, _)| {
            nexus_core::cosine_similarity_slices(vec, &profiles[aid].expert_vector)
                > config.overlap_threshold
        });
        if conflict {
            suppressed.insert(id.to_string());
        } else {
            activated.push((id, comp));
        }
    }

    // 3. select_top_k:activated 降序取前 k,其余→suppressed
    activated.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let top: Vec<String> = activated
        .iter()
        .take(config.top_k)
        .map(|(id, _)| id.to_string())
        .collect();
    for (id, _) in activated.iter().skip(config.top_k) {
        suppressed.insert(id.to_string());
    }
    (top, suppressed)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// 早停优化后的 resolve_conflicts 与全扫描参考实现等价
    ///
    /// WHY distinct gate + priority=1.0:使 composite 互异,消除排序平局歧义,
    /// 确保 activated 序列可逐元素比较;冲突由随机 64 维向量 + 阈值驱动。
    #[test]
    fn prop_resolve_conflicts_early_stop_equivalence(
        vectors in prop::collection::vec(prop_vector_64(), 2..10),
        top_k in 1usize..4,
        overlap_threshold in 0.3f32..0.95,
    ) {
        let config = GeaConfig { top_k, overlap_threshold, ..Default::default() };
        let mut profiles: HashMap<ExpertId, ExpertProfile> = HashMap::new();
        let mut candidates: Vec<Candidate> = Vec::new();
        for (i, v) in vectors.into_iter().enumerate() {
            let id = ExpertId::new(format!("e-{i}"));
            // distinct gate(降序互异),priority=1.0 → composite=gate 互异
            let gate = (i as f32 + 1.0) / 100.0;
            profiles.insert(
                id.clone(),
                ExpertProfile::new(format!("e-{i}"), v, 1.0, vec!["t".into()]),
            );
            candidates.push((id, gate));
        }

        let new_result = resolve_conflicts(candidates.clone(), &profiles, &config)
            .expect("resolve_conflicts 应成功");
        let (ref_activated, ref_suppressed) = reference_full_scan(candidates, &profiles, &config);

        let new_activated: Vec<String> =
            new_result.activated.iter().map(|id| id.to_string()).collect();
        let new_suppressed: HashSet<String> =
            new_result.suppressed.iter().map(|id| id.to_string()).collect();

        prop_assert_eq!(new_activated, ref_activated, "activated 序列应与全扫描参考一致");
        prop_assert_eq!(new_suppressed, ref_suppressed, "suppressed 集合应与全扫描参考一致");
    }
}
