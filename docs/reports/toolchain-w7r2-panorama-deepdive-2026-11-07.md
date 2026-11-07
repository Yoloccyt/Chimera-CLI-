# W7-R2 全景深审终稿（W3 / W4 / W5+W5.0 / X2 / P1~P4 合成 · 2026-11-07）

> **文档性质**：W7-R2 阶段二/三五路深审（W3、W4、W5+W5.0、X2、P1~P4）的**合成终稿**。上游执行方案 = `docs/reports/toolchain-parallel-optimization-w7r2-panorama-plan-2026-11-07.md`（下称「全景方案」，已入库）；其上游 = W7 续作原始方案（W7 续作原始方案 §12 含 v2 交叉校订补遗）——**该原始方案仅存于作者本机仓库外路径 `C:/Users/30324/.workbuddy/plans/radiant-thunder-darwin-xenXoOpR.md`，不入库、后续会话不可达**（§3.1 自证其全库 0 命中）；其与本稿相关的全部裁决（E-1~E-7 / U-20 / W3 低风险路线）已被已入库的全景方案 §11/§5.4 吸收，后续会话回查**以全景方案与本稿为唯一可达来源**。本终稿把三路深审材料（`tmp/w7r2-deepdive/{facts,W3,W5,X2,P1-P4}.md`，同属 tmp/ 不入库——故深审中有后续执行价值的独立结论均已收录进本稿正文，含 §2.10 事件路线回退清单）+ 9 条复核勘误合成为供后续会话逐波执行的唯一规格底稿。
> **取证时点**：2026-11-07；分支 `feat/toolchain-parallel-optimization`；**HEAD = `a0d6b6b`**（W2 收官提交），工作树干净（本会话 `git log -3 --oneline` / `git status --short` 复核，与 `facts.md` 基线一致）。
> **证据纪律**：ADR-234「证据先于断言」。本终稿每条断言标注证据层级：
> - **【实测·本轮】** = 合成会话现场执行（grep/sed/python/ls/tar，命令与输出在 §14 验证记录）；
> - **【深审实测】** = 深审规格会话自证（标注来源深审文件，如 W3.md §3.1）；行号如与现场漂移，以现场现取为准；
> - **【材料自述】** = facts.md / 全景方案的登记值（未在本轮独立复跑处如实标注）。
> **只读纪律**：本合成会话未改/建/删任何仓库文件（本文件为唯一落盘例外）；未运行 cargo bench / cargo test；未做 git 写操作。
> **行号时效**：全部 file:line 为 HEAD `a0d6b6b` 现值（标注【实测·本轮】者为本会话 grep/sed 现取，其余为深审时点同 HEAD 值）。执行波次如漂移，以现场现取为准并在波次报告登记。

---

## 0. 结论摘要

1. **现势基线**：W1（`22adf6b`，ADR-246 策略配置源）与 W2（`a0d6b6b`，ADR-247 run --execute 闭环 + execute_batch 首个生产调用方）已收官落提交；本会话复跑 `check_perf_redlines.py invariants` **EXIT=0 全绿**（I1 83/83、I2 1≤1 顶格、I3/I7 55、I9 83、I4a 1≤1、I6 62、I8=22、I8c=18），`inventory` EXIT=0（total=126 / registered=53 / dev-only=62 / gated=11 / **unknown=0**）——与 facts.md 取证逐值一致（§1）。
2. **W3 裁决**：P-13 批量归因走 **BusLogger 计数器路线（C-A 主案）**，零事件变体新增——`BusLogger` 现状未暴露注册 API（【深审实测】W3.md §3.1 + 【实测·本轮】§2.3 复核），需 event-bus 加性小改 `register_counter`（≈15-25 行）；`BatchAttributionSink` 依赖倒置契约落在 L7 batch.rs，实现落在 L10 组合根（§2）。
3. **W4 裁决**：链内全借用化「反转主次」——脊柱**复用既有 `BorrowedToolDispatcher`**（不新增同形 trait，防双源漂移）；上游 blanket 适配设想经 coherence 分析**不可行**（E0119/E0117），改显式适配器 `OwnedDispatcherCompat`；静态收益上限 10→7 次分配（约 30%），seccore 判决体 2 次/派发是**域外地板**；采纳判据 = median 改善 ≥5% 且 CI 不重叠，**否决亦为正式结论**（§3）。
4. **W5 裁决**：W5.0（hnsw_tiered 幽灵门补词条 + 新不变式 I10）为必闭；5.1 纠偏为「per-event 无界并发 → 有界并发 bound=8」（上游「串行 await」表述被现势证伪）；5.5 为纯核对确认点（ShardingPolicy 边界已全部实现）；5.3 倾向记录不采纳；5.4 需先找到第二生产调用源否则否决（§4）。
5. **X2 裁决**：N-1 收紧程序存在**两个深审新发现的隐藏缺口**（bench 执行器未声明能力 ⇒ 直接重跑必测串行；09:46 日志为能力表落地前测量不可用），9 步程序已内置两道护栏；**复核勘误撤回原「陷阱④」**——`tmp/x1_criterion_backup/toolplan_layers_pre_x3` 经本会话实测**存在且内容完整**（mean=63,804,161ns 恰为 ADR-252 登记口径），N-1 改前基线直接引用该目录，mean-同源口径（Q1）获得改前基线直接证据（§5，勘误并入处标〔已修正〕）。
6. **P1~P4 裁决**：P1 = McpToolExecutor 接入四环（默认 InProcess 零网络、能力 None 诚实声明、复用 2PC 参与者面）；P2 = nexus-subagent 组合根装配（QuestEngine 浅 Clone 案 β、禁嵌套深度 1 严于 chimera-mas ≤5，两委托面不合并）；P3 = nexus-hook register_all 生产化 + **深审增补项 DynamicEventRegistry 生产构造点**；P4 = U 项三态裁决（U-20 改判「已接线」、U-08 写面已接线仅读面预留、F-09 为接线候选随 P2），全局零新增事件变体（§6~§9）。
7. **勘误处置**：9 条复核勘误全部并入，其中 7 条完全实证采纳；2 条（勘误 8/9，spec.rs / main.rs 行号）方向正确但**自身行号与本次实测仍有偏差**，本终稿按「现场现取」纪律以本次 grep -n 实测值为准并在 §4.4/§4.6 显式登记矛盾（§4.4/§4.6、§5.1、§13）。
8. **开放问题**：跨波次共 27 项需主会话/用户裁决，统一编号 **OQ-01~OQ-27** 见 §13（OQ 编号与门禁 ID（G-27/G-35/G-88 等）**无关联**，避免撞名消歧）；含 W3 路线字面裁决、X2 与 W3 执行顺序、跨 await 豁免 stale 处置归属等阻塞性问题。

---

## 1. 现势复核（对齐 W1/W2 收官后状态）

### 1.1 目标

把执行基线钉在 W1/W2 收官后的 HEAD `a0d6b6b`，供后续五波（W3/W4/W5/X2/P1~P4）共享同一取证底座；区分「本会话复跑」与「facts.md 自述」。

### 1.2 HEAD 与工作树【实测·本轮】

- `git log -3 --oneline` → `a0d6b6b`（W2）/ `22adf6b`（W1）/ `810f18c`（W0.4-r，F-06 五 md 豁免 + CI 断言，ADR-250）；
- `git status --short` 零输出 ⇒ 工作树干净。
- 全景方案基线为 `c6eb9b6`（ADR-254 能力表落地）；`a0d6b6b` = `c6eb9b6` + W1 + W2 两个提交（facts.md §1）。

### 1.3 门禁现值【实测·本轮，本会话复跑】

| 门 | 命令 | 结果 | 关键读数 |
| --- | --- | --- | --- |
| G-1 | `python scripts/check_perf_redlines.py invariants` | **EXIT=0，RESULT: PASS** | I1 83/83、I2 1≤1（顶格）、I3/I7 55、I9 83（dict found=True）、I4a 1≤1、I5 (11 rl/6 slo)、I6 62、I8 comment=22/total=22、I8b scanned=4/total=22、I8c const=18/arms=18 |
| G-2 | `python scripts/check_perf_redlines.py inventory` | **EXIT=0，RESULT: PASS** | total=126 / gated=11 / registered=53 / dev-only=62 / **unknown=0**；`registered gqep-executor/toolplan_layers`、`registered chimera-cli/run_execute_batch` 在册 |

- 相对 ADR-252 时点值（79 键/54 run_bench）：**净增 +4 键/+1 run_bench** = W2 的 `runx_*` 四键 + `run_execute_batch` bench，双源对拍 PASS（facts.md §2 G-1 同口径）。
- 相对全景方案 §6.2 余量表：I2/I4a/I6 三项一致；**G-27 一项不符**——facts.md G-8 实测 `run_gate_manifest.py` mismatch=9（含 G-27/G-35 stale 条目 `participant_client.rs:556`），且 `git diff c6eb9b6..HEAD -- crates/mcp-mesh/` = 0 行 ⇒ 该红在方案基线时点已存在、非 W1/W2 引入（【材料实测】facts.md §2 G-8；处置归 W5 5.6 前置 + §13 开放问题）。
- facts.md G-3~G-8 的其余读数（config parity 177 行/16 字段、composition root 7 点、event_ownership 154/136/18/18、reachability 46 members/38 可达/0 孤岛/8 GATED、ci parity matched=69/unmatched=0）为本 Ask 未指定复跑项，**按【材料实测】引用，未在本轮独立复跑**（如实标注）。

### 1.4 台账现值【实测·本轮（关键行亲读）+【深审实测】】

| 台账 | 现值 | 证据 |
| --- | --- | --- |
| `VARIANT_COUNT_LOCK` | **154** | `crates/event-bus/tests/variant_count_test.rs:49`【实测·本轮 sed】 |
| `scripts/doc_count_freeze.txt` | **154** | `:25` `nexus_event_variants = 154`【实测·本轮 sed】 |
| `scripts/event_ownership_baseline.txt` 尾注 | `# total 152 variant(s): 134 owned / 18 without a production ctor` | `:36`【实测·本轮 sed】；实体台账行 18 与 G-5 门账本一致（facts.md §3），**尾注统计行漂移 ⇒ F-07 仍未闭合** |
| `tplan_undeclared_parallel` | **160000000**（160ms 改前上界，未收紧） | `scripts/perf_thresholds.toml:116` + `:104-115` 注释块（「ADR-254 能力表落地后本键应收紧到 ≈38ms（只收紧不放宽）」）+ heredoc `bench_check.yml:394` `160_000_000`【实测·本轮 sed/grep】；关键词两侧 `:270`/`:530` = `["toolplan_layers/undeclared8_ms2/8"]` |
| W2 增量键 | `runx_serial_zero/serial_1ms/parallel_zero/parallel_1ms` | `perf_thresholds.toml:272-275`【实测·本轮 sed】；注释记录 2026-11-07 本机 median 8.45×（`:117-122`） |
| ADR 登记行 | ADR-247→`:429`、ADR-254→`:431`、ADR-246→`:433`、**ADR-252→`:435`**、**ADR-253→`:437`** | `docs/architecture/adr_index.md`【实测·本轮 grep + 亲读】（勘误 2 处锚点） |
| F-06 五 md 豁免 | `.gitignore:81-85` 五条 `!` 豁免在 + `:80` 断言注释 | 【实测·本轮 sed】 |

### 1.5 W1/W2 闭环增量核对（facts.md §4 增量，本会话抽查关键点）

| 条目 | 现势 | 本轮抽查 |
| --- | --- | --- |
| F-01（W1） | `composition.rs:174` `tool_runtime_policy(config: &ChimeraConfig)`、`:180` 读 `config.tool_policy` | 【实测·本轮 sed :170-182】✓ |
| F-02/F-13（W2） | `run.rs:148` `ctx.tool_runtime.execute_batch(...)`（`execute_quest_tool_batch` 内，首个生产调用方）；`:277` `ensure_execute_authorized` 前置唯一化 | 【深审实测】facts.md §4.1②（本会话未逐行复读 run.rs，行号以 facts 为准） |
| N-7（W2） | `run.rs` 内 `_perm` 零命中、形参转正 | 【深审实测】facts.md §4.2 |
| `execution_batch` 生产调用方 | `execute_quest_tool_batch` 内 `:148` 唯一新增 | 【深审实测】facts.md §4.1② |

### 1.6 回滚 / 验收命令 / ADR 决策点

- 本章为复核章，无施工量；验收命令 = §1.3 两行（本会话已跑，EXIT=0）。
- 任何波次开工前重跑 §1.3 两命令取余量读数（ADR-239 纪律：新增阈值/事件/豁免前必查，见 §11）。

### 1.7 波次全局执行顺序与依赖图（开工前必读）

> 合并全景方案 §5.0 执行序 + 各深审局部约束（W5.0 的 I10 先于第一个新键 §4.3；X2 bench 窗口唯一且审计期禁跑 §5.4；X2 与 W3 先后 = OQ-19）。**「并行」仅指不同 crate/文件的代码开发可同时进行；合入与提交一律按主干序。**

```
【阶段二】
W5.0（I10 先落 —— 硬前置：阻塞 W5 全部新键、且阻塞 W4-9 的 bchain_* 双写）
 └─ W3（计数器路线 C-A；event-bus/gqep-executor/chimera-cli 三 crate；单 commit 可 revert）
     └─ W4（与 W3 可并行开发 —— W4-5 seccore 与 W3-1 event-bus 分属不同 crate；
         合入在 W3 后。若 OQ-01 裁事件路线，则 W4 合入必须待 W3 事件台账单批次完成）
         └─ W5（5.1~5.6 逐点；5.6 的 stale re-key 前置修复应在 W5 首个改码点之前）
             └─ X2（N-1 的 bench 窗口 = 全支唯一 bench 期，置于 W4/W5 稳定后跑，
                 避免 W4-6/W5 改码使 bench 基线中途失效；F-07 顺序受 OQ-19 约束）
【阶段三】（P1 依赖 W1+W2 已收官的策略/批量语义；OQ-04 裁决 N-6 归属后开工）
P1（mcp_tool_executor + composition 一行；依赖 OQ-04）
 └─ P2（nexus-subagent 装配；与 P1 无代码耦合，可并行开发；P4 的 F-09 随 P2）
     ├─ P3（nexus-hook 装配；与 P2 无耦合，可并行开发）
     └─ P4（收尾裁决波 —— 必须在 P2 之后：F-09 发布点随 P2 落地、U-20 文档化小项随 P2/P3 现势写终态）
W6 收尾验收（全景方案 §5.7：收官报告 + ADR-246~259 复核 + 回滚演练）压轴
```

- **强约束汇总**：① I10 先于一切新键（W4-9/W5 新键都过 `invariants`，I10 缺位则双写漏接无法机检）；② X2 bench 窗口唯一（深审/审计期禁跑，W4/W5 改码完成后再跑，防止基线漂移）；③ W3 若走事件路线（OQ-01 否决计数器），F-07 三账在 155 上一次闭合（OQ-19）；④ P4 在 P2 后；⑤ 每波合入前跑 §10.1 门禁列。
- **可并行矩阵**：W3-1/W3-2（event-bus）‖ W4-5（seccore）‖ W5-5.1（action_orchestrator）——三者不同 crate；P2（nexus-app-server/quest-engine）‖ P3（nexus-hook/event-bus 装配面）——不同 crate。
- **首波建议**：W5.0 单独成 commit 先行（纯 CI/脚本面，零 Rust 行为），为后续所有波次建立双源对拍地基。

---

## 2. W3 规格 —— P-13 并行归因指标（BusLogger 计数器路线，F-03/U-15，ADR-248）

### 2.1 目标

1. `BatchAttribution` 归因快照从 `tracing::debug!` 升级为**运行期可观测**：组合根注入 `BatchAttributionSink`，计数器经 BusLogger Registry 真实累加并可 `render_metrics()` 渲染——以「运行期可观测」兑现 F-03 的「接入」语义（显式偏差：不引入 nexus-telemetry，见 §2.8 决策 5）。
2. **零事件变体新增**：完全绕开六锁/七点位与 `VARIANT_COUNT_LOCK` 漂移（事件路线回退清单完整收录为本文件 **§2.10**——深审材料在 tmp/ 不入库，故必须收录进本稿；仅在主会话按字面裁决 OQ-01「不可行」时启用）。
3. 旧签名不变（A5 功能不削减）：`dispatch_batch` 逐字保留，**`sink=None` 时行为逐位等价今日**；`sink=Some` 时为「新增计数器 + 保留既有 `tracing::debug!`」（A5 只约束 None 路径的逐位等价，见 §2.4② 裁决）。

### 2.2 五元组（模块 | 文件:行号 | 影响 | 阻塞 | 前置）

| # | 模块 | 文件:行号 | 影响 | 阻塞 | 前置 |
| --- | --- | --- | --- | --- | --- |
| W3-1 | event-bus（L1） | `crates/event-bus/src/logging.rs:120-141`（结构体字段私有）、`:157-198`（`new` 仅注册 3 固定指标）【深审实测 + 实测·本轮复核 :120-141/:157-186】 | 增 `register_counter` + `RegisteredCounter`（加性 API，域中立） | 无（加性） | 无 |
| W3-2 | event-bus（L1） | `crates/event-bus/src/bus.rs:1113-1116`（`set_logger` 收值包 Arc）、`:1120-1121`（`logger() -> Option<&BusLogger>`）【实测·本轮 sed】 | 组合根「先注册后 set」顺序约束；渲染面已可达 | 无 | W3-1 |
| W3-3 | gqep-executor（L7） | `crates/gqep-executor/src/tool_runtime/batch.rs:144-158`（`BatchAttribution`）、`:296-305`（现状仅 `tracing::debug!`）【实测·本轮 sed :294-306】 | 增 `BatchAttributionSink` trait + `dispatch_batch_with_sink`；旧签名不变（A5） | 无（加性） | 无 |
| W3-4 | gqep-executor（L7） | `crates/gqep-executor/src/tool_runtime/runtime.rs:202-208`（`execute_batch`）、`:148-186`（`with_optional_delegate`）【深审实测】 | `ToolRuntime` 持 `Option<Arc<dyn BatchAttributionSink>>`；增 `with_batch_sink` 消费式 builder | 无（加性） | W3-3 |
| W3-5 | chimera-cli（L10 组合根） | `crates/chimera-cli/src/composition.rs:260`（`bus.set_logger(BusLogger::new("chimera-cli"))` 单行）、`:305-313`（ToolRuntime 装配块）【实测·本轮 sed 两处】 | 拆 :260 为「构造→注册 5 计数器→set_logger」；装配块注入 `TelemetryBatchSink` | 唯一装配点纪律（G-4/check_composition_root.py R1 允许，facts.md G-4） | W3-1/W3-4 |
| W3-6 | chimera-cli（L10） | 新增 `crates/chimera-cli/src/tool_batch_telemetry.rs`（构造调用须留在组合根） | `TelemetryBatchSink` 结构体 + `BatchAttributionSink` impl（持 5 个 `RegisteredCounter` 句柄） | 无 | W3-1/W3-3 |
| W3-7 | chimera-cli（L10） | `crates/chimera-cli/src/commands/run.rs:321-323`【深审实测】 | 注释订正：预写「W3 起逐批归因统一由 ToolBatchAttributed 承载」→ 改「由 TelemetryBatchSink 计数器承载」，防预言成陷阱 | 无 | W3 路线裁决 |
| W3-8 | 测试 | `batch.rs`/`logging.rs` 单元 + `crates/chimera-cli/tests/tool_batch_telemetry.rs`（新增集成） | 消费者闭合断言（§2.5 W3-T7） | — | W3-1~W3-6 |

### 2.3 现势取证（Spike 结论）

**问：BusLogger 的 `Family<Counter>`/`Registry` 是否暴露自定义计数器注册 API？答：未暴露。**【深审实测 W3.md §3.1 + 实测·本轮复核】

1. `logging.rs:120-141` `BusLogger` 全部字段私有——`registry: Registry`（:134）、`event_total: Family<TopicLabelSet, Counter>`（:136）、`critical_event_total: Counter`（:138）、`publish_duration: Histogram`（:140）【实测·本轮 sed :100-141 逐行】；
2. `logging.rs:157-198` `pub fn new(module_id)` 仅 3 次 `registry.register(...)`：`nexus_event`（:162）/`nexus_critical_event`（:170）/`nexus_event_publish_duration_seconds`（:182），无对外注册入口【实测·本轮 sed :157-186】；
3. impl 块全量通读（W3.md §3.1-3）：公开 API 仅 `new/render_metrics/log_*/total_*`，**无 register/get_or_create 类公开方法**；`TopicLabel`(:66-77)/`TopicLabelSet`(:100-103) 私有 ⇒ 复用既有 Family 的 get_or_create 通道对外关闭【深审实测】；
4. `bus.rs:1113-1116` `set_logger(&mut self, logger: BusLogger)` 按值收编 `Arc<BusLogger>`；`bus.rs:1120-1121` `logger() -> Option<&BusLogger>` 只读借用【实测·本轮 sed】；
5. 可行性排除表（W3.md §3.2）：Registry 可注册（prometheus-client 0.22 标准用法，`new` 内已在用）、Counter 句柄可 clone（Arc 型）、组合根可在 `set_logger` 前拿句柄（构造期 `&mut self`）、依赖零新增（`crates/event-bus/Cargo.toml:29` + workspace `Cargo.toml:167` = 0.22；chimera-cli 已依赖 event-bus）【深审实测】。

**裁决：计数器路线可行，需一笔域中立加性小改（≈15-25 行 + 测试）。** 全景方案 §5.4 把二选一挂在「是否已暴露 API」的字面条件上；按工程意图裁决走计数器路线，字面分歧列 OQ-01 供主会话确认。

### 2.4 接口签名（Rust 2021 edition）

**① sink trait（`crates/gqep-executor/src/tool_runtime/batch.rs`，紧随 `BatchAttribution` 之后）**：

```rust
/// 批量归因数据汇 — 依赖倒置接缝（F-03/W3，ADR-248）
///
/// WHY trait 注入而非直接依赖指标库：本 crate 保持零指标依赖
/// （batch.rs 既有承诺），prometheus/telemetry 由上层组合根装配。
/// WHY 只有有限取值域聚合计数：高基数（工具名/错误文案）禁入指标标签
/// （batch.rs :294-295 既有纪律），逐条细节仍由 `BatchOutcome::results` 承载。
pub trait BatchAttributionSink: Send + Sync {
    /// 记录一次批量归因快照。
    ///
    /// # 实现方契约
    /// - 不得 panic、不得长阻塞（在批量返回临界路径上同步调用）；
    /// - 实现自身保证线程安全（`Send + Sync`）。
    fn record(&self, a: &BatchAttribution);
}
```

**② 双签名（旧签名逐字不变，A5）**：

```rust
/// 既有签名不变（A5）——薄委托，行为逐位等价于 `sink=None`
pub async fn dispatch_batch<'a>(
    dispatcher: &'a dyn ToolDispatcher,
    requests: &'a [DispatchRequest],
    limits: BatchLimits,
) -> BatchOutcome {
    dispatch_batch_with_sink(dispatcher, requests, limits, None).await
}

/// 新增：带归因汇的批量派发
pub async fn dispatch_batch_with_sink<'a>(
    dispatcher: &'a dyn ToolDispatcher,
    requests: &'a [DispatchRequest],
    limits: BatchLimits,
    sink: Option<&'a dyn BatchAttributionSink>,
) -> BatchOutcome
```

发布点：在 `outcome` 构造完成（现 batch.rs :280-286）之后、返回之前；`sink.is_some() && total > 0` 才调用（**total==0 不发**，防刷屏）；调用点旁注释引用 `is_conserved()` 守恒（batch.rs :104-106）——发送的 `BatchAttribution` 由 `outcome.attribution()`（:122-136）派生，字段与 outcome 逐位同源【深审实测】。

**〔修订·审读〕既有 `tracing::debug!`（batch.rs:296-305）在 sink=Some 时**保留**（不删）**：理由 ① A5 的「行为逐位等价」承诺只约束 `sink=None` 路径（§2.1-3），sink=Some 本就是新增观测面，保留 debug! 不违反；② debug 级别零生产开销，删除反而是**削减既有观测**（与全景方案上游 W3 步骤 3「同时保留原 tracing::debug! 结构化字段，不削减既有观测」一致）；③ 「total==0 不发」的防刷屏纪律针对 sink.record，与 debug! 无关。裁决记入 ADR-248 决策 3。

**BatchAttribution 字段权威清单（§2.5 W3-T3/T7 与 §2.4⑤ 实现的共同依据）**【实测·本轮 sed batch.rs :120-160】：

| 字段 | 类型 | 语义 | TelemetryBatchSink 对应物 |
| --- | --- | --- | --- |
| `total` | `usize` | 请求总数（= `results.len()`） | 计数器 `tool_batch` |
| `succeeded` | `usize` | 成功数 | 计数器 `tool_batch_succeeded` |
| `failed` | `usize` | 失败数（含单操作超时，不含全局 deadline 放弃/未启动） | 计数器 `tool_batch_failed` |
| `abandoned` | `usize` | 已入流被全局 deadline 放弃 | 计数器 `tool_batch_abandoned` |
| `not_started` | `usize` | 从未入流（并发上界下 backlog 尾部） | 计数器 `tool_batch_not_started` |
| `success_ratio_bp` | `u32` | 成功率千分比（`total==0` 时 0；**派生量**） | **不设计数器**——比率非累加量，且可由 `tool_batch_succeeded/tool_batch` 两计数器在读取面派生；逐条细节仍由 `BatchOutcome::results` 承载 |

- `is_conserved()` 是 `BatchOutcome` 的四段守恒方法（`succeeded+failed+abandoned+not_started == results.len()`，batch.rs :104-106【实测】）——覆盖前 5 个计数型字段（total 是 results.len() 本身）；`success_ratio_bp` 由前五字段派生（attribution() 内同式计算），不进守恒式。**W3-T3 的「六字段逐位一致」精确化为：5 个计数字段与 `BatchOutcome` 对应字段逐位相等 + `success_ratio_bp` 与 attribution() 派生式一致。**

**③ ToolRuntime 侧（`runtime.rs`）**：

```rust
pub struct ToolRuntime {
    dispatcher: SandboxedDispatcher,
    capabilities: Option<Arc<dyn ToolExecutor>>,
    /// 批量归因汇 — None = 现行为（L1 等价，单点可回退）
    batch_sink: Option<Arc<dyn BatchAttributionSink>>,   // 新增字段
}

impl ToolRuntime {
    /// 消费式 builder：注入批量归因汇（组合根专用；其余字段不变）
    #[must_use]
    pub fn with_batch_sink(mut self, sink: Arc<dyn BatchAttributionSink>) -> Self {
        self.batch_sink = Some(sink);
        self
    }
}

// execute_batch(:202-208) 改为：
pub async fn execute_batch(&self, requests: &[DispatchRequest], limits: BatchLimits) -> BatchOutcome {
    dispatch_batch_with_sink(
        &self.dispatcher, requests, limits,
        self.batch_sink.as_deref(),   // Option<&dyn BatchAttributionSink>
    ).await
}
```

**④ BusLogger 加性 API（C-A 主案，`logging.rs`）**：

```rust
/// 已注册计数器句柄 — prometheus Counter 的最小封装（Clone 廉价：Arc 句柄）
#[derive(Clone, Debug)]
pub struct RegisteredCounter { inner: prometheus_client::metrics::counter::Counter }

impl RegisteredCounter {
    pub fn inc(&self);
    pub fn inc_by(&self, v: u64);
    /// 读数（测试断言与 doctor 探针用；Prometheus 渲染仍走 Registry）
    pub fn get(&self) -> u64;
}

impl BusLogger {
    /// 注册一个自定义计数器并返回可 inc 的句柄。
    ///
    /// # 前置
    /// 必须在 `set_logger`/clone 之前调用（收编 Arc 后不可再 &mut）。
    ///
    /// # 命名纪律
    /// `name` 不得以 `_total` 结尾——prometheus-client 对 Counter 在文本
    /// 编码时自动追加 `_total`（logging.rs :154-156 既有注释），手动追加会
    /// 变成 `_total_total`。注册 `"tool_batch"` → 渲染 `tool_batch_total`。
    pub fn register_counter(&mut self, name: &str, help: &str) -> RegisteredCounter;
}
```

实现要点：`let c = Counter::default(); self.registry.register(name, help, c.clone()); RegisteredCounter { inner: c }`（模式同 `new()` :161-186【实测·本轮】）。**不**新增 HashMap 登记——Registry 本身承担存储与渲染。

**⑤ 组合根 sink（C-A，`crates/chimera-cli/src/tool_batch_telemetry.rs` 新文件）**：

```rust
/// 工具批量归因计数器组 — BusLogger Registry 的 5 个计数器句柄
pub(crate) struct TelemetryBatchSink {
    total: RegisteredCounter,
    succeeded: RegisteredCounter,
    failed: RegisteredCounter,
    abandoned: RegisteredCounter,
    not_started: RegisteredCounter,
}

impl TelemetryBatchSink {
    /// 注册 5 计数器（须在 `bus.set_logger` 之前、对 BusLogger 构造期调用）
    pub(crate) fn register(logger: &mut BusLogger) -> Self {
        Self {
            total: logger.register_counter("tool_batch", "Tool batch dispatches"),
            succeeded: logger.register_counter("tool_batch_succeeded", "Tool batch requests succeeded"),
            failed: logger.register_counter("tool_batch_failed", "Tool batch requests failed"),
            abandoned: logger.register_counter("tool_batch_abandoned", "Tool batch requests abandoned by global deadline"),
            not_started: logger.register_counter("tool_batch_not_started", "Tool batch requests never started"),
        }
    }
}

impl BatchAttributionSink for TelemetryBatchSink {
    fn record(&self, a: &BatchAttribution) {
        self.total.inc_by(a.total as u64);
        self.succeeded.inc_by(a.succeeded as u64);
        self.failed.inc_by(a.failed as u64);
        self.abandoned.inc_by(a.abandoned as u64);
        self.not_started.inc_by(a.not_started as u64);
        // 渲染名（prometheus-client 自动加 _total）：tool_batch_total /
        // tool_batch_succeeded_total / tool_batch_failed_total /
        // tool_batch_abandoned_total / tool_batch_not_started_total
    }
}
```

命名说明与正则裁决（〔修订·审读〕统一口径）：`check_composition_root.py` 的装配正则 **`ASSEMBLY_CALL = re.compile(r"\b(EventBus|QuestEngine|McaGateway|AppServer)::(?:new|with_[A-Za-z_]*)\s*\(")`**（`scripts/check_composition_root.py:51-55`【实测·本轮亲读】）——**仅匹配上述四类型的 `new`/`with_*` 关联函数**；`ToolRuntime::with_batch_sink`（类型不在名单内）与 `TelemetryBatchSink::register`/`RegisteredCounter::inc_by` 均不命中 ⇒ **组合根门「基线 7 点不变」成立（W3-T9 的依赖条件闭环）**。本稿 §8.3 引 P1-P4.md 的「:51-53」与早期「`Type::new`/`Type::with_*`」宽泛描述统一订正为本实测口径。

### 2.5 TDD 矩阵

> TDD 纪律：先写失败测试再实现（AGENTS.md §3.4.1 原则 3）。每条注明先红方式【深审实测 W3.md §5】。

| # | 层 | 测试 | 断言核心 | 先红方式 |
| --- | --- | --- | --- | --- |
| W3-T1 | 单元（event-bus） | `register_counter_renders_with_total_suffix` | `register_counter("tool_batch", ...)` 后 `render_metrics()` 含 `tool_batch_total` 行（**不含** `_total_total`）；`inc_by(3)` 后渲染值 `3`；`get()==3` | API 不存在 ⇒ 编译红 |
| W3-T2 | 单元（event-bus） | `register_counter_after_arc_is_not_required` | 构造期注册 + `set_logger` 后 `bus.logger().unwrap().render_metrics()` 可读 | 同上 |
| W3-T3 | 单元（gqep-executor） | `dispatch_batch_with_sink_records_once` | CountingSink：N=5 请求 ⇒ `record` 恰 1 次；入参 `BatchAttribution` 六字段（清单见 §2.4②）中 5 个计数字段与 `BatchOutcome` 对应字段逐位相等 + `success_ratio_bp` 与派生式一致，且 `is_conserved()` | trait 不存在 ⇒ 编译红 |
| W3-T4 | 单元（gqep-executor） | `sink_not_called_for_empty_batch` | `requests=[]` ⇒ `record` 0 次 | 同上 |
| W3-T5 | 单元（gqep-executor） | `sink_none_matches_dispatch_batch`（proptest 64） | `dispatch_batch` 与 `dispatch_batch_with_sink(..., None)` 结果逐位相等 | 同上 |
| W3-T6 | 单元（gqep-executor） | `sink_record_is_on_return_path_not_per_request` | N 条请求 ⇒ record 恰 1 次（非 N 次） | 同上 |
| W3-T7 | 集成（chimera-cli） | `composition_root_batch_counters_render`（新增 `tests/tool_batch_telemetry.rs`） | `composition::build(&config)` → `ctx.tool_runtime.execute_batch(&[text.echo 请求], default)` 成功 1 条 ⇒ `ctx.bus.logger().unwrap().render_metrics()` 含 `tool_batch_total 1` 与 `tool_batch_succeeded_total 1`。**消费者闭合断言**：锁「生产装配真实会被发布→采集→渲染」，非「能构造」 | sink 未注入 ⇒ 断言红 |
| W3-T8 | 集成（chimera-cli） | `tool_runtime_wiring` 反向断言扩展（既有 `tests/tool_runtime_wiring.rs:268` 模式）【深审实测】 | 源码扫描：`with_batch_sink` 调用点仅 composition.rs（无第二装配点） | 人为旁路 ⇒ 红 |
| W3-T9 | 门 | `python scripts/check_composition_root.py` | rc=0（基线 7 点不变——成立依据：`with_batch_sink` 不命中 ASSEMBLY_CALL 正则，见 §2.4⑤ 正则裁决） | — |

### 2.6 同轮对照基准（W3 无新 bench，回归护栏）

- `bus_bench`（阈值 `perf_thresholds.toml:165` = 200000ns）与 `cross_layer`（`:50` = 100000ns）作回归护栏：C-A 不改 `bus.rs` 发布路径（仅 logging.rs 构造期加性），预期零退化；实施者跑两 bench 对比基线，退化 >10% 即排查【深审实测 W3.md §7.4；本深审未运行 bench，为实施者验收项】。
- C-A 的单一 Registry 优势：U-20 文件 dump 只需 `bus.logger().render_metrics()` 一次渲染即含全部指标（避免进程指标面双源分叉）；counter 渲染自动走 logging.rs :154-156 的 `_total` 后缀规则【深审实测 W3.md §3.3】。

### 2.7 回滚

| 层 | 手段 |
| --- | --- |
| L1 秒级 | sink 语义自带回退：`ToolRuntime` 不注入 sink = 今日行为逐位一致（None 分支）；计数器多渲染 5 行无害 |
| L2 commit revert | 单 commit revert：event-bus API + sink 接线 + 组合根两处 + run.rs:321 注释；revert 后 `cargo test -p gqep-executor -p event-bus -p chimera-cli` |
| 判据触发 | bus/cross_layer 退化 >10% 且不可定位 → revert（采样发布方案另行评估） |

### 2.8 验收命令与 ADR-248 决策点

验收命令：

```bash
cargo check --workspace --all-targets
cargo test -p event-bus -p gqep-executor --lib
cargo test -p chimera-cli --test tool_batch_telemetry -- --test-threads=1
python scripts/check_composition_root.py                     # rc=0
python scripts/check_perf_redlines.py invariants             # I1~I9 键数不变（83/55/83）
python scripts/check_event_ownership.py                      # 154/136/18/18 不变（零事件变体证明）
# 回归护栏（实施者跑）：
# cargo bench -p event-bus --bench bus_bench / cross_layer_bench（退化 ≤10%）
```

ADR-248 决策点（W3.md §9.1 全量收录）：

1. **路线**：计数器路线（零事件变体）采纳；子路线 C-A（BusLogger 通用 `register_counter`）为主案、C-B（chimera-cli 本地 Registry，进程内双 Registry 需两段渲染拼接）为回退——ADR 落档时终裁。
2. **sink 依赖倒置**：trait 在 L7 batch.rs，实现在 L10 组合根；gqep-executor 零指标依赖承诺不变。
3. **发布纪律**：total==0 不发（针对 sink.record）；`record` 契约不 panic 不长阻塞；消费者闭合断言锁「会被发布→可渲染」（W3-T7）；**既有 `tracing::debug!` 保留不删**（不削减既有观测；A5 逐位等价承诺仅约束 sink=None 路径，§2.4②）。
4. **失效条件**：出现第二 sink 装配点 ⇒ composition-root 门红；`register_counter` 名称带 `_total` ⇒ 渲染断言红（W3-T1）。
5. **显式偏差**：F-03 五元组原文「未接入 nexus-telemetry」——本设计不引入 nexus-telemetry（其职责为计算面饱和遥测，`crates/nexus-telemetry/src/lib.rs:1-40` CRATE-CONTRACT【深审实测】；强行接入徒增 L0-B↔L7 缝隙）；「接入」以「运行期可观测（计数器真实累加+可渲染）」兑现。**此偏差须在 ADR-248 登记。**

### 2.10 W3 事件路线回退清单（仅当 OQ-01 按字面裁决「不可行」时启用；自 W3.md 附录 A 收录——深审材料不入库，此处为本清单唯一可达载体）

> 启用即按全景方案 §5.4「七点位单 commit + F-07 三账同批对齐为 155」执行。

**七点位**（变体 `NexusEvent::ToolBatchAttributed`，定级 Normal/System、Topic=Execution、**不进 Critical mpsc**）：
1. `crates/event-bus/src/types.rs` 增变体（含 `metadata` + 六字段载荷或 payload 结构）；
2. `crates/event-bus/src/registry.rs` `define_event_registry!` 注册定级（不定级即编译失败；AGENTS.md §5.3）；
3. `crates/event-bus/tests/variant_count_test.rs` `all_variant_samples()` + `variant_ordinals()` 登记样本；
4. `VARIANT_COUNT_LOCK 154→155`（`variant_count_test.rs:49`）+ 分布锁 `NORMAL_LOCK 121→122`（`:55`；Critical/Info 不变 :51/:53）——维护规约见该文件 :20-25；
5. severity 分布锁 + `LANE_FORBIDDEN_SHARD` 双向互锁核对（variant_count_test.rs:15-18；Normal 级不触 `bus.rs` 的 `CRITICAL_MPSC_VARIANTS=18`/`CRITICAL_TOTAL=22`，facts.md I8/I8c）；
6. `scripts/doc_count_freeze.txt:25` `nexus_event_variants = 154→155`；
7. F-07 三账同批：`python scripts/check_event_ownership.py --emit` 机器再生 `scripts/event_ownership_baseline.txt`（现尾注 total 152 漂移一并闭合）+ 生产构造点（`ToolRuntime` 需持 `Arc<EventBus>` 发布——gqep-executor 已依赖 event-bus，`crates/gqep-executor/Cargo.toml:14`）+ 消费者闭合测试（禁零消费者构造点，U-10/`check_event_ownership.py` 明禁）+ TUI 穷尽 match 补臂（`crates/chimera-tui/src/` 内 `NexusEvent::` 消费面 grep 命中 ≥5 文件）+ `run.rs:321` 注释兑现。

**六锁全绿**：① 编译期穷举锁（variant_ordinals 无通配）② 计数锁（155）③ 分级分布锁（23/11/122，若 Critical 不变则 22/11/123——以定级裁决为准）④ registry 宏锁 ⑤ doc_count_freeze ⑥ ownership 台账。**附加成本**：I2 子串检查、`bench_check` 无涉、`cross_layer`/`bus_bench` 回归、每发布点 `publish_blocking`/async 纪律（AGENTS.md §4.4-8）。

> 收录说明：本节内容为 W3.md 附录 A 的逐字收录（该深审文件在 tmp/ 不入库），行号为深审时点 HEAD `a0d6b6b` 值；启用前按 §0 行号时效纪律现取复核。

---

## 3. W4 规格 —— 链内全借用化「反转主次」（F-04/U-14，ADR-249）

### 3.1 目标

1. 设计链内全借用化最小改动：链内脊柱换借用 trait + 适配器，严守 ADR-241 D3 **不另建链**；拥有型 `dispatch` 退化为零分配适配器（反转主次，权威文本 = 全景方案 §5.5 第 2 条 `toolchain-parallel-optimization-w7r2-panorama-plan-2026-11-07.md:273` 与 ADR-249 行 `:411`【深审实测 W3.md 头注溯源订正——ask 所引「上游 §12.5」在盘内不可达，原文 `radiant-thunder-darwin-xenXoOpR.md` 全库 0 命中】）。
2. 给出静态收益上限（分配清单）与 256-case 判决等价扩展。
3. `dispatch_borrowed_chain/{owned,borrowed}×{c1,c4,c8}` 同轮对照 bench；判据「median 改善 ≥5% 且 CI 噪声区间不重叠，否则否决」。
4. 顺带评估 N-6（`ExecPolicyToolExecutor` 能力转发，§3.7）。

### 3.2 五元组

| # | 模块 | 文件:行号 | 影响 | 阻塞 | 前置 |
| --- | --- | --- | --- | --- | --- |
| W4-1 | gqep-executor | `crates/gqep-executor/src/tool_runtime/chain.rs:246-261`（`dispatch_borrowed` 仍 `to_owned_request`）、`:30-127`（四层各持 `next: Arc<dyn ToolDispatcher>`）、`:129-200`（builder）、`:229-233`（owned 入口）【深审实测 + 实测·本轮复核 :244-261 dispatch_borrowed 体】 | 四层换借用脊柱 + 删各自 owned 实现；`SandboxedDispatcher` 双实现；`check_ref` 生产接线 | 借用链与拥有链判决等价（256-case 锁） | W4-2 |
| W4-2 | gqep-executor | `crates/gqep-executor/src/tool_runtime/dispatch.rs:39-45`（`BorrowedToolDispatcher` 既有契约）【实测·本轮 sed】、`crates/gqep-executor/src/tool_runtime/mod.rs:18`（已导出）【深审实测】 | 脊柱类型 = 既有 `BorrowedToolDispatcher`（**不新增同形 trait**，§3.3-①裁决）+ 新增 `OwnedDispatcherCompat` 适配器 | 无 | 无 |
| W4-3 | gqep-executor | `crates/gqep-executor/src/tool_runtime/inproc.rs:29-43`（叶子已 `&str` 直传）【深审实测】 | `InProcDispatcher` 增借用原生实现（零分配直达 `ToolExecutor::execute(&str,&str)`）+ 拥有适配 | 无 | W4-2 |
| W4-4 | gqep-executor | `crates/gqep-executor/src/tool_runtime/runtime.rs:77`、`:166`（`inproc: Arc<dyn ToolDispatcher>` 两处）、`:79-90`/`:169-180`（builder 喂参）、`:99-101`（owned 入口）、`:112-118`（borrowed 入口）【深审实测】 | 装配面换 `Arc<dyn BorrowedToolDispatcher>`；`ToolRuntime::dispatch` 改经 `SandboxedDispatcher` owned 适配 | 生产 API 语义零变化（A5 面） | W4-1/W4-3 |
| W4-5 | seccore（L4） | `crates/seccore/src/execpolicy.rs:133-142`（`RulePattern::matches` 收 `&[String]`）、`:217-231`（`evaluate`/`evaluate_uncounted`）【深审实测】 | 增 `evaluate_ref(&[&str])` + `matches` 泛型化 `S: AsRef<str>`（先例：`crates/seccore/src/policy.rs:329-331`） | 判决逐位一致（复用同一判决体） | 无 |
| W4-6 | gqep-executor | `crates/gqep-executor/src/tool_runtime/chain.rs:43-45`（PolicyLayer 调 `evaluate`）；`crates/gqep-executor/src/exec_tool_executor.rs:68`（`&[args_json.to_string()]` 强制分配）【实测·本轮 sed :68 逐行】 | 借用路径改调 `evaluate_ref`；:68 顺手切换（消 1 次分配/派发） | W4-5 先行 | W4-5 |
| W4-7 | seccore（仅静态分析） | `crates/seccore/src/policy.rs:254-266`（`validate_command_verdict`）、`:332-342`（`build_full_command_string`）、`:227`（`check_whitelisted` 的 `to_lowercase`）、`:201-202`（非 ASCII 条件）【深审实测】 | **不改码**；作为收益地板的静态清单（§3.5） | — | — |
| W4-8 | 测试/bench | `crates/gqep-executor/tests/tool_runtime_borrowed_equivalence.rs:58`（`with_cases(64)`）→ 256；新增 `crates/gqep-executor/benches/dispatch_borrowed_chain.rs`【深审实测】 | 等价矩阵扩展 + 同轮对照 bench | — | W4-1~W4-6 |
| W4-9 | 阈值双源 | `scripts/perf_thresholds.toml` + `.github/workflows/bench_check.yml` heredoc | `bchain_*` 前缀键族登记（先实测后登记；I1/I3/I7/I9 对拍） | 棘轮余量核查（I2=1≤1 顶格） | W4-8 |

### 3.3 接口签名（Rust 2021 edition）

**① 脊柱类型裁决：复用既有 `BorrowedToolDispatcher`（dispatch.rs:39-45），不新增 `ToolDispatcherRef` trait。**
理由：其方法签名与上游设想完全同形（`fn dispatch_borrowed<'a>(&'a self, req: &'a DispatchRequestRef<'a>) -> BorrowedDispatchFuture<'a>`，dispatch.rs:41-44【实测·本轮】），已导出（mod.rs:18、lib.rs:107）、已承载等价契约文档（dispatch.rs:30-38）。再立同形 trait = 第二份契约双源（漂移风险），违背 ADR-241 D3「不另建」精神。ADR-249 落档写明「`ToolDispatcherRef`（上游概念名）≡ 既有 `BorrowedToolDispatcher` 复用裁决」【深审实测 W3.md §4.3-①】。

**② 「blanket 适配」的 coherence 订正（对全景方案 §5.5-① 的设计修正）**：

- `impl<T: ToolDispatcher + ?Sized> BorrowedToolDispatcher for T` 全量 blanket **不可行**：`SandboxedDispatcher` 必须同时实现两 trait（owned 公开入口 + 借用脊柱），命中 E0119；
- 反向 `impl<T: BorrowedToolDispatcher + ?Sized> ToolDispatcher for T` **不可行**：`ToolDispatcher` 是 L0 外部 trait（nexus-contracts），裸类型参数 blanket 违反孤儿规则（E0117）；
- **替代：显式适配器（newtype）+ 手工适配 impl**【深审实测 W3.md §4.3-②】：

```rust
// dispatch.rs 新增 — 「拥有型 → 借用脊柱」的显式升级缝
// （升级点可见，tool_dispatch.rs:69-72 同款纪律）
/// 让只讲拥有型 `ToolDispatcher` 的叶子/夹具接入借用脊柱；
/// 代价 = 入站 2 次分配（与 ADR-241 D6 记录的转换成本同源，显式化而非隐藏）。
pub struct OwnedDispatcherCompat(pub Arc<dyn ToolDispatcher>);

impl BorrowedToolDispatcher for OwnedDispatcherCompat {
    fn dispatch_borrowed<'a>(&'a self, req: &'a DispatchRequestRef<'a>) -> BorrowedDispatchFuture<'a> {
        let inner = Arc::clone(&self.0);
        Box::pin(async move {
            let owned = req.to_owned_request();
            inner.dispatch(&owned).await
        })
    }
}
```

**③ 四层改造（`chain.rs`，每层删 owned 实现、只留借用路径）**【深审实测 W3.md §4.3-③】：

```rust
pub(crate) struct PolicyLayer {
    policy: Arc<ExecPolicy>,
    next: Arc<dyn BorrowedToolDispatcher>,          // 原 Arc<dyn ToolDispatcher>（:32）
}
impl BorrowedToolDispatcher for PolicyLayer {
    fn dispatch_borrowed<'a>(&'a self, req: &'a DispatchRequestRef<'a>) -> BorrowedDispatchFuture<'a> {
        // evaluate 的 &[String] 签名会强制分配 → 走 evaluate_ref（W4-5）
        let action = self.policy.evaluate_ref(req.tool_name, &[req.args_json]);
        match action { /* 三臂消息与今日 chain.rs:47-58 逐字一致，仅 next.dispatch(req) → next.dispatch_borrowed(req) */ }
    }
}
// SandboxLayer：借用路径调 self.gate.check_ref(req)（sandbox_gate.rs:166-180 既有实现）——check_ref 生产接线即此
// RouteLayer：routes: HashMap<String, Arc<dyn BorrowedToolDispatcher>>；HashMap::get(&req.tool_name) 对 &str 直接可用，零分配
// TransportLayer：inner: Arc<dyn BorrowedToolDispatcher>，直转发
```

**④ 链头双实现（`SandboxedDispatcher`）**【深审实测 W3.md §4.3-④；〔修订·审读〕owned 适配的借用期形态订正】：

```rust
impl BorrowedToolDispatcher for SandboxedDispatcher {
    fn dispatch_borrowed<'a>(&'a self, req: &'a DispatchRequestRef<'a>) -> BorrowedDispatchFuture<'a> {
        self.head.dispatch_borrowed(req)     // head: Arc<dyn BorrowedToolDispatcher>，零分配
    }
}
impl ToolDispatcher for SandboxedDispatcher {
    /// 拥有型退化为零分配适配器（反转主次）：Ref::from 零拷贝（tool_dispatch.rs:58-66），
    /// BorrowedDispatchFuture 与 DispatchFuture 是**同型**类型别名
    /// （两者均为 Pin<Box<dyn Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>>，
    /// dispatch.rs:21-22 ≡ tool_dispatch.rs:116-117【实测·本轮两处亲读】），async 块 Box::pin 后直接相容。
    ///
    /// WHY 先构造 Ref 局部再 move 进 async 块：`&DispatchRequestRef::from(req)` 对函数内
    /// 临时值取引用满足不了 `&'a DispatchRequestRef<'a>`（临时值语句末消亡，返回的 future
    /// 捕获该引用即借用期错误 E0597）。把拥有所有权的 Ref 绑定到局部变量后 move 进
    /// async 块、在块内取引用——引用存活期 = Ref 在块内的存活期 = future 存活期 ≤ 'a，
    /// 生命周期自洽。这与 chain.rs:253-261 既有注释「owned 在 async 块内部创建……
    /// 生命周期自洽」是同一纪律的镜像（那边构造 owned，这边构造 Ref）。
    fn dispatch<'a>(&'a self, req: &'a DispatchRequest) -> DispatchFuture<'a> {
        let r = DispatchRequestRef::from(req);   // 拥有所有权的零拷贝视图（内含 &'a str）
        Box::pin(async move { self.head.dispatch_borrowed(&r).await })
    }
}
```

**⑤ 叶子（`inproc.rs`）**：`InProcDispatcher` 增 `BorrowedToolDispatcher` 原生实现——`ToolExecutor::execute(tool_name: &str, args_json: &str)`（toolplan_runner.rs:43）本就收 `&str`，现 inproc.rs:33-34 的「先取借用再 move 进 future」写法原样平移，**零分配直达叶子**；其 owned 实现改为同款零分配适配【深审实测】。

**⑥ builder/装配面**：`DispatcherBuilder` 五字段与 `with_*` 签名 `Arc<dyn ToolDispatcher>` → `Arc<dyn BorrowedToolDispatcher>`；外部使用面全在 crate 内（W3.md grep 证据：`benches/tool_runtime_chain.rs`、`benches/tool_dispatch_layers.rs`、`src/tool_runtime/{chain,runtime,mod}.rs`、`src/lib.rs`、`tests/tool_runtime_chain_proptest.rs`）——2 bench + 1 proptest 同批更新（夹具改持借用脊柱，或经 `OwnedDispatcherCompat` 包裹保持夹具写法不变）。`runtime.rs:77/:166` 两处 `inproc` 与 `:79-90/:169-180` 喂参同步换型【深审实测】。

**⑦ seccore 借用评估（W4-5/W4-6）**【深审实测 W3.md §4.3-⑦】：

```rust
// execpolicy.rs（加性；参照 policy.rs:329-331 的 S: AsRef<str> 泛型化先例）
impl RulePattern {
    /// 泛型化匹配核心 — 拥有型 &[String] 与借用型 &[&str] 共用同一份实现，
    /// 匹配形状分叉即判决分叉（安全语义）
    pub fn matches<S: AsRef<str>>(&self, program: &str, args: &[S]) -> bool { /* 现体不变 :133-142 */ }
}
impl ExecPolicy {
    /// 借用评估入口 — 与 evaluate 共用 evaluate_uncounted 泛型化体；计数语义一致（record_decision）
    pub fn evaluate_ref(&self, program: &str, args: &[&str]) -> PolicyAction {
        let action = self.evaluate_uncounted_ref(program, args);
        self.record_decision(action);
        action
    }
}
```

同步改造点：`evaluate_uncounted`(:224-231) 泛型化或新增 `_ref` 变体（两入口共用同一迭代体，**禁双源**）；`exec_tool_executor.rs:68` 的 `&[args_json.to_string()]` 改 `&[args_json]`（消 1 次分配/派发，加性；**两路径共用**——该行在 `ExecPolicyToolExecutor::execute` 内，owned 与借用派发最终都到达它，见 §3.4 表与 §3.6 护栏的联动裁决）。**等价锁（〔修订·审读〕语义明确化）**：本项是**新施工项而非现状描述**——既有 execpolicy 单测（:458-490）本体**保持 `&[String]` 形态不动**（A5 零删除/零修改），双形态一致性由 **W4-T2 的 proptest** 承担（`evaluate` 与 `evaluate_ref` 双调逐位一致 + `RulePattern::matches` 泛型化后双形态一致）；不新增独立等价锁施工项，避免与 W4-T2 分工双源。

### 3.4 W4 静态收益上限（E-4 分配清单 + 地板论证；〔修订·审读〕四列口径自洽化）

> 静态代码走查（W3.md §6 逐行实读，未运行 benchmark——深审纪律禁 cargo bench）；量化以实施者 bench 实测为准。
> 〔修订·审读〕原三列表的「W4 后拥有路径」列按行加总 = 7 而合计行写 ≈8、结论 3 又写「8→8 不变」——三处互斥。根因：`:68` 的 `args_json.to_string()` 切换在 `ExecPolicyToolExecutor::execute` 内，**owned 与借用两条派发路径最终都到达它**（链头 owned 入口与借用入口在 SandboxedDispatcher 汇合后走同一条链、同一个叶子 executor），所以它是**两路径共用**的改进点。下表改为四列（现状/W4 后 × 借用/拥有），逐行与合计均可复算：

| 分配点 | 位置 | 现状·借用 | 现状·拥有 | W4 后·借用 | W4 后·拥有 |
| --- | --- | --- | --- | --- | --- |
| 入站 owned 升级 | chain.rs:257 `to_owned_request` → tool_dispatch.rs:74-79 | **2** | 0（本来就直调） | **0**（反转消除） | 0 |
| 策略评估·规则匹配 `program.to_lowercase()` | execpolicy.rs:134（每规则 1 次；PolicyLayer 与 ExecPolicyToolExecutor 双重评估） | 2k | 2k | 2k（泛型化不消） | 2k |
| 策略评估·入参升级 `args_json.to_string()` | exec_tool_executor.rs:68【实测·本轮】（**两路径共用**，见上） | 1 | 1 | 0（`evaluate_ref` + `&[args_json]`） | 0 |
| 策略评估·命中规则 `args.join(" ")` | execpolicy.rs:137 | 1 | 1 | 1 | 1 |
| 沙箱·完整命令串 | policy.rs:259 → `build_full_command_string` :332-342 | 1 | 1 | 1（**地板**） | 1（**地板**） |
| 沙箱·白名单比较 `to_lowercase` | policy.rs:227 | 1 | 1 | 1（**地板**） | 1（**地板**） |
| 沙箱·非 ASCII 条件 | policy.rs:201-202 | 0~m | 0~m | 0~m | 0~m |
| future 装箱 | inproc.rs:35（`Box::pin`，全链仅叶子 1 次） | 1 | 1 | 1 | 1 |
| 派生策略缓存未命中 | sandbox_gate.rs:121-128（缓存 ≤256） | 摊销 0 | 摊销 0 | 摊销 0 | 摊销 0 |
| 执行载荷 `content_json` | executor 返回 String | 1 | 1 | 1 | 1 |
| **合计（k=1，ASCII）** | | **10** | **8** | **7** | **7** |

结论【W3.md §6.2 + 〔修订·审读〕改写】：
1. 可消除面：借用路径 10→7（约 30% 分配数下降；每次省 2 个小 String，bench 域内 ≈10B/≤120B）；拥有路径 8→7（仅来自 :68 共用切换）。
2. **不可消除地板（W4 范围外）**：seccore 判决体 2 次/派发与策略规则 `to_lowercase` 2k——消除需改 seccore 判决体，属**另立 ADR 的 seccore 域优化**，不得混入 W4（否则污染 256-case 等价归因）。列为 ADR-249 后续候选。
3. **owned 臂预期读数（〔修订·审读〕消除三处互斥的唯一裁决）**：反转本身使 owned 入口**零新增分配**（§3.3④ 的 Ref 适配不分配），但 :68 共用切换使拥有路径 **8→7（-1 次分配/派发，约 -12.5%）**——因此 bench 的 owned 臂**预期为轻微改善方向**（-1 次小 String 分配，耗时影响远 <5%，大概率淹没在噪声里）。护栏判据相应改为：**owned_c8 相对改造前基线出现「恶化 >5%」或「与预期方向不符的大幅变化（>5% 改善，意味着混入了未登记的改动）」才触发回归排查**；owned 臂不再作为「严格零变化」护栏使用。登记义务：ADR-249 正文必须写明「owned 臂预期 -1 分配（:68 共用切换），非零变化」——否则后续读者会重蹈本条三处互斥的覆辙。
4. 预估区间（静态推断非实测）：热路径中 2 个小 String 分配约占 5-15%（Windows GNU malloc ≈20-50ns/次 vs 全链 ≈1-3µs）——**≥5% median 改善是可争但非必然**，必须以 bench 实测裁决；不达标按 ADR-241 D5 先例否决落档（D6「净收益 0」诚实记录为直接先例，adr_index.md:252【深审实测】）。

### 3.5 TDD 矩阵

| # | 层 | 测试 | 断言核心 | 先红方式 |
| --- | --- | --- | --- | --- |
| W4-T1 | proptest（既有扩容） | `tool_runtime_borrowed_equivalence.rs:58` `with_cases(64)`→`256` | `borrowed_dispatch_matches_owned_dispatch` + `borrowed_sandbox_verdict_matches_owned` 逐位等价（含错误变体与文案） | 改造分叉 ⇒ 红 |
| W4-T2 | proptest 新增 | `policy_layer_borrowed_matches_owned` | 同一请求 `evaluate`（&[String]）与 `evaluate_ref`（&[&str]）返回 `PolicyAction` 逐位一致；`RulePattern::matches` 双形态一致 | W4-5 未实现 ⇒ 编译红 |
| W4-T3 | 单元（chain） | `chain_short_circuit_order_borrowed`（改造既有 chain.rs:357-532 五测） | policy Deny → 沙箱/传输 0 次调用；sandbox Deny → 传输 0 次；route miss → 传输 0 次——**借用链**上计数 | 反转后顺序破坏 ⇒ 红 |
| W4-T4 | 单元（sandbox_gate） | `check_ref_used_by_borrowed_chain` | SandboxLayer 借用路径触达 `gate.check_ref`（生产调用数 0 → 每次借用派发 1） | 接线前计数 0 ⇒ 红 |
| W4-T5 | 单元（inproc） | `inproc_borrowed_zero_upgrade` | 借用路径 `DispatchRequestRef` 直达 execute（内容断言），执行恰好 1 次 | 借用实现缺 ⇒ 编译红 |
| W4-T6 | 单元（compat） | `owned_compat_upgrades_explicitly` | `OwnedDispatcherCompat` 包裹 CountingTransport：借用路径结果与 owned 直调逐位一致 | 类型不存在 ⇒ 编译红 |
| W4-T7 | 单元（exec_tool_executor） | `capability_forwarding`（N-6） | 包装 `declared_tools()==["text.echo","fs.read","fs.list"]`、`capability_of("text.echo")==Some(ReadOnly)`、`capability_of("other")==None`；单独作 PlanRunner executor 时并发判据生效 | 默认 None ⇒ 红 |
| W4-T8 | 单元（runtime） | `runtime_routes_from_wrapped_delegate`（N-6 行为增量锁） | `ToolRuntime::new(Arc::new(ExecPolicyToolExecutor::new(policy, Arc::new(BuiltinToolExecutor))), policy)`（**双参签名**，runtime.rs:69【实测】）的路由表现含三个内置工具名 | 未转发 ⇒ 路由表空 ⇒ 红 |
| W4-T9 | 回归面 | 既有 `tool_runtime_chain_proptest.rs` + `tool_runtime_batch_production_invariants` 全绿 | 改造零回归 | — |

### 3.6 同轮对照基准（唯一允许 L4 判性能的波次）

**bench 结构**（新增 `crates/gqep-executor/benches/dispatch_borrowed_chain.rs`）【深审实测 W3.md §7.1】：
- 组名 `dispatch_borrowed_chain`；`group.sample_size(30)`（对齐既有 `benches/tool_runtime_chain.rs:61`）。
- 6 测点 = `{owned, borrowed} × {c1, c4, c8}`：同一条生产装配链（`ToolRuntime::new(Arc::new(EchoExecutor), Arc::new(allow_policy))` 形态——签名 = `pub fn new(delegate: Arc<dyn ToolExecutor>, policy: Arc<ExecPolicy>) -> Result<Self, DispatchError>`（runtime.rs:69【实测·本轮亲读】）；〔修订·审读〕统一为双参形态，本节与 W4-T8 同口径）、同输入内容（固定短名 + 定长 args_json ≈120B，落在等价测试 `.{0,120}` 域内）；`owned_*` 臂 N 次 `runtime.dispatch(&req)`、`borrowed_*` 臂 N 次 `runtime.dispatch_borrowed(&ref)`（ref 借自循环外拥有缓冲，构造在计时区外）；c∈{1,4,8} 用 FuturesUnordered 有界并发 harness（**入参形态为唯一变量**）；c1 = 串行基线。
- **同轮采集**：owned 与 borrowed 交替插入同一 `BenchmarkGroup` 逐测点运行（同机同进程同轮），禁跨轮对比；首轮冷启动丢弃（29-33% 噪声下限）。
- 护栏测点（〔修订·审读〕与 §3.4 结论 3 联动）：**owned_c8 相对改造前基线「恶化 >5%」或「预期外大幅变化（>5% 改善）」视为回归排查**——owned 臂预期读数 = 轻微改善方向（:68 共用切换 -1 分配，§3.4 结论 3），**不是严格零变化**；bench 改造前基线 = W4 改码前先用 §3.6 同款测点跑一轮存量读数（或引用既有 `tool_dispatch_layers`/`tool_runtime_chain` 组登记值），禁凭空对照。

**采纳判据（ADR-249 硬门，逐字对齐全景方案 §5.5-4/§6.1）**：
- 操作定义：对每对 `(owned_cX, borrowed_cX)` 取 criterion median 95% CI；采纳条件 = `borrowed_median ≤ 0.95 × owned_median` **且** 两组 median CI 不相交；c1/c4/c8 三对**全部**满足才采纳（任一对不满足 → 按否决落档，恢复 `chain.rs:246-261` 原 owned 升级实现，ADR-241 D5 单点回退）。
- **否决亦为正式结论**：ADR-249 记录实测数与否决理由（先例：D5 DashMap 否决、D6 净收益 0）。

**阈值双源登记（W4-9）**【勘误 7 修正后的 I2 口径，〔已修正〕】：
1. 本机先实测 6 测点 median → `perf_thresholds.toml` 增 `bchain_owned_c1/c4/c8`、`bchain_borrowed_c1/c4/c8` 六键（先实测后登记）。
2. **新键的 criterion 叶子路径词条须与既有 101 词条两两双向不为子串（I2 口径，`check_perf_redlines.py:595` `keyword_overlaps`）**——不是「键名对 83 键」的子串检查【〔已修正〕：原 W3 表述「新增键名须与 83 键无子串关系」口径失准——I2 实现检查的是 path_keywords **词条集**（`check_perf_redlines.py:317` `overlaps = keyword_overlaps(kw)`，`kw` 为 path_keywords dict；`:595-605` 对两键的**词条集**做双向真子串判定），词条全集现值 **101**（【实测·本轮】`tomllib` 统计 = 101，非 83）。W3/W5 全部候选键（`bchain_owned_{c1,c4,c8}`/`bchain_borrowed_{c1,c4,c8}`/`actx_serial_zero`/`actx_parallel_zero`/`xloop_assemble`/`action_orch_bounded`/`hnsw_tiered` 词条）已按词条集与键名集双口径机器检查，均 NONE 冲突——结论不受影响，仅口径订正】。登记前必跑 `python scripts/check_perf_redlines.py invariants` 取余量读数（基线：I1 83/83、I3/I7 55、I9 83【实测·本轮】）。
3. `[path_keywords]`/`[[run_bench]]` 映射 + `bench_check.yml` heredoc 同批双写（I7/I9 对拍）。
4. 映射 bench = `dispatch_borrowed_chain`（gqep-executor 第 6 个 bench；`inventory` 门 unknown 必须保持 0）。

### 3.7 N-6（加性转发，随 W4；3 行级）

```rust
// exec_tool_executor.rs — impl ToolExecutor for ExecPolicyToolExecutor 增两方法：
fn capability_of(&self, tool_name: &str) -> Option<SideEffectDecl> {
    self.delegate.capability_of(tool_name)      // 透传被包装的原始执行器
}
fn declared_tools(&self) -> Vec<String> {
    self.delegate.declared_tools()
}
```

背景【W3.md §4.4】：`ToolRuntime` 能力视图已在构造点就地保留原 delegate（runtime.rs:39-46/:74/:161），路由表不受影响；N-6 实际缺口是 `ExecPolicyToolExecutor` **单独**作为 PlanRunner executor 时能力表静默失效（fail-closed 串行）。转发后 `ToolRuntime::new` 内其 `declared_tools()` 从空集变透传——**行为增量**：路由表从「全 fallback」变为「按 delegate 声明注册」（runtime.rs:80-84/:170-174），与 `with_optional_delegate` 路径今日行为拉齐，属修复而非语义漂移；须 ADR-249 登记 + 测试锁定（W4-T7/T8）。落 W4 还是移 P1 = §13 开放问题 OQ-04（W3.md Q4）。

**〔修订·审读〕与 P1「路由表 3 条」论断的依赖关系澄清（回应 missing-8）**：实测装配代码证明 **P1 的「路由表 3 条」不依赖 N-6**——`with_optional_delegate` 路径中 `capabilities = delegate.clone()`（runtime.rs:161【实测·本轮亲读】）取的是**未包装的 CompositeToolExecutor**，其 `declared_tools()` 转发 builtin 是 **P1 自身规格内代码**（§6.3 CompositeToolExecutor 实现）；`ExecPolicyToolExecutor` 包装只发生在链内传输环（runtime.rs:163 `Some(exec) => Arc::new(ExecPolicyToolExecutor::new(policy, exec))`），**不进入 capabilities**（ADR-254 D3「能力视图包装前就地保留」）。N-6 影响的是「把 ExecPolicyToolExecutor 包装件**直接当 delegate 传入** ToolRuntime::new」的场景（此时 capabilities = 包装件，其默认空集使路由表空，W4-T8 锁的就是这个场景）。结论：OQ-04 无论裁给 W4 还是 P1，都不阻塞 P1 的路由表论断；但 OQ-04 若裁给 P1，P1 需在 §6 规格追加 N-6 施工行（3 行级 + W4-T7/T8 测试随迁）。

### 3.8 回滚

| 层 | 手段 |
| --- | --- |
| L1 | 无运行时开关（安全链，ADR-242 §3.5 不设开关）——回退走 L2 |
| L2 | 按 ADR-241 D5 单点回退：`chain.rs` 四层恢复 `Arc<dyn ToolDispatcher>` + `dispatch_borrowed` 恢复 owned 升级（:250-259 原体）；`OwnedDispatcherCompat`/`evaluate_ref` 保留无害（加性）；每撤销一步跑 §10 门禁集 |
| 判据触发 | §3.6 判据不满足 → 主动否决 revert（否决亦为正式结论） |

不可回滚项：四环顺序、fail-closed、缺环构造失败、seccore 6 条不可学习红线（`crates/seccore/src/policy.rs:420-427`【深审实测】）——W4 不触碰判决体语义，256-case 等价测试为守门证据。

### 3.9 验收命令与 ADR-249 决策点

```bash
cargo check -p gqep-executor -p seccore --all-targets
cargo test -p gqep-executor --test tool_runtime_borrowed_equivalence   # 256 cases
cargo test -p gqep-executor --test tool_runtime_chain_proptest
cargo bench -p gqep-executor --bench dispatch_borrowed_chain           # 同轮对照（先备份 criterion！ADR-252 D4）
python scripts/check_perf_redlines.py invariants                       # 登记前后键数对拍
python scripts/check_perf_redlines.py inventory                        # unknown=0
```

ADR-249 决策点【W3.md §9.2】：
1. **反转主次非另建链**：脊柱复用 `BorrowedToolDispatcher`；`ToolDispatcherRef` 作为概念名落档。
2. **blanket → 显式适配器**：coherence 修正，`OwnedDispatcherCompat` 为唯一升级缝。
3. **check_ref 生产接线**：SandboxLayer 借用路径调 `check_ref`（W4-T4 锁定）。
4. **seccore 加性**：`evaluate_ref` + `matches` 泛型化；判决体 policy.rs 不动。
5. **判据**：median ≥5% 且 CI 不重叠；否决亦为正式结论。
6. **失效条件**：判决分叉/短路顺序变 ⇒ 256-case 红；owned 臂漂移 >5% ⇒ 回归排查。

---

## 4. W5 与 W5.0 规格 —— 并行化深化六点 + 阈值双源治理（ADR-251）

### 4.1 目标

1. **W5.0（必闭，CI 已授权）**：消灭 N-3 幽灵门残留——heredoc `PATH_KEYWORDS` 补 `hnsw_tiered` 词条，使该 2ms 阈值（ADR-192）在 CI legacy 断言路径真实生效；落新不变式 **I10**（heredoc PATH_KEYWORDS 键集 vs TOML path_keywords 键集对拍）防复发——没有 I10 本类漂移必然复发（I9 只对拍 THRESHOLDS_NS）。
2. **5.1~5.6 逐点定性**：关键纠偏——**5.1 与 5.5 的上游表述与现势代码结构不符**（5.1 现势已是 per-event 无界并发；5.5 的 Auto/Off 边界已全部实现），须按现势重定改造点，不得照方案字面施工。
3. 全部新基准键遵守 I2 棘轮（`max_keyword_overlaps = 1` 顶格，`perf_thresholds.toml:336`）——任何新键与既有键产生新**词条**子串重叠即破门；新键先机器验 I2 再落 TOML+heredoc 双写。

### 4.2 五元组

| # | 模块 | 文件:行号 | 影响 | 阻塞 | 前置 |
| --- | --- | --- | --- | --- | --- |
| W5.0 | 阈值双源 | `.github/workflows/bench_check.yml:412-536`（PATH_KEYWORDS dict，末条 runx_* :532-535，dict 收口 :536）；`scripts/perf_thresholds.toml:189/:323/:634-637`（hnsw_tiered 三处已在）；`scripts/check_perf_redlines.py:529`（CI_THRESHOLDS_DICT_RE）、`:347-353`（I9 断言）【深审实测】 | hnsw_tiered 2ms 阈值从「CI 恒 skip」变「CI 真实断言」 | 阻塞 W5 全部新增键的双写流程（I10 先落） | 无（TOML 侧零改动） |
| 5.1 | chimera-cli action 编排 | `crates/chimera-cli/src/action_orchestrator.rs:356-398`（spawn_action_orchestrator，**:375 per-event `tokio::spawn` 无界**）【实测·本轮 sed :373-378 spawn 行】、`:136-319`（route_action 单 match 臂） | TUI 长动作并发上限受控化 | 无 | 纠偏确认（OQ-07） |
| 5.2 | chimera-cli 经验闭环装配 | `crates/chimera-cli/src/experience_loop.rs:123-317`（**12 处 join_handles.push = :153/:172/:196/:222/:234(条件)/:244/:264/:274/:286(条件)/:296/:306 + attach_observability 内 :386(条件)**【〔修订·审读〕实测·本轮 grep 精确清单；原稿列 11 个行号且把 :222 误注为 attach_observability——:222 实为 quest_engine::spawn_metrics_subscriber，attach_observability 的条件 push 在 :386】）；生产调用方 `commands/tui.rs:279-291`【深审实测】 | TUI 装配期 SQLite 打开与订阅器群启动时延 | 无 | 依赖图分区（§4.5） |
| 5.3 | chimera-cli runtime 构建 | `crates/chimera-cli/src/main.rs:218-229`（build_runtime；**:225-227 multi_thread 分支未设 worker_threads**）、`:158`（调用点）、`:238-240`（use_current_thread，CHIMERA_RT_FLAVOR=auto 现存）【实测·本轮 sed :218-244 全段】 | one-shot 启动税 | 无 | tokio 默认行为确认 |
| 5.4 | event-bus 批量发布 | `crates/event-bus/src/bus.rs:1165-1168`（publish_batch）、`:1182-1239`（dispatch_batch）、`:62`/`:69`（常量）【实测·本轮 sed :62-69】；唯一生产调用方 `crates/parliament/src/debate/mod.rs:1305`【深审实测】 | 「同 tick 合批」需新聚合调用点 | 无 | 调用点画像（OQ-10） |
| 5.5 | chimera-cli 分片策略边界 | `crates/chimera-cli/src/spec.rs`（枚举 :33-39；plan 表实测行号见 §4.10）【实测·本轮】；`composition.rs:232-237`/`:245-254`；接线 `commands/mod.rs:87-89`【深审实测】 | **已全部实现**——核对确认点，非改造点 | 无 | 无 |
| 5.6 | 同步卸载冻结 + 跨 await 豁免 | `scripts/sync_offload_guard_freeze.txt:19-31`（**13 条实体**=5 GUARDED+8 NO_ASYNC_BOUNDARY，方案记 12 条为漂移）；`scripts/await_guard_allowlist.txt:35-39,54-57`（9 条实体顶格）；门 `gate_manifest.toml:294-296`（G-27）、`:982-984`（G-88）【深审实测 + 实测·本轮 :19-31 计数=13、:55 行亲读】 | G-27/G-88 基线状态修复 | **G-27/G-35 当前 mismatch**（stale 条目 `participant_client.rs:556`【实测·本轮 :55 亲读】；`git diff c6eb9b6..HEAD -- crates/mcp-mesh/` = 0 行 ⇒ 基线时点已红【深审实测 facts.md G-8】） | stale 条目 re-key（OQ-08） |

### 4.3 W5.0 接口与双写步骤（必闭，TOML 侧零改动）

**现势与根因**（W5.md §3.0，全部深审会话机器实证；I2 词条全集口径见 §3.6〔已修正〕段，词条全集现值 101【实测·本轮】）：
1. 差集唯一且精确：heredoc `PATH_KEYWORDS` 82 键 vs TOML `path_keywords` 83 键，差集 `{hnsw_tiered}`。
2. 阈值恒空转机制：`bench_check.yml:546` `bench_results = {name: [] for name in THRESHOLDS_NS}`（hnsw_tiered 在 THRESHOLDS_NS 有键→空列表）；`:561-565` 归类循环只遍历 `PATH_KEYWORDS.items()`——hnsw_tiered 无词条→其 criterion 路径 `l2_knn_tiered/...` 落 `unmatched_files`（:566-567）；`:574-579` 空结果只打 `::warning::` 后 `continue`（skip 非阻塞）；unmatched 仅诊断打印（:593-596）不报错。
3. 为何三道自证全盲：I9 提取正则 `CI_THRESHOLDS_DICT_RE`（check_perf_redlines.py:529）只抓 THRESHOLDS_NS；compare 门只遍历 legacy.items() 且 :724-727 注释自陈「py 侧多出的键从未进入比对」；I1 只在 TOML 内部对拍。
4. heredoc 结构适刻性：PATH_KEYWORDS 值均为字符串数组，提取法原样适用——I10 改动量 ≈15 行。

**双写步骤**：

- 步骤 1：`bench_check.yml` PATH_KEYWORDS dict 内 `"runx_parallel_1ms"` 行（:535）之后、dict 收口 `}`（:536）之前插入：

```yaml
              # ADR-192：与 scripts/perf_thresholds.toml [path_keywords].hnsw_tiered 双写同步
              # （N-3 幽灵门残留收口，W5.0）。词条与其余 82 键无双向子串关系（I2 已机器核），
              # 插入位置不影响任何 first-match-wins 归类（hit 即 break，仅本词条命中的路径唯一）。
              "hnsw_tiered": ["l2_knn_tiered/top10_recall/10000_tiered"],
```

- 步骤 2：`scripts/check_perf_redlines.py` 落 I10（复用 I9 形态）：`CI_PATHKEYWORDS_DICT_RE = re.compile(r"PATH_KEYWORDS\s*=\s*\{(.*?)\n\s*\}", re.S)`；比对函数与 `ci_threshold_key_sync`（:555-584）逐行同构，双向差集 + `ci_pathkeywords_dict_present` 防伪绿（同 :587-592 形态）。
- 步骤 3：`mode_invariants` 中 I9 断言（:347-353）之后加 I10 断言（措辞 `I10 bench_check.yml PATH_KEYWORDS keys == [[path_keywords]] keys (N keys; dict found=...)`）。
- 步骤 4：I2 机器验证先行（〔修订·审读〕脚本口径钉死：**`cand` 填候选键的 criterion 叶子路径词条，逐词条跑——不填键名**。键名与词条是不同字符串（W5.0 案例键名 = `hnsw_tiered`、词条 = `l2_knn_tiered/top10_recall/10000_tiered`），I2 权威口径是「词条对词条」（§11.2-1，`check_perf_redlines.py:595` 词条集双向子串）；深审所称「hnsw_tiered 词条零冲突已验证」指对其**词条** `l2_knn_tiered/top10_recall/10000_tiered` 的检查，实施时按下述脚本对全部候选词条复跑存证）：

```bash
python -I -c "
import tomllib
doc = tomllib.load(open('scripts/perf_thresholds.toml','rb'))
# cand = 新键的全部词条（非键名！）。W5.0 案例：cands = ['l2_knn_tiered/top10_recall/10000_tiered']
cands = ['<候选词条1>', '<候选词条2>']
clash = []
for cand in cands:
    for kws in doc['path_keywords'].values():
        for k in kws:
            if k != cand and (k in cand or cand in k):
                clash.append((cand, k))
print('I2 clashes:', clash if clash else 'NONE'); exit(1 if clash else 0)
"
```

- 步骤 5：**emit-slo-filter 校正：无需重生成**——`emit-slo-filter`（check_perf_redlines.py:1541-1571）消费 `ignored_test_inventory_freeze.txt` 的 `slo-blocking` 条目，与 PATH_KEYWORDS 零共享状态；`slo.yml:107` expected 对拍幂等。全景方案 W5.0 的「emit-slo-filter 重生成」为冗余步骤，按只读实证驳回（若同批改 ignored 冻结表才需重跑）。hnsw 相关冻结条目实况：mlc-engine 两条 `slo-daily`（:72-73）、repo-wiki 三条 `slo-blocking`（:91-92,94）——均不因本步变动【深审实测 W5.md §3.0】。

**I2 顶格纪律（全 W5 适用）**：需避开的既有宽词雷区（`perf_thresholds.toml:204-323` 实录）：`reduce`、`mixed`、`gather`、`entangle_alone`、`cache_hit`、`degradation`、`deliberate`、`cross_layer`、`hybrid_search`、`100k_to_32k`、`compute_all_masks`、`checkpoint_save`、`shadow_stats`、`dedup_semantic`、`single_write`、`15_sections`、`incremental_5pct`、`apply_decision`、`handle_quest_created`、`probe_window_select/`、`hit_vs_recompute`、`produce_verify`、`xts_top_k`、`percentile_sorted`、`immune_probe`、`hot_lru`、`memory_graph_recall_2k_depth2`、`sequential_compute_5_masks` 等全集（以 TOML 现值为准逐键核）。

**双写纪律（全波次）**：任何 W5 新基准键落表 = TOML `[thresholds_ns]`+`[path_keywords]`+`[[run_bench]]` 三处 + heredoc `THRESHOLDS_NS`+`PATH_KEYWORDS` 两处**五处同 commit 双写**，且 I10 必须先于第一个新键落地。

### 4.4 5.1 action 编排有界并发（候选，纠偏后裁决）

**现势纠偏（与上游方案三处不符，W5.md §3.1 实证）**：
1. 「全分支串行 await」不成立：`route_action`（:136-319）是单 `match action_id` 的**互斥臂**（九组臂），单请求只命中一臂；臂内步骤均为顺序依赖，**臂内无可并行分支**。
2. 事件间已并发：`spawn_action_orchestrator` 对每个 `TuiActionRequested` 在 :375 `tokio::spawn` 独立任务——**是无界并发而非串行**【实测·本轮 spawn 行】；`handle_action_event` 直接 await 形态（:85-126）仅测试消费。
3. 「四环短路顺序不可变」与本点正交：四环锚在 `chain.rs`（gqep-executor），`route_action` 路由到 QuestEngine **不经四环**；上游把两者并列属概念混淆。dchain_* 六键与 action 编排无关——上游给 5.1 的对照键 `dchain_*` 属**误植**。

**纠偏后的真实改造点**：:375 per-event spawn **无界**——用户高频派发可堆积无上限任务。改造 = 有界并发：

```rust
// action_orchestrator.rs（示意，非终稿）
/// 有界并发上限：与 gqep-executor BatchLimits::default 的 bound=8 同源口径
const ACTION_CONCURRENCY_DEFAULT: usize = 8;

pub fn spawn_action_orchestrator(
    bus: EventBus,
    engine: Arc<QuestEngine>,
    overwindow: Option<OverWindowHandle>,
    chat: Option<ChatMessagesProvider>,
    curation: CurationConfig,
) -> JoinHandle<()> {
    let mut rx = bus.subscribe(); // subscribe-before-spawn 红线（既有）
    let sem = Arc::new(tokio::sync::Semaphore::new(resolve_action_concurrency()));
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) if matches!(event, NexusEvent::TuiActionRequested { .. }) => {
                    // 有界：许可耗尽时 await 排队（背压回压到 recv），不丢弃、不无界堆积。
                    let permit = sem.clone().acquire_owned().await.expect("semaphore 未被关闭");
                    // ... 原克隆 + tokio::spawn 原样，任务尾 drop(permit)
                }
                // ... 错误分支原样
            }
        }
    })
}

/// ADR-242 式回退：`CHIMERA_ACTION_CONCURRENCY=unlimited` 恢复无界 per-event spawn
/// （现行为）；数字 N = 有界 N；未设 = 8。
fn resolve_action_concurrency() -> usize { /* env 解析，纯函数可测 */ }
```

**`resolve_action_concurrency` 表驱动期望列（〔修订·审读〕补 missing-5，与 §4.8 测试一一对应）**：

| 输入（`CHIMERA_ACTION_CONCURRENCY`） | 期望输出 | 附加行为 |
| --- | --- | --- |
| 未设（None） | `8`（`ACTION_CONCURRENCY_DEFAULT`） | 无 |
| `"unlimited"` | `usize::MAX`（无界 ≈ 现 per-event spawn 语义） | 无 |
| `"0"` | `8`（0 无意义——0 许可 = 永久阻塞；回退默认而非拒绝） | `tracing::warn!` |
| 负数（如 `"-1"`） | `8`（usize 解析必败） | `tracing::warn!` |
| 非数字（如 `"abc"`） | `8` | `tracing::warn!` |
| 合法正整数 N | `N` | 无 |

**丢事件风险论证（〔修订·审读〕补 missing-4，采纳裁决前必读）**：上文形态在 recv 循环内 `acquire_owned().await`——许可耗尽时 recv 循环**停止消费**，新事件积压在 broadcast 通道；`tokio::broadcast` 满载语义是**丢弃最旧消息**（订阅者收到 `RecvError::Lagged` 跳号），即有界化会把「任务无界堆积」转化为「通道 Lagged 丢事件」——事件总线默认容量 `DEFAULT_CAPACITY = 1024`（`bus.rs:33`【实测·本轮】，`EventBus::new()` → `with_capacity(DEFAULT_CAPACITY)` :619-625）。风险评估：① TUI 单用户事件率远低于 1024 容量，现实触发概率低；② 但 8 个 120s 长动作并发占满许可后，第 9 个动作起 recv 停转最长达 120s——高频连击可触达 Lagged；③ Lagged 丢失的可能是**任意类型**事件（含非 TuiAction），影响面大于本点。**缓解设计（推荐，随实现落地）**：permit 获取改 `try_acquire` 优先——失败时**在 recv 循环内同步执行**该动作（背压降级为串行处理而非停止 recv：不丢事件、不无界堆积、长动作期间新事件留在容量缓冲内）；`acquire_owned().await` 仅作 try_acquire 后的兜底等待（理论不可达路径）。**proptest 联动（§4.8）**：在缓解形态下「每 request_id 回执恰一次」才可承诺；若主会话不接受缓解形态的复杂度，则 5.1 记录不采纳（回归无界 spawn 现状——现状同样存在 Lagged 风险但有界化不放大它），此裁决进 ADR-251 D-W5-A。

- 同 Quest 控制动作乱序风险：单用户命令面板语义下可接受（回执按 request_id 归属）；proptest 锁「同一 request_id 回执恰一次」（§4.8）。
- **定性：候选**。收益画像低（TUI 单用户事件率低），但**无界堆积的防护价值独立于性能收益**，需 ADR 裁决是否按健壮性立项（OQ-07）。

### 4.5 5.2 经验闭环装配并行化（候选，收益面已重定位）

**现势依赖图**（W5.md §3.2 逐行实读）：I/O 重活仅 :135-136 `ExperienceCardStorage::new`（SQLite 打开，失败降级内存【实测·本轮 sed :134-137】）；其余纯内存构造 µs 级。三条真实顺序约束：① card_bus 先于 :153/:162；② router 依赖 mlc（:168-171）、feedback loop 依赖 router（:172）；③ **subscribe-before-spawn 红线**（:152/:320-321/:409 注释三处自陈）——并行化单元是「整段装配函数」（内部各自遵守红线），**不把 subscribe→spawn 对拆进两个并行块**。

上游句「Critical 旁路订阅须在 AppContext 移出前完成」在现势找不到对应结构：experience_loop 不经 `composition::build` 的 AppContext（生产调用方 commands/tui.rs:279）；按现势重写为「所有 Critical mpsc 双通道订阅（含 attach_observability 内 EfficiencyMonitor::start_event_subscriber :368-377）必须先于对应 spawn 同步完成」。

**设计（〔修订·审读〕分区归属完整化，补 missing-14）**：三条顺序约束（§4.5 上文）的成员落位——约束①（card_bus → :153 持久化订阅 / :159-163 mlc+with_card_bus）与约束②（router :168-171 依赖 mlc、feedback :172 依赖 router）的全部成员留在 **join 之前的串行主体**；并行分区只有两个且互无数据依赖：

```rust
// 分区归属（join 前串行主体 = 约束①② 的 card_bus 链 + router/feedback 链，:131-:172）
let storage_fut = open_storage_or_fallback(&db_path);   // 分区 A：SQLite 打开（:134-150 抽出）
let bus_side_fut = async {
    // 分区 B：:183-306 中仅依赖 nexus_bus 的订阅器群（auditor/decb、
    // :264 faae、:274 throughput、:286 interception、:296 saturation、:306 attribution tap）
};
let (storage, ()) = tokio::join!(storage_fut, bus_side_fut);
// card_bus 侧链「保持串行」的精确含义 = 位于 join! 之前的顺序主体内，
// 相对于 join 的两个并行分支先执行完毕；其内部 :153/:162 依赖序不变。
```

- **定性：候选**。预计收益 = 一次 SQLite open（ms 级）的重叠，TUI 启动占比小。上游对照键 `cli_startup_e2e` **覆盖不了本点**（cli_parse bench 的该组 = parse+分类+runtime 构建，不含 experience_loop 装配）——**方案偏差登记**；需新键 `xloop_assemble`（候选名，`experience_loop_assemble/` 组）或记录不采纳（OQ-12）。

### 4.6 5.3 runtime worker_threads 显式化（候选，大概率不采纳）

**现势**：main.rs:225-227 `Builder::new_multi_thread().enable_all().build()`——未调 `.worker_threads()`，tokio 默认 = `available_parallelism`；`:220` 经 `use_current_thread` 判定走 current_thread；**`CHIMERA_RT_FLAVOR=auto` 已是现存回退机制，非新增**【实测·本轮 sed :218-244 全段】。

**〔已修正〕行号登记（勘误 9 + 本轮实测矛盾显式裁决）**：复核勘误称「`use_current_thread` 引用 :238-240 有误，fn 定义实际在 :240」；**本轮实测 `grep -n "fn use_current_thread" crates/chimera-cli/src/main.rs` = `:238`**（fn 定义 :238-240 含函数体两行，W5 原引 :238-240 恰好覆盖完整 fn 定义，**W5 原值正确、勘误 9 的 ":240" 不精确**）；`:225-227` multi_thread 分支未设 worker_threads 的断言勘误与实测**精确吻合**。终稿以本次实测为准：`fn use_current_thread` 定义于 `main.rs:238-240`。

**设计（若做）**：`.worker_threads(resolve_workers())`，`CHIMERA_RT_WORKERS` env：未设/auto = 现行为（零行为变更），数字 N = 显式 N。

**否决倾向论证**：显式 `worker_threads(available_parallelism)` 与默认行为逐位等价（同义反复）；唯一真实收益是 worker 数 cap 旋钮——属新配置面非并行化改造。`cli_startup_runtime` 现键（5ms，`perf_thresholds.toml:137`；bench 组 `cli_startup_runtime/build_multi_thread`）已守护构建成本。**定性：候选，默认记录不采纳**（OQ-09）。

### 4.7 5.4 事件发布同 tick 合批（候选，调用点先行）

**现势**：`publish_batch`（bus.rs:1165-1168）+ `dispatch_batch`（:1182-1239）**已存在且语义完整**：摊销采样、逐条保留 Critical mpsc 旁路与无订阅者告警、分片路由逐条判定（Critical/OrderSensitive 恒走单流）、信用流逐条判定。Critical 不合批清单即 `is_critical_mpsc_event`，规模锚 CRITICAL_MPSC_VARIANTS=18（bus.rs:62【实测·本轮】）、CRITICAL_TOTAL=22（:69）。唯一生产调用方 `parliament/src/debate/mod.rs:1305`；其余为 bench（bus_bench.rs:156）与测试。bus_bench 已有对照组 `publish_batch_vs_serial` 组（bus_bench.rs:135-168）【深审实测 W5.md §3.4】。

**设计（合批的是调用点不是 bus 内部）**：bus 内部无可再合（逐条 send 是固有开销）；「同 tick 非 Critical 合批」的真实工程是**在聚合发布者处**攒 Vec 走一次 publish_batch。候选调用点画像：a) 议会 debate 已接；b) TUI tick 循环三报告器族周期 60s——太长无合批意义；c) 若无真实高频同 tick 多事件源，**本点无生产收益=否决**（OQ-10）。新调用点若落地需新键（`bchain_*` 前缀族——全景方案 ADR-251 行预留 `bchain_` 前缀守 I2【深审实测】；落表前仍须跑 §4.3 I2 命令）。

### 4.8 TDD 矩阵

| 点 | 单元 | 集成 | proptest |
| --- | --- | --- | --- |
| W5.0 | check_perf_redlines.py selftest 增 I10 断言（键集不等→fatal；dict 缺失→不伪绿，复用 :1307-1314 先例） | `invariants` 全绿且 I10 行现形 | 无（确定性表） |
| 5.1 | `resolve_action_concurrency` 表驱动（未设=8/unlimited/0/负数/非数字逐值——**期望列见 §4.4 期望表**〔修订·审读〕）；permit 在任务体内释放 | TUI 装配冒烟：8 并发下 quest.start+agent.chat 混发，回执 request_id 全归属 | ①每 request_id 回执恰一次；②并发任务数峰值 ≤ bound；③unlimited 与现行为等价 |
| 5.2 | storage 失败降级路径行为不变 | experience_loop_closure_e2e 既有 5 调用面零回归；subscribe-before-spawn 既有测试锁定 | 装配顺序扰动下事件投递序不变量（卡片先持久化后反馈） |
| 5.3 | `resolve_workers` 表驱动 | cli_startup_runtime 不越 5ms | 无 |
| 5.4 | 聚合器：tick 内 N 条非 Critical → 1 次 publish_batch；混入 Critical → 拆出走单流旁路（18 变体逐臂锁定） | publish_contract_test 批路径契约扩展；bus_bench 零回归 | 任意事件流切分下不丢 + Critical 旁路送达率 100% |
| 5.5 | 无新测试（既有 spec.rs tests 锁定） | 无 | 无 |
| 5.6 | re-key 后 `check_await_across_guard.py --selftest` 绿 | `run_gate_manifest.py` G-27/G-35/G-88 三门 rc=0 | 无 |
| 通用 | — | `cargo test -p chimera-cli -p event-bus --no-fail-fast`；clippy；逐包 fmt | 每新增并发点必有 proptest（TDD 守恒） |

### 4.9 同轮对照基准

| 点 | 基准键 | 类型 | 设计 |
| --- | --- | --- | --- |
| 5.1 | 新键族 `actx_*`（候选：`actx_serial_zero`/`actx_parallel_zero`，criterion 组 `action_orch_bounded/`，词条取叶子全路径——ADR-252 D2 纪律防宽词互窃） | 新键 | 上游给「dchain_* 六键」**误植**（dchain 是四环链键）；action 面现无任何 bench【深审实测 grep】。落键前跑 §4.3 I2 命令 |
| 5.2 | 若采纳：新键 `xloop_assemble`（组 `experience_loop_assemble/`）；若不采纳：记录 | 新键或记录 | 上游 `cli_startup_e2e` 覆盖不了 experience_loop——**方案偏差登记** |
| 5.3 | `cli_startup_runtime`（现键，:137/:284） | 现有键 | 阈值 5ms 不动 |
| 5.4 | `bus_bench`（:165/:302/:568-571）+ `cross_layer`（:50/:227/:436-438） | 现有键 | bus 内部零改动 ⇒ 现键守护；若新增聚合调用点另立 `bchain_*` 新键 |
| 5.5 | `cli_startup_config`/`cli_startup_runtime`/`cli_startup_e2e` 三键 | 现有键 | 核对点零改动，键仅作回归哨兵 |
| 5.6 | G-27/G-88 门 rc（非 bench 键） | 门基线 | re-key 后三门绿即基线恢复 |
| W5.0 | I1/I7/I9(+新 I10) | 门 | `invariants` 全绿 + I9 83/83 不变 + I10 83/83 现形 |

### 4.10 5.5 ShardingPolicy 边界核对（已实现，定性=核对确认/代码零改动）

**〔已修正〕行号（勘误 8 + 本轮实测矛盾显式裁决）**：复核勘误称 spec.rs plan 表「Auto 臂(含注释)约 :67-74，Off 臂约 :75-83，NoContext 臂约 :85 起」；**本轮 `grep -n` 实测**：`sharding: ShardingPolicy::Auto` 在 **:75**、`ShardingPolicy::Off` 在 **:85**、`) => CommandPlan::NoContext` 在 **:98**——即 Auto 臂（含注释）= **:72-77**、Off 臂 = **:78-86**、NoContext 臂 = **:87-98**。勘误 8 的替代行号（:67-74 等）与本次实测偏差约 5 行；反而 W5 原值（:73-77/:78-86/:88-98）与实测偏差 ≤2 行。**终稿以本次 grep 实测为准**（Auto :72-77 / Off :78-86 / NoContext :87-98），勘误方向（行号需现取）正确、其给值不采纳。枚举 `:33-39` 勘误与实测**精确吻合**（`pub enum ShardingPolicy` :34 / Off :36 / Auto :38）。

| 上游方案断言 | 现势（本轮实测） | 判定 |
| --- | --- | --- |
| `spec.rs:27-50` Off/Auto 枚举 | `spec.rs:33-39` 枚举两变体（Off :36「one-shot：低事件量、秒级退出，64 worker 纯税」/ Auto :38「长驻宿主」） | ✓ 已实现 |
| 长驻 Auto | `spec.rs:72-77`：`None \| Chat \| Tui \| Serve \| Acp → Auto` | ✓ |
| one-shot Off | `spec.rs:78-86`：`Run/Exec/Quest/Parliament/Agent/Doctor → Off` | ✓ |
| `composition.rs:222-224` | 该行号现势是 doc 注释；真实实现 `composition.rs:232-237`（build 默认 Auto）+ `:245-254`（build_with_sharding；Off 在 `EventBus::new()` 后、leaf 构造前 `without_sharding()` :252-254）【深审实测】 | ✓ 方案行号已漂移，登记 |
| 接线 | `commands/mod.rs:87-89`：按 `spec::plan` 选策略调 `build_with_sharding` | ✓ |

**定性：核对确认（否决任何进一步改动）**。回归守护 = 既有 `cli_startup_*` 三键 + spec.rs 19 命令穷尽分类测试（深审只确认存在与注释口径，断言体未逐条展开——如实标注）。

### 4.11 5.6 同步卸载与跨 await 豁免（必闭=基线修复；增量=纪律约束）

**现势**：`sync_offload_guard_freeze.txt:19-31` 实体 **13 条**（5 GUARDED + 8 NO_ASYNC_BOUNDARY）【实测·本轮非注释计数=13】——方案记 12 条为漂移；`await_guard_allowlist.txt` 实体 **9 条** = **顶格**，「不新增第 10 条」是硬约束；镜像表 `lock_await_freeze.txt` 键集 1:1 由 G-27 机械复核。**基线红**：G-27/G-35 mismatch——`[STALE] crates/mcp-mesh/src/quantum/participant_client.rs:556`（allowlist :55【实测·本轮亲读】，守卫实际位移至 :608）。

**设计**：
- 必闭项 A：守卫位移 re-key——**双表同步**（`await_guard_allowlist.txt:55` 与 `lock_await_freeze.txt` 对应行）改 `:556`→`:608`（或按实义核查后删除）；SCANNER 理由语随行核实。
- 必闭项 B：若 5.1/5.2/5.4 任一落地引入新同步调用 → 走既有 GUARDED 模式或 `tokio::fs`/`spawn_blocking` 消掉；**禁止新增第 10 条豁免**。
- 现有 13 条（尤其 :19 overwindow_bridge.rs::dense_scores，其注释 :14-18 明示与 action_orchestrator.rs:375 spawn-worker 的关系）与 5.1 有界并发改造**无冲突**；落码后复跑 G-88 自证（OQ-13）。
- **〔修订·审读〕波内改码的冻结表 re-key 义务（补 missing-10）**：本会话实测三表对波次目标文件**零行号条目**——`await_guard_allowlist.txt`/`lock_await_freeze.txt`/`sync_offload_guard_freeze.txt` 的实体行均不锚定 action_orchestrator.rs / composition.rs / experience_loop.rs（`grep` 实测仅 `sync_offload_guard_freeze.txt:15` 注释提及 :375，非条目；GUARDED 5 条 = overwindow_bridge/faae-router×2/hcw-window/osa-coordinator【实测·本轮 sed :19-23】），故 W3/P1/P2/P3 在 composition.rs 插行、5.1 在 action_orchestrator.rs 插行**不会产生新 stale**。义务仍立（防未来条目扩展）：**每波合入前若改动波及三表锚定文件，必须复跑 G-27/G-35/G-88；出现新位移行随波 re-key（re-key 不改变条目数，不违反「禁增第 10 条」）**——列入 §10.1 通用门禁列。

### 4.12 回滚

| 层 | 手段 | 适用 |
| --- | --- | --- |
| L1 秒级 env | **〔修订·审读〕仅列正文设计中实际实现的开关**：5.1 `CHIMERA_ACTION_CONCURRENCY=unlimited`（§4.4 正文有定义）；5.3 `CHIMERA_RT_WORKERS` 未设=现行为（仅当 §4.6 被采纳时才存在）。**删除原表两行误植**：`CHIMERA_BUS_TICK_BATCH=0`（5.4 正文倾向直接否决、无聚合调用点施工，无开关可实现）与 `CHIMERA_XLOOP_PARALLEL=0`（5.2 为静态 join! 分区、无运行时切换面）——回退一律走 L2 revert | 并行化/装配路径（与正文一一对应） |
| L2 小时级 | 每点独立可 revert commit（W5.0 一个、5.1~5.6 各一个），逆序撤销 | 全部点 |
| L3 天级 | 分支级回退 | 整波下线 |

- **不提供开关的安全面**：W5.0 的 I10 不变量、5.6 的「禁增第 10 条豁免」纪律、5.5 边界（已实现态）——治理资产不是行为开关，回退=删代码不做 feature flag。
- 5.1 特别项：unlimited 回退保留 `resolve_action_concurrency` 纯函数与全部测试（只摘执行体分支，同 W2 回退先例）。

### 4.13 验收命令与 ADR-251 决策点

```powershell
# 1. 阈值门（W5.0 后 I10 应现形 83/83）
python scripts/check_perf_redlines.py invariants
python scripts/check_perf_redlines.py all
python scripts/check_perf_redlines.py compare --legacy-out bench_threshold_legacy.log   # hnsw_tiered 进入 compared 集合

# 2. 门基线（5.6 前置修复后必须三门 rc=0）
python scripts/run_gate_manifest.py
python scripts/check_await_across_guard.py
python scripts/check_sync_offload_guard.py

# 3. Rust 面
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings
cargo fmt -p chimera-cli -- --check
cargo test -p chimera-cli -p event-bus --no-fail-fast --jobs 2

# 4. W5.0 词条正确性（CI 外等价验证，只读）
python -I -c "import re;y=open('.github/workflows/bench_check.yml',encoding='utf-8-sig').read();d=re.search(r'PATH_KEYWORDS\s*=\s*\{(.*?)\n\s*\}',y,re.S).group(1);print('hnsw_tiered' in re.findall(r'\"([a-z0-9_]+)\":',d,re.M))"
# 期望输出: True
```

ADR-251 决策点【W5.md §8】：**D-W5-A**（5.1 重定性为有界并发 + bound 默认 8）；**D-W5-B**（5.3 默认记录不采纳）；**D-W5-C**（5.4 无第二生产源则否决）；**D-W5-D**（I10 属门强度增长，须 ADR 记录，不改 max_keyword_overlaps=1 棘轮）；**D-W5-E**（G-27 stale re-key 归属）；**D-W5-F**（方案偏差台账：5.5 行号漂移、5.6 条数 12→13、5.1/5.2 对照键误植、emit-slo-filter 冗余——回写方案勘误或 ADR-251 追记）。

---

## 5. X2 规格 —— 治理债收口（N-1/N-2/N-4/N-5/N-8/N-9/F-07/F-08/F-06 剩余）

### 5.1 目标与三项深审新发现

X2 全部属既有 ADR 决策的执行（无新 ADR）。深审三项超出方案断言的新发现【X2.md §1】：

1. **N-1 隐藏前置缺口**：`toolplan_layers` bench 执行器未实现能力表（`benches/toolplan_layers.rs:38-43` SleepExecutor 只实现 `execute`，`capability_of` **0 命中**【实测·本轮 grep -c = 0】；`PlanRunner::new(Box::new(SleepExecutor { delay_ms: 2 }), PlanGuards::default())` 在 **:88-91**【实测·本轮 sed；〔已修正〕原 X2 写 :84-88，勘误 5 订正为 :88-91，本轮实测与勘误一致】）——判决点 `toolplan_runner.rs:410-411` `node.side_effect.or_else(|| executor.capability_of(declared))`【实测·本轮 sed :405-413】⇒ 未声明计划 + 沉默执行器 = **恒串行**，直接重跑 bench 必测串行读数，×2.5 后 ≥160ms 触发「只收紧不放宽」护栏停机。
2. **N-1 的 09:46 日志不可用作收紧依据**：`tmp/x3_bench_toolplan.log`（mtime 09:46）早于 `c6eb9b6`（11:21 能力表提交）；其 `undeclared8_ms2/8` mean 75.628ms ≈ 串行，×2.5 = 171.58ms > 160ms = 放宽，棘轮禁【深审实测 X2.md §4.1 陷阱①】。
3. **方案 §5.1 步骤 4 的前提与机制不符**：`check_ci_manifest_parity.py` 按「workflow 是否引用 `scripts/*.py`」对账（:57-61 SCRIPT_RE），Traceability 步骤（纯 `git ls-files`）从不进入 ci_steps ⇒ F-06 断言扩展**不需要** manifest 联动（X2.md §8.3）。

**〔已修正〕陷阱④撤回（勘误 1，severity=high，本轮全量实证）**：原 X2.md §4.1 陷阱④断言「ADR-254 §6 承诺的改前快照 `tmp/x1_criterion_backup/toolplan_layers_pre_x3` 不存在」——**与仓库实况不符，撤回**。本轮实测证据链：

- `ls tmp/x1_criterion_backup/` = **4 项**：`criterion_6475027.tar.gz` + `dispatch_chain_pre_x2/` + `dispatch_layers_pre_x2/` + `toolplan_layers_pre_x3/`（非 X2 所记 2 项）；
- `toolplan_layers_pre_x3/` 内含全部值组数据（`1/`、`8/`、`readonly8_ms2/`、`undeclared8_ms2/`，各自含 1/8/report 子层）；
- `toolplan_layers_pre_x3/undeclared8_ms2/8/new/estimates.json` 读取：**mean.point_estimate = 63,804,161.25 ns（=63.804ms，恰为 ADR-252 登记口径）、median.point_estimate = 58,348,375 ns**；`base/estimates.json` 与 new **同值**【实测·本轮 python json.load】；
- `docs/architecture/ADR-254-executor-capability-table-and-route-honesty.md:140` 明文「改前 estimates 快照：`tmp/x1_criterion_backup/toolplan_layers_pre_x3`」【实测·本轮 sed :138-142】；
- 备份目录 mtime **2026-11-07 09:44**（早于 09:46 bench 与 11:21 提交 `c6eb9b6`）【实测·本轮 ls -la】;
- X2 原「tar 包 0 命中 undeclared8_ms2」属实（`tar -tzf criterion_6475027.tar.gz | grep -c undeclared8_ms2` = 0【实测·本轮复现】），但**不能推出「备份不存在」**——tar 是 commit `6475027` 时点包，目录快照是其后的独立产物。
- 注：tmp/ 不入库，无法排除「X2 取证后该目录才被创建」的时序可能，但**当前实况与断言矛盾属实**，且 ADR-254:140 的明文引用 + 读数同源使该目录成为改前基线的合法锚。

### 5.2 五元组

| ID | 模块 | 文件:行号 | 影响 | 阻塞 | 前置 |
| --- | --- | --- | --- | --- | --- |
| N-1 | gqep-executor bench / 性能台账 | `perf_thresholds.toml:116`（值）+ `:104-115`（注释块）+ `:270`（关键词，不动）；`bench_check.yml:394`（heredoc 值）+ `:530`（关键词，不动）【实测·本轮】；`benches/toolplan_layers.rs:38-43/:88-91`（测量面缺口）【实测·本轮】；ADR-254 §6（回填） | 生产默认路径的并行收益无阈值守护；160ms 是改前上界 | 开放（本波闭合） | bench 执行器补 `capability_of` 声明（§5.3）→ **核验既有 pre_x3 快照并装回作 base（§5.4 步骤 3-3.5，ADR-252 D4 的勘误 1 修订形态）** → 实测 → ×2.5 |
| N-2 | 治理索引 | `adr_index.md:431`（ADR-254 补行**已闭合**【实测·本轮亲读】）；余微订正 `:423`「七份」→「八份」【实测·本轮亲读 :423 确含「无这七份文件」与 :240-247 八行登记矛盾】 | 索引覆盖完整性 | 已闭合（余微订正） | 无 |
| N-4 | CI bench 门注释 | `bench_check.yml:11`（「运行 5 个」）+ `:69-71`（「47 个 run_bench 别名」）【深审实测；现势真值 = **55**（I3/I7 本轮实测）】 | 注释整数漂移误导 | 开放（本波闭合） | 无（纯注释） |
| N-5 | event-bus 注释 | `bus.rs:2632-2634`（Ask 圈定）+ 同类 `:47`/`:66`/`:135`【实测·本轮 :66「21 = 17(mpsc 旁路)+ 4(...」亲读】；常量真值 `:62`=18/`:69`=22【实测·本轮】 | 注释字面量 17/21 vs 常量 18/22；I8/I8b/I8c 扫描面不覆盖字面数字注释 | 开放（本波闭合） | 无 |
| N-8 | chimera-cli chat REPL | `commands/chat.rs:551`（守卫 :530-536；注册表 :399/:441）【深审实测】 | 防御性不变式，语义正确 | 开放（**登记不动**） | 无 |
| N-9 | 过程规格台账 | `.trae/specs/optimize-toolchain-dispatch-parallelization/{spec,tasks,checklist}.md`；`.gitignore:87`【实测·本轮 :87 = `.trae/`】 | 过程文档不入库 | 开放（X2 裁决，默认**不入库**） | 用户裁决（OQ-17） |
| F-07 | 事件台账 | `event_ownership_baseline.txt:36` 尾注【实测·本轮】；三账对照 `variant_count_test.rs:49`（154）+ `doc_count_freeze.txt:25`（154）【实测·本轮】 | 三账漂移（仅尾注统计行） | 开放（本波闭合） | `--emit` 机器再生；W3 联动协议 |
| F-08 | 覆盖率台账 | `scripts/coverage_per_crate_floor.toml`（`[[crate]]` 现 6 行，缺 gqep-executor）【实测·本轮 grep -c = 6】 | 核心 crate 的 ≥85% 验收无逐 crate 锚 | 开放（两步走） | 步骤 1 本地；步骤 2 出数点=CI coverage.yml |
| F-06R | 可回溯性（剩余） | `adr_index.md:240-247/:423`；worktree `.qoder/worktrees/toolplane-execution/`（8 文件）；`ci.yml:54-60`；`.gitignore:68/:71/:81-85/:87` | ADR-234 正文在 main 检出零载体；229~236 八编号链在途 | 开放（建议随在途分支整批合并） | `feat/toolplane-execution` 合并；用户裁决 |

### 5.3 接口与签名（X2 全波零 Rust 生产签名变更）

涉及 Rust 面仅一处 bench-only trait 实现（N-1 前置）与三处 doc-comment 订正（N-5）。

**N-1 前置：bench 执行器补能力声明**（`crates/gqep-executor/benches/toolplan_layers.rs`，加性，文件头已有 `#![allow(clippy::unwrap_used, clippy::expect_used)]`）：

```rust
// benches/toolplan_layers.rs —— SleepExecutor 追加两个 trait 默认方法的覆写。
// WHY：ADR-252 D3 把 tplan_undeclared_parallel 的收紧锚定在「ADR-254 能力表落地后
// 未声明计划走并发分支」这一生产形态上；生产执行器（BuiltinToolExecutor，
// chimera-cli/src/exec_plan.rs:52 BUILTIN_READONLY_TOOLS）自报只读能力，
// 而 bench 的 SleepExecutor 一直沉默 ⇒ 该值组测的是「执行器也不声明」的双缺省
// 形态，不是生产默认路径。补声明后 undeclared8_ms2/8 才是 ADR-252 D3 预言的
// 「≈readonly8/8」读数，×2.5 收紧才有依据。
#[async_trait]
impl ToolExecutor for SleepExecutor {
    async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        Ok(args_json.to_string())
    }

    // ADR-254 D1：执行器自报只读能力（与生产 BuiltinToolExecutor 同形态，
    // 对齐 tests/toolplan_capability_table.rs:60 的夹具先例【实测·本轮 :60 = capability_of 夹具】）。
    fn capability_of(&self, _tool_name: &str) -> Option<SideEffectDecl> {
        Some(SideEffectDecl::ReadOnly)
    }

    // ADR-254 D4：declared_tools 供路由表建表；bench 不经 RouteLayer，
    // 但实现它保持「能执行 ⇔ 已声明」的契约一致性（默认空集 = 撒谎）。
    fn declared_tools(&self) -> Vec<String> {
        (0..8).map(|i| format!("bench.readonly.{i}")).collect()
    }
}
```

语义登记义务：此改动使 `undeclared8_ms2` 组从「双缺省」变为「计划未声明+能力表声明」（= 生产默认路径）；「执行器亦沉默 ⇒ 保守串行」的代价由 `tests/toolplan_capability_table.rs::unknown_capability_stays_serial_fail_closed`（ADR-254 §3 结构锁）承接。**必须写入 ADR-254 §6 追记**，否则历史读数（63.804/68.632ms）与新读数失去可比性而无人知情。备选 A2（保守）：不改 `undeclared8_ms2`，新增 `capability8_ms2` 值组——代价 = 新增 2 键（I1 83→85 / I3、I7 55→57 / I9 85）双源四写 + I2 词条自查（OQ-15 裁决）。

**N-5：bus.rs doc-comment 订正文本**（四处；订正原则 = 注释不再抄写会漂移的整数，指向常量——ADR-252 D5 先例）：

```rust
// ── :2632-2634（test_critical_double_list_d8_counts 的 doc）──
    /// D-8 双清单计数守护(P1-T11):CRITICAL_MPSC_VARIANTS、
    /// CRITICAL_TOTAL、mpsc ⊆ Critical、LANE_FORBIDDEN_SHARD 一一对应
    /// (常量真值见 :62/:69——注释不再抄数字,防 M44 同型漂移;2026-11-07 X2 订正)

// ── :47 ──  旧「(17 ⊆ 21)」→ 新「(mpsc ⊆ Critical,规模见下常量)」
// ── :66 ──  旧「21 = 17(mpsc 旁路)+ 4(...」【实测·本轮原文在】
//            → 新「总数 = CRITICAL_MPSC_VARIANTS(mpsc 旁路)+ 4(历史 Critical 只走 broadcast:」
// ── :135 ── 旧「互锁测试守护(17 ⊆ 21)」→ 新「互锁测试守护(mpsc 旁路清单 ⊆ severity()==Critical 清单)」
```

**〔已修正〕运行期字面量归类（勘误 6）**：`bus.rs:2684` 与 `:2794` 均为 `assert!` 消息「mpsc 变体 {name} 不在 severity() Critical 清单中(17 ⊆ 21 违反)」【实测·本轮 sed 两处逐行】；**`:3622` 是注释行**「// 4 个广播专属 Critical 不得被 mpsc 旁路误捕(17 ⊆ 21,差集恒为 4)」【实测·本轮】，非断言消息——〔已修正〕原 X2 §8 Q3 把 :3622 与前两者同归「断言消息字面量」归类不精确，硬化项清单随之细分：断言消息字面量 = :2684/:2794 两处；注释字面量 = :3622（与 :47/:66/:135 同类）。运行期断言消息与历史标签测试名 `mpsc_bypass_exact_13_not_17`（:3609-3611）**本波不动**（改名/改消息属行为面微变，可选硬化 OQ-16）。

**N-4：workflow 注释订正文本**（`bench_check.yml`）：

```yaml
# ── :11 引导句改（〔已修正〕勘误 4：5 条示例行的行号 = :12-16【实测·本轮 sed :8-26 逐行】，
#    :18-24 是 render_bench 阈值说明注释块，原 X2「:18-24 列出 5 条阈值示例」行号漂移）──
# 运行全部 registered bench（run_bench 别名数以 `python scripts/check_perf_redlines.py
# invariants` 的 I3/I7 行现取为准，勿把此数抄死——ADR-252 D5：注释整数必漂），
# 解析 estimates.json,断言中位延迟。下列 5 条为首批历史示例（非全集）：
#   - exit_code_matrix / diff_engine_bench / wal_recovery / degradation_chain / render_bench（:12-16 原样保留）
# ── :69-71「47 个 run_bench 别名」段改 ──
    # WHY: 兜底防卡死。不在此登记 bench 数与典型耗时：注释里写死的计数已两度
    # 失真（「5 benchmark」→「47 个」→ 2026-11-07 实测 55）。别名数的唯一权威源
    # = `check_perf_redlines.py invariants` 的 I3/I7 行（与 [[run_bench]] 逐对对拍）。
```

**F-08：`coverage_per_crate_floor.toml` 新增行**（步骤 1 占位形态）：

```toml
# 追加在 chimera-cli 行之后（文件末尾）：
[[crate]]
name = "gqep-executor"
floor = 45            # = [defaults].floor 占位：本地 E0463 不可代跑（本文件头注），
                      # 禁编造；待 CI coverage.yml 产物回填实测-3~5pp（棘轮只升不降）
tier = "critical"     # 四环链/批量派发/计划执行判据载体（ADR-239/252/254），step summary 单列
adr = "ADR-239"
reason = "W7-R2 重构核心 crate：四环链与 PlanRunner 判据源，聚合达标不许掩盖此处裸奔"
```

合法性论证（X2.md 亲读 `coverage_gate.py`）：V1-GHOST 检查「explicit floor 行的 crate ∈ members ∪ exempt」（coverage_gate.py:187-189/:239-241），gqep-executor 是 member ⇒ 安全；`--validate-config` 纯静态零 cargo ⇒ 步骤 1 本地可验收；floor=45 与 defaults 同值 ⇒ 步骤 1 语义零变化。

### 5.4 N-1 收紧程序（9 步；〔已修正〕步骤 3 按勘误 1 改写）

**执行波次**：X2 落码执行段是唯一指定 bench 窗口；深审/审计期禁跑（criterion 会轮转 new→base，破坏「测量态=提交态」基线）。

1. **前置确认**：`git log --oneline -1` 确认 HEAD ⊇ `c6eb9b6`；D 盘空闲 ≥40GB（bench 走 release profile `lto=true` 大量写 target/）。
2. **bench 补声明**：按 §5.3 修改 `benches/toolplan_layers.rs`（bench-only，零生产语义）；`cargo check -p gqep-executor --benches` 过编译。
3. **〔已修正〕先核验既有备份再引用（勘误 1）**：原程序「复制 `target/criterion/toolplan_layers/` → `tmp/x2_criterion_backup/toolplan_layers_pre_x2/`（禁引用不存在的 toolplan_layers_pre_x3）」——勘误撤回后改为：**核验既有 `tmp/x1_criterion_backup/toolplan_layers_pre_x3` 完整性后直接引用为改前基线**（核验项 = ①目录 mtime 早于本次 bench 窗口；②`undeclared8_ms2/8/new/estimates.json` mean=63,804,161ns / median=58,348,375ns 与 base 同值【实测·本轮读数】；③readonly8_ms2 与 1/8 层在位）。仅在实施者复核发现该目录缺失/损坏时，才回退原「先备份后测量」动作新建 `tmp/x2_criterion_backup/`。备份/引用路径写入 ADR-254 §6 追记。**该既有快照的 mean 口径与 ADR-252 登记值同源（63.804ms），恰好为 Q1「mean-同源」方案提供改前基线直接证据**（〔已修正〕勘误 1 附加结论）。
3.5. **〔修订·审读〕把快照装回 target/criterion 作 base（补 missing-6——否则 change% 对照落空）**：criterion 只对 `target/criterion/<bench>/<group>/<param>/base/` 里的存量 estimates 计算 change%；tmp/ 快照不会自动参与。操作：`cp -r tmp/x1_criterion_backup/toolplan_layers_pre_x3/undeclared8_ms2/8/base/. target/criterion/toolplan_layers/undeclared8_ms2/8/base/`（同法装回 readonly8_ms2 与 /1 各组；装回前确认现 target/criterion 的 09:46 钉定态已另行备份到 `tmp/x2_pre_restore_backup/`，装回动作可逆）。装回后首轮 bench 的 change% 即对照改前基线（预期 ~-80%「improvement」= 能力表生效读数）。
4. **跑 bench 两次，取第二轮**（规避冷启动首轮 29-33% 偏高）：`cargo bench -p gqep-executor --bench toolplan_layers`，输出 tee 到 `tmp/x2_bench_toolplan.log`。**轮转语义说明（〔修订·审读〕）**：首轮结束后 criterion 会把装回的 base 轮转覆盖——首轮 change% = 新读数 vs 改前基线（要的对照）；第二轮 change% = 第二轮 vs 首轮冷启动读数（**忽略，不作证据**）；取值 = 第二轮 estimates.json（§步骤 6）。
5. **内置合理性门（不过即停）**：第二轮 `undeclared8_ms2/8` 典型值必须回落到 `readonly8_ms2/8` 的 ~2× 以内（预期 ≈11-15ms；Windows timer granularity 使绝对值偏高属预期）。若仍 ≈65ms ⇒ 能力判决未达 bench ⇒ **停止，排查 `capability_of` 转发链，不得登记任何值**。
6. **取数与口径**：读 `target/criterion/toolplan_layers/undeclared8_ms2/8/new/estimates.json`，取 **`mean.point_estimate`**（与 ADR-252 的 160ms 登记口径同源——ADR-252 的 63.804 实为 console mean 点估计，既有 pre_x3 快照 mean=63,804,161ns 同值互证【实测·本轮】），同时抄录 `median.point_estimate` 作对照列。`new_ns = ceil(mean × 2.5)`，向上取整到 1ms 粒度（先例：63.804×2.5=159.51→160）。
7. **棘轮护栏（机检）**：登记前断言 `new_ns < 160_000_000`（只收紧不放宽）。不满足 ⇒ 负结果路径：阈值不动，按 ADR-239 §2.6/ADR-241 D6 先例在 ADR-254 §6 登记「实测不可收紧 + 原因」，N-1 转遗留（**否决任何 ≥160ms 的写值**）。
8. **双源登记（同 commit）**：① `perf_thresholds.toml:116` 改值 + `:104-115` 注释块重写（新 median、日期、backup 路径、组语义变化）；② `bench_check.yml:394` heredoc 同值（下划线格式）。关键词 `:270`/`:530` 不动。登记后跑 §5.7 门禁。
9. **ADR-254 §6 回填**：以模板替换「待填」节（模板全文见 X2.md §4.1-A，核心字段 = 测量树 commit、命令、改前基线路径、readonly8/undeclared8 双值组 c1/c8 mean 表、c8/c1 ≥4× 端到端判据、阈值收紧值、测量面订正说明、完整日志路径）。

### 5.5 其余各项精确修复程序

- **N-2**（已闭合，标注即可）：`:431` 存在完整 ADR-254 登记行【实测·本轮亲读】。余两处微订正：① `:423` 补记「无这**七份**文件」→「八份」（`:240-247` = ADR-229~236 八行登记【实测·本轮】）；② `:431` 尾注在 N-1 完成后追加「（已于 <日期> 回填/收紧）」。
- **N-4**：现势真值 55（I3/I7）【实测·本轮】；订正文本按 ADR-252 D5 不写死整数（§5.3）。
- **N-5**：§5.3 订正文本；机器盲区实证——I8 只扫 shard.rs、I8c 只匹配「N 类」散文形态，「17 ⊆ 21」字面数字形态不在任何扫描面，故恒绿而漂【深审实测 X2.md §4.4 + 本轮 invariants I8b scanned=4/I8c scanned=5 旁证】。
- **N-8**：处置 = **登记不动，零代码改动**。语义：守卫 `chat.rs:530-536` 先于 match :543-552；注册表恰 6 命令与 match 6 臂一一对应 ⇒ 该臂今日真不可达；存在价值 = 对「注册表加第 7 命令而 match 忘加臂」做 fail-visible 暴露。全 `crates/chimera-cli/src` 唯一 panic 宏命中【深审实测】。登记载体 = X2 收口报告一行 +（可选）ADR-259 P4 汇总表。
- **N-9**：处置建议**不入库**（默认）。理由：① tasks.md 21 行/checklist.md 52 行含 `[x]`，决策已由 ADR-239~255 + 已入库报告承载；② `.gitignore:87` 忽略整个 `.trae/` 目录，「父目录被排除无法 re-include」，入库须改写 `:87` 为 `.trae/*` + 三条反豁免链，爆炸半径大；③ `.trae/` 现仅 1 个 tracked 文件（`.trae/rules/nuxus规则.md`，历史 force-add 产物），照抄该模式 = 鼓励 `-f` 绕过白名单纪律。若用户裁定入库：原子步骤 = `.gitignore:87` 改 `.trae/*` + `!.trae/specs/`、`!.trae/specs/optimize-toolchain-dispatch-parallelization/`、`!.trae/specs/optimize-toolchain-dispatch-parallelization/*.md`（顺序敏感）+ ci.yml `:54-60` 断言块追加 3 行 + 单 commit 逐文件 add + `audit_commit_coupling.py`。**无需 gate_manifest 联动**（§5.3 机制订正）。
- **F-07**（7 步）：三账现状 154=154≠152（漂移仅尾注统计行，门实跑 EXIT=0）；`check_event_ownership.py --emit` 只打印 stdout（:793-813，执行安全）。程序：① 确认 EXIT=0（改前态）；② `--emit > tmp/f07_emit.txt`；③ **diff 硬门**：emit 实体行集合与现 baseline 实体 18 行必须同集同名（行序重排可接受；任何新增/消失 ⇒ 真实所有权变化 ⇒ 停止另查）；④ 重建 = 保留现头注块 + emit 实体行 + **回贴 2 行人工前缀**（`BenchmarkMetricsCollected` :15 / `VoteCastRequested` :24——emit 会剥掉）+ **回贴富证据附注**（emit 行格式是纯机器 census，现文件的人工判据附注会被降级丢失——头注未警告，程序必须显式处理）+ emit 尾注（`# total 154 ...`）；⑤ 复跑门 EXIT=0；⑥ 三账复读 154=154=154 ⇒ F-07 闭合；⑦ **W3 联动协议**：W3 走事件路线则七点位同批使三账 →155（顺序 = X2 先行闭合在 154、W3 在后自担 155）；W3 走计数器路线则三账保持 154。**不得**把 emit 的 assembly-gated 段写进本文件（独立台账 `scripts/event_assembly_gated.txt`，脚本 :70-77 分工注释）。
- **F-08**（两步走）：步骤 1 本地 = 追加 gqep-executor 行（floor=45 占位）+ `python scripts/coverage_gate.py --validate-config` EXIT=0。步骤 2 回填 = CI 出数后：出数点 A = coverage.yml 的 `Coverage gate verdict` 步骤日志——**〔已修正〕`coverage_gate.py:388` 才是逐 crate 打印循环**（`print(f"  [{flag}] {c}: {pct:.2f}% (floor={floor})")`，gate 绿时逐 crate 行 flag=[OK]【实测·本轮 sed :385-395】；原 X2 引 :391-393 是失败分支 `[FAIL] coverage gate violations`——行号订正为 :388，打印形态实为 `[flag]` 前缀，语义描述不变）；出数点 B = artifact `coverage-report-<run_number>`（retention 45 天）内 `coverage/coverage.json` 按路径前缀自算。回填 `floor = 实测 − 3~5pp`（只升不降）；本地禁止项重申：`cargo llvm-cov` 本机 GNU 缺 profiler runtime（E0463）——**本地跑不出数，任何本地「估算覆盖率」都属编造**。
- **F-06R**：推荐 R-1 = 随 `feat/toolplane-execution` 整批合并（八文件互引成链，单拷 234 = 编号链半入库；`.gitignore:71` `!docs/architecture/ADR-*.md` 白名单已覆盖，合并即自动入库零新行）；R-2 备选 = 仅拷 ADR-234 单文件 + `:423` 显式登记「半入库态」；**否决项 = 静默拷贝不订正补记**。终稿文档自身入库路径（通用模板）：`.gitignore` F-06 块追加 `!docs/reports/<终稿文件名>.md` + ci.yml `:54-60` 断言追加 `git ls-files --error-unmatch` + **无需 gate_manifest 联动**（parity 机制订正：`check_ci_manifest_parity.py:57-61` 按 scripts/*.py 引用对账，Traceability 步骤从不进 ci_steps）+ 单 commit 逐文件 add + `audit_commit_coupling.py`。

### 5.6 TDD 矩阵

> X2 无新增 Rust 行为 ⇒ 无新增单元/集成/proptest 测试。本波「测试」= 既有机器门矩阵（改前基线均为深审亲测或本轮实测）：

| 层 | 检查 | 命令 | 改前基线 | 期望（改后） |
| --- | --- | --- | --- | --- |
| 台账一致 | 性能不变量 | `check_perf_redlines.py invariants` | EXIT=0；I1 83/83、I2 1≤1、I3/I7 55、I9 83、I4a 1≤1、I8 22、I8c 18【实测·本轮】 | N-1 后键数不变（A1 路线）；A2 路线则 85/57/85 |
| 台账一致 | bench 清单 | `check_perf_redlines.py inventory` | EXIT=0，toolplan_layers 在册【实测·本轮】 | 不变 |
| 静态门 | 阈值静态自证 | `bash scripts/check_perf_redlines.sh --static-only` | 未跑（发布清单项） | X2 落码段跑 rc=0 |
| 事件账 | 所有权门 | `check_event_ownership.py` | EXIT=0，154/136/18/18【材料实测 facts.md G-5】 | F-07 后账本 18 不变 |
| 事件账 | --emit 再生核对 | `check_event_ownership.py --emit`（stdout） | 实体 18 行集合 == baseline 实体行 | 步骤 3 硬门通过 |
| 覆盖率 | 配置静态自洽 | `coverage_gate.py --validate-config` | 未跑 | 步骤 1 后 rc=0 |
| CI 对账 | manifest parity | `check_ci_manifest_parity.py` | EXIT=0，matched=69/unmatched=0【材料实测 facts.md G-7】 | F-06 扩展后不变 |
| 编译 | bench 面 | `cargo check -p gqep-executor --benches` | 未跑（深审纪律） | rc=0 |
| 行为锁 | 能力表结构测试 | `cargo test -p gqep-executor toolplan_capability_table` | 未跑 | 5 条全绿（bench 补声明不得破坏） |
| 端到端 | toolplan bench | `cargo bench -p gqep-executor --bench toolplan_layers` | **禁跑（审计期）** | 落码段按 §5.4 步骤 4 执行 |

proptest：零新增；既有 `toolplan_concurrency_proptest.rs` 四条不变量在 bench 补声明后**一条未改、全绿**（与 ADR-254 §3 同判据）。

### 5.7 同轮对照基准

- **N-1（唯一涉测量项）**：同轮三重对照——① `readonly8_ms2/8` 为「并发上限」同 run 对照组；② **改前态 = `tmp/x1_criterion_backup/toolplan_layers_pre_x3/`**（〔已修正〕勘误 1：09:44 钉定态，undeclared8/8 mean=63.804ms/median=58.348ms，能力表落地前【实测·本轮】）为跨轮基线——**生效方式 = §5.4 步骤 3.5 装回 target/criterion 作 base 后，首轮 change% 自动对照（〔修订·审读〕补前提，原「自动对它计算」缺装回动作）；预期 ~-80% 「improvement」是能力表生效的预期读数、非性能优化宣称，追记模板含说明义务**；③ 判据比值 c8/c1 ≥ 4×（ADR-254 §3）——**口径（〔修订·审读〕补 missing-12）= `mean.point_estimate` 比值（与阈值登记同源，§5.4 步骤 6）；median 比值仅抄录对照列不参与判定**（pre_x3 样本 mean=63.804ms 与 median=58.348ms 相差 ~9%，两口径会给出不同判定，必须钉死其一；若 mean 口径不达标而 median 口径达标，按负结果路径登记并交 OQ-14 复裁）。证明力等级 L4（criterion），仅用于阈值登记，**不用于宣称用户可见性能**。
- 其余各项零性能面——不设对照，杜绝「为台账变更编造对照」的假仪式；F-07/F-08 的对照是**集合一致性 diff**，不是时间对照。

### 5.8 回滚

> commit 粒度遵循 §12 L2 统一口径：X2 按本表「项」粒度分 commit（N-1 一个 / N-4+N-5 注释可合一个 / F-07 一个 / F-08 一个 / F-06R 终稿豁免一个 / N-2 微订正搭车），逐项独立可 revert。

| 项 | 回滚动作 | 验证 |
| --- | --- | --- |
| N-1 | revert 双源两处值编辑 + bench 补声明；改前态回拷 `target/criterion/toolplan_layers/`（从 pre_x3 快照恢复钉定态） | `invariants`（键数复原）+ bench 文件编译 |
| N-2 | 微订正文字单独 revert | `grep` 复读 |
| N-4/N-5 | 纯注释 revert，零行为面 | `invariants`（I8/I8c 仍 PASS）+ `cargo check -p event-bus` |
| N-8 | 无动作 | — |
| N-9 | 默认无动作；若入库按 §5.5 逆序撤 | `git status` 干净 + ci 断言不红 |
| F-07 | `git checkout -- scripts/event_ownership_baseline.txt`（文件 tracked） | 门复跑 EXIT=0 |
| F-08 | 删除新增 `[[crate]]` 行（floor=45 与 defaults 同值，删除后语义不变） | `--validate-config` rc=0 |
| F-06R | R-1 路径随分支 revert；终稿豁免 = 删 2 行 + `git rm --cached` | `audit_commit_coupling.py` + parity |

### 5.9 验收命令与 ADR 决策点

```bash
# ① N-1 双源+bench 面
cargo check -p gqep-executor --benches
cargo test -p gqep-executor toolplan_capability_table
cargo test -p gqep-executor toolplan_concurrency_proptest
cargo bench -p gqep-executor --bench toolplan_layers        # §5.4 步骤 4（先核验既有备份！）
python scripts/check_perf_redlines.py invariants
python scripts/check_perf_redlines.py inventory
bash scripts/check_perf_redlines.sh --static-only

# ② N-4/N-5 注释面
cargo check -p event-bus
python scripts/check_perf_redlines.py invariants

# ③ F-07
python scripts/check_event_ownership.py
python scripts/check_event_ownership.py --emit

# ④ F-08
python scripts/coverage_gate.py --validate-config

# ⑤ F-06R / N-2
python scripts/audit_commit_coupling.py
python scripts/check_ci_manifest_parity.py
git ls-files --error-unmatch docs/architecture/adr_index.md

# ⑥ 波次收口
cargo clippy -p gqep-executor -p event-bus -p chimera-cli --all-targets --jobs 2 -- -D warnings
cargo fmt -p gqep-executor -p event-bus -- --check
# 全量回归 cargo test --workspace --no-fail-fast --jobs 2 —— 若 X2 单独成批则跑；与 W3 同批则并入该批验收
```

ADR 决策点（X2 原则上无新 ADR；Q1/Q2 建议随 ADR-254 §6 追记承载）：
- **Q1（N-1 口径）**：×2.5 输入取 `mean.point_estimate`（同源）还是 `median.point_estimate`（稳健）。**推荐 mean 同源 + median 抄录对照**——〔已修正〕勘误 1 后推荐被强化：既有 pre_x3 快照的 mean=63,804,161ns 与 ADR-252 登记值 63.804ms 逐位同源，为 mean-同源提供改前基线直接证据。
- **Q2（N-1 测量面）**：A1 改 `undeclared8_ms2` 执行器语义（推荐，键数不变、直连 ADR-252 D3 机检收紧路径）vs A2 新增 `capability8_ms2` 组（+2 键四写同步）。推荐 A1。
- **Q3（N-5 硬化）**：〔已修正〕细分后——断言消息字面量（:2684/:2794）是否 format! 自常量派生 + 测试改名是否纳入本轮；:3622 等注释字面量已随 N-5 主体处置。**推荐本轮只修 4 处注释，硬化缓一拍**。
- **Q4（N-9）**：`.trae/specs` 入库与否。推荐不入库。
- **Q5（F-06R 链处置）**：R-1 整批合并（推荐）vs R-2 单文件补建；「终稿文档」具体指哪份。
- **Q6（执行顺序）**：X2 与 W3 的顺序——方案定 X2 先行；若先跑 W3 且走事件路线，F-07 应直接在 155 上执行（跳过 154 中间态）。
- **Q7（9 门 mismatch 移交）**：`run_gate_manifest.py` 9 门 mismatch（G-04/G-18/G-27/G-35/G-53/G-72/G-91/G-123/G-124）不在 X2 范围，但红因与治理债同面——是否另立波次。

---

## 6. P1 规格 —— McpToolExecutor 接入四环传输环（F-10 主体，ADR-256）

### 6.1 目标

工具执行面与 MCP 面打通：`DispatchRequest` 可路由到 MCP 参与者，四环完整前置无旁路；组合根唯一装配点（ADR-244 D1）；**零新增事件变体、零新增 crate、零 JSON-RPC 栈**（ADR-253 边界）。

### 6.2 五元组

| 模块 | 文件:行号 | 影响 | 阻塞 | 前置 |
| --- | --- | --- | --- | --- |
| 新执行器 `McpToolExecutor` | `crates/chimera-cli/src/mcp_tool_executor.rs`（**新建**；挂载点 `chimera-cli/src/lib.rs` pub mod 列表 :56-101 内插一行）【深审实测】 | 工具面 × MCP 打通 | P1 主体 | L7 trait `ToolExecutor`（`gqep-executor/src/toolplan_runner.rs:34-68`，L10→L7 向下合法） |
| 复合执行器 | 同上文件（`CompositeToolExecutor`） | Builtin 与 Mcp 双 delegate 组合，`declared_tools`/`capability_of` 仅转发 Builtin（保 ADR-254 D4 路由表） | — | `BuiltinToolExecutor`（`exec_plan.rs:41-82`） |
| 组合根装配 | `composition.rs:305-313`（改 `tool_delegate` 构造一行；`with_optional_delegate` 唯一调用不变）【实测·本轮 :305-313 亲读】 | 无第二装配点 | — | ADR-244 D1；`tool_runtime_policy` :174-206 不动 |
| 命名解析 | `mcp_tool_executor.rs` 内纯函数；约束真值源 `nexus-core/src/config.rs:680-685`（`McpServerConfig{id,url}`，id 无字符校验）【深审实测】 | `mcp.<server>.<tool>` 解析边界 | — | ADR-256 D1/D3 |
| SSRF 双道守卫复用 | `mcp-mesh/src/server_registry.rs:194`（`validate_endpoint`）+ `:281`（`is_reserved_ip`）【深审实测】 | 内网/元数据地址拦截 | — | 注册范式复用 `commands/mcp.rs:102-137` |
| permission 前置语义 | `commands/mcp.rs:238-244` 范式；McpToolExecutor 非 CLI 上下文改由 **policy 环 Ask 语义承担** | 破坏性操作确认 | — | ADR-256 D5 |
| policy 环定向 | `seccore/src/execpolicy.rs:112-143`（program 精确匹配）+ `composition.rs:174-206` | 规则可定向单工具全名 | Z-1 订正 | ADR-256 D6 |
| 事件面 | `event-bus/src/types.rs:514` `ExecutionCompleted`（既有变体；**其生产发布者是 quest-engine 的 Quest 完成路径** `quest-engine/src/engine.rs:544/:553`【实测·本轮 grep，非四环派发路径】——见 §6.3 尾部澄清）【实测·本轮行号】 | 零新变体；P1 不新增发布点、不为 mcp 调用虚构发布（防 U-10 同型「有变体无生产发布」） | — | 事件预算纪律 |
| ADR-253 边界调和 | `adr_index.md:437` 登记【实测·本轮】 | 不新造 JSON-RPC | — | ADR-256 D7 |
| `mcp serve` | `commands/mcp.rs:197-204`（恒 NotImplemented） | 不在本轮 | — | 显式排除 |

**现势订正（深审对上游方案的增量，均有实证）**：
- **Z-1**：上游骨架 4「用户可写 `pattern: "mcp.*"` 规则」**不成立**——`RulePattern::matches` 的 program 匹配是**精确（小写）**（`execpolicy.rs:134`），仅 args_glob 走 glob（:141）；`mcp.*` 作 program 恒不命中。
- **Z-2**：上游骨架 8「缺 server/缺 tool/非法字符 → Route 错误」**机制上归因 Transport**——ToolExecutor 的 `Err(String)` 经 `InProcDispatcher` 包装为 `DispatchError::Transport`（inproc.rs:40）；`DispatchError::Route` 仅由缺环构造（chain.rs:265）或 RouteLayer 无 fallback 命中产生（chain.rs:104-105），而现装配恒有 fallback（runtime.rs:179）。

### 6.3 接口签名（Rust 2021 edition）

```rust
// crates/chimera-cli/src/mcp_tool_executor.rs（新文件）
//! McpToolExecutor — 将 `mcp.<server>.<tool>` 形态的工具调用接入四环传输环（ADR-256）
//!
//! WHY L10 落点：mcp-mesh(L10) 与 chimera-cli(L10) 同层互引合法；
//! ToolExecutor trait 属 L7（gqep-executor），L10→L7 向下依赖合法。
//! WHY 不在 commands/mcp.rs 内实现：命令面是 ephemeral 一次性构造，
//! 执行器要被组合根长持（composition.rs tool_delegate），生命周期不同，分文件。

use crate::config::ChimeraConfig;
use async_trait::async_trait;
use gqep_executor::ToolExecutor;
use mcp_mesh::{McpMesh, MeshConfig, MeshServer};

/// 工具名命名空间前缀（ADR-256 D1）
pub const MCP_TOOL_PREFIX: &str = "mcp.";

/// MCP 工具名解析错误 — 语义归因 `route:`（ADR-256 D3：外层机制归因 Transport，
/// 文案前缀保留 route 语义供诊断）
#[derive(Debug, thiserror::Error)]
#[error("route: mcp tool name malformed ({kind}): `{raw}`")]
pub struct McpRouteError {
    pub kind: McpRouteErrorKind,
    pub raw: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpRouteErrorKind {
    /// `mcp.` 前缀后为空或全为分隔符
    EmptySegment,
    /// server_id 含非法字符（白名单外）
    IllegalServerId,
    /// tool 段为空
    EmptyTool,
    // 〔修订·审读〕移除原 ServerNotRegistered 变体：parse_tool_name 是纯函数、
    // 查不了运行时注册表，该 variant 在 parse 中不可达；「server 未注册」统一由
    // execute() 第 3 步内联 Err 文案承载（同一语义只留一条机制，见下方裁决注）。
}

/// server_id 白名单校验（ADR-256 D1：配置层 id 无校验，解析消歧在执行器入口收紧；
/// 不动 L1 配置 schema，避免 parity 门重派生）。
/// 规则：长度 1..=64；仅 [a-zA-Z0-9_-]；点号不在白名单内（保证解析无歧义）。
fn is_legal_server_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub struct McpToolExecutor {
    /// 进程内 ephemeral mesh（同 mcp.rs 范式；executor 与宿主同生命周期）
    mesh: McpMesh,
}

impl McpToolExecutor {
    /// 组合根唯一构造入口。注册语义逐字复用 commands/mcp.rs:102-137：
    /// url 型注册 + SSRF 拒绝 warn 跳过；stdio 型（无 url）warn 跳过；空 id warn 跳过。
    /// 默认传输恒 InProcessClient（零网络副作用）。
    #[must_use]
    pub fn from_config(config: &ChimeraConfig) -> Self { /* ... */ }

    /// 命名解析（纯函数，供 proptest 穷举）：
    /// `mcp.<server>.<tool>` → (server, tool)；server_id 白名单排除 `.` ⇒ 首个 `.` 即边界。
    pub fn parse_tool_name(tool_name: &str) -> Result<(String, String), McpRouteError> { /* ... */ }
}

#[async_trait]
impl ToolExecutor for McpToolExecutor {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        // 1. 前缀守卫（防直接注入本执行器）
        let Some(rest) = tool_name.strip_prefix(MCP_TOOL_PREFIX) else {
            return Err(format!("route: mcp executor received non-mcp tool: {tool_name}"));
        };
        // 2. 解析边界（Err 文案前缀 `route:`）
        let (server_id, tool) = Self::parse_tool_name(tool_name).map_err(|e| e.to_string())?;
        // 3. 目标服务器注册校验（真值源 = from_config 注册结果）
        let Some(server) = self.mesh.registry().get(&server_id) else {
            return Err(format!("route: mcp server not registered: {server_id}"));
        };
        let _ = server;
        // 4. 经 2PC 参与者面执行（ADR-253 边界：复用 execute_transaction，不新造 JSON-RPC）
        let result = self.mesh
            .execute_transaction(vec![server_id], tool.to_string(), vec![args_json.to_string()])
            .await
            .map_err(|e| format!("transport: mcp transaction failed: {e}"))?;
        // 5. 结果序列化为单层 JSON（勿双层编码）
        Ok(serde_json::json!({ "success": result.success, "transaction_id": result.transaction_id }).to_string())
    }

    // ADR-256 D2：能力声明诚实语义 —— 未知副作用 → None（计划层保守串行 fail-closed）；
    // 工具名全集空集 → 路由 fallback（与既有行为逐字一致）。「诚实优于虚构」。
    fn capability_of(&self, _tool_name: &str) -> Option<nexus_contracts::tool_plan::SideEffectDecl> {
        None
    }
    fn declared_tools(&self) -> Vec<String> {
        Vec::new()
    }
}

/// 复合执行器 — Builtin 与 Mcp 两个 delegate 的前缀分派组合（ADR-256 D2）
///
/// WHY 独立类型而非 enum dispatch：两分支生命周期/构造依赖不同。
pub struct CompositeToolExecutor {
    builtin: crate::exec_plan::BuiltinToolExecutor,
    mcp: McpToolExecutor,
}

impl CompositeToolExecutor {
    #[must_use]
    pub fn new(builtin: crate::exec_plan::BuiltinToolExecutor, mcp: McpToolExecutor) -> Self {
        Self { builtin, mcp }
    }
}

#[async_trait]
impl ToolExecutor for CompositeToolExecutor {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        if tool_name.starts_with(MCP_TOOL_PREFIX) {
            self.mcp.execute(tool_name, args_json).await
        } else {
            self.builtin.execute(tool_name, args_json).await
        }
    }
    // 能力/路由表只转发 Builtin（mcp 分支 None/空集是诚实声明）：
    fn capability_of(&self, tool_name: &str) -> Option<SideEffectDecl> {
        self.builtin.capability_of(tool_name)
    }
    fn declared_tools(&self) -> Vec<String> {
        self.builtin.declared_tools()
    }
}
```

组合根装配（`composition.rs:306-307` 仅改 delegate 一行，`with_optional_delegate` 单装配点不动）：

```rust
// 改前：let tool_delegate: Arc<dyn gqep_executor::ToolExecutor> = Arc::new(crate::exec_plan::BuiltinToolExecutor);
let tool_delegate: Arc<dyn gqep_executor::ToolExecutor> = Arc::new(
    crate::mcp_tool_executor::CompositeToolExecutor::new(
        crate::exec_plan::BuiltinToolExecutor,
        crate::mcp_tool_executor::McpToolExecutor::from_config(config),
    ),
);
```

装配后自动成立的行为（机制证据）：能力视图 `runtime.rs:161` → `declared_tools()` 仅 builtin 3 工具 → 路由表 3 条、`mcp.*` 不进表走 fallback（runtime.rs:179）→ 归因行为与现状逐字一致；四环前置完整；`ExecPolicyToolExecutor` 包装（runtime.rs:163）对 mcp 分支同样生效——policy 规则可拦截 `mcp.<server>.<tool>` 全名（Z-1 口径）。

**〔修订·审读〕三条澄清（回应 unclear-12 / missing-7 / missing-8）**：

1. **错误通道单一机制裁决**：`McpRouteErrorKind::ServerNotRegistered` 已从枚举移除（§6.3 代码注）——`parse_tool_name` 是纯函数、查不了运行时注册表，该 variant 在 parse 中**不可达**；「server 未注册」统一由 `execute()` 第 3 步的**内联 `Err(format!(...))` 文案**承载（上文代码即裁决形态）。ADR-256 D3 的完整口径 = 「解析边界错误走 `McpRouteError`（`route:` 前缀，供 proptest 穷举）；运行时注册态错误走内联文案（同 `route:` 前缀供诊断）——外层机制归因 `DispatchError::Transport` 不变」。TDD 锁文案前缀（`unregistered_server_fails_with_route_text`）与两机制不冲突。
2. **ExecutionCompleted 无新增发布点**：该变体的生产发布者是 quest-engine 的 Quest 完成路径（`quest-engine/src/engine.rs:544/:553`【实测·本轮 grep】），**不在四环派发链上**；P1 不为 mcp 调用新增发布点、也不宣称「复用」（「复用」的准确含义 = 零新增变体，非「本路径会发它」）——否则恰是本稿 U-10/F-09 反复警示的「有消费无生产/为绿造接线」。P1 的执行观测面 = 错误归因文案 + （W3 落地后）批量计数器；TDD 矩阵**不添加**「mcp 调用产生 ExecutionCompleted」断言。
3. **「路由表 3 条」成立条件 = CompositeToolExecutor 自身的转发实现 + capabilities 包装前就地保留（runtime.rs:161【实测·本轮亲读】）——不依赖 N-6**（`ExecPolicyToolExecutor` 只包链内传输环 runtime.rs:163，不进 capabilities；详细机制论证见 §3.7 尾部〔修订·审读〕段）。OQ-04 的归属裁决不影响本论断。

新增 `crates/chimera-cli/tests/mcp_tool_executor_wiring.rs`（集成）+ `mcp_tool_executor.rs` 内 `#[cfg(test)]` + proptest：

| 层 | 用例 | 断言要点 |
| --- | --- | --- |
| 单元 | `parse_rejects_empty_prefix_body` | `"mcp."`/`"mcp"` → Err(EmptySegment)，文案以 `route:` 开头 |
| 单元 | `parse_rejects_illegal_server_id` | `"mcp.cai..tool"`/`"mcp.a b.tool"`/`"mcp.<64+字符>.t"` → Err(IllegalServerId) |
| 单元 | `parse_rejects_empty_tool` | `"mcp.server."` → Err(EmptyTool) |
| 单元 | `parse_accepts_legal_name` | `"mcp.github.list_issues"` → Ok(("github","list_issues"))；`-`/`_` 合法 |
| 单元 | `capability_none_and_declared_empty` | 恒 None/恒空（诚实语义锁定） |
| 单元 | `composite_forwards_capability_and_declared` | `declared_tools()` == builtin 3 项；`capability_of("text.echo")==Some(ReadOnly)`、`capability_of("mcp.x.y")==None` |
| 集成 | `unregistered_server_fails_with_route_text` | 空 config → Err 含 `route: mcp server not registered`（零 mesh 注册也零网络） |
| 集成 | `inprocess_default_zero_network` | 默认档全程无 socket（复用 `tests/mcp_call_network.rs` 默认路径判据） |
| 集成 | `unknown_builtin_still_fails`（回归） | `text.nonexistent` 仍 Err 含 `unknown builtin tool`（ADR-244 D2 不回归） |
| 集成 | `policy_rule_blocks_mcp_tool` | `tool_policy.rules += {pattern: "mcp.gh.deploy", action: deny}` → Err 前缀 `policy:`（Z-1 口径） |
| 集成 | `sandbox_still_guards_mcp_args` | 注入形态 args → Err 归因 `Sandbox`（ADR-238 D4 对 mcp 同样生效） |
| 集成 | `composition_root_single_assembly`（反向） | `McpToolExecutor::from_config`/`CompositeToolExecutor::new` 全仓命中仅 composition.rs 与自身模块；`check_composition_root.py` EXIT=0 |
| 集成 | `no_second_json_rpc_stack`（ADR-253 调和） | 无 JSON-RPC 字面量/编解码（文本断言）；执行路径唯一经 `execute_transaction` |
| proptest | `prop_parse_boundary` | 任意串分派：合法形态 ⇔ 前缀+白名单 server 段+非空 tool 段（block-named 语法） |
| proptest | `prop_no_network_on_default` | 任意合法工具名 + 空注册表 → 恒 Err 且无 TCP 连接尝试 |

TDD 顺序：红（`composite_forwards_capability_and_declared` + `parse_*` 先行，模块不存在编译失败即红）→ 绿 → 反向断言 → 集成回归。

### 6.5 同轮对照基准

| 对照点 | A 面（经四环） | B 面（直连对照） | 判据 |
| --- | --- | --- | --- |
| 归因一致性 | `ctx.tool_runtime.execute("mcp.gh.list",…)` | `McpMesh::execute_transaction` 直调（仅测试构造） | A 面错误文案携带对应语义段；B 面 `ServerNotFound` 与 A 面 `route: mcp server not registered` 语义对齐 |
| 无旁路反证 | `execute` 全链 | 直调 `InProcDispatcher` | A 面 policy Deny 下恒 Err 且**先于** mesh 调用（零 2PC 事务创建）；B 面无 policy 环 |
| 缺省行为零回归 | P1 前 bench `cli_startup_config` | P1 后同测点（〔修订·审读〕原表误写「W1 前/W1 后」为复制残留，与 P1 无关） | 启动分段无显著劣化（`from_config` 仅读 config + 空注册表构造） |

不引入 criterion 新 bench（P1 无性能主张；性能可证伪纪律约束的是「优化」，P1 是能力接线）。

### 6.6 回滚

- **L2**：P1 单 commit 独立可 revert。revert 后 `composition.rs:306-307` 回到 `Arc::new(BuiltinToolExecutor)` 单体，`mcp_tool_executor.rs` 随 commit 消失；`tool_runtime_policy`/`with_optional_delegate` 签名零改动。
- **语义回退面**：`mcp.*` 调用回退为 `unknown builtin tool` Err——非安全层降级（四环顺序/缺环构造/fail-closed 不动）。
- **配置面**：`from_config` 只读 `config.mcp.servers`，无新增配置段 ⇒ 回滚零配置残留。

### 6.7 验收命令与 ADR-256 决策点

```powershell
cargo check -p chimera-cli --all-targets
cargo clippy -p chimera-cli --all-targets --jobs 2 -- -D warnings
cargo fmt -p chimera-cli -- --check
cargo test -p chimera-cli --test mcp_tool_executor_wiring -- --test-threads=1
cargo test -p chimera-cli --lib mcp_tool_executor
cargo test -p chimera-cli --test exec_plan_wiring --test tool_runtime_wiring --test tool_policy_config   # 回归
cargo test -p gqep-executor --test tool_runtime_borrowed_equivalence                                      # 四环等价回归
python scripts/check_composition_root.py        # canonical 7 点 + 0 bypass 不变
python scripts/check_perf_redlines.py invariants
python scripts/run_gate_manifest.py             # mismatch 项对照 facts.md G-8 基线（9 项既有红非本轮引入）
```

ADR-256 决策点（D1~D8 摘要，全文见 P1-P4.md §8）：**D1** 白名单 `[A-Za-z0-9_-]{1,64}` + 首 `.` 切分（A 案采纳；B 案最长前缀匹配依赖运行时状态否决；C 案动 L1 schema 否决）；**D2** 独立 struct `CompositeToolExecutor` + 前缀分派；**D3** 解析错误机制归因 `DispatchError::Transport`、文案前缀保留 `route:`（Z-2 订正采纳）；**D4** 传输缺省恒 InProcessClient、本轮不引入新配置段；**D5** 非 CLI 上下文不直调 permission::confirm，破坏性控制由 policy 环 Ask 承担；**D6** policy 定向须写全名（`mcp.*` 通配当前不可行，Z-1 订正；样例 `{ pattern: "mcp.github.create_issue", action: ask }`）；**D7** ADR-253 边界调和——复用同一 2PC 参与者面，两面共享 registry 与 SSRF 守卫，`mcp serve` 保持 NotImplemented；**D8** 诚实性声明（必写进 ADR 正文）：默认档 prepare/commit 恒 ACK ⇒ 成功回执是**协议级 ACK 非业务级工具结果**，真实业务结果待真实 server 接入批次。

---

## 7. P2 规格 —— nexus-subagent 装配接线（U-02，ADR-257）

### 7.1 目标

`SubAgentQuestEngine`（L10 适配器，自陈 GATED-UNWIRED【实测·本轮 :1/:35 亲读】）经组合根真实装配；委托驱动面闭合（spawn → 执行 → join → 事件回执）；零旁路 ToolRuntime。

### 7.2 五元组

| 模块 | 文件:行号 | 影响 | 阻塞 | 前置 |
| --- | --- | --- | --- | --- |
| L10 适配器（已写好） | `nexus-app-server/src/subagent_engine.rs:1/:35`（双处 GATED-UNWIRED 自述）【实测·本轮】；`spawn_quest` :71-109、`join_next` :112-114 | 适配器 API 已备，缺装配 | P2 主体 | — |
| L7 运行时 | `nexus-subagent/src/lib.rs:5-6`（CONSUMERS: 1 nexus-app-server）；`SWARM_LIMIT=8`（types.rs:8）；禁嵌套运行期断言（runtime.rs:110-132）【深审实测】 | 不变量已实现 | — | L10→L7 合法 |
| 组合根装配点 | `composition.rs`（`build_with_sharding` :245-379 内新增装配行 + `AppContext` :93-134 新增字段）【深审实测】 | 装配点唯一 | — | **所有权方案裁决（D1）** |
| Quest 委托驱动面 | **现势 quest-engine 无委托钩子**（生产码 delegate/AgentTaskDelegated 0 命中）；既有委托面是 chimera-mas `RootOrchestrator::delegate`（agent.rs:15-16/:176-181）【深审实测】 | 「Quest 分解自动委托」机制不存在，须显式裁决 | — | 决策点 D2/D3 |
| 事件面 | `AgentTaskDelegated`（types.rs:1601）/`AgentTaskCompleted`（:1621）/`AgentTaskFailed`（:1639，Critical）【实测·本轮行号】 | 回执闭合经既有变体，零新增 | — | 事件预算纪律 |

### 7.3 接口与签名

适配器本体不动；动两处：

**D1 所有权方案（关键装配障碍）**：`SubAgentQuestEngine::new(engine: QuestEngine)` **消费 owned**【实测·本轮 :47-58 亲读】，而 `AppContext.engine: QuestEngine` 同为 owned——同一实例无法两处拥有；`QuestEngine` 未实现 Clone。三案裁决：

```rust
// 案 α（适配器改收 Arc）：推导后仍卡 Clone（组合根侧 engine 已 move 进 AppContext）——不成立。
// 案 γ（AppContext.engine 改 Arc<QuestEngine>）：波及全部命令面借用点
//   （chat.rs:113/:814/:835、exec.rs:88 等）与 build_app_server 的 move（composition.rs:434
//   QuestBackend::with_engine(ctx.engine,…)）【深审实测】，破坏面最大——否决。
// 案 β（推荐）：QuestEngine 增加 impl Clone（浅共享，与 EventBus Clone = Arc 内核同型惯例）
// quest-engine/src/engine.rs 新增：
impl Clone for QuestEngine {
    /// WHY 浅共享：全部字段为 Arc 分片容器/EventBus(Clone=Arc)/配置快照，
    /// Clone 产生同一引擎状态的两个视图（非独立副本）；与 SubAgentQuestEngine
    /// 「共享 QuestEngine」的文档语义一致。
    fn clone(&self) -> Self { Self { /* 逐字段 clone；DashMap/EventBus/Config 均 Clone */ } }
}
// composition.rs（engine 构造 :277 之后）：
let engine = QuestEngine::new(bus.clone());
let subagent_engine = Arc::new(nexus_app_server::SubAgentQuestEngine::new(engine.clone()));
// AppContext 新字段（:93-134）：
pub subagent_engine: Arc<Mutex<SubAgentQuestEngine>>,   // 非 Option（构造不可失败；D2 并发包装）
```

**D2 并发包装（〔修订·审读〕「锁外驱动」表述撤回，形态如实化——回应 missing-9）**：`spawn_quest` 收 `&mut self`（subagent_engine.rs:71）、**`join_next` 亦收 `&mut self`**（`pub async fn join_next(&mut self) -> Option<(String, Result<String, String>)>`，subagent_engine.rs:112-114【实测·本轮亲读】；`&mut` 根因 = `SubAgentRuntime.active: JoinSet<…>`（nexus-subagent/src/runtime.rs:55【实测】）的 `JoinSet::join_next(&mut self)` 所有权语义）。因此：
- AppContext 字段类型 = `Arc<tokio::sync::Mutex<SubAgentQuestEngine>>`（tokio **异步锁**——允许跨 await 持有、无死锁风险；AGENTS.md §4.4-1「禁持锁跨 await」针对的是 `std::sync`/`DashMap` **同步锁**在 async 上下文阻塞调度线程的死锁模式，对 tokio::sync::Mutex 不适用——此为语义澄清，消除原文「持锁禁跨 await——join_next 在锁外驱动」的自相矛盾：join_next 收 `&mut`，在当前签名下「锁外驱动」**不可实现**）；
- 代价声明：持锁跨 await 使同一引擎实例的 spawn/join **串行化**——与 `JoinSet &mut` 所有权语义同构，可接受；ADR-257 必须登记该代价；
- **驱动形态（第一波）**：不启动常驻驱动任务；驱动入口（后续命令面波次）在专属任务体内 `engine.lock().await.join_next().await` 循环驱动（**单消费者**——并发多个 join_next 消费者会互相抢 `&mut`，语义上也不成立）；
- 若评审要求无锁并发，需改 `SubAgentRuntime` 内部并发原语（如 JoinSet → mpsc 结果通道）——属 L7 改动，本轮否决（与原 D2 备选一致）。

**D3 驱动挂接（最小可接线形态）**：第一波只做「装配 + 显式驱动 API 暴露」，**不虚构「Quest 分解自动委托」**（机制不存在已实证）；`ctx.subagent_engine.lock().spawn_quest(spec, goal, from_task)` 驱动入口，命令面挂接留后续波次。

**深度/预算不变量表述调和（〔修订·审读〕深度与个数口径拆开——回应 unclear-17）**：「委托深度 ≤5」是 **chimera-mas INV 不变量**（MAS-Q 三层递归委托的**深度**上限；chimera-mas 侧每层委托**个数**口径本稿未裁定，以 chimera-mas INV 不变量现取为准，不与深度混写）；nexus-subagent 语义是**禁嵌套**（运行期断言拒绝运行中任务再 spawn，runtime.rs:110-132；测试 `nesting_forbidden_through_engine`）⇒ 深度上限 = 1，**比 ≤5 更严**。ADR-257 表述：nexus-subagent 禁嵌套（深度 1）优先；chimera-mas 委托深度 ≤5 是另一委托面（RootOrchestrator）的不变量，**二者不合并**（D4）。

**F-09 施工项并入（〔修订·审读〕回应 unclear-9——P4 裁「接线候选随 P2」，此处为 P2 侧对应物，消除脱节）**：

| # | 模块 | 文件:行号 | 影响 | 阻塞 | 前置 |
| --- | --- | --- | --- | --- | --- |
| F09-P2 | chimera-cli 长时动作进度发布 | `action_orchestrator.rs` route_action 长时分支（quest 提交/批量执行——**具体分支集合未决，挂 OQ-27**）；消费端 `commands/chat.rs:305-320` 已就绪【深审实测】 | `TuiActionProgressed`（types.rs:1776）补生产发布点；**request_id 维持不加**（types.rs:1772-1774 既有决策，OQ 表 D7 维持） | 随 P2 长时动作落地 | P2 D3 驱动形态裁毕 |

TDD 追加一行：`f09_progress_published_from_long_action`——长时动作执行中段发布 1~N 次 Progressed，`chat.rs:305-320` 消费面可订阅到且 `action_id` 归属正确（有消费者无生产者 → 补生产发布点，与 U-10 反向缺口同型闭环）。回滚：F09-P2 独立 commit 可 revert（不随 P2 主体回滚连带）。

### 7.4 TDD 矩阵

| 层 | 用例 | 断言要点 |
| --- | --- | --- |
| 单元（quest-engine） | `quest_engine_clone_shares_state` | Clone 后两视图对同一 `create_quest` 可见（浅共享语义锁定） |
| 集成（chimera-cli） | `composition_build_wires_subagent` | `ctx.subagent_engine` 可用；`spawned_total()==0` 初值 |
| 集成 | `subagent_spawn_join_event_closure` | spawn_quest → join_next → Quest ID 以 `quest-` 开头（复用 subagent_engine.rs:130-146 断言形态落 chimera-cli 侧） |
| 集成 | `subagent_swarm_limit_enforced` | 连发 8 个成功、第 9 个 Err（SWARM_LIMIT=8） |
| 集成 | `subagent_event_publishes_agent_task` | spawn/join 闭环内 `AgentTaskDelegated`/`AgentTaskCompleted` 可订阅到（subscribe-before-spawn） |
| 集成 | `subagent_delegation_not_bypassing_tool_runtime`（反向） | SubAgent 闭包仅 `create_quest`；生产文本扫描无第二执行链 |
| proptest | `prop_spawn_never_nests` | 任意活跃任务 ID 作 from_task → spawn 恒 Err；未知 ID 保守放行语义与 runtime 一致 |
| proptest | `prop_swarm_capacity_bound` | 任意 spawn 序列下 `active_count() <= 8` |

### 7.5 同轮对照基准 / 7.6 回滚 / 7.7 验收命令

- 对照：装配前后 `cli_startup_config` 同测点——新增构造为 `SubAgentRuntime::new()` + Arc 包装（零 IO），预期无显著差异；不显著则记录（ADR-234 L3 证明力纪律）。
- 回滚：单 commit revert——AppContext 字段、composition.rs 装配行、quest-engine Clone impl 三点同时消失；`SubAgentQuestEngine::new` 签名还原 owned（其 `#[cfg(test)]` 4 条测试不需改动）；回退后 crate 回到 GATED-UNWIRED 诚实态。
- 验收：

```powershell
cargo check -p nexus-app-server -p quest-engine -p chimera-cli --all-targets
cargo test -p quest-engine --lib quest_engine_clone
cargo test -p chimera-cli --test subagent_wiring -- --test-threads=1   # 新集成文件
cargo test -p nexus-app-server --lib subagent_engine                    # 既有 4 条回归
python scripts/check_composition_root.py
bash scripts/check_crate_reachability.sh   # 38 可达不变
```

### 7.8 ADR-257 决策点

**D1** 所有权 = 案 β（QuestEngine 浅 Clone + 适配器签名保持 owned + AppContext 非 Option 字段）；**D2** 并发包装 `Arc<tokio::sync::Mutex<…>>`（tokio 异步锁允许跨 await；spawn/join 串行化代价登记；驱动形态 = 单消费者任务体锁内循环，§7.3 D2 修订形态）；**D3** 第一波仅装配 + 句柄暴露（诚实登记自动委托机制不存在）；**D4** 与 chimera-mas `RootOrchestrator::delegate` **不合并、不互替**——mas 管复杂度分级子 Agent 委托（深度 ≤5 的递归委托面，个数口径另见 chimera-mas INV 现取），nexus-subagent 管 Quest 型长任务派发（禁嵌套深度 1）；事件共用 `AgentTask*` 三变体按 `from_task` 归属区分；**D5** 契约头同步（subagent_engine.rs:1 状态改 WIRED；nexus-subagent lib.rs CONSUMERS 计数不变仍 1）；**D6（新增，〔修订·审读〕）** F-09 发布点随本波长时动作落地（F09-P2 施工行见 §7.3，分支集合挂 OQ-27）。

---

## 8. P3 规格 —— nexus-hook 生命周期接线（U-03，ADR-258）

### 8.1 目标

`HookEventBridge::register_all` 生产化 + hook 生命周期事件发布/消费闭合 + 契约头更新（CONSUMERS ≥1）。**深审增补**：`DynamicEventRegistry` 生产构造点必须一并落地（上游方案未列）。

### 8.2 五元组

| 模块 | 文件:行号 | 影响 | 阻塞 | 前置 |
| --- | --- | --- | --- | --- |
| 注册机制（已写好，生产零调用） | `nexus-hook/src/event_bridge.rs:93-104` `register_all`【实测·本轮亲读——函数体为生产码；`#[cfg(test)]` 门的是**调用面**，全仓调用点 4 处全在 mod tests，生产调用 0】 | 去门即用 | P3 主体 | — |
| 14 个生命周期事件 | `lifecycle.rs:43` `pub const ALL: [LifecycleEvent; 14]`；`toml_section` :82-83 → `hook.*` type_id | 注册内容 | — | 配额 14 ≤ 64（event_bridge.rs:92） |
| 注册表宿主（**增补项**） | `event-bus/src/dynamic_registry.rs:58` `DynamicEventRegistry`；**EventBus 不持有**该 registry（bus.rs grep 0 命中）；lib.rs:39「零引用」注记与现势矛盾（nexus-hook 引用成立，注记漂移）【深审实测】 | 生产构造点必须新增于组合根 | — | 决策点 D1 |
| 组合根装配 | `composition.rs` `build_with_sharding` 内（`spawn_critical_subscriber` :316 前后插行）+ AppContext 新字段 | 装配点唯一 | — | — |
| 契约头 | `nexus-hook/src/lib.rs:5-6` `PRODUCERS: 0 -`/`CONSUMERS: 0 -`【实测·本轮亲读】→ 更新 | 台账诚实 | — | 契约派生器流程（ADR-224 D3） |
| subscribe-before-spawn | `composition.rs:489-491` 既有范式注释；hook 若带后台订阅任务必须同律 | 丢事件防护 | — | §4.4 反模式 3 |
| Hook 失败不反噬 | `nexus-hook/src/lib.rs:24-26`（单 hook 超时 5s 熔断不阻主流程） | fail-visible 非 fail-silent | — | — |

### 8.3 接口与签名

```rust
// composition.rs（build_with_sharding 内新增；AppContext 增字段）：
// 1) 生产构造注册表（DynamicEventRegistry::new 不在 ASSEMBLY_CALL 名单——
//    check_composition_root.py:51-53 正则仅 EventBus|QuestEngine|McaGateway|AppServer，
//    不触发门禁；但按 ADR-240 D5 精神仍只在组合根构造）
let hook_registry = std::sync::Arc::new(event_bus::DynamicEventRegistry::new());
// 2) 注册全部 14 个 hook.* 事件（幂等，event_bridge.rs:101 同 ID 覆盖为 Replaced）
nexus_hook::event_bridge::HookEventBridge::register_all(&hook_registry);
// AppContext:
pub hook_registry: Arc<event_bus::DynamicEventRegistry>,
```

```rust
// 触发实例发布/消费闭合（后续消费方挂接的最小形态）：
let trigger = nexus_hook::event_bridge::HookEventBridge::trigger_from_audit(
    session_id, &audit_entry);           // event_bridge.rs:108-110
hook_registry.register(std::sync::Arc::new(trigger));   // registry.get/serialize 可消费
```

约束落码要点：订阅/注册**先于**任何 spawn；Hook 执行失败仅 `tracing::warn!` + 计数（不反噬主流程，对齐 composition.rs:438-445 **N-4 降级姿态先例**——**〔修订·审读〕此处 N-4 是会话存储批次的债项编号，非本文件 §5.2 的 N-4（CI bench 门注释债）**：载体 = composition.rs:427 doc 注释「# N-4 降级姿态（必须先定义才能接线的那个前置）」与 :442 运行时 warn「会话持久化不可用，已降级为不双写启动（N-4 best-effort）」【实测·本轮 grep :427/:442 亲读】，语义 = 可选依赖失败时 warn + 降级续跑、不阻断主装配）；零新事件变体（`hook.*` 全走轨二 DynamicEvent，不占 154 变体数）。

### 8.4 TDD 矩阵

| 层 | 用例 | 断言要点 |
| --- | --- | --- |
| 集成（chimera-cli） | `composition_build_wires_hook_registry` | `ctx.hook_registry` 非空且 `list_by_namespace(Hook).len()==14` |
| 集成 | `hook_register_idempotent_in_composition` | build 两次互不干扰；同 registry 内重复 register_all 不增计数 |
| 集成 | `hook_trigger_event_roundtrip` | 审计条目 → `trigger_from_audit` → register → `get(EventTypeId::new("hook.pre_tool_use"))` → serialize 含 `sandbox_denied`/`command` 字段 |
| 集成 | `hook_failure_does_not_poison_main_flow` | 注册失败/触发异常 → `build()` 仍 Ok（warn + 计数） |
| 集成 | `subscribe_before_spawn_discipline`（反向） | 组合根内所有 hook 相关 spawn 点 preceding subscribe |
| 回归 | `check_event_ownership.py` | 台账同步（hook.* 属轨二不占变体数，154 不变） |
| 单元（nexus-hook 既有） | event_bridge.rs:134-197 四条 | 原样通过（零改动证明） |

### 8.5 对照基准 / 8.6 回滚 / 8.7 验收命令

- 对照：`cli_startup_config` 前/后——新增 `DynamicEventRegistry::new()` + 14 次 register（内存 Map 写入 O(14)），预期无显著差异；不显著则记录。
- 回滚：单 commit revert——AppContext 字段 + 组合根 2 行 + 契约头还原 0/0；`event_bridge.rs`/`lifecycle.rs` 零改动；回退后 nexus-hook 回到「机制已写、生产零装配」诚实态。
- 验收：

```powershell
cargo check -p nexus-hook -p chimera-cli --all-targets
cargo test -p nexus-hook --lib event_bridge
cargo test -p chimera-cli --test hook_wiring -- --test-threads=1
python scripts/check_composition_root.py
python scripts/check_event_ownership.py     # variants=154 不变
cargo clippy -p nexus-hook -p chimera-cli --all-targets --jobs 2 -- -D warnings
```

### 8.8 ADR-258 决策点

**D1** `DynamicEventRegistry` 生产构造点 = 组合根（上游方案遗漏项，必答；「挂在 EventBus」需动 L1 结构，否决）；**D2** 消费面第一波仅「注册 + 触发实例可查」，`get` 拉取消费方留独立增量——**不为绿造消费者**（U-10 教训）；**D3** `register_all` 表述订正：函数体生产码、调用面全在 cfg(test)，「去门」实际动作 = 组合根新增生产调用，非改 event_bridge.rs；**D4** Hook 失败姿态 warn + 计数；**D5** 契约头：`PRODUCERS: 0` 诚实保留（第一波只闭合注册面）、`CONSUMERS: 1 chimera-cli`——区分「注册面闭合」与「触发事件生产」两个状态。

---

## 9. P4 规格 —— U 项三态裁决（ADR-259 汇总）

### 9.1 目标

对 U-05/06/07/08/09/11/20 与 F-09/F-12 逐项给「现状 file:line + 接线成本 + 三态建议 + 理由」；全程零新增事件变体。

### 9.2 三态裁决表

| 项 | 现状（file:line） | 三态 | 理由要点 |
| --- | --- | --- | --- |
| **U-05** VoteCastRequested 双向控制通道 | `chimera-tui/src/app/event_loop.rs:1359-1369` `publish_vote` **刻意不发**（注释自陈「全仓无消费者……发布后请求石沉大海」）【深审实测；行号含 `app/` 目录层——上游写 `event_loop.rs` 未含目录层，漂移登记】 | **维持预留** | 注释自陈的不变量正是 U-10 纪律正确形态；解锁条件 = L8 提案模型落地批次 |
| **U-06** slash → ActionRegistry id 级映射 | `chimera-tui/src/actions/slash_registry.rs`（881 行；`action_id: None` 计数实测 **56**【深审实测，上游记 54 漂移 +2】；`Some` = 0） | **移交 TUI 批次** | 56 条逐条映射 + 回归面属 TUI 域（tui-r5 批次在途）；在本支引入跨域回归面违反波次隔离 |
| **U-07** `TuiChatSubmitted.slash_command` 只写不读 | `event-bus/src/types.rs:1831-1834`（恒 None）；构造点 `orchestrator.rs:316`；生产读取点 0【深审实测】 | **维持预留**（与 U-06 同批解锁） | types.rs 注释已把契约收窄写死（B4-S F-57），单方面恢复填充制造半途契约 |
| **U-08** session-store 读/回放面 | **Z-3 订正**：写面已生产装配——`composition.rs:436-437` `CbmrWriter::open` + `AppServer::with_session_store`（**N-4 降级分支 :438-445**——〔修订·审读〕N-4 = 会话存储批次债项编号，定义见 §8.3 注，非本文件 §5.2 的 N-4）【实测·本轮亲读该段】；`session-store/src/lib.rs:16-18` 订正段仍写「无 with_session_store 调用」⇒ **注记漂移**。读面 `replay_since`（server.rs:334）生产调用方 0（命中仅 tests） | 写面**已接线**（订正）；读面**维持预留**；**必做小项 = 修复 lib.rs:16-18 漂移注记** | 读面缺真实消费场景（现无断线重连 CLI 面），为绿接线=虚构消费 |
| **U-09** pvl-layer 反馈闭环 | 订正上游：① `concurrency.rs:16-26` 模块头自审生产死码，挂 ADR-224；② 上游「verifier.rs:383/399/425 feedback_rx 丢弃」需订正——生产 `run()` **有反馈发送**（verifier.rs:209/:228），`_feedback_rx` 丢弃点全在 `#[cfg(test)]`；③ 真断点 = `FeedbackChannel`（feedback.rs:35/:74）生产装配 0 | **维持预留**（挂 ADR-224 议程） | 登记在先的 ADR-224 议程面（重复裁决会双源）；上游三处行号引的是测试段丢弃 |
| **U-11** RewardSignalReported 发布链 | `omega-learner/src/lib.rs:151-153` 自陈生产零调用方；实测 `reward_signal_event` 生产 0 | **维持预留**（RL 闸门期） | 消费者是 Python 训练服务——RL 闸门期禁止实施（AGENTS.md §1.2 ⑥ 治理决策） |
| **U-20** BusLogger Prometheus 计数器导出 | **Z-4 订正**：文件 dump **已生产接线**——`experience_loop.rs:384-396`（`CHIMERA_METRICS_DUMP_PATH` env → 5s 周期 `bus.logger().render_metrics()` → `tokio::fs::write`）【实测·本轮 :384-396 逐行亲读】，调用链 `attach_observability` :361、生产调用点 :220；`composition.rs:260` 计数器真实累加【实测·本轮】 | **改判「已接线（env 可选文件 dump）」**；残余 = 文档化小项（`--help`/doctor 提示该 env）；本地端口/HTTP `/metrics` 端点**否决**（零端口暴露） | 上游「确证未接线」表述已漂移 |
| **F-09** `TuiActionProgressed` 生产发布点 | 变体 `types.rs:1776`【实测·本轮】；**消费面已存在**——`commands/chat.rs:305-320` 生产订阅消费；生产发布点 0（构造仅 chat.rs:871 测试）；**「不带 request_id」是刻意决策**（types.rs:1772-1774 注释明示） | **接线候选**（小波，随 P2 长时动作——**施工项已并入 §7.3 F09-P2 行〔修订·审读〕，分支集合挂 OQ-27**）；**request_id 维持不加** | 有消费者无生产者 = 「补生产发布点」型缺口（U-10 反向），补点真实可达消费；尊重既有 ADR 决策 |
| **F-12** 已知 TODO / 诚实失败 | ① `gqep-executor/src/lib.rs:119` TimeoutStats TODO；② `action_orchestrator.rs:259-262` task.* 五动作诚实未实现；③ `chat.rs:551` unreachable!（=N-8） | **三处全维持** | per-task 调度子系统是 Phase 3 独立 ADR 量级；unreachable 登记不动（N-8） |

### 9.3 事件预算纪律（本波次全局约束，六锁同批口径）

P1~P4 **零新增 NexusEvent 变体**（P1 事件面 = **零新增、零新发布点**——`ExecutionCompleted` :514 的生产发布者在 quest-engine Quest 完成路径 engine.rs:553【实测·本轮】，不在四环派发链，「复用」的准确含义 = 零新增变体而非本路径会发它，澄清见 §6.3；P2 复用 `AgentTask*` :1601/:1621/:1639 既有生产发布面；P3 走轨二 DynamicEvent）。若任何波次后续确需新增变体，必须**六锁同批**（现值取 facts.md §3）：

1. `variant_count_test.rs:49` `VARIANT_COUNT_LOCK`（现 154）；
2. `scripts/doc_count_freeze.txt:25`（现 154）；
3. `crates/event-bus/src/registry.rs` `define_event_registry!` 定级行（不定级即编译失败）；
4. `scripts/event_ownership_baseline.txt` 台账行（现 18 实体行；尾注 152 三账漂移 F-07 未闭合，X2 处理——新增变体前先对齐，否则新行无基线锚）；
5. `bus.rs:62/:69` 两常量（若新变体 Critical）+ `is_critical_mpsc_event` 臂数；
6. TUI 穷尽 match 补臂（`chimera-tui/tests/event_coverage_drift_test.rs` 等穷尽匹配面）。

另注意：新变体要么带生产构造者、要么带证据登记行，**禁止为绿造无消费者构造点**（U-10 教训）。

### 9.4 TDD 矩阵（ADR-259 本体回归锁）

| 用例 | 断言 |
| --- | --- |
| `u20_metrics_dump_env_path` | 设 `CHIMERA_METRICS_DUMP_PATH`(tempdir) → experience_loop 跑一周期 → 文件存在且含 `nexus_event_total`（已接线回归锁） |
| `u08_session_store_note_parity` | session-store lib.rs 注记段落含「composition.rs with_session_store」（防再漂移；文本锁） |
| `f09_no_producer_is_deliberate` | types.rs:1772-1774 注释段存在（文本锁，防误判为缺陷私加 request_id） |
| `u05_vote_remains_unpublished` | event_loop.rs:1359-1369 注释块存在 + `publish_vote` 无 publish 调用 |
| 事件计数 | `VARIANT_COUNT_LOCK=154` 不变（零新增变体机器锁） |

**跨 crate 源码路径基准（〔修订·审读〕补 missing-11）**：本表文本锁断言对象分布在 chimera-tui / event-bus / session-store 等 crate，而测试挂在 `-p chimera-cli` 集成测试下——**禁止依赖测试进程 CWD**（cargo 下测试进程 CWD = 包根，且跨平台/CI 渲染有差异）；一律以 `Path::new(env!("CARGO_MANIFEST_DIR"))` 起算（= `crates/chimera-cli`），跨 crate 用 `join("../../crates/<crate>/src/<path>")` 相对拼接到 workspace 根。既有先例：`crates/chimera-cli/tests/tool_runtime_wiring.rs:223` `std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")` 与 `tests/doctor_async_probe.rs:30` 同款模式【实测·本轮 grep 两个先例】。

### 9.5 对照基准 / 回滚 / 验收 / ADR-259 决策点

- 对照基准：不适用（裁决波无性能主张）；验收面 = 回归锁 + 门禁集。
- 回滚：ADR-259 为纯裁决 + 回归锁测试 + 少量注记订正；revert 单 commit 即回裁决前。
- 验收：

```powershell
cargo test -p chimera-cli --test panorama_adjudication_locks -- --test-threads=1
python scripts/check_event_ownership.py
python scripts/check_perf_redlines.py invariants     # I8/I8c 计数锁不变
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings
```

- ADR-259 决策点：D1(U-05 维持预留，解锁=L8 落地)；D2(U-06/U-07 移交 TUI，计数 56 登记)；D3(U-08 写面已接线订正+读面预留+注记修复)；D4(U-09 维持预留挂 ADR-224+上游行号订正)；D5(U-11 维持预留 RL 闸门)；D6(U-20 终态=已接线 env 文件 dump，HTTP 端点否决)；D7(F-09 接线候选随 P2，request_id 不加)；D8(F-12 三处全维持)；D9(全局零新增事件变体，六锁同批纪律)。

---

## 10. 全局验收矩阵（每波测试 + 量化指标 + 门禁 + 证明力分层）

### 10.1 波次级验收矩阵

| 波次 | 测试 | 量化指标 | 门禁 | 证明力 |
| --- | --- | --- | --- | --- |
| **W3** | W3-T1~T9（§2.5） | bus/cross_layer 退化 ≤10%；`tool_batch_total` 渲染值 = 批次 total | composition root / invariants（键数不变）/ event_ownership（154 不变） | L1+L2 |
| **W4** | W4-T1~T9（§3.5） | median ≥5% 且 CI 不重叠，**否则否决**；owned 臂漂移 ≤5% | invariants（bchain_* 登记前后对拍）/ inventory unknown=0 / 256-case 等价 | **L1+L4**（唯一允许 L4 判性能） |
| **W5.0** | I10 selftest 断言 | I10 83/83 现形；hnsw_tiered 进入 compared 集合（≥1） | invariants / compare | L1 |
| **W5** | §4.8 矩阵 | 每点同轮对照；不显著则记录不采纳 | G-27/G-35/G-88 三门 rc=0（re-key 后）/ invariants | L1+L3(+L4) |
| **X2** | §5.6 门矩阵 | undeclared_parallel 收紧至实测 mean×2.5（仅 A1 路线）；三账 =154 一致 | invariants / ownership --emit / coverage --validate-config / audit_commit_coupling / parity | L1（+N-1 的 L4 阈值登记） |
| **P1** | §6.4 矩阵 | 归因文案语义段对齐；启动分段无显著劣化 | composition root / invariants / 四环等价回归 | L1+L2 |
| **P2** | §7.4 矩阵 | SWARM_LIMIT=8 边界；禁嵌套 | composition root / reachability（38 不变）/ quest_engine_clone | L1+L2 |
| **P3** | §8.4 矩阵 | 14 个 hook.* 注册；幂等 | composition root / event_ownership（154 不变） | L1+L2 |
| **P4** | §9.4 回归锁 | VARIANT_COUNT_LOCK=154 不变 | event_ownership / invariants / clippy | L1 |
| **每波通用** | — | — | `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings`；逐包 `cargo fmt -p <pkg> -- --check`；`cargo check --workspace --all-targets`；波次末 `run_gate_manifest.py`（mismatch 对照 facts.md G-8 基线 9 项既有红，非本轮引入者不新增）；**〔修订·审读〕若波内改码触及 await_guard_allowlist / lock_await_freeze / sync_offload_guard_freeze 三表锚定文件，复跑 G-27/G-35/G-88 并对新增位移行随波 re-key（义务定义见 §4.11）** | — |

### 10.2 证明力分层纪律（ADR-234）

L1 结构 > L2 运行时计数器 > L3 比值墙钟 > L4 criterion；「测试全绿」不得顶替 L1；冷启动首轮 29-33% 噪声下限之下差值不构成证据。

**L4 读数的三类用法边界（〔修订·审读〕定义，回应 unclear-18——消除「W4 之外何时允许用 L4 做决策」的猜测空间）**：

| 用法 | 定义 | 允许的波次 | 决策含义 |
| --- | --- | --- | --- |
| **判性能（采纳/否决）** | 以 L4 读数决定「改造是否采纳」 | **仅 W4**（§3.6 判据） | 采纳/否决均为正式结论（ADR-249） |
| **阈值登记（棘轮）** | 以 L4 读数设定新阈值或收紧旧阈值 | 任何涉测量波次（ADR-239 既有授权）：W2 `runx_*`（已完成）、W4 `bchain_*`、W5 新键、**X2 N-1** | 只收紧不放宽 + 先实测后登记；不构成「用户可见性能宣称」 |
| **回归护栏 / 停机门** | 以 L4 读数判断「是否退化/是否停机」 | 任何涉 bench 波次：W3（bus/cross_layer ≤10%）、W5 各点同轮对照、X2 N-1 步骤 5 合理性门（未达即停不登记） | 超限触发排查/停机/回退，不产生采纳结论 |

→ X2 N-1 的 L4 用法属后两类（阈值登记 + 停机门），**不属于**「判性能」——与「W4 是唯一允许判性能的波次」不冲突；§10.1 X2 行自注「+N-1 的 L4 阈值登记」即此分类的落点。

### 10.3 全量回归口径

`cargo test --workspace --no-fail-fast --jobs 2`（基线 ≥12065 passed / 0 failed / 89 ignored【材料自述·收官报告 §3.4】；**必须带 `--no-fail-fast`**——AGENTS.md §7.2 已证不带该旗标的「全量绿」实为部分覆盖）。若 X2 单独成批则跑全量；与 W3 同批提交则并入该批验收，避免双重全量。

---

## 11. 棘轮余量与新增键约束

### 11.1 现值【实测·本轮 invariants + inventory】

| 棘轮 | 现值 | 余量判定 |
| --- | --- | --- |
| I1 键集对等 | 83/83 | 满额对等（W2 后 +4 键） |
| **I2 关键词重叠** | **1 ≤ 1** | **顶格**——任何新重叠即破门 |
| I3/I7 run_bench | 55 | 双源对等 |
| I9 heredoc 键集 | 83 | 双源对等（I10 落地后 PATH_KEYWORDS 同为 83） |
| I4a calibration_pending | 1 ≤ 1 | 顶格 |
| I6 dev-only registry | 62 | floor 60，余量 2 |
| I8/I8b/I8c | 22/22/18 | 满额 |
| inventory | total=126 / gated=11 / registered=53 / dev-only=62 / **unknown=0** | unknown 必须**恒 0** |
| G-27/G-35 豁免 | 9/9 顶格（当前 stale mismatch，5.6 前置修复） | **禁增第 10 条** |
| 事件六锁 | 154/154/18 实体/22 | F-07 闭合前三账尾注 152 漂移 |

### 11.2 新增键约束（操作规程）

1. **I2 口径（〔已修正〕勘误 7 后唯一正确口径）**：新键的 criterion 叶子路径**词条**须与既有 **101 词条**（非 83 键名）两两双向不为子串——检查实现在 `check_perf_redlines.py:595` `keyword_overlaps`（对两键的词条集做双向真子串判定；:317 调用点传 path_keywords dict）【实测·本轮 :317/:595-607 + 词条计数 101】。
2. 新键落表前必跑 §4.3 I2 自查脚本（候选键逐一验）；`runx_*` 改名教训（首版组名含 tbatch 宽词使 I2 重叠 4>1 立红，ADR-247 登记行 :429 自注）证明此步不可省。
3. 五处同 commit 双写：TOML `[thresholds_ns]`+`[path_keywords]`+`[[run_bench]]` + heredoc `THRESHOLDS_NS`+`PATH_KEYWORDS`；I10 先于第一个新键落地。
4. 先实测后登记、只收紧不放宽（ADR-239 分级余量：ns×10 / µs×3-8 / ms×2-2.5）。
5. 阈值收紧输入钉死单一尺度：N-1 用 `mean.point_estimate`（§5.4 步骤 6）；median 抄录对照列。
6. bench 目录先备份后测量（ADR-252 D4；X2 改为核验既有 pre_x3 快照，§5.4 步骤 3）。
7. `emit-slo-filter` 仅在 ignored 冻结表变动时重生成（W5.0 实证其为独立状态）。

### 11.3 新增事件变体约束

见 §9.3 六锁同批；W3 计数器路线下**本支全程零新增变体**，六锁仅作后备。

---

## 12. 三层回滚（汇总）

| 层 | 手段 | 适用 | 关键开关/动作 |
| --- | --- | --- | --- |
| **L1 秒级（env 开关）** | ADR-242 台账既有开关 + 本轮新增 | 并行化/启动/配置路径 | W3：sink 不注入 = 今日行为；W5：`CHIMERA_ACTION_CONCURRENCY`/`CHIMERA_RT_WORKERS`/`CHIMERA_BUS_TICK_BATCH`/`CHIMERA_XLOOP_PARALLEL`；既有 `CHIMERA_CONFIG_EAGER=1`/`CHIMERA_RT_FLAVOR=auto` |
| **L2 小时级（commit revert）** | **粒度统一口径（〔修订·审读〕消除 §4.12/§5.8 与本行的不一致）**：commit 粒度 = 「每个独立可 revert 的最小语义单元」——下限每波 1 个（语义不可拆时），上限遵循 CONTRIBUTING.md「每波 ≤8 工作项」；各波具体 = W3 单 commit / W4 单 commit（借↔拥有反转不可拆）/ W5.0 一个 + 5.1~5.6 各一个（共 ≤7，点间互不依赖）/ X2 按 §5.8 项粒度（N-1、N-4+N-5 合、F-07、F-08、F-06R）/ P1~P4 各自单 commit（F09-P2 在 P2 内独立）；逆序撤销，每撤销一个跑 §10 门禁 | W3/W4/W5.0/W5 各点/X2 各项/P1~P4 | W4 按 ADR-241 D5 单点回退（chain.rs 四层恢复 owned）；W2 先例「只摘 Allow 分支执行体、保留断言」适用于一切带稳定前缀的改动；X2 各项回滚表见 §5.8 |
| **L3 天级（分支级）** | 分支 `feat/toolchain-parallel-optimization` 级回退；发布按 CONTRIBUTING.md §6 | 整波下线 | — |

**不可回滚项（安全层，明确声明，无任何运行时开关）**：四环链 policy→sandbox→route→transport 顺序、fail-closed 语义、缺环构造失败、seccore 6 条不可学习红线（policy.rs:420-427）。排查读 `DispatchError` 前缀（`policy:`/`sandbox:`/`route:`/`transport:`）。

**治理资产不做 feature flag**：W5.0 的 I10 不变量、5.6 禁增第 10 条豁免、5.5 已实现边界、F-07 机器再生账本——回退=删代码。

---

## 13. 开放问题清单（需主会话/用户裁决，统一编号）

> 合并自 W3.md §9.3（Q1-Q6）、W5.md §9（Q1-Q7）、X2.md §8-9（Q1-Q7）、P1-P4.md 各波 O 项；统一编号为 **OQ-01~OQ-27**，括注来源。
> **〔修订·审读〕编号命名空间声明（回应 unclear-14）**：OQ-xx 是**开放问题**编号，与**门禁 ID**（`G-27`/`G-35`/`G-88` 等 gate_manifest 编号）**无任何关联**——本表内出现的「G-27 stale」「G-88 复跑」「G-04/G-18/G-53…9 门 mismatch」均为门禁 ID 引用（OQ-08/OQ-13/OQ-20 行），读时以「OQ-」前缀识别开放问题、以「G-」识别门禁，杜绝撞名消歧。

| # | 问题 | 各深审倾向 | 阻塞面 |
| --- | --- | --- | --- |
| OQ-01（W3-Q1） | W3 路线字面裁决：计数器路线「可行」但需加性 15-25 行（API 未现成）；按字面 → 事件路线（六锁+三账成本）。 | 计数器路线 C-A（字面条件的原意应是排除「不可行机制」而非「未写完的 API」） | W3 开工 |
| OQ-02（W3-Q2） | W4 脊柱复用 `BorrowedToolDispatcher` 而非新增同形 trait。 | 复用；ADR-249 写明概念名映射 | W4 开工 |
| OQ-03（W3-Q3） | seccore `execpolicy.rs` 的 `evaluate_ref`/泛型化是否纳入 W4 同批（全景方案文件清单只列 policy.rs）。不纳则收益上限再降约 1/7。 | 纳入同批；至少 ADR-249 显式记录取舍 | W4 范围 |
| OQ-04（W3-Q4/W5 定性） | N-6 能力转发落 W4 还是 P1（两处口径）。转发带来路由表空集→透传行为增量（W4-T8 锁）。 | 随 W4（加性、3 行级）；二选一皆须 ADR 记录 | W4/P1 |
| OQ-05（W3-Q5） | F-07 三账漂移：计数器路线下 W3 不再强制三账联动，X2 独立收口是否维持。 | 维持 X2 计划（`--emit` 机器再生，禁手改） | X2 |
| OQ-06（W3-Q6） | run.rs:321-323 预写「ToolBatchAttributed 承载」注释订正归 W3 还是留 P4/U-20 终态。 | W3 同批订正（防注释成幽灵承诺） | W3 |
| OQ-07（W5-Q1） | 5.1 意图确认：重定为「有界并发 bound=8 + CHIMERA_ACTION_CONCURRENCY 回退」？若否记录不采纳并归档证伪证据。 | 有界化（无界堆积防护价值独立于性能） | W5-5.1 |
| OQ-08（W5-Q2/X2-Q7） | G-27 stale（participant_client.rs 556→608）re-key 是否随 W5 前置闭合？不闭则 W5 验收 §4.13-2 无法全绿（与 W5.0 CI 双源治理相互独立）。 | 随 W5 前置；不闭则另立治理 commit | W5-5.6 |
| OQ-09（W5-Q3） | 是否需要 `CHIMERA_RT_WORKERS` cap 旋钮？ | 不需要则 5.3 记录不采纳 | W5-5.3 |
| OQ-10（W5-Q4） | 除 parliament debate 外是否存在真实「同 tick 高频多事件」生产源？ | 无则 5.4 否决 | W5-5.4 |
| OQ-11（W5-Q5） | I10 落地与「emit-slo-filter 无需重生成」勘误是否按 D-W5-D/D-W5-F 进 ADR-251 追记。 | 是 | W5.0 |
| OQ-12（W5-Q6） | 5.2 对照键：新键 `xloop_assemble` 还是直接记录不采纳（零新键）。 | 二选一 | W5-5.2 |
| OQ-13（W5-Q7） | 5.1 有界化采纳时 G-88 复跑自证是否列入 5.1 验收步。 | 建议是 | W5-5.1 |
| OQ-14（X2-Q1） | N-1 口径：mean（同源）还是 median（稳健）。 | mean 同源 + median 对照（〔已修正〕勘误 1 后被 pre_x3 快照强化） | N-1 步骤 6 |
| OQ-15（X2-Q2） | N-1 测量面：A1（改 undeclared 组语义，推荐）还是 A2（新增 capability8 组，+2 键四写）。 | A1 | N-1 步骤 2 |
| OQ-16（X2-Q3） | bus.rs 断言消息 format 化 + 测试改名（:3609 预告）是否纳入本轮。 | 缓一拍（X2 只修 4 处注释） | N-5 硬化 |
| OQ-17（X2-Q4） | `.trae/specs` 三件套入库与否。 | 不入库 | N-9 |
| OQ-18（X2-Q5） | ADR-229~236 链 = R-1 整批合并（推荐）还是 R-2 单拷 ADR-234？「终稿文档」入库的确切文件名。 | R-1；本文件即候选入库对象之一 | F-06R |
| OQ-19（X2-Q6） | X2 与 W3 执行顺序确认：若 W3 先行且走事件路线，F-07 直接在 155 上执行。 | 按实际排期确认 | X2/W3 |
| OQ-20（X2-Q7） | 9 门 mismatch（G-04/G-18/G-27/G-35/G-53/G-72/G-91/G-123/G-124）是否另立治理波次。 | 主会话定夺（与 OQ-08 部分重叠） | 治理 |
| OQ-21（P1-O1） | 是否立项「seccore RulePattern program 通配支持」（P1 D6 遗留；需独立 ADR + 拦截等价 proptest）。 | 留后续 | P1 后 |
| OQ-22（P1-O2） | `mcp inspect` 静态枚举只读工具时 `capability_of` 显式声明 ReadOnly 的触发条件与真值源。 | 未裁 | P1 后 |
| OQ-23（P1-O3） | `MeshConfig::default()` transaction_timeout_ms=200 对慢工具偏紧；是否为 mcp 分支引入独立超时。 | 随 D4 增量一并裁 | P1 后 |
| OQ-24（P2-O4） | `SubAgentKind`（coder/explore/plan）与命令面映射未定。 | D3 后续波次 | P2 后 |
| OQ-25（P2-O5） | 「SubAgent 真实执行 Quest」需 Quest 执行编排接入（ADR-231 D1 quest_exec 面）——P2 范围止于装配与回执闭合。 | 执行深化另立波次 | P2 后 |
| OQ-26（P3-O6/O7） | event-bus lib.rs:39「零引用」注记订正是否随 P3（一行 doc，零风险）；hook 触发的真实生产源（如四环 PostToolUse 点位）需独立波次 + 决策（动 ADR-238 链序文档）。 | 注记随 P3 订正；生产源另立 | P3 后 |
| OQ-27（P4-O8/O9） | F-09 发布点位落在 action_orchestrator 哪些长时分支（与 P2 D3 合并裁）；U-08 读面消费场景方向。 | 随 P2 裁 | P4/P2 |

---

## 14. 本合成会话验证记录（可复现命令原样）

> 全部在本会话（2026-11-07，HEAD `a0d6b6b`）执行；深审规格内的其余命令见各深审文件附录。

```
git log -3 --oneline                    # → a0d6b6b / 22adf6b / 810f18c
git status --short                      # → 空（工作树干净）
python scripts/check_perf_redlines.py invariants    # → EXIT=0，I1 83/83、I2 1≤1、I3/I7 55、I9 83、I4a 1≤1、I6 62、I8 22、I8c 18
python scripts/check_perf_redlines.py inventory     # → EXIT=0，total=126 gated=11 registered=53 dev-only=62 unknown=0
ls -la tmp/x1_criterion_backup/         # → 4 项（tar + dispatch_chain_pre_x2 + dispatch_layers_pre_x2 + toolplan_layers_pre_x3）〔勘误1〕
ls tmp/x1_criterion_backup/toolplan_layers_pre_x3/undeclared8_ms2/8/   # → base/new/report
python -I -c "<json.load pre_x3 estimates.json>"    # → mean=63804161.25 / median=58348375.0，base==new 同值〔勘误1〕
sed -n '138,142p' docs/architecture/ADR-254-executor-capability-table-and-route-honesty.md   # → :140 改前快照路径〔勘误1〕
sed -n '433,439p' docs/architecture/adr_index.md + grep -n "ADR-25[234] 登记"   # → :435=ADR-252、:437=ADR-253、:431=ADR-254〔勘误2〕
sed -n '385,395p' scripts/coverage_gate.py          # → :388 逐 crate 打印、:391 失败分支〔勘误3〕
sed -n '8,26p' .github/workflows/bench_check.yml    # → :12-16 五条示例、:18-24 render_bench 说明〔勘误4〕
sed -n '82,95p' crates/gqep-executor/benches/toolplan_layers.rs   # → :88-91 PlanRunner::new〔勘误5〕
sed -n '2682,2686p;2792,2796p;3620,3624p' crates/event-bus/src/bus.rs   # → :2684/:2794 assert!、:3622 注释〔勘误6〕
sed -n '315,319p;593,607p' scripts/check_perf_redlines.py + tomllib 词条计数   # → :317 调用、:595 定义、词条 101〔勘误7〕
grep -n "ShardingPolicy\|NoContext" crates/chimera-cli/src/spec.rs   # → :75 Auto / :85 Off / :98 NoContext（Auto臂:72-77/Off臂:78-86/NoContext臂:87-98）〔勘误8 矛盾裁决〕
grep -n "use_current_thread\|new_multi_thread" crates/chimera-cli/src/main.rs   # → :238 fn 定义（:225-227 multi 分支吻合）〔勘误9 矛盾裁决〕
tar -tzf tmp/x1_criterion_backup/criterion_6475027.tar.gz | grep -c undeclared8_ms2   # → 0〔勘误1 旁证〕
sed -n '104,122p;268,276p' scripts/perf_thresholds.toml   # → :116=160000000、:270 关键词、:272-275 runx_*
grep -n "tplan_undeclared_parallel" .github/workflows/bench_check.yml   # → :394 值、:530 关键词
sed -n '36,44p' crates/gqep-executor/benches/toolplan_layers.rs + grep -c capability_of（=0）
sed -n '405,415p' crates/gqep-executor/src/toolplan_runner.rs   # → :410-411 判决点
sed -n '100,141p;157,186p' crates/event-bus/src/logging.rs   # → BusLogger 私有 + new 3 注册
sed -n '1110,1122p;62,69p' crates/event-bus/src/bus.rs       # → set_logger/logger + 18/22
sed -n '294,306p' crates/gqep-executor/src/tool_runtime/batch.rs   # → debug! 发表点
sed -n '258,262p;303,315p;170,182p;434,446p' crates/chimera-cli/src/composition.rs
sed -n '244,262p' crates/gqep-executor/src/tool_runtime/chain.rs   # → dispatch_borrowed owned 升级
sed -n '37,46p' crates/gqep-executor/src/tool_runtime/dispatch.rs  # → BorrowedToolDispatcher
sed -n '63,72p' crates/gqep-executor/src/exec_tool_executor.rs     # → :68 to_string 分配点
sed -n '1,8p;33,38p;45,60p' crates/nexus-app-server/src/subagent_engine.rs   # → GATED-UNWIRED + new(engine: QuestEngine)
sed -n '1,8p' crates/nexus-hook/src/lib.rs + sed -n '93,104p' crates/nexus-hook/src/event_bridge.rs
sed -n '49p' crates/event-bus/tests/variant_count_test.rs（154）+ :25 doc_count_freeze（154）+ :36 baseline 尾注（152）
grep -c "^\[\[crate\]\]" scripts/coverage_per_crate_floor.toml   # → 6
sed -n '55p' scripts/await_guard_allowlist.txt   # → participant_client.rs:556
sed -n '19,31p' scripts/sync_offload_guard_freeze.txt | grep -c -v "^#"   # → 13
sed -n '80,87p' .gitignore   # → 五条豁免 + .trae/
sed -n '373,378p' crates/chimera-cli/src/action_orchestrator.rs   # → :375 spawn
sed -n '134,137p;384,396p' crates/chimera-cli/src/experience_loop.rs   # → SQLite 打开 + metrics dump
grep -n 各事件变体 crates/event-bus/src/types.rs   # → :514/:1601/:1621/:1639/:1776
sed -n '423p;240,247p' docs/architecture/adr_index.md   # → 「七份」矛盾 + 八行登记
sed -n '58,64p' crates/gqep-executor/tests/toolplan_capability_table.rs   # → :60 夹具
```

---

### 14.1 勘误处置汇总表（9 条全量）

| # | 勘误目标 | 处置 | 本轮验证结论 |
| --- | --- | --- | --- |
| 1 | X2 §4.1 陷阱④ + 程序步骤 3 + §6（pre_x3「不存在」断言） | **撤回陷阱④**；§5.4 步骤 3 改「核验既有 pre_x3 完整性后直接引用」；改前态锚改指该目录（mean 口径与 ADR-252 同源，为 Q1 提供改前基线直接证据） | **全部实证采纳**：4 项目录、mean=63,804,161.25ns/median=58,348,375ns、base==new、ADR-254:140 明文、mtime 09:44、tar 0 命中属实但推论不成立 |
| 2 | X2 N-1 真值现状（:437 → :435） | 采纳：ADR-252 登记行 = `adr_index.md:435` | 实证：:435 含 ADR-252 全文（「160ms 改前上界」逐字在）；:437 = ADR-253 |
| 3 | X2 F-08 出数点 A（:391-393 → :388） | 采纳：逐 crate 打印 = `coverage_gate.py:388`；打印形态 `[flag]` 前缀 | 实证：:388 = `print(f"  [{flag}] ...")`；:391 = 失败分支 |
| 4 | X2 §3.3 顺带段（:18-24 → :12-16） | 采纳 | 实证：:12-16 = 5 条示例；:18-24 = render_bench 说明 |
| 5 | X2 §3.1（PlanRunner::new :84-88 → :88-91） | 采纳 | 实证：:88-91 = PlanRunner::new 三参调用 |
| 6 | X2 §8 Q3（:3622 归类） | 采纳：:3622 = 注释字面量，与 :2684/:2794（assert! 消息）细分 | 实证：三处形态逐行确认 |
| 7 | W3 W4-9（I2 口径：83 键名 → 101 词条） | 采纳：§3.6 改为「词条集双向子串（check_perf_redlines.py:595），全集 101」；结论不受影响（候选键双口径均 NONE） | 实证：:317/:595-607 + 词条计数 101 |
| 8 | W5 §2 5.5 行（spec.rs plan 表行号） | **部分采纳**：勘误方向（行号现取）正确，但其替代值（:67-74/:75-83/:85 起）与本次实测（Auto :72-77 / Off :78-86 / NoContext :87-98）仍有偏差；终稿以本次 grep 实测为准；枚举 :33-39 勘误与实测精确吻合 | **矛盾显式登记**（§4.6） |
| 9 | W5 §3.3（use_current_thread :238-240 → :240） | **部分采纳**：:225-227 multi_thread 断言勘误与实测精确吻合；但「fn 定义实际在 :240」不成立——实测 fn 定义 :238-240，**W5 原引 :238-240 正确** | **矛盾显式登记**（§4.6） |

---

*本终稿依 ADR-234「证据先于断言、低报与高报同属错误」合成：9 条复核勘误全部并入并在被修正处标注〔已修正〕；输入材料矛盾处（勘误 8/9 行号二次偏差）显式列出并采用有据一方（本次 grep -n 实测），未静默取舍。各波实施如遇行号漂移，以现场现取为准并在波次报告登记。*

### 14.2 审读修订记录（2026-11-07 第二轮，34 条逐条处置索引）

> 结构与路径不变（仍为 §0~§14）；全部修订在正文以〔修订·审读〕标注；新增小节：§1.7（执行顺序图）、§2.4② 字段清单表、§2.10（事件路线回退清单收录）、§10.2 用法边界表、§7.3 F09-P2 施工行。

| # | 审读条目 | 处置 | 落点 |
| --- | --- | --- | --- |
| U1 | W4 收益表三处互斥 + :68 共用切换与 owned 零变化护栏冲突 | 收益表改四列自洽（借用 10→7 / 拥有 8→7）；裁决 = :68 为两路径共用改进、owned 臂预期 -1 分配（轻微改善）；护栏改「恶化 >5% 或预期外大幅变化」；ADR-249 登记义务 | §3.4（全表+结论 3）、§3.6 护栏测点 |
| U2 | §3.3④ owned 适配借用期错误 | 改为「先构造 Ref 局部再 move 进 async 块」可编译形态；两类型别名同型已实测（dispatch.rs:21-22 ≡ tool_dispatch.rs:116-117） | §3.3④ |
| U3 | W3-T3 六字段 vs 5 计数器 | BatchAttribution 六字段权威清单表（success_ratio_bp 为派生量**不设计数器**）；W3-T3 精确化 | §2.4② 表、§2.5 W3-T3 |
| U4 | revert 粒度两处不一致 | §12 L2 统一口径（最小语义单元；下限每波 1 / 上限 ≤8 工作项；各波具体粒度列出）；§4.12/§5.8 注明遵循 | §12、§4.12、§5.8 头注 |
| U5 | BUS_TICK_BATCH/XLOOP_PARALLEL 无对应物 | §4.12 L1 行删除两误植行（附删除原因），仅保留正文有定义的开关 | §4.12 |
| U6 | 5.2 push 清单 11 vs 12 | 实测精确清单：:153/:172/:196/:222/:234(条)/:244/:264/:274/:286(条)/:296/:306/:386(条) = 12 处；:222 误注订正 | §4.2 5.2 行 |
| U7 | 5.5 引用 §4.6 错位 | 改引 §4.10 | §4.2 5.5 行 |
| U8 | ToolRuntime::new 两处形态不一 | 统一双参签名（runtime.rs:69【实测】）；bench 与 W4-T8 同口径 | §3.6、§3.5 W4-T8 |
| U9 | F-09「随 P2」脱节 | §7.3 新增 F09-P2 施工行（五元组+TDD+独立 revert）；§9.2 F-09 行反向引用 | §7.3、§9.2 |
| U10 | N-1 前置与勘误后程序双轨 | 五元组前置改「核验既有 pre_x3 快照并装回作 base（§5.4 步骤 3-3.5）」 | §5.2 N-1 行 |
| U11 | W3-T9 正则宽窄不一 | 实测 ASSEMBLY_CALL 正则全文（check_composition_root.py:51-55）= 四类型；with_batch_sink 不命中 ⇒ 7 点不变成立；统一 §2.4⑤/§8.3 口径 | §2.4⑤、§2.5 W3-T9 |
| U12 | P1 ServerNotRegistered 二义 | 变体从 McpRouteErrorKind 移除（parse 纯函数不可达）；未注册 server 统一 execute 内联文案；ADR-256 D3 口径补全 | §6.3 枚举+澄清 1 |
| U13 | N-4 指代不明 | 首现处定义：会话存储批次债项编号，锚 composition.rs:427/:442 注释自陈；与 §5.2 N-4 显式区分 | §8.3、§9.2 U-08 |
| U14 | §13 G-xx 与门禁 ID 撞名 | 全表重编 OQ-01~OQ-27 + 命名空间声明（OQ=开放问题 / G=门禁，互不关联）；全文引用同步 | §13、§0-8、全文引用点 |
| U15 | 「附录 A」悬空 | W3.md 附录 A 事件路线回退清单完整收录为 §2.10（tmp/ 不入库 ⇒ 本稿为唯一载体） | §2.1、新增 §2.10 |
| U16 | §6.5 W1→P1 波次写错 | 改「P1 前/P1 后」 | §6.5 |
| U17 | mas「1-5 个」深度/个数混用 | 拆口径：深度 ≤5（chimera-mas INV）；每层个数口径未裁定（以 INV 现取为准） | §7.3、§7.8 D4 |
| U18 | §10.2 L4 边界含混 | 定义三类用法表（判性能=仅 W4 / 阈值登记=W2·W4·W5·X2 / 护栏停机门=W3·W5·X2）；X2 N-1 归类明确 | §10.2 |
| U19 | §3.3⑦ 等价锁语义不明 | 明确为新施工约束=既有单测不动（A5）+ 双形态一致性由 W4-T2 proptest 承担（无独立新施工项） | §3.3⑦ |
| U20 | I2 脚本 cand 口径未写明 | 钉死：cand = 候选键的 criterion **词条**（非键名），逐词条跑；脚本改多词条遍历；hnsw 案例双值示例 | §4.3 步骤 4 |
| M1 | 全局执行顺序缺失 | 新增 §1.7 依赖图（W5.0→W3→W4→W5→X2→P1→P2/P3‖→P4→W6；强约束 5 条 + 并行矩阵） | §1.7 |
| M2 | sink=Some 时 debug! 保留与否 | 裁决保留（A5 只约束 None 路径；不削减既有观测）；记入 ADR-248 决策 3 | §2.1-3、§2.4②、§2.8 决策 3 |
| M3 | BatchAttribution 清单缺失 | 同 U3（六字段表 + is_conserved 覆盖面） | §2.4② |
| M4 | 5.1 丢事件风险未评估 | 新增风险论证节（DEFAULT_CAPACITY=1024【实测 bus.rs:33】、broadcast Lagged 语义、8×120s 长动作占满场景）+ 缓解设计（try_acquire 优先/失败 recv 内同步执行）+ proptest 联动 + OQ 裁决点 | §4.4 |
| M5 | resolve_action_concurrency 期望列缺失 | 六行期望表（含 0→8 回退、非数字 warn 不 panic） | §4.4 表、§4.8 引用 |
| M6 | pre_x3 装回 base 动作缺失 | §5.4 增步骤 3.5（装回命令 + 逆备份）+ 步骤 4 轮转语义说明（首轮 change%=vs 改前、第二轮取值）；§5.7② 补前提 | §5.4、§5.7 |
| M7 | P1 ExecutionCompleted 发布点缺失 | 实测发布者在 quest-engine engine.rs:544/:553（非四环）；P1 零新增发布点、不虚构发布；五元组/§9.3/§6.3 三处澄清 | §6.2、§6.3 澄清 2、§9.3 |
| M8 | P1「路由表 3 条」前提未声明 | 实测证明不依赖 N-6（capabilities=包装前原 delegate runtime.rs:161【实测】）；机制论证入 §3.7 尾 + §6.3 澄清 3 | §3.7、§6.3 |
| M9 | P2 join_next 锁形态缺失 | 实测 join_next(&mut self)（subagent_engine.rs:112-114，JoinSet 语义）；「锁外驱动」撤回；tokio Mutex 跨 await 语义澄清；单消费者驱动形态 + 代价登记 | §7.3 D2、§7.8 D2 |
| M10 | 冻结表新漂移处置义务 | 实测三表对波次目标文件零条目（插行不产生新 stale）；义务仍立=触及锚定文件复跑三门 + 随波 re-key；入 §10.1 通用列 | §4.11、§10.1 |
| M11 | P4 回归锁路径基准缺失 | CARGO_MANIFEST_DIR 起算 + ../../crates/ 相对拼接；先例 tool_runtime_wiring.rs:223 / doctor_async_probe.rs:30【实测】 | §9.4 |
| M12 | c8/c1 判据口径未指明 | 钉死 = mean 比值（与登记同源）；median 抄录不判定；不符时负结果路径 + OQ-14 复裁 | §5.7③ |
| M13 | 上游原始方案不可达 | 头部注记：仅存作者本机路径、不入库；相关裁决已被全景方案吸收；回查以全景方案+本稿为准 | 文档头 |
| M14 | 5.2 join 分区归属不完整 | 三条顺序约束成员落位（card_bus 链+router/feedback 链留 join 前串行主体）；「串行相对于谁」定义 | §4.5 |


---

*本终稿依 ADR-234「证据先于断言、低报与高报同属错误」合成（第一轮：9 条复核勘误并入，标注〔已修正〕；第二轮：34 条审读意见逐条处置，标注〔修订·审读〕，新增施工级验证 10 项均以【实测·本轮】给出命令与输出）。输入材料矛盾处均显式列出并采用有据一方，未静默取舍。各波实施如遇行号漂移，以现场现取为准并在波次报告登记。*
