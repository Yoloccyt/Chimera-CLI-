# ADR-270 -- 取消通道的生产者接线：进程级取消根 + 单个 Ctrl+C 监听

- 状态：Accepted（2026-11-09）
- 决策者：R6 / W4
- 关联：ADR-268/269（同批 CLI 面改动）、ADR-242 §3.5（安全面不提供降级开关 —— 本批**新增**的是取消能力，非降级面）、ADR-120/127/199（唯一派发入口，本批不新建）
- 影响面：`crates/chimera-cli/src/call_budget.rs`（新增 `cancel_root()`）、`crates/chimera-cli/src/commands/exec.rs`、`crates/chimera-cli/src/commands/run.rs`（两处）、新增 `crates/chimera-cli/tests/cancel_root.rs`
- 行为影响：默认路径**新增**「Ctrl+C 可取消在飞调用」；不取消时与今日逐字相同

## 1. 背景：消费者齐备，生产者为零

`call_budget::call_with_budget` 的取消语义是完整的：

- 预先取消 ⇒ 返回 `Cancelled` 且 future **一次都不轮询**（避免早退副作用）；
- 在飞取消 ⇒ `tokio::select!` 立即返回 `Cancelled`，future 被 Drop；
- 专用错误变体 `CallBudgetError::Cancelled`（与 `Timeout` 区分，不发超时事件）。

但生产侧**从未触发过取消**：

- `exec.rs`、`run.rs`（**两处**）各自 `CancellationToken::new()` 后把句柄交给
  `call_with_budget`，注释自陈「新建未触发令牌，预留全局取消链路接线」；
- 全仓 `.cancel()` 的 5 处命中全部落在 `call_budget.rs` 的 `#[cfg(test)]` 模块内。

⇒ 取消分支在生产上是死代码：Ctrl+C 只在 permission prompt 内被处理，
`exec --plan` / `run --execute` 执行期间**没有任何路径能中断在飞调用**。

## 2. 决策

### D1：进程级取消根（`OnceLock`）+ 恒 1 个监听 task

```rust
static CANCEL_ROOT: OnceLock<CancellationToken> = OnceLock::new();
pub fn cancel_root() -> &'static CancellationToken {
    CANCEL_ROOT.get_or_init(|| {
        let token = CancellationToken::new();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let t = token.clone();
            handle.spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() { t.cancel(); }
            });
        }
        token
    })
}
```

**WHY 进程级单例**：用户按 Ctrl+C 的语义是「停掉当前前台操作」，不是「停掉某一次
工具调用」。若每个命令各建一个令牌，Ctrl+C 只能命中其中一个 —— 单例保证
「所有在飞的可取消调用看的是同一个信号」。

**WHY 恒 1 个 task**：监听器只注册一次（`OnceLock` 初始化时），不随命令数增长，
不违本仓的无界 spawn 禁令。它是**信号源**，不是新的派发/调度面。

**WHY 无 runtime 时静默跳过**：纯同步上下文（部分单测）挂不了 task；此时令牌仍是
「新建且永不触发」的既有行为 —— fail-safe，不 panic、不改变语义。

### D2：三个生产入口改用根令牌

`exec.rs`（`exec.create_quest`）与 `run.rs` 的**两处**（`run.execute_batch`、
`run.create_quest`）把 `CancellationToken::new()` 换成
`crate::call_budget::cancel_root().clone()`。令牌是 `Clone` 且共享同一取消状态，
故 clone 后取消根即取消全部使用者。

`run.rs` 的两处都改：只改一处会留下「一半可取消、一半不可取消」的半接线状态，
比全不改更难推理。

### D3：为什么这不是「被禁的平行调度器 / 第二派发入口」

- 不新增任何 `dispatch` / `route` / `ComputeBridge` 入口；
- 不新增 crate、不新增 feature；
- spawn 的 task 数恒 1（信号监听），与「层内无上限 spawn」的禁令无关；
- 复用**已存在**的消费者（`call_with_budget` 的 `select!` 分支），属 ADR-211 §6.4
  「接线批必须指名它兑现了哪个现有机制的消费者」的正面案例。

### D4：不发新事件

取消复用既有 `CallBudgetError::Cancelled`（调用方按既有错误映射处理），
**不新增** `NexusEvent` 变体 ⇒ 不触 ADR-259 D9 的「六锁同批」纪律。

## 3. 验收（现跑）

| 判据 | 要求 |
|---|---|
| 单例身份 | `cancel_root()` 两次调用返回同一地址 |
| 源码结构锁 | `exec.rs` / `run.rs` **不含** `CancellationToken::new()`，且含 `cancel_root()` |
| 取消传播 | 取消根后，在飞 `call_with_budget`（60s 预算 + 30s sleep）在 2s 内返回 `Cancelled` |
| 既有语义 | `call_budget.rs` 既有测试零修改全绿 |

**为什么需要结构锁**：`tokio::signal::ctrl_c()` 的真实触发在测试进程里无法可靠模拟
（会与测试框架的信号处理竞争）。可测的是「令牌来自同一个根」+「取消能传播」；
「信号 → cancel()」这条边只能靠结构锁 + 代码审查守护。本 ADR 不假装它被行为测试覆盖。

### 3.1 追记（待填）

见 §6。

## 4. 回滚

三文件 revert 即回「令牌新建且永不触发」的今日行为（监听 task 随之消失）。
取消是**新增能力**，回滚不削减任何既有功能。

## 5. 未采纳

| 项 | 结论 |
|---|---|
| 每命令一个令牌 + 每命令一个监听器 | 否决：Ctrl+C 会只命中其中一个；且监听器数量随命令增长 |
| 用 `tokio::select!` 在命令层直接监听信号（不经令牌） | 否决：`call_with_budget` 已把取消做成令牌契约；另建一条会形成两份取消逻辑（ADR-241 D3 同型病理） |
| 取消时发布新事件（如 `UserCancelled`） | 否决：需六锁同批且无消费者；`Cancelled` 错误变体已足够 |
| 在 `exec --plan` 里把取消映射为退出码 4 | 未做：取消与工具失败的退出码语义需独立裁决；本批只保证「可取消」，错误映射沿用调用方既有路径 |

## 6. 实测追记（已填）

| 判据 | 结果 |
|---|---|
| `cargo test -p chimera-cli --test cancel_root` | **1 passed / 0 failed**（三段合一：单例身份 + 源码结构锁 + 取消传播；在飞调用在 20ms 后取消，2s 超时内返回 `Cancelled`） |
| `cargo test -p chimera-cli --lib call_budget` | 既有取消/超时/预先取消语义全绿（零修改） |
| `cargo check -p chimera-cli` | 通过 |
| `cargo clippy -p chimera-cli --all-targets -- -D warnings` | 通过 |

**同批清掉的失效导入**：`exec.rs` / `run.rs` 的 `use tokio_util::sync::CancellationToken;`
在改用 `cancel_root()` 后不再需要（类型由返回值推断），已同批删除 ——
否则 `unused_imports` 会让 clippy 门变红。

**已知限制（重复强调，避免误读）**：「信号 → cancel()」这条边**没有**行为测试，
只有源码结构锁。测试能覆盖的是「令牌来自同一根」+「取消能传播」。
任何声称「Ctrl+C 已在 CI 验证」的说法都超出了本批的证据。

**Windows 非交互宿主**：`tokio::signal::ctrl_c` 依赖控制台事件；被父进程以无控制台
方式启动时可能收不到信号 —— 此时令牌保持未触发，与今日行为一致（fail-safe）。
