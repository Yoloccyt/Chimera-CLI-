# 维度 C:性能瓶颈审计报告

## 1. 执行摘要

- **审计日期**: 2026-06-28
- **审计范围**: 34 个 crates 的性能模式(热路径 clone、Top-K 算法、并发原语、spawn_blocking 覆盖、分配热点、benchmark 完整性、算法复杂度)
- **总体评价**: **良好** — 核心热路径已广泛采用 `select_nth_unstable_by`、`FuturesUnordered`、`with_capacity` 等最佳实践,但存在 2 处 Critical 级 SQLite 阻塞异步运行时问题,以及若干 clone 优化未闭环的 Major 问题
- **问题数量**: Critical 2 / Major 5 / Minor 6

### 总体亮点

| 优化项 | 覆盖范围 | 评价 |
|--------|---------|------|
| `select_nth_unstable_by` Top-K 选择 | 10+ 个 crate | ⭐ 优秀:csn-substitutor、hcw-window、gea-activator、faae-router、ssra-fusion、kvbsr-router、sesa-router、mlc-engine、osa-coordinator 均已采用 |
| `FuturesUnordered` 流式并发 | gqep-executor、parliament | ⭐ 优秀:零 `join_all` 使用,GQEP/Parliament 均用 FuturesUnordered 流式处理 |
| `spawn_blocking` 包装 SQLite | cmt-tiering(warm/cold/ice)、mlc-engine(l3_procedural) | ⚠️ 部分覆盖:repo-wiki、scc-cache/wal 缺失 |
| `with_capacity` 预分配 | 70+ 处 | ⭐ 优秀:几乎所有 Vec/HashMap 初始化都预分配容量 |
| HCW `entries_index` O(1) 查找 | hcw-window | ⭐ 优秀:SubTask 19.5 已用 HashMap 索引替代 `iter().find()` O(n) |
| MLC L2 SharedCLV 池化 | mlc-engine | ⭐ 优秀:`Arc<[f32]>` intern 池,4096 条目内存从 8MB 降至 k×2KB |

---

## 2. 热路径 clone 扫描

### 2.1 HCW `get()` 返回深拷贝(Minor-4 未完全闭环)

**位置**: `crates/hcw-window/src/window.rs:157-165`

```rust
pub async fn get(&self, id: &str) -> Result<Option<ContextEntry>, HcwError> {
    let mut state = self.state.write().await;
    if let Some(entry) = state.get_mut(id) {
        entry.increment_access();
        return Ok(Some(entry.clone()));  // ← 深拷贝 ContextEntry(含 content String)
    }
    Ok(None)
}
```

**问题**: `get()` 在热路径上 `entry.clone()` 深拷贝 `ContextEntry`(含 `content: String`、`clv: Option<CLV>` 等大字段)。多消费者场景下每个消费者各自 clone,造成冗余分配。

**`get_arc()` 修复未闭环**: `window.rs:176-183` 提供了 `get_arc()` 返回 `Arc<ContextEntry>`,但内部实现仍为 `Arc::new(entry.clone())` — 先深拷贝再包 Arc,**未真正消除 clone**。根本原因是 `entries: Vec<ContextEntry>` 存储的是值而非 `Arc<ContextEntry>`。

**建议**: 将 `HcwState.entries` 改为 `Vec<Arc<ContextEntry>>`,`get_arc()` 直接返回 `Arc::clone(&entry)`(引用计数递增,零深拷贝)。`get()` 可保留向后兼容或标记 deprecated。

### 2.2 MLC `list_all()` 全量 clone

**位置**:
- `crates/mlc-engine/src/l0_working.rs:453`
- `crates/mlc-engine/src/l1_episodic.rs:296-301`
- `crates/mlc-engine/src/l2_semantic.rs:322-327`

```rust
pub fn list_all(&self) -> Result<Vec<MemoryEntry>, MlcError> {
    let inner = self.inner.read()...;
    Ok(inner.entries.values().cloned().collect())  // ← 全量 clone
}
```

**问题**: 三个 tier 的 `list_all()` 均 `values().cloned().collect()`,对 4096 条目(每条含 CLV 2KB)产生 ~8MB 堆分配。若在迁移/快照场景频繁调用,分配压力显著。

**建议**: 提供 `list_all_arc() -> Vec<Arc<MemoryEntry>>` 变体,或将 entries 存储为 `Arc<MemoryEntry>`。

### 2.3 KVBSR 候选工具向量 clone

**位置**: `crates/kvbsr-router/src/router.rs:334-338`

```rust
for tid in &candidate_tool_ids {
    if let Some(tv) = self.tool_vectors.get(tid) {
        candidate_tool_vectors.insert(tid.clone(), tv.clone());  // ← clone ToolVector(64-dim Vec<f32>)
    }
}
```

**问题**: 每次路由对 20-50 个候选工具 `tv.clone()` 深拷贝 `ToolVector`(64-dim `Vec<f32>` = 256 bytes + String id)。1000 次路由 = 25000 次分配。

**建议**: `tool_vectors` 改为 `DashMap<ToolId, Arc<ToolVector>>`,候选收集时 `Arc::clone` 零深拷贝;或直接在 DashMap entry 上计算相似度,无需收集到中间 HashMap。

### 2.4 OSA masks 二次 clone

**位置**: `crates/osa-coordinator/src/masks.rs:169-171`

```rust
let active_ids: Vec<T> = top_k_indices.iter().map(|&i| ids[i].clone()).collect();  // clone #1
let active_set = HashSet::from_iter(active_ids.iter().cloned());  // clone #2
```

**问题**: 先 clone Top-K ids 到 `Vec`,再从 `Vec` clone 到 `HashSet`。第二次 clone 可通过 `HashSet::from_iter(active_ids.iter().cloned())` 改为先建 set 再 collect vec,或直接 `active_ids.iter().cloned().collect::<HashSet<_>>()` 后保留所有权。

**影响**: Minor — K 通常 ≤ 50,影响有限。

### 2.5 csn-substitutor 目标向量 clone

**位置**: `crates/csn-substitutor/src/substitutor.rs:154`

```rust
let target_vector = match self.capabilities.get(capability_id) {
    Some(entry) => entry.semantic_vector.clone(),  // ← clone Vec<f32>
    None => return Vec::new(),
};
```

**问题**: 每次查找替代候选时 clone 目标向量。`r.key().clone()` 在 line 165 也对每个候选能力 clone capability_id。

**建议**: DashMap entry 期间直接借用 `entry.semantic_vector` 计算,或用 `Arc<Vec<f32>>` 共享。

---

## 3. Top-K 选择算法

### 3.1 核验结论:⭐ 优秀

全量扫描 46 处 `sort_by` / `sort_by_key` 调用,生产代码中 **所有 Top-K 场景均已使用 `select_nth_unstable_by`**。`sort_by` 仅用于:

1. **Top-K 内部排序**(K log K,可接受):`faae-router/src/router.rs:183`、`kvbsr-router/src/router.rs:440,485`、`mlc-engine/src/l2_semantic.rs:218`、`hcw-window/src/compressor.rs:161`、`osa-coordinator/src/masks.rs:162`、`csn-substitutor/src/substitutor.rs:180` — 均在 `select_nth_unstable_by` 之后对前 K 个元素排序确保降序
2. **全排序场景**(非 Top-K,sort_by 正确):`model-router/src/registry.rs:83,94`(按成本/延迟全排序)、`nexus-core/src/state.rs:89`(哈希确定性排序)、`ssra-fusion/src/templates.rs:166`(时间戳排序)
3. **测试代码**:`mlc-engine/tests/*.rs`、`hcw-window/tests/*.rs`、`osa-coordinator/tests/*.rs` 等延迟统计排序 — 非生产路径

### 3.2 已采用 select_nth_unstable_by 的 crate 清单

| Crate | 文件:行号 | 场景 |
|-------|----------|------|
| csn-substitutor | `src/substitutor.rs:240` | 能力替代 Top-K |
| hcw-window | `src/compressor.rs:155` | 上下文压缩 Top-N |
| gea-activator | `src/conflict.rs:148` | GEA 激活 Top-K |
| faae-router | `src/router.rs:178` | FaaE 工具选择 Top-K |
| ssra-fusion | `src/fusion/engine.rs:214` | SSRA 融合 Top-K |
| kvbsr-router | `src/router.rs:435,480` | KVBSR 两级路由 Top-N + Top-K |
| sesa-router | `src/sparsity.rs:139`、`src/activation.rs:328` | SESA 稀疏化 + 激活 Top-K |
| mlc-engine | `src/l2_semantic.rs:212` | L2 KNN 召回 Top-K |
| osa-coordinator | `src/masks.rs:153` | OSA 五维掩码 Top-K |

### 3.3 gea-activator 冲突检测的 sort_by(合理)

**位置**: `crates/gea-activator/src/conflict.rs:67-70`

```rust
// WHY 全排序而非 select_nth_unstable:冲突检测需按评分从高到低贪心遍历,
// select_nth_unstable 仅保证 Top-K 在前 K 位但内部无序,无法满足贪心顺序要求。
scored.sort_by(|a, b| b.2.partial_cmp(&a.2)...);
```

**评价**: 注释清晰,WHY 解释了为何此处用全排序而非部分排序 — 贪心算法需要全局有序。设计合理。

---

## 4. FuturesUnordered vs join_all

### 4.1 核验结论:⭐ 优秀

**全量扫描未发现任何 `join_all` 使用**(0 处)。并发收集均采用 `FuturesUnordered` 或等价的 `tokio::task::JoinSet`。

### 4.2 FuturesUnordered 使用情况

| Crate | 文件:行号 | 场景 | 评价 |
|-------|----------|------|------|
| gqep-executor | `src/gatherer.rs:94` | GQEP 聚集并发 future,集成 QEEP entangle | ⭐ 流式处理,慢操作不阻塞快操作 |
| parliament | `src/debate.rs:347` | 5 角色 Opinion 并发收集 | ⭐ 流式处理,yield 让出调度 |
| mcp-mesh | `src/quantum/superposition.rs` | 量子叠加 fanout | ⭐ 使用 `JoinSet`(等价 FuturesUnordered + spawn) |

### 4.3 GQEP 的 FuturesUnordered 设计

**位置**: `crates/gqep-executor/src/gatherer.rs:7-10`

```
//! - **FuturesUnordered vs join_all**:FuturesUnordered 支持流式处理
//!   `join_all` 在 1000 个 Future 同时聚集时内存峰值高。
```

**评价**: 明确以文档形式记录了选型理由,与架构红线"所有异步操作必须有 GQEP 聚集/超时处理"一致。

---

## 5. spawn_blocking 覆盖

### 5.1 核验结论:⚠️ 部分覆盖(2 处 Critical)

### 5.2 已正确使用 spawn_blocking 的 crate

| Crate | 文件 | SQLite/IO 操作 | 评价 |
|-------|------|---------------|------|
| cmt-tiering | `src/warm.rs:140-455` | WarmTier 全部 SQLite 操作(12 处) | ⭐ 所有 async 方法均 spawn_blocking |
| cmt-tiering | `src/cold.rs:177-530` | ColdTier 全部 SQLite 操作(16 处) | ⭐ 所有 async 方法均 spawn_blocking |
| cmt-tiering | `src/ice.rs:80-213` | IceTier 全部文件 IO(8 处) | ⭐ std::fs 读写均 spawn_blocking |
| mlc-engine | `src/l3_procedural.rs:126-406` | ProceduralMemory 全部 SQLite 操作(8 处) | ⭐ 所有 async 方法均 spawn_blocking |

### 5.3 ❌ Critical: repo-wiki/store.rs — SQLite 操作未用 spawn_blocking

**位置**: `crates/repo-wiki/src/store.rs:40-45, 138-186, 198-300`

```rust
pub struct WikiStore {
    conn: Mutex<Connection>,  // ← 同步 Mutex,无 spawn_blocking
}

pub fn insert(&self, entry: &WikiEntry) -> Result<(), WikiError> {
    let conn = self.conn.lock()...;  // ← 同步持锁
    conn.execute(...)?;  // ← 同步 SQLite 调用,阻塞异步运行时
    Ok(())
}
```

**问题**: `WikiStore` 的所有方法(`insert`/`get`/`delete`/`list_all`/`search_fulltext`)均为**同步方法**,直接在 `Mutex<Connection>` 上调用 `rusqlite::Connection::execute`/`query_row`。若从 async 上下文调用,将阻塞 Tokio 工作线程。

**对比**: cmt-tiering 的 `WarmTier`/`ColdTier` 同样使用 `Arc<Mutex<Connection>>`,但所有 async 方法内部用 `spawn_blocking(move || { ... })` 包装 SQLite 操作。repo-wiki 完全缺失此模式。

**影响**: Wiki 条目操作(单次 < 5ms)在 async 上下文中会阻塞整个工作线程,高并发下导致运行时饥荒。

**建议**: 将 `WikiStore` 方法改为 `async fn`,内部用 `spawn_blocking` 包装;或将 `Mutex<Connection>` 改为 `tokio::sync::Mutex` 并在 spawn_blocking 中 clone connection( rusqlite 不支持,需用 `r2d2` 连接池)。

### 5.4 ❌ Critical: scc-cache/wal.rs — SQLite WAL 操作未用 spawn_blocking

**位置**: `crates/scc-cache/src/wal.rs:260-305, 314-359`

```rust
pub struct SqliteWal {
    conn: Mutex<rusqlite::Connection>,  // ← 同步 Mutex
}

pub fn recover(&self) -> Result<Vec<WalEntry>, SccError> {
    let conn = self.conn.lock()...;  // ← 同步持锁
    let mut stmt = conn.prepare(...)?;  // ← 同步 SQLite,阻塞运行时
    ...
}
```

**问题**: `SqliteWal` 的 `new`/`recover`/`write_ahead_log`/`commit_log`/`rollback_log` 均为同步方法,直接调用 rusqlite。WAL 恢复(1000 条目)可能耗时数十毫秒,在 async 上下文中阻塞工作线程。

**影响**: SCC 缓存的崩溃恢复路径若在 async 上下文调用,会导致运行时阻塞。

**建议**: 实现 `WalTrait` 的 async 包装层,用 `spawn_blocking` 委托到同步 `SqliteWal`。

---

## 6. 分配热点

### 6.1 PVL verifier `to_lowercase()` 每次分配

**位置**: `crates/pvl-layer/src/verifier.rs:95`

```rust
let content_lower = operation.content.to_lowercase();  // ← 每次验证分配新 String
for keyword in DANGEROUS_KEYWORDS {
    if content_lower.contains(keyword) { ... }
}
```

**问题**: 每次验证分配 `content.to_lowercase()` 新 String。对于大操作(如长 shell 命令),分配开销显著。

**建议**: 用 `unicase` crate 进行大小写不敏感匹配,或用 `contains` + 手动大小写比较避免分配。

### 6.2 nexus-core `snapshot_hash` 排序 + 序列化

**位置**: `crates/nexus-core/src/state.rs:82-108`

```rust
let mut entries: Vec<_> = inner.active_quests.iter().collect();
entries.sort_by(|a, b| a.0.cmp(b.0));  // ← O(n log n) 排序
for (quest_id, quest) in &entries {
    let json = serde_json::to_string(quest).unwrap_or_default();  // ← 每条目分配 String
    ...
}
```

**问题**: 每次 `snapshot_hash()` 收集所有 quest 到 Vec + 排序 + 逐个 JSON 序列化。若 quest 数量大且哈希频繁调用,分配压力显著。

**建议**: 用 BTreeMap 存储 active_quests(天然有序,省排序);或缓存哈希值,仅在状态变更时重算。

### 6.3 model-router `list_by_cost/latency` 全量 clone + 排序

**位置**: `crates/model-router/src/registry.rs:81-96`

```rust
pub fn list_by_cost(&self) -> Vec<ModelInfo> {
    let mut models = self.list();  // ← clone 全部模型
    models.sort_by(...);  // ← O(n log n) 排序
    models
}
```

**问题**: `self.list()` 先 clone 所有 `ModelInfo`(`registry.rs:76-78`),再排序。若每次路由请求都调用,分配 + 排序开销累积。

**建议**: 缓存按成本/延迟排序的结果,仅在模型注册/注销时更新;或用 `select_nth_unstable` 仅选最优模型(若只需 Top-1)。

### 6.4 ⭐ with_capacity 预分配覆盖良好

全量扫描发现 70+ 处 `Vec::with_capacity` / `HashMap::with_capacity` / `String::with_capacity`,覆盖:
- `kvbsr-router/src/router.rs:220,320,333` — HashMap/HashSet 预分配
- `sesa-router/src/activation.rs:209` — scored Vec 预分配
- `nmc-encoder/src/fusion.rs:79` — merged CLV Vec 预分配
- `parliament/src/ahirt.rs:184,406,427` — payloads/results 预分配
- `efficiency-monitor/src/dashboard.rs:69` — `String::with_capacity(value.len())` 转义

**评价**: 预分配意识强,几乎无"裸 `Vec::new()` 后 push 循环"的反模式。

---

## 7. benchmark 覆盖完整性

### 7.1 覆盖率统计

- **总 crate 数**: 34
- **有 `benches/` 目录**: 24 个(70.6%)
- **无 `benches/` 目录**: 10 个(29.4%)

### 7.2 ❌ 缺失 benchmark 的核心 crate

| Crate | 层级 | 重要性 | 缺失影响 |
|-------|------|--------|---------|
| **nexus-core** | L1 Core | ⭐⭐⭐ | CLV 操作、state hash、sqlite_pragma 无基准 |
| **event-bus** | L1 Core | ⭐⭐⭐ | broadcast 吞吐量、背压无基准 |
| **model-router** | L1 Core | ⭐⭐ | CACR 决策、策略选择无基准 |
| **repo-wiki** | L5 Knowledge | ⭐⭐ | SQLite CRUD、向量检索无基准(且 spawn_blocking 缺失) |
| **qeep-protocol** | L4 Security | ⭐⭐ | 量子纠缠检测、orphan 检测无基准 |
| **decay-engine** | L4 Security | ⭐⭐ | 能力衰减计算无基准 |
| **acb-governor** | L8 Parliament | ⭐ | 预算分配、tier 切换无基准 |
| **auto-dpo** | L5 Knowledge | ⭐ | DPO 对生成无基准 |
| **chimera-cli** | L10 Interface | ⭐ | CLI 端到端无基准(可接受) |
| **chimera-tui** | L10 Interface | ⭐ | TUI 渲染无基准(可接受) |

### 7.3 benchmark 质量核验

| Bench 文件 | 规模 | 断言 | 评价 |
|-----------|------|------|------|
| `scc-cache/benches/wal_recovery.rs` | 1000 次崩溃恢复 | ✅ 单次 < 100ms + 零丢失 | ⭐ 优秀:含正确性验证 + 延迟测量 |
| `sesa-router/benches/three_layer_routing.rs` | 1000 工具(50块×20) | ✅ 验收标准文档化(p95 ≤ 5ms) | ⭐ 优秀:三层串联端到端 |
| `ssra-fusion/benches/fusion_benchmark.rs` | 10/50/100 模板 | ⚠️ 无运行时断言(文档 p95 ≤ 20ms) | 良好:多规模扫描 |
| `kvbsr-router/benches/route.rs` | 300 + 1000 工具 | ❌ 无断言 | 良好:双规模,缺加速比断言 |
| `osa-coordinator/benches/compute_masks.rs` | 50工具+2000文件+50记忆 | ❌ 无断言 | 良好:真实规模 profile |
| `mlc-engine/benches/l2_recall.rs` | **仅 100 条目** | ❌ 无断言 | ⚠️ 缺 4096 规模(config 容量 4096) |
| `hcw-window/benches/compress.rs` | 100K → 32K | ❌ 无断言 | 良好:对应压缩率 > 3× 目标 |

### 7.4 mlc-engine L2 benchmark 规模不足

**位置**: `crates/mlc-engine/benches/l2_recall.rs:21-37`

**问题**: 仅测试 100 条目 Top-10 召回,但 `SemanticMemory` 容量上限为 4096(`mlc-engine/src/config.rs`)。缺少 4096 规模基准,无法验证 "Top-10 召回 < 200ms" 的设计目标。

**建议**: 增加 `bench_l2_recall_4096_entries` 基准,验证大规模下的线性扫描 KNN 性能。

---

## 8. 性能基线数据核验

### 8.1 WAL 恢复基线

**文件**: `crates/scc-cache/benches/wal_recovery.rs`

| 指标 | project_memory 基线 | benchmark 验证 | 状态 |
|------|---------------------|---------------|------|
| 1000 次崩溃恢复零丢失 | ✅ | `verify_1000_crash_recoveries()` 每次 `assert_eq!(recovered.len(), expected)` | ✅ 已验证 |
| p95 = 4ms | ✅ | 单次断言 < 100ms(宽松阈值),实际 p95=4ms 需运行 bench 确认 | ⚠️ 阈值过宽 |
| 目标 2000ms | ✅ | 单次 < 100ms × 1000 = < 100s,远低于 2000ms 上限 | ✅ 满足 |

### 8.2 三层路由基线

**文件**: `crates/sesa-router/benches/three_layer_routing.rs`

| 指标 | project_memory 基线 | benchmark 验证 | 状态 |
|------|---------------------|---------------|------|
| 三层串联 p95 | 78.79µs | 验收标准 p95 ≤ 5ms,78.79µs << 5ms | ✅ 64× 余量 |
| 1000 工具规模 | ✅ | 50 块 × 20 工具/块 = 1000 | ✅ 规模匹配 |

### 8.3 SSRA 融合基线

**文件**: `crates/ssra-fusion/benches/fusion_benchmark.rs`

| 指标 | project_memory 基线 | benchmark 验证 | 状态 |
|------|---------------------|---------------|------|
| 100 模板融合 | 5.64µs | bench 测量 100 模板 fuse 延迟 | ✅ 已覆盖 |
| 目标 20ms | ✅ | 5.64µs << 20ms | ✅ 3500× 余量 |
| 目标 15ms(25% 余量) | ✅ | 5.64µs << 15ms | ✅ |

### 8.4 KVBSR 路由基线

**文件**: `crates/kvbsr-router/benches/route.rs`

| 指标 | project_memory 基线 | benchmark 验证 | 状态 |
|------|---------------------|---------------|------|
| 10× 加速(1000 tools) | ✅ | `bench_route_1000_tools` 存在,但无对比 full_scan 的断言 | ⚠️ 缺加速比断言 |
| 规模 1000 工具 / 50 块 × 20 工具 | ✅ | `make_scale_test_data()` 50 块 × 20 工具 | ✅ 规模匹配 |

**注**: `test_scale_speedup_vs_full_scan` 在 `tests/scale.rs` 中(阈值 2.0×),但 bench 本身无加速比断言。

### 8.5 OSA 掩码基线

**文件**: `crates/osa-coordinator/benches/compute_masks.rs`

| 指标 | project_memory 基线 | benchmark 验证 | 状态 |
|------|---------------------|---------------|------|
| compute_all_masks | — | 50工具+2000文件+50记忆+100操作+10任务 | ✅ 真实规模 |
| 无具体延迟基线 | — | 无断言 | ⚠️ 缺性能断言 |

---

## 9. 算法复杂度审计

### 9.1 ⭐ KVBSR 两级块路由 — 达到 10× 加速设计

**位置**: `crates/kvbsr-router/src/router.rs:290-391`

**复杂度分析**:
- 第一级(块级):50 块 × 64-dim 余弦 = O(50×64) = O(3200),`select_nth_unstable_by` O(50) 选 Top-3
- 第二级(块内):~60 工具(3块×20) × 64-dim 余弦 = O(60×64) = O(3840),`select_nth_unstable_by` O(60) 选 Top-8
- **总复杂度**: O(7040) vs 全扫描 O(1000×64) = O(64000),加速比 ≈ 9×,符合 10× 设计目标

**优化亮点**:
- `clv_to_block_dim` 返回 `&'a [f32]` 借用(line 408),零分配
- 锁内仅收集候选工具 ID(不 clone blocks),锁外 DashMap 无锁读(line 310-329)
- `select_nth_unstable_by` + K log K 排序(line 435-440)

### 9.2 ⭐ HCW 压缩器权重计算 — O(n + K log K)

**位置**: `crates/hcw-window/src/compressor.rs:68-231`

**复杂度分析**:
- 评分阶段:O(n) 遍历 + O(1) per entry `compute_importance`
- 选择阶段:`select_nth_unstable_by` O(n) + Top-K 排序 O(K log K)
- **总复杂度**: O(n + K log K),优于 O(n log n) 全排序

**优化亮点**:
- 接受 `&[ContextEntry]` 借用(SubTask 19.4),消除全量 clone
- 仅对贪心保留的 Top-N clone(line 190),1000 条目保留 100 条时 clone 次数从 1000 降到 100
- `estimated_k` 上界估计(line 150)确保 select_nth 语义正确

### 9.3 ⭐ MLC L2 语义检索 KNN — O(n×d) 线性扫描

**位置**: `crates/mlc-engine/src/l2_semantic.rs:177-231`

**复杂度分析**:
- 评分:O(n×d),n=4096,d=512 → ~2M 次浮点运算
- 选择:`select_nth_unstable_by` O(n) + Top-K 排序 O(K log K)
- **总复杂度**: O(n×d),100 条目 < 5ms,4096 条目 < 200ms(文档声明)

**优化亮点**:
- `Vec<(usize, f32)>` 索引替代 `Vec<(MemoryId, f32)>`(line 173-176),消除 4096 次 String 堆分配
- SharedCLV 池化(`Arc<[f32]>` intern),4096 条目内存从 8MB 降至 k×2KB
- 延迟 clone:仅对 Top-K(≤10)clone MemoryId(line 224-227)

### 9.4 PVL verifier 验证复杂度 — O(k×m)

**位置**: `crates/pvl-layer/src/verifier.rs:85-114`

**复杂度分析**:
- 语法检查:O(content.len()) trim
- 安全检查:O(k×m),k=7 关键词,m=content 长度 → O(7m)
- 依赖检查:O(m) contains
- **总复杂度**: O(m),m 为操作内容长度

**评价**: 占位实现(关键词列表),实际场景应从 SecCore 获取动态黑名单。`to_lowercase()` 分配是主要开销(见 §6.1)。

---

## 10. 问题清单

| ID | 严重程度 | 问题 | 代码位置 | 修复建议 |
|----|---------|------|---------|---------|
| C-01 | **Critical** | repo-wiki SQLite 操作未用 spawn_blocking,阻塞 async 运行时 | `crates/repo-wiki/src/store.rs:42,138-186,247,269` | 改为 async fn + spawn_blocking 包装,或用 r2d2 连接池 |
| C-02 | **Critical** | scc-cache/wal SQLite 操作未用 spawn_blocking,阻塞 async 运行时 | `crates/scc-cache/src/wal.rs:262,314-359` | 实现 WalTrait 的 async 包装层,spawn_blocking 委托 |
| M-01 | **Major** | HCW `get_arc()` 内部仍 `entry.clone()`,Minor-4 未完全闭环 | `crates/hcw-window/src/window.rs:180` | entries 改为 `Vec<Arc<ContextEntry>>`,get_arc 返回 `Arc::clone` |
| M-02 | **Major** | HCW `get()` 深拷贝 ContextEntry(含 content String) | `crates/hcw-window/src/window.rs:162` | 同 M-01,或提供 `get_ref` 返回 `Arc<ContextEntry>` |
| M-03 | **Major** | MLC 三个 tier `list_all()` 全量 clone,4096 条目 ~8MB 分配 | `crates/mlc-engine/src/{l0_working,l1_episodic,l2_semantic}.rs` | 提供 `list_all_arc()` 变体,或存储为 `Arc<MemoryEntry>` |
| M-04 | **Major** | 10 个核心 crate 缺失 benchmark(nexus-core、event-bus、model-router、repo-wiki、qeep-protocol、decay-engine 等) | 见 §7.2 | 优先为 nexus-core、event-bus、repo-wiki 补充基准 |
| M-05 | **Major** | mlc-engine L2 benchmark 仅 100 条目,缺 4096 规模验证 | `crates/mlc-engine/benches/l2_recall.rs:21` | 增加 `bench_l2_recall_4096_entries` 验证 < 200ms 目标 |
| m-01 | Minor | KVBSR 候选工具向量 `tv.clone()` 每次路由 20-50 次深拷贝 | `crates/kvbsr-router/src/router.rs:336` | tool_vectors 改为 `DashMap<ToolId, Arc<ToolVector>>` |
| m-02 | Minor | PVL verifier `to_lowercase()` 每次验证分配新 String | `crates/pvl-layer/src/verifier.rs:95` | 用 unicase 或手动大小写比较 |
| m-03 | Minor | nexus-core `snapshot_hash` 每次排序 + 逐条 JSON 序列化 | `crates/nexus-core/src/state.rs:88-95` | 用 BTreeMap 天然有序,或缓存哈希值 |
| m-04 | Minor | model-router `list_by_cost/latency` 全量 clone + 排序 | `crates/model-router/src/registry.rs:81-96` | 缓存排序结果,仅注册/注销时更新 |
| m-05 | Minor | OSA masks `active_set` 二次 clone Top-K ids | `crates/osa-coordinator/src/masks.rs:169-171` | 先建 HashSet 再 collect Vec,或直接消费 active_ids |
| m-06 | Minor | csn-substitutor `target_vector.clone()` + `r.key().clone()` 每候选 | `crates/csn-substitutor/src/substitutor.rs:154,165` | DashMap entry 期间借用计算,或 Arc 共享向量 |

---

## 11. 长期主义建议

### 11.1 SQLite 访问层统一化

**当前状态**: 4 个 crate(cmt-tiering、mlc-engine、repo-wiki、scc-cache)各自实现 SQLite 访问,spawn_blocking 覆盖不一致。

**建议**: 在 `nexus-core` 或新建 `sqlite-util` crate 中提供统一的 `AsyncSqlitePool` 抽象:
- 封装 `r2d2` 连接池 + `spawn_blocking`
- 所有 crate 通过统一 async API 访问 SQLite,消除 spawn_blocking 遗漏风险
- 统一应用 `apply_performance_pragmas`(已存在于 `nexus-core/src/sqlite_pragma.rs`)

### 11.2 Arc 共享存储改造

**当前状态**: HCW、MLC、KVBSR 等热路径存在 clone 大对象(ContextEntry、MemoryEntry、ToolVector)。

**建议**: 逐步将读多写少的大对象存储从值类型改为 `Arc<T>`:
- HCW: `Vec<ContextEntry>` → `Vec<Arc<ContextEntry>>`
- MLC: `HashMap<MemoryId, MemoryEntry>` → `HashMap<MemoryId, Arc<MemoryEntry>>`
- KVBSR: `DashMap<ToolId, ToolVector>` → `DashMap<ToolId, Arc<ToolVector>>`

改造后 `get()`/`get_arc()` 返回 `Arc::clone`(原子计数递增,零深拷贝),彻底消除热路径 clone。

### 11.3 benchmark 全覆盖计划

**建议**: 按以下优先级补充缺失 benchmark:

1. **P0**: `nexus-core`(CLV 操作、state hash)— L1 Core 是所有上层的基础
2. **P0**: `event-bus`(broadcast 吞吐、背压触发)— L1 通信枢纽
3. **P1**: `repo-wiki`(SQLite CRUD + 向量检索)— 补充后可同时验证 spawn_blocking 修复效果
4. **P1**: `qeep-protocol`(orphan 检测、entangle 开销)— L4 安全关键路径
5. **P2**: `model-router`、`decay-engine`、`acb-governor` — 治理与衰减

### 11.4 性能回归 CI 守护

**建议**: 在周验收流程(§7.2)中增加 benchmark 回归检测:
- 将关键 bench 指标(WAL p95、三层路由 p95、SSRA 5.64µs)写入 `perf-baseline.json`
- CI 中运行 bench 并与基线对比,回归 > 10% 时告警
- 防止性能退化在后续开发中悄悄累积

### 11.5 sqlite-vec 接入规划

**当前状态**: `repo-wiki/src/vector.rs` 与 `mlc-engine/src/l2_semantic.rs` 均用线性扫描 KNN,文档标注"Week 6 后接入 sqlite-vec"。

**建议**: 当 MLC L2 条目超过 4096 或 repo-wiki 超过 10000 时,线性扫描 O(n×d) 将成瓶颈。应按计划接入 `sqlite-vec` 扩展,将 KNN 复杂度从 O(n×d) 降至 O(log n × d)。需解决 `#![forbid(unsafe_code)]` 与 `sqlite3_auto_extension` 的 unsafe 冲突(参考 `repo-wiki/src/vector.rs:6-15` 的设计决策记录)。
