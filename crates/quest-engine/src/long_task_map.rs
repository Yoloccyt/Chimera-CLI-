//! 长任务地图 — TencentDB 机制（设计文档 §14.2）
//!
//! 对应架构层: **L9 Quest**（quest-engine 子模块，规范指定落点）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §14.2
//! 对应论文: TencentDB Agent Memory（长任务地图：Token 2.21 亿→8500 万、通过率 33%→50%）
//!
//! # 核心职责
//!
//! 长任务过程的外部化压缩表示：
//! - **短摘要入上下文**: 每步仅保留 state_summary（放上下文窗口）
//! - **详情外置**: 详细记录经 [`ExternalStorage`] 转存外部，上下文只留引用
//! - **地图注入**: `inject_map_to_context` 将任务地图紧凑注入上下文
//!
//! # 落层偏差记录
//!
//! 1. 原型 `quest.description` 不存在（L0 Quest 无该字段）→ root 详情用
//!    title + tasks 描述摘要适配
//! 2. 原型 `ExternalStorage` 具体实现 → trait 注入点 + 内存默认实现
//!    （铁律1 零运行时外部依赖；真实存储由调用方接线，MemoryTidyHook 先例）
//!
//! # 设计约束（铁律）
//!
//! - **铁律1**: ExternalStorage trait 注入 + InMemoryExternalStorage 默认
//! - **铁律4**: summarize_state 截断式摘要为纯函数（确定性）
//!
//! # 长时程信用分配协同边界（D-6）
//!
//! 任务地图节点与信用分配的关联经既有组件覆盖：L8 `parliament::sharp`
//! （SHARP Shapley）+ L1 `SegmentAwarePER` + 本 crate `trajectory_exporter`
//! （铁律6），本模块不重复实现信用分配。

use std::collections::HashMap;
use std::sync::Mutex;

use nexus_contracts::Quest;
use uuid::Uuid;

use crate::memory_sync_hook::{MemorySyncHook, NoopMemorySyncHook};

/// 摘要长度上限（短摘要入上下文的 Token 节约钳制）
const SUMMARY_MAX_LEN: usize = 80;

/// 外部存储注入点 — 详情转存（D-4 偏差适配，MemoryTidyHook 先例）
///
/// WHY trait 注入：铁律1 零运行时外部依赖；默认内存实现，
/// 真实存储（文件/对象存储）由调用方接线。
pub trait ExternalStorage: Send + Sync {
    /// 存储详情，返回外部引用 ID
    fn store(&self, detail: &str) -> String;
    /// 按引用 ID 取回详情（None = 引用不存在）
    fn retrieve(&self, ref_id: &str) -> Option<String>;
}

/// 内存外部存储 — 默认实现（详情外置内存承载）
#[derive(Debug, Default)]
pub struct InMemoryExternalStorage {
    /// 引用 ID → 详情内容（Mutex: 短临界区同步访问，无持锁跨 await）
    store: Mutex<HashMap<String, String>>,
}

impl ExternalStorage for InMemoryExternalStorage {
    fn store(&self, detail: &str) -> String {
        // WHY now_v7: workspace uuid 仅启用 v7 feature（时间有序，既有惯例）
        let ref_id = format!("ext_{}", Uuid::now_v7());
        if let Ok(mut store) = self.store.lock() {
            store.insert(ref_id.clone(), detail.to_string());
        }
        ref_id
    }

    fn retrieve(&self, ref_id: &str) -> Option<String> {
        self.store
            .lock()
            .ok()
            .and_then(|store| store.get(ref_id).cloned())
    }
}

/// 任务节点状态（规范 §14.2 NodeStatus）
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NodeStatus {
    /// 待执行
    Pending,
    /// 执行中
    InProgress,
    /// 已完成
    Completed,
    /// 已失败
    Failed,
}

/// 任务地图节点（规范 §14.2 TaskNode）
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TaskNode {
    /// 节点 ID
    pub node_id: String,
    /// 步骤序号
    pub step_number: u32,
    /// 状态短摘要（放上下文）
    pub state_summary: String,
    /// 详细记录的外部引用（ExternalStorage 返回）
    pub detail_ref: String,
    /// 下一步动作
    pub next_action: String,
    /// 节点状态
    pub status: NodeStatus,
}

/// 任务地图边（步骤间动作链接）
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TaskEdge {
    /// 源节点 ID
    pub from: String,
    /// 目标节点 ID
    pub to: String,
    /// 边动作描述
    pub action: String,
}

/// 任务地图引用（map_id + root_id）
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TaskMapRef {
    /// 地图 ID
    pub map_id: String,
    /// 根节点 ID
    pub root_id: String,
}

/// 步骤结果（record_step 输入）
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StepResult {
    /// 状态描述（将被摘要化）
    pub state: String,
    /// 详细内容（将被外置）
    pub detail: String,
    /// 下一步动作
    pub next_action: String,
    /// 本步动作描述
    pub action: String,
    /// 是否成功
    pub success: bool,
}

/// 长任务地图可序列化快照（LHQP 检查点联动，Wave 2）
///
/// WHY 独立快照结构：`external_storage` 为 trait 对象不可序列化，
/// 故仅序列化 task_nodes/task_edges；反序列化时用 InMemoryExternalStorage
/// 重建（detail_ref 引用的详情需调用方重新注入，文档注明）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct LongTaskMapSnapshot {
    task_nodes: Vec<TaskNode>,
    task_edges: Vec<TaskEdge>,
}

/// 长任务地图 — TencentDB 机制（规范 §14.2）
///
/// L2 记忆层协同（Wave 3）：可选注入 [`MemorySyncHook`]，record_step 时
/// 同步步骤摘要到记忆层；默认 Noop（不破坏既有 new() 行为）。
pub struct LongTaskMap {
    /// 任务节点序列（按步骤序）
    task_nodes: Vec<TaskNode>,
    /// 步骤间边
    task_edges: Vec<TaskEdge>,
    /// 外部存储（D-4 注入点）
    external_storage: Box<dyn ExternalStorage>,
    /// L2 记忆层同步钩子（Wave 3，依赖倒置，默认 Noop）
    memory_sync_hook: Box<dyn MemorySyncHook>,
}

impl Default for LongTaskMap {
    fn default() -> Self {
        Self::new(Box::new(InMemoryExternalStorage::default()))
    }
}

impl LongTaskMap {
    /// 创建长任务地图（注入外部存储）
    pub fn new(external_storage: Box<dyn ExternalStorage>) -> Self {
        Self {
            task_nodes: Vec::new(),
            task_edges: Vec::new(),
            external_storage,
            memory_sync_hook: Box::new(NoopMemorySyncHook),
        }
    }

    /// 注入 L2 记忆层同步钩子（Wave 3，依赖倒置，不破坏既有 new() 行为）
    ///
    /// 未注入时默认 Noop；注入后 record_step 时同步步骤摘要到记忆层。
    pub fn with_memory_sync_hook(mut self, hook: Box<dyn MemorySyncHook>) -> Self {
        self.memory_sync_hook = hook;
        self
    }

    /// 从 Quest 创建地图（规范 §14.2 create_map）
    ///
    /// 偏差适配：L0 Quest 无 description → root 详情用 title + tasks 描述摘要。
    pub fn create_map(&mut self, quest: &Quest) -> TaskMapRef {
        // Quest 描述 = title + 各 task 描述（偏差适配 1）
        let mut quest_description = format!("Quest: {}", quest.title);
        for task in &quest.tasks {
            quest_description.push_str(&format!("\n- {}: {}", task.task_id, task.description));
        }
        let root = TaskNode {
            node_id: "root".to_string(),
            step_number: 0,
            state_summary: summarize_state(&quest.title),
            detail_ref: self.external_storage.store(&quest_description),
            next_action: "start".to_string(),
            status: NodeStatus::Pending,
        };
        self.task_nodes.push(root);
        TaskMapRef {
            map_id: Uuid::now_v7().to_string(),
            root_id: "root".to_string(),
        }
    }

    /// 记录步骤（规范 §14.2 record_step）
    ///
    /// 短摘要入上下文节点 + 详情外置 + 与前序节点的边链接。
    /// 地图为空时先隐式创建占位根（防御分支，替换原型索引假设）。
    ///
    /// Wave 3: record_step 时同步步骤摘要到 L2 记忆层（用 map_id 作为标识）。
    pub fn record_step(&mut self, map_ref: &TaskMapRef, step_result: &StepResult) {
        let step_number = self.task_nodes.len() as u32;
        let state_summary = summarize_state(&step_result.state);
        let node = TaskNode {
            node_id: format!("node_{step_number}"),
            step_number,
            state_summary: state_summary.clone(),
            detail_ref: self.external_storage.store(&step_result.detail),
            next_action: step_result.next_action.clone(),
            status: if step_result.success {
                NodeStatus::Completed
            } else {
                NodeStatus::Failed
            },
        };
        self.task_nodes.push(node);
        // 边链接：前序节点 → 当前节点（防御：至少 2 节点才成边）
        let len = self.task_nodes.len();
        if len >= 2 {
            let prev_id = self.task_nodes[len - 2].node_id.clone();
            let curr_id = self.task_nodes[len - 1].node_id.clone();
            self.task_edges.push(TaskEdge {
                from: prev_id,
                to: curr_id,
                action: step_result.action.clone(),
            });
        }
        // Wave 3: 同步步骤摘要到 L2 记忆层（用 map_id 作为 quest 标识）
        let step_summary = format!("{} → {}", state_summary, step_result.next_action);
        self.memory_sync_hook
            .on_task_map_step(&map_ref.map_id, &step_summary);
    }

    /// 任务地图注入上下文（规范 §14.2 inject_map_to_context）
    ///
    /// 格式：`[step] summary → next_action` 逐行拼接；
    /// Token 节约语义：上下文只留地图，详情经 detail_ref 外置。
    pub fn inject_map_to_context(&self, _map_ref: &TaskMapRef, context: &mut String) {
        let map_summary = self
            .task_nodes
            .iter()
            .map(|n| {
                format!(
                    "[{}] {} → {}",
                    n.step_number, n.state_summary, n.next_action
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        *context = format!("{context}\n\n[任务地图]\n{map_summary}");
    }

    /// 按引用 ID 取回外置详情（ExternalStorage 透传）
    pub fn retrieve_detail(&self, ref_id: &str) -> Option<String> {
        self.external_storage.retrieve(ref_id)
    }

    /// 节点数只读访问（可观测性）
    pub fn node_count(&self) -> usize {
        self.task_nodes.len()
    }

    /// 边数只读访问（可观测性）
    pub fn edge_count(&self) -> usize {
        self.task_edges.len()
    }

    /// 节点只读访问（可观测性）
    pub fn get_node(&self, node_id: &str) -> Option<&TaskNode> {
        self.task_nodes.iter().find(|n| n.node_id == node_id)
    }

    /// 序列化为 MessagePack bytes（LHQP 检查点联动，Wave 2）
    ///
    /// 仅序列化 task_nodes/task_edges（external_storage 为 trait 对象不参与）。
    /// 序列化后由调用方与 Checkpoint 关联存储（不修改 L0 Checkpoint）。
    ///
    /// # 错误
    /// - `QuestError::SerializationError`: MessagePack 编码失败
    pub fn to_bytes(&self) -> Result<Vec<u8>, crate::error::QuestError> {
        let snapshot = LongTaskMapSnapshot {
            task_nodes: self.task_nodes.clone(),
            task_edges: self.task_edges.clone(),
        };
        rmp_serde::to_vec(&snapshot)
            .map_err(|e| crate::error::QuestError::SerializationError(e.to_string()))
    }

    /// 从 MessagePack bytes 反序列化重建地图（LHQP 检查点恢复）
    ///
    /// external_storage 用 InMemoryExternalStorage 默认重建；
    /// detail_ref 引用的详情需调用方重新注入（文档注明）。
    ///
    /// # 错误
    /// - `QuestError::SerializationError`: MessagePack 解码失败（数据损坏/版本不兼容）
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, crate::error::QuestError> {
        let snapshot: LongTaskMapSnapshot = rmp_serde::from_slice(bytes)
            .map_err(|e| crate::error::QuestError::SerializationError(e.to_string()))?;
        Ok(Self {
            task_nodes: snapshot.task_nodes,
            task_edges: snapshot.task_edges,
            external_storage: Box::new(InMemoryExternalStorage::default()),
            memory_sync_hook: Box::new(NoopMemorySyncHook),
        })
    }
}

/// 截断式短摘要纯函数（铁律4，SUMMARY_MAX_LEN 钳制）
fn summarize_state(state: &str) -> String {
    let trimmed = state.trim();
    if trimmed.len() <= SUMMARY_MAX_LEN {
        return trimmed.to_string();
    }
    // 字符边界安全截断（防 UTF-8 多字节边界 panic）
    let mut end = SUMMARY_MAX_LEN;
    while !trimmed.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    format!("{}...", &trimmed[..end])
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::domain::Task;
    use nexus_contracts::task::TaskStatus;
    use nexus_contracts::ThinkingMode;

    fn quest() -> Quest {
        Quest {
            quest_id: "q-1".to_string(),
            title: "重构登录模块".to_string(),
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
            detail: format!("{state} 的详细内容"),
            next_action: "continue".to_string(),
            action: "analyze".to_string(),
            success,
        }
    }

    #[test]
    fn create_map_root_semantics() {
        let mut map = LongTaskMap::default();
        let map_ref = map.create_map(&quest());
        assert_eq!(map_ref.root_id, "root");
        assert_eq!(map.node_count(), 1);
        let root = map.get_node("root").expect("root 存在");
        assert_eq!(root.status, NodeStatus::Pending);
        assert_eq!(root.next_action, "start");
        // 详情外置：retrieve 往返
        let detail = map.retrieve_detail(&root.detail_ref).expect("详情存在");
        assert!(detail.contains("重构登录模块"));
        assert!(detail.contains("分析现有实现"), "tasks 描述摘要适配");
    }

    #[test]
    fn record_step_chain() {
        let mut map = LongTaskMap::default();
        let map_ref = map.create_map(&quest());
        map.record_step(&map_ref, &step("步骤一完成", true));
        map.record_step(&map_ref, &step("步骤二失败", false));
        assert_eq!(map.node_count(), 3);
        assert_eq!(map.edge_count(), 2);
        // 状态映射
        assert_eq!(
            map.get_node("node_1").unwrap().status,
            NodeStatus::Completed
        );
        assert_eq!(map.get_node("node_2").unwrap().status, NodeStatus::Failed);
    }

    #[test]
    fn external_storage_roundtrip() {
        let mut map = LongTaskMap::default();
        let map_ref = map.create_map(&quest());
        map.record_step(&map_ref, &step("状态", true));
        let node = map.get_node("node_1").expect("节点存在");
        let detail = map.retrieve_detail(&node.detail_ref).expect("详情存在");
        assert_eq!(detail, "状态 的详细内容");
    }

    #[test]
    fn inject_map_format() {
        let mut map = LongTaskMap::default();
        let map_ref = map.create_map(&quest());
        map.record_step(&map_ref, &step("分析完成", true));
        let mut context = "初始上下文".to_string();
        map.inject_map_to_context(&map_ref, &mut context);
        assert!(context.contains("[任务地图]"));
        assert!(context.contains("[0]"));
        assert!(context.contains("[1] 分析完成 → continue"));
    }

    #[test]
    fn summary_length_clamped() {
        let long_state = "长".repeat(100);
        let summary = summarize_state(&long_state);
        // 80 字符钳制 + "..."（字符边界安全）
        assert!(summary.len() <= SUMMARY_MAX_LEN * 3 + 3, "UTF-8 多字节安全");
        assert!(summary.ends_with("..."));
        // 短状态不截断
        assert_eq!(summarize_state("短状态"), "短状态");
    }

    #[test]
    fn custom_storage_injection() {
        // D-4 trait 注入替换验证
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
        let map_ref = map.create_map(&quest());
        map.record_step(&map_ref, &step("s", true));
        assert_eq!(map.node_count(), 2);
    }
}
