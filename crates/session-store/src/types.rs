//! 核心类型 — SessionId / SegmentId / Offset / SessionEvent 与存储配置
//!
//! 对应架构层: **L3 Storage**（session-store,Phase 2 新增,ADR-141）
//! 对应任务: **P2-T2**（手册 W9 T-07 / ADR-108 CBMR 微批写 / v4.0 WI-18 存储面）
//!
//! # SessionEvent 与 Checkpoint 的关系（WHY）
//!
//! L0 `nexus_contracts::Checkpoint`（checkpoint.rs）是 Quest 状态的**线性快照**
//! （serialized_state = MessagePack 序列化状态,恢复时整体加载）。
//! 本 crate 的 `SessionEvent` 是**增量事件流**（append-only 段,逐条持久化）:
//!
//! - **store 是 Checkpoint 快照的「事件流补充」**:恢复路径 = 先加载 Checkpoint
//!   基线快照,再回放事件流补齐快照之后的增量（P2-T3 双写兼容设计）。
//! - 快照适合低频全量落盘,事件流适合高频增量追加;两者互补而非替代。
//! - `SessionEvent` 引用 L0 `EventMetadata`（event_id/timestamp/source,
//!   nexus-contracts/src/event_metadata.rs）:UUIDv7 时间有序,支持跨进程因果追踪;
//!   nexus-contracts **无现成会话事件类型**（已读 checkpoint.rs / app.rs 确认,
//!   app.rs 仅有 ThreadId/TurnId/ItemId 协议三原语),故在本 crate 定义。
//!
//! # Offset 双键语义（P2-T3 k-way 归并铺路,ADR-109）
//!
//! `Offset` 由两个字段构成:
//! - `seq`:全局单调序列号（会话内跨段连续,由调用方/上层维护单调性）
//! - `row`:段内行号（0-based,定位 JSONL 段文件内物理位置）
//!
//! WHY 双键:T3 的 k-way 归并回放按 `seq` 排序合并多段文件（ADR-109）,
//! 而单段内按 `row` 直接索引物理行;`Ord` 实现以 `seq` 为第一排序键,
//! 保证任意多段的 Offset 序列严格单调。

use std::path::PathBuf;

use nexus_contracts::EventMetadata;
use serde::{Deserialize, Serialize};

// ============================================================
// 标识 newtype（对齐 nexus-contracts ThreadId 的 Box<str> 风格）
// ============================================================

/// 会话 ID — 一段对话/任务执行流的唯一标识
///
/// WHY `Box<str>`:与 L0 `ThreadId` 同构（nexus-contracts/src/app.rs）,
/// 字符串 ID 可编码 QuestSession 组合键（goal_id + run_id）;`Box<str>`
/// 相比 `String` 少 8 字节堆头,Hash 键场景内存更紧凑。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(pub Box<str>);

impl SessionId {
    /// 由任意字符串创建会话 ID
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(Box::from(id.into()))
    }

    /// 生成新会话 ID（UUIDv7,时间有序）
    ///
    /// WHY UUIDv7:与 `EventMetadata.event_id` 同源（时间有序,便于审计排序）
    #[must_use]
    pub fn generate() -> Self {
        Self(Box::from(uuid::Uuid::now_v7().to_string()))
    }

    /// 底层字符串引用
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 段 ID — SQLite `segments` 表主键（全局唯一）
///
/// WHY 独立于 SessionId + segment_index:fork 复制父段元数据行时需要为
/// 复制行分配新主键,若用「会话.索引」复合编码则复制行无法复用父段身份;
/// 全局 UUID 主键使段行可被多会话引用（零数据拷贝的引用链根基）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SegmentId(pub Box<str>);

impl SegmentId {
    /// 由任意字符串创建段 ID
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(Box::from(id.into()))
    }

    /// 生成新段 ID（UUIDv7）
    #[must_use]
    pub fn generate() -> Self {
        Self(Box::from(uuid::Uuid::now_v7().to_string()))
    }

    /// 底层字符串引用
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SegmentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ============================================================
// Offset（双键,P2-T3 归并铺路）
// ============================================================

/// 事件 Offset — 段内行号 + 全局序列号双键（ADR-109 归并排序键）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Offset {
    /// 全局单调序列号（会话内跨段连续;T3 k-way 归并的第一排序键）
    pub seq: u64,
    /// 段内行号（0-based;定位 JSONL 段文件内物理行）
    pub row: u64,
}

impl Offset {
    /// 构造双键 Offset
    #[must_use]
    pub const fn new(seq: u64, row: u64) -> Self {
        Self { seq, row }
    }
}

impl PartialOrd for Offset {
    /// 以 `seq` 为第一排序键,`row` 为第二排序键
    ///
    /// WHY:跨段归并时不同段的 row 独立编号不可比,必须按全局 seq 排序;
    /// 同段内 seq 与 row 同步递增,row 兜底保证严格全序（测试断言严格单调）。
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Offset {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.seq
            .cmp(&other.seq)
            .then_with(|| self.row.cmp(&other.row))
    }
}

impl std::fmt::Display for Offset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Offset(seq={}, row={})", self.seq, self.row)
    }
}

// ============================================================
// SessionEvent（会话事件流载荷）
// ============================================================

/// 会话事件 — append-only 段的最小持久化单元
///
/// # 与 Checkpoint 的关系（见模块文档）
///
/// - `metadata`:L0 `EventMetadata`（event_id UUIDv7 时间有序 / timestamp /
///   source / correlation_id / payload_version）——引用 L0 契约类型,
///   保证跨层审计一致性（依赖铁律 §2.2:L3 → L0 恒允许）
/// - `event_type`:开放字符串事件类型名（如 `"message.turn"` / `"tool.result"`）
/// - `payload`:可选业务负载字节（调用方按自身格式编码,存储层不透传解析）
///
/// WHY event_type 用 String 而非封闭枚举:存储层对事件类型保持开放
/// （封闭语义枚举留给上层转译层,对齐 app.rs 枚举密封精神但存储面需版本演进）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEvent {
    /// 事件元数据（L0 契约:event_id/timestamp/source/correlation_id）
    pub metadata: EventMetadata,
    /// 事件类型名（开放字符串,如 "message.turn"）
    pub event_type: String,
    /// 可选业务负载字节（存储层不解析）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Vec<u8>>,
}

impl SessionEvent {
    /// 创建事件（metadata 自动生成 event_id + timestamp）
    #[must_use]
    pub fn new(event_type: impl Into<String>) -> Self {
        Self {
            metadata: EventMetadata::new("session-store"),
            event_type: event_type.into(),
            payload: None,
        }
    }

    /// 创建带业务负载的事件
    #[must_use]
    pub fn with_payload(event_type: impl Into<String>, payload: Vec<u8>) -> Self {
        Self {
            metadata: EventMetadata::new("session-store"),
            event_type: event_type.into(),
            payload: Some(payload),
        }
    }

    /// 以显式元数据创建事件（测试与外部转译层用）
    #[must_use]
    pub fn from_metadata(event_type: impl Into<String>, metadata: EventMetadata) -> Self {
        Self {
            metadata,
            event_type: event_type.into(),
            payload: None,
        }
    }
}

// ============================================================
// StoreConfig（存储配置）
// ============================================================

/// 会话存储配置 — 段滚动阈值 / 微批参数 / 自适应窗口 / 数据目录
///
/// # 微批参数语义（手册 §10.4,ADR-108）
///
/// - `batch_size`:微批最大事件数（默认 64）——pending 队列满该数立即刷写,
///   不等窗口到期（「≤64 条 / 2ms 自适应窗口」的上界）
/// - `base_window_ms`:自适应窗口基准（默认 2ms）——实际窗口在 1-4ms 间
///   按近期批大小反向调整:批大（吞吐高）缩窗至 1ms 降低延迟,
///   批小（吞吐低）扩窗至 4ms 攒批减少 IO 次数（文档注明"自适应"语义:
///   窗口 = 期望攒到足够事件的时间上限,随吞吐负反馈）
/// - `spawn_flush_loop`:是否自动启动后台定时刷写任务（默认 true;测试
///   关闭以获得确定性批触发;无 tokio runtime 环境自动不启动）
#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// 段文件最大行数,超过则滚动新段（默认 4096）
    pub max_rows_per_segment: u64,
    /// 微批最大事件数（默认 64）
    pub batch_size: usize,
    /// 自适应窗口基准毫秒数（默认 2;实际 1-4ms 负反馈调整）
    pub base_window_ms: u64,
    /// 是否自动启动后台定时刷写任务（默认 true）
    pub spawn_flush_loop: bool,
    /// 数据目录（JSONL 段文件 + SQLite 树索引）
    pub data_dir: PathBuf,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            max_rows_per_segment: 4096,
            batch_size: 64,
            base_window_ms: 2,
            spawn_flush_loop: true,
            data_dir: PathBuf::from("data/sessions"),
        }
    }
}

impl StoreConfig {
    /// 以指定数据目录构建默认配置
    #[must_use]
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: dir.into(),
            ..Self::default()
        }
    }

    /// 供测试用的紧凑配置（小段阈值 + 关闭后台任务,确定性批触发）
    #[must_use]
    pub fn test_config(dir: impl Into<PathBuf>) -> Self {
        Self {
            max_rows_per_segment: 8,
            batch_size: 4,
            base_window_ms: 2,
            spawn_flush_loop: false,
            data_dir: dir.into(),
        }
    }
}
