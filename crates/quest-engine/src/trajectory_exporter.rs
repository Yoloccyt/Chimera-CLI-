//! P4-W16.1.3: Quest 轨迹导出器 — Checkpoint → 四元组(state/action/reward/context)
//!
//! 对应架构层:L9 Quest
//! 对应 spec.md §Scenario "model-router 轨迹捕获" 捕获点 2
//! 对应 tasks.md P4-W16.1.3: quest-engine Checkpoint(MessagePack 快照)
//!   → 轨迹导出器(状态/动作/奖励/上下文摘要四元组)
//!
//! # 设计原则
//!
//! ## 1. 纯函数转换(无副作用)
//! 导出器仅观察 `Checkpoint` + `Quest`,产出 `QuestTrajectory`。
//! 不修改输入,不进行 I/O,便于测试与并发调用。
//! 与 P4-W16.1.1 `RouteHook` 的"不可变借用契约"一致。
//!
//! ## 2. 奖励护栏合规(spec §P4-W16.3.3)
//! `TrajectoryReward` 必须含 ≥L3 执行反馈信号。
//! 本实现使用 `TaskStatus`(Completed/Failed)作为 L3 反馈来源,
//! 满足"禁止单独使用模型自评/LLM 评判"的约束。
//!
//! ## 3. 与捕获点 1 的衔接
//! 捕获点 1(P4-W16.1.2 `RecordingHook`)记录单次路由调用轨迹,
//! 捕获点 2(本模块)记录 Quest 检查点时刻的系统状态轨迹。
//! 两者共同构成经验回放池(P4-W16.2)的输入源:
//! - 捕获点 1 → 短时序、高频率的路由决策轨迹
//! - 捕获点 2 → 长时序、低频率的 Quest 状态轨迹
//!
//! ## 4. 序列化支持
//! 所有类型派生 `Serialize + Deserialize`,支持序列化到经验回放池(P4-W16.2)。
//! 使用 JSON 序列化便于人工排查;回放池内部可转为 MessagePack 节省空间。
//!
//! # RL 四元组映射
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    QuestTrajectory                          │
//! ├─────────────────────────────────────────────────────────────┤
//! │  state   (s_t): Quest 观测状态(任务进度+优先级+标题)         │
//! │  action  (a_t): TTG 选择的 ThinkingMode                     │
//! │  reward  (r_t): 基于 TaskStatus 的 L3 执行反馈奖励           │
//! │  context (ctx): Checkpoint 元数据(便于去重与审计)            │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # 数据流
//! ```text
//! CheckpointManager.save(quest)
//!   └── Checkpoint { serialized_state: Vec<u8>(MessagePack Quest) }
//!        │
//!        ▼
//! export_trajectory(&checkpoint)
//!   ├── rmp_serde::from_slice → Quest
//!   ├── TaskProgress::from_quest(&quest)
//!   ├── TrajectoryState { quest_id, title, task_progress, priority }
//!   ├── TrajectoryAction { thinking_mode }
//!   ├── TrajectoryReward { completion_rate, failure_rate, net_reward }
//!   └── ContextSummary { checkpoint_id, created_at, memory_snapshot_hash, size }
//!        │
//!        ▼
//! QuestTrajectory → 经验回放池(P4-W16.2)
//! ```

use chrono::{DateTime, Utc};
use nexus_core::{Checkpoint, Quest, TaskStatus, ThinkingMode};
use serde::{Deserialize, Serialize};

use crate::error::QuestError;

// ============================================================
// 四元组类型定义
// ============================================================

/// 轨迹状态 — Checkpoint 时刻的 Quest 观测快照(s_t)
///
/// # 设计决策
/// - **不含 thinking_mode**:thinking_mode 是 TTG 的"动作"(a_t),不应混入状态。
///   状态仅描述"环境"(任务进度+优先级),动作描述"决策"(思考模式)。
///   这避免了 RL 中"状态-动作混淆"的反模式。
/// - **含 task_progress**:任务进度是 L3 执行反馈的客观事实,
///   是奖励信号与状态观测的共同来源。
/// - **含 priority**:优先级影响 TTG 决策(高优先级 Quest 倾向 Deep 模式),
///   是状态的重要维度。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrajectoryState {
    /// 所属 Quest ID(便于跨模块追踪)
    pub quest_id: String,
    /// Quest 标题(人类可读,便于人工排查回放池)
    pub title: String,
    /// 任务进度统计(L3 执行反馈的客观事实)
    pub task_progress: TaskProgress,
    /// Quest 优先级(0-255,默认 128)
    pub priority: u8,
}

/// 任务进度统计 — 状态表征与奖励计算的共同来源
///
/// # 设计决策
/// - **u32 计数**:Quest 任务数理论上限 ~4G,u32 足够且内存紧凑
/// - **派生 Default**:便于构造空进度用于测试或边界场景
/// - **Eq + PartialEq**:进度是纯计数,无浮点,可派生 Eq
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskProgress {
    /// 总任务数
    pub total: u32,
    /// 待执行任务数
    pub pending: u32,
    /// 执行中任务数
    pub running: u32,
    /// 已完成任务数
    pub completed: u32,
    /// 已失败任务数
    pub failed: u32,
}

impl TaskProgress {
    /// 从 Quest 的任务列表统计进度
    ///
    /// WHY 静态方法:纯函数,无副作用,便于测试。
    /// 遍历一次 O(n),n = 任务数(通常 < 16,与 GQEP 批处理窗口对齐)。
    pub fn from_quest(quest: &Quest) -> Self {
        let mut progress = TaskProgress {
            total: quest.tasks.len() as u32,
            ..Default::default()
        };
        for task in &quest.tasks {
            match task.status {
                TaskStatus::Pending => progress.pending += 1,
                TaskStatus::Running => progress.running += 1,
                TaskStatus::Completed => progress.completed += 1,
                TaskStatus::Failed => progress.failed += 1,
                // Task 3.10: Cancelled 视为非成功终止(归入 failed),
                // Paused 视为进行中(归入 running)
                TaskStatus::Cancelled => progress.failed += 1,
                TaskStatus::Paused => progress.running += 1,
            }
        }
        progress
    }

    /// 已结束任务数(Completed + Failed)— 用于奖励归一化
    ///
    /// WHY:Pending/Running 任务尚未产生明确反馈,
    /// 奖励计算应基于"已结束"任务,避免未完成任务稀释信号。
    pub fn finished(&self) -> u32 {
        self.completed + self.failed
    }
}

/// 轨迹动作 — TTG 在 s_t 选择的思考模式(a_t)
///
/// # 设计决策
/// - **仅含 thinking_mode**:这是 omega-learner 试图优化的唯一动作维度
/// - **Copy 类型**:ThinkingMode 已为 Copy,封装为 action 便于后续扩展
///   (如未来增加"路由策略"作为第二动作维度时,只需扩展此结构)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrajectoryAction {
    /// TTG 选择的思考模式
    pub thinking_mode: ThinkingMode,
}

impl TrajectoryAction {
    /// 从 Quest 的 thinking_mode 构造动作
    pub fn from_quest(quest: &Quest) -> Self {
        Self {
            thinking_mode: quest.thinking_mode,
        }
    }
}

/// 轨迹奖励 — 基于 L3 执行反馈(TaskStatus)计算的奖励信号(r_t)
///
/// # 奖励护栏合规(spec §P4-W16.3.3)
/// 奖励必须含 ≥L3 执行反馈信号,本实现的 L3 反馈来源:
/// - **L3 反馈**:TaskStatus(Completed/Failed/Pending/Running)
/// - **不含**:模型自评、LLM 评判(违反护栏)
///
/// # 奖励公式
/// ```text
/// completion_rate = completed / total       ∈ [0.0, 1.0]
/// failure_rate    = failed / total          ∈ [0.0, 1.0]
/// net_reward      = completion_rate - 0.5 * failure_rate  ∈ [-0.5, 1.0]
/// ```
///
/// # 设计决策
/// - **失败惩罚权重 0.5**:失败应惩罚,但不应过度抑制探索。
///   0.5 是经验值:若权重=1.0,完成+失败抵消为零,无法区分"全完成"与"半完成半失败"。
/// - **f32 精度**:奖励计算不需 f64 精度,f32 节省内存且与项目 §4.4 f32 约定一致。
/// - **保留原始计数**:便于上层(omega-learner)重新加权或调试。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrajectoryReward {
    /// 任务完成率 [0.0, 1.0]
    pub completion_rate: f32,
    /// 任务失败率 [0.0, 1.0]
    pub failure_rate: f32,
    /// 综合奖励 [-0.5, 1.0]
    ///
    /// 计算公式:`completion_rate - 0.5 * failure_rate`
    /// - 全完成(net=1.0):完美执行
    /// - 全失败(net=-0.5):应避免的状态
    /// - 全 pending(net=0.0):中性,尚无反馈
    pub net_reward: f32,
    /// 已完成任务数
    pub completed_tasks: u32,
    /// 已失败任务数
    pub failed_tasks: u32,
    /// 总任务数
    pub total_tasks: u32,
}

impl TrajectoryReward {
    /// 从任务进度计算奖励
    ///
    /// WHY 静态方法:纯函数,无副作用,便于单元测试与属性测试(proptest)。
    ///
    /// # 边界处理
    /// - `total = 0`:completion_rate 与 failure_rate 均为 0.0(避免除零)
    /// - 否则按公式计算
    pub fn from_progress(progress: &TaskProgress) -> Self {
        let total = progress.total;
        // WHY f32 转换在前:避免 `u32 as f32 / u32 as f32` 的重复转换
        let total_f = total as f32;
        let completion_rate = if total == 0 {
            0.0
        } else {
            progress.completed as f32 / total_f
        };
        let failure_rate = if total == 0 {
            0.0
        } else {
            progress.failed as f32 / total_f
        };
        // net_reward 公式见类型文档
        let net_reward = completion_rate - 0.5 * failure_rate;
        Self {
            completion_rate,
            failure_rate,
            net_reward,
            completed_tasks: progress.completed,
            failed_tasks: progress.failed,
            total_tasks: total,
        }
    }
}

/// 上下文摘要 — 附加在四元组上的元数据(ctx)
///
/// # 用途
/// - **回放池去重**:`memory_snapshot_hash` 唯一标识 Quest 状态,
///   回放池据此避免重复存储相同状态的轨迹。
/// - **版本追踪**:`checkpoint_id` 含 UUIDv7 时间戳,便于按时序排列。
/// - **容量规划**:`serialized_state_size` 帮助回放池预估内存占用。
/// - **审计追溯**:与 L4 seccore 的 Merkle 链对齐,便于安全审计。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextSummary {
    /// 检查点 ID(UUIDv7,时间有序)
    pub checkpoint_id: String,
    /// 检查点创建时间(UTC,从 Checkpoint.created_at 复制)
    pub created_at: DateTime<Utc>,
    /// 记忆快照哈希(SHA-256 hex,从 Checkpoint.memory_snapshot_hash 复制)
    ///
    /// 用于回放池去重:相同 hash 表示相同 Quest 状态,无需重复存储。
    pub memory_snapshot_hash: String,
    /// 序列化状态字节数(用于容量规划)
    ///
    /// WHY u64:虽然 u32 足够(单 Quest 序列化后通常 < 4GB),
    /// 但 u64 与回放池其他容量指标(如总字节)对齐,避免混合精度。
    pub serialized_state_size: u64,
}

/// Quest 轨迹四元组 — 经验回放池的存储单元
///
/// 完整记录一次 Checkpoint 时刻的系统观测:
/// (state, action, reward, context)
///
/// # 序列化支持
/// 派生 `Serialize + Deserialize`,支持:
/// - JSON 序列化(人工排查、调试)
/// - MessagePack 序列化(回放池内部存储,节省 30-50% 空间)
///
/// # Send + Sync
/// 所有字段均为 Owned 类型(String/Vec/u32/f32/DateTime),
/// 自动派生 Send + Sync,可在 async 任务间传递。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuestTrajectory {
    /// 状态观测(s_t)
    pub state: TrajectoryState,
    /// 动作决策(a_t)
    pub action: TrajectoryAction,
    /// 奖励信号(r_t)
    pub reward: TrajectoryReward,
    /// 上下文摘要(ctx)
    pub context: ContextSummary,
}

// ============================================================
// 导出函数
// ============================================================

/// 从 Checkpoint 导出轨迹四元组
///
/// 对应 spec.md §Scenario "model-router 轨迹捕获" 捕获点 2 的主入口。
///
/// # 流程
/// 1. 反序列化 `Checkpoint.serialized_state` 为 `Quest`(MessagePack 解码)
/// 2. 调用 [`export_trajectory_from_quest`] 构造四元组
///
/// # 错误
/// - `QuestError::TrajectoryExportFailed`:MessagePack 反序列化失败
///   (可能原因:序列化数据损坏、版本不兼容、Quest 字段缺失)
///
/// # 性能
/// - MessagePack 反序列化:~10-50μs(取决于 Quest 大小)
/// - 四元组构造:~1-5μs(纯计算)
/// - 总开销:<100μs,适合在 Checkpoint 保存后立即调用
///
/// # 使用示例
/// ```rust,ignore
/// use quest_engine::checkpoint::CheckpointManager;
/// use quest_engine::trajectory_exporter::export_trajectory;
///
/// # async fn example() {
/// let cm = CheckpointManager::new("/tmp/checkpoints".into());
/// let checkpoint = cm.load_latest("quest-123").await?.expect("存在检查点");
/// let trajectory = export_trajectory(&checkpoint)?;
/// println!("net_reward = {:.3}", trajectory.reward.net_reward);
/// # }
/// ```
pub fn export_trajectory(checkpoint: &Checkpoint) -> Result<QuestTrajectory, QuestError> {
    // 反序列化 Quest 状态(MessagePack → Quest)
    // WHY TrajectoryExportFailed 而非 SerializationError:便于上层针对性处理
    let quest: Quest = rmp_serde::from_slice(&checkpoint.serialized_state).map_err(|e| {
        QuestError::TrajectoryExportFailed(format!(
            "msgpack decode quest failed: {e} (checkpoint_id={}, quest_id={})",
            checkpoint.checkpoint_id, checkpoint.quest_id
        ))
    })?;
    Ok(export_trajectory_from_quest(checkpoint, &quest))
}

/// 从 Checkpoint + 已解码 Quest 导出轨迹四元组
///
/// 避免重复解码:调用方若已通过 `restore_from_checkpoint` 解码 Quest,
/// 可直接传入避免二次 `rmp-serde` 反序列化开销。
///
/// # 设计决策
/// - **纯函数**:不修改输入,无副作用
/// - **同步**:仅做内存计算,无 I/O
/// - **不返回 Result**:无失败可能(Quest 已解码,后续均为纯计算)
///
/// # 参数
/// - `checkpoint`:提供 context 字段(checkpoint_id/created_at/hash/size)
/// - `quest`:提供 state/action/reward 字段(quest_id/title/progress/thinking_mode)
pub fn export_trajectory_from_quest(checkpoint: &Checkpoint, quest: &Quest) -> QuestTrajectory {
    // 计算任务进度(L3 执行反馈)
    let progress = TaskProgress::from_quest(quest);

    // 构造状态(s_t):Quest 观测快照(不含 thinking_mode,避免与 action 混淆)
    let state = TrajectoryState {
        quest_id: quest.quest_id.clone(),
        title: quest.title.clone(),
        task_progress: progress.clone(),
        priority: quest.priority,
    };

    // 构造动作(a_t):TTG 选择的 thinking_mode
    let action = TrajectoryAction::from_quest(quest);

    // 构造奖励(r_t):基于 L3 执行反馈(TaskStatus)计算
    let reward = TrajectoryReward::from_progress(&progress);

    // 构造上下文摘要(ctx):Checkpoint 元数据
    let context = ContextSummary {
        checkpoint_id: checkpoint.checkpoint_id.clone(),
        created_at: checkpoint.created_at,
        memory_snapshot_hash: checkpoint.memory_snapshot_hash.clone(),
        serialized_state_size: checkpoint.serialized_state.len() as u64,
    };

    QuestTrajectory {
        state,
        action,
        reward,
        context,
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nexus_core::{MultimodalInput, Task, UserIntent};

    // ============================================================
    // 测试夹具
    // ============================================================

    /// 构造测试用 Quest(可指定任务状态分布)
    fn make_quest_with_status(
        quest_id: &str,
        title: &str,
        thinking_mode: ThinkingMode,
        priority: u8,
        tasks: Vec<Task>,
    ) -> Quest {
        Quest {
            quest_id: quest_id.into(),
            title: title.into(),
            tasks,
            thinking_mode,
            checkpoint_id: None,
            priority,
        }
    }

    /// 构造测试用 Task(指定状态)
    fn make_task(id: &str, status: TaskStatus) -> Task {
        Task {
            task_id: id.into(),
            description: format!("task-{id}"),
            status,
            dependencies: vec![],
        }
    }

    /// 构造测试用 Checkpoint(序列化 Quest 为 MessagePack)
    fn make_checkpoint(quest: &Quest) -> Checkpoint {
        let serialized_state = rmp_serde::to_vec(quest).expect("msgpack encode 必须成功");
        let memory_snapshot_hash = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&serialized_state);
            hex::encode(hasher.finalize())
        };
        Checkpoint::new(
            quest.quest_id.clone(),
            String::from("cp-test-001"),
            memory_snapshot_hash,
            serialized_state,
        )
    }

    // ============================================================
    // TaskProgress 测试
    // ============================================================

    #[test]
    fn test_task_progress_empty_quest() {
        let quest = make_quest_with_status("q-1", "Empty", ThinkingMode::Standard, 128, vec![]);
        let progress = TaskProgress::from_quest(&quest);
        assert_eq!(progress.total, 0);
        assert_eq!(progress.pending, 0);
        assert_eq!(progress.running, 0);
        assert_eq!(progress.completed, 0);
        assert_eq!(progress.failed, 0);
        assert_eq!(progress.finished(), 0);
    }

    #[test]
    fn test_task_progress_mixed_statuses() {
        let quest = make_quest_with_status(
            "q-1",
            "Mixed",
            ThinkingMode::Deep,
            200,
            vec![
                make_task("t-1", TaskStatus::Completed),
                make_task("t-2", TaskStatus::Completed),
                make_task("t-3", TaskStatus::Failed),
                make_task("t-4", TaskStatus::Running),
                make_task("t-5", TaskStatus::Pending),
            ],
        );
        let progress = TaskProgress::from_quest(&quest);
        assert_eq!(progress.total, 5);
        assert_eq!(progress.completed, 2);
        assert_eq!(progress.failed, 1);
        assert_eq!(progress.running, 1);
        assert_eq!(progress.pending, 1);
        assert_eq!(
            progress.finished(),
            3,
            "finished = completed + failed = 2+1"
        );
    }

    #[test]
    fn test_task_progress_default() {
        let p = TaskProgress::default();
        assert_eq!(p.total, 0);
        assert_eq!(p.finished(), 0);
    }

    // ============================================================
    // TrajectoryReward 测试
    // ============================================================

    #[test]
    fn test_reward_zero_total_quest() {
        // total=0 边界:completion_rate=0, failure_rate=0, net_reward=0
        let progress = TaskProgress {
            total: 0,
            pending: 0,
            running: 0,
            completed: 0,
            failed: 0,
        };
        let reward = TrajectoryReward::from_progress(&progress);
        assert_eq!(reward.completion_rate, 0.0);
        assert_eq!(reward.failure_rate, 0.0);
        assert_eq!(reward.net_reward, 0.0);
        assert_eq!(reward.total_tasks, 0);
    }

    #[test]
    fn test_reward_all_completed() {
        // 全完成:net_reward = 1.0
        let progress = TaskProgress {
            total: 4,
            pending: 0,
            running: 0,
            completed: 4,
            failed: 0,
        };
        let reward = TrajectoryReward::from_progress(&progress);
        assert!((reward.completion_rate - 1.0).abs() < f32::EPSILON);
        assert_eq!(reward.failure_rate, 0.0);
        assert!(
            (reward.net_reward - 1.0).abs() < f32::EPSILON,
            "全完成 net_reward 应为 1.0,实际: {}",
            reward.net_reward
        );
    }

    #[test]
    fn test_reward_all_failed() {
        // 全失败:net_reward = -0.5
        let progress = TaskProgress {
            total: 3,
            pending: 0,
            running: 0,
            completed: 0,
            failed: 3,
        };
        let reward = TrajectoryReward::from_progress(&progress);
        assert_eq!(reward.completion_rate, 0.0);
        assert!((reward.failure_rate - 1.0).abs() < f32::EPSILON);
        assert!(
            (reward.net_reward - (-0.5)).abs() < f32::EPSILON,
            "全失败 net_reward 应为 -0.5,实际: {}",
            reward.net_reward
        );
    }

    #[test]
    fn test_reward_mixed() {
        // 2 完成 + 1 失败 + 1 pending,total=4
        // completion_rate = 2/4 = 0.5
        // failure_rate = 1/4 = 0.25
        // net_reward = 0.5 - 0.5 * 0.25 = 0.375
        let progress = TaskProgress {
            total: 4,
            pending: 1,
            running: 0,
            completed: 2,
            failed: 1,
        };
        let reward = TrajectoryReward::from_progress(&progress);
        assert!((reward.completion_rate - 0.5).abs() < 1e-6);
        assert!((reward.failure_rate - 0.25).abs() < 1e-6);
        assert!(
            (reward.net_reward - 0.375).abs() < 1e-6,
            "net_reward 应为 0.375,实际: {}",
            reward.net_reward
        );
        assert_eq!(reward.completed_tasks, 2);
        assert_eq!(reward.failed_tasks, 1);
        assert_eq!(reward.total_tasks, 4);
    }

    #[test]
    fn test_reward_half_failed_half_completed() {
        // 1 完成 + 1 失败,total=2
        // completion_rate = 0.5, failure_rate = 0.5
        // net_reward = 0.5 - 0.5*0.5 = 0.25
        let progress = TaskProgress {
            total: 2,
            pending: 0,
            running: 0,
            completed: 1,
            failed: 1,
        };
        let reward = TrajectoryReward::from_progress(&progress);
        assert!((reward.net_reward - 0.25).abs() < 1e-6);
    }

    // ============================================================
    // TrajectoryAction 测试
    // ============================================================

    #[test]
    fn test_action_from_quest_each_mode() {
        for mode in [
            ThinkingMode::Fast,
            ThinkingMode::Standard,
            ThinkingMode::Deep,
        ] {
            let quest = make_quest_with_status("q-1", "T", mode, 128, vec![]);
            let action = TrajectoryAction::from_quest(&quest);
            assert_eq!(action.thinking_mode, mode);
        }
    }

    // ============================================================
    // export_trajectory_from_quest 测试
    // ============================================================

    #[test]
    fn test_export_trajectory_from_quest_full_fields() {
        let quest = make_quest_with_status(
            "q-export-1",
            "Export Test",
            ThinkingMode::Deep,
            200,
            vec![
                make_task("t-1", TaskStatus::Completed),
                make_task("t-2", TaskStatus::Completed),
                make_task("t-3", TaskStatus::Failed),
                make_task("t-4", TaskStatus::Pending),
            ],
        );
        let checkpoint = make_checkpoint(&quest);

        let trajectory = export_trajectory_from_quest(&checkpoint, &quest);

        // 验证 state 字段
        assert_eq!(trajectory.state.quest_id, "q-export-1");
        assert_eq!(trajectory.state.title, "Export Test");
        assert_eq!(trajectory.state.priority, 200);
        assert_eq!(trajectory.state.task_progress.total, 4);
        assert_eq!(trajectory.state.task_progress.completed, 2);
        assert_eq!(trajectory.state.task_progress.failed, 1);
        assert_eq!(trajectory.state.task_progress.pending, 1);
        // state 不应含 thinking_mode(避免与 action 混淆)
        // (TrajectoryState 结构无 thinking_mode 字段,编译时保证)

        // 验证 action 字段
        assert_eq!(trajectory.action.thinking_mode, ThinkingMode::Deep);

        // 验证 reward 字段
        // completion_rate = 2/4 = 0.5, failure_rate = 1/4 = 0.25
        // net_reward = 0.5 - 0.5*0.25 = 0.375
        assert!((trajectory.reward.completion_rate - 0.5).abs() < 1e-6);
        assert!((trajectory.reward.failure_rate - 0.25).abs() < 1e-6);
        assert!((trajectory.reward.net_reward - 0.375).abs() < 1e-6);
        assert_eq!(trajectory.reward.completed_tasks, 2);
        assert_eq!(trajectory.reward.failed_tasks, 1);
        assert_eq!(trajectory.reward.total_tasks, 4);

        // 验证 context 字段
        assert_eq!(trajectory.context.checkpoint_id, "cp-test-001");
        assert_eq!(
            trajectory.context.memory_snapshot_hash,
            checkpoint.memory_snapshot_hash
        );
        assert_eq!(
            trajectory.context.serialized_state_size,
            checkpoint.serialized_state.len() as u64
        );
        // created_at 应在合理时间范围内
        let now = Utc::now();
        let diff = now.signed_duration_since(trajectory.context.created_at);
        assert!(
            diff.num_seconds() < 60,
            "created_at 应为最近时间,实际偏差: {}s",
            diff.num_seconds()
        );
    }

    #[test]
    fn test_export_trajectory_from_quest_empty_tasks() {
        // 边界:无任务的 Quest
        let quest = make_quest_with_status("q-empty", "Empty", ThinkingMode::Fast, 128, vec![]);
        let checkpoint = make_checkpoint(&quest);

        let trajectory = export_trajectory_from_quest(&checkpoint, &quest);

        assert_eq!(trajectory.state.task_progress.total, 0);
        assert_eq!(trajectory.reward.completion_rate, 0.0);
        assert_eq!(trajectory.reward.failure_rate, 0.0);
        assert_eq!(trajectory.reward.net_reward, 0.0);
    }

    // ============================================================
    // export_trajectory 测试(含 MessagePack 解码)
    // ============================================================

    #[test]
    fn test_export_trajectory_decodes_msgpack() {
        // 端到端:Checkpoint.serialized_state → MessagePack 解码 → 四元组
        let quest = make_quest_with_status(
            "q-e2e",
            "E2E Test",
            ThinkingMode::Standard,
            128,
            vec![
                make_task("t-1", TaskStatus::Completed),
                make_task("t-2", TaskStatus::Failed),
            ],
        );
        let checkpoint = make_checkpoint(&quest);

        let trajectory = export_trajectory(&checkpoint).expect("导出必须成功");

        // 验证解码后的字段
        assert_eq!(trajectory.state.quest_id, "q-e2e");
        assert_eq!(trajectory.state.title, "E2E Test");
        assert_eq!(trajectory.state.task_progress.completed, 1);
        assert_eq!(trajectory.state.task_progress.failed, 1);
        assert_eq!(trajectory.action.thinking_mode, ThinkingMode::Standard);
    }

    #[test]
    fn test_export_trajectory_corrupted_data_fails() {
        // 损坏的 serialized_state 应返回 TrajectoryExportFailed
        let checkpoint = Checkpoint::new(
            "q-bad",
            "cp-bad",
            "hash-bad",
            vec![0xFF, 0xFF, 0xFF], // 无效 MessagePack
        );
        let result = export_trajectory(&checkpoint);
        assert!(
            matches!(result, Err(QuestError::TrajectoryExportFailed(_))),
            "损坏数据应返回 TrajectoryExportFailed,实际: {:?}",
            result
        );
        if let Err(QuestError::TrajectoryExportFailed(msg)) = result {
            assert!(
                msg.contains("msgpack decode"),
                "错误消息应含解码上下文: {}",
                msg
            );
            assert!(
                msg.contains("cp-bad"),
                "错误消息应含 checkpoint_id: {}",
                msg
            );
        }
    }

    #[test]
    fn test_export_trajectory_empty_serialized_state_fails() {
        // 空 serialized_state 也应失败
        let checkpoint = Checkpoint::new("q-empty", "cp-empty", "hash", vec![]);
        let result = export_trajectory(&checkpoint);
        assert!(matches!(result, Err(QuestError::TrajectoryExportFailed(_))));
    }

    // ============================================================
    // 序列化往返测试(为 P4-W16.2 回放池做准备)
    // ============================================================

    #[test]
    fn test_quest_trajectory_json_serde_roundtrip() {
        let quest = make_quest_with_status(
            "q-serde",
            "Serde Test",
            ThinkingMode::Deep,
            150,
            vec![
                make_task("t-1", TaskStatus::Completed),
                make_task("t-2", TaskStatus::Pending),
            ],
        );
        let checkpoint = make_checkpoint(&quest);
        let trajectory = export_trajectory_from_quest(&checkpoint, &quest);

        let json = serde_json::to_string(&trajectory).expect("JSON 序列化必须成功");
        let de: QuestTrajectory = serde_json::from_str(&json).expect("JSON 反序列化必须成功");
        assert_eq!(de, trajectory);
    }

    #[test]
    fn test_quest_trajectory_msgpack_serde_roundtrip() {
        // MessagePack 往返(回放池内部存储格式)
        let quest = make_quest_with_status(
            "q-msgpack",
            "MsgPack Test",
            ThinkingMode::Standard,
            128,
            vec![make_task("t-1", TaskStatus::Completed)],
        );
        let checkpoint = make_checkpoint(&quest);
        let trajectory = export_trajectory_from_quest(&checkpoint, &quest);

        let bytes = rmp_serde::to_vec(&trajectory).expect("MessagePack 序列化必须成功");
        let de: QuestTrajectory =
            rmp_serde::from_slice(&bytes).expect("MessagePack 反序列化必须成功");
        assert_eq!(de, trajectory);
    }

    // ============================================================
    // Send + Sync 静态断言(便于 async 任务间传递)
    // ============================================================

    #[test]
    fn test_trajectory_types_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TrajectoryState>();
        assert_send_sync::<TrajectoryAction>();
        assert_send_sync::<TrajectoryReward>();
        assert_send_sync::<TaskProgress>();
        assert_send_sync::<ContextSummary>();
        assert_send_sync::<QuestTrajectory>();
    }

    // ============================================================
    // 不可变借用契约测试(与 RouteHook 一致)
    // ============================================================

    #[test]
    fn test_export_does_not_mutate_inputs() {
        let quest = make_quest_with_status(
            "q-immut",
            "Immutability",
            ThinkingMode::Deep,
            100,
            vec![
                make_task("t-1", TaskStatus::Completed),
                make_task("t-2", TaskStatus::Failed),
            ],
        );
        let quest_before = quest.clone();
        let checkpoint = make_checkpoint(&quest);
        let checkpoint_before = checkpoint.clone();

        let _trajectory = export_trajectory_from_quest(&checkpoint, &quest);

        // 输入未被修改
        assert_eq!(quest, quest_before, "Quest 不应被修改");
        assert_eq!(checkpoint, checkpoint_before, "Checkpoint 不应被修改");
    }

    // ============================================================
    // 集成测试:与 UserIntent 创建 Quest 后导出轨迹
    // ============================================================

    #[test]
    fn test_export_trajectory_after_quest_creation() {
        // 模拟真实流程:UserIntent → Quest → Checkpoint → 轨迹
        let intent = UserIntent {
            intent_id: "i-1".into(),
            raw_text: "分析需求。设计方案。".into(),
            multimodal_inputs: vec![MultimodalInput::Text("...".into())],
            risk_level: 10,
        };

        // 简化:create_quest 内部会分解任务,这里手动构造等价 Quest
        let quest = Quest {
            quest_id: "quest-integration".into(),
            title: "Integration Test".into(),
            tasks: vec![
                Task {
                    task_id: "t-1".into(),
                    description: "分析".into(),
                    status: TaskStatus::Completed,
                    dependencies: vec![],
                },
                Task {
                    task_id: "t-2".into(),
                    description: "设计".into(),
                    status: TaskStatus::Running,
                    dependencies: vec!["t-1".into()],
                },
            ],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: 128,
        };
        let _ = intent; // 标记使用

        let checkpoint = make_checkpoint(&quest);
        let trajectory = export_trajectory(&checkpoint).expect("导出必须成功");

        // 验证完整四元组
        assert_eq!(trajectory.state.quest_id, "quest-integration");
        assert_eq!(trajectory.state.task_progress.total, 2);
        assert_eq!(trajectory.state.task_progress.completed, 1);
        assert_eq!(trajectory.state.task_progress.running, 1);
        assert_eq!(trajectory.action.thinking_mode, ThinkingMode::Standard);
        // 1 完成 / 2 总 = 0.5
        assert!((trajectory.reward.completion_rate - 0.5).abs() < 1e-6);
    }
}
