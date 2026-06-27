# Phase 1 Architecture Analysis and Planning Spec

## Why

NEXUS-OMEGA 是一个 10 层架构、34 个 Rust crate 的超大规模分布式 AI Coding Agent 系统。当前仓库处于 Stage 0(仅 Cargo.toml 骨架,无 `.rs` 实现),在进入编码阶段前必须完成两件事:

1. **架构深度分析** — 对系统组件划分、数据流转路径、节点间通信机制、潜在性能瓶颈进行结构化分析,覆盖技术选型、资源分配、风险控制三个维度,避免在实现阶段才发现架构缺陷(对应 Claude Code 尸检教训:神函数、孤儿调用、竞态、内存爆炸)。
2. **第一阶段(Week 1)开发任务计划** — 将 8 周推进计划中的 Week 1(L0-L1 基础设施)细化为可执行、可监控、可调整的任务清单,明确模块划分、优先级、技术实现要求、量化验收标准、时间节点与责任人(专家子代理角色)。

本 spec 是 Stage 0 → Stage 1 过渡的决策依据,所有后续实现任务必须与本 spec 一致。

## What Changes

- 产出 NEXUS-OMEGA 分布式系统架构深度分析报告(组件划分、数据流、通信机制、性能瓶颈)
- 产出技术选型、资源分配、风险控制三维度评估
- 产出第一阶段(Week 1)开发任务计划,含 7 个工作日的任务分解
- 明确每个任务的责任人映射(6 类专家子代理)
- 明确每个任务的量化验收标准(对应 §8.3 性能基准目标)
- 不修改任何现有代码或 Cargo.toml(本 spec 仅产出规划文档)

## Impact

- Affected specs:
  - `establish-elite-collaboration-team`(责任人角色定义来源)
  - `init-crates-workspace`(Workspace 骨架已就绪,本 spec 在其上规划实现)
  - 所有后续 Week 1-8 实现 spec
- Affected code:
  - 第一阶段将新增 `src/lib.rs` / `src/main.rs` 及模块文件到以下 crate:
    - L1: `event-bus`、`nexus-core`、`model-router`
    - L4: `seccore`、`decay-engine`、`qeep-protocol`
    - L10: `chimera-cli`
  - 本 spec 阶段不触碰任何代码,仅产出分析文档

## ADDED Requirements

### Requirement: 分布式系统架构深度分析

系统 SHALL 产出覆盖以下四个子维度的架构分析,每个维度需经多轮结构化思考验证:

#### Scenario: 系统组件划分分析
- **WHEN** 分析 10 层架构与 34 个 crate 的职责边界
- **THEN** 明确每个 crate 的单一职责、对外公开 API 边界、与同层/跨层 crate 的依赖关系
- **THEN** 标注关键协调器(`OmniSparseCoordinator`、`KVBlockSemanticRouter`、`QuestEngine`、`Parliament`)的内部子组件分解
- **THEN** 验证依赖方向符合 §2.2 铁律(L(N)→L(N-1) 允许,L(N)→L(N+1) 禁止,跨层走 Event Bus)

#### Scenario: 数据流转路径分析
- **WHEN** 分析从用户输入到结果输出的完整数据流
- **THEN** 描绘主数据流路径:NMC 编码 → Quest 分解 → TTG 切换 → Parliament 审议 → PVL 生产验证 → OSA 协调 → KVBSR 路由 → GEA 激活 → MTPE 预测 → GQEP 聚集 → QEEP 纠缠 → ISCM 更新 → Wiki 沉淀 → GSOE 进化 → Event Bus 广播
- **THEN** 标注每一步的输入/输出数据结构(参照 §10.1 核心数据结构)
- **THEN** 标注每一步的同步/异步特性与背压点

#### Scenario: 节点间通信机制分析
- **WHEN** 分析 crate 间、进程间、跨平台间的通信
- **THEN** 明确三层通信机制:
  - 进程内:`event-bus`(tokio::broadcast,MessagePack 序列化)
  - 跨进程:`mcp-mesh`(MCP 协议,stdio + HTTP)
  - 跨平台:`chtc-bridge`(5 IDE 双向集成)
- **THEN** 标注每类通信的延迟目标、可靠性保证、故障转移策略
- **THEN** 验证 QEEP 量子纠缠协议覆盖所有异步操作(零孤儿调用,对应尸检教训 5.4% 孤儿率)

#### Scenario: 潜在性能瓶颈评估
- **WHEN** 评估系统在满载场景下的性能瓶颈
- **THEN** 标注以下高风险瓶颈点及其缓解措施:
  - KVBSR 路由(300 工具池,目标 < 2ms)→ SIMD 加速 + 两级路由
  - OSA 全维稀疏(5 维度协调,目标 < 1ms)→ 并行计算 + 缓存掩码
  - HCW 分层窗口(1M Token)→ 分层加载 + OSA 稀疏化(对应尸检教训:1M Token 暴力加载)
  - MLC 四级记忆 → CMT 热/温/冷/冰分层 + LRU 驱逐
  - Event Bus 广播 → tokio::broadcast 背压 + 慢消费者隔离
  - SQLite 向量查询 → sqlite-vec + WAL 模式 + 索引优化
- **THEN** 产出瓶颈优先级矩阵(影响面 × 发生概率)

### Requirement: 三维度评估(技术选型 / 资源分配 / 风险控制)

系统 SHALL 对架构分析结果进行三维度交叉验证:

#### Scenario: 技术选型评估
- **WHEN** 评估 §5.1 核心技术栈选型
- **THEN** 验证每项选型的选型理由与来源映射(Rust 1.85+ / Tokio 1.40+ / Ratatui 0.29+ / Clap 4.5+ / sqlite-vec 0.1+ / gVisor / Wasmtime 22.0+)
- **THEN** 标注每项选型的替代方案与切换成本
- **THEN** 验证 workspace 共享依赖避免版本碎片化(对应 §4.1 workspace 级依赖铁律)

#### Scenario: 资源分配评估
- **WHEN** 评估 8 周推进计划的资源分配
- **THEN** 标注每周的人力(专家子代理)分配与并行度
- **THEN** 标注关键路径(Week 1 Event Bus → Week 2 Quest Engine → Week 3 OSA → Week 4 PVL → Week 5 Parliament)
- **THEN** 验证每周验收节点的资源释放与下一周资源接入

#### Scenario: 风险控制评估
- **WHEN** 评估架构与实施风险
- **THEN** 标注以下风险及其缓解措施:
  - 架构风险:跨层耦合 → 依赖方向铁律 + ADR 特批
  - 安全风险:命令注入/沙箱逃逸 → SecCore 零信任 + Decay 衰减 + AHIRT 红队
  - 性能风险:1M Token 暴力加载 → HCW 分层 + OSA 稀疏化
  - 质量风险:神函数 → 单函数 ≤200 行 + Clippy 强制
  - 进度风险:下层未稳定上层已开工 → 严格 L1→L10 顺序 + 周验收门禁
  - 兼容性风险:5 IDE 适配 → CHTC 统一协议
- **THEN** 产出风险矩阵(影响 × 概率 × 缓解成本)

### Requirement: 第一阶段(Week 1)开发任务计划

系统 SHALL 产出 Week 1(L0-L1 基础设施)的详细任务计划,严格对齐 §7 Week 1 推进计划:

#### Scenario: 任务模块划分与优先级
- **WHEN** 划分 Week 1 任务模块
- **THEN** 按以下优先级(P0)排序,严格按序推进:
  - **Task 1**(Day 1):Workspace 骨架补全 + CI/CD — 验证 34 crate 骨架可构建
  - **Task 2**(Day 2):Event Bus 实现 — 20+ 事件类型,tokio::broadcast
  - **Task 3**(Day 3):SecCore 零信任 — gVisor + seccomp 沙箱
  - **Task 4**(Day 4):DecayEngine 能力衰减 — 连续权限流体模型
  - **Task 5**(Day 5):QEEP 量子纠缠 — 零孤儿调用保证
  - **Task 6**(Day 6):CLI 入口 + Figment 配置 — Clap 子命令体系
  - **Task 7**(Day 7):Week 1 验收 — 全量测试 + 覆盖率 > 85%

#### Scenario: 技术实现要求
- **WHEN** 实现每个 Task
- **THEN** 遵循 §4 Rust 编码规范:workspace 级依赖、`Send + 'static + 'async` 约束、`anyhow::Result`/`thiserror`、避免 `unwrap`/`Box<dyn Trait>`
- **THEN** 遵循 §4.2 模块组织模式:每个 crate 含 `lib.rs`/`types.rs`/`config.rs`/`error.rs`/`tests/`
- **THEN** 遵循 §6 架构红线:单函数 ≤200 行、所有异步操作经 GQEP 聚集、所有外部调用经 SecCore 沙箱、所有 async 必须 await 或 spawn 管理
- **THEN** 每个 Task 先写类型定义与基础测试(TDD-first),再写业务逻辑

#### Scenario: 可量化验收标准
- **WHEN** 每个 Task 完成
- **THEN** 满足以下量化标准(对齐 §8.3 性能基准目标):
  - Task 1:`cargo build --workspace` 通过、`cargo clippy --workspace -- -D warnings` 无警告
  - Task 2:Event Bus 1000 事件/秒、20+ 事件类型、背压处理
  - Task 3:SecCore 拦截 6 种攻击(注入/越权/泄露/逃逸/篡改/滥用)、SHA-256 审计链
  - Task 4:DecayEngine 5 次冻结测试、连续 [0,1] 权限流体
  - Task 5:QEEP 10000 次操作零孤儿调用、EntangledCall 超时处理
  - Task 6:`aether --version` < 200ms、`config init` 生成 `~/.aether/omega.yaml`
  - Task 7:覆盖率 > 85%、`cargo test --workspace` 全通过、`cargo build --release` 通过

#### Scenario: 时间节点规划
- **WHEN** 排定 Week 1 时间线
- **THEN** 按 Day 1-7 严格排定,每日一个主任务 + 当日测试
- **THEN** Day 7 为验收门禁,未通过则不进入 Week 2
- **THEN** 每个 Task 内部按"骨架 → 类型 → 测试 → 实现 → 审查 → 验证"六步推进

#### Scenario: 责任人分配(专家子代理映射)
- **WHEN** 分配 Task 责任人
- **THEN** 按 `establish-elite-collaboration-team` spec 定义的 6 类专家子代理分配:
  - Task 1:DevOps 专家(主导)+ 架构专家(审查)
  - Task 2:架构专家(主导)+ 性能专家(审查延迟)
  - Task 3:安全专家(主导)+ 实现专家(协助沙箱集成)
  - Task 4:安全专家(主导)+ 实现专家(协助模型实现)
  - Task 5:架构专家(主导)+ 质量专家(审查零孤儿测试)
  - Task 6:实现专家(主导)+ DevOps 专家(审查 CLI 打包)
  - Task 7:质量专家(主导)+ 全员参与验收
- **THEN** 每个 Task 由主导专家负责实现,审查专家独立审查,审查未通过返回实现

### Requirement: 计划的可执行性、可监控性、可调整性

系统 SHALL 确保第一阶段计划具备三性:

#### Scenario: 可执行性
- **WHEN** 任务被分派给子代理
- **THEN** 每个 Task 有明确的输入(依赖项)、输出(交付物)、验收标准
- **THEN** 每个 Task 的代码目标与测试目标可独立验证
- **THEN** 无依赖的 Task 可并行(Week 1 内 Task 3/4/5 在 Task 2 完成后可部分并行)

#### Scenario: 可监控性
- **WHEN** 任务执行中
- **THEN** 通过 `tasks.md` 勾选状态追踪进度
- **THEN** 通过 `checklist.md` 勾选验收点
- **THEN** 每日提交信息遵循 §7 规范(如 `feat(event-bus): typed broadcast bus`)

#### Scenario: 可调整性
- **WHEN** 任务延期或验收未通过
- **THEN** 在 `tasks.md` 追加修复 Task,不删除原 Task
- **THEN** 调整后续 Task 的依赖关系,但不动摇 Week 1 验收门禁
- **THEN** 重大调整需更新本 spec 的 ADDED Requirements

## MODIFIED Requirements

无(本 spec 为新增,不修改现有 spec)

## REMOVED Requirements

无

## 附录:架构分析关键发现(预填,实现阶段验证)

### A.1 组件划分关键发现

- **L1 基础设施层**是全局瓶颈源:`event-bus`(tokio::broadcast)是唯一跨层通信通道,故障将导致全系统瘫痪 → 必须在 Week 1 Day 2 优先稳定
- **L4 安全层**与 **L6 执行层**存在强耦合风险:SecCore 沙箱拦截所有外部调用,若 OSA 协调器未考虑沙箱延迟,可能导致路由超时 → 需在 Week 3 OSA 实现时回归测试
- **L8 议会层**依赖 L6 + L5 + L3 三层,是依赖最广的层 → Week 5 实现前必须确认 L3/L5/L6 已稳定

### A.2 数据流关键发现

- 主数据流有 14 步,每一步都是潜在故障点 → QEEP 必须覆盖全部 14 步
- `NexusState` 是全局状态聚合点,需保证线程安全(`Arc<RwLock<>>` 或 actor 模型)
- `CLV`(512-dim f32)是跨层通用语言,必须在 L1 定义并在所有上层复用

### A.3 通信机制关键发现

- 进程内通信:`event-bus` 单一通道,需区分事件类型(20+ 类型)避免消息风暴
- 跨进程通信:`mcp-mesh` 需支持超位置 + 纠缠,Week 7 实现
- 跨平台通信:`chtc-bridge` 需统一 5 IDE 协议,Week 6 实现

### A.4 性能瓶颈关键发现

| 瓶颈点 | 影响面 | 发生概率 | 缓解措施 | 实现周次 |
|--------|--------|---------|---------|---------|
| KVBSR 路由延迟 | 高 | 高 | SIMD + 两级路由 | Week 3 |
| OSA 全维稀疏计算 | 高 | 高 | 并行 + 缓存掩码 | Week 3 |
| HCW 1M Token 加载 | 高 | 中 | 分层加载 + OSA 稀疏化 | Week 3 |
| Event Bus 背压 | 中 | 中 | 慢消费者隔离 | Week 1 |
| SQLite 向量查询 | 中 | 中 | sqlite-vec + WAL | Week 2 |
| 议会 5 角色并行决策 | 中 | 中 | 并行 + 共识算法 | Week 5 |

### A.5 风险矩阵摘要

| 风险 | 影响 | 概率 | 缓解成本 | 优先级 |
|------|------|------|---------|--------|
| 跨层耦合破坏依赖方向 | 高 | 中 | 低(规则强制) | P0 |
| 1M Token 暴力加载 | 高 | 中 | 高(HCW+OSA) | P1 |
| 命令注入(SecCore 绕过) | 高 | 低 | 高(gVisor+seccomp) | P0 |
| 神函数(>200 行) | 中 | 高 | 低(单函数限制) | P0 |
| 下层未稳定上层开工 | 高 | 中 | 低(周验收门禁) | P0 |
| 5 IDE 兼容性 | 中 | 中 | 中(CHTC 统一协议) | P2 |
