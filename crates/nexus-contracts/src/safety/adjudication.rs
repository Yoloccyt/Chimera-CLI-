//! 裁决权与认识类型契约 — Claim/Certify/Declaim 语义分离 + MLC 二维模型认识标签
//!
//! 对应架构层: **L0 Contracts**（纯类型零依赖契约层，ADR-033）
//! 对应设计源: `Chimera架构重构与优化方案_v2.3` §(b.1) L0 payload / §(b.3) 认识类型标签
//!
//! # WHY（本模块存在的理由）
//! v2.3 方案的核心红线之一「裁决权外置」要求 agent 不能自行宣布成功：
//! `Claim`（我宣布 X 成立）与 `Certify`（验证器确认 X 成立）在系统层面分通道、
//! 分权限。为遵守「不动 L0 分层边界」的最高原则（缺口从既有层长出协议），
//! 这三个语义的**载荷类型**与 MLC 二维模型的**认识类型标签**先落 L0 契约，
//! 枚举变体本体则落 L1 `event-bus`（两段式同批落库，防 ADR 立案错位）。
//!
//! 纯类型 + 零逻辑（ADR-033）：本模块仅定义数据形状与 serde derive，
//! 不含任何判定/IO/状态变更逻辑；`severity()` 级别的 Critical 判定红线
//! 留在 L1 event-bus（架构不变量，见 nexus-contracts lib.rs §事件载荷契约）。

use serde::{Deserialize, Serialize};

/// 认识类型标签 — MLC 记忆条目「按认识类型分库」的正交维（§(b.3)）
///
/// 与既有 `MemoryTier`（L0Working/L1Episodic/L2Semantic/L3Procedural，保留与
/// 用途层级）正交：tier 表达「保留/用途层级」，`EpistemicType` 表达「认识类型」。
/// 之所以必须正交，是因为 Episodic/Semantic/Procedural 已含认知语义，若把
/// 认识类型塞进同一维会造成语义重叠（v2.3 §(g) 遗留决策 #3 的正交性评审对象）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EpistemicType {
    /// 客观事实 — 可独立核验的陈述（如「函数 foo 返回 i32」）
    Fact,
    /// 情景经验 — 带时间/上下文的一次性经历记录
    Episode,
    /// 归纳观察 — 从多条原始记录派生的结论（须带 `evidence_ptrs` 指向来源）
    Observation,
    /// 心智模型 — 固定问题的预计算答案，读取 = 一次查询、零模型调用
    MindModel,
}

/// 验证器种类 — `Certify` 的合法签发者白名单（§(b.1)）
///
/// WHY 封闭枚举：Certify 语义「验证器确认」必须由可信验证器签发，agent 无签发权。
/// 白名单治理门据此判定哪些 crate 有资格发布 Certify（未列入者视为越权）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VerifierKind {
    /// 编译器（类型/借用/穷举等编译期保证）
    Compiler,
    /// 测试套件（单元/集成/E2E 通过）
    TestSuite,
    /// 治理门（gate_manifest 登记的脚本门）
    GovernanceGate,
    /// 形式化检查（FormalVerifier 属性验证）
    FormalCheck,
    /// 异构审查者（AdversarialReview 进程内异构一期；跨厂商待 mca 接线）
    HeterogeneousReviewer,
    /// 人审校准（人机一致率锚点；§(b.6) 评测层）
    HumanCalibration,
}

/// Certify 裁决结果 — 验证器对被认证命题的最终判定
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CertifyVerdict {
    /// 已认证：命题成立，UI 可渲染「已验证」
    Certified,
    /// 已否决：命题不成立
    Rejected,
    /// 悬置：认证流程进行中，UI 须单独呈现「正在等待验证」（区别于已验证）
    Deferred,
}

/// Claim 载荷 — agent 宣布某命题成立（broadcast 级，agent 可写）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimPayload {
    /// 声明全局唯一 ID（供 Certify/Declaim 引用）
    pub claim_id: String,
    /// 被宣布成立的命题主体（人类可读摘要，大对象应引用而非内联）
    pub subject: String,
    /// 声明发起者标识（agent/expert ID）
    pub claimant: String,
    /// 认识类型标签（与 Certify 侧一致，便于悬置项分类呈现）
    pub epistemic: EpistemicType,
    /// 关联 Quest ID（可选，跨层追踪用）
    pub quest_id: Option<String>,
}

/// Certify 载荷 — 验证器确认某 Claim 成立（Critical 候选，验证器语义独占）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CertifyPayload {
    /// 被认证/否决的 Claim ID（悬空引用由治理门/测试拦截）
    pub claim_ref: String,
    /// 命题主体（冗余携带，便于审计与 UI 渲染，应与 Claim 一致）
    pub subject: String,
    /// 签发验证器种类（白名单治理门据此授权）
    pub verifier: VerifierKind,
    /// 裁决结果
    pub verdict: CertifyVerdict,
    /// 证据引用列表（如测试名/门号/形式化属性 ID；可空但推荐非空）
    pub evidence_refs: Vec<String>,
}

/// Declaim 载荷 — agent 撤回此前提出的 Claim（broadcast 级，审计完整性）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeclaimPayload {
    /// 被撤回的 Claim ID
    pub claim_ref: String,
    /// 撤回原因（人类可读）
    pub reason: String,
}

/// 失效边界修复状态 — FailureRegistry 条目的生命周期标记
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FailureFixStatus {
    /// 未处理：仍是一个活跃陷阱
    Open,
    /// 已缓解：有临时防御但根因未除
    Mitigated,
    /// 已修复：根因消除
    Resolved,
    /// 不修复：设计接受该边界（附理由）
    WontFix,
}

/// 失败登记条目 — 运行时踩坑的结构化记录（Critical 级，走 mpsc 旁路必达）
///
/// WHY Critical：丢失一条 = 全 swarm 集体重复踩坑（Anthropic「独有信息淹没」
/// 是结构性的）。四字段（适用条件/实验依据/失效边界/修复状态）对齐 Anthropic
/// 多智能体失效模式研究的可复用失败卡片结构。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailureRecord {
    /// 失败签名（用于去重与定向检索，如错误指纹/panic 归一串）
    pub signature: String,
    /// 适用条件：在什么上下文/配置下会触发该失效
    pub applicability: String,
    /// 实验依据：证据来源（测试名/日志锚点/复现步骤）
    pub experiment_basis: String,
    /// 失效边界：不成立的边界/前提，划清「此结论何时失效」
    pub invalidation_boundary: String,
    /// 修复状态
    pub fix_status: FailureFixStatus,
    /// 受影响组件（crate 名或模块路径，便于归属）
    pub affected_component: String,
}
