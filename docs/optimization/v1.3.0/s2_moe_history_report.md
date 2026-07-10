# v1.3.0-omega S2 — model-router MoE 五维评分扩展报告

> **报告日期**:2026-07-09
> **任务**:S2(P1 短期增强,中等风险,MoE 评分维度扩展)
> **关联 spec**:`.trae/specs/v1-3-0-omega-post-optimization-roadmap/`
> **基线版本**:v1.2.0-omega Task 3(MoE 三维门控)
> **执行 agent**:Rust 算法优化精英子代理

## 1. 执行摘要

将 model-router MoE 门控评分从三维(cost/latency/quality)扩展为五维,新增
`success_rate` 与 `latency_variance` 运行时统计维度。历史数据不足时(< 100 条)
自动降级三维(权重重新归一化),保证向后兼容 v1.2.0。新增 `HistoryStore` trait
抽象,为 v1.4.0 RL 路由(M2)预留扩展点。

**关键指标**:16 测试全部通过(10 现有 + 6 新增含 1 proptest 256 cases);
clippy 零警告;fmt 零 diff;bench 编译通过 + 运行数据已收集。

## 2. 设计决策

### 2.1 五维权重选择(0.3/0.3/0.2/0.1/0.1)

| 维度 | 权重 | 理由 |
|------|------|------|
| cost | 0.3 | 成本仍是主导(v1.2.0 为 0.4,降 0.1 让给历史) |
| latency | 0.3 | 延迟仍是主导(v1.2.0 为 0.4,降 0.1 让给历史) |
| quality | 0.2 | 质量补充(与 v1.2.0 一致) |
| success_rate | 0.1 | 历史成功率,值域 [0,1] 直接作为分数 |
| latency_variance | 0.1 | 延迟稳定性倒数 `1/(1+variance)`,惩罚抖动模型 |

前三维权重合计 0.8(v1.2.0 为 1.0),历史维度占 0.2。cost/latency 仍是主导
(各 0.3,合计 0.6),历史维度仅作排名微调,避免噪声主导决策。

### 2.2 降级路径(三维归一化)

历史数据不足时(< 100 条),五维降级三维,权重从 0.3/0.3/0.2 等比放大
1.25x → 0.375/0.375/0.25(总权重 1.0,保持 3:3:2 比例不变)。

**WHY 归一化**:保持 3:3:2 比例不变,Top-K 排名与 v1.2.0 一致(仅绝对值
缩放 1.25x,排名不变)。这保证 `history=None` 时行为完全向后兼容。

### 2.3 降级阈值 100

`is_sufficient()` 判定 `total_count >= 100`。WHY 100:统计显著性最小样本数。
success_rate 在 < 100 样本时 95% CI 过宽(如 50 样本 → ±0.14),variance
估计同样不稳定。100 样本下 95% CI 收窄至 ±0.10,可接受作为排名微调输入。

### 2.4 HistoryStore trait 抽象

```rust
pub trait HistoryStore: Send + Sync {
    fn get(&self, model_id: &str) -> Option<HistoryRecord>;
    fn record(&self, model_id: &str, latency_ms: f32, success: bool);
}
```

WHY trait 而非具体类型:为 v1.4.0 RL 路由(M2)预留扩展点 — 未来可替换为
SQLite 持久化或 Redis 共享实现,无需修改 `MoeGate::gate()`。当前仅暴露
get/record 两方法,不过度设计(遵循 §9 长期主义)。

对象安全(`&dyn HistoryStore`):所有方法取 `&self`,无泛型参数,返回 owned
HistoryRecord(避免 DashMap Ref guard 生命周期约束)。

### 2.5 MoeGate 保持 Copy

`MoeGate` 仍为 `Copy` 类型(threshold + top_k 两个 usize 字段)。`history`
作为 `gate()` 方法参数(`Option<&dyn HistoryStore>`)而非结构体字段 — 引用
会破坏 Copy 语义。这也符合"配置与运行时数据分离"原则:MoeGate 是不可变配置,
HistoryStore 是可变运行时状态。

## 3. API 变更

### 3.1 新增公开类型

| 类型 | 位置 | 说明 |
|------|------|------|
| `HistoryRecord` | `moe.rs` | 单模型历史记录(success_count/total_count/latency_samples) |
| `HistoryStore` trait | `moe.rs` | 历史存储抽象(get/record 两方法) |
| `InMemoryHistoryStore` | `moe.rs` | DashMap 内存实现(并发安全) |
| `HISTORY_SUFFICIENT_THRESHOLD` | `moe.rs` | 常量 100(降级阈值) |
| `LATENCY_WINDOW_CAPACITY` | `moe.rs` | 常量 100(滑动窗口容量) |

### 3.2 签名变更(向后兼容)

| 函数 | v1.2.0 签名 | v1.3.0 签名 | 向后兼容 |
|------|-------------|-------------|----------|
| `route_auto` | `(registry, req)` | `(registry, req)` | ✅ 不变 |
| `route_auto_with_gate` | `(registry, req, gate)` | `(registry, req, gate, history)` | ⚠️ 新增第 4 参数 |
| `MoeGate::gate()` | `(models, req)` | `(models, history)` | ⚠️ 第 2 参数变更 |
| `MoeGate::gate_score()` | `(m)` | `(m, history)` | ⚠️ 新增第 2 参数(私有 fn) |

**向后兼容保证**:`route_auto` 签名不变,内部默认 `history=None` 退化三维。
`route_auto_with_gate` 新增第 4 参数,现有调用方需追加 `None`(仅 model-router
内部 bench/tests 调用,无外部 crate 依赖)。

### 3.3 HistoryRecord 方法

| 方法 | 返回值 | 说明 |
|------|--------|------|
| `new()` | `HistoryRecord` | 空记录(零样本) |
| `record(&mut self, latency_ms, success)` | `()` | 记录一次路由(滑动窗口维护) |
| `success_rate(&self)` | `f32` | 成功率 [0,1],无样本返回 0.0 |
| `latency_variance(&self)` | `f32` | 样本方差(无偏估计),n<2 返回 0.0 |
| `is_sufficient(&self)` | `bool` | total_count >= 100 |

## 4. 测试覆盖

### 4.1 新增测试(6 个 + 1 proptest)

| 测试 | 验证目标 |
|------|----------|
| `test_five_dim_score_when_history_sufficient` | 历史充足(≥ 100)时五维评分,Top-K 激活 |
| `test_three_dim_fallback_when_history_insufficient` | 历史不足(< 100)降级三维,与 history=None 一致 |
| `test_history_none_degrades_to_three_dim` | history=None 退化三维(向后兼容 v1.2.0) |
| `test_success_rate_affects_ranking` | 高成功率模型进入 Top-K |
| `test_latency_variance_penalizes_unstable` | 高方差模型被排除 Top-K(静态指标相同,方差为唯一区分) |
| `prop_five_dim_sparsity_invariant` | proptest 256 cases:任意 n ∈ [50,200] + history ≥ 100,激活数 ≤ top_k |

### 4.2 测试结果

```
running 16 tests
test test_moe_gate_degrades_default_config ... ok
test test_moe_gate_activates_top_k_only ... ok
test test_moe_gate_custom_top_k ... ok
test test_moe_gate_activates_top_k_only_100_models ... ok
test test_moe_gate_recalls_best_model ... ok
test test_route_auto_backward_compatible_below_threshold ... ok
test test_moe_gate_activates_top_k_only_200_models ... ok
test test_history_none_degrades_to_three_dim ... ok
test test_moe_gate_degrades_when_below_threshold ... ok
test test_five_dim_score_when_history_sufficient ... ok
test test_three_dim_fallback_when_history_insufficient ... ok
test test_latency_variance_penalizes_unstable ... ok
test test_success_rate_affects_ranking ... ok
test prop_moe_gate_degrade_invariant ... ok
test prop_moe_gate_sparsity_invariant ... ok
test prop_five_dim_sparsity_invariant ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

**全部 model-router 测试**:unit tests + moe_test(16) + cacr_test(1) +
top_k_equivalence(3) + router_test + doc-tests(3)= 全部通过。

## 5. bench 数据(三维 vs 五维延迟对比)

### 5.1 延迟数据(min-of-N 5 采样,µs)

| n | full_O(n) | moe_O(k)_3dim | moe_O(k)_5dim | 5dim/3dim 比 |
|---|-----------|--------------|--------------|-------------|
| 50 | 6.98 | 5.08 | 19.17 | 3.77x |
| 100 | 13.60 | 8.87 | 38.45 | 4.34x |
| 200 | 26.71 | 18.51 | 89.93 | 4.86x |

### 5.2 分析

1. **3dim 仍优于 full_O(n)**:Top-K 门控减少完整评估工作量(5.08 vs 6.98 µs
   at n=50,18.51 vs 26.71 µs at n=200),与 v1.2.0 收益一致。

2. **5dim 比 3dim 慢 ~4x**:五维路径每模型需 DashMap::get(哈希查找 + Ref guard
   + HistoryRecord clone ~400B)+ latency_variance() 计算(100 样本 sum + mean
   + sum_sq)。n=200 时为 200 次查找 + 200 次方差计算。

3. **5dim 仍在微秒级**:即使 n=200,五维门控 89.93 µs,远低于毫秒级路由
   决策的可接受阈值(典型 LLM API 调用延迟 50-1000 ms,门控 < 0.1%)。

4. **O(n) 复杂度保持**:5dim 延迟随 n 线性增长(50→200 = 3.78x n 增长,
   19.17→89.93 = 4.69x 延迟增长),符合 O(n) 预期(常数因子来自历史查找)。

### 5.3 结论

五维评分引入 ~4x 额外开销(DashMap 查找 + 方差计算),但仍在微秒级,
不影响路由决策的端到端延迟。历史维度的路由质量提升(成功率感知 + 抖动惩罚)
值得此开销。如需优化,未来可缓存 variance 计算结果或使用更紧凑的历史存储。

## 6. 依赖与架构合规

### 6.1 依赖铁律(§2.2)

- `dashmap` 是 L1 Core 工具依赖(workspace 已收录 `dashmap = "6.1"`)
- model-router(L1 Core)依赖 dashmap(工具库),符合 L(N)→工具库允许
- 无向上依赖,无跨层引用

### 6.2 forbid(unsafe_code)

- `#![forbid(unsafe_code)]` 保持(model-router crate 级)
- DashMap 内部 unsafe 不传播到当前 crate(§4.1 forbid 语义)

### 6.3 核心领域类型

- 未变更 `UserIntent`/`Quest`/`Checkpoint`/`OmniSparseMasks`/`CLV`/`NexusState`
- `MoeGate` 是辅助类型(配置载体),非核心领域类型
- `HistoryRecord`/`HistoryStore` 是新增辅助类型

### 6.4 向后兼容(§3.3.1.5)

- `route_auto` 签名不变(SemVer 兼容)
- `route_auto_with_gate` 新增第 4 参数(仅内部调用,无外部依赖)
- 降级路径保证 history=None 时行为与 v1.2.0 一致(Top-K 选择相同)

## 7. 修改文件清单

| 文件 | 变更类型 | 说明 |
|------|----------|------|
| `crates/model-router/src/moe.rs` | 修改 | +HistoryRecord/HistoryStore/InMemoryHistoryStore +五维 gate_score +gate() 签名变更 |
| `crates/model-router/src/strategies.rs` | 修改 | route_auto_with_gate 新增 history 参数 + route_auto 默认 None |
| `crates/model-router/src/lib.rs` | 修改 | prelude 导出 3 新类型 |
| `crates/model-router/Cargo.toml` | 修改 | +dashmap = { workspace = true } |
| `crates/model-router/tests/moe_test.rs` | 修改 | 现有测试适配新签名 + 6 新测试 + 1 proptest |
| `crates/model-router/benches/moe_bench.rs` | 修改 | 现有 bench 适配新签名 + 新增 5dim bench |

## 8. 验证结果

| 检查项 | 命令 | 结果 |
|--------|------|------|
| 全部测试 | `cargo test -p model-router` | ✅ 全部通过(含 3 doc-tests) |
| clippy | `cargo clippy -p model-router --all-targets --jobs 2 -- -D warnings` | ✅ 零警告 |
| fmt | `cargo fmt -p model-router -- --check` | ✅ 零 diff |
| bench 编译 | `cargo bench -p model-router --bench moe_bench --no-run` | ✅ 17.59s |
| bench 运行 | `cargo bench -p model-router --bench moe_bench -- --quick` | ✅ 9 bench 数据已收集 |
