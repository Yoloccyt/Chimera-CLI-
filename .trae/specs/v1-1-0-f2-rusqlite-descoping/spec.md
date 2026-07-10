# v1.1.0-omega F2 rusqlite 下沉 Spec

## Why

v1.0.0-omega GA 已发布(2026-07-04),进入 v1.1.0 开发周期。路线图 F2(rusqlite 下沉)是 Phase 1 最高优先级,预算 80 工时(2 周)。

**核心问题**:`nexus-core`(L1 Core)直接依赖 `rusqlite`,违反 §2.2 依赖铁律的"L1 必须保持最小依赖"原则。虽然 nexus-core 仅提供 `apply_performance_pragmas` 工具函数(56 行,无 SQL 执行),但:

1. **架构纯净度受损**:L1 应仅包含领域类型(UserIntent / Quest / CLV / NexusState),不应有任何 IO/存储依赖。当前 `nexus-core/Cargo.toml` 直接 `rusqlite = { workspace = true }`,`error.rs` 含 `SqliteError(#[from] rusqlite::Error)` 变体,使 L1 与具体存储引擎耦合。
2. **阻塞 F1 L10 编排接线**:F1(L10 真实编排)是 v1.1.0 Must 项,依赖 F2.3 重构完成。L10 编排器不应在依赖图中看到 L1 的 rusqlite 痕迹。
3. **现状审计已闭合 F2.1.1**:nexus-core 仅 4 处 rusqlite 使用(Cargo.toml / lib.rs / error.rs / sqlite_pragma.rs),下游仅 3 处调用(cmt-tiering × 2 + mlc-engine × 1),实际工作量小于路线图预估。

**重要说明**:
- 下沉目标 crate 待 ADR-006 决策,候选方案见 ADR-006(候选:`scc-cache` / `lsct-tiering` / 新建 `storage-backbone` crate)
- quest-engine 不依赖 sqlite_pragma(路线图 F2.3.4 提及,但审计证伪),实际下游仅 `mlc-engine` 与 `cmt-tiering`
- nexus-core 不直接执行 SQL,仅提供 PRAGMA 工具函数,F2.3 重构工作量小于路线图预估

## What Changes

### Must(阻塞 v1.1.0 发布)

- **M1:nexus-core 零 rusqlite 依赖**:移除 `crates/nexus-core/Cargo.toml` 的 `rusqlite` 依赖,删除 `crates/nexus-core/src/sqlite_pragma.rs`,移除 `error.rs` 的 `SqliteError(#[from] rusqlite::Error)` 变体,更新 `lib.rs` 模块声明。`cargo tree -p nexus-core` 与 `cargo tree -p nexus-core --all-features` 均不再出现 `rusqlite`
- **M2:nexus-core(L1)定义 PragmaCapable trait + apply_performance_pragmas 泛型函数**:在 `crates/nexus-core/src/storage_traits.rs`(新建模块,L1 不依赖 rusqlite)定义 `pub trait PragmaCapable`,含 `pragma_update_string` / `pragma_update_int` 两个方法;并实现 `pub fn apply_performance_pragmas<T: PragmaCapable>(conn: &T) -> Result<(), NexusError>` 泛型函数,内部通过 trait 方法设置原 5 个 PRAGMA(synchronous=NORMAL / cache_size=-65536 / mmap_size=268435456 / temp_store=MEMORY / wal_autocheckpoint=1000)。trait 与泛型函数均不引用任何 rusqlite 类型,L1 保持纯净
- **M3:下游 2 个 crate 以 newtype wrapper 实现 PragmaCapable**:在 `cmt-tiering` / `mlc-engine` 各自新增 `storage_impl.rs`,定义 `pub struct PragmaConn<'a>(pub &'a rusqlite::Connection)` newtype wrapper,并实现 `impl<'a> nexus_core::PragmaCapable for PragmaConn<'a>`(impl 块在 L2/L3,引用 rusqlite 合规)。将 `rusqlite::Connection::pragma_update` 包装为 trait 方法,错误通过 `.map_err()` 转换为 `NexusError::SerializationError`(沿用现有变体,避免 F2.3.3 删除 `SqliteError` 时破坏 impl)。**WHY newtype wrapper**:Rust coherence 规则禁止两个 crate 同时 impl 同一 trait for 同一 type,而根 Cargo.toml 的 E2E 测试同时依赖 cmt-tiering 和 mlc-engine,直接 impl `PragmaCapable for rusqlite::Connection` 会触发 `conflicting implementations`。newtype wrapper 是 spec F2.1.5.6 预置回滚方案,用户 2026-07-08 决策采纳
- **M4:3 处下游调用改为泛型调用 apply_performance_pragmas(&PragmaConn(conn))**:重构 `cmt-tiering/src/warm.rs:464` / `cmt-tiering/src/cold.rs:539` / `mlc-engine/src/l3_procedural.rs:415` 三处 `nexus_core::sqlite_pragma::apply_performance_pragmas` 调用,改为 `nexus_core::apply_performance_pragmas(&PragmaConn(conn))` 泛型调用(单态化,零 trait object 开销),保持现有性能特征(查询延迟降低 30-50%)。调用点在函数内构造 `PragmaConn(conn)` wrapper,函数签名不变
- **M5:全量测试与 lint 通过**:`cargo test --workspace` 全绿(现有测试不删除,新测试覆盖率 ≥ 90%),`cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 零警告,`cargo fmt --all -- --check` 零 diff

### Should(强烈建议)

- **S1:ADR-006 决策文档已采纳方案 E(L1 trait abstraction)**:撰写《ADR-006 rusqlite 下沉决策文档》,记录方案 E(trait 定义在 L1 `nexus-core`,`PragmaCapable` trait + `apply_performance_pragmas<T>` 泛型函数,L2/L3 实现并调用)的选定理由,对比方案 A-D(L3 StorageBackend / 本地复制 PRAGMA / 新建 storage-backbone crate / EventBus 事件)的权衡分析。决策结果:方案 E 既不引入新 crate,也不违反 L2→L3 依赖方向,且零 trait object 运行时开销
- **S2:proptest 验证 PragmaCapable trait 不变量**:为 `PragmaCapable` trait 与 `apply_performance_pragmas<T>` 泛型函数添加 proptest,验证:(a) PRAGMA 幂等性(多次调用结果一致);(b) 泛型调用正确性(对 mock 实现与 rusqlite::Connection 实现行为一致);(c) trait 方法签名稳定性(不引用 rusqlite 类型,L1 编译时无 rusqlite 依赖)
- **S3:CODE_WIKI.md §2.1 L1 层职责同步**:更新 `CODE_WIKI.md` §2.1 L1 层职责描述,移除"提供 sqlite_pragma 工具函数"相关表述,补充"L1 定义 PragmaCapable trait,L2/L3 实现并调用(ADR-006 方案 E)"

### Could(周期内补齐)

- **C1:benches 对比重构前后性能**:为 `apply_performance_pragmas<T: PragmaCapable>` 泛型函数添加 criterion benches,对比重构前后 PRAGMA 设置开销(p95 延迟 ≤ 重构前 +5%),确保 trait 抽象与单态化不引入性能回退
- **C2:cmt-tiering / mlc-engine spawn_blocking 一致性审计**:审计 3 处下游调用点的 `spawn_blocking` 包装一致性(§4.4 第 2 条:rusqlite 调用必须 spawn_blocking),修复任何在 async 上下文直接调用 rusqlite 的反模式

### 不在本 Spec 范围

- F1 L10 真实编排接线(独立 Spec 处理,依赖 F2.3 完成)
- F3-F9 其他 v1.1.0 功能(独立 Spec 处理)
- `PragmaCapable` trait 的完整 SQL 抽象扩展(本 Spec 仅覆盖 PRAGMA 设置方法,`execute_query` / `persist` 等完整 SQL 抽象延后到 v1.2,届时可在 trait 上新增方法而无需破坏性变更)
- 引入新存储引擎(如 PostgreSQL / sled,本 Spec 仅迁移现有 rusqlite)
- 修改 `nexus-core` 的核心领域类型(`UserIntent` / `Quest` / `Checkpoint` / `CLV` / `NexusState` 保持不变)

## Impact

### 受影响 specs

- `v1-0-0-omega-ga-release-sprint`(v1.0.0 GA 已发布,本 Spec 是其 v1.1.0 接续)
- `v1-1-0-f1-l10-orchestration-wiring`(F1 L10 编排,依赖本 Spec F2.3 完成)
- v1.1.0 路线图(`docs/release/v1.1.0_roadmap.md` §2.1 Phase 1)

### 受影响代码

**Must 修改(L1 新增 trait + 移除 rusqlite)**:
- `crates/nexus-core/src/storage_traits.rs`(新建,定义 `PragmaCapable` trait + `apply_performance_pragmas<T>` 泛型函数,不引用 rusqlite)
- `crates/nexus-core/src/lib.rs`(移除 `pub mod sqlite_pragma;`,新增 `pub mod storage_traits;`,更新 `prelude` 导出)
- `crates/nexus-core/src/sqlite_pragma.rs`(删除整个文件,56 行,逻辑迁移到 `storage_traits.rs` 泛型函数)
- `crates/nexus-core/src/error.rs`(移除 `SqliteError(#[from] rusqlite::Error)` 变体,第 36-42 行;`NexusError` 仍保留作下游 `.map_err()` 转换目标,但不再 `#[from]` rusqlite)
- `crates/nexus-core/Cargo.toml`(移除 `rusqlite = { workspace = true }` 依赖声明)

**Must 修改(L2/L3 newtype wrapper + 调用泛型函数)**:
- `crates/cmt-tiering/src/storage_impl.rs`(新建,定义 `pub struct PragmaConn<'a>(pub &'a rusqlite::Connection)` + `impl<'a> PragmaCapable for PragmaConn<'a>`,供 warm.rs / cold.rs 共用)
- `crates/cmt-tiering/src/lib.rs`(新增 `pub mod storage_impl;` + `pub use storage_impl::PragmaConn;`)
- `crates/cmt-tiering/src/warm.rs`(第 459-466 行,改为 `nexus_core::apply_performance_pragmas(&PragmaConn(conn))` 泛型调用,函数内构造 wrapper)
- `crates/cmt-tiering/src/cold.rs`(第 534-541 行同上)
- `crates/mlc-engine/src/storage_impl.rs`(新建,同 cmt-tiering 模式,各 crate 独立 newtype 避免 coherence 冲突)
- `crates/mlc-engine/src/lib.rs`(新增 `pub mod storage_impl;` + `pub use storage_impl::PragmaConn;`)
- `crates/mlc-engine/src/l3_procedural.rs`(第 410-417 行,改为泛型调用)
- `crates/cmt-tiering/Cargo.toml`(已声明 `rusqlite = { workspace = true }`(第 12 行),无需修改)
- `crates/mlc-engine/Cargo.toml`(已声明 `rusqlite = { workspace = true }`(第 12 行),无需修改)

**Should 文档同步**:
- `docs/adr/ADR-006-rusqlite-descoping.md`(新建,记录方案 E 决策与对比 A-D 方案权衡)
- `CODE_WIKI.md`(§2.1 L1 层职责描述更新,§2.3 ADR 表格新增 ADR-006)
- `CHANGELOG.md`(追加 v1.1.0 F2 章节)
- `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md`(追加方案 E trait abstraction 经验教训)

**Could 测试与审计**:
- `crates/nexus-core/benches/storage_traits_bench.rs`(新建,泛型函数性能对比基准)
- `crates/nexus-core/tests/proptest.rs`(新建,trait 不变量验证;proptest 用 mock impl 而非 rusqlite,保持 L1 测试纯净)

### 受影响产物(外部)

- v1.1.0-omega git tag(本 Spec 完成后才能启动 F1 L10 编排)
- v1.1.0-omega Release Notes(F2 章节记录 rusqlite 下沉)
- CI workflow(`release.yml` 触发的 binary 体积可能因依赖图变化而微调,需重新核验 < 50MB 约束)

## ADDED Requirements

### Requirement: nexus-core 零 rusqlite 依赖

`nexus-core` crate SHALL 不再直接或间接依赖 `rusqlite`,L1 Core 仅保留领域类型与纯计算逻辑。

#### Scenario: Cargo.toml 依赖清理

- **WHEN** 检查 `crates/nexus-core/Cargo.toml`
- **THEN** `[dependencies]` 段 MUST 不包含 `rusqlite` 条目
- **AND** `[dev-dependencies]` 段 MUST 不包含 `rusqlite` 条目(测试不应通过 dev-deps 绕过)

#### Scenario: cargo tree 零 rusqlite

- **WHEN** 执行 `cargo tree -p nexus-core`
- **THEN** 输出 MUST 不包含 `rusqlite` 任何版本
- **AND** 执行 `cargo tree -p nexus-core --all-features` 输出 MUST 同样不包含 `rusqlite`
- **AND** 执行 `cargo tree -p nexus-core --all-targets` 输出 MUST 同样不包含 `rusqlite`

#### Scenario: error.rs SqliteError 变体移除

- **WHEN** 检查 `crates/nexus-core/src/error.rs`
- **THEN** `NexusError` enum MUST 不包含 `SqliteError(#[from] rusqlite::Error)` 变体
- **AND** MUST 不包含任何 `rusqlite` import
- **AND** 现有 4 个测试(`test_invalid_clv_dimension_display` / `test_quest_not_found_display` / `test_quest_already_exists_display` / `test_io_error_from`)MUST 全部保留且通过

#### Scenario: sqlite_pragma.rs 删除

- **WHEN** 检查 `crates/nexus-core/src/`
- **THEN** MUST 不存在 `sqlite_pragma.rs` 文件
- **AND** `lib.rs` MUST 不包含 `pub mod sqlite_pragma;` 声明
- **AND** `lib.rs` MUST 包含 `pub mod storage_traits;` 声明(方案 E 新模块)
- **AND** 原 3 个测试(`test_apply_performance_pragmas_success` / `test_cache_size_set` / `test_temp_store_memory`)MUST 迁移到下游 crate(`cmt-tiering` 或 `mlc-engine`)的 `tests/` 目录中,因测试需要真实 `rusqlite::Connection`,L1 不能依赖 rusqlite 故不能保留在 nexus-core;迁移后测试逻辑 MUST 保留(可重命名为 `test_pragma_capable_impl_*` 等)
- **AND** `nexus-core` 自身的新增测试(若需要验证泛型函数逻辑)MUST 使用 mock impl 而非真实 rusqlite,保持 L1 测试纯净

### Requirement: L1 PragmaCapable trait 抽象(方案 E)

`nexus-core`(L1)SHALL 定义 `PragmaCapable` trait 与 `apply_performance_pragmas<T: PragmaCapable>` 泛型函数,trait 与泛型函数均不引用任何 rusqlite 类型;L2/L3 下游 crate 实现 `PragmaCapable for rusqlite::Connection` 并调用泛型函数,严格遵循 §2.2 依赖铁律(L2/L3 向下依赖 L1,合规)。

#### Scenario: PragmaCapable trait 定义

- **WHEN** 实施 `crates/nexus-core/src/storage_traits.rs`
- **THEN** MUST 定义 `pub trait PragmaCapable` 含以下方法:
  - `fn pragma_update_string(&self, key: &str, value: &str) -> Result<(), NexusError>` — 设置字符串型 PRAGMA
  - `fn pragma_update_int(&self, key: &str, value: i64) -> Result<(), NexusError>` — 设置整型 PRAGMA
- **AND** trait MUST 声明在 `crates/nexus-core/src/lib.rs` 公开导出(`pub use storage_traits::PragmaCapable;`)
- **AND** trait 与泛型函数所在文件 MUST 不引用任何 `rusqlite` 类型(`use rusqlite::...` 禁止)
- **AND** crate MUST 保持 `#![forbid(unsafe_code)]`(crate 级属性)

#### Scenario: apply_performance_pragmas 泛型函数实现

- **WHEN** 实施 `pub fn apply_performance_pragmas<T: PragmaCapable>(conn: &T) -> Result<(), NexusError>`
- **THEN** MUST 通过 trait 方法设置原 5 个 PRAGMA:`synchronous=NORMAL` / `cache_size=-65536` / `mmap_size=268435456` / `temp_store=MEMORY` / `wal_autocheckpoint=1000`
- **AND** MUST 在 WAL 模式设置之后调用(沿用原 `sqlite_pragma.rs` 注释约束,文档注释需说明)
- **AND** MUST 使用静态分发(泛型 `<T: PragmaCapable>`,非 `&dyn PragmaCapable`),零 trait object 运行时开销
- **AND** 实现必须保持 `#![forbid(unsafe_code)]`

#### Scenario: 下游 PragmaCapable 实现

- **WHEN** 在 `cmt-tiering` 与 `mlc-engine` 实施 `impl PragmaCapable for rusqlite::Connection`
- **THEN** MUST 在 L2/L3 crate 内部完成 impl(遵循 orphan rule,trait 在 L1 定义、类型在下游 crate 引入)
- **AND** MUST 在 `pragma_update_string` / `pragma_update_int` 内部调用 `rusqlite::Connection::pragma_update(None, key, value)`,错误通过 `.map_err(|e| NexusError::SqliteError(...))` 或等价方式转换(具体错误变体命名由实施时决定,但 MUST 与下游现有错误链兼容)
- **AND** `cmt-tiering` 与 `mlc-engine` 各自的 `Cargo.toml` MUST 显式声明 `rusqlite = { workspace = true }` 依赖(因 L1 已移除,下游需自行声明)

#### Scenario: 下游调用点重构

- **WHEN** 重构 `cmt-tiering/src/warm.rs:464` 的 `apply_performance_pragmas` 函数
- **THEN** MUST 不再调用 `nexus_core::sqlite_pragma::apply_performance_pragmas`
- **AND** MUST 改为调用 `nexus_core::storage_traits::apply_performance_pragmas(conn)` 泛型函数
- **AND** `cmt-tiering/src/cold.rs:539` 与 `mlc-engine/src/l3_procedural.rs:415` MUST 同样重构
- **AND** 重构后 PRAGMA 设置 MUST 仍然生效(查询延迟不显著回退)

### Requirement: 现有测试守恒

重构过程中 SHALL 保留所有现有测试,不删除任何已通过的测试用例。

#### Scenario: nexus-core 测试守恒

- **WHEN** 重构 `nexus-core/src/error.rs` 与 `storage_traits.rs`
- **THEN** 现有 4 个 error.rs 测试 MUST 全部保留且通过
- **AND** `sqlite_pragma.rs` 的 3 个测试 MUST 迁移到下游 crate(`cmt-tiering` 或 `mlc-engine`)的 `tests/` 目录(可重命名,但测试逻辑必须保留),因 L1 不能依赖 rusqlite
- **AND** `nexus-core` 新增的泛型函数测试(若需)MUST 使用 mock impl,不引入 rusqlite dev-dependency

#### Scenario: 下游 crate 测试守恒

- **WHEN** 重构 `cmt-tiering` 与 `mlc-engine` 的 3 处调用点
- **THEN** `cmt-tiering` 现有测试 MUST 全部保留且通过
- **AND** `mlc-engine` 现有测试 MUST 全部保留且通过
- **AND** 若重构改变公开 API,新增测试 MUST 覆盖新 API

### Requirement: 全量测试与 lint 通过

重构后 SHALL 通过全量测试与 lint 检查,确保不引入回归。

#### Scenario: cargo test 全绿

- **WHEN** 执行 `cargo test --workspace`
- **THEN** 退出码 MUST 为 0
- **AND** 测试数量 MUST ≥ v1.0.0-omega GA 基线(3002+ 测试,因迁移可能微调)
- **AND** 执行 `cargo test -- --ignored --nocapture` 压力测试 MUST 同样通过

#### Scenario: cargo clippy 零警告

- **WHEN** 执行 `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings`(§9 clippy OOM 缓解)
- **THEN** 退出码 MUST 为 0
- **AND** 输出 MUST 不包含任何 warning

#### Scenario: cargo fmt 零 diff

- **WHEN** 执行 `cargo fmt --all -- --check`
- **THEN** 退出码 MUST 为 0
- **AND** 输出 MUST 不包含任何 diff

### Requirement: 架构合规性

重构 SHALL 严格遵守 §2.2 依赖铁律与 §4.1 Rust 编码规范。

#### Scenario: 依赖方向合规

- **WHEN** 检查重构后的依赖图
- **THEN** `nexus-core`(L1)MUST 不依赖任何 L2+ crate,且 MUST 不依赖 `rusqlite`
- **AND** `PragmaCapable` trait 与 `apply_performance_pragmas<T>` 泛型函数 MUST 定义在 L1 `nexus-core/src/storage_traits.rs`,不引用 rusqlite 类型
- **AND** L2 `mlc-engine` 与 L3 `cmt-tiering` MUST 向下依赖 L1 `nexus-core`(合规),实现 `PragmaCapable for rusqlite::Connection` 并调用泛型函数
- **AND** L2/L3 crate 之间 MUST 无相互依赖(各自独立 impl trait,代码可重复但不耦合)
- **AND** 方案 E 不存在 L2→L3 依赖,无需 trait object 或 dev-dependencies 绕过

#### Scenario: forbid(unsafe_code) 保持

- **WHEN** 检查重构后的所有 crate
- **THEN** `nexus-core` MUST 保持 `#![forbid(unsafe_code)]`
- **AND** `cmt-tiering` / `mlc-engine` MUST 保持 `#![forbid(unsafe_code)]`(crate 级)
- **AND** rusqlite bundled 内部 unsafe 不影响当前 crate(§4.1 已确立原则)

#### Scenario: rusqlite spawn_blocking 合规

- **WHEN** 检查 L2/L3 下游 crate 的 `PragmaCapable for rusqlite::Connection` 实现与调用点
- **THEN** 所有 rusqlite 调用 MUST 在 `spawn_blocking` 上下文中执行(§4.4 第 2 条)
- **AND** MUST 不在 async 上下文直接调用 `Connection::pragma_update` 等阻塞 API

## MODIFIED Requirements

### Requirement: CODE_WIKI.md L1 层职责描述

`CODE_WIKI.md` §2.1 L1 Core 层职责 SHALL 移除"提供 sqlite_pragma 工具函数"相关表述,补充"L1 定义 PragmaCapable trait,L2/L3 实现并调用(ADR-006 方案 E)"。

#### Scenario: §2.1 L1 描述更新

- **WHEN** 阅读 `CODE_WIKI.md` §2.1 L1 Core 章节
- **THEN** MUST 不再出现"sqlite_pragma"或"提供 PRAGMA 工具"等表述
- **AND** MUST 补充说明"rusqlite 依赖已通过 L1 PragmaCapable trait 抽象隔离,L2/L3 实现 trait 并调用泛型函数(ADR-006 方案 E)"
- **AND** MUST 在 ADR 表格中新增 ADR-006 条目(rusqlite 下沉决策,选定方案 E)

## REMOVED Requirements

无。本 Spec 不删除任何已有需求,仅迁移 rusqlite 依赖位置。

---

## 约束条件(Constraint)

### C-1: §2.2 依赖铁律

- L(N) → L(N-1) 允许(向下依赖)
- L(N) → L(N+1) 禁止(向上依赖)
- `nexus-core`(L1)禁止 import 任何 L2+ crate,且禁止依赖 `rusqlite`
- 跨层通信只能走 Event Bus 或 L1 定义的 trait(方案 E 采用 L1 trait + 泛型,非 trait object,合规)

### C-2: §4.1 Rust 编码规范

- 所有 crate 必须 `#![forbid(unsafe_code)]`(crate 级)
- workspace 共享依赖,不独立声明版本
- 库层 thiserror enum,应用层 anyhow
- 避免 `unwrap()` / `expect()`,边界用 `?` 或 `match`
- 单函数 ≤ 200 行(§6.1 红线)
- 优先静态分发(泛型 `<T: Trait>`),避免 `Box<dyn Trait>`(§4.1)

### C-3: §4.4 async 反模式清单

- rusqlite 调用必须 `spawn_blocking`(第 2 条)
- 禁止持锁跨 `.await`(第 1 条)
- broadcast 先 subscribe 再 spawn(第 3 条,若涉及 EventBus)

### C-4: §3.1 RC 阶段守恒(已 GA 但 Spec-driven 不变)

- 现有测试必须保留,不删除任何已通过测试
- 任何 bugfix 必须先写失败测试再修复(TDD 守恒)

### C-5: ADR-006 决策已采纳方案 E

- 方案 E(trait abstraction)选定理由:
  - (a) 不引入新 crate,避免增加 workspace 复杂度(方案 D 排除)
  - (b) 不违反 L2→L3 依赖方向(方案 A 排除,L2 不能依赖 L3)
  - (c) 不重复 PRAGMA 代码(方案 B 排除,5 个 PRAGMA 在 3 处下游复制会引发不一致风险)
  - (d) trait 定义在 L1,泛型函数零运行时开销(方案 C trait object 有运行时开销)
  - (e) 沿用现有 `NexusError` 错误类型,不引入 `StorageError`,减少错误链改造范围
- ADR-006 文档必须在 F2.4 验收前定稿,但方案 E 已确定,可在实施并行撰写

### C-6: 不修改核心领域类型

- `UserIntent` / `Quest` / `Checkpoint` / `OmniSparseMasks` / `CLV` / `NexusState` 保持不变
- 仅迁移 rusqlite 依赖位置,不重构领域模型

---

## 验收标准(Acceptance Criteria)

### AC-1: nexus-core 零 rusqlite 依赖(可量化)

```powershell
# 期望输出为空
cargo tree -p nexus-core | Select-String -Pattern 'rusqlite'
cargo tree -p nexus-core --all-features | Select-String -Pattern 'rusqlite'
cargo tree -p nexus-core --all-targets | Select-String -Pattern 'rusqlite'
```

### AC-2: 文件删除验证

```powershell
# 期望文件不存在
Test-Path "d:\Chimera CLI\crates\nexus-core\src\sqlite_pragma.rs"  # False
# 期望 Cargo.toml 不含 rusqlite
Select-String -Path "d:\Chimera CLI\crates\nexus-core\Cargo.toml" -Pattern 'rusqlite'  # 无匹配
# 期望 error.rs 不含 SqliteError
Select-String -Path "d:\Chimera CLI\crates\nexus-core\src\error.rs" -Pattern 'SqliteError|rusqlite'  # 无匹配
```

### AC-3: 全量测试通过

```powershell
cargo test --workspace  # 退出码 0
cargo test -- --ignored --nocapture  # 压力测试通过
```

### AC-4: lint 零警告

```powershell
$env:RUST_MIN_STACK = '33554432'; $env:CARGO_INCREMENTAL = '0'
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings  # 退出码 0
cargo fmt --all -- --check  # 退出码 0
```

### AC-5: 下游调用点重构验证

```powershell
# 期望 3 处调用点不再调用 nexus_core::sqlite_pragma
Select-String -Path "d:\Chimera CLI\crates\cmt-tiering\src\warm.rs" -Pattern 'nexus_core::sqlite_pragma'  # 无匹配
Select-String -Path "d:\Chimera CLI\crates\cmt-tiering\src\cold.rs" -Pattern 'nexus_core::sqlite_pragma'  # 无匹配
Select-String -Path "d:\Chimera CLI\crates\mlc-engine\src\l3_procedural.rs" -Pattern 'nexus_core::sqlite_pragma'  # 无匹配
# 期望 3 处调用点改为泛型调用
Select-String -Path "d:\Chimera CLI\crates\cmt-tiering\src\warm.rs" -Pattern 'nexus_core::storage_traits::apply_performance_pragmas'  # 有匹配
Select-String -Path "d:\Chimera CLI\crates\cmt-tiering\src\cold.rs" -Pattern 'nexus_core::storage_traits::apply_performance_pragmas'  # 有匹配
Select-String -Path "d:\Chimera CLI\crates\mlc-engine\src\l3_procedural.rs" -Pattern 'nexus_core::storage_traits::apply_performance_pragmas'  # 有匹配
# 期望下游 crate 实现 PragmaCapable trait
Select-String -Path "d:\Chimera CLI\crates\cmt-tiering\src\*.rs" -Pattern 'impl PragmaCapable for rusqlite::Connection'  # 有匹配
Select-String -Path "d:\Chimera CLI\crates\mlc-engine\src\*.rs" -Pattern 'impl PragmaCapable for rusqlite::Connection'  # 有匹配
```

### AC-6: L1 PragmaCapable trait 实现验证

- `PragmaCapable` trait 在 `nexus-core` 公开导出(`crates/nexus-core/src/lib.rs` 含 `pub use storage_traits::PragmaCapable;`)
- `apply_performance_pragmas<T: PragmaCapable>` 泛型函数在 `nexus-core` 公开导出
- `crates/nexus-core/src/storage_traits.rs` 不引用任何 rusqlite 类型(`Select-String -Path "d:\Chimera CLI\crates\nexus-core\src\storage_traits.rs" -Pattern 'rusqlite'` 无匹配)
- 原 5 个 PRAGMA(synchronous=NORMAL / cache_size=-65536 / mmap_size=268435456 / temp_store=MEMORY / wal_autocheckpoint=1000)在泛型函数中通过 trait 方法设置
- 下游 `cmt-tiering` / `mlc-engine` 实现 `impl PragmaCapable for rusqlite::Connection`
- 新增单元测试覆盖率 ≥ 90%(仅限新增 `storage_traits.rs` 文件,使用 mock impl 测试泛型逻辑)

### AC-7: 文档同步验证

- `docs/adr/ADR-006-rusqlite-descoping.md` 创建完成,含方案 E 选定理由、A-D 方案权衡分析、CCB 评审记录
- `CODE_WIKI.md` §2.1 L1 层职责描述更新,移除 sqlite_pragma 表述,补充"L1 定义 PragmaCapable trait,L2/L3 实现并调用"
- `CODE_WIKI.md` ADR 表格新增 ADR-006 条目(选定方案 E)
- `CHANGELOG.md` 追加 v1.1.0 F2 章节

---

## 风险与应对

| 风险 ID | 风险描述 | 可能性 | 影响 | 应对措施 |
|---------|---------|--------|------|---------|
| R-F2-1 | ADR-006 文档撰写延迟阻塞 F2.4 验收 | 低 | 中 | 方案 E 已确定,可在实施并行撰写 ADR-006,不阻塞 F2.2/F2.3 启动 |
| R-F2-2 | 下游 3 处调用点重构后 PRAGMA 不生效,导致性能回退 | 低 | 中 | 重构后运行 benches 对比(C1),验证 5 个 PRAGMA 仍然生效;方案 E 使用静态分发,理论零开销 |
| R-F2-3 | ~~L2 mlc-engine 依赖 L3 StorageBackend 违反依赖方向~~ 方案 E 通过 L1 trait abstraction 避免 L2→L3 依赖违规,trait 定义在 L1 `nexus-core`,`PragmaCapable` trait + `apply_performance_pragmas<T>` 泛型函数均不引用 rusqlite 类型,L2/L3 向下依赖 L1 合规(§2.2 铁律),无需 trait object 或 dev-deps 绕过 | 低 | 低 | 严格遵循 §2.2,L1 trait 文件不 `use rusqlite::...`,CI 加 grep 检查防止回归 |
| R-F2-4 | 移除 `SqliteError(#[from] rusqlite::Error)` 变体后,下游 `.map_err()` 转换链断裂 | 中 | 中 | 方案 E 沿用现有 `NexusError` 错误类型,下游 impl trait 时通过 `.map_err()` 显式构造错误变体;重构前 grep 所有 `NexusError::SqliteError` 使用点,逐一迁移(预期:无直接 match,因下游均通过 `.map_err()` 转换为各自库层错误类型) |
| R-F2-5 | orphan rule 阻止下游 crate 实现 `impl PragmaCapable for rusqlite::Connection` | 低 | 中 | trait 在 L1 `nexus-core` 定义(本地 crate),`rusqlite::Connection` 是外部类型;orphan rule 允许下游 crate impl(trait 或类型至少一个在本 crate)。预判无障碍,若受阻可改为 newtype wrapper |

---

## 依赖关系

- **前置**:无(F2 是 v1.1.0 Phase 1 起始任务)
- **阻塞**:F1 L10 编排接线(F1 依赖 F2.3 完成,见 v1.1.0 路线图 §3.1)
- **并行**:F6 MCP Mesh 2PC 设计阶段可并行(无强依赖)
- **后续**:F1 L10 编排、Phase 8 集成测试

---

## 参考文档

- `docs/release/v1.1.0_roadmap.md` §2.1 Phase 1 F2 WBS
- `.trae/rules/nuxus规则.md` §2.2 依赖铁律、§4.1 Rust 编码规范、§4.4 async 反模式、§6.2 Week 1-8 红线
- `CODE_WIKI.md` §2.1 十层架构、§2.3 ADR 表格
- `CHANGELOG.md` v1.0.0-omega GA 章节

---

**NEXUS-OMEGA — Ω-Sparse · Ω-Compress · Ω-Evolve · Ω-Event**
