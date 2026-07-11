//! 生产验证闭环 — 并行流式生成与验证的 Producer-Verifier 循环
//!
//! 对应架构层:L7 Execution
//! 对应创新点:PVL(Producer-Verifier Loop)
//!
//! ## 核心职责
//! - Producer 流式生成操作,通过 mpsc 通道发送给 Verifier
//! - Verifier 流式验证操作,发送反馈给 FeedbackChannel
//! - FeedbackChannel 实时监控拒绝率,触发 Producer 策略调整
//! - 全程无共享可变状态,通道所有权转移保证无竞态
//!
//! ## 对应尸检教训
//! Claude Code 5.4% 孤儿调用(void Promise 无 await)的根因是:
//! - 异步操作 spawn 后,JoinHandle 未被 await
//! - future 被 drop 但无运行时检测
//!
//! PVL 通过以下机制杜绝此类问题:
//! - 所有 async 操作(tx.send, rx.recv)均显式 await
//! - mpsc 通道所有权转移,无共享可变状态(无竞态)
//! - 通道关闭时显式返回错误(ChannelClosed),不静默丢弃
//!
//! ## 通道选择决策(对应 spec.md)
//! - **tokio::sync::mpsc**:多生产者单消费者,适合 Producer→Verifier 单向流
//! - **不选 broadcast**:PVL 是 1:1 的 Producer-Verifier,broadcast 适合 1:N
//! - **不选 oneshot**:PVL 需要流式多消息,oneshot 仅支持单消息
//! - **WHY 通道而非共享状态**:通道天然无竞态(消息所有权转移),共享状态需要锁
//!
//! ## 快速示例
//! ```no_run
//! use pvl_layer::{PvlConfig, Producer, Verifier, FeedbackChannel};
//! use event_bus::EventBus;
//!
//! # async fn run() {
//! let bus = EventBus::new();
//! let config = PvlConfig::default();
//! let producer = Producer::new(config.clone(), bus.clone());
//! let verifier = Verifier::new(config.clone(), bus.clone());
//! let feedback = FeedbackChannel::new(config, bus);
//!
//! let (op_tx, mut op_rx) = tokio::sync::mpsc::channel(128);
//! let (fb_tx, mut fb_rx) = tokio::sync::mpsc::channel(128);
//!
//! // 启动验证者(后台任务)
//! let verifier_handle = tokio::spawn(async move {
//!     verifier.run(&mut op_rx, &fb_tx).await
//! });
//!
//! // 生产操作
//! producer.produce("quest-1", 10, &op_tx).await.unwrap();
//! drop(op_tx);
//!
//! // 处理反馈
//! while let Some(fb) = fb_rx.recv().await {
//!     if feedback.process_feedback(fb) {
//!         feedback.check_and_adjust_strategy(&producer).ok();
//!     }
//! }
//!
//! verifier_handle.await.unwrap().unwrap();
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod attention;
pub mod chunked_producer;
pub mod config;
pub mod error;
pub mod feedback;
pub mod priority_scheduler;
pub mod producer;
pub mod types;
pub mod verifier;

// === 关键类型重导出,简化外部导入 ===
pub use config::PvlConfig;
pub use error::PvlError;
pub use feedback::FeedbackChannel;
pub use producer::Producer;
pub use types::{
    FeedbackMessage, Operation, OperationId, OperationStatus, ProducerStrategy, VerificationResult,
};
pub use verifier::Verifier;

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::config::PvlConfig;
    pub use crate::error::PvlError;
    pub use crate::feedback::FeedbackChannel;
    pub use crate::producer::Producer;
    pub use crate::types::{
        FeedbackMessage, Operation, OperationId, OperationStatus, ProducerStrategy,
        VerificationResult,
    };
    pub use crate::verifier::Verifier;
}
