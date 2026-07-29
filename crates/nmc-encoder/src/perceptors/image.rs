//! 图像感知器 — 使用 ONNX Runtime 将图像编码为语义嵌入
//!
//! 对应架构层: L2 Memory
//! 模型: CLIP ViT-B/32 (224x224 RGB → 512-dim embedding)
//! 当 ONNX 模型不可用时，回退到占位实现（返回 EncodingFailed）

use crate::config::NmcConfig;
use crate::error::NmcError;
use crate::perceptors::onnx_backend::{ModelType, OnnxBackend};
use crate::perceptors::{sha256_hex, Perceptor};
use crate::types::{CognitiveElement, Modality, PerceptionInput};

/// 图像感知器 — 使用 ONNX Runtime 将图像编码为语义嵌入
///
/// 模型: CLIP ViT-B/32 (224x224 RGB → 512-dim embedding)
/// 当 ONNX 模型不可用时，回退到占位实现（返回 EncodingFailed）
pub struct ImagePerceptor {
    /// ONNX 推理后端（可选，None 表示未启用 ONNX）
    backend: Option<OnnxBackend>,
    /// 配置（存储用于未来日志/重配置等场景，遵循 OnnxBackend 的相同模式）
    #[allow(dead_code)]
    config: NmcConfig,
}

impl ImagePerceptor {
    /// 创建图像感知器，尝试加载 ONNX 模型
    ///
    /// 加载失败时 backend 为 None，perceive() 将回退到占位行为
    pub fn new(config: NmcConfig) -> Self {
        let backend = if config.model_dir.is_empty() {
            None
        } else {
            OnnxBackend::load(&config, ModelType::Image).ok()
        };
        Self { backend, config }
    }
}

impl Perceptor for ImagePerceptor {
    fn modality(&self) -> Modality {
        Modality::Image
    }

    fn perceive(&self, input: &PerceptionInput) -> Result<CognitiveElement, NmcError> {
        // 1. 校验输入模态
        let bytes = match input {
            PerceptionInput::Image(b) => b,
            other => {
                return Err(NmcError::InvalidModality {
                    reason: format!("ImagePerceptor 仅接受 Image 输入,收到 {}", other.modality()),
                });
            }
        };

        // 2. 空输入检查
        if bytes.is_empty() {
            return Err(NmcError::PreprocessError {
                modality: "Image".into(),
                reason: "空图像字节数组".into(),
            });
        }

        // 3. ONNX 推理路径
        if let Some(ref backend) = self.backend {
            let tensor = OnnxBackend::preprocess_image(bytes)?;
            let embedding = backend.run(tensor)?;
            let content_hash = sha256_hex(bytes);
            return Ok(CognitiveElement::new(
                Modality::Image,
                content_hash,
                embedding,
            ));
        }

        // 4. Fallback: 占位实现
        Err(NmcError::EncodingFailed {
            modality: "Image".into(),
            reason: "ONNX 模型未加载，请配置 model_dir 并放置 clip-vit-b32.onnx".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认配置（model_dir 为空）→ 返回 EncodingFailed（向后兼容）
    #[test]
    fn test_image_perceptor_onnx_fallback() {
        let p = ImagePerceptor::new(NmcConfig::default());
        let result = p.perceive(&PerceptionInput::Image(vec![0xFF; 1024]));
        assert!(matches!(result, Err(NmcError::EncodingFailed { .. })));
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Image"));
        assert!(err.to_string().contains("ONNX"));
    }

    /// 错误模态输入 → InvalidModality
    #[test]
    fn test_image_perceptor_wrong_modality() {
        let p = ImagePerceptor::new(NmcConfig::default());
        let result = p.perceive(&PerceptionInput::Text("hello".into()));
        assert!(matches!(result, Err(NmcError::InvalidModality { .. })));
    }

    /// 空输入 → PreprocessError（不再是 EncodingFailed）
    #[test]
    fn test_image_perceptor_empty_bytes() {
        let p = ImagePerceptor::new(NmcConfig::default());
        let result = p.perceive(&PerceptionInput::Image(vec![]));
        assert!(matches!(result, Err(NmcError::PreprocessError { .. })));
        let err = result.unwrap_err();
        assert!(err.to_string().contains("空图像"));
    }

    /// 有效 PNG 但无 ONNX 模型 → 正确 fallback
    #[test]
    fn test_image_perceptor_valid_png_no_model() {
        let p = ImagePerceptor::new(NmcConfig::default());
        // 有效 PNG 但无 ONNX 模型 → 正确 fallback 到 EncodingFailed
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([128, 64, 32]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("编码 PNG 应成功");

        let result = p.perceive(&PerceptionInput::Image(buf));
        assert!(matches!(result, Err(NmcError::EncodingFailed { .. })));
        let err = result.unwrap_err();
        assert!(err.to_string().contains("ONNX"));
    }
}
