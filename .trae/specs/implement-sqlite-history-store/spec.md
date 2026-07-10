# SqliteHistoryStore 持久化实现 Spec

## Why

v1.3.0 引入的 `HistoryStore` trait + `InMemoryHistoryStore`(DashMap)为 MoE 五维门控评分提供运行时统计,但内存实现在进程重启后丢失历史,导致 v1.4.0 M2 RL 路由触发条件(历史数据 > 10000 条)在短周期内无法达成。需要为 `HistoryStore` 提供 SQLite 持久化实现,跨重启保留历史数据,解除 M2 阻塞。

## What Changes

- **新增 `history` 模块**(`crates/model-router/src/history/`):
  - `mod.rs`:`HistoryStore` trait + `HISTORY_SUFFICIENT_THRESHOLD` / `LATENCY_WINDOW_CAPACITY` 常量(权威源迁移自 `moe.rs`)
  - `memory.rs`:`InMemoryHistoryStore`(从 `moe.rs` 迁移,行为不变)
  - `sqlite.rs`:`SqliteHistoryStore`(v1.4.0 P1 新增,`Mutex<Connection>` + MessagePack + WAL)
- **新增 `HistoryPersistence` 配置 enum**(`config.rs`):`Memory`(默认,向后兼容)/ `Sqlite { db_path }`(opt-in)
- **修改 `RouterConfig`**:新增 `history_persistence` 字段,带 `#[serde(default)]`
- **修改 `moe.rs`**:迁移 trait + 常量到 `history` 模块,通过 `pub use` 保持 `crate::moe::HistoryStore` 路径可见(向后兼容)
- **修改 `error.rs`**:新增 `SqliteHistoryError(String)` 变体
- **修改 `lib.rs`**:导出 `history` 模块 + `HistoryPersistence` + `SqliteHistoryStore`
- **新增 `tests/history_test.rs`**:9 个 TDD 测试(record/get/persistence/UPSERT/roundtrip/spawn_blocking/config default/config backward compatible/memory-sqlite 等价)
- **新增 bench 对比**:`benches/moe_bench.rs` 增加 `history_store_record_bench` + `history_store_get_bench`
- **新增归档报告**:`docs/optimization/v1.4.0/p1_sqlite_history_report.md`
- **更新 CHANGELOG.md**:插入 P1 章节

## Impact

- **Affected specs**:model-router crate(L1 Core);M2 RL 路由(解除触发条件阻塞)
- **Affected code**:
  - `crates/model-router/src/history/{mod,memory,sqlite}.rs`(新建)
  - `crates/model-router/src/{config,error,lib,moe}.rs`(修改)
  - `crates/model-router/Cargo.toml`(新增 `rusqlite` + `rmp-serde` 依赖)
  - `crates/model-router/tests/history_test.rs`(新建)
  - `crates/model-router/benches/moe_bench.rs`(修改)
  - `docs/optimization/v1.4.0/p1_sqlite_history_report.md`(新建)
  - `CHANGELOG.md`(修改)
  - `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md`(归档原则 14/15/16)

## ADDED Requirements

### Requirement: SqliteHistoryStore 持久化实现

系统 SHALL 提供 `SqliteHistoryStore` 作为 `HistoryStore` trait 的 SQLite 持久化实现,跨进程重启保留历史数据。

#### Scenario: 基本记录与查询
- **WHEN** 调用 `SqliteHistoryStore::record(model_id, latency_ms, success)` 后调用 `get(model_id)`
- **THEN** 返回的 `HistoryRecord` 包含正确的 `success_count` / `total_count` / `latency_samples`

#### Scenario: 跨重启持久化
- **WHEN** `SqliteHistoryStore` Drop 后用相同 `db_path` 重新 `new()`
- **THEN** 之前 `record` 的历史数据完整保留(WAL 模式保证)

#### Scenario: UPSERT 累积与滑动窗口
- **WHEN** 对同一 `model_id` 连续 `record` 150 次
- **THEN** `total_count` = 150(累计统计,不滑动),`latency_samples.len()` = 100(滑动窗口淘汰最旧 50 条)

#### Scenario: 配置默认 Memory
- **WHEN** `RouterConfig::default()` 构造
- **THEN** `history_persistence` = `HistoryPersistence::Memory`(向后兼容 v1.3.0)

#### Scenario: 配置向后兼容
- **WHEN** 旧配置文件(v1.3.0,无 `history_persistence` 字段)反序列化为 `RouterConfig`
- **THEN** `history_persistence` = `HistoryPersistence::Memory`(`#[serde(default)]` 回退)

#### Scenario: SQLite opt-in
- **WHEN** 配置文件显式指定 `{"history_persistence": {"Sqlite": {"db_path": "history.db"}}}`
- **THEN** 反序列化后的 `RouterConfig.history_persistence` = `HistoryPersistence::Sqlite { db_path }`

#### Scenario: async 上下文不阻塞 runtime
- **WHEN** 在 `#[tokio::test]` 中用 `tokio::task::spawn_blocking` 包装 `record` 调用
- **THEN** 在 1 秒 timeout 内完成,不阻塞 async runtime

#### Scenario: Memory 与 SQLite 语义等价
- **WHEN** 对相同输入序列分别调用 `InMemoryHistoryStore` 与 `SqliteHistoryStore` 的 `record` + `get`
- **THEN** 两者返回的 `HistoryRecord` 字段值完全相同(交叉验证)

## MODIFIED Requirements

### Requirement: HistoryStore trait 定义位置

trait 与常量(`HISTORY_SUFFICIENT_THRESHOLD` / `LATENCY_WINDOW_CAPACITY`)的权威定义源从 `moe.rs` 迁移至 `history/mod.rs`。`moe.rs` 通过 `pub use crate::history::{...}` 保持 `crate::moe::HistoryStore` 路径可见(向后兼容,strategies.rs 等内部模块无需修改 import)。

### Requirement: RouterConfig 字段

`RouterConfig` 新增 `history_persistence: HistoryPersistence` 字段,带 `#[serde(default)]`。旧配置文件无此字段时回退到 `Memory`,保证 v1.3.0 配置文件无需修改即可加载。

### Requirement: RouterError 变体

`RouterError` 新增 `SqliteHistoryError(String)` 变体,用于 SQLite 操作失败(open / pragma / schema / query)。
