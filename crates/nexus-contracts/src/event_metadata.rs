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
/// - `trace_id`:分布式追踪 ID——同一逻辑请求链(含跨进程 spawn)上所有事件共享,
///   span 继承的载体;None 表示尚未纳入追踪链(根事件由 [ensure_trace_id] 懒生成)
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
    /// 分布式追踪 ID（WI-29 可观测性）— span 继承载体
    ///
    /// # 语义
    /// 同一逻辑请求链上的所有事件共享同一 `trace_id`（含跨 crate、跨
    /// `tokio::spawn` 的派生事件——经 [child_of] 传播）。与 `correlation_id`
    /// 的分工:`correlation_id` 是业务关联（如某 Quest 的多步骤），`trace_id`
    /// 是追踪链（一次用户请求引发的全部事件，含多 Quest）。
    ///
    /// # 兼容性（新增字段,向后兼容）
    /// WHY `Option` 不 `skip_serializing_if`:与 `graph_identity` 同款——
    /// rmp-serde 位置编码下跳过字段破坏反序列化长度（ADR-004）;本 crate 的
    /// MessagePack 统一走 `to_vec_named`（命名 map 编码）,旧负载缺失
    /// `trace_id` 键反序列化自动 `None`,新负载多字段对旧结构被忽略——双向兼容。
    /// JSON 侧同理（注释见 `legacy_json_without_trace_id_deserializes_to_none` 用例）。
    pub trace_id: Option<String>,
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
            trace_id: None,
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
            trace_id: None,
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
            trace_id: None,
        }
    }

    /// 设置/覆盖追踪 ID（链式）— span 继承的显式挂载点
    ///
    /// # 参数
    /// - `trace_id`: 追踪链标识（通常取自上游事件的 `trace_id`）
    ///
    /// # 返回
    /// 设置后的元数据（`self`,便于链式调用）
    pub fn with_trace(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    /// 设置追踪 ID（`Option` 形态,链式）— 可选传播场景
    ///
    /// # 参数
    /// - `trace_id`: `Some(id)` 设置;`None` 清除（回到"未纳入追踪链"）
    pub fn with_trace_opt(mut self, trace_id: Option<String>) -> Self {
        self.trace_id = trace_id;
        self
    }

    /// **span 继承传播原语** — 以父元数据派生子事件元数据
    ///
    /// # 语义
    /// 子事件获得新的 `event_id` 与 `timestamp`（自身身份），但**继承**父事件
    /// 的 `trace_id`（追踪链延续）、`correlation_id`（业务关联延续）与
    /// `graph_identity`（成本归因延续）。用于跨 crate / 跨 `tokio::spawn`
    /// 的事件派生点，使全链路共享同一追踪 ID（WI-29 验收:span 继承覆盖）。
    ///
    /// # 参数
    /// - `parent`: 父事件元数据（传播来源）
    /// - `source`: 子事件发布者 crate 名（与父不同,标识派生点）
    ///
    /// # 返回
    /// 继承追踪上下文的子事件元数据
    pub fn child_of(parent: &Self, source: impl Into<String>) -> Self {
        Self {
            event_id: Uuid::now_v7(),
            timestamp: Utc::now(),
            source: source.into(),
            correlation_id: parent.correlation_id.clone(),
            payload_version: parent.payload_version,
            graph_identity: parent.graph_identity.clone(),
            trace_id: parent.trace_id.clone(),
        }
    }

    /// 确保追踪 ID 存在 — 根事件的**懒生成**（幂等）
    ///
    /// # 语义
    /// `trace_id` 为 `None` 时,以本事件 `event_id` 的 UUID 字符串填充并返回;
    /// 已有值时直接返回既有值。用于追踪链的起点（如用户请求入口）,使
    /// 根事件不需要预先分配 ID——首个 `ensure_trace_id` 调用即锚定全链。
    ///
    /// # 返回
    /// 本事件当前的追踪 ID（借用,生命周期与 `self` 绑定）
    pub fn ensure_trace_id(&mut self) -> &str {
        self.trace_id
            .get_or_insert_with(|| self.event_id.to_string())
            .as_str()
    }

    /// 读取追踪 ID（可选,不修改）
    pub fn trace_id(&self) -> Option<&str> {
        self.trace_id.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v2.28.x 之前的 JSON 负载(无 trace_id 键)反序列化 → None(向后兼容)
    #[test]
    fn legacy_json_without_trace_id_deserializes_to_none() {
        let legacy = r#"{
            "event_id": "018f5a2e-0000-7000-8000-000000000001",
            "timestamp": "2026-09-01T00:00:00Z",
            "source": "legacy-producer",
            "correlation_id": null,
            "payload_version": 1,
            "graph_identity": null
        }"#;
        let meta: EventMetadata =
            serde_json::from_str(legacy).expect("旧 JSON 负载必须可解析(新增字段兼容)");
        assert!(meta.trace_id.is_none(), "缺失 trace_id 键 → None");
        assert_eq!(meta.source, "legacy-producer");
    }

    /// v2.28.x 之前的 MessagePack 负载(6 字段,无 trace_id 键) → None
    ///
    /// WHY 用 LegacyMetadata 结构体而非 `serde_json::Value`:Uuid 在
    /// MessagePack 的二进制格式中用 16 字节 array 编码(human-readable
    /// 格式才用字符串),Value 路径构造的负载不具真实旧版本语义——
    /// 以"旧结构体 → msgpack → 新结构体"复现真实升级路径。
    #[test]
    fn legacy_msgpack_without_trace_id_deserializes_to_none() {
        /// 旧版本(v2.28.x 前)的元数据结构——无 trace_id 字段
        #[derive(Serialize)]
        struct LegacyMetadata {
            event_id: Uuid,
            timestamp: DateTime<Utc>,
            source: String,
            correlation_id: Option<String>,
            payload_version: u32,
            graph_identity: Option<GraphIdentity>,
        }
        let legacy = LegacyMetadata {
            event_id: Uuid::now_v7(),
            timestamp: Utc::now(),
            source: "legacy-producer".into(),
            correlation_id: Some("quest-legacy".into()),
            payload_version: 1,
            graph_identity: None,
        };
        // to_vec_named = 本 crate 事件持久化的实际编码(bus.rs::serialize_msgpack)
        let bytes = rmp_serde::to_vec_named(&legacy).expect("旧负载编码");
        let meta: EventMetadata =
            rmp_serde::from_slice(&bytes).expect("旧 MessagePack 负载必须可解析(新增字段兼容)");
        assert!(meta.trace_id.is_none(), "缺失 trace_id 键 → None");
        assert_eq!(meta.correlation_id.as_deref(), Some("quest-legacy"));
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(32))]

        /// 属性:任意 trace_id 取值的元数据经 JSON roundtrip 逐字段保真
        #[test]
        fn prop_json_roundtrip_preserves_trace_id(trace in proptest::option::of("[a-z0-9-]{1,40}")) {
            let meta = EventMetadata::new("test").with_trace_opt(trace.clone());
            let json = serde_json::to_string(&meta).expect("序列化");
            let back: EventMetadata = serde_json::from_str(&json).expect("反序列化");
            proptest::prop_assert_eq!(&back, &meta);
            proptest::prop_assert_eq!(back.trace_id, trace);
        }

        /// 属性:任意 trace_id 取值经 MessagePack(named)roundtrip 保真
        #[test]
        fn prop_msgpack_roundtrip_preserves_trace_id(trace in proptest::option::of("[a-z0-9-]{1,40}")) {
            let meta = EventMetadata::new("test").with_trace_opt(trace.clone());
            let bytes = rmp_serde::to_vec_named(&meta).expect("序列化");
            let back: EventMetadata = rmp_serde::from_slice(&bytes).expect("反序列化");
            proptest::prop_assert_eq!(&back, &meta);
            proptest::prop_assert_eq!(back.trace_id, trace);
        }
    }

    /// span 继承传播:child_of 延续追踪链,但获得新身份
    #[test]
    fn child_of_inherits_trace_and_correlation_with_new_identity() {
        let mut parent = EventMetadata::with_correlation("parent-crate", "quest-42");
        parent.ensure_trace_id();
        let child = EventMetadata::child_of(&parent, "child-crate");
        assert_eq!(child.trace_id, parent.trace_id, "trace_id 必须继承");
        assert_eq!(child.correlation_id.as_deref(), Some("quest-42"));
        assert_eq!(child.source, "child-crate", "source 标识派生点");
        assert_ne!(child.event_id, parent.event_id, "子事件有新身份");
    }

    /// 根事件懒生成:ensure_trace_id 幂等且锚定 event_id
    #[test]
    fn ensure_trace_id_is_idempotent_and_anchors_event_id() {
        let mut meta = EventMetadata::new("root");
        let anchored = meta.ensure_trace_id().to_string();
        assert_eq!(anchored, meta.event_id.to_string(), "首调以 event_id 锚定");
        let again = meta.ensure_trace_id().to_string();
        assert_eq!(again, anchored, "二次调用幂等(不重新生成)");
    }

    /// with_trace 覆盖既有值(显式挂载上游 ID)
    #[test]
    fn with_trace_overrides() {
        let meta = EventMetadata::new("test")
            .with_trace("upstream-trace")
            .with_trace("override-trace");
        assert_eq!(meta.trace_id(), Some("override-trace"));
    }
}
