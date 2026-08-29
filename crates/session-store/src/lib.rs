//! session-store — 会话事件流存储（append-only 段 + CBMR 微批写）
//!
//! 对应架构层: **L3 Storage**（Phase 2 新增,ADR-141,workspace 第 40 个 crate）
//! 对应任务: **P2-T2**（手册 W9 T-07 / ADR-108 CBMR 微批写 / v4.0 WI-18 存储面）
//!
//! # 设计概览（Checkpoint 线性快照 → append-only 段 + 微批写）
//!
//! 会话持久化从「Checkpoint 线性快照」（L0 `nexus_contracts::Checkpoint`,
//! 低频全量落盘）升级为「事件流补充」:
//!
//! ```text
//!   写路径:  append(ev) ──► pending 攒批(≤64/2ms 自适应窗口)
//!                                │ flush(经 spawn_blocking)
//!                                ▼
//!                        JSONL 段文件(每 Thread 一段, 长度前缀 WAL 意向)
//!                                │
//!                        SQLite 树索引(segments/events, 单事务批量)
//!
//!   读路径:  read_events ──► SQLite 树索引(spawn_blocking, 与写并发 WAL)
//!
//!   fork:    fork(session, offset) ──► 前缀段元数据复制(零事件数据拷贝)
//! ```
//!
//! - **段文件**:`<session_id>.<segment_index>.jsonl`,长度前缀 + JSON 数据,
//!   崩溃时截断半条尾部,`append` 返回 Ok 前 fsync（不丢已确认事件）
//! - **Offset 双键**:`{ seq 全局单调, row 段内行号 }`——P2-T3 k-way 归并
//!   回放的排序键（ADR-109;归并本身是 T3,本任务只实现追加与 Offset 语义）
//! - **CBMR 微批写**:SQLite 写操作从 N 次（单条直写）降为 ceil(N/64) 次,
//!   基准输出 `syscall_reduction_pct`（T8/T9 模式固定 n 单次采样）
//! - **model-visible 不变量打点**:属于 T3,本任务只做存储面
//!
//! # 已知约束（T3 承接）
//!
//! 段 fsync 与索引 insert 非原子;崩溃后索引重建/续写幂等由 P2-T3
//! k-way 归并回放承接,T2 边界内「已确认 = 段 fsync」语义成立
//! （详见 [`writer::CbmrWriter::flush_sync`] 的 WHY 注释）。
//!
//! # 红线遵守
//!
//! - `#![forbid(unsafe_code)]`:rusqlite 内部 FFI unsafe 不传播
//! - SQLite 写入只经 `tokio::task::spawn_blocking`（无 runtime 降级同步）
//! - 禁持锁跨 await（锁段内快照 → 释放 → await）
//! - `PRAGMA journal_mode=WAL` 经 `pragma_update`（项目红线）
//! - 库代码禁 unwrap/expect（thiserror 传播）

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

/// 错误类型 — StoreError（Io / WalCorrupt / OffsetMismatch / ForkViolation 等）
pub mod error;

/// 核心类型 — SessionId / SegmentId / Offset 双键 / SessionEvent / StoreConfig
pub mod types;

/// append-only 段 — SegmentWriter（WAL 意向 + 崩溃恢复截断）
pub mod segment;

/// SQLite 树索引 — TreeIndex（segments/events + fork 零拷贝）
pub mod tree;

/// CBMR 微批写 — CbmrWriter（≤64/2ms 自适应窗口 + spawn_blocking）
pub mod writer;

/// k-way 归并回放 — 多段时间线全局单调序列（ADR-109,纯段文件读取）
pub mod replay;

/// model-visible 白名单投影 — 「model-visible means logged」不变量（WI-18）
pub mod model_view;

pub use error::StoreError;
pub use model_view::{to_model_view, ModelVisibleEvent};
pub use replay::{replay, ReplayItem, ReplayStream};
pub use segment::{list_segment_files, segment_path, SegmentFileReader, SegmentMeta, SegmentRecord, SegmentWriter};
pub use tree::{EventRow, RebuildStats, SegmentNode, SegmentSource, SessionTree, StoredEvent, TreeIndex};
pub use types::{Offset, SegmentId, SessionEvent, SessionId, StoreConfig};
pub use writer::CbmrWriter;

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// proptest 的事件生成策略:event_type 序号化 + 可选随机负载
    fn event_strategy() -> impl Strategy<Value = SessionEvent> {
        // WHY any::<Option<Vec<u8>>>:proptest 为 Option<T> 提供 Arbitrary(T: Arbitrary),
        // 比 proptest::option::any 组合器更简洁
        (0u64..1024, proptest::option::of(proptest::collection::vec(any::<u8>(), 0..64)))
            .prop_map(|(i, payload)| SessionEvent {
                metadata: nexus_contracts::EventMetadata::new("session-store"),
                event_type: format!("ev-{i}"),
                payload,
            })
    }

    /// 从段文件读回全部事件（测试专用 reader:剥长度前缀 + 解析 JSON）
    fn read_segment_file(path: &std::path::Path) -> Result<Vec<SessionEvent>, StoreError> {
        let bytes = std::fs::read(path).map_err(|e| StoreError::Io {
            context: format!("读取段文件 {} 失败", path.display()),
            source: e,
        })?;
        let mut pos = 0usize;
        let mut out = Vec::new();
        while pos < bytes.len() {
            if bytes.len() - pos < 4 {
                return Err(StoreError::WalCorrupt {
                    path: path.display().to_string(),
                    reason: "尾部残留不足 4 字节长度前缀".into(),
                });
            }
            let len = u32::from_le_bytes(bytes[pos..pos + 4].try_into().expect("len")) as usize;
            pos += 4;
            if bytes.len() - pos < len {
                return Err(StoreError::WalCorrupt {
                    path: path.display().to_string(),
                    reason: "数据不足长度前缀指示".into(),
                });
            }
            // JSON 行 = SegmentRecord(内嵌 seq/row),仅取事件本体
            let record: SegmentRecord =
                serde_json::from_slice(&bytes[pos..pos + len]).map_err(|e| {
                    StoreError::Serialization {
                        reason: format!("段文件 JSON 解析失败: {e}"),
                    }
                })?;
            out.push(record.event);
            pos += len;
            if pos >= bytes.len() {
                break;
            }
            if bytes[pos] != b'\n' {
                return Err(StoreError::WalCorrupt {
                    path: path.display().to_string(),
                    reason: "记录未以换行结尾".into(),
                });
            }
            pos += 1;
        }
        Ok(out)
    }

    proptest! {
        /// 任意事件序列 append 后顺序完整（属性测试）
        ///
        /// 不变量:① Offset.seq 严格单调(+1);② Offset.row 严格单调(+1);
        /// ③ 段文件读回的事件顺序 == append 顺序;④ 事件内容保真(event_type/payload)。
        #[test]
        fn append_sequence_preserves_order_and_content(events in proptest::collection::vec(event_strategy(), 0..64)) {
            let dir = tempfile::tempdir().expect("tempdir");
            let sid = SessionId::new("prop-seq");
            let mut writer = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0)
                .expect("open_or_create");

            let mut last_seq: Option<u64> = None;
            let mut last_row: Option<u64> = None;
            for ev in &events {
                let off = writer.append(ev).expect("append");
                if let Some(ls) = last_seq {
                    prop_assert_eq!(off.seq, ls + 1, "全局 seq 必须逐 +1 单调");
                } else {
                    prop_assert_eq!(off.seq, 0, "首事件 seq 从 0 起");
                }
                if let Some(lr) = last_row {
                    prop_assert_eq!(off.row, lr + 1, "段内 row 必须逐 +1 单调");
                }
                last_seq = Some(off.seq);
                last_row = Some(off.row);
            }
            prop_assert_eq!(writer.row_count(), events.len() as u64);

            // 段文件读回顺序与内容保真
            let read_back = read_segment_file(writer.path()).expect("read segment");
            prop_assert_eq!(read_back.len(), events.len());
            for (a, b) in read_back.iter().zip(&events) {
                prop_assert_eq!(a, b, "事件内容必须逐条保真");
            }
        }
    }
}
