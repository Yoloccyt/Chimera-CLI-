//! 视频感知器 — 占位实现(Week 7/8 接入 ort ONNX Runtime)
//!
//! 对应架构层:L2 Memory
//!
//! # 当前状态
//! 本周为占位实现,`perceive` 始终返回 `EncodingFailed` 错误。
//! Week 7/8 将接入 ort ONNX Runtime 实现视频编码(如 VideoMAE)。

use crate::error::NmcError;
use crate::perceptors::Perceptor;
use crate::types::{CognitiveElement, Modality, PerceptionInput};

/// 视频感知器 — 占位实现
///
/// TODO(Week 7/8): 接入 ort ONNX Runtime 实现视频编码
pub struct VideoPerceptor;

impl VideoPerceptor {
    /// 创建视频感知器(占位,无配置)
    pub fn new() -> Self {
        Self
    }
}

impl Default for VideoPerceptor {
    fn default() -> Self {
        Self::new()
    }
}

impl Perceptor for VideoPerceptor {
    fn modality(&self) -> Modality {
        Modality::Video
    }

    fn perceive(&self, input: &PerceptionInput) -> Result<CognitiveElement, NmcError> {
        if !matches!(input, PerceptionInput::Video(_)) {
            return Err(NmcError::InvalidModality {
                reason: format!("VideoPerceptor 仅接受 Video 输入,收到 {}", input.modality()),
            });
        }
        // TODO(Week 7/8): 接入 ort ONNX Runtime 实现视频编码
        Err(NmcError::EncodingFailed {
            modality: "Video".into(),
            reason: "Video perceptor not implemented yet, TODO Week 7/8".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_video_perceptor_returns_error() {
        let p = VideoPerceptor::new();
        let result = p.perceive(&PerceptionInput::Video(vec![0; 1024]));
        assert!(matches!(result, Err(NmcError::EncodingFailed { .. })));
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Video"));
    }

    #[test]
    fn test_video_perceptor_wrong_modality() {
        let p = VideoPerceptor::new();
        let result = p.perceive(&PerceptionInput::Audio(vec![0; 512]));
        assert!(matches!(result, Err(NmcError::InvalidModality { .. })));
    }
}
