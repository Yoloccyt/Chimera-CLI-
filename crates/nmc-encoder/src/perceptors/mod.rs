//! 感知器模块 — 五种模态感知器的统一 trait 与实现
//!
//! 对应架构层:L2 Memory
//! 对应创新点:NMC(Native Multimodal Context,原生多模态上下文编码)
//!
//! # 设计决策(WHY)
//! - **同步 trait 而非 async**:感知器是 CPU 密集型操作(哈希、嵌入计算),
//!   不涉及 IO 等待,使用同步方法避免 async-trait 的堆分配开销。
//!   v3.0 接入 ort ONNX Runtime 后,若需 GPU 异步推理可再改为 async
//! - **trait Perceptor 而非 enum dispatch**:5 种感知器实现差异大(文本 vs 图像),
//!   trait 提供统一接口;调用方通过枚举分发选择具体实现,避免 `Box<dyn Trait>`
//!
//! # 感知器清单(v2.0 P0-2 全部完成)
//! | 感知器 | 模态 | 状态 | 说明 |
//! |--------|------|------|------|
//! | TextPerceptor | Text | v2.0 | n-gram 语义感知哈希 + 位置加权 |
//! | ImagePerceptor | Image | v2.0 | 像素统计特征(颜色/亮度/边缘/纹理) |
//! | VideoPerceptor | Video | v2.0 | 帧统计 + 运动特征 + 时间序列 |
//! | AudioPerceptor | Audio | v2.0 | 时频统计(能量/频谱/过零率/节奏) |
//! | DesktopPerceptor | Desktop | v2.0 | 基于区域描述文本的语义嵌入 |

use crate::error::NmcError;
use crate::types::{CognitiveElement, Modality, PerceptionInput};

pub mod audio;
pub mod desktop;
pub mod image;
pub mod text;
pub mod video;

pub use audio::AudioPerceptor;
pub use desktop::DesktopPerceptor;
pub use image::ImagePerceptor;
pub use text::TextPerceptor;
pub use video::VideoPerceptor;

/// 感知器 trait — 将多模态输入编码为认知元素
///
/// 每种模态有一个对应实现,`perceive` 是同步方法(CPU 密集型,无需 async)
pub trait Perceptor {
    /// 返回此感知器处理的模态
    fn modality(&self) -> Modality;

    /// 感知输入,产出认知元素(含内容哈希与嵌入向量)
    ///
    /// # 错误
    /// - `InvalidModality`:输入模态与此感知器不匹配
    /// - `EncodingFailed`:编码过程出错(如空数据或格式不支持)
    fn perceive(&self, input: &PerceptionInput) -> Result<CognitiveElement, NmcError>;
}

/// 计算 SHA256 哈希并返回十六进制字符串
///
/// WHY 共享函数:所有感知器都需要内容哈希,
/// 提取到此处消除重复实现(遵循 DRY 原则)
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    // 手动格式化为 hex,避免引入 hex crate 依赖
    result.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NmcConfig;

    #[test]
    fn test_text_perceptor_modality() {
        let p = TextPerceptor::new(NmcConfig::default());
        assert_eq!(p.modality(), Modality::Text);
    }

    #[test]
    fn test_image_perceptor_modality() {
        let p = ImagePerceptor::new();
        assert_eq!(p.modality(), Modality::Image);
    }

    #[test]
    fn test_video_perceptor_modality() {
        let p = VideoPerceptor::new();
        assert_eq!(p.modality(), Modality::Video);
    }

    #[test]
    fn test_audio_perceptor_modality() {
        let p = AudioPerceptor::new();
        assert_eq!(p.modality(), Modality::Audio);
    }

    #[test]
    fn test_desktop_perceptor_modality() {
        let p = DesktopPerceptor::new(NmcConfig::default());
        assert_eq!(p.modality(), Modality::Desktop);
    }

    // ── v2.0 P0-2 验收测试:所有感知器均返回有效嵌入 ──

    #[test]
    fn test_all_perceptors_return_valid_embedding() {
        // 验证所有 5 个感知器都能返回 512-dim 有效嵌入
        let text_p = TextPerceptor::new(NmcConfig::default());
        let image_p = ImagePerceptor::new();
        let video_p = VideoPerceptor::new();
        let audio_p = AudioPerceptor::new();
        let desktop_p = DesktopPerceptor::new(NmcConfig::default());

        let text_elem = text_p.perceive(&PerceptionInput::Text("hello world".into())).unwrap();
        let image_elem = image_p.perceive(&PerceptionInput::Image(vec![0x80; 1024])).unwrap();
        let video_elem = video_p.perceive(&PerceptionInput::Video(vec![0x80; 8192])).unwrap();
        let audio_elem = audio_p.perceive(&PerceptionInput::Audio(vec![0x80; 4096])).unwrap();
        let desktop_elem = desktop_p.perceive(&PerceptionInput::Desktop(
            crate::types::DesktopCapture::new(1920, 1080, "code editor")
        )).unwrap();

        // 所有嵌入维度均为 512(与 CLV 对齐)
        assert_eq!(text_elem.embedding_dim(), 512, "TextPerceptor 嵌入维度应为 512");
        assert_eq!(image_elem.embedding_dim(), 512, "ImagePerceptor 嵌入维度应为 512");
        assert_eq!(video_elem.embedding_dim(), 512, "VideoPerceptor 嵌入维度应为 512");
        assert_eq!(audio_elem.embedding_dim(), 512, "AudioPerceptor 嵌入维度应为 512");
        assert_eq!(desktop_elem.embedding_dim(), 512, "DesktopPerceptor 嵌入维度应为 512");

        // 所有非空嵌入 L2 归一化后范数 ≈ 1.0
        for (name, elem) in [
            ("Text", &text_elem),
            ("Image", &image_elem),
            ("Video", &video_elem),
            ("Audio", &audio_elem),
            ("Desktop", &desktop_elem),
        ] {
            let norm_sq: f32 = elem.embedding.iter().map(|&v| v * v).sum();
            assert!(
                (norm_sq - 1.0).abs() < 1e-2,
                "{name}Perceptor 嵌入 L2 范数应为 1.0,实际为 {norm_sq}"
            );
        }
    }

    #[test]
    fn test_all_perceptors_different_inputs_different_hashes() {
        // 验证不同输入产生不同哈希
        let text_p = TextPerceptor::new(NmcConfig::default());
        let elem1 = text_p.perceive(&PerceptionInput::Text("hello".into())).unwrap();
        let elem2 = text_p.perceive(&PerceptionInput::Text("world".into())).unwrap();
        assert_ne!(elem1.content_hash, elem2.content_hash);

        let image_p = ImagePerceptor::new();
        let elem1 = image_p.perceive(&PerceptionInput::Image(vec![0xFF; 1024])).unwrap();
        let elem2 = image_p.perceive(&PerceptionInput::Image(vec![0x00; 1024])).unwrap();
        assert_ne!(elem1.content_hash, elem2.content_hash);
    }
}
//! 感知器模块 — 五种模态感知器的统一 trait 与实现
//!
//! 对应架构层:L2 Memory
//! 对应创新点:NMC(Native Multimodal Context,原生多模态上下文编码)
//!
//! # 设计决策(WHY)
//! - **同步 trait 而非 async**:感知器是 CPU 密集型操作(哈希、嵌入计算),
//!   不涉及 IO 等待,使用同步方法避免 async-trait 的堆分配开销。
//!   Week 7/8 接入 ort ONNX Runtime 后,若需 GPU 异步推理可再改为 async
//! - **trait Perceptor 而非 enum dispatch**:5 种感知器实现差异大(文本 vs 占位),
//!   trait 提供统一接口;调用方通过枚举分发选择具体实现,避免 `Box<dyn Trait>`
//!
//! # 感知器清单
//! | 感知器 | 模态 | 状态 | 说明 |
//! |--------|------|------|------|
//! | TextPerceptor | Text | 已实现 | SHA256 + 字符频率嵌入 |
//! | ImagePerceptor | Image | 占位 | Week 7/8 接入 ort ONNX |
//! | VideoPerceptor | Video | 占位 | Week 7/8 接入 ort ONNX |
//! | AudioPerceptor | Audio | 占位 | Week 7/8 接入 ort ONNX |
//! | DesktopPerceptor | Desktop | 已实现 | 基于区域描述文本的哈希嵌入 |

use crate::error::NmcError;
use crate::types::{CognitiveElement, Modality, PerceptionInput};

pub mod audio;
pub mod desktop;
pub mod image;
pub mod text;
pub mod video;

pub use audio::AudioPerceptor;
pub use desktop::DesktopPerceptor;
pub use image::ImagePerceptor;
pub use text::TextPerceptor;
pub use video::VideoPerceptor;

/// 感知器 trait — 将多模态输入编码为认知元素
///
/// 每种模态有一个对应实现,`perceive` 是同步方法(CPU 密集型,无需 async)
pub trait Perceptor {
    /// 返回此感知器处理的模态
    fn modality(&self) -> Modality;

    /// 感知输入,产出认知元素(含内容哈希与嵌入向量)
    ///
    /// # 错误
    /// - `InvalidModality`:输入模态与此感知器不匹配
    /// - `EncodingFailed`:编码过程出错(如占位感知器未实现)
    fn perceive(&self, input: &PerceptionInput) -> Result<CognitiveElement, NmcError>;
}

/// 计算 SHA256 哈希并返回十六进制字符串
///
/// WHY 共享函数:TextPerceptor 与 DesktopPerceptor 都需要内容哈希,
/// 提取到此处消除重复实现(遵循 DRY 原则)
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    // 手动格式化为 hex,避免引入 hex crate 依赖
    result.iter().map(|b| format!("{b:02x}")).collect()
}

/// 基于字节频率生成嵌入向量
///
/// 将输入字节映射到 `dim` 个桶(取模),统计频率并归一化到 [0, 1]。
/// WHY 字节频率:简单、确定性、对任意文本(含中文/Unicode)有效,
/// Week 7/8 将替换为 ort ONNX Runtime 的语义嵌入
pub(crate) fn byte_frequency_embedding(data: &[u8], dim: usize) -> Vec<f32> {
    let mut counts = vec![0u32; dim];
    let mut total = 0u32;
    for &b in data {
        let bucket = (b as usize) % dim;
        counts[bucket] += 1;
        total += 1;
    }
    if total == 0 {
        return vec![0.0; dim];
    }
    let total_f = total as f32;
    counts.into_iter().map(|c| c as f32 / total_f).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NmcConfig;

    #[test]
    fn test_text_perceptor_modality() {
        let p = TextPerceptor::new(NmcConfig::default());
        assert_eq!(p.modality(), Modality::Text);
    }

    #[test]
    fn test_image_perceptor_modality() {
        let p = ImagePerceptor::new();
        assert_eq!(p.modality(), Modality::Image);
    }

    #[test]
    fn test_video_perceptor_modality() {
        let p = VideoPerceptor::new();
        assert_eq!(p.modality(), Modality::Video);
    }

    #[test]
    fn test_audio_perceptor_modality() {
        let p = AudioPerceptor::new();
        assert_eq!(p.modality(), Modality::Audio);
    }

    #[test]
    fn test_desktop_perceptor_modality() {
        let p = DesktopPerceptor::new(NmcConfig::default());
        assert_eq!(p.modality(), Modality::Desktop);
    }
}
