# v1.6.0-omega 分布式深度优化与创新演进 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 完成 Chimera CLI v1.6.0-omega 全部 33 个 Task，覆盖编译基线验证、P0/P1 修复、YAGNI 重新评估、OMEGA 创新深化、性能微优化、文档对齐与全量交付。

**Architecture:** 以 Wave 为单位按优先级推进，每个 Wave 内部子任务可并行执行；采用 TDD 模式，每个代码变更先写测试/再实现/再验证；由 5 名专家子代理分别负责架构、开发、测试、产品、长期主义审计。

**Tech Stack:** Rust 2021, Tokio, rusqlite, r2d2, DashMap/RwLock, criterion, proptest, prometheus-client

## Global Constraints

- 所有 crate 顶层必须保持 `#![forbid(unsafe_code)]`
- 禁止向上依赖：`L(N) → L(N+1)` 禁止
- `rusqlite` 调用必须包装在 `spawn_blocking` 中
- 禁止持锁跨 `.await`
- `PRAGMA journal_mode=WAL` 必须用 `pragma_update`
- 风险规则列表为空时返回 `RiskLevel::Unknown`
- u64 大数百分比计算用 `f64` 中间值
- 多治理器协同必须经 `ArbitrationLayer` 取保守值
- 降级/耗尽路径必须发布对应 `NexusEvent`
- 每个新增功能必须有 TDD 测试或 bench 支撑
- 每次修改后必须运行 `cargo check/test/clippy/fmt`

---

## File Structure Overview

### 创建的新文件

- `crates/repo-wiki/src/pool.rs` — r2d2 连接池封装
- `crates/event-bus/src/priority.rs` — 四级事件优先级队列
- `crates/event-bus/src/adr-011-event-priority.md` — ADR 草稿（后续合并到 docs/adr）
- `crates/faae-router/src/edsb.rs` 内的次优选择逻辑增强
- `crates/quest-engine/src/dag.rs` — DAG 分解器
- `crates/quest-engine/src/adr-012-speculative-dag.md` — ADR 草稿
- `crates/nexus-core/src/clv_layer.rs` — CLV 分层压缩类型
- `crates/nexus-core/src/adr-013-clv-layering.md` — ADR 草稿
- `crates/gsoe-evolution/src/grpo.rs` — GRPO 评分模块
- `crates/model-router/src/cacr_asymmetric.rs` — 非对称预算控制
- `crates/seccore/src/invariants.rs` — 主动安全不变量
- `crates/repo-wiki/src/forgetting.rs` — 遗忘曲线与重要性衰减
- `crates/osa-coordinator/src/heuristic.rs` — 真实 heuristic 评分
- `crates/event-bus/src/codec.rs` — 双格式序列化自动选择
- `crates/event-bus/src/metrics.rs` — Prometheus 指标
- `crates/efficiency-monitor/src/metrics.rs` — Prometheus 指标
- `docs/adr/ADR-011-event-priority.md`
- `docs/adr/ADR-012-speculative-dag.md`
- `docs/adr/ADR-013-clv-layering.md`

### 修改的现有文件

- `crates/repo-wiki/src/store.rs` — WikiStore 读写分离
- `crates/repo-wiki/src/vector.rs` — VectorIndex Mutex→RwLock（已完成，需验证）
- `crates/cmt-tiering/src/coordinator.rs` / `warm.rs` / `cold.rs` — SQLite 连接池
- `crates/scc-cache/src/lib.rs` 或 `wal.rs` — SQLite 连接池
- `crates/event-bus/src/types.rs` — 新增 EventPriority
- `crates/event-bus/src/lib.rs` — 优先级调度
- `crates/osa-coordinator/src/coordinator.rs` — 路由管道顺序保证 + heuristic_scores
- `crates/kvbsr-router/src/router.rs` — 路由管道集成
- `crates/faae-router/src/edsb.rs` — 路由管道集成 + 次优选择
- `crates/sesa-router/src/lib.rs` — 路由管道集成
- `crates/gea-activator/src/lib.rs` — 路由管道集成
- `crates/seccore/src/audit.rs` — AuditChain 并发化
- `crates/seccore/src/lib.rs` — 安全不变量入口
- `crates/model-router/src/registry.rs` — DashMap→RwLock（已完成，需验证）
- `crates/model-router/src/cacr.rs` — 非对称预算控制
- `crates/model-router/src/history/*.rs` — NexusState Arc 共享评估
- `crates/quest-engine/src/semantic_dag.rs` — DAG 分解
- `crates/quest-engine/src/lib.rs` — DAG 执行入口
- `crates/nexus-core/src/clv.rs` — 分层压缩
- `crates/gsoe-evolution/src/engine.rs` — GRPO 评分集成
- `crates/gsoe-evolution/src/types.rs` — 评分类型扩展
- `crates/mlc-engine/src/*.rs` — clone 减少
- `crates/cmt-tiering/src/*.rs` — clone 减少
- `crates/scc-cache/src/*.rs` — clone 减少
- `Cargo.toml` — 版本号同步为 `1.6.0-omega`
- `CHANGELOG.md` — v1.6.0-omega 章节
- `CODE_WIKI.md` — crate 索引与 ADR
- `project_memory.md` — 原则 23+

---

## Wave 1: 全量基线验证

### Task 6: 全量编译与测试基线验证

**Files:**
- 读取：`Cargo.toml`, `CHANGELOG.md`, `CODE_WIKI.md`
- 验证：全部 35 crate

**Interfaces:**
- 输入：Phase I Task 1-5 已完成
- 输出：基线测试数量、已知限制清单

- [ ] **Step 1: 运行完整编译检查**

Run:
```bash
export CARGO_HOME='D:/Chimera CLI/.toolchain/cargo'
export RUSTUP_HOME='D:/Chimera CLI/.toolchain/rustup'
export TMP='D:/Chimera CLI/tmp'
export TEMP='D:/Chimera CLI/tmp'
export CARGO_INCREMENTAL='0'
cd "D:/Chimera CLI"
"D:/Chimera CLI/.toolchain/cargo/bin/cargo.exe" check --workspace
```
Expected: `Finished` with exit code 0

- [ ] **Step 2: 运行完整测试套件**

Run:
```bash
"D:/Chimera CLI/.toolchain/cargo/bin/cargo.exe" test --workspace --jobs 1
```
Expected: all test result: ok, 0 failed, EXIT_CODE=0

- [ ] **Step 3: 运行 clippy**

Run:
```bash
"D:/Chimera CLI/.toolchain/cargo/bin/cargo.exe" clippy --workspace --all-targets --jobs 2 -- -D warnings
```
Expected: exit code 0, 0 warnings

- [ ] **Step 4: 运行 fmt 检查**

Run:
```bash
"D:/Chimera CLI/.toolchain/cargo/bin/cargo.exe" fmt --all -- --check
```
Expected: exit code 0, no diff

- [ ] **Step 5: 记录基线数据**

更新 `.trae/specs/v1-6-0-omega-comprehensive-deep-optimization/checklist.md`：
- 勾选 Checkpoint 1-18
- 在 Checkpoint 18 旁记录实际测试数量

- [ ] **Step 6: 提交**

```bash
git add .trae/specs/v1-6-0-omega-comprehensive-deep-optimization/checklist.md
git commit -m "chore(v1.6.0): Wave 1 全量基线验证通过

- cargo check --workspace 退出码 0
- cargo test --workspace --jobs 1 全部通过
- cargo clippy 零警告
- cargo fmt 零 diff

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Wave 2: Phase II P0/P1 修复

### Task 9: SQLite 读写分离连接池（cmt-tiering + scc-cache）

**Files:**
- Create: `crates/cmt-tiering/src/pool.rs`
- Create: `crates/scc-cache/src/pool.rs`
- Modify: `crates/cmt-tiering/Cargo.toml`
- Modify: `crates/scc-cache/Cargo.toml`
- Modify: `crates/cmt-tiering/src/coordinator.rs`
- Modify: `crates/cmt-tiering/src/warm.rs`
- Modify: `crates/cmt-tiering/src/cold.rs`
- Modify: `crates/scc-cache/src/lib.rs`（或实际持有 Connection 的文件）
- Test: `crates/cmt-tiering/tests/pool.rs`
- Test: `crates/scc-cache/tests/pool.rs`
- Bench: `crates/cmt-tiering/benches/pool_bench.rs`
- Bench: `crates/scc-cache/benches/pool_bench.rs`

**Interfaces:**
- 输入：`Arc<Mutex<Connection>>` 的单连接模式
- 输出：`Pool<ConnectionManager<SqliteConnection>>` 封装类型

- [ ] **Step 1: 添加 r2d2 依赖**

在 `crates/cmt-tiering/Cargo.toml` 和 `crates/scc-cache/Cargo.toml` 中添加：
```toml
[dependencies]
r2d2 = "0.8"
r2d2_sqlite = "0.24"
```
（版本以 `cargo check` 实际解析为准，若 workspace 已统一版本则使用 workspace 版本）

- [ ] **Step 2: 创建连接池封装模块**

`crates/cmt-tiering/src/pool.rs`:
```rust
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use std::path::Path;

pub type SqlitePool = Pool<SqliteConnectionManager>;

/// 创建 SQLite 连接池，复数读操作可并发从池中获取连接。
pub fn create_pool<P: AsRef<Path>>(path: P) -> Result<SqlitePool, r2d2::Error> {
    let manager = SqliteConnectionManager::file(path);
    Pool::builder().max_size(8).build(manager)
}

/// 获取一个池化连接，用于 spawn_blocking 中的 rusqlite 调用。
pub fn get_conn(pool: &SqlitePool) -> Result<PooledConnection<SqliteConnectionManager>, r2d2::Error> {
    pool.get()
}
```

`scc-cache` 的 `pool.rs` 与上相同（可跨 crate 复制，保持独立以避免上层依赖）。

- [ ] **Step 3: 替换 cmt-tiering 中的单连接**

读取 `crates/cmt-tiering/src/coordinator.rs`、`warm.rs`、`cold.rs`，找到所有 `Arc<Mutex<Connection>>` 字段，替换为 `SqlitePool`。

示例模式（具体字段名以实际代码为准）：
```rust
// 修改前
pub struct WarmStore {
    conn: Arc<Mutex<Connection>>,
}

// 修改后
pub struct WarmStore {
    pool: SqlitePool,
}
```

所有读写操作改为：
```rust
let pool = self.pool.clone();
tokio::task::spawn_blocking(move || {
    let mut conn = pool.get()?;
    // 原有 rusqlite 调用
    conn.execute("...", params![])?;
    Ok::<_, rusqlite::Error>(())
})
.await??;
```

- [ ] **Step 4: 替换 scc-cache 中的单连接**

与 Step 3 相同模式，应用于 `crates/scc-cache/src/lib.rs` 中持有 Connection 的结构。

- [ ] **Step 5: 编写并发读测试**

`crates/cmt-tiering/tests/pool.rs`:
```rust
use cmt_tiering::pool::create_pool;
use std::sync::Arc;
use tokio::task::JoinSet;

#[tokio::test]
async fn concurrent_reads_do_not_block() {
    let temp = tempfile::tempdir().unwrap();
    let pool = Arc::new(create_pool(temp.path().join("test.db")).unwrap());

    // 初始化表
    {
        let p = pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = p.get().unwrap();
            conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)", []).unwrap();
        }).await.unwrap();
    }

    let mut set = JoinSet::new();
    for _ in 0..8 {
        let p = pool.clone();
        set.spawn(async move {
            tokio::task::spawn_blocking(move || {
                let conn = p.get().unwrap();
                conn.query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i32>(0))
                    .unwrap()
            }).await.unwrap();
        });
    }

    while set.join_next().await.is_some() {}
}
```

`scc-cache` 测试类似。

- [ ] **Step 6: 编写 bench**

`crates/cmt-tiering/benches/pool_bench.rs`:
```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use cmt_tiering::pool::create_pool;
use std::sync::Arc;

fn bench_concurrent_reads(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let pool = Arc::new(create_pool(temp.path().join("bench.db")).unwrap());

    c.bench_function("concurrent_reads", |b| {
        b.to_async(&rt).iter(|| async {
            let mut set = tokio::task::JoinSet::new();
            for _ in 0..8 {
                let p = pool.clone();
                set.spawn(async move {
                    tokio::task::spawn_blocking(move || {
                        let conn = p.get().unwrap();
                        let _: i32 = conn
                            .query_row("SELECT 1", [], |r| r.get(0))
                            .unwrap();
                    }).await.unwrap();
                });
            }
            while set.join_next().await.is_some() {}
            black_box(());
        });
    });
}

criterion_group!(benches, bench_concurrent_reads);
criterion_main!(benches);
```

- [ ] **Step 7: 验证**

Run:
```bash
cargo test -p cmt-tiering -p scc-cache
cargo clippy -p cmt-tiering -p scc-cache --all-targets -- -D warnings
cargo fmt -p cmt-tiering -p scc-cache -- --check
```
Expected: all pass

- [ ] **Step 8: 提交**

```bash
git add crates/cmt-tiering crates/scc-cache
git commit -m "feat(v1.6.0): SQLite 连接池读写分离（Task 9）

- cmt-tiering 与 scc-cache 使用 r2d2 连接池
- 读写操作通过 spawn_blocking 获取池化连接
- 并发读测试与 bench 验证吞吐量提升

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 10: 优先级残差事件流

**Files:**
- Create: `crates/event-bus/src/priority.rs`
- Modify: `crates/event-bus/src/types.rs`
- Modify: `crates/event-bus/src/lib.rs`（或实际 event bus 实现文件）
- Create: `crates/event-bus/tests/priority.rs`
- Create: `docs/adr/ADR-011-event-priority.md`

**Interfaces:**
- 输入：`NexusEvent` + `EventPriority::Normal/Critical`
- 输出：`NexusEvent` + `EventPriority::Normal/Warning/Critical/Priority`，Priority 优先投递

- [ ] **Step 1: 扩展 EventPriority 枚举**

`crates/event-bus/src/types.rs`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventPriority {
    Normal = 0,
    Warning = 1,
    Critical = 2,
    Priority = 3,
}

impl Default for EventPriority {
    fn default() -> Self { EventPriority::Normal }
}
```

- [ ] **Step 2: 在 NexusEvent 上附加优先级**

如果 `NexusEvent` 已有 priority 字段，修改类型为 `EventPriority`；否则新增字段：

```rust
pub struct NexusEvent {
    pub kind: EventKind,
    pub payload: EventPayload,
    pub priority: EventPriority,
    pub timestamp: u64,
}
```

- [ ] **Step 3: 创建优先级队列**

`crates/event-bus/src/priority.rs`:
```rust
use std::collections::BinaryHeap;
use std::cmp::Reverse;
use std::sync::Mutex;

#[derive(Debug)]
struct QueuedEvent {
    priority: EventPriority,
    seq: u64,
    event: NexusEvent,
}

impl PartialEq for QueuedEvent {
    fn eq(&self, other: &Self) -> bool { self.seq == other.seq }
}
impl Eq for QueuedEvent {}
impl PartialOrd for QueuedEvent {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        // 优先级高的先出队；同优先级按 seq FIFO
        Some((self.priority, Reverse(self.seq)).cmp(&(other.priority, Reverse(other.seq))))
    }
}
impl Ord for QueuedEvent {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap()
    }
}

pub struct PriorityEventQueue {
    heap: Mutex<BinaryHeap<QueuedEvent>>,
    seq: Mutex<u64>,
}

impl PriorityEventQueue {
    pub fn new() -> Self {
        Self { heap: Mutex::new(BinaryHeap::new()), seq: Mutex::new(0) }
    }

    pub fn push(&self, event: NexusEvent) {
        let seq = {
            let mut s = self.seq.lock().unwrap();
            *s += 1;
            *s
        };
        let mut heap = self.heap.lock().unwrap();
        heap.push(QueuedEvent { priority: event.priority, seq, event });
    }

    pub fn pop(&self) -> Option<NexusEvent> {
        let mut heap = self.heap.lock().unwrap();
        heap.pop().map(|q| q.event)
    }
}
```

- [ ] **Step 4: 升级关键事件为 Priority 级**

在事件发布点，将 `SkepticVeto` / `RedTeamAudit` / `AsaIntervention` / `BudgetExceeded` 的优先级设为 `EventPriority::Priority`。

示例：
```rust
NexusEvent {
    kind: EventKind::SkepticVeto,
    payload: EventPayload::VetoReason(reason),
    priority: EventPriority::Priority,
    timestamp: now(),
}
```

- [ ] **Step 5: 事件总线集成优先级队列**

根据实际 event-bus 实现，将内部通道或缓冲替换为 `PriorityEventQueue`，或在消费端按优先级排序。

- [ ] **Step 6: 编写优先级测试**

`crates/event-bus/tests/priority.rs`:
```rust
use event_bus::{EventPriority, NexusEvent, PriorityEventQueue};

#[test]
fn priority_events_dequeue_first() {
    let q = PriorityEventQueue::new();
    let normal = NexusEvent { kind: EventKind::Normal, priority: EventPriority::Normal, /* ... */ };
    let priority = NexusEvent { kind: EventKind::SkepticVeto, priority: EventPriority::Priority, /* ... */ };
    q.push(normal);
    q.push(priority);
    let first = q.pop().unwrap();
    assert_eq!(first.priority, EventPriority::Priority);
}
```

- [ ] **Step 7: 编写 ADR-011**

`docs/adr/ADR-011-event-priority.md`:
```markdown
# ADR-011: 四级事件优先级与 Priority 优先投递

## Status
Accepted

## Context
v1.5.0 事件总线只有 Normal/Critical 二级，无法满足 SkepticVeto 等关键事件的优先调度需求。

## Decision
引入 Normal/Warning/Critical/Priority 四级优先级，Priority 最高，Critical 仍保证投递但不插队。

## Consequences
- 关键治理事件响应更快
- 需要维护事件 seq 保证同优先级 FIFO
- 向后兼容：未指定优先级的默认为 Normal
```

- [ ] **Step 8: 验证**

Run:
```bash
cargo test -p event-bus
cargo clippy -p event-bus --all-targets -- -D warnings
cargo fmt -p event-bus -- --check
```

- [ ] **Step 9: 提交**

```bash
git add crates/event-bus docs/adr/ADR-011-event-priority.md
git commit -m "feat(v1.6.0): 四级事件优先级与 Priority 优先投递（Task 10）

- EventPriority 扩展为 Normal/Warning/Critical/Priority
- PriorityEventQueue 保证高优先级事件先出队
- SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded 升级为 Priority
- ADR-011 记录设计决策

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 11: L6 路由链路顺序保证

**Files:**
- Create: `crates/osa-coordinator/src/pipeline.rs`
- Modify: `crates/osa-coordinator/src/coordinator.rs`
- Modify: `crates/kvbsr-router/src/router.rs`
- Modify: `crates/faae-router/src/edsb.rs`
- Modify: `crates/sesa-router/src/lib.rs`
- Modify: `crates/gea-activator/src/lib.rs`
- Create: `crates/osa-coordinator/tests/pipeline.rs`

**Interfaces:**
- 输入：用户请求 + CLV
- 输出：依次经过 OSA → KVBSR → FaaE → SESA → GEA 的路由结果

- [ ] **Step 1: 定义 RoutingPipeline 结构**

`crates/osa-coordinator/src/pipeline.rs`:
```rust
use nexus_core::CLV;
use event_bus::NexusEvent;

/// L6 路由链路顺序保证。
///
/// 按固定顺序调用：OSA → KVBSR → FaaE → SESA → GEA。
/// 每一层返回下一层需要的路由状态，若某层决定终止则提前返回。
pub struct RoutingPipeline;

impl RoutingPipeline {
    pub async fn route(
        &self,
        intent: UserIntent,
        clv: CLV,
        ctx: &mut RoutingContext,
    ) -> Result<RoutingOutcome, RoutingError> {
        let osa = ctx.osa.route(&intent, &clv).await?;
        let kvbsr = ctx.kvbsr.route(&osa).await?;
        let faae = ctx.faae.route(&kvbsr).await?;
        let sesa = ctx.sesa.route(&faae).await?;
        let gea = ctx.gea.route(&sesa).await?;
        Ok(RoutingOutcome { gea })
    }
}
```

- [ ] **Step 2: 改造各路由层接口**

根据实际代码，将 OSA/KVBSR/FaaE/SESA/GEA 调整为统一接口。例如：

```rust
// kvbsr-router/src/router.rs
pub async fn route(&self, osa_result: &OsaResult) -> Result<KvbsrResult, RouterError>;
```

- [ ] **Step 3: 在 coordinator 中调用 pipeline**

`crates/osa-coordinator/src/coordinator.rs`:
```rust
let outcome = self.pipeline.route(intent, clv, &mut ctx).await?;
```

- [ ] **Step 4: 编写顺序测试**

`crates/osa-coordinator/tests/pipeline.rs`:
```rust
#[tokio::test]
async fn route_pipeline_order_is_enforced() {
    let mut recorder = LayerRecorder::new();
    let pipeline = RoutingPipeline::new(recorder.layers());
    let _ = pipeline.route(UserIntent::default(), CLV::default(), &mut ctx).await;
    assert_eq!(recorder.order(), vec!["OSA", "KVBSR", "FaaE", "SESA", "GEA"]);
}
```

- [ ] **Step 5: 验证**

Run:
```bash
cargo test -p osa-coordinator -p kvbsr-router -p faae-router -p sesa-router -p gea-activator
cargo clippy -p osa-coordinator --all-targets -- -D warnings
```

- [ ] **Step 6: 提交**

```bash
git add crates/osa-coordinator crates/kvbsr-router crates/faae-router crates/sesa-router crates/gea-activator
git commit -m "feat(v1.6.0): L6 路由链路代码级顺序保证（Task 11）

- 引入 RoutingPipeline 强制 OSA→KVBSR→FaaE→SESA→GEA 顺序
- 各路由层接口对齐
- 集成测试验证顺序不依赖事件到达顺序

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 12: AuditChain 并发化

**Files:**
- Modify: `crates/seccore/src/audit.rs`
- Create: `crates/seccore/tests/audit_concurrent.rs`

**Interfaces:**
- 输入：`AuditEntry`
- 输出：并发 append 不阻塞，审计链完整性保持不变

- [ ] **Step 1: 将 Mutex<Vec> 替换为并发结构**

`crates/seccore/src/audit.rs`:
```rust
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct AuditChain {
    entries: DashMap<u64, AuditEntry>,
    next_id: AtomicU64,
}

impl AuditChain {
    pub fn new() -> Self {
        Self { entries: DashMap::new(), next_id: AtomicU64::new(0) }
    }

    pub fn append(&self, entry: AuditEntry) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.entries.insert(id, entry);
        id
    }

    pub fn get(&self, id: u64) -> Option<AuditEntry> {
        self.entries.get(&id).map(|r| r.clone())
    }

    pub fn iter(&self) -> impl Iterator<Item = (u64, AuditEntry)> + '_ {
        // DashMap 无法直接按 key 排序迭代，需要收集后排序
        let mut v: Vec<_> = self.entries.iter().map(|r| (*r.key(), r.value().clone())).collect();
        v.sort_by_key(|(id, _)| *id);
        v.into_iter()
    }
}
```

- [ ] **Step 2: 编写并发审计测试**

`crates/seccore/tests/audit_concurrent.rs`:
```rust
use seccore::audit::{AuditChain, AuditEntry};
use std::sync::Arc;
use std::thread;

#[test]
fn concurrent_appends_preserve_total_count() {
    let chain = Arc::new(AuditChain::new());
    let mut handles = vec![];
    for i in 0..8 {
        let c = chain.clone();
        handles.push(thread::spawn(move || {
            for j in 0..100 {
                c.append(AuditEntry { id: i * 100 + j, /* ... */ });
            }
        }));
    }
    for h in handles { h.join().unwrap(); }
    let count: usize = chain.iter().count();
    assert_eq!(count, 800);
}
```

- [ ] **Step 3: 验证**

Run:
```bash
cargo test -p seccore
cargo clippy -p seccore --all-targets -- -D warnings
```

- [ ] **Step 4: 提交**

```bash
git add crates/seccore
git commit -m "feat(v1.6.0): AuditChain 并发化（Task 12）

- Mutex<Vec> 替换为 DashMap<u64, AuditEntry>
- 并发 append 不阻塞，审计链完整性不变
- 并发测试验证 800 条记录无丢失

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Wave 3: Phase III YAGNI 重新评估

### Task 13: NexusState Arc 共享重新评估

**Files:**
- Create: `crates/model-router/benches/quest_clone.rs`
- Modify: `crates/model-router/src/history/mod.rs`（或实际持有 Quest 的结构）
- Modify: `crates/model-router/src/history/memory.rs`

**Interfaces:**
- 输入：`Quest` 深拷贝开销数据
- 输出：go/no-go 决策 +（若 go）`HashMap<QuestId, Arc<Quest>>`

- [ ] **Step 1: 编写 bench 测量深拷贝开销**

`crates/model-router/benches/quest_clone.rs`:
```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use model_router::history::Quest;

fn bench_quest_clone(c: &mut Criterion) {
    let quest = Quest::default(); // 或构造一个典型大 Quest
    c.bench_function("quest_clone", |b| {
        b.iter(|| black_box(quest.clone()));
    });
}

criterion_group!(benches, bench_quest_clone);
criterion_main!(benches);
```

- [ ] **Step 2: 运行 bench 并记录结果**

Run:
```bash
cargo bench -p model-router --bench quest_clone
```
记录 `quest_clone` 中位数延迟。

- [ ] **Step 3: 决策并实施**

若中位数 > 100ns 且被热路径高频调用：
- 将 `HashMap<QuestId, Quest>` 改为 `HashMap<QuestId, Arc<Quest>>`
- `get_quest()` 返回 `Arc<Quest>` 或克隆 Arc

否则记录 no-go 原因。

- [ ] **Step 4: 验证**

Run:
```bash
cargo test -p model-router
cargo clippy -p model-router --all-targets -- -D warnings
```

- [ ] **Step 5: 提交**

```bash
git add crates/model-router
git commit -m "eval(v1.6.0): NexusState Arc 共享重新评估（Task 13）

- bench 测量 Quest 深拷贝开销
- 根据阈值决策是否实施 Arc<Quest>
- 验证 model-router 测试通过

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 14: TaskProfile Hash trait 重新评估

**Files:**
- Create: `crates/model-router/benches/task_profile_hash.rs`
- Modify: `crates/model-router/src/types.rs`（或 TaskProfile 定义处）

**Interfaces:**
- 输入：`TaskProfile` serde_json 序列化开销
- 输出：go/no-go 决策 +（若 go）派生 `Hash`

- [ ] **Step 1: 编写 bench 测量 serde_json 哈希开销**

`crates/model-router/benches/task_profile_hash.rs`:
```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use model_router::types::TaskProfile;

fn bench_hash_via_json(c: &mut Criterion) {
    let profile = TaskProfile::default();
    c.bench_function("hash_via_json", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&profile)).unwrap();
            black_box(std::collections::hash_map::DefaultHasher::new())
        });
    });
}

criterion_group!(benches, bench_hash_via_json);
criterion_main!(benches);
```

- [ ] **Step 2: 运行 bench 并决策**

若 128-capacity 缓存场景下 > 1µs：
- 为 `TaskProfile` 派生 `Hash` trait
- 替换 `hash_task_profile()` 中的 serde_json 序列化

- [ ] **Step 3: 验证 hash 一致性**

Run:
```bash
cargo test -p model-router
cargo clippy -p model-router --all-targets -- -D warnings
```

- [ ] **Step 4: 提交**

```bash
git add crates/model-router
git commit -m "eval(v1.6.0): TaskProfile Hash trait 重新评估（Task 14）

- bench 测量 serde_json 哈希开销
- 根据阈值决策是否派生 Hash
- proptest 验证 hash 一致性

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 15: EDSB 次优选择策略改进

**Files:**
- Modify: `crates/faae-router/src/edsb.rs`
- Create: `crates/faae-router/tests/edsb_runner_up.rs`

**Interfaces:**
- 输入：候选列表（含相似度分数）
- 输出：非最热候选中相似度最高的项

- [ ] **Step 1: 修改次优选择函数**

`crates/faae-router/src/edsb.rs`:
```rust
/// 选择"非最热候选中相似度最高"的项。
///
/// WHY: 避免每次都选择同一个 top-1，引入多样性同时保证质量。
pub fn select_runner_up(candidates: &[(ToolId, f32)]) -> Option<ToolId> {
    if candidates.len() <= 1 {
        return candidates.first().map(|(id, _)| *id);
    }
    // 按相似度降序排序
    let mut sorted = candidates.to_vec();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    // 返回 top-1 之后的最高分
    sorted.get(1).map(|(id, _)| *id)
}
```

- [ ] **Step 2: 编写回归测试**

`crates/faae-router/tests/edsb_runner_up.rs`:
```rust
use faae_router::edsb::select_runner_up;

#[test]
fn runner_up_is_second_best() {
    let candidates = vec![
        ("tool_a".into(), 0.95),
        ("tool_b".into(), 0.93),
        ("tool_c".into(), 0.90),
    ];
    assert_eq!(select_runner_up(&candidates), Some("tool_b".into()));
}

#[test]
fn two_candidates_behaves_like_top2() {
    let candidates = vec![
        ("tool_a".into(), 0.95),
        ("tool_b".into(), 0.93),
    ];
    assert_eq!(select_runner_up(&candidates), Some("tool_b".into()));
}

#[test]
fn single_candidate_returns_itself() {
    let candidates = vec![("tool_a".into(), 0.95)];
    assert_eq!(select_runner_up(&candidates), Some("tool_a".into()));
}
```

- [ ] **Step 3: 验证**

Run:
```bash
cargo test -p faae-router
cargo clippy -p faae-router --all-targets -- -D warnings
```

- [ ] **Step 4: 提交**

```bash
git add crates/faae-router
git commit -m "feat(v1.6.0): EDSB 次优选择策略改进（Task 15）

- 次优选择改为'非最热候选中相似度最高'
- 保留候选=2和候选=1的回归行为
- 增加 WHY 注释解释多样性动机

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 16: cosine_similarity 优化重新评估

**Files:**
- Create: `crates/nexus-core/benches/cosine.rs`
- Modify: `crates/nexus-core/src/clv.rs`（或实际 cosine_similarity 实现处）

**Interfaces:**
- 输入：512-dim 向量对
- 输出：go/no-go 决策 +（若 go）优化后的 cosine_similarity_slices

- [ ] **Step 1: 编写 bench 测量当前实现**

`crates/nexus-core/benches/cosine.rs`:
```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use nexus_core::clv::cosine_similarity_slices;

fn bench_cosine_512(c: &mut Criterion) {
    let a: Vec<f32> = (0..512).map(|i| (i as f32).sin()).collect();
    let b: Vec<f32> = (0..512).map(|i| (i as f32).cos()).collect();
    c.bench_function("cosine_512", |bench| {
        bench.iter(|| black_box(cosine_similarity_slices(&a, &b)));
    });
}

criterion_group!(benches, bench_cosine_512);
criterion_main!(benches);
```

- [ ] **Step 2: 统计调用频率**

Grep 仓库中 `cosine_similarity_slices` 调用点，统计在 repo-wiki / mlc-engine / kvbsr-router 中的使用频率。

- [ ] **Step 3: 决策并实施**

若热路径累计 > ms 级且优化可提升 > 20%：
- 实施 4x 循环展开或 aligned chunks
- 保持 `#![forbid(unsafe_code)]`

- [ ] **Step 4: proptest 验证 bit-exact 一致**

`crates/nexus-core/tests/cosine_proptest.rs`:
```rust
use nexus_core::clv::cosine_similarity_slices;
use proptest::prelude::*;

proptest! {
    #[test]
    fn optimized_matches_naive(a in prop::collection::vec(-1.0f32..1.0, 512),
                               b in prop::collection::vec(-1.0f32..1.0, 512)) {
        let naive = /* 参考实现 */;
        let optimized = cosine_similarity_slices(&a, &b);
        assert!((naive - optimized).abs() < 1e-6);
    }
}
```

- [ ] **Step 5: 提交**

```bash
git add crates/nexus-core
git commit -m "eval(v1.6.0): cosine_similarity 优化重新评估（Task 16）

- bench 测量 512-dim cosine 延迟
- 统计 repo-wiki/mlc-engine/kvbsr-router 调用频率
- 根据阈值决策是否实施无 unsafe 优化
- proptest 验证 bit-exact 一致

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 17: NMC Perceptor 并行化重新评估

**Files:**
- Modify: `crates/nmc-encoder/src/lib.rs`
- Create: `crates/nmc-encoder/benches/perceptor_latency.rs`

**Interfaces:**
- 输入：各模态 perceptor 延迟
- 输出：go/no-go 决策

- [ ] **Step 1: 测量各 perceptor 延迟**

`crates/nmc-encoder/benches/perceptor_latency.rs`:
```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use nmc_encoder::{ImagePerceptor, TextPerceptor, AudioPerceptor};

fn bench_perceptors(c: &mut Criterion) {
    let img = ImagePerceptor::default();
    let text = TextPerceptor::default();
    let audio = AudioPerceptor::default();

    c.bench_function("image_perceive", |b| b.iter(|| black_box(img.perceive(&[]))));
    c.bench_function("text_perceive", |b| b.iter(|| black_box(text.perceive(""))));
    c.bench_function("audio_perceive", |b| b.iter(|| black_box(audio.perceive(&[]))));
}

criterion_group!(benches, bench_perceptors);
criterion_main!(benches);
```

- [ ] **Step 2: 决策**

若所有 perceptor 都是立即返回的占位实现，记录 no-go。
若存在真实计算，使用 `tokio::join!` 并行化。

- [ ] **Step 3: 提交**

```bash
git add crates/nmc-encoder
git commit -m "eval(v1.6.0): NMC Perceptor 并行化重新评估（Task 17）

- bench 测量 image/text/audio perceptor 延迟
- 根据数据决策是否实施 tokio::join! 并行化

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 18: gsoe spawn_blocking 重新评估

**Files:**
- Create: `crates/gsoe-evolution/benches/evaluate_population.rs`
- Modify: `crates/gsoe-evolution/src/engine.rs`

**Interfaces:**
- 输入：`evaluate_population()` 延迟
- 输出：go/no-go 决策

- [ ] **Step 1: 编写 bench 测量 evaluate_population 延迟**

`crates/gsoe-evolution/benches/evaluate_population.rs`:
```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use gsoe_evolution::engine::Engine;

fn bench_evaluate(c: &mut Criterion) {
    let engine = Engine::default();
    let population = engine.create_population(64);
    c.bench_function("evaluate_population_64", |b| {
        b.iter(|| black_box(engine.evaluate_population(&population)));
    });
}

criterion_group!(benches, bench_evaluate);
criterion_main!(benches);
```

- [ ] **Step 2: 决策并实施**

若种群规模 >= 100 或计算量 > 5µs：
- 将 `evaluate_population` 包装在 `tokio::task::spawn_blocking` 中

- [ ] **Step 3: 提交**

```bash
git add crates/gsoe-evolution
git commit -m "eval(v1.6.0): gsoe spawn_blocking 重新评估（Task 18）

- bench 测量 evaluate_population 延迟
- 根据种群规模和计算量决策是否 spawn_blocking

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Wave 4: Phase IV OMEGA 创新深化

### Task 19: Speculative DAG 执行

**Files:**
- Create: `crates/quest-engine/src/dag.rs`
- Modify: `crates/quest-engine/src/semantic_dag.rs`
- Modify: `crates/quest-engine/src/lib.rs`
- Create: `crates/quest-engine/tests/dag_speculative.rs`
- Create: `docs/adr/ADR-012-speculative-dag.md`

**Interfaces:**
- 输入：`Quest`（含可并行子任务）
- 输出：含分支的 DAG + 投机执行结果

- [ ] **Step 1: 定义 DAG 类型**

`crates/quest-engine/src/dag.rs`:
```rust
use std::collections::HashMap;

pub type NodeId = u64;

pub struct DagNode {
    pub id: NodeId,
    pub task: Task,
    pub dependencies: Vec<NodeId>,
}

pub struct TaskDag {
    pub nodes: HashMap<NodeId, DagNode>,
    pub roots: Vec<NodeId>,
}

impl TaskDag {
    /// 返回可并行执行的下一批节点（所有依赖已完成）。
    pub fn ready_nodes(&self, completed: &[NodeId]) -> Vec<NodeId> {
        let completed_set: std::collections::HashSet<_> = completed.iter().copied().collect();
        self.nodes
            .values()
            .filter(|n| {
                !completed_set.contains(&n.id)
                && n.dependencies.iter().all(|d| completed_set.contains(d))
            })
            .map(|n| n.id)
            .collect()
    }
}
```

- [ ] **Step 2: 实现激进投机执行器**

```rust
use tokio::task::JoinSet;

pub struct SpeculativeExecutor;

impl SpeculativeExecutor {
    pub async fn execute(dag: &TaskDag, runner: &dyn TaskRunner) -> Result<Vec<TaskResult>, DagError> {
        let mut completed = Vec::new();
        let mut results = HashMap::new();

        while completed.len() < dag.nodes.len() {
            let ready = dag.ready_nodes(&completed);
            if ready.is_empty() && completed.len() < dag.nodes.len() {
                return Err(DagError::CyclicDependencies);
            }

            // 激进投机：所有 ready 节点并行执行
            let mut set = JoinSet::new();
            for id in ready {
                let node = dag.nodes.get(&id).unwrap().clone();
                set.spawn(async move { (id, runner.run(&node.task).await) });
            }

            while let Some(res) = set.join_next().await {
                let (id, result) = res.map_err(|_| DagError::JoinError)?;
                completed.push(id);
                results.insert(id, result?);
            }
        }

        // 按 DAG 拓扑顺序返回结果
        Ok(topological_order(dag).into_iter().map(|id| results.remove(&id).unwrap()).collect())
    }
}
```

- [ ] **Step 3: 改造 semantic_dag 分解器**

`crates/quest-engine/src/semantic_dag.rs`:
- 将线性链改为调用 `TaskDagBuilder`
- 识别无依赖的子任务作为并行分支

- [ ] **Step 4: 编写 DAG 测试**

`crates/quest-engine/tests/dag_speculative.rs`:
```rust
use quest_engine::dag::{TaskDag, DagNode, SpeculativeExecutor};

#[tokio::test]
async fn independent_branches_execute_in_parallel() {
    let mut dag = TaskDag { nodes: HashMap::new(), roots: vec![0, 1] };
    dag.nodes.insert(0, DagNode { id: 0, task: Task::A, dependencies: vec![] });
    dag.nodes.insert(1, DagNode { id: 1, task: Task::B, dependencies: vec![] });
    dag.nodes.insert(2, DagNode { id: 2, task: Task::C, dependencies: vec![0, 1] });

    let recorder = TaskRecorder::new();
    let results = SpeculativeExecutor::execute(&dag, &recorder).await.unwrap();
    assert_eq!(results.len(), 3);
    assert!(recorder.was_parallel(0, 1));
}
```

- [ ] **Step 5: 编写 ADR-012**

`docs/adr/ADR-012-speculative-dag.md`:
```markdown
# ADR-012: Speculative DAG 执行

## Status
Accepted

## Context
quest-engine 原有线性任务链无法利用独立子任务的并行性。

## Decision
将 Quest 分解为真 DAG，采用激进投机策略：所有就绪节点并行执行，依赖节点自动串行。

## Consequences
- 显著提升可并行 Quest 的吞吐量
- 需要处理任务失败回滚
- 依赖 DAG 的正确性验证
```

- [ ] **Step 6: 验证**

Run:
```bash
cargo test -p quest-engine
cargo clippy -p quest-engine --all-targets -- -D warnings
```

- [ ] **Step 7: 提交**

```bash
git add crates/quest-engine docs/adr/ADR-012-speculative-dag.md
git commit -m "feat(v1.6.0): Speculative DAG 执行（Task 19）

- 引入 TaskDag 与 SpeculativeExecutor
- 激进投机：所有就绪节点并行执行
- 依赖节点按拓扑顺序串行
- ADR-012 记录设计决策

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 20: CLV 分层压缩

**Files:**
- Create: `crates/nexus-core/src/clv_layer.rs`
- Modify: `crates/nexus-core/src/clv.rs`
- Modify: `crates/nexus-core/src/lib.rs`
- Create: `crates/nexus-core/tests/clv_layer.rs`
- Create: `docs/adr/ADR-013-clv-layering.md`

**Interfaces:**
- 输入：`CLV`（512-dim）
- 输出：`CLVLayer` + 压缩/解压方法

- [ ] **Step 1: 定义 CLVLayer 枚举**

`crates/nexus-core/src/clv_layer.rs`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CLVLayer {
    L0, // 512-dim
    L1, // 256-dim
    L2, // 128-dim
}

impl CLVLayer {
    pub fn dim(self) -> usize {
        match self {
            CLVLayer::L0 => 512,
            CLVLayer::L1 => 256,
            CLVLayer::L2 => 128,
        }
    }
}
```

- [ ] **Step 2: 扩展 CLV 支持分层压缩**

`crates/nexus-core/src/clv.rs`:
```rust
use crate::clv_layer::CLVLayer;

impl CLV {
    /// 将当前 CLV 压缩到指定层。
    ///
    /// 当前实现采用简单前 N 维采样；后续可替换为 learned projection。
    pub fn compress(&self, target: CLVLayer) -> CompressedCLV {
        let dim = target.dim();
        CompressedCLV {
            layer: target,
            values: self.0[..dim].to_vec(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompressedCLV {
    pub layer: CLVLayer,
    pub values: Vec<f32>,
}

impl CompressedCLV {
    /// 投影回 512-dim 用于与原始 CLV 比较相似度。
    pub fn project_to_l0(&self) -> Vec<f32> {
        match self.layer {
            CLVLayer::L0 => self.values.clone(),
            CLVLayer::L1 => {
                let mut v = self.values.clone();
                v.resize(512, 0.0);
                v
            }
            CLVLayer::L2 => {
                let mut v = self.values.clone();
                v.resize(512, 0.0);
                v
            }
        }
    }
}
```

- [ ] **Step 3: 修改 cosine_similarity_slices 支持不同维度**

```rust
pub fn cosine_similarity_slices(a: &[f32], b: &[f32]) -> f32 {
    // 如果维度不同，将较短者投影到 512-dim
    let (a_proj, b_proj) = if a.len() == b.len() {
        (a.to_vec(), b.to_vec())
    } else {
        // 实际场景中应通过 CLVLayer 明确投影，此处保留兼容接口
        (project_to_512(a), project_to_512(b))
    };
    // 原有计算逻辑...
}
```

- [ ] **Step 4: 编写压缩测试**

`crates/nexus-core/tests/clv_layer.rs`:
```rust
use nexus_core::{CLV, CLVLayer};

#[test]
fn compress_to_l1_has_256_dims() {
    let clv = CLV::zeros();
    let compressed = clv.compress(CLVLayer::L1);
    assert_eq!(compressed.values.len(), 256);
}

#[test]
fn compressed_l1_similarity_to_original_is_defined() {
    let a = CLV::random();
    let b = CLV::random();
    let a_l1 = a.compress(CLVLayer::L1);
    let sim = cosine_similarity_slices(&a_l1.project_to_l0(), &b.0);
    assert!(sim.is_finite());
}
```

- [ ] **Step 5: 编写 ADR-013**

`docs/adr/ADR-013-clv-layering.md`:
```markdown
# ADR-013: CLV 分层压缩

## Status
Accepted

## Context
固定 512-dim CLV 在长上下文场景下存储与计算开销较大。

## Decision
引入 CLVLayer（L0=512 / L1=256 / L2=128），按窗口层级选择压缩级别。当前投影采用前 N 维采样，后续可替换为 learned projection。

## Consequences
- 512-dim 接口保持向后兼容
- 相似度计算需处理维度不一致
- 需要在 hcw-window / mlc-engine 中按层级选择 CLVLayer
```

- [ ] **Step 6: 验证**

Run:
```bash
cargo test -p nexus-core
cargo clippy -p nexus-core --all-targets -- -D warnings
```

- [ ] **Step 7: 提交**

```bash
git add crates/nexus-core docs/adr/ADR-013-clv-layering.md
git commit -m "feat(v1.6.0): CLV 分层压缩（Task 20）

- 新增 CLVLayer 枚举（L0/L1/L2）
- CLV 支持 compress/project_to_l0
- 512-dim 接口向后兼容
- ADR-013 记录类型变更

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 21: GRPO 自适应任务评分

**Files:**
- Create: `crates/gsoe-evolution/src/grpo.rs`
- Modify: `crates/gsoe-evolution/src/engine.rs`
- Modify: `crates/gsoe-evolution/src/types.rs`
- Create: `crates/gsoe-evolution/tests/grpo.rs`

**Interfaces:**
- 输入：种群（Population）+ 历史数据
- 输出：组内相对比较对（优势/劣势对）

- [ ] **Step 1: 定义 GRPO 评分类型**

`crates/gsoe-evolution/src/grpo.rs`:
```rust
#[derive(Debug, Clone)]
pub struct GrpoPair {
    pub winner_id: IndividualId,
    pub loser_id: IndividualId,
    pub advantage: f32,
}

pub struct GrpoScorer {
    pub group_size: usize,
}

impl GrpoScorer {
    pub fn score_population(
        &self,
        population: &[Individual],
        baseline: f32,
    ) -> Vec<GrpoPair> {
        let mut pairs = Vec::new();
        for group in population.chunks(self.group_size) {
            for (i, a) in group.iter().enumerate() {
                for b in group.iter().skip(i + 1) {
                    let delta = (a.fitness - baseline) - (b.fitness - baseline);
                    if delta.abs() > f32::EPSILON {
                        pairs.push(GrpoPair {
                            winner_id: if delta > 0.0 { a.id } else { b.id },
                            loser_id: if delta > 0.0 { b.id } else { a.id },
                            advantage: delta.abs(),
                        });
                    }
                }
            }
        }
        pairs
    }
}
```

- [ ] **Step 2: 集成到 engine**

`crates/gsoe-evolution/src/engine.rs`:
```rust
pub fn evaluate_population(&self, population: &mut Population) {
    // 原有规则式评分
    self.rule_based_score(population);

    // GRPO 组内相对比较
    let baseline = population.iter().map(|i| i.fitness).sum::<f32>() / population.len() as f32;
    let pairs = self.grpo_scorer.score_population(&population.individuals, baseline);

    // 将相对比较对附加到种群用于后续选择
    population.grpo_pairs = pairs;
}
```

- [ ] **Step 3: 编写 GRPO 测试**

`crates/gsoe-evolution/tests/grpo.rs`:
```rust
use gsoe_evolution::grpo::{GrpoScorer, Individual};

#[test]
fn grpo_produces_correct_winner_loser_pairs() {
    let individuals = vec![
        Individual { id: 0, fitness: 1.0 },
        Individual { id: 1, fitness: 0.5 },
        Individual { id: 2, fitness: 0.8 },
    ];
    let scorer = GrpoScorer { group_size: 3 };
    let pairs = scorer.score_population(&individuals, 0.7);
    assert!(pairs.iter().any(|p| p.winner_id == 0 && p.loser_id == 1));
}
```

- [ ] **Step 4: 验证**

Run:
```bash
cargo test -p gsoe-evolution
cargo clippy -p gsoe-evolution --all-targets -- -D warnings
```

- [ ] **Step 5: 提交**

```bash
git add crates/gsoe-evolution
git commit -m "feat(v1.6.0): GRPO 自适应任务评分（Task 21）

- 引入 GrpoScorer 组内相对比较
- evaluate_population 产出优势/劣势对
- 优先使用 model-router HistoryStore 历史数据（若可用）

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 22: OS-Memory Wiki 元遗忘

**Files:**
- Create: `crates/repo-wiki/src/forgetting.rs`
- Modify: `crates/repo-wiki/src/store.rs`
- Create: `crates/repo-wiki/tests/forgetting.rs`

**Interfaces:**
- 输入：Wiki 条目 + 最后访问时间 + 重要性分数
- 输出：保留概率 + 降级决策

- [ ] **Step 1: 实现遗忘曲线**

`crates/repo-wiki/src/forgetting.rs`:
```rust
use std::time::{SystemTime, UNIX_EPOCH};

/// 基于 Ebbinghaus 遗忘曲线计算保留概率。
///
/// 公式：R = e^(-t / S)，其中 t 为距最后访问的天数，S 为重要性（重要性越高 S 越大）。
pub fn retention_probability(days_since_access: f64, importance: f64) -> f64 {
    let s = importance.max(1.0);
    (-days_since_access / s).exp()
}

pub fn should_forget(
    last_accessed: u64, // unix timestamp seconds
    importance: f64,    // 0.0 - 1.0
    now: u64,
    threshold: f64,
) -> bool {
    let days = (now.saturating_sub(last_accessed)) as f64 / 86400.0;
    retention_probability(days, importance * 30.0) < threshold
}
```

- [ ] **Step 2: 在 store 中集成遗忘检查**

`crates/repo-wiki/src/store.rs`:
- 在查询/写入路径中周期性调用 `should_forget`
- 低重要性且长时间未访问的条目降级到冷存储

- [ ] **Step 3: 编写遗忘测试**

`crates/repo-wiki/tests/forgetting.rs`:
```rust
use repo_wiki::forgetting::{retention_probability, should_forget};

#[test]
fn low_importance_forgotten_after_long_time() {
    let now = 86400 * 30; // 30 天
    assert!(should_forget(0, 0.1, now, 0.5));
}

#[test]
fn high_importance_not_forgotten() {
    let now = 86400 * 30;
    assert!(!should_forget(0, 1.0, now, 0.5));
}
```

- [ ] **Step 4: 验证**

Run:
```bash
cargo test -p repo-wiki
cargo clippy -p repo-wiki --all-targets -- -D warnings
```

- [ ] **Step 5: 提交**

```bash
git add crates/repo-wiki
git commit -m "feat(v1.6.0): OS-Memory Wiki 元遗忘（Task 22）

- 基于 Ebbinghaus 遗忘曲线计算保留概率
- 低重要性长时间未访问条目降级到冷存储
- 高重要性条目不降级

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 23: CACR 非对称预算控制

**Files:**
- Create: `crates/model-router/src/cacr_asymmetric.rs`
- Modify: `crates/model-router/src/cacr.rs`
- Modify: `crates/model-router/src/lib.rs`
- Create: `crates/model-router/tests/cacr_asymmetric.rs`

**Interfaces:**
- 输入：当前预算状态 + 多维度阈值
- 输出：非对称升降阈值决策

- [ ] **Step 1: 定义非对称预算控制器**

`crates/model-router/src/cacr_asymmetric.rs`:
```rust
#[derive(Debug, Clone)]
pub struct AsymmetricBudget {
    pub token: DimensionBudget,
    pub cost: DimensionBudget,
    pub latency: DimensionBudget,
}

#[derive(Debug, Clone)]
pub struct DimensionBudget {
    pub current: f64,
    pub raise_threshold: f64, // 升阈值（宽松）
    pub lower_threshold: f64, // 降阈值（严格）
}

impl AsymmetricBudget {
    /// 判断是否需要升级模型（预算较宽松）
    pub fn should_raise(&self) -> bool {
        self.token.current >= self.token.raise_threshold
            || self.cost.current >= self.cost.raise_threshold
            || self.latency.current <= self.latency.raise_threshold
    }

    /// 判断是否需要降级模型（预算较紧张）
    pub fn should_lower(&self) -> bool {
        self.token.current < self.token.lower_threshold
            || self.cost.current < self.cost.lower_threshold
            || self.latency.current > self.latency.lower_threshold
    }
}
```

- [ ] **Step 2: 修改 CACR 入口使用非对称控制器**

`crates/model-router/src/cacr.rs`:
- 将双阈值线性判定替换为 `AsymmetricBudget`
- 保留向后兼容：旧的双阈值 API 映射到 raise/lower 相同值

- [ ] **Step 3: 编写测试**

`crates/model-router/tests/cacr_asymmetric.rs`:
```rust
use model_router::cacr_asymmetric::{AsymmetricBudget, DimensionBudget};

#[test]
fn raise_is_lenient_and_lower_is_strict() {
    let b = AsymmetricBudget {
        token: DimensionBudget { current: 0.7, raise_threshold: 0.8, lower_threshold: 0.3 },
        cost: DimensionBudget { current: 0.0, raise_threshold: 1.0, lower_threshold: 0.0 },
        latency: DimensionBudget { current: 0.0, raise_threshold: 1.0, lower_threshold: 0.0 },
    };
    assert!(!b.should_raise()); // 0.7 < 0.8
    assert!(!b.should_lower()); // 0.7 > 0.3
}
```

- [ ] **Step 4: 验证**

Run:
```bash
cargo test -p model-router
cargo clippy -p model-router --all-targets -- -D warnings
```

- [ ] **Step 5: 提交**

```bash
git add crates/model-router
git commit -m "feat(v1.6.0): CACR 非对称预算控制（Task 23）

- 支持 Token/Cost/Latency 多维度独立阈值
- 升阈值宽松、降阈值严格，防止振荡
- 向后兼容双阈值场景

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 24: 主动安全不变量检查

**Files:**
- Create: `crates/seccore/src/invariants.rs`
- Modify: `crates/seccore/src/lib.rs`
- Create: `crates/seccore/tests/invariants.rs`

**Interfaces:**
- 输入：待执行命令 + 当前上下文
- 输出：通过 / 阻止 + `SecurityInvariantViolated` 事件

- [ ] **Step 1: 定义安全不变量**

`crates/seccore/src/invariants.rs`:
```rust
use event_bus::{NexusEvent, EventKind};

pub enum InvariantResult {
    Allow,
    Violate { reason: String },
}

pub struct SecurityInvariants;

impl SecurityInvariants {
    pub fn check(command: &Command, ctx: &SecurityContext) -> InvariantResult {
        // 不变量 1：不执行未审计命令
        if !ctx.audit.contains(command) {
            return InvariantResult::Violate { reason: "未审计命令".into() };
        }
        // 不变量 2：不超出预算限制
        if ctx.budget.exceeded() {
            return InvariantResult::Violate { reason: "预算已耗尽".into() };
        }
        // 不变量 3：QK-Clip（示例）
        if command.qk_score() > ctx.qk_clip_threshold {
            return InvariantResult::Violate { reason: "QK-Clip 超限".into() };
        }
        InvariantResult::Allow
    }

    pub fn to_event(reason: String) -> NexusEvent {
        NexusEvent {
            kind: EventKind::SecurityInvariantViolated,
            payload: EventPayload::InvariantReason(reason),
            priority: EventPriority::Priority,
            timestamp: now(),
        }
    }
}
```

- [ ] **Step 2: 在命令执行入口调用不变量检查**

`crates/seccore/src/lib.rs`:
```rust
pub async fn execute(command: Command, ctx: SecurityContext) -> Result<Output, SecError> {
    match SecurityInvariants::check(&command, &ctx) {
        InvariantResult::Allow => { /* 正常执行 */ }
        InvariantResult::Violate { reason } => {
            event_bus.publish(SecurityInvariants::to_event(reason.clone()));
            return Err(SecError::InvariantViolated(reason));
        }
    }
}
```

- [ ] **Step 3: 编写不变量测试**

`crates/seccore/tests/invariants.rs`:
```rust
use seccore::invariants::{SecurityInvariants, InvariantResult};

#[test]
fn unaudited_command_is_blocked() {
    let cmd = Command::new("rm -rf /");
    let ctx = SecurityContext::empty();
    assert!(matches!(
        SecurityInvariants::check(&cmd, &ctx),
        InvariantResult::Violate { .. }
    ));
}
```

- [ ] **Step 4: 验证**

Run:
```bash
cargo test -p seccore
cargo clippy -p seccore --all-targets -- -D warnings
```

- [ ] **Step 5: 提交**

```bash
git add crates/seccore
git commit -m "feat(v1.6.0): 主动安全不变量检查（Task 24）

- 增加 SecurityInvariants 检查未审计命令、预算超限、QK-Clip
- 违反时阻止执行并发布 SecurityInvariantViolated 事件
- 正常操作不受影响

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Wave 5: Phase V 性能微优化

### Task 25: 热路径 clone 减少

**Files:**
- Modify: `crates/mlc-engine/src/*.rs`
- Modify: `crates/cmt-tiering/src/*.rs`
- Modify: `crates/scc-cache/src/*.rs`
- Create: `crates/mlc-engine/benches/clone_count.rs`
- Create: `crates/cmt-tiering/benches/clone_count.rs`
- Create: `crates/scc-cache/benches/clone_count.rs`

**Interfaces:**
- 输入：当前 clone 密集代码
- 输出：热路径 clone 减少 ≥ 30%

- [ ] **Step 1: 统计当前 clone 数量**

Grep 各 crate 中的 `.clone()` 调用：
```bash
grep -r "\.clone()" crates/mlc-engine/src | wc -l
grep -r "\.clone()" crates/cmt-tiering/src | wc -l
grep -r "\.clone()" crates/scc-cache/src | wc -l
```
记录基线。

- [ ] **Step 2: 识别热路径上的不必要深拷贝**

使用 bench 和 flamegraph（如可用）识别热路径。重点关注：
- `String` / `Vec` 在循环中的 clone
- 大结构体的函数参数传递
- 返回值的 `.clone()`

- [ ] **Step 3: 替换为 Arc 共享或引用传递**

示例模式：
```rust
// 修改前
fn process(data: Vec<f32>) -> Vec<f32> { /* ... */ data.clone() }

// 修改后
fn process(data: Arc<[f32]>) -> Arc<[f32]> { /* ... */ data }
```

- [ ] **Step 4: 验证 clone 数量下降**

重新统计并确认下降 ≥ 30%。

- [ ] **Step 5: 运行测试**

```bash
cargo test -p mlc-engine -p cmt-tiering -p scc-cache
cargo clippy -p mlc-engine -p cmt-tiering -p scc-cache --all-targets -- -D warnings
```

- [ ] **Step 6: 提交**

```bash
git add crates/mlc-engine crates/cmt-tiering crates/scc-cache
git commit -m "perf(v1.6.0): 热路径 clone 减少（Task 25）

- mlc-engine / cmt-tiering / scc-cache 热路径深拷贝优化
- String/Vec 替换为 Arc 共享或引用传递
- bench 验证性能提升

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 26: VectorIndex Mutex→RwLock（已完成，需验证）

**Files:**
- Verify: `crates/repo-wiki/src/vector.rs`
- Test: `crates/repo-wiki/tests/vector_concurrent.rs`
- Bench: `crates/repo-wiki/benches/vector_search.rs`

- [ ] **Step 1: 验证当前实现**

Read `crates/repo-wiki/src/vector.rs`，确认 `Mutex<HashMap>` 已改为 `RwLock<HashMap>`。

- [ ] **Step 2: 编写并发搜索测试**

`crates/repo-wiki/tests/vector_concurrent.rs`:
```rust
use repo_wiki::vector::VectorIndex;
use std::sync::Arc;

#[tokio::test]
async fn concurrent_knn_reads_do_not_block() {
    let index = Arc::new(VectorIndex::new_in_memory());
    // 预插入数据
    index.insert("id1", vec![1.0, 0.0, 0.0]).await.unwrap();

    let mut set = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let idx = index.clone();
        set.spawn(async move {
            idx.knn(vec![1.0, 0.0, 0.0], 1).await.unwrap();
        });
    }
    while set.join_next().await.is_some() {}
}
```

- [ ] **Step 3: 验证**

Run:
```bash
cargo test -p repo-wiki
cargo clippy -p repo-wiki --all-targets -- -D warnings
```

- [ ] **Step 4: 提交**

```bash
git add crates/repo-wiki
git commit -m "perf(v1.6.0): VectorIndex RwLock 并发搜索验证（Task 26）

- 确认 Mutex<HashMap> 已替换为 RwLock<HashMap>
- 并发 KNN 搜索测试验证读不阻塞
- bench 验证吞吐量提升

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 27: 双格式序列化完成

**Files:**
- Create: `crates/event-bus/src/codec.rs`
- Modify: `crates/event-bus/src/lib.rs`
- Modify: `crates/event-bus/Cargo.toml`
- Create: `crates/event-bus/tests/codec.rs`

**Interfaces:**
- 输入：`NexusEvent`
- 输出：JSON（<1KB）或 MessagePack（≥1KB）字节流

- [ ] **Step 1: 添加 rmp-serde 依赖**

`crates/event-bus/Cargo.toml`:
```toml
[dependencies]
rmp-serde = "1.1"
```

- [ ] **Step 2: 实现自动选择编码器**

`crates/event-bus/src/codec.rs`:
```rust
use serde::{Serialize, Deserialize};

pub enum SerializationFormat {
    Json,
    MessagePack,
}

const SIZE_THRESHOLD: usize = 1024;

pub fn serialize<T: Serialize>(value: &T) -> Result<(Vec<u8>, SerializationFormat), CodecError> {
    let json = serde_json::to_vec(value)?;
    if json.len() < SIZE_THRESHOLD {
        Ok((json, SerializationFormat::Json))
    } else {
        let msgpack = rmp_serde::to_vec(value)?;
        Ok((msgpack, SerializationFormat::MessagePack))
    }
}

pub fn deserialize<T: for<'de> Deserialize<'de>>(bytes: &[u8], format: SerializationFormat) -> Result<T, CodecError> {
    match format {
        SerializationFormat::Json => serde_json::from_slice(bytes).map_err(Into::into),
        SerializationFormat::MessagePack => rmp_serde::from_slice(bytes).map_err(Into::into),
    }
}
```

- [ ] **Step 3: 在 event-bus 序列化入口集成**

`crates/event-bus/src/lib.rs`:
- 替换原有单一 JSON 序列化，调用 `codec::serialize`
- 在消息头中携带 format 标识

- [ ] **Step 4: 编写测试**

`crates/event-bus/tests/codec.rs`:
```rust
use event_bus::codec::{serialize, deserialize, SerializationFormat};

#[test]
fn small_payload_uses_json() {
    let small = vec![1u8; 100];
    let (bytes, format) = serialize(&small).unwrap();
    assert!(matches!(format, SerializationFormat::Json));
    assert_eq!(deserialize::<Vec<u8>>(&bytes, format).unwrap(), small);
}

#[test]
fn large_payload_uses_messagepack() {
    let large = vec![1u8; 2048];
    let (bytes, format) = serialize(&large).unwrap();
    assert!(matches!(format, SerializationFormat::MessagePack));
    assert_eq!(deserialize::<Vec<u8>>(&bytes, format).unwrap(), large);
}
```

- [ ] **Step 5: 验证**

Run:
```bash
cargo test -p event-bus
cargo clippy -p event-bus --all-targets -- -D warnings
```

- [ ] **Step 6: 提交**

```bash
git add crates/event-bus
git commit -m "feat(v1.6.0): event-bus 双格式序列化自动选择（Task 27）

- <1KB payload 使用 JSON
- ≥1KB payload 使用 MessagePack
- 消息头携带 format 标识

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 28: Prometheus 指标扩展

**Files:**
- Create: `crates/event-bus/src/metrics.rs`
- Create: `crates/efficiency-monitor/src/metrics.rs`
- Modify: `crates/event-bus/src/lib.rs`
- Modify: `crates/efficiency-monitor/src/lib.rs`
- Modify: `crates/event-bus/Cargo.toml`
- Modify: `crates/efficiency-monitor/Cargo.toml`
- Create: `crates/event-bus/tests/metrics.rs`
- Create: `crates/efficiency-monitor/tests/metrics.rs`

**Interfaces:**
- event-bus: `events_published_total{kind}` Counter
- efficiency-monitor: `alerts_triggered_total{level}` Counter

- [ ] **Step 1: 添加 prometheus-client 依赖**

参照 `crates/repo-wiki/Cargo.toml` 中已有的 `prometheus-client` 版本，在 `event-bus` 和 `efficiency-monitor` 中添加：
```toml
[dependencies]
prometheus-client = "0.22"
```

- [ ] **Step 2: 实现 event-bus 指标**

`crates/event-bus/src/metrics.rs`:
```rust
use prometheus_client::metrics::counter::Counter;
use prometheus_client::registry::Registry;
use std::sync::atomic::AtomicU64;
use std::collections::HashMap;

pub struct EventMetrics {
    pub published: HashMap<EventKind, Counter<AtomicU64, u64>>,
}

impl EventMetrics {
    pub fn inc_published(&mut self, kind: EventKind) {
        self.published.entry(kind).or_default().inc();
    }
}
```

- [ ] **Step 3: 实现 efficiency-monitor 指标**

`crates/efficiency-monitor/src/metrics.rs`:
```rust
use prometheus_client::metrics::counter::Counter;
use std::sync::atomic::AtomicU64;
use std::collections::HashMap;

pub struct MonitorMetrics {
    pub alerts_triggered: HashMap<AlertLevel, Counter<AtomicU64, u64>>,
}

impl MonitorMetrics {
    pub fn inc_alert(&mut self, level: AlertLevel) {
        self.alerts_triggered.entry(level).or_default().inc();
    }
}
```

- [ ] **Step 4: 在业务代码中埋点**

`crates/event-bus/src/lib.rs`:
```rust
self.metrics.inc_published(event.kind);
```

`crates/efficiency-monitor/src/lib.rs`:
```rust
self.metrics.inc_alert(alert.level);
```

- [ ] **Step 5: 编写测试**

`crates/event-bus/tests/metrics.rs`:
```rust
use event_bus::{EventBus, EventKind};

#[tokio::test]
async fn event_metrics_increments_on_publish() {
    let mut bus = EventBus::new();
    bus.publish(NexusEvent::dummy(EventKind::Normal)).await;
    assert_eq!(bus.metrics.published.get(&EventKind::Normal).unwrap().get(), 1);
}
```

- [ ] **Step 6: 验证**

Run:
```bash
cargo test -p event-bus -p efficiency-monitor
cargo clippy -p event-bus -p efficiency-monitor --all-targets -- -D warnings
```

- [ ] **Step 7: 提交**

```bash
git add crates/event-bus crates/efficiency-monitor
git commit -m "feat(v1.6.0): Prometheus 指标扩展（Task 28）

- event-bus 暴露 events_published_total（按 EventKind 分维度）
- efficiency-monitor 暴露 alerts_triggered_total（按 AlertLevel 分维度）
- 指标值正确反映操作

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 29: heuristic_scores() 真实实现

**Files:**
- Create: `crates/osa-coordinator/src/heuristic.rs`
- Modify: `crates/osa-coordinator/src/coordinator.rs`
- Create: `crates/osa-coordinator/tests/heuristic.rs`

**Interfaces:**
- 输入：工具能力 + 用户意图 CLV
- 输出：工具匹配度评分

- [ ] **Step 1: 实现真实评分函数**

`crates/osa-coordinator/src/heuristic.rs`:
```rust
use nexus_core::CLV;

pub struct ToolCapability {
    pub name: String,
    pub tags: Vec<String>,
    pub example_clv: CLV,
}

pub fn heuristic_score(tool: &ToolCapability, intent_clv: &CLV, intent_tags: &[String]) -> f32 {
    let clv_sim = cosine_similarity(tool.example_clv, intent_clv);
    let tag_overlap = tag_jaccard(&tool.tags, intent_tags);
    // 0.7 * CLV 相似度 + 0.3 * 标签重叠
    0.7 * clv_sim + 0.3 * tag_overlap
}

fn cosine_similarity(a: &CLV, b: &CLV) -> f32 {
    nexus_core::clv::cosine_similarity_slices(&a.0, &b.0)
}

fn tag_jaccard(a: &[String], b: &[String]) -> f32 {
    let a: std::collections::HashSet<_> = a.iter().cloned().collect();
    let b: std::collections::HashSet<_> = b.iter().cloned().collect();
    let intersection = a.intersection(&b).count() as f32;
    let union = a.union(&b).count() as f32;
    if union == 0.0 { 0.0 } else { intersection / union }
}
```

- [ ] **Step 2: 替换 coordinator 中的占位符**

`crates/osa-coordinator/src/coordinator.rs`:
```rust
// 替换原有的 heuristic_scores() 占位实现
pub fn heuristic_scores(
    &self,
    tools: &[ToolCapability],
    intent_clv: &CLV,
    intent_tags: &[String],
) -> Vec<f32> {
    tools.iter().map(|t| heuristic::heuristic_score(t, intent_clv, intent_tags)).collect()
}
```

- [ ] **Step 3: 编写测试**

`crates/osa-coordinator/tests/heuristic.rs`:
```rust
use osa_coordinator::heuristic::{heuristic_score, ToolCapability};
use nexus_core::CLV;

#[test]
fn perfect_match_scores_high() {
    let tool = ToolCapability {
        name: "search".into(),
        tags: vec!["search".into(), "web".into()],
        example_clv: CLV::from_slice(&[1.0; 512]),
    };
    let score = heuristic_score(&tool,
        &CLV::from_slice(&[1.0; 512]),
        &["search".into()],
    );
    assert!(score > 0.9);
}
```

- [ ] **Step 4: 验证**

Run:
```bash
cargo test -p osa-coordinator
cargo clippy -p osa-coordinator --all-targets -- -D warnings
```

- [ ] **Step 5: 提交**

```bash
git add crates/osa-coordinator
git commit -m "feat(v1.6.0): heuristic_scores() 真实实现（Task 29）

- 基于 CLV 余弦相似度 + 能力标签匹配
- 替换 coordinator 占位符
- TDD 测试验证评分正确性

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Wave 6: Phase VI 文档与经验沉淀

### Task 30: 更新 CODE_WIKI.md

**Files:**
- Modify: `CODE_WIKI.md`

- [ ] **Step 1: 更新 crate 索引**

在 CODE_WIKI.md §3.1 中确认：
- 35 crate 数量正确
- online-learning 已登记
- 新增模块（如 codec/metrics）有说明

- [ ] **Step 2: 新增 ADR 权威源**

在 CODE_WIKI.md §2.3 添加：
```markdown
- ADR-011: 四级事件优先级与 Priority 优先投递
- ADR-012: Speculative DAG 执行
- ADR-013: CLV 分层压缩
```

- [ ] **Step 3: 验证**

Grep 确认 ADR 编号连续，无冲突。

- [ ] **Step 4: 提交**

```bash
git add CODE_WIKI.md
git commit -m "docs(v1.6.0): 更新 CODE_WIKI.md（Task 30）

- crate 索引反映 v1.6.0 变更
- 新增 ADR-011/012/013 权威源

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 31: 更新 CHANGELOG.md

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: 添加 v1.6.0-omega 汇总章节**

在 CHANGELOG.md 顶部新增：
```markdown
## v1.6.0-omega 汇总

### 编译基线
- 修复 mlc-engine / nmc-encoder / repo-wiki / hcw-window / parliament 预存在编译错误

### P0/P1 修复
- WikiStore 异步读写分离（r2d2 连接池）
- ModelRegistry DashMap→RwLock
- cmt-tiering / scc-cache SQLite 连接池
- 四级事件优先级 Priority 优先投递
- L6 路由链路代码级顺序保证
- AuditChain 并发化

### YAGNI 重新评估
- NexusState Arc 共享：根据 bench 决策
- TaskProfile Hash trait：根据 bench 决策
- EDSB 次优选择策略改进
- cosine_similarity 优化：根据 bench 决策
- NMC Perceptor 并行化：根据评估决策
- gsoe spawn_blocking：根据 bench 决策

### OMEGA 创新深化
- Speculative DAG 执行（激进投机）
- CLV 分层压缩（L0/L1/L2）
- GRPO 自适应任务评分
- OS-Memory Wiki 元遗忘
- CACR 非对称预算控制
- 主动安全不变量检查

### 性能微优化
- 热路径 clone 减少
- VectorIndex RwLock 并发搜索
- event-bus 双格式序列化
- Prometheus 指标扩展
- heuristic_scores() 真实实现

### 文档与经验
- CODE_WIKI.md 更新
- ADR-011/012/013
- project_memory 原则 23+
```

- [ ] **Step 2: 记录跳过的任务及原因**

对每个 YAGNI 未实施项，记录："bench 显示未达阈值，延后至 v1.7.0 评估"。

- [ ] **Step 3: 提交**

```bash
git add CHANGELOG.md
git commit -m "docs(v1.6.0): 更新 CHANGELOG.md（Task 31）

- 添加 v1.6.0-omega 汇总章节
- 记录每个 Phase 变更与跳过任务原因

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 32: project_memory 经验沉淀

**Files:**
- Modify: `project_memory.md`（或项目记忆文件路径）

- [ ] **Step 1: 新增原则 23+**

示例新增原则：
```markdown
### 原则 23: 编译基线零错误优先
任何优化或新功能前，必须先确保 `cargo check --workspace` 退出码 0。编译错误会掩盖真实问题，不可接受。

### 原则 24: SQLite 并发必须连接池
`rusqlite` 的 `Connection` 不是 `Sync`，多并发场景应使用 r2d2 连接池 + spawn_blocking，禁止持锁跨 await。

### 原则 25: 事件优先级需显式分级
关键治理事件必须使用独立 Priority 级，避免与 Normal 事件混排导致响应延迟。

### 原则 26: DAG 投机执行需可回滚
激进并行必须配套失败回滚机制，确保依赖分支结果一致性。

### 原则 27: CLV 维度变更需 ADR
CLV 作为核心跨层类型，维度/压缩级别变更必须经 ADR 记录并评估向后兼容性。

### 原则 28: 预算控制非对称防振荡
升阈值宽松、降阈值严格，避免模型在阈值边界反复切换。
```

- [ ] **Step 2: 验证编号连续**

Grep 确认原则编号从 23 开始连续无重复。

- [ ] **Step 3: 提交**

```bash
git add project_memory.md
git commit -m "docs(v1.6.0): project_memory 经验沉淀（Task 32）

- 新增原则 23-28
- 覆盖编译基线、SQLite 连接池、事件优先级、DAG、CLV、预算控制

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Wave 7: Phase VII 全量验证与交付

### Task 33: 全量验证与交付

**Files:**
- Modify: `Cargo.toml`
- Modify: `.trae/specs/v1-6-0-omega-comprehensive-deep-optimization/checklist.md`
- Create: `docs/reports/v1-6-0-omega-comprehensive-report.md`

- [ ] **Step 1: 同步版本号**

修改 `Cargo.toml`：
```toml
[workspace.package]
version = "1.6.0-omega"
```

- [ ] **Step 2: 运行完整验证套件**

```bash
export CARGO_HOME='D:/Chimera CLI/.toolchain/cargo'
export RUSTUP_HOME='D:/Chimera CLI/.toolchain/rustup'
export TMP='D:/Chimera CLI/tmp'
export TEMP='D:/Chimera CLI/tmp'
export CARGO_INCREMENTAL='0'

cd "D:/Chimera CLI"
"D:/Chimera CLI/.toolchain/cargo/bin/cargo.exe" check --workspace
"D:/Chimera CLI/.toolchain/cargo/bin/cargo.exe" test --workspace --jobs 1
"D:/Chimera CLI/.toolchain/cargo/bin/cargo.exe" clippy --workspace --all-targets --jobs 2 -- -D warnings
"D:/Chimera CLI/.toolchain/cargo/bin/cargo.exe" fmt --all -- --check
"D:/Chimera CLI/.toolchain/cargo/bin/cargo.exe" audit --deny warnings --ignore RUSTSEC-2026-0190 --ignore RUSTSEC-2026-0002 --ignore RUSTSEC-2024-0436
```

- [ ] **Step 3: 运行 Docker 构建验证**

```bash
docker build -t chimera-cli:v1.6.0-rc .
docker run --rm chimera-cli:v1.6.0-rc --version
docker image inspect chimera-cli:v1.6.0-rc --format '{{.Size}}'
```
Expected: 版本输出匹配 `^(aether|chimera) 1\.6\.0`，镜像体积 < 100 MB

- [ ] **Step 4: 运行 release binary 体积验证**

```bash
"D:/Chimera CLI/.toolchain/cargo/bin/cargo.exe" build --workspace --release
ls -lh target/release/aether.exe
```
Expected: binary 体积 < 50 MB

- [ ] **Step 5: 更新 checklist**

将 `.trae/specs/v1-6-0-omega-comprehensive-deep-optimization/checklist.md` 所有 `[ ]` 改为 `[x]`，并记录每个 checkpoint 的验证命令/文件证据。

- [ ] **Step 6: 生成综合报告**

`docs/reports/v1-6-0-omega-comprehensive-report.md`:
```markdown
# v1.6.0-omega 综合优化报告

## 执行摘要
- 完成 Task 数：33/33
- 新增代码文件：约 25 个
- 修改 crate 数：约 20 个
- 全量测试通过：是
- 镜像体积：XX MB
- Release binary 体积：XX MB

## 各 Wave 验证结果
| Wave | 关键输出 | 验证状态 |
|------|---------|---------|
| Wave 1 | 编译基线 | ✅ |
| Wave 2 | P0/P1 修复 | ✅ |
| Wave 3 | YAGNI 评估 | ✅/延后 X 项 |
| Wave 4 | 创新深化 | ✅ |
| Wave 5 | 性能优化 | ✅ |
| Wave 6 | 文档沉淀 | ✅ |
| Wave 7 | 全量验证 | ✅ |

## 性能数据摘要
- WikiStore 并发读吞吐量：提升 X 倍
- ModelRegistry ≤10 模型延迟：降低 X%
- 热路径 clone 减少：X%
- cosine_similarity 优化：X%（若实施）

## 风险与后续建议
- ...
```

- [ ] **Step 7: 提交**

```bash
git add Cargo.toml .trae/specs/v1-6-0-omega-comprehensive-deep-optimization/checklist.md docs/reports/v1-6-0-omega-comprehensive-report.md
git commit -m "release(v1.6.0): 全量验证与交付（Task 33）

- workspace version 同步为 1.6.0-omega
- cargo check/test/clippy/fmt/audit 全部通过
- Docker 镜像与 release binary 体积达标
- checklist 全部勾选
- 综合报告完成

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 8: 发布 tag（用户授权后执行）**

```bash
git tag v1.6.0-omega
git push origin v1.6.0-omega
```

---

## 执行后检查清单

- [ ] 所有 33 个 Task 完成
- [ ] 所有 checkpoint `[x]`
- [ ] `Cargo.toml` version = "1.6.0-omega"
- [ ] `CHANGELOG.md` 有 v1.6.0-omega 章节
- [ ] `CODE_WIKI.md` 反映 v1.6.0 变更
- [ ] ADR-011/012/013 存在
- [ ] `project_memory.md` 原则 23+
- [ ] 全量验证套件通过
- [ ] Docker 镜像体积 < 100 MB
- [ ] Release binary 体积 < 50 MB
- [ ] 无向上依赖违规
- [ ] 所有 crate 保持 `#![forbid(unsafe_code)]`
- [ ] 用户确认推送 `v1.6.0-omega` tag

---
