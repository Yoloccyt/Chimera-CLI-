//! L2 记忆层协同接口 — MemorySyncHook 依赖倒置（设计文档 §14 二次审查增强）
//!
//! 对应架构层: **L9 Quest**（quest-engine 子模块）
//! 对应设计源: Phase 9 二次审查增强计划 Wave 3（L2 记忆层协同接口）
//!
//! # 核心职责
//!
//! 为 SearchTreeManager / LongTaskMap 提供与 L2 记忆层（mlc-engine）的
//! 协同接口，采用**依赖倒置**（类似 `ambient_mode::MemoryTidyHook` 先例）：
//! - L9 不直接依赖 L2 mlc-engine（L9→L2 虽向下合规，但直接依赖引入耦合）
//! - 定义 `MemorySyncHook` trait，由 mlc-engine 或调用方注入实现
//! - 未注入时使用 `NoopMemorySyncHook`（无操作，不破坏既有行为）
//!
//! # 协同语义
//!
//! - **搜索树最优路径变更** → `on_search_tree_best_path`：搜索树 best_node
//!   更新时同步最优路径摘要到记忆层（供记忆召回/沉淀）
//! - **任务地图步骤记录** → `on_task_map_step`：任务地图 record_step 时
//!   同步步骤摘要到记忆层（供长任务记忆沉淀）
//!
//! # 设计约束
//!
//! - **不直接依赖 mlc-engine**：trait 注入保持模块解耦
//! - **同步调用**：hook 在状态变更上下文同步调用，实现须轻量
//!   （异步内部化由实现方负责，同 MemoryTidyHook 先例）

/// L2 记忆层同步钩子 — 依赖倒置注入点
///
/// 由 mlc-engine 或调用方注入实现；未注入时用 [`NoopMemorySyncHook`]。
///
/// WHY `Debug` 超trait：SearchTreeManager/LongTaskMap 派生 Debug，
/// 持有 `Box<dyn MemorySyncHook>` 字段需 Debug（实现方须提供）。
pub trait MemorySyncHook: Send + Sync + std::fmt::Debug {
    /// 搜索树最优路径变更时同步到记忆层
    ///
    /// - `quest_id`: 所属任务 ID
    /// - `best_path_summary`: 最优路径摘要（根 → best 节点的 method_family 链）
    fn on_search_tree_best_path(&self, quest_id: &str, best_path_summary: &str);

    /// 任务地图步骤记录时同步到记忆层
    ///
    /// - `quest_id`: 所属任务 ID
    /// - `step_summary`: 步骤摘要（state_summary + next_action）
    fn on_task_map_step(&self, quest_id: &str, step_summary: &str);
}

/// 空操作记忆同步钩子 — 默认实现（未注入 hook 时无操作）
///
/// 保持既有 SearchTreeManager/LongTaskMap `new()` 行为不变。
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopMemorySyncHook;

impl MemorySyncHook for NoopMemorySyncHook {
    fn on_search_tree_best_path(&self, _quest_id: &str, _best_path_summary: &str) {
        // 无操作
    }

    fn on_task_map_step(&self, _quest_id: &str, _step_summary: &str) {
        // 无操作
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// 计数钩子 — 记录调用次数
    #[derive(Debug, Default)]
    struct CountingHook {
        best_path_calls: AtomicUsize,
        task_step_calls: AtomicUsize,
    }

    impl MemorySyncHook for CountingHook {
        fn on_search_tree_best_path(&self, _quest_id: &str, _summary: &str) {
            self.best_path_calls.fetch_add(1, Ordering::SeqCst);
        }

        fn on_task_map_step(&self, _quest_id: &str, _summary: &str) {
            self.task_step_calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn noop_hook_no_panic() {
        let hook = NoopMemorySyncHook;
        hook.on_search_tree_best_path("q1", "root → n1");
        hook.on_task_map_step("q1", "step summary");
        // 无操作不 panic 即通过
    }

    #[test]
    fn counting_hook_records_calls() {
        let hook = Arc::new(CountingHook::default());
        hook.on_search_tree_best_path("q1", "path");
        hook.on_task_map_step("q1", "step");
        hook.on_task_map_step("q1", "step2");
        assert_eq!(hook.best_path_calls.load(Ordering::SeqCst), 1);
        assert_eq!(hook.task_step_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn trait_object_dispatch() {
        // trait 对象动态分派验证（Box<dyn MemorySyncHook>）
        let hook: Box<dyn MemorySyncHook> = Box::new(CountingHook::default());
        hook.on_search_tree_best_path("q1", "path");
        hook.on_task_map_step("q1", "step");
        // 无 panic 即通过（trait 对象可正常分派）
    }
}
