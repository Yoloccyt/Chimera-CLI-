# ADR-273 -- `mcp call` 命令面接入 policy + sandbox 准入前置（W7）

- 状态：Accepted（2026-11-09）
- 决策者：R6 / W7
- 关联：ADR-256（ToolExecutor 面四环接入 —— **命令面不在其射程内**）、ADR-244 D1（ToolRuntime 组合根唯一装配点）、ADR-245（`mcp call` 传输选择）、ADR-242 §3.5（安全面**无**运行期开关）、ADR-239（先实测后登记）
- 影响面：`crates/chimera-cli/src/commands/mcp.rs`、`commands/mod.rs`、`spec.rs`；`crates/gqep-executor/src/tool_runtime/{chain.rs,runtime.rs}`；测试 3 个文件 + 新增 1 个
- 行为影响：默认 `Allow` 下结果不变；新增 policy/sandbox 拒绝路径；`mcp call` 启动多付一次组合根装配（见 §3/§6）

## 1. 背景（U-02）

ADR-256 把 `mcp.<server>.<tool>` 接进了 **ToolExecutor 面**（`exec --plan` 的计划节点），
但命令面 `chimera mcp call` 一直是：

```text
permission::confirm  →  mesh.execute_transaction
```

**没有任何 execpolicy、没有任何沙箱门**。也就是说「所有外部调用经 SecCore 沙箱」
这条 §6 红线，在 mcp 命令面上不成立 —— 而 ADR-256 全文并未裁决命令面
（那是 ToolExecutor 面），所以这是一个**未被任何 ADR 覆盖的缺口**。

## 2. 决策

### D1 分类按动作分级：只有 `Call` 升为 FullContext

| 动作 | 分类 | 理由 |
|---|---|---|
| `McpAction::Call` | `FullContext { sharding: Off }` | 会执行外部工具 ⇒ 需要策略/沙箱前置 |
| `List` / `Inspect` | `NoContext` | 只读注册表，无执行面 |
| `Serve` | `NoContext` | 维持 `NotImplemented`（ADR-256 D7） |

### D2 前置用 `admit()`，**不是** `dispatch()`

原方案措辞是「经 `ctx.tool_runtime.dispatch` 跑 policy+sandbox」。实现必须订正为
**新入口 `ToolRuntime::admit`**，理由是硬的：

- `dispatch` 是完整四环 —— Allow 时会落到传输环**真的执行**工具。命令面随后
  自己还要执行一次 ⇒ **二次执行**；且执行者是组合根装配的 MCP 执行器，
  不是命令面自己的 ephemeral mesh（两套生命周期，ADR-256 已登记）。
- `admit` 只穿 **policy → sandbox** 两环，然后由命令面继续自己的执行。

判决体单一性（这是本 ADR 的关键实现约束）：

```rust
// chain.rs：抽出唯一判决体，PolicyLayer/SandboxLayer 改为调用它们
pub(crate) fn evaluate_policy(policy: &ExecPolicy, req: &DispatchRequest) -> Result<(), DispatchError>
pub(crate) fn check_sandbox(gate: &dyn SandboxGate, req: &DispatchRequest) -> Result<(), DispatchError>
pub(crate) fn evaluate_admission(policy, gate, req) -> Result<(), DispatchError> // 前两环顺序组合
```

`ToolRuntime` 保存的 `AdmissionRings` 与链内是**同一对 `Arc`**
（`Arc<ExecPolicy>` / `Arc<dyn SandboxGate>`）⇒ 同一策略实例、同一沙箱实例、
同一判决函数 ⇒ `admit` 与链内前两环**不可能给出不同结论**。
（若在命令面重新构造一份 `ExecPolicy`，就是 ADR-244 D1 要消灭的「双源无编译期一致性」。）

### D3 顺序：confirm → admit → dry_run/execute

```text
permission::confirm（用户意图闸门）
        ↓
ToolRuntime::admit（机器规则闸门：policy → sandbox）
        ↓
dry_run ? 预览 : mesh.execute_transaction（既有执行面）
```

- **confirm 在前**：用户同意不能替代策略放行；策略放行也不能替代用户同意。
  两道闸门互不推导。
- **admit 在 dry_run 之前**：预览必须反映真实结论。若预览说「会调用」、实跑却被
  策略拒绝，dry-run 就没有意义。
- 沙箱检查的对象是 `mcp.<server>.<tool>` 全名 + `args_json`：
  `SeccoreSandboxGate` 会把工具名本身加入派生白名单（非 shell 程序名），
  但 Injection / PrivilegeEscalation / SandboxEscape 等拦截模式对
  `tool_name + args` 完整生效 —— 这正是我们要的那层。

### D4 拒绝归因

`ChimeraCliError::PermissionDenied`（exit 5，语义是「权限策略拒绝」而非「输入错误」），
消息保留上游 `policy:` / `sandbox:` 前缀 ⇒ 失败归因不丢失。

### D5 缺 runtime **不得**静默跳过

`execute(..., runtime: Option<&ToolRuntime>)`；`Call` 分支拿到 `None` 时直接返回
`EngineError("内部装配错误：mcp call 缺少 ToolRuntime")`。

分类已保证生产路径不会走到这里 —— 这条分支存在是为了防一种退化：
把前置写成 `if let Some(rt) = runtime { … }`，于是任何直调者传 `None`
就重新裸奔。负对照测试 `mcp_call_without_runtime_is_internal_error` 钉住它。

### D6 只复用判决，不复用执行

组合根的 `McpToolExecutor` mesh 与命令面 ephemeral mesh 生命周期不同
（前者长持、后者一次性），本轮**不合并**两者。合并属独立裁决（需先回答
「长持 mesh 的注册表何时刷新」这一现网问题）。

## 3. 装配税

`mcp call` 从 NoContext 变为 FullContext ⇒ 需要构建 `AppContext`。
本机 dev 日志实测（W6 同批）：

```text
startup phase: parse+init  ~1.9–9.4 ms
startup phase: config      ~2.8–11.0 ms
startup phase: runtime     ~0.8–1.1 ms
startup phase: assemble    ~1.19 ms
```

裁决：**接受**。理由是安全前置优先于毫秒级启动延迟，且 `List/Inspect/Serve`
零增量（保持 NoContext）。不静默放宽任何阈值；A/B 实测见 §6。

## 4. 验收（现跑）

| 判据 | 要求 | 结果 |
|---|---|---|
| `mcp_command_admission` | 5/5（3 前置 + 1 负对照 + 1 分类锁） | ✅ 5 passed |
| `mcp_call_network`（既有） | 2/2 零修改绿（Allow 路径行为不变） | ✅ 2 passed |
| `mcp_config_defaults`（既有） | 3/3 零修改绿（List 仍 NoContext） | ✅ 3 passed |
| `spec::tests` | 4/4（含新增 Call/Inspect 用例） | ✅ 4 passed |
| `tool_runtime_chain_proptest` + `transport_ring_traversal` | 链路判决等价（抽函数无行为变化） | ✅ 3 + 6 passed |
| `cargo clippy -p gqep-executor -p chimera-cli --all-targets -- -D warnings` | 0 warning | ✅ |

## 5. 回滚

单 commit revert。**两种半回滚都禁止**：

- 保留 `admit` 调用却回退分类 ⇒ `Call` 无 `AppContext`，生产直接报内部装配错误；
- 保留分类却删除 `admit` ⇒ 白付装配税且退回裸奔。

## 6. 实测：`mcp call` 装配税（同轮 A/B）

对照：baseline = `target/debug/deps/chimera-def5eee1a648ba86.exe`（2026-11-08 构建，
W7 之前的命令行语义）；W7 = 本波重建的 `target/debug/chimera.exe`。
Python `perf_counter()` 单进程计时，各 7 次交替热跑：

| 命令 | baseline 中位数 | W7 中位数 | Δ |
|---|---|---|---|
| `mcp call dead text.echo`（升 FullContext） | 43.57 ms（39.2–75.0） | 48.19 ms（43.2–51.6） | **+4.63 ms（+10.6%）** |
| `mcp list`（**对照组**，两侧都 NoContext） | 45.39 ms（38.4–64.4） | 46.97 ms（42.5–50.8） | +1.57 ms（+3.5%） |

- 对照组自身就有 +1.57 ms 的构建间漂移（两个二进制不是同一次编译），
  ⇒ **可归因于装配的增量 ≈ 3.1 ms**，与 dev 日志 `assemble_ms=1.19` 同量级
  （其余为 FullContext 路径的额外构造）。
- **裁决：接受**。安全前置（policy + sandbox）优先于毫秒级启动延迟；
  `List/Inspect/Serve` 保持 NoContext，零增量。
- `cli_startup_e2e`（5 ms 阈值）只 bench `run` 一条命令（`benches/cli_parse.rs:128-148`），
  本 ADR 不触碰该路径 ⇒ **阈值与 bench 均无需改动**（不放宽、不新增键）。

## 7. 未采纳

| 项 | 结论 |
|---|---|
| 命令面新建第二个 `ToolRuntime` | 否决：违 ADR-244 D1（同名工具两种策略语义） |
| 合并组合根 mesh 与命令面 mesh | 否决：生命周期不同；合并需先裁「长持 registry 刷新」 |
| 为前置加运行期开关（可关掉 policy/sandbox） | 否决：违 ADR-242 §3.5（安全面**无**开关） |
| 让 `mcp call` 走完整 `dispatch` 并复用其执行结果 | 否决：二次执行 + 第二套 mesh 生命周期 |
| 把 `List/Inspect` 也升 FullContext「统一处理」 | 否决：零收益装配税；分类的目的就是按需付税 |
