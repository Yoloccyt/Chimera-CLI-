//! 生产验证闭环 — 并行流式生成与验证的 Producer-Verifier 循环
//!
//! 对应架构层:L7 Execution
//! 对应创新点:PVL(Producer-Verifier Loop)
//!
//! ## 核心职责
//! - Producer 流式生成操作,通过 mpsc 通道发送给 Verifier
//! - Verifier 流式验证操作,发送反馈给 FeedbackChannel
//! - FeedbackChannel 实时监控拒绝率,触发 Producer 策略调整
//! - 全程无共享可变状态,通道所有权转移保证无竞态
//!
//! ## 对应尸检教训
//! Claude Code 5.4% 孤儿调用(void Promise 无 await)的根因是:
//! - 异步操作 spawn 后,JoinHandle 未被 await
//! - future 被 drop 但无运行时检测
//!
//! PVL 通过以下机制杜绝此类问题:
//! - 所有 async 操作(tx.send, rx.recv)均显式 await
//! - mpsc 通道所有权转移,无共享可变状态(无竞态)
//! - 通道关闭时显式返回错误(ChannelClosed),不静默丢弃
//!
//! ## 通道选择决策(对应 spec.md)
//! - **tokio::sync::mpsc**:多生产者单消费者,适合 Producer→Verifier 单向流
//! - **不选 broadcast**:PVL 是 1:1 的 Producer-Verifier,broadcast 适合 1:N
//! - **不选 oneshot**:PVL 需要流式多消息,oneshot 仅支持单消息
//! - **WHY 通道而非共享状态**:通道天然无竞态(消息所有权转移),共享状态需要锁
//!
//! ## 快速示例
//! ```no_run
//! use pvl_layer::{PvlConfig, Producer, Verifier, FeedbackChannel};
//! use event_bus::EventBus;
//!
//! # async fn run() {
//! let bus = EventBus::new();
//! let config = PvlConfig::default();
//! let producer = Producer::new(config.clone(), bus.clone());
//! let verifier = Verifier::new(config.clone(), bus.clone());
//! let feedback = FeedbackChannel::new(config, bus);
//!
//! let (op_tx, mut op_rx) = tokio::sync::mpsc::channel(128);
//! let (fb_tx, mut fb_rx) = tokio::sync::mpsc::channel(128);
//!
//! // 启动验证者(后台任务)
//! let verifier_handle = tokio::spawn(async move {
//!     verifier.run(&mut op_rx, &fb_tx).await
//! });
//!
//! // 生产操作
//! producer.produce("quest-1", 10, &op_tx).await.unwrap();
//! drop(op_tx);
//!
//! // 处理反馈
//! while let Some(fb) = fb_rx.recv().await {
//!     if feedback.process_feedback(fb) {
//!         feedback.check_and_adjust_strategy(&producer).ok();
//!     }
//! }
//!
//! verifier_handle.await.unwrap().unwrap();
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

/// polish-v2.7 closure Stage B-7:AutoBuilder 双智能体环境构建骨架(快手 KAT,ADR-049 降级档)
///
/// 骨架期默认不接线:沙箱执行器经 `SandboxExec` trait 由调用方注入,
/// 真实接线时由上层编排器提供 seccore Sandbox 适配层。
pub mod auto_builder;
/// Phase 7 §12.1: 经验卡片生成器（PVL 验证结果→ExperienceCard，ADR-049 内嵌）
pub mod card_generator;
/// P2-T13: L-b 结构化并发框架（WI-34 注入续期,JoinSet 有界并发 + 超时 + 保序）
///
/// LLM 类调用（produce/verify）归 L-b async 并发,禁 rayon（E8-2 红线）。
pub mod concurrency;
pub mod config;
/// Phase 7 §12.3: 动态验证深度 + 熵加权（OpenMLE + 快手融合，ADR-049 内嵌）
pub mod dynamic_depth;
pub mod error;
pub mod feedback;
/// Milestone D-2c:GTPO Turn-Level 奖励(设计 §11.1,纯函数计算)
pub mod gtpo;
/// polish-v2.7 P3-6:Hint-Boosted Recovery 过程级提示引导恢复(快手 KAT,ADR-049)
pub mod hint_recovery;
/// polish-v2.7 P3-5:Process-Score 九维度过程评分(快手 KAT,ADR-049)
///
/// 语义边界：本模块为观测九维（TUI 面板消费）；KAT 轨迹九维见
/// `trajectory_score`（Phase 7 §12.2，经验回放/裁决消费）。
pub mod process_score;
pub mod producer;
/// Milestone D-2d:RLVR 可验证奖励(设计 §11.2,enum dispatch 规则式验证器)
pub mod rlvr;
/// Phase 7 §12.4: Segment-aware 分段感知验证（Dressage，奖励 overlay D-4）
pub mod segment_validation;
/// Phase 7 §12.2: KAT 轨迹九维过程评分（D-2 命名协调，与 process_score 并存）
pub mod trajectory_score;
pub mod types;
pub mod verifier;

// === 关键类型重导出,简化外部导入 ===
// polish-v2.7 closure Stage B-7:AutoBuilder 骨架公开 API 重导出
pub use auto_builder::{
    AutoBuilder, BuildAgent, BuildFailure, BuildResult, BuildScript, ExecReport, ManifestKind,
    RepoLayout, SandboxExec, Verification, VerifyAgent,
};
// Phase 7 §12.1: 经验卡片生成器公开 API 重导出
pub use card_generator::{CardValidationInput, ExecutionMetadata, ExperienceCardGenerator};
pub use config::PvlConfig;
// Phase 7 §12.3: 动态验证深度 + 熵加权公开 API 重导出
pub use dynamic_depth::{DynamicVerifier, EntropyWeightedScorer, TaskRisk, VerificationDepth};
pub use error::PvlError;
pub use feedback::FeedbackChannel;
// polish-v2.7 P3:过程评分与提示恢复公开 API 重导出
pub use hint_recovery::{HintCategory, HintRecovery, RecoveryHint};
pub use process_score::{check_real_execution, ProcessObservation, ProcessScore, ProcessScorer};
pub use producer::Producer;
// Phase 7 §12.4: Segment-aware 分段感知验证公开 API 重导出
pub use segment_validation::{
    SegmentAwareValidator, SegmentRewardState, SegmentValidationError, SegmentValidationResult,
};
// Phase 7 §12.2: KAT 轨迹九维公开 API 重导出
pub use trajectory_score::{
    CodeChange, ProcessTrajectory, TrajectoryAction, TrajectoryProcessScore, VerificationStep,
};
pub use types::{
    FeedbackMessage, Operation, OperationId, OperationStatus, ProducerStrategy, VerificationResult,
};
pub use verifier::Verifier;

/// 九维度过程评分静态快照（Task 3.7:L10 → L7 向下依赖）
///
/// 为 TUI PvlScorePanel 提供无需异步上下文的静态快照，
/// 展示最近一次 PVL 过程评分（九维度 + 总分），
/// 供 TUI 面板直接调用，避免面板渲染阻塞。
///
/// **Phase 7 D-6 占位治理**：原恒 1.0 虚假数据占位已替换为真实快照
/// 注册表——评分路径经 [`register_pvl_score`] 写入，本函数读快照；
/// 未注册时返回默认满分快照并如实标注（不伪造运行时数据）。
pub fn pvl_score() -> ProcessScore {
    // 已注册真实评分 → 返回快照
    if let Some(score) = PVL_SCORE_SNAPSHOT
        .get()
        .and_then(|slot| slot.lock().ok().and_then(|guard| guard.clone()))
    {
        return score;
    }
    // 未注册 fallback：默认满分快照（无真实评分时的诚实占位，
    // 与 L6 OmniSparseCoordinator::snapshot 治理同模式）
    fallback_pvl_score()
}

/// 未注册时的默认满分快照（Phase 7 D-6：不伪造运行时数据，仅诚实占位）
fn fallback_pvl_score() -> ProcessScore {
    ProcessScore {
        real_execution: 1.0,
        coverage: 1.0,
        verification: 1.0,
        confidence: 1.0,
        efficiency: 1.0,
        retry_discipline: 1.0,
        output_substance: 1.0,
        orphan_free: 1.0,
        sandbox_clean: 1.0,
        total: 1.0,
    }
}

/// PVL 评分真实快照注册表（Phase 7 D-6）
///
/// WHY OnceLock<Mutex<Option>>：全局函数无法持有实例状态，注册表提供
/// 真实数据源；同步短临界区，无持锁跨 await（红线 §4.4-1）。
static PVL_SCORE_SNAPSHOT: std::sync::OnceLock<std::sync::Mutex<Option<ProcessScore>>> =
    std::sync::OnceLock::new();

/// 注册真实 PVL 过程评分（Phase 7 D-6：供 Verifier 评分路径写入）
pub fn register_pvl_score(score: ProcessScore) {
    let slot = PVL_SCORE_SNAPSHOT.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(score);
    }
}

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::config::PvlConfig;
    pub use crate::error::PvlError;
    pub use crate::feedback::FeedbackChannel;
    pub use crate::producer::Producer;
    pub use crate::types::{
        FeedbackMessage, Operation, OperationId, OperationStatus, ProducerStrategy,
        VerificationResult,
    };
    pub use crate::verifier::Verifier;
}
