//! CMT 核心领域类型 — 能力内存四级分层的统一数据模型
//!
//! 对应架构层:L3 Storage
//! 对应创新点:CMT(Capability Memory Tiering,能力内存四级分层)
//!
//! # 类型职责
//! - `CapabilityId`:能力条目唯一标识(String 别名)
//! - `Tier`:四级分层标识(Hot / Warm / Cold / Ice)
//! - `MigrationReason`:层级迁移原因(LRU 驱逐 / 空闲超时 / 衰减到期 / 访问提升 / 手动操作 等 10 种)
//! - `CapabilityEntry`:统一的能力条目载体,跨四级复用
//!
//! # 设计决策(WHY)
//! - **统一 CapabilityEntry**:四级复用同一载体,通过 `tier` 字段区分所在层级,
//!   避免各层自定义不同结构导致的转换开销与一致性维护成本(参考 mlc-engine)
//! - **CapabilityId 为 String 别名**:便于与 EventBus 事件中的 ID 字段直接交互,
//!   避免额外转换开销(与 MemoryId 保持一致的设计哲学)
//! - **MigrationReason 为 enum**:迁移原因需在事件 payload 中携带,枚举比字符串
//!   更类型安全,且便于消费者按原因做不同处理(如 LRU 驱逐 vs 衰减降级)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Deref;

/// 能力条目唯一标识 newtype — 四级存储的统一标识
///
/// WHY:newtype 模式使编译器能拦截 `CapabilityId` 误传为其他 ID 类型,
/// 通过 `Deref<Target=str>` 保持与 `&str` 接口兼容(零运行时开销),
/// `#[serde(transparent)]` 确保与原 `String` 别名序列化向后兼容。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityId(pub String);

impl CapabilityId {
    /// 从任意可转换为 String 的值构造 ID
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// 返回内部字符串引用(零拷贝)
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for CapabilityId {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for CapabilityId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for CapabilityId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<String> for CapabilityId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for CapabilityId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 能力分层 — 四级存储层级标识
///
/// 对应 CMT 创新点的四级架构:
/// - `Hot`:热层,内存 DashMap + LRU,容量 256,访问延迟 < 1μs
/// - `Warm`:温层,SQLite WAL 模式,容量 4096,访问延迟 < 5ms
/// - `Cold`:冷层,SQLite 附加数据库,容量 65536,访问延迟 < 50ms
/// - `Ice`:冰层,归档只读文件,无容量上限,访问延迟 < 500ms
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Tier {
    /// 热层:内存 DashMap + LRU,容量 256,延迟 < 1μs
    Hot,
    /// 温层:SQLite WAL 模式,容量 4096,延迟 < 5ms
    Warm,
    /// 冷层:SQLite 附加数据库,容量 65536,延迟 < 50ms
    Cold,
    /// 冰层:归档只读文件,无容量上限,延迟 < 500ms
    Ice,
}

impl Tier {
    /// 返回层级名称(用于事件 payload 与日志)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hot => "Hot",
            Self::Warm => "Warm",
            Self::Cold => "Cold",
            Self::Ice => "Ice",
        }
    }

    /// 从字符串解析层级(用于事件消费时反序列化)
    ///
    /// 返回 None 表示输入不是合法的层级名称
    ///
    /// WHY 命名为 parse_tier 而非 from_str:避免与标准库 `std::str::FromStr::from_str`
    /// 方法名冲突(clippy::should_implement_trait)。返回 Option 而非 Result,
    /// 因为解析失败是正常的(用户输入可能非法),不需要携带详细错误信息。
    pub fn parse_tier(s: &str) -> Option<Self> {
        match s {
            "Hot" => Some(Self::Hot),
            "Warm" => Some(Self::Warm),
            "Cold" => Some(Self::Cold),
            "Ice" => Some(Self::Ice),
            _ => None,
        }
    }
}

/// 迁移原因 — 能力条目在层级间迁移的触发原因
///
/// 用于 `CapabilityTiered` 事件的 `reason` 字段,消费者据此做不同处理。
///
/// # 变体分类
/// - **自动驱逐类**:`LruEviction`(LRU 算法)、`CapacityEviction`(容量满,非 LRU)
/// - **自动降级类**:`IdleTimeout`(空闲超时)、`DecayExpired`(衰减到期)、`DecayDemotion`(衰减降级,通用)
/// - **自动提升类**:`AccessPromotion`(访问触发)、`AccessPatternChange`(访问模式变化)
/// - **手动操作类**:`ManualPromote`(手动提升)、`ManualDemote`(手动降级)
/// - **系统类**:`SystemStartup`(系统启动初始分级)
///
/// # WHY 保留 `DecayDemotion` 与 `DecayExpired` 两个变体
/// - `DecayDemotion`:通用衰减降级术语,保留向后兼容(历史事件 payload 可能含 "decay_demotion")
/// - `DecayExpired`:更具体的"衰减到期"语义,priority < 0.1 触发的自动降级用此变体,
///   日志可读性更好(明确区分"到期自动降级"与"手动衰减降级")
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum MigrationReason {
    /// LRU 驱逐:Hot 层容量满,按 LRU 算法驱逐最久未访问的条目到 Warm
    LruEviction,
    /// 空闲超时:条目长时间未被访问,降级到下层
    IdleTimeout,
    /// 衰减降级(通用):基于衰减优先级的降级迁移,保留向后兼容
    DecayDemotion,
    /// 访问提升:条目被访问,提升到更上层
    AccessPromotion,
    /// 衰减到期:能力衰减优先级低于阈值(0.1)导致的自动降级
    ///
    /// WHY:与 `DecayDemotion` 区分 — 此变体专指 `run_decay_cycle` 自动触发的到期降级,
    /// 日志与事件消费者可据此区分"自动到期"与"手动衰减降级"
    DecayExpired,
    /// 容量驱逐:层级容量满导致的驱逐(非 LRU 算法,如 Warm/Cold 层容量满)
    ///
    /// WHY:与 `LruEviction` 区分 — `LruEviction` 专指 Hot 层 LRU 算法驱逐,
    /// `CapacityEviction` 用于其他层级的容量满驱逐(如 Warm 层容量满驱逐到 Cold)
    CapacityEviction,
    /// 手动提升:用户或外部系统显式请求提升到更高层级
    ManualPromote,
    /// 手动降级:用户或外部系统显式请求降级到更低层级
    ManualDemote,
    /// 访问模式变化:访问频率/模式变化导致的迁移(如热点转移)
    AccessPatternChange,
    /// 系统启动:系统启动时的初始分级(从持久化加载到 Hot/Warm)
    SystemStartup,
}

impl MigrationReason {
    /// 返回原因的字符串表示(用于事件 payload 与日志)
    ///
    /// 字符串采用 snake_case 格式,作为 `CapabilityTiered` 事件的 `reason` 字段。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LruEviction => "lru_eviction",
            Self::IdleTimeout => "idle_timeout",
            Self::DecayDemotion => "decay_demotion",
            Self::AccessPromotion => "access_promotion",
            Self::DecayExpired => "decay_expired",
            Self::CapacityEviction => "capacity_eviction",
            Self::ManualPromote => "manual_promote",
            Self::ManualDemote => "manual_demote",
            Self::AccessPatternChange => "access_pattern_change",
            Self::SystemStartup => "system_startup",
        }
    }
}

/// 能力条目 — 四级存储的统一载体
///
/// 跨 Hot/Warm/Cold/Ice 复用同一结构,通过 `tier` 字段区分所在层级。
/// 各层对字段的填充要求一致(与 mlc-engine 的 MemoryEntry 设计保持一致)。
///
/// # 字段语义
/// - `id`:能力唯一标识(调用方生成 UUIDv7)
/// - `content`:能力内容(自然语言文本或序列化 JSON)
/// - `tier`:当前所在层级(跨层迁移时更新)
/// - `created_at`:条目首次写入时间(不变)
/// - `last_accessed_at`:最后访问时间(每次 get 更新,LRU 与衰减依据)
/// - `access_count`:访问次数(衰减公式 `priority = access_count × exp(-Δt / τ)` 的输入)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilityEntry {
    /// 能力条目唯一标识(UUIDv7,由调用方生成)
    pub id: CapabilityId,
    /// 能力内容(自然语言文本或序列化 JSON)
    pub content: String,
    /// 当前所在层级(用于跨层迁移时校验)
    pub tier: Tier,
    /// 创建时间(UTC,条目首次写入时设置,不变)
    pub created_at: DateTime<Utc>,
    /// 最后访问时间(UTC,LRU 驱逐与衰减计算依据,每次 get 更新)
    pub last_accessed_at: DateTime<Utc>,
    /// 访问次数(用于热度统计与衰减公式)
    pub access_count: u64,
}

impl CapabilityEntry {
    /// 创建新能力条目,`created_at` 与 `last_accessed_at` 自动设为当前 UTC
    ///
    /// # 参数
    /// - `id`:条目唯一标识(接受 `CapabilityId`/`String`/`&str`,通过 `Into<CapabilityId>` 转换)
    /// - `content`:能力内容
    /// - `tier`:初始层级
    pub fn new(id: impl Into<CapabilityId>, content: impl Into<String>, tier: Tier) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            content: content.into(),
            tier,
            created_at: now,
            last_accessed_at: now,
            access_count: 0,
        }
    }

    /// 标记被访问:更新 `last_accessed_at` 与 `access_count`
    ///
    /// WHY:每次 get 调用此方法,实现 LRU 语义(最近访问的不易被驱逐)
    /// 与衰减公式输入(access_count 越高,衰减后优先级越高)
    pub fn touch(&mut self) {
        self.last_accessed_at = Utc::now();
        self.access_count = self.access_count.saturating_add(1);
    }

    /// 更新层级(迁移时调用)
    pub fn with_tier(mut self, tier: Tier) -> Self {
        self.tier = tier;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_as_str() {
        assert_eq!(Tier::Hot.as_str(), "Hot");
        assert_eq!(Tier::Warm.as_str(), "Warm");
        assert_eq!(Tier::Cold.as_str(), "Cold");
        assert_eq!(Tier::Ice.as_str(), "Ice");
    }

    #[test]
    fn test_tier_parse_tier() {
        assert_eq!(Tier::parse_tier("Hot"), Some(Tier::Hot));
        assert_eq!(Tier::parse_tier("Warm"), Some(Tier::Warm));
        assert_eq!(Tier::parse_tier("Cold"), Some(Tier::Cold));
        assert_eq!(Tier::parse_tier("Ice"), Some(Tier::Ice));
        assert_eq!(Tier::parse_tier("Invalid"), None);
    }

    #[test]
    fn test_migration_reason_as_str() {
        // 原有变体
        assert_eq!(MigrationReason::LruEviction.as_str(), "lru_eviction");
        assert_eq!(MigrationReason::IdleTimeout.as_str(), "idle_timeout");
        assert_eq!(MigrationReason::DecayDemotion.as_str(), "decay_demotion");
        assert_eq!(
            MigrationReason::AccessPromotion.as_str(),
            "access_promotion"
        );
        // SubTask 14.5 新增变体
        assert_eq!(MigrationReason::DecayExpired.as_str(), "decay_expired");
        assert_eq!(
            MigrationReason::CapacityEviction.as_str(),
            "capacity_eviction"
        );
        assert_eq!(MigrationReason::ManualPromote.as_str(), "manual_promote");
        assert_eq!(MigrationReason::ManualDemote.as_str(), "manual_demote");
        assert_eq!(
            MigrationReason::AccessPatternChange.as_str(),
            "access_pattern_change"
        );
        assert_eq!(MigrationReason::SystemStartup.as_str(), "system_startup");
    }

    #[test]
    fn test_migration_reason_serialization_roundtrip() {
        // 验证所有变体的序列化/反序列化往返一致
        let reasons = [
            MigrationReason::LruEviction,
            MigrationReason::IdleTimeout,
            MigrationReason::DecayDemotion,
            MigrationReason::AccessPromotion,
            MigrationReason::DecayExpired,
            MigrationReason::CapacityEviction,
            MigrationReason::ManualPromote,
            MigrationReason::ManualDemote,
            MigrationReason::AccessPatternChange,
            MigrationReason::SystemStartup,
        ];
        for reason in reasons {
            let json = serde_json::to_string(&reason).unwrap();
            let decoded: MigrationReason = serde_json::from_str(&json).unwrap();
            assert_eq!(reason, decoded, "序列化往返失败: {reason:?}");
        }
    }

    #[test]
    fn test_capability_entry_new_defaults() {
        let entry = CapabilityEntry::new("cap-1", "内容", Tier::Hot);
        assert_eq!(entry.id.as_str(), "cap-1");
        assert_eq!(entry.content, "内容");
        assert_eq!(entry.tier, Tier::Hot);
        assert_eq!(entry.access_count, 0);
    }

    #[test]
    fn test_capability_entry_touch_increments_access_count() {
        let mut entry = CapabilityEntry::new("cap-1", "内容", Tier::Hot);
        assert_eq!(entry.access_count, 0);
        entry.touch();
        entry.touch();
        assert_eq!(entry.access_count, 2);
    }

    #[test]
    fn test_capability_entry_with_tier() {
        let entry = CapabilityEntry::new("cap-1", "内容", Tier::Hot);
        let migrated = entry.with_tier(Tier::Warm);
        assert_eq!(migrated.tier, Tier::Warm);
    }

    #[test]
    fn test_capability_entry_serialization_roundtrip() {
        let entry = CapabilityEntry::new("cap-1", "内容", Tier::Hot);
        let json = serde_json::to_string(&entry).unwrap();
        let decoded: CapabilityEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, decoded);
    }
}
