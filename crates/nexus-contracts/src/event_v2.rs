//! 事件双轨契约 — DynamicEvent 注册表与 EventMetadataV2（WI-21）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts，纯类型 + 同步 trait）
//! 对应工作项: **WI-21 事件双轨（NexusEventV2 批判性吸收）**（v4.0 统一执行总案 §6.5）
//! 对应设计源: 外部修订版 NexusEventV2 trait 化提案（批判性吸收——拒绝全 trait 化，
//!             保留编译期穷举匹配；采纳双轨制）
//!
//! # 双轨制设计（T3 裁决）
//!
//! - **轨一（不动）**: `event-bus` 内置 144 枚举变体保持编译期穷尽匹配与优化
//! - **轨二（新增）**: [`DynamicEvent`] 注册表供 MCP/SubAgent/Hook 等外部源注册，
//!   命名空间化（`mcp.github.issue_created`），注册表配命名空间配额
//!   （≤64 类型/空间）+ 注册审计
//! - **元数据统一**: [`EventMetadataV2`] 双轨统一承载（WI-04 图身份 /
//!   WI-12 可压缩性 / WI-20 残留权重 / WI-15 订阅者模式）
//!
//! # 设计约束（ADR-033 + WI-21）
//!
//! - **纯类型 + 同步 trait**: L0 零依赖铁律，不引入 async-trait
//!   （`rl_hooks::RLHook` 同步 trait 先例）
//! - **144 枚举与 severity() 权威源不动**: 本模块仅承载注册表契约面，
//!   不新增内置变体（v4.0 §17 治理红线）
//! - **序列化形态**: `serialize()` 返回 `Bytes`——但 L0 不引入 bytes 依赖，
//!   使用 `Vec<u8>`（零依赖铁律优先于类型美观）

use serde::{Deserialize, Serialize};

use crate::graph_identity::GraphIdentity;

// ============================================================
// 命名空间与类型 ID
// ============================================================

/// 事件命名空间 — 双轨事件来源域（WI-21 §6.5）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventNamespace {
    /// 内置（轨一：144 枚举，编译期穷举）
    Builtin,
    /// MCP 外部源（命名空间前缀 "mcp."）
    Mcp,
    /// SubAgent 外部源（"subagent."）
    SubAgent,
    /// Hook 外部源（"hook."）
    Hook,
    /// 其他外部源（"external."）
    External,
}

impl EventNamespace {
    /// 命名空间字符串前缀（注册表键命名约定）
    pub fn prefix(&self) -> &'static str {
        match self {
            Self::Builtin => "builtin.",
            Self::Mcp => "mcp.",
            Self::SubAgent => "subagent.",
            Self::Hook => "hook.",
            Self::External => "external.",
        }
    }
}

/// 事件类型 ID — 命名空间化类型标识（如 "mcp.github.issue_created"）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventTypeId(pub Box<str>);

impl EventTypeId {
    /// 创建事件类型 ID
    pub fn new(id: impl Into<String>) -> Self {
        Self(Box::from(id.into()))
    }

    /// 底层字符串引用
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ============================================================
// 元数据扩展字段类型
// ============================================================

/// 重要性评分 — 残留/压缩/路由决策的输入（WI-20/WI-12/WI-15）
///
/// `f32` 取值范围 [0.0, 1.0]（低 → 高）。仅 `PartialEq`（f32 浮点字段
/// 禁止 derive `Eq`/`Hash`——token_evidence.rs 先例）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ImportanceScore(pub f32);

impl ImportanceScore {
    /// 创建重要性评分
    ///
    /// # Panics
    ///
    /// 评分超出 [0.0, 1.0] 时 panic —— 不变量：评分归一化到单位区间。
    pub fn new(score: f32) -> Self {
        assert!(
            (0.0..=1.0).contains(&score),
            "ImportanceScore 不变量: 评分必须在 [0.0, 1.0] 内, 实际 {score}"
        );
        Self(score)
    }

    /// 底层数值
    pub fn value(&self) -> f32 {
        self.0
    }
}

/// 可压缩性评级 — WI-12 CSC 四级压缩链的输入
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compressibility {
    /// 不可压缩（关键符号/决策锚点）
    Incompressible,
    /// 低可压缩性（结构性内容）
    Low,
    /// 中可压缩性（可签名化）
    Medium,
    /// 高可压缩性（冗余/可摘要）
    High,
}

/// 事件订阅模式 — WI-15 SER 精确路由的声明式订阅契约
///
/// # 阶段约束（WI-15 批判性收窄）
/// - **阶段一**: 仅 `Exact` / `Namespace` / `And`（精确匹配，语义与广播等价）
/// - **阶段二门禁**: `Semantic` 仅当订阅者 > 500 且精确索引 P99 > 1ms 时启用
///   （近似路由漏发风险不可接受——漏发率 = 0 硬门禁）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EventPattern {
    /// 精确类型匹配（如 "mcp.github.issue_created"）
    Exact(EventTypeId),
    /// 命名空间前缀匹配（如 "mcp.github.*"）
    Namespace(Box<str>),
    /// 语义相似度匹配（阶段二门禁；嵌入向量 + 阈值）
    Semantic {
        /// 语义嵌入（CLV 512 维 f32 向量）
        embedding: Vec<f32>,
        /// 相似度阈值 [0.0, 1.0]
        threshold: f32,
    },
    /// 复合条件（全部满足）
    And(Box<[EventPattern]>),
}

// ============================================================
// 事件元数据 v2 — 双轨统一元数据
// ============================================================

/// 事件元数据 v2 — 双轨统一承载（WI-21 §6.5）
///
/// # 与 v1 的关系
/// `base` 为 L0 [`crate::event_metadata::EventMetadata`]（v1 基座，
/// event_id/timestamp/source/correlation_id/payload_version），
/// v2 扩展字段叠加上层——**144 内置枚举的 metadata() 权威源不动**，
/// 本类型仅供动态事件（轨二）与内置事件元数据扩展字段使用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventMetadataV2 {
    /// v1 基座元数据（event_id/timestamp/source/correlation_id/payload_version）
    pub base: crate::event_metadata::EventMetadata,
    /// 图身份三元组（WI-04：goal/run/node 成本归因）
    ///
    /// WHY Option 不 skip_serializing_if: rmp-serde array 位置编码下跳过字段
    /// 破坏反序列化长度（ADR-004 同源）；缺失字段反序列化自动 None。
    pub graph_identity: Option<GraphIdentity>,
    /// 残留权重（WI-20：跨轮事件残留注入强度）
    pub residual_weight: f64,
    /// 残留衰减率（WI-20）
    pub residual_decay: f64,
    /// 可压缩性评级（WI-12）
    pub compressibility: Compressibility,
    /// 不可压缩关键符号（WI-12 IndexShare 输入）
    pub key_symbols: Vec<Box<str>>,
    /// 订阅者模式（WI-15：索引数据源）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscriber_pattern: Option<EventPattern>,
}

impl EventMetadataV2 {
    /// 创建事件元数据 v2（默认：无图身份、无残留、中等可压缩性、无订阅模式）
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            base: crate::event_metadata::EventMetadata::new(source),
            graph_identity: None,
            residual_weight: 0.0,
            residual_decay: 0.9,
            compressibility: Compressibility::Medium,
            key_symbols: Vec::new(),
            subscriber_pattern: None,
        }
    }

    /// 挂载图身份（WI-04 渐进铺开）
    pub fn with_graph_identity(mut self, gi: GraphIdentity) -> Self {
        self.graph_identity = Some(gi);
        self
    }

    /// 设置残留权重（WI-20）
    pub fn with_residual(mut self, weight: f64, decay: f64) -> Self {
        self.residual_weight = weight;
        self.residual_decay = decay;
        self
    }

    /// 设置可压缩性（WI-12）
    pub fn with_compressibility(mut self, level: Compressibility) -> Self {
        self.compressibility = level;
        self
    }

    /// 声明订阅模式（WI-15）
    pub fn with_pattern(mut self, pattern: EventPattern) -> Self {
        self.subscriber_pattern = Some(pattern);
        self
    }
}

// ============================================================
// 双轨注册契约
// ============================================================

/// 动态事件 — 轨二注册契约（WI-21 §6.5）
///
/// 供 MCP/SubAgent/Hook 等外部源实现。**同步 trait**（L0 零依赖铁律，
/// `rl_hooks::RLHook` 先例）；注册后由 event-bus 侧的动态注册表
/// （L1 实现，本模块不承载注册表存储）按命名空间配额管理。
///
/// # 路由语义（L1 实现侧）
/// - 默认走普通 broadcast 道
/// - `importance` ≥ Critical 阈值时强制升格广播（Critical mpsc 红线隔离）
pub trait DynamicEvent: Send + Sync + 'static {
    /// 事件类型 ID（命名空间化："mcp.github.issue_created"）
    fn event_type(&self) -> EventTypeId;

    /// 命名空间（Builtin / Mcp / SubAgent / Hook / External）
    fn namespace(&self) -> EventNamespace;

    /// 序列化为字节载荷（协议自定，如 MessagePack/JSON）
    fn serialize(&self) -> Result<Vec<u8>, String>;

    /// 双轨统一元数据
    fn metadata(&self) -> &EventMetadataV2;

    /// 重要性评分 [0.0, 1.0]（残留/压缩/路由决策输入）
    fn importance(&self) -> ImportanceScore;

    /// 提取关键符号（WI-12 IndexShare 输入）
    fn extract_symbols(&self) -> Vec<Box<str>>;
}

/// 命名空间配额 — 动态事件注册上限（WI-21 防注册表膨胀）
///
/// 每命名空间 ≤ 64 类型（v4.0 §6.5 规格）；注册超限拒绝并审计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamespaceQuotaV2 {
    /// 每命名空间最大注册类型数
    pub max_types_per_namespace: usize,
    /// 全注册表最大类型数
    pub max_total_types: usize,
}

impl Default for NamespaceQuotaV2 {
    fn default() -> Self {
        Self {
            max_types_per_namespace: 64,
            max_total_types: 512,
        }
    }
}

/// 双轨元数据桥接 — 内置枚举元数据提取契约（WI-04 渐进铺开）
///
/// 内置 144 枚举变体经此 trait 暴露 v2 扩展字段（无身份时返回 None），
/// 使消费方（账本聚合/成本归因）不感知枚举内部差异。
pub trait MetadataBridge {
    /// 提取图身份（无身份时 None——成本归因覆盖率 = 100% 目标）
    fn graph_identity(&self) -> Option<&GraphIdentity>;
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_id_wire_format() {
        let id = EventTypeId::new("mcp.github.issue_created");
        assert_eq!(id.as_str(), "mcp.github.issue_created");
    }

    #[test]
    fn namespace_prefixes() {
        assert_eq!(EventNamespace::Builtin.prefix(), "builtin.");
        assert_eq!(EventNamespace::Mcp.prefix(), "mcp.");
        assert_eq!(EventNamespace::SubAgent.prefix(), "subagent.");
        assert_eq!(EventNamespace::Hook.prefix(), "hook.");
        assert_eq!(EventNamespace::External.prefix(), "external.");
    }

    #[test]
    fn metadata_v2_json_roundtrip() {
        let meta = EventMetadataV2::new("test-crate")
            .with_graph_identity(GraphIdentity::new("goal-1", "run-1", "node-1"))
            .with_residual(0.7, 0.9)
            .with_compressibility(Compressibility::High)
            .with_pattern(EventPattern::Namespace(Box::from("mcp.github.")));
        let json = serde_json::to_string(&meta).expect("JSON 序列化失败");
        let decoded: EventMetadataV2 = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, meta);
        assert!(json.contains("\"goal_id\":\"goal-1\""));
    }

    #[test]
    fn metadata_v2_default_no_identity() {
        // 无身份时 graph_identity 序列化为 null（Option 总是序列化——
        // rmp-serde array 位置编码兼容），反序列化回 None
        let meta = EventMetadataV2::new("test-crate");
        let json = serde_json::to_string(&meta).expect("JSON 序列化失败");
        assert!(json.contains("\"graph_identity\":null"));
        let decoded: EventMetadataV2 = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert!(decoded.graph_identity.is_none());
    }

    #[test]
    fn importance_score_bounds_asserted() {
        assert!(std::panic::catch_unwind(|| ImportanceScore::new(1.5)).is_err());
        assert!(std::panic::catch_unwind(|| ImportanceScore::new(-0.1)).is_err());
        let score = ImportanceScore::new(0.7);
        assert!((score.value() - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn event_pattern_exact_and_namespace() {
        // 阶段一: 精确 + 前缀匹配（WI-15 精确路由语义）
        let exact = EventPattern::Exact(EventTypeId::new("mcp.github.issue_created"));
        let ns = EventPattern::Namespace(Box::from("mcp.github."));
        let json = serde_json::to_string(&exact).expect("JSON 序列化失败");
        let decoded: EventPattern = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, exact);
        assert!(matches!(ns, EventPattern::Namespace(_)));
    }

    #[test]
    fn event_pattern_semantic_gated() {
        // 阶段二门禁语义: Semantic 模式仅在数据达标后启用（类型存在即可）
        let semantic = EventPattern::Semantic {
            embedding: vec![0.1, 0.2, 0.3],
            threshold: 0.8,
        };
        let json = serde_json::to_string(&semantic).expect("JSON 序列化失败");
        let decoded: EventPattern = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, semantic);
    }

    #[test]
    fn namespace_quota_defaults() {
        let quota = NamespaceQuotaV2::default();
        assert_eq!(quota.max_types_per_namespace, 64);
        assert_eq!(quota.max_total_types, 512);
    }

    #[test]
    fn dynamic_event_trait_object_send_sync() {
        // 编译期断言: DynamicEvent 可装箱为 Send + Sync trait object
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<Box<dyn DynamicEvent>>();
    }
}
