//! Wiki 存储层 — SQLite 持久化与结构化检索
//!
//! 对应架构层:L5 Knowledge
//!
//! # 设计要点
//! - `Arc<Mutex<Connection>>` 包装:`rusqlite::Connection` 不是 `Sync`,
//!   用 `Mutex` 提供线程安全访问;`Arc` 包装支持 Clone 与跨任务共享,
//!   使 `spawn_blocking` 闭包可拥有连接(参考 cmt-tiering/warm.rs 实现)
//! - **spawn_blocking 包装所有 SQLite 操作**(C-01 修复):
//!   SQLite 操作是同步阻塞 I/O,直接在 async 上下文中调用会阻塞 Tokio
//!   runtime 的工作线程,影响其他任务的响应性(架构红线:无孤儿调用、
//!   async fn 满足 Send + 'static)。`spawn_blocking` 将阻塞操作转移到
//!   专用阻塞线程池,async runtime 可继续调度其他任务
//! - WAL 模式:提升并发读写性能,适合"读多写少"的 Wiki 场景
//! - tags 存储:JSON 字符串序列化,读取时反序列化
//! - embedding 存储:BLOB(小端序 f32),读取时反序列化
//! - 时间戳:ISO 8601 字符串,可读且可排序
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
use std::sync::{Arc, Mutex};
use tokio::task::spawn_blocking;
use uuid::Uuid;

use crate::error::WikiError;
use crate::iscm::{IscmAnchor, Layer};
use crate::types::{WikiConfig, WikiEntry};

/// Wiki 存储器 — 封装 SQLite Connection,提供线程安全的条目 CRUD
///
/// 所有 SQLite 操作通过 `tokio::task::spawn_blocking` 转移到阻塞线程池,
/// 避免阻塞 async runtime(参考 cmt-tiering/warm.rs 实现)。
/// `Arc<Mutex<Connection>>` 包装支持 Clone 与跨任务共享。
///
/// # 线程安全
/// `Arc<Mutex<Connection>>` 包装,可 Clone(廉价,Arc 引用计数)。
/// 所有 async fn 满足 `Send + 'static` 约束(架构红线)。
#[derive(Clone)]
pub struct WikiStore {
    /// SQLite 连接(`Arc<Mutex>` 包装,支持 Clone 与跨任务共享)
    ///
    /// WHY `Arc<Mutex>`:spawn_blocking 需要 'static + Send 的闭包,
    /// `Arc<Mutex>` 允许将连接所有权转移到阻塞线程
    conn: Arc<Mutex<Connection>>,
    /// 存储配置(运行时只读)
    config: WikiConfig,
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
    /// 启用 WAL 模式(若配置允许),创建 `entries` 表与索引。
    /// 同时创建 `anchors` 表(ISCM 跨层共享锚点,Week 2 Task 5)。
    pub fn open_with_config(config: WikiConfig) -> Result<Self, WikiError> {
        let conn = Connection::open(&config.db_path)?;

        // 启用 WAL 模式(若配置允许)
        // WHY:WAL(Write-Ahead Logging)模式允许读写并发,
        // 默认的 rollback journal 模式下写会阻塞读
        if config.wal_enabled {
            conn.pragma_update(None, "journal_mode", "WAL")?;
        }

        // 创建 entries 表
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
        // WHY:tags 存储为 JSON 数组字符串,LIKE '%tag%' 需全表扫描;
        // 索引虽不能消除 LIKE 扫描,但可加速其他等值查询
        conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_entries_tags ON entries(tags);")?;

        // 创建 anchors 表(ISCM 跨层共享锚点)
        // WHY:同一知识实体可能被 L2/L5/L9 等多层引用,统一锚点表确保
        // 跨层一致性;is_dangling 用 INTEGER(0/1)存储因 SQLite 无原生 bool
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

        // 创建 anchors 索引(加速按 entity_id 与 layer 查询)
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_anchors_entity ON anchors(entity_id);
             CREATE INDEX IF NOT EXISTS idx_anchors_layer ON anchors(layer);",
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            config,
        })
    }

    /// 返回配置的引用
    pub fn config(&self) -> &WikiConfig {
        &self.config
    }

    /// 异步查询当前 journal_mode(用于验证 WAL 是否启用)
    ///
    /// WHY spawn_blocking:PRAGMA 查询仍是同步 SQLite 调用,需转移到阻塞线程池
    pub async fn journal_mode(&self) -> Result<String, WikiError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;
            let mode: String = conn.query_row("PRAGMA journal_mode;", [], |row| row.get(0))?;
            Ok(mode)
        })
        .await
        .map_err(WikiError::BlockingJoinError)?
    }

    /// 异步插入或更新 Wiki 条目(UPSERT 语义)
    ///
    /// 若 `entry_id` 已存在,则更新所有字段(含 `created_at` 重置);
    /// 否则插入新记录。
    ///
    /// WHY spawn_blocking:SQLite INSERT/UPDATE 是同步阻塞 I/O,
    /// 直接调用会阻塞 async runtime 工作线程(参考 cmt-tiering/warm.rs)
    pub async fn insert(&self, entry: WikiEntry) -> Result<(), WikiError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;

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

            Ok(())
        })
        .await
        .map_err(WikiError::BlockingJoinError)?
    }

    /// 异步按 entry_id 精确查找
    ///
    /// 返回 `None` 表示条目不存在。
    ///
    /// WHY spawn_blocking:SQLite SELECT 是同步阻塞 I/O,需转移到阻塞线程池
    pub async fn get(&self, entry_id: String) -> Result<Option<WikiEntry>, WikiError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;

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
        .map_err(WikiError::BlockingJoinError)?
    }

    /// 异步删除条目并联动标记悬空锚点
    ///
    /// 若条目不存在,返回 `Ok(())`(幂等)。
    /// 注意:此方法仅删除 SQLite 中的记录,不删除 VectorIndex 中的向量;
    /// 调用方需同步调用 `VectorIndex::delete` 保持一致性。
    ///
    /// WHY:删除 Wiki 条目后,所有指向该条目的 ISCM 锚点应标记为悬空
    /// (is_dangling=true),保留审计轨迹而非物理删除锚点。
    /// 这样跨层引用方在 `resolve_anchor` 时会收到 `AnchorDangling` 错误,
    /// 知晓实体已失效,可触发清理或重建逻辑。
    ///
    /// WHY spawn_blocking:删除条目 + 联动更新锚点是多步 SQLite 操作,
    /// 需在同一锁内完成以保证原子性,整体作为阻塞任务执行
    pub async fn delete(&self, entry_id: String) -> Result<(), WikiError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;
            conn.execute(
                "DELETE FROM entries WHERE entry_id = ?1;",
                params![entry_id],
            )?;

            // 联动标记悬空锚点:同一 entry_id 的所有锚点置为 is_dangling=1
            // 物理删除条目,逻辑标记锚点 — 保留跨层审计轨迹
            let now_iso = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE anchors SET is_dangling = 1, updated_at = ?1 WHERE entity_id = ?2;",
                params![now_iso, entry_id],
            )?;
            Ok(())
        })
        .await
        .map_err(WikiError::BlockingJoinError)?
    }

    /// 异步按 tag 过滤(精确匹配 tags JSON 数组中的某个元素)
    ///
    /// WHY:tags 存储为 JSON 数组字符串(如 `["a","b"]`),
    /// 用 LIKE `"%"tag"%"` 匹配 JSON 元素边界,避免误匹配子串。
    ///
    /// WHY spawn_blocking:SQLite SELECT 是同步阻塞 I/O,需转移到阻塞线程池
    pub async fn list_by_tag(&self, tag: String) -> Result<Vec<WikiEntry>, WikiError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;

            // 用双引号包裹 tag,匹配 JSON 数组元素边界
            // 如 tags = ["tag-0","other"],LIKE '%"tag-0"%' 可命中
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
        .map_err(WikiError::BlockingJoinError)?
    }

    /// 异步全文模糊匹配(LIKE)— 在 title 与 content 中搜索
    ///
    /// 大小写不敏感(SQLite LIKE 默认对 ASCII 不敏感)。
    ///
    /// WHY spawn_blocking:SQLite SELECT 是同步阻塞 I/O,需转移到阻塞线程池
    pub async fn search_fulltext(&self, query: String) -> Result<Vec<WikiEntry>, WikiError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;

            let pattern = format!("%{query}%");

            let mut stmt = conn.prepare(
                "SELECT entry_id, title, content, tags, embedding, created_at, updated_at
                 FROM entries
                 WHERE title LIKE ?1 OR content LIKE ?1;",
            )?;
            let rows = stmt.query_map(params![pattern], row_to_entry)?;
            let mut entries = Vec::new();
            for row in rows {
                entries.push(row?);
            }
            Ok(entries)
        })
        .await
        .map_err(WikiError::BlockingJoinError)?
    }

    /// 异步列出所有条目(按 created_at 升序)
    ///
    /// WHY spawn_blocking:SQLite SELECT 是同步阻塞 I/O,需转移到阻塞线程池
    pub async fn list_all(&self) -> Result<Vec<WikiEntry>, WikiError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;

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
        .map_err(WikiError::BlockingJoinError)?
    }

    /// 异步计算条目总数
    ///
    /// WHY spawn_blocking:SQLite COUNT 查询是同步阻塞 I/O,需转移到阻塞线程池
    pub async fn count(&self) -> Result<u32, WikiError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM entries;", [], |row| row.get(0))?;
            Ok(u32::try_from(count).unwrap_or(0))
        })
        .await
        .map_err(WikiError::BlockingJoinError)?
    }

    // ============================================================
    // ISCM 跨层共享锚点方法(Week 2 Task 5)
    // ============================================================

    /// 异步创建跨层共享锚点
    ///
    /// 锚点 ID 自动生成 UUIDv7(时间有序),`created_at`/`updated_at` 自动设为当前 UTC。
    /// 同一 (layer, crate_name, entity_id) 组合可创建多个锚点(不同层引用同一实体)。
    ///
    /// WHY spawn_blocking:SQLite INSERT 是同步阻塞 I/O,需转移到阻塞线程池
    pub async fn create_anchor(
        &self,
        layer: Layer,
        crate_name: String,
        entity_id: String,
    ) -> Result<IscmAnchor, WikiError> {
        // 锚点对象在闭包外构造(无需阻塞),clone 一份供闭包使用
        let anchor = IscmAnchor::new(layer, crate_name, entity_id);
        let anchor_for_closure = anchor.clone();
        let conn = self.conn.clone();
        // WHY 显式返回类型标注:nexus_core 也 impl From<rusqlite::Error>,
        // 闭包结果经 `?` 丢弃后接 Ok(anchor),编译器无法推断 E = WikiError
        spawn_blocking(move || -> Result<(), WikiError> {
            let conn = conn
                .lock()
                .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;

            conn.execute(
                "INSERT INTO anchors
                    (anchor_id, layer, crate_name, entity_id, created_at, updated_at, is_dangling)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7);",
                params![
                    anchor_for_closure.anchor_id.to_string(),
                    anchor_for_closure.layer.as_str(),
                    anchor_for_closure.crate_name,
                    anchor_for_closure.entity_id,
                    anchor_for_closure.created_at.to_rfc3339(),
                    anchor_for_closure.updated_at.to_rfc3339(),
                    anchor_for_closure.is_dangling as i64,
                ],
            )?;

            Ok(())
        })
        .await
        // 双层 ? :外层处理 JoinError,内层处理闭包返回的 WikiError(SQLite 错误),
        // 避免 SQLite 错误被静默丢弃(架构红线:无孤儿调用)
        .map_err(WikiError::BlockingJoinError)??;
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
    /// 5. 返回 WikiEntry
    ///
    /// WHY spawn_blocking:多步 SQLite 操作需在同一锁内完成以保证一致性,
    /// 整体作为阻塞任务执行
    pub async fn resolve_anchor(&self, anchor_id: Uuid) -> Result<WikiEntry, WikiError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;

            // 1. 查询锚点
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

            let (entity_id, _layer_str, is_dangling) = anchor_row
                .ok_or_else(|| WikiError::EntryNotFound(format!("anchor {anchor_id}")))?;

            // 2. 锚点已标记悬空
            if is_dangling != 0 {
                return Err(WikiError::AnchorDangling(format!(
                    "anchor {anchor_id} marked dangling"
                )));
            }

            // 3. 查询实体条目
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
                    // 4. 实体不存在,懒标记锚点为悬空
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
        })
        .await
        .map_err(WikiError::BlockingJoinError)?
    }

    /// 异步标记锚点为悬空(实体被删除或失效时调用)
    ///
    /// 幂等操作:对已悬空的锚点再次标记不会报错。
    ///
    /// WHY spawn_blocking:SQLite UPDATE 是同步阻塞 I/O,需转移到阻塞线程池
    pub async fn mark_dangling(&self, anchor_id: Uuid) -> Result<(), WikiError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;
            let now_iso = Utc::now().to_rfc3339();
            let affected = conn.execute(
                "UPDATE anchors SET is_dangling = 1, updated_at = ?1 WHERE anchor_id = ?2;",
                params![now_iso, anchor_id.to_string()],
            )?;
            if affected == 0 {
                return Err(WikiError::EntryNotFound(format!("anchor {anchor_id}")));
            }
            Ok(())
        })
        .await
        .map_err(WikiError::BlockingJoinError)?
    }

    /// 异步列出指定实体的所有锚点(跨层引用查询)
    ///
    /// 用于审计:查看某知识实体被哪些层、哪些 crate 引用。
    ///
    /// WHY spawn_blocking:SQLite SELECT 是同步阻塞 I/O,需转移到阻塞线程池
    pub async fn list_anchors_by_entity(
        &self,
        entity_id: String,
    ) -> Result<Vec<IscmAnchor>, WikiError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;

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
        .map_err(WikiError::BlockingJoinError)?
    }

    /// 异步列出指定层的所有锚点(层内审计)
    ///
    /// 用于层内自检:查看某层引用了哪些知识实体。
    ///
    /// WHY spawn_blocking:SQLite SELECT 是同步阻塞 I/O,需转移到阻塞线程池
    pub async fn list_anchors_by_layer(&self, layer: Layer) -> Result<Vec<IscmAnchor>, WikiError> {
        let conn = self.conn.clone();
        let layer_str = layer.as_str();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;

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
        .map_err(WikiError::BlockingJoinError)?
    }
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
fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<WikiEntry> {
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
        let lightweight_task = tokio::time::timeout(std::time::Duration::from_millis(100), async {
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
    /// WHY:多个 spawn_blocking 任务并发执行时,Mutex 串行化访问,
    /// 但不应死锁或丢数据。此测试并发插入 + 计数,验证最终一致性。
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
