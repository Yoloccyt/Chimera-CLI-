//! 全维稀疏掩码 — 五维度掩码聚合体 `OmniSparseMasks`
//!
//! 对应架构层: L0 Contracts（从 osa-coordinator L6 上提）
//! 对应 ADR: ADR-033
//!
//! # 设计决策(WHY)
//!
//! - **从 osa-coordinator 上提**: 原 `OmniSparseMasks` 定义在 `osa-coordinator/src/coordinator.rs`，
//!   被 L6 Router × 3（kvbsr-router / faae-router / sesa-router）依赖。上提至 L0 后消除星型耦合
//!
//! - **分离纯数据与哈希逻辑**: L0 禁止依赖 `sha2` / `hex`（仅允许 serde 派生），
//!   因此 `mask_hash` 计算逻辑保留在 `osa-coordinator`（L6）。
//!   L0 的 `OmniSparseMasks` 仅承载五维度掩码数据 + `average_sparsity()` 纯计算
//!
//! - **向后兼容**: 字段名与序列化格式与原 `osa-coordinator::OmniSparseMasks` 完全一致，
//!   原 `#[serde(skip)] mask_hash` 字段移除后不影响序列化输出（本来就被 skip）。
//!   `osa-coordinator` 后续版本将提供 `compute_mask_hash()` 自由函数替代缓存字段
//!
//! # 消费者
//!
//! - L2 `hcw-window`: 订阅 `OmniSparseMasksComputed` 事件后根据 context_mask 加载活跃文件
//! - L6 `kvbsr-router` / `faae-router` / `sesa-router`: 读取 OSA 计算的掩码进行路由决策

use crate::ids::{FileId, MemoryId, OperationId, TaskId, ToolId};
use crate::masks::SparseMask;
use serde::{Deserialize, Serialize};

/// 全维稀疏掩码 — 五维度掩码的聚合体
///
/// 由 `OmniSparseCoordinator::compute_all_masks` 返回，包含:
/// - `routing`: 工具稀疏掩码(Top-K 工具)
/// - `context`: 文件稀疏掩码(Top-K 文件)
/// - `memory`: 记忆稀疏掩码(Top-K 记忆)
/// - `audit`: 操作稀疏掩码(按采样率选取)
/// - `budget`: 任务稀疏掩码(按保护比例选取)
///
/// WHY: 聚合为单一结构体，便于一次性传递给下游消费者(如 HCW)，
/// 避免五维度分多次传递导致的状态不一致
///
/// # 哈希计算
///
/// `mask_hash` 计算逻辑保留在 `osa-coordinator`（L6），因 L0 禁止依赖 `sha2` / `hex`。
/// `osa-coordinator` 提供 `compute_omni_mask_hash(masks: &OmniSparseMasks) -> Result<String, OsaError>`
/// 自由函数，在 `compute_all_masks()` 中调用一次后随事件发布。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OmniSparseMasks {
    /// routing 维度: 工具稀疏掩码
    pub routing: SparseMask<ToolId>,
    /// context 维度: 文件稀疏掩码
    pub context: SparseMask<FileId>,
    /// memory 维度: 记忆稀疏掩码
    pub memory: SparseMask<MemoryId>,
    /// audit 维度: 操作稀疏掩码
    pub audit: SparseMask<OperationId>,
    /// budget 维度: 任务稀疏掩码
    pub budget: SparseMask<TaskId>,
}

impl OmniSparseMasks {
    /// 构造 OmniSparseMasks
    ///
    /// 直接组装五维度掩码，不计算 mask_hash。
    /// 哈希计算由 `osa-coordinator::compute_omni_mask_hash()` 自由函数提供（L6 依赖 sha2/hex）。
    ///
    /// WHY: L0 禁止依赖 `sha2` / `hex`，哈希逻辑留在 L6。
    /// 原 `osa-coordinator::OmniSparseMasks::new()` 返回 `Result<Self, OsaError>` 因哈希可能失败，
    /// 迁移后 `new()` 为纯构造函数，无需 Result 包装。
    pub fn new(
        routing: SparseMask<ToolId>,
        context: SparseMask<FileId>,
        memory: SparseMask<MemoryId>,
        audit: SparseMask<OperationId>,
        budget: SparseMask<TaskId>,
    ) -> Self {
        Self {
            routing,
            context,
            memory,
            audit,
            budget,
        }
    }

    /// 计算五维度掩码的平均稀疏度 [0.0, 1.0]
    ///
    /// WHY: 平均稀疏度作为 `OmniSparseMasksComputed` 事件的 `sparsity` 字段，
    /// 消费者据此快速判断整体稀疏程度，无需解析具体掩码
    pub fn average_sparsity(&self) -> f32 {
        (self.routing.sparsity_ratio
            + self.context.sparsity_ratio
            + self.memory.sparsity_ratio
            + self.audit.sparsity_ratio
            + self.budget.sparsity_ratio)
            / 5.0
    }

    /// 返回 routing 维度活跃工具列表的引用
    pub fn routing_ids(&self) -> &[ToolId] {
        &self.routing.active_ids
    }

    /// 返回 context 维度活跃文件列表的引用
    pub fn context_ids(&self) -> &[FileId] {
        &self.context.active_ids
    }

    /// 返回 memory 维度活跃记忆列表的引用
    pub fn memory_ids(&self) -> &[MemoryId] {
        &self.memory.active_ids
    }

    /// 返回 audit 维度活跃操作列表的引用
    pub fn audit_ids(&self) -> &[OperationId] {
        &self.audit.active_ids
    }

    /// 返回 budget 维度活跃任务列表的引用
    pub fn budget_ids(&self) -> &[TaskId] {
        &self.budget.active_ids
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_masks() -> OmniSparseMasks {
        let routing = SparseMask::full(vec![ToolId::new("tool-1"), ToolId::new("tool-2")]);
        let context = SparseMask::full(vec![FileId::new("file-1")]);
        let memory = SparseMask::empty();
        let audit = SparseMask::select_top_k(
            &[OperationId::new("op-1"), OperationId::new("op-2")],
            &[0.8, 0.2],
            1,
        );
        let budget = SparseMask::full(vec![TaskId::new("task-1")]);
        OmniSparseMasks::new(routing, context, memory, audit, budget)
    }

    #[test]
    fn test_omni_masks_construction() {
        let masks = make_test_masks();
        assert_eq!(masks.routing.active_count(), 2);
        assert_eq!(masks.context.active_count(), 1);
        assert_eq!(masks.memory.active_count(), 0);
        assert_eq!(masks.audit.active_count(), 1);
        assert_eq!(masks.budget.active_count(), 1);
    }

    #[test]
    fn test_average_sparsity() {
        let masks = make_test_masks();
        // routing: 2 items, full → sparsity 0.0
        // context: 1 item, full → sparsity 0.0
        // memory: empty → sparsity 1.0
        // audit: 2 items, select 1 → sparsity 0.5
        // budget: 1 item, full → sparsity 0.0
        // average = (0.0 + 0.0 + 1.0 + 0.5 + 0.0) / 5 = 0.3
        assert!((masks.average_sparsity() - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_serde_roundtrip() {
        let masks = make_test_masks();
        let json = serde_json::to_string(&masks).expect("序列化失败");
        let restored: OmniSparseMasks = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(masks, restored);
    }

    #[test]
    fn test_accessor_methods() {
        let masks = make_test_masks();
        assert_eq!(masks.routing_ids().len(), 2);
        assert_eq!(masks.context_ids().len(), 1);
        assert_eq!(masks.memory_ids().len(), 0);
        assert_eq!(masks.audit_ids().len(), 1);
        assert_eq!(masks.budget_ids().len(), 1);
    }

    #[test]
    fn test_partial_eq_independent_of_hash() {
        // 两个相同五维度掩码的 OmniSparseMasks 应相等
        // (迁移后无 mask_hash 字段，PartialEq 直接派生)
        let masks1 = make_test_masks();
        let masks2 = make_test_masks();
        assert_eq!(masks1, masks2);
    }

    #[test]
    fn test_serde_no_mask_hash_field() {
        // 迁移后序列化输出不应包含 mask_hash 字段
        // (原 osa-coordinator 版本有 #[serde(skip)] mask_hash，本版本直接移除字段)
        let masks = make_test_masks();
        let json = serde_json::to_string(&masks).expect("序列化失败");
        assert!(
            !json.contains("mask_hash"),
            "序列化输出不应包含 mask_hash 字段: {json}"
        );
    }
}
