//! 神经多模态上下文编码器 — 将多模态输入编码为统一的潜在表示
//!
//! 对应架构层:L2 Memory
//! 对应创新点:NMC(Native Multimodal Context,原生多模态上下文编码)
//! 设计来源:Minimax M3 Native Multimodal + 创新点 5(MPP 多模态感知管道)+ ADR-016
//!
//! # 核心机制(v3.0 P0-1 已完成)
//! 5 种模态感知器(文本/图像/视频/音频/桌面)→ 统一 CLV(512-dim f32)输出
//! - **TextPerceptor**:神经网络语义嵌入(v3.0) / n-gram SipHash Mock 回退
//! - **ImagePerceptor**:像素统计特征(颜色/亮度/边缘/纹理)(v2.0)
//! - **VideoPerceptor**:帧统计 + 运动特征 + 时间序列(v2.0)
//! - **AudioPerceptor**:时频统计(能量/频谱/过零率/节奏)(v2.0)
//! - **DesktopPerceptor**:基于区域描述文本的语义嵌入(v2.0)
//!
//! # 融合策略
//! - **Concat**:拼接后截断/填充到 512 维
//! - **Mean**:对齐维度后取平均,截断/填充到 512 维
//! - **Weighted**:按模态权重加权求和(归一化),截断/填充到 512 维
//!
//! # 架构红线
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect()
//! - 输出维度严格 512(与 CLV::DIMENSION 对齐)
//! - 优先 impl Trait / enum dispatch,避免 `Box<dyn Trait>`
//!
//! # 快速示例
//! ```
//! use nmc_encoder::{NmcEncoder, NmcConfig, PerceptionInput};
//! use event_bus::EventBus;
//!
//! let bus = EventBus::new();
//! let encoder = NmcEncoder::with_event_bus(NmcConfig::default(), bus).unwrap();
//! let output = encoder.perceive(PerceptionInput::Text("hello".into())).unwrap();
//! assert_eq!(output.dimension(), 512);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

// === 模块声明 ===
pub mod config;
pub mod embedding_client;
pub mod error;
pub mod fusion;
pub mod mla_compress;
pub mod perceptors;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use config::{FusionStrategy, NmcConfig};
pub use embedding_client::{EmbeddingClient, EmbeddingRequest, EmbeddingResponse};
pub use error::NmcError;
pub use fusion::{MultimodalFusionEngine, NmcEncoder};
pub use mla_compress::MlaCompressor;
pub use perceptors::{
    AudioPerceptor, DesktopPerceptor, ImagePerceptor, Perceptor, TextPerceptor, VideoPerceptor,
};
pub use types::{ClvOutput, CognitiveElement, DesktopCapture, Modality, PerceptionInput};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::config::{FusionStrategy, NmcConfig};
    pub use crate::embedding_client::{EmbeddingClient, EmbeddingRequest, EmbeddingResponse};
    pub use crate::error::NmcError;
    pub use crate::fusion::{MultimodalFusionEngine, NmcEncoder};
    pub use crate::perceptors::Perceptor;
    pub use crate::types::{
        ClvOutput, CognitiveElement, DesktopCapture, Modality, PerceptionInput,
    };
}
