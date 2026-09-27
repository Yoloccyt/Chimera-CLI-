//! Critical 事件保底送达 sink(B-a,M0;`arch-refactor-directions-v2-2026-09-12` 方向 B-a)
//!
//! # 背景(痛点证据)
//!
//! §6.2 红线要求 Critical 安全/治理告警事件"确保送达"。既有实现的双通道:
//! broadcast(主通道)+ Critical mpsc 旁路(保障通道)。但旁路消费者由**组合根
//! 显式接线**(`spawn_critical_subscriber`),实测全库 9 处生产 `EventBus::new()`
//! 中仅 `composition.rs::build_app_server` 一处接线 —— 其余 8 条路径
//! (chat/run/exec/quest/parliament/agent/tui/doctor)的 Critical 事件在
//! 无订阅者分支被 **warn + 放弃**,旁路空转,"确保送达"承诺落空。
//!
//! # 本模块的修复
//!
//! 把"必须有对端"从组合根义务下沉为总线自身保证:发布路径的空订阅者分支
//! 改为投递到可插拔的 [`CriticalSink`](CriticalSink)。默认实现
//! [`LogCriticalSink`](LogCriticalSink) 将事件以结构化 `error!` 级日志落盘
//! (携带 event_type/severity/event_id/source 四个定位字段),任何
//! `EventBus::new()` 路径**零接线即获得保底落点**。
//!
//! # 语义边界(设计决策记录)
//!
//! - **不回放**:sink 是"发布时刻的保底落点",不缓存事件、不向后续订阅者补发
//!   —— `subscribe_critical_events` "从订阅时刻开始接收,不回放历史"的既有
//!   语义保持不变(见 `EventBus::subscribe_critical_events` 文档)。
//! - **可能重复**:并发交错下同一事件可能同时进入 mpsc 订阅者与 sink
//!   (检查订阅者与投递非原子)。保底通道的重复落盘无害;丢失才有害。
//!   因此 sink 语义是 **at-least-once 观测**,不是恰好一次投递。
//! - **不替代真实消费者**:组合根的显式订阅者仍是"结构化消费"的推荐路径
//!   (可升级为 TUI 面板/告警管道);sink 只是不可绕过的最低保障。

use crate::types::{EventSeverity, NexusEvent};
use std::fs::OpenOptions;
use std::io::{LineWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Critical 事件保底送达 sink(发布路径空订阅者分支的投递目标)
///
/// # 实现约束
///
/// - `Send + Sync + 'static`:`EventBus` 派生 `Clone`(内部 `Arc<dyn CriticalSink>`
///   跨线程共享),sink 必须可安全跨线程调用;发布路径为同步上下文,实现体
///   **不得阻塞、不得 panic**(§4.4 红线:热路径禁止阻塞调用)。
/// - **at-least-once 语义**:见模块文档"可能重复"条目,实现方不得假设恰好一次。
///
/// # 示例
///
/// ```no_run
/// use std::sync::Arc;
/// use event_bus::{EventBus, CriticalSink, NexusEvent};
///
/// struct CollectingSink;
/// impl CriticalSink for CollectingSink {
///     fn on_critical(&self, event: &NexusEvent) {
///         // 生产环境可转发到告警管道/持久化存储;此处仅演示
///         let _ = event.type_name();
///     }
/// }
///
/// // 无 mpsc 订阅者时,Critical 事件投递到 CollectingSink 而非被放弃
/// let bus = EventBus::new()
///     .with_critical_fallback(Arc::new(CollectingSink));
/// ```
pub trait CriticalSink: Send + Sync + 'static {
    /// 接收一条无 mpsc 订阅者的 Critical 事件(保底落点)
    ///
    /// # 参数
    /// - `event`:发布路径判入 Critical 清单(`is_critical_mpsc_event`)的事件
    ///   借用引用;实现方如需持有请自行 clone(发布方不保证生命周期)。
    ///
    /// # 返回
    /// 无返回值:保底通道不允许失败路径(实现方内部降级,不得上抛)。
    fn on_critical(&self, event: &NexusEvent);
}

/// 默认保底 sink — 结构化 `error!` 级日志
///
/// WHY `error!` 而非既有 `warn!`:warn 语义是"告警但事件已放弃";保底送达
/// 之后事件已有落点,error 级更准确表达"该事件未到达任何真实消费者,请运维
/// 关注"。字段与既有 C3 观测一致(event_type/severity/event_id),另补
/// `source`(事件来源 crate,定位发布方)。
pub struct LogCriticalSink;

impl CriticalSink for LogCriticalSink {
    fn on_critical(&self, event: &NexusEvent) {
        tracing::error!(
            event_type = %event.type_name(),
            severity = ?event.severity(),
            event_id = %event.metadata().event_id,
            source = %event.metadata().source,
            is_critical = event.severity() == EventSeverity::Critical,
            "Critical 事件保底送达(无 mpsc 订阅者,结构化日志落点)"
        );
    }
}

/// WAL 保底 sink(E-1 / ADR-191 D4)——把无 mpsc 订阅者的 Critical 事件以
/// JSON Lines 追加落盘,提供崩溃后 `replay()` 审计能力(**at-least-once**)。
///
/// WHY `LineWriter`+`Mutex` 而非后台线程:`on_critical` 仅在 "Critical 且无 mpsc
/// 订阅者" 低频分支触发(非高频广播),`LineWriter` 逐行 write 为 µs 级同步 append,
/// Critical 本身低频,不违反 "publish 热路径不阻塞"红线(该分支为异常保底,
/// 默认 LogCriticalSink 的 `error!` 亦同步)。写失败**不上抛**(保底通道不得 panic/失败),
/// 降级为 `tracing::error` 并继续。
pub struct FileWalCriticalSink {
    writer: Mutex<LineWriter<std::fs::File>>,
    path: PathBuf,
}

impl FileWalCriticalSink {
    /// 打开(或创建)WAL 文件,追加模式。失败上抛(由调用方组合根决定回退到 LogCriticalSink)。
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path.as_ref())?;
        Ok(Self {
            writer: Mutex::new(LineWriter::new(file)),
            path: path.as_ref().to_path_buf(),
        })
    }

    /// WAL 文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读取并反序化 WAL 全部可解析行(损坏行跳过,尽力而为)。不改动文件。
    pub fn replay(&self) -> std::io::Result<Vec<NexusEvent>> {
        let data = std::fs::read_to_string(&self.path)?;
        Ok(data
            .lines()
            .filter_map(|l| serde_json::from_str::<NexusEvent>(l).ok())
            .collect())
    }
}

impl CriticalSink for FileWalCriticalSink {
    fn on_critical(&self, event: &NexusEvent) {
        // 序列化失败/锁毒化均不上抛:降级日志,保底通道不得 panic(§4.4)。
        let line = match serde_json::to_string(event) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "Critical WAL 序列化失败,事件未落盘");
                return;
            }
        };
        match self.writer.lock() {
            Ok(mut w) => {
                if let Err(e) = writeln!(w, "{line}") {
                    tracing::error!(error = %e, path = %self.path.display(), "Critical WAL 写入失败");
                }
                // LineWriter 已逐行 flush;此处无需显式 fsync(热路径避免阻塞);
                // 若需强持久可后续接一个后台 sync 周期(ADR-191 标为可配)。
            }
            Err(_) => {
                tracing::error!(path = %self.path.display(), "Critical WAL 锁毒化,事件未落盘")
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod file_wal_tests {
    use super::*;
    use crate::EventMetadata;

    fn quest_event(id: &str) -> NexusEvent {
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("test-harness"),
            quest_id: id.to_string(),
            title: "wal".to_string(),
            task_count: 1,
        }
    }

    #[test]
    fn file_wal_sink_persists_and_replays() {
        let path = std::env::temp_dir().join(format!("chimera_wal_{}.logl", uuid::Uuid::now_v7()));
        let sink = FileWalCriticalSink::open(&path).expect("open wal");
        sink.on_critical(&quest_event("q-1"));
        sink.on_critical(&quest_event("q-2"));
        // LineWriter 逐行 flush,无需 drop 即可读回。
        let got = sink.replay().expect("replay");
        assert_eq!(got.len(), 2, "两条 Critical 均应落盘可重放");
        assert!(got
            .iter()
            .any(|e| matches!(e, NexusEvent::QuestCreated { quest_id, .. } if quest_id == "q-1")));
        drop(sink);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_wal_sink_is_critical_sink_object_safe() {
        // 验证可作为 Arc<dyn CriticalSink> 注入(与 with_critical_fallback 契合)。
        let path = std::env::temp_dir().join(format!(
            "chimera_wal_traitobj_{}.logl",
            uuid::Uuid::now_v7()
        ));
        let sink: std::sync::Arc<dyn CriticalSink> =
            std::sync::Arc::new(FileWalCriticalSink::open(&path).expect("open"));
        sink.on_critical(&quest_event("q-3"));
        drop(sink);
        let _ = std::fs::remove_file(&path);
    }

    /// E-1 崩溃持久性契约（存储层）：一个 sink 实例写入后 drop（关闭文件句柄，
    /// 模拟持有者进程终止），用**全新实例**打开同一路径 replay 仍可检索
    /// ⇒ WAL 跨句柄/跨“重启”存活。（真进程 SIGKILL 重放 e2e 属 CI 级，本测
    /// 验证其本质契约：数据已在磁盘上、与内存句柄生命周期无关。）
    #[test]
    fn file_wal_survives_handle_reopen() {
        let path =
            std::env::temp_dir().join(format!("chimera_wal_restart_{}.logl", uuid::Uuid::now_v7()));
        {
            let sink = FileWalCriticalSink::open(&path).expect("open wal");
            sink.on_critical(&quest_event("q-restart-1"));
            sink.on_critical(&quest_event("q-restart-2"));
            // sink 在此 drop ⇒ 文件句柄关闭（模拟持有者进程结束）
        }
        // “重启”：全新实例读同一磁盘 WAL
        let reopened = FileWalCriticalSink::open(&path).expect("reopen wal");
        let got = reopened.replay().expect("replay after reopen");
        assert_eq!(
            got.len(),
            2,
            "进程终止后重启，两条 Critical 仍应可从磁盘 WAL 检索"
        );
        assert!(got.iter().any(
            |e| matches!(e, NexusEvent::QuestCreated { quest_id, .. } if quest_id == "q-restart-1")
        ));
        drop(reopened);
        let _ = std::fs::remove_file(&path);
    }

    /// WAL 撕裂韧性（崩溃真实失败模式）：kill -9 常留下未写完的最后一行。
    /// 追加一条合法记录后直接写一段残缺 JSON（无换行），replay 应跳过损坏行、
    /// 仍返回已完整落盘的事件 ⇒ 保底通道不因尾行损坏而整体不可读。
    #[test]
    fn file_wal_replay_skips_corrupt_trailing_line() {
        let path =
            std::env::temp_dir().join(format!("chimera_wal_corrupt_{}.logl", uuid::Uuid::now_v7()));
        {
            let sink = FileWalCriticalSink::open(&path).expect("open wal");
            sink.on_critical(&quest_event("q-ok"));
            // LineWriter 已 flush q-ok；再模拟崩溃时半写的尾行（截断、无换行）
            let mut raw = OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open raw");
            write!(raw, "{{\"truncated\": true").expect("append torn line");
        }
        let reopened = FileWalCriticalSink::open(&path).expect("reopen wal");
        let got = reopened.replay().expect("replay with torn tail");
        assert_eq!(got.len(), 1, "损坏尾行应被跳过，完整落盘的 q-ok 仍可检索");
        assert!(matches!(
            &got[0],
            NexusEvent::QuestCreated { quest_id, .. } if quest_id == "q-ok"
        ));
        drop(reopened);
        let _ = std::fs::remove_file(&path);
    }
}
