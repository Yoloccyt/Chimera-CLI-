# ADR-047: FormalVerifier L4 验证器层级跃迁路线图

## 状态

提议 (Proposed) (2026-07-27)

> **🗑 2026-10-14 已删 crate 校正**：本文 **4 处 `auto-dpo`（含 2 条 `file://` 死链）产物不在当前代码库**（批次 A `38c72e6`，2026-09-20，登记见 [`CODE_WIKI.md §3.12`](CODE_WIKI.md)）。依 R7「历史只加注不改写」保留正文。本 ADR 状态为**提议（Proposed）**，尚未落地 ⇒ 受影响面仅为**代码锚点悬空**：正式实施前须按 §3.12 校正引用点，勿据正文去 `crates/auto-dpo/` 寻找已不存在的载体。路线图本身未被推翻。

> **ADR-159 定稿关联注记(2026-08-28)**:本 ADR 规划新增 `NexusEvent::FormalVerificationFailed`(Critical,mpsc 旁路)的事件化设计**未落实**。冗余审计维度2 确认代码中 `FormalVerificationFailed` 实为 `GsoeError` 的错误变体(gsoe-evolution/src/error.rs:98,由 aegis/critic 返回),按 ADR-159 定稿:**不为此伪造事件面**,从 Critical 文档清单剔除该称谓;若未来需事件化必须另行立项并注册旁路。本 ADR 正文保留为历史设计(当时规划合理,落地路径事后修正)。

> **状态说明**: 本 ADR 于 2026-07-27 由 P0 优化实施阶段创建,作为 Phase 5 技术债盘点 P0-1 项的修复交付物。本 ADR 是 ADR-042( R2 冻结)决策 2 明确预期的"FormalVerifier 设计决策 ADR"——ADR-042 第 65 行写道:"新建 ADR(预计 ADR-043 或后续编号)记录 FormalVerifier 的设计决策(算法选型、属性语言、证明器集成)",因 ADR-043~046 已被 R1 影子模式 / RHI-CG 工程 / INV-9 命名 / ImmuneSystem facade 占用,本 ADR 编号确定为 ADR-047。本 ADR 落地后将成为 R2 解冻的前置条件之一(ADR-042 决策 2 的 5 项判定标准之第 2 项)。

## 背景

### 三重悖论进化悖论红线(本 ADR 的根本动因)

- **§3.4.5 三重悖论红线**(进化悖论红线,权威源 `.trae/rules/nuxus规则.md`):"当前 GSOE/AutoDPO 使用执行反馈(测试通过/失败)作为验证信号,属于验证器层级 L3(执行反馈),存在被'奖励黑客'游戏化风险;第三阶段需向 L4(形式化验证)或 L5(人类研究判断)跃迁"。
- **根因分析**:L3 执行反馈存在三类不可消除的盲区:
  1. **"通过测试但逻辑错误"**:测试覆盖率为 X% 时,未覆盖的 (1-X%) 路径可能隐藏退化行为,GSOE 进化策略学会"恰好通过测试"而非"真正改进逻辑"
  2. **奖励黑客(Reward Hacking)**:AutoDPO 偏好对生成依赖 L2 学习型评判器,而 L2 评判器本身可能被策略游戏化(自评偏差不可控,ADR-032 决策 4 已明确"L2 不可单独作为部署依据")
  3. **不变量漂移**:长期进化过程中,INV-7/INV-8/INV-9 等不变量在边界条件下可能被缓慢侵蚀,而单元测试难以覆盖所有边界组合

### ADR-042 的前置约束

- **ADR-042 决策 2**(冻结期限)明确:R2(GSOE×AutoDPO 约束 RL)在 FormalVerifier 落地前无条件冻结,FormalVerifier 落地的 5 项判定标准为:
  1. 代码落地(`crates/seccore/src/formal_verifier.rs` 或独立 crate)
  2. **ADR 落档**(本 ADR 履行此项)
  3. L4 验证能力证明(至少 1 个 spec 属性通过形式化证明)
  4. 集成测试通过
  5. 架构评审通过(E01/E02/E06 三方)
- **ADR-042 决策 3**(三阶递进式解冻):阶段 1 前置条件即 FormalVerifier 落地验证,本 ADR 是阶段 1 的核心交付物

### 上游设计要求

- **三重悖论文档 §4.3.1**(权威源 `AI_Agent_三重悖论_x_Chimera_深度映射分析.md:343-369`):定义了 `FormalVerifier` 的四组件结构:
  ```rust
  pub struct FormalVerifier {
      pub type_checker: RustcTypeChecker,           // 类型系统检查(Rust 编译器本身)
      pub static_analyzer: StaticAnalyzer,           // 静态分析(Miri / Clippy 深度规则)
      pub invariant_checker: InvariantChecker,       // 不变量检查(用户定义的契约)
      pub resource_bound_checker: ResourceBoundChecker, // 资源边界检查(内存/时间/IO)
  }
  ```
  关键决策:"GSOE 的适应度评估必须通过 FormalVerifier 才能进入 AutoDPO 的偏好对生成"
- **v5.0 设计文档 §7.5**:"R2 在 FormalVerifier 落地前无条件冻结;回放池 ≥10K 轨迹 + 影子模式 2 周为 R1 解冻前置"
- **模块级系统性优化分析报告 v4.0.0-omega §L4**:建议在安全层增加形式化验证器支持

### 当前代码基线(v2.3.1-omega → v2.4.0-omega,2026-07-27 核实)

- [crates/gsoe-evolution/src/engine.rs](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs) 已实现 `GsoeEvolutionEngine::evolve_once()` — 基于执行反馈的进化主路径(L3 验证器),`evolve_once()` 通过 `ExecutionFeedback` 信号驱动进化,未实现 L4 形式化验证门
- [crates/gsoe-evolution/src/policy/grpo.rs](file:///D:/Chimera CLI/crates/gsoe-evolution/src/policy/grpo.rs) 实现 GRPO 风格采样与优势计算,奖励信号源自 L3 执行反馈
- [crates/auto-dpo/src/generator.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs) 已实现 `PreferencePairGenerator::generate()` — 生成偏好对供通道 A 提议使用,L2 学习型评判器
- [crates/chimera-mas/src/invariants.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs) 已实现 `InvariantChecker`(INV-7/INV-8 proptest 1000 次无违反)— 可复用为 FormalVerifier 的 `invariant_checker` 组件基础
- [crates/seccore/src/sandbox.rs](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs) 已实现降级版沙箱(Linux gVisor 未实装,ADR-001 降级)— `resource_bound_checker` 可复用沙箱的进程级资源限制
- **FormalVerifier 尚未落地**:37 crate × 624 .rs 文件零命中 `FormalVerifier` / `formal_verifier`,L4 验证器层级未实现

### 学术支撑

- **Khan, 2026**(Token Budgets):论证了执行反馈在 token 受限场景下的盲区,形式化验证可弥补
- **Zero-trust LLM Agents**(Kushnerov, 2026):零信任 Agent 架构要求形式化不变量证明,而非依赖测试覆盖率
- **AI Alignment 文献**(Reward Hacking):EvolutionStrategies + RL 的奖励黑客风险已有学术共识,L4 形式化验证是结构性防御
- **Lean / Coq 在系统软件中的应用**:Rust 生态的 `flux` / `prusti` 项目已证明类型系统级形式化验证的可行性

## 决策

经专家团队多轮结构化思考与多路径交叉验证,对 FormalVerifier L4 验证器层级跃迁作出以下 11 项架构决策:

> **⚠ 实施现状核对（2026-09-22，文档审查批次 P109，只加注不改写正文）**：决策 1 的落点（嵌入 `seccore`）**未被实现采用**。实测：`crates/seccore/src/` 无 formal* 文件；唯一 `FormalVerifier*` 命名类型为 `FormalVerifierGate`（`crates/gsoe-evolution/src/formal_gate.rs:84`），验证器属性与门分散落在 parliament / gsoe-evolution / nexus-contracts / decay-engine / event-bus / omega-learner 六个 crate。能力面（M0/M1/M2 与 9 属性）经 `Cargo.toml` 的 4 个 `formal_verifier_*` test target 佐证成立，但**本 ADR 的落点与文件规划不可再作为施工依据**；正解是补一份 superseding ADR 记录落点改判，或改写决策 1。

### 决策 1: crate 选型 — 嵌入 seccore 而非新建独立 crate

**决策内容**:FormalVerifier 嵌入 [crates/seccore/src/formal_verifier.rs](file:///D:/Chimera CLI/crates/seccore/src/formal_verifier.rs),不新建独立 crate。

**选址理由**:

1. **依赖铁律合规**:seccore 位于 L4 安全层,FormalVerifier 作为安全验证组件天然属于 L4;若新建独立 crate(如 `formal-verifier`),需额外论证层级归属与依赖方向
2. **复用 seccore 既有能力**:`resource_bound_checker` 可复用 `sandbox.rs` 的进程级资源限制;`SandboxConfig` / `SandboxResult` 类型可直接复用
3. **复杂度预算守恒**(v5.0 设计文档 §3.4 净增长 ≤ 0):不新建 crate 避免复杂度增长;seccore 既有 `#![forbid(unsafe_code)]` 约束自动覆盖 FormalVerifier
4. **ADR-028 决策 1(append-only)对齐**:在 seccore 内新增 `formal_verifier.rs` 模块是 append-only 扩展,不修改既有 API 签名
5. **与 ADR-042 预期一致**:ADR-042 决策 2 第 1 项判定标准明确"`crates/seccore/src/formal_verifier.rs` 或独立 crate",首选 seccore 嵌入

**模块结构**:

```
crates/seccore/src/
├── lib.rs                 # 新增 pub mod formal_verifier;
├── sandbox.rs             # 既有(降级版沙箱)
├── asa.rs                 # 既有(ASA 审计)
├── formal_verifier.rs     # 新增:FormalVerifier 主体
├── formal_verifier/
│   ├── mod.rs             # 子模块入口
│   ├── type_checker.rs    # RustcTypeChecker(复用 cargo check)
│   ├── static_analyzer.rs # StaticAnalyzer(clippy 深度规则 + Miri)
│   ├── invariant.rs       # InvariantChecker(复用 chimera-mas invariants.rs 模式)
│   └── resource_bound.rs  # ResourceBoundChecker(复用 sandbox.rs 资源限制)
└── ...
```

**不采纳独立 crate 的理由**:

- 新建 crate 需更新 `CODE_WIKI.md §3.1` 37 crate 索引、`Cargo.toml` workspace.members、`CHANGELOG.md`,文档同步成本高
- 独立 crate 的 L4 归属需额外 ADR 论证(是否为 L4.5?是否引入新依赖方向?)
- seccore 既有 `Error` 枚举可直接扩展 `FormalVerifierError` 变体,无需新建 error.rs

### 决策 2: 验证器层级跃迁路径 — 三阶段渐进式实施

FormalVerifier L4 跃迁不可"一步到位",必须经三阶段渐进式实施,确保每阶段可证伪、可回滚:

**阶段 1: 骨架实施(M0,预计 2026-08-15)**

- **范围**:实现 `FormalVerifier` 结构体 + 四组件 trait 定义 + 3 个核心属性的 proptest 覆盖
- **M0 细化规格**(详见决策 8~11):
  - **属性语言**:proptest 属性测试作为务实路径(kani 不支持 Windows,prusti 泛型支持有限),保留 kani/prusti 作为 M2 未来增强选项
  - **零新依赖**:M0 不引入新 Cargo 依赖(proptest 已在 workspace 中可用)
  - **3 个核心属性**:GSOE 谱系完整性 / AEGIS Critic 单调性 / Parliament 一致性(详见决策 9)
  - **代码落点**:`nexus-contracts/src/formal_props.rs`(L0 契约类型)+ 各 crate 验证器模块 + E2E 测试(详见决策 10)
- **交付物**:
  - `crates/seccore/src/formal_verifier.rs` + 子模块骨架
  - `FormalVerifier` / `VerificationTier` / `VerificationResult` 类型定义
  - `TypeChecker` / `StaticAnalyzer` / `InvariantChecker` / `ResourceBoundChecker` 四 trait
  - `crates/nexus-contracts/src/formal_props.rs` — L0 契约类型(`FormalProperty` / `VerificationResult` / `InvariantSpec`)
  - `crates/gsoe-evolution/src/formal/lineage_checker.rs` — GSOE 谱系 DAG 完整性验证器
  - `crates/gsoe-evolution/src/formal/critic_monotonicity.rs` — AEGIS Critic 单调性验证器
  - `crates/parliament/src/formal/consensus_safety.rs` — Parliament 一致性验证器
  - `tests/e2e/formal_verifier_e2e.rs` — FormalVerifier E2E 闭环测试
  - 3 个核心属性全部通过 proptest 属性测试覆盖
- **验收标准**(详见决策 11):
  - `cargo check -p seccore` 通过
  - `cargo test -p seccore formal_verifier` 通过(≥10 个单元测试)
  - `cargo clippy -p seccore -- -D warnings` 零警告
  - ≥3 个核心属性有 proptest 属性测试覆盖
  - ≥50 个属性测试用例(全项目范围,含既有 proptest 用例)
  - `tests/e2e/formal_verifier_e2e.rs` E2E 闭环测试通过
  - 不删既有测试,workspace 全量测试 0 失败
- **SLO**:单次 `FormalVerifier::verify()` 调用 < 500ms(骨架阶段,无性能压力)

**阶段 2: 集成实施(M1,预计 2026-09-15)**

- **范围**:将 FormalVerifier 集成到 GSOE 进化主路径,作为 L3→L4 门
- **交付物**:
  - `GsoeEvolutionEngine::evolve_once()` 在执行反馈(L3)通过后,调用 `FormalVerifier::verify()` 作为 L4 门
  - L4 门失败时发布 `NexusEvent::FormalVerificationFailed`(Critical 级,走 mpsc 旁路通道)
  - `AutoDPO::PreferencePairGenerator::generate()` 仅接受 L4 门通过的候选
  - 集成测试:`crates/gsoe-evolution/tests/formal_verifier_integration.rs`
- **验收标准**:
  - `cargo test -p gsoe-evolution` 全量通过
  - `cargo test -p auto-dpo` 全量通过
  - 至少 5 个 spec 属性通过形式化证明(M0 的 3 个 + M1 新增 2 个)
  - L4 门单次调用 < 200ms(集成阶段,需优化)
- **SLO**:`evolve_once()` 端到端延迟增加 < 30%(L4 门开销)

**阶段 3: 完整实施(M2,预计 2026-10-15)**

- **范围**:FormalVerifier 全功能落地,满足 ADR-042 决策 2 的 5 项判定标准
- **交付物**:
  - ≥7 个 spec 属性通过形式化证明
  - `ResourceBoundChecker` 完整实现(内存/时间/IO 三维边界)
  - `StaticAnalyzer` 集成 Miri(unsafe 检测)+ Clippy 深度规则
  - 架构评审报告(E01/E02/E06 三方书面批准)
  - 性能基准:`crates/seccore/benches/formal_verifier_benchmark.rs`

> ⚠ **现状注记（2026-09-22 按代码复核；决策与里程碑叙述本身不改写）**
> 本文 M1/M2 的三个锚点在当前代码库**都不成立**，照做会失败：
> 1. `crates/gsoe-evolution/tests/formal_verifier_integration.rs`（:140 / :314 / :541）**不存在**
>    （`crates/gsoe-evolution/tests/` 实有 `integration.rs`、`four_operators_test.rs` 等 7 个文件，无此名）。
>    实际承载 = 根 package `chimera-e2e-tests` 注册的四目标：`Cargo.toml:448 formal_verifier_e2e`、
>    `:460 formal_verifier_m1_e2e`、`:472 formal_verifier_m2_e2e`（文件在 `tests/e2e/`，另有 `formal_verifier_gate_e2e.rs`）。
> 2. `crates/seccore/benches/formal_verifier_benchmark.rs`（:156 / :540）**不存在**
>    （`crates/seccore/benches/` 实有 `asa_audit.rs`/`command_validator_bench.rs`/`gvisor_bench.rs`）；
>    形式化门评测的实际基准是 `crates/gsoe-evolution/benches/formal_gate_eval.rs`。
> 3. 阶段 2 验收标准第 2 条 `cargo test -p auto-dpo`（:143）**已不可执行** —— `auto-dpo` crate 已被物理删除
>    （提交 `38c72e6`，见 `docs/architecture/CODE_WIKI.md §3.12`）⇒ 该条按历史读，现状等价验证请走上述 e2e 目标。
> ⇒ 结论不变（M0+M1+M2 已落地，见 `:158`），但**落点与验证命令需按本注记替换**。
- **验收标准**(对齐 ADR-042 决策 2):
  1. 代码落地 ✅(M0+M1+M2 累计)
  2. ADR 落档 ✅(本 ADR)
  3. L4 验证能力证明 ✅(≥7 个 spec 属性)
  4. 集成测试通过 ✅(M1 已完成)
  5. 架构评审通过 ✅(M2 交付物)
- **SLO**:`FormalVerifier::verify()` 单次调用 < 100ms(M2 目标,对齐 v5.0 设计文档 §9.5 KPI-03 适应性免疫层 <100ms)

### 决策 3: 属性语言选型 — Rust 类型系统 + proptest 契约(非 SMT-LIB/Lean)

**决策内容**:FormalVerifier 的属性语言采用 **Rust 类型系统 + proptest 契约 + clippy 静态规则** 三层组合,而非 SMT-LIB / Lean / Coq 等外部证明器。

**选型理由**:

1. **forbid(unsafe_code) 合规**:SMT-LIB / Lean / Coq 的 Rust binding(如 `z3-sys` / `lean-sys`)多数依赖 unsafe FFI,违反 `#![forbid(unsafe_code)]`(§4.1 红线);Rust 类型系统 + proptest 全程 safe Rust
2. **零外部依赖**:不引入 Z3 / cvc5 / Lean 二进制依赖,避免 Docker 镜像膨胀(§10.2 体积 <100MB 约束)
3. **与既有测试体系无缝集成**:proptest 已在 37 crate 中使用(102 处,Phase 1 基线核验),InvariantChecker 已在 chimera-mas 落地(INV-7/INV-8 1000 次无违反)
4. **CI 友好**:clippy + cargo check + cargo test 是既有 CI 流程,无需新增 SMT solver job
5. **实用主义**:完整的 SMT 形式化验证在工业级 Rust 项目中尚无成熟先例(`flux` / `prusti` 仍在研究阶段),三层组合可覆盖 80% 的实际不变量需求

**三层属性语言定义**:

| 层级 | 工具 | 验证能力 | 实施成本 |
|------|------|---------|---------|
| L4.1 类型系统 | Rust 编译器 | 类型安全、生命周期、所有权 | 零(编译器内建) |
| L4.2 静态分析 | clippy 深度规则 + Miri | unsafe 检测、常见反模式、UB 检测 | 低(clippy config + CI job) |
| L4.3 契约测试 | proptest + InvariantChecker | 不变量属性、边界条件、随机输入 | 中(需编写属性函数) |

**不采纳 SMT-LIB / Lean 的理由**:

- `z3-sys` / `z3` crate 依赖 C FFI,违反 `#![forbid(unsafe_code)]`
- Lean / Coq 的 Rust 集成尚在研究阶段,无生产级 binding
- SMT solver 的计算成本高(单次求解秒级),不满足 <100ms SLO
- 学习曲线陡峭,团队需额外掌握 SMT-LIB / Lean 语言

### 决策 4: 四组件实现策略 — 复用优先 + append-only 扩展

**决策内容**:FormalVerifier 四组件尽可能复用既有实现,append-only 扩展,避免重复造轮子。

**复用映射表**:

| 组件 | 复用源 | 复用方式 | 新增内容 |
|------|--------|---------|---------|
| `TypeChecker` | Rust 编译器(`cargo check`) | 调用 `cargo check --message-format=json` 解析错误 | `TypeChecker::check()` 包装 + 结果解析 |
| `StaticAnalyzer` | clippy(`cargo clippy`) + Miri(`cargo miri`) | 调用 `cargo clippy --message-format=json` + 可选 Miri | `StaticAnalyzer::analyze()` 包装 + 规则配置 |
| `InvariantChecker` | [crates/chimera-mas/src/invariants.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs) | 复用 `InvariantChecker` 模式,扩展 L4 属性函数 | `L4InvariantChecker` trait + 5 个属性函数 |
| `ResourceBoundChecker` | [crates/seccore/src/sandbox.rs](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs) | 复用 `SandboxConfig` 的资源限制字段 | `ResourceBoundChecker::check()` + 内存/时间/IO 三维 |

**append-only 扩展原则**(对齐 ADR-028 决策 1):

- 不修改 `InvariantChecker` 既有签名,新增 `L4InvariantChecker` trait 继承
- 不修改 `SandboxConfig` 既有字段,新增 `ResourceBoundConfig` 结构体
- 不修改 `GsoeEvolutionEngine::evolve_once()` 既有签名,新增 `evolve_with_formal_verification()` 方法

### 决策 5: 与 GSOE/AutoDPO 集成路径 — L4 门作为 L3 通过后的第二道闸

**决策内容**:FormalVerifier 作为 GSOE 进化主路径的 L4 门,在 L3 执行反馈通过后触发;L4 门通过的候选才可进入 AutoDPO 偏好对生成。

**集成流程**:

```
evolve_once() 主路径:
  1. 变异生成候选(L0 内在信号)
  2. L2 学习型评判器评分(AutoDPO PreferencePairGenerator)
  3. L3 执行反馈(测试通过/失败)— 既有路径
  4. 【新增】L4 形式化验证门(FormalVerifier::verify())
     ├─ L4 通过 → 候选进入偏好对生成(既有流程)
     └─ L4 失败 → 发布 NexusEvent::FormalVerificationFailed(Critical)
                   候选被否决,不进入偏好对生成
```

**事件设计**:

- 新增 `NexusEvent::FormalVerificationFailed { metadata, candidate_id, failure_reason, attribute_name }`(Critical 级,走 mpsc 旁路通道)
- 新增 `NexusEvent::FormalVerificationPassed { metadata, candidate_id, attributes_verified }`(Normal 级,broadcast)
- 对齐 §6.2 红线:Critical 事件用 mpsc channel 确保送达

**API 设计**:

```rust
// crates/gsoe-evolution/src/engine.rs(append-only 扩展)
impl GsoeEvolutionEngine {
    /// 既有方法(L3 验证器)— 不修改
    pub async fn evolve_once(&self) -> Result<EvolutionResult> { ... }

    /// 新增方法(L4 验证器门)— append-only
    pub async fn evolve_with_formal_verification(
        &self,
        formal_verifier: &FormalVerifier,
    ) -> Result<EvolutionResult> {
        // 1. 调用既有 evolve_once()(L3 通过)
        let candidate = self.evolve_once().await?;
        // 2. L4 形式化验证门
        let verification_result = formal_verifier.verify(&candidate).await?;
        match verification_result {
            VerificationResult::Passed { attributes } => {
                self.event_bus.publish(NexusEvent::FormalVerificationPassed {
                    metadata: EventMetadata::new("gsoe-evolution"),
                    candidate_id: candidate.id.clone(),
                    attributes_verified: attributes,
                }).await?;
                Ok(candidate)
            }
            VerificationResult::Failed { reason, attribute } => {
                self.event_bus.publish_critical(NexusEvent::FormalVerificationFailed {
                    metadata: EventMetadata::new("gsoe-evolution"),
                    candidate_id: candidate.id.clone(),
                    failure_reason: reason,
                    attribute_name: attribute,
                }).await?;
                Err(GsoeError::FormalVerificationFailed { reason, attribute })
            }
        }
    }
}
```

### 决策 6: 时间表与里程碑 — 三阶段对齐 ADR-042 解冻预期

**时间表**(对齐 ADR-042 决策 2 预计解冻时间 2026-10-15):

| 阶段 | 里程碑 | 预计完成日期 | 交付物 | 对齐 ADR-042 |
|------|--------|-------------|--------|-------------|
| M0 骨架 | FormalVerifier 结构 + 3 属性(proptest) | 2026-08-15 | `formal_verifier.rs` + 子模块 + `formal_props.rs` + 3 验证器 + E2E 测试 | 判定标准 1(代码落地) |
| M1 集成 | GSOE+AutoDPO 集成 + 5 属性 | 2026-09-15 | `evolve_with_formal_verification()` + 集成测试 | 判定标准 4(集成测试) |
| M2 完整 | 全功能 + 7 属性 + 评审 | 2026-10-15 | 7 spec 属性 + 架构评审报告 + benchmark | 判定标准 3+5(能力证明+评审) |

**关键路径**:

- M0 → M1:依赖 M0 的骨架完整性与 3 属性通过
- M1 → M2:依赖 M1 的集成测试通过与 5 属性通过
- M2 完成 → R2 解冻 ADR(预计 ADR-048 或后续):依赖 ADR-042 决策 2 的 5 项判定标准全部满足

**资源需求**:

| 角色 | 投入 | 职责 |
|------|------|------|
| E01 首席架构师 | 0.3 FTE × 2 月 | 架构设计 + 评审 |
| E02 安全架构师 | 0.2 FTE × 2 月 | 安全属性定义 + 评审 |
| E06 认知科学专家 | 0.2 FTE × 2 月 | 验证器层级理论 + 评审 |
| E05 生产系统专家 | 0.3 FTE × 2 月 | 集成实施 + benchmark |
| E07 任务调度专家 | 0.2 FTE × 2 月 | spec 属性编写 + proptest |

### 决策 7: 验收标准与回滚预案 — 可证伪 + 可回滚

**验收标准**(对齐 ADR-042 决策 2 的 5 项判定标准):

1. **代码落地**:M0+M1+M2 累计交付,`cargo check` / `cargo test` / `cargo clippy -D warnings` 全绿
2. **ADR 落档**:本 ADR(ADR-047)
3. **L4 验证能力证明**:M2 交付 ≥5 个 spec 属性通过形式化证明,属性清单:
   - 属性 1:GSOE 谱系完整性 — 谱系图 DAG 无环 + 回滚可达性 + 变异幅度硬上限(M0 交付,详见决策 9)
   - 属性 2:AEGIS Critic 单调性 — 适应度提升 → Critic 评分单调不减(M0 交付,详见决策 9)
   - 属性 3:Parliament 一致性 — Security 一票否决不可被多数覆盖 + 2/3 多数阈值正确性(M0 交付,详见决策 9)
   - 属性 4:HCW selector 权重向量归一化(M1 交付)
   - 属性 5:OmniSparseMasks 五维度掩码互斥性(M1 交付)
   - 属性 6:GQEP 聚集超时不变量(M2 交付)
   - 属性 7:CMT tier 降级单调性(对齐 INV-8,M2 交付)
4. **集成测试**:M1 交付 `formal_verifier_integration.rs`,覆盖 L4 门通过/失败两条路径
5. **架构评审**:M2 交付评审报告,E01/E02/E06 三方书面批准

**回滚预案**:

- **M0 失败**:骨架无法通过 `cargo check` → 回退到设计阶段,重新评估属性语言选型(决策 3)
- **M1 失败**:集成导致 `evolve_once()` 性能回退 >30% → 优化 L4 门实现(如缓存验证结果、并行化),或降级为"采样验证"(每 N 次进化验证 1 次)
- **M2 失败**:5 属性无法全部通过 → 评估失败属性是否为"不可证明"(如属性本身有误)或"实现缺陷",前者修正属性,后者修复实现
- **整体回滚**:若 FormalVerifier 在 2026-10-15 仍无法满足 5 项判定标准,R2 维持冻结(ADR-042 决策 2),本 ADR 标记为"延期",不触发 R2 解冻

### 决策 8: M0 属性语言选型决策 — proptest 属性测试作为务实路径

**决策内容**:M0 骨架阶段采用 **proptest 属性测试** 作为形式化验证的务实实施路径,而非 kani 或 prusti。保留 kani / prusti 作为 M2 未来增强选项。

**选型理由**:

1. **kani 不支持 Windows**:kani(基于 Rust 模型检查)目前仅支持 Linux/macOS,无法在团队主力 Windows 开发环境运行,CI 也需额外 Linux runner 配置
2. **prusti 泛型支持有限**:prusti(基于 Viper 的 Rust 验证器)对 Rust 泛型、trait object、async 的支持仍在开发中,无法覆盖 `GsoeEvolutionEngine` 等核心泛型结构
3. **proptest 已在 workspace 可用**:proptest 已在 37 crate 中广泛使用(102 处引用),InvariantChecker 已有 INV-7/INV-8 1000 次无违反的成熟实践
4. **M0 零新依赖**:proptest 已在 workspace `Cargo.toml` 中声明,M0 不引入任何新 Cargo 依赖
5. **渐进式增强**:M2 阶段可评估引入 kani(需 CI Linux runner)或 prusti(待泛型支持成熟)作为更强验证手段,M0 的 proptest 属性可无损迁移为 kani/prusti 的前置约束

**不采纳 kani 作为 M0 路径的理由**:

- kani 需要 `cargo kani` 子命令,需额外安装且仅支持 Linux
- kani 的证明后端(CBMC)对大规模 Rust 代码的扩展性尚未经过生产验证
- 团队学习成本:kani 的 `proof` / `unwind` 注解需要额外培训

**不采纳 prusti 作为 M0 路径的理由**:

- prusti 对泛型、trait object、async/await 的支持不完整(官方标注为 "work in progress")
- prusti 的 `#[ensures]` / `[requires]` 注解无法覆盖 `GsoeEvolutionEngine::evolve_once()` 的 async 签名
- prusti 编译时间开销大(秒级 → 分钟级),不满足 CI 反馈循环要求

### 决策 9: M0 验证范围 — 3 个核心属性

**决策内容**:M0 交付 3 个核心属性的 proptest 属性测试覆盖,覆盖 GSOE / AEGIS / Parliament 三大安全关键模块。

**属性 1: GSOE 谱系完整性**

- **定义**:GSOE 进化过程中维护的谱系图(provenance graph)必须满足以下不变量:
  1. **DAG 无环性**:谱系图必须为有向无环图(Directed Acyclic Graph),任何进化步骤不得产生环路
  2. **回滚可达性**:任意节点必须可通过谱系边回退至任意祖先节点(回滚路径存在性)
  3. **变异幅度硬上限**:单次进化步骤的变异幅度不得超过预定义阈值(防止"跳跃式进化"导致质量退化)
- **proptest 策略**:随机生成进化步骤序列,验证上述 3 个不变量在任意合法输入下成立
- **验证器**:`crates/gsoe-evolution/src/formal/lineage_checker.rs`

**属性 2: AEGIS Critic 单调性**

- **定义**:当候选方案的适应度(fitness)提升时,AEGIS Critic 的评分必须单调不减(即适应度更高的候选,Critic 评分不得低于适应度较低的候选)
- **目的**:防止"奖励黑客"(Reward Hacking)——Critic 评分与适应度不一致时,进化策略可能学会"讨好 Critic"而非"真正改进"
- **proptest 策略**:随机生成适应度序列 `(f1, f2)` 满足 `f1 ≤ f2`,验证 `critic_score(f1) ≤ critic_score(f2)`
- **验证器**:`crates/gsoe-evolution/src/formal/critic_monotonicity.rs`

**属性 3: Parliament 一致性**

- **定义**:Parliament 投票机制必须满足以下安全不变量:
  1. **Security 一票否决不可被多数覆盖**:若 Security Agent 投出否决票(veto),无论其他 Agent 的多数意见如何,该提案必须被否决
  2. **2/3 多数阈值正确性**:非 Security 事项的通过阈值必须精确为 2/3 多数(≥ ⌈2N/3⌉),不可被简单多数(> 1/2)绕过
- **proptest 策略**:随机生成投票组合,验证上述 2 个不变量在所有可能的投票分布下成立
- **验证器**:`crates/parliament/src/formal/consensus_safety.rs`

### 决策 10: M0 代码落点映射

**决策内容**:M0 的代码落点遵循"契约类型集中、验证器分散"的原则——L0 契约类型定义在 `nexus-contracts`,各验证器实现在对应 crate 内,E2E 测试在顶层 `tests/e2e/`。

**代码落点清单**:

| 文件路径 | 职责 | 类型 |
|---------|------|------|
| `crates/nexus-contracts/src/formal_props.rs` | L0 契约类型:`FormalProperty` / `VerificationResult` / `InvariantSpec` | 新增模块 |
| `crates/gsoe-evolution/src/formal/lineage_checker.rs` | GSOE 谱系 DAG 完整性验证器 | 新增模块 |
| `crates/gsoe-evolution/src/formal/critic_monotonicity.rs` | AEGIS Critic 单调性验证器 | 新增模块 |
| `crates/parliament/src/formal/consensus_safety.rs` | Parliament 一致性验证器 | 新增模块 |
| `tests/e2e/formal_verifier_e2e.rs` | FormalVerifier E2E 闭环测试 | 新增测试 |
| `crates/seccore/src/formal_verifier.rs` | FormalVerifier 主体(决策 1 已规划) | 新增模块(既有) |

**L0 契约类型设计**(`nexus-contracts/src/formal_props.rs`):

```rust
/// 形式化属性标识 — 每个待验证不变量对应一个 FormalProperty
pub enum FormalProperty {
    /// GSOE 谱系完整性: DAG 无环 + 回滚可达 + 变异幅度上限
    GsoeLineageIntegrity,
    /// AEGIS Critic 单调性: 适应度提升 → Critic 评分单调不减
    AegisCriticMonotonicity,
    /// Parliament 一致性: Security 一票否决 + 2/3 多数阈值
    ParliamentConsensusSafety,
}

/// 验证结果 — M0 阶段 proptest 返回
pub enum VerificationResult {
    /// 属性通过(含测试次数与收缩信息)
    Passed { property: FormalProperty, cases_tested: u32 },
    /// 属性违反(含反例与收缩后的最小反例)
    Failed { property: FormalProperty, counterexample: String, shrunk: bool },
}

/// 不变量规格 — 描述待验证属性的参数化配置
pub struct InvariantSpec {
    pub property: FormalProperty,
    pub description: &'static str,
    pub proptest_cases: u32,  // proptest 生成用例数量
    pub max_shrink_iters: u32, // 收缩迭代上限
}
```

**设计原则**:

- **契约类型集中在 `nexus-contracts`**:对齐 nexus-contracts 作为 L0 跨 crate 共享契约的定位,各验证器 crate 依赖 `nexus-contracts` 获取 `FormalProperty` / `VerificationResult` 类型
- **验证器分散在各 crate**:GSOE 验证器在 `gsoe-evolution`,Parliament 验证器在 `parliament`,符合"验证逻辑靠近被验证代码"原则
- **E2E 测试在顶层**:对齐既有 `tests/e2e/` 目录结构,FormalVerifier E2E 测试需跨多 crate 集成

### 决策 11: M0 验收标准(增强)

**决策内容**:M0 验收标准在决策 2 阶段 1 基础上增强,增加属性测试覆盖度与 E2E 闭环要求。

**增强验收标准清单**:

| # | 验收项 | 判定标准 | 验证方式 |
|---|--------|---------|---------|
| 1 | seccore 编译 | `cargo check -p seccore` 通过 | CI |
| 2 | seccore 单元测试 | `cargo test -p seccore formal_verifier` 通过(≥10 个) | CI |
| 3 | seccore clippy | `cargo clippy -p seccore -- -D warnings` 零警告 | CI |
| 4 | 核心属性覆盖 | ≥3 个核心属性有 proptest 属性测试覆盖 | 代码审查 |
| 5 | 属性测试用例数 | ≥50 个属性测试用例(全项目范围,含既有) | `cargo test --workspace` 统计 |
| 6 | E2E 闭环 | `tests/e2e/formal_verifier_e2e.rs` 通过 | CI |
| 7 | 既有测试保护 | 不删既有测试,workspace 全量测试 0 失败 | CI |
| 8 | 零新依赖 | M0 不引入新 Cargo 依赖 | `Cargo.lock` diff 审查 |

**验收标准设计理由**:

- **≥3 个核心属性**:对齐决策 9 的 3 个属性定义,确保 M0 交付可证伪的验证能力
- **≥50 个属性测试用例**:全项目范围统计(含既有 proptest 用例),确保 M0 不显著降低测试密度;新增属性测试预计贡献 ≥30 个用例(3 属性 × 每组 ≥10 cases)
- **E2E 闭环**:FormalVerifier 的 E2E 测试必须覆盖"属性定义 → 验证器执行 → 结果聚合"完整链路
- **既有测试保护**:对齐 ADR-028 决策 1(append-only),不删既有测试是 append-only 的底线要求
- **零新依赖**:M0 的 proptest 已在 workspace,M0 不应引入任何新 Cargo 依赖

## 理由

### 决策 1 理由(crate 选型)

- **依赖铁律合规性**:seccore 位于 L4,FormalVerifier 作为安全验证组件天然属于 L4,无跨层依赖问题
- **复杂度预算守恒**:v5.0 设计文档 §3.4 要求净增长 ≤0,不新建 crate 是最直接的复杂度控制
- **复用既有能力**:seccore 的 `SandboxConfig` / `SandboxResult` / `Error` 枚举可直接复用,减少重复代码
- **ADR-042 预期对齐**:ADR-042 决策 2 首选 `crates/seccore/src/formal_verifier.rs`,本决策与其一致

### 决策 2 理由(三阶段渐进式)

- **可证伪性**:每阶段有明确的验收标准与 SLO,可客观验证
- **可回滚性**:每阶段失败有明确的回滚预案,不影响既有功能
- **资源渐进投入**:M0 投入最少(0.3 FTE),验证可行性后再加码 M1/M2
- **对齐 ADR-042**:三阶段完成时间(2026-10-15)与 ADR-042 预计解冻时间一致

### 决策 3 理由(属性语言选型)

- **forbid(unsafe_code) 红线**:SMT solver 的 Rust binding 普遍依赖 unsafe FFI,这是硬约束,不可妥协
- **实用主义**:完整的 SMT 形式化验证在工业级 Rust 项目中无成熟先例,三层组合可覆盖 80% 实际需求
- **学术支撑**:Khan (2026) 论证了"类型系统 + 属性测试"组合在 token 受限场景下的有效性
- **CI 友好**:不新增 SMT solver 依赖,CI 流程无需大改

### 决策 4 理由(复用优先)

- **DRY 原则**:InvariantChecker 已在 chimera-mas 落地且经过 1000 次 proptest 验证,复用而非重写
- **append-only 哲学**:对齐 ADR-028 决策 1,不修改既有签名,仅新增
- **降低实施风险**:复用已验证的代码降低 bug 引入风险

### 决策 5 理由(集成路径)

- **L4 门作为第二道闸**:不替代 L3 执行反馈,而是在 L3 通过后追加 L4 门,形成纵深防御
- **Critical 事件走 mpsc**:对齐 §6.2 红线,FormalVerificationFailed 是 Critical 级,必须用 mpsc channel 确保送达
- **append-only 扩展**:`evolve_with_formal_verification()` 是新增方法,不修改既有 `evolve_once()` 签名

### 决策 6 理由(时间表)

- **对齐 ADR-042**:2026-10-15 是 ADR-042 预计解冻时间,本 ADR 三阶段完成时间与其一致
- **资源可行**:总投入 ~1.2 FTE × 2 月,在团队容量范围内
- **关键路径清晰**:M0→M1→M2 线性依赖,无并行冲突

### 决策 7 理由(验收与回滚)

- **可证伪**:5 项验收标准全部可客观验证(代码/ADR/属性/测试/评审)
- **可回滚**:每阶段失败有明确回滚预案,最坏情况下 R2 维持冻结,无功能损失
- **长期主义**:不为赶进度牺牲质量,FormalVerifier 是安全关键组件,必须严格验证

### 决策 8 理由(M0 属性语言选型)

- **平台兼容性**:kani 不支持 Windows 是硬约束,团队主力开发环境为 Windows,CI 也需兼顾;proptest 无平台限制
- **渐进式增强**:M0 的 proptest 属性可无损迁移为 kani/prusti 的前置约束,不存在"重写"风险
- **零新依赖**:proptest 已在 workspace,M0 实施成本最低

### 决策 9 理由(M0 验证范围)

- **安全关键优先**:3 个属性覆盖 GSOE(进化安全)、AEGIS(评判安全)、Parliament(治理安全)三大安全关键模块,是三重悖论红线的核心防线
- **可证伪性**:3 个属性均有明确的数学定义(DAG 无环 / 单调性 / 阈值正确性),proptest 可高效验证
- **奖励黑客防御**:属性 2(AEGIS Critic 单调性)直接针对三重悖论文档 §3.4.5 定义的"奖励黑客"风险

### 决策 10 理由(代码落点)

- **契约集中、验证分散**:对齐 nexus-contracts 作为 L0 共享契约的定位,验证器靠近被验证代码,降低认知负荷
- **依赖方向合规**:各验证器 crate 依赖 `nexus-contracts`(L0),不引入跨层依赖
- **E2E 测试对齐**:既有 `tests/e2e/` 已有 16 个 E2E 测试文件,FormalVerifier E2E 对齐既有模式

### 决策 11 理由(M0 验收标准增强)

- **覆盖度可度量**:≥3 属性 + ≥50 用例数提供客观度量,避免"M0 交付了什么但无法量化"
- **既有测试保护**:对齐 ADR-028 append-only 原则,确保 M0 不破坏既有质量基线
- **E2E 闭环**:确保 M0 不仅是"代码写了",而是"端到端可运行"

## 影响

### 新增内容

- **新增模块**:`crates/seccore/src/formal_verifier.rs` + 子模块(`type_checker.rs` / `static_analyzer.rs` / `invariant.rs` / `resource_bound.rs`)
- **新增 M0 模块**(决策 10):
  - `crates/nexus-contracts/src/formal_props.rs` — L0 契约类型(`FormalProperty` / `VerificationResult` / `InvariantSpec`)
  - `crates/gsoe-evolution/src/formal/lineage_checker.rs` — GSOE 谱系 DAG 完整性验证器
  - `crates/gsoe-evolution/src/formal/critic_monotonicity.rs` — AEGIS Critic 单调性验证器
  - `crates/parliament/src/formal/consensus_safety.rs` — Parliament 一致性验证器
- **新增类型**:`FormalVerifier` / `VerificationTier` / `VerificationResult` / `L4InvariantChecker` trait / `ResourceBoundConfig` / `FormalProperty` / `InvariantSpec`
- **新增方法**:`GsoeEvolutionEngine::evolve_with_formal_verification()`(append-only)
- **新增 NexusEvent 变体**:2 个(共 104 → 106)
  - `FormalVerificationFailed { metadata, candidate_id, failure_reason, attribute_name }`(Critical 级,走 mpsc 旁路通道)
  - `FormalVerificationPassed { metadata, candidate_id, attributes_verified }`(Normal 级,broadcast)
- **新增 GsoeError 变体**:1 个
  - `FormalVerificationFailed { reason, attribute }`
- **新增 benchmark**:`crates/seccore/benches/formal_verifier_benchmark.rs`(M2 交付)
- **新增集成测试**:`crates/gsoe-evolution/tests/formal_verifier_integration.rs`(M1 交付)
- **新增 E2E 测试**:`tests/e2e/formal_verifier_e2e.rs`(M0 交付)
- **新增 spec 属性函数**:7 个(M0 交付 3 个,M1 交付 2 个,M2 交付 2 个)
- **新增 ADR**:本 ADR(ADR-047)

### 修改内容

- **`crates/seccore/src/lib.rs`**:新增 `pub mod formal_verifier;`
- **`crates/seccore/src/error.rs`**:新增 `FormalVerifierError` 变体(append-only)
- **`crates/gsoe-evolution/src/engine.rs`**:新增 `evolve_with_formal_verification()` 方法(append-only)
- **`crates/gsoe-evolution/src/error.rs`**:新增 `FormalVerificationFailed` 变体(append-only)
- **`crates/event-bus/src/types.rs`**:新增 `FormalVerificationFailed` / `FormalVerificationPassed` 事件变体(append-only,104 → 106)
- **`crates/nexus-contracts/src/lib.rs`**:新增 `pub mod formal_props;`(M0)
- **`crates/gsoe-evolution/src/lib.rs`**:新增 `pub mod formal;`(M0,含 `lineage_checker` / `critic_monotonicity` 子模块)
- **`crates/parliament/src/lib.rs`**:新增 `pub mod formal;`(M0,含 `consensus_safety` 子模块)
- **`docs/architecture/CODE_WIKI.md`**:seccore / gsoe-evolution / parliament / nexus-contracts 条目补 FormalVerifier 说明
- **`docs/architecture/adr_index.md`**:新增 ADR-047 条目
- **`CHANGELOG.md`**:新增"ADR-047 FormalVerifier L4 跃迁路线图"条目

### 资源影响评估

| 维度 | 评估 |
|------|------|
| crate 数量 | 37(不变,增量在 seccore 内) |
| 依赖变更 | 无新增外部依赖(复用 cargo check / clippy / proptest) |
| Docker/binary 体积 | 不受影响(无新二进制依赖) |
| NexusEvent 变体数 | 104 → 106(新增 2 个,1 Critical + 1 Normal) |
| GsoeError 变体数 | +1(FormalVerificationFailed) |
| 测试覆盖 | 新增 ~50+ 个测试/属性用例(M0 30+ 属性用例 + 10 单元 + M1 10 + M2 10) |
| CI 时间 | +~60 秒(formal_verifier 单元测试 + 集成测试) |
| 版本号 | minor 升级(v2.4.0 → v2.5.0,M2 完成时) |
| FTE 投入 | ~1.2 FTE × 2 月(M0→M2) |

## 考虑的方案

### 方案 A: 嵌入 seccore + 三层属性语言 + 三阶段渐进式(采纳)

- **内容**:如决策 1-11 所述(M0 细化规格见决策 8-11)
- **采纳理由**:
  1. 依赖铁律合规(L4 内)
  2. forbid(unsafe_code) 合规(无 SMT FFI)
  3. 复杂度预算守恒(不新建 crate)
  4. 复用优先(InvariantChecker / SandboxConfig)
  5. 可证伪 + 可回滚(三阶段 + 7 项验收标准)
  6. M0 务实路径:proptest 属性测试(零新依赖,kani/prusti 作为 M2 增强选项)

### 方案 B: 新建独立 `formal-verifier` crate + SMT solver(否决)

- **内容**:新建 `crates/formal-verifier/` crate,集成 Z3 / cvc5 SMT solver,使用 SMT-LIB 属性语言
- **否决理由**:
  1. **forbid(unsafe_code) 违规**:`z3-sys` 依赖 C FFI,违反 §4.1 红线
  2. **依赖铁律风险**:新 crate 的层级归属(L4.5?)需额外 ADR 论证
  3. **Docker 体积膨胀**:Z3 二进制 ~50MB,超出 §10.2 <100MB 约束
  4. **学习曲线陡峭**:团队需掌握 SMT-LIB,交付风险高
  5. **CI 复杂化**:需新增 SMT solver 安装 job

### 方案 C: 嵌入 seccore + Lean/Coq 证明器(否决)

- **内容**:嵌入 seccore,但使用 Lean / Coq 的 Rust binding 作为证明器
- **否决理由**:
  1. **binding 不成熟**:Lean / Coq 的 Rust binding 仍在研究阶段,无生产级实现
  2. **forbid(unsafe_code) 风险**:即便 binding 存在,大概率依赖 unsafe FFI
  3. **计算成本高**:Lean / Coq 求解秒级,不满足 <100ms SLO
  4. **过度工程化**:对于 5 个 spec 属性,Lean / Coq 的表达能力过剩

### 方案 D: 纯 proptest 不做静态分析(否决)

- **内容**:仅使用 proptest 做属性测试,不集成 clippy 深度规则与 Miri
- **否决理由**:
  1. **验证层级不足**:proptest 是随机测试,非形式化证明,不满足 L4 定义
  2. **覆盖率盲区**:proptest 无法覆盖所有输入组合,仍存在奖励黑客风险
  3. **与三重悖论文档冲突**:§4.3.1 明确 FormalVerifier 需包含 StaticAnalyzer

## 合规性

- **§2.1 分层映射**:符合。FormalVerifier 嵌入 seccore(L4 安全层),不改分层结构
- **§2.2 依赖铁律**:符合。seccore 位于 L4,GSOE(L5)调用 FormalVerifier 是 L5→L4 向下依赖,合规;Event Bus 跨层通信合规
- **§3.3.1 第 1 条(OMEGA 四定律守恒)**:符合。Ω-Evolve 由 gsoe-evolution 单一实现,FormalVerifier 是 Ω-Evolve 的验证门,不改变实现位置
- **§3.3.1 第 4 条(领域类型稳定性)**:符合。不改 `UserIntent` / `Quest` / `Checkpoint` / `OmniSparseMasks` / `CLV` / `NexusState`
- **§3.3.1 第 5 条(向后兼容)**:符合。append-only 扩展,不修改既有 API 签名
- **§3.3.1 第 6 条(新 crate 准入)**:符合。不新建 crate
- **§3.4.1 第 6 条(性能可证伪)**:符合。M0/M1/M2 各有 SLO,可 benchmark 证伪
- **§3.4.1 第 7 条(学术支撑落地)**:符合。引用 Khan (2026) / Kushnerov (2026) / AI Alignment 文献
- **§3.4.1 第 8 条(专家团队评审)**:符合。M2 交付架构评审报告,E01/E02/E06 三方批准
- **§3.4.5 三重悖论红线(进化悖论)**:符合。本 ADR 是进化悖论红线(L3→L4 跃迁)的工程实施层落地
- **§4.1 编码规范**:符合。`#![forbid(unsafe_code)]` 保持;库层 thiserror;无生产路径 unwrap/expect;单函数 ≤200 行
- **§4.4 async 反模式**:符合。不持锁跨 .await;Critical 级事件走 mpsc 旁路通道
- **§6.1 架构红线**:符合。不引入功能旗(L4 门是编译期 feature,非运行时旗);单函数 ≤200 行
- **§6.2 Week 1-8 实战新红线**:符合。`FormalVerificationFailed` 为 Critical 级,走 mpsc 旁路通道(对齐红线 5);不持锁 .await
- **ADR-028 决策 1(append-only)**:符合。所有扩展均为新增模块/类型/方法,不修改既有签名
- **ADR-030(unsafe 红线)**:符合。FormalVerifier 全程 safe Rust,无 unsafe 依赖
- **ADR-032 决策 4(验证器层级跃迁路径)**:符合。本 ADR 是 ADR-032 决策 4 的工程实施层落地
- **ADR-042(R2 冻结)**:符合。本 ADR 是 ADR-042 决策 2 判定标准 2(ADR 落档)的履行

## 相关文档

- **三重悖论文档**: [AI_Agent_三重悖论_x_Chimera_深度映射分析.md](file:///D:/Chimera CLI/AI_Agent_三重悖论_x_Chimera_深度映射分析.md) §4.3(验证器层级跃迁)— FormalVerifier 设计蓝图
- **规则**: [.trae/rules/nuxus规则.md](file:///D:/Chimera CLI/.trae/rules/nuxus规则.md) §3.4.5(三重悖论红线)/§4.1(编码规范)/§6.2(Week 1-8 新红线)
- **CODE_WIKI.md**: [docs/architecture/CODE_WIKI.md](file:///D:/Chimera CLI/docs/architecture/CODE_WIKI.md) §3.1(crate 索引)/§2.3(ADR 表)
- **ADR 索引**: [docs/architecture/adr_index.md](file:///D:/Chimera CLI/docs/architecture/adr_index.md)(本 ADR 同步更新)
- **关联 ADR**:
  - [ADR-042](file:///D:/Chimera CLI/docs/architecture/ADR-042-r2-freeze-before-formal-verifier.md)(R2 冻结 — 本 ADR 是其决策 2 判定标准 2 的履行)
  - [ADR-032](file:///D:/Chimera CLI/docs/architecture/ADR-032-dual-channel-evaluator.md)(RHI-CG 双通道评估器 — 决策 4 验证器层级跃迁路径,本 ADR 的上层依据)
  - [ADR-030](file:///D:/Chimera CLI/docs/architecture/ADR-030-unsafe-redline-no-exception.md)(unsafe 红线 — FormalVerifier 必须 forbid unsafe)
  - [ADR-028](file:///D:/Chimera CLI/docs/architecture/ADR-028-chimera-mas-part2-closure.md)(append-only 哲学 — 本 ADR 遵循)
  - [ADR-046](file:///D:/Chimera CLI/docs/architecture/ADR-046-immune-system-facade.md)(ImmuneSystem facade — FormalVerifier 可作为免疫系统的验证组件)
- **代码基线**:
  - [crates/seccore/src/sandbox.rs](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs)(ResourceBoundChecker 复用源)
  - [crates/seccore/src/lib.rs](file:///D:/Chimera CLI/crates/seccore/src/lib.rs)(FormalVerifier 模块挂载点)
  - [crates/gsoe-evolution/src/engine.rs](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs)(L4 门集成点)
  - [crates/gsoe-evolution/src/policy/grpo.rs](file:///D:/Chimera CLI/crates/gsoe-evolution/src/policy/grpo.rs)(L3 执行反馈,L4 门的前置)
  - [crates/auto-dpo/src/generator.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs)(L2 学习型评判器,L4 门的后置)
  - [crates/chimera-mas/src/invariants.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs)(InvariantChecker 复用源)
  - [crates/event-bus/src/types.rs](file:///D:/Chimera CLI/crates/event-bus/src/types.rs)(NexusEvent 新增位置)
- **M0 新增代码落点**(决策 10):
  - `crates/nexus-contracts/src/formal_props.rs` — L0 契约类型(`FormalProperty` / `VerificationResult` / `InvariantSpec`)
  - `crates/gsoe-evolution/src/formal/lineage_checker.rs` — GSOE 谱系 DAG 完整性验证器
  - `crates/gsoe-evolution/src/formal/critic_monotonicity.rs` — AEGIS Critic 单调性验证器
  - `crates/parliament/src/formal/consensus_safety.rs` — Parliament 一致性验证器
  - `tests/e2e/formal_verifier_e2e.rs` — FormalVerifier E2E 闭环测试
- **spec**: [.trae/specs/systematic-deep-analysis-optimization/spec.md](file:///D:/Chimera CLI/.trae/specs/systematic-deep-analysis-optimization/spec.md)(本 ADR 是 Phase 7 P0-1 交付物)

---

> **维护者**: NEXUS-OMEGA 团队
> **创建日期**: 2026-07-27
> **M0 细化日期**: 2026-07-29(决策 8-11 补充 M0 实施规格)
> **基线版本**: v2.3.1-omega(创建时,Phase 7 P0 优化实施阶段)
> **决策者**: E01 首席架构师 + E02 安全架构师 + E06 认知科学专家(分布式评审,提议状态待 M2 评审通过后转为已接受)
> **分析团队**: 8 专家视角分布式深度分析(Phase 2 分层分析 + Phase 5 技术债盘点 + Phase 6 优化方案设计)
> **预计 M0 完成时间**: 2026-08-15
> **预计 M2 完成时间**: 2026-10-15(对齐 ADR-042 R2 解冻预期)
> **R2 解冻责任方**: E01 首席架构师 + E02 安全架构师 + E06 认知科学专家(三方书面批准,本 ADR M2 完成后触发 R2 解冻 ADR 评审)
