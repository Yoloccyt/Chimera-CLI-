//! 线程回放重构器 — 事件流 → Item 序列 + Offset↔ItemId 游标映射（ADR-292 D2/D3）
//!
//! 对应架构层: **L3 Storage**（session-store;ADR-292 P3 桥——threads 404 回放安全网）
//! 对应任务: ADR-292 D2（event→Item 纯函数重构器）+ D3（Offset↔ItemId 游标映射）
//!
//! # 消费场景（ADR-292 §背景,蓝图 §二/§5.5）
//!
//! Anthropic Message threads 的 `continue` 返回 404（服务端线程 best-effort,
//! 空闲过期）时，harness 必须重新 `create`——按 Thread 段**完整回放重建对话**。
//! append-only 会话流（ADR-141）天然持有重建所需的完整对话：本模块把
//! `thread.started` / `item.append` 事件流（D1 schema）重构回协议面
//! `Vec<Item>`，使「自己的持久化兜底」可执行。
//!
//! # 纯函数纪律（D2）
//!
//! 输入事件切片、输出重构结果——无 IO 无状态，与回放的 I/O 面（[`crate::replay`]
//! k-way 归并）解耦，可独立单测。
//!
//! # 旧数据容忍（ADR-292 验收线）
//!
//! 仅含 `turn.submit` 的历史会话流（D1 之前的数据）→ 空/部分结果,**不 panic**：
//! 重构器只识别 `thread.started` / `item.append` 两类事件,其余类型跳过;
//! `item.append` 缺载荷或载荷解析失败的事件同样跳过（写入侧序列化降级 /
//! 段文件损坏尾巴的容忍面）。
//!
//! # 游标语义（D3）
//!
//! AppServer 内存游标是 ItemId 字符串、store 游标是 Offset（u64）。
//! [`rebuild_items`] 输出的对账表 [`CursorEntry`] 按**输入切片下标**填充
//! `offset`——依据本存储的已测不变量：**全量（from=0）回放流中 Offset.seq
//! 从 0 起逐 +1**（`replay` 测试锁定）,故全量流下下标 == store Offset.seq;
//! 带 from 的后缀流请由调用方按 (真实 offset, event) 对齐后再切片。

use nexus_contracts::app::{Item, ItemId, ThreadId, TurnId};

use crate::types::{
    ItemAppendPayload, SessionEvent, ThreadStartedPayload, EVENT_TYPE_ITEM_APPEND,
    EVENT_TYPE_THREAD_STARTED,
};

/// (Offset, ItemId) 对账条目 — store 游标 ↔ 协议游标（ADR-292 D3）
///
/// `offset` = 该 `item.append` 事件在输入切片中的下标（全量 from=0 流下
/// == store Offset.seq,见模块文档游标语义）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorEntry {
    /// store 侧游标（输入切片下标;全量流 == Offset.seq）
    pub offset: u64,
    /// 协议面游标（Item ID 字符串形态）
    pub item_id: String,
}

/// 事件流重构结果（ADR-292 D2/D3）
#[derive(Debug, Clone, PartialEq)]
pub struct RebuildResult {
    /// 重构出的 Item 序列（事件流时间序;逐字段与原始写入一致——零丢失验收线）
    pub items: Vec<Item>,
    /// (Offset, ItemId) 对账表（按事件流顺序;仅 `item.append` 事件入表）
    pub cursors: Vec<CursorEntry>,
}

/// `thread.started` 事件构造 — 线程元数据落盘（ADR-292 D1 写侧镜像）
///
/// # 序列化降级
/// 纯数据结构的 serde 序列化无失败路径（无运行时输入）,但库红线禁
/// unwrap——序列化失败（不可达）降级为无载荷事件,重构端按缺载荷跳过,
/// 不 panic。
#[must_use]
pub fn thread_started_event(thread_id: &str, created_at_ms: u64) -> SessionEvent {
    let payload = ThreadStartedPayload {
        thread_id: thread_id.to_string(),
        created_at: created_at_ms,
    };
    build_event(EVENT_TYPE_THREAD_STARTED, &payload)
}

/// `item.append` 事件构造 — Item 级内容落盘（ADR-292 D1 写侧镜像）
///
/// 载荷为 Item 协议字段的完整镜像（字段取舍依据见 [`ItemAppendPayload`] 文档）。
#[must_use]
pub fn item_append_event(item: &Item) -> SessionEvent {
    let payload = ItemAppendPayload {
        item_id: item.item_id.as_str().to_string(),
        item_kind: item.kind.to_string(),
        payload: item.payload.to_string(),
        turn_id: item.turn_id.as_str().to_string(),
        status: item.status,
        extras: item.extras.as_ref().map(std::string::ToString::to_string),
    };
    build_event(EVENT_TYPE_ITEM_APPEND, &payload)
}

/// 载荷序列化 → 事件构造（写侧共用;失败降级无载荷,WHY 见 [`thread_started_event`]）
fn build_event<P: serde::Serialize>(event_type: &str, payload: &P) -> SessionEvent {
    match serde_json::to_vec(payload) {
        Ok(bytes) => SessionEvent::with_payload(event_type, bytes),
        // 不可达分支:纯数据结构序列化无失败路径;降级保持事件可落盘（审计面
        // 「发生了该事件」仍成立,内容缺失由重构端容忍）
        Err(_) => SessionEvent::new(event_type),
    }
}

/// 纯函数重构器 — 事件切片 → Item 序列 + 游标对账表（ADR-292 D2/D3 核心）
///
/// # 识别语义
/// - `thread.started`：更新流上下文的线程身份（后续 Item 的 thread_id 来源）;
///   载荷缺失/损坏时保留上一身份（写入侧契约:thread.started 为流首事件）
/// - `item.append`：载荷解析成功 → 产出 Item + 对账条目;缺载荷/解析失败 →
///   跳过（旧数据/损坏容忍,不中断重构）
/// - 其余类型（`turn.submit` 等）：跳过——它们是回合语义事件,不参与 Item 重建
///
/// # thread_id 退化
/// `item.append` 先于任何 `thread.started` 出现（非本 crate 写入侧产出的形态,
/// 或 thread.started 因 N-4 降级未落盘）时,重构 Item 的 thread_id 为空串占位;
/// 消费方（AppServer 回放桥）以请求的线程身份归一。
#[must_use]
pub fn rebuild_items(events: &[SessionEvent]) -> RebuildResult {
    let mut items: Vec<Item> = Vec::new();
    let mut cursors: Vec<CursorEntry> = Vec::new();
    // 流上下文:最近一次 thread.started 的线程身份（Item.thread_id 的来源）
    let mut thread_id = String::new();

    for (pos, ev) in events.iter().enumerate() {
        match ev.event_type.as_str() {
            EVENT_TYPE_THREAD_STARTED => {
                if let Some(p) = ev
                    .payload
                    .as_deref()
                    .and_then(|bytes| serde_json::from_slice::<ThreadStartedPayload>(bytes).ok())
                {
                    thread_id = p.thread_id;
                }
            }
            EVENT_TYPE_ITEM_APPEND => {
                // 缺载荷/解析失败 → 跳过该条（容忍,不中断;WHY 见模块文档旧数据容忍）
                let Some(p) = ev
                    .payload
                    .as_deref()
                    .and_then(|bytes| serde_json::from_slice::<ItemAppendPayload>(bytes).ok())
                else {
                    continue;
                };
                cursors.push(CursorEntry {
                    offset: pos as u64,
                    item_id: p.item_id.clone(),
                });
                // WHY Item::new + with_extras 而非结构体字面量:Item 是
                // #[non_exhaustive]（L0 协议冻结面）,跨 crate 禁字面量构造
                let mut item = Item::new(
                    ItemId::new(p.item_id),
                    ThreadId::new(thread_id.clone()),
                    TurnId::new(p.turn_id),
                    &p.item_kind,
                    p.status,
                    &p.payload,
                );
                if let Some(extras) = p.extras {
                    item = item.with_extras(extras);
                }
                items.push(item);
            }
            // turn.submit 等其余类型:回合语义事件,不参与 Item 重建（旧数据容忍）
            _ => {}
        }
    }

    RebuildResult { items, cursors }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;

    /// 构造测试 Item（id/kind/status/payload 逐条可区分;with_extras 可选）
    fn item(n: usize, extras: Option<&str>) -> Item {
        let status = match n % 4 {
            0 => nexus_contracts::app::ItemStatus::Started,
            1 => nexus_contracts::app::ItemStatus::InProgress,
            2 => nexus_contracts::app::ItemStatus::Completed,
            _ => nexus_contracts::app::ItemStatus::Failed,
        };
        let it = Item::new(
            ItemId::new(format!("turn-{}-{}", n / 2 + 1, n % 2 + 1)),
            ThreadId::new("goal-1::run-1"),
            TurnId::new(format!("turn-{}", n / 2 + 1)),
            if n.is_multiple_of(2) { "message" } else { "tool_call" },
            status,
            &format!(r#"{{"content":"内容-{n}","n":{n}}}"#),
        );
        match extras {
            Some(e) => it.with_extras(e),
            None => it,
        }
    }

    /// 标准线程流:thread.started + N 条 item.append（写入侧镜像顺序）
    fn seed_events(n: usize) -> (Vec<SessionEvent>, Vec<Item>) {
        let originals: Vec<Item> = (0..n).map(|i| item(i, if i == 2 { Some("extras-2") } else { None })).collect();
        let mut events = vec![thread_started_event("goal-1::run-1", 1_700_000_000_000)];
        for it in &originals {
            events.push(item_append_event(it));
        }
        (events, originals)
    }

    #[test]
    fn rebuild_full_thread_stream_matches_originals_field_by_field() {
        // 完整线程流重建:逐条一致（零丢失验收线,含 status/extras 往返）
        let (events, originals) = seed_events(4);
        let result = rebuild_items(&events);
        assert_eq!(result.items.len(), originals.len(), "重建条数 == 写入条数");
        for (a, b) in result.items.iter().zip(&originals) {
            assert_eq!(a, b, "Item 逐字段一致(含 status/extras 往返)");
        }
        // thread_id 来自流上下文（thread.started 事件）
        assert!(result
            .items
            .iter()
            .all(|i| i.thread_id.as_str() == "goal-1::run-1"));
    }

    #[test]
    fn legacy_turn_submit_only_stream_tolerated() {
        // 旧数据容忍:仅 turn.submit + 未知类型 → 空结果,不 panic
        let events = vec![
            SessionEvent::with_payload("turn.submit", br#"{"turn_id":"turn-1"}"#.to_vec()),
            SessionEvent::new("session.flushed"),
        ];
        let result = rebuild_items(&events);
        assert!(result.items.is_empty(), "旧数据流重建为空");
        assert!(result.cursors.is_empty());
    }

    #[test]
    fn degraded_item_append_events_skipped_not_panic() {
        // 缺载荷 / 载荷损坏的 item.append 跳过,不中断后续正常条目
        let (mut events, originals) = seed_events(3);
        // 在第 1 条后插入两条退化事件:缺载荷 + 载荷损坏
        events.insert(2, SessionEvent::new(EVENT_TYPE_ITEM_APPEND));
        events.insert(
            3,
            SessionEvent::with_payload(EVENT_TYPE_ITEM_APPEND, vec![0xFF, 0xFE, 0x00]),
        );
        let result = rebuild_items(&events);
        assert_eq!(
            result.items.len(),
            originals.len(),
            "退化事件被跳过,正常条目全保留"
        );
        for (a, b) in result.items.iter().zip(&originals) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn cursor_map_roundtrip_within_stream() {
        // 游标映射往返:对账表 offset == 切片下标 == 全量流 Offset.seq（不变量）;
        // 按游标 resume（游标后切片重构）得到后缀序列
        let (events, originals) = seed_events(4);
        let result = rebuild_items(&events);
        // thread.started 在下标 0,item.append 从下标 1 起
        for (k, cursor) in result.cursors.iter().enumerate() {
            assert_eq!(cursor.offset, (k + 1) as u64, "游标 offset = 事件切片下标");
            assert_eq!(
                cursor.item_id,
                originals[k].item_id.as_str(),
                "游标 item_id 与写入顺序一致"
            );
        }
        // 游标往返:以 cursors[1].offset 为 resume 点 → 其后事件重构出后缀序列
        let resume_at = result.cursors[1].offset as usize;
        let suffix = rebuild_items(&events[resume_at + 1..]);
        assert_eq!(suffix.items.len(), originals.len() - 2, "resume 后缀条数正确");
        for (a, b) in suffix.items.iter().zip(&originals[2..]) {
            // WHY 只比内容字段不比 thread_id:后缀切片不含 thread.started,
            // 线程身份退化为空串占位（模块文档 thread_id 退化语义）
            assert_eq!(a.item_id, b.item_id);
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.payload, b.payload);
            assert_eq!(a.status, b.status);
            assert_eq!(a.turn_id, b.turn_id);
        }
        // thread.started 缺席 → thread_id 空串占位（退化语义可预期）
        assert!(suffix.items.iter().all(|i| i.thread_id.as_str().is_empty()));
    }

    #[test]
    fn thread_started_payload_corruption_keeps_previous_context() {
        // thread.started 载荷损坏:保留上一线程身份,不 panic
        let events = vec![
            thread_started_event("goal-1::run-1", 1),
            item_append_event(&item(0, None)),
            // 损坏的 thread.started:载荷非合法 JSON
            SessionEvent::with_payload(EVENT_TYPE_THREAD_STARTED, vec![0xDE, 0xAD]),
            item_append_event(&item(1, None)),
        ];
        let result = rebuild_items(&events);
        assert!(result
            .items
            .iter()
            .all(|i| i.thread_id.as_str() == "goal-1::run-1"),
            "损坏的 thread.started 不清空流上下文");
    }
}
