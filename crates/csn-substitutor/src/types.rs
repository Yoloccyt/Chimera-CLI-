//! CSN 核心类型定义 — 能力描述符、替代候选与降级链
//!
//! 对应架构层:L10 Interface
//! 对应创新点:CSN(Capability Substitution Network)
//!
//! ## 类型关系
//! - `CapabilityDescriptor`:注册到注册表的能力描述,携带 50 维语义向量
//! - `SubstitutionCandidate`:替代查询产出,携带候选 ID、相似度得分与层级
//! - `DegradationChain`:多级降级链,支持 ≥ 3 级逐级回退

use serde::{Deserialize, Serialize};

/// 能力元数据 — 携带能力的非语义属性
///
/// 用于在替代选择时辅助决策(如优先选择同 tier 的能力)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilityMetadata {
    /// 能力类别(如 "shell"/"code-gen"/"search")
    pub category: String,
    /// 能力版本(语义版本字符串,如 "1.0.0")
    pub version: String,
    /// 是否为关键能力(关键能力降级时需额外告警)
    pub critical: bool,
}

impl CapabilityMetadata {
    /// 创建能力元数据
    pub fn new(category: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            category: category.into(),
            version: version.into(),
            critical: false,
        }
    }

    /// 标记为关键能力,返回 self 以便链式调用
    pub fn with_critical(mut self, critical: bool) -> Self {
        self.critical = critical;
        self
    }
}

impl Default for CapabilityMetadata {
    fn default() -> Self {
        Self {
            category: "unknown".into(),
            version: "0.0.0".into(),
            critical: false,
        }
    }
}

/// 能力描述符 — 注册到 CSN 注册表的能力单元
///
/// 每个能力携带 50 维语义向量(`semantic_vector`),用于余弦相似度计算。
/// `metadata` 提供辅助决策信息(类别、版本、关键性)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilityDescriptor {
    /// 能力 ID(唯一标识,如 "shell-exec" / "code-gen")
    pub capability_id: String,
    /// 语义向量(50 维,与 `CsnConfig::vector_dimension` 对齐)
    pub semantic_vector: Vec<f32>,
    /// 能力元数据
    pub metadata: CapabilityMetadata,
}

impl CapabilityDescriptor {
    /// 创建能力描述符,metadata 使用默认值
    pub fn new(capability_id: impl Into<String>, semantic_vector: Vec<f32>) -> Self {
        Self {
            capability_id: capability_id.into(),
            semantic_vector,
            metadata: CapabilityMetadata::default(),
        }
    }

    /// 设置元数据,返回 self 以便链式调用
    pub fn with_metadata(mut self, metadata: CapabilityMetadata) -> Self {
        self.metadata = metadata;
        self
    }
}

/// 替代候选 — 替代查询的产出
///
/// `tier` 表示候选在降级链中的层级:
/// - `0`:primary(首选替代)
/// - `1`:secondary(次选替代)
/// - `2`:tertiary(末选替代)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubstitutionCandidate {
    /// 候选能力 ID
    pub candidate_id: String,
    /// 余弦相似度得分([-1.0, 1.0],越高越相似)
    pub similarity_score: f32,
    /// 降级层级(0=primary, 1=secondary, 2=tertiary)
    pub tier: u32,
}

impl SubstitutionCandidate {
    /// 创建替代候选
    pub fn new(candidate_id: impl Into<String>, similarity_score: f32, tier: u32) -> Self {
        Self {
            candidate_id: candidate_id.into(),
            similarity_score,
            tier,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === CapabilityMetadata 测试 ===

    #[test]
    fn test_metadata_new_defaults() {
        let m = CapabilityMetadata::new("shell", "1.0.0");
        assert_eq!(m.category, "shell");
        assert_eq!(m.version, "1.0.0");
        assert!(!m.critical, "默认非关键");
    }

    #[test]
    fn test_metadata_with_critical() {
        let m = CapabilityMetadata::new("shell", "1.0.0").with_critical(true);
        assert!(m.critical);
    }

    #[test]
    fn test_metadata_default_values() {
        let m = CapabilityMetadata::default();
        assert_eq!(m.category, "unknown");
        assert_eq!(m.version, "0.0.0");
    }

    // === CapabilityDescriptor 测试 ===

    #[test]
    fn test_descriptor_new() {
        let d = CapabilityDescriptor::new("cap-1", vec![1.0; 50]);
        assert_eq!(d.capability_id, "cap-1");
        assert_eq!(d.semantic_vector.len(), 50);
        assert_eq!(d.metadata.category, "unknown");
    }

    #[test]
    fn test_descriptor_with_metadata() {
        let meta = CapabilityMetadata::new("code-gen", "2.0.0").with_critical(true);
        let d = CapabilityDescriptor::new("cap-1", vec![1.0; 50]).with_metadata(meta);
        assert_eq!(d.metadata.category, "code-gen");
        assert_eq!(d.metadata.version, "2.0.0");
        assert!(d.metadata.critical);
    }

    #[test]
    fn test_descriptor_serde_roundtrip() {
        let d = CapabilityDescriptor::new("cap-1", vec![1.0, 0.5, 0.0])
            .with_metadata(CapabilityMetadata::new("shell", "1.0.0").with_critical(true));
        let json = serde_json::to_string(&d).expect("序列化失败");
        let restored: CapabilityDescriptor = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(d, restored);
    }

    // === SubstitutionCandidate 测试 ===

    #[test]
    fn test_candidate_new() {
        let c = SubstitutionCandidate::new("cap-2", 0.85, 0);
        assert_eq!(c.candidate_id, "cap-2");
        assert!((c.similarity_score - 0.85).abs() < 1e-6);
        assert_eq!(c.tier, 0);
    }

    #[test]
    fn test_candidate_serde_roundtrip() {
        let c = SubstitutionCandidate::new("cap-2", 0.95, 1);
        let json = serde_json::to_string(&c).expect("序列化失败");
        let restored: SubstitutionCandidate = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(c, restored);
    }

    #[test]
    fn test_candidate_equality() {
        let c1 = SubstitutionCandidate::new("cap-1", 0.9, 0);
        let c2 = SubstitutionCandidate::new("cap-1", 0.9, 0);
        let c3 = SubstitutionCandidate::new("cap-1", 0.9, 1);
        assert_eq!(c1, c2, "字段全等应相等");
        assert_ne!(c1, c3, "tier 不同应不等");
    }
}
