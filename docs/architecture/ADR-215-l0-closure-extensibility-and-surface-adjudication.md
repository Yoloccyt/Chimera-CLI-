# ADR-215: L0 契约层收口 —— 档 2 逐类型可扩展性开档、零消费符号逐个裁决、文档口径同步、bench unsafe 豁免与三项治理候选评估

- **Status**: ✅ Accepted (2026-10-15)
- **关联 ADR**: ADR-033（L0 建立 + 2026-10-09 追加节"受控纯逻辑"口径）/ ADR-212（边界总裁决，决策 1-5 全部继承）/ ADR-213（R4/R5 pub 面棘轮、R8 Class A/B wire 划界、F4′）/ ADR-214（D-22 档 2 的执行收口、装配消费者四口径）
- **权威源**: 代码实测（2026-10-15，分支 `feat/c1-compute-plane-governance`）+ `scripts/derive_l0_extensibility_evidence.py` 派生证据（`tmp/l0_ext_evidence.tsv`）。两份设计文档（v3.4.0 / 十六源手册）仅作意图参照，其 L0 章节已被 ADR-212 决策 1 归档。

## 1. 背景

ADR-214 档 2 未开工：L0 的 227 个 pub 类型全部登记 `class=closed`（G-115 台账），意味着任何变体/字段新增都是源码级破坏（36 个声明面消费 crate 受影响）。同轮盘点还遗留：约 13 个初判零消费 pub 符号未逐个裁决（初判含 doc 注释与 re-export 路径误差）、两处文档口径漂移、bench 面 `unsafe impl GlobalAlloc` 无 ADR 明示、三项治理候选待评估。

本批事实基线（全部机采，B0 脚本自检 6 断言双向咬合后真树运行）：

- 短名单实测 **23 enum（零外部提及）+ 64 struct（无外部字面量）**；ADR-214 记 21/64——enum 侧 +2 属扫描面修正（B0 含根 tests/ 与 L0 自身 benches，与 ADR-214 的"否定式主张射程必须含根 tests/"教训同源）。8 个裁决类 enum 全数在短名单内。
- 12 个候选零消费符号经符号级四步精确化（区分生产/测试/re-export 路径/prelude glob 使用位）：**全部外部零消费**；此前初判的"OmniMessage 有 1 文件消费"经查为 session-store 的 **doc 注释**假阳性（blank_rust 词法器正确剔除）。
- `dyn VectorStore` / `Box<dyn VectorStore` 全仓（L0 外）**0 命中**。
- schema_version 现落 `checkpoint.rs` 与 `token_evidence.rs`；`experience_card.rs` / `harness_spec.rs` 未落。
- 工作树在途改动 0（执行前 `git status` 复核）。

## 2. 决策

### 决策 1（D-23）：档 2 逐类型可扩展性裁决 —— 56 open / 171 closed

裁决规则（三条，逐类型可判定）：

- **R-E（enum）**：短名单 enum 一律 **open**（`#[non_exhaustive]`），**除** 8 个裁决类 enum 保持 **closed(刻意)**——编译期穷尽处理是安全特性（沿 `event-bus/registry.rs` 无通配 match 先例，ADR-214 决策 3 点名）。零外部提及 ⇒ 无下游 match 可破坏，open 今日零成本、明日防破坏。
- **R-S1（struct·open）**：演化型数据载体（报表/记录/载荷/配置面/规格 envelope，按语义命名判定）⇒ **open**；短名单成员外部零字面量 ⇒ open 编译安全（已由 `cargo check --workspace --all-targets` 实证）。
- **R-S2（struct·closed 终态）**：语义定型 value 类型——id newtype、数学参数/投影记录、opaque 序列化载体、成熟规格契约（formal_props 四件、capability token、sparse mask、skill lifecycle contract 等）⇒ **closed(终态)**，字段集即语义。

**open（56 = 15 enum + 41 struct，属性 + 台账行同批落地，commit d72e6d3）**：

| 模块 | open 类型 |
|---|---|
| core_domain/event_v2 | `Compressibility`、`EventPattern`（enum）；`NamespaceQuotaV2`（struct） |
| mca/affinity | `ServiceTier`、`ModalityKind`（enum）；`EndpointSpec`、`SamplingParams`、`AffinityOverrides`（struct） |
| mca/omni_message | `OmniMessage`（enum，v3.4.0 §5.8 预留协议面）；`TokenUsage`（struct） |
| mca/price_vector | `ModelPrice`（struct） |
| safety/adjudication | `FailureFixStatus`（enum）；`ClaimPayload`、`CertifyPayload`、`DeclaimPayload`、`FailureRecord`（struct） |
| safety/calibration | `HumanLabel`、`EvalSampleKind`（enum）；`CalibrationSample`、`EvalRecord`、`AgreementTrendPoint`、`BadCaseConversion`、`GateCoverage`、`MonthlyEvalReport`（struct） |
| safety/sensitive | `SensitiveKind`（enum）；`ScanHit`、`ScanVerdict`（struct） |
| safety/command_validation | `BlockedPattern`、`CommandPolicy`（struct） |
| memory/archive_monotonicity | `InvariantViolation`（struct） |
| memory/memory_pyramid | `SceneBlock`、`PersonaSummary`（struct） |
| memory/quota | `NamespaceQuota`、`QuotaLimits`（struct） |
| memory/token_evidence | `ToolCallRecord`（struct） |
| protocol/app | `Thread`、`Item`（struct）；`AppTokenUsage`、`ApprovalRequest`（struct） |
| spec/harness_dimensions | `WorkflowType`、`RetentionPolicy`、`EvictionStrategy`、`ExtractionFormat`、`FallbackStrategy`（enum）；`StopStrategyConfig`、`ValidationRule`、`ContextAssemblyContract`、`ToolInteractionContract`、`GenerationControlContract`、`TaskOrchestrationContract`、`MemoryManagementContract`、`OutputProcessingContract`（struct，D1-D6 控制面） |
| spec/platform_grounding | `GroundingCategory`（enum）；`GroundingRequirement`、`PlatformGroundingSpec`（struct） |
| seams/reward | `RewardSpec`（struct） |

**closed(刻意)（8 裁决类 enum，仅台账登记不改码）**：`ErrorLayer`、`RecoveryStrategy`、`ToolFailure`、`EventFailure`、`GateVerdict`、`CertifyVerdict`、`PlanViolation`、`JudgeTier`。

**closed(终态)（23 struct，台账行不变）**：`GraphIdentity`、`NormalizedThreeFactor`、`SkillLifecycleContract`、`ThreadId`、`TurnId`、`ItemId`、`ReqId`、`CapabilityToken`、`SerializedPolicy`、`RLState`、`SparseMask<T>`、`ContractExample`、`VerifiedWithStrength`、`InvariantSpec`、`EmptyFormalProvider`、`FormalProperty`、`RetryPolicy`、`VariantId`、`SinkhornParams`、`ProjectedMatrix`、`WeightedList`、`FusedItem`、`VectorHit`。

**编译实证与一笔实测修正**：open 化前先打属性跑 `cargo check --workspace --all-targets`，咬出 **11 处 E0639**——全部位于 L0 自身集成测试 `tests/wire_roundtrip.rs`（集成测试是独立编译单元，等同外部 crate；B0 短名单口径本就不含 self_tests，破坏面预测与编译器完全一致）。适配为既有 pub 构造函数 / `Default`+字段赋值（`TokenUsage` 保留原 `wrapping_add` 防溢出语义）；36 个外部消费 crate 零改动。这验证了 belt-and-suspenders 的价值：文本零提及是充分条件，编译通过是最终判据。

**反方案**：①"短名单全 open"——否决，23 个定型 value struct 开档只会永久禁止本应合法的外部字面量构造；②"全维持 closed"——否决，等于放弃 ADR-214 D-22 已识别的 61 处 ExperienceCard 外部字面量同款风险；③"`#[non_exhaustive]` 替代字段序冻结"——已由 ADR-213 R8 反方案③否决（两轴正交）。

### 决策 2（D-24）：零消费 pub 符号逐个裁决表（12 项）

| 符号 | 证据（2026-10-15 全仓实测） | 裁决 | 动作 |
|---|---|---|---|
| `looks_high_entropy`（fn, safety/sensitive.rs） | prod_ext=0；唯一调用方是同模块 `scan()`（已接线生产入口） | **收窄可见性** | `pub fn`→私有 fn；根 re-export + prelude 条目同批移除；G-105 台账删行（live 255→254，合法收缩） |
| `rm_distillation_eligible`（fn, safety/calibration.rs） | 外部零消费；但语义绑定 `JudgeTier::RewardModel`（§(b.6) RM 蒸馏触发判据） | **DEFERRED** | 保留；与 RL 开发闸门同步（删除将与保留 JudgeTier 矛盾） |
| `CalibrationSample` | `agreement_rate_milli`/`certifiable`/`from_calibration`/`from_samples` 签名类型 | **判已消费** | 仅本表登记 |
| `MonthlyEvalReport` / `AgreementTrendPoint` / `BadCaseConversion` / `GateCoverage` | 评测月报三表前瞻面，簇内互连；G-91 基线已登记 | **DEFERRED** | 保留，G-91 行不动 |
| `JudgeTier` | 裁决类 enum（closed 刻意）；`rm_distillation_eligible` 语义绑定 | **DEFERRED** | 保留 |
| `RecoveryStrategy` | `Recoverable` trait 返回契约；NexusError 家族 13 文件消费 | **DEFERRED** | 保留 |
| `MetadataBridge` / `NamespaceQuotaV2` | event_v2 WI-21 注册表配额/桥接设计面 | **DEFERRED** | 保留 |
| `ContextAssemblyContract` | D1-D6 控制面统一暴露的一员（D6 `HarnessConfigContract` 已被消费） | **DEFERRED** | 保留 |

机器面登记现状：上述 DEFERRED 类型的 `<2 消费者` 状态已由 G-91 基线 `contract_consumer_baseline.txt`（only-shrink）承载，本批不动其行；晋级 ≥2 消费者时按门头规则删行。反方案：①"全部删除"——否决，将腰斩模块语义（月报三表互连、错误契约家族）；②"全部登记 G-101 DEFERRED"——否决，G-101 账本语义是调用位契约（nexus-core ComputeBridge 先例），类型级未接线状态已有 G-91 承载，双登记违反"分母与基线互不相同、禁止互抄"（ADR-213 R8.3）。

### 决策 3（D-25）：文档口径同步

- `.qoder/rules/chimera.md` §2.1 层级表行 188 与 §1.1 v2.4.0 变更注行 202：「纯类型零依赖契约层」旧口径订正为「纯类型+受控纯逻辑」+ 指针（ADR-033 追加节 2026-10-09；chimera2.md 已是订正后口径，本轮补齐 chimera.md）。该文件不入库，本地面生效。
- `CODE_WIKI.md` §3.1 nexus-core 关键类型行：移除权威归属 L0 的 7 个领域类型，仅留 `CLV`/`NexusState` + 指针——消解同文档 §3.1 与 §4/agents.md §5.1 的新旧口径并存（commit 9d7bedd）。
- G-72/doc_drift/doc_symbol_claims/doc_consistency 四门复验 rc=0。

### 决策 4（D-26）：benches/util_micro.rs 的 `unsafe impl GlobalAlloc` 登记豁免

`#![forbid(unsafe_code)]` 在 lib.rs 只辖 lib 编译单元；bench 是独立 crate，G-99 纯度门辖区本就是 `src/**`——这不是门的漏洞而是辖区外事实，需要的是 ADR 明示而非补丁。`CountingAlloc` 是零堆分配硬断言（B0 性能证据链）的唯一实现手段，std 无等价替代。豁免边界：unsafe 仅限转发 `System` 分配器（alloc/dealloc/realloc 三个 unsafe fn），不做指针运算、不做自定义内存布局。反方案：①"换非 unsafe 实现"——不存在（`#[global_allocator]` 本身即 unsafe impl）；②"搬出 L0"——切断零分配断言与被测纯函数的证据链归属。

### 决策 5（D-27）：候选 (a) schema_version 铺开 → **defer**

Checkpoint（LHQP envelope）与 token_evidence 已落；`ExperienceCard`/`HarnessSpec` 未落。铺开须等第二个 wire 演进实需，且 **`HarnessSpec::canonical_merkle_input`（95 行 Merkle 叶子序列化）是硬约束**：给 HarnessSpec 加任何字段都会改变 canonical 输入即改变证据链哈希，须另立迁移方案，不得按"尾部追加 + default"惯性操作。触发条件：出现对 ExperienceCard/HarnessSpec 的实际字段演进需求时，在同一 ADR 批次内按 ADR-213 §9 的实测纪律（同轮镜像结构体字节比对）落地。

### 决策 6（D-28）：候选 (b) VectorStore trait dyn 兼容拆分 → **reject（本轮）**

`dyn VectorStore` / `Box<dyn` / `Arc<dyn` 全仓（L0 外）**0 命中**（B0 派生证据）——不存在需要对象安全的调用形态。拆分 object-safe 核心 trait 是对实现方 crate 的破坏性重构，无需求证据即施工是 ADR-213 W8 教训（"把可能错误的方向锁成铁律"）的镜像。触发条件：出现真实 dyn 分发需求（如多向量后端运行时切换）时重评，需先给出使用面证据。

### 决策 7（D-29）：候选 (c) G-91 升级为逐 crate 装配面棘轮 → **defer**

G-91（`check_contract_consumers.py` + `contract_consumer_baseline.txt` only-shrink）已锁 `<2 消费者` 面；逐 crate 消费矩阵是新轴（新门号 G-117+、新台账、脚本+台账+manifest+ci.yml 四件同批），其收益（防单 crate 消费漂移）低于本批 B1/B2 已付的逐类型审查成本。触发条件：下波再出现装配面事故（消费 crate 静默流失导致契约退化）时立项。

## 3. 影响

| 维度 | 影响 |
|---|---|
| 代码 | 16 个 src 文件 +56 处 `#[non_exhaustive]`；`sensitive.rs` 1 fn 收窄可见性；`lib.rs` 2 处 re-export 行收缩；`tests/wire_roundtrip.rs` 11 处构造点适配 |
| 台账 | G-115 56 行 closed→open（open=56/closed=171）；G-105 删 1 行（255→254）；G-107/G-91/G-113 零变更 |
| 门 | 零新增门、零 manifest/ci.yml 变更（三候选均 defer/reject） |
| 公共 API | open 化 = 对外语义收紧（外部禁字面量构造/穷尽 match 需通配臂）；`looks_high_entropy` 移出 pub 面。两者均为编译期可见的源码级变更，36 消费 crate 实测零受影响（外部零字面量/零提及 + 全 workspace 编译 rc=0） |
| 事件 | `NexusEvent` 三锁零变更（152 / Critical 20 / mpsc 16） |
| 依赖 | 零新增依赖边；crate 数 39 不变；L0 仍单 crate（ADR-212 决策 1 延续） |
| unsafe | 零新增（bench 豁免为既有事实的登记） |

## 4. 验收判据（全部 2026-10-15 实测 rc=0）

- `cargo check --workspace --all-targets`；`cargo test -p nexus-contracts`（58+15+13 passed / 0 failed）
- G-99/101/105/107/113/115/91/58/62/33 + 依赖铁律逐门 rc=0；`audit_gate_parity` + `check_gates_provenance` rc=0
- G-72/doc_drift/doc_symbol_claims/doc_consistency 四门 rc=0
- `cargo clippy -p nexus-contracts --all-targets -- -D warnings`；`cargo fmt -p nexus-contracts -- --check`

## 5. 回滚

四个独立提交（d72e6d3 B1 / 6d87e9a B2 / 9d7bedd B3 / 2a9c1e2 B4）逐个 `git revert`——代码与台账同批互锁，revert 即整体还原。禁 `git checkout --`/`restore`/`stash`/`add -A`。`.qoder/rules/chimera.md`（不入库）为单行反向编辑。

## 6. 未做 / 待裁（诚实登记）

- B0 扫描器文本匹配看不见宏拼接标识符（`paste!`/`concat_idents!` 类）；workspace 依赖现测未引入此类 crate，若未来引入须在派生脚本登记盲区。
- struct 短名单判据"无外部字面量"不含 serde 字符串形态的构造（`serde_json::from_value`）；open 化对该路径无影响（Deserialize 不受 non_exhaustive 限制），但 D-23 的证据口径不含它。
- G-91 基线中与本批无关的其他 `<2 消费者` 行（如 affinity 组部分类型）未逐个复核——超出本轮 L0 裁决范围。
- D-18 孤儿源文件 9 处（chimera-tui 5 / parliament 3 / quest-engine 1）属 L0 外，未动。

## 7. 执行记录

| 提交 | 内容 |
|---|---|
| d72e6d3 | B1：56 类型 open + G-115 台账同批 + wire_roundtrip 11 处适配 + B0 派生脚本入库 |
| 6d87e9a | B2：looks_high_entropy 收窄 + G-105 删行 + D-24 裁决 |
| 9d7bedd | B3：CODE_WIKI §3.1 口径订正（chimera.md 本地面同步） |
| 2a9c1e2 | B4：util_micro 豁免登记 |

验收报告：`docs/reports/L0-closure-ADR-215_2026-10-15.md`。
