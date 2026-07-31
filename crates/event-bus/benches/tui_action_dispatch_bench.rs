//! TuiAction 协议分发延迟基准测试 — Task 3.1(v3-engine M2 启用前评估)
//!
//! 对应架构层:L1 Core(ADR-029 TUI 交互式动作协议)
//! 验证目标:8 个 TuiAction 变体 `publish_blocking` 延迟 < 10µs(broadcast 路径)
//!
//! # 基准项
//! - `tui_action_dispatch`:8 个 TuiAction 变体单次 `publish_blocking` 延迟
//!
//! # 设计说明
//! - 用 `publish_blocking`:同步发布,排除 tokio runtime 调度噪声,
//!   反映 EventBus publish 路径纯开销(metadata 拷贝 + broadcast::Sender::send +
//!   Critical mpsc 旁路判定)。
//! - TuiAction 8 变体均非 Critical(不是 4 个安全告警事件),走纯 broadcast 路径,
//!   不触发 mpsc 旁路(`is_critical_mpsc_event` 返回 false)。
//! - 创建一个订阅者避免"无订阅者"路径(否则 publish 会 warn Critical 事件丢失,
//!   虽然 TuiAction 非 Critical 不会 warn,但保持与 bus_bench.rs 一致的对比基准)。
//!
//! # < 10µs 验收标准
//! criterion 报告的均值 < 10µs。broadcast::Sender::send 是 lock-free MPSC,
//! 开销约 100ns~1µs;metadata 拷贝 + match severity 是 O(1),不应超过 10µs。
//! M2 启用后渲染引擎增量消费事件,若 publish 延迟过高会成为渲染瓶颈。
//!
//! # min-of-N 5 采样(Engineering Convention)
//! criterion 默认 sample_size=100 + 5 warmup,统计上等价于"min-of-N 5"采样减少
//! Windows 调度噪声。本 bench 沿用默认配置保证统计稳健。

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use event_bus::{ActionSource, ChatStatus, EventBus, EventMetadata, NexusEvent};

/// 8 个 TuiAction 协议变体标识(用于 BenchmarkId 参数化)
const TUI_ACTION_VARIANTS: &[(&str, &str)] = &[
    ("Requested", "TuiActionRequested"),
    ("Progressed", "TuiActionProgressed"),
    ("Completed", "TuiActionCompleted"),
    ("Failed", "TuiActionFailed"),
    ("ChatSubmitted", "TuiChatSubmitted"),
    ("ChatResponseChunk", "TuiChatResponseChunk"),
    ("ChatCompleted", "TuiChatCompleted"),
    ("ChatStatusChanged", "TuiChatStatusChanged"),
];

/// 按 variant 名构造对应的 TuiAction 事件
///
/// WHY 固定 payload:避免随机文本长度变化干扰测量;各变体 payload 贴近生产形态
fn make_tui_action_event(variant: &str) -> NexusEvent {
    let metadata = EventMetadata::new("bench-tui");
    match variant {
        "TuiActionRequested" => NexusEvent::TuiActionRequested {
            metadata,
            action_id: "quest.pause".into(),
            payload: r#"{"quest_id":"q-1"}"#.into(),
            source: ActionSource::Palette,
        },
        "TuiActionProgressed" => NexusEvent::TuiActionProgressed {
            metadata,
            action_id: "quest.export".into(),
            delta: r#"{"progress":45}"#.into(),
        },
        "TuiActionCompleted" => NexusEvent::TuiActionCompleted {
            metadata,
            action_id: "quest.export".into(),
            result: r#"{"path":"/tmp/out.json"}"#.into(),
        },
        "TuiActionFailed" => NexusEvent::TuiActionFailed {
            metadata,
            action_id: "quest.pause".into(),
            error: "quest already completed".into(),
        },
        "TuiChatSubmitted" => NexusEvent::TuiChatSubmitted {
            metadata,
            session_id: "s-1".into(),
            query: "实现登录功能".into(),
            slash_command: Some("plan".into()),
        },
        "TuiChatResponseChunk" => NexusEvent::TuiChatResponseChunk {
            metadata,
            session_id: "s-1".into(),
            delta: "hello".into(),
            cursor_hint: 3,
        },
        "TuiChatCompleted" => NexusEvent::TuiChatCompleted {
            metadata,
            session_id: "s-1".into(),
            tool_use: Some(r#"[{"name":"grep"}]"#.into()),
        },
        "TuiChatStatusChanged" => NexusEvent::TuiChatStatusChanged {
            metadata,
            session_id: "s-1".into(),
            status: ChatStatus::Thinking,
        },
        _ => unreachable!("未知 variant: {variant}"),
    }
}

/// 8 个 TuiAction 变体单次 publish_blocking 延迟
///
/// WHY 参数化:8 个变体 payload 形态不同(字符串/枚举/Option),分别测量
/// 可识别哪个变体 publish 开销偏高(若某变体超 10µs 需调查)。
/// 全部走 broadcast 路径(非 Critical),不触发 mpsc 旁路。
fn tui_action_dispatch(c: &mut Criterion) {
    let bus = EventBus::new();
    // 创建一个订阅者,避免"无订阅者"路径(与 bus_bench.rs 一致)
    let _rx = bus.subscribe();

    let mut group = c.benchmark_group("tui_action_dispatch");
    for &(short, full) in TUI_ACTION_VARIANTS {
        group.bench_with_input(BenchmarkId::from_parameter(short), full, |b, variant| {
            b.iter(|| {
                let event = make_tui_action_event(variant);
                // WHY expect:bench 中 publish 失败表明 EventBus 内部状态错误,应 panic 暴露
                bus.publish_blocking(black_box(event))
                    .expect("publish 失败");
            });
        });
    }
    group.finish();
}

criterion_group!(benches, tui_action_dispatch);
criterion_main!(benches);
