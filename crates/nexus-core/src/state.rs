//! NexusState — 全局运行时状态(线程安全)
//!
//! 对应架构层:L1 Core
//! 对应创新点:NexusState — 所有上层 crate 共享的全局状态快照
//!
//! # 设计决策(WHY)
//! - **`Arc<RwLock>`**:读多写少(快照哈希、查询 Quest 远比注册频繁),
//!   RwLock 允许并发读,提升吞吐
//! - **Clone 基于 Arc**:NexusState 克隆是廉价的 Arc 引用计数递增,
//!   可跨线程传递(如 Event Bus 回调、异步任务)
//! - **快照哈希确定性**:snapshot_hash 按 quest_id 排序后哈希,
//!   保证相同状态产生相同哈希(HashMap 迭代序不确定,需排序)
//!
//! # 架构红线
//! - 所有状态变更通过 Event Bus 广播(NexusStateChanged 事件)
//! - 上层 crate 不直接修改 NexusState,只通过事件订阅感知变更

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use sha2::{Digest, Sha256};

use crate::error::NexusError;
use crate::types::Quest;

/// NexusState 内部状态 — 不对外公开,仅通过 NexusState 方法访问
#[derive(Default)]
struct NexusStateInner {
    /// 活跃 Quest 映射(quest_id → Quest)
    active_quests: HashMap<String, Quest>,
    /// 全局预算(已消耗 token 数)
    global_budget: u64,
    /// 模型注册表快照(model_id 列表)
    model_registry_snapshot: Vec<String>,
}

/// 全局运行时状态 — 线程安全的共享状态容器
///
/// 基于 `Arc<RwLock<NexusStateInner>>`,支持并发读、互斥写。
/// 克隆是廉价的(Arc 引用计数),可跨线程传递。
#[derive(Clone)]
pub struct NexusState(Arc<RwLock<NexusStateInner>>);

impl NexusState {
    /// 创建空状态实例
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(NexusStateInner::default())))
    }

    /// 注册新 Quest — 重复 ID 返回 `QuestAlreadyExists`
    pub fn register_quest(&self, quest: Quest) -> Result<(), NexusError> {
        let mut inner = self.0.write().unwrap_or_else(|e| e.into_inner());
        if inner.active_quests.contains_key(&quest.quest_id) {
            return Err(NexusError::QuestAlreadyExists(quest.quest_id));
        }
        inner.active_quests.insert(quest.quest_id.clone(), quest);
        Ok(())
    }

    /// 更新 Quest 进度 — Quest 不存在返回 `QuestNotFound`
    ///
    /// WHY:Week 2 阶段仅校验 Quest 存在性,实际进度通过 Task.status
    /// 字段反映(由 GQEP 执行器更新)。此方法作为进度通知的校验入口,
    /// 后续阶段可扩展为触发 QuestProgressUpdated 事件。
    pub fn update_quest_progress(
        &self,
        quest_id: &str,
        _completed: u32,
        _total: u32,
    ) -> Result<(), NexusError> {
        let inner = self.0.read().unwrap_or_else(|e| e.into_inner());
        if !inner.active_quests.contains_key(quest_id) {
            return Err(NexusError::QuestNotFound(quest_id.to_string()));
        }
        Ok(())
    }

    /// 计算当前状态的 SHA-256 哈希(64 字符 hex)
    ///
    /// WHY:按 quest_id 排序后哈希,保证确定性(HashMap 迭代序不确定)。
    /// 用于 NexusStateChanged 事件的链式校验(prev_hash → state_hash)。
    pub fn snapshot_hash(&self) -> String {
        let inner = self.0.read().unwrap_or_else(|e| e.into_inner());

        let mut hasher = Sha256::new();

        // 按 quest_id 排序,保证哈希确定性
        let mut entries: Vec<_> = inner.active_quests.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));

        for (quest_id, quest) in &entries {
            hasher.update(quest_id.as_bytes());
            hasher.update(b"\x00");
            // 序列化 Quest 后哈希;序列化失败(理论不可能)用空串兜底
            let json = serde_json::to_string(quest).unwrap_or_default();
            hasher.update(json.as_bytes());
            hasher.update(b"\x00");
        }

        hasher.update(inner.global_budget.to_le_bytes());
        hasher.update(b"\x00");

        for model_id in &inner.model_registry_snapshot {
            hasher.update(model_id.as_bytes());
            hasher.update(b"\x00");
        }

        hex::encode(hasher.finalize())
    }

    /// 按 ID 获取 Quest 克隆(不存在返回 None)
    pub fn get_quest(&self, quest_id: &str) -> Option<Quest> {
        let inner = self.0.read().unwrap_or_else(|e| e.into_inner());
        inner.active_quests.get(quest_id).cloned()
    }

    /// 列出所有活跃 Quest(无序)
    pub fn list_quests(&self) -> Vec<Quest> {
        let inner = self.0.read().unwrap_or_else(|e| e.into_inner());
        inner.active_quests.values().cloned().collect()
    }

    /// 获取全局预算(已消耗 token 数)
    pub fn global_budget(&self) -> u64 {
        let inner = self.0.read().unwrap_or_else(|e| e.into_inner());
        inner.global_budget
    }

    /// 设置全局预算
    pub fn set_global_budget(&self, budget: u64) {
        let mut inner = self.0.write().unwrap_or_else(|e| e.into_inner());
        inner.global_budget = budget;
    }

    /// 获取模型注册表快照
    pub fn model_registry_snapshot(&self) -> Vec<String> {
        let inner = self.0.read().unwrap_or_else(|e| e.into_inner());
        inner.model_registry_snapshot.clone()
    }

    /// 设置模型注册表快照
    pub fn set_model_registry_snapshot(&self, models: Vec<String>) {
        let mut inner = self.0.write().unwrap_or_else(|e| e.into_inner());
        inner.model_registry_snapshot = models;
    }
}

impl Default for NexusState {
    fn default() -> Self {
        Self::new()
    }
}

/// 显式标注 Send + Sync — NexusState 可跨线程共享
///
/// WHY:Arc<RwLock<T>> 自动满足 Send + Sync(当 T: Send + Sync),
/// 此标注作为编译期断言,防止未来修改破坏线程安全契约。
// 静态断言无需运行时开销
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<NexusState>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Task, TaskStatus, ThinkingMode};

    fn make_quest(id: &str) -> Quest {
        Quest {
            quest_id: id.to_string(),
            title: format!("Test Quest {id}"),
            tasks: vec![Task {
                task_id: "t1".into(),
                description: "test task".into(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            }],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        }
    }

    #[test]
    fn test_new_state_is_empty() {
        let state = NexusState::new();
        assert!(state.list_quests().is_empty());
        assert_eq!(state.global_budget(), 0);
        assert!(state.model_registry_snapshot().is_empty());
    }

    #[test]
    fn test_register_and_get_quest() {
        let state = NexusState::new();
        let quest = make_quest("q1");
        state.register_quest(quest.clone()).unwrap();

        let retrieved = state.get_quest("q1").unwrap();
        assert_eq!(retrieved.quest_id, "q1");
        assert_eq!(retrieved.title, "Test Quest q1");
    }

    #[test]
    fn test_register_duplicate_quest() {
        let state = NexusState::new();
        let quest = make_quest("q1");
        state.register_quest(quest).unwrap();

        let duplicate = make_quest("q1");
        let result = state.register_quest(duplicate);
        assert!(matches!(result, Err(NexusError::QuestAlreadyExists(_))));
    }

    #[test]
    fn test_update_progress_nonexistent() {
        let state = NexusState::new();
        let result = state.update_quest_progress("nonexistent", 0, 10);
        assert!(matches!(result, Err(NexusError::QuestNotFound(_))));
    }

    #[test]
    fn test_update_progress_existing() {
        let state = NexusState::new();
        state.register_quest(make_quest("q1")).unwrap();
        let result = state.update_quest_progress("q1", 5, 10);
        assert!(result.is_ok());
    }

    #[test]
    fn test_snapshot_hash_is_64_hex() {
        let state = NexusState::new();
        let hash = state.snapshot_hash();
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_snapshot_hash_changes_on_mutation() {
        let state = NexusState::new();
        let hash1 = state.snapshot_hash();

        state.register_quest(make_quest("q1")).unwrap();
        let hash2 = state.snapshot_hash();

        assert_ne!(hash1, hash2, "hash must change after registering quest");
    }

    #[test]
    fn test_snapshot_hash_deterministic() {
        let state1 = NexusState::new();
        state1.register_quest(make_quest("q1")).unwrap();
        state1.register_quest(make_quest("q2")).unwrap();

        let state2 = NexusState::new();
        state2.register_quest(make_quest("q2")).unwrap();
        state2.register_quest(make_quest("q1")).unwrap();

        // 插入顺序不同,但状态相同 → 哈希应一致(排序保证确定性)
        assert_eq!(state1.snapshot_hash(), state2.snapshot_hash());
    }

    #[test]
    fn test_concurrent_register_quest() {
        let state = NexusState::new();
        let state1 = state.clone();
        let state2 = state.clone();

        let quest_id = "concurrent-quest".to_string();
        let quest_id2 = quest_id.clone();

        let h1 = std::thread::spawn(move || state1.register_quest(make_quest(&quest_id)));
        let h2 = std::thread::spawn(move || state2.register_quest(make_quest(&quest_id2)));

        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();

        // 恰好一个成功,另一个返回 QuestAlreadyExists
        let ok_count = [&r1, &r2].iter().filter(|r| r.is_ok()).count();
        assert_eq!(ok_count, 1, "exactly one thread should succeed");

        if let Err(e) = &r1 {
            assert!(
                matches!(e, NexusError::QuestAlreadyExists(_)),
                "expected QuestAlreadyExists, got {e:?}"
            );
        }
        if let Err(e) = &r2 {
            assert!(
                matches!(e, NexusError::QuestAlreadyExists(_)),
                "expected QuestAlreadyExists, got {e:?}"
            );
        }
    }

    #[test]
    fn test_list_quests() {
        let state = NexusState::new();
        state.register_quest(make_quest("q1")).unwrap();
        state.register_quest(make_quest("q2")).unwrap();

        let mut ids: Vec<_> = state
            .list_quests()
            .into_iter()
            .map(|q| q.quest_id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["q1", "q2"]);
    }
}
