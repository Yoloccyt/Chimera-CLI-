//! L2 记忆层协同接口集成测试 — MemorySyncHook 注入闭环（v3.4.0 §14 二次审查增强 Wave 3）
//!
//! 覆盖: MemorySyncHook 注入闭环（搜索树 best_path / 任务地图 step 同步）/
//! Noop 默认 / 自定义 hook 计数 / proptest hook 调用次数 = 状态变更次数

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, ExecutionStatus, ThreeFactorScore,
};
use nexus_contracts::{domain::Task, task::TaskStatus, ExperienceCard, Quest, ThinkingMode};
use proptest::prelude::*;
use quest_engine::{
    LongTaskMap, MemorySyncHook, NoopMemorySyncHook, SearchTreeManager, StepResult,
};

fn card(node_id: &str, parent_id: Option<&str>, score: f32) -> ExperienceCard {
    ExperienceCard {
        card_id: Box::from(format!("card_{node_id}")),
        task_id: Box::from("task-1"),
        node_id: Box::from(node_id),
        parent_id: parent_id.map(Box::from),
        created_at: Utc::now(),
        operator: AtomicOperator::Improve,
        score,
        delta_vs_parent: 0.1,
        method_family: Box::from("test"),
        error_signature: None,
        three_factor: ThreeFactorScore {
            quality: score,
            progress: 0.1,
            novelty: 0.5,
        },
        execution_status: ExecutionStatus::Success,
        token_evidence_ids: Vec::new(),
        segment_id: None,
        metadata: CardMetadata::default(),
    }
}

fn quest() -> Quest {
    Quest {
        quest_id: "q-1".to_string(),
        title: "任务".to_string(),
        tasks: vec![Task {
            task_id: "t-1".to_string(),
            description: "分析".to_string(),
            status: TaskStatus::Pending,
            dependencies: Vec::new(),
        }],
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
        priority: 128,
    }
}

fn step(state: &str) -> StepResult {
    StepResult {
        state: state.to_string(),
        detail: format!("{state} 详情"),
        next_action: "continue".to_string(),
        action: "analyze".to_string(),
        success: true,
    }
}

/// 记录型钩子 — 记录调用次数与最近摘要
#[derive(Debug, Default)]
struct RecordingHook {
    best_path_calls: AtomicUsize,
    task_step_calls: AtomicUsize,
    last_best_path: Mutex<String>,
    last_task_step: Mutex<String>,
}

impl MemorySyncHook for RecordingHook {
    fn on_search_tree_best_path(&self, _quest_id: &str, summary: &str) {
        self.best_path_calls.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut last) = self.last_best_path.lock() {
            *last = summary.to_string();
        }
    }

    fn on_task_map_step(&self, _quest_id: &str, summary: &str) {
        self.task_step_calls.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut last) = self.last_task_step.lock() {
            *last = summary.to_string();
        }
    }
}

// ----------------------------------------------------------
// SearchTreeManager hook 注入闭环
// ----------------------------------------------------------

#[test]
fn search_tree_hook_called_on_best_update() {
    let hook = Arc::new(RecordingHook::default());
    let mut tree = SearchTreeManager::new(10)
        .with_memory_sync_hook(Box::new(CountingWrapper(Arc::clone(&hook))));
    let root_id = tree.create_root("task-1");
    // expand 触发 best_node 更新 → hook 调用
    tree.expand_node(&root_id, card("n1", Some(root_id.as_str()), 0.8))
        .expect("扩展");
    assert_eq!(hook.best_path_calls.load(Ordering::SeqCst), 1);
    // best_path 摘要包含 method_family 链
    let last = hook.last_best_path.lock().unwrap().clone();
    assert!(
        last.contains("test"),
        "best_path 摘要应含 method_family: {last}"
    );
}

#[test]
fn search_tree_noop_hook_default() {
    // 未注入 hook → Noop，expand 不 panic
    let mut tree = SearchTreeManager::new(10);
    let root_id = tree.create_root("task-1");
    tree.expand_node(&root_id, card("n1", Some(root_id.as_str()), 0.8))
        .expect("扩展");
    // 无 panic 即通过
}

// ----------------------------------------------------------
// LongTaskMap hook 注入闭环
// ----------------------------------------------------------

#[test]
fn task_map_hook_called_on_record_step() {
    let hook = Arc::new(RecordingHook::default());
    let mut map =
        LongTaskMap::default().with_memory_sync_hook(Box::new(CountingWrapper(Arc::clone(&hook))));
    let map_ref = map.create_map(&quest());
    map.record_step(&map_ref, &step("步骤一"));
    map.record_step(&map_ref, &step("步骤二"));
    assert_eq!(hook.task_step_calls.load(Ordering::SeqCst), 2);
    let last = hook.last_task_step.lock().unwrap().clone();
    assert!(last.contains("步骤二"), "步骤摘要应含 state: {last}");
}

#[test]
fn task_map_noop_hook_default() {
    // 未注入 hook → Noop，record_step 不 panic
    let mut map = LongTaskMap::default();
    let map_ref = map.create_map(&quest());
    map.record_step(&map_ref, &step("步骤"));
    // 无 panic 即通过
}

#[test]
fn noop_hook_struct_accessible() {
    let hook = NoopMemorySyncHook;
    hook.on_search_tree_best_path("q1", "path");
    hook.on_task_map_step("q1", "step");
    // 无操作不 panic
}

/// Arc 包装器 — 使 Arc<RecordingHook> 可作为 Box<dyn MemorySyncHook>
#[derive(Debug)]
struct CountingWrapper(Arc<RecordingHook>);

impl MemorySyncHook for CountingWrapper {
    fn on_search_tree_best_path(&self, quest_id: &str, summary: &str) {
        self.0.on_search_tree_best_path(quest_id, summary);
    }
    fn on_task_map_step(&self, quest_id: &str, summary: &str) {
        self.0.on_task_map_step(quest_id, summary);
    }
}

// ----------------------------------------------------------
// proptest: hook 调用次数 = 状态变更次数
// ----------------------------------------------------------

proptest! {
    /// 任意步骤数：task_step hook 调用次数 = record_step 次数
    #[test]
    fn hook_call_count_equals_step_count(
        n_steps in 1usize..10,
    ) {
        let hook = Arc::new(RecordingHook::default());
        let mut map = LongTaskMap::default()
            .with_memory_sync_hook(Box::new(CountingWrapper(Arc::clone(&hook))));
        let map_ref = map.create_map(&quest());
        for i in 0..n_steps {
            map.record_step(&map_ref, &step(&format!("步骤{i}")));
        }
        prop_assert_eq!(hook.task_step_calls.load(Ordering::SeqCst), n_steps);
    }
}
