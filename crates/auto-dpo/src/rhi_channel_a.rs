//! RHI-CG 通道 A — Recursive Harness Improvement Channel A（提议通道）
//!
//! 对应架构层: L5 Knowledge
//! 对应 ADR: ADR-032（双通道评估器）/ ADR-044（P5 工程实施）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.4（RHI-CG 双通道）
//! 对应任务: **P5.1.1**（PreferencePair 扩展）+ **P5.1.2**（评判器 LLM 调用接口）
//!
//! # 核心职责
//!
//! RHI-CG 通道 A 是"提议通道"，负责从相邻 HarnessSpec 版本生成偏好对：
//! 1. 接收相邻版本 spec（v_i 与 v_{i-1}）
//! 2. 调用评判器（LLM 或 stub）对比两个版本
//! 3. 生成 PreferencePair（chosen = 胜出版本，rejected = 失败版本）
//! 4. 返回偏好对供下游 gsoe-evolution 进化使用
//!
//! # RHI-CG 双通道架构（设计文档 §7.4）
//!
//! ```text
//! 相邻 spec 版本 v_i / v_{i-1}
//!              │
//!              ▼
//!    ┌─────────────────────────┐
//!    │  通道 A（提议）：         │  ←── 本文件
//!    │  - LLM 评判器对比         │
//!    │  - 生成 PreferencePair   │
//!    │  - 写入 mlc-engine L2    │  ←── P5.1.3（self_history.rs）
//!    └────────┬────────────────┘
//!             │
//!             ▼
//!    ┌─────────────────────────┐
//!    │  通道 B（否决）：         │  ←── gsoe-evolution/src/ci_gate.rs（P5.2）
//!    │  - cargo test + criterion │
//!    │  - INV-7/8/9 不变量       │
//!    │  - 连续 3 次显著回归才否决 │
//!    └─────────────────────────┘
//! ```
//!
//! # 设计决策（WHY）
//!
//! ## 1. 复用而非新建（C2 决策）
//!
//! 通道 A **不新建** PreferencePair 替代品，而是扩展既有 `PreferencePair::from_adjacent_specs()`。
//! 评判结果 `JudgeVerdict` 是新增类型，但其字段直接映射到 PreferencePair 的 chosen/rejected/score。
//!
//! ## 2. 评判器 trait + boxed Future 模式
//!
//! WHY `Pin<Box<dyn Future>>` 而非 `async-trait`：
//! - 项目 workspace 未引入 `async-trait` 依赖（保持依赖最小化）
//! - boxed Future 是 Rust 1.75 前的标准模式，与 `dyn Trait` 对象安全兼容
//! - 评判器调用一次延迟约秒级，Box 堆分配开销（~50ns）相对网络 RTT 可忽略
//! - 与 `model-router::RouteHook` 模式一致（同步 trait + 内部 tokio::spawn）
//!
//! ## 3. 不可进化面保护（设计 §7.2）
//!
//! 通道 A 仅读取 `HarnessSpec::canonical_merkle_input()` 规范化字符串作为 chosen/rejected 内容：
//! - **无写路径**：spec 字段是只读数据，无法通过偏好对生成路径写入文件
//! - **Merkle 完整性**：spec 任何字段变化都反映在 merkle input 中，下游可校验
//! - **不可进化面引用检查**：spec.validate() 已在加载时执行（P4-W15.1.1）
//!
//! ## 4. R2 冻结声明（ADR-042）
//!
//! 通道 A **不在 R2 冻结范围**：
//! - R2 = GSOE×AutoDPO 约束 RL 路径（gsoe-evolution 的 GrpoPolicy + auto-dpo 的 R2 训练）
//! - 通道 A 是"提议"机制，仅生成 PreferencePair，不直接执行 RL 训练
//! - 通道 B（CI 否决）是 R2 的安全门，R2 在 FormalVerifier 落地前无条件冻结
//!
//! # 学习不在关键路径（设计 §7.1）
//!
//! 通道 A 的评判器调用是异步的，但调用方（RHI-CG 编排器）**不在请求关键路径**：
//! - 编排器在 spec 版本切换后异步触发通道 A
//! - 评判结果持久化到 mlc-engine L2，供下次 spec 进化使用
//! - 调用方的本地 fallback 是"沿用上一版本 spec"（不阻塞请求）

use crate::error::AutoDpoError;
use crate::types::PreferencePair;
use nexus_contracts::HarnessSpec;
use std::future::Future;
use std::pin::Pin;

// ============================================================
// SpecVersion — 评判胜出者标识
// ============================================================

/// Spec 版本标识 — 用于 JudgeVerdict.winner 字段
///
/// WHY enum 而非 bool/u8:
/// - 编译期穷尽性（match 必须覆盖两个变体）
/// - 语义清晰（Current/Previous 比 true/false 易读）
/// - 与 `from_adjacent_specs` 的 match 配合，避免误用
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecVersion {
    /// 当前版本 v_i（被提议的新版本）
    Current,
    /// 上一版本 v_{i-1}（基线版本，用于对比）
    Previous,
}

impl SpecVersion {
    /// 返回人类可读的标识字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Previous => "previous",
        }
    }
}

impl std::fmt::Display for SpecVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// JudgeVerdict — 评判结果
// ============================================================

/// 评判结果 — LLM 评判器对相邻 spec 版本的对比输出
///
/// # 字段语义
///
/// | 字段 | 类型 | 范围 | 含义 |
/// |------|------|------|------|
/// | `winner` | SpecVersion | Current/Previous | 胜出版本标识 |
/// | `winner_score` | f32 | [0.0, 1.0] | 胜出者质量评分 |
/// | `loser_score` | f32 | [0.0, 1.0] | 失败者质量评分 |
/// | `confidence` | f32 | [0.0, 1.0] | 评判器自评置信度 |
/// | `rationale` | String | 任意 | 评判理由（人类可读，用于审计） |
///
/// # 不变量
///
/// - `winner_score >= loser_score`（胜出者分数不低于失败者）
/// - 所有 f32 字段 ∈ [0.0, 1.0]（合法评分范围）
/// - `rationale` 非空（便于审计追溯）
///
/// WHY 不派生 Serialize: 评判结果仅在内存流转，不需要序列化到磁盘。
/// 如需持久化，使用 PreferencePair 作为载体（已派生 Serialize）。
#[derive(Debug, Clone, PartialEq)]
pub struct JudgeVerdict {
    /// 胜出版本（Current = v_i，Previous = v_{i-1}）
    pub winner: SpecVersion,
    /// 胜出者质量评分 [0.0, 1.0]
    pub winner_score: f32,
    /// 失败者质量评分 [0.0, 1.0]
    pub loser_score: f32,
    /// 评判器自评置信度 [0.0, 1.0]
    ///
    /// WHY 单独字段而非从 winner_score - loser_score 派生:
    /// - LLM 评判器可能对高分对（如 0.9 vs 0.85）给出低置信度
    /// - 置信度影响下游加权训练的样本权重
    pub confidence: f32,
    /// 评判理由（人类可读，用于审计与调试）
    pub rationale: String,
}

impl JudgeVerdict {
    /// 创建新的评判结果，并执行字段合法性校验
    ///
    /// # 校验规则
    /// - 所有 f32 字段必须 ∈ [0.0, 1.0]
    /// - `winner_score` 必须 >= `loser_score`（允许相等，表示平局）
    /// - `rationale` 必须非空
    ///
    /// # 错误
    /// - `InvalidVerdict`: 任一字段越界或逻辑不一致
    pub fn new(
        winner: SpecVersion,
        winner_score: f32,
        loser_score: f32,
        confidence: f32,
        rationale: impl Into<String>,
    ) -> Result<Self, AutoDpoError> {
        let rationale = rationale.into();

        // 校验评分范围 [0.0, 1.0]
        let clamp_check = |field: &str, v: f32| -> Result<(), AutoDpoError> {
            if v.is_nan() || !(0.0..=1.0).contains(&v) {
                return Err(AutoDpoError::InvalidVerdict {
                    field: field.to_string(),
                    value: format!("{v}"),
                });
            }
            Ok(())
        };

        clamp_check("winner_score", winner_score)?;
        clamp_check("loser_score", loser_score)?;
        clamp_check("confidence", confidence)?;

        // 校验 winner_score >= loser_score
        if winner_score < loser_score {
            return Err(AutoDpoError::InvalidVerdict {
                field: "winner_score".to_string(),
                value: format!("winner_score ({winner_score}) < loser_score ({loser_score})"),
            });
        }

        // 校验 rationale 非空
        if rationale.is_empty() {
            return Err(AutoDpoError::InvalidVerdict {
                field: "rationale".to_string(),
                value: "empty".to_string(),
            });
        }

        Ok(Self {
            winner,
            winner_score,
            loser_score,
            confidence,
            rationale,
        })
    }

    /// 偏好信号强度 = winner_score - loser_score
    ///
    /// WHY 提供: 下游加权训练按 score_gap 排序选样本
    pub fn score_gap(&self) -> f32 {
        self.winner_score - self.loser_score
    }
}

// ============================================================
// JudgeClient trait — 评判器接口
// ============================================================

/// 评判器 trait — RHI-CG 通道 A 的 LLM 评判接口
///
/// # 实现契约
///
/// - 必须 `Send + Sync`（RhiChannelA 可在 async 任务间共享）
/// - `judge` 方法返回 `Pin<Box<dyn Future>>`，调用方 `.await` 获取结果
/// - 实现可在内部使用 `tokio::spawn` 包装实际 LLM 调用
/// - 实现不应 panic（可能导致 RhiChannelA 不可用）
///
/// # 设计决策（WHY）
///
/// ## boxed Future 而非 async fn in trait
///
/// - 项目未引入 `async-trait` 依赖（workspace Cargo.toml 无此包）
/// - `async fn in trait` 在 Rust 1.75+ 稳定但 `dyn Trait` 不安全（需 async_fn_in_trait feature）
/// - `Pin<Box<dyn Future>>` 是兼容 `dyn Trait` 的标准模式
/// - 评判延迟秒级，Box 堆分配开销（~50ns）可忽略
///
/// ## 与 RouteHook 模式对比
///
/// | 维度 | RouteHook | JudgeClient |
/// |------|-----------|-------------|
/// | 同步性 | 同步 | 异步（boxed Future） |
/// | 用途 | 观察路由轨迹 | LLM 评判 spec 对比 |
/// | 调用频率 | 每次 route() | 每次 spec 版本切换 |
/// | 失败处理 | 静默丢弃 | 返回 Err 中断通道 A |
///
/// WHY 不同: RouteHook 是观测副作用（失败不影响主流程），JudgeClient 是核心评判逻辑（失败必须中断）。
///
/// # 默认实现
///
/// trait 不提供默认实现，强制实现者显式提供评判逻辑（避免忘记实现导致空评判）。
pub trait JudgeClient: Send + Sync {
    /// 评判相邻 spec 版本，返回胜出者与置信度
    ///
    /// # 参数
    /// - `spec_v_i`: 当前版本 spec（v_i，被提议的新版本）
    /// - `spec_v_i_minus_1`: 上一版本 spec（v_{i-1}，基线版本）
    ///
    /// # 返回
    /// - `Ok(JudgeVerdict)`: 评判成功，携带胜出者与评分
    /// - `Err(AutoDpoError::JudgeFailed)`: 评判器调用失败（LLM 不可达 / 超时）
    /// - `Err(AutoDpoError::InvalidVerdict)`: 评判器返回非法数据（越界 / 逻辑不一致）
    ///
    /// # 调用方约束
    /// - 调用方应在 async 上下文中 `.await` 返回的 Future
    /// - 同一 JudgeClient 实例可被并发调用（实现需保证 Send + Sync）
    fn judge<'a>(
        &'a self,
        spec_v_i: &'a HarnessSpec,
        spec_v_i_minus_1: &'a HarnessSpec,
    ) -> Pin<Box<dyn Future<Output = Result<JudgeVerdict, AutoDpoError>> + Send + 'a>>;
}

// ============================================================
// StubJudgeClient — 测试与离线开发桩
// ============================================================

/// Stub 评判器 — 用于测试与离线开发
///
/// # 设计意图
///
/// 提供确定性的评判结果，避免测试依赖外部 LLM 服务：
/// - `winner` 与 `confidence` 在构造时固定
/// - `winner_score` / `loser_score` 使用预设的合理值（0.8 / 0.4）
/// - `rationale` 固定为 "Stub evaluation"
///
/// # 使用场景
///
/// - 单元测试：验证 RhiChannelA 的编排逻辑（不依赖 LLM）
/// - 离线开发：在无 LLM 服务的环境下迭代通道 A 实现
/// - 基准测试：criterion bench 需要确定性输入
///
/// # 不变量
///
/// - 构造时校验 `confidence ∈ [0.0, 1.0]`（避免下游 panic）
/// - `judge()` 永远返回 `Ok`（stub 不模拟失败，失败场景用 MockJudgeClient）
pub struct StubJudgeClient {
    /// 固定返回的胜出者
    winner: SpecVersion,
    /// 固定返回的置信度
    confidence: f32,
}

impl StubJudgeClient {
    /// 创建 stub 评判器
    ///
    /// # 错误
    /// - `confidence` 越界 [0.0, 1.0] 时 panic（编程错误，非运行时错误）
    pub fn new(winner: SpecVersion, confidence: f32) -> Self {
        assert!(
            (0.0..=1.0).contains(&confidence),
            "StubJudgeClient confidence must be in [0.0, 1.0], got {confidence}"
        );
        Self { winner, confidence }
    }

    /// 创建一个总是裁决 Current 胜出的 stub（便捷构造器）
    pub fn current_wins() -> Self {
        Self::new(SpecVersion::Current, 0.9)
    }

    /// 创建一个总是裁决 Previous 胜出的 stub（便捷构造器，模拟通道 B 否决场景）
    pub fn previous_wins() -> Self {
        Self::new(SpecVersion::Previous, 0.9)
    }
}

impl JudgeClient for StubJudgeClient {
    fn judge<'a>(
        &'a self,
        _spec_v_i: &'a HarnessSpec,
        _spec_v_i_minus_1: &'a HarnessSpec,
    ) -> Pin<Box<dyn Future<Output = Result<JudgeVerdict, AutoDpoError>> + Send + 'a>> {
        // WHY Box::pin: 将 async 块转为 Pin<Box<dyn Future>>，满足 trait 签名
        // WHY 'a 生命周期: 闭包捕获 &self，Future 生命周期不超过 self
        Box::pin(async move {
            // stub 不构造真实 JudgeVerdict，使用 JudgeVerdict::new 校验合法性
            JudgeVerdict::new(
                self.winner,
                0.8, // winner_score: 合理的胜出者分数
                0.4, // loser_score: 合理的失败者分数
                self.confidence,
                "Stub evaluation".to_string(),
            )
        })
    }
}

// ============================================================
// RhiChannelA — 通道 A 编排器
// ============================================================

/// RHI-CG 通道 A 编排器 — 协调评判器与偏好对生成
///
/// # 职责
///
/// 1. 接收相邻 spec 版本（v_i, v_{i-1}）
/// 2. 调用 JudgeClient 评判器获取 JudgeVerdict
/// 3. 通过 `PreferencePair::from_adjacent_specs()` 生成偏好对
/// 4. 返回偏好对供下游（gsoe-evolution / 持久化）使用
///
/// # 设计决策（WHY）
///
/// - **pair_id 生成策略**: 使用 `format!("rhi-pair-{v_i}-{v_i_minus_1}")` 命名空间，
///   与 `PreferencePairGenerator::next_pair_id()` 的 `dpo-pair-{counter}` 解耦，
///   便于下游（gsoe-evolution SpecRegistry）从 pair_id 反推 spec 版本谱系
///
/// - **不内置 EventBus**: 通道 A 的偏好对生成事件由调用方负责发布
///   （避免与既有 PreferencePairGenerator 重复事件发布逻辑）
///
/// - **不内置持久化**: 偏好对持久化到 mlc-engine L2 由 P5.1.3（self_history.rs）承担，
///   通道 A 仅生成内存中的 PreferencePair，持久化是调用方职责
///
/// # 线程安全
///
/// - `judge_client` 为 `Arc<dyn JudgeClient>`，可在 async 任务间共享
/// - RhiChannelA 本身无内部可变状态，`&self` 即可调用
pub struct RhiChannelA {
    /// 评判器客户端（Arc<dyn> 模式，允许在 async 任务间共享）
    judge_client: std::sync::Arc<dyn JudgeClient>,
}

impl RhiChannelA {
    /// 创建通道 A 编排器
    ///
    /// # 参数
    /// - `judge_client`: 评判器客户端（Arc 包装，可在 async 任务间共享）
    pub fn new(judge_client: std::sync::Arc<dyn JudgeClient>) -> Self {
        Self { judge_client }
    }

    /// 从评判器客户端获取 Arc 引用（便于测试与外部共享）
    pub fn judge_client(&self) -> &std::sync::Arc<dyn JudgeClient> {
        &self.judge_client
    }

    /// 生成偏好对 — 通道 A 主流程
    ///
    /// # 流程
    /// 1. 调用 `JudgeClient::judge()` 评判相邻 spec 版本
    /// 2. 构造 pair_id（`rhi-pair-{v_i_version}-{v_i_minus_1_version}`）
    /// 3. 调用 `PreferencePair::from_adjacent_specs()` 生成偏好对
    ///
    /// # 参数
    /// - `spec_v_i`: 当前版本 spec（v_i，被提议的新版本）
    /// - `spec_v_i_minus_1`: 上一版本 spec（v_{i-1}，基线版本）
    ///
    /// # 返回
    /// - `Ok(PreferencePair)`: 偏好对生成成功
    /// - `Err(AutoDpoError)`: 评判器失败或评判结果非法
    ///
    /// # 异步性
    ///
    /// 此方法是 async 的（内部 await 评判器 Future）。调用方需在 async 上下文中使用。
    pub async fn generate_preference_pair(
        &self,
        spec_v_i: &HarnessSpec,
        spec_v_i_minus_1: &HarnessSpec,
    ) -> Result<PreferencePair, AutoDpoError> {
        // 步骤 1: 调用评判器
        let verdict = self.judge_client.judge(spec_v_i, spec_v_i_minus_1).await?;

        // 步骤 2: 构造 pair_id（命名空间 rhi-pair-{v_i}-{v_i_minus_1}）
        let pair_id = format!(
            "rhi-pair-{}-{}",
            spec_v_i.meta.version, spec_v_i_minus_1.meta.version
        );

        // 步骤 3: 生成偏好对
        let pair =
            PreferencePair::from_adjacent_specs(pair_id, spec_v_i, spec_v_i_minus_1, &verdict);

        tracing::info!(
            pair_id = %pair.pair_id,
            winner = %verdict.winner,
            winner_score = verdict.winner_score,
            loser_score = verdict.loser_score,
            confidence = verdict.confidence,
            "RHI-CG channel A: preference pair generated"
        );

        Ok(pair)
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
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

    // ============================================================
    // SpecVersion 测试
    // ============================================================

    #[test]
    fn test_spec_version_as_str() {
        assert_eq!(SpecVersion::Current.as_str(), "current");
        assert_eq!(SpecVersion::Previous.as_str(), "previous");
    }

    #[test]
    fn test_spec_version_display() {
        assert_eq!(format!("{}", SpecVersion::Current), "current");
        assert_eq!(format!("{}", SpecVersion::Previous), "previous");
    }

    // ============================================================
    // JudgeVerdict 测试
    // ============================================================

    #[test]
    fn test_judge_verdict_new_valid() {
        let verdict = JudgeVerdict::new(
            SpecVersion::Current,
            0.85,
            0.45,
            0.9,
            "v2 wins on test coverage",
        );
        assert!(verdict.is_ok());
        let v = verdict.unwrap();
        assert_eq!(v.winner, SpecVersion::Current);
        assert!((v.winner_score - 0.85).abs() < 1e-6);
        assert!((v.loser_score - 0.45).abs() < 1e-6);
        assert!((v.confidence - 0.9).abs() < 1e-6);
        assert_eq!(v.rationale, "v2 wins on test coverage");
    }

    #[test]
    fn test_judge_verdict_new_winner_score_below_loser_fails() {
        let verdict = JudgeVerdict::new(
            SpecVersion::Current,
            0.3, // winner_score < loser_score
            0.8,
            0.9,
            "invalid",
        );
        assert!(verdict.is_err());
        let err = verdict.unwrap_err();
        match err {
            AutoDpoError::InvalidVerdict { field, .. } => {
                assert_eq!(field, "winner_score");
            }
            other => panic!("期望 InvalidVerdict，实际: {other:?}"),
        }
    }

    #[test]
    fn test_judge_verdict_new_score_out_of_range_fails() {
        // winner_score > 1.0
        let verdict = JudgeVerdict::new(SpecVersion::Current, 1.5, 0.3, 0.9, "test");
        assert!(verdict.is_err());

        // confidence < 0.0
        let verdict = JudgeVerdict::new(SpecVersion::Current, 0.8, 0.3, -0.1, "test");
        assert!(verdict.is_err());

        // loser_score = NaN
        let verdict = JudgeVerdict::new(SpecVersion::Current, 0.8, f32::NAN, 0.9, "test");
        assert!(verdict.is_err());
    }

    #[test]
    fn test_judge_verdict_new_empty_rationale_fails() {
        let verdict = JudgeVerdict::new(SpecVersion::Current, 0.8, 0.3, 0.9, "");
        assert!(verdict.is_err());
        let err = verdict.unwrap_err();
        match err {
            AutoDpoError::InvalidVerdict { field, .. } => {
                assert_eq!(field, "rationale");
            }
            other => panic!("期望 InvalidVerdict (rationale)，实际: {other:?}"),
        }
    }

    #[test]
    fn test_judge_verdict_new_equal_scores_allowed() {
        // winner_score == loser_score 应允许（平局场景）
        let verdict = JudgeVerdict::new(
            SpecVersion::Current,
            0.7,
            0.7, // 平局
            0.5,
            "tie",
        );
        assert!(verdict.is_ok());
    }

    #[test]
    fn test_judge_verdict_score_gap() {
        let verdict =
            JudgeVerdict::new(SpecVersion::Current, 0.9, 0.2, 0.95, "strong win").unwrap();
        assert!((verdict.score_gap() - 0.7).abs() < 1e-6);
    }

    // ============================================================
    // StubJudgeClient 测试
    // ============================================================

    #[test]
    fn test_stub_judge_client_new_valid() {
        // 合法 confidence
        let _stub = StubJudgeClient::new(SpecVersion::Current, 0.9);
        let _stub = StubJudgeClient::new(SpecVersion::Previous, 0.0);
        let _stub = StubJudgeClient::new(SpecVersion::Current, 1.0);
    }

    #[test]
    #[should_panic(expected = "StubJudgeClient confidence must be in [0.0, 1.0]")]
    fn test_stub_judge_client_new_confidence_out_of_range_panics() {
        let _stub = StubJudgeClient::new(SpecVersion::Current, 1.5);
    }

    #[test]
    #[should_panic(expected = "StubJudgeClient confidence must be in [0.0, 1.0]")]
    fn test_stub_judge_client_new_negative_confidence_panics() {
        let _stub = StubJudgeClient::new(SpecVersion::Current, -0.1);
    }

    #[test]
    fn test_stub_judge_client_convenience_constructors() {
        let current_wins = StubJudgeClient::current_wins();
        // 验证便捷构造器不 panic 且生成合法 stub
        assert!(matches!(current_wins.winner, SpecVersion::Current));

        let previous_wins = StubJudgeClient::previous_wins();
        assert!(matches!(previous_wins.winner, SpecVersion::Previous));
    }

    #[tokio::test]
    async fn test_stub_judge_client_judge_returns_valid_verdict() {
        let stub = StubJudgeClient::current_wins();
        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let verdict = stub.judge(&spec_v_i, &spec_v_i_minus_1).await.unwrap();
        assert_eq!(verdict.winner, SpecVersion::Current);
        assert!((verdict.winner_score - 0.8).abs() < 1e-6);
        assert!((verdict.loser_score - 0.4).abs() < 1e-6);
        assert!((verdict.confidence - 0.9).abs() < 1e-6);
        assert_eq!(verdict.rationale, "Stub evaluation");
    }

    #[tokio::test]
    async fn test_stub_judge_client_previous_wins_returns_previous() {
        let stub = StubJudgeClient::previous_wins();
        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let verdict = stub.judge(&spec_v_i, &spec_v_i_minus_1).await.unwrap();
        assert_eq!(verdict.winner, SpecVersion::Previous);
    }

    // ============================================================
    // RhiChannelA 集成测试
    // ============================================================

    #[tokio::test]
    async fn test_rhi_channel_a_generate_preference_pair_current_wins() {
        // 评判器裁决当前版本胜出
        let stub = std::sync::Arc::new(StubJudgeClient::current_wins());
        let channel_a = RhiChannelA::new(stub);

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let pair = channel_a
            .generate_preference_pair(&spec_v_i, &spec_v_i_minus_1)
            .await
            .unwrap();

        // 验证 pair_id 格式
        assert_eq!(pair.pair_id, "rhi-pair-2-1");

        // 验证 chosen = v_i 的 merkle input（current 胜出）
        assert_eq!(pair.chosen, spec_v_i.canonical_merkle_input());
        assert_eq!(pair.rejected, spec_v_i_minus_1.canonical_merkle_input());

        // 验证评分来自 stub（winner_score=0.8, loser_score=0.4）
        assert!((pair.chosen_score - 0.8).abs() < 1e-6);
        assert!((pair.rejected_score - 0.4).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_rhi_channel_a_generate_preference_pair_previous_wins() {
        // 评判器裁决上一版本胜出（通道 B 否决场景）
        let stub = std::sync::Arc::new(StubJudgeClient::previous_wins());
        let channel_a = RhiChannelA::new(stub);

        let spec_v_i = make_test_spec(3, "v3");
        let spec_v_i_minus_1 = make_test_spec(2, "v2");

        let pair = channel_a
            .generate_preference_pair(&spec_v_i, &spec_v_i_minus_1)
            .await
            .unwrap();

        // 验证 pair_id 格式（v3 vs v2）
        assert_eq!(pair.pair_id, "rhi-pair-3-2");

        // 验证 chosen = v_{i-1} 的 merkle input（previous 胜出）
        assert_eq!(pair.chosen, spec_v_i_minus_1.canonical_merkle_input());
        assert_eq!(pair.rejected, spec_v_i.canonical_merkle_input());
    }

    #[tokio::test]
    async fn test_rhi_channel_a_judge_client_accessor() {
        let stub: std::sync::Arc<dyn JudgeClient> =
            std::sync::Arc::new(StubJudgeClient::current_wins());
        // 克隆一份以保留外部引用，便于验证 strong_count
        let external_clone = std::sync::Arc::clone(&stub);
        let channel_a = RhiChannelA::new(stub);

        // 验证 judge_client 访问器返回的是同一个 Arc（与 external_clone 共享底层数据）
        let client_ref = channel_a.judge_client();
        // strong_count = 2: external_clone + channel_a 内部
        assert_eq!(std::sync::Arc::strong_count(client_ref), 2);
        assert!(std::sync::Arc::ptr_eq(client_ref, &external_clone));
    }

    #[tokio::test]
    async fn test_rhi_channel_a_generate_with_high_version_numbers() {
        // 验证高版本号（如 v47 vs v46）也能正确生成 pair_id
        let stub = std::sync::Arc::new(StubJudgeClient::current_wins());
        let channel_a = RhiChannelA::new(stub);

        let spec_v_i = make_test_spec(47, "v47");
        let spec_v_i_minus_1 = make_test_spec(46, "v46");

        let pair = channel_a
            .generate_preference_pair(&spec_v_i, &spec_v_i_minus_1)
            .await
            .unwrap();

        assert_eq!(pair.pair_id, "rhi-pair-47-46");
    }

    // ============================================================
    // 不可进化面保护测试（防注入验证）
    // ============================================================

    #[tokio::test]
    async fn test_rhi_channel_a_does_not_modify_specs() {
        // 验证通道 A 不修改输入 spec（防注入红线）
        let stub = std::sync::Arc::new(StubJudgeClient::current_wins());
        let channel_a = RhiChannelA::new(stub);

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        // 记录原始 merkle input
        let original_merkle_v_i = spec_v_i.canonical_merkle_input();
        let original_merkle_v_i_minus_1 = spec_v_i_minus_1.canonical_merkle_input();

        let _pair = channel_a
            .generate_preference_pair(&spec_v_i, &spec_v_i_minus_1)
            .await
            .unwrap();

        // 验证 spec 未被修改（merkle input 应保持不变）
        assert_eq!(spec_v_i.canonical_merkle_input(), original_merkle_v_i);
        assert_eq!(
            spec_v_i_minus_1.canonical_merkle_input(),
            original_merkle_v_i_minus_1
        );
    }

    /// MockJudgeClient — 用于测试失败场景
    ///
    /// 与 StubJudgeClient 不同：MockJudgeClient 可注入失败结果，用于测试错误处理
    pub struct MockJudgeClient {
        failure_reason: Option<String>,
    }

    impl MockJudgeClient {
        pub fn always_failing(reason: impl Into<String>) -> Self {
            Self {
                failure_reason: Some(reason.into()),
            }
        }

        pub fn always_succeeding() -> Self {
            Self {
                failure_reason: None,
            }
        }
    }

    impl JudgeClient for MockJudgeClient {
        fn judge<'a>(
            &'a self,
            _spec_v_i: &'a HarnessSpec,
            _spec_v_i_minus_1: &'a HarnessSpec,
        ) -> Pin<Box<dyn Future<Output = Result<JudgeVerdict, AutoDpoError>> + Send + 'a>> {
            Box::pin(async move {
                if let Some(reason) = &self.failure_reason {
                    Err(AutoDpoError::JudgeFailed {
                        reason: reason.clone(),
                    })
                } else {
                    JudgeVerdict::new(SpecVersion::Current, 0.8, 0.4, 0.9, "Mock success")
                }
            })
        }
    }

    #[tokio::test]
    async fn test_rhi_channel_a_judge_failure_propagates() {
        // 验证评判器失败时通道 A 返回 JudgeFailed 错误
        let mock: std::sync::Arc<dyn JudgeClient> =
            std::sync::Arc::new(MockJudgeClient::always_failing("LLM service unreachable"));
        let channel_a = RhiChannelA::new(mock);

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let result = channel_a
            .generate_preference_pair(&spec_v_i, &spec_v_i_minus_1)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            AutoDpoError::JudgeFailed { reason } => {
                assert_eq!(reason, "LLM service unreachable");
            }
            other => panic!("期望 JudgeFailed，实际: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_rhi_channel_a_judge_success_with_mock() {
        let mock: std::sync::Arc<dyn JudgeClient> =
            std::sync::Arc::new(MockJudgeClient::always_succeeding());
        let channel_a = RhiChannelA::new(mock);

        let spec_v_i = make_test_spec(2, "v2");
        let spec_v_i_minus_1 = make_test_spec(1, "v1");

        let pair = channel_a
            .generate_preference_pair(&spec_v_i, &spec_v_i_minus_1)
            .await
            .unwrap();

        // 验证 mock 返回的胜出者为 Current
        assert_eq!(pair.chosen, spec_v_i.canonical_merkle_input());
    }
}
