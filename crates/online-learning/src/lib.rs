//! 在线学习框架 — 统一参数在线学习与自适应优化
//!
//! 对应架构层:L1 Core (跨层通用框架)
//! 对应创新点:P2-1 全crate算法参数在线学习
//!
//! # 核心机制
//! - `LearnableParameter`:可学习参数的抽象,支持标量/向量/矩阵参数
//! - `OnlineLearner`:基于反馈信号的参数更新器(梯度下降/指数加权移动平均)
//! - `ParameterRegistry`:全局参数注册表,所有crate通过此注册表注册可学习参数
//! - `FeedbackSignal`:反馈信号类型(成功/失败/延迟/资源消耗)
//!
//! # 设计决策(WHY)
//! - **统一注册表**:所有crate的参数集中管理,避免各crate各自实现学习逻辑
//! - **反馈驱动**:参数更新由运行时反馈触发(如Quest成功率、延迟、资源消耗)
//! - **持久化**:参数状态定期序列化到JSON,重启后可恢复
//! - **线程安全**:基于DashMap实现并发安全的参数读写

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod error;
pub mod learner;
pub mod registry;
pub mod types;

pub use error::LearningError;
pub use learner::{GradientDescent, OnlineLearner};
pub use registry::ParameterRegistry;
pub use types::{FeedbackSignal, LearnableParameter, ParameterValue};
