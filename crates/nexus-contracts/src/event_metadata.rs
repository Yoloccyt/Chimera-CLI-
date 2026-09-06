//! 事件元数据契约 — L0 共享的事件追踪元信息(Task 3.10,ADR-033 扩展)
//!
//! 对应架构层: **L0 Contracts**(从 L1 `event-bus` 上提,缓解 L1 上帝 crate)
//! 对应 ADR: **ADR-033**(L0 nexus-contracts 契约层建立,本模块为 Task 3.10 类型扩展)
//!
//! # 核心职责
//!
//! 承载事件总线每个事件携带的通用追踪元信息(event_id / timestamp / source)。
//! 原定义于 `event-bus/src/payloads.rs`,因被 100+ 文件依赖(L1 上帝 crate 病理),
//! 下沉到 L0 共享契约层,供 L1-L10 所有上层 crate 直接导入。
//!
//! # 设计约束(ADR-033 + Task 3.10 扩展)
//!
//! - **纯类型 + 基础构造函数**: 仅类型定义与 `new()` 构造函数,不含业务逻辑
//! - **新增 ADR-033 例外**: `chrono` + `uuid` 作为基础类型库加入 L0 依赖白名单
//!   (与 `serde` 同级例外,无运行时业务逻辑)
//! - **向后兼容**: `event-bus/src/payloads.rs` 保留 `pub use nexus_contracts::EventMetadata`
//!   re-export,100+ 文件现有 `use event_bus::EventMetadata` 路径不破坏
//!
//! # 字段说明
//!
//! - `event_id`: UUIDv7(时间有序),便于跨进程因果追踪与去重
//! - `timestamp`: 事件产生时刻(UTC),审计日志按此排序
//! - `source`: 发布者 crate 名(如 "osa-coordinator"),用于依赖方向校验

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::graph_identity::GraphIdentity;

/// #[serde(default)] 用默认值函数: payload_version 默认 1
fn default_payload_version() -> u32 {
    1
}

/// 事件元数据 — 每个事件携带,用于追踪、审计与因果排序
///
/// WHY 字段说明:
/// - `event_id`:UUIDv7(时间有序),便于跨进程因果追踪与去重
/// - `timestamp`:单调时钟来源,审计日志按此排序
/// - `source`:发布者 crate 名(如 "osa-coordinator"),用于依赖方向校验
/// - `correlation_id`:可选关联 ID,用于跨事件因果追踪(None 表示无关联事件)
/// - `payload_version`:载荷 schema 版本号,支持事件格式演进(默认 1)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventMetadata {
    /// 事件唯一标识(UUIDv7,时间有序)
    pub event_id: Uuid,
    /// 事件产生时刻(UTC)
    pub timestamp: DateTime<Utc>,
    /// 发布者 crate 名,用于依赖方向校验与审计
    pub source: String,
    /// 可选关联 ID,用于跨事件因果追踪(如 Quest 内多步骤关联)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    /// 载荷 schema 版本号,支持事件格式演进(默认 1=v1 格式)
    /// #[serde(default)] 确保旧格式 JSON 缺失此字段时反序列化不失败
    #[serde(default = "default_payload_version")]
    pub payload_version: u32,
    /// 图身份三元组（WI-04 GIP）— 任意 Goal/节点成本归因
    ///
    /// WHY Option 不 skip_serializing_if: rmp-serde array 位置编码下跳过字段
    /// 破坏反序列化长度（ADR-004）；缺失字段反序列化自动 None（serde 默认）。
    pub graph_identity: Option<GraphIdentity>,
}

impl EventMetadata {
    /// 以指定 source 创建元数据,event_id 与 timestamp 自动生成,
    /// correlation_id 为 None,payload_version 默认为 1
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            event_id: Uuid::now_v7(),
            timestamp: Utc::now(),
            source: source.into(),
            correlation_id: None,
            payload_version: 1,
            graph_identity: None,
        }
    }

    /// 创建带关联 ID 的元数据,用于跨事件因果追踪
    pub fn with_correlation(source: impl Into<String>, correlation_id: impl Into<String>) -> Self {
        Self {
            event_id: Uuid::now_v7(),
            timestamp: Utc::now(),
            source: source.into(),
            correlation_id: Some(correlation_id.into()),
            payload_version: 1,
            graph_identity: None,
        }
    }

    /// 创建带图身份的三元组元数据（WI-04 GIP 挂载点）
    ///
    /// # WHY
    /// 成本归因从"总账"细化到"任意 Goal/节点瀑布"（WI-04 验收:
    /// 给定 run_id 拉出完整成本瀑布）。既有 `new`/`with_correlation`
    /// 保持 graph_identity = None（零回归）。
    pub fn with_graph_identity(source: impl Into<String>, graph_identity: GraphIdentity) -> Self {
        Self {
            event_id: Uuid::now_v7(),
            timestamp: Utc::now(),
            source: source.into(),
            correlation_id: None,
            payload_version: 1,
            graph_identity: Some(graph_identity),
        }
    }
}
