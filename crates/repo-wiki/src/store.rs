//! Wiki 存储层 — SQLite 持久化与结构化检索
//!
//! 对应架构层:L5 Knowledge
//!
//! # 设计要点
//! - `Mutex<Connection>` 包装:`rusqlite::Connection` 不是 `Sync`,
//!   用 `Mutex` 提供线程安全访问(§4.1 规范)
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
use std::sync::Mutex;
use uuid::Uuid;

use crate::error::WikiError;
use crate::iscm::{IscmAnchor, Layer};
use crate::types::{WikiConfig, WikiEntry};

/// Wiki 存储器 — 封装 SQLite Connection,提供线程安全的条目 CRUD
///
/// 所有方法通过 `Mutex::lock()` 串行化访问,避免并发写冲突。
/// 在 10-1000 条目规模下性能足够(单次操作 < 5ms)。
pub struct WikiStore {
    /// SQLite 连接(Mutex 包装以满足 Sync 约束)
    conn: Mutex<Connection>,
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
            conn: Mutex::new(conn),
            config,
        })
    }

    /// 返回配置的引用
    pub fn config(&self) -> &WikiConfig {
        &self.config
    }

    /// 查询当前 journal_mode(用于验证 WAL 是否启用)
    pub fn journal_mode(&self) -> Result<String, WikiError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;
        let mode: String = conn.query_row("PRAGMA journal_mode;", [], |row| row.get(0))?;
        Ok(mode)
    }

    /// 插入或更新 Wiki 条目(UPSERT 语义)
    ///
    /// 若 `entry_id` 已存在,则更新所有字段(含 `created_at` 重置);
    /// 否则插入新记录。
    pub fn insert(&self, entry: &WikiEntry) -> Result<(), WikiError> {
        let conn = self
            .conn
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
    }

    /// 按 entry_id 精确查找
    ///
    /// 返回 `None` 表示条目不存在。
    pub fn get(&self, entry_id: &str) -> Result<Option<WikiEntry>, WikiError> {
        let conn = self
            .conn
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
    }

    /// 删除条目并联动标记悬空锚点
    ///
    /// 若条目不存在,返回 `Ok(())`(幂等)。
    /// 注意:此方法仅删除 SQLite 中的记录,不删除 VectorIndex 中的向量;
    /// 调用方需同步调用 `VectorIndex::delete` 保持一致性。
    ///
    /// WHY:删除 Wiki 条目后,所有指向该条目的 ISCM 锚点应标记为悬空
    /// (is_dangling=true),保留审计轨迹而非物理删除锚点。
    /// 这样跨层引用方在 `resolve_anchor` 时会收到 `AnchorDangling` 错误,
    /// 知晓实体已失效,可触发清理或重建逻辑。
    pub fn delete(&self, entry_id: &str) -> Result<(), WikiError> {
        let conn = self
            .conn
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
    }

    /// 按 tag 过滤(精确匹配 tags JSON 数组中的某个元素)
    ///
    /// WHY:tags 存储为 JSON 数组字符串(如 `["a","b"]`),
    /// 用 LIKE `"%"tag"%"` 匹配 JSON 元素边界,避免误匹配子串。
    pub fn list_by_tag(&self, tag: &str) -> Result<Vec<WikiEntry>, WikiError> {
        let conn = self
            .conn
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
    }

    /// 全文模糊匹配(LIKE)— 在 title 与 content 中搜索
    ///
    /// 大小写不敏感(SQLite LIKE 默认对 ASCII 不敏感)。
    pub fn search_fulltext(&self, query: &str) -> Result<Vec<WikiEntry>, WikiError> {
        let conn = self
            .conn
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
    }

    /// 列出所有条目(按 created_at 升序)
    pub fn list_all(&self) -> Result<Vec<WikiEntry>, WikiError> {
        let conn = self
            .conn
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
    }

    /// 计算条目总数
    pub fn count(&self) -> Result<u32, WikiError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM entries;", [], |row| row.get(0))?;
        Ok(u32::try_from(count).unwrap_or(0))
    }

    // ============================================================
    // ISCM 跨层共享锚点方法(Week 2 Task 5)
    // ============================================================

    /// 创建跨层共享锚点
    ///
    /// 锚点 ID 自动生成 UUIDv7(时间有序),`created_at`/`updated_at` 自动设为当前 UTC。
    /// 同一 (layer, crate_name, entity_id) 组合可创建多个锚点(不同层引用同一实体)。
    pub fn create_anchor(
        &self,
        layer: Layer,
        crate_name: &str,
        entity_id: &str,
    ) -> Result<IscmAnchor, WikiError> {
        let anchor = IscmAnchor::new(layer, crate_name, entity_id);
        let conn = self
            .conn
            .lock()
            .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;

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

        Ok(anchor)
    }

    /// 解析锚点 — 返回指向的 Wiki 条目
    ///
    /// 解析流程:
    /// 1. 查询 anchors 表,若锚点不存在返回 `EntryNotFound`
    /// 2. 若 `is_dangling=true`,返回 `AnchorDangling`(实体已被删除)
    /// 3. 根据 `entity_id` 查询 entries 表
    /// 4. 若条目不存在,自动标记锚点为悬空并返回 `AnchorDangling`
    ///    (懒清理:发现悬空时才更新状态,避免删除时全表扫描)
    /// 5. 返回 WikiEntry
    pub fn resolve_anchor(&self, anchor_id: Uuid) -> Result<WikiEntry, WikiError> {
        let conn = self
            .conn
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

        let (entity_id, _layer_str, is_dangling) =
            anchor_row.ok_or_else(|| WikiError::EntryNotFound(format!("anchor {}", anchor_id)))?;

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
    }

    /// 标记锚点为悬空(实体被删除或失效时调用)
    ///
    /// 幂等操作:对已悬空的锚点再次标记不会报错。
    pub fn mark_dangling(&self, anchor_id: Uuid) -> Result<(), WikiError> {
        let conn = self
            .conn
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
    }

    /// 列出指定实体的所有锚点(跨层引用查询)
    ///
    /// 用于审计:查看某知识实体被哪些层、哪些 crate 引用。
    pub fn list_anchors_by_entity(&self, entity_id: &str) -> Result<Vec<IscmAnchor>, WikiError> {
        let conn = self
            .conn
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
    }

    /// 列出指定层的所有锚点(层内审计)
    ///
    /// 用于层内自检:查看某层引用了哪些知识实体。
    pub fn list_anchors_by_layer(&self, layer: Layer) -> Result<Vec<IscmAnchor>, WikiError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| WikiError::VectorIndexError(format!("mutex poisoned: {e}")))?;

        let mut stmt = conn.prepare(
            "SELECT anchor_id, layer, crate_name, entity_id, created_at, updated_at, is_dangling
             FROM anchors WHERE layer = ?1 ORDER BY created_at ASC;",
        )?;
        let rows = stmt.query_map(params![layer.as_str()], row_to_anchor)?;
        let mut anchors = Vec::new();
        for row in rows {
            anchors.push(row?);
        }
        Ok(anchors)
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

    #[test]
    fn test_open_and_journal_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();
        let mode = store.journal_mode().unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[test]
    fn test_insert_and_get() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();

        let entry = WikiEntry::new("e-1", "标题", "内容", vec!["t".into()], vec![0.5; 512]);
        store.insert(&entry).unwrap();

        let fetched = store.get("e-1").unwrap().unwrap();
        assert_eq!(fetched.entry_id, "e-1");
        assert_eq!(fetched.title, "标题");
        assert_eq!(fetched.tags, vec!["t".to_string()]);
        assert_eq!(fetched.embedding.len(), 512);
        assert!((fetched.embedding[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_get_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();
        let result = store.get("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();

        let entry = WikiEntry::new("e-1", "标题", "内容", vec![], vec![0.0; 512]);
        store.insert(&entry).unwrap();
        assert_eq!(store.count().unwrap(), 1);

        store.delete("e-1").unwrap();
        assert_eq!(store.count().unwrap(), 0);
        assert!(store.get("e-1").unwrap().is_none());
    }

    #[test]
    fn test_list_by_tag() {
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
            store.insert(&entry).unwrap();
        }

        let tagged = store.list_by_tag("tag-0").unwrap();
        assert_eq!(tagged.len(), 6);

        let tagged_1 = store.list_by_tag("tag-1").unwrap();
        assert_eq!(tagged_1.len(), 1);
    }

    #[test]
    fn test_search_fulltext() {
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
        store.insert(&entry).unwrap();

        let found = store.search_fulltext("Rust").unwrap();
        assert!(!found.is_empty());

        let not_found = store.search_fulltext("nonexistent").unwrap();
        assert!(not_found.is_empty());
    }

    #[test]
    fn test_count() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();

        assert_eq!(store.count().unwrap(), 0);
        for i in 0..5 {
            let entry = WikiEntry::new(format!("e-{i}"), "t", "c", vec![], vec![0.0; 512]);
            store.insert(&entry).unwrap();
        }
        assert_eq!(store.count().unwrap(), 5);
    }

    #[test]
    fn test_list_all() {
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
            store.insert(&entry).unwrap();
        }

        let all = store.list_all().unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_upsert_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = WikiStore::open(&db_path).unwrap();

        let entry_v1 = WikiEntry::new("e-1", "v1", "c1", vec![], vec![0.0; 512]);
        store.insert(&entry_v1).unwrap();

        let entry_v2 = WikiEntry::new("e-1", "v2", "c2", vec![], vec![0.0; 512]);
        store.insert(&entry_v2).unwrap();

        assert_eq!(store.count().unwrap(), 1);
        let fetched = store.get("e-1").unwrap().unwrap();
        assert_eq!(fetched.title, "v2");
    }
}
