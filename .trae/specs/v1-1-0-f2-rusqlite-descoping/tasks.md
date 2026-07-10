# Tasks — v1.1.0-omega F2 rusqlite 下沉

> 任务按 MoSCoW 优先级分组:Must 阻塞 v1.1.0 发布、Should 强烈建议、Could 周期内补齐。
> v1.1.0 已突破 RC 阶段,允许跨层重构,但方案 E 不引入新 crate。
> 现状审计:F2.1.1 已完成(nexus-core 仅 4 处 rusqlite 使用,下游仅 3 处调用,quest-engine 无依赖)。
> **方案 E(L1 trait abstraction)已采纳**:trait 定义在 L1 `nexus-core`,`PragmaCapable` trait + `apply_performance_pragmas<T>` 泛型函数,L2/L3 实现并调用,严格遵循 §2.2 依赖铁律。

## 阶段一:F2.1 现状分析与设计(6h,路线图预估 16h,因 F2.1.1 已完成 + F2.1.4 删除)

> **依赖**:无前置
> **并行性**:F2.1.2 / F2.1.3 可并行分析,F2.1.5 ADR-006 必须最后完成(但方案 E 已确定,不阻塞 F2.2 启动)
> **预算调整**:路线图 16h,实际 6h(F2.1.1 已完成、quest-engine 无依赖、F2.1.4 删除)

- [x] **Task F2.1.1: 审计 nexus-core 中所有 rusqlite 调用点** ✅ 已完成(2026-07-04)
  - [x] SubTask F2.1.1.1: `grep rusqlite in crates/nexus-core/src/` 已执行,4 处使用已确认
  - [x] SubTask F2.1.1.2: 审计结果:nexus-core 仅 `Cargo.toml:18-19` / `lib.rs:39` / `error.rs:42` / `sqlite_pragma.rs:31` 四处
  - [x] SubTask F2.1.1.3: 下游调用点 3 处已确认(`cmt-tiering/warm.rs:464` / `cmt-tiering/cold.rs:539` / `mlc-engine/l3_procedural.rs:415`)
  - [x] SubTask F2.1.1.4: quest-engine 无依赖(路线图 F2.3.4 提及但审计证伪)

- [x] **Task F2.1.2: 分析下游 crate 对 sqlite_pragma 的依赖**(2h,路线图 4h,因 quest-engine 无依赖)✅ 已完成(2026-07-08)
  - [x] SubTask F2.1.2.1: 读取 `crates/cmt-tiering/src/warm.rs:459-466` 确认调用模式(已读:委托 + map_err 转 CmtError::StorageError)
  - [x] SubTask F2.1.2.2: 读取 `crates/cmt-tiering/src/cold.rs:534-541` 确认调用模式(已读:同 warm.rs)
  - [x] SubTask F2.1.2.3: 读取 `crates/mlc-engine/src/l3_procedural.rs:410-417` 确认调用模式(已读:委托 + map_err 转 MlcError::StorageError)
  - [x] SubTask F2.1.2.4: 撰写《下游依赖分析报告》,确认 3 处调用点均通过 `.map_err()` 转换为各自库层错误类型,无公开 API 暴露 `NexusError::SqliteError`
  - [x] SubTask F2.1.2.5: grep `NexusError::SqliteError` 全 workspace 使用点,确认下游未直接 match 该变体(若发现需在 F2.3.4 逐一迁移)

- [x] **Task F2.1.3: 设计 PragmaCapable trait + apply_performance_pragmas 泛型函数**(2h,原 4h,方案 E 设计简化)✅ 已完成(2026-07-08)
  - [x] SubTask F2.1.3.1: 设计 `pub trait PragmaCapable` 方法签名(`pragma_update_string(&self, key: &str, value: &str) -> Result<(), NexusError>` / `pragma_update_int(&self, key: &str, value: i64) -> Result<(), NexusError>`)
  - [x] SubTask F2.1.3.2: 设计 `pub fn apply_performance_pragmas<T: PragmaCapable>(conn: &T) -> Result<(), NexusError>` 泛型函数签名(静态分发,零 trait object 开销)
  - [x] SubTask F2.1.3.3: 验证 trait 与泛型函数签名不引用任何 rusqlite 类型(仅依赖 `&str` / `i64` / `NexusError`),L1 编译时无 rusqlite 依赖
  - [x] SubTask F2.1.3.4: 评估 orphan rule 合规性:trait 在 L1 nexus-core(本地 crate)定义,`rusqlite::Connection` 是外部类型,下游 crate impl 合规(trait 在本 crate 即可)
  - [x] SubTask F2.1.3.5: 撰写《PragmaCapable trait 设计稿》,含方法签名、泛型函数、orphan rule 分析、错误类型沿用 NexusError 的理由

- [ ] **Task F2.1.4: ~~设计 EventBus 事件类型~~** — 已删除(方案 E 是同步 trait 调用,不需要 EventBus 事件)
  - 方案 E 通过泛型函数 `apply_performance_pragmas<T>(conn: &T)` 直接调用,无跨层状态广播需求
  - 若后续需要存储状态变更通知,可独立引入 EventBus 事件,不在本 Spec 范围

- [x] **Task F2.1.5: 撰写 ADR-006 rusqlite 下沉决策文档**(2h)— 方案 E 已确定,可与 F2.2 并行 ✅ 已完成(2026-07-08)
  - [x] SubTask F2.1.5.1: 创建 `docs/adr/ADR-006-rusqlite-descoping.md`
  - [x] SubTask F2.1.5.2: 记录方案 E(trait abstraction)选定理由:不引入新 crate / 不违反依赖方向 / 不重复代码 / 零运行时开销 / 沿用 NexusError
  - [x] SubTask F2.1.5.3: 对比方案 A(L3 StorageBackend trait object,违反 L2→L3 依赖方向)、方案 B(本地复制 PRAGMA,代码重复风险)、方案 C(L1 trait + trait object,运行时开销)、方案 D(新建 storage-backbone crate,增加 workspace 复杂度)的权衡
  - [x] SubTask F2.1.5.4: 记录 CCB 评审结果与选定理由
  - [x] SubTask F2.1.5.5: 在 `CODE_WIKI.md` §2.3 ADR 表格新增 ADR-006 条目(选定方案 E)
  - [x] SubTask F2.1.5.6: 记录回滚方案(若 orphan rule 受阻,改为 newtype wrapper `pub struct ChimeraConnection(rusqlite::Connection)` 在下游 crate 实现 trait)

## 阶段二:F2.2 nexus-core trait 抽象实现(10h,原 24h,方案 E 简化)

> **依赖**:F2.1.3 设计稿完成(但方案 E 已确定,可与 F2.1.5 并行)
> **并行性**:F2.2.1 / F2.2.2 串行(trait 先于泛型函数),F2.2.3 依赖 F2.2.1,F2.2.4 / F2.2.5 可并行
> **预算调整**:路线图 24h,实际 10h(方案 E 无需创建新 crate、无需 SqliteStorageBackend 结构体、无需 trait object 注入设计)

- [x] **Task F2.2.1: 在 nexus-core 创建 storage_traits.rs 定义 PragmaCapable trait**(3h)✅ 已完成(2026-07-08)
  - [x] SubTask F2.2.1.1: 在 `crates/nexus-core/src/` 新建 `storage_traits.rs` 模块
  - [x] SubTask F2.2.1.2: 定义 `pub trait PragmaCapable` 含 `pragma_update_string` / `pragma_update_int` 两个方法,返回 `Result<(), NexusError>`
  - [x] SubTask F2.2.1.3: 在文件顶部添加 `#![forbid(unsafe_code)]` 约束(继承 crate 级属性)
  - [x] SubTask F2.2.1.4: 验证文件不引用任何 `rusqlite` 类型(grep 检查)
  - [x] SubTask F2.2.1.5: 在 `crates/nexus-core/src/lib.rs` 添加 `pub mod storage_traits;` + `pub use storage_traits::PragmaCapable;`
  - [x] SubTask F2.2.1.6: `cargo check -p nexus-core` 验证 trait 定义编译通过(此时 nexus-core 仍依赖 rusqlite,但 storage_traits.rs 不引用)

- [x] **Task F2.2.2: 在 nexus-core 实现 apply_performance_pragmas 泛型函数**(3h)✅ 已完成(2026-07-08)
  - [x] SubTask F2.2.2.1: 在 `crates/nexus-core/src/storage_traits.rs` 实现 `pub fn apply_performance_pragmas<T: PragmaCapable>(conn: &T) -> Result<(), NexusError>`
  - [x] SubTask F2.2.2.2: 通过 trait 方法设置原 5 个 PRAGMA:
    - `conn.pragma_update_string("synchronous", "NORMAL")?`
    - `conn.pragma_update_int("cache_size", -65536)?`
    - `conn.pragma_update_int("mmap_size", 268435456)?`
    - `conn.pragma_update_string("temp_store", "MEMORY")?`
    - `conn.pragma_update_int("wal_autocheckpoint", 1000)?`
  - [x] SubTask F2.2.2.3: 添加文档注释说明:必须在 `journal_mode=WAL` 设置之后调用(沿用原 `sqlite_pragma.rs` 注释约束)
  - [x] SubTask F2.2.2.4: 在 `lib.rs` 导出 `pub use storage_traits::apply_performance_pragmas;`
  - [x] SubTask F2.2.2.5: 添加 mock impl 单元测试(`struct MockPragmaCapable` 实现 trait,验证泛型函数调用正确性,不依赖 rusqlite)
  - [x] SubTask F2.2.2.6: `cargo test -p nexus-core` 验证 mock impl 测试通过(45 unit + 11 proptest + 27 integration + 7 doc-test 全绿)

- [x] **Task F2.2.3: 在 cmt-tiering / mlc-engine 以 newtype wrapper 实现 PragmaCapable**(4h)✅ 已完成(2026-07-08)
  - **WHY newtype wrapper**:spec 原计划"两 crate 分别 impl `PragmaCapable for rusqlite::Connection`"会触发 Rust coherence 冲突(`conflicting implementations`),因根 Cargo.toml 的 E2E 测试同时依赖两 crate。用户决策(2026-07-08)采纳 spec F2.1.5.6 预置回滚方案:newtype wrapper。
  - [x] SubTask F2.2.3.1: 在 `crates/cmt-tiering/src/` 新增 `storage_impl.rs`,定义 `pub struct PragmaConn<'a>(pub &'a rusqlite::Connection)` 并实现 `impl<'a> nexus_core::PragmaCapable for PragmaConn<'a>`
    - `pragma_update_string`: 调用 `self.0.pragma_update(None, key, value).map_err(|e| NexusError::SerializationError(format!("SQLite PRAGMA {key}={value} 失败: {e}")))`
    - `pragma_update_int`: 同上,泛型参数 `value: i64` 传入 `pragma_update`
    - 错误变体选用 `SerializationError`(沿用 mock test 既有先例,避免 F2.3.3 删除 `SqliteError` 变体时破坏 impl)
  - [x] SubTask F2.2.3.2: 在 `crates/cmt-tiering/src/lib.rs` 添加 `pub mod storage_impl;` + `pub use storage_impl::PragmaConn;`
  - [x] SubTask F2.2.3.3: 在 `crates/mlc-engine/src/` 新增 `storage_impl.rs`,同样定义 `pub struct PragmaConn<'a>(pub &'a rusqlite::Connection)` 并 impl(各 crate 独立 newtype,避免 coherence 冲突)
  - [x] SubTask F2.2.3.4: 在 `crates/mlc-engine/src/lib.rs` 添加 `pub mod storage_impl;` + `pub use storage_impl::PragmaConn;`
  - [x] SubTask F2.2.3.5: 验证 `crates/cmt-tiering/Cargo.toml` 已声明 `rusqlite = { workspace = true }`(审计确认:第 12 行已存在,无需修改)
  - [x] SubTask F2.2.3.6: 验证 `crates/mlc-engine/Cargo.toml` 已声明 `rusqlite = { workspace = true }`(审计确认:第 12 行已存在,无需修改)
  - [x] SubTask F2.2.3.7: `cargo check -p cmt-tiering && cargo check -p mlc-engine` 验证 impl 编译通过

- [ ] **Task F2.2.4: 添加 PragmaCapable trait 单元测试**(已并入 F2.2.2.5,无独立任务)
  - mock impl 测试在 `crates/nexus-core/src/storage_traits.rs` 内,nexus-core 不依赖 rusqlite
  - 真实 rusqlite::Connection 的 impl 测试在 F2.4.3 proptest 中处理(需迁移原 3 个测试到下游 crate)

- [ ] **Task F2.2.5: 添加 PragmaCapable 集成测试**(已并入 F2.4.3,无独立任务)
  - 跨 crate 调用测试在 `crates/cmt-tiering/tests/` 与 `crates/mlc-engine/tests/` 中,验证 `apply_performance_pragmas(conn)` 真实生效

## 阶段三:F2.3 nexus-core 重构与下游调用迁移(14h,原 16h,方案 E 下游迁移简单)

> **依赖**:F2.2.2 泛型函数实现完成 + F2.2.3 下游 impl 完成
> **并行性**:F2.3.1 / F2.3.2 / F2.3.3 可并行(独立文件修改),F2.3.4 依赖前三者
> **预算调整**:路线图 24h,实际 14h(方案 E 下游调用改为泛型调用,无需 trait object 注入)

- [x] **Task F2.3.1: 移除 nexus-core/Cargo.toml 的 rusqlite 依赖**(1h)✅ 已完成(2026-07-08)
  - [x] SubTask F2.3.1.1: 删除 `crates/nexus-core/Cargo.toml` 第 18-19 行 `# rusqlite:SubTask 21.2 — sqlite_pragma 模块需要 Connection 类型` 与 `rusqlite = { workspace = true }`
  - [x] SubTask F2.3.1.2: `cargo check -p nexus-core` 验证编译(预期失败,因 sqlite_pragma.rs 仍引用 rusqlite,需 F2.3.2 完成后才能通过)
  - [x] SubTask F2.3.1.3: 暂不提交,等 F2.3.2 / F2.3.3 / F2.3.4 完成后统一提交

- [x] **Task F2.3.2: 删除 nexus-core/src/sqlite_pragma.rs**(1h)✅ 已完成(2026-07-08)
  - [x] SubTask F2.3.2.1: 删除整个 `crates/nexus-core/src/sqlite_pragma.rs` 文件(56 行,逻辑已迁移到 `storage_traits.rs` 泛型函数)
  - [x] SubTask F2.3.2.2: 删除 `crates/nexus-core/src/lib.rs` 第 39 行 `pub mod sqlite_pragma;`
  - [x] SubTask F2.3.2.3: 确认 3 个测试(`test_apply_performance_pragmas_success` / `test_cache_size_set` / `test_temp_store_memory`)将在 F2.4.3 迁移到下游 crate `tests/` 目录,不丢失

- [x] **Task F2.3.3: 重构 nexus-core 中 4 处 rusqlite 调用为 PragmaCapable trait**(4h)✅ 已完成(2026-07-08)
  - [x] SubTask F2.3.3.1: 核查 nexus-core 中 4 处涉及 rusqlite 的位置(`Cargo.toml` 依赖声明 / `lib.rs` 模块声明 / `error.rs` 错误变体 / `sqlite_pragma.rs` 实现),确认全部通过 PragmaCapable trait 抽象隔离
  - [x] SubTask F2.3.3.2: 删除 `crates/nexus-core/src/error.rs` 第 36-42 行 `SqliteError(#[from] rusqlite::Error)` 变体及其文档注释(因 `#[from]` 自动转换不再需要,L1 不依赖 rusqlite)
  - [x] SubTask F2.3.3.3: grep 全 workspace `NexusError::SqliteError` 使用点,确认下游均通过 `.map_err()` 转换为各自库层错误类型(`CmtError::StorageError` / `MlcError::StorageError`),无直接 match(预期:无,因下游已通过 `.map_err()` 包装)
  - [x] SubTask F2.3.3.4: 保留现有 4 个 error.rs 测试(`test_invalid_clv_dimension_display` / `test_quest_not_found_display` / `test_quest_already_exists_display` / `test_io_error_from`)
  - [x] SubTask F2.3.3.5: `cargo test -p nexus-core` 验证 4 个测试全部通过
  - [x] SubTask F2.3.3.6: 确认 `storage_traits.rs` 中 `apply_performance_pragmas<T>` 泛型函数内部通过 `conn.pragma_update_string(...)` / `conn.pragma_update_int(...)` trait 方法调用,无直接 rusqlite API 引用

- [x] **Task F2.3.4: 更新 mlc-engine / cmt-tiering 的调用为泛型 apply_performance_pragmas**(6h)— **已并入 F2.2.3 并行执行** ✅ 已完成(2026-07-08)
  - **WHY 并入 F2.2.3**:newtype wrapper 创建后立即可用,与调用点迁移合并可避免 dead code 警告(wrapper impl 未使用),且减少子代理切换开销
  - [x] SubTask F2.3.4.1: 重构 `crates/cmt-tiering/src/warm.rs:459-466` `apply_performance_pragmas` 函数
    - 改为 `nexus_core::apply_performance_pragmas(&PragmaConn(conn))` 泛型调用(函数内构造 wrapper)
    - 错误处理:`.map_err(|e| CmtError::StorageError(...))`(沿用现有错误转换链)
  - [x] SubTask F2.3.4.2: 重构 `crates/cmt-tiering/src/cold.rs:534-541` 同 warm.rs
  - [x] SubTask F2.3.4.3: 重构 `crates/mlc-engine/src/l3_procedural.rs:410-417` 同 cmt-tiering(注意错误类型转换为 MlcError)
  - [x] SubTask F2.3.4.4: 验证 `cmt-tiering` / `mlc-engine` 的 `Cargo.toml` 已声明 `rusqlite`(审计确认:两 crate 第 12 行均已存在,无需修改)
  - [x] SubTask F2.3.4.5: `cargo check -p cmt-tiering && cargo check -p mlc-engine` 验证编译通过
  - [x] SubTask F2.3.4.6: `cargo test -p cmt-tiering && cargo test -p mlc-engine` 验证现有测试全部通过

- [x] **Task F2.3.5: 验证 nexus-core 零 rusqlite 依赖**(1h)✅ 已完成(2026-07-08)
  - [x] SubTask F2.3.5.1: `cargo tree -p nexus-core | Select-String rusqlite` 输出为空
  - [x] SubTask F2.3.5.2: `cargo tree -p nexus-core --all-features | Select-String rusqlite` 输出为空
  - [x] SubTask F2.3.5.3: `cargo tree -p nexus-core --all-targets | Select-String rusqlite` 输出为空
  - [x] SubTask F2.3.5.4: `cargo build -p nexus-core` 验证无 rusqlite 链接
  - [x] SubTask F2.3.5.5: 在核验报告中记录三条命令的输出截图

- [x] **Task F2.3.6: 更新 lib.rs 模块声明**(1h)✅ 已完成(2026-07-08)
  - [x] SubTask F2.3.6.1: 确认 `crates/nexus-core/src/lib.rs` 不再含 `pub mod sqlite_pragma;`(已在 F2.3.2.2 删除)
  - [x] SubTask F2.3.6.2: 确认 `crates/nexus-core/src/lib.rs` 含 `pub mod storage_traits;` 与 `pub use storage_traits::{PragmaCapable, apply_performance_pragmas};`(已在 F2.2.1.5 / F2.2.2.4 添加)
  - [x] SubTask F2.3.6.3: 更新 `lib.rs` 顶部文档注释,移除"提供 sqlite_pragma 工具"相关描述,补充"L1 定义 PragmaCapable trait,L2/L3 实现并调用"
  - [x] SubTask F2.3.6.4: 更新 `prelude` 模块,导出 `PragmaCapable` 与 `apply_performance_pragmas`(若 prelude 含 sqlite_pragma 相关导出需移除)

## 阶段四:F2.4 测试与验收(14h,原 16h,proptest 调整为 mock impl)

> **依赖**:F2.3 全部完成
> **并行性**:F2.4.1 / F2.4.2 / F2.4.3 / F2.4.4 可并行,F2.4.5 文档同步独立

- [x] **Task F2.4.1: cargo test --workspace 全绿**(2h)✅ 已完成(2026-07-08)
  - [x] SubTask F2.4.1.1: 执行 `cargo test --workspace`,断言退出码 0
  - [x] SubTask F2.4.1.2: 测试数量 ≥ v1.0.0-omega GA 基线(3002+ 测试,因迁移 3 个测试到下游 crate,数量应保持或增加)
  - [x] SubTask F2.4.1.3: 执行 `cargo test -- --ignored --nocapture` 压力测试,断言通过
  - [x] SubTask F2.4.1.4: 在核验报告中记录测试输出末尾的 `test result: ok. N passed; 0 failed;`

- [x] **Task F2.4.2: cargo clippy 零警告**(4h)✅ 已完成(2026-07-08)
  - [x] SubTask F2.4.2.1: 设置环境变量 `$env:RUST_MIN_STACK = '33554432'; $env:CARGO_INCREMENTAL = '0'`(§9 clippy OOM 缓解)
  - [x] SubTask F2.4.2.2: 执行 `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings`,断言退出码 0
  - [x] SubTask F2.4.2.3: 执行 `cargo fmt --all -- --check`,断言退出码 0
  - [x] SubTask F2.4.2.4: 修复任何 clippy 警告(优先使用 `cargo clippy --fix --allow-dirty --allow-no-vcs`)

- [x] **Task F2.4.3: 添加 proptest 验证 PragmaCapable trait 不变量**(4h)✅ 已完成(2026-07-08)
  - [x] SubTask F2.4.3.1: 在 `crates/nexus-core/tests/proptest.rs` 创建 mock impl proptest,验证泛型函数对 mock 的调用正确性(L1 测试不依赖 rusqlite)
    - proptest PRAGMA 幂等性:多次调用 `apply_performance_pragmas(&mock)`,mock 内部记录的 PRAGMA 值不变
    - proptest 泛型调用正确性:对 mock impl 与不同输入,函数调用 trait 方法次数与顺序正确
    - 使用 proptest 1.11+ block-named 语法(§4.1:`fn test_name(x in 0..100u32) { ... }`)
  - [x] SubTask F2.4.3.2: 在 `crates/cmt-tiering/tests/pragma_capable_proptest.rs` 创建真实 rusqlite impl proptest,验证 trait impl 行为
    - proptest PRAGMA 幂等性:多次调用 `apply_performance_pragmas(&conn)`,真实 PRAGMA 值不变
    - proptest WAL 模式持久性:设置 WAL + apply_performance_pragmas → 关闭连接 → 重开 → PRAGMA 仍生效
    - 迁移原 3 个测试(`test_apply_performance_pragmas_success` / `test_cache_size_set` / `test_temp_store_memory`)到本文件,逻辑保留
  - [x] SubTask F2.4.3.3: 在 `crates/mlc-engine/tests/pragma_capable_proptest.rs` 同样创建真实 rusqlite impl proptest(可选,若 cmt-tiering 已覆盖可省略)
  - [x] SubTask F2.4.3.4: proptest trait 方法签名稳定性:验证 trait 不引用 rusqlite 类型(grep `rusqlite` in `storage_traits.rs` 为空)
  - [x] SubTask F2.4.3.5: `cargo test -p nexus-core --test proptest` 与 `cargo test -p cmt-tiering --test pragma_capable_proptest` 全部通过

- [x] **Task F2.4.4: 添加 benches 对比泛型函数性能**(2h)✅ 已完成(2026-07-08)
  - [x] SubTask F2.4.4.1: 创建 `crates/cmt-tiering/benches/pragma_capable_bench.rs`(因 nexus-core 不依赖 rusqlite,bench 放下游 crate)
  - [x] SubTask F2.4.4.2: benchmark `apply_performance_pragmas(&conn)` 单次调用延迟(泛型 + 真实 rusqlite::Connection)
  - [x] SubTask F2.4.4.3: benchmark PRAGMA 设置后查询延迟(对比未设置 PRAGMA 基线)
  - [x] SubTask F2.4.4.4: 断言重构后 p95 延迟 ≤ 重构前 +5%(C1 验收标准,方案 E 静态分发理论零开销)
  - [x] SubTask F2.4.4.5: 在 `crates/cmt-tiering/Cargo.toml` 添加 `[[bench]] name = "pragma_capable_bench" harness = false`

- [x] **Task F2.4.5: 更新 CODE_WIKI.md §2.1 L1 层职责描述**(2h,原 4h,因方案 E 描述简单)✅ 已完成(2026-07-08)
  - [x] SubTask F2.4.5.1: 读取 `CODE_WIKI.md` §2.1 L1 Core 章节当前内容
  - [x] SubTask F2.4.5.2: 移除"提供 sqlite_pragma 工具函数"相关表述
  - [x] SubTask F2.4.5.3: 补充"rusqlite 依赖已通过 L1 PragmaCapable trait 抽象隔离,L2/L3 实现 trait 并调用泛型函数(ADR-006 方案 E)"
  - [x] SubTask F2.4.5.4: 在 §2.3 ADR 表格新增 ADR-006 条目(rusqlite 下沉决策,选定方案 E)
  - [x] SubTask F2.4.5.5: 更新 §1.2 当前开发阶段为"v1.1.0-omega 开发中,F2 rusqlite 下沉已完成"
  - [x] SubTask F2.4.5.6: `CHANGELOG.md` 追加 v1.1.0 F2 章节(记录迁移范围、ADR-006 方案 E 决策、测试结果)

## 阶段五:文档同步与归档(并行进行)

- [x] **Task F2.5.1: project_memory 经验教训更新**(1h,与 F2.4 并行)✅ 已完成(2026-07-08,3 条核心教训已追加:coherence 冲突防范/newtype wrapper 修正/错误变体选择原则)
  - [x] SubTask F2.5.1.1: 在 `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md` 追加"rusqlite 下沉方案 E 经验教训"
  - [x] SubTask F2.5.1.2: 记录 ADR-006 方案 E 决策结果与 A-D 方案权衡
  - [x] SubTask F2.5.1.3: 记录 L1 trait abstraction 的最佳实践(trait 定义在 L1,L2/L3 impl,orphan rule 合规,静态分发零开销)
  - [x] SubTask F2.5.1.4: 记录测试迁移注意事项(3 个 PRAGMA 测试从 L1 迁移到下游 crate,因 L1 不能依赖 rusqlite)

- [x] **Task F2.5.2: cmt-tiering / mlc-engine spawn_blocking 一致性审计**(2h,可并行,C2)✅ 已完成(2026-07-08,审计通过无需修改)
  - [x] SubTask F2.5.2.1: grep 3 处下游调用点的 `spawn_blocking` 包装,确认所有 rusqlite 调用在 `spawn_blocking` 上下文(§4.4 第 2 条)
  - [x] SubTask F2.5.2.2: 修复任何在 async 上下文直接调用 rusqlite 的反模式
  - [x] SubTask F2.5.2.3: 验证 `cmt-tiering/src/warm.rs:455` 的 `spawn_blocking join 错误` 处理一致
  - [x] SubTask F2.5.2.4: 验证 `cmt-tiering/src/cold.rs:530` 同上
  - [x] SubTask F2.5.2.5: 验证 `mlc-engine/src/l3_procedural.rs:406` 同上

## Task Dependencies

- **F2.1.1** 已完成,无前置
- **F2.1.2 / F2.1.3** 可并行分析,均无前置(F2.1.1 已完成)
- **F2.1.4** 已删除(方案 E 不需要 EventBus 事件)
- **F2.1.5 ADR-006** 依赖 F2.1.2 / F2.1.3 完成(需要下游分析与 trait 设计作为决策输入),但方案 E 已确定,可与 F2.2 并行撰写
- **F2.2.1 / F2.2.2** 串行(trait 先于泛型函数),依赖 F2.1.3 设计稿完成
- **F2.2.3** 依赖 F2.2.1(需要 trait 定义才能 impl),F2.2.3 与 F2.2.2 可并行
- **F2.2.4 / F2.2.5** 已并入 F2.2.2 / F2.4.3,无独立任务
- **F2.3.1 / F2.3.2 / F2.3.3** 可并行(独立文件修改),依赖 F2.2.2(需要泛型函数就绪才能移除 L1 依赖)
- **F2.3.4** 依赖 F2.3.1 / F2.3.2 / F2.3.3 + F2.2.3(nexus-core 重构完成 + 下游 impl 就绪才能更新调用)
- **F2.3.5 / F2.3.6** 依赖 F2.3.4
- **F2.4.1 / F2.4.2 / F2.4.3 / F2.4.4** 可并行,依赖 F2.3 全部完成
- **F2.4.5** 文档同步独立,可在 F2.3 完成后立即启动
- **F2.5.1 / F2.5.2** 与 F2.4 并行

## 并行执行建议

- **串行关键路径**:F2.1.3(trait 设计)→ F2.2.1(trait 定义)→ F2.2.2(泛型函数)→ F2.3.4(下游重构)→ F2.4.1(test)→ F2.4.5(文档)
- **并行批次 1**(F2.1.5 完成前):F2.1.2 + F2.1.3 + F2.1.5(分析与设计并行,方案 E 已确定)
- **并行批次 2**(F2.2.1 完成后):F2.2.2 + F2.2.3(泛型函数与下游 impl 并行)
- **并行批次 3**(F2.3.4 完成后):F2.3.5 + F2.3.6 + F2.4.5(L1 验证 + 文档同步)
- **并行批次 4**(F2.3 全部完成后):F2.4.1 + F2.4.2 + F2.4.3 + F2.4.4 + F2.5.1 + F2.5.2(测试与审计并行)

## 预算汇总

| 阶段 | 路线图预估 | 实际预算 | 调整原因 |
|------|-----------|---------|---------|
| F2.1 现状分析与设计 | 16h | 6h | F2.1.1 已完成、F2.1.4 删除(方案 E 不需要 EventBus)、方案 E 设计简化 |
| F2.2 L1 trait 抽象实现 | 24h | 10h | 方案 E 无需创建新 crate、无需 SqliteStorageBackend 结构体、无需 trait object 注入设计 |
| F2.3 nexus-core 重构与下游迁移 | 24h | 14h | 方案 E 下游调用改为泛型调用,无需 trait object 注入 |
| F2.4 测试与验收 | 16h | 14h | proptest 调整为 mock impl + 真实 rusqlite 双轨,nexus-core bench 迁移到下游 |
| F2.5 文档与审计(并行) | — | 3h | 新增,从 F2.4 拆出 |
| **总计** | **80h** | **47h** | **节省 33h,方案 E 比 L3 StorageBackend 简单约 40%** |
