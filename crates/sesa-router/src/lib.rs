//! SESA 子专家稀疏激活 — 对专家子集进行稀疏化激活以降低计算开销
//!
//! 对应架构层:L6 Router
//! 对应创新点:SESA(Sub-Expert Sparse Activation)
//!
//! # 核心机制
//! - **256-bit 位向量掩码**:`SesaMask` 用 32 字节位向量表示最多 256 个专家的激活状态
//! - **分层 1024 专家掩码(P1-8)**:`HierarchicalSesaMask` 用 4 层 × 256 位支持 1024 专家
//! - **O(n) Top-K 选择**:使用 `select_nth_unstable_by` 选 Top-K 专家,避免 O(n log n) 排序
//! - **稀疏度强制 < 40%**:`enforce_sparsity` 确保激活专家数不超过总专家数的 40%
//! - **EventBus 集成**:激活完成发布 `SesaActivationCompleted`,订阅 `ConsensusReached` 调整策略
//!
//! # 架构红线
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect() 在非测试代码
//! - 所有 async fn 满足 Send 约束
//! - 持锁状态下不可 await,避免死锁
//! - `#![forbid(unsafe_code)]`:popcount 用 `u8::count_ones` 内建(SIMD 友好)
//!
//! # 快速示例
//! ```no_run
//! use sesa_router::{SesaRouter, SesaConfig, ActivationRequest, ExpertDescriptor};
//! use event_bus::EventBus;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let bus = EventBus::new();
//! let router = SesaRouter::with_event_bus(SesaConfig::default(), bus);
//!
//! // 注册专家(每个专家携带语义向量)
//! for i in 0..100 {
//!     let expert = ExpertDescriptor::new(format!("expert-{i}"), vec![0.1 * i as f32; 64]);
//!     router.register_expert(expert)?;
//! }
//!
//! // 激活 Top-8 专家(从 100 中选 8,稀疏度 8%)
//! let request = ActivationRequest::new("req-1", vec![0.5; 64], 8, 5);
//! let (mask, profile) = router.activate(request).await?;
//! assert!(profile.sparsity_ratio < 0.4, "稀疏度必须 < 40%");
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

// === 模块声明 ===
pub mod activation;
pub mod config;
pub mod error;
pub mod mask;
pub mod prerequisite;
pub mod sparsity;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use activation::SesaRouter;
pub use config::SesaConfig;
pub use error::SesaError;
pub use mask::{HierarchicalSesaMask, SesaMask, EXPERTS_PER_LAYER, LAYER_COUNT, TOTAL_EXPERTS};
pub use prerequisite::PrerequisiteChecker;
pub use sparsity::{enforce_sparsity, max_allowed_active, SparsityProfile};
pub use types::{ActivationRequest, ExpertDescriptor};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::activation::SesaRouter;
    pub use crate::config::SesaConfig;
    pub use crate::error::SesaError;
    pub use crate::mask::{
        HierarchicalSesaMask, SesaMask, EXPERTS_PER_LAYER, LAYER_COUNT, TOTAL_EXPERTS,
    };
    pub use crate::prerequisite::PrerequisiteChecker;
    pub use crate::sparsity::{enforce_sparsity, max_allowed_active, SparsityProfile};
    pub use crate::types::{ActivationRequest, ExpertDescriptor};
}
