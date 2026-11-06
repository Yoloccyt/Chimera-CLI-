# ADR-241 -- 工具链并行化深化：异步探针 IO、借用化派发契约、无锁读候选的**否决**

- 状态：Accepted（2026-11-06）
- 决策者：工具链并行化重构第三波（W3）
- 关联：ADR-240（四环链生产接线）、ADR-239（性能阈值基线与棘轮）、ADR-238（强制沙箱派发链）
- 影响面：`crates/chimera-cli/src/commands/doctor/probes.rs`、`crates/chimera-cli/src/commands/doctor.rs`、`crates/chimera-cli/tests/doctor_async_probe.rs`（新）、`crates/nexus-contracts/src/spec/tool_dispatch.rs`、`crates/gqep-executor/src/tool_runtime/{dispatch,chain,runtime,sandbox_gate,mod}.rs`、`crates/gqep-executor/tests/tool_runtime_borrowed_equivalence.rs`（新）、`crates/gqep-executor/benches/tool_dispatch_layers.rs`

## 1. 背景与问题

ADR-240 把四环链接入组合根（安全前置真实生效），ADR-239 建立了性能判据。本波处理三个剩余问题：

| 问题 | 实测证据 | 后果 |
|------|----------|------|
| doctor 探针内同步 FS | `probes.rs` 的 `check_sqlite_path` 在 `async fn` 内直接调 `std::fs::write` / `std::fs::remove_file`；全仓扫描 `std::fs::\|std::thread::sleep\|block_on` 仅此 2 处命中 | 该探针经 `doctor.rs` 的 `gather_collected` 与其他探针并发执行 —— 同步 FS **阻塞所在 Tokio worker**，抵消并发收益 |
| 热路径每次派发 2 次堆分配 | `DispatchRequest { tool_name: String, args_json: String }` 是拥有型；`runtime.rs` 的 `execute()` 每次 `to_string()` 构造新请求 | 批量派发 N 条即 2N 次纯浪费的分配（拥有型数据派发后即丢弃） |
| 沙箱门缓存的锁争用（**假设**） | `sandbox_gate.rs` 的 `tool_policies: RwLock<HashMap<...>>`；读锁是原子读改写 | 批量并发下可能构成串行化点 —— **但这是假设，未经验证** |

## 2. 决策

### 2.1 D1：doctor 探针改用 `tokio::fs`

`check_sqlite_path` 的两处同步文件 IO 改为 `tokio::fs::write` / `tokio::fs::remove_file`（`.await`）。

**WHY 不 `spawn_blocking`**：`tokio::fs` 内部已是「异步 API + 阻塞池执行」，语义与手写 `spawn_blocking` 等价但更简洁，且不需在调用方手写 `JoinHandle` 管理。

**清理失败不改变判定**：`remove_file` 的错误仍被忽略（`let _ = ...`）—— 清理失败不影响「目录可写」这一判定本身，语义与改动前逐字一致。

### 2.2 D2：借用化派发契约 —— L0 新增 `DispatchRequestRef<'a>`

L0 契约（`nexus_contracts::spec::tool_dispatch`）新增：

```rust
pub struct DispatchRequestRef<'a> {
    pub tool_name: &'a str,
    pub args_json: &'a str,
}
```

**WHY 与拥有型并存而非替换**：拥有型 `DispatchRequest` 是 L0 的**边界类型** —— 能跨 `.await` 持有、能跨层传递、能存进集合，是公开契约的一部分。替换它是 BREAKING（违反「不削减现有功能」）。本视图作为**增量契约**提供，既有签名零改动。

**大小判据**：`size_of::<DispatchRequestRef>() == 2 * size_of::<&str>()` —— 无隐藏分配、无填充。由 `nexus-contracts` 与集成测试双重锁定。

### 2.3 D3：借用路径与拥有路径走**同一套四环**

`SandboxedDispatcher::dispatch_borrowed` 的实现只做**入参形态**的适配（借用 → 拥有一次），之后调 `head.dispatch(&owned)` —— 与 `dispatch` 走**完全相同**的策略层/沙箱层/路由层/传输层实例。

**WHY 不另建一条借用链**：另建链意味着四层各有两个实现 —— 行为可能分叉，借用路径就成了一条**绕过安全校验的旁路**。这比不优化严重得多。

**代价**：入站处一次「借用 → 拥有」转换（2 次分配）。
**收益**：调用方（热路径上的批量编排）**不再为每次派发构造 `DispatchRequest`** —— 那 2 次分配被消除，且调用方可直接复用同一份 `args_json` 缓冲。

### 2.4 D4：沙箱门新增 `check_ref`，判决等价是硬约束

`SandboxGate` trait 新增：

```rust
fn check_ref(&self, req: &DispatchRequestRef<'_>) -> Result<(), DispatchError> {
    self.check(&req.to_owned_request())   // 默认实现：语义等价，保留分配
}
```

`SeccoreSandboxGate` **覆写**该方法走零分配路径（省去 `check` 内的 2 次 clone）。

**判决等价是硬约束**：对同一工具名与参数，`check` 与 `check_ref` 的结果必须完全相同（含错误变体与文案）。由 property test `borrowed_sandbox_verdict_matches_owned` 锁定（64 cases）。

**WHY 这条不能只靠端到端结果间接证明**：若两者都放行，端到端结果也相同，但拦截能力已经退化。必须**独立**断言沙箱判决一致。

### 2.5 D5：无锁读候选 —— **已评估、不采纳**（关键裁决）

按 ADR-239 D-8「每个并行化改造点须有同轮对照基准，收益不显著则回退」，本波对沙箱门缓存的无锁读（`DashMap` 分片锁替代 `RwLock` 单锁）做了同轮对照。

**对照设计**：`tool_dispatch_layers.rs` 内联复刻两种实现（判决完全等价 —— 同一套拦截模式 + 同一份派生策略，只有并发读路径不同），在**多线程并发读**下同轮测量。单线程微基准看不出差异（读锁几乎无争用），只有并发读才暴露 cacheline 争用。

**实测结果**（2026-11-06，criterion sample_size=30）：

| 测点 | `RwLock`（现状） | `DashMap`（候选） | 差异 |
|------|-----------------|------------------|------|
| 4 线程并发读 | 242.72 µs | 294.86 µs | 候选**慢 21.5%** |
| 8 线程并发读 | 476.87 µs | 545.60 µs | 候选**慢 14.4%** |

**裁决：回退，保留 `RwLock`。**

**WHY 候选更慢**：`RwLock` 的读锁在本场景下几乎无争用 —— 沙箱门的读临界区极短（一次 `HashMap` 查找 + `Arc::clone`），锁持有时间远小于线程调度开销。而 `DashMap` 的分片哈希**每次读都要算一次 hash 并定位分片**，这个固定成本在短临界区场景下超过了锁的争用成本。

**这一裁决的价值**：若无对照基准就改，会引入一个**确定更慢**的实现 + 一条新生产依赖（供应链面扩大）。**「不采纳」是本次重构中与「采纳」同等重要的结论** —— 它证明改造决策有客观依据，而非凭直觉。

**附带处置**：`dashmap` **不**进生产依赖（保持 dev-dependency 仅供对照基准使用），供应链面不扩大。

### 2.6 D6：借用化的实际收益边界（诚实记录）

本波的借用化**只消除了调用方的请求构造成本**（每条派发 2 次分配），**没有**消除链内的分配：

| 分配点 | 借用化后 | 说明 |
|--------|---------|------|
| 调用方构造 `DispatchRequest` | **消除** | 改用 `DispatchRequestRef` |
| 入站「借用 → 拥有」转换 | 新增 1 次（2 次分配） | D3 的代价 |
| `InProcDispatcher` 派发 | 无分配 | 原本就借用 |
| `SandboxGate::check` 的 `Command` 构造 | 仍有 2 次 clone | `seccore::validate_command` 的签名要求拥有型 |

**净效果**：调用方原本的 2 次分配被「入站转换的 2 次」抵消 —— **热路径分配的净减少为 0**。

**WHY 仍保留该改造**：借用化建立了 L0 的**增量契约**，使后续「链内全借用化」有确定的接口落点（`check_ref` 已是零分配的）；同时调用方可复用同一份参数缓冲（多次派发同一工具时）。但**必须诚实记录净收益为 0** —— 这不是一次有效的性能优化，是一次**接口准备**。

**后续路径**：若要真正消除链内分配，需 `seccore::validate_command` 提供借用入参变体（其内部 `build_full_command_string` 本身就要构造字符串 —— 收益上限受此约束）。这属 `seccore` 的独立变更，本波不涉及。

## 3. 后果

### 3.1 正向

- doctor 并发探针不再阻塞 Tokio worker（同步 FS 消除）。
- L0 获得借用化派发契约，既有 `ToolDispatcher` 签名零改动（兼容性保障）。
- 借用路径与拥有路径的**逐位等价**由 property test 锁定（含沙箱判决的独立断言）。
- 无锁读候选的否决有客观数据支撑，避免了一次确定更慢的改动。

### 3.2 负向与代价

- `SandboxGate` trait 新增一个方法 —— 属**加性**变更，既有实现无需改动（默认实现兜底）。
- L0 契约新增一个公开类型 —— 属**加性**变更（ADR-239 §7.4 已预告此类需弃用期管理，但加性类型不破坏既有签名）。
- 借用化的**净性能收益为 0**（见 D6）—— 诚实记录。

### 3.3 待办（明确留给后续批次）

1. **生产 `ToolExecutor` 实现仍缺** —— 传输环仍未接线（ADR-240 §3.3）。
2. **策略规则的声明式来源仍缺** —— 需新增配置段。
3. **并行归因指标**（P-13）—— 在线可观测的批量指标尚未接入 `nexus-telemetry`。
4. **链内全借用化** —— 需 `seccore::validate_command` 的借用入参变体。

## 4. 验收证据（2026-11-06 实测）

```
cargo test -p chimera-cli --test doctor_async_probe
  test sqlite_path_probe_has_no_sync_fs_calls ... ok
  test sqlite_path_probe_behavior_unchanged ... ok
  test sqlite_path_probe_does_not_block_worker ... ok
  test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

cargo test -p gqep-executor --test tool_runtime_borrowed_equivalence
  test dispatch_request_ref_has_no_hidden_allocation ... ok
  test dispatch_request_ref_conversion_is_field_exact ... ok
  test borrowed_dispatch_preserves_sandbox_short_circuit ... ok
  test borrowed_sandbox_verdict_matches_owned ... ok
  test borrowed_dispatch_matches_owned_dispatch ... ok
  test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

cargo test -p gqep-executor --test tool_runtime_batch_production_invariants
  test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

cargo bench -p gqep-executor --bench tool_dispatch_layers -- dispatch_sandbox_concurrent
  dispatch_sandbox_concurrent/rwlock_read_c4   time: [239.12 µs 242.72 µs 246.74 µs]
  dispatch_sandbox_concurrent/dashmap_read_c4  time: [276.82 µs 294.86 µs 311.64 µs]
  dispatch_sandbox_concurrent/rwlock_read_c8   time: [459.23 µs 476.87 µs 494.29 µs]
  dispatch_sandbox_concurrent/dashmap_read_c8  time: [495.38 µs 545.60 µs 599.18 µs]
```

## 5. 失效条件

1. 若 `SandboxGate::check_ref` 的判决与 `check` 分叉，`borrowed_sandbox_verdict_matches_owned` 会红。
2. 若 `dispatch_borrowed` 绕过四环链，`borrowed_dispatch_matches_owned_dispatch` 会红。
3. 若借用路径的短路顺序被改变（先执行再校验），`borrowed_dispatch_preserves_sandbox_short_circuit` 会红。
4. 若 `check_sqlite_path` 回到同步 IO，`sqlite_path_probe_has_no_sync_fs_calls` 会红。
5. 若 `DispatchRequestRef` 引入隐藏分配，`dispatch_request_ref_has_no_hidden_allocation` 会红。
6. 若未来有人以「DashMap 更快」为由重提该改造，本 ADR §2.5 的同轮数据即是反驳依据 —— 除非能给出**优于本表**的同轮对照结果。
