//! 视频感知器 — 使用 ONNX Runtime 将视频编码为语义嵌入
//!
//! 对应架构层:L2 Memory
//!
//! # 模型
//! VideoMAE (采样帧 × 224x224 → 512-dim embedding)
//! 当 ONNX 模型不可用时，回退到占位实现（返回 EncodingFailed）

use crate::config::NmcConfig;
use crate::error::NmcError;
use crate::perceptors::onnx_backend::{ModelType, OnnxBackend};
use crate::perceptors::{sha256_hex, Perceptor};
use crate::types::{CognitiveElement, Modality, PerceptionInput};

/// 视频感知器 — 使用 ONNX Runtime 将视频编码为语义嵌入
///
/// 模型: VideoMAE (采样帧 × 224x224 → 512-dim embedding)
/// 当 ONNX 模型不可用时，回退到占位实现（返回 EncodingFailed）
pub struct VideoPerceptor {
    /// ONNX 推理后端（可选，None 表示未启用 ONNX）
    backend: Option<OnnxBackend>,
    /// 配置（存储用于未来日志/重配置等场景，遵循 OnnxBackend 的相同模式）
    #[allow(dead_code)]
    config: NmcConfig,
}

impl VideoPerceptor {
    /// 创建视频感知器，尝试加载 ONNX 模型
    ///
    /// 加载失败时 backend 为 None，perceive() 将回退到占位行为
    pub fn new(config: NmcConfig) -> Self {
        let backend = if config.model_dir.is_empty() {
            None
        } else {
            OnnxBackend::load(&config, ModelType::Video).ok()
        };
        Self { backend, config }
    }
}

impl Perceptor for VideoPerceptor {
    fn modality(&self) -> Modality {
        Modality::Video
    }

    fn perceive(&self, input: &PerceptionInput) -> Result<CognitiveElement, NmcError> {
        // 1. 校验输入模态
        let bytes = match input {
            PerceptionInput::Video(b) => b,
            other => {
                return Err(NmcError::InvalidModality {
                    reason: format!("VideoPerceptor 仅接受 Video 输入,收到 {}", other.modality()),
                });
            }
        };

        // 2. 空输入检查
        if bytes.is_empty() {
            return Err(NmcError::PreprocessError {
                modality: "Video".into(),
                reason: "空视频字节数组".into(),
            });
        }

        // 3. ONNX 推理路径
        if let Some(ref backend) = self.backend {
            let tensor = OnnxBackend::preprocess_video(bytes)?;
            let embedding = backend.run(tensor)?;
            let content_hash = sha256_hex(bytes);
            return Ok(CognitiveElement::new(
                Modality::Video,
                content_hash,
                embedding,
            ));
        }

        // 4. Fallback: 占位实现
        Err(NmcError::EncodingFailed {
            modality: "Video".into(),
            reason: "ONNX 模型未加载，请配置 model_dir 并放置 videomae-base.onnx".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认配置下 model_dir 为空，backend 为 None，perceive 返回 EncodingFailed
    #[test]
    fn test_video_perceptor_onnx_fallback() {
        let p = VideoPerceptor::new(NmcConfig::default());
        let result = p.perceive(&PerceptionInput::Video(vec![0; 1024]));
        assert!(matches!(result, Err(NmcError::EncodingFailed { .. })));
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Video"));
        assert!(err.to_string().contains("ONNX"));
    }

    /// 错误模态输入应返回 InvalidModality
    #[test]
    fn test_video_perceptor_wrong_modality() {
        let p = VideoPerceptor::new(NmcConfig::default());
        let result = p.perceive(&PerceptionInput::Audio(vec![0; 512]));
        assert!(matches!(result, Err(NmcError::InvalidModality { .. })));
    }

    /// 空字节输入应返回 PreprocessError
    #[test]
    fn test_video_perceptor_empty_bytes() {
        let p = VideoPerceptor::new(NmcConfig::default());
        let result = p.perceive(&PerceptionInput::Video(vec![]));
        assert!(matches!(result, Err(NmcError::PreprocessError { .. })));
        let err = result.unwrap_err();
        match err {
            NmcError::PreprocessError { modality, reason } => {
                assert_eq!(modality, "Video");
                assert!(reason.contains("空"));
            }
            _ => panic!("期望 PreprocessError"),
        }
    }
}
