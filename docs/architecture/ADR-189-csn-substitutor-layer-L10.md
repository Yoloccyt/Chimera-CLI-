# ADR-189: csn-substitutor 层号归 L10 Interface（层图与门禁账面收口，附补偿控制实测）

> **状态**: Accepted (2026-09-24)
> **决策日期**: 2026-09-24
> **关联 ADR**: ADR-033(L0 契约层) / ADR-065/068(MCA 能力亲和，csn 消费 `CapabilitySet`) / ADR-160(生产可达性棘轮) / ADR-185(例外边收编范式)
> **关联文档**: `docs/architecture/CODE_WIKI.md` §2.1 层图 + §3.11 孤岛表 + 目录树 + E08 映射；`CHIMERA_架构减法与职责重组设计方案_v2.30.md` §2.4；`docs/reports/SAB-B0-2-impl-maturity-gate-2026-09-23.md` §59
> **权威源**: 本 ADR 记录 `csn-substitutor` 层归属的裁决、其安全代价的**实测结论**，以及补偿控制的真实当前状态

---

## 1. 背景

`csn-substitutor`（能力替代网络 / CSN 降级链）长期存在**唯一一处**层图与依赖门的分歧，且早已登记（非新发现）：

* 文档侧五处一致记 **L10 Interface**：`CODE_WIKI.md §2.1` 层图、`§3.11` 孤岛表、仓库目录树、E08 专家映射，以及**该 crate 自己 `Cargo.toml:11` 的注释**「WHY L10 → L0 合规(依赖铁律 §2.2)」；
* 机器侧三处记 **L7 Execution**：`check_dependency_rules.sh` 的 `layer_of()` 臂、`.ps1` 的 `$layerMap`、由二者派生的 `src/lib.rs` 契约头 `LAYER: L7`。

`CODE_WIKI.md:111-120` 把它写成"待治理裁决"，并给出两个代价：① 抬层"须 ADR"；② 抬层后「门按 L7 放行」变「按 L10 放行」，**会失去对未来 L8/L9 向上边的拦截**。

## 2. 选项与判据

| 选项 | 内容 | 判定 |
|---|---|---|
| A | 改 4 处散文归 **L7**（零铁律变化，无 ADR 需求） | **否决**：与 `Cargo.toml:11` 的 crate 自述意图相反；与 mcp-mesh 先例相反（`.sh` 注释自陈 `mcp-mesh 2026-09-02 T10: 对齐文档 L10 归属(原脚本误置 L1)`）；CSN 是跨进程能力替代网络，语义即接口面 |
| B | 改 `layer_of()`/`$layerMap` 归 **L10**（放松铁律，须 ADR） | **采纳**，但代价 ② 必须先实测再写进结论——不许照抄文档推断 |

## 3. 代价 ② 的实测结论：**拦截没有丢，但换了一道门，且那道门当前是红的**

在同一棵树上植入一条 `csn-substitutor -> parliament`（L10→L8）探针依赖，分别跑两道门，随后按 sha256 核验复原：

| 门 | 判据 | 实测 |
|---|---|---|
| `check_dependency_rules.sh`（依赖铁律） | 只 flag `dep_layer > own_layer` 的向上边 | **0 提及该边**（L10→L8 视为合法向下）⇒ 文档的推断**在这道门上成立** |
| `scripts/check_dep_edge_freeze.py`（G-33，内部装配边 only-decrease 棘轮） | 「树里有、冻结册里没有 ⇒ FAIL」 | **rc=1** 并点名 `[NEW] csn-substitutor -> parliament` ⇒ 拦截由这道门**接管**，文档"失去拦截"的说法**不完整**（它只看了层号判据，没看边棘轮） |

**必须同时记下的边界**：G-33 自身在工作树当前是**红**的（`8 NEW internal dependency edge(s)`，属本批已登记的非绿门之一），且 `csn-substitutor` 的三条边（`event-bus`/`nexus-contracts`/`nexus-core`）已在冻结册内。因此本 ADR 的安全论证是"**机制上仍被拦**"，不是"当前全绿"。补偿控制的有效性以植入实验为证，不以门的 rc 为证。

## 4. 决策与执行面

采纳 B。层号改动**方向上是惰性的**：`csn-substitutor` 的非 optional 内部依赖只有 `nexus-contracts`(L0)、`event-bus`(L1)、`nexus-core`(L1)，抬层不放行任何现存边。

同一次改动的完整落点（一处层号 ⇒ 七面，缺一即门红）：

1. `scripts/check_dependency_rules.sh` L7 臂摘除、L10 臂加入并附裁决注释；
2. `scripts/check_dependency_rules.ps1` `$layerMap` 同步；
3. `crates/csn-substitutor/src/lib.rs:2` 契约头 `LAYER: L7 → L10`（**不可独立改**：`crate_contract.py`(G-58/G-59) 的 LAYER 字段按 `layer_of()` 派生再比对）；
4. 根 `Cargo.toml` 依赖块行内标注 `# L10 Interface`（新落码的 Check D 会把分组注释与 `layer_of()` 对拍，不标即红）；
5–8. `docs/architecture/views/` **四份**派生件（`layers.txt`/`domains.txt`/`dependency.dot`/`layer_contracts.txt`）经 `emit_architecture_views.py --write` 重算——`--check` 不比对逐字内容，**只有 `--verify`(G-70) 会判红**；
9. `CODE_WIKI.md:111` 分歧注记改为已闭合。

## 5. 验证（全部实跑，2026-09-24）

| 项 | 结果 |
|---|---|
| `check_layer_map_parity.py`(G-45/G-46，含新 Check D) | `--selftest` **11/11 PASS**；真树 `[OK] sh case=41 sh list=41 ps1=41 members=41 dep groupings=41`，rc=0 |
| `crate_contract.py --check`(G-59) | rc=0，`41/41 contract blocks valid` |
| `emit_architecture_views.py --verify`(G-70) | 重算前 **rc=1**（4 份不一致）→ `--write` → 重算后 **rc=0**；视图节点标签现为 `csn-substitutor\nL10` |
| `check_dependency_rules.sh` | rc=1，但唯一命中仍是既有 `[GAP-A] gsoe-evolution -> decay-engine`（内环边界待裁决项，见 `docs/reports/Q3-adjudication-inner-ring-gate-2026-09-21.md`）⇒ **本改动零新增 gap** |
| `check_dep_edge_freeze.py`(G-33) | 植入期 9 条 NEW，复原后回到 **8** 条 ⇒ 本改动**不新增**冻结册债 |
| 行为回归 `cargo test -p csn-substitutor --offline --jobs 1` | **136 passed / 0 failed**（另 2 ignored），rc=0 ⇒ 现有功能不受影响 |

## 6. 撤销条件

若 G-33 的 8 条未登记边被清理为"直接放行"（而非登记或改走 EventBus），补偿控制即失效，本 ADR 的"拦截未丢"结论随之失效 ⇒ 届时须在 `.sh`/`.ps1` 重新引入对 `csn-substitutor` 的显式向上边检查，或把该 crate 降回 L7 并同步 4 处散文（回到本文 §2 选项 A）。
