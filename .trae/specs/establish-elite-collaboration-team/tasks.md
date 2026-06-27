# Tasks

- [x] Task 1: 定义并文档化 6 类专家角色与 10 层架构的映射关系
  - 架构专家 → L1 基础设施、模块划分、crate 依赖
  - 实现专家 → 全层 Rust 代码编写
  - 安全专家 → L4 安全层（SecCore、ASA、AHIRT、Decay、QEEP）
  - 质量与验证专家 → 每周验收节点、测试、lint
  - 性能与效率专家 → L6 执行层、L3 预算层、延迟敏感逻辑
  - DevOps 专家 → 构建、CI/CD、Docker、跨平台发布

- [x] Task 2: 建立 P0/P1/P2 三级任务优先级体系，与 8 周计划对齐
  - P0（Week 1）：Workspace、Event Bus、SecCore、Decay、QEEP、CLI 入口
  - P1（Week 2-5）：Quest、Wiki、Router、MLC、HCW、OSA、PVL、Parliament、预算
  - P2（Week 6-8）：SSRA、GSOE、NMC、CHTC、MCP 网格、监控、发布
  - 明确每个优先级的验收标准

- [x] Task 3: 设计五步分布式协作工作流
  - Step 1 需求澄清：使用 AskUserQuestion，杜绝假设
  - Step 2 方案讨论：多专家并行评估，选最小可行方案
  - Step 3 实现：实现专家编写代码
  - Step 4 代码审查：独立专家审查，未通过则返回 Step 3
  - Step 5 验证：质量专家执行测试，更新 checklist

- [x] Task 4: 制定代码质量标准与长期主义执行规范
  - 7 项代码质量标准（单一职责、命名、注释、错误处理、可测试性、Rust 最佳实践、workspace 一致性）
  - 长期主义原则：最小可行实现、资源控制、不做未要求的事

- [x] Task 5: 授权与工具使用规范
  - 可调用工具范围：sub-agents、mcp、skills、RunCommand、SearchCodebase 等
  - 工具选择原则：使用最专用、最经济的工具
  - 并行原则：无依赖才并行，有依赖严格按序

# Task Dependencies
- Task 1 与 Task 2 可并行
- Task 3 依赖 Task 1、Task 2
- Task 4 与 Task 5 可与 Task 3 并行
