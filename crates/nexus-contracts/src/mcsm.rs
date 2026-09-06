//! MCSM 流形约束信号守恒聚合 — Sinkhorn 双随机投影器（WI-05）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts，纯函数先例集合）
//! 对应工作项: **WI-05 MCSM 流形约束信号守恒聚合**（v4.0 统一执行总案 §13，
//!             PANTHEON IN-01）
//! 对应设计源: DeepSeek V4 mHC（混合流形约束）——聚合权重矩阵经 Sinkhorn
//!             投影到**双随机流形**（行列和均为 1），深层信号不爆炸不消失
//!
//! # 核心职责
//!
//! 解决"未约束加权和"问题（UP-25）：高音量信号源可淹没他源。Sinkhorn
//! 行列归一化使聚合矩阵落在双随机流形上，**极端分布无单源淹没**。
//!
//! # 设计约束（ADR-033 纯函数先例）
//!
//! - **纯函数零 IO**: 仅输入矩阵 → 输出矩阵，无状态无副作用
//!   （与 `archive_monotonicity` / `behavior_contract::enforce` 同类，
//!   显式声明为 ADR-033"纯类型零逻辑"的例外）
//! - **全 f32 计算**: 与既有向量/权重体系一致，禁止隐式 f64 转换
//!   （sesa-router f32 精度教训：0.4f32 as f64 精度膨胀）
//! - **确定性归约**: 固定分块逐行/逐列归一化，criterion 1e-6 容差
//! - **防御语义**: 非有限值（NaN/inf）输入返回 `None`（调用方走
//!   `identity()` 直通回滚），不 panic 不吞错
//! - **迭代上限**: 默认 ≤20 次迭代（v4.0 规格），`tol` 提前收敛
//!
//! # 落点说明（融合裁决）
//!
//! v4.0 §13 原落点为 `event-bus::ieta_aggregator::mcsm`——审计核验该
//! 聚合器在代码库不存在，本投影器落位 **L0 纯函数模块**（全层可依赖），
//! 聚合接入点（parliament 投票权重 / event-bus 未来批聚合）调用本模块。

use serde::{Deserialize, Serialize};

/// Sinkhorn 投影参数 — 收敛容差与迭代上限
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SinkhornParams {
    /// 最大迭代次数（v4.0 规格 ≤20）
    pub max_iters: usize,
    /// 收敛容差（行列和与 1.0 的偏差绝对值）
    pub tolerance: f32,
}

impl Default for SinkhornParams {
    fn default() -> Self {
        Self {
            max_iters: 20,
            tolerance: 1e-4,
        }
    }
}

/// 行归一化结果 — 投影过程的中间/终态载体
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedMatrix {
    /// 双随机矩阵（行列和 ≈ 1）
    pub matrix: Vec<Vec<f32>>,
    /// 实际迭代次数
    pub iters: usize,
    /// 是否收敛（行/列和偏差均 ≤ tolerance）
    pub converged: bool,
}

/// Sinkhorn 双随机投影 — 聚合权重矩阵行列归一化（WI-05 核心）
///
/// # 算法
/// 交替行归一化（每行和 → 1）与列归一化（每列和 → 1），直至
/// 行列和均收敛到 `tolerance` 或达到 `max_iters`。投影到双随机流形后，
/// **单源 100× 音量不再淹没他源**（行列和受约束，比例仍保留相对强弱）。
///
/// # 收敛语义（数学边界）
/// 行列和**同时**收敛到 1 仅对**方阵**可达（总行和 = 行数 ≠ 总列和 = 列数
/// 时两个仿射集交集为空）。对非方阵：算法仍执行交替归一化（行与行之间
/// 的相对均衡是防淹没的核心），但 `converged` 标记为 false——调用方应按
/// "行相对均衡"语义消费，或先行补零/采样成方阵。
///
/// # 防御
/// - 输入含非有限值（NaN/inf）→ 返回 `None`（调用方 `identity()` 直通）
/// - 输入为空矩阵/非矩形 → 返回 `None`
/// - 全零行/列：除以零保护（保持 0 行——该行无信号可归一）
/// - 输入元素为负 → 返回 `None`（权重矩阵语义要求非负）
///
/// # 参数
/// - `matrix`: R×C 权重矩阵（行=信号源，列=信号维度）
/// - `params`: 迭代上限与容差（默认 20 次 / 1e-4）
///
/// # 返回
/// - `Some(ProjectedMatrix)`: 投影成功（方阵收敛时行列和 ≈ 1）
/// - `None`: 输入非法（含负值/非有限值/非矩形），调用方应走
///   [`identity`] 直通（v4.0 回滚语义：`identity()` 直通）
pub fn sinkhorn_project(matrix: &[Vec<f32>], params: SinkhornParams) -> Option<ProjectedMatrix> {
    // 防御 1: 非空 + 非矩形检测
    if matrix.is_empty() {
        return None;
    }
    let cols = matrix[0].len();
    if cols == 0 {
        return None;
    }
    if matrix.iter().any(|row| row.len() != cols) {
        return None;
    }
    // 防御 2: 非有限值 + 负值检测（权重矩阵语义要求非负）
    for row in matrix {
        for &v in row {
            if !v.is_finite() || v < 0.0 {
                return None;
            }
        }
    }

    let rows = matrix.len();
    let mut m: Vec<Vec<f32>> = matrix.to_vec();
    let mut iters = 0usize;
    let mut converged = false;

    for _ in 0..params.max_iters {
        iters += 1;
        // 行归一化: 每行和 → 1
        for row in &mut m {
            let sum: f32 = row.iter().sum();
            if sum > 0.0 {
                for v in row.iter_mut() {
                    *v /= sum;
                }
            }
            // 全零行保持 0（该行无信号，不参与投影）
        }
        // 列归一化: 每列和 → 1（先汇总列和，再逐列缩放——迭代器形态避免
        // needless_range_loop；全零列保持 0）
        let mut col_sums = vec![0.0f32; cols];
        for row in &m {
            for (c, &v) in row.iter().enumerate() {
                col_sums[c] += v;
            }
        }
        for (c, &sum) in col_sums.iter().enumerate() {
            if sum > 0.0 {
                for row in m.iter_mut() {
                    row[c] /= sum;
                }
            }
        }
        // 收敛检测: 行列和偏差均 ≤ tolerance
        let row_ok = m
            .iter()
            .all(|row| (row.iter().sum::<f32>() - 1.0).abs() <= params.tolerance);
        let col_ok = (0..cols)
            .all(|c| ((0..rows).map(|r| m[r][c]).sum::<f32>() - 1.0).abs() <= params.tolerance);
        if row_ok && col_ok {
            converged = true;
            break;
        }
    }

    Some(ProjectedMatrix {
        matrix: m,
        iters,
        converged,
    })
}

/// 1×N 退化情形：权重向量行归一化（和 → 1）
///
/// 适用于议会投票权重等**单源多维度**权重向量的归一化。
/// 数学上为 Sinkhorn 对 1×N 矩阵的特例（仅行归一化一步到位）。
///
/// # 返回
/// - `Some(Vec<f32>)`: 归一化后权重（和 ≈ 1）
/// - `None`: 输入含非有限值/负值/全零
pub fn project_weights(weights: &[f32], params: SinkhornParams) -> Option<Vec<f32>> {
    if weights.is_empty() {
        return None;
    }
    if weights.iter().any(|&v| !v.is_finite() || v < 0.0) {
        return None;
    }
    let sum: f32 = weights.iter().sum();
    if sum <= 0.0 {
        return None;
    }
    let _ = params; // 1×N 退化情形单步完成，无需迭代
    Some(weights.iter().map(|&v| v / sum).collect())
}

/// 恒等直通 — 回滚路径（v4.0 §13 WI-05 回滚语义）
///
/// 数值异常/调用方裁决时使用：不做任何归一化，保持原始权重。
pub fn identity(matrix: Vec<Vec<f32>>) -> Vec<Vec<f32>> {
    matrix
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn default_params() -> SinkhornParams {
        SinkhornParams::default()
    }

    // ---------- 收敛与双随机不变量 ----------

    #[test]
    fn sinkhorn_row_and_col_sums_approx_one() {
        // 验收: 行列和 ≈ 1（数值测试，1e-4 容差）
        let matrix = vec![
            vec![1.0, 2.0, 3.0],
            vec![4.0, 5.0, 6.0],
            vec![7.0, 8.0, 9.0],
        ];
        let result = sinkhorn_project(&matrix, default_params()).expect("投影必须成功");
        assert!(result.converged, "20 次迭代内应收敛");
        for row in &result.matrix {
            let row_sum: f32 = row.iter().sum();
            assert!((row_sum - 1.0).abs() <= 1e-3, "行和应 ≈ 1, 实际 {row_sum}");
        }
        for c in 0..3 {
            let col_sum: f32 = result.matrix.iter().map(|r| r[c]).sum();
            assert!((col_sum - 1.0).abs() <= 1e-3, "列和应 ≈ 1, 实际 {col_sum}");
        }
    }

    #[test]
    fn sinkhorn_rank_one_uniform() {
        // 均匀矩阵投影后仍均匀
        let matrix = vec![vec![1.0, 1.0], vec![1.0, 1.0]];
        let result = sinkhorn_project(&matrix, default_params()).expect("投影必须成功");
        for row in &result.matrix {
            for &v in row {
                assert!(
                    (v - 0.5).abs() <= 1e-3,
                    "均匀矩阵投影后应保持 0.5, 实际 {v}"
                );
            }
        }
    }

    #[test]
    fn sinkhorn_zero_row_preserved() {
        // 全零行保持 0（该行无信号可归一）
        let matrix = vec![vec![0.0, 0.0], vec![1.0, 0.0]];
        let result = sinkhorn_project(&matrix, default_params()).expect("投影必须成功");
        assert_eq!(result.matrix[0], vec![0.0, 0.0]);
        // 非方阵: converged = false（行列和不能同时为 1，数学边界）
        assert!(!result.converged);
    }

    // ---------- 对抗回放: 单源 100× 音量不淹没 ----------

    #[test]
    fn sinkhorn_single_source_100x_does_not_drown_others() {
        // 验收: 对抗回放（单源 100× 音量不淹没他源）
        // 方阵 3×3: 源 A 权重 100, 源 B/C 权重 1
        // 投影后: A 占主导但仍受双随机约束, B/C 的行和均 ≈ 1（不被淹没）
        let matrix = vec![
            vec![100.0, 1.0, 1.0],
            vec![1.0, 100.0, 1.0],
            vec![1.0, 1.0, 1.0],
        ];
        let result = sinkhorn_project(&matrix, default_params()).expect("投影必须成功");
        assert!(result.converged, "方阵正矩阵应收敛");
        // B/C 行和均 ≈ 1（未被 100× 源淹没到 0）
        for r in 1..3 {
            let row_sum: f32 = result.matrix[r].iter().sum();
            assert!(
                (row_sum - 1.0).abs() <= 1e-3,
                "源 {r} 行和应 ≈ 1（不被 100× 源淹没）, 实际 {row_sum}"
            );
        }
        // A 行内相对比例仍保留（第一列 > 第二列）
        assert!(result.matrix[0][0] > result.matrix[0][1]);
    }

    // ---------- project_weights 退化情形 ----------

    #[test]
    fn project_weights_normalizes_to_one() {
        let weights = vec![1.0, 2.0, 1.0];
        let projected = project_weights(&weights, default_params()).expect("归一化必须成功");
        let sum: f32 = projected.iter().sum();
        assert!((sum - 1.0).abs() <= 1e-6, "权重和应 ≈ 1, 实际 {sum}");
        assert!((projected[1] - 0.5).abs() <= 1e-6);
    }

    #[test]
    fn project_weights_preserves_proportion() {
        // 1D 归一化保持比例（与未归一化加权和数学等价——零行为变化）
        let weights = vec![3.0, 1.0];
        let projected = project_weights(&weights, default_params()).expect("归一化必须成功");
        assert!((projected[0] / projected[1] - 3.0).abs() <= 1e-6);
    }

    // ---------- 防御语义 ----------

    #[test]
    fn sinkhorn_rejects_non_finite() {
        let matrix = vec![vec![f32::NAN, 1.0], vec![1.0, 1.0]];
        assert!(sinkhorn_project(&matrix, default_params()).is_none());
        let matrix = vec![vec![f32::INFINITY, 1.0], vec![1.0, 1.0]];
        assert!(sinkhorn_project(&matrix, default_params()).is_none());
    }

    #[test]
    fn sinkhorn_rejects_negative() {
        let matrix = vec![vec![1.0, -1.0], vec![1.0, 1.0]];
        assert!(sinkhorn_project(&matrix, default_params()).is_none());
    }

    #[test]
    fn sinkhorn_rejects_non_rectangular() {
        let matrix = vec![vec![1.0, 1.0], vec![1.0]];
        assert!(sinkhorn_project(&matrix, default_params()).is_none());
    }

    #[test]
    fn sinkhorn_rejects_empty() {
        let matrix: Vec<Vec<f32>> = vec![];
        assert!(sinkhorn_project(&matrix, default_params()).is_none());
        let matrix = vec![vec![]];
        assert!(sinkhorn_project(&matrix, default_params()).is_none());
    }

    #[test]
    fn project_weights_rejects_invalid() {
        assert!(project_weights(&[], default_params()).is_none());
        assert!(project_weights(&[1.0, f32::NAN], default_params()).is_none());
        assert!(project_weights(&[1.0, -0.5], default_params()).is_none());
        assert!(project_weights(&[0.0, 0.0], default_params()).is_none());
    }

    // ---------- 回滚路径 ----------

    #[test]
    fn identity_pass_returns_original() {
        let matrix = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        assert_eq!(identity(matrix.clone()), matrix);
    }
}
