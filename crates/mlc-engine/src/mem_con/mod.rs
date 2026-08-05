//! MemCon 自适应控制器 — 幽灵记忆检测与策略自适应调整
//!
//! 对应架构层:L2 Memory
//! 对应任务:P2-8 MemCon 自适应控制器
//!
//! # 模块职责
//! MemCon(Memory Controller) 是 P2-8 任务的核心产出,通过主动检测
//! 幽灵记忆模式并动态调整记忆策略,解决静态稀疏掩码导致的过时事实
//! 与当前事实共召回的问题。
//!
//! # 架构概览
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                    MemConController                      │
//! │  ┌────────────────┐  ┌──────────────────────────────┐   │
//! │  │GhostMemory     │  │(调整 MemoryStrategyPolicy)    │   │
//! │  │Detector        │  │  - 检测到幽灵记忆→Aggressive  │   │
//! │  │  - 滑动窗口统计│  │  - 高频 Ghost→MinimalRecall  │   │
//! │  │  - 幽灵记忆模式│  │  - 恢复稳定→StandardTopK    │   │
//! │  │    检测        │  │  - 熔断→StandardTopK        │   │
//! │  └───────┬────────┘  └──────────┬───────────────────┘   │
//! │          │                      │                        │
//! │          ▼                      ▼                        │
//! │  ┌──────────────────────────────────────────────────┐    │
//! │  │              EventBus Integration                │    │
//! │  │  - 订阅: (被动,由 recall hook 驱动)              │    │
//! │  │  - 发布: GhostMemoryDetected                     │    │
//! │  │  - 发布: MemConStrategyAdjusted                  │    │
//! │  └──────────────────────────────────────────────────┘    │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! # 模块文件
//! - `config.rs`: MemConConfig 配置结构体
//! - `types.rs`: MemConStats、AdjustmentReason、AdjustmentOutcome 类型
//! - `detector.rs`: GhostMemoryDetector 滑动窗口检测器
//! - `controller.rs`: MemConController 自适应控制器
//!
//! # 设计原则
//! - 无锁设计在控制器内部使用 RwLock 实现内部可变性
//! - 冷却期保护避免频繁震荡
//! - 熔断机制确保系统安全
//! - C4 合规三层 fallback

pub mod config;
pub mod controller;
pub mod detector;
pub mod types;

// 重导出核心类型,简化外部导入
pub use config::MemConConfig;
pub use controller::MemConController;
pub use types::{AdjustmentOutcome, AdjustmentReason, MemConStats};
