# ADR-200: CausalGraph 归因台账处置 — telemetry 导出侧接线（AttributionTap）+ 内环四模块状态终裁

- **状态**: ✅ **Accepted（2026-10-07，所有者认证）**；来源 = L0-L3 融合打磨方案 W4-0 T0-2（U-4 裁决「出 ADR 并接线」）；施工登记 = W4 批 B WI-G8a
- **决策者**: E01（架构）；证据面 = 2026-10-07 现测（`crates/event-bus/src/` 只读勘察）
- **取号证据**: 取号前对 `docs/`、`scripts/`、根 `*.md`、`.claude/`、`.qoder/`、`.trae/` 全域双通道 grep（ripgrep + PowerShell Select-String），`ADR-20[01]` 仅命中 v3.2 方案文本与 `docs/reports/cpu-plane-closeout-*.md` 的**预期号引用**（本批自身预留），无实体占用
- **承袭**: ADR-132（CausalGraph 归因管道原始设计）/ ADR-181（决策 2：接线或退役预算，本 ADR 即其触发点）/ ADR-175（批次机制）/ ADR-195 D2（Critical 发布不阻塞永久排除面）/ ADR-197（装配层启动 reporter + env 门控先例）/ ADR-199（反「为保留 API 制造调用方」先例）
- **不触碰**: 发布热路径（零 `publish` 改动）；`NexusEvent` 变体集（152 锁定）；crate 数（39 锁定）；R2 冻结面（ADR-042/053）

---

## 1. 背景与判据

### 1.1 现状（2026-10-07 现测）

- `event-bus::attribution::CausalAttributionLedger`（ADR-132，环形 4096 / 5s 窗口，`record` O(1) push + `attribute` 窗口回溯）自落地起**零生产消费方**；ADR-181 决策 2 归类 **EXPERIMENTAL-UNWIRED**，接线路径 =「接入 telemetry 导出侧，或降为内部结构」，退役条件 =「下一遥测批次结束零消费 → 删除」。
- `causal::VectorClock`（1659 行，71 测试基线）消费方仅 attribution（同 crate 内部基座）；`causal::CausalBuffer`（乱序重排缓冲，`causal.rs:381-588`）零外部引用；`shadow_diff`（双跑指纹比对 + ≥7 天台账）为**运维工具态**（`run_shadow_sojourn.ps1` 手动触发），其「分片转正决策数据源」职能已随 ADR-153 Go 完成。
- 原始设计（ADR-132 决策 5）：归因输入 = 双跑比对输出的 `first_diff` 期望指纹；记录侧设计挂载点 =「发布路径 `record()` 一次调用」。
- 泛型记录所需素材已在位：`registry.rs:89` 宏生成的 `NexusEvent::metadata() -> &EventMetadata`（全 152 变体第一字段统一），`EventMetadata.source` 可作时钟节点键。

### 1.2 判据（为何选 A 而非 B，以及 A 的形态约束）

- U-4 裁决倾向接线；本 ADR 采用 ADR-181 既有接线路径①（telemetry 导出侧），**且不制造高频业务调用方**——ADR-199 候选 A 已确立反模式：「为保留 API 而制造调用方 = 反向制造需求」。
- **红线约束决定接线形态**：① ADR-195 D2 永久排除面含「Critical 发布不阻塞」——记录**不得**挂发布热路径（`record(&mut self)` 需锁，挂热路径即引入锁竞争，违 UP-15 治理方向）；② 跨节点全序因果需事件携带 VectorClock 字段（L0 契约面破坏性扩展），本轮**明确不做**。
- 因此形态收敛为：**订阅侧 tap（不碰发布路径）+ 单进程 per-source 时钟 + 异常事件自动归因导出**。该形态同时满足「接线真实性」（组合根常驻启动、生产进程中真实消费 `attribute()`）与「零行为变更」（只观测 + 异常时日志/导出）。

---

## 2. 决策

### D1 选项 A 落地：`AttributionTap`（订阅侧记录 + 异常自动归因），由装配层启动

- `event-bus` 新增 `attribution::spawn_attribution_tap(bus: &EventBus) -> AttributionTapHandle`：
  - **订阅侧记录**：先 `subscribe` 后 `spawn`（总线红线，G-79/G-80 守护）；每事件 `record(event_fingerprint(ev), per_source_clock(ev), now_ms)`；**零发布路径改动**（tick 0 行 diff 于 `bus.rs` publish 面）。
  - **时钟语义（诚实登记）**：`clock` 为单节点时钟 `{event.metadata().source: 该源计数++}`——语义 = **源内偏序 + 跨源并发**；在 `attribute()` 的 Before|Concurrent 双类入链规则下，链 = 「窗口内同源前驱 + 跨源并发观察者」。全局因果（跨节点时钟交换）需 L0 字段扩展，**登记为局限，本轮不做**。
  - **异常自动归因（消费点）**：tap 观测到**触发集**事件（显式常量清单 + 测试锁；初始 = `SlowConsumerDropped` / `BudgetExceeded`，实施期以现测变体名定格、扩展须评审）时，调用 `attribute()` 并将因果链（fingerprint / relation / delta_ms）写入 `tracing::warn!` 诊断日志；可选 JSONL 导出（env 路径门控，默认关，对齐 `CHIMERA_METRICS_DUMP_PATH` 先例）。
  - **装配**：由组合根（`chimera-cli` composition/experience_loop）启动，与 `spawn_saturation_reporter` / `spawn_throughput_reporter` 同族（ADR-197 D3 先例）；**零新增依赖边**（chimera-cli → event-bus 边已存在，G-33 不受扰）。
- **首个生产消费方** = tap 自身（生产进程内常驻的自动归因 + 诊断导出）；`AttributionTapHandle` 暴露只读查询口，供后续消费者（错误签名检索链 / parliament 裁决归因 / doctor 诊断）**按真实需求**在各自波次接入——非本轮强制、非预授权。

### D2 四模块状态终裁（ADR-181 决策 2 的到点复核）

| 模块 | 终态 | 依据 |
|---|---|---|
| `attribution::CausalAttributionLedger` | **D1 落地后转 PRODUCTION-WIRED** | 本 ADR |
| `causal::VectorClock` | **D1 落地后转 PRODUCTION-WIRED（经 attribution）**；对外 API 面不扩大 | 同上 |
| `causal::CausalBuffer` | 维持 **EXPERIMENTAL-UNWIRED**（无真实重排需求证据，**不并接**；处置绑定 attribution 的第一次治理复核） | 无消费方 |
| `shadow_diff`（Recorder / Report / SojournLedger） | 维持**运维工具态**；Go 决策已完成，**保留 7 天窗口语义、不删除**（ADR-181 决策 4） | ADR-153/156 |

### D3 反幽灵纪律与回滚

- 不新增事件变体、不新增 crate、不新增依赖边；`critical_channel_test` / `subscribe_order` / `await_across_guard` 门面零触碰。
- 回滚：整批 revert（单提交可回滚，订阅侧新增无存量依赖）。
- **go/no-go 内嵌**：批 B 施工后设观察窗——若诊断面长期零使用价值证据，按 ADR-181 退役条件改判（删除 ledger + examples 同步退役），由下一次治理批次复核并落档。

---

## 3. 后果

- **正面**：ADR-132 的「diff → 5s 因果链」链路首次进入生产进程（黑匣子式事故归因：异常事件发生时自动产出前 5s 事件链）；EXPERIMENTAL-UNWIRED 中间态收窄一项；ADR-181 决策 2 的预算闭环一项（接线即达，非退役）。
- **代价**：生产进程多一个 broadcast 订阅者（O(1)/事件；tap lag 只影响自身，broadcast 语义保证不向他方传导背压）；异常事件发生时多一次窗口扫描（≤4096 线性，异常稀疏）；新增诊断 warn 行（仅异常触发）。
- **局限（诚实登记）**：① 单进程 per-source 时钟非全序因果；② 链输出为 fingerprint 级（人读需配合事件日志/审计导出）。二者如需升级（L0 携因果时钟字段 / 链节点带类型摘要），属契约面变更，须另立 ADR。

---

## 4. 验收（批 B WI-G8a 适用）

| # | 判据 | 验证方式 |
|---|---|---|
| V1 | 接线真实性 | `spawn_attribution_tap` 由组合根启动；如纳入 `check_dead_contract_apis.py` 台账则该 API 为 WIRED（≥1 生产调用点） |
| V2 | 零发布路径改动 | `bus.rs` publish/publish_critical 相关 diff 为空；`critical_channel_test` 与订阅者计数测试不回退 |
| V3 | 自动归因可测 | 集成测试：注入前置窗口事件 + 触发集事件 → 断言诊断输出含因果链（`diff_found` 与链内容） |
| V4 | bench 不回退 | `cargo bench -p event-bus` 既有键复测（含 shard/bus 面） |
| V5 | 状态标签同步 | `attribution.rs` / `causal.rs` doc 头状态行与 `lib.rs` 模块位标与 D2 表一致 |

---

## 5. 剩余开放项

1. 后续消费者（错误签名检索链 / parliament 归因 / doctor）接入与否 = 各自波次按真实需求评审（本 ADR 不预授权）。
2. `CausalBuffer` 的复用或删除，绑定 attribution 第一次治理复核。
3. L0 级「事件携带因果时钟」字段扩展 = 破坏性契约变更，须独立 ADR（不在本轮）。

---

- **证据锚点**: `crates/event-bus/src/attribution.rs:65-172`（ledger API）· `crates/event-bus/src/causal.rs:381-588`（CausalBuffer）· `crates/event-bus/src/registry.rs:89`（`metadata()` 统一访问口）· `crates/event-bus/src/shadow_diff.rs:1-23`（运维工具态自述）· `docs/architecture/ADR-181-shadow-capability-status-classification.md:47-57`（接线/退役预算表）· `docs/architecture/ADR-132-causal-graph-attribution.md:16-47`
