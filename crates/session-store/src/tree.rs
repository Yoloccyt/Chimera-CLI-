//! SQLite 树索引 — segments / events 两表 + fork 零拷贝（v4.0 WI-18 存储面）
//!
//! 对应架构层: **L3 Storage**（session-store,Phase 2 新增,ADR-141）
//! 对应任务: **P2-T2**（v4.0 WI-18:append-only JSONL 段 + SQLite 树索引 + fork 零拷贝）
//!
//! # Schema（WI-18）
//!
//! ```sql
//! segments(segment_id PK, session_id, segment_index, parent_segment_id,
//!          start_offset, end_offset)
//! events(offset, session_id, segment_id, payload BLOB)  -- 复合主键 (session_id, offset)
//! ```
//!
//! - `events.payload` = rmp-serde `to_vec_named`（map 编码,ADR-004 协议）序列化的
//!   `SessionEvent`;紧凑索引格式,与段文件 JSONL 人读格式双轨并存。
//!   WHY named:EventMetadata.correlation_id 带 skip_serializing_if,array 位置编码
//!   下字段错位（ADR-004 同坑,event-bus 先例 shard.rs/bus.rs）,map 按字段名
//!   匹配不受 skip 影响
//! - `parent_segment_id` 非 NULL 的行 = fork 复制行,指向父会话的段
//! - 复合索引 `(session_id, offset)`:P2-T3 k-way 归并回放的按会话顺序扫描路径
//!
//! # WAL 模式（项目红线）
//!
//! `PRAGMA journal_mode=WAL` 必须经 `Connection::pragma_update` 设置
//! （对齐 scc-cache wal.rs 先例）;WAL 模式允许读-写并发,微批写入不阻塞
//! 读取路径（ADR-108 读写分区放大）。
//!
//! # fork 零拷贝（WI-18）
//!
//! `fork(session_id, from_offset)` 复制**前缀段元数据行**到新会话
//! （新行 `segment_id` 重新分配,`parent_segment_id` 指向父段原行）,
//! **零事件数据拷贝**:
//! - `events` 表零行插入（事件 payload 仍在父段名下,回查经引用链）
//! - JSONL 段文件零复制（无新文件;仅元数据行复制）
//!
//! 最小实现选「仅元数据 + 引用」而非硬链接/引用计数:跨平台（Windows 硬链接
//! 需 SeCreateSymbolicLinkPrivilege）、无权限依赖、语义最简——引用链天然
//! 支持「父段事件对新会话可见」（树索引回查）。
//!
//! # 线程安全
//!
//! `Arc<Mutex<rusqlite::Connection>>`（`Connection` 非 Sync）;所有方法为
//! **同步**签名,由上层经 `tokio::task::spawn_blocking` 调用（红线 3:
//! SQLite 写入只经 spawn_blocking;同步方法仅供阻塞池/无 runtime 环境使用）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::{params, OptionalExtension};
use tracing::debug;

use crate::error::StoreError;
use crate::segment::{file_stem, SegmentFileReader, SegmentMeta};
use crate::types::{SegmentId, SessionEvent, SessionId};

/// SQLite 树索引 — segments / events 两表的封装
#[derive(Clone)]
pub struct TreeIndex {
    conn: Arc<Mutex<rusqlite::Connection>>,
    /// SQLite 写事务次数（基准指标:单条直写 = N 次,微批 = ceil(N/64) 次）
    transaction_count: Arc<AtomicU64>,
}

/// 待写入索引的事件行（payload 由 TreeIndex 内部 rmp-serde 序列化）
#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    /// 全局序列号（段内 Offset.seq）
    pub offset: u64,
    /// 所属会话
    pub session_id: SessionId,
    /// 所属段（segments 表主键）
    pub segment_id: SegmentId,
    /// 事件本体
    pub event: SessionEvent,
}

/// 从索引读回的事件（offset + 反序列化事件）
#[derive(Debug, Clone, PartialEq)]
pub struct StoredEvent {
    /// 全局序列号
    pub offset: u64,
    /// 事件本体
    pub event: SessionEvent,
}

/// 段节点 — 会话树的最小单元（WI-18:节点 = segment,`parent_segment_id` 指针）
///
/// # WHY 自包含快照
/// 与 `segments` 表行同构的纯数据快照,树查询（tree/fork_tree/ancestors）
/// 一次性读入,避免逐节点往返 SQLite。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentNode {
    /// 段主键
    pub segment_id: SegmentId,
    /// 所属会话
    pub session_id: SessionId,
    /// 会话内段索引
    pub segment_index: u32,
    /// 父段引用（fork 复制行为非 NULL,指向父会话段;非 fork 段为 None）
    pub parent_segment_id: Option<SegmentId>,
    /// 段内起始全局序列号
    pub start_offset: u64,
    /// 段内结束全局序列号（含;fork 截断段 = fork 点 - 1）
    pub end_offset: u64,
}

/// 会话树（WI-18）— 会话的段节点集合 + parent 指针形成的树
///
/// 节点均为**该会话的 segments 行**;fork 复制行的 `parent_segment_id`
/// 指向父会话段（跨会话边）,父段节点本身不在本树内（经
/// [`TreeIndex::fork_tree`] 可取得包含祖先节点的完整视图）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTree {
    /// 所属会话
    pub session_id: SessionId,
    /// 该会话的全部段节点（按段索引升序）
    pub nodes: Vec<SegmentNode>,
}

impl SessionTree {
    /// 根节点 — parent_segment_id 为 NULL 的段（非 fork 复制段）
    #[must_use]
    pub fn roots(&self) -> Vec<&SegmentNode> {
        self.nodes
            .iter()
            .filter(|n| n.parent_segment_id.is_none())
            .collect()
    }

    /// 指定父段（segment_id）的直接子节点（本会话内）
    #[must_use]
    pub fn children_of(&self, parent: &SegmentId) -> Vec<&SegmentNode> {
        self.nodes
            .iter()
            .filter(|n| n.parent_segment_id.as_ref() == Some(parent))
            .collect()
    }

    /// 按段 ID 查节点
    #[must_use]
    pub fn node(&self, segment_id: &SegmentId) -> Option<&SegmentNode> {
        self.nodes.iter().find(|n| &n.segment_id == segment_id)
    }

    /// 节点数
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

/// 会话可见段的来源解析结果 — 定位物理段文件（k-way 归并回放的输入）
///
/// # WHY 物理解析
/// fork 复制行的 session_id 是子会话（无物理文件）,事件数据实际在父会话
/// 段名下;回放必须沿 parent 引用链解析到**物理写入会话**才能打开文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentSource {
    /// 段主键（本会话视角）
    pub segment_id: SegmentId,
    /// 物理文件所属会话（沿 parent 链解析到最底层物理段）
    pub file_session: SessionId,
    /// 物理段文件段索引
    pub segment_index: u32,
    /// 父段引用（None = 物理段）
    pub parent_segment_id: Option<SegmentId>,
    /// 段起始全局序列号
    pub start_offset: u64,
    /// 会话可见段尾（fork 截断段 = fork 点 - 1）
    pub end_offset: u64,
}

/// 索引重建统计 — 崩溃自愈的观察结果（测试断言幂等用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebuildStats {
    /// 扫描的段文件数
    pub segments_scanned: u64,
    /// 新插入的 events 行数
    pub rows_inserted: u64,
    /// 已存在而跳过的 events 行数（去重;幂等性来源）
    pub rows_skipped: u64,
}

impl TreeIndex {
    /// 打开或创建 SQLite 树索引（WAL 模式 + 建表 + 建索引）
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = rusqlite::Connection::open(path).map_err(|e| StoreError::Sqlite {
            reason: format!("打开树索引数据库 {} 失败: {e}", path.display()),
        })?;
        Self::init(conn)
    }

    /// 打开内存树索引（测试 / 基准用,不落盘）
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = rusqlite::Connection::open_in_memory().map_err(|e| {
            StoreError::Sqlite {
                reason: format!("打开内存树索引失败: {e}"),
            }
        })?;
        Self::init(conn)
    }

    /// 公共初始化:WAL pragma（红线）+ 建表 + 建索引
    fn init(conn: rusqlite::Connection) -> Result<Self, StoreError> {
        // 红线:journal_mode=WAL 必须 pragma_update(而非 execute 拼 SQL)
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| StoreError::Sqlite {
                reason: format!("设置 journal_mode=WAL 失败: {e}"),
            })?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS segments (
                segment_id        TEXT    PRIMARY KEY,
                session_id        TEXT    NOT NULL,
                segment_index     INTEGER NOT NULL,
                parent_segment_id TEXT,
                start_offset      INTEGER NOT NULL,
                end_offset        INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_segments_session
                ON segments(session_id, segment_index);
            CREATE TABLE IF NOT EXISTS events (
                offset     INTEGER NOT NULL,
                session_id TEXT    NOT NULL,
                segment_id TEXT    NOT NULL,
                payload    BLOB    NOT NULL,
                PRIMARY KEY (session_id, offset)
            );
            -- 复合索引:按会话 + offset 的顺序扫描(P2-T3 回放路径)
            CREATE INDEX IF NOT EXISTS idx_events_session_offset
                ON events(session_id, offset);",
        )
        .map_err(|e| StoreError::Sqlite {
            reason: format!("初始化树索引表结构失败: {e}"),
        })?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            transaction_count: Arc::new(AtomicU64::new(0)),
        })
    }

    /// 插入或更新段元数据行（upsert:保留首次 start_offset,更新 end_offset）
    ///
    /// WHY upsert:同一段跨多次微批 flush,end_offset 递增推进;start_offset
    /// 必须保持段首事件 seq（重建语义,覆盖会破坏 fork 覆盖校验）。
    pub fn insert_segment(&self, meta: &SegmentMeta) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("segments 表锁中毒: {p}"),
        })?;
        conn.execute(
            "INSERT INTO segments
                (segment_id, session_id, segment_index, parent_segment_id, start_offset, end_offset)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(segment_id) DO UPDATE SET
                end_offset = excluded.end_offset",
            params![
                meta.segment_id.as_str(),
                meta.session_id.as_str(),
                meta.segment_index,
                meta.parent_segment_id.as_ref().map(SegmentId::as_str),
                meta.start_offset,
                meta.end_offset,
            ],
        )
        .map_err(|e| StoreError::Sqlite {
            reason: format!("插入段元数据 {} 失败: {e}", meta.segment_id),
        })?;
        Ok(())
    }

    /// 批量插入事件行（单事务,写操作计数 +1）——微批写索引面入口
    ///
    /// # 计数语义（bench 门禁）
    /// 每次调用 = 一次 SQLite 写事务:单条直写 N 次调用 = N 次写;
    /// 微批写 N 条 = ceil(N/batch_size) 次调用。`transaction_count()` 供
    /// bench 计算 `syscall_reduction_pct`。
    pub fn insert_events(&self, rows: &[EventRow]) -> Result<(), StoreError> {
        if rows.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("events 表锁中毒: {p}"),
        })?;
        // 单事务批量:微批的 SQLite 面收益(一次 BEGIN/COMMIT = 一次写事务)
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| StoreError::Sqlite {
                reason: format!("开启写事务失败: {e}"),
            })?;
        let result = (|| -> Result<(), StoreError> {
            {
                let mut stmt = conn
                    .prepare(
                        "INSERT INTO events (offset, session_id, segment_id, payload)
                         VALUES (?1, ?2, ?3, ?4)",
                    )
                    .map_err(|e| StoreError::Sqlite {
                        reason: format!("预编译事件插入语句失败: {e}"),
                    })?;
                for row in rows {
                    // WHY to_vec_named(map 编码)而非 to_vec(array 位置编码):
                    // L0 EventMetadata.correlation_id 带 skip_serializing_if,array
                    // 编码下字段错位(ADR-004 同坑,event-bus 先例 shard.rs/bus.rs
                    // 均用 to_vec_named);named 按字段名匹配,skip 不影响反序列化
                    let payload = rmp_serde::to_vec_named(&row.event).map_err(|e| {
                        StoreError::Serialization {
                            reason: format!("SessionEvent rmp 序列化失败: {e}"),
                        }
                    })?;
                    stmt.execute(params![
                        row.offset,
                        row.session_id.as_str(),
                        row.segment_id.as_str(),
                        payload,
                    ])
                    .map_err(|e| StoreError::Sqlite {
                        reason: format!(
                            "插入事件 offset={} 失败: {e}",
                            row.offset
                        ),
                    })?;
                }
            }
            Ok(())
        })();
        // 无论成败都提交/回滚,释放事务(不回滚会滞留写锁)
        match result {
            Ok(()) => conn.execute_batch("COMMIT").map_err(|e| StoreError::Sqlite {
                reason: format!("提交写事务失败: {e}"),
            })?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        self.transaction_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// fork 会话 — 复制前缀段元数据到新会话（零事件数据拷贝）
    ///
    /// # 参数
    /// - `session_id`:父会话
    /// - `from_offset`:fork 点(全局 seq);前缀 = `[0, from_offset)` 的事件
    ///
    /// # 语义
    /// - 新会话事件 seq 从 `from_offset` 起(调用方以 `from_offset` 作为
    ///   新会话首段 start_seq),回放时前缀事件 + 新事件按 seq 无缝拼接
    /// - 复制段 = `start_offset < from_offset` 的段;跨 fork 点的段被截断
    ///   （end_offset = from_offset - 1）;覆盖校验失败 → `ForkViolation`
    /// - 零数据拷贝:events 表零插入、JSONL 段文件零创建（见模块文档）
    ///
    /// # 返回
    /// 新会话 ID（UUIDv7）
    pub fn fork(&self, session_id: &SessionId, from_offset: u64) -> Result<SessionId, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("fork 锁中毒: {p}"),
        })?;

        // 1. 读父会话全部段(按段索引升序)
        let mut stmt = conn
            .prepare(
                "SELECT segment_id, segment_index, parent_segment_id, start_offset, end_offset
                 FROM segments WHERE session_id = ?1 ORDER BY segment_index ASC",
            )
            .map_err(|e| StoreError::Sqlite {
                reason: format!("预编译父段查询失败: {e}"),
            })?;
        let parent_rows = stmt
            .query_map(params![session_id.as_str()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })
            .map_err(|e| StoreError::Sqlite {
                reason: format!("查询父会话 {} 段失败: {e}", session_id),
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Sqlite {
                reason: format!("读取父会话段行失败: {e}"),
            })?;

        // 2. 过滤前缀段 + 部分截断
        let mut copied: Vec<(String, i64, Option<String>, i64, i64)> = Vec::new();
        for (seg_id, idx, parent, start, end) in parent_rows {
            if (start as u64) < from_offset {
                let mut end_adj = end as u64;
                if (end as u64) >= from_offset {
                    end_adj = from_offset - 1; // 跨 fork 点:截断
                }
                copied.push((seg_id, idx, parent, start, end_adj as i64));
            }
        }

        // 3. 覆盖校验:前缀必须覆盖 [0, from_offset)
        if from_offset > 0 {
            if copied.is_empty() {
                return Err(StoreError::ForkViolation {
                    reason: format!(
                        "父会话 {} 在 fork 点 {from_offset} 前无任何段(可能 fork 点超出历史)",
                        session_id
                    ),
                });
            }
            let first_start = copied.first().map(|c| c.3 as u64).unwrap_or(0);
            let last_end = copied.last().map(|c| c.4 as u64).unwrap_or(0);
            if first_start != 0 {
                return Err(StoreError::ForkViolation {
                    reason: format!(
                        "前缀段覆盖不连续:首个复制段 start_offset={first_start} != 0"
                    ),
                });
            }
            if last_end != from_offset - 1 {
                return Err(StoreError::ForkViolation {
                    reason: format!(
                        "前缀段未覆盖到 fork 点:最后复制段 end_offset={last_end}, 期望 {}",
                        from_offset - 1
                    ),
                });
            }
        }

        // 4. 插入复制行(新 segment_id + parent 引用)+ 新会话 ID
        let new_session = SessionId::generate();
        {
            let mut stmt = conn
                .prepare(
                    "INSERT INTO segments
                        (segment_id, session_id, segment_index, parent_segment_id, start_offset, end_offset)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(|e| StoreError::Sqlite {
                    reason: format!("预编译 fork 复制行插入失败: {e}"),
                })?;
            for (parent_seg, idx, _, start, end) in &copied {
                stmt.execute(params![
                    SegmentId::generate().as_str(),
                    new_session.as_str(),
                    idx,
                    Some(parent_seg.as_str()),
                    start,
                    end,
                ])
                .map_err(|e| StoreError::Sqlite {
                    reason: format!("插入 fork 复制段行失败: {e}"),
                })?;
            }
        }
        debug!(
            "fork 会话 {} -> {}: 复制 {} 个前缀段元数据(零事件数据拷贝), fork 点 {}",
            session_id,
            new_session,
            copied.len(),
            from_offset
        );
        Ok(new_session)
    }

    /// 按会话读回事件（前缀引用段 + 自己段,按 seq 升序）——树索引回读路径
    ///
    /// # 参数
    /// - `session_id`:目标会话（可为 fork 产生的新会话）
    /// - `from`:可选 seq 下界（断点续传,None = 从头）
    ///
    /// # 合并语义
    /// 前缀事件（经 parent_segment_id 引用,offset < fork 点）天然排在
    /// 自己段事件（offset >= fork 点）之前,直接拼接即为全序（ADR-109 归并预演）。
    pub fn read_events(
        &self,
        session_id: &SessionId,
        from: Option<u64>,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("读回锁中毒: {p}"),
        })?;
        let from = from.unwrap_or(0);

        // 前缀引用段的 end 上限(= fork 点 - 1;无前缀段为 NULL)
        let prefix_end: Option<i64> = conn
            .query_row(
                "SELECT MAX(end_offset) FROM segments
                 WHERE session_id = ?1 AND parent_segment_id IS NOT NULL",
                params![session_id.as_str()],
                |r| r.get(0),
            )
            .map_err(|e| StoreError::Sqlite {
                reason: format!("查询前缀段 end 上限失败: {e}"),
            })?;

        // 前缀事件(来自父段引用)
        let mut prefix: Vec<StoredEvent> = Vec::new();
        if let Some(pe) = prefix_end {
            if from <= (pe as u64) {
                let mut stmt = conn
                    .prepare(
                        "SELECT e.offset, e.payload FROM events e
                         WHERE e.segment_id IN (
                            SELECT parent_segment_id FROM segments
                            WHERE session_id = ?1 AND parent_segment_id IS NOT NULL
                         )
                         AND e.offset >= ?2 AND e.offset <= ?3
                         ORDER BY e.offset ASC",
                    )
                    .map_err(|e| StoreError::Sqlite {
                        reason: format!("预编译前缀事件查询失败: {e}"),
                    })?;
                let rows = stmt
                    .query_map(params![session_id.as_str(), from, pe], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
                    })
                    .map_err(|e| StoreError::Sqlite {
                        reason: format!("查询前缀事件失败: {e}"),
                    })?;
                for row in rows {
                    let (off, payload) = row.map_err(|e| StoreError::Sqlite {
                        reason: format!("读取前缀事件行失败: {e}"),
                    })?;
                    let event: SessionEvent = rmp_serde::from_slice(&payload).map_err(|e| {
                        StoreError::Serialization {
                            reason: format!("前缀事件 rmp 反序列化失败: {e}"),
                        }
                    })?;
                    prefix.push(StoredEvent {
                        offset: off as u64,
                        event,
                    });
                }
            }
        }

        // 自己段事件
        let mut stmt = conn
            .prepare(
                "SELECT offset, payload FROM events
                 WHERE session_id = ?1 AND offset >= ?2
                 ORDER BY offset ASC",
            )
            .map_err(|e| StoreError::Sqlite {
                reason: format!("预编译本段事件查询失败: {e}"),
            })?;
        let rows = stmt
            .query_map(params![session_id.as_str(), from], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|e| StoreError::Sqlite {
                reason: format!("查询本段事件失败: {e}"),
            })?;
        let mut own: Vec<StoredEvent> = Vec::new();
        for row in rows {
            let (off, payload) = row.map_err(|e| StoreError::Sqlite {
                reason: format!("读取本段事件行失败: {e}"),
            })?;
            let event: SessionEvent = rmp_serde::from_slice(&payload).map_err(|e| {
                StoreError::Serialization {
                    reason: format!("本段事件 rmp 反序列化失败: {e}"),
                }
            })?;
            own.push(StoredEvent {
                offset: off as u64,
                event,
            });
        }

        // 拼接:前缀(< fork 点) + 自己(>= fork 点),天然全序
        prefix.extend(own);
        Ok(prefix)
    }

    /// 会话树（WI-18）— 该会话全部段节点 + parent 指针
    pub fn tree(&self, session_id: &SessionId) -> Result<SessionTree, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("会话树锁中毒: {p}"),
        })?;
        let nodes = query_segment_nodes(&conn, session_id)?;
        Ok(SessionTree {
            session_id: session_id.clone(),
            nodes,
        })
    }

    /// 按段 ID 查段节点（fork_tree 跨会话收集祖先节点用）
    pub fn segment_node(&self, segment_id: &SegmentId) -> Result<SegmentNode, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("段节点查询锁中毒: {p}"),
        })?;
        let node: Option<SegmentNode> = conn
            .query_row(
                "SELECT segment_id, session_id, segment_index, parent_segment_id, start_offset, end_offset
                 FROM segments WHERE segment_id = ?1",
                params![segment_id.as_str()],
                row_to_node,
            )
            .optional()
            .map_err(|e| StoreError::Sqlite {
                reason: format!("查询段节点 {} 失败: {e}", segment_id),
            })?;
        node.ok_or_else(|| StoreError::NotFound {
            what: format!("段 {} 不存在", segment_id),
        })
    }

    /// fork 树视图（WI-18）— fork 子会话的完整树:自己会话节点 + 沿 parent
    /// 引用链收集的祖先段节点（跨会话,去重）
    ///
    /// # WHY 完整视图
    /// `tree()` 只含本会话 segments 行,parent 指针指向外部节点（父段）;
    /// fork_tree 沿指针把祖先节点也拉入 nodes,使树从根到叶完整可遍历。
    pub fn fork_tree(&self, session_id: &SessionId) -> Result<SessionTree, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("fork 树锁中毒: {p}"),
        })?;
        let mut nodes = query_segment_nodes(&conn, session_id)?;
        // 沿 parent 链收集祖先节点（跨会话）;seen 防环（fork 链理论无环,
        // 防御性去重避免异常数据死循环）
        let mut seen: std::collections::HashSet<SegmentId> = nodes
            .iter()
            .map(|n| n.segment_id.clone())
            .collect();
        let mut frontier: Vec<SegmentId> = nodes
            .iter()
            .filter_map(|n| n.parent_segment_id.clone())
            .collect();
        while let Some(pid) = frontier.pop() {
            if !seen.insert(pid.clone()) {
                continue;
            }
            let parent_node = query_segment_node(&conn, &pid)?;
            if let Some(gp) = parent_node.parent_segment_id.clone() {
                frontier.push(gp);
            }
            nodes.push(parent_node);
        }
        // 祖先节点按段索引升序稳定化输出顺序（原顺序为 BFS 收集序）
        nodes.sort_by_key(|n| n.segment_index);
        Ok(SessionTree {
            session_id: session_id.clone(),
            nodes,
        })
    }

    /// 祖先链（WI-18）— 会话的祖先段 ID,从直接父层到根层（每层内按
    /// segment_id 字典序保证确定性输出）
    ///
    /// # 示例
    /// 链式 fork A → B → C:ancestors(C) = [B 的段, A 的段]
    pub fn ancestors(&self, session_id: &SessionId) -> Result<Vec<SegmentId>, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("祖先链锁中毒: {p}"),
        })?;
        let nodes = query_segment_nodes(&conn, session_id)?;
        let mut out: Vec<SegmentId> = Vec::new();
        let mut seen: std::collections::HashSet<SegmentId> = std::collections::HashSet::new();
        // 分层 BFS:layer = 当前层的直接父段集合,处理完再推入祖父层
        let mut layer: std::collections::HashSet<SegmentId> = nodes
            .iter()
            .filter_map(|n| n.parent_segment_id.clone())
            .collect();
        while !layer.is_empty() {
            let mut next: std::collections::HashSet<SegmentId> = std::collections::HashSet::new();
            let mut ordered: Vec<SegmentId> = layer.iter().cloned().collect();
            ordered.sort();
            for pid in ordered {
                if !seen.insert(pid.clone()) {
                    continue;
                }
                out.push(pid.clone());
                let node = query_segment_node(&conn, &pid)?;
                if let Some(gp) = node.parent_segment_id {
                    next.insert(gp);
                }
            }
            layer = next;
        }
        Ok(out)
    }

    /// 会话可见段来源解析 — k-way 归并回放的段文件定位（ADR-109）
    ///
    /// 对会话每段（含 fork 复制行）沿 parent 链解析到物理文件;fork 复制行
    /// 的 `end_offset` 保持会话视角（截断段 = fork 点 - 1）,回放按此过滤。
    pub fn segment_sources(&self, session_id: &SessionId) -> Result<Vec<SegmentSource>, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("段来源解析锁中毒: {p}"),
        })?;
        let nodes = query_segment_nodes(&conn, session_id)?;
        let mut out = Vec::with_capacity(nodes.len());
        for node in nodes {
            let (file_session, file_idx) =
                resolve_physical(&conn, &node.segment_id, 0)?;
            out.push(SegmentSource {
                segment_id: node.segment_id,
                file_session,
                segment_index: file_idx,
                parent_segment_id: node.parent_segment_id,
                start_offset: node.start_offset,
                end_offset: node.end_offset,
            });
        }
        Ok(out)
    }

    /// 索引重建（崩溃自愈,幂等）— 段文件完整但 SQLite 索引缺行时扫描段文件
    /// 重建 segments/events 行
    ///
    /// # 场景（T2 审查 Important #2 承接）
    /// `flush_sync` 中段文件 fsync 与索引 insert 非原子:崩溃窗口可能出现
    /// 「段文件已 fsync、索引缺行」。本方法以段文件为权威源自愈。
    ///
    /// # 幂等性
    /// events 行用 `INSERT OR IGNORE`（已存在 offset 跳过）;segments 行
    /// upsert 保留 start_offset。重复调用无副作用（第二次 rows_inserted = 0,
    /// rows_skipped = 全量）。
    ///
    /// # 范围
    /// 只重建该会话**物理拥有**的段文件（`<stem>.<idx>.jsonl`）;fork 复制行
    /// 不重建（其 events 行本来就零拷贝,事件经父段引用可见）。
    pub fn rebuild_index(
        &self,
        data_dir: &Path,
        session_id: &SessionId,
    ) -> Result<RebuildStats, StoreError> {
        // 1. 枚举段文件（无锁 IO）:`<stem>.<idx>.jsonl`,按 idx 升序
        let stem = file_stem(session_id);
        let prefix = format!("{stem}.");
        let mut files: Vec<(u32, PathBuf)> = Vec::new();
        let entries = std::fs::read_dir(data_dir).map_err(|e| StoreError::Io {
            context: format!("读取数据目录 {} 失败(rebuild_index)", data_dir.display()),
            source: e,
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| StoreError::Io {
                context: "读取数据目录条目失败(rebuild_index)".to_string(),
                source: e,
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let rest = name
                .strip_prefix(&prefix)
                .and_then(|r| r.strip_suffix(".jsonl"));
            if let Some(idx_str) = rest {
                if let Ok(idx) = idx_str.parse::<u32>() {
                    files.push((idx, entry.path()));
                }
            }
        }
        files.sort_by_key(|(idx, _)| *idx);

        // 2. 读全部段文件记录（无锁 IO;半条停止——已确认记录才重建）
        let mut scanned: Vec<(u32, Vec<crate::segment::SegmentRecord>)> = Vec::new();
        for (idx, path) in &files {
            let mut reader = SegmentFileReader::open(path)?;
            let mut records = Vec::new();
            while let Some(rec) = reader.next_record()? {
                records.push(rec);
            }
            // 空段文件跳过:无记录无法推断 start_seq,后续 writer flush 会补齐
            if !records.is_empty() {
                scanned.push((*idx, records));
            }
        }

        // 3. 锁内批量写库（单事务;upsert segments + INSERT OR IGNORE events）
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("rebuild 锁中毒: {p}"),
        })?;
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| StoreError::Sqlite {
                reason: format!("rebuild 开启事务失败: {e}"),
            })?;
        let result = (|| -> Result<RebuildStats, StoreError> {
            let mut stats = RebuildStats {
                segments_scanned: scanned.len() as u64,
                rows_inserted: 0,
                rows_skipped: 0,
            };
            {
                let mut stmt_seg = conn
                    .prepare(
                        "INSERT INTO segments
                            (segment_id, session_id, segment_index, parent_segment_id, start_offset, end_offset)
                         VALUES (?1, ?2, ?3, NULL, ?4, ?5)
                         ON CONFLICT(segment_id) DO UPDATE SET end_offset = excluded.end_offset",
                    )
                    .map_err(|e| StoreError::Sqlite {
                        reason: format!("rebuild 预编译段 upsert 失败: {e}"),
                    })?;
                let mut stmt_ev = conn
                    .prepare(
                        "INSERT OR IGNORE INTO events (offset, session_id, segment_id, payload)
                         VALUES (?1, ?2, ?3, ?4)",
                    )
                    .map_err(|e| StoreError::Sqlite {
                        reason: format!("rebuild 预编译事件插入失败: {e}"),
                    })?;
                for (idx, records) in &scanned {
                    // 段 ID:复用已有（索引残留行）或生成新（崩溃缺行）
                    let seg_id = query_segment_id(&conn, session_id, *idx)?.unwrap_or_else(SegmentId::generate);
                    let start = records
                        .first()
                        .map(|r| r.seq)
                        .ok_or_else(|| StoreError::InvalidInput {
                            reason: "rebuild 空段文件不可达(已在上层过滤)".into(),
                        })?;
                    let end = records.last().map(|r| r.seq).unwrap_or(start);
                    stmt_seg
                        .execute(params![
                            seg_id.as_str(),
                            session_id.as_str(),
                            idx,
                            start,
                            end,
                        ])
                        .map_err(|e| StoreError::Sqlite {
                            reason: format!("rebuild upsert 段 {} 失败: {e}", seg_id),
                        })?;
                    for rec in records {
                        // WHY to_vec_named:与 insert_events 同源（map 编码,ADR-004）
                        let payload = rmp_serde::to_vec_named(&rec.event).map_err(|e| {
                            StoreError::Serialization {
                                reason: format!("rebuild 事件 rmp 序列化失败: {e}"),
                            }
                        })?;
                        let inserted = stmt_ev
                            .execute(params![
                                rec.seq,
                                session_id.as_str(),
                                seg_id.as_str(),
                                payload,
                            ])
                            .map_err(|e| StoreError::Sqlite {
                                reason: format!("rebuild 插入事件 offset={} 失败: {e}", rec.seq),
                            })?;
                        // 去重统计:INSERT OR IGNORE 返回 0 = 已存在跳过
                        if inserted == 0 {
                            stats.rows_skipped += 1;
                        } else {
                            stats.rows_inserted += 1;
                        }
                    }
                }
            }
            Ok(stats)
        })();
        match result {
            Ok(stats) => conn
                .execute_batch("COMMIT")
                .map_err(|e| StoreError::Sqlite {
                    reason: format!("rebuild 提交事务失败: {e}"),
                })
                .map(|_| stats),
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// 事件行总数（`session_id` 为 None 时全库统计;测试断言零拷贝用）
    pub fn event_count(&self, session_id: Option<&SessionId>) -> Result<u64, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("计数锁中毒: {p}"),
        })?;
        let (sql, sid): (&str, Option<&str>) = match session_id {
            Some(s) => (
                "SELECT COUNT(*) FROM events WHERE session_id = ?1",
                Some(s.as_str()),
            ),
            None => ("SELECT COUNT(*) FROM events", None),
        };
        let count = match sid {
            Some(s) => conn
                .query_row(sql, params![s], |r| r.get::<_, i64>(0))
                .map_err(|e| StoreError::Sqlite {
                    reason: format!("统计事件数失败: {e}"),
                })?,
            None => conn
                .query_row(sql, [], |r| r.get::<_, i64>(0))
                .map_err(|e| StoreError::Sqlite {
                    reason: format!("统计事件数失败: {e}"),
                })?,
        };
        Ok(count as u64)
    }

    /// 段元数据行数（测试断言 fork 复制行数量）
    pub fn segment_count(&self, session_id: &SessionId) -> Result<u64, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("段计数锁中毒: {p}"),
        })?;
        let count = conn
            .query_row(
                "SELECT COUNT(*) FROM segments WHERE session_id = ?1",
                params![session_id.as_str()],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| StoreError::Sqlite {
                reason: format!("统计段数失败: {e}"),
            })?;
        Ok(count as u64)
    }

    /// SQLite 写事务计数（bench 门禁:syscall_reduction_pct 的分母/分子）
    #[must_use]
    pub fn transaction_count(&self) -> u64 {
        self.transaction_count.load(Ordering::Relaxed)
    }

    /// 按 (session_id, segment_index) 查既有段 ID（续写幂等复用残留行用）
    ///
    /// # WHY（P2-T3 续写幂等承接）
    /// writer 重开后续写同一物理段时,若重新 `generate()` 段 ID 会产生
    /// **重复段行**（旧行 end_offset 定格、新行重复覆盖）→ segment_sources
    /// 按段索引返回多行 → 回放重复打开同一文件、事件重复输出。复用既有
    /// ID 使 insert_segment 走 upsert（end_offset 推进）路径,索引行唯一。
    /// None = 会话尚无该段的行（新段 / 索引缺行待 rebuild）。
    pub fn segment_id_for(
        &self,
        session_id: &SessionId,
        segment_index: u32,
    ) -> Result<Option<SegmentId>, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("段 ID 查询锁中毒: {p}"),
        })?;
        query_segment_id(&conn, session_id, segment_index)
    }

    /// fork 会话的续写起点 — 前缀段最大 end_offset + 1（重开续写恢复用）
    ///
    /// # WHY（P2-T3 续写幂等承接）
    /// fork 会话零拷贝无自己的段文件,重开续写无法从文件推断起点;fork 点
    /// 信息仅存于前缀复制行的 end_offset（fork 时截断到 fork 点-1）→ 恢复
    /// 点 = max(end_offset) + 1。None = 会话无前缀复制行（非 fork 会话/新
    /// 会话,续写点应为 0）。
    pub fn fork_point(&self, session_id: &SessionId) -> Result<Option<u64>, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("fork 点查询锁中毒: {p}"),
        })?;
        let prefix_end: Option<i64> = conn
            .query_row(
                "SELECT MAX(end_offset) FROM segments
                 WHERE session_id = ?1 AND parent_segment_id IS NOT NULL",
                params![session_id.as_str()],
                |r| r.get(0),
            )
            .map_err(|e| StoreError::Sqlite {
                reason: format!("查询会话 {} 前缀段 end 上限失败: {e}", session_id),
            })?;
        Ok(prefix_end.map(|e| e as u64 + 1))
    }
}

// ============================================================
// 模块级私有辅助（行映射 / 物理解析 / 段 ID 查询）
// ============================================================

/// segments 行 → SegmentNode 映射（query_row 回调）
fn row_to_node(r: &rusqlite::Row<'_>) -> rusqlite::Result<SegmentNode> {
    Ok(SegmentNode {
        segment_id: SegmentId::new(r.get::<_, String>(0)?),
        session_id: SessionId::new(r.get::<_, String>(1)?),
        segment_index: r.get::<_, i64>(2)? as u32,
        parent_segment_id: r
            .get::<_, Option<String>>(3)?
            .map(SegmentId::new),
        start_offset: r.get::<_, i64>(4)? as u64,
        end_offset: r.get::<_, i64>(5)? as u64,
    })
}

/// 查询会话全部段节点（按段索引升序）
fn query_segment_nodes(
    conn: &rusqlite::Connection,
    session_id: &SessionId,
) -> Result<Vec<SegmentNode>, StoreError> {
    let mut stmt = conn
        .prepare(
            "SELECT segment_id, session_id, segment_index, parent_segment_id, start_offset, end_offset
             FROM segments WHERE session_id = ?1 ORDER BY segment_index ASC",
        )
        .map_err(|e| StoreError::Sqlite {
            reason: format!("预编译会话段查询失败: {e}"),
        })?;
    let rows = stmt
        .query_map(params![session_id.as_str()], row_to_node)
        .map_err(|e| StoreError::Sqlite {
            reason: format!("查询会话 {} 段失败: {e}", session_id),
        })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| StoreError::Sqlite {
        reason: format!("读取会话段行失败: {e}"),
    })
}

/// 按段 ID 查询单段节点（None → NotFound）
fn query_segment_node(
    conn: &rusqlite::Connection,
    segment_id: &SegmentId,
) -> Result<SegmentNode, StoreError> {
    let node: Option<SegmentNode> = conn
        .query_row(
            "SELECT segment_id, session_id, segment_index, parent_segment_id, start_offset, end_offset
             FROM segments WHERE segment_id = ?1",
            params![segment_id.as_str()],
            row_to_node,
        )
        .optional()
        .map_err(|e| StoreError::Sqlite {
            reason: format!("查询段 {} 失败: {e}", segment_id),
        })?;
    node.ok_or_else(|| StoreError::NotFound {
        what: format!("段 {} 不存在", segment_id),
    })
}

/// 按 (session_id, segment_index) 查既有段 ID（rebuild 复用残留行用）
fn query_segment_id(
    conn: &rusqlite::Connection,
    session_id: &SessionId,
    segment_index: u32,
) -> Result<Option<SegmentId>, StoreError> {
    // WHY 排除 parent_segment_id IS NOT NULL 的复制行:fork 复制行与子会话
    // 自己的段行共享 segment_index（复制行保留父段 index,子段也从 0 起）——
    // 若不过滤,查询可能命中复制行 ID,后续 insert_segment 的 upsert 会把
    // 复制行当真实段行推进 end_offset,污染前缀上界（fork_point 漂移,
    // 回放/read_events 前缀越界多输出）。物理段行的 parent 恒为 NULL。
    let id: Option<String> = conn
        .query_row(
            "SELECT segment_id FROM segments
             WHERE session_id = ?1 AND segment_index = ?2 AND parent_segment_id IS NULL",
            params![session_id.as_str(), segment_index],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| StoreError::Sqlite {
            reason: format!("查询会话 {} 段 {} 的 ID 失败: {e}", session_id, segment_index),
        })?;
    Ok(id.map(SegmentId::new))
}

/// 沿 parent 引用链解析段到物理文件所属会话 + 段索引
///
/// # WHY 递归
/// 链式 fork 下复制行的 session_id 是中间会话（无物理文件）,须逐层向上
/// 回溯到 parent_segment_id IS NULL 的物理段（其 session_id + segment_index
/// 决定段文件路径）。`depth` 为防环上限（fork 链理论无环,异常数据兜底）。
fn resolve_physical(
    conn: &rusqlite::Connection,
    segment_id: &SegmentId,
    depth: u32,
) -> Result<(SessionId, u32), StoreError> {
    if depth > 64 {
        return Err(StoreError::InvalidInput {
            reason: format!("fork 引用链过深或成环(段 {}): 超过 64 层上限", segment_id),
        });
    }
    let node = query_segment_node(conn, segment_id)?;
    match &node.parent_segment_id {
        None => Ok((node.session_id, node.segment_index)),
        Some(parent) => resolve_physical(conn, parent, depth + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::{segment_path, SegmentWriter};
    use crate::types::SessionEvent;

    fn ev(i: u64) -> SessionEvent {
        SessionEvent::with_payload(format!("ev-{i}"), vec![i as u8])
    }

    /// 用 SegmentWriter 写入父会话段 + 索引,返回 (tree, dir, sid)
    fn seed_session(
        tree: &TreeIndex,
        dir: &Path,
        sid: &SessionId,
        n: u64,
        segment_size: u64,
    ) -> Result<(), StoreError> {
        let mut idx = 0u32;
        let mut start_seq = 0u64;
        while start_seq < n {
            let mut w = SegmentWriter::open_or_create(dir, sid, idx, start_seq)?;
            let take = segment_size.min(n - start_seq);
            let events: Vec<SessionEvent> = (start_seq..start_seq + take).map(ev).collect();
            let offsets = w.append_batch(&events)?;
            let seg_id = SegmentId::generate();
            tree.insert_segment(&w.meta(seg_id.clone(), None))?;
            let rows: Vec<EventRow> = offsets
                .iter()
                .zip(events)
                .map(|(off, event)| EventRow {
                    offset: off.seq,
                    session_id: sid.clone(),
                    segment_id: seg_id.clone(),
                    event,
                })
                .collect();
            tree.insert_events(&rows)?;
            start_seq += take;
            idx += 1;
        }
        Ok(())
    }

    #[test]
    fn insert_and_read_events_roundtrip() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("roundtrip");
        seed_session(&tree, dir.path(), &sid, 8, 8).expect("seed");
        assert_eq!(tree.event_count(Some(&sid)).expect("count"), 8);
        assert_eq!(tree.segment_count(&sid).expect("segments"), 1);

        let stored = tree.read_events(&sid, None).expect("read");
        assert_eq!(stored.len(), 8);
        for (i, s) in stored.iter().enumerate() {
            assert_eq!(s.offset, i as u64);
            assert_eq!(s.event.event_type, format!("ev-{i}"));
        }
        // from 过滤(断点续传)
        let tail = tree.read_events(&sid, Some(5)).expect("read from 5");
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].offset, 5);
    }

    #[test]
    fn multiple_segments_read_in_order() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("multi-seg");
        // 3 个段,每段 4 条
        seed_session(&tree, dir.path(), &sid, 12, 4).expect("seed");
        assert_eq!(tree.segment_count(&sid).expect("segments"), 3);
        let stored = tree.read_events(&sid, None).expect("read");
        assert_eq!(stored.len(), 12);
        for (i, s) in stored.iter().enumerate() {
            assert_eq!(s.offset, i as u64);
        }
    }

    #[test]
    fn fork_copies_metadata_zero_data_copy() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = SessionId::new("fork-parent");
        // 父会话 12 条,3 段 × 4
        seed_session(&tree, dir.path(), &parent, 12, 4).expect("seed");
        let files_before = std::fs::read_dir(dir.path()).expect("list dir").count();

        // fork 点 7:前缀 = [0,7) = 段0(0-3) + 段1(4-7 截断为 4-6)
        let child = tree.fork(&parent, 7).expect("fork");
        assert_ne!(child, parent);

        // 元数据复制:新会话有 2 个前缀复制行(parent_segment_id 非 NULL)
        assert_eq!(tree.segment_count(&child).expect("child segs"), 2);
        // 零数据拷贝:全库 events 行数不变
        assert_eq!(tree.event_count(None).expect("all events"), 12);
        // 零数据拷贝:无新 JSONL 段文件
        let files_after = std::fs::read_dir(dir.path()).expect("list dir").count();
        assert_eq!(files_before, files_after, "fork 不得创建段文件");
    }

    #[test]
    fn fork_child_reads_parent_prefix() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = SessionId::new("fork-read");
        seed_session(&tree, dir.path(), &parent, 10, 5).expect("seed");

        let child = tree.fork(&parent, 7).expect("fork");
        // 回查:新会话可见前缀 [0,7)
        let stored = tree.read_events(&child, None).expect("read");
        assert_eq!(stored.len(), 7, "fork 后回查父段事件可用");
        for (i, s) in stored.iter().enumerate() {
            assert_eq!(s.offset, i as u64);
            assert_eq!(s.event.event_type, format!("ev-{i}"));
        }
        // 前缀 + 新事件按 seq 无缝拼接:父会话继续写 7-9 不受影响
        let parent_tail = tree.read_events(&parent, Some(7)).expect("parent tail");
        assert_eq!(parent_tail.len(), 3);
    }

    #[test]
    fn fork_exact_segment_boundary() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = SessionId::new("fork-boundary");
        seed_session(&tree, dir.path(), &parent, 8, 4).expect("seed");
        // fork 点 4 = 段1 起点:前缀 = 段0 完整复制(无截断)
        let child = tree.fork(&parent, 4).expect("fork");
        assert_eq!(tree.segment_count(&child).expect("child segs"), 1);
        let stored = tree.read_events(&child, None).expect("read");
        assert_eq!(stored.len(), 4);
        assert_eq!(stored[0].offset, 0);
        assert_eq!(stored[3].offset, 3);
    }

    #[test]
    fn fork_beyond_history_is_rejected() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = SessionId::new("fork-oob");
        seed_session(&tree, dir.path(), &parent, 8, 4).expect("seed");
        let err = tree.fork(&parent, 100).expect_err("超出历史必须报错");
        assert!(matches!(err, StoreError::ForkViolation { .. }));
    }

    #[test]
    fn fork_at_zero_has_no_prefix() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = SessionId::new("fork-zero");
        seed_session(&tree, dir.path(), &parent, 8, 4).expect("seed");
        let child = tree.fork(&parent, 0).expect("fork at 0");
        assert_eq!(tree.segment_count(&child).expect("child segs"), 0);
        assert!(tree.read_events(&child, None).expect("read").is_empty());
    }

    #[test]
    fn transaction_count_tracks_writes() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("txn-count");
        let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("open");
        let events: Vec<SessionEvent> = (0..8).map(ev).collect();
        let offsets = w.append_batch(&events).expect("append");
        let seg_id = SegmentId::generate();
        tree.insert_segment(&w.meta(seg_id.clone(), None)).expect("seg");
        let rows: Vec<EventRow> = offsets
            .iter()
            .zip(events)
            .map(|(off, event)| EventRow {
                offset: off.seq,
                session_id: sid.clone(),
                segment_id: seg_id.clone(),
                event,
            })
            .collect();
        tree.insert_events(&rows).expect("insert");
        assert_eq!(tree.transaction_count(), 1, "一次批量 = 一次事务");
        tree.insert_events(&[]).expect("empty no-op");
        assert_eq!(tree.transaction_count(), 1, "空批不计数");
    }

    #[test]
    fn upsert_segment_keeps_start_updates_end() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let sid = SessionId::new("upsert");
        let m1 = SegmentMeta {
            segment_id: SegmentId::new("seg-1"),
            session_id: sid.clone(),
            segment_index: 0,
            parent_segment_id: None,
            start_offset: 0,
            end_offset: 4,
        };
        tree.insert_segment(&m1).expect("insert");
        let m2 = SegmentMeta {
            segment_id: SegmentId::new("seg-1"),
            session_id: sid.clone(),
            segment_index: 0,
            parent_segment_id: None,
            start_offset: 0,
            end_offset: 9,
        };
        tree.insert_segment(&m2).expect("upsert");
        let conn = tree.conn.lock().expect("lock");
        let (start, end): (i64, i64) = conn
            .query_row(
                "SELECT start_offset, end_offset FROM segments WHERE segment_id = 'seg-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("query");
        assert_eq!(start, 0, "start 保留首值");
        assert_eq!(end, 9, "end 更新");
    }

    #[test]
    fn segment_path_matches_naming_convention() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("path-naming");
        let p = segment_path(dir.path(), &sid, 3);
        assert_eq!(p.file_name().expect("name"), "path-naming.3.jsonl");
    }

    // ============================================================
    // 会话树（WI-18）:tree / fork_tree / ancestors
    // ============================================================

    /// 链式 fork 辅助:seed 父会话 → fork → 再 fork（返回 [parent, child, grandchild]）
    fn seed_fork_chain(
        tree: &TreeIndex,
        dir: &Path,
    ) -> Result<(SessionId, SessionId, SessionId), StoreError> {
        let a = SessionId::new("tree-fork-a");
        seed_session(tree, dir, &a, 10, 5).expect("seed a");
        let b = tree.fork(&a, 7).expect("fork a->b");
        let c = tree.fork(&b, 7).expect("fork b->c");
        Ok((a, b, c))
    }

    #[test]
    fn tree_lists_session_segments() {
        // tree():3 段全列出,根 = 全部（无 fork 复制行）
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("tree-view");
        seed_session(&tree, dir.path(), &sid, 12, 4).expect("seed");
        let t = tree.tree(&sid).expect("tree");
        assert_eq!(t.node_count(), 3);
        assert_eq!(t.roots().len(), 3, "非 fork 会话全部为根");
        assert!(t.roots().iter().all(|n| n.parent_segment_id.is_none()));
        // 段索引升序
        for (i, n) in t.nodes.iter().enumerate() {
            assert_eq!(n.segment_index, i as u32);
        }
    }

    #[test]
    fn fork_tree_shows_complete_view() {
        // fork 子会话:复制行 + 祖先节点（完整树视图）
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, b, _c) = seed_fork_chain(&tree, dir.path()).expect("chain");
        // B 的 tree():只含 B 的复制行（parent 指针指向 A 的段）
        let t_b = tree.tree(&b).expect("tree b");
        assert_eq!(t_b.node_count(), 2, "B 有 2 个复制段(10 条/5 每段,fork 点 7 截断段 1)");
        assert_eq!(t_b.roots().len(), 0, "B 全部为 fork 复制行,无根");
        assert!(t_b.nodes.iter().all(|n| n.parent_segment_id.is_some()));
        // fork_tree(B):B 节点 + A 祖先节点
        let ft_b = tree.fork_tree(&b).expect("fork_tree b");
        assert_eq!(ft_b.node_count(), 4, "B 的 2 节点 + A 的 2 段(段1被截断仍属 A 物理段)");
        // 祖先段（session_id == a）存在
        let a_nodes = ft_b
            .nodes
            .iter()
            .filter(|n| n.session_id == a)
            .collect::<Vec<_>>();
        assert_eq!(a_nodes.len(), 2, "完整视图含 A 的 2 个物理段节点");
    }

    #[test]
    fn ancestors_chain_across_forks() {
        // 链式 fork A→B→C:ancestors(C) = [B 的段, A 的段]（直接父层在前）
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let (_a, b, c) = seed_fork_chain(&tree, dir.path()).expect("chain");
        let t_b = tree.tree(&b).expect("tree b");
        let b_segs: std::collections::HashSet<SegmentId> = t_b
            .nodes
            .iter()
            .map(|n| n.segment_id.clone())
            .collect();
        let anc = tree.ancestors(&c).expect("ancestors");
        // 祖先链 = B 的 2 段 + A 的 2 段
        assert_eq!(anc.len(), 4, "B 层 2 段 + A 层 2 段");
        // 直接父层在前:B 的段出现在前两位
        assert!(b_segs.contains(&anc[0]) && b_segs.contains(&anc[1]), "直接父层在前");
        // 全链无重复
        let dedup: std::collections::HashSet<SegmentId> = anc.iter().cloned().collect();
        assert_eq!(dedup.len(), anc.len(), "祖先链无重复");
    }

    #[test]
    fn segment_sources_resolves_physical_file_owner() {
        // 段来源解析:fork 复制段 → 父物理文件（session_id = A,文件可打开）
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, b, _c) = seed_fork_chain(&tree, dir.path()).expect("chain");
        let sources = tree.segment_sources(&b).expect("sources");
        assert_eq!(sources.len(), 2, "B 的 2 个复制段");
        for src in &sources {
            assert_eq!(src.file_session, a, "物理文件归属父会话 A");
            // 截断段:B 视角 end_offset = fork 点-1 = 6
            assert!(src.end_offset <= 6, "fork 截断段 end 不超过 fork 点-1");
            // 物理文件必须存在
            let path = segment_path(dir.path(), &src.file_session, src.segment_index);
            assert!(path.exists(), "物理段文件存在: {}", path.display());
        }
        // 段 0 完整复制(0-4),段 1 截断(5-6)
        assert_eq!(sources[0].end_offset, 4);
        assert_eq!(sources[1].end_offset, 6);
    }

    // ============================================================
    // 索引重建（崩溃自愈 + 幂等）:rebuild_index
    // ============================================================

    #[test]
    fn rebuild_index_restores_after_events_deleted() {
        // 模拟崩溃:段文件完整、索引缺 events 行 → rebuild → read_events 完整
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("rebuild-crash");
        seed_session(&tree, dir.path(), &sid, 12, 4).expect("seed");
        // 崩溃模拟:删除 events 全部行（段文件保留）
        {
            let conn = tree.conn.lock().expect("lock");
            conn.execute("DELETE FROM events", []).expect("delete events");
        }
        assert_eq!(tree.event_count(Some(&sid)).expect("count"), 0);

        let stats = tree
            .rebuild_index(dir.path(), &sid)
            .expect("rebuild");
        assert_eq!(stats.segments_scanned, 3, "3 个物理段文件");
        assert_eq!(stats.rows_inserted, 12, "12 行重建");
        assert_eq!(stats.rows_skipped, 0);
        // rebuild 后 read_events 完整且顺序正确
        let stored = tree.read_events(&sid, None).expect("read");
        assert_eq!(stored.len(), 12);
        for (i, s) in stored.iter().enumerate() {
            assert_eq!(s.offset, i as u64);
            assert_eq!(s.event.event_type, format!("ev-{i}"));
        }
    }

    #[test]
    fn rebuild_index_is_idempotent() {
        // 幂等:第二次 rebuild 全跳过,无副作用
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("rebuild-idem");
        seed_session(&tree, dir.path(), &sid, 12, 4).expect("seed");
        {
            let conn = tree.conn.lock().expect("lock");
            conn.execute("DELETE FROM events", []).expect("delete");
        }
        let first = tree.rebuild_index(dir.path(), &sid).expect("first");
        assert_eq!(first.rows_inserted, 12);
        let second = tree.rebuild_index(dir.path(), &sid).expect("second");
        assert_eq!(second.rows_inserted, 0, "第二次全去重跳过");
        assert_eq!(second.rows_skipped, 12, "幂等:已存在 offset 全跳过");
        assert_eq!(tree.event_count(Some(&sid)).expect("count"), 12, "行数不变");
    }

    #[test]
    fn rebuild_index_partial_missing_rows_merge() {
        // 部分缺行（索引残留一半）:rebuild 补齐缺失,已存在跳过
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("rebuild-partial");
        seed_session(&tree, dir.path(), &sid, 8, 4).expect("seed");
        // 删掉 offset >= 4 的行（后半段索引缺失）
        {
            let conn = tree.conn.lock().expect("lock");
            conn.execute("DELETE FROM events WHERE offset >= 4", []).expect("delete");
        }
        let stats = tree.rebuild_index(dir.path(), &sid).expect("rebuild");
        assert_eq!(stats.rows_inserted, 4, "补齐缺失的 4 行");
        assert_eq!(stats.rows_skipped, 4, "已存在 4 行跳过");
        assert_eq!(tree.event_count(Some(&sid)).expect("count"), 8);
    }
}
