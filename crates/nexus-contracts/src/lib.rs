//! CRATE-CONTRACT BEGIN
//! LAYER:    L0
//! ROLE:     NEXUS-OMEGA L0-A 契约层 facade — 纯类型 + 受控纯逻辑，对 workspace 的依赖仅限 6 个 L0-A sibling
//! BACKEND:  memory
//! PRODUCERS: 0 -
//! CONSUMERS: 34 chimera-cli,chimera-mas,chimera-tui,cmt-tiering,csn-substitutor…
//! MATURITY: TRUE
//! CRATE-CONTRACT END
//! NEXUS-OMEGA L0-A 契约层 facade — 纯类型 + 受控纯逻辑，对 workspace 的依赖仅限 6 个 L0-A sibling
//!
//! 对应架构层: **L0 Contracts / L0-A 纯契约亚层**（机器层号 0；nexus-core 之下，十层架构新增最低层）
//! 对应 ADR: **ADR-033**（L0 nexus-contracts 契约层建立；其追加节 2026-10-09 把口径由
//! 「零逻辑」量体订正为「受控纯逻辑」）/ **ADR-212**（边界 = 单 crate + 纯度门 G-99）/
//! **ADR-218**（「受控」的逻辑量判据 G-119 与例外台账 `scripts/l0_logic_ledger.txt`）/
//! **ADR-222**（部分 supersede ADR-212 D1/D2：本 crate 拆为 1+6，L0 分为 L0-A/L0-B 两个层名亚层，
//! 机器层号 0/1 不重排；本 crate 作 facade 重导出 6 个 sibling，旧路径逐字不变）
//! 对应设计源: `docs/architecture/CODE_WIKI.md`（架构权威源）+ `docs/architecture/adr_index.md`。
//! 旧引用 `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` 经全仓检索确认**不在盘**，与
//! `AETHER_NEXUS_OMEGA_ULTIMATE.md` 同属悬空权威源，故改指在盘源而非在此复制其内容。
//!
//! # 核心职责
//!
//! 承载跨层共享的**纯类型契约**，消除 L1 `nexus-core` 的上帝 crate 问题（被 7+ crate 依赖）
//! 与 L6 `osa-coordinator` 的星型耦合问题（3 router 均依赖 OSA 取 `OmniSparseMasks`）。
//!
//! # 设计约束（ADR-033）
//!
//! - **纯类型 + 受控纯逻辑**: 类型定义 + 契约自身语义的纯计算（校验/规范化/哈希/
//!   参考实现/状态机/熵启发式），无 IO / 无 async / 无网络 / 无随机；含逻辑模块
//!   逐条登记于 `ADR_033_EXCEPTION_MODULES`（口径见 ADR-033 追加节，**不得再整体引用「零逻辑」**）
//! - **workspace 内部依赖受控**（ADR-222 D2/D4 订正，原口径为「零 workspace 内部依赖」）:
//!   本 umbrella 仅允许依赖 6 个 L0-A sibling（`nexus-contracts-{core,sparse,safety,seams,mca,memory}`），
//!   且**方向单向**：sibling 不得反向依赖本 crate（否则即 crate 环，G-99 已按此判红）；
//!   两者都**禁止**依赖 `nexus-core` / `event-bus` 或任何层号 ≥ 2 的 crate（G-125 `[PURITY-LEAK]`/`[UPWARD]`）
//! - **外部依赖白名单**: `serde`(derive) + `chrono` + `uuid` + `thiserror`（均无运行时业务
//!   逻辑的基础类型/派生库，ADR-033 例外；G-99 `check_l0_purity.py` 机器锁）
//! - **依赖铁律扩展**: `L(N) → L(0)` 恒允许（任何层均可依赖 L0）
//!
//! # 承载类型（按功能分组；ADR-222 拆分后的 7-crate 实测结构，下述文件数为 2026-10-20 现取）
//!
//! L0-A `src/*.rs` 总扫描面以 G-99 实测为准（散文不登记数字；历史口径 71 vs 实测 72
//! 的漂移已按此纪律收口；2026-11 现取 74 = 本 umbrella 23 + 6 sibling 51）= 本 umbrella + 6 个 sibling：
//!
//! | crate | 保留组目录 | `src/*.rs` | 说明 |
//! |---|---|---|---|
//! | `nexus-contracts`（本 crate，facade） | `spec/` `vector_math/` `protocol/` `testutil/` | 23 | 4 组未拆 + facade_surface.rs（L0-A 值级面单源宏）+ faces.rs（L0-C 域聚合面）+ re-export |
//! | `nexus-contracts-core` | `core_domain/` | 10 | W3.1 首批，含 `is_schema_version_unset` 升 `pub` |
//! | `nexus-contracts-sparse` | `sparse/` | 5 | W3.2 |
//! | `nexus-contracts-safety` | `safety/` | 8 | W3.4 |
//! | `nexus-contracts-seams` | `seams/` | 14 | W3.5（MATURITY=NOT-IMPL-CN 台账行随代码迁至本 crate） |
//! | `nexus-contracts-mca` | `mca/` | 6 | W3.3（旧表 5 为笔误：lib.rs + 4 leaf + mod.rs；L0-B 修正） |
//! | `nexus-contracts-memory` | `memory/` | 8 | W3.6（依赖 core + mca + seams） |
//!
//! 每个 sibling **保留原组目录名**（如 `nexus-contracts-core/src/core_domain/`），因此
//! 消费方两条旧路径 `nexus_contracts::<leaf>::Type` 与 `nexus_contracts::<group>::<leaf>::Type`
//! 逐字不变；旧口径「64 源文件 = 53 叶子模块 + 10 组 mod.rs + lib.rs」为拆分前单 crate 时点值，
//! 现由上表取代（根 re-export 计数以 G-107 台账实测为准，散文不登记数字）。（v3.5 W3 子模块化）
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
//! | `FormalProperty` / `InvariantSpec` / `VerificationResult` | 新建（T6-2 FormalVerifier 骨架） | 各 owner crate formal 验证器(7 个,聚合门 gsoe-evolution formal_gate) / L8 parliament |
//! | `ArchiveTier` / `assert_archive_monotonicity` | 新建（P0-2 INV-8 下沉） | L2 mlc-engine / L3 cmt-tiering |
//!
//! ## MCA 亲和与向量存储
//! | 类型 | 来源 | 消费层 |
//! |------|------|--------|
//! | `ProviderId` / `ProtocolDialect` / `CapabilitySet` / `ModelAffinitySpec` | 新建（ADR-065 MCA 体系） | L10 mca-gateway / L6 omega-learner |
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
//! | `RLHook` / `SerializedPolicy` / `RLTrajectory` / `RLStateVector` / `RLActionVector` | 新建（v3.4.0 §5.7 RL 预留） | L1 nexus-core(rl_client) / L10 chimera-cli(rl_client) |
//! | `OperatorSelectionStrategy` / `StopStrategyConfig` + 六维 OpenMLE 扩展字段 | 扩展（v3.4.0 §5.6） | L6 Router / L9 Quest |
//!
//! ## 测试工具与纯函数（ADR-033 例外）
//! | 类型 | 来源 | 消费层 |
//! |------|------|--------|
//! | `scale_timeout` / `scaled_timeout!` | 新建（P9-T2 测试缩放，ADR-033 例外 1） | 全 workspace 测试 |
//! | `assert_archive_monotonicity` | 新建（P0-2 INV-8 下沉，ADR-033 例外 2） | L2 mlc-engine / L3 cmt-tiering |
//! | `CapabilityToken` EWMA 方法 | 新建（P4-W14.5 C4 合规，ADR-033 例外 3） | L4 decay-engine / L9 编排器 |
//! | `ExperienceCard` 三因子/状态纯函数 | 新建（v3.4.0 §5.2，ADR-033 例外 4） | L1 event-bus / L5 three-factor-selector |
//! | `HarnessSpec::validate` / `canonical_merkle_input` | 新建（P4-W13~16 DSL 校验，ADR-033 例外 5；2026-10-09 v3.5 量体后补登） | L5 gsoe-evolution / L9 quest-engine |
//! | `PlatformGroundingSpec::from_doc/check` | 新建（Milestone B-4，纯函数先例） | L9 efficiency-monitor |
//! | `BehaviorContract::enforce` / `ProceduralBlueprint::validate_plan` | 新建（polish-v2.7，纯函数先例） | L9 efficiency-monitor / L9 quest-engine |
//!
//! > 注：后两类为 ADR-033"纯类型零逻辑"约束的**纯函数先例**（无 IO 无状态变更），
//! > 与 `archive_monotonicity` 同类；显式声明以消除审计误判（2026-08-16 修复 A2）。
//! >
//! > ⚠ 性质口径订正（2026-10-09 v3.5 W6-3）：本 crate 的**总括**性质不是「零逻辑」而是
//! > 「**受控纯逻辑**」（无 IO / 无 async / 无网络 / 无随机，仅这五项为红线）。上方例外表
//! > 就是证据：`capability_token` 27 个非测试有体函数（EWMA 灰度状态机）、`harness_spec`
//! > 13 个（`validate` 143 行 + `canonical_merkle_input` 95 行）。而 `vector.rs` 的「零逻辑」
//! > **依旧成立**（7 个微助手均 ≤7 行 + 8 个 trait 无体声明）——不得拿一个模块代表全 crate。
//! > 完整量体数据与口径推导见 `ADR-033` 追加节；本结论无机器门，仍靠人工审计 + 单测（已如实登记）。
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
// 模块声明 —— v3.5 ADR-212 W3：53 平铺模块收敛为 10 内聚子模块组。
// crate 根对每个叶子模块 re-export（`pub use <group>::<leaf>;`），使
// `crate::<leaf>`（facade / prelude / 内部交叉引用）与
// `nexus_contracts::<leaf>::Type`（外部消费方）旧路径逐字不变，并新增
// `nexus_contracts::<group>::<leaf>` 分组路径。叶子文件头 `//!` 即各模块
// 文档（满足 missing_docs）；下方 `pub use` re-export 上方的 `///` 原文注释
// 转为对该 re-export 的文档说明（内容不变，仅声明点从 `pub mod` 改为 `pub use`）。
// ============================================================

// L0-A sibling（ADR-222 D2）：core_domain 领域契约。以 re-export 保持 `crate::core_domain::<leaf>`
// 与 `nexus_contracts::core_domain::<leaf>::Type` 两条路径逐字不变（34 消费 crate 零改动）。
pub use nexus_contracts_core::core_domain;
// L0-A sibling（ADR-222 D2）：core re-export 保持 `crate::mca::<leaf>`
// 与两条外部路径逐字不变（34 个消费 crate 零改动）。
pub use nexus_contracts_mca::mca;
// L0-A sibling（ADR-222 D2）：core re-export 保持 `crate::memory::<leaf>`
// 与两条外部路径逐字不变（34 个消费 crate 零改动）。
pub use nexus_contracts_memory::memory;
pub mod protocol;
// L0-A sibling（ADR-222 D2）：core re-export 保持 `crate::safety::<leaf>`
// 与两条外部路径逐字不变（34 个消费 crate 零改动）。
pub use nexus_contracts_safety::safety;
// L0-A sibling（ADR-222 D2）：core re-export 保持 `crate::seams::<leaf>`
// 与两条外部路径逐字不变（34 个消费 crate 零改动）。
pub use nexus_contracts_seams::seams;
// L0-A sibling（ADR-222 D2）：core re-export 保持 `crate::sparse::<leaf>`
// 与两条外部路径逐字不变（34 个消费 crate 零改动）。
pub use nexus_contracts_sparse::sparse;
pub mod spec;
pub mod testutil;
pub mod vector_math;

// L0-C（ADR-新-1 同批）：域导向聚合面 —— additive 便捷再导出，
// 不改变既有 `<leaf>` / `<group>::<leaf>` 路径（详见 faces.rs 文件头）。
pub mod faces;

// L0-A（ADR-新-1）：值级 facade 单源声明宏（crate-private，仅下方调用一次）。
// WHY 单独成文件而非 lib.rs 内嵌：宏体含 prelude 模块文档与示例 doctest，
// 独立文件让 lib.rs 聚焦声明数据本身。
mod facade_surface;

/// ID newtype 类型 — 五维度稀疏化的统一标识
pub use sparse::ids;

/// 稀疏掩码容器 — 泛型 `SparseMask<T>`
pub use sparse::masks;

/// 全维稀疏掩码 — 五维度掩码聚合体 `OmniSparseMasks`
pub use sparse::omni_masks;

/// Harness-as-Spec 契约类型 — P4 学习闭环规格
pub use spec::harness_spec;

/// 时间元数据 — P3 时间感知记忆扩展
pub use core_domain::temporal;

/// 命名空间配额 — chimera-mas 资源配额上提
pub use memory::quota;

/// 向量存储契约 — 跨层向量检索抽象（VectorStore trait + VectorStoreExt 扩展）
pub use vector_math::vector;

/// 选择器策略契约 — D1 病理修复（selector 权重外置，P3-W10.3）
pub use seams::policy;

/// 密度档位策略契约 — S1 接缝（DDR/HCW 密度档位，P4-W13.2）
pub use seams::density;

/// 记忆策略契约 — S2 接缝（mlc-engine 记忆策略选择，P4-W14.1）
pub use seams::strategy;

/// OSA memory 维度自适应记忆策略契约 — S2 桥接层（Task 2）
pub use seams::memory_strategy;

/// 预取策略契约 — S3 接缝（scc-cache 预取策略选择，P4-W14.2）
pub use seams::prefetch;

/// Parliament 激活策略契约 — S5 接缝（Parliament 激活策略选择，P4-W14.3）
pub use seams::parliament_policy;

/// 衰减参数策略契约 — S6 接缝（decay-engine 衰减档位选择，P4-W14.4）
pub use seams::decay_profile;

/// 能力场令牌契约 — 学习策略灰度授权载体（P4-W14.5 C4 合规）
pub use seams::capability_token;

/// 召回配额策略契约 — S7 接缝（R1 离线 RL，P4-W16.2.2）
pub use seams::recall_quota;

/// 行为契约 — 类型使用规范契约(polish-v2.7 P1-3,ADR-049)
pub use spec::behavior_contract;

/// 变体契约 — Harness 变体标识与性能契约(polish-v2.7 P3-2,ADR-051)
pub use spec::variant;

/// 流程蓝图契约 — 隐性流程经验载体(polish-v2.7 P4-7,ADR-049)
pub use spec::blueprint;

/// 形式化属性定义框架 — FormalVerifier L4 骨架基础类型(T6-2)
pub use spec::formal_props;

/// 工具计划契约 — 声明式 ToolPlan DSL + 校验 + 守卫常量（P3-T8,WI-16）
pub use spec::tool_plan;

/// 工具派发契约 — 执行面最小类型集（T1-1，ADR-033 上提）
pub use spec::tool_dispatch;

/// 批派发准入契约 — BatchPlan/BatchReport 四段守恒（P-A，ADR-新-3）
pub use spec::tool_admission;

/// 工具名解析契约 — 双 MCP 命名格式单一解析入口（W1 / L2 工具层重构方案 §6.1）；
/// 与 tool_plan/tool_dispatch/tool_admission 同为工具族根面叶子，
/// root_reexport_freeze 同批显式登记（P-A 先例）。
pub use spec::tool_name;

/// 副作用真值源统一契约 — 判定纯函数 + O(1) 声明索引（W3 / L2 方案 §6.4）；
/// 工具族根面叶子，root_reexport_freeze 同批显式登记。
pub use spec::tool_effects;

/// 调度契约 — mas-sched 控制面类型先移 L0（P3-T2 补,ADR-033 先例）
pub use spec::scheduler_contract;

/// 事件元数据契约 — L0 共享的事件追踪元信息(Task 3.10,ADR-033 扩展)
pub use core_domain::event_metadata;

/// 任务状态契约 — L0 共享的 Task 生命周期枚举(Task 3.10,ADR-033 扩展)
pub use core_domain::task;

/// 检查点契约 — L0 共享的 Quest 断点恢复快照(Task 3.10,ADR-033 扩展)
pub use core_domain::checkpoint;

/// 预算档位契约 — DECB 三档枚举上提(ADR-054 决策 3,P9-T3)
pub use safety::budget_tier;

/// 模型亲和契约 — MCA 体系 L0 类型(ADR-065,PANTHEON 计划)
pub use mca::affinity;

/// 模型上下文能力档案 — HCW v3 P0（ADR-291）的 L0 契约面
pub use mca::context_capability;

/// 命令验证契约 — 攻击类型/命令/策略/trait 上提(ADR-054 决策 3,P9-T4)
pub use safety::command_validation;

/// 纯领域类型契约 — 用户意图/长期任务/思考模式上提(ADR-054 决策 6,P9-T7)
pub use core_domain::domain;

/// 事件载荷契约 — 纯载荷枚举下沉(ADR-054 决策 6,P9-T7 Task 2)
pub use core_domain::event_payload;

/// P9-T2 测试等待缩放工具 — CHIMERA_TEST_TIMEOUT_SCALE 协议
pub use testutil::test_scale;

/// RL 共享类型契约 — RLState/RLAction/RLExperience（ADR-049 修订补齐）
pub use seams::rl_types;

/// 平台接地契约 — PlatformGroundingSpec（Milestone B-4，北大 NL2Pipeline gap 解）
pub use spec::platform_grounding;

/// 奖励函数统一框架契约（Milestone C-1）— RewardSpec/RewardSignal/SecuritySeverity
pub use seams::reward;

/// 归档单调性契约 — INV-8 独立公共 API（P0-2 审计修复）
pub use memory::archive_monotonicity;

/// 六维控制面契约 — MemoHarness D1-D6 融合（设计文档 §5.3.1）
pub use spec::harness_dimensions;

/// OmniMessage 协议 — 模型-环境解耦统一消息协议（设计文档 §5.3.2）
pub use mca::omni_message;

/// 经验卡片契约 — OpenMLE 核心数据结构（v3.4.0 §5.2）
pub use memory::experience_card;

/// Token 级证据契约 — Dressage 融合（v3.4.0 §5.3）
pub use memory::token_evidence;

/// 记忆金字塔契约 — MSCE + TencentDB 融合（v3.4.0 §5.4）
pub use memory::memory_pyramid;

/// Skill 生命周期契约 — MSCE 融合（v3.4.0 §5.5）
pub use memory::skill_lifecycle;

/// RL 预留钩子契约 — v4.0 升级路径（v3.4.0 §5.7 + §17）
pub use seams::rl_hooks;

// ============================================================
// v4.0 统一执行总案 A0/WI-04/WI-05/WI-21 新增模块（2026-08-22）
// ============================================================

/// 图身份契约 — GIP 跨层成本归因三元组（WI-04）
pub use core_domain::graph_identity;

/// MCSM 流形约束信号守恒聚合 — Sinkhorn 双随机投影器（WI-05）
pub use vector_math::mcsm;

/// 外部协议契约 — AppOp/AppEvent 与 Thread/Turn/Item 三原语（WI-01）
pub use protocol::app;

/// 事件双轨契约 — DynamicEvent 注册表与 EventMetadataV2（WI-21）
pub use core_domain::event_v2;

/// 公共 Top-K 收敛工具 — 红线 #8(WS-2 C1)
pub use vector_math::util;

/// 统一错误层级契约 — NexusError 与 Recoverable（A0/WI-01 §6.6）
pub use safety::errors;

/// 裁决权与认识类型契约 — Claim/Certify/Declaim 语义分离 + MLC 二维认识标签
pub use safety::adjudication;

/// 写入敏感信息扫描引擎 — 记忆/上下文/会话持久化前的敏感载荷拦截（v2.3 §(b.5)）
pub use safety::sensitive;

/// 统一价格向量契约 — 聚合分散模型单价为可路由消费向量（v2.3 §(b.6)/(c)）
pub use mca::price_vector;

/// 人机一致率校准契约 — 评测层「一致率锤点」（v2.3 §(b.6)）
pub use safety::calibration;

/// 多路召回 RRF 融合原语 — 二期四路召回入口的 L0 共享算法（v2.3 §(b.3)/§(d)⑨）
pub use vector_math::recall_fusion;

// ============================================================
// ADR-033「受控纯逻辑」例外登记表（机器可读单一真值源）
// ============================================================

/// ADR-033「纯类型 + 受控纯逻辑」例外登记表（唯一机器可读源）。
///
/// 列出的模块在「纯类型」之外含有**实质性受控纯逻辑**（校验 / 拓扑判环 /
/// 状态机 / 恢复策略决策表 / 安全策略 validate / 规范化 / 哈希 / 参考实现 /
/// 熵启发式），红线仍只有四项：无 IO / 无 async / 无网络 / 无随机。
///
/// 与两份邻近记录的关系（刻意不复制事实，避免第二真值源）：
/// - 本表是**模块粒度**的登记，唯一权威；项数不在此写死，由
///   `tests/l0_exception_registry.rs` 的双向锁与 G-119 的 `registered=` 自报复核。
/// - 本文件顶部「测试工具与纯函数（ADR-033 例外）」表是例外 1-5 的**来源登记**
///   （provenance：谁、因何、被哪层消费），**不是**全量清单；新增模块不回填该表。
/// - `scripts/l0_logic_ledger.txt`（G-119）是**函数粒度**的实况台账，键集取
///   派生逻辑 ∪ 政策表 ∪ 本表，故两处视角互补而非互相抄录。
/// - **ADR-222 拆分后的归属**：本表仍是 L0-A 唯一的**模块粒度**登记权威，但登记名对应的源文件
///   已物理落在 6 个 sibling（如 `masks` → `nexus-contracts-sparse/src/sparse/masks.rs`）；
///   故表内继续用**裸模块名**而不改成 crate 限定名——台账键不变是 D4 兼容承诺的一部分，
///   可达性由 `tests/l0_exception_registry.rs` 跨 7 个 src 根解析（空集不得读作通过）。
///
/// 小型访问器 / 枚举 `as_str` 等平凡逻辑不登记（如 `vector.rs` 的 ≤7 行微助手 +
/// 无体 trait 声明，其量体结论为「零逻辑仍成立」）。决策原语
/// （`affinity::sampling_bucket` / `price_vector::cheapest_model` 等）曾挂
/// 「D-待裁决」，已由 `ADR-218` 决策 1 逐条裁为受控纯逻辑并于下方补齐。
///
/// `tests/l0_exception_registry.rs` 交叉校验本表：① 每个登记名必须是 crate 根可达
/// 声明模块；② 登记表与实质性逻辑集**双向相等**；③ W3 十个组声明均存在（结构锁）；
/// ④ **已登记模块的头部不得再以项目符号句式自称「纯类型零逻辑」**（名实矛盾）。
pub const ADR_033_EXCEPTION_MODULES: &[&str] = &[
    "test_scale",
    "archive_monotonicity",
    "capability_token",
    "experience_card",
    "harness_spec",
    "platform_grounding",
    "behavior_contract",
    "blueprint",
    "util",
    "mcsm",
    "sensitive",
    "calibration",
    "recall_fusion",
    // v3.5 深度复审补齐（B-P1-6：修正登记表与实况不符）：
    "skill_lifecycle", // 三态状态机 next_state + 阈值（模块自声明例外，之前未登记）
    "tool_plan",       // validate + Kahn 拓扑判环（O(V+E) 图算法）
    "errors",          // Recoverable::recovery_strategy 恢复决策表 + to_nexus_error
    "command_validation", // CommandValidator::validate 遍历 20 条拦截模式（ADR-054 决策 3 授权的安全策略）
    // ADR-218 决策 1/2 补齐（承 ADR-033 追加节自陈缺口 → G-99 符号门 → G-119 逻辑量门）：
    // 以下 11 个模块由 G-119 从源码派生现形（旧登记表与实况脱节 11 项），逐条裁决记于
    // ADR-218 决策 1；逐函数读数见 scripts/l0_logic_ledger.txt，勿在此重复台账内容。
    "affinity", // preferred_dialect 遍历选型 + sampling_bucket 分桶启发式（确定性，无随机源）
    "price_vector", // cheapest_model min-scan + estimate_cost_micro 微单元成本算术
    "quota",    // NamespaceQuota::validate 六段区间校验 + 生效上限折减
    "token_evidence", // TokenLedgerEntry::new 三序列一一对应的证据完整性不变量
    "density",  // from_rho 为 min_by 最近档搜索（名为查表、实非查表）+ actual_load 容量折减
    "policy",   // is_valid 权重与策略形状校验
    "reward",   // LAYER_WEIGHTS 八维奖励权重政策表（Milestone C-1 RewardSpec）
    "masks",    // SparseMask::select_top_k（select_nth_unstable O(n)，ADR-213 D-7 本域裁决）
    "formal_props", // collect_results 结果归并 + from_result_with_auto_confidence 置信度分档
    "harness_dimensions", // default_contract 六维默认契约装配
    "variant",  // VariantId::new 标识归一化构造
    // R12 W0 订正（ADR-296）：本行由 R9/ADR-291 加入，但当时漏了两件同批动作——
    // ① crate 根没有对应的 `pub use mca::context_capability;` 声明；
    // ② 测试侧 `SUBSTANTIVE_LOGIC_MODULES` 未同步。
    // 于是 `l0_exception_registry` 的两条不变量长期红（多登记 + 无根声明），而 G-119
    // 又要求"有派生逻辑必须登记"——两门互斥。订正方式 = 补根声明 + 补测试期望集，
    // 而不是摘掉本行（摘掉会让 G-119 报 REGISTRY-GAP，把真模块藏起来）。
    "context_capability", // external_prior_profile 七模型先验表查选 + CalibrationTier::as_str 映射 + ThreadSupport::is_supported 判定（确定性，无 IO/随机）
    // P-A（ADR-新-3）：批派发准入契约纯判定函数族（is_conserved 四段守恒计数 /
    // segment_counts / is_read_safe）达 G-119 派生逻辑检阈；与
    // tests/l0_exception_registry.rs 的 SUBSTANTIVE_LOGIC_MODULES 同批加入。
    "tool_admission",
    // W1（L2 工具层重构方案 §6.1）：工具名解析纯函数族（parse_tool_name 双格式
    // 结构分类 / is_legal_server_id 白名单 / canonical_name 归一）达 G-119 派生
    // 逻辑检阈；与 tests/l0_exception_registry.rs 的 SUBSTANTIVE_LOGIC_MODULES
    // 同批加入（l0_exception_registry 双向相等锁 + l0_logic_ledger FN 行同批）。
    "tool_name",
    // W3（L2 方案 §6.4）：副作用真值源统一纯函数族（resolve_effect 三级判定 /
    // resolve_effect_with_descriptor 四级 / SideEffectIndex O(1) 查表索引）
    // 达 G-119 派生逻辑检阈；与 tests/l0_exception_registry.rs 同批加入。
    "tool_effects",
];

// ============================================================
// 公开 API 导出 —— L0-A（ADR-新-1）：值级面单源声明
// ============================================================
//
// 顶层 `pub use <leaf>::{…}` 与 `pub mod prelude` 由下方 `l0_surface!` 宏
// 从**同一份符号声明**生成（各条目尾标记 `prelude` = 进 glob 面 / `top` =
// 只进顶层）。重构前二者是手工镜像清单，漂移实证见
// tests/prelude_surface_test.rs 文件头；"无孤儿 / 同族完备"两条不变量
// 现由构造保证，测试保留 planted-defect 自检与基线等价锁
// （tests/fixtures/l0_surface_baseline.json）。
//
// 声明顺序与重构前逐字一致（rustdoc 渲染序稳定）；新增契约类型的接线点
// 从 3-4 处收敛为本声明一处 + 上方模块级 `pub use <group>::<leaf>;` 一处。

crate::facade_surface::l0_surface! {
    capability_token prelude { CapabilityToken, CapabilityTokenStatus, SeamId }
    // polish-v2.7 P1-3: 行为契约(BehaviorContract + ContractContext + ContractExample,ADR-049)
    // Milestone B-3c: 强制层校验结果（ContractCheckOutcome）
    behavior_contract prelude { BehaviorContract, ContractCheckOutcome, ContractContext, ContractExample }
    // P4-W16.2.2: R1 召回配额（RecallQuota + RecallQuotaPolicy，S7 接缝）
    density prelude { DensityPolicy, DensityTier }
    recall_quota prelude { RecallQuota, RecallQuotaPolicy }
    // P4-W15.1.1: HarnessSpec 扩展（ImmutableSurface 不可进化面清单 + HarnessSpecError 校验错误）
    harness_spec prelude { ContractSpec, HarnessMeta, HarnessSpec, HarnessSpecError, HopSpec, ImmutableSurface, RetryPolicy, REQUIRED_ACCEPTANCE_GATES }
    ids prelude { FileId, MemoryId, OperationId, TaskId, ToolId }
    masks prelude { SparseMask }
    omni_masks prelude { OmniSparseMasks }
    // P4-W14.3: Parliament 激活策略（ActivationStrategy + ParliamentPolicy，S5 接缝）
    parliament_policy prelude { ActivationStrategy, ParliamentPolicy }
    // P4-W14.4: 衰减参数策略（DecayProfile + DecayPolicy，S6 接缝）
    decay_profile prelude { DecayPolicy, DecayProfile }
    // P4-W14.2: 预取策略（PrefetchStrategy + PrefetchPolicy，S3 接缝）
    prefetch prelude { PrefetchPolicy, PrefetchStrategy }
    strategy prelude { MemoryStrategy, MemoryStrategyPolicy }
    // Task 2: OSA memory 维度 S2 桥接契约（MemoryTaskPhase + MemoryStrategyProvider trait）
    memory_strategy prelude { MemoryStrategyProvider, MemoryTaskPhase }
    // P3-W10.3: 选择器策略（SelectorPolicy + SelectorWeights，D1 修复）
    policy prelude { SelectorPolicy, SelectorWeights }
    quota prelude { NamespaceQuota, QuotaError, QuotaLimits }
    temporal prelude { TemporalMeta, TransitionType }
    // polish-v2.7 P3-2: 变体契约(VariantId + VariantContract,ADR-051)
    // 有意取舍：泛用名进 glob 扩大冲突面 → 只进顶层（ADR-217 D-35）
    variant top { VariantContract, VariantId }
    // polish-v2.7 P4-7: 流程蓝图(ProceduralBlueprint + 计划校验,ADR-049) — 同上有意取舍
    blueprint top { BlueprintSource, BlueprintStep, PlanViolation, ProceduralBlueprint }
    // P2-W7.3: 向量存储契约（VectorStore trait + VectorHit + 扩展 trait）
    vector prelude { VectorBackend, VectorHit, VectorStore, VectorStoreExt, VectorStoreStats }
    // T6-2: 形式化属性定义框架（FormalVerifier L4 骨架基础类型）
    formal_props prelude { EmptyFormalProvider, FormalProperty, FormalResultProvider, InvariantSpec, PropertyCategory, VerificationMethod, VerificationResult, VerifiedWithStrength }
    // P3-T8: 工具计划契约（WI-16 ToolPlan DSL）— 有意取舍：ToolOp/PlanEdge 等泛用名只进顶层
    tool_plan top { guards, PlanEdge, PlanError, SideEffectDecl, ToolNode, ToolOp, ToolPlan }
    // P3-T2 补: 调度契约（WI-29 mas-sched 类型先移 L0）— Quota/Priority 泛用名只进顶层
    scheduler_contract top { ClaimOutcome, DenyReason, Lease, Priority, Quota, RenewOutcome, ShouldRunVerdict, TodoClaim, HANDOFF }
    // Task 3.10: L0 共享类型扩展(EventMetadata / TaskStatus / Checkpoint)
    checkpoint prelude { Checkpoint }
    event_metadata prelude { EventMetadata }
    task prelude { TaskStatus }
    // ADR-054 决策 3: DECB 预算档位(BudgetTier 从 L8 decb-governor 上提,P9-T3)
    budget_tier prelude { BudgetTier }
    // ADR-065: MCA 模型亲和契约(能力协商唯一事实源 + 统一请求/响应)
    // WHY 不导出全部子类型: PricingSpec/EndpointSpec/QuirkRule 等仅 spec 装配方使用,
    // 走 affinity:: 路径引用,避免顶层命名空间膨胀。
    affinity prelude { sampling_bucket, AffinityRequest, AffinityResponse, CapabilitySet, ContentBlock, ModelAffinitySpec, NegotiationFidelity, OutputBudget, OutputFormat, ProtocolDialect, ProviderId, SamplingParams, ThinkingPreference, TokenCacheKey, UsageReport }
    // ADR-054 决策 3:命令验证契约(AttackType/Command/CommandPolicy/CommandValidator 上提,P9-T4)
    command_validation prelude { AttackType, BlockedPattern, Command, CommandPolicy, CommandValidationError, CommandValidator }
    // ADR-054 决策 6:纯领域类型(ThinkingMode/MultimodalInput/UserIntent/Quest/Task 上提,P9-T7)
    domain prelude { MultimodalInput, Quest, Task, ThinkingMode, UserIntent }
    // ADR-054 决策 6:事件载荷契约(EventSeverity/TaskPriority/AgentStatus 下沉,P9-T7 Task 2)
    event_payload prelude { AgentStatus, EventSeverity, TaskPriority }
    // ADR-049 修订: RL 共享类型补齐（RLState/RLAction/RLExperience）
    rl_types prelude { MemPiAction, RLAction, RLExperience, RLState }
    // P0-2 修复: INV-8 归档单调性独立公共 API（ArchiveTier + 判定函数 + 错误类型）
    archive_monotonicity prelude { assert_archive_monotonicity, ArchiveTier, InvariantViolation }
    // 设计文档 §5.3.1: 六维控制面契约（MemoHarness D1-D6 融合）+ §5.6 扩展
    harness_dimensions prelude { CompressionStrategy, ContextAssemblyContract, EvictionStrategy, ExtractionFormat, FallbackStrategy, GenerationControlContract, HarnessConfigContract, MemoryManagementContract, OperatorSelectionStrategy, OutputProcessingContract, RetentionPolicy, StopStrategyConfig, TaskOrchestrationContract, ToolInteractionContract, ValidationRule, WorkflowType }
    // 设计文档 §5.3.2: OmniMessage 协议（PenguinHarness 模型-环境解耦）
    omni_message prelude { ModelConfig, OmniMessage, TokenUsage }
    // v3.4.0 §5.2: 经验卡片契约（OpenMLE）
    experience_card prelude { AtomicOperator, CardMetadata, EnvironmentInfo, ErrorSignature, ExecutionStatus, ExperienceCard, NormalizedThreeFactor, ThreeFactorScore }
    // v3.4.0 §5.3: Token 证据契约（Dressage）
    token_evidence prelude { SegmentCreationReason, SegmentMetadata, TokenLedgerEntry, ToolCallRecord }
    // v3.4.0 §5.4: 记忆金字塔契约（MSCE + TencentDB）
    memory_pyramid prelude { AtomicCardType, AtomicMemoryCard, MemoryPyramidLevel, PersonaSummary, SceneBlock }
    // v3.4.0 §5.5: Skill 生命周期契约（MSCE）
    skill_lifecycle prelude { SkillLifecycleContract, SkillLifecycleState }
    // v3.4.0 §5.7: RL 预留钩子契约
    rl_hooks prelude { PolicyFormat, RLActionVector, RLHook, RLStateVector, RLTrajectory, SerializedPolicy }
    // v4.0 A0/WI-04/WI-05/WI-21: 外部协议面（AppOp/AppEvent 与 Thread/Turn/Item 三原语）
    app prelude { AppEvent, AppOp, AppTokenUsage, ApprovalDecision, ApprovalRequest, Item, ItemId, ItemStatus, PermissionMode, ReqId, Thread, ThreadId, ThreadStartParams, TurnId, UserInput }
    // A0/WI-01 §6.6: 统一错误层级契约（NexusError 与 Recoverable）
    errors prelude { ErrorLayer, EventFailure, NexusError, Recoverable, RecoveryStrategy, ToolFailure }
    // v2.3 (b.1)/(b.3): 裁决权与认识类型契约（Claim/Certify/Declaim payload + EpistemicType）
    adjudication prelude { CertifyPayload, CertifyVerdict, ClaimPayload, DeclaimPayload, EpistemicType, FailureFixStatus, FailureRecord, VerifierKind }
    // v2.3 (b.5): 写入敏感信息扫描引擎（纯函数，零依赖，L0）
    sensitive prelude { redact, scan, ScanHit, ScanPolicy, ScanVerdict, SensitiveDisposition, SensitiveKind }
    // v4.0 WI-21: 事件双轨契约（DynamicEvent 注册表与 EventMetadataV2）
    event_v2 prelude { Compressibility, DynamicEvent, EventMetadataV2, EventNamespace, EventPattern, EventTypeId, ImportanceScore, MetadataBridge, NamespaceQuotaV2 }
    graph_identity prelude { GraphIdentity }
    mcsm prelude { identity, project_weights, sinkhorn_project, ProjectedMatrix, SinkhornParams }
    price_vector prelude { ModelPrice, PriceVector }
    // v2.3 (b.6): 人机一致率校准契约（纯函数，零依赖，L0）
    calibration prelude { agreement_rate_milli, certifiable, CalibrationSample, GateVerdict, HumanLabel, DEFAULT_AGREEMENT_THRESHOLD_MILLI }
}
