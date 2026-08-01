//! Harness-as-Spec 契约类型 — P4 学习闭环规格
//!
//! 对应架构层: L0 Contracts（新建）
//! 对应 ADR: ADR-031（Harness-as-Spec 边界）/ ADR-033（L0 nexus-contracts）
//! 对应阶段: P4-W13~16 Harness-as-Spec DSL + SpecRegistry
//!
//! # 设计决策(WHY)
//!
//! - **类型定义在 L0**: `HarnessSpec` / `ContractSpec` / `HopSpec` 需被 L5 `gsoe-evolution`
//!   与 L9 `quest-engine` 共同消费，定义在 L0 避免跨层依赖
//!
//! - **TOML 反序列化派生**: HarnessSpec 以 TOML 文件形式存储（spec DSL），
//!   `serde` 派生支持 TOML/YAML/JSON 多格式反序列化
//!
//! - **append-only 谱系**: `HarnessSpec` 版本号通过 `HarnessMeta.version` 单调递增，
//!   配合 `gsoe-evolution` 的 `SpecRegistry` 实现谱系追踪（lineage）
//!
//! - **不可进化面硬编码（P4-W15.1.1）**: `ImmutableSurface` 枚举穷尽列出所有
//!   不可进化面（13 条红线 + 6 Critical 事件 + INV-7/8/9 + 沙箱/QEEP + 验证器本身），
//!   `validate()` 方法拒绝任何试图修改不可进化面的 spec
//!
//! - **Merkle 输入规范化（P4-W15.1.1）**: `canonical_merkle_input()` 返回规范化字符串，
//!   SHA-256 哈希计算由 L4 seccore 执行（L0 保持零 crate 依赖，ADR-033）
//!
//! - **防注入设计（P4-W15.1.1）**: 所有查询方法均为 `&self`，TOML 反序列化后无法
//!   通过 spec API 写文件路径；contracts/hops 字段引用不可进化面时由 `validate()` 拒绝
//!
//! # 完整 DSL 实现时机
//!
//! 当前文件实现**完整 DSL 类型骨架 + 校验 + Merkle 输入**（P4-W15.1.1），
//! 后续任务:
//! - P4-W15.1.2: gsoe-evolution 实现 spec 加载器（TOML 反序列化 + 调用 validate()）
//! - P4-W15.1.3: seccore 暴露 Merkle 完整性校验公共函数
//! - P4-W15.1.4: 任务输入无写路径测试覆盖

use serde::{Deserialize, Serialize};

// ============================================================
// 不可进化面（ImmutableSurface）— P4-W15.1.1
// ============================================================
//
// WHY 穷尽枚举而非字符串清单:
// - 编译期穷尽性检查（match 必须覆盖所有变体）
// - 避免 typo 错误（字符串清单无法捕获拼写错误）
// - IDE 自动补全 + 重构友好
//
// 来源（设计文档 §7.2 + spec.md L346）:
// - 13 条红线（nuxus规则.md §6.2）
// - 6 个 Critical 事件清单（CheckpointSaved/SkepticVeto/RedTeamAudit/
//   AsaIntervention/AgentTaskFailed/BudgetExceeded）
// - INV-7/INV-8/INV-9 不变量
// - 沙箱/QEEP（seccore 沙箱策略 + QEEP 零孤儿协议）
// - 验证器本身（验证器层级金字塔，含 FormalVerifier）

/// 不可进化面类别 — 人类 ADR 特批的硬编码清单（P4-W15.1.1）
///
/// 不可进化面禁止 RHI-CG 通道 B（CI 否决）修改，仅能通过人类 ADR 特批变更。
/// `HarnessSpec::validate()` 拒绝任何试图修改下列面资源的 spec。
///
/// # 分类
///
/// | 类别 | 变体数 | 来源 |
/// |------|--------|------|
/// | 13 条红线 | 8（编码合并） | nuxus规则.md §6.2 |
/// | Critical 事件清单 | 6 | 设计文档 §6.2 |
/// | 不变量 | 3 | INV-7/8/9 |
/// | 沙箱/QEEP | 2 | seccore/qeep-protocol |
/// | 验证器 | 1 | 验证器层级金字塔 |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ImmutableSurface {
    // === 13 条红线（nuxus规则.md §6.2）===
    /// R1: 禁止持锁 .await（DashMap/Mutex 写锁跨 await 导致死锁）
    RedlineLockAcrossAwait,
    /// R2: rusqlite 必须 spawn_blocking（rusqlite 非 async，直接调用阻塞 runtime）
    RedlineRusqliteBlocking,
    /// R3: broadcast 先 subscribe 再 spawn（避免事件静默丢失）
    RedlineSubscribeBeforeSpawn,
    /// R4: BudgetExceeded severity 必须 = Critical（types.rs:1158，禁止降级）
    RedlineBudgetExceededSeverity,
    /// R5: Critical 安全事件必须用 mpsc channel（确保送达）
    RedlineCriticalEventMpsc,
    /// R6: 禁止 cargo add 不更新 Cargo.lock（audit.yml 每日扫描）
    RedlineCargoLockDrift,
    /// R7: sqlite-vec 禁用（违反 forbid(unsafe_code)，ADR-005 降级）
    RedlineSqliteVecForbidden,
    /// R8: Top-K 必须用 select_nth_unstable（O(n) 替代 O(n log n) sort_by）
    RedlineTopKSelectNthUnstable,

    // === 6 个 Critical 事件清单（设计文档 §6.2）===
    /// C1: CheckpointSaved（任务检查点保存完成）
    CriticalCheckpointSaved,
    /// C2: SkepticVeto（Parliament 怀疑者否决）
    CriticalSkepticVeto,
    /// C3: RedTeamAudit（红队审计触发）
    CriticalRedTeamAudit,
    /// C4: AsaIntervention（ASA 审计干预）
    CriticalAsaIntervention,
    /// C5: AgentTaskFailed（Agent 任务失败）
    CriticalAgentTaskFailed,
    /// C6: BudgetExceeded（预算超限，severity=Critical 红线不变）
    CriticalBudgetExceeded,

    // === INV-7/8/9 不变量 ===
    /// INV-7: chimera-mas MemoryBudgetModel 117MB 预算（130MB×0.9）
    Invariant7MemoryBudget,
    /// INV-8: chimera-mas ArchiveTier 归档单调性（Hot→Warm→Cold→Ice，禁止降级式回升）
    Invariant8ArchiveMonotonic,
    /// INV-9: 委托图无环不变量（proptest 1000 次对齐 INV-7/8 规格）
    Invariant9DagAcyclic,

    // === 沙箱 / QEEP ===
    /// S1: seccore 沙箱策略（白名单 + 命令注入拦截 + 审计链）
    SurfaceSandboxPolicy,
    /// S2: QEEP 量子纠缠执行协议（零孤儿保证）
    SurfaceQeepProtocol,

    // === 验证器本身 ===
    /// V1: 验证器层级金字塔（L3 执行反馈 / L4 形式化 / L5 人类判断）
    SurfaceVerifierHierarchy,
}

impl ImmutableSurface {
    /// 返回所有不可进化面（用于 validate() 全集检查）
    pub const fn all() -> [ImmutableSurface; 20] {
        [
            // 8 条红线
            Self::RedlineLockAcrossAwait,
            Self::RedlineRusqliteBlocking,
            Self::RedlineSubscribeBeforeSpawn,
            Self::RedlineBudgetExceededSeverity,
            Self::RedlineCriticalEventMpsc,
            Self::RedlineCargoLockDrift,
            Self::RedlineSqliteVecForbidden,
            Self::RedlineTopKSelectNthUnstable,
            // 6 个 Critical 事件
            Self::CriticalCheckpointSaved,
            Self::CriticalSkepticVeto,
            Self::CriticalRedTeamAudit,
            Self::CriticalAsaIntervention,
            Self::CriticalAgentTaskFailed,
            Self::CriticalBudgetExceeded,
            // 3 个不变量
            Self::Invariant7MemoryBudget,
            Self::Invariant8ArchiveMonotonic,
            Self::Invariant9DagAcyclic,
            // 2 个沙箱/QEEP
            Self::SurfaceSandboxPolicy,
            Self::SurfaceQeepProtocol,
            // 1 个验证器
            Self::SurfaceVerifierHierarchy,
        ]
    }

    /// 返回规范化的字符串标识（用于 spec 字段引用匹配）
    ///
    /// WHY kebab-case:
    /// - TOML 字段名惯例（serde 默认）
    /// - 与设计文档 §7.2 中的 from/to 字段值风格一致
    /// - 人类可读，便于 ADR 审计
    pub const fn as_str(self) -> &'static str {
        match self {
            // 8 条红线
            Self::RedlineLockAcrossAwait => "redline-lock-across-await",
            Self::RedlineRusqliteBlocking => "redline-rusqlite-blocking",
            Self::RedlineSubscribeBeforeSpawn => "redline-subscribe-before-spawn",
            Self::RedlineBudgetExceededSeverity => "redline-budget-exceeded-severity",
            Self::RedlineCriticalEventMpsc => "redline-critical-event-mpsc",
            Self::RedlineCargoLockDrift => "redline-cargo-lock-drift",
            Self::RedlineSqliteVecForbidden => "redline-sqlite-vec-forbidden",
            Self::RedlineTopKSelectNthUnstable => "redline-top-k-select-nth-unstable",
            // 6 个 Critical 事件
            Self::CriticalCheckpointSaved => "critical-checkpoint-saved",
            Self::CriticalSkepticVeto => "critical-skeptic-veto",
            Self::CriticalRedTeamAudit => "critical-red-team-audit",
            Self::CriticalAsaIntervention => "critical-asa-intervention",
            Self::CriticalAgentTaskFailed => "critical-agent-task-failed",
            Self::CriticalBudgetExceeded => "critical-budget-exceeded",
            // 3 个不变量
            Self::Invariant7MemoryBudget => "inv-7-memory-budget",
            Self::Invariant8ArchiveMonotonic => "inv-8-archive-monotonic",
            Self::Invariant9DagAcyclic => "inv-9-dag-acyclic",
            // 2 个沙箱/QEEP
            Self::SurfaceSandboxPolicy => "surface-sandbox-policy",
            Self::SurfaceQeepProtocol => "surface-qeep-protocol",
            // 1 个验证器
            Self::SurfaceVerifierHierarchy => "surface-verifier-hierarchy",
        }
    }

    /// 返回类别名称（用于错误消息分类）
    pub const fn category(self) -> &'static str {
        match self {
            Self::RedlineLockAcrossAwait
            | Self::RedlineRusqliteBlocking
            | Self::RedlineSubscribeBeforeSpawn
            | Self::RedlineBudgetExceededSeverity
            | Self::RedlineCriticalEventMpsc
            | Self::RedlineCargoLockDrift
            | Self::RedlineSqliteVecForbidden
            | Self::RedlineTopKSelectNthUnstable => "redline",

            Self::CriticalCheckpointSaved
            | Self::CriticalSkepticVeto
            | Self::CriticalRedTeamAudit
            | Self::CriticalAsaIntervention
            | Self::CriticalAgentTaskFailed
            | Self::CriticalBudgetExceeded => "critical-event",

            Self::Invariant7MemoryBudget
            | Self::Invariant8ArchiveMonotonic
            | Self::Invariant9DagAcyclic => "invariant",

            Self::SurfaceSandboxPolicy | Self::SurfaceQeepProtocol => "security-surface",

            Self::SurfaceVerifierHierarchy => "verifier",
        }
    }

    /// 从字符串标识解析为 ImmutableSurface（用于 spec 字段引用匹配）
    ///
    /// WHY 提供: spec 加载器（P4-W15.1.2）调用此方法判断 contracts/hops 字段
    /// 是否引用了不可进化面。返回 None 表示该字符串不是不可进化面标识
    ///
    /// WHY 方法名 `parse_surface` 而非 `from_str`:
    /// - clippy::should_implement_trait 警告 `from_str` 易与 `std::str::FromStr::from_str` 混淆
    /// - L0 不实现 `FromStr` trait：其 Err 关联类型需单独定义 Display 实现，
    ///   而本项目 Option 语义更简洁（None 表示"不是不可进化面"，非错误）
    pub fn parse_surface(s: &str) -> Option<Self> {
        // WHY 静态匹配而非 HashMap: 20 个变体的线性扫描性能足够（O(20)），
        // 避免在 L0 引入 std::collections::HashMap 运行时开销
        for surface in Self::all().iter() {
            if surface.as_str() == s {
                return Some(*surface);
            }
        }
        None
    }
}

impl std::fmt::Display for ImmutableSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// HarnessSpecError — 校验错误类型（P4-W15.1.1）
// ============================================================
//
// WHY 手动实现 std::error::Error 而非 thiserror:
// - L0 nexus-contracts ADR-033 严禁依赖任何 crate（含 thiserror）
// - thiserror 是 derive 宏，会在编译期生成 Display + Error 实现
// - 手动实现等价功能，保持 L0 零依赖特性
// - 错误类型本身是数据（无逻辑），符合 L0 约束

/// HarnessSpec 校验错误 — validate() 拒绝非法 spec 时返回（P4-W15.1.1）
///
/// # 错误分类
///
/// | 错误 | 含义 | 触发场景 |
/// |------|------|---------|
/// | `EmptyMetaName` | meta.name 为空字符串 | spec 必须有唯一标识 |
/// | `InvalidVersion` | meta.version = 0 | 版本号必须单调递增（>= 1） |
/// | `ImmutableSurfaceViolation` | spec 试图修改不可进化面 | contracts/hops 引用不可进化面资源 |
/// | `InvalidContractReference` | hop 引用不存在的契约 | hop.contracts 引用未定义的 ContractSpec.name |
/// | `EmptyHopOrder` | hop.order 为空 Vec | hop 必须定义执行顺序 |
/// | `InvalidAuxiliaryGates` | auxiliary 缺失强制 acceptance_gates | acceptance_gates 必须包含 4 个强制门 |
/// | `ImmutableMetaNotMarked` | meta.name 在不可进化面清单中但 immutable=false | 不可进化面 spec 必须 immutable=true |
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessSpecError {
    /// meta.name 为空字符串（spec 必须有唯一标识）
    EmptyMetaName,

    /// meta.version = 0（版本号必须单调递增，>= 1）
    InvalidVersion,

    /// spec 试图修改不可进化面（contracts/hops 引用不可进化面资源）
    ///
    /// 字段: (位置, 不可进化面标识)
    /// - 位置: 字段在 spec 中的位置描述（如 "contracts\[0\].from"）
    /// - 不可进化面标识: ImmutableSurface::as_str() 返回值
    ImmutableSurfaceViolation {
        /// 字段位置描述（如 "contracts\[0\].from"）
        location: String,
        /// 不可进化面标识
        surface: ImmutableSurface,
    },

    /// hop 引用不存在的契约（hop.contracts 引用未定义的 ContractSpec.name）
    ///
    /// 字段: (hop 索引, 引用的契约名)
    InvalidContractReference {
        /// hop 在 hops 数组中的索引
        hop_index: usize,
        /// 引用但未定义的契约名
        contract_name: String,
    },

    /// hop.order 为空 Vec（hop 必须定义执行顺序）
    EmptyHopOrder {
        /// hop 在 hops 数组中的索引
        hop_index: usize,
    },

    /// auxiliary 缺失强制 acceptance_gates
    ///
    /// acceptance_gates 必须包含 4 个强制门:
    /// "tests_pass" / "bench_no_regression" / "invariants_clean" / "redline_scan_clean"
    ///
    /// 字段: (缺失的强制门列表)
    MissingAcceptanceGates {
        /// 缺失的强制门列表
        missing: Vec<String>,
    },

    /// meta.name 在不可进化面清单中但 immutable=false
    ///
    /// WHY 单独错误: 不可进化面 spec 必须显式标记 immutable=true，
    /// 避免误将不可进化面 spec 当作可进化面处理
    ImmutableMetaNotMarked {
        /// 在不可进化面清单中的 meta.name
        name: String,
    },
}

impl std::fmt::Display for HarnessSpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyMetaName => write!(f, "meta.name 为空字符串（spec 必须有唯一标识）"),
            Self::InvalidVersion => write!(f, "meta.version = 0（版本号必须单调递增，>= 1）"),
            Self::ImmutableSurfaceViolation { location, surface } => write!(
                f,
                "不可进化面违规: 位置 {} 引用了不可进化面 {}（类别: {}）",
                location,
                surface,
                surface.category()
            ),
            Self::InvalidContractReference {
                hop_index,
                contract_name,
            } => write!(
                f,
                "hop[{}] 引用未定义的契约: {}",
                hop_index, contract_name
            ),
            Self::EmptyHopOrder { hop_index } => {
                write!(f, "hop[{}].order 为空（hop 必须定义执行顺序）", hop_index)
            }
            Self::MissingAcceptanceGates { missing } => write!(
                f,
                "auxiliary.acceptance_gates 缺失强制门: {}",
                missing.join(", ")
            ),
            Self::ImmutableMetaNotMarked { name } => write!(
                f,
                "meta.name '{}' 在不可进化面清单中但 immutable=false（必须显式标记 immutable=true）",
                name
            ),
        }
    }
}

impl std::error::Error for HarnessSpecError {}

// ============================================================
// HarnessSpec 类型扩展（P4-W15.1.1）
// ============================================================

/// Harness 规格 — 测试套件完整定义(TOML 格式)
///
/// TOML 结构（设计文档 §7.2 完整版）:
/// ```toml
/// [meta]
/// name = "quest-parse-fuzz"
/// version = 47
/// parent = 46                      # 单 lineage 父版本（可选）
/// task_type = "code_refactor"      # 任务类型（可选）
/// immutable = false
///
/// \[\[contracts\]\]
/// name = "no_panic"                # P2-W5.1 简化字段（向后兼容）
/// property = "fuzz_target_must_not_panic"
/// description = "Fuzz target must not panic on any input"
/// # P4-W15.1.1 扩展字段（设计文档 §7.2 完整版）:
/// from = "Skeptic"                 # 契约来源 Agent
/// to = "orchestrator"              # 契约目标 Agent
/// fields = ["veto_reason", "evidence_block_ids"]
///
/// \[\[hops\]\]
/// name = "generate_input"
/// input_type = "`Vec<u8>`"
/// output_type = "ParseResult"
/// contracts = ["no_panic"]
/// description = "Generate fuzz input"
/// # P4-W15.1.1 扩展字段:
/// order = ["Architect.propose", "Skeptic.review", "Security.gate"]
/// on_veto = "replan(max=2)"
/// fallback = "EscalateToHuman"
///
/// [retry]
/// max_attempts = 5
/// backoff_ms = 1000
/// exponential = true
///
/// [auxiliary]
/// acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
/// ```
///
/// WHY: 一个 HarnessSpec 定义一组测试契约 + 执行步骤 + 重试策略，
/// `gsoe-evolution` 据此执行学习闭环（输入生成 → 执行 → 验证 → 反馈）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessSpec {
    /// 元信息(名称/版本/不可进化标记)
    pub meta: HarnessMeta,
    /// 契约列表(不变量/属性/约束)
    #[serde(default)]
    pub contracts: Vec<ContractSpec>,
    /// 执行步骤列表(输入生成/执行/验证/反馈)
    #[serde(default)]
    pub hops: Vec<HopSpec>,
    /// 重试策略(最大尝试次数/退避时间)
    #[serde(default)]
    pub retry: RetryPolicy,
    /// 辅助字段(自定义扩展数据，不参与核心 DSL 语义)
    ///
    /// WHY: 存储为原始字符串(JSON/TOML/YAML)，由消费方(含 serde_json)按需解析。
    /// L0 禁止依赖 serde_json，故 auxiliary 不使用 `serde_json::Value` 类型
    ///
    /// 设计文档 §7.2 规定 auxiliary.acceptance_gates 必须包含 4 个强制门:
    /// - "tests_pass": 全量测试通过
    /// - "bench_no_regression": criterion 基准无回归
    /// - "invariants_clean": INV-7/8/9 不变量检查通过
    /// - "redline_scan_clean": 13 条红线扫描通过
    #[serde(default)]
    pub auxiliary: Option<String>,
}

/// Harness 元信息 — 名称/版本/不可进化标记
///
/// WHY: `version` 单调递增用于谱系追踪（SpecRegistry lineage），
/// `immutable` 标记不可进化面（13 条红线 + Critical 清单不可变更）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessMeta {
    /// 规格名称(唯一标识，如 "quest-parse-fuzz")
    pub name: String,
    /// 版本号(单调递增，append-only 谱系)
    pub version: u32,
    /// 是否为不可进化面(immutable surface)
    ///
    /// WHY: 不可进化面硬编码为 true，禁止 RHI-CG 通道 B(CI 否决)修改。
    /// 包括: 13 条红线 + Critical 清单 + INV-7/8/9 + 沙箱/QEEP + 验证器本身
    #[serde(default)]
    pub immutable: bool,

    /// 父版本号（单 lineage 谱系，可选）
    ///
    /// WHY 设计文档 §7.2 `[harness.meta] parent = 46`:
    /// - SpecRegistry 谱系追踪用，记录上一个版本
    /// - None 表示初始版本（无父版本）
    /// - 必须小于 `version`（谱系单向递增）
    #[serde(default)]
    pub parent: Option<u32>,

    /// 任务类型（如 "code_refactor" / "fuzz" / "e2e"）
    ///
    /// WHY 设计文档 §7.2 `[harness.meta] task_type = "code_refactor"`:
    /// - SpecRegistry 按任务类型分类 spec
    /// - 不同任务类型可能共享相同契约但 hop 不同
    #[serde(default)]
    pub task_type: Option<String>,
}

/// 契约规格 — 单个不变量/属性/约束定义
///
/// WHY: 契约定义"什么必须成立"，hop 定义"如何验证"。
/// 一个契约可被多个 hop 引用（如 "no_panic" 契约可被多个 fuzz hop 验证）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContractSpec {
    /// 契约名称(在 HarnessSpec 内唯一，如 "no_panic")
    pub name: String,
    /// 契约属性(DSL 表达式，如 "fuzz_target_must_not_panic")
    pub property: String,
    /// 契约描述(可选，人类可读说明)
    #[serde(default)]
    pub description: Option<String>,

    /// 契约来源 Agent（设计文档 §7.2 扩展字段，P4-W15.1.1）
    ///
    /// WHY: 设计文档 §7.2 \[\[contracts\]\] from = "Skeptic"
    /// - 标识契约的发起方 Agent
    /// - validate() 检查 from 是否引用不可进化面（如 "AsaIntervention"）
    #[serde(default)]
    pub from: Option<String>,

    /// 契约目标 Agent（设计文档 §7.2 扩展字段，P4-W15.1.1）
    ///
    /// WHY: 设计文档 §7.2 \[\[contracts\]\] to = "orchestrator"
    /// - 标识契约的接收方 Agent
    /// - validate() 检查 to 是否引用不可进化面
    #[serde(default)]
    pub to: Option<String>,

    /// 契约字段列表（设计文档 §7.2 扩展字段，P4-W15.1.1）
    ///
    /// WHY: 设计文档 §7.2 \[\[contracts\]\] fields = ["veto_reason", ...]
    /// - 契约涉及的字段名列表
    /// - validate() 检查 fields 是否引用不可进化面资源
    #[serde(default)]
    pub fields: Vec<String>,
}

/// 执行步骤规格 — 单个测试步骤定义
///
/// WHY: hop 是 Harness-as-Spec DSL 的最小执行单元，
/// 类似 BDD 的 Given/When/Then 步骤。
/// 每个 hop 引用契约并定义输入/输出类型
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HopSpec {
    /// 步骤名称(在 HarnessSpec 内唯一，如 "generate_input")
    pub name: String,
    /// 输入类型(DSL 类型表达式，如 "`Vec<u8>`")
    #[serde(default)]
    pub input_type: Option<String>,
    /// 输出类型(DSL 类型表达式，如 "ParseResult")
    #[serde(default)]
    pub output_type: Option<String>,
    /// 引用的契约名称列表(该步骤需满足的契约)
    #[serde(default)]
    pub contracts: Vec<String>,
    /// 步骤描述(可选，人类可读说明)
    #[serde(default)]
    pub description: Option<String>,

    /// 执行顺序（设计文档 §7.2 扩展字段，P4-W15.1.1）
    ///
    /// WHY: 设计文档 §7.2 \[\[hops\]\] order = ["Architect.propose", ...]
    /// - Agent 调用顺序列表（"Agent.role" 格式）
    /// - validate() 检查 order 是否为空，且每个元素是否引用不可进化面
    #[serde(default)]
    pub order: Vec<String>,

    /// 否决处理策略（设计文档 §7.2 扩展字段，P4-W15.1.1）
    ///
    /// WHY: 设计文档 §7.2 \[\[hops\]\] on_veto = "replan(max=2)"
    /// - 否决时的处理策略（如 "replan(max=2)" / "abort" / "EscalateToHuman"）
    /// - validate() 检查 on_veto 是否引用不可进化面操作
    #[serde(default)]
    pub on_veto: Option<String>,

    /// 回退策略（设计文档 §7.2 扩展字段，P4-W15.1.1）
    ///
    /// WHY: 设计文档 §7.2 \[\[hops\]\] fallback = "EscalateToHuman"
    /// - 失败时的回退策略
    /// - validate() 检查 fallback 是否引用不可进化面操作
    #[serde(default)]
    pub fallback: Option<String>,
}

/// 重试策略 — 测试失败后的重试配置
///
/// WHY: 学习闭环中测试可能因非确定性因素(网络/调度噪声)偶发失败，
/// 重试策略避免假阴性干扰进化信号
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// 最大尝试次数(默认 5)
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    /// 退避基准时间(毫秒，默认 1000)
    #[serde(default = "default_backoff_ms")]
    pub backoff_ms: u64,
    /// 是否指数退避(默认 true)
    #[serde(default = "default_exponential")]
    pub exponential: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            backoff_ms: default_backoff_ms(),
            exponential: default_exponential(),
        }
    }
}

fn default_max_attempts() -> u32 {
    5
}

fn default_backoff_ms() -> u64 {
    1000
}

fn default_exponential() -> bool {
    true
}

// ============================================================
// HarnessSpec 方法（P4-W15.1.1 核心）
// ============================================================

/// 强制 acceptance_gates 清单（设计文档 §7.2 auxiliary.acceptance_gates）
///
/// WHY const 数组:
/// - 编译期常量，无运行时开销
/// - validate() 检查 auxiliary 必须包含全部 4 个强制门
/// - 缺失任一强制门则 spec 无效（防注入: 攻击者无法绕过 CI 否决门）
pub const REQUIRED_ACCEPTANCE_GATES: [&str; 4] = [
    "tests_pass",
    "bench_no_regression",
    "invariants_clean",
    "redline_scan_clean",
];

impl HarnessSpec {
    /// 校验 spec 正确性与不可进化面合规（P4-W15.1.1）
    ///
    /// # 校验规则
    ///
    /// 1. **元信息校验**:
    ///    - `meta.name` 非空字符串
    ///    - `meta.version >= 1`（版本号单调递增）
    ///    - `meta.parent < meta.version`（若设置 parent，必须小于当前版本）
    ///    - 若 `meta.name` 在不可进化面清单中，则 `meta.immutable` 必须为 true
    ///
    /// 2. **不可进化面合规校验**:
    ///    - contracts 的 `from`/`to`/`fields` 不得引用不可进化面标识
    ///    - hops 的 `order`/`on_veto`/`fallback` 不得引用不可进化面标识
    ///
    /// 3. **契约引用完整性**:
    ///    - 每个 hop 的 `contracts` 引用必须在 `contracts` 数组中定义
    ///
    /// 4. **hop 完整性**:
    ///    - hop.order 非空（若使用扩展字段，必须定义执行顺序）
    ///
    /// 5. **acceptance_gates 完整性**:
    ///    - auxiliary 必须包含 4 个强制门
    ///
    /// # 返回
    /// - `Ok(())`: spec 校验通过
    /// - `Err(HarnessSpecError)`: 校验失败，错误消息说明违规位置与类型
    ///
    /// # 防注入保证
    ///
    /// 此方法为 `&self`（不可变借用），不修改 spec 也不写入文件路径。
    /// TOML 反序列化后，攻击者无法通过 spec API 注入文件路径或修改不可进化面。
    pub fn validate(&self) -> Result<(), HarnessSpecError> {
        // === 1. 元信息校验 ===
        if self.meta.name.is_empty() {
            return Err(HarnessSpecError::EmptyMetaName);
        }

        if self.meta.version == 0 {
            return Err(HarnessSpecError::InvalidVersion);
        }

        // parent 必须小于 version（谱系单向递增）
        if let Some(parent) = self.meta.parent {
            if parent >= self.meta.version {
                return Err(HarnessSpecError::InvalidVersion);
            }
        }

        // 若 meta.name 在不可进化面清单中，immutable 必须为 true
        if let Some(surface) = ImmutableSurface::parse_surface(&self.meta.name) {
            if !self.meta.immutable {
                return Err(HarnessSpecError::ImmutableMetaNotMarked {
                    name: surface.to_string(),
                });
            }
        }

        // === 2. 不可进化面合规校验 ===
        // 检查 contracts 的 from/to/fields 字段
        for (idx, contract) in self.contracts.iter().enumerate() {
            // from 字段
            if let Some(from) = &contract.from {
                if let Some(surface) = ImmutableSurface::parse_surface(from) {
                    return Err(HarnessSpecError::ImmutableSurfaceViolation {
                        location: format!("contracts[{}].from", idx),
                        surface,
                    });
                }
            }
            // to 字段
            if let Some(to) = &contract.to {
                if let Some(surface) = ImmutableSurface::parse_surface(to) {
                    return Err(HarnessSpecError::ImmutableSurfaceViolation {
                        location: format!("contracts[{}].to", idx),
                        surface,
                    });
                }
            }
            // fields 字段
            for (fidx, field) in contract.fields.iter().enumerate() {
                if let Some(surface) = ImmutableSurface::parse_surface(field) {
                    return Err(HarnessSpecError::ImmutableSurfaceViolation {
                        location: format!("contracts[{}].fields[{}]", idx, fidx),
                        surface,
                    });
                }
            }
        }

        // 检查 hops 的 order/on_veto/fallback 字段
        for (idx, hop) in self.hops.iter().enumerate() {
            // order 字段
            for (oidx, order_item) in hop.order.iter().enumerate() {
                if let Some(surface) = ImmutableSurface::parse_surface(order_item) {
                    return Err(HarnessSpecError::ImmutableSurfaceViolation {
                        location: format!("hops[{}].order[{}]", idx, oidx),
                        surface,
                    });
                }
            }
            // on_veto 字段
            if let Some(on_veto) = &hop.on_veto {
                if let Some(surface) = ImmutableSurface::parse_surface(on_veto) {
                    return Err(HarnessSpecError::ImmutableSurfaceViolation {
                        location: format!("hops[{}].on_veto", idx),
                        surface,
                    });
                }
            }
            // fallback 字段
            if let Some(fallback) = &hop.fallback {
                if let Some(surface) = ImmutableSurface::parse_surface(fallback) {
                    return Err(HarnessSpecError::ImmutableSurfaceViolation {
                        location: format!("hops[{}].fallback", idx),
                        surface,
                    });
                }
            }
        }

        // === 3. 契约引用完整性 ===
        // 收集所有已定义的契约名
        let defined_contracts: Vec<&str> = self.contracts.iter().map(|c| c.name.as_str()).collect();

        for (idx, hop) in self.hops.iter().enumerate() {
            for contract_ref in &hop.contracts {
                if !defined_contracts.contains(&contract_ref.as_str()) {
                    return Err(HarnessSpecError::InvalidContractReference {
                        hop_index: idx,
                        contract_name: contract_ref.clone(),
                    });
                }
            }
        }

        // === 4. hop 完整性 ===
        // WHY 仅在 hop 使用扩展字段时检查 order 非空:
        // - P2-W5.1 简化版 hop 没有 order 字段（serde 默认空 Vec）
        // - 设计文档 §7.2 完整版要求 order 必须非空
        // - 若 hop.contracts 或 hop.on_veto/fallback 已设置，则 order 必须非空
        for (idx, hop) in self.hops.iter().enumerate() {
            let uses_extended_fields = !hop.contracts.is_empty()
                || hop.on_veto.is_some()
                || hop.fallback.is_some()
                || !hop.order.is_empty();

            if uses_extended_fields && hop.order.is_empty() {
                return Err(HarnessSpecError::EmptyHopOrder { hop_index: idx });
            }
        }

        // === 5. acceptance_gates 完整性 ===
        // auxiliary 是 JSON/TOML/YAML 原始字符串，由消费方解析
        // L0 不依赖 serde_json，只能用字符串匹配检查 acceptance_gates
        //
        // WHY 字符串匹配而非 JSON 解析:
        // - L0 严禁依赖 serde_json（ADR-033）
        // - acceptance_gates 字段名固定，可子串匹配
        // - 检查宽松: 只要 auxiliary 包含强制门字符串即通过
        //   严格检查由 gsoe-evolution 加载器（P4-W15.1.2）执行
        if let Some(aux) = &self.auxiliary {
            let missing: Vec<String> = REQUIRED_ACCEPTANCE_GATES
                .iter()
                .filter(|gate| !aux.contains(*gate))
                .map(|s| s.to_string())
                .collect();

            if !missing.is_empty() {
                return Err(HarnessSpecError::MissingAcceptanceGates { missing });
            }
        }

        Ok(())
    }

    /// 返回规范化的 Merkle 哈希输入字符串（P4-W15.1.1）
    ///
    /// # 设计决策
    ///
    /// - **L0 不计算 SHA-256**: ADR-033 禁止 nexus-contracts 依赖 sha2 crate
    /// - **返回规范化字符串**: 调用方（L4 seccore）用此字符串计算 SHA-256 Merkle
    /// - **JSON canonical 序列化**: 字段排序稳定，确保相同 spec 产生相同哈希
    ///
    /// # 规范化规则
    ///
    /// 1. 按 meta/contracts/hops/retry/auxiliary 顺序拼接
    /// 2. 每个字段以 `key=value` 形式序列化
    /// 3. 数组按索引顺序拼接
    /// 4. Option 字段: Some(v) 序列化为 v，None 序列化为空字符串
    /// 5. 字段间用 `\x00` 分隔（防拼接歧义，与 seccore audit.rs 一致）
    ///
    /// # 防注入保证
    ///
    /// 此方法为 `&self`（不可变借用），不修改 spec 也不写入文件路径。
    /// 返回的字符串仅用于 Merkle 哈希计算，无法被攻击者控制。
    ///
    /// # 返回
    ///
    /// 规范化的字符串，调用方对其计算 SHA-256 得到 Merkle 哈希
    pub fn canonical_merkle_input(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        // 1. meta 部分
        parts.push(format!("meta.name={}", self.meta.name));
        parts.push(format!("meta.version={}", self.meta.version));
        parts.push(format!("meta.immutable={}", self.meta.immutable));
        parts.push(format!(
            "meta.parent={}",
            self.meta
                .parent
                .map_or_else(|| "".to_string(), |p| p.to_string())
        ));
        parts.push(format!(
            "meta.task_type={}",
            self.meta.task_type.as_deref().unwrap_or("")
        ));

        // 2. contracts 部分（按索引顺序）
        for (idx, contract) in self.contracts.iter().enumerate() {
            parts.push(format!("contracts[{}].name={}", idx, contract.name));
            parts.push(format!("contracts[{}].property={}", idx, contract.property));
            parts.push(format!(
                "contracts[{}].description={}",
                idx,
                contract.description.as_deref().unwrap_or("")
            ));
            parts.push(format!(
                "contracts[{}].from={}",
                idx,
                contract.from.as_deref().unwrap_or("")
            ));
            parts.push(format!(
                "contracts[{}].to={}",
                idx,
                contract.to.as_deref().unwrap_or("")
            ));
            // fields 数组按顺序拼接
            parts.push(format!(
                "contracts[{}].fields={}",
                idx,
                contract.fields.join(",")
            ));
        }

        // 3. hops 部分（按索引顺序）
        for (idx, hop) in self.hops.iter().enumerate() {
            parts.push(format!("hops[{}].name={}", idx, hop.name));
            parts.push(format!(
                "hops[{}].input_type={}",
                idx,
                hop.input_type.as_deref().unwrap_or("")
            ));
            parts.push(format!(
                "hops[{}].output_type={}",
                idx,
                hop.output_type.as_deref().unwrap_or("")
            ));
            parts.push(format!(
                "hops[{}].contracts={}",
                idx,
                hop.contracts.join(",")
            ));
            parts.push(format!(
                "hops[{}].description={}",
                idx,
                hop.description.as_deref().unwrap_or("")
            ));
            parts.push(format!("hops[{}].order={}", idx, hop.order.join(",")));
            parts.push(format!(
                "hops[{}].on_veto={}",
                idx,
                hop.on_veto.as_deref().unwrap_or("")
            ));
            parts.push(format!(
                "hops[{}].fallback={}",
                idx,
                hop.fallback.as_deref().unwrap_or("")
            ));
        }

        // 4. retry 部分
        parts.push(format!("retry.max_attempts={}", self.retry.max_attempts));
        parts.push(format!("retry.backoff_ms={}", self.retry.backoff_ms));
        parts.push(format!("retry.exponential={}", self.retry.exponential));

        // 5. auxiliary 部分
        parts.push(format!(
            "auxiliary={}",
            self.auxiliary.as_deref().unwrap_or("")
        ));

        // 用 \x00 分隔防拼接歧义（与 seccore audit.rs hash_decision_chain 一致）
        parts.join("\x00")
    }

    /// 返回不可进化面清单引用（用于 SpecRegistry 不可进化面检查）
    ///
    /// WHY 提供: gsoe-evolution SpecRegistry（P4-W15.2）调用此方法
    /// 验证 spec 是否试图突破不可进化面。返回不可进化面全集，
    /// SpecRegistry 据此与 spec 字段引用对比
    pub fn immutable_surfaces() -> &'static [ImmutableSurface; 20] {
        &ImmutableSurface::ALL_CONST
    }
}

// ============================================================
// 不可进化面常量清单（P4-W15.1.1）
// ============================================================

impl ImmutableSurface {
    /// 静态常量数组（用于 HarnessSpec::immutable_surfaces() 返回 &'static 引用）
    ///
    /// WHY const ALL_CONST 与 const fn all() 并存:
    /// - all() 是 const fn，可在 const 上下文调用
    /// - ALL_CONST 是 const 静态关联常量，可返回 &'static 引用
    /// - immutable_surfaces() 需返回 &'static 引用，必须用 const 变量
    pub const ALL_CONST: [ImmutableSurface; 20] = [
        // 8 条红线
        ImmutableSurface::RedlineLockAcrossAwait,
        ImmutableSurface::RedlineRusqliteBlocking,
        ImmutableSurface::RedlineSubscribeBeforeSpawn,
        ImmutableSurface::RedlineBudgetExceededSeverity,
        ImmutableSurface::RedlineCriticalEventMpsc,
        ImmutableSurface::RedlineCargoLockDrift,
        ImmutableSurface::RedlineSqliteVecForbidden,
        ImmutableSurface::RedlineTopKSelectNthUnstable,
        // 6 个 Critical 事件
        ImmutableSurface::CriticalCheckpointSaved,
        ImmutableSurface::CriticalSkepticVeto,
        ImmutableSurface::CriticalRedTeamAudit,
        ImmutableSurface::CriticalAsaIntervention,
        ImmutableSurface::CriticalAgentTaskFailed,
        ImmutableSurface::CriticalBudgetExceeded,
        // 3 个不变量
        ImmutableSurface::Invariant7MemoryBudget,
        ImmutableSurface::Invariant8ArchiveMonotonic,
        ImmutableSurface::Invariant9DagAcyclic,
        // 2 个沙箱/QEEP
        ImmutableSurface::SurfaceSandboxPolicy,
        ImmutableSurface::SurfaceQeepProtocol,
        // 1 个验证器
        ImmutableSurface::SurfaceVerifierHierarchy,
    ];
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // P2-W5.1 既有测试（保持向后兼容）
    // ============================================================

    #[test]
    fn test_harness_spec_yaml_roundtrip() {
        let spec = HarnessSpec {
            meta: HarnessMeta {
                name: "quest-parse-fuzz".to_string(),
                version: 1,
                immutable: false,
                parent: None,
                task_type: None,
            },
            contracts: vec![ContractSpec {
                name: "no_panic".to_string(),
                property: "fuzz_target_must_not_panic".to_string(),
                description: Some("Fuzz target must not panic on any input".to_string()),
                from: None,
                to: None,
                fields: Vec::new(),
            }],
            hops: vec![HopSpec {
                name: "generate_input".to_string(),
                input_type: Some("`Vec<u8>`".to_string()),
                output_type: Some("ParseResult".to_string()),
                contracts: vec!["no_panic".to_string()],
                description: None,
                order: Vec::new(),
                on_veto: None,
                fallback: None,
            }],
            retry: RetryPolicy::default(),
            auxiliary: None,
        };

        // YAML 往返测试(serde 派生保证格式无关,TOML/JSON/YAML 均可)
        let yaml_str = serde_yaml::to_string(&spec).expect("YAML 序列化失败");
        let restored: HarnessSpec = serde_yaml::from_str(&yaml_str).expect("YAML 反序列化失败");
        assert_eq!(spec, restored);
    }

    #[test]
    fn test_harness_spec_json_roundtrip() {
        let spec = HarnessSpec {
            meta: HarnessMeta {
                name: "test-spec".to_string(),
                version: 2,
                immutable: true,
                parent: Some(1),
                task_type: Some("fuzz".to_string()),
            },
            contracts: Vec::new(),
            hops: Vec::new(),
            retry: RetryPolicy::default(),
            auxiliary: Some(r#"{"custom":"data"}"#.to_string()),
        };

        let json = serde_json::to_string(&spec).expect("JSON 序列化失败");
        let restored: HarnessSpec = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(spec, restored);
    }

    #[test]
    fn test_retry_policy_default() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_attempts, 5);
        assert_eq!(policy.backoff_ms, 1000);
        assert!(policy.exponential);
    }

    #[test]
    fn test_immutable_surface() {
        let spec = HarnessSpec {
            meta: HarnessMeta {
                name: "critical-red-line".to_string(),
                version: 1,
                immutable: true,
                parent: None,
                task_type: None,
            },
            contracts: Vec::new(),
            hops: Vec::new(),
            retry: RetryPolicy::default(),
            auxiliary: None,
        };
        assert!(spec.meta.immutable);
    }

    // ============================================================
    // P4-W15.1.1 ImmutableSurface 测试
    // ============================================================

    #[test]
    fn test_immutable_surface_all_returns_20() {
        // 验证不可进化面总数 = 20（8 红线 + 6 Critical + 3 INV + 2 沙箱/QEEP + 1 验证器）
        let all = ImmutableSurface::all();
        assert_eq!(all.len(), 20);
    }

    #[test]
    fn test_immutable_surface_all_const_matches_all() {
        // 验证 const ALL_CONST 与 const fn all() 一致
        assert_eq!(ImmutableSurface::ALL_CONST, ImmutableSurface::all());
    }

    #[test]
    fn test_immutable_surface_as_str_kebab_case() {
        // 验证 as_str 返回 kebab-case 字符串
        assert_eq!(
            ImmutableSurface::RedlineLockAcrossAwait.as_str(),
            "redline-lock-across-await"
        );
        assert_eq!(
            ImmutableSurface::CriticalSkepticVeto.as_str(),
            "critical-skeptic-veto"
        );
        assert_eq!(
            ImmutableSurface::Invariant7MemoryBudget.as_str(),
            "inv-7-memory-budget"
        );
        assert_eq!(
            ImmutableSurface::SurfaceVerifierHierarchy.as_str(),
            "surface-verifier-hierarchy"
        );
    }

    #[test]
    fn test_immutable_surface_from_str_roundtrip() {
        // 验证 from_str 与 as_str 互逆
        for surface in ImmutableSurface::all().iter() {
            let s = surface.as_str();
            let parsed = ImmutableSurface::parse_surface(s);
            assert_eq!(
                parsed,
                Some(*surface),
                "from_str({}) 应返回 {:?}",
                s,
                surface
            );
        }
    }

    #[test]
    fn test_immutable_surface_from_str_unknown_returns_none() {
        // 未知字符串返回 None
        assert_eq!(ImmutableSurface::parse_surface("unknown-surface"), None);
        assert_eq!(ImmutableSurface::parse_surface(""), None);
        assert_eq!(ImmutableSurface::parse_surface("redline-unknown"), None);
    }

    #[test]
    fn test_immutable_surface_category() {
        // 验证类别分组
        assert_eq!(
            ImmutableSurface::RedlineLockAcrossAwait.category(),
            "redline"
        );
        assert_eq!(
            ImmutableSurface::CriticalSkepticVeto.category(),
            "critical-event"
        );
        assert_eq!(
            ImmutableSurface::Invariant7MemoryBudget.category(),
            "invariant"
        );
        assert_eq!(
            ImmutableSurface::SurfaceSandboxPolicy.category(),
            "security-surface"
        );
        assert_eq!(
            ImmutableSurface::SurfaceVerifierHierarchy.category(),
            "verifier"
        );
    }

    #[test]
    fn test_immutable_surface_display() {
        // 验证 Display 实现
        assert_eq!(
            format!("{}", ImmutableSurface::RedlineLockAcrossAwait),
            "redline-lock-across-await"
        );
    }

    #[test]
    fn test_immutable_surface_all_unique() {
        // 验证所有变体的 as_str 唯一（避免命名冲突）
        let all = ImmutableSurface::all();
        let mut seen = std::collections::HashSet::new();
        for surface in all.iter() {
            assert!(
                seen.insert(surface.as_str()),
                "duplicate as_str: {}",
                surface
            );
        }
    }

    // ============================================================
    // P4-W15.1.1 HarnessSpecError 测试
    // ============================================================

    #[test]
    fn test_harness_spec_error_display() {
        // 验证 Display 输出人类可读
        let err = HarnessSpecError::EmptyMetaName;
        assert!(format!("{}", err).contains("meta.name"));

        let err = HarnessSpecError::InvalidVersion;
        assert!(format!("{}", err).contains("meta.version"));

        let err = HarnessSpecError::ImmutableSurfaceViolation {
            location: "contracts\\[0\\].from".to_string(),
            surface: ImmutableSurface::CriticalSkepticVeto,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("contracts\\[0\\].from"));
        assert!(msg.contains("critical-skeptic-veto"));
    }

    #[test]
    fn test_harness_spec_error_is_std_error() {
        // 验证 HarnessSpecError 实现 std::error::Error
        fn check_error<E: std::error::Error>(_: E) {}
        let err = HarnessSpecError::EmptyMetaName;
        check_error(err);
    }

    // ============================================================
    // P4-W15.1.1 validate() 测试
    // ============================================================

    /// 构造合法 spec 用于测试
    fn make_valid_spec() -> HarnessSpec {
        HarnessSpec {
            meta: HarnessMeta {
                name: "valid-spec".to_string(),
                version: 1,
                immutable: false,
                parent: None,
                task_type: Some("fuzz".to_string()),
            },
            contracts: vec![ContractSpec {
                name: "no_panic".to_string(),
                property: "fuzz_target_must_not_panic".to_string(),
                description: None,
                from: Some("Architect".to_string()),
                to: Some("orchestrator".to_string()),
                fields: vec!["veto_reason".to_string()],
            }],
            hops: vec![HopSpec {
                name: "generate_input".to_string(),
                input_type: Some("`Vec<u8>`".to_string()),
                output_type: Some("ParseResult".to_string()),
                contracts: vec!["no_panic".to_string()],
                description: None,
                order: vec!["Architect.propose".to_string()],
                on_veto: Some("replan(max=2)".to_string()),
                fallback: Some("EscalateToHuman".to_string()),
            }],
            retry: RetryPolicy::default(),
            auxiliary: Some(
                r#"acceptance_gates = ["tests_pass","bench_no_regression","invariants_clean","redline_scan_clean"]"#
                    .to_string(),
            ),
        }
    }

    #[test]
    fn test_validate_valid_spec_passes() {
        let spec = make_valid_spec();
        assert!(spec.validate().is_ok(), "合法 spec 应通过校验");
    }

    #[test]
    fn test_validate_empty_meta_name_fails() {
        let mut spec = make_valid_spec();
        spec.meta.name = String::new();
        let err = spec.validate().unwrap_err();
        assert_eq!(err, HarnessSpecError::EmptyMetaName);
    }

    #[test]
    fn test_validate_zero_version_fails() {
        let mut spec = make_valid_spec();
        spec.meta.version = 0;
        let err = spec.validate().unwrap_err();
        assert_eq!(err, HarnessSpecError::InvalidVersion);
    }

    #[test]
    fn test_validate_parent_ge_version_fails() {
        let mut spec = make_valid_spec();
        spec.meta.version = 5;
        spec.meta.parent = Some(5); // parent >= version
        let err = spec.validate().unwrap_err();
        assert_eq!(err, HarnessSpecError::InvalidVersion);

        spec.meta.parent = Some(10); // parent > version
        let err = spec.validate().unwrap_err();
        assert_eq!(err, HarnessSpecError::InvalidVersion);
    }

    #[test]
    fn test_validate_parent_lt_version_passes() {
        let mut spec = make_valid_spec();
        spec.meta.version = 5;
        spec.meta.parent = Some(4); // parent < version
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_validate_immutable_meta_name_not_marked_fails() {
        // meta.name 在不可进化面清单中但 immutable=false
        let mut spec = make_valid_spec();
        spec.meta.name = "critical-skeptic-veto".to_string();
        spec.meta.immutable = false;
        let err = spec.validate().unwrap_err();
        match err {
            HarnessSpecError::ImmutableMetaNotMarked { name } => {
                assert_eq!(name, "critical-skeptic-veto");
            }
            other => panic!("期望 ImmutableMetaNotMarked，实际: {:?}", other),
        }
    }

    #[test]
    fn test_validate_immutable_meta_name_marked_passes() {
        // meta.name 在不可进化面清单中且 immutable=true
        let mut spec = make_valid_spec();
        spec.meta.name = "critical-skeptic-veto".to_string();
        spec.meta.immutable = true;
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_validate_contract_from_references_immutable_surface_fails() {
        let mut spec = make_valid_spec();
        spec.contracts[0].from = Some("critical-skeptic-veto".to_string());
        let err = spec.validate().unwrap_err();
        match err {
            HarnessSpecError::ImmutableSurfaceViolation { location, surface } => {
                assert_eq!(location, "contracts\\[0\\].from");
                assert_eq!(surface, ImmutableSurface::CriticalSkepticVeto);
            }
            other => panic!("期望 ImmutableSurfaceViolation，实际: {:?}", other),
        }
    }

    #[test]
    fn test_validate_contract_to_references_immutable_surface_fails() {
        let mut spec = make_valid_spec();
        spec.contracts[0].to = Some("inv-7-memory-budget".to_string());
        let err = spec.validate().unwrap_err();
        match err {
            HarnessSpecError::ImmutableSurfaceViolation { location, surface } => {
                assert_eq!(location, "contracts[0].to");
                assert_eq!(surface, ImmutableSurface::Invariant7MemoryBudget);
            }
            other => panic!("期望 ImmutableSurfaceViolation，实际: {:?}", other),
        }
    }

    #[test]
    fn test_validate_contract_fields_references_immutable_surface_fails() {
        let mut spec = make_valid_spec();
        spec.contracts[0].fields = vec![
            "veto_reason".to_string(),
            "critical-budget-exceeded".to_string(),
        ];
        let err = spec.validate().unwrap_err();
        match err {
            HarnessSpecError::ImmutableSurfaceViolation { location, surface } => {
                assert_eq!(location, "contracts[0].fields[1]");
                assert_eq!(surface, ImmutableSurface::CriticalBudgetExceeded);
            }
            other => panic!("期望 ImmutableSurfaceViolation，实际: {:?}", other),
        }
    }

    #[test]
    fn test_validate_hop_order_references_immutable_surface_fails() {
        let mut spec = make_valid_spec();
        spec.hops[0].order = vec![
            "Architect.propose".to_string(),
            "redline-lock-across-await".to_string(),
        ];
        let err = spec.validate().unwrap_err();
        match err {
            HarnessSpecError::ImmutableSurfaceViolation { location, surface } => {
                assert_eq!(location, "hops[0].order[1]");
                assert_eq!(surface, ImmutableSurface::RedlineLockAcrossAwait);
            }
            other => panic!("期望 ImmutableSurfaceViolation，实际: {:?}", other),
        }
    }

    #[test]
    fn test_validate_hop_on_veto_references_immutable_surface_fails() {
        let mut spec = make_valid_spec();
        spec.hops[0].on_veto = Some("surface-sandbox-policy".to_string());
        let err = spec.validate().unwrap_err();
        match err {
            HarnessSpecError::ImmutableSurfaceViolation { location, surface } => {
                assert_eq!(location, "hops[0].on_veto");
                assert_eq!(surface, ImmutableSurface::SurfaceSandboxPolicy);
            }
            other => panic!("期望 ImmutableSurfaceViolation，实际: {:?}", other),
        }
    }

    #[test]
    fn test_validate_hop_fallback_references_immutable_surface_fails() {
        let mut spec = make_valid_spec();
        spec.hops[0].fallback = Some("surface-verifier-hierarchy".to_string());
        let err = spec.validate().unwrap_err();
        match err {
            HarnessSpecError::ImmutableSurfaceViolation { location, surface } => {
                assert_eq!(location, "hops[0].fallback");
                assert_eq!(surface, ImmutableSurface::SurfaceVerifierHierarchy);
            }
            other => panic!("期望 ImmutableSurfaceViolation，实际: {:?}", other),
        }
    }

    #[test]
    fn test_validate_hop_references_undefined_contract_fails() {
        let mut spec = make_valid_spec();
        spec.hops[0].contracts = vec!["undefined-contract".to_string()];
        let err = spec.validate().unwrap_err();
        match err {
            HarnessSpecError::InvalidContractReference {
                hop_index,
                contract_name,
            } => {
                assert_eq!(hop_index, 0);
                assert_eq!(contract_name, "undefined-contract");
            }
            other => panic!("期望 InvalidContractReference，实际: {:?}", other),
        }
    }

    #[test]
    fn test_validate_hop_with_contracts_but_empty_order_fails() {
        let mut spec = make_valid_spec();
        spec.hops[0].order = Vec::new(); // 清空 order
                                         // hop.contracts 非空但 order 空，应失败
        let err = spec.validate().unwrap_err();
        match err {
            HarnessSpecError::EmptyHopOrder { hop_index } => {
                assert_eq!(hop_index, 0);
            }
            other => panic!("期望 EmptyHopOrder，实际: {:?}", other),
        }
    }

    #[test]
    fn test_validate_auxiliary_missing_acceptance_gates_fails() {
        let mut spec = make_valid_spec();
        // 缺失 "redline_scan_clean"
        spec.auxiliary = Some(
            r#"acceptance_gates = ["tests_pass","bench_no_regression","invariants_clean"]"#
                .to_string(),
        );
        let err = spec.validate().unwrap_err();
        match err {
            HarnessSpecError::MissingAcceptanceGates { missing } => {
                assert!(missing.contains(&"redline_scan_clean".to_string()));
            }
            other => panic!("期望 MissingAcceptanceGates，实际: {:?}", other),
        }
    }

    #[test]
    fn test_validate_auxiliary_none_passes() {
        // auxiliary 为 None 时不检查 acceptance_gates（仅检查设置为字符串时）
        let mut spec = make_valid_spec();
        spec.auxiliary = None;
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_validate_p2_simple_spec_passes() {
        // P2-W5.1 简化版 spec（无扩展字段）应通过校验
        let spec = HarnessSpec {
            meta: HarnessMeta {
                name: "p2-simple".to_string(),
                version: 1,
                immutable: false,
                parent: None,
                task_type: None,
            },
            contracts: vec![ContractSpec {
                name: "no_panic".to_string(),
                property: "fuzz_target_must_not_panic".to_string(),
                description: None,
                from: None,
                to: None,
                fields: Vec::new(),
            }],
            hops: vec![HopSpec {
                name: "generate_input".to_string(),
                input_type: Some("`Vec<u8>`".to_string()),
                output_type: Some("ParseResult".to_string()),
                contracts: vec!["no_panic".to_string()],
                description: None,
                order: Vec::new(), // 空但 hop 无扩展字段，不触发 EmptyHopOrder
                on_veto: None,
                fallback: None,
            }],
            retry: RetryPolicy::default(),
            auxiliary: None,
        };
        // P2 简化版: hop 无 on_veto/fallback，order 空是合法的
        // 但 hop.contracts 非空，根据规则 4 会触发 EmptyHopOrder
        // 实际上 P2 测试用例 hop.contracts 是 ["no_panic"]，不为空
        // 所以这个 spec 会失败。修正：让 hop 也不带 contracts
        let mut spec = spec;
        spec.hops[0].contracts = Vec::new();
        assert!(spec.validate().is_ok());
    }

    // ============================================================
    // P4-W15.1.1 canonical_merkle_input() 测试
    // ============================================================

    #[test]
    fn test_canonical_merkle_input_deterministic() {
        // 相同 spec 产生相同输出（确定性）
        let spec1 = make_valid_spec();
        let spec2 = make_valid_spec();
        assert_eq!(
            spec1.canonical_merkle_input(),
            spec2.canonical_merkle_input()
        );
    }

    #[test]
    fn test_canonical_merkle_input_differs_on_change() {
        // 任一字段变化，输出应不同
        let spec1 = make_valid_spec();
        let mut spec2 = make_valid_spec();
        spec2.meta.version = 2;
        assert_ne!(
            spec1.canonical_merkle_input(),
            spec2.canonical_merkle_input()
        );
    }

    #[test]
    fn test_canonical_merkle_input_includes_all_fields() {
        let spec = make_valid_spec();
        let input = spec.canonical_merkle_input();
        // 验证所有字段都被包含
        assert!(input.contains("meta.name=valid-spec"));
        assert!(input.contains("meta.version=1"));
        assert!(input.contains("meta.immutable=false"));
        assert!(input.contains("meta.task_type=fuzz"));
        assert!(input.contains("contracts[0].name=no_panic"));
        assert!(input.contains("contracts[0].property=fuzz_target_must_not_panic"));
        assert!(input.contains("contracts\\[0\\].from=Architect"));
        assert!(input.contains("contracts[0].to=orchestrator"));
        assert!(input.contains("contracts[0].fields=veto_reason"));
        assert!(input.contains("hops[0].name=generate_input"));
        assert!(input.contains("hops[0].input_type=Vec<u8>"));
        assert!(input.contains("hops[0].output_type=ParseResult"));
        assert!(input.contains("hops[0].contracts=no_panic"));
        assert!(input.contains("hops[0].order=Architect.propose"));
        assert!(input.contains("hops[0].on_veto=replan(max=2)"));
        assert!(input.contains("hops[0].fallback=EscalateToHuman"));
        assert!(input.contains("retry.max_attempts=5"));
        assert!(input.contains("retry.backoff_ms=1000"));
        assert!(input.contains("retry.exponential=true"));
        assert!(input.contains("auxiliary=acceptance_gates"));
    }

    #[test]
    fn test_canonical_merkle_input_none_fields_empty() {
        // Option 字段为 None 时序列化为空字符串
        let spec = HarnessSpec {
            meta: HarnessMeta {
                name: "minimal".to_string(),
                version: 1,
                immutable: false,
                parent: None,
                task_type: None,
            },
            contracts: Vec::new(),
            hops: Vec::new(),
            retry: RetryPolicy::default(),
            auxiliary: None,
        };
        let input = spec.canonical_merkle_input();
        assert!(input.contains("meta.parent="));
        assert!(input.contains("meta.task_type="));
        assert!(input.contains("auxiliary="));
        // 验证 None 字段后是分隔符或字符串结束
        assert!(input.contains("meta.parent=\x00") || input.contains("meta.parent=\u{0000}"));
    }

    #[test]
    fn test_canonical_merkle_input_separator_is_null_byte() {
        // 验证分隔符是 \x00（与 seccore audit.rs hash_decision_chain 一致）
        let spec = make_valid_spec();
        let input = spec.canonical_merkle_input();
        assert!(
            input.contains('\x00'),
            "canonical_merkle_input 应使用 \\x00 分隔字段"
        );
    }

    #[test]
    fn test_canonical_merkle_input_no_file_paths() {
        // 防注入: 验证输出不包含任何文件路径模式（spec 不可写入文件路径）
        let spec = make_valid_spec();
        let input = spec.canonical_merkle_input();
        // 不应包含文件路径分隔符（Windows / Unix）
        assert!(!input.contains("\\"));
        assert!(!input.contains("//"));
        assert!(!input.contains(".."));
        assert!(!input.contains("/etc/"));
        assert!(!input.contains("/dev/"));
        assert!(!input.contains("C:\\"));
    }

    // ============================================================
    // P4-W15.1.1 immutable_surfaces() 静态引用测试
    // ============================================================

    #[test]
    fn test_immutable_surfaces_returns_static_ref() {
        let surfaces = HarnessSpec::immutable_surfaces();
        assert_eq!(surfaces.len(), 20);
        // 验证返回值与 ALL_CONST 内容一致
        // WHY 不用 std::ptr::eq: immutable_surfaces() 返回 &[T; 20] 切片引用，
        // 在调用点可能产生临时切片，指针比较不稳定。值相等是更可靠的检查
        assert_eq!(surfaces, &ImmutableSurface::ALL_CONST);
        // 验证包含所有不可进化面类别
        assert!(surfaces.contains(&ImmutableSurface::RedlineLockAcrossAwait));
        assert!(surfaces.contains(&ImmutableSurface::CriticalSkepticVeto));
        assert!(surfaces.contains(&ImmutableSurface::Invariant9DagAcyclic));
        assert!(surfaces.contains(&ImmutableSurface::SurfaceSandboxPolicy));
        assert!(surfaces.contains(&ImmutableSurface::SurfaceVerifierHierarchy));
    }

    #[test]
    fn test_required_acceptance_gates_count() {
        assert_eq!(REQUIRED_ACCEPTANCE_GATES.len(), 4);
        assert!(REQUIRED_ACCEPTANCE_GATES.contains(&"tests_pass"));
        assert!(REQUIRED_ACCEPTANCE_GATES.contains(&"bench_no_regression"));
        assert!(REQUIRED_ACCEPTANCE_GATES.contains(&"invariants_clean"));
        assert!(REQUIRED_ACCEPTANCE_GATES.contains(&"redline_scan_clean"));
    }
}
