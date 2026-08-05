//! 召回报告 — recall@tier / needle_recall@8 / position_bias / chain_success_rate 汇总
//!
//! 对应 PROBE P0.2：P0.5 双基线对照表的输出载体
//!
//! # 设计
//! - 单次评测输出 `RecallReport`（窗口档 + 四指标 + 延迟/吞吐预留位）
//! - `Display` 实现输出可解析对照表行（TUI OsaSparse 面板 / 文档同步消费）
//! - 全部 f32 指标（f32 红线），格式化时用 f32 直接格式化（禁止 as f64）

use std::fmt;

/// 单次召回评测报告 — 一个窗口档 × 一条选择路径的完整指标快照
#[derive(Debug, Clone, PartialEq)]
pub struct RecallReport {
    /// 窗口档标识（L0/L1/L2/L3 或 "static"/"recall-pipeline" 路径对照）
    pub tier: String,
    /// 单针召回（0/1，recall@tier 口径）
    pub recall_at_tier: f32,
    /// 多针召回率（needle_recall@8 口径，目标 ≥ 0.90）
    pub needle_recall_at_8: f32,
    /// 位置偏置比（中段 ÷ 头尾，目标 ≥ 0.85）
    pub position_bias: f32,
    /// 链路成功率（多跳，目标 ≥ 0.80）
    pub chain_success_rate: f32,
    /// 选中块数（诊断：是否贴近窗口容量）
    pub selected_count: usize,
}

impl RecallReport {
    /// 创建召回报告
    ///
    /// # 参数
    /// - `tier`: 窗口档/路径标识
    /// - `recall_at_tier`: 单针召回 ∈ [0,1]
    /// - `needle_recall_at_8`: 多针召回 ∈ [0,1]
    /// - `position_bias`: 位置偏置比 ∈ [0,1]
    /// - `chain_success_rate`: 链路成功率 ∈ [0,1]
    /// - `selected_count`: 选中块数
    pub fn new(
        tier: impl Into<String>,
        recall_at_tier: f32,
        needle_recall_at_8: f32,
        position_bias: f32,
        chain_success_rate: f32,
        selected_count: usize,
    ) -> Self {
        Self {
            tier: tier.into(),
            recall_at_tier,
            needle_recall_at_8,
            position_bias,
            chain_success_rate,
            selected_count,
        }
    }

    /// 是否满足 PROBE 验收指标（§6.1 三召回目标）
    ///
    /// # 返回值
    /// `true` 当且仅当 needle_recall@8 ≥ 0.90 且 position_bias ≥ 0.85
    /// 且 chain_success_rate ≥ 0.80（多针/位置/多跳三目标齐备）
    pub fn meets_acceptance(&self) -> bool {
        self.needle_recall_at_8 >= 0.90
            && self.position_bias >= 0.85
            && self.chain_success_rate >= 0.80
    }
}

impl fmt::Display for RecallReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 可解析对照表行：tier,recall@tier,needle@8,bias,chain,selected
        write!(
            f,
            "{} | recall@tier={:.3} needle@8={:.3} bias={:.3} chain={:.3} selected={}",
            self.tier,
            self.recall_at_tier,
            self.needle_recall_at_8,
            self.position_bias,
            self.chain_success_rate,
            self.selected_count
        )
    }
}

/// 双路径对照报告 — Static 老路径 vs recall/ 管线（P0.5 双基线对照表）
#[derive(Debug, Clone, PartialEq)]
pub struct BaselineComparison {
    /// Static 路径报告（对照组）
    pub static_report: RecallReport,
    /// recall/ 管线报告（实验组）
    pub pipeline_report: RecallReport,
}

impl BaselineComparison {
    /// 创建双路径对照
    ///
    /// # 参数
    /// - `static_report`: Static compressor 路径报告
    /// - `pipeline_report`: recall/ 管线报告
    pub fn new(static_report: RecallReport, pipeline_report: RecallReport) -> Self {
        Self {
            static_report,
            pipeline_report,
        }
    }

    /// 逐项差异（pipeline − static，正值为管线更优）
    ///
    /// # 返回值
    /// `(Δrecall@tier, Δneedle@8, Δbias, Δchain)`
    pub fn deltas(&self) -> (f32, f32, f32, f32) {
        (
            self.pipeline_report.recall_at_tier - self.static_report.recall_at_tier,
            self.pipeline_report.needle_recall_at_8 - self.static_report.needle_recall_at_8,
            self.pipeline_report.position_bias - self.static_report.position_bias,
            self.pipeline_report.chain_success_rate - self.static_report.chain_success_rate,
        )
    }

    /// 是否有任何召回项下降（A/B 回归闸判定）
    ///
    /// # 返回值
    /// `true` 当且仅当任一召回指标（needle@8 / bias / chain）pipeline < static
    /// —— 触发"任一项下降不合并"回归闸（计划 §4.2 回归闸 3）
    pub fn any_recall_regression(&self) -> bool {
        self.pipeline_report.needle_recall_at_8 < self.static_report.needle_recall_at_8
            || self.pipeline_report.position_bias < self.static_report.position_bias
            || self.pipeline_report.chain_success_rate < self.static_report.chain_success_rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(tier: &str, needle: f32, bias: f32, chain: f32) -> RecallReport {
        RecallReport::new(tier, 1.0, needle, bias, chain, 100)
    }

    #[test]
    fn test_meets_acceptance() {
        assert!(report("L2", 0.95, 0.90, 0.85).meets_acceptance());
        assert!(!report("L2", 0.89, 0.90, 0.85).meets_acceptance());
        assert!(!report("L2", 0.95, 0.84, 0.85).meets_acceptance());
        assert!(!report("L2", 0.95, 0.90, 0.79).meets_acceptance());
    }

    #[test]
    fn test_baseline_comparison() {
        let static_r = report("static", 0.60, 0.55, 0.50);
        let pipeline_r = report("pipeline", 0.90, 0.88, 0.82);
        let cmp = BaselineComparison::new(static_r, pipeline_r);
        let (d1, d2, d3, d4) = cmp.deltas();
        assert!((d1 - 0.0).abs() < 1e-6); // recall@tier 两者都 1.0
        assert!((d2 - 0.30).abs() < 1e-6);
        assert!((d3 - 0.33).abs() < 1e-6);
        assert!((d4 - 0.32).abs() < 1e-6);
        assert!(!cmp.any_recall_regression());
    }

    #[test]
    fn test_regression_detection() {
        // 管线 needle 下降 → 触发回归闸
        let static_r = report("static", 0.90, 0.88, 0.82);
        let pipeline_r = report("pipeline", 0.80, 0.90, 0.85);
        let cmp = BaselineComparison::new(static_r, pipeline_r);
        assert!(cmp.any_recall_regression());
        // 全部持平 → 不触发
        let same = BaselineComparison::new(
            report("static", 0.9, 0.9, 0.9),
            report("pipeline", 0.9, 0.9, 0.9),
        );
        assert!(!same.any_recall_regression());
    }

    #[test]
    fn test_display() {
        let r = report("L2", 0.90, 0.85, 0.80);
        let s = r.to_string();
        assert!(s.contains("L2") && s.contains("needle@8=0.900"));
    }
}
