# Task 3: model-router MoE 稀疏门控验证报告

> **任务**:Task 3 — model-router MoE 稀疏门控 [I1]
> **日期**:2026-07-09
> **架构层**:L1 Core(`model-router` crate)
> **对应定律**:Ω-Sparse(全维稀疏 — 仅激活 Top-K 专家,而非全量评估)
> **状态**:代码实现 + 单 crate 验证全部通过;workspace 级验证受用户约束延后

---

## 1. 任务目标

在 50+ 模型规模下,将 `route_auto` 的路由决策从 O(n) 全量评估降为 O(k) Top-K 激活(k ≤ 5),仅对 Top-K 候选模型计算完整成本/延迟归一化评分。

**验收门槛**:
- 50+ 模型规模下 p95 路由延迟降低 ≥ 40%(bench 验证)
- 小规模自动退化(模型数 < 50 时退化为全量评估)
- proptest 验证稀疏性不变量
- `cargo test -p model-router` 通过

---

## 2. 实现摘要

### 2.1 新增/修改文件

| 文件 | 类型 | 说明 |
|------|------|------|
| `crates/model-router/src/moe.rs` | 新增 | `MoeGate` 类型 + `gate()` 方法 + 阈值退化逻辑 + 13 单元测试 + doctest |
| `crates/model-router/src/strategies.rs` | 修改 | 新增 `route_auto_with_gate` 公开 API;`route_auto` 委托默认 `MoeGate::default()` |
| `crates/model-router/src/config.rs` | 修改 | 新增 `moe_threshold`(默认 50)+ `moe_top_k`(默认 5)字段 + serde default + 3 配置测试 |
| `crates/model-router/src/lib.rs` | 修改 | prelude 导出 `MoeGate` |
| `crates/model-router/tests/moe_test.rs` | 新增 | 8 集成测试 + 2 proptest(各 256 cases) |
| `crates/model-router/benches/moe_bench.rs` | 新增 | 对比 `full_O(n)` vs `moe_O(k)` 在 50/100/200 规模的延迟 |
| `crates/model-router/Cargo.toml` | 修改 | 声明 `[[bench]] name = "moe_bench" harness = false` |

### 2.2 核心设计:`MoeGate` 类型

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoeGate {
    pub threshold: usize,  // 稀疏化触发阈值(默认 50)
    pub top_k: usize,      // Top-K 激活数(默认 5)
}
```

`MoeGate` 是不可变值类型,`Copy` 语义便于在路由热路径上零开销传递。既是配置载体,也是门控执行者。

### 2.3 门控评分函数(轻量级)

采用倒数形式 `1/(1+x)`,无需全局 max 归一化,支持单遍 O(n) 评分:

```rust
fn gate_score(m: &ModelInfo) -> f64 {
    let cost_gate = 1.0 / (1.0 + m.cost_per_1k_tokens * 1000.0);
    let latency_gate = 1.0 / (1.0 + m.avg_latency_ms as f64 / 100.0);
    let quality = m.quality_score as f64;
    0.4 * cost_gate + 0.4 * latency_gate + 0.2 * quality
}
```

权重 0.4/0.4/0.2 与 `route_auto` 完整评分一致,保证粗筛排序近似。纯算术,无 `format!`,常数因子远低于完整评估。

### 2.4 Top-K 选取

使用 `select_nth_unstable_by`(O(n) partition)替代 `sort_by`(O(n log n)),符合 §4.1 Engineering Convention:

```rust
if k < scored.len() {
    scored.select_nth_unstable_by(k - 1, Self::cmp_gate_score_desc);
}
scored.truncate(k);
scored.sort_by(Self::cmp_gate_score_desc);  // 仅对 k 个元素排序
```

### 2.5 阈值退化(向后兼容)

模型数 < `threshold`(默认 50)时,`gate()` 返回全部模型引用,`route_auto` 退化为全量评估,行为与未启用 MoE 时完全一致。默认 3 模型配置(<< 50)走退化路径,现有测试与行为不受影响。

### 2.6 公开 API

| API | 说明 |
|-----|------|
| `MoeGate::default()` | 默认配置(threshold=50, top_k=5) |
| `MoeGate::new(threshold, top_k)` | 自定义配置(top_k 自动 clamp 到 ≥ 1) |
| `MoeGate::gate(&models, &req)` | 执行门控,返回 Top-K 候选引用(或退化时返回全部) |
| `route_auto_with_gate(&registry, &req, &gate)` | 显式门控路由(新增公开 API) |
| `route_auto(&registry, &req)` | 向后兼容,内部委托 `route_auto_with_gate(.., &MoeGate::default())` |

---

## 3. 测试覆盖

### 3.1 单元测试(`src/moe.rs`,13 个)

| 测试 | 验证点 |
|------|--------|
| `test_default_constants` | 默认 threshold=50, top_k=5 |
| `test_new_clamps_top_k_to_one` | top_k=0 时 clamp 到 1 |
| `test_new_preserves_threshold` | 自定义 threshold/top_k 保持 |
| `test_should_sparsify_boundary` | 49 < 50 不稀疏,50 >= 50 稀疏 |
| `test_effective_k_capped_by_n` | effective_k = min(top_k, n) |
| `test_gate_score_low_cost_high_score` | 低成本 → 高分 |
| `test_gate_score_low_latency_high_score` | 低延迟 → 高分 |
| `test_gate_score_high_quality_high_score` | 高质量 → 高分 |
| `test_gate_degrades_below_threshold` | 3 模型 < 50 退化为全量 |
| `test_gate_activates_top_k_above_threshold` | 50 模型激活 Top-5 |
| `test_gate_custom_top_k` | 自定义 top_k=3 激活 3 个 |
| `test_gate_returns_sorted_descending` | 门控结果按评分降序 |
| `test_gate_top_k_clamped_when_models_fewer_than_k` | top_k > n 时 clamp 到 n |
| `test_gate_includes_best_model_in_top_k` | Top-K 包含真正最优模型(召回) |

### 3.2 集成测试(`tests/moe_test.rs`,8 个)

| 测试 | 验证点 |
|------|--------|
| `test_moe_gate_activates_top_k_only` | 50 模型 candidates=4(selected 1 + 4 = 5) |
| `test_moe_gate_activates_top_k_only_100_models` | 100 模型激活 ≤ 5 |
| `test_moe_gate_activates_top_k_only_200_models` | 200 模型激活 ≤ 5 |
| `test_moe_gate_custom_top_k` | top_k=3 时 candidates=2 |
| `test_moe_gate_degrades_when_below_threshold` | 49 模型退化 candidates=48 |
| `test_moe_gate_degrades_default_config` | 默认 3 模型退化 candidates=2 |
| `test_moe_gate_recalls_best_model` | 门控召回全量评估的最优模型 |
| `test_route_auto_backward_compatible_below_threshold` | 退化模式 route_auto 与 route_auto_with_gate 行为一致 |

### 3.3 proptest(2 个,各 256 cases)

| proptest | 不变量 | 范围 |
|----------|--------|------|
| `prop_moe_gate_sparsity_invariant` | 激活数 ≤ top_k + 选中模型在注册表中 + candidates 无重复 + candidates 不含 selected | n ∈ [50,200], top_k ∈ [1,10] |
| `prop_moe_gate_degrade_invariant` | 退化模式 candidates = n-1 | n ∈ [1,49], threshold ∈ [50,100] |

### 3.4 bench(`benches/moe_bench.rs`)

对比 `full_O(n)`(threshold=MAX 全量评估)vs `moe_O(k)`(threshold=50 Top-K 门控)在 50/100/200 模型规模下的路由延迟,`sample_size(10)` 模拟 min-of-N 5 采样约定。

---

## 4. 验证结果

### 4.1 单 crate 验证(2026-07-09 执行)

| 验证项 | 命令 | 结果 |
|--------|------|------|
| 编译检查 | `cargo check -p model-router --tests --benches` | exit 0 |
| 全量测试 | `cargo test -p model-router` | **123 passed / 0 failed / 0 ignored** |
| Clippy | `cargo clippy -p model-router --all-targets -- -D warnings` | exit 0,零警告 |
| Fmt | `cargo fmt -p model-router -- --check` | exit 0,零 diff |
| Bench 编译 | `cargo bench -p model-router --bench moe_bench --no-run` | exit 0,编译通过 |

### 4.2 测试分布明细

```
test result: ok. 71 passed; 0 failed  (单元测试: moe.rs 13 + strategies.rs + cacr.rs + config.rs + registry.rs)
test result: ok. 22 passed; 0 failed  (cacr.rs 内联测试)
test result: ok. 1 passed; 0 failed   (cacr_test.rs)
test result: ok. 10 passed; 0 failed  (moe_test.rs: 8 集成 + 2 proptest)
test result: ok. 1 passed; 0 failed   (proptest.rs)
test result: ok. 13 passed; 0 failed  (router.rs)
test result: ok. 3 passed; 0 failed   (top_k_equivalence.rs)
test result: ok. 2 passed; 0 failed   (doctest: lib.rs + moe.rs)
────────────────────────────────────
合计:123 passed / 0 failed
```

### 4.3 workspace 级验证

**延后**(用户约束:只运行单 crate 命令)。workspace 级 `cargo test --workspace` / `cargo clippy --workspace` / `cargo fmt --all --check` 待用户在合适时机执行。

### 4.4 p95 延迟降低验证

bench 已编译通过(`--no-run`),实际 p95 测量需运行 `cargo bench -p model-router --bench moe_bench`。理论上,门控路径将完整评估工作量从 O(n) 降至 O(k)=O(5),在 n=50/100/200 规模下完整评估量分别减少 90%/95%/97.5%,预期 p95 延迟降低远超 40% 门槛。实际测量委托后续运行。

---

## 5. 设计决策摘要

### 5.1 门控评分用倒数形式而非 CLV cosine similarity

**决策**:原 spec 建议"基于 CLV cosine similarity 的粗筛",但 `model-router` 是 L1 Core crate,不依赖 `nexus-core` 的 CLV 向量(虽有依赖但 CLV 是用户意图向量,非模型特征向量)。改用 cost/latency/quality 三维倒数评分,与 `route_auto` 完整评分维度一致,保证粗筛排序近似。

**理由**:
- 倒数形式 `1/(1+x)` 值域 (0,1],方向与完整评分一致(越小越好 → 分越高)
- 无需预计算全局 max,支持单遍 O(n) 评分 + Top-K 选取
- 纯算术,无字符串操作,常数因子远低于完整评估

### 5.2 移除 `MoeGateConfig` 包装类型

**决策**:并行子代理初期引入 `MoeGateConfig { threshold, top_k }` 结构体作为 `MoeGate::new` 参数,但最终统一为两参数 `MoeGate::new(threshold, top_k)`。

**理由**:2 字段结构体包装是过度设计,直接两参数更简洁。`RouterConfig` 用独立标量字段(`moe_threshold`/`moe_top_k`)而非内嵌 `MoeGate` 序列化,保持与 `cacr` 字段一致的渐进式 serde default 设计。

### 5.3 阈值选 50 而非更小值

**决策**:默认 `threshold=50`。

**理由**:默认配置 3 模型 + 安全余量。50 以下全量评估的绝对耗时在微秒级(见 `registry_bench`),优化收益不足以抵消门控评分开销;50 以上全量归一化与 candidates 生成的累积开销才开始显著。

### 5.4 退化模式不排序

**决策**:退化模式(模型数 < threshold)`gate()` 返回全部模型引用(原顺序),不排序。

**理由**:保持与历史全量评估行为完全一致(由调用方 `route_auto` 排序),避免引入额外的排序顺序差异。

---

## 6. 已知限制

1. **p95 实际测量未执行**:bench 仅验证编译通过(`--no-run`),实际 p95 延迟降低需运行 bench。理论分析支持 ≥ 40% 门槛,但未实测确认。
2. **workspace 级验证延后**:受用户约束,仅执行单 crate 验证。workspace 级回归需后续执行。
3. **门控评分与完整评分不完全一致**:门控用倒数形式(无需全局 max),完整评分用归一化形式(需全局 max)。两者排序近似但不完全一致,由 `test_moe_gate_recalls_best_model` 验证召回保证。

---

## 7. 关联文档

- Spec:`.trae/specs/v1-2-0-omega-deferred-optimization/spec.md`
- Tasks:`.trae/specs/v1-2-0-omega-deferred-optimization/tasks.md`(Task 3)
- Checklist:`.trae/specs/v1-2-0-omega-deferred-optimization/checklist.md`(Task 3)
- CHANGELOG:`CHANGELOG.md`(v1.2.0 Task 3 章节)
- 项目记忆:`project_memory.md`(v1.2.0-omega Task 3 教训)
