//! GEA 核心类型 — 专家、门控值、激活结果与任务画像
//!
//! 对应架构层:L6 Router
//! 对应创新点:GEA(Gated Expert Activation)
//!
//! # 设计决策(WHY)
//! - `ExpertId` 用 newtype:类型安全,防止与其他 ID 混用
//! - `GateValue` 包装 f32:封装 `is_active` 逻辑,避免阈值判断散落各处
//! - `ExpertProfile.expert_vector` 为 64 维:与 CLV(512 维)不同,
//!   专家向量是压缩表示,门控计算时取最小长度(由 `cosine_similarity_slices` 处理)
//! - `TaskProfile.clv` 为可变长度 Vec:兼容 512 维 CLV 与其他维度向量

use serde::{Deserialize, Serialize};

use crate::error::GeaError;

// ============================================================
// 专家 ID — newtype 模式,类型安全
// ============================================================

nexus_core::id_newtype!(ExpertId, "专家唯一标识");

// ============================================================
// 专家画像
// ============================================================

/// 专家画像 — 描述一个专家的能力向量与元信息
///
/// `expert_vector` 为 64 维压缩表示,用于与任务 CLV 计算相关性。
/// WHY 64 维:专家向量是能力压缩表示,维度低于 CLV(512)以降低存储与计算成本;
/// 门控计算时由 `cosine_similarity_slices` 取最小长度,兼容维度差异。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpertProfile {
    /// 专家唯一 ID
    pub expert_id: ExpertId,
    /// 专家能力向量(64 维压缩表示)
    pub expert_vector: Vec<f32>,
    /// 优先级 [0.0, 1.0],影响冲突消解时的综合评分
    pub priority: f32,
    /// 能力标签(如 ["code-gen", "rust", "async"])
    pub capability_tags: Vec<String>,
}

impl ExpertProfile {
    /// 创建新的专家画像
    pub fn new(
        expert_id: impl Into<String>,
        expert_vector: Vec<f32>,
        priority: f32,
        capability_tags: Vec<String>,
    ) -> Self {
        Self {
            expert_id: ExpertId::new(expert_id),
            expert_vector,
            priority,
            capability_tags,
        }
    }
}

// ============================================================
// 门控值 — 包装 f32,封装激活判断
// ============================================================

/// 门控值 — sigmoid 输出的 [0.0, 1.0] 标量
///
/// 封装 `is_active` 判断逻辑,避免阈值比较散落各处。
/// 构造时校验值域,防止外部传入越界值。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GateValue(f32);

impl GateValue {
    /// 创建门控值,校验值域 ∈ [0.0, 1.0]
    ///
    /// # 错误
    /// - `InvalidGateValue`:值不在 [0.0, 1.0] 区间
    pub fn new(value: f32) -> Result<Self, GeaError> {
        if !(0.0..=1.0).contains(&value) || value.is_nan() {
            return Err(GeaError::InvalidGateValue { value });
        }
        Ok(Self(value))
    }

    /// 返回内部 f32 值
    pub fn value(&self) -> f32 {
        self.0
    }

    /// 判断是否激活:门控值 >= threshold
    ///
    /// WHY:阈值比较集中在此方法,避免各处硬编码 `>=`
    pub fn is_active(&self, threshold: f32) -> bool {
        self.0 >= threshold
    }
}

// ============================================================
// 激活结果
// ============================================================

/// 激活结果 — 包含已激活、被抑制的专家列表与最高门控值
///
/// `activated` 为 Top-K 专家(经冲突消解后),`suppressed` 为其余候选。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivationResult {
    /// 已激活的专家 ID 列表(Top-K,按综合评分降序)
    pub activated: Vec<ExpertId>,
    /// 被抑制的专家 ID 列表(未进入 Top-K 或因冲突被抑制)
    pub suppressed: Vec<ExpertId>,
    /// 综合评分最高的专家门控值 [0.0, 1.0]
    pub top_gate_value: f32,
}

impl ActivationResult {
    /// 创建空的激活结果(无专家激活)
    pub fn empty() -> Self {
        Self {
            activated: Vec::new(),
            suppressed: Vec::new(),
            top_gate_value: 0.0,
        }
    }

    /// 判断是否激活了至少一个专家
    pub fn has_activated(&self) -> bool {
        !self.activated.is_empty()
    }
}

// ============================================================
// 任务画像
// ============================================================

/// 任务画像 — 描述待激活专家的任务特征
///
/// `clv` 为上下文潜在向量,与专家向量计算相关性。
/// 维度可与 CLV(512)不同,门控计算取最小长度。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProfile {
    /// 复杂度评分 [0.0, 1.0]
    pub complexity_score: f32,
    /// 任务类型(如 "code-gen"、"refactor"、"test")
    pub task_type: String,
    /// 风险等级(0-100)
    pub risk_level: u8,
    /// 上下文潜在向量(通常 512 维 CLV)
    pub clv: Vec<f32>,
}

impl TaskProfile {
    /// 创建新的任务画像
    pub fn new(
        complexity_score: f32,
        task_type: impl Into<String>,
        risk_level: u8,
        clv: Vec<f32>,
    ) -> Self {
        Self {
            complexity_score,
            task_type: task_type.into(),
            risk_level,
            clv,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expert_id_newtype() {
        let id = ExpertId::new("expert-1");
        assert_eq!(id.as_str(), "expert-1");
        let id2 = ExpertId::from("expert-1");
        assert_eq!(id, id2);
    }

    #[test]
    fn test_expert_profile_new() {
        let profile = ExpertProfile::new(
            "e-1",
            vec![0.1; 64],
            0.8,
            vec!["code-gen".into(), "rust".into()],
        );
        assert_eq!(profile.expert_id.as_str(), "e-1");
        assert_eq!(profile.expert_vector.len(), 64);
        assert!((profile.priority - 0.8).abs() < 1e-6);
        assert_eq!(profile.capability_tags.len(), 2);
    }

    #[test]
    fn test_gate_value_valid() {
        let gv = GateValue::new(0.5).unwrap();
        assert!((gv.value() - 0.5).abs() < 1e-6);
        assert!(gv.is_active(0.5));
        assert!(!gv.is_active(0.6));
    }

    #[test]
    fn test_gate_value_boundary() {
        // 边界值 0.0 和 1.0 合法
        assert!(GateValue::new(0.0).is_ok());
        assert!(GateValue::new(1.0).is_ok());
    }

    #[test]
    fn test_gate_value_invalid() {
        assert!(GateValue::new(-0.1).is_err());
        assert!(GateValue::new(1.1).is_err());
        assert!(GateValue::new(f32::NAN).is_err());
    }

    #[test]
    fn test_activation_result_empty() {
        let result = ActivationResult::empty();
        assert!(!result.has_activated());
        assert!(result.activated.is_empty());
        assert!(result.suppressed.is_empty());
        assert!((result.top_gate_value - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_activation_result_has_activated() {
        let result = ActivationResult {
            activated: vec![ExpertId::new("e-1")],
            suppressed: vec![],
            top_gate_value: 0.8,
        };
        assert!(result.has_activated());
    }

    #[test]
    fn test_task_profile_new() {
        let task = TaskProfile::new(0.7, "code-gen", 30, vec![0.5; 512]);
        assert!((task.complexity_score - 0.7).abs() < 1e-6);
        assert_eq!(task.task_type, "code-gen");
        assert_eq!(task.risk_level, 30);
        assert_eq!(task.clv.len(), 512);
    }

    #[test]
    fn test_serde_roundtrip() {
        let profile = ExpertProfile::new("e-1", vec![0.1; 64], 0.8, vec!["rust".into()]);
        let json = serde_json::to_string(&profile).unwrap();
        let restored: ExpertProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.expert_id, profile.expert_id);
        assert!((restored.priority - profile.priority).abs() < 1e-6);
    }
}
