//! MTPE 多步预测执行器 — 核心预测逻辑
//!
//! 对应架构层:L7 Execution
//! 对应创新点:MTPE(Multi-Token Prediction Execution)
//!
//! # 设计要点
//! - 一次推理预测 N 个 token,减少推理调用次数,加速吞吐
//! - N ∈ [1, 10],N=1 退化为单步预测(基准),N=10 为上限
//! - Week 4 占位实现:基于上下文哈希的伪预测,验证架构
//! - Week 6 NMC 实现后接入真实模型

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tokio::sync::RwLock;
use tracing::debug;

use crate::config::MtpeConfig;
use crate::error::MtpeError;
use crate::types::{PredictionContext, PredictionResult, PredictionStats, Token};

/// 模拟推理启动开销 — 每次 predict 调用的固定延迟
///
/// WHY 固定延迟:真实推理中,模型启动/上下文编码的开销远大于生成单个
/// token 的开销,且此开销与 N 无关(一次推理可产出 N 个 token)。
/// MTPE 的核心优势就是减少推理启动次数。伪预测中加入此延迟,
/// 使加速比测试能反映真实场景的加速效果(1000×N=5 vs 5000×N=1)
// TODO(Week 7): SIMULATED_INFERENCE_DELAY 与 generate_pseudo_predictions 为伪实现,
// 替换为真实模型推理延迟与多步预测。
const SIMULATED_INFERENCE_DELAY: Duration = Duration::from_micros(50);

/// MTPE 执行器 — 多步预测执行的核心组件
///
/// 线程安全:内部使用 `RwLock<PredictionStats>` 与 `AtomicU64`,
/// 可在多任务间共享(`&self` 接口)。
///
/// # 字段说明
/// - `config`:运行参数(max_n、成功率阈值、回退开关)
/// - `event_bus`:事件总线,发布 `PredictionMade`/`PredictionStatsReported` 事件
/// - `stats`:按 N 值分组的成功率统计,读写锁保护
/// - `prediction_count`:预测计数器,每 100 次触发统计事件发布
pub struct MtpeExecutor {
    /// 执行器配置
    config: MtpeConfig,
    /// 事件总线(跨层通信唯一通道,§2.2 依赖铁律)
    event_bus: EventBus,
    /// 预测成功率统计(按 N 值分组)
    stats: RwLock<PredictionStats>,
    /// 预测计数器(用于触发周期性统计事件)
    prediction_count: AtomicU64,
}

/// 上下文哈希的稳定种子 — 用于伪预测生成确定性 token
///
/// WHY 常量:伪预测仅需确定性输出,无需密码学强度,用固定种子简化实现
const CONTEXT_HASH_SEED: u32 = 0x4D54_5045; // "MTPE" 的 ASCII

impl MtpeExecutor {
    /// 创建 MTPE 执行器
    pub fn new(config: MtpeConfig, event_bus: EventBus) -> Self {
        Self {
            config,
            event_bus,
            stats: RwLock::new(PredictionStats::new()),
            prediction_count: AtomicU64::new(0),
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &MtpeConfig {
        &self.config
    }

    /// 获取事件总线引用
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 多步预测 — 一次推理预测 N 个 token
    ///
    /// # 参数
    /// - `context`:预测上下文(quest_id、history、clv)
    /// - `n`:预测步数,有效范围 [1, config.max_n]
    ///
    /// # 返回
    /// - `Ok(PredictionResult)`:N 个预测 token + 延迟
    /// - `Err(InvalidN)`:N 值超出范围
    ///
    /// # 事件
    /// 预测完成后发布 `PredictionMade` 事件(携带 quest_id/n/avg_confidence)
    ///
    /// # 占位实现说明
    /// Week 4 使用基于上下文哈希的伪预测:
    /// - Token.text = format!("pred_{}_{}", i, hash_of_context)
    /// - Token.confidence = 1.0 - (i * 0.05),步数越高置信度越低
    pub async fn predict(
        &self,
        context: &PredictionContext,
        n: usize,
    ) -> Result<PredictionResult, MtpeError> {
        // 校验 N 值范围
        if !self.config.is_valid_n(n) {
            return Err(MtpeError::InvalidN {
                n,
                max: self.config.max_n,
            });
        }

        let start = Instant::now();

        // 模拟推理启动开销(与 N 无关的固定延迟)
        // WHY:真实推理中,模型启动/上下文编码开销远大于生成单个 token,
        // 且此开销与 N 无关。MTPE 通过一次推理产出 N 个 token 来摊薄此开销
        tokio::time::sleep(SIMULATED_INFERENCE_DELAY).await;

        // 伪预测:基于上下文哈希生成 N 个确定性 token
        let context_hash = compute_context_hash(context);
        let predicted_tokens = generate_pseudo_predictions(n, context_hash);

        let latency_ms = start.elapsed().as_secs_f32() * 1000.0;

        // 计算平均置信度,用于事件上报
        let avg_confidence = compute_avg_confidence(&predicted_tokens);

        // 更新预测计数器
        let count = self.prediction_count.fetch_add(1, Ordering::Relaxed) + 1;

        // 发布 PredictionMade 事件
        let event = NexusEvent::PredictionMade {
            metadata: EventMetadata::new("mtpe-executor"),
            quest_id: context.quest_id.clone(),
            n,
            avg_confidence,
        };
        // WHY 忽略 publish 错误:无订阅者时事件被静默丢弃,不影响预测主流程
        let _ = self.event_bus.publish(event).await;

        debug!(
            quest_id = %context.quest_id,
            n,
            avg_confidence,
            latency_ms,
            prediction_count = count,
            "MTPE 预测完成"
        );

        Ok(PredictionResult {
            predicted_tokens,
            latency_ms,
            n,
        })
    }

    /// 记录预测验证结果 — 由 PVL 验证层调用
    ///
    /// # 参数
    /// - `n`:被验证预测的步数
    /// - `success`:PVL 验证结果(true=成功)
    ///
    /// # 事件
    /// 每 100 次预测发布 `PredictionStatsReported` 事件(携带 success_rate_by_n)
    pub async fn record_verification(&self, n: usize, success: bool) {
        // 更新统计(写锁)
        {
            let mut stats = self.stats.write().await;
            stats.record(n, success);
        }

        // 每 100 次预测发布统计事件
        let count = self.prediction_count.load(Ordering::Relaxed);
        if count > 0 && count.is_multiple_of(100) {
            let rate_map = {
                let stats = self.stats.read().await;
                stats.to_rate_map()
            };

            let event = NexusEvent::PredictionStatsReported {
                metadata: EventMetadata::new("mtpe-executor"),
                success_rate_by_n: rate_map,
            };
            // WHY 忽略错误:统计事件为 Normal 级,丢失不影响主流程
            let _ = self.event_bus.publish(event).await;

            debug!(prediction_count = count, "MTPE 统计事件已发布");
        }
    }

    /// 获取指定 N 值的成功率
    ///
    /// 返回 0.0 表示无记录,调用方可据此判断是否需要降级 N
    pub async fn get_success_rate(&self, n: usize) -> f32 {
        let stats = self.stats.read().await;
        stats.success_rate(n)
    }

    /// 获取统计快照(克隆当前统计)
    pub async fn stats_snapshot(&self) -> PredictionStats {
        let stats = self.stats.read().await;
        stats.clone()
    }

    /// 获取当前预测计数
    pub fn prediction_count(&self) -> u64 {
        self.prediction_count.load(Ordering::Relaxed)
    }
}

/// 计算上下文哈希 — 伪预测的确定性种子
///
/// WHY 使用 FNV-1a 变体:简单快速,无需密码学强度,
/// 仅需对相同上下文产生相同哈希(确定性)
fn compute_context_hash(context: &PredictionContext) -> u32 {
    let mut hash: u32 = CONTEXT_HASH_SEED;

    // 混入 quest_id
    for byte in context.quest_id.as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }

    // 混入 history 最后一个元素(最近上下文权重最高)
    if let Some(last) = context.history.last() {
        for byte in last.as_bytes() {
            hash ^= *byte as u32;
            hash = hash.wrapping_mul(0x0100_0193);
        }
    }

    // 混入 clv 前 8 维(降维哈希,避免全量计算开销)
    for (i, &v) in context.clv.iter().take(8).enumerate() {
        let bits = v.to_bits();
        hash ^= bits.wrapping_add(i as u32);
        hash = hash.wrapping_mul(0x0100_0193);
    }

    hash
}

/// 生成伪预测 token 列表
///
/// # 伪预测逻辑
/// - Token.text = format!("pred_{}_{}", i, hash)
/// - Token.confidence = 1.0 - (i * 0.05),步数越高置信度越低
///
/// WHY 置信度递减:多步预测存在误差累积,后续 token 置信度自然降低,
/// 此模型与真实 LLM 预测的行为特征一致
// TODO(Week 7): 伪预测实现,替换为真实模型多步预测。
fn generate_pseudo_predictions(n: usize, context_hash: u32) -> Vec<Token> {
    (0..n)
        .map(|i| {
            let confidence = (1.0 - (i as f32 * 0.05)).max(0.0);
            Token {
                text: format!("pred_{}_{}", i, context_hash),
                confidence,
            }
        })
        .collect()
}

/// 计算平均置信度
fn compute_avg_confidence(tokens: &[Token]) -> f32 {
    if tokens.is_empty() {
        return 0.0;
    }
    let sum: f32 = tokens.iter().map(|t| t.confidence).sum();
    sum / tokens.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_context(quest_id: &str, history: Vec<&str>) -> PredictionContext {
        PredictionContext {
            quest_id: quest_id.into(),
            history: history.into_iter().map(String::from).collect(),
            clv: vec![0.1; 8],
        }
    }

    #[tokio::test]
    async fn test_predict_n1() {
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
        let ctx = make_context("q-1", vec!["hello"]);

        let result = executor.predict(&ctx, 1).await.unwrap();
        assert_eq!(result.n, 1);
        assert_eq!(result.predicted_tokens.len(), 1);
        assert!((result.predicted_tokens[0].confidence - 1.0).abs() < f32::EPSILON);
        assert!(result.latency_ms >= 0.0);
    }

    #[tokio::test]
    async fn test_predict_n5() {
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
        let ctx = make_context("q-1", vec!["hello"]);

        let result = executor.predict(&ctx, 5).await.unwrap();
        assert_eq!(result.n, 5);
        assert_eq!(result.predicted_tokens.len(), 5);

        // 验证置信度递减
        for (i, token) in result.predicted_tokens.iter().enumerate() {
            let expected = 1.0 - (i as f32 * 0.05);
            assert!(
                (token.confidence - expected).abs() < f32::EPSILON,
                "token {} confidence mismatch: got {}, expected {}",
                i,
                token.confidence,
                expected
            );
        }
    }

    #[tokio::test]
    async fn test_predict_n10() {
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
        let ctx = make_context("q-1", vec!["hello"]);

        let result = executor.predict(&ctx, 10).await.unwrap();
        assert_eq!(result.n, 10);
        assert_eq!(result.predicted_tokens.len(), 10);
        // 第 10 个 token(索引 9)置信度 = 1.0 - 9*0.05 = 0.55
        let last = &result.predicted_tokens[9];
        assert!((last.confidence - 0.55).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_predict_n0_invalid() {
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
        let ctx = make_context("q-1", vec!["hello"]);

        let result = executor.predict(&ctx, 0).await;
        assert!(matches!(result, Err(MtpeError::InvalidN { n: 0, max: 10 })));
    }

    #[tokio::test]
    async fn test_predict_n11_invalid() {
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
        let ctx = make_context("q-1", vec!["hello"]);

        let result = executor.predict(&ctx, 11).await;
        assert!(matches!(
            result,
            Err(MtpeError::InvalidN { n: 11, max: 10 })
        ));
    }

    #[tokio::test]
    async fn test_predict_deterministic() {
        // 相同上下文应产生相同预测(伪预测确定性)
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
        let ctx = make_context("q-1", vec!["hello"]);

        let r1 = executor.predict(&ctx, 5).await.unwrap();
        let r2 = executor.predict(&ctx, 5).await.unwrap();
        assert_eq!(r1.predicted_tokens, r2.predicted_tokens);
    }

    #[tokio::test]
    async fn test_predict_publishes_event() {
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
        let mut rx = executor.event_bus().subscribe();
        let ctx = make_context("q-evt", vec!["test"]);

        executor.predict(&ctx, 3).await.unwrap();

        let event = rx.recv().await.unwrap();
        match event {
            NexusEvent::PredictionMade { quest_id, n, .. } => {
                assert_eq!(quest_id, "q-evt");
                assert_eq!(n, 3);
            }
            other => panic!("expected PredictionMade, got {:?}", other),
        }
    }

    // ============================================================
    // SubTask 26.3: 成功率统计测试
    // ============================================================

    #[tokio::test]
    async fn test_record_verification_updates_stats() {
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());

        executor.record_verification(5, true).await;
        executor.record_verification(5, true).await;
        executor.record_verification(5, false).await;

        let rate = executor.get_success_rate(5).await;
        assert!((rate - (2.0 / 3.0)).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_record_verification_grouped() {
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());

        // N=1: 4 次成功
        for _ in 0..4 {
            executor.record_verification(1, true).await;
        }
        // N=5: 3 次成功,1 次失败
        for _ in 0..3 {
            executor.record_verification(5, true).await;
        }
        executor.record_verification(5, false).await;

        assert!((executor.get_success_rate(1).await - 1.0).abs() < f32::EPSILON);
        assert!((executor.get_success_rate(5).await - 0.75).abs() < f32::EPSILON);
        assert!((executor.get_success_rate(10).await - 0.0).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_stats_event_every_100_predictions() {
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
        let mut rx = executor.event_bus().subscribe();
        let ctx = make_context("q-stats", vec!["test"]);

        // 执行 100 次预测 + 验证,触发统计事件
        for _ in 0..100 {
            executor.predict(&ctx, 1).await.unwrap();
            executor.record_verification(1, true).await;
        }

        // 收集事件,应包含 PredictionStatsReported
        let mut found_stats = false;
        // 先消费所有 PredictionMade 事件,再找 PredictionStatsReported
        // 使用 try_recv 非阻塞消费
        loop {
            match rx.try_recv() {
                Ok(Some(NexusEvent::PredictionStatsReported { .. })) => {
                    found_stats = true;
                    break;
                }
                Ok(Some(_)) => continue, // 其他事件继续
                Ok(None) => break,       // 无更多事件
                Err(_) => break,
            }
        }
        assert!(found_stats, "应发布 PredictionStatsReported 事件");
    }

    #[test]
    fn test_compute_context_hash_deterministic() {
        let ctx1 = make_context("q-1", vec!["hello"]);
        let ctx2 = make_context("q-1", vec!["hello"]);
        assert_eq!(compute_context_hash(&ctx1), compute_context_hash(&ctx2));
    }

    #[test]
    fn test_compute_context_hash_differs() {
        let ctx1 = make_context("q-1", vec!["hello"]);
        let ctx2 = make_context("q-2", vec!["hello"]);
        assert_ne!(compute_context_hash(&ctx1), compute_context_hash(&ctx2));
    }

    #[test]
    fn test_generate_pseudo_predictions_confidence() {
        let tokens = generate_pseudo_predictions(5, 12345);
        assert_eq!(tokens.len(), 5);
        // 置信度应递减:1.0, 0.95, 0.9, 0.85, 0.8
        assert!((tokens[0].confidence - 1.0).abs() < f32::EPSILON);
        assert!((tokens[4].confidence - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn test_compute_avg_confidence_empty() {
        let tokens: Vec<Token> = vec![];
        assert!((compute_avg_confidence(&tokens) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_compute_avg_confidence() {
        let tokens = vec![
            Token {
                text: "a".into(),
                confidence: 1.0,
            },
            Token {
                text: "b".into(),
                confidence: 0.5,
            },
        ];
        assert!((compute_avg_confidence(&tokens) - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn test_to_rate_map_via_stats() {
        let mut stats = PredictionStats::new();
        stats.record(1, true);
        stats.record(1, true);
        stats.record(5, false);

        let map: HashMap<usize, f32> = stats.to_rate_map();
        assert_eq!(map.get(&1), Some(&1.0));
        assert_eq!(map.get(&5), Some(&0.0));
    }
}
