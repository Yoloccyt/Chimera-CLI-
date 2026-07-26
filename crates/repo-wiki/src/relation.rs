//! 条目间关系 — 矛盾检测的关系图谱(P3-W11.2.1)
//!
//! 对应架构层:L5 Knowledge(repo-wiki 层内,不跨层共享)
//! 对应设计:spec.md:298 "写入路径检测 Contradicts 关系 → 标记过渡期不删旧记录"
//!
//! # 设计决策(WHY)
//!
//! - **关系定义在 L5 层内**:`EntryRelation` 当前仅被 repo-wiki 写入路径消费,
//!   不跨层共享。若后续 chimera-mas 需读取矛盾关系,再上提至 L0 nexus-contracts
//!   (避免过早抽象,§全局指令-通用编码约束)
//!
//! - `Contradicts` 为唯一关系类型:P3 阶段仅检测矛盾关系,后续可扩展
//!   `Supports`/`Extends`/`Derives` 等正向关系(P4 学习层)
//!
//! - `source_id` 为新条目(矛盾源),`target_id` 为被矛盾的旧条目(标记 Historical)
//!   方向性保证:新 → 旧,语义清晰

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 关系类型 — 条目间的语义关系
///
/// WHY(P3-W11.2 D12):矛盾检测在写入路径执行,检测到矛盾时
/// 记录关系到 `entry_relations` 表,旧条目标记 Historical 不删除,
/// 保留谱系完整性供后续审计与时间感知召回。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelationKind {
    /// 矛盾关系 — 新条目与旧条目语义冲突
    ///
    /// WHY:spec.md:298 要求"检测 Contradicts 关系"。
    /// 检测标准(P3-W11.2.1 MVP):向量余弦相似度 > 阈值(默认 0.9)
    /// 判为矛盾候选。"宁可多标记不漏",幽灵冲突率 <1% 靠"标记过渡期不删旧"兜底
    Contradicts,
}

impl RelationKind {
    /// 返回关系名称(用于 SQLite 存储与日志)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Contradicts => "contradicts",
        }
    }

    /// 从字符串解析关系类型(用于 SQLite 反序列化)
    ///
    /// WHY:未实现 `std::str::FromStr` — 与 `Layer::from_str` 一致,
    /// 返回 `Option<Self>` 更适合"非错误即缺失"的语义
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "contradicts" => Some(Self::Contradicts),
            _ => None,
        }
    }
}

/// 条目间关系 — 记录两个 WikiEntry 之间的语义关系
///
/// WHY(P3-W11.2 D12):spec.md:298 "写入路径检测 Contradicts 关系 → 标记过渡期不删旧记录"
///
/// # 字段方向
///
/// - `source_id`:新条目(矛盾源,保持 Current 状态)
/// - `target_id`:被矛盾的旧条目(标记 Historical,不删除)
///
/// # 证据与置信度
///
/// - `evidence`:矛盾检测的证据描述(如 "cosine_similarity=0.9523")
/// - `confidence`:矛盾置信度 [0.0, 1.0],通常等于相似度值
///
/// # INV-8 兼容性
///
/// 旧条目被标记为 `Historical` 遵循 INV-8 单调性:
/// `Current` → `Historical` 单向降级,不逆向升级
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntryRelation {
    /// 关系唯一标识(格式: "rel-{source_id}-{target_id}")
    pub relation_id: String,
    /// 新条目 ID(矛盾源,保持 Current)
    pub source_id: String,
    /// 被矛盾的旧条目 ID(标记 Historical,不删除)
    pub target_id: String,
    /// 关系类型(当前仅 Contradicts)
    pub kind: RelationKind,
    /// 矛盾证据描述(如 "cosine_similarity=0.9523")
    pub evidence: String,
    /// 矛盾置信度 [0.0, 1.0]
    pub confidence: f32,
    /// 关系创建时间(UTC)
    pub created_at: DateTime<Utc>,
}

impl EntryRelation {
    /// 创建矛盾关系
    ///
    /// # 参数
    /// - `source_id`:新条目 ID(矛盾源)
    /// - `target_id`:被矛盾的旧条目 ID
    /// - `evidence`:矛盾证据描述
    /// - `confidence`:矛盾置信度
    pub fn new_contradiction(
        source_id: impl Into<String>,
        target_id: impl Into<String>,
        evidence: impl Into<String>,
        confidence: f32,
    ) -> Self {
        let source_id = source_id.into();
        let target_id = target_id.into();
        Self {
            relation_id: format!("rel-{source_id}-{target_id}"),
            source_id,
            target_id,
            kind: RelationKind::Contradicts,
            evidence: evidence.into(),
            confidence,
            created_at: Utc::now(),
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relation_kind_as_str_roundtrip() {
        let kind = RelationKind::Contradicts;
        let s = kind.as_str();
        assert_eq!(RelationKind::from_str(s), Some(kind));
    }

    #[test]
    fn test_relation_kind_from_str_invalid() {
        assert_eq!(RelationKind::from_str("invalid"), None);
        assert_eq!(RelationKind::from_str(""), None);
        assert_eq!(RelationKind::from_str("supports"), None); // 未实现的关系
    }

    #[test]
    fn test_entry_relation_new_contradiction() {
        let rel = EntryRelation::new_contradiction("e-new", "e-old", "cosine=0.95", 0.95);
        assert_eq!(rel.relation_id, "rel-e-new-e-old");
        assert_eq!(rel.source_id, "e-new");
        assert_eq!(rel.target_id, "e-old");
        assert_eq!(rel.kind, RelationKind::Contradicts);
        assert_eq!(rel.evidence, "cosine=0.95");
        assert!((rel.confidence - 0.95).abs() < 1e-6);
    }

    #[test]
    fn test_entry_relation_serde_roundtrip() {
        let rel = EntryRelation::new_contradiction("e-1", "e-2", "test", 0.9);
        let json = serde_json::to_string(&rel).unwrap();
        let restored: EntryRelation = serde_json::from_str(&json).unwrap();
        assert_eq!(rel, restored);
    }

    #[test]
    fn test_entry_relation_directionality() {
        // source = 新条目,target = 旧条目(被矛盾)
        let rel = EntryRelation::new_contradiction("e-new", "e-old", "", 0.9);
        assert_ne!(rel.source_id, rel.target_id);
        assert_eq!(rel.source_id, "e-new");
        assert_eq!(rel.target_id, "e-old");
    }

    #[test]
    fn test_relation_id_uniqueness() {
        let rel1 = EntryRelation::new_contradiction("a", "b", "", 0.9);
        let rel2 = EntryRelation::new_contradiction("a", "b", "", 0.9);
        // 相同 source/target 生成相同 relation_id(幂等,便于 UPSERT)
        assert_eq!(rel1.relation_id, rel2.relation_id);

        let rel3 = EntryRelation::new_contradiction("b", "a", "", 0.9);
        // 不同方向生成不同 relation_id
        assert_ne!(rel1.relation_id, rel3.relation_id);
    }
}
