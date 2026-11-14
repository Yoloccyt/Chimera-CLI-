//! LLM 客户端契约与循环数据类型（Phase 7，ADR-232）
//!
//! 对应架构层: L10 Interface（chimera-cli）
//!
//! # 为什么这层薄
//! 模型协议由 L10 `mca-gateway` 承载（`VendorAdapter`），工具执行由 L7
//! `gqep_executor::tool_plane::ToolRuntime` 承载。本模块只做一件事：把
//! "模型说要调工具 → 执行 → 结果回填 → 再问模型"这条多轮闭环的**契约**定下来，
//! 不另起第二条执行路径，也不新增事件变体（预算耗尽复用既有 `BudgetExceeded`）。
//!
//! WHY 用 `#[async_trait]` 而非方案 §5.7 草图的手工 `Pin<Box<dyn Future>>`：
//! `async-trait` 已是本 crate 的既有依赖，且 Phase 3 的 `ToolProvider` 实现同宏；
//! 手写 Box::pin 只增加噪声，"不新增生产依赖"这一目的两者等价。

use async_trait::async_trait;
use nexus_contracts::affinity::{AffinityRequest, AffinityResponse, UsageReport};
use thiserror::Error;

/// LLM 客户端契约 —— 一次补全请求
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// 客户端标识（进 `EventMetadata.source` 与日志，便于区分 mock / mca）
    fn provider_id(&self) -> &str;

    /// 一次补全往返
    ///
    /// WHY 传 `&AffinityRequest` 而非 owned：请求体含整段历史（多轮循环里逐轮
    /// 增长），按引用传避免每轮一次深拷；实现方需要留存时自行 clone。
    async fn complete(&self, request: &AffinityRequest) -> Result<AffinityResponse, LlmError>;
}

/// 客户端错误 —— 库层 thiserror（§4.1）
#[derive(Debug, Error)]
pub enum LlmError {
    /// provider 不可用（未配置 / 缺 API key / 装配失败）
    #[error("provider unavailable: {0}")]
    ProviderUnavailable(String),
    /// 传输层失败（网络 / 超时 / HTTP 状态）
    #[error("transport: {0}")]
    Transport(String),
    /// 协议/响应形状非法
    #[error("protocol: {0}")]
    Protocol(String),
    /// provider 侧预算拒绝（本地预算判定由循环给出 `LlmLoopStop::Budget`）
    #[error("budget exceeded: {0}")]
    BudgetExceeded(String),
}

/// 循环停止原因 —— 必须区分"模型说完了"与"被预算/上限掐断"
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmLoopStop {
    /// 模型返回无 ToolUse 的最终文本
    Final,
    /// 达到 `max_turns`
    MaxTurns,
    /// 达到 `max_tool_calls`
    MaxToolCalls,
    /// 同一 `tool + args` 重复次数超上限（模型打转的确定性信号）
    MaxRepeatToolCalls,
    /// token 预算耗尽（已发布 `BudgetExceeded`）
    Budget,
}

/// 循环产出
///
/// WHY 不派生 `Default`：`stop` 无"合法的未设置值"——让构造点显式给出，
/// 漏落停止原因就会编译报错，而不是静默伪装成 `Final`。
#[derive(Debug, Clone)]
pub struct LlmLoopOutcome {
    /// 最终文本（各轮 Text 块拼接）
    pub text: String,
    /// 实际进行的补全轮数
    pub turns: usize,
    /// 实际派发的工具调用数（含被审批拒绝而未执行的）
    pub tool_calls: usize,
    /// 累计 token 计量
    pub usage: UsageReport,
    /// 停止原因
    pub stop: LlmLoopStop,
    /// 因审批未通过而**未执行**的工具调用数（可观测：模型是否越界写）
    pub denied_tool_calls: usize,
}

/// 循环错误
#[derive(Debug, Error)]
pub enum LlmLoopError {
    /// 客户端失败（不可重试语义：直接上抛，由调用方决定降级）
    #[error("client: {0}")]
    Client(#[from] LlmError),
    /// 被取消 —— 携带已完成的轮数/调用数，便于审计"取消了多晚"
    #[error("cancelled after {turns} turns / {tool_calls} tool calls")]
    Cancelled {
        /// 已完成轮数
        turns: usize,
        /// 已派发工具调用数
        tool_calls: usize,
    },
}

/// 循环错误对应的终态停止原因（`Err` 路径也要能落 `stop` 语义时用）
impl LlmLoopError {
    /// 是否为取消（区分于客户端失败，供 CLI 决定退出码）
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled { .. })
    }
}
