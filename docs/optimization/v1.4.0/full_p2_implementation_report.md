# v1.4.0-omega P2 实施综合报告

> 版本:v1.4.0-omega
> 日期:2026-07-10
> 范围:P0(监控缺口补齐)+ P1(M2 历史数据持久化)+ P2/P3/P4(条件触发评估)
> 基线:v1.3.0-omega(3416 passed / 0 failed / 56 ignored)

---

## 1. 实施概述

v1.4.0-omega P2 实施路线图包含 5 个优先级层级,本阶段完成 P0 + P1 前置必做任务,
P2/P3/P4 为条件触发任务(触发条件未满足,仅做评估报告)。

| 优先级 | 任务 | 状态 | 工时 |
|--------|------|------|------|
| P0 | repo-wiki 监控指标接入 | ✅ 完成 | ~4h |
| P1 | SqliteHistoryStore 持久化 | ✅ 完成 | ~12h |
| P2 | M1 向量索引升级 | ⏸ 未触发(entries < 100, KNN p95 < 1ms) | 评估完成 |
| P3 | M2 RL 路由策略 | ⏸ 未触发(持久化已就绪,历史数据需积累) | 评估完成 |
| P4 | M3 配置热重载 | ⏸ 未触发(无用户请求,无 daemon 模式) | 评估完成 |

---

## 2. P0: repo-wiki 监控指标接入

### 2.1 目标
为 M1 向量索引升级的触发条件(Wiki entries > 1000 且 KNN p95 > 10ms)提供数据支撑。
暴露 `wiki_entries_total` gauge 指标,运维可通过 Prometheus 监控 entries 增长趋势。

### 2.2 设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 指标类型 | Gauge(非 Counter) | entries 可因 delete 减少,Gauge 支持可增可减 |
| 数值类型 | i64(非 f64) | 条目数是整数,默认泛型参数更简洁(prometheus-client 0.22 默认) |
| 共享方式 | Arc<WikiMetrics> | WikiStore::clone 共享写线程,指标也需共享一致视图 |
| 预警阈值 | 800(80%) | 触发阈值 1000 的 80%,预留 200 条目缓冲期 |
| 刷新时机 | insert/delete 后自动 | 调用 count() + set_entries(),保证 gauge 与实际数据一致 |

### 2.3 文件变更

| 文件 | 变更 | 说明 |
|------|------|------|
| `crates/repo-wiki/Cargo.toml` | 修改 | 添加 `prometheus-client = { workspace = true }` |
| `crates/repo-wiki/src/metrics.rs` | 新增 | WikiMetrics 结构体 + set_entries + 阈值常量 + 4 单元测试 |
| `crates/repo-wiki/src/lib.rs` | 修改 | 导出 metrics 模块 + WikiMetrics 重导出 |
| `crates/repo-wiki/src/store.rs` | 修改 | WikiStore 新增 metrics 字段 + Clone + insert/delete 刷新 |
| `crates/repo-wiki/tests/metrics_test.rs` | 新增 | 4 个 TDD 集成测试 |

### 2.4 验证结果
- `cargo test -p repo-wiki`:113 passed / 0 failed
- `cargo clippy -p repo-wiki`:零警告
- `cargo fmt -p repo-wiki`:零 diff
- 向后兼容:open/open_with_config/insert/delete/count 签名不变

---

## 3. P1: SqliteHistoryStore 持久化实现

### 3.1 目标
为 M2 RL 路由的触发条件(历史数据 > 10000 条)解除阻塞。实现 HistoryStore trait 的
SQLite 持久化,使历史数据跨进程重启保留。

### 3.2 设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| trait 同步性 | 保持同步 | v1.3.0 已发布 API,改 async 破坏对象安全性 + MoeGate::gate() 签名 |
| UPSERT 策略 | SELECT-merge-INSERT OR REPLACE | VecDeque 滑动窗口合并(pop_front+push_back)无法用 SQL 表达 |
| 序列化 | MessagePack(rmp-serde) | ADR-004 一致选型,比 JSON 紧凑(500B vs 1200B) |
| Connection | Mutex<Connection> | 与 repo-wiki 模式一致,写操作互斥 |
| WAL 模式 | 启用 | 崩溃恢复友好,Drop → 重新打开数据不丢失 |
| 默认实现 | Memory(opt-in SQLite) | 向后兼容 v1.3.0,SQLite 需显式配置 |

### 3.3 文件变更

| 文件 | 变更 | 说明 |
|------|------|------|
| `crates/model-router/Cargo.toml` | 修改 | 添加 `rusqlite` + `rmp-serde` workspace 依赖 |
| `crates/model-router/src/history/mod.rs` | 新增 | HistoryStore trait + 常量(从 moe.rs 迁移) |
| `crates/model-router/src/history/memory.rs` | 新增 | InMemoryHistoryStore(从 moe.rs 迁移) |
| `crates/model-router/src/history/sqlite.rs` | 新增 | SqliteHistoryStore(Mutex<Connection> + WAL + MessagePack + UPSERT) |
| `crates/model-router/src/moe.rs` | 修改 | `pub use` 保持路径可见性 |
| `crates/model-router/src/config.rs` | 修改 | HistoryPersistence enum + RouterConfig 字段 |
| `crates/model-router/src/lib.rs` | 修改 | 导出 history 模块 + HistoryPersistence |
| `crates/model-router/src/error.rs` | 修改 | SqliteHistoryError 变体 |
| `crates/model-router/tests/history_test.rs` | 新增 | 9 个 TDD 测试 |
| `crates/model-router/benches/moe_bench.rs` | 修改 | 新增 memory vs sqlite 延迟对比 bench |

### 3.4 bench 数据(memory vs sqlite)

| 操作 | memory | sqlite | 比率 |
|------|--------|--------|------|
| record | 70.9 ns | 98.0 µs | sqlite 慢 1380x |
| get | 25.3 ns | 38.7 µs | sqlite 慢 1530x |

SQLite 操作均在微秒级,在路由热路径(单次决策)上可接受。

### 3.5 验证结果
- `cargo test -p model-router`:143 passed / 0 failed
- `cargo clippy -p model-router`:零警告
- `cargo fmt -p model-router`:零 diff
- 向后兼容:HistoryStore trait 签名不变;MoeGate::gate() 签名不变;默认 Memory

### 3.6 project_memory 教训沉淀
- 原则 14:同步 trait 的 SQLite 实现(trait 保持同步,调用方负责 async 包装)
- 原则 15:MessagePack 序列化 VecDeque(ADR-004 一致选型,反序列化失败降级保留标量字段)
- 原则 16:SQLite UPSERT 原子合并(SELECT-merge-INSERT OR REPLACE + Mutex 串行)

---

## 4. P2/P3/P4: 条件触发评估

### 4.1 P2 — M1 向量索引升级
- **触发条件**:Wiki entries > 1000 且 KNN p95 > 10ms
- **当前状态**:entries < 100,KNN p95 < 1ms(10x 余量)
- **结论**:继续延后,下次评估 2026-10(每季度)
- **候选方案**:qdrant > milvus > sqlite-vec(unsafe 排除)

### 4.2 P3 — M2 RL 路由策略
- **触发条件**:历史路由数据 > 10000 条 且 静态权重次优率 > 5%
- **当前状态**:P1 持久化已就绪,历史数据需生产环境积累
- **结论**:继续延后,待历史数据积累后重新评估
- **候选方案**:Bandit > 在线梯度 > 离线训练

### 4.3 P4 — M3 配置热重载
- **触发条件**:用户明确请求运行时配置变更
- **当前状态**:无 daemon 模式,无用户请求
- **结论**:继续延后,待用户请求或 daemon 模式引入时重新评估
- **候选方案**:notify crate(跨平台 + 无 unsafe)

---

## 5. 全量验证

| 验证项 | 结果 |
|--------|------|
| `cargo check --workspace` | ✅ 通过 |
| `cargo test --workspace --jobs 1` | ✅ 全部通过(0 failed) |
| `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` | ✅ 零警告(50.08s) |
| `cargo fmt --all -- --check` | ✅ 零 diff |

---

## 6. 关键设计决策汇总

1. **Gauge 而非 Counter**(P0):entries 可因 delete 减少,Gauge 支持可增可减
2. **i64 而非 f64**(P0):条目数是整数,默认泛型参数更简洁
3. **Arc<WikiMetrics> 共享**(P0):WikiStore::clone 共享写线程,指标也需共享
4. **trait 保持同步**(P1):v1.3.0 已发布 API,改 async 破坏对象安全性
5. **SELECT-merge-INSERT OR REPLACE**(P1):VecDeque 滑动窗口合并无法用 SQL 表达
6. **MessagePack 序列化**(P1):ADR-004 一致选型,比 JSON 紧凑
7. **默认 Memory,opt-in SQLite**(P1):向后兼容 v1.3.0
8. **YAGNI 原则**(P2/P3/P4):触发条件未满足前不实施,仅做评估

---

## 7. 关联文档

- P0 报告:`docs/optimization/v1.4.0/p0_metrics_report.md`
- P1 报告:`docs/optimization/v1.4.0/p1_sqlite_history_report.md`
- M1 评估:`docs/optimization/v1.4.0/m1_vector_index_assessment.md`
- M2 评估:`docs/optimization/v1.4.0/m2_rl_routing_assessment.md`
- M3 评估:`docs/optimization/v1.4.0/m3_config_hot_reload_assessment.md`
- v1.3.0 综合报告:`docs/optimization/v1.3.0/full_post_optimization_report.md`
- spec 路径:`.trae/specs/v1-4-0-omega-p2-implementation/`
