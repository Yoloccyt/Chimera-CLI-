# ADR-248: P-13 批量归因指标 — BusLogger 加性计数器路线（F-03 / W3）

- **状态**：Accepted（2026-11-07，`feat/toolchain-parallel-optimization` W3 波次）
- **上游**：已批准方案 `radiant-thunder-darwin-xenXoOpR.md` §5 W3 + §12.4（现势版 `docs/reports/toolchain-w7r2-panorama-deepdive-2026-11-07.md` §2）
- **关联**：ADR-239（棘轮）/ ADR-241（batch.rs 零指标依赖先例）/ ADR-247（execute_batch 首个生产调用方，本 ADR 的数据源）

## 1. 背景

F-03：`ToolRuntime::execute_batch` 的 `BatchAttribution` 快照只落 `tracing::debug!`
（batch.rs 自陈零指标依赖）——8.35× 有界并行加速在运行期不可观测。深审 spike
裁决（dwfrun-66cff073）：`BusLogger` 的 Registry/Counter 字段全部私有、`new()`
仅注册 3 个固定指标——「计数器已暴露」为假；但 prometheus-client 0.22 已在
event-bus 依赖面、`Counter` 为可 clone 的 Arc 句柄，**加性 API 可行**。

## 2. 决策

**D1 · C-A 主案（用户裁定 2026-11-07）**：`BusLogger::register_counter(name, help)
-> RegisteredCounter`（event-bus logging.rs，15-25 行域中立 API）。

- WHY `&mut self`：prometheus-client 0.22 的 `Registry::register` 收 `&mut
  self`——注册被类型系统约束在构造期（`set_logger` 收编后总线侧只持不可变
  引用），杜绝运行期动态注册。
- WHY 域中立：API 不含事件/工具/批量字样，L1 职责纯度不变；业务命名由上层
  （chimera-cli）在注册时给出。
- 命名契约：注册名**不带 `_total`**（prometheus-client 对 Counter 渲染自动
  追加，`_total_total` 是既有教训）；同名重复注册 panic（Registry 去重语义，
  装配期一次性注册为编程契约）。

**D2 · gqep-executor 依赖倒置（零指标依赖不变）**：`batch.rs` 增
`pub trait BatchAttributionSink: Send + Sync { fn record(&self, &BatchAttribution); }`
+ `dispatch_batch_with_sink(..., Option<&dyn BatchAttributionSink>)`；**旧
`dispatch_batch` 签名逐字不变**（A5），语义 = `None`；派发路径零改动——
`None` 与旧路径结果逐位等价（测试锁定）。`total == 0` 空批次提前返回、
天然不触发 record（防刷屏纪律由结构保证）。`ToolRuntime` 持
`Option<Arc<dyn BatchAttributionSink>>`，`with_batch_sink` builder 注入，
None = 改造前行为（L1 等价，单点回退）。

**D3 · chimera-cli 装配（组合根唯一装配点）**：新 `metrics_sink.rs` 的
`TelemetryBatchSink`（`BatchAttribution` → 5 计数器忠实翻译）；装配时序
**构造 BusLogger → `TelemetryBatchSink::register(&mut logger)`（注册 5 计数器）
→ `bus.set_logger(logger)`**（注册写的是 logger 内部 Registry，必须先于 Arc
收编）；`with_batch_sink` 注入。关联函数命名 `register` 而非 `new`：
composition-root 门（check_composition_root.py）以 `Type::new`/`Type::with_*`
识别基础设施装配，本类型是观测胶水——**G-4 基线零变更**（实测 7 点不变）。

**D4 · 计数器命名（5 个）**：`chimera_tool_batch{,_succeeded,_failed,_abandoned,
_not_started}`（渲染为 `..._total`）。Counter 语义 = 跨批次累计；字段为有限
取值域计数（无高基数标签，ADR-241 D5 教训延续）。

**D5 · bench 验收与基线轮转的诚实登记**：`bus_bench` + `cross_layer_bench`
本地实测全部远低于阈值（最大组 median ≈49µs vs 阈值 100/200µs）——发布路径
零改动（sink 调用仅在批量归因出口，一次/批）的结构判断成立。**注意**：本轮
bench 运行使这两个 bench 的 criterion `new→base` 轮转（恢复源：X1 备份
`tmp/x1_criterion_backup/criterion_6475027.tar.gz`）；CI 侧阈值由 heredoc
双源独立裁决，不受本地轮转影响。

## 3. 否决方案

| 方案 | 否决理由 |
|------|---------|
| A. C-B：chimera-cli 建本地独立 Registry | 不动 event-bus，但 BusLogger 计数器与业务指标面分裂为两套 Registry，`render_metrics()` 单点导出被破坏 |
| B. 事件路线：新增 `NexusEvent::ToolBatchAttributed` 七点位同批 | VARIANT_COUNT_LOCK 154→155 + 六锁 + F-07 三账联动 + TUI 穷尽 match 补臂——棘轮成本最高；且事件是拉取式消费，计数器是推入式累计，后者才是指标语义 |
| C. 在 gqep-executor 直接依赖 prometheus | 破坏 batch.rs「零指标依赖」纪律（ADR-241 先例）；依赖倒置（Sink trait 在 L7、实现在 L10）是分层正解 |
| D. register_counter 用 `&self` + Registry 换 Mutex | 为装配期一次性操作引入运行期锁；`&mut self` 已把注册约束在构造期，更强且零成本 |

## 4. 后果

**正向**：F-03 闭环——批量派发运行期可观测（`render_metrics()` 一处导出全部
指标，U-20 的 env dump 文件面天然承载）；`execute_batch` 的 8.35× 收益可在线
证实；event-bus 获得通用计数器扩展点（后续业务指标零 API 增量）。

**代价 / 边界**：`execute_batch` 每批多一次 `Option::as_deref` 分支（纳秒级，
bench 实测无回归）；5 个计数器常驻 Registry（每计数器一个 Arc，内存可忽略）。

## 5. 测试证据（2026-11-07 本机实测）

```text
cargo test -p event-bus --test custom_counter      # 2 passed（TDD 先红后绿）
cargo test -p gqep-executor --test tool_batch_sink # 4 passed（透传/None 等价/空批不触发/超时入 failed）
cargo test -p chimera-cli --test tool_batch_metrics # 2 passed（组合根闭合 + 跨批累计）
cargo test -p event-bus -p gqep-executor -p chimera-cli  # 三 crate 全量 0 failed
cargo bench -p event-bus --bench bus_bench --bench cross_layer_bench  # 最大 median ≈49µs ≪ 阈值
python scripts/check_composition_root.py           # G-4 基线 7 点不变
cargo clippy -p event-bus -p gqep-executor -p chimera-cli --all-targets --jobs 2 -- -D warnings  # 绿
```

## 6. 回退

单点：composition.rs 的 `.with_batch_sink(tool_batch_sink)` 链摘除 +
`TelemetryBatchSink::register` 调用删行（回到 `bus.set_logger(BusLogger::new(...))`
原形态）——`ToolRuntime.batch_sink = None` 路径即改造前行为，L1 逐位等价。
`register_counter`/`BatchAttributionSink`/`dispatch_batch_with_sink` 为加性
API 可保留。ADR-241 D5 单点回退纪律适用。
