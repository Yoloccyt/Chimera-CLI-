//! k-way 归并回放 — 多段时间线归并为全局单调序列（ADR-109 核心）
//!
//! 对应架构层: **L3 Storage**（session-store,Phase 2 新增,ADR-141）
//! 对应任务: **P2-T3**（ADR-109 k-way 归并时间线回放 / 手册 §11.3 SessionStore 契约）
//!
//! # 核心语义（ADR-109）
//!
//! ```text
//!  各段文件有序（append-only,段内 seq/row 严格单调）
//!        │
//!        ▼  k-way 归并（k = 段数,BinaryHeap 最小堆）
//!  全局单调序列（Offset 双键:seq 主序 + row 次序）
//! ```
//!
//! - **回放顺序与 Critical 流一致率 100% 门禁**:Offset.seq 是 Critical 流
//!   顺序的持久化镜像（append 路径分配全局单调 seq）。测试断言:任意事件
//!   序列写入后,replay 输出顺序与写入顺序逐项一致（含跨段滚动）。
//! - **from 偏移续读**:`replay(session_id, from)` 返回 seq >= from 的流,
//!   断点续传语义（与 `TreeIndex::read_events(from)` 对齐）。
//! - **fork 会话**:段文件经 `segments` 表 parent 引用链解析到物理文件,
//!   前缀段按 fork 截断点（end_offset）过滤——子会话只看到 fork 点之前
//!   的前缀 + 自己的事件（与 read_events 语义一致）。
//!
//! # 时间戳校验语义（手册 ADR-109「时间戳+序列号双键」）
//!
//! 排序键 = Offset（seq 主序,row 次序）——时间戳**不参与排序**（时钟
//! 回拨/同批同毫秒是正常现象）。replay 在输出路径上校验时间戳单调非减:
//! 乱序时间戳 → `tracing::warn` 记录警告**不中断**输出——语义:时间戳是
//! 审计元数据而非排序权威,乱序只提示时钟异常,不影响回放正确性（排序
//! 权威始终是 seq）。`out_of_order_count()` 暴露乱序计数供诊断。
//!
//! # 红线
//!
//! 只读路径不经 SQLite（纯段文件扫描）——回放与索引解耦:索引缺失
//! （rebuild_index 自愈中）不影响 replay 正确性,段文件是权威源。

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::Path;

use tracing::warn;

use crate::error::StoreError;
use crate::segment::{segment_path, SegmentFileReader, SegmentRecord};
use crate::tree::TreeIndex;
use crate::types::{Offset, SessionEvent, SessionId};

/// 回放条目 — Offset 双键 + 事件本体
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayItem {
    /// 全局单调排序键（seq 主序 + row 次序）
    pub offset: Offset,
    /// 事件本体
    pub event: SessionEvent,
}

/// k-way 归并堆条目 — 排序键 = Offset（seq 主序 + row 次序）
///
/// WHY 包一层:BinaryHeap 是最大堆,`Reverse<HeapEntry>` 取最小——全局单调
/// 序列要求每次弹出现有 k 条游标中 Offset 最小的一条（最小堆语义）。
#[derive(Debug)]
struct HeapEntry {
    offset: Offset,
    /// 对应段游标在 `readers` 中的索引
    cursor: usize,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.offset == other.offset
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // 排序键 = Offset 全序（seq 主序,row 次序）;cursor 仅作稳定化兜底
        self.offset
            .cmp(&other.offset)
            .then_with(|| self.cursor.cmp(&other.cursor))
    }
}

/// 单段游标 — 只读流 + 当前预读记录 + 会话视角段尾
#[derive(Debug)]
struct SegCursor {
    reader: SegmentFileReader,
    /// 会话视角段尾上限（fork 截断段 = fork 点-1;物理段 = 段内实际末尾）:
    /// seq > end_offset 的记录不属于该会话可见范围,游标视为 EOF
    end_offset: u64,
    /// 当前预读记录（未消费;None = EOF）
    current: Option<SegmentRecord>,
}

impl SegCursor {
    /// 预读下一条属于 [from, end_offset] 的记录,更新 current
    ///
    /// # WHY 初始化与续读共用
    /// from 过滤只发生在段头（段内 seq 单调,一旦 >= from 后续都满足）;
    /// end_offset 过滤是持续约束（fork 截断段的后半段记录必须跳过）。
    fn advance(&mut self, from: Offset) -> Result<(), StoreError> {
        loop {
            match self.reader.next_record()? {
                Some(rec) => {
                    if rec.seq > self.end_offset {
                        // 超出会话可见范围:该段结束（fork 截断或物理末尾之后
                        // 不应存在的记录——防御性停止）
                        self.current = None;
                        return Ok(());
                    }
                    let off = Offset::new(rec.seq, rec.row);
                    if off >= from {
                        self.current = Some(rec);
                        return Ok(());
                    }
                    // 小于 from:继续预读（段内 seq 单调,最终会越过或 EOF）
                }
                None => {
                    self.current = None;
                    return Ok(());
                }
            }
        }
    }
}

/// k-way 归并回放流 — 输出全局单调序列（seq 主序 + row 次序）
///
/// # 用法
/// ```ignore
/// let stream = replay(&tree, data_dir, &sid, Offset::new(0, 0))?;
/// while let Some(item) = stream.next_item()? { ... }
/// ```
///
/// # 非迭代器设计（WHY）
/// `next_item` 返回 `Result`（读取可能失败——IO 错误/损坏）,不实现
/// `std::iter::Iterator`（其 `next` 不能返回 Result）。批量场景用
/// [`ReplayStream::collect`]。
#[derive(Debug)]
pub struct ReplayStream {
    readers: Vec<SegCursor>,
    heap: BinaryHeap<Reverse<HeapEntry>>,
    /// 时间戳单调校验游标（Unix 毫秒;None = 首条）
    last_timestamp_ms: Option<i64>,
    /// 乱序时间戳计数（诊断;乱序不中断输出）
    out_of_order_count: u64,
}

impl ReplayStream {
    /// 读取下一条全局有序回放条目（EOF → Ok(None)）
    pub fn next_item(&mut self) -> Result<Option<ReplayItem>, StoreError> {
        let entry = match self.heap.pop() {
            Some(Reverse(e)) => e,
            None => return Ok(None),
        };
        let cursor = self
            .readers
            .get_mut(entry.cursor)
            .ok_or_else(|| StoreError::InvalidInput {
                reason: format!("堆条目引用不存在的段游标 {}", entry.cursor),
            })?;
        let rec = cursor.current.take().ok_or_else(|| {
            StoreError::InvalidInput {
                reason: "堆顶段游标无当前记录(状态不一致)".into(),
            }
        })?;
        // 续读:从该段拉取下一条记录重新入堆（k-way 归并推进被消费段）
        cursor.advance(Offset::new(rec.seq, rec.row))?;
        if let Some(next) = &cursor.current {
            self.heap.push(Reverse(HeapEntry {
                offset: Offset::new(next.seq, next.row),
                cursor: entry.cursor,
            }));
        }

        // 时间戳单调校验:乱序 → warn 不中断（排序权威始终是 seq）
        let ts = rec.event.metadata.timestamp.timestamp_millis();
        if let Some(last) = self.last_timestamp_ms {
            if ts < last {
                self.out_of_order_count += 1;
                warn!(
                    "回放时间戳乱序: seq={} timestamp_ms={ts} < 上一条 {last}(记录警告不中断;时间戳是审计元数据而非排序权威)",
                    rec.seq
                );
            }
        }
        self.last_timestamp_ms = Some(ts);

        Ok(Some(ReplayItem {
            offset: Offset::new(rec.seq, rec.row),
            event: rec.event,
        }))
    }

    /// 收集全部条目为 Vec（测试 / 小数据量便捷 API;等价于循环 next_item）
    pub fn collect(mut self) -> Result<Vec<ReplayItem>, StoreError> {
        let mut out = Vec::new();
        while let Some(item) = self.next_item()? {
            out.push(item);
        }
        Ok(out)
    }

    /// 乱序时间戳计数（诊断;语义见模块文档）
    #[must_use]
    pub fn out_of_order_count(&self) -> u64 {
        self.out_of_order_count
    }

    /// 归并段数 k（诊断 / 测试断言多段归并）
    #[must_use]
    pub fn merge_degree(&self) -> usize {
        self.readers.len()
    }
}

/// k-way 归并回放 — 会话事件流全局单调回放（ADR-109）
///
/// # 参数
/// - `tree`:树索引（仅用于解析段文件来源:segments 表 + parent 引用链）;
///   回放数据本身**纯读段文件**（索引缺失不影响正确性）
/// - `data_dir`:段文件所在目录（与 `StoreConfig::data_dir` 一致）
/// - `session_id`:目标会话（含 fork 会话,自动合并前缀引用段）
/// - `from`:回放下界 Offset（seq 主过滤键;断点续传）
///
/// # 返回
/// 全局单调回放流（seq 主序 + row 次序;顺序与写入顺序一致率 100% 门禁）。
pub fn replay(
    tree: &TreeIndex,
    data_dir: &Path,
    session_id: &SessionId,
    from: Offset,
) -> Result<ReplayStream, StoreError> {
    // 1. 解析会话可见段来源（含 fork 引用链 → 物理文件）
    let sources = tree.segment_sources(session_id)?;

    // 2. 打开 k 个段游标,各预读首条 >= from 的记录
    let mut readers: Vec<SegCursor> = Vec::with_capacity(sources.len());
    let mut heap: BinaryHeap<Reverse<HeapEntry>> = BinaryHeap::new();
    for (i, src) in sources.iter().enumerate() {
        let path = segment_path(data_dir, &src.file_session, src.segment_index);
        let reader = SegmentFileReader::open(&path)?;
        let mut cursor = SegCursor {
            reader,
            end_offset: src.end_offset,
            current: None,
        };
        cursor.advance(from)?;
        if let Some(rec) = &cursor.current {
            heap.push(Reverse(HeapEntry {
                offset: Offset::new(rec.seq, rec.row),
                cursor: i,
            }));
        }
        readers.push(cursor);
    }

    Ok(ReplayStream {
        readers,
        heap,
        last_timestamp_ms: None,
        out_of_order_count: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::EventRow;
    use crate::types::SegmentId;
    use crate::{SegmentWriter, TreeIndex};
    use proptest::prelude::*;

    /// 构造测试事件（event_type 带序号便于断言顺序）
    fn ev(i: u64) -> SessionEvent {
        SessionEvent::with_payload(format!("ev-{i}"), vec![i as u8])
    }

    /// 同步写入 n 条事件到 segment_size 每段的段文件 + 树索引
    ///（测试辅助:对齐 tree.rs tests 的 seed_session,但段内事件数可控）
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

    /// 回放全部事件,断言与写入顺序逐项一致（门禁:一致率 100%）
    fn assert_replay_order(
        tree: &TreeIndex,
        dir: &Path,
        sid: &SessionId,
        expected_len: u64,
    ) -> Vec<ReplayItem> {
        let stream = replay(tree, dir, sid, Offset::new(0, 0)).expect("replay");
        let items = stream.collect().expect("collect");
        assert_eq!(items.len(), expected_len as usize, "回放条数必须完整");
        for (i, item) in items.iter().enumerate() {
            assert_eq!(item.offset.seq, i as u64, "回放 seq 必须逐 +1 与写入顺序一致");
            assert_eq!(item.event.event_type, format!("ev-{i}"), "事件顺序逐项一致");
        }
        items
    }

    #[test]
    fn replay_single_segment_in_order() {
        // 单段回放:顺序与写入一致
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("r-single");
        seed_session(&tree, dir.path(), &sid, 16, 64).expect("seed");
        assert_replay_order(&tree, dir.path(), &sid, 16);
    }

    #[test]
    fn replay_multi_segment_merges_in_order() {
        // 多段（3 段 × 4 条）:k-way 归并跨段顺序一致率 100%
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("r-multi");
        seed_session(&tree, dir.path(), &sid, 12, 4).expect("seed");
        let stream = replay(&tree, dir.path(), &sid, Offset::new(0, 0)).expect("replay");
        assert_eq!(stream.merge_degree(), 3, "k = 段数 = 3");
        assert_replay_order(&tree, dir.path(), &sid, 12);
    }

    #[test]
    fn replay_across_rollover_order_preserved() {
        // 跨段滚动:2 段边界（段 0 满 8,段 1 续 4）顺序一致
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("r-roll");
        seed_session(&tree, dir.path(), &sid, 12, 8).expect("seed");
        assert_replay_order(&tree, dir.path(), &sid, 12);
    }

    #[test]
    fn replay_from_offset_resumes() {
        // from 偏移续读:seq >= 5 起,首条必须是 offset(5, x)
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("r-from");
        seed_session(&tree, dir.path(), &sid, 12, 4).expect("seed");
        let stream = replay(&tree, dir.path(), &sid, Offset::new(5, 0)).expect("replay");
        let items = stream.collect().expect("collect");
        assert_eq!(items.len(), 7, "seq>=5 共 7 条");
        assert_eq!(items[0].offset.seq, 5);
        assert_eq!(items[0].event.event_type, "ev-5");
        assert_eq!(items[6].offset.seq, 11);
    }

    #[test]
    fn replay_empty_session_returns_empty() {
        // 空会话:无段文件 → 空流（不报错）
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("r-empty");
        let stream = replay(&tree, dir.path(), &sid, Offset::new(0, 0)).expect("replay");
        assert!(stream.collect().expect("collect").is_empty());
    }

    #[test]
    fn replay_fork_child_sees_prefix_only() {
        // fork 子会话:前缀段（截断在 fork 点）+ 自己事件,顺序无缝
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = SessionId::new("r-fork-parent");
        seed_session(&tree, dir.path(), &parent, 10, 5).expect("seed");
        let child = tree.fork(&parent, 7).expect("fork");
        // 子会话写自己的事件（seq 从 7 起）
        let mut w = SegmentWriter::open_or_create(dir.path(), &child, 0, 7).expect("child seg0");
        let offs = w.append_batch(&[ev(100), ev(101)]).expect("child append");
        let cseg = SegmentId::generate();
        tree.insert_segment(&w.meta(cseg.clone(), None)).expect("child seg meta");
        let rows: Vec<EventRow> = offs
            .iter()
            .zip([ev(100), ev(101)])
            .map(|(off, event)| EventRow {
                offset: off.seq,
                session_id: child.clone(),
                segment_id: cseg.clone(),
                event,
            })
            .collect();
        tree.insert_events(&rows).expect("child events");

        let stream = replay(&tree, dir.path(), &child, Offset::new(0, 0)).expect("child replay");
        let items = stream.collect().expect("collect");
        // 前缀 [0,7) + 自己 [7,9) = 9 条连续
        assert_eq!(items.len(), 9);
        for (i, item) in items.iter().enumerate() {
            assert_eq!(item.offset.seq, i as u64, "前缀+新事件 seq 无缝拼接");
        }
        assert_eq!(items[7].event.event_type, "ev-100", "fork 点后是子会话事件");
    }

    #[test]
    fn replay_tolerates_out_of_order_timestamps() {
        // 乱序时间戳:警告不中断,输出完整（时间戳非排序权威）
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("r-ts");
        // 手工构造:段 0 写 2 条正常事件,再篡改第 2 条时间戳使其早于第 1 条
        {
            let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("open");
            let e0 = ev(0);
            let mut e1 = ev(1);
            // 显式时间戳:e1 的毫秒数早于 e0（构造乱序）
            e1.metadata.timestamp = e0.metadata.timestamp - chrono::Duration::seconds(5);
            w.append(&e0).expect("append0");
            w.append(&e1).expect("append1");
            let seg_id = SegmentId::generate();
            tree.insert_segment(&w.meta(seg_id.clone(), None)).expect("meta");
            for (off, event) in [0u64, 1u64].iter().zip([e0, e1]) {
                tree.insert_events(&[EventRow {
                    offset: *off,
                    session_id: sid.clone(),
                    segment_id: seg_id.clone(),
                    event,
                }])
                .expect("events");
            }
        }
        let mut stream = replay(&tree, dir.path(), &sid, Offset::new(0, 0)).expect("replay");
        // WHY 手动 next_item 循环而非 collect:collect 消费 stream,后续
        // out_of_order_count 需借用——循环后 stream 仍可借用（诊断统计）
        let mut items = Vec::new();
        while let Some(item) = stream.next_item().expect("next") {
            items.push(item);
        }
        assert_eq!(items.len(), 2, "乱序时间戳不中断回放");
        assert_eq!(stream.out_of_order_count(), 1, "乱序计数 = 1");
    }

    /// proptest 事件策略（对齐 lib.rs tests）
    fn event_strategy() -> impl Strategy<Value = SessionEvent> {
        (0u64..1024, proptest::option::of(proptest::collection::vec(any::<u8>(), 0..64)))
            .prop_map(|(i, payload)| SessionEvent {
                metadata: nexus_contracts::EventMetadata::new("session-store"),
                event_type: format!("ev-{i}"),
                payload,
            })
    }

    proptest! {
        /// 属性:任意多段写入（随机段大小 1-8）→ replay 顺序 = 写入顺序（一致率 100%）
        ///
        /// 覆盖跨段滚动的所有边界:段大小随机,段数随事件数自然变化;
        /// 断言全局 seq 逐 +1 且事件内容逐项保真。
        #[test]
        fn prop_replay_order_matches_write_order(
            events in proptest::collection::vec(event_strategy(), 0..64),
            segment_size in 1usize..8usize,
        ) {
            let tree = TreeIndex::open_in_memory().expect("open");
            let dir = tempfile::tempdir().expect("tempdir");
            let sid = SessionId::new("prop-replay");
            // 同步写入（单会话单批,跨段滚动由 segment_size 控制）
            let mut idx = 0u32;
            let mut start_seq = 0u64;
            let mut written: Vec<SessionEvent> = Vec::new();
            let mut pos = 0usize;
            while pos < events.len() {
                let mut w = SegmentWriter::open_or_create(dir.path(), &sid, idx, start_seq)
                    .expect("open seg");
                let take = segment_size.min(events.len() - pos);
                let batch: Vec<SessionEvent> = events[pos..pos + take].to_vec();
                let offsets = w.append_batch(&batch).expect("append_batch");
                let seg_id = SegmentId::generate();
                tree.insert_segment(&w.meta(seg_id.clone(), None)).expect("meta");
                let rows: Vec<EventRow> = offsets.iter().zip(batch.iter().cloned())
                    .map(|(off, event)| EventRow {
                        offset: off.seq,
                        session_id: sid.clone(),
                        segment_id: seg_id.clone(),
                        event,
                    })
                    .collect();
                tree.insert_events(&rows).expect("events");
                written.extend(batch);
                start_seq += take as u64;
                idx += 1;
                pos += take;
            }

            let items = replay(&tree, dir.path(), &sid, Offset::new(0, 0))
                .expect("replay").collect().expect("collect");
            prop_assert_eq!(items.len(), events.len(), "回放条数 == 写入条数");
            for (i, item) in items.iter().enumerate() {
                prop_assert_eq!(item.offset.seq, i as u64, "全局 seq 逐 +1(跨段连续)");
                prop_assert_eq!(&item.event, &written[i], "事件内容逐项保真");
            }
        }
    }
}
