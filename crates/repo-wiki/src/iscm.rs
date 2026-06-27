//! ISCM(Inter-Shared Cross Module)跨层共享锚点
//!
//! 对应架构层:L5 Knowledge(但被 L1-L10 全层引用)
//! 对应创新点:ISCM 跨层共享索引
//!
//! # 核心职责
//! - 标识同一知识实体在不同架构层间的引用关系
//! - 使用 UUIDv7(时间有序)作为锚点 ID,便于跨进程因果追踪
//! - 通过 `is_dangling` 标记实现"逻辑悬空"(物理条目删除后锚点保留审计轨迹)
//!
//! # 设计动机(WHY)
//! 在十层架构中,L2 Memory、L5 Knowledge、L9 Quest 等多层可能引用同一知识实体。
//! 若各层独立维护引用,会出现:
//! - 数据不一致:A 层认为实体存在,B 层已删除
//! - 重复存储:同一实体在多层冗余
//! - 因果断裂:无法追踪实体的跨层流转
//!
//! ISCM 通过统一锚点 ID 解决上述问题:任何层引用实体时,先创建/查询锚点,
//! 通过 `resolve_anchor` 获取最新实体状态,删除实体时联动标记锚点为悬空。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 架构层枚举 — L1-L10 全覆盖(对应 §2.1 分层映射)
///
/// 每个层对应一个或多个 crate,锚点通过 `Layer` 标识来源层,
/// 便于跨层一致性审计与悬空检测。
///
/// WHY:变体命名保留 `L1_Core` 下划线风格(而非 `L1Core`),
/// 与架构手册 §2.1 分层映射表一致,便于跨文档检索。
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Layer {
    /// L1 Core — nexus-core · event-bus · model-router
    L1_Core,
    /// L2 Memory — nmc-encoder · hcw-window · mlc-engine
    L2_Memory,
    /// L3 Storage — scc-cache · lsct-tiering · cmt-tiering
    L3_Storage,
    /// L4 Security — seccore · qeep-protocol · decay-engine
    L4_Security,
    /// L5 Knowledge — repo-wiki · gsoe-evolution · auto-dpo
    L5_Knowledge,
    /// L6 Router — osa-coordinator · kvbsr-router · faae-router · sesa-router
    L6_Router,
    /// L7 Execution — pvl-layer · gqep-executor · mtpe-executor · ssra-fusion
    L7_Execution,
    /// L8 Parliament — parliament · acb-governor · decb-governor
    L8_Parliament,
    /// L9 Quest — quest-engine · gea-activator · efficiency-monitor
    L9_Quest,
    /// L10 Interface — chimera-cli · chimera-tui · chtc-bridge · mcp-mesh · csn-substitutor
    L10_Interface,
}

impl Layer {
    /// 转换为字符串(用于 SQLite 存储与日志输出)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::L1_Core => "L1_Core",
            Self::L2_Memory => "L2_Memory",
            Self::L3_Storage => "L3_Storage",
            Self::L4_Security => "L4_Security",
            Self::L5_Knowledge => "L5_Knowledge",
            Self::L6_Router => "L6_Router",
            Self::L7_Execution => "L7_Execution",
            Self::L8_Parliament => "L8_Parliament",
            Self::L9_Quest => "L9_Quest",
            Self::L10_Interface => "L10_Interface",
        }
    }

    /// 从字符串解析(用于 SQLite 反序列化)
    ///
    /// 返回 `None` 表示字符串不是有效的层标识(不阻断查询,由调用方决定降级策略)。
    ///
    /// WHY:未实现 `std::str::FromStr` trait — 该 trait 要求返回 `Result<Self, Err>`,
    /// 而本方法返回 `Option<Self>` 更适合"非错误即缺失"的语义(无效层标识不属于
    /// 需要传播的错误,降级为 `None` 即可)。clippy 警告已显式抑制。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "L1_Core" => Some(Self::L1_Core),
            "L2_Memory" => Some(Self::L2_Memory),
            "L3_Storage" => Some(Self::L3_Storage),
            "L4_Security" => Some(Self::L4_Security),
            "L5_Knowledge" => Some(Self::L5_Knowledge),
            "L6_Router" => Some(Self::L6_Router),
            "L7_Execution" => Some(Self::L7_Execution),
            "L8_Parliament" => Some(Self::L8_Parliament),
            "L9_Quest" => Some(Self::L9_Quest),
            "L10_Interface" => Some(Self::L10_Interface),
            _ => None,
        }
    }
}

/// 跨层共享锚点 — 标识同一知识实体在不同层间的引用关系
///
/// WHY:ISCM(Inter-Shared Cross Module)确保 L2 Memory、L5 Knowledge、L9 Quest
/// 引用同一知识实体时使用同一锚点 ID,避免数据不一致。
/// 锚点使用 UUIDv7(时间有序),便于跨进程因果追踪。
///
/// # 字段说明
/// - `anchor_id`:全局唯一标识(UUIDv7,时间有序)
/// - `layer`:来源架构层(L1-L10)
/// - `crate_name`:来源 crate 名(便于审计定位)
/// - `entity_id`:指向的 Wiki 条目 ID(跨层共享的实体)
/// - `is_dangling`:是否悬空(实体被删除后标记为 true,保留审计轨迹)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IscmAnchor {
    /// 锚点唯一标识(UUIDv7,时间有序,便于跨进程因果追踪)
    pub anchor_id: Uuid,
    /// 来源架构层
    pub layer: Layer,
    /// 来源 crate 名(如 "quest-engine"、"repo-wiki")
    pub crate_name: String,
    /// 指向的 Wiki 条目 ID
    pub entity_id: String,
    /// 创建时间(UTC)
    pub created_at: DateTime<Utc>,
    /// 最后更新时间(UTC,悬空标记时刷新)
    pub updated_at: DateTime<Utc>,
    /// 是否悬空(实体被删除后标记为 true)
    pub is_dangling: bool,
}

impl IscmAnchor {
    /// 创建新锚点(anchor_id 自动生成 UUIDv7)
    ///
    /// WHY:UUIDv7 而非 UUIDv4 — v7 时间有序,SQLite 主键索引更友好(B-tree 局部性),
    /// 且跨进程事件排序时可从 ID 推断因果顺序,无需额外时间戳字段。
    pub fn new(layer: Layer, crate_name: impl Into<String>, entity_id: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            anchor_id: Uuid::now_v7(),
            layer,
            crate_name: crate_name.into(),
            entity_id: entity_id.into(),
            created_at: now,
            updated_at: now,
            is_dangling: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layer_as_str_roundtrip() {
        let layers = [
            Layer::L1_Core,
            Layer::L2_Memory,
            Layer::L3_Storage,
            Layer::L4_Security,
            Layer::L5_Knowledge,
            Layer::L6_Router,
            Layer::L7_Execution,
            Layer::L8_Parliament,
            Layer::L9_Quest,
            Layer::L10_Interface,
        ];
        for layer in layers {
            let s = layer.as_str();
            assert_eq!(Layer::from_str(s), Some(layer), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn test_layer_from_str_invalid() {
        assert_eq!(Layer::from_str("L0_Unknown"), None);
        assert_eq!(Layer::from_str(""), None);
        assert_eq!(Layer::from_str("invalid"), None);
    }

    #[test]
    fn test_anchor_new_generates_uuidv7() {
        let anchor = IscmAnchor::new(Layer::L5_Knowledge, "repo-wiki", "e-1");
        assert_eq!(anchor.layer, Layer::L5_Knowledge);
        assert_eq!(anchor.crate_name, "repo-wiki");
        assert_eq!(anchor.entity_id, "e-1");
        assert!(!anchor.is_dangling);
        assert_eq!(anchor.created_at, anchor.updated_at);
        // UUIDv7 版本号应为 SortRand(uuid crate 中 UUIDv7 对应的版本枚举)
        assert_eq!(
            anchor.anchor_id.get_version(),
            Some(uuid::Version::SortRand)
        );
    }

    #[test]
    fn test_anchor_uuidv7_time_ordered() {
        // 连续创建多个锚点,验证 UUIDv7 时间有序
        let anchor1 = IscmAnchor::new(Layer::L1_Core, "nexus-core", "e-1");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let anchor2 = IscmAnchor::new(Layer::L1_Core, "nexus-core", "e-2");

        // UUIDv7 时间有序:anchor1 < anchor2
        assert!(
            anchor1.anchor_id < anchor2.anchor_id,
            "UUIDv7 should be time-ordered"
        );
    }

    #[test]
    fn test_anchor_serde_roundtrip() {
        let anchor = IscmAnchor::new(Layer::L9_Quest, "quest-engine", "e-1");
        let json = serde_json::to_string(&anchor).unwrap();
        let restored: IscmAnchor = serde_json::from_str(&json).unwrap();
        assert_eq!(anchor, restored);
    }

    #[test]
    fn test_anchor_uuid_uniqueness() {
        let mut ids = std::collections::HashSet::new();
        for i in 0..1000 {
            let anchor = IscmAnchor::new(Layer::L5_Knowledge, "repo-wiki", format!("e-{i}"));
            assert!(ids.insert(anchor.anchor_id), "UUID 冲突 at iteration {i}");
        }
    }
}
