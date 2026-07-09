# Task 1: V-10 测试覆盖补齐验证报告

> **阶段**: v1.2.0-omega 第二阶段开发 — Task 1 测试基础设施补齐
> **日期**: 2026-07-09
> **Spec**: `v1-2-0-omega-deferred-optimization`
> **执行方式**: 多路并行子代理(Agent A benches/proptest + Agent B proptest/fuzz)+ 质量验证子代理整体验证归档
> **基线**: v1.1.0 Phase V 完成时 3228 passed / 0 failed / 55 ignored
> **结果**: Task 1 完成 3339 passed / 0 failed / 56 ignored(增量 +111 passed / +1 ignored)

---

## 1. 概述

### 1.1 Task 1 目标

补齐 Phase V 延后的 V-10 测试覆盖配套任务,涵盖四个维度:

1. **criterion benches** — 5 crate 性能基准(延迟 + 吞吐量双维度)
2. **proptest** — 5 crate 不变量属性测试
3. **doctest** — 3 crate 模块级 `# 快速示例` 补齐
4. **fuzz** — fuzz target 从 3 扩展到 6

### 1.2 完成状态

| SubTask | 维度 | 交付物 | 状态 |
|---------|------|--------|------|
| 1.1 | criterion benches | 5 crate × 2 维度(延迟 + 吞吐量) | ✅ 完成 |
| 1.2 | proptest | 5 crate × 8 invariants | ✅ 完成 |
| 1.3 | doctest | 3 crate 模块级示例 | ✅ 完成 |
| 1.4 | fuzz 3→6 | 3 新增 target | ✅ 完成 |
| 1.5 | 整体验证与归档 | fmt / clippy / test + 报告 + CHANGELOG + memory | ✅ 完成 |

---

## 2. 交付物清单

### 2.1 SubTask 1.1: criterion benches(5 crate)

| Crate | bench 文件 | 维度 |
|-------|-----------|------|
| event-bus | `benches/bus_bench.rs` | 延迟(publish 单次) + 吞吐量(并发 publish events/sec) |
| acb-governor | `benches/governor_bench.rs` | 四级延迟(L0-L3 tier switch) + 并发吞吐 |
| decay-engine | `benches/decay_bench.rs` | 单步衰减延迟 + 批量衰减吞吐 |
| qeep-protocol | `benches/protocol_bench.rs` | entangle 延迟 + 批量 entangle 吞吐 |
| auto-dpo | `benches/dpo_bench.rs` | 更新延迟 + 批量更新吞吐 |

**设计原则**:
- 每个 bench 含 2 个维度(延迟 + 吞吐量),延迟用单次操作,吞吐量用并发/批量
- `Throughput::Elements` 让 criterion 报告 events/sec
- 全部编译通过,clippy 零警告

### 2.2 SubTask 1.2: proptest(5 crate,8 invariants)

| Crate | 文件 | invariants × cases |
|-------|------|---------------------|
| acb-governor | `tests/proptest.rs` | 3 × 64(级别递增 / 预算不超限 / degrade/upgrade 单调) |
| model-router | `tests/proptest.rs` | 1 × 32(CACR 预算一致性) |
| repo-wiki | `tests/proptest.rs` | 1 × 32(KNN 返回最近 k 个) |
| sesa-router | `tests/proptest.rs` | 2 × 32(稀疏比不变量 + 裁剪满足约束) |
| gea-activator | `tests/proptest.rs` | 1 × 32(激活幂等性) |

**关键设计**:
- gea-activator `activate()` 是 async,proptest 宏内无法直接 `.await`,用 `tokio::runtime::Builder::new_current_timestamp()` + `block_on()` 包裹
- repo-wiki proptest 中 `String` 不实现 `Copy`,需用 `&actual.0` 引用比较而非值比较(E0507)

### 2.3 SubTask 1.3: doctest 补齐(3 crate)

| Crate | 补齐内容 |
|-------|---------|
| qeep-protocol | 模块级 `# 快速示例`(entangle 三元组基本用法) |
| decay-engine | 模块级 `# 快速示例`(衰减引擎基本用法) |
| chimera-cli | 模块级 `# 快速示例`(CLI 入口基本用法) |

**验证**:`cargo test --doc --workspace` 34 crate 全部通过

### 2.4 SubTask 1.4: fuzz 3→6 target

| 新增 target | 文件 | 模糊测试对象 |
|-------------|------|-------------|
| cacr_budget_parse | `fuzz/fuzz_targets/cacr_budget_parse.rs` | `CacrConfig` 预算解析 |
| checkpoint_deserialize | `fuzz/fuzz_targets/checkpoint_deserialize.rs` | `Checkpoint` MessagePack 反序列化 |
| config_section_parse | `fuzz/fuzz_targets/config_section_parse.rs` | `ChimeraConfig` section 解析 |

**配置**:`fuzz/Cargo.toml` 含 6 个 `[[bin]]`,Rust 源码静态验证通过
**平台限制**:libFuzzer 的 `FuzzerExtFunctionsWindows.cpp` 仅适配 MSVC,MinGW g++ 无法解析 C++ 符号,Windows GNU-only 环境无法实际执行,委托 Linux CI(§10.3)

---

## 3. 验证结果

### 3.1 代码质量验证

| 验证项 | 命令 | 结果 |
|--------|------|------|
| 格式检查 | `cargo fmt --all -- --check` | ✅ exit 0,零 diff |
| Lint 检查 | `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` | ✅ exit 0,零警告(`RUST_MIN_STACK=33554432` + `CARGO_INCREMENTAL=0`) |
| 全量测试 | `cargo test --workspace --jobs 1` | ✅ exit 0,3339 passed / 0 failed / 56 ignored |

### 3.2 测试增量统计

| 阶段 | passed | failed | ignored | 增量 |
|------|--------|--------|---------|------|
| v1.0.0-omega GA | 3002 | 0 | — | — |
| v1.1.0 Phase I-V 完成 | 3228 | 0 | 55 | +226 passed / +2 ignored |
| **v1.2.0 Task 1 完成** | **3339** | **0** | **56** | **+111 passed / +1 ignored** |

> **新基线**:v1.2.0 Task 1 完成后测试基线为 **3339 passed / 0 failed / 56 ignored**(总计 3395 测试用例)。
> **门槛达成**:测试总数 3339 ≥ 期望门槛 3248,超出 91。

### 3.3 测试统计汇总方法

从 `cargo test --workspace --jobs 1` 输出(写入 `tmp/task1_test_run.log`)提取所有 `test result: ok. N passed; M failed; K ignored` 行:
- 测试结果行数:214
- 总通过(passed 累加):3339
- 总失败(failed 累加):0
- 总忽略(ignored 累加):56
- FAILED 行数:0
- 编译错误 `error[E\d+]:`:0
- panic `panicked at`:0

---

## 4. 预存 bug 修复(非 Task 1 交付物,阻塞验证)

### 4.1 问题

执行 `cargo clippy --workspace --all-targets -- -D warnings` 时发现 Phase V V-3 引入的测试文件 `crates/gqep-executor/tests/gatherer_test.rs` L130 编译失败:

```
error[E0597]: `i` does not live long enough
   --> crates\gqep-executor\tests\gatherer_test.rs:132:33
    |
129 |         .map(|i| {
    |               - binding `i` declared here
130 |             Box::pin(async {
    |                      ----- value captured here by coroutine
131 |                 tokio::time::sleep(Duration::from_millis(200)).await;
132 |                 Ok(format!("ok-{i}"))
    |                 ^ borrowed value does not live long enough
133 |             }) as GqepFuture<String>
    |                   ------------------ type annotation requires that `i` is borrowed for `'static`
```

### 4.2 根因

`GqepFuture<String>` 要求 `'static` 生命周期,但 `async { ... }` block 默认按引用捕获外部变量 `i`(range 迭代变量),闭包返回的 `Pin<Box<dyn Future>>` 跨越 `.map()` 调用后 `i` 即被 drop,触发 E0597。

### 4.3 修复

为 `async` block 添加 `move` 关键字,让 `i`(Copy 类型 `u32`)按值捕获:

```rust
.map(|i| {
    // move 捕获 i:GqepFuture 要求 'static,async block 默认引用捕获会触发 E0597
    Box::pin(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(format!("ok-{i}"))
    }) as GqepFuture<String>
})
```

同文件 L91(`test_gather_global_deadline_not_triggered`)已正确使用 `async move`,L44(`|_|` 不捕获变量)与 L178(不捕获外部变量)无需修改。

### 4.4 影响评估

- 此 bug 是 Phase V V-3 提交时的遗漏(可能因当时 `cargo check` 跳过该测试 target 的某些组合)
- 修复仅 1 行改动 + 1 行注释,符合 §3.1 RC 阶段"仅允许 bugfix"原则
- 修复后 clippy 全 workspace 零警告通过

---

## 5. 约束遵守确认

### 5.1 `#![forbid(unsafe_code)]` 合规

- 所有新增 bench/proptest/doctest/fuzz 文件均含 `#![forbid(unsafe_code)]`(crate 级属性已传播)
- prometheus-client / rusqlite bundled 内部 unsafe 不影响当前 crate 源码(§4.1 规则)
- Task 1 新增代码零 unsafe 引入

### 5.2 依赖铁律(L(N)→L(N-1) 允许,L(N)→L(N+1) 禁止)

- Task 1 为测试基础设施补齐,所有新增文件位于 `benches/` / `tests/` / `fuzz/` 目录
- dev-dependencies 可绕过生产依赖方向(§2.2 例外),仅限 `tests/` 目录
- 无生产代码修改,无向上依赖引入

### 5.3 OMEGA 四定律对齐

| 定律 | Task 1 实践 |
|------|------------|
| Ω-Sparse | benches 度量稀疏化开销;proptest 验证稀疏比不变量(sesa-router) |
| Ω-Compress | benches 度量压缩吞吐量;fuzz 验证反序列化鲁棒性(checkpoint_deserialize) |
| Ω-Evolve | proptest 不变量固化演进约束;benches 提供演进性能基线 |
| Ω-Event | event-bus bench 度量事件发布延迟/吞吐;fuzz 验证事件序列化(event_serialize 已有) |

### 5.4 TDD 守恒

- Task 1 为测试基础设施补齐(非功能实现),无生产代码删除
- 新增测试全部通过,零已有测试删除
- 修复的预存 bug(gqep-executor/tests/gatherer_test.rs)是 Phase V V-3 测试遗漏,修复后测试通过

### 5.5 RC 阶段规则(§3.1)

- Task 1 属 v1.2.0-omega 第二阶段开发,不适用 RC 阶段"仅 bugfix"约束
- 但本次唯一的生产代码无关改动(gqep-executor 测试文件 bugfix)符合 RC bugfix 原则
- 未引入新 crate,未变更核心领域类型

---

## 6. 关联文档

- `docs/optimization/v1.1.0/phase5_progressive_optimization_report.md` — Phase V 主报告(V-10 延后到 v1.2.0)
- `docs/optimization/v1.2.0/task0_desensitization_report.md` — Task 0 脱敏化处理
- `.trae/specs/v1-2-0-omega-deferred-optimization/` — v1.2.0 spec 定义
- `CHANGELOG.md` — v1.2.0 Task 1 章节
- `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md` — Task 1 教训归档

---

## 7. 修改文件清单

### 7.1 Task 1 新增文件(SubTask 1.1-1.4)

| 文件 | SubTask | 说明 |
|------|---------|------|
| `crates/event-bus/benches/bus_bench.rs` | 1.1 | event-bus 延迟 + 吞吐 bench |
| `crates/acb-governor/benches/governor_bench.rs` | 1.1 | acb-governor 四级延迟 + 并发 bench |
| `crates/decay-engine/benches/decay_bench.rs` | 1.1 | decay-engine 单步 + 批量 bench |
| `crates/qeep-protocol/benches/protocol_bench.rs` | 1.1 | qeep-protocol entangle bench |
| `crates/auto-dpo/benches/dpo_bench.rs` | 1.1 | auto-dpo 更新 bench |
| `crates/acb-governor/tests/proptest.rs` | 1.2 | acb-governor 3 invariants |
| `crates/model-router/tests/proptest.rs` | 1.2 | model-router CACR 一致性 |
| `crates/repo-wiki/tests/proptest.rs` | 1.2 | repo-wiki KNN 不变量 |
| `crates/sesa-router/tests/proptest.rs` | 1.2 | sesa-router 稀疏比 + 裁剪 |
| `crates/gea-activator/tests/proptest.rs` | 1.2 | gea-activator 幂等性 |
| `fuzz/fuzz_targets/cacr_budget_parse.rs` | 1.4 | CacrConfig 解析 fuzz |
| `fuzz/fuzz_targets/checkpoint_deserialize.rs` | 1.4 | Checkpoint 反序列化 fuzz |
| `fuzz/fuzz_targets/config_section_parse.rs` | 1.4 | ChimeraConfig section fuzz |

### 7.2 SubTask 1.3 doctest 补齐(修改文件)

| 文件 | 说明 |
|------|------|
| `crates/qeep-protocol/src/lib.rs` | 模块级 `# 快速示例` |
| `crates/decay-engine/src/lib.rs` | 模块级 `# 快速示例` |
| `crates/chimera-cli/src/lib.rs` | 模块级 `# 快速示例` |

### 7.3 SubTask 1.5 整体验证(修改文件)

| 文件 | 说明 |
|------|------|
| `crates/gqep-executor/tests/gatherer_test.rs` | 修复 Phase V V-3 遗漏的 `async move` 缺失(L130) |
| `CHANGELOG.md` | 追加 v1.2.0 Task 1 章节 |
| `docs/optimization/v1.2.0/task1_test_coverage_report.md` | 本报告(新建) |
| `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md` | 追加 Task 1 教训 |

### 7.4 配置文件更新

| 文件 | 说明 |
|------|------|
| `fuzz/Cargo.toml` | 新增 3 个 `[[bin]]` 声明(fuzz target 3→6) |
| 各 crate `Cargo.toml` | 新增 `[[bench]]` 声明 + dev-dependencies(criterion / proptest) |

---

## 8. 结论

Task 1 V-10 测试覆盖补齐完成全部 4 个 SubTask,全量验证通过:

- ✅ `cargo fmt --all -- --check` exit 0
- ✅ `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` exit 0(零警告,修复 1 处预存 E0597)
- ✅ `cargo test --workspace --jobs 1` exit 0,3339 passed / 0 failed / 56 ignored

**测试增量**: +111 passed / +1 ignored(从 Phase V 基线 3228 → 3339)

**门槛达成**: 测试总数 3339 ≥ 期望 3248,超出 91

**约束遵守**: `#![forbid(unsafe_code)]` / 依赖铁律 / OMEGA 四定律 / TDD 守恒全部满足

**预存 bug 修复**: Phase V V-3 测试文件 `gqep-executor/tests/gatherer_test.rs` L130 `async` block 缺 `move` 关键字导致 E0597 编译失败,本次验证发现并修复(1 行改动 + 1 行注释),解除 clippy 阻塞。

Task 1 验收通过,可继续 v1.2.0-omega 后续 Task 推进。
