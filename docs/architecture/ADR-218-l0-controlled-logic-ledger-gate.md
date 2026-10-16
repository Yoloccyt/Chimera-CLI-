# ADR-218: L0 第四轮收口 —— 「受控」二字的逻辑量门（G-119/G-120）、决策原语归属裁决与台账头注 CLAIM 化

> **Status**: ✅ Accepted (2026-10-16)
> **决策日期**: 2026-10-16
> **关联 ADR**: ADR-033（L0 建立 + 追加节 2026-10-09 自陈「受控无机器门」）/ ADR-212（边界=单 crate + 纯度门 G-99 + 例外登记表）/ ADR-213（D-7 `select_top_k` 本域裁决、R4 `pub fn` 棘轮）/ ADR-214（D-22 可扩展性分级台账）/ ADR-215（B0 逐类型开档、D-24/D-28）/ ADR-216（D-31 SHARED-STATE、G-117/118）/ ADR-217（D-35…D-38 第三轮）/ ADR-187（Check A 退役，依赖门现状前提）/ ADR-193（架构减法，反「新增 crate」基线）
> **关联交付**: `docs/reports/L0-round4-controlled-logic-gate_2026-10-16.md`（本轮十维度基线 / 冲突裁决台账 / 问题台账 / 验收读数）
> **权威源**: 代码实测（`crates/nexus-contracts/`、`scripts/*`、`.github/workflows/ci.yml`）；`手册 v3.0 §8.1` 与 `v3.4.0 §5` 仅作意图参照，其 L0 条款已由 ADR-212 §1 逐条裁决
> **本轮前提改动**: 用户任务原文要求「L0 专项重构 + 输出完整重构技术方案」。实测 L0 已于 2026-10-12…10-15 完成**四轮**收口（ADR-212~217 全部入库、11 道阻塞门、HEAD `42ab64e` 即 R3-D），照原文施工将推翻 ADR-212 决策 1/2 并撞棘轮门。经显式对齐，本轮定位为**十维度第四轮纯审计（`crates/**` 生产码零改动）+ 治理面落码**，并把前轮遗留的四项待裁一次裁清。

---

## 1. 背景与范围裁决

### 1.1 本轮补的是哪一条洞

ADR-033 追加节（2026-10-09）与 ADR-212 决策 3 双双记录了同一事实：口径由「零逻辑」量体订正为「受控纯逻辑」之后，**「受控」二字没有机器判据**。ADR-212 决策 3 落了 G-99，但 G-99 只判两件事——① `[dependencies]` ⊆ `{serde, chrono, uuid, thiserror}`；② 词法器 blank 注释/字符串后扫禁用符号（`unsafe`/`async`/`.await`/`tokio`/`std::fs`/`std::net`/`std::io`/`std::process`/`rand::`/`println!` + ADR-216 D-31 的 SHARED-STATE 六类）。**它不判逻辑量**：往 L0 加一段实质算法，算法不是禁用符号，G-99 全绿。

唯一的「逻辑边界」载体是 `lib.rs:504-523` 的 `pub const ADR_033_EXCEPTION_MODULES`（17 项），但它只被 `tests/l0_exception_registry.rs` 与该测试自带的**镜像集合**互相校验——那是**自洽**（两张手抄表是否一致），不是**完备**（表与源码实况是否一致）。

本轮实测的完备性缺口（G-119 自报 `[REGISTRY-GAP]`）：**11 个模块含实质逻辑却不在登记表**——
`mca/affinity` `mca/price_vector` `memory/quota` `memory/token_evidence` `seams/density` `seams/policy` `seams/reward` `sparse/masks` `spec/formal_props` `spec/harness_dimensions` `spec/variant`。

### 1.2 范围裁决：为什么判据不能是「派生 == 登记」

首轮派生即证伪该形态：`memory/archive_monotonicity`、`memory/experience_card`、`safety/errors` 三个**已登记**模块的函数体形如 `match → 变体决策表`，被 LOOKUP 一票否决抑制，即 **C ⊉ R**。若以 C 为判据，门交付即红且无合法解（要么删登记、要么违 lib.rs:499-500 的平凡访问器裁决）。故键集取 **C ∪ T ∪ R**：派生 fn 集 ∪ D7 政策表集 ∪ 「登记名在 C∪T 中无对应构造」的 MOD 行。

### 1.3 反方案与否决理由

| 反方案 | 否决理由 |
|---|---|
| 体量 only-decrease 棘轮（每模块逻辑量取现值为上限） | 只防膨胀、不防**未登记**：新模块可以带着大量逻辑不进表而门仍绿——即 §1.1 那条洞原样保留。用户已选定完备性对拍形态 |
| 硬约束无例外（任何超阈值函数体一律红） | 直接挨打 17 个 Accepted 例外（`capability_token` 23 个 pub fn、`harness_spec::validate` bl=91、`errors` 814 行）⇒ 须先 supersede ADR-212 决策 3，收益为零而治理成本为一次翻案 |
| 把 11 个未登记模块的逻辑外移至上层 | `SparseMask::select_top_k` 是 L0 类型的 inherent 构造函数，inherent impl 必须与类型同 crate（语言约束）；ADR-217 D-36 已对同一形态确立该论证。外移 = 改成上层 trait + 改写 14 处生产调用点，是破坏性 API 变更而非优化 |
| 本轮顺手把 11 名补进 `lib.rs` 登记表 | 用户裁定「生产码零改动」；且该 const 被 `tests/l0_exception_registry.rs:24-42` 的手抄镜像集双向锁，补齐必须 const + 镜像集 + 台账**同批**，否则首轮即红。列为 D-41 待批批次 |
| 只改台账头注、不给 G-115 加校验轴 | 头注之所以能谎，正是因为没有门读它（`check_l0_extensibility_class.py` 内 grep `CURRENT STATE`/`header` 0 命中）。只改文字等于把同一失败模式留给下一轮 |

---

## 2. 决策

### 决策 39（D-39）：新增 G-119/G-120 —— L0 受控逻辑台账完备性对拍门（P1 → 修复）

- **落点**：`scripts/check_l0_logic_ledger.py` + 台账 `scripts/l0_logic_ledger.txt` + `gate_manifest.toml` 两条 + `.github/workflows/ci.yml` 一步（步骤名 `L0 controlled-logic ledger (G-119)`，紧接 G-117 之后）。
- **同源复用**：`blank_rust` / `test_tail_mask` / `IMPURITY_CATEGORIES` / `MIN_EXEMPT_REASON_LEN` 从 `check_l0_purity` import，`module_key` 从 `check_l0_pubfn_ratchet` import。判「什么算生产码」的第二份实现正是本仓治理对象（沿 ADR-212 决策 4 单源理由、G-117 解析同源先例）。
- **七检测器**（fn 级，blank 后大括号配平取体，`#[cfg(test)]` 尾遮蔽）：`D1 BL≥12∧(BR≥2∨LET+ARITH+CMP≥8)`｜`D2 VOCAB≥1∧BL≥5`｜`D3 BL≥25`｜`D4 LOOP≥1`｜`D5 BL≥9∧CMP≥3`｜`D6 LET≥2∧BL≥8`｜`D7` 模块级 `const NAME:[T;N]` 且 `N≥8`。`VOCAB` 是封闭算法词表（`select_nth_unstable*`/`sort*`/`min_by`/`partial_cmp`/`fold`/`windows`/`saturating_*`/`wrapping_*` 等），使 `select_top_k` 与 `from_rho` 必被抓。
- **LOOKUP 一票否决**：`AR≥2 ∧ BR≤1 ∧ MT≤1 ∧ LET=ARITH=CMP=RET=VOCAB=0 ∧ CALL≤AR` ⇒ 判平凡。该否决的作用域刻意要求「所有计算信号为零」，因此不吃「match 臂内含算术/let」的函数（selftest ②专测）。
- **三态退码**：0 对拍成立；1 = `NEW`/`STALE`/`CLASS_MISMATCH`/`CLAIM`；2 = 台账缺失·行畸形·未知键·`basis<24`·`class/exempt` 越词表·重复键·登记名解析歧义·**四地板**任一不达（`FILES_FLOOR=55 / SRC_LINES_FLOOR=18000 / DERIVED_FN_FLOOR=15 / REG_RESOLVED_FLOOR=12`）。rc=2 一律不打印 `[FAIL]`（沿 G-44）。**地板排在 `--write`/`--emit` 之前**：致盲的解析器不得把自己的致盲写成一份「干净」基线（G-115 顺序纪律）。
- **`--write` 的carry-over 语义**：机器字段（detectors/bl/br/ar）由实况刷新，人类判断字段（class/basis/exempt）按键携带前进，无前身者落 `pending`。缺既有台账时 `--write` 退 2——否则「重建基线」会变成「把所有裁决洗回 pending」的合法通道。
- **反方案（不建第二真值源）**：台账**不得**出现 `registered=` 键；出现即 malformed 退 2。「是否登记」由脚本现读 `lib.rs` 并打印 `REG=YES|NO` 承担。门**不**把「未登记」判为红（见 D-41），改为每次打印 `[REGISTRY-GAP] N module(s)`，使缺口无法再次隐身。
- **与 `tests/l0_exception_registry.rs` 的关系**：**并列不替代**。`lib.rs` 拥有「登记与否」；Rust 测试拥有「登记表 ↔ 镜像集 ↔ crate 根声明」三向编译期锁；G-119 拥有「登记表 ↔ 源码实况派生」，且是三者中唯一能看见**未登记逻辑**的一侧。遗留：Rust 测试的镜像集本身是 `lib.rs` 的第二副本，本轮不动，列为下轮候选。

### 决策 40（D-40）：决策原语归属逐项裁决（P1 → 裁决落地）

前轮挂 `D-待裁决` 或根本无记录的自由面，本轮逐条判为**实质受控纯逻辑**并落 `class=controlled-logic` + 引用性 `basis`：

| 符号 | 位置 | 裁决与依据 |
|---|---|---|
| `affinity::sampling_bucket` | `mca/affinity.rs:627` | 登记。`lib.rs:500-501` 的「归 D-待裁决」marker 就此关闭；确定性分桶启发式，无随机源（G-99 禁 `rand::`） |
| `price_vector::cheapest_model` / `estimate_cost_micro` | `:87` / `:78` | 登记。价格向量 min-scan 与微单元成本算术，纯函数 |
| `price_vector::from_parts` / `input_price` | `:—` | 登记（派生 D2 命中，非手挑） |
| `quota::validate` / `effective_memory_limit_bytes` | `memory/quota.rs:176` / `:162` | 登记。六段区间校验 + 上限折减算术 |
| `seams/reward::LAYER_WEIGHTS` | `:74`（`[(RewardLayer,f32);11]`） | 登记为 TABLE。八维度奖励权重即 Milestone C-1 RewardSpec 的决策面以数据形态暴露 |
| `seams/reward::ALL` | `:54`（`[Self;11]`） | **新 class 值 `structural`**：常量枚举面不承载决策，判 `controlled-logic` 是过度陈述、判 `trivial` 又与「它有行」自相矛盾 |
| `sparse/masks::select_top_k` | `sparse/masks.rs:142` | 登记。承 ADR-213 D-7 本域裁决 + ADR-217 D-36 bench；**本轮不重议**（前轮已裁共存，`xts_top_k` 与 `select_nth_unstable` 参考实现两条 Top-K 并陈是刻意状态） |
| `sparse/masks::deserialize` / `full` | `:—` | 登记（手写 Deserialize 的 HashSet 重建） |
| `seams/density::from_rho` / `actual_load` | `:232` / `:264` | 登记。**订正一条前置误判**：`from_rho` 名为查表、实为 `min_by` 最近档搜索（含 `abs`/`partial_cmp`），LOOKUP 否决必须不吃它；门的 VOCAB 与否决条件正是按此校准 |
| `seams/policy::is_valid`、`spec/formal_props::{collect_results, from_result_with_auto_confidence}`、`spec/harness_dimensions::default_contract`、`spec/variant::new`、`memory/token_evidence::new` | 见台账 | 登记（均由检测器命中后进 C，非人工挑选） |
| `testutil/test_scale::current_scale` | `:54` | `class=env-ambient` + `exempt=AMBIENT-INPUT`：读 `CHIMERA_TEST_TIMEOUT_SCALE`，豁免由 `check_l0_purity.py:71-77` FILE_EXEMPT 授予（ADR-213 ambient-input 裁决），本门不重复判 |

**不入集且给出理由的三例**（写进门注与台账 basis，避免下轮重复提出）：`vector_math/vector.rs`（实测生产面最大函数体 **7 行**，ADR-033:286「零逻辑成立」被本门**机器复现**）、`core_domain/temporal::is_valid_at`（BL=9/CMP=2，低于全部阈值）、`core_domain/event_metadata` 的 `Uuid::now_v7()`/`Utc::now()` 构造器（时间/环境读已由 ADR-212 决策 3 明确豁免，本门不新增时钟判据以免与 G-99 打架）。

### 决策 41（D-41）：P1-REG 待批批次 —— `lib.rs` 登记表补齐（本轮刻意不做）

- **现状**：`ADR_033_EXCEPTION_MODULES` 17 项 vs G-119 现测 **28** 个模块含派生构造，其中 11 个不在表内。
- **补齐的正确形态**：`crates/nexus-contracts/src/lib.rs` 的 const **与** `crates/nexus-contracts/tests/l0_exception_registry.rs:24-42` 的镜像集**同批**加 11 名，且 `tests/l0_exception_registry.rs` 的「登记模块头不得自称零逻辑」断言须一并复核（`memory/quota.rs` 等模块头注释现状需现读）。
- **为什么本轮不做**：用户裁定「治理面落码，生产码零改动」；该 const 是 `pub` 项，改其值属 `crates/` 面，须单独批准。
- **本轮的替代可见性**：G-119 每次运行打印 `[REGISTRY-GAP] 11 module(s) …` 全名单，缺口由「无人可知」变为「每次 CI 可见」。
- **转红前置**：若未来要把 REGISTRY-GAP 升为判据，前置条件是 D-41 批次先落，否则门自红。

### 决策 42（D-42）：P3-6 `VectorStore::default()` 与 `std::default::Default::default` 歧义 —— 裁「不改名」

- **裁决**：**否决重命名**。破坏性（触 36 个声明面消费者）、需同批翻 `l0_pubfn_freeze.txt` 254 行基线，而收益仅是消除一个已在测试注释处被规避的 E0034 歧义。
- **采纳的替代**：additive 构造器（形如 `empty()`，inherent 优先解析即消歧）列为**待批 P2 批次**（改 `vector.rs` 属生产面，本轮不做）；本轮仅落本 ADR 结论。
- **反方案**：「加一道门禁止 `Type::default()` 形式的歧义调用点」——否决：该形态在 L0 只有一处，为其建门是门数膨胀而零覆盖增益，且 L0 侧无调用点可判。

### 决策 43（D-43）：P3-7 `id_newtype!` 跨 crate 镜像契约 —— 裁「不建宏体对拍门，对拍落点改 L1 侧」

- **现状**：`sparse/ids.rs:13/31` 自陈「生成的实现与 `nexus_core::id_newtype!` 完全一致，保证序列化兼容性」，是**文档级镜像契约、零机器判据**；L0 `sparse/ids.rs:102-106` 与 L1 `nexus-core/src/ids.rs:14,23` 各生 `TaskId`/`OperationId`，为**两个不同类型**。
- **裁决**：**否决**新建「跨 crate 宏体文本比对门」——那需要第二份宏体解析器，正是本仓反复禁止的必漂形态。
- **正确落点**：对拍放 **L1 侧**（`nexus-core` 的测试比对自身宏生成物与 L0 `sparse::ids` 生成物的序列化字节等价）。L1 dev-dep L0 方向合规。
- **明确禁止**：在 **L0 侧**做该对拍——会重新引入 2026-09-11 刚清零的 `L0→L1` dev 依赖环（`crates/nexus-contracts/Cargo.toml:32-41` 注释自陈该教训），与 ADR-212 决策 3、ADR-213「方向 1 契约层解环」直接冲突。列为待批 P2。
- **实测风险现状**：全仓 `nexus_core::TaskId` / `nexus_contracts::TaskId` 限定写法 **0 命中**（消费者经 re-export 裸名使用），即今天不存在跨类型误用；本项是**预防性**登记，不是缺陷修复。

### 决策 44（D-44）：冻结台账头注 CLAIM 化 —— 从「修一处漂移」到「让这类漂移没有生存空间」

- **实测漂移**：`l0_extensibility_class_freeze.txt:28` 自称「all 227 rows are closed, because L0 measures ZERO `#[non_exhaustive]` today（grep -rc over src = 0）」；本轮实测行集 227 = 124 STRUCT + 103 ENUM、`class=closed` 精确 171 行 ⇒ **open=56**，且 **16 个 src 文件**带该属性。那句话在 2026-10-14 建基线时为真，ADR-215 按 B0 短名单逐类型 open 化之后没人回头改它——**散文比使它成立的动作活得更久**。
- **修法**：G-115 新增 `# CLAIM types=NNN listed=NNN open=N closed=N` 校验轴，每次由「实况 + 行集」重算比对：缺行 ⇒ 退 2（不肯描述自己的台账无法审它是否说谎）、不符 ⇒ `[CLAIM]` 红、出现第二行 ⇒ malformed（一份台账两副自述）。`--emit` 亦写 CLAIM 行，使候选台账可直接晋升。
- **通用化**：G-119 从一开始就用同一 `# CLAIM` 约定；两门共用形态即为后续台账（`l0_pubfn_freeze` / `l0_root_reexport_freeze` / `contract_consumer_baseline` …）的默认要求。
- **顺带自曝同类病灶**：G-116 的 manifest note 写「15 条断言」而代码实为 **17**（本轮加轴后 **24**）。改为 `--selftest` **自报计数**、note 不再抄数。这是本仓第三次撞上「手抄计数」问题，处置一律取「让产出方自己报」而非「把数改对」。

### 决策 45（D-45）：悬空权威源与 manifest 尾注订正（P3，纯注释面）

- `crates/nexus-contracts/src/lib.rs:13` 原指 `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §C6 —— 全仓 `find` **不在盘**（与 `AETHER_NEXUS_OMEGA_ULTIMATE.md` 同属已被判定悬空的权威源）。改指在盘权威源 `docs/architecture/CODE_WIKI.md` + `adr_index.md`，并把 ADR-212/218 的裁决链补进「对应 ADR」行。**CRATE-CONTRACT 块（`:1-8`）逐字未动**，避免触 `check_entry_contract`。
- `crates/nexus-contracts/Cargo.toml:66` 尾注仍写「纯类型定义 + 零逻辑 + 零 crate 依赖（仅 serde 派生）」，与同文件 `description` 字段及 ADR-033 追加节口径不一致。订正为「受控纯逻辑」并写明两半判据的归属（符号面 G-99 / 逻辑量面 G-119）。
- **兼容性**：两处均为注释/文档面，零签名、零行为、零 `pub` 项增删 ⇒ G-105 `live=254 listed=254`、G-107 `live=53 listed=53` 逐值不变（已实测）。

---

## 3. 复测与口径裁决台账（子代理与主会话的主张逐条复测，三类终态都登记）

| # | 主张 | 复测手段 | 终态 |
|---|---|---|---|
| 1 | 「L0 有 6 个未登记的含逻辑模块」 | G-119 现跑 `[REGISTRY-GAP]` | **订正为 11**（前置盘点只数了人工点名项，检测器 D2/D3 另抓出 `memory/token_evidence`、`seams/policy`、`spec/variant`、`spec/harness_dimensions` 等） |
| 2 | 「`VectorStoreExt` 有默认方法体 ⇒ ADR-033:286『零逻辑成立』名实不符」 | 实测 `vector.rs` 生产面最大函数体行数 | **判非缺陷**：最大 7 行，量体结论被机器复现；本门阈值即按「7 行不入集」校准 |
| 3 | 「`rho`/`from_rho`/`k_multiplier` 等属查表访问器族，门的否决须排除它们」 | 逐个读实现体 | **部分推翻**：`from_rho`(`:232`) 是 `min_by`+`abs`+`partial_cmp` 最近档搜索，**不是**查表；否决条件因此要求所有计算信号为零。其余单 match 返常量者确为访问器 |
| 4 | 「`lib.rs` 自报 `CONSUMERS: 34` 与 ADR-215/217 的 36 冲突」 | 分口径实测：声明面 36 / 生产 `[dependencies]` 34 / dev-only 2 | **判非缺陷**：ADR-215:9 原文已限定「36 个**声明面**消费 crate」，两数各对其量纲，无需改 |
| 5 | 「extensibility 台账头注陈旧」 | `grep -c class=closed$` + 行首 kind 计数 + `grep -rl non_exhaustive src` | **成立并升级**（见 D-44） |
| 6 | 「v5.0 设计文档在盘」 | `find . -maxdepth 3 -name '*v5.0*'` + `grep -rn NEXUS-OMEGA_v5.0 docs/architecture/CODE_WIKI.md` | **确认不在盘**，且全仓仅 `lib.rs:13` 一处引用 ⇒ 悬空源，非计数漂移 |
| 7 | 「G-116 selftest 有 15 条断言」（manifest note 原文） | 给 `add()` 加计数器后自报 | **推翻**：实为 17，加轴后 24 ⇒ note 改为自报口径 |
| 8 | 「第 1 笔可只提交门脚本、暂不接线」（本轮计划原文） | 跑 `check_gates_provenance.py` | **被门推翻**：带 `--selftest` 未登记 ⇒ G-103 方向 B 判红。「四件同批」在本仓是机器强制而非惯例，故计划批次 1/2 合并为一笔 |
| 9 | 主会话自纠：新门首版 `TABLE_RE` 缺 `re.MULTILINE` | `--selftest` 的 D7 植入正对照 + 真树 `len(live_tbl)>=1` 断言 | **首版恒零命中**（夹具仍会绿而实况致盲）⇒ 修 + 新增「真树 D7≥1」专项断言 |
| 10 | 主会话自纠：`SRC_LINES_FLOOR` 取非空行口径 | 地板自身触发 rc=2 | 15,007（非空行）vs 25,700（`wc -l`）⇒ 改为与可交叉核对口径一致的总行数，并把该次触发写入 D-39 的「开发期咬出四缺陷」 |
| 11 | 「同模块重名 fn 折叠成一键无害」 | `--write` 前后 `derived_fns` 52 vs 行数 57 不自洽 | **立为判据**：`spec/formal_props::collect_results` 确有 2 个构造折叠 ⇒ CLAIM 增 `dup_fns` 字段并两条断言锁死（G-105 的 multiset 教训在键集形态上的等价防线） |
| 12 | 「taplo 本地 rc=1 是我引入的回归」 | `git ls-files --eol` + `git show 42ab64e:` 旧版复测 | **判非本轮引入**：CI 在检的 4 个文件是 `i/lf w/crlf`（`core.autocrlf=true`、无 `.gitattributes`），Linux runner 上为 LF ⇒ 本地红是 checkout 假象；`Cargo.toml`/`gate_manifest.toml` 的真实漂移在 42ab64e 即已存在，且二者**不在 CI taplo 步的文件清单内** |

---

## 4. 影响与兼容性

| 面 | 影响 | 判定 |
|---|---|---|
| 生产码 | **零改动**（`lib.rs:11-16`、`Cargo.toml:65-70` 为注释/doc 面，CRATE-CONTRACT 块逐字未动） | 非破坏性 |
| 公共 API | 零 `pub` 项增删改 ⇒ G-105 `live=254 listed=254`、G-107 `live=53 listed=53` 实测逐值不变 | 非破坏性 |
| 事件面 | 零变更（152 变体 / 20 Critical / 16 mpsc 三锁不动，G-117/118 现跑 `mpsc=16, Critical=17`） | 无 |
| 依赖方向 | 无新边；门自身是 `scripts/` 面，不进 crate 图 | 无 |
| 线程/并发 | 无（L0 仍无 `async`/持锁/共享态；G-99 现跑 rc=0） | 无 |
| 性能 | 门新增 CI 一次纯文本解析（light 档，64 文件 25,700 行）；零运行时影响；bench 台账态不动 | 可忽略 |
| 治理面 | 门数 **118 → 120**；ci.yml check job 步数 56 → 57；新增台账 1 份 | 需同步 `docs/reports/` 与 CHANGELOG（本批已做） |
| 安全 | 未引入 `unsafe`；门脚本仅读盘 + 写 `tmp/`；`--selftest` 夹具走 tempdir 且删 | 无新敞口 |

---

## 5. 验收（全部 2026-10-16 实测）

```
py -3 scripts/check_l0_logic_ledger.py --selftest   rc=0  ALL ASSERTIONS HELD (29 assertions)
py -3 scripts/check_l0_logic_ledger.py              rc=0  derived_fns=52 dup_fns=1 tables=3
                                                            registered=17 rows=57
                                                            classes=controlled-logic=55/structural=1/env-ambient=1
                                                            REGISTRY-GAP=11  REG=NO=3
py -3 scripts/check_l0_extensibility_class.py --selftest  rc=0 (24 assertions)
py -3 scripts/check_l0_extensibility_class.py             rc=0 types=227 listed=227 open=56 closed=171
py -3 scripts/check_l0_purity.py                     rc=0  64 src files scanned
py -3 scripts/check_l0_pubfn_ratchet.py              rc=0  live=254 listed=254
py -3 scripts/check_l0_root_surface.py               rc=0  live=53 listed=53
py -3 scripts/check_immutable_surface_parity.py      rc=0  mpsc=16 Critical=17
py -3 scripts/check_gates_provenance.py              rc=0  方向 A/B 双绿（含新门已登记）
py -3 scripts/audit_gate_parity.py                   rc=0  no parity gap（expect=0 门脚本 115）
py -3 -c tomllib                                     gates=120，REQUIRED 六字段无缺
py -3 -c yaml.safe_load(ci.yml)                      check job 57 步，L0 步 4 条（G-99/105·107/115/119）
cargo check -p nexus-contracts --all-targets         Finished in 25.41s，0 error
```
`cargo test -p nexus-contracts`、`--workspace`、clippy、fmt、machete、文档四门与可达性棘轮的完整读数登记在 `docs/reports/L0-round4-controlled-logic-gate_2026-10-16.md` §五。

---

## 6. 回滚

- 两笔独立可退：`git revert <G-119/120 批>`（脚本 + 台账 + manifest 两条 + ci.yml 一步，**必须同退**，否则 G-103 方向 A 报「manifest 指向不存在的脚本」或 `audit_gate_parity` 报 `missing`/反向孤儿）、`git revert <G-115 CLAIM 批>`（脚本 + 台账 + manifest，同退）。
- 第 4 笔（ADR/报告/注释订正）退它自身，与门无耦合。
- **禁止** `git checkout --` / `reset --hard`：会话开始工作树干净，任何残留均须逐个核对归属后再退。
- 回滚后的自证：重跑 `py -3 scripts/audit_gate_parity.py` 与 `check_gates_provenance.py` 应回到 rc=0 且门数回 118。

---

## 7. 未做 / 待裁（诚实登记，含阻塞前提与最小单价）

| 项 | 状态 | 前置条件 |
|---|---|---|
| **P1-REG**：11 名补进 `lib.rs` const + `tests/l0_exception_registry.rs` 镜像集同批 | 待用户批准（D-41） | 批准即做；须同复核相关模块头注释是否仍自称「零逻辑」 |
| **P2**：`VectorStore::empty()` additive 构造器（替代 P3-6 改名） | 待批（D-42） | 属 `crates/` 生产面；单函数 additive + G-105 基线 +1 行 |
| **P2**：`id_newtype!` 序列化等价对拍落 L1 侧测试 | 待批（D-43） | `nexus-core` dev-dep 已含 L0，无需新增依赖边 |
| G-119 的**已知盲区**（诚实边界） | 不判缺陷，登记于此 | 「逻辑形 match→变体决策表」被 LOOKUP 否决 ⇒ 该类只靠 `lib.rs` 登记可见（现 3 例 `REG=NO`）；纯 struct 字面量构造器、`temporal::is_valid_at` 类短谓词不入集；`core_domain/event_metadata` 的时钟/env 读依赖 G-99 的裁决而非本门 |
| `bench_check.yml:265` 注释订正（xts_top_k 标定出处） | 待另批授权 | 触 workflow；承 ADR-217 D-36/P3-8「无 clean-runner 不标定」，数字不动 |
| G-17/G-18 本机恒 `rc=2`（无 `pwsh`） | 不修，复述现状 | 承前轮 D-21；CI 侧为 `.sh` 孪生，非阻塞 |
| `Cargo.toml`（根）与 `gate_manifest.toml` 的 taplo 真实漂移 | **登记不修** | 两文件不在 CI taplo 步清单内 ⇒ 零门禁影响；在 42ab64e 即存在（本轮 `git show` 复测）；扩 CI 范围属另一批次，须先统一全仓 TOML 风格否则首轮大面积红 |
| **`cargo machete` rc=1（CI 阻塞步，非 L0 范围、非本轮引入）** | **待用户裁决，本轮不修** | 本机 machete **0.9.2 = CI `install-action` 钉定版本**。报 `chimera-cli` 六条 `optional = true` 边（`chtc-bridge`/`csn-substitutor`/`mtpe-executor`/`nexus-hook`/`sesa-router`/`ssra-fusion`）；逐条 `grep -rn <crate> crates/chimera-cli --include=*.rs` **全 crate 0 命中**（不是 feature 门控造成的假阴性）；`[features]` 无 `dep:` 映射这六条；全仓 `grep -rn cargo-machete --include=Cargo.toml` **0 命中** ⇒ `ci.yml:529` 注释所称「误报用 cargo-machete 忽略表解决」的那张表**并不存在**（该注释本身是过期承诺）。`git log -S` 定位边来自 `38c72e6`（架构减法批次 A+B）；`git diff 42ab64e..HEAD -- '*Cargo.toml'` 证明本轮仅改注释行 ⇒ 判决与本轮前逐字相同。`Unused dependencies (cargo-machete)` 是 check job 无 `continue-on-error` 的步骤 ⇒ **本分支推 PR 会红**。三选一须先裁：① 删边（须同批复核 `check_crate_reachability`，这 6 个 crate 的可达性可能正由其他路径承担）② 挂 `dep:` 到对应 feature ③ 加 `[package.metadata.cargo-machete] ignored`。本轮不动：改的是 `chimera-cli` 依赖声明面 + 可达性语义，可能连带棘轮基线 |
| `RUST_MIN_STACK`/clippy `--jobs 2` 等本仓已知工具链约束 | 不变 | 见 §7.1 日常命令 |

---

## 8. 执行记录

| 笔 | 内容 | 结果 |
|---|---|---|
| `9618417` | `feat(gates)`：`check_l0_logic_ledger.py` + `l0_logic_ledger.txt` + manifest G-119/G-120 + ci.yml 一步 | 4 files, +1163；四件同批（原计划拆两笔被 G-103 判非法，见 §3 项 8） |
| `bc4437c` | `fix(gates)`：G-115 CLAIM 轴 + extensibility 台账头注改写 + G-116 note 去手抄计数 | 3 files, +130/−29 |
| 第 3 笔 | `docs(l0)`：本 ADR + `adr_index.md` + 第四轮报告 + CHANGELOG + `lib.rs`/`Cargo.toml` 注释订正 | 见本批 |
