//! NMC 编码器错误类型 — 库层 thiserror enum
//!
//! 对应架构层:L2 Memory
//!
//! # 设计决策(WHY)
//! - 使用 `thiserror` 而非 `anyhow`:库层错误需明确变体,便于调用方按错误类型决策
//! - 7 个变体覆盖模态校验、编码失败、配置错误、维度不匹配、模型加载、推理、预处理的全部失败场景
//! - `EncodingFailed` 携带 modality 与 reason,便于定位是哪个感知器失败
//! - `ModelLoadError`/`InferenceError`/`PreprocessError` 为 ONNX 推理流程专用,
//!   分别对应模型加载、推理执行、输入预处理三个阶段,便于调用方区分错误来源
//! - 新增变体不影响现有 `From<EventBusError>` / `From<NexusError>` 转换逻辑

use thiserror::Error;

/// NMC 编码器错误枚举 — 覆盖多模态感知与融合的所有失败场景
#[derive(Debug, Error)]
pub enum NmcError {
    /// 无效模态 — 输入与感知器模态不匹配,或模态不被支持
    #[error("无效模态: {reason}")]
    InvalidModality {
        /// 失败原因描述
        reason: String,
    },

    /// 编码失败 — 某模态感知器在编码过程中出错
    #[error("编码失败(模态={modality}): {reason}")]
    EncodingFailed {
        /// 失败的模态名称(如 "Text"/"Image")
        modality: String,
        /// 失败原因描述
        reason: String,
    },

    /// 配置错误 — NmcConfig 校验失败(如 clv_dim != CLV::DIMENSION)
    #[error("配置错误: {reason}")]
    ConfigError {
        /// 失败原因描述
        reason: String,
    },

    /// 维度不匹配 — 融合输出维度与 CLV::DIMENSION(512)不一致
    #[error("维度不匹配: 期望 {expected}, 实际 {actual}")]
    DimensionMismatch {
        /// 期望维度(固定为 512)
        expected: usize,
        /// 实际维度
        actual: usize,
    },

    /// 模型加载失败 — ONNX 模型文件不存在、损坏或格式不兼容
    #[error("模型加载失败(模型={model_name}): {reason}")]
    ModelLoadError {
        /// 模型名称(含模态标签,如 "图像(clip-vit-b32.onnx)")
        model_name: String,
        /// 失败原因(如 "文件不存在"、"格式不兼容")
        reason: String,
    },

    /// 推理失败 — ONNX 推理过程中出错
    #[error("推理失败(模态={modality}): {reason}")]
    InferenceError {
        /// 模态名称(如 "Image"/"Video"/"Audio")
        modality: String,
        /// 失败原因(如 "会话创建失败"、"张量形状不匹配")
        reason: String,
    },

    /// 预处理失败 — 输入数据格式不正确或无法解码
    #[error("预处理失败(模态={modality}): {reason}")]
    PreprocessError {
        /// 模态名称(如 "Image"/"Video"/"Audio")
        modality: String,
        /// 失败原因(如 "解码失败"、"尺寸不匹配")
        reason: String,
    },
}

/// 从 event_bus::EventBusError 转换 — 事件发布失败时向上传播
impl From<event_bus::EventBusError> for NmcError {
    fn from(e: event_bus::EventBusError) -> Self {
        Self::EncodingFailed {
            modality: "EventBus".into(),
            reason: e.to_string(),
        }
    }
}

/// 从 nexus_core::NexusError 转换 — CLV 构造失败时向上传播
impl From<nexus_core::NexusError> for NmcError {
    fn from(e: nexus_core::NexusError) -> Self {
        match e {
            nexus_core::NexusError::InvalidClvDimension { expected, actual } => {
                Self::DimensionMismatch { expected, actual }
            }
            other => Self::EncodingFailed {
                modality: "CLV".into(),
                reason: other.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_modality_display() {
        let err = NmcError::InvalidModality {
            reason: "unsupported modality".into(),
        };
        assert!(err.to_string().contains("unsupported modality"));
    }

    #[test]
    fn test_encoding_failed_display() {
        let err = NmcError::EncodingFailed {
            modality: "Image".into(),
            reason: "not implemented".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("Image"));
        assert!(msg.contains("not implemented"));
    }

    #[test]
    fn test_config_error_display() {
        let err = NmcError::ConfigError {
            reason: "clv_dim must be 512".into(),
        };
        assert!(err.to_string().contains("clv_dim must be 512"));
    }

    #[test]
    fn test_dimension_mismatch_display() {
        let err = NmcError::DimensionMismatch {
            expected: 512,
            actual: 256,
        };
        let msg = err.to_string();
        assert!(msg.contains("512"));
        assert!(msg.contains("256"));
    }

    #[test]
    fn test_model_load_error_display() {
        let err = NmcError::ModelLoadError {
            model_name: "clip-vit-b32.onnx".into(),
            reason: "文件不存在".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("clip-vit-b32.onnx"));
        assert!(msg.contains("文件不存在"));
        assert!(msg.contains("模型加载失败"));
    }

    #[test]
    fn test_inference_error_display() {
        let err = NmcError::InferenceError {
            modality: "Image".into(),
            reason: "会话创建失败".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("Image"));
        assert!(msg.contains("会话创建失败"));
        assert!(msg.contains("推理失败"));
    }

    #[test]
    fn test_preprocess_error_display() {
        let err = NmcError::PreprocessError {
            modality: "Video".into(),
            reason: "解码失败:不支持的编码格式".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("Video"));
        assert!(msg.contains("解码失败"));
        assert!(msg.contains("预处理失败"));
    }

    #[test]
    fn test_from_nexus_error_invalid_clv() {
        let nexus_err = nexus_core::NexusError::InvalidClvDimension {
            expected: 512,
            actual: 256,
        };
        let nmc_err: NmcError = nexus_err.into();
        assert!(matches!(nmc_err, NmcError::DimensionMismatch { .. }));
    }
}
