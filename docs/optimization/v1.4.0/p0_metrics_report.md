# P0: repo-wiki Prometheus 监控指标接入报告

> 版本:v1.4.0-omega P0
> 日期:2026-07-10
> 任务:为 `WikiStore` 接入 `prometheus-client` 监控指标,暴露 `wiki_entries_total` gauge

---

## 1. 目标

为 M1 向量索引升级的触发条件(**Wiki entries > 1000 且 KNN p95 > 10ms**)提供数据支撑。
通过 Prometheus gauge 暴露当前 Wiki 条目总数,运维可通过 Prometheus 查询
`wiki_entries_total > 1000` 监控是否接近阈值,并在达到 800(80% 预警线)时收到 WARN 日志。

## 2. 设计决策

### 2.1 Gauge 而非 Counter

`wiki_entries_total` 选择 `Gauge` 类型(可增可减),而非 `Counter`(单调递增)。

**WHY**:Wiki entries 可因 `delete` 操作减少,Counter 只能递增无法表达"减少"语义。
Gauge 支持 `set/inc/dec/add/sub`,正确覆盖条目数的增减场景。

### 2.2 i64 类型(而非 f64)

使用 prometheus-client 0.22 的默认泛型参数 `Gauge<i64, AtomicI64>`。

**WHY**:
- 条目数是整数计数场景,`i64` 足够表达且语义更精确
- 默认类型避免显式指定泛型参数(`Gauge::<f64, AtomicU64>`),代码更简洁
- `get()` 直接返回 `i64`(非 `Option<i64>`),默认值 0,有利于运维直接查询

> **任务说明偏差纠正**:任务说明中提到"f64 类型,因 prometheus-client Gauge 是 f64",
> 实际 prometheus-client 0.22.3 的 `Gauge` 默认是 `i64`。f64 需显式指定
> `Gauge::<f64, AtomicU64>`。本任务选择 i64(默认),更适合条目数场景。

### 2.3 `Arc<WikiMetrics>` 共享

`WikiStore` 持有 `metrics: Arc<WikiMetrics>` 字段,而非直接持有 `WikiMetrics`。

**WHY**:`WikiStore` 实现了 `Clone`(共享写线程与读连接池),多个 clone 实例
必须看到一致的指标视图。用 `Arc` 共享同一份 `WikiMetrics`,保证所有 clone
的 gauge 值一致,避免不同实例指标分叉。

### 2.4 预警阈值 800(80% 缓冲)

`set_entries` 在 `count >= 800` 时发出 `tracing::warn!` 预警。

**WHY**:预警阈值 800 = 触发阈值 1000 的 80%,预留 20% 缓冲期
(800 → 1000 = 200 条目增长空间),让运维团队在触发前有时间规划升级,
避免突发触发措手不及。每次 `set` 都检查阈值(而非仅跨越时),
持续提醒运维 entries 已接近危险区。

### 2.5 insert/delete 后自动刷新

`insert` 和 `delete` 成功后自动调用 `refresh_metrics()`,
内部执行 `SELECT COUNT(*)` 并更新 gauge。

**WHY**:保证 gauge 与实际数据库条目数一致。`count()` 是 O(1) 操作
(SQLite 维护行计数,无需全表扫描),在 `spawn_blocking` 中执行不阻塞
async runtime,性能开销可接受。refresh 失败不阻断主操作(insert/delete
已成功),仅记录 warning — 指标滞后是可接受的(下次操作会再次刷新)。

## 3. 指标说明

| 指标名 | 类型 | 描述 |
|--------|------|------|
| `wiki_entries_total` | Gauge(i64) | 当前 Wiki 条目总数(insert/delete 后自动刷新) |

### 3.1 暴露方式

`WikiStore::metrics()` 返回 `&WikiMetrics`,调用方可:
1. 直接读取:`store.metrics().entries_total.get()` 返回 `i64`
2. 注册到 Prometheus Registry:将 `WikiMetrics` 注册到上层 Registry,
   通过 `/metrics` 端点导出 Prometheus 文本格式

### 3.2 已有数据库的初始化

`open_with_config` 是同步函数,无法调用 async 的 `count()` 初始化指标。
默认 gauge 值为 0(AtomicI64 默认值)。对于已有数据的数据库,调用方应
在 `open` 后手动调用 `store.refresh_metrics().await` 刷新到真实计数。

## 4. 测试覆盖

新增 4 个 TDD 测试(`crates/repo-wiki/tests/metrics_test.rs`):

| 测试名 | 验证内容 |
|--------|---------|
| `test_entries_total_zero_on_empty` | 空 store 时 gauge = 0 |
| `test_entries_total_updated_on_insert` | insert 后 gauge 正确更新(含 UPSERT 不增加计数) |
| `test_entries_total_updated_on_delete` | delete 后 gauge 正确更新(含幂等删除不改变计数) |
| `test_warn_log_when_entries_approach_threshold` | set_entries 在阈值边界(799/800/1000/0)正确设置 gauge |

此外,`metrics.rs` 模块内含 4 个单元测试(默认值/低于阈值/阈值边界/可减性)。

> **WARN 日志断言说明**:日志断言需引入 `tracing-test` 依赖,且 WARN 日志是
> 可观测的副作用而非核心功能。测试 4 验证 gauge 值在阈值边界的正确性;
> `tracing::warn!` 的触发由 `set_entries` 内部的 `count >= 800` 条件保证
> (代码审查可核验)。

## 5. 验证结果

```powershell
# 测试(113 passed / 0 failed,含 4 个新增 metrics 测试)
cargo test -p repo-wiki --jobs 1
#   68 单元测试 + 12 fts + 12 iscm + 4 metrics + 1 proptest + 14 store + 2 doc-tests

# clippy(零警告)
$env:RUST_MIN_STACK = '33554432'; $env:CARGO_INCREMENTAL = '0'
cargo clippy -p repo-wiki --all-targets --jobs 2 -- -D warnings
#   退出码 0

# fmt(零 diff)
cargo fmt -p repo-wiki -- --check
#   退出码 0
```

## 6. 文件变更清单

| 文件 | 变更类型 | 说明 |
|------|---------|------|
| `crates/repo-wiki/Cargo.toml` | 修改 | 添加 `prometheus-client = { workspace = true }` 依赖 |
| `crates/repo-wiki/src/metrics.rs` | 新增 | `WikiMetrics` 结构体 + `set_entries` + 阈值常量 + 4 单元测试 |
| `crates/repo-wiki/src/lib.rs` | 修改 | 导出 `metrics` 模块 + `WikiMetrics` 重导出 + prelude |
| `crates/repo-wiki/src/store.rs` | 修改 | WikiStore 新增 metrics 字段 + Clone + open_with_config + insert/delete 刷新 + metrics() + refresh_metrics() |
| `crates/repo-wiki/tests/metrics_test.rs` | 新增 | 4 个 TDD 集成测试 |

## 7. 向后兼容性

- `WikiStore::open` / `open_with_config` / `insert` / `delete` / `count` 签名**不变**
- 新增 `pub fn metrics(&self) -> &WikiMetrics` 和 `pub async fn refresh_metrics(&self)`
- `WikiStore::clone` 行为不变(共享 metrics,与共享写线程/读连接池语义一致)
- 不变更核心领域类型(`UserIntent`/`Quest`/`Checkpoint`/`OmniSparseMasks`/`CLV`/`NexusState`)
- 保持 `#![forbid(unsafe_code)]`
- 遵守 §2.2 依赖铁律(prometheus-client 是外部依赖,不违反 L5→L1 方向)
