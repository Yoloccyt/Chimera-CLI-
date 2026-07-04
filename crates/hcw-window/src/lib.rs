//! 分层上下文窗口 - 4K/32K/128K/1M 四级上下文窗口管理
//!
//! 对应架构层:L2 Memory
//! 对应创新点:HCW(Hierarchical Context Window,分层上下文窗口)
//!
//! # 核心职责
//! - 按 `complexity` 自动选择窗口层级(L0=4K/L1=32K/L2=128K/L3=1M 等效)
//! - 窗口溢出时自动升级 tier(L0->L1->L2->L3 降级链,可逆)
//! - 应用 OSA context_mask 稀疏化(仅加载活跃文件上下文)
//! - 基于重要性评分压缩上下文(0.4*时近性 + 0.3*频次 + 0.3*任务相关性)
//! - 发布 `ContextWindowSwitched`/`ContextCompressed` 事件
//! - 订阅 `OmniSparseMasksComputed` 事件(修正 V1 违规:不直接 import OSA)
//!
//! # V1 违规修正
//! 原架构:OSA(L6)直接 import HCW(L2) -> 向上依赖违规
//! 修正后:OSA 发布 `OmniSparseMasksComputed` 事件,HCW 订阅消费,
//! HCW 不持有 OSA 的引用,仅通过 EventBus 接收掩码信息(依赖铁律)
//!
//! # 1M 等效实现(架构红线)
//! L3 的 1M 等效通过"分层 + 稀疏化"实现,而非暴力加载:
//! - 实际加载容量 = l3_capacity / 8 = 128K
//! - 通过 OSA 稀疏化(8x 压缩比)跳过 87.5% 内容
//! - 实现 1M 等效,避免内存爆炸(架构红线:禁止 1M 暴力加载)
//!
//! # 快速示例
//! ```no_run
//! use hcw_window::{HcwWindow, HcwConfig, ContextEntry, WindowTier};
//! use event_bus::EventBus;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let bus = EventBus::new();
//! let window = HcwWindow::with_default_config(bus)?;
//!
//! let entry = ContextEntry::new("e-1", "file-1", "content", 100);
//! window.insert(entry).await?;
//!
//! let tier = window.select_window(0.6).await?; // 选择 L2 窗口
//! assert_eq!(tier, WindowTier::L2);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

// === 模块声明 ===
pub mod compressor;
pub mod config;
pub mod error;
pub mod selector;
pub mod types;
pub mod window;

// === 关键类型重导出,简化外部导入 ===
pub use compressor::ContextCompressor;
pub use error::HcwError;
pub use selector::WindowSelector;
pub use types::{CompressionReport, ContextEntry, HcwConfig, HcwState, WindowTier};
pub use window::HcwWindow;

/// 预导入模块 - 提供最常用类型
///
/// 使用方式:`use hcw_window::prelude::*;`
pub mod prelude {
    pub use crate::compressor::ContextCompressor;
    pub use crate::error::HcwError;
    pub use crate::selector::WindowSelector;
    pub use crate::types::{CompressionReport, ContextEntry, HcwConfig, HcwState, WindowTier};
    pub use crate::window::HcwWindow;
}
