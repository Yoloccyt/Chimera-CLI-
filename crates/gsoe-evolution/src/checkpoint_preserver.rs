//! CheckpointPreserver — 保留历史最佳 checkpoint(RSIBench,文档 §10.3.3)
//!
//! 对应架构层:L5 Knowledge(gsoe-evolution 子模块)
//! 对应创新点:Ω₉-Preserve(保留历史最佳 + 停止策略)
//!
//! # 设计来源(RSIBench)
//!
//! RSIBench 关键发现:**78.26% 的继续搜索最终以低于历史峰值结束**。
//! 因此进化搜索必须显式保留历史最佳 checkpoint,并在继续搜索的
//! 期望收益低于阈值时停止。本模块是纯逻辑实现(无 IO/无事件),
//! 上层(GSOE 引擎 / 自我改进流水线)在每轮搜索后调用 `evaluate`
//! 评估新 checkpoint,并用 `should_stop` 裁决是否继续。
//!
//! # 与 mca-gateway::early_stop::StopDecision 的语义区分(WHY)
//!
//! 同名类型分属不同 crate,语义不同:
//! - 本模块 `StopDecision`:进化搜索停止策略(RSIBench,基于历史最佳与尝试次数)
//! - `mca_gateway::early_stop::StopDecision`:流式 token 早停(基于 StreamEvent)
//! 两者无引用关系,仅命名巧合;本模块 doc 明确标注以避免混淆。
//!
//! # R2 冻结声明(ADR-042)
//!
//! 本模块为规则/统计驱动的纯逻辑决策,无梯度更新、无 RL 训练路径,
//! 不在 R2(GSOE×AutoDPO 约束 RL)冻结范围内。

use std::collections::HashMap;

/// 进化搜索 checkpoint — 某任务类型下的一轮搜索结果快照
///
/// `score` 为归一化质量分(越高越好,由调用方的评估器产生)。
#[derive(Debug, Clone, PartialEq)]
pub struct Checkpoint {
    /// 任务类型(如 "bugfix" / "refactor"),按类型独立保留最佳
    pub task_type: String,
    /// 归一化质量分(越高越好)
    pub score: f64,
    /// 自由描述元数据(如版本号/来源),便于追溯最佳 checkpoint 的来源
    pub metadata: String,
}

impl Checkpoint {
    /// 创建 checkpoint
    pub fn new(task_type: impl Into<String>, score: f64, metadata: impl Into<String>) -> Self {
        Self {
            task_type: task_type.into(),
            score,
            metadata: metadata.into(),
        }
    }
}

/// 保留决策 — `evaluate` 的返回,表达新 checkpoint 与历史最佳的关系
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreserveDecision {
    /// 首次 checkpoint,直接保留为最佳
    KeepAsBest,
    /// 新 checkpoint 分数更高,替换历史最佳
    ReplaceBest,
    /// 新 checkpoint 分数更低(或相等),保留旧最佳
    KeepOldBest,
}

/// 停止决策 — `should_stop` 的返回,裁决是否继续搜索
///
/// 语义:进化搜索停止策略(RSIBench),非流式 token 早停。
#[derive(Debug, Clone, PartialEq)]
pub enum StopDecision {
    /// 继续搜索尚未明显劣化,应继续
    Continue,
    /// 达到停止条件,返回历史最佳 checkpoint
    Stop {
        /// 停止原因(人类可读)
        reason: String,
        /// 建议选用的历史最佳 checkpoint
        selected: Checkpoint,
    },
}

/// 最大尝试次数阈值 — 超过此值且已有最佳时建议停止(文档 §10.3.3)
///
/// WHY 10:RSIBench 观察到的搜索收益递减拐点经验值;
/// 文档 §10.3.3 `should_stop` 示例同款阈值。
pub const MAX_ATTEMPTS_BEFORE_STOP: u32 = 10;

/// 保留历史最佳 checkpoint 的纯逻辑容器
///
/// 按 `task_type` 隔离最佳;无全局状态、无 IO、无事件发布,
/// 可独立单测且确定性可复现(Ω₈-Assess 证据纪律)。
#[derive(Debug, Clone, Default)]
pub struct CheckpointPreserver {
    /// task_type → 历史最佳 checkpoint
    best_checkpoints: HashMap<String, Checkpoint>,
}

impl CheckpointPreserver {
    /// 创建空的保留器
    pub fn new() -> Self {
        Self::default()
    }

    /// 评估新 checkpoint,决定是否保留,并返回保留决策
    ///
    /// # 语义
    /// - 首次出现该 task_type → `KeepAsBest`(写入最佳)
    /// - 分数严格高于当前最佳 → `ReplaceBest`(更新最佳)
    /// - 分数相等或更低 → `KeepOldBest`(最佳不变)
    ///
    /// WHY 严格大于:相等分数替换会引入无意义抖动(metadata 反复覆盖),
    /// 严格比较保证最佳 checkpoint 稳定(Ω₉-Preserve)。
    pub fn evaluate(&mut self, checkpoint: &Checkpoint) -> PreserveDecision {
        match self.best_checkpoints.get(&checkpoint.task_type) {
            None => {
                self.best_checkpoints
                    .insert(checkpoint.task_type.clone(), checkpoint.clone());
                PreserveDecision::KeepAsBest
            }
            Some(best) => {
                if checkpoint.score > best.score {
                    self.best_checkpoints
                        .insert(checkpoint.task_type.clone(), checkpoint.clone());
                    PreserveDecision::ReplaceBest
                } else {
                    PreserveDecision::KeepOldBest
                }
            }
        }
    }

    /// 停止策略裁决:判断是否继续搜索(文档 §10.3.3)
    ///
    /// # 规则
    /// - 无该 task_type 的最佳 → 始终 `Continue`(无依据停止)
    /// - `attempts > MAX_ATTEMPTS_BEFORE_STOP`(10)且有最佳 → `Stop`
    ///   (RSIBench:继续搜索 78.26% 以低于历史峰值结束,继续搜索期望收益为负)
    /// - 其余 → `Continue`
    pub fn should_stop(&self, task_type: &str, attempts: u32) -> StopDecision {
        match self.best_checkpoints.get(task_type) {
            Some(best) if attempts > MAX_ATTEMPTS_BEFORE_STOP => StopDecision::Stop {
                reason: format!(
                    "Max attempts ({attempts} > {MAX_ATTEMPTS_BEFORE_STOP}) reached with valid best checkpoint"
                ),
                selected: best.clone(),
            },
            _ => StopDecision::Continue,
        }
    }

    /// 返回历史最佳映射的引用(测试与审计用)
    pub fn best_checkpoints(&self) -> &HashMap<String, Checkpoint> {
        &self.best_checkpoints
    }
}
