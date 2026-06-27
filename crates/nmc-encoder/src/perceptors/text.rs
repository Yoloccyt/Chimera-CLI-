//! 文本感知器 — 将文本输入编码为认知元素
//!
//! 对应架构层:L2 Memory
//!
//! # 实现说明
//! 本周使用 SHA256 + 字符频率统计的占位实现,Week 7/8 将接入 ort ONNX
//! Runtime 实现真正的语义嵌入。占位实现的特性:
//! - **确定性**:相同输入始终产生相同输出(哈希 + 频率统计)
//! - **维度**:text_dim 维(默认 256),每个 UTF-8 字节映射到一个桶
//! - **归一化**:频率向量归一化到 [0, 1],便于后续融合

use crate::config::NmcConfig;
use crate::error::NmcError;
use crate::perceptors::{byte_frequency_embedding, sha256_hex, Perceptor};
use crate::types::{CognitiveElement, Modality, PerceptionInput};

/// 文本感知器 — 基于字符频率统计的占位实现
///
/// TODO(Week 7/8): 接入 ort ONNX Runtime 实现语义嵌入
pub struct TextPerceptor {
    /// 配置(含 text_dim 维度参数)
    config: NmcConfig,
}

impl TextPerceptor {
    /// 创建文本感知器
    pub fn new(config: NmcConfig) -> Self {
        Self { config }
    }

    /// 返回配置引用
    pub fn config(&self) -> &NmcConfig {
        &self.config
    }
}

impl Perceptor for TextPerceptor {
    fn modality(&self) -> Modality {
        Modality::Text
    }

    fn perceive(&self, input: &PerceptionInput) -> Result<CognitiveElement, NmcError> {
        let text = match input {
            PerceptionInput::Text(t) => t.as_str(),
            other => {
                return Err(NmcError::InvalidModality {
                    reason: format!("TextPerceptor 仅接受 Text 输入,收到 {}", other.modality()),
                });
            }
        };

        // content_hash: SHA256 of UTF-8 bytes
        let content_hash = sha256_hex(text.as_bytes());

        // embedding: 字符频率统计(text_dim 维)
        // TODO(Week 7/8): 接入 ort ONNX Runtime 实现语义嵌入
        let embedding = byte_frequency_embedding(text.as_bytes(), self.config.text_dim);

        Ok(CognitiveElement::new(
            Modality::Text,
            content_hash,
            embedding,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_perceptor_empty_text() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem = p.perceive(&PerceptionInput::Text(String::new())).unwrap();
        assert_eq!(elem.modality, Modality::Text);
        assert_eq!(elem.embedding_dim(), 256);
        // 空文本:所有桶为 0.0
        assert!(elem.embedding.iter().all(|&v| v == 0.0));
        // 空文本仍有有效哈希
        assert!(!elem.content_hash.is_empty());
    }

    #[test]
    fn test_text_perceptor_long_text_10kb() {
        let p = TextPerceptor::new(NmcConfig::default());
        let long_text = "a".repeat(10_000);
        let elem = p
            .perceive(&PerceptionInput::Text(long_text.clone()))
            .unwrap();
        assert_eq!(elem.embedding_dim(), 256);
        // 全 'a' 文本:字节 0x61 对应桶 0x61 % 256 = 97,该桶值为 1.0
        assert!((elem.embedding[97] - 1.0).abs() < 1e-6);
        // 其余桶为 0.0
        for (i, &v) in elem.embedding.iter().enumerate() {
            if i != 97 {
                assert!(v.abs() < 1e-6, "桶 {i} 应为 0,实际为 {v}");
            }
        }
    }

    #[test]
    fn test_text_perceptor_chinese() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem = p
            .perceive(&PerceptionInput::Text("你好世界".into()))
            .unwrap();
        assert_eq!(elem.embedding_dim(), 256);
        // 中文 UTF-8 编码为多字节,频率应分布在多个桶
        let non_zero = elem.embedding.iter().filter(|&&v| v > 0.0).count();
        assert!(non_zero > 0, "中文文本应产生非零嵌入");
        // 频率之和应接近 1.0(归一化)
        let sum: f32 = elem.embedding.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "频率之和应为 1.0,实际为 {sum}");
    }

    #[test]
    fn test_text_perceptor_unicode_emoji() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem = p
            .perceive(&PerceptionInput::Text("Hello 🌍🚀".into()))
            .unwrap();
        assert_eq!(elem.embedding_dim(), 256);
        let sum: f32 = elem.embedding.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_text_perceptor_special_chars() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem = p
            .perceive(&PerceptionInput::Text("!@#$%^&*()".into()))
            .unwrap();
        assert_eq!(elem.embedding_dim(), 256);
        let sum: f32 = elem.embedding.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_text_perceptor_repeated_text() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem1 = p.perceive(&PerceptionInput::Text("abcabc".into())).unwrap();
        let elem2 = p.perceive(&PerceptionInput::Text("abcabc".into())).unwrap();
        // 相同文本产生相同哈希与嵌入
        assert_eq!(elem1.content_hash, elem2.content_hash);
        assert_eq!(elem1.embedding, elem2.embedding);
    }

    #[test]
    fn test_text_perceptor_content_hash_deterministic() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem1 = p.perceive(&PerceptionInput::Text("hello".into())).unwrap();
        let elem2 = p.perceive(&PerceptionInput::Text("hello".into())).unwrap();
        assert_eq!(elem1.content_hash, elem2.content_hash);
        // SHA256 of "hello" 应为固定值
        assert_eq!(
            elem1.content_hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn test_text_perceptor_different_text_different_hash() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem1 = p.perceive(&PerceptionInput::Text("hello".into())).unwrap();
        let elem2 = p.perceive(&PerceptionInput::Text("world".into())).unwrap();
        assert_ne!(elem1.content_hash, elem2.content_hash);
    }

    #[test]
    fn test_text_perceptor_wrong_modality() {
        let p = TextPerceptor::new(NmcConfig::default());
        let result = p.perceive(&PerceptionInput::Image(vec![1, 2, 3]));
        assert!(matches!(result, Err(NmcError::InvalidModality { .. })));
    }

    #[test]
    fn test_text_perceptor_custom_text_dim() {
        let config = NmcConfig::default().with_text_dim(128);
        let p = TextPerceptor::new(config);
        let elem = p.perceive(&PerceptionInput::Text("test".into())).unwrap();
        assert_eq!(elem.embedding_dim(), 128);
    }

    #[test]
    fn test_text_perceptor_embedding_normalized() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem = p
            .perceive(&PerceptionInput::Text("The quick brown fox".into()))
            .unwrap();
        let sum: f32 = elem.embedding.iter().sum();
        // 非空文本的频率之和应为 1.0
        assert!((sum - 1.0).abs() < 1e-5, "频率之和应为 1.0,实际为 {sum}");
    }
}
