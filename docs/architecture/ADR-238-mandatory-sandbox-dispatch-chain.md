# ADR-238 -- 强制沙箱派发链（policy → sandbox → route → transport，四环缺一不可）

- 状态：Accepted（2026-11-05）
- 决策者：工具链重构第二波（T2-3 / T2-4）
- 关联：ADR-229（工具执行面落 L7）、ADR-230（MCP ToolInvoke RPC）、ADR-232（LLM 工具循环）、ADR-235（MCP 语义层）、ADR-237（权限三态）
- 影响面：`crates/gqep-executor/src/tool_runtime/`（新模块）、`crates/gqep-executor/src/lib.rs`、`Cargo.toml`（新增 bench 目标）

## 1. 背景与问题

工具执行面在 v4 波次中分裂成多条装配路径（CLI 直连、P0 计划执行、MCP 调用、LLM 工具循环）。每条路径各自决定「先查策略还是先执行」「沙箱检查放哪」。这类重复装配带来的问题不是风格问题：

| 现状 | 失败模式 |
|---|---|
| 策略检查写在 CLI 分支里 | 第二、第三条路径复用时漏掉，且没有任何编译期提示 |
| 沙箱检查在传输层之后 | 拒绝时副作用已经发生；错误如实返回，但「没有执行」的保证已丢 |
| 路由 miss 落到默认执行器 | 未注册工具被默认执行，等同于隐式放行 |

共同点是：**顺序是口头约定，不是结构约束**。

## 2. 决策

### 2.1 D1：四环装饰器链，顺序即安全边界

```
PolicyLayer → SandboxLayer → RouteLayer → TransportLayer
```

每个装饰器仅在「放行」时调用 `next`：

| 层 | 拒绝 | 后续层 |
|---|---|---|
| `PolicyLayer` | `ExecPolicy::evaluate` 返回 `Deny` / `Ask` | 沙箱、路由、传输**均不调用** |
| `SandboxLayer` | `SandboxGate::check` 返回 `Err` | 路由、传输**均不调用** |
| `RouteLayer` | 精确命中；否则 fallback；再否则 `DispatchError::Route` | 传输不调用 |
| `TransportLayer` | 实际执行 | — |

**Ask 也必须前置拒绝**：`ExecPolicy` 的 `Ask` 表示「需审批」；在无审批流上下文时把它当作放行会让审批形同虚设。本层统一返回 `DispatchError::Policy("ask_required: ...")`。

### 2.2 D2：组装只能经 builder，缺环是构造错误

`SandboxedDispatcher` 字段私有、无公有构造；`DispatcherBuilder::build()` 是唯一入口，缺任一环返回：

```
DispatchError::Route("incomplete dispatcher chain: missing policy|sandbox|routes|transport")
```

**WHY 归因为 Route 而非 Policy/Sandbox**：这是组装期错误，不是运行期策略或沙箱判定；错误变体必须能区分「被拒绝」与「没装好」。

### 2.3 D3：错误按层归因且带前缀

`DispatchError { Policy, Sandbox, Route, Transport }`，`Display` 输出 `policy:{msg}` / `sandbox:{msg}` / `route:{msg}` / `transport:{msg}`。
既有 `streaming_dispatch::DispatchOutcome` 已在 crate 根命名空间，故执行面结果类型只经 `tool_runtime::DispatchOutcome` 全路径导出，**不**做根级 re-export（避免 E0252）。

### 2.4 D4：沙箱门默认使用 seccore，工具名按逻辑标识补齐白名单

`SeccoreSandboxGate` 持有 `CommandPolicy` + `EnvPolicy`（`default_secure()`）。`check` 时 clone 策略并把 `req.tool_name` 小写插入 `allowed_commands`：

- **WHY 允许插入**：工具名是逻辑标识（如 `repo.search`），不是 shell 程序；不插入会让所有非 `echo/ls/...` 工具被判 Abuse。
- **WHY 仍安全**：六类拦截模式（注入 / 提权 / 沙箱逃逸 / 数据泄露 / 篡改 / 滥用）仍对 `tool_name + args_json` 完整生效，注入形态参数照样拒绝。

`AllowAllSandboxGate` 为显式命名的逃生门，doc 明确限定「仅纯内存、无副作用 in-proc 工具」。

### 2.5 D5：零新依赖、零新事件

- 异步 trait 用 `std::future::Future` + `std::pin::Pin` 手写 `DispatchFuture<'a>`，不引入 `async-trait` 之外的框架（且 `async-trait` 不参与本层，因需要 `Arc<dyn ToolDispatcher>` 对象安全 + 显式生命周期）。
- 不新增 NexusEvent 变体；派发错误是 L7 内部错误类型。

## 3. 后果

### 3.1 正向

- 「忘了挂沙箱」无法通过编译期/构造期；任一前置层拒绝都不会产生副作用。
- 链序不变量由两层测试锁定：单元测试计数断言 + proptest（64 cases × 3 条）。
- allow 路径端到端延迟可由 criterion 基准持续观测。

### 3.2 负向与代价

- 每次派发多 4 次 boxed future 与 `Arc` 解引用；基准目标为「不引入数量级开销」，不是零开销。
- `RouteLayer` 目前不参与 fallback 之外的传输选择：`transport` 环作为链尾被强制存在并由 `SandboxedDispatcher::transport()` 暴露，供上层复用同一实例；默认 miss 路径仍返回 `Route` 错误。

## 4. 验收证据（2026-11-05 实测）

```
cargo test -p gqep-executor --test tool_runtime_chain_proptest
running 3 tests
test denied_policy_never_reaches_transport ... ok
test route_miss_never_reaches_transport ... ok
test sandbox_denied_never_reaches_transport ... ok
test result: ok. 3 passed; 0 failed

cargo test -p gqep-executor tool_runtime
   (lib) 10 passed; 0 failed   # chain 6 + sandbox 3 + 既有命中
```

## 5. 失效条件

1. 若任一装饰器改成「先 next 再记录错误」，链序不变量失效，proptest 的 `transport_calls == 0` 会先红。
2. 若 `SandboxedDispatcher` 增加公有构造或公有字段，D2 失效（调用方可绕过 builder）。
3. 若 `SeccoreSandboxGate` 的 `allowed_commands` 插入改为「整条策略放宽」或跳过拦截模式，D4 的安全论证失效，`blocks_injection_in_args` 用例会红。
