# ADR-237 -- 权限三态与 fail-closed 降级（非交互 Unknown → Deny）

- 状态：Accepted（2026-11-05）
- 决策者：工具链重构第二波（T2-1 / T2-2）
- 关联：ADR-229（工具执行面）、ADR-232（LLM 工具循环审批默认拒绝）、ADR-233（计划路径审批信号）、ADR-238（强制沙箱派发链）
- 影响面：`crates/chimera-cli/src/permission.rs`、`crates/chimera-cli/src/commands/*`（既有调用面语义不变）

## 1. 背景与问题

既有 `permission::confirm` 只有二元判断：`--yes` / `--no-permission` 跳过，否则读 stdin 提示。这个模型在两个场景下不成立：

| 场景 | 既有行为 | 风险 |
|---|---|---|
| 非交互环境（管道 / CI / 子进程捕获）且无 flag | 仍然尝试读 stdin；读到 EOF 后按默认 N 处理 | 读到意外输入或阻塞；无法把「无法问」与「用户拒绝」区分 |
| `--no-permission` 与 `--yes` 之外的新增审批来源（LLM 工具循环、MCP 调用） | 只能复用二元开关 | 新来源无法表达「我不知道，你决定」，只能先放行再报错 |

根因是把「无 flag」当成「用户会回答」。正确模型是**三态**：Allow（显式允许）/ Deny（显式拒绝）/ Unknown（需要问，但环境可能问不了）。

## 2. 决策

### 2.1 D1：三态决策与授权结果分离

- `PermissionDecision { Allow, Deny, Unknown }`
- `PermissionAuthorization { decision, downgraded }`

`evaluate()` 是纯函数：`--yes || --no-permission → Allow`，否则 `Unknown`。
`authorize()` 在 `evaluate()` 之上应用环境能力与计数，是唯一产生 `Deny` 与 `downgraded` 的位置。
**WHY 分离**：评估可被无副作用的单测与上层组合（如工具循环先判定再交给审批流），授权才携带 fail-closed 副作用。

### 2.2 D2：非交互 Unknown 必须 fail-closed

`Unknown` 且 `!interactive` → `Deny` + `downgraded = true`，同时：

- `tracing::warn!(reason = "unknown_permission_fail_closed", ...)`
- 全局 `AtomicU64` 计数器 +1，经 `fail_closed_downgrades()` 读取

**拒绝项 R1**：拒绝把「非交互 Unknown」按 `Allow` 处理。任何默认放行都会让「忘了传 `--yes`」在 CI 中静默扩权。

### 2.3 D3：交互能力可注入，不依赖 TTY 才能测

`PermissionGate::from_stdin()` 用 `std::io::stdin().is_terminal()`（`std::io::IsTerminal`，零新依赖）；
测试路径用 `PermissionGate::new(false/true)` 注入。`confirm()` 委托内部 `confirm_with(&gate, ...)`，使非 TTY 环境也能覆盖 Deny 路径。

### 2.4 D4：Deny 不读 stdin

`confirm_with` 先 `authorize`：

| 决策 | 行为 |
|---|---|
| `Allow` | `Ok(true)`，不读 stdin |
| `Deny`（含降级） | `Ok(false)`，不读 stdin |
| `Unknown` | 原交互 prompt + Ctrl+C `select` 路径 |

**WHY**：读 stdin 本身是副作用；在 Deny 分支读入会把管道中的后续字节吞掉。

## 3. 后果

### 3.1 正向

- 非交互环境默认拒绝，无需调用方补丁。
- 降级可观测（warn + 计数），可接入指标面板。
- 既有 7 个权限单测零改动通过（`--yes` 不读 stdin 等语义保持）。

### 3.2 负向与代价

- 交互环境行为不变（`Unknown` 仍走原 prompt），交互体验无回归。
- 计数器是进程级全局量，跨测试并发断言只能判「递增」而非精确差值；单测按 `after > before` 断言。

## 4. 验收证据（2026-11-05 实测）

```
cargo test -p chimera-cli --lib permission
running 11 tests
test permission::tests::permission_matrix ... ok
test permission::tests::test_confirm_non_interactive_without_flag_returns_false ... ok
test permission::tests::test_confirm_allow_matrix_returns_true ... ok
test permission::tests::test_confirm_with_yes_returns_true_without_reading_stdin ... ok
test result: ok. 11 passed; 0 failed
```

## 5. 失效条件

1. 若未来引入显式 `--deny` 一类配置，必须另起字段并保持 `--yes` 与 `--no-permission` 的或语义不变，否则矩阵测试的红/绿会互相掩盖。
2. 若把 `from_stdin()` 换成「默认交互」，则 D2 失效，必须同步删除 `fail_closed_downgrades` 计数器与相应用例。
