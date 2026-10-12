# ADR-032: RHI-CG 双通道评估器(通道 A 提议 L2 + 通道 B 否决 L3)

## 状态

已批准 (Accepted) (2026-07-23)

## 背景

- v5.0 设计文档提出 **RHI-CG(Recursive Harness-Induced Code Generation)** 双通道进化机制:通过"提议—否决"分离的递归式 spec 进化,驱动 Chimera CLI 的 Harness(测试套件 / 基准 / lint 规则)自演化,而非依赖人工编写测试。
- **进化悖论红线**(§3.4.5 三重悖论):当前 `gsoe-evolution` / `auto-dpo` 使用执行反馈(测试通过 / 失败)作为验证信号,属于验证器层级 **L3(执行反馈)**,存在被"奖励黑客(reward hacking)"游戏化风险——进化策略可能学会让测试变简单而非让代码变正确。第三阶段需向 **L4(形式化验证)** 或 **L5(人类研究判断)** 跃迁。
- v5.0 设计文档 **C2 决策**:通道 A 复用 `auto-dpo` 的 `PreferencePair` 机制生成 spec 候选对;进化执行复用 `gsoe-evolution` 的 `GrpoRollout` / `EvolutionResult` 谱系机制(命名映射见 ADR-031 附录 A)。
- v5.0 设计文档 **C4 决策**:spec 候选的部署灰度走 `decay-engine` 能力场(连续权限流体),而非运行时功能标志(§6.1 红线"禁止功能标志,用能力场自然进化替代")。
- **当前代码基线**(v2.3.1-omega,2026-07-23 核实):
  - [auto-dpo/src/generator.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs) 已实现 [PreferencePairGenerator::generate()](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L94)(从候选 `ModelOutput` 选 chosen/rejected,发布 `DpoPairGenerated` 事件)与 [with_event_bus()](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L61)(注入共享 EventBus)。
  - [auto-dpo/src/types.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/types.rs) 已定义 [PreferencePair](file:///D:/Chimera CLI/crates/auto-dpo/src/types.rs#L117)(pair_id / chosen / rejected / chosen_score / rejected_score / quality)。
  - [gsoe-evolution/src/engine.rs](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs) 已实现 [GsoeEvolutionEngine::evolve_once()](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs#L106)(采样→评估→选择→变异→发布 `GsoePolicyUpdated`)与 [with_event_bus()](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs#L63)。
  - [gsoe-evolution/src/types.rs](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs) 已定义 [EvolutionPolicy](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L23) / [GrpoRollout](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L90) / [EvolutionResult](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L156)。
  - [chimera-mas/src/invariants.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs) 已实现 `InvariantChecker`(纯函数,关联函数)的 INV-7(上下文预算界)与 INV-8(归档单调性),为 INV-9 提供既定模式。
- 经 E01 首席架构师 + E06 认知科学专家 + E07 任务调度专家分布式深度分析与多轮交叉验证,确认 RHI-CG 双通道机制可通过 **复用 auto-dpo / gsoe-evolution 既有类型 + 新增双通道协调逻辑** 落地,无需新建 crate、无需修改核心领域类型、无需引入 unsafe 依赖。
- 本 ADR 记录将 RHI-CG 双通道评估器以 **append-only 增量** 落地的 5 项关键决策,作为 spec `implement-rhi-cg-dual-channel` 实现的权威依据。

> **ADR 编号确认**: `docs/architecture/adr_index.md` 现有最大编号为 ADR-029(2026-07-23 核实)。ADR-030 / ADR-031 为 v5.0 设计文档协同规划的并行 ADR(Harness-as-Spec 边界 / omega-learner 边界),本 ADR 编号确认为 **ADR-032**,与 `CODE_WIKI.md §2.3 ADR 决策参考` 保持一致。

## 决策

经专家团队多轮结构化思考与多路径交叉验证,对 RHI-CG 双通道评估器作出以下 5 项架构决策:

### 决策 1: 通道 A(提议,L2 验证器)— 复用 PreferencePairGenerator 扩展 spec 候选对

通道 A 负责生成 spec 候选(提议),验证器层级为 **L2(LLM 评判)**,复用 `auto-dpo` 既有机制扩展:

- **复用 [PreferencePairGenerator::generate()](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L94)**:输入为相邻 spec 版本的两个候选输出(`ModelOutput` 列表),输出为 [PreferencePair](file:///D:/Chimera CLI/crates/auto-dpo/src/types.rs#L117)(chosen = 更优 spec 版本,rejected = 较弱 spec 版本)。沿用既有"选最高分为 chosen、最低分为 rejected + 质量门控 + 偏好信号校验"流程。
- **评判器 LLM 经 `model-router` 调用**:模型选择走白名单(§6.1 红线"裸奔"——所有外部调用经 SecCore 沙箱)。评判器 LLM **仅读不写**:只接收 spec 候选文本返回评分,无写路径(防止 prompt 注入污染 spec 仓库)。
- **自比较历史持久化于 `mlc-engine` L2 语义记忆**:通道 A 产生的 `PreferencePair` 序列(记为 `P.F[]`,即 Preference Family)写入 `mlc-engine` 的 L2 层(温记忆),携带时间证据包(生成时间 / 评判器模型 ID / spec 版本号)。时间证据包确保任务阶段切换时可区分新旧事实(§3.4.5 记忆悖论红线:固定 top-k 召回会产生"幽灵记忆")。
- **提议性质**:通道 A **仅生成 spec 候选,不直接部署**。生成的 `PreferencePair` 进入待审队列,由通道 B 做否决门检查后才进入单 lineage 更新(决策 3)。这是"提议与否决分离"的核心——通道 A 的 L2 评判成本低于通道 B 的 L3 执行,先发散后收敛。

> **L2 验证器定义**:验证器层级 L2 = LLM 评判(模型作为裁判打分)。成本低、延迟低,但存在"模型自评偏差"(模型倾向偏好自身风格输出),因此不可单独作为部署依据,必须经通道 B 的 L3 执行反馈否决。

### 决策 2: 通道 B(否决,L3 验证器)— CI 执行门 + INV-9 否决证据充分性

通道 B 负责否决不合格 spec 候选,验证器层级为 **L3(执行反馈)**,集成既有 CI 门控:

- **CI 执行门集成**:通道 B 的否决基于 4 类执行信号:
  1. `cargo test --workspace`(全量测试通过)
  2. criterion 基准(无统计显著回归)
  3. 红线 lint(`cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` + `cargo fmt --all -- --check`)
  4. `InvariantChecker`(INV-7 上下文预算 / INV-8 归档单调性 / **INV-9 否决证据充分性**)
- **连续 3 次统计显著回归才否决(防 bench 抖动误杀)**:单次 criterion 基准回归不触发否决。否决条件为:
  - 基准回归幅度 > 阈值(默认 10%,可配)
  - 统计显著性 p < 0.05(Welch t-test,对比最近 N 次基线)
  - **连续 3 次**满足上述两条(即 3 次独立 CI 运行均统计显著回归)
  - 目标误杀率 < 5%(误杀 = spec 候选实际无害但被通道 B 否决)
- **新增 INV-9 不变量(否决证据充分性)**:
  > INV-9 — 否决决策必须基于连续 `VETO_STREAK_THRESHOLD`(=3)次统计显著回归,禁止单次基准抖动触发否决。
  - `InvariantChecker::check_inv9_veto_evidence(regression_streak: u32, significance: f64) -> Result<()>`
  - `regression_streak < 3` 或 `significance >= 0.05` 时返回 `Ok(())`(证据不足,不否决,候选通过)
  - `regression_streak >= 3 && significance < 0.05` 时返回 `Err(MasError::VetoEvidenceInsufficient)`(证据充分,否决)
  - 纯函数设计,沿用 [InvariantChecker](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs) 既有模式(关联函数,无状态,无副作用)
- **在 `.github/workflows/release.yml` 集成 CI 否决门**:通道 B 否决逻辑作为 release workflow 的一个 job(`rhi-cg-veto-gate`),在 `build` / `test` job 之后执行。否决门失败时阻止 spec 候选部署,但不阻塞正常 release(否决门仅作用于 RHI-CG 进化路径,非主干发布路径)。

> **L3 验证器定义**:验证器层级 L3 = 执行反馈(CI 通过 / 失败 + 基准回归)。成本高(需全量构建 + 测试 + 基准)、延迟高,但客观可证伪(§3.4.1 第 6 条"性能可证伪")。L3 是当前可达的最高验证器层级,L4 形式化验证待 FormalVerifier 落地(决策 4)。

### 决策 3: 单 lineage 更新 — 复用 GsoeEvolutionEngine 扩展谱系更新路径

通道 A 提议 + 通道 B 放行后,通过单 lineage(单谱系)更新将 spec 候选纳入进化谱系:

- **扩展 [GsoeEvolutionEngine](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs#L30) 单 lineage 更新路径**:新增方法 `evolve_lineage(&mut self, current_spec, self_comparison_history: &[PreferencePair]) -> Result<EvolutionResult, GsoeError>`。
  - 输入 = 当前 spec + 全部自比较历史(通道 A 的 `P.F[]`)
  - 复用 [evolve_once()](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs#L106) 的"采样→评估→选择→变异→发布"流程,但采样源从随机策略扰动改为基于 `P.F[]` 的 spec 候选对
  - 复用 [GrpoRollout](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L90)(单次采样轨迹)+ [EvolutionResult](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L156)(新策略 + 改进幅度 + 世代数)谱系机制
- **命名对账(ADR-031 附录 A)**:v5.0 设计文档使用 `GrpoPolicy` / `EvolutionRecord` 命名,代码基线为 [EvolutionPolicy](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L23) / `EvolutionResult`。本 ADR 遵循代码基线命名,映射关系:
  | v5.0 设计文档命名 | 代码基线命名(本 ADR 采用) |
  |------------------|--------------------------|
  | `GrpoPolicy` | `EvolutionPolicy` |
  | `EvolutionRecord` | `EvolutionResult` |
  | `Rollout` | `GrpoRollout` |
- **部署灰度走 `decay-engine` 能力场(C4 合规)**:新 spec 版本不通过运行时旗部署,而是经 `decay-engine` 的连续权限流体渐进激活。能力场控制 spec 版本的生效比例(如灰度 10% → 50% → 100%),违反 §6.1 红线"禁止功能标志"的方案一律否决(见考虑的方案)。

### 决策 4: 验证器层级跃迁路径 — L2/L3 当前 → L4 形式化 → L5 人类判断

明确 RHI-CG 验证器层级的演进路线,避免在 L3 停滞导致奖励黑客风险累积:

| 阶段 | 验证器层级 | 信号来源 | 状态 |
|------|-----------|---------|------|
| **当前(M0)** | L2 + L3 | LLM 评判(通道 A)+ CI 执行反馈(通道 B) | ✅ 本 ADR 落地 |
| **近未来(M1)** | L4 形式化验证 | FormalVerifier(待落地)— spec 属性的形式化证明 | ⏳ R2 冻结 |
| **终极(M2)** | L5 人类研究判断 | 5 任务集进化 3 轮胜率 ≥60% 北极星指标 | 🎯 北极星 |

- **R2 冻结规则**:在 FormalVerifier 落地前,通道 B 的 L3 执行反馈是最高验证器层级。**禁止**在 R2 冻结期内将 L2 LLM 评判单独作为部署依据(防奖励黑客,L2 模型自评偏差不可控)。
- **L4 形式化验证跃迁条件**:FormalVerifier 落地后,通道 B 增加 L4 门(spec 属性的可证明检查,如"不变量在所有输入下成立")。L4 通过的 spec 候选可跳过 L3 的连续 3 次回归要求(单次 CI 通过即可),加速进化。
- **L5 北极星指标**:5 个任务集覆盖 `code_refactor` / `debug` / `test` / `docs` / `feature` 五种 task_type,进化 3 轮后胜率 ≥60% 方认定为"进化有效"。胜率 = 进化后 spec 在该任务集上的表现优于进化前的比例。

### 决策 5: 奖励护栏 — RL 奖励必须含 ≥L3 执行反馈信号

防止奖励黑客的核心护栏:

- **奖励组成强制约束**:RL(强化学习)奖励函数 **必须**包含至少 L3(执行反馈)层级的信号。禁止单独使用以下信号作为唯一奖励:
  - 模型自评(L0,模型给自身打分)
  - LLM 评判(L2,通道 A 的评判器评分)
- **奖励公式(参考)**:`reward = w_exec * R_exec(L3) + w_llm * R_llm(L2) + w_human * R_human(L5)`,其中 `w_exec > 0`(强制),`w_llm ∈ [0, 0.3]`(上限,防 L2 主导),`w_human` 仅在 L5 评估期非零。
- **奖励黑客检测**:若发现进化策略在 L2 评判分数持续上升但 L3 执行反馈无改善(或下降),触发 `NexusEvent::RewardHackingSuspected`(Normal 级 broadcast),暂停该 lineage 进化并通知 E02 安全架构师审查。
- **护栏与 §3.4.5 进化悖论红线对齐**:当前 GSOE/AutoDPO 属 L3 验证器层级,存在被游戏化风险。奖励护栏是"在 L4/L5 落地前的过渡期防御措施",确保 L3 信号始终在奖励中占主导。

## 理由

### 决策 1 理由(通道 A 复用 PreferencePairGenerator)

- **§3.4.1 第 7 条(学术支撑落地)**:GRPO(Group Relative Policy Optimization)算法要求成对比较信号,`PreferencePair` 的 chosen/rejected 结构天然适配。复用既有类型避免重复造轮子(§6.1 红线"功能乱")。
- **§2.2 依赖铁律**:`auto-dpo`(L5)→ `model-router`(L1)向下依赖合法;`auto-dpo` → `mlc-engine`(L2)向下依赖合法。通道 A 的跨层调用符合依赖方向。
- **§6.2 红线(学习层走异步广播)**:`PreferencePairGenerator` 通过 [with_event_bus()](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L61) 注入共享 EventBus,生成的 `PreferencePair` 经 `DpoPairGenerated` 事件异步广播给 `gsoe-evolution` / `mlc-engine`,不阻塞提议主路径。
- **§3.4.5 记忆悖论红线**:自比较历史持久化于 `mlc-engine` L2 层并携带时间证据包,避免固定 top-k 召回产生"幽灵记忆"(新旧 spec 版本共存无法区分时间有效性)。

### 决策 2 理由(通道 B CI 执行门 + INV-9)

- **§3.4.1 第 6 条(性能可证伪)**:criterion 基准是性能 claims 的客观证据。通道 B 的否决基于统计显著回归(p < 0.05),而非主观判断。
- **§3.4.5 进化悖论红线**:L3 执行反馈是当前可达的最高验证器层级。连续 3 次统计显著回归的阈值设计,是防 bench 抖动误杀的统计学护栏(单次回归可能由环境噪声导致,如 CI runner 负载波动)。
- **§6.2 红线**:INV-9 违反时复用 `MasError` 错误体系(库层 thiserror),否决决策走 `InvariantChecker` 纯函数,不引入状态与副作用。
- **误杀率 < 5% 的依据**:连续 3 次独立 CI 运行均统计显著回归的概率,在无真实回归时约为 (0.05)^3 = 0.0125%(假设独立),远低于 5% 目标。实际因 CI 运行非完全独立(共享代码库状态),误杀率略高但仍在可接受范围。

### 决策 3 理由(单 lineage 更新复用 GsoeEvolutionEngine)

- **§3.3.1 第 1 条(OMEGA 四定律守恒)**:Ω-Evolve 由 `gsoe-evolution` 单一实现。通道 A/B 的进化执行复用 [GsoeEvolutionEngine](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs#L30),不自实现进化逻辑。
- **§6.1 红线(功能乱)**:Claude Code 尸检教训"44 个未发布标志",本项目红线"禁止功能标志,用能力场自然进化替代"。部署灰度走 `decay-engine` 能力场(C4 决策),非运行时旗。
- **§3.3.1 第 4 条(领域类型稳定性)**:复用 [EvolutionPolicy](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L23) / [GrpoRollout](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L90) / [EvolutionResult](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L156),不新建核心领域类型,避免触发 ADR 流程。
- **append-only 策略**:新增 `evolve_lineage` 方法,不修改 [evolve_once()](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs#L106) 现有签名,零回归风险(§3.3.1 第 5 条向后兼容)。

### 决策 4 理由(验证器层级跃迁路径)

- **§3.4.5 进化悖论红线**:当前 L3 执行反馈存在被"奖励黑客"游戏化风险。R2 冻结规则确保在 L4 形式化验证落地前,L3 始终是最高验证器层级,且 L2 不可单独作为部署依据。
- **§3.4.1 第 7 条(学术支撑落地)**:验证器层级跃迁参考 DeepSeek V4 的 RLHF 多层验证理念(执行反馈 → 形式化约束 → 人类对齐)。
- **北极星指标的可证伪性**:5 任务集 × 3 轮 × 胜率 ≥60% 是可量化、可复现的进化有效性指标,符合 §3.4.1 第 6 条"性能可证伪"。

### 决策 5 理由(奖励护栏)

- **§3.4.5 进化悖论红线**:"验证器层级 3→需跃迁至层级 4 形式化验证"。在 L4 落地前,奖励护栏是过渡期防御措施,确保 L3 信号始终主导奖励。
- **§6.1 红线(裸奔)**:Claude Code 尺检教训"命令插值 + auth 跳过"。奖励黑客是 RL 系统的"auth 跳过"——进化策略学会绕过验证器而非真正改进。奖励护栏强制 L3 信号在场,是反绕过的结构性约束。
- **奖励黑客检测的事件化**:`RewardHackingSuspected` 事件走 EventBus 异步广播(§6.2 红线"学习层走异步广播"),不阻塞进化主路径,但触发 E02 安全架构师审查闭环。

## 影响

### 新增内容

- **新增 INV-9 不变量**:`crates/chimera-mas/src/invariants.rs` 新增
  - `InvariantChecker::check_inv9_veto_evidence(regression_streak, significance) -> Result<()>`(纯函数,关联函数)
  - `VETO_STREAK_THRESHOLD: u32 = 3` 常量
  - `VETO_SIGNIFICANCE_THRESHOLD: f64 = 0.05` 常量
  - 测试覆盖:单元测试(边界场景)+ proptest 属性测试(随机 streak / significance 组合)
- **新增 MasError 变体**:1 个(共 34 个)
  - `VetoEvidenceInsufficient { regression_streak, significance }`(决策 2 INV-9 违反)
- **新增 GsoeEvolutionEngine 方法**:`crates/gsoe-evolution/src/engine.rs` 新增
  - `evolve_lineage(&mut self, current_spec, self_comparison_history: &[PreferencePair]) -> Result<EvolutionResult, GsoeError>`(单 lineage 更新路径,append-only,不改 [evolve_once()](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs#L106) 签名)
- **新增 RHI-CG 协调模块**:在 `gsoe-evolution` 内新增 `rhi_cg.rs` 子模块(双通道协调:通道 A 调用 `PreferencePairGenerator` + 通道 B 调用 `InvariantChecker::check_inv9_veto_evidence`)
  - 不新建 crate(§3.3.1 第 6 条新 crate 准入,RHI-CG 是 gsoe-evolution 的进化能力扩展,非独立子系统)
- **新增 NexusEvent 变体**:1 个
  - `RewardHackingSuspected { metadata, lineage_id, llm_trend, exec_trend }`(决策 5 奖励黑客检测,Normal 级 broadcast)
- **新增 CI job**:`.github/workflows/release.yml` 新增 `rhi-cg-veto-gate` job(通道 B 否决门,在 build/test 之后,非主干阻塞)
- **新增 spec**:`.trae/specs/implement-rhi-cg-dual-channel/`(spec + tasks + checklist)
- **新增 criterion 基准**:2 项
  - `rhi_cg_propose_latency`(通道 A 提议延迟,目标 < 500ms)
  - `rhi_cg_veto_check_latency`(通道 B INV-9 检查延迟,目标 < 1ms,纯函数)

### 修改内容

- **`crates/chimera-mas/src/invariants.rs`**:新增 INV-9 检查方法 + 常量(INV-7/INV-8 既有方法不变)
- **`crates/chimera-mas/src/error.rs`**:新增 `VetoEvidenceInsufficient` 变体(静态断言 33 → 34)
- **`crates/gsoe-evolution/src/engine.rs`**:新增 `evolve_lineage` 方法(append-only)
- **`crates/gsoe-evolution/src/lib.rs`**:新增 `rhi_cg` 模块声明 + 重导出
- **`crates/event-bus/src/types.rs`**:新增 `RewardHackingSuspected` 事件变体(74 → 75)
- **`.github/workflows/release.yml`**:新增 `rhi-cg-veto-gate` job
- **`CHANGELOG.md`**:新增 RHI-CG 双通道评估器章节
- **`docs/architecture/CODE_WIKI.md`**:gsoe-evolution / chimera-mas 条目补 RHI-CG 子模块
- **`docs/architecture/adr_index.md`**:新增 ADR-032 条目

### 资源影响评估

| 维度 | 评估 |
|------|------|
| crate 数量 | 35(不变,增量在 gsoe-evolution / chimera-mas 内) |
| 依赖变更 | 无新增外部依赖(复用既有 auto-dpo / gsoe-evolution / model-router / mlc-engine / decay-engine) |
| Docker/binary 体积 | 不受影响(纯 Rust 新增代码,无 unsafe 依赖) |
| NexusEvent 变体数 | 74 → 75(新增 `RewardHackingSuspected`,Normal 级 broadcast) |
| MasError 变体数 | 33 → 34(新增 `VetoEvidenceInsufficient`) |
| InvariantChecker 不变量数 | 2 → 3(新增 INV-9 否决证据充分性) |
| 测试覆盖 | 新增 ~30 个测试(单元 + proptest + 集成) |
| criterion 基准 | 新增 2 项 RHI-CG 基准(总基准数 +2) |
| CI 时间 | `rhi-cg-veto-gate` job 增加 ~2-3 分钟(全量测试 + 基准对比),非主干阻塞 |
| 版本号 | append-only minor 增量(2.3.1-omega → 2.4.0-omega,待 release 确认) |

## 考虑的方案

### 方案 A: 双通道(提议 L2 + 否决 L3)+ 单 lineage 更新(采纳)

- **内容**:通道 A 复用 `auto-dpo::PreferencePairGenerator` 做 L2 LLM 评判提议;通道 B 集成 CI 执行门 + INV-9 做 L3 否决;单 lineage 更新复用 `gsoe-evolution::GsoeEvolutionEngine`。
- **采纳理由**:
  1. 提议与否决的验证器层级分离,降低误杀(L2 低成本发散 + L3 高成本收敛)
  2. 80% 复用既有类型(`PreferencePair` / `GrpoRollout` / `EvolutionResult` / `InvariantChecker`),避免重复造轮子
  3. INV-9 的连续 3 次统计显著回归阈值,将误杀率控制在 < 5%
  4. R2 冻结规则防奖励黑客(L4 落地前 L2 不可单独部署)
  5. append-only 策略,零回归风险

### 方案 B: 单通道(仅 CI 否决)(否决)

- **内容**:仅保留通道 B(CI 执行门否决),不实现通道 A(LLM 评判提议)。
- **否决理由**:
  1. **缺少提议能力,进化停滞**:无通道 A 的 L2 提议,进化只能依赖人工编写 spec 候选,无法实现 RHI-CG 的"递归式 spec 自演化"目标
  2. **违反 v5.0 设计文档 C2 决策**:C2 明确要求通道 A 复用 auto-dpo 的 PreferencePair 机制
  3. **L3 单独成本过高**:若每次 spec 候选都直接走 CI 全量测试 + 基准,无 L2 预筛,CI 资源消耗过大

### 方案 C: 三通道(提议 + 否决 + 形式化验证)(否决)

- **内容**:在方案 A 基础上增加通道 C(L4 形式化验证),三通道并行评估。
- **否决理由**:
  1. **FormalVerifier 未落地**:L4 形式化验证器尚在规划中,无代码基线可复用
  2. **R2 冻结**:决策 4 明确在 FormalVerifier 落地前 R2 无条件冻结,通道 C 无法实现
  3. **过早抽象**:§6.1 红线"禁止功能标志,用能力场自然进化替代"——三通道在 L4 未就绪时是过早的架构抽象,应等 L4 落地后以 append-only 方式扩展(决策 4 的 L4 跃迁路径已预留扩展点)

## 合规性

- **§2.1 分层映射**:符合。增量在 `gsoe-evolution`(L5)/ `chimera-mas`(L9)/ `auto-dpo`(L5)/ `event-bus`(L1)内,不改分层。通道 A 跨层调用 `model-router`(L1)向下依赖合法。
- **§2.2 依赖铁律 + 唯一通道**:符合。学习层经 EventBus 异步广播(决策 5 `RewardHackingSuspected` 事件);L5 → L1/L2 向下依赖,不向上依赖。
- **§3.3.1 第 1 条(OMEGA 四定律守恒)**:符合。Ω-Evolve 由 gsoe-evolution 单一实现(决策 3),不自实现进化逻辑;Ω-Event 由 event-bus 单一实现。
- **§3.3.1 第 4 条(领域类型稳定性)**:符合。不改 `UserIntent` / `Quest` / `Checkpoint` / `OmniSparseMasks` / `CLV` / `NexusState`,复用 `EvolutionPolicy` / `GrpoRollout` / `EvolutionResult` / `PreferencePair`。
- **§3.3.1 第 5 条(向后兼容)**:符合。append-only(决策 3 新增 `evolve_lineage` 不改 `evolve_once` 签名),minor 版本。
- **§3.3.1 第 6 条(新 crate 准入)**:符合。不新建 crate,RHI-CG 协调模块在 gsoe-evolution 内。
- **§3.4.1 第 6 条(性能可证伪)**:符合。2 项 criterion 基准(通道 A 提议延迟 + 通道 B INV-9 检查延迟);通道 B 否决基于统计显著回归(p < 0.05)。
- **§3.4.1 第 7 条(学术支撑落地)**:符合。GRPO 算法 + DeepSeek V4 RLHF 多层验证理念(决策 4 验证器层级跃迁)。
- **§3.4.5 三重悖论红线**:符合。记忆悖论(决策 1 时间证据包)、推理悖论(决策 3 单 lineage 降低协调成本)、进化悖论(决策 4/5 验证器层级跃迁 + 奖励护栏)三条红线全部对齐。
- **§4.1 编码规范**:符合。`#![forbid(unsafe_code)]` 保持;库层 thiserror;无生产路径 unwrap/expect;单函数 ≤200 行;Top-K 用 `select_nth_unstable`(沿用 [select_elite](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs#L248) 既有模式)。
- **§4.4 async 反模式**:符合。不持锁跨 .await;broadcast 先 subscribe 再 spawn;学习层走异步 broadcast(§6.2 红线)。
- **§6.1 架构红线**:符合。不引入功能标志(决策 3 C4 能力场);单函数 ≤200 行;async 必须 await 或 spawn 管理;L2 评判器经 model-router 白名单调用(无写路径防注入)。
- **§6.2 Week 1-8 实战新红线**:符合。不持锁 .await;学习层走异步广播;`RewardHackingSuspected` 为 Normal 级 broadcast(非 Critical,不走 mpsc);INV-9 违反返回 `MasError`(库层 thiserror)。
- **C2 决策(嫁接 auto-dpo / gsoe-evolution)**:符合。决策 1 复用 `PreferencePairGenerator` / `PreferencePair`;决策 3 复用 `GsoeEvolutionEngine` / `GrpoRollout` / `EvolutionResult`。
- **C4 决策(灰度走能力场)**:符合。决策 3 部署灰度经 `decay-engine` 能力场,非运行时旗。
- **ADR-026 / ADR-028 既有决策**:全部保持。INV-9 沿用 `InvariantChecker` 纯函数模式(ADR-028 决策 3);`MasError` 变体扩展沿用 append-only(ADR-028 决策 1)。

## 相关文档

- **spec**: `.trae/specs/implement-rhi-cg-dual-channel/`(本 ADR 配套,待创建)
- **设计文档**: v5.0 设计文档 §7 学习系统 / §14 实施路线图(P5 阶段)— RHI-CG 双通道进化机制设计源
- **规则**: `.trae/rules/nuxus规则.md` §2.1(分层映射)/§2.2(依赖铁律)/§3.3.1(第二阶段开发原则)/§3.4.1(第三阶段开发原则)/§3.4.5(三重悖论红线)/§4.1(编码规范)/§4.4(async 反模式)/§6.1(架构红线)/§6.2(Week 1-8 新红线)
- **CODE_WIKI.md**: `docs/architecture/CODE_WIKI.md` §3.1(crate 索引)/§2.3(ADR 表)
- **ADR 索引**: `docs/architecture/adr_index.md`(本 ADR 同步更新)
- **关联 ADR**:
  - ADR-026(MAS 子系统 Stage A,InvariantChecker 既有模式来源)
  - ADR-028(Part II 闭环,INV-7/INV-8 不变量 + StabilityGuard + PdcaLoop)
  - ADR-031(Harness-as-Spec + omega-learner 边界,命名映射表 + 9 项对账附录 — 协同参考,待创建)
- **代码基线**:
  - [crates/auto-dpo/src/generator.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs)([PreferencePairGenerator::generate()](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L94) — 通道 A 复用)
  - [crates/auto-dpo/src/types.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/types.rs)([PreferencePair](file:///D:/Chimera CLI/crates/auto-dpo/src/types.rs#L117) — 通道 A 类型)
  - [crates/gsoe-evolution/src/engine.rs](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs)([GsoeEvolutionEngine::evolve_once()](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs#L106) — 决策 3 扩展基线)
  - [crates/gsoe-evolution/src/types.rs](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs)([EvolutionPolicy](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L23) / [GrpoRollout](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L90) / [EvolutionResult](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L156) — 决策 3 谱系类型)
  - [crates/chimera-mas/src/invariants.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs)(InvariantChecker — 决策 2 INV-9 扩展基线)

---

> **维护者**: NEXUS-OMEGA 团队
> **创建日期**: 2026-07-23
> **基线版本**: v2.3.1-omega(创建时)
> **决策者**: E01 首席架构师 + E06 认知科学专家 + E07 任务调度专家(分布式评审)
> **分析团队**: 3 专家视角分布式深度分析(首席架构 + 认知科学 + 任务调度)
