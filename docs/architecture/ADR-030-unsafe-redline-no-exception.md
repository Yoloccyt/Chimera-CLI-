# ADR-030: `#![forbid(unsafe_code)]` 红线不特批,安全等价物重写

> **🔄 v2.4.0-omega 基线增补**(2026-07-26):本文档原始生效范围为 v2.3.1-omega 全 workspace **35 crate**;v2.4.0-omega 后 workspace 演进为 **37 crate**(新增 L0 `nexus-contracts` + L6 `omega-learner`),`#![forbid(unsafe_code)]` 红线一并延伸至 37/37 crate 全合规(E01 2026-07-26 穷举 37 个 Cargo.toml 验证)。决策核心(arc-swap/crossbeam/bumpalo 安全等价物清单 + unsafe 不传播边界判定)对 37 crate 全部适用,无需重写。

## 状态

已批准 (Accepted, 2026-07-23)

- **决策者**: E01 首席架构师 + E02 安全架构师(分布式深度评审)
- **评审方式**: 8 专家视角分布式结构化思考与多轮交叉验证(架构 / 安全 / Rust 内存模型 / 性能 / 合规审计)
- **生效范围**: 全 workspace 35 crate(含未来新增 crate `nexus-contracts` / `omega-learner`)
- **不可进化面等级**: 列入 v5.0 设计文档 §7.2 不可进化面 13 条红线之一

## 背景

### 1. 既有红线与历史降级

NEXUS-OMEGA 自第一阶段(8 周推进)起即确立核心编码铁律:**所有 35 crate 必须 `#![forbid(unsafe_code)]`**(crate 级属性,不传播到依赖)。该铁律记于 `.trae/rules/nuxus规则.md` §4.1 通用约定与 §6.2 Week 1-8 实战新红线,是项目"安全优先于性能"哲学的编译期保证。

历史上有两处与 unsafe 相关的降级决策,均为"**移除 unsafe 依赖、改用 safe 等价物**"方向:

1. **ADR-005(持久化存储选型)**: `sqlite-vec 0.1.9` 的 binding 需 `unsafe`,违反 crate 级 `#![forbid(unsafe_code)]`。决策降级为内存 KNN(10-1000 entry scale),性能由 `wiki_knn@1000 < 10ms` / `wiki_knn@10 < 1ms` criterion 基准可证伪(见 ADR-028 决策 5)。
2. **ADR-001(沙箱运行时选择)**: 原设计 Linux 生产环境采用 gVisor(runsc)+ seccomp。当前实现为降级版本 — 完全基于 [tokio::process::Command](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs#L25),通过 [env_clear()](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs#L195) / [kill_on_drop(true)](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs#L206) / [tokio::time::timeout](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs#L212) 实现进程隔离,零 unsafe、零 native syscall 绑定。

经核验 `crates/seccore/Cargo.toml`,seccore 依赖清单仅含 `sha2` / `hex` / `thiserror` / `serde` / `tokio` / `chrono` / `tracing` / `event-bus`(见 [seccore/Cargo.toml](file:///D:/Chimera CLI/crates/seccore/Cargo.toml)),**确无 wasmtime 依赖**,与 v5.0 §10 安全架构的 wasmtime 重启路径(ADR-035 详述)尚未落地的事实一致。

### 2. v5.0 设计文档的新压力

v5.0 设计文档在两处对本红线构成压力:

1. **§10 安全架构**: 提出 wasmtime 沙箱重启路径,用于替代当前降级版 `tokio::process::Command` 沙箱,实现内核级隔离。wasmtime 自身提供 safe API,其内部 `unsafe` 封装在 wasmtime crate 内部,**不传播**到调用方 crate — 这意味着 seccore 引入 wasmtime 不会违反 crate 级 `#![forbid(unsafe_code)]`。
2. **§7.2 不可进化面**: C5 决策明确要求"**unsafe 红线不特批**"作为不可进化面 13 条红线之一。即:禁止任何 crate 通过 `#[allow(unsafe_code)]` 在性能关键路径特批绕过 forbid;新增 crate 必须从设计阶段就采用 safe 等价物。

### 3. 决策触发点

v5.0 §7.2 C5 决策虽已确立"不特批"原则,但缺乏工程落地层面的"安全等价物清单"与"既有红线守护机制"。本 ADR 将该原则转化为可执行的 4 项架构决策,并明确:

- safe 等价物的具体技术选型(覆盖文件 / 进程 / 向量 / 沙箱 / 并发五类场景)
- wasmtime 等第三方 crate 内部 unsafe 的"不传播"边界(避免误判 seccore 合规性)
- 本决策与 ADR-001 / ADR-005 / ADR-035 的衔接关系

> **ADR 编号确认**: `docs/architecture/adr_index.md` 现有最大编号为 ADR-028(MAS Part II 闭环)。本 ADR 编号确认为 **ADR-030**(ADR-029 已由 TUI v3.1 引擎特性开关占用,见 `nuxus规则.md` §1.3 v3-engine 条目)。

## 决策

经专家团队多轮结构化思考与多路径交叉验证,对 `#![forbid(unsafe_code)]` 红线不特批作出以下 4 项架构决策:

### 决策 1: 不特批原则 — 禁止任何 crate 绕过 forbid

**核心条款**:

1. **禁止 `#[allow(unsafe_code)]` 绕过**: 任何 crate 不得通过 `#[allow(unsafe_code)]` 属性在函数 / 模块 / crate 任何粒度绕过 `#![forbid(unsafe_code)]`。`forbid` 与 `allow` 同时存在时编译器会报 `forbid` 覆盖 `allow` 的硬错误,本决策将该编译器行为上升为架构红线。
2. **新增 crate 设计阶段准入**: 未来新增 crate(`nexus-contracts` / `omega-learner` 等)必须在设计阶段即采用 safe 等价物,不得在实现阶段因性能压力回退到 unsafe。设计评审清单新增"unsafe 等价物选型"必答项。
3. **既有 35 crate 100% 守护**: 当前 35 crate × `#![forbid(unsafe_code)]` 100% 合规(经 §3.4.1 第 6 条性能可证伪要求下的多次审计核验),本决策将其锁定为不可逆状态。

### 决策 2: 安全等价物清单 — 五类场景的 safe 替代

针对常见的 unsafe 诱惑场景,确立以下 safe 等价物清单作为新 crate 设计与既有 crate 演进的强制选型:

| 场景 | unsafe 诱惑(禁用) | safe 等价物(强制) | 落地证据 |
|------|-------------------|-------------------|---------|
| 文件操作 | `memmap2` / 手写 `mmap` unsafe | `std::fs` + `tokio::fs` | repo-wiki / scc-cache 全部采用 async fs |
| 进程隔离 | `nix` 原生 syscall / 手写 `fork+exec` | `tokio::process::Command` + `env_clear` + `kill_on_drop` | [sandbox.rs:L195](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs#L195) env_clear / [sandbox.rs:L206](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs#L206) kill_on_drop |
| 向量计算 | raw pointer 指针算术 / 手写 SIMD intrinsics | `ndarray` safe API + `select_nth_unstable`(Top-K) | §4.1 Top-K 红线;mlc-engine / repo-wiki KNN 实现 |
| 沙箱执行 | `libseccomp` binding / 手写 seccomp BPF | wasmtime safe API(unsafe 封装在 wasmtime crate,不传播) | ADR-035 wasmtime 重启路径;seccore 当前降级版见 [sandbox.rs:L181](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs#L181) |
| 并发原语 | 手写 `AtomicU64` + `UnsafeCell` / 自旋锁 | `tokio::sync` / `parking_lot` / `dashmap` | §4.4 async 反模式清单;faae-router DashMap 用法 |

### 决策 3: 第三方 crate unsafe 不传播边界

明确 `#![forbid(unsafe_code)]` 的"不传播"语义边界,避免误判合规性:

1. **forbid 不传播到依赖**: crate 级 `#![forbid(unsafe_code)]` 只约束**当前 crate 源码**,不传播到其依赖(见 `nuxus规则.md` §4.1)。因此:
   - `rusqlite` bundled 内部 unsafe 不影响当前 crate 合规
   - `wasmtime` 内部 unsafe 不影响 seccore 合规(待 ADR-035 落地)
   - `hnsw_rs` / `parking_lot` / `dashmap` 等内部 unsafe 均不影响当前 crate 合规
2. **合规判定标准**: 判定某 crate 是否合规,以 `cargo check` 是否对该 crate 源码报 `unsafe_code` lint 为准,不以依赖树中是否存在 unsafe 为准。
3. **依赖准入审查**: 新增外部依赖必须审查其是否暴露 unsafe API 给当前 crate。若其公开 API 要求调用方写 unsafe(如 `sqlite-vec 0.1.9`),则该依赖违反本红线,必须改用 safe 替代(见 ADR-005 降级)。

### 决策 4: 不可进化面锁定 — 列入 v5.0 §7.2 红线

本决策列入 v5.0 设计文档 §7.2 不可进化面 13 条红线之一,与下列既有不可进化项并列:

- Critical 安全事件清单(SkepticVeto / RedTeamAudit / AsaIntervention / BudgetExceeded / AgentTaskFailed)
- INV-7 / INV-8 / INV-9 不变量(MAS 上下文预算 / 归档单调性 / 派生上限)
- 沙箱 / QEEP 量子纠缠协议本身
- 验证器层级本身(GSOE / AutoDPO 执行反馈验证器)

**不可进化语义**: 本决策一旦批准,不得通过后续 ADR 以"性能优化"为由放宽。任何对 `#![forbid(unsafe_code)]` 的修改提案必须先修订 v5.0 §7.2 不可进化面,属于 major 版本破坏性变更,需经 8 专家全票通过。

## 理由

### 决策 1 理由(不特批原则)

- **§6.1 架构红线(裸奔)**: Claude Code 尸检教训"命令插值 + auth 跳过",本项目红线"所有外部调用经 SecCore 沙箱 + Decay 衰减"。`#[allow(unsafe_code)]` 特批是"裸奔"的编译期入口,一旦开口子即破坏红线一致性。
- **§3.4.1 第 1 条(OMEGA 四定律守恒)**: OMEGA 四定律之首是安全(Ω-Sparse 的审计维度),unsafe 是对内存安全的根本性破坏,与四定律不可调和。
- **编译期保证优于运行时审计**: `#![forbid(unsafe_code)]` 是编译期硬约束,`cargo check` 自动拦截;若改为运行时审计,需付出持续的人力审查成本且无法保证覆盖率。C5 决策的"不特批"正是将编译期保证锁死。
- **历史教训**: ADR-005 的 sqlite-vec 降级证明,unsafe 依赖的引入是"一次妥协、长期偿还" — 若非当时坚决降级,后续每次向量检索审计都需复核 unsafe 边界。

### 决策 2 理由(安全等价物清单)

- **§4.1 编码规范**: 已明确 "Top-K 选择必须用 `select_nth_unstable` (O(n)) — 禁止 `sort_by` (O(n log n))"。本决策将该思路推广到五类场景,形成系统化等价物清单。
- **§6.2 红线**: "sqlite-vec 禁用(违反 forbid unsafe)" 已是既定红线,本决策将其泛化为"任何 unsafe 依赖禁用"。
- **性能可证伪**: safe 等价物的性能劣势由 criterion 基准可证伪。例如内存 KNN 在 10-1000 entry scale 的 `wiki_knn@1000 < 10ms`(ADR-028 决策 5)已证明 safe 方案在项目实际 scale 下性能足够;若未来 scale 突破,应优先考虑算法优化(如 HNSW safe 实现 `hnsw_rs`)而非回退 unsafe。
- **沙箱场景的特殊性**: wasmtime safe API 是本清单的关键例外 — wasmtime 内部用 unsafe 实现沙箱隔离,但其公开 API 全 safe,unsafe 封装在 wasmtime crate 内部不传播。这正是 ADR-035 wasmtime 重启路径可行的理论基础。

### 决策 3 理由(不传播边界)

- **Rust 语言语义**: `#![forbid(unsafe_code)]` 的官方语义即"只约束当前 crate,不传播到依赖"(Rust Reference §8.4)。本决策重申该语义,避免项目内部误判。
- **审计成本**: 若以"依赖树中存在 unsafe"为合规标准,则几乎所有 Rust 项目都不合规(标准库本身含 unsafe),审计无法执行。以"当前 crate 源码不报 lint"为标准,可由 `cargo check` 自动化。
- **依赖准入的实操性**: 决策 3.3 将审查焦点从"依赖内部是否 unsafe"收窄到"依赖公开 API 是否要求调用方写 unsafe",这是可机械化检测的(检查依赖的 pub fn 签名是否含 unsafe)。

### 决策 4 理由(不可进化面锁定)

- **§3.4.1 第 4 条(领域类型稳定性)**: 核心领域类型变更需 ADR 记录。本决策将 unsafe 红线提升至与领域类型同等的稳定性等级 — 不仅是 ADR 记录,而是"不可进化"。
- **三重悖论红线(§3.4.5)**: 进化悖论红线指出"当前 GSOE/AutoDPO 使用执行反馈作为验证信号,存在被奖励黑客游戏化风险"。unsafe 红线若可进化,则验证器本身可能被 unsafe 破坏,使进化悖论雪上加霜。将 unsafe 锁为不可进化面,是验证器层级跃迁(向 L4 形式化验证)的前提。
- **降低未来博弈成本**: 不可进化面一旦确立,后续任何"为性能引入 unsafe"的提案都不需重复论证红线本身,只需论证"是否值得修订不可进化面",将博弈成本前置。

## 影响

### 正向影响

- **编译期内存安全保证**: 35 crate × `#![forbid(unsafe_code)]` 由 `cargo check` 编译期强制,无需运行时 sanitizers 即可保证无内存安全漏洞(数据竞争 / use-after-free / buffer overflow / null deref)。
- **CI lint 自动拦截**: `audit.yml` 与 `release.yml` 的 `cargo check --workspace` 步骤自动拦截任何 unsafe 引入,无需额外 lint 规则。
- **审计成本降低**: 安全审计(§8.3 `tests/security/owasp_top10.rs`)无需复核 unsafe 边界,审计焦点可集中于业务逻辑漏洞。
- **品牌一致性**: `#![forbid(unsafe_code)]` 全覆盖是项目"安全优先"哲学的可证伪证据,与 Dockerfile `nonroot:nonroot` USER(§10.2)形成"编译期 + 运行时"双重安全叙事。

### 负向影响

- **部分性能场景需算法优化替代**: 手写 SIMD / raw pointer 算术在向量计算密集场景的理论性能上限更高。本决策要求以 `ndarray` safe API + `select_nth_unstable` 替代,在超大 scale(> 10^6 entry)下可能存在性能差距。缓解: §3.4.1 第 6 条要求性能优化必须有 criterion 证据,若 safe 等价物在目标 scale 不达阈值,应优先考虑算法层优化(如 HNSW)而非 unsafe。
- **沙箱隔离强度受限**: 当前降级版 `tokio::process::Command` 沙箱(见 [sandbox.rs:L181](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs#L181))弱于 gVisor 内核级隔离。缓解: ADR-035 wasmtime 重启路径将在不违反本红线的前提下(决策 3.1 不传播边界)提升隔离强度。
- **依赖选型约束**: 部分高性能 crate(如 `sqlite-vec` / 某些 FFI binding)被排除,需寻找 safe 替代或自行实现。缓解: 决策 2 安全等价物清单已覆盖五类高频场景。

### 中性影响

- **wasmtime / hnsw_rs 等第三方 crate 内部 unsafe 不影响合规**: 由决策 3 不传播边界保证。这既是正向(扩大可选依赖范围)也是中性(需审查依赖公开 API 是否要求调用方 unsafe,见决策 3.3)。

### 资源影响评估

| 维度 | 评估 |
|------|------|
| crate 数量 | 35(不变,本 ADR 不新增 crate) |
| 依赖变更 | 无(本 ADR 是约束性决策,不引入新依赖) |
| Docker / binary 体积 | 不受影响 |
| NexusEvent 变体数 | 不变 |
| MasError 变体数 | 不变 |
| 测试覆盖 | 不直接新增测试,但 `audit.yml` 的 `cargo check --workspace` 步骤成为本红线的自动守护 |
| criterion 基准 | 不直接新增,但未来 safe 等价物选型需附基准证据(§3.4.1 第 6 条) |
| 版本号 | 不直接升版本(本 ADR 是架构约束,非功能增量) |
| CI 守护 | `cargo check --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` 自动拦截 unsafe(§7.1 日常命令) |

## 考虑的方案

### 方案 A: 完全 forbid + safe 等价物重写(采纳)

- **内容**: 所有 35 crate 维持 `#![forbid(unsafe_code)]`,unsafe 诱惑场景由决策 2 安全等价物清单替代,第三方 unsafe 经决策 3 不传播边界判定合规。
- **采纳理由**:
  1. 符合 v5.0 §7.2 C5 决策"unsafe 红线不特批"
  2. 编译期保证 + CI 自动拦截,审计成本最低
  3. 与 ADR-005 sqlite-vec 降级 / ADR-001 沙箱降级方向一致(均为移除 unsafe 依赖)
  4. safe 等价物在项目实际 scale(10-1000 entry / 50 Agent / 1M 上下文)下性能可证伪足够

### 方案 B: 允许 `#[allow(unsafe_code)]` 在性能关键路径特批(否决)

- **内容**: 在 P0 性能关键路径(如向量检索 / 哈希计算 / 序列化)允许经 ADR 审批的 `#[allow(unsafe_code)]` 特批,换取理论性能上限。
- **否决理由**:
  1. **违反 v5.0 §7.2 C5 决策**: C5 明确"unsafe 红线不特批"是不可进化面,本方案直接推翻该决策
  2. **破坏红线一致性**: 一旦开口子,特批边界难以界定(何为"性能关键"?由谁裁定?),导致红线逐步侵蚀(boiling frog)
  3. **审计成本上升**: 每处特批需独立审查 unsafe 块的正确性,无法由 `cargo check` 自动化
  4. **与既有降级方向矛盾**: ADR-005 已为 sqlite-vec 付出降级成本,若现在允许特批,则该降级决策的牺牲失去意义
  5. **三重悖论风险**: unsafe 特批可能被"奖励黑客"利用(进化悖论红线),绕过安全护栏

### 方案 C: 通过 FFI 调用 C 库绕过(否决)

- **内容**: 对性能瓶颈场景,通过 FFI 调用 C 库(如 `blake3` C binding / 自写 C 向量库),在 Rust 侧用 unsafe FFI 包装。
- **否决理由**:
  1. **违反 §2.2 依赖铁律**: FFI 引入跨语言边界,破坏 Rust 单一语言栈的一致性,增加构建复杂度(需 C 工具链)
  2. **引入 unsafe 传播**: FFI 包装本身即 unsafe,直接违反决策 1 不特批原则
  3. **CI 守护失效**: `cargo audit` 无法审计 C 库漏洞,RUSTSEC 不覆盖 C 生态
  4. **跨平台风险**: C 库在 Windows GNU / Linux / macOS 三平台的编译行为差异大,违反 §10.1 5 平台 matrix build 准入门槛
  5. **Docker 镜像膨胀**: C 库需额外动态 / 静态链接,违反 §10.2 镜像 < 100MB 约束

## 合规性

- **§2.1 分层映射**: 符合。本决策是横向约束,不改变 L1-L10 分层。
- **§2.2 依赖铁律**: 符合。本决策不引入跨层依赖,反而通过禁止 FFI(方案 C 否决)强化铁律。
- **§4.1 通用约定**: 符合。重申 `#![forbid(unsafe_code)]` crate 级属性(决策 1),重申 Top-K 用 `select_nth_unstable`(决策 2 向量计算行)。
- **§4.4 async 反模式**: 符合。决策 2 并发原语行强制 `tokio::sync` / `parking_lot` / `dashmap`,与 §4.4 "禁止持锁 .await" 红线一致。
- **§6.1 架构红线(裸奔)**: 符合。`#![forbid(unsafe_code)]` 是"所有外部调用经 SecCore 沙箱 + Decay 衰减"的编译期前置条件。
- **§6.2 Week 1-8 实战新红线**: 符合。本决策将 §6.2 "sqlite-vec 禁用(违反 forbid unsafe)" 红线泛化为"任何 unsafe 依赖禁用"(决策 2),并新增"unsafe 红线不特批"作为不可进化面(决策 4)。
- **§3.4.1 第 6 条(性能可证伪)**: 符合。决策 2 安全等价物的性能由 criterion 基准可证伪(如 `wiki_knn@1000 < 10ms`)。
- **§3.4.1 第 4 条(领域类型稳定性)**: 符合。本决策不改任何核心领域类型,但将 unsafe 红线提升至与领域类型同等的稳定性等级(决策 4 不可进化面)。
- **§3.4.5 三重悖论红线**: 符合。决策 4 不可进化面锁定防止 unsafe 破坏验证器层级,是进化悖论红线的前置保障。
- **§10.1 CI/CD 准入门槛**: 符合。`audit.yml` + `release.yml` 的 `cargo check --workspace` / `cargo clippy -- -D warnings` 步骤自动守护本红线。
- **§10.2 Docker 镜像约束**: 符合。方案 C 否决避免 C 库链接导致镜像膨胀,镜像 < 100MB 约束保持。
- **ADR-001 沙箱运行时选择**: 保持。本决策不推翻 gVisor 设计目标,当前降级版 [sandbox.rs](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs) 的 `tokio::process::Command` 实现符合决策 2 进程隔离行;wasmtime 升级路径由 ADR-035 在本红线约束内推进。
- **ADR-005 持久化存储选型**: 保持。本决策将 ADR-005 的 sqlite-vec 降级泛化为系统化原则,内存 KNN 方案不变。
- **ADR-026 / ADR-027 / ADR-028**: 保持。chimera-mas 全部子模块遵循 `#![forbid(unsafe_code)]`,本决策不修改 MAS 任何签名。
- **ADR-029 TUI v3.1 引擎**: 保持。`v3-engine` feature 与本决策无冲突。

## 相关文档

- **设计文档**: v5.0 设计文档 §7.2 不可进化面(C5 决策)/ §10 安全架构(wasmtime 重启路径)
- **规则**: `.trae/rules/nuxus规则.md` §2.2(依赖铁律)/ §4.1(通用约定,`#![forbid(unsafe_code)]`)/ §4.4(async 反模式)/ §6.1(架构红线)/ §6.2(Week 1-8 新红线,sqlite-vec 禁用)/ §3.4.1(第三阶段开发原则)/ §3.4.5(三重悖论红线)/ §10.1(CI/CD 准入)/ §10.2(Docker 镜像约束)
- **CODE_WIKI.md**: `docs/architecture/CODE_WIKI.md` §3.1(crate 索引)/ §2.3(ADR 决策参考)
- **ADR 索引**: `docs/architecture/adr_index.md`(本 ADR 同步更新,总数 29 → 30)
- **代码证据**:
  - [seccore/Cargo.toml](file:///D:/Chimera CLI/crates/seccore/Cargo.toml)(零 wasmtime 依赖核验)
  - [sandbox.rs:L25](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs#L25)(`tokio::process::Command` 导入)
  - [sandbox.rs:L181](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs#L181)(`execute_in_sandbox` 降级实现)
  - [sandbox.rs:L195](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs#L195)(`env_clear` 零信任环境隔离)
  - [sandbox.rs:L206](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs#L206)(`kill_on_drop` 孤儿进程防护)
  - [sandbox.rs:L212](file:///D:/Chimera CLI/crates/seccore/src/sandbox.rs#L212)(`tokio::time::timeout` DoS 防护)
- **关联 ADR**:
  - ADR-001(沙箱运行时选择,gVisor,当前降级为 tokio::process::Command)
  - ADR-005(持久化存储选型,sqlite-vec 降级为内存 KNN,本 ADR 的先例)
  - ADR-029(TUI v3.1 引擎特性开关,占用 ADR-029 编号)
  - ADR-035(威胁模型下修 + wasmtime 重启路径,本 ADR wasmtime 安全性的细化,待落地)

---

> **维护者**: NEXUS-OMEGA 团队
> **创建日期**: 2026-07-23
> **基线版本**: v2.3.1-omega
> **分析团队**: 8 专家视角分布式深度分析(E01 首席架构师主导 + E02 安全架构师联合评审 + 架构优化 / Rust 架构 / 代码审查 / TDD 守护 / 性能优化 / DevOps 可观测)
> **不可进化面**: 本决策列入 v5.0 §7.2 不可进化面 13 条红线之一,禁止以"性能优化"为由放宽
