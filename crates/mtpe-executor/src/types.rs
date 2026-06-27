//! MTPE 核心类型定义 — 多步预测执行的数据模型
//!
//! 对应架构层:L7 Execution
//! 对应创新点:MTPE(Multi-Token Prediction Execution)
//!
//! # 类型关系
//! ```text
//! PredictionContext ──predict(n)──▶ PredictionResult
//!                                        │
//!                                        ▼
//!                                  record_verification
//!                                        │
//!                                        ▼
//!                                PredictionStats
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// 预测 Token — 单步预测产出
///
/// WHY 独立结构而非裸 String:携带 confidence 供 PVL 验证层评分,
/// 低置信度 token 可触发回退(见 fallback 模块)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    /// 预测文本
    pub text: String,
    /// 置信度 [0.0, 1.0],步数越高通常越低(误差累积)
    pub confidence: f32,
}

/// 预测上下文 — 执行多步预测所需的输入
///
/// - `quest_id`:关联 Quest,用于事件追踪与审计
/// - `history`:历史对话/代码上下文,伪预测基于最后一个元素哈希
/// - `clv`:Context Latent Vector(512-dim),Week 6 NMC 接入后用于真实语义路由
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionContext {
    /// 关联的 Quest ID
    pub quest_id: String,
    /// 历史上下文(对话轮次或代码片段)
    pub history: Vec<String>,
    /// 上下文潜在向量(512 维),Week 6 NMC 产出
    pub clv: Vec<f32>,
}

/// 预测结果 — 一次 predict 调用的产出
///
/// - `predicted_tokens`:N 个预测 token,长度等于调用时的 n
/// - `latency_ms`:本次预测耗时,用于性能监控与加速比验证
/// - `n`:实际预测步数(冗余字段,便于消费者不依赖调用方)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PredictionResult {
    /// 预测的 N 个 token
    pub predicted_tokens: Vec<Token>,
    /// 预测延迟(毫秒)
    pub latency_ms: f32,
    /// 预测步数
    pub n: usize,
}

/// 预测统计 — 按 N 值分组的成功率统计
///
/// WHY 按 N 分组:不同 N 值成功率差异显著(N=1 接近 100%,
/// N=10 可能低于 60%),分组统计支持动态 N 值选择策略
///
/// `success_rate_by_n` 映射:N → (total, success)
/// - total:该 N 值的总预测次数
/// - success:其中被 PVL 验证为成功的次数
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PredictionStats {
    /// 总预测次数(所有 N 值合计)
    pub total_predictions: u64,
    /// 成功预测次数(所有 N 值合计)
    pub successful_predictions: u64,
    /// 按 N 值分组的 (总数, 成功数)
    pub success_rate_by_n: HashMap<usize, (u64, u64)>,
}

impl PredictionStats {
    /// 创建空统计
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次验证结果
    ///
    /// WHY 内联此方法:统计逻辑简单且与 stats 字段强耦合,
    /// 内联避免跨模块调用开销,同时保持单一数据写入入口
    pub fn record(&mut self, n: usize, success: bool) {
        self.total_predictions += 1;
        if success {
            self.successful_predictions += 1;
        }
        let entry = self.success_rate_by_n.entry(n).or_insert((0, 0));
        entry.0 += 1;
        if success {
            entry.1 += 1;
        }
    }

    /// 获取指定 N 值的成功率
    ///
    /// 返回 0.0 表示无记录(避免除零),调用方可据此判断是否需要降级 N
    pub fn success_rate(&self, n: usize) -> f32 {
        match self.success_rate_by_n.get(&n) {
            Some((total, success)) if *total > 0 => *success as f32 / *total as f32,
            _ => 0.0,
        }
    }

    /// 导出为事件所需的 `HashMap<usize, f32>` 格式
    ///
    /// WHY 单独方法:事件 `PredictionStatsReported` 字段类型为
    /// `HashMap<usize, f32>`,与内部 `(u64, u64)` 不同,集中转换避免散落
    pub fn to_rate_map(&self) -> HashMap<usize, f32> {
        self.success_rate_by_n
            .iter()
            .map(|(&n, &(_total, _success))| (n, self.success_rate(n)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_serialization() {
        let token = Token {
            text: "hello".into(),
            confidence: 0.95,
        };
        let json = serde_json::to_string(&token).unwrap();
        let restored: Token = serde_json::from_str(&json).unwrap();
        assert_eq!(token, restored);
    }

    #[test]
    fn test_prediction_stats_record() {
        let mut stats = PredictionStats::new();
        stats.record(5, true);
        stats.record(5, true);
        stats.record(5, false);

        assert_eq!(stats.total_predictions, 3);
        assert_eq!(stats.successful_predictions, 2);
        assert_eq!(stats.success_rate(5), 2.0 / 3.0);
    }

    #[test]
    fn test_prediction_stats_grouped() {
        let mut stats = PredictionStats::new();
        // N=1: 4 次成功
        for _ in 0..4 {
            stats.record(1, true);
        }
        // N=5: 3 次成功,1 次失败
        for _ in 0..3 {
            stats.record(5, true);
        }
        stats.record(5, false);

        assert_eq!(stats.success_rate(1), 1.0);
        assert_eq!(stats.success_rate(5), 0.75);
        assert_eq!(stats.success_rate(10), 0.0); // 无记录
    }

    #[test]
    fn test_to_rate_map() {
        let mut stats = PredictionStats::new();
        stats.record(1, true);
        stats.record(1, true);
        stats.record(5, false);

        let map = stats.to_rate_map();
        assert_eq!(map.get(&1), Some(&1.0));
        assert_eq!(map.get(&5), Some(&0.0));
    }
}
