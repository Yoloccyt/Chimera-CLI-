# Chimera CLI 架构重构与优化方案 v3.5 — 全栈终局总账（L0-L10）

> **输入**：六份融合文档（九源 V1.0 / 三源 v5.0.0 / 十六源 v3.0 / CPU_GPU v6.0.0 / v3.4.0 二十三篇 / v4.0 六编终版，合计约 9800 行实测）+ 既有 v3.0~v3.4 谱系（含冲突账本 C-01…C-36、授权包 P-1…P-5）+ **仓库机器现实**。
> **本轮范围**：全栈 L0-L10（v3.2~v3.4 谱系只覆盖 L0-L3）+ 按用户授权对四项冻结方向重新取证给判。
> **纪律**：一切计数现取；无实测依据的性能数字一律标 `[假设]/[目标]/【待验证】`；文档与仓库冲突时**文档让位于代码**。
> **执行状态**：W6-0 / W6-1 / W6-1b / W6-2 / W6-4（撤主张批已完）/ W6-6 / **W6-8 收口批（G-48 转绿）** 已实施并验证（报告：`v3.5-w60-w62-gate-debt-and-governance-execution-2026-10-08.md` 含新增 §10，与 `docs/reports/v3.5-w66-decision-packages-2026-10-09.md`）；W6-3 部分完成（session-store 装配态订正 + L1 撤主张 + L3 降档为可证伪主张），W6-5 已完四项（R2 假绿防护 / **INV-9 双源对账两层（2026-10-10）**/ 权限三源互转核验 / seccore 口径只登记）其余需 ADR。批次跨自然日：开工 2026-10-08 晚、续做 2026-10-09 与 10-10（文中“2026-10-08”为批次标签，非逐项取证日期）。**终局读数（2026-10-10）**：门清单 **98**（light 91 + heavy 7），全量 light **91 OK / 0 MISMATCH / 0 ENV-FAIL / 7 heavy SKIP**，`[SUM] … 合计 98/98` rc=0；首次跑重门层（`--with-heavy`）得 **G-19 真红 1 条（已修，见下）+ 6 条 heavy 因磁盘 14.2 GB < 门槛 20/25 GB 记 UNKNOWN（运行器拒绝静默跳过，不得当通过）**；全量回归 **505 组 / passed 11758 / ignored 81 / 0 failed**。`fmt` 口径订正：官方门 **G-19 数是 40 个 package（39 crates + 根 `chimera-e2e-tests`）**，此前几轮本批写的“逐包 39/39 洁净”用的是只枚举 `crates/*` 的自建工具 ⇒ **漏根包**，本批自己引入的一处未格式化行因此逃过五轮复验（台账 S-31）。**doctest/E0462 口径（两向收窄，S-30）**：2026-10-10 实测 `repo-wiki --doc` 14 passed、`chimera-mas --doc` 23 passed ⇒ 该限制**不是恒常属性**；但 `cpu-plane-closeout` wave1/wave2 有 `cargo clean -p repo-wiki`（后仍 E0462）与全新 target 下复现的记录，当时已按“A 路认定”判为 windows-gnu 本地系统性限制（权威验证在 CI Linux clean runner）⇒ **不得拿单轮观测撤销历史判定，也不得反向写成“已修复”**（上一轮“定性被否证”的说法过强，已收窄）。
>
> **行号约定**：本文 `file:line` 均为取证时点读数，改前须按符号名重定位（复述行号正是本轮治理的缺陷类）。

## 0 证据等级口径

| 级 | 含义 |
|---|---|
| A | 本次实跑/实测可复核（门输出、脚本解析、`cargo` 读数） |
| B | 登记面文本（`scripts/*.txt`、`gate_manifest.toml`、ADR、CHANGELOG、`docs/reports/`） |
| C | 融合文档主张，未经仓库核对 |
| D | 盘上缺失 / 仅规划 / 零载体设计名 |

## 1 事实基线（现取，全部 A 级）

| 项 | 现值 | 权威源 |
|---|---|---|
| crate 数 | **39**（`workspace.members` 与 `crates/*` 对称差为空） | `Cargo.toml` |
| 版本 | `2.29.0-omega`；最新 tag `v2.29.0-omega`；HEAD `12b6568` @ `feat/c1-compute-plane-governance` | `Cargo.toml:84` / git |
| NexusEvent | **152**（Critical 20 / Info 11 / Normal 121；mpsc 旁路 **16**） | `event-bus/src/types.rs`、`bus.rs:55/62`、`tests/variant_count_test.rs` 四锁 |
| PanelId | 枚举 **30** / `ALL` **30** / `REGISTERED_FOCUS_ORDER` **29**；真不变量 `ALL == FOCUS ∪ {InjectionStrategy}` | `chimera-tui/src/types.rs:1776` + 数组实数 |
| 治理门 | **98**（light 91 + heavy 7）；2026-10-10 本轮由 91→92→94→96→**98**：P-5 升入口级后新增配对门 **G-92**（已接 `ci.yml`）；按 C-52 默认值新增 **G-93/G-94**（向上 dev-dependency 边门 + 自测）；按 C-43 新增 **G-95/G-96**（形式化名册 vs 实装 checker 对账门 + 自测）；按原则 2 取证新增 **G-97/G-98**（v3.4.0 提案名册 vs 代码事实 vs 附录 4A 判定表三方对账 + 自测，`crates/...rs:NN` 证据不可定位即红）。后五组均 `ci_exempt` 写明观察期/待授权理由 | `scripts/gate_manifest.toml` |
| light 门读数 | 开工 **75 OK / 6 MISMATCH / 3 ENV-FAIL** / 7 heavy SKIP → 中段 83 OK / 1 MISMATCH（仅 G-48）→ 终局 84 OK（91 门 light 子集**首次全绿**）→ P-5 批 85 OK → C-48/C-52 批 87 OK → **本批（C-43 入门后，清单 96 条）89 OK / 0 MISMATCH / 0 ENV-FAIL / 7 heavy SKIP，runner rc=0**。**读数自纠（2026-10-10）**：本行旧值与 Spec 基线表分别把开工记成“76 OK / 6 MISMATCH / 3 ENV-FAIL”（相加 85）与“76 / 5 / 3”（列了 6 个 id 却写 5）——按 `tmp/v35-w61-gates-light.txt` 逐行重数实为 **75/6/3，合计 84 = 当时 light 数**（G-07/G-27/G-35/G-44/G-48/G-83 六条 MISMATCH；G-05/G-47/G-52 三条 ENV-FAIL）。这类“分类计数不闭合”已不再靠人：`run_gate_manifest.py` 末尾新增 `[SUM] ok/mismatch/env-fail/unknown/skip` 自报与**记账不闭合即退 2**（牙齿：去掉 `len(skipped)` 一项 ⇒ rc=2，`tmp/closure_teeth.py`）。“判决行数 == 预期 light 数”仍为必断言项（S-18/S-22）。中途一次真实回红：G-44 拦住新写门自身的 4 处入口契约缺陷，已改脚本而非加登记。**原则 2 取证批（清单 98 条）：91 OK / 0 MISMATCH / 0 ENV-FAIL / 7 heavy SKIP，`[SUM] … 合计 98/98`，rc=0**；新门首跑即抓到我自己写下的不可定位证据（LANDED 行缺 `:94` 行号）⇒ 补行号而非进基线；新脚本本身也被 G-44 拦到 1 条 C2（`__main__` 无 crash→2 护栏），同样改脚本而不登记 | `tmp/v35-w61-gates-light.txt`（开工）、`tmp/v35-gates-pathfix.txt`（中段）、`tmp/v35-gates-w6close.txt`（终局）、`tmp/p5_light_run2.txt`（P-5 批）、`tmp/n4_light.txt`与`tmp/o5_light.txt`（C-48/C-52 批）、`tmp/z5_light.txt`（本批 89/89） |
| ADR | 物理 104 档 / 最大主编号 **201** / 索引 117 行；**仅 4 档入 git** | `docs/architecture/` + `git ls-files` |
| 依赖边台账 | 143 行，口径 = `[dependencies]`（optional 边不在册） | `scripts/dep_edge_freeze.txt` 头注（本轮写明） |
| **登记台账覆盖面（原则 4 首次穷尽验证）** | **24 张**（Spec 写 17，已漂）；逐张查两件独立事：**自声明变更方向 24/24**、**被 expect=0 门读取 24/24**，零孤儿。口径订正：不是“24 张都只减不增”——部分登记册声明的方向是“发现必登记/删条目需留去向”，二者不可混写 | `tmp/audit_ledger_gates2.py`（枚举 + 双向判定；该工具自己两轮假阳性已记报告 §21.1） |
| 人类可读层图对拍（原则 1） | G-59/G-45/G-46 现比对 **6 源**：`.sh layer_of()` / `.sh layered_crates` / `.ps1 $layerMap` / `Cargo.toml members` / `[workspace.dependencies]` 分组 / **`CODE_WIKI.md` §2.1 ASCII 层块**；现读 39=39=39=39=39=39 全等 rc=0，夹具由 11 项增至 17 项，并在真 CODE_WIKI 上跑两处变异均判红后字节还原。**仍不得写“所有层图都是派生物”**：§2.1 依旧是手写块，只是从今日开始被门看着 | `scripts/check_layer_map_parity.py`、`tmp/doc_layer_teeth.py` |
| 测试基线 | passed 11722 / ignored 81 / 504 组；失败 target 仅 `chimera-mas --doc`、`repo-wiki --doc`（原文定性为“windows-gnu E0462 族系统性限制”） | `docs/reports/v3.4-w5-closeout-2026-10-08.md`（该定性 2026-10-10 实测已不复现，但历史上有跨 `cargo clean` 复现记录 ⇒ 口径收窄为“不恒常”，见下两行与报告 §29/§31） |
| 全量回归（2026-10-10 INV-9 对账第二层复测） | **505 组 / 11758 passed / 0 failed / ignored 81**（组数 +1 = 新 target `chimera-mas/tests/inv9_parity_proptest.rs`；passed +4 = 该文件的四个测（三个 proptest 性质 + 一个边界矩阵），**增量与改动精确吻合**；同一份数字在“冗余 dev-dep 回退前/后”各跑一遍完全一数 ⇒ 不拿推断当读数）；clippy `-p chimera-mas --all-targets` rc=0、fmt rc=0、`check_dependency_rules.sh` 全通过；牙齿见报告 §26.4（L5 改为跳过自环 ⇒ 立刻 FAILED 报“双源漂移”，还原后字节全等）。**修正上行的“系统性限制”定性（2026-10-10 单独复跑；该结论已于同日收窄，见报告 §31）**：同一工具链（rustc 1.97.1，host `x86_64-pc-windows-gnu`）上 `cargo test -p repo-wiki --doc` = **14 passed / 0 failed**、`cargo test -p chimera-mas --doc` = **23 passed / 0 failed / 3 ignored**——正是基线点名“失败”的那两个 target ⇒ 该限制**不是恒常属性**，当前失败 target 集合为 **空集**；但本行上一版写的“不成立”说过强——`cpu-plane-closeout` wave1 有 `cargo clean -p repo-wiki`（移除 7.4 GiB）后**仍 E0462**、wave2 有全新 target 下 chimera-mas 亦复现并据“A 路认定”升级的记录 ⇒ 正确口径是**当前不复现 ≠ 已消失**，也不得反向写成“已修”。九月失败的确切成因**不可复现**；但 E0462（found staticlib / 无效元数据）与 E0460（possibly newer version）的错误码语义就是**产物与 rustc 版本错配**，故成因属“错配态”而非平台——这一归因是**推断（B 级）**，不得当 A 级写。另：发布门全链已逐项跑齐（见 §14 W6-7 行与报告 §22.1） | `tmp/final15_reg.txt`与`tmp/final15_reg2.txt`（回退前后各一份）、`tmp/reg_totals.py`（三桶闭合聚合）、`tmp/w2_test.txt`（末次全链复跑） |
| 本机硬件 | 16 逻辑核 / 15.19 GB RAM / **RTX 4060 Laptop（Status OK）+ Radeon 780M** | `Win32_VideoController` / `Win32_ComputerSystem` |

## 2 统一问题地图（本轮定性，按"病灶类型"而非按层）

终局判断：**项目不缺能力，缺的是"主张与现实的对应关系"。** 全部病灶归为五类：

| 类 | 定义 | 本轮实例（含处置） |
|---|---|---|
| **P1 零载体设计名** | 文档/注释宣称的机制在码内无实现载体 | `CoreLoopEvent`（内环唯一强制点是治理脚本名单）；MCA `bounded mpsc`（`STREAM_CHANNEL_CAPACITY=256` 声明于 `mca-gateway/src/sse.rs:24` 但**无通道构造点**，`feed()` 返回 `Vec`）；`nexus-moe-router` 等 14 个十六源提案 crate 全部 `Test-Path=False`。处置：一律标 D 级，禁止进入任何施工前提 |
| **P2 账实不符** | 登记表/契约头声称的状态与代码不一致 | `session-store` CRATE-CONTRACT `CONSUMERS 1 / MATURITY TRUE`，但生产 `store: None`（`with_session_store` 唯一构造点在 `server.rs` 的 `#[cfg(test)]` 之后）；`ADR-195` "ONNX 族 0 命中" vs `tract-onnx` 是 `nmc-encoder` 现役生产依赖；影子期"≥14 天已满"vs **无起算时钟**。处置：ADR 注记 + 决策包，不擅改机器派生字段 |
| **P3 判据缺陷（假红/假绿）** | 门自己的写法与自述语义相反 | **G-83**：排除式 `"/tests/" in "/"+rel_dir` 对顶层 `tests` 恒 False ⇒ 把测试夹具当生产码判红（**本轮已修 + 15 条夹具**）；**G-27/G-35**：跨行方法链过近似 ⇒ 同一处两次被迫 re-key（**本轮已消形状，键 9→8**）；**G-07**：把 selftest 字面量 `"build.rs"` 解析成真实路径依赖（本轮由入库消解，但抽取器仍看不见 Rust `mod` 面）。假绿向：`r2_freeze_violation_e2e` 的 `if src_dir.exists()` 短路（auto-dpo 已删 ⇒ vacuous）、`slo.yml:68 continue-on-error: true`（判红不阻断） |
| **P4 结构性债** | 真实存在但需要专门波次偿还 | fnlen 7 项（685/675 行 `i18n::lookup` 等，**本轮已实证与本批无关**）；L0 契约 165/236 公开项消费者 <2；event-bus 零装配子系统 **5 个、约 2.7k 行**（旧值“2.2k/8 个”已推翻，§7.2）；ENV-FAIL 3 条（本机 `python3` 缺失，已根治为 0） |
| **P5 数字漂移** | 文档值与现值不可对齐 | 见 §7 漂移表；处置原则 = **计数不入文档，只入登记面**（`doc_count_freeze.txt` 现仅 3 键：152/10/39） |

## 3 终局架构原则（本轮及后续批次的最高准绳）

1. **层数与层表唯一化**：保持 L0-L10；唯一权威层表 = `scripts/check_dependency_rules.sh::layer_of()`，人类可读层图（CODE_WIKI §2.1）为其镜像，**派生生成、禁止手抄**（G-59 已按 `layer_of()` 比对 LAYER 字段，机制现成）。被否证的层义：十六源 §8.0（9 处冲突：mcp-mesh L9→实 L10、nexus-app-server L8→实 L10、mas-sched L5→实 L9、nexus-subagent L5→实 L7、nexus-hook L2→实 L9、nexus-core L0→实 L1、csn-substitutor L7→实 L10 等）、九源 §7.4（被 ADR-145~152 否证）、v4.0 §5.2（含 4 个幽灵 crate：model-router/auto-dpo/acb-governor/lsct-tiering）。**正面例外（本轮新结论，此前谱系未显式登记）**：v4.0 §5.2 的**五个新 crate 层义**（app-server=L10 / hook=L9 / subagent=L7 / mas-sched=L9 / session-store=L3）与 `Cargo.toml` 实档及 `layer_of()` **逐项一致**，即六源中**唯一**与仓库相符的层图表述（已在该文档头部标注固化）；但它的其余部分（幽灵 crate 引用、§13 WI 回执缺失、§15 假设值）仍属失真区。C-21 只裁过 3 点，须升格为全表裁决（需 ADR-202）。
2. **crate 总账冻结**：不批准新增 crate（39 已达 v2.29 预算且 7 个无生产消费者）；不批准新的跨层直连。v3.4.0 §4.1「括号内新 crate」经逐项三态判定后**没有一项成为新 crate**（对齐 ADR-049“0 新增 crate”）：§4.1 口径 70 项 = 已以模块/符号落地 36（全名级）+ 28（异名或散落级，附 file:line 证据）+ 无载体作废 6；并入 §4.2 组件清单后并集 101 项 = 58 + 33 + 10。**旧文本的两处缺陷：数量漂移（“约 40 项”→ 真值 70/101），与无逐项证据**；选言“全部作废或已以模块落地”本身成立，但容易被读成“多数已作废”——作废实为少数（§4.1 口径 6/70）；逐项表与判定规则见 §4 附录 4A，取证入口 = **G-97/G-98**（机械层 `scripts/check_v340_crate_proposals.py`，含 11 项夹具与人判层 `tmp/v340_three_state.py` + `tmp/v340_appendix.py`，2026-10-10）。
3. **主张必须可证伪**：凡"已落地/已接线"须能指到**生产构造点**；指不到即降级为 `DESIGN-NAME-NO-CARRIER` / `DORMANT`。"零 Stub" ≠ "已装配"。
4. **台账只减，且优先"减条目"而非"改条目"**：行号键的续期（re-key）本身是缺陷复发源；能用重构消掉形状就不加豁免（G-27 先例）。
5. **减法优先序**：删除 > 合并 > 下沉 > 抽象 > 新增；修判据 > 加登记豁免。
6. **性能诚实**：未实测阈值禁入三表（`perf_redlines.toml`/`perf_thresholds.toml`/`bench_check.yml`，ADR-195 D7）；跨轮绝对值不可比，只比同轮配对；无实测数字一律【待验证】。
7. **冻结项不因硬件或意见解冻**：只按各自 ADR 判据（可机检形式）复评。

## 4 crate 总账（39 行，装配态口径）

> 装配态：`WIRED` 生产链有构造点｜`PARTIAL` 部分接线｜`GATED` feature 门控｜`DORMANT` 声明装配但生产关闭｜`DECORATIVE` 零生产引用（台账在册）。层级源 = `layer_of()`；装配态源 = `gated_wiring_baseline.txt` + 本次符号反查（G-56/G-57 自报 `optional internal deps of chimera-cli: 10; debt rows 9`）。

| crate | L | 装配态 | crate | L | 装配态 |
|---|---|---|---|---|---|
| nexus-contracts | 0 | WIRED | parliament | 8 | WIRED |
| nexus-core | 1 | WIRED | decb-governor | 8 | WIRED |
| event-bus | 1 | WIRED | quest-engine | 9 | WIRED |
| nmc-encoder | 2 | WIRED | efficiency-monitor | 9 | WIRED |
| hcw-window | 2 | WIRED | chimera-mas | 9 | WIRED |
| mlc-engine | 2 | WIRED（部分模块 ② 态） | gea-activator | 9 | 生产边仅 dev-deps |
| scc-cache | 3 | WIRED（有构造零数据流） | mas-sched | 9 | DECORATIVE |
| cmt-tiering | 3 | WIRED（落盘真、分层决策未闭） | nexus-hook | 9 | DECORATIVE |
| session-store | 3 | **DORMANT**（生产 `store=None`） | chimera-cli | 10 | WIRED（唯一组合根） |
| seccore | 4 | WIRED | chimera-tui | 10 | WIRED |
| qeep-protocol | 4 | WIRED（唯一消费者 gqep） | chtc-bridge | 10 | DECORATIVE |
| decay-engine | 4 | PARTIAL | mca-gateway | 10 | GATED（`--features mca`） |
| repo-wiki | 5 | WIRED | nexus-app-server | 10 | WIRED |
| gsoe-evolution | 5 | WIRED（R2 冻结域，改动需 ADR） | mcp-mesh | 10 | WIRED（进程内 ephemeral） |
| osa-coordinator | 6 | WIRED | csn-substitutor | 10 | DECORATIVE |
| kvbsr-router | 6 | WIRED | omega-learner | 6 | PARTIAL（默认轨不生效，见 §6.3） |
| faae-router | 6 | WIRED | pvl-layer | 7 | WIRED |
| sesa-router | 6 | DECORATIVE | gqep-executor | 7 | WIRED（4 子模块零生产构造，WI-16 未真闭） |
| ssra-fusion | 7 | DECORATIVE | mtpe-executor | 7 | DECORATIVE |
| nexus-subagent | 7 | DORMANT（构造仅 `#[cfg(test)]`） | | | |

账合：39。**本轮结论：不删任何一个**（DECORATIVE 是"未接线"而非"无价值"，ADR-145/146/148 交付物有回退语义；退役需仿 ADR-193 先例走独立 ADR）。

<!-- BEGIN v340-4.1-three-state-appendix -->
### 附录 4A：v3.4.0 §4.1/§4.2 「括号内新 crate」逐项三态判定（原则 2 的逐项证据）

取证器：机械层 `scripts/check_v340_crate_proposals.py`（已入门清单 **G-97/G-98**，含 11 项夹具）；人工终判层与本表回写由 `tmp/v340_three_state.py` + `tmp/v340_appendix.py` 完成（标记块幂等）。源：`Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §4.1（ASCII 架构图 line 277-356，括号组跨行且 L1 存在嵌套 `[`）与 §4.2（「新增/强化组件清单」表 line 357-434）；取证日期 2026-10-10。本表与源文档/代码的对账不再靠人：G-97 校「候选名集合 == 表行集」与「每条 `file:line` 证据真存在且不越界」。

**候选名规则**：括号组 / 表格反引号内、形状像 crate 的 token（去 `(预留)` 尾注，剔除中文组），按名去重。
**机械五态**：① `crates/<name>` 在籍｜② 同名模块文件在册｜②b 同名声明符号在册（大小写与下划线不敏感）｜②c 专名词根痕迹（词根在候选名集合内唯一且命中 <=3 个 crate）｜③ 无任何痕迹。②c/③ 共 43 行**逐行人工确证**（机械规则只能证明“有痕迹”，不能证明“同一事物”）。

**记账闭合**：并集 101 = IN-PLACE 58（① 12 + ② 30 + ②b 16） + 改判 LANDED 33 + 改判 VOID 10；§4.1 口径 70 = 36 + 28 + 6。① 类 12 项全部来自 §4.2 组件列的基线既有 crate，**§4.1 括号内成为新 crate 者 = 0** ⇒ 原则 2 的「不批准新增 crate」与 ADR-049 一致。

**对原则 2 原文的订正**：原写“约 40 项”是漂移数（真值：§4.1 括号内 70 项，并入 §4.2 后 101 项）。选言断言本身成立（每项要么是模块落地要么是作废，无第三种），但它此前**没有逐项证据**，且容易被读成“多数已作废”——事实上作废是少数：§4.1 口径 6/70，并集 10/101；多数（64/70）以既有 crate 内的模块或符号形式落地，其中 28 项属于异名或散落落地。

| 提案名 | 层 | 来源 | 机械判定 | 终判 | 证据 |
|---|---|---|---|---|---|
| `aegis-gsoe` | L5 | §4.1+§4.2 | ②c PARTIAL | LANDED | crates/gsoe-evolution/src/aegis/critic.rs:75 AegisCritic |
| `ambient-mode` | L9 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：quest-engine） |
| `ancestor-sibling-index` | L2 | §4.1+§4.2 | ②c PARTIAL | LANDED | crates/mlc-engine/src/on_demand_synthesizer.rs:255 find_ancestors / :275 find_siblings（函数级，无独立 index 模块） |
| `atomic-operators` | L7 | §4.1+§4.2 | ③ VOID | LANDED | crates/nexus-contracts/src/memory/experience_card.rs:65 enum AtomicOperator；faae-router/src/operator_router.rs:130 OPERATOR_ORDER |
| `auto-builder` | L4 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：pvl-layer） |
| `behavior-contracts` | L0 | §4.1 | ③ VOID | LANDED | crates/nexus-contracts/src/spec/behavior_contract.rs:29 BehaviorContract |
| `behavior-localization` | L5/L8 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：repo-wiki） |
| `checkpoint-preserver` | L5 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：gsoe-evolution） |
| `chimera-tui` | L10 | §4.2 | ① CRATE | IN-PLACE | 机械判定即终判（crates/chimera-tui 在籍） |
| `cmt-tiering` | L3 | §4.2 | ① CRATE | IN-PLACE | 机械判定即终判（crates/cmt-tiering 在籍） |
| `conflict-arbitration` | L8 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：parliament） |
| `conflict-degradation` | L2 | §4.2 | ③ VOID | VOID | ConflictDegradation/conflict_degradation 零命中；parliament 只有 conflict-arbitration（另一提案） |
| `critical-path-identifier` | L8 | §4.1 | ③ VOID | LANDED | crates/parliament/src/critical_path.rs:74 CriticalPathReport |
| `dag-viz-panel` | L10 | §4.1 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：chimera-tui） |
| `dqn-trainer` | L2 | §4.2 | ②c PARTIAL | LANDED | crates/cmt-tiering/src/rl_migration.rs:68 DQNMigrationPolicy（异层：提案 L2 实落 L3） |
| `dual-experience-bank` | L5 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：mlc-engine） |
| `dual-experience-pool` | L3 | §4.1 | ③ VOID | VOID | 无 Pool 载体；同名近体 crates/mlc-engine/src/dual_experience_bank.rs:49 是另一提案（dual-experience-bank） |
| `dynamic-verification-depth` | L7 | §4.1+§4.2 | ②c PARTIAL | LANDED | crates/pvl-layer/src/dynamic_depth.rs:28 enum VerificationDepth（异名：dynamic_depth） |
| `entropy-weighted-scoring` | L7 | §4.1+§4.2 | ②c PARTIAL | LANDED | crates/pvl-layer/src/dynamic_depth.rs:199 pub struct EntropyWeightedScorer |
| `error-signature-collector` | L4 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：seccore） |
| `error-signature-contracts` | L0 | §4.1+§4.2 | ③ VOID | LANDED | crates/nexus-contracts/src/memory/experience_card.rs:169 struct ErrorSignature |
| `error-signature-index` | L3 | §4.1+§4.2 | ③ VOID | LANDED | crates/cmt-tiering/src/experience_card_storage.rs:168 CREATE INDEX idx_error_hash |
| `event-bus` | L1 | §4.2 | ① CRATE | IN-PLACE | 机械判定即终判（crates/event-bus 在籍） |
| `execution-status-contracts` | L0 | §4.1+§4.2 | ②c PARTIAL | LANDED | crates/nexus-contracts/src/memory/experience_card.rs:190 enum ExecutionStatus |
| `experience-card` | L0 | §4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：nexus-contracts） |
| `experience-card-bus` | L1 | §4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：event-bus） |
| `experience-card-cache` | L2 | §4.1 | ③ VOID | LANDED | crates/cmt-tiering/src/experience_card_storage.rs:222 update_hot_cache（卡片热缓存落在存储层，非独立模块） |
| `experience-card-collector` | L9 | §4.1 | ③ VOID | VOID | CardCollector/card_collector 零命中；seccore 的 error_signature_collector 是另一提案 |
| `experience-card-contracts` | L0 | §4.1 | ③ VOID | LANDED | crates/nexus-contracts/src/memory/experience_card.rs:297 struct ExperienceCard |
| `experience-card-generator` | L7 | §4.1+§4.2 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：pvl-layer） |
| `experience-card-storage` | L3 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：cmt-tiering） |
| `experience-card-system` | L2 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：mlc-engine） |
| `experience-card-viz` | L10 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：chimera-tui） |
| `four-operators` | L5 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：gsoe-evolution） |
| `global-experience-board` | L2 | §4.1+§4.2 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：mlc-engine） |
| `grpo-trainer` | L5 | §4.2 | ②c PARTIAL | LANDED | crates/gsoe-evolution/src/policy/grpo.rs:69；types.rs:90 GrpoRollout |
| `gtpo-trainer` | L7 | §4.2 | ②c PARTIAL | LANDED | crates/pvl-layer/src/gtpo.rs:30 struct GTPO |
| `harness-config-manager` | L6 | §4.1 | ③ VOID | LANDED | crates/nexus-contracts/src/spec/harness_spec.rs:421 struct HarnessSpec（异名） |
| `hcw-window` | L2 | §4.2 | ① CRATE | IN-PLACE | 机械判定即终判（crates/hcw-window 在籍） |
| `hils-attention` | L2 | §4.1+§4.2 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：hcw-window） |
| `hint-boosted-recovery` | L7 | §4.1 | ②c PARTIAL | LANDED | crates/pvl-layer/src/hint_recovery.rs:32 HintCategory / :63 HintRecovery |
| `injection-strategy` | L2 | §4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：chimera-tui） |
| `long-task-map` | L9 | §4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：quest-engine） |
| `long-term-credit-assigner` | L9 | §4.1+§4.2 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：quest-engine） |
| `mappo-trainer` | L8 | §4.2 | ②c PARTIAL | LANDED | crates/parliament/src/mappo.rs:213 struct MAPPO（离线研究面，见报告 W6-4 撤主张） |
| `memory-graph` | L2 | §4.1 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：mlc-engine） |
| `memory-pyramid-contracts` | L0 | §4.1+§4.2 | ③ VOID | LANDED | crates/nexus-contracts/src/memory/memory_pyramid.rs:42 |
| `memory-sideagent` | L2 | §4.1 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：mlc-engine） |
| `meta-agent-adapter` | L5 | §4.1 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：gsoe-evolution） |
| `mlc-engine` | L2 | §4.2 | ① CRATE | IN-PLACE | 机械判定即终判（crates/mlc-engine 在籍） |
| `msce-integration` | L5 | §4.1+§4.2 | ③ VOID | LANDED | crates/nexus-contracts/src/memory/memory_pyramid.rs:241 with_msce_elements |
| `nexus-contracts` | L0 | §4.2 | ① CRATE | IN-PLACE | 机械判定即终判（crates/nexus-contracts 在籍） |
| `nexus-core` | L1 | §4.2 | ① CRATE | IN-PLACE | 机械判定即终判（crates/nexus-core 在籍） |
| `omni-message` | L0 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：nexus-contracts） |
| `on-demand-synthesizer` | L2 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：mlc-engine） |
| `openforge-proxy` | L1 | §4.2 | ③ VOID | VOID | 仅 //! 论文出处注释（event-bus/src/segment_per.rs:5、token_ledger.rs:5），无代码实体 |
| `operator-contracts` | L0 | §4.1+§4.2 | ③ VOID | LANDED | 算子契约类型在 crates/nexus-contracts/src/memory/experience_card.rs:65（无独立 operator_contracts.rs） |
| `operator-router` | L6 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：faae-router） |
| `osa-coordinator` | L6 | §4.2 | ① CRATE | IN-PLACE | 机械判定即终判（crates/osa-coordinator 在籍） |
| `output-validator` | L4 | §4.1 | ②c PARTIAL | VOID | OutputValidator/validate_output/output_validation 全库零命中 |
| `paddock-sandbox` | L4 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：seccore） |
| `parent-selector` | L6 | §4.1+§4.2 | ②c PARTIAL | LANDED | crates/faae-router/src/parent_context.rs:31 ParentSelection |
| `parliament` | L8 | §4.2 | ① CRATE | IN-PLACE | 机械判定即终判（crates/parliament 在籍） |
| `per-priority-sampler` | L3 | §4.1 | ③ VOID | VOID | PrioritySampler/per_priority 零命中（§4.1 原注即「预留」） |
| `platform-grounding-specs` | L0 | §4.1 | ②c PARTIAL | LANDED | crates/nexus-contracts/src/spec/platform_grounding.rs:57 PlatformGroundingSpec |
| `ppo-trainer` | L6 | §4.2 | ②c PARTIAL | LANDED | crates/seccore/src/asa.rs:207 with_ppo(…PpoCritic)（异层：提案 L6 实落 L4） |
| `procedural-blueprint` | L5 | §4.1 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：repo-wiki） |
| `process-score-calculator` | L7 | §4.1+§4.2 | ②c PARTIAL | LANDED | crates/pvl-layer/src/process_score.rs:42（lib.rs:98 pub mod process_score） |
| `pvl-layer` | L7 | §4.2 | ① CRATE | IN-PLACE | 机械判定即终判（crates/pvl-layer 在籍） |
| `pyramid-storage` | L3 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：cmt-tiering） |
| `quest-engine` | L9 | §4.2 | ① CRATE | IN-PLACE | 机械判定即终判（crates/quest-engine 在籍） |
| `retrieval-three-way` | L2 | §4.2 | ②c PARTIAL | VOID | 唯一 three_way 命中是 nexus-core/src/compute/bridge.rs:577 的 #[test] fn decide_three_way，语义为算力三态而非记忆检索 |
| `rl-client` | L1 | §4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：chimera-cli,nexus-core） |
| `rl-hook-contracts` | L0 | §4.1+§4.2 | ②c PARTIAL | LANDED | crates/nexus-contracts/src/seams/rl_hooks.rs:50 |
| `rl-types` | L1 | §4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：nexus-contracts） |
| `runtime-auditor` | L10 | §4.1+§4.2 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：efficiency-monitor） |
| `search-tree-manager` | L9 | §4.1+§4.2 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：quest-engine） |
| `search-tree-viz` | L10 | §4.1 | ③ VOID | VOID | crates/chimera-tui/src/panels/search_tree_viz.rs 不存在；quest-engine 的 on_search_tree_best_path 是记忆同步钩子，非 L10 面板 |
| `seccore` | L4 | §4.2 | ① CRATE | IN-PLACE | 机械判定即终判（crates/seccore 在籍） |
| `segment-aware-per` | L1 | §4.2 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：event-bus） |
| `segment-aware-validation` | L7 | §4.1+§4.2 | ②c PARTIAL | LANDED | crates/event-bus/src/segment_per.rs:178 SegmentAwarePER |
| `self-assessment-panel` | L10 | §4.1+§4.2 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：chimera-tui） |
| `six-class-feedback-integrator` | L4 | §4.1+§4.2 | ③ VOID | VOID | SixClass/six_class 全库零命中 |
| `six-dimension-adjuster` | L6 | §4.1+§4.2 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：osa-coordinator） |
| `six-dimension-contracts` | L0 | §4.1+§4.2 | ③ VOID | LANDED | crates/osa-coordinator/src/six_dimension.rs:94 pub struct SixDimensionAdjuster（六维控制面契约本体在 osa-coordinator，非 L0 独立模块） |
| `skill-graph` | L5 | §4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：repo-wiki） |
| `skill-lifecycle` | L5 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：nexus-contracts,repo-wiki） |
| `skill-lifecycle-contracts` | L0 | §4.1+§4.2 | ③ VOID | LANDED | crates/nexus-contracts/src/memory/skill_lifecycle.rs:34 + crates/repo-wiki/src/skill_lifecycle.rs:30 |
| `skills-progressive-loader` | L6 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：repo-wiki） |
| `stat-learning-policy` | L1 | §4.2 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：nexus-core） |
| `stop-ruling` | L8 | §4.1 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：parliament） |
| `stop-strategy` | L8/L9 | §4.1+§4.2 | ③ VOID | LANDED | crates/nexus-contracts/src/spec/harness_dimensions.rs:154 StopStrategyConfig |
| `three-factor` | L0 | §4.2 | ③ VOID | VOID | §4.2 表内反引号片段，不是完整提案名（解析噪声，已计入分母以示诚实） |
| `three-factor-adjudicator` | L8 | §4.1+§4.2 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：parliament） |
| `three-factor-index` | L3 | §4.1+§4.2 | ③ VOID | LANDED | crates/cmt-tiering/src/experience_card_storage.rs:163 idx_three_factor + :312 query_by_three_factor |
| `three-factor-selector` | L5 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：gsoe-evolution） |
| `token-evidence-contracts` | L0 | §4.1+§4.2 | ②c PARTIAL | LANDED | crates/nexus-contracts/src/memory/token_evidence.rs:39（模块在册；无同名 struct） |
| `token-ledger` | L1 | §4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：event-bus） |
| `tool-schema-pruner` | L6 | §4.1+§4.2 | ②b SYMBOL | IN-PLACE | 机械判定即终判（同名声明符号（大小写不敏感）于：osa-coordinator） |
| `variant-parliament` | L8 | §4.1+§4.2 | ③ VOID | LANDED | crates/parliament/src/variant_pool.rs:31 VariantPool（异名） |
| `variant-pool` | L5 | §4.1+§4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：parliament） |

终判口径：IN-PLACE = 机械层已能直接定论（①/②/②b）；LANDED = 人工确证为同一事物的异名或散落落地；VOID = 无载体（按原则 2 作废）。LANDED 不等同于“已接线”：接线与否另需生产构造点（原则 3）。

<!-- END v340-4.1-three-state-appendix -->

## 5 接口契约库与事件契约

### 5.1 L0 契约面（`crates/nexus-contracts/src/`，54 文件 / 236 个顶层 pub 项）

- 实测：`pub enum` 103 + `pub struct` 124 + `pub trait` 9；`contract_consumer_baseline.txt` **165 行豁免**（判据"每个 L0 pub 类型须 ≥2 生产消费 crate"）⇒ **约 70% 公开项无第二消费者**（A 级）。
- "纯类型零依赖"的实况：L0 内含算法（`capability_token` 70 / `vector` 62 / `harness_spec` 52 个 `pub fn`）；`util.rs` 实为 **7 个 `pub fn`**（其中 5 个零副作用纯函数属 ADR-033 受控例外，`perf_scale`/`perf_scale_ms` 读 env 属测量旋钮，**非纯函数**）。已按实况回写 CODE_WIKI（本轮完成）。
- 反向依赖：**无**（`omega_learner::` 命中全在注释）。
- 双源风险：L0 `SeamId`（S1..S9）与 `omega_learner::SeamId` 物理独立且**规模不等**（`capability_token.rs:97-100` 自陈）⇒ P4-W14.6 上提单源，触核心领域类型需 ADR。
- 治理建议：165 项豁免采**季度删除制**（每季必须有 STALE 删除，否则门本身失去意义）。

### 5.2 L1 事件契约（`event-bus`，24 文件）

- 单源与锁：`types.rs` 152 变体 ↔ `registry.rs` `define_event_registry!` 152 行 ↔ `variant_count_test.rs` 四锁（`VARIANT_COUNT_LOCK=152` / `CRITICAL_LOCK=20` / `INFO_LOCK=11` / `NORMAL_LOCK=121`）+ `variant_ordinals()` 编译期穷举 + `test_critical_set_matches_lane_forbidden_shard` 双向互锁 —— **本次逐项核对一致**（A 级）。
- Critical 分层：20 个 Critical 中 **16 走 mpsc 旁路**、4 只走 broadcast（`CheckpointSaved`/`ConsensusReached`/`SlowConsumerDropped`/`OrphanCallDetected`）。判定为**设计意图**（`bus.rs` + `registry.rs` + 测试三处互证），非遗漏；缺陷仅在于注释复述数字导致漂移（`registry.rs` 两处"13"，本轮已改为指向常量名）。
- 零装配子系统（2026-10-10 逐模块重测：**5 个、约 2.7k 行**；旧行写“约 2.2k 行/8 个”已被现取推翻，见 §7.2）：`TokenLedger`（约 0.6k）/`SegmentAwarePER`（约 0.4k）/`MembraneFilter`（约 1k）/`PatternIndex`（约 0.3k）/`DynamicEventRegistry`（404 行；本轮补登，crate 内外零引用）——这些类型本身生产构造点为 0。**粒度订正（本轮自查）**：旧行还把 `CausalConsistencyChecker`/`ShadowDiffRecorder`/`VectorClock` 一并归为“零构造”，但这三个情况并不相同：`VectorClock`/`CausalRelation`/`event_fingerprint` **确在已接线的归因链里被用**（`attribution.rs:40/42/57/114/315`），而 `CausalConsistencyChecker`/`ShadowDiffRecorder` 本身仍无构造点 ⇒ **判“零装配”必须按类型粒度，不能按模块名划**。已装配的真链路只有 `attribution::spawn_attribution_tap` 与 `ExperienceCardBus`（经 `chimera-cli/src/experience_loop.rs`）。
  处置：**就地保留 + 撤 L1 语义主张**（迁出为独立 crate 违 ADR-049）。`MATURITY` 是机器派生字段，标 `UNWIRED-SUBSYSTEM` 需先扩词表并增 `impl_maturity_baseline` 行（违只减方向）⇒ **必须走 ADR**，本轮不做（计划偏差已登记）。
- 概念重复待裁：`event-bus::PerBuffer` vs `omega-learner::per_buffer::PerBuffer`；`event-bus::TokenLedger` vs `nexus-contracts::token_evidence::TokenLedgerEntry`。
- 事件密度失配（结构性）：L10 事件最密（Tui\*/Mcp\*/Chtc\*/Ssra\*/Csn\*），**L2/L3/L6 几乎无事件** ⇒ 记忆/存储/路由的学习生效性**不可观测**，这正是 session-store 与 LinUCB 无法被证伪的根因。建议：每层 ≥1 个生产可观测事件（Check E 扩展项）。

### 5.3 同批登记矩阵（施工硬约束，缺一即 CI 红）

| 动作 | 必须同批改 |
|---|---|
| 改事件 | `types.rs` enum → `registry.rs` 注册表行 → `bus.rs::is_critical_mpsc_event` + `LANE_FORBIDDEN_SHARD` → `variant_count_test.rs` 四锁与互锁测 → `event_ownership_baseline.txt` → `doc_count_freeze.txt` → CODE_WIKI/agents.md 措辞 → 涉面板则 `n4_binding_registry.txt` |
| 改门 | `gate_manifest.toml` + 对应 `check_*` + 其 baseline（方向不可反）+ 双清单类必须同批（`lock_await_freeze.txt` **与** `await_guard_allowlist.txt`）+ `doc_count_freeze.txt` |
| 改契约 | `nexus-contracts/src/*` + `contract_consumer_baseline.txt` 逐条对账 + ADR + 涉属性则 `formal_props.rs`/`formal_gate` 名单与 `DEFAULT_MIN_SATISFIED` |
| 改面板 | `PanelId` 枚举 + `ALL` + `REGISTERED_FOCUS_ORDER` + `app/mod.rs` 工厂 + `app/tests.rs` + `tests/integration.rs`（四处断言，穷举守卫在 `types.rs`） |
| 改 crate | `workspace.members` + `dep_edge_freeze.txt` + `crate_reachability_freeze.txt` + `gated_wiring_baseline.txt` + `retired_crates.txt` + `coverage_per_crate_floor.toml` + `impl_maturity_baseline.txt` + `forbid(unsafe_code)` 头 |

## 6 通信与并发拓扑（现状）

### 6.1 通信

| 通道 | 实况 | 判定 |
|---|---|---|
| broadcast 总线 | `bus.rs`（3109 行）单通道 capacity 1024，Lagged→SlowConsumerDropped；全库 `broadcast::channel` 命中**只在 event-bus** | WIRED |
| Critical mpsc | 先 mpsc 后 broadcast；`CRITICAL_CHANNEL_CAPACITY=4096`；生产消费者 5 crate 6 点位，其中组合根侧仅 `tracing::error!` | WIRED（消费语义偏观测） |
| Critical WAL | `FileWalCriticalSink`（open+replay，敏感命中跳过落盘不 panic），at-least-once 语义自陈；启用仅由 env `CHIMERA_CRITICAL_WAL_PATH` 门控，生产构造点 **1** | GATED |
| ShardedBus | `shard.rs` 64 片 × 256 crossbeam + 三车道；9 crate / 10 生产调用点（与 `lib.rs`"九文件十处"声明**逐一对账成立**） | WIRED |
| MCP mesh | `commands/doctor.rs`、`commands/mcp.rs`，两处自注"进程内 ephemeral" | WIRED-EPHEMERAL |
| MCA bounded mpsc | **零载体**（§2 P1） | D |
| 内环直接调用 | 外环→内环 9 条边只减棘轮；`CoreLoopEvent` 无载体 | 治理脚本单点强制 |

### 6.2 并发

| 原语 | 分布（src 作用域） | 真实链路要点 |
|---|---|---|
| rayon | 281 命中 / 13 crate（nexus-core 143 为最） | CPU 热路径唯一卸载入口 = `ComputeBridge::route()` |
| spawn_blocking | 293 / 25 crate（cmt 64、scc 38…） | `sqlite_blocking_allowlist.txt` 仅 **1 行**且标 SCANNER false-positive ⇒ rusqlite-in-async 债务面近乎为零 |
| block_in_place | 46 / 6 crate（faae 16、osa 13…） | 语义 = 占住 worker；前提 `on_multi_thread_runtime()`；LocalSet 边界"登记但不检查"（真闸门是 tokio 私有 `allow_block_in_place`） |
| FuturesUnordered | 68 / 6 | gqep 23、mas 18、parliament 14 |
| 持锁跨 await | **双门 1:1 互锁**（G-27 ↔ G-35，两表键集强制相等） | 本轮键 9→8（真偿还 + 牙齿实证） |
| unsafe | 39/39 crate `forbid(unsafe_code)`；`measure_unsafe.py` 在 | 手写 SIMD 已被 ADR-SIMD-001 否决（三源 §2.6 不可施工） |
| HTS/计算池 | 生产组合根调用点 **1**（`overwindow_bridge.rs` `spawn_compute_batch(TaskKind::ClvSimilarity,…)`）；G-87 = 1 WIRED + 2 CRATE_API_LIVE + 1 DIAGNOSTIC + **5 DEAD** | 卸载可达性未成立（C-39） |

文字化：单 tokio multi-thread 进程 → L10 组合根持事件循环 → 慢 IO 一律 spawn_blocking → CPU 热路径二形态（rayon 经唯一入口 / `block_in_place` 就地）→ 事件扇出 64 片，Critical 恒单流禁分片。

### 6.3 学习面（默认轨实况）

`chimera-cli/src/selector_orchestrator.rs` 首行 `#![cfg(feature = "r2_unfreeze")]` ⇒ 默认轨整模块被编译剔除；`hcw-window/src/window.rs` 恒 `SelectorLearnerHolder::new()`（默认 Static），`with_learner`/`update_selector_policy` 无生产调用点。**结论（A 级）：默认轨 100% 走静态策略，LinUCB 从未下发**；`omega-learner` 台账 `PARTIAL` 属高估（其 crate 自身无 `[features]` 段，模块常驻编译，门控点在 CLI 组合根）。

### 6.4 本轮亲自复测的关键 A 级断言（不依赖子代理转述；可逐条重跑）

| 断言 | 复测命令/方法 | 结果 |
|---|---|---|
| `CoreLoopEvent` 零载体 | `Select-String 'CoreLoopEvent'` over `crates/**/*.rs` | **0 命中** |
| MCA “bounded mpsc”零载体 | 全 `mca-gateway` 检索 `mpsc::channel` 与 `STREAM_CHANNEL_CAPACITY` | 常量**只出现在注释与自身声明**，无通道构造点 |
| L0 公开项规模 | 行首正则计 `crates/nexus-contracts/src/**` | enum **103** + struct **124** + trait **9** = **236** |
| L0 低消费者面 | `contract_consumer_baseline.txt` 非注释行 | **165** 行（即 165/236 未达“≥2 生产消费方”） |
| v4.0 五 crate 层义与仓库一致 | `Cargo.toml` description × `layer_of()` 双源 | app-server=L10 / hook=L9 / subagent=L7 / mas-sched=L9 / session-store=L3 **逐项一致** |
| `session-store` 生产关闭 | `with_session_store` 全库调用点定位 | 唯二调用点 `server.rs:920` / `:977` 均在 `#[cfg(test)]` 区（起点 `:634`）内；组合根 `composition.rs:251` 走 `with_backend` ⇒ `store=None`（`server.rs:283`）；`replay_since`（pub API `server.rs:334`）**无生产调用方** |
| **段测 speedup 不能外推到端到端** | 新增 bench 组 `overwindow_e2e_dense_ab`（三语料 × 四臂，被测对象**自报** `block_count()` 与 `route()`，附同路质控臂） | 实测 681/1100/3684 块 → **1.375× / 1.65× / 2.57×**；§11 同规模 dense 段隔离读数为 2.3–5.0× ⇒ 外推差 **≈49%**（判据 20%）。同路两臂差 ≤3.2%，而质控漂幅 0.3–16.4% ⇒ 本机读数精度下限已量化 |

## 7 冲突账本 C-37…C-63 与数字漂移表

### 7.1 新增冲突（续 v3.4 的 C-01…C-36）

| # | 冲突 | 裁决 | 级 | 本轮状态 |
|---|---|---|---|---|
| C-37 | ADR-195"ONNX 族 0 命中" vs `tract-onnx` 7 包在 `Cargo.lock` 且为 `nmc-encoder` 生产依赖 | 结论不变、证据链改写；G-86 盲区登记且**不扩禁列**（tract 无 GPU feature，禁它误伤） | A | 已回写 §8 |
| C-38 | `HtsTable` 阈值"已校准" | 否证：7 表项全 `OfflineMeasured{measured_at: 0}`，测试主动断言防冒充 | A | 已回写 ADR-196 §11 |
| C-39 | ADR-196 D1 支撑句"CPU 并行度已解决" vs G-87 组合根口径 1/9 | 支撑句永久失效；GPU 否决此后只依赖"SLO 全绿 + ADR-192 达成分层目标" | A | 已回写 |
| C-40 | 影子期"≥14 天" vs ADR-053 rev2/rev4"≥14 独立批次"；2026-07-29 是签署日非起算日 | 以 rev4 为准；并实证"从未起算" | A | 已回写 CODE_WIKI + adr_index |
| C-41 | v3.4.0 §18.2 `FUSION_UNLEARNABLE_RULES` 19 条 vs `seccore::policy::UNLEARNABLE_SECURITY_RULES` **6 条 + `len()==6` 断言门** | 19 条作废；6 条为唯一权威，扩条需独立 ADR，禁与重构同批。裁决已写入唯一权威表 `docs/architecture/nine-layer-defense-map.md` 的「2026-10-08 融合文档口径裁决」节 | A | **已实施** |
| C-42 | 防御层数三口径（十条红线 / "十层防御"实列 11 行 / 16 处注释"九层防御"） | **本行初裁已被证据推翻并当场纠正**：仓内存在唯一权威源 `nine-layer-defense-map.md`（自声明“宣称→证据唯一映射文档”，`doc-roles-matrix.md:95`/`INDEX.md:48` 均确认），正确口径不是“防御面即 L0-L10 十一档”，而是：**防御 L0-L8 与架构 L0-L10 属两个正交分区，仅在部分层重合，不得互推亦不得互计数**（举证：防御 L2 的实现载体 `omega-learner` 是架构 L6；架构 L9/L10 无对应防御层）。实施：该表新增维护规则 4 + 口径裁决节；v3.4.0 §18.1 “十层矩阵”作废引用；16 处“九层防御 Lx”注释**不改名**（它们引用防御分区，改名反而制造错映射） | A/B | **已实施**（含本轮自查纠错） |
| C-43 | "9 属性"主张 vs 实装清单 | **拆成三层，其中两层不是缺陷**：① “9 属性”已由 `c2-formal-verifier-honesty-report-2026-09-04.md` 做过口径订正（宣称清零 + CI gate 接线实证 34/34），`agents.md:360` 的 `7+1+1=9` 与子代理定义文件自洽（P124 已判为假冲突）⇒ **不当新缺陷**；② 真缺陷只有一处：名单 7 项中 `preference-consistency` 的 checker 宿主随 auto-dpo 已删（现由 `EmptyFormalProvider` 恒返 Skipped 承载），而**有实装的 `consensus-safety`（`parliament::formal::ConsensusSafetyChecker`）不在名单**；③ `FormalResultProvider` 全库**只有一实现**（`EmptyFormalProvider`）⇒ “聚合门”无生产数据提供方，真验证链靠各 crate formal 模块 + e2e target。现算清单（A 级）：gsoe×3（Lineage/CriticMonotonicity/InvariantClosure）+ event-bus CausalConsistency + omega-learner LearningMonotonicity + decay-engine DecayConsistency + parliament ConsensusSafety = **7 个真 checker**，与名单交得 6 匹配 / 1 幽灵 / 1 未列 | A | **已定性 + 已入门（2026-10-10）**：G-95/G-96 `check_formal_property_ledger.py` 把两处错配固定为“不可扩大且必须可见”（名册位无 checker 未登记⇒红、checker 文件被剔空⇒红、登记失效⇒红），并在 **ADR-052 新增追加事实注记**（只加注，盘点原文不改）写明“7/7 应读作 7 个名册位、其中 6 个有验证器，另有 1 个已实装验证器未计入”；**换名册本身仍需新 ADR**（它改变 ADR-052/053 已签署的 R2 证据集内容，不得无文书换身份） |
| C-44 | 记忆投毒五道防线 | 换名分散实现（sensitive scan + `ContradictionDetector` 0.9 + `is_current` 幽灵记忆 + `s8_mem_pi` abstain 0.7）+ 真缺（统一 `sanitize()` 入口、多验证器锦标赛、记忆侧 `PendingHuman`、`content_hash` 去重；`MemorySideagent::verify` 阈值 0.6 且零接线） | A | 待 W6-5 |
| C-45 | BashGuard 12 条 + 五级权限 + `permissions.yml` 0 命中 | 换名部分实现（`CommandPolicy::default_secure()` 18 白名单 + 20 blocked_pattern）；真缺 = 权限模型三源统一（`ApprovalMode` 三态 / `PermissionMode` 六模式 / `PermissionTier` L0-L2 **无互转**）+ 外置配置 | A | **互转事实已核（2026-10-09，A 级）**：三类型间 `impl From/TryFrom/Into` 全库 0 命中，`ApprovalMode` 唯一看似桥接的 `capability_policy_label` 只返回 i18n 文本键并自陈“不触达能力层”⇒“无互转”从推断升为事实；单一权威 + 外置配置需契约级 ADR |
| C-46 | Critical 20 vs mpsc 16 是否遗漏 | 设计意图（三处互证）；缺陷 = 注释复述数字（已修）+ I8/I8b 不校验旁路子数 ⇒ **已由 I8c 闭合（2026-10-09）**：两个静态子断言（`CRITICAL_MPSC_VARIANTS == is_critical_mpsc_event` 臂数；“清单/判定的 + N 类”措辞 == 常量），落地当日即抓到 `backpressure_test.rs:49` 残留旧值 4⇒16，并因 4 处“本测试覆盖 4 类”假阳性而收紧为**双条件锚** | A | **已闭合**（invariants 与 `--static-only` 双 rc=0） |
| C-47 | `crate_reachability_freeze.txt` 零实账 vs gated 台账缺行 | **子代理转述被否证**：`sesa-router`/`ssra-fusion` 均在册（9 行=上限 9），落点与论域判断亦错（session-store 不属该台账论域） | A | 不改台账；改走决策包 |
| C-48 | ADR-188 D3"nexus-hook 真接线 B4" vs 台账 `DECORATIVE` + `nexus_hook::` 外部零引用 | 直接冲突，需复核 ADR 二选一（接线 / 撤主张），默认**撤主张** ⇒ **已按默认执行**（2026-10-10）：ADR-188 新增 §5 追加更正**撤回主张**为未排期规划项（原文逐字保留），并附接线前置清单（新 ADR 定宿主 + dep_edge/gated/reachability 三本台账同批）；另查出一处**台账内部自相矛盾**：`crate_reachability_freeze.txt` :71 “wired” 与 :73 “remain GATED” 对同一对象相反结论，已补订正注记封误读（不改计数行）；CODE_WIKI nexus-hook 小节新增装配态行。**零代码/零台账行变更**（不为兑现主张而造构造点，同 C-60 口径） | A | **已撤主张并三处回写** |
| C-49 | 六文档自拟 ADR 号段互斥（v3.4.0 077~095 / v5 095~102 / v6 103~110 / 十六源 170~182）与仓库同实档异义（`ADR-170`=Ω 十一定律） | 融合文档临号**整体作废**；新号自 ADR-202 起且先查 `adr_index.md` 重复 | B | 待 W6-2 余批 |
| C-50 | `ADR-XXX-inner-ring-bug-fix.md` 未入 git ⇒ 原计划"移出 docs/architecture/" | **处置更正**：`.gitignore` 排除 `*.md` 是刻意政策（"文档本地保留不入远程"，G-04/G-07 均已显式记账），而 `adr_index_sync.py` 协议是"最后一条引用退役才删盘"，该文件仍被 ADR-188/adr_index/v3.0 引用 ⇒ **保留在盘**，治理职能由 `PLACEHOLDER_FREEZE` 承担 | A | 已按更正处置 |
| C-51 | `dep_edge_freeze.txt` 138/143/146 三口径 | 口径写明 = 只收 `[dependencies]`；权威计数以 `--emit` 派生为准；**顺带发现真实缺口**：新增 optional 直连边不触发本棘轮（由 G-56/G-57 侧管） | A | 已回写头注 |
| C-52 | 2 条向上 dev-dependency 边（`hcw-window→scc-cache`、`parliament→quest-engine`）机器不可见 | `workspace_deps_of()` 只读 `[dependencies]` ⇒ "注释即治理"。**已按默认做**（2026-10-10），且刻意**不改 Check B 本体**（它与 `.sh`/`.ps1` 逐字等价对账绑定，单侧改必触 parity）而是新开独立门 **G-93/G-94**：层号派生自 `layer_of()`，台账 `dev_dep_freeze.txt` 只减不增（新向上边未登记 GAP-D 红、已消边未删行 STALE 红、层表/基线读不出退 2）。**发现两条例子本来就有注释型豁免**（`hcw-window/Cargo.toml:45-46`、`parliament/Cargo.toml:58-59` 的"W8 清理记档"）——本门的价值不是新建口径，而是把那句只能靠人读的注释变成会被执行的判据 | A | **已落地（观察期）**；整链牙齿三项均验（见报告 §16） |
| C-53 | G-A「`check_perf_redlines.py slo` 出现真实判红」的前置是否有人监测 | 分两层：**有**每日 cron `slo.yml` A1 13 条 p95 红线 + `emit-slo-filter` 与 workflow 清单对拍；但 A1 `continue-on-error: true` ⇒ **判红不阻断**；且 `gate_manifest` 内 `redlines.py slo` 条目数 **0** ⇒ criterion 侧 6 条 `[[slo]]` 读数无持续监测 | A | 已登记，建门需 ADR |
| C-54 | G-07 提交耦合门的论域 | 只扫 `workflows + scripts`（自报 129 个），**不含 Rust `mod`/`include!` 面** ⇒ `lib.rs:59 mod bench_provider;` 指向未跟踪文件这一真实破坏面曾无门可见 | A | 本轮由入库消解；盲区待 W7 建门（可机检形式：`mod`/`include!` 目标文件必须 tracked） |
| C-55 | 项目规则要求“降级路径必须发布对应 NexusEvent（含 `MemoryNearBudget` / `ExpertOverload` / `ArchiveIoContention` 三类压力源）”，但现测三个名字在 `crates/event-bus/src/**` **0 命中**（连 `event_ownership_baseline.txt` 的 16 行“无生产构造点”清单都不在，即它们从来不是变体） | 定性为**不可执行红线**（规则点名了不存在的契约，与 P1 零载体同类但严重一级：它挂在 always-on 规则里，会被每个新实例当“已遵守”引用）。本轮**不新造变体**：新增事件变体要同时改 `types.rs` + `registry.rs` + `bus.rs` 旁路清单 + 四锁测 + 两张登记面（六锁同批）且属契约层变更，需 ADR；已写入 N-8 决策包 | A | 已定性；接法待 ADR |
| C-56 | G-67 `check_event_ownership.py` 的 `MOD_HEAD` 只认 `pub mod`，而 `event-bus/src/bus.rs:1934` 是 `#[cfg(test)] #[allow(…)] **pub(crate) mod** tests_helpers`——它一次性构造全部 16 个 mpsc 旁路 Critical 变体 | 定性为**门判据缺陷致假绿**（非新增债）：四个零生产 ctor 的治理事件（`R2FreezeViolation`/`R2FreezeRollbackFailed`/`R1ShadowRollbackFailed`/`FailureRegistered`）长期被判 owned。修法＝正则与 `FN_HEAD` 同形（可见性前缀）+ **双向夹具**（无 cfg(test) 的同形 `pub(crate) mod` 必须算生产，谁改回去必红）+ 新暴露债按门自述的第二条修法带证据补登。接生产者属行为变更且落在 R2/R1 面 ⇒ **需 ADR**（已归 N-6/N-8） | A | **已修已登**（selftest 13→17 全 PASS；台账 16→20 行） |
| C-57 | `composition_root_bench`（已在 `bench_inventory_freeze.txt` 在册）的 `bench_build_app_context` 无 runtime 语境调 `build()`，而 `build()` 经 `composition.rs:266` 调 `tokio::spawn` | 实测 `cargo bench` 与 `cargo test --all-targets` 双双 **rc=101** ⇒ “在册但从未产数”，与 §1 ENV-FAIL 四条 `.sh` 门同病（清单上看着不是红，实际从未裁决）。同文件另两个基准函数各自持 rt，唯独第一个没有——先例存在但未贯彻。已按先例补 `rt.enter()`（不改被测路径），三组均产数（仍为 [Directional]，**不入** SLO 三表） | A | **已修已验** |
| C-58 | 项目规则文档称“`mas-sched` 边 = composition-root wiring”（`dep_edge_freeze.txt:181`），但 `mas_sched` 在 `chimera-cli/src` **零引用**，CRATE-CONTRACT 自陈 `PRODUCERS 0/CONSUMERS 0`，`gated_wiring_baseline.txt:13` 已记 DECORATIVE | 台账与实况矛盾时以实况为准：该边是 **GATED-UNWIRED**（optional + feature 门控声明）而非已接线。**不删行**（本文件头注自述退役权威是 ADR-188，它不授权删除），只把注释改为与同批其余六行同形并写明三口径互证 | A | **已改注释**（零行为变更） |
| C-59 | Spec W6-3 断言“L2/L3/L6 三层事件几乎缺席，是 C1/C2 无法被证伪的根因”，并据此要求“各补≥一生产事件” | **前提被量体否证**：非测试作用域里有生产态事件构造点的 crate 数为 **L2=3 / L3=2 / L6=5**（全层：L1=2 L2=3 L3=2 L4=2 L5=2 L6=5 L7=4 L8=2 L9=4 L10=6；唯二为 0 的是 `session-store`（L3，根因即 P-4）与 `nexus-subagent`（L7，DECORATIVE 在册））。**但断言本身仍升为机器门**：G-67 新增 `[GAP-L]` 子判据（层表派生自 `layer_of()`，不重写；纯函数 + 3 条双向夹具；selftest 17→20）——否则“某层发布面被清空”仍只靠人眼。新发事件属行为变更需 ADR，本轮不接线 | A | **前提已否证；断言已入门** |
| C-60 | mlc-engine 三模块的装配态：Spec/文档称 sideagent·dual_experience_bank·value_backfill 为“②态”，而 `harness_dimensions` 的三个同名旗标默认 true 暗示能力已启用 | **现取三处均零生产构造**：`MemorySideagent` 仅在自身 `#[cfg(test)]`；`DualExperienceBank`（mlc 版）仅在本文件 tests + `tests/`；`DualSignalBackfill`/`ReflectionScorer`/`L1Trace` 同上。而 `MemoryManagementContract` 的 `on_demand_synthesis`/`dual_experience_bank`/`global_experience_board` 三旗标**全仓唯一 `.字段` 引用是定义件自身的默认值断言**（含解构形式一起查），零消费 ⇒ 默认 true 不代表能力已启用。另登记**同名双实现**：`repo-wiki::experience_bank::DualExperienceBank` 是另一个同名类型（L5，有 `distilled_insights` 表）⇒ 引用必须带路径前缀 | A | **已逐个写进模块头/契约头**；接开关属行为变更需 ADR |
| C-61 | G-84 文件级判据的自述与实况：门 note 写“源文件确引用 scan 即算已插桩”，并把“升入口级”登为“**仍待 ADR**”；而 Spec §需裁决事项 6 P-5 默认值是“**采纳入口级断言**” | 两个错登同时被现取推翻：① **口径错**——文件级 MARKER 使三个**批量写入口绕过单条守卫**（`mlc l3_procedural.rs::insert_batch`、`cmt warm.rs::insert_batch`、`cold.rs::insert_batch`；单条 `insert` 有 §(b.5) 扫描门而批量没有，且 `coordinator.rs:560/609` 生产降级腿真走该旁路）；② **权限错**——我把门备注当事实源，没回 Spec 取默认值（自证循环）。修：三入口补 scan（整批拒绝，与单条同语义；对照 `store_evidence_batch` 逐条跳过的不同前提）+ 判据升双层 [F]+[E] + `ENTRY_EXEMPT` 11 条带证据（UPSTREAM-GUARDED 机械核验委托关系）+ 新登 G-92 | A | **已修 + 已入门**；未解决面三项已写进门头注释（非文本面签名不可见 / SQL 语义靠人工复审 / 跨 crate 的 ScanPolicy 副本仍多源） |
| C-62 | **本轮推翻上一轮自己的否证**：报告 §5 曾把 Spec W6-4 的“scc-cache 零数据流⇒doctor 应明确 0 entries 为设计态”判为“不成立”，依据是 `check_scc_cache` 注释里写着“装配初期 0 访问/0 命中为预期行为，不降级” | 该否证把“已说明设计态”与“已叙述为初期”当成同一件事，而两者含义相反（“初期”预设流量会来）；盘上实况：生产链**无任何 `SccCache` 写入方**（组合根只装配句柄，探针只读 `stats()`），而探针还给 `✓ 缓存在役` + 恒 `Ok`——正是终局原则 3 要拦的形状。**已按 Spec 施工**：判定抽为纯函数 `scc_probe_verdict`（条目+访问均 0 ⇒ Warn 且文案必写“设计态/尚无写入方”；否则 ⇒ Ok），第 10 探针文案“在役”→“已装配”；新增两向测（含“零数据流不得出现✓”与“有访问仍给 Ok”防反向误伤），探针数仍 11 | A | **已修**；上一轮错判已在报告 §5/S-16 订正留痕 |
| C-63 | **同一文件内两句自相矛盾**：`session-store/src/lib.rs:14`（2026-10-06 注）称“replay 已接入 `nexus-app-server`（双写 + `replay_since` + 集成测试）”，而同文件文末订正段与本表 §4 行均说生产未启用 | 现取定论：“已接入”仅指**代码/测试层**——`server.rs:634` 起是 `#[cfg(test)]` 区，`with_session_store` 唯二调用点在 `:920`/`:977`，`replay_since` 的唯一调用在 `:806`（均测试区）；组合根 `composition.rs:251` 走 `with_backend` ⇒ `store=None`（`server.rs:283`）。已在 lib.rs 追加「装配态再订正（2026-10-10）」段（只加注不改历史），并落 `ADR-201` §6 追加事实注记。**同时否证 Spec 自身文本**：“收口径 ② 是一个提交闭台账债、零运行期风险”不成立（`nexus-app-server` 内 `session_store` 引用 17 处均在 `server.rs`，降 optional 需 cfg-gate 生产 API 面）；CONSUMERS/MATURITY 属机器派生（`crate_contract.py:8-10`）⇒ 语义变更需 ADR | A | **已订正**（纯注释/文档，零行为变更）；`crate_contract --check`、文档符号门、G-40、adr_index 同步均 rc=0 |

### 7.2 数字漂移登记表（文档值 → 现值）

| 项 | 文档值 | 现值（出处） |
|---|---|---|
| crate | 34/37/38/41/43/**48**/52/53 | **39**（`Cargo.toml`） |
| NexusEvent | 144/146/150/151 | **152**（`variant_count_test.rs:44`） |
| Critical | "17 走 mpsc 单流" | mpsc **16** / 总 **20**（`bus.rs:55/62`） |
| 门 | 60/82 | **98**（`gate_manifest.toml`；本轮由 91→92→94→96→98，新增 G-92/G-93/G-94/G-95/G-96/G-97/G-98） |
| ADR | 80+/86/001–182/199 | 最大 **201**、物理 104、索引 117 |
| 测试 | 13,500/15,000/18,000/20,000 | **11,758 passed / 81 ignored / 505 组**（2026-10-10 实测，`tmp/reg_totals.py` 聚合且三桶闭合；上一登记值 11,754/504 组为 P-5 批时点，11,722 为 v3.4 W5 时点）。**失败 target 集合当前为空**：基线点名的 `repo-wiki --doc` / `chimera-mas --doc` 单独复跑得 14 与 23 passed ⇒ 该限制**不恒常**；但历史上有跨 `cargo clean` 复现记录 ⇒ 不得写为“定性被否证”或“已修复”（详 §1 全量回归行与报告 §29/§31） |
| 面板 | 22/27 | **30 枚举 / 29 焦点环** |
| WGSL 内核 | 18 | **0** |
| SLO/criterion 门禁 | 12 项（九源 Ch13） | `perf_redlines.toml` **6 条 `[[slo]]`** + 11 条 `[[redline]]`；`perf_thresholds.toml` **47 条 `[[run_bench]]`** |
| 六文档行数 | 1147/1056/1395/1637/3562/1128（融合文档自陈） | **逐文件现取（2026-10-10 重测，具名不写一串无主数字）**：九源 V1.0 = **1421**、三源 v5.0.0 = **1312**、十六源 v3.0 = **1675**、CPU_GPU v6.0.0 = **1989**、v3.4.0 二十三篇 = **4100**、v4.0 六编 = **1392**（**2026-10-10 第二次刷新**：本轮给其中四份文档补“幽灵 crate 引用清单”，行数再漂 +4/+4/+4/+3/0/0 ⇒ 同一课第二次应验，见报告 §30 与台账 S-29）。**为何与上一登记值（1415/1298/1660/1976/4094/1384）全部上漂 +2~+11**：本批 W6-2/W6-8 自己给这六份文档加了“历史设计蓝图、非施工依据”头注与 ADR 临号作废回执 ⇒ 行数增长（不是别人改的）。旧写法只存六个无主数字，一改就漂且无法自校验 ⇒ 改为**文件名 = 行数**逐对列出 |
| 登记台账张数（Spec 终局原则 4） | “17 个 freeze/baseline 文件” | **24 个**（现取枚举；含本轮新增 `dev_dep_freeze.txt` 与历史批次新增的 compute_offload/outer_inner/mojibake 等）⇒ 拿“17”做治理盘点会漏计 7 张 |
| **event-bus 零装配子系统**（Spec W6-3 清单） | “8 个，约 2.2k 行” | **5 个，约 2.7k 行**（token_ledger 0.6k / segment_per 0.4k / membrane 1k / pattern_index 0.3k / `dynamic_registry` **404 行，本轮补登**）。**其余三个被误入清单**：`credit_flow` 是 `EventBus` 流控本体（字段 + 3 处构造 + 发送路径 `try_acquire/release` + shard worker 持有），`causal`/`shadow_diff` 被已接线的 `attribution` 引用（`attribution.rs:40/42`，`VectorClock` 在 `:57/:114/:315` 构造）⇒ 模块在生效链路内。类型粒度订正：`CausalConsistencyChecker`/`ShadowDiffRecorder` 本身仍无构造点，“零装配”必须按类型而非模块名判。证据与口径见 `event-bus/src/lib.rs` 装配态节（本轮现取：`tmp/w63_eb_modules.py`）；拿“8 个”估工时会高估约 1.5k 行可删面 |
| **CODE_WIKI 顶部“基线代码版本”行**（contributor 首读） | `:4` 写 `v2.28.0-omega 在途` | 现值 **`2.29.0-omega` / 39 crates / 152 变体**（行 9 三方一致性行 + G-04 canonical）。`:4` 实为**历史迭代链叙事**却被打上“基线”标签；另 `:1771` / `:1784` 两处“基线状态”带日期但未标“当时快照” ⇒ 三处均**只加限定语不改数字**（避免破坏 `[F2]` 计数比对）；复验 `[F2]` 仍 152 匹配、三方对账 rc=0（详报告 §28.6） |
| overwindow 块数标称 | `provider_select_2344`（注释“≈2344 块”）/ `_24k`（“≈24K 块”） | 600k token 语料实测 **3684 块**（`block_count()` 自报）；“24K”由同一失真除数（256 token/块）换算 ⇒ **【待验证】**，名称与注释均为估算而非实测（已改注释口径，改名待裁） |
| `ClvSimilarity` 拐点 n\* | 287（ADR-196 §10） | **≤ 256**（5 轮×正/反序双组，六点全 5/5 同号） |
| L0 “含算法”模块的函数计数 | 方案原文：capability_token **70** / vector **62** / harness_spec **52** 个 `pub fn` | `pub fn` 实为 **20 / 7 / 4**；70/62/52 是含私有+含 trait 无体声明+含 `cfg(test)` 的 `fn` 总数。**判“是否含逻辑”的口径 = 非测试有体函数：27 / 7 / 13**（`tmp/l0_fn_bodies.py`）⇒ “零逻辑”仅对 `vector.rs` 成立，对 `harness_spec`（`validate` 143 行）/`capability_token`（EWMA）失真 |
| fnlen 台账 | 9 行（含 `lookup` 517/511、`all_variant_samples` 945、`execute_with_ctx` 300 等） | **4 行**（7 项真分解后五项逐出测量集；`new_internal` cap 472→470）（`scripts/fnlen_baseline.txt`，均“只减”） |
| 事件所有权账本 | 16 行（“136 owned / 16 unowned”） | **20 行**（132 owned / 20 unowned）——扫描器可见性盲点修复后**新暴露的既存债**，非新增债 |
| “L2/L3/L6 事件几乎缺席” | Spec 断言（据此要求接线） | 实测发布面 L2=3 / L3=2 / L6=5 个 crate（`tmp/probe_layer_events.py`）⇒ 断言为**假**；但“每层必须有发布面”已升为 G-67 子判据 `[GAP-L]` |
| **v3.4.0 §4.1「括号内新 crate」项数**（原则 2 自陈） | “约 40 项” | **70 项**（§4.1 括号组口径）/ **101 项**（并入 §4.2 组件清单后的并集）。逐项三态已入 §4 附录 4A，并由 **G-97/G-98** 三方对账（提案名册 ↔ 代码事实 ↔ 本表行集）；结论修正：选言“全部作废或已以模块落地”成立，但**作废是少数**（§4.1 口径 6/70），且 §4.1 括号内成为新 crate 者 = **0** |
| fuzz target | 8（CLAUDE.md） | `fuzz/Cargo.toml` 8 个 `[[bin]]`（本轮未重算，沿用登记；引用前须现取） |

## 8 冻结项复评结论（按用户授权"重新取证给判"）

| 方向 | 判定 | 依据（A 级） | 复评触发条件（可机检形式） |
|---|---|---|---|
| GPU / 异构 | **维持冻结**，但论据须重写 | G-A 不满足（`tmp/slo_rerun.log` 六条 SLO failed=0）；G-B 不满足（全仓 `*.wgsl`=0、`gpu_*.rs`=0、`kernels/`=0，v6 §1.2 的 12 个 A 级点位输入规模在码内不可达）；D2 四面（确定性归约/Critical 发布/事件相等性/R2）任一 GPU 化即违**正确性**契约；D5 三前置实验 E1/E2/E3 **全部未做**；本机有 RTX 4060 **与三门判据无耦合** | G-A：`check_perf_redlines.py slo` 出现真实判红**且**该点位 CPU 算法/并行优化已留档；G-B：生产常驻规模 ≥ 现阈值×10 **且** roofline 判定通过 **且** 无可行业级改进；G-C：改造点不属 D2 四面 **且** 提供 CPU 参照逐位对照判据。解冻需 supersession 逐条推翻 ADR-107/123/124/126 + 新 ADR |
| v6 平行调度器 | **维持冻结** | ADR-196 D1 否决性质是**架构同一性**（撞名同功能位 + ADR-127 唯一入口），非"性能未验证"；解冻无可衡量收益面且违 ADR-120/127 | 不适用。真实工作项转为"给已登记 CPU 卸载面找流量"（5 DEAD + 2 CRATE_API_LIVE） |
| RL / Python / R2 | **维持冻结**，且必须先修"守卫真实性" | 影子期**从未起算**（入参无时钟）+ `Stage3Prerequisites` 六项 Default=false + `ShadowModeOrchestrator` 生产零构造 + CI 从不启 `r2_unfreeze`（`.github` 0 命中）+ 文档所称五关键词 CI 扫描**不存在**（实际守卫是 e2e，且 auto-dpo 目录已删致 `if exists()` 短路 = vacuous green） | 需 ADR-042 五条件 + ADR-053 **rev4 批次制**判定（≥14 独立批次 + min(Wilson,bootstrap) 单侧 95% 下界 > 0.5 + 四维门全过 + 负面异常门未触发）+ ADR-054 签署 + ADR-055 解冻文书；Python 侧另受 Rust-First（2026-08-15）独立约束 |
| 性能阈值/利用率 | **维持不预设 65% 硬线**；`ClvSimilarity` 1000 **不改（端到端 A/B 已测完：段测不可外推，且本机精度下限不够标定最优阈）** | ADR-157 原文"combined 如实报告、不预设 65% 硬线，三条件联合为主判据"，否决"按 65% 塑形达标"违诚实数据红线（ADR-155/157 两度否决）；**M3 已实跑重定拐点**（新增 384 点位 + 正/反序双对照组 ×5 轮）：256/384/512 六点均 **5/5 同号且换序不翻转**，offload 分别快 1.25–1.38× / 1.57–1.71× / 1.95–2.15× ⇒ **n\* ≤ 256**（旧值 287 作废；旧 bench 集跨过 287 却无 (256,512) 点，结构上不可证伪）；生产带 `[513,3906]` 内 offload 稳定约 2×；**M6 端到端 A/B 已完（A 级）**：实测 681/1100/3684 块下 inline 与走池配对得 **1.375× / 1.65× / 2.57×**，同路质控重测漂幅 +0.3%→+16.4%（⇒ 本机读数精度下限），按 n 内插后端到端收益比段测低 **≈49%**（判据 20% 不通过）；已越阈规模上 256 与 1000 同路、差 ≤0.9% ⇒ **收益只来自“越阈/未越阈”这一次跳变**（旧版“阈值影响面超出 dense 段”已作废，成因是本批拿估算块数冒充实测块数，见 `ADR-196 §12`） | 不改阈的现理由（三条，均取代旧版“收益成因尚未归因”）：① 本机单轮不满足 ADR-198 静默窗口的多点 ×≥5 轮**交替随机序**判据；② 本机质控漂幅最高 16.4%，精度下限只够支撑“越阈/未越阈”二分，不够标定最优值；③ 结论：用段测值标定阈值会**系统性高估收益约一倍**。改阈走路 B（`update_threshold` + `measured_at≠0`），**不走路 A**（Promote 一次只折半 1000→500，且 ADR-198 勘误后 CI 默认 `off` ⇒ apply 路径生产中不发生）。“1000 偏保守”已由 C 级升 **B 级**；转 A 级需 clean-runner 重标定 + ADR |

## 9 波次状态（W6 序列）

| 波次 | 内容 | 状态 |
|---|---|---|
| W6-0 | G-07 阻断收口（两文件入索引）+ `build.rs` rerun 粒度 + `BenchStats` 破坏面核验 | ✅ 完成并验证 |
| W6-1 | 全量 light 门基线 + 门债：G-27/G-35（重构消形状，键 9→8）、G-44（10 crash 护栏 + 2 读码证明）、G-83（修判据 + 15 夹具）、G-48（归因实证，本轮不动）、**ENV-FAIL 三条已根治**（运行器从 `usable_bash()` 反推 coreutils 目录；G-05/G-47/G-52 转为绿色真判定，终局 0 ENV-FAIL） | ✅ 完成并验证。**G-48 已于 W6-8 收口批真分解清偿**（不再归因不动） |
| W6-1b | M3 拐点实跑（新增 384 点位 + 反序对照组 + 5 轮双组同号检验，n\* ≤ 256）、M5 G-86 盲区夹具（8/8）、M1 六条 SLO（全 PASS，最小余量 5.8×）、M2 双序复测（退化未复现）、M4 `--features r2_unfreeze` 轨（197 passed / 0 failed）、**M6 端到端 A/B 已建并已测：681/1100/3684 块 → 1.375×/1.65×/2.57×，段测不可外推（差 ≈49%）** | ✅ 完成。**旧版“阈值影响面超出 dense 段”已作废**：那是本批用估算块数冒充实测块数造成的假矛盾（见 `ADR-196 §12` 与报告 §9 S-10），逐阶段归因工作项**取消**；**M1–M6 汇总交付物已补落** `docs/reports/v3.5-baseline-measurement-2026-10-10.md`（第 20 轮完成度审计查出：Spec W6-1 点名的这个具名文件此前**从未创建**，读数散在总账 §14 / 本报告 §3・§22 / ADR-195 §8 三处） |
| W6-2 | ADR-195 §8（含 §8.7 M1 实测）、ADR-196 §11、ADR-192 §7、CODE_WIKI（util + R2）、adr_index（R2）、`nine-layer-defense-map`（规则 4 + 口径裁决节）、`fusion-concept-map` §4（临号作废回执）、registry.rs 注释、`dep_edge_freeze` 头注、两份规则文件面板口径、六文档头部“历史设计蓝图”标注 | ✅ 完成（C-41/C-42/C-49 均已落） |
| W6-3 | L0 季度删除制、L1 撤主张、L2/L3/L6 可观测事件、L3 session-store 生效语义 | 🔄 L0 **“含受控逻辑”显式登记已做**（2026-10-09）：`ADR-033` 追加节 + `lib.rs` 例外表新增例 5（harness_spec）+ 三个模块头逐个定性（量体：有体函数 27/7/13，“零逻辑”仅 `vector.rs` 成立）+ CODE_WIKI/agents.md/两份规则文件总括口径改为“纯类型 + 受控纯逻辑”；顺带闭合 WI-G1 审计 §6.1 建议未落的“唯一例外 vs 第二个例外”自相矛盾。L1 **已撤主张**（`event-bus/src/lib.rs` 装配态口径段）；L1 **观测盲区已接刃**（**I8c** 双静态子断言）；L2/L3/L6 可观测性：**前提已否证（L2=3/L3=2/L6=5 已有发布面）且断言已升为 G-67 子判据 `[GAP-L]`**（selftest 17→20，层表派生自 `layer_of()`）；L3 **已降档为可证伪主张**；季度删除制与 `DORMANT` 词表需 ADR；L2/L3/L6 新增可观测事件需六锁同批 + ADR |
| W6-4 | gqep 四模块接线、scc/cmt/mlc 主张对账、mas-sched 虚边、nexus-hook/subagent 复核。**已删除两项主张**：① `mcp-mesh::client_v2` 不是缺陷（`lib.rs:48` 就是 `pub mod`，公开可达；未进 prelude 只是风格约定，导出≠接线）；② **`StopRulingIssued` “有事件无发出点”也不成立**（现取两处生产发出点：`quest-engine/src/control.rs:76` + `chimera-cli/src/quest_loop.rs:286`；旧判是把“在 `parliament/src` 搜不到”当成了“全仓不存在”） | ✅ 撤主张批已完（2026-10-09，零新增生产构造点）：parliament mappo/sharp 五处文档降为“离线研究面”、cmt-tiering 分层决策闭环订正为未接线、**mlc-engine 三模块（sideagent / dual_experience_bank / value_backfill）逐个加②态装配段**（含 `repo-wiki` 同名双实现警示与 `harness_dimensions` 三旗标零消费声明，C-60）、mas-sched 边注释改为 GATED-UNWIRED（C-58）。⏸ gqep 接线仍转 **N-7 决策包**（不自创接入点） |
| W6-5 | formal 名单对账（C-43）、INV-9 双源对账测、统一 `sanitize()`、权限三源统一、R2 假绿防护 | 🔄 **已完三项（本轮把 INV-9 从单层补成两层）**：R2 假绿形状修正（`r2_freeze_violation_e2e` 22 passed）+ **INV-9 双源对账两层且均零新增依赖边**：固定样本集（`invariants_test.rs:245-293`，该 target 14 passed）+ 本轮补的随机图层（`inv9_parity_proptest.rs`，4 passed：结论全等 2000 例 / 报错路径真实性 / 顺序无关 / 边界矩阵），**变异牙齿已验**（让 L5 跳过自环 ⇒ 报“双源漂移 L9 ok=false vs L5 ok=true”，还原后字节全等，详报告 §26）+ **权限三源互转事实核验（A 级）**：全库 `impl From/TryFrom/Into` 在三类型间 **0 命中** ⇒ C-45 由【待验证】升为事实（详报告 §10.5）。formal 名单需 ADR（R2 证据）；`sanitize()` 入口与权限单一权威需契约级 ADR |
| W6-6 | 冻结项裁决包定稿（P-1…P-5 更新 + N-1…N-8 新增 + **F-1…F-3 冻结项复评**，每包固定六段） | ✅ 完成：`docs/reports/v3.5-w66-decision-packages-2026-10-09.md`（2026-10-09 补齐 F-1 GPU/异构、F-2 v6 平行调度器、F-3 RL/Python/R2 三个六段式包；阈值方向即 P-2，含 M3/M6 实测）。三包结论均为**维持冻结**，但论据已按本批改写（G-A 由 M1 实跑支撑；F-3 新增“四个 Critical 事件零生产发出点”与 M4 可编译证据） |
| W6-7 | 本总账落盘 + 发布门前置全跑 | 🔄 本文即落盘；全量 light 门首次 84/84 OK（`tmp/v35-gates-w6close.txt`）；覆盖率地板 85 归 CI nightly。**口径升级（2026-10-10，A 级现因）**：“本地不跑”不是选择而是**不能跑**——本机 rustc（stable-x86_64-pc-windows-gnu 1.97.1）缺 profiler runtime，`cargo llvm-cov -p ssra-fusion` 在插桩构建阶段即 `E0463: can't find crate for profiler_builtins`（与 2026-09-04 phaseR-wave1 记的同一因，至今未变；已清理 0 MB 残留目录）。**由此抖出一条真开的债**：`coverage_per_crate_floor.toml` 自写“首次真实测量后须把 defaults.floor 与各 [[crate]].floor 抬到实测−3~5pp，禁止长期停在兜底 45”，而 2026-09-05 已真测过——但 `coverage-ratchet-calibration-2026-09-05.md:6` 只公布了**聚合 92.97% + “逐 crate floor 全过”**，未留逐 crate 数值 ⇒ 无据可抬，不得编数据填表；已在该文件头注记此债与出数地点（详报告 §32） |
| W6-8 收口批 | G-48 七项真分解（逐行等价已证）+ **两处门判据缺陷修复**（C-56 事件所有权扫描器可见性盲点、C-57 `composition_root_bench` 从未产数）+ W6-4 撤主张 + W6-5 权限三源核验 | ✅ 完成并验证（报告 §10）；唯一遗留：`new_internal` 仍登记 470（“减”方向已走，进 ≤200 需再拆一轮） |

## 10 验收指标（每波硬门）

`cargo check --workspace --all-targets` → `clippy --workspace --all-targets --jobs 2 -- -D warnings` → `fmt --all -- --check`（逐包，`--all` 已知 os error 206）→ 全量回归（对照 11722/81，**新增测试数须与增量精确吻合**，失败 target 集合须与基线**完全同集**）→ 全量 light 门 84 条（不是子集）→ `cargo audit`（6 条 ignore，口径同 CI）→ `cargo check --manifest-path fuzz/Cargo.toml` → 覆盖率 floor 85 → 版本三源同步（`workspace.package.version` + CHANGELOG 章节 + tag）。

方法论继承：等价性判据必须**逐位对照 + 牙齿探针**（本轮 G-27/G-83 已两次实证"门与文档打架"，凡新增/删除豁免都反向验一次"改回去必红"）。

## 11 风险登记册（终局重构特有）

| # | 风险 | 缓解 | 回滚 |
|---|---|---|---|
| R-1 | "顺手启用" `r2_unfreeze` ⇒ 未授权自演进路径（`composition.rs` 内 placeholder `step()` 无条件 `Ok(())`，已记账 `PLACEHOLDER-EN=1`） | CI 不启该 feature；批次内只做删除或实装 | feature off 即恢复 |
| R-2 | 冻结守护 vacuous green（`if src_dir.exists()` 短路） | 显式断言"目录不存在 + `DELETED-BY-ADR` 记录"，**严禁**复活目录喂门 | 还原测试 |
| R-3 | 按"19 条"扩 `UNLEARNABLE_SECURITY_RULES` 破 `len()==6` 断言 | 6 条为唯一权威；扩条独立 ADR | 断言即红 |
| R-4 | INV-9 双源（L9 权威 + L5 镜像）只改一处 ⇒ 门绿内核红 | 同批改 + 对账 proptest（**已落地两层：固定样本集 + 随机图/路径真实性/顺序无关；变异牙齿已验**，报告 §26）；终局下沉 L0（需 ADR） | 保留双份加对账 |
| R-5 | 虚假还债（`optional`/feature 当已接线） | 两账分开（`reachable ≠ wired`）；**已落成 `.github/PULL_REQUEST_TEMPLATE.md` §2（2026-10-10，本行原先只写“写进 PR 模板”而模板文件并不存在）**；还债须消台账行 | 棘轮回增即红 |
| R-6 | 记忆防线接线引入新生产边违铁律 | 接前登记 `dep_edge_freeze`；阈值变更属行为变更 | 撤 feature 回退 gated |
| R-7 | 平台降级被误当"沙箱已具备"（Windows-GNU 仅 `ProcessFence`，`gvisor::is_available()` 恒 false） | 明确当前唯一实生效边界；禁据此放宽 `execpolicy` | 规则式阈值回退 |
| R-8 | 本机内存压力污染 bench（W5-C 见 hot_lru/hnsw 2.8×/2.9× 选择性退化，同批 session_store 仅 +15%、l2_recall −35%） | 三档对照（空闲/高压/CI clean-runner），比值 >1.5 即判污染；权威读数归 CI。**但 M2 已推翻本风险的因变量归因**：同档压力（2.5–2.8 GB vs 当时 2.33 GB）下 hot_lru/hnsw 均恢复到第一轮量级（26–34 µs / 293 µs）⇒ “内存压力致因”降为未证实；同一基准三轮分层收益比 6.84×/9.8×/13.07× ⇒ **单轮同机 speedup 一律 `[Directional]`**（详 `ADR-192` §7 与 `v3.4-w5-closeout` §4.2） | 结论降级【待验证】 |
| R-9 | 批次扩面过宽不可验证（六文档残余 40+ 项） | 每波 ≤8 工作项、单提交独立 revert、触发文件不外溢 | 逐 commit 回退 |
| R-10 | **行号键台账反复 re-key**（G-27 已 3 次；`sqlite_blocking_allowlist` 亦漂移） | 优先消形状/改 path::fn 键（G-44 基线已是无行号键先例）；禁再单纯改行号 | 单条回退 |

## 12 开放问题与需裁决事项（默认值已给）

1. W6-0 后续：**是否提交**这批改动（含继承的 bench 一期 WIP）——默认**不提交**，等指令。
2. P-1 N-4 八点位 API 级统一（默认：不动点位，仅保留姿态登记 `ADJUDICATED_IN_PLACE_FALLBACK_2026_10_08`）。
3. P-2 阈值重标定（默认：先 M3 复测，不预改）。**已执行**：M3 定拐点 ≤256，M6 端到端 A/B 已完成⇒ 段测不
   可外推（差 ≈49%），阈值维持 1000；“1000 偏保守”升为 B 级，真改需 clean-runner 重标定 + ADR（走路 B）。
4. P-3 NMC 资产（owner 域，默认：保持诚实 SKIP；全仓 `.onnx` = 0）。
5. P-4 session-store 生效语义。**本轮按证据拆为两层（不是“选更小的做完”）**：
   （a）**不需 ADR、已做（2026-10-10）**＝表述订正：`session-store/src/lib.rs` 头部新增「装配态再订正」段。
   缘于自查发现同一文件内两句共存而不自洽：`:14`（2026-10-06 注）写“replay 已接入…双写 + 集成测试”，
   而文末订正段与 §4 行均说生产未启用；现取证据：`server.rs:634` 是 `#[cfg(test)]` 起点，
   `with_session_store` 调用点在 `:920`/`:977`（均测试区），组合根 `composition.rs:251` 走
   `with_backend` ⇒ `store=None`（`server.rs:283`）。（b）**需 ADR**＝降 optional + CONSUMERS/MATURITY
   语义变更（`crate_contract.py:8-10`：CONSUMERS 由非 optional 边派生、MATURITY 由自陈标签派生，
   手写改值被 G-59 拒）；**并需更正本 Spec 自身文本**：“② 是一个提交闭台账债、零运行期风险”**实测不成立**
   （`nexus-app-server` 内 `session_store` 引用 **17 处**均在 `server.rs`，降 optional 需 cfg-gate 生产 API 面
   + 删 `dep_edge_freeze.txt` 一行）。详见 `ADR-201` §6 追加事实注记与 C-63。
6. P-5 G-84 文件级→入口级（默认：采纳入口级断言，纯静态低成本）。**已执行**（2026-10-10）：
   三处批量旁路已补守卫，判据升为 [F]+[E] 双层，新登 G-92（门清单 91→92）；详见报告 §14 与 C-61。
7. C-52 Check B 扩 dev-deps（默认：做，独立 id 观察一周期）。**已按默认执行**（2026-10-10）：
   不改 Check B 本体而新增独立门 G-93/G-94 + 只减台账 `dev_dep_freeze.txt`；观察期待出口见该文件头注。
8. C-48 nexus-hook（默认：撤 ADR-188 D3 主张）。**已按默认执行**（2026-10-10）：主张已以
   ADR-188 §5 追加更正撤回，零代码变更；接线仍需新 ADR（前置清单已写入该节）。
9. C-41/C-42/C-49 治理回写授权（纯文档，零代码）。
10. **新增**：是否把 C-53（A1 切阻塞 + `redlines.py slo` 建门）与 C-54（Rust `mod` 面耦合门）立为 W7 两个新门 —— 二者都需新 ADR 取号（自 ADR-202 起）。

## 13 不做清单（本轮及后续批次红线）

GPU/wgpu/着色器｜第三运行时（crossbeam 独立池）｜手写 SIMD（`#[target_feature]`）｜任何 `unsafe`/`allow(unsafe_code)`｜新增 crate｜新跨层直连｜删除 NexusEvent 变体｜扩 `UNLEARNABLE_SECURITY_RULES`｜R2/RL 解冻｜Python RL 服务实体｜把六文档数字当事实（其 crate 数/事件数/测试数/层图已系统性失真）。

## 14 全局追溯矩阵（Spec 条目 → 落点 → 验证方式 → 读数）

> 用途：每一行都必须能被第三人独立重跑。`状态` 列只写三种：✅ 已实现并验、⚖ 需 ADR/授权（已成交付包）、❌ 已被现取否证（不施工）。
> 证据级沿用 §0（A=本轮实跑可复核；B=登记面文本）。

| Spec 条目 | 落点（文件 / 符号） | 验证方式 | 读数 | 状态 |
|---|---|---|---|---|
| W6-0 两文件入库使 `mod bench_provider` 自洽 | `chimera-cli/build.rs`、`src/bench_provider.rs`、`lib.rs:59` | `git status` + G-07 + `cargo test -p chimera-cli bench_provider` | G-07 rc=0（0 条未满足耦合）；2 passed（A） | ✅ |
| W6-0 `rerun-if-changed` 整目录→逐文件 | `build.rs`（根 Cargo.toml + 各成员 manifest + `perf_thresholds.toml`，`sort()` 定序） | 改 manifest 后二次构建是否重跑（人工）+ `build.rs` 注释 | 逻辑已落；无机器门可证抖动（B） | ✅ |
| W6-0 `BenchStats` 破坏面 + 无源字段诚实 N/A | `panels/bench.rs`、`bench_provider.rs`（`sourceless_runtime_fields_are_none_not_zero`） | `bench_panel_test` 8 例 + `--lib` 969 | 0 failed（A） | ✅ |
| W6-0 同批登记面（i18n parity / surface / bench inventory / doc count / G-58・59 / CHANGELOG） | 五张 `scripts/*_freeze.txt` 等 | G-40・G-04・G-53・G-58・G-59・G-81/82 | 全 rc=0（A） | ✅ |
| W6-1 M1 六条 SLO | `check_perf_redlines.py slo` | 实跑 | 全 PASS，**最小余量 5.8×**（A） | ✅ |
| W6-1 M2 双档对照 | 同上 hot_lru / hnsw_tiered | 同进程双序复测 | W5-C 的 2.8×/2.9× **未复现** ⇒ 内存压力致因降为未证实（A） | ✅ |
| W6-1 M3 拐点重定 | `bridge_bench.rs` +384 点位 + `_rev` 对照组；`tmp/m3_knee_rounds.py` | 5 轮×双组同号检验 | n\* ≤ 256，六点 5/5 同号且换序不翻转（A） | ✅ |
| W6-1 M4 r2_unfreeze 轨 | `cargo test -p chimera-cli --features r2_unfreeze --lib` | 实跑 | 197 passed / 0 failed（A；可编译 ≠ 已验证，CI 仍不启） | ✅ |
| W6-1 M5 G-86 tract 盲区 | `check_accel_exclusion.scan_lock()` 直调 | 负向夹具 8/8 | wgpu/ort/candle 拦住；tract-onnx/linalg **不拦**（A） | ✅ |
| W6-1 M6 端到端 A/B | `chimera-cli/benches/overwindow_bridge.rs::bench_e2e_dense_ab`；`OverWindowBridge::block_count()` | 三语料×四臂（含同路质控臂）+ 自报 `route()` | 681/1100/3684 → 1.375×/1.65×/2.57×；外推差 ≈49% ≫ 20%；质控漂幅 0.3–16.4%（A） | ✅（旧“阈值影响面超出 dense 段”＝❌本批自错，已作废） |
| W6-1 门债 G-27/G-35 | `pipeline.rs::take_locked` + 两张孪生表同删键 | 牙齿探针 `tooth_probe_tmp.rs`（植入必红、删除回绿） | 键集 9→8；两门 rc=0（A） | ✅ |
| W6-1 门债 G-44 | 10 个脚本 `gate_rc` 护栏 + `entry_contract_baseline.txt` 2 行 | 单跑各脚本 rc + `check_entry_contract.py` | rc=0（suppressed=8/stale=0/no_reason=0/dup=0）（A） | ✅ |
| W6-1 门债 G-48（fnlen） | 7 项真分解（i18n×2 / variant 清单 / new_internal / execute_with_ctx / spawn_experience_loop / osa e2e） | **逐行快照对拍**（en 679、zh 669、variant 984）+ `test_fnlen_smoke.sh` | rc=0；台账 9→4 行且均为“减”（A） | ✅ |
| W6-1 门债 G-83 | `check_certify_whitelist.py::is_production_src`/`scan_lines` | `--selftest` 15 夹具（含反向） | GREEN 15/15（A） | ✅ |
| W6-1 ENV-FAIL×3 | `run_gate_manifest.py::augment_path()`；`check_slash_vocab.sh` 自检 | 干净 shell 实跑全量 light 门 | 终局 **0 ENV-FAIL**（A） | ✅ |
| W6-1 “全门绿”口径 | 本表 + §1 | 全量 light 清单 | **当时（2026-10-08 收口）** 84 OK / 0 MISMATCH / 0 ENV-FAIL / 7 heavy SKIP（A）；**终态已进版：2026-10-10 为 91 OK / 0 / 0（门清单 98）**，现值只看 §1 两行 | ✅ |
| W6-2 治理回写（ADR-195 §8 / ADR-196 §11・§12 / ADR-192 §7 / CODE_WIKI / adr_index / defense-map / concept-map / registry / dep_edge / 规则面板 / 六文档头注） | 11 处文档与注释 | G-04・G-40・`adr_index_sync`・`crate_contract --check`、CJK 扫描 | 全 rc=0；新符号均可解析（A） | ✅ |
| W6-2 `check_r2_shadow_redlines.ps1` 门内身份 | 该 `.ps1` 头注 + fail-closed | 牙齿双向（目标全在 rc=0；一个不存在 rc=2） | 已验（A） | ✅ |
| W6-3 L0 含受控逻辑登记 | `ADR-033` 追加节、`lib.rs` 例外 5、`harness_spec`/`capability_token`/`vector` 模块头、四处口径 | 量体脚本 `tmp/l0_fn_bodies.py` + G-40 | 有体函数 27/7/13；“零逻辑”仅 vector 成立（A） | ✅ |
| W6-3 L1 事件面三分治理 + 同名概念 | `event-bus/src/lib.rs` 装配态段；`PerBuffer`/`TokenLedger`/`DualExperienceBank` 登记 | G-67・G-53・四锁测 | rc=0；152 变体全保留（A） | ✅ |
| **W6-3 补漏：零装配子系统清单本身被现取推翻** | `event-bus/src/lib.rs` 装配态表新增第 5 行（`dynamic_registry` 404 行、零引用）+ 新增“看似同族、实则已接线”一节 | `tmp/w63_eb_modules.py` 逐模块反查 crate 内外引用；`credit_flow` 逐行定位到 bus.rs:329/567/596/629/443/464/900 与 shard.rs:248/254/429 | Spec 写“8 个/约 2.2k 行”，实为 **5 个/约 2.7k 行**；`credit_flow`（总线流控本体）与 `causal`/`shadow_diff`（已接线归因链内）**不得归入零装配**；`-p event-bus --lib` 299 passed / 0 failed（纯注释）（A） | ✅ |
| W6-3 L1 观测盲区（I8c） | `check_perf_redlines.py::I8c`；`bus.rs:42-52` 注释 | 首跑即翻红（`backpressure_test.rs:49` 旧值 4⇒16）+ 4 处假阳性反向收紧 | invariants / `--static-only` 双 rc=0（A） | ✅ |
| W6-3 L2/L3/L6 可观测性 | `check_event_ownership.py::[GAP-L]` + `layer_event_presence()`（层表**派生**自 `layer_of()`） | `--selftest` 新增 3 条双向夹具 | selftest 17→20 全 PASS；真树打印 L1=2…L10=6（A）；**Spec 前提“三层几乎缺席”＝❌ 已否证** | ✅ |
| **落点偏离说明**（追溯诚实项） | Spec 建议“作为 Check E 扩展项”，实际落在 G-67 | 现取：Check E = `check_dependency_rules.sh` 的 **chimera-tui 出站边界**检查（头注自陈 “Mirror of .ps1 Check E”） | 理由：① 语义不属于“依赖方向”门；② 落 Check E 需 `.sh`+`.ps1` 同批双改并触 parity；G-67 已在扫同一事实面且层表可派生 ⇒ 未选“更容易”，面同一事实只能有一处定论（A） | ✅ |
| W6-3 L3 session-store 生效语义 | `session-store/src/lib.rs` 装配态订正；P-4 包 | `crate_contract --check`（不擅改派生字段） | rc=0（A）；optional 降级＝⚖ 需授权（触 G-56/57 1:1） | ✅/⚖ |
| W6-4 撤主张（parliament/cmt/mlc三/mas-sched） | 五处文档 + 四个源码注释（含 C-58/C-60） | 每项现取证据（grep 计数）写入报告 §10.4 | 零新增生产构造点（A） | ✅ |
| **W6-4 scc-cache 诚实降级**（上一轮被错判为“不成立”，本轮推翻并施工） | `doctor.rs::scc_probe_verdict`（新纯函数）+ `check_scc_cache` + 第 10 探针文案 + `StorageHealth::message` | 两向测 `scc_probe_verdict_matrix`（零数据流⇒Warn且不得出现 ✓；有条目/有访问⇒Ok 并报命中率）+ 原验收测改断言 Warn；`--test cli doctor` 3 passed 证明未破对外契约 | chimera-cli lib doctor 14 passed / 0 failed（+1 = 新增矩阵测）；fmt/clippy 本 crate rc=0；探针数仍 11（A） | ✅ |
| W6-4 gqep 四模块接线 | N-7 决策包（三选项 + 不自创接入点） | 台账已在册核验 | ⚖ 执行面产品语义，需 ADR（ADR-208 候选） | ⚖ |
| **C-48 nexus-hook 撤主张**（Spec §需裁决事项 8 默认值） | ADR-188 新增 §5 追加更正 + `crate_reachability_freeze.txt` 订正注 + CODE_WIKI nexus-hook 装配态行 | 三条独立路径互证：全库 `crates/*/src`（除自身）对 `nexus_hook` 逐 crate 递归检索 0 命中 + `gated_wiring_baseline.txt` DECORATIVE 在册 + `dep_edge_freeze.txt` 仅 optional 声明边 | 0 外部引用；G-53/G-57/G-33/G-40/指针完整性/adr_index/crate_contract 均 rc=0（注释级改动未触计数；A） | ✅ |
| W6-5 R2 假绿防护 / INV-9 / 权限三源 / seccore 口径 | `r2_freeze_violation_e2e.rs`、`invariants_test.rs`、报告 §10.5/§10.7 | 实跑：`r2_freeze_violation_e2e` + `invariants_test` + 全库 `impl From/TryFrom/Into` 检索 + 三处口径交叉校 | 22 passed；14 passed（+2）；互转 **0 命中**；平台边界三处已闭环（A） | ✅ |
| W6-5 **P-5：G-84 升入口级**（Spec 默认值即“采纳”，不是需 ADR 项） | `check_write_sensitive_instrumentation.py`（双层 [F]+[E] + `ENTRY_EXEMPT`）、`mlc l3_procedural.rs`/`cmt warm.rs`/`cold.rs` 的 `insert_batch`、`cmt lib.rs::hard_payload_scan_policy()`、新登 G-92 + `ci.yml` step | 三重独立路径：`tmp/p5_gate_reconcile.py` 把门自报计数与探针逐文件对账；`tmp/p5_teeth.py` 把三处守卫短路看测是否翻红；G-44 入口契约门反过来抓我新写脚本 | 入口 26 / 自带 scan 15 / 带证据豁免 11 / 缺口 0（rc=0）；3 mutant 全被抓（`1 failed` x3）且还原字节一致；门清单 91→92（A） | ✅ |
| **C-43 形式化名册对账入门**（上游 Spec 裁为“纠偏非扩面”，但换名册触及已签 R2 证据集） | `scripts/check_formal_property_ledger.py`（新）+ `gate_manifest.toml` G-95/G-96 + **ADR-052 追加事实注记** | 名册源与 checker 均机械解析（名册 = `EmptyFormalProvider` 体内 id；checker = 逐个文件存活性 + 含 `pub struct`）；`--selftest` 10 项夹具含“真树形状断言”（名册 7 且 checker 7）；G-44 入域后复跑确认新门自身合规 | 名册 7 / checker 7 / 登记错配 2 且均仍成立（rc=0）；夹具 10/10；门清单 94→96（A） | ✅（修名册本身＝⚖ 需新 ADR） |
| **C-52 dev-deps 向上边可机检化**（Spec §需裁决事项 7 默认值：做，独立 id 观察一周期） | `scripts/check_dev_dep_direction.py`（新）、`scripts/dev_dep_freeze.txt`（新只减台账）、`gate_manifest.toml` G-93/G-94 | 单元：`--selftest` 13 项双向夹具（含 target 限定段不漏检、成员集空退 2、层表派生失败退 2）；整链：`tmp/c52_teeth.py` 对真 Cargo.toml/真基线做 3 个变异并字节还原；另跑 provenance/parity/runner selftest 确认新门接入无副作用 | 39 成员/内部 dev 边 27/向上 2（均在台账且引到各自注释型豁免源头行号）/未登记 0；G-93・G-94・provenance・parity・runner selftest 均 rc=0（A）。**W6-4 尾项“gea-activator 把 gqep/scc 放 dev-deps”已用 G-93 自解析器取实据**（`tmp/gea_dev_evidence.py`）：其内部 dev 边共 **5 条**（faae-router L6 / gqep-executor・mtpe-executor・pvl-layer L7 / scc-cache L3），**向上 0 条** ⇒ 合法不入基线；“无生产消费链”一面**不是账实不符**——G-31 自陈论域仅生产面（`:23`/`:113`），dep_edge/reachability 同此，故无任何台账把 dev 边当接线声称（口径：已被看见并判为合法，不是已被消除） | ✅ |
| W6-5 `sanitize()` / `content_hash` / `PendingHuman` | N-8/C-44 交付包 | — | ⚖ 需契约级 ADR。**本轮明确拒绝“只建不接的脚手架”**：写出无消费方的类型/函数就是刚登记过的 `harness_dimensions` 旗标那种“配置面上为真、行为面上为假”（C-60） | ⚖ |
| W6-6 裁决包（P-1…P-5 / N-1…N-8 / **F-1…F-3**，每包固定六段） | `docs/reports/v3.5-w66-decision-packages-2026-10-09.md` | 逐包六段完整性目检 + 本矩阵行引用 | 13 个 P/N 包 + 3 个 F 包均为六段；阈值包含 M3/M6 实测（B） | ✅ |
| W6-7 发布门 | 本行各项 | `check --workspace --all-targets`→clippy→fmt→全量回归→light 门→audit→fuzz→版本三源。**本轮逐环跑齐**：`cargo check --workspace --all-targets` rc=0；clippy 全库 rc=0；**`fmt` 逐包 40/40 洁净**（官方门 G-19 论域 = **40 个 package：39 crates + 根 `chimera-e2e-tests`**；本批前三轮用只枚举 `crates/*` 的自建工具报“39/39”属口径错，2026-10-10 跑重门层时 G-19 抓到根包 11 行未格式化（本批自己引入）→ 已修正为 40/40 rc=0，台账 S-31；Spec 实写 `--all` 已知 os error 206）；`check_perf_redlines.py invariants` PASS（含 I8c const=16/arms=16）；`cargo audit --no-fetch` rc=0（690 依赖、6 warning 全在 allow）；`cargo check --manifest-path fuzz/Cargo.toml` rc=0；版本三源由 G-04 自报一致 | 回归 505 组/**11758 passed / 0 failed**/ignored 81（含本轮新增的 INV-9 对账 target）；全量 light **91/91 OK**（判决行数==预期数）；**重门层首次真跑**：6 条 heavy 因磁盘 14.2 GB < 门槛 20/25 GB 记 UNKNOWN（不静默跳，不得当通过），G-19 抓到根包 fmt 未净→已修至 40/40；audit/fuzz/版本均 rc=0；覆盖率＝⚖ 归 CI nightly | ✅/⚖ |
| W6-8 新发现：G-67 扫描器可见性盲点 / `composition_root_bench` 从未产数 / R2 四事件零发出点 | `check_event_ownership.py::MOD_HEAD`、`composition_root_bench.rs`、`event_ownership_baseline.txt` | 双向夹具 + 修前/修后 rc 对比（101→0） | selftest 13→17；bench 三组均产数；台账 16→20 行（A；C-56/C-57） | ✅ |
| “需你裁决事项”8 条 | §12 | 逐条按默认值执行 | 未提交（等指令）等 8 项已记 | ⚖ |
| **终局原则 4（登记台账方向不可反）首次穷尽验证** | 无代码变更；审计工具 `tmp/audit_ledger_gates2.py` | 逐张双问：“自声明方向?”与“被 expect=0 门读?”；审计工具自身两轮假阳性（窄词表认不出 only-shrink/baseline/登记表）已先排除再下结论 | **24 张（Spec 写 17 ⇒ 已入 §7.2）；方向声明 24/24；阻塞门读者 24/24；零孤儿**（A） | ✅（口径已订正：非全为只减不增） |
| **终局原则 1（文档层图禁手抄）首次获得机检** | `check_layer_map_parity.py` 第 6 源 + G-45/G-46 备注订正（不新增门 id） | 先取实况再动手：csn L7/L10 分歧已由 **ADR-189 收口**，文档正确⇒未改一字（若直接“修”反而造新错）；夹具 11→17 项（解析/一致/错层/漏项/幽灵/拒判）+ **真树两处变异均 rc=1** 并字节还原 | 6 源 39=39=39=39=39=39 全等 rc=0；首版正则缺 `\b` 致把 `Contracts`→`ontracts` 误当 crate，**由真树跑抖出（11 项原夹具全没报）** | ✅（§2.1 仍为手写块，只是已被比对） |
| **终局原则 2（crate 预算冻结）首次拿到逐项证据** | 总账 §4 附录 4A（101 行判定表，由生成器幂等写入）+ 新门 `scripts/check_v340_crate_proposals.py`（**G-97/G-98**） | 机械五态（在册/同名模块/同名符号/专名词根/无痕迹）+ 对 ②c/③ 共 43 行**逐行人工确证**；G-97 三方对账（§4.1 提案名 ≠ 成员 crate；候选名集 == 表行集；每条 `file:line` 真存在且不越界）；11 项夹具与真树双口径记账闭合 | §4.1 括号组 **70 项** / 并集 **101 项**（原文“约 40”为漂移）；**成为新 crate 0 项**；§4.1 口径落地 36+28、作废 6（并集 58/33/10）；门首跑抓到本人一条不可定位证据并补行号（非进基线） | ✅（取证器六次自纠见报告 §25.3） |
| **W6-5 尾项：INV-9 双源对账补到第二层** | `crates/chimera-mas/tests/inv9_parity_proptest.rs`（新，4 测）；消费现有生产边 `dep_edge_freeze.txt` 已册的 chimera-mas → gsoe-evolution | 先取实况再动手：**误判过台账一次**（只 grep `src/` 就认定“对账测不存在”，读 `invariants_test.rs:245-293` 后推翻自己⇒已有固定样本层，本轮只补随机图层）；牙齿：`tmp/inv9_teeth.py` 将 L5 改为跳过自环 ⇒ baseline PASS→mutated FAILED→还原 PASS 且字节全等 | clippy `-p chimera-mas --all-targets -D warnings` rc=0（`unwrap_used` 在 dev profile 也拦，已改普通 `assert!`）；fmt rc=0；`check_dependency_rules.sh` 全通过；新 target **4 passed / 0 failed**；零新增依赖边 | ✅（三处施工错已记录并修正，报告 §26.3；含文档侧第四处裸竖线） |
| **C-63：同文件内口径不自洽（“已接入” vs “生产未启用”）** | `crates/session-store/src/lib.rs`（追加注释段）、`docs/architecture/ADR-201-model-view-event-taxonomy.md` §6（追加事实注记）、本报告 §27 | 先取实况再动手：逐行定位 `server.rs:634/920/977/806/283/334` 与 `composition.rs:251`，确认“已接入”仅代码/测试层成立；**同时量体否证 Spec 自身的“② 零风险一个提交”**（17 处引用需 cfg-gate）；因 MATURITY/CONSUMERS 是 `crate_contract.py` 派生字段⇒ 不手改机器字段，只加注释与 ADR 注记 | fmt/clippy `-p session-store --all-targets` rc=0；`crate_contract --check` rc=0（39/39 契约块派生字段与盘上一致）；该过度声称在 CODE_WIKI/总账/agents.md **零出现**（已按“推翻旧数字必清扫全部出现位置”查过） | ✅（纯注释/文档，零行为变更） |
| **W6-1 具名交付物补齐（M1–M6 汇总报告）** | `docs/reports/v3.5-baseline-measurement-2026-10-10.md`（新，含§0 前提口径 / §1-§6 逐项 / §7 冻结项映射 / §8 未闭合清单 / §9 重跑命令） | 本文件是**汇总不是新测**：每个读数只搬运已有证据（`tmp/m3_knee_rounds.py`、`target/criterion/**/estimates.json`、各门 rc），并**直写未做的档**（M2 高压档与 CI clean-runner 档未做 ⇒ `hnsw_tiered` 维持 B 级）；发布的四条 bench 命令经 `cargo metadata` 逐个核实 target 真实存在（`tmp/verify_bench_targets.py`）；本轮另记两条对 Spec 自身的否证：① M4 括注把 `composition.rs:191` 的“从未编译通过”前科写成 `Cargo.toml:191`（行对文件错）；② 上一轮“三表零改动”的核验命令里有一个路径写错（`config/perf_redlines.toml` 不存在 ⇒ 那半是空查），本轮改用真实路径 `scripts/perf_redlines.toml` 等三处并确认 `git diff` 为空 | 三表现取条目数 6/11/47 与总账 §7.2 一致；三表 `git diff --stat HEAD` **无输出**；bench target 命中 4/4 rc=0 | ✅（零代码变更；具名交付物缺失已闭） |

**矩阵覆盖结论**：标 ✅ 的行均有可重跑证据；标 ⚖ 的行不是“做得不够”，而是**仓库规则（需 ADR）或用户权限（提交/降级/改）不在本会话手中**，且均已逐条交六段式决策包；标 ❌ 的行是本轮量体推翻的 Spec/子代理主张，按“不施工 + 留痕”处理。
⇒ 准确说法是：**本 Spec 在当前权限下可实施的部分已全部完成并留证；剩下的不是“遗漏”，而是被 ADR 门槛或用户授权挡住**（逐条见本表 ⚖ 行与 §12）。未拿到 ADR/授权前不得宣称 Spec 已整体完成。

> **一句话终局**：这个仓库的真实风险不在于"还缺哪个机制"，而在于**主张跑在了实现前面**。v3.5 的全部工作就是把主张拉回与现实等长——能证伪的保留，证伪不了的降级为规划或作废，别让下一轮建设者把 C 级证据当 A 级施工。
