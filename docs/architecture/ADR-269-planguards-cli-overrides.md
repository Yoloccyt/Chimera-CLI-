# ADR-269 -- `PlanGuards` 三字段的 CLI 投影入口（只收紧，缺省逐字不变）

- 状态：Accepted（2026-11-09）
- 决策者：R6 / W3
- 关联：ADR-254 D5（只读投影修复 F-A21 —— 本 ADR 让它**生产可达**）、ADR-268（W2 的 `--plan-strict` 与同一函数签名）、ADR-239（只收紧不放宽）
- 影响面：`crates/chimera-cli/src/exec_plan.rs`（`PlanGuardOverrides` + `plan_guards_with`）、`crates/chimera-cli/src/commands/exec.rs`、`crates/chimera-cli/src/commands/mod.rs`、`crates/chimera-cli/src/cli.rs`、`crates/chimera-cli/src/spec.rs`（测试夹具）、新增 `crates/chimera-cli/tests/exec_plan_guards.rs`
- 行为影响：**缺省零变化**；三个旗标全部默认关闭/未设

## 1. 背景：只读投影修好了但没人能开

ADR-254 D5 把 `PlanGuards.readonly_only` 的判据从「只看 `Some(Write)`」改成
「有效副作用 ≠ 只读即拒」，堵住了「省略 `side_effect` 绕过只读模式」的洞（F-A21）。
但实测：`readonly_only` 在 `crates/*/src` 中**除定义与默认值外零调用点**，
唯一置 `true` 的地方是 `toolplan_runner.rs` 的单测。

⇒ 该修复在现网**不可触发**。同理 `max_steps`（默认 64）与 `budget_bytes`（默认 64 KiB）
也没有任何 CLI 入口，计划作者只能吃默认值。

## 2. 决策

### D1：加性引入 `PlanGuardOverrides`，语义是「只收紧」

```rust
pub struct PlanGuardOverrides { pub readonly: bool, pub max_steps: Option<usize>, pub budget_bytes: Option<usize> }
impl PlanGuardOverrides {
    pub fn apply(self, base: PlanGuards) -> PlanGuards {
        PlanGuards {
            readonly_only: base.readonly_only || self.readonly,
            max_steps: self.max_steps.map_or(base.max_steps, |v| v.min(base.max_steps)),
            budget_bytes: self.budget_bytes.map_or(base.budget_bytes, |v| v.min(base.budget_bytes)),
            timeout_ms: base.timeout_ms,
        }
    }
}
```

**只收紧**的理由：既有默认（64 步 / 64 KiB / 非只读）就是当前生产契约。
允许 CLI 放宽会让「同一个二进制在不同参数下有不同的资源上界」，
与 `effective_op_timeout_ms`「CLI 只能收紧、不能越过全局」的既有纪律不一致 ——
同一类上界应当用同一条规则。

`PlanGuardOverrides::default()`（全空/false）⇒ 与 `plan_guards_for` 输出**逐字相同**，
故所有既有调用点行为不变。

### D2：CLI 面三个旗标，缺省即今天的行为

| 旗标 | 语义 | 缺省 |
|---|---|---|
| `--plan-readonly` | 只读投影：拒任何非只读节点 | false |
| `--plan-max-steps <N>` | 步数上界（只能 ≤ 64） | 未设 = 64 |
| `--plan-budget-bytes <BYTES>` | 回填预算（只能 ≤ 65536） | 未设 = 65536 |

解析类型用 `u64` + `range(1..)`（与既有 `--op-timeout-ms`/`--plan-timeout-ms` 同款），
再以 `usize::try_from(...).unwrap_or(usize::MAX)` 饱和转换：
32 位宿主上超大值退化为 `usize::MAX`，随后 `apply()` 把它夹回默认上界 —— **仍然只收紧**，
不会因为平台差异而放宽，也不会因 `0` 造成「任何计划都不可执行」的脚枪
（`0` 被 clap `range(1..)` 在解析期拒绝）。

### D3：`run_plan_with_failures_timed` 加一个参数，兼容入口传默认值

签名加第 6 参 `overrides: PlanGuardOverrides`；`run_plan_with_failures` 与
`execute_plan_with_ctx` 等兼容入口一律传 `PlanGuardOverrides::default()`
⇒ 既有调用点行为不变（与 W2 对 `execute_plan_with_timeouts` 的处理同型）。

## 3. 验收（现跑）

| 判据 | 要求 |
|---|---|
| `plan_readonly_flag_rejects_write_node` | 含显式 `Write` 节点 + 只读覆写 ⇒ 校验期拒，`EngineError→3`（不进 4） |
| `plan_readonly_default_off_unchanged` | **反向对照**：同一计划不传只读覆写 ⇒ 照常执行（证明上一条的红来自覆写） |
| `plan_max_steps_flag_caps` | 3 节点链 + `max_steps: 2` ⇒ 校验期拒，exit 3 |
| `plan_budget_bytes_flag_tightens` | 70 KiB 回显 + 预算收到 1 KiB + `--plan-strict` ⇒ exit 4 且原因 `budget_bytes` |
| `cargo test -p chimera-cli --lib spec` | 6/6（`Commands::Exec` 夹具补齐三字段后分类表不变） |

**第 2 条为什么必须有**：只测「只读覆写会拒写」，一个「任何计划都拒绝」的实现也会通过 ——
反向对照把失败原因锁在覆写本身。

### 3.1 追记（待填）

见 §6。

## 4. 回滚

三个旗标缺省 ⇒ 运行时零差异。单 commit revert：去掉旗标与 `overrides` 参数，
`PlanGuardOverrides` 可保留（加性、无调用方即无害），但须同批撤掉测试。

## 5. 未采纳

| 项 | 结论 |
|---|---|
| 允许 CLI **放宽** `max_steps`/`budget_bytes` | 否决：破坏「同一二进制同一资源上界」，与超时旗标的只收紧纪律不一致 |
| 用 `usize` 直接做 clap 值类型 | 否决：`clap::value_parser!(usize)` 不提供 `.range`（实测 `E0599`），改用 `u64` + 饱和转换 |
| 把 `readonly_only` 默认改为 true | 否决：会拒绝既有写计划 ⇒ 削减功能；只读必须由调用方显式请求 |
| 只加 `PlanGuards` 字段而不给 CLI 入口 | 否决：正是本 ADR 要修的 F-08（修复不可达＝死码） |

## 6. 实测追记（待填）

- `cargo test -p chimera-cli --test exec_plan_guards`（4 项）
- `cargo test -p chimera-cli --test exec_plan_truncation`（W2 三测，回归不破）
- `cargo test -p chimera-cli --test exec_plan_wiring`（18 项零修改）
- `cargo test -p chimera-cli --lib spec`（6 项）
- `cargo clippy -p gqep-executor -p chimera-cli --all-targets -- -D warnings`
- 旗标面变化对 `cli_parse` 的影响并入 **最终同代 A/B**（W2+W3 合并测），
  不单独引用跨轮 `change`（见 ADR-267 §5.2 / ADR-268 §6.1 的 base 漂移说明）
