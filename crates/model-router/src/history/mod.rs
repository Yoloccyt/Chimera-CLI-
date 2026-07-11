//! 历史路由存储模块 — `HistoryStore` trait + 内存/SQLite 双实现
//!
//! 对应架构层:L1 Core(model-router)
//! 对应创新点:无(基础设施,为 v1.4.0 M2 RL 路由提供持久化基础)
//!
//! # 设计动机
//! v1.3.0 引入 `HistoryStore` trait + `InMemoryHistoryStore`(DashMap 并发安全),
//! 为 MoE 五维门控评分提供 success_rate / latency_variance 运行时统计。但内存实现
//! 在进程重启后丢失历史,导致统计样本重新累积,延迟 M2 RL 路由触发条件
//! (历史数据 > 10000 条)的达成。
//!
//! v1.4.0 P1 新增 `SqliteHistoryStore` 持久化实现,将历史数据写入 SQLite 文件,
//! 跨重启保留。默认仍使用 Memory(向后兼容 v1.3.0),SQLite 为 opt-in
//! (`RouterConfig.history_persistence = HistoryPersistence::Sqlite`)。
//!
//! # 模块组织
//! - `mod.rs`(本文件):`HistoryStore` trait + 常量定义(权威源)
//! - `memory.rs`:`InMemoryHistoryStore`(DashMap,v1.3.0 行为不变)
//! - `sqlite.rs`:`SqliteHistoryStore`(v1.4.0 新增,Mutex<Connection> + MessagePack)
//!
//! # 关键设计决策
//! - **trait 保持同步**:`HistoryStore` 是 `&self` 方法(非 async),`SqliteHistoryStore`
//!   的 get/record 也是同步的。SQLite 单行 UPSERT/SELECT 是微秒级操作,在同步上下文
//!   中调用可接受。调用方在 async 上下文中调用时,需用 `tokio::task::spawn_blocking`
//!   包装整个 gate() 调用(§4.4 #7 fire-and-forget 评估框架)。
//! - **常量权威源**:`HISTORY_SUFFICIENT_THRESHOLD` / `LATENCY_WINDOW_CAPACITY` 迁移到
//!   本模块作为权威定义源,moe.rs 的 `HistoryRecord` 通过 `use` 引用(消费者)。
//! - **双向 `use` 合法性**:同 crate 内模块双向 `use` 不是循环依赖(编译时所有模块
//!   符号可见),`history` 引用 `moe::HistoryRecord`,`moe` 引用 `history` 的常量与 trait。

#![forbid(unsafe_code)]

pub mod memory;
pub mod sqlite;

pub use memory::InMemoryHistoryStore;
pub use sqlite::SqliteHistoryStore;

// WHY `pub use`:从 moe 模块重导出 HistoryRecord,使 `crate::history::HistoryRecord`
// 路径有效(供子模块 memory.rs / sqlite.rs 与外部测试使用)。HistoryRecord 的
// 权威定义仍在 moe.rs(与 MoeGate 内聚),history 模块只是重导出便于子模块访问。
pub use crate::moe::HistoryRecord;

/// 历史数据充分性阈值 — 样本数达到此值时启用五维评分
///
/// WHY 100:统计显著性最小样本数。success_rate 在 < 100 样本时 95% CI
/// 过宽(如 50 样本 → ±0.14),variance 估计同样不稳定。100 样本下
/// 95% CI 收窄至 ±0.10,可接受作为门控排名微调输入。低于此值降级三维。
pub const HISTORY_SUFFICIENT_THRESHOLD: u64 = 100;

/// 延迟样本滑动窗口容量 — 保留最近 N 次延迟用于方差估计
///
/// WHY 100:滑动窗口平衡内存(每模型约 400B = 100 × 4B f32)与时效性。
/// 100 个样本足够计算稳定方差(无偏估计需 ≥ 2,但稳定性随 n 增长),
/// 同时丢弃过旧样本使方差反映"近期"抖动而非全历史均值。
///
/// SQLite 实现一致性:`SqliteHistoryStore` 的滑动窗口与 `InMemoryHistoryStore`
/// 使用相同容量,保证两种实现的 `latency_samples` 语义等价(交叉验证见
/// `tests/history_test.rs::test_memory_and_sqlite_behavior_equivalence`)。
pub const LATENCY_WINDOW_CAPACITY: usize = 100;

/// 模型历史路由存储 trait(抽象,允许内存/持久化实现)
///
/// WHY trait 抽象:为 v1.4.0 RL 路由(M2)预留扩展点 — 内存实现适合
/// 短生命周期进程,SQLite 持久化实现适合长周期统计累积(M2 触发条件
/// 需 > 10000 条历史)。`MoeGate::gate()` 对实现透明,无需修改。
///
/// # 对象安全(Object Safety)
/// trait 可作为 `&dyn HistoryStore` 使用:
/// - 所有方法取 `&self`(无 `&mut self`、无 `Self` 返回)
/// - 无泛型参数
/// - 返回 owned HistoryRecord(避免 DashMap Ref guard 生命周期约束,
///   也避免 SQLite Connection 锁 guard 跨作用域问题)
///
/// # 同步语义(重要)
/// trait 方法是同步的(`&self` 而非 async),原因:
/// - `MoeGate::gate()` 是同步方法(路由热路径,不引入 async 开销)
/// - SQLite 单行操作(SELECT/UPSERT)是微秒级,同步调用可接受
/// - 调用方在 async 上下文中调用 `gate()` 时,需用 `spawn_blocking` 包装
///   整个 `gate()` 调用(而非单个 record/get),避免阻塞 runtime
pub trait HistoryStore: Send + Sync {
    /// 查询指定模型的历史记录(返回 owned clone)
    ///
    /// 返回 None 表示该模型无历史(降级三维处理)。
    fn get(&self, model_id: &str) -> Option<HistoryRecord>;

    /// 记录一次路由结果(latency_ms + success)
    ///
    /// 语义:`total_count += 1`,`success_count += 1`(if success),
    /// `latency_samples` 滑动窗口(满则淘汰最旧,保持容量 `LATENCY_WINDOW_CAPACITY`)。
    fn record(&self, model_id: &str, latency_ms: f32, success: bool);

    /// 查询指定模型的延迟方差(带缓存)
    ///
    /// 返回 None 表示该模型无历史记录。有记录时返回 `Some(variance)`。
    ///
    /// # 缓存语义(v1.5.0-omega)
    /// - 默认实现:调用 `get()` + `HistoryRecord::latency_variance()`,clone 上计算
    /// - `InMemoryHistoryStore` override:操作 DashMap 内的 stored record,
    ///   缓存跨 `get()` clone 持久化,显著降低 `gate()` 热路径延迟
    /// - `record()` 调用后缓存自动失效(由 `HistoryRecord::record()` 处理)
    ///
    /// WHY trait 方法而非直接调 `get()` + `latency_variance()`:trait 方法允许
    /// `InMemoryHistoryStore` 在 stored record 上计算(缓存持久),而非在 clone 上
    /// 计算(缓存随 clone drop 丢失)。`gate()` 通过 `&dyn HistoryStore` 调用此方法。
    fn latency_variance(&self, model_id: &str) -> Option<f32> {
        self.get(model_id).map(|r| r.latency_variance())
    }
}
