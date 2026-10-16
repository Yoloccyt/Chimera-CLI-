# ADR-220: L0 治理面三连——机制字段化（G-119 改造）、零消费者派生台账（G-123/G-124）、SparseMask 入站面（第六轮）

> **Status**: ✅ Accepted (2026-10-16)
> **决策日期**: 2026-10-16
> **轮次**: L0 专项第六轮（前五轮见 ADR-212 / ADR-213 / ADR-214 / ADR-215 / ADR-216 / ADR-217 / ADR-218 / ADR-219）
> **关联 ADR**: ADR-033 追加节 2026-10-09（「受控」二字无机器判据的自陈）/ ADR-212 决策 1、3（L0 单 crate 边界与 G-99）/ ADR-213 D-7 与 R4（`select_top_k` 归属、pubfn 棘轮）/ ADR-217 D-36（dev-only bench）/ ADR-218 D-39~D-45（逻辑量台账门本体，本 ADR 更正其两处归因）/ ADR-004 与 ADR-184（MessagePack 命名 map 约定）/ ADR-199 7 Q2（G-91 的 ≥2 消费者规则本体）
> **权威源**: 代码实测——`scripts/check_contract_consumers.py` 自报读数、`scripts/check_l0_logic_ledger.py` 派生结果、`crates/nexus-contracts/src/sparse/masks.rs` 类型与手写 impl、`cargo test -p nexus-contracts` 实跑
> **编号合法性**: `ls docs/architecture/` 主编号现取至 **ADR-219**；`grep -rn "ADR-220|ADR-221" docs/ agents.md` 在本文落盘前 **0 命中**（无预留注记、无并发占用）⇒ 220 空闲可用

---

## 1. 背景：三个缺口的共同形状是「门在，但判据打在错误的抽象层」

| 缺口 | 实测证据（2026-10-16，本文落盘前现取） | 为何既有门接不住 |
|---|---|---|
| **F-11** 台账散文可说谎 | `scripts/l0_logic_ledger.txt` 三个 `MOD` 行的 `basis` 均断言「其逻辑形如 match→变体决策表，被 LOOKUP 一票否决抑制」；实测 `memory/archive_monotonicity::assert_archive_monotonicity` 与 `memory/experience_card::normalize` 的 `=>` 计数为 **0**、体长 **7** 行 | G-119 校验 `len(basis) >= 24`，**从不校验内容**。`basis` 是人写的自由文本，门对它没有任何判据 ⇒ 一句假话只要够长就是绿的 |
| **F-12** 手写 `Deserialize` 无入站面 | `grep -rn SparseMask crates/nexus-contracts/tests/` = **0**；该类型此前只被同文件 `#[cfg(test)]` 尾内一份 `serde_json` 往返测过 | round-trip 恒等式对「入站非法形状」天然失明：喂进去的每个值都是自己编码的。若手写 impl 退化成 derive，`active_set` 两边都空 ⇒ **恒等式仍成立**，而 `is_active` 已恒 false |
| **F-13** 豁免台账覆盖全集 | G-91 自报 `defs: 236 (baseline: 161; gaps<2: 161)`，即 `gaps<2` **恰等于** `baseline`；其中生产消费者为 **0** 的有 **75** 个 | G-91 的 `judge` 是 `stale = baseline - gap_set`：删除/改名有牙，但**新增**一个零消费者公共类型只要顺手补一行就永久静默。规则想防的形态恰好是它判不了的形态 |

三者同源：**用「长度/存在/计数」代理了「内容」**。本 ADR 的三条决策都是把代理信号换成与被代理对象的直接对拍。

---

## 2. 决策 1：G-123/G-124 —— 零消费者 exact-key-set **派生**台账

**裁决**：新增 `scripts/check_contract_zero_consumers.py`，其判据文件 `scripts/contract_zero_consumer.txt` 由 `--write` **机器派生**（禁手编），键集必须逐字等于实况的零消费者集合。

```
# CLAIM defs=236 zero=75 one=86 ge2=75 gap=161 baseline=161 rows=75 sum=OK
NAME | consumers=0 | declared_in=<module>
```

- 三向判红：`NEW`（实况零消费者而无行）/ `STALE`（有行而该类型已不再零消费者）/ `CLAIM`（头注与实况不符）。
- 跨门哨：`zero - baseline` 非空 ⇒ `[UNREGISTERED]` 红（两个门对同一条豁免台账必须一致）。
- 结构不变量：`zero + one + ge2 == defs`（`sum=OK` 由代码算，不手写）。

**复用纪律（防第二真值源）**：脚本 `import check_contract_consumers as cc`，消费者数取 `cc.consumer_crates`、gap 集取 `cc.compute_gaps`、定义枚举取 `cc.DEF_RE` + `cc.read_production_text`。两条硬锁：

1. `declared_in` 需要「名字在哪个文件」，而 `cc.build_model()` 返回的 `defs` 是**集合、丢位置** ⇒ 本门必须第二遍扫盘。第二遍只用导入的 `cc.DEF_RE` 与 `cc.read_production_text`，并强制 `set(第二遍) == model["defs"]`，不等 ⇒ **退 2**。否则这一列就是无人对拍的影子事实。
2. **方言锁** `zero ∪ one == cc.compute_gaps(model)`，不等 ⇒ 退 2 `[DIALECT]`。该守卫在真实树上永不触发，故 `--selftest` 用**内存替换 `cc.compute_gaps` 后重新 scan**（try/finally 还原）证明它会红——只 patch 不重扫是无效测试，因为 `facts["gaps"]` 在 scan 期内已算完。

**被否决的反方案**

| 反方案 | 否决理由（实测/结构） |
|---|---|
| 标量只减棘轮 `zero_max=75` | 改名让一个名字出去、一个进来，**净额不变 ⇒ 恒绿**，而台账内容已完全换掉。G-115（可扩展性分级）与 G-119（逻辑量台账）都是「从计数迁到 exact-set 之后才有牙」的同型先例 |
| 新入 `zero` 者须带 ≥24 字符理由 | 今天 75 行在 G-91 基线里**全是裸名** ⇒ 该规则首跑即红，把「立门」与「清 75 行存量」混成一次改动，事后无法归因。改为打印 `[REGISTER-DEBT] bare=NN` **advisory**（不进 rc、不新增门号），与 ADR-218 记录的「不能判红 ≠ 只能静默」同法 |
| 给 `rows` 设下限 | `rows` 是**债**的度量：正确接线会把它推向 0。设下限等于让门禁的绿依赖缺陷继续存在——本仓记录过的极端病理形态（「门禁的绿由待清理的缺陷撑着」）。故本门地板只守**扫描面**（`DEFS_FLOOR=50`），且 `--selftest` 用 `run_real(crates_root=5-def 夹具)` 证明它**接进了生产路径**（G-121 自曝过「声明了却没接进 judge 的地板等于没有地板」） |

**首轮不红的核算**：`zero + one = 75 + 86 = 161 = baseline` ⇒ 派生即自洽，无需任何豁免动作。
**四件同批**：脚本 + 派生台账 + `gate_manifest.toml` G-123/G-124 两条 + `ci.yml` 一步（`spec_ref` 指向本 ADR 决策 1，故 ADR 同批）。拆笔即非法——G-103 方向 B 抓「带 `--selftest` 却未登记」，`audit_gate_parity.py` 抓「`expect=0` 却未接 CI」。

---

## 3. 决策 2：`mech` / `near` / `supp` 机器字段 + `basis` 散文禁令（G-119 内改造，不新增门号）

**裁决**：机制归因从散文搬进三个机器字段，由 `mech_of()` 单一派生（`required_keys` / `judge` / `--write` 共用同一份），值域 `MECH_VOCAB = DETECTED | LOOKUP-SUPPRESSED | BELOW-DETECTOR | NO-BODY | TABLE`。

```
mech_of(kind, hits, mod_fns):
    TABLE 行            -> TABLE
    hits 非空           -> DETECTED
    模块内有被否决者     -> LOOKUP-SUPPRESSED, supp=个数
    模块无生产体        -> NO-BODY
    其余                -> BELOW-DETECTOR, near=<全模块离阈值最近者>  # 形如 level:D5.bl=8/9
```

两道牙：① `judge` 把 `mech/near/supp` 并入既有 `detectors/bl/br/ar` 的逐字 diff ⇒ 手写错值即 `[MECH]` 红；② `BASIS_MECH_RX`（`trips no detector | no detector | decision[ -]?table | match[ -]to[ -]variant | lookup | veto | suppress | threshold | algorithm`，大小写不敏感）命中 `basis` ⇒ **退 2**。

**为何取 rc 2 而非 rc 1**：命中时读者无法判断是散文还是字段描述这棵树——那不是「判出违规」，是「判不了」，与本仓 0/1/2 退码方言一致（不可判定 ≠ 通过 ≠ 红）。

**迁移零人工**：`--write` 派生填三字段，并按 `;` 切 `basis` 子句、丢弃命中禁令的子句；若剩余不足 24 字符则**拒绝改动**而打印 `[BASIS-KEEP] <key>` 点名人工——否则「把 basis 缩短到违规」会被静默完成。本轮实跑 `stripped=4`、需人工 `0`。

**本轮实测出的纪律**（写下来是因为它不是常识）：`seams/density::from_rho` 的原 basis 把**引用**（`ADR-218 D-1:`）和**叙述**（`not a lookup table`）写在同一子句里，禁令把两者一起吃掉。⇒ **basis 应一句引用、一句事实分写**；混写时自动迁移会削掉 provenance。

**被否决的反方案**

| 反方案 | 否决理由 |
|---|---|
| 只把那三句错词改对 | 改完仍然没有判据能保证下一次不再写错——症状消失、病因留下。本门的价值是「让这类漂移没有生存空间」，与 G-115 的 `# CLAIM` 改造同型 |
| 把「零逻辑」三字在全库禁用 | 盘上命中分两形：自我主张形（`//! - **纯类型 + 零逻辑**`）与引用叙述形（`capability_token.rs` 自陈「本模块**不是**零逻辑」）。后者是对该词的**谈论**，判红即假红；该区分已由 ADR-218 的不变量 4（`registered_exception_never_claims_zero_logic`）落码，本决策不重复其射程 |
| 让 `mech` 由 `basis` 文本解析而得 | 假派生——用被检对象的散文当被检事实的来源。故 `--selftest` 必含一条：夹具写「basis 声称 LOOKUP 而实况是 BELOW-DETECTOR」必须 `[MECH]` 红 |

**回滚耦合（实测）**：新脚本对旧台账判 **rc 2**（缺 `mech` 键 ⇒ 越词表），旧脚本对新台账判 **rc 2**（`mech` 是未知键 ⇒ malformed）。两侧都不存在「半个批次仍绿」的中间态 ⇒ 脚本与台账必须同 commit，退也同退。

---

## 4. 决策 3：D8「决策表检测器」**否决**登记

曾考虑再加一个检测器 D8，把被 LOOKUP 否决抑制的决策表显式化。两个候选公式都被实测否证：

- **公式 A**：`ar >= N 且 (arith + cmp) >= 1` —— 对目标模块**抓 0 个**（那些表的臂全是常量返回，算术与比较为 0），是一条死键。
- **公式 B**：「match 臂内含表达式即可疑」—— 必然误伤现存合法形态：`seams/recall_quota.rs:110` 的 `Some(Self::K5)`、`memory_pyramid.rs` 的 `from`、`mca/omni_message.rs:240`、`spec/harness_spec.rs:395` 的 `write!` 臂、`seams/capability_token.rs::short_name` 等；`seams/` 与 `mca/` 下满足 `ar>=2 且 is_lookup` 的生产函数 **>=12** 个。收益上限也只是把 3 行盲区收口到 1 行。

**否决结论**：立 D8 的假阳性成本高于其覆盖增益。**重议触发条件**（不满足则本否决持续有效）：`memory/archive_monotonicity` 或 `memory/experience_card` 任一模块出现体长 ≥ 12 的函数——届时 D1 已能命中，D8 的必要性须重新量一次而不是沿用本 ADR。

---

## 5. 决策 4：`SparseMask` 的 wire 面分工与「现状钉」

**前提更正**：本 ADR 计划稿曾假设 `active_set` 私有、集成测试只能间接断言。实测 `crates/nexus-contracts/src/sparse/masks.rs:63` 该行是 **`pub active_set: HashSet<T>`**。前提作废，而且作废得有用：正因为它是 pub，才能在**不改一行生产码**的前提下手工造出「derive 退化形状」（vec 完好、集合为空），从而证明新写的集合断言与 `PartialEq` 断言都会红。测试 `set_consistency_checks_would_fire_on_the_derive_shape` 就是这条牙口反事实。

**两文件分工**（先例 = `tests/wire_format_compat.rs` 专判位置式 vs 命名式）：

- `tests/wire_roundtrip.rs` 增 `SparseMask<ToolId>` 与 `OmniSparseMasks` 五维嵌套：判**恒等**，复用现成 `assert_roundtrip`（同时给 JSON ≤4 ulp 容差与 MessagePack 位精确）。策略用 `prop_oneof!` 组合四支，第四支是关键——`empty`/`full`/`select_top_k` 造不出「ratio 与 ids 长度不成比例」的合法线上态。
- 新 `tests/wire_sparse_mask_inbound.rs`：判**入站非法形状**，喂的是别人写的字节。

**被否决的反方案**：给 L0 类型加 `Arbitrary` impl。那等于为测试改公共面；且 `full()` 不校验 `sparsity_ratio`，一个基于它的 `Arbitrary` 会成为「合法的错态生成器」，把本应显式讨论的值域问题藏进随机数里。

**钉成「现状」而非「不变量」的三条**（每条都在注释里写明：改了实现就该来这里签字）：

| 事实 | 断言 |
|---|---|
| `active_count()` 取 `active_ids.len()`，而 `is_active` 走 HashSet | 重复入站 id：`active_count()==3` 而 `active_set.len()==2` |
| 手写 impl 不校验值域 | `sparsity_ratio` 取 `1.5 / -0.5 / 100.0` 今天**读得进来**且原样透传 |
| serde derive 同时接受 map 与 seq | 位置式 array **可读**（本类型 2 字段、无可选）——这不是新主张，是对 `tests/wire_format_compat.rs` 全 crate 实测结论的**复证**，也正是当年 F1 双读 shim 被取消的依据 |

---

## 6. 决策 5：陈旧散文的更正纪律——标注过期，不改写历史

本轮实测推翻的四处散文全部更正，但方式不同：

- `scripts/gate_manifest.toml` 的 **G-119 note**：删去「三个已登记模块的逻辑形如 match→变体决策表，被 LOOKUP 否决抑制」这一错误归因；删去其内联首跑数字块（`derived_fns=52 / registered=17 / REGISTRY-GAP=11` + 11 个名字，现实况为 `51 / 28 / 0`，**全部过期**）改为自报指针；补 `mech/near/supp` 与散文禁令。
- **G-120 note**：补新增断言轴，仍按自报口径不抄数（承 ADR-218 D-44）。
- `docs/architecture/ADR-218-*.md:25` 与 `:194`：**追加订正节**并指向本 ADR，不覆写原文。理由：`C ⊉ R` 这一结论（键集取 `C ∪ T ∪ R` 的依据）**至今成立且不变**，被推翻的只是三个模块的机制归因；把原文改掉会抹掉「当初为什么这样设计」的可追溯性，而那份设计取舍至今仍是正确的。
- `docs/reports/L0-round4-controlled-logic-gate_2026-10-16.md:214`：同上（该文件按 `.gitignore` 属本地单一事实源，不入 git）。

---

## 7. 复测台账（每条主张的取证方式）

| # | 主张 | 取证 | 结果 |
|---|---|---|---|
| 1 | G-91 今日零约束力 | `py -3 scripts/check_contract_consumers.py` 自报行 | `defs: 236 (baseline: 161; gaps<2: 161)` ⇒ `gaps<2 == baseline` 成立 |
| 2 | 三行里两行归因是假的 | `mech_of` 实算 + 逐函数 `=>` 计数 | `BELOW-DETECTOR=2 / LOOKUP-SUPPRESSED=1`；`near=level:D5.bl=8/9`、`near=default_token_estimate:D5.bl=8/9` |
| 3 | 新门首跑即绿且自洽 | `--write` 后 `py -3 scripts/check_contract_zero_consumers.py` | rc 0，`sum=OK`，`zero+one=161=baseline` |
| 4 | `--write` 派生幂等 | 连写两次比 md5 | 同一哈希 |
| 5 | 新门有牙（真树） | 对实况台账做 6 类单点破坏 | 删行→`[NEW]` rc1；重复→rc2；改名→`[NEW]+[STALE]` rc1（**正是标量棘轮抓不到的那类**）；错 `declared_in`→`[LOC]` rc1；过期 CLAIM→rc1；删头注→rc2 |
| 6 | 方言锁不是死守卫 | 内存替换 `cc.compute_gaps` 后重扫 | `[DIALECT]` rc2；还原后同一台账 rc0 |
| 7 | 地板接进了生产路径 | `run_real(crates_root=5-def 夹具)` | rc2 且消息含 `below floor`，且发生在任何读写之前 |
| 8 | `SparseMask` 集合断言不是恒真 | 手工把 `active_set` 清空造 derive 形状 | 断言面区分得开（`PartialEq` 亦判不等） |
| 9 | 无公共 API 位移 | `check_l0_pubfn_ratchet.py` / `check_l0_root_surface.py` | `254/254`、`53/53` 逐值不变 |
| 10 | 无功能/性能退化 | `cargo test -p nexus-contracts`；生产码零改动 | **939 passed / 0 failed**（基线 926 + 13）；clippy `--all-targets -D warnings` 干净 |

---

## 8. 影响面与兼容性

- **生产码零改动**（`crates/**/src` 未动一行）；改动面 = `scripts/`（2 改 2 增）+ `crates/nexus-contracts/tests/`（1 改 1 增）+ 本 ADR + `.github/workflows/ci.yml` 一步。
- 无事件契约变更 ⇒ 不动 `NexusEvent`/`doc_count_freeze.txt`/mpsc 双清单。
- 无依赖变更 ⇒ 不动 `Cargo.toml`/`Cargo.lock`（新门 stdlib-only，与 ADR-219 的钉定解释器无 site-packages 约束一致）。
- 兼容性：**非破坏性**。`contract_zero_consumer.txt` 是新文件；G-123 的 `expect=0` 首跑即绿。
- 既有红登记（**非本批引入，本批不动**）：G-16 `audit_spec_coverage.py` 在 HEAD 实测 rc=1 而 `expect=0`，内容为「仓外 spec 仍指 `crates/nexus-contracts/src/util.rs`」（后继 `src/vector_math/util.rs`）。该条已由 **ADR-212:108** 裁决为「spec 在仓外，不宜在仓内代改」，且 `.github/workflows/` 全文不调度该脚本 ⇒ 不阻塞 CI。本 ADR 登记它的唯一目的：终验全门快照里这条红不得被误读成第六轮造成。

---

## 9. 回滚

- **批 B（G-123/G-124 + 派生台账 + ci.yml + 本 ADR）不可拆退**：退脚本而留台账 ⇒ `expect=0` 的门 cmd 不存在；退本 ADR 而留 manifest ⇒ `spec_ref` 悬空；退台账而留脚本 ⇒ 缺文件退 2。整组逆序退。
- **批 A（脚本 + 台账）同 commit、同退**（理由见 §3 末的回滚耦合实测）。
- **批 C（两份测试）与其余无耦合**，可独立退。
- **决策 5 的散文更正**（G-119/G-120 note、ADR-218 订正节）是纯文本、不改判定，可独立退。
- 每笔回滚前先 `git apply -R --check` 预检；冲突则整组逆序退，**不得**用 `git checkout --` 覆盖未提交内容。

---

## 10. 遗留与待裁决

| 编号 | 事项 | 状态 |
|---|---|---|
| T-2 | `cargo machete` rc=1：6 条 `chimera-cli` `optional = true` 边全库 `.rs` 零引用，且无 ignore 表（`ci.yml` 里承诺有一张的注释是过期主张），溯源 `38c72e6` | **待用户在「删边 / 挂 `dep:` feature / 加 `[package.metadata.cargo-machete] ignored`」三者中裁决**（非 L0，本批不动） |
| T-3 | `l0_exception_registry.rs` 的 `SUBSTANTIVE_LOGIC_MODULES` 与 `ADR_033_EXCEPTION_MODULES` 是手抄对称（第二副本） | 待裁决 |
| T-4 | 把 `# CLAIM` 头注推广到其余冻结台账（`l0_pubfn_freeze` / `bench_inventory_freeze` / `crate_reachability_freeze`） | 待排期 |
| 维度 8 | 51 个派生逻辑函数中仅 **3** 个有 bench；热路径 `sensitive::scan`（21 站点 / 14 文件 / **11 crate**，每写入路径）与 `cosine_similarity_slices`（经 `nexus-core/src/clv.rs:161` 转发，6 crate 生产点）无 bench；`canonical_merkle_input`（bl=82）生产 **0** 调用（12 处全在 `#[cfg(test)]` 之后） | 已授权真跑 bench，但**明确划出本轮**（避免与 G-123 接线混批） |
| 本 ADR 射程外 | 决策 3 的 D8 否决若因 `archive_monotonicity` / `experience_card` 出现长函数而失效 | 触发条件见 §4 |
