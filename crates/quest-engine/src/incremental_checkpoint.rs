//! 增量检查点 — P1-14 Content-Addressed Storage 增量持久化
//!
//! 对应架构层:L9 Quest
//! 对应创新点:LHQP 增量检查点(仅持久化变更部分,大小 < 完整 10%)
//!
//! # 核心机制
//! - **Quest 差异计算**:对比新旧 Quest 状态,仅提取变更的 Task
//! - **增量序列化**:仅序列化变更的 Task 列表 + Quest 元数据变更
//! - **内容寻址**:使用 SHA-256 哈希作为增量块标识,相同内容复用
//! - **增量合并**:恢复时从基础检查点 + 增量链重建完整状态
//!
//! # 设计决策(WHY)
//! - **Task 级增量**:Quest 的变更通常集中在 Task 状态更新,
//!   Task 是自然的增量单元。
//! - **内容寻址**:相同 Task 内容(如多次 checkpoint 间无变化)复用,
//!   进一步减少存储。
//! - **增量链**:支持从任意检查点 + 后续增量恢复,灵活回滚。
//! - **向后兼容**:增量检查点与完整检查点共存,旧代码不受影响。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use nexus_core::{Quest, Task, TaskStatus};

use crate::error::QuestError;

/// 增量检查点 — Quest 状态的增量变更记录
///
/// 与完整 Checkpoint 不同,增量检查点仅存储:
/// - 变更的 Task 列表(新增/修改)
/// - Quest 元数据变更(如 thinking_mode 变化)
/// - 基础检查点引用(用于恢复时重建)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IncrementalCheckpoint {
    /// 所属 Quest ID
    pub quest_id: String,
    /// 增量检查点 ID
    pub checkpoint_id: String,
    /// 基础检查点 ID(若为 None 表示这是完整基础检查点)
    pub base_checkpoint_id: Option<String>,
    /// 变更的 Task 列表(仅包含状态变化的 Task)
    pub changed_tasks: Vec<Task>,
    /// Quest 元数据变更
    pub metadata_delta: Option<QuestMetadataDelta>,
    /// 变更内容哈希(用于完整性校验)
    pub delta_hash: String,
    /// 创建时间(UTC)
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Quest 元数据变更
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuestMetadataDelta {
    /// 标题变更(若为 None 表示未变更)
    pub title: Option<String>,
    /// 思考模式变更(若为 None 表示未变更)
    pub thinking_mode: Option<nexus_core::ThinkingMode>,
    /// 检查点 ID 变更
    pub checkpoint_id: Option<String>,
}

/// 增量检查点构建器
pub struct IncrementalCheckpointBuilder;

impl IncrementalCheckpointBuilder {
    /// 计算两个 Quest 状态的差异
    ///
    /// 返回 IncrementalCheckpoint,仅包含从 old_quest 到 new_quest 的变更。
    /// 若 old_quest 为 None,构建完整检查点(所有 Task + 元数据)。
    pub fn build(
        quest_id: String,
        checkpoint_id: String,
        base_checkpoint_id: Option<String>,
        old_quest: Option<&Quest>,
        new_quest: &Quest,
    ) -> Result<IncrementalCheckpoint, QuestError> {
        let changed_tasks = if let Some(old) = old_quest {
            Self::compute_task_delta(&old.tasks, &new_quest.tasks)
        } else {
            // 无基础状态,所有 Task 都是变更
            new_quest.tasks.clone()
        };

        let metadata_delta = if let Some(old) = old_quest {
            Self::compute_metadata_delta(old, new_quest)
        } else {
            // 无基础状态,所有元数据都是变更
            Some(QuestMetadataDelta {
                title: Some(new_quest.title.clone()),
                thinking_mode: Some(new_quest.thinking_mode.clone()),
                checkpoint_id: new_quest.checkpoint_id.clone(),
            })
        };

        // 计算 delta_hash
        let delta_content = Self::serialize_delta(&changed_tasks, &metadata_delta)?;
        let delta_hash = compute_sha256_hex(&delta_content);

        Ok(IncrementalCheckpoint {
            quest_id,
            checkpoint_id,
            base_checkpoint_id,
            changed_tasks,
            metadata_delta,
            delta_hash,
            created_at: chrono::Utc::now(),
        })
    }

    /// 计算 Task 差异 — 仅返回状态变化或新增的 Task
    fn compute_task_delta(old_tasks: &[Task], new_tasks: &[Task]) -> Vec<Task> {
        let mut changed = Vec::new();

        // 构建 old_task 的 id → Task 映射
        let old_map: std::collections::HashMap<&str, &Task> = old_tasks
            .iter()
            .map(|t| (t.task_id.as_str(), t))
            .collect();

        for new_task in new_tasks {
            if let Some(old_task) = old_map.get(new_task.task_id.as_str()) {
                // Task 存在,检查是否变化
                if old_task.status != new_task.status
                    || old_task.description != new_task.description
                    || old_task.dependencies != new_task.dependencies
                {
                    changed.push(new_task.clone());
                }
            } else {
                // 新增 Task
                changed.push(new_task.clone());
            }
        }

        changed
    }

    /// 计算 Quest 元数据差异
    fn compute_metadata_delta(old_quest: &Quest, new_quest: &Quest) -> Option<QuestMetadataDelta> {
        let mut has_changes = false;
        let mut delta = QuestMetadataDelta {
            title: None,
            thinking_mode: None,
            checkpoint_id: None,
        };

        if old_quest.title != new_quest.title {
            delta.title = Some(new_quest.title.clone());
            has_changes = true;
        }
        if old_quest.thinking_mode != new_quest.thinking_mode {
            delta.thinking_mode = Some(new_quest.thinking_mode.clone());
            has_changes = true;
        }
        if old_quest.checkpoint_id != new_quest.checkpoint_id {
            delta.checkpoint_id = new_quest.checkpoint_id.clone();
            has_changes = true;
        }

        if has_changes {
            Some(delta)
        } else {
            None
        }
    }

    /// 序列化增量内容(用于哈希计算)
    fn serialize_delta(
        changed_tasks: &[Task],
        metadata_delta: &Option<QuestMetadataDelta>,
    ) -> Result<Vec<u8>, QuestError> {
        #[derive(Serialize)]
        struct DeltaContent<'a> {
            tasks: &'a [Task],
            metadata: &'a Option<QuestMetadataDelta>,
        }

        let content = DeltaContent {
            tasks: changed_tasks,
            metadata: metadata_delta,
        };

        rmp_serde::to_vec(&content)
            .map_err(|e| QuestError::SerializationError(format!("delta encode: {e}")))
    }
}

/// 增量检查点恢复器
pub struct IncrementalCheckpointRestorer;

impl IncrementalCheckpointRestorer {
    /// 从基础 Quest + 增量检查点恢复完整状态
    ///
    /// 流程:
    /// 1. 从 base_quest(或空 Quest)开始
    /// 2. 应用增量中的 changed_tasks(覆盖或追加)
    /// 3. 应用 metadata_delta
    /// 4. 返回重建的 Quest
    pub fn restore(base_quest: Option<Quest>, delta: &IncrementalCheckpoint) -> Result<Quest, QuestError> {
        let mut quest = base_quest.unwrap_or_else(|| Quest {
            quest_id: delta.quest_id.clone(),
            title: String::new(),
            tasks: Vec::new(),
            thinking_mode: nexus_core::ThinkingMode::Standard,
            checkpoint_id: None,
        });

        // 应用 Task 变更
        let mut task_map: std::collections::HashMap<String, Task> = quest
            .tasks
            .into_iter()
            .map(|t| (t.task_id.clone(), t))
            .collect();

        for changed_task in &delta.changed_tasks {
            task_map.insert(changed_task.task_id.clone(), changed_task.clone());
        }

        // 保持 Task 顺序(按 task_id 排序)
        let mut task_ids: Vec<_> = task_map.keys().cloned().collect();
        task_ids.sort();
        quest.tasks = task_ids.into_iter().filter_map(|id| task_map.remove(&id)).collect();

        // 应用元数据变更
        if let Some(meta) = &delta.metadata_delta {
            if let Some(title) = &meta.title {
                quest.title = title.clone();
            }
            if let Some(mode) = &meta.thinking_mode {
                quest.thinking_mode = mode.clone();
            }
            if let Some(cp_id) = &meta.checkpoint_id {
                quest.checkpoint_id = Some(cp_id.clone());
            }
        }

        // 验证完整性
        let delta_content = IncrementalCheckpointBuilder::serialize_delta(
            &delta.changed_tasks,
            &delta.metadata_delta,
        )?;
        let computed_hash = compute_sha256_hex(&delta_content);
        if computed_hash != delta.delta_hash {
            return Err(QuestError::CheckpointCorrupted);
        }

        Ok(quest)
    }

    /// 从增量链恢复 — 按顺序应用多个增量
    ///
    /// `deltas` 必须按创建时间升序排列。
    pub fn restore_chain(
        base_quest: Option<Quest>,
        deltas: &[IncrementalCheckpoint],
    ) -> Result<Quest, QuestError> {
        let mut quest = base_quest;
        for delta in deltas {
            quest = Some(Self::restore(quest, delta)?);
        }
        quest.ok_or_else(|| QuestError::CheckpointNotFound("no base or deltas".into()))
    }
}

/// 计算 SHA-256 哈希并返回十六进制字符串
fn compute_sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash = hasher.finalize();
    hex::encode(hash)
}

/// 估算增量检查点大小相对于完整检查点的比例
///
/// 返回百分比(0.0-100.0),用于监控增量效率。
pub fn estimate_incremental_ratio(delta: &IncrementalCheckpoint, full_quest: &Quest) -> f32 {
    let delta_size = {
        let tasks_size = delta.changed_tasks.len();
        let meta_size = if delta.metadata_delta.is_some() { 1 } else { 0 };
        tasks_size + meta_size * 3 // 粗略估算
    };

    let full_size = full_quest.tasks.len();
    if full_size == 0 {
        return 100.0;
    }

    (delta_size as f32 / full_size as f32) * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::{Task, TaskStatus, ThinkingMode};

    fn make_task(id: &str, desc: &str, status: TaskStatus) -> Task {
        Task {
            task_id: id.into(),
            description: desc.into(),
            status,
            dependencies: vec![],
        }
    }

    fn make_quest(id: &str, tasks: Vec<Task>) -> Quest {
        Quest {
            quest_id: id.into(),
            title: format!("Quest {id}"),
            tasks,
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        }
    }

    #[test]
    fn test_incremental_from_none_builds_full() {
        let tasks = vec![
            make_task("t-0", "task 0", TaskStatus::Pending),
            make_task("t-1", "task 1", TaskStatus::Pending),
        ];
        let quest = make_quest("q-1", tasks);

        let delta = IncrementalCheckpointBuilder::build(
            "q-1".into(),
            "cp-1".into(),
            None,
            None,
            &quest,
        )
        .unwrap();

        assert_eq!(delta.changed_tasks.len(), 2); // 所有 Task 都是变更
        assert!(delta.metadata_delta.is_some());
        assert!(delta.base_checkpoint_id.is_none());
    }

    #[test]
    fn test_incremental_task_status_change_only() {
        let old_tasks = vec![
            make_task("t-0", "task 0", TaskStatus::Pending),
            make_task("t-1", "task 1", TaskStatus::Pending),
        ];
        let old_quest = make_quest("q-1", old_tasks);

        let mut new_tasks = old_quest.tasks.clone();
        new_tasks[0].status = TaskStatus::Completed; // 仅变更状态
        let new_quest = make_quest("q-1", new_tasks);

        let delta = IncrementalCheckpointBuilder::build(
            "q-1".into(),
            "cp-2".into(),
            Some("cp-1".into()),
            Some(&old_quest),
            &new_quest,
        )
        .unwrap();

        assert_eq!(delta.changed_tasks.len(), 1); // 仅 1 个 Task 变更
        assert_eq!(delta.changed_tasks[0].task_id, "t-0");
        assert_eq!(delta.changed_tasks[0].status, TaskStatus::Completed);
    }

    #[test]
    fn test_incremental_no_changes() {
        let tasks = vec![make_task("t-0", "task 0", TaskStatus::Pending)];
        let quest = make_quest("q-1", tasks);

        let delta = IncrementalCheckpointBuilder::build(
            "q-1".into(),
            "cp-2".into(),
            Some("cp-1".into()),
            Some(&quest),
            &quest, // 相同状态
        )
        .unwrap();

        assert!(delta.changed_tasks.is_empty());
        assert!(delta.metadata_delta.is_none());
    }

    #[test]
    fn test_restore_from_base() {
        let base_tasks = vec![
            make_task("t-0", "task 0", TaskStatus::Pending),
            make_task("t-1", "task 1", TaskStatus::Pending),
        ];
        let base_quest = make_quest("q-1", base_tasks);

        let mut new_tasks = base_quest.tasks.clone();
        new_tasks[0].status = TaskStatus::Completed;
        let new_quest = make_quest("q-1", new_tasks);

        let delta = IncrementalCheckpointBuilder::build(
            "q-1".into(),
            "cp-2".into(),
            Some("cp-1".into()),
            Some(&base_quest),
            &new_quest,
        )
        .unwrap();

        let restored = IncrementalCheckpointRestorer::restore(Some(base_quest), &delta).unwrap();
        assert_eq!(restored.tasks[0].status, TaskStatus::Completed);
        assert_eq!(restored.tasks[1].status, TaskStatus::Pending);
    }

    #[test]
    fn test_restore_from_none() {
        let tasks = vec![make_task("t-0", "task 0", TaskStatus::Pending)];
        let quest = make_quest("q-1", tasks);

        let delta = IncrementalCheckpointBuilder::build(
            "q-1".into(),
            "cp-1".into(),
            None,
            None,
            &quest,
        )
        .unwrap();

        let restored = IncrementalCheckpointRestorer::restore(None, &delta).unwrap();
        assert_eq!(restored.tasks.len(), 1);
        assert_eq!(restored.title, "Quest q-1");
    }

    #[test]
    fn test_restore_chain() {
        // 基础状态
        let base_tasks = vec![
            make_task("t-0", "task 0", TaskStatus::Pending),
            make_task("t-1", "task 1", TaskStatus::Pending),
        ];
        let base_quest = make_quest("q-1", base_tasks);

        // 第一次增量:t-0 完成
        let mut tasks1 = base_quest.tasks.clone();
        tasks1[0].status = TaskStatus::Completed;
        let quest1 = make_quest("q-1", tasks1);
        let delta1 = IncrementalCheckpointBuilder::build(
            "q-1".into(),
            "cp-1".into(),
            None,
            Some(&base_quest),
            &quest1,
        )
        .unwrap();

        // 第二次增量:t-1 完成
        let mut tasks2 = quest1.tasks.clone();
        tasks2[1].status = TaskStatus::Completed;
        let quest2 = make_quest("q-1", tasks2);
        let delta2 = IncrementalCheckpointBuilder::build(
            "q-1".into(),
            "cp-2".into(),
            Some("cp-1".into()),
            Some(&quest1),
            &quest2,
        )
        .unwrap();

        // 从链恢复
        let restored =
            IncrementalCheckpointRestorer::restore_chain(Some(base_quest), &[delta1, delta2])
                .unwrap();
        assert_eq!(restored.tasks[0].status, TaskStatus::Completed);
        assert_eq!(restored.tasks[1].status, TaskStatus::Completed);
    }

    #[test]
    fn test_metadata_delta_thinking_mode() {
        let mut old_quest = make_quest("q-1", vec![]);
        old_quest.thinking_mode = ThinkingMode::Standard;

        let mut new_quest = old_quest.clone();
        new_quest.thinking_mode = ThinkingMode::Deep;

        let delta = IncrementalCheckpointBuilder::build(
            "q-1".into(),
            "cp-2".into(),
            Some("cp-1".into()),
            Some(&old_quest),
            &new_quest,
        )
        .unwrap();

        assert!(delta.metadata_delta.is_some());
        assert_eq!(delta.metadata_delta.as_ref().unwrap().thinking_mode, Some(ThinkingMode::Deep));
    }

    #[test]
    fn test_estimate_incremental_ratio() {
        let old_tasks = vec![
            make_task("t-0", "task 0", TaskStatus::Pending),
            make_task("t-1", "task 1", TaskStatus::Pending),
            make_task("t-2", "task 2", TaskStatus::Pending),
        ];
        let old_quest = make_quest("q-1", old_tasks);

        let mut new_tasks = old_quest.tasks.clone();
        new_tasks[0].status = TaskStatus::Completed;
        let new_quest = make_quest("q-1", new_tasks);

        let delta = IncrementalCheckpointBuilder::build(
            "q-1".into(),
            "cp-2".into(),
            Some("cp-1".into()),
            Some(&old_quest),
            &new_quest,
        )
        .unwrap();

        let ratio = estimate_incremental_ratio(&delta, &new_quest);
        // 1/3 Task 变更 ≈ 33%
        assert!(ratio < 50.0, "增量应小于 50%, got {}", ratio);
    }

    #[test]
    fn test_integrity_verification() {
        let tasks = vec![make_task("t-0", "task 0", TaskStatus::Pending)];
        let quest = make_quest("q-1", tasks);

        let delta = IncrementalCheckpointBuilder::build(
            "q-1".into(),
            "cp-1".into(),
            None,
            None,
            &quest,
        )
        .unwrap();

        // 正常恢复应成功
        let restored = IncrementalCheckpointRestorer::restore(None, &delta);
        assert!(restored.is_ok());
    }
}
