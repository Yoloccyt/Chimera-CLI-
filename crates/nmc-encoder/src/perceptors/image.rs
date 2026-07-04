//! 图像感知器 — 占位实现(Week 7/8 接入 ort ONNX Runtime)
//!
//! 对应架构层:L2 Memory
//!
//! # 当前状态
//! 本周为占位实现,`perceive` 始终返回 `EncodingFailed` 错误。
//! Week 7/8 将接入 ort ONNX Runtime 实现图像编码(如 CLIP ViT)。

use crate::error::NmcError;
use crate::perceptors::Perceptor;
use crate::types::{CognitiveElement, Modality, PerceptionInput};

/// 图像感知器 — 占位实现
///
/// TODO(Week 7/8): 接入 ort ONNX Runtime 实现图像编码
pub struct ImagePerceptor;

impl ImagePerceptor {
    /// 创建图像感知器(占位,无配置)
    pub fn new() -> Self {
        Self
    }
}

impl Default for ImagePerceptor {
    fn default() -> Self {
        Self::new()
    }
}

impl Perceptor for ImagePerceptor {
    fn modality(&self) -> Modality {
        Modality::Image
    }

    fn perceive(&self, input: &PerceptionInput) -> Result<CognitiveElement, NmcError> {
        // 校验输入模态
        if !matches!(input, PerceptionInput::Image(_)) {
            return Err(NmcError::InvalidModality {
                reason: format!("ImagePerceptor 仅接受 Image 输入,收到 {}", input.modality()),
            });
        }
        // TODO(Week 7/8): 接入 ort ONNX Runtime 实现图像编码
        Err(NmcError::EncodingFailed {
            modality: "Image".into(),
            reason: "Image perceptor not implemented yet, TODO Week 7/8".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_perceptor_returns_error() {
        let p = ImagePerceptor::new();
        let result = p.perceive(&PerceptionInput::Image(vec![0xFF; 1024]));
        assert!(matches!(result, Err(NmcError::EncodingFailed { .. })));
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Image"));
        assert!(err.to_string().contains("Week 7/8"));
    }

    #[test]
    fn test_image_perceptor_wrong_modality() {
        let p = ImagePerceptor::new();
        let result = p.perceive(&PerceptionInput::Text("hello".into()));
        assert!(matches!(result, Err(NmcError::InvalidModality { .. })));
    }

    #[test]
    fn test_image_perceptor_empty_bytes() {
        let p = ImagePerceptor::new();
        let result = p.perceive(&PerceptionInput::Image(vec![]));
        // 即使空字节,占位实现也返回 EncodingFailed(而非 InvalidModality)
        assert!(matches!(result, Err(NmcError::EncodingFailed { .. })));
    }
}
