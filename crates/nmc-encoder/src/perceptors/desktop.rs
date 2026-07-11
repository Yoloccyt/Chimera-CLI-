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

/// 桌面感知器 — 基于区域描述文本的字节频率嵌入
///
/// v2.0:使用 clv_dim(512)维度输出 + L2 归一化,与其他感知器统一。
/// 未来可扩展为结合截图字节实现多模态桌面编码。
pub struct DesktopPerceptor {
    /// 配置(含 clv_dim 维度参数,用于描述文本嵌入维度)
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

        // v2.0 迁移:使用 clv_dim(512)而非 text_dim(256),与 CLV 对齐。
        // 所有 v2.0 感知器输出统一 512-dim 嵌入,便于融合引擎处理。
        let mut embedding = byte_frequency_embedding(desc_bytes, self.config.clv_dim);
        // L2 归一化,与其他 v2.0 感知器(Image/Video/Audio)保持一致,
        // 确保嵌入在超球面上,余弦相似度可比。
        l2_normalize(&mut embedding);

        Ok(CognitiveElement::new(
            Modality::Desktop,
            content_hash,
            embedding,
        ))
    }
}

/// L2 归一化 — 将向量缩放到单位长度
///
/// WHY:确保桌面嵌入在超球面上,与其他 v2.0 感知器一致,
/// 使余弦相似度等价于欧氏距离,不同描述文本的嵌入幅度可比。
fn l2_normalize(vec: &mut [f32]) {
    let sum_sq: f32 = vec.iter().map(|&v| v * v).sum();
    if sum_sq > 0.0 {
        let norm = sum_sq.sqrt();
        for v in vec.iter_mut() {
            *v /= norm;
        }
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
        // v2.0:嵌入维度为 clv_dim(512),与 CLV 对齐
        assert_eq!(elem.embedding_dim(), 512);
        assert!(!elem.content_hash.is_empty());
        // v2.0:L2 归一化后范数 ≈ 1.0
        let norm_sq: f32 = elem.embedding.iter().map(|&v| v * v).sum();
        assert!((norm_sq - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_desktop_perceptor_empty_description() {
        let p = DesktopPerceptor::new(NmcConfig::default());
        let input = PerceptionInput::Desktop(make_capture(""));
        let elem = p.perceive(&input).unwrap();
        assert_eq!(elem.modality, Modality::Desktop);
        // v2.0:嵌入维度为 clv_dim(512)
        assert_eq!(elem.embedding_dim(), 512);
        // 空描述:所有桶为 0.0(L2 归一化对零向量无操作)
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
        // v2.0:text_dim 不再影响输出维度,输出始终为 clv_dim(512)
        // (与 TextPerceptor.test_text_perceptor_custom_text_dim 行为一致)
        let config = NmcConfig::default().with_text_dim(128);
        let p = DesktopPerceptor::new(config);
        let elem = p
            .perceive(&PerceptionInput::Desktop(make_capture("test")))
            .unwrap();
        assert_eq!(elem.embedding_dim(), 512);
    }

    #[test]
    fn test_desktop_perceptor_chinese_description() {
        let p = DesktopPerceptor::new(NmcConfig::default());
        let elem = p
            .perceive(&PerceptionInput::Desktop(make_capture("代码编辑器")))
            .unwrap();
        // v2.0:嵌入维度为 clv_dim(512)
        assert_eq!(elem.embedding_dim(), 512);
        // v2.0:L2 归一化后范数 ≈ 1.0
        let norm_sq: f32 = elem.embedding.iter().map(|&v| v * v).sum();
        assert!((norm_sq - 1.0).abs() < 1e-5);
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
