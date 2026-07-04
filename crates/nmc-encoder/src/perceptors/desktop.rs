//! 桌面感知器 — 基于区域描述文本的编码
//!
//! 对应架构层:L2 Memory
//!
//! # 实现说明
//! 桌面模态不传递原始像素(数据量过大),而是传递区域描述文本。
//! 感知器基于描述文本生成 SHA256 哈希与字符频率嵌入,逻辑与 TextPerceptor 类似。
//! Week 7/8 可扩展为结合截图字节与描述文本的多模态编码。

use crate::config::NmcConfig;
use crate::error::NmcError;
use crate::perceptors::{byte_frequency_embedding, sha256_hex, Perceptor};
use crate::types::{CognitiveElement, Modality, PerceptionInput};

/// 桌面感知器 — 基于区域描述文本的占位编码
///
/// TODO(Week 7/8): 结合截图字节实现多模态桌面编码
pub struct DesktopPerceptor {
    /// 配置(含 text_dim 维度参数,用于描述文本嵌入)
    config: NmcConfig,
}

impl DesktopPerceptor {
    /// 创建桌面感知器
    pub fn new(config: NmcConfig) -> Self {
        Self { config }
    }

    /// 返回配置引用
    pub fn config(&self) -> &NmcConfig {
        &self.config
    }
}

impl Perceptor for DesktopPerceptor {
    fn modality(&self) -> Modality {
        Modality::Desktop
    }

    fn perceive(&self, input: &PerceptionInput) -> Result<CognitiveElement, NmcError> {
        let capture = match input {
            PerceptionInput::Desktop(c) => c,
            other => {
                return Err(NmcError::InvalidModality {
                    reason: format!(
                        "DesktopPerceptor 仅接受 Desktop 输入,收到 {}",
                        other.modality()
                    ),
                });
            }
        };

        // 基于 region_description 生成哈希与嵌入
        // WHY:桌面模态的核心语义信息在区域描述文本中(如 "code editor with Rust syntax"),
        // 像素数据本周不处理,描述文本足以区分不同桌面状态
        let desc_bytes = capture.region_description.as_bytes();
        let content_hash = sha256_hex(desc_bytes);
        let embedding = byte_frequency_embedding(desc_bytes, self.config.text_dim);

        Ok(CognitiveElement::new(
            Modality::Desktop,
            content_hash,
            embedding,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DesktopCapture;

    fn make_capture(desc: &str) -> DesktopCapture {
        DesktopCapture::new(1920, 1080, desc)
    }

    #[test]
    fn test_desktop_perceptor_basic() {
        let p = DesktopPerceptor::new(NmcConfig::default());
        let input = PerceptionInput::Desktop(make_capture("code editor with Rust syntax"));
        let elem = p.perceive(&input).unwrap();
        assert_eq!(elem.modality, Modality::Desktop);
        assert_eq!(elem.embedding_dim(), 256);
        assert!(!elem.content_hash.is_empty());
        let sum: f32 = elem.embedding.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_desktop_perceptor_empty_description() {
        let p = DesktopPerceptor::new(NmcConfig::default());
        let input = PerceptionInput::Desktop(make_capture(""));
        let elem = p.perceive(&input).unwrap();
        assert_eq!(elem.modality, Modality::Desktop);
        assert_eq!(elem.embedding_dim(), 256);
        // 空描述:所有桶为 0.0
        assert!(elem.embedding.iter().all(|&v| v == 0.0));
        assert!(!elem.content_hash.is_empty());
    }

    #[test]
    fn test_desktop_perceptor_hash_deterministic() {
        let p = DesktopPerceptor::new(NmcConfig::default());
        let input1 = PerceptionInput::Desktop(make_capture("terminal window"));
        let input2 = PerceptionInput::Desktop(make_capture("terminal window"));
        let elem1 = p.perceive(&input1).unwrap();
        let elem2 = p.perceive(&input2).unwrap();
        assert_eq!(elem1.content_hash, elem2.content_hash);
        assert_eq!(elem1.embedding, elem2.embedding);
    }

    #[test]
    fn test_desktop_perceptor_different_desc_different_hash() {
        let p = DesktopPerceptor::new(NmcConfig::default());
        let elem1 = p
            .perceive(&PerceptionInput::Desktop(make_capture("editor")))
            .unwrap();
        let elem2 = p
            .perceive(&PerceptionInput::Desktop(make_capture("browser")))
            .unwrap();
        assert_ne!(elem1.content_hash, elem2.content_hash);
    }

    #[test]
    fn test_desktop_perceptor_wrong_modality() {
        let p = DesktopPerceptor::new(NmcConfig::default());
        let result = p.perceive(&PerceptionInput::Text("hello".into()));
        assert!(matches!(result, Err(NmcError::InvalidModality { .. })));
    }

    #[test]
    fn test_desktop_perceptor_custom_text_dim() {
        let config = NmcConfig::default().with_text_dim(128);
        let p = DesktopPerceptor::new(config);
        let elem = p
            .perceive(&PerceptionInput::Desktop(make_capture("test")))
            .unwrap();
        assert_eq!(elem.embedding_dim(), 128);
    }

    #[test]
    fn test_desktop_perceptor_chinese_description() {
        let p = DesktopPerceptor::new(NmcConfig::default());
        let elem = p
            .perceive(&PerceptionInput::Desktop(make_capture("代码编辑器")))
            .unwrap();
        assert_eq!(elem.embedding_dim(), 256);
        let sum: f32 = elem.embedding.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_desktop_perceptor_width_height_ignored_in_embedding() {
        let p = DesktopPerceptor::new(NmcConfig::default());
        let cap1 = DesktopCapture::new(1920, 1080, "same description");
        let cap2 = DesktopCapture::new(800, 600, "same description");
        let elem1 = p.perceive(&PerceptionInput::Desktop(cap1)).unwrap();
        let elem2 = p.perceive(&PerceptionInput::Desktop(cap2)).unwrap();
        // 相同描述 → 相同哈希与嵌入(宽高不影响编码)
        assert_eq!(elem1.content_hash, elem2.content_hash);
        assert_eq!(elem1.embedding, elem2.embedding);
    }
}
