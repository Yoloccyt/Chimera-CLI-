//! Wiki 存储层 — SQLite 持久化与结构化检索
//!
//! 对应架构层:L5 Knowledge
//!
//! # 设计要点
//! - **写线程分离**:所有写操作(insert/delete/anchor 变更)通过 `mpsc` 发送到
//!   专用 `std::thread` 持有的单一 `Connection`,由该线程串行执行。
//!   这样消除了写操作之间的锁竞争,并保证写顺序。
//! - **读连接池并发**:配置 `read_pool_size` 个独立只读 `Connection`,用
//!   `AtomicUsize` round-robin 选取,再在 `spawn_blocking` 中上锁查询。
//!   配合 WAL 模式,读操作可与写操作真正并发。
//! - **spawn_blocking 包装所有 SQLite 读操作**(C-01 修复):
//!   SQLite 查询是同步阻塞 I/O,直接在 async 上下文中调用会阻塞 Tokio
//!   runtime 的工作线程。`spawn_blocking` 将阻塞操作转移到专用阻塞线程池,
//!   async runtime 可继续调度其他任务。
//! - WAL 模式:提升并发读写性能,适合"读多写少"的 Wiki 场景。
//! - tags 存储:JSON 字符串序列化,读取时反序列化。
//! - embedding 存储:BLOB(小端序 f32),读取时反序列化。
//! - 时间戳:ISO 8601 字符串,可读且可排序。
//!
//! # Schema
//! ```sql
//! CREATE TABLE IF NOT EXISTS entries (
//!     entry_id   TEXT PRIMARY KEY,
//!     title      TEXT NOT NULL,
//!     content    TEXT NOT NULL,
//!     tags       TEXT NOT NULL,       -- JSON 数组
//!     embedding  BLOB NOT NULL,       -- 512 * 4 字节(f32 LE)
//!     created_at TEXT NOT NULL,       -- ISO 8601
//!     updated_at TEXT NOT NULL
//! );
//! ```

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use tokio::sync::oneshot;
use tokio::task::spawn_blocking;
use uuid::Uuid;

use crate::error::WikiError;
use crate::fts::{self, FtsCapability};
use crate::iscm::{IscmAnchor, Layer};
use crate::metrics::WikiMetrics;
use crate::types::{WikiConfig, WikiEntry};

/// 写入线程接收的操作。
///
/// 每个变体携带 `oneshot::Sender`,写入线程执行完毕后通过它返回结果。
/// 这种"命令 + 响应"模式让 async 调用方既能 await 结果,
/// 又不必在写入线程里处理 async runtime。
enum WriteOp {
    /// 插入或替换 Wiki 条目
    Insert(WikiEntry, oneshot::Sender<Result<(), WikiError>>),
    /// 删除条目并联动标记悬空锚点
    Delete(String, oneshot::Sender<Result<(), WikiError>>),
    /// 创建 ISCM 锚点
    CreateAnchor {
        anchor: IscmAnchor,
        respond: oneshot::Sender<Result<(), WikiError>>,
    },
    /// 解析锚点并返回对应条目
    ResolveAnchor(Uuid, oneshot::Sender<Result<WikiEntry, WikiError>>),
    /// 将锚点标记为悬空
    MarkDangling(Uuid, oneshot::Sender<Result<(), WikiError>>),
}

/// Wiki 存储器 — 封装 SQLite Connection,提供线程安全的条目 CRUD
///
/// 写操作通过专用写入线程序列化,读操作通过连接池并发执行。
/// `Clone` 共享同一个写入线程和读连接池,而不是创建新连接。
///
/// # 线程安全
/// - 写入线程:`std::thread` 持有单一 `Connection`,通过 `mpsc` 接收写命令。
/// - 读连接池:`Arc<Vec<Mutex<Connection>>>` 支持跨任务共享,
///   每个查询在 `spawn_blocking` 中短暂持锁。
/// - 所有 async fn 满足 `Send + 'static` 约束(架构红线)。
pub struct WikiStore {
    /// 写入命令发送端
    write_tx: mpsc::Sender<WriteOp>,
    /// 写入线程句柄 — 仅在最后一个 Clone 被 Drop 时才 join,
    /// 确保 SQLite 连接在线程退出前关闭,避免 Windows 上临时目录清理失败。
    writer_handle: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    /// 只读连接池
    read_conns: Arc<Vec<Mutex<Connection>>>,
    /// 下一个使用的读连接索引(round-robin)
    next_reader: AtomicUsize,
    /// 存储配置(运行时只读)
    config: WikiConfig,
    /// FTS5 全文索引可用性(运行时检测,决定 search_fulltext 查询路径)
    ///
    /// WHY:open 时检测一次,后续查询据此选择 MATCH 或 LIKE 路径。
    /// `Copy` 语义,clone 时复制;运行时不变。检测一次避免每次查询重复
    /// 探测虚拟表开销。若运行中 FTS5 表被外部删除,`search_fulltext` 的
    /// FTS5 路径会失败并降级 LIKE(运行时容错)。
    fts_capability: FtsCapability,
    /// Prometheus 监控指标(通过 Arc 在所有 clone 间共享)
    ///
    /// WHY Arc 共享:WikiStore::clone 共享同一写线程与读连接池,
    /// 指标也必须共享同一实例,否则不同 clone 的 gauge 值会不一致。
    metrics: Arc<WikiMetrics>,
}

impl Clone for WikiStore {
    fn clone(&self) -> Self {
        Self {
            write_tx: self.write_tx.clone(),
            writer_handle: Arc::clone(&self.writer_handle),
            read_conns: Arc::clone(&self.read_conns),
            next_reader: AtomicUsize::new(self.next_reader.load(Ordering::Relaxed)),
            config: self.config.clone(),
            fts_capability: self.fts_capability,
            metrics: Arc::clone(&self.metrics),
        }
    }
}

impl Drop for WikiStore {
    fn drop(&mut self) {
        // 关闭发送端,写入线程的 recv 会收到 Disconnected,从而退出循环。
        drop(std::mem::replace(&mut self.write_tx, mpsc::channel().0));

        // 只有当前是最后一个持有 store 的实例(包括所有 clone)时才 join 线程,
        // 避免 clone 仍存活时提前关闭底层 Connection。
        if Arc::strong_count(&self.writer_handle) == 1 {
            if let Ok(mut guard) = self.writer_handle.lock() {
                if let Some(handle) = guard.take() {
                    let _ = handle.join();
                }
            }
        }
    }
}

impl WikiStore {
    /// 使用默认配置打开/创建数据库
    ///
    /// 等价于 `open_with_config(WikiConfig { db_path, ..default() })`。
    /// 自动启用 WAL 模式并创建 `entries` 表(若不存在)。
    pub fn open(path: &Path) -> Result<Self, WikiError> {
        let config = WikiConfig {
            db_path: path.to_path_buf(),
            ..Default::default()
        };
        Self::open_with_config(config)
    }

    /// 使用指定配置打开/创建数据库
    ///
    /// 1. 打开初始化用连接,启用 WAL(若配置允许),创建 schema。
    /// 2. 启动专用写入线程,持有该初始化连接。
    /// 3. 创建 `read_pool_size` 个只读连接,供并发查询使用。
    pub fn open_with_config(config: WikiConfig) -> Result<Self, WikiError> {
        let db_path = config.db_path.to_string_lossy();

        // WHY:SQLite `:memory:` 数据库每个 Connection 是独立实例,
        // 读连接池无法看到写线程连接的数据。即使 read_pool_size=0,
        // 后续代码也会把 pool_size 提升到至少 1,导致读操作看到的是空库。
        // 因此彻底拒绝 `:memory:`,强制使用文件数据库以保证读写一致性。
        if db_path == ":memory:" {
            return Err(WikiError::DatabaseError(
                rusqlite::Error::InvalidParameterName(
                    ":memory: is not supported; use a file path".into(),
                ),
            ));
        }

        let writer_conn = Connection::open(&config.db_path)?;

        // 启用 WAL 模式(若配置允许)并应用一致的并发调优 PRAGMA。
        // WHY:WAL(Write-Ahead Logging)模式允许读写并发;
        // `synchronous=NORMAL` 在 WAL 下仍保证一致性但减少 fsync 开销;
        // `wal_autocheckpoint=1000` 防止 WAL 文件在持续写入时无限膨胀。
        if config.wal_enabled {
            writer_conn.pragma_update(None, "journal_mode", "WAL")?;
            writer_conn.pragma_update(None, "synchronous", "NORMAL")?;
            writer_conn.pragma_update(None, "wal_autocheckpoint", "1000")?;
        }

        init_schema(&writer_conn)?;

        // 检测 FTS5 可用性并初始化虚拟表(若配置启用)。
        // WHY:FTS5 通过 .cargo/config.toml [env] SQLITE_ENABLE_FTS5 = "1"
        // 在 bundled SQLite 编译时启用。此处运行时检测保证跨平台/非 bundled
        // 场景降级到 LIKE 而非硬失败。config.fts_enabled = false 可强制禁用。
        let fts_capability = if config.fts_enabled {
            fts::init_fts_table(&writer_conn)
        } else {
            FtsCapability::Unavailable
        };

        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || run_writer(writer_conn, rx, fts_capability));

        // 读连接池大小至少为 1,避免除零;默认 2 以体现并发优势。
        let pool_size = config.read_pool_size.max(1);
        let mut read_conns = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            let conn = Connection::open(&config.db_path)?;
            // 每个读连接也要显式启用 WAL 并应用相同 PRAGMA,
            // 确保读写连接看到的 journal_mode 一致,WAL 文件自动检查点行为相同。
            if config.wal_enabled {
                conn.pragma_update(None, "journal_mode", "WAL")?;
                conn.pragma_update(None, "synchronous", "NORMAL")?;
                conn.pragma_update(None, "wal_autocheckpoint", "1000")?;
            }
            read_conns.push(Mutex::new(conn));
        }

        // 初始化 Prometheus 指标:Gauge 默认值为 0(AtomicI64::default()),
        // 无需显式 set(0)。对于已有数据的数据库,调用方应在 open 后手动调用
        // refresh_metrics() 刷新到真实计数(open_with_config 是同步函数,无法调用
        // async 的 count())。
        let metrics = Arc::new(WikiMetrics::new());

        Ok(Self {
            write_tx: tx,
            writer_handle: Arc::new(Mutex::new(Some(handle))),
            read_conns: Arc::new(read_conns),
            next_reader: AtomicUsize::new(0),
            config,
            fts_capability,
            metrics,
        })
    }

    /// 返回配置的引用
    pub fn config(&self) -> &WikiConfig {
        &self.config
    }

    /// 返回 FTS5 tokenizer 能力状态 — open 时检测一次的结果(v1.3.0 三值)。
    ///
    /// 调用方可据此感知底层全文检索引擎:
    /// - `AvailableTrigram` / `AvailableUnicode61` 走 FTS5 MATCH(O(log n))
    /// - `Unavailable` 走 LIKE(O(n) 全表扫描)
    ///
    /// 通常无需调用方关心,`search_fulltext` 已内部处理三级降级链;
    /// 此方法仅供监控/测试/上层决策感知(如选择是否启用 FTS5 索引同步)。
    pub fn fts_capability(&self) -> FtsCapability {
        self.fts_capability
    }

    /// 异步查询当前 journal_mode(用于验证 WAL 是否启用)
    pub async fn journal_mode(&self) -> Result<String, WikiError> {
        self.with_read_conn(|conn| {
            let mode: String = conn.query_row("PRAGMA journal_mode;", [], |row| row.get(0))?;
            Ok(mode)
        })
        .await
    }

    /// 异步插入或更新 Wiki 条目(UPSERT 语义)
    ///
    /// 若 `entry_id` 已存在,则更新所有字段(含 `created_at` 重置);
    /// 否则插入新记录。
    ///
    /// WHY insert 后调用 refresh_metrics:保证 `wiki_entries_total` gauge
    /// 与实际数据库条目数一致。refresh 失败不阻断主操作(insert 已成功),
    /// 仅记录 warning — 指标滞后是可接受的(下次 insert/delete 会再次刷新)。
    pub async fn insert(&self, entry: WikiEntry) -> Result<(), WikiError> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteOp::Insert(entry, tx))
            .map_err(|_| WikiError::WriteChannelClosed)?;
        // WHY 双 ??:rx.await 返回 Result<Result<(), WikiError>, RecvError>。
        // 第一个 ? 展开 RecvError,第二个 ? 展开写入线程返回的 WikiError。
        // 原实现仅单 ?(作为函数最后表达式直接返回内层 Result),现在需在
        // 后续调用 refresh_metrics,必须完全展开为 ()。
        rx.await.map_err(|_| WikiError::WriteChannelClosed)??;

        // 刷新 Prometheus 指标(失败不阻断已成功的 insert)
        if let Err(e) = self.refresh_metrics().await {
            tracing::warn!(error = %e, "refresh_metrics after insert failed");
        }
        Ok(())
    }

    /// 异步按 entry_id 精确查找
    ///
    /// 返回 `None` 表示条目不存在。
    pub async fn get(&self, entry_id: String) -> Result<Option<WikiEntry>, WikiError> {
        self.with_read_conn(move |conn| {
            let result = conn
                .query_row(
                    "SELECT entry_id, title, content, tags, embedding, created_at, updated_at
                     FROM entries WHERE entry_id = ?1;",
                    params![entry_id],
                    row_to_entry,
                )
                .optional()?;
            Ok(result)
        })
        .await
    }

    /// 异步删除条目并联动标记悬空锚点
    ///
    /// 若条目不存在,返回 `Ok(())`(幂等)。
    /// 注意:此方法仅删除 SQLite 中的记录,不删除 VectorIndex 中的向量;
    /// 调用方需同步调用 `VectorIndex::delete` 保持一致性。
    ///
    /// WHY delete 后调用 refresh_metrics:与 insert 对称,保证 gauge 反映
    /// delete 后的实际条目数(条目数减少是 Gauge 而非 Counter 的关键场景)。
    pub async fn delete(&self, entry_id: String) -> Result<(), WikiError> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteOp::Delete(entry_id, tx))
            .map_err(|_| WikiError::WriteChannelClosed)?;
        // WHY 双 ??:同 insert,展开 oneshot 的 RecvError + 写入线程的 WikiError。
        rx.await.map_err(|_| WikiError::WriteChannelClosed)??;

        // 刷新 Prometheus 指标(失败不阻断已成功的 delete)
        if let Err(e) = self.refresh_metrics().await {
            tracing::warn!(error = %e, "refresh_metrics after delete failed");
        }
        Ok(())
    }

    /// 异步按 tag 过滤(精确匹配 tags JSON 数组中的某个元素)
    ///
    /// WHY:tags 存储为 JSON 数组字符串(如 `["a","b"]`),
    /// 用 `"%"tag"%"` 匹配 JSON 元素边界,避免误匹配子串。
    pub async fn list_by_tag(&self, tag: String) -> Result<Vec<WikiEntry>, WikiError> {
        self.with_read_conn(move |conn| {
            let pattern = format!("%\"{tag}\"%");
            let mut stmt = conn.prepare(
                "SELECT entry_id, title, content, tags, embedding, created_at, updated_at
                 FROM entries WHERE tags LIKE ?1;",
            )?;
            let rows = stmt.query_map(params![pattern], row_to_entry)?;
            let mut entries = Vec::new();
            for row in rows {
                entries.push(row?);
            }
            Ok(entries)
        })
        .await
    }

    /// 异步全文检索 — v1.3.0 三级降级链(trigram > unicode61 > LIKE)。
    ///
    /// # 引擎选择(根据 `FtsCapability`)
    /// - `AvailableTrigram`:CJK 三字以上子串走 trigram MATCH(直接命中);
    ///   短查询(< 3 字符)trigram 无优势,直接降级 LIKE;
    ///   MATCH 报错(特殊字符)降级 LIKE
    /// - `AvailableUnicode61`:走 unicode61 MATCH,空结果降级 LIKE
    ///   (v1.2.0 行为 — unicode61 将连续 CJK 视为单 token,子串不命中)
    /// - `Unavailable`:直接 LIKE 全表扫描(v1.2.0 行为)
    ///
    /// # WHY trigram 空结果不降级 LIKE(与 unicode61 不同)
    /// trigram 对 CJK 三字以上子串应能命中(生成对应 trigram token)。若 trigram
    /// MATCH 返回空 Vec,说明文档确实不含该子串(trigram 工作正常),返回空 Vec
    /// 是正确语义。若降级 LIKE,会引入子串匹配的"部分命中"语义(LIKE %query%
    /// 可能匹配更多结果),破坏 trigram 的精确匹配语义。
    ///
    /// 而 unicode61 对 CJK 子串有不命中问题(整体 token),空结果可能是
    /// tokenizer 局限而非真的无匹配,故需降级 LIKE 保证召回率。
    ///
    /// # WHY 短查询(< 3 字符)直接降级 LIKE
    /// trigram 按 3 字符滑窗分词,1-2 字符查询无法生成有效 trigram token,
    /// 继续走 trigram MATCH 会空结果(误判为无匹配)。直接降级 LIKE 更高效
    /// (LIKE 对短查询性能足够,子串扫描开销小),且语义更宽松(子串匹配)。
    ///
    /// # WHY 降级而非硬失败
    /// FTS5 的 MATCH 对特殊字符(如不平衡引号)会报语法错误。直接返回 Err 会
    /// 暴露底层引擎细节给调用方。降级到 LIKE 保证:即使 query 无法用 FTS5 表达,
    /// 仍能通过子串匹配召回结果(语义降级,功能不丢失)。
    pub async fn search_fulltext(&self, query: String) -> Result<Vec<WikiEntry>, WikiError> {
        let capability = self.fts_capability;
        self.with_read_conn(move |conn| {
            match capability {
                FtsCapability::AvailableTrigram => {
                    // WHY 短查询降级:trigram 按 3 字符滑窗分词,1-2 字符无法生成
                    // 有效 trigram token,MATCH 会空结果(误判无匹配)。LIKE 对短
                    // 查询性能足够,且子串匹配语义更宽松。chars().count() 按 Unicode
                    // 标量值计数,正确处理 CJK(每个汉字算 1 字符)。
                    if query.chars().count() >= 3 {
                        match fts::search_fts(conn, &query) {
                            // trigram 应直接命中(空或非空都是正确语义,不降级 LIKE)
                            Ok(entries) => return Ok(entries),
                            Err(e) => {
                                // FTS5 查询失败(常见于 query 含 FTS5 非法语法,如不平衡引号),
                                // 降级到 LIKE,记录 warning 便于排查降级频率。
                                tracing::warn!(
                                    error = %e,
                                    query = %query,
                                    "trigram MATCH failed, falling back to LIKE"
                                );
                            }
                        }
                    }
                    // 短查询(< 3 字符)或 MATCH 失败:fall through 到 LIKE
                }
                FtsCapability::AvailableUnicode61 => {
                    // v1.2.0 行为:unicode61 MATCH + 空结果降级 LIKE
                    match fts::search_fts(conn, &query) {
                        Ok(entries) if !entries.is_empty() => return Ok(entries),
                        Ok(_) => {
                            // WHY 空结果降级:FTS5 unicode61 tokenizer 将连续 CJK 字符
                            // 视为单个 token,导致中文子串检索无法 MATCH 命中
                            // (如 "分析" 不匹配 "性能分析报告" 这一整体 token)。
                            // LIKE 的 %query% 子串匹配能正确召回此类结果。
                            // 降级不影响 FTS5 在英文/分词文本上的性能优势。
                        }
                        Err(e) => {
                            // FTS5 查询失败(常见于 query 含 FTS5 非法语法,如不平衡引号),
                            // 降级到 LIKE,记录 warning 便于排查降级频率。
                            tracing::warn!(
                                error = %e,
                                query = %query,
                                "unicode61 MATCH failed, falling back to LIKE"
                            );
                        }
                    }
                }
                FtsCapability::Unavailable => {
                    // v1.2.0 行为:LIKE 全表扫描(无 FTS5 可用)
                }
            }
            fts::search_like(conn, &query)
        })
        .await
    }

    /// 异步列出所有条目(按 created_at 升序)
    pub async fn list_all(&self) -> Result<Vec<WikiEntry>, WikiError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT entry_id, title, content, tags, embedding, created_at, updated_at
                 FROM entries ORDER BY created_at ASC;",
            )?;
            let rows = stmt.query_map([], row_to_entry)?;
            let mut entries = Vec::new();
            for row in rows {
                entries.push(row?);
            }
            Ok(entries)
        })
        .await
    }

    /// 异步计算条目总数
    pub async fn count(&self) -> Result<u32, WikiError> {
        self.with_read_conn(|conn| {
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM entries;", [], |row| row.get(0))?;
            Ok(u32::try_from(count).unwrap_or(0))
        })
        .await
    }

    /// 返回 Prometheus 监控指标引用
    ///
    /// 调用方可通过 `store.metrics().entries_total.get()` 读取当前条目数,
    /// 或将 `WikiMetrics` 注册到 Prometheus Registry 供 /metrics 端点采集。
    ///
    /// WHY 返回 &WikiMetrics 而非 Arc<WikiMetrics>:避免调用方意外长期持有
    /// Arc 副本导致指标实例生命周期与 store 解耦。引用绑定到 store 生命周期,
    /// 保证指标在 store 存活期间可用。
    pub fn metrics(&self) -> &WikiMetrics {
        &self.metrics
    }

    /// 刷新 Prometheus 指标 — 调用 `count()` 后更新 `wiki_entries_total` gauge
    ///
    /// WHY 在 insert/delete 后自动调用:保证 gauge 与实际数据库条目数一致。
    /// `count()` 是 O(1) 的 `SELECT COUNT(*)`(SQLite 维护行计数,无需全表扫描),
    /// 在 `spawn_blocking` 中执行不阻塞 async runtime,性能开销可接受。
    ///
    /// 对于 `open_with_config` 打开已有数据库的场景,调用方应手动调用此方法
    /// 初始化指标(因 open_with_config 是同步函数,无法调用 async 的 count)。
    pub async fn refresh_metrics(&self) -> Result<(), WikiError> {
        let count = self.count().await?;
        self.metrics.set_entries(count);
        Ok(())
    }

    // ============================================================
    // ISCM 跨层共享锚点方法(Week 2 Task 5)
    // ============================================================

    /// 异步创建跨层共享锚点
    ///
    /// 锚点 ID 自动生成 UUIDv7(时间有序),`created_at`/`updated_at` 自动设为当前 UTC。
    /// 同一 (layer, crate_name, entity_id) 组合可创建多个锚点(不同层引用同一实体)。
    pub async fn create_anchor(
        &self,
        layer: Layer,
        crate_name: String,
        entity_id: String,
    ) -> Result<IscmAnchor, WikiError> {
        let anchor = IscmAnchor::new(layer, crate_name, entity_id);
        let anchor_for_writer = anchor.clone();
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteOp::CreateAnchor {
                anchor: anchor_for_writer,
                respond: tx,
            })
            .map_err(|_| WikiError::WriteChannelClosed)?;
        rx.await.map_err(|_| WikiError::WriteChannelClosed)??;
        Ok(anchor)
    }

    /// 异步解析锚点 — 返回指向的 Wiki 条目
    ///
    /// 解析流程:
    /// 1. 查询 anchors 表,若锚点不存在返回 `EntryNotFound`
    /// 2. 若 `is_dangling=true`,返回 `AnchorDangling`(实体已被删除)
    /// 3. 根据 `entity_id` 查询 entries 表
    /// 4. 若条目不存在,自动标记锚点为悬空并返回 `AnchorDangling`
    ///    (懒清理:发现悬空时才更新状态,避免删除时全表扫描)
    pub async fn resolve_anchor(&self, anchor_id: Uuid) -> Result<WikiEntry, WikiError> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteOp::ResolveAnchor(anchor_id, tx))
            .map_err(|_| WikiError::WriteChannelClosed)?;
        rx.await.map_err(|_| WikiError::WriteChannelClosed)?
    }

    /// 异步标记锚点为悬空(实体被删除或失效时调用)
    ///
    /// 幂等操作:对已悬空的锚点再次标记不会报错。
    pub async fn mark_dangling(&self, anchor_id: Uuid) -> Result<(), WikiError> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteOp::MarkDangling(anchor_id, tx))
            .map_err(|_| WikiError::WriteChannelClosed)?;
        rx.await.map_err(|_| WikiError::WriteChannelClosed)?
    }

    /// 异步列出指定实体的所有锚点(跨层引用查询)
    ///
    /// 用于审计:查看某知识实体被哪些层、哪些 crate 引用。
    pub async fn list_anchors_by_entity(
        &self,
        entity_id: String,
    ) -> Result<Vec<IscmAnchor>, WikiError> {
        self.with_read_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT anchor_id, layer, crate_name, entity_id, created_at, updated_at, is_dangling
                 FROM anchors WHERE entity_id = ?1 ORDER BY created_at ASC;",
            )?;
            let rows = stmt.query_map(params![entity_id], row_to_anchor)?;
            let mut anchors = Vec::new();
            for row in rows {
                anchors.push(row?);
            }
            Ok(anchors)
        })
        .await
    }

    /// 异步列出指定层的所有锚点(层内审计)
    ///
    /// 用于层内自检:查看某层引用了哪些知识实体。
    pub async fn list_anchors_by_layer(&self, layer: Layer) -> Result<Vec<IscmAnchor>, WikiError> {
        let layer_str = layer.as_str();
        self.with_read_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT anchor_id, layer, crate_name, entity_id, created_at, updated_at, is_dangling
                 FROM anchors WHERE layer = ?1 ORDER BY created_at ASC;",
            )?;
            let rows = stmt.query_map(params![layer_str], row_to_anchor)?;
            let mut anchors = Vec::new();
            for row in rows {
                anchors.push(row?);
            }
            Ok(anchors)
        })
        .await
    }

    /// 在读连接上执行查询的通用包装
    ///
    /// WHY:所有读操作共用同一模式(round-robin 选连接 + spawn_blocking + 上锁),
    /// 抽成 helper 避免重复,并确保 `MutexGuard` 不会跨越 `.await`。
    async fn with_read_conn<F, R>(&self, f: F) -> Result<R, WikiError>
    where
        F: FnOnce(&Connection) -> Result<R, WikiError> + Send + 'static,
        R: Send + 'static,
    {
        let pool = Arc::clone(&self.read_conns);
        let len = pool.len();
        let idx = self.next_reader.fetch_add(1, Ordering::Relaxed) % len;
        spawn_blocking(move || {
            let conn = pool[idx]
                .lock()
                .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;
            f(&conn)
        })
        .await
        .map_err(WikiError::BlockingJoinError)?
    }
}

/// 初始化 schema — 创建 entries 表、tags 索引与 anchors 表/索引
fn init_schema(conn: &Connection) -> Result<(), WikiError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS entries (
            entry_id   TEXT PRIMARY KEY,
            title      TEXT NOT NULL,
            content    TEXT NOT NULL,
            tags       TEXT NOT NULL,
            embedding  BLOB NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );",
    )?;

    // 创建 tags 索引(加速 list_by_tag 的 LIKE 查询)
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_entries_tags ON entries(tags);")?;

    // 创建 anchors 表(ISCM 跨层共享锚点)
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS anchors (
            anchor_id   TEXT PRIMARY KEY,
            layer       TEXT NOT NULL,
            crate_name  TEXT NOT NULL,
            entity_id   TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            is_dangling INTEGER NOT NULL DEFAULT 0
        );",
    )?;

    // 创建 anchors 索引
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_anchors_entity ON anchors(entity_id);
         CREATE INDEX IF NOT EXISTS idx_anchors_layer ON anchors(layer);",
    )?;

    Ok(())
}

/// 写入线程主循环 — 持有单一 `Connection` 串行执行写操作
///
/// `fts` 为 FTS5 可用性状态,用于决定 insert/delete 时是否同步 FTS5 索引。
/// `Copy` 语义,每次循环复制无需担心所有权。
fn run_writer(conn: Connection, rx: mpsc::Receiver<WriteOp>, fts: FtsCapability) {
    while let Ok(op) = rx.recv() {
        match op {
            WriteOp::Insert(entry, respond) => {
                let _ = respond.send(writer_insert(&conn, entry, fts));
            }
            WriteOp::Delete(entry_id, respond) => {
                let _ = respond.send(writer_delete(&conn, entry_id, fts));
            }
            WriteOp::CreateAnchor { anchor, respond } => {
                let _ = respond.send(writer_create_anchor(&conn, anchor));
            }
            WriteOp::ResolveAnchor(anchor_id, respond) => {
                let _ = respond.send(writer_resolve_anchor(&conn, anchor_id));
            }
            WriteOp::MarkDangling(anchor_id, respond) => {
                let _ = respond.send(writer_mark_dangling(&conn, anchor_id));
            }
        }
    }
}

/// 写入线程:执行 insert
///
/// `fts` 决定是否同步 FTS5 索引。FTS5 可用时,entries 写入成功后调用
/// `sync_fts_insert` 同步索引(先删后插,保证 UPSERT 不产生重复行)。
fn writer_insert(conn: &Connection, entry: WikiEntry, fts: FtsCapability) -> Result<(), WikiError> {
    let tags_json = serde_json::to_string(&entry.tags)?;
    let embedding_blob = embedding_to_blob(&entry.embedding);
    let created_iso = entry.created_at.to_rfc3339();
    let updated_iso = entry.updated_at.to_rfc3339();

    conn.execute(
        "INSERT OR REPLACE INTO entries
            (entry_id, title, content, tags, embedding, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7);",
        params![
            entry.entry_id,
            entry.title,
            entry.content,
            tags_json,
            embedding_blob,
            created_iso,
            updated_iso,
        ],
    )?;

    // FTS5 可用时同步索引(先删后插,保证 UPSERT 不产生重复行)
    if fts.is_available() {
        fts::sync_fts_insert(conn, &entry)?;
    }
    Ok(())
}

/// 写入线程:执行 delete 并联动标记悬空锚点
///
/// `fts` 决定是否同步删除 FTS5 索引。FTS5 可用时,entries 删除后调用
/// `sync_fts_delete` 同步清除索引,保持索引与数据一致。
fn writer_delete(conn: &Connection, entry_id: String, fts: FtsCapability) -> Result<(), WikiError> {
    conn.execute(
        "DELETE FROM entries WHERE entry_id = ?1;",
        params![entry_id],
    )?;

    let now_iso = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE anchors SET is_dangling = 1, updated_at = ?1 WHERE entity_id = ?2;",
        params![now_iso, entry_id],
    )?;

    // FTS5 可用时同步删除索引(entry_id 借用已释放,&String deref 到 &str)
    if fts.is_available() {
        fts::sync_fts_delete(conn, &entry_id)?;
    }
    Ok(())
}

/// 写入线程:创建锚点
fn writer_create_anchor(conn: &Connection, anchor: IscmAnchor) -> Result<(), WikiError> {
    conn.execute(
        "INSERT INTO anchors
            (anchor_id, layer, crate_name, entity_id, created_at, updated_at, is_dangling)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7);",
        params![
            anchor.anchor_id.to_string(),
            anchor.layer.as_str(),
            anchor.crate_name,
            anchor.entity_id,
            anchor.created_at.to_rfc3339(),
            anchor.updated_at.to_rfc3339(),
            anchor.is_dangling as i64,
        ],
    )?;
    Ok(())
}

/// 写入线程:解析锚点,必要时懒标记悬空
fn writer_resolve_anchor(conn: &Connection, anchor_id: Uuid) -> Result<WikiEntry, WikiError> {
    let anchor_row: Option<(String, String, i64)> = conn
        .query_row(
            "SELECT entity_id, layer, is_dangling FROM anchors WHERE anchor_id = ?1;",
            params![anchor_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;

    let (entity_id, _layer_str, is_dangling) =
        anchor_row.ok_or_else(|| WikiError::EntryNotFound(format!("anchor {anchor_id}")))?;

    if is_dangling != 0 {
        return Err(WikiError::AnchorDangling(format!(
            "anchor {anchor_id} marked dangling"
        )));
    }

    let entry_result = conn
        .query_row(
            "SELECT entry_id, title, content, tags, embedding, created_at, updated_at
             FROM entries WHERE entry_id = ?1;",
            params![entity_id],
            row_to_entry,
        )
        .optional()?;

    match entry_result {
        Some(entry) => Ok(entry),
        None => {
            let now_iso = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE anchors SET is_dangling = 1, updated_at = ?1
                 WHERE anchor_id = ?2;",
                params![now_iso, anchor_id.to_string()],
            )?;
            Err(WikiError::AnchorDangling(format!(
                "anchor {anchor_id} entity {entity_id} missing"
            )))
        }
    }
}

/// 写入线程:标记锚点悬空
fn writer_mark_dangling(conn: &Connection, anchor_id: Uuid) -> Result<(), WikiError> {
    let now_iso = Utc::now().to_rfc3339();
    let affected = conn.execute(
        "UPDATE anchors SET is_dangling = 1, updated_at = ?1 WHERE anchor_id = ?2;",
        params![now_iso, anchor_id.to_string()],
    )?;
    if affected == 0 {
        return Err(WikiError::EntryNotFound(format!("anchor {anchor_id}")));
    }
    Ok(())
}

/// 将 f32 向量序列化为小端序 BLOB
fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(embedding.len() * 4);
    for &v in embedding {
        blob.extend_from_slice(&v.to_le_bytes());
    }
    blob
}

/// 将小端序 BLOB 反序列化为 f32 向量
fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    if !blob.len().is_multiple_of(4) {
        return Vec::new();
    }
    blob.chunks_exact(4)
        .map(|chunk| {
            let bytes: [u8; 4] = chunk.try_into().unwrap_or([0; 4]);
            f32::from_le_bytes(bytes)
        })
        .collect()
}

/// 将 SQLite 行映射为 WikiEntry
///
/// `pub(crate)` 暴露给 `fts` 模块复用(FTS5 JOIN 查询与 LIKE 查询的列顺序一致),
/// 避免在 fts.rs 重复行映射逻辑。
pub(crate) fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<WikiEntry> {
    let entry_id: String = row.get(0)?;
    let title: String = row.get(1)?;
    let content: String = row.get(2)?;
    let tags_json: String = row.get(3)?;
    let embedding_blob: Vec<u8> = row.get(4)?;
    let created_iso: String = row.get(5)?;
    let updated_iso: String = row.get(6)?;

    // tags JSON 反序列化,失败时降级为空数组(不阻断查询)
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();

    // 时间戳解析,失败时降级为当前时间(不阻断查询)
    let created_at = DateTime::parse_from_rfc3339(&created_iso)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let updated_at = DateTime::parse_from_rfc3339(&updated_iso)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    Ok(WikiEntry {
        entry_id,
        title,
        content,
        tags,
        embedding: blob_to_embedding(&embedding_blob),
        created_at,
        updated_at,
    })
}

/// 将 SQLite 行映射为 IscmAnchor
///
/// 字段顺序与 `SELECT anchor_id, layer, crate_name, entity_id, created_at, updated_at, is_dangling` 对齐。
/// 时间戳与 layer 解析失败时降级处理(不阻断查询),保证审计查询的健壮性。
fn row_to_anchor(row: &rusqlite::Row<'_>) -> rusqlite::Result<IscmAnchor> {
    let anchor_id_str: String = row.get(0)?;
    let layer_str: String = row.get(1)?;
    let crate_name: String = row.get(2)?;
    let entity_id: String = row.get(3)?;
    let created_iso: String = row.get(4)?;
    let updated_iso: String = row.get(5)?;
    let is_dangling_i: i64 = row.get(6)?;

    // UUID 解析,失败时返回错误(锚点 ID 损坏属于数据完整性问题,不应静默降级)
    let anchor_id = Uuid::parse_str(&anchor_id_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;

    // layer 解析,失败时降级为 L5_Knowledge(默认知识层,保证查询不阻断)
    let layer = Layer::from_str(&layer_str).unwrap_or(Layer::L5_Knowledge);

    // 时间戳解析,失败时降级为当前时间
    let created_at = DateTime::parse_from_rfc3339(&created_iso)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let updated_at = DateTime::parse_from_rfc3339(&updated_iso)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    Ok(IscmAnchor {
        anchor_id,
        layer,
        crate_name,
        entity_id,
        created_at,
        updated_at,
        is_dangling: is_dangling_i != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_embedding_blob_roundtrip() {
        let original: Vec<f32> = (0..512).map(|i| i as f32 * 0.1).collect();
        let blob = embedding_to_blob(&original);
        assert_eq!(blob.len(), 512 * 4);
        let restored = blob_to_embedding(&blob);
        assert_eq!(restored.len(), 512);
        for (a, b) in original.iter().zip(restored.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_blob_to_embedding_invalid_length() {
        // 长度不是 4 的倍数,返回空向量(不 panic)
        let blob = vec![0u8, 1, 2];
        let result = blob_to_embedding(&blob);
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_open_and_journal_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();
        let mode = store.journal_mode().await.unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    /// 验证 `:memory:` 数据库被彻底拒绝。
    ///
    /// WHY:SQLite `:memory:` 每个 Connection 是独立实例,读连接池无法
    /// 看到写线程的数据;即使 read_pool_size=0,后续逻辑也会创建至少 1 个
    /// 读连接,导致读操作看到的是空库。彻底拒绝可避免静默的数据"丢失"。
    #[test]
    fn test_open_memory_db_rejected() {
        let config = WikiConfig {
            db_path: std::path::PathBuf::from(":memory:"),
            vector_dim: 512,
            wal_enabled: false,
            read_pool_size: 0,
            fts_enabled: false,
        };
        match WikiStore::open_with_config(config) {
            Err(err) => assert!(err.to_string().contains(":memory:")),
            Ok(_) => panic!(":memory: should be rejected; use a file path"),
        }
    }

    #[tokio::test]
    async fn test_insert_and_get() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();

        let entry = WikiEntry::new("e-1", "标题", "内容", vec!["t".into()], vec![0.5; 512]);
        store.insert(entry).await.unwrap();

        let fetched = store.get("e-1".to_string()).await.unwrap().unwrap();
        assert_eq!(fetched.entry_id, "e-1");
        assert_eq!(fetched.title, "标题");
        assert_eq!(fetched.tags, vec!["t".to_string()]);
        assert_eq!(fetched.embedding.len(), 512);
        assert!((fetched.embedding[0] - 0.5).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_get_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();
        let result = store.get("nonexistent".to_string()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();

        let entry = WikiEntry::new("e-1", "标题", "内容", vec![], vec![0.0; 512]);
        store.insert(entry).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);

        store.delete("e-1".to_string()).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);
        assert!(store.get("e-1".to_string()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_list_by_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();

        for i in 0..6 {
            let entry = WikiEntry::new(
                format!("e-{i}"),
                format!("Entry {i}"),
                "content",
                vec!["tag-0".into(), format!("tag-{i}")],
                vec![0.0; 512],
            );
            store.insert(entry).await.unwrap();
        }

        let tagged = store.list_by_tag("tag-0".to_string()).await.unwrap();
        assert_eq!(tagged.len(), 6);

        let tagged_1 = store.list_by_tag("tag-1".to_string()).await.unwrap();
        assert_eq!(tagged_1.len(), 1);
    }

    #[tokio::test]
    async fn test_search_fulltext() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();

        let entry = WikiEntry::new(
            "e-1",
            "Rust 编程",
            "Rust 是一门系统级编程语言",
            vec![],
            vec![0.0; 512],
        );
        store.insert(entry).await.unwrap();

        let found = store.search_fulltext("Rust".to_string()).await.unwrap();
        assert!(!found.is_empty());

        let not_found = store
            .search_fulltext("nonexistent".to_string())
            .await
            .unwrap();
        assert!(not_found.is_empty());
    }

    #[tokio::test]
    async fn test_count() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();

        assert_eq!(store.count().await.unwrap(), 0);
        for i in 0..5 {
            let entry = WikiEntry::new(format!("e-{i}"), "t", "c", vec![], vec![0.0; 512]);
            store.insert(entry).await.unwrap();
        }
        assert_eq!(store.count().await.unwrap(), 5);
    }

    #[tokio::test]
    async fn test_list_all() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();

        for i in 0..3 {
            let entry = WikiEntry::new(
                format!("e-{i}"),
                format!("Entry {i}"),
                "content",
                vec![],
                vec![0.0; 512],
            );
            store.insert(entry).await.unwrap();
        }

        let all = store.list_all().await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_upsert_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();

        let entry_v1 = WikiEntry::new("e-1", "v1", "c1", vec![], vec![0.0; 512]);
        store.insert(entry_v1).await.unwrap();

        let entry_v2 = WikiEntry::new("e-1", "v2", "c2", vec![], vec![0.0; 512]);
        store.insert(entry_v2).await.unwrap();

        assert_eq!(store.count().await.unwrap(), 1);
        let fetched = store.get("e-1".to_string()).await.unwrap().unwrap();
        assert_eq!(fetched.title, "v2");
    }

    /// 验证读操作可在写操作进行时并发完成,不被阻塞。
    ///
    /// WHY:旧实现使用单 `Mutex<Connection>` 串行化所有读写;
    /// 本测试一个任务持续写入,另一个任务持续读取,
    /// 若读仍被写阻塞,`timeout` 会触发。
    #[tokio::test]
    async fn test_read_during_write_not_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test_rw.db");
        let store = WikiStore::open(&db_path).unwrap();

        let writer = store.clone();
        let write_handle = tokio::spawn(async move {
            for i in 0..100 {
                let entry = WikiEntry::new(
                    format!("e-{i}"),
                    format!("Entry {i}"),
                    "content",
                    vec![],
                    vec![0.0; 512],
                );
                writer.insert(entry).await.unwrap();
            }
        });

        let reader = store.clone();
        let read_handle = tokio::spawn(async move {
            for _ in 0..50 {
                tokio::time::timeout(Duration::from_millis(500), reader.count())
                    .await
                    .expect("读取在写期间被阻塞,超时")
                    .expect("count 失败");
            }
        });

        // 两者应同时完成,读取不会因为写入而超时
        write_handle.await.unwrap();
        read_handle.await.unwrap();
    }

    /// 验证多个并发写入同一 entry_id 最终被序列化,状态一致。
    ///
    /// WHY:写入线程序列化所有写操作,UPSERT 不会产生重复记录;
    /// 本测试确保并发写不会破坏该不变量。
    #[tokio::test]
    async fn test_write_serializes() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test_serial.db");
        let store = WikiStore::open(&db_path).unwrap();

        let mut handles = Vec::new();
        for i in 0..10 {
            let store_clone = store.clone();
            handles.push(tokio::spawn(async move {
                let entry = WikiEntry::new(
                    "e-same",
                    format!("title-{i}"),
                    format!("content-{i}"),
                    vec![],
                    vec![0.0; 512],
                );
                store_clone.insert(entry).await.unwrap();
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        assert_eq!(store.count().await.unwrap(), 1);
        let fetched = store.get("e-same".to_string()).await.unwrap().unwrap();
        assert!(fetched.title.starts_with("title-"));
    }

    /// 验证 `WikiStore::clone` 共享同一个写入线程与读连接池。
    ///
    /// WHY:clone 不能创建新连接,否则跨 clone 的数据不可见且资源泄漏。
    #[tokio::test]
    async fn test_clone_shares_writer() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test_clone.db");
        let store = WikiStore::open(&db_path).unwrap();
        let cloned = store.clone();

        let entry = WikiEntry::new("e-clone", "clone-title", "content", vec![], vec![0.0; 512]);
        cloned.insert(entry).await.unwrap();

        let fetched = store.get("e-clone".to_string()).await.unwrap().unwrap();
        assert_eq!(fetched.title, "clone-title");
    }

    /// 回归测试:验证 SQLite 操作不阻塞 async runtime
    ///
    /// WHY:若 SQLite 操作未用 spawn_blocking 包装,直接在 async 上下文中
    /// 执行同步阻塞 I/O,会卡住 Tokio 工作线程,导致并发的 async 任务
    /// 无法被调度。此测试在执行 list_all(可能较慢)的同时,并发运行
    /// 一个轻量 async 任务,验证轻量任务能在超时时间内完成(说明
    /// runtime 未被阻塞)。
    #[tokio::test]
    async fn test_spawn_blocking_does_not_block_runtime() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test_blocking.db");
        let store = WikiStore::open(&db_path).unwrap();

        // 预置数据(5 条)
        for i in 0..5 {
            let entry = WikiEntry::new(
                format!("e-{i}"),
                format!("Entry {i}"),
                format!("Content {i}"),
                vec![],
                vec![0.0; 512],
            );
            store.insert(entry).await.unwrap();
        }

        // 并发执行:WikiStore 操作 + 轻量 async 计时任务
        // 轻量任务仅做 yield + 简单计算,正常情况下应在 1ms 内完成
        // 若 SQLite 操作阻塞了 runtime,轻量任务会被拖延,触发超时
        let store_clone = store.clone();
        let db_task = tokio::spawn(async move {
            // 执行可能较慢的 SQLite 查询
            store_clone.list_all().await
        });

        // 轻量 async 任务:多次 yield 让出执行权
        // WHY:若 runtime 被阻塞,yield_now 无法被调度,任务无法完成
        let lightweight_task = tokio::time::timeout(Duration::from_millis(100), async {
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
            42
        })
        .await;

        // 轻量任务应在超时前完成(实际通常 < 1ms)
        assert!(
            lightweight_task.is_ok(),
            "轻量 async 任务超时 — SQLite 操作可能阻塞了 runtime"
        );
        assert_eq!(lightweight_task.unwrap(), 42);

        // 等待 DB 任务完成,验证功能正确性
        let entries = db_task
            .await
            .expect("db task join 失败")
            .expect("list_all 失败");
        assert_eq!(entries.len(), 5, "应列出 5 条条目");
    }

    /// 回归测试:验证并发场景下 spawn_blocking 的功能正确性
    ///
    /// WHY:多个 spawn_blocking 任务并发执行时,读连接池可并行,
    /// 写入线程串行化写操作,整体不应死锁或丢数据。
    #[tokio::test]
    async fn test_concurrent_operations_correctness() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test_concurrent.db");
        let store = WikiStore::open(&db_path).unwrap();

        // 并发插入 10 条(每个任务独立 insert)
        let mut handles = Vec::new();
        for i in 0..10 {
            let store_clone = store.clone();
            handles.push(tokio::spawn(async move {
                let entry = WikiEntry::new(
                    format!("e-{i}"),
                    format!("Entry {i}"),
                    format!("Content {i}"),
                    vec![format!("tag-{}", i % 3)],
                    vec![0.0; 512],
                );
                store_clone.insert(entry).await
            }));
        }

        // 等待所有插入完成
        for handle in handles {
            handle
                .await
                .expect("insert task join 失败")
                .expect("insert 失败");
        }

        // 验证最终一致性
        assert_eq!(store.count().await.unwrap(), 10, "应持久化 10 条");
        let all = store.list_all().await.unwrap();
        assert_eq!(all.len(), 10, "list_all 应返回 10 条");

        // 按 tag 验证(0,3,6,9 → tag-0;1,4,7 → tag-1;2,5,8 → tag-2)
        let tag0 = store.list_by_tag("tag-0".to_string()).await.unwrap();
        let tag1 = store.list_by_tag("tag-1".to_string()).await.unwrap();
        let tag2 = store.list_by_tag("tag-2".to_string()).await.unwrap();
        assert_eq!(tag0.len(), 4, "tag-0 应有 4 条");
        assert_eq!(tag1.len(), 3, "tag-1 应有 3 条");
        assert_eq!(tag2.len(), 3, "tag-2 应有 3 条");
    }
}
