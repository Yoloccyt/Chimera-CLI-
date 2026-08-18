//! KAT 轨迹九维过程评分 — 快手 KAT 融合（设计文档 §12.2）
//!
//! 对应架构层: **L7 Execution**（pvl-layer 子模块，ADR-049 决策 1 内嵌）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §12.2
//! 对应论文: 快手 KAT（轨迹式过程评分九维度）
//!
//! # 核心职责
//!
//! 从执行轨迹（算子序列 + 代码变更 + 验证步 + 错误报告）计算九维过程质量：
//! 探索 / 定位 / 忠实 / 最小 / 验证 / 诚实 / 效率 / 鲁棒 / 可读。
//! 全部为纯函数（铁律4），消费 L0 [`AtomicOperator`] 契约。
//!
//! # 落层偏差记录（D-2 命名协调）
//!
//! 规范 §12.2 字面名 `ProcessScore` 与既有 `process_score.rs` 的观测九维
//! （real_execution/coverage/…，polish-v2.7 P3-5，被 chimera-tui PvlScorePanel
//! 与 chimera-mas shadow 消费）冲突。本模块命名 [`TrajectoryProcessScore`]
//! 与之并存：
//! - **观测九维**（process_score.rs）：运行时观测指标，TUI 面板消费
//! - **轨迹九维**（本模块）：执行轨迹过程质量，经验回放/裁决消费
//!
//! # 性能说明
//!
//! `score_exploration` 用 4 位算子布尔计数替代 HashSet（算子枚举仅 4 变体，
//! 免堆分配，红线意识）；全程 f32 不隐式升 f64。

use nexus_contracts::experience_card::AtomicOperator;

/// 轨迹动作 — 算子执行记录
#[derive(Clone, Debug)]
pub struct TrajectoryAction {
    /// 执行的原子算子
    pub operator: AtomicOperator,
    /// 时间戳（毫秒，轨迹内单调）
    pub timestamp_ms: u64,
    /// 是否成功
    pub success: bool,
}

/// 代码变更 — 文件级增删行
#[derive(Clone, Debug)]
pub struct CodeChange {
    /// 变更文件路径
    pub file_path: String,
    /// 新增行数
    pub lines_added: i32,
    /// 删除行数
    pub lines_removed: i32,
}

/// 验证步 — 覆盖率验证记录
#[derive(Clone, Debug)]
pub struct VerificationStep {
    /// 验证类型（如 "cargo_test" / "clippy"）
    pub step_type: String,
    /// 是否通过
    pub passed: bool,
    /// 覆盖率百分比（0-100）
    pub coverage_percent: f32,
}

/// 执行轨迹 — 九维评分输入（规范 §12.2 ProcessTrajectory）
#[derive(Clone, Debug)]
pub struct ProcessTrajectory {
    /// 算子动作序列
    pub actions: Vec<TrajectoryAction>,
    /// 累计 Token 消耗
    pub total_tokens: u64,
    /// 最终评分
    pub final_score: f32,
    /// 目标评分（≤0 时忠实维度满分）
    pub target_score: f32,
    /// 代码变更列表
    pub code_changes: Vec<CodeChange>,
    /// 验证步列表
    pub verification_steps: Vec<VerificationStep>,
    /// 自报告错误集合
    pub reported_errors: Vec<String>,
    /// 实际错误集合
    pub actual_errors: Vec<String>,
}

/// KAT 轨迹九维过程评分（规范 §12.2，D-2 命名协调见模块文档）
#[derive(Clone, Debug, PartialEq)]
pub struct TrajectoryProcessScore {
    /// 探索 — 算子多样性（4 算子覆盖比例）
    pub exploration: f32,
    /// 定位 — Debug 动作成功率（无 Debug 时满分）
    pub localization: f32,
    /// 忠实 — 目标达成度（final/target）
    pub fidelity: f32,
    /// 最小 — 变更规模克制度
    pub minimality: f32,
    /// 验证 — 验证步覆盖率均值
    pub verification: f32,
    /// 诚实 — 错误报告 F1（虚报/漏报惩罚）
    pub honesty: f32,
    /// 效率 — Token 经济性
    pub efficiency: f32,
    /// 鲁棒 — 预留（规范占位 0.5）
    pub robustness: f32,
    /// 可读 — 预留（规范占位 0.5）
    pub readability: f32,
}

impl TrajectoryProcessScore {
    /// 从轨迹计算九维评分（纯函数，铁律4）
    pub fn from_trajectory(traj: &ProcessTrajectory) -> Self {
        Self {
            exploration: score_exploration(traj),
            localization: score_localization(traj),
            fidelity: score_fidelity(traj),
            minimality: score_minimality(traj),
            verification: score_verification(traj),
            honesty: score_honesty(traj),
            efficiency: score_efficiency(traj),
            robustness: 0.5,  // 规范占位（鲁棒维度待生产信号接入）
            readability: 0.5, // 规范占位（可读维度待 AST 分析接入）
        }
    }

    /// 加权总分 — 权重和 = 1.0（0.15/0.15/0.15/0.10/0.15/0.10/0.10/0.05/0.05）
    pub fn overall(&self) -> f32 {
        self.exploration * 0.15
            + self.localization * 0.15
            + self.fidelity * 0.15
            + self.minimality * 0.10
            + self.verification * 0.15
            + self.honesty * 0.10
            + self.efficiency * 0.10
            + self.robustness * 0.05
            + self.readability * 0.05
    }
}

/// 探索 — 算子多样性（4 位布尔计数免 HashSet 堆分配）
fn score_exploration(traj: &ProcessTrajectory) -> f32 {
    let mut seen = [false; 4];
    for action in &traj.actions {
        let idx = match action.operator {
            AtomicOperator::Draft => 0,
            AtomicOperator::Improve => 1,
            AtomicOperator::Debug => 2,
            AtomicOperator::Crossover => 3,
        };
        seen[idx] = true;
    }
    (seen.iter().filter(|&&b| b).count() as f32 / 4.0).min(1.0)
}

/// 定位 — Debug 动作成功率（无 Debug 动作时满分）
fn score_localization(traj: &ProcessTrajectory) -> f32 {
    let debug_actions: Vec<&TrajectoryAction> = traj
        .actions
        .iter()
        .filter(|a| matches!(a.operator, AtomicOperator::Debug))
        .collect();
    if debug_actions.is_empty() {
        return 1.0;
    }
    debug_actions.iter().filter(|a| a.success).count() as f32 / debug_actions.len() as f32
}

/// 忠实 — 目标达成度（target ≤ 0 时满分）
fn score_fidelity(traj: &ProcessTrajectory) -> f32 {
    if traj.target_score <= 0.0 {
        return 1.0;
    }
    (traj.final_score / traj.target_score).min(1.0)
}

/// 最小 — 变更规模克制度（baseline 50 行）
fn score_minimality(traj: &ProcessTrajectory) -> f32 {
    let total: i32 = traj
        .code_changes
        .iter()
        .map(|c| c.lines_added.abs() + c.lines_removed.abs())
        .sum();
    if total == 0 {
        return 1.0;
    }
    let baseline = 50.0f32;
    (baseline / (total as f32 + baseline)).min(1.0)
}

/// 验证 — 验证步覆盖率均值（无验证步时 0.5 中性）
fn score_verification(traj: &ProcessTrajectory) -> f32 {
    if traj.verification_steps.is_empty() {
        return 0.5;
    }
    traj.verification_steps
        .iter()
        .map(|v| v.coverage_percent)
        .sum::<f32>()
        / traj.verification_steps.len() as f32
        / 100.0
}

/// 诚实 — 错误报告 F1 + 虚报/漏报惩罚
fn score_honesty(traj: &ProcessTrajectory) -> f32 {
    use std::collections::HashSet;
    let reported: HashSet<&str> = traj.reported_errors.iter().map(String::as_str).collect();
    let actual: HashSet<&str> = traj.actual_errors.iter().map(String::as_str).collect();
    if actual.is_empty() {
        // 无实际错误: 如实报告空集满分，虚报扣半
        return if reported.is_empty() { 1.0 } else { 0.5 };
    }
    let correct = reported.intersection(&actual).count();
    let precision = if reported.is_empty() {
        1.0
    } else {
        correct as f32 / reported.len() as f32
    };
    let recall = correct as f32 / actual.len() as f32;
    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };
    // 虚报每个 -0.1，漏报每个 -0.2，惩罚上限 0.5
    let penalty =
        ((reported.len() - correct) as f32 * 0.1 + (actual.len() - correct) as f32 * 0.2).min(0.5);
    (f1 - penalty).max(0.0)
}

/// 效率 — Token 经济性（每分 Token 成本，baseline 10000）
fn score_efficiency(traj: &ProcessTrajectory) -> f32 {
    if traj.final_score <= 0.0 {
        return 0.0;
    }
    let tokens_per_point = traj.total_tokens as f32 / traj.final_score;
    let baseline = 10000.0f32;
    if tokens_per_point < baseline {
        1.0
    } else {
        (baseline / tokens_per_point).min(1.0)
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_trajectory() -> ProcessTrajectory {
        ProcessTrajectory {
            actions: Vec::new(),
            total_tokens: 0,
            final_score: 0.0,
            target_score: 0.0,
            code_changes: Vec::new(),
            verification_steps: Vec::new(),
            reported_errors: Vec::new(),
            actual_errors: Vec::new(),
        }
    }

    fn action(operator: AtomicOperator, success: bool) -> TrajectoryAction {
        TrajectoryAction {
            operator,
            timestamp_ms: 0,
            success,
        }
    }

    #[test]
    fn empty_trajectory_baseline() {
        let score = TrajectoryProcessScore::from_trajectory(&empty_trajectory());
        assert_eq!(score.exploration, 0.0, "无动作探索为 0");
        assert_eq!(score.localization, 1.0, "无 Debug 满分");
        assert_eq!(score.fidelity, 1.0, "target≤0 满分");
        assert_eq!(score.minimality, 1.0, "无变更满分");
        assert_eq!(score.verification, 0.5, "无验证步中性");
        assert_eq!(score.honesty, 1.0, "双空集诚实满分");
        assert_eq!(score.efficiency, 0.0, "final≤0 效率为 0");
    }

    #[test]
    fn exploration_covers_all_operators() {
        let mut traj = empty_trajectory();
        traj.actions = vec![
            action(AtomicOperator::Draft, true),
            action(AtomicOperator::Improve, true),
            action(AtomicOperator::Debug, true),
            action(AtomicOperator::Crossover, true),
        ];
        let score = TrajectoryProcessScore::from_trajectory(&traj);
        assert!((score.exploration - 1.0).abs() < 1e-6, "四算子全覆盖满分");
    }

    #[test]
    fn localization_debug_success_rate() {
        let mut traj = empty_trajectory();
        traj.actions = vec![
            action(AtomicOperator::Debug, true),
            action(AtomicOperator::Debug, false),
        ];
        let score = TrajectoryProcessScore::from_trajectory(&traj);
        assert!(
            (score.localization - 0.5).abs() < 1e-6,
            "2 Debug 1 成功 = 0.5"
        );
    }

    #[test]
    fn fidelity_target_achievement() {
        let mut traj = empty_trajectory();
        traj.final_score = 0.8;
        traj.target_score = 1.0;
        let score = TrajectoryProcessScore::from_trajectory(&traj);
        assert!((score.fidelity - 0.8).abs() < 1e-6);
    }

    #[test]
    fn minimality_change_size() {
        let mut traj = empty_trajectory();
        traj.code_changes = vec![CodeChange {
            file_path: "src/a.rs".into(),
            lines_added: 50,
            lines_removed: 0,
        }];
        let score = TrajectoryProcessScore::from_trajectory(&traj);
        // baseline 50 / (50 + 50) = 0.5
        assert!((score.minimality - 0.5).abs() < 1e-6);
    }

    #[test]
    fn honesty_f1_full_match() {
        let mut traj = empty_trajectory();
        traj.reported_errors = vec!["err-a".into()];
        traj.actual_errors = vec!["err-a".into()];
        let score = TrajectoryProcessScore::from_trajectory(&traj);
        assert!((score.honesty - 1.0).abs() < 1e-6, "完全匹配 F1=1 无惩罚");
    }

    #[test]
    fn honesty_false_report_penalty() {
        let mut traj = empty_trajectory();
        // 实际无错误但虚报 → 0.5
        traj.reported_errors = vec!["err-fake".into()];
        let score = TrajectoryProcessScore::from_trajectory(&traj);
        assert!((score.honesty - 0.5).abs() < 1e-6, "虚报扣半");
    }

    #[test]
    fn honesty_missed_error_penalty() {
        let mut traj = empty_trajectory();
        // 实际 1 错误漏报 → F1=0，penalty=0.2，max(0-0.2, 0)=0
        traj.actual_errors = vec!["err-missed".into()];
        let score = TrajectoryProcessScore::from_trajectory(&traj);
        assert_eq!(score.honesty, 0.0, "漏报无 F1 + 惩罚归零");
    }

    #[test]
    fn efficiency_token_economy() {
        let mut traj = empty_trajectory();
        traj.final_score = 1.0;
        traj.total_tokens = 20_000; // 2 倍 baseline
        let score = TrajectoryProcessScore::from_trajectory(&traj);
        assert!((score.efficiency - 0.5).abs() < 1e-6, "10000/20000 = 0.5");
    }

    #[test]
    fn overall_weights_sum_to_one() {
        // 全 1.0 维度 → overall = 权重和 = 1.0
        let full = TrajectoryProcessScore {
            exploration: 1.0,
            localization: 1.0,
            fidelity: 1.0,
            minimality: 1.0,
            verification: 1.0,
            honesty: 1.0,
            efficiency: 1.0,
            robustness: 1.0,
            readability: 1.0,
        };
        assert!((full.overall() - 1.0).abs() < 1e-6, "权重和应为 1.0");
    }

    #[test]
    fn overall_pure_function() {
        // 铁律4: 同输入同输出
        let traj = empty_trajectory();
        let s1 = TrajectoryProcessScore::from_trajectory(&traj);
        let s2 = TrajectoryProcessScore::from_trajectory(&traj);
        assert_eq!(s1, s2);
        assert_eq!(s1.overall(), s2.overall());
    }
}
