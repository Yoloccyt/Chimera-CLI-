# ADR-244 -- 工具运行时 delegate 装配点收敛（ADR-240 D1 的落地修订）

- 状态：Accepted（2026-11-06）
- 决策者：收尾批次 P0（装配点统一）
- 关联：ADR-240（组合根唯一装配点 / delegate=None 的 fail-closed 姿态）、ADR-238（四环链）、ADR-241（工具链并行化）
- 影响面：`crates/chimera-cli/src/composition.rs`、`crates/chimera-cli/src/exec_plan.rs`、`crates/chimera-cli/src/commands/run.rs`、`crates/chimera-cli/tests/tool_runtime_wiring.rs`

## 1. 背景与问题

ADR-240 D1 声明「组合根是四环链的唯一生产装配点」，但其 D3 把传输环 delegate 留成 `None`
（当时本仓无生产 `ToolExecutor`）。Wave E′ 为打通 `chimera exec --plan`，在
`run_plan_with_failures` 内**局部再装配**了一个 `ToolRuntime`（`Some(BuiltinToolExecutor)`）。

实测因此同一进程内并存两个装配点与两种传输语义：

| 装配点 | 位置 | delegate | 传输环行为 |
|---|---|---|---|
| 组合根 | `composition.rs:275-280` | `None` | 恒 `Err(Transport("not wired"))` |
| exec 路径 | `exec_plan.rs:262-268` | `BuiltinToolExecutor` | 真实执行内置只读工具 |

证据（调用点 / 生命周期 / 错误语义 / 测试契约）：

- 调用点：`ctx.tool_runtime` 目前无任何生产消费方；`exec_plan::run_plan_with_failures` 直接构造局部实例。
- 生命周期：两者都挂在 `AppContext` 生命周期上方的 ephemeral 进程内，但局部实例每次计划执行重建，
  策略规则来自 `tool_runtime_policy()`、与组合根同源——一旦未来任一处置换规则或默认动作，两处**无编译期约束**保证一致。
- 错误语义：`repo.search` 在组合根是「not wired」，同一名字若出现在 `exec` 计划里则是
  「unknown builtin tool」——同名工具两种归因，审计链无法对齐。
- 测试契约：`tests/tool_runtime_wiring.rs::composition_root_wires_tool_runtime` 把
  「组合根传输环必须是 not wired」写成了断言，这使**任何**真实接线都会先撞红该测试，
  从而反向激励「再开一个局部装配点」。

## 2. 决策

### 2.1 D1：收敛到组合根一处装配（采纳）

- `composition::build()` 以 `Some(Arc::new(BuiltinToolExecutor))` 装配 `ctx.tool_runtime`；
  delegate 类型显式为 `Arc<dyn ToolExecutor>`。
- `exec_plan::run_plan_with_failures` 删除局部装配，改为
  `RuntimeToolExecutor { inner: Arc::clone(&ctx.tool_runtime) }`。
- 策略规则与默认动作的唯一来源仍是 `composition::tool_runtime_policy()`，两处不再各自取一份。

### 2.2 D2：fail-closed 契约迁移（不弱化）

原 `composition_root_wires_tool_runtime` 防的回归类是：
**传输环缺失时以「伪造成功」掩盖缺失（fail-open）**——它断言 `Ok` 绝不出现。

delegate 真实装配后，「not wired」这一具体形态不复存在，但同一回归类由**双断言**守住，强度不降反升：

1. 负向：未注册工具名（`repo.search`）必须 `Err(DispatchError::Transport)` 且文案含
   `unknown builtin tool`，**绝不 `Ok`**；
2. 正向：`text.echo` 必须经 `ctx.tool_runtime` 真实返回 `Ok`——证明接线是真的，不是换了句错误文案的空壳。

另：`production_code_does_not_bypass_tool_runtime` 保持原样（仍禁止在 `src` 内
`ExecPolicyToolExecutor::new` 与裸 `Box::new(InProcDispatcher`），防止命令层再开旁路。

### 2.3 D3：`run --execute` 仍拒绝，但文案改为真实根因

`run --execute` 的拒绝不变（仍是 `NotImplemented`，退出码 2，零副作用）。
过时文案（「缺生产 ToolExecutor」）修正为：「生产 ToolExecutor 与 policy/sandbox 环已在组合根装配，
但 `run` 子命令不路由到执行；执行入口是 `chimera exec --plan <PATH|->`」。
稳定前缀 `execute_not_enabled:` 保持不变（脚本按前缀 grep）。

### 2.4 D4：回退方式

单点回退：把 `composition::build()` 的 delegate 改回 `None`，并在
`run_plan_with_failures` 恢复局部 `ToolRuntime::with_optional_delegate(Some(Arc::new(BuiltinToolExecutor), ...))`。
两处改动互不牵连，回退不需要改测试以外的公共 API。

## 3. 被否决的替代方案

| 方案 | 内容 | 否决理由 |
|---|---|---|
| B：分层装配 + 职责边界 | 组合根保持 fail-closed 供未来消费方；`exec` 自持一份真实运行时 | 同进程两种传输语义（同名工具两种归因）；策略/沙箱配置双源、无编译期一致性；ADR-240 D1 已声明唯一装配点，分层方案实质是推翻它而没有给出新不变量 |
| C：把内置执行器下移到更低层 crate，再由命令层装配 | 新增 L7/L6 依赖或 crate | 与「最小改动」相悖；`chimera-cli` 已依赖 `gqep-executor`，内置只读工具留在 CLI 侧不构成跨层负担（L10 组合根持有 L7 运行时是允许的向下依赖） |

## 4. 后果

- 正向：单装配点、单策略源；`ctx.tool_runtime` 成为真实可消费对象；`repo.search` 归因统一为 transport。
- 负向/边界：组合根派发现在具备真实文件 IO 能力（`fs.read` / `fs.list`）。
  约束仍然生效：仅相对路径、拒绝 `..`/绝对路径/盘符前缀、`fs.read` ≤256 KiB、
  未知工具恒 `Err`。故默认能力面仍是「只读内置工具」，不引入网络或写入。
- 文档：`EXECUTE_REFUSAL_MSG` 与 `tool_runtime_wiring.rs` 头注释同步改写，避免误导性说明。
