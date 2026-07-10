# Checklist

## 实现完整性

- [x] `crates/model-router/src/history/mod.rs` 存在且定义 `HistoryStore` trait + 两个常量
- [x] `crates/model-router/src/history/memory.rs` 存在且实现 `InMemoryHistoryStore`(行为不变)
- [x] `crates/model-router/src/history/sqlite.rs` 存在且实现 `SqliteHistoryStore`(Mutex<Connection> + WAL + MessagePack)
- [x] `crates/model-router/src/config.rs` 包含 `HistoryPersistence` enum + `RouterConfig.history_persistence` 字段
- [x] `crates/model-router/src/error.rs` 包含 `SqliteHistoryError(String)` 变体
- [x] `crates/model-router/src/lib.rs` 导出 `history` 模块 + `HistoryPersistence` + `SqliteHistoryStore`(prelude 同步)
- [x] `crates/model-router/src/moe.rs` 通过 `pub use` 保持 `crate::moe::HistoryStore` 路径可见
- [x] `crates/model-router/Cargo.toml` 添加 `rusqlite` + `rmp-serde` workspace 依赖

## 测试覆盖

- [x] `crates/model-router/tests/history_test.rs` 包含 9 个测试
- [x] test_sqlite_history_record_and_get 验证基本记录查询
- [x] test_sqlite_history_persistence_across_restart 验证跨重启持久化
- [x] test_sqlite_history_upsert_accumulates 验证 UPSERT 累积与滑动窗口(150 次 record)
- [x] test_sqlite_history_latency_samples_roundtrip 验证 VecDeque 序列化(空/单/满/超出/NaN/Inf 边界)
- [x] test_sqlite_history_spawn_blocking_not_blocking_runtime 验证 async 不阻塞(1 秒 timeout)
- [x] test_config_persistence_memory_default 验证默认 Memory
- [x] test_config_persistence_backward_compatible_without_field 验证旧配置向后兼容
- [x] test_config_persistence_sqlite_opt_in 验证 SQLite opt-in
- [x] test_memory_and_sqlite_behavior_equivalence 验证 Memory 与 SQLite 语义等价

## 代码质量

- [x] `cargo test -p model-router` 全量通过(143 passed / 0 failed / 0 ignored)
- [x] `cargo clippy -p model-router --all-targets -- -D warnings` 零警告
- [x] `cargo fmt -p model-router --check` 零 diff
- [x] 所有 crate 保持 `#![forbid(unsafe_code)]`
- [x] 不删除已有测试(TDD 守恒)
- [x] 不变更核心领域类型(RC 阶段约束)
- [x] 向后兼容(旧配置文件无需修改即可加载)

## bench 对比

- [x] `benches/moe_bench.rs` 包含 `history_store_record_bench` + `history_store_get_bench`
- [x] `criterion_group!` 注册新 bench
- [x] bench 数据已收集(record 中位数:memory 70.9ns / sqlite 98.0µs;get 中位数:memory 25.3ns / sqlite 38.7µs)

## 文档与归档

- [x] `docs/optimization/v1.4.0/p1_sqlite_history_report.md` 存在且包含设计决策 / 测试覆盖 / bench 数据 / 验证结果 / 向后兼容性 / 文件变更 / M2 解除阻塞说明
- [x] `CHANGELOG.md` 插入 P1 章节(倒序惯例,在 P0 章节前)
- [x] `project_memory.md` 末尾追加 `## v1.4.0-omega 总结教训(2026-07-10)` 章节
- [x] `project_memory.md` 追加原则 14(同步 trait 的 SQLite 实现)
- [x] `project_memory.md` 追加原则 15(MessagePack 序列化 VecDeque)
- [x] `project_memory.md` 追加原则 16(SQLite UPSERT 原子合并)
- [x] `project_memory.md` 关联文档列表追加 v1.4.0 报告路径

## 架构合规

- [x] 依赖方向合规(L1 内部模块重组,无跨层依赖引入)
- [x] 跨层通信规则无变更(未引入 Event Bus 新事件)
- [x] `Mutex<Connection>` 不跨 `.await`(§4.4 #1 反模式防护)
- [x] fire-and-forget 语义符合 §4.4 #7(record 失败仅记日志,不 panic)
- [x] 配置 `#[serde(default)]` 向后兼容设计(RC 阶段约束)
