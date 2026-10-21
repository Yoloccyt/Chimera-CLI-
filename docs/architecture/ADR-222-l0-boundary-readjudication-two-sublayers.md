# ADR-222: L0 边界重裁 —— L0-A 纯契约（1+6）+ L0-B 核心基础面双层，部分 supersede ADR-212 D1/D2 与 ADR-049 D1

> **Status**: ✅ Accepted (2026-10-19，业主批准《L0 边界重裁与双层重构实施计划》即授权)
> **决策日期**: 2026-10-19
> **关联 ADR**: ADR-033（L0 建立 + 2026-10-09 追加节「受控纯逻辑」）/ ADR-049 决策 1（**部分 supersede**）/ ADR-187（Check A 退役）/ ADR-193（架构减法，**方向反转已登记**）/ ADR-195（GPU 否决，继续有效）/ ADR-212（**D1/D2 部分 supersede**，D3/D5 继承）/ ADR-213~221（门纪律与台账全部继承）
> **关联交付**: `docs/reports/L0-boundary-readjudication_2026-10-19.md`（事实基线 / 冲突台账 / 十维 / 成本对照）
> **权威源**: 代码实测（`Cargo.toml`、`scripts/check_dependency_rules.{sh,ps1}`、`scripts/check_layer_map_parity.py`、`crates/nexus-contracts/src/**`、`crates/nexus-core/src/compute/**`）；两份设计文档仅作意图参照
> **编号合法性**: 落盘前现取 ADR 物理文件 **117** / 主编号 max **221**；`grep -rn "ADR-222" docs/ agents.md scripts/` **0 命中**（无预留、无并发占用）⇒ 222 空闲可用。门号同理：`gate_manifest.toml` 现取 max **G-124**，G-125/G-126 无占用。

---

## 1. 背景

L0 已完成七轮收口（ADR-212~221），边界被 ADR-212 决策 1/2 裁为「仅 `nexus-contracts` 单 crate」并否决了两种文档版本：

- **十六源手册 v3.0 §8.1**：L0 = `nexus-core` + `nexus-telemetry`（含 `GpuComputePool` / 18 内核 / 144 变体 / 17 Critical）；
- **v3.4.0 §5**：L0 = `nexus-contracts` + 6 个 `*-contracts` 兄弟 crate（1+6=7）。

2026-10-19 业主明确**重开边界裁决**，并要求两条路径同时落地。本轮取证确认：两份文档的**成员意图**可采纳，但其**计数与 GPU 面不可采纳**（152/20/16、ADR-195 否决、`nexus-telemetry` 不存在、6 crate 无名单）；同时实测到三个硬约束：`nexus-core` 携带 tokio/rayon/rand 且依赖 L1 `event-bus`、遥测模块与 `KindCounters` 构成双向依赖、6 张台账的键是 crate 相对路径。

因此本 ADR 的裁决不是「照文档施工」，而是：**把 L0 定义为两个亚层、用兼容手段把爆炸半径压到 34 个消费 crate 零改动、把两台拆分各自断环/冻结键**。

---

## 2. 决策

### D1：L0 = L0-A（纯契约）+ L0-B（核心基础面）；**层号不变、层名分离**

- **L0-A（机器层号 0）**：`nexus-contracts`（umbrella）+ 6 个 sibling（D2），共 7 crate。
- **L0-B（机器层号 1）**：`nexus-core`、`event-bus`、`nexus-telemetry`（D3 新建），共 3 crate。
- **不重编号 L2-L10**。理由：116+ 份历史 ADR 与七轮报告全部以「L4 seccore / L9 quest-engine」等层号引用；字面重编号会让历史证据链整体失真，违反本仓「只加注不改写」纪律；且 6 源层表 + G-45/46 需全量重算而无任何机器收益。
- 机器表达：`sublayer_of()`（`.sh`/`.ps1` 双源）把 0 映射 `0A`、1 映射 `0B`；`layer_of()` 与 `layered_crates` 不动。新增 G-125/126 守护 L0-R1/R2（见 D3）。
- **明确否决**：把 L0-B 改成数字 0、L2-L10 顺移的「字面重编号」——见 D5 反方案 R-B。

### D2：1+6 拆分边界（文档未给，本 ADR 补齐）

以**实测模块依赖 DAG** 为唯一依据（组间边仅 5 条，全部向下）：

| # | crate | 承载 | 依赖（L0-A 内） |
|---|---|---|---|
| 1 | `nexus-contracts`（umbrella + facade） | `spec/`、`vector_math/`、`protocol/`、`testutil/` | 全部 6 sibling（spec→core+sparse） |
| 2 | `nexus-contracts-core` | `core_domain/` | 无 |
| 3 | `nexus-contracts-sparse` | `sparse/` | 无 |
| 4 | `nexus-contracts-safety` | `safety/` | 无 |
| 5 | `nexus-contracts-seams` | `seams/` | 无 |
| 6 | `nexus-contracts-mca` | `mca/` | 无 |
| 7 | `nexus-contracts-memory` | `memory/` | core、mca、seams |

- **语义分工**：6 个 sibling 是「领域契约」；umbrella 保留「平台与算法契约 + 外层协议 + 测试工具 + 聚合 facade」。
- **命名**：`nexus-contracts-<domain>`（lib `nexus_contracts_<domain>`），避免与 `nexus-core` 混淆。
- **硬阻断 N-4**：`core_domain/checkpoint.rs` 的 `pub(crate) fn is_schema_version_unset` 被 `memory/token_evidence.rs` 跨模块引用，拆分后必须升 `pub fn`。这是本 crate 自建立以来第 3 个 `pub(crate)` 项的可见性上移，须与 G-105 台账同批（254→255）并在本 ADR 留痕。
- **禁止扁平化**：每个 sibling 内部保留原组目录（`src/core_domain/...`），使 G-105/G-113/G-115/G-119 的键逐字不变。
- **明确否决**：文档「6 个 `*-contracts`」未给名称与边界，不得声称「照 v3.4.0 §5 实施」；本表是新设计，责任在本 ADR。

### D3：G-99 辖区由「L0」改称「L0-A」，判据零变更；L0-B 另立较宽门

- `check_l0_purity.py` 的扫描根由单个 `nexus-contracts` 改为 7 个 L0-A crate；依赖白名单仍 `{serde, chrono, uuid, thiserror}`；禁用符号/类别/豁免机制**逐条不变**。
- 新增 `check_l0_sublayer.py` + G-125/G-126：
  - L0-R1：L0-A 出边指向 L0-B ⇒ `[PURITY-LEAK]` rc=1；
  - L0-R2：L0-B 出边指向 layer ≥2 ⇒ `[UPWARD]` rc=1；
  - 不可判定（未知 crate / 两表不一致）⇒ rc=2。
- **诚实边界**：L0-B **不是纯契约**。它允许 tokio/rayon/rand、env 读取、事件上报；本 ADR 不把 G-99 的判据套到 L0-B 上，也不声称 L0-B「零依赖」。若未来要把 L0-B 收窄，须另立 ADR。

### D4：兼容手段两条，34 个消费 crate 零改动

1. **umbrella facade**：`nexus-contracts/src/lib.rs` 以 `pub use nexus_contracts_<domain>::<group>;` + `pub use crate::<group>::<leaf>;` 保持两条旧路径；
2. **保留组目录**：sibling 内部路径与拆分前一致，冻结 4 张台账的键；只改 G-107（正则接受 `pub use crate::`）与 G-123（`declared_in=<crate>:<module>`）两张。**【执行后注 2026-10-20】两个「只改」最终均未发生**：G-107 只加了「组绑定不计为叶子」的识别规则而未改键格式（偏差 3），G-123 主动保留 crate-相对键而不重出 75 行（偏差表「实测不需要」第 6 项）——原因是保留组目录已使模块路径在 7 个 root 内唯一，改键只造漂移不造安全。**下轮不得把本句当成待补项去“补做”重键**。

**验收方式**：拆分前后 `cargo check --workspace --all-targets` 均 rc=0，且 34 个消费 crate 的 `Cargo.toml` 与源码**零 diff**（`git diff -- 'crates/*/src/**'` 排除 L0 自身与 nexus-core/telemetry 改动面）。

### D5：反方案与否决理由

| 反方案 | 内容 | 否决理由 |
|---|---|---|
| R-A | 只做 L0-A/L0-B 语义标签 + 登记表，不拆 crate、不建 telemetry | 满足不了业主选定的两条路径；且「标签无牙」正是本仓反复治理的「承诺在散文、判据为零」 |
| R-B | 字面重编号：L0-B 改数字 0，L2-L10 顺移 | 116+ 份 ADR 与七轮报告的层引用整体失真；6 源层表 + G-45/46 + 全部 crate 头注释同步爆炸；收益仅是词汇纯粹性 |
| R-C | 一次性大爆炸单波落地（10 crate 同时改） | 违反「四件同批 / 逐门验证 / 单波可回滚」；任一红无法归因 |
| R-D | 6 个 sibling 扁平化目录 | 让 G-105/G-113/G-115/G-119 五张台账全部换键（254 + 227 + …行），零收益 |
| R-E | 遥测留在 `nexus-core` 内，只建一个同名空壳 crate | 假 crate：无独立依赖边、无独立测试面，属「零生产者装饰」，本仓明令禁止 |

### D6：回滚

- 每一波独立提交、独立回滚；W3 的 7 个子批各自可 `git revert`。
- 门与脚本同进退（防 G-103 方向 A 死判据）；台账与脚本同 commit（G-119/G-123 已实测的 rc=2 耦合）。
- 禁止 `git checkout --` / `reset --hard` / `stash`（工作树含他人未提交改动）；回滚前 `git apply -R --check` 预检。
- 若 W3 任一子批出现非预期台账键漂移 ⇒ 只回滚该子批，不带病前进。

---

## 3. 影响

| 维度 | 影响 |
|---|---|
| crate 数 | 39 → 46（+6 契约 sibling +1 telemetry） |
| L0 成员 | 1 → 10（L0-A 7 + L0-B 3） |
| 消费方 | 34 个生产消费 crate **零改动**（facade 保证）；`nexus-core` 的 23 个依赖方零改动（层标签变化不改编译图） |
| 公共 API | 仅 1 处 `pub(crate) → pub`（N-4）；其余路径逐字不变；G-105 254→255、G-107 53→53（格式变） |
| 事件 | `NexusEvent` 变体/severity/mpsc 三锁**零变更**（152 / 20 / 16） |
| 依赖 | L0-A 白名单 4 项不变；L0-B 新增 `nexus-core → nexus-telemetry` 单向边；无环 |
| 门 | +2（G-125/126）；G-99/105/107/113/115/117/119/123 辖区或键格式调整，判据语义不变 |
| 文档 | CODE_WIKI §2.1/§3.0/§3.1、agents.md §2.1、nuxus规则 §2.1、根 Cargo.toml 注释、lib.rs 头文档、CHANGELOG、`doc_count_freeze.txt` |
| 兼容性 | 拆分本身 100% 向后兼容（路径不变）；`pub(crate)→pub` 是**放宽**而非破坏；唯一潜在破坏是 sibling 间 `#[non_exhaustive]` 类型的字面量构造，已由 ADR-215 的 11 处适配先例覆盖 |
| 治理叙事 | **与 ADR-193「架构减法」方向相反**：本 ADR 明确登记为业主主动反转，理由是可维护性与按域纯度门，而非功能收益 |

---

## 4. 验收判据（逐波，实测）

**W0**
- `py -3 scripts/adr_index_sync.py` rc=0（无 UNINDEXED）
- `py -3 scripts/check_doc_consistency.sh` rc=0；`py -3 scripts/check_entry_contract.py` GREEN；`py -3 scripts/check_doc_drift.sh` rc=0
- `lib.rs` 三处幽灵引用消失：`grep -n "model-router\|rl-client\|memory-pyramid" crates/nexus-contracts/src/lib.rs` = 0

**W1**
- `check_l0_sublayer.py --selftest` / 真实均 rc=0；三条植入例逐条判红
- `check_layer_map_parity.py` `39=39=39=39=39=39`；`audit_gate_parity` / `check_gates_provenance` rc=0

**W2**
- `cargo test -p nexus-telemetry` / `-p nexus-core` / `-p nexus-contracts` 0 失败
- `cargo check --workspace --all-targets` rc=0；`check_layer_map_parity` `40=40=40=40=40=40`

**W3（每子批）**
- `cargo check -p <new> --all-targets` + `cargo check --workspace --all-targets` rc=0
- `cargo test -p nexus-contracts` 0 失败
- G-105/G-115/G-119/G-113/G-117/G-123 键集 diff = 0（W3.1 允许 +1）

**W4**
- 逐包 `cargo fmt --check`（46/46）；`cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` rc=0
- `cargo test --workspace --jobs 4 --exclude chimera-e2e-tests` 0 失败
- `run_gate_manifest.py` 全量 light；`check_layer_map_parity` `46=46=46=46=46=46`
- **已知红不洗白**：G-16（仓外 spec 指针，ADR-212:108 维持不修，无 workflow 调度）

---

## 5. 回滚

| 波 | 回滚 |
|---|---|
| W0 | 纯文档 + 3 行注释，反向编辑 |
| W1 | G-125/126 四件同批 `git revert`；`.sh`/`.ps1` 的 `sublayer_of()` 单点删除 |
| W2 | 新 crate + `counters.rs` + 3 处装配点同提交，整组逆序退 |
| W3 | 7 个子批逐批 `git revert`；facade 行随子批同退 |
| W4 | 4 个子批（纯度/根面/其他门/文档）独立退 |

---

## 6. 未做 / 待裁（诚实登记）

1. ~~**L0-B 的纯度上限未定义**~~ **已关闭（R9，见 §9 附录）**：白名单改为 `{event-bus, tokio, tracing}`，新增第 4 个外部依赖须另立 ADR；不新增冻结门（理由与触发条件见附录）。
2. **`nexus-telemetry` 的事件面**：复用既有 `ComputePoolSaturated`，不新增变体。若未来需要独立的 telemetry 事件族，须走七点位同批（含 `doc_count_freeze.txt` 与 `variant_count_test`）。
3. **`#[non_exhaustive]` 跨 crate 构造**：sibling 拆分后，umbrella 的集成测试仍是「外部 crate」，不得对 sibling 的 `closed` 类型使用结构体字面量；由 G-115 台账与编译共同守护，本轮不新增属性。
4. **ADR-193 的反转叙事**：本 ADR 已登记「+7 crate」与「架构减法」冲突；若治理委员会要求，须另立一份「crate 数政策」ADR，不在本 ADR 内扩展。
5. **G-16 既有红**：维持 ADR-212:108「不修」；本轮不代改仓外 spec。
6. **T-3/T-4/F-6/Q-3**：沿用 ADR-221 §11 的 defer 与触发条件，本轮不动。

---

## 7. 继承与推翻摘要

- **继承**：ADR-033 受控纯逻辑；ADR-212 D3/D5（G-99 + 例外登记表）；ADR-213~221 全部门纪律与台账；ADR-187（Check A 仍退役）；ADR-195（GPU 仍否决）。
- **部分推翻**：ADR-212 D1（L0 单 crate 边界）与 D2（否决拆 6）；ADR-049 D1（0 新 crate）。
- **方向反转登记**：ADR-193 的「架构减法」在本轮被业主主动反转（+7 crate），理由与代价见 §3 与 §6.4。

---

## 8. 执行记录（2026-10-20，W0-W4 全部落地）

**终态（本轮实测，非引用）**：`workspace.members = 46`；层表 6 源 `46=46=46=46=46=46`；G-125 自报 `0A=7 / 0B=3 / internal edges=14`；G-99 `70 src files / 0 impurity`；G-105 `255/255`；G-107 `53/53`；G-115 `227/227 (open=56/closed=171)`；G-91/G-123 `defs=236 zero=75 one=86 ge2=75 sum=OK`；全量 light 门集 `ok=118 / mismatch=1 / skip=7 = 126/126`（唯一红 = 既有 G-16）；`cargo check --workspace --all-targets` 与 `cargo clippy --workspace --all-targets -- -D warnings` 均 rc=0；逐包 `cargo fmt -- --check` 46/46 + 根包 rc=0。**上述整条链在文档批与 `.ps1` 修复落盘后全量重跑一次复核，读数逐值不变**（含 `cargo check` 1m58s rc=0、clippy rc=0、fmt `fmt_bad=0 / 46`、根包 rc=0、门集 126/126、G-07 提交耦合 rc=0）；铁律双源现为 `bash .sh` rc=0 与 `pwsh .ps1` rc=0 两者均实测（首轮只跑了 `.sh`，见下条缺陷）。

**回归证据（与 CI `check` job 同口径，不含 doctest）**：`cargo test --workspace --exclude chimera-e2e-tests --lib --bins --tests -j 4` → **rc=0 / 11308 passed / 0 failed / 443 个 target**；`cargo test -p chimera-e2e-tests --tests -j 4` → **rc=0 / 316 passed / 0 failed / 35 个 target**；合计 **11624 passed / 0 failed**。**doctest 已全量跑过一遍，归因经两次订正定稿**：`cargo test --workspace --exclude chimera-e2e-tests --doc --no-fail-fast` → **46/46 doc target 全跑完、267 passed / 26 failed**，失败集**仅** `chimera-mas`（9 passed/14 failed/3 ignored）与 `repo-wiki` 两个 target——与 2026-10 历史基线**完全同集**，且两 crate 本波**零字节改动** ⇒ **本波零 doctest 回归**。**失败签名清一色 `error[E0462]: found staticlib <proc-macro> instead of rlib or dylib`**（zerocopy_derive/prost_derive/pastey/pest_derive/equator_macro）⇒ windows-gnu 下 doctest 编译单元把 proc-macro 解析成 staticlib 的**平台/工具链类条件**。**复测证据**：将 `chimera-mas` 彻底 clean 后 `-j 1` 重建再跑，仍 9/14 failed 且逐条 E0462 ⇒ 不是产物缓存问题（构建阶段的 `E0786/mmap/OOM` 是另一件事：3480 个 0 字节 `.rmeta` + 内存压力，见报告 §5.3）。**合入路径上无 doctest 执行者**：PR 快轨三 job 全用 `cargo nextest run`（不跑 doc-tests，`ci.yml:628` 自陈），唯一执行体是 `release.yml` 的 Linux test job（仅 tag/dispatch）⇒ 登记为验证空白 **R8-10**。本波新增/改动 crate 的 doctest 已逐包验过：`nexus-contracts` 8、`-mca` 2、`-memory` 3、`-seams` 44、`-sparse` 1、`parliament` 9（均 0 failed）。

**9 处实施偏差（1-7 均实测驱动、不改结论；8-9 是「计划条目与自家门互斥」或「按字面不必要」；完整证据见报告 §2.1）**：

1. **门重键提前**（原计划 W4 → 实做 W3.1 前）：五张门的扫描根写死单 crate，`core_domain` 一搬走 `check_contract_consumers` 就自报 `defs 236→213`，将产 22 条伪 `[STALE]`——per-sub-batch 验收要求的「零换键」必须先让门变成 roster 驱动。
2. **新增 `scripts/l0_roster.py` 单一权威源**（本 ADR 未预先列出，现登记）：五份元组的「漏改一处即静默缩小辖区」风险高于多一个文件的成本；G-125 因此多了 `[ROSTER]` 判据（roster 与 `.sh/.ps1` 的 0A 集不等 ⇒ rc 2，含双向夹具）。
3. **G-107 未改键格式**：facade 用 `pub use nexus_contracts_<domain>::<group>;` + 保留 53 条 `pub use <group>::<leaf>;`，台账逐字不变；只加了「组绑定不计为叶子」的识别规则。**计划写的「重出 53 行」未发生**。
4. **每个 sibling 带 `nexus-contracts` 的 dev-dep**：迁出的 doctest 仍举 facade 路径，那才是 D4 兼容承诺的活证明；改为 sibling 自有路径等于把断言作废。代价：一个仅在 dev 面存在的环（G-99 只判 `[dependencies]`，不因此豁免）。
5. **W2 的「断环设计」代码块不可照字面实现**：它给的 `trait SaturationSource { fn snapshot(&self) -> SaturationSnapshot }` 无法承载 HEAD `telemetry.rs:286` 循环体的三件事（采样发布 / `calibration::flush(&ComputeBridge)` 写表 / 逐 kind 证据）：写表需要 `nexus-core` 的桥句柄，单方法 `snapshot()` 调不动它。要么丢写表（功能回归），要么让 telemetry 依赖 nexus-core（本 ADR D3 与计划第 120 行共同禁止）。落地为三方法 trait，DTO 为 `CalibrationEvidence`（枚举字段字符串化以免引 crate 环），`period` 保持既有 `period_secs: u64`。反方案 R-E（字面单方法）与 R-F（事件构造上提）的否决理由与行级证据见报告 §2.1 第 5 条；**目的（依赖单向 + 断环）不变且由 G-125 双向夹具锁住**。
6. **计划 W2 点名的 `efficiency-monitor/src/collectors.rs:248` 经复测不是调用点**：全仓 `spawn_saturation_reporter` 仅 3 处装配调用（chimera-cli 的 `experience_loop.rs:292` / `commands/acp.rs:121` / `commands/serve.rs:43`），而行 248 实为测试里构造 `ComputePoolSaturated` 的字段；且 ADR-197 D3 已否决 `efficiency-monitor → nexus-core` 边（G-33 只减不增），故该 crate 不可能持有上报器。属规划期误引，零改动。
7. **`nexus-telemetry` 的生产依赖是 3 个而非计划列的 4 个**（无 `nexus-contracts`）：抽取出的代码不引用任何 L0 类型，按字面加上去会直接撞 G-31 `check_declared_dep_usage.py`（声明未用即红）；属「为满足清单而造死依赖」，与本仓对零生产者装饰的禁令同病类，故不加。
8. **计划 W3 step 3 的 `//!` 头「LAYER: L0-A」与自家门互斥**：`LAYER` 行由 `crate_contract.py` 从 `.sh layer_of()` 派生为 `"L%d"`，写 `L0-A` 会被 G-59 判「says L0-A, disk says L0」。落地形式：7 个 L0-A crate 的 `LAYER` 均为 `L0`，**亚层载于 ROLE 文本 + `.sh/.ps1` 双源表 + G-125**——这是 D1「层号不重排」的直接推论，不是漏做。
9. **计划 W4.1 的「`FILE_EXEMPT` 键加 crate 前缀（2 行）」按字面未做**：两个被豁免文件均在未拆的 umbrella 内，路径未变；sibling 侧另有前缀机制。单独加前缀会把 `2 exempted` 掉成 `0 exempted + 2 impurity`，除非与 `scan_symbols()` 的相对路径计算同批改——登记以免下轮误判为漏做。

**审计后补做的三项文档产物**（首轮交付漏项，由完成度审计发现并于同日补齐）：`.qoder/rules/chimera2.md`（§3 层表 L0/L1 行与层名/层号分离说明；首轮该文件未被编辑却在报告里被记为已同步——已订正）、`crates/nexus-contracts/src/lib.rs` 头文档（W4.2 明列项：「64 源文件」→ 7-crate 实测结构、「零 workspace 内部依赖」→「仅 6 个 L0-A sibling 且方向单向」、例外表 ADR-222 归属说明；ROLE 重写后重跑 `emit_architecture_views.py --write`，G-70 `--verify` rc=0）、`CODE_WIKI.md` §3/§3.0/§3.1/§11（W4.2 明列的「§3.0/§3.1 crate 索引（+7）与 §11 ADR 索引指针」，首轮只改了 §2.1：现 §3 标题 39→46、§3.0 改 L0-A 7 crates 并补六 sibling 索引表、§3.1 补 `nexus-telemetry` 完整块（标题原写 3 crates 而表里只有 2 个）、§11 加批次指针并标「主编号至 ADR-198」为过期快照）。

**次轮审计又抖出的「现在时态陈旧口径」清扫（全部零行为变更，同一病灶：文档声明比使它成立的那次改动活得久）**：① `crates/nexus-contracts/Cargo.toml` 的 `description` 与三段注释仍以现在时态写「零 workspace 内部依赖 / 禁止依赖任何 workspace crate」，而**同一文件自己的 `[dependencies]` 就声明了 6 条**（已按 ADR-222 D2/D4 口径订正，并写明机器判据 = G-99 + G-125）；同族旧口径另在 4 处注释：`sparse/ids.rs:12`、`seams/memory_strategy.rs:55`、`nexus-core/tests/id_newtype_parity.rs:8`（附带修一个因拆分而失效的 `crates/nexus-contracts/src/sparse/ids.rs:38` 路径指针）、`spec/harness_spec.rs:321`，以及 `CHIMERA_系统架构总图_v2.29.0-omega.html:171`（按「历史文档只加注不改写」纪律只加时点注）。② CODE_WIKI 顶部的“现行基线指针”行仍写 39 members / 本文档 = 39 crates ⇒ 已标为 2026-10-02 快照并新增一行现势 46 指针；§3.11 可达性行加「成员数现 46、可达性未重测」注；三处 `41/41 crate 全合规` 改为 **46/46（现取，旧值标为时点）**；§1.1 技术栈行 39→46；目录锚点 `#3-39-crate-完整索引` 随标题同步为 46（首轮回标题时漏改，属本轮自己引入的断链）。③ `adr_index.md` 的 ADR-222 行与本文 D4 原写「只改 G-107 与 G-123 两张」，而两张最终均未换键——已各加「执行后注」，防止下轮把重键当待补项。④ `gate_manifest.toml` G-125 note 抄了过期实况（0A=1/0B=2/边=3）——已删数并改指自报行，与 G-126 note 自定纪律对齐。⑤ 报告 §4.2 的 G-33 读数 `146/146` 为登记中途时点值，现取为 `154/154`；§2.1 第 4 条「每个 sibling 带 dev-dep」过宽（`nexus-contracts-core` 无），已改措辞并补理由；W0 报告 §5.2 标题「实测 15 条边」无可复现口径，已按 11（跨 crate，= G-33 本波新增 11 行）/ 14（crate 级，G-125 自报）两个量纲就地订正。

**公共 API 的唯一变化**：`core_domain/checkpoint::is_schema_version_unset` 由 `pub(crate)` 升 `pub`（被 `memory/token_evidence.rs` 跨 crate 的 serde 属性引用），与 G-105 台账 +1 行同批。

**本波引入并已修的一处缺陷（不归给既有、也不当作“已验无问题”）**：W4.3 明列的 `pwsh scripts/check_dependency_rules.ps1` 首跑实为 **rc=1 ParserError**（`$layerMap` 与 `$subLayerMap` 各多写了一个 `nexus-contracts-safety` 键），即上一版 §8 写的「逐条 rc=0」在这一条上不成立——成因是批量编辑重复追键，而 **G-45/46 的 `.ps1` 解析器用 dict 建表，重复键被后写覆盖（键数仍 46 ⇒ 对拍恒等），且 `ci.yml` 只跑 `.sh`，没有任何执行体拒这张表**。除删重复键外，已给三道现有门补上形状判据（无新门号 ⇒ 不触发四件同批）：`parse_ps1` 任意重复键即拒、`parse_sh` 同 crate 两层号臂即拒、G-125 `$subLayerMap` 重复键判**不可判定 rc=2**并逐名点出；自测条数由脚本自报（G-46 22 / G-126 23），并拿真文件副本做了四路牙齿验证（clean / 两处植入重复 / 冲突臂）。可复用判据：**双源表的对拍门必须拿该源自己的解释器语义当上界**；残留登记为报告 §7 R8-9（PowerShell-only 的其它语法错仍无执行体会拒，要补只能补 Windows runner 一步，需单独授权）。

**未推翻且未动的**：`NexusEvent` 三锁零变更；Check A 仍退役；GPU 仍否决；G-16 仍按 ADR-212:108 不修；T-3/T-4/F-6/Q-3 沿用 ADR-221 的 defer。

**验收途中发现并修掉的两处既有缺陷（均非本波引入，但阻塞 W4「全量 0 失败」判据）**：

1. `chimera-tui/tests/data_test.rs::pipeline_handles_1000_events_per_second`——单跑 3 次 1 过 2 败，失败轮均停在 128 且撞满其自述 3s 窗口。根因是测试配置与 Eco 降频相撞：首轮 drain 条数由调度决定，只要 ≥ `event_backlog_threshold`(100) 就切 Eco（1s 节奏），而旧配置 tick=250ms + 窗口 3s 使「消费完」与窗口重叠。修法只改测试配置（阈值抬到突发之上 + tick 抬快 + 窗口改作看门狗），**断言口径未动**；修后六连跑全绿（~1.8-2.4s）。Eco 本身是正确的事件风暴保护，不得改其生产语义。
2. `crates/parliament/src/delegation.rs:15` 的「# 公式」块是裸 ``` 围栏包伪代码，被 rustdoc 当 Rust 编译⇒ doctest 语法错（`×` 为非法 token）。该文件相对 HEAD 干净，缺陷由 `62b3834`（D-18 接线，另一批次）带入。修法：围栏改 ```text，零行为变更；修后 `cargo test -p parliament --doc` 9 passed / 0 failed。

**仍待补的覆盖缺口（已登记，不在本波夹带）**：流水线的 Eco 切换规则（`pipeline.rs` 的 backlog→Eco 与 5 次倒计时）全仓**无测试覆盖**——上述修法只是不再依赖它，不是验了它；见报告 §7 R8-7。**【R9 已收口】** 该判定已抽成纯函数 `eco_transition(mode, countdown, backlog, threshold)`，由 `configurable_tick_test.rs` 的 6 条确定性夹具驱动（边界 `>=` 语义、连续 5 次低 backlog 才退出、Eco 中重新越阈重置倒计时、Normal 不动倒计时、阈值 0 边界、纯函数可重复），不再依赖 `sleep`。

**提交状态（R9 更新）**：上一版「全部改动在工作树、未 commit」已过时——R9 已完成分组提交（C3-1 原子拆分批 / C3-2 CI 门批 / C3-3 Eco 测试批 / C3-4 文档批），每组均按显式路径 `git add`，可独立 `git revert`。

---

## 9. 附录：L0-B 依赖白名单与新增流程（R9 收口 R8-1）

**适用范围**：层名为 L0-B 的三个 crate（`nexus-core` / `event-bus` / `nexus-telemetry`）。L0-A 的依赖规则不在此附录，仍由 ADR-033 白名单与 G-99 守护。

**白名单（现行，2026-10-21 实测）**：

| crate | 生产 `[dependencies]` 里的 **非 workspace** 依赖 | 依据 |
|---|---|---|
| `nexus-telemetry` | `tokio`、`tracing` | 上报器周期循环 + 结构化证据输出；抽取前即由 `nexus-core` 持有，上提不改面 |
| `nexus-core` / `event-bus` | 维持各自既有面不动 | 本附录不追溯改写既有依赖，只约束「新增」 |

同层 `event-bus` 属 L0-B 内部边（`nexus-telemetry → event-bus`），不计入外部白名单。

**新增流程（硬要求）**：L0-B 要引入白名单之外的**任何**新外部依赖，必须另立 ADR 并同时给出：①最小性论证（为何现有三个不够）；②层向论证（是否可下沉 L0-A 或不引入）；③体积/编译/供应链影响（`cargo deny`/`cargo audit` 面）；④若涉及异步运行时能力，需说明是否打开 `tokio` 新 feature。**不得以「反正 L0-B 不是纯契约」为由直接加**。

**为何不设冻结门**：G-90（`nexus_core_dep_freeze.txt`）的形态只适合「一旦冻结就长期不动」的枢纽 crate；L0-B 的依赖面是**三个 crate 的并集**，且 `nexus-telemetry` 刚上提、演进概率高。此时加 only-decrease 棘轮会把「合法新增」也逼成台账手术，属门膨胀。**触发条件**：L0-B 第一次真的需要第 4 个外部依赖时，同批补一道冻结门（脚本 + 台账 + manifest + ci，四件同批），而不是事后补。

**边界提醒**：L0-B 允许依赖 L0-A（向下，恒允许）；**禁止**依赖层号 ≥ 2（G-125 `[UPWARD]` 机器守护）。本附录不改变这条。
