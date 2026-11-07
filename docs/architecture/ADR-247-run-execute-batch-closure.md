# ADR-247: `run --execute` 闭环 + `execute_batch` 首个生产调用方（F-02/F-13 / W2）

- **状态**：Accepted（2026-11-07，`feat/toolchain-parallel-optimization` W2 波次）
- **上游**：已批准方案 `radiant-thunder-darwin-xenXoOpR.md` §5 W2 + §12 E-1（现势版 `docs/reports/toolchain-parallel-optimization-w7r2-panorama-plan-2026-11-07.md` §5.3）
- **关联**：ADR-239（阈值棘轮）/ ADR-242 §3.5（安全层无开关）/ ADR-244 D1（组合根唯一装配）/ ADR-246（策略配置源）/ ADR-252（bench 台账纪律）

## 1. 背景

ADR-244 时代 `run --execute` 在**两处**独立分支（wrapper `execute()` 与
`execute_with_ctx`）恒返回 `NotImplemented`（退出码 2、零副作用）——上游方案
E-1 实测确认，只改一处会留下半闭环。同时 `ToolRuntime::execute_batch` 的
4.4× 有界并行加速**只在 bench 成立**（全库生产调用点穷举为零；同名命中均为
rusqlite `Connection::execute_batch` / MAS `BatchExecutor` 无关面）——用户
核心诉求「所有可并行环节并行化」在生产路径完全未兑现（F-13/U-13）。

## 2. 决策

**D1 · 授权前置唯一化（E-1 由构造消除）**：授权逻辑收敛到
`execute_with_ctx` 内的 `ensure_execute_authorized`（`create_quest` **之前**，
拒绝路径零副作用——既有测试锁定共享 bus 上零 Quest 事件）；wrapper
`execute()` 不再自带拒绝分支，仅装配后委托。三态语义：

- `Allow`（`--yes`/`--no-permission`）→ 放行，不读 stdin；
- 非交互 `Unknown` → gate fail-closed 降级 `Deny`（`FAIL_CLOSED_DOWNGRADES`
  计数 + warn）→ 稳定前缀拒绝（退出码 2、不读 stdin）；
- 交互 `Unknown` → `confirm_with` prompt（Ctrl+C 竞争 → `UserCancelled`）；
  用户明确拒绝 → `UserCancelled`（退出码 4——主动放弃不是「未实装」）。

**D2 · 稳定契约零破坏**：`EXECUTE_REFUSAL_MSG` 前缀 `execute_not_enabled:`
与既有 5 条文本断言子串（`sandbox`/`composition root`/`chimera exec --plan`/
`without --execute`）逐字保留、既有测试零修改；文案正文语义更新为
「未授权 fail-closed」（原「仅 exec --plan 消费」在闭环后为假）。

**D3 · 工具来源（零新 CLI 参数）**：Quest 分解结果经纯函数
`build_batch_requests` 映射为 `text.echo` 批量（任务描述回显，可端到端验证
输入序）；`Commands` 枚举变体集合不变（A5）。真实工具路由（P1 McpToolExecutor）
扩展此映射，不在本批引入网络/进程副作用面。

**D4 · `execute_batch` 首个生产调用方（F-13 闭环）**：`execute_quest_tool_batch`
经组合根唯一 `ctx.tool_runtime.execute_batch(requests, BatchLimits::default())`
有界并发派发（8/30s/120s 双层超时）；`exec --plan` 单次路径零行为变化；
`check_composition_root.py` 绿（无第二装配点）。

**D5 · 预算/取消/超时**：`call_with_budget`（120s 进程级 + `CancellationToken`
预留）包裹整批，超时发既有 `OperationTimedOut` 事件——与 `create_quest` 同一
防护口径；并发度配置化（从 gqep 配置派生）留待 W5，本批用 `BatchLimits::default()`
单源缺省。

**D6 · 退出码归因（ADR-060 单一真值源，不新造退出码）**：`batch_outcome_to_cli_error`
取首个失败逐条归因——策略/沙箱拒绝 → `PermissionDenied`(5)；路由/非 deadline
传输失败 → `EngineError`(3)；整批 deadline（`BATCH_DEADLINE_PREFIX` 单一真值）→
`Timeout`(6)；全成功/空分解 → 0。上游方案草案中「拒绝 4」系笔误，按矩阵实值 5。

**D7 · 权限门注入式**：`execute`/`execute_with_ctx` 增 `gate: &PermissionGate`
参数——生产入口 `PermissionGate::from_stdin()`（dispatch 单点），测试注入
`PermissionGate::new(interactive)`，不依赖进程 tty（测试在本机交互终端与 CI
行为一致）。`_perm` 闲置形参就此转正消费（N-7 闭合）。

**D8 · bench 登记（`runx_*` 前缀 + I2 教训）**：新 bench
`run_execute_batch`（chimera-cli）经**生产装配入口** `with_optional_delegate`
构造 ToolRuntime，同轮对照串行/并行（64 请求 × {0,1}ms 模拟 I/O）。
2026-11-07 本机 median：1ms 档串行 415.62ms / 并行 49.76ms ⇒ **8.35× 有界并行
加速**（Windows timer granularity 抬高绝对值，同轮比值有效，ADR-252 D5 同源）；
零延迟档 42.01µs / 52.31µs（并行 +24% 属预期编排开销，ADR-239 D6）。
阈值 ms 档 ×2.5、µs 档 ×8（400µs/450µs/1.3s/155ms）。
**教训**：首版组名 `runx_{serial,parallel}_ms{0,1}` 使叶子路径包含 tbatch 宽词
`serial_ms0`/`parallel_ms0`/`serial_ms1`，I2 重叠 4>1 立红；改名
`runx_{serial,parallel}_{zero,1ms}` 后回到基线 1≤1——新键命名必须对全部既有
关键词做双向子串自查，I2 门有牙。

## 3. 否决方案

| 方案 | 否决理由 |
|------|---------|
| A. 新增 `--plan <PATH>` CLI 参数指定工具来源 | 改 `Commands` 枚举变体集合（违反 A5）+ 退出码/参数校验新面；D3 沿用 Quest 分解零参数达成 |
| B. 命令层自行装配第二份 ToolRuntime 跑批量 | 违反组合根单装配点（ADR-244 D1）；`check_composition_root.py` 与反向断言先红 |
| C. runx 关键词用宽词（`runx_` 单键归类四组） | 单键无法区分串/并行腿的各自阈值；叶子全路径是 ADR-252 D2 纪律 |
| D. 保留组名 `runx_*_ms0/ms1` 仅改关键词 | 叶子路径本身含宽词，双向子串关系无法消除；改名是唯一解（I2 实测抓到） |
| E. 非交互环境弹 prompt 或直接执行 | 违反 D1 fail-closed 与「Deny 不读 stdin」红线（ADR-237 三态语义） |

## 4. 后果

**正向**：`run --execute` 从恒拒绝转为「授权即真实执行」；4.4×→8.35× 有界并行
加速首次在生产路径兑现；批量归因结构化日志（W3 事件化前置）；E-1 半闭环风险
由唯一授权点构造性消除。

**代价 / 边界**：`execute`/`execute_with_ctx` 签名扩展（库调用方需补 gate 参数，
编译器强制）；拒绝路径多付一次 AppContext 装配（~1ms，一次性）；空分解 Quest
在 `--execute` 下为合法成功（零请求零结果）。

## 5. 测试证据（2026-11-07 本机实测）

```text
cargo test -p chimera-cli --test run_execute_e2e        # 13 passed / 0 failed（TDD 先红后绿）
cargo test -p chimera-cli                               # 全套绿（218 lib + 全部集成，零回归;
                                                        #  含 run.rs 既有 5 条文案断言与零副作用断言）
python scripts/check_perf_redlines.py invariants        # PASS（I1 83/83、I2 1<=1、I7 55 对、I9 83 键）
python scripts/check_perf_redlines.py inventory         # unknown=0（registered 52→53）
python scripts/check_workflow_yaml_lexical.py           # 11 文件 0 findings
python scripts/check_ci_manifest_parity.py              # unmatched=0
cargo bench -p chimera-cli --bench run_execute_batch    # 同轮对照 8.35×（median 见 D8）
```

## 6. 回退

**只摘除 Allow 分支执行体**：`execute_with_ctx` 的 execute 路径回退为
`execute_refusal()` 恒拒绝（保留 `ensure_execute_authorized` 骨架或整体回退至
两处 `if execute` 拒绝），保留 `EXECUTE_REFUSAL_MSG`/稳定前缀/全部断言 → 回到
ADR-244 语义；`execute_batch` 本体与 `build_batch_requests`/归因纯函数可保留
（加性 API 无行为）。bench 台账（TOML + heredoc 双源）逆序摘除，复跑
`check_perf_redlines.py invariants`。安全层无任何回退面（四环顺序/fail-closed
不可降级）。
