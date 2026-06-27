# Establish Elite Collaboration Team Spec

## Why
NEXUS-OMEGA 是一个 10 层架构、34 个 Rust crate 的超大规模 AI Coding Agent 系统，涵盖事件驱动、零信任安全、全维稀疏协调、多模型路由、预算治理、在线进化等前沿领域。单一代理无法在所有维度保持深度与准确性。需组建一支跨领域精英专家团队，以 8 周推进计划为骨架、以任务优先级为核心，通过分布式深度分析与严谨验证，确保从零开发的高质量交付。

## What Changes
- 建立 6 个专家级子代理角色，每个角色明确映射到 10 层架构中的具体职责
- 建立 P0/P1/P2 三级任务优先级体系，与 8 周推进计划对齐
- 建立"需求澄清 → 方案讨论 → 实现 → 代码审查 → 验证"五步分布式协作工作流
- 建立代码质量标准、长期主义约束与资源控制原则
- 授权团队调用所有系统允许的工具资源（sub-agents、mcp、skills、RunCommand 等）

## Impact
- Affected specs: 所有后续 spec 与实现任务（Week 1-8 全部任务）
- Affected code: 全部 34 个 crate 的代码、测试、配置、文档

## ADDED Requirements

### Requirement: 专家角色定义与架构映射
系统 SHALL 在每次复杂任务中按需调用以下 6 类专家子代理，每个角色有明确的架构层级映射：

#### Scenario: 架构专家（Architecture Expert）
- **WHEN** 任务涉及 L1 基础设施、模块划分、crate 间依赖关系、接口设计
- **THEN** 由架构专家主导，产出清晰、可演进、最小化的设计方案
- **THEN** 确保设计符合 10 层架构分层原则，避免跨层耦合

#### Scenario: 实现专家（Implementation Expert）
- **WHEN** 任务进入编码阶段，涉及 Rust 代码编写
- **THEN** 由实现专家编写清晰、可读、注释完善、符合 Rust API Guidelines 的代码
- **THEN** 确保使用 workspace 共享依赖，避免版本碎片化

#### Scenario: 安全专家（Security Expert）
- **WHEN** 任务涉及 L4 安全层（SecCore、ASA、AHIRT、Decay、QEEP）或任何安全边界
- **THEN** 由安全专家进行威胁建模与安全审查
- **THEN** 确保零信任原则、能力衰减模型、反黑客红队覆盖

#### Scenario: 质量与验证专家（Quality & Verification Expert）
- **WHEN** 任务进入验证阶段或每周验收节点
- **THEN** 由质量与验证专家执行测试、类型检查、lint、审查 checklist
- **THEN** 确保覆盖率 > 85%，性能基准达标

#### Scenario: 性能与效率专家（Performance & Efficiency Expert）
- **WHEN** 任务涉及 L6 执行层（OSA、KVBSR、GEA、GQEP）、L3 预算层或延迟敏感逻辑
- **THEN** 由性能与效率专家评估时间/空间复杂度与资源消耗
- **THEN** 确保路由延迟 < 2ms、稀疏度 < 40%、压缩率 > 4×

#### Scenario: 工具与 DevOps 专家（DevOps Expert）
- **WHEN** 任务涉及构建、CI/CD、依赖管理、Docker、跨平台发布、监控
- **THEN** 由 DevOps 专家负责工具链与自动化验证
- **THEN** 确保 `cargo build` 通过、5 平台 binary 生成、Docker 镜像构建

### Requirement: 任务优先级体系（与 8 周计划对齐）
系统 SHALL 按以下优先级顺序推进任务，严格禁止跨优先级跳跃：

#### Scenario: P0 - 基础骨架（Week 1）
- **WHEN** P0 任务未完成
- **THEN** 优先完成：Workspace 骨架、Event Bus、SecCore 零信任、DecayEngine、QEEP、CLI 入口
- **THEN** 验收标准：`cargo build` 通过、覆盖率 > 85%、1000 事件/秒

#### Scenario: P1 - 关键路径功能（Week 2-5）
- **WHEN** P0 已完成
- **THEN** 推进：Quest Engine、Repo Wiki、Model Router、MLC 记忆、HCW 窗口、OSA 稀疏、PVL 执行、Parliament 议会、预算治理
- **THEN** 验收标准：端到端 Quest 通过、路由准确率 > 90%、决策准确率 > 90%

#### Scenario: P2 - 增强与生产化（Week 6-8）
- **WHEN** P1 已完成
- **THEN** 推进：SSRA 适配、GSOE 进化、NMC 多模态、CHTC 跨平台、MCP 网格、监控、渗透测试、发布
- **THEN** 验收标准：5 平台 binary 生成、OWASP Top 10 通过、1000 次无泄漏

### Requirement: 分布式深度分析与验证工作流
系统 SHALL 对每个复杂任务执行五步结构化流程：

#### Scenario: Step 1 - 需求澄清
- **WHEN** 任务存在歧义或多种理解
- **THEN** 使用 AskUserQuestion 澄清，绝不基于假设推进

#### Scenario: Step 2 - 方案讨论
- **WHEN** 存在多种实现路径或架构选择
- **THEN** 调度多名专家子代理并行评估，综合优缺点后决策
- **THEN** 优先选择最小可行方案，仅在明确要求时增加复杂度

#### Scenario: Step 3 - 实现
- **WHEN** 方案确定
- **THEN** 由实现专家子代理编写代码，遵循代码质量标准

#### Scenario: Step 4 - 代码审查
- **WHEN** 代码实现完成
- **THEN** 由独立专家子代理进行审查，确保逻辑、可读性、注释、规范
- **THEN** 若审查未通过，返回 Step 3 修正

#### Scenario: Step 5 - 验证
- **WHEN** 代码审查通过
- **THEN** 由质量与验证专家执行测试、lint、类型检查
- **THEN** 更新 checklist.md，勾选通过的检查点

### Requirement: 长期主义与资源控制
系统 SHALL 避免短期行为和过度工程：

#### Scenario: 最小可行实现
- **WHEN** 功能可实现为简单方案
- **THEN** 优先选择简单方案，拒绝为假设性未来需求过度设计
- **THEN** 三行相似代码优于一个过早抽象

#### Scenario: 资源控制
- **WHEN** 使用 sub-agents 或外部工具
- **THEN** 避免重复计算、冗余请求和无谓的并行开销
- **THEN** 无依赖的任务才并行，有依赖的严格按序执行

#### Scenario: 不做未要求的事
- **WHEN** 任务只要求修复一个 bug
- **THEN** 不顺手重构周边代码、不添加未要求的注释或类型标注
- **THEN** 不创建非必要的文件

## MODIFIED Requirements
### Requirement: 代码质量标准
所有新增或修改的代码 SHALL 满足：
- **单一职责**：每个函数/模块只做一件事，模块边界清晰
- **命名准确**：变量、函数、类型命名准确反映意图
- **注释解释意图**：注释解释"为什么"而非"是什么"，不重复代码
- **错误处理显式**：使用 `Result`/`Option`，避免 `unwrap`/`expect` 在非测试代码中
- **可测试性**：代码结构支持单元测试，依赖注入优先
- **Rust 最佳实践**：符合 Rust API Guidelines、Clippy 无警告、`cargo fmt` 格式化
- **workspace 一致性**：使用 `workspace.dependencies` 共享依赖，避免版本碎片

## REMOVED Requirements
无
