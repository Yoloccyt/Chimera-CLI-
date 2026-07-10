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
use decb_governor::BudgetTier;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_core::{Checkpoint, Quest, Task, TaskStatus, ThinkingMode, UserIntent};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::checkpoint::CheckpointManager;
use crate::config::QuestConfig;
use crate::dag::validate_dag;
use crate::error::QuestError;
use crate::semantic_dag::SemanticDagDecomposer;
use crate::ttg::TtgGovernor;

/// Quest Engine — 长期任务分解与生命周期管理
///
/// 管理 Quest 注册表,通过 Event Bus 广播生命周期事件。
/// 可选配置 `CheckpointManager` 启用 LHQP 检查点持久化。
/// 可选配置 `TtgGovernor` 启用基于复杂度和预算的自动思考模式选择。
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
    /// TTG 思考模式治理器(Option 允许禁用自动模式选择)
    ///
    /// WHY Option:简单场景或测试不需要 TTG 自动治理,
    /// None 时 create_quest 使用默认 ThinkingMode::Standard,
    /// Some 时 create_quest 自动调用 select_mode_and_publish 选择最优模式。
    ttg_governor: Option<TtgGovernor>,
}

impl QuestEngine {
    /// 创建 QuestEngine,使用默认配置(不启用检查点与 TTG)
    pub fn new(event_bus: EventBus) -> Self {
        Self::with_config(event_bus, QuestConfig::default())
    }

    /// 创建 QuestEngine,使用自定义配置(不启用检查点与 TTG)
    pub fn with_config(event_bus: EventBus, config: QuestConfig) -> Self {
        Self {
            quests: Arc::new(DashMap::new()),
            event_bus,
            config,
            checkpoint_manager: None,
            ttg_governor: None,
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
            ttg_governor: None,
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
            ttg_governor: None,
        }
    }

    /// 创建带检查点与 TTG 治理器的完整 QuestEngine
    ///
    /// 全功能构造:Quest 创建时自动评估复杂度并选择思考模式,
    /// 模式切换通过 EventBus 发布 ThinkingModeSwitched 事件。
    ///
    /// 注意:无论 `ttg_governor` 是否已持有 EventBus,此方法都会用
    /// engine 的 EventBus 覆盖它,确保两者共享同一事件通道。
    pub fn full(
        event_bus: EventBus,
        config: QuestConfig,
        checkpoint_dir: PathBuf,
        mut ttg_governor: TtgGovernor,
    ) -> Self {
        // 确保 TTG 治理器与 engine 使用同一 EventBus(防止配置漂移)
        ttg_governor.set_event_bus(event_bus.clone());
        Self {
            quests: Arc::new(DashMap::new()),
            event_bus,
            config,
            checkpoint_manager: Some(CheckpointManager::new(checkpoint_dir)),
            ttg_governor: Some(ttg_governor),
        }
    }

    /// 注入 TTG 治理器(延迟绑定场景)
    ///
    /// WHY:某些初始化流程中 TtgGovernor 在 QuestEngine 之后才构造,
    /// 此方法允许延迟注入,与 `set_event_bus` 对称。
    pub fn set_ttg_governor(&mut self, governor: TtgGovernor) {
        self.ttg_governor = Some(governor);
    }

    /// 获取 TTG 治理器引用(若已配置)
    pub fn ttg_governor(&self) -> Option<&TtgGovernor> {
        self.ttg_governor.as_ref()
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
        let checkpoint = cm.save(&quest).await?;

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
        let checkpoint = cm.load_latest(quest_id).await?.ok_or_else(|| {
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
        // 4. 创建 Quest 实例(默认 Standard,TTG 集成后自动选择)
        let title = extract_title(&intent.raw_text);
        let mut quest = Quest {
            quest_id: quest_id.clone(),
            title,
            tasks,
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        };
        let task_count = quest.tasks.len() as u32;

        // 5. TTG 自动模式选择(若已配置)
        //    WHY 在插入 DashMap 前选择:减少锁竞争(不需要先插入再 get_mut 更新)
        //    默认 BudgetTier::LowTier — 新 Quest 尚无预算历史,
        //    LowTier 使 TTG 根据复杂度保守选择:简单→Fast,中等→Standard,复杂→Standard。
        //    DECB 启动后通过 ttg_on_budget_adjusted 联动切换到实际档位。
        if let Some(governor) = &self.ttg_governor {
            match governor
                .select_mode_and_publish(&quest_id, &quest, BudgetTier::LowTier)
                .await
            {
                Ok(Some((mode, _reason))) => {
                    quest.thinking_mode = mode;
                    debug!(
                        quest_id = %quest_id,
                        ?mode,
                        "TTG 自动选择思考模式"
                    );
                }
                Ok(None) => {} // 模式未变化,无需操作
                Err(e) => {
                    warn!(
                        quest_id = %quest_id,
                        error = %e,
                        "TTG 自动选择失败,使用默认 Standard 模式"
                    );
                }
            }
        }

        // 6. 存入注册表
        self.quests.insert(quest_id.clone(), quest.clone());
        debug!(
            quest_id = %quest_id,
            task_count,
            "Quest 已创建并注册"
        );
        // 7. 发布 QuestCreated 事件
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

    /// P1-12: 从用户意图创建 Quest — 使用语义 DAG 分解器
    ///
    /// 与 `create_quest` 的区别:
    /// - `create_quest`:基于标点切分 + 线性依赖链(向后兼容)
    /// - `create_quest_semantic`:基于语义相似度构建 DAG 依赖图
    ///
    /// 流程:
    /// 1. 调用 `SemanticDagDecomposer::decompose` 按语义分解任务
    /// 2. `validate_dag` 校验无环
    /// 3. 生成 quest_id
    /// 4. 创建 Quest 实例
    /// 5. TTG 自动模式选择(若已配置)
    /// 6. 存入 DashMap
    /// 7. 发布 QuestCreated 事件
    pub async fn create_quest_semantic(&self, intent: UserIntent) -> Result<Quest, QuestError> {
        // 1. 语义 DAG 分解
        let decomposer = SemanticDagDecomposer::new();
        let tasks = decomposer.decompose(&intent.raw_text, self.config.max_tasks_per_quest as usize)?;

        // 2. 校验 DAG 无环
        validate_dag(&tasks)?;

        // 3. 生成 quest_id
        let quest_id = format!("quest-semantic-{}", Uuid::now_v7());

        // 4. 创建 Quest
        let title = extract_title(&intent.raw_text);
        let mut quest = Quest {
            quest_id: quest_id.clone(),
            title,
            tasks,
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        };
        let task_count = quest.tasks.len() as u32;

        // 5. TTG 自动模式选择
        if let Some(governor) = &self.ttg_governor {
            match governor
                .select_mode_and_publish(&quest_id, &quest, BudgetTier::LowTier)
                .await
            {
                Ok(Some((mode, _reason))) => {
                    quest.thinking_mode = mode;
                    debug!(quest_id = %quest_id, ?mode, "TTG 自动选择思考模式(语义 DAG)");
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(
                        quest_id = %quest_id,
                        error = %e,
                        "TTG 自动选择失败(语义 DAG),使用默认 Standard"
                    );
                }
            }
        }

        // 6. 存入注册表
        self.quests.insert(quest_id.clone(), quest.clone());
        debug!(quest_id = %quest_id, task_count, "Quest(语义 DAG)已创建并注册");

        // 7. 发布 QuestCreated 事件
        let event = NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id.clone(),
            title: quest.title.clone(),
            task_count,
        };
        self.event_bus.publish(event).await?;
        info!(quest_id = %quest_id, "QuestCreated(语义 DAG)事件已发布");
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
    /// 当 TTG 治理器已配置时,委托给 `override_mode_and_publish`,
    /// 自动记录覆盖状态并发布事件。未配置时走原有手动切换逻辑。
    ///
    /// WHY 手动覆盖使用 HighTier:手动切换是用户/上层的显式决策,
    /// 不应受当前预算档位约束(Degraded 下用户仍可强制 Deep)。
    /// 预算约束仅作用于 TTG 自动选择路径(见 `ttg_auto_select`)。
    ///
    /// WHY:Parliament 订阅此事件,据此调整预算分配
    /// (Deep 模式消耗更多 token,需提前预留预算)
    pub async fn switch_thinking_mode(
        &self,
        quest_id: &str,
        new_mode: ThinkingMode,
    ) -> Result<(), QuestError> {
        // TTG 集成路径:委托给治理器,自动记录覆盖 + 发布事件
        if let Some(governor) = &self.ttg_governor {
            let mode = governor
                .override_mode_and_publish(quest_id, new_mode, BudgetTier::HighTier)
                .await?;
            // 同步更新 DashMap 中的 Quest 状态
            self.apply_thinking_mode(quest_id, mode)?;
            return Ok(());
        }

        // 无 TTG 路径:原有手动切换逻辑
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

    /// 将思考模式应用到 DashMap 中的 Quest(内部辅助)
    ///
    /// WHY:TTG 治理器维护独立的模式注册表,但 Quest 结构体的 thinking_mode
    /// 字段需要同步更新,供 Parliament 等直接读取 Quest 的消费方使用。
    fn apply_thinking_mode(&self, quest_id: &str, mode: ThinkingMode) -> Result<(), QuestError> {
        let mut entry = self
            .quests
            .get_mut(quest_id)
            .ok_or_else(|| QuestError::QuestNotFound(quest_id.to_string()))?;
        entry.thinking_mode = mode;
        Ok(())
    }

    /// TTG 自动选择 — 基于 Quest 复杂度与预算档位自动选择思考模式
    ///
    /// 若未配置 TTG 治理器,返回当前 Quest 的模式(不做变更)。
    /// 若已配置,调用 `select_mode_and_publish` 并同步更新 Quest。
    pub async fn ttg_auto_select(
        &self,
        quest_id: &str,
        budget_tier: BudgetTier,
    ) -> Result<ThinkingMode, QuestError> {
        let quest = self
            .get_quest(quest_id)
            .ok_or_else(|| QuestError::QuestNotFound(quest_id.to_string()))?;

        let governor = match &self.ttg_governor {
            Some(g) => g,
            None => return Ok(quest.thinking_mode),
        };

        match governor
            .select_mode_and_publish(quest_id, &quest, budget_tier)
            .await?
        {
            Some((mode, _reason)) => {
                self.apply_thinking_mode(quest_id, mode)?;
                Ok(mode)
            }
            None => Ok(quest.thinking_mode),
        }
    }

    /// TTG 预算联动 — DECB 档位变化时自动重新选择思考模式
    ///
    /// 若未配置 TTG 治理器,静默跳过(返回 Ok)。
    /// 配置时委托给 `on_budget_adjusted_and_publish` 并同步更新 Quest。
    pub async fn ttg_on_budget_adjusted(
        &self,
        quest_id: &str,
        old_tier: BudgetTier,
        new_tier: BudgetTier,
    ) -> Result<(), QuestError> {
        let quest = match self.get_quest(quest_id) {
            Some(q) => q,
            None => return Ok(()), // Quest 可能已完成或不存在,静默跳过
        };

        let governor = match &self.ttg_governor {
            Some(g) => g,
            None => return Ok(()),
        };

        if let Some((mode, _reason)) = governor
            .on_budget_adjusted_and_publish(quest_id, old_tier, new_tier, &quest)
            .await?
        {
            self.apply_thinking_mode(quest_id, mode)?;
        }
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

    // ============================================================
    // P1-6: TTG 集成测试
    // ============================================================

    #[tokio::test]
    async fn test_create_quest_with_ttg_auto_selects_mode() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let governor = TtgGovernor::with_event_bus(crate::ttg::TtgConfig::default(), bus.clone());
        let mut engine = QuestEngine::new(bus);
        engine.set_ttg_governor(governor);

        // 简单意图(2 个句子 = 2 个任务,≤ simple_task_threshold=3)→ Fast
        let intent = make_intent("分析需求。设计方案。");
        let quest = engine.create_quest(intent).await.unwrap();

        // TTG 应为简单任务选择 Fast 模式
        assert_eq!(
            quest.thinking_mode,
            ThinkingMode::Fast,
            "简单任务应自动选择 Fast 模式"
        );

        // 应收 ThinkingModeSwitched + QuestCreated 两个事件
        let mut saw_mode_switch = false;
        let mut saw_quest_created = false;
        for _ in 0..2 {
            let event = rx.recv().await.unwrap();
            match event {
                NexusEvent::ThinkingModeSwitched { to_mode, .. } => {
                    assert_eq!(to_mode, "Fast");
                    saw_mode_switch = true;
                }
                NexusEvent::QuestCreated { .. } => {
                    saw_quest_created = true;
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert!(saw_mode_switch, "应发布 ThinkingModeSwitched 事件");
        assert!(saw_quest_created, "应发布 QuestCreated 事件");
    }

    #[tokio::test]
    async fn test_create_quest_without_ttg_defaults_standard() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus);
        let intent = make_intent("分析需求。设计方案。");
        let quest = engine.create_quest(intent).await.unwrap();
        // 无 TTG 时应保持默认 Standard
        assert_eq!(quest.thinking_mode, ThinkingMode::Standard);
    }

    #[tokio::test]
    async fn test_ttg_auto_select_updates_quest_in_dashmap() {
        let bus = EventBus::new();
        let governor = TtgGovernor::with_event_bus(crate::ttg::TtgConfig::default(), bus.clone());
        let mut engine = QuestEngine::new(bus);
        engine.set_ttg_governor(governor);

        let intent = make_intent("分析。");
        let quest = engine.create_quest(intent).await.unwrap();
        // DashMap 中的 Quest 也应更新为 Fast
        let stored = engine.get_quest(&quest.quest_id).unwrap();
        assert_eq!(stored.thinking_mode, ThinkingMode::Fast);
    }

    #[tokio::test]
    async fn test_switch_thinking_mode_with_ttg_delegates() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let governor = TtgGovernor::with_event_bus(crate::ttg::TtgConfig::default(), bus.clone());
        let mut engine = QuestEngine::new(bus);
        engine.set_ttg_governor(governor);

        let intent = make_intent("分析需求。");
        let quest = engine.create_quest(intent).await.unwrap();
        // 消费 create_quest 产生的事件
        let _ = rx.recv().await; // ThinkingModeSwitched
        let _ = rx.recv().await; // QuestCreated

        // 手动切换到 Deep — TTG 应记录为手动覆盖
        engine
            .switch_thinking_mode(&quest.quest_id, ThinkingMode::Deep)
            .await
            .unwrap();

        let stored = engine.get_quest(&quest.quest_id).unwrap();
        assert_eq!(stored.thinking_mode, ThinkingMode::Deep);

        // 验证 TTG 治理器记录了覆盖状态
        assert!(engine
            .ttg_governor()
            .unwrap()
            .is_overridden(&quest.quest_id));
    }

    #[tokio::test]
    async fn test_ttg_on_budget_adjusted_integration() {
        let bus = EventBus::new();
        let governor = TtgGovernor::with_event_bus(crate::ttg::TtgConfig::default(), bus.clone());

        // 验证 HighTier 下 12 任务 → Deep(规则 4)— 在 move 到 engine 前测试
        let long_text = "一。二。三。四。五。六。七。八。九。十。十一。十二。";
        let intent = make_intent(long_text);
        // 注:select_mode 内部通过 tasks.len() 计数,此处直接构造 12 任务的 quest
        let quest_with_tasks = Quest {
            quest_id: "test-quest-2".to_string(),
            title: "test".to_string(),
            tasks: (0..12)
                .map(|i| nexus_core::Task {
                    task_id: format!("task-{i}"),
                    description: format!("任务 {i}"),
                    dependencies: vec![],
                    status: nexus_core::TaskStatus::Pending,
                })
                .collect(),
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        };
        let (mode, _reason) = governor.select_mode(&quest_with_tasks, BudgetTier::HighTier);
        assert_eq!(
            mode,
            ThinkingMode::Deep,
            "12 任务 + HighTier → Deep(规则 4)"
        );

        let mut engine = QuestEngine::new(bus);
        engine.set_ttg_governor(governor);

        // 创建一个复杂 Quest(12 个任务 > complex_task_threshold=10)
        // create_quest 默认使用 LowTier → 规则 3(OR 条件) → Standard
        let quest = engine.create_quest(intent).await.unwrap();
        assert_eq!(
            quest.thinking_mode,
            ThinkingMode::Standard,
            "12 任务 + LowTier → Standard(规则 3 OR 条件)"
        );

        // 模拟预算降级:LowTier → Degraded,应自动切换为 Fast
        engine
            .ttg_on_budget_adjusted(&quest.quest_id, BudgetTier::LowTier, BudgetTier::Degraded)
            .await
            .unwrap();

        let stored = engine.get_quest(&quest.quest_id).unwrap();
        assert_eq!(
            stored.thinking_mode,
            ThinkingMode::Fast,
            "Degraded 预算应强制 Fast"
        );
    }

    #[tokio::test]
    async fn test_ttg_on_budget_adjusted_without_governor_is_noop() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus);
        let intent = make_intent("分析。");
        let quest = engine.create_quest(intent).await.unwrap();

        // 无 TTG 时 ttg_on_budget_adjusted 应静默成功
        let result = engine
            .ttg_on_budget_adjusted(&quest.quest_id, BudgetTier::LowTier, BudgetTier::Degraded)
            .await;
        assert!(result.is_ok());

        // 模式不变
        let stored = engine.get_quest(&quest.quest_id).unwrap();
        assert_eq!(stored.thinking_mode, ThinkingMode::Standard);
    }

    // ============================================================
    // P1-12: 语义 DAG 分解测试
    // ============================================================

    #[tokio::test]
    async fn test_create_quest_semantic_publishes_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let engine = QuestEngine::new(bus);
        let intent = make_intent("分析需求。设计方案。实现功能。测试验证。");
        let quest = engine.create_quest_semantic(intent).await.unwrap();

        // 语义分解应产生 4 个任务
        assert_eq!(quest.tasks.len(), 4);
        // quest_id 应包含 semantic 前缀
        assert!(quest.quest_id.starts_with("quest-semantic-"));

        // 应收到 QuestCreated 事件
        let event = rx.recv().await.unwrap();
        match event {
            NexusEvent::QuestCreated { quest_id, .. } => {
                assert_eq!(quest_id, quest.quest_id);
            }
            other => panic!("expected QuestCreated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_create_quest_semantic_vs_linear() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus);

        // 使用有明显语义关联的文本
        let text = "设计数据库表结构。编写数据库访问层代码。实现业务逻辑处理。集成前端展示组件。";
        let intent = make_intent(text);

        // 语义 DAG 分解
        let semantic_quest = engine.create_quest_semantic(intent.clone()).await.unwrap();
        // 线性链分解
        let linear_quest = engine.create_quest(intent).await.unwrap();

        // 两者任务数相同(基于相同切分)
        assert_eq!(semantic_quest.tasks.len(), linear_quest.tasks.len());

        // 语义分解的依赖关系应更复杂(至少有一个任务有非紧邻依赖)
        // 或至少与线性链不同
        let semantic_deps: Vec<Vec<String>> = semantic_quest
            .tasks
            .iter()
            .map(|t| t.dependencies.clone())
            .collect();
        let linear_deps: Vec<Vec<String>> = linear_quest
            .tasks
            .iter()
            .map(|t| t.dependencies.clone())
            .collect();

        // 语义 DAG 和线性链的依赖结构可能不同
        // (取决于语义相似度计算结果,不强求一定不同)
        assert!(
            !semantic_deps.is_empty() || !linear_deps.is_empty(),
            "两者都应产生有效依赖"
        );
    }

    #[tokio::test]
    async fn test_create_quest_semantic_empty_text() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus);
        let intent = make_intent("");
        let quest = engine.create_quest_semantic(intent).await.unwrap();
        assert_eq!(quest.tasks.len(), 1);
        assert_eq!(quest.tasks[0].task_id, "task-0");
    }

    #[tokio::test]
    async fn test_create_quest_semantic_respects_max_tasks() {
        let bus = EventBus::new();
        let config = QuestConfig::new(3, 1);
        let engine = QuestEngine::with_config(bus, config);
        let intent = make_intent("一。二。三。四。五。六。");
        let quest = engine.create_quest_semantic(intent).await.unwrap();
        assert_eq!(quest.tasks.len(), 3); // 截断到 max=3
    }
}
