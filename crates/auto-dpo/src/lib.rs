//! 自动偏好对生成 — 自动构造 DPO 训练所需的偏好对样本
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:无(知识层辅助模块,服务于 GSOE 进化闭环)
//!
//! # 核心职责
//! - 从模型输出候选中构造偏好对(chosen / rejected)
//! - 基于质量评分进行样本门控(过滤低质量样本)
//! - 通过 EventBus 发布 `DpoPairGenerated` 事件,供 GSOE/Parliament 消费
//!
//! # 依赖方向(§2.2 依赖铁律)
//! AutoDPO 是 L5 层,向下依赖 L1 的 event-bus。不向上依赖 L8 Parliament,
//! 议会共识通过订阅 `ConsensusReached` 事件传入(由调用方驱动)。
//!
//! # 事件集成(Ω-Event 定律)
//! 已集成 event-bus,发布以下事件:
//! - 偏好对生成 → `DpoPairGenerated` 事件(携带 pair_id / chosen / rejected)
//!
//! # 快速示例
//! ```
//! use auto_dpo::{PreferencePairGenerator, AutoDpoConfig, ModelOutput};
//!
//! let generator = PreferencePairGenerator::new(AutoDpoConfig::default()).unwrap();
//! let outputs = vec![
//!     ModelOutput::new("output-a", 0.9),
//!     ModelOutput::new("output-b", 0.3),
//! ];
//! let pair = generator.generate(&outputs).unwrap();
//! println!("chosen: {}, rejected: {}", pair.chosen, pair.rejected);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod config;
pub mod error;
/// FormalVerifier M1:偏好对一致性形式化验证(P7-T3,ADR-047)
///
/// R2 解冻阶段 1 前置:在 R2(GSOE×AutoDPO 约束 RL)解冻前,
/// 先形式化保证其训练数据源(偏好对)的一致性与反奖励黑客性质。
pub mod formal;
// 冻结违反处置:回滚守卫(尽力回滚 pending 更新;失败则经 Critical 旁路
// 发布 R2FreezeRollbackFailed,为该事件提供真实生产发布方)
pub mod freeze_guard;
pub mod generator;
pub mod rhi_channel_a;
pub mod rhi_judge_client;
// P5.1.3: 自比较历史持久化（wrap mlc-engine L2 SemanticMemory）
pub mod self_history;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use config::AutoDpoConfig;
pub use error::AutoDpoError;
// FormalVerifier M1:偏好对一致性验证器重导出(P7-T3)
pub use formal::PreferenceConsistencyChecker;
pub use generator::PreferencePairGenerator;
// P5.1.1: RHI-CG 通道 A 核心类型（JudgeClient trait + JudgeVerdict + RhiChannelA 编排器）
pub use rhi_channel_a::{JudgeClient, JudgeVerdict, RhiChannelA, SpecVersion, StubJudgeClient};
// P5.1.2: 评判器 LLM 调用接口（ModelRouterJudgeClient + LlmInvoker trait + StubLlmInvoker）
pub use rhi_judge_client::{
    FailingLlmInvoker, JudgeClientConfig, JudgePromptTemplate, JudgeResponseParser, LlmInvoker,
    LlmResponse, ModelRouterJudgeClient, StubLlmInvoker, TokenUsage,
};
// P5.1.3: 自比较历史持久化（SelfComparisonRecord + SelfComparisonHistory + 确定性 CLV 生成）
pub use self_history::{
    generate_deterministic_clv, SelfComparisonHistory, SelfComparisonRecord, DEFAULT_CAPACITY,
};
pub use types::{ModelOutput, PreferencePair, SampleQuality};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::config::AutoDpoConfig;
    pub use crate::error::AutoDpoError;
    pub use crate::generator::PreferencePairGenerator;
    pub use crate::rhi_channel_a::{
        JudgeClient, JudgeVerdict, RhiChannelA, SpecVersion, StubJudgeClient,
    };
    pub use crate::rhi_judge_client::{
        FailingLlmInvoker, JudgeClientConfig, JudgePromptTemplate, JudgeResponseParser, LlmInvoker,
        LlmResponse, ModelRouterJudgeClient, StubLlmInvoker, TokenUsage,
    };
    pub use crate::self_history::{
        generate_deterministic_clv, SelfComparisonHistory, SelfComparisonRecord, DEFAULT_CAPACITY,
    };
    pub use crate::types::{ModelOutput, PreferencePair, SampleQuality};
}
