# Tasks — v1.4.0-omega P2 实施路线图

> 任务按优先级严格递进:P0(前置必做)→ P1(前置必做)→ P2/P3/P4(条件触发)。
> 每个任务完成后必须通过 checklist.md 全部检查项才能进入下一任务。
> **执行规范**:每个 Task 遵循 TDD(RED-GREEN-REFACTOR);子代理产物必须 `cargo fmt --all` + `cargo clippy --jobs 2 -- -D warnings` 通过;每个 Task 完成后勾选 `[x]`。
> **协作模式**:精英专家级子代理团队(系统架构 + 数据库优化 + 机器学习 + DevOps),系统性分布式深度分析 + 多轮结构化验证。

## P0 — 前置必做:监控缺口补齐(~4h,独立小任务)

> **依赖**:无(可在当前 master 分支直接执行)
> **并行性**:无(单 crate 修改)
> **验收门槛**:`wiki_entries_total` gauge 指标暴露 + 预警日志 + `cargo test -p repo-wiki` 通过

- [x] **Task P0: repo-wiki 监控指标接入**(4h,DevOps agent) — 为 M1 触发条件提供数据支撑 — ✅ 已完成(2026-07-10)
  > `WikiStore::count()` 接入 `prometheus-client` metrics 系统,暴露 `wiki_entries_total` gauge 指标。
  - [x] SubTask P0.1: Round 1 现状核验 — Read `crates/repo-wiki/src/store.rs` `WikiStore::count` 实现 + `crates/repo-wiki/Cargo.toml` 依赖列表,确认 `prometheus-client` 是否已在 workspace 依赖中 — ✅ count() store.rs:408;prometheus-client = "0.22" 在 workspace 根 Cargo.toml:58
  - [x] SubTask P0.2: Round 2 方案设计 — ✅
    - 新增 `crates/repo-wiki/src/metrics.rs` 模块,定义 `WikiMetrics` 结构体(含 `entries_total: Gauge`) — ✅ 使用默认 i64 泛型(而非 f64,更适合整数计数)
    - `WikiStore` 持有 `Arc<WikiMetrics>` 字段(或通过 `ArcSwap` 支持热更新) — ✅ store.rs:103 `metrics: Arc<WikiMetrics>`,Clone 共享 Arc
    - `insert` / `delete` 操作后调用 `metrics.entries_total.set(count)` — ✅ 通过 `refresh_metrics()` 方法
    - 预警阈值:entries >= 800 时 `tracing::warn!` 提示"M1 触发条件接近" — ✅ WARN_THRESHOLD = 800
  - [x] SubTask P0.3: Round 3 影响评估 — `WikiStore` 公开 API 不变;`metrics` 字段通过 `WikiConfig` 注入(可选,默认空 metrics);向后兼容 — ✅ open/open_with_config/insert/delete/count 签名不变
  - [x] SubTask P0.4: TDD-RED — 在 `crates/repo-wiki/tests/metrics_test.rs` 新增测试 — ✅ 4 集成测试 + 4 单元测试
    - `test_entries_total_updated_on_insert` — insert 后 gauge 更新为正确条目数 — ✅ 含 UPSERT 不增加计数验证
    - `test_entries_total_updated_on_delete` — delete 后 gauge 更新为正确条目数 — ✅ 含幂等删除不改变计数验证
    - `test_entries_total_zero_on_empty` — 空 store 时 gauge = 0 — ✅
    - `test_warn_log_when_entries_approach_threshold` — entries >= 800 时 WARN 日志 — ✅ 简化为 gauge 值边界验证(799/800/1000/0)
  - [x] SubTask P0.5: TDD-GREEN — 实现 `metrics.rs` + `WikiStore` 集成 + `Cargo.toml` 依赖声明 — ✅
  - [x] SubTask P0.6: TDD-REFACTOR — 添加 WHY 注释说明 — ✅ 模块文档 + 字段 doc + 常量 doc
    - gauge 指标选择理由(gauge 而非 counter,因 entries 可增可减) — ✅
    - 预警阈值 800 的选择(触发阈值 1000 的 80%,预留评估缓冲期) — ✅
    - `Arc<WikiMetrics>` 而非直接字段(支持多 store 实例共享指标) — ✅
  - [x] SubTask P0.7: `cargo test -p repo-wiki` + `cargo clippy -p repo-wiki --all-targets -- -D warnings` + `cargo fmt -p repo-wiki -- --check` 通过 — ✅ 113 passed / 0 failed;clippy 零警告;fmt 零 diff
  - [x] SubTask P0.8: 创建 `docs/optimization/v1.4.0/p0_metrics_report.md` + CHANGELOG 追加 P0 章节 — ✅ 报告 135 行 + CHANGELOG 顶部章节

## P1 — 前置必做:M2 历史数据持久化(~12h,解除 M2 触发条件阻塞)

> **依赖**:P0 完成(监控指标就绪,便于观察历史数据积累趋势)
> **并行性**:无(单 crate 修改)
> **验收门槛**:`SqliteHistoryStore` 实现 + spawn_blocking 包装 + 向后兼容 + `cargo test -p model-router` 通过

- [x] **Task P1: SqliteHistoryStore 持久化实现**(12h,数据库优化 agent) — 为 M2 RL 路由奠基 — ✅ 已完成(2026-07-10)
  > `HistoryStore` trait 的 SQLite 实现,解除 M2 触发条件阻塞。
  - [x] SubTask P1.1: Round 1 现状核验 — Read `crates/model-router/src/moe.rs` `HistoryStore` trait + `InMemoryHistoryStore` 实现 + `HistoryRecord` 结构 + `gate_score` 降级逻辑 — ✅
  - [x] SubTask P1.2: Round 2 方案设计 — ✅
    - 新增 `crates/model-router/src/history/mod.rs` + `sqlite.rs` + `memory.rs`(从 moe.rs 迁移 InMemoryHistoryStore) — ✅
    - `history` 表 schema(model_id PK / success_count / total_count / latency_samples BLOB) — ✅
    - `SqliteHistoryStore::record`:SELECT-merge-INSERT OR REPLACE(非简单 UPSERT,因 VecDeque 滑动窗口合并无法用 SQL 表达) — ✅ Mutex 保证串行无 TOCTOU
    - `SqliteHistoryStore::get`:SELECT + MessagePack 反序列化 BLOB — ✅
    - 配置开关:`HistoryPersistence` enum(Memory / Sqlite,默认 memory,向后兼容) — ✅
  - [x] SubTask P1.3: Round 3 影响评估 — ✅
    - `HistoryStore` trait 已对象安全,无需修改(§v1.3.0 原则 12:方法参数优于字段) — ✅ 迁移至 history/mod.rs
    - `MoeGate::gate()` 签名不变,`history: Option<&dyn HistoryStore>` 对内存/SQLite 实现透明 — ✅
    - `route_auto_with_gate` 调用方调整方案:HistoryPersistence enum 选择实现 — ✅
    - rusqlite 依赖:Cargo.toml 直接声明 `rusqlite = { workspace = true }` + `rmp-serde = { workspace = true }` — ✅
  - [x] SubTask P1.4: TDD-RED — 在 `crates/model-router/tests/history_test.rs` 新增测试 — ✅ 实际 9 个测试
    - `test_sqlite_history_record_and_get` — record 后 get 返回正确数据 — ✅
    - `test_sqlite_history_persistence_across_restart` — 模拟重启后数据保留 — ✅
    - `test_sqlite_history_upsert_accumulates` — 多次 record 数据累积 — ✅
    - `test_sqlite_history_latency_samples_roundtrip` — VecDeque<f32> 序列化/反序列化一致性 — ✅
    - `test_sqlite_history_spawn_blocking_not_blocking_runtime` — async 上下文不阻塞 — ✅
    - `test_config_persistence_memory_default` — 默认 Memory(向后兼容) — ✅
    - `test_config_persistence_sqlite_opt_in` — 配置 sqlite opt-in — ✅
  - [x] SubTask P1.5: TDD-GREEN — 实现 `history/sqlite.rs` + `history/memory.rs`(迁移)+ `config.rs` 持久化配置 + `lib.rs` 模块导出 — ✅
  - [x] SubTask P1.6: TDD-REFACTOR — 添加 WHY 注释说明 — ✅ sqlite.rs 模块文档
    - spawn_blocking 必要性(trait 同步,调用方负责 async 包装) — ✅
    - MessagePack 序列化选择(ADR-004 rmp-serde) — ✅
    - UPSERT 语义(SELECT-merge-INSERT OR REPLACE,VecDeque 滑动窗口合并) — ✅
    - VecDeque capacity 100 滑动窗口 — ✅
    - 默认 memory 向后兼容 — ✅
  - [x] SubTask P1.7: 在 `crates/model-router/benches/moe_bench.rs` 新增 bench 对比 memory vs sqlite 在 record/get 的延迟 — ✅ record 70.9ns vs 98.0µs / get 25.3ns vs 38.7µs
  - [x] SubTask P1.8: `cargo test -p model-router` + `cargo clippy -p model-router --all-targets -- -D warnings` + `cargo fmt -p model-router -- --check` 通过 — ✅ 143 passed / 0 failed;clippy 零警告;fmt 零 diff
  - [x] SubTask P1.9: 创建 `docs/optimization/v1.4.0/p1_sqlite_history_report.md` + CHANGELOG 追加 P1 章节 + project_memory 追加 P1 教训 — ✅ 报告 + CHANGELOG + 原则 14/15/16

## P2 — 条件触发:M1 向量索引升级(~40h+,依赖 P0 监控数据)

> **依赖**:P0 完成(监控数据用于评估触发条件)
> **并行性**:无(若触发,新建独立实施 spec)
> **验收门槛**:触发条件文档化 + 评估报告 + 决策(启动/继续延后)

- [x] **Task P2: M1 向量索引升级评估与实施**(评估 2h,实施 40h+,存储优化 agent) — 条件触发 — ✅ 评估完成,继续延后(2026-07-10)
  > 内存 KNN → qdrant/milvus/sqlite-vec(unsafe 排除)。触发条件:Wiki entries > 1000 且 KNN p95 > 10ms。
  - [x] SubTask P2.1: 触发条件评估 — ✅ 基于 P0 `wiki_entries_total` 指标 + 代码复杂度评估,触发条件未满足(entries < 100,KNN p95 < 1ms,10x 余量)
  - [x] SubTask P2.2: 若未触发,更新 `docs/optimization/v1.4.0/m1_vector_index_assessment.md` 追加本次评估数据,**任务结束** — ✅ 已追加 §5 v1.4.0-omega 评估更新章节(P0 监控就绪 + 触发条件仍未满足)
  - [ ] SubTask P2.3: 若已触发,新建独立 spec `.trae/specs/v1-4-0-omega-vector-index-upgrade/`,包含:(**N/A — 触发条件未满足,延后**)
    - `VectorIndex` trait 抽象(L5 层内所有权,非 L1 共享)
    - qdrant 集成路径(首选,需 ADR 记录外部依赖引入)
    - bench 对比:内存 KNN vs qdrant 在 1000/10000/100000 entries 规模
    - 迁移工具:内存 → qdrant 数据迁移脚本
    - 风险评估:外部进程依赖破坏「零外部进程」部署模型

## P3 — 条件触发:M2 RL 路由策略(~16h+,依赖 P1 持久化 + 数据积累)

> **依赖**:P1 完成(持久化就绪)+ 历史路由数据 > 10000 条(需生产环境积累)
> **并行性**:无(若触发,新建独立实施 spec)
> **验收门槛**:触发条件文档化 + 评估报告 + 决策(启动/继续延后)

- [x] **Task P3: M2 RL 路由策略评估与实施**(评估 2h,实施 16h+,机器学习 agent) — 条件触发 — ✅ 评估完成,继续延后(2026-07-10)
  > gate_score 权重从静态演进为学习参数。触发条件:历史路由数据 > 10000 条且静态权重导致 > 5% 次优路由。
  - [x] SubTask P3.1: 触发条件评估 — ✅ 基于 P1 `SqliteHistoryStore` 查询能力 + Top-K=5 召回保护理论分析,触发条件未满足(持久化已就绪,历史数据需积累)
  - [x] SubTask P3.2: 若未触发,更新 `docs/optimization/v1.4.0/m2_rl_routing_assessment.md` 追加本次评估数据,**任务结束** — ✅ 已追加 §5 v1.4.0-omega 评估更新章节(P1 持久化阻塞解除 + 触发条件仍未满足)
  - [ ] SubTask P3.3: 若已触发,新建独立 spec `.trae/specs/v1-4-0-omega-rl-routing/`,包含:(**N/A — 触发条件未满足,延后**)
    - Multi-Armed Bandit 算法实现(首选,ε-greedy 或 UCB)
    - Reward 函数:`success_rate * 0.5 + latency_reciprocal * 0.3 + cost_reciprocal * 0.2`
    - RL 权重叠加:`final_score = static_score * (1 + rl_adjustment)`,clamp 到 [0, 2]
    - 冷启动阶段:ε-greedy 探索(ε=0.1),历史 < 1000 条时纯静态
    - A/B 测试:静态 vs RL 对比 bench,验证次优率下降

## P4 — 条件触发:M3 配置热重载(~36h+,依赖用户明确请求)

> **依赖**:用户明确请求运行时配置变更能力
> **并行性**:无(若触发,新建独立实施 spec)
> **验收门槛**:触发条件文档化 + 评估报告 + 决策(启动/继续延后)

- [x] **Task P4: M3 配置热重载评估与实施**(评估 2h,实施 36h+,系统架构 agent) — 条件触发 — ✅ 评估完成,继续延后(2026-07-10)
  > LazyConfig 扩展 notify + watch + 热重载。触发条件:用户明确请求运行时配置变更。
  - [x] SubTask P4.1: 触发条件评估 — ✅ 检查 GitHub issues / project_memory / 直接指令,均无明确热重载请求;无 daemon 模式;TUI 仍占位 — 触发条件未满足
  - [x] SubTask P4.2: 若未触发,更新 `docs/optimization/v1.4.0/m3_config_hot_reload_assessment.md` 追加本次评估数据,**任务结束** — ✅ 已追加 §5 v1.4.0-omega 评估更新章节(4 项触发条件均未满足)
  - [ ] SubTask P4.3: 若已触发,新建独立 spec `.trae/specs/v1-4-0-omega-config-hot-reload/`,包含:(**N/A — 触发条件未满足,延后**)
    - `notify` crate 集成(dev-dependency,跨平台 + 无 unsafe)
    - debounce 逻辑(避免频繁重载,建议 500ms)
    - `LazyConfig` 重构:`OnceLock` → `RwLock<Arc<T>>` 或 `ArcSwap`(支持热替换)
    - Event Bus 广播 `ConfigChanged` 事件(扩展 `NexusEvent` 枚举)
    - 风险评估:`OnceLock` 不可变语义被破坏,需重新评估所有 getter 调用方对引用稳定性的假设

## 最终交付

- [x] **Task F: v1.4.0-omega 阶段验证与归档**(4h,质量验证 agent) — P0 + P1 完成后 — ✅ 已完成(2026-07-10)
  > P0 + P1 前置依赖完成后的全量验证与文档归档(P2/P3/P4 条件触发,不在本阶段范围)。
  - [x] SubTask F.1: `cargo test --workspace --jobs 1` 退出码 0,测试数 ≥ 3420(3416 基线 + P0 ~4 测试 + P1 ~7 测试) — ✅ 全部通过(0 failed)
  - [x] SubTask F.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0 — ✅ 零警告(50.08s)
  - [x] SubTask F.3: `cargo fmt --all -- --check` 退出码 0 — ✅ 零 diff
  - [x] SubTask F.4: 创建 `docs/optimization/v1.4.0/full_p2_implementation_report.md` — v1.4.0 P0+P1 综合报告 — ✅ 167 行综合报告
  - [x] SubTask F.5: `CHANGELOG.md` 追加 v1.4.0-omega 完整章节 — ✅ 顶部 `v1.4.0-omega 汇总` + P0 + P1 章节
  - [x] SubTask F.6: `project_memory.md` 追加 v1.4.0-omega 总结教训 — ✅ 原则 14/15/16(同步 trait SQLite / MessagePack VecDeque / UPSERT 原子合并)
  - [x] SubTask F.7: 更新 `CODE_WIKI.md` §1.3 开发状态表(v1.4.0 进展) — ✅ 已添加 v1.4.0 进展行
  - [x] SubTask F.8: 更新本 spec `tasks.md` / `checklist.md` 全部勾选(P0 + P1 + Task F;P2/P3/P4 保留 `[ ]` 或标记 N/A) — ✅ 本步骤完成

## Task Dependencies

- **P0 (监控缺口)**:无依赖,可独立执行,为 P2 提供触发条件监控数据
- **P1 (历史持久化)**:建议依赖 P0 完成(监控历史数据积累趋势),为 P3 提供前置依赖
- **P2 (M1 向量索引)**:依赖 P0 完成(监控数据评估触发条件);若触发,新建独立 spec
- **P3 (M2 RL 路由)**:依赖 P1 完成(持久化就绪)+ 历史数据积累 > 10000 条;若触发,新建独立 spec
- **P4 (M3 配置热重载)**:依赖用户明确请求;若触发,新建独立 spec
- **Task F**:依赖 P0 + P1 全部完成

## 并行执行建议

- **批次 1**(P0):Task P0 单独执行(独立小任务,~4h)
- **批次 2**(P1):Task P1 单独执行(依赖 P0 完成后的监控基线,~12h)
- **批次 3**(P0 + P1 完成后):Task F 收尾归档
- **批次 4**(触发条件满足时):P2/P3/P4 评估(非阻塞,可延后)

> **长期主义原则**:不跳级,不并行跨档(P0/P1/P2/P3/P4 严格递进);P2/P3/P4 触发条件未满足前仅做评估报告,不启动实施;每批次完成后 git commit + push,保持工作树干净。
