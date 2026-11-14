//! 同步 DB 打开的统一出口 — rusqlite 侧的 §4.4 红线 2 落点
//!
//! 对应架构层: L10 Interface（chimera-cli 组合面）
//!
//! # 为什么需要这个模块
//! `WikiStore::open` 内部是 `Connection::open` + `PRAGMA journal_mode=WAL` +
//! `CREATE TABLE/FTS5` —— 全是同步文件 IO。此前 CLI 在 async 上下文里**直接调用**它，
//! 并在 `commands/wiki.rs` 留了一句"同步但快速，故不需要 spawn_blocking"的注释：
//! 判据不是"快不快"，而是"会不会占住 runtime 工作线程"——冷启动、页面缓存未命中、
//! 磁盘竞争下同一句调用可以从 µs 级变成几十 ms，而这段时间该线程上的**所有其他任务**
//! 都被挂住（TUI tick、事件订阅、其他并发命令）。故统一到 blocking 池。
//!
//! WHY 把 seam 做成公开泛型函数而不是只在调用点写 `spawn_blocking`：测试可以喂进一个
//! "记录所在线程 + 故意慢"的闭包，直接证明生产码走的是 blocking 池；否则测到的只是
//! 测试自己抄的副本（本仓纪律：手抄夹具测不到生产码）。

use repo_wiki::WikiStore;
use std::path::PathBuf;

use crate::error::ChimeraCliError;

/// 在 blocking 线程池执行一次同步工作，返回其结果
///
/// # 参数
/// - `work`:必须是 `Send + 'static` 的同步闭包（内部不得 `.await`）
///
/// # 错误
/// blocking 任务 panic/被取消 → `EngineError`（与调用点旧行为同型，不改退出码类别）
pub async fn on_blocking_thread<T, F>(work: F) -> Result<T, ChimeraCliError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| ChimeraCliError::EngineError(format!("blocking 任务失败: {e}")))
}

/// 在 blocking 线程打开 WikiStore（连接池 + WAL + FTS5 建表）
///
/// WHY 返回 `WikiStore` 而非 `Arc<WikiStore>`：`WikiStore` 自身是 `Clone`（内部
/// `Arc<Connection>` 池），调用点沿用既有 `store.clone()` 语义，本层不做二次包装。
pub async fn open_wiki_store(db_path: PathBuf) -> Result<WikiStore, ChimeraCliError> {
    on_blocking_thread(move || {
        WikiStore::open(&db_path)
            .map_err(|e| ChimeraCliError::EngineError(format!("WikiStore 打开失败: {e}")))
    })
    .await?
}
