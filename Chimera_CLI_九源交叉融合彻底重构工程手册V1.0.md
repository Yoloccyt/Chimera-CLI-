# Chimera CLI 九源交叉融合彻底重构工程手册

> # 🔖 档案化权威基线核准横幅（2026-09-02 追加，2026-09-06 核验刷新，历史文档只加注不改写）
>
> **档案化时点**：2026-09-02（2026-09-06 核验）
> **权威基线**：v2.28.0-omega（发布提交 af62e44 已落 2026-09-02，tag 待推）· 43 crates（28 生产可达 / 14 冻结孤岛 + 1 GATED（mca-gateway，ADR-177））· 144 NexusEvent（types.rs 单表，event_types.rs 镜像已退役）· 11,587 tests / 485 target（2026-09-02 重测，以实测为准）· ADR 主编号至 182（新编号段自 ADR-183 起）
> **tag 事实订正**：v2.27.1-omega 本地与 origin 均无 tag（CHANGELOG-only 补丁），实际最新已发 tag = v2.27.0-omega
>
> **与现行代码已知偏差**：
> - 基线锚 v2.27.1-omega（38 crate / 10,836 tests / 86 ADR）已陈旧；
> - 48/53 crate 目标态重命名体系与现行 43-crate 命名/分层错位，不可直接照单执行（愿景经 `docs/architecture/fusion-concept-map.md` 概念映射表承担）；
> - ADR-095~134 编号段与现行主编号至 160 重叠，新 ADR 自 161 起；
> - "17 Critical"为陈旧口径（代码真值源 `event-bus/src/bus.rs::is_critical_mpsc_event()` 现为 13，ADR-159 定稿）；
> - Ch10/Ch11 crate 命名体系（nexus-context/memory/search 等）在现行 43 crate 中不存在，其绝大部分主张已作为子模块落地于既有 crate（nexus-core/compute · event-bus · hcw-window 等）。
>
> 本文档已档案化：历史溯源 + 愿景参考，权威基线以代码 + CODE_WIKI.md + CHANGELOG.md + ADR 为准

> **版本**：v1.0.0-confluence ｜ **日期**：2026-08-22 ｜ **事实基线**：Chimera CLI v2.27.1-omega（CODE_WIKI，2026-08-20）
> **输入**：十份上传文档（去重后九个独立源，见 §2.1）｜ **输出**：单一彻底重构工程手册
> **定位**：这不是十份文档的摘要合集，而是经过 21 项交叉冲突裁决、20 轮反事实推演、九源逐一尸检之后的**重构终案**。能落地到 crate、接口、周计划与验收门禁。

> ## 🔖 落地状态（2026-08-30 追加）
>
> 本手册是 **Phase 1（ADR-095~134 合并档）的主要来源文档**。其裁决与重构骨架已随 Phase 1-5 W1-W26（2026-08-28 全部收尾）落地:ComputeBridge 双运行时、ShardedBus 分片总线、CBMR 微批写、CausalGraph(ADR-132 独立补建)等均见 ADR-095~134 合并档与 `docs/reports/phase1-*-closure.md`;本手册 §3.3 登记的 12 项被否决设计(Triple-Runtime/并行 EventBus/rayon::join 误用等)在执行期维持否决,未复活。
> 事实基线已由撰写时 v2.27.1(38 crate/10836)演进至 v2.28.0(发布提交 af62e44 已落 2026-09-02，tag 待推)：43 crate(28 可达/14 冻结孤岛 + 1 GATED，ADR-177)/144 事件/11587 tests(2026-09-02 重测)/ADR 至 182,以 CODE_WIKI 为准。本手册保留为 Phase 1 决策溯源档案。

## 查重声明

本手册与任一源文档的章节级重复率**自评低于 15%**，依据与方法见 Ch17（查重率分析）：

- **0 个章节照单全收**。包括前任融合总案 S9——其框架被继承，但其三处缺口（S8 未处置、ADR 编号冲突、半层写法张力）在本手册中被修复，且全部 15 轮推演经本手册逐条复核重述为结论登记表而非原文转录。
- **21 处交叉冲突全部显性裁决**（§2.3 裁决账本），每一处附「为什么不」的否决理由，而非沉默地二选一。
- **12 项源设计被整体否决**并登记避雷索引（§3.3）：S8 的 Triple-Runtime、RuntimeSwitcher、ShardedMap-SegQueue、wait_spin 自旋、ParallelEventBus 并存、rayon::join 误用，S3 的 feature 标志降级，S4 的忙轮询背压，S7 的两项迁移提案，S9 已否决的 GPU 卸载与第三运行时等。
- **代码骨架全部重写**。凡源文档中出现的伪代码/不可编译示例（如 S8 的 `rayon::join` 百闭包、S7 省略号示例），本手册以可编译的 Rust 2021 惯用法重铸或明确标注为伪代码。
- **数据诚实**：一切性能数字标注来源（W1 待测定 / 源文档引用 / 门禁目标），无测量不造数。

## 目录

- **Ch1** 执行摘要
- **Ch2** 源清册与交叉裁决总账
- **Ch3** 九源尸检：病灶、基因与免疫
- **Ch4** 二十轮反事实推演
- **Ch5** 设计哲学与红线
- **Ch6** 统一技术选型与 ADR 总登记（ADR-095~134）
- **Ch7** 统一分层架构（48/53 crate）
- **Ch8** PARA-CPU 六层并行模型终版
- **Ch9** 十二大融合创新点规格
- **Ch10** 核心模块从零实现
- **Ch11** 接口契约库
- **Ch12** 26 周逐周推进计划
- **Ch13** 测试与验收体系
- **Ch14** 安全模型
- **Ch15** 运维与观测
- **Ch16** 风险登记册
- **Ch17** 查重率分析
- **Ch18** UP-01~27 终局回应与开放问题
- **附录 A** 术语表 ｜ **附录 B** chimera.toml 配置参考 ｜ **附录 C** 指标字典 ｜ **附录 D** 判断债登记

---

## 第一章 执行摘要

### 1.1 这是什么

对十份上传文档（九个独立源）进行极致深度分析与多轮交叉验证后，产出的**一份**彻底重构工程手册。目标系统：Chimera CLI——Rust 2021 workspace，38 crate、L0–L10 十一层、144 个 NexusEvent 变体（17 Critical）、86 条已登记 ADR、10,836 测试全绿。重构目标：在不违反任何一条既有红线的前提下，完成 CPU 全核并行化与六个 2026 旗舰大模型架构理念的工程化迁移。

### 1.2 与前任融合（S9 v7.0.0）的关系——明说，不伪称原创

S9 是上一份、也是质量最高的一份融合总案。本手册与它的关系是**继承并修复**：

| 维度 | S9 v7.0.0 | 本手册 v1.0.0-confluence |
|---|---|---|
| 覆盖源 | S1–S7 | S1–S8（补齐 S8 的七项裁决 C8–C13/C21） |
| PARA-CPU 六层 | 建立 | 继承，修正 S8 草样中的三处不可编译/禁用模式后落终版（Ch8） |
| ADR 登记 | 095–125 | 095–134（S8 的 104~112 编号冲突重编为 126–134） |
| 半层写法 | 存在 L5.5/L6.5 等张力 | ADR-131 全部重映射回正式层（§7.4） |
| 周计划 | W1–W26 | 继承，明确 22 周基线 + 4 周缓冲双轨（ADR-134） |
| 验证体系 | criterion 门禁 + loom + 影子双跑 | 继承，新增 T-08 外部 API 守卫与 CausalGraph×W5 联动（ADR-132/133） |

### 1.3 TL;DR 八点

1. **基线唯一**：一切决策锚定 S1 v2.27.1-omega；S3–S8 基于 v2.26.0 的「缺失 MCP/ACP/Daemon」诊断属漂移误诊，已按 §2.2 收敛——mcp-mesh（L9）与 nexus-acp（L7）**增强而非新建**。
2. **双运行时，没有第三个**：tokio 管 IO，rayon 管 CPU（池 = num_cpus−2）；S8 的 Crossbeam 第三运行时否决（ADR-124/126），其 ArrayQueue 仅作分片内部实现。
3. **顺序红线不可碰**：17 个 Critical 事件永远 mpsc 单流 + 双链表同步；分片总线只服务非 Critical，按会话键 FNV-1a 哈希分 64 片。
4. **调度收敛为一个入口**：ComputeBridge 2.0（OnceLock 全局单例）统一承载 L-f 路由；S8 的 RuntimeSwitcher 合并为 `route()` 方法，输出 `DispatchPlan{Inline, Rayon, Async}` 三态（ADR-127）。
5. **十二大融合创新点**全部回答「吸收谁、否决谁、挂载哪」：HTS-CPU、ShardedBus+CBF、DetReduce、CBMR、CSC 四级压缩、ThinkingPreserve、RSB、TSR×MoE、AERA、VerifierTournament、SER、PTC（Ch9）。
6. **48/53 crate**：38 现有（12 A 级 rayon 化 / 17 B 级桥接 / 6 C 级保序 / 3 禁止并行）+ 10 新增，余量 5；新增半层一律重映射（ADR-131）。
7. **26 周双轨计划**：W1 观测基线 → W24 分片决策 → W26 GA；22 周基线 + 4 周缓冲；每周有门禁，不过门禁不进下周（Ch12）。
8. **诚实数据与零破坏**：`#![forbid(unsafe_code)]` 38/38 无例外；禁手写 SIMD；禁 Python RL 实体；禁 feature 标志降级（子命令+配置替代）；性能数字要么是 W1 测定占位，要么是门禁目标，无编造。

### 1.4 读者地图

- **架构委员会**：Ch2（裁决账本）、Ch5（红线）、Ch6（ADR 总登记）、Ch16（风险）。
- **实现工程师**：Ch7（分层）、Ch8（并行模型）、Ch10（代码骨架）、Ch11（契约）、Ch12（周计划）。
- **QA/SRE**：Ch13（测试门禁）、Ch14（安全）、Ch15（指标/故障手册/配置）。
- **审计者**：Ch17（查重率分析）、Ch18（UP-27 回应）、research/ 交叉验证痕迹。

---

## 第二章 源清册与交叉裁决总账

### 2.1 九源清册（十份上传，MD5 去重后九源）

| 编号 | 文档 | 版本/日期 | 角色 | 体量 |
|---|---|---|---|---|
| S1 | Chimera CLI CODE_WIKI 开发文档 | v2.27.1-omega，2026-08-20 | **事实基线** | 38 crate 台账/144 事件/86 ADR |
| S2 | Chimera CLI 终极演进蓝图 | v4.0.0 | **主路线图** | 10 判断债/UP-27/批次制 |
| S3 | Task A 深度优化报告 | — | 证据池 A | 7 竞品×11 维度/T1–T6 |
| S4 | Task A Rust 架构重构报告 | — | 证据池 A' | EventBusV2/背压水位 |
| S5 | Task B 架构算法深度优化方案 | v1 | 证据池 B | 12 引擎雏形/4 阶段计划 |
| S6 | Task B 深度优化计划 | v2 | 证据池 B' | 12 引擎精炼/NexusEventV2/翻译对照表 |
| S7 | 大模型架构迁移报告 | v1.0 | LLM 理念源 | 6 模型解读/5+7+2 迁移清单 |
| S8 | 并行重构方案 | v1.0 | CPU 并行源 | Triple-Runtime/背压协奏四组件 |
| S9 | 九源极致融合与 CPU 全核并行化重构总案 | v7.0.0 | **前任融合** | PARA-CPU/14 算法/ADR-095~125/W1–W26 |

> **去重说明**：上传十份中「v3.0.0」与「v7.0.0」两份总案 MD5 相同（06049703fa6a58ce731d2b6b1c889c01），计为一源（S9）。交叉验证全过程留痕于 `research/cross_verification.md`。

### 2.2 偏差收敛原则（基线漂移修正）

S3–S8 部分诊断基于 v2.26.0 旧基线，与 S1 v2.27.1-omega 存在事实偏差。**收敛规则：事实以 S1 为准，理念按新旧基线差异重新定位**。

| 漂移诊断（旧基线） | S1 事实（v2.27.1） | 收敛动作 |
|---|---|---|
| 「无 MCP/ACP 支持」（S3/S6/S7） | mcp-mesh（L9）、nexus-acp（L7）已存在 | 增强而非新建：ACP 作边界膜（E12）、mesh 承载协作突破 |
| 「Daemon 模式缺失」（S3） | nexus-app-server 已列入规划 | 落实为 L8 新 crate（§7.2） |
| 「预留层为空」（S8） | L6/L9 已有 crate | S8 的并行组件改挂载现有层（§7.4） |
| 「无事件溯源」（S8） | session-store 已规划 + 双链表同步已存在 | S8 的 k-way 归并回放降级为回放加速器（ADR-109） |

### 2.3 二十一项交叉冲突裁决账本

> 每条裁决：冲突双方 → 结论 → 理由一句话 → 落地位置。完整推演见 Ch4，避雷索引见 §3.3。

| # | 冲突 | 裁决 | 理由（一句话） | 落地 |
|---|---|---|---|---|
| C1 | 基线版本 v2.26.0（S3–S8）vs v2.27.1（S1） | 锚定 S1 | 事实基线必须唯一且最新 | §2.2 |
| C2 | 「MCP/ACP 缺失」误诊（S3/S6/S7）vs 已存在 | 增强而非新建 | 重复造轮违反 Ω₆ | §2.2、Ch7 |
| C3 | NexusEventV2 双枚举并存（S6）vs V1 为准 | V2 降级为查询视图 | 双 schema = 序列化分裂 | ADR-121 |
| C4 | 事件背压形式：统一背压（S4）vs 分级 | 分级背压，Critical 豁免 | Critical 背压 = 死锁源（推演 9） | §8.2 |
| C5 | feature 标志降级（S3）vs 子命令+配置（S9） | 子命令+配置 | feature 组合爆炸且违反红线 | ADR-122 |
| C6 | 多臂老虎机 vs 贝叶斯优化职责重叠（S5） | E03 消融，探索-利用归 E05 | 两引擎同责 = 双源真相 | Ch9 |
| C7 | RL 权重在线更新 vs 影子学习 | 影子学习 + 人工审批 | 在线硬更新不稳定且不可审计 | 推演 12 |
| C8 | Triple-Runtime（S8）vs 双运行时（S9） | 双运行时；crossbeam 内部化 | 两套窃取调度器争用同一批 OS 线程 | ADR-126、推演 16 |
| C9 | RuntimeSwitcher 独立组件（S8）vs L-f 一体 | 合并为 ComputeBridge `route()` | 决策与执行分离多一跳且统计重复 | ADR-127、推演 17 |
| C10 | ShardedMap-SegQueue 存值（S8）vs DashMap | DashMap + arc-swap | SegQueue pop 破坏性读取破坏 map 语义 | ADR-128、推演 18 |
| C11 | wait_spin/yield_now 自旋（S8/S4）vs Notify | 全系统禁自旋，统一 Notify | 500ms 自旋 × 8 线程 = 4 核秒纯浪费 | ADR-129、推演 19 |
| C12 | ParallelEventBus 与 ShardedBus 并存（S8） | 唯一分片总线；并发扇出限内部 | 两套背压协议 = 验证复杂度翻倍 | ADR-130、推演 20 |
| C13 | rayon::join 百闭包（S8 代码）vs par_iter | 修正为 par_iter/scope | join 签名只收 2 闭包，原示例不可编译 | §8.3 |
| C14 | GPU 卸载嵌入向量 vs 纯 CPU | 否决 GPU，预留 ComputeKernel | PCIe 往返吃光 CLI 场景收益 | ADR-123 |
| C15 | 手写 SIMD vs 自动向量化 | 禁手写 SIMD，双构建 | forbid(unsafe_code) + 可移植性 | ADR-101 |
| C16 | 压缩一步到位 vs 四级渐进 | 四级渐进（×1.3/×1.15/×1.0） | 信息悬崖不可逆 | ADR-119 |
| C17 | Thinking 块压缩时丢弃 vs 保留 | 保留（ThinkingPreserve） | Qwen preserve_thinking 证据 + Ω₉ | Ch9-T02 |
| C18 | Python RL 实体 vs Rust-only | Rust-only 或搁置 | R2 冻结令 2026-08-15 无豁免 | §5.2 红线 5 |
| C19 | 六大权衡的 feature 标志方案（S3） | 否决，改配置档案 | 同 C5；T1–T6 用配置组合表达 | ADR-122 |
| C20 | 4 阶段粗计划（S5）vs 周计划（S9） | 周计划 + 22/4 双轨 | 无周粒度则无门禁 | ADR-134 |
| C21 | S8 的 ADR-104~112 与 S9 编号冲突 | S8 贡献重编为 ADR-126~134 | ADR 编号即历史，不可覆盖 | §6.2 |

**裁决原则**：能回答「为什么不」的融合才是真融合。每一处否决都留下避雷索引（§3.3），每一处修正都留下前后对照。

---

## 第三章 九源尸检：病灶、基因与免疫

> 每份文档按同一把手术刀解剖：**病灶**（被本手册否决或修正的具体设计）、**基因**（被本手册吸收的原创贡献）、**免疫**（为防止病灶复发而写入的制度性条款）。解剖之后给出来源关系图与融合平衡表。

### 3.1 九源逐一尸检

**S1｜CODE_WIKI 开发文档 v2.27.1-omega（事实基线）**

| 类别 | 内容 |
|---|---|
| 基因 | 38 crate 精确台账与 L0–L10 分层拓扑；144 个 NexusEvent 变体与 17 Critical 双链表同步机制；OMEGA 九定律；86 ADR 已有登记；8 个真实测试缺口与判断债清单；86% 公开 API 带 `#[must_use]` 的契约文化 |
| 病灶 | 非病灶而是"现实"：事件总线单线程扇出在重负载下饱和；parking 机制治标不治本；事件订阅无背压控制 |
| 免疫 | 本手册所有架构决策以 S1 为唯一事实基线（§2.2 偏差收敛原则）；Ch13 修复计划逐项对应 S1 的 8 项判断债 |

**S2｜终极演进蓝图 v4.0.0（主路线图）**

| 类别 | 内容 |
|---|---|
| 基因 | 10 项判断债的修复优先级排序（P0/P1/P2）；UP-01~27 终局质询清单；架构漂移三招（半层、循环依赖、ASCII 图与代码不同步）；批次制修复（B1~B5）+ 三大停滞突破项目（Memory/Skills/团队协作） |
| 病灶 | 无实质病灶（质询型文档）；部分批次工作量估计偏乐观 |
| 免疫 | 批次制被吸收为 §12.2 波次 0；UP-27 全部得到回应（Ch18）；"不允许新增半层"升级为 §5.2 红线与 Ch7 半层否决重映射表 |

**S3｜Task A 深度优化报告（证据池 A）**

| 类别 | 内容 |
|---|---|
| 基因 | 7 个竞品 CLI 的 11 维度对比；4 个不可妥协核心（会话连续性、审批、纯 Rust 安全、extism WASM）；6 项关键权衡 T1–T6；10 个消除 flake 的具体设计；Agent 反馈注入与传统 RL 的本质区分 |
| 病灶 | 基线漂移（误诊断 v2.26.0 无 MCP/ACP/Daemon）；建议为六大权衡引入 feature 标志 + 配置覆盖的降级路径——违反"禁止 feature 标志"红线，降级复杂度爆炸 |
| 免疫 | C19 裁决：feature 标志否决，按 S9 模式改为子命令 + 配置文件降级；偏差收敛原则（§2.2）直接源于对 S3 病灶的反思 |

**S4｜Task A 重构报告（证据池 A'）**

| 类别 | 内容 |
|---|---|
| 基因 | 事件总线背压水位机制的具体形态；安全事件流（SecurityEventBus）的思路 |
| 病灶 | EventBusV2 的 `try_send` 忙轮询 + `yield_now` 自旋——属禁止忙等；v2.26.0 漂移 |
| 免疫 | C4/C11 裁决：背压保留、自旋否决，统一为 async `send` + CBF 信用流；"事件总线不得自旋"写入 §8.2 禁令表 |

**S5｜Task B 架构优化方案 v1（证据池 B）**

| 类别 | 内容 |
|---|---|
| 基因 | 12 引擎规格表的雏形；TaskAuctionMarket / SymbolicChecker / ConsistencyGuardian 三个辅助设计（其中 ConsistencyGuardian 被吸收为缝合点 S4）；8 个月 4 阶段计划框架 |
| 病灶 | 引擎之间存在概念重复（多臂老虎机与贝叶斯优化职责重叠）；阶段计划无周粒度 |
| 免疫 | 引擎去重合并（E03 贝叶斯消融，探索-利用归 E05）；周计划模板由 S9 建立并被本手册继承 |

**S6｜Task B 深度优化计划 v2（证据池 B'）**

| 类别 | 内容 |
|---|---|
| 基因 | 12 引擎精炼版 + 每引擎失败模式表；NexusEventV2 分类设计；LLM→CLI 翻译对照表；E11 Hook 生命周期四阶段；E12 ACP 作为边界膜 |
| 病灶 | NexusEventV2 与 V1 双枚举并存会造成序列化分裂——S9 已裁决 V2 降级为查询视图，本手册维持；v2.26.0 漂移 |
| 免疫 | C3 裁决维持；Ch11 契约库只定义一份事件 schema |

**S7｜大模型架构迁移报告 v1.0（LLM 理念源）**

| 类别 | 内容 |
|---|---|
| 基因 | 六个 2026 旗舰模型的技术解读（DeepSeek CSA、Kimi KDA/MoE/RLVR、GLM IndexShare、MiniMax MSA、Qwen GDN）；5 项零样本迁移 + 7 项改造迁移 + 2 项否决的全部理由；Thinking 保留三建议；上下文四级压缩阈值表；Error Recovery Gym |
| 病灶 | 部分代码示例省略号过多不可直接编译（注释中已自承）；微调代码片段走 Python 生态但被 R2 冻结令拦截——该文档自身也已标注 |
| 免疫 | 代码重写与占位符原则（§5.4 第 4 条）；Python RL 实体禁令（§5.2 红线 5）在本手册中无条件执行 |

**S8｜并行重构方案 v1.0（CPU 并行源）**

| 类别 | 内容 |
|---|---|
| 基因 | Triple-Runtime 中 rayon 与 tokio 的划分直觉（计算与 IO 分离）；L-a 层固定 5 步模版；背压协奏四组件（ShardedBus/CBF/BatchedSequencer/DetReduce）；L-f 自适应调度的方向正确性 |
| 病灶 | ① 引入 Crossbeam 作为第三运行时——S9 ADR-124 已否决，S8 属重复提案；② RuntimeSwitcher 独立组件与 L-f 职责重叠；③ `ShardedMap` 用 SegQueue 存值——pop 语义破坏键值持久性，属 API 误用；④ `wait_spin()` 自旋——禁止忙等；⑤ `rayon::join` 一次提交 100 个闭包——join 签名只接受 2 个闭包，示例代码不可编译；⑥ ParallelEventBus 与 ShardedBus 功能重复；⑦ v2.26.0 漂移（声称无事件溯源、预留层为空） |
| 免疫 | C8–C13、C21 七项裁决（§2.3）逐条对应；S8 经修正后的部分（C9/C10/C11/C12）才被融合；"双运行时而非三运行时"写入 §5.2 红线 6 |

**S9｜九源融合与 CPU 全核并行化总案 v7.0.0（前任融合）**

| 类别 | 内容 |
|---|---|
| 基因 | 全部框架性遗产：PARA-CPU 六层模型；HTS-CPU 阈值体系；ComputeBridge 2.0；14 项算法规格；38 crate 处置表与 10 个新 crate 提案；ADR-095~125；22 条变体否决登记；W1–W26 周计划模板；验证体系（criterion 门禁 + 幂等审计 + 影子双跑 + loom）；配置参考与指标字典；15 轮反事实推演 |
| 病灶 | 未覆盖 S8（Triple-Runtime 处置不完整）；ADR-104~112 与 S8 编号冲突未解决；少数新 crate 的层归属与"禁止半层"原则存在张力（L5.5/L6.5 等写法） |
| 免疫 | 本手册即为此三病灶的修复：S8 处置补齐（C8–C13/C21）、ADR 重编号（ADR-126~134）、半层重映射（§7.4）；同时明文声明继承关系（§1.2），不伪称原创 |

### 3.2 来源关系与融合平衡表

```
                    大模型架构前沿（DeepSeek V4 / Kimi K3 / GLM 5.3 / MiniMax M3 / Qwen3.8-Max）
                                        │  理念迁移（零样本 5 / 改造 7 / 否决 2）
                                        ▼
   S1 事实基线 ──┐                ┌── S7 LLM 迁移报告
                 │                │
   S2 主路线图 ──┼──► S9 前任融合（PARA-CPU v7.0.0）──┐
                 │                ▲                  │ 继承并补齐三缺口
   S3/S4 证据 A ─┘                │                  ▼
   S5/S6 证据 B ──────────────────┘          ★ 本手册 v1.0.0-confluence
                                        ▲
   S8 并行重构方案（新源，经 7 项裁决后部分融合）──┘
```

| 统计维度 | 数量 | 说明 |
|---|---|---|
| 交叉验证冲突裁决 | 21 | §2.3 全登记 |
| 对源文档的修正 | 21 | 每条裁决均含修正动作 |
| 整体否决的设计提案 | 12 | S3 三运行时/S4 忙等/S8 七处/S7 两项迁移/S9 已否 GPU 沿用 |
| 照单全收的章节 | 0 | 即使 S9 也经逐节重读与三处修复 |
| 新增原创构件 | 5 | DispatchPlan 三态路由、CausalGraph×W5 联动、T-08 外部 API 守卫、半层重映射表、本手册自身的九源审计链 |

### 3.3 避雷索引（十二项整体否决登记）

| # | 被否决设计 | 来源 | 一句话避雷理由 | 裁决/推演 |
|---|---|---|---|---|
| R-01 | Triple-Runtime（crossbeam 第三运行时） | S8 | 两套窃取调度器争用同一批 OS 线程 | C8 / 推演 16 / ADR-126 |
| R-02 | RuntimeSwitcher 独立调度组件 | S8 | 决策与执行分离多一跳，统计重复 | C9 / 推演 17 / ADR-127 |
| R-03 | ShardedMap 以 SegQueue 存值 | S8 | pop 破坏性读取破坏 map 幂等语义 | C10 / 推演 18 / ADR-128 |
| R-04 | `wait_spin()` 自旋等待 | S8 | 毫秒级 LLM 间隔上自旋 = 核秒纯浪费 | C11 / 推演 19 / ADR-129 |
| R-05 | ParallelEventBus 与 ShardedBus 并存 | S8 | 双总线 = 双背压协议 = 验证翻倍 | C12 / 推演 20 / ADR-130 |
| R-06 | `rayon::join` 提交百个闭包 | S8 | join 只收 2 闭包，示例不可编译 | C13 / §8.3 |
| R-07 | feature 标志降级路径 | S3 | 组合爆炸，违反「禁 feature 标志」红线 | C5/C19 / ADR-122 |
| R-08 | EventBusV2 `try_send`+`yield_now` 忙轮询 | S4 | 事件总线不得忙等 | C4/C11 / §8.2 |
| R-09 | 逐行翻译 LLM 训练代码到 CLI | S7 | 训练语义 ≠ 推理语义，必须重铸 | §5.3 第 4 条 |
| R-10 | Python RL 实体组件 | S7 | R2 冻结令无豁免 | C18 / 红线 5 |
| R-11 | GPU 卸载嵌入向量 | S9 登记沿用 | PCIe 往返吃光 CLI 微批收益 | C14 / ADR-123 |
| R-12 | 手写 SIMD / 内联汇编 | S9 登记沿用 | forbid(unsafe_code) + 可移植性 | C15 / ADR-101 |

---

## 第四章 二十轮反事实推演

> 对每项关键设计做"如果不这样会怎样"的压力测试。第 1–15 轮继承 S9 结论（经验证其推理链完整，不重述过程，只登记结论与本文档的复核意见）；第 16–20 轮为本手册新增，专门裁决 S8 带来的争议。

### 4.1 继承的十五轮（S9 推演，本手册复核通过）

| # | 反事实设问 | 结论 | 复核 |
|---|---|---|---|
| 1 | 事件总线直接换 crossbeam 无界通道？ | 否决：无界 = 内存泄漏温床，背压缺失 | ✅ 维持 |
| 2 | 分片总线按事件类型分片？ | 否决：类型分布不均导致热点；按会话键 FNV-1a 哈希 | ✅ 维持 |
| 3 | Critical 事件也走分片？ | 否决：17 Critical 红线，顺序性不可分片 | ✅ 维持 |
| 4 | rayon 线程数 = num_cpus 全用？ | 否决：饿死 tokio 与 UI；num_cpus−2 | ✅ 维持 |
| 5 | 每次 compute 调用新建 rayon pool？ | 否决：池创建毫秒级开销；OnceLock 全局单例 | ✅ 维持 |
| 6 | sqlite 直接在 rayon 线程跑？ | 否决：IO-on-rayon 红线；spawn_blocking | ✅ 维持 |
| 7 | 归约用普通浮点累加？ | 否决：跨构建不可复现；审计模式 ReproBLAS 预舍入 | ✅ 维持 |
| 8 | 手写 SIMD 内联汇编？ | 否决：forbid(unsafe_code) 与可移植性；自动向量化 + 双构建 | ✅ 维持 |
| 9 | 所有事件都加统一背压？ | 否决：Critical 背压 = 死锁源；分级背压（Critical 豁免） | ✅ 维持 |
| 10 | 压缩一次到位（直接 Autocompact）？ | 否决：信息悬崖；四级渐进 ×1.3/×1.15/×1.0 | ✅ 维持 |
| 11 | Thinking 块在压缩时直接丢弃？ | 否决：Qwen preserve_thinking 证据；T-02 保留 | ✅ 维持 |
| 12 | RL 权重在线硬更新？ | 否决：在线学习不稳定；影子学习 + 人工审批 | ✅ 维持 |
| 13 | GPU 卸载嵌入向量？ | 否决（ADR-123）：CLI 场景 PCIe 往返吃光收益；预留 ComputeKernel | ✅ 维持 |
| 14 | 阶段间用共享内存 + 锁？ | 否决：违反锁最小化；有界 mpsc 流水线 | ✅ 维持 |
| 15 | 调度阈值拍脑袋定？ | 否决：HTS-CPU 先离线测、再运行时序检验、再 Promote | ✅ 维持 |

### 4.2 新增五轮（裁决 S8）

**第 16 轮：如果接受 S8 的 Triple-Runtime（tokio + rayon + crossbeam）？**
推演：crossbeam 的 work-stealing deque 与 rayon 的 Chase-Lev deque 在算法层同构，引入后形成两套窃取调度器争用同一批 OS 线程；线程会计表需要为三池各留余量，总预留从 2 核升到 3–4 核，计算池可用核数下降；依赖面 +1（crossbeam 全家桶）。结论：**否决**，与 ADR-124 一致。crossbeam 降级为 nexus-event-bus 分片内部实现细节（ArrayQueue 即源于 crossbeam 生态），不作为第三运行时暴露。

**第 17 轮：如果接受 RuntimeSwitcher 独立组件？**
推演：RuntimeSwitcher 决策时延 50–200μs，而 L-f 的 HTS 阈值判断是纳秒级查表；独立组件意味着调度决策与调度执行分离，多出一次 IPC 或至少一次锁；且其历史窗口统计与 L-f 的 PostHog 时序检验重复。结论：**否决独立性，合并吸收**为 ComputeBridge L-f 路由器的 `route()` 方法，输出 `DispatchPlan{Inline, Rayon, Async}` 三态（C9）。

**第 18 轮：如果接受 ShardedMap 的 SegQueue 存值设计？**
推演：`SegQueue::pop` 是破坏性读取——弹出后值消失，而 map 语义要求 `get` 幂等可重复。S8 自己的 `worker_loop` 示例里 `map.get(&key)` 期望返回完整 Vec，与 SegQueue 语义直接矛盾；强行使用会导致幂等消费者读到空值，违反"幂等消费者"要求。结论：**否决**，热路径会话状态用 `DashMap`（分片锁、读多写少近乎无竞争）+ 配置热更用 `arc-swap`（无锁读、单次写替换）（C10）。

**第 19 轮：如果接受 `wait_spin()` 自旋等待？**
推演：LLM 流式间隔 50–500ms，自旋 8 线程 × 500ms = 4 核秒的纯浪费，与"每一纳秒计算能力服务于架构目标"的纲领直接冲突；且自旋线程不参与 rayon 窃取调度，破坏线程会计。结论：**否决**，统一 `tokio::sync::Notify` + `notified().await`，微秒级唤醒、零 CPU 占用（C11）。S4 的 `yield_now` 忙轮询同理否决。

**第 20 轮：如果接受 ParallelEventBus 与 ShardedBus 并存？**
推演：两个并行总线 = 两套背压协议 + 两套指标 + 订阅者需要选择困难；事件投递路径分裂使 Ch13 的双跑验证复杂度翻倍。结论：**否决并存**，ShardedEventBus 为唯一分片总线；`for_each_concurrent` 式的并发扇出作为 ParallelEventBus 的残留价值，降级为 SessionManager 内部的 `futures` 工具调用（opt-in，不进公共 API）（C12、C21）。

---

## 第五章 设计哲学与红线

### 5.1 OMEGA 九定律（继承 S1，重述为祈使句）

| 定律 | 祈使句 | 违反案例 |
|---|---|---|
| Ω₁ Sparse | 每模块只依赖确切所需；禁伞型依赖 | 任何 crate 依赖 nexus-core 全家桶 |
| Ω₂ Deterministic | 相同输入相同输出；种子显式传递 | `HashMap` 迭代序泄漏进序列化 |
| Ω₃ Contract | 公开 API 必须文档 + `#[must_use]` + 错误类型化 | `unwrap()` 出现在库代码 |
| Ω₄ Traceable | 每决策可追溯到 ADR 或 issue | 口头约定改架构 |
| Ω۵ Recoverable | 任何失败有定义好的降级路径 | panic 沿用户输入路径传播 |
| Ω₆ Minimal | 能用标准库不引依赖 | 为 shuffle 引 rand 全家桶 |
| Ω₇ Testable | 纯函数与效果分离；缝合点可注入 | 业务逻辑里直接 `SystemTime::now()` |
| Ω₈ Observable | 每跨边界操作有结构化日志 | `println!` 调试残留 |
| Ω₉ Preserve | 上下文与推理痕迹不可静默丢弃 | 压缩时丢弃 thinking 块 |

### 5.2 六条红线（绝对禁止，凌驾所有优化）

1. **顺序红线**：17 个 Critical 事件永远走 mpsc 单流 + 双链表同步；顺序敏感通道永不分片、永不近似、永不丢弃。
2. **分层红线**：L0–L10 依赖只向下；跨层只经事件总线；跨进程只经 mcp-mesh；**禁止新增半层**（Lx.5 一律重映射，见 §7.4）。
3. **运行时红线**：sqlite 与一切阻塞 IO 永不进 rayon；rayon 池 = num_cpus−2；批大小 = 线程数倍数。
4. **forbid(unsafe_code)**：38/38 crate 无例外；simd-json 类需求走 cfg(feature) 白名单审（本周期无需求）。
5. **Rust-Only**：禁止新增 Python 实体组件；R2 冻结令（2026-08-15）无豁免；RL 影子学习用 Rust（burn/inference only）或搁置。
6. **双运行时**：tokio（IO）+ rayon（计算）为全部；禁止第三调度器进入公共 API（crossbeam 仅作内部实现）。

### 5.3 八条设计哲学（融合九源共识）

1. **诚实数据**：无测量不写数字；一切阈值有方法论来源（离线测定 / 时序检验 / cgroup 校正）。
2. **渐进增强**：所有优化可关、可降、可回退（子命令 + 配置，非 feature 标志）；正确性先行于性能。
3. **融合非堆砌**：每个创新点回答「吸收了谁、否决了谁、挂载在哪」；否决与吸收同等重要。
4. **代码重写**：迁移的理念必须重铸为 Rust 惯用法，杜绝逐行翻译；示例代码必须可编译或标注为伪代码。
5. **判断债显性化**：每个 TODO/FIXME 关联判断债编号与最晚偿还版本。
6. **契约先行**：接口先于实现冻结（Ch11），实现可迭代，契约只增不破坏。
7. **证据链完整**：源码位置 → 优化点 → 验证方法 → 回退路径，四件套缺一不可。
8. **Ω₉ 扩展**：不仅保留用户上下文，也保留**决策上下文**——本手册的 21 项裁决账本即示范。

---

## 第六章 统一技术选型与 ADR 总登记

### 6.1 技术选型表（裁决后）

| 领域 | 选型 | 版本约束 | 落选方案与理由 |
|---|---|---|---|
| 异步运行时 | tokio | 1.x, rt-multi-thread | smol（生态窄）；自研 executor（疯狂） |
| CPU 并行 | rayon | 1.10+ | crossbeam 作为第三运行时（ADR-124/126）；std::thread 手搓（无窃取调度） |
| 无锁队列 | crossbeam-queue ArrayQueue | 内部实现用 | SegQueue 存 map 值（C10）；flume（重复 tokio mpsc） |
| 并发 map | dashmap + arc-swap | 热状态 / 配置 | ShardedMap-SegQueue（C10）；RwLock<HashMap>（写饥饿） |
| 事件通道 | tokio::sync::mpsc（Critical）+ 分片 ArrayQueue（非 Critical） | — | crossbeam channel 无界（推演 1）；broadcast（无背压） |
| 序列化 | serde + bincode（内部）/ serde_json（边界） | — | rmp（调试体验差） |
| 存储 | rusqlite + WAL（spawn_blocking） | — | sled（维护停滞）；redb（待 W1 评估） |
| 全文检索 | sqlite FTS5（spawn_blocking） | — | tantivy（重，且与 sqlite 双源真相） |
| 向量索引 | 自研 flat + 阈值后 hnsw（usearch 评估中） | W1 测定 | faiss（C++ 绑定违反 Ω₆） |
| 随机 | ChaCha8（rand_chacha，显式种子） | — | thread_rng（Ω₂ 违反） |
| 模糊评分 | 保留现有 skim 移植 | — | 重写（风险大于收益） |
| 进程隔离 | landlock/seccomp（Linux）+ sandbox-exec（macOS）+ Job Object（Windows）+ fork 兜底 | — | 容器（CLI 过重） |
| 确定性归约 | 固定分块树 + ReproBLAS 预舍入（审计模式） | — | Kahan（跨 FMA/非 FMA 仍漂移，推演 7） |
| 时序检验 | 自实现 P-hacking 防御版序贯检验 | — | 直接引 posthog 库（crate 不存在，S9 原文指方法而非库） |
| 指标 | 自研原子计数器 + Prometheus 导出（nexus-telemetry） | — | statsd（额外守护进程） |
| 模糊评分/GPU | 不引入 | ADR-123 | Candle/CUDA（PCIe 往返吃光收益） |

### 6.2 ADR 总登记（ADR-095 ~ ADR-134，40 条）

| 编号 | 决策 | 状态 | 来源 |
|---|---|---|---|
| ADR-095 | 双运行时分工（tokio=IO / rayon=CPU） | ✅ 已决 | S9 |
| ADR-096 | ComputeBridge 全局单例 + oneshot 返回 | ✅ 已决 | S9 |
| ADR-097 | rayon 池 = num_cpus−2 | ✅ 已决 | S9 |
| ADR-098 | 批大小 = 线程数倍数 | ✅ 已决 | S9 |
| ADR-099 | IO-on-rayon 禁令（sqlite→spawn_blocking） | ✅ 已决 | S9 |
| ADR-100 | 双构建矩阵（x86-64-v3 + native） | ✅ 已决 | S9 |
| ADR-101 | 禁止手写 SIMD | ✅ 已决 | S9 |
| ADR-102 | 跨构建确定性：双模式归约 + 1e-6 容忍 | ✅ 已决 | S9 |
| ADR-103 | HTS-CPU 阈值三重来源法 | ✅ 已决 | S9 |
| ADR-104 | ShardedBus 背压协议 = 分片丢弃 + mpsc 阻塞 | ✅ 已决 | S9 |
| ADR-105 | Session 会话键分片（FNV-1a + 启动快照） | ✅ 已决 | S9 |
| ADR-106 | 双模式归约挂载 DetReduce 管线 | ✅ 已决 | S9 |
| ADR-107 | ComputeKernel trait 预留（GPU 钩子但不实现） | ✅ 已决 | S9 |
| ADR-108 | CBMR 微批读取（读/写分区放大） | ✅ 已决 | S9 |
| ADR-109 | k-way 归并时间线回放 | ✅ 已决 | S9 |
| ADR-110 | VerifierTournament 五阶段验证管线 | ✅ 已决 | S9 |
| ADR-111 | SER 两阶段检索 + HNSW 阈值门控 | ✅ 已决 | S9 |
| ADR-112 | TSR×MoE 任务-子代理路由（top-k 6~8） | ✅ 已决 | S9 |
| ADR-113 | AERA 自适应错误恢复分配 | ✅ 已决 | S9 |
| ADR-114 | CBF 信用流背压 | ✅ 已决 | S9 |
| ADR-115 | RSB 三缓冲模拟状态 | ✅ 已决 | S9 |
| ADR-116 | CausalGraph 5s 归因窗口 | ✅ 已决 | S9 |
| ADR-117 | ProcedureRegistry 三级检索 | ✅ 已决 | S9 |
| ADR-118 | PTC 并行工具协调 DAG | ✅ 已决 | S9 |
| ADR-119 | CSC 四级压缩阈值 ×1.3/×1.15/×1.0 | ✅ 已决 | S9 |
| ADR-120 | L-f 任务粒度自适应调度 | ✅ 已决 | S9 |
| ADR-121 | NexusEventV2 降级为查询视图（序列化仍以 V1 为准） | ✅ 已决 | S9 |
| ADR-122 | 降级路径 = 子命令 + 配置（禁 feature 标志） | ✅ 已决 | S9 |
| ADR-123 | 否决 GPU 卸载 | ✅ 已决 | S9 |
| ADR-124 | 否决 crossbeam 作为第三运行时 | ✅ 已决 | S9 |
| ADR-125 | PerCpuPadded 遥测计数器（批提交） | ✅ 已决 | S9 |
| **ADR-126** | **S8 Triple-Runtime 否决登记**（crossbeam 降为内部实现） | ✅ 已决 | 本手册 C8 |
| **ADR-127** | **RuntimeSwitcher 合并入 ComputeBridge L-f route()，DispatchPlan 三态** | ✅ 已决 | 本手册 C9 |
| **ADR-128** | **DashMap/arc-swap 替代 ShardedMap-SegQueue** | ✅ 已决 | 本手册 C10 |
| **ADR-129** | **全系统禁自旋：统一 Notify 等待** | ✅ 已决 | 本手册 C11 |
| **ADR-130** | **ParallelEventBus 并入 ShardedEventBus；for_each_concurrent 限内部** | ✅ 已决 | 本手册 C12/C21 |
| **ADR-131** | **半层否决与 L5.5/L6.5/L7.5/L8.5/L9.5 重映射**（§7.4） | ✅ 已决 | 本手册 |
| **ADR-132** | **CausalGraph × W5 反馈回路联动设计** | ✅ 已决 | 本手册缝合 |
| **ADR-133** | **T-08 外部 API 守卫（LLM 供应商漂移检测）** | ✅ 已决 | 本手册缝合 |
| **ADR-134** | **22 周基线 + 4 周缓冲的双轨计划** | ✅ 已决 | 本手册 |

---

## 第七章 统一分层架构（48/53）

### 7.1 层职责宪法（L0–L10，继承 S1 并冻结）

| 层 | 名称 | 职责 | 允许依赖 |
|---|---|---|---|
| L0 | 基座 | 错误类型、日志、追踪、时间、ID | 仅外部 crate |
| L1 | 契约 | 事件定义、trait 契约、DTO | L0 |
| L2 | 工具 | 内置工具实现（fs/shell/web） | L0–L1 |
| L3 | 持久化 | sqlite、会话存储、回放 | L0–L2 |
| L4 | 智能体核心 | 会话状态机、上下文、压缩 | L0–L3 |
| L5 | 编排 | 调度、子代理、MoE 路由、并行工具 | L0–L4 |
| L6 | 智能 | 过程记忆、技能库、影子学习 | L0–L5 |
| L7 | 协议 | MCP 客户端、ACP 边界膜、LSP | L0–L6 |
| L8 | 接口 | TUI、CLI、app-server | L0–L7 |
| L9 | 网格 | mcp-mesh 跨进程、协作 | L0–L8 |
| L10 | 应用 | 二进制装配、配置、诊断 | 全部 |

铁律：**依赖只向下**；跨层通信只经 nexus-event-bus；跨进程只经 mcp-mesh；任何 crate 不得反向依赖上层。

### 7.2 48 crate 总账（38 现有 + 10 新增）

**现有 38 个的处置摘要**（完整逐 crate 处置继承 S9 §5.3，此处登记变更项）：

- **12 个 A 级并行化 crate**（rayon 直接受益）：nexus-context、nexus-memory、nexus-index、nexus-search、nexus-session、nexus-compress、nexus-tokenizer、nexus-diff、nexus-verify、nexus-sim、nexus-sched、nexus-proc。
- **17 个 B 级**（ComputeBridge 封装调用）。
- **6 个 C 级**（保持顺序，仅微批优化）。
- **3 个禁止并行化**：nexus-provider、nexus-acp、nexus-market（纯 IO，进 rayon 即违反红线 3）。

**新增 10 个**（48/53，余量 5）：

| crate | 层 | 职责 | 来源裁决 |
|---|---|---|---|
| nexus-app-server | L8 | 外部程序化 API（JSON-RPC over stdio/socket） | S9 提案，确认 |
| session-store | L3 | 会话持久化与回放（从 nexus-session 拆出） | S9 提案，确认 |
| mas-sched | L5 | 多代理调度器（TSR×MoE 挂载点） | S9 提案，确认 |
| nexus-sparse-attention | L4 | 稀疏注意力上下文选择（CSA/KDA 迁移） | S9 提案，确认 |
| nexus-moe-router | L5 | MoE 路由核心（aux-loss-free 偏置） | S9 提案，确认 |
| nexus-compress | L4 | CSC 四级压缩 | S9 提案，确认 |
| nexus-residual | L4 | 注意力残差（AttnRes 迁移） | S9 提案，确认 |
| nexus-subagent | L5 | 子代理生命周期（Agent Swarm 迁移） | S9 提案，确认 |
| nexus-hook | L2 | Hook 生命周期四阶段 | S9 提案，确认 |
| nexus-telemetry | L0 | 指标、PerCpuPadded 计数器、Prometheus 导出 | S9 提案，确认 |

### 7.3 依赖拓扑（关键路径）

```
L10 chimera-cli (bin)
 └─ L8 nexus-tui / nexus-app-server
     └─ L7 nexus-mcp / nexus-acp
         └─ L6 nexus-proc / nexus-skills / nexus-learn(shadow)
             └─ L5 mas-sched / nexus-moe-router / nexus-subagent
                 └─ L4 nexus-session / nexus-compress / nexus-sparse-attention / nexus-residual
                     └─ L3 session-store / nexus-store
                         └─ L2 nexus-tools / nexus-hook
                             └─ L1 nexus-events (契约)
                                 └─ L0 nexus-core / nexus-telemetry
横向：nexus-event-bus（L1 特例，人人可用）；mcp-mesh（L9，跨进程唯一通道）
```

### 7.4 半层否决重映射表（ADR-131）

S6/S7/S9 中出现的 L5.5/L6.5/L7.5/L8.5/L9.5 写法一律映射回正式层，并在层内以模块区分：

| 原写法 | 重映射 | 落地形态 |
|---|---|---|
| L5.5「调度增强层」 | L5 | mas-sched 内 `enhanced` 模块 |
| L6.5「元学习层」 | L6 | nexus-learn 内 `meta` 模块（影子模式） |
| L7.5「协议适配层」 | L7 | nexus-acp 内 `adapters` 模块 |
| L8.5「外部接口层」 | L8 | nexus-app-server（独立 crate，层内位置） |
| L9.5「协作网格扩展」 | L9 | mcp-mesh 内 `collab` 模块 |

理由：半层破坏依赖铁律的可检查性（cargo-deny 无法表达半层），且 S2 已将其列为漂移三招之首。层内模块 + feature 无关的 cfg(target_os) 足以表达全部差异。

---

## 第八章 PARA-CPU 六层并行模型终版

> 继承 S9 的六层骨架，修正 S8 草样中的三处禁用模式（自旋、SegQueue 存值、rayon::join 误用）后落终版。六层：L-f 任务粒度自适应调度 → L-a rayon 计算池 → L-b Tokio 结构化并发 → L-c 流水线 → L-d 自动向量化与双构建 → L-e 进程隔离。

### 8.1 总览图

```
                 ┌──────────────────────────────────────────────┐
                 │ L-f  任务粒度自适应调度（HTS-CPU 阈值表）      │
                 │   route(task) -> DispatchPlan                │
                 └───────┬──────────────┬─────────────┬────────┘
                         │ Inline       │ Rayon       │ Async
                         ▼              ▼             ▼
                 (调用线程直跑)   ┌───────────┐  ┌────────────────┐
                                 │ L-a rayon │  │ L-b tokio      │
                                 │ num_cpus-2│  │ JoinSet/FUS/   │
                                 │ 2MB stack │  │ try_join_all   │
                                 └─────┬─────┘  └───────┬────────┘
                                       │ oneshot         │ mpsc
                                       ▼                 ▼
                 ┌──────────────────────────────────────────────┐
                 │ L-c  流水线：WI-07 → WI-17 → WI-13            │
                 │   有界 mpsc + Send-only + CBF 信用流           │
                 └──────────────────────┬───────────────────────┘
                                        ▼
                 ┌──────────────────────────────────────────────┐
                 │ L-d  自动向量化 + 双构建（v3 portable/native） │
                 │ L-e  进程隔离（4 沙箱后端，不可信工具）         │
                 └──────────────────────────────────────────────┘
```

### 8.2 六层规格与禁令表

| 层 | 职责 | 关键机制 | 绝对禁令 |
|---|---|---|---|
| L-f 调度 | 决定每个任务在哪执行 | HTS-CPU 阈值表 + PostHog 式序贯检验 + cgroup 核数校正；输出 `DispatchPlan{Inline, Rayon, Async}` | 禁止运行时新建调度器实例；禁止阈值拍脑袋 |
| L-a 计算 | CPU 密集批量计算 | OnceLock 全局 rayon 池（num_cpus−2，2MB 栈）；oneshot 返回；信号量切片防大任务独占 | 禁 IO（含 sqlite）进 rayon；禁手写 SIMD；禁 `join` 超 2 闭包误用 |
| L-b 并发 | LLM/网络等 IO 并发 | JoinSet（生命周期有界）、FuturesUnordered（流式）、try_join_all（全成或全败） | 禁在 tokio worker 做 >1ms CPU 计算（须 offload 到 L-a） |
| L-c 流水线 | 阶段间数据流动 | WI-07 解析 → WI-17 检索 → WI-13 注入；有界 mpsc；只传 Send 类型；CBF 信用流背压 | 禁共享内存+锁跨阶段；禁无界通道 |
| L-d 向量化 | 榨干单核 | auto-vectorization；`target-cpu=x86-64-v3`（便携）与 `native`（本地）双构建；`compile_error!` 守卫防误配 | 禁 `unsafe` intrinsics；禁单构建发布 |
| L-e 隔离 | 不可信代码执行 | landlock/seccomp（Linux）、sandbox-exec（macOS）、Job Object（Windows）、fork 兜底 | 禁在主进程内 eval 不可信工具输出 |

### 8.3 L-f 路由器（融合 S8 RuntimeSwitcher 后的终版形态，ADR-127）

```rust
// nexus-core/src/compute/bridge.rs（骨架，可编译级伪代码）
use std::sync::OnceLock;

/// 三态派发计划：S8 的 RuntimeSwitcher 裁决后被合并为此枚举（ADR-127）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchPlan {
    Inline,   // 任务量 < 阈值：调用线程直跑，零调度开销
    Rayon,    // CPU 密集且 >= 阈值：进 L-a 全局池
    Async,    // IO 密集：进 L-b tokio
}

pub struct ComputeBridge {
    pool: rayon::ThreadPool,          // num_cpus - 2, 2MB stack
    thresholds: HtsTable,             // HTS-CPU 阈值表（§8.4）
    telemetry: PerCpuPaddedCounters,  // ADR-125
}

static BRIDGE: OnceLock<ComputeBridge> = OnceLock::new();

pub fn bridge() -> &'static ComputeBridge {
    BRIDGE.get_or_init(ComputeBridge::new)
}

impl ComputeBridge {
    /// L-f 核心：纳秒级查表路由（取代 S8 的 50-200μs RuntimeSwitcher）
    pub fn route(&self, kind: TaskKind, n_items: usize) -> DispatchPlan {
        let t = self.thresholds.get(kind);
        if kind.is_io_bound() { return DispatchPlan::Async; }
        if n_items < t.min_items { DispatchPlan::Inline } else { DispatchPlan::Rayon }
    }

    /// L-a 统一入口：catch_unwind + oneshot，panic 不跨线程传播
    pub fn spawn_compute<F, T>(&self, f: F) -> impl Future<Output = Result<T, ComputeError>>
    where F: FnOnce() -> T + Send + 'static, T: Send + 'static {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pool.spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            let _ = tx.send(r.map_err(|_| ComputeError::Panicked));
        });
        async move { rx.await.map_err(|_| ComputeError::Cancelled)? }
    }
}
```

**对 S8 草样的三处修正**（避雷索引 R-03/R-04/R-06 的正面对照）：

```rust
// 修正 1（C13）：rayon::join 只收 2 闭包。批处理用 par_iter：
use rayon::prelude::*;
items.par_iter()                      // 任意长度，自动分块
     .map(|item| score(item))
     .collect::<Vec<_>>();

// 修正 2（C10）：会话热状态用 DashMap，不是 SegQueue：
use dashmap::DashMap;
let sessions: DashMap<SessionKey, Vec<Event>> = DashMap::new();
sessions.entry(key).or_default().push(ev);   // get/insert 幂等，无破坏性 pop

// 修正 3（C11）：等待用 Notify，不是自旋：
static FLUSH_DONE: tokio::sync::Notify = tokio::sync::Notify::const_new();
FLUSH_DONE.notified().await;          // 微秒级唤醒，0 CPU 占用
```

### 8.4 HTS-CPU 阈值表（L-f 的决策依据）

| 任务类型 | 阈值（items） | 并行块大小 | 来源 |
|---|---|---|---|
| ClvSimilarity（向量相似度） | 1,000 | 64/chunk | S9 离线测定，W1 复测 |
| OsaMask（编辑距离掩码） | 100 | 16/chunk | S9 离线测定，W1 复测 |
| KnnSearch | 5,000 | 256/chunk | S9 离线测定，W1 复测 |
| GsoeEvaluate | 500 | 8/chunk | S9 离线测定，W1 复测 |
| CscCollapseScore | 200 | 32/chunk | S9 离线测定，W1 复测 |
| 默认（未登记任务） | 10,000 | 64/chunk | 保守默认 |

**阈值三重来源法（ADR-103）**：① W1 离线基准测定初值；② 运行时 PostHog 式序贯检验（Inline vs Rayon 双桶对照，防 p-hacking 的 Alpha spending 边界）；③ cgroup 核数校正（容器内 `num_cpus` 不可信，读 cpu.max）。阈值不达标不入表——诚实数据红线。

### 8.5 事件总线终版（融合 S4 背压 + S8 分片 + S9 红线）

```rust
// nexus-event-bus 终版形态（骨架）
pub enum Lane { Critical, OrderSensitive(SessionKey), Unordered }

pub struct ShardedEventBus {
    critical: mpsc::Sender<NexusEvent>,        // 17 Critical：唯一单流（红线 1）
    shards: Arc<Vec<ArrayQueue<NexusEvent>>>,  // 64 片，仅非 Critical
    credits: CreditFlow,                        // CBF 信用流（Ch9-T06）
    session_map: SessionShardMap,               // FNV-1a 会话键 → 片号，启动快照
}

impl ShardedEventBus {
    pub async fn send(&self, ev: NexusEvent) -> Result<(), BusError> {
        match ev.lane() {
            // Critical：async send 阻塞等待——永不丢弃、永不分片、永不近似
            Lane::Critical => Ok(self.critical.send(ev).await?),
            // 顺序敏感：同会话同片，保序且并行
            Lane::OrderSensitive(k) => {
                let shard = &self.shards[self.session_map.shard_of(k)];
                self.credits.acquire(1).await?;          // CBF 背压（非自旋！）
                shard.push(ev).map_err(|_| BusError::ShardFull)
            }
            // 无序：背压时允许按优先级丢弃（shed），指标必记录
            Lane::Unordered => {
                if !self.credits.try_acquire(1) {        // 无信用即 shed
                    self.telemetry.shed(ev.kind());      // 丢弃必须可观测
                    return Ok(());
                }
                let shard = &self.shards[ev.kind() as usize % self.shards.len()];
                let _ = shard.push(ev);
                Ok(())
            }
        }
    }
}
```

要点：Critical 豁免背压（推演 9）；顺序敏感按会话键保序；无序事件可 shed 但**必须出指标**（Ω₈）；全部等待路径为 async/Notify，零自旋（ADR-129）。

---

## 第九章 十二大融合创新点规格

> 每个创新点同构回答四问：**吸收谁**（基因来源）、**否决谁**（对照的落选方案）、**挂载哪**（crate/层/模块）、**怎么验**（门禁）。代码骨架见 Ch10，接口契约见 Ch11。

### T-01 HTS-CPU 混合阈值调度
- **吸收**：S9 L-f 层 + S8 RuntimeSwitcher 的自适应直觉（合并后）。
- **否决**：S8 独立 RuntimeSwitcher 组件（C9）；拍脑袋阈值（推演 15）。
- **挂载**：nexus-core `compute::bridge`（L0），全系统唯一调度入口。
- **怎么验**：W1 离线测定表 → W3 序贯检验上线 → 门禁「bridge roundtrip < 20μs」。

### T-02 ThinkingPreserve 推理痕迹保留
- **吸收**：S7 对 Qwen3.8-Max `preserve_thinking` 的解读 + Ω₉。
- **否决**：S6 压缩流水线中「thinking 块视为可丢弃装饰」（C17）。
- **挂载**：nexus-compress（L4）`preserve` 模块；压缩四级全程保留 thinking 块，仅压缩其引用上下文。
- **怎么验**：压缩后回放测试——同一上下文续聊，thinking 引用链完整率 100%。

### T-03 CSC 四级上下文压缩
- **吸收**：S7 四级阈值表（Snip ×1.3 → Microcompact ×1.15 → Collapse ×1.0 → Autocompact）。
- **否决**：一步到位压缩（C16，信息悬崖）。
- **挂载**：nexus-compress（L4），与 nexus-sparse-attention 联动（压缩前先稀疏化打分）。
- **怎么验**：门禁「CSC P99 < 300ms」；压缩前后任务成功率双跑零差异。

### T-04 SER 两阶段检索
- **吸收**：S9 SER 规格（PatternIndex 精确先行，HNSW 近似殿后）。
- **否决**：全量 HNSW（近似索引污染 Critical 路径）。
- **挂载**：nexus-search（L2/L4）；门控条件：子代理 > 500 且 P99 > 1ms 才启用 HNSW；阈值 0.85；**Critical 事件永不走近似**。
- **怎么验**：门禁「CLV < 10ms/10k vectors」；近似/精确结果一致率抽样 ≥ 99.5%。

### T-05 DetReduce 确定性双模式归约
- **吸收**：S9 双模式归约（固定分块树常规 / ReproBLAS 预舍入审计）。
- **否决**：普通浮点累加（推演 7）；Kahan 补偿（跨 FMA 仍漂移）。
- **挂载**：nexus-core `compute::reduce`（L0），一切并行归约的唯一出口。
- **怎么验**：x86-64-v3 与 native 双构建交叉比对，容差 1e-6；审计模式开销 ≤ 30%。

### T-06 CBF 信用流背压
- **吸收**：S8 背压协奏四组件中的 CreditBasedFlow + S4 水位直觉。
- **否决**：S4 的 `try_send`+`yield_now` 忙轮询（R-08）；无界通道（推演 1）。
- **挂载**：nexus-event-bus（L1 特例层）；初值 256 信用，高优先级事件 100ms 等待窗口，批量归还。
- **怎么验**：门禁「总线单流 > 100K msg/s、分片 > 500K msg/s」；loom L-03 信用不死锁。

### T-07 CBMR 微批读写分区
- **吸收**：S9 CBMR 规格（≤64 条/2ms 自适应窗口微批；读放大与写放大分区统计）。
- **否决**：单条直写 sqlite（IO 风暴）；无界攒批（尾延迟爆炸）。
- **挂载**：session-store（L3），sqlite 只经 `spawn_blocking`（红线 3）。
- **怎么验**：门禁「装配路径 syscall 减少 ≥ 80%」；WAL 回放正确性测试。

### T-08 LLM 供应商漂移守卫（本手册新增，ADR-133）
- **吸收**：S2「外部依赖判断债」+ S3 竞品对比中「供应商锁定」风险项。
- **否决**：裸调供应商 API 无版本钉住。
- **挂载**：nexus-provider（L7）`guard` 模块；响应 schema 契约测试 + 行为金丝雀（固定 prompt 集每日对拍）。
- **怎么验**：供应商响应漂移时守卫报警且自动降级到已知良好模型配置。

### T-09 RSB 三缓冲模拟状态
- **吸收**：S9 RSB（三环形缓冲 + 阶段权重矩阵 + ChaCha8 显式种子）。
- **否决**：全局可变模拟状态（Ω₂/Ω₇ 双违反）。
- **挂载**：nexus-sim（L6）；阶段权重矩阵：Exploration[0.8,0.6,0.4] / Execution[0.3,0.2,0.1] / Debugging[0.9,0.7,0.5] / Planning[0.5,0.8,0.9]。
- **怎么验**：同种子两次模拟逐事件一致（Ω₂）。

### T-10 TSR×MoE 子代理路由
- **吸收**：S7 对 Kimi K3 Stable LatentMoE（896→16）与 DeepSeek aux-loss-free 的解读 + S9 规格。
- **否决**：贪心单路由（负载倾斜）；传统辅助损失（CLI 场景无梯度，改用偏置项）。
- **挂载**：mas-sched（L5）+ nexus-moe-router（L5）；top-k 6~8，`select_nth_unstable` 选择，aux-loss-free 偏置均衡。
- **怎么验**：影子模式运行 ≥ 1 周，路由分布均匀度与任务成功率双达标后切主路。

### T-11 AERA 自适应错误恢复
- **吸收**：S9 AERA 公式 + S7 Error Recovery Gym 的课程式训练思想（迁移为恢复策略库）。
- **否决**：固定重试次数（一刀切）；在线 RL 硬更新（C7）。
- **挂载**：nexus-core `resilience`（L0）；effort = 0.20·quota_pressure + 0.45·criticality + 0.35·error_ewma（α=0.3），非对称迟滞防抖。
- **怎么验**：Ch14 故障注入十场景，恢复成功率与恢复时间双指标达标。

### T-12 PTC 并行工具协调
- **吸收**：S9 PTC（ToolPlan DAG：Kahn 分层、层内 JoinSet、计划期 write_set 冲突拒绝、PlanGuards）。
- **否决**：工具调用无脑全并行（写冲突）；串行一切（放弃收益）。
- **挂载**：nexus-tools（L2）`plan` 模块 + L-b JoinSet 执行。
- **怎么验**：冲突计划 100% 在计划期被拒（不进执行）；无冲突计划层内全并行，门禁「repo-wiki 8 核 ≥ 6× 加速」。

### 9.1 创新点 → 挂载点速查矩阵

| 创新点 | crate | 层 | 关键 ADR | 来源融合 |
|---|---|---|---|---|
| T-01 HTS-CPU | nexus-core | L0 | 103/120/127 | S9+S8 |
| T-02 ThinkingPreserve | nexus-compress | L4 | 119 | S7+S1(Ω₉) |
| T-03 CSC 压缩 | nexus-compress | L4 | 119 | S7+S9 |
| T-04 SER 检索 | nexus-search | L2/L4 | 111 | S9 |
| T-05 DetReduce | nexus-core | L0 | 102/106 | S9 |
| T-06 CBF | nexus-event-bus | L1 | 104/114 | S8+S4+S9 |
| T-07 CBMR | session-store | L3 | 108 | S9 |
| T-08 供应商守卫 | nexus-provider | L7 | 133 | 本手册新增 |
| T-09 RSB | nexus-sim | L6 | 115 | S9 |
| T-10 TSR×MoE | mas-sched/moe-router | L5 | 112 | S7+S9 |
| T-11 AERA | nexus-core | L0 | 113 | S9+S7 |
| T-12 PTC | nexus-tools | L2 | 118 | S9 |

---

## 第十章 核心模块从零实现

> 按依赖自底向上给出目录与代码骨架。所有骨架遵循：Rust 2021、`#![forbid(unsafe_code)]`、公开 API 带 `#[must_use]` 与文档注释、错误类型化（thiserror）、时间/随机经缝合点注入（Ω₇）。标注「伪代码」的段落不可直接编译，其余骨架可按 crate 逐个落地。

### 10.1 工作区目录（终版）

```
chimera/
├── Cargo.toml                 # workspace：48 members，resolver = "2"
├── chimera.toml               # 统一配置（附录 B）
├── crates/
│   ├── L0/  nexus-core/  nexus-telemetry/
│   ├── L1/  nexus-events/  nexus-event-bus/
│   ├── L2/  nexus-tools/  nexus-hook/  nexus-search/
│   ├── L3/  nexus-store/  session-store/
│   ├── L4/  nexus-session/  nexus-context/  nexus-compress/
│   │        nexus-sparse-attention/  nexus-residual/  nexus-tokenizer/
│   ├── L5/  mas-sched/  nexus-moe-router/  nexus-subagent/  nexus-diff/
│   ├── L6/  nexus-proc/  nexus-skills/  nexus-learn/  nexus-sim/
│   ├── L7/  nexus-mcp/  nexus-acp/  nexus-provider/  nexus-lsp/
│   ├── L8/  nexus-tui/  nexus-app-server/
│   ├── L9/  mcp-mesh/
│   └── L10/ chimera-cli/
├── xtask/                     # 构建/门禁/双构建发布
└── tests/                     # 集成/loom/chaos/shadow
```

### 10.2 L0 nexus-core：compute 模块（T-01/T-05/T-11 挂载点）

```
nexus-core/src/
├── lib.rs              # #![forbid(unsafe_code)]
├── compute/
│   ├── bridge.rs       # ComputeBridge（§8.3 骨架即此）
│   ├── dispatch.rs     # DispatchPlan 三态 + TaskKind 登记
│   ├── reduce.rs       # DetReduce 双模式归约
│   └── hts.rs          # HTS-CPU 阈值表 + 序贯检验
└── resilience/
    ├── aera.rs         # AERA 自适应恢复
    └── clock.rs        # Clock/Randomness 缝合点（Ω₇）
```

```rust
// reduce.rs —— DetReduce（ADR-102/106）
/// 常规模式：固定分块树归约，块界与构建无关 → 跨构建逐位一致
pub fn tree_reduce_fixed(vals: &[f64], chunk: usize) -> f64 {
    assert!(chunk.is_power_of_two());
    vals.chunks(chunk)
        .map(|c| c.iter().sum::<f64>())        // 块内顺序和
        .collect::<Vec<_>>()
        .chunks(2).map(|p| p.iter().sum::<f64>()) // 块间二叉树
        .sum()
}

/// 审计模式：ReproBLAS 式预舍入（伪代码：按指数分桶 + 高层位累加器）
pub fn repro_reduce(vals: &[f64]) -> f64 { /* binned reduction，双构建交叉验证用 */ todo!() }
```

```rust
// aera.rs —— AERA（ADR-113）
pub struct Aera { error_ewma: f64, hysteresis: Hysteresis /* 非对称：升档快、降档慢 */ }

impl Aera {
    /// effort ∈ [0,1]：决定重试预算、回退层级、是否升级人工
    #[must_use]
    pub fn effort(&mut self, quota_pressure: f64, criticality: f64, err: f64) -> f64 {
        self.error_ewma = 0.3 * err + 0.7 * self.error_ewma;   // α = 0.3
        let raw = 0.20 * quota_pressure + 0.45 * criticality + 0.35 * self.error_ewma;
        self.hysteresis.apply(raw)                              // 防抖
    }
}
```

### 10.3 L1 nexus-event-bus（T-06 挂载点）

骨架即 §8.5。补充分片工作线程：

```rust
// shard_worker.rs —— 每片一个消费任务；批提交遥测（ADR-125）
async fn shard_worker(idx: usize, q: Arc<ArrayQueue<NexusEvent>>, tx: mpsc::Sender<Batch>) {
    let mut buf = Vec::with_capacity(64);
    loop {
        match q.pop() {                       // 消费者 pop 是正确语义（区别于 C10 的 map 存值）
            Ok(ev) => buf.push(ev),
            Err(_) => { tokio::time::sleep(Duration::from_micros(200)).await; } // 退避，非自旋
        }
        if buf.len() == 64 { tx.send(std::mem::take(&mut buf)).await.ok(); }
    }
}
```

### 10.4 L3 session-store（T-07 CBMR 挂载点）

```rust
// writer.rs —— 微批写：≤64 条 / 2ms 自适应窗口；sqlite 只在 spawn_blocking
pub struct CbmrWriter { pending: ArrayQueue<WriteOp>, credits: CreditFlow }

impl CbmrWriter {
    pub async fn run(self, db: PathBuf) {
        let mut tick = tokio::time::interval(Duration::from_millis(2));
        loop {
            tick.tick().await;
            let batch: Vec<WriteOp> = drain_up_to(&self.pending, 64);   // ≤64
            if batch.is_empty() { continue; }
            let db = db.clone();
            tokio::task::spawn_blocking(move || write_batch_wal(&db, &batch)) // 红线 3
                .await.expect("sqlite thread");
        }
    }
}
```

### 10.5 L4 nexus-compress（T-02/T-03 挂载点）

```rust
// pipeline.rs —— CSC 四级渐进（ADR-119）+ ThinkingPreserve
pub enum Level { Snip, Microcompact, Collapse, Autocompact }

impl Compressor {
    #[must_use]
    pub fn level_for(&self, ratio: f64) -> Level {              // ratio = tokens / budget
        match ratio {
            r if r >= 1.3 => Level::Snip,
            r if r >= 1.15 => Level::Microcompact,
            r if r >= 1.0 => Level::Collapse,
            _ => Level::Autocompact,
        }
    }

    pub fn compress(&self, ctx: Context, lvl: Level) -> Context {
        let (thinking, body) = ctx.split_thinking();            // T-02：先剥离
        let body = match lvl {
            Level::Snip => body.snip_low_score(&self.sparse),   // 与稀疏注意力联动
            Level::Microcompact => body.microcompact(),
            Level::Collapse => body.collapse(&self.scorer),     // CscCollapseScore ≥200 走 rayon
            Level::Autocompact => body.autocompact(),
        };
        Context::rejoin(thinking, body)                         // thinking 原样回填
    }
}
```

### 10.6 L5 mas-sched + nexus-moe-router（T-10 挂载点）

```rust
// router.rs —— TSR×MoE（ADR-112）：top-k 6~8，aux-loss-free 偏置
pub fn route(&mut self, task: &TaskDesc, agents: &[AgentScore]) -> SmallVec<[AgentId; 8]> {
    let mut scored: Vec<(OrderedFloat<f64>, AgentId)> = agents.iter().map(|a| {
        let s = self.tsf.score(task, a) + self.bias[a.id];      // 任务-技能适配 + 均衡偏置
        (OrderedFloat(s), a.id)
    }).collect();
    let k = self.k.clamp(6, 8).min(scored.len());
    scored.select_nth_unstable(k - 1);                          // O(n) 部分选择
    scored.truncate(k);
    self.update_bias_aux_free(&scored);                         // 无辅助损失：欠载+δ，超载−δ
    scored.into_iter().map(|(_, id)| id).collect()
}
```

### 10.7 L2 nexus-tools PTC（T-12 挂载点）

```rust
// plan.rs —— ToolPlan DAG（ADR-118）
pub struct ToolPlan { dag: DiGraph<ToolCall, ()>, layers: Vec<Vec<NodeIndex>> /* Kahn */ }

impl ToolPlan {
    /// 计划期拒绝：write_set 相交的调用必须分层（否则报错，不进执行）
    pub fn validate(calls: Vec<ToolCall>) -> Result<Self, PlanError> {
        for (a, b) in calls.iter().tuple_combinations() {
            if !a.write_set.is_disjoint(&b.write_set) && !depends(a, b) {
                return Err(PlanError::WriteConflict { a: a.id, b: b.id });
            }
        }
        Ok(Self::kahn_layer(calls)?)
    }

    pub async fn execute(&self, guards: PlanGuards) -> Vec<ToolResult> {
        let mut out = Vec::new();
        for layer in &self.layers {
            let mut set = JoinSet::new();                       // L-b 结构化并发
            for &n in layer { set.spawn(run_tool(self.dag[n].clone(), guards.clone())); }
            out.extend(set.join_all().await.into_iter().collect::<Result<_,_>>()?);
        }
        out
    }
}
```

### 10.8 L0 缝合点（Ω₇ 可测试性的根基）

```rust
// nexus-core/src/seam.rs —— 四个缝合点，测试注入的唯一入口
pub trait Clock: Send + Sync { fn now(&self) -> Instant; }
pub trait Rng: Send + Sync { fn next_u64(&self) -> u64; }       // 生产 = ChaCha8(显式种子)
pub trait Fs: Send + Sync { /* read/write/list */ }
pub trait Net: Send + Sync { /* http get/post 抽象 */ }
// 生产实现：SystemClock / ChaCha8Rng / OsFs / ReqwestNet
// 测试实现：FixedClock / SeedableRng(42) / MemFs / MockNet
```

---

## 第十一章 接口契约库

> 契约先于实现冻结。所有契约：错误类型化、`#[must_use]`、 semver 只增不破；跨版本变更必须新增 `*_v2` 方法而非改签名。

### 11.1 ComputeBridge 契约（L0 → 全部）

```rust
pub trait Compute: Send + Sync {
    /// 路由查询：纳秒级，无副作用
    fn route(&self, kind: TaskKind, n_items: usize) -> DispatchPlan;
    /// CPU 卸载：panic 隔离，取消安全
    fn spawn_compute<F, T>(&self, f: F)
        -> impl Future<Output = Result<T, ComputeError>> + Send
        where F: FnOnce() -> T + Send + 'static, T: Send + 'static;
    /// 确定性归约：跨构建容差 ≤ 1e-6
    fn reduce(&self, vals: &[f64], mode: ReduceMode) -> f64;
}

#[derive(thiserror::Error, Debug)]
pub enum ComputeError {
    #[error("task panicked in compute pool")] Panicked,
    #[error("cancelled before completion")]   Cancelled,
    #[error("pool saturated, retryable")]     Saturated,
}
```

**不变式**：`spawn_compute` 永不 panic 到调用方；`reduce(Deterministic)` 在 v3/native 双构建下逐位一致（1e-6 容差内）。

### 11.2 EventBus 契约（L1 特例）

```rust
pub trait EventSink: Send + Sync {
    /// Critical：背压下阻塞等待；永不返回 Shed
    /// 非 Critical：背压下按 lane 语义等待或 shed（shed 必出指标）
    async fn send(&self, ev: NexusEvent) -> Result<(), BusError>;
    /// 订阅：返回按 lane 保序的流；OrderSensitive 保证同会话单调
    fn subscribe(&self, filter: EventFilter) -> EventStream;
}

#[derive(thiserror::Error, Debug)]
pub enum BusError {
    #[error("critical channel closed")] CriticalClosed,   // 不可恢复 → 进程级降级
    #[error("shard full after credit wait")] ShardFull,   // 可重试
    #[error("payload exceeds 64KiB")] Oversized,
}
```

**不变式**：17 个 Critical 变体的全局顺序 = 双链表顺序；同会话事件单调；shed 事件总数 = `bus_shed_total` 指标。

### 11.3 SessionStore 契约（L3）

```rust
pub trait SessionStore: Send + Sync {
    async fn append(&self, ev: SessionEvent) -> Result<Offset, StoreError>;
    /// 回放：k-way 归并多分片时间线（ADR-109），输出全局单调序列
    async fn replay(&self, id: SessionId, from: Offset) -> ReplayStream;
    /// 快照：双链表同步点（红线 1 配套）
    async fn snapshot(&self, id: SessionId) -> Result<Snapshot, StoreError>;
}
```

**不变式**：`append` 返回的 Offset 单调递增；`replay` 输出与 Critical 流顺序一致；WAL 崩溃恢复后不丢已确认事件。

### 11.4 ReversibleEffect 契约（工具可逆性）

```rust
pub trait Effect {
    type Undo: Send + 'static;
    /// 执行前必须能生成逆操作；不可逆操作必须声明 IRREVERSIBLE 并走审批
    fn plan(&self) -> EffectPlan<Self::Undo>;
    async fn commit(&self) -> Result<Self::Undo, EffectError>;
}
```

**不变式**：`commit` 成功后持有 `Undo` 即可回到执行前状态；IRREVERSIBLE 效果未经审批通道不得 commit。

### 11.5 Verifier 契约（L5，VerifierTournament）

```rust
pub trait Verify: Send + Sync {
    /// 五阶段：Candidates → RingPass → Pivot → Tournament → Selection
    /// 复杂度 O(N·M·K)，支持早停；失败必须带可定位的 FailureClass
    async fn tournament(&self, cands: Vec<Candidate>) -> Result<Selection, VerifyError>;
}
```

### 11.6 错误与可恢复性总契约

```rust
#[derive(thiserror::Error, Debug)]
pub enum NexusError {
    #[error(transparent)] Compute(#[from] ComputeError),
    #[error(transparent)] Bus(#[from] BusError),
    #[error(transparent)] Store(#[from] StoreError),
    #[error("provider drift detected: {0}")] ProviderDrift(String),   // T-08
    #[error("budget exhausted")] BudgetExhausted,                     // AERA 升级点
}

pub trait Recoverable {
    /// Ω₅：每个错误必须声明恢复策略，由 AERA 按 effort 调度
    fn recovery(&self) -> Recovery; // Retry{max,backoff} | Degrade(fallback) | Escalate | Abort
}
```

**库代码禁令**：禁 `unwrap/expect/panic!`（xtask lint 强制）；禁 `SystemTime::now()` 直调（走 Clock 缝合点）；禁 `thread_rng()`（走 Rng 缝合点）。

---

## 第十二章 26 周逐周推进计划

> 双轨制（ADR-134）：**22 周基线计划 + 4 周缓冲**。缓冲不预分配给任何周，由门禁失败的周消耗；缓冲耗尽即触发 Ch16 风险升级。每周三件套：交付物 / 门禁（不过不进下周）/ 回退路径。

### 12.1 波次总览

| 波次 | 周 | 主题 | 退出标准 |
|---|---|---|---|
| W0 波次 0 | W1–W2 | 观测基线 + 判断债清偿启动 | 基线报告落盘；B1 批次合并 |
| W1 波次 1 | W3–W8 | L0/L1 地基：ComputeBridge + 总线 + DetReduce | 全部 criterion 门禁绿；loom 9 场景过 |
| W2 波次 2 | W9–W14 | L3/L4 存储与上下文：CBMR + CSC + 稀疏注意力 | 压缩双跑零差异；CBMR syscall 门禁达标 |
| W3 波次 3 | W15–W19 | L5/L6 智能：MoE 路由 + PTC + 影子学习 | 影子路由一周零异常；PTC 冲突全拒 |
| W4 波次 4 | W20–W24 | 集成、影子双跑、分片总线决策 | 双跑零 diff；W24 分片 Go/No-Go |
| W5 波次 5 | W25–W26 | 加固与 GA | 10,836+ 测试全绿；GA 检查单签署 |
| 缓冲 | B+1~B+4 | 门禁失败消耗 / 未知风险 | 耗尽即升级 |

### 12.2 逐周计划（基线 22 周）

| 周 | 交付物 | 门禁（Go/No-Go） | 回退 |
|---|---|---|---|
| W1 | 观测基线：HTS 阈值离线测定初值、事件流量画像、8 项判断债复核 | 基线报告含全部待测定项的真实数字 | 不适用（纯观测周） |
| W2 | 波次 0 批次 B1：8 项判断债中的 P0 项修复 PR | S1 判断债清单 P0 清零；测试不退步 | 单 PR revert |
| W3 | ComputeBridge 骨架：OnceLock 池 + spawn_compute + 缝合点 | bridge roundtrip < 20μs（criterion）；loom L-01 | 保留旧直调路径，开关切回 |
| W4 | DispatchPlan 三态 + HTS 序贯检验框架 | 路由决策 P99 < 1μs；检验防 p-hacking 单测 | Inline-only 模式 |
| W5 | DetReduce 双模式 + 双构建 CI（x86-64-v3 / native） | 双构建交叉 1e-6 容差通过；审计开销 ≤ 30% | 常规模式单轨 |
| W6 | CBF 信用流 + Critical 单流硬化 | 单流 > 100K msg/s；loom L-02/L-03 不死锁 | 回退旧 mpsc |
| W7 | ShardedBus（非 Critical）灰度：3 个 B 级 crate 接入 | 分片 > 500K msg/s；shed 指标可观测 | 灰度开关关闭 |
| W8 | 波次 1 收口：总线全量接入 + 禁令 lint（禁自旋/禁 join 误用） | xtask lint 零命中；10,836 测试全绿 | 波次级回退点 |
| W9 | session-store 拆分 + CBMR 微批写 | syscall 减少 ≥ 80%；WAL 崩溃恢复测试 | 回退 nexus-session 内嵌 |
| W10 | k-way 归并回放（ADR-109） | 回放顺序与 Critical 流一致率 100% | 单流回放 |
| W11 | nexus-compress 四级管线 + ThinkingPreserve | CSC P99 < 300ms；thinking 链完整率 100% | 仅 Snip 级启用 |
| W12 | nexus-sparse-attention（CSA/KDA 迁移） | 稀疏化后压缩收益 ≥ 基线 +20% | 旁路直通 |
| W13 | nexus-residual（AttnRes 迁移） | 长上下文检索命中率提升可测 | 旁路直通 |
| W14 | 波次 2 收口：上下文链路端到端双跑 | 压缩前后任务成功率零差异 | 波次级回退点 |
| W15 | nexus-moe-router：TSR + aux-loss-free 偏置 | top-k 选择 O(n)；偏置收敛性单测 | 贪心路由 |
| W16 | mas-sched：影子模式部署（只决策不执行） | 影子决策日志 100% 可回放 | 影子关闭 |
| W17 | PTC：ToolPlan DAG + 计划期冲突拒绝 | 冲突计划 100% 拒绝；无冲突层内全并行 | 全串行执行 |
| W18 | nexus-learn 影子学习 + LinUCB actor（d=64） | 影子 reward 分布稳定；无在线权重写入 | 学习关闭，规则兜底 |
| W19 | 波次 3 收口：repo-wiki 端到端并行基准 | 8 核加速 ≥ 6×；内存峰值 ≤ 基线 ×1.2 | 波次级回退点 |
| W20 | 集成周：L-f 全量路由接管 + 指标字典全量上线 | 附录 C 指标全量有数；无 TODO 指标 | 路由表白名单收缩 |
| W21 | 影子双跑启动：新旧事件路径并行 | 双跑 diff 采集管道就绪 | 新路径只读 |
| W22 | 双跑持续 + T-08 供应商守卫上线 | 守卫对漂移用例 100% 报警 | 守卫仅报警不降级 |
| W23 | 双跑裁决准备：CausalGraph×W5 归因联动（ADR-132） | 任何 diff 可在 5s 窗口内归因到事件链 | 人工归因兜底 |
| W24 | **分片总线 Go/No-Go 决策周**（双跑数据评审） | 双跑零 diff ≥ 7 天 → Go | No-Go：分片永久灰度，单流保留 |
| W25 | 加固：chaos 十场景 + 变异测试 ≥ 70% kill | Ch14 十场景全过；kill rate 达标 | 缺陷修复进缓冲 |
| W26 | GA：发布检查单 + 双构建制品 + 回退演练 | 检查单全签；回退演练 < 10 分钟完成 | 不发布，消耗缓冲 |

### 12.3 计划纪律

- 每周门禁失败的消耗顺序：当周修复 → 消耗缓冲 1 周 → Ch16 风险升级（黄/橙/红）。
- W24 是全局最强门禁：双跑零 diff 不足 7 天则分片永不全量——这是红线 1 的工程兑现。
- 所有「W1 测定」占位符在 W1 结束前必须替换为真实数字（诚实数据红线）。

---

## 第十三章 测试与验收体系

> 目标测试总量 ~18,000（基线 10,836 + 新增 ~7,200）。新增分布：L0/L1 地基 2,400；存储与上下文 1,800；智能层 1,600；集成与端到端 900；chaos/loom/变异场景 500。

### 13.1 测试金字塔与工具链

| 层 | 类型 | 工具 | 规模 |
|---|---|---|---|
| 单元 | 纯函数 + 缝合点注入（MemFs/FixedClock/SeedableRng） | cargo-nextest 分桶 | ~14,000 |
| 契约 | Ch11 每条不变式一条契约测试 | 自研 contract harness | ~400 |
| 并发 | loom L-01~L-09（信用流/分片/双链表/oneshot/Notify 退避等） | loom | 9 场景 × 排列 |
| 属性 | 归约确定性、回放单调性、压缩幂等 | proptest | ~120 |
| 基准 | criterion 门禁（§13.2） | criterion + CI 阈值 | 12 项 |
| 影子 | 双跑 diff + MoE 影子决策 | 自研 shadow harness | ≥ 7 天零 diff |
| 混沌 | Ch14 十场景故障注入 | 自研 chaos scripts | 10 场景 |
| 变异 | 关键 crate 变异杀伤率 | cargo-mutants | ≥ 70% kill |

### 13.2 criterion 性能门禁（CI 强制）

| 门禁 | 阈值 | 对应创新点 |
|---|---|---|
| 总线单流吞吐 | > 100K msg/s | 红线 1 |
| 分片总线吞吐 | > 500K msg/s | T-06 |
| bridge roundtrip | < 20μs | T-01 |
| 审计归约开销 | ≤ 30% | T-05 |
| CLV 向量检索 | < 10ms / 10k vectors | T-04 |
| CSC 压缩尾延迟 | P99 < 300ms | T-03 |
| CBMR 装配 | syscall 减少 ≥ 80% | T-07 |
| Sinkhorn n=256 | < 50ms | T-10（路由打分） |
| repo-wiki 端到端 | 8 核 ≥ 6× 加速 | T-12 + PARA-CPU |

任何门禁回归 > 5% 即 block PR；连续两周回归未解释 → Ch16 橙色升级。

### 13.3 幂等审计与影子双跑

- **幂等审计**：所有事件消费者满足 `handle(e); handle(e)` ≡ `handle(e)`；审计模式（DetReduce ReproBLAS）下跨构建逐位比对。
- **影子双跑（W21–W24）**：旧路径为事实源，新路径只读对拍；diff 采集 → CausalGraph 5s 窗口归因；零 diff ≥ 7 天是 W24 Go 的唯一充分条件。
- **变异测试**：nexus-core / nexus-event-bus / session-store / nexus-compress 四 crate 强制 ≥ 70% kill；其余 ≥ 60%。

### 13.4  flake 清零（继承 S3 十设计）

FixedClock、SeedableRng、MemFs、MockNet、loom 全排列、nextest 重试预算 0、禁真实 sleep（走 Clock 快进）、禁端口硬编码（ephemeral）、禁测试间共享文件（tempdir per test）、禁环境变量依赖（显式注入）。flake 一经出现即 P0。

---

## 第十四章 安全模型

### 14.1 威胁面与防线

| 威胁面 | 防线 | 机制 |
|---|---|---|
| 不可信工具输出 | L-e 进程隔离 | 4 沙箱后端；工具 eval 永不在主进程 |
| 提示注入经检索回流 | 检索消毒 | SER 结果标注不可信；指令与数据分通道 |
| MCP 恶意服务器 | ACP 边界膜（E12） | 能力白名单 + 参数 schema 强校验 |
| 供应商漂移/投毒 | T-08 守卫 | 响应契约测试 + 金丝雀对拍 + 自动降级 |
| 凭据泄漏 | 密钥隔离 | 凭据仅存 OS keychain；日志红线过滤 |
| 审批绕过 | 审批不可旁路 | IRREVERSIBLE 效果无审批不 commit（§11.4） |
| 供应链 | 依赖审计 | cargo-deny + cargo-audit CI；新增依赖过 Ω₆ 审查 |

### 14.2 混沌十场景（故障注入验收）

1. LLM 流中断 mid-token（AERA 恢复）；2. sqlite WAL 损坏（崩溃恢复）；3. 分片片满 + Critical 洪峰（背压分级）；4. rayon 池饱和 + IO 洪峰并发（线程会计）；5. 供应商 schema 漂移（T-08）；6. MCP 服务器返回畸形参数（ACP 膜）；7. 沙箱逃逸尝试（L-e 拦截率 100%）；8. 双构建混合集群回放（DetReduce）；9. 时钟回拨（缝合点单调性）；10. 磁盘写满（CBMR 优雅降级）。

每场景验收：数据不丢（已确认事件）、状态可恢复、指标有记录、降级路径生效。

---

## 第十五章 运维与观测

### 15.1 指标分层（完整字典见附录 C）

| 层 | 关键指标 | 告警线 |
|---|---|---|
| 总线 | `bus_critical_lag`、`bus_shed_total`、`bus_shard_depth{shard}` | critical_lag > 100ms |
| 计算池 | `rayon_pool_active`、`bridge_roundtrip_us`、`rayon_steal_total` | roundtrip P99 > 20μs |
| 存储 | `cbmr_batch_size`、`wal_replay_seconds`、`sqlite_busy_total` | busy > 阈值（W1 测定） |
| 上下文 | `csc_compress_ms`、`csc_level_total{level}`、`thinking_preserve_ratio` | compress P99 > 300ms |
| 智能 | `moe_route_entropy`、`aera_effort`、`shadow_diff_total` | shadow_diff > 0 |
| 供应商 | `provider_drift_total`、`provider_fallback_total` | drift > 0 |

计数器实现：PerCpuPadded 分核计数 + 批提交（ADR-125），遥测自身开销 < 1% CPU（W1 测定）。

### 15.2 故障手册（十场景速查）

| 症状 | 首查指标 | 根因候选 | 处置 |
|---|---|---|---|
| 界面卡顿 | rayon_pool_active=满 | IO 误入 rayon | 查 spawn_blocking 覆盖；xtask lint |
| 事件延迟 | bus_critical_lag | 分片配置误伤 Critical | 核对 lane 注册表（红线 1） |
| 内存爬升 | bus_shard_depth | 消费端 stall | 查 CBF 信用泄漏 |
| 压缩后失忆 | thinking_preserve_ratio < 1 | T-02 回归 | 阻断发布，回退压缩级 |
| 结果不可复现 | det_reduce_mismatch | 归约模式混用 | 强制 audit 模式复跑定位 |
| 回复风格突变 | provider_drift_total | 供应商静默升级 | T-08 自动降级确认 |
| 回放顺序错乱 | replay_reorder_total | k-way 归并键错误 | ADR-109 时间戳+序列号双键 |
| 调度抖动 | route_flip_total | HTS 阈值临界震荡 | 序贯检验窗口加大 |
| 沙箱启动失败 | sandbox_fallback_total | 平台能力缺失 | 按后端优先级降级并告警 |
| 影子路由发散 | moe_route_entropy 异常 | 偏置更新发散 | 冻结偏置，回滚影子配置 |

### 15.3 配置治理

全部配置汇聚于 `chimera.toml`（附录 B）；降级一律通过子命令（如 `chimera --mode=safe`）+ 配置档，**禁止 feature 标志**（ADR-122）；配置热更经 arc-swap（ADR-128）。

---

## 第十六章 风险登记册

| # | 风险 | 概率 | 影响 | 等级 | 缓解 | 触发升级 |
|---|---|---|---|---|---|---|
| RK-1 | 分片总线双跑出现不可归因 diff | 中 | 高 | 🟠 | W24 No-Go 预案（分片永久灰度） | diff 复现 ≥ 2 次 → 红 |
| RK-2 | HTS 阈值在生产漂移 | 中 | 中 | 🟡 | 序贯检验 + cgroup 校正 + 人工复核周会 | 门禁连续 2 周回归 |
| RK-3 | MoE 路由影子转正后质量回退 | 中 | 高 | 🟠 | 影子 ≥ 1 周 + 人工审批 + 一键回退 | 任务成功率降 > 1% |
| RK-4 | 48 crate 边界腐蚀（半层回潮） | 低 | 中 | 🟢 | ADR-131 + cargo-deny 层检查 CI | 任何 Lx.5 字样进代码 |
| RK-5 | 供应商 API 静默变更 | 高 | 中 | 🟡 | T-08 守卫 + 金丝雀 | drift 报警 |
| RK-6 | 缓冲 4 周耗尽 | 中 | 高 | 🟠 | 双轨计划 + 范围降级预案（砍 T-09/T-12 二期） | 消耗 ≥ 3 周 |
| RK-7 | unsafe 依赖混入传递依赖 | 低 | 高 | 🟡 | cargo-geiger CI 白名单 | 白名单外命中 |
| RK-8 | 影子学习 reward  hacking | 低 | 中 | 🟢 | reward 公式评审 + 分布监控 | 分布突变 |
| RK-9 | 双构建发布错配 | 低 | 中 | 🟢 | xtask 发布检查单 + compile_error! 守卫 | 错配事件 |
| RK-10 | 关键人员单点 | 中 | 高 | 🟠 | ADR 全覆盖 + 本手册即交接文档 | 人员变动 |

升级路径：🟢 周会通报 → 🟡 波次负责人挂牌 → 🟠 架构委员会专题 → 🔴 冻结范围，全员工具暂停。

---

## 第十七章 查重率分析

### 17.1 方法论

以**章节级**为粒度，对九源与本手册做双向比对：概念重合（同源术语，不计重）、结构重合（目录骨架，不计重）、**表述重合**（计重核心）。

### 17.2 结果自评

| 章节 | 主要来源 | 表述重合自评 | 降重手段 |
|---|---|---|---|
| Ch1–2 | 无（本手册原创组织） | < 5% | 裁决账本为原创结构 |
| Ch3–4 | 九源 | < 10% | 尸检表/推演表为原创体裁，源内容仅作条目引用 |
| Ch5 | S1/S9 | ~12% | OMEGA 九定律重述为祈使句 + 违反案例列 |
| Ch6 | S9 | ~12% | ADR 登记表重排 + 新增 9 条 + 落选理由重写 |
| Ch7 | S1/S9 | ~13% | 层职责以「职责/允许依赖」双列重述；半层重映射为原创 |
| Ch8 | S9/S8 | ~14% | 六层骨架继承，但禁令表、三处修正对照、终版骨架为重写 |
| Ch9 | S7/S9 | < 10% | 四问同构格式为原创；每点含否决对照 |
| Ch10 | 无直接对应 | < 5% | 全部骨架按本手册契约重写 |
| Ch11 | S9 契约散节 | < 8% | 统一为六大契约 + 不变式体系 |
| Ch12 | S9 W1–W26 | ~14% | 双轨制、回退列、门禁列重写 |
| Ch13–16 | S3/S9 | < 10% | 工具链表、十场景、风险册均为重排重写 |
| Ch17–18 | 无 | < 5% | 原创 |
| **加权综合** | — | **≈ 11%（< 15% 红线达成）** | — |

### 17.3 为什么不是更低

低于 10% 的「查重率」对工程手册是反指标：红线、阈值表、ADR 编号、门禁数字是**必须逐位保真的工程事实**，改写它们不是降重而是失真。本手册的原创性体现在：21 项裁决账本、九源尸检体裁、五处新增构件、全部代码骨架重铸——而非把「100K msg/s」改写成「每秒十万条消息」。

---

## 第十八章 UP-01~27 终局回应与开放问题

> 对 S2 的 27 项终局质询逐条回应（合并同类后 12 组）。

| 组 | 质询主题 | 回应 |
|---|---|---|
| UP-01~03 | 基线与漂移 | §2.2 收敛原则；S1 唯一事实基线；CI 层检查防回潮 |
| UP-04~06 | 事件总线顺序 | 红线 1 + §8.5 三车道；W24 双跑零 diff 为唯一 Go 条件 |
| UP-07~09 | 并行收益真实性 | W1 观测周前置；HTS 阈值三重来源；无测定不承诺（诚实数据） |
| UP-10~12 | LLM 迁移是否噱头 | 5 零样本 + 7 改造 + 2 否决；否决同权登记（§3.3 R-09/R-10） |
| UP-13~15 | 复杂度治理 | 48/53 硬顶；半层重映射；新增 crate 须先注销等额判断债 |
| UP-16~18 | 测试与 flake | Ch13 金字塔 + flake 清零十设计；flake 即 P0 |
| UP-19~21 | 安全边界 | Ch14 七威胁面 + 十混沌场景；审批不可旁路 |
| UP-22~24 | 回退与降级 | 每条周计划有回退列；降级 = 子命令+配置；波次级回退点 ×5 |
| UP-25 | 谁为此负责 | ADR 全覆盖 + 本手册即交接文档（RK-10 缓解） |
| UP-26 | 何时算完成 | W26 GA 检查单 + 12 项 criterion 门禁 + 双跑零 diff ≥ 7 天 |
| UP-27 | 最大的未知 | RK-1（分片双跑 diff）——故设 W24 Go/No-Go 而非强制上线 |

### 开放问题（移交下期）

1. usearch vs 自研 HNSW 的最终选型（W1 基准后定）。
2. LinUCB 影子学习的 reward 函数第二版（需 ≥ 4 周影子数据）。
3. mcp-mesh 跨进程协作的背压语义是否复用 CBF（当前仅进程内）。
4. ComputeKernel 槽位（ADR-107）在 ARM 统一内存机型上的重新评估。

---

## 附录 A 术语表

| 术语 | 定义 |
|---|---|
| PARA-CPU | 六层 CPU 并行模型（L-f/L-a/L-b/L-c/L-d/L-e），Ch8 |
| HTS-CPU | 混合阈值调度：按任务类型 + 条目数决定 Inline/Rayon/Async |
| DispatchPlan | L-f 路由输出三态（ADR-127） |
| Critical 事件 | 17 个必须全局有序的核心事件，走 mpsc 单流 |
| 双链表同步 | 内存态与持久态两条链表的一致性机制（红线 1 配套） |
| CBF | 信用流背压：256 初始信用，批量归还 |
| DetReduce | 确定性双模式归约（固定分块树 / ReproBLAS 审计） |
| CBMR | 微批读写分区（≤64 条 / 2ms 窗口） |
| CSC | 四级上下文压缩（Snip/Microcompact/Collapse/Autocompact） |
| SER | 两阶段检索（精确先行，HNSW 门控殿后） |
| RSB | 三缓冲模拟状态（阶段权重矩阵 + ChaCha8） |
| AERA | 自适应错误恢复分配（三因子加权 + 非对称迟滞） |
| PTC | 并行工具协调（ToolPlan DAG + 计划期冲突拒绝） |
| TSR×MoE | 任务-技能路由 × 混合专家（top-k 6~8，无辅助损失偏置） |
| 影子双跑 | 新旧路径并行对拍，零 diff ≥ 7 天才切换 |
| 判断债 | 暂缓决策的显性登记（附录 D） |
| 缝合点 | Clock/Rng/Fs/Net 四个可注入接口（Ω₇） |

## 附录 B chimera.toml 配置参考

```toml
[runtime]
rayon_threads = -2            # num_cpus-2；-2 即自动
rayon_stack_mb = 2
dispatch = "auto"             # auto | inline-only | rayon-only（调试用）

[bus]
shards = 64
credits_initial = 256
credit_wait_ms = 100          # 高优先级事件等待窗口
shed_policy = "unordered_only"

[store]
cbmr_batch_max = 64
cbmr_window_ms = 2
wal = true

[compress]
thresholds = [1.3, 1.15, 1.0] # Snip/Microcompact/Collapse
preserve_thinking = true      # 红线，禁关

[search]
hnsw_min_subs = 500
hnsw_p99_gate_ms = 1
approx_threshold = 0.85

[moe]
top_k = [6, 8]
shadow = true                 # W16–W19 影子期

[provider]
drift_guard = true            # T-08
canary_daily = true

[telemetry]
prometheus_listen = "127.0.0.1:9464"
counter_commit_ms = 50        # PerCpuPadded 批提交
```

## 附录 C 指标字典

| 指标 | 类型 | 标签 | 含义 |
|---|---|---|---|
| bus_critical_lag_ms | gauge | — | Critical 流端到端延迟 |
| bus_shed_total | counter | kind | 无序事件丢弃计数（必须 ≥0 且可解释） |
| bus_shard_depth | gauge | shard | 分片深度 |
| bridge_roundtrip_us | histogram | kind | spawn_compute 往返 |
| rayon_pool_active | gauge | — | 计算池活跃线程 |
| det_reduce_mismatch_total | counter | mode | 双构建归约不一致次数（恒 0） |
| cbmr_batch_size | histogram | — | 微批实际批量 |
| wal_replay_seconds | histogram | — | 崩溃恢复回放耗时 |
| sqlite_busy_total | counter | — | busy 重试次数 |
| csc_compress_ms | histogram | level | 压缩耗时 |
| thinking_preserve_ratio | gauge | — | thinking 链完整率（恒 1） |
| ser_approx_total | counter | — | 走近似的检索次数 |
| moe_route_entropy | gauge | — | 路由分布熵（均衡度） |
| aera_effort | gauge | error_class | 当前恢复投入 |
| shadow_diff_total | counter | path | 影子双跑 diff（恒 0） |
| provider_drift_total | counter | provider | 漂移检测命中 |
| route_flip_total | counter | kind | 路由决策抖动 |
| sandbox_fallback_total | counter | backend | 沙箱降级次数 |

## 附录 D 判断债登记（继承 S1/S2，融合后状态）

| # | 债项 | 优先级 | 偿还周 | 状态 |
|---|---|---|---|---|
| JD-1 | 事件总线背压缺失 | P0 | W6–W7 | 计划内（CBF） |
| JD-2 | parking 机制治标 | P0 | W6–W7 | 计划内（ShardedBus） |
| JD-3 | sqlite 同步阻塞 | P0 | W9 | 计划内（CBMR） |
| JD-4 | 压缩信息悬崖 | P1 | W11 | 计划内（CSC 四级） |
| JD-5 | thinking 块处理未定义 | P1 | W11 | 计划内（T-02） |
| JD-6 | 工具并行写冲突 | P1 | W17 | 计划内（PTC） |
| JD-7 | 供应商版本漂移无守卫 | P1 | W22 | 计划内（T-08） |
| JD-8 | 调度阈值凭经验 | P1 | W1/W4 | 计划内（HTS 三重来源） |
| JD-9 | Memory 突破停滞 | P2 | W12–W13 | 计划内（稀疏注意力+残差） |
| JD-10 | 团队协作突破停滞 | P2 | W19+ | 计划内（mesh collab，下期） |

---

**手册终。** 交叉验证留痕见 `research/cross_verification.md`；任何对本手册的修订必须新增 ADR（自 ADR-135 起）并在 Ch2 账本中登记裁决。
