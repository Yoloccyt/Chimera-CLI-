//! 质量趋势分析器 — 滑动窗口跟踪共识质量趋势
//!
//! 维护最近 N 次审议的 `ConsensusQualityMetrics` 滑动窗口，
//! 检测分歧度异常、弃权趋势，计算综合健康评分（0-100）。
//!
//! # 设计决策 (WHY)
//! - 使用 `VecDeque` 实现滑动窗口，无需外部依赖：
//!   `VecDeque` 的 `push_back`/`pop_front` 均为 O(1)，适合固定大小窗口。
//! - 窗口大小默认为 20（≈ 一次 Quest 的典型审议次数）：
//!   20 次审议在 L8 正常负载下约覆盖 2-3 个 Quest 的完整生命周期。
//! - 异常检测基于连续计数而非简单阈值，避免瞬态波动误报：
//!   单次分歧度 > 0.7 可能只是某次争议性提案的正常现象，
//!   连续 5 次才是系统性退化信号。
//! - 健康评分缓存更新机制：每次 `push` 时重新计算，`consensus_health_score`
//!   为 O(1) 读取，避免遍历窗口的 O(n) 开销。

use std::collections::VecDeque;

use crate::voting::ConsensusQualityMetrics;

/// 质量趋势分析器 — 滑动窗口跟踪共识质量趋势
///
/// 维护最近 N 次审议的 ConsensusQualityMetrics 滑动窗口，
/// 检测分歧度异常、弃权趋势，计算综合健康评分。
///
/// # 设计决策
/// - 使用 VecDeque 实现滑动窗口，无需外部依赖
/// - 窗口大小固定为 20（≈ 一次 Quest 的典型审议次数）
/// - 异常检测基于连续计数而非简单阈值，避免瞬态波动误报
pub struct QualityTrendAnalyzer {
    /// 滑动窗口中的质量指标
    window: VecDeque<ConsensusQualityMetrics>,
    /// 最大窗口大小
    max_size: usize,
    /// 连续分歧异常计数（divergence > 0.7 的连续次数）
    consecutive_divergence_anomalies: usize,
    /// 连续弃权趋势计数（abstention_rate > 0.4 的连续次数）
    consecutive_abstention_anomalies: usize,
    /// 当前健康评分 (0-100)，每次 push 时更新
    health_score: u8,
}

impl QualityTrendAnalyzer {
    /// 创建新的质量趋势分析器
    ///
    /// # 参数
    /// - `max_size`: 可选窗口大小，默认 20
    ///
    /// # 示例
    /// ```
    /// use parliament::quality_trend::QualityTrendAnalyzer;
    /// let analyzer = QualityTrendAnalyzer::new(None);
    /// assert_eq!(analyzer.consensus_health_score(), 100);
    /// ```
    pub fn new(max_size: Option<usize>) -> Self {
        Self {
            window: VecDeque::new(),
            max_size: max_size.unwrap_or(20),
            consecutive_divergence_anomalies: 0,
            consecutive_abstention_anomalies: 0,
            health_score: 100,
        }
    }

    /// 推入新指标，更新窗口和异常检测
    ///
    /// # 流程
    /// 1. 若窗口已满，弹出最早条目（O(1)）
    /// 2. 推入新指标到窗口尾部（O(1)）
    /// 3. 更新连续异常计数（基于最新条目）
    /// 4. 更新综合健康评分
    pub fn push(&mut self, metrics: ConsensusQualityMetrics) {
        // 1. 窗口已满时弹出最早条目
        if self.window.len() >= self.max_size {
            self.window.pop_front();
        }

        // 2. 推入新指标
        self.window.push_back(metrics);

        // 3. 更新连续异常计数
        // WHY 连续计数而非窗口扫描：连续计数在 O(1) 内完成。
        // 若仅扫描窗口，每次 push 需遍历所有窗口元素检查 divergence 阈值，
        // 而连续计数器仅跟踪最新趋势，足够反映"最近 N 次是否持续异常"。
        if metrics.divergence > 0.7 {
            self.consecutive_divergence_anomalies =
                self.consecutive_divergence_anomalies.saturating_add(1);
        } else {
            self.consecutive_divergence_anomalies = 0;
        }

        if metrics.abstention_rate > 0.4 {
            self.consecutive_abstention_anomalies =
                self.consecutive_abstention_anomalies.saturating_add(1);
        } else {
            self.consecutive_abstention_anomalies = 0;
        }

        // 4. 更新健康评分
        self.update_health_score();
    }

    /// 更新综合健康评分（内部方法，每次 push 时调用）
    ///
    /// 评分规则（从 100 向下扣减，钳制到 [0, 100]）：
    /// - 默认 100 分
    /// - 连续 5 次分歧度 > 0.7（`divergence_anomaly` 为 true）：扣 20 分
    /// - 连续 10 次弃权率 > 0.4（`abstention_trend` 为 true）：扣 15 分
    /// - 窗口内每条 `approval_rate < 0.5` 的条目：扣 10 分
    ///
    /// WHY 扣分项设计：分歧异常和弃权趋势是系统性退化信号，
    /// 单次扣除较高（20/15）；低赞成率是逐条累积的细粒度信号，
    /// 每条扣 10 分反映"低共识质量在窗口中的占比"。
    fn update_health_score(&mut self) {
        let mut score: i32 = 100;

        // 分歧异常：连续 5 次分歧度 > 0.7
        if self.consecutive_divergence_anomalies >= 5 {
            score -= 20;
        }

        // 弃权趋势：连续 10 次弃权率 > 0.4
        if self.consecutive_abstention_anomalies >= 10 {
            score -= 15;
        }

        // 低赞成率：窗口内每条 approval_rate < 0.5 的条目扣 10 分
        for m in &self.window {
            if m.approval_rate < 0.5 {
                score -= 10;
            }
        }

        self.health_score = score.clamp(0, 100) as u8;
    }

    /// 检测分歧度异常
    ///
    /// 连续 5 次分歧度 > 0.7 视为异常。
    /// 5 次阈值 = 约 1/4 窗口大小，代表系统性分歧而非单次争议。
    pub fn divergence_anomaly(&self) -> bool {
        self.consecutive_divergence_anomalies >= 5
    }

    /// 检测弃权趋势
    ///
    /// 连续 10 次弃权率 > 0.4 视为趋势异常。
    /// 10 次阈值 = 1/2 窗口大小，代表半数以上审议出现高弃权，
    /// 暗示角色参与度或议题相关性系统性下降。
    pub fn abstention_trend(&self) -> bool {
        self.consecutive_abstention_anomalies >= 10
    }

    /// 获取当前健康评分 (0-100)
    ///
    /// 100 = 完全健康，0 = 严重退化。
    /// 供上层（自适应策略选择器）在策略决策时作为反馈信号。
    pub fn consensus_health_score(&self) -> u8 {
        self.health_score
    }

    /// 生成汇总报告
    ///
    /// 包含窗口内所有指标的均值、标准差、异常标志和健康评分。
    /// 标准差使用总体标准差公式（除 n 而非 n-1），
    /// 因为窗口是全体样本而非抽样。
    ///
    /// # 返回
    /// `QualityReport` — 汇总统计摘要
    pub fn generate_report(&self) -> QualityReport {
        let n = self.window.len();
        if n == 0 {
            return QualityReport {
                avg_approval_rate: 0.0,
                avg_abstention_rate: 0.0,
                avg_divergence: 0.0,
                std_approval_rate: 0.0,
                std_divergence: 0.0,
                health_score: self.health_score,
                has_divergence_anomaly: self.divergence_anomaly(),
                has_abstention_anomaly: self.abstention_trend(),
                sample_count: 0,
            };
        }

        let n_f32 = n as f32;

        // 计算均值：单次遍历累加
        let (sum_approval, sum_abstention, sum_divergence) =
            self.window
                .iter()
                .fold((0.0f32, 0.0f32, 0.0f32), |(sa, sb, sd), m| {
                    (
                        sa + m.approval_rate,
                        sb + m.abstention_rate,
                        sd + m.divergence,
                    )
                });

        let avg_approval_rate = sum_approval / n_f32;
        let avg_abstention_rate = sum_abstention / n_f32;
        let avg_divergence = sum_divergence / n_f32;

        // 计算标准差（总体标准差，除 n）
        // 用 E[X²] - E[X]² 单趟等效，但直接计算偏差平方和更数值稳定
        let (sum_sq_approval, sum_sq_divergence) =
            self.window.iter().fold((0.0f32, 0.0f32), |(sa, sd), m| {
                let da = m.approval_rate - avg_approval_rate;
                let dd = m.divergence - avg_divergence;
                (sa + da * da, sd + dd * dd)
            });

        // max(0.0) 兜底浮点误差导致的负方差
        let std_approval_rate = (sum_sq_approval / n_f32).max(0.0).sqrt();
        let std_divergence = (sum_sq_divergence / n_f32).max(0.0).sqrt();

        QualityReport {
            avg_approval_rate,
            avg_abstention_rate,
            avg_divergence,
            std_approval_rate,
            std_divergence,
            health_score: self.health_score,
            has_divergence_anomaly: self.divergence_anomaly(),
            has_abstention_anomaly: self.abstention_trend(),
            sample_count: n,
        }
    }
}

/// 质量趋势报告 — 汇总统计摘要
///
/// 包含窗口内所有指标的均值、标准差和异常标志，
/// 供上层（自适应策略选择器、TUI 监控面板）消费。
#[derive(Debug, Clone, PartialEq)]
pub struct QualityReport {
    /// 平均赞成率
    pub avg_approval_rate: f32,
    /// 平均弃权率
    pub avg_abstention_rate: f32,
    /// 平均分歧度
    pub avg_divergence: f32,
    /// 赞成率标准差
    pub std_approval_rate: f32,
    /// 分歧度标准差
    pub std_divergence: f32,
    /// 当前健康评分 (0-100)
    pub health_score: u8,
    /// 是否有分歧异常
    pub has_divergence_anomaly: bool,
    /// 是否有弃权趋势异常
    pub has_abstention_anomaly: bool,
    /// 窗口条目数
    pub sample_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用 ConsensusQualityMetrics
    fn make_metrics(
        approval_rate: f32,
        abstention_rate: f32,
        divergence: f32,
    ) -> ConsensusQualityMetrics {
        ConsensusQualityMetrics {
            approval_rate,
            abstention_rate,
            divergence,
            consensus_margin: approval_rate - 0.6,
            skeptic_stance: 0.5,
        }
    }

    // ============================================================
    // 正常趋势测试
    // ============================================================

    #[test]
    fn test_normal_trend_no_anomaly() {
        // 正常趋势：高赞成率、低弃权率、低分歧度
        let mut analyzer = QualityTrendAnalyzer::new(None);

        for _ in 0..20 {
            analyzer.push(make_metrics(0.85, 0.1, 0.2));
        }

        assert!(!analyzer.divergence_anomaly(), "正常趋势不应有分歧异常");
        assert!(!analyzer.abstention_trend(), "正常趋势不应有弃权趋势");
        assert_eq!(
            analyzer.consensus_health_score(),
            100,
            "正常趋势应保持 100 分"
        );
    }

    #[test]
    fn test_normal_trend_initial_state() {
        // 初始状态：空窗口，健康评分 100
        let analyzer = QualityTrendAnalyzer::new(None);
        assert_eq!(analyzer.consensus_health_score(), 100);
        assert!(!analyzer.divergence_anomaly());
        assert!(!analyzer.abstention_trend());

        let report = analyzer.generate_report();
        assert_eq!(report.sample_count, 0);
        assert_eq!(report.health_score, 100);
    }

    // ============================================================
    // 分歧异常检测测试
    // ============================================================

    #[test]
    fn test_divergence_anomaly_detection() {
        // 连续 5 次分歧度 > 0.7 → 触发异常
        let mut analyzer = QualityTrendAnalyzer::new(None);

        // 前 4 次：不触发
        for _ in 0..4 {
            analyzer.push(make_metrics(0.5, 0.2, 0.8));
            assert!(!analyzer.divergence_anomaly(), "4 次分歧应不触发异常");
        }

        // 第 5 次：触发
        analyzer.push(make_metrics(0.5, 0.2, 0.8));
        assert!(analyzer.divergence_anomaly(), "连续 5 次分歧应触发异常");
    }

    #[test]
    fn test_divergence_anomaly_resets_on_normal() {
        // 连续 5 次异常后，正常值应重置计数器
        let mut analyzer = QualityTrendAnalyzer::new(None);

        // 5 次异常
        for _ in 0..5 {
            analyzer.push(make_metrics(0.5, 0.2, 0.8));
        }
        assert!(analyzer.divergence_anomaly());

        // 正常值恢复
        analyzer.push(make_metrics(0.85, 0.1, 0.2));
        assert!(!analyzer.divergence_anomaly(), "正常值应重置分歧异常计数器");
    }

    #[test]
    fn test_divergence_anomaly_edge_threshold() {
        // 边界测试：divergence 恰好等于 0.7 不应触发异常
        let mut analyzer = QualityTrendAnalyzer::new(None);
        for _ in 0..5 {
            analyzer.push(make_metrics(0.5, 0.2, 0.7));
        }
        // 0.7 不 > 0.7，不触发
        assert!(
            !analyzer.divergence_anomaly(),
            "divergence=0.7 不应触发异常"
        );

        // 0.7001 > 0.7，触发
        let mut analyzer2 = QualityTrendAnalyzer::new(None);
        for _ in 0..5 {
            analyzer2.push(make_metrics(0.5, 0.2, 0.7001));
        }
        assert!(
            analyzer2.divergence_anomaly(),
            "divergence=0.7001 应触发异常"
        );
    }

    // ============================================================
    // 弃权趋势检测测试
    // ============================================================

    #[test]
    fn test_abstention_trend_detection() {
        // 连续 10 次弃权率 > 0.4 → 触发趋势
        let mut analyzer = QualityTrendAnalyzer::new(None);

        // 前 9 次：不触发
        for _ in 0..9 {
            analyzer.push(make_metrics(0.5, 0.5, 0.3));
            assert!(!analyzer.abstention_trend(), "9 次弃权应不触发趋势");
        }

        // 第 10 次：触发
        analyzer.push(make_metrics(0.5, 0.5, 0.3));
        assert!(analyzer.abstention_trend(), "连续 10 次弃权应触发趋势");
    }

    #[test]
    fn test_abstention_trend_resets_on_normal() {
        let mut analyzer = QualityTrendAnalyzer::new(None);

        // 10 次弃权趋势
        for _ in 0..10 {
            analyzer.push(make_metrics(0.5, 0.5, 0.3));
        }
        assert!(analyzer.abstention_trend());

        // 正常值恢复
        analyzer.push(make_metrics(0.85, 0.1, 0.2));
        assert!(!analyzer.abstention_trend(), "正常值应重置弃权趋势计数器");
    }

    // ============================================================
    // 健康评分计算测试
    // ============================================================

    #[test]
    fn test_health_score_divergence_deduction() {
        // 分歧异常扣 20 分
        let mut analyzer = QualityTrendAnalyzer::new(None);
        for _ in 0..5 {
            analyzer.push(make_metrics(0.5, 0.2, 0.8));
        }
        // 异常扣 20，无低赞成率（approval_rate=0.5 不 < 0.5）
        assert_eq!(analyzer.consensus_health_score(), 80);
    }

    #[test]
    fn test_health_score_abstention_deduction() {
        // 弃权趋势扣 15 分
        let mut analyzer = QualityTrendAnalyzer::new(None);
        for _ in 0..10 {
            analyzer.push(make_metrics(0.6, 0.5, 0.3));
        }
        // 弃权趋势扣 15，无低赞成率（approval_rate=0.6 ≥ 0.5）
        assert_eq!(analyzer.consensus_health_score(), 85);
    }

    #[test]
    fn test_health_score_low_approval_deduction() {
        // 低赞成率每条扣 10 分
        let mut analyzer = QualityTrendAnalyzer::new(None);
        // 推入 3 条低赞成率
        for _ in 0..3 {
            analyzer.push(make_metrics(0.3, 0.2, 0.3));
        }
        // 3 条低赞成率 × 10 = 30 分
        assert_eq!(analyzer.consensus_health_score(), 70);
    }

    #[test]
    fn test_health_score_combined_deductions() {
        // 同时触发多种扣分
        let mut analyzer = QualityTrendAnalyzer::new(None);

        // 5 次分歧异常 + 低赞成率
        for _ in 0..5 {
            analyzer.push(make_metrics(0.3, 0.2, 0.8));
        }
        // 分歧异常扣 20 + 5 条低赞成率 × 10 = 50
        // 100 - 20 - 50 = 30
        assert_eq!(analyzer.consensus_health_score(), 30);
    }

    #[test]
    fn test_health_score_clamped_to_zero() {
        // 健康评分不应低于 0
        let mut analyzer = QualityTrendAnalyzer::new(None);

        // 20 条低赞成率 + 分歧异常
        for _ in 0..20 {
            analyzer.push(make_metrics(0.1, 0.2, 0.8));
        }
        // 分歧异常扣 20 + 20 条低赞成率 × 10 = 200 → 220 扣分 → 钳制到 0
        assert_eq!(analyzer.consensus_health_score(), 0, "健康评分应钳制到 0");
    }

    #[test]
    fn test_health_score_no_change_on_normal_values() {
        // 正常值：健康评分保持 100
        let mut analyzer = QualityTrendAnalyzer::new(None);
        for _ in 0..20 {
            analyzer.push(make_metrics(0.9, 0.05, 0.1));
        }
        assert_eq!(analyzer.consensus_health_score(), 100);
    }

    // ============================================================
    // 边界条件测试
    // ============================================================

    #[test]
    fn test_boundary_empty_window_report() {
        let analyzer = QualityTrendAnalyzer::new(None);
        let report = analyzer.generate_report();

        assert_eq!(report.sample_count, 0);
        assert_eq!(report.avg_approval_rate, 0.0);
        assert_eq!(report.avg_abstention_rate, 0.0);
        assert_eq!(report.avg_divergence, 0.0);
        assert_eq!(report.std_approval_rate, 0.0);
        assert_eq!(report.std_divergence, 0.0);
        assert!(!report.has_divergence_anomaly);
        assert!(!report.has_abstention_anomaly);
        assert_eq!(report.health_score, 100);
    }

    #[test]
    fn test_boundary_single_entry() {
        // 单条记录：报告应正确计算
        let mut analyzer = QualityTrendAnalyzer::new(None);
        analyzer.push(make_metrics(0.8, 0.1, 0.2));

        let report = analyzer.generate_report();
        assert_eq!(report.sample_count, 1);
        assert!((report.avg_approval_rate - 0.8).abs() < 1e-6);
        assert!((report.avg_abstention_rate - 0.1).abs() < 1e-6);
        assert!((report.avg_divergence - 0.2).abs() < 1e-6);
        // 单个条目标准差为 0
        assert!(report.std_approval_rate.abs() < 1e-6);
        assert!(report.std_divergence.abs() < 1e-6);
    }

    #[test]
    fn test_boundary_window_full_eviction() {
        // 窗口满后应自动淘汰最早条目
        let mut analyzer = QualityTrendAnalyzer::new(Some(3));

        analyzer.push(make_metrics(0.9, 0.1, 0.2));
        analyzer.push(make_metrics(0.8, 0.2, 0.3));
        analyzer.push(make_metrics(0.7, 0.3, 0.4));
        assert_eq!(analyzer.window.len(), 3);

        // 第 4 条推入，第 1 条被淘汰
        analyzer.push(make_metrics(0.6, 0.4, 0.5));
        assert_eq!(analyzer.window.len(), 3);

        // 验证最早的条目已被淘汰：窗口条目应为第 2、3、4 条
        let report = analyzer.generate_report();
        assert_eq!(report.sample_count, 3);
        // 均值应为 (0.8 + 0.7 + 0.6) / 3 ≈ 0.7
        assert!((report.avg_approval_rate - 0.7).abs() < 1e-6);
    }

    #[test]
    fn test_boundary_custom_window_size() {
        // 自定义窗口大小
        let mut analyzer = QualityTrendAnalyzer::new(Some(5));
        for _ in 0..10 {
            analyzer.push(make_metrics(0.5, 0.2, 0.3));
        }
        assert_eq!(analyzer.window.len(), 5, "自定义窗口大小应生效");
    }

    #[test]
    fn test_boundary_window_size_zero() {
        // max_size = 0 的退化情况：窗口始终只有最新 1 条
        // 实际上结果显示为 1 条（VecDeque 的 push_back 在 len >= 0 时 pop_front 再 push）
        let mut analyzer = QualityTrendAnalyzer::new(Some(0));
        analyzer.push(make_metrics(0.9, 0.1, 0.2));
        assert_eq!(analyzer.window.len(), 1, "max_size=0 时窗口保持 1 条");
        analyzer.push(make_metrics(0.8, 0.2, 0.3));
        assert_eq!(analyzer.window.len(), 1, "新条目替换旧条目");
    }

    // ============================================================
    // Health score 反馈策略选择测试
    // ============================================================

    #[test]
    fn test_health_score_feedback_high_score() {
        // 高健康评分（≥ 80）→ 策略可保持 Full 级别
        let mut analyzer = QualityTrendAnalyzer::new(None);
        for _ in 0..10 {
            analyzer.push(make_metrics(0.85, 0.1, 0.2));
        }
        let score = analyzer.consensus_health_score();
        // 高健康评分应 ≥ 80
        assert!(score >= 80, "高健康评分应 ≥ 80, 实际: {score}");
        // 高健康评分下不应有任何异常
        assert!(!analyzer.divergence_anomaly());
        assert!(!analyzer.abstention_trend());
    }

    #[test]
    fn test_health_score_feedback_medium_score() {
        // 中等健康评分（50-79）→ 需降级策略，如 Simplified
        let mut analyzer = QualityTrendAnalyzer::new(None);
        // 分歧异常扣 20 + 5 条低赞成率 × 10 = 70 → 健康评分 30
        // 需要更精确控制：仅分歧异常，赞成率保持 ≥ 0.5
        for _ in 0..5 {
            analyzer.push(make_metrics(0.5, 0.2, 0.8));
        }
        // 5 条低赞成率？approval_rate=0.5 不 < 0.5 → 只扣分歧 20 → 80
        // 再加 4 条低赞成率(0.3)：
        for _ in 0..4 {
            analyzer.push(make_metrics(0.3, 0.2, 0.8));
        }
        // 4 条低赞成率 × 10 = 40 + 分歧异常 20 = 60 扣分 → 40
        // 这个测试验证健康评分可被上层用于策略降级决策
        let score = analyzer.consensus_health_score();
        assert!(score < 50, "低健康评分应 < 50, 实际: {score}");
    }

    #[test]
    fn test_health_score_feedback_low_score_triggers_conservative() {
        // 极低健康评分应触发保守策略选择
        let mut analyzer = QualityTrendAnalyzer::new(None);
        // 20 条全低质量
        for _ in 0..20 {
            analyzer.push(make_metrics(0.2, 0.5, 0.8));
        }
        let score = analyzer.consensus_health_score();
        // 异常 + 大量低赞成率 → 0 分
        assert_eq!(score, 0, "极低质量应得 0 分");

        // 验证所有异常标志均触发
        assert!(analyzer.divergence_anomaly(), "极低质量应触发分歧异常");
        assert!(analyzer.abstention_trend(), "极低质量应触发弃权趋势");

        // 上层可根据 health_score 选择策略：
        let suggested_strategy = if score >= 80 {
            "Full"
        } else if score >= 50 {
            "Simplified"
        } else {
            "FastPath"
        };
        assert_eq!(suggested_strategy, "FastPath", "0 分应建议 FastPath");
    }

    // ============================================================
    // 报告生成测试
    // ============================================================

    #[test]
    fn test_generate_report_accurate_stats() {
        let mut analyzer = QualityTrendAnalyzer::new(None);

        // 推入 5 条已知数据
        let data = [
            (0.9, 0.05, 0.1),
            (0.8, 0.10, 0.2),
            (0.7, 0.15, 0.3),
            (0.6, 0.20, 0.4),
            (0.5, 0.25, 0.5),
        ];
        for (apr, abr, div) in &data {
            analyzer.push(make_metrics(*apr, *abr, *div));
        }

        let report = analyzer.generate_report();

        // 均值验证
        assert!((report.avg_approval_rate - 0.7).abs() < 1e-6);
        assert!((report.avg_abstention_rate - 0.15).abs() < 1e-6);
        assert!((report.avg_divergence - 0.3).abs() < 1e-6);
        assert_eq!(report.sample_count, 5);
    }

    #[test]
    fn test_generate_report_no_anomalies() {
        // 正常趋势下报告不应有异常标志
        let mut analyzer = QualityTrendAnalyzer::new(None);
        for _ in 0..10 {
            analyzer.push(make_metrics(0.85, 0.1, 0.2));
        }
        let report = analyzer.generate_report();
        assert!(!report.has_divergence_anomaly);
        assert!(!report.has_abstention_anomaly);
        assert_eq!(report.health_score, 100);
    }

    // ============================================================
    // Proptest: 任意输入下不 panic
    // ============================================================

    proptest::proptest! {
        /// 属性：任意合法 ConsensusQualityMetrics 输入下，所有方法不 panic
        #[test]
        fn prop_quality_trend_never_panics(
            approval_rate in 0.0f32..=1.0,
            abstention_rate in 0.0f32..=1.0,
            divergence in 0.0f32..=1.0,
            consensus_margin in -1.0f32..=1.0,
            skeptic_stance in 0.0f32..=1.0,
            iterations in 0usize..30,
        ) {
            let mut analyzer = QualityTrendAnalyzer::new(None);
            for _ in 0..iterations {
                let metrics = ConsensusQualityMetrics {
                    approval_rate,
                    abstention_rate,
                    divergence,
                    consensus_margin,
                    skeptic_stance,
                };
                analyzer.push(metrics);
            }
            // 所有方法应在任意输入下不 panic
            let _ = analyzer.divergence_anomaly();
            let _ = analyzer.abstention_trend();
            let score = analyzer.consensus_health_score();
            let report = analyzer.generate_report();

            // 验证健康评分在 [0, 100] 范围内
            proptest::prop_assert!((0..=100).contains(&score), "health_score ∈ [0,100]");
            proptest::prop_assert!((0..=100).contains(&report.health_score), "report.health_score ∈ [0,100]");
            // 报告字段应在合法范围内
            proptest::prop_assert!((0.0..=1.0).contains(&report.avg_approval_rate), "avg_approval_rate ∈ [0,1]");
            proptest::prop_assert!((0.0..=1.0).contains(&report.avg_abstention_rate), "avg_abstention_rate ∈ [0,1]");
            proptest::prop_assert!((0.0..=1.0).contains(&report.avg_divergence), "avg_divergence ∈ [0,1]");
            proptest::prop_assert!((0.0..=1.0).contains(&report.std_approval_rate), "std_approval_rate ∈ [0,1]");
            proptest::prop_assert!((0.0..=1.0).contains(&report.std_divergence), "std_divergence ∈ [0,1]");
            proptest::prop_assert_eq!(report.sample_count, iterations.min(20), "sample_count 应为 min(iterations, 20)");
        }
    }
}
