//! GSOE 策略模型调用注入测试（Milestone C-2，Week-7 TODO 闭合）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §6 C-2）：
//! fitness/grpo/mutation 占位替换为真实模型调用——经 `PolicyModelInvoker`
//! 注入（编排器在 L10 接线真实模型；L5 不依赖 L10，依赖铁律合规）；
//! 未注入时回退确定性 Lcg 实现（现行为保持不变）。

#![forbid(unsafe_code)]

use gsoe_evolution::policy::fitness::{evaluate_fitness, evaluate_fitness_with_invoker};
use gsoe_evolution::policy::grpo::{sample_rollouts, sample_rollouts_with_invoker};
use gsoe_evolution::policy::model::{DeterministicInvoker, PolicyModelInvoker};
use gsoe_evolution::policy::mutation::mutate_with_direction;
use gsoe_evolution::types::{EvolutionPolicy, GrpoRollout};

/// 未注入 invoker → 回退现行为（与 sample_rollouts 等价）
#[test]
fn fallback_without_invoker_matches_current_behavior() {
    let policy = EvolutionPolicy::default();
    let a = sample_rollouts(&policy, 5);
    let b = sample_rollouts_with_invoker(&policy, 5, None);
    assert_eq!(a.len(), b.len());
    // Lcg 确定性：同 seed 同结果（两条路径均应使用 Lcg(42)）
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.actions, y.actions, "回退路径应保持确定性等价");
        assert_eq!(x.reward, y.reward);
    }
}

/// 注入 DeterministicInvoker → logits 采样路径生效
#[test]
fn injected_invoker_drives_sampling() {
    let policy = EvolutionPolicy::default();
    let invoker = DeterministicInvoker::new(0.5); // 固定 logits 值 0.5
    let rollouts = sample_rollouts_with_invoker(&policy, 3, Some(&invoker));
    assert_eq!(rollouts.len(), 3);
    // 注入后动作 = OPTIMAL_ACTION(1.0) + mutation_rate × logits(0.5)
    let expected = 1.0 + policy.mutation_rate * 0.5;
    for r in &rollouts {
        assert!(
            r.actions.iter().all(|a| (*a - expected).abs() < 1e-6),
            "注入路径应使用 invoker logits: {:?}",
            r.actions
        );
    }
}

/// 未注入 fitness → 规则式 (reward+1)/2（现行为）
#[test]
fn fitness_fallback_uses_rule() {
    let rollout = GrpoRollout::new("t-1".into(), vec![0.0; 4], 1.0);
    let rule = evaluate_fitness(&rollout);
    let fallback = evaluate_fitness_with_invoker(&rollout, None);
    assert_eq!(rule.fitness_score, fallback.fitness_score);
    assert!((rule.fitness_score - 1.0).abs() < 1e-6, "(1+1)/2=1.0");
}

/// 注入 invoker → 模型评判分生效
#[test]
fn fitness_injected_uses_model_judgement() {
    let rollout = GrpoRollout::new("t-2".into(), vec![0.0; 4], 0.5);
    let invoker = DeterministicInvoker::new(0.25); // judge 返回固定 0.25
    let report = evaluate_fitness_with_invoker(&rollout, Some(&invoker));
    assert_eq!(report.fitness_score, 0.25);
}

/// mutation 方向引导：注入方向 → 变异沿方向；未注入 → Lcg 现行为
#[test]
fn mutation_direction_guidance() {
    let policy = EvolutionPolicy::default();
    let invoker = DeterministicInvoker::new(0.75);
    let guided = mutate_with_direction(&policy, 0.5, Some(&invoker)).expect("方向引导应成功");
    assert_eq!(
        guided.magnitude,
        0.5 * 0.75,
        "方向引导应 = rate × direction"
    );

    let unguided = mutate_with_direction(&policy, 0.5, None)
        .expect("回退应成功")
        .magnitude;
    assert!(
        (-0.5..0.5).contains(&unguided),
        "未引导应保持 Lcg 扰动范围: {unguided}"
    );
}

/// PolicyModelInvoker 可动态分发（编排器注入真实模型实现的契约）
#[test]
fn invoker_is_dyn_dispatchable() {
    let invoker: Box<dyn PolicyModelInvoker> = Box::new(DeterministicInvoker::new(0.1));
    let v = invoker.logits(42, 4);
    assert_eq!(v.len(), 4);
    assert!(v.iter().all(|x| (*x - 0.1).abs() < 1e-6));
}
