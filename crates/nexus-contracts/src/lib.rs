//! NEXUS-OMEGA L0 契约层 — 纯类型定义，零逻辑，零 crate 依赖
//!
//! 对应架构层: **L0 Contracts**（nexus-core 之下，十层架构新增最低层）
//! 对应 ADR: **ADR-033**（L0 nexus-contracts 契约层建立）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §C6
//!
//! # 核心职责
//!
//! 承载跨层共享的**纯类型契约**，消除 L1 `nexus-core` 的上帝 crate 问题（被 7+ crate 依赖）
//! 与 L6 `osa-coordinator` 的星型耦合问题（3 router 均依赖 OSA 取 `OmniSparseMasks`）。
//!
//! # 设计约束（ADR-033）
//!
//! - **纯类型 + 零逻辑**: 仅类型定义与基础构造函数，不含业务逻辑
//! - **零 crate 依赖**: 禁止依赖任何 workspace crate（含 `nexus-core` / `event-bus`）
//! - **唯一外部依赖**: `serde`（仅 derive 宏，用于跨 crate 序列化）
//! - **依赖铁律扩展**: `L(N) → L(0)` 恒允许（任何层均可依赖 L0）
//!
//! # 承载类型（按功能分组，41 个源文件 = 40 个功能模块 + lib.rs）
//!
//! ## 稀疏掩码体系
//! | 类型 | 来源 | 消费层 |
//! |------|------|--------|
//! | `OmniSparseMasks` | 从 `osa-coordinator` 上提 | L2 HCW / L6 Router × 3 |
//! | `SparseMask<T>` | 从 `osa-coordinator` 上提 | L6 OSA / L2 HCW |
//! | `ToolId` / `FileId` / `MemoryId` / `OperationId` / `TaskId` | 从 `osa-coordinator` 上提 | L6 Router × 3 / L7 Execution |
//!
//! ## Harness Spec 与契约
//! | 类型 | 来源 | 消费层 |
//! |------|------|--------|
//! | `HarnessSpec` / `ContractSpec` / `HopSpec` / `RetryPolicy` / `ImmutableSurface` | 新建（P4 Harness-as-Spec） | L5 gsoe-evolution / L9 quest-engine |
//! | `BehaviorContract` / `ContractExample` / `ContractCheckOutcome` | 新建（polish-v2.7 P1-3，ADR-049） | L9 efficiency-monitor / L5 AEGIS |
//! | `ProceduralBlueprint` / `BlueprintStep` / `PlanViolation` | 新建（polish-v2.7 P4-7，ADR-049） | L5 repo-wiki / L9 quest-engine |
//! | `VariantId` / `VariantContract` | 新建（polish-v2.7 P3-2，ADR-051） | L8 parliament / L5 AEGIS |
//!
//! ## 策略契约（S1-S9 接缝）
//! | 类型 | 来源 | 消费层 |
//! |------|------|--------|
//! | `SelectorPolicy` / `SelectorWeights` | 新建（P3-W10.3 D1 修复） | L2 hcw-window / L6 omega-learner |
//! | `DensityPolicy` / `DensityTier` | 新建（P4-W13.2 S1 接缝） | L2 hcw-window / L6 omega-learner |
//! | `MemoryStrategy` / `MemoryStrategyPolicy` | 新建（P4-W14.1 S2 接缝） | L2 mlc-engine / L6 omega-learner |
//! | `MemoryTaskPhase` / `MemoryStrategyProvider` | 新建（Task 2 OSA S2 桥接） | L6 osa-coordinator / L6 omega-learner |
//! | `PrefetchStrategy` / `PrefetchPolicy` | 新建（P4-W14.2 S3 接缝） | L3 scc-cache / L6 omega-learner |
//! | `ActivationStrategy` / `ParliamentPolicy` | 新建（P4-W14.3 S5 接缝） | L8 parliament / L6 omega-learner |
//! | `DecayProfile` / `DecayPolicy` | 新建（P4-W14.4 S6 接缝） | L4 decay-engine / L6 omega-learner |
//! | `RecallQuota` / `RecallQuotaPolicy` | 新建（P4-W16.2.2 S7 接缝） | L6 omega-learner / L9 编排器 |
//! | `CapabilityToken` / `SeamId` | 新建（P4-W14.5 C4 合规） | L4 decay-engine / L6 omega-learner / L9 编排器 |
//!
//! ## 事件与领域类型
//! | 类型 | 来源 | 消费层 |
//! |------|------|--------|
//! | `EventMetadata` | 从 `event-bus` 下沉（Task 3.10） | L1 event-bus / L2-L10 |
//! | `TaskStatus` | 从 `nexus-core` 下沉（Task 3.10） | L1 nexus-core / L9 quest-engine |
//! | `Checkpoint` | 从 `nexus-core` 下沉（Task 3.10） | L1 nexus-core / L9 quest-engine |
//! | `ThinkingMode` / `MultimodalInput` / `UserIntent` / `Quest` / `Task` | 从 `nexus-core` 上提（ADR-054 决策 6） | L1 nexus-core / L9 quest-engine |
//! | `EventSeverity` / `TaskPriority` / `AgentStatus` | 从 `event-bus` 下沉（ADR-054 决策 6） | L1 event-bus / L9 chimera-mas |
//! | `TemporalMeta` / `TransitionType` | 新建（P3 时间扩展） | L2 mlc-engine |
//!
//! ## 安全与治理契约
//! | 类型 | 来源 | 消费层 |
//! |------|------|--------|
//! | `AttackType` / `Command` / `CommandPolicy` / `CommandValidator` | 从 `seccore` 上提（ADR-054 决策 3） | L4 seccore / L8 parliament |
//! | `BudgetTier` | 从 `decb-governor` 上提（ADR-054 决策 3） | L8 decb-governor / L9 quest-engine |
//! | `FormalProperty` / `InvariantSpec` / `VerificationResult` | 新建（T6-2 FormalVerifier 骨架） | L4 formal-verifier / L8 parliament |
//! | `ArchiveTier` / `assert_archive_monotonicity` | 新建（P0-2 INV-8 下沉） | L2 mlc-engine / L3 cmt-tiering |
//!
//! ## MCA 亲和与向量存储
//! | 类型 | 来源 | 消费层 |
//! |------|------|--------|
//! | `ProviderId` / `ProtocolDialect` / `CapabilitySet` / `ModelAffinitySpec` | 新建（ADR-065 MCA 体系） | L10 mca-gateway / L1 model-router |
//! | `AffinityRequest` / `AffinityResponse` / `ContentBlock` | 新建（ADR-065 MCA 体系） | L10 mca-gateway / L9 quest-engine |
//! | `VectorStore` / `VectorHit` / `VectorBackend` | 新建（P2-W7.3 向量抽象） | L5 repo-wiki / L2 mlc-engine |
//!
//! ## RL 与奖励体系
//! | 类型 | 来源 | 消费层 |
//! |------|------|--------|
//! | `RLState` / `RLAction` / `RLExperience` / `MemPiAction` | 新建（ADR-049 修订补齐） | L6 omega-learner / L3 cmt-tiering |
//! | `RewardSpec` / `RewardSignal` / `RewardLayer` / `SecuritySeverity` | 新建（Milestone C-1） | L6 omega-learner / L9 efficiency-monitor |
//!
//! ## 平台接地与配额
//! | 类型 | 来源 | 消费层 |
//! |------|------|--------|
//! | `PlatformGroundingSpec` / `GroundingRequirement` | 新建（Milestone B-4） | L9 efficiency-monitor RuntimeAuditor |
//! | `NamespaceQuota` / `QuotaLimits` | 新建（chimera-mas 配额上提） | L9 chimera-mas |
//!
//! ## 六维控制面与消息协议
//! | 类型 | 来源 | 消费层 |
//! |------|------|--------|
//! | `HarnessConfigContract` / `ContextAssemblyContract` / `ToolInteractionContract` | 新建（设计文档 §5.3.1 MemoHarness D1-D6） | L6 Router / L9 Quest / L10 Interface |
//! | `GenerationControlContract` / `TaskOrchestrationContract` / `MemoryManagementContract` / `OutputProcessingContract` | 新建（设计文档 §5.3.1 MemoHarness D1-D6） | L6 Router / L9 Quest / L10 Interface |
//! | `OmniMessage` / `ModelConfig` / `TokenUsage` | 新建（设计文档 §5.3.2 PenguinHarness） | L10 Interface / 外部环境 |
//!
//! ## v3.4.0 融合契约（OpenMLE / Dressage / MSCE / TencentDB / RL 预留）
//! | 类型 | 来源 | 消费层 |
//! |------|------|--------|
//! | `ExperienceCard` / `AtomicOperator` / `ThreeFactorScore` / `ErrorSignature` / `ExecutionStatus` | 新建（v3.4.0 §5.2 OpenMLE） | L1 event-bus / L7 PVL / L5 gsoe-evolution |
//! | `TokenLedgerEntry` / `SegmentMetadata` / `SegmentCreationReason` | 新建（v3.4.0 §5.3 Dressage） | L1 event-bus token-ledger / L7 segment-validation |
//! | `MemoryPyramidLevel` / `AtomicMemoryCard` / `SceneBlock` / `PersonaSummary` | 新建（v3.4.0 §5.4 MSCE + TencentDB） | L2 mlc-engine / L3 cmt-tiering |
//! | `SkillLifecycleState` / `SkillLifecycleContract` | 新建（v3.4.0 §5.5 MSCE） | L5 skill-graph / L6 skills-loader |
//! | `RLHook` / `SerializedPolicy` / `RLTrajectory` / `RLStateVector` / `RLActionVector` | 新建（v3.4.0 §5.7 RL 预留） | L1 rl-client / L2 memory-pyramid |
//! | `OperatorSelectionStrategy` / `StopStrategyConfig` + 六维 OpenMLE 扩展字段 | 扩展（v3.4.0 §5.6） | L6 Router / L9 Quest |
//!
//! ## 测试工具与纯函数（ADR-033 例外）
//! | 类型 | 来源 | 消费层 |
//! |------|------|--------|
//! | `scale_timeout` / `scaled_timeout!` | 新建（P9-T2 测试缩放，ADR-033 例外 1） | 全 workspace 测试 |
//! | `assert_archive_monotonicity` | 新建（P0-2 INV-8 下沉，ADR-033 例外 2） | L2 mlc-engine / L3 cmt-tiering |
//! | `CapabilityToken` EWMA 方法 | 新建（P4-W14.5 C4 合规，ADR-033 例外 3） | L4 decay-engine / L9 编排器 |
//! | `ExperienceCard` 三因子/状态纯函数 | 新建（v3.4.0 §5.2，ADR-033 例外 4） | L1 event-bus / L5 three-factor-selector |
//! | `PlatformGroundingSpec::from_doc/check` | 新建（Milestone B-4，纯函数先例） | L9 efficiency-monitor |
//! | `BehaviorContract::enforce` / `ProceduralBlueprint::validate_plan` | 新建（polish-v2.7，纯函数先例） | L9 efficiency-monitor / L9 quest-engine |
//!
//! > 注：后两类为 ADR-033"纯类型零逻辑"约束的**纯函数先例**（无 IO 无状态变更），
//! > 与 `archive_monotonicity` 同类；显式声明以消除审计误判（2026-08-16 修复 A2）。
//!
//! # 示例
//!
//! ```
//! use nexus_contracts::{OmniSparseMasks, SparseMask, ToolId};
//!
//! let routing = SparseMask::select_top_k(
//!     &[ToolId::new("tool-1"), ToolId::new("tool-2")],
//!     &[0.9, 0.1],
//!     1,
//! );
//! assert!(routing.is_active(&ToolId::new("tool-1")));
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]
#![doc(html_root_url = "https://docs.rs/nexus-contracts")]

// ============================================================
// 模块声明
// ============================================================

/// ID newtype 类型 — 五维度稀疏化的统一标识
pub mod ids;

/// 稀疏掩码容器 — 泛型 `SparseMask<T>`
pub mod masks;

/// 全维稀疏掩码 — 五维度掩码聚合体 `OmniSparseMasks`
pub mod omni_masks;

/// Harness-as-Spec 契约类型 — P4 学习闭环规格
pub mod harness_spec;

/// 时间元数据 — P3 时间感知记忆扩展
pub mod temporal;

/// 命名空间配额 — chimera-mas 资源配额上提
pub mod quota;

/// 向量存储契约 — 跨层向量检索抽象（VectorStore trait + VectorStoreExt 扩展）
pub mod vector;

/// 选择器策略契约 — D1 病理修复（selector 权重外置，P3-W10.3）
pub mod policy;

/// 密度档位策略契约 — S1 接缝（DDR/HCW 密度档位，P4-W13.2）
pub mod density;

/// 记忆策略契约 — S2 接缝（mlc-engine 记忆策略选择，P4-W14.1）
pub mod strategy;

/// OSA memory 维度自适应记忆策略契约 — S2 桥接层（Task 2）
///
/// 承载 `MemoryTaskPhase` 任务阶段枚举与 `MemoryStrategyProvider` trait，
/// 供 OSA memory 维度通过 L0 trait 调用 omega-learner S2（依赖铁律 §2.2 合规）。
pub mod memory_strategy;

/// 预取策略契约 — S3 接缝（scc-cache 预取策略选择，P4-W14.2）
pub mod prefetch;

/// Parliament 激活策略契约 — S5 接缝（Parliament 激活策略选择，P4-W14.3）
pub mod parliament_policy;

/// 衰减参数策略契约 — S6 接缝（decay-engine 衰减档位选择，P4-W14.4）
pub mod decay_profile;

/// 能力场令牌契约 — 学习策略灰度授权载体（P4-W14.5 C4 合规）
///
/// 承载 CapabilityToken 类型，使编排器在注入 Learned 策略前查询授权等级，
/// 实现 C4 合规要求的"运行时灰度走能力场，而非散落运行时布尔旗"。
/// ADR-033"纯类型 + 零逻辑"的第三个明确例外（EWMA 算法 + 灰度状态机），
/// 详见模块级文档。
pub mod capability_token;

/// 召回配额策略契约 — S7 接缝（R1 离线 RL，P4-W16.2.2）
///
/// 承载 R1（召回配额 CQL/IQL）接缝的离散动作空间 `RecallQuota`
/// 与灰度授权载体 `RecallQuotaPolicy`，供 L6 omega-learner 与 L9 编排器共享。
pub mod recall_quota;

/// 行为契约 — 类型使用规范契约(polish-v2.7 P1-3,ADR-049)
///
/// 承载 BehaviorContract(前置/后置/不变量断言)与 ContractExample,
/// 供 L9 RuntimeAuditor 审计与 L5 AEGIS Evolver 约束输入。
pub mod behavior_contract;

/// 变体契约 — Harness 变体标识与性能契约(polish-v2.7 P3-2,ADR-051)
///
/// 承载 VariantId(spec 主键复用)与 VariantContract(任务类型适用域 +
/// 性能承诺),供 L8 parliament 变体池与 L5 AEGIS 变体登记共享。
pub mod variant;

/// 流程蓝图契约 — 隐性流程经验载体(polish-v2.7 P4-7,ADR-049)
///
/// 承载 ProceduralBlueprint(步骤/前置/成功率)与纯函数计划校验,
/// 供 L5 repo-wiki 轨迹提取与 L9 quest-engine 计划预检共享。
pub mod blueprint;

/// 形式化属性定义框架 — FormalVerifier L4 骨架基础类型(T6-2)
///
/// 承载 PropertyCategory / VerificationResult / InvariantSpec / FormalProperty,
/// 供 L4 formal-verifier 验证器实现与 L8 parliament 审议时查询属性满足状态共享。
pub mod formal_props;

/// 工具计划契约 — 声明式 ToolPlan DSL + 校验 + 守卫常量（P3-T8,WI-16）
///
/// 承载 ToolPlan / ToolNode / ToolOp / PlanEdge / PlanError / guards,
/// 供 gqep-executor PlanRunner 解释执行（L0 纯类型契约层,零依赖）。
pub mod tool_plan;

/// 调度契约 — mas-sched 控制面类型先移 L0（P3-T2 补,ADR-033 先例）
///
/// 承载 TodoClaim / Lease / Quota / Priority / DenyReason / RenewOutcome /
/// ShouldRunVerdict / TaskId / HANDOFF,供 mas-sched PeerScheduler 与后续
/// chimera-mas 拆出依赖时经本契约承接（strangler:类型先移、接口后接）。
pub mod scheduler_contract;

/// 事件元数据契约 — L0 共享的事件追踪元信息(Task 3.10,ADR-033 扩展)
///
/// 承载 `EventMetadata`(event_id / timestamp / source),从 L1 `event-bus/src/payloads.rs`
/// 上提至 L0,缓解 L1 上帝 crate 病理(被 100+ 文件依赖)。
/// 依赖: chrono + uuid(ADR-033 Task 3.10 新增例外,基础类型库)。
pub mod event_metadata;

/// 任务状态契约 — L0 共享的 Task 生命周期枚举(Task 3.10,ADR-033 扩展)
///
/// 承载 `TaskStatus`(Pending/Running/Completed/Failed),从 L1 `nexus-core/src/types.rs`
/// 上提至 L0,缓解 L1 上帝 crate 病理(被 65+ 文件依赖)。纯 enum,仅 serde derive 依赖。
pub mod task;

/// 检查点契约 — L0 共享的 Quest 断点恢复快照(Task 3.10,ADR-033 扩展)
///
/// 承载 `Checkpoint`(quest_id / checkpoint_id / memory_snapshot_hash / serialized_state / created_at),
/// 从 L1 `nexus-core/src/types.rs` 上提至 L0,缓解 L1 上帝 crate 病理(被 42+ 文件依赖)。
/// 依赖: chrono(ADR-033 Task 3.10 新增例外,基础类型库)。
pub mod checkpoint;

/// 预算档位契约 — DECB 三档枚举上提(ADR-054 决策 3,P9-T3)
///
/// 承载 `BudgetTier`(HighTier/LowTier/Degraded),从 L8 `decb-governor` 上提至 L0,
/// 消除 L9 `quest-engine` 对 L8 的生产依赖边(L9→L8 违规方向,ADR-054 裁决)。
/// decb-governor 保留 re-export 保兼容;纯枚举,仅 serde derive 依赖(ADR-033)。
pub mod budget_tier;

/// 模型亲和契约 — MCA 体系 L0 类型(ADR-065,PANTHEON 计划)
///
/// 承载 ProviderId / ProtocolDialect / CapabilitySet / ModelAffinitySpec /
/// AffinityRequest / AffinityResponse 等"能力协商取代名字嗅探"（原则 P1）
/// 契约类型,供 L10 mca-gateway / L1 model-router / L6 omega-learner 共享。
pub mod affinity;

/// 命令验证契约 — 攻击类型/命令/策略/trait 上提(ADR-054 决策 3,P9-T4)
///
/// 承载 `AttackType` / `Command` / `BlockedPattern` / `CommandPolicy` /
/// `CommandValidationError` / `CommandValidator` trait,从 L4 `seccore` 上提至 L0,
/// 消除 L8 `parliament` 对 L4 `seccore` 的生产依赖违规边(ADR-054 裁决)。
/// seccore 保留 re-export 保兼容 + 实现 `CommandValidator`;parliament 改 L0 trait
/// 注入 + 未注入优雅降级(MemoryStrategyProvider 先例)。默认策略语义冻结迁移。
pub mod command_validation;

/// 纯领域类型契约 — 用户意图/长期任务/思考模式上提(ADR-054 决策 6,P9-T7)
///
/// 承载 `ThinkingMode` / `MultimodalInput` / `UserIntent` / `Quest` / `Task`,
/// 从 L1 `nexus-core` 上提至 L0,缓解 D1 上帝 crate 病理(nexus-core 被 30 依赖方引用)。
/// nexus-core 保留 re-export 保兼容;纯类型,仅 serde derive 依赖(ADR-033)。
pub mod domain;

/// 事件载荷契约 — 纯载荷枚举下沉(ADR-054 决策 6,P9-T7 Task 2)
///
/// 承载 `EventSeverity` / `TaskPriority` / `AgentStatus` 三个高价值纯载荷枚举,
/// 从 L1 `event-bus/src/payloads.rs` 下沉至 L0,缓解 L1 超级节点(34 依赖方)负担。
/// **severity() 判定逻辑不迁移**(架构红线:Critical 事件 mpsc 保障,判定逻辑留在
/// L1 event-bus);本模块仅承载纯枚举类型(变体/derive 与 L1 原定义逐字一致)。
pub mod event_payload;

/// P9-T2 测试等待缩放工具 — CHIMERA_TEST_TIMEOUT_SCALE 协议
///
/// 提供 `scale_timeout` / `scaled_timeout!` 宏,允许 PR fast 档通过环境变量
/// 收敛硬编码 2s+ 等待。是 nexus-contracts "纯类型 + 零逻辑" 约束的**唯一例外**:
/// 工具仅读 env,不引入任何领域逻辑;详见 module-level 文档。
pub mod test_scale;

/// RL 共享类型契约 — RLState/RLAction/RLExperience（ADR-049 修订补齐）
///
/// 承载跨接缝 RL 训练共享类型: RLAction（S1-S9 接缝动作封闭枚举）/ RLState
/// （上下文状态快照）/ RLExperience（经验四元组）。纯类型零逻辑（ADR-033）,
/// 不含训练逻辑（R2 冻结面外,ADR-042）;接缝映射 = 枚举变体包装既有契约类型。
pub mod rl_types;

/// 平台接地契约 — PlatformGroundingSpec（Milestone B-4，北大 NL2Pipeline gap 解）
///
/// 承载平台/环境约束的可审计契约（Env/Toolchain/Path/Permission/Config 五类），
/// 供 L9 efficiency-monitor RuntimeAuditor 第 0 维度（契约遵守）消费。纯类型零逻辑。
pub mod platform_grounding;

/// 奖励函数统一框架契约（Milestone C-1）— RewardSpec/RewardSignal/SecuritySeverity
///
/// 承载设计 §17 八维度权重表的 L0 契约：层权重 + 维度组件 + 奖励信号流载荷，
/// R1 数据面先接入，R2 训练面解冻后激活（ADR-042）；L4 安全奖励仅观测。
pub mod reward;

/// 归档单调性契约 — INV-8 独立公共 API（P0-2 审计修复）
///
/// 承载 `ArchiveTier`（Hot/Warm/Cold/Ice 四级）与 `assert_archive_monotonicity`
/// 判定函数（降级 + 同层保持合法，回升拒绝）。INV-8 判定此前仅存在于
/// L9 `chimera-mas`，本模块将其下沉至 L0，使 mlc-engine（L2）/ cmt-tiering（L3）
/// 的归档入口可独立执行单调性断言（依赖铁律 §2.2：L(N) → L(0) 恒允许）。
/// ADR-033"纯类型 + 零逻辑"的第二个明确例外（首个为 `test_scale`），
/// 详见模块级文档。
pub mod archive_monotonicity;

/// 六维控制面契约 — MemoHarness D1-D6 融合（设计文档 §5.3.1）
///
/// 承载 MemoHarness 六维控制面的跨层契约定义（D1 上下文组装 / D2 工具交互 /
/// D3 生成控制 / D4 任务编排 / D5 Memory 管理 / D6 输出处理），
/// 供 L6 Router / L9 Quest / L10 Interface 按统一契约调整 Harness 行为。
/// 纯类型零逻辑（ADR-033）；RetryPolicy 复用 harness_spec（L0 同层引用）。
pub mod harness_dimensions;

/// OmniMessage 协议 — 模型-环境解耦统一消息协议（设计文档 §5.3.2）
///
/// 承载 PenguinHarness OmniMessage 协议的 6 变体枚举（ModelRequest / ModelResponse /
/// ToolRequest / ToolResult / StateUpdate / TraceRecord），解耦 LLM 调用与环境执行。
/// 纯类型零逻辑（ADR-033）；JSON 字段用 `Box<str>`（遵循 affinity.rs 先例，
/// 保持 L0 零 crate 依赖铁律）。
pub mod omni_message;

/// 经验卡片契约 — OpenMLE 核心数据结构（v3.4.0 §5.2）
///
/// 承载 ExperienceCard / AtomicOperator / ThreeFactorScore / ErrorSignature /
/// ExecutionStatus / CardMetadata。不可变契约（铁律3）+ 纯函数（铁律4）,
/// ADR-033"纯类型 + 零逻辑"的第四个明确例外,详见模块级文档。
pub mod experience_card;

/// Token 级证据契约 — Dressage 融合（v3.4.0 §5.3）
///
/// 承载 TokenLedgerEntry / ToolCallRecord / SegmentMetadata /
/// SegmentCreationReason。纯类型零逻辑（ADR-033）;铁律9 分段身份
/// 不可篡改（parent_traj_id 共享 + anchor 承载终局 reward）。
pub mod token_evidence;

/// 记忆金字塔契约 — MSCE + TencentDB 融合（v3.4.0 §5.4）
///
/// 承载 MemoryPyramidLevel / AtomicMemoryCard / SceneBlock / PersonaSummary。
/// 纯类型 + 层级映射纯函数（ADR-033 先例）;同层引用 rl_hooks 向量类型。
pub mod memory_pyramid;

/// Skill 生命周期契约 — MSCE 融合（v3.4.0 §5.5）
///
/// 承载 SkillLifecycleState / SkillLifecycleContract 三态状态机。
/// 纯类型 + 状态转移纯函数（ADR-033 先例）。
pub mod skill_lifecycle;

/// RL 预留钩子契约 — v4.0 升级路径（v3.4.0 §5.7 + §17）
///
/// 承载 RLHook trait / SerializedPolicy / RLTrajectory / RLStateVector /
/// RLActionVector。同步 trait（L0 零依赖铁律，不引入 async-trait）;
/// 铁律6: 所有统计学习机制可导出为 RLTrajectory。
pub mod rl_hooks;

// ============================================================
// v4.0 统一执行总案 A0/WI-04/WI-05/WI-21 新增模块（2026-08-22）
// ============================================================

/// 图身份契约 — GIP 跨层成本归因三元组（WI-04）
///
/// 承载 GraphIdentity（goal_id/run_id/node_id），经 EventMetadata 可选字段
/// 渐进铺开；144 事件枚举本体不动（v4.0 §17 治理红线）。
pub mod graph_identity;

/// MCSM 流形约束信号守恒聚合 — Sinkhorn 双随机投影器（WI-05）
///
/// 纯函数投影器（ADR-033 纯函数先例）：聚合权重矩阵行列归一化，
/// 防单源 100× 音量淹没他源；含 identity() 直通回滚路径。
pub mod mcsm;

/// 外部协议契约 — AppOp/AppEvent 与 Thread/Turn/Item 三原语（WI-01）
///
/// 内闭外开（T6）：NexusEvent 永不进外部协议；本模块为转译层协议面类型。
pub mod app;

/// 事件双轨契约 — DynamicEvent 注册表与 EventMetadataV2（WI-21）
///
/// 轨一（内置 144 枚举）不动；轨二（动态注册）供 MCP/SubAgent/Hook 外部源。
pub mod event_v2;

/// 公共 Top-K 收敛工具 — 红线 #8(WS-2 C1)
///
/// 承载 `xts_top_k` / `xts_top_k_by`,以 O(n) `select_nth_unstable_by` +
/// O(k log k) 局部排序取代"全排后截断"的 `sort_by` 全排序。
/// ADR-033"纯类型 + 零逻辑"下的纯函数工具先例(与 test_scale / archive_monotonicity / mcsm 同级)。
pub mod util;

/// 统一错误层级契约 — NexusError 与 Recoverable（A0/WI-01 §6.6）
///
/// 跨层边界结构化错误枚举 + 恢复策略五档；应用层 anyhow 包装。
pub mod errors;

// ============================================================
// 公开 API 导出
// ============================================================

pub use capability_token::{CapabilityToken, CapabilityTokenStatus, SeamId};
// polish-v2.7 P1-3: 行为契约(BehaviorContract + ContractContext + ContractExample,ADR-049)
// Milestone B-3c: 强制层校验结果（ContractCheckOutcome）
pub use behavior_contract::{
    BehaviorContract, ContractCheckOutcome, ContractContext, ContractExample,
};
// P4-W16.2.2: R1 召回配额（RecallQuota + RecallQuotaPolicy，S7 接缝）
pub use density::{DensityPolicy, DensityTier};
pub use recall_quota::{RecallQuota, RecallQuotaPolicy};
// P4-W14.1: 记忆策略（MemoryStrategy + MemoryStrategyPolicy，S2 接缝）
// P4-W15.1.1: HarnessSpec 扩展（ImmutableSurface 不可进化面清单 + HarnessSpecError 校验错误）
pub use harness_spec::{
    ContractSpec, HarnessMeta, HarnessSpec, HarnessSpecError, HopSpec, ImmutableSurface,
    RetryPolicy, REQUIRED_ACCEPTANCE_GATES,
};
pub use ids::{FileId, MemoryId, OperationId, TaskId, ToolId};
pub use masks::SparseMask;
pub use omni_masks::OmniSparseMasks;
// P4-W14.3: Parliament 激活策略（ActivationStrategy + ParliamentPolicy，S5 接缝）
pub use parliament_policy::{ActivationStrategy, ParliamentPolicy};
// P4-W14.4: 衰减参数策略（DecayProfile + DecayPolicy，S6 接缝）
pub use decay_profile::{DecayPolicy, DecayProfile};
// P4-W14.2: 预取策略（PrefetchStrategy + PrefetchPolicy，S3 接缝）
pub use prefetch::{PrefetchPolicy, PrefetchStrategy};
pub use strategy::{MemoryStrategy, MemoryStrategyPolicy};
// Task 2: OSA memory 维度 S2 桥接契约（MemoryTaskPhase + MemoryStrategyProvider trait）
pub use memory_strategy::{MemoryStrategyProvider, MemoryTaskPhase};
// P3-W10.3: 选择器策略（SelectorPolicy + SelectorWeights，D1 修复）
pub use policy::{SelectorPolicy, SelectorWeights};
pub use quota::{NamespaceQuota, QuotaLimits};
pub use temporal::{TemporalMeta, TransitionType};
// polish-v2.7 P3-2: 变体契约(VariantId + VariantContract,ADR-051)
pub use variant::{VariantContract, VariantId};
// polish-v2.7 P4-7: 流程蓝图(ProceduralBlueprint + 计划校验,ADR-049)
pub use blueprint::{BlueprintSource, BlueprintStep, PlanViolation, ProceduralBlueprint};
// P2-W7.3: 向量存储契约（VectorStore trait + VectorHit + 扩展 trait）
pub use vector::{VectorBackend, VectorHit, VectorStore, VectorStoreExt, VectorStoreStats};
// T6-2: 形式化属性定义框架（FormalVerifier L4 骨架基础类型）
pub use formal_props::{
    FormalProperty, InvariantSpec, PropertyCategory, VerificationMethod, VerificationResult,
};
// P3-T8: 工具计划契约（WI-16 ToolPlan DSL）
pub use tool_plan::{guards, PlanEdge, PlanError, SideEffectDecl, ToolNode, ToolOp, ToolPlan};
// P3-T2 补: 调度契约（WI-29 mas-sched 类型先移 L0,ADR-033 先例）
// TaskId 已由 ids.rs 导出,此处不重复导出
pub use scheduler_contract::{
    ClaimOutcome, DenyReason, Lease, Priority, Quota, RenewOutcome, ShouldRunVerdict, TodoClaim,
    HANDOFF,
};
// Task 3.10: L0 共享类型扩展(EventMetadata / TaskStatus / Checkpoint)
// 从 L1 nexus-core / event-bus 下沉,缓解 L1 上帝 crate 病理(100+/65+/42+ 文件依赖)
pub use checkpoint::Checkpoint;
pub use event_metadata::EventMetadata;
pub use task::TaskStatus;
// ADR-054 决策 3: DECB 预算档位(BudgetTier 从 L8 decb-governor 上提,P9-T3)
pub use budget_tier::BudgetTier;
// ADR-065: MCA 模型亲和契约(能力协商唯一事实源 + 统一请求/响应)
// WHY 不导出全部子类型: PricingSpec/EndpointSpec/QuirkRule 等仅 spec 装配方
// (L10 spec_loader)使用,走 affinity:: 路径引用,避免顶层命名空间膨胀。
pub use affinity::{
    sampling_bucket, AffinityRequest, AffinityResponse, CapabilitySet, ContentBlock,
    ModelAffinitySpec, NegotiationFidelity, OutputBudget, OutputFormat, ProtocolDialect,
    ProviderId, SamplingParams, ThinkingPreference, TokenCacheKey, UsageReport,
};
// ADR-054 决策 3:命令验证契约(AttackType/Command/CommandPolicy/CommandValidator 上提,P9-T4)
pub use command_validation::{
    AttackType, BlockedPattern, Command, CommandPolicy, CommandValidationError, CommandValidator,
};
// ADR-054 决策 6:纯领域类型(ThinkingMode/MultimodalInput/UserIntent/Quest/Task 上提,P9-T7)
// WHY 顶层导出: 与 nexus-core re-export 形成对等路径,依赖方可直接 `use nexus_contracts::Quest`
pub use domain::{MultimodalInput, Quest, Task, ThinkingMode, UserIntent};
// ADR-054 决策 6:事件载荷契约(EventSeverity/TaskPriority/AgentStatus 下沉,P9-T7 Task 2)
// severity() 判定逻辑留在 L1 event-bus(架构红线:Critical 事件 mpsc 保障)
pub use event_payload::{AgentStatus, EventSeverity, TaskPriority};
// ADR-049 修订: RL 共享类型补齐（RLState/RLAction/RLExperience,v3.0.x）
// WHY 顶层导出: 与既有接缝契约类型（DensityTier/RecallQuota 等）对等路径,
// 依赖方可直接 `use nexus_contracts::rl_types::RLAction` 或顶层 `RLAction`
pub use rl_types::{MemPiAction, RLAction, RLExperience, RLState};
// P0-2 修复: INV-8 归档单调性独立公共 API（ArchiveTier + 判定函数 + 错误类型）
// WHY 顶层导出: 与既有契约函数/类型对等路径,依赖方可直接
// `use nexus_contracts::assert_archive_monotonicity` 或顶层 `ArchiveTier`
pub use archive_monotonicity::{assert_archive_monotonicity, ArchiveTier, InvariantViolation};
// 设计文档 §5.3.1: 六维控制面契约（MemoHarness D1-D6 融合）
// WHY 顶层导出: 与既有契约类型对等路径,依赖方可直接
// `use nexus_contracts::HarnessConfigContract` 或顶层 `ContextAssemblyContract`
pub use harness_dimensions::{
    CompressionStrategy, ContextAssemblyContract, EvictionStrategy, ExtractionFormat,
    FallbackStrategy, GenerationControlContract, HarnessConfigContract, MemoryManagementContract,
    OutputProcessingContract, RetentionPolicy, TaskOrchestrationContract, ToolInteractionContract,
    ValidationRule, WorkflowType,
};
// 设计文档 §5.3.2: OmniMessage 协议（PenguinHarness 模型-环境解耦）
// WHY 顶层导出: 与既有契约类型对等路径,依赖方可直接
// `use nexus_contracts::OmniMessage` 或顶层 `ModelConfig`
pub use omni_message::{ModelConfig, OmniMessage, TokenUsage};
// v3.4.0 §5.2: 经验卡片契约（OpenMLE）
// WHY 顶层导出: 与既有契约类型对等路径,依赖方可直接 `use nexus_contracts::ExperienceCard`
pub use experience_card::{
    AtomicOperator, CardMetadata, EnvironmentInfo, ErrorSignature, ExecutionStatus, ExperienceCard,
    NormalizedThreeFactor, ThreeFactorScore,
};
// v3.4.0 §5.3: Token 证据契约（Dressage）
pub use token_evidence::{
    SegmentCreationReason, SegmentMetadata, TokenLedgerEntry, ToolCallRecord,
};
// v3.4.0 §5.4: 记忆金字塔契约（MSCE + TencentDB）
pub use memory_pyramid::{
    AtomicCardType, AtomicMemoryCard, MemoryPyramidLevel, PersonaSummary, SceneBlock,
};
// v3.4.0 §5.5: Skill 生命周期契约（MSCE）
pub use skill_lifecycle::{SkillLifecycleContract, SkillLifecycleState};
// v3.4.0 §5.7: RL 预留钩子契约
pub use rl_hooks::{
    PolicyFormat, RLActionVector, RLHook, RLStateVector, RLTrajectory, SerializedPolicy,
};
// v3.4.0 §5.6: 六维控制面扩展（OperatorSelectionStrategy + StopStrategyConfig）
pub use harness_dimensions::{OperatorSelectionStrategy, StopStrategyConfig};
// v4.0 A0/WI-04/WI-05/WI-21: 图身份 / MCSM 投影器 / 外部协议 / 事件双轨 / 统一错误
// WHY 顶层导出: 与既有契约类型对等路径,依赖方可直接 `use nexus_contracts::GraphIdentity`
pub use app::{
    AppEvent, AppOp, AppTokenUsage, ApprovalDecision, ApprovalRequest, Item, ItemId, ItemStatus,
    PermissionMode, ReqId, Thread, ThreadId, ThreadStartParams, TurnId, UserInput,
};
pub use errors::{NexusError, Recoverable, RecoveryStrategy};
pub use event_v2::{
    Compressibility, DynamicEvent, EventMetadataV2, EventNamespace, EventPattern, EventTypeId,
    ImportanceScore, MetadataBridge, NamespaceQuotaV2,
};
pub use graph_identity::GraphIdentity;
pub use mcsm::{identity, project_weights, sinkhorn_project, ProjectedMatrix, SinkhornParams};

/// 预导出模块 — 常用类型的便捷导入
///
/// # 示例
/// ```
/// use nexus_contracts::prelude::*;
///
/// let id = ToolId::new("tool-1");
/// let mask: SparseMask<ToolId> = SparseMask::empty();
/// ```
pub mod prelude {
    // Milestone B-3c: 强制层校验结果（与顶层导出同集）
    pub use crate::behavior_contract::{ContractCheckOutcome, ContractContext, ContractExample};
    pub use crate::capability_token::{CapabilityToken, CapabilityTokenStatus, SeamId};
    // P4-W16.2.2: R1 召回配额（S7 接缝）
    pub use crate::density::{DensityPolicy, DensityTier};
    pub use crate::recall_quota::{RecallQuota, RecallQuotaPolicy};
    // P4-W15.1.1: HarnessSpec 扩展（ImmutableSurface + HarnessSpecError）
    pub use crate::harness_spec::{
        ContractSpec, HarnessMeta, HarnessSpec, HarnessSpecError, HopSpec, ImmutableSurface,
        RetryPolicy, REQUIRED_ACCEPTANCE_GATES,
    };
    pub use crate::ids::{FileId, MemoryId, OperationId, TaskId, ToolId};
    pub use crate::masks::SparseMask;
    pub use crate::omni_masks::OmniSparseMasks;
    // P3-W10.3: 选择器策略（D1 修复）
    pub use crate::policy::{SelectorPolicy, SelectorWeights};
    // P4-W14.3: Parliament 激活策略（S5 接缝）
    pub use crate::parliament_policy::{ActivationStrategy, ParliamentPolicy};
    // P4-W14.4: 衰减参数策略（S6 接缝）
    pub use crate::decay_profile::{DecayPolicy, DecayProfile};
    // P4-W14.2: 预取策略（S3 接缝）
    pub use crate::prefetch::{PrefetchPolicy, PrefetchStrategy};
    pub use crate::quota::{NamespaceQuota, QuotaLimits};
    // P4-W14.1: 记忆策略（S2 接缝）
    pub use crate::strategy::{MemoryStrategy, MemoryStrategyPolicy};
    // Task 2: OSA memory 维度 S2 桥接契约
    pub use crate::memory_strategy::{MemoryStrategyProvider, MemoryTaskPhase};
    pub use crate::temporal::{TemporalMeta, TransitionType};
    pub use crate::vector::{
        VectorBackend, VectorHit, VectorStore, VectorStoreExt, VectorStoreStats,
    };
    // T6-2: 形式化属性定义框架
    pub use crate::formal_props::{
        FormalProperty, InvariantSpec, PropertyCategory, VerificationMethod, VerificationResult,
    };
    // Task 3.10: L0 共享类型扩展(EventMetadata / TaskStatus / Checkpoint)
    pub use crate::checkpoint::Checkpoint;
    pub use crate::event_metadata::EventMetadata;
    pub use crate::task::TaskStatus;
    // ADR-054 决策 3: DECB 预算档位(BudgetTier 上提,P9-T3)
    pub use crate::budget_tier::BudgetTier;
    // ADR-065: MCA 模型亲和契约(与顶层导出同集)
    pub use crate::affinity::{
        sampling_bucket, AffinityRequest, AffinityResponse, CapabilitySet, ContentBlock,
        ModelAffinitySpec, NegotiationFidelity, OutputBudget, OutputFormat, ProtocolDialect,
        ProviderId, SamplingParams, ThinkingPreference, TokenCacheKey, UsageReport,
    };
    // ADR-054 决策 3:命令验证契约(AttackType/Command/CommandPolicy/CommandValidator 上提,P9-T4)
    pub use crate::command_validation::{
        AttackType, BlockedPattern, Command, CommandPolicy, CommandValidationError,
        CommandValidator,
    };
    // ADR-054 决策 6:纯领域类型(与顶层导出同集)
    pub use crate::domain::{MultimodalInput, Quest, Task, ThinkingMode, UserIntent};
    // ADR-054 决策 6:事件载荷契约(EventSeverity/TaskPriority/AgentStatus 下沉,P9-T7 Task 2)
    // severity() 判定逻辑留在 L1 event-bus(架构红线:Critical 事件 mpsc 保障)
    pub use crate::event_payload::{AgentStatus, EventSeverity, TaskPriority};
    // ADR-049 修订: RL 共享类型（与顶层导出同集）
    pub use crate::rl_types::{MemPiAction, RLAction, RLExperience, RLState};
    // P0-2 修复: INV-8 归档单调性契约（与顶层导出同集）
    pub use crate::archive_monotonicity::{
        assert_archive_monotonicity, ArchiveTier, InvariantViolation,
    };
    // 设计文档 §5.3.1: 六维控制面契约（与顶层导出同集）
    pub use crate::harness_dimensions::{
        CompressionStrategy, ContextAssemblyContract, EvictionStrategy, ExtractionFormat,
        FallbackStrategy, GenerationControlContract, HarnessConfigContract,
        MemoryManagementContract, OutputProcessingContract, RetentionPolicy,
        TaskOrchestrationContract, ToolInteractionContract, ValidationRule, WorkflowType,
    };
    // 设计文档 §5.3.2: OmniMessage 协议（与顶层导出同集）
    pub use crate::omni_message::{ModelConfig, OmniMessage, TokenUsage};
    // v3.4.0 §5.2: 经验卡片契约（与顶层导出同集）
    pub use crate::experience_card::{
        AtomicOperator, CardMetadata, EnvironmentInfo, ErrorSignature, ExecutionStatus,
        ExperienceCard, NormalizedThreeFactor, ThreeFactorScore,
    };
    // v3.4.0 §5.3: Token 证据契约（与顶层导出同集）
    pub use crate::token_evidence::{
        SegmentCreationReason, SegmentMetadata, TokenLedgerEntry, ToolCallRecord,
    };
    // v3.4.0 §5.4: 记忆金字塔契约（与顶层导出同集）
    pub use crate::memory_pyramid::{
        AtomicCardType, AtomicMemoryCard, MemoryPyramidLevel, PersonaSummary, SceneBlock,
    };
    // v3.4.0 §5.5: Skill 生命周期契约（与顶层导出同集）
    pub use crate::skill_lifecycle::{SkillLifecycleContract, SkillLifecycleState};
    // v3.4.0 §5.7: RL 预留钩子契约（与顶层导出同集）
    pub use crate::rl_hooks::{
        PolicyFormat, RLActionVector, RLHook, RLStateVector, RLTrajectory, SerializedPolicy,
    };
    // v3.4.0 §5.6: 六维控制面扩展（与顶层导出同集）
    pub use crate::harness_dimensions::{OperatorSelectionStrategy, StopStrategyConfig};
    // v4.0 A0/WI-04/WI-05/WI-21: 图身份 / MCSM / 外部协议 / 事件双轨 / 统一错误（与顶层导出同集）
    pub use crate::app::{
        AppEvent, AppOp, AppTokenUsage, ApprovalDecision, ApprovalRequest, Item, ItemId,
        ItemStatus, PermissionMode, ReqId, Thread, ThreadId, ThreadStartParams, TurnId, UserInput,
    };
    pub use crate::errors::{NexusError, Recoverable, RecoveryStrategy};
    pub use crate::event_v2::{
        Compressibility, DynamicEvent, EventMetadataV2, EventNamespace, EventPattern, EventTypeId,
        ImportanceScore, MetadataBridge, NamespaceQuotaV2,
    };
    pub use crate::graph_identity::GraphIdentity;
    pub use crate::mcsm::{
        identity, project_weights, sinkhorn_project, ProjectedMatrix, SinkhornParams,
    };
}
