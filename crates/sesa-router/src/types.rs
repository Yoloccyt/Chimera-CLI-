//! SESA 核心类型定义 — 激活请求与专家描述符
//!
//! 对应架构层:L6 Router
//! 对应创新点:SESA(Sub-Expert Sparse Activation)
//!
//! ## 类型关系
//! - `ExpertDescriptor`:专家元数据,携带语义向量用于激活评分
//! - `ActivationRequest`:运行时激活请求,指定查询向量与 Top-K 参数
//! - `TaskPhase`:任务阶段枚举,用于动态稀疏度策略选择
//!
//! WHY 拆分为独立 types.rs:遵循 ssra-fusion/faae-router 的模块组织模式,
//! 将领域类型与业务逻辑分离,便于跨模块引用。

use serde::{Deserialize, Serialize};

/// 任务阶段 — 驱动 SESA 稀疏度阈值的动态自适应策略
///
/// 不同任务阶段对专家覆盖度的需求不同:
/// - 编码阶段: 需求精确,稀疏化最激进(20%),仅激活最相关的少数专家
/// - 执行阶段: 需求适中,标准稀疏化(40%),平衡计算开销与专家覆盖
/// - 调试阶段: 需求广泛,稀疏化最宽松(60%),激活更多专家以覆盖边界情况
///
/// 对应 ADR-055(SESA 动态稀疏度阈值),解决静态 40% 阈值在编码/调试
/// 阶段过于激进或过于宽松的问题。
///
/// # 稀疏度策略映射
///
/// | 阶段     | max_sparsity_ratio | 语义                          |
/// |----------|--------------------|-------------------------------|
/// | `Coding` | 0.2                | 激进稀疏,仅激活最相关专家    |
/// | `Execute`| 0.4                | 标准稀疏,平衡开销与覆盖      |
/// | `Debug`  | 0.6                | 宽松稀疏,高覆盖以排查问题    |
/// | `None`   | 配置默认值         | 未指定阶段,使用 SesaConfig   |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskPhase {
    /// 编码阶段: 需求精确,稀疏化激进(20%)
    Coding,
    /// 执行阶段: 需求适中,标准稀疏化(40%)
    Execute,
    /// 调试阶段: 需求广泛,稀疏化宽松(60%)
    Debug,
}

impl TaskPhase {
    /// 返回当前阶段对应的最大稀疏度比例
    ///
    /// 返回 `None` 表示未指定阶段,应使用 `SesaConfig.max_sparsity_ratio` 默认值。
    pub fn max_sparsity_ratio(self) -> Option<f32> {
        match self {
            TaskPhase::Coding => Some(0.2),
            TaskPhase::Execute => Some(0.4),
            TaskPhase::Debug => Some(0.6),
        }
    }
}

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
///
/// 新增 `task_phase` 字段(ADR-055):用于动态稀疏度策略选择。
/// 未指定时使用 `SesaConfig.max_sparsity_ratio` 静态默认值。
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
    /// 任务阶段 — 用于动态稀疏度策略选择(ADR-055)
    ///
    /// None 时使用 `SesaConfig.max_sparsity_ratio` 静态默认值(40%)。
    /// 指定阶段时采用对应策略:
    /// - `Coding` → 20%(激进稀疏,仅激活最相关专家)
    /// - `Execute` → 40%(标准稀疏,平衡开销与覆盖)
    /// - `Debug` → 60%(宽松稀疏,高覆盖以排查问题)
    pub task_phase: Option<TaskPhase>,
}

impl ActivationRequest {
    /// 创建激活请求(不指定任务阶段,使用静态默认稀疏度)
    ///
    /// # 参数
    /// - `request_id`:请求唯一标识(用于事件追踪)
    /// - `query_vector`:查询向量(与专家向量同维度)
    /// - `top_k`:Top-K 选择数
    /// - `deadline_ms`:截止时间(毫秒)
    ///
    /// 若需指定任务阶段以启用动态稀疏度,使用 `with_task_phase()` builder 方法。
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
            task_phase: None,
        }
    }

    /// 设置任务阶段(builder 模式,链式调用)
    ///
    /// 指定后激活时使用该阶段对应的动态稀疏度阈值,
    /// 而非 `SesaConfig.max_sparsity_ratio` 静态默认值。
    ///
    /// # 示例
    /// ```ignore
    /// let req = ActivationRequest::new("req-1", vec![0.5; 64], 8, 5)
    ///     .with_task_phase(TaskPhase::Coding); // 20% 激进稀疏
    /// ```
    pub fn with_task_phase(mut self, phase: TaskPhase) -> Self {
        self.task_phase = Some(phase);
        self
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
        assert!(restored.task_phase.is_none());
    }

    // === TaskPhase 测试 ===

    #[test]
    fn test_task_phase_max_sparsity_ratio() {
        assert_eq!(TaskPhase::Coding.max_sparsity_ratio(), Some(0.2));
        assert_eq!(TaskPhase::Execute.max_sparsity_ratio(), Some(0.4));
        assert_eq!(TaskPhase::Debug.max_sparsity_ratio(), Some(0.6));
    }

    #[test]
    fn test_activation_request_with_task_phase() {
        let req =
            ActivationRequest::new("req-1", vec![0.5; 64], 8, 5).with_task_phase(TaskPhase::Coding);
        assert_eq!(req.task_phase, Some(TaskPhase::Coding));
        assert_eq!(
            req.task_phase.and_then(|p| p.max_sparsity_ratio()),
            Some(0.2)
        );
    }

    #[test]
    fn test_activation_request_default_no_task_phase() {
        let req = ActivationRequest::new("req-1", vec![0.5; 64], 8, 5);
        assert!(req.task_phase.is_none());
    }

    #[test]
    fn test_activation_request_serde_with_task_phase() {
        let req =
            ActivationRequest::new("req-1", vec![0.5; 64], 8, 5).with_task_phase(TaskPhase::Debug);
        let json = serde_json::to_string(&req).expect("序列化失败");
        let restored: ActivationRequest = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.task_phase, Some(TaskPhase::Debug));
    }
}
