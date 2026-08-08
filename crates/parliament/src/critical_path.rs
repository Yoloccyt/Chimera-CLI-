//! 关键路径动态识别 — 规则驱动风险综合（Milestone B-6）
//!
//! 对应方案: `CHIMERA_V3_专项优化方案_v2.21基线.md` §5.1 P3 / §6 B-6
//! 对应设计: 根目录设计文档 §12.2（关键路径动态识别）
//!
//! # 职责
//!
//! 六风险因子规则综合，判定任务链是否为关键路径（高风险链应优先治理），
//! 与 `quest-engine::coordination_metrics`（ADR-063 推理悖论红线度量）接线互补：
//! 协调成本/推理增益比值作为因子 3 输入。
//!
//! # 规则（纯函数，无学习——R2 冻结面外）
//!
//! | 因子 | 阈值 | 归一化 |
//! |---|---|---|
//! | 任务规模 task_count | > 32 | min(count/64, 1) |
//! | 依赖深度 max_dependency_depth | > 8 | min(depth/16, 1) |
//! | 协调成本比 coordination_to_gain | > 1.0 | min(ratio/3, 1) |
//! | 否决率 veto_rate | > 0.3 | 原值 [0,1] |
//! | 超时率 timeout_rate | > 0.2 | 原值 [0,1] |
//! | 资源水位 budget_watermark | > 0.85 | 原值 [0,1] |
//!
//! 加权和：规模 0.2 / 深度 0.2 / 协调比 0.25 / 否决 0.15 / 超时 0.1 / 水位 0.1。
//! `is_critical` = 任一因子超标（单因子否决语义）或加权分 > 0.6。
//!
//! # 依赖铁律（§5.3）
//!
//! L8 parliament 纯输入结构 + 纯函数，不依赖 L4（无 seccore 引用）；
//! 输入由调用方（编排器）从各层度量聚合注入。

/// 六风险因子输入
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RiskFactorInput {
    /// 因子 1:任务规模（DAG 节点数）
    pub task_count: u32,
    /// 因子 2:依赖深度（最长链长度）
    pub max_dependency_depth: u32,
    /// 因子 3:协调成本/推理增益比值（quest-engine CoordinationToGainRatio）
    pub coordination_to_gain: f64,
    /// 因子 4:审议否决率 [0,1]
    pub veto_rate: f64,
    /// 因子 5:超时率 [0,1]
    pub timeout_rate: f64,
    /// 因子 6:资源水位 [0,1]
    pub budget_watermark: f64,
}

impl Default for RiskFactorInput {
    fn default() -> Self {
        Self {
            task_count: 1,
            max_dependency_depth: 1,
            coordination_to_gain: 0.0,
            veto_rate: 0.0,
            timeout_rate: 0.0,
            budget_watermark: 0.0,
        }
    }
}

/// 超标因子（诊断输出）
#[derive(Debug, Clone, PartialEq)]
pub struct RiskFactor {
    /// 因子名（中文，供报告/仪表盘直接展示）
    pub name: &'static str,
    /// 实测值
    pub value: f64,
    /// 阈值
    pub threshold: f64,
}

/// 关键路径评估报告
#[derive(Debug, Clone, PartialEq)]
pub struct CriticalPathReport {
    /// 综合风险分数 [0,1]
    pub risk_score: f64,
    /// 超标因子列表（空 = 无超标）
    pub contributing_factors: Vec<RiskFactor>,
    /// 是否关键路径（任一因子超标或加权分 > 0.6）
    pub is_critical: bool,
}

/// 因子阈值表（规则常量，公开供文档/测试对齐）
pub const FACTOR_THRESHOLDS: [(f64, f64); 6] = [
    // (阈值, 归一化分母)
    (32.0, 64.0), // 任务规模
    (8.0, 16.0),  // 依赖深度
    (1.0, 3.0),   // 协调成本比
    (0.3, 1.0),   // 否决率
    (0.2, 1.0),   // 超时率
    (0.85, 1.0),  // 资源水位
];

/// 加权分关键阈值
const CRITICAL_WEIGHTED_SCORE: f64 = 0.6;

/// 评估关键路径 — 六因子规则综合
///
/// # 返回
/// `CriticalPathReport`：风险分数 + 超标因子 + 关键判定。
/// 纯函数（&self 无状态），调用方可任意频次评估。
pub fn assess_critical_path(input: &RiskFactorInput) -> CriticalPathReport {
    let mut factors: Vec<RiskFactor> = Vec::new();
    let mut normalized: Vec<f64> = Vec::with_capacity(6);

    // 各因子归一化 + 超标检测（阈值表与输入字段一一对应）
    let raw = [
        input.task_count as f64,
        input.max_dependency_depth as f64,
        input.coordination_to_gain,
        input.veto_rate,
        input.timeout_rate,
        input.budget_watermark,
    ];
    let names = [
        "任务规模",
        "依赖深度",
        "协调成本比",
        "否决率",
        "超时率",
        "资源水位",
    ];
    for (i, (threshold, denom)) in FACTOR_THRESHOLDS.iter().enumerate() {
        let value = raw[i];
        let norm = (value / denom).clamp(0.0, 1.0);
        normalized.push(norm);
        if value > *threshold {
            factors.push(RiskFactor {
                name: names[i],
                value,
                threshold: *threshold,
            });
        }
    }

    // 加权和（规模/深度 0.2，协调比 0.25，否决 0.15，超时 0.1，水位 0.1）
    let weights = [0.2, 0.2, 0.25, 0.15, 0.1, 0.1];
    let risk_score: f64 = normalized
        .iter()
        .zip(weights.iter())
        .map(|(n, w)| n * w)
        .sum::<f64>()
        .clamp(0.0, 1.0);

    let is_critical = !factors.is_empty() || risk_score > CRITICAL_WEIGHTED_SCORE;

    CriticalPathReport {
        risk_score,
        contributing_factors: factors,
        is_critical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_input_is_safe() {
        let report = assess_critical_path(&RiskFactorInput::default());
        assert!(!report.is_critical);
        // 默认输入含基线噪声（task_count=1/depth=1 的归一化贡献），
        // 断言低风险而非精确值（避免脆性绑定归一化公式）。
        assert!(
            report.risk_score < 0.1,
            "默认输入风险应极低: {}",
            report.risk_score
        );
    }

    #[test]
    fn threshold_table_has_six_factors() {
        assert_eq!(FACTOR_THRESHOLDS.len(), 6);
    }
}
