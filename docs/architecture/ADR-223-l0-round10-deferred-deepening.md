# ADR-223: L0 深水区收口（R10）—— L0-B 依赖面门、遥测事件族、sibling 装配面棘轮、NOT-IMPL 债清偿

> **Status**: ✅ Accepted (2026-10-23，业主批准《L0 深水区（R10）》计划并选定「深改 deferred 项 + 事实基线含工作树全部」)
> **决策日期**: 2026-10-23
> **关联 ADR**: ADR-222（L0 双层边界，D1–D6 **全部继承、不重开**）/ ADR-215 D-29 与 ADR-221 D-29（装配面棘轮，本 ADR **解除 defer**）/ ADR-212:108（G-16 不修，继承）/ ADR-218~220（门纪律、台账形态、机制字段，全部继承）/ ADR-197（`ComputePoolSaturated` 先例）/ ADR-173（动态路由冻结，本 ADR 只澄清符号不计债）/ ADR-033 追加节（L0-A 受控纯逻辑）
> **关联交付**: `docs/reports/L0-round10-deferred-deepening_2026-10-23.md`（基线 / 冲突台账 / 十维触及面 / 验证读数 / a–e）；上游 `L0-round8-verification_2026-10-19.md`、`L0-round9-closure_2026-10-21.md`
> **权威源**: 代码与脚本实测——`crates/event-bus/src/{types.rs,registry.rs}`、`crates/nexus-telemetry/src/report.rs`、`crates/*/Cargo.toml`、`scripts/{check_l0a_sibling_assembly.py,check_l0b_dep_surface.py,check_impl_maturity_claims.py,gate_manifest.toml}`；两份设计文档（v3.4.0 二十三篇 / 十六源手册 v3.0，均在**仓库根**而非 `docs/architecture/`）仅作意图参照
> **编号合法性**: 落盘前现取 ADR 物理文件 **118** 份、主编号 max **222**；`grep -rn "ADR-223"` 全库 **0 命中**（无预留、无并发占用）⇒ 223 空闲可用。门号同理：`gate_manifest.toml` 落盘前 max **G-128** ⇒ 本 ADR 取 **G-129/G-130**（装配面）与 **G-131/G-132**（L0-B 依赖面）

---

## 1. 背景

R9 收口（ADR-222 §8 的 R9 段）关闭了 R8-1/7/9/10，并把 R8-2/3/4/8 留作 defer。此后 HEAD 又前进了两批：event-bus 分片（`bus.rs +370`、`shard.rs`、新增 `without_sharding()` / `request_drain()` / `drain_residual()`）与 CLI/TUI 侧的收尾批次。三条 defer 的触发条件中，只有「装配面事故」真的发生了形状变化——**1+6 拆分本身让 sibling 成了新的装配单元**，而全库没有任何门看得见 sibling 的装配边；同时 L0-B 在依赖面持续生长却只有 ADR-222 D3 那句"另立较宽门"的意向。

本轮按业主选择做**深改**：把三条 defer 落成机器判据或真实实现，并对分片后的 L0-B 补一道成本可控的收紧门。

## 2. 决策

### D1：L0-B 外部依赖面设 only-decrease 门（**反转 R8-1 的「不设冻结门」**）

- 新门 **G-131/G-132**（`scripts/check_l0b_dep_surface.py` + 台账 `scripts/l0b_dep_freeze.txt`）：对 `nexus-core` / `event-bus` / `nexus-telemetry` 的 `[dependencies]` + `[build-dependencies]` + target 专属依赖表建 `<crate>::<dep>` 键集，只锁**外部依赖**；新增必红并须另立 ADR，删除/改名必红为 STALE。
- **口径边界**：`[dev-dependencies]` 不计（与 G-125 披露的 dev 边界同源，避免同一事实两处口径）；workspace 内部边**不在此判**，方向唯一归 G-125（一事一判，不造第二法官）。
- **反转理由与代价对照**：R8-1 当年以"门膨胀"为由不设门，并给了触发条件「L0-B 第一次真的需要第 4 个外部依赖时」。实测该条件的形态已变——分片把 `crossbeam-queue` / `arc-swap` 这类并发原语带进 L0-B，依赖面是**生长中**而非稳定态；此时设门的成本是 27 行台账（`--emit` 机器派生、禁手编），收益是"新增外部依赖必须过 ADR"从散文变成机器判据。**明确不承担的**：不锁 L0-B 的 `pub fn` 面（`event-bus` 单 crate 实测 281 个 `pub fn`，锁它等于造一份 300 行影子台账），该轴登记为 §6 待裁。
- 反方案与否决见 §5 R-A/R-B。

### D2：遥测独立事件族落地为 2 个 Normal 事件（关闭 R8-4）

- 新增 `NexusEvent::TelemetryCalibrationApplied { applied_rows }` 与 `NexusEvent::TelemetryReporterDegraded { requested_period_secs, effective_period_secs }`，定级 **Normal / System**，**不进** mpsc 旁路与 `LANE_FORBIDDEN_SHARD`（二者都不是"必须送达"的否决/预算类信号）。
- **真实生产者**（这是本决策与"装饰变体"的分界）：
  - `TelemetryCalibrationApplied` ← `nexus-telemetry/src/report.rs::reporter_tick` 的 `applied > 0` 分支（原来只有 `tracing::info!`，治理动作不可订阅）；
  - `TelemetryReporterDegraded` ← `spawn_saturation_reporter` 启动时 `clamped_period(period_secs) != period_secs`（`0 → 1s` 是本 crate 唯一"输入被静默改写"的规则）。
- **七点位同批**（缺一即编译或门红）：`types.rs` 变体 + `registry.rs` 定级 + `variant_count_test.rs` 样本清单 + `VARIANT_COUNT_LOCK` + severity 分布锁 + `scripts/doc_count_freeze.txt` + 新增 `crates/nexus-telemetry/tests/telemetry_events.rs`（生产者→消费者闭合，锁"会被发布"而非"能构造"）。
- **兼容性**：`NexusEvent` 未加 `#[non_exhaustive]`，新增变体对穷尽 match 是**破坏性**变更。实测唯一命中点是 `crates/chimera-tui/tests/event_coverage_drift_test.rs` 的 `classify`（该文件的**设计意图**就是"新增变体编译失败"），按 TUI 消费口径判为 `NotForTui`（与 `ComputePoolSaturated` 同类：治理与可观测面，不占面板）。`CONSUMED_NAMES` 规模锁（80）不变。对外调用方给 semver-minor + `_ =>` 通配迁移说明。
- 反方案与否决见 §5 R-C/R-D。

### D3：sibling 装配面棘轮解除 defer（关闭 R8-3）

- 新门 **G-129/G-130**（`scripts/check_l0a_sibling_assembly.py` + 台账 `scripts/l0a_sibling_assembly.txt`）：键 = `<sibling> -> <consumer>`，取自 `crates/*/Cargo.toml` 的生产 `[dependencies]`；only-decrease（NEW/STALE 皆红）、畸形或重复行退 2、roster 非"umbrella + 6"退 2、边数低于地板退 2。
- **实测形状（现取，`--emit` 自报）**：装配边 **9** 条，其中 facade（消费者为 umbrella）6 条、**外部直连仅 3 条且全部指向 `nexus-contracts-memory`**。⇒ "1+6 拆分"当前的真实装配面是**facade 主导**：除 memory 外没有第二个 sibling 直连消费者。本门的价值正是把这个事实钉成判据——今后任何人静默摘掉一条 sibling 边即红，而不是等事故。
- **地板为何是 2 而不是现值**：地板只防"盲扫假绿"（夹具树/遍历损坏），不锁规模；把地板设成今天的边数会让一次有意评审的收缩被判成"不可判定"，而它应被判 STALE 以强制评审。
- **与既有门的轴分工**：G-91 数"定义的消费者"（umbrella 视角）、G-123/124 数"零消费者公共类型"、G-125 数"crate 级方向"，本门数"**装配边**"——四者分母不同，不构成重复立案。ADR-215 D-29 当年否决的是"在只有 1 个 L0 crate 时提前建逐 crate 轴"，1+6 拆分正是它写的触发前提。

### D4：L0 的三条 NOT-IMPL-CN 台账行按实证分类处置（关闭 R8-8）

先分类再动手，不统一按"实现"处理：

| 台账行 | 实证 | 处置 |
|---|---|---|
| `nexus-core.NOT-IMPL-CN=2`（`compute/seam.rs`） | **真债**：`OsFs::read/write/list` 一律返回 `Unsupported` | **实装**：`std::fs` 三操作；错误新增 `SeamError::Io(String)` 且文案带路径；`write` 不隐式建父目录；`list` 排序返回；trait 文档写明"同步接口，异步调用方必须 `spawn_blocking`"（阻塞 IO 红线）。测试 `os_fs_roundtrip_list_and_errors` 覆盖往返/排序/缺文件/错路径四种形状 |
| `nexus-contracts-seams.NOT-IMPL-CN=1`（`memory_strategy.rs:31`） | **误报**：散文叙述"三重悖论病理"（过去的 OSA 行为），不是本契约的完成度自陈 | 改写为"缺少随任务阶段自适应的记忆策略"，并给 G-53 的 `NEGATION` 增补 `从未实现` 类词表 |
| `event-bus.NOT-IMPL-CN=1`（`dynamic_registry.rs:17`） | **误报**：`DynamicEventRouter` 是被 ADR-173 裁定冻结的**已废弃符号**，"从未实现"是对 absent 的描述 | 改写为"从未落地…不予实现"；同类句式由 `NEGATION` 兜住；`test_impl_maturity_smoke.sh` 增第 5 条断言证明"被否定的 WIP 措辞不计债"在真实扫描路径上成立 |

- **公共 API 影响**：`SeamError` 去掉 `Copy`、新增 `Io(String)` 变体。实测 `SeamError` 的全部使用点都在 `nexus-core` 内（全库 grep 仅此文件），故不产生下游破坏；登记为 L0-B 内部收窄。
- **本轮不碰的同类**：`crates/event-bus/tests/backpressure_test.rs` 有若干"未实现 API `critical_channel_capacity()`"注记，位于 `tests/`，G-53 只扫 `crates/*/src` ⇒ 不计债，也不在本轮实装（属分片批次未接线的观测接口，触发条件见 §6）。

### D5：回滚

见 §5。

## 3. 影响

- **波及 crate**：`event-bus`（2 变体 + 定级）、`nexus-telemetry`（2 个发布点 + 1 个测试靶）、`nexus-core`（`seam.rs` 实装与错误面）、`chimera-tui`（仅测试分类臂）、`nexus-contracts-seams` / `event-bus`（文档措辞）；新增门脚本 2 个 + 台账 2 个 + manifest 4 条 + ci 2 步。
- **事件面**：`NexusEvent` 152 → **154**，分布 Critical 20 / Info 11 / Normal **123**；mpsc 旁路 16 与 `CRITICAL_TOTAL` 20 **不变**。
- **契约/可见性**：`Fs` trait 签名不变；`SeamError` 增 `Io(String)` 去 `Copy`（内部使用）。
- **性能**：本轮唯一新增运行期动作是两处 `publish_blocking`（校准写表时与启动时各一次），非热路径；`OsFs` 无生产调用方 ⇒ 零热路径影响。**不以"无性能影响"作断言**：SLO 三档由既有门复验（见 R10 报告 §4）。
- **安全**：零 `unsafe`（`#![forbid(unsafe_code)]` 不动）；`OsFs` 不做路径规范化/白名单——它是**装配层注入用的通用 FS 接缝**，权限决策属 L4 `seccore`，在此层加校验等于把策略下沉到契约层（ADR-033 口径）。

## 4. 验收判据（计划判据 + 实测，实测段见 §7）

1. `check_l0a_sibling_assembly.py --selftest` 与 `check_l0b_dep_surface.py --selftest` 全绿且**两向有牙**（NEW/STALE 都判得出）。
2. `check_impl_maturity_claims.py` 对 L0 三 crate 不再报 `NOT-IMPL-CN`；smoke 五条断言全绿。
3. 事件七点位同值：`VARIANT_COUNT_LOCK = 154`、分布 `20/11/123`、`doc_count_freeze.txt: nexus_event_variants = 154`；`telemetry_events.rs` 断言"applied=0 不发布 / applied>0 发布且透传"与"period=0 只发一次降级"。
4. `cargo check --workspace --all-targets`、clippy `-D warnings`、逐包 fmt、全量测试（不含 doctest）0 failed、`run_gate_manifest` 全量 light、`.sh`/`.ps1` 双源铁律逐项通过；例外只允许既有登记红。
5. 工作树只剩业主指定排除清单 + 在途批文件（未提交、未修改）。

## 5. 反方案与回滚

**反方案（均已被实测否决）**
- **R-A：L0-B 锁 `pub fn` 面而非依赖面** —— `event-bus` 单 crate 实测 281 个 `pub fn`，需 300 行级影子台账，且与 G-105（L0-A 定义面）语义不同；依赖面才是"新增外部依赖须过 ADR"这条已宣布规则的唯一无主面。
- **R-B：把 L0-B 依赖白名单写进 G-125** —— G-125 的判据是 crate 级**方向**，其 note 已明确声明依赖白名单"不在本门辖区"；把两个判据塞进门会让"方向合法但依赖新增"与"依赖合法但方向违规"共用一个 rc，读者无法归因。
- **R-C：只加 `tracing` 不加总线事件** —— R8-4 的原意就是"遥测是否值得独立事件族"，tracing-only 保留"不可订阅"盲区。
- **R-D：一次加 5+ 个遥测事件（采样/证据/失败各一）** —— `SaturationSource` 无失败返回形状、证据输出每 tick 都发生，加它们就是零生产者装饰或刷屏，违反本仓对装饰变体的禁令；只保留两个有真实分支生产者且语义不重叠的（治理动作结果 / 启动期配置降级）。
- **R-E：把 `OsFs` 改判为"永久占位"并从台账删除** —— 等于把真占位债合法化；且台账是机器派生，删行会立刻被 `--emit` 打回。

**回滚**
- 本轮 5 个提交各自独立可 `git revert`；其中**事件族必须与 `doc_count_freeze.txt` / `variant_count_test.rs` 同批退**（否则计数锁先红），故 R10 把「事件 + 锁 + 报告同批」钉为一个提交。
- 两道新门各自 revert 需**同批**删脚本 + 台账 + manifest 两条 + ci 一步（门删脚本留即死判据）。
- `OsFs` 实装可独立 revert（回退为 `Unsupported` 占位），但必须同批恢复 `impl_maturity_baseline.txt` 的 2 行 `nexus-core.NOT-IMPL-CN`，否则台账 STALE。

## 6. 未做 / 待裁（诚实登记）

1. **L0-B `pub fn` 面**：本轮只锁外部依赖；281 个 `pub fn` 的公共面未登记为轴。触发条件：L0-B 出现"公共面扩张导致下游耦合"的实案，或需要 semver 面清单时。
2. **`critical_channel_capacity()` 未接线**（`event-bus/tests/backpressure_test.rs` 注记）：属分片批次的观测接口，本轮不代其实现。触发条件：该批接线时同批恢复测试。
3. **`chimera-cli.SIMPLIFIED-CN` 台账行已随在途 `chat.rs` 改动变 STALE**：**非本轮引入**（HEAD 有该措辞、工作树已删），由在途批次同批删行；本轮不代改他批文件与他批台账。
4. **G-16**：维持 ADR-212:108「不修」。
5. **release 压测档 / `cargo audit` / fuzz / Docker / machete**：非本轮判据，报告登记为未复跑。

## 7. 执行记录（2026-10-23，实测）

**门与判据**
- `check_l0a_sibling_assembly.py --selftest`：**11 断言 PASS**；真树 `[OK] L0-A sibling assembly: siblings=6 edges=9 external=3 facade=6`，rc=0。
- `check_l0b_dep_surface.py --selftest`：**13 断言 PASS**；真树 `[OK] L0-B dependency surface: crates=3 external_deps=27`，rc=0。
- `check_impl_maturity_claims.py`：L0 三 crate 的 `NOT-IMPL-CN` 行已归零（台账 38 → 35 行）；`test_impl_maturity_smoke.sh` **5 条断言全绿**（含新增的"被否定 WIP 措辞不计债"）。唯一残留红 = `chimera-cli.SIMPLIFIED-CN`，由**在途** `chat.rs` 造成（HEAD 命中 1 / 工作树命中 0），归他批同批处理。
- 事件面：`cargo check --workspace --all-targets` rc=0，穷尽 match 唯一命中点即 TUI 分类测试；`cargo test -p event-bus -p nexus-telemetry` rc=0；`cargo test -p nexus-core` rc=0（含 `os_fs_roundtrip_list_and_errors`）。

**验证链终态**：见 `docs/reports/L0-round10-deferred-deepening_2026-10-23.md` §4（全量测试、门集、doctest、`.sh`/`.ps1`、fmt/clippy 逐值）。
