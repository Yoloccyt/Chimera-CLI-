# ADR-240 -- 工具运行时四环链的生产接线（组合根唯一装配点）

- 状态：Accepted（2026-11-06）
- 决策者：工具链并行化重构第二波（W2-T1）
- 关联：ADR-238（强制沙箱派发链四环）、ADR-237（权限三态 fail-closed）、ADR-239（性能阈值基线与棘轮）
- 影响面：`crates/chimera-cli/src/composition.rs`、`crates/chimera-cli/tests/tool_runtime_wiring.rs`（新）、`crates/gqep-executor/src/tool_runtime/runtime.rs`、`crates/seccore/src/execpolicy.rs`

## 1. 背景与问题

ADR-238 建立了强制沙箱派发链（policy → sandbox → route → transport），其验收证据来自 `gqep-executor` 的单元测试与 proptest。但**生产路径无调用点**：

```console
$ git grep -n "ToolRuntime"
crates/gqep-executor/src/lib.rs:101:// T2-3/T2-4: 强制沙箱派发链公开 API（...）
crates/gqep-executor/src/lib.rs:108:    ToolRuntime,
```

只有 `lib.rs` 的注释与 re-export。进一步取证发现缺口比「接线未做」更深：

| 层 | ADR-238 落地后的实际状态 | 证据 |
|---|---|---|
| `ToolDispatcher` 四环链 | 仅被 `gqep-executor` 的 tests/benches 使用 | `git grep "ToolRuntime"` 仅 2 处命中 |
| `ExecPolicyToolExecutor` | **生产路径亦无调用点** | `git grep "ExecPolicyToolExecutor::new"` 命中全在 `exec_tool_executor.rs` 的测试模块与 `tool_runtime/runtime.rs` |
| `ToolExecutor` 生产实现 | **不存在** | `grep "impl ToolExecutor for"` 的非测试命中只有 `ExecPolicyToolExecutor`（它自身包裹 delegate）与 `ToolRuntime`（它自身实现 trait） |
| CLI 侧工具执行面 | **完全空白** | `grep "ToolExecutor\|tool_executor\|execute_tool" crates/chimera-cli/src` → 无命中 |

这意味着 ADR-238 承诺的「四环缺一不可」「任一前置层拒绝都不产生副作用」这条安全承诺，**从未在生产装配中被验证过**。工具执行路径实际上既无策略层也无沙箱层。

## 2. 决策

### 2.1 D1：组合根是四环链的唯一生产装配点

`AppContext` 新增字段：

```rust
pub tool_runtime: Arc<gqep_executor::tool_runtime::ToolRuntime>,
```

**类型是 `Arc<ToolRuntime>` 而非 `Option<...>`** —— 组合根**必须**装配它，没有「不装配」这条路径。若将来有人改成 `Option` 且默认 `None`，`tool_runtime_wiring.rs` 的 `composition_root_wires_tool_runtime` 会红。

**WHY 在组合根而非各命令**：沙箱层是**全局安全前置**。在命令层装配必然漏掉某条路径 —— 这正是 ADR-238 落地后仍存在该缺口的根因。

### 2.2 D2：默认动作用 Allow 而非零信任 Deny —— 不削减现有功能

`seccore::execpolicy::ExecPolicy::new()` 的零信任默认是 `Deny`（未声明即拒绝）。但 CLI 当前的工具执行路径**本来就没有策略层**（见 §1 取证）。

若组合根用零信任默认，所有工具调用会从「可执行」变成「一律拒绝」 ⇒ **削减现有功能**，违反用户硬约束「在不削减工具链现有全部功能的前提下」。

故 `composition::tool_runtime_policy()` 返回：

```rust
(Vec::new(), PolicyAction::Allow)   // 无规则 + 默认放行
```

| 层 | 接线前 | 接线后 | 变化 |
|---|---|---|---|
| policy | **不存在** | 存在，默认 Allow，无规则 | 无功能削减（行为等价于无策略层） |
| sandbox | **不存在** | **真实生效**（seccore 六类拦截模式） | **新增防护** |
| route | 不存在 | 存在（空表 + fallback） | 无外部可见变化 |
| transport | 不存在 | 存在（未接线，见 D3） | 见 D3 |

净效果：**不削减任何既有功能，同时新增沙箱层防护**。

策略规则的声明式来源（配置面）留待独立批次 —— 那需要新增配置段并触发 `config_sample` 门与 14 段 Figment 的联动，属另一个变更的范畴。

### 2.3 D3：未接线传输环必须 fail-closed，不得虚构成功

本仓尚无生产 `ToolExecutor` 实现。组合根面临选择：

| 方案 | 后果 | 裁决 |
|---|---|---|
| 装一个「总是成功」的 delegate | **虚构功能** —— 任何工具调用都返回成功，把「这条路径没有真实执行器」掩盖成「工具执行成功」 | **拒绝** |
| 装一个恒拒绝的 delegate | 未接线状态如实可见：派发返回 `transport: tool executor not wired: ...` | **采纳** |

`ToolRuntime::with_optional_delegate(None, rules, default_action)` 在 delegate 缺席时装 `UnwiredToolExecutor` —— 恒拒绝，方向是 fail-closed（拒绝 > 误放行）。

**关键**：可缺省委托**只影响传输环**。policy 与 sandbox 两环的行为与装了真实 delegate 时**完全一致** —— 它们仍先于传输环短路。

### 2.4 D4：`seccore` 新增 `with_default_action` 构造器

`ExecPolicy` 的 `default_action` 是私有字段，原有 `new()` + `add_rule()` 链式 API **无法**表达「默认放行 + 规则设防」语义（后者只能加规则，无法把「未命中」从 `Deny` 改为 `Allow`）。

新增：

```rust
pub fn with_default_action(default_action: PolicyAction) -> Self
```

**不改变** `new()` 的零信任默认值 —— 默认值的安全性不变，本构造器只暴露选择权。既有 `ExecPolicy::new()` 的所有调用方零影响。

### 2.5 D5：反向断言锁死「只有组合根装配」

正向测试只能证明「组合根装了它」，无法证明「**只有**它」。若某命令层自行 `ExecPolicyToolExecutor::new(...)` 或直接持裸 delegate，沙箱层就会被绕过 —— 而这类绕过**不会有任何编译错误或测试失败**。

`tool_runtime_wiring.rs::production_code_does_not_bypass_tool_runtime` 扫描 `crates/chimera-cli/src` 的生产代码文本（跳过 `#[cfg(test)]` 模块），断言：

- 不存在 `ExecPolicyToolExecutor::new`（绕过四环链直接装配策略执行器）
- 不存在 `Box::new(InProcDispatcher`（绕过 builder 自行装配派发器）

### 2.6 D6：安全不变量逐条自查（seccore 6 条不可学习红线）

| 红线 | 本次接线后的状态 |
|---|---|
| 沙箱不可降级 | ✅ 沙箱层真实生效，`SeccoreSandboxGate::default_secure()`；`AllowAllSandboxGate` 未被生产路径使用 |
| 审计链不可篡改 | ✅ 未触及事件总线与审计链 |
| 零孤儿不可绕过 | ✅ 四环链无 `tokio::spawn`，无 fire-and-forget |
| 最小权限底线 | ✅ CapabilityToken 相关代码未触及 |
| forbid(unsafe) 不可移除 | ✅ 三个改动 crate 顶层均有 `#![forbid(unsafe_code)]` |
| BudgetExceeded=Critical 不可降级 | ✅ 未触及事件定级 |

## 3. 后果

### 3.1 正向

- 四环链首次进入生产装配路径，「缺环即构造失败」的不变量在生产侧可验证。
- 沙箱层成为**真实生效**的全局安全前置：注入形态、提权、沙箱逃逸、数据泄露、篡改、滥用六类模式对 `tool_name + args_json` 完整生效。
- 传输环的未接线状态**如实可见**，不再被「总是成功」掩盖。

### 3.2 负向与代价

- 组合根每次 `build()` 多一次四环链装配（四次 `Arc` 分配 + 一次策略引擎构造）。实测成本远低于一次 `multi_thread` runtime 构建（783µs），可接受。
- `AppContext` 结构变更 ⇒ 任何手工构造 `AppContext` 的代码需同步（当前组合根是唯一构造点，实测无其他构造点）。

### 3.3 尚未解决（明确记为后续批次）

1. **生产 `ToolExecutor` 实现仍缺** —— 传输环仍为未接线状态。真实工具执行器（进程调用 / MCP 调用）的落地是独立批次。
2. **策略规则的声明式来源仍缺** —— 需要新增配置段并联动 `config_sample` 门与 Figment。
3. **`--execute` 仍显式拒绝** —— 按 Q-1 裁决，本批次只接线不启用。

## 4. 验收证据（2026-11-06 实测）

```
cargo test -p chimera-cli --test tool_runtime_wiring
running 4 tests
test tool_runtime_policy_default_action_is_allow ... ok
test composition_root_tool_runtime_enforces_sandbox_gate ... ok
test composition_root_wires_tool_runtime ... ok
test production_code_does_not_bypass_tool_runtime ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

cargo test -p chimera-cli -p gqep-executor -p seccore
  （逐 target 全绿，无回归）

cargo clippy -p chimera-cli -p gqep-executor -p seccore --all-targets --jobs 2 -- -D warnings
  EXIT=0

pwsh scripts/check_dependency_rules.ps1
  [OK] dependency iron-law audit all pass
  EXIT=0

cargo fmt -p chimera-cli -- --check   EXIT=0
cargo fmt -p gqep-executor -- --check EXIT=0
cargo fmt -p seccore -- --check       EXIT=0
```

## 5. 失效条件

1. 若 `AppContext.tool_runtime` 改成 `Option` 且组合根默认装 `None`，`composition_root_wires_tool_runtime` 会红。
2. 若生产代码在命令层自行 `ExecPolicyToolExecutor::new(...)`（绕过组合根），`production_code_does_not_bypass_tool_runtime` 会红。
3. 若沙箱层被换成 `AllowAllSandboxGate` 或被跳过，`composition_root_tool_runtime_enforces_sandbox_gate` 会红。
4. 若策略默认动作被改为 `Deny`，`tool_runtime_policy_default_action_is_allow` 会红（并会削减现有功能）。
5. 若传输环改为「总是成功」的 delegate，`composition_root_wires_tool_runtime` 的「不得返回成功」断言会红。
