//! 音频感知器 — 占位实现(Week 7/8 接入 ort ONNX Runtime)
//!
//! 对应架构层:L2 Memory
//!
//! # 当前状态
//! 本周为占位实现,`perceive` 始终返回 `EncodingFailed` 错误。
//! Week 7/8 将接入 ort ONNX Runtime 实现音频编码(如 Whisper encoder)。

use crate::error::NmcError;
use crate::perceptors::Perceptor;
use crate::types::{CognitiveElement, Modality, PerceptionInput};

/// 音频感知器 — 占位实现
///
/// TODO(Week 7/8): 接入 ort ONNX Runtime 实现音频编码
pub struct AudioPerceptor;

impl AudioPerceptor {
    /// 创建音频感知器(占位,无配置)
    pub fn new() -> Self {
        Self
    }
}

impl Default for AudioPerceptor {
    fn default() -> Self {
        Self::new()
    }
}

impl Perceptor for AudioPerceptor {
    fn modality(&self) -> Modality {
        Modality::Audio
    }

    fn perceive(&self, input: &PerceptionInput) -> Result<CognitiveElement, NmcError> {
        if !matches!(input, PerceptionInput::Audio(_)) {
            return Err(NmcError::InvalidModality {
                reason: format!("AudioPerceptor 仅接受 Audio 输入,收到 {}", input.modality()),
            });
        }
        // TODO(Week 7/8): 接入 ort ONNX Runtime 实现音频编码
        Err(NmcError::EncodingFailed {
            modality: "Audio".into(),
            reason: "Audio perceptor not implemented yet, TODO Week 7/8".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_perceptor_returns_error() {
        let p = AudioPerceptor::new();
        let result = p.perceive(&PerceptionInput::Audio(vec![0; 512]));
        assert!(matches!(result, Err(NmcError::EncodingFailed { .. })));
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Audio"));
    }

    #[test]
    fn test_audio_perceptor_wrong_modality() {
        let p = AudioPerceptor::new();
        let result = p.perceive(&PerceptionInput::Text("hello".into()));
        assert!(matches!(result, Err(NmcError::InvalidModality { .. })));
    }
}
