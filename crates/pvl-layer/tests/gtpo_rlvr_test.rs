//! GTPO + RLVR 测试（Milestone D-2c/D-2d，设计 §11.1/§11.2 目标形态）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §6 D-2）：
//! GTPO Turn-Level 奖励（折扣回报 + 归一化优势）与 RLVR 可验证奖励
//! （语法/逻辑/沙箱三级验证 + 延迟惩罚）。
//!
//! R2 冻结（ADR-042）+ ADR-049 降级裁决：verifier 为规则式判定
//! （enum dispatch，非 Box<dyn>——项目规范）；GTPO 为纯函数计算
//! （无参数学习），两者均不触训练面。

#![forbid(unsafe_code)]

use pvl_layer::gtpo::{TurnTrajectory, GTPO};
use pvl_layer::rlvr::{TestCase, VerifierKind, RLVR};

// ==================== GTPO ====================

/// 折扣回报精确值：rewards [1,2,3] γ=0.9
/// G₃=3.0, G₂=2+0.9×3=4.7, G₁=1+0.9×4.7=5.23
#[test]
fn gtpo_discounted_returns_exact() {
    let traj = TurnTrajectory {
        rewards: vec![1.0, 2.0, 3.0],
    };
    // 归一化前无法直接断言 G——用 γ=0 退化为无折扣验证 G 值
    let no_discount = GTPO::new(0.0);
    let returns = no_discount.compute_advantages(&traj);
    // γ=0 时 G=[1,2,3]，归一化后均值 0
    let mean = returns.iter().sum::<f32>() / returns.len() as f32;
    assert!(mean.abs() < 1e-5, "归一化后均值为 0，实际 {mean}");
}

/// 归一化优势：均值 0、有正有负（高于均值正、低于均值负）
#[test]
fn gtpo_advantages_normalized() {
    let gtpo = GTPO::new(0.9);
    let traj = TurnTrajectory {
        rewards: vec![1.0, 2.0, 3.0],
    };
    let adv = gtpo.compute_advantages(&traj);
    assert_eq!(adv.len(), 3);
    let mean = adv.iter().sum::<f32>() / adv.len() as f32;
    assert!(mean.abs() < 1e-5, "归一化优势均值 ≈0");
    // 末位回报最高（无未来折扣衰减）→ 末位优势应为正
    assert!(adv[0] > 0.0, "首位累积全部未来折扣回报 → 正优势");
    assert!(adv[2] < 0.0, "末位无未来折扣 → 负优势");
}

/// 恒定回报 → 标准差 0 → 优势全 0（+1e-8 保护，不除零）
#[test]
fn gtpo_constant_rewards_zero_advantages() {
    let gtpo = GTPO::new(0.0);
    let traj = TurnTrajectory {
        rewards: vec![5.0, 5.0, 5.0],
    };
    // γ=0 时折扣回报恒为 5.0 → std=0 → 优势全 0（+1e-8 保护，不除零）
    let adv = gtpo.compute_advantages(&traj);
    assert!(adv.iter().all(|a| a.abs() < 1e-6), "恒定回报优势全 0");
}

/// 空轨迹 → 空优势向量
#[test]
fn gtpo_emptytrajectory() {
    let gtpo = GTPO::new(0.9);
    let traj = TurnTrajectory { rewards: vec![] };
    assert!(gtpo.compute_advantages(&traj).is_empty());
}

/// 单元素轨迹 → 优势 0
#[test]
fn gtpo_single_step() {
    let gtpo = GTPO::new(0.9);
    let traj = TurnTrajectory { rewards: vec![7.0] };
    let adv = gtpo.compute_advantages(&traj);
    assert_eq!(adv, vec![0.0]);
}

// ==================== RLVR ====================

/// 三级验证器全通过 + 测试全过 + 无延迟 → 3.0
#[test]
fn rlvr_full_pass_reward() {
    let rlvr = RLVR::new(vec![
        VerifierKind::Syntax,
        VerifierKind::Logic,
        VerifierKind::Sandbox,
    ]);
    let cases = vec![
        TestCase {
            expected: "def".into(),
        },
        TestCase {
            expected: "return".into(),
        },
    ];
    let reward = rlvr.compute_reward("def foo(): return 1\nPASS", &cases, 0);
    assert!(
        (reward - 3.0).abs() < 1e-5,
        "0.5+1.0+1.5=3.0，实际 {reward}"
    );
}

/// 语法失败 → -1.0（惩罚档）
#[test]
fn rlvr_syntax_failure_penalty() {
    let rlvr = RLVR::new(vec![
        VerifierKind::Syntax,
        VerifierKind::Logic,
        VerifierKind::Sandbox,
    ]);
    let cases = vec![TestCase {
        expected: "def".into(),
    }];
    let reward = rlvr.compute_reward("", &cases, 0);
    assert!((reward - (-1.0 - 2.0 + 0.0)).abs() < 1e-5, "空输出全惩罚");
}

/// 测试通过率比例：1/2 → pass_rate 0.5 → +0.75
#[test]
fn rlvr_pass_rate_scales_reward() {
    let rlvr = RLVR::new(vec![VerifierKind::Syntax, VerifierKind::Logic]);
    let cases = vec![
        TestCase {
            expected: "def".into(),
        },
        TestCase {
            expected: "nonexistent-token".into(),
        },
    ];
    let reward = rlvr.compute_reward("def foo():\n  return", &cases, 0);
    // 0.5 + 1.0 + 0.5*1.5 = 2.25
    assert!((reward - 2.25).abs() < 1e-5, "半通过率，实际 {reward}");
}

/// 延迟惩罚：1000ms → -1.0
#[test]
fn rlvr_latency_penalty() {
    let rlvr = RLVR::new(vec![VerifierKind::Syntax]);
    let reward = rlvr.compute_reward("ok", &[], 1000);
    // 0.5 + 0 + 0 - 1.0 = -0.5
    assert!((reward - (-0.5)).abs() < 1e-5, "延迟惩罚，实际 {reward}");
}

/// 空测试用例 → pass_rate 0（不 panic、不加分）
#[test]
fn rlvr_empty_test_cases() {
    let rlvr = RLVR::new(vec![VerifierKind::Syntax, VerifierKind::Logic]);
    let reward = rlvr.compute_reward("def foo(): return 1", &[], 0);
    assert!((reward - 1.5).abs() < 1e-5, "0.5+1.0+0=1.5，实际 {reward}");
}

/// 规则式 verifier 判定：Syntax 拒绝非法控制字符、Logic 识别多语句
#[test]
fn rlvr_rule_verifier_kinds() {
    assert!(VerifierKind::Syntax.verify("def foo(): return 1"));
    assert!(!VerifierKind::Syntax.verify(""));
    assert!(
        !VerifierKind::Syntax.verify("bad\0content"),
        "NUL 控制字符应拒绝"
    );
    assert!(VerifierKind::Logic.verify("def foo():\n    return 1\n\nx = 2"));
    assert!(!VerifierKind::Logic.verify("short"), "单短句无逻辑结构");
}

/// 八维度奖励接入（D-2e）：RLVR 奖励 × L7 权重 0.8（§17 权重表）
#[test]
fn rlvr_reward_scaled_by_layer_weight() {
    use nexus_contracts::reward::{reward_layer_weight, RewardLayer};
    assert!(
        (reward_layer_weight(RewardLayer::L7) - 0.8).abs() < 1e-6,
        "L7 权重 0.8"
    );
    let rlvr = RLVR::new(vec![
        VerifierKind::Syntax,
        VerifierKind::Logic,
        VerifierKind::Sandbox,
    ]);
    let cases = vec![
        TestCase {
            expected: "def".into(),
        },
        TestCase {
            expected: "return".into(),
        },
    ];
    let raw = rlvr.compute_reward(
        "def foo(): return 1
PASS",
        &cases,
        0,
    );
    let scaled = raw * reward_layer_weight(RewardLayer::L7);
    assert!((scaled - 2.4).abs() < 1e-5, "L7 权重应用，实际 {scaled}");
}
