# Tasks

- [x] Task 1: 设计 `history` 模块结构(mod/memory/sqlite 三文件)
  - [x] SubTask 1.1: 创建 `crates/model-router/src/history/mod.rs`,定义 `HistoryStore` trait + `HISTORY_SUFFICIENT_THRESHOLD` / `LATENCY_WINDOW_CAPACITY` 常量
  - [x] SubTask 1.2: 创建 `crates/model-router/src/history/memory.rs`,迁移 `InMemoryHistoryStore`(从 `moe.rs`)
  - [x] SubTask 1.3: 创建 `crates/model-router/src/history/sqlite.rs`,实现 `SqliteHistoryStore`(`Mutex<Connection>` + MessagePack + WAL + UPSERT)

- [x] Task 2: 迁移 HistoryStore trait + InMemoryHistoryStore + 常量
  - [x] SubTask 2.1: 从 `moe.rs` 移除 trait 定义、`InMemoryHistoryStore`、两个常量
  - [x] SubTask 2.2: 在 `moe.rs` 顶部用 `pub use crate::history::{HistoryStore, InMemoryHistoryStore, HISTORY_SUFFICIENT_THRESHOLD, LATENCY_WINDOW_CAPACITY};` 保持 `crate::moe::HistoryStore` 路径可见
  - [x] SubTask 2.3: 验证 `strategies.rs` 等内部模块无需修改 import(向后兼容)

- [x] Task 3: 新增 SqliteHistoryStore 实现(SQLite + MessagePack + UPSERT)
  - [x] SubTask 3.1: 实现 `new(path: &Path) -> Result<Self, RouterError>`(打开 + WAL + init_schema)
  - [x] SubTask 3.2: 实现 `get(&self, model_id: &str) -> Option<HistoryRecord>`(SELECT + 反序列化 BLOB)
  - [x] SubTask 3.3: 实现 `record(&self, model_id: &str, latency_ms: f32, success: bool)`(SELECT-merge-INSERT OR REPLACE,fire-and-forget 语义)
  - [x] SubTask 3.4: 添加 3 个单元测试(test_new_creates_schema / test_new_reuses_existing_database / test_db_path_accessor)

- [x] Task 4: 新增 HistoryPersistence 配置(默认 Memory,SQLite opt-in)
  - [x] SubTask 4.1: 在 `config.rs` 新增 `HistoryPersistence` enum(`Memory` / `Sqlite { db_path: PathBuf }`)
  - [x] SubTask 4.2: 实现 `Default for HistoryPersistence`(返回 `Memory`)
  - [x] SubTask 4.3: `RouterConfig` 新增 `history_persistence` 字段(带 `#[serde(default)]`)
  - [x] SubTask 4.4: 更新 `Default for RouterConfig` 添加 `history_persistence: HistoryPersistence::default()`
  - [x] SubTask 4.5: `error.rs` 新增 `SqliteHistoryError(String)` 变体
  - [x] SubTask 4.6: `lib.rs` 导出 `history` 模块 + `HistoryPersistence` + `SqliteHistoryStore`(prelude 同步更新)
  - [x] SubTask 4.7: `Cargo.toml` 添加 `rusqlite = { workspace = true }` + `rmp-serde = { workspace = true }`

- [x] Task 5: 新增 9 个 TDD 测试 + bench 对比
  - [x] SubTask 5.1: 创建 `crates/model-router/tests/history_test.rs`
  - [x] SubTask 5.2: 编写 9 个测试(record_and_get / persistence_across_restart / upsert_accumulates / latency_samples_roundtrip / spawn_blocking_not_blocking_runtime / config_persistence_memory_default / config_persistence_backward_compatible_without_field / config_persistence_sqlite_opt_in / memory_and_sqlite_behavior_equivalence)
  - [x] SubTask 5.3: 在 `benches/moe_bench.rs` 添加 `history_store_record_bench` + `history_store_get_bench`
  - [x] SubTask 5.4: 更新 `criterion_group!` 注册新 bench

- [x] Task 6: 验证与文档同步
  - [x] SubTask 6.1: `cargo test -p model-router` 全量通过(143 passed / 0 failed / 0 ignored)
  - [x] SubTask 6.2: `cargo clippy -p model-router --all-targets -- -D warnings` 零警告
  - [x] SubTask 6.3: `cargo fmt -p model-router --check` 零 diff
  - [x] SubTask 6.4: `cargo bench -p model-router --bench moe_bench -- history_store` 收集 bench 数据

- [x] Task 7: 归档报告与 CHANGELOG
  - [x] SubTask 7.1: 创建 `docs/optimization/v1.4.0/p1_sqlite_history_report.md`(设计决策 / 测试覆盖 / bench 数据 / 验证结果 / 向后兼容性 / 文件变更 / M2 解除阻塞说明)
  - [x] SubTask 7.2: 更新 `CHANGELOG.md`(在 P0 章节前插入 P1 章节,倒序惯例)

- [x] Task 8: 更新 project_memory.md(归档原则 14/15/16)
  - [x] SubTask 8.1: 在 `project_memory.md` 末尾追加 `## v1.4.0-omega 总结教训(2026-07-10)` 章节
  - [x] SubTask 8.2: 追加原则 14(同步 trait 的 SQLite 实现)
  - [x] SubTask 8.3: 追加原则 15(MessagePack 序列化 VecDeque)
  - [x] SubTask 8.4: 追加原则 16(SQLite UPSERT 原子合并)
  - [x] SubTask 8.5: 更新"关联文档"列表追加 v1.4.0 报告路径

# Task Dependencies

- Task 2 依赖 Task 1(迁移前需先建立新模块)
- Task 3 依赖 Task 2(SqliteHistoryStore 实现 HistoryStore trait)
- Task 4 依赖 Task 3(配置切换需要实现可选)
- Task 5 依赖 Task 4(测试验证完整实现)
- Task 6 依赖 Task 5(验证在实现与测试完成后)
- Task 7 依赖 Task 6(归档报告需要验证结果数据)
- Task 8 依赖 Task 7(归档原则提炼需要报告作为来源)
