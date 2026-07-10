//! 多步预测执行 — 多 Token 预测的执行器,加速推理吞吐
//!
//! 对应架构层:L7 Execution
//! 对应创新点:MTPE(Multi-Token Prediction Execution)
//!
//! # 核心职责
//! - 一次推理预测 N 个 token(N ∈ [1, 10]),减少推理调用次数
//! - 按 N 值分组统计成功率,支持动态 N 值选择
//! - 失败步回退到单步预测,减少计算浪费
//! - 通过 EventBus 发布预测/统计/回退事件
//!
//! # 快速示例
//! ```no_run
//! use mtpe_executor::{MtpeExecutor, MtpeConfig, PredictionContext};
//! use event_bus::EventBus;
//!
//! # async fn run() {
//! let bus = EventBus::new();
//! let executor = MtpeExecutor::new(MtpeConfig::default(), bus);
//! let ctx = PredictionContext {
//!     quest_id: "q-1".into(),
//!     history: vec!["hello".into()],
//!     clv: vec![0.1; 512],
//! };
//! let result = executor.predict(&ctx, 5).await.unwrap();
//! assert_eq!(result.n, 5);
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod config;
pub mod error;
pub mod fallback;
pub mod inference_client;
pub mod predictor;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use config::MtpeConfig;
pub use error::MtpeError;
pub use inference_client::{InferenceClient, InferenceRequest, InferenceResponse, InferenceToken, to_mtpe_tokens};
pub use predictor::MtpeExecutor;
pub use types::{PredictionContext, PredictionResult, PredictionStats, Token};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::config::MtpeConfig;
    pub use crate::error::MtpeError;
    pub use crate::inference_client::{InferenceClient, InferenceRequest, InferenceResponse, InferenceToken};
    pub use crate::predictor::MtpeExecutor;
    pub use crate::types::{PredictionContext, PredictionResult, PredictionStats, Token};
}
