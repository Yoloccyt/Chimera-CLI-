//! SSRA 核心类型定义 — 模板、融合请求与结果
//!
//! 对应架构层:L7 Execution
//! 对应创新点:SSRA(Slime-Style Rapid Adaptation)
//!
//! ## 类型关系
//! - `SlimeTemplate`:预编译的可融合模板单元,携带权重与融合策略
//! - `FusionRequest`:运行时融合请求,指定源适配器与 Top-K 参数
//! - `FusionResult`:融合产出,包含模板 ID、延迟与置信度

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 融合策略 — 控制多模板融合时的聚合方式
///
/// 不同策略影响最终置信度的计算公式:
/// - `WeightedAverage`:Σ(w_i²) / Σ(w_i),偏向高权重模板
/// - `TopK`:max(w_i),取最强模板的权重
/// - `MeanField`:Σ(w_i) / k,Top-K 算术平均
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FusionStrategy {
    /// 加权平均:按权重平方加权,偏向高适配强度模板
    WeightedAverage,
    /// Top-K:取评分最高的单个模板置信度
    TopK,
    /// 平均场:取 Top-K 模板置信度的算术平均
    MeanField,
}

/// 预编译适配器模板 — slime 机制的最小可融合单元
///
/// 每个模板对应一个能力(capability),携带参数形状、融合策略与适配权重。
/// `compiled_at` 用于 LRU 驱逐排序(最旧的优先驱逐)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SlimeTemplate {
    /// 能力 ID(唯一标识,如 "shell-exec" / "code-gen")
    pub capability_id: String,
    /// 参数形状(参数名列表,用于源适配器兼容性校验)
    pub parameter_shape: Vec<String>,
    /// 融合策略(该模板参与融合时采用的聚合方式)
    pub fusion_strategy: FusionStrategy,
    /// 预编译时刻(UTC,用于 LRU 驱逐排序)
    pub compiled_at: DateTime<Utc>,
    /// 适配权重 [0.0, 1.0],表示该模板的历史适配强度
    ///
    /// WHY:权重越高代表该模板历史适配成功率越高,融合时优先选择。
    /// 预编译时默认 1.0,运行时可根据 GSOE 进化信号动态调整。
    pub weight: f32,
}

impl SlimeTemplate {
    /// 创建新模板,weight 默认 1.0,compiled_at 默认当前 UTC 时间
    pub fn new(
        capability_id: impl Into<String>,
        parameter_shape: Vec<String>,
        fusion_strategy: FusionStrategy,
    ) -> Self {
        Self {
            capability_id: capability_id.into(),
            parameter_shape,
            fusion_strategy,
            compiled_at: Utc::now(),
            weight: 1.0,
        }
    }

    /// 设置适配权重,返回 self 以便链式调用
    pub fn with_weight(mut self, weight: f32) -> Self {
        self.weight = weight.clamp(0.0, 1.0);
        self
    }
}

/// 融合请求 — 描述一次运行时适配融合的输入
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusionRequest {
    /// 源适配器能力 ID 列表(从 registry 查找对应模板)
    pub source_adapters: Vec<String>,
    /// 目标能力 ID(融合产出的模板归属)
    pub target_capability: String,
    /// 融合截止时间(毫秒),超时返回 FusionTimeout
    pub deadline_ms: u64,
    /// Top-K 选择的 K 值(从源模板中选出评分最高的 K 个)
    pub top_k: usize,
    /// 关联的 Quest ID(用于事件发布与因果追踪)
    pub quest_id: String,
}

impl FusionRequest {
    /// 创建融合请求
    ///
    /// - `quest_id`:关联 Quest,用于事件追踪
    /// - `source_adapters`:源适配器能力 ID 列表
    /// - `target_capability`:融合目标能力 ID
    /// - `deadline_ms`:截止时间(毫秒)
    /// - `top_k`:Top-K 选择数
    pub fn new(
        quest_id: impl Into<String>,
        source_adapters: Vec<String>,
        target_capability: impl Into<String>,
        deadline_ms: u64,
        top_k: usize,
    ) -> Self {
        Self {
            quest_id: quest_id.into(),
            source_adapters,
            target_capability: target_capability.into(),
            deadline_ms,
            top_k,
        }
    }
}

/// 融合结果 — 描述一次适配融合的产出
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FusionResult {
    /// 融合产出的模板 ID(UUIDv7 字符串)
    pub fused_template_id: String,
    /// 融合延迟(毫秒)
    pub latency_ms: u64,
    /// 融合置信度 [0.0, 1.0]
    pub confidence: f32,
    /// 实际参与融合的模板数量
    pub selected_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_new_default_weight() {
        let t = SlimeTemplate::new("cap-1", vec!["x".into()], FusionStrategy::TopK);
        assert_eq!(t.capability_id, "cap-1");
        assert_eq!(t.parameter_shape, vec!["x"]);
        assert_eq!(t.fusion_strategy, FusionStrategy::TopK);
        assert!((t.weight - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_template_with_weight_clamped() {
        let t = SlimeTemplate::new("cap-1", vec![], FusionStrategy::TopK).with_weight(1.5);
        assert!(
            (t.weight - 1.0).abs() < f32::EPSILON,
            "weight 应被钳制到 1.0"
        );

        let t2 = SlimeTemplate::new("cap-2", vec![], FusionStrategy::TopK).with_weight(-0.5);
        assert!(
            (t2.weight - 0.0).abs() < f32::EPSILON,
            "weight 应被钳制到 0.0"
        );
    }

    #[test]
    fn test_fusion_request_new() {
        let req = FusionRequest::new("q-1", vec!["a".into(), "b".into()], "target", 20, 8);
        assert_eq!(req.quest_id, "q-1");
        assert_eq!(req.source_adapters.len(), 2);
        assert_eq!(req.target_capability, "target");
        assert_eq!(req.deadline_ms, 20);
        assert_eq!(req.top_k, 8);
    }

    #[test]
    fn test_fusion_strategy_serde() {
        let s = FusionStrategy::WeightedAverage;
        let json = serde_json::to_string(&s).expect("序列化失败");
        let restored: FusionStrategy = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(s, restored);
    }

    #[test]
    fn test_template_serde_roundtrip() {
        let t = SlimeTemplate::new(
            "cap-1",
            vec!["x".into(), "y".into()],
            FusionStrategy::MeanField,
        )
        .with_weight(0.75);
        let json = serde_json::to_string(&t).expect("序列化失败");
        let restored: SlimeTemplate = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(t, restored);
    }
}
