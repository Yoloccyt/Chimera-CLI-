# Checklist — v1.4.0-omega P2 实施路线图

> 每个检查项对应 spec.md 的 Requirement 与 tasks.md 的 Task。
> 所有检查项必须勾选 `[x]` 才能声明任务完成。
> P0/P1 必做(前置依赖);P2/P3/P4 条件触发(未触发仅做评估报告)。

## Task P0: repo-wiki 监控指标接入(前置必做)

### SubTask P0.1-P0.3: 设计与影响评估

- [x] `crates/repo-wiki/src/store.rs` `WikiStore::count` 实现已核验 — ✅ store.rs:408,async 方法,SELECT COUNT(*) FROM entries
- [x] `crates/repo-wiki/Cargo.toml` 依赖列表已核验(`prometheus-client` 是否在 workspace 依赖中) — ✅ workspace 根 Cargo.toml:58 `prometheus-client = "0.22"`,repo-wiki Cargo.toml 已添加 `prometheus-client = { workspace = true }`
- [x] `WikiMetrics` 结构体设计完成(含 `entries_total: Gauge`) — ✅ metrics.rs,使用默认 i64 泛型(而非 f64,更适合整数计数)
- [x] `WikiStore` 持有 `Arc<WikiMetrics>` 字段设计完成 — ✅ store.rs:103 `metrics: Arc<WikiMetrics>`,Clone 共享 Arc
- [x] `insert` / `delete` 操作后调用 `metrics.entries_total.set(count)` 设计完成 — ✅ 通过 `refresh_metrics()` 方法,调用 count() + set_entries()
- [x] 预警阈值设计完成(entries >= 800 时 `tracing::warn!`) — ✅ WARN_THRESHOLD 常量 = 800,在 set_entries() 中检查
- [x] `WikiStore` 公开 API 不变(metrics 字段通过 `WikiConfig` 注入,向后兼容) — ✅ open/open_with_config/insert/delete/count 签名不变,新增 metrics() + refresh_metrics()

### SubTask P0.4-P0.6: TDD 实现

- [x] 4 个 TDD 测试已新增(insert 更新 / delete 更新 / 空 store = 0 / WARN 日志) — ✅ tests/metrics_test.rs 4 测试 + metrics.rs 4 单元测试
- [x] `crates/repo-wiki/src/metrics.rs` 已创建(`WikiMetrics` 结构体) — ✅ 含 WikiMetrics + set_entries + M1_TRIGGER_THRESHOLD/WARN_THRESHOLD 常量
- [x] `WikiStore` 集成 metrics 完成 — ✅ store.rs 新增 metrics 字段 + Clone + open_with_config + insert/delete 刷新
- [x] `Cargo.toml` 依赖声明完成(`prometheus-client`) — ✅ `prometheus-client = { workspace = true }`
- [x] WHY 注释说明:gauge 而非 counter / 预警阈值 800 选择 / `Arc<WikiMetrics>` 设计 — ✅ metrics.rs 模块文档 + 字段 doc + 常量 doc

### SubTask P0.7-P0.8: 验证与归档

- [x] `cargo test -p repo-wiki` 退出码 0(含新增 4 测试) — ✅ 113 passed / 0 failed
- [x] `cargo clippy -p repo-wiki --all-targets -- -D warnings` 零警告 — ✅ 退出码 0
- [x] `cargo fmt -p repo-wiki -- --check` 零 diff — ✅ 退出码 0
- [x] `docs/optimization/v1.4.0/p0_metrics_report.md` 已创建(含设计决策 + 测试覆盖 + 指标说明) — ✅ 135 行
- [x] `CHANGELOG.md` 追加 P0 章节 — ✅ 顶部 `v1.4.0-omega P0` 章节

## Task P1: SqliteHistoryStore 持久化实现(前置必做)

### SubTask P1.1-P1.3: 设计与影响评估

- [x] `crates/model-router/src/moe.rs` `HistoryStore` trait 已核验(对象安全,`&self` 方法 + owned 返回) — ✅ moe.rs:190-198
- [x] `InMemoryHistoryStore` 实现已核验(DashMap + `entry().or_default()` 原子写入) — ✅ moe.rs:215-242
- [x] `HistoryRecord` 结构已核验(success_count / total_count / latency_samples VecDeque<f32> capacity 100) — ✅ moe.rs:94-102
- [x] `history` 表 schema 设计完成(model_id PK / success_count / total_count / latency_samples BLOB) — ✅ sqlite.rs
- [x] `SqliteHistoryStore::record` 设计完成(SELECT-merge-INSERT OR REPLACE) — ✅ 非简单 UPSERT,因 VecDeque 滑动窗口合并无法用 SQL 表达
- [x] `SqliteHistoryStore::get` 设计完成(SELECT + 反序列化 BLOB) — ✅ MessagePack 反序列化
- [x] 配置开关设计完成(`HistoryPersistence` enum: Memory / Sqlite,默认 Memory) — ✅ config.rs
- [x] `HistoryStore` trait 无需修改(对象安全,对内存/SQLite 实现透明) — ✅ 迁移至 history/mod.rs
- [x] `MoeGate::gate()` 签名不变(`history: Option<&dyn HistoryStore>`) — ✅
- [x] `route_auto_with_gate` 调用方调整方案完成(根据配置选择实现) — ✅ HistoryPersistence enum
- [x] rusqlite 依赖确认(model-router 是否需直接声明) — ✅ Cargo.toml 添加 `rusqlite = { workspace = true }` + `rmp-serde = { workspace = true }`

### SubTask P1.4-P1.6: TDD 实现

- [x] 7 个 TDD 测试已新增(record+get / 持久化 / 累积 / VecDeque 序列化 / spawn_blocking 非阻塞 / 默认 memory / opt-in sqlite) — ✅ 实际 9 个测试(history_test.rs)
- [x] `crates/model-router/src/history/mod.rs` 已创建(模块导出) — ✅ HistoryStore trait + 常量
- [x] `crates/model-router/src/history/sqlite.rs` 已创建(`SqliteHistoryStore`) — ✅ Mutex<Connection> + WAL + MessagePack + UPSERT
- [x] `crates/model-router/src/history/memory.rs` 已创建(从 moe.rs 迁移 `InMemoryHistoryStore`) — ✅
- [x] `crates/model-router/src/config.rs` 持久化配置完成(persistence 字段 + serde default) — ✅ HistoryPersistence enum
- [x] `crates/model-router/src/lib.rs` 模块导出完成 — ✅ `pub mod history;` + 重导出
- [x] WHY 注释说明:spawn_blocking 必要性 / MessagePack 序列化 / UPSERT 语义 / VecDeque capacity / 默认 memory 向后兼容 — ✅ sqlite.rs 模块文档

### SubTask P1.7-P1.9: 验证与归档

- [x] `crates/model-router/benches/moe_bench.rs` 新增 memory vs sqlite record/get 延迟对比 bench — ✅ 2 bench(record 70.9ns vs 98.0µs / get 25.3ns vs 38.7µs)
- [x] `cargo test -p model-router` 退出码 0(含新增 7 测试) — ✅ 143 passed / 0 failed
- [x] `cargo clippy -p model-router --all-targets -- -D warnings` 零警告 — ✅
- [x] `cargo fmt -p model-router -- --check` 零 diff — ✅
- [x] `docs/optimization/v1.4.0/p1_sqlite_history_report.md` 已创建 — ✅
- [x] `CHANGELOG.md` 追加 P1 章节 — ✅ 顶部 `v1.4.0-omega P1` 章节
- [x] `project_memory.md` 追加 P1 教训 — ✅ 原则 14/15/16

## Task P2: M1 向量索引升级(条件触发)

- [x] 触发条件已评估(Wiki entries 规模 via P0 `wiki_entries_total` + KNN p95 via `vector_bench.rs`) — ✅ entries < 100,KNN p95 < 1ms(10x 余量),触发条件未满足
- [x] 若未触发:`docs/optimization/v1.4.0/m1_vector_index_assessment.md` 追加本次评估数据,任务结束 — ✅ 已追加 §5 v1.4.0-omega 评估更新章节
- [ ] ~~若已触发:新建独立 spec `.trae/specs/v1-4-0-omega-vector-index-upgrade/`(不在本 spec 范围)~~ — **N/A:触发条件未满足,延后**

## Task P3: M2 RL 路由策略(条件触发)

- [x] 触发条件已评估(历史数据规模 via `SELECT COUNT(*) FROM history` + 静态权重次优率 via 新增 bench) — ✅ 持久化已就绪(P1),历史数据需积累(未达 10000 条),触发条件未满足
- [x] 若未触发:`docs/optimization/v1.4.0/m2_rl_routing_assessment.md` 追加本次评估数据,任务结束 — ✅ 已追加 §5 v1.4.0-omega 评估更新章节
- [ ] ~~若已触发:新建独立 spec `.trae/specs/v1-4-0-omega-rl-routing/`(不在本 spec 范围)~~ — **N/A:触发条件未满足,延后**

## Task P4: M3 配置热重载(条件触发)

- [x] 触发条件已评估(是否有用户明确请求) — ✅ 无用户请求,无 daemon 模式,TUI 仍占位 — 触发条件未满足
- [x] 若未触发:`docs/optimization/v1.4.0/m3_config_hot_reload_assessment.md` 追加本次评估数据,任务结束 — ✅ 已追加 §5 v1.4.0-omega 评估更新章节
- [ ] ~~若已触发:新建独立 spec `.trae/specs/v1-4-0-omega-config-hot-reload/`(不在本 spec 范围)~~ — **N/A:触发条件未满足,延后**

## Task F: v1.4.0-omega 阶段验证与归档(P0 + P1 完成后)

- [x] `cargo test --workspace --jobs 1` 退出码 0,测试数 ≥ 3420(3416 基线 + P0 ~4 + P1 ~7) — ✅ 全部通过(0 failed)
- [x] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0 — ✅ 零警告(50.08s)
- [x] `cargo fmt --all -- --check` 退出码 0 — ✅ 零 diff
- [x] `docs/optimization/v1.4.0/full_p2_implementation_report.md` 已创建 — ✅ 167 行综合报告
- [x] `CHANGELOG.md` 追加 v1.4.0-omega 完整章节 — ✅ 顶部 `v1.4.0-omega 汇总` + P0 + P1 章节
- [x] `project_memory.md` 追加 v1.4.0-omega 总结教训 — ✅ 原则 14/15/16
- [x] `CODE_WIKI.md` §1.3 开发状态表已更新(v1.4.0 进展) — ✅ 已添加 v1.4.0 进展行
- [x] 本 spec `tasks.md` / `checklist.md` 全部勾选(P0 + P1 + Task F;P2/P3/P4 保留 `[ ]` 或标记 N/A) — ✅ 本步骤完成

## 跨任务通用检查

- [x] 所有变更遵守 §2.2 依赖铁律(L(N)→L(N-1) 允许,L(N)→L(N+1) 禁止) — ✅ P0 在 L5(repo-wiki)内;P1 在 L1(model-router)内
- [x] 所有变更遵守 OMEGA 四定律(Ω-Sparse / Ω-Compress / Ω-Evolve / Ω-Event) — ✅ 无核心架构变更
- [x] 所有 crate 保持 `#![forbid(unsafe_code)]`(sqlite-vec 排除,notify 无 unsafe,rusqlite bundled 不传播) — ✅ P1 验证 SqliteHistoryStore 无 unsafe,rusqlite bundled 内部 unsafe 不传播
- [x] 所有 async fn 满足 `Send + 'static` 约束 — ✅
- [x] 所有 rusqlite 调用通过 `spawn_blocking` 包装(§4.4 #2,P1 强制) — ✅ HistoryStore trait 同步,调用方 spawn_blocking 包装(已验证 `test_sqlite_history_spawn_blocking_not_blocking_runtime`)
- [x] 所有变更遵循 TDD(RED-GREEN-REFACTOR),先写失败测试再实现 — ✅ P0 4+4 测试,P1 9 测试
- [x] 不删除已有测试,只允许增强 — ✅
- [x] 所有变更遵循 §3.3.1.5 向后兼容(SemVer,破坏性变更需 major 版本升级) — ✅ P0 API 签名不变;P1 默认 Memory,SQLite opt-in
- [x] 不变更核心领域类型(UserIntent / Quest / Checkpoint / OmniSparseMasks / CLV / NexusState) — ✅
- [x] 不新建 crate(严格遵守 §3.3.1.6 新 crate 准入;P2 qdrant 若触发需 ADR) — ✅ P2 未触发,无新 crate
- [x] 单函数 ≤ 200 行 — ✅ SqliteHistoryStore::record/get 均远小于 200 行
- [x] 所有关键决策有 WHY 注释(隐藏约束 / 变通方案 / 反直觉行为) — ✅ P0/P1 均有 WHY 注释
- [x] 优先级严格递进(P0 → P1 → P2/P3/P4,不跳级,不并行跨档) — ✅ P0 → P1 → P2/P3/P4 评估 → Task F
- [x] P2/P3/P4 触发条件未满足前仅做评估报告,不启动实施(长期主义) — ✅ 评估完成,均延后
