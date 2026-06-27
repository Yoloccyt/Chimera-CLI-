# Week 6 适配 + 进化 + 多模态 开发方案 Spec

> **本周目标**:完成 SSRA · LSCT · GSOE · NMC · CHTC 五大 crate 从骨架到生产级实现,通过 Week 6 端到端验收,同步消化 Week 5 遗留 P1(文档失同步)/P2(AHIRT 配置化)问题,为 Week 7(MCP Mesh + 监控 + 集成)奠定健康基础。

---

## 1. 现状分析与基线

### 1.1 项目当前进度(截至 2026-06-26)

| 维度 | 当前状态 | 数据来源 |
|------|---------|---------|
| 已完成周次 | Week 1-5 全部验收通过 | project_memory / Week 5 验收报告 |
| 全量测试 | 2023 passed / 0 failed | Week 5 验收基线 |
| clippy 警告 | 0 warnings | Week 5 验收基线 |
| `#![forbid(unsafe_code)]` 覆盖 | 35/35 crate | Week 5 验收基线 |
| 安全免疫率 | 100%(100 载荷) | Week 5 验收基线 |
| CSA 端到端延迟 | < 300ms | Week 5 验收基线 |
| Workspace 成员数 | 34 crates | 根 Cargo.toml |
| Week 6 涉及 crate 状态 | 5/5 骨架(仅 lib.rs 模块文档 + `#![forbid(unsafe_code)]`) | crates/{ssra-fusion,lsct-tiering,gsoe-evolution,nmc-encoder,chtc-bridge}/src/lib.rs |

### 1.2 Week 1-5 验收标准对比分析

| 周次 | 主题 | 核心验收指标 | 关键交付 | 滚动到 Week 6 的约束 |
|------|------|-------------|---------|--------------------|
| Week 1 | L0-L1 基础设施 | Event Bus · SecCore · Decay · QEEP · CLI 入口 | 5 crate 完整实现 | EventBus API 稳定,publish/publish_blocking 双模 |
| Week 2 | L9+L5+L1 | Quest Engine · Repo Wiki · Model Router · CACR | Quest 分解流 + Wiki 沉淀 | Quest 类型作为 Week 6 SSRA 融合输入 |
| Week 3 | L5+L6 | MLC · HCW · CMT · OSA · KVBSR | 四级记忆 + 全维稀疏 | CMT 分层接口必须被 LSCT 复用 |
| Week 4 | L6+L7 | GEA · GQEP · PVL · MTPE · SCC · EDSB | 执行链路完整 | GQEP 聚集结果作为 SSRA 适配反馈源 |
| Week 5 | L8+L4+L3 | Parliament · ASA · AHIRT · TTG · DECB | 7700 行源码 + 2023 测试 | TTG/DECB/AHIRT 事件必须被 Week 6 订阅 |

**对比分析结论**:
- Week 1-5 累计完成 29 crate(L1-L9 横向铺开),Week 6 是首次触及 L10 Interface 与 L2 进化层
- Week 6 的 5 个 crate 横跨 5 个架构层(L2/L3/L5/L7/L10),是迄今跨层最广的一周,关键路径风险显著高于前几周
- Week 5 引入的 EventBus 事件(ConsensusReached/RedTeamAudit/BudgetAdjusted/AsaIntervention/AhirtProbeCompleted)是 Week 6 多个 crate 的输入源,事件契约稳定性是 Week 6 启动硬前置

### 1.3 Week 5 遗留问题清单(本周须处理)

| 编号 | 级别 | 描述 | 影响范围 | 本周处理策略 |
|------|------|------|---------|-------------|
| P1-1 | Major | CODE_WIKI.md 与 Week 5 实现失同步 | 文档可读性 | Task 7 同步更新 |
| P1-2 | Major | CHANGELOG.md Week 5 章节与实际功能/指标对应性未核验 | 文档可读性 | Task 7 同步核验 |
| P1-3 | Major | 5 个 crate 的 lib.rs 文档注释未含 Week 5 新增模块 | API 文档 | Task 7 同步补全 |
| P1-4 | Major | week5-parliament-security-budget spec 文档状态未核验 | Spec 一致性 | Task 7 同步核验 |
| P1-5 | Major | project_memory.md Week 5 经验教训时效性未核验 | 知识沉淀 | Task 7 同步核验 |
| P2-1 | Minor | AHIRT 5 分钟周期探测与 0.95 检测率阈值硬编码 | 配置灵活性 | Task 8 引入 AhirtConfig |
| Week5-Review-A | Minor | Week 5 复审 SubTask 2.5/2.6/3.4/3.6/4.2/4.4/5.3-5.8/6.2/6.4/7.1/7.2 未勾选 | 复审闭环 | Task 9 收尾核验 |

### 1.4 Week 6 涉及 crate 现状详表

| Crate | 架构层 | 当前 src/ 文件 | 当前代码行数 | 需新增模块(规划) |
|-------|--------|--------------|------------|-----------------|
| ssra-fusion | L7 Execution | lib.rs(13 行,仅文档) | 0 实现 | types/config/error/templates/fusion/engine |
| lsct-tiering | L3 Storage | lib.rs(13 行,仅文档) | 0 实现 | types/config/error/tiering/promoter/demoter |
| gsoe-evolution | L5 Knowledge | lib.rs(13 行,仅文档) | 0 实现 | types/config/error/policy/grpo/mutation/fitness |
| nmc-encoder | L2 Memory | lib.rs(13 行,仅文档) | 0 实现 | types/config/error/perceptors/text/image/video/audio/fusion |
| chtc-bridge | L10 Interface | lib.rs(13 行,仅文档) | 0 实现 | types/config/error/protocol/adapters/{vscode,idea,vim,emacs,zed}/bridge |

---

## 2. 反馈跟踪矩阵

### 2.1 多方反馈整合

| 反馈来源 | 内容描述 | 优先级 | 处理状态 | 对应 Task |
|---------|---------|--------|---------|----------|
| Week 5 复审(架构师) | CHTC L10→L7 直接调用 SSRA 风险 | Must | 待处理 | Task 5 设计强制 EventBus 解耦 |
| Week 5 复审(集成专家) | GSOE 进化需订阅 ConsensusReached 事件 | Must | 待处理 | Task 3 事件订阅契约 |
| Week 5 复审(并发专家) | SSRA 融合 < 20ms 须避免锁竞争 | Must | 待处理 | Task 1 性能基准 |
| Week 5 复审(测试专家) | NMC 多模态输入需覆盖异常格式 | Should | 待处理 | Task 4 错误路径测试 |
| Week 5 复审(文档专家) | 5 文档失同步问题(P1) | Must | 待处理 | Task 7 集中修复 |
| 产品建议 | CHTC 应预留 JetBrains IDE 适配扩展点 | Could | 待处理 | Task 5 adapter trait 设计 |
| 设计建议 | NMC 多模态融合后须输出统一 CLV(512-dim) | Must | 待处理 | Task 4 输出契约 |
| 测试建议 | Week 6 E2E 须覆盖 NMC→SSRA→CHTC 链路 | Must | 待处理 | Task 6 E2E |
| 客户反馈 | AHIRT 周期与阈值需可配置(P2) | Should | 待处理 | Task 8 配置化 |

---

## 3. 本周开发范围

### 3.1 5 大 crate 实现目标

#### 3.1.1 SSRA-fusion(L7 Execution,黏液式快速适配)

**设计来源**:GLM 5.2 slime 机制(2 天合并专家)+ ADR-022
**核心机制**:预编译适配器模板 + 运行时低延迟融合(< 20ms)
**关键类型**:
- `SlimeTemplate` — 预编译模板(含 capability_id, parameter_shape, fusion_strategy)
- `FusionRequest` — 融合请求(含 source_adapters, target_capability, deadline)
- `FusionResult` — 融合结果(含 fused_template, latency_ms, confidence)
- `SlimeFusionEngine` — 融合引擎核心

**性能硬指标**:单次融合延迟 ≤ 20ms(p95),通过预编译 + 零拷贝 + select_nth_unstable Top-K 实现

**对外事件**:
- 发布:`SsraFusionCompleted`(Normal,source = "ssra-fusion")
- 订阅:`ConsensusReached`(来自 Parliament,触发适配)、`RedTeamAudit`(来自 AHIRT,触发防御性适配)

#### 3.1.2 LSCT-tiering(L3 Storage,任务感知能力分层)

**设计来源**:GLM 5.2 LayerSplit + 创新点 36(LSCT 动态热层)
**核心机制**:按任务负载动态调整能力存储层级(热/温/冷/冰),编译任务升温、调试任务降温
**关键类型**:
- `TaskLoadProfile` — 任务负载画像(含 task_type, intensity, frequency)
- `TierAssignment` — 层级分配(含 capability_id, target_tier, reason)
- `LsctPromoter` — 升温器(冷→温→热)
- `LsctDemoter` — 降温器(热→温→冷→冰)
- `LsctCoordinator` — 协调器

**关键复用**:复用 CMT 四级分层接口(Week 3 已实现),LSCT 在 CMT 之上叠加任务感知策略

**对外事件**:
- 发布:`LsctTierSwitched`(Normal,source = "lsct-tiering")
- 订阅:`QuestActivated`(来自 Quest Engine,触发负载画像)

#### 3.1.3 GSOE-evolution(L5 Knowledge,在线进化)

**设计来源**:DeepSeek V4 GRPO + ADR-025
**核心机制**:GRPO 风格的在线强化学习,基于议会共识与红队审计生成策略更新
**关键类型**:
- `EvolutionPolicy` — 进化策略(含 mutation_rate, selection_pressure, elite_ratio)
- `GrpoRollout` — GRPO 采样轨迹
- `MutationCandidate` — 变异候选
- `FitnessReport` — 适应度报告
- `GsoeEvolutionEngine` — 进化引擎

**关键约束**:不引入真实模型推理(本周占位为基于规则的适应度评估,TODO Week 7 接入 MCP Mesh 真实模型)

**对外事件**:
- 发布:`GsoePolicyUpdated`(Normal,source = "gsoe-evolution")
- 订阅:`ConsensusReached`(来自 Parliament)、`RedTeamAudit`(来自 AHIRT)、`SsraFusionCompleted`(来自 SSRA,作为进化信号)

#### 3.1.4 NMC-encoder(L2 Memory,原生多模态上下文编码)

**设计来源**:Minimax M3 Native Multimodal + 创新点 5(MPP 多模态感知管道)+ ADR-016
**核心机制**:5 种模态感知器(文本/图像/视频/音频/桌面)→ 统一 CLV(512-dim f32)输出
**关键类型**:
- `PerceptionInput` — 多模态输入枚举(Text/Image/Video/Audio/Desktop)
- `CognitiveElement` — 认知元素(含 modality, content_hash, embedding)
- `MultimodalFusionEngine` — 多模态融合引擎
- `NmcEncoder` — 编码器入口

**关键约束**:
- 图像/视频/音频感知器本周占位(纯 Rust 实现成本过高,TODO Week 7/8 接入 ort ONNX Runtime)
- 文本与桌面感知器本周实现,其余提供 trait 接口 + 占位实现
- 输出必须为 CLV(512-dim f32),与 nexus-core 的 CLV 类型对齐

**对外事件**:
- 发布:`NmcEncoded`(Normal,source = "nmc-encoder")
- 无外部订阅(作为数据流入口,被动接收用户输入)

#### 3.1.5 CHTC-bridge(L10 Interface,跨平台工具兼容桥)

**设计来源**:Qwen 3.7 + ADR-020
**核心机制**:5 大 IDE 适配器(VSCode/IntelliJ/Vim/Emacs/Zed)+ 统一工具调用协议
**关键类型**:
- `UnifiedToolCall` — 统一工具调用(含 tool_id, parameters, ide_source)
- `IdeAdapter` trait — IDE 适配器接口
- `VscodeAdapter` / `IntelliJAdapter` / `VimAdapter` / `EmacsAdapter` / `ZedAdapter`
- `ChtcBridge` — 桥接器入口

**关键约束**:
- L10→下层严禁直接依赖,所有跨层调用通过 EventBus 或 MCP Mesh(Week 7)
- 本周实现协议层 + 5 个适配器的接口与基础调用转换,真实 IDE 集成测试留待 Week 7/8
- 5 个适配器必须通过 enum dispatch(避免 Box<dyn Trait>)

**对外事件**:
- 发布:`ChtcToolCallReceived`(Normal,source = "chtc-bridge")
- 订阅:`ChtcToolCallReceived`(自消费,触发工具路由)

### 3.2 非范围(本周明确不做)

- MCU 多语言代码理解(原 Week 6 Day 40):用户指定 5 模块,MCU 延后至 Week 7+
- NMC 图像/视频/音频感知器的真实模型推理:本周仅占位
- CHTC 真实 IDE 插件打包与发布:本周仅协议层 + 适配器骨架
- GSOE 真实 RL 训练循环:本周仅基于规则的适应度评估
- Week 5 复审未勾选检查项的全部修复:仅集中处理 P1/P2,其余 Minor 项结转

---

## 4. 团队组建与职责分配

### 4.1 专家团队(7 名子智能体)

| 角色 | 资质要求 | 负责范围 | 验收产出 |
|------|---------|---------|---------|
| Lead Architect(主导架构师) | 10 年+ Rust workspace + 事件驱动架构,熟悉 OMEGA 四定律 | 全局架构决策 + RACI 协调 + Critical 路径把控 | 架构决策记录 + 跨 crate 接口契约 |
| SSRA Specialist | 10 年+ 性能优化 + 模板元编程,熟悉 slime 机制 | ssra-fusion crate 全部实现 | SSRA 实现 + < 20ms 基准测试 |
| Storage Specialist | 10 年+ 分层存储 + 索引设计,熟悉 CMT 复用 | lsct-tiering crate 全部实现 | LSCT 实现 + 升降温测试 |
| RL/Evolution Specialist | 10 年+ 强化学习 + GRPO,熟悉策略梯度 | gsoe-evolution crate 全部实现 | GSOE 实现 + 进化循环测试 |
| Multimodal Specialist | 10 年+ 多模态感知 + 嵌入向量,熟悉 ONNX/CLV | nmc-encoder crate 全部实现 | NMC 实现 + CLV 输出测试 |
| Cross-Platform Specialist | 10 年+ IDE 插件 + 协议设计,熟悉 5 大 IDE 适配 | chtc-bridge crate 全部实现 | CHTC 实现 + 适配器测试 |
| QA/Docs Specialist | 10 年+ 测试工程 + 技术文档,熟悉 TDD/proptest | E2E + 文档同步 + P1/P2 修复 | E2E 测试 + 文档同步报告 |

### 4.2 RACI 责任矩阵

| Task | Lead Arch | SSRA Spec | Storage Spec | RL Spec | MM Spec | XP Spec | QA/Docs |
|------|----------|----------|-------------|---------|---------|---------|---------|
| Task 1 SSRA | A | R | C | I | I | I | C |
| Task 2 LSCT | A | I | R | I | I | I | C |
| Task 3 GSOE | A | I | I | R | I | I | C |
| Task 4 NMC | A | I | I | I | R | I | C |
| Task 5 CHTC | A | I | I | I | I | R | C |
| Task 6 E2E | C | C | C | C | C | C | R/A |
| Task 7 文档同步 | C | I | I | I | I | I | R/A |
| Task 8 AHIRT 配置化(P2) | C | I | I | I | I | I | R/A |
| Task 9 Week5 复审收尾 | C | I | I | I | I | I | R/A |
| Task 10 Week 6 验收 | R/A | C | C | C | C | C | C |

> R=Responsible(执行) / A=Accountable(负责) / C=Consulted(咨询) / I=Informed(知情)

### 4.3 协作机制

- **每日站会**(异步,15 分钟):每位专家通过 tasks.md 勾选状态 + 留言同步阻塞点
- **周例会**(同步,60 分钟):Lead Architect 主持,回顾进度偏差 + 调整优先级
- **紧急响应机制**(2 小时内响应):Critical 问题直接 SendMessage 给 Lead Architect,24 小时内提供解决方案
- **跨专家会审**:发现跨 crate 接口冲突时,触发 2-3 名相关专家的同步会审

---

## 5. 任务优先级(MoSCoW)

### 5.1 Must(必须做,阻塞 Week 6 验收)

- Task 1: SSRA-fusion 完整实现 + < 20ms 基准
- Task 2: LSCT-tiering 完整实现 + 升降温逻辑
- Task 3: GSOE-evolution 完整实现 + GRPO 风格循环
- Task 4: NMC-encoder 完整实现 + 文本/桌面感知器 + CLV 输出
- Task 5: CHTC-bridge 完整实现 + 5 适配器骨架 + 协议层
- Task 6: E2E 测试覆盖 NMC→SSRA→CHTC 主链路
- Task 10: Week 6 端到端验收

### 5.2 Should(应该做,本周完成)

- Task 7: 5 文档失同步修复(P1)
- Task 8: AHIRT 配置化(P2)

### 5.3 Could(可以做,资源允许时)

- CHTC JetBrains 适配扩展点预留
- NMC 图像感知器 ort ONNX 接口设计(不实现)

### 5.4 Won't(本周不做)

- MCU 多语言代码理解
- NMC 视频/音频感知器真实实现
- CHTC 真实 IDE 插件打包

---

## 6. 质量验收基准

### 6.1 功能验收

| 指标 | 目标值 | 验证方法 |
|------|--------|---------|
| 5 crate 完整实现 | 5/5 | cargo build --workspace 通过 |
| 全量测试通过率 | ≥ 99% | cargo test --workspace,允许 ≤ 5 个 flaky 重试 |
| 新增测试数 | ≥ 200 个(5 crate × 40+) | cargo test --workspace 统计 |
| 安全免疫率 | 100% | 复用 Week 5 安全免疫测试套件 + Week 6 新增 50 载荷 |
| E2E 测试 | ≥ 5 个用例 | tests/e2e/week6_*.rs |

### 6.2 性能验收

| 指标 | 目标值 | 验证方法 |
|------|--------|---------|
| SSRA 融合延迟(p95) | ≤ 20ms | criterion 基准(cargo bench 运行) |
| LSCT 升降温延迟(p95) | ≤ 50ms | criterion 基准 |
| GSOE 单轮进化延迟 | ≤ 500ms | criterion 基准 |
| NMC 文本编码延迟(p95) | ≤ 30ms | criterion 基准 |
| CHTC 工具调用转发延迟(p95) | ≤ 10ms | criterion 基准 |
| CSA 端到端延迟(Week 6 扩展) | ≤ 400ms(从 300ms 上浮 100ms,新增 NMC/CHTC) | E2E 测试 |

### 6.3 代码质量

| 指标 | 目标值 | 验证方法 |
|------|--------|---------|
| `cargo clippy --workspace -- -D warnings` | 0 warnings | 周末验收命令 |
| `#![forbid(unsafe_code)]` 覆盖率 | 40/40 crate(35 + 5 新实现) | Grep 验证 |
| 单函数长度 | ≤ 200 行 | 代码审查 |
| 非测试代码 unwrap/expect | 0(锁中毒场景使用 unwrap_or_else) | Grep 验证 |
| Box<dyn Trait> 使用 | ≤ 3 处(必须有 ADR 理由) | Grep 验证 |
| WHY 注释覆盖率 | 关键决策点 100% | 代码审查 |

### 6.4 架构合规

| 指标 | 目标值 | 验证方法 |
|------|--------|---------|
| 依赖方向违规 | 0 | Cargo.toml 依赖审查 + L10→下层仅走 EventBus |
| 跨层通信合规 | 100% 走 EventBus | Grep 验证无跨层直接 import |
| 新增事件类型 | ≥ 5 个(SsraFusionCompleted/LsctTierSwitched/GsoePolicyUpdated/NmcEncoded/ChtcToolCallReceived) | event-bus/src/types.rs 验证 |
| Critical 事件背压 | 100%(本周新增事件无 Critical 级) | event-bus 验证 |

---

## 7. 风险评估与缓释

### 7.1 风险登记表

| 风险 ID | 风险描述 | 可能性 | 影响程度 | 优先级 | 应对措施 |
|---------|---------|--------|---------|--------|---------|
| R1 | SSRA < 20ms 融合延迟无法达成 | 中 | 高 | P0 | 预编译模板 + select_nth_unstable + 零拷贝;若不达标,引入 DMP(Data Memory Prefetch) |
| R2 | CHTC L10→L7 直接依赖 SSRA 违反架构 | 高 | 高 | P0 | 强制 EventBus 解耦;Lead Architect 在 Task 5 启动前审查 Cargo.toml |
| R3 | GSOE GRPO 实现复杂度超预期 | 中 | 中 | P1 | 本周占位为基于规则的适应度;真实 GRPO 留待 Week 7+ |
| R4 | NMC 多模态融合输出 CLV 维度不匹配 | 中 | 高 | P0 | 严格对齐 nexus-core CLV 类型;NMC Specialist 与 Lead Architect 在 Task 4 启动前对齐 |
| R5 | Week 5 P1 文档同步工作量超预期 | 低 | 低 | P2 | Task 7 仅做关键同步,Could 项延后 |
| R6 | 5 crate 并行实现导致集成测试阶段集中爆发冲突 | 中 | 中 | P1 | Task 6 E2E 测试提前到 Day 4 启动,非周末才启动 |
| R7 | Week 5 复审未勾选项过多导致 Task 9 工作量爆炸 | 低 | 低 | P2 | Task 9 仅核验关键项,Minor 项结转 Week 7 |
| R8 | CHTC 5 适配器全部实现工作量过大 | 中 | 中 | P1 | 5 适配器实现 VSCode 完整 + 其余 4 个仅 trait + 骨架,真实集成留 Week 7+ |
| R9 | AHIRT 配置化(P2)改动 parliament crate 触发回归 | 低 | 中 | P1 | Task 8 必须重跑 parliament 全部测试 + Week 5 安全免疫套件 |
| R10 | 跨层事件契约破坏 Week 5 稳定性 | 低 | 高 | P0 | 新事件全部为新增,不修改 Week 5 事件;Lead Architect 把关 |

### 7.2 风险监控机制

- 每日站会同步 R1/R2/R4 三大 P0 风险状态
- R1 SSRA 延迟在 Day 1 完成预编译模板后立即跑基准,不达标当天调整
- R2 CHTC 架构在 Day 5 启动前 Lead Architect 必须审查 Cargo.toml 依赖图
- R6 集成测试 Task 6 提前到 Day 4 启动,每日增量集成

---

## 8. 时间计划(7 天 Day 36-42)

### 8.1 甘特图(简化版)

```
Day 36 [一] │ SSRA-T1.1骨架 │ LSCT-T2.1骨架 │ GSOE-T3.1骨架 │ NMC-T4.1骨架 │ CHTC-T5.1骨架 │
Day 37 [二] │ SSRA-T1.2模板 │ LSCT-T2.2分层 │ GSOE-T3.2GRPO │ NMC-T4.2文本 │ CHTC-T5.2协议 │
Day 38 [三] │ SSRA-T1.3融合 │ LSCT-T2.3升降 │ GSOE-T3.3变异 │ NMC-T4.3融合 │ CHTC-T5.3适配 │
Day 39 [四] │ SSRA-T1.4测试 │ LSCT-T2.4测试 │ GSOE-T3.4测试 │ NMC-T4.4测试 │ CHTC-T5.4测试 │ E2E-T6.1启动│
Day 40 [五] │ SSRA-T1.5基准 │ LSCT-T2.5基准 │ GSOE-T3.5基准 │ NMC-T4.5基准 │ CHTC-T5.5基准 │ E2E-T6.2链路│ 文档-T7启动│
Day 41 [六] │ P1修复-T7集中 │ P2修复-T8 AHIRT │ Week5复审收尾-T9 │ E2E-T6.3完善 │
Day 42 [日] │ Week6 验收-T10 │ 全量回归 │ 文档定稿 │
```

### 8.2 关键路径(CPM)

```
关键路径: Task 1/2/3/4/5(并行,Day 36-40) → Task 6 E2E(Day 39-41) → Task 10 验收(Day 42)
非关键路径: Task 7 文档同步(Day 40-41) / Task 8 AHIRT 配置化(Day 41) / Task 9 复审收尾(Day 41)
```

**关键路径任务**:Task 1-5 的 5 crate 并行实现 + Task 6 E2E + Task 10 验收
**瓶颈识别**:Task 6 E2E 依赖 5 crate 全部完成,任何一 crate 滑期都会阻塞 E2E;缓释策略为 Day 4 启动 mock E2E 框架搭建

---

## 9. 执行原则与变更控制

### 9.1 执行原则

1. **优先级驱动**:严格按 MoSCoW 顺序, Must 任务全部完成后再启动 Should
2. **长期主义**:所有实现必须符合 OMEGA 四定律,不为短期达标牺牲架构健康度
3. **TDD-first**:每个 crate 先写类型定义 + 基础测试,再写业务逻辑(项目规则 §3.1)
4. **依赖方向铁律**:L10→L9/L7/L5/L3/L2/L1 仅允许向下,跨层走 EventBus(项目规则 §2.2)
5. **资源监控**:每日 Lead Architect 评估各专家工作量,超过 8 小时/日触发任务重分配
6. **变更控制**:所有架构变更必须经过 CCB(Lead Architect + 受影响专家)审批,记录到 spec.md 附录

### 9.2 变更控制流程(CCB)

1. 提出方填写变更申请(变更内容 + 影响范围 + 理由)
2. Lead Architect 评估,若涉及跨 crate 接口则召开 CCB 会议(2-3 名相关专家)
3. CCB 决议写入 spec.md 附录 A(变更记录)
4. tasks.md 同步更新受影响 Task

### 9.3 卡点排查机制

- 任何专家卡点超过 2 小时,立即 SendMessage 通知 Lead Architect
- Lead Architect 评估是否需要跨专家会审
- 卡点解决后,记录到 project_memory.md 经验教训

---

## 10. 代码质量规范

### 10.1 Rust 编码规范(项目规则 §4 强化)

- workspace 级依赖:`{ dep = { workspace = true } }`,禁止独立声明版本
- async fn:`Send + 'static + 'async` 约束,避免 spawn 失败
- 错误处理:库层 `thiserror` enum,应用层 `anyhow::Result<T>`
- 禁用 `unwrap()`/`expect()`,锁中毒场景使用 `unwrap_or_else(|p| p.into_inner())`
- 优先 `impl Trait` 或 `enum dispatch`,避免 `Box<dyn Trait>`
- 单函数 ≤ 200 行,超过必须拆模块

### 10.2 模块组织标准(项目规则 §4.2)

```
my-crate/
├── Cargo.toml
├── src/
│   ├── lib.rs           # pub mod 导出 + 文档注释
│   ├── types.rs         # 核心类型定义
│   ├── config.rs        # 配置解析
│   ├── error.rs         # 错误类型(thiserror)
│   └── <功能子模块>.rs
└── tests/
    └── integration.rs   # 集成测试
```

### 10.3 注释规范

- **不主动写注释**,只在 WHY 不明显处加(项目规则 §4.1)
- WHY 注释覆盖:隐藏约束 / 变通方案 / 反直觉行为
- 不写 WHAT 注释(代码自解释)
- 不引用当前任务编号(如 "fix #123"),改写到 PR description

### 10.4 命名规范(项目规则 §4.3)

| 模式 | 示例 | 适用 |
|------|------|------|
| `*Coordinator` | `LsctCoordinator` | 协调多子组件 |
| `*Engine` | `SlimeFusionEngine` / `GsoeEvolutionEngine` | 独立生命周期 |
| `*Adapter` | `VscodeAdapter` | 适配器模式 |
| `*Template` | `SlimeTemplate` | 模板模式 |
| `*Promoter` / `*Demoter` | `LsctPromoter` | 升降温器 |
| `*Rollout` | `GrpoRollout` | RL 采样轨迹 |

### 10.5 工具强制执行

```powershell
# 周末验收必跑
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo build --workspace --release
cargo fmt --all -- --check
```

---

## 11. 资源授权与保障

### 11.1 工具与资源清单

| 工具类别 | 工具名称 | 使用范围 | 权限级别 |
|---------|---------|---------|---------|
| 文件操作 | Read/Write/Edit/Glob/Grep | 全 workspace 读写 | 读/写 |
| 编译验证 | cargo check/build/test/clippy | 全 workspace | 执行 |
| 子代理调度 | Agent(rust-architecture-expert) | 5 crate 并行实现 | 调度 |
| 子代理调度 | Agent(Explore) | 跨 crate 依赖调研 | 调度 |
| MCP 工具 | mcp_Sequential_Thinking | 复杂架构决策 | 读 |
| MCP 工具 | mcp_Memory | 经验教训沉淀 | 读/写 |
| Skill | superpowers-main | TDD 流程引导 | 读 |
| Skill | TRAE-code-review | 代码审查 | 读 |
| Skill | TRAE-security-review | 安全审查 | 读 |
| Skill | test-driven-development | TDD 强制 | 读 |

### 11.2 培训与支持

- **Lead Architect** 作为技术支持联系人,任何工具使用问题直接 SendMessage
- **Skill 使用指南**:参见 system-reminder 中各 skill 描述
- **MCP 工具使用**:调用前必须 Read tool schema 确认参数(项目规则 §工具使用偏好)

---

## 12. 验收流程

### 12.1 周末验收命令

```powershell
$env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
$env:TMP = 'D:\Chimera CLI\tmp'
$env:TEMP = 'D:\Chimera CLI\tmp'
$env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"

cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo build --workspace --release
cargo fmt --all -- --check
```

### 12.2 验收检查清单(详见 checklist.md)

- 5 crate 完整实现 + 测试通过
- 性能指标全部达标(SSRA < 20ms 等)
- 安全免疫率 100%
- 文档同步完成(Task 7)
- AHIRT 配置化完成(Task 8)
- E2E 测试覆盖主链路
- 架构合规性 100%

### 12.3 评审与存档

- 评审专家:Lead Architect + QA/Docs Specialist(2 名技术专家)
- 评审意见记录到 spec.md 附录 B(评审记录)
- 评审通过后,更新 project_memory.md Week 6 经验教训
- 评审不通过,问题写入 tasks.md 新 Task,进入下一轮迭代

---

## 13. 参考文献

- `AETHER_NEXUS_OMEGA_ULTIMATE.md` §3.1 十层架构 / §5.2 模块接口 / §7 Week 6 推进计划 / §10.1 核心类型
- `AETHER_NEXUS_GEN3_OMEGA.md` §2 创新点 5(MPP)/ §2 创新点 4(MCSL)
- `CODE_WIKI.md` 代码 Wiki
- `.trae/specs/week5-deep-review/spec.md` Week 5 复审基线
- `.trae/specs/week5-parliament-security-budget/spec.md` Week 5 实现规范
- `project_memory.md` Week 5 经验教训
- ADR-016 NMC / ADR-020 CHTC / ADR-022 SSRA / ADR-025 GSOE

---

## 附录 A:变更记录

(待填充,变更发生时按时间顺序记录)

## 附录 B:评审记录

(待填充,周末验收后记录)
