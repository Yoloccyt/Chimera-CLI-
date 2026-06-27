//! 多模态融合引擎与 NMC 编码器 — 将多模态认知元素融合为统一 CLV
//!
//! 对应架构层:L2 Memory
//! 对应创新点:NMC(Native Multimodal Context,原生多模态上下文编码)
//!
//! # 核心组件
//! - `MultimodalFusionEngine`:将多个 CognitiveElement 的 embedding 融合为 512 维 CLV
//! - `NmcEncoder`:编排感知 → 融合 → 事件发布的完整编码流水线
//!
//! # 融合策略(WHY)
//! - **Concat**:拼接所有 embedding 后截断/填充。适合单模态(直接保留中间表示),
//!   多模态时各模态信息分布在不同维度区间,互不干扰
//! - **Mean**:对齐到最大维度后取平均。适合同构多模态(各模态 embedding 维度相同),
//!   简单但会稀释各模态的区分度
//! - **Weighted**:按模态权重加权求和(归一化)。Text 权重最高(0.5)因文本语义最丰富,
//!   Desktop 权重 0.1 因描述文本信息密度较低。归一化确保输出幅度与元素数量无关

use event_bus::{EventBus, EventMetadata, NexusEvent};

use crate::config::{FusionStrategy, NmcConfig};
use crate::error::NmcError;
use crate::perceptors::{
    AudioPerceptor, DesktopPerceptor, ImagePerceptor, Perceptor, TextPerceptor, VideoPerceptor,
};
use crate::types::{ClvOutput, CognitiveElement, Modality, PerceptionInput};

/// 多模态融合引擎 — 将多个认知元素融合为 512 维 CLV 输出
pub struct MultimodalFusionEngine {
    /// 配置(含 clv_dim 与 fusion_strategy)
    config: NmcConfig,
}

impl MultimodalFusionEngine {
    /// 创建融合引擎
    pub fn new(config: NmcConfig) -> Self {
        Self { config }
    }

    /// 融合多个认知元素为 CLV 输出
    ///
    /// - 若 elements 为空,返回零向量 CLV
    /// - 否则按 config.fusion_strategy 执行融合,输出维度严格为 clv_dim(512)
    pub fn fuse(&self, elements: Vec<CognitiveElement>) -> Result<ClvOutput, NmcError> {
        if elements.is_empty() {
            return Ok(ClvOutput::zero());
        }

        let clv_dim = self.config.clv_dim;
        let merged = match self.config.fusion_strategy {
            FusionStrategy::Concat => fuse_concat(&elements, clv_dim),
            FusionStrategy::Mean => fuse_mean(&elements, clv_dim),
            FusionStrategy::Weighted => fuse_weighted(&elements, clv_dim),
        };

        // 维度校验(防御性:融合逻辑应始终产出 clv_dim 维)
        if merged.len() != clv_dim {
            return Err(NmcError::DimensionMismatch {
                expected: clv_dim,
                actual: merged.len(),
            });
        }

        ClvOutput::from_vec(merged)
    }
}

/// 将向量截断或填充到目标维度
fn resize_to_dim(mut vec: Vec<f32>, target: usize) -> Vec<f32> {
    if vec.len() > target {
        vec.truncate(target);
    } else {
        vec.resize(target, 0.0);
    }
    vec
}

/// Concat 策略:顺序拼接 embedding,截断/填充到 clv_dim
fn fuse_concat(elements: &[CognitiveElement], clv_dim: usize) -> Vec<f32> {
    let mut merged: Vec<f32> = Vec::with_capacity(clv_dim);
    for elem in elements {
        if merged.len() >= clv_dim {
            break;
        }
        merged.extend_from_slice(&elem.embedding);
    }
    resize_to_dim(merged, clv_dim)
}

/// Mean 策略:对齐到最大维度后取平均,截断/填充到 clv_dim
fn fuse_mean(elements: &[CognitiveElement], clv_dim: usize) -> Vec<f32> {
    let max_dim = elements
        .iter()
        .map(|e| e.embedding.len())
        .max()
        .unwrap_or(0);
    if max_dim == 0 {
        return vec![0.0; clv_dim];
    }

    let mut sums = vec![0.0_f32; max_dim];
    for elem in elements {
        for (i, &v) in elem.embedding.iter().enumerate() {
            sums[i] += v;
        }
    }

    let count = elements.len() as f32;
    let avg: Vec<f32> = sums.iter().map(|&s| s / count).collect();
    resize_to_dim(avg, clv_dim)
}

/// Weighted 策略:按模态权重加权求和(归一化),截断/填充到 clv_dim
fn fuse_weighted(elements: &[CognitiveElement], clv_dim: usize) -> Vec<f32> {
    let max_dim = elements
        .iter()
        .map(|e| e.embedding.len())
        .max()
        .unwrap_or(0);
    if max_dim == 0 {
        return vec![0.0; clv_dim];
    }

    let mut sums = vec![0.0_f32; max_dim];
    let mut total_weight = 0.0_f32;
    for elem in elements {
        let weight = modality_weight(elem.modality);
        total_weight += weight;
        for (i, &v) in elem.embedding.iter().enumerate() {
            sums[i] += v * weight;
        }
    }

    // WHY 归一化:确保输出幅度与元素数量无关(单模态 Text 权重 0.5/0.5=1.0)
    if total_weight == 0.0 {
        return vec![0.0; clv_dim];
    }
    let weighted: Vec<f32> = sums.iter().map(|&s| s / total_weight).collect();
    resize_to_dim(weighted, clv_dim)
}

/// 模态权重 — Text:0.5 / Image:0.2 / Video:0.1 / Audio:0.1 / Desktop:0.1
///
/// WHY:文本语义最丰富(权重最高),图像次之,视频/音频/桌面描述信息密度较低
fn modality_weight(modality: Modality) -> f32 {
    match modality {
        Modality::Text => 0.5,
        Modality::Image => 0.2,
        Modality::Video => 0.1,
        Modality::Audio => 0.1,
        Modality::Desktop => 0.1,
    }
}

/// NMC 编码器 — 编排感知 → 融合 → 事件发布的完整流水线
///
/// 持有 5 种感知器与融合引擎,可选携带 EventBus 用于发布 NmcEncoded 事件。
/// `perceive` 是同步方法(感知器为 CPU 密集型,事件发布用 publish_blocking)
pub struct NmcEncoder {
    /// 文本感知器
    text_perceptor: TextPerceptor,
    /// 图像感知器(占位)
    image_perceptor: ImagePerceptor,
    /// 视频感知器(占位)
    video_perceptor: VideoPerceptor,
    /// 音频感知器(占位)
    audio_perceptor: AudioPerceptor,
    /// 桌面感知器
    desktop_perceptor: DesktopPerceptor,
    /// 多模态融合引擎
    fusion: MultimodalFusionEngine,
    /// 可选事件总线(编码成功后发布 NmcEncoded 事件)
    event_bus: Option<EventBus>,
}

impl NmcEncoder {
    /// 创建编码器(无事件总线)
    pub fn new(config: NmcConfig) -> Result<Self, NmcError> {
        config.validate()?;
        Ok(Self {
            text_perceptor: TextPerceptor::new(config.clone()),
            image_perceptor: ImagePerceptor::new(),
            video_perceptor: VideoPerceptor::new(),
            audio_perceptor: AudioPerceptor::new(),
            desktop_perceptor: DesktopPerceptor::new(config.clone()),
            fusion: MultimodalFusionEngine::new(config),
            event_bus: None,
        })
    }

    /// 创建带事件总线的编码器
    pub fn with_event_bus(config: NmcConfig, bus: EventBus) -> Result<Self, NmcError> {
        config.validate()?;
        Ok(Self {
            text_perceptor: TextPerceptor::new(config.clone()),
            image_perceptor: ImagePerceptor::new(),
            video_perceptor: VideoPerceptor::new(),
            audio_perceptor: AudioPerceptor::new(),
            desktop_perceptor: DesktopPerceptor::new(config.clone()),
            fusion: MultimodalFusionEngine::new(config),
            event_bus: Some(bus),
        })
    }

    /// 感知输入并编码为 CLV 输出
    ///
    /// 流程:选择感知器 → 感知 → 融合 → 发布事件(若有总线)
    pub fn perceive(&self, input: PerceptionInput) -> Result<ClvOutput, NmcError> {
        let modality = input.modality();
        let element = self.perceive_by_modality(&input)?;
        let content_hash = element.content_hash.clone();
        let clv_output = self.fusion.fuse(vec![element])?;

        self.publish_encoded_event(modality, content_hash, clv_output.dimension());

        Ok(clv_output)
    }

    /// 根据输入模态选择对应感知器执行感知
    fn perceive_by_modality(&self, input: &PerceptionInput) -> Result<CognitiveElement, NmcError> {
        match input {
            PerceptionInput::Text(_) => self.text_perceptor.perceive(input),
            PerceptionInput::Image(_) => self.image_perceptor.perceive(input),
            PerceptionInput::Video(_) => self.video_perceptor.perceive(input),
            PerceptionInput::Audio(_) => self.audio_perceptor.perceive(input),
            PerceptionInput::Desktop(_) => self.desktop_perceptor.perceive(input),
        }
    }

    /// 发布 NmcEncoded 事件(若已配置事件总线)
    ///
    /// WHY 事件发布失败仅记录 warn 不阻断编码:CLV 已成功计算,
    /// 事件丢失可由下一次编码补偿(NmcEncoded 为 Normal 级别)
    fn publish_encoded_event(&self, modality: Modality, content_hash: String, clv_dim: usize) {
        let Some(bus) = &self.event_bus else {
            return;
        };
        let event = NexusEvent::NmcEncoded {
            metadata: EventMetadata::new("nmc-encoder"),
            modality: modality.as_str().into(),
            content_hash,
            clv_dimension: clv_dim,
        };
        if let Err(e) = bus.publish_blocking(event) {
            tracing::warn!(error = %e, "NmcEncoded 事件发布失败");
        }
    }

    /// 返回事件总线引用(若有)
    pub fn event_bus(&self) -> Option<&EventBus> {
        self.event_bus.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DesktopCapture;
    use nexus_core::CLV;

    fn make_element(modality: Modality, embedding: Vec<f32>) -> CognitiveElement {
        CognitiveElement::new(modality, "test_hash".into(), embedding)
    }

    // ── 融合引擎测试 ──

    #[test]
    fn test_fusion_empty_returns_zero() {
        let engine = MultimodalFusionEngine::new(NmcConfig::default());
        let output = engine.fuse(vec![]).unwrap();
        assert_eq!(output.dimension(), CLV::DIMENSION);
        assert!(output.as_slice().iter().all(|&v| v == 0.0));
    }

    #[test]
    fn test_fusion_single_element_concat() {
        let config = NmcConfig::default().with_fusion_strategy(FusionStrategy::Concat);
        let engine = MultimodalFusionEngine::new(config);
        let elem = make_element(Modality::Text, vec![0.5; 256]);
        let output = engine.fuse(vec![elem]).unwrap();
        assert_eq!(output.dimension(), 512);
        // Concat:前 256 维为 0.5,后 256 维填充为 0.0
        assert!(output.as_slice()[..256].iter().all(|&v| v == 0.5));
        assert!(output.as_slice()[256..].iter().all(|&v| v == 0.0));
    }

    #[test]
    fn test_fusion_single_element_mean() {
        let config = NmcConfig::default().with_fusion_strategy(FusionStrategy::Mean);
        let engine = MultimodalFusionEngine::new(config);
        let elem = make_element(Modality::Text, vec![0.8; 256]);
        let output = engine.fuse(vec![elem]).unwrap();
        assert_eq!(output.dimension(), 512);
        // Mean:单元素直接取值,前 256 维为 0.8,后 256 维填充 0.0
        assert!(output.as_slice()[..256].iter().all(|&v| v == 0.8));
    }

    #[test]
    fn test_fusion_single_element_weighted() {
        let config = NmcConfig::default().with_fusion_strategy(FusionStrategy::Weighted);
        let engine = MultimodalFusionEngine::new(config);
        let elem = make_element(Modality::Text, vec![1.0; 256]);
        let output = engine.fuse(vec![elem]).unwrap();
        assert_eq!(output.dimension(), 512);
        // Weighted 归一化:单 Text 元素权重 0.5/0.5=1.0,保留原始值
        assert!(output.as_slice()[..256]
            .iter()
            .all(|&v| (v - 1.0).abs() < 1e-6));
    }

    #[test]
    fn test_fusion_output_dimension_always_512() {
        let strategies = [
            FusionStrategy::Concat,
            FusionStrategy::Mean,
            FusionStrategy::Weighted,
        ];
        for strategy in strategies {
            let config = NmcConfig::default().with_fusion_strategy(strategy);
            let engine = MultimodalFusionEngine::new(config);
            let elem = make_element(Modality::Text, vec![0.3; 100]);
            let output = engine.fuse(vec![elem]).unwrap();
            assert_eq!(
                output.dimension(),
                512,
                "策略 {strategy:?} 输出维度应为 512"
            );
        }
    }

    #[test]
    fn test_fusion_concat_multiple_elements() {
        let config = NmcConfig::default().with_fusion_strategy(FusionStrategy::Concat);
        let engine = MultimodalFusionEngine::new(config);
        let elem1 = make_element(Modality::Text, vec![0.1; 256]);
        let elem2 = make_element(Modality::Desktop, vec![0.2; 256]);
        let output = engine.fuse(vec![elem1, elem2]).unwrap();
        assert_eq!(output.dimension(), 512);
        // Concat:前 256 维为 0.1,后 256 维为 0.2
        assert!(output.as_slice()[..256]
            .iter()
            .all(|&v| (v - 0.1).abs() < 1e-6));
        assert!(output.as_slice()[256..]
            .iter()
            .all(|&v| (v - 0.2).abs() < 1e-6));
    }

    #[test]
    fn test_fusion_mean_averages_correctly() {
        let config = NmcConfig::default().with_fusion_strategy(FusionStrategy::Mean);
        let engine = MultimodalFusionEngine::new(config);
        let elem1 = make_element(Modality::Text, vec![0.4; 256]);
        let elem2 = make_element(Modality::Desktop, vec![0.6; 256]);
        let output = engine.fuse(vec![elem1, elem2]).unwrap();
        // Mean:平均 (0.4 + 0.6) / 2 = 0.5
        assert!(output.as_slice()[..256]
            .iter()
            .all(|&v| (v - 0.5).abs() < 1e-6));
    }

    #[test]
    fn test_fusion_weighted_applies_modality_weights() {
        let config = NmcConfig::default().with_fusion_strategy(FusionStrategy::Weighted);
        let engine = MultimodalFusionEngine::new(config);
        // Text(0.5) + Desktop(0.1) = 0.6 total
        // (0.4 * 0.5 + 0.6 * 0.1) / 0.6 = (0.2 + 0.06) / 0.6 = 0.4333...
        let elem1 = make_element(Modality::Text, vec![0.4; 256]);
        let elem2 = make_element(Modality::Desktop, vec![0.6; 256]);
        let output = engine.fuse(vec![elem1, elem2]).unwrap();
        let expected = (0.4 * 0.5 + 0.6 * 0.1) / 0.6;
        assert!(
            output.as_slice()[..256]
                .iter()
                .all(|&v| (v - expected).abs() < 1e-5),
            "期望 {expected},实际 {:?}",
            &output.as_slice()[..10]
        );
    }

    #[test]
    fn test_fusion_weighted_text_higher_weight_than_desktop() {
        let config = NmcConfig::default().with_fusion_strategy(FusionStrategy::Weighted);
        let engine = MultimodalFusionEngine::new(config);
        // 相同 embedding,Text 权重应使结果更接近 Text 值
        let elem1 = make_element(Modality::Text, vec![1.0; 256]);
        let elem2 = make_element(Modality::Desktop, vec![0.0; 256]);
        let output = engine.fuse(vec![elem1, elem2]).unwrap();
        // (1.0*0.5 + 0.0*0.1) / 0.6 = 0.5/0.6 ≈ 0.833
        let expected = 0.5_f32 / 0.6;
        assert!((output.as_slice()[0] - expected).abs() < 1e-5);
    }

    // ── NmcEncoder 测试 ──

    #[test]
    fn test_nmc_encoder_perceive_text() {
        let encoder = NmcEncoder::new(NmcConfig::default()).unwrap();
        let output = encoder
            .perceive(PerceptionInput::Text("hello world".into()))
            .unwrap();
        assert_eq!(output.dimension(), 512);
    }

    #[test]
    fn test_nmc_encoder_perceive_desktop() {
        let encoder = NmcEncoder::new(NmcConfig::default()).unwrap();
        let input = PerceptionInput::Desktop(DesktopCapture::new(1920, 1080, "editor"));
        let output = encoder.perceive(input).unwrap();
        assert_eq!(output.dimension(), 512);
    }

    #[test]
    fn test_nmc_encoder_perceive_image_error() {
        let encoder = NmcEncoder::new(NmcConfig::default()).unwrap();
        let result = encoder.perceive(PerceptionInput::Image(vec![0; 1024]));
        assert!(matches!(result, Err(NmcError::EncodingFailed { .. })));
    }

    #[test]
    fn test_nmc_encoder_perceive_video_error() {
        let encoder = NmcEncoder::new(NmcConfig::default()).unwrap();
        let result = encoder.perceive(PerceptionInput::Video(vec![0; 1024]));
        assert!(matches!(result, Err(NmcError::EncodingFailed { .. })));
    }

    #[test]
    fn test_nmc_encoder_perceive_audio_error() {
        let encoder = NmcEncoder::new(NmcConfig::default()).unwrap();
        let result = encoder.perceive(PerceptionInput::Audio(vec![0; 512]));
        assert!(matches!(result, Err(NmcError::EncodingFailed { .. })));
    }

    #[test]
    fn test_nmc_encoder_without_event_bus() {
        let encoder = NmcEncoder::new(NmcConfig::default()).unwrap();
        assert!(encoder.event_bus().is_none());
        // 无总线时 perceive 仍正常工作
        let output = encoder
            .perceive(PerceptionInput::Text("test".into()))
            .unwrap();
        assert_eq!(output.dimension(), 512);
    }

    #[test]
    fn test_nmc_encoder_with_event_bus() {
        let bus = EventBus::new();
        let encoder = NmcEncoder::with_event_bus(NmcConfig::default(), bus).unwrap();
        assert!(encoder.event_bus().is_some());
        let output = encoder
            .perceive(PerceptionInput::Text("test".into()))
            .unwrap();
        assert_eq!(output.dimension(), 512);
    }

    #[test]
    fn test_nmc_encoder_config_validation_fails() {
        let bad_config = NmcConfig::default().with_clv_dim(256);
        let result = NmcEncoder::new(bad_config);
        assert!(matches!(result, Err(NmcError::ConfigError { .. })));
    }

    #[test]
    fn test_nmc_encoder_deterministic_text_encoding() {
        let encoder = NmcEncoder::new(NmcConfig::default()).unwrap();
        let input = PerceptionInput::Text("deterministic test".into());
        let output1 = encoder.perceive(input.clone()).unwrap();
        let output2 = encoder.perceive(input).unwrap();
        assert_eq!(output1, output2);
    }
}
