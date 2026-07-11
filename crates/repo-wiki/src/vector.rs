//! 向量检索层 — 内存 KNN 检索 + SQLite 持久化(P0-8-wiki)
//!
//! 对应架构层:L5 Knowledge
//!
//! # 设计演进(P0-8-wiki)
//! 原实现为纯内存 HashMap,进程重启后向量索引丢失。
//! P0-8-wiki 实现 SQLite 持久化:
//! - 内存索引保持 KNN 查询性能(O(n) 遍历,10-1000 条目 < 10ms)
//! - SQLite 表 `vector_index` 持久化向量(BLOB 小端序 f32)
//! - 启动时从 SQLite 加载恢复内存索引
//! - upsert/delete 时同步写入 SQLite(WAL 模式保证一致性)
//!
//! # 降级说明(保留)
//! 原计划使用 `sqlite-vec` 扩展提供 SQLite 原生向量检索,但:
//! 1. `sqlite-vec 0.1.9` 的 Rust binding 仅暴露 C 入口 `sqlite3_vec_init`
//! 2. 注册扩展需调用 `rusqlite::ffi::sqlite3_auto_extension` + `unsafe` 代码
//! 3. 项目铁律 `#![forbid(unsafe_code)]` 禁止任何 unsafe 块
//! 4. 因此使用手动 BLOB 存储 + 内存 KNN 作为替代方案

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, RwLock};

use rusqlite::{params, Connection};

use crate::error::WikiError;

/// 向量索引 — 内存 KNN 检索 + SQLite 持久化
///
/// 使用 `RwLock<HashMap<String, Vec<f32>>>` 存储向量用于快速查询,
/// 可选的 SQLite 连接用于持久化。
///
/// WHY RwLock 而非 Mutex(仅 vectors 字段):B1 优化,search 是高频读操作(KNN 遍历),
/// RwLock 允许多个并发 search 同时执行,仅在写入时互斥。
///
/// WHY sqlite_conn 用 Mutex 而非 RwLock:`rusqlite::Connection` 内部含 `RefCell`
/// (StatementCache),仅实现 `Send` 不实现 `Sync`。`RwLock<T>: Sync` 要求 `T: Send + Sync`,
/// 故 `RwLock<Option<Connection>>` 不 `Sync`,`Arc<VectorIndex>` 不 `Send`,
/// bench 无法 `spawn_blocking`/`rt.spawn`。`Mutex<T>: Sync` 仅要求 `T: Send`,
/// 改用 `Mutex` 让 `VectorIndex: Sync` + `Arc<VectorIndex>: Send`。语义等价:
/// 原代码对 sqlite_conn 只用 `.read()`,从未用 `.write()`,Mutex 独占访问无差异。
pub struct VectorIndex {
    /// 向量维度(应与 WikiConfig.vector_dim 一致)
    dim: usize,
    /// 内存向量存储(entry_id → embedding)
    vectors: RwLock<HashMap<String, Vec<f32>>>,
    /// 可选的 SQLite 持久化连接
    sqlite_conn: Mutex<Option<Connection>>,
}

impl VectorIndex {
    /// 创建指定维度的空向量索引(无持久化)
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            vectors: RwLock::new(HashMap::new()),
            sqlite_conn: Mutex::new(None),
        }
    }

    /// 创建向量索引并绑定 SQLite 持久化
    ///
    /// 打开 SQLite 数据库,创建 `vector_index` 表(若不存在),
    /// 并加载已有向量到内存索引。
    pub fn with_sqlite(dim: usize, db_path: &Path) -> Result<Self, WikiError> {
        let conn = Connection::open(db_path).map_err(|e| {
            WikiError::DatabaseError(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("打开向量索引数据库失败: {e}")),
            ))
        })?;

        // 启用 WAL 模式
        conn.execute("PRAGMA journal_mode=WAL;", []).ok();

        // 创建向量索引表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS vector_index (
                entry_id TEXT PRIMARY KEY,
                embedding BLOB NOT NULL,
                dim INTEGER NOT NULL
            );",
            [],
        )
        .map_err(WikiError::DatabaseError)?;

        // 创建维度索引加速查询
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_vector_dim ON vector_index(dim);",
            [],
        )
        .ok();

        let index = Self {
            dim,
            vectors: RwLock::new(HashMap::new()),
            sqlite_conn: Mutex::new(Some(conn)),
        };

        // 从 SQLite 加载已有向量到内存
        index.load_from_sqlite()?;

        Ok(index)
    }

    /// 返回配置的向量维度
    pub fn dimension(&self) -> usize {
        self.dim
    }

    /// 插入或更新向量(UPSERT 语义)
    ///
    /// 若 `entry_id` 已存在,覆盖旧向量。
    /// 维度不匹配时返回 `VectorIndexError`。
    /// 若绑定了 SQLite,同步写入持久化。
    pub fn upsert(&self, entry_id: &str, embedding: &[f32]) -> Result<(), WikiError> {
        if embedding.len() != self.dim {
            return Err(WikiError::VectorIndexError(format!(
                "embedding dimension mismatch: expected {}, got {}",
                self.dim,
                embedding.len()
            )));
        }

        // 更新内存索引
        {
            let mut vectors = self
                .vectors
                .write()
                .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
            vectors.insert(entry_id.to_string(), embedding.to_vec());
        }

        // 同步写入 SQLite
        self.persist_upsert(entry_id, embedding)?;

        Ok(())
    }

    /// KNN 检索 — 返回与查询向量最相似的 Top-K 条目
    ///
    /// 返回 `(entry_id, similarity_score)` 列表,按相似度降序排列。
    /// 相似度 ∈ [0.0, 1.0](余弦相似度,1.0 表示完全相同)。
    ///
    /// # 性能
    /// O(n) 遍历 + O(n) Top-K 选择(`select_nth_unstable_by`)+ O(K log K) 局部排序,
    /// n 为索引中的向量总数。在 10-1000 条目规模下延迟 < 10ms。
    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<(String, f32)>, WikiError> {
        if query.len() != self.dim {
            return Err(WikiError::VectorIndexError(format!(
                "query dimension mismatch: expected {}, got {}",
                self.dim,
                query.len()
            )));
        }

        let vectors = self
            .vectors
            .read()
            .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;

        // 计算所有向量的余弦相似度
        // SubTask 21.4:使用 nexus_core 统一的 cosine_similarity_slices
        let mut scored: Vec<(String, f32)> = vectors
            .iter()
            .map(|(id, vec)| (id.clone(), nexus_core::cosine_similarity_slices(query, vec)))
            .collect();

        // Top-K 选择用 select_nth_unstable_by (O(n)),仅对前 K 做 K-log-K 排序
        // WHY 不用 sort_by:工程约定 Top-K 必须用 select_nth_unstable(O(n)) 替代 O(n log n)
        if top_k < scored.len() {
            scored.select_nth_unstable_by(top_k, |a, b| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        scored.truncate(top_k);
        // 前 K 元素已是无序的 Top-K 集合,这里做最终降序排序(K log K)
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored)
    }

    /// 删除向量
    ///
    /// 若 `entry_id` 不存在,返回 `Ok(())`(幂等)。
    /// 若绑定了 SQLite,同步删除持久化记录。
    pub fn delete(&self, entry_id: &str) -> Result<(), WikiError> {
        // 更新内存索引
        {
            let mut vectors = self
                .vectors
                .write()
                .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
            vectors.remove(entry_id);
        }

        // 同步删除 SQLite 记录
        self.persist_delete(entry_id)?;

        Ok(())
    }

    /// 返回索引中的向量总数
    pub fn len(&self) -> Result<usize, WikiError> {
        let vectors = self
            .vectors
            .read()
            .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
        Ok(vectors.len())
    }

    /// 返回索引是否为空
    pub fn is_empty(&self) -> Result<bool, WikiError> {
        Ok(self.len()? == 0)
    }

    /// 从 SQLite 加载所有向量到内存索引
    fn load_from_sqlite(&self) -> Result<(), WikiError> {
        let conn_guard = self
            .sqlite_conn
            .lock()
            .map_err(|e| WikiError::VectorIndexError(format!("sqlite mutex poisoned: {e}")))?;

        let conn = match conn_guard.as_ref() {
            Some(c) => c,
            None => return Ok(()), // 无持久化连接,直接返回
        };

        let mut stmt = conn
            .prepare("SELECT entry_id, embedding FROM vector_index WHERE dim = ?1;")
            .map_err(WikiError::DatabaseError)?;

        let rows = stmt
            .query_map(params![self.dim as i64], |row| {
                let entry_id: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                let embedding = blob_to_embedding(&blob);
                Ok((entry_id, embedding))
            })
            .map_err(WikiError::DatabaseError)?;

        let mut vectors = self
            .vectors
            .write()
            .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;

        for row in rows {
            let (entry_id, embedding) = row.map_err(WikiError::DatabaseError)?;
            if embedding.len() == self.dim {
                vectors.insert(entry_id, embedding);
            }
        }

        Ok(())
    }

    /// 将向量持久化到 SQLite
    fn persist_upsert(&self, entry_id: &str, embedding: &[f32]) -> Result<(), WikiError> {
        let conn_guard = self
            .sqlite_conn
            .lock()
            .map_err(|e| WikiError::VectorIndexError(format!("sqlite mutex poisoned: {e}")))?;

        let conn = match conn_guard.as_ref() {
            Some(c) => c,
            None => return Ok(()), // 无持久化连接,直接返回
        };

        let blob = embedding_to_blob(embedding);
        conn.execute(
            "INSERT OR REPLACE INTO vector_index (entry_id, embedding, dim)
             VALUES (?1, ?2, ?3);",
            params![entry_id, blob, self.dim as i64],
        )
        .map_err(WikiError::DatabaseError)?;

        Ok(())
    }

    /// 从 SQLite 删除向量记录
    fn persist_delete(&self, entry_id: &str) -> Result<(), WikiError> {
        let conn_guard = self
            .sqlite_conn
            .lock()
            .map_err(|e| WikiError::VectorIndexError(format!("sqlite mutex poisoned: {e}")))?;

        let conn = match conn_guard.as_ref() {
            Some(c) => c,
            None => return Ok(()), // 无持久化连接,直接返回
        };

        conn.execute(
            "DELETE FROM vector_index WHERE entry_id = ?1;",
            params![entry_id],
        )
        .map_err(WikiError::DatabaseError)?;

        Ok(())
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
    let mut embedding = Vec::with_capacity(blob.len() / 4);
    for chunk in blob.chunks_exact(4) {
        let bytes: [u8; 4] = chunk.try_into().unwrap_or([0; 4]);
        embedding.push(f32::from_le_bytes(bytes));
    }
    embedding
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_and_len() {
        let idx = VectorIndex::new(4);
        assert!(idx.is_empty().unwrap());

        idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        idx.upsert("b", &[0.0, 1.0, 0.0, 0.0]).unwrap();
        assert_eq!(idx.len().unwrap(), 2);
        assert!(!idx.is_empty().unwrap());
    }

    #[test]
    fn test_upsert_dimension_mismatch() {
        let idx = VectorIndex::new(4);
        let result = idx.upsert("a", &[1.0, 0.0, 0.0]);
        assert!(matches!(result, Err(WikiError::VectorIndexError(_))));
    }

    #[test]
    fn test_search_identical_vector() {
        let idx = VectorIndex::new(4);
        idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        idx.upsert("b", &[0.0, 1.0, 0.0, 0.0]).unwrap();

        let results = idx.search(&[1.0, 0.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "a");
        // 相同向量余弦相似度应接近 1.0
        assert!((results[0].1 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_search_top_k() {
        let idx = VectorIndex::new(2);
        idx.upsert("a", &[1.0, 0.0]).unwrap();
        idx.upsert("b", &[0.9, 0.1]).unwrap();
        idx.upsert("c", &[0.0, 1.0]).unwrap();

        let results = idx.search(&[1.0, 0.0], 2).unwrap();
        assert_eq!(results.len(), 2);
        // 最相似的应是 "a"(完全相同),其次是 "b"(0.9, 0.1)
        assert_eq!(results[0].0, "a");
        assert_eq!(results[1].0, "b");
    }

    #[test]
    fn test_search_query_dimension_mismatch() {
        let idx = VectorIndex::new(4);
        idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        let result = idx.search(&[1.0, 0.0, 0.0], 1);
        assert!(matches!(result, Err(WikiError::VectorIndexError(_))));
    }

    #[test]
    fn test_delete() {
        let idx = VectorIndex::new(4);
        idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        assert_eq!(idx.len().unwrap(), 1);

        idx.delete("a").unwrap();
        assert_eq!(idx.len().unwrap(), 0);

        // 删除不存在的条目应幂等返回 Ok
        idx.delete("nonexistent").unwrap();
    }

    #[test]
    fn test_delete_removes_from_search() {
        let idx = VectorIndex::new(4);
        idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        idx.upsert("b", &[0.0, 1.0, 0.0, 0.0]).unwrap();

        idx.delete("a").unwrap();

        let results = idx.search(&[1.0, 0.0, 0.0, 0.0], 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "b");
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        // SubTask 21.4:使用 nexus_core 统一的 cosine_similarity_slices
        // 零向量与任意向量:返回 0.0(非 NaN)
        assert_eq!(
            nexus_core::cosine_similarity_slices(&[0.0; 4], &[1.0, 0.0, 0.0, 0.0]),
            0.0
        );
        assert_eq!(
            nexus_core::cosine_similarity_slices(&[0.0; 4], &[0.0; 4]),
            0.0
        );
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        // 正交向量:相似度为 0
        let sim = nexus_core::cosine_similarity_slices(&[1.0, 0.0], &[0.0, 1.0]);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let sim =
            nexus_core::cosine_similarity_slices(&[0.5, 0.5, 0.5, 0.5], &[0.5, 0.5, 0.5, 0.5]);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_upsert_overwrites() {
        let idx = VectorIndex::new(4);
        idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        idx.upsert("a", &[0.0, 1.0, 0.0, 0.0]).unwrap();

        assert_eq!(idx.len().unwrap(), 1);

        let results = idx.search(&[0.0, 1.0, 0.0, 0.0], 1).unwrap();
        assert_eq!(results[0].0, "a");
        assert!((results[0].1 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_search_empty_index() {
        let idx = VectorIndex::new(4);
        let results = idx.search(&[1.0, 0.0, 0.0, 0.0], 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_top_k_larger_than_size() {
        let idx = VectorIndex::new(4);
        idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();

        let results = idx.search(&[1.0, 0.0, 0.0, 0.0], 10).unwrap();
        assert_eq!(results.len(), 1);
    }

    // ── P0-8-wiki: SQLite 持久化测试 ──

    #[test]
    fn test_sqlite_persist_and_restore() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let db_path = tmp_dir.path().join("vectors.db");

        // 创建索引并插入向量
        {
            let idx = VectorIndex::with_sqlite(4, &db_path).expect("创建索引失败");
            idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
            idx.upsert("b", &[0.0, 1.0, 0.0, 0.0]).unwrap();
            assert_eq!(idx.len().unwrap(), 2);
        }

        // 重新打开索引,验证向量已恢复
        {
            let idx = VectorIndex::with_sqlite(4, &db_path).expect("恢复索引失败");
            assert_eq!(idx.len().unwrap(), 2);

            let results = idx.search(&[1.0, 0.0, 0.0, 0.0], 2).unwrap();
            assert_eq!(results.len(), 2);
            assert_eq!(results[0].0, "a");
            assert!((results[0].1 - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn test_sqlite_delete_persisted() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let db_path = tmp_dir.path().join("vectors.db");

        // 创建索引并插入向量
        {
            let idx = VectorIndex::with_sqlite(4, &db_path).expect("创建索引失败");
            idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
            idx.upsert("b", &[0.0, 1.0, 0.0, 0.0]).unwrap();
            idx.delete("a").unwrap();
            assert_eq!(idx.len().unwrap(), 1);
        }

        // 重新打开索引,验证删除已持久化
        {
            let idx = VectorIndex::with_sqlite(4, &db_path).expect("恢复索引失败");
            assert_eq!(idx.len().unwrap(), 1);

            let results = idx.search(&[1.0, 0.0, 0.0, 0.0], 10).unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].0, "b");
        }
    }

    #[test]
    fn test_sqlite_overwrite_persisted() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let db_path = tmp_dir.path().join("vectors.db");

        // 创建索引并插入向量
        {
            let idx = VectorIndex::with_sqlite(4, &db_path).expect("创建索引失败");
            idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        }

        // 覆盖向量
        {
            let idx = VectorIndex::with_sqlite(4, &db_path).expect("恢复索引失败");
            idx.upsert("a", &[0.0, 1.0, 0.0, 0.0]).unwrap();
        }

        // 验证覆盖已持久化
        {
            let idx = VectorIndex::with_sqlite(4, &db_path).expect("恢复索引失败");
            let results = idx.search(&[0.0, 1.0, 0.0, 0.0], 1).unwrap();
            assert_eq!(results[0].0, "a");
            assert!((results[0].1 - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn test_embedding_blob_roundtrip() {
        let original = vec![1.0f32, 0.5, -0.25, 0.0];
        let blob = embedding_to_blob(&original);
        let restored = blob_to_embedding(&blob);
        assert_eq!(original, restored);
    }
}
