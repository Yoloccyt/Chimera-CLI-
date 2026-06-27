//! QEEP 核心类型定义
//!
//! 包含量子纠缠执行协议的核心领域类型:
//! - `EntangledCallId`:纠缠调用唯一标识
//! - `Request` / `Ack` / `Receipt`:请求-确认-回执三元组
//! - `CallState`:调用状态机
//! - `EntangledCall`:聚合视图(供调用者查询)
//! - `OrphanReport`:孤儿调用报告

use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::QeepError;

/// 纠缠调用 ID,全局唯一。
///
/// 基于 UUIDv7(含时间戳),便于按时间排序与追溯。
/// 对应架构:每个 async 操作经 `entangle()` 包裹后获得此 ID,
/// 用于在 Event Bus 中追踪调用生命周期。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntangledCallId(pub Uuid);

/// 调用请求,包裹用户 payload 与超时配置。
///
/// 对应 QEEP 三元组的第一个环节:请求发出 → 执行单元接收。
#[derive(Debug, Clone)]
pub struct Request<T> {
    /// 调用唯一 ID
    pub id: EntangledCallId,
    /// 用户 payload(任意类型)
    pub payload: T,
    /// 请求创建时间
    pub created_at: DateTime<Utc>,
    /// 超时窗口(超过则告警 + 重试)
    pub timeout: Duration,
}

/// 确认回执,表示执行单元已接收请求并开始处理。
///
/// 对应 QEEP 三元组的第二个环节:执行单元 → 调用者。
/// Ack 的存在证明 future 已被 poll,不再是"未启动"状态。
#[derive(Debug, Clone)]
pub struct Ack {
    /// 对应的调用 ID
    pub id: EntangledCallId,
    /// 确认时间
    pub acknowledged_at: DateTime<Utc>,
}

/// 完成回执,包含最终结果。
///
/// 对应 QEEP 三元组的第三个环节:执行单元 → 调用者。
/// Receipt 的存在证明调用已终结(成功或失败),不再是孤儿。
#[derive(Debug, Clone)]
pub struct Receipt<T> {
    /// 对应的调用 ID
    pub id: EntangledCallId,
    /// 最终结果(成功值或错误)
    pub result: Result<T, QeepError>,
    /// 完成时间
    pub completed_at: DateTime<Utc>,
}

/// 调用状态机
///
/// 描述纠缠调用从创建到终结的生命周期。
/// 状态转移:Pending → Acknowledged → Completed
///                 ↘ Timeout / Failed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    /// 已创建但未确认(等待 Ack)
    Pending,
    /// 已确认,执行中(已收到 Ack)
    Acknowledged,
    /// 已完成(已收到 Receipt,成功)
    Completed,
    /// 超时(超过 timeout 窗口)
    Timeout,
    /// 失败(执行单元返回错误,或被检测为孤儿)
    Failed,
}

/// 纠缠调用聚合视图,供调用者查询完整状态。
///
/// 这是一个"快照"类型,protocol 内部不直接存储 `EntangledCall<T>`
/// (因为 T 是泛型,无法统一存储),而是存储非泛型的 `CallRecord`。
/// 调用者可通过 protocol 提供的方法获取 `EntangledCall<T>` 视图。
#[derive(Debug, Clone)]
pub struct EntangledCall<T> {
    /// 原始请求
    pub request: Request<T>,
    /// 当前状态
    pub state: CallState,
    /// 确认回执(若已 Ack)
    pub ack: Option<Ack>,
    /// 完成回执(若已 Completed)
    pub receipt: Option<Receipt<T>>,
}

/// 孤儿调用报告,记录未 await 完成的 future。
///
/// 当 `OrphanGuard` 被 drop 且 `completed` 标志为 false 时生成此报告。
/// 对应 Claude Code 尸检教训:5.4% 孤儿调用(void Promise 无 await),
/// QEEP 通过运行时检测从机制上杜绝此类问题。
#[derive(Debug, Clone)]
pub struct OrphanReport {
    /// 孤儿调用的 ID
    pub call_id: EntangledCallId,
    /// 调用创建时间
    pub created_at: DateTime<Utc>,
    /// 检测到孤儿的时间
    pub orphaned_at: DateTime<Utc>,
    /// 孤儿原因(人类可读描述)
    pub reason: String,
}
