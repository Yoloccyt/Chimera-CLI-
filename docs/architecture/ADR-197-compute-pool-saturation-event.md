# ADR-197: 计算池饱和观测事件与契约 API 有生产者巡检

> **状态**: ✅ Accepted（2026-10-01）
> **决策主体**: 用户批准《计算面 Phase A 实施》计划（交付形态 = 完整 Phase A + 新增事件变体并同步六锁）
> **层级**: L1 Core（`nexus-core::compute` + `event-bus`）+ L9（`efficiency-monitor` 消费）+ L10（`chimera-cli` 装配）
> **关联**: ADR-103/107/123/124/126/127/128（计算地基与 HTS）、ADR-153/157（分片总线与双口径利用率）、ADR-160（可达性棘轮）、ADR-188（事件所有权台账）、ADR-194（六锁同步范式）、ADR-195（GPU 维持否决）、ADR-196（CPU 侧承接，储备）
> **证据正文**: `docs/reports/cpu-hetero-four-dimension-audit-2026-10-01.md`（发现编号 S-/P-/U-/A-）
> **实施规范**: `docs/superpowers/specs/2026-10-01-cpu-compute-plane-optimization-design.md` §3 Phase A

---

## 1. 背景与问题

四维审计（稳定性 / 性能 / CPU 利用率 / 架构效率）在 CPU 计算面发现一类共性缺陷：**契约已承诺、闭环未接上**，且**测试与 lint 两层都看不见它**。

| 编号 | 事实 | 证据 |
|---|---|---|
| S-1 | `ComputeError::Saturated` 无构造点，注释却称"D15 熔断触发路径，T9/T14 接线" | `nexus-core/src/compute/bridge.rs` 枚举 + 全库大小写敏感检索 |
| S-3 | 并行结果错误被 `unwrap_or(0.0)` 折算成合法值 ⇒ 池内 panic 变成"适应度 0 分" | `gsoe-evolution/src/fitness_parallel.rs:75` |
| S-4 | 9 个 crate 的 `let _ = enable_sharding(...)` 吞掉启用结果 ⇒ "本次运行没分片"不可归因 | `osa-coordinator/src/coordinator.rs:184` 等 10 处 |
| S-5 | `kind` 在卸载入口 `let _ = kind;`，ADR-125 遥测计数从未接线 | `bridge.rs` 两处 |
| S-8 | `shadow_stats()` 唯一非测试消费者是 bench ⇒ shed 回退频次产线不可见 | `event-bus/src/bus.rs:430` + `benches/bus_shard_bench.rs:184` |
| U-2 | 双口径利用率探针停在 `examples/` 层 | `nexus-core/examples/utilization_probe.rs` |
| A-5 | `cargo test -p nexus-core` 235 项全绿 **且** `cargo clippy --all-targets` 零告警，上述死路径无一被捕获 | 2026-10-01 实测 |

根因不是能力缺失，而是**"发布契约"与"接线实现"之间没有强制点**：公开 API 不受 `dead_code` 约束，测试只覆盖已实现行为，于是"文档写了 = 系统做了"这类失真可以长期存活。

## 2. 决策

### D1 新增 `NexusEvent::ComputePoolSaturated`（Normal / System），并完成六锁同步

字段全部为整型或千分比（不使用浮点，避免"微小差异可接受"的语义滑坡）：

```text
ComputePoolSaturated { metadata, active_tasks: usize, pool_threads: usize,
                       saturation_ratio_bp: u32, combined_utilization_bp: u32,
                       threshold_bp: u32, saturation_count: u64,
                       degraded_count: u64, hottest_kind: String }
```

**两个利用率字段并存是有意设计**：`saturation_ratio_bp` 为**单口径**（仅 rayon 侧占用，用作超阈判定），`combined_utilization_bp` 为**双口径合成**（ADR-157 口径：tokio 与 rayon 各 0.5 权重，无 runtime 时按既有规则退化）。只留一个就会重现 ADR-155 的“55.2% 偏差”一类单侧化误读（当时只看了 IO 侧）。

同理 **`saturation_count`（超阈观测次数）与 `degraded_count`（实际降级次数）必须分开**：默认策略为 Observe（只观测不改分支），此时降级数恒为 0；若两者合用一个字段，事件会把“观测到超阈”谎报为“发生了行为变更”，且去抖会退化为“首次报过就永远不再报”。

**字段形态选扁平内联，不进 `payloads.rs`**（与实施计划的差异显式登记）：`payloads.rs` 现有结构体的 WHY 注释均指向同一个理由——“消费者所在的 L10 不能上依生产者所在层”（`ClvSummary`、`RouterStatsPayload`、`BudgetMetricsPayload` 三例）。本事件的消费者（`efficiency-monitor`）只读 `degraded_count` 一个整数，无跳层类型需求；且三个同族周期观测事件（`BusThroughputReported`、`SecurityInterceptionReported`、`HcwRecallReported`）均为扁平字段。再包一层结构体属于无收益的间接层，故跟同同族惯例。（若 Phase B 需要按 kind 导出详细数组，届时再提 `payloads.rs` 结构体并走 supersession。）

定级理由：它**只报告观测量、不改变对外结果语义**（饱和时降级为 Inline 执行，方向是"变慢不变错"），因此**不进** `is_critical_mpsc_event`（16）与 `LANE_FORBIDDEN_SHARD`（20），Critical 双清单分毫不动；账面同步为 `151 → 152`、Normal 分布 `120 → 121`。

六锁同步站点：`event-bus/src/types.rs`（变体）、`registry.rs`（显式定级，宏无通配符 ⇒ 编译器强制）、`membrane.rs::is_read_metric`（观测面本地消化）、`tests/variant_count_test.rs`（样本清单 + 序数 152 + 两把计数锁）、`scripts/doc_count_freeze.txt`（重锁 152）、`chimera-tui/tests/event_coverage_drift_test.rs`（`TuiEventClass::NotForTui`）。

### D2 饱和处置走 **env 三态策略**，默认 `Observe`（零行为变更）；禁 feature 标志，不新增 Figment 配置段

- `CHIMERA_COMPUTE_SATURATION_BP`（默认 **900 = 90%**；合法区间 100‰–10000‰，非法值一律 fail-safe 回落默认）；
- `CHIMERA_COMPUTE_SATURATION_MODE` = `observe`（**默认**：只记观测与发事件，任务仍走池）/ `degrade`（走 Inline，结果与走池逐元素一致）/ `strict`（不执行闭包，直接返回 `Saturated`）；
- 旧 `CHIMERA_COMPUTE_STRICT_POOL=1/true/on/yes` 仍可用，等价于 `strict`（仅在新变量未设置时生效）。

WHY：① **默认必须零行为变更**。实施计划对 A1 的风险缓解明文写的是“先以不改变分支的观测模式（warn-only）上线，拐点数据充足后再开降级”——降级与报错都是行为变更，在拐点未测前不得当默认态（首轮实现取的是“默认降级”，完成审计时与此缓解相左，已改为本三态）。② 项目红线要求“降级路径 = 子命令 + 配置，禁 feature 标志”（ADR-122）。③ 不新增 Figment 配置段（避开 14 段模型、`check_config_sample_parity.py` 与 `config_section_parse` fuzz target）；env 经 `OnceLock` 启动期一次读取，热路径不 `getenv`。

`ComputeError::Saturated` 仍是**生产可达状态**（`strict` 分支），因此 S-1 的注释与实现一致；同时实现内部用 `Option::take` 而非 bool 标记来保证闭包与 oneshot 发送端各自只被移动一次（运行时 flag 无法让借用检查器证明分支互斥）。

### D3 上报器由**装配层启动**，判定与发布收敛在双口径采样器，否决 `efficiency-monitor → nexus-core` 新依赖边

`nexus-core::compute::spawn_saturation_reporter(bus, period)` 由 `chimera-cli/src/experience_loop.rs` 与 `spawn_throughput_reporter`、`spawn_interception_reporter` 并列启动；它**只负责周期驱动**，采样/累计/去抖/发布全部收敛在 `UtilizationSampler::sample_and_report`（与 `report_from` 分离面配套，先例 = `dispatch.rs::decide` 把三态判定从 IO 取值里拆出来以便确定性单测）。同一判定不得两处各写一份（本仓曾因 `publish`/`publish_blocking` 双份实现踩过静默分叉，现以宏形式收敛，见 `bus.rs` 注释）。

**被否方案**：让 `efficiency-monitor` 直接读 `nexus-core` 的探针 —— 现测 `crates/efficiency-monitor/Cargo.toml` 实际依赖为 `event-bus` / `nexus-contracts` / `decb-governor`，`scripts/dep_edge_freeze.txt` 亦无 `efficiency-monitor|nexus-core` 边；新增该边会撞 G-33 边冻结棘轮（只减不增）。而 `nexus-core` 本就依赖 `event-bus` 且已有发布 `ClvSnapshotReported` 的先例，故"owner crate 提供 reporter + 装配层启动"是零依赖变更的正解。

### D4 静默失真按"区分合法回退与掩盖失败"处置

- `gsoe` 的 `unwrap_or(0.0)` 改为 `merge_with_serial`：失败项回退**同一算式的串行值**并 `warn!`，保持索引对齐（调用方按下标消费，不得丢项）；
- 9 处分片启用改为 `EventBus::try_enable_sharding(component)`：降级方向不变，但结果按类别落日志（幂等 `AlreadyEnabled` → `debug!` 并返回 true；runtime 缺位 → 首次 `warn!` 后续 `debug!`）；
- **明确不算缺陷**的对照组：`hcw-window/src/parallel.rs:270` 的 `unwrap_or(0)`、`osa-coordinator/src/parallel.rs:198/207/222` 的 `unwrap_or(&heuristic)` 是空入参保护与文档化启发式回退，不改动。

### D5 立 `scripts/check_dead_contract_apis.py` 作为回归护栏（暂不成门）

WIRED 项必须有 ≥1 生产调用点，回落到 0 即 FAIL；尚未接线项标 DEFERRED 并写理据，一旦出现真实使用则提示"升级为 WIRED"。脚本带**双向夹具** `--selftest`（真阳 + 真阴），防止"正则匹配不到任何东西 ⇒ 永远绿"的假守护。

登记为正式门（G-85）需 `spec_ref` 与委员会决议，本 ADR 只交付脚本本身。

## 3. 被否 / 推迟的方案

| 方案 | 判定 | 理由 |
|---|---|---|
| 删除 `Saturated` 变体 | 否 | 抹掉 D15 契约承诺，等于用删除掩盖未实现 |
| 在 gsoe 用 `expect` 传播池内 panic | 否 | 违反"池内 panic 永不跨线程传播到调用方"不变式（`bridge.rs` §11.1） |
| 把饱和事件定为 Critical 走 mpsc 旁路 | 否 | 它不承载不可丢语义；进 Critical 反而稀释旁路通道容量并触发双清单同步 |
| 在 `spawn_compute*` 内直接发事件 | 否 | 需给 L1 计算桥注入全局 `EventBus` 句柄，热路径引入 IO 侧依赖；且发事件去抖需状态，落点在周期报告器更自然 |
| 默认即降级（`Degrade` 当默认态） | 否（已改） | 计划的风险缓解要求先观测后变更；降级是行为变更，无拐点实测前不当默认。现默认 `Observe`，有专项用例守住“只记观测且 `degradations == 0`” |
| 用真池占用 + 定时等待做集成测试 | 否 | 慢机（15GB / `jobs=4`）上必飘；改为把去抖判定抽成纯函数 `report_needed` + 分离面 `report_from` 做确定性单测 |
| 同时实施 TaskKind 四登记位 | 推迟 | 属 ADR-196（仍 Proposed），须先获批准；本 ADR 不重复立案 |

## 4. 后果

**正面**：饱和从"文档承诺"变为"可判定、可计数、可上报、可回归"；利用率与降级发生率进入 Ω₈ 证据链（`nexus_compute_pool_degraded_total` 指标）；9 处分片启用可归因；`compute/telemetry.rs` 让并行决策具备按负载归因的输入，为 Phase B（阈值校准闭环）解锁前置。

**代价与风险**：① 变体数 151→152 触发六处账面同步（由编译器 + 计数锁保证不漏）；② 默认 `Observe` 零行为变更，一旦运维改 `degrade`/`strict` 就可能暴露既有饱和（收益是发现，代价是新告警）；③ 周期上报器每 60s 一次双口径采样（定长原子读 + 千分比运算，非热路径），实测 `is_saturated` 仅 4.88 ns；④ `spawn_compute` 与 `update_threshold` 仍无生产调用者（台账 DEFERRED，不伪装已接）。

## 5. 验证（实施后回填，逐项见进度报告）

`docs/reports/cpu-plane-phaseA-progress-2026-10-01.md`：新增测试清单与通过数、`cargo test --workspace` 结果、`clippy --all-targets -D warnings`、`fmt --check`、`check_doc_drift.sh` 重锁后 rc、`check_dead_contract_apis.py`（`--selftest` 与主判定）、`bridge_bench` / `hts_bench` 前后对照、依赖边与可达性门。
