//! SESA 核心类型定义 — 激活请求与专家描述符
//!
//! 对应架构层:L6 Router
//! 对应创新点:SESA(Sub-Expert Sparse Activation)
//!
//! ## 类型关系
//! - `ExpertDescriptor`:专家元数据,携带语义向量用于激活评分
//! - `ActivationRequest`:运行时激活请求,指定查询向量与 Top-K 参数
//!
//! WHY 拆分为独立 types.rs:遵循 ssra-fusion/faae-router 的模块组织模式,
//! 将领域类型与业务逻辑分离,便于跨模块引用。

use serde::{Deserialize, Serialize};

/// 专家描述符 — 描述一个可激活的子专家
///
/// 每个专家携带语义向量(expert_vector),激活时与查询向量计算余弦相似度,
/// 评分最高的 Top-K 专家将被激活(掩码对应位置置 1)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpertDescriptor {
    /// 专家 ID(唯一标识,如 "expert-1")
    pub expert_id: String,
    /// 语义向量(与查询向量同维度,通常为 64-dim)
    ///
    /// WHY 64-dim:与 FaaE 专家向量对齐,余弦相似度计算时维度匹配。
    /// 维度由调用方保证,SESA 不强制校验(零向量返回相似度 0)。
    pub expert_vector: Vec<f32>,
    /// 专家在掩码中的索引(0-255,由注册顺序自动分配)
    ///
    /// WHY 内部字段:由 `SesaRouter::register_expert` 自动分配,
    /// 外部无需设置。索引一旦分配不可变,专家注销后索引不回收。
    pub mask_index: u32,
}

impl ExpertDescriptor {
    /// 创建新专家描述符(mask_index 默认 0,由 router 注册时分配)
    ///
    /// # 参数
    /// - `expert_id`:专家唯一标识
    /// - `expert_vector`:语义向量(与查询向量同维度)
    pub fn new(expert_id: impl Into<String>, expert_vector: Vec<f32>) -> Self {
        Self {
            expert_id: expert_id.into(),
            expert_vector,
            mask_index: 0,
        }
    }

    /// 设置掩码索引(builder 模式,由 SesaRouter 内部调用或测试使用)
    ///
    /// WHY pub 而非 pub(crate):作为 builder 链式 API 的一部分,
    /// 便于测试构造特定 mask_index 的专家描述符。
    pub fn with_mask_index(mut self, idx: u32) -> Self {
        self.mask_index = idx;
        self
    }
}

/// 激活请求 — 描述一次子专家稀疏激活的输入
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivationRequest {
    /// 请求 ID(UUIDv7 字符串,由调用方生成,用于事件追踪)
    pub request_id: String,
    /// 查询向量(与专家向量同维度,通常为 64-dim)
    pub query_vector: Vec<f32>,
    /// Top-K 选择的 K 值(从注册专家中选出评分最高的 K 个)
    pub top_k: usize,
    /// 激活截止时间(毫秒),超时返回 ActivationTimeout
    pub deadline_ms: u64,
}

impl ActivationRequest {
    /// 创建激活请求
    ///
    /// # 参数
    /// - `request_id`:请求唯一标识(用于事件追踪)
    /// - `query_vector`:查询向量(与专家向量同维度)
    /// - `top_k`:Top-K 选择数
    /// - `deadline_ms`:截止时间(毫秒)
    pub fn new(
        request_id: impl Into<String>,
        query_vector: Vec<f32>,
        top_k: usize,
        deadline_ms: u64,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            query_vector,
            top_k,
            deadline_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expert_descriptor_new() {
        let expert = ExpertDescriptor::new("expert-1", vec![0.5; 64]);
        assert_eq!(expert.expert_id, "expert-1");
        assert_eq!(expert.expert_vector.len(), 64);
        assert_eq!(expert.mask_index, 0, "默认 mask_index 应为 0");
    }

    #[test]
    fn test_expert_descriptor_with_mask_index() {
        let expert = ExpertDescriptor::new("expert-1", vec![0.5; 64]).with_mask_index(42);
        assert_eq!(expert.mask_index, 42);
    }

    #[test]
    fn test_activation_request_new() {
        let req = ActivationRequest::new("req-1", vec![0.5; 64], 8, 5);
        assert_eq!(req.request_id, "req-1");
        assert_eq!(req.query_vector.len(), 64);
        assert_eq!(req.top_k, 8);
        assert_eq!(req.deadline_ms, 5);
    }

    #[test]
    fn test_expert_descriptor_serde_roundtrip() {
        let expert = ExpertDescriptor::new("expert-1", vec![0.1, 0.2, 0.3]).with_mask_index(5);
        let json = serde_json::to_string(&expert).expect("序列化失败");
        let restored: ExpertDescriptor = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.expert_id, "expert-1");
        assert_eq!(restored.expert_vector, vec![0.1, 0.2, 0.3]);
        assert_eq!(restored.mask_index, 5);
    }

    #[test]
    fn test_activation_request_serde_roundtrip() {
        let req = ActivationRequest::new("req-1", vec![0.5; 64], 8, 5);
        let json = serde_json::to_string(&req).expect("序列化失败");
        let restored: ActivationRequest = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.request_id, "req-1");
        assert_eq!(restored.top_k, 8);
        assert_eq!(restored.deadline_ms, 5);
    }
}
