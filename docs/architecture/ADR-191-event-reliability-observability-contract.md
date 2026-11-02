# ADR-191：L1 事件可靠性契约（Critical 持久化 + 背压降级 + 可观测底座）

- **状态**：✅ Accepted（2026-09-25；核心决策 D1-D4 均已实现并经本地 cargo 实证：D1 挂载+指标端到端测+5c 文件导出、D2 可观测面(logger+E-3)、D3 告警链 e2e 4 passed、D4 FileWalCriticalSink 含重启重放/损尾行韧性 4 测；另 workspace check --all-targets 净 + --lib 41 crate 全绿。残留一项为实施后置而非本 ADR 决策未定：D4 真进程 SIGKILL e2e（需 CI）；D2 降级策略已于 2026-09-26 以 `DownsampleUnderPressure` 策略落地（见 D2 进度段））
- **关联**：ADR-054（内环）· ADR-160（可达性棘轮）· ADR-187（依赖治理）· ADR-190（门失效化范式）
- **动因**：四维深审（`docs/reports/arch-quality-deep-review-2026-09-25.md`）§2/§4 实测的三处
  "机制在、生产零接线"缺口——L1 指标恒零、Lagged 静默丢弃、Critical 无持久化。本 ADR 定契约与
  边界，Phase 1/2 代码据此实现（D1 已本地 cargo 编译+过测；bash 门不可跑但 cargo 经管道可跑，见深审报告 §8）。
- **不变量（不得破坏）**：`#![forbid(unsafe_code)]`（41/41）· Critical mpsc 旁路全序投递语义 ·
  `at-least-once` 观测（sink/重复无害、丢失才有害）· publish 热路径不阻塞不 panic（§4.4 红线）。

---

## 1. 决策与实现契约

### D1（Phase 1 O-1）L1 指标接线 + 出口
- **现状**：`bus.rs:181 logger: Option<Arc<BusLogger>>`，`new()` 置 `None`（`bus.rs:461`）；
  `with_logger(capacity, logger)`（`bus.rs:484`）/ `set_logger(&mut self, logger)`（`bus.rs:689`）
  生产零调用；`BusLogger::new(module_id)`（`logging.rs:157`）、`render_metrics()`（`logging.rs:209`）已备。
  ⇒ `logging.rs` 全套 Prometheus 计数器生产从不构造。
- **契约**：组合根 `chimera-cli/src/composition.rs`（现 `EventBus::new()`，约 :140）以
  `let mut bus = EventBus::new(); bus.set_logger(BusLogger::new("chimera-cli"));` 挂载
  （须 `use event_bus::logging::BusLogger;`——实现时以 lib.rs 实际 re-export 路径为准）。
- **出口**：新增一条**非阻塞**指标导出（二选一，实现时择轻者）：
  (a) 周期性把 `bus.logger().map(|l| l.render_metrics())` 落 `metrics_snapshot_path` 文件；或
  (b) `nexus-app-server` 加 `/metrics` GET 路由返回该文本。**不得**在 publish 热路径同步导出。
- **进度（2026-09-25）**：挂载步已落地——composition.rs `build()` 改 `let mut bus = EventBus::new(); bus.set_logger(BusLogger::new("chimera-cli"));`（`use event_bus::{logging::BusLogger, EventBus}`）；`check_composition_root`/`impl_maturity` 门仍 rc=0。**本地 cargo 已验**：`cargo check -p chimera-cli` 零 error + `build_attaches_bus_logger_for_observability`（logger Some）+ `metrics_test::test_publish_through_attached_logger_reflects_in_metrics`（bus.publish→render_metrics 计数可见，7 passed/0 failed）。待：`/metrics`/文件导出（D1 后半，需暴露触发点）。**已补**：experience_loop 5c 以 `CHIMERA_METRICS_DUMP_PATH` env opt-in 周期(5s) `tokio::fs::write` render_metrics 落盘（未设=零常态变化）；`experience_loop_closure_e2e` 4 passed/0 failed。
- **验收（CI）**：启动后 e2e 断言 `render_metrics()` 含 `nexus_event_publish_total` 且 > 0；
  新增"logger 为 Some"装配自检（防回潮）。

### D2（Phase 2 E-2）Lagged 语义升级：静默丢弃 → 可观测 + 降级
- **现状**：`bus.rs:1429-1464 recv()` 遇 `Lagged` 仅 `lagged_count.fetch_add` +（logger 在时）
  `log_slow_consumer_dropped` + 返回 `Err(SlowConsumerDropped)`；**不发布** `NexusEvent`；
  `backpressure.rs:79-120` 的 `SlowConsumerDetector`/`BackpressurePolicy` **生产零接线=死码**；
  `backpressure_stats()`（`bus.rs:1276-1290`）生产零消费者。logger 恒 None ⇒ 连日志都没有。
- **契约**：
  1. Lagged 分支**除计数外发布 `NexusEvent::SlowConsumerDropped{ subscriber_id, lag, dropped_total }`**
     （事件已存在于 types.rs 清单；发布走普通 broadcast，非 Critical，不进 mpsc，不破全序）。
  2. 把 `BackpressurePolicy` 接入 publish：发送前水位（`bus.rs:411-425` 已探 `queued > cap*3/4`）
     超阈时按策略对**非 Critical 广播**降采样/DropOldest，Critical 恒不丢（红线）。
  3. 定义 `lagged_ratio` 阈值 → `publish` 侧 `NexusEvent`（如 `MemoryNearBudget` 同族压力事件）或
     经 D3 的 Monitor 出口告警。
- **验收（CI）**：`cargo test` 断言 Lagged 后能 `recv` 到 `SlowConsumerDropped`；
  `bus.rs:1429-1464` 与 `backpressure.rs` 单测不重复；`--nocapture` 无热路径阻塞。
- **进度（2026-09-25）**：D2 可观测面已闭合——O-1(logger 挂载后 recv 内 `log_slow_consumer_dropped` 真实落盘) + E-3(monitor 周期拉 `backpressure_stats` 并 publish `SlowConsumerDropped`)；避免在 recv 内直接 publish 的自反馈/死锁风险。**余**：`BackpressurePolicy` 接入 publish 驱动非-Critical 降采样（降级策略）需单独设计。
- **进度（2026-09-26，D2 残留闭合）**：`BackpressurePolicy::DownsampleUnderPressure` 已实现并接入 publish 热路径（四维深审 E-2 用户裁决实施）：千分比滞回状态机（`pressure_latch_next` 纯函数核，proptest 死区不翻转/单调性双不变量）+ 锁存期内非 Critical 按「每 every 条广播 1 条」降采样 + `downsampled_total`/`pressure_engaged` 运维可见性；`with_policy` 构造器，**默认策略零行为零性能变更**（params=None 单判早退；单 publish bench 332ns 与历史同量级；event-bus lib 294/0 + 全 integration 零回归 + 新 e2e 3/3 含降级量确定性断言 112）。Critical 红线：旁路 mpsc 与降采样判定双重全量（e2e 断言）。未新增事件变体（复用 `SlowConsumerDropped`，双清单零触碰）。

### D3（Phase 1 O-3 + Phase 2 E-3）压力/Critical 丢弃告警链路闭合
- **现状**：`efficiency-monitor/monitor.rs:458` 采样 `critical_dropped_count`→publish；
  消费端 `chimera-tui/data/sync.rs:30 nexus_critical_event_dropped_total` 在，但
  `EfficiencyMonitor::with_event_bus` 全仓 src 命中仅测试模块 ⇒ 中间未装配。
- **契约**：组合根（`experience_loop.rs` 旁，`RuntimeAuditor` 同处）构造并 spawn
  `EfficiencyMonitor`，使其订阅 D2 的 `SlowConsumerDropped` + Critical 丢弃计数，回灌 TUI/日志。
- **进度（2026-09-25）**：D3 已落地——`experience_loop.rs` 审计器块后新增 EfficiencyMonitor 装配（`with_event_bus` + `start_event_subscriber`），闭合 Critical 丢弃告警链；`cargo test -p chimera-cli --test experience_loop_closure_e2e` 4 passed / 0 failed（零回归）。待：注入丢弃→面板可见的专用 e2e。
- **验收（CI）**：注入 N 次 Lagged/Critical-drop → TUI 面板/日志可见对应指标非占位。

### D4（Phase 2 E-1）Critical WAL（at-least-once 持久化，不回放破坏语义）
- **现状**：`critical_sink.rs` 已有可插拔 `CriticalSink` trait（`on_critical(&self,&NexusEvent)`）+
  默认 `LogCriticalSink`（`tracing::error!`）+ `with_critical_fallback`。缺**持久化**。
- **契约**：新增 `FileWalCriticalSink: CriticalSink`——`on_critical` 以 append + 行级 JSON 落
  WAL 文件（复用 `scc-cache` WAL 先例的 fsync 纪律），**非阻塞**（写失败降级 `error!`，不上抛，
  守 at-least-once）。启动重放只在**新增**订阅通道暴露，不改 `subscribe_critical_events`
  "从订阅时刻起、不回放历史"的既有语义（避免回归）；重放产物写审计账本而非塞回 live 流。
  组合根以 `EventBus::new().with_critical_fallback(Arc::new(FileWalCriticalSink::open(path)?))`
  替换默认。
- **不变量**：sink 实现体禁阻塞/禁 panic（热路径）；不引入 `unsafe`；WAL 文件路径走配置（可空=保持 LogCriticalSink）。
- **进度（2026-09-25）**：D4 已落地——`FileWalCriticalSink`（event-bus/critical_sink.rs）+ lib.rs 导出 + composition.rs `CHIMERA_CRITICAL_WAL_PATH` env opt-in 接线（默认仍 LogCriticalSink；config 化留后续）。`cargo test -p event-bus --lib file_wal` 2 passed；`cargo check -p chimera-cli` 零 error。待：崩溃-重放 e2e（需真进程 kill，CI 环）与 bench 延迟回退对照。
- **验收（CI）**：e2e 注入 `SkepticVeto`→杀进程→重启→WAL 可检索该事件；publish 延迟无显著回退（bench）。

## 2. 否决方案
- γ' 事件化 Critical 安全读（gsoe→decay）：已于 ADR-187 否决（fail-closed 退化），本 ADR 不重提。
- 同步 flush 于 publish 路径：违反"热路径不阻塞"红线 ⇒ 采异步/尽力而为 + 后台 fsync。
- 用大括号启发式实现 P-3/E-6 静态门：实测 3/4 假阳（深审 §4.5），否决；须 `syn` AST 另立。

## 3. 落地边界（诚实标注）
本 ADR 是**契约与精确改点**；Phase 1/2 的 Rust 实现须在有 cargo 的环境完成并以
`cargo check/clippy/test --workspace`（测试数不降、0 fail）+ 上述各 D 验收 e2e 为 Accepted 前置。
本沙箱 **bash 门不可执行但 cargo 经管道/`Tee` 可用**（深审报告 §8）：D1（O-1 挂载）已 `cargo check -p chimera-cli` 零 error + `build_attaches_bus_logger_for_observability` 测试 ok。D2-D4 逐 CR 验证（各 D 验收 e2e 绿）后升 Accepted；E-2 因 recv 内 publish 有自反馈/死锁设计风险，需单独论证非盲上。

## 4. 受影响清单
`crates/event-bus/src/bus.rs`（recv Lagged、publish 背压、set_logger 调用点）、
`crates/event-bus/src/backpressure.rs`（接线）、`crates/event-bus/src/critical_sink.rs`（FileWalCriticalSink）、
`crates/chimera-cli/src/composition.rs`（D1/D3/D4 装配）、
`crates/efficiency-monitor/src/monitor.rs`（D3 装配点）、`crates/chimera-tui/src/data/sync.rs`（消费）、
新增 `ADR-191` 登记 adr_index/CODE_WIKI §11。
