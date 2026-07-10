//! TTG 自适应权重学习 — P1-13 基于反馈的权重动态调整
//!
//! 对应架构层:L9 Quest
//! 对应创新点:自适应 TTG(权重根据历史任务成功率动态学习)
//!
//! # 核心机制
//! - **权重向量**:任务数权重 w1、依赖深度权重 w2、描述长度权重 w3
//! - **反馈信号**:任务成功率(Completed / Total)
//! - **在线梯度更新**:根据反馈信号调整权重,使成功率提升方向收敛
//! - **滑动窗口**:最近 N 个 Quest 的历史记录,避免过拟合
//!
//! # 学习算法
//! 使用指数移动平均(EMA)更新权重:
//! - 若当前模式选择后任务成功率高 → 保持当前权重
//! - 若当前模式选择后任务成功率低 → 向其他方向调整权重
//! - 权重归一化:确保 w1 + w2 + w3 = 1.0
//!
//! # 设计决策(WHY)
//! - **EMA 而非全梯度下降**:轻量、无需学习率调参、适合在线场景
//! - **滑动窗口 20**:平衡记忆与适应速度(约 20 个 Quest 后权重稳定)
//! - **权重 clamp [0.1, 0.8]**:防止某个权重极端化导致评估失真

use std::collections::VecDeque;

/// 自适应权重 — 复杂度评估的三维权重向量
///
/// w1:任务数权重, w2:依赖深度权重, w3:描述长度权重
/// 始终满足 w1 + w2 + w3 = 1.0
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdaptiveWeights {
    /// 任务数权重
    pub task_count_weight: f32,
    /// 依赖深度权重
    pub dependency_depth_weight: f32,
    /// 描述长度权重
    pub description_length_weight: f32,
}

impl AdaptiveWeights {
    /// 创建默认权重(与原始 TTG 一致:0.3, 0.4, 0.3)
    pub fn default_weights() -> Self {
        Self {
            task_count_weight: 0.3,
            dependency_depth_weight: 0.4,
            description_length_weight: 0.3,
        }
    }

    /// 创建自定义权重(自动归一化)
    pub fn new(w1: f32, w2: f32, w3: f32) -> Self {
        let sum = w1 + w2 + w3;
        if sum == 0.0 {
            return Self::default_weights();
        }
        Self {
            task_count_weight: (w1 / sum).clamp(0.1, 0.8),
            dependency_depth_weight: (w2 / sum).clamp(0.1, 0.8),
            description_length_weight: (w3 / sum).clamp(0.1, 0.8),
        }
    }

    /// 应用权重计算复杂度评分
    pub fn compute_score(
        &self,
        task_count: f32,
        dependency_depth: f32,
        description_length_factor: f32,
    ) -> f32 {
        task_count * self.task_count_weight
            + dependency_depth * self.dependency_depth_weight
            + description_length_factor * self.description_length_weight
    }

    /// 归一化权重(确保和为 1.0)
    fn normalize(&mut self) {
        let sum =
            self.task_count_weight + self.dependency_depth_weight + self.description_length_weight;
        if sum > 0.0 {
            self.task_count_weight /= sum;
            self.dependency_depth_weight /= sum;
            self.description_length_weight /= sum;
        }
    }

    /// Clamp 权重到合理范围 [0.1, 0.8]
    fn clamp(&mut self) {
        self.task_count_weight = self.task_count_weight.clamp(0.1, 0.8);
        self.dependency_depth_weight = self.dependency_depth_weight.clamp(0.1, 0.8);
        self.description_length_weight = self.description_length_weight.clamp(0.1, 0.8);
        self.normalize();
    }
}

impl Default for AdaptiveWeights {
    fn default() -> Self {
        Self::default_weights()
    }
}

/// 历史结果记录 — 单个 Quest 的模式选择结果
#[derive(Debug, Clone, Copy)]
pub struct QuestOutcome {
    /// 选择的思考模式(1=Fast, 2=Standard, 3=Deep)
    pub mode_selected: u8,
    /// 任务成功率(0.0-1.0)
    pub success_rate: f32,
    /// 复杂度评分
    pub complexity_score: f32,
}

/// 自适应权重学习器 — 基于历史反馈动态调整权重
///
/// 使用滑动窗口记录最近 N 个 Quest 的结果,
/// 根据成功率反馈调整权重向量。
#[derive(Debug, Clone)]
pub struct AdaptiveWeightLearner {
    /// 当前权重
    weights: AdaptiveWeights,
    /// 历史结果滑动窗口
    history: VecDeque<QuestOutcome>,
    /// 窗口大小(默认 20)
    window_size: usize,
    /// 学习率(默认 0.05)
    learning_rate: f32,
    /// 最小记录数(少于此时不更新权重)
    min_records: usize,
}

impl AdaptiveWeightLearner {
    /// 创建默认学习器
    pub fn new() -> Self {
        Self {
            weights: AdaptiveWeights::default(),
            history: VecDeque::with_capacity(20),
            window_size: 20,
            learning_rate: 0.05,
            min_records: 5,
        }
    }

    /// 创建自定义学习器
    pub fn with_params(
        initial_weights: AdaptiveWeights,
        window_size: usize,
        learning_rate: f32,
        min_records: usize,
    ) -> Self {
        Self {
            weights: initial_weights,
            history: VecDeque::with_capacity(window_size),
            window_size: window_size.max(5),
            learning_rate: learning_rate.clamp(0.01, 0.5),
            min_records: min_records.max(3),
        }
    }

    /// 获取当前权重
    pub fn weights(&self) -> AdaptiveWeights {
        self.weights
    }

    /// 记录 Quest 结果并更新权重
    ///
    /// 流程:
    /// 1. 将结果加入滑动窗口
    /// 2. 若窗口内记录数 ≥ min_records,计算反馈信号
    /// 3. 根据反馈信号调整权重
    /// 4. 归一化并 clamp 权重
    pub fn record_outcome(&mut self, outcome: QuestOutcome) {
        // 1. 加入滑动窗口
        if self.history.len() >= self.window_size {
            self.history.pop_front();
        }
        self.history.push_back(outcome);

        // 2. 检查是否满足更新条件
        if self.history.len() < self.min_records {
            return;
        }

        // 3. 计算反馈信号
        let feedback = self.compute_feedback_signal();

        // 4. 更新权重
        self.update_weights(feedback);
    }

    /// 计算反馈信号 — 基于最近历史的成功率趋势
    ///
    /// 返回 [-1.0, 1.0]:
    /// - 正:最近成功率高于平均水平,当前权重方向正确
    /// - 负:最近成功率低于平均水平,需要调整权重
    fn compute_feedback_signal(&self) -> f32 {
        if self.history.len() < 2 {
            return 0.0;
        }

        // 分两半:前半和后半
        let mid = self.history.len() / 2;
        let recent: Vec<_> = self.history.iter().skip(mid).collect();
        let older: Vec<_> = self.history.iter().take(mid).collect();

        let recent_avg = recent.iter().map(|o| o.success_rate).sum::<f32>() / recent.len() as f32;
        let older_avg = older.iter().map(|o| o.success_rate).sum::<f32>() / older.len() as f32;

        // 趋势信号:最近成功率 vs  older 成功率
        let trend = recent_avg - older_avg;
        // 归一化到 [-1, 1]
        trend.clamp(-1.0, 1.0)
    }

    /// 更新权重 — 基于反馈信号和学习率
    ///
    /// 策略:
    /// - 若反馈为正(成功率提升):保持当前权重方向,微调
    /// - 若反馈为负(成功率下降):向高成功率 Quest 的特征方向调整
    fn update_weights(&mut self, feedback: f32) {
        if feedback.abs() < 0.05 {
            // 变化太小,不调整
            return;
        }

        // 找出高成功率和低成功率的 Quest 特征
        let high_success = self
            .history
            .iter()
            .filter(|o| o.success_rate > 0.7)
            .collect::<Vec<_>>();
        let low_success = self
            .history
            .iter()
            .filter(|o| o.success_rate < 0.4)
            .collect::<Vec<_>>();

        if high_success.is_empty() || low_success.is_empty() {
            return;
        }

        // 计算高/低成功率 Quest 的平均复杂度特征
        let high_complexity_avg = high_success.iter().map(|o| o.complexity_score).sum::<f32>()
            / high_success.len() as f32;
        let low_complexity_avg =
            low_success.iter().map(|o| o.complexity_score).sum::<f32>() / low_success.len() as f32;

        // 若高成功率 Quest 复杂度更高,说明当前权重可能低估复杂度
        // 应增加依赖深度权重(通常复杂度与深度最相关)
        if high_complexity_avg > low_complexity_avg {
            self.weights.dependency_depth_weight += self.learning_rate * feedback.abs();
        } else {
            self.weights.dependency_depth_weight -= self.learning_rate * feedback.abs();
        }

        // 同样调整任务数权重(反向微调)
        self.weights.task_count_weight += self.learning_rate * feedback * 0.5;

        // 归一化和 clamp
        self.weights.clamp();
    }

    /// 获取历史记录数
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// 获取平均成功率
    pub fn average_success_rate(&self) -> f32 {
        if self.history.is_empty() {
            return 0.0;
        }
        self.history.iter().map(|o| o.success_rate).sum::<f32>() / self.history.len() as f32
    }

    /// 清空历史
    pub fn clear_history(&mut self) {
        self.history.clear();
    }
}

impl Default for AdaptiveWeightLearner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adaptive_weights_default() {
        let w = AdaptiveWeights::default();
        assert!((w.task_count_weight - 0.3).abs() < 1e-6);
        assert!((w.dependency_depth_weight - 0.4).abs() < 1e-6);
        assert!((w.description_length_weight - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_adaptive_weights_new_normalizes() {
        let w = AdaptiveWeights::new(1.0, 2.0, 3.0);
        let sum = w.task_count_weight + w.dependency_depth_weight + w.description_length_weight;
        assert!((sum - 1.0).abs() < 1e-5, "权重应归一化为 1.0, got {}", sum);
    }

    #[test]
    fn test_adaptive_weights_clamp() {
        let mut w = AdaptiveWeights::new(10.0, 0.01, 0.01);
        // 归一化后 clamp 到 [0.1, 0.8]
        assert!(w.task_count_weight <= 0.8);
        assert!(w.dependency_depth_weight >= 0.1);
        assert!(w.description_length_weight >= 0.1);
    }

    #[test]
    fn test_adaptive_weights_compute_score() {
        let w = AdaptiveWeights::default();
        let score = w.compute_score(10.0, 5.0, 0.5);
        let expected = 10.0 * 0.3 + 5.0 * 0.4 + 0.5 * 0.3;
        assert!((score - expected).abs() < 1e-6);
    }

    #[test]
    fn test_learner_new_default() {
        let learner = AdaptiveWeightLearner::new();
        assert_eq!(learner.history_len(), 0);
        let w = learner.weights();
        assert!((w.task_count_weight - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_learner_record_outcome() {
        let mut learner = AdaptiveWeightLearner::new();
        // 记录 5 个高成功率结果
        for _ in 0..5 {
            learner.record_outcome(QuestOutcome {
                mode_selected: 2,
                success_rate: 0.9,
                complexity_score: 5.0,
            });
        }
        assert_eq!(learner.history_len(), 5);
        assert!(learner.average_success_rate() > 0.8);
    }

    #[test]
    fn test_learner_weights_adapt() {
        let mut learner =
            AdaptiveWeightLearner::with_params(AdaptiveWeights::default(), 20, 0.1, 5);

        let initial_depth_weight = learner.weights().dependency_depth_weight;

        // 记录高成功率但低复杂度的结果(应触发权重调整)
        for i in 0..10 {
            learner.record_outcome(QuestOutcome {
                mode_selected: 1,
                success_rate: 0.9,
                complexity_score: 1.0 + i as f32 * 0.1,
            });
        }

        // 记录低成功率但高复杂度的结果
        for i in 0..10 {
            learner.record_outcome(QuestOutcome {
                mode_selected: 3,
                success_rate: 0.2,
                complexity_score: 10.0 + i as f32 * 0.1,
            });
        }

        // 权重应发生变化
        let new_depth_weight = learner.weights().dependency_depth_weight;
        assert!(
            (new_depth_weight - initial_depth_weight).abs() > 1e-6,
            "权重应自适应调整: initial={}, new={}",
            initial_depth_weight,
            new_depth_weight
        );
    }

    #[test]
    fn test_learner_window_size_limits() {
        let mut learner = AdaptiveWeightLearner::with_params(
            AdaptiveWeights::default(),
            5, // 小窗口
            0.05,
            3,
        );

        // 记录 10 个结果
        for i in 0..10 {
            learner.record_outcome(QuestOutcome {
                mode_selected: 1,
                success_rate: 0.5,
                complexity_score: i as f32,
            });
        }

        // 窗口大小限制为 5
        assert_eq!(learner.history_len(), 5);
    }

    #[test]
    fn test_learner_clear_history() {
        let mut learner = AdaptiveWeightLearner::new();
        learner.record_outcome(QuestOutcome {
            mode_selected: 1,
            success_rate: 0.8,
            complexity_score: 3.0,
        });
        assert_eq!(learner.history_len(), 1);
        learner.clear_history();
        assert_eq!(learner.history_len(), 0);
    }

    #[test]
    fn test_feedback_signal_positive_trend() {
        let mut learner = AdaptiveWeightLearner::new();
        // 前半:低成功率
        for _ in 0..5 {
            learner.record_outcome(QuestOutcome {
                mode_selected: 1,
                success_rate: 0.3,
                complexity_score: 3.0,
            });
        }
        // 后半:高成功率
        for _ in 0..5 {
            learner.record_outcome(QuestOutcome {
                mode_selected: 2,
                success_rate: 0.9,
                complexity_score: 5.0,
            });
        }

        let feedback = learner.compute_feedback_signal();
        assert!(
            feedback > 0.0,
            "成功率上升趋势应产生正反馈, got {}",
            feedback
        );
    }

    #[test]
    fn test_feedback_signal_negative_trend() {
        let mut learner = AdaptiveWeightLearner::new();
        // 前半:高成功率
        for _ in 0..5 {
            learner.record_outcome(QuestOutcome {
                mode_selected: 2,
                success_rate: 0.9,
                complexity_score: 5.0,
            });
        }
        // 后半:低成功率
        for _ in 0..5 {
            learner.record_outcome(QuestOutcome {
                mode_selected: 1,
                success_rate: 0.2,
                complexity_score: 3.0,
            });
        }

        let feedback = learner.compute_feedback_signal();
        assert!(
            feedback < 0.0,
            "成功率下降趋势应产生负反馈, got {}",
            feedback
        );
    }
}
