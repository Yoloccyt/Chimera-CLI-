# ADR-033: L0 nexus-contracts 契约层建立(依赖铁律扩展 L(N) → L(0) 恒允许)

> **🔄 v2.4.0-omega 基线增补**(2026-07-26):本文档基于 v2.3.1-omega **35 crate / 内环 9 + 外环 25 crate** 架构撰写;**v2.4.0-omega 后 workspace 演进为 37 crate**(本 ADR 决策 1 新建 `nexus-contracts` @ L0,实际为 36 → 37 增量之一;另一个为 L6 `omega-learner`,ADR-031 落地)——**L0 契约层 100% 落地**,依赖铁律扩展 `L(N) → L(0)` 恒允许,E01 穷举 37 个 Cargo.toml 验证零违规。**L1 上帝 crate 病理部分缓解(B+ 级)**:nexus-core/event-bus 仍为高频依赖,但新增 L0 解耦基底有效。

## 状态

已批准 (2026-07-23)

## 背景

- 项目当前处于 v2.3.1-omega(第三阶段 — 模块级系统性优化)。既有 35 crate 架构自 L1 Core(`nexus-core` / `event-bus` / `model-router`)起向上延伸至 L10 Interface,**无 L0 契约层**。跨环(内环 9 crate + 外环 25 crate,C6 决策)共享的领域类型散落在各业务 crate 内,导致"谁定义、谁拥有、谁可依赖"的边界模糊。
- v5.0 设计文档 §3 设计总纲提出"四环收敛架构",要求一个比 L1 Core 更底层的契约层承载**跨环共享类型**,使内环与外环均能无方向违规地引用同一组类型定义。
- v5.0 设计文档 §11 深模块候选项提出 `VectorStore` trait 抽象(方案 C + A 的 remove),该 trait 被 `mlc-engine` / `repo-wiki` / `scc-cache` 等多个 crate 隐式实现,但缺少统一契约定义点,应置于 L0。
- v5.0 设计文档 D6 病理指出:HCW selector 手写权重 / OSA 静态掩码表 / scc 一阶马尔可夫等"策略"应移入 learner(crate 内只留 fallback)。策略的**配置契约**(静态常量 vs 学习版本号)需要 L0 提供统一枚举。
- v5.0 设计文档 C6 决策:内环 9 crate + 外环 25 crate,跨环共享类型必须上提 L0。
- **代码基线核实(v2.3.1-omega)**:`OmniSparseMasks` 当前定义于 [coordinator.rs](file:///D:/Chimera%20CLI/crates/osa-coordinator/src/coordinator.rs#L44-L63)(L6 Router `osa-coordinator`),是 OSA 产出的核心契约类型(五维稀疏掩码 + SHA-256 `mask_hash`)。该结构体依赖 `SparseMask<T>`(osa-coordinator 内部 `masks` 模块)、五维 ID newtype([types.rs:L25](file:///D:/Chimera%20CLI/crates/osa-coordinator/src/types.rs#L25) 由 `nexus_core::id_newtype!` 宏生成)与哈希计算逻辑(`sha2`/`hex`/`serde_json`)。
- **星型耦合诊断的代码基线校正**:v5.0 设计文档 D4 病理诊断"3 个 router(kvbsr-router / faae-router / sesa-router)星型依赖 osa-coordinator"。经核实,3 个 router 的 [Cargo.toml](file:///D:/Chimera%20CLI/crates/kvbsr-router/Cargo.toml#L6-L10) 当前生产依赖为 `nexus-core` + `event-bus`,**并未直接依赖 `osa-coordinator`**;真正在生产依赖图中依赖 osa-coordinator 的是 [chimera-mas/Cargo.toml:L52](file:///D:/Chimera%20CLI/crates/chimera-mas/Cargo.toml#L52)(用于 `OmniSparseCoordinator::compute_masks()`)。D4 诊断的"星型耦合"系**前瞻性风险**:`OmniSparseMasks` 作为 OSA 核心产出,一旦 router 需消费稀疏掩码(§6.1 红线要求"必须经 HCW 分层 + OSA 稀疏化后再加载",router 感知稀疏度是合理演进方向),即会形成对 `osa-coordinator`(L6)的星型依赖。本 ADR **前置消解**该风险,而非事后修补。
- 经 E01 首席架构师 + E03 记忆系统专家 + E04 路由算法专家分布式评审与多轮交叉验证,确认建立 L0 契约层是消解跨环类型共享边界模糊、前置消除星型耦合风险、为 VectorStore 深模块抽象提供统一落点三项诉求的**共同最小充分解**。

> **ADR 编号确认**:既有 ADR 序列为 ADR-028(Part II 闭环)→ ADR-029(TUI v3.1 引擎)→ ADR-031(Harness-as-Spec + omega-learner 边界)。本 ADR 编号确认为 **ADR-033**,承接 ADR-031 的 `HarnessSpec` / `ContractSpec` 契约上提诉求。

## 决策

经专家团队多轮结构化思考与多路径交叉验证,对 L0 契约层建立作出以下 6 项架构决策:

### 决策 1: 新建 `crates/nexus-contracts/` crate(L0 Contracts)

- **位置**: `crates/nexus-contracts/`
- **架构层归属**: L0 Contracts(新增层,比 L1 Core 更底层,为整个十层架构的契约根)
- **性质**: 纯类型 + 零逻辑 + 零外部业务依赖(仅 `serde` 派生用于序列化契约)
- **crate 级属性**: `#![forbid(unsafe_code)]`(与 workspace 全 crate 一致,§6.2 红线)
- **标准布局**: 遵循 §4.2 模块组织模式

```rust
// crates/nexus-contracts/src/lib.rs
#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

//! L0 契约层 — 跨环共享的纯类型契约
//!
//! 架构层归属: L0 Contracts(十层架构契约根,比 L1 Core 更底层)
//! 核心职责: 为内环 9 crate + 外环 25 crate 提供无方向违规的共享类型定义点
//! 设计约束: 纯类型 + 零逻辑 + 零业务依赖(仅 serde 派生)

pub mod sparse;
pub mod temporal;
pub mod vector_store;
pub mod selector;
pub mod harness;
pub mod quota;

pub use sparse::*;
pub use temporal::*;
pub use vector_store::*;
pub use selector::*;
pub use harness::*;
pub use quota::*;

pub mod prelude {
    pub use crate::{sparse::*, temporal::*, vector_store::*, selector::*, harness::*, quota::*};
}
```

### 决策 2: 依赖铁律扩展 — L(N) → L(0) 恒允许

原 §2.2 依赖铁律:

```
L(N) → L(N)   ✓ 同层互引允许
L(N) → L(N-1) ✓ 向下依赖允许
L(N) → L(N+1) ✗ 向上依赖禁止
L(N) ──event-bus── L(M)  ✓ 跨层通信只能走 Event Bus
```

**新增条款**:

```
L(N) → L(0)   ✓ 恒允许(任何层可依赖 L0 契约层,N ∈ [1, 10])
L(0) → L(M)   ✗ 禁止(L0 自身零业务依赖,不依赖任何 L1+ crate)
```

- **L0 零依赖铁律**: `nexus-contracts` 的 `[dependencies]` 仅允许 `serde`(派生契约)、`std`;**禁止**依赖 `nexus-core` / `event-bus` 或任何 L1+ crate。这是 L0 作为"契约根"的根本约束——若 L0 依赖 L1,则形成 L0→L1→L0 循环,铁律崩塌。
- **推论**: 当前由 `nexus_core::id_newtype!` 宏([types.rs:L25](file:///D:/Chimera%20CLI/crates/osa-coordinator/src/types.rs#L25))生成的五维 ID newtype,上提至 L0 时**不能**沿用 `nexus-core` 的宏。L0 须自带轻量 newtype 定义(见决策 3 依赖类型处置)。
- **后续演进**: `id_newtype!` 宏可下沉至 L0(`nexus-core` 反向从 L0 引用),但属独立 ADR,本 ADR 不强制。

### 决策 3: L0 承载类型清单 + 逻辑分层原则

L0 承载以下 6 类跨环共享类型。所有上提类型遵循**逻辑分层原则**:纯数据结构 + 不依赖外部 crate 的纯计算方法(如 `PartialEq`、`average_sparsity`、`sparsity_ratio` 访问器)留在 L0;依赖外部 crate 的逻辑(如 `sha2`/`hex` 哈希、`serde_json` 序列化、`select_nth_unstable` Top-K)通过消费者 crate 定义的扩展 trait(`*Ext`)提供。

| 类型 | 来源 | L0 承载内容 | 扩展 trait 留存 |
|------|------|------------|---------------|
| `OmniSparseMasks` | `osa-coordinator` 上提 | 结构体 + 五维字段 + `average_sparsity()` + `PartialEq` | `OmniSparseMasksExt`(`compute_mask_hash`/`to_json`/`mask_hash`,留 osa-coordinator,依赖 sha2/hex/serde_json) |
| `SparseMask<T>` | `osa-coordinator` 上提 | 数据结构(`active_ids` + `sparsity_ratio`)+ `empty()`/`active_count()` | `SparseMaskExt`(`select_top_k` 用 `select_nth_unstable`,留 osa-coordinator) |
| 五维 ID newtype | `osa-coordinator` 上提 | `ToolId`/`FileId`/`MemoryId`/`OperationId`/`TaskId`(L0 内联 String newtype,derive `Deref`/`AsRef`/`Display`/`serde(transparent)`) | — |
| `HarnessSpec`/`ContractSpec`/`HopSpec` | ADR-031 契约上提 | TOML 反序列化派生类型(`#[derive(Deserialize)]`) | — |
| `TemporalMeta`/`TransitionType` | D12 修复 | `Current`/`Historical`/`Transition` 枚举 + 时间元数据 | — |
| `NamespaceQuota` | `chimera-mas` 上提 | 命名空间配额(上限/已用/保留) | — |
| `VectorStore` trait + `VectorStoreExt` | v5.0 §11 深模块候选 | trait 定义(`query`/`upsert`/`remove` 签名)+ 扩展 trait(`query_top_k` 默认实现) | 实现留各 crate(mlc-engine/repo-wiki/scc-cache) |
| `SelectorPolicy` 枚举 | D6/D1 修复 | `Static(常量)` / `Learned { version: u64 }` 两变体 | 策略执行逻辑留各 learner crate |

**`OmniSparseMasks` 上提的依赖链处置**(关键工程约束):

`OmniSparseMasks` 当前定义(见 [coordinator.rs:L44-L63](file:///D:/Chimera%20CLI/crates/osa-coordinator/src/coordinator.rs#L44-L63))依赖 `SparseMask<T>` 与五维 ID newtype。这三者构成依赖链,必须**整链上提**或**整链 re-export**,否则 L0 的 `OmniSparseMasks` 无法编译(字段类型未定义)。本 ADR 采用整链上提策略:

1. `SparseMask<T>` 作为纯数据结构上提 L0(`sparse` 模块)
2. 五维 ID newtype 在 L0 内联定义(不依赖 `nexus_core::id_newtype!` 宏,改用 L0 本地 derive)
3. `OmniSparseMasks` 结构体上提 L0,字段类型引用 L0 内的 `SparseMask<T>` 与 ID newtype
4. 哈希逻辑(`compute_mask_hash`,依赖 `sha2`/`hex`/`serde_json`)通过 osa-coordinator 定义的 `OmniSparseMasksExt` 扩展 trait 提供,不进入 L0

**`osa-coordinator` re-export 兼容**:`osa-coordinator/src/lib.rs` 改为 `pub use nexus_contracts::{OmniSparseMasks, SparseMask, ToolId, ...}`,使现有消费者(如 [chimera-mas/Cargo.toml:L52](file:///D:/Chimera%20CLI/crates/chimera-mas/Cargo.toml#L52))的 `use osa_coordinator::OmniSparseMasks` 语句**零修改**通过编译,保证向后兼容。

### 决策 4: 下游消费者迁移 — re-export 兼容 + 前置消解星型耦合

迁移分两阶段,以 re-export 保证零破坏:

**阶段 A(re-export 兼容,本 ADR 落地即完成)**:

- `crates/osa-coordinator/src/lib.rs`:`OmniSparseMasks` / `SparseMask` / 五维 ID newtype 改为 `pub use nexus_contracts::*` re-export,`OmniSparseMasksExt` 扩展 trait 保留在本 crate
- `crates/osa-coordinator/src/coordinator.rs`:[第 44-145 行](file:///D:/Chimera%20CLI/crates/osa-coordinator/src/coordinator.rs#L44-L145)的 `OmniSparseMasks` 定义移除,改为从 L0 引入;`compute_all_masks`(见 [第 201-267 行](file:///D:/Chimera%20CLI/crates/osa-coordinator/src/coordinator.rs#L201-L267))内调用 `OmniSparseMasksExt` 扩展 trait 的哈希方法
- `crates/osa-coordinator/Cargo.toml`:`[dependencies]` 新增 `nexus-contracts = { path = "../nexus-contracts" }`
- `crates/chimera-mas/Cargo.toml`:**保持不变**(继续 `use osa_coordinator::OmniSparseMasks`,经 re-export 透明穿透),本阶段不强制 chimera-mas 直接依赖 L0

**阶段 B(显式依赖切换,后续 PR)**:

- `crates/chimera-mas/Cargo.toml`:`osa-coordinator` 依赖可降级为 dev-dependency(若 chimera-mas 仅需类型而不需 `compute_masks` 调用),或新增 `nexus-contracts` 直接依赖以获取纯类型
- 3 个 router(kvbsr/faae/sesa):**当前不依赖 osa-coordinator**(代码基线已确认),无需切换;未来若需消费 `OmniSparseMasks`,直接依赖 `nexus-contracts`(L0),**前置消解**对 osa-coordinator(L6)的星型耦合风险

**效果**: `osa-coordinator` 的"星型中心"地位下降——`OmniSparseMasks` 的所有权转移至 L0,osa-coordinator 退化为"计算逻辑提供方 + 类型 re-export 中转站",Insight 2 星型耦合病理消解。

### 决策 5: workspace 注册

根 `Cargo.toml` 两处变更:

```toml
# [workspace] members 新增(插入于 nexus-core 之前,L0 在最前)
members = [
    "crates/nexus-contracts",
    "crates/nexus-core", "crates/event-bus", /* ... 既有 35 crate */
]

# [workspace.dependencies] 新增
nexus-contracts = { path = "crates/nexus-contracts" }
```

- 根 `Cargo.toml` workspace members 从 35 → **36** crate
- `nexus-contracts` 注册位置置于 members 列表首位,语义上标识其"L0 契约根"地位
- `.cargo/config.toml` 无需变更(linker/增量编译配置与 crate 数无关)

### 决策 6: 版本号策略 — minor(2.3.1-omega → 2.4.0-omega)

本增量以 SemVer **minor** 交付,推荐 `workspace.package.version`: `2.3.1-omega` → `2.4.0-omega`。

- **非破坏性依据**: 阶段 A 采用 re-export 兼容(决策 4),`use osa_coordinator::OmniSparseMasks` 等现有语句零修改通过编译;依赖铁律扩展是**约束放宽**(新增允许条款),不破坏任何现有合法依赖
- **不升 major**: 升 major(3.0.0-omega)的触发条件是破坏性 API 变更。本增量 re-export 保证 API 兼容,无破坏性变更。3.0.0-omega 保留给未来"移除 re-export、强制消费者迁移至 L0 直接依赖"的破坏性里程碑
- **决策点(邀请确认)**: 若团队倾向"一步到位"推行强制迁移(阶段 A + B 同 PR,移除 osa-coordinator re-export,迫使 chimera-mas 直接依赖 L0),则应升 major(3.0.0-omega)。本 ADR 默认推荐 minor + 两阶段渐进,降低单次发布风险

## 理由

### 决策 1 理由(新建 L0 crate)

- **C6 决策**: 内环 9 crate + 外环 25 crate,跨环共享类型必须上提 L0。无 L0 则跨环类型只能放在某业务 crate 内,导致该 crate 成为隐式"上帝 crate"
- **§3.4.1 第 4 条(领域类型稳定性)**: `OmniSparseMasks` 上提需 ADR 记录。L0 为其提供稳定的契约归宿,后续演进有明确变更点
- **纯类型 + 零逻辑**: 契约层不应承载业务逻辑,避免"契约层膨胀"。L0 的编译时间与复杂度最小化(仅 serde 派生,无 async/无 IO)

### 决策 2 理由(依赖铁律扩展)

- **§2.2 依赖铁律**: 原铁律无"L0 恒允许"条款,若严格执行"L(N)→L(N-1)",则 L2 想用 L0 类型需经 L1 中转,徒增耦合。新增"L(N)→L(0) 恒允许"使 L0 成为所有层的合法公共契约源
- **L0 零依赖铁律**: 这是 L0 不退化"上帝 crate"的根本保障。若 L0 依赖 L1,则 L1 变更会传导至 L0 再传导至全层,契约稳定性丧失
- **id_newtype 宏推论**: 严格推导 L0 零依赖后,`nexus_core::id_newtype!` 宏不能在 L0 使用。L0 内联 newtype 定义虽略有重复,但换取了契约层真正的零依赖,长期价值高于短期的宏复用

### 决策 3 理由(类型清单 + 逻辑分层)

- **整链上提**: `OmniSparseMasks` 依赖 `SparseMask<T>` 与 ID newtype,三者在 osa-coordinator 内耦合(见 [coordinator.rs:L29-L30](file:///D:/Chimera%20CLI/crates/osa-coordinator/src/coordinator.rs#L29-L30) `use crate::masks::SparseMask` 与 `use crate::types::{...}`)。部分上提会导致 L0 类型引用未定义类型,无法编译。整链上提是唯一自洽方案
- **逻辑分层原则**: L0 定位"纯类型 + 零逻辑"。`compute_mask_hash`(依赖 `sha2`/`hex`)若进入 L0,则 L0 不再"零依赖",违反决策 2。通过 `OmniSparseMasksExt` 扩展 trait 将逻辑留在 osa-coordinator,既保 L0 纯净,又不丢失功能
- **VectorStore trait**: v5.0 §11 深模块候选,被 mlc-engine/repo-wiki/scc-cache 隐式实现。L0 提供 trait 定义,各 crate 显式 `impl VectorStore for ...`,实现深模块抽象落地
- **SelectorPolicy 枚举**: D6 病理要求"策略移入 learner"。`Static`/`Learned` 枚举为策略选择提供统一契约,learner crate 据 variant 执行不同逻辑

### 决策 4 理由(re-export 兼容 + 前置消解)

- **§3.4.1 第 5 条(向后兼容)**: re-export 保证 `use osa_coordinator::OmniSparseMasks` 零修改,属非破坏性变更
- **代码基线校正**: 3 个 router 当前不依赖 osa-coordinator(已核实 Cargo.toml)。本 ADR 如实陈述,将 D4 诊断定位为前瞻性风险,前置消解而非误判已发生的耦合
- **chimera-mas 为真实消费者**: [chimera-mas/Cargo.toml:L52](file:///D:/Chimera%20CLI/crates/chimera-mas/Cargo.toml#L52) 是 osa-coordinator 的真实生产依赖。re-export 使 chimera-mas 阶段 A 零修改,降低迁移风险

### 决策 5 理由(workspace 注册)

- **§4.1 通用约定**: workspace 级依赖统一管理,`nexus-contracts` 注册于 `[workspace.dependencies]` 供成员通过 `workspace = true` 引用
- **members 首位**: 语义标识 L0 契约根地位,与分层映射 L0→L10 的阅读顺序一致

### 决策 6 理由(版本号策略)

- **§3.3.1 第 5 条(向后兼容)**: re-export 属非破坏性变更,SemVer 归 minor
- **降低单次发布风险**: minor 版本独立可验收,两阶段渐进(re-export → 显式切换)使回滚成本低
- **承接 ADR-028 决策 6**: ADR-028 已确立"3.0.0-omega 保留给真正破坏性 API 变更里程碑",本 ADR 延续该原则

## 影响

### 新增内容

- **新增 crate**: `crates/nexus-contracts/`(L0 Contracts,workspace 第 36 个 crate)
  - `src/lib.rs`(模块声明 + prelude)
  - `src/sparse.rs`(`SparseMask<T>` + `OmniSparseMasks` + 五维 ID newtype)
  - `src/temporal.rs`(`TemporalMeta` / `TransitionType`)
  - `src/vector_store.rs`(`VectorStore` trait + `VectorStoreExt`)
  - `src/selector.rs`(`SelectorPolicy` 枚举)
  - `src/harness.rs`(`HarnessSpec` / `ContractSpec` / `HopSpec`)
  - `src/quota.rs`(`NamespaceQuota`)
  - `src/error.rs`(`ContractsError` thiserror enum,序列化/反序列化错误)
  - `tests/contracts.rs`(类型 serde 往返 + PartialEq 不变量测试)
- **新增 ADR**: 本文档(ADR-033)

### 修改内容

- **根 `Cargo.toml`**: `[workspace] members` 新增 `crates/nexus-contracts`(35 → 36);`[workspace.dependencies]` 新增 `nexus-contracts = { path = "crates/nexus-contracts" }`
- **`crates/osa-coordinator/Cargo.toml`**: `[dependencies]` 新增 `nexus-contracts = { path = "../nexus-contracts" }`
- **`crates/osa-coordinator/src/lib.rs`**: `OmniSparseMasks` / `SparseMask` / 五维 ID newtype 改为 `pub use nexus_contracts::*` re-export;新增 `OmniSparseMasksExt` 扩展 trait 声明
- **`crates/osa-coordinator/src/coordinator.rs`**: 移除 [第 44-145 行](file:///D:/Chimera%20CLI/crates/osa-coordinator/src/coordinator.rs#L44-L145)的 `OmniSparseMasks` 定义,改为从 L0 引入;`compute_all_masks` 内哈希调用改为 `OmniSparseMasksExt` 方法
- **`crates/osa-coordinator/src/types.rs`**: 五维 ID newtype 定义移除(已上提 L0),保留 `TaskProfile` / `ComplexityBand` 等非上提类型
- **`.trae/rules/nuxus规则.md` §2.2**: 依赖铁律新增 `L(N) → L(0) ✓ 恒允许` 与 `L(0) → L(M) ✗ 禁止` 两条款
- **`docs/architecture/CODE_WIKI.md`**: §2.1 分层映射新增 L0 Contracts 层;§3.1 crate 索引新增 `nexus-contracts`(L0)
- **`docs/architecture/adr_index.md`**: 新增 ADR-033 条目
- **`CHANGELOG.md`**: 新增 v2.4.0-omega 章节

### 资源影响评估

| 维度 | 评估 |
|------|------|
| crate 数量 | 35 → 36(新增 L0 nexus-contracts) |
| 依赖变更 | osa-coordinator 新增 nexus-contracts 依赖;chimera-mas 阶段 A 不变(re-export 穿透) |
| Docker/binary 体积 | 微增(L0 纯类型编译产物极小,< 50KB);binary 仍 < 50MB |
| 编译时间 | L0 零逻辑零 async,编译开销可忽略;并行编译不受影响 |
| 测试覆盖 | 新增 ~20 个契约测试(serde 往返 + PartialEq + 类型构造);osa-coordinator 现有测试零回归(re-export 兼容) |
| 依赖铁律 | §2.2 新增 2 条款(L(N)→L(0) 恒允许 / L(0)→L(M) 禁止) |
| 版本号 | 2.3.1-omega → 2.4.0-omega(minor) |

## 考虑的方案

### 方案 1: 新建 L0 nexus-contracts + 依赖铁律扩展(采纳)

- **内容**: 如决策 1-6 所述,新建 L0 契约层,扩展依赖铁律 L(N)→L(0) 恒允许,整链上提 OmniSparseMasks 依赖类型,re-export 兼容。
- **采纳理由**: 同时满足"跨环类型共享""前置消解星型耦合""VectorStore 深模块落点"三项诉求;re-export 保证零破坏;L0 零依赖铁律防止退化上帝 crate。

### 方案 2: 类型上提到 nexus-core(L1)(否决)

- **内容**: 将 `OmniSparseMasks` 等共享类型上提到 `nexus-core`(L1),不新建 L0。
- **否决理由**:
  1. `nexus-core` 已是被 workspace 全部 crate 依赖的"上帝 crate"(见 [nexus-core/Cargo.toml](file:///D:/Chimera%20CLI/crates/nexus-core/Cargo.toml) 被引频次),再注入跨环契约类型加重其耦合与编译负担
  2. L1 承载业务核心类型(`CLV`/`NexusState`),与"纯契约零逻辑"定位冲突——nexus-core 已有 async/IO 逻辑,不符合 L0 纯类型约束
  3. 无法表达"L0 零依赖"铁律:L1 已依赖 `ndarray`/`sha2`/`rusqlite` 等,契约类型混入会继承这些依赖

### 方案 3: 各 crate 内复制定义(否决)

- **内容**: 不上提,在需要 `OmniSparseMasks` 的各 crate 内复制类型定义。
- **否决理由**:
  1. 违反 DRY,类型漂移风险极高(`mask_hash` 计算逻辑、`PartialEq` 语义一旦分叉,跨 crate 断言失效)
  2. `serde` 派生的类型若字段顺序/命名不一致,序列化结果不兼容,`mask_hash` 跨 crate 不一致
  3. 无法为 `VectorStore` trait 提供统一定义点,深模块抽象无法落地

## 合规性

- **§2.1 分层映射**: 扩展。新增 L0 Contracts 层(比 L1 Core 更底层),分层映射从 L1→L10 扩展为 L0→L10。
- **§2.2 依赖铁律**: 扩展。新增 `L(N) → L(0) ✓ 恒允许` 与 `L(0) → L(M) ✗ 禁止` 两条款;L0 零依赖铁律保证契约根不退化。
- **§3.3.1 第 4 条(领域类型稳定性)**: 符合。`OmniSparseMasks` 上提经本 ADR 记录;L0 为其提供稳定契约归宿。
- **§3.3.1 第 5 条(向后兼容)**: 符合。re-export 兼容(决策 4),minor 版本(决策 6)。
- **§3.3.1 第 6 条(新 crate 准入)**: 符合。新建 `nexus-contracts` 经本 ADR 审批,同步更新 `CODE_WIKI.md §3.1` 索引。
- **§3.4.1 第 4 条(领域类型稳定性)**: 符合。`OmniSparseMasks` 变更(所有权转移)经 ADR 记录。
- **§4.1 编码规范**: 符合。`#![forbid(unsafe_code)]` crate 级;workspace 级依赖;库层 thiserror(`ContractsError`);无生产路径 unwrap/expect。
- **§6.1 架构红线(功能乱)**: 符合。不引入功能标志;L0 纯类型不承载逻辑,避免功能重叠。
- **§6.2 Week 1-8 实战新红线**: 符合。`#![forbid(unsafe_code)]` 保持;不引入 unsafe 依赖(L0 仅 serde)。
- **ADR-026 决策 7**: 保持。`OmniSparseMasks` 所有权转移至 L0,但 `compute_masks()` 计算逻辑仍由 osa-coordinator 独占(Ω-Sparse 单一实现不变)。
- **ADR-028 决策 7.5**: 保持。`OmniSparseMasks` 类型本身不变(字段/语义不变),仅所有权与定义位置变更,re-export 保证消费者零感知。
- **ADR-031**: 承接。`HarnessSpec`/`ContractSpec` 契约在 L0 落地,为 Harness-as-Spec DSL 提供统一反序列化类型定义点。

## 相关文档

- **设计文档**: v5.0 设计文档 §3(设计总纲)/§6(环间膜)/§11(深模块候选项)/D4(星型耦合病理)/D6(策略移入 learner)/C6(内环外环决策)
- **规则**: `.trae/rules/nuxus规则.md` §2.1(分层映射)/§2.2(依赖铁律,本 ADR 扩展)/§3.3.1(第二阶段开发原则)/§3.4.1(第三阶段开发原则)/§4.1(编码规范)/§4.2(模块组织模式)/§6.1(架构红线)/§6.2(Week 1-8 新红线)
- **CODE_WIKI.md**: `docs/architecture/CODE_WIKI.md` §2.1(分层映射,新增 L0)/§3.1(crate 索引,新增 nexus-contracts)
- **ADR 索引**: `docs/architecture/adr_index.md`(本 ADR 同步更新)
- **关联 ADR**: ADR-026(MAS 子系统,OmniSparseCoordinator::compute_masks 消费者)/ADR-028(Part II 闭环,OmniSparseMasks 类型稳定性约束)/ADR-031(Harness-as-Spec,HarnessSpec/ContractSpec 契约来源)/ADR-005(持久化存储,内存 KNN 先例)/ADR-012(内存安全,forbid unsafe 全覆盖)
- **代码基线**: [coordinator.rs](file:///D:/Chimera%20CLI/crates/osa-coordinator/src/coordinator.rs#L44-L145)(OmniSparseMasks 当前定义)/[types.rs](file:///D:/Chimera%20CLI/crates/osa-coordinator/src/types.rs#L25)(id_newtype 宏)/[kvbsr-router/Cargo.toml](file:///D:/Chimera%20CLI/crates/kvbsr-router/Cargo.toml#L6-L10)/[faae-router/Cargo.toml](file:///D:/Chimera%20CLI/crates/faae-router/Cargo.toml#L6-L10)/[sesa-router/Cargo.toml](file:///D:/Chimera%20CLI/crates/sesa-router/Cargo.toml#L6-L22)(3 router 依赖基线)/[chimera-mas/Cargo.toml](file:///D:/Chimera%20CLI/crates/chimera-mas/Cargo.toml#L52)(真实消费者)/[根 Cargo.toml](file:///D:/Chimera%20CLI/Cargo.toml#L14-L27)(workspace members)

## 追加：L0 性质订正 ——「零逻辑」失真，正确口径是「含受控纯逻辑」（2026-10-09；v3.5 W6-3；只加注，上方决策 1 与「合规性」§6.1 原文逐字保留）

**为何必须加注**：决策 1 把本 crate 性质写成「纯类型 + **零逻辑**」，“合规性”§6.1 行进一步写「L0 纯类型不承载逻辑」。逐模块量体后发现：**该断言对两个模块失真、对一个模块成立**（取证脚本 `tmp/l0_fn_bodies.py`，可重跑）：

| 模块 | 非测试**有体**函数 | trait 无体声明 | 最长真实体 | IO / async / 网络 / 随机 | 「零逻辑」是否成立 |
|---|---|---|---|---|---|
| `capability_token.rs` | **27** | 0 | `maybe_promote` 33 行；`allows_learned_policy` 27；`maybe_recover_from_cooldown` 27；`trigger_asa_intervention` 20 | 0 / 0 / 0 / 0 | **不成立**（EWMA 灰度状态机，`lib.rs` 例外表已登记为例外 3） |
| `vector.rs` | **7** | 8（`upsert`/`top_k`/`remove`/`insert_batch`/`compact`/`default`） | 均 ≤7 行（`is_near_capacity`/`capacity_tier`/`as_str`） | 0 / 0 / 0 / 0 | **成立**（trait 抽象 + 微助手） |
| `harness_spec.rs` | **13** | 0 | `validate` **143 行**、`canonical_merkle_input` **95 行**、`fmt` 34 | 0 / 0 / 0 / 0 | **不成立**（规格校验 + Merkle 规范化算法） |

**口径自身也要订正两次（留痕不静默删）**：
1. 上游方案写的「capability_token 70 / vector 62 / harness_spec 52 个 `pub fn`」——`pub fn` 实为 **20 / 7 / 4**；70/62/52 是**含私有 + 含 trait 声明 + 含 `cfg(test)` 模块**的 `fn` 总数。
2. 本追加节的**首版**也用同一粗计口径写了「`upsert` 66 行」——那其实是 **trait 无体声明**被括号配对扫到了下一个函数。现以「非测试有体函数」为唯一计逻辑单位的口径（上表）。教训与本批 M6 同型：**判“有多少逻辑”前，先确认计数口径是不是你要的那个东西**。

**仍然成立、不必回退的部分**：L0 零外部业务依赖、仅 `serde` 派生、`#![forbid(unsafe_code)]`、`L(N) → L(0)` 恒允许 / `L(0) → L(M)` 禁止的方向铁律——三个模块 **0 IO、0 async、0 网络、0 随机**，均为输入确定即输出确定的纯计算。

**失真的部分**：`harness_spec::validate`（143 行，34 处 `if/match` + 16 处循环 + 143 处算术/位运算）与 `capability_token` 的灰度判定确凿是逻辑。称它们「零逻辑」会让下一个施工者以为 **L0 不可能有行为缺陷**，从而跳过对它们的测试与评审——这比数字写错严重。（注：这两个模块的**例外登记已经存在**——`lib.rs:111-119` 例外表已列 `CapabilityToken` EWMA=例外 3 与多个“纯函数先例”；失真的是“零逻辑”这个**总括词**，不是例外机制本身。）

**此后引用本 ADR 性质时一律采用下述口径**（`CODE_WIKI`、两份规则文件与三个模块头已同步）：

> L0 Contracts = 纯类型 + **受控纯逻辑**：无 IO、无 async、无网络、无随机、无外部业务依赖（仅 `serde` 派生）；
> 允许存在的逻辑限于**契约自身语义**（校验 / 规范化 / 哈希 / 参考实现），逐模块在 `lib.rs` 例外表登记，
> 并受 WI-G1 三硬条件约束（`docs/reports/wi-g1-pure-fn-exception-audit-2026-10-07.md`）。
> **不得再整体引用「零逻辑」**——它已被量体证伪；`vector.rs` 类的“确实零逻辑”也不得反过来当作全 crate 的样本。

**诚实登记的残留缺口**：上述“受控”二字今日**无机器门**——`check_perf_redlines.py` 的 I1–I8c 均不覆盖函数纯度，L0 逻辑的三硬条件仍靠 WI-G1 人工审计与单测。把「受控逻辑」升格为机器判据（例如禁 `std::fs`/`async fn`/`tokio::spawn`/网络/随机符号出现在 `crates/nexus-contracts/src/**`）属新门，需先查 ADR 号再立（参 `adr_index.md`，新号自 ADR-202 起）。
**本加注未做的事**：未将 `validate` 一类算法下沉 L1+（触核心领域类型搬迁，需另立 ADR，同 SeamId 双源上提一类）；未改动任何代码、依赖方向或门判定。

---

> **维护者**: NEXUS-OMEGA 团队
> **创建日期**: 2026-07-23
> **基线版本**: v2.3.1-omega(创建时)→ v2.4.0-omega(本增量落地后)
> **决策者**: E01 首席架构师 + E03 记忆系统专家 + E04 路由算法专家(分布式评审)
> **分析团队**: 3 专家视角分布式深度分析(架构设计 + 记忆系统 + 路由算法)
