//! 长任务地图集成测试 — Quest 消费 + 外置存储闭环（v3.4.0 §14.2）
//!
//! 覆盖: 顶层 API 可达性（re-export 验证）/ Quest 消费（title/tasks 偏差适配）/
//! 外置存储往返 / 地图注入端到端 / ExternalStorage trait 注入 mock /
//! proptest 摘要钳制不变量

#![forbid(unsafe_code)]

use nexus_contracts::domain::Task;
use nexus_contracts::task::TaskStatus;
use nexus_contracts::{Quest, ThinkingMode};
use proptest::prelude::*;
use quest_engine::{
    ExternalStorage, InMemoryExternalStorage, LongTaskMap, NodeStatus, StepResult, TaskMapRef,
};

fn quest(title: &str) -> Quest {
    Quest {
        quest_id: "q-1".to_string(),
        title: title.to_string(),
        tasks: vec![Task {
            task_id: "t-1".to_string(),
            description: "分析现有实现".to_string(),
            status: TaskStatus::Pending,
            dependencies: Vec::new(),
        }],
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
        priority: 128,
    }
}

fn step(state: &str, success: bool) -> StepResult {
    StepResult {
        state: state.to_string(),
        detail: format!("{state} 的详细记录"),
        next_action: "continue".to_string(),
        action: "analyze".to_string(),
        success,
    }
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let map = LongTaskMap::default();
    assert_eq!(map.node_count(), 0);
    assert_eq!(map.edge_count(), 0);
}

// ----------------------------------------------------------
// Quest 消费闭环（title/tasks 偏差适配）
// ----------------------------------------------------------

#[test]
fn quest_consumption_closed_loop() {
    let mut map = LongTaskMap::default();
    let map_ref: TaskMapRef = map.create_map(&quest("重构登录模块"));
    assert_eq!(map_ref.root_id, "root");
    // root 摘要 = title
    let root = map.get_node("root").expect("root 存在");
    assert_eq!(root.state_summary, "重构登录模块");
    // 详情外置包含 title + tasks 描述（偏差适配 1）
    let detail = map.retrieve_detail(&root.detail_ref).expect("详情存在");
    assert!(detail.contains("重构登录模块"));
    assert!(detail.contains("分析现有实现"));
}

// ----------------------------------------------------------
// 地图注入端到端
// ----------------------------------------------------------

#[test]
fn inject_map_end_to_end() {
    let mut map = LongTaskMap::default();
    let map_ref = map.create_map(&quest("重构"));
    map.record_step(&map_ref, &step("分析完成", true));
    map.record_step(&map_ref, &step("重构失败", false));
    let mut context = "初始".to_string();
    map.inject_map_to_context(&map_ref, &mut context);
    assert!(context.contains("[任务地图]"));
    assert!(context.contains("[0] 重构 → start"));
    assert!(context.contains("[1] 分析完成 → continue"));
    assert!(context.contains("[2] 重构失败 → continue"));
    // 状态映射（成功/失败）
    assert_eq!(
        map.get_node("node_1").unwrap().status,
        NodeStatus::Completed
    );
    assert_eq!(map.get_node("node_2").unwrap().status, NodeStatus::Failed);
}

// ----------------------------------------------------------
// ExternalStorage trait 注入 mock（D-4）
// ----------------------------------------------------------

#[test]
fn external_storage_mock_injection() {
    struct CountingStorage {
        inner: InMemoryExternalStorage,
        stores: std::sync::atomic::AtomicU32,
    }
    impl ExternalStorage for CountingStorage {
        fn store(&self, detail: &str) -> String {
            self.stores
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.store(detail)
        }
        fn retrieve(&self, ref_id: &str) -> Option<String> {
            self.inner.retrieve(ref_id)
        }
    }
    let mut map = LongTaskMap::new(Box::new(CountingStorage {
        inner: InMemoryExternalStorage::default(),
        stores: std::sync::atomic::AtomicU32::new(0),
    }));
    let map_ref = map.create_map(&quest("t"));
    map.record_step(&map_ref, &step("s1", true));
    // root + 1 step = 2 次 store 调用
    assert_eq!(map.node_count(), 2);
    let node = map.get_node("node_1").expect("节点存在");
    assert_eq!(
        map.retrieve_detail(&node.detail_ref),
        Some("s1 的详细记录".to_string())
    );
}

// ----------------------------------------------------------
// proptest：摘要钳制不变量
// ----------------------------------------------------------

proptest! {
    /// 任意长度状态文本: record_step 后摘要长度有界；地图注入保留全部步骤行
    #[test]
    fn summary_bounded_and_inject_complete(
        state_len in 0usize..300,
    ) {
        let mut map = LongTaskMap::default();
        let map_ref = map.create_map(&quest("t"));
        let long_state = "x".repeat(state_len);
        map.record_step(&map_ref, &step(&long_state, true));
        let node = map.get_node("node_1").expect("节点存在");
        // 摘要钳制：≤ 80 字符 + 省略号
        prop_assert!(node.state_summary.len() <= 80 * 4 + 3, "UTF-8 安全上限");
        if state_len > 80 {
            prop_assert!(node.state_summary.ends_with("..."), "超长应省略");
        } else {
            prop_assert!(!node.state_summary.ends_with("..."), "短文本不省略");
        }
        // 地图注入包含全部步骤行（root + 1 step）
        let mut context = String::new();
        map.inject_map_to_context(&map_ref, &mut context);
        prop_assert!(context.contains("[1]"));
        // 详情外置完整保留（不因摘要截断而丢失）
        let detail = map.retrieve_detail(&node.detail_ref).expect("详情存在");
        prop_assert_eq!(detail.len(), long_state.len() + " 的详细记录".len());
    }
}
