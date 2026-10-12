# ADR-042: R2（GSOE×AutoDPO 约束 RL）在 FormalVerifier 落地前无条件冻结

## 状态

已批准 (Accepted) (2026-07-25)

> **状态说明**: 本 ADR 于 2026-07-25 由 P4-W16.2.3 子任务创建并批准。本 ADR 是 ADR-032 决策 4（验证器层级跃迁路径）的**工程实施层具体化与硬约束落地**——ADR-032 决策 4 在 RHI-CG 双通道评估器架构层面定义了 R2 冻结规则,本 ADR 将其落地为代码层面的硬约束、解冻流程与违反处置预案。属 append-only 扩展(对齐 ADR-028 决策 1 哲学),不修改 ADR-032 的裁决结论。

## 背景

- **v5.0 设计文档 §7.5 经验回放池** 明确指出:"离线 RL 两接缝(R1 召回配额 CQL/IQL、R2 GSOE×AutoDPO 约束 RL)维持 v4.0 设计,**R2 在 FormalVerifier 落地前无条件冻结**;回放池 ≥10K 轨迹 + 影子模式 2 周为 R1 解冻前置。"
- **spec.md line 397**(Scenario: Harness lineage 胜率)定义:"R2(GSOE×AutoDPO 约束 RL)在 FormalVerifier 落地前无条件冻结"作为 P5 验收条件之一。
- **ADR-032 决策 4**(验证器层级跃迁路径)定义了 R2 冻结的高层规则:"在 FormalVerifier 落地前,通道 B 的 L3 执行反馈是最高验证器层级。**禁止**在 R2 冻结期内将 L2 LLM 评判单独作为部署依据(防奖励黑客,L2 模型自评偏差不可控)。"
- **§3.4.5 三重悖论红线**(进化悖论红线):"当前 GSOE/AutoDPO 使用执行反馈(测试通过/失败)作为验证信号,属于验证器层级 L3(执行反馈),存在被'奖励黑客'游戏化风险;第三阶段需向 L4(形式化验证)或 L5(人类研究判断)跃迁"。
- **当前代码基线**(v2.3.1-omega → v2.4.0-omega 实际发布,2026-07-26 核实):
  - [crates/gsoe-evolution/src/engine.rs](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs) 已实现 [GsoeEvolutionEngine::evolve_once()](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs#L106) — 基于执行反馈的进化主路径(L3 验证器),未实现 R2 路径(GSOE×AutoDPO 约束 RL)。
  - [crates/auto-dpo/src/generator.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs) 已实现 [PreferencePairGenerator::generate()](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L94) — 生成偏好对(P.F[]),供通道 A 提议使用,未与 R2 路径对接。
  - [crates/omega-learner/src/replay_pool.rs](file:///D:/Chimera CLI/crates/omega-learner/src/replay_pool.rs) 已实现 `ReplayPool<T>`(P4-W16.2.1,34 个单元测试全绿) — 经验回放池基础设施,供离线 RL 训练使用。
  - [crates/model-router/src/trajectory.rs](file:///D:/Chimera CLI/crates/model-router/src/trajectory.rs) 已实现 `RecordingHook` + `TrajectoryEvent`(P4-W16.1.2) — 轨迹捕获点 1。
  - [crates/quest-engine/src/trajectory_exporter.rs](file:///D:/Chimera CLI/crates/quest-engine/src/trajectory_exporter.rs) 已实现 `export_trajectory()`(P4-W16.1.3) — 轨迹捕获点 2,从 Checkpoint 导出 RL 四元组。
  - **FormalVerifier 尚未落地**:35 crate × 624 .rs 文件零命中 `FormalVerifier` / `formal_verifier`,L4 验证器层级未实现。
- **风险矩阵 R8**(P4_P5_IMPLEMENTATION_PLAN.md 第 293 行):"R2 误解冻 | 低概率 | 高影响 | ADR 硬编码 + FormalVerifier 评审前置 | R2 解冻必须经 ADR + FormalVerifier 落地"。
- 经 E01 首席架构师 + E02 安全架构师 + E06 认知科学专家分布式深度分析与多轮交叉验证,确认 R2 冻结需作为独立 ADR 落档,理由:① ADR-032 决策 4 是 RHI-CG 双通道架构层面的规则,需独立 ADR 承载工程实施细节(冻结范围、解冻流程、违反处置);② R8 风险(误解冻)需可追溯的硬约束文档;③ P4-W16.2.3 任务明确要求"写入 ADR"。
- 本 ADR 记录 R2 冻结的 5 项工程实施决策,作为 P4-W16.2.3 验收依据与 P5 阶段 RHI-CG 实施的前置约束。

> **ADR 编号确认**: `docs/architecture/adr_index.md` 现有最大编号为 ADR-037(2026-07-25 核实)。`P4_P5_IMPLEMENTATION_PLAN.md` 第 681-688 行预占 ADR-036~041 编号分配(ADR-036 omega-learner LinUCB / ADR-038 Harness-as-Spec TOML / ADR-039 SpecRegistry 版本化 / ADR-040 RHI-CG 双通道架构 / ADR-041 ImmuneSystem facade)。本 ADR 编号确认为 **ADR-042**,作为下一个连续编号,与既有规划不冲突。本 ADR 落地后,原规划 ADR-040(RHI-CG 双通道架构)主题已由 ADR-032 承载,可在 P5-W17.1 启动时合并或释放编号。

## 决策

经专家团队多轮结构化思考与多路径交叉验证,对 R2(GSOE×AutoDPO 约束 RL)冻结作出以下 5 项工程实施决策:

### 决策 1: R2 冻结范围 — GSOE×AutoDPO 约束 RL 路径完全禁用

**冻结的 R2 路径定义**:

R2 = "GSOE×AutoDPO 约束 RL" 是 v5.0 设计文档 §7.5 定义的离线 RL 两接缝之一,具体指:

- **R2 输入**:经验回放池(P4-W16.2.1 `ReplayPool<T>`)中累积的 RL 轨迹四元组(state/action/reward/context)
- **R2 算法**:基于 GSOE 适应度评估 + AutoDPO 偏好对的约束强化学习(Constrained RL),输出约束策略
- **R2 输出**:经约束 RL 训练的策略参数,拟注入 GsoeEvolutionEngine 的进化谱系
- **R2 验证器层级**:L3(执行反馈),未达 L4(形式化验证)

**冻结范围**(本 ADR 生效期间完全禁用):

1. **代码层面**:禁止在 `crates/gsoe-evolution/` 或 `crates/auto-dpo/` 中实现 R2 约束 RL 训练路径(包括但不限于:`evolve_with_constrained_rl()` 方法、`ConstrainedRLPolicy` 类型、`train_r2_path()` 函数等)
2. **数据层面**:禁止将经验回放池的轨迹数据用于 R2 约束 RL 训练(回放池仅可用于 R1 召回配额 CQL/IQL 训练,且 R1 需满足影子模式 2 周前置)
3. **部署层面**:禁止通过任何方式(运行时旗、能力场、配置开关)激活 R2 路径——即便代码意外实现,也必须通过编译期 feature gate 默认关闭
4. **事件层面**:禁止发布与 R2 相关的 `NexusEvent` 变体(如 `R2PolicyUpdated` / `ConstrainedRLTrained`)

**不在冻结范围的项**(澄清):

- **R1(召回配额 CQL/IQL)**:不在本 ADR 冻结范围,但解冻前置条件为"影子模式 2 周"(P4-W16.2.4)
- **RHI-CG 通道 A(提议,L2 验证器)**:不在本 ADR 冻结范围,可在 P5-W17.1 实施
- **RHI-CG 通道 B(否决,L3 验证器)**:不在本 ADR 冻结范围,可在 P5-W18.1 实施
- **经验回放池本身(P4-W16.2.1)**:已落地,不在冻结范围,但仅可用于 R1 路径与 RHI-CG 双通道
- **GsoeEvolutionEngine::evolve_once()**:既有 L3 进化主路径,不在冻结范围

### 决策 2: 冻结期限 — FormalVerifier 落地前无条件冻结

**冻结期限定义**:R2 冻结期限为"FormalVerifier 落地前"——这是一个事件触发型期限,非时间触发型。

**FormalVerifier 落地的判定标准**(必须同时满足):

1. **代码落地**:`crates/seccore/src/formal_verifier.rs` 或独立 crate(如 `formal-verifier`)实现完成,通过 `cargo check` / `cargo test` / `cargo clippy -D warnings` 全绿
2. **ADR 落档**:新建 ADR(预计 ADR-043 或后续编号)记录 FormalVerifier 的设计决策(算法选型、属性语言、证明器集成)
3. **L4 验证能力证明**:至少 1 个 spec 属性(如"INV-9 在所有委托图下成立"或"HCW selector 权重向量在所有输入下满足归一化约束")通过 FormalVerifier 的形式化证明
4. **集成测试**:FormalVerifier 与 GsoeEvolutionEngine 集成测试通过,L4 验证可作为通道 B 的否决门之一
5. **架构评审通过**:经 E01 首席架构师 + E02 安全架构师 + E06 认知科学专家三方评审通过,出具书面评审报告

**"无条件冻结"的语义澄清**:

- **无条件**:不附加任何前置条件(如"在 X 时间后" / "在 Y 任务完成后" / "在 Z 指标达成后")——只有 FormalVerifier 落地是唯一解冻触发器
- **冻结**:R2 路径在代码、数据、部署、事件四个层面均不可激活(决策 1)
- **例外**:无任何例外,包括但不限于"实验性验证" / "影子模式运行" / "限定场景试用"——R2 在解冻前**完全不存在于运行时**

**冻结起始时间**:2026-07-25(本 ADR 批准日期)

**预计解冻时间**:2026-10-15(Action Item A10:FormalVerifier 设计评审,决定 R2 解冻时间)

> **关键约束**:即便 A10 评审通过,解冻仍需新建 ADR(预计 ADR-044 或后续编号)正式记录解冻决策,本 ADR 不可被"自动解冻"。

### 决策 3: 解冻条件 — 三阶递进式解冻流程

R2 解冻不可"一步解冻",必须经三阶递进式流程,确保奖励黑客风险可控:

**阶段 1: FormalVerifier 落地验证(前置条件)**

- 完成 ADR-042 决策 2 的 5 项 FormalVerifier 落地判定标准
- 输出:FormalVerifier 落地证明(代码 + ADR + 集成测试 + 评审报告)

**阶段 2: R2 解冻 ADR 评审(决策门)**

- 新建 ADR(预计 ADR-044 或后续编号)记录 R2 解冻决策
- ADR 必须包含:
  1. R2 约束 RL 算法选型(CQL / IQL / 其他)与理论依据
  2. R2 与 FormalVerifier 的集成路径(L4 验证如何约束 R2 训练)
  3. R2 奖励函数设计(必须含 ≥L3 执行反馈信号,对齐 ADR-032 决策 5)
  4. R2 影子模式 2 周验证计划(对齐 R1 的影子模式前置)
  5. R2 解冻后的回滚预案(若发现奖励黑客,如何回滚)
- ADR 评审通过:E01 首席架构师 + E02 安全架构师 + E06 认知科学专家三方书面批准

**阶段 3: R2 影子模式 2 周验证(后置条件)**

- R2 解冻后进入影子模式运行 2 周(对齐 R1 的影子模式前置)
- 影子模式期间:
  - R2 训练策略不部署到生产路径(仅记录与对比)
  - 每日输出对比报告:R2 策略 vs L3 主路径策略的胜率、奖励黑客检测指标、稳定性指标
  - 若发现 R2 策略在 L2 评判分数持续上升但 L3 执行反馈无改善,触发 `NexusEvent::RewardHackingSuspected`(ADR-032 决策 5)
- 影子模式 2 周后,经评审通过,R2 正式激活

**解冻失败的处置**:

- 阶段 1 失败:FormalVerifier 未落地 → R2 维持冻结
- 阶段 2 失败:ADR 评审未通过 → R2 维持冻结,返回阶段 1 重新设计
- 阶段 3 失败:影子模式发现奖励黑客 → 立即回滚 R2 激活,R2 重新冻结,返回阶段 2 重新评审

### 决策 4: 违反处置预案 — 自动回滚 + 告警 + 事故复盘

若 R2 在冻结期内被意外或恶意激活(违反本 ADR),触发以下处置预案:

**检测机制**:

1. **CI 检测**:`.github/workflows/release.yml` 新增 `r2-freeze-guard` job,扫描 `crates/gsoe-evolution/` 与 `crates/auto-dpo/` 源码,若发现 R2 路径实现(关键词匹配:`constrained_rl` / `r2_policy` / `train_r2` / `GsoeAutoDpoRL`),CI 失败阻止合并
2. **运行时检测**:`crates/gsoe-evolution/src/engine.rs` 在 `evolve_once()` 入口处断言 R2 路径未激活(`debug_assert!(!cfg!(feature = "r2_path"))`),违反时 panic 阻止进化
3. **审计检测**:`crates/seccore/src/asa.rs` AsaAuditor 周期性扫描进化路径,若发现 R2 路径激活痕迹,发布 `NexusEvent::R2FreezeViolation` (Critical 级,走 mpsc 旁路通道,对齐 §6.2 红线)

**处置流程**(三步):

1. **自动回滚**(立即,~1 分钟):
   - `git revert` 最近一次涉及 R2 路径的 commit
   - `cargo build --workspace` 验证回滚后构建通过
   - 若回滚失败,触发 `NexusEvent::R2FreezeRollbackFailed` (Critical 级)
2. **告警广播**(立即,~5 分钟):
   - 发布 `NexusEvent::R2FreezeViolation` Critical 事件
   - 通知 E01 首席架构师 + E02 安全架构师 + E06 认知科学专家
   - 冻结相关 PR 合并权限(24 小时)
3. **事故复盘**(24 小时内):
   - 输出事故复盘报告:违反原因、影响范围、修复方案、预防措施
   - 报告归档至 `docs/audit/r2_freeze_violation_<date>.md`
   - 更新 CI 检测规则,防止同类违反再次发生

**处置优先级**:P0(最高,阻断一切其他工作)

### 决策 5: 工程实施层面的硬约束 — 代码标记 + 文档同步 + CI 检查

R2 冻结在工程实施层面的硬约束:

**1. 代码标记**(编译期可见):

- `crates/gsoe-evolution/src/engine.rs` 顶部新增模块级注释:
  ```rust
  //! ## R2 冻结声明(ADR-042)
  //!
  //! **冻结状态**:R2(GSOE×AutoDPO 约束 RL)路径在 FormalVerifier 落地前无条件冻结。
  //! **冻结依据**:ADR-042(2026-07-25 批准)
  //! **解冻条件**:FormalVerifier 落地 + 新 ADR 评审 + 影子模式 2 周验证
  //! **违反处置**:自动回滚 + 告警 + 事故复盘(ADR-042 决策 4)
  ```
- `crates/auto-dpo/src/generator.rs` 顶部新增同样注释
- `crates/omega-learner/src/replay_pool.rs` 顶部新增注释:回放池数据**仅可用于 R1 路径**,R2 路径冻结期间禁用

**2. 文档同步**(本 ADR 批准后 24 小时内):

- [CHANGELOG.md](file:///D:/Chimera CLI/CHANGELOG.md) 新增"ADR-042 R2 冻结"条目,记录冻结决策与解冻条件
- [docs/architecture/CODE_WIKI.md](file:///D:/Chimera CLI/docs/architecture/CODE_WIKI.md) gsoe-evolution / auto-dpo / omega-learner 条目补 R2 冻结声明
- [docs/architecture/adr_index.md](file:///D:/Chimera CLI/docs/architecture/adr_index.md) 新增 ADR-042 条目
- [.trae/rules/nuxus规则.md](file:///D:/Chimera CLI/.trae/rules/nuxus规则.md) §6.2 Week 1-8 实战新红线新增"R2 冻结红线"(可选,视规则膨胀情况)
- [.trae/specs/nexus-omega-v5-implementation-plan/tasks.md](file:///D:/Chimera CLI/.trae/specs/nexus-omega-v5-implementation-plan/tasks.md) P4-W16.2.3 标记为完成,引用本 ADR

**3. CI 检查**(`.github/workflows/release.yml`):

- 新增 `r2-freeze-guard` job(在 build/test 之后,非主干阻塞):
  - 扫描 `crates/gsoe-evolution/src/` 与 `crates/auto-dpo/src/` 源码
  - 关键词匹配(大小写不敏感):`constrained_rl` / `r2_policy` / `train_r2` / `GsoeAutoDpoRL` / `evolve_with_constrained_rl`
  - 若发现匹配,CI 失败并输出违反报告
  - 例外:在 `tests/` 目录下的测试代码允许引用 R2 关键词(用于测试 R2 冻结本身)
- 新增 `r2-freeze-assert` 集成测试(`crates/gsoe-evolution/tests/r2_freeze_guard.rs`):
  - 断言 `GsoeEvolutionEngine` 未实现 `evolve_with_constrained_rl` 方法(通过 `cargo doc` 或 trait 检查)
  - 断言 `ReplayPool<T>` 的 API 文档明确声明"R2 路径冻结期间禁用"
  - 断言 `Cargo.toml` 中 `gsoe-evolution` 与 `auto-dpo` 未启用 `r2_path` feature

## 理由

### 决策 1 理由(R2 冻结范围)

- **§3.4.5 进化悖论红线**:R2(GSOE×AutoDPO 约束 RL)使用 GSOE 适应度 + AutoDPO 偏好对作为约束信号,验证器层级为 L3(执行反馈),存在被"奖励黑客"游戏化风险。在 L4 形式化验证落地前,R2 路径完全禁用是结构性防御。
- **§6.1 红线(裸奔)**:Claude Code 尸检教训"命令插值 + auth 跳过"。R2 路径若被意外激活,等同于 RL 系统的"auth 跳过"——进化策略学会绕过 L3 验证器而非真正改进代码质量。完全禁用是反绕过的最严格约束。
- **代码层面禁用的必要性**:仅文档层面禁止不足以防止意外实现——开发者可能在不知情的情况下为 GsoeEvolutionEngine 添加 `evolve_with_constrained_rl` 方法。代码标记 + CI 检查(决策 5)是工程层面的硬约束。
- **数据层面禁用的必要性**:即便代码未实现 R2 路径,若回放池数据被误用于 R2 训练(如通过外部脚本),仍构成违反。回放池 API 文档必须明确禁用。
- **事件层面禁用的必要性**:NexusEvent 是模块间通信的唯一通道(§2.2 依赖铁律),若发布 R2 相关事件,会触发下游模块的 R2 路径激活。事件层面禁用是传播阻断。

### 决策 2 理由(冻结期限)

- **"无条件"的语义严格性**:若允许任何前置条件(如"在 X 时间后" / "在 Y 任务完成后"),会引入"条件满足但 FormalVerifier 未落地"的灰色地带,增加误解冻风险(R8)。无条件冻结是最严格的冻结语义。
- **FormalVerifier 落地的 5 项判定标准**:每项标准都是可证伪的客观证据(代码 / ADR / 测试 / 评审报告),不依赖主观判断。5 项同时满足确保 FormalVerifier 不仅"代码存在",而且"能力被证明"。
- **预计解冻时间 2026-10-15**:基于 P4_P5_IMPLEMENTATION_PLAN.md Action Item A10,FormalVerifier 设计评审定于 W17(2026-10-15)。这是预计时间,非承诺时间——若评审未通过或 FormalVerifier 未落地,R2 维持冻结。
- **冻结起始时间 2026-07-25**:本 ADR 批准日期,与 P4-W16.2.3 任务完成日期一致。

### 决策 3 理由(三阶递进式解冻)

- **阶段 1 前置条件的必要性**:FormalVerifier 落地是 R2 解冻的硬前置,本 ADR 决策 2 已定义。阶段 1 验证 5 项判定标准全部满足,确保 FormalVerifier 真正可用。
- **阶段 2 ADR 评审的必要性**:R2 解冻涉及算法选型(CQL / IQL / 其他)、奖励函数设计、集成路径等复杂决策,需独立 ADR 承载。三方评审(E01/E02/E06)确保架构、安全、认知科学三视角全覆盖。
- **阶段 3 影子模式 2 周的必要性**:对齐 R1 的影子模式前置(P4-W16.2.4),R2 解冻后也需影子模式验证。影子模式期间 R2 策略不部署到生产路径,仅记录与对比,确保奖励黑客风险可控。
- **解冻失败回滚的设计**:三阶递进式流程的每一阶段都有失败处置(返回上一阶段或重新冻结),确保解冻过程可回滚。这是"长期主义"原则的体现——不为短期通过牺牲长期可维护性。
- **与 ADR-032 决策 4 的关系**:ADR-032 决策 4 定义了 L4 形式化验证跃迁条件("L4 通过的 spec 候选可跳过 L3 的连续 3 次回归要求"),本 ADR 决策 3 的阶段 1 是 ADR-032 决策 4 的前置条件。

### 决策 4 理由(违反处置预案)

- **三重检测机制的必要性**:
  - CI 检测:开发阶段的静态防护,阻止 R2 路径代码合并
  - 运行时检测:运行阶段的动态防护,阻止 R2 路径激活
  - 审计检测:周期性扫描,发现遗漏的 R2 路径痕迹
- **Critical 级事件走 mpsc 旁路通道**:`R2FreezeViolation` 是 Critical 级事件(对齐 §6.2 红线"Critical 安全事件用 mpsc"),必须用 mpsc channel 确保送达,不走 broadcast(可能丢弃)。
- **三步处置流程的设计**:
  - 自动回滚(立即):减少违反影响窗口
  - 告警广播(立即):通知专家团队介入
  - 事故复盘(24 小时内):防止同类违反再次发生
- **P0 优先级**:R2 违反等同于安全事件,优先级最高,阻断一切其他工作。

### 决策 5 理由(工程实施硬约束)

- **代码标记的必要性**:模块级注释是开发者最先看到的文档,在 `engine.rs` / `generator.rs` / `replay_pool.rs` 顶部新增 R2 冻结声明,确保开发者在修改这些文件时第一时间知晓冻结状态。
- **文档同步的必要性**:CHANGELOG / CODE_WIKI / adr_index / tasks.md 是项目的权威文档,必须 24 小时内同步,确保所有团队成员知晓 R2 冻结决策。
- **CI 检查的必要性**:仅靠文档与注释不足以防止违反——开发者可能在不知情的情况下添加 R2 路径代码。CI 检查是机械性硬约束,不依赖人的自觉。
- **CI job 非主干阻塞的设计**:`r2-freeze-guard` job 在 build/test 之后执行,失败不阻塞正常 release(仅阻塞 R2 相关 PR)。这避免 CI 检查对正常开发流程造成干扰,同时确保 R2 路径代码无法合并。
- **集成测试的必要性**:CI 静态扫描可能漏检(如 R2 路径代码用其他命名),集成测试通过 trait 检查与 API 文档断言,提供运行时层面的防护。

## 影响

### 新增内容

- **新增 NexusEvent 变体**:2 个(共 75 → 77)
  - `R2FreezeViolation { metadata, violation_type, evidence }`(Critical 级,走 mpsc 旁路通道)— 决策 4 违反检测
  - `R2FreezeRollbackFailed { metadata, reason }`(Critical 级,走 mpsc 旁路通道)— 决策 4 回滚失败
- **新增 MasError 变体**:1 个(共 34 → 35)
  - `R2FreezeViolation { violation_type, evidence }`(决策 4 违反处置)
- **新增 CI job**:`.github/workflows/release.yml` 新增 `r2-freeze-guard` job(在 build/test 之后,非主干阻塞)
- **新增集成测试**:`crates/gsoe-evolution/tests/r2_freeze_guard.rs`(断言 R2 路径未实现 + API 文档禁用声明)
- **新增代码标记**:`crates/gsoe-evolution/src/engine.rs` / `crates/auto-dpo/src/generator.rs` / `crates/omega-learner/src/replay_pool.rs` 顶部新增 R2 冻结声明注释
- **新增 audit 文档目录**:`docs/audit/`(已存在,无需创建)— R2 违反事故复盘报告归档位置
- **新增 ADR**:本 ADR(ADR-042)

### 修改内容

- **`crates/event-bus/src/types.rs`**:新增 `R2FreezeViolation` / `R2FreezeRollbackFailed` 事件变体(75 → 77,NexusEvent 变体数对齐 CHANGELOG)
- **`crates/chimera-mas/src/error.rs`**:新增 `R2FreezeViolation` 变体(34 → 35)
- **`crates/gsoe-evolution/src/engine.rs`**:顶部新增 R2 冻结声明模块级注释
- **`crates/auto-dpo/src/generator.rs`**:顶部新增 R2 冻结声明模块级注释
- **`crates/omega-learner/src/replay_pool.rs`**:顶部新增 R2 冻结声明模块级注释(回放池数据仅可用于 R1)
- **`.github/workflows/release.yml`**:新增 `r2-freeze-guard` job
- **`CHANGELOG.md`**:新增"ADR-042 R2 冻结"条目
- **`docs/architecture/CODE_WIKI.md`**:gsoe-evolution / auto-dpo / omega-learner 条目补 R2 冻结声明
- **`docs/architecture/adr_index.md`**:新增 ADR-042 条目
- **`.trae/specs/nexus-omega-v5-implementation-plan/tasks.md`**:P4-W16.2.3 标记为完成,引用本 ADR

### 资源影响评估

| 维度 | 评估 |
|------|------|
| crate 数量 | 35(不变,增量在既有 crate 内) |
| 依赖变更 | 无新增外部依赖(仅新增事件变体与错误变体) |
| Docker/binary 体积 | 不受影响(纯 Rust 新增代码,无 unsafe 依赖) |
| NexusEvent 变体数 | 75 → 77(新增 `R2FreezeViolation` / `R2FreezeRollbackFailed`,Critical 级走 mpsc) |
| MasError 变体数 | 34 → 35(新增 `R2FreezeViolation`) |
| 测试覆盖 | 新增 ~10 个测试(r2_freeze_guard 集成测试) |
| CI 时间 | `r2-freeze-guard` job 增加 ~30 秒(源码扫描 + 集成测试),非主干阻塞 |
| 版本号 | 不变(本 ADR 是约束性决策,非功能性新增) |

## 考虑的方案

### 方案 A: 完全禁用 R2 路径(采纳)

- **内容**:在代码、数据、部署、事件四个层面完全禁用 R2 路径,FormalVerifier 落地前无任何例外。
- **采纳理由**:
  1. 最严格的防御措施,完全消除奖励黑客风险
  2. 工程实施简单(代码标记 + CI 检查即可)
  3. 与 v5.0 设计文档 §7.5 "无条件冻结"语义一致
  4. 与 ADR-032 决策 4 验证器层级跃迁路径一致

### 方案 B: 影子模式运行 R2(否决)

- **内容**:允许 R2 路径在影子模式运行(不部署到生产路径,仅记录与对比),FormalVerifier 落地后正式激活。
- **否决理由**:
  1. **违反"无条件冻结"语义**:v5.0 设计文档明确"无条件冻结",影子模式属于"有条件运行"
  2. **影子模式仍需 R2 路径实现**:R2 影子模式要求 R2 算法代码存在,这本身已构成"R2 路径激活"的潜在风险
  3. **奖励黑客风险**:影子模式期间 R2 策略可能已学会绕过 L3 验证器,即便不部署,算法本身的游戏化风险已存在
  4. **R1 已有影子模式前置**:R1 的影子模式 2 周前置(P4-W16.2.4)是 R1 解冻条件,不是 R2 解冻条件。R2 需更严格的前置(FormalVerifier 落地)

### 方案 C: 限定场景启用 R2(否决)

- **内容**:在特定场景(如低风险任务、非生产环境)允许 R2 路径激活,其他场景冻结。
- **否决理由**:
  1. **"限定场景"是功能旗**:§6.1 红线"禁止功能标志,用能力场自然进化替代"。"限定场景启用"本质是运行时旗,违反 C4 决策
  2. **场景边界难以界定**:何为"低风险任务"?何为"非生产环境"?边界模糊会导致 R2 路径在不应激活的场景被激活
  3. **奖励黑客不区分场景**:R2 策略学会的绕过行为可迁移到任何场景,限定场景启用无法防止风险扩散
  4. **与 ADR-032 决策 4 冲突**:ADR-032 决策 4 明确"L2 不可单独作为部署依据",限定场景启用 R2 等同于 L2 单独部署,违反该决策

## 合规性

- **§2.1 分层映射**:符合。本 ADR 不改变分层结构,增量在 `gsoe-evolution`(L5)/ `auto-dpo`(L5)/ `omega-learner`(L6)/ `event-bus`(L1)/ `chimera-mas`(L9)内,不改分层。
- **§2.2 依赖铁律**:符合。R2 冻结是约束性决策,不引入新的跨层依赖。`R2FreezeViolation` 事件走 EventBus 异步广播(§6.2 红线"学习层走异步广播"),Critical 级走 mpsc 旁路通道。
- **§3.3.1 第 1 条(OMEGA 四定律守恒)**:符合。Ω-Evolve 由 gsoe-evolution 单一实现,R2 冻结不改变 Ω-Evolve 的实现位置,仅约束其进化路径。
- **§3.3.1 第 4 条(领域类型稳定性)**:符合。不改 `UserIntent` / `Quest` / `Checkpoint` / `OmniSparseMasks` / `CLV` / `NexusState`。
- **§3.3.1 第 5 条(向后兼容)**:符合。append-only 扩展(新增事件变体 + 错误变体 + 注释 + CI job),不修改既有 API 签名。
- **§3.3.1 第 6 条(新 crate 准入)**:符合。不新建 crate。
- **§3.4.1 第 6 条(性能可证伪)**:符合。R2 冻结本身是约束性决策,无性能 claim。CI job 的扫描时间(~30 秒)是可测量的。
- **§3.4.1 第 7 条(学术支撑落地)**:符合。R2 冻结的学术依据是进化悖论红线(§3.4.5)+ 奖励黑客风险(AI Alignment 文献)。
- **§3.4.5 三重悖论红线(进化悖论)**:符合。R2 冻结是进化悖论红线(验证器层级 L3→L4 跃迁)的工程实施层面落地。
- **§4.1 编码规范**:符合。`#![forbid(unsafe_code)]` 保持;库层 thiserror;无生产路径 unwrap/expect;单函数 ≤200 行。
- **§4.4 async 反模式**:符合。不持锁跨 .await;Critical 级事件走 mpsc 旁路通道;`R2FreezeViolation` 事件发布用 `publish_blocking`(sync 方法)或 `publish_critical().await`(async 方法)。
- **§6.1 架构红线**:符合。不引入功能旗(决策 1 完全禁用,非"限定场景启用");单函数 ≤200 行;async 必须 await 或 spawn 管理。
- **§6.2 Week 1-8 实战新红线**:符合。`R2FreezeViolation` / `R2FreezeRollbackFailed` 为 Critical 级,走 mpsc 旁路通道(对齐红线 5);不持锁 .await。
- **C2 决策(嫁接 auto-dpo / gsoe-evolution)**:符合。R2 冻结不影响 C2 嫁接决策,仅约束 R2 路径的激活。
- **C4 决策(灰度走能力场)**:符合。R2 冻结是"完全禁用",不涉及灰度部署。R2 解冻后的灰度走 decay-engine 能力场(CapabilityToken),非运行时旗。
- **ADR-026 / ADR-028 既有决策**:全部保持。`MasError` 变体扩展沿用 append-only(ADR-028 决策 1);`NexusEvent` 变体扩展沿用 append-only(ADR-026)。
- **ADR-032 决策 4(验证器层级跃迁路径)**:符合。本 ADR 是 ADR-032 决策 4 的工程实施层具体化,不修改其裁决结论。R2 冻结规则(ADR-032 决策 4)在本 ADR 中扩展为 5 项工程实施决策。
- **ADR-037(CapabilityToken 灰度工程化)**:符合。R2 冻结期间不涉及 CapabilityToken,R2 解冻后的灰度走 ADR-037 的 CapabilityToken 机制。

## 相关文档

- **设计文档**: [NEXUS-OMEGA_v5.0_系统性完整设计文档.md](file:///D:/Chimera CLI/NEXUS-OMEGA_v5.0_系统性完整设计文档.md) §7.5 经验回放池 — R2 冻结设计源
- **规则**: [.trae/rules/nuxus规则.md](file:///D:/Chimera CLI/.trae/rules/nuxus规则.md) §2.1(分层映射)/§2.2(依赖铁律)/§3.3.1(第二阶段开发原则)/§3.4.1(第三阶段开发原则)/§3.4.5(三重悖论红线)/§4.1(编码规范)/§4.4(async 反模式)/§6.1(架构红线)/§6.2(Week 1-8 新红线)
- **spec**: [.trae/specs/nexus-omega-v5-implementation-plan/spec.md](file:///D:/Chimera CLI/.trae/specs/nexus-omega-v5-implementation-plan/spec.md) line 397(Scenario: Harness lineage 胜率 — R2 冻结验收条件)
- **tasks**: [.trae/specs/nexus-omega-v5-implementation-plan/tasks.md](file:///D:/Chimera CLI/.trae/specs/nexus-omega-v5-implementation-plan/tasks.md) P4-W16.2.3(R2 冻结 ADR 落档)
- **CODE_WIKI.md**: [docs/architecture/CODE_WIKI.md](file:///D:/Chimera CLI/docs/architecture/CODE_WIKI.md) §3.1(crate 索引)/§2.3(ADR 表)
- **ADR 索引**: [docs/architecture/adr_index.md](file:///D:/Chimera CLI/docs/architecture/adr_index.md)(本 ADR 同步更新)
- **关联 ADR**:
  - [ADR-032](file:///D:/Chimera CLI/docs/architecture/ADR-032-dual-channel-evaluator.md)(RHI-CG 双通道评估器 — 决策 4 验证器层级跃迁路径,本 ADR 的上层依据)
  - [ADR-037](file:///D:/Chimera CLI/docs/architecture/ADR-037-capability-token-grayscale-engineering.md)(能力场灰度工程化 — R2 解冻后灰度部署机制)
  - [ADR-035](file:///D:/Chimera CLI/docs/architecture/ADR-035-threat-model-revision-wasmtime-restart.md)(威胁模型下修 — FormalVerifier 与沙箱安全模型的关系)
- **代码基线**:
  - [crates/gsoe-evolution/src/engine.rs](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs)([GsoeEvolutionEngine::evolve_once()](file:///D:/Chimera CLI/crates/gsoe-evolution/src/engine.rs#L106) — R2 冻结声明的承载位置)
  - [crates/auto-dpo/src/generator.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs)([PreferencePairGenerator::generate()](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L94) — R2 冻结声明的承载位置)
  - [crates/omega-learner/src/replay_pool.rs](file:///D:/Chimera CLI/crates/omega-learner/src/replay_pool.rs)(`ReplayPool<T>` — R2 冻结期间数据使用约束的承载位置)
  - [crates/event-bus/src/types.rs](file:///D:/Chimera CLI/crates/event-bus/src/types.rs)(NexusEvent — `R2FreezeViolation` / `R2FreezeRollbackFailed` 新增位置)
  - [crates/chimera-mas/src/error.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/error.rs)(MasError — `R2FreezeViolation` 新增位置)
- **P4_P5 实施计划**: [.trae/specs/nexus-omega-v5-implementation-plan/P4_P5_IMPLEMENTATION_PLAN.md](file:///D:/Chimera CLI/.trae/specs/nexus-omega-v5-implementation-plan/P4_P5_IMPLEMENTATION_PLAN.md) 第 67/111/293/398/440 行(R2 冻结相关上下文)

---

> **维护者**: NEXUS-OMEGA 团队
> **创建日期**: 2026-07-25
> **基线版本**: v2.3.1-omega(创建时,P4 阶段进行中)
> **决策者**: E01 首席架构师 + E02 安全架构师 + E06 认知科学专家(分布式评审)
> **分析团队**: 3 专家视角分布式深度分析(首席架构 + 安全架构 + 认知科学)
> **解冻责任方**: E01 首席架构师 + E02 安全架构师 + E06 认知科学专家(三方书面批准)
> **预计解冻评审时间**: 2026-10-15(Action Item A10:FormalVerifier 设计评审)
