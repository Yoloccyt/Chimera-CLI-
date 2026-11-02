# ADR-034: 灰度 = 能力场 + 编译期 feature(否决运行时 Feature Flag)

## 状态

已批准 (Accepted) (2026-07-23)


> **实施现状核对（2026-09-22，本块只登记事实，不改写上方决策）**
> - ✅ 决策本体成立：编译期 feature 路线在盘——`crates/chimera-tui/Cargo.toml` 声明 `default`/`v3-engine`，
>   `crates/chimera-cli/Cargo.toml` 声明 `mca`/`rl-client`/`r2_unfreeze`/`r2_island_repayment`
>   （tomllib 结构解析 + 正则双源一致；全仓 43 份 manifest 中仅 7 份声明 features，共 10 个唯一特征名）。
> - ❌ **「受影响面」第 3 项未落地**：`release.yml` 的 feature 双组合矩阵（`--features v3-engine` 与默认）**不存在**——
>   `.github/workflows/release.yml` 全文对 `features`/`v3-engine` **0 命中**；全部 workflow 里 `--features` 只出现在
>   `ci.yml:395-443` 的独立 `--features mca` job（`cargo check --workspace --all-targets --features mca` +
>   `cargo nextest run --workspace --features mca`）。`bench_check.yml:13,168` 提到 v3-engine 仅是**基准注释**，不是特性矩阵。
>   ⇒ 该项状态 = **待实现**（是否补做属 CI 变更，需授权）。
> - 🔎 免编译校验法（本块据此判定，可复用）：`cargo tree -p <pkg> --features <f>`；
>   实测 `cargo tree -p chimera-cli --features mca` → **rc=0**；负控 `--features frobnicate-xyz` →
>   **rc=101** `error: the package 'chimera-cli' does not contain this feature: frobnicate-xyz`。

## 背景

- 项目当前处于 v2.3.1-omega(第三阶段模块级系统性优化,35 crate 全覆盖),`NEXUS-OMEGA_v5.0_系统性完整设计文档.md` 为下一里程碑(v3.0.0-omega)设计蓝图。v5.0 §7 引入学习系统(omega-learner + RHI-CG 双通道进化回路),其核心诉求是"新学习策略需灰度部署,表现好才升权"。
- 学习系统的灰度需求与项目 §6.1 架构红线 5("禁止功能标志,用能力场自然进化替代",源自 Claude Code 44 个未发布标志教训)存在表面冲突:灰度天然要求"按开关切换路径",而红线禁止散落的运行时布尔旗。
- v5.0 §7.1(对应设计文档第 321-333 行)以 **C4 决策**裁决该冲突:**运行时灰度走 decay-engine 能力场**(红线提倡的"能力场自然进化"正路),**迁移期双轨走 Cargo 编译期 feature**(ADR-029 `v3-engine` 先例),**学习失效回退走策略枚举双实现**(`SelectorPolicy::Static` / `Learned`),三方共同排除运行时 Feature Flag。
- 既有先例一:`chimera-tui` 的 [Cargo.toml:10-12](file:///D:/Chimera%20CLI/crates/chimera-tui/Cargo.toml#L10) `v3-engine` feature 默认 off,注释明确"自研引擎与现有 ratatui 路径双轨并存,逐面板迁移,每个里程碑可 cargo test --workspace 全绿、随时回滚"——编译期 feature 模式已有落地先例(ADR-029)。
- 既有先例二:`decay-engine` 已实现 ADR-002 连续权限流体模型:[types.rs:22](file:///D:/Chimera%20CLI/crates/decay-engine/src/types.rs#L22) 的 `CapabilityLevel(f32)` newtype 校验范围 [0.0, 1.0],[types.rs:54](file:///D:/Chimera%20CLI/crates/decay-engine/src/types.rs#L54) 的 `Capability` 结构体(id/name/level/frozen/last_decay_at),[types.rs:107](file:///D:/Chimera%20CLI/crates/decay-engine/src/types.rs#L107) 的 `DecayEvent::ViolationPenalty` 双驱动衰减(时间驱动 + 事件驱动)。能力场灰度可复用此既有衰减闭环。
- 既有先例三:§6.2 红线"Critical 安全事件用 mpsc"与 `AsaIntervention` 事件——学习策略若引发安全干预,可经既有事件链触发能力衰减(安全闭环)。
- v5.0 §7.3(第 368-381 行)定义六接缝(S1-S6)灰度锚点,均需走本 ADR 裁定的能力场路径;§7.4(第 394-395 行)"部署灰度 = decay-engine 能力场,新 spec 初始低能力,随成功率提升授权"。
- 本 ADR 记录将 C4 裁决落地为 5 项工程决策,作为后续 spec(`capability-field-gradual-rollout`)实现的权威依据,并明确否决运行时 Feature Flag 与硬编码无灰度两套替代方案。

> **ADR 编号确认**: v5.0 设计文档 §11.2(第 680 行)已将"灰度 = 能力场 + 编译期 feature"预占编号 ADR-034 并标注为"提议"状态。本 ADR 将其状态从"提议"升格为"已批准",编号保持 ADR-034。引用的 ADR-031(Harness-as-Spec + omega-learner 边界)、ADR-032(双通道评估器)在 v5.0 §11.2 同为"提议"状态,尚未建立为独立文档——本 ADR 引用其**概念定义**(六接缝、双通道验证器),不假设其已落地,待各自建档时与本 ADR 双向校验。

## 决策

经专家团队(E01 首席架构师 + E05 生产系统专家 + E07 任务调度专家)多轮结构化思考与多路径交叉验证,对灰度策略作出以下 5 项架构决策:

### 决策 1: 否决运行时 Feature Flag — 三态分离排除散落布尔旗

明确否决以 `if feature_x_enabled { ... } else { ... }` 形态控制代码路径的运行时 Feature Flag,具体禁止三类反模式:

1. **禁止环境变量控路径**: 不允许通过 `ENV_VAR=on/off` 切换代码分支(运行时旗)。
2. **禁止配置文件控路径**: 不允许通过 `config.yaml` 的布尔字段决定走哪条代码路径(运行时旗)。
3. **禁止数据库字段控路径**: 不允许通过 DB 布尔列控制二进制内代码分支(运行时旗)。

灰度需求按其**语义类型三态分离**,分别走决策 2/3/4 的正路,不混用:

| 灰度语义 | 时间尺度 | 走向 | 对应决策 |
|---------|---------|------|---------|
| 学习策略渐进授权 | 运行时(秒~分钟级) | decay-engine 能力场(CapabilityToken) | 决策 2 |
| 大版本迁移期双轨 | 编译时(版本级) | Cargo 编译期 feature | 决策 3 |
| 学习失效回退 | 运行时(瞬时降级) | 策略枚举双实现 fallback | 决策 4 |

现存运行时旗(如有遗留)在 P4 阶段(W13-16 学习层落地期)统一清理,清理清单入配套 spec 的 tasks。

### 决策 2: 能力场灰度(CapabilityToken 机制)— 复用 decay-engine 衰减闭环

在 `crates/decay-engine/src/` 扩展 `CapabilityToken` 机制承载学习策略灰度,核心设计如下:

- **新增类型 `CapabilityToken`**: 承载一个学习策略的灰度通行证,内含 `token_id` / `strategy_version` / `level: CapabilityLevel` / `frozen: bool`。复用既有 [types.rs:22](file:///D:/Chimera%20CLI/crates/decay-engine/src/types.rs#L22) `CapabilityLevel(f32)` 连续流体类型(不新建权限模型),复用既有 [types.rs:54](file:///D:/Chimera%20CLI/crates/decay-engine/src/types.rs#L54) `Capability` 的 frozen 语义(对应 Skeptic 否决权)。
- **能力等级四档语义**: `CapabilityLevel` 的 [0.0, 1.0] 连续值在灰度语境下映射四档离散语义(非离散权限,而是连续值的语义标注):
  - `Draft` ≈ 0.0-0.3(初始低能力,仅影子模式,不下发生产策略)
  - `Canary` ≈ 0.3-0.6(小流量灰度,部分场景下发)
  - `Stable` ≈ 0.6-0.95(全量下发,但仍受衰减约束)
  - `Pinned` ≈ 0.95-1.0(冻结为不可降级基线,仅人类 ADR 特批可达)
- **自然进化路径**: 新策略初始注册为 `Draft`(level=0.2),随通道 A(auto-dpo 成对比较)成功率提升,由 gsoe-evolution 经 `PolicyUpdated` 事件异步上调能力;`Canary → Stable` 的迁移需连续 N 次统计显著正向(防抖动误升)。
- **安全闭环**: 学习策略引发 `AsaIntervention`(§6.2 Critical 安全事件)时,复用既有 [types.rs:107](file:///D:/Chimera%20CLI/crates/decay-engine/src/types.rs#L107) `DecayEvent::ViolationPenalty` 触发能力自动衰减(severity 由干预级别决定);连续衰减至 `freeze_threshold`(0.05)以下自动冻结,该策略版本退出生产(对应 Skeptic 否决权)。
- **能力等级迁移审计**: 每次 `Draft → Canary → Stable` 迁移与冻结/解冻走 Merkle 审计链(复用 seccore `audit.rs`,v5.0 §7.1 审计护栏),与 ADR-032 通道 B 验证器(CI 否决门)协同——能力等级迁移记录作为通道 B InvariantChecker 的输入之一。

> **与 ADR-002 的关系**: ADR-002 定义连续权限流体模型用于"能力衰减"(权限随违规递减);本决策将其语义扩展到"能力晋升"(策略随成功率递增),但**不修改 ADR-002 的 `CapabilityLevel` 类型与衰减算法**,仅新增 `CapabilityToken` wrapper 与晋升路径。属 append-only 扩展(对齐 ADR-028 决策 1 哲学)。

### 决策 3: 编译期 feature(Cargo feature)— 迁移期双轨零运行时开销

用于"渐进迁移"场景(如 `chimera-tui` 的 `v3-engine` feature 默认 off,逐面板迁移),遵循以下约束:

- **编译期决定**: feature 在 `cargo build` 时二选一,二进制内**无活旗**(无 `if cfg!(feature = ...)` 之外的运行时分支)。
- **必须显式声明**: feature 必须在 crate 的 `Cargo.toml [features]` 表显式声明,禁止隐式 feature。先例见 [chimera-tui/Cargo.toml:10-12](file:///D:/Chimera%20CLI/crates/chimera-tui/Cargo.toml#L10)。
- **双轨 CI 验证**: CI 必须验证 feature 开启与关闭两种组合均可 `cargo check` / `cargo test` 通过(release.yml 矩阵扩展),防止 feature 腐化导致某一组合编译失败。
- **默认 off 原则**: 渐进迁移期 feature 默认 off,迁移完成后在某 major 版本移除旧路径并删除 feature(对齐 ADR-029 v3-engine M0→M2 路径)。
- **适用边界**: 编译期 feature **仅用于"大版本迁移期双轨"**(如 ratatui → 自研引擎),**不用于学习策略灰度**(学习策略是运行时动态授权,编译期无法决定)。两者职责严格分离,不可互替。

### 决策 4: 策略枚举双实现 fallback — 默认值编译进二进制

学习失效回退走策略枚举双实现,而非运行时布尔开关:

- **双实现枚举**: 学习接缝的策略类型设计为枚举,例如 HCW selector 的 `SelectorPolicy::Static(常量) / Learned(版本号)`。静态变体(`Static`)的默认值 = 当前常量,**fallback 编译进同一二进制**(非运行时旗)。
- **本地 fallback**: learner(omega-learner,新 crate L6)只异步下发策略,调用方本地执行。learner panic / 超时 / 通道断开时,调用方降级到 `SelectorPolicy::Static`(本地编译进二进制的常量),**无跨 crate 旗标**。
- **fallback 默认值锚点**: 例——HCW selector 的 `w1/w2/w3` 常量从 [hcw-window/src/selector.rs](file:///D:/Chimera%20CLI/crates/hcw-window/src/selector.rs) 的硬编码常量改为注入式 `SelectorPolicy`(默认静态值 = 当前常量),omega-learner 异步下发学习值;fallback 即回退到该常量。此即 v5.0 设计文档第 235 行"fallback 编译进同一二进制,非运行时旗(C4)"的落地。
- **与决策 2 的分工**: 决策 2(能力场)管"新策略能否下发"(授权);决策 4(枚举 fallback)管"下发后失败如何回退"(降级)。两者正交:一个策略可同时是 `Stable` 级别(决策 2 授权全量下发)且 learner 超时时本地 fallback 到 `Static`(决策 4 降级)。

### 决策 5: 六接缝灰度路径(S1-S6)— 全部走能力场

v5.0 §7.3(第 368-381 行)定义的六接缝全部走决策 2 的能力场灰度(CapabilityToken),非运行时旗。每个接缝对应一个 `CapabilityToken`,其 `level` 决定该接缝学习策略是否生效:

| # | 接缝(真实代码锚点) | 上下文 | 臂(reward) | 能力场 token |
|---|---------------------|--------|------------|--------------|
| S1 | DDR/HCW 密度档位(hcw-window selector) | 任务类型/DAG 深度/内存压力 | ρ∈{0.5,2,5,10} | `token_s1_density` |
| S2 | 记忆策略选择(mlc-engine recall 路径) | 任务阶段(初期/卡壳/长跑) | 最小检索/标准TopK/查询重构/激进剪枝/时间聚焦 | `token_s2_recall` |
| S3 | SCC 预取(scc-cache prefetch.rs) | 编辑历史/调用图 | 预取候选集 | `token_s3_prefetch` |
| S4 | selector 权重系数(hcw-window selector.rs 的 w1/w2/w3) | 块类型/访问时序/错误关联 | 权重向量 | `token_s4_weights` |
| S5 | Parliament 激活(parliament Fast Path) | risk_level/只读性/历史模式 | 跳过/精简/完整辩论 | `token_s5_parliament` |
| S6 | 衰减参数(decay-engine DecayProfile) | 操作类型/风险信号密度 | profile 参数 | `token_s6_decay` |

- **learner 边界**: omega-learner 只异步下发策略(`PolicyUpdated` 经 event-bus 膜异步广播,v5.0 §7.1 确定性护栏),调用方本地执行 + 本地 fallback(决策 4)。learner 不直接 mutate 调用方状态。
- **S4 的特殊性**: S4(selector 权重系数)是 v5.0 §7.1 明确点名的 fallback 编译进二进制先例——`w1/w2/w3` 常量 fallback 必须与当前 [hcw-window/src/selector.rs](file:///D:/Chimera%20CLI/crates/hcw-window/src/selector.rs) 常量严格一致(P3-W10.3 实施),作为能力场灰度的兜底基线。
- **S6 的自指性**: S6(衰减参数)是 decay-engine 自身的能力场参数,其灰度走 `token_s6_decay`,但**衰减算法本身不可被学习**(属 v5.0 §7.2 不可进化面:验证器本身不可进化)。仅 profile 参数(如 time_decay_rate 微调)可学习,算法结构冻结。
- **六接缝灰度上线顺序**: 按风险从低到高灰度——S3(SCC 预取,可回退)→ S4(selector 权重,有静态 fallback)→ S1(密度档位)→ S2(记忆策略)→ S5(Parliament 激活,安全敏感)→ S6(衰减参数,自指)。每个接缝灰度上线前必须先有对应 criterion 基准(§3.4.1 第 6 条性能可证伪)。

## 理由

### 决策 1 理由(否决运行时 Feature Flag)

- **§6.1 红线 5(功能乱)**: Claude Code 尸检教训"44 个未发布标志"导致功能乱。本项目红线"禁止功能标志,用能力场自然进化替代"。运行时 Feature Flag 是该红线直接命中的反模式。
- **三态分离避免语义混淆**: 灰度的三种语义(运行时授权 / 编译期双轨 / 瞬时回退)时间尺度与触发条件不同,混用同一套运行时旗会导致状态空间爆炸(44 标志的根因之一)。三态分离使每种灰度有唯一正路。
- **可证伪性**: 运行时旗的覆盖矩阵是 2^N(指数级),无法穷举测试;三态分离后,能力场有 proptest 不变量,编译期 feature 有 CI 双组合验证,枚举 fallback 有穷尽 match。

### 决策 2 理由(能力场 CapabilityToken)

- **§6.1 红线 5 的正路**: 红线原文"用能力场自然进化替代",能力场正是红线提倡的方式。本决策是红线字面落地,非绕行。
- **复用 ADR-002 既有闭环**: decay-engine 已有完整的衰减—冻结—恢复闭环(时间驱动 + 事件驱动 + Skeptic 否决权),能力晋升只是该闭环的对称扩展(成功率驱动上调)。复用既有类型 `CapabilityLevel` / `Capability` / `DecayEvent`,append-only。
- **安全闭环(AsaIntervention 触发衰减)**: §6.2 红线"Critical 安全事件用 mpsc"。学习策略引发安全干预时,经既有 `AsaIntervention` Critical 事件链触发能力衰减,形成"学习—安全—衰减"闭环,无需新建安全通道。
- **审计护栏协同(v5.0 §7.1)**: 策略版本入 ADR + Merkle 链,复用 seccore `audit.rs`。能力等级迁移可追溯,满足 §3.4.1 第 7 条学术支撑落地的可审计要求。
- **与三重悖论记忆悖论红线对齐(§3.4.5)**: 记忆策略(S2)的灰度走能力场,能力等级随任务阶段自适应,避免固定 top-k 召回的"幽灵记忆"问题——能力场天然支持阶段切换时降级旧策略。

### 决策 3 理由(编译期 feature)

- **ADR-029 v3-engine 先例成立**: [chimera-tui/Cargo.toml:10-12](file:///D:/Chimera%20CLI/crates/chimera-tui/Cargo.toml#L10) 已验证编译期 feature 双轨可行,注释明确"每个里程碑可 cargo test --workspace 全绿、随时回滚"。
- **零运行时开销**: 编译期决定,二进制内无 `if` 分支,符合 §4.1 零成本抽象。
- **职责边界清晰**: 编译期 feature 仅管"大版本迁移期双轨",不越界管学习策略灰度(那是能力场职责),避免单一机制承载两种语义。
- **CI 双组合验证**: 防止 feature 腐化(某组合长期不编译),符合 §3.4.1 第 6 条性能可证伪的 CI 证据要求。

### 决策 4 理由(策略枚举双实现 fallback)

- **fallback 编译进二进制(v5.0 §7.1)**: 第 235 行明确"fallback 编译进同一二进制,非运行时旗(C4)"。枚举双实现的 `Static` 变体即此设计。
- **本地 fallback 无跨 crate 旗标**: learner panic / 超时时调用方本地降级,不依赖 learner 存活,符合 §4.4 async 反模式第 7 条(tokio::spawn fire-and-forget 评估框架)——关键路径必须管理 JoinHandle / 有 fallback。
- **默认值 = 当前常量**: 保证灰度前行为与灰度后 fallback 行为一致,回归风险可控。
- **与决策 2 正交**: 授权(决策 2)与降级(决策 4)分离,一个策略可同时 Stable 授权 + 本地 fallback,语义不冲突。

### 决策 5 理由(六接缝灰度路径)

- **v5.0 §7.3 六接缝定义**: 第 368-381 行已给出 S1-S6 的真实代码锚点(hcw-window selector / mlc-engine recall / scc-cache prefetch / selector.rs w1/w2/w3 / parliament Fast Path / decay-engine DecayProfile),非抽象概念。
- **learner 只异步下发(v5.0 §7.3 第 370 行)**: "learner 只异步下发策略,调用方本地执行 + 本地 fallback"。本决策将此约束编码为能力场 token + 枚举 fallback 的组合。
- **S4 的 fallback 先例(v5.0 §7.1 第 235 行)**: w1/w2/w3 常量 fallback 是 C4 决策明确点名的落地示例,本决策将其推广到全部六接缝。
- **S6 不可进化面(v5.0 §7.2 第 365 行)**: 衰减算法本身属不可进化面(硬编码,人类 ADR 特批),仅 profile 参数可学习,防止"验证器游戏化"(三重悖论进化悖论红线 §3.4.5)。
- **灰度顺序按风险**: 从可回退(S3)到安全敏感(S5/S6),每步有 criterion 基准,符合 §3.4.1 第 6 条性能可证伪与第 8 条专家评审。

## 影响

### 新增内容

- **新增类型 `CapabilityToken`**: `crates/decay-engine/src/types.rs` 新增(append-only,不改既有 `CapabilityLevel` / `Capability` / `DecayEvent`)
  - 字段:`token_id: String` / `strategy_version: u32` / `level: CapabilityLevel` / `frozen: bool` / `last_decay_at: Instant`
  - 四档语义常量:`DRAFT_THRESHOLD = 0.3` / `CANARY_THRESHOLD = 0.6` / `STABLE_THRESHOLD = 0.95` / `PINNED_THRESHOLD = 0.95`(连续值的语义标注,非离散枚举)
- **新增方法(DecayEngine 扩展)**:
  - `register_strategy_token(&self, token_id, strategy_version, initial_level)` — 注册新学习策略通行证(初始 Draft)
  - `promote_token(&self, token_id)` — 能力晋升(Draft → Canary → Stable),需统计显著正向证据
  - `freeze_token(&self, token_id, reason)` — 冻结策略通行证(复用既有 Freeze 语义,对应 Skeptic 否决权)
  - `token_level(&self, token_id) -> CapabilityLevel` — 查询当前能力等级(调用方据此决定是否采用学习策略)
- **新增策略枚举(各接缝)**: hcw-window / mlc-engine / scc-cache / parliament / decay-engine 各新增 `*Policy` 枚举(`Static / Learned`),默认 `Static` = 当前常量
- **新增 Merkle 审计扩展**: seccore `audit.rs` 新增能力等级迁移记录上链(复用既有 Merkle 机制)
- **新增测试**: ~60 个测试用例(单元:CapabilityToken 注册/晋升/冻结;proptest:能力场不变量;集成:六接缝灰度端到端)
- **新增 criterion 基准**: 能力场查询延迟(`token_level` < 1μs,对齐 decay-engine 既有 `decay_compute < 1μs`)
- **新增 ADR**: 本文档(ADR-034)
- **新增 spec(配套)**: `.trae/specs/capability-field-gradual-rollout/`(spec + tasks + checklist,后续建立)

### 修改内容

- **`crates/decay-engine/src/lib.rs`**: 新增 `pub use types::CapabilityToken;` 重导出(对齐 [lib.rs:37](file:///D:/Chimera%20CLI/crates/decay-engine/src/lib.rs#L37) 既有重导出风格)
- **`crates/decay-engine/src/types.rs`**: 新增 `CapabilityToken` 类型与四档阈值常量(append-only)
- **`crates/decay-engine/src/engine.rs`**: 新增 4 个方法(register/promote/freeze/token_level)
- **`crates/hcw-window/src/selector.rs`**: `w1/w2/w3` 从硬编码常量改为注入式 `SelectorPolicy`(默认值 = 当前常量,append-only)
- **`.github/workflows/release.yml`**: feature 矩阵扩展为双组合(`--features v3-engine` 与默认)验证
- **`CHANGELOG.md`**: 新增对应版本章节
- **`docs/architecture/adr_index.md`**: 新增 ADR-034 条目(总数 30,现有 ADR-006/026/027/028/029 + ADR-034;ADR-031/032 待建档时再增)
- **`docs/architecture/CODE_WIKI.md`**: decay-engine 条目补 CapabilityToken 子能力;§9.1 红线 5 补"能力场灰度见 ADR-034"交叉引用

### 资源影响评估

| 维度 | 评估 |
|------|------|
| crate 数量 | 35(不变,增量在 decay-engine + 各接缝 crate 内) |
| 依赖变更 | 无新增外部依赖(CapabilityToken 复用既有 f32 / Instant / String) |
| Docker/binary 体积 | 不受影响(纯 Rust 新增代码,无 unsafe 依赖) |
| NexusEvent 变体数 | 不变(复用既有 `AsaIntervention` / `PolicyUpdated`,不新增) |
| DecayError 变体数 | +2(`StrategyTokenNotFound` / `PromotionInsufficientEvidence`),append-only |
| 测试覆盖 | 新增 ~60 个测试,目标覆盖率 ≥ 85% |
| criterion 基准 | +1 项(`token_level` 查询延迟) |
| CI 时间 | 编译期 feature 双组合验证导致 +1 个 check job(约 +2-3 分钟) |
| 版本号 | 由后续 spec 决定(本 ADR 不锁定版本,对齐 ADR-028 决策 6 的 minor 交付哲学) |

## 考虑的方案

### 方案 A: 能力场 + 编译期 feature + 枚举 fallback(采纳)

- **内容**: 三态分离——运行时学习策略灰度走 decay-engine CapabilityToken(决策 2);大版本迁移期双轨走 Cargo 编译期 feature(决策 3);学习失效回退走策略枚举双实现(决策 4)。六接缝全部走能力场(决策 5)。
- **采纳理由**: 对齐 v5.0 §7.1 C4 决策(第 321-333 行);复用 ADR-002 既有衰减闭环;满足 §6.1 红线 5"用能力场自然进化替代"字面要求;有 ADR-029 v3-engine 编译期 feature 先例;有 hcw-window selector w1/w2/w3 fallback 先例;每个决策有真实代码锚点与回退路径。
- **风险**: CI 时间增长(双组合验证),可控。

### 方案 B: 运行时 Feature Flag(否决)

- **内容**: 通过 `LEARNER_ENABLED=true` / `config.yaml: learner: on` / DB 字段控制学习策略是否生效,散落的 `if learner_enabled { ... } else { static_policy }` 分支。
- **否决理由**:
  1. **直接违反 §6.1 红线 5**: 红线原文"禁止功能标志,用能力场自然进化替代"。运行时 Feature Flag 是该红线直接命中的反模式(Claude Code 44 个未发布标志教训)。
  2. **覆盖矩阵指数爆炸**: N 个学习接缝 × 2 态 = 2^N 测试组合,无法穷举验证,违反 §3.4.1 第 6 条性能可证伪的 CI 证据要求。
  3. **无安全闭环**: 运行时旗无法关联 AsaIntervention 触发衰减,学习策略失控时无自动冻结机制,违反 §6.2 红线"Critical 安全事件用 mpsc"的安全闭环设计。
  4. **v5.0 §11.2 第 705 行明确否决**: "运行时 Feature Flag 灰度学习策略 → 撞红线 5(ADR-034)"。

### 方案 C: 硬编码无灰度(否决)

- **内容**: 学习策略直接硬编码为常量,不提供灰度机制,一次性全量上线或不上线。
- **否决理由**:
  1. **学习策略无法渐进部署**: 新策略无灰度直接全量上线,风险高,违反 §3.4.1 第 3 条 TDD 守恒(新特性必须先写失败测试再实现)——无灰度即无渐进验证。
  2. **无回退能力**: 硬编码策略上线后发现问题需重新编译发布,不符合 §3.3.1 第 5 条向后兼容的渐进演进原则。
  3. **违背学习系统本质**: v5.0 §7 学习系统的核心是"新策略按风险/成功率动态衰减授权"(第 329 行),硬编码等于否定学习系统的灰度价值。
  4. **与 ADR-029 哲学冲突**: ADR-029 v3-engine feature 的"逐面板迁移、随时回滚"哲学证明灰度是必要的,硬编码无灰度是退化。

## 合规性

- **§2.1 分层映射**: 符合。增量在 decay-engine(L4 Security)与各接缝 crate(L2/L3/L6/L8)内,不改分层。
- **§2.2 依赖铁律 + 唯一通道**: 符合。CapabilityToken 在 decay-engine(L4)内;学习策略经 event-bus `PolicyUpdated` 异步广播(膜,§2.2 唯一通道);调用方向下依赖,不向上依赖。
- **§3.3.1 第 1 条(OMEGA 四定律守恒)**: 符合。能力场是 Ω-Evolve 的"自然进化"正路;不新建并行进化机制。
- **§3.3.1 第 4 条(领域类型稳定性)**: 符合。不改 `UserIntent` / `Quest` / `Checkpoint` / `OmniSparseMasks` / `CLV` / `NexusState` 核心领域类型;CapabilityToken 是 decay-engine 内部类型,非核心领域类型。
- **§3.3.1 第 5 条(向后兼容)**: 符合。append-only 扩展(不改 `CapabilityLevel` / `Capability` / `DecayEvent` 既有签名),归为 minor。
- **§3.4.1 第 3 条(TDD 守恒)**: 符合。能力场灰度先写失败测试(能力晋升需统计显著正向证据,proptest 1000 次不变量)再实现。
- **§3.4.1 第 6 条(性能可证伪)**: 符合。新增 `token_level` criterion 基准(< 1μs);六接缝灰度上线前各需对应 criterion 基准。
- **§3.4.1 第 7 条(学术支撑落地)**: 符合。能力场灰度有 v5.0 §7.1 C4 决策与 ADR-002 连续权限流体学术支撑;Merkle 审计有 seccore audit.rs 工业实现。
- **§3.4.1 第 8 条(专家团队评审)**: 符合。本 ADR 经 E01 首席架构师 + E05 生产系统专家 + E07 任务调度专家分布式评审。
- **§3.4.5 三重悖论红线**:
  - **记忆悖论红线**: 符合。S2(记忆策略选择)走能力场,能力等级随任务阶段自适应(MinimalRecall→StandardTopK→QueryReformulation→AggressivePruning),避免幽灵记忆。
  - **推理悖论红线**: 符合。能力场灰度的"协调成本/推理增益"比值可经 criterion 基准测量,超阈值时能力衰减退出。
  - **进化悖论红线**: 符合。S6 衰减算法本身属不可进化面(v5.0 §7.2),仅 profile 参数可学习,防验证器游戏化。
- **§4.1 编码规范**: 符合。`#![forbid(unsafe_code)]` 保持(decay-engine [lib.rs:28](file:///D:/Chimera%20CLI/crates/decay-engine/src/lib.rs#L28) 已有);库层 thiserror;无生产路径 unwrap/expect;单函数 ≤200 行;Top-K 用 `select_nth_unstable`;并发收集用 `FuturesUnordered`;f32 禁隐式转 f64(CapabilityLevel 全程 f32)。
- **§4.4 async 反模式**: 符合。learner 异步下发策略不持锁跨 .await;`PolicyUpdated` 经 event-bus 广播先 subscribe 再 spawn(第 3 条);Critical 安全事件(AsaIntervention 触发衰减)用 mpsc(第 5 条);fire-and-forget 评估框架——能力场查询是幂等读,关键路径(晋升/冻结)管理 JoinHandle(第 7 条)。
- **§6.1 架构红线(功能乱)**: 符合。本 ADR 是红线 5"禁止功能标志,用能力场自然进化替代"的直接落地;三态分离避免 44 标志反模式。
- **§6.2 Week 1-8 实战新红线**: 符合。不持锁 .await;broadcast 先 subscribe;BudgetExceeded severity = Critical;Critical 安全事件用 mpsc;不引入 unsafe 依赖;Top-K 用 select_nth_unstable。AsaIntervention 触发衰减走既有 mpsc 通道。
- **§10.2 Docker 镜像约束**: 符合。镜像 < 100MB,binary < 50MB(无新增 unsafe 依赖,纯 Rust append-only)。
- **ADR-002 能力衰减模型**: 保持。本 ADR 复用其 `CapabilityLevel` / `Capability` / `DecayEvent` 类型与衰减算法,仅 append-only 扩展 `CapabilityToken` wrapper 与晋升路径,不修改既有衰减语义。
- **ADR-029 TUI v3.1(v3-engine feature)**: 保持。本 ADR 将其 `v3-engine` feature 模式提炼为编译期 feature 决策(决策 3)的先例,不修改 ADR-029 任何决策。

## 相关文档

- **设计文档**: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.1(C4 决策,第 321-333 行)/ §7.2(Harness-as-Spec 不可进化面,第 365 行)/ §7.3(omega-learner 六接缝,第 368-381 行)/ §7.4(RHI-CG 双通道,第 394-395 行)/ §11.2(ADR 编号预占,第 678-680 行)
- **规则**: `.trae/rules/nuxus规则.md` §2.1(分层映射)/§2.2(依赖铁律)/§3.3.1(第二阶段开发原则)/§3.4(第三阶段开发)/§3.4.5(三重悖论红线)/§4.1(编码规范)/§4.4(async 反模式)/§6.1(架构红线,红线 5 禁止功能标志)/§6.2(Week 1-8 新红线)/§10.2(Docker 镜像约束)
- **CODE_WIKI.md**: `docs/architecture/CODE_WIKI.md` §9.1(红线 5)/§3.1(crate 索引)/§11(ADR 表)
- **ADR 索引**: `docs/architecture/adr_index.md`(本 ADR 同步更新,新增 ADR-034 条目)
- **关联 ADR**:
  - **ADR-002**(能力衰减模型设计,decay-engine 落地)— 本 ADR 复用其连续权限流体,append-only 扩展晋升路径
  - **ADR-029**(TUI v3.1,v3-engine feature 双轨先例)— 本 ADR 将其编译期 feature 模式提炼为决策 3 先例
  - **ADR-031**(Harness-as-Spec + omega-learner 边界,六接缝来源)— **v5.0 §11.2 标注"提议"状态,尚未建立为独立文档**;本 ADR 引用其六接缝概念定义(v5.0 §7.3),待 ADR-031 建档时与本 ADR 双向校验
  - **ADR-032**(双通道评估器,能力等级迁移 Merkle 审计协同)— **v5.0 §11.2 标注"提议"状态,尚未建立为独立文档**;本 ADR 引用其通道 B 验证器概念(v5.0 §7.4),能力等级迁移记录作为通道 B InvariantChecker 输入;待 ADR-032 建档时与本 ADR 双向校验
- **代码锚点**:
  - [decay-engine/src/lib.rs](file:///D:/Chimera%20CLI/crates/decay-engine/src/lib.rs#L28)(`#![forbid(unsafe_code)]` + 重导出风格)
  - [decay-engine/src/types.rs](file:///D:/Chimera%20CLI/crates/decay-engine/src/types.rs#L22)(`CapabilityLevel` newtype)
  - [decay-engine/src/types.rs](file:///D:/Chimera%20CLI/crates/decay-engine/src/types.rs#L54)(`Capability` 结构体)
  - [decay-engine/src/types.rs](file:///D:/Chimera%20CLI/crates/decay-engine/src/types.rs#L107)(`DecayEvent::ViolationPenalty` 双驱动衰减)
  - [chimera-tui/Cargo.toml](file:///D:/Chimera%20CLI/crates/chimera-tui/Cargo.toml#L10)(`v3-engine` feature 编译期双轨先例)
  - `crates/hcw-window/src/selector.rs`(S4 selector 权重 w1/w2/w3 fallback 锚点)

---

> **维护者**: NEXUS-OMEGA 团队
> **创建日期**: 2026-07-23
> **基线版本**: v2.3.1-omega(创建时)
> **目标里程碑**: v3.0.0-omega(P4 学习层 W13-16 落地期,六接缝按风险顺序灰度)
> **决策团队**: E01 首席架构师 + E05 生产系统专家 + E07 任务调度专家(分布式评审)
> **分析依据**: v5.0 设计文档 §7.1 C4 决策 + §7.3 六接缝 + §7.4 双通道 + 既有 ADR-002/ADR-029 代码实证
