# Phase II 正确性修复验证报告 (v1.1.0-omega)

> **范围**:v1.1.0-omega 系统化优化 Phase II 的三项 P0 正确性修复(N2 / N3 / A1)的 TDD 测试补齐与生产代码核验。
> **验证日期**:2026-07-09
> **执行人**:Aether CLI Team
> **原则**:仅验证与文档化,不修改生产逻辑;所有代码片段均取自当前实际实现,行号引用真实文件位置。

---

## Phase II 概述

Phase II 聚焦三项 P0 正确性 bug:

| 编号 | 严重度 | 目标 crate | bug 描述 |
|------|--------|-----------|---------|
| N2 | P0 | ssra-fusion | `select_nth_unstable_by` 后取 `selected[0]` 作为主导策略,但该函数不保证 `[0]` 是最大值 |
| N3 | P0 | qeep-protocol | 三元组协议(Request→Ack→Receipt)中 Ack 从未被创建,实际仅实现二元组 |
| A1 | P0 | quest-engine | `checkpoint.rs` 的 `save()`/`load()`/`load_latest()`/`prune_old()` 使用同步 `fs::write`/`fs::read`,直接阻塞 Tokio worker 线程 |

### 实施方式说明

根据 `tasks.md` 的"实施前核实(2026-07-09)"记录,DEEP_RESEARCH 报告(2026-07-08)之后的代码演进已在生产代码层解决 N2/N3/A1。因此 Phase II 调整为"补齐缺失的 TDD 测试 + 辅助函数抽象",**不重写已修复的生产代码**。本报告核验生产代码修复的真实性与测试覆盖的完整性。

### 与任务描述的路径差异(如实记录)

任务描述中提及的文件路径与实际修复位置存在差异,以实际代码为权威:

| 任务描述路径 | 实际修复路径 | 说明 |
|------------|------------|------|
| N2:`kvbsr-router/src/router.rs`、`sesa-router/src/activation.rs` | `ssra-fusion/src/fusion/engine.rs` | 实际 N2 bug 位于 ssra-fusion 的主导策略选取,DEEP_RESEARCH 报告 N2 明确指向 ssra-fusion |
| A1:`scc-cache/`、`cmt-tiering/`、`mlc-engine/` 等 | `quest-engine/src/checkpoint.rs` | 实际 A1 阻塞点位于 quest-engine 的检查点持久化,DEEP_RESEARCH 报告 A1 明确指向 quest-engine/checkpoint.rs |
| N3:`qeep-protocol/src/protocol.rs` | `qeep-protocol/src/types.rs` + `protocol.rs` | 一致,Ack 类型定义在 types.rs,创建逻辑在 protocol.rs |

> 上述路径以 `tasks.md` "实施前核实"章节与 `DEEP_RESEARCH_34CRATE_OPTIMIZATION.md` N2/N3/A1 条目为权威源,任务描述中的候选路径仅为搜索提示。

---

## 1. Task II-1:N2 `select_nth_unstable_by` 主导策略选取语义修复

### 1.1 问题根因

`ssra-fusion` 的 `select_top_k_desc` 使用 `select_nth_unstable_by` 选 Top-K 模板策略,随后误用 `selected[0]` 作为"主导策略"(权重最大者)。

`select_nth_unstable_by(slice, k, cmp)` 的语义保证:
- `slice[k-1]` 是第 k 大的元素(pivot);
- `slice[0..k]` 中所有元素都 ≥ pivot;
- **但不保证 `slice[0]` 是最大值**——`slice[0..k]` 内部是无序的。

因此取 `selected[0]` 作为主导策略,在某些输入下会选到非最大权重的策略,导致融合置信度计算错误。

此外,`partial_cmp` 在 NaN 输入下返回 `None`,若用 `unwrap()` 会触发 panic。必须使用 `unwrap_or(Ordering::Equal)` 安全降级。

### 1.2 修复方案

修复位于 [engine.rs](file:///D:/Chimera%20CLI/crates/ssra-fusion/src/fusion/engine.rs),提取独立可测函数 `pick_max_weight`,在 `select_nth_unstable_by` 后用 `max_by` 显式选取真正最大权重。

**修复后 — Top-K 降序选择**([engine.rs:220-233](file:///D:/Chimera%20CLI/crates/ssra-fusion/src/fusion/engine.rs#L220-L233)):

```rust
/// Top-K 降序选择 — 使用 `select_nth_unstable_by` 实现 O(n) 平均复杂度
///
/// 返回前 `k` 个权重最大的元素(未完全排序,但保证是最大的 K 个)。
fn select_top_k_desc(metas: &mut [(f32, FusionStrategy)], k: usize) -> &[(f32, FusionStrategy)] {
    if k >= metas.len() {
        return metas;
    }
    let idx = k - 1;
    // 降序:b.0 vs a.0(大的在前)
    metas.select_nth_unstable_by(idx, |a, b| {
        b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
    });
    &metas[..k]
}
```

**修复后 — 主导策略显式取最大**([engine.rs:209-218](file:///D:/Chimera%20CLI/crates/ssra-fusion/src/fusion/engine.rs#L209-L218)):

```rust
/// 从 `(weight, strategy)` 切片中挑选权重最大的元素
///
/// WHY: 单独抽出可测函数,便于回归测试验证 `select_nth_unstable_by` 后
/// 必须显式取最大,而不是误用 `selected[0]`。
/// f32 使用 `partial_cmp` 并在 NaN 时回退到 `Equal`,与现有排序语义一致。
fn pick_max_weight(metas: &[(f32, FusionStrategy)]) -> Option<&(f32, FusionStrategy)> {
    metas
        .iter()
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
}
```

**主流程调用点**([engine.rs:138-141](file:///D:/Chimera%20CLI/crates/ssra-fusion/src/fusion/engine.rs#L138-L141)):`fuse_inner` 在 `select_top_k_desc` 后调用 `pick_max_weight(selected)` 选取主导策略,而非 `selected[0]`。

### 1.3 新增测试清单

| 测试文件 | 测试名 | 行号 | 覆盖点 |
|---------|--------|------|--------|
| [strategy_proptest.rs](file:///D:/Chimera%20CLI/crates/ssra-fusion/tests/strategy_proptest.rs) | `prop_main_strategy_always_max` | L69 | 128 次随机权重向量,验证主导策略置信度 == 独立计算的最大权重策略置信度 |
| [engine.rs](file:///D:/Chimera%20CLI/crates/ssra-fusion/src/fusion/engine.rs) | `test_pick_max_weight_single_element` | L586 | 单元素边界 |
| [engine.rs](file:///D:/Chimera%20CLI/crates/ssra-fusion/src/fusion/engine.rs) | `test_pick_max_weight_all_nan` | L594 | 全 NaN 输入不 panic |
| [engine.rs](file:///D:/Chimera%20CLI/crates/ssra-fusion/src/fusion/engine.rs) | `test_pick_max_weight_nan_mixed` | L606 | NaN 与有效值混合,有效最大值不被 NaN 覆盖 |
| [engine.rs](file:///D:/Chimera%20CLI/crates/ssra-fusion/src/fusion/engine.rs) | `test_select_top_k_desc_empty` | L619 | 空切片边界 |
| [engine.rs](file:///D:/Chimera%20CLI/crates/ssra-fusion/src/fusion/engine.rs) | `test_select_top_k_desc_dominant_is_true_max` | L626 | 回归核心:分区后 `[0]` 非最大时,`pick_max_weight` 仍取真正最大 |

### 1.4 WHY 注释要点

- [engine.rs:211-213](file:///D:/Chimera%20CLI/crates/ssra-fusion/src/fusion/engine.rs#L211-L213):说明 `pick_max_weight` 单独抽出的原因 —— 便于回归测试验证 `select_nth_unstable_by` 后必须显式取最大,而非误用 `selected[0]`;并说明 f32 在 NaN 时回退到 `Equal` 与现有排序语义一致。
- [strategy_proptest.rs:3-4](file:///D:/Chimera%20CLI/crates/ssra-fusion/tests/strategy_proptest.rs#L3-L4):回归目标注释 —— 验证 `fuse_inner` 在 `select_top_k_desc` 后通过显式 `max_by` 挑选最大权重,而非 `selected[0]`。

---

## 2. Task II-2:N3 QEEP 三元组 Ack 状态机修复

### 2.1 问题根因

QEEP 协议设计为三元组 Request→Ack→Receipt,但原实现中 Ack 从未被创建,实际仅实现二元组(Request→Receipt 直接跳转)。这导致:
- 调用者无法区分"future 未启动"与"future 已 poll 但未完成";
- 零孤儿调用保证缺少可观测的中间状态点;
- 状态机缺少 Acknowledged 阶段,无法支撑 Ack 缺失场景的重试/降级。

### 2.2 修复方案

修复分布在 `qeep-protocol` 的 `types.rs`(类型定义)与 `protocol.rs`(创建逻辑)。

**Ack 类型定义**([types.rs:40-50](file:///D:/Chimera%20CLI/crates/qeep-protocol/src/types.rs#L40-L50)):

```rust
/// 确认回执,表示执行单元已接收请求并开始处理。
///
/// 对应 QEEP 三元组的第二个环节:执行单元 → 调用者。
/// Ack 的存在证明 future 已被 poll,不再是"未启动"状态。
#[derive(Debug, Clone)]
pub struct Ack {
    /// 对应的调用 ID
    pub id: EntangledCallId,
    /// 确认时间
    pub acknowledged_at: DateTime<Utc>,
}
```

**调用状态机**([types.rs:66-83](file:///D:/Chimera%20CLI/crates/qeep-protocol/src/types.rs#L66-L83)):

```rust
/// 调用状态机
///
/// 描述纠缠调用从创建到终结的生命周期。
/// 状态转移:Pending → Acknowledged → Completed
///                 ↘ Timeout / Failed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    Pending,        // 已创建但未确认(等待 Ack)
    Acknowledged,   // 已确认,执行中(已收到 Ack)
    Completed,      // 已完成(已收到 Receipt,成功)
    Timeout,        // 超时
    Failed,         // 失败
}
```

**Ack 创建点**([protocol.rs:118-143](file:///D:/Chimera%20CLI/crates/qeep-protocol/src/protocol.rs#L118-L143)):`entangle()` 在注册 Request(Pending)后、执行 future 前创建 Ack 并转入 Acknowledged:

```rust
// 步骤 1:注册到 pending_calls
self.inner.pending_calls.insert(
    id,
    CallRecord {
        id,
        state: CallState::Pending,
        created_at,
        completed_at: None,
        ack: None,
    },
);

// 步骤 1.5:创建 Ack 并进入 Acknowledged 状态
// WHY: QEEP 三元组要求 Request → Ack → Receipt;Ack 表示 future
// 已被接收并即将 poll,是零孤儿调用保证的关键可观测点。
let ack = Ack {
    id,
    acknowledged_at: Utc::now(),
};
if let Some(mut entry) = self.inner.pending_calls.get_mut(&id) {
    entry.state = CallState::Acknowledged;
    entry.ack = Some(ack);
}
```

`CallRecord` 持有 `ack: Option<Ack>`([protocol.rs:61-62](file:///D:/Chimera%20CLI/crates/qeep-protocol/src/protocol.rs#L61-L62)),通过 `call_ack(id)` 公开查询。

### 2.3 新增测试清单

| 测试文件 | 测试名 | 行号 | 覆盖点 |
|---------|--------|------|--------|
| [protocol_test.rs](file:///D:/Chimera%20CLI/crates/qeep-protocol/tests/protocol_test.rs) | `test_full_triplet_request_ack_receipt` | L27 | 完整三元组生命周期:Pending→Acknowledged→Completed,Ack.id 一致,完成后记录清理 |
| [protocol_test.rs](file:///D:/Chimera%20CLI/crates/qeep-protocol/tests/protocol_test.rs) | `test_ack_missing_blocks_receipt` | L86 | 不变量:Ack 是到达 Receipt 的必要条件,轮询验证 Ack 必先于 Completed 出现 |

### 2.4 WHY 注释要点

- [protocol.rs:134-135](file:///D:/Chimera%20CLI/crates/qeep-protocol/src/protocol.rs#L134-L135):QEEP 三元组要求 Request→Ack→Receipt;Ack 表示 future 已被接收并即将 poll,是零孤儿调用保证的关键可观测点。
- [types.rs:40-43](file:///D:/Chimera%20CLI/crates/qeep-protocol/src/types.rs#L40-L43):Ack 的存在证明 future 已被 poll,不再是"未启动"状态。
- [types.rs:66-70](file:///D:/Chimera%20CLI/crates/qeep-protocol/src/types.rs#L66-L70):状态转移图 Pending→Acknowledged→Completed(↘Timeout/Failed)。

### 2.5 与 spec 描述的命名差异(如实记录)

`tasks.md` 中 Task II-2.5 提及引入 `TripletState` 状态机(`AwaitingAck` / `Acked` / `Receipted`)。实际实现采用 `CallState`(`Pending` / `Acknowledged` / `Completed` / `Timeout` / `Failed`)。

两者功能等价(都建模了 Ack 前置的必要性),但实际实现的 `CallState` 额外覆盖了 `Timeout`/`Failed` 终结态,语义更完整。本报告以实际代码为权威,不视为缺陷。

---

## 3. Task II-3:A1 Checkpoint `spawn_blocking` 包装修复

### 3.1 问题根因

`quest-engine/src/checkpoint.rs` 的四个方法 `save()`/`load()`/`load_latest()`/`prune_old()` 原使用同步 `fs::write`/`fs::read`,在 async 上下文中直接调用会阻塞 Tokio worker 线程。检查点操作涉及:
- MessagePack 序列化/反序列化;
- SHA-256 校验计算;
- 多次磁盘 I/O(`load_latest` 最差情况需 `max_keep` 次文件读取 + 完整性校验);

累计可达数十毫秒,在高并发下会耗尽 worker 线程导致 runtime 饥饿。

### 3.2 修复方案

四个方法改为 `async fn`,内部用 `tokio::task::spawn_blocking` 将阻塞操作移到专用阻塞线程池。阻塞逻辑提取为独立静态函数 `*_blocking`,避免 `spawn_blocking` 闭包捕获 `&self` 引发借用冲突。

**save() — 异步 + spawn_blocking**([checkpoint.rs:64-97](file:///D:/Chimera%20CLI/crates/quest-engine/src/checkpoint.rs#L64-L97)):

```rust
/// 保存检查点 — 序列化 Quest 为 MessagePack,写入磁盘(异步)
///
/// 内部通过 `spawn_blocking` 将磁盘 I/O + MessagePack 序列化 + SHA-256 校验
/// 放到阻塞线程池执行,避免阻塞 tokio worker 线程。
pub async fn save(&self, quest: &Quest) -> Result<Checkpoint, QuestError> {
    // UUIDv7 生成 + Quest 序列化是纯 CPU,在 async 上下文完成
    // (避免 clone Quest 到 spawn_blocking)
    let serialized_state = rmp_serde::to_vec(quest)...;
    // ... 构造 Checkpoint ...

    // 阻塞操作:创建目录 + 写文件 + prune_old 移到 spawn_blocking
    let dir = self.checkpoint_dir.clone();
    let max_keep = self.max_keep;
    tokio::task::spawn_blocking(move || Self::save_blocking(checkpoint, &dir, max_keep))
        .await
        .map_err(|e| QuestError::CheckpointSaveFailed(format!("spawn_blocking join: {e}")))?
}
```

四个方法对应的阻塞实现:

| 异步方法 | spawn_blocking 调用行 | 阻塞静态函数 |
|---------|----------------------|------------|
| `save()` | [checkpoint.rs:94](file:///D:/Chimera%20CLI/crates/quest-engine/src/checkpoint.rs#L94) | `save_blocking` ([L103](file:///D:/Chimera%20CLI/crates/quest-engine/src/checkpoint.rs#L103)) |
| `load()` | [checkpoint.rs:158](file:///D:/Chimera%20CLI/crates/quest-engine/src/checkpoint.rs#L158) | `load_blocking` |
| `load_latest()` | [checkpoint.rs:190](file:///D:/Chimera%20CLI/crates/quest-engine/src/checkpoint.rs#L190) | `load_latest_blocking` |
| `prune_old()` | [checkpoint.rs:262](file:///D:/Chimera%20CLI/crates/quest-engine/src/checkpoint.rs#L262) | `prune_old_blocking` ([L270](file:///D:/Chimera%20CLI/crates/quest-engine/src/checkpoint.rs#L270)) |

### 3.3 辅助函数形式差异(如实记录)

`tasks.md` Task II-3.5 提及提取泛型辅助函数 `spawn_blocking_io<F, T>(f: F) -> Result<T>`。实际实现未采用单一泛型包装,而是为每个方法提取独立静态函数 `*_blocking`。

WHY 注释([checkpoint.rs:101-102](file:///D:/Chimera%20CLI/crates/quest-engine/src/checkpoint.rs#L101-L102))说明:`spawn_blocking` 要求闭包 `Send + 'static`,将所需参数显式传入独立静态函数可避免捕获 `&self` 引发借用冲突。这是比单一泛型包装更安全的选择(各阻塞函数签名明确,无类型擦除),功能等价,不视为缺陷。

### 3.4 新增测试清单

| 测试文件 | 测试名 | 行号 | 覆盖点 |
|---------|--------|------|--------|
| [checkpoint.rs](file:///D:/Chimera%20CLI/crates/quest-engine/tests/checkpoint.rs) | `test_save_load_not_blocking_runtime` | L574 | multi_thread runtime(2 workers),save 期间轻量任务在 100ms 内完成,证明 runtime 未阻塞 |
| [checkpoint.rs](file:///D:/Chimera%20CLI/crates/quest-engine/tests/checkpoint.rs) | `test_load_latest_not_blocking_runtime` | L601 | load_latest 期间轻量任务在 100ms 内完成 |
| [checkpoint.rs](file:///D:/Chimera%20CLI/crates/quest-engine/tests/checkpoint.rs) | `test_concurrent_save_load_correctness` | L632 | 4 workers 并发 save/load 无数据丢失或 ID 冲突 |
| [checkpoint.rs](file:///D:/Chimera%20CLI/crates/quest-engine/tests/checkpoint.rs) | `test_checkpoint_save_load_roundtrip` | L79 | save→load 往返一致性(基线回归) |

### 3.5 WHY 注释要点

- [checkpoint.rs:15-17](file:///D:/Chimera%20CLI/crates/quest-engine/src/checkpoint.rs#L15-L17)(模块级):所有磁盘操作通过 `spawn_blocking` 封装,避免阻塞 tokio worker 线程;save/load 涉及多次文件 I/O + MessagePack 序列化 + SHA-256 校验,累计可达数十毫秒。
- [checkpoint.rs:66-67](file:///D:/Chimera%20CLI/crates/quest-engine/src/checkpoint.rs#L66-L67):`save` 内部通过 `spawn_blocking` 将磁盘 I/O + 序列化 + 校验放到阻塞线程池。
- [checkpoint.rs:101-102](file:///D:/Chimera%20CLI/crates/quest-engine/src/checkpoint.rs#L101-L102):独立静态函数的原因 —— `spawn_blocking` 要求闭包 `Send + 'static`,显式传参避免 `&self` 借用冲突。
- [checkpoint.rs:149-150](file:///D:/Chimera%20CLI/crates/quest-engine/src/checkpoint.rs#L149-L150)、[checkpoint.rs:184-185](file:///D:/Chimera%20CLI/crates/quest-engine/src/checkpoint.rs#L184-L185):`load`/`load_latest` 同理,`load_latest` 最差情况(max_keep 次文件读取)必须移出 worker 线程。

---

## 4. 与预期不符的发现汇总

| 项 | 预期(spec/tasks.md 描述) | 实际实现 | 评估 |
|----|------------------------|---------|------|
| N2 修复路径 | kvbsr-router / sesa-router | ssra-fusion/src/fusion/engine.rs | 路径以 DEEP_RESEARCH N2 条目为准,ssra-fusion 为权威 |
| N2 辅助函数名 | `pick_max_weight()` | `pick_max_weight()` | 一致 |
| N3 状态机名 | `TripletState`(AwaitingAck/Acked/Receipted) | `CallState`(Pending/Acknowledged/Completed/Timeout/Failed) | 功能等价,实际命名更完整(含 Timeout/Failed 终结态) |
| A1 修复路径 | scc-cache / cmt-tiering / mlc-engine | quest-engine/src/checkpoint.rs | 路径以 DEEP_RESEARCH A1 条目为准,quest-engine 为权威 |
| A1 辅助函数 | 泛型 `spawn_blocking_io<F,T>` | 独立静态函数 `*_blocking` | 功能等价,独立静态函数避免借用冲突,更安全 |
| 测试增量门槛 | ≥ 9(N2 proptest + N3 三元组 + A1 异步验证) | N2: 1 proptest + 5 单元;N3: 2 集成;A1: 3 异步 + 1 基线 = 12 | 超出门槛 |

所有差异均不构成缺陷:路径以 DEEP_RESEARCH 报告为权威源,命名/辅助函数形式以实际代码为准且功能等价或更优。

---

## 5. Phase II 验证结果

### 5.1 验证命令与结果(2026-07-09)

Phase II 修复的测试已包含在 Phase III 验收时的全量测试中(Phase II/III 同日完成,生产代码先于测试落地)。引用 2026-07-09 的验证日志:

| 验证项 | 命令 | 结果 |
|--------|------|------|
| 全量测试 | `cargo test --workspace --jobs 1` | exit 0,3249 passed / 0 failed / 55 ignored |
| Lint | `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings`(`RUST_MIN_STACK=33554432` + `CARGO_INCREMENTAL=0`) | exit 0,零警告 |
| 格式 | `cargo fmt --all -- --check` | exit 0,零 diff |

### 5.2 单 crate 测试核验

| crate | 命令 | 关键测试 |
|-------|------|---------|
| ssra-fusion | `cargo test -p ssra-fusion` + `cargo test -p ssra-fusion --test strategy_proptest` | `prop_main_strategy_always_max`(128 cases 全过)+ 5 个 `pick_max_weight` 单元测试 |
| qeep-protocol | `cargo test -p qeep-protocol` | `test_full_triplet_request_ack_receipt` + `test_ack_missing_blocks_receipt` + proptest 状态机完备性 |
| quest-engine | `cargo test -p quest-engine` + `cargo check --workspace` | `test_save_load_not_blocking_runtime` + `test_load_latest_not_blocking_runtime` + `test_concurrent_save_load_correctness`;调用点 `.await` 全部更新,workspace 编译通过 |

### 5.3 测试增量

Phase II 新增测试共 12 项(N2: 1 proptest + 5 单元;N3: 2 集成;A1: 3 异步 + 1 基线回归),超出验收门槛 ≥ 9 的要求。

### 5.4 红线合规

- `#![forbid(unsafe_code)]`:三个 crate 均在 crate 级声明,核验通过;
- async 反模式:无持锁跨 `.await`、`spawn_blocking` 正确包装同步 I/O、无 fire-and-forget 关键路径;
- Top-K 选择使用 `select_nth_unstable_by`(O(n)),`partial_cmp` 用 `unwrap_or(Equal)` 安全降级。

---

## 6. 补充核验:scc-cache LRU(N10)归属与代码核验

### 6.1 任务描述与实际归档的编号映射差异

本任务描述将 `scc-cache` 马尔可夫链 LRU 标注为 **Phase II Task II-1(N2)**,但实际归档体系中该修复归属如下(以 `tasks.md` / `CHANGELOG.md` / `project_memory.md` 为权威源):

| 维度 | 任务描述标注 | 实际归档(tasks.md / CHANGELOG) |
|------|------------|---------------------------|
| 缺陷编号 | N2 | **N10** |
| 阶段归属 | Phase II Task II-1 | **Phase III Task III-3** |
| 目标文件 | `crates/scc-cache/src/prefetch.rs` | 一致 |

> 说明:任务描述的编号映射存在错位 —— 其中的 N2 实际指 `ssra-fusion` 主导策略 bug(本报告第 1 章),A1 实际指 `quest-engine` checkpoint spawn_blocking(本报告第 3 章),N3 指本报告第 2 章 QEEP Ack。本报告以 `tasks.md` / `CHANGELOG.md` 的实际归档为权威,保持 Phase II 主体三章(N2/N3/A1)结构不变;因任务描述要求核验 scc-cache LRU 代码,本节给出核验结论与归属说明。

### 6.2 代码核验结论

经核验,LRU 修复已落地于 [prefetch.rs](file:///D:/Chimera%20CLI/crates/scc-cache/src/prefetch.rs):

- [prefetch.rs:39](file:///D:/Chimera%20CLI/crates/scc-cache/src/prefetch.rs#L39):`DEFAULT_PATTERN_CAPACITY = 10_000` 容量上限常量
- [prefetch.rs:41-55](file:///D:/Chimera%20CLI/crates/scc-cache/src/prefetch.rs#L41-L55):`LruNode` — 基于 Vec 索引的无 unsafe 双向链表节点(`prev` / `next` 为 `Option<usize>`)
- [prefetch.rs:65-217](file:///D:/Chimera%20CLI/crates/scc-cache/src/prefetch.rs#L65-L217):`LruPatternMap` — 容量受限的 LRU 访问模式图,含 `record_transition` / `move_to_tail` / `evict_lru` / `alloc_node` 等 O(1) 操作
- [prefetch.rs:260-266](file:///D:/Chimera%20CLI/crates/scc-cache/src/prefetch.rs#L260-L266):`AccessPatternLearner::with_capacity` 显式容量构造函数
- 测试:[prefetch_test.rs](file:///D:/Chimera%20CLI/crates/scc-cache/tests/prefetch_test.rs) 3 个测试(容量上限 / LRU 顺序更新 / capacity=1 边界)

修复前反模式(马尔可夫链转移矩阵 `patterns: RwLock<HashMap<...>>` 无容量上限,长期运行导致内存无限增长)已在 Phase III Task III-3 修复并归档。

WHY 注释([prefetch.rs:42-46](file:///D:/Chimera%20CLI/crates/scc-cache/src/prefetch.rs#L42-L46))说明不用 `std::collections::LinkedList` 的原因:其 Cursor API 在 Rust 2021 不稳定,无法在不使用 unsafe 指针的情况下 O(1) 移动节点;用 Vec 索引 + prev/next 可在 `#![forbid(unsafe_code)]` 约束下实现真正的 O(1) LRU 维护。

### 6.3 不重复归档说明

`scc-cache` LRU(N10)的完整文档归档已在 Phase III 完成,本报告不重复其内容:
- `CHANGELOG.md` Phase III 章节第 III-3 条
- `project_memory.md` 第 157 行 "Phase III LRU 无 unsafe 实现"
- `docs/optimization/v1.1.0/phase3_performance_verification_report.md`

---

## 7. 关联文档

- `CHANGELOG.md` — Phase II 正确性修复章节
- `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md` — Phase II 教训
- `.trae\specs\v1-1-0-systematic-optimization-deep-analysis\tasks.md` — Task II-1/II-2/II-3/II-4
- `.trae\specs\v1-1-0-systematic-optimization-deep-analysis\checklist.md` — Phase II 验收项
- `DEEP_RESEARCH_34CRATE_OPTIMIZATION.md` — N2/N3/A1 原始 bug 报告
