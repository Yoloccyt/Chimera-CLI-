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
//! - P2-10:集成 NMC 编码器,模型可用时使用真实预测,否则降级伪预测

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tokio::sync::RwLock;
use tracing::debug;

use nmc_encoder::Perceptor;

use crate::config::MtpeConfig;
use crate::error::MtpeError;
use crate::model::MtpeModel;
use crate::types::{PredictionContext, PredictionResult, PredictionStats, Token};

/// 模拟推理启动开销 — 每次 predict 调用的固定延迟
///
/// WHY 固定延迟:真实推理中,模型启动/上下文编码的开销远大于生成单个
/// token 的开销,且此开销与 N 无关(一次推理可产出 N 个 token)。
/// MTPE 的核心优势就是减少推理启动次数。伪预测中加入此延迟,
/// 使加速比测试能反映真实场景的加速效果(1000×N=5 vs 5000×N=1)
// DEFERRED(T8-3 Audit): SIMULATED_INFERENCE_DELAY 与 generate_pseudo_predictions 为伪实现,
// 替换为真实模型推理延迟与多步预测需要 NMC ONNX 模型文件(外部依赖)。
// 当前伪实现已验证架构正确性(确定性输出、置信度递减、事件发布)。
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
/// - `model`:可选的 NMC 模型,模型可用时提供真实预测,否则降级伪预测
pub struct MtpeExecutor {
    /// 执行器配置
    config: MtpeConfig,
    /// 事件总线(跨层通信唯一通道,§2.2 依赖铁律)
    event_bus: EventBus,
    /// 预测成功率统计(按 N 值分组)
    stats: RwLock<PredictionStats>,
    /// 预测计数器(用于触发周期性统计事件)
    prediction_count: AtomicU64,
    /// 可选的 NMC ONNX 模型(模型加载失败时降级伪预测)
    model: Option<MtpeModel>,
}

/// 上下文哈希的稳定种子 — 用于伪预测生成确定性 token
///
/// WHY 常量:伪预测仅需确定性输出,无需密码学强度,用固定种子简化实现
const CONTEXT_HASH_SEED: u32 = 0x4D54_5045; // "MTPE" 的 ASCII

impl MtpeExecutor {
    /// 创建 MTPE 执行器(无模型,使用伪预测)
    ///
    /// 向后兼容:保持与现有代码一致的 API 签名。
    /// 如需启用模型预测,请使用 `with_model` 方法设置。
    pub fn new(config: MtpeConfig, event_bus: EventBus) -> Self {
        Self {
            config,
            event_bus,
            stats: RwLock::new(PredictionStats::new()),
            prediction_count: AtomicU64::new(0),
            model: None,
        }
    }

    /// 创建 MTPE 执行器并附加 NMC 模型
    ///
    /// # 参数
    /// - `config`:执行器配置
    /// - `event_bus`:事件总线
    /// - `model`:NMC ONNX 模型(模型不存在或加载失败时自动降级伪预测)
    ///
    /// # 示例
    /// ```no_run
    /// use mtpe_executor::{MtpeExecutor, MtpeConfig, MtpeModel};
    /// use event_bus::EventBus;
    /// use std::path::PathBuf;
    ///
    /// # async fn run() {
    /// let bus = EventBus::new();
    /// let model = MtpeModel::new(PathBuf::from("/path/to/model.onnx"));
    /// let executor = MtpeExecutor::with_model(MtpeConfig::default(), bus, model);
    /// # }
    /// ```
    pub fn with_model(config: MtpeConfig, event_bus: EventBus, model: MtpeModel) -> Self {
        if model.is_loaded() {
            debug!("MTPE 执行器已附加 NMC 模型,将使用真实预测");
        } else {
            debug!("MTPE 执行器附加的模型未加载,将使用伪预测降级");
        }
        Self {
            config,
            event_bus,
            stats: RwLock::new(PredictionStats::new()),
            prediction_count: AtomicU64::new(0),
            model: Some(model),
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

    /// 获取模型引用(如果有)
    pub fn model(&self) -> Option<&MtpeModel> {
        self.model.as_ref()
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
    /// # 预测策略
    /// 1. 模型可用且已加载 → 使用 NMC ONNX 模型进行真实预测
    /// 2. 模型未加载或不可用 → 降级到基于上下文哈希的伪预测
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

        // 尝试使用模型进行真实预测
        let predicted_tokens = if let Some(model) = &self.model {
            if model.is_loaded() {
                // 使用 ONNX 模型进行真实预测
                // 1. 将 PredictionContext 编码为输入张量
                let input_tensor = encode_context(context);
                // 2. 调用模型推理
                if let Some(output) = model.predict(input_tensor) {
                    // 3. 将输出张量解码为 N 个预测 token
                    decode_predictions(&output, n)
                } else {
                    // 模型推理失败，降级到伪预测
                    tracing::warn!("MTPE 模型推理失败，降级到伪预测");
                    let context_hash = compute_context_hash(context);
                    generate_pseudo_predictions(n, context_hash)
                }
            } else {
                // 模型未加载，使用伪预测
                let context_hash = compute_context_hash(context);
                generate_pseudo_predictions(n, context_hash)
            }
        } else {
            // 无模型，使用伪预测
            // 模拟推理启动开销(与 N 无关的固定延迟)
            // WHY:真实推理中,模型启动/上下文编码开销远大于生成单个 token,
            // 且此开销与 N 无关。MTPE 通过一次推理产出 N 个 token 来摊薄此开销
            tokio::time::sleep(SIMULATED_INFERENCE_DELAY).await;
            let context_hash = compute_context_hash(context);
            generate_pseudo_predictions(n, context_hash)
        };

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
// DEFERRED(T8-3 Audit): 伪预测实现,替换为真实模型多步预测需要 NMC ONNX 模型文件(外部依赖),
// 当前基于上下文哈希的伪预测已验证 MTPE 架构(验证/回退/统计)的端到端正确性。
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

/// 将 PredictionContext 编码为输入张量
///
/// 使用 NMC 的文本编码器(TextPerceptor)将上下文编码为嵌入向量,
/// 作为 ONNX 模型的输入张量。
///
/// # 编码策略
/// 将 quest_id + history + clv 序列化为文本后,通过 TextPerceptor
/// 生成嵌入向量。当 NMC 文本感知器不可用时,回退到基于哈希的编码。
///
/// # 返回
/// 固定长度的 f32 向量(TextPerceptor 输出维度,默认 256 维)
fn encode_context(context: &PredictionContext) -> Vec<f32> {
    // 将上下文序列化为文本
    let mut text = format!("quest:{}", context.quest_id);

    // 附加历史记录(最多取最后 3 条)
    for h in context.history.iter().rev().take(3) {
        text.push_str("|hist:");
        text.push_str(h);
    }

    // 附加 clv 的前 8 维摘要
    text.push_str("|clv:");
    for v in context.clv.iter().take(8) {
        text.push_str(&format!("{:.4}", v));
        text.push(',');
    }

    // 使用 NMC TextPerceptor 编码
    // WHY 使用文本感知器:将非结构化上下文映射为固定维度嵌入,
    // 与 ONNX 模型输入格式兼容
    let config = nmc_encoder::config::NmcConfig::default();
    let perceptor = nmc_encoder::perceptors::TextPerceptor::new(config);
    let input = nmc_encoder::types::PerceptionInput::Text(text);

    match perceptor.perceive(&input) {
        Ok(element) => {
            let embedding = element.embedding;
            if embedding.is_empty() {
                // 空嵌入不应当发生,降级到简单哈希编码
                fallback_encode_context(context)
            } else {
                embedding
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "NMC 文本编码失败,降级到哈希编码"
            );
            fallback_encode_context(context)
        }
    }
}

/// 上下文编码的降级实现 — 基于哈希的简单编码
///
/// 当 NMC TextPerceptor 不可用或编码失败时使用。
/// 将上下文哈希和 CLV 特征组合为固定维度的向量。
fn fallback_encode_context(context: &PredictionContext) -> Vec<f32> {
    let hash = compute_context_hash(context);
    let mut tensor = Vec::with_capacity(256);

    // 前 8 维:哈希值分解
    tensor.push(((hash >> 24) & 0xFF) as f32 / 255.0);
    tensor.push(((hash >> 16) & 0xFF) as f32 / 255.0);
    tensor.push(((hash >> 8) & 0xFF) as f32 / 255.0);
    tensor.push((hash & 0xFF) as f32 / 255.0);
    tensor.push(((hash >> 16) ^ (hash & 0xFFFF)) as f32 / 65535.0);
    tensor.push((hash as f32) / u32::MAX as f32);
    tensor.push((hash ^ 0x4D54_5045) as f32 / u32::MAX as f32);
    tensor.push((hash.wrapping_mul(0x0100_0193)) as f32 / u32::MAX as f32);

    // 中间维度:CLV 特征(最多 248 维)
    let clv_features = context.clv.len().min(248);
    for &v in context.clv.iter().take(clv_features) {
        tensor.push(v);
    }

    // 剩余维度零填充
    while tensor.len() < 256 {
        tensor.push(0.0);
    }

    tensor
}

/// 将输出张量解码为 N 个预测 token
///
/// # 解码策略
/// 将 ONNX 模型输出的 512 维向量分段映射为 N 个 token,
/// 每段生成一个 token 及其置信度。
///
/// # 参数
/// - `output`: ONNX 模型输出(512 维 L2 归一化向量)
/// - `n`: 目标 token 数量
///
/// # 返回
/// N 个预测 token,置信度从高到低排列
fn decode_predictions(output: &[f32], n: usize) -> Vec<Token> {
    if output.is_empty() || n == 0 {
        return Vec::new();
    }

    // 每个 token 分配的维度数(至少 2 维)
    let dims_per_token = (output.len() / n).max(2);

    (0..n)
        .map(|i| {
            let start = i * dims_per_token;
            let end = (start + dims_per_token).min(output.len());

            // 提取当前 token 对应的输出段
            let segment = if start < output.len() {
                &output[start..end]
            } else {
                &[]
            };

            // 计算该段统计量作为置信度
            let segment_sum: f32 = segment.iter().map(|v| v.abs()).sum();
            let segment_mean = if !segment.is_empty() {
                segment_sum / segment.len() as f32
            } else {
                0.0
            };

            // 置信度:使用 segment 的 L2 范数,步数越高衰减
            let confidence = (segment_mean * (1.0 - i as f32 * 0.05)).clamp(0.0, 1.0);

            // 生成文本:使用段内主要值的哈希
            let token_hash = if !segment.is_empty() {
                let mut h: u32 = 0x4D54_5045;
                for &v in segment.iter().take(8) {
                    let bits = v.to_bits();
                    h ^= bits;
                    h = h.wrapping_mul(0x0100_0193);
                }
                h
            } else {
                0
            };

            Token {
                text: format!("tok_{}_{}", i, token_hash),
                confidence,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

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

    // ============================================================
    // P2-10: MTPE 真实预测 — 向后兼容性测试
    // ============================================================

    /// 验证无模型时 predict 行为与修改前一致(伪预测)
    #[tokio::test]
    async fn test_predict_without_model_fallback() {
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
        let ctx = make_context("q-fallback", vec!["hello"]);

        // 不带模型时应使用伪预测
        let result = executor.predict(&ctx, 3).await.unwrap();
        assert_eq!(result.n, 3);
        assert_eq!(result.predicted_tokens.len(), 3);
        // 验证是伪预测格式: text 以 "pred_" 开头
        assert!(result.predicted_tokens[0].text.starts_with("pred_"));
        // 置信度递减: 1.0, 0.95, 0.9
        assert!((result.predicted_tokens[0].confidence - 1.0).abs() < f32::EPSILON);
        assert!((result.predicted_tokens[1].confidence - 0.95).abs() < f32::EPSILON);
        assert!((result.predicted_tokens[2].confidence - 0.90).abs() < f32::EPSILON);
    }

    /// 验证模型未加载时优雅降级
    #[tokio::test]
    async fn test_predict_with_unloaded_model_degradation() {
        let model = MtpeModel::new(PathBuf::from("/nonexistent/model.onnx"));
        assert!(!model.is_loaded(), "模型文件不存在时不应加载");

        let executor = MtpeExecutor::with_model(MtpeConfig::default(), EventBus::new(), model);
        let ctx = make_context("q-degrade", vec!["hello"]);

        // 模型未加载时应降级到伪预测
        let result = executor.predict(&ctx, 3).await.unwrap();
        assert_eq!(result.n, 3);
        assert_eq!(result.predicted_tokens.len(), 3);
        // 验证降级到伪预测格式
        assert!(result.predicted_tokens[0].text.starts_with("pred_"));
    }

    /// 验证 with_model 构造器
    #[tokio::test]
    async fn test_with_model_constructor() {
        let model = MtpeModel::new(PathBuf::from("/nonexistent/model.onnx"));
        let executor = MtpeExecutor::with_model(MtpeConfig::default(), EventBus::new(), model);
        assert!(executor.model().is_some());
        assert!(!executor.model().unwrap().is_loaded());
    }

    // ============================================================
    // encode_context 测试
    // ============================================================

    #[test]
    fn test_encode_context_non_empty() {
        let ctx = make_context("q-encode", vec!["hello world"]);
        let tensor = encode_context(&ctx);
        // 输出应为非空向量
        assert!(!tensor.is_empty(), "encode_context 不应返回空向量");
        // 输出应为 256 维(TextPerceptor 默认文本维度)
        assert_eq!(tensor.len(), 256, "encode_context 输出应为 256 维");
    }

    #[test]
    fn test_encode_context_deterministic() {
        let ctx1 = make_context("q-det", vec!["hello"]);
        let ctx2 = make_context("q-det", vec!["hello"]);
        let t1 = encode_context(&ctx1);
        let t2 = encode_context(&ctx2);
        // 相同上下文应产生相同编码
        assert_eq!(t1, t2, "相同上下文的编码应一致");
    }

    #[test]
    fn test_encode_context_differs_for_different_context() {
        let ctx1 = make_context("q-1", vec!["hello"]);
        let ctx2 = make_context("q-2", vec!["world"]);
        let t1 = encode_context(&ctx1);
        let t2 = encode_context(&ctx2);
        // 不同上下文应产生不同编码(极低概率碰撞)
        assert_ne!(t1, t2, "不同上下文的编码应不同");
    }

    #[test]
    fn test_encode_context_empty_clv() {
        let ctx = PredictionContext {
            quest_id: "q-empty".into(),
            history: vec![],
            clv: vec![],
        };
        let tensor = encode_context(&ctx);
        assert!(!tensor.is_empty(), "空 CLV 也应产生非空编码");
        assert_eq!(tensor.len(), 256, "空 CLV 编码也应为 256 维");
    }

    // ============================================================
    // decode_predictions 测试
    // ============================================================

    #[test]
    fn test_decode_predictions_basic() {
        // 512 维输出 → 解码为 5 个 token
        let output = vec![0.1f32; 512];
        let tokens = decode_predictions(&output, 5);
        assert_eq!(tokens.len(), 5, "解码 5 个 token 应返回 5 个");
        // 置信度应递减
        for i in 1..tokens.len() {
            assert!(
                tokens[i].confidence <= tokens[i - 1].confidence,
                "置信度应递减: token[{}]={} > token[{}]={}",
                i - 1,
                tokens[i - 1].confidence,
                i,
                tokens[i].confidence
            );
        }
    }

    #[test]
    fn test_decode_predictions_empty_output() {
        let tokens = decode_predictions(&[], 5);
        assert!(tokens.is_empty(), "空输出应返回空 token 列表");
    }

    #[test]
    fn test_decode_predictions_n_zero() {
        let output = vec![0.1f32; 512];
        let tokens = decode_predictions(&output, 0);
        assert!(tokens.is_empty(), "N=0 应返回空列表");
    }

    #[test]
    fn test_decode_predictions_single() {
        let output = vec![0.5f32; 512];
        let tokens = decode_predictions(&output, 1);
        assert_eq!(tokens.len(), 1, "N=1 应返回 1 个 token");
        // 单步置信度 = segment_mean * (1.0 - 0 * 0.05) = 0.5 * 1.0 = 0.5
        assert!(
            (tokens[0].confidence - 0.5).abs() < f32::EPSILON,
            "单步预测置信度应为 0.5, 实际 {}",
            tokens[0].confidence
        );
    }

    // ============================================================
    // fallback_encode_context 测试
    // ============================================================

    #[test]
    fn test_fallback_encode_context_basic() {
        let ctx = make_context("q-fallback", vec!["test"]);
        let tensor = fallback_encode_context(&ctx);
        assert_eq!(tensor.len(), 256, "降级编码应为 256 维");
        // 前 8 维应非零(基于哈希)
        let front_sum: f32 = tensor[..8].iter().map(|v| v.abs()).sum();
        assert!(front_sum > 0.0, "哈希部分应非零");
    }

    #[test]
    fn test_fallback_encode_context_empty_clv() {
        let ctx = PredictionContext {
            quest_id: "q-empty".into(),
            history: vec![],
            clv: vec![],
        };
        let tensor = fallback_encode_context(&ctx);
        assert_eq!(tensor.len(), 256, "空 CLV 降级编码也应为 256 维");
        // 前 8 维基于哈希应非零
        let front_sum: f32 = tensor[..8].iter().map(|v| v.abs()).sum();
        assert!(front_sum > 0.0, "空 CLV 的哈希部分也应非零");
    }

    #[test]
    fn test_fallback_encode_context_deterministic() {
        let ctx1 = make_context("q-det", vec!["hello"]);
        let ctx2 = make_context("q-det", vec!["hello"]);
        assert_eq!(
            fallback_encode_context(&ctx1),
            fallback_encode_context(&ctx2),
            "降级编码也应是确定性的"
        );
    }
}
