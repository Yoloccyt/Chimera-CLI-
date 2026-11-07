# Chimera CLI 工具链全景深度优化与重构方案（W7-R2 · 现势对账版）

> **文档性质**：W7 续作（`radiant-thunder-darwin-xenXoOpR.md`，已批准）的**现势对账续篇 + 全景扩展**。本文件是本轮（2026-11-07）唯一权威执行方案；与上游方案冲突处，**以本文件的现势取证为准**并显式标注。
> **基线快照**：分支 `feat/toolchain-parallel-optimization`，HEAD `c6eb9b6`（2026-11-07 11:21，ADR-254 能力表落地）；工作树干净；`Cargo.toml:93` version `2.29.0-omega`；workspace 46 members。
> **本轮用户裁定（2026-11-07）**：
> ① 交付物 = 本方案文档 + **连续执行 W0.4-r → W1 → W2**；
> ② 范围从「执行/派发链主线」**扩展为全景接线**（P1~P4 波纳入方案，逐波独立 ADR + 独立 revert）；
> ③ **授权修改 `.github/workflows`**（按 ADR-234 A4 先贴片段、原子同批落地）。
> **审计基准**：当前主检出工作树现态（排除 `.qoder/worktrees/`、`.tower/`、`tmp/`、`target/`）。
> **证据纪律**：ADR-234「证据先于断言、低报与高报同属错误」；全部结论标注【实测】（本轮 2026-11-07 三路并行取证，file:line 为 HEAD c6eb9b6 现值）/【报告自述】/【静态文本】/【不可判定】。

---

## 0. 元规则（继承上游方案 §0，本轮全文有效）

| 编号 | 规则 | 要点 |
| --- | --- | --- |
| M-1 | `.md` 存在性判定用 `ls`/`test -f`/`git ls-files`，禁用 Glob 作判据 | `Glob` 遵循 `.gitignore` 的 `*.md` 排除会漏报 |
| M-2 | 「已接线」= 生产构造点 + 生产消费点同时存在 | 仅测试构造点只能判 DORMANT/DECORATIVE（`CONTRIBUTING.md:90`） |
| M-3 | 结论分级：代码实证 > 文档陈述 > 报告自述 > 推定 | 四层证明力 L1 结构 > L2 计数器 > L3 比值墙钟 > L4 criterion；「测试全绿」不得顶替 L1 |
| M-4 | 多 worktree 分叉，引用前现取复核 | ADR-229~236 物理文件仅在 `.qoder/worktrees/toolplane-execution/` |
| M-5 | 数字一律现取、标注日期与尺子 | 见 §3/§4 各表 |
| M-6 | md 入库状态按白名单分层，不笼统写「md 不入库」 | HEAD `.gitignore:67-76`：`*.md` 全局忽略 + 3 条豁免（ADR-*/v3.5 总账/DOC-AUDIT-FACTS_*） |
| M-7（新增） | **git 写操作最小化**：逐文件 add、HEREDOC 提交、禁 `-A`/`--no-verify`/amend/force | `AGENTS.md` Git 协作红线 |

---

## 1. 现势对账：相对上游方案已发生的变化（本轮三路并行取证）

### 1.1 W0 报告（2026-11-06 21:09）之后落地的 3 个提交

| 提交 | 时间 | 内容 | 关闭的遗留项 |
| --- | --- | --- | --- |
| `253d96c` | 11-06 23:38 | 忽略增补（`coverage/` + 5 个本地工具状态目录）+ G-141/G-142 词法门接线进 ci.yml + toolplan undeclared 值组 + Cargo.toml 日期订正 + composition.rs 回滚注释订正 | **Q-A**（5 项在途改动吸收提交）、**F-05**（composition.rs:165-168 注释已按 ADR-244 订正）、**F-11**（Cargo.toml 日期笔误） |
| `0a9aee5` | 11-07 09:04 | `toolplan_layers` 升格 registered（perf_thresholds.toml:499-506 + bench 映射 :255-258 + 别名 :504-506）+ 缺省 side_effect 测量锚（undeclared_parallel=160ms 刻意改前上界） | **Q-D**（路径 B：升格 registered），ADR-252；`inventory unknown=1→0` |
| `c6eb9b6` | 11-07 11:21 | 内置工具只读能力声明（`BUILTIN_READONLY_TOOLS` + `capability_of`/`declared_tools`）+ PlanRunner 判据改造（有效副作用优先级 + F-A21 堵绕过）+ ToolRuntime 能力视图就地保留 + 路由表由 declared_tools 驱动 + sandbox_gate `check_ref` 抹平 to_string + ADR-255 mcp.servers 收口为空 | ADR-254 全部结构决策落地 |

### 1.2 已闭环项（相对上游方案 §3/§4 断言的现势订正）【实测】

| 上游编号 | 上游断言 | 现势（HEAD c6eb9b6） | 证据 |
| --- | --- | --- | --- |
| F-05 | composition.rs 头注释仍写「传输环未接线」 | ✅ 已订正（W0 R5 落码 + 253d96c 提交） | composition.rs:165-168 |
| F-11 | Cargo.toml:652 引用日期笔误 | ✅ 已订正（253d96c） | Cargo.toml |
| Q-A | 5 项在途改动未提交 | ✅ 已被 253d96c 吸收，工作树干净 | `git status` 零输出 |
| Q-D | toolplan_layers 未登记（inventory unknown=1） | ✅ 升格 registered，I1 79/79、I2 1≤1、I3/I7 54、I9 79 | ADR-252 + perf_thresholds.toml:499-506 |
| U-14 前置 | `check_ref` 两次 `to_string()` | ✅ **已抹平**：`check_ref` 直传借用至 `validate_command_verdict`（但生产仍零调用，见 F-04） | sandbox_gate.rs:166-180 |
| C-04 并发 | PlanRunner 只读层整层串行 | ✅ **已变化**：内置三工具声明 ReadOnly 后同层只读计划进有界并发（Semaphore=8）；未知能力仍保守串行 | toolplan_runner.rs:136/246-253/286-324/405-430 |
| C-09 | `mcp call` args 静默丢弃 | ✅ args 三层透传（Prepare 帧 `#[serde(default)] args`）+ superposition 真实传输错误映射 + stdio 显式 warn 跳过 | ADR-253；mcp.rs:259-261；participant_client.rs:72-84/733-746/772-783 |
| C-09b | MCP 配置默认三条幽灵 server | ✅ 收口为空默认 + 测试契约迁移 | ADR-255 |

### 1.3 未闭环项总览（详见 §3/§4）

- **上游 W1~W6 波次主体全部未动**：F-01/F-02/F-13/F-03/F-04 逐项确证仍开放（§3）。
- **W0 被推迟的三项治理债未动**：F-06（md 不入库）、F-07（事件计数三账漂移）、F-08（coverage floor 缺行）。
- **上游方案未覆盖的新债务（N 系列）**：§4.3。

---

## 2. 已实现功能清单与闭环状态核验表（C 系列现势版）

> 上游方案 §2 的 15 项（C-01~C-15）全部复核。状态：✅ 已闭环 / ◐ 部分闭环。所有行号为 HEAD c6eb9b6 现值【实测】。

| # | 功能域 | 关键文件:行号 | 状态 | 现势证据与增量 |
| --- | --- | --- | --- | --- |
| C-01 | 四环强制派发链 | `gqep-executor/src/tool_runtime/{mod,runtime,chain,dispatch,sandbox_gate,inproc,batch}.rs`；L0 `nexus-contracts/src/spec/tool_dispatch.rs:22/:51/:84/:91/:120` | ✅ | L0 面无新增类型（149 行）；`BorrowedToolDispatcher` 在 gqep-executor dispatch.rs 非 L0 |
| C-02 | 组合根单装配点 | `chimera-cli/src/composition.rs:275-282` | ✅ | `BuiltinToolExecutor` 唯一 delegate；`exec_plan.rs:358-361` 明确复用不再局部装配 |
| C-03 | 生产传输环（内置只读工具） | `chimera-cli/src/exec_plan.rs:43-82` | ✅ | `text.echo`/`fs.read`(≤256KiB)/`fs.list`；**增量**：`BUILTIN_READONLY_TOOLS` 能力声明（:52）+ `capability_of`(:70-74) + `declared_tools`(:76-81) |
| C-04 | 计划执行与三层超时 | `exec_plan.rs:188-220/:257-262/:297-314/:275-289`；`toolplan_runner.rs:115-158` | ✅ | **增量**：只读层有界并发真生效（Semaphore=8）；写层串行判据改为「有效副作用 != ReadOnly」（:424-430，堵 F-A21） |
| C-05 | 批量有界并发派发 | `batch.rs:48-70/:190`（钳制 [1, MAX_GATHER_FANOUT]） | ✅ | BatchLimits 默认 8/30s/120s（:63-69） |
| C-06 | 批量归因快照 | `batch.rs:144-158/:296-305` | ◐ | **未变**：仍仅 `tracing::debug!`，注释自陈零指标依赖 → F-03/U-15 |
| C-07 | TuiAction 请求-回执闭环 | `action_orchestrator.rs:83-126`；event-bus types.rs:1739/1776/1786/1805 | ✅ | `TuiActionProgressed`(:1776) 仍无生产发布点（F-09 预留） |
| C-08 | CLI 启动分段与按需配置 | `main.rs:53-131/:218-229`；`spec.rs:116` | ✅ | `cli_startup_e2e` 982µs（L4 快照，W0 对拍差 2.1%） |
| C-09 | `mcp call` 显式联网 | `commands/mcp.rs:48-92/:228-281`；`participant_client.rs`（1410 行真实 TCP） | ✅ | **增量**：args 透传（ADR-253 D1）+ superposition 注入客户端真实传输（D2）+ stdio warn 跳过（D3）；SSRF 双道（server_registry.rs:194/:281 + participant_client.rs:464-472 DNS rebinding 校验） |
| C-10 | 遥测周期上报 | `nexus-telemetry/src/report.rs:75-146` | ✅ | 不变 |
| C-11 | 性能判据与门禁 | `perf_thresholds.toml`（79 键/54 run_bench）+ `bench_check.yml` heredoc | ◐ | toolplan 四键已入双源（0a9aee5）；**残留**：heredoc PATH_KEYWORDS 78 vs TOML 79（缺 hnsw_tiered）→ N-3 |
| C-12 | 回滚开关台账 | ADR-242 §3 | ✅ | 不变；安全层无开关（§3.5） |
| C-13 | 会话存储写链 | composition.rs:405-406；nexus-app-server/server.rs:524 | ✅ | 写面 WIRED；读面 DORMANT（U-08） |
| C-14 | doctor 探针异步化 | `commands/doctor/probes.rs` | ✅ | 不变 |
| C-15 | 权限三态 fail-closed | `permission.rs:103-140` | ✅ | **增量发现**：生产调用全部经 `permission::confirm` 间接到达（agent/chat/mcp/llm/quest 5 处）；`run.rs:106/:128` 的 `_perm` 形参**未使用**（W2 接线点） |
| **C-16（新增）** | 执行器能力表与路由诚实性 | `toolplan_runner.rs:55-67/:399-412`；`runtime.rs:36-47/158-165/227-239`；`exec_plan.rs:43-82` | ✅ | ADR-254：三级取值（计划声明 > 能力表 > 未知保守串行）；显式 Write 压过能力表；5 条结构测试锁定（toolplan_capability_table.rs） |
| **C-17（新增）** | MCP 配置诚实化 | `nexus-core/src/config.rs`（mcp.servers 空默认） | ✅ | ADR-255：默认空 servers + 协议边界声明；`mcp list` 空提示 |

### 2.1 功能不削减的实证基线（继承上游 A5 + 现势增量）

上游收官报告 §6 六判据 + **新增第七判据**：能力声明路由的行为变化仅发生在「同层只读计划」——由 `toolplan_capability_table.rs` 5 条判据 + `toolplan_concurrency_proptest.rs` 四条不变量锁定，写语义零变化（显式 Write 与未知能力仍串行）。

---

## 3. 未闭环功能点清单（F 系列 v2，五元组）

> 五元组 = 所属模块 / 关联文件:行号 / 影响范围 / 阻塞状态 / 前置依赖。

| ID | 功能点 | 所属模块 | 关联文件:行号 | 影响范围 | 阻塞状态 | 前置依赖 | 本轮 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **F-01** | 策略规则声明式配置源缺失：`tool_runtime_policy()` 硬编码 `(Vec::new(), PolicyAction::Allow)` | chimera-cli 组合根 / nexus-core | `composition.rs:169-176`（硬编码体 :174-175） | 策略环生产形同虚设（永远放行） | **未闭环，本轮必闭** | A9 双文件 + G-68/G-69 parity | ✅ W1 |
| **F-02** | `run --execute` 恒 `NotImplemented`（退出码 2，零副作用）；**两处**独立拒绝分支（E-1） | chimera-cli `run` | `commands/run.rs:74-78`（EXECUTE_REFUSAL_MSG，ADR-244 后文案已改「本命令路径不消费工具运行时」）+ `:109-111` 与 `:131-133` 两处 `if execute` + `:261-286` 测试（现 **5 条文本断言 + 2 条类型断言**，:272 新增 "composition root"） | `run` 无法触发任何工具执行 | **未闭环，本轮必闭** | A8 稳定前缀；C-02/03/04；D1 三态 | ✅ W2 |
| **F-03** | P-13 并行归因指标未接入 nexus-telemetry | gqep-executor → nexus-telemetry | `batch.rs:296-305`（仅 debug!）；`ToolBatchAttributed` 事件**不存在**（types.rs grep=0） | 4.4× 收益运行期不可观测 | 未闭环（W3） | W2 数据源；BusLogger spike（composition.rs:225-230） | 阶段二 |
| **F-04** | 链内全借用化未完成 | gqep-executor / seccore | `chain.rs:246-261` `dispatch_borrowed` 仍 `to_owned_request()`→`head.dispatch(&owned)`（:257-258）；`check_ref` 生产零调用（唯一调用 tests/tool_runtime_borrowed_equivalence.rs:111） | 热路径 2 次分配；借用化仍是接口 veneer | 未闭环（W4）——**范围收窄**：seccore 已有 `validate_command_verdict` 借用直传（sandbox_gate.rs:173） | ToolDispatcherRef trait；D2 | 阶段二 |
| **F-06** | 可回溯性缺口：`adr_index.md`、`docs/release/` 7 文件、工具链系列报告全部不入库 | 治理 / git | `.gitignore:67-76`（仅 3 条豁免）；`git ls-files`：adr_index=0 / docs/release=0 / docs/reports=23（全为 json/txt/DOC-AUDIT-FACTS） | CI 干净克隆无 ADR 索引载体 | **未闭环，本轮 W0.4-r 必闭（最小集）** | CI 授权（已获裁定 ③） | ✅ W0.4-r |
| **F-07** | 事件计数三账漂移：`VARIANT_COUNT_LOCK=154`（variant_count_test.rs:49）= `doc_count_freeze.txt:25`（154）≠ `event_ownership_baseline.txt:36`（total 152） | 治理台账 | 三文件 | 新增事件时机器门基线歧义 | 未闭环（X2 与 W3 同批对齐） | `check_event_ownership.py --emit` 机器再生 | 阶段二 |
| **F-08** | coverage_per_crate_floor.toml 缺 `gqep-executor` 行 | 覆盖率 | 该文件（[[crate]] 仅 6 行：event-bus 55/seccore 60/parliament 55/quest-engine 55/nexus-core 50/chimera-cli 45） | 本轮核心 crate ≥85% 验收无据 | 未闭环（X2；出数点=CI coverage.yml nightly） | CI 产物 | 阶段二 |
| F-09 | `TuiActionProgressed` 无生产发布点且不带 request_id | event-bus / TUI | types.rs:1776；event_ownership_baseline 零生产构造 | 长时动作无进度反馈 | 设计性预留（P4 裁决） | 独立 ADR | 阶段三 |
| **F-10** | 真实外部执行器仍缺：BuiltinToolExecutor 仅 3 个只读工具；`mcp call` 是独立命令面（不经四环）；`mcp serve` NotImplemented | chimera-cli / mcp-mesh | exec_plan.rs:57-67；mcp.rs:197-204 | 工具链能力面限于只读文本/文件；工具执行与 MCP 两面未打通 | 未闭环（**全景 P1 必闭**） | ADR-256；W1 策略配置（规则可定向 mcp.*）；ADR-253 边界裁决 | 阶段三 |
| F-12 | 已知 TODO / 诚实失败 | 多模块 | `gqep-executor/src/lib.rs:119`（RuntimeAuditor TODO）；`action_orchestrator.rs:259-261`（task.* 尚未实现→诚实 TuiActionFailed）；`chat.rs:551` unreachable! | 局部功能不可达（fail-visible 非 fail-silent） | 部分 P4 裁决 | 逐项 | 阶段三 |

### 3.1 四类缺口自查（对应用户要求 1 的细化，现势增量）

- **异常分支**：`run --execute` 现在完全无执行分支（F-02）；W2 需补全超时/取消/预算/退出码/审计五类 + 非交互 `Unknown→Deny` 保留。
- **边界条件**：`fs.read` ≤256KiB 与 `safe_relative_path` 拒绝四类（exec_plan.rs:136-156）不变；**新增**：`undeclared_parallel` 阈值仍为改前上界 160ms 未收紧（N-1）；`ExecPolicyToolExecutor` 单独作 PlanRunner executor 时不转发能力表（fail-closed，ADR-254 已登记）。
- **错误处理**：配置解析失败仅 `warn` 回退缺省 `Allow`（F-01）；`coverage_gate.py` 本地 EXIT=2（E0463）标不可判定（F-08）。
- **用户交互**：`run.rs` 的 `_perm` 未使用（W2 接线）；`permission::confirm` 是唯一交互入口（:221-251）；`mcp call` permission 确认在 :238-244。

---

## 4. 依赖接线关系全景与未接线链路（U 系列 v2 + N 系列）

### 4.1 主线调用链（✅ 已接线部分，现势）

```
main.rs:87 Cli::parse → spec::needs_full_config → config::load (Figment 14 段)
  → build_runtime → composition::build()
      ├─ composition.rs:275-282 ToolRuntime::with_optional_delegate(Some(BuiltinToolExecutor), rules=vec![], Allow)
      ├─ composition.rs:230 bus.set_logger(BusLogger::new("chimera-cli"))   ← Prometheus 计数器已挂，导出面未接（U-20）
      ├─ composition.rs:285 spawn_critical_subscriber(&bus)
      └─ AppContext { tool_runtime: Arc<ToolRuntime>, ... }
  chimera exec --plan → exec_plan.rs:336-396
      → CountingExecutor → OpTimeoutExecutor → RuntimeToolExecutor → ToolRuntime::execute（单次）
      → PlanRunner::new(...).with_concurrency(8)
          ├─ 只读层（BUILTIN_READONLY_TOOLS 声明 ReadOnly）→ exec_layer_concurrent（Semaphore=8）✅ c6eb9b6 起
          └─ 写/未知层 → exec_layer_serial（保守 fail-closed）
  dispatch_batch / ToolRuntime::execute_batch（runtime.rs:202-208）
      → 生产零调用【实测穷举：同名命中均为 rusqlite Connection::execute_batch / chimera-mas BatchExecutor】 ← F-13
  TuiActionRequested → action_orchestrator.rs:136-319 route_action（全分支串行 await，无 join!）→ 回执原样 request_id
```

### 4.2 未接线链路清单（U 系列 v2）

| ID | 未接线链路 | 所属模块 | 关联文件:行号 | 阻塞状态 | 处置 |
| --- | --- | --- | --- | --- | --- |
| U-01 | mca-gateway 真实请求通路 | L10 | `--features mca` 装配态 | GATED（ADR-177 长期） | 影响面，不接线 |
| **U-02** | **nexus-subagent 运行时** | L7/L10 | `nexus-app-server/src/subagent_engine.rs:1/:35`（GATED-UNWIRED 自述）；nexus-subagent lib.rs 契约 CONSUMERS:1 | 确证未接线 | **P2 全景接线** |
| **U-03** | **nexus-hook 生命周期 Hook 整链** | L9 | `nexus-hook/src/lib.rs:1-8`（PRODUCERS:0/CONSUMERS:0）；event_bridge.rs:93-104 register_all 仅 `#[cfg(test)]` | 确证未接线 | **P3 全景接线** |
| U-04 | r2_island_repayment 六条 optional 边 | L10/L6 | gated_wiring_baseline.txt:10-17 | GATED（ADR-042 冻结） | 影响面，不接线 |
| **U-05** | VoteCastRequested 双向控制通道 | TUI | event_loop.rs:1359-1369（刻意不发） | 设计性预留 | P4 三态裁决 |
| **U-06** | slash 54 条 → ActionRegistry id 级映射 | TUI | slash_registry.rs:102-548（54 条 action_id: None） | 未接线 | P4 三态裁决 |
| **U-07** | TuiChatSubmitted.slash_command 只写不读 | event-bus | types.rs:1831-1834（恒 None） | 确证未接线 | P4 三态裁决 |
| **U-08** | session-store 读/回放面 | L3 | session-store/src/lib.rs:18-25 自陈 DORMANT | 写面 WIRED / 读面 DORMANT | P4 三态裁决 |
| **U-09** | pvl-layer produce→verify→feedback 反馈闭环 | L7 | concurrency.rs:22-25 死码；verifier.rs:383/399/425 feedback_rx 丢弃 | 确证未接线 | P4 三态裁决 |
| U-10 | 18 零生产构造点 + 15 不可达事件 | event-bus | event_ownership_baseline.txt:12-35 | **治理门明禁造假** | 保持诚实账本，不为绿造构造点 |
| **U-11** | RewardSignalReported 发布链 | L6 omega-learner | omega-learner/src/lib.rs:151 | 确证未接线 | P4 三态裁决（RL 闸门期） |
| U-12 | EvolutionOrchestrator::step placeholder | chimera-cli | composition.rs:78-83 | feature 门内预留 | ADR-042 解冻后 |
| **U-20** | BusLogger Prometheus 计数器 → /metrics 导出面 | event-bus/chimera-cli | composition.rs:225-230（注释自陈导出面为后续增量）；event-bus/src/logging.rs（Family<Counter/Histogram> per-instance Registry） | 确证未接线（计数器已装） | **W3 spike 首选落点 + P4 终态裁决** |

### 4.3 新发现债务（N 系列，上游方案未覆盖）【实测】

| ID | 债务 | 证据 | 处置 |
| --- | --- | --- | --- |
| **N-1** | ADR-254 §6 端到端判据「待填」未回填；`undeclared_parallel=160ms` 未按 ADR-252 D3 收紧至 ≈38ms（实测 declared c8=14.9ms / undeclared 改前 c8=63.8ms） | perf_thresholds.toml:104-116；ADR-254 §6 | X2：c6eb9b6 后重跑 `toolplan_layers` bench 取实测 median → ×2.5 收紧（只收紧不放宽） |
| **N-2** | adr_index.md 缺 ADR-254 登记行（索引覆盖到 255 但 254 无行） | adr_index.md:429-433 | X2：补登记行（随 F-06 豁免后入库） |
| **N-3** | heredoc PATH_KEYWORDS 78 vs TOML 79（缺 hnsw_tiered）——该 2ms 阈值在 legacy 断言路径恒空转（M44 同型幽灵门残留一半） | bench_check.yml:194/:303 有键、:404-523 段无键 | W5.0 双源治理补齐（CI 已授权） |
| **N-4** | bench_check.yml 头注释计数过期（「5 个 benchmark」/「47 个 run_bench 别名」vs 实际 54） | bench_check.yml:11/:68-70 | X2 订正 |
| **N-5** | bus.rs:2632-2633 注释仍写「17、21」vs 常量 18/22（:62/:69/:3613 断言 18） | bus.rs | X2 订正 |
| **N-6** | `ExecPolicyToolExecutor` 未覆写 capability_of/declared_tools（走 trait 默认 None/空集）——单独作 PlanRunner executor 时能力表静默失效（fail-closed 串行） | exec_tool_executor.rs:35-55 + runtime.rs:39-46 注释自陈 | 登记；P1 一起评估是否在装饰器内转发（ADR-254 残留清单） |
| **N-7** | run.rs:106/:128 `_perm: &PermissionCtx` 形参未使用 | run.rs | W2 接线点（转正使用） |
| **N-8** | `unreachable!("registry.get 已校验,此处不可达")` chimera-cli src 唯一 panic 宏命中 | chat.rs:551 | 登记不动（防御性不变式，语义正确） |
| **N-9** | `.trae/specs/optimize-toolchain-dispatch-parallelization/` 台账三件套不入库（`.gitignore:78 .trae/`） | tasks.md 21/21 [x]、checklist.md 51/51 [x] | 随 F-06 一并评估豁免（X2 裁决，默认不入库——规格属过程文档） |

### 4.4 事件总线并行化基线（现势，继承上游 §4.3 + 订正）

`bus.rs:62 CRITICAL_MPSC_VARIANTS=18`、`:69 CRITICAL_TOTAL=22`（**N-5：:2632-2633 注释漂移待订正**）；跨 await 持锁豁免 9/9 顶格（G-27）；`rayon` 独立全局池（num_cpus-2）；**本轮不得新增第 10 条豁免**。事件变体 154（types.rs 2825 行）；`Tool|Exec` 前缀变体仅 3 个：`ToolsRouted`(:491)、`ExecutionCompleted`(:514)、`ChtcToolCallReceived`(:1186)。

---

## 5. 波次设计

### 5.0 执行顺序总表

```
【本轮批准后立即执行】
W0.4-r（F-06 最小集：5 md 豁免 + ci.yml 断言 + gate_manifest 登记 + 原子提交）  ← 串行前置
 └─ W1 策略配置源（F-01/U-16/U-19；TDD；1 commit + ADR-246）
     └─ W2 run --execute 闭环 + execute_batch 首个生产调用方（F-02/F-13/U-13/U-17；TDD；1 commit + ADR-247）
【阶段二（后续会话，按本方案执行）】
W3 P-13 归因指标（F-03/U-15；BusLogger spike 先行）
W4 链内全借用化（F-04/U-14；范围已收窄）
W5 并行化深化 6 点 + 阈值双源治理（N-3）
W6 收尾验收 + ADR-246~251 复核 + 收官报告 + 回滚演练
X2 治理债收口（N-1/N-2/N-4/N-5 + F-07 + F-08；无新 ADR，属既有 ADR 决策的执行）
【阶段三：全景接线（用户裁定 ② 扩展范围）】
P1 McpToolExecutor 接入四环传输环（F-10 主体；ADR-256）
P2 nexus-subagent 装配接线（U-02；ADR-257）
P3 nexus-hook 生命周期接线（U-03；ADR-258）
P4 其余 U 项逐项三态裁决（U-05/06/07/08/09/11/20 + F-09/F-12；ADR-259 汇总）
```

并行开发（不同 crate/文件）：W1 测试与模板、W3 Sink 契约与消费者、W5 的 5.1/5.3/5.4/5.6。必须串行：W0.4-r、W1→W2（策略语义依赖）、W3 七点位单 commit、组合根单装配点、阈值双源对拍、P1 依赖 W1+W2。

### 5.1 W0.4-r · F-06 可回溯性最小集（本轮执行）

1. **方案文档先行**：本文件落盘（它是豁免对象之一）。
2. `.gitignore` 增 5 条精确豁免（不放宽全局 `*.md`）：
   - `!docs/architecture/adr_index.md`
   - `!docs/release/release_guide.md`
   - `!docs/reports/toolchain-parallel-optimization-closure-2026-11-06.md`
   - `!docs/reports/toolchain-w0-progress-2026-11-06.md`
   - `!docs/reports/toolchain-parallel-optimization-w7r2-panorama-plan-2026-11-07.md`（本文件）
3. `ci.yml` 增步骤（**与 W0 报告 Q-B 建议的偏差及理由**：ADR-234 路径暂不纳入断言——其正文补建因 ADR-229~236 八编号散文引用链被 W0 报告裁定推迟至随在途分支整批合并，先断言必红；待阶段二补建后扩展断言并在此文件登记）：

```yaml
      - name: Traceability assertion (tracked docs, F-06/W0.4-r)
        run: |
          git ls-files --error-unmatch docs/architecture/adr_index.md
          git ls-files --error-unmatch docs/release/release_guide.md
          git ls-files --error-unmatch docs/reports/toolchain-parallel-optimization-closure-2026-11-06.md
          git ls-files --error-unmatch docs/reports/toolchain-w0-progress-2026-11-06.md
          git ls-files --error-unmatch docs/reports/toolchain-parallel-optimization-w7r2-panorama-plan-2026-11-07.md
```

4. **G-137 parity 联动**：ci.yml 新步骤须在 `scripts/gate_manifest.toml` 同步登记（跑 `check_ci_manifest_parity.py` 验证 matched 数增长、unmatched=0）。
5. **原子同批提交**（单 commit）：逐文件 `git add` 5 个 md + `.gitignore` + `ci.yml` + `gate_manifest.toml`；提交后跑 `python scripts/audit_commit_coupling.py`（防 REF_UNTRACKED 升级）+ parity + `run_gate_manifest.py`（本地 wsl 黑名单则标不可判定，引 CI）。

### 5.2 W1 · F-01/U-16/U-19 策略规则声明式配置源（本轮执行）

> **TDD 先行**：先写失败测试 `crates/chimera-cli/tests/tool_policy_config.rs`，再实现。测试矩阵：① 缺省（无段/空段）恒 `(vec![], Allow)`；② 显式 Deny 规则使 `ToolRuntime` 对匹配工具返回 `DispatchError::Policy`；③ 非法 action/空 pattern 在配置边界报可读错误（`ConfigError`，不 panic）；④ `serde(default)` 全字段（缺省 Allow）；⑤ 组合根集成（`composition::build()` 装配的 ToolRuntime 对声明规则真实生效）；⑥ env 覆盖 `CHIMERA_TOOL_RUNTIME_POLICY__DEFAULT_ACTION=Deny` 生效。

1. **契约（双文件同批，A9）**：
   - `crates/nexus-core/src/config.rs`：`ChimeraConfig` 增 `#[serde(default)] pub tool_policy: ToolPolicySection`；新类型同文件：
     ```rust
     #[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
     pub struct ToolPolicySection {
         #[serde(default)]
         pub default_action: PolicyActionDef,          // 默认 Allow
         #[serde(default)]
         pub rules: Vec<ToolPolicyRuleDef>,            // 缺省空
     }
     pub struct ToolPolicyRuleDef { pub pattern: String, pub action: PolicyActionDef }  // pattern 必填
     // PolicyActionDef: Allow|Ask|Deny（serde snake_case，映射 seccore::execpolicy::PolicyAction）
     ```
   - `crates/chimera-cli/src/config.rs` `omega_yaml_template()` 同步 YAML 模板段（带注释：缺省 Allow 不削减、Deny 仅显式声明生效）。
2. **parity**：`python scripts/check_config_sample_parity.py --write` 重生成样例；`--selftest`（G-68）+ 真树（G-69）双绿。
3. **映射层**：`ToolPolicyRuleDef → seccore::execpolicy::ExecPolicyRule`（复用 `RulePattern::new` + `ExecPolicy::with_default_action`，execpolicy.rs 现取）；非法 pattern/action 只在配置边界拒绝。
4. **组合根**：`composition.rs:169-176` `tool_runtime_policy()` 改签名收 `&ChimeraConfig`（或配置片段），读 `tool_policy` 段映射；**缺省/解析失败仍返回 `(Vec::new(), Allow)` + `tracing::warn!`**；装配期一次读取，**不做热加载**（运行期切换策略 = 安全能力开关，ADR-242 §3.5 禁）。
5. **bench 联动**：`config_concurrency_bench` 段数 14→15 → 该 bench 组键首跑校准（先实测后登记；禁用旧阈值判红；若组名硬编码段数则同步改名 + perf_thresholds 双源同步）。
6. **ADR-246 落档**：决策要点（双文件同批 / 缺省 Allow 不削减 / 无热加载 / 解析失败 warn 回退）；失效条件（缺省变 Deny 或 parity 未 --write ⇒ 门先红）。
7. **验收**（L1+L2）：`cargo test -p chimera-cli tool_policy` + `cargo test -p nexus-core config` + parity rc=0 + `run.rs:261-286` 全断言零回归 + `cargo clippy -p chimera-cli -p nexus-core --all-targets --jobs 2 -- -D warnings` + `cargo fmt -p <改动包> -- --check`。

### 5.3 W2 · F-02/F-13/U-13/U-17 `run --execute` 闭环 + 批量派发首个生产调用方（本轮执行）

> **TDD 先行**：`crates/chimera-cli/tests/run_execute_e2e.rs`。测试矩阵：① `--execute` + 非交互（无 tty）→ 恒 `NotImplemented`、退出码 2、稳定前缀、零副作用（权限 Unknown fail-closed）；② `--execute` + 交互 Allow（注入 PermissionGate stub / `should_skip_prompt` 路径）→ 真实走 `execute_batch` 成功；③ 交互 Deny → 拒绝（退出码 4 系拒绝语义，按 ADR-060 矩阵现取核对）；④ 未知工具恒 `Err(Transport)`；⑤ 超时（小 BatchLimits）→ 退出码 6 + `OperationTimedOut`；⑥ 取消（CancellationToken）→ 优雅中止；⑦ proptest：随机批量与逐条串行结果逐位等价 + 四段守恒 `is_conserved()`；⑧ **两处分支各一条断言**（E-1：`execute()` wrapper 与 `execute_with_ctx()` 都要覆盖）。

1. **两处分支同时改造（E-1，上游方案遗漏已订正）**：`run.rs:109-111` 与 `:131-133` 均改为先调 `PermissionGate` 三态（接线现成闲置的 `_perm: &PermissionCtx`，N-7）；`Unknown && !interactive → Deny → execute_refusal()`（文案稳定前缀 `execute_not_enabled:` 与退出码 2 不变，零副作用，Deny 不读 stdin）；仅交互且 `PermissionDecision::Allow` 才进入执行。
2. **文案语义更新不破坏断言**：拒绝文案正文更新以反映新语义（原文「本命令路径不消费工具运行时，仅 chimera exec --plan 消费」在 W2 后为假），但 **5 个断言子串逐字保留**（`execute_not_enabled:` / `sandbox` / `composition root` / `chimera exec --plan` / `without --execute`），既有测试零修改。若不可自然容纳 → 回退为仅更新 `:64-73` 注释 + 文案保持原样，在 ADR-247 记录该裁决。
3. **工具来源（D3）**：沿用 Quest 分解结果，把任务映射为 `text.echo` 的 `DispatchRequest` 批量（`serde_json::json!` 构造）；零新 CLI 参数、`Commands` 变体集合不变（A5）。
4. **装配（组合根单装配点）**：`Arc::clone(&ctx.tool_runtime)` → **`ToolRuntime::execute_batch(requests, limits)`**（runtime.rs:202——**首个生产调用方**，关闭 F-13/U-13）；`limits` 从既有 GQEP 配置派生并钳制（E-5：`BatchLimits` 钳制 [1, MAX_GATHER_FANOUT]，0 等价串行），缺省 `BatchLimits::default()`；`exec --plan` 单次路径零行为变化；`python scripts/check_composition_root.py` 必须绿。
5. **预算/取消/超时**：`call_with_budget`（call_budget.rs:91）+ `CancellationToken`（action_orchestrator.rs:162-172 范式）包裹整批；`BatchLimits` 双层 deadline（单操作 30s / 整批 120s）天然兜底；超时发既有 `OperationTimedOut` 事件、错误文本回用户，不静默不伪造成功。
6. **审计与退出码**：按 `BatchOutcome.results` 映射 ADR-060 退出码矩阵（全成功 0 / 拒绝 4 / 超时 6 / 系统错误 3——实施时现取 ADR-060 核对）；逐批结构化 `tracing::info!`（含 succeeded/failed/abandoned/not_started）——**W3 前过渡态，不新增事件变体**（棘轮余量 I2 1≤1 顶格）。
7. **bench（双源登记，CI 已授权）**：新 `crates/chimera-cli/benches/run_execute_batch.rs` 同轮对照「串行 execute vs execute_batch」（1ms 模拟 I/O × 64 请求，参照 4.4×；零延迟并行略慢属预期，ADR-239 D6）；**先实测后登记** `runx_*` 前缀键（第五前缀族语义，避 I2 子串冲突；登记前跑 `check_perf_redlines.py invariants` 取余量读数）；`[[run_bench]]` + `[thresholds_ns]` + `[path_keywords]` + `bench_check.yml` heredoc 双写 + `run_bench` 行同步；`slo.yml` 过滤串如新增 `#[ignore]` 测试则由 `emit-slo-filter` 重生成。
8. **ADR-247 落档**：决策要点（非交互仍 fail-closed / 稳定前缀不变 / 交互 Allow 才走有界批量 / execute_batch 首个生产调用方 / 同轮 4.4× 对照）；失效条件（命令层自行装配第二份 ToolRuntime ⇒ `check_composition_root.py` 与 `production_code_does_not_bypass_tool_runtime` 先红）。
9. **验收**（L1+L2）：三层测试全绿 + 门禁集（composition root / parity / clippy / fmt 逐包 / perf invariants）+ **全量回归** `cargo test --workspace --no-fail-fast --jobs 2`（基线 ≥ 12065 passed / 0 failed / 89 ignored，收官报告 §3.4）+ 手动冒烟双路径（交互 Allow 执行成功；非交互拒绝退出码 2——本环境无 tty，以集成测试模拟交互面 + 留待用户终端实测）。

### 5.4 W3 · F-03/U-15 P-13 并行归因指标（阶段二）

- **Spike 先行（只读，不落码）**：验证 `BusLogger`（event-bus/src/logging.rs）是否暴露自定义计数器注册 API（`Family<Counter>`）。**可行** → 走计数器路线：`TelemetryBatchSink` 直接登记 `tool_batch_total/succeeded/failed/abandoned/not_started`，**零事件变体新增**（完全绕开六锁/七点位与 VARIANT_COUNT_LOCK 漂移，棘轮风险显著下降）；**不可行** → 回退事件方案：`NexusEvent::ToolBatchAttributed` 七点位单 commit（types.rs 变体 + registry 定级 Normal/System（**不进 Critical mpsc**）+ variant_count 样本 + `VARIANT_COUNT_LOCK 154→155` + severity 分布锁 + doc_count_freeze + telemetry 消费测试），F-07 三账同批对齐为 155。
- **契约（加性，两路线通用）**：`batch.rs` 增 `pub trait BatchAttributionSink: Send + Sync { fn record(&self, a: &BatchAttribution); }`；`dispatch_batch` 旧签名不变（A5），新增 `dispatch_batch_with_sink`；`ToolRuntime` 持 `Option<Arc<dyn BatchAttributionSink>>`，None = 现行为（L1 等价，单点可回退）。
- **发布纪律**：`total == 0` 不发（防刷屏）；发布失败仅 `tracing::warn!` 不反噬主循环；消费者闭合断言锁「会被发布」而非「能构造」（governance 门禁造零消费者构造点被明禁）。
- **组合根注入**：唯一装配点注入 sink；`check_composition_root.py` 断言无第二装配点。
- **验收**：`bus_bench` < 200µs / `cross_layer_bench` < 100µs 退化 ≤10%（超限改采样发布）；六锁全绿（若走事件路线）。

### 5.5 W4 · F-04/U-14/U-18 链内全借用化（阶段二，范围收窄）

> **现势收窄**（相对上游方案）：seccore 已有 `validate_command_verdict` 纯判决借用入口（sandbox_gate.rs:173 已接入 check/check_ref），`check_ref` 的两次 to_string 已抹平。**剩余范围**：① `chain.rs:257-258` `dispatch_borrowed` 仍升级 owned——需 `ToolDispatcherRef` trait（四层各一实现，严守 ADR-241 D3 不另建链：把链内 `next: Arc<dyn ToolDispatcher>` 换为 `Arc<dyn ToolDispatcherRef>` + blanket 适配）；② `check_ref` 生产接线（当前链内 SandboxLayer 用 `check`，borrowed 路径走 owned 链不经 check_ref）。

1. **收益上限论证先行**：`build_full_command_string` 必构造 String + `to_lowercase()` 派生分配（policy.rs:131-157/242/272，E-4）→ 链内分配上限受此约束；静态分析 + 微基准估理论值，<5% 直接按 ADR-241 D5 否决并记录（否决亦为正式结论）。
2. **反转主次**：借用视图成为链主路径，拥有型 `dispatch` 退化为适配器；两条路径同源共享四环实例与判决核心。
3. **等价硬约束**：`tool_runtime_borrowed_equivalence.rs` 64→≥256 cases，独立断言沙箱判决逐位一致（含错误文案）+ 短路顺序不变 + check_ref 不绕过 policy/sandbox。
4. **同轮对照**（唯一允许 L4 判性能的波次）：`dispatch_borrowed_chain/{owned,borrowed}×{c1,c4,c8}`；采纳判据 median 改善 ≥5% 且 CI 噪声区间不重叠。
5. **顺带评估 N-6**：`ExecPolicyToolExecutor` 能力转发（装饰器内实现 capability_of/declared_tools 透传）——加性 API，随本波或 P1 落地。

### 5.6 W5 · 并行化深化 + 阈值双源（阶段二）

| # | 改造点 | 文件:行号 | 手法 | 对照基准 |
| --- | --- | --- | --- | --- |
| 5.1 | `route_action` 单请求内串行 await | action_orchestrator.rs:136-319 | 无依赖分支 `tokio::join!`；批量 `FuturesUnordered` 有界并发 | `dchain_*` 六键；四环短路顺序不可变 |
| 5.2 | 装配期顺序 push ~12 个 JoinHandle | experience_loop.rs:123-317（12 处 push） | 独立订阅者并行 `try_join!` | Critical 旁路订阅须在 AppContext 移出前完成；`cli_startup_e2e` |
| 5.3 | runtime 未设 worker_threads | main.rs:218-229 | `num_cpus` 派生 + `CHIMERA_RT_FLAVOR=auto` 回滚 | `cli_startup_runtime`（multi 783µs） |
| 5.4 | 事件发布批处理 | bus.rs:1165 publish_batch | 同 tick 非 Critical 合批；Critical 18 变体不合批 | `bus_bench`/`cross_layer_bench` |
| 5.5 | ShardingPolicy Off/Auto | spec.rs:27-50、composition.rs:222-224 | 长驻 Auto；one-shot 保持 Off | `cli_startup_*` |
| 5.6 | 同步 FS/阻塞调用 | sync_offload_guard_freeze.txt 12 条 | `tokio::fs`/`spawn_blocking`；不新增第 10 条跨 await 豁免 | G-27 9/9 |

**W5.0 前置**：heredoc/TOML 双源治理——补 `hnsw_tiered` 进 heredoc PATH_KEYWORDS（N-3，CI 已授权）+ `emit-slo-filter` 重生成 + I1/I7/I9 对拍。

### 5.7 W6 · 收尾验收（阶段二）

§6 门禁集逐条执行对拍；ADR-246~251 落档复核 + adr_index 同步；收官报告（含采纳/否决双清单）；ADR-242 台账更新；功能不削减七判据复验；覆盖率 ≥85% 复验（CI 产物）；逐 commit revert 演练。

### 5.8 X2 · 治理债收口（阶段二，无新 ADR——属既有 ADR 决策的执行）

1. **N-1**：c6eb9b6 后重跑 `cargo bench -p gqep-executor --bench toolplan_layers` 取实测 median → `undeclared_parallel` 收紧至 median×2.5（先实测后登记、只收紧不放宽；TOML + heredoc 双写）→ 回填 ADR-254 §6 实测追记。
2. **N-2**：adr_index.md 补 ADR-254 登记行（W0.4-r 后该文件入库，CI 克隆可核）。
3. **N-4/N-5**：bench_check.yml 头注释计数订正（54）；bus.rs:2632-2633 注释订正（18/22）。
4. **F-07**：`python scripts/check_event_ownership.py --emit` 机器再生账本（152→154 对齐；若 W3 已加事件则 155）；禁手改。
5. **F-08**：coverage_per_crate_floor.toml 补 `gqep-executor` 行——**出数点为 CI coverage.yml nightly 产物**，本地 E0463 不可代跑；先以 defaults.floor=45 显式登记占位 + 注释引 CI 产物待回填，或引最新 nightly 实测值（有据才写数，禁编造）。

### 5.9 阶段三 · 全景接线 P1~P4（用户裁定 ②，每波独立 ADR-256+、独立 revert）

#### P1 · McpToolExecutor 接入四环传输环（F-10 主体，ADR-256）

**目标**：工具执行面与 MCP 面打通——`DispatchRequest` 可路由到 MCP 参与者，四环（policy→sandbox→route→transport）完整前置，无旁路。

**技术骨架（ADR-256 编写时终裁）**：
1. **新执行器**：`chimera-cli`（L10）新增 `McpToolExecutor` 实现 `gqep_executor::ToolExecutor`（L7 trait，依赖方向合法：L10→L7 ✓；mcp-mesh 同层 ✓）。命名约定 `mcp.<server>.<tool>`（tool_name 前缀路由；ADR-256 终裁分隔符）。
2. **传输缺省**：**默认 `InProcessClient`（零网络副作用，A2）**；`TcpParticipantClient` 仅经显式配置段启用（配置非 feature flag，不违 A2/不新增 feature）。复用 mcp.rs 既有 SSRF 双道守卫与 permission 确认范式。
3. **能力声明**：`capability_of` 对 mcp.* 返回 `None`（未知副作用 → 计划层保守串行，fail-closed）；`declared_tools` 初期返回空集（路由 fallback）——**诚实优于虚构**（ADR-240 D3 先例）。若后续 `mcp inspect` 可静态枚举只读工具，再逐个显式声明 ReadOnly。
4. **四环前置完整**：policy 环由 W1 的声明式规则天然覆盖（用户可写 `pattern: "mcp.*"` 规则）；sandbox 环对 mcp.* 的 `validate_command_verdict` 行为需 ADR-256 明确（预期：默认 Allow 缺省 + 可被规则 Deny）；route 环由 declared_tools fallback。
5. **组合根装配**：装饰器链 `McpToolExecutor → BuiltinToolExecutor`（或组合 executor），仍经 `ToolRuntime::with_optional_delegate` 单装配点。
6. **与 ADR-253 边界的关系（须在 ADR-256 显式裁决）**：ADR-253 声明的是「CLI `mcp call` 命令面不说 MCP JSON-RPC，走私有 2PC 参与者协议」——P1 不新造 JSON-RPC 栈，**复用同一 2PC 参与者面**，两面共享 participant_client；`mcp serve` 服务端仍 NotImplemented（不在本轮）。
7. **事件面**：执行结果经既有 `ExecutionCompleted`/错误面；如需专属事件，与 W3 路线合并裁决（**不新增第二个事件变体**，上游 12.4 纪律）。
8. **测试**：TDD 三层 + proptest（工具名解析边界：缺 server/缺 tool/非法字符 → Route 错误）；同轮对照（mcp 调用经四环 vs 直连，验证无旁路）；`check_composition_root.py`。

#### P2 · nexus-subagent 装配接线（U-02，ADR-257）

1. **现状**：`nexus-app-server/src/subagent_engine.rs:1/:35` 自陈 GATED-UNWIRED（L10 侧适配器已写、未装配）；`nexus-subagent`（L7）CONSUMERS:1（nexus-app-server）——依赖方向合法，缺的只是组合根装配与驱动。
2. **接线**：composition root 构造 `SubAgentEngine`（复用 `Arc<ToolRuntime>`/`EventBus` 注入）→ 挂接 Quest 委托链路（Quest 分解产出可委托任务时经 engine spawn/join）；CRATE-CONTRACT 头同步（nexus-app-server CONSUMERS 增量、subagent_engine.rs 状态改 WIRED）。
3. **预算**：委托深度 ≤5、并发子代理上限（chimera-mas INV 不变量对齐）；每次 spawn/publish 经事件面（TaskAuction 相关变体现取）。
4. **测试**：TDD（spawn→执行→join→事件回执闭合）；proptest（深度上限、无环）。

#### P3 · nexus-hook 生命周期接线（U-03，ADR-258）

1. **现状**：`nexus-hook` lib.rs 契约 PRODUCERS:0/CONSUMERS:0；`event_bridge.rs:93-104 register_all` 仅 `#[cfg(test)]`——注册机制已写、生产零装配。
2. **接线**：composition root 生产调用 `register_all`（去 cfg(test) 门）→ 生命周期事件（QuestStarted/Completed 等现取变体）发布/消费闭合；契约头更新（CONSUMERS ≥1）。
3. **约束**：订阅须在 spawn 前（broadcast 红线）；Hook 失败不反噬主流程（warn + 计数）。
4. **测试**：TDD（生命周期事件 → hook 触发 → 副作用可观测）；`check_event_ownership.py` 台账同步。

#### P4 · 其余 U 项逐项三态裁决（ADR-259 汇总）

| 项 | 候选处置 | 倾向 |
| --- | --- | --- |
| U-05 VoteCastRequested | 接线（TUI event_loop.rs:1359-1369 去刻意不发）vs 维持预留 | 接线需 L8 落地前提（TUI R5 批次遗留）→ 倾向维持预留 + ADR 记录条件 |
| U-06 slash 54 条映射 | 逐条接线到 ActionRegistry | 工程量大、TUI 域（tui-r5 批次七在途）→ 移交 TUI 批次或 ADR 裁决最小集 |
| U-07 slash_command 字段 | 读取点接线 | 与 U-06 同批 |
| U-08 session-store 读面 | 回放/视图命令接线 | 独立小波；读面 API 已有 |
| U-09 pvl-layer feedback 闭环 | verifier feedback_rx 接线 | 独立 ADR（L7 内部闭环） |
| U-11 RewardSignalReported | 发布链接线 | **RL 闸门期**（AGENTS.md §1.2 ⑥：Rust 侧 only）→ 倾向维持预留 |
| U-20 /metrics 导出 | 导出端点（文件 dump 或本地端口） | W3 spike 后终态裁决；倾向文件 dump（零端口暴露） |
| F-09 TuiActionProgressed | 生产发布点 + request_id | 随 P2/P4 长时动作需要裁决 |
| F-12 task.* 五动作 | 实现或维持诚实失败 | 倾向维持（需 per-task 调度子系统，Phase 3 ADR） |

**纪律**：每项裁决（接线/否决/维持预留）都进 ADR-259 表格；**禁止为让门绿造无消费者构造点**（U-10 教训）；接线项各自带 TDD + 事件台账同批。

---

## 6. 逐阶段验证与验收标准

### 6.0 独立复跑门禁集（每波末必跑；W0.4-r 起含 ci.yml 断言）

```
python scripts/check_perf_redlines.py all / inventory / invariants
python scripts/run_gate_manifest.py            # 本地 wsl 黑名单→标不可判定，引 CI
python scripts/check_ci_manifest_parity.py
python scripts/check_config_sample_parity.py [--selftest]   # W1 起
python scripts/check_composition_root.py       # W2 起
bash   scripts/check_crate_reachability.sh     # 46 members / 38 可达 / 0 孤岛 / 8 GATED
python scripts/audit_commit_coupling.py        # 每次涉及 md 豁免/新增文件的提交后
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings
cargo fmt -p <每改动包> -- --check             # 逐包规避 os error 206
cargo test --workspace --no-fail-fast --jobs 2 # 波次级；基线 ≥12065/0/89
```

### 6.1 每波验收矩阵（证明力依 ADR-234：L1>L2>L3>L4）

| 波次 | 测试 | 量化指标 | 证明力 |
| --- | --- | --- | --- |
| W0.4-r | audit_commit_coupling + parity + run_gate_manifest | 5 路径 `git ls-files --error-unmatch` 全过；unmatched=0 | L1 |
| W1 | tool_policy_config 6 组 + run.rs 断言零回归 | 缺省恒 Allow；G-68/G-69 rc=0；`cli_startup_config` 校准首跑 | L1+L2 |
| W2 | run_execute_e2e 8 组 + proptest 等价/守恒 | 非交互恒拒绝；1ms×64 并行/串行 ≥3×（参照 4.4×）；`runx_*` 阈值先实测后登记 | L1+L2 |
| W3 | 消费者闭合 + proptest 快照 | bus/cross_layer 退化 ≤10% | L1+L2 |
| W4 | 等价 ≥256 cases + 短路顺序 | median ≥5% 且噪声区间不重叠，**否则否决** | L1+L4 |
| W5 | 各点同轮对照 | 每点有同轮对照；不显著则记录不采纳 | L1+L3(+L4) |
| X2 | invariants + ownership --emit | undeclared_parallel 收紧至实测×2.5；三账 =154/155 一致 | L1 |
| P1~P4 | 各波 TDD 三层 | composition root 绿 + 事件台账同步 + 无第二装配点 | L1+L2 |

### 6.2 棘轮余量（新增前必查）

I2 重叠 1≤1（顶格）、I6 dev-only 62（floor 60，余量 2）、I4a cap=1、G-27 豁免 9/9（顶格）、生产 crate 46/46、`calibration_pending` 1/1。W3 新增 1 个事件变体可行但须六锁+七点位同批+TUI 穷尽 match 补臂；任何新增前先跑 `invariants` + `run_gate_manifest` 取余量。

---

## 7. 回滚预案（三层 + 不可回滚项）

| 层 | 手段 | 适用 |
| --- | --- | --- |
| **L1 秒级** | ADR-242 台账环境开关（`CHIMERA_CONFIG_EAGER=1`、`CHIMERA_RT_FLAVOR=auto` 等）；**安全层（policy/sandbox/route/fail-closed）明确不提供开关** | 并行化/启动/配置路径 |
| **L2 小时级** | 每波 1 个独立可 revert commit，逆序撤销；每撤销一个跑 §6.0 | W0.4-r/W1/W2/各波 |
| **L3 天级** | 分支级回退；发布按 CONTRIBUTING.md §6 | 整体下线 |

**逐项回退**：
- W0.4-r：删除 5 条豁免行 + ci.yml 步骤 + manifest 行（已提交的 md 保持 tracked，`git rm --cached` 还原）。
- W1：清空/删除 tool_policy 配置段 → `tool_runtime_policy()` 回 `(vec![], Allow)`；parity --write 重生成。
- W2：**只摘除 Allow 分支执行体**，保留 `execute_refusal()`/稳定前缀/全部断言 → 回到 ADR-244 恒拒绝语义；`execute_batch` 本体保留（本就存在）。
- W4 借用化：按 ADR-241 D5 单点回退（chain.rs 回 to_owned_request）。
- 不可回滚项：四环顺序、fail-closed、缺环构造失败、seccore 6 条不可学习红线——无任何运行时关闭开关；排查读 `DispatchError` 前缀（`policy:`/`sandbox:`/`route:`/`transport:`）。

---

## 8. 架构决策记录（ADR 编号分配）

| 编号 | 主题 | 波次 | 决策要点 | 失效条件 |
| --- | --- | --- | --- | --- |
| ADR-246 | 工具策略规则声明式配置源 | W1 | 双文件同批 + 缺省 Allow + 无热加载 + 解析失败 warn 回退 | 缺省变 Deny 或 parity 未 --write ⇒ 门红 |
| ADR-247 | `run --execute` 闭环 + execute_batch 首个生产调用方 | W2 | 非交互 fail-closed 拒绝、稳定前缀不变；交互 Allow 才有界批量 | 第二装配点 ⇒ composition_root 门红 |
| ADR-248 | P-13 并行归因指标 | W3 | Sink 依赖倒置；BusLogger 计数器路线优先（spike 裁决）；事件路线则七点位单 commit | 只造能构造无发布消费 ⇒ 所有权门红 |
| ADR-249 | 链内全借用化的评估与裁决 | W4 | 反转主次非另建链；≥5% 且噪声不重叠才采纳，否决亦为正式结论 | 判决分叉/短路顺序变 ⇒ 256-case 红 |
| ADR-250 | 可回溯性与台账对齐 | W0.4-r/W6 | 精确豁免 5 路径 + CI 断言；ADR-234 补建后扩展断言（偏差已登记 §5.1） | 干净克隆缺索引 ⇒ CI 断言红 |
| ADR-251 | 并行化深化与阈值双源 | W5 | 每点同轮对照；`runx_`/`bchain_` 前缀族守 I2；heredoc+TOML 双写 | I1/I7/I9 键集不等 ⇒ invariants 红 |
| ADR-256 | McpToolExecutor 接入四环传输环 | P1 | 复用 2PC 参与者面（不新造 JSON-RPC）；默认 InProcess 零网络；能力 None 保守串行；四环前置完整 | 绕过四环直连 ⇒ composition_root/反向断言红 |
| ADR-257 | nexus-subagent 装配接线 | P2 | 组合根装配 + Quest 委托驱动；深度 ≤5 | 委托旁路 ToolRuntime ⇒ 反向断言红 |
| ADR-258 | nexus-hook 生命周期接线 | P3 | register_all 生产化 + 契约头更新 | Hook 失败反噬主流程 ⇒ 违反 fail-visible 纪律 |
| ADR-259 | U 项三态裁决汇总 | P4 | 每项接线/否决/维持预留 + 理由 | 为绿造构造点 ⇒ 所有权门红 |

> 新增 ADR 沿用 `docs/architecture/ADR-*.md` 命名（`.gitignore:71` 白名单自动入库）；`adr_index.md` 同步行随 W0.4-r 入库后可被 CI 克隆核验。

---

## 9. 关键文件清单（相对 `D:/Chimera CLI`）

**本轮将修改**：`.gitignore`、`.github/workflows/ci.yml`、`scripts/gate_manifest.toml`、`crates/nexus-core/src/config.rs`、`crates/chimera-cli/src/config.rs`、`crates/chimera-cli/src/composition.rs`、`crates/chimera-cli/src/commands/run.rs`、`scripts/perf_thresholds.toml`（W2 bench）、`.github/workflows/bench_check.yml`（W2 heredoc）。

**本轮将新增**：`docs/reports/toolchain-parallel-optimization-w7r2-panorama-plan-2026-11-07.md`（本文件）、`crates/chimera-cli/tests/tool_policy_config.rs`、`crates/chimera-cli/tests/run_execute_e2e.rs`、`crates/chimera-cli/benches/run_execute_batch.rs`、`docs/architecture/ADR-246-tool-policy-declarative-source.md`、`docs/architecture/ADR-247-run-execute-batch-closure.md`、`examples/config.sample.yaml`（--write 再生）。

**阶段二/三将修改**（预告）：`crates/gqep-executor/src/tool_runtime/{batch,runtime,chain,dispatch}.rs`、`crates/nexus-telemetry/src/*`、`crates/event-bus/src/{types,registry,bus}.rs`（W3 事件路线时）、`crates/chimera-cli/src/action_orchestrator.rs`、`experience_loop.rs`、`main.rs`、`crates/seccore/src/policy.rs`（W4）、`crates/chimera-cli/src/commands/mcp.rs` + 新 `mcp_tool_executor.rs`（P1）、`crates/nexus-app-server/src/subagent_engine.rs`（P2）、`crates/nexus-hook/src/*`（P3）、`scripts/{event_ownership_baseline.txt,coverage_per_crate_floor.toml}` + `bus.rs` 注释（X2）。

**只读取证**：`crates/seccore/src/{execpolicy,policy}.rs`、`crates/nexus-contracts/src/spec/{tool_dispatch,tool_plan}.rs`、`crates/gqep-executor/src/{toolplan_runner,exec_tool_executor}.rs`、`crates/chimera-cli/src/{permission,spec,call_budget}.rs`、`scripts/check_perf_redlines.py`、ADR-239~255、`docs/reports/toolchain-parallel-optimization-closure-2026-11-06.md`、`docs/reports/toolchain-w0-progress-2026-11-06.md`。

---

## 10. 风险、证明力上限与不确定性

1. **证明力上限**：W1/W2 只到 L1+L2；W4 是唯一允许 L4 判性能的波次；冷启动首轮 29-33% 噪声下限之下差值不构成证据。
2. **本地不可判定门**：`coverage_gate.py --check`（E0463）、`run_gate_manifest.py`（wsl.exe 黑名单）、fuzz（libFuzzer 仅 MSVC）——如实标注「门在清单里、CI 裁决」，不伪造绿。
3. **棘轮余量顶格项**：I2 1≤1、G-27 9/9、I4a cap=1——W2 新 bench 键前必跑 invariants；`runx_` 前缀与既有 79 键无子串关系需机器验证。
4. **交互路径实测限制**：本执行环境无 tty，`run --execute` 交互 Allow 路径以集成测试（注入 PermissionGate/`should_skip_prompt`）+ 代码走查覆盖；真实终端双路径冒烟留待用户实测（诚实登记）。
5. **ADR-234 正文缺口**（M-4）：主检出零载体，四层判据引用双源标注（worktree 原文 + adr_index.md:245 摘要）；补建归阶段二整批。
6. **P 波安全边界**：P1 引入 MCP 工具执行属**能力面扩展**（非削减），但网络副作用面必须默认关闭（InProcess）+ 显式配置才启用 TCP + SSRF/permission/policy/sandbox 全前置——ADR-256 未落档前 P1 不动码。
7. **事件台账成本**：任何新增 NexusEvent 变体 = 六锁同批 + 三账对齐 + TUI 穷尽 match 补臂，成本已计入 W3/P 波。
8. **上游方案与本文件的冲突处理**：E-1（两处分支）已并入 W2；E-6（CRATE-CONTRACT 头）归 W3；本文件 §1.2 的现势订正（check_ref 已抹平等）覆盖上游对应断言。

---

## 11. 与上游方案的追溯关系

| 上游条目 | 本文件处置 |
| --- | --- |
| 上游 §5 W1/W2 | §5.2/§5.3 全量继承 + 现势修正（E-1、_perm、5 条断言现值、check_ref 已抹平） |
| 上游 §5 W3/W4/W5/W6 | §5.4~§5.7 继承 + 范围收窄标注 |
| 上游 §12 E-1~E-7 | E-1→W2；E-2→W3 spike + U-20；E-3/E-4→W4；E-5→W2 步骤 4；E-6→W3；E-7 已复核成立 |
| 上游 Q1~Q4/D1~D4 | 全部继续有效（D3 工具来源、D1 非交互拒绝等） |
| 上游 Q-A~Q-D（W0） | Q-A/Q-D 已闭环（§1.1）；Q-B 由用户裁定 ③ 解锁 → W0.4-r；Q-C（hnsw_tiered）→ W5.0 |
| 用户新裁定 ②（全景） | §5.9 P1~P4 + ADR-256~259（上游范围扩展，非变更） |

---

*本方案依 ADR-234「证据先于断言」编写：§1~§4 全部行号为 2026-11-07 HEAD c6eb9b6 三路并行取证现值；不可判定项显式标注。执行波次时如现势与本文行号漂移，以现场现取为准并在波次报告中登记。*
