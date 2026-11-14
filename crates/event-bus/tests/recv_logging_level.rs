//! `BusLogger::log_recv` 的日志分级契约（Phase 6A / T6A.5）
//!
//! 锁三件事，缺一即回归：
//! 1. **普通事件降 debug**：默认 `RUST_LOG=info` 下接收面**不产生任何输出**
//!    （TUI/REPL 每 tick 都收事件，逐事件 info 会把终端刷满）；
//! 2. **Critical 仍默认可见**：`info` 过滤下必须落一行；
//! 3. **计数与日志解耦**：日志整面关掉的条件下 `total_received` 仍累加
//!    ——否则"降噪"会顺手把指标改成假零。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use event_bus::{BusLogger, EventMetadata, NexusEvent};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::{fmt, EnvFilter};

/// 内存 writer：收集 fmt 输出
#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'w> MakeWriter<'w> for Buf {
    type Writer = Buf;

    fn make_writer(&'w self) -> Self {
        self.clone()
    }
}

/// 在指定 EnvFilter 的 subscriber 下运行 body，返回其写出的日志文本
///
/// WHY `with_default`：`tracing::enabled!`（生产码里的放行判据）读的是当前默认
/// dispatcher，只有把测试 subscriber 装成默认，才能真的走到那条分支。
fn capture(filter: &str, body: impl FnOnce()) -> String {
    let buf = Buf::default();
    let subscriber = fmt::Subscriber::builder()
        .with_env_filter(EnvFilter::new(filter))
        .with_writer(buf.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, body);
    let bytes = buf.0.lock().unwrap().clone();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Normal 级事件：默认 info 过滤下不得产生接收日志
#[test]
fn normal_recv_is_silent_at_info_level() {
    let logger = BusLogger::new("test-normal");
    let event = NexusEvent::QuestCreated {
        metadata: EventMetadata::new("test-normal"),
        quest_id: "q-1".into(),
        title: "标题".into(),
        task_count: 2,
    };
    let log = capture("info", || logger.log_recv(&event));
    assert!(
        !log.contains("event_received"),
        "普通事件在 info 级不应落接收日志（降噪点），实际:\n{log}"
    );
    assert_eq!(logger.total_received(), 1, "日志降级的同时计数必须照加");
}

/// Normal 级事件：开到 debug 才可见，且落在 DEBUG 级
#[test]
fn normal_recv_is_debug_when_enabled() {
    let logger = BusLogger::new("test-debug");
    let event = NexusEvent::QuestCreated {
        metadata: EventMetadata::new("test-debug"),
        quest_id: "q-2".into(),
        title: "标题".into(),
        task_count: 2,
    };
    let log = capture("debug", || logger.log_recv(&event));
    assert!(
        log.contains("event_received") && log.contains("DEBUG"),
        "debug 级应可见且为 DEBUG，实际:\n{log}"
    );
    assert!(
        !log.contains(" level=INFO"),
        "普通事件不得以 INFO 落盘，实际:\n{log}"
    );
}

/// Critical 级事件：默认 info 过滤下仍必须可见（安全事件红线）
#[test]
fn critical_recv_stays_visible_at_info_level() {
    let logger = BusLogger::new("test-critical");
    let event = NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("test-critical"),
        quest_id: "q-3".into(),
        veto_reason: "unsafe shell injection detected".into(),
        frozen_capabilities: vec!["cap-1".into()],
    };
    assert_eq!(event.severity(), event_bus::EventSeverity::Critical);
    let log = capture("info", || logger.log_recv(&event));
    assert!(
        log.contains("event_received") && log.contains("INFO"),
        "Critical 必须默认可见，实际:\n{log}"
    );
    assert!(
        log.contains("SkepticVeto"),
        "event_type 必须在字段里，实际:\n{log}"
    );
    assert_eq!(logger.total_received(), 1);
}

/// 负对照：日志整面关掉时，计数仍累加（证明"降噪 ≠ 不统计"）
#[test]
fn counters_advance_even_with_logging_off() {
    let logger = BusLogger::new("test-off");
    let normal = NexusEvent::QuestCreated {
        metadata: EventMetadata::new("test-off"),
        quest_id: "q-4".into(),
        title: "标题".into(),
        task_count: 1,
    };
    let critical = NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("test-off"),
        quest_id: "q-4".into(),
        veto_reason: "veto".into(),
        frozen_capabilities: vec![],
    };
    let log = capture("off", || {
        logger.log_recv(&normal);
        logger.log_recv(&critical);
    });
    assert!(log.is_empty(), "filter=off 不应有任何输出，实际:\n{log}");
    assert_eq!(logger.total_received(), 2, "计数不得因日志关闭而丢");
}
