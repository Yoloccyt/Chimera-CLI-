# ADR-188：退役 crate 的事件所有权冻结与零装配 optional 边处置

- **状态**：🟡 Proposed（2026-09-23 登记，等待治理签署；本 ADR **不授权任何删除动作**）
- **编号选取（遵循 `ADR-XXX-inner-ring-bug-fix.md` 头部规则的实测记录）**：取号前对
  `docs/ scripts/ *.md .claude .qoder .trae` 全域 grep ⇒ **186 归 M13 decb-scc 重路由批次预留**
  （`CODE_WIKI.md:718` 注记；同族撞号事故已登记一次，见 `docs/reports/gate-trust-B0-2026-09-20.md:791`）、
  **187 归 Q3 内环界 α′ 裁决草案**（`gate_manifest.toml` G-39 `spec_ref`）、188–190 **零命中**
  ⇒ 本 ADR 落 **188**。本文件初稿曾命名 `ADR-186-…`，系按"磁盘最大编号 + 1"取号而未查预留注记，
  与上述两条同时撞号，已就地改名（`mv`，无历史丢失）。
- **关联**：ADR-160（可达性棘轮）· ADR-172（model-router 退役）· ADR-182（acb-governor 退役）·
  ADR-054（三环重组）· ADR-065（mca-gateway 承接成本面）
- **动机文档**：`CHIMERA_架构减法与职责重组设计方案_v2.30.md` §5（A 组处置表）与 §6（批次路线图 B3/B4）
- **证据链**：`docs/reports/SAB-B0-2-impl-maturity-gate-2026-09-23.md` §17–§20

---

## 1. 背景与实测事实（全部当轮现取，非引用旧文档）

### 1.1 三个候选退役 crate 在装配面上已是"零入度"

`cargo metadata --offline --no-deps` 的**非 optional 内部边**集合中：

```
mtpe-executor : non-optional production consumers = 0
chtc-bridge   : non-optional production consumers = 0
ssra-fusion   : non-optional production consumers = 0
```

唯一入边是 `crates/chimera-cli/Cargo.toml:21-24` 的 `optional = true` 声明，挂在
`r2_island_repayment` 特性上（不在 `default`）：

```toml
r2_island_repayment = [
    "dep:mtpe-executor", "dep:ssra-fusion", "dep:sesa-router",
    "dep:csn-substitutor", "dep:chtc-bridge", "dep:mas-sched",
    "dep:nexus-hook"
]
```

`check_gated_wiring_substance.py`（G-56/G-57）判其中 **7 条为 DECORATIVE**：特性被声明、
但 crate 自身零 `cfg` 引用 ⇒ "特性开与不开，装配图与二进制都不变"。
`check_declared_dep_usage.py`（G-31）同时把这 7 条报为"声明未用"。

### 1.2 六个 `NexusEvent` 变体的**唯一生产构造点**在待退 crate 内

| 变体 | 定级（`crates/event-bus/src/registry.rs`） | 生产构造点 |
| --- | --- | --- |
| `PredictionMade` | `Normal, Execution`（:174） | `mtpe-executor/src/predictor.rs:202,597` |
| `PredictionStatsReported` | `Normal, Execution`（:175） | `mtpe-executor/src/predictor.rs:250,658` |
| `PredictionRolledBack` | `Normal, Execution`（:176） | `mtpe-executor/src/fallback.rs:68,128` |
| `SsraFusionCompleted` | `Normal, Execution`（:204） | `ssra-fusion/src/fusion/engine.rs:162` |
| `ChtcAdapterStatus` | `Normal, System`（:230） | `chtc-bridge/src/bridge.rs:173,539` |
| `ChtcToolCallReceived` | `Normal, System`（:202） | `chtc-bridge/src/bridge.rs:118,280` |

**关键：六个全为 `Normal`，无一是 Critical** ⇒ 不触及 `is_critical_mpsc_event()` 的 mpsc 旁路
红线（§6.2），事件送达保证面不受影响。

其余同名命中已逐条定性（避免把消费点当生产者）：
`event-bus/src/topic.rs:232` 与 `mcp-mesh/src/mesh.rs:1190` 在 `#[test]` 内、
`event-bus/src/membrane.rs:442,446` 是 match 臂、`mcp-mesh/src/mesh.rs:701` 是 `if let` 消费、
`chimera-tui/src/data/mod.rs:750` 位于该文件唯一 `#[cfg(test)] mod tests`（起于 `:72`，全文 1428 行）内
⇒ **均为测试夹具或消费，不构成第二生产者**。

### 1.3 测试面分档（决定"不删真实行为测试"约束落在谁身上）

- `chtc-bridge`：断言对象即 mock 自洽值（`adapters/vscode.rs:3 MOCK-ONLY`、`:36 模拟执行:返回成功结果`）
  ⇒ `MOCK-SELFCONSISTENT`
- `mtpe-executor`：`tests/speedup.rs:39,79,113` 的加速比建立在自家假延迟上 ⇒ `PSEUDO-CONSTRUCT`；
  `gea-activator/tests/e2e.rs:161` 的 `success_rate > 0.85` 由测试自注入 18 true/2 false ⇒ 自证式；
  但 `gea-activator/tests/e2e.rs:310-311` 两条 `assert_eq!` 是对伪实现**确定性形状**的真实断言，须逐条登记
- `ssra-fusion`：`tests/e2e` 即 `week6_setup.rs:296-302` 断言 `selected_count == 1`、
  `|confidence - 0.8| < 1e-5` 并断言 `SsraFusionCompleted` 事件 ⇒ **真实行为测试，必须先迁移**

---

## 2. 决策

### 决策 1：事件契约一律保留，登记为 `UNOWNED-PRODUCER`
按本轮四条自我约束之"不删事件契约"：退役后 `NexusEvent` 的 6 个变体、其 registry 定级行、
topic 归类与所有消费点**全部原样保留**。变化的只是 **本仓内的 emit 面**——退役后本仓库
不再有任何生产代码构造这 6 个变体。

> ⚠ 措辞边界（2026-09-23 复核后收紧）：`NexusEvent` 带
> `#[derive(Serialize, Deserialize)] #[serde(tag = "type", content = "data")]`（`types.rs:26-28`），
> 因此**外部进程仍可经线帧反序列化把事件物化进本进程**。本决策说的是"本地无人可发"，
> 不是"该事件在全系统内不可达"。据此不得把"退役后零构造"当作删除契约的理由——
> 它可能仍是跨进程入站契约。

#### 1-a 唯一生产者取证（2026-09-23 实测，判据与命令见进度报告 §22）

每个变体**当前恰好有 1 处生产态构造点**（`#[cfg(test)]` 之前的 `NexusEvent::V { … metadata: … }`），
且构造者就是拟退役 crate 自身 ⇒ "退役后零投递"成立：

| 变体 | 唯一生产态构造点 | 退役后仍留在盘上的**死消费面** |
|---|---|---|
| `PredictionMade` | `crates/mtpe-executor/src/predictor.rs:202` | 无（同 crate 的匹配臂在测试内，随 crate 同删） |
| `PredictionStatsReported` | `crates/mtpe-executor/src/predictor.rs:250` | `crates/event-bus/src/membrane.rs:442` 路由臂 |
| `PredictionRolledBack` | `crates/mtpe-executor/src/fallback.rs:68` | 无 |
| `ChtcToolCallReceived` | `crates/chtc-bridge/src/bridge.rs:118` | `crates/mcp-mesh/src/mesh.rs:701` 的 `if let` 分支 |
| `ChtcAdapterStatus` | `crates/chtc-bridge/src/bridge.rs:173` | `crates/chimera-tui/src/data/sync.rs:735` 面板数据臂 + `membrane.rs:446` |
| `SsraFusionCompleted` | `crates/ssra-fusion/src/fusion/engine.rs:162` | 无 |

> 四处死消费面**全部实测为生产态**（`sync.rs:735` < 该文件首个 `#[cfg(test)]` `:1199`；
> `mesh.rs:701` < `:1007`；`membrane.rs:442/446` < `:588`）。

#### 1-b 本决策的补充要求（原稿缺此项，本轮补）

"保留事件契约"**不等于**"消费面可以不管"。上表第三列是**编译通过但永不命中**的分支——
正是本方案 §1.2 要消灭的"账面与事实脱钩"族的一个新实例。故追加约束：

1. B3 的每个退役 commit **必须同时处置**该变体的死消费面：或删除该臂，或就地标注
   `// NOT-REACHABLE since <crate> 退役（ADR-188 决策 1）`并保留其测试；两者都不做即视为该批未收口。
2. `chimera-tui` 的 CHTC 面板（`panels/chtc.rs` 自述"数据来源：L10 chtc-bridge 通过
   `NexusEvent::ChtcAdapterStatus` 发布"）在 `chtc-bridge` 退役后**数据源归零** ⇒
   退役前必须先给该面板另立数据源或显式下线，否则面板由"真实统计"退回"恒空"——
   这与 ADR-183 把 `InjectionStrategy` 下线时所走的判据同类。

### 1-c 零 emit 面必须分两栏（由 G-67 `scripts/check_event_ownership.py` 量化）

本 ADR 决策 1 表列的 6 枚属于 **"拟退役将致零 emit"**；实测还有一批 **"今天已经零 emit"** 的契约，
两栏不得混同，否则会把"上一批退役留下的债"记到"这一批退役的账上"：

| 栏 | 实测数量 | 清单所在 | 已核成因 |
|---|---|---|---|
| 既有零 emit（本仓生产作用域内已无构造点） | **15**（全库 146 = 131 有 + 15 无；2026-09-23 修掉门自身的 3 枚假阳性后的现值，首版曾误报 18） | `scripts/event_ownership_baseline.txt` 逐行证据；逐枚裁定见 `docs/reports/SAB-B0-2-impl-maturity-gate-2026-09-23.md §24.3` | `ModelRouteSelected`（`model-router` 已删，ADR-172）、`DpoPairGenerated`（`auto-dpo` 随批次 A `38c72e6` 删除）；另 2 枚是**刻意处置**（`BenchmarkMetricsCollected` 依 ADR-159 决策 4 预留禁删、`VoteCastRequested` 依 FC-A 诚实降级）；**其余 10 枚属未接线债，应发方 file:line 已逐枚实测** |
| 退役将致零 emit | 6 | 本 ADR §1-a | 唯一构造者即拟退役 crate 自身（构造点行号已逐枚实测） |

**处置要求**：上述 18 枚每枚须三选一裁定 —— ① 在本仓接一个真实 emit 点；② 认定为 **wire-only 入站契约**
（因 `NexusEvent` 派生 `Deserialize`，外部进程仍可物化，故"零本地构造"不等于"无用"）；③ 另立 ADR 退役该契约。
本 ADR **不代为裁定**，也**不因零 emit 而授权删除**——四条自我约束之"不删事件契约"在此仍然生效。

### 决策 2：退役顺序 = `mtpe-executor` → `chtc-bridge` → `ssra-fusion`，且各带前置

| 序 | crate | 开工前置（缺一不可） |
| --- | --- | --- |
| 1 | `mtpe-executor` | ① 本 ADR 签署；② `scripts/ignored_test_inventory_freeze.txt:44,72,73,74` 四条 slo-daily 条目随测试文件同批注销（:44 属 gea-activator 的 mtpe 延迟测试）；③ `gea-activator/tests/e2e.rs:310-311` 断言按 §1.3 分类登记后进 `scripts/test_retirement_ledger.txt` |
| 2 | `chtc-bridge` | ① `protocol.rs`（414 行，7 个 `from_*_format`）的落点在 `mcp-mesh` **先建后删**——实测 `mcp-mesh/src` 无等价实现（`grep -rni vscode crates/mcp-mesh/src` = 2 命中且均为事件消费）；② `chimera-tui` 的 `PanelId::Chtc` 面板语义处置随本 ADR 签署一并定 |
| 3 | `ssra-fusion` | ① `compute_confidence`（`fusion/engine.rs:229`）三策略公式与 `week6_setup.rs:296-302` 断言**先迁移**到落点；② `scripts/topk_sortby_freeze.txt:24` 的 `lru_evict` DEFER 项要求先补 id tie-break，否则迁移即行为变更；③ Top-K 本体**无需再迁**（`engine.rs:135-138` 已直接调用 `nexus_contracts::util::xts_top_k_by`，定义在 `util.rs:73`） |

### 决策 3：七条 `dep:` 幽灵边的处置分两类
- `nexus-hook`：既定动作是**真接线**（B4，`ADR-146` 生命周期 Hook 尚无宿主），不得以"删边"冒充减法；
- 其余 6 条：随各自 crate 的 B3/B4 动作**同 commit** 删除 `[dependencies]` 声明 + 对应 `dep:` 特性项 +
  `dep_edge_freeze.txt` / `crate_reachability_freeze.txt` 相关行（棘轮只减不增，删边是允许方向）。
  ⚠ 特性项收缩会改变 `ci.yml` 的 `cargo-hack --each-feature` 覆盖集，故必须在 crate 真要退的同批做，
  不可提前单删。

### 决策 4：`csn-substitutor` 降级链主链归属**不在本 ADR 内裁定**
`chimera-mas/src/stability.rs:225` 是名义持有者、`csn/src/degradation_chain.rs` 是实装者；
直接接线会造出同语义双发布者。该项按 §16 第 5 条留待治理裁决，本 ADR 只登记冲突。

### 决策 5：撤回一条未证主张，改为能力缺口清单
设计文档 §5 原写"`mtpe-executor` 的等价闭环已在 `pvl-layer/src/verifier.rs:237` →
`experience_loop.rs:345` 跑通"。实测 `crates/pvl-layer/src` 与 `experience_loop.rs` 两侧
`MTPE`/`mtpe` 两种拼写**0 命中**；`pvl-layer/src/dynamic_depth.rs:112` 的 `select_depth`
是验证深度而非 N-token 预测 ⇒ **该主张撤回，不作为退役依据**。
替代登记：若"多步预测"仍是需要的能力，它是一个**待建能力**（不是已存在的等价物），
应在 `mtpe-executor` 退役前明确由谁承接，否则退役即能力净损失。

---

## 3. 后果

**必须不变**（可机检）：`NexusEvent` 变体数、registry 定级表、`is_critical_mpsc_event()` 清单、
默认二进制构成、启动耗时、Ω₁~Ω₁₁、依赖方向铁律（`emit_architecture_views.py --check` 现值
`edges=138 upward=0`）。

**会变（且是本意）**：装配图节点 `41 → 38`（B3 三 crate 退完后）；`chimera-cli` 特性面收缩 6 个
`dep:` 项；`criterion` bench 与 `#[ignore]` slo-daily 条目随之下降；`cargo test --workspace` 墙钟下降。

**诚实边界**：本批减法**不减小二进制、不加快启动**（被减者本就是 feature-off 的 optional 边，
从不在默认二进制里）。收益面只有装配图节点数、测试/bench 面与测试墙钟三项。
6 个事件失去生产者属**观测面收窄**，须接受"这些事件自此不再出现于任何运行轨迹"。

---

## 4. 复验锚（每条都能被现成命令重跑）

```bash
# 零入度事实
python3 scripts/check_contract_consumers_parity.py            # G-63: 两法边集一致
python3 scripts/check_crate_reachability.py                   # G-?: reachable=32 / GATED=9 / islands=0
# 幽灵边与装配面
python3 scripts/check_gated_wiring_substance.py               # G-57: 7 DECORATIVE
python3 scripts/check_declared_dep_usage.py                   # G-31: 7 条声明未用
python3 scripts/check_dep_edge_freeze.py                      # G-33: 146 vs 138
# 铁律与派生视图
python3 scripts/emit_architecture_views.py --check            # G-65: upward=0
# 事件面
grep -n "PredictionMade\|PredictionStatsReported\|PredictionRolledBack\|SsraFusionCompleted\|ChtcAdapterStatus\|ChtcToolCallReceived" crates/event-bus/src/registry.rs
```

签署后，B3 各步在同一 commit 内附"删除前后 `cargo tree -e normal -p chimera-cli` 逐字节等值"
与 `run_gate_manifest.py` 全量 light 档对照（既有红基线 = 14 条，见进度报告 §20）。

## 5. 追加更正（v3.5 C-48，2026-10-10）：决策 3 第一条的「真接线」主张**撤回**

> 本节为追加注记，决策 3 原文（:141）逐字保留不改写。授权来由：v3.5 Spec《需你裁决事项》
> 第 8 条对 C-48 的**默认值就是「撤主张」**（与「接线」二选一），且用户已批准该 Spec。

**被撤回的主张**：决策 3 写「`nexus-hook`：既定动作是**真接线**（B4，ADR-146 生命周期 Hook 尚无宿主）」。
自签署至本注记日，该动作**未发生**；把它继续当作"既定动作"留在决策文本里，会误导下一个
读者以为接线已在排期（终局原则 3：指不到生产构造点的主张就不得称"已/将接线"）。

**现取证据（三条独立路径，均 A 级）**：
1. `nexus_hook::` / `use nexus_hook` 在**除自身外**的 `crates/*/src/**/*.rs` 全库 **0 命中**
   （逐 crate 递归检索；含 `chimera-cli` 组合根——即无宿主，也无调用方）。
2. `scripts/gated_wiring_baseline.txt:15` 在册 `nexus-hook|DECORATIVE`（G-57 实跑 rc=0）。
3. `scripts/dep_edge_freeze.txt:183` 只有 `chimera-cli|nexus-hook` 这条 **optional GATED** 声明边；
   即"已在装配面声明"而非"已接进启动链"（终局原则 4 / 风险 R-5 的"拿 optional 当已接线"族）。

**附带登记一处台账内部自相矛盾**（不属本 ADR 裁决面，但本撤回必须提及，否则下一个读者仍会踩）：
`scripts/crate_reachability_freeze.txt` 同一注释块内，:71 写「batch-P3: …/nexus-hook **wired**」，
:73 又写「mas-sched/nexus-hook **remain GATED**」。二者对同一对象给出相反结论；以现取为准，
:71 的 "wired" 只能读作"已在 Cargo.toml 声明为 optional 依赖"，不得读作"已接线"。已同批在该文件
补订正注记（历史行不改写）。

**为何不反过来做（即为何不"为了兑现主张而接线"）**：接线会新增一条真实生产边并引入运行期行为
（每次工具调用都要过 hook 执行器与 `ProcessFence`），而宿主选型本身尚未定案（`chimera-tui` / `nexus-app-server` /
`quest-engine` 三处生命周期语义不等价），且 `PreToolUse` 非零退出码可拒否工具调用属于**安全行为变更**。
按本仓纪律，不得为了让门或主张"有意义"而造无裁决的构造点（同批已有先例：拒绝为 `harness_dimensions`
旗标造消费者，总账 C-60）。

**若将来要接线，前置清单（缺一不可）**：① 新 ADR（建议自 ADR-202 起取号并先查 `adr_index.md`）定宿主与
生命周期语义，含 `PreToolUse` 拒否与超时熔断对用户可见行为的定义；② 同批登记 `dep_edge_freeze.txt`
（新增非 optional 生产边）与 `gated_wiring_baseline.txt`（DECORATIVE 行注销，只减方向）；
③ `crate_reachability_freeze.txt` 的 GATED 计数随批更新；④ 新事件/面板一律不动（本项不需要新变体）。

**本撤回的实际效果**：不改任何代码与台账行（零行为变更）；只把"既定动作 = 真接线"降为
"未排期规划项，前置见上清单"，并同步 CODE_WIKI 的 nexus-hook 装配态行与总账 C-48/§12。
