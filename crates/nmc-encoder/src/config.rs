//! NMC 配置 — 感知器中间维度、融合策略、ONNX 模型配置
//!
//! 对应架构层:L2 Memory
//!
//! # 设计决策(WHY)
//! - **text_dim 默认 256**:文本感知器的中间嵌入维度。256 维字符频率统计
//!   足以区分不同文本(每个 UTF-8 字节值对应一个桶),同时保持低计算成本
//! - **clv_dim 必须等于 CLV::DIMENSION(512)**:融合输出必须与 nexus_core::CLV
//!   维度对齐,否则 CLV::from_vec 会拒绝构造。validate() 在此显式校验,
//!   提前暴露配置错误而非等到运行时 CLV 构造失败
//! - **FusionStrategy 三选一**:Concat(拼接)/Mean(平均)/Weighted(加权),
//!   不同场景适用不同策略(如单模态用 Concat,多模态对齐用 Weighted)
//! - **ONNX 模型字段默认值为空/预置名**:model_dir 为空时表示未启用 ONNX 推理,
//!   ImagePerceptor/VideoPerceptor/AudioPerceptor 将回退到占位实现。
//!   当 model_dir 非空时,validate() 会校验三个模型文件是否存在,
//!   提前暴露部署问题而非等到运行时加载失败。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::NmcError;

/// 融合策略 — 多个 CognitiveElement 融合为 CLV 的方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FusionStrategy {
    /// 拼接策略:将所有 embedding 顺序拼接,截断/填充到 clv_dim
    Concat,
    /// 平均策略:对齐到最大维度后取平均,截断/填充到 clv_dim
    Mean,
    /// 加权策略:按模态权重加权求和(Text:0.5/Image:0.2/Video:0.1/Audio:0.1/Desktop:0.1)
    Weighted,
}

impl FusionStrategy {
    /// 返回策略名称字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Concat => "Concat",
            Self::Mean => "Mean",
            Self::Weighted => "Weighted",
        }
    }
}

/// NMC 编码器配置 — 感知器维度、融合策略、ONNX 模型路径
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NmcConfig {
    /// 文本感知器中间维度(默认 256,基于字符频率统计的桶数)
    pub text_dim: usize,
    /// 最终 CLV 维度(默认 512,必须等于 CLV::DIMENSION)
    pub clv_dim: usize,
    /// 融合策略(默认 Weighted)
    pub fusion_strategy: FusionStrategy,
    /// ONNX 模型文件所在目录(默认空字符串,表示未启用 ONNX 推理)
    #[serde(default)]
    pub model_dir: String,
    /// 图像编码 ONNX 模型文件名(默认 "clip-vit-b32.onnx")
    #[serde(default = "default_image_model")]
    pub image_model: String,
    /// 视频编码 ONNX 模型文件名(默认 "videomae-base.onnx")
    #[serde(default = "default_video_model")]
    pub video_model: String,
    /// 音频编码 ONNX 模型文件名(默认 "whisper-encoder.onnx")
    #[serde(default = "default_audio_model")]
    pub audio_model: String,
}

// === 默认值函数,供 serde(default = "...") 使用 ===

fn default_image_model() -> String {
    "clip-vit-b32.onnx".to_string()
}

fn default_video_model() -> String {
    "videomae-base.onnx".to_string()
}

fn default_audio_model() -> String {
    "whisper-encoder.onnx".to_string()
}

impl NmcConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置文本感知器中间维度
    pub fn with_text_dim(mut self, dim: usize) -> Self {
        self.text_dim = dim;
        self
    }

    /// 设置 CLV 维度(必须为 512)
    pub fn with_clv_dim(mut self, dim: usize) -> Self {
        self.clv_dim = dim;
        self
    }

    /// 设置融合策略
    pub fn with_fusion_strategy(mut self, strategy: FusionStrategy) -> Self {
        self.fusion_strategy = strategy;
        self
    }

    /// 设置 ONNX 模型目录
    pub fn with_model_dir(mut self, dir: impl Into<String>) -> Self {
        self.model_dir = dir.into();
        self
    }

    /// 设置图像编码模型文件名
    pub fn with_image_model(mut self, name: impl Into<String>) -> Self {
        self.image_model = name.into();
        self
    }

    /// 设置视频编码模型文件名
    pub fn with_video_model(mut self, name: impl Into<String>) -> Self {
        self.video_model = name.into();
        self
    }

    /// 设置音频编码模型文件名
    pub fn with_audio_model(mut self, name: impl Into<String>) -> Self {
        self.audio_model = name.into();
        self
    }

    /// 拼接模型目录与模型文件名,返回完整路径
    ///
    /// 当 `model_dir` 为空字符串时,仅返回模型文件名(视为当前目录)。
    /// 使用 `Path::join` 确保跨平台路径分隔符正确。
    pub fn model_path(&self, model_name: &str) -> String {
        if self.model_dir.is_empty() {
            model_name.to_string()
        } else {
            Path::new(&self.model_dir)
                .join(model_name)
                .to_string_lossy()
                .into_owned()
        }
    }

    /// 校验配置合法性
    ///
    /// 校验规则:
    /// - `text_dim` 必须 > 0(感知器需要至少 1 个桶)
    /// - `clv_dim` 必须等于 `CLV::DIMENSION`(512),否则 CLV::from_vec 会失败
    /// - 当 `model_dir` 非空时,校验三个 ONNX 模型文件是否存在,提前暴露部署问题
    pub fn validate(&self) -> Result<(), NmcError> {
        if self.text_dim == 0 {
            return Err(NmcError::ConfigError {
                reason: "text_dim 不能为 0".into(),
            });
        }
        if self.clv_dim != nexus_core::CLV::DIMENSION {
            return Err(NmcError::ConfigError {
                reason: format!(
                    "clv_dim 必须等于 CLV::DIMENSION({}),当前为 {}",
                    nexus_core::CLV::DIMENSION,
                    self.clv_dim
                ),
            });
        }
        // 仅当 model_dir 非空时校验 ONNX 模型文件,空目录表示未启用 ONNX 推理
        if !self.model_dir.is_empty() {
            for (model_name, label) in [
                (&self.image_model, "图像"),
                (&self.video_model, "视频"),
                (&self.audio_model, "音频"),
            ] {
                let full_path = self.model_path(model_name);
                if !Path::new(&full_path).exists() {
                    return Err(NmcError::ModelLoadError {
                        model_name: format!("{}({})", label, model_name),
                        reason: format!("文件不存在: {}", full_path),
                    });
                }
            }
        }
        Ok(())
    }
}

impl Default for NmcConfig {
    fn default() -> Self {
        Self {
            text_dim: 256,
            clv_dim: nexus_core::CLV::DIMENSION,
            fusion_strategy: FusionStrategy::Weighted,
            // model_dir 为空字符串,表示未启用 ONNX 推理
            model_dir: String::new(),
            image_model: default_image_model(),
            video_model: default_video_model(),
            audio_model: default_audio_model(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = NmcConfig::default();
        assert_eq!(config.text_dim, 256);
        assert_eq!(config.clv_dim, 512);
        assert_eq!(config.fusion_strategy, FusionStrategy::Weighted);
        // ONNX 字段默认值
        assert_eq!(config.model_dir, "");
        assert_eq!(config.image_model, "clip-vit-b32.onnx");
        assert_eq!(config.video_model, "videomae-base.onnx");
        assert_eq!(config.audio_model, "whisper-encoder.onnx");
    }

    #[test]
    fn test_builder_chain() {
        let config = NmcConfig::new()
            .with_text_dim(128)
            .with_clv_dim(512)
            .with_fusion_strategy(FusionStrategy::Concat);
        assert_eq!(config.text_dim, 128);
        assert_eq!(config.clv_dim, 512);
        assert_eq!(config.fusion_strategy, FusionStrategy::Concat);
    }

    #[test]
    fn test_builder_chain_onnx() {
        let config = NmcConfig::new()
            .with_model_dir("/models/onnx")
            .with_image_model("custom-clip.onnx")
            .with_video_model("custom-video.onnx")
            .with_audio_model("custom-audio.onnx");
        assert_eq!(config.model_dir, "/models/onnx");
        assert_eq!(config.image_model, "custom-clip.onnx");
        assert_eq!(config.video_model, "custom-video.onnx");
        assert_eq!(config.audio_model, "custom-audio.onnx");
    }

    #[test]
    fn test_model_path_empty_dir() {
        let config = NmcConfig::default();
        // model_dir 为空时,仅返回文件名
        assert_eq!(config.model_path("clip-vit-b32.onnx"), "clip-vit-b32.onnx");
    }

    #[test]
    fn test_model_path_with_dir() {
        let config = NmcConfig::new().with_model_dir("/opt/models");
        let path = config.model_path("clip-vit-b32.onnx");
        assert!(path.contains("clip-vit-b32.onnx"));
        assert!(path.contains("/opt/models"));
    }

    #[test]
    fn test_validate_valid() {
        let config = NmcConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_zero_text_dim() {
        let config = NmcConfig::new().with_text_dim(0);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, NmcError::ConfigError { .. }));
    }

    #[test]
    fn test_validate_invalid_clv_dim() {
        let config = NmcConfig::new().with_clv_dim(256);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, NmcError::ConfigError { .. }));
        assert!(err.to_string().contains("512"));
    }

    #[test]
    fn test_validate_missing_model_file() {
        // 设置一个不存在的目录,校验应返回 ModelLoadError
        let config = NmcConfig::new().with_model_dir("/nonexistent/onnx/models");
        let err = config.validate().unwrap_err();
        assert!(matches!(err, NmcError::ModelLoadError { .. }));
        assert!(err.to_string().contains("文件不存在"));
    }

    #[test]
    fn test_fusion_strategy_as_str() {
        assert_eq!(FusionStrategy::Concat.as_str(), "Concat");
        assert_eq!(FusionStrategy::Mean.as_str(), "Mean");
        assert_eq!(FusionStrategy::Weighted.as_str(), "Weighted");
    }
}
