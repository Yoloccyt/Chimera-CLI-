//! Agent 记忆三级归档(1mo / 3mo / 6mo)— Task 17 §17 实现
//!
//! 架构层归属: L9 Quest(chimera-mas 内部子模块)
//! 核心职责: 按 1 月 / 3 月 / 6 月三级归档 Agent 记忆,防止长期运行记忆膨胀
//!
//! ## 设计来源
//!
//! 基于 `CHIMERA_MULTI_AGENT_四象限协同工作系统_系统性设计文档.md` §17,
//! 经 ADR-026 决策 4/5 规避(sqlite-vec / Kuzu / LanceDB / Cognee 均不引入),
//! 复用 mlc-engine(L2 Memory)+ cmt-tiering(L3 Storage)+ scc-cache(L3 Storage)
//! 三层存储能力,不新建 memory/ 子模块。
//!
//! ## 三级归档映射(§17.2)
//!
//! | 归档级 | 触发(时间 + 容量混合)        | 压缩策略                          | 存储层         | 保留/降级阈值          |
//! |--------|-------------------------------|-----------------------------------|---------------|------------------------|
//! | 1 月   | `0 2 1 * *` 或 条目 > 10000   | HCW 摘要 ≤500 tok(权重 0.4/0.3/0.3) | CMT Warm(4096) | priority < 0.1 降级    |
//! | 3 月   | `0 3 1 1,4,7,10 *`            | 关系抽取 → mlc L2 语义(512-dim CLV) | CMT Cold(65536)| 衰减 τ=24h             |
//! | 6 月   | `0 4 1 1,7 *`                 | 深度压缩 + 模式抽取                | CMT Ice(∞)     | KeepForever(只读)      |
//!
//! ## 触发机制降级适配(§17.3)
//!
//! - v5.0.0 CronScheduler → 事件驱动 + efficiency-monitor 触发(不新建调度 crate)
//! - rusqlite 归档操作必须 `spawn_blocking`(§6.2 红线)
//!
//! ## 膨胀防护与不变量(§17.5)
//!
//! - Hot 256 条 LRU + Warm 空闲降级 + Cold 衰减降级 → 记忆常驻内存恒有界
//! - **INV-8(归档单调性)**:记忆只能沿 Hot→Warm→Cold→Ice 单向降级归档,
//!   不可反向膨胀;6 月级 KeepForever 且关键决策不压缩(防幽灵记忆)
//!
//! ## 红线对齐
//!
//! - §2.2: L9→L2/L3 向下依赖允许(mlc-engine / cmt-tiering / scc-cache)
//! - §4.1: 库层 thiserror,无 unwrap/expect
//! - §4.4 反模式 6: f32 禁止隐式转 f64,全程 f64(降级判定)
//! - §6.1: 单函数 ≤ 200 行
//! - §6.2: rusqlite 必须 spawn_blocking(本模块当前无 rusqlite 直接调用,
//!   复用 cmt-tiering 已包装的 API)
//! - ADR-026 决策 4/5: 不引入 sqlite-vec / Kuzu / LanceDB / Cognee
//! - `#![forbid(unsafe_code)]`: crate 级已在 lib.rs 声明,本模块无需重复

/// 归档压缩器与降级判定纯函数(compute_priority / should_demote_metadata)
pub mod compressor;
/// 归档调度器(事件驱动)+ cron / τ 常量
pub mod scheduler;
/// 归档层级与压缩策略定义(ArchiveScheduleLevel / CompressionStrategy)
pub mod tier;

// ============================================================
// 公共 API 重导出(简化外部导入路径)
// ============================================================

// 来自 tier.rs — 归档层级与压缩策略类型
pub use tier::{
    ArchiveOperation, ArchiveScheduleLevel, ArchiveTriggerCondition, CompressionStrategy,
    CRON_MONTH1, CRON_MONTH3, CRON_MONTH6, HCW_SUMMARY_WEIGHTS, MAX_HCW_SUMMARY_TOKENS,
};

// 来自 compressor.rs — 压缩器与降级判定纯函数
pub use compressor::{
    compute_priority, should_demote_metadata, ArchiveCompressor, CompressedContent,
    CompressionMetadata, DEMOTION_THRESHOLD_F64,
};

// 来自 scheduler.rs — 归档调度器与 τ 常量
pub use scheduler::{ArchiveScheduler, TAU_1MO_SECONDS, TAU_3MO_SECONDS, TAU_6MO_SECONDS};
