# ADR-213: L0 残余债务收口 —— 合并门可移植性、advisory 门升格、第五类杂质「环境隐输入」、公共面双棘轮与 wire 编码统一序列

> **Status**: ✅ Accepted (2026-10-13)
> **决策日期**: 2026-10-13
> **关联 ADR**: ADR-033（L0 建立 + 2026-10-09 追加节纯度自陈）/ ADR-049 决策 1（0 新 crate）/ ADR-170（Ω₁₀/Ω₁₁）/ ADR-173（事件契约单表）/ ADR-187（Check A 退役）/ ADR-192/193/195（减法与 GPU 否决）/ ADR-197 A6·D5（advisory 账本「暂不登记」）/ ADR-198 §3.4（门升格需 spec_ref + 委员会）/ **ADR-212（本 ADR 全盘继承其决策 1–5，仅补其 §7.5 未裁项）**
> **关联交付**: `docs/reports/L0-contracts-residual-debt_2026-10-13.md`（事实基线 / 冲突台账 / 十维残余审计 / F0 附表）
> **权威源**: 代码实测（`crates/nexus-contracts/`、`crates/*/src`、`scripts/`、`.github/workflows/`）；两份设计文档仅作意图参照

---

## 1. 背景

ADR-212 已把 L0 的**结构性**问题裁决完毕（单 crate、不拆分、纯度机器门 G-99、W3 子模块化）。本 ADR 只处理其 §7.5 未裁项与 W0–W6 遗留的五类**残余**，其中两类是新发现的、ADR-212 自评「全部本地门禁绿」时不可见的缺陷：

- **合并门不可跑**：`ci.yml` 触发面实测为 `pull_request:[main]` / `push:[main]` / `workflow_dispatch`（`:14-19`），四个 job 全 `runs-on: ubuntu-latest`（`:35,522,591,653`），全文件唯一 `continue-on-error` 是 job 级 `benchmark`（`:655`）。在途分支既非 main 也无 PR ⇒ ci.yml 从未为本批改动跑过。`tests/dependency_rules_inner_ring_test.rs` 四处 `Command::new("powershell")` + `.current_dir("d:\\Chimera CLI")`、`tests/composition_root_omega_learner_test.rs:105` 一处同型调用，在 Linux runner 上 spawn 失败即 `.expect` panic，而 e2e job 的 `cargo nextest run -p chimera-e2e-tests -E 'not binary(/stress/)'`（`:566`）不含排除该 target 的表达式。
- **advisory 门从不执行**：`check_dead_contract_apis.py` / `check_gates_provenance.py` 均自带双向 `--selftest`、退出码编码判定、`gate_rc` 崩溃退 2 且现跑 EXIT=0，却因 ADR-197 D5「暂不登记」被 `gate_provenance_exemptions.txt:24-25` 明文豁免；后者方向 B 专抓「有 --selftest 未登记清单」，即执行「升格须同批删豁免行」这条纪律的门自身不在清单内 ⇒ 忘删无人可抓。
- **纯度面缺第五类**：G-99 的禁用符号表 11 条（`check_l0_purity.py:60-72`）不含环境变量读取；L0 生产面实有 2 处（`src/testutil/test_scale.rs:52` 读 `CHIMERA_TEST_TIMEOUT_SCALE`、`src/vector_math/util.rs:478` 的 `perf_scale()` 读 `CHIMERA_PERF_SCALE`）。ADR-212 决策 3 显式豁免了时间却从未讨论环境。
- **公共函数面零治理**：G-91 的 `DEF_RE`（`check_contract_consumers.py:67`）只枚举 `pub struct|enum|trait|type` ⇒ L0 的 `pub fn`（实测 255 个生产站点）不在任何门统计内；W3 保留的 53 条根 re-export（双公开路径）同样无「不得增殖」约束。
- **编码口径混用**：`crates/*/src` 内 MessagePack 站点实测 54 处（生产 24 / 测试尾 30），其中**生产 positional 编码器 9 处 / 4 crate**（L9 quest-engine 5、L3 cmt-tiering 2、L10 mca-gateway 1、L10 chimera-tui 1）与**生产 named 编码器 5 处**（L1 event-bus 3、L3 session-store 2）并存；持久化 envelope（`Checkpoint`，`src/core_domain/checkpoint.rs:36`）无 `schema_version` 字段。

## 2. 决策

### 决策 R1：把两支 advisory 检查器升格为阻塞门（G-101~G-104）——本 ADR 即 ADR-197 D5 / ADR-198 §3.4 要求的「委员会裁决」记录
- 登记 `G-101/G-102 = check_dead_contract_apis.py`（真实 / `--selftest`）、`G-103/G-104 = check_gates_provenance.py`（真实 / `--selftest`），`expect=0 / kind=light / test_kind=cross_crate`；**同批删除** `gate_provenance_exemptions.txt` 对应两行（该文件 `:17-19` 自订纪律）。
- G-85 号已被 `check_ps1_bom_gate.py` 占用（exemption 文本自陈「故本脚本尚无候选号」），本 ADR 直接分配空段 **G-101+**（实测清单最大号为 G-100）。`check_gates_provenance.py` 的候选号 G-86 同理作废、改取 G-103/G-104。
- **轴分工写进 note，禁止互抄基线**：G-91 = L0 pub 类型 × 消费 crate 数（≥2）；G-101 = 手写账本里 L1/L3/L6/L9 跨层契约成员 × 生产调用点（≥1）；G-105 = L0 函数面新增即需登记。三者分母、单位、基线文件互不相交（先例：`event-bus/src/registry.rs:29-35` 双清单刻意不同源，防互锁退化为自比较）。
- **诚实边界**：G-101 只在「已登记成员失守」方向有牙，不声称穷举全部契约成员（其 pattern 覆盖度是 batch W1 的开放事项，已在 note 登记）。

### 决策 R2：新增第五类杂质「环境隐输入（AMBIENT-INPUT）」，走门内精确豁免，不移出 L0
- **定义**：返回值不由入参而由进程环境决定的读取（`std::env::var/vars/var_os`）及其写入（`set_var/remove_var/set_current_dir`）。与 IO 类的区别是**无可见副作用**——正因如此容易被漏，且**设了变量的 CI 可以在零代码 diff 下改变函数语义**。
- G-99 追加 `env-read` / `env-write` 两条规则；豁免按 **(文件 × 类别)** 精确粒度登记于脚本内 `FILE_EXEMPT`，三条件缺一即红：① 在该文件白名单内 ② 类别属闭集 `IMPURITY_CATEGORIES`（未知类别退 2）③ 理由长度 ≥ `MIN_EXEMPT_REASON_LEN`（24）；**幽灵豁免判红**（令牌已从生产面消失 ⇒ STALE ⇒ 必须同批删行）。纪律抄 G-92 与 `check_write_sensitive_instrumentation.py::EXEMPT_CATEGORIES`，非自造机制。
- **时间维持豁免**：`Utc::now`（L0 现 7 处）由 ADR-212 决策 3 已裁可接受，本 ADR 正式记为「已裁」而非「未讨论」。
- **反方案 β（把 `test_scale` / `perf_scale` 移出 L0 生产公开 API）——否决**。实测消费面是**测试面而非生产 crate**：`perf_scale{,_ms}` 被 9 个文件消费（`gea-activator/tests/csa.rs:53`、`hcw-window/tests/` 六份、`kvbsr-router/tests/scale.rs:118,172`、`mtpe-executor/tests/speedup.rs:65,103,146`、`repo-wiki/tests/hnsw_p95_test.rs:63`），`scaled_timeout!` 覆盖 8 个 crate 的 test 面；而 CI 注入口（`ci.yml:499,563,578`、`slo.yml:73,169`、`stress.yml:44`）与函数物理位置无关 ⇒ 搬迁不降低任何风险、只造 churn。更硬的一条：`tests/l0_exception_registry.rs:63-72` 不变量① 硬要求 lib.rs 含 `::test_scale;`，撤根 re-export 会同时打红既有锁。
- **反方案 γ（feature 门控）——否决**：edition 2021 下 workspace feature unification 使其在整图构建恒 on，「生产不得依赖」仍无机器保证，却要改 9 份 Cargo.toml。
- **计划偏差登记**：原计划用源码内联标记词 `L0-IMPURITY-EXEMPT: ambient-input`。实做改为仓内既有方言（结构化台账 + 类别闭集 + 理由下限 + 幽灵判红）。理由有二：①「豁免台账」是本仓既有约定（`gate_provenance_exemptions.txt` / `*_baseline.txt` / `*_freeze.txt`），自造新词违反「治理标记须先 grep 既有用词」纪律；② 内联标记会把门的书记账污点写进 L0 源码，且文件级标记无法表达「类别」维度。
- **同轮发现并修复的 G-99 自身缺陷（重要）**：为剔除 `#[cfg(test)]` 尾，本 ADR 起初直接移植 G-91 的 test-tail 规则，但那条规则**只定起点、不定终点**（`in_test_tail` 置真后不复位）。在 `vector_math/util.rs` 里首个 `#[cfg(test)] mod` 位于 `:93`，而生产 `perf_scale()` 在 `:470` ⇒ 尾会吞掉其后整份文件，使纯度门对该文件**实质缴械**。已改为**按花括号深度闭合**（`test_tail_mask`），并加两条夹具（尾后生产项仍须被扫；嵌套块内同理）。起点判据仍与 G-91/G-101 逐字同形，终点判据是本门特需、差异已写入源码注释。
- 附带纠正：本 ADR 原拟把 `test_scale.rs:19` 的 doctest 由 `rust,no_run` 改 `ignore`——**作废**，`no_run` 会编译（类型检查）而 `ignore` 不编译，降级一个有效检查无收益。

### 决策 R3：退役 `tests/dependency_rules_inner_ring_test.rs` 及其 `[[test]]` 注册
- 逐用例判据后继实测：TC-1 断言「`[GAP-A]` 不出现」是**恒真守卫**（Check A 已随 ADR-187 从 `.sh:249-257` / `.ps1:221-230` 双源删除），其合法后继已存在于 `check_dependency_rules.sh:383-385` 的 selftest；TC-2/TC-5 的字面串在 `.sh` 侧逐字同形（`:424`、`:360`）⇒ 该 target 只是把 CI 已跑的门输出复制进测试，且跑的是 CI 从不调用的 `.ps1` 孪生；TC-3 在 nextest 进程内二次全量编译 39 crate；文件头 `:2` 仍写 `ADR-XXX` 占位符（`adr_index_sync.py:41-42` 本就把幽灵号判红）。
- `composition_root_omega_learner_test.rs` **不整体退役**：仅给 `:105` 的 powershell 分支加 `#[cfg(windows)]`（先例 `tests/security/owasp_top10.rs:673`）；`Command` 仍被 TC-4/TC-6 使用，故 Unix 侧不会因 unused import 触发 `clippy -D warnings`。
- **反方案 a（改写为纯 Rust `cargo metadata` 层图断言）——否决**：会在 L0 之外再造第 5 份手写「crate→层」表，而现 4 源已由 G-45/G-46 锁死，正是该门要治理的漂移病灶。**反方案 b（改调 `.sh` + 平台护栏）——部分否决**：`.sh` 本机实跑 >120s，在测试二进制内重跑等于翻倍门时延；仅保留 Windows 侧残值。
- 孪生检查面不等（`.sh` C2 多一层「layered_crates 条目查不到层」循环，`.ps1` 仅比计数）：**不改孪生**（改单侧必触 G-45/46 与 `gate_predicate_baseline.txt` 只减棘轮），登记为已知方向性差异（账本已有同型条目 `P2|GAP-C emit sites .sh=4 .ps1=3`）；仅订正 `gate_predicate_baseline.txt` 头部与实况相反的「保持 P3|Check A 红」表述。
- 本决策使 `ci.yml:118` 的注释（以该 target 为 `.ps1` 存在理由）失效 ⇒ 随附补丁一并订正，判据本身不变。

### 决策 R4：新建 L0 `pub fn` 面只减棘轮（G-105/G-106）
- `scripts/check_l0_pubfn_ratchet.py` + `scripts/l0_pubfn_freeze.txt`，键 = `模块路径::函数名`，**多重集**（同模块内两个 `impl` 各定义 `new` 记 2 行；set 去重会让第三个 `new` 逃判，夹具已钉）。
- 判据：NEW 未登记⇒红；STALE（函数已不存在而行未删）⇒红；行畸形或键缺 `::`⇒退 2；抽取数 < `DEFS_FLOOR(100)`⇒退 2（解析致盲会报得与「干净」一模一样）。
- 生产面口径与 G-99 **共享**同一词法器与尾遮蔽（import 复用而非手抄）。首跑=全量登记态：**live=255 / listed=255**；交叉对照：行级 grep 256（1 处在 cfg(test) 尾内）、去重键 217。真树反事实：注入 `pub fn w7_planted_probe_marker` ⇒ 报 NEW、rc=1；还原 ⇒ rc=0。

### 决策 R5：冻结 L0 根 re-export 面（G-107/G-108），W3 物理搬移维持
- `scripts/check_l0_root_surface.py` + `scripts/l0_root_reexport_freeze.txt`：`lib.rs` 中 `pub use <组>::<叶>;` 声明须与台账逐行相等；**新增⇒红**（新叶子仅可经组路径可达，双路径不再增殖）、**撤除不撤行⇒STALE 红**（撤公开路径是显式评审动作）。值级 `pub use crate::叶::{A,B};`、doc 注释与字符串字面量均不计入（夹具钉住）。首跑：**live=53 / listed=53**（与 ADR-212 决策 5 自陈的 53 叶子同值，两法交叉一致）。真树反事实：追加 `pub use sparse::w7_planted_probe;` ⇒ NEW、rc=1；`cmp` 证还原逐字节相同。
- **W3 物理搬移保留**（业主裁决）：目录即命名空间的可读性收益 > 路径键产物维护面；代价按 ADR-212 §7.4 明示承担。本轮**未**实施「路径锚门」（见 §6 未做项），故该代价目前仍为人工。
- **反方案（一次性撤根 re-export、只留组路径）——否决**：直接断 34 消费 crate 既有路径并打红 `l0_exception_registry.rs` 不变量①，且 W3 已实证路径漂移 blast radius。

### 决策 R6：D-7 消解（零代码裁决）
`masks::select_top_k` 在 L0 外有 83 处引用、跨 L2–L6（csn-substitutor/gea-activator/osa-coordinator/sesa-router），迁 L6 必造 L2→L6 向上边、直撞 Check B 与 G-33。裁定：L0 保留 `select_nth_unstable` 参考实现属 ADR-033「受控纯逻辑」本域，G-20/G-21 冻结的 `xts_top_k{,_by}` 仍是唯一合规范式 ⇒ **D-7 不再待裁**。

### 决策 R7（2026-10-13 改正）：D-6/D-10 wire 序列 —— 初版"顺序不可颠倒"的论证已被本轮实测推翻
- **改正记录（必须先读）**：本 ADR 初版（同日 30 分钟前）写道"给 positional struct 加字段会改变数组长度并平移其后所有位置 ⇒ 旧 blob 读不回且**错误静默**"，并据此规定 F2 必须先于 F3。实测否证了该说法的两个环节。证据 = 永久回归测试 `crates/nexus-contracts/tests/wire_format_compat.rs`（7 例全绿）：

  | 字段变更 | `to_vec_named` | `to_vec`（数组） |
  |---|---|---|
  | 尾部新增字段 + `#[serde(default)]` | Ok，取默认值 | **也 Ok**，serde 以默认值补齐缺失尾元素 |
  | 中间插入字段 | Ok，`b` 仍为原值 | **显式 Err**（`invalid length 2, expected 3 elements`）⇒ 大声失败 |
  | 两个同类型字段**交换声明顺序**（元数不变） | Ok，按 key 取值恒正确 | **Ok 但值互换** ⇒ 唯一会静默腐败的形态 |

  ⇒ **F3 的 `schema_version` 不依赖 F2 先行**（只要以"尾部追加 + default"落地）；F2 的依据被收窄并**加强**为一条：数组形态把字段顺序钉成 wire 契约，同类型换序会**静默改值**且不报错、不让任何测试在解码点失败。
- **F1（读侧双解码垫片）取消**：实测 `rmp_serde::from_slice::<T>` 对两种形态**通吃**，含带 `skip_serializing_if` 的 Option（`Checkpoint.description`）与内嵌枚举 + `Box<[u8]>`（`SegmentMetadata`）两类真实形状 ⇒ 双读垫片会成"没人走的路径"，属过度工程，撤销。F2 写侧切换自身即前后兼容。
- **依据来源校正**：`session-store/src/tree.rs:14` 自陈具名编码为「ADR-004 协议」。现读 `adr_index.md` 的 ADR-004 正文，其决策只有"MessagePack(非 JSON)"与"LHQP 检查点持久化用 MessagePack"，**未规定 map/array 形态** ⇒ 属过度归属，登记 **D-17**；F2 不得引用 ADR-004 作规范依据。
- **跨进程风险已实测解除**：`mcp-mesh` / `chtc-bridge` / `nexus-app-server` 对 `rmp-serde` **零引用**（grep 实测）⇒ MessagePack 只在本地持久化面流转，F2 不是对外协议变更。
- **可行性前提已实测**（撤销初版"本地无编译验证条件"之说）：4.6 GB 余量下 `cargo check -p quest-engine` = 35.26s，`cargo test -p quest-engine` = **383 passed / 0 failed / 1 ignored** 且磁盘不降 ⇒ 初版把阻塞报成了"全有或全无"，实为逐 crate 可验。
- **改正后的序列**：**F0** 精确二分（54 站点／生产 24／测试尾 30；生产 positional 编码器 9 处 4 crate；生产解码 10 处 6 文件）→ **F2** 写侧逐 crate 切 `to_vec_named`（序 L9 quest-engine → L3 cmt-tiering → L10 mca-gateway → L10 chimera-tui；`event-bus`/`session-store` 已是具名）→ **F3** envelope 尾部追加 `schema_version: u16`（`#[serde(default, skip_serializing_if)]`，0 表"前版本"）→ **F4** 禁止生产面新增 `rmp_serde::to_vec(`。
- **`schema_version` 采「共存式」而非「fail-closed」**：两先例为 `faae-router/src/routing_history.rs:125`（v1/v2 共存，`:308-324` 有跨版本测试）与 `mca-gateway/src/spec_loader.rs:47,177`（非 1 即拒）。对持久化 envelope 用 fail-closed 会让所有历史 checkpoint 变砖 ⇒ 取共存。
- **反方案（"F2 无必要，既然双读通吃"）——否决**：双读通吃解决的是**迁移期可读性**，不解决换序静默腐败；F2 的价值在后者加 payload 自述。**反方案（"F3 必须先于 F2"）——否决**：其唯一支柱（数组不可长字段）正是被否证的断言。**应急切片**：若某 crate 的 F2 引发不可控回归，退回"只落 F4 冻结、其余另批"，冻结非破坏。

## 3. 影响

| 维度 | 影响 |
|---|---|
| 代码 | 退役 1 个 test target + `Cargo.toml` 注册行；`composition_root_omega_learner_test.rs` 加 1 处 `#[cfg(windows)]`；`Cargo.toml:670,672` 两处层标注订正；`test_scale.rs` 模块头口径订正（纯文档） |
| 新门 | `gate_manifest.toml` +10 条（G-101~G-110）；新脚本 4 支 + 新台账 3 张；`gate_provenance_exemptions.txt` −2 行；`ci.yml` +3 步骤（G-105/107/109 组与 G-101/103 组） |
| G-99 | 判据 2→3 类（+AMBIENT-INPUT 读写）、生产面口径新增 cfg(test) 尾遮蔽（**深度闭合**）、豁免台账 + 三项守卫 |
| CI | **待授权补丁**：`tmp/ci_yml_w7_gates.patch`（44 行 / 2 hunk，`git apply --check` 通过）。未接线期间 `audit_gate_parity` 判红 8 条（清单 expect=0 而无 workflow 调用）——这是该门的正确行为，不用 `ci_exempt` 洗白 |
| crate 数 | 不变（39）；L0 仍单 crate（ADR-212 决策 1 延续） |
| 消费方 | 零改动（34 生产消费 crate 不受影响；无公共 API 变更） |
| 事件 | `NexusEvent` 变体/severity/mpsc 三锁**零变更**（152 / Critical 20 / mpsc 16） |
| 兼容性 | 已落地部分 100% 向后兼容。R7 改正后：F2 写侧切换**自身前后兼容**（实测 `from_slice` 双形态通吃，无需 F1 垫片）；F3 若以尾部追加 + default 落地亦不破坏旧 blob；真正的风险面收窄为"同类型换序静默改值" |

## 4. 验收判据
- `py -3 scripts/check_l0_purity.py --selftest` 与真实扫描均 EXIT=0（现读 64 文件 / 0 未豁免 / 2 已豁免 AMBIENT-INPUT）
- `py -3 scripts/check_l0_pubfn_ratchet.py --selftest` 与真实均 EXIT=0（live=255=listed）
- `py -3 scripts/check_l0_root_surface.py --selftest` 与真实均 EXIT=0（live=53=listed）
- `py -3 scripts/check_layer_map_parity.py --selftest` 与真实均 EXIT=0（扫面含根 `[dev-dependencies]`，dep rows=73/去重 39）
- `py -3 scripts/check_dead_contract_apis.py{, --selftest}`、`check_gates_provenance.py{, --selftest}` 均 EXIT=0
- `py -3 scripts/check_entry_contract.py` EXIT=0（**G-44 实测曾对本波新脚本判红 5 条 C4**：rc=2 路径打了 `[FAIL]` 标签；已按「不可判定≠判红」改用 `[TOOL-ERROR]` 后转绿）
- `py -3 scripts/run_gate_manifest.py --list` EXIT=0（108 条，TOML 可解析）
- `cargo metadata --format-version 1 --no-deps` EXIT=0 且 `inner_ring` target 消失、`composition_root` 保留（实测 35 个 test target）
- `cargo check -p nexus-contracts` EXIT=0；`cargo test -p nexus-contracts` 0 失败（实测 748 unit + 全部集成 + doctest）
- 判据：无功能退化 / 无性能劣化（本轮未触热路径） / 无新增 unsafe / 无公共 API 不当暴露（G-107 反向锁定增殖面）
- **未达成即明说**：`cargo test --workspace`、`cargo clippy --workspace`、`audit_gate_parity`（待 CI 补丁）、S10 合并门实跑 —— 见交付报告 §4「未执行项」

## 5. 回滚
逐提交 `git revert`：G-101~G-108 与两支新脚本同批（门删了脚本留着无害，反之则有清单死判据）；`gate_provenance_exemptions.txt` 两行随 revert 复原（回到 advisory 态）；G-99 的类别扩展与尾遮蔽在单文件内、revert 即回；R3 恢复文件 + `[[test]]` 注册行；R7 若未来实施则 F2/F3/F4 分层回滚（F2 逐 crate 可单独撤；实测 `from_slice` 双形态通吃 ⇒ 撤后老/新 blob 仍可互读，不需要额外垫片常驻）。ADR 与报告为文档（`.md` 不入库），可独立回退。

## 6. 未做项（诚实登记，非遗漏）
1. **CI 接线**：`tmp/ci_yml_w7_gates.patch` 已就绪未应用（改 `.github/workflows/*` 须业主授权；本仓另有既定纪律：先贴补丁）。
2. **路径锚门**（原计划 G-107/108 位次的 `check_path_anchor_liveness.py`）：未实施。理由是此类自造扫描器若植入不了有效的坏例自检就是假绿，而「扫哪些产物」的抽取面尚未穷举清楚（`docs/reports/**`、派生视图、ADR 证据路径、gate note 四类口径不同）。宁可留人工，不留假门。
3. **F2–F4 wire 实施（本轮已解除"阻塞"误判）**：4.6 GB 余量实测可跑逐 crate 定向回归（`cargo check -p quest-engine` 35.26s；`cargo test -p quest-engine` 383 passed/0 failed 且磁盘不降）⇒ F1 已取消（实测不需要），F2 从 quest-engine 起逐 crate 推进，F3/F4 续后。初版所记"本地无编译验证条件"是把阻塞报成全有或全无，已改正。
4. **`#[non_exhaustive]` 逐个加**：属源码级破坏（跨 crate 结构体字面量构造与穷尽 `match`），且下游匹配点未测 ⇒ 与 F2/F3 同批处理更省。
5. **26 处「零逻辑」措辞**：本轮只改 `test_scale.rs:5` 一处**错误的 crate 级总括**；其余为模块级自述、与 `ADR_033_EXCEPTION_MODULES` 一致，批量改写属 churn ⇒ 登记 D-14 待裁（是否把「零逻辑」纳入某支文档门的禁用词）。

---

## 7. W8 实施与改正记录（2026-10-14）

本 ADR 生效后立即进入执行波，产出三类结果，全部以实测为凭（详表见 `docs/reports/L0-contracts-residual-debt_2026-10-13.md` §7）：

**(1) 本 ADR 初版的三条技术断言被实测否证，已就地改正**（见 R7 改正段）：
- 「positional 加字段必读不回且静默」→ 尾部追加 + `default` 时数组也可读；中间插入**显式报错**；只有**同类型换序**静默改值。
- 「需要 F1 读侧双解码垫片」→ `rmp_serde::from_slice::<T>` 双形态通吃，**F1 取消**。
- 「本地无编译验证条件」→ 4.6 GB 余量下 `cargo check -p quest-engine` 35.26s、`cargo test -p quest-engine` 383/0/1 且磁盘不降；**该说法是把阻塞报成全有或全无，撤回**。
教训固化：规范条款里的**行为断言**必须在写入 ADR 前用可执行实验验证；本例的验证成本是一个 8 分钟能写完的测试文件，而错误代价是一条被写成"顺序不可颠倒"的铁律。

**(2) 已落地**：D-9（`TitoFallback` + `#[serde(rename)]`，零 wire 变更，748+集成+58 doctest 全绿）；F2 的 quest-engine 4 处（383/0/1、clippy/fmt rc=0；`wal.rs` 处已还原）；G-109/G-110 孤儿源文件门（见 (3)）；两份 `ci.yml` 补丁应用，`audit_gate_parity` 转绿且未使用 `ci_exempt`。

**(3) 新发现 D-18：零警告孤儿源文件**——9 个 `crates/*/src/**/*.rs` 无任何 `mod` 指向（`chimera-tui` 5 / `parliament` 3 / `quest-engine` 1）。cargo 对此**不给任何警告**，故 crate 级可达性棘轮（ADR-160）在文件维度是盲区。三法交叉验证（BFS 解析／裸 grep 声明数=0／`cargo test --lib --list` 行为侧命中=0）后固化为 `scripts/check_src_module_reachability.py` + 9 行只减台账，真树反事实：放一个无 `mod` 指向的文件 ⇒ NEW 红 rc=1；删除 ⇒ rc=0。**该门只判"是否参与编译"，不判"功能是否在别处另有已编译实现"**（逐文件未验），台账属可见债务而非许可。

**(4) 有意暂缓**：F4「禁止新增 positional 写点」门**不建**，直至 D-19（具名化的类型级准入门槛）裁定——实测具名化在 43 B 小记录上是 +272% 字节，先把可能错误的方向锁成铁律是本末倒置。F2 剩余 4 处随 D-19/D-20 决定。

---

## 8. 追加决策 R8（2026-10-14，业主裁定 D-19）：wire 编码口径按**记录生命周期**划界

业主选择「按记录生命周期划界」，故本 ADR §7(4) 所暂缓的 F4 由"一刀切禁止 `to_vec`"改为下述**双类别 + 类别 B 字段序冻结**方案。依据仍是 §7(1) 的实测矩阵：唯一会静默腐败的形态是**同类型字段换序**，而尾部追加 + `#[serde(default)]` 在两种形态下均安全——所以真正需要机器守护的是"顺序"，不是"array 这个形式"。

### 决策 R8.1：类别划分（写入 ADR 即为规范，逐类型可判定）
- **Class A｜长生命周期持久化 envelope**：一次写入、跨版本长期回读、量级可控。判据（满足任一即归 A）：被 `session-store`/`cmt-tiering`/`quest-engine` 作为**恢复或审计真值**落盘；或含嵌套可选字段/枚举演化历史。
  ⇒ **必须具名 map**（`to_vec_named`）。已归 A 并已切换：`quest-engine` 的 Quest 状态 blob、Checkpoint 落盘、长任务图快照、搜索树 `to_bytes`（4 处）。待切换：`cmt-tiering/experience_card_storage.rs` ×2、`mca-gateway/session.rs` ×1、`chimera-tui/popup.rs` ×1（popup 是否为持久化面需逐点确认，若仅进程内中转则不属 A/B 任一，可豁免）。
- **Class B｜高频短命记录**：写完很快被截断/压实，条数大、字节成本敏感（典型：`quest-engine/src/wal.rs` 的 WalEntry——**该文件当前是孤儿，见 D-18**；`event-bus/token_ledger.rs` 的导出条目——实测属 `#[cfg(test)]` 面）。
  ⇒ **允许保持位置式**，但必须同时满足 R8.2 两条，否则按 Class A 处理。
- **类别不是永久身份**：某类型从 B 升为 A（例如 WAL 条目被留作长期审计源）时，须在同一提交内切具名并更新台账。

### 决策 R8.2：类别 B 的两条硬约束（这才是新门 F4′ 的判据）
1. **字段顺序冻结**：每个 Class B 持久化类型在 `scripts/positional_field_order_freeze.txt` 登记 `类型::[字段名有序表]`；源码字段序与台账不符 ⇒ 红（覆盖"新增未登记的位置式持久类型"）。这是把 §7(1) 的静默换序风险直接变成机器判据——比"禁用 array"更对症，且不强迫小记录付 3–4× 字节。
2. **只增尾、不改中、必带默认**：新增字段必须出现在**末尾**且带 `#[serde(default)]`（实测：尾部追加在 array 与 map 两形态下均可回读；中间插入在 array 下会**显式报错**，故同样需要登记变更而非静默发生）。违反 ⇒ 红。

### 决策 R8.3：与既有门的关系（避免重复与互锁）
- F4′（拟号 G-111/G-112，实际取号须现场 `grep 'id = "G-'` 现取，勿抄本文数字）判据是**类型字段序**；G-105 判 `pub fn` 面；G-91 判 pub 类型消费数；`check_dep_edge_freeze` 判装配边。四者分母与基线文件互不相同，**禁止互抄基线**（沿 `registry.rs` 双清单刻意不同源的先例）。
- 本决策**取代** §7(4) 的"F4 暂缓"表述：F4 不再以"禁止 `rmp_serde::to_vec(`"形态落地。

### 反方案与否决理由
- **反方案①「全部具名化 + 禁止 array」**：被实测成本否证——`SegmentMetadata` 类 43 B 小记录膨胀到 160 B（+272%），而 WAL/ledger 是高频写路径；且它并未解决 Class A 之外的任何真实风险，只是把风险面换成账单。
- **反方案②「不建门，靠 code review 盯字段顺序」**：否证——静默换序**不产生编译错、不产生测试失败**（array 形态下值被互换后仍 `Ok`），review 是唯一防线时其失败模式是静默数据腐败，正是本仓一贯拒绝的"假绿"形态。
- **反方案③「用 `#[non_exhaustive]` 代替字段序冻结」**：否证——它防的是**源码级**外部构造/穷尽匹配，对 wire 层换序毫无作用（serde 仍按声明顺序编码），且对 34 个消费 crate 是破坏性变更。

### 实施次序（本决策生效后）
R8.1 类别表落 ADR（本节）→ F4′ 门实现（含"植入一次静默换序必须判红"的反事实夹具）→ Class A 剩余 4 处切具名（逐 crate 定向回归：quest-engine 已证单价可承受）→ `popup.rs` 归类确认 → Class B 台账登记（含 `wal.rs` 的处置，取决于 D-18 定性结果）。

---

## 9. F3 首块落地与前向兼容实测（2026-10-14，收窄 §8 的一处表述）

### 9.1 落地
`Checkpoint`（LHQP 持久化 envelope，D-6 点名对象）新增尾字段
`#[serde(default, skip_serializing_if = "is_schema_version_unset")] pub schema_version: u16`，
`0` = 加字段前的历史形态且 0 时**整字段不写入**；两构造函数默认 0。
辅助谓词以 **`pub(crate)`** 落在 `core_domain/checkpoint.rs`，是本 crate 自建立以来**第一个 `pub(crate)` 项**（此前实测 `pub(crate)` = 0），即 §决策 R2 的可见性约定由此开始被兑现，供其余 envelope 模块共用。

### 9.2 两条实测（证据固化为 `crates/nexus-contracts/tests/wire_format_compat.rs`，该文件现 10 例全绿）
1. **非破坏被证明而非声称**：用"改动前形状的镜像结构体 `CheckpointV0Mirror`"与真类型**同轮**编码比对，`description` 取 Some/None 两种情形下，**array 与 named 字节均逐字相同**，且镜像（旧形态）payload 可被新类型直接回读 ⇒ 加字段对既有存储与既有读端零影响。
   附带取证教训：跨轮比较 `Checkpoint` 字节数会因 `created_at` 的时间戳编码长度漂移（实测 187/115 → 190/118，差 3 B 与本次改动无关），所以**字节等同性必须在同一轮内配对断言**，写死 golden 字节的测试会 flaky。
2. **前向兼容是非对称的**（`forward_compat_needs_named_form_measured_not_assumed`）：给结构新增字段后，旧读端读**具名** payload ⇒ `Ok`（未知 key 被忽略）；读**位置式** payload ⇒ **Err**。

### 9.3 对 §7.1 / §8 表述的收窄（自我更正）
本 ADR §7.1 曾写"F1 取消……F2 写侧切换自身即前后兼容"，§8 亦以"尾部追加在两形态下均安全"立论。实测显示该说法只对**后向兼容**成立：
- 后向（旧 payload → 新代码）：两形态皆可 ⇒ **F1 双读垫片确实不必要**（此结论不变）。
- 前向（新 payload → 旧代码）：**只有具名形态可容忍** ⇒ **Class A 必须具名化不止是"防换序静默腐败"，它是版本化能真正生效的前提**。
⇒ R8.1 的 Class A 判据由"两条理由"支撑（换序免疫 + 前向兼容），而非 §8 初稿所写的单条。F2 的价值因此被**上调**，不是被 §7.1 的措辞贬为"仅防换序"。

### 9.4 验证与剩余
`cargo check -p nexus-contracts`、`cargo check --workspace`（39 crate 生产面，1m08s）、`cargo check --all-targets -p event-bus -p quest-engine -p session-store -p cmt-tiering -p pvl-layer`（含测试面）均 **rc=0**；全仓 `Checkpoint` 无外部结构体字面量构造点（同名歧义已排除：`chimera-cli/src/cli.rs:529` 是 clap 枚举变体、`gsoe-evolution`/`chimera-mas` 的 `Checkpoint` 是各自的同名类型）。
**未做（挂账）**：`SegmentMetadata` / `TokenLedgerEntry` 加同一字段（外部字面量构造均为 0，照 9.1 可直做）；`ExperienceCard` 因 **61 处**外部字面量须先派生 `Default` 再逐点补 `..`，其验证需 `--workspace --all-targets`——当前 D 盘 3.0 GB，不在此批冒险展开。

---

## 10. 追加硬约束 R9（2026-10-14）：位置式持久类型禁止新增 `default + skip` 字段

F3 扩展至 `SegmentMetadata` / `TokenLedgerEntry` 时的实测（证据 `wire_format_compat.rs::nonzero_schema_version_needs_named_form_and_array_form_is_measured_unsafe`）：

`to_vec` 的 array 形态下，若类型中**已存在**可跳过字段（如 `Checkpoint.description: Option<_> + skip_serializing_if`），则任何**其后**新增的"0 时跳过、非 0 时写入"字段会在写入非 0 值时**占据错误的位置**，被解码到前一个字段的类型上——本例表现为 `invalid type: integer \`1\`, expected a string`（响亮失败）；若相邻字段同类型则**静默错值**。

**决策 R9**：以 array 形态持久化的类型，**禁止**新增 `#[serde(default, skip_serializing_if = ...)]` 字段；需要版本化/可选演进的持久类型必须先转具名形态（即先做 F2）。该判据并入 F4′ 门（与字段序冻结同支），因为两者都是"位置即身份"这一编码前提的守护条款。

**对 R8.1 的影响（增强，不改结论）**：Class A 必须具名的理由由两条增至三条——① 同类型字段换序会静默改值；② 前向兼容（旧读端容忍新 key）只有具名形态提供；③ **版本化字段本身在位置式形态下不可实现**。③ 是最直接的一条：它使 F3 依赖 F2，而不是仅"受益于" F2。

**验证缺口自陈并已改流程**：本波最初仅跑 `cargo check -p nexus-contracts`（lib）与单个 test target，漏掉 `tests/wire_roundtrip.rs` 内两处同 crate 结构体字面量（E0063 由 `cargo test` 才暴露），且 `cargo fmt --check` 再次拦到我未格式化的新代码（`ci.yml` 阻塞门）。此后固定顺序：`cargo fmt -p <crate>` → `cargo check -p <crate> --all-targets` → `cargo test`，且收口清单按**本波实际改动的文件集**逐项覆盖，不复用上波清单。
