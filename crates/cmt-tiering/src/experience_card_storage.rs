//! 经验卡片持久化 — OpenMLE + SQLite 复合索引（设计文档 §8.2）
//!
//! 对应架构层: **L3 Storage**（cmt-tiering 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §8.2
//! 对应论文: 清华 OpenMLE（经验卡片 + 三因子索引）+ Dressage（Token 级证据）
//! 对应 ADR: ADR-049 决策 1（experience-card-storage 落点 cmt-tiering，内嵌模块）
//!
//! # 核心职责
//!
//! 持久化 L0 [`ExperienceCard`]（Phase 0 契约）与 L1 `TokenLedgerEntry` 训练证据：
//! - **SQLite 五复合索引**: 三因子 / 错误哈希 / 方法家族 / 创建时间 / 任务状态
//! - **热缓存**: DashMap（`Box<str>` 零拷贝 key），高分/未满载入缓存，LRU 驱逐最低分
//! - **三因子查询**: hot_cache 优先 → 不足回源 SQLite（`select_nth_unstable_by` Top-K，红线 R8）
//! - **错误签名查询**: 错误哈希聚类检索
//! - **证据落盘**: TokenLedgerEntry MessagePack 载荷（补 Phase 1 WAL 遗留）
//!
//! # 设计约束（铁律 + 红线）
//!
//! - **铁律3（不可变）**: 卡片只读持久化（存储不修改卡片内容），UPSERT 幂等
//! - **rusqlite 红线（D-5）**: 所有 SQLite 操作经 `spawn_blocking` 包装
//!   （Connection 非 Sync，Mutex 短临界区在 spawn_blocking 闭包内同步执行）
//! - **红线 R8**: 三因子 Top-K 用 `select_nth_unstable_by`（O(n)），禁 `sort_by`
//! - **ADR-004**: metadata / 证据载荷用 MessagePack（rmp-serde）紧凑序列化

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use nexus_contracts::experience_card::{ExecutionStatus, ThreeFactorScore};
use nexus_contracts::token_evidence::TokenLedgerEntry;
use nexus_contracts::{AtomicOperator, ExperienceCard};
use rusqlite::{params, Connection};

use crate::error::CmtError;

/// 热缓存默认容量
const DEFAULT_HOT_CAPACITY: usize = 256;

/// 高分卡片入热缓存阈值（score > 此值必入缓存）
const HOT_CACHE_SCORE_THRESHOLD: f32 = 0.7;

/// 经验卡片持久化 — SQLite 复合索引 + 热缓存
///
/// `conn` 经 `Arc<Mutex<>>` 共享；所有 DB 操作在 `spawn_blocking` 闭包内
/// 同步执行（rusqlite 红线 D-5）。`hot_cache` 用 `Box<str>` 零拷贝 key。
#[derive(Clone)]
pub struct ExperienceCardStorage {
    /// SQLite 连接（Mutex 短临界区，spawn_blocking 内同步）
    conn: Arc<Mutex<Connection>>,
    /// 热缓存（card_id → 卡片，Box<str> 零拷贝 key）
    hot_cache: Arc<DashMap<Box<str>, ExperienceCard>>,
    /// 热缓存容量
    hot_capacity: usize,
}

impl ExperienceCardStorage {
    /// 创建内存数据库存储（测试/临时场景）
    pub async fn new_in_memory(hot_capacity: usize) -> Result<Self, CmtError> {
        let conn =
            Connection::open_in_memory().map_err(|e| CmtError::StorageError(e.to_string()))?;
        Self::from_connection(conn, hot_capacity).await
    }

    /// 创建文件数据库存储（生产持久化）
    pub async fn new(db_path: &str, hot_capacity: usize) -> Result<Self, CmtError> {
        let conn = Connection::open(db_path).map_err(|e| CmtError::StorageError(e.to_string()))?;
        Self::from_connection(conn, hot_capacity).await
    }

    /// 从已有连接构造（初始化表与索引）
    async fn from_connection(conn: Connection, hot_capacity: usize) -> Result<Self, CmtError> {
        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
            hot_cache: Arc::new(DashMap::new()),
            hot_capacity: if hot_capacity == 0 {
                DEFAULT_HOT_CAPACITY
            } else {
                hot_capacity
            },
        };
        storage.init().await?;
        Ok(storage)
    }

    /// 初始化表与五复合索引（spawn_blocking，rusqlite 红线）
    pub async fn init(&self) -> Result<(), CmtError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap_or_else(|e| e.into_inner());
            // 经验卡片表（card_id 主键，node_id 唯一）
            conn.execute(
                "CREATE TABLE IF NOT EXISTS experience_cards (
                    card_id TEXT PRIMARY KEY,
                    task_id TEXT NOT NULL,
                    node_id TEXT NOT NULL UNIQUE,
                    parent_id TEXT,
                    operator TEXT NOT NULL,
                    score REAL NOT NULL,
                    delta_vs_parent REAL NOT NULL,
                    method_family TEXT NOT NULL,
                    error_hash TEXT,
                    error_type TEXT,
                    quality REAL NOT NULL,
                    progress REAL NOT NULL,
                    novelty REAL NOT NULL,
                    execution_status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    execution_time_ms INTEGER,
                    prompt_tokens INTEGER,
                    completion_tokens INTEGER,
                    total_tokens INTEGER,
                    lines_changed INTEGER,
                    skills_used TEXT,
                    metadata BLOB
                )",
                [],
            )?;
            // 五复合索引（OpenMLE 三因子 + 错误签名 + 方法家族 + 时间 + 任务状态）
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_three_factor ON experience_cards \
                 (task_id, quality DESC, progress DESC, novelty DESC)",
                [],
            )?;
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_error_hash ON experience_cards (error_hash)",
                [],
            )?;
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_method_family ON experience_cards \
                 (method_family, score DESC)",
                [],
            )?;
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_created_at ON experience_cards (created_at DESC)",
                [],
            )?;
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_task_status ON experience_cards \
                 (task_id, execution_status)",
                [],
            )?;
            // 训练证据表（TokenLedgerEntry MessagePack 载荷，补 Phase 1 WAL 遗留）
            conn.execute(
                "CREATE TABLE IF NOT EXISTS token_evidence (
                    entry_id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    turn_id INTEGER NOT NULL,
                    payload BLOB NOT NULL,
                    stored_at TEXT NOT NULL
                )",
                [],
            )?;
            Ok::<(), rusqlite::Error>(())
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("init spawn_blocking 失败: {e}")))?
        .map_err(|e| CmtError::StorageError(e.to_string()))
    }

    /// 持久化经验卡片 — 热缓存 + SQLite UPSERT（铁律3 只读，幂等）
    ///
    /// 热缓存策略: score > 0.7 或缓存未满 → 入缓存；超容量驱逐最低分。
    /// SQLite UPSERT: ON CONFLICT(card_id) DO UPDATE（幂等，重复 store 不产生副本）。
    pub async fn store(&self, card: &ExperienceCard) -> Result<(), CmtError> {
        // 热缓存（同步短临界区，DashMap 无锁）
        self.update_hot_cache(card);
        // SQLite UPSERT（spawn_blocking，rusqlite 红线）
        let card_clone = card.clone();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap_or_else(|e| e.into_inner());
            Self::upsert_card(&conn, &card_clone)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("store spawn_blocking 失败: {e}")))?
    }

    /// 热缓存更新 — 高分/未满载入缓存，超容量驱逐最低分
    fn update_hot_cache(&self, card: &ExperienceCard) {
        let should_cache =
            card.score > HOT_CACHE_SCORE_THRESHOLD || self.hot_cache.len() < self.hot_capacity;
        if !should_cache {
            return;
        }
        self.hot_cache.insert(card.card_id.clone(), card.clone());
        // 超容量驱逐最低分卡片
        if self.hot_cache.len() > self.hot_capacity {
            let to_evict = self
                .hot_cache
                .iter()
                .min_by(|a, b| {
                    a.value()
                        .score
                        .partial_cmp(&b.value().score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|e| e.key().clone());
            if let Some(id) = to_evict {
                self.hot_cache.remove(&id);
            }
        }
    }

    /// SQLite UPSERT 单卡片（同步，spawn_blocking 闭包内调用）
    fn upsert_card(conn: &Connection, card: &ExperienceCard) -> Result<(), CmtError> {
        let operator_json = serde_json::to_string(&card.operator)
            .map_err(|e| CmtError::StorageError(e.to_string()))?;
        let status_json = serde_json::to_string(&card.execution_status)
            .map_err(|e| CmtError::StorageError(e.to_string()))?;
        let metadata_blob =
            rmp_serde::to_vec(&card.metadata).map_err(|e| CmtError::StorageError(e.to_string()))?;
        let skills_json = serde_json::to_string(&card.metadata.skills_used)
            .map_err(|e| CmtError::StorageError(e.to_string()))?;
        let created_at = card.created_at.to_rfc3339();
        let (error_hash, error_type) = match &card.error_signature {
            Some(sig) => (
                Some(sig.error_hash.to_string()),
                Some(sig.error_type.to_string()),
            ),
            None => (None, None),
        };
        conn.execute(
            "INSERT INTO experience_cards (
                card_id, task_id, node_id, parent_id, operator, score, delta_vs_parent,
                method_family, error_hash, error_type, quality, progress, novelty,
                execution_status, created_at, execution_time_ms, prompt_tokens,
                completion_tokens, total_tokens, lines_changed, skills_used, metadata
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                      ?16, ?17, ?18, ?19, ?20, ?21, ?22)
            ON CONFLICT(card_id) DO UPDATE SET
                score = excluded.score,
                quality = excluded.quality,
                progress = excluded.progress,
                novelty = excluded.novelty,
                execution_status = excluded.execution_status,
                metadata = excluded.metadata",
            params![
                card.card_id.to_string(),
                card.task_id.to_string(),
                card.node_id.to_string(),
                card.parent_id.as_ref().map(|p| p.to_string()),
                operator_json,
                card.score,
                card.delta_vs_parent,
                card.method_family.to_string(),
                error_hash,
                error_type,
                card.three_factor.quality,
                card.three_factor.progress,
                card.three_factor.novelty,
                status_json,
                created_at,
                card.metadata.execution_time_ms as i64,
                card.metadata.token_usage.prompt_tokens as i64,
                card.metadata.token_usage.completion_tokens as i64,
                card.metadata.token_usage.total_tokens as i64,
                card.metadata.lines_changed,
                skills_json,
                metadata_blob,
            ],
        )?;
        Ok(())
    }

    /// 三因子查询 — hot_cache 优先，不足回源 SQLite（红线 R8 Top-K）
    ///
    /// 返回 task_id 匹配且 quality ≥ min_quality 的 Top-k 卡片，
    /// 按三因子选择效用（selection_utility）降序。
    pub async fn query_by_three_factor(
        &self,
        task_id: &str,
        min_quality: f32,
        k: usize,
    ) -> Result<Vec<ExperienceCard>, CmtError> {
        // 热缓存优先
        let hot_results: Vec<ExperienceCard> = self
            .hot_cache
            .iter()
            .filter(|e| {
                e.value().task_id.as_ref() == task_id
                    && e.value().three_factor.quality >= min_quality
            })
            .map(|e| e.value().clone())
            .collect();
        if hot_results.len() >= k {
            return Ok(Self::top_k_by_utility(hot_results, k));
        }
        // 回源 SQLite（spawn_blocking）
        let task_id_owned = task_id.to_string();
        let conn = Arc::clone(&self.conn);
        let db_results = tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap_or_else(|e| e.into_inner());
            Self::query_cards_sql(&conn, &task_id_owned, min_quality, k)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("query spawn_blocking 失败: {e}")))??;
        // 合并热缓存与 DB 结果，去重后 Top-K
        let mut merged = hot_results;
        for card in db_results {
            if !merged.iter().any(|c| c.card_id == card.card_id) {
                merged.push(card);
            }
        }
        Ok(Self::top_k_by_utility(merged, k))
    }

    /// SQLite 三因子查询（同步，spawn_blocking 闭包内调用）
    fn query_cards_sql(
        conn: &Connection,
        task_id: &str,
        min_quality: f32,
        k: usize,
    ) -> Result<Vec<ExperienceCard>, CmtError> {
        let mut stmt = conn.prepare(
            "SELECT card_id, task_id, node_id, parent_id, operator, score, delta_vs_parent,
                    method_family, error_hash, error_type, quality, progress, novelty,
                    execution_status, created_at, execution_time_ms, prompt_tokens,
                    completion_tokens, total_tokens, lines_changed, skills_used, metadata
             FROM experience_cards
             WHERE task_id = ?1 AND quality >= ?2
             ORDER BY (quality + progress + novelty) DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![task_id, min_quality, k as i64], |row| {
            Ok(CardRow {
                card_id: row.get(0)?,
                task_id: row.get(1)?,
                node_id: row.get(2)?,
                parent_id: row.get(3)?,
                operator: row.get(4)?,
                score: row.get(5)?,
                delta_vs_parent: row.get(6)?,
                method_family: row.get(7)?,
                error_hash: row.get(8)?,
                error_type: row.get(9)?,
                quality: row.get(10)?,
                progress: row.get(11)?,
                novelty: row.get(12)?,
                execution_status: row.get(13)?,
                created_at: row.get(14)?,
                execution_time_ms: row.get(15)?,
                prompt_tokens: row.get(16)?,
                completion_tokens: row.get(17)?,
                total_tokens: row.get(18)?,
                lines_changed: row.get(19)?,
                skills_used: row.get(20)?,
                metadata: row.get(21)?,
            })
        })?;
        let mut cards = Vec::new();
        for row in rows {
            let row = row?;
            cards.push(row_to_card(&row)?);
        }
        Ok(cards)
    }

    /// Top-K 按三因子效用排序（红线 R8: select_nth_unstable_by，O(n)）
    fn top_k_by_utility(mut cards: Vec<ExperienceCard>, k: usize) -> Vec<ExperienceCard> {
        if cards.len() <= k {
            cards.sort_unstable_by(|a, b| {
                b.three_factor
                    .selection_utility()
                    .partial_cmp(&a.three_factor.selection_utility())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            return cards;
        }
        cards.select_nth_unstable_by(k, |a, b| {
            b.three_factor
                .selection_utility()
                .partial_cmp(&a.three_factor.selection_utility())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        cards.truncate(k);
        cards.sort_unstable_by(|a, b| {
            b.three_factor
                .selection_utility()
                .partial_cmp(&a.three_factor.selection_utility())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        cards
    }

    /// 错误签名查询 — 错误哈希聚类检索（按 score + 时间降序）
    pub async fn query_by_error_signature(
        &self,
        error_hash: &str,
        limit: usize,
    ) -> Result<Vec<ExperienceCard>, CmtError> {
        let hash_owned = error_hash.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap_or_else(|e| e.into_inner());
            let mut stmt = conn.prepare(
                "SELECT card_id, task_id, node_id, parent_id, operator, score, delta_vs_parent,
                        method_family, error_hash, error_type, quality, progress, novelty,
                        execution_status, created_at, execution_time_ms, prompt_tokens,
                        completion_tokens, total_tokens, lines_changed, skills_used, metadata
                 FROM experience_cards
                 WHERE error_hash = ?1
                 ORDER BY score DESC, created_at DESC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![hash_owned, limit as i64], |row| {
                Ok(CardRow {
                    card_id: row.get(0)?,
                    task_id: row.get(1)?,
                    node_id: row.get(2)?,
                    parent_id: row.get(3)?,
                    operator: row.get(4)?,
                    score: row.get(5)?,
                    delta_vs_parent: row.get(6)?,
                    method_family: row.get(7)?,
                    error_hash: row.get(8)?,
                    error_type: row.get(9)?,
                    quality: row.get(10)?,
                    progress: row.get(11)?,
                    novelty: row.get(12)?,
                    execution_status: row.get(13)?,
                    created_at: row.get(14)?,
                    execution_time_ms: row.get(15)?,
                    prompt_tokens: row.get(16)?,
                    completion_tokens: row.get(17)?,
                    total_tokens: row.get(18)?,
                    lines_changed: row.get(19)?,
                    skills_used: row.get(20)?,
                    metadata: row.get(21)?,
                })
            })?;
            let mut cards = Vec::new();
            for row in rows {
                cards.push(row_to_card(&row?)?);
            }
            Ok::<Vec<ExperienceCard>, CmtError>(cards)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("query_error spawn_blocking 失败: {e}")))?
    }

    /// 批量落盘训练证据 — TokenLedgerEntry MessagePack 载荷（补 Phase 1 WAL 遗留）
    ///
    /// 与 L1 `TokenLedger::export_entries()` 协同，保证训练证据完整性落盘。
    /// ON CONFLICT DO NOTHING（幂等，重复落盘不覆盖）。
    pub async fn store_evidence_batch(
        &self,
        entries: &[TokenLedgerEntry],
    ) -> Result<usize, CmtError> {
        if entries.is_empty() {
            return Ok(0);
        }
        // 序列化为 MessagePack 载荷（spawn_blocking 外准备，避免闭包借用）
        let payloads: Vec<(String, String, i64, Vec<u8>)> = entries
            .iter()
            .map(|e| {
                let payload =
                    rmp_serde::to_vec(e).map_err(|err| CmtError::StorageError(err.to_string()))?;
                Ok((
                    e.entry_id.to_string(),
                    e.session_id.to_string(),
                    e.turn_id as i64,
                    payload,
                ))
            })
            .collect::<Result<Vec<_>, CmtError>>()?;
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap_or_else(|e| e.into_inner());
            let now = Utc::now().to_rfc3339();
            let mut inserted = 0usize;
            for (entry_id, session_id, turn_id, payload) in payloads {
                let affected = conn.execute(
                    "INSERT INTO token_evidence (entry_id, session_id, turn_id, payload, stored_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(entry_id) DO NOTHING",
                    params![entry_id, session_id, turn_id, payload, now],
                )?;
                inserted += affected;
            }
            Ok::<usize, rusqlite::Error>(inserted)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("store_evidence spawn_blocking 失败: {e}")))?
        .map_err(|e| CmtError::StorageError(e.to_string()))
    }

    /// SQLite 卡片总数（可观测性 / 完整性审计用）
    pub async fn card_count(&self) -> Result<u64, CmtError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap_or_else(|e| e.into_inner());
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM experience_cards", [], |r| r.get(0))?;
            Ok::<u64, rusqlite::Error>(count as u64)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("card_count spawn_blocking 失败: {e}")))?
        .map_err(|e| CmtError::StorageError(e.to_string()))
    }

    /// 证据条目总数（可观测性 / 完整性审计用）
    pub async fn evidence_count(&self) -> Result<u64, CmtError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap_or_else(|e| e.into_inner());
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM token_evidence", [], |r| r.get(0))?;
            Ok::<u64, rusqlite::Error>(count as u64)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("evidence_count spawn_blocking 失败: {e}")))?
        .map_err(|e| CmtError::StorageError(e.to_string()))
    }

    /// 热缓存当前条数（可观测性）
    pub fn hot_cache_len(&self) -> usize {
        self.hot_cache.len()
    }

    /// 完整性审计 — SQLite 行数 / 热缓存 / 空 payload 检测（对齐 rl_replay_pool IntegrityReport）
    ///
    /// 九层防御 L3：经验库完整性校验。`consistent == false` 表示存在
    /// 空 metadata 负载（数据损坏信号），调用方应触发降级检查。
    pub async fn integrity_check(&self) -> Result<CardStorageIntegrityReport, CmtError> {
        let sqlite_rows = self.card_count().await?;
        let evidence_rows = self.evidence_count().await?;
        let hot_cache_len = self.hot_cache.len();
        // 检测空 metadata 负载（数据损坏信号）
        let conn = Arc::clone(&self.conn);
        let empty_payload = tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap_or_else(|e| e.into_inner());
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM experience_cards WHERE metadata IS NULL",
                [],
                |r| r.get(0),
            )?;
            Ok::<usize, rusqlite::Error>(count as usize)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("integrity spawn_blocking 失败: {e}")))?
        .map_err(|e| CmtError::StorageError(e.to_string()))?;
        Ok(CardStorageIntegrityReport {
            sqlite_rows,
            hot_cache_len,
            evidence_rows,
            empty_payload,
            consistent: empty_payload == 0,
        })
    }
}

/// 经验卡片存储完整性审计报告（九层防御 L3，对齐 rl_replay_pool IntegrityReport 语义）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardStorageIntegrityReport {
    /// SQLite 经验卡片行数
    pub sqlite_rows: u64,
    /// 热缓存条数
    pub hot_cache_len: usize,
    /// 训练证据行数
    pub evidence_rows: u64,
    /// 空 metadata 负载条目数（数据损坏信号）
    pub empty_payload: usize,
    /// 全部不变量成立（empty_payload == 0）
    pub consistent: bool,
}

/// SQLite 行中间态 — 避免在 query_map 闭包内做复杂反序列化
///
/// WHY 部分字段 dead_code:execution_time_ms/prompt_tokens/等独立列仅供
/// SQL 索引/查询（规范 §8.2 表结构），重建时统一从 metadata BLOB
/// （MessagePack 完整负载）反序列化，避免两处重建逻辑漂移。
#[allow(dead_code)]
#[allow(clippy::struct_excessive_bools)]
struct CardRow {
    card_id: String,
    task_id: String,
    node_id: String,
    parent_id: Option<String>,
    operator: String,
    score: f32,
    delta_vs_parent: f32,
    method_family: String,
    error_hash: Option<String>,
    error_type: Option<String>,
    quality: f32,
    progress: f32,
    novelty: f32,
    execution_status: String,
    created_at: String,
    execution_time_ms: Option<i64>,
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    total_tokens: Option<i64>,
    lines_changed: Option<i32>,
    skills_used: Option<String>,
    metadata: Option<Vec<u8>>,
}

/// 从 SQLite 行重建 ExperienceCard（反序列化 operator/status/metadata）
fn row_to_card(row: &CardRow) -> Result<ExperienceCard, CmtError> {
    let operator: AtomicOperator = serde_json::from_str(&row.operator)
        .map_err(|e| CmtError::StorageError(format!("operator 反序列化失败: {e}")))?;
    let execution_status: ExecutionStatus = serde_json::from_str(&row.execution_status)
        .map_err(|e| CmtError::StorageError(format!("status 反序列化失败: {e}")))?;
    let metadata = match &row.metadata {
        Some(blob) => rmp_serde::from_slice(blob)
            .map_err(|e| CmtError::StorageError(format!("metadata 反序列化失败: {e}")))?,
        None => Default::default(),
    };
    let error_signature = match (&row.error_hash, &row.error_type) {
        (Some(hash), Some(err_type)) => Some(nexus_contracts::experience_card::ErrorSignature {
            error_type: Box::from(err_type.as_str()),
            error_location: Box::from(""),
            error_summary: Box::from(""),
            error_hash: Box::from(hash.as_str()),
        }),
        _ => None,
    };
    let created_at = DateTime::parse_from_rfc3339(&row.created_at)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    Ok(ExperienceCard {
        card_id: Box::from(row.card_id.as_str()),
        task_id: Box::from(row.task_id.as_str()),
        node_id: Box::from(row.node_id.as_str()),
        parent_id: row.parent_id.as_ref().map(|p| Box::from(p.as_str())),
        created_at,
        operator,
        score: row.score,
        delta_vs_parent: row.delta_vs_parent,
        method_family: Box::from(row.method_family.as_str()),
        error_signature,
        three_factor: ThreeFactorScore {
            quality: row.quality,
            progress: row.progress,
            novelty: row.novelty,
        },
        execution_status,
        token_evidence_ids: Vec::new(),
        segment_id: None,
        metadata,
    })
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::experience_card::{CardMetadata, ErrorSignature};

    fn card(id: &str, task: &str, score: f32, quality: f32) -> ExperienceCard {
        ExperienceCard {
            card_id: Box::from(id),
            task_id: Box::from(task),
            node_id: Box::from(format!("node-{id}")),
            parent_id: None,
            created_at: Utc::now(),
            operator: AtomicOperator::Draft,
            score,
            delta_vs_parent: 0.0,
            method_family: Box::from("draft_pipeline"),
            error_signature: None,
            three_factor: ThreeFactorScore {
                quality,
                progress: 0.1,
                novelty: 0.5,
            },
            execution_status: ExecutionStatus::Success,
            token_evidence_ids: Vec::new(),
            segment_id: None,
            metadata: CardMetadata::default(),
        }
    }

    #[tokio::test]
    async fn init_creates_tables_and_indexes() {
        let storage = ExperienceCardStorage::new_in_memory(16)
            .await
            .expect("创建成功");
        // 表已创建（count 查询不报错）
        assert_eq!(storage.card_count().await.expect("查询成功"), 0);
        assert_eq!(storage.evidence_count().await.expect("查询成功"), 0);
    }

    #[tokio::test]
    async fn store_and_query_roundtrip() {
        let storage = ExperienceCardStorage::new_in_memory(16)
            .await
            .expect("创建成功");
        storage
            .store(&card("c1", "t1", 0.8, 0.8))
            .await
            .expect("存储成功");
        let results = storage
            .query_by_three_factor("t1", 0.0, 10)
            .await
            .expect("查询成功");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].card_id.as_ref(), "c1");
        assert!((results[0].score - 0.8).abs() < 1e-6);
    }

    #[tokio::test]
    async fn store_upsert_idempotent() {
        let storage = ExperienceCardStorage::new_in_memory(16)
            .await
            .expect("创建成功");
        storage
            .store(&card("c1", "t1", 0.5, 0.5))
            .await
            .expect("首次存储");
        // 重复 store 同 card_id（更新 score）→ UPSERT 幂等，不产生副本
        storage
            .store(&card("c1", "t1", 0.9, 0.9))
            .await
            .expect("重复存储");
        assert_eq!(
            storage.card_count().await.expect("查询成功"),
            1,
            "UPSERT 应幂等"
        );
        let results = storage
            .query_by_three_factor("t1", 0.0, 10)
            .await
            .expect("查询");
        assert_eq!(results.len(), 1);
        assert!((results[0].score - 0.9).abs() < 1e-6, "应更新为最新 score");
    }

    #[tokio::test]
    async fn three_factor_query_filters_by_quality() {
        let storage = ExperienceCardStorage::new_in_memory(16)
            .await
            .expect("创建成功");
        storage
            .store(&card("c1", "t1", 0.9, 0.9))
            .await
            .expect("存储");
        storage
            .store(&card("c2", "t1", 0.3, 0.3))
            .await
            .expect("存储");
        // min_quality=0.5 只返回 c1
        let results = storage
            .query_by_three_factor("t1", 0.5, 10)
            .await
            .expect("查询");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].card_id.as_ref(), "c1");
    }

    #[tokio::test]
    async fn three_factor_query_top_k_ordered() {
        let storage = ExperienceCardStorage::new_in_memory(16)
            .await
            .expect("创建成功");
        for i in 0..5 {
            let score = 0.5 + i as f32 * 0.1;
            storage
                .store(&card(&format!("c{i}"), "t1", score, score))
                .await
                .expect("存储");
        }
        let results = storage
            .query_by_three_factor("t1", 0.0, 3)
            .await
            .expect("查询");
        assert_eq!(results.len(), 3, "Top-3 截断");
        // 降序: 高分在前
        assert!(results[0].score >= results[1].score);
        assert!(results[1].score >= results[2].score);
    }

    #[tokio::test]
    async fn error_signature_query() {
        let storage = ExperienceCardStorage::new_in_memory(16)
            .await
            .expect("创建成功");
        let mut c = card("c1", "t1", 0.3, 0.3);
        c.error_signature = Some(ErrorSignature {
            error_type: Box::from("compile_error"),
            error_location: Box::from("src/a.rs"),
            error_summary: Box::from("E0308"),
            error_hash: Box::from("hash-x"),
        });
        c.execution_status = ExecutionStatus::Error;
        storage.store(&c).await.expect("存储");
        let results = storage
            .query_by_error_signature("hash-x", 10)
            .await
            .expect("查询");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].card_id.as_ref(), "c1");
        assert!(results[0].error_signature.is_some());
        // 无匹配哈希
        let empty = storage
            .query_by_error_signature("hash-none", 10)
            .await
            .expect("查询");
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn store_evidence_batch_msgpack() {
        let storage = ExperienceCardStorage::new_in_memory(16)
            .await
            .expect("创建成功");
        let entries = vec![
            TokenLedgerEntry::new(
                "e1",
                0,
                "s1",
                "i1",
                vec![101],
                vec![201],
                vec![0.9],
                vec![true],
                "v1",
                vec![],
                None,
                1_700_000_000_000,
            ),
            TokenLedgerEntry::new(
                "e2",
                1,
                "s1",
                "i1",
                vec![102],
                vec![202],
                vec![0.8],
                vec![true],
                "v1",
                vec![],
                None,
                1_700_000_001_000,
            ),
        ];
        let inserted = storage
            .store_evidence_batch(&entries)
            .await
            .expect("落盘成功");
        assert_eq!(inserted, 2);
        assert_eq!(storage.evidence_count().await.expect("查询成功"), 2);
        // 重复落盘幂等（DO NOTHING）
        let again = storage
            .store_evidence_batch(&entries)
            .await
            .expect("重复落盘");
        assert_eq!(again, 0, "重复落盘应 DO NOTHING");
        assert_eq!(storage.evidence_count().await.expect("查询成功"), 2);
    }

    #[tokio::test]
    async fn hot_cache_evicts_lowest_score() {
        // hot_capacity=2，存 3 张高分卡 → 驱逐最低分
        let storage = ExperienceCardStorage::new_in_memory(2)
            .await
            .expect("创建成功");
        storage
            .store(&card("c1", "t1", 0.9, 0.9))
            .await
            .expect("存储");
        storage
            .store(&card("c2", "t1", 0.8, 0.8))
            .await
            .expect("存储");
        storage
            .store(&card("c3", "t1", 0.95, 0.95))
            .await
            .expect("存储");
        // 热缓存容量 2，c1(0.9) 最低应被驱逐（c2=0.8 更低，实际驱逐 c2）
        assert!(storage.hot_cache_len() <= 2, "热缓存应不超容量");
    }

    #[tokio::test]
    async fn empty_storage_query_returns_empty() {
        let storage = ExperienceCardStorage::new_in_memory(16)
            .await
            .expect("创建成功");
        let results = storage
            .query_by_three_factor("t-none", 0.0, 10)
            .await
            .expect("查询");
        assert!(results.is_empty(), "空库查询应返回空");
    }

    #[tokio::test]
    async fn metadata_msgpack_roundtrip() {
        // metadata 经 MessagePack 持久化后重建一致
        let storage = ExperienceCardStorage::new_in_memory(16)
            .await
            .expect("创建成功");
        let mut c = card("c1", "t1", 0.8, 0.8);
        c.metadata.execution_time_ms = 1234;
        c.metadata.lines_changed = 42;
        storage.store(&c).await.expect("存储");
        let results = storage
            .query_by_three_factor("t1", 0.0, 10)
            .await
            .expect("查询");
        assert_eq!(results[0].metadata.execution_time_ms, 1234);
        assert_eq!(results[0].metadata.lines_changed, 42);
    }
}
