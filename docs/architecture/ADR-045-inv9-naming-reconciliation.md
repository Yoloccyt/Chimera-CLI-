# ADR-045: INV-9 委托图无环不变量命名调和

## 状态

已批准 (Accepted) (2026-07-26)

> **状态说明**: 本 ADR 于 2026-07-26 由 E01 首席架构师创建并批准,用于调和 INV-9(委托图无环不变量)在 v5.0 设计文档、ADR-032 决策 2、ADR-044 决策 8 与 P3-W11.3 实际代码之间的命名/接口/proptest 规格/错误类型/输入类型偏差。本 ADR 是 ADR-044 通道 B 实施的**前置约束**——ADR-044 决策 8 明确"通道 B 实施前必须先完成 ADR-045 命名调和"。属 append-only 调和性决策,不修改 ADR-027 / ADR-028 / ADR-032 / ADR-044 的既有裁决结论,仅澄清 INV-9 的权威命名与接口。

## 背景

- **v5.0 设计文档 §5.2 九项裁决第 8 项**([NEXUS-OMEGA_v5.0_系统性完整设计文档.md:276](file:///D:/Chimera CLI/NEXUS-OMEGA_v5.0_系统性完整设计文档.md))明确指出:

  > | DAG 无环靠运行时检查 | delegation.rs 层级递归 + 深度常量 | **InvariantChecker 扩展 INV-9:委托图无环** | 与 INV-7/8 同构编码 + proptest,不引入新框架 |

  即 INV-9 的语义是**委托图无环检查**(输入:委托边列表),与 INV-7(上下文预算界)/ INV-8(归档单调性)同构编码,使用 1000 次 proptest 验证。

- **v5.0 设计文档 §9.3 ReasoningState 七态转移表**([NEXUS-OMEGA_v5.0_系统性完整设计文档.md:460](file:///D:/Chimera CLI/NEXUS-OMEGA_v5.0_系统性完整设计文档.md))进一步确认:

  > **规格对齐 INV-7/8 的 1000 次 proptest 先例**(chimera-mas invariants.rs),新增 INV-9(委托无环)同规格。

- **ADR-027**(chimera-mas 四象限分工)与 **ADR-028**(Part II 闭环)在 §21.2 定义 INV-7/INV-8/INV-9 三类不变量,其中 INV-9 由 P3-W11.3 落地(2026-07-15),实现于 [crates/chimera-mas/src/invariants.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs)。

- **ADR-032 决策 2**([ADR-032-dual-channel-evaluator.md:39-56](file:///D:/Chimera CLI/docs/architecture/ADR-032-dual-channel-evaluator.md))在 RHI-CG 双通道架构层面提到 INV-9,但将其语义误述为"否决证据充分性",并规划方法签名 `check_inv9_veto_evidence(regression_streak: u32, significance: f64) -> Result<()>`,错误变体 `VetoEvidenceInsufficient`,常量 `VETO_STREAK_THRESHOLD = 3`。这与 v5.0 设计文档 §5.2 第 8 项的"委托图无环"语义不符。

- **ADR-044 决策 8**([ADR-044-rhi-cg-engineering.md:222-243](file:///D:/Chimera CLI/docs/architecture/ADR-044-rhi-cg-engineering.md))继承 ADR-032 的误述,记录"当前 `crates/chimera-mas/src/invariants.rs` 的方法命名为 `check_inv9_veto_evidence`",并期望 ADR-045 将其重命名为 `check_inv9(regression_streak: u32)`,与 INV-7/INV-8 的 `check_inv7` / `check_inv8` 命名一致。但实际代码(P3-W11.3 落地)采用 `check_inv9_delegation_acyclic(edges: &[DelegationEdge])` 签名,与 ADR-044 描述完全不符。

- **P3-W11.3 实施现状**(2026-07-26 核实,本 ADR 创建前):
  - [crates/chimera-mas/src/invariants.rs:368](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs) 实际实现方法 `check_inv9_delegation_acyclic(edges: &[DelegationEdge]) -> Result<()>`,采用 DFS 三色标记法(CLSR §22.3),O(V+E) 时间复杂度
  - [crates/chimera-mas/src/invariants.rs:162-189](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs) 定义输入类型 `DelegationEdge { from: String, to: String }`,含 `new()` 构造器
  - [crates/chimera-mas/src/error.rs:197-201](file:///D:/Chimera CLI/crates/chimera-mas/src/error.rs) 定义错误变体 `MasError::DelegationCycleDetected { cycle_path: Vec<String> }`
  - [crates/chimera-mas/tests/proptest.rs:938-1045](file:///D:/Chimera CLI/crates/chimera-mas/tests/proptest.rs) 含 4 个 INV-9 proptest,显式配置 `ProptestConfig { cases: 1000, .. }`,覆盖 DAG/树/含环图/DAG+回边四类性质
  - [crates/chimera-mas/src/invariants.rs:593-797](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs) 含 13 个 INV-9 单元测试,覆盖空边/单边/线性链/树/自环/2-3 节点环/多连通分量/深度 5 链/DAG 交叉边/环闭合性/重复边
  - 无 `VetoEvidenceInsufficient` 错误变体,无 `VETO_STREAK_THRESHOLD` 常量,无 `check_inv9_veto_evidence` 方法

- 经 E01 首席架构师(15+ 年)分布式深度分析与多轮结构化思考,确认 P3-W11.3 实施遵循 v5.0 设计文档(权威源),偏离 ADR-032 决策 2 的错误描述(非权威源),命名/接口/proptest 规格/错误类型/输入类型五维均与 v5.0 设计文档对齐。本 ADR 正式确认此偏离为**正确实施**,并修正 ADR-032 / ADR-044 的描述偏差。

> **ADR 编号确认**: `docs/architecture/adr_index.md` 现有最大编号为 ADR-044(2026-07-26 核实,本 ADR 创建前)。本 ADR 编号确认为 **ADR-045**,作为下一个连续编号,与既有规划不冲突。

> **与 ADR-027 / ADR-028 的关系**: 本 ADR 是 ADR-027(四象限分工,定义 INV-7/8/9 雏形)与 ADR-028(Part II 闭环,定义 INV-7/8 proptest 1000 次规格)的**实施层调和**——确认 P3-W11.3 落地的 INV-9 实现遵循 ADR-028 决策 3 的"1000 次 proptest 无违反"规格。属 append-only 调和,不修改 ADR-027 / ADR-028 既有决策。

> **与 ADR-032 的关系**: 本 ADR 是 ADR-032 决策 2(通道 B CI 执行门 + INV-9 否决证据充分性)的**命名/语义修正**——ADR-032 决策 2 将 INV-9 误述为"否决证据充分性"(输入 regression_streak),与 v5.0 设计文档 §5.2 第 8 项"委托图无环"(输入 DelegationEdge)冲突。本 ADR 确认 INV-9 的权威语义为"委托图无环",通道 B 的否决证据检查应作为独立逻辑(`CiGate::check_veto_evidence` 或 `VetoStreakChecker`),不与 INV-9 混淆。本 ADR 不修改 ADR-032 决策 2 的"连续 3 次统计显著回归才否决"判定逻辑,仅修正其命名引用。

> **与 ADR-044 的关系**: 本 ADR 是 ADR-044 决策 8(与 ADR-045 INV-9 命名调和的依赖关系)的**前置条件**——ADR-044 决策 8 明确"通道 B 实施前必须先完成 ADR-045 命名调和",否则通道 B 的 CI 执行门接口无法稳定调用。本 ADR 批准后,通道 B 可放心调用 `InvariantChecker::check_inv9_delegation_acyclic(&edges)`,接口稳定。

## 决策

经专家团队多轮结构化思考与多路径交叉验证,对 INV-9 委托图无环不变量的命名调和作出以下 8 项决策:

### 决策 1: INV-9 权威语义确认 — 委托图无环(非否决证据充分性)

确认 INV-9 的权威语义为 **"委托图无环不变量"**(v5.0 设计文档 §5.2 九项裁决第 8 项),非"否决证据充分性"(ADR-032 决策 2 误述)。

**权威源对照**:

| 维度 | v5.0 设计文档 §5.2 第 8 项(权威) | ADR-032 决策 2(误述) |
|------|--------------------------------|----------------------|
| INV-9 语义 | 委托图无环(零循环委托) | 否决证据充分性(连续 3 次统计显著回归) |
| 输入类型 | 委托边列表 `&[DelegationEdge]` | `(regression_streak: u32, significance: f64)` |
| 错误变体 | `DelegationCycleDetected { cycle_path }` | `VetoEvidenceInsufficient { regression_streak, significance }` |
| 检查时机 | 委托派生前(MAS 子系统运行不变量) | 通道 B 否决决策时(RHI-CG 进化回路) |
| 架构层 | L9 Quest(chimera-mas 子系统不变量) | L5 Knowledge(gsoe-evolution 通道 B 逻辑) |
| 1000 次 proptest | 是(v5.0 §9.3 规格) | 否(数值阈值检查,无需 proptest) |

**判定依据**:

1. v5.0 设计文档 §5.2 第 8 项明确将 INV-9 定义为"委托图无环",与 INV-7/INV-8 同构编码("与 INV-7/8 同构编码 + proptest")
2. v5.0 设计文档 §7.4 通道 B 描述"`InvariantChecker(INV-7/8/9)`"作为 CI 执行门的一部分,INV-9 与 INV-7/INV-8 并列,语义同属 MAS 子系统运行不变量
3. v5.0 设计文档 §9.3 明确"规格对齐 INV-7/8 的 1000 次 proptest 先例,新增 INV-9(委托无环)同规格"
4. ADR-028 决策 3 在 Part II 闭环层面定义 INV-7/INV-8 + 1000 次 proptest,P3-W11.3 按 v5.0 设计文档扩展至 INV-9
5. ADR-032 决策 2 创建于 RHI-CG 双通道架构设计阶段,将通道 B 的"否决证据充分性"逻辑误冠以 INV-9 之名,与 v5.0 设计文档冲突

> **本决策不修改 ADR-032 决策 2 的"连续 3 次统计显著回归才否决"判定逻辑**,仅修正其命名引用——通道 B 的否决证据检查应作为独立逻辑,不占用 INV-9 编号。

### 决策 2: 实际代码命名 `check_inv9_delegation_acyclic` 确认为权威命名

确认 [crates/chimera-mas/src/invariants.rs:368](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs) 实际实现的 `check_inv9_delegation_acyclic` 方法命名为 **INV-9 的权威方法名**,弃用 ADR-032 决策 2 的 `check_inv9_veto_evidence` 命名与 ADR-044 决策 8 期望的 `check_inv9` 简称。

**命名模式对齐**:

| 不变量 | 方法名 | 命名模式 |
|--------|--------|---------|
| INV-7 | `check_inv7_context_budget` | `check_inv<N>_<descriptor>` |
| INV-8 | `check_inv8_archive_monotonicity` | `check_inv<N>_<descriptor>` |
| INV-9 | `check_inv9_delegation_acyclic` | `check_inv<N>_<descriptor>` |

**命名模式一致性论证**:

- INV-7/INV-8/INV-9 三者均采用 `check_inv<N>_<descriptor>` 模式,后缀描述不变量的检查对象(context_budget / archive_monotonicity / delegation_acyclic)
- 此模式优于 ADR-044 期望的 `check_inv9` 简称——简称无法自描述检查对象,降低代码可读性
- 弃用 ADR-032 的 `check_inv9_veto_evidence` 命名——该命名混淆了 INV-9(MAS 子系统不变量)与通道 B 否决逻辑(L5 进化回路)

**实际代码方法签名**(权威源):

```rust
/// INV-9 — 委托图无环不变量检查(P3-W11.3 §21.2)
///
/// 验证委托关系构成的有向图无环,防止循环委托导致递归死循环
/// (§6.2 红线:零循环委托)。
///
/// 算法: DFS 三色标记法(Cormen et al., CLRS §22.3),O(V+E) 时间复杂度
pub fn check_inv9_delegation_acyclic(edges: &[DelegationEdge]) -> Result<()> {
    // 实现见 crates/chimera-mas/src/invariants.rs:368-401
}
```

> **代码权威源**: [crates/chimera-mas/src/invariants.rs:368-401](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs)

### 决策 3: 检查接口确认 — `&[DelegationEdge]` 纯函数关联函数

确认 INV-9 检查接口为 **纯函数关联函数**(非 `&self` 方法),输入 `&[DelegationEdge]`,返回 `Result<(), MasError>`。

**接口特性**:

| 特性 | INV-9 实现 | 设计文档对齐 |
|------|-----------|-------------|
| 调用方式 | `InvariantChecker::check_inv9_delegation_acyclic(&edges)` | 关联函数(非 `&self`) |
| 输入类型 | `&[DelegationEdge]`( borrowed slice) | v5.0 §5.2 第 8 项"委托图无环" |
| 返回类型 | `Result<(), MasError>` | `Ok(())` 或 `Err(DelegationCycleDetected)` |
| 副作用 | 无(纯函数,不持状态,不发布事件) | INV-7/INV-8 同模式 |
| 状态依赖 | 无(无 `&self`,无需实例化 `InvariantChecker`) | INV-7/INV-8 同模式 |
| 失败处理 | 返回错误,调用方负责发布 Critical 事件(§6.2) | INV-7/INV-8 同模式 |

**调用示例**(对应设计文档预期接口):

```rust
use chimera_mas::invariants::{DelegationEdge, InvariantChecker};

// 委托派生前检查委托图无环
let edges = vec![
    DelegationEdge::new("root-agent", "main-agent-1"),
    DelegationEdge::new("main-agent-1", "sub-agent-a"),
    DelegationEdge::new("main-agent-1", "sub-agent-b"),
];
match InvariantChecker::check_inv9_delegation_acyclic(&edges) {
    Ok(()) => {
        // 委托图无环,允许继续委托派生
    }
    Err(MasError::DelegationCycleDetected { cycle_path }) => {
        // 检测到环,发布 Critical 事件(§6.2 红线:走 mpsc 旁路通道)
        // 阻断涉环 Agent 的进一步派生
    }
}
```

> **与 INV-7/INV-8 接口一致性**: 三者均为 `InvariantChecker::check_inv<N>_<descriptor>(...)` 关联函数,纯函数无副作用,失败返回 `MasError` 变体。本 ADR 确认 INV-9 接口与 INV-7/INV-8 同构,符合 v5.0 设计文档 §5.2 第 8 项"与 INV-7/8 同构编码"要求。

### 决策 4: proptest 规格确认 — 1000 次四性质属性测试

确认 INV-9 的 proptest 规格为 **1000 次 cases,覆盖四性质**(无环/全可达/无意外循环/回边检测),对齐 v5.0 设计文档 §9.3"规格对齐 INV-7/8 的 1000 次 proptest 先例"与 ADR-028 决策 3"1000 次属性测试"。

**proptest 实施现状**(权威源):

| 测试名 | 性质 | 输入策略 | 验证断言 |
|--------|------|---------|---------|
| `inv9_dag_always_acyclic` | 无环 | `arb_dag_edges()`(仅 i < j 边,拓扑序=索引序) | DAG 必返回 `Ok(())` |
| `inv9_tree_always_acyclic_and_reachable` | 全可达 | `arb_tree_edges()`(每节点 i≥1 父从 0..i 选) | 树必返回 `Ok(())` |
| `inv9_cyclic_graph_always_rejected` | 无意外循环 | `arb_cyclic_edges()`(完整环 n0→n1→...→n0) | 必返回 `Err(DelegationCycleDetected)`,环路径首尾相同 |
| `inv9_dag_plus_back_edge_always_rejected` | 回边检测 | `arb_dag_plus_back_edge()`(DAG + n0↔n1 回边) | 必返回 `Err(DelegationCycleDetected)`,环路径首尾相同 |

**配置证据**([crates/chimera-mas/tests/proptest.rs:938-942](file:///D:/Chimera CLI/crates/chimera-mas/tests/proptest.rs)):

```rust
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1000,
        .. ProptestConfig::default()
    })]

    /// INV-9 性质 1 — 无环: 任意 DAG, INV-9 检查必通过
    #[test]
    fn inv9_dag_always_acyclic(edges in arb_dag_edges()) { ... }

    /// INV-9 性质 2 — 全可达: 任意树形委托图, INV-9 检查必通过
    #[test]
    fn inv9_tree_always_acyclic_and_reachable(edges in arb_tree_edges()) { ... }

    /// INV-9 性质 3a — 无意外循环: 任意含环委托图, INV-9 检查必拒绝
    #[test]
    fn inv9_cyclic_graph_always_rejected(edges in arb_cyclic_edges()) { ... }

    /// INV-9 性质 3b — 无意外循环: DAG + 回边, INV-9 检查必拒绝
    #[test]
    fn inv9_dag_plus_back_edge_always_rejected(edges in arb_dag_plus_back_edge()) { ... }
}
```

**对齐验证**:

| 规格 | v5.0 设计文档要求 | ADR-028 决策 3 要求 | P3-W11.3 实际实现 | 对齐状态 |
|------|------------------|--------------------|------------------|---------|
| cases 数 | 1000 次(§9.3) | 1000 次 | `cases: 1000` | ✅ 一致 |
| 测试维度 | 四性质(无环/全可达/无意外循环/回边) | INV-7/8 同构 | 4 个 proptest 覆盖四性质 | ✅ 一致 |
| 测试语法 | block-named(§4.1) | block-named | `fn name(edges in strategy) { ... }` | ✅ 一致 |
| 覆盖范围 | DAG/树/含环图/DAG+回边 | 同 INV-7/8 规格扩展 | 四策略 `arb_dag_edges` / `arb_tree_edges` / `arb_cyclic_edges` / `arb_dag_plus_back_edge` | ✅ 一致 |
| 测试位置 | `tests/proptest.rs` | `tests/proptest.rs` | `crates/chimera-mas/tests/proptest.rs:833-1045` | ✅ 一致 |

> **本决策确认 INV-9 的 proptest 规格已对齐 v5.0 设计文档与 ADR-028 决策 3 的 1000 次要求,无规格差距。**

### 决策 5: 错误类型确认 — `MasError::DelegationCycleDetected`

确认 INV-9 的错误变体为 **`MasError::DelegationCycleDetected { cycle_path: Vec<String> }`**,弃用 ADR-032 决策 2 的 `VetoEvidenceInsufficient` 变体(未实现,且语义不符)。

**错误变体定义**(权威源 [crates/chimera-mas/src/error.rs:197-201](file:///D:/Chimera CLI/crates/chimera-mas/src/error.rs)):

```rust
/// 委托图有环 — INV-9 委托图无环不变量违反(P3-W11.3 §21.2)
///
/// 触发场景:`InvariantChecker::check_inv9_delegation_acyclic()` 检测到委托关系
/// 构成的有向图存在环(DFS 三色标记法遇到 GRAY 节点 = 回边)。
///
/// 处理策略:拒绝继续委托,调用方应发布 Critical 事件(§6.2 红线)并阻断
/// 涉环 Agent 的进一步派生,防止递归委托死循环(§6.2 红线:零循环委托)。
#[error("Delegation cycle detected (INV-9): cycle_path = {cycle_path:?}")]
DelegationCycleDetected {
    /// 检测到的环路径(Agent ID 序列,首尾相同构成环,如 ["A", "B", "C", "A"])
    cycle_path: Vec<String>,
}
```

**错误类型对齐验证**:

| 维度 | v5.0 设计文档 | ADR-027/028 | P3-W11.3 实际 | 对齐状态 |
|------|--------------|-------------|--------------|---------|
| 变体名 | (未明确) | (未明确,§21.2 仅说"INV-9") | `DelegationCycleDetected` | ✅ 自描述 |
| 字段 | (未明确) | (未明确) | `cycle_path: Vec<String>` | ✅ 携带诊断信息 |
| Display | (未明确) | (未明确) | `"Delegation cycle detected (INV-9): cycle_path = {:?}"` | ✅ 含 INV-9 标识 |
| 严重级别 | Critical(§6.2 零循环委托红线) | Critical | 调用方发布 Critical 事件走 mpsc | ✅ 一致 |
| 处理策略 | 拒绝继续委托 | 拒绝继续委托 | 返回错误,调用方阻断涉环 Agent 派生 | ✅ 一致 |

**MasError 变体总数**: 34 个(P3-W11.3 落地后,含 `DelegationCycleDetected`,见 [crates/chimera-mas/src/error.rs:559-675](file:///D:/Chimera CLI/crates/chimera-mas/src/error.rs) 静态断言 `variants.len() >= 34`)。

> **本决策弃用 ADR-032 决策 2 的 `VetoEvidenceInsufficient` 变体**——该变体未在 P3-W11.3 实现,且其语义(否决证据不足)属于通道 B 逻辑,不属于 INV-9(MAS 子系统不变量)。通道 B 实施时如需"否决证据不足"错误,应作为 `AutoDpoError` 或 `GsoeError` 的变体,不应占用 `MasError` 命名空间。

### 决策 6: 输入类型确认 — `DelegationEdge { from, to }`

确认 INV-9 的输入类型为 **`DelegationEdge`**(独立类型,非复用 `AgentTask`),含 `from: String` 与 `to: String` 两字段,表示有向委托边。

**类型定义**(权威源 [crates/chimera-mas/src/invariants.rs:161-189](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs)):

```rust
/// 委托边 — INV-9 委托图无环检查的输入单元(P3-W11.3 §21.2)
///
/// 表示一条有向委托关系: `from`(委托方/父 Agent) → `to`(被委托方/子 Agent)。
/// 多条 `DelegationEdge` 构成委托有向图,INV-9 要求该图无环。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DelegationEdge {
    /// 委托方 Agent ID(父 Agent,委托发起方)
    pub from: String,
    /// 被委托方 Agent ID(子 Agent,委托接收方)
    pub to: String,
}

impl DelegationEdge {
    /// 创建新的委托边
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self { from: from.into(), to: to.into() }
    }
}
```

**设计决策论证**(已落地实现):

| 设计决策 | 选择 | 理由 |
|---------|------|------|
| 独立类型 vs 复用 `AgentTask` | 独立 `DelegationEdge` | `AgentTask` 含 `parent_agent_id: Option<String>`,但被委托方 Agent ID 在执行时才生成(`{parent}::sub::{task_id}`),不存在于 `AgentTask` 中。独立 `DelegationEdge` 显式表达 from→to 有向边,语义清晰 |
| `String` vs `&str` | `String` owned | 纯函数接口需 `'static` 生命周期,`String` owned 避免调用方维护引用生命周期。构造开销可接受(委托边数量受 `MAX_AGENT_DEPTH=5` 约束) |
| `pub` 字段 vs getter | `pub` 字段 | 纯数据类型,无不变量需保护,`pub` 字段简化访问 |
| `new(impl Into<String>, impl Into<String>)` | `impl Into<String>` | 接受 `&str` / `String` / `String` 字面量,API 友好 |

**对齐验证**:

| 维度 | v5.0 设计文档 | ADR-027/028 | P3-W11.3 实际 | 对齐状态 |
|------|--------------|-------------|--------------|---------|
| 类型名 | (未明确,泛指"委托边") | (未明确) | `DelegationEdge` | ✅ 自描述 |
| 字段 | (未明确) | (未明确) | `from: String` + `to: String` | ✅ 显式有向边 |
| 构造器 | (未明确) | (未明确) | `new(impl Into<String>, impl Into<String>)` | ✅ API 友好 |
| Trait 派生 | (未明确) | (未明确) | `Debug + Clone + PartialEq + Eq + Hash` | ✅ 可序列化/可比较/可哈希 |
| 位置 | `crates/chimera-mas/src/invariants.rs` | 同 | `crates/chimera-mas/src/invariants.rs:161-189` | ✅ 一致 |

### 决策 7: ADR-032 / ADR-044 描述修正 — INV-9 命名引用更新

修正 ADR-032 决策 2 与 ADR-044 决策 8 中对 INV-9 命名的错误描述,统一指向本 ADR 确认的权威命名。

**ADR-032 决策 2 描述修正**(本 ADR 不修改 ADR-032 文档本身,仅记录修正):

| ADR-032 决策 2 原描述 | 本 ADR 修正后描述 |
|---------------------|------------------|
| "新增 INV-9 不变量(否决证据充分性)" | "通道 B 否决证据检查(独立逻辑,非 INV-9)" |
| `check_inv9_veto_evidence(regression_streak, significance)` | `check_inv9_delegation_acyclic(edges: &[DelegationEdge])`(INV-9 真实命名) |
| `VetoEvidenceInsufficient { regression_streak, significance }` | (未实现,作废) |
| `VETO_STREAK_THRESHOLD: u32 = 3` | (未实现,通道 B 实施时作为 `CiGate` 内部常量) |
| "通道 B 调用 `InvariantChecker::check_inv9_veto_evidence`" | "通道 B 调用 `InvariantChecker::check_inv9_delegation_acyclic` 检查 MAS 委托图无环 + `CiGate::check_veto_evidence` 检查否决证据(独立两步)" |

**ADR-044 决策 8 描述修正**(本 ADR 不修改 ADR-044 文档本身,仅记录修正):

| ADR-044 决策 8 原描述 | 本 ADR 修正后描述 |
|---------------------|------------------|
| "当前 `crates/chimera-mas/src/invariants.rs` 的方法命名为 `check_inv9_veto_evidence`" | "当前 `crates/chimera-mas/src/invariants.rs` 的方法命名为 `check_inv9_delegation_acyclic`(P3-W11.3 落地)" |
| "ADR-045 规划将其重命名为 `check_inv9`,与 INV-7/INV-8 的 `check_inv7` / `check_inv8` 命名一致" | "ADR-045 确认 `check_inv9_delegation_acyclic` 为权威命名,与 INV-7/INV-8 的 `check_inv7_context_budget` / `check_inv8_archive_monotonicity` 命名模式(`check_inv<N>_<descriptor>`)一致" |
| `let inv9_result = InvariantChecker::check_inv9(regression_streak, ...)` | `let inv9_result = InvariantChecker::check_inv9_delegation_acyclic(&delegation_edges)` |
| "通道 B 实施前必须先完成 ADR-045 命名调和" | (本 ADR 批准后,通道 B 可放心调用,接口稳定) |

> **修正原则**: 本 ADR 不修改 ADR-032 / ADR-044 文档本身(append-only 原则),仅记录命名修正。后续读者查阅 ADR-032 / ADR-044 时,应以本 ADR 为权威命名源。

### 决策 8: 通道 B 调用接口稳定确认 — P5.2 可放心调用

确认 [crates/chimera-mas/src/invariants.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs) 的 INV-9 接口稳定,P5.2 通道 B 实施时可放心调用 `InvariantChecker::check_inv9_delegation_acyclic(&edges)`,无需等待进一步重命名。

**通道 B 调用 INV-9 的预期路径**(P5.2 实施时落地):

```rust
use chimera_mas::invariants::{DelegationEdge, InvariantChecker};
use chimera_mas::MasError;

/// 通道 B CI 执行门 — 调用 INV-9 检查 MAS 委托图无环
///
/// 对应 ADR-044 决策 5(CiGate trait)+ 决策 8(与 ADR-045 命名调和依赖)
pub fn check_inv9_in_ci_gate(delegation_edges: &[DelegationEdge]) -> Result<(), MasError> {
    // 调用 INV-9 检查 MAS 子系统委托图无环
    // 接口稳定,无需等待重命名(本 ADR 决策 2 确认)
    InvariantChecker::check_inv9_delegation_acyclic(delegation_edges)
}

/// 通道 B 否决证据检查 — 独立于 INV-9 的 CiGate 内部逻辑
///
/// 对应 ADR-032 决策 2 的"连续 3 次统计显著回归"判定
/// 本 ADR 决策 1 将其从 INV-9 中拆分,作为独立逻辑
pub fn check_veto_evidence(regression_streak: u32, significance: f64) -> Result<(), GsoeError> {
    const VETO_STREAK_THRESHOLD: u32 = 3;
    const SIGNIFICANCE_THRESHOLD: f64 = 0.05;
    if regression_streak >= VETO_STREAK_THRESHOLD && significance < SIGNIFICANCE_THRESHOLD {
        Ok(())  // 证据充分,允许否决
    } else {
        Err(GsoeError::VetoEvidenceInsufficient { regression_streak, significance })
    }
}
```

**接口稳定性保证**:

1. **方法名稳定**: `check_inv9_delegation_acyclic` 已落地,本 ADR 确认为权威命名,未来不会重命名(除非 v6.0 设计文档重新定义 INV-9 语义,需新 ADR)
2. **签名稳定**: `&[DelegationEdge] -> Result<()>` 已落地,符合 INV-7/INV-8 同构模式
3. **错误变体稳定**: `MasError::DelegationCycleDetected` 已落地,无重命名计划
4. **输入类型稳定**: `DelegationEdge` 已落地,无重命名计划
5. **proptest 规格稳定**: 1000 次 cases 四性质已落地,无调整计划

> **本决策解除 ADR-044 决策 8 的"通道 B 实施前必须先完成 ADR-045 命名调和"约束**——本 ADR 批准后,通道 B 可启动实施,接口稳定可放心调用。

## 理由

### 决策 1 理由(INV-9 权威语义确认)

- **设计文档权威性**: v5.0 设计文档 §5.2 九项裁决是 INV-7/8/9 不变量的权威定义源,ADR-032 决策 2 是 RHI-CG 双通道架构层面的引用,不应覆盖 §5.2 的语义定义。本 ADR 遵循"权威源优先"原则,确认 INV-9 语义为"委托图无环"。
- **架构层归属**: INV-7/INV-8/INV-9 三者均属 L9 Quest(chimera-mas 子系统不变量),与 ADR-027 / ADR-028 一致。ADR-032 决策 2 将 INV-9 误置于 L5 Knowledge(gsoe-evolution 通道 B 逻辑),违反架构层归属。
- **同构编码要求**: v5.0 设计文档 §5.2 第 8 项明确"与 INV-7/8 同构编码",要求 INV-9 与 INV-7/INV-8 在方法命名/接口模式/proptest 规格上同构。P3-W11.3 实现遵循此要求,本 ADR 确认此实现为权威。
- **零循环委托红线**: §6.2 红线"零循环委托"要求 MAS 子系统在委托派生前检查委托图无环,这是 INV-9 的本意。通道 B 的"否决证据充分性"是 RHI-CG 进化回路的逻辑,与 MAS 子系统运行不变量无关。

### 决策 2 理由(命名模式对齐)

- **自描述性**: `check_inv9_delegation_acyclic` 命名含 `delegation_acyclic` 后缀,自描述检查对象(委托无环),优于 ADR-044 期望的简称 `check_inv9`(无法自描述)。
- **模式一致性**: INV-7/INV-8/INV-9 三者均采用 `check_inv<N>_<descriptor>` 模式,后缀描述检查对象(context_budget / archive_monotonicity / delegation_acyclic)。此模式已落地,无需调整。
- **弃用 veto_evidence 命名**: ADR-032 决策 2 的 `check_inv9_veto_evidence` 命名混淆了 INV-9(MAS 子系统不变量)与通道 B 否决逻辑(L5 进化回路),弃用此命名可避免概念混淆。
- **代码可读性**: 代码评审者读到 `check_inv9_delegation_acyclic` 即知检查对象是委托图无环,无需查阅文档;读到 `check_inv9_veto_evidence` 则需查阅文档才能理解"veto evidence"含义。

### 决策 3 理由(检查接口确认)

- **纯函数设计**: INV-9 检查无状态,无需实例化 `InvariantChecker`,关联函数便于在派生准入闸 / 归档降级点直接调用。与 INV-7/INV-8 同模式。
- **`&[DelegationEdge]` borrowed slice**: 避免所有权转移,调用方保留 `Vec<DelegationEdge>` 所有权,可继续用于其他用途(如发布事件、记录审计)。
- **`Result<(), MasError>` 返回类型**: 失败时仅返回错误,不修改输入,符合"纯函数无副作用"原则。调用方负责发布 Critical 事件(§6.2 红线:Critical 用 mpsc)。
- **失败处理分离**: 检查逻辑与事件发布分离,符合"单一职责原则"。`check_inv9_delegation_acyclic` 只做检查,事件发布由调用方完成,便于在不同上下文(派生准入闸 / CI 执行门 / 集成测试)复用。

### 决策 4 理由(proptest 1000 次规格)

- **v5.0 §9.3 明确要求**: "规格对齐 INV-7/8 的 1000 次 proptest 先例,新增 INV-9(委托无环)同规格"。P3-W11.3 实现的 1000 次 cases 完全对齐此要求。
- **ADR-028 决策 3 先例**: ADR-028 决策 3 已确立 INV-7/INV-8 的 1000 次 proptest 规格,P3-W11.3 沿用此规格扩展至 INV-9,符合"同构编码"要求。
- **四性质覆盖**: DAG/树/含环图/DAG+回边四类输入策略覆盖 INV-9 的全部语义场景(合法图 + 非法图),属性测试验证"对任意输入,不变量恒成立",而非重复单元测试的固定 case。
- **proptest 1.11+ block-named 语法**: 遵循 §4.1 规范,使用 `fn name(arg in strategy) { body }` 语法,避免 closure 形式的解析失败问题。

### 决策 5 理由(错误类型确认)

- **自描述变体名**: `DelegationCycleDetected` 直接表达"委托图检测到环",优于 ADR-032 的 `VetoEvidenceInsufficient`(需查阅文档才能理解"veto evidence"含义)。
- **`cycle_path` 诊断字段**: 环路径(Agent ID 序列,首尾相同)提供诊断与审计追溯信息,便于调用方发布 Critical 事件时携带详细上下文。
- **Display 含 INV-9 标识**: `"Delegation cycle detected (INV-9): cycle_path = {:?}"` 含 INV-9 标识,便于日志检索与告警图表分类。
- **MasError 变体总数稳定**: P3-W11.3 落地后 MasError 共 34 个变体,静态断言 `variants.len() >= 34` 保持。本 ADR 不新增/删除变体,仅确认 `DelegationCycleDetected` 为权威变体。
- **弃用 `VetoEvidenceInsufficient`**: 该变体未实现,且其语义(否决证据不足)属于通道 B 逻辑,不属于 INV-9。通道 B 实施时如需"否决证据不足"错误,应作为 `AutoDpoError` 或 `GsoeError` 变体,避免占用 `MasError` 命名空间。

### 决策 6 理由(输入类型确认)

- **独立类型而非复用 `AgentTask`**: `AgentTask` 含 `parent_agent_id: Option<String>`,但被委托方 Agent ID 在执行时才生成(`{parent}::sub::{task_id}`),不存在于 `AgentTask` 中。独立 `DelegationEdge` 显式表达 from→to 有向边,语义清晰。
- **`String` owned 而非 `&str`**: 纯函数接口需 `'static` 生命周期,`String` owned 避免调用方维护引用生命周期。构造开销可接受(委托边数量受 `MAX_AGENT_DEPTH=5` 约束,规模有限)。
- **`pub` 字段**: 纯数据类型,无不变量需保护,`pub` 字段简化访问,符合 Rust 惯例。
- **`new(impl Into<String>, impl Into<String>)`**: 接受 `&str` / `String` / `String` 字面量,API 友好,符合 Rust 惯例。
- **Trait 派生**: `Debug + Clone + PartialEq + Eq + Hash` 派生,支持调试/序列化/比较/哈希,便于在测试与生产环境复用。

### 决策 7 理由(ADR-032 / ADR-044 描述修正)

- **append-only 原则**: 本 ADR 不修改 ADR-032 / ADR-044 文档本身(append-only 原则,ADR-028 决策 1 哲学),仅记录命名修正。后续读者查阅 ADR-032 / ADR-044 时,应以本 ADR 为权威命名源。
- **避免破坏性变更**: 直接修改 ADR-032 / ADR-044 文档属于破坏性变更(违反 append-only),本 ADR 通过"调和性决策"方式修正描述,不破坏既有 ADR 的完整性。
- **文档可追溯**: 本 ADR 记录命名修正的全过程,后续读者可通过本 ADR 理解 ADR-032 / ADR-044 的描述偏差如何产生、如何修正。
- **修正范围明确**: 本 ADR 仅修正 ADR-032 决策 2 与 ADR-044 决策 8 中对 INV-9 命名的描述,不修改其他决策(如 ADR-032 决策 2 的"连续 3 次统计显著回归才否决"判定逻辑、ADR-044 决策 5 的 CiGate trait 设计)。

### 决策 8 理由(通道 B 调用接口稳定)

- **解除 ADR-044 决策 8 约束**: ADR-044 决策 8 明确"通道 B 实施前必须先完成 ADR-045 命名调和"。本 ADR 批准后,该约束解除,通道 B 可启动实施。
- **接口五维稳定**: 方法名/签名/错误变体/输入类型/proptest 规格五维均由本 ADR 决策 2-6 确认为权威,P5.2 通道 B 实施时无需等待进一步重命名。
- **通道 B 调用示例**: 本 ADR 决策 8 提供通道 B 调用 INV-9 的预期路径代码示例,P5.2 实施时可参考。
- **否决证据检查独立化**: 本 ADR 决策 1 将通道 B 的"否决证据充分性"逻辑从 INV-9 中拆分,作为独立 `CiGate::check_veto_evidence` 或 `VetoStreakChecker` 逻辑,避免概念混淆。P5.2 实施时应遵循此拆分。

## 影响

### 新增内容

- **新增 ADR**: 本文档(ADR-045)
- **新增文档同步条目**: `docs/architecture/adr_index.md` 新增 ADR-045 条目(总数 39 → 40,需同步更新)

### 修改内容(文档层面,非代码)

- **`docs/architecture/adr_index.md`**: 新增 ADR-045 条目,记录命名调和决策
- **`CHANGELOG.md`**: 新增"ADR-045 INV-9 命名调和"条目,记录 P3-W11.3 实施确认与 ADR-032/044 描述修正
- **`docs/architecture/CODE_WIKI.md`**: §2.3 ADR 表新增 ADR-045 条目
- **`.trae/specs/nexus-omega-v5-implementation-plan/tasks.md`**: P3-W11.3 标记为完成,引用本 ADR

### 不修改内容(代码层面,本 ADR 是调和性决策)

- **`crates/chimera-mas/src/invariants.rs`**: 不修改(P3-W11.3 实现已正确,本 ADR 仅确认)
- **`crates/chimera-mas/src/error.rs`**: 不修改(`DelegationCycleDetected` 已存在)
- **`crates/chimera-mas/tests/proptest.rs`**: 不修改(1000 次 proptest 已存在)
- **`crates/chimera-mas/tests/invariants_test.rs`**: 不修改(INV-9 单元测试已存在)
- **`crates/chimera-mas/benches/mas_benchmark.rs`**: 不修改(本 ADR 不涉及基准测试)
- **`crates/chimera-mas/src/lib.rs`**: 不修改(模块声明已存在)
- **`crates/event-bus/src/types.rs`**: 不修改(本 ADR 不涉及事件变体)
- **ADR-032 / ADR-044 文档**: 不修改(append-only 原则,本 ADR 仅记录命名修正)

### 资源影响评估

| 维度 | 评估 |
|------|------|
| crate 数量 | 35(不变,本 ADR 是调和性决策,无代码新增) |
| 依赖变更 | 无(本 ADR 不引入新依赖) |
| Docker/binary 体积 | 不受影响(无代码变更) |
| NexusEvent 变体数 | 不变(本 ADR 不涉及事件变体) |
| MasError 变体数 | 34(不变,`DelegationCycleDetected` 已存在,`VetoEvidenceInsufficient` 未实现且作废) |
| InvariantChecker 不变量数 | 3(INV-7/8/9,不变) |
| 测试覆盖 | 不变(INV-9 已有 4 proptest + 13 单元测试) |
| 版本号 | 不变(本 ADR 是约束性决策,非功能性新增) |

## 考虑的方案

### 方案 A: 确认 P3-W11.3 实现为权威,修正 ADR-032/044 描述(采纳)

- **内容**: 本 ADR 确认 P3-W11.3 实现遵循 v5.0 设计文档(权威源),命名/接口/proptest 规格/错误类型/输入类型五维均与 v5.0 对齐。ADR-032 决策 2 与 ADR-044 决策 8 的描述偏差通过本 ADR 修正(不修改原文档,仅记录修正)。
- **采纳理由**:
  1. v5.0 设计文档 §5.2 第 8 项是 INV-9 的权威定义源,P3-W11.3 实现遵循此源
  2. P3-W11.3 实现与 INV-7/INV-8 同构编码,符合 v5.0 设计文档要求
  3. 1000 次 proptest 已落地,对齐 v5.0 §9.3 与 ADR-028 决策 3 规格
  4. 修正 ADR-032/044 描述遵循 append-only 原则,不破坏既有 ADR 完整性
  5. 通道 B 调用接口稳定,P5.2 可放心实施

### 方案 B: 按 ADR-044 期望重命名为 `check_inv9` 简称(否决)

- **内容**: 将 `check_inv9_delegation_acyclic` 重命名为 `check_inv9`,符合 ADR-044 决策 8 的期望。
- **否决理由**:
  1. **违反命名模式一致性**: INV-7/INV-8 采用 `check_inv<N>_<descriptor>` 模式,简称 `check_inv9` 破坏一致性
  2. **降低自描述性**: `check_inv9` 无法自描述检查对象,代码可读性下降
  3. **破坏性变更**: 重命名已落地方法属于破坏性 API 变更,违反 append-only 原则(ADR-028 决策 1)
  4. **无必要**: ADR-044 决策 8 的期望基于 ADR-032 决策 2 的误述,本 ADR 决策 1 已修正此误述,简称需求不成立

### 方案 C: 按 ADR-032 期望实现 `check_inv9_veto_evidence`(否决)

- **内容**: 在 `crates/chimera-mas/src/invariants.rs` 新增 `check_inv9_veto_evidence(regression_streak, significance)` 方法,与 `check_inv9_delegation_acyclic` 共存。
- **否决理由**:
  1. **概念混淆**: `veto_evidence` 是通道 B 否决逻辑(L5),不属于 INV-9(L9 MAS 子系统不变量),共存会混淆架构层归属
  2. **重复实现**: 通道 B 的否决证据检查应作为 `CiGate` trait 的方法或 `VetoStreakChecker` 独立类型,不应占用 `InvariantChecker` 命名空间
  3. **违反单一职责**: `InvariantChecker` 应专注 MAS 子系统不变量(INV-7/8/9),不应承载通道 B 进化回路逻辑
  4. **错误变体污染**: 新增 `VetoEvidenceInsufficient` 会污染 `MasError` 命名空间,该错误应属 `AutoDpoError` 或 `GsoeError`

### 方案 D: 修改 ADR-032 / ADR-044 文档本身(否决)

- **内容**: 直接修改 ADR-032 决策 2 与 ADR-044 决策 8 的描述,将 `check_inv9_veto_evidence` 改为 `check_inv9_delegation_acyclic`。
- **否决理由**:
  1. **违反 append-only 原则**: ADR-028 决策 1 已确立 append-only 哲学,直接修改 ADR 文档属于破坏性变更
  2. **失去文档可追溯性**: 直接修改原文档会丢失"命名偏差如何产生"的历史信息,本 ADR 通过调和性决策方式记录修正,保留可追溯性
  3. **影响范围不可控**: 修改 ADR-032 / ADR-044 可能影响其引用的其他文档,本 ADR 隔离修正影响

## 合规性

- **§2.1 分层映射**: 符合。INV-9 属 L9 Quest(chimera-mas 子系统不变量),本 ADR 不改变分层结构,确认 P3-W11.3 实现的架构层归属正确。
- **§2.2 依赖铁律 + 唯一通道**: 符合。本 ADR 不引入新的跨层依赖。`DelegationCycleDetected` 错误返回后,调用方发布 Critical 事件走 mpsc 旁路通道(§6.2 红线)。
- **§3.3.1 第 1 条(OMEGA 四定律守恒)**: 符合。本 ADR 不自实现压缩/进化/总线,INV-9 是 MAS 子系统不变量,与 Ω-Sparse/Ω-Compress/Ω-Evolve/Ω-Event 四定律正交。
- **§3.3.1 第 4 条(领域类型稳定性)**: 符合。本 ADR 不改 `UserIntent` / `Quest` / `Checkpoint` / `OmniSparseMasks` / `CLV` / `NexusState` / `AgentType` / `AgentTask`。`DelegationEdge` 是 INV-9 输入类型,属 chimera-mas 内部类型,非核心领域类型。
- **§3.3.1 第 5 条(向后兼容)**: 符合。本 ADR 是调和性决策,不修改任何公共 API 签名,P3-W11.3 实现的 `check_inv9_delegation_acyclic` 接口稳定。
- **§3.4.1 第 6 条(性能可证伪)**: 符合。INV-9 的 1000 次 proptest 提供客观证据,覆盖四性质(无环/全可达/无意外循环/回边检测)。
- **§3.4.1 第 7 条(学术支撑落地)**: 符合。INV-9 的 DFS 三色标记法基于 CLRS §22.3(Cormen et al., 算法导论),O(V+E) 时间复杂度有学术依据。
- **§4.1 编码规范**: 符合。`#![forbid(unsafe_code)]` 保持;库层 thiserror;无生产路径 unwrap/expect;单函数 ≤200 行(`check_inv9_delegation_acyclic` < 30 行,核心逻辑委托 `dfs_visit_cycle`)。
- **§4.4 async 反模式**: 符合。INV-9 是纯函数,不涉及 async/await,无持锁跨 .await 风险。
- **§6.1 架构红线(零循环委托)**: 符合。INV-9 是 §6.2 红线"零循环委托"的工程实施层落地,通过 DFS 三色标记法检测委托图环,防止递归委托死循环。
- **§6.2 Week 1-8 实战新红线**: 符合。`DelegationCycleDetected` 触发后,调用方应发布 Critical 事件走 mpsc 旁路通道(§6.2 红线 5);不持锁 .await(纯函数,无锁)。
- **ADR-027 / ADR-028 既有决策**: 全部保持。INV-9 沿用 `InvariantChecker` 纯函数模式(ADR-028 决策 3);`MasError` 变体扩展沿用 append-only(ADR-028 决策 1);1000 次 proptest 对齐 ADR-028 决策 3 规格。
- **ADR-032 决策 2**: 本 ADR 修正其 INV-9 命名引用,不修改其"连续 3 次统计显著回归才否决"判定逻辑。通道 B 的否决证据检查作为独立逻辑,通过 `CiGate::check_veto_evidence` 或 `VetoStreakChecker` 实现。
- **ADR-044 决策 8**: 本 ADR 解除其"通道 B 实施前必须先完成 ADR-045 命名调和"约束。本 ADR 批准后,通道 B 可启动实施,接口稳定可放心调用。

## 相关文档

- **设计文档**: [NEXUS-OMEGA_v5.0_系统性完整设计文档.md](file:///D:/Chimera CLI/NEXUS-OMEGA_v5.0_系统性完整设计文档.md) §5.2 九项裁决第 8 项(INV-9 权威定义源)+ §7.4 RHI-CG 双通道(通道 B 引用 INV-9)+ §9.3(proptest 1000 次规格)
- **规则**: [.trae/rules/nuxus规则.md](file:///D:/Chimera CLI/.trae/rules/nuxus规则.md) §2.1(分层映射)/§2.2(依赖铁律)/§3.3.1(第二阶段开发原则)/§3.4.1(第三阶段开发原则)/§4.1(编码规范)/§4.4(async 反模式)/§6.1(架构红线)/§6.2(Week 1-8 新红线)
- **CODE_WIKI.md**: [docs/architecture/CODE_WIKI.md](file:///D:/Chimera CLI/docs/architecture/CODE_WIKI.md) §3.1(crate 索引)/§2.3(ADR 表)
- **ADR 索引**: [docs/architecture/adr_index.md](file:///D:/Chimera CLI/docs/architecture/adr_index.md)(本 ADR 同步更新)
- **关联 ADR**:
  - [ADR-027](file:///D:/Chimera CLI/docs/architecture/ADR-027-chimera-mas-quadrant.md)(四象限分工 — INV-7/8/9 雏形定义,本 ADR 确认 INV-9 实施)
  - [ADR-028](file:///D:/Chimera CLI/docs/architecture/ADR-028-chimera-mas-part2-closure.md)(Part II 闭环 — INV-7/8 proptest 1000 次规格,本 ADR 确认 INV-9 同规格)
  - [ADR-032](file:///D:/Chimera CLI/docs/architecture/ADR-032-dual-channel-evaluator.md)(RHI-CG 双通道评估器 — 决策 2 误述 INV-9 语义,本 ADR 决策 1/7 修正其命名引用)
  - [ADR-044](file:///D:/Chimera CLI/docs/architecture/ADR-044-rhi-cg-engineering.md)(RHI-CG 双通道工程实施 — 决策 8 期望 ADR-045 命名调和,本 ADR 决策 8 解除此约束)
- **代码基线**(权威源,本 ADR 确认):
  - [crates/chimera-mas/src/invariants.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs)(`InvariantChecker` + `check_inv9_delegation_acyclic` + `DelegationEdge` + `dfs_visit_cycle` + 13 单元测试)
  - [crates/chimera-mas/src/error.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/error.rs)(`MasError::DelegationCycleDetected` 变体定义,共 34 变体)
  - [crates/chimera-mas/tests/proptest.rs](file:///D:/Chimera CLI/crates/chimera-mas/tests/proptest.rs)(4 个 INV-9 proptest,1000 cases 覆盖四性质)
  - [crates/chimera-mas/tests/invariants_test.rs](file:///D:/Chimera CLI/crates/chimera-mas/tests/invariants_test.rs)(INV-7/INV-8 集成测试,INV-9 单元测试在 `src/invariants.rs` 模块内)
  - [crates/chimera-mas/benches/mas_benchmark.rs](file:///D:/Chimera CLI/crates/chimera-mas/benches/mas_benchmark.rs)(9 项 criterion 基准,本 ADR 不涉及新增基准)

---

> **维护者**: NEXUS-OMEGA 团队
> **创建日期**: 2026-07-26
> **基线版本**: v2.3.1-omega(P3-W11.3 已落地,本 ADR 是调和性决策,无代码变更)
> **决策者**: E01 首席架构师(15+ 年,分布式深度分析与多轮结构化思考)
> **分析团队**: E01 首席架构师(主导)+ E04 路由算法专家(算法复核)+ E05 生产系统专家(接口稳定性复核)
> **前置依赖**: 无(本 ADR 是 ADR-044 通道 B 实施的前置约束,本 ADR 批准后通道 B 可启动)
> **后续约束**: P5.2 通道 B 实施时,调用 `InvariantChecker::check_inv9_delegation_acyclic(&edges)` 检查 MAS 委托图无环;通道 B 的否决证据检查作为独立 `CiGate::check_veto_evidence` 或 `VetoStreakChecker` 逻辑,不与 INV-9 混淆

## 附录: INV-9 实施现状核实表

> 本附录记录 ADR-045 创建前(2026-07-26)对 INV-9 实施现状的五维核实结果,作为本 ADR 决策的事实基础。

### A.1 命名核实

| 维度 | v5.0 设计文档 | ADR-032 决策 2(误述) | ADR-044 决策 8(误述) | P3-W11.3 实际(权威) | 一致性 |
|------|--------------|---------------------|---------------------|---------------------|--------|
| 不变量编号 | INV-9 | INV-9 | INV-9 | INV-9 | ✅ |
| 不变量语义 | 委托图无环 | 否决证据充分性 | (引用 ADR-032) | 委托图无环 | ✅(与 v5.0 一致) |
| 方法名 | (未明确) | `check_inv9_veto_evidence` | `check_inv9`(期望) | `check_inv9_delegation_acyclic` | ✅(与 v5.0 同构模式一致) |
| 命名模式 | `check_inv<N>_<descriptor>` | (偏离) | `check_inv<N>` 简称 | `check_inv<N>_<descriptor>` | ✅(与 INV-7/8 同构) |

### A.2 检查接口核实

| 维度 | v5.0 设计文档 | ADR-032 决策 2(误述) | P3-W11.3 实际(权威) | 一致性 |
|------|--------------|---------------------|---------------------|--------|
| 调用方式 | (未明确) | 关联函数 | `InvariantChecker::check_inv9_delegation_acyclic(&edges)` | ✅ |
| 输入类型 | 委托边列表 | `(regression_streak, significance)` | `&[DelegationEdge]` | ✅(与 v5.0 一致) |
| 返回类型 | (未明确) | `Result<()>` | `Result<(), MasError>` | ✅ |
| 副作用 | (未明确) | 无(纯函数) | 无(纯函数,不持状态,不发布事件) | ✅ |
| 失败处理 | 拒绝继续委托 | 返回错误 | 返回 `Err(DelegationCycleDetected)`,调用方发布 Critical 事件 | ✅ |

### A.3 proptest 规格核实

| 维度 | v5.0 设计文档 | ADR-028 决策 3 | P3-W11.3 实际(权威) | 一致性 |
|------|--------------|----------------|---------------------|--------|
| cases 数 | 1000 次(§9.3) | 1000 次 | `cases: 1000`(显式配置) | ✅ |
| 测试维度 | 四性质(无环/全可达/无意外循环/回边) | INV-7/8 同构 | 4 个 proptest 覆盖四性质 | ✅ |
| 测试语法 | block-named(§4.1) | block-named | `fn name(edges in strategy) { ... }` | ✅ |
| 测试位置 | `tests/proptest.rs` | `tests/proptest.rs` | `crates/chimera-mas/tests/proptest.rs:833-1045` | ✅ |
| 输入策略 | DAG/树/含环图/DAG+回边 | 同 INV-7/8 规格扩展 | `arb_dag_edges` / `arb_tree_edges` / `arb_cyclic_edges` / `arb_dag_plus_back_edge` | ✅ |

### A.4 错误类型核实

| 维度 | v5.0 设计文档 | ADR-032 决策 2(误述) | P3-W11.3 实际(权威) | 一致性 |
|------|--------------|---------------------|---------------------|--------|
| 变体名 | (未明确) | `VetoEvidenceInsufficient` | `DelegationCycleDetected` | ✅(自描述) |
| 字段 | (未明确) | `regression_streak, significance` | `cycle_path: Vec<String>` | ✅(携带诊断信息) |
| Display | (未明确) | (未明确) | `"Delegation cycle detected (INV-9): cycle_path = {:?}"` | ✅(含 INV-9 标识) |
| 严重级别 | Critical(§6.2) | (未明确) | Critical(调用方发布走 mpsc) | ✅ |
| 处理策略 | 拒绝继续委托 | (未明确) | 返回错误,调用方阻断涉环 Agent 派生 | ✅ |
| 变体总数 | (未明确) | 34(预期) | 34(实际,静态断言) | ✅ |

### A.5 输入类型核实

| 维度 | v5.0 设计文档 | ADR-027/028 | P3-W11.3 实际(权威) | 一致性 |
|------|--------------|-------------|---------------------|--------|
| 类型名 | (未明确,泛指"委托边") | (未明确) | `DelegationEdge` | ✅ |
| 字段 | (未明确) | (未明确) | `from: String` + `to: String` | ✅(显式有向边) |
| 构造器 | (未明确) | (未明确) | `new(impl Into<String>, impl Into<String>)` | ✅(API 友好) |
| Trait 派生 | (未明确) | (未明确) | `Debug + Clone + PartialEq + Eq + Hash` | ✅(可序列化/可比较/可哈希) |
| 位置 | `crates/chimera-mas/src/invariants.rs` | 同 | `crates/chimera-mas/src/invariants.rs:161-189` | ✅ |
| 设计决策 | (未明确) | (未明确) | 独立类型(非复用 `AgentTask`),`String` owned(非 `&str`) | ✅(语义清晰,生命周期简单) |

### A.6 通道 B 调用约束核实

| 维度 | ADR-044 决策 8 期望 | P3-W11.3 实际(权威) | 本 ADR 决策 |
|------|---------------------|---------------------|-------------|
| 调用方法名 | `check_inv9`(期望重命名后) | `check_inv9_delegation_acyclic`(已落地) | 决策 2 确认 `check_inv9_delegation_acyclic` 为权威命名 |
| 调用签名 | `check_inv9(regression_streak: u32, ...)` | `check_inv9_delegation_acyclic(edges: &[DelegationEdge])` | 决策 3 确认接口稳定 |
| 前置约束 | "通道 B 实施前必须先完成 ADR-045 命名调和" | (本 ADR 创建前不存在) | 决策 8 解除此约束,通道 B 可放心调用 |
| 否决证据检查 | (与 INV-9 混淆) | (未实现) | 决策 1 将否决证据检查作为独立 `CiGate::check_veto_evidence` 逻辑 |

### A.7 单元测试覆盖核实

| 测试类别 | 测试数量 | 覆盖场景 | 文件位置 |
|---------|---------|---------|---------|
| 空边/单边 | 2 | 空边列表无环 / 单条边无环 | `src/invariants.rs:594-607` |
| 线性链/树 | 2 | 线性链(A→B→C→D) / 树结构(root→A,B;A→C,D) | `src/invariants.rs:610-633` |
| 自环/2-3 节点环 | 3 | 自环(A→A) / 两节点环(A↔B) / 三节点环(A→B→C→A) | `src/invariants.rs:636-699` |
| 多连通分量 | 2 | 全无环 / 一个有环 | `src/invariants.rs:702-731` |
| 深度约束 | 1 | 深度 5 线性链(匹配 MAX_AGENT_DEPTH) | `src/invariants.rs:734-746` |
| DAG 交叉边 | 1 | A→B,A→C,B→D,C→D(D 有两父但无环) | `src/invariants.rs:749-760` |
| 环闭合性 | 1 | 环路径首尾必须相同 | `src/invariants.rs:763-786` |
| 重复边 | 1 | 重复边(A→B, A→B)不构成环 | `src/invariants.rs:789-797` |
| **合计** | **13** | **覆盖全部边界场景** | `crates/chimera-mas/src/invariants.rs:593-797` |

### A.8 proptest 覆盖核实

| 测试名 | 性质 | cases | 输入策略 | 文件位置 |
|--------|------|-------|---------|---------|
| `inv9_dag_always_acyclic` | 无环 | 1000 | `arb_dag_edges()` | `tests/proptest.rs:949-955` |
| `inv9_tree_always_acyclic_and_reachable` | 全可达 | 1000 | `arb_tree_edges()` | `tests/proptest.rs:963-969` |
| `inv9_cyclic_graph_always_rejected` | 无意外循环 | 1000 | `arb_cyclic_edges()` | `tests/proptest.rs:976-1008` |
| `inv9_dag_plus_back_edge_always_rejected` | 回边检测 | 1000 | `arb_dag_plus_back_edge()` | `tests/proptest.rs:1015-1045` |
| **合计** | **四性质** | **4000 cases** | **四策略** | `crates/chimera-mas/tests/proptest.rs:833-1045` |
