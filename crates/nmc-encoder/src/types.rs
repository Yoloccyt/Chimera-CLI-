//! NMC 核心领域类型 — 多模态感知输入与认知元素的统一数据模型
//!
//! 对应架构层:L2 Memory
//! 对应创新点:NMC(Native Multimodal Context,原生多模态上下文编码)
//!
//! # 类型职责
//! - `Modality`:五种模态标识(Text/Image/Video/Audio/Desktop)
//! - `PerceptionInput`:多模态输入的统一枚举,感知器据此分发
//! - `DesktopCapture`:桌面捕获描述(宽高 + 区域描述文本)
//! - `CognitiveElement`:感知器输出,携带模态、内容哈希与嵌入向量
//! - `ClvOutput`:融合引擎输出,包装 `nexus_core::CLV`,保证严格 512 维
//!
//! # 设计决策(WHY)
//! - **ClvOutput newtype**:而非直接返回 CLV,在类型层面区分"原始 CLV"与
//!   "NMC 编码产物",防止调用方误用未编码的 CLV;同时保证维度严格 512
//!   (CLV 内部已做维度校验,newtype 不可绕过)
//! - **PerceptionInput 枚举**:而非 trait object,因为输入类型有限且固定,
//!   枚举分发比 `Box<dyn Trait>` 零成本且类型安全(遵循项目规范:避免 `Box<dyn Trait>`)
//! - **Image/Video/Audio 用 `Vec<u8>`**:原始字节而非解码后的结构体,
//!   本周占位不处理,Week 7/8 接入 ort ONNX Runtime 时再解码

use nexus_core::CLV;
use serde::{Deserialize, Serialize};

use crate::error::NmcError;

/// 模态标识 — 五种感知模态
///
/// 对应 5 种感知器:TextPerceptor / ImagePerceptor / VideoPerceptor /
/// AudioPerceptor / DesktopPerceptor
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Modality {
    /// 文本模态
    Text,
    /// 图像模态
    Image,
    /// 视频模态
    Video,
    /// 音频模态
    Audio,
    /// 桌面捕获模态
    Desktop,
}

impl Modality {
    /// 返回模态名称字符串(用于事件发布与日志)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Desktop => "Desktop",
        }
    }
}

impl std::fmt::Display for Modality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 桌面捕获描述 — 桌面模态的结构化输入
///
/// WHY:桌面模态不传递原始像素(数据量过大),而是传递区域描述文本,
/// 感知器基于描述文本生成嵌入。Week 7/8 可扩展为传递截图字节
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopCapture {
    /// 屏幕宽度(像素)
    pub width: u32,
    /// 屏幕高度(像素)
    pub height: u32,
    /// 区域描述文本(如 "top-left quadrant: code editor with Rust syntax")
    pub region_description: String,
}

impl DesktopCapture {
    /// 创建桌面捕获描述
    pub fn new(width: u32, height: u32, region_description: impl Into<String>) -> Self {
        Self {
            width,
            height,
            region_description: region_description.into(),
        }
    }
}

/// 多模态感知输入 — 感知器分发的统一入口
///
/// 每个变体对应一种模态,`modality()` 方法返回对应的 `Modality` 标识
#[derive(Debug, Clone, PartialEq)]
pub enum PerceptionInput {
    /// 文本输入(UTF-8 字符串)
    Text(String),
    /// 图像输入(原始字节,本周占位不处理)
    Image(Vec<u8>),
    /// 视频输入(原始字节,本周占位不处理)
    Video(Vec<u8>),
    /// 音频输入(原始字节,本周占位不处理)
    Audio(Vec<u8>),
    /// 桌面捕获输入(结构化描述)
    Desktop(DesktopCapture),
}

impl PerceptionInput {
    /// 返回输入对应的模态标识
    pub fn modality(&self) -> Modality {
        match self {
            Self::Text(_) => Modality::Text,
            Self::Image(_) => Modality::Image,
            Self::Video(_) => Modality::Video,
            Self::Audio(_) => Modality::Audio,
            Self::Desktop(_) => Modality::Desktop,
        }
    }
}

/// 认知元素 — 感知器的输出,携带模态、内容哈希与嵌入向量
///
/// WHY:`embedding` 为 `Vec<f32>` 而非 CLV:感知器输出的中间维度
/// (如 TextPerceptor 输出 256 维)与最终 CLV(512 维)不同,
/// 融合引擎负责将多个 CognitiveElement 的 embedding 聚合为 512 维 CLV
#[derive(Debug, Clone, PartialEq)]
pub struct CognitiveElement {
    /// 产生此元素的模态
    pub modality: Modality,
    /// 内容哈希(SHA256 hex),用于去重与事件引用
    pub content_hash: String,
    /// 嵌入向量(维度由感知器决定,如文本为 text_dim 维)
    pub embedding: Vec<f32>,
}

impl CognitiveElement {
    /// 创建认知元素
    pub fn new(modality: Modality, content_hash: String, embedding: Vec<f32>) -> Self {
        Self {
            modality,
            content_hash,
            embedding,
        }
    }

    /// 返回嵌入向量维度
    pub fn embedding_dim(&self) -> usize {
        self.embedding.len()
    }
}

/// CLV 输出 — 融合引擎的产物,包装 `nexus_core::CLV`
///
/// WHY newtype:在类型层面区分"NMC 编码产物"与"原始 CLV",
/// 防止调用方误用未编码的 CLV。维度严格为 512(CLV 内部已校验)
#[derive(Debug, Clone, PartialEq)]
pub struct ClvOutput(CLV);

impl ClvOutput {
    /// 从 CLV 构造 ClvOutput(维度必须为 512,由 CLV::from_vec 保证)
    pub fn new(clv: CLV) -> Self {
        Self(clv)
    }

    /// 构造零向量输出(空输入场景)
    pub fn zero() -> Self {
        Self(CLV::zero())
    }

    /// 从 `Vec<f32>` 构造,维度必须为 512
    pub fn from_vec(v: Vec<f32>) -> Result<Self, NmcError> {
        Ok(Self(CLV::from_vec(v)?))
    }

    /// 访问内部 CLV 引用
    pub fn as_inner(&self) -> &CLV {
        &self.0
    }

    /// 消费 self,返回内部 CLV
    pub fn into_inner(self) -> CLV {
        self.0
    }

    /// 返回维度(始终为 512)
    pub fn dimension(&self) -> usize {
        self.0.as_slice().len()
    }

    /// 只读访问内部 f32 切片
    pub fn as_slice(&self) -> &[f32] {
        self.0.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modality_as_str() {
        assert_eq!(Modality::Text.as_str(), "Text");
        assert_eq!(Modality::Image.as_str(), "Image");
        assert_eq!(Modality::Video.as_str(), "Video");
        assert_eq!(Modality::Audio.as_str(), "Audio");
        assert_eq!(Modality::Desktop.as_str(), "Desktop");
    }

    #[test]
    fn test_modality_display() {
        assert_eq!(format!("{}", Modality::Text), "Text");
        assert_eq!(format!("{}", Modality::Desktop), "Desktop");
    }

    #[test]
    fn test_perception_input_modality() {
        assert_eq!(
            PerceptionInput::Text("hi".into()).modality(),
            Modality::Text
        );
        assert_eq!(
            PerceptionInput::Image(vec![1, 2]).modality(),
            Modality::Image
        );
        assert_eq!(
            PerceptionInput::Desktop(DesktopCapture::new(1920, 1080, "desc")).modality(),
            Modality::Desktop
        );
    }

    #[test]
    fn test_desktop_capture_new() {
        let cap = DesktopCapture::new(1920, 1080, "full screen");
        assert_eq!(cap.width, 1920);
        assert_eq!(cap.height, 1080);
        assert_eq!(cap.region_description, "full screen");
    }

    #[test]
    fn test_cognitive_element_new() {
        let elem = CognitiveElement::new(Modality::Text, "abc123".into(), vec![0.1, 0.2, 0.3]);
        assert_eq!(elem.modality, Modality::Text);
        assert_eq!(elem.content_hash, "abc123");
        assert_eq!(elem.embedding_dim(), 3);
    }

    #[test]
    fn test_clv_output_zero() {
        let out = ClvOutput::zero();
        assert_eq!(out.dimension(), CLV::DIMENSION);
        assert!(out.as_slice().iter().all(|&v| v == 0.0));
    }

    #[test]
    fn test_clv_output_from_vec_valid() {
        let v = vec![0.5_f32; CLV::DIMENSION];
        let out = ClvOutput::from_vec(v).unwrap();
        assert_eq!(out.dimension(), 512);
        assert!(out.as_slice().iter().all(|&v| v == 0.5));
    }

    #[test]
    fn test_clv_output_from_vec_invalid() {
        let v = vec![0.0_f32; 256];
        let result = ClvOutput::from_vec(v);
        assert!(matches!(result, Err(NmcError::DimensionMismatch { .. })));
    }

    #[test]
    fn test_clv_output_into_inner() {
        let v = vec![1.0_f32; CLV::DIMENSION];
        let out = ClvOutput::from_vec(v).unwrap();
        let clv = out.into_inner();
        assert_eq!(clv.as_slice().len(), 512);
    }
}
