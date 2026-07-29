//! 音频感知器 — 使用 ONNX Runtime 将音频编码为语义嵌入
//!
//! 对应架构层:L2 Memory
//!
//! # 模型
//! Whisper encoder (16kHz 单声道 → Mel 频谱 → 512-dim embedding)
//! 当 ONNX 模型不可用时，回退到占位实现（返回 EncodingFailed）

use crate::config::NmcConfig;
use crate::error::NmcError;
use crate::perceptors::onnx_backend::{ModelType, OnnxBackend};
use crate::perceptors::{sha256_hex, Perceptor};
use crate::types::{CognitiveElement, Modality, PerceptionInput};

/// 音频感知器 — 使用 ONNX Runtime 将音频编码为语义嵌入
///
/// 模型: Whisper encoder (16kHz 单声道 → Mel 频谱 → 512-dim embedding)
/// 当 ONNX 模型不可用时，回退到占位实现（返回 EncodingFailed）
pub struct AudioPerceptor {
    /// ONNX 推理后端（可选，None 表示未启用 ONNX）
    backend: Option<OnnxBackend>,
    /// 配置（存储用于未来日志/重配置等场景，遵循 OnnxBackend 的相同模式）
    #[allow(dead_code)]
    config: NmcConfig,
}

impl AudioPerceptor {
    /// 创建音频感知器，尝试加载 ONNX 模型
    ///
    /// 加载失败时 backend 为 None，perceive() 将回退到占位行为
    pub fn new(config: NmcConfig) -> Self {
        let backend = if config.model_dir.is_empty() {
            None
        } else {
            OnnxBackend::load(&config, ModelType::Audio).ok()
        };
        Self { backend, config }
    }
}

impl Perceptor for AudioPerceptor {
    fn modality(&self) -> Modality {
        Modality::Audio
    }

    fn perceive(&self, input: &PerceptionInput) -> Result<CognitiveElement, NmcError> {
        // 1. 校验输入模态
        let bytes = match input {
            PerceptionInput::Audio(b) => b,
            other => {
                return Err(NmcError::InvalidModality {
                    reason: format!("AudioPerceptor 仅接受 Audio 输入,收到 {}", other.modality()),
                });
            }
        };

        // 2. 空输入检查
        if bytes.is_empty() {
            return Err(NmcError::PreprocessError {
                modality: "Audio".into(),
                reason: "空音频字节数组".into(),
            });
        }

        // 3. ONNX 推理路径
        if let Some(ref backend) = self.backend {
            let tensor = OnnxBackend::preprocess_audio(bytes)?;
            let embedding = backend.run(tensor)?;
            let content_hash = sha256_hex(bytes);
            return Ok(CognitiveElement::new(
                Modality::Audio,
                content_hash,
                embedding,
            ));
        }

        // 4. Fallback: ONNX 模型未加载时返回占位错误
        Err(NmcError::EncodingFailed {
            modality: "Audio".into(),
            reason: "ONNX 模型未加载，请配置 model_dir 并放置 whisper-encoder.onnx".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认配置（model_dir 为空）→ 返回 EncodingFailed（向后兼容）
    #[test]
    fn test_audio_perceptor_onnx_fallback() {
        let p = AudioPerceptor::new(NmcConfig::default());
        let result = p.perceive(&PerceptionInput::Audio(vec![0; 512]));
        assert!(matches!(result, Err(NmcError::EncodingFailed { .. })));
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Audio"));
        assert!(err.to_string().contains("ONNX 模型未加载"));
    }

    /// 错误的输入模态 → 返回 InvalidModality
    #[test]
    fn test_audio_perceptor_wrong_modality() {
        let p = AudioPerceptor::new(NmcConfig::default());
        let result = p.perceive(&PerceptionInput::Text("hello".into()));
        assert!(matches!(result, Err(NmcError::InvalidModality { .. })));
    }

    /// 空字节输入 → 返回 PreprocessError
    #[test]
    fn test_audio_perceptor_empty_bytes() {
        let p = AudioPerceptor::new(NmcConfig::default());
        let result = p.perceive(&PerceptionInput::Audio(vec![]));
        assert!(matches!(result, Err(NmcError::PreprocessError { .. })));
        let err = result.unwrap_err();
        match err {
            NmcError::PreprocessError { modality, reason } => {
                assert_eq!(modality, "Audio");
                assert!(reason.contains("空"));
            }
            _ => panic!("期望 PreprocessError"),
        }
    }
}
