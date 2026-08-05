//! RHI-CG 通道 A 评判器 — 基于 model-router 的 LLM 评判客户端（P5.1.2）
//!
//! 对应架构层: L5 Knowledge
//! 对应 ADR: ADR-032（双通道评估器决策 1）/ ADR-044（P5 工程实施）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.4（RHI-CG 双通道）
//! 对应任务: **P5.1.2**（评判器 LLM 调用接口经 model-router）
//!
//! # 核心职责
//!
//! 本模块实现 `JudgeClient` trait 的生产级实现 `ModelRouterJudgeClient`：
//! 1. 接收相邻 `HarnessSpec` 版本（v_i 与 v_{i-1}）
//! 2. 经 `model-router` 路由决策选择评估模型
//! 3. 构造评判 prompt（包含两版本规范化输入）
//! 4. 调用 `LlmInvoker` 执行实际 LLM 调用（HTTP/Stub）
//! 5. 解析 LLM JSON 响应为 `JudgeVerdict`
//!
//! # 架构位置
//!
//! ```text
//! auto-dpo (L5 Knowledge)
//!     │
//!     ├── nexus-contracts (L0)      ← HarnessSpec 类型
//!     ├── model-router (L1)         ← ModelRouter 路由决策
//!     │       └── nexus-core (L1)   ← UserIntent / MultimodalInput
//!     └── event-bus (L1)            ← 事件总线
//! ```
//!
//! 依赖铁律合规：L5 → L1/L0 均为向下依赖，符合 §2.2。
//!
//! # 设计决策（WHY）
//!
//! ## 1. LlmInvoker trait 接缝模式
//!
//! 项目当前未引入 `reqwest` / `hyper` HTTP 客户端依赖（workspace 全局无此包）。
//! 实际 LLM 调用由外部系统承担（如部署时的 HTTP gateway）。
//! 此 trait 提供 P5.1.2 的接缝，允许：
//! - 测试用 `StubLlmInvoker` 返回确定性响应（无需网络）
//! - 生产环境由调用方注入 HTTP-backed 实现（后续 P5.x 阶段补齐）
//! - 基准测试用 stub 避免外部依赖
//!
//! ## 2. JSON 协议契约
//!
//! 评判 prompt 要求 LLM 返回结构化 JSON：
//! ```json
//! {
//!   "winner": "current" | "previous",
//!   "winner_score": 0.0..=1.0,
//!   "loser_score": 0.0..=1.0,
//!   "confidence": 0.0..=1.0,
//!   "rationale": "非空字符串"
//! }
//! ```
//!
//! 解析失败（非 JSON / 字段缺失 / 越界）统一返回 `InvalidVerdict`。
//! 评判器调用失败（路由失败 / LLM 不可达）统一返回 `JudgeFailed`。
//!
//! ## 3. 路由请求构造
//!
//! 评判器调用经 model-router 路由，请求特征：
//! - `quest_id`: 命名空间 `rhi-judge-{v_i}-{v_i_minus_1}`，便于追踪
//! - `intent.raw_text`: 固定为 "spec evaluation"，标识评估类请求
//! - `estimated_tokens`: prompt 估算长度（保守 4096）
//! - `strategy`: 默认 `Auto`（综合最优），可通过 config 配置
//!
//! ## 4. 不可进化面保护（设计 §7.2）
//!
//! 评判器仅读取 `HarnessSpec::canonical_merkle_input()`，无写路径。
//! prompt 中包含的是 spec 的规范化字符串快照，无法注入文件路径或执行命令。
//!
//! ## 5. R2 冻结声明（ADR-042）
//!
//! 评判器仅生成偏好对（PreferencePair），不执行 R2 约束 RL 训练。
//! R2 路径（GSOE×AutoDPO）在 FormalVerifier 落地前完全冻结。

use crate::error::AutoDpoError;
use crate::rhi_channel_a::{JudgeClient, JudgeVerdict, SpecVersion};
use model_router::{ModelRouter, RoutingRequest, RoutingStrategy};
use nexus_contracts::affinity::ThinkingPreference;
use nexus_contracts::HarnessSpec;
use nexus_core::{MultimodalInput, UserIntent};
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

// ============================================================
// LlmInvoker trait — LLM 调用抽象接缝
// ============================================================

/// LLM 调用 trait — 抽象实际 LLM HTTP/gRPC 调用
///
/// # 实现契约
///
/// - 必须 `Send + Sync`（`ModelRouterJudgeClient` 可在 async 任务间共享）
/// - `invoke` 返回 `Pin<Box<dyn Future>>`，与 `JudgeClient::judge` 模式一致
/// - 实现可在内部使用 `tokio::spawn` 包装异步 HTTP 客户端
/// - 实现不应 panic（可能导致评判器不可用）
///
/// # 设计决策（WHY）
///
/// ## boxed Future 而非 async fn in trait
///
/// - 与 `JudgeClient::judge` 签名一致，降低认知负担
/// - 兼容 `dyn Trait` 对象安全
/// - LLM 调用延迟秒级，Box 堆分配开销（~50ns）可忽略
///
/// ## 返回 `LlmResponse` 而非裸 `String`
///
/// - 携带 `model_id` 便于审计追溯（评判使用了哪个模型）
/// - 携带 `TokenUsage` 供成本核算（CACR 集成准备）
/// - 携带 `content` 是 LLM 原始输出（JSON 字符串）
///
/// # 实现示例
///
/// - `StubLlmInvoker`：测试用，返回确定性 JSON 响应
/// - 未来 `HttpLlmInvoker`：基于 reqwest 的真实 HTTP 实现（后续 P5.x 阶段）
pub trait LlmInvoker: Send + Sync {
    /// 调用 LLM 生成响应
    ///
    /// # 参数
    /// - `model_id`: 目标模型 ID（由 `ModelRouter::route` 决策）
    /// - `prompt`: 评判 prompt（已格式化，包含两版本 spec）
    ///
    /// # 返回
    /// - `Ok(LlmResponse)`: LLM 调用成功，携带原始输出
    /// - `Err(AutoDpoError::JudgeFailed)`: LLM 不可达 / 超时 / HTTP 错误
    fn invoke<'a>(
        &'a self,
        model_id: &'a str,
        prompt: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse, AutoDpoError>> + Send + 'a>>;
}

/// LLM 响应 — 标准化响应载体
///
/// # 字段语义
///
/// | 字段 | 类型 | 含义 |
/// |------|------|------|
/// | `content` | String | LLM 原始输出（期望为 JSON 字符串） |
/// | `model_id` | String | 实际使用的模型 ID（便于审计） |
/// | `usage` | TokenUsage | token 消耗统计（成本核算） |
#[derive(Debug, Clone, PartialEq)]
pub struct LlmResponse {
    /// LLM 原始输出（期望为 JSON 字符串，由 `JudgeResponseParser` 解析）
    pub content: String,
    /// 实际使用的模型 ID（从 `RoutingDecision.model_id` 传入）
    pub model_id: String,
    /// token 消耗统计
    pub usage: TokenUsage,
}

/// Token 消耗统计 — 评判器调用的成本核算载体
///
/// # 设计决策
///
/// - 携带 `prompt_tokens` / `completion_tokens` 便于精确成本核算
/// - 未来 CACR 集成时，评判器调用成本应纳入预算管理
/// - 字段为 `u32`（足够覆盖 4M token 上下文，避免 u64 内存浪费）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenUsage {
    /// 输入 prompt 的 token 数
    pub prompt_tokens: u32,
    /// LLM 输出的 token 数
    pub completion_tokens: u32,
}

impl TokenUsage {
    /// 总 token 数 = prompt + completion
    pub fn total(&self) -> u32 {
        self.prompt_tokens + self.completion_tokens
    }
}

// ============================================================
// StubLlmInvoker — 测试用确定性 LLM 调用桩
// ============================================================

/// 响应工厂闭包类型别名 — 接收 (model_id, prompt) 返回 LlmResponse
///
/// WHY 类型别名: 避免 clippy `type_complexity` 警告（原始类型
/// `Arc<dyn Fn(&str, &str) -> LlmResponse + Send + Sync>` 过于复杂）。
/// 类型别名同时提升可读性，便于在签名中复用。
type ResponseFactory = Arc<dyn Fn(&str, &str) -> LlmResponse + Send + Sync>;

/// Stub LLM 调用器 — 测试与离线开发用
///
/// # 设计意图
///
/// 提供确定性的 JSON 响应，避免测试依赖外部 LLM 服务：
/// - 响应内容在构造时固定（通过闭包或预生成响应）
/// - 不模拟网络延迟（单元测试需要快速确定性）
/// - 不模拟失败（失败场景用 `FailingLlmInvoker`）
///
/// # 使用场景
///
/// - 单元测试：验证 `ModelRouterJudgeClient` 的编排逻辑
/// - 离线开发：在无 LLM 服务的环境下迭代评判器实现
/// - 基准测试：criterion bench 需要确定性输入
///
/// # 不变量
///
/// - `response_factory` 为 `Arc<dyn Fn>`，可在多线程间共享
/// - factory 接收 `(model_id, prompt)` 返回 `LlmResponse`，允许动态响应
pub struct StubLlmInvoker {
    /// 响应工厂闭包 — 接收 (model_id, prompt) 返回 LlmResponse
    ///
    /// WHY Arc<dyn Fn>:允许 factory 在 async 任务间共享（`&self` 即可调用）
    response_factory: ResponseFactory,
}

impl StubLlmInvoker {
    /// 创建 stub 调用器，使用固定的 JSON 响应
    ///
    /// # 参数
    /// - `response_json`: 固定返回的 JSON 字符串（content 字段）
    /// - `model_id`: 固定返回的模型 ID
    pub fn with_fixed_response(
        response_json: impl Into<String>,
        model_id: impl Into<String>,
    ) -> Self {
        let response_json = response_json.into();
        let model_id = model_id.into();
        let factory = move |_: &str, _: &str| LlmResponse {
            content: response_json.clone(),
            model_id: model_id.clone(),
            usage: TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
            },
        };
        Self {
            response_factory: Arc::new(factory),
        }
    }

    /// 创建 stub 调用器，使用动态响应工厂
    ///
    /// # 参数
    /// - `factory`: 闭包接收 `(model_id, prompt)` 返回 `LlmResponse`
    ///
    /// # 使用场景
    /// 根据输入 prompt 动态生成响应（如验证 prompt 格式）
    pub fn with_dynamic_response<F>(factory: F) -> Self
    where
        F: Fn(&str, &str) -> LlmResponse + Send + Sync + 'static,
    {
        Self {
            response_factory: Arc::new(factory),
        }
    }

    /// 创建一个总是裁决 Current 胜出的 stub（便捷构造器）
    ///
    /// 返回固定 JSON：`{"winner":"current","winner_score":0.85,"loser_score":0.45,"confidence":0.9,"rationale":"stub verdict"}`
    pub fn current_wins() -> Self {
        Self::with_fixed_response(
            r#"{"winner":"current","winner_score":0.85,"loser_score":0.45,"confidence":0.9,"rationale":"stub verdict: current version wins"}"#,
            "stub-judge-model",
        )
    }

    /// 创建一个总是裁决 Previous 胜出的 stub（便捷构造器，模拟通道 B 否决场景）
    pub fn previous_wins() -> Self {
        Self::with_fixed_response(
            r#"{"winner":"previous","winner_score":0.80,"loser_score":0.40,"confidence":0.9,"rationale":"stub verdict: previous version wins"}"#,
            "stub-judge-model",
        )
    }
}

impl LlmInvoker for StubLlmInvoker {
    fn invoke<'a>(
        &'a self,
        model_id: &'a str,
        prompt: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse, AutoDpoError>> + Send + 'a>> {
        Box::pin(async move {
            let response = (self.response_factory)(model_id, prompt);
            Ok(response)
        })
    }
}

/// 始终失败的 LLM 调用器 — 用于测试错误处理
///
/// # 设计意图
///
/// 与 `StubLlmInvoker` 互补：模拟 LLM 不可达场景，验证 `ModelRouterJudgeClient` 的错误传播。
pub struct FailingLlmInvoker {
    /// 失败原因（人类可读，如 "LLM service unreachable"）
    reason: String,
}

impl FailingLlmInvoker {
    /// 创建始终失败的 LLM 调用器
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl LlmInvoker for FailingLlmInvoker {
    fn invoke<'a>(
        &'a self,
        _model_id: &'a str,
        _prompt: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse, AutoDpoError>> + Send + 'a>> {
        let reason = self.reason.clone();
        Box::pin(async move { Err(AutoDpoError::JudgeFailed { reason }) })
    }
}

// ============================================================
// JudgePromptTemplate — 评判 prompt 构造
// ============================================================

/// 评判 prompt 模板 — 构造 LLM 评判输入
///
/// # 设计决策
///
/// - **模板化**: 固定结构，仅 spec 内容可变，避免 prompt 注入
/// - **JSON 协议要求**: 明确告知 LLM 返回 JSON 格式，降低解析失败率
/// - **版本号注入**: prompt 中包含版本号，便于 LLM 引用
///
/// # 不可变设计
///
/// 模板构造后不可修改（`&self` 调用 `format`），可在多线程间共享。
pub struct JudgePromptTemplate {
    /// 系统指令前缀（固定文本，可配置用于 A/B 测试）
    system_prefix: String,
}

impl JudgePromptTemplate {
    /// 创建默认 prompt 模板
    pub fn new() -> Self {
        Self::with_system_prefix(Self::default_system_prefix())
    }

    /// 创建带自定义系统指令前缀的模板
    ///
    /// # 使用场景
    /// - A/B 测试不同 prompt 策略
    /// - 多语言评判（中文/英文 prompt）
    pub fn with_system_prefix(system_prefix: impl Into<String>) -> Self {
        Self {
            system_prefix: system_prefix.into(),
        }
    }

    /// 默认系统指令前缀
    fn default_system_prefix() -> String {
        r#"You are an expert judge evaluating two versions of a system specification (HarnessSpec).

Your task: Compare the two spec versions and determine which is better based on:
1. Contract coverage (completeness of property/field declarations)
2. Hop structure clarity (explicit input/output types, order, veto/fallback)
3. Retry policy robustness (max_attempts, backoff strategy)
4. Spec discipline (no unnecessary complexity, clear naming)

Respond with ONLY a JSON object (no markdown, no explanation outside JSON):
{
  "winner": "current" or "previous",
  "winner_score": <float 0.0-1.0>,
  "loser_score": <float 0.0-1.0>,
  "confidence": <float 0.0-1.0>,
  "rationale": "<non-empty string explaining your verdict>"
}

Constraints:
- winner_score must be >= loser_score
- All scores must be in [0.0, 1.0]
- rationale must be non-empty
"#
        .to_string()
    }

    /// 格式化评判 prompt — 注入两版本 spec 的规范化输入
    ///
    /// # 参数
    /// - `spec_v_i`: 当前版本 spec（v_i，被提议的新版本）
    /// - `spec_v_i_minus_1`: 上一版本 spec（v_{i-1}，基线版本）
    ///
    /// # 返回
    /// 完整的 prompt 字符串，包含系统指令与两版本 spec 内容
    pub fn format(&self, spec_v_i: &HarnessSpec, spec_v_i_minus_1: &HarnessSpec) -> String {
        format!(
            "{system_prefix}\n\n## Current Version (v{v_i}):\n```\n{v_i_input}\n```\n\n## Previous Version (v{v_i_minus_1}):\n```\n{v_i_minus_1_input}\n```\n\nNow evaluate and respond with JSON only:",
            system_prefix = self.system_prefix,
            v_i = spec_v_i.meta.version,
            v_i_input = spec_v_i.canonical_merkle_input(),
            v_i_minus_1 = spec_v_i_minus_1.meta.version,
            v_i_minus_1_input = spec_v_i_minus_1.canonical_merkle_input(),
        )
    }
}

impl Default for JudgePromptTemplate {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// JudgeResponseParser — LLM 响应解析
// ============================================================

/// LLM 响应解析器 — 将 JSON 字符串解析为 `JudgeVerdict`
///
/// # 设计决策
///
/// - **纯函数无状态**: 解析器无内部状态，可在多线程间共享
/// - **serde_json 反序列化**: 利用 `#[derive(Deserialize)]` 自动派生
/// - **错误聚合**: 字段级错误聚合（一次报告所有缺失/越界字段）
///
/// # JSON 协议
///
/// 期望 LLM 返回如下 JSON 结构：
/// ```json
/// {
///   "winner": "current" | "previous",
///   "winner_score": 0.0..=1.0,
///   "loser_score": 0.0..=1.0,
///   "confidence": 0.0..=1.0,
///   "rationale": "非空字符串"
/// }
/// ```
pub struct JudgeResponseParser;

/// serde 反序列化用的中间结构
///
/// WHY 中间结构:LLM 返回的 JSON 字段名与 Rust 命名规范不同（snake_case），
/// 且 `winner` 为字符串而非枚举，需手动转换。
#[derive(Debug, Deserialize)]
struct RawJudgeResponse {
    winner: String,
    winner_score: f32,
    loser_score: f32,
    confidence: f32,
    rationale: String,
}

impl JudgeResponseParser {
    /// 解析 LLM 响应为 `JudgeVerdict`
    ///
    /// # 参数
    /// - `content`: LLM 原始输出（期望为 JSON 字符串）
    ///
    /// # 返回
    /// - `Ok(JudgeVerdict)`: 解析成功，已通过字段校验
    /// - `Err(AutoDpoError::InvalidVerdict)`: JSON 解析失败 / 字段缺失 / 越界
    ///
    /// # 解析流程
    /// 1. `serde_json::from_str` 反序列化为 `RawJudgeResponse`
    /// 2. 解析 `winner` 字符串为 `SpecVersion` 枚举
    /// 3. 调用 `JudgeVerdict::new` 执行字段校验（范围/逻辑）
    pub fn parse(content: &str) -> Result<JudgeVerdict, AutoDpoError> {
        // 步骤 1: JSON 反序列化
        let raw: RawJudgeResponse =
            serde_json::from_str(content).map_err(|e| AutoDpoError::InvalidVerdict {
                field: "json_parse".to_string(),
                value: format!("serde_json error: {e}"),
            })?;

        // 步骤 2: 解析 winner 字符串为 SpecVersion 枚举
        let winner = match raw.winner.as_str() {
            "current" => SpecVersion::Current,
            "previous" => SpecVersion::Previous,
            other => {
                return Err(AutoDpoError::InvalidVerdict {
                    field: "winner".to_string(),
                    value: format!("expected 'current' or 'previous', got '{other}'"),
                });
            }
        };

        // 步骤 3: 调用 JudgeVerdict::new 执行字段校验并构造
        JudgeVerdict::new(
            winner,
            raw.winner_score,
            raw.loser_score,
            raw.confidence,
            raw.rationale,
        )
    }
}

// ============================================================
// JudgeClientConfig — 评判器配置
// ============================================================

/// 评判器客户端配置
///
/// # 设计决策
///
/// - **配置集中**: 路由策略/quest_id 前缀/预估 token 集中管理
/// - **Builder 模式**: 通过 `Default` 提供合理默认，可通过字段更新覆盖
/// - **不可变**: 构造后不可修改（`ModelRouterJudgeClient` 持有 `&Config`）
///
/// # P1-4 重试与降级配置
///
/// - `max_retries`: JSON 解析失败最大重试次数（默认 2，含首次共 3 次尝试）
/// - `retry_delay_ms`: 重试间隔（毫秒）（默认 100ms）
/// - `fallback_on_parse_failure`: 重试耗尽后是否使用保守默认裁决（默认 true）
///
/// WHY 默认重试 2 次:LLM 响应解析失败通常是偶发（JSON 格式偶发异常），
/// 2 次重试在 99% 场景下足够。LLM 调用延迟为秒级，100ms 抖动很小。
#[derive(Debug, Clone)]
pub struct JudgeClientConfig {
    /// 路由策略（默认 `Auto`，综合最优）
    pub routing_strategy: RoutingStrategy,
    /// quest_id 前缀（默认 "rhi-judge"，便于事件追踪）
    pub quest_id_prefix: String,
    /// 预估 token 数（默认 4096，保守估计 prompt + completion）
    pub estimated_tokens: u32,

    // ============================================================
    // P1-4: LLM Judge 响应解析降级策略（重试 + 默认裁决）
    // ============================================================
    /// JSON 解析失败最大重试次数（默认 2，含首次共 3 次尝试）
    pub max_retries: u32,
    /// 重试间隔（毫秒）（默认 100ms）
    ///
    /// WHY 100ms:LLM 调用延迟为秒级，100ms 相对很小，不会显著增加总延迟。
    /// 线性递增:第 N 次重试前等待 N × retry_delay_ms，避免重试风暴。
    pub retry_delay_ms: u64,
    /// 重试耗尽后是否使用保守默认裁决（默认 true）
    ///
    /// WHY true:LLM 不可用不应阻塞通道 A 的提议流程。保守默认裁决
    /// （Previous 胜出、中性评分、零置信度）确保系统在 LLM 异常时
    /// 保持"偏向保守"的行为，而不是直接报错中断整个流程。
    pub fallback_on_parse_failure: bool,
}

impl Default for JudgeClientConfig {
    fn default() -> Self {
        Self {
            routing_strategy: RoutingStrategy::Auto,
            quest_id_prefix: "rhi-judge".to_string(),
            estimated_tokens: 4096,
            // P1-4: 默认重试 2 次（共 3 次尝试），100ms 间隔，启用降级
            max_retries: 2,
            retry_delay_ms: 100,
            fallback_on_parse_failure: true,
        }
    }
}

// ============================================================
// ModelRouterJudgeClient — 生产级评判器客户端
// ============================================================

/// 基于 model-router 的评判器客户端 — `JudgeClient` 生产级实现
///
/// # 架构位置
///
/// ```text
/// judge(spec_v_i, spec_v_i_minus_1)
///     │
///     ▼
/// ┌──────────────────────────────────┐
/// │ ModelRouterJudgeClient            │
/// │   1. 构造 RoutingRequest          │
/// │   2. router.route() ──────────────┼──> ModelRouter (L1)
/// │   3. 构造 prompt (template.format)│
/// │   4. invoker.invoke() ───────────┼──> LlmInvoker (trait)
/// │   5. parser.parse(content)       │
/// │   6. 返回 JudgeVerdict            │
/// └──────────────────────────────────┘
/// ```
///
/// # 线程安全
///
/// - `router` 为 `Arc<ModelRouter>`，可在 async 任务间共享
/// - `invoker` 为 `Arc<dyn LlmInvoker>`，trait object 共享
/// - `prompt_template` 为 `JudgePromptTemplate`，无内部可变状态
/// - `config` 为值类型配置，`Clone` 廉价
///
/// # 使用示例
///
/// ```rust,ignore
/// use auto_dpo::{JudgeClient, rhi_judge_client::*};
/// use model_router::{ModelRouter, ModelRegistry, config::RouterConfig};
/// use std::sync::Arc;
/// use event_bus::EventBus;
///
/// # async fn run() {
/// let bus = EventBus::new();
/// let registry = ModelRegistry::from_config(&RouterConfig::default());
/// let router = Arc::new(ModelRouter::new(registry, bus));
/// let invoker = Arc::new(StubLlmInvoker::current_wins());
///
/// let client = ModelRouterJudgeClient::new(router, invoker);
/// // let verdict = client.judge(&spec_v_i, &spec_v_i_minus_1).await.unwrap();
/// # }
/// ```
pub struct ModelRouterJudgeClient {
    /// 模型路由器（共享 Arc，可被多个评判器实例复用）
    router: Arc<ModelRouter>,
    /// LLM 调用器（trait object，stub 或未来 HTTP 实现）
    invoker: Arc<dyn LlmInvoker>,
    /// 评判 prompt 模板
    prompt_template: JudgePromptTemplate,
    /// 客户端配置
    config: JudgeClientConfig,
}

impl ModelRouterJudgeClient {
    /// 创建评判器客户端，使用默认配置
    ///
    /// # 参数
    /// - `router`: 模型路由器（共享 Arc）
    /// - `invoker`: LLM 调用器（trait object）
    pub fn new(router: Arc<ModelRouter>, invoker: Arc<dyn LlmInvoker>) -> Self {
        Self::with_config(
            router,
            invoker,
            JudgePromptTemplate::default(),
            JudgeClientConfig::default(),
        )
    }

    /// 创建带自定义配置的评判器客户端
    ///
    /// # 参数
    /// - `router`: 模型路由器
    /// - `invoker`: LLM 调用器
    /// - `prompt_template`: prompt 模板
    /// - `config`: 客户端配置
    pub fn with_config(
        router: Arc<ModelRouter>,
        invoker: Arc<dyn LlmInvoker>,
        prompt_template: JudgePromptTemplate,
        config: JudgeClientConfig,
    ) -> Self {
        Self {
            router,
            invoker,
            prompt_template,
            config,
        }
    }

    /// 构造保守默认裁决 — 重试耗尽时的降级方案
    ///
    /// # 返回
    ///
    /// 返回 Previous 胜出、中性评分 0.5/0.5、零置信度的默认裁决。
    ///
    /// # 设计决策（WHY）
    ///
    /// - **Previous 胜出**:保守策略，LLM 不可用时偏向保留现有版本。
    ///   通道 B（CI 否决）仍会独立验证，不会因保守默认裁决而引入退化。
    /// - **中性评分 0.5/0.5**:不引入偏好信号，下游可据此识别降级裁决。
    /// - **零置信度**:0.0 置信度明确告知下游"此裁决无可靠依据"。
    /// - **rationale 标记**:包含 "fallback" 关键词，便于审计追溯。
    fn fallback_verdict() -> JudgeVerdict {
        JudgeVerdict::new(
            SpecVersion::Previous,
            0.5, // winner_score（中性）
            0.5, // loser_score（中性，平局）
            0.0, // confidence（零置信度）
            "fallback: LLM judge response parse failed after all retries".to_string(),
        )
        .expect("fallback verdict: hardcoded valid values should not fail")
    }

    /// 构造路由请求 — 评判器的路由特征
    ///
    /// # 设计决策
    ///
    /// - `quest_id`: 命名空间 `rhi-judge-{v_i}-{v_i_minus_1}`，便于事件追踪
    /// - `intent.raw_text`: 固定 "spec evaluation"，标识评估类请求
    /// - `risk_level`: 10（低风险，评估类请求不涉及命令执行）
    fn build_routing_request(
        &self,
        spec_v_i: &HarnessSpec,
        spec_v_i_minus_1: &HarnessSpec,
    ) -> RoutingRequest {
        let quest_id = format!(
            "{}-{}-{}",
            self.config.quest_id_prefix, spec_v_i.meta.version, spec_v_i_minus_1.meta.version
        );

        RoutingRequest {
            quest_id,
            intent: UserIntent {
                intent_id: format!(
                    "rhi-judge-intent-{}-{}",
                    spec_v_i.meta.version, spec_v_i_minus_1.meta.version
                ),
                raw_text: "spec evaluation".to_string(),
                multimodal_inputs: vec![MultimodalInput::Text("evaluate spec versions".into())],
                risk_level: 10,
            },
            estimated_tokens: self.config.estimated_tokens,
            strategy: self.config.routing_strategy,
            // MCA P2:rhi 评判器使用标准思考模式(平衡延迟与质量)
            thinking_pref: ThinkingPreference::Standard,
        }
    }
}

impl JudgeClient for ModelRouterJudgeClient {
    /// 评判相邻 spec 版本，经 model-router 路由 + LLM 调用
    ///
    /// # 流程
    /// 1. 构造 `RoutingRequest`（quest_id 命名空间 `rhi-judge-*`）
    /// 2. 调用 `router.route()` 获取 `RoutingDecision`（含 model_id）
    /// 3. 调用 `prompt_template.format()` 构造评判 prompt
    /// 4. - 5. 重试循环：调用 LLM + 解析 JSON，最多 `max_retries + 1` 次尝试
    ///    - LLM 调用失败（网络/超时）→ 不重试，直接传播错误
    ///    - JSON 解析失败 → 重试（线性递增等待间隔）
    /// 6. 重试耗尽：
    ///    - `fallback_on_parse_failure = true` → 返回保守默认裁决
    ///    - `fallback_on_parse_failure = false` → 返回最后一次解析错误
    ///
    /// # P1-4 重试设计（WHY）
    ///
    /// - **仅重试 Invoke+Parsing**:路由决策（步骤 2）不在重试范围内。
    ///   路由失败通常是配置问题，重试无意义。
    /// - **LLM 调用失败不重试**:网络/超时是基础设施问题，重试可能立即再次失败。
    ///   唯一的例外是 JSON 解析失败，这可能是 LLM 偶发输出格式异常，重试可缓解。
    /// - **线性递增等待**:第 N 次重试前等待 N × retry_delay_ms，避免重试风暴。
    /// - **降级默认裁决**:Previous 胜出 + 中性评分 + 零置信度，确保系统保守运行。
    ///
    /// # 错误传播
    /// - 路由失败（如空注册表）→ `JudgeFailed`（reason 携带 RouterError 详情）
    /// - LLM 调用失败 → `JudgeFailed`（reason 来自 LlmInvoker，不重试）
    /// - 重试耗尽且 `fallback_on_parse_failure = false` → `InvalidVerdict`
    fn judge<'a>(
        &'a self,
        spec_v_i: &'a HarnessSpec,
        spec_v_i_minus_1: &'a HarnessSpec,
    ) -> Pin<Box<dyn Future<Output = Result<JudgeVerdict, AutoDpoError>> + Send + 'a>> {
        // 预计算路由请求与 prompt（在闭包外捕获，避免 'a 生命周期问题）
        let request = self.build_routing_request(spec_v_i, spec_v_i_minus_1);
        let prompt = self.prompt_template.format(spec_v_i, spec_v_i_minus_1);
        let max_retries = self.config.max_retries;
        let retry_delay_ms = self.config.retry_delay_ms;
        let fallback_on_parse_failure = self.config.fallback_on_parse_failure;

        Box::pin(async move {
            // 步骤 1: 调用路由器（model-router 经路由决策选择评估模型）
            let decision =
                self.router
                    .route(request)
                    .await
                    .map_err(|e| AutoDpoError::JudgeFailed {
                        reason: format!("model-router routing failed: {e:?}"),
                    })?;

            let model_id = decision.model_id.clone();

            // 步骤 2-5: 重试循环 — 调用 LLM + 解析，最多重试 max_retries 次
            let mut last_parse_err: Option<AutoDpoError> = None;
            let total_attempts = max_retries + 1; // 首次 + max_retries 次重试
            for attempt in 0..total_attempts {
                if attempt > 0 {
                    // 线性递增等待：第 N 次重试前等待 N × retry_delay_ms
                    // WHY 线性递增:固定间隔在重试次数多时可能造成同时爆发，
                    // 指数退避对 LLM 调用（秒级延迟）过于激进。线性递增简单有效。
                    let delay = retry_delay_ms * attempt as u64;
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }

                // 步骤 2: 调用 LLM（经 LlmInvoker trait）
                let response = match self.invoker.invoke(&model_id, &prompt).await {
                    Ok(r) => r,
                    Err(e) => {
                        // LLM 调用失败（网络/超时）不重试——基础设施问题
                        tracing::warn!(
                            model_id = %model_id,
                            attempt = attempt + 1,
                            total = total_attempts,
                            error = %e,
                            "RHI-CG channel A judge: LLM invocation failed"
                        );
                        return Err(e);
                    }
                };

                // 步骤 3: 解析 LLM 响应为 JudgeVerdict
                match JudgeResponseParser::parse(&response.content) {
                    Ok(verdict) => {
                        tracing::info!(
                            model_id = %response.model_id,
                            attempt = attempt + 1,
                            total = total_attempts,
                            prompt_tokens = response.usage.prompt_tokens,
                            completion_tokens = response.usage.completion_tokens,
                            winner = %verdict.winner,
                            confidence = verdict.confidence,
                            "RHI-CG channel A judge: LLM evaluation completed"
                        );
                        return Ok(verdict);
                    }
                    Err(e) => {
                        // 解析失败：记录警告并重试
                        tracing::warn!(
                            model_id = %response.model_id,
                            attempt = attempt + 1,
                            total = total_attempts,
                            error = %e,
                            "RHI-CG channel A judge: response parse failed"
                        );
                        last_parse_err = Some(e);
                    }
                }
            }

            // 步骤 4: 重试耗尽——使用降级策略
            if fallback_on_parse_failure {
                tracing::warn!(
                    model_id = %model_id,
                    max_retries = max_retries,
                    "RHI-CG channel A judge: all retries exhausted, using fallback verdict"
                );
                Ok(Self::fallback_verdict())
            } else {
                Err(
                    last_parse_err.unwrap_or_else(|| AutoDpoError::InvalidVerdict {
                        field: "json_parse".to_string(),
                        value: "retry exhausted without parse error (unreachable)".to_string(),
                    }),
                )
            }
        })
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rhi_channel_a::JudgeClient;
    use model_router::{ModelRegistry, RouterConfig};
    use nexus_contracts::{ContractSpec, HarnessMeta, HopSpec, RetryPolicy};

    /// 构造最小合法 HarnessSpec 用于测试
    fn make_test_spec(version: u32, name_suffix: &str) -> HarnessSpec {
        HarnessSpec {
            meta: HarnessMeta {
                name: format!("rhi-test-{name_suffix}"),
                version,
                immutable: false,
                parent: if version > 1 { Some(version - 1) } else { None },
                task_type: Some("code_refactor".to_string()),
            },
            contracts: vec![ContractSpec {
                name: "no_panic".to_string(),
                property: "must_not_panic".to_string(),
                description: None,
                from: None,
                to: None,
                fields: Vec::new(),
            }],
            hops: vec![HopSpec {
                name: "execute".to_string(),
                input_type: None,
                output_type: None,
                contracts: Vec::new(),
                description: None,
                order: Vec::new(),
                on_veto: None,
                fallback: None,
            }],
            retry: RetryPolicy::default(),
            auxiliary: None,
        }
    }

    /// 构造测试用 ModelRouter（使用默认 RouterConfig，包含 lite-model 等）
    fn make_test_router() -> Arc<ModelRouter> {
        let bus = event_bus::EventBus::new();
        let registry = ModelRegistry::from_config(&RouterConfig::default());
        Arc::new(ModelRouter::new(registry, bus))
    }

    // ============================================================
    // TokenUsage 测试
    // ============================================================

    #[test]
    fn test_token_usage_total() {
        let usage = TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
        };
        assert_eq!(usage.total(), 150);
    }

    #[test]
    fn test_token_usage_zero_total() {
        let usage = TokenUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
        };
        assert_eq!(usage.total(), 0);
    }

    // ============================================================
    // StubLlmInvoker 测试
    // ============================================================

    #[tokio::test]
    async fn test_stub_llm_invoker_fixed_response() {
        let invoker = StubLlmInvoker::with_fixed_response(
            r#"{"winner":"current","winner_score":0.9,"loser_score":0.3,"confidence":0.85,"rationale":"test"}"#,
            "test-model",
        );

        let response = invoker.invoke("model-1", "test prompt").await.unwrap();
        assert_eq!(response.model_id, "test-model");
        assert!(response.content.contains("current"));
        assert_eq!(response.usage.prompt_tokens, 100);
        assert_eq!(response.usage.completion_tokens, 50);
    }

    #[tokio::test]
    async fn test_stub_llm_invoker_current_wins() {
        let invoker = StubLlmInvoker::current_wins();
        let response = invoker.invoke("any-model", "any-prompt").await.unwrap();
        assert!(response.content.contains("\"current\""));
        assert!(response.content.contains("0.85"));
    }

    #[tokio::test]
    async fn test_stub_llm_invoker_previous_wins() {
        let invoker = StubLlmInvoker::previous_wins();
        let response = invoker.invoke("any-model", "any-prompt").await.unwrap();
        assert!(response.content.contains("\"previous\""));
        assert!(response.content.contains("0.80"));
    }

    #[tokio::test]
    async fn test_stub_llm_invoker_dynamic_response() {
        let invoker = StubLlmInvoker::with_dynamic_response(|model_id, _prompt| LlmResponse {
            content: format!(
                r#"{{"winner":"current","winner_score":0.9,"loser_score":0.3,"confidence":0.9,"rationale":"evaluated by {model_id}"}}"#
            ),
            model_id: model_id.to_string(),
            usage: TokenUsage {
                prompt_tokens: 200,
                completion_tokens: 80,
            },
        });

        let response = invoker.invoke("gpt-4o", "prompt").await.unwrap();
        assert_eq!(response.model_id, "gpt-4o");
        assert!(response.content.contains("evaluated by gpt-4o"));
        assert_eq!(response.usage.prompt_tokens, 200);
    }

    #[tokio::test]
    async fn test_failing_llm_invoker_returns_judge_failed() {
        let invoker = FailingLlmInvoker::new("LLM service unreachable");
        let result = invoker.invoke("any-model", "any-prompt").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            AutoDpoError::JudgeFailed { reason } => {
                assert_eq!(reason, "LLM service unreachable");
            }
            other => panic!("期望 JudgeFailed，实际: {other:?}"),
        }
    }

    // ============================================================
    // JudgePromptTemplate 测试
    // ============================================================

    #[test]
    fn test_prompt_template_default_contains_system_prefix() {
        let template = JudgePromptTemplate::default();
        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");
        let prompt = template.format(&spec_v_i, &spec_v_i_minus_1);

        assert!(prompt.contains("expert judge"));
        assert!(prompt.contains("Current Version (v2)"));
        assert!(prompt.contains("Previous Version (v1)"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn test_prompt_template_includes_spec_content() {
        let template = JudgePromptTemplate::default();
        let spec_v_i = make_test_spec(5, "v5");
        let spec_v_i_minus_1 = make_test_spec(4, "v4");
        let prompt = template.format(&spec_v_i, &spec_v_i_minus_1);

        // 验证 spec 的 canonical_merkle_input 被注入
        assert!(prompt.contains("meta.name=rhi-test-v5"));
        assert!(prompt.contains("meta.name=rhi-test-v4"));
        assert!(prompt.contains("meta.version=5"));
        assert!(prompt.contains("meta.version=4"));
    }

    #[test]
    fn test_prompt_template_custom_system_prefix() {
        let template =
            JudgePromptTemplate::with_system_prefix("Custom judge instruction for testing.");
        let spec_v_i = make_test_spec(1, "v1");
        let spec_v_i_minus_1 = make_test_spec(0, "v0");
        let prompt = template.format(&spec_v_i, &spec_v_i_minus_1);

        assert!(prompt.starts_with("Custom judge instruction for testing."));
    }

    // ============================================================
    // JudgeResponseParser 测试
    // ============================================================

    #[test]
    fn test_parser_valid_current_wins() {
        let content = r#"{"winner":"current","winner_score":0.85,"loser_score":0.45,"confidence":0.9,"rationale":"v2 has better coverage"}"#;
        let verdict = JudgeResponseParser::parse(content).unwrap();
        assert_eq!(verdict.winner, SpecVersion::Current);
        assert!((verdict.winner_score - 0.85).abs() < 1e-6);
        assert!((verdict.loser_score - 0.45).abs() < 1e-6);
        assert!((verdict.confidence - 0.9).abs() < 1e-6);
        assert_eq!(verdict.rationale, "v2 has better coverage");
    }

    #[test]
    fn test_parser_valid_previous_wins() {
        let content = r#"{"winner":"previous","winner_score":0.80,"loser_score":0.40,"confidence":0.85,"rationale":"v1 is more stable"}"#;
        let verdict = JudgeResponseParser::parse(content).unwrap();
        assert_eq!(verdict.winner, SpecVersion::Previous);
        assert!((verdict.winner_score - 0.80).abs() < 1e-6);
    }

    #[test]
    fn test_parser_invalid_json_returns_invalid_verdict() {
        let content = "this is not json";
        let result = JudgeResponseParser::parse(content);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            AutoDpoError::InvalidVerdict { field, .. } => {
                assert_eq!(field, "json_parse");
            }
            other => panic!("期望 InvalidVerdict，实际: {other:?}"),
        }
    }

    #[test]
    fn test_parser_invalid_winner_value_returns_invalid_verdict() {
        let content = r#"{"winner":"invalid","winner_score":0.8,"loser_score":0.4,"confidence":0.9,"rationale":"test"}"#;
        let result = JudgeResponseParser::parse(content);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            AutoDpoError::InvalidVerdict { field, value } => {
                assert_eq!(field, "winner");
                assert!(value.contains("invalid"));
            }
            other => panic!("期望 InvalidVerdict (winner)，实际: {other:?}"),
        }
    }

    #[test]
    fn test_parser_missing_field_returns_invalid_verdict() {
        // 缺少 rationale 字段
        let content =
            r#"{"winner":"current","winner_score":0.8,"loser_score":0.4,"confidence":0.9}"#;
        let result = JudgeResponseParser::parse(content);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            AutoDpoError::InvalidVerdict { field, .. } => {
                assert_eq!(field, "json_parse");
            }
            other => panic!("期望 InvalidVerdict (json_parse)，实际: {other:?}"),
        }
    }

    #[test]
    fn test_parser_out_of_range_score_returns_invalid_verdict() {
        // winner_score > 1.0
        let content = r#"{"winner":"current","winner_score":1.5,"loser_score":0.4,"confidence":0.9,"rationale":"test"}"#;
        let result = JudgeResponseParser::parse(content);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            AutoDpoError::InvalidVerdict { field, .. } => {
                assert_eq!(field, "winner_score");
            }
            other => panic!("期望 InvalidVerdict (winner_score)，实际: {other:?}"),
        }
    }

    #[test]
    fn test_parser_empty_rationale_returns_invalid_verdict() {
        let content = r#"{"winner":"current","winner_score":0.8,"loser_score":0.4,"confidence":0.9,"rationale":""}"#;
        let result = JudgeResponseParser::parse(content);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            AutoDpoError::InvalidVerdict { field, .. } => {
                assert_eq!(field, "rationale");
            }
            other => panic!("期望 InvalidVerdict (rationale)，实际: {other:?}"),
        }
    }

    #[test]
    fn test_parser_winner_score_below_loser_returns_invalid_verdict() {
        let content = r#"{"winner":"current","winner_score":0.3,"loser_score":0.8,"confidence":0.9,"rationale":"test"}"#;
        let result = JudgeResponseParser::parse(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_parser_tie_allowed() {
        // winner_score == loser_score 应允许（平局场景）
        let content = r#"{"winner":"current","winner_score":0.7,"loser_score":0.7,"confidence":0.5,"rationale":"tie"}"#;
        let verdict = JudgeResponseParser::parse(content).unwrap();
        assert!((verdict.score_gap() - 0.0).abs() < 1e-6);
    }

    // ============================================================
    // JudgeClientConfig 测试
    // ============================================================

    #[test]
    fn test_judge_client_config_default() {
        let config = JudgeClientConfig::default();
        assert_eq!(config.routing_strategy, RoutingStrategy::Auto);
        assert_eq!(config.quest_id_prefix, "rhi-judge");
        assert_eq!(config.estimated_tokens, 4096);
        // P1-4: 验证重试配置默认值
        assert_eq!(config.max_retries, 2, "默认重试 2 次");
        assert_eq!(config.retry_delay_ms, 100, "默认重试间隔 100ms");
        assert!(config.fallback_on_parse_failure, "默认启用降级裁决");
    }

    // ============================================================
    // P1-4: 重试与降级策略测试
    // ============================================================

    /// 构造一个动态 LLM 调用器，前 `fail_count` 次调用返回非法 JSON，
    /// 之后返回合法 JSON（裁决 Current 胜出）
    ///
    /// WHY 使用 `Arc<AtomicU32>`:闭包是 `Fn`（非 `FnMut`），
    /// 需通过原子变量实现可变的调用计数。
    fn make_retryable_invoker(fail_count: u32) -> StubLlmInvoker {
        use std::sync::atomic::{AtomicU32, Ordering};
        let call_counter = Arc::new(AtomicU32::new(0));
        let fail_count_arc = Arc::new(fail_count);

        StubLlmInvoker::with_dynamic_response(move |_model_id, _prompt| {
            let current = call_counter.fetch_add(1, Ordering::SeqCst);
            if current < *fail_count_arc {
                // 返回非法 JSON（解析失败）
                LlmResponse {
                    content: "this is not valid json".to_string(),
                    model_id: "retry-model".to_string(),
                    usage: TokenUsage {
                        prompt_tokens: 100,
                        completion_tokens: 10,
                    },
                }
            } else {
                // 返回合法 JSON
                LlmResponse {
                    content: r#"{"winner":"current","winner_score":0.85,"loser_score":0.45,"confidence":0.9,"rationale":"retry succeeded"}"#.to_string(),
                    model_id: "retry-model".to_string(),
                    usage: TokenUsage {
                        prompt_tokens: 100,
                        completion_tokens: 50,
                    },
                }
            }
        })
    }

    #[tokio::test]
    async fn test_judge_retry_success_after_parse_failure() {
        // 前 1 次调用失败，第 2 次成功（使用默认配置 max_retries=2）
        let router = make_test_router();
        let invoker = Arc::new(make_retryable_invoker(1));
        let config = JudgeClientConfig {
            max_retries: 2,
            retry_delay_ms: 1, // 1ms 避免测试延迟
            ..Default::default()
        };
        let client = ModelRouterJudgeClient::with_config(
            router,
            invoker,
            JudgePromptTemplate::default(),
            config,
        );

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let verdict = client.judge(&spec_v_i, &spec_v_i_minus_1).await.unwrap();
        assert_eq!(verdict.winner, SpecVersion::Current);
        assert!((verdict.winner_score - 0.85).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_judge_retry_exhausted_fallback() {
        // 所有 3 次尝试都失败（max_retries=2，共 3 次），使用降级默认裁决
        let router = make_test_router();
        let invoker = Arc::new(make_retryable_invoker(3)); // 前 3 次都失败
        let config = JudgeClientConfig {
            max_retries: 2,
            retry_delay_ms: 1,
            fallback_on_parse_failure: true,
            ..Default::default()
        };
        let client = ModelRouterJudgeClient::with_config(
            router,
            invoker,
            JudgePromptTemplate::default(),
            config,
        );

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let verdict = client.judge(&spec_v_i, &spec_v_i_minus_1).await.unwrap();
        // 降级裁决：Previous 胜出，中性评分，零置信度
        assert_eq!(verdict.winner, SpecVersion::Previous);
        assert!((verdict.winner_score - 0.5).abs() < 1e-6);
        assert!((verdict.loser_score - 0.5).abs() < 1e-6);
        assert!((verdict.confidence - 0.0).abs() < 1e-6);
        assert!(verdict.rationale.contains("fallback"));
    }

    #[tokio::test]
    async fn test_judge_retry_exhausted_no_fallback() {
        // 所有重试都失败，且 fallback=false，返回解析错误
        let router = make_test_router();
        let invoker = Arc::new(make_retryable_invoker(3));
        let config = JudgeClientConfig {
            max_retries: 2,
            retry_delay_ms: 1,
            fallback_on_parse_failure: false,
            ..Default::default()
        };
        let client = ModelRouterJudgeClient::with_config(
            router,
            invoker,
            JudgePromptTemplate::default(),
            config,
        );

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let result = client.judge(&spec_v_i, &spec_v_i_minus_1).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            AutoDpoError::InvalidVerdict { field, .. } => {
                assert_eq!(field, "json_parse");
            }
            other => panic!("期望 InvalidVerdict，实际: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_judge_invoker_failure_no_retry() {
        // LLM 调用失败（网络/超时）→ 不重试，直接传播错误
        let router = make_test_router();
        let invoker = Arc::new(FailingLlmInvoker::new("network timeout"));
        let config = JudgeClientConfig {
            max_retries: 3, // 即使配置了重试，LLM 调用失败也不重试
            retry_delay_ms: 1,
            ..Default::default()
        };
        let client = ModelRouterJudgeClient::with_config(
            router,
            invoker,
            JudgePromptTemplate::default(),
            config,
        );

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let result = client.judge(&spec_v_i, &spec_v_i_minus_1).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AutoDpoError::JudgeFailed { reason } => {
                assert_eq!(reason, "network timeout");
            }
            other => panic!("期望 JudgeFailed，实际: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_judge_retry_zero_retries_fallback() {
        // max_retries=0：仅尝试 1 次，失败后立即降级
        let router = make_test_router();
        let invoker = Arc::new(make_retryable_invoker(1)); // 第 1 次就失败
        let config = JudgeClientConfig {
            max_retries: 0,
            retry_delay_ms: 1,
            fallback_on_parse_failure: true,
            ..Default::default()
        };
        let client = ModelRouterJudgeClient::with_config(
            router,
            invoker,
            JudgePromptTemplate::default(),
            config,
        );

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let verdict = client.judge(&spec_v_i, &spec_v_i_minus_1).await.unwrap();
        // 降级裁决
        assert_eq!(verdict.winner, SpecVersion::Previous);
        assert!(verdict.rationale.contains("fallback"));
    }

    #[test]
    fn test_fallback_verdict_values() {
        // 验证 fallback_verdict 的硬编码值合法
        let verdict = ModelRouterJudgeClient::fallback_verdict();
        assert_eq!(verdict.winner, SpecVersion::Previous);
        assert!((verdict.winner_score - 0.5).abs() < 1e-6);
        assert!((verdict.loser_score - 0.5).abs() < 1e-6);
        assert!((verdict.confidence - 0.0).abs() < 1e-6);
        assert!(verdict.rationale.contains("fallback"));
        // 验证不变量：winner_score >= loser_score
        assert!(verdict.winner_score >= verdict.loser_score);
    }

    // ============================================================
    // ModelRouterJudgeClient 集成测试
    // ============================================================

    #[tokio::test]
    async fn test_model_router_judge_client_current_wins() {
        let router = make_test_router();
        let invoker = Arc::new(StubLlmInvoker::current_wins());
        let client = ModelRouterJudgeClient::new(router, invoker);

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let verdict = client.judge(&spec_v_i, &spec_v_i_minus_1).await.unwrap();
        assert_eq!(verdict.winner, SpecVersion::Current);
        assert!((verdict.winner_score - 0.85).abs() < 1e-6);
        assert!((verdict.loser_score - 0.45).abs() < 1e-6);
        assert!((verdict.confidence - 0.9).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_model_router_judge_client_previous_wins() {
        let router = make_test_router();
        let invoker = Arc::new(StubLlmInvoker::previous_wins());
        let client = ModelRouterJudgeClient::new(router, invoker);

        let spec_v_i = make_test_spec(3, "v3");
        let spec_v_i_minus_1 = make_test_spec(2, "v2");

        let verdict = client.judge(&spec_v_i, &spec_v_i_minus_1).await.unwrap();
        assert_eq!(verdict.winner, SpecVersion::Previous);
        assert!((verdict.winner_score - 0.80).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_model_router_judge_client_router_failure_propagates() {
        // 空注册表导致路由失败
        let bus = event_bus::EventBus::new();
        let empty_registry = ModelRegistry::new();
        let router = Arc::new(ModelRouter::new(empty_registry, bus));
        let invoker = Arc::new(StubLlmInvoker::current_wins());
        let client = ModelRouterJudgeClient::new(router, invoker);

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let result = client.judge(&spec_v_i, &spec_v_i_minus_1).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            AutoDpoError::JudgeFailed { reason } => {
                assert!(reason.contains("model-router routing failed"));
                assert!(reason.contains("NoModelsRegistered"));
            }
            other => panic!("期望 JudgeFailed，实际: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_model_router_judge_client_invoker_failure_propagates() {
        let router = make_test_router();
        let invoker = Arc::new(FailingLlmInvoker::new("network timeout"));
        let client = ModelRouterJudgeClient::new(router, invoker);

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let result = client.judge(&spec_v_i, &spec_v_i_minus_1).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            AutoDpoError::JudgeFailed { reason } => {
                assert_eq!(reason, "network timeout");
            }
            other => panic!("期望 JudgeFailed，实际: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_model_router_judge_client_invalid_response_propagates() {
        let router = make_test_router();
        // 返回非 JSON 响应；使用 max_retries=0, fallback=false 确保错误传播
        // (P1-4:默认配置启用了重试+降级，需显式关闭以验证原始错误传播路径)
        let invoker = Arc::new(StubLlmInvoker::with_fixed_response(
            "this is not json",
            "broken-llm",
        ));
        let config = JudgeClientConfig {
            max_retries: 0,
            fallback_on_parse_failure: false,
            ..Default::default()
        };
        let client = ModelRouterJudgeClient::with_config(
            router,
            invoker,
            JudgePromptTemplate::default(),
            config,
        );

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let result = client.judge(&spec_v_i, &spec_v_i_minus_1).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            AutoDpoError::InvalidVerdict { field, .. } => {
                assert_eq!(field, "json_parse");
            }
            other => panic!("期望 InvalidVerdict，实际: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_model_router_judge_client_with_custom_config() {
        let router = make_test_router();
        let invoker = Arc::new(StubLlmInvoker::current_wins());
        let config = JudgeClientConfig {
            routing_strategy: RoutingStrategy::Lite,
            quest_id_prefix: "test-judge".to_string(),
            estimated_tokens: 2048,
            // 其余字段使用默认值（P1-4 retry 配置）
            ..Default::default()
        };
        let client = ModelRouterJudgeClient::with_config(
            router,
            invoker,
            JudgePromptTemplate::default(),
            config,
        );

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        // 验证自定义配置下仍能正常工作
        let verdict = client.judge(&spec_v_i, &spec_v_i_minus_1).await.unwrap();
        assert_eq!(verdict.winner, SpecVersion::Current);
    }

    #[tokio::test]
    async fn test_model_router_judge_client_with_high_version_numbers() {
        let router = make_test_router();
        let invoker = Arc::new(StubLlmInvoker::current_wins());
        let client = ModelRouterJudgeClient::new(router, invoker);

        let spec_v_i = make_test_spec(100, "v100");
        let spec_v_i_minus_1 = make_test_spec(99, "v99");

        let verdict = client.judge(&spec_v_i, &spec_v_i_minus_1).await.unwrap();
        assert_eq!(verdict.winner, SpecVersion::Current);
    }

    #[tokio::test]
    async fn test_model_router_judge_client_does_not_modify_specs() {
        // 验证评判器不修改输入 spec（防注入红线）
        let router = make_test_router();
        let invoker = Arc::new(StubLlmInvoker::current_wins());
        let client = ModelRouterJudgeClient::new(router, invoker);

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let original_merkle_v_i = spec_v_i.canonical_merkle_input();
        let original_merkle_v_i_minus_1 = spec_v_i_minus_1.canonical_merkle_input();

        let _verdict = client.judge(&spec_v_i, &spec_v_i_minus_1).await.unwrap();

        // 验证 spec 未被修改
        assert_eq!(spec_v_i.canonical_merkle_input(), original_merkle_v_i);
        assert_eq!(
            spec_v_i_minus_1.canonical_merkle_input(),
            original_merkle_v_i_minus_1
        );
    }
}
