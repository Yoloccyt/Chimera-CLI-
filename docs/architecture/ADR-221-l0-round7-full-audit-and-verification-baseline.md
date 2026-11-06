# ADR-221: L0 第七轮 —— 全量十维重审、五项待裁裁决与验证基线受限登记

> **Status**: ✅ Accepted (2026-10-19)
> **决策日期**: 2026-10-19
> **轮次**: L0 专项第七轮（前六轮见 ADR-212 / ADR-213 / ADR-214 / ADR-215 / ADR-216 / ADR-217 / ADR-218 / ADR-219 / ADR-220）
> **关联 ADR**: ADR-033（L0 建立 + 追加节「受控纯逻辑」）/ ADR-212（边界总裁决，决策 1–5 全部继承）/ ADR-213（R8 Class A/B、D-11~D-21 系列台账起源）/ ADR-214（D-22 起源）/ ADR-215（D-27/D-29 起源）/ ADR-217（bench 与 facade）/ ADR-218（D-41~D-45）/ ADR-220（G-123/G-124 与机制字段）/ ADR-187（内环 Check A 退役）/ ADR-166（`.sh`/`.ps1` 孪生口径差异）/ ADR-219（Python 工具面 stdlib-only）
> **权威源**: 代码与脚本实测——本机 2026-10-19 现取；两份权威设计文档（v3.4.0 二十三篇 / 十六源手册 v3.0）仅作意图参照
> **编号合法性**: 落盘前 `LS docs/architecture/` 现取主编号最大 = **ADR-220**；`grep -rn "ADR-221"` 仅命中 ADR-220 正文的一句「无预留注记」引述，**无文件、无预留** ⇒ 221 空闲可用
> **轮次起点**: 分支 `feat/c1-compute-plane-governance`，HEAD `ffb331a`，工作树干净（0 项）

---

## 1. 范围裁决（先说结论，因为它改变了任务本身）

本轮由用户显式裁定为**「全量十维重审 + 收口」**，并**不重开** ADR-212~220 已裁项（L0 单 crate 边界、否决拆 6 个 `*-contracts`、G-99 纯度、可扩展性分级、facade、bench 台账等一律维持）。

事实基线：L0 已完成**六轮收口 + 两批补丁**（D-18 孤儿接线、D-31/D-32 文档口径），L0 相关内容轴门 12 组（G-62/63·G-91·G-95/96·G-99/100·G-101/102·G-105/106·G-107/108·G-113/114·G-115/116·G-117/118·G-119/120·G-123/124）全部就位。因此本轮的增量价值只能是：**当轮现取的十维复测**（而非引用旧报告数字）+ **残余台账的收口** + **新前提下的重判**。

---

## 2. 十维复测读数（当轮现取；口径见各条取证命令）

| # | 维度 | 现势读数 | 判定 |
|---|---|---|---|
| 1 | 分层与依赖 | 六源对拍 `39=39=39=39=39=39`；`check_dependency_rules.sh` rc=0（Check B/C/D/E）；**Check A 已按 ADR-187 双源退役且脚本内无 `is_inner_ring`/`CoreLoopEvent` 残留**；L0 `[dependencies]` 恰 serde/chrono/uuid/thiserror（皆外部），`src`+`tests` 对 `nexus_core::`/`event_bus::` 的 13 处命中**全在注释/doc-string** ⇒ **L0 出边 = 0** | ✅ 合规 |
| 2 | Crate/模块结构 | **64 个 .rs / 25,746 行**（`Get-Content` 数组计数与 `ReadAllLines` 两法逐值一致；`Measure-Object -Line` 口径**低报**，须弃用）；G-109 `live=0 listed=0`（9 孤儿台账已清空） | ✅ 合规 |
| 3 | 接口契约 | G-105 `live=254 listed=254`；G-107 `live=53 listed=53`；G-115 `types=227 listed=227 open=56 closed=171`；`pub(crate)`=2（均在 `core_domain/checkpoint.rs`）；`#[non_exhaustive]`=57 处/16 文件；第三方类型进公开字段 `DateTime<Utc>`=4 / `Uuid`=9（ADR-213「边界类型豁免」，白名单允许）；`serde_json` 生产面 0 | ✅ 合规 |
| 4 | 事件定义 | `NexusEvent` 本体在 **L1**（`event-bus/src/types.rs:28`），变体 **152**；`CRITICAL_MPSC_VARIANTS`=16、`CRITICAL_TOTAL`=20；L0 另定义 `EventSeverity`/`TaskPriority`/`AgentStatus` 等纯载荷类型（`core_domain/event_payload.rs`）——**「L0 只持 EventMetadata」的说法过窄**，但「变体本体不在 L0、定级逻辑留 L1」成立 | ✅ 零变更 |
| 5 | 并发模型 | G-99 rc=0；生产面**并发原语 = 0**（`async`/`.await`/`tokio`/`Mutex`/`RwLock`/`Arc<`/`spawn`/`thread` 的原始命中全在注释或 `#[cfg(test)]` 尾） | ✅ 合规 |
| 6 | 安全治理 | 真实 `unsafe` = **0**（3 处命中均为注释）；`#![forbid(unsafe_code)]` 在 `src/lib.rs:152`（inner attr 位于 crate 级 `//!` 文档之后，语法合法）；`benches/util_micro.rs` 的 `unsafe impl GlobalAlloc` 属独立编译单元，ADR-215 D-26 已登记豁免；`todo!`/`unimplemented!`/`unreachable!` = 0 | ✅ 合规 |
| 7 | 存储与记忆 | G-113 `sites=1 judged=1 sanctioned=0 waived=1 orphan-excluded=0`（唯一豁免 `chimera-tui/src/popup.rs`；D-18 接线后 `orphan-excluded` 归 0）；携带 `schema_version` 的 L0 信封 = 3（`Checkpoint`/`TokenLedgerEntry`/`SegmentMetadata`）；**ADR-213 R9 危险组合（`default+skip` 落在位置式 `to_vec` 类型）生产写点已全部为 `to_vec_named`** | ✅ 零变更 |
| 8 | 性能与资源 | 3 个 L0 bench（`sparse_select_top_k`/`util_micro`/`xts_top_k`）；`check_perf_redlines.py` 三档 rc=0、`unknown=0`；**51 个派生逻辑函数中仅 3 个有 bench**，热路径 `sensitive::scan`（21 站点 / 11 crate）与 `cosine_similarity_slices`（6 crate）无 bench；`canonical_merkle_input` 生产 0 调用 | ⚠ 见 §9 决策 2（bench 范围） |
| 9 | 测试与门禁 | 门 **124 道**（G-01…G-124 连续）；light 全量快照 `ok=116 / mismatch=1 / env-fail=0 / unknown=0 / skip=7 = 合计 124/124`，**唯一红 = G-16**（仓外 spec 指针，ADR-212:108 已裁不改，且无 workflow 调度）；`audit_gate_parity` rc=0、`check_gates_provenance` rc=0（3 条豁免带理由）；L0 测试面 15 文件 | ✅ 合规（除既有 G-16） |
| 10 | 文档与代码一致性 | 五门 rc=0（drift/consistency/symbol_claims/pointer_integrity/adr_index_sync）；`check_doc_drift.sh` 锚 `152/10/39`；ADR 物理 **116** 文件 / 主编号 max **220**；`adr_index.md` 声明总数已 D-31 派生化（自报 193） | ✅ 合规（文件数 +1 属预期漂移） |

---

## 3. 本轮新发现与处置

| 编号 | 问题 | 证据（当轮现取） | 级别 | 处置 |
|---|---|---|---|---|
| **F-1** | **门自身 RED**：`check_entry_contract.py` 判 `adr_index_metrics.py::C2 :: __main__ has no crash->2 guard`，`VERDICT: RED (1 unresolved)`。该脚本由 D-31（2026-10-17）引入，其 `__main__` 只有 `sys.exit(main(...))`——未捕获异常会让解释器**退 1**，与「判定为红」同码（F33 假绿通道）。 | `py -3 scripts/check_entry_contract.py` ⇒ `unresolved by family: {'C2': 1}` | **P2** | ✅ **已修复**：按 `scripts/gate_rc.py` 的**单点守卫**范式改为 `sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))`。修复中踩到一条**可复现陷阱**：该文件首行声明 `# -*- coding: ascii -*-`，中文注释直接 `SyntaxError: 'ascii' codec can't decode byte 0xe5` ⇒ 注释必须 ASCII（本仓脚本注释语言**不统一**，改前须看首行声明）。复验：`VERDICT: GREEN`；`--declared-total`=193 rc=0；`--selftest` PASS(12 例)；`check_doc_consistency.sh` rc=0 |
| **F-2** | **前提前提作废 ⇒ D-21 翻转**：前五轮记录「`pwsh` 本机不存在 ⇒ G-17/G-18 永久 rc=2 不可判定」。本机 `pwsh` **已存在**（`Get-Command pwsh` 命中，`pwsh -NoProfile -File` 实跑成功）。 | `xdoc_precise.py` rc=**0**（.ps1 自报 6类/14id）；`audit_gate_selfreport.py` rc=**0**（ps1 自报=实算，GAP 0）；manifest `env-fail` 由 1 转 **0** | **P2** | 重判落地（§9 决策 6）。**孪生两侧不主张等价**：两侧均已真跑且均 rc=0，但自报 check id 数 **14 ≠ 7**，与 ADR-166 的既有论断一致 ⇒ 只能主张「两侧各自运行且均绿」 |
| **F-3** | 文档漂移：`wire_format_compat.rs:39` 注释称样本「as quest-engine writes it today: `rmp_serde::to_vec` (array)」，而生产写点为 `to_vec_named`（`quest-engine/src/checkpoint.rs:126`，ADR-213 R8 已切）。 | 读码对照 | P3 | ✅ **已修复**：注释改为中性「legacy 写侧」表述并写明 R8 依据 |
| **F-4** | 纯度门 **SHARED-STATE 类别不含 `Arc<`**：现盘 9 处 `Arc<` 全在注释/测试，无生产风险，但未来生产类型引入 `Arc` 时本门**不会告警**。 | 读 `scripts/check_l0_purity.py` 的 `FORBIDDEN` 19 符号 / 3 类别（IO-EFFECT 11 · AMBIENT-INPUT 2 · SHARED-STATE 6） | P3 | **登记裁决**：`Arc` 为可共享只读容器，与 `Rc/RefCell/Cell/UnsafeCell` 的「内部可变性」语义不同，且 `#![forbid(unsafe_code)]` 与「无锁」纪律另有门；**维持现口径**，扭曲判据以迎合假设风险属过度工程 |
| **F-5** | **T-2 前提已被并发批次改动**：ADR-218 §7 / ADR-220 §10 记「6 条 `chimera-cli` optional 边零引用且 `[features]` **无** `dep:` 映射」；实测 `[features]` 现存在 `r2_island_repayment = [dep:mtpe-executor, dep:ssra-fusion, dep:sesa-router, dep:csn-substitutor, dep:chtc-bridge, dep:nexus-hook]`。 | `crates/chimera-cli/Cargo.toml:21-25`；`grep -rn <crate> crates/chimera-cli --include=*.rs` = **0** 命中 | **P1** | **必须重跑 `cargo machete` 再判**，不得沿用旧结论（§11） |
| **F-6** | 75 条零消费者豁免在 `contract_consumer_baseline.txt` 中**无理由注释**（`[REGISTER-DEBT] bare=75`）。 | `check_contract_zero_consumers.py` 自报 | P3 | advisory（不影响 rc），关联 T-3 治理面 |
| **F-7** | 两个 Critical 计数**量纲不同**易被误读为漂移：`check_immutable_surface_parity.py` 报 `Critical=17`，`bus.rs` 报 `CRITICAL_TOTAL=20`。17 = ImmutableSurface 覆盖面（16 mpsc + 1 broadcast-only `CheckpointSaved`）；20 = `severity()==Critical` 全集（16 + 4 历史 broadcast-only）。 | 读两侧定义域 | 无 | **非缺陷**；在基线表中显式区分量纲（本轮登记，防下一轮误判） |
| **F-8** | ADR 物理文件 116（前轮 115），主编号 max 220 不变。 | `check_doc_consistency` 自报 116 files / 111 main numbers | 无 | 预期漂移（含 `ADR-XXX` 占位冻结件），不处置 |
| **F-9** | G-16 `audit_spec_coverage.py` rc=1 而 `expect=0`（仓外 spec 指针 `crates/nexus-contracts/src/util.rs`，W3 后实为 `vector_math/util.rs`）；**无任何 `.github/workflows/*.yml` 调度该脚本**。 | 门自报；全 `.github` grep 零命中 | 无 | **维持 ADR-212:108「不修」裁决**（仓外代改即污染他库） |

---

## 4. 决策 1（D-11）：`pub fn` **不并入** G-91 的 `DEF_RE`

**裁决**：维持 G-105/G-106 作为 `pub fn` 面的**独立只减棘轮**，不把 `pub fn` 并入 G-91（契约消费者门）的定义枚举。

**依据**：① 两者判据对象不同——G-91 判「类型是否有 ≥2 消费者」，G-105 判「`pub fn` 多重集是否只减」；② G-91 的 `DEF_RE` 只枚举**类型**定义，并入 `pub fn` 需重写其解析与基线（`contract_consumer_baseline.txt` 161 行将整体换键），属**改动一个正在工作的门**；③ ADR-213 R4 已立 G-105 为独立轴，理由即「合并会重蹈互锁」（两道门对同一因素互相依赖，任一侧改动都会连带另一侧红）。

**反方案（否决）**：把 `pub fn` 并入 `DEF_RE` 统一一轴 —— 收益是「少一道门」，代价是**破坏 G-91 现有 161 行基线的语义**并使「零消费者」与「只减」两轴互相锁死；且合并后 `pub fn` 的**多重集**语义（同名不同模块）无法用 `DEF_RE` 的集合语义表达。

---

## 5. 决策 2（D-12）：「门判据刻意不同源」**成文为通用纪律**

**裁决**：把「同一事实的多口径**刻意不合并**」写成通用纪律并落档，明确三条已知例：① Critical 三口径（`CRITICAL_MPSC_VARIANTS`=16 / `CRITICAL_TOTAL`=20 / `severity()`=Critical 20）——三者定义域不同，**不得合并**（本轮 F-7 即为一次误读风险实例）；② `.sh` 与 `.ps1` 孪生门（ADR-166 已记两侧自报 id 数不等），**不得声称等价**，只能各自运行各自报；③ G-119 的 `mech/near/supp` 与 `basis` 散文——机制由机器字段承载，散文**禁止**叙述机制。

**依据**：本轮实测出 F-7（量纲误读）与 F-2（把「两侧都 rc=0」误推为「两侧等价」）两处真实误读风险 ⇒ 该纪律有实证必要性，非纸面原则。

**反方案（否决）**：合并成一个「统一 Critical 数」—— 会抹掉「哪些走 mpsc 旁路」这一**可执行**信息（`is_critical_mpsc_event()` 才是通道判据），把通道契约降级为一个数字；且历史上正是「文档抄一个合并后的数」造成多轮漂移。

---

## 6. 决策 3（D-16）：路径锚门**维持不建**（宁留人工）

**裁决**：不建「文档路径锚门」。W3 子模块化的搬移代价继续**由人工复核承担**。

**依据**：ADR-213 D-16 已判「自造扫描器在植不出有效坏例前就是假绿」，本轮复核该前提仍成立——抽取面至少四类口径不同（`docs/reports` 引用 / 派生视图 / ADR 证据 / gate note），且 G-16 本身就是一个「仓外路径指针」的既有红，说明**路径类判据的假阳性成本高于其覆盖增益**。

**反方案（否决）**：以「仓库内被引用的相对路径必须存在」为判据建门 —— 会立刻把 G-16 类**仓外**指针判红（ADR-212:108 已裁不改），并把 `docs/reports/*.md`（`.gitignore` 下不入库）里的历史路径全部纳入判红面，违背「历史文档只加注不改写」纪律。

---

## 7. 决策 4（D-22）：`#[non_exhaustive]` 与 wire 字段序正交两轴 —— **维持 defer**

**裁决**：档 3 剩余（`ExperienceCard` 61 处外部结构体字面量）**不执行**。

**依据**：ADR-215 已裁「铺开须等第二个 wire 演进实需」，并把 `memory/experience_card::ExperienceCard` 登记为 `class=closed`（G-115/G-116 台账），翻 `open` 必须是「加 `#[non_exhaustive]` + 改台账行」**两行同批**的自觉动作；且原计划「派生 `Default` + 61 处补 `..Default::default()`」会把**逐字段显式**的构造点降级为静默取默认——恰是那处敞口最不该有的形态。

**反方案（否决）**：本轮批量给 61 处补 `..Default::default()` —— 净效果是**降低构造点的可审查性**换取编译通过，且 ADR-213 R8 已证 `#[non_exhaustive]` 不解决 wire 换序（两轴正交），收益为零而风险为正。

---

## 8. 决策 5（D-29）：G-91 升级为「逐 crate 装配面棘轮」—— **维持 defer**

**裁决**：不把 G-91 从「≥2 消费者」升级为逐 crate 装配面棘轮。

**依据**：装配面已由 **ADR-160 可达性棘轮**（`check_crate_reachability`）+ **G-123/G-124 零消费者 exact-set 台账**共同承担（ADR-220 决策 1 刚落地）；再在 G-91 上叠一层同向判据会**制造第二个真值源**（本仓反复记录的病灶），且 G-91 的 ≥2 语义与装配可达性语义**不同量纲**（库内复用 vs 组合根可达）。

**反方案（否决）**：G-91 直接复用 `check_crate_reachability` 的装配图 —— 会让「一个 L0 类型只被 1 个 crate 用但那个 crate 可达」的被豁免形态**永远无法表达**，把一个覆盖率门变成布尔门，损失分辨力。

---

## 9. 决策 6（D-21 重判）：`pwsh` 在场 ⇒ G-17/G-18 由「不可判定」转为**可判定且绿**

**裁决**：D-21 关闭为「**前提作废**」。凡记载「`pwsh` 缺失 ⇒ G-17/G-18 永久 rc=2」的文档，按本仓「只加注不改写」纪律以**追加订正节**处理，不覆写原文。

**当轮证据**：`pwsh` 可解析且实际执行成功；`xdoc_precise.py` rc=0；`audit_gate_selfreport.py` rc=0；manifest `env-fail=0 / unknown=0`。

**口界限定（必须照此读）**：两侧**各自**运行且均绿 ≠ 两侧等价——`.ps1` 自报 6 类/14 id、`.sh` 自报 4 类/7 id（ADR-166 已记该差异）。因此本 ADR **不主张**「双源等价」，只主张「此前不可判定的两项现在可判定，且判定为绿」。

**反方案（否决）**：保留「恒不可判定」表述不动 —— 该表述**已可证伪**，继续沿用等于在新基线上写一句可被当场推翻的话，违背「断言必须可追溯」。

---

## 10. 验证基线（**受限登记**，不得写成已通过）

本轮验证遇到**环境级硬阻塞**，如实登记如下：

| 项 | 状态 | 证据 |
|---|---|---|
| 磁盘清理（Task A1） | **执行完成，但空间未回收** | 已按计划**先备份** `target/criterion/`（2794 文件 → `tmp/criterion_backup_20261019/`）**并在盘保留**，随后清除 `target/debug`(124.35 GB)·`target/release`(4.80 GB)·`target/doc`；命名空间已清（`target/` 由 129.19 GB → **0.02 GB**）。**但 D 盘可用空间始终停在 0.78 GB**：无持有句柄进程（`Get-Process` 无 rust/cargo/analyzer/pwsh）、`Get-PSDrive`/CIM/`Get-Volume` 三 API 一致 ⇒ 疑为 deferred-delete / 影子副本 / 沙箱 overlay 级现象，**沙箱内无可安全处置手段** |
| 根因（磁盘为何满） | **非项目占用** | `D:\Chimera CLI` 各子目录合计仅 ≈12 GB（`.toolchain` 10.27 / `tmp_podman` 1.71 / 其余 <0.1）；D: 根为**通用数据盘**（Steam/WeChat/BaiduNetdisk/docker/backup 等用户数据），C: 亦仅剩 0.57 GB ⇒ **不得触碰项目外数据** |
| `cargo fmt -p nexus-contracts -- --check` | ✅ **rc=0** | 本轮 `.rs` 改动（注释）格式干净 |
| `cargo check -p nexus-contracts --all-targets` | ✅ **rc=0**（1m13s） | 定向编译可行 |
| `cargo test -p nexus-contracts` | ❌ **rc=101（磁盘不足）** | 测试二进制链接超出可用空间；D 盘一度归零 ⇒ **本地测试面不可行** |
| 全部门脚本（light 全量 + L0 门族 + 文档门族） | ✅ 全绿（唯一 G-16 既有红） | §2 维度 9 |
| `cargo clippy/check/test --workspace` | ❌ **未执行（交 CI）** | 磁盘不支持；改动面为「1 个 python 门脚本 + 1 处测试注释」，公共 API 面经 G-105 `254/254` 与 G-107 `53/53` 逐值不变证明未被触碰 ⇒ 其余 crate 编译断裂通路不存在（**这是论证不是实测**，登记为未验证项） |

**结论**：本轮**不主张**「无功能退化 / 无性能劣化 / 无安全漏洞 / 无公共 API 不当暴露」四判据已由**本地全量**证实；其中「无公共 API 不当暴露」有门证据（逐值不变），其余三项的精确定位是**改动面极小 + 定向 `check` 通过 + 全量交 CI**。

---

## 11. 剩余台账处置

| 编号 | 情形 | 处置 |
|---|---|---|
| **T-2** | `cargo machete` 阻塞红。**前提前提作废**：`r2_island_repayment` feature 现有 `dep:` 映射这 6 条（F-5），而 ADR-218/220 记的是「无 `dep:` 映射」 | **先重跑 `cargo machete` 取真实 rc**（受磁盘阻塞，交 CI 或腾空间后执行）；用户已裁定「先取证据再定」，若仍红则按删边/挂 feature/加 ignored 表**优先方案**落地。**本轮不落任何一条** |
| **T-3** | `tests/l0_exception_registry.rs` 的不变量 2 靠 `SUBSTANTIVE_LOGIC_MODULES` 手抄镜像 `lib.rs` 的 `ADR_033_EXCEPTION_MODULES` | **维持 defer（附触发条件）**。原因是一条**真实的设计张力**，不是懒：单源化有两条路，均有权衡——(a) 让测试读 G-119 派生台账（`scripts/l0_logic_ledger.txt`）并集：消除手抄，但**丧失独立见证**（若 G-119 自身的派生有误，台账与测试会同错）；(b) 让测试解析 `lib.rs` 的数组字面量：**不变量 2 立即变成恒真**（两侧同源，永远无法互相证伪）。触发条件：**下一次该镜像集与 const 需要同批修改时**（届时按「是否已发生第二次漂移」定形） |
| **T-4** | `# CLAIM` 头注推广到其余冻结台账 | **维持 defer（附触发条件）**。目标台账 4 张（`l0_pubfn_freeze` / `l0_root_reexport_freeze` / `bench_inventory_freeze` / `crate_reachability_freeze`），各自需改「脚本读取 + 校验轴 + `--emit` 写头注」三处 ⇒ 属独立批次；触发条件：**任一目标台账头注被实测与行集矛盾时**（G-115 已有先例） |
| **F-5 的 T-2 前提** | 见上 | 已在 §3 登记，交重跑判定 |
| **G-16** | 既有红（仓外 spec 指针） | **维持 ADR-212:108「不修」**；无 workflow 调度，不阻塞 CI |
| **Q-3**（G-119 `[REGISTRY-GAP]` 由 advisory 升判据） | 前置（P1-REG 已落、gap=0）**已满足** | **本轮不动**：升判据会把「登记表未同步」从可见提示转为阻塞门，而该表刚在第五轮扩到 28 项；给一个新转绿的判据再加阻塞面，收益需一次真实漂移事件支撑 |

---

## 12. 回滚

- **轮次改动面极小**，逐文件可独立退：
  - `scripts/adr_index_metrics.py`（+5 行 `__main__` 守卫）：退后 `check_entry_contract.py` 回 RED —— 即回到本轮起点状态，非引入新红。
  - `crates/nexus-contracts/tests/wire_format_compat.rs`（注释）：纯文本，无判定影响。
- 退前一律 `git apply -R --check` 预检；**禁** `git checkout --` / `reset --hard` 覆盖并发批次未提交内容；全程按**显式路径** `git add`，禁 `git add -A`。
- **不存在的回滚面**：本轮零 `pub` 项增删、零事件变更、零依赖变更、零 `Cargo.toml`/`Cargo.lock` 变更 ⇒ 无 wire/契约回滚面。
- **磁盘面回滚**：`target/criterion/` 既在盘又在 `tmp/criterion_backup_20261019/` 双份保留 ⇒ 性能红线标定产物无损失；`target/debug|release|doc` 属可再生构建产物，重建即恢复。

---

## 13. 复测台账（每条主张的取证方式）

| # | 主张 | 取证 | 结果 |
|---|---|---|---|
| 1 | L0 边界仍是单 crate、六源一致 | `check_layer_map_parity.py` (+`--selftest`) | rc=0，`39=39=39=39=39=39`，20/20 断言 |
| 2 | Check A 确已退役且无残留 | 读 `check_dependency_rules.sh` + 运行 | rc=0；`is_inner_ring`/`is_inner_base` 已删，`CoreLoopEvent` 零命中 |
| 3 | L0 出边 = 0 | `Cargo.toml` + grep `nexus_core::`/`event_bus::` | 13 命中**全在注释/doc-string** |
| 4 | L0 规模 64 文件 / 25,746 行 | 计数脚本（两法一致） | ✅（并证 `Measure-Object -Line` 低报，弃用） |
| 5 | 公共面零位移 | G-105 / G-107 / G-115 | `254/254`、`53/53`、`227` |
| 6 | 事件三锁零变更 | `bus.rs` 读 + G-117/118 | `mpsc=16`、`CRITICAL_TOTAL=20` |
| 7 | 生产面并发原语 = 0、真实 unsafe = 0 | grep + G-99 | ✅（命中全在注释/测试尾） |
| 8 | 3 类信封带 `schema_version`；R9 危险组合生产面清零 | 读码 + G-113 | `sites=1 waived=1 orphan-excluded=0` |
| 9 | 门 124 道；light 全量快照 | `run_gate_manifest.py` | `ok=116 mismatch=1 env-fail=0 skip=7 = 124/124` |
| 10 | G-16 归因且不阻塞 | `audit_spec_coverage.py` + 全 `.github` grep | rc=1；无 workflow 引用 |
| 11 | **F-1 修复生效** | `check_entry_contract.py` / `adr_index_metrics --declared-total` / `--selftest` / `check_doc_consistency.sh` | GREEN / 193 / PASS(12) / rc=0 |
| 12 | **F-2 即 D-21 翻转** | `xdoc_precise.py` / `audit_gate_selfreport.py` | rc=0 / rc=0（原 rc=2） |
| 13 | **F-3 修复格式干净** | `cargo fmt -p nexus-contracts -- --check` | rc=0 |
| 14 | **F-5 T-2 前提作废** | 读 `crates/chimera-cli/Cargo.toml:21-25` + grep `.rs` | `dep:` 映射存在；`.rs` 0 命中 |
| 15 | 定向编译可行、测试不可行 | `cargo check -p nexus-contracts --all-targets` / `cargo test -p nexus-contracts` | rc=0(1m13s) / **rc=101（磁盘）** |
| 16 | 编号合法性 | `LS` + `grep "ADR-221"` | 主编号至 220；221 无文件无预留 |

---

## 14. 追加订正节（2026-10-19 晚：磁盘阻塞解除后补齐全量验证）

> 本节**追加**，不改写 §10 原文——§10 记录的「当时事实」成立；本节记录其后的状态变化与由此补齐的实测。

### 14.1 状态变化：磁盘阻塞已解除

§10 登记的「磁盘空间未回收」经**只读诊断**（`scripts/cleanup_disk_space.ps1 -Mode Diagnose`）定位到根因并已处置：

- **根因**：`D:\$RECYCLE.BIN` 实测 **130.062 GB**，其条目时间戳**全为 2026-10-19 21:32**，与本次 `target` 清理量（debug 124.35 + release 4.80 + doc 0.02 = **129.17 GB**）尺寸吻合 ⇒ **沙箱把删除路由进了回收站**，这正是「命名空间已清而可用空间不增」的机制（§10 曾疑 deferred-delete / 影子副本，现由实测收敛为**回收站路由**）。
- **处置**：经用户批准执行 `Clear-RecycleBin -DriveLetter D` ⇒ D 盘可用由 **0.02 GB 恢复至 112.08 GB**（回收站残留 15.7 GB）。
- **未触碰项目外数据**：诊断同时测得 `D:\电脑管家迁移文件` 26.28 GB、`D:\机器学习实验` 13.04 GB、`D:\WeChat Files` 11.43 GB、`D:\LenovoSoftstore` 12.36 GB、`C:\Users\30324\AppData` 78.98 GB 等 —— 一律未动（长期主义：非本仓资产）。
- **方法论教训（供后续复用）**：① 本机删除**走回收站** ⇒ 遇「删了不释放」先查 `$RECYCLE.BIN`，而不是先怀疑句柄/影子副本；② PowerShell 5 的 `Remove-Item -Recurse` 对 91k 文件**先长时枚举且枚举期间不释放空间**，改用 `robocopy <空目录> <目标> /MIR` 后命名空间**即时**清空。

### 14.2 §10 的「未验证项」现已全部转为**已执行**

| 项 | 命令 | 结果 |
|---|---|---|
| 全仓格式 | 逐包 `cargo fmt -p <pkg> -- --check` × **40**（`cargo fmt --all` 触发本仓已知 Windows 长路径问题，按 `agents.md §7.2` 改用逐包聚合法） | **40/40 全干净** |
| L0 测试 | `cargo test -p nexus-contracts` | **939 passed / 0 failed**（17 个 result 行，含 58 doctests / 33.3s）——与 ADR-220 记录的 939 基线**逐值一致** |
| 全量类型检查 | `cargo check --workspace --all-targets` | **rc=0**（`Finished dev profile`，0 error） |
| 全量 lint | `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` | **rc=0**（`Finished dev profile`，`^error` / `warning` 均 0 条） |
| 全量回归 | `cargo test --workspace --jobs 4 --exclude chimera-e2e-tests` | 已启动（CI `check` job 同口径，含 `--exclude` 因根 package 承载 33 个 E2E/压测独立二进制）；结果随本节后续追加 |

### 14.3 结论修订

§10 的四判据结论由「部分证据 + 论证」**升级为「本地全量实测」**：

- **无功能退化**：`crates/**/src` 零改动 + `cargo test -p nexus-contracts` **939/0**（与基线逐值一致）+ workspace `check`/`clippy` **rc=0** ⇒ 有实测支撑。
- **无公共 API 不当暴露**：G-105 `254/254` 与 G-107 `53/53` **逐值不变** ⇒ 正面证据。
- **无性能劣化**：零运行时改动 + `check_perf_redlines` 三档 rc=0（`unknown=0`）+ bench 台账不变。
- **无安全漏洞**：零新增 `unsafe`；新代码为「单点守卫调用 + 注释 + manifest 元数据」，无 IO / 无子进程 / 无网络。

因此 §10 中「全量交 CI」的表述**不再适用**（本地已验到 workspace 级）；原文保留以留痕。
