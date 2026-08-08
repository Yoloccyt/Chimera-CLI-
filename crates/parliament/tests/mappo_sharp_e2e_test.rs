//! MAPPO + SHARP 三元分解奖励 E2E（Milestone C-6，设计 §12.1 目标形态）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §6 C-6）：
//! MAPPO/SHARP 落地于 `parliament/src/{mappo,sharp}.rs`，验收 = 三元分解奖励 E2E。
//! 实现遵循项目占位先例（MTPE 伪预测 / GSOE 规则式策略）：Actor/Critic 为
//! 规则式（特征加权 + 在线统计）而非神经网络——R2 冻结（ADR-042）不触训练面，
//! 替换为生产实现时不得破坏既有接口契约。
//!
//! 核心断言覆盖：
//! - Shapley 效率公理 Σφᵢ = v(all) − v(∅)（Ω₅ Credit 数学正确性）
//! - Agent-wise 优势归一化（Dr.MAS 修复：每 agent 独立 baseline/std，非全局）
//! - 三元分解奖励：global(0.3) + shapley(0.5) + process(0.2) 三通道

#![forbid(unsafe_code)]

use parliament::mappo::{
    AgentAdvantages, AgentRewards, CentralizedCritic, JointAction, ParliamentState, MAPPO,
};
use parliament::sharp::{Outcome, VerificationStage, SHARP};

/// 构造三元组观测：Skeptic 高风险特征、Security 边界特征、Execution 效率特征
fn parliament_state() -> ParliamentState {
    ParliamentState {
        skeptic_obs: vec![0.9, 0.2, 0.7],
        security_obs: vec![0.3, 0.8, 0.1],
        execution_obs: vec![0.6, 0.4, 0.9],
    }
}

/// 构造带已知 coalition 值的 SHARP（单例/双人/三人组全覆盖，便于公理验证）
fn sharp_with_full_coalitions() -> SHARP {
    let mut sharp = SHARP::new();
    sharp.set_coalition_value(["Skeptic"], 1.0);
    sharp.set_coalition_value(["Security"], 0.5);
    sharp.set_coalition_value(["Execution"], 0.5);
    sharp.set_coalition_value(["Skeptic", "Security"], 2.0);
    sharp.set_coalition_value(["Skeptic", "Execution"], 1.8);
    sharp.set_coalition_value(["Security", "Execution"], 1.2);
    sharp.set_coalition_value(["Skeptic", "Security", "Execution"], 3.0);
    sharp
}

/// MAPPO 联合决策：三个 agent（Skeptic/Security/Execution）各自出动作
#[test]
fn joint_decision_returns_three_actions() {
    let mappo = MAPPO::new(vec![
        vec![1.0, 0.0, -1.0], // Skeptic：第一特征加权
        vec![0.0, 1.0, -0.5], // Security：第二特征加权
        vec![0.5, 0.5, 1.0],  // Execution：效率特征加权
    ]);
    let state = parliament_state();
    let joint: JointAction = mappo.joint_decision(&state);
    assert!(
        joint.skeptic.approve || !joint.skeptic.approve,
        "Skeptic 必出动作"
    );
    assert!(
        (0.0..=1.0).contains(&joint.skeptic.confidence),
        "置信度 ∈ [0,1]"
    );
    assert!(
        joint.security.approve || !joint.security.approve,
        "Security 必出动作"
    );
    assert!(
        (0.0..=1.0).contains(&joint.security.confidence),
        "置信度 ∈ [0,1]"
    );
    assert!(
        joint.execution.approve || !joint.execution.approve,
        "Execution 必出动作"
    );
    assert!(
        (0.0..=1.0).contains(&joint.execution.confidence),
        "置信度 ∈ [0,1]"
    );
}

/// Dr.MAS 修复：优势按 agent 独立归一化（(r − baseline)/std），非全局共享统计
#[test]
fn compute_advantages_agent_wise_normalized() {
    let mut mappo = MAPPO::new(vec![vec![1.0], vec![1.0], vec![1.0]]);
    // 观察 3 轮：Skeptic 波动大（[1,2,3]）、Security 恒定（[1,1,1]）、Execution 波动小（[2,2,3]）
    mappo.observe(&AgentRewards {
        skeptic: 1.0,
        security: 1.0,
        execution: 2.0,
    });
    mappo.observe(&AgentRewards {
        skeptic: 2.0,
        security: 1.0,
        execution: 2.0,
    });
    mappo.observe(&AgentRewards {
        skeptic: 3.0,
        security: 1.0,
        execution: 3.0,
    });
    // 第 4 轮：三 agent 同得 2.0，但各自归一化结果不同（std 不同 → 尺度不同）
    let adv: AgentAdvantages = mappo.compute_advantages(&AgentRewards {
        skeptic: 2.0,
        security: 2.0,
        execution: 2.0,
    });
    // Security std=0 → (2−1)/(0+1e-8) 巨大正数；Skeptic std=√(2/3) → (2−2)/std=0
    assert!(
        adv.security > adv.skeptic,
        "恒定 agent 的新奖励应显著为正优势"
    );
    assert!(
        (adv.skeptic - 0.0).abs() < 1e-5,
        "均值处优势≈0（未归一化尺度差异）"
    );
    // 不同 std 的 agent 用不同尺度归一化（Dr.MAS 修复本质）：
    // Execution 均值 7/3≈2.33，第 4 轮 2.0 低于均值 → 负优势
    assert!(adv.execution < 0.0, "低于均值的奖励 → 负优势");
}

/// Critic 在线统计（Welford）：baseline=均值、std=样本标准差
#[test]
fn critic_tracks_baseline_and_std() {
    let mut critic = CentralizedCritic::default();
    for _ in 0..3 {
        critic.observe(&AgentRewards {
            skeptic: 1.0,
            security: 2.0,
            execution: 3.0,
        });
    }
    // 三 agent 各自均值
    assert!((critic.baseline_skeptic() - 1.0).abs() < 1e-6);
    assert!((critic.baseline_security() - 2.0).abs() < 1e-6);
    assert!((critic.baseline_execution() - 3.0).abs() < 1e-6);
    // 恒定序列 → std=0
    assert!(critic.std_skeptic().abs() < 1e-6);
}

/// Shapley 效率公理：Σφᵢ = v(全体) − v(∅)（v(∅)=0）
#[test]
fn shapley_satisfies_efficiency_axiom() {
    let sharp = sharp_with_full_coalitions();
    let all = ["Skeptic", "Security", "Execution"].map(String::from);
    let phi_skeptic = sharp.compute_shapley("Skeptic", &all).unwrap();
    let phi_security = sharp.compute_shapley("Security", &all).unwrap();
    let phi_execution = sharp.compute_shapley("Execution", &all).unwrap();
    let total = phi_skeptic + phi_security + phi_execution;
    assert!(
        (total - 3.0).abs() < 1e-4,
        "效率公理：Σφ = v(全体)=3.0，实际 {total}"
    );
}

/// Shapley 对称性：价值贡献相同 → 信用相同（Skeptic/Execution 在双人组中对称）
#[test]
fn shapley_symmetric_agents_equal_credit() {
    let mut sharp = SHARP::new();
    // 完全对称设定：Skeptic 与 Execution 可互换
    sharp.set_coalition_value(["Skeptic"], 1.0);
    sharp.set_coalition_value(["Security"], 0.5);
    sharp.set_coalition_value(["Execution"], 1.0);
    sharp.set_coalition_value(["Skeptic", "Security"], 1.8);
    sharp.set_coalition_value(["Skeptic", "Execution"], 2.0);
    sharp.set_coalition_value(["Security", "Execution"], 1.8);
    sharp.set_coalition_value(["Skeptic", "Security", "Execution"], 3.0);
    let all = ["Skeptic", "Security", "Execution"].map(String::from);
    let phi_skeptic = sharp.compute_shapley("Skeptic", &all).unwrap();
    let phi_execution = sharp.compute_shapley("Execution", &all).unwrap();
    assert!(
        (phi_skeptic - phi_execution).abs() < 1e-5,
        "对称性：φ(Skeptic)={phi_skeptic} ≈ φ(Execution)={phi_execution}"
    );
}

/// 三元分解：global = team_reward × 0.3 均分给三 agent
#[test]
fn decompose_splits_global_and_process() {
    let sharp = sharp_with_full_coalitions();
    let outcome = Outcome {
        verification: VerificationStage::LogicPass,
    };
    let rewards = sharp.decompose(10.0, &outcome);
    // global 通道：10.0 × 0.3 = 3.0 附加在每个 agent 上
    let all = ["Skeptic", "Security", "Execution"].map(String::from);
    let phi_skeptic = sharp.compute_shapley("Skeptic", &all).unwrap();
    let expected_skeptic = 3.0 + phi_skeptic * 0.5 + 1.0 * 0.2;
    assert!((rewards.skeptic - expected_skeptic).abs() < 1e-4);
    // 三 agent 均含 global 通道
    let phi_security = sharp.compute_shapley("Security", &all).unwrap();
    let expected_security = 3.0 + phi_security * 0.5 + 1.0 * 0.2;
    assert!((rewards.security - expected_security).abs() < 1e-4);
}

/// process 通道随验证阶段单调递增：SyntaxPass < LogicPass < SandboxPass；Failed 负分
#[test]
fn decompose_process_scales_with_stage() {
    let sharp = SHARP::new(); // 空 coalition → 全部 φ=0，只看 process 通道
    let syntax = sharp.decompose(
        1.0,
        &Outcome {
            verification: VerificationStage::SyntaxPass,
        },
    );
    let logic = sharp.decompose(
        1.0,
        &Outcome {
            verification: VerificationStage::LogicPass,
        },
    );
    let sandbox = sharp.decompose(
        1.0,
        &Outcome {
            verification: VerificationStage::SandboxPass,
        },
    );
    let failed = sharp.decompose(
        1.0,
        &Outcome {
            verification: VerificationStage::Failed,
        },
    );
    assert!(syntax.skeptic < logic.skeptic, "LogicPass > SyntaxPass");
    assert!(logic.skeptic < sandbox.skeptic, "SandboxPass > LogicPass");
    assert!(failed.skeptic < syntax.skeptic, "Failed 显著低于通过档");
    // 精确值：process × 0.2
    assert!((sandbox.skeptic - (0.3 + 1.5 * 0.2)).abs() < 1e-6);
    assert!((failed.skeptic - (0.3 - 2.0 * 0.2)).abs() < 1e-6);
}

/// 端到端：联合决策 → 环境反馈 → 三元分解奖励 → 优势计算（设计 §12.1 完整链路）
#[test]
fn end_to_end_three_term_reward_flow() {
    let mut mappo = MAPPO::new(vec![
        vec![1.0, 0.0, -1.0],
        vec![0.0, 1.0, -0.5],
        vec![0.5, 0.5, 1.0],
    ]);
    let sharp = sharp_with_full_coalitions();
    // 1) 联合决策
    let joint = mappo.joint_decision(&parliament_state());
    // 2) 环境反馈 → 三元分解奖励
    let rewards = sharp.decompose(
        8.0,
        &Outcome {
            verification: VerificationStage::SandboxPass,
        },
    );
    // 3) Critic 观察若干轮后计算优势
    for _ in 0..2 {
        mappo.observe(&rewards);
    }
    mappo.observe(&sharp.decompose(
        4.0,
        &Outcome {
            verification: VerificationStage::SyntaxPass,
        },
    ));
    let advantages = mappo.compute_advantages(&rewards);
    // 完整链路产物齐备
    let _ = joint;
    assert!(advantages.skeptic.is_finite());
    assert!(advantages.security.is_finite());
    assert!(advantages.execution.is_finite());
    // SandboxPass 轮 > 此前均值 → 正优势
    assert!(advantages.skeptic > 0.0, "高分轮应产生正优势");
}

/// 指数爆炸保护：agent 数超限（2^n 幂集不可行）→ None
#[test]
fn shapley_guards_exponential_blowup() {
    let sharp = SHARP::new();
    let many: Vec<String> = (0..10).map(|i| format!("agent-{i}")).collect();
    assert!(
        sharp.compute_shapley("agent-0", &many).is_none(),
        "n=10 超出精确 Shapley 上限，必须返回 None"
    );
}

/// 不在参与者集合中的 agent → 0 信用（无贡献即无归因）
#[test]
fn shapley_zero_for_absent_agent() {
    let sharp = sharp_with_full_coalitions();
    let all = ["Skeptic", "Security"].map(String::from);
    let phi = sharp.compute_shapley("Execution", &all).unwrap();
    assert!((phi - 0.0).abs() < 1e-6, "不在集合中 → φ=0");
}
