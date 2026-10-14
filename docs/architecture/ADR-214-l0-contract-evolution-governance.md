# ADR-214: L0 契约演进治理 —— F4′ 位置式字段序冻结门落地、D-14「零逻辑」以名实一致性关闭、`#[non_exhaustive]` 与 wire 层正交裁决、装配消费者四口径定义

> **Status**: ✅ Accepted (2026-10-14)
> **决策日期**: 2026-10-14
> **关联 ADR**: ADR-033（L0 建立 + 2026-10-09 追加节量体订正）/ ADR-049 决策 1（0 新 crate）/ ADR-187（Check A 退役，δ「契约下移」否决理由复用）/ ADR-212（决策 1–5 全盘继承）/ ADR-213（**R8.2 + R9 是本 ADR 决策 1 的规范源**；§6.5 对 D-14 的处置被本 ADR 收口）
> **关联交付**: `scripts/check_positional_field_order_freeze.py`（G-113/114）+ `scripts/positional_field_order_freeze.txt` + `crates/nexus-contracts/tests/l0_exception_registry.rs` 不变量 4 + `docs/reports/L0-专项重构总交付_2026-10-14.md`
> **权威源**: 代码实测（`crates/*/src`、`scripts/`、`.github/workflows/`）；两份设计文档（v3.4.0 / 十六源手册 v3.0）仅作意图参照，其 L0 章节已被 ADR-212 决策 1 归档

---

## 1. 背景

W13 开工前的实测事实：**ADR-213 已把两条编码铁律写成规范，但判据为零**。

- R8.2 要求"每个 Class B 持久化类型登记 `类型::[字段名有序表]`，源码字段序与台账不符 ⇒ 红"，R9 要求"位置式持久类型禁止新增 `default + skip` 字段"。而 `scripts/positional_field_order_freeze.txt` **不存在**（`ls` 实测），`gate_manifest.toml` 无任何门引用这两条 ⇒ 规范处于"已裁决、无执行证据"态。
- 它们守护的失败模式恰是**唯一不留信号的形态**，且由 `crates/nexus-contracts/tests/wire_format_compat.rs` 实测钉住（非推演）：array 编码下①中间插入字段 ⇒ 显式 `invalid length`；②尾部追加 + `#[serde(default)]` ⇒ 仍可回读；③**两个同类型字段交换声明顺序 ⇒ 解码 `Ok` 而值静默互换**。既无编译错也无测试失败，人工评审是唯一防线时其失败模式即静默数据腐败。
- `#[non_exhaustive]` 在 L0 **实测 0 命中**（`grep -rc` 全 src 零）。它上一轮被挂在"待 D-19 类裁决"，而 D-19 实际裁的是 wire 划界 ⇒ 该项成为无人认领的孤儿；ADR-213 R8 反方案③ 只否证了"以 `#[non_exhaustive]` **代替**字段序冻结"，并未否证其自身价值。
- D-14（"26 处「零逻辑」措辞是否纳入禁用词"）与 CONSUMERS 计数口径均需裁决，且本轮取证**推翻了本 ADR 起草前的一条计划假设**（见 §6）。

## 2. 决策

### 决策 1：落地 F4′ 门（G-113 真实 / G-114 selftest），把 R8.2 + R9 变机器判据
- 四件同批：`scripts/check_positional_field_order_freeze.py`、`scripts/positional_field_order_freeze.txt`、`gate_manifest.toml` 两条、`ci.yml` 一步（`--selftest` 先行再真实裁决）。理由：`audit_gate_parity` 判红"expect=0 却无 workflow 调用"，`check_gates_provenance` 方向 B 判红"有 `--selftest` 未登记清单" ⇒ 拆开任一件即制造新红，上一波已两次实证该联动。
- **键 = `<文件>::<所在 fn>`，不用行号**：W3 物理搬移实测使 L0 全部行号漂移，行锚台账会因一次纯搬迁整表假红；`fn` 名对插行/搬迁免疫，解析不到 ⇒ `[TOOL-ERROR]` 退 2 而非静默。
- **判据只取 `to_vec(`，不取 `to_vec_named(`**：具名 map 形态对换序免疫（实测），纳入只会造出风险不存在的假红。
- **形态判定必到"写点"粒度**：同一类型可同时有 named 与 array 写点——`Checkpoint` 实测正是（生产落盘已切具名 / `popup.rs:249` 非持久 array / `wal.rs:135` 孤儿 array）。⇒ "类型 + 声明形态"单键**已被此实例否证**（形态不是类型属性，是写点属性）。
- **与 G-109 联动**：`src_module_reachability_freeze.txt` 内文件的站点入 `ORPHAN-EXCLUDED`（不记账但打印计数）。该文件一旦接线即离开 G-109 台账、落入本门面 ⇒ 判 `NEW`。不联动则"接线当天"正是静默漏判窗口。
- **词法器复用而非重写**：`import check_l0_purity` 取 `blank_rust` + `test_tail_mask`（沿 G-105/G-107 先例）；两份"什么算生产码"正是 G-45/G-46 要治理的漂移病灶。
- 现状实测（走 799 个 `crates/*/src` 文件）：生产位置式写点**恰 2 处**——`wal.rs::append_entry`（G-109 孤儿，排除）、`popup.rs::event_detail`（取证后 EXEMPT：`:249` 编的字节在 `:263` 同函数即被解回、仅经 `hex_string()` 上屏，`payload_raw` 无其它消费点，popup.rs 全文无 fs/write/save，且被编码物是 `serde_json::Value` 本无声明序可冻）。⇒ **Class B 现存 0 员，本门语义是"新增准入"而非"存量清算"**，此为实况而非缺陷，如实登记。

### 决策 2：D-14 以「名实一致性不变量」关闭，不采全局禁用词
- 盘上「零逻辑」命中分两形：**自我主张形** `//! - **纯类型 + 零逻辑**: 仅类型定义…`（断言本模块性质）与**引用叙述形**（`capability_token.rs:10`「本模块**不是**"零逻辑"」、`mcsm.rs:18`「显式声明为…的例外」、`util.rs:14`「受控例外」、`archive_monotonicity.rs:22` 转述 ADR-033 原文）。二者语义相反，全局禁用词会把叙述形一并判红 ⇒ **不可机判为"禁用"，可机判为"矛盾"**。
- 裁定不变量：**登记于 `ADR_033_EXCEPTION_MODULES` 的模块，其模块头不得再以主张形自称零逻辑**。落地为 `tests/l0_exception_registry.rs` 不变量 4（第 5 测），并附规模守卫（解析到的登记模块数必须等于登记表长度，防覆盖面静默缩水）。
- 现取冲突集：主张形共 **13 个文件**使用，其中**仅 1 个**在例外登记表内——`spec/behavior_contract.rs:9`（自称"仅类型定义与基础构造函数"，实有 `enforce` 做子串覆盖判定）⇒ 已改为陈述实际形状。其余 12 个未登记模块的自称与其"不在例外表"的实况一致（`core_domain/domain.rs` 复核：生产面只有 `default_priority`/`Default::default`，其余全在 `#[cfg(test)]` 之后）。
- **未登记模块使用主张形是合法的**：登记表语义为"含实质逻辑者须登记"，无逻辑者宣称无逻辑与实况一致 ⇒ 本轮不批量改写其余 26 文件的措辞。
- **否决反方案「并入 G-99 或 G-41」**：G-99 判据语义是"注释/字符串 blank 后不参与判定"，塞入文档词判定会毁其 CLEAN 夹具自证；G-41 自述刻意收窄为"markdown 表行 PascalCase 名是否存在于代码"，与本条（模块头自述 vs 登记表）不同轴，且本仓纪律是"轴分工写进 note、禁止互抄基线"。新门的收益也不匹配：为一个已修缺陷占用一对门号 + 一次 ci.yml 写窗口，而不变量测试在每次 `cargo test -p nexus-contracts` 即为阻塞。

### 决策 3：`#[non_exhaustive]` 与 wire 字段序是**正交两轴**，本轮不批量加标、登记为 D-22
- 正交论证：`#[non_exhaustive]` 约束**源码级**跨 crate 结构体字面量构造与穷尽 `match`；serde derive 不消费该属性 ⇒ 字节零变更；反之它不防 wire 层换序。两轴各自需独立锁，互不替代（此即 ADR-213 R8 反方案③ 被否证的准确边界——它否证的是"替代"，不是"价值"）。
- 现状敞口：L0 无一项标 `#[non_exhaustive]` ⇒ 下游可对 L0 公开枚举穷尽匹配、可用字面量构造 L0 公开结构，**L0 增任一变体/字段即源码级破坏**。上一轮 §12.3 的 E0063（漏跑 `--all-targets` 即暴露同 crate 字面量）与 `ExperienceCard` 61 处外部字面量散布 9 crate 是同一敞口的两次实测显形。
- 分档与本轮结果：**档 1 已落地为机器锁**（G-115/G-116 `scripts/check_l0_extensibility_class.py` + `scripts/l0_extensibility_class_freeze.txt`，227 行全 `closed`，零源码改动）。判据是语义双向锁 `class=open ⇔ 带 #[non_exhaustive]`，故"开放某类型"必须同批做两件事（翻行 + 加属性 + ≥24 字符理由），只做一半即 CLASS_MISMATCH 红。台账基线由 `--emit` 从实况派生（属性命中 0 ⇒ 全 closed），因此**它不新增任何决定，只把从未显式声明的默认值变成可评审、可回退的登记**。
- 档 2 候选集**已由本会话自测**（不复用子代理值）：以"类型名在其它 crate 的 src/**tests**/benches/examples 与根 `tests/` 里零文本出现"为充分条件，实测 **21 个 `pub enum`** 合格、**64 个 `pub struct`** 无外部 `Name {` 字面量；L0 非标 derive 实测为 `none`，故原拟的"derive 排除集"经验上无对象。**先前登记的 24/103 是错的且方向偏松**：首轮只扫 `crates/*/src` 得 22 个，补入根 `tests/` 后 `VerificationMethod` 被证实在 `tests/e2e/formal_verifier{,_m1,_m2}_e2e.rs` 三处出现 ⇒ 若照 22/24 加标会在下游 e2e 的穷尽匹配处编译断裂。教训固化：**"零提及"类否定式主张的射程必须显式覆盖根 `tests/`**。
- 反方案「21 个全加标」——**不采纳为默认**，理由不是美学而是本仓自己的承重机制：`event-bus/src/registry.rs` 的 `define_event_registry!` 展开 5 个无通配 match，正是把"新变体必须被每个消费方处理"当安全属性用；候选集里 `ErrorLayer`/`RecoveryStrategy`/`ToolFailure`/`EventFailure`/`GateVerdict`/`CertifyVerdict`/`PlanViolation`/`JudgeTier` 属该类，加标后未来新增变体会静默落入 `_` 臂。故档 2 改为**逐类型经 G-115 台账显式开档**（每次开档=一行理由 + 一处属性 + 全量编译取证）。
- 档 3（`ExperienceCard` 61 处外部字面量等）需 `--workspace --all-targets`：本轮磁盘已放宽到 6.1 GB 并实测 `cargo check --workspace --all-targets` **rc=0 / 3m39s / 磁盘 -0.2 GB**，故该档不再受"无验证条件"阻断，仅受"逐类型政策未定"阻断。

### 决策 4：装配消费者口径定为四把尺，权威值由机器派生
| 口径 | 值 | 定义 | 权威性与用途 |
|---|---|---|---|
| 生产装配消费者 | **34** | 非-optional `[dependencies]` 边指向 nexus-contracts 的 crate 数 | **唯一权威**：由 `crate_contract.py:286 derive_consumers` 派生、`:324` 写入 `lib.rs:6` 头，受 G-59 与 `check_contract_consumers_parity.py`（`ci.yml:298`）锁 ⇒ **散文不得手抄改此值** |
| 边冻结生产边 | 32 | `dep_edge_freeze.txt` 中 `|nexus-contracts` 行 | 冻结台账口径，与装配口径可因豁免而差 |
| 声明行所在 manifest | 36 | 任何位置（含 dev/build 段）声明 `nexus-contracts` | 仅用于"总声明面"，禁止当消费者数引用 |
| 本会话 shell 粗测 | 30 | `grep -A20 "[dependencies]"` 截断 | **假阴性示范**：数组超 20 行即漏 ⇒ 记录为取数教训，不得引用 |

## 3. 影响

| 维度 | 影响 |
|---|---|
| 代码 | `spec/behavior_contract.rs` 模块头 4 行（纯文档）；`tests/l0_exception_registry.rs` +5 测函数与 1 helper（dev 面）；`session-store/src/tree.rs` 注释 6 行（D-17 过度归属关闭）；根 `Cargo.toml:52` 注释路径订正（D-漂移）；`agents.md §2.2` 补 `L(N) → L(0)` 一行（D-15） |
| 新门 | `gate_manifest.toml` +2（G-113/114，号现取：开工时最大 G-110，落盘前复测已变 G-112）；新脚本 1 支 + 新台账 1 张（首跑 live 站点 2、sanctioned 0、waived 1、orphan-excluded 1） |
| CI | `ci.yml` +1 步骤（G-109 步骤之后，先 selftest 后裁决）；本地 taplo 红为 CRLF + `tmp/` 被排除双假信号且根 manifest/gate_manifest **不在 CI taplo 清单内**，不构成本波风险 |
| 事件 | `NexusEvent` 变体/severity/mpsc 三锁**零变更**（152 / 20 / 16） |
| crate 数 | 不变（39）；L0 仍单 crate（ADR-212 决策 1 延续） |
| 公共 API | 零变更：无新增 pub 项（G-91 即刻要求 ≥2 消费者，本波刻意不触）；G-105/G-107/G-99 复跑 rc=0 |
| 兼容性 | 100% 向后兼容（纯文档 + 新门 + 新测试；R8.2/R9 自此有牙但不改任何既有字节序列） |

## 4. 验收判据（实测）
- `py -3 scripts/check_positional_field_order_freeze.py` 与 `--selftest` 均 EXIT=0（selftest 27 条断言全绿；真实 0.70s）
- 真树反事实：撤 `popup.rs` 那行 EXEMPT ⇒ `[GAP] NEW` EXIT=1；还原 ⇒ EXIT=0 且 `cmp` 证台账逐字节相同
- `cargo test -p nexus-contracts --test l0_exception_registry` = **5 passed / 0 failed**（先红后绿：加测时精确报出 `behavior_contract.rs:9`，修正文档后转绿）
- `rustfmt --edition 2021 --check` 对本波两个 `.rs` 干净；`cargo check -p session-store` EXIT=0（12.68s）
- G-99 / G-105 / G-107 / G-109 / G-91 复跑 EXIT=0；`run_gate_manifest.py --list` EXIT=0（TOML 可解析）；`check_gates_provenance.py` EXIT=0
- 判据：无功能退化（测试逐位对齐基线）/ 无性能劣化（未触热路径）/ 无新增 unsafe（G-99 + `forbid` 未动）/ 无公共 API 不当暴露（G-107 反向锁）
- **未达成即明说**：`cargo test --workspace`、`clippy --workspace --all-targets`、`audit_gate_parity`（现 EXIT=1，2 条红全属并发批次 `check_deleted_crate_registry.py` 未接 CI）、`check_entry_contract`（现 EXIT=1，唯一 finding 是同一脚本缺崩溃守卫）、合并门终局证据（本分支从未跑过 CI）—— 均**不由本波洗白**，不加 `ci_exempt`、不扩基线。

## 5. 回滚
G-113/114 四件同批 `git revert`（门与脚本同进退，反之留死判据触发 G-103 方向 A）；台账删除即回滚；`l0_exception_registry.rs` 不变量 4 单函数删除即回滚（余 4 测不受影响）；文档行为单行反向编辑。**工作树含 146 项他人在途改动 ⇒ 禁用 `git checkout --` / `git restore` / `git stash` / `git add -A`**，回滚一律用落盘前 `git diff -- <file>` 快照或反向编辑。

## 6. 未做 / 待裁（诚实登记）
1. ~~**D-22 `#[non_exhaustive]` 三档推进**未开工~~ → **档 1 已落地**（G-115/G-116，见决策 3）；档 2 待逐类型开档，候选集已自测为 21 enum / 64 struct（旧记 24/103 作废，且其偏松方向已定位：首轮漏算根 `tests/`）。
2. **S7「161 个低消费者豁免」分桶定性未做**（本轮预算内未执行）。已确立的判据前提：G-91 `build_model` 只 walk `crates/*/src`，对 `crates/*/tests/**` 零可见性 ⇒ "仅测试消费"这一桶 G-91/G-101 两支门都算不出，须另建第三轴；不得复用二者基线。
3. **bench 阈值数值对等无门**：`perf_thresholds.toml:62` 与 `bench_check.yml:265` 各存 `500000`/`500_000`；`check_perf_redlines.py` 的 I1 只比键集、I7 只比 `run_bench` 行集 ⇒ 数值未覆盖。处置：在既有 G-01/G-02 脚本内加 I9，**不新增门**（未实施）。
4. **覆盖率无 L0 专项条目**：`coverage_per_crate_floor.toml` 无 nexus-contracts 行 ⇒ 落 `[defaults] floor=45`；该文件自陈"不得用估算或编造值填表" ⇒ 只能由 CI 侧 llvm-cov 出数后填，本轮不造数。
5. ~~**并发批次 2 条红**（非本波引入、非本波可修）~~ → 业主随后**明确授权代修**，已按最小单价落地（见 §7 追加）。
6. 取数教训（已入本 ADR §2 决策 4 表）：shell 粗算消费者数会因 `grep -A20` 截断假阴；`taplo fmt --check` 不带 `-c .taplo.toml` 会静默回退默认格式化并**对 `tmp/` 下路径报 excluded=1 而"零文件即绿"**——两种假信号都曾被误读为判决。

---

## 7. 同轮追加（2026-10-14 晚，W14）

**(1) 决策 3 的档 1 落地为 G-115/G-116**（四件同批：脚本 / `l0_extensibility_class_freeze.txt` 227 行全 closed / `gate_manifest` / `ci.yml`）。开发期由自测夹具连续现形三处自身缺陷，全部先于落地修掉：① `crate` 根文件的模块键是 `lib` 而非 `m`，而首版断言写成 `... or len(types)==6` ⇒ **键全错仍报绿**（恒真守卫，本仓反复禁止的那一类）；② NEW 分支 `"..." % (a, b), c` 的 `%` 优先级写错 ⇒ 3 占位符只给 2 参数，TypeError 意味着**该门最核心的判据从未执行过**；③ `evaluate()` 与调用方双打印。教训与前两波一致：门的 rc 不编码判定内容时，"跑过且绿"完全可以对应"判据从未跑通"。

**(2) 授权代修的跨批次 2 条红已清零**：`scripts/check_deleted_crate_registry.py` 的 `__main__` 补 `gate_rc.run` 守卫（崩溃→退 2，不再借 Python 默认的退 1 冒充"判过且红"），其 G-111/112 接入 `ci.yml` 的 `Static read-only audit gates (G-25 wiring)` 步骤（按字母序插在 `contract_consumers_parity` 与 `doc_pointer_integrity` 之间）。复跑：`audit_gate_parity` **rc=0**（"no parity gap"）、`check_entry_contract` **rc=0**（VERDICT GREEN）。归属如实登记：这两处改动由 W14 会话代该批次完成，非其原班产出。

**(3) 上一提交的两处"未执行项"已被事后证据补上**：磁盘放宽到 6.1 GB 后实测 `cargo check --workspace --all-targets` **rc=0（3m39s，磁盘净减 0.2 GB）** ⇒ `67f7749` + `ff54bd5` 两提交所构成的树在**全部 39 crate 的 src+tests+benches 面**上可编译，不再只是逐 crate 结论。合并门终局证据仍缺（本分支从未触发 `ci.yml`）。

**(4) 我自己写坏过一次 `gate_manifest.toml` 并被门当场抓住**：note 是 TOML 基本字符串，我在其中使用 ASCII 双引号（`"干净"` 等 5 处）提前闭合字符串 ⇒ `run_gate_manifest.py --list`、`check_gates_provenance`、`check_entry_contract` **同时退 2**。这不是"门太脆"，而是三份门共享同一份解析器、语法一坏集体失明的结构性事实——也再次说明登记类改动必须**改完立刻复跑被它影响的门**，而不是改完就宣称。已全量改用「」。
