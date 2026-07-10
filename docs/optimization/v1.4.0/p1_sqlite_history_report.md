# P1: SqliteHistoryStore 持久化实现报告

> 任务:v1.4.0-omega P1(model-router)
> 日期:2026-07-10
> 目标:为 `HistoryStore` trait 实现 SQLite 持久化,解除 M2 RL 路由触发条件阻塞(历史数据 > 10000 条)

## 1. 设计决策

### 1.1 trait 保持同步(向后兼容)

`HistoryStore` trait 是同步的(`&self` 方法,非 async),`SqliteHistoryStore` 的 `get`/`record` 也是同步的。

**WHY**:trait 是 v1.3.0 已发布的公共 API,变更为 async 会破坏对象安全性与 `MoeGate::gate()` 签名(同步方法)。SQLite 单行 UPSERT/SELECT 是微秒级操作(见 §3 bench),在同步上下文中调用可接受。调用方在 async 上下文中调用 `gate()` 时,需用 `tokio::task::spawn_blocking` 包装整个 `gate()` 调用(而非单个 record/get),避免阻塞 runtime。

### 1.2 MessagePack 序列化(`VecDeque<f32>` → BLOB)

`latency_samples: VecDeque<f32>` 用 `rmp-serde` 序列化为 BLOB 存储。

**WHY rmp-serde 而非 JSON**:
- ADR-004 已采用 MessagePack 作为项目消息序列化协议,复用一致选型
- 二进制格式比 JSON 紧凑(100 个 f32 ≈ 500 字节 vs JSON ~1200 字符)
- `VecDeque<f32>` 已实现 serde `Serialize`/`Deserialize`(标准支持),无需自定义序列化

### 1.3 UPSERT 语义:SELECT-merge-INSERT OR REPLACE

`record` 实现采用 `SELECT 旧值 → 合并 → INSERT OR REPLACE` 三步流程。

**WHY 不用 `ON CONFLICT DO UPDATE SET`**:SQLite UPSERT 只能做简单算术合并(`success_count = success_count + ?`),无法表达 `VecDeque` 滑动窗口的 `pop_front + push_back` 合并语义。`Mutex<Connection>` 保证串行访问,SELECT-merge-INSERT OR REPLACE 在 Mutex 保护下不存在 TOCTOU(§4.4 #1 反模式:锁内取快照→释放→await;这里锁内全部完成,不跨 await)。

### 1.4 默认 Memory,SQLite 为 opt-in

`RouterConfig.history_persistence: HistoryPersistence` 字段,默认 `Memory`(v1.3.0 行为不变)。SQLite 通过配置显式启用:

```json
{
  "history_persistence": {
    "Sqlite": { "db_path": "history.db" }
  }
}
```

`#[serde(default)]` 保证旧配置文件(v1.3.0 无此字段)回退到 `Memory`,向后兼容。

### 1.5 WAL 模式

启用 `journal_mode=WAL` + `synchronous=NORMAL`(与 repo-wiki 一致)。WAL 在进程异常退出后,下次打开自动恢复未 checkpoint 的数据,保证持久化测试(Drop → 重新打开)数据不丢失。

## 2. 测试覆盖

新建 `crates/model-router/tests/history_test.rs`,共 9 个测试(7 要求 + 2 附加):

| # | 测试名 | 验证目标 |
|---|--------|---------|
| 1 | `test_sqlite_history_record_and_get` | record 后 get 返回正确数据(success/total/latency) |
| 2 | `test_sqlite_history_persistence_across_restart` | Drop → 重新打开同路径,数据保留 |
| 3 | `test_sqlite_history_upsert_accumulates` | 150 次 record 累积:total_count=150,滑动窗口=100 |
| 4 | `test_sqlite_history_latency_samples_roundtrip` | VecDeque 序列化一致性(空/单/满/超出/NaN/Inf) |
| 5 | `test_sqlite_history_spawn_blocking_not_blocking_runtime` | async + spawn_blocking 不阻塞 runtime(timeout 验证) |
| 6 | `test_config_persistence_memory_default` | 默认 Memory(向后兼容) |
| 7 | `test_config_persistence_backward_compatible_without_field` | 旧配置无字段 → Memory |
| 8 | `test_config_persistence_sqlite_opt_in` | 显式配置 Sqlite |
| 9 | `test_memory_and_sqlite_behavior_equivalence` | Memory 与 SQLite 语义等价(交叉验证) |

**完整测试结果**:143 passed / 0 failed / 0 ignored(含 lib 单元 74 + 集成 65 + doctest 4)。

## 3. bench 数据(memory vs sqlite 延迟对比)

运行命令:`cargo bench -p model-router --bench moe_bench -- history_store`

### 3.1 record 延迟(单次 UPSERT 全流程)

| 实现 | 延迟(中位数) | 范围 | 说明 |
|------|-------------|------|------|
| memory | 70.9 ns | 69.2 - 71.3 ns | DashMap entry().or_default() + 内存操作 |
| sqlite | 98.0 µs | 60.3 - 248.7 µs | SELECT + 反序列化 + 合并 + 序列化 + INSERT OR REPLACE |

**比率**:sqlite 比 memory 慢约 **1380x**(98µs / 0.071µs)。SQLite 波动较大(WAL fsync 时机不确定),但均在微秒级。

### 3.2 get 延迟(单次 SELECT + 反序列化)

| 实现 | 延迟(中位数) | 范围 | 说明 |
|------|-------------|------|------|
| memory | 126.8 ns | 126.6 - 126.9 ns | DashMap get + clone(400B) |
| sqlite | 5.80 µs | 5.77 - 5.80 µs | SELECT + BLOB 反序列化 → VecDeque<f32> |

**比率**:sqlite 比 memory 慢约 **46x**(5.80µs / 0.127µs)。

### 3.3 结论

SQLite 延迟在微秒级(100-250µs record,5.8µs get),在路由热路径(单次决策)上可接受。MoeGate::gate() 对 N 个模型查询历史,O(N) 次 get;50 模型规模下 SQLite gate 的历史查询总开销 ≈ 50 × 5.8µs = 290µs,远低于路由决策的可接受延迟(< 1ms)。Memory 实现在性能敏感场景仍是首选,SQLite 适用于需要跨重启累积历史的 M2 RL 路由场景。

## 4. 验证结果

| 检查项 | 命令 | 结果 |
|--------|------|------|
| 测试 | `cargo test -p model-router --jobs 1` | 143 passed / 0 failed / 0 ignored |
| clippy | `cargo clippy -p model-router --all-targets --jobs 2 -- -D warnings` | 零警告 |
| fmt | `cargo fmt -p model-router -- --check` | 零 diff |
| bench 编译 | `cargo bench -p model-router --bench moe_bench --no-run` | 通过 |

## 5. 向后兼容性说明

| 维度 | 兼容性 | 说明 |
|------|--------|------|
| `HistoryStore` trait 签名 | 不变 | get/record 方法签名完全保持 |
| `MoeGate::gate()` 签名 | 不变 | `Option<&dyn HistoryStore>` 参数透明 |
| `RouterConfig` 默认行为 | 不变 | 默认 Memory,旧配置无 `history_persistence` 字段时回退 Memory |
| `InMemoryHistoryStore` 行为 | 不变 | 从 moe.rs 迁移至 history/memory.rs,代码逻辑完全保持 |
| `crate::moe::HistoryStore` 路径 | 有效 | moe.rs 通过 `pub use` 重导出,向后兼容 |
| `crate::moe::InMemoryHistoryStore` 路径 | 有效 | 同上 |
| 现有测试 | 全部通过 | moe_test(16)/router(13)/cacr(22)/top_k(3) 不破坏 |

## 6. 文件变更

### 新建文件
- `crates/model-router/src/history/mod.rs` — 模块导出 + HistoryStore trait + 常量(权威源)
- `crates/model-router/src/history/memory.rs` — InMemoryHistoryStore(从 moe.rs 迁移)
- `crates/model-router/src/history/sqlite.rs` — SqliteHistoryStore(新实现)
- `crates/model-router/tests/history_test.rs` — 9 个 TDD 测试
- `docs/optimization/v1.4.0/p1_sqlite_history_report.md` — 本报告

### 修改文件
- `crates/model-router/Cargo.toml` — 添加 `rusqlite` + `rmp-serde` 依赖
- `crates/model-router/src/lib.rs` — 新增 `pub mod history` + 重导出
- `crates/model-router/src/moe.rs` — 移除 trait/InMemory/常量,`pub use` 重导出
- `crates/model-router/src/config.rs` — 新增 `HistoryPersistence` enum + RouterConfig 字段
- `crates/model-router/src/error.rs` — 新增 `SqliteHistoryError` 变体
- `crates/model-router/benches/moe_bench.rs` — 新增 memory vs sqlite 延迟对比 bench

## 7. 对 M2 RL 路由的解除阻塞

M2 RL 路由触发条件需历史数据 > 10000 条。`InMemoryHistoryStore` 在进程重启后丢失历史,短周期内无法累积 10000 条。`SqliteHistoryStore` 跨重启保留历史,使长周期统计累积成为可能:

- 配置 `history_persistence = HistoryPersistence::Sqlite { db_path }` 启用持久化
- 进程重启后自动加载已有历史,继续累积
- 当 `total_count >= 10000` 时(可配置),M2 RL 路由可触发

**注意**:SqliteHistoryStore 仅提供持久化基础,M2 RL 路由的具体实现(> 10000 条触发逻辑)是后续任务,本任务不涉及。
