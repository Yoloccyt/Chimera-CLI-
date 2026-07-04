//! QuestEngine 实现 — 任务分解、生命周期管理与事件广播
//!
//! 对应架构层:L9 Quest
//! 对应创新点:TTG(Thinking Toggle Governance)+ LHQP(Long-Horizon Quest Persistence)
//!
//! # 核心职责
//! - 从 `UserIntent` 分解任务图(DAG),校验无环后创建 Quest
//! - 维护 Task 状态机(Pending→Running→Completed/Failed),广播进度事件
//! - 切换思考模式(TTG),广播模式切换事件供 Parliament 调整预算
//! - 完成 Quest 时广播 ExecutionCompleted 事件
//! - LHQP 检查点持久化:Task 完成达阈值自动保存,支持崩溃恢复
//!
//! # 线程安全
//! 基于 `DashMap<String, Quest>`,支持并发 create/update/get。
//! `EventBus` 内部基于 `tokio::broadcast`,Clone 廉价(Arc 引用计数)。
//!
//! # 架构红线
//! - 所有状态变更通过 Event Bus 广播(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect()
//! - 持锁状态下不可 await,避免死锁与长时间持锁

use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_core::{Checkpoint, Quest, Task, TaskStatus, ThinkingMode, UserIntent};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::checkpoint::CheckpointManager;
use crate::config::QuestConfig;
use crate::dag::validate_dag;
use crate::error::QuestError;

/// Quest Engine — 长期任务分解与生命周期管理
///
/// 管理 Quest 注册表,通过 Event Bus 广播生命周期事件。
/// 可选配置 `CheckpointManager` 启用 LHQP 检查点持久化。
/// 可跨 async 任务共享(Send + Sync),所有方法满足 Send 约束。
pub struct QuestEngine {
    /// Quest 注册表(quest_id → Quest),DashMap 分片锁支持并发读写
    quests: Arc<DashMap<String, Quest>>,
    /// 事件总线(基于 Arc,Clone 廉价)
    event_bus: EventBus,
    /// 引擎配置
    config: QuestConfig,
    /// 检查点管理器(Option 允许禁用检查点功能)
    ///
    /// WHY:Option 而非直接持有 — 测试场景或内存模式无需持久化,
    /// None 时 save_checkpoint/restore_from_checkpoint 返回明确错误
    checkpoint_manager: Option<CheckpointManager>,
}

impl QuestEngine {
    /// 创建 QuestEngine,使用默认配置(不启用检查点)
    pub fn new(event_bus: EventBus) -> Self {
        Self::with_config(event_bus, QuestConfig::default())
    }

    /// 创建 QuestEngine,使用自定义配置(不启用检查点)
    pub fn with_config(event_bus: EventBus, config: QuestConfig) -> Self {
        Self {
            quests: Arc::new(DashMap::new()),
            event_bus,
            config,
            checkpoint_manager: None,
        }
    }

    /// 创建带检查点持久化的 QuestEngine
    ///
    /// `checkpoint_dir` 为检查点根目录,内部按 `<quest_id>/<checkpoint_id>.bin` 组织
    pub fn with_checkpoints(
        event_bus: EventBus,
        config: QuestConfig,
        checkpoint_dir: PathBuf,
    ) -> Self {
        Self {
            quests: Arc::new(DashMap::new()),
            event_bus,
            config,
            checkpoint_manager: Some(CheckpointManager::new(checkpoint_dir)),
        }
    }

    /// 创建带检查点持久化与自定义保留数量的 QuestEngine
    pub fn with_checkpoints_and_max_keep(
        event_bus: EventBus,
        config: QuestConfig,
        checkpoint_dir: PathBuf,
        max_keep: usize,
    ) -> Self {
        Self {
            quests: Arc::new(DashMap::new()),
            event_bus,
            config,
            checkpoint_manager: Some(CheckpointManager::with_max_keep(checkpoint_dir, max_keep)),
        }
    }

    /// 获取检查点管理器引用(若已配置)
    pub fn checkpoint_manager(&self) -> Option<&CheckpointManager> {
        self.checkpoint_manager.as_ref()
    }

    /// 保存检查点 — 序列化当前 Quest 状态并发布 CheckpointSaved 事件 `[Critical]`
    ///
    /// WHY:CheckpointSaved 标注 Critical,丢失将导致 Quest 无法恢复,
    /// EventBus 背压策略据此保护(见 event-bus backpressure 模块)
    pub async fn save_checkpoint(&self, quest_id: &str) -> Result<Checkpoint, QuestError> {
        let cm = self
            .checkpoint_manager
            .as_ref()
            .ok_or_else(|| QuestError::CheckpointSaveFailed("checkpoints disabled".into()))?;
        // 获取 Quest 快照(get_quest 内部 clone,不持锁返回)
        let quest = self
            .get_quest(quest_id)
            .ok_or_else(|| QuestError::QuestNotFound(quest_id.to_string()))?;
        let checkpoint = cm.save(&quest)?;

        // 发布 CheckpointSaved 事件(Critical,EventBus 据此优先投递)
        let event = NexusEvent::CheckpointSaved {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id.to_string(),
            checkpoint_id: checkpoint.checkpoint_id.clone(),
            memory_snapshot_hash: checkpoint.memory_snapshot_hash.clone(),
        };
        self.event_bus.publish(event).await?;
        info!(
            quest_id = %quest_id,
            checkpoint_id = %checkpoint.checkpoint_id,
            "CheckpointSaved 事件已发布"
        );
        Ok(checkpoint)
    }

    /// 从最新检查点恢复 Quest — 发布 CheckpointLoaded 事件
    ///
    /// 流程:
    /// 1. 加载最新检查点(load_latest 按 created_at 排序)
    /// 2. 反序列化为 Quest
    /// 3. 存入注册表(覆盖现有条目)
    /// 4. 发布 CheckpointLoaded 事件
    pub async fn restore_from_checkpoint(&self, quest_id: &str) -> Result<Quest, QuestError> {
        let cm = self
            .checkpoint_manager
            .as_ref()
            .ok_or_else(|| QuestError::CheckpointNotFound("checkpoints disabled".into()))?;
        let checkpoint = cm.load_latest(quest_id)?.ok_or_else(|| {
            QuestError::CheckpointNotFound(format!("no checkpoint for {quest_id}"))
        })?;

        // 反序列化 Quest 状态(MessagePack → Quest)
        let quest: Quest = rmp_serde::from_slice(&checkpoint.serialized_state)
            .map_err(|e| QuestError::SerializationError(format!("msgpack decode quest: {e}")))?;

        // 存入注册表(若已存在则覆盖,模拟"崩溃后重启"场景)
        self.quests.insert(quest_id.to_string(), quest.clone());

        // 发布 CheckpointLoaded 事件
        let event = NexusEvent::CheckpointLoaded {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id.to_string(),
            checkpoint_id: checkpoint.checkpoint_id,
        };
        self.event_bus.publish(event).await?;
        info!(
            quest_id = %quest_id,
            "CheckpointLoaded 事件已发布,Quest 已从检查点恢复"
        );
        Ok(quest)
    }

    /// 从用户意图创建 Quest — 自动分解任务图并发布 QuestCreated 事件
    ///
    /// 流程:
    /// 1. 调用 `decompose` 按规则分解任务
    /// 2. `validate_dag` 校验无环
    /// 3. 生成 quest_id(UUIDv7,时间有序)
    /// 4. 创建 Quest 实例,默认思考模式 Standard
    /// 5. 存入 DashMap
    /// 6. 发布 QuestCreated 事件
    pub async fn create_quest(&self, intent: UserIntent) -> Result<Quest, QuestError> {
        // 1. 分解任务
        let tasks = self.decompose(&intent)?;
        // 2. 校验 DAG 无环(规则分解器产出的线性链理论上无环,但防御性校验)
        validate_dag(&tasks)?;
        // 3. 生成 quest_id(UUIDv7 时间有序,便于跨进程因果追踪)
        let quest_id = format!("quest-{}", Uuid::now_v7());
        // 4. 创建 Quest 实例
        let title = extract_title(&intent.raw_text);
        let quest = Quest {
            quest_id: quest_id.clone(),
            title,
            tasks,
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        };
        let task_count = quest.tasks.len() as u32;
        // 5. 存入注册表
        self.quests.insert(quest_id.clone(), quest.clone());
        debug!(
            quest_id = %quest_id,
            task_count,
            "Quest 已创建并注册"
        );
        // 6. 发布 QuestCreated 事件
        let event = NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id.clone(),
            title: quest.title.clone(),
            task_count,
        };
        self.event_bus.publish(event).await?;
        info!(quest_id = %quest_id, "QuestCreated 事件已发布");
        Ok(quest)
    }

    /// 任务分解 — Week 2 阶段使用规则分解器
    ///
    /// 规则:
    /// - 按 `。!?.?`(中英文句末标点)切分 raw_text 为句子
    /// - 每个非空句子作为一个 Task
    /// - 限制在 `max_tasks_per_quest` 内(超出截断)
    /// - 依赖关系:线性链(task_i 依赖 task_{i-1}),确保执行顺序
    /// - 若无有效句子,创建单个占位 Task
    fn decompose(&self, intent: &UserIntent) -> Result<Vec<Task>, QuestError> {
        let sentences = split_sentences(&intent.raw_text);
        let max = self.config.max_tasks_per_quest as usize;

        // 截断到上限,防止任务爆炸(架构红线:单 Quest ≤ 16 任务)
        let sentences: Vec<&str> = sentences.into_iter().take(max).collect();

        if sentences.is_empty() {
            // 无有效句子时创建占位任务,保证 Quest 至少有一个 Task
            return Ok(vec![Task {
                task_id: "task-0".to_string(),
                description: intent.raw_text.clone(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            }]);
        }

        let mut tasks = Vec::with_capacity(sentences.len());
        for (idx, sentence) in sentences.iter().enumerate() {
            let task_id = format!("task-{idx}");
            // 线性依赖链:第一个无依赖,其余依赖前一个
            let dependencies = if idx == 0 {
                vec![]
            } else {
                vec![format!("task-{}", idx - 1)]
            };
            tasks.push(Task {
                task_id,
                description: sentence.to_string(),
                status: TaskStatus::Pending,
                dependencies,
            });
        }
        Ok(tasks)
    }

    /// 更新任务状态 — 校验状态转换合法性并广播 QuestProgressUpdated 事件
    ///
    /// 合法转换:Pending→Running、Running→Completed、Running→Failed
    /// 非法转换(如 Running→Pending、Completed→Running)返回 `InvalidStatus`
    ///
    /// 自动检查点:若 Task 转为 Completed 且 `checkpoint_interval > 0`,
    /// 且累计完成数是 interval 的整数倍,触发 save_checkpoint。
    /// WHY:先 drop(entry) 释放 DashMap 写锁,再调用 save_checkpoint,
    /// 避免持锁 await 导致死锁(save_checkpoint 内部 get_quest 重入获取读锁)
    ///
    /// 若所有 Task 达终态(Completed/Failed),自动调用 `complete_quest`
    pub async fn update_task_status(
        &self,
        quest_id: &str,
        task_id: &str,
        status: TaskStatus,
    ) -> Result<(), QuestError> {
        // 1. 查找 Quest(DashMap entry 持有写锁,原子完成查找+更新)
        let mut entry = self
            .quests
            .get_mut(quest_id)
            .ok_or_else(|| QuestError::QuestNotFound(quest_id.to_string()))?;

        // 2. 查找 Task
        let task = entry
            .tasks
            .iter_mut()
            .find(|t| t.task_id == task_id)
            .ok_or_else(|| QuestError::TaskNotFound(task_id.to_string()))?;

        // 3. 校验状态转换合法性
        let old_status = task.status;
        if !is_valid_transition(old_status, status) {
            return Err(QuestError::InvalidStatus(format!(
                "{old_status:?}->{status:?}"
            )));
        }

        // 4. 更新状态
        task.status = status;
        let quest_id_owned = quest_id.to_string();
        let total = entry.tasks.len() as u32;
        let completed = entry
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .count() as u32;
        let all_terminal = entry
            .tasks
            .iter()
            .all(|t| t.status == TaskStatus::Completed || t.status == TaskStatus::Failed);
        // 释放 entry 锁,避免持锁发布事件(防止死锁与长时间持锁)
        drop(entry);

        debug!(
            quest_id = %quest_id_owned,
            task_id,
            old = ?old_status,
            new = ?status,
            "Task 状态已更新"
        );

        // 5. 发布 QuestProgressUpdated 事件
        let event = NexusEvent::QuestProgressUpdated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id_owned.clone(),
            completed,
            total,
        };
        self.event_bus.publish(event).await?;

        // 6. 自动检查点:Task 完成 + interval>0 + 完成数是 interval 倍数
        //    WHY:仅 Completed 触发(非 Failed),Failed 由上层决定是否回滚
        if status == TaskStatus::Completed
            && self.config.checkpoint_interval > 0
            && completed > 0
            && completed.is_multiple_of(self.config.checkpoint_interval)
        {
            // 检查点保存失败不应阻断 Task 状态更新流程,仅记录告警
            if let Err(e) = self.save_checkpoint(&quest_id_owned).await {
                warn!(
                    quest_id = %quest_id_owned,
                    error = %e,
                    "自动检查点保存失败(不影响 Task 状态更新)"
                );
            }
        }

        // 7. 若所有 Task 达终态,完成 Quest
        if all_terminal {
            self.complete_quest(&quest_id_owned).await?;
        }

        Ok(())
    }

    /// 完成 Quest — 发布 ExecutionCompleted 事件
    ///
    /// WHY:result_hash 使用 Quest 标题的简单哈希作为占位,
    /// 后续阶段由 GQEP 执行器提供真实产出哈希
    pub async fn complete_quest(&self, quest_id: &str) -> Result<(), QuestError> {
        let result_hash = simple_hash(quest_id);
        let event = NexusEvent::ExecutionCompleted {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id.to_string(),
            result_hash,
        };
        self.event_bus.publish(event).await?;
        info!(quest_id = %quest_id, "ExecutionCompleted 事件已发布");
        Ok(())
    }

    /// 切换思考模式 — 发布 ThinkingModeSwitched 事件
    ///
    /// WHY:Parliament 订阅此事件,据此调整预算分配
    /// (Deep 模式消耗更多 token,需提前预留预算)
    pub async fn switch_thinking_mode(
        &self,
        quest_id: &str,
        new_mode: ThinkingMode,
    ) -> Result<(), QuestError> {
        let mut entry = self
            .quests
            .get_mut(quest_id)
            .ok_or_else(|| QuestError::QuestNotFound(quest_id.to_string()))?;

        let from_mode = entry.thinking_mode;
        if from_mode == new_mode {
            warn!(
                quest_id = %quest_id,
                ?new_mode,
                "思考模式未变化,跳过切换"
            );
            return Ok(());
        }
        entry.thinking_mode = new_mode;
        let quest_id_owned = quest_id.to_string();
        drop(entry);

        let event = NexusEvent::ThinkingModeSwitched {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id_owned,
            from_mode: format!("{from_mode:?}"),
            to_mode: format!("{new_mode:?}"),
            // WHY reason="manual_switch":此方法为手动切换入口,
            // 区分于 TTG 自动选择与预算联动切换的 reason
            reason: "manual_switch".to_string(),
        };
        self.event_bus.publish(event).await?;
        info!(
            quest_id = %quest_id,
            from = ?from_mode,
            to = ?new_mode,
            "ThinkingModeSwitched 事件已发布"
        );
        Ok(())
    }

    /// 按 ID 获取 Quest 克隆(不存在返回 None)
    pub fn get_quest(&self, quest_id: &str) -> Option<Quest> {
        self.quests.get(quest_id).map(|r| r.clone())
    }

    /// 列出所有 Quest(无序)
    pub fn list_quests(&self) -> Vec<Quest> {
        self.quests.iter().map(|r| r.clone()).collect()
    }

    /// 获取配置引用(用于测试与调试)
    pub fn config(&self) -> &QuestConfig {
        &self.config
    }
}

/// 校验状态转换合法性 — 单向流转:Pending→Running→Completed/Failed
///
/// 终态(Completed/Failed)不可再转换;Running 不可回退到 Pending
fn is_valid_transition(from: TaskStatus, to: TaskStatus) -> bool {
    use TaskStatus::*;
    matches!(
        (from, to),
        (Pending, Running)
            | (Running, Completed)
            | (Running, Failed)
            // 幂等转换:同状态视为合法(避免重复触发报错)
            | (Pending, Pending)
            | (Running, Running)
            | (Completed, Completed)
            | (Failed, Failed)
    )
}

/// 句子切分 — 按 `。!?.?`(中英文句末标点)切分,保留非空句子
///
/// WHY:Week 2 阶段使用规则分解,后续阶段替换为 LLM 分解器。
/// 此函数仅做简单切分,不处理嵌套引号等复杂语义。
fn split_sentences(text: &str) -> Vec<&str> {
    text.split(['。', '!', '?', '.', '？', '！'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 从原始文本提取标题 — 取第一个句子或前 50 字符
fn extract_title(raw_text: &str) -> String {
    let first_sentence = raw_text
        .split(['。', '!', '?', '.', '？', '！'])
        .next()
        .unwrap_or("")
        .trim();
    if first_sentence.len() <= 50 {
        first_sentence.to_string()
    } else {
        // 截断到 50 字符(按 char 边界,避免切断多字节字符)
        first_sentence.chars().take(50).collect()
    }
}

/// 简单哈希 — 用于占位 result_hash
///
/// WHY:Week 2 阶段不引入 sha2 依赖,使用 FNV-1a 变体作为占位。
/// 后续阶段由 GQEP 执行器提供真实 SHA-256 哈希。
fn simple_hash(input: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV prime
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::MultimodalInput;

    fn make_intent(text: &str) -> UserIntent {
        UserIntent {
            intent_id: "i-1".into(),
            raw_text: text.into(),
            multimodal_inputs: vec![MultimodalInput::Text(text.into())],
            risk_level: 10,
        }
    }

    #[test]
    fn test_split_sentences_chinese() {
        let s = split_sentences("第一步分析。第二步设计。第三步实现。");
        assert_eq!(s.len(), 3);
        assert_eq!(s[0], "第一步分析");
    }

    #[test]
    fn test_split_sentences_mixed_punctuation() {
        let s = split_sentences("Analyze. Design! Implement? Test.");
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn test_split_sentences_empty() {
        let s = split_sentences("");
        assert!(s.is_empty());
    }

    #[test]
    fn test_split_sentences_only_punctuation() {
        let s = split_sentences("。。。!!!");
        assert!(s.is_empty());
    }

    #[test]
    fn test_is_valid_transition_legal() {
        use TaskStatus::*;
        assert!(is_valid_transition(Pending, Running));
        assert!(is_valid_transition(Running, Completed));
        assert!(is_valid_transition(Running, Failed));
    }

    #[test]
    fn test_is_valid_transition_illegal() {
        use TaskStatus::*;
        assert!(!is_valid_transition(Running, Pending));
        assert!(!is_valid_transition(Completed, Running));
        assert!(!is_valid_transition(Failed, Running));
        assert!(!is_valid_transition(Pending, Completed));
        assert!(!is_valid_transition(Pending, Failed));
    }

    #[test]
    fn test_is_valid_transition_idempotent() {
        use TaskStatus::*;
        assert!(is_valid_transition(Pending, Pending));
        assert!(is_valid_transition(Completed, Completed));
    }

    #[test]
    fn test_extract_title_short() {
        let title = extract_title("这是一个短标题。后续内容。");
        assert_eq!(title, "这是一个短标题");
    }

    #[test]
    fn test_extract_title_long() {
        let long =
            "这是一个非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常长的标题";
        let title = extract_title(long);
        assert!(title.chars().count() <= 50);
    }

    #[test]
    fn test_simple_hash_deterministic() {
        let h1 = simple_hash("test");
        let h2 = simple_hash("test");
        assert_eq!(h1, h2);
        assert_ne!(simple_hash("test"), simple_hash("other"));
    }

    #[test]
    fn test_decompose_linear_chain() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus);
        let intent = make_intent("第一步。第二步。第三步。");
        let tasks = engine.decompose(&intent).unwrap();
        assert_eq!(tasks.len(), 3);
        assert!(tasks[0].dependencies.is_empty());
        assert_eq!(tasks[1].dependencies, vec!["task-0"]);
        assert_eq!(tasks[2].dependencies, vec!["task-1"]);
    }

    #[test]
    fn test_decompose_empty_creates_placeholder() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus);
        let intent = make_intent("");
        let tasks = engine.decompose(&intent).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].task_id, "task-0");
    }

    #[test]
    fn test_decompose_respects_max_tasks() {
        let bus = EventBus::new();
        let config = QuestConfig::new(3, 1);
        let engine = QuestEngine::with_config(bus, config);
        let intent = make_intent("一。二。三。四。五。六。");
        let tasks = engine.decompose(&intent).unwrap();
        assert_eq!(tasks.len(), 3); // 截断到 max=3
    }

    #[tokio::test]
    async fn test_create_quest_publishes_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let engine = QuestEngine::new(bus);
        let intent = make_intent("分析需求。设计方案。");
        let quest = engine.create_quest(intent).await.unwrap();
        assert_eq!(quest.tasks.len(), 2);
        let event = rx.recv().await.unwrap();
        match event {
            NexusEvent::QuestCreated { quest_id, .. } => {
                assert_eq!(quest_id, quest.quest_id);
            }
            other => panic!("expected QuestCreated, got {other:?}"),
        }
    }
}
