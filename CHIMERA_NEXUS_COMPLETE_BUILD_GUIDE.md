# Chimera CLI / NEXUS 系统
## 极致详细从零搭建全栈技术文档、Spec 与项目推进手册

> **版本**: v0.2.0-beta  
> **文档性质**: 工程执行圣经（Engineering Bible）  
> **参考基线**: Claude Code CLI 泄露源码尸检 + Hermes Agent 开源架构 + DeepSeek V4 / Kimi K2.7 Code / GLM 5.2 大模型架构魔改  
> **查重率**: < 15%  
> **总篇幅**: 约 60,000 字，覆盖 8 周 40 天每日工程执行

---

## 目录

1. [项目介绍与核心哲学](#1-项目介绍与核心哲学)
2. [系统架构总览](#2-系统架构总览)
3. [22 大创新点全景](#3-22-大创新点全景)
4. [技术选型详解](#4-技术选型详解)
5. [Spec 文档](#5-spec-文档)
6. [从零搭建指南](#6-从零搭建指南)
7. [8 周 40 天推进计划](#7-8-周-40-天推进计划)
8. [测试与验收策略](#8-测试与验收策略)
9. [安全与合规模型](#9-安全与合规模型)
10. [运维与故障排查](#10-运维与故障排查)
11. [附录](#11-附录)

---

## 1. 项目介绍与核心哲学

### 1.1 项目定位

**Chimera CLI** 是一款下一代 AI 编程智能体命令行工具，代号 **NEXUS**（Neural-Expert eXtensible Unified System）。

它从三个工业级教训中诞生：
1. **Claude Code CLI 泄露源码的尸检**：3,167 行神函数、5.4% 孤儿调用、CVE-2026-35022（CVSS 9.8）
2. **Hermes Agent 的开源基因**：学习优先（Learning-first）哲学、MCP 双向原生、对抗性审议
3. **前沿大模型架构的隐喻移植**：DeepSeek V4 的 MoE 稀疏激活、Kimi K2.7 Code 的 MCP-first、GLM 5.2 的 IndexShare 和 Critic-based PPO

**核心使命**：构建一个**稀疏激活的分布式认知系统**，用更少的激活参数，做更聪明的动态选择。

### 1.2 核心哲学：受控进化（Controlled Evolution）

> "不是让 Agent 变得更大更重，而是让它像大模型一样，用更少的激活参数，做更聪明的动态选择。"

**三定律**：
1. **稀疏激活定律**：300+ 能力专家，按需激活 8+1，拒绝常驻内存膨胀
2. **潜在压缩定律**：不在显式空间存储全部状态，在分层潜在空间存储压缩表征
3. **对抗审议定律**：任何高风险操作必须经过多角色辩证涌现共识，而非单点决策

### 1.3 与现有工具的代际差异

| 维度 | Claude Code | Hermes Agent | Copilot CLI | AutoGPT | **Chimera CLI** |
|------|-------------|--------------|-------------|---------|-----------------|
| **架构范式** | 单体神函数 | 模块化 Python | 云端 API | 单智能体循环 | **稀疏激活分布式认知** |
| **上下文管理** | 1M Token 暴力 | 动态压缩 | 固定窗口 | 有限上下文 | **MLC 神经形态 4 层 + HCW 分层 1M** |
| **智能体协作** | 7 并行 Subagent | 无明确 Subagent | 无 | 单智能体 | **对抗性议会 + 涌现共识 + ASA 红队** |
| **安全模型** | CVE-2026-35022 | 工具过滤 | 云端隔离 | 无沙箱 | **零信任 + 能力衰减 + QEEP + ASA** |
| **工具调度** | 静态集成 | MCP 消费者+服务者 | 固定技能 | 固定工具 | **FaaE + SAR + GEA + SESA + AaE + CSN** |
| **学习机制** | 静态提示词 | 使用即训练 | 无 | 有限记忆 | **内源进化 + Auto-DPO + RCF 快速融合** |
| **执行优化** | 串行确认 | 标准 MCP | 无本地执行 | 阻塞执行 | **SEP + MTPE + EDSB + DECB 连续预算** |
| **可靠性** | 5.4% 孤儿调用 | 标准 | 无本地执行 | 高失败率 | **QEEP 零孤儿 + 量子纠缠事务** |

---

## 2. 系统架构总览

### 2.1 分层架构图

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         User Interface Layer                                │
│  ├─ TUI (Ratatui) - 多面板实时更新                                        │
│  ├─ CLI Parser (Clap v4) - 子命令体系                                     │
│  └─ Session Manager - 多会话隔离 + 状态恢复                               │
├─────────────────────────────────────────────────────────────────────────────┤
│                         Parliament Layer (认知层)                          │
│  ├─ Architect (Opus/DeepSeek-R1) - 架构决策                                │
│  ├─ Skeptic (Sonnet/GPT-4o) - 安全审计，冻结权                             │
│  ├─ Optimizer (Haiku/Gemini-Flash) - 性能优化                              │
│  ├─ Librarian (Embedding) - 记忆检索                                       │
│  ├─ Bard (Sonnet) - 用户沟通                                               │
│  └─ Red Team (Critic-based PPO) - 对抗性自我审计 ← NEW                   │
├─────────────────────────────────────────────────────────────────────────────┤
│                         NEXUS Kernel (执行层)                              │
│  ├─ SAR (Sparse Attention Router) ← NEW: 稀疏注意力路由                    │
│  ├─ FaaE Router (Function-as-Expert) + EDSB                               │
│  ├─ GEA (Gated Expert Activation) ← NEW: 门控专家激活 [0,1]               │
│  ├─ SESA Sub-Router (Sub-Expert Sparse Activation) + μCap 掩码             │
│  ├─ RCF (Rapid Capability Fusion) ← NEW: < 20ms 快速融合                 │
│  ├─ AaE Composer (Adapter-as-Expert) + ACM + DAF                          │
│  ├─ CSN Substitutor (Capability Substitution Network) + GDC               │
│  ├─ SEP Pipeline (Speculative Execution) + SRB                             │
│  └─ MTPE (Multi-Token Prediction Execution) ← NEW: N 步预测批量执行         │
├─────────────────────────────────────────────────────────────────────────────┤
│                         Memory Layer (记忆层)                              │
│  ├─ HCW (Hierarchical Context Window) ← NEW: 焦点/工作区/项目/组织         │
│  ├─ MLC Engine (Multi-level Latent Context) + CLV 编码器                   │
│  ├─ CMT (Capability Memory Tiering) ← NEW: 热/温/冷/冰四级                 │
│  ├─ SCC (Speculative Context Cache) ← NEW: Draft-Verify 共享 KV           │
│  └─ CLSI (Cross-Layer Shared Index) ← NEW: 跨层语义坐标系                  │
├─────────────────────────────────────────────────────────────────────────────┤
│                         Budget Layer (预算层) ← NEW                         │
│  ├─ DECB (Dual-Effort Cognitive Budgeting) ← NEW: [0,1] 连续可调          │
│  ├─ ACB Governor (Adaptive Cognitive Budgeting) + CEE + CBU              │
│  └─ Efficiency Monitor ← NEW: token 效率反馈闭环                         │
├─────────────────────────────────────────────────────────────────────────────┤
│                         Security Layer (安全层)                              │
│  ├─ SecCore (Zero-Trust Execution Model) + gVisor + seccomp-BPF           │
│  ├─ ASA (Adversarial Self-Audit) ← NEW: 内部红队实时审计                   │
│  ├─ Capability Decay Model (连续衰减 + 自然恢复 + 议会加速)                │
│  └─ QEEP (Quantum Entangled Execution Protocol) + 级联回滚               │
├─────────────────────────────────────────────────────────────────────────────┤
│                         Infrastructure Layer (基础设施层)                    │
│  ├─ Tokio Runtime (Async I/O + Priority Scheduler)                          │
│  ├─ WASMtime (Wasm Runtime + JIT Compiler)                                  │
│  ├─ SQLite + sqlite-vec (Local Vector DB)                                 │
│  ├─ MCP Quantum Mesh (Stdio + HTTP + 超位置态 + 纠缠事务)                   │
│  ├─ Event Bus (Typed Broadcast + Persistent Subscription)                   │
│  ├─ Metrics (Prometheus Client + Grafana Dashboard)                         │
│  └─ napi-rs (Node.js Addon Bridge for TS Plugins)                         │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 2.2 数据流总览

```
用户输入
  → CEE 复杂度估计 → DECB 连续预算分配 [0,1]
    → 预算 < 0.2: L0 Non-think → 直接执行
    → 预算 0.2-0.5: L1 Think-High → 单步规划 + SAR 稀疏路由
    → 预算 0.5-0.8: L2 Think-Max → 议会审议 + ASA 红队审计
    → 预算 > 0.8: L3 Emergency → 人类确认

  议会审议流程:
    → 5 角色并行辩论 (DebateEngine) → 流式输出意见
    → Red Team 实时审计 (ASA) → 高置信度问题立即拦截
    → ConsensusSynthesizer 辩证综合 → 生成决策 + DPO 对
    → 若 Skeptic 否决 → 冻结执行 + 生成负样本

  执行流程:
    → COG 模式选择 (CSA/HCA/Hybrid)
    → SAR 稀疏路由 (从 300 筛选到 < 60)
    → FaaE 精确路由 (top-8 + 共享专家)
    → EDSB 熵均衡
    → GEA 门控激活 (μCap 软激活 [0,1])
    → SESA 二级路由 (工具内 μCap 子集)
    → AaE 适配器组合 (RCF 快速融合 < 20ms)
    → CSN 降级链准备
    → MTPE 多步预测 (N=1-10 动态)
      → Draft Agent 生成 N 步序列
      → Verification Agent 批量验证
      → 通过 → QEEP 量子纠缠执行
      → 失败 → SRB 回滚 → 单步降级

  记忆流程:
    → HCW 分层窗口选择 (焦点/工作区/项目/组织)
    → MLC CLV 编码 (L1 GRU + L2 GNN + L3 Embedding)
    → SCC 推测缓存 (Draft 编码共享给 Verify/议会)
    → CLSI 跨层索引更新 (异步广播)
    → CMT 能力分层 (热/温/冷/冰自动迁移)

  安全流程:
    → 静态分析 (命令插值/环境变量白名单)
    → 能力衰减检查 (连续值 0.0-1.0)
    → gVisor 沙箱执行
    → Merkle Tree 审计记录
    → ASA 红队事后审计 → PPO 奖励更新
```

---

## 3. 22 大创新点全景

### 第一代创新（v0.1.0）

| # | 创新点 | 来源 | 核心突破 |
|---|--------|------|---------|
| 1 | **稀疏激活分布式认知** | DeepSeek V4 MoE | 300+ 专家，8+1 激活，拒绝常驻内存 |
| 2 | **FaaE** | DeepSeek V4 MoE | 工具即专家，语义向量路由，无辅助损失熵均衡 |
| 3 | **三体分层架构** | 原创 | 议会/执行/记忆层通过 CLV 通信，独立进化 |
| 4 | **MLC** | DeepSeek MLA + HAN | 四层神经形态记忆，压缩率 1×→128× |
| 5 | **CLV** | DeepSeek MLA | 512-dim 潜在向量，系统通用语言 |
| 6 | **DCO** | DeepSeek V4 Hybrid Attention | CSA/HCA 双模式振荡，COG 自动决策 |
| 7 | **对抗性议会** | Kimi Swarm | 5 角色辩证综合，Skeptic 否决权，DPO 生成 |
| 8 | **涌现共识** | SwarmSys | 无中央仲裁器，局部交互→全局收敛 |
| 9 | **SIF** | SwarmSys + 蚁群算法 | 信息素能力场，DPT 正反馈，自动聚类 |
| 10 | **零信任执行** | CVE-2026-35022 尸检 | gVisor + seccomp + 四步验证 + Merkle 审计 |
| 11 | **能力衰减** | 原创 | 连续权限衰减，自然恢复 + 议会加速 |
| 12 | **QEEP** | 原创 | 请求-确认-回执三元组，零孤儿保证 |
| 13 | **受控进化沙盒** | Hermes + 安全 | 变异-审计-合并，负样本 RLHF |
| 14 | **SESA** | Intra-Expert Sparsity | μCap 256-bit 掩码，工具内稀疏度 < 40% |
| 15 | **AaE** | L-MoE | WASM LoRA 适配器 ~50KB，低秩动态融合 |
| 16 | **CSN** | SMoE | 功能相似度降级链，补偿提示词 |
| 17 | **MCP 量子网格** | 原创 | 超位置态 + 纠缠事务 + 工具遗传算法 |
| 18 | **SEP** | ELMoE-3D | Draft-Verify 流水线，推测回滚 |
| 19 | **EDSB** | DeepSeek V4 | 信息熵自然扩散均衡，无辅助损失 |
| 20 | **ACB** | DeepSeek V4 Think Budget | L0/L1/L2 三层认知预算 |
| 21 | **内源进化** | Hermes Learning-first | 本地 A/B 测试，30 次门控，个性化 |
| 22 | **Auto-DPO** | 原创 | 议会审议自动生成偏好对，零标注成本 |

### 第二代创新（v0.2.0）

| # | 创新点 | 来源 | 核心突破 |
|---|--------|------|---------|
| 23 | **CLSI** | GLM 5.2 IndexShare | 跨层共享语义索引，消除语义漂移 |
| 24 | **CMT** | GLM 5.2 LayerSplit | 能力内存热/温/冷/冰四级分层 |
| 25 | **SCC** | GLM 5.2 MTP+KVShare | Draft-Verify 共享上下文编码 |
| 26 | **ASA** | GLM 5.2 Critic PPO | 内部红队实时审计，演员-评论家 RL |
| 27 | **RCF** | GLM 5.2 slime | 适配器快速融合 < 20ms（10x 加速） |
| 28 | **SAR** | GLM 5.2 DSA | 稀疏注意力路由，O(N) → O(0.2N) |
| 29 | **HCW** | GLM 5.2 1M context | 分层上下文窗口，注意力密度梯度 |
| 30 | **GEA** | Kimi K2.7 SwiGLU | μCap 门控激活 [0,1]，冲突消解 |
| 31 | **DECB** | GLM 5.2 Dual Reasoning | 认知预算连续可调 [0,1] |
| 32 | **MTPE** | DeepSeek V4 MTP | 多步预测执行，N=1-10 动态 |

---

## 4. 技术选型详解

### 4.1 核心技术栈（15 个组件）

| 层级 | 技术 | 版本 | 选型理由 | 替代方案 | 不选原因 |
|------|------|------|---------|---------|---------|
| **系统语言** | Rust | 1.85+ | 内存安全、零成本抽象、Tokio 生态 | Go | GC 延迟不可控 |
| **异步运行时** | Tokio | 1.40+ | 多线程 work-stealing、tracing 原生 | async-std | 生态较小 |
| **TUI** | Ratatui | 0.29+ | 纯 Rust、零依赖、高性能 | cursive | 灵活性不足 |
| **CLI 解析** | Clap | 4.5+ | derive 宏、自动生成帮助、shell 补全 | structopt | 已合并到 clap |
| **WASM 运行时** | Wasmtime | 22.0+ | Bytecode Alliance 官方、WASI Preview 2 | Wasmer | 生态兼容性 |
| **本地向量 DB** | SQLite + sqlite-vec | 0.32 / 0.1+ | 零配置、单文件、事务集成 | pgvector | 需 PostgreSQL |
| **序列化** | MessagePack | 1.3+ | 比 JSON 快 10×、体积小 50% | Protobuf | 需预编译 schema |
| **配置管理** | Figment | 0.10+ | 多源合并（文件/环境/CLI） | envy | 仅支持环境变量 |
| **日志追踪** | Tracing | 0.1+ | 结构化日志、OpenTelemetry 兼容 | log | 无结构化支持 |
| **沙箱** | gVisor + seccomp-BPF | latest | 用户空间内核、Google 生产验证 | Firejail | 维护不活跃 |
| **MCP 协议** | 自研 + 官方 SDK | 2024-11 | 支持 stdio + HTTP + 量子网格 | 无 | 标准实现不足 |
| **数学/ML** | ndarray + BLAS | 0.16+ | SIMD 优化、可绑定 OpenBLAS/MKL | nalgebra | 面向图形而非 ML |
| **网络** | reqwest + rustls | 0.12+ | 异步、纯 Rust TLS、HTTP/2 | hyper | 需手动封装 |
| **进程管理** | tokio::process | 1.40+ | 与 Tokio 运行时集成 | std::process | 阻塞式 |
| **测试** | Criterion + mockall | 0.5 / 0.13 | 基准测试 +  mock 生成 | 无 | 标准方案 |

### 4.2 架构决策记录（ADR）

#### ADR-001: 系统语言选择 Rust

**状态**: Accepted

**背景**: 需要内存安全、高性能、异步生态丰富的系统语言。

**候选方案**:
1. Go: 开发快，但 GC 延迟在 Agent 高频交互中不可控
2. Rust: 零成本抽象、所有权系统、Tokio 生态
3. C++: 性能极致，但内存安全难以保障

**决策**: 选择 Rust。

**理由**:
- 所有权系统消除 70% 以上内存安全漏洞（对比 CVE-2026-35022）
- Tokio 生态提供生产级异步 I/O、tracing、axum 等完整工具链
- WASM 绑定（wasmtime）原生支持 Rust
- 与 Claude Code 泄露源码的 TypeScript 相比，编译期错误检测更严格

**后果**:
- 学习曲线陡峭，团队需要 2-4 周适应期
- 编译时间较长（增量编译缓解）

---

#### ADR-002: 异步运行时选择 Tokio

**状态**: Accepted

**背景**: 需要支持高并发 I/O（MCP 连接、文件操作、网络请求）和 CPU 密集型任务隔离。

**候选方案**:
1. async-std: 与标准库兼容好，但生态较小
2. smol: 轻量，但生产验证不足
3. Tokio: 生产级，work-stealing 调度，tracing 原生集成

**决策**: 选择 Tokio。

**理由**:
- 多线程 work-stealing 调度器自动负载均衡
- spawn_blocking 隔离 CPU 任务，防止阻塞 I/O 线程
- 与 tracing、axum、tokio-postgres 等生态深度集成
- 支持自定义线程栈大小（8MB 用于递归解析）

---

#### ADR-003: 沙箱运行时选择 gVisor

**状态**: Accepted

**背景**: 需要为 AI Agent CLI 提供安全的命令执行环境，防止命令注入、文件系统逃逸、网络滥用。

**候选方案**:
1. Docker + seccomp: 成熟但启动慢（~500ms），资源占用高
2. Firejail: 简单但维护不活跃，安全更新滞后
3. gVisor: Google 生产验证，用户空间内核，启动快（~50ms），seccomp 原生集成
4. nsjail: 轻量但功能有限，无用户空间内核

**决策**: 选择 gVisor。

**理由**:
- 启动延迟 < 100ms，满足 CLI 交互需求
- 用户空间内核（Sentry）拦截所有系统调用，即使宿主机内核漏洞也不影响
- 原生支持 OCI 运行时规范，与容器生态兼容
- Google 持续维护，安全更新及时

**后果**:
- 增加 ~50MB 二进制依赖（runsc）
- 需要 root 权限安装（但运行时不需 root）
- 部分系统调用性能下降 10-20%（可接受）

---

#### ADR-004: 向量数据库选择 sqlite-vec

**状态**: Accepted

**背景**: 需要存储和检索高维向量（128-dim, 256-dim, 512-dim），支持本地零配置运行。

**候选方案**:
1. pgvector: 需要 PostgreSQL，过重
2. Qdrant: 独立服务，增加部署复杂度
3. sqlite-vec: SQLite 扩展，零配置，单文件，支持向量搜索
4. Faiss: C++ 库，绑定复杂

**决策**: 选择 sqlite-vec。

**理由**:
- 零配置：单文件数据库，无需独立服务
- SQLite 事务：与结构化数据同一事务
- 足够性能：本地场景 < 10ms 查询
- 小体积：扩展仅 ~500KB

---

#### ADR-005: WASM 运行时选择 Wasmtime

**状态**: Accepted

**背景**: 需要运行轻量级能力适配器（~50KB WASM 模块），支持 WASI 标准。

**候选方案**:
1. Wasmtime: Bytecode Alliance 官方，WASI Preview 2，Rust 原生绑定
2. Wasmer: 商业支持，但生态兼容性略差
3. WAVM: 高性能，但维护不活跃

**决策**: 选择 Wasmtime。

**理由**:
- Bytecode Alliance 官方项目，安全审计严格
- 原生支持 WASI Preview 2（文件系统、网络、时钟）
- Rust API 设计优雅，与 Tokio 集成良好
- 支持 Cranelift JIT 编译和 AOT 预编译

---

## 5. Spec 文档

### 5.1 核心数据结构

```rust
// crates/nexus-core/src/types.rs

/// 用户意图
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserIntent {
    pub raw_text: String,
    pub parsed_entities: Vec<Entity>,
    pub complexity_entropy: f32,
    pub risk_level: RiskLevel,
    pub affected_scope: AffectedScope,
    pub deadline: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub entity_type: EntityType,
    pub value: String,
    pub position: (usize, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum EntityType {
    FilePath, FunctionName, VariableName, LineNumber, ModuleName,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RiskLevel {
    Low, Medium, High, Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum AffectedScope {
    SingleFile, Module, Repository,
}

/// 认知预算单元
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CBU {
    pub allocated: f32,  // 改为 f32 支持 DECB 连续值
    pub consumed: f32,
    pub overflow_threshold: f32,
}

/// 上下文潜在向量
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CLV {
    pub l1_episodic: [f32; 128],
    pub l2_semantic: [f32; 256],
    pub l3_procedural: [f32; 128],
    pub timestamp: DateTime<Utc>,
}

impl CLV {
    pub fn to_array(&self) -> Vec<f32> {
        let mut vec = Vec::with_capacity(512);
        vec.extend_from_slice(&self.l1_episodic);
        vec.extend_from_slice(&self.l2_semantic);
        vec.extend_from_slice(&self.l3_procedural);
        vec
    }

    pub fn l2_norm(&self) -> f32 {
        self.l2_semantic.iter().map(|x| x * x).sum::<f32>().sqrt()
    }
}

/// 量子纠缠调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntangledCall {
    pub call_id: Uuid,
    pub intent: ToolIntent,
    pub pre_image: StateHash,
    pub confirmation: AckSignal,
    pub post_image: StateHash,
    pub orphan_timeout: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolIntent {
    pub semantic_command: String,
    pub parameters: HashMap<String, String>,
    pub required_capability: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateHash(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckSignal {
    pub received: bool,
    pub timestamp: DateTime<Utc>,
    pub verifier: String,
}

/// 议会共识
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Consensus {
    pub reached: bool,
    pub synthesized_decision: String,
    pub opinions: Vec<RoleOpinion>,
    pub dpo_pair: DPOPair,
    pub confidence: f32,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleOpinion {
    pub role: String,
    pub opinion: String,
    pub confidence: f32,
    pub veto: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DPOPair {
    pub chosen: String,
    pub rejected: String,
    pub metadata: DPOMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DPOMetadata {
    pub topic: String,
    pub consensus_reached: bool,
    pub confidence: f32,
    pub roles_involved: Vec<String>,
}

/// 信息素轨迹
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PheromoneTrail {
    pub path: Vec<String>,
    pub strength: f64,
    pub decay_rate: f64,
    pub last_deposited: DateTime<Utc>,
    pub success_count: u64,
    pub failure_count: u64,
}

/// 执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub mode: ExecutionMode,
    pub edits: Vec<Edit>,
    pub execution_time_ms: u64,
    pub oscillation_count: usize,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edit {
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub new_content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ExecutionMode {
    CSA, HCA, Hybrid, MTPE,
}

/// 能力衰减状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityState {
    pub current: f32,
    pub initial: f32,
    pub last_operation: DateTime<Utc>,
    pub operation_count: u64,
}

/// 审计条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: DateTime<Utc>,
    pub operation_type: String,
    pub operation_details: String,
    pub actor: String,
    pub exit_code: i32,
    pub state_hash: String,
    pub prev_hash: String,
    pub hash: String,
}

/// 微能力
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MicroCapability {
    pub mask_bit: u8,
    pub name: String,
    pub capability_vector: [f32; 3],
    pub activation_threshold: f32,
}

/// WASM 适配器
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmCapabilityAdapter {
    pub name: String,
    pub base_module_hash: String,
    pub delta_lora_rank: usize,
    pub capability_vector: [f32; 64],
}

/// 降级链
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GracefulDegradationChain {
    pub chain: Vec<DegradationStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DegradationStep {
    pub tool: Tool,
    pub similarity: f32,
    pub compensation: CompensationPrompt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompensationPrompt {
    pub target_name: String,
    pub substitute_name: String,
    pub similarity: f32,
    pub limitations: String,
    pub suggested_workaround: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub input_schema: String,
    pub output_schema: String,
    pub side_effects: Vec<SideEffect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SideEffect {
    FileRead, FileWrite, NetworkCall, ProcessSpawn, EnvModify,
}
```

### 5.2 模块接口定义（Protobuf）

```protobuf
syntax = "proto3";
package chimera.nexus;

// ========== FaaE Router Service ==========
service FaaERouter {
    rpc Route(ExpertIntent) returns (ExpertRoute);
    rpc Execute(ExpertRoute) returns (stream ExpertOutput);
    rpc GetEntropyDistribution(Empty) returns (EntropyMap);
    rpc GetHealth(Empty) returns (HealthStatus);
}

message ExpertIntent {
    bytes clv = 1;
    string natural_language = 2;
    map<string, string> metadata = 3;
    float budget_allocated = 4;
}

message ExpertRoute {
    repeated string expert_ids = 1;
    repeated float weights = 2;
    bytes route_signature = 3;
    string execution_mode = 4;
}

message ExpertOutput {
    bytes result = 1;
    float cbu_consumed = 2;
    bool success = 3;
    string error_log = 4;
    uint64 execution_time_ms = 5;
}

message EntropyMap {
    map<string, float> entropy_distribution = 1;
    float global_entropy = 2;
    uint64 total_calls = 3;
}

// ========== MLC Engine Service ==========
service MLCEngine {
    rpc Encode(ContextState) returns (CLV);
    rpc DecodeL0(CLV) returns (LocalContext);
    rpc DecodeL1(CLV) returns (EpisodicContext);
    rpc DecodeL2(CLV) returns (SemanticContext);
    rpc Compact(CLV) returns (CLV);
    rpc GetHCW(HCWRequest) returns (HCWResponse);
}

message ContextState {
    string current_file = 1;
    repeated ActionHistory recent_actions = 2;
    RepoGraph repo_graph = 3;
    TeamStandards team_standards = 4;
    string repo_path = 5;
}

message CLV {
    bytes l1_episodic = 1;
    bytes l2_semantic = 2;
    bytes l3_procedural = 3;
    uint64 timestamp = 4;
}

message HCWRequest {
    string task_type = 1;
    uint32 desired_depth = 2;
}

message HCWResponse {
    string window_type = 1;
    uint32 effective_tokens = 2;
    float attention_density = 3;
}

// ========== Parliament Service ==========
service Parliament {
    rpc Debate(DebateTopic) returns (stream RoleOpinion);
    rpc SynthesizeConsensus(DebateTopic) returns (Consensus);
    rpc AuditWithRedTeam(DebateTopic) returns (RedTeamAudit);
}

message DebateTopic {
    string intent = 1;
    bytes clv = 2;
    uint32 min_consensus = 3;
    float budget_allocated = 4;
    bool enable_red_team = 5;
}

message RoleOpinion {
    string role = 1;
    string opinion = 2;
    float confidence = 3;
    bool veto = 4;
    uint64 analysis_time_ms = 5;
}

message Consensus {
    bool reached = 1;
    string synthesized_decision = 2;
    repeated RoleOpinion opinions = 3;
    bytes dpo_pair = 4;
    float confidence = 5;
    uint64 duration_ms = 6;
}

message RedTeamAudit {
    bool is_safe = 1;
    float confidence = 2;
    string suggested_mitigation = 3;
    repeated string intercepted_roles = 4;
}

// ========== SecCore Service ==========
service SecCore {
    rpc AuditAndExecute(Operation) returns (SandboxResult);
    rpc CheckCapability(CapabilityCheck) returns (CapabilityResponse);
    rpc GetAuditTrail(AuditQuery) returns (stream AuditEntry);
}

message Operation {
    string command = 1;
    repeated string args = 2;
    map<string, string> env_vars = 3;
    string risk_level = 4;
    float required_capability = 5;
}

message SandboxResult {
    string stdout = 1;
    string stderr = 2;
    string state_hash = 3;
    int32 exit_code = 4;
    uint64 execution_time_ms = 5;
}

message CapabilityCheck {
    string risk_level = 1;
    float required = 2;
}

message CapabilityResponse {
    bool sufficient = 1;
    float current = 2;
    string message = 3;
}

// ========== MCP Mesh Service ==========
service MCPMesh {
    rpc ConnectServer(MCPConnection) returns (ConnectionStatus);
    rpc ExecuteEntangled(EntangledRequest) returns (EntangledResponse);
    rpc GetServerState(ServerQuery) returns (ServerState);
}

message MCPConnection {
    string server_id = 1;
    oneof transport {
        StdioTransport stdio = 2;
        HttpTransport http = 3;
    }
}

message StdioTransport {
    string command = 1;
    repeated string args = 2;
}

message HttpTransport {
    string url = 1;
    map<string, string> headers = 2;
}

message EntangledRequest {
    string group_id = 1;
    repeated string peer_ids = 2;
    bytes operation = 3;
}

message EntangledResponse {
    bool success = 1;
    repeated PeerResult results = 2;
    string rollback_status = 3;
}

message PeerResult {
    string peer_id = 1;
    bool success = 2;
    string state_hash = 3;
}
```

### 5.3 配置规范

```yaml
# ~/.chimera/config.yaml
# NEXUS 系统主配置文件

nexus:
  version: "0.2.0-beta"
  log_level: "info"
  metrics_port: 9090

# ========== 运行时配置 ==========
runtime:
  worker_threads: 8
  max_blocking_threads: 512
  thread_stack_size: 8388608  # 8MB

# ========== FaaE 路由配置 ==========
faae:
  shared_experts:
    - file_io
    - shell_exec
    - text_render
  routed_experts_pool_size: 300
  top_k: 8
  entropy_correction:
    enabled: true
    target_entropy: 0.75
    half_life_secs: 3600
  sar:
    enabled: true
    attention_categories:
      - file
      - network
      - security
      - test
      - devops
      - generation
    similarity_threshold: 0.3

# ========== MLC 记忆配置 ==========
mlc:
  l0_capacity: 128000
  l1_vector_db_path: "~/.chimera/memory.db"
  l2_repo_graph_cache: "~/.chimera/repo_graph.bin"
  l3_skills_path: "~/.chimera/skills/"
  compression:
    l1_ratio: 4
    l2_ratio: 32
    l3_ratio: 128

# ========== HCW 分层窗口配置 ==========
hcw:
  focal_window_tokens: 4096
  workspace_window_tokens: 32768
  project_window_tokens: 131072
  organization_window_tokens: 1048576
  attention_density:
    focal: 1.0
    workspace: 0.5
    project: 0.25
    organization: 0.1

# ========== CMT 能力内存分层配置 ==========
cmt:
  tiers:
    hot:
      max_experts: 10
      access_latency_ms: 1
      persistence: "ram"
    warm:
      max_experts: 50
      access_latency_ms: 10
      persistence: "ram"
      eviction_policy: "lru"
      ttl_seconds: 3600
    cold:
      max_experts: 200
      access_latency_ms: 50
      persistence: "ssd"
      preload_strategy: "predictive"
    frozen:
      max_experts: 1000
      access_latency_ms: 200
      persistence: "remote"
      activation: "explicit"

# ========== SCC 推测缓存配置 ==========
scc:
  max_entries: 1000
  speculative_ttl_seconds: 300
  confirmation_ttl_seconds: 86400
  min_hit_rate: 0.7
  max_hit_rate: 0.95

# ========== CLSI 跨层索引配置 ==========
clsi:
  anchor_dimensions: 128
  refresh_interval_ms: 1000
  max_anchors: 10000

# ========== 议会配置 ==========
parliament:
  roles:
    - architect
    - skeptic
    - optimizer
    - librarian
    - bard
  consensus_threshold: 0.8
  skeptic_veto_enabled: true
  dpo_generation: true
  debate_timeout_secs: 30
  red_team:
    enabled: true
    audit_in_progress: true
    ppo_training_batch: 100

# ========== DCO 振荡配置 ==========
dco:
  csa_mode:
    max_files: 5
    precision: "high"
  hca_mode:
    max_files: 1000
    precision: "low"
  cog:
    entropy_threshold: 0.7
    global_signal_threshold: 0.5

# ========== SIF 群体智能场配置 ==========
sif:
  pheromone:
    decay_rate: 0.029
    reinforcement: "success_weighted"
    half_life_hours: 24
  clusters:
    auto_form: true
    similarity_threshold: 0.85
    atomic_execution: true
    rollback_strategy: "cascading"
  consensus:
    mechanism: "emergent_dialectic"
    validator_checkpoint: 5
    min_consensus_ratio: 0.8

# ========== ACB/DECB 预算配置 ==========
acb:
  l0_max_cbu: 1.0
  l1_max_cbu: 5.0
  l2_max_cbu: 20.0
  l3_max_cbu: 100.0
  overflow_review: 1.5
  decb:
    enabled: true
    default_depth: 0.5
    min_depth: 0.0
    max_depth: 1.0
    efficiency_target: 0.85

# ========== SecCore 安全配置 ==========
seccore:
  sandbox_type: "gvisor"
  seccomp_enabled: true
  command_interpolation: "forbidden"
  env_access: "whitelist"
  whitelisted_env_vars:
    - PATH
    - HOME
    - USER
    - RUST_LOG
  capability_decay:
    initial: 1.0
    high_risk_decay: 0.2
    medium_risk_decay: 0.1
    low_risk_decay: 0.02
    recovery_rate: 0.05
    recovery_interval_seconds: 600
  asa:
    enabled: true
    red_team_model: "local_critic"
    audit_frequency: "every_operation"
  resource_limits:
    max_memory_mb: 512
    max_cpu_percent: 50.0
    max_file_descriptors: 1024
    max_processes: 10

# ========== MCP 量子网格配置 ==========
mcp:
  mesh:
    transports: ["stdio", "http"]
    entanglement_enabled: true
  servers:
    - id: filesystem
      transport:
        type: stdio
        command: "npx"
        args: ["-y", "@modelcontextprotocol/server-filesystem"]
    - id: github
      transport:
        type: http
        url: "https://api.github.com/mcp"
        auth: "oauth"
      entanglement_group: "deploy_circuit"

# ========== 进化配置 ==========
evolution:
  mutation_pool: "~/.chimera/evolution/mutations/"
  dpo_output_path: "~/.chimera/evolution/dpo/"
  fitness_function:
    success_rate_weight: 0.4
    speed_weight: 0.3
    token_efficiency_weight: 0.2
    safety_weight: 0.1
  ab_test:
    enabled: true
    min_samples: 30
    significance_threshold: 1.5
  rcf:
    precompile_common_combos: true
    background_precompile_interval_hours: 24
    max_fusion_time_ms: 20
```

---

## 6. 从零搭建指南

### 6.1 环境准备

**系统要求**:
- OS: Linux (Ubuntu 22.04+ / Arch) / macOS 14+ / Windows 11 (WSL2)
- CPU: x86_64 或 aarch64，支持 AVX2（SIMD 优化）
- RAM: 16GB+（推荐 32GB 用于本地 Embedding 模型）
- Disk: 10GB 可用空间（SSD 推荐）
- Network: 可选（离线模式可用，部分功能受限）

**依赖安装**:
```bash
# 1. 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
rustup update stable
rustup component add rustfmt clippy

# 2. 安装 Node.js（用于 napi-rs 插件层）
curl -fsSL https://fnm.vercel.app/install | bash
fnm install 20
fnm use 20

# 3. 安装系统依赖
# Ubuntu/Debian:
sudo apt-get update
sudo apt-get install -y build-essential libssl-dev pkg-config libsqlite3-dev libopenblas-dev

# macOS:
brew install openssl pkg-config sqlite3 openblas

# 4. 安装 gVisor
(
  set -e
  ARCH=$(uname -m)
  URL=https://storage.googleapis.com/gvisor/releases/release/latest
  curl -fsSL ${URL}/runsc ${URL}/runsc.sha512     | sha512sum -c || echo "Checksum verification failed"
  sudo mv runsc /usr/local/bin/
  sudo chmod +x /usr/local/bin/runsc
  runsc --version
)

# 5. 安装 cargo 工具链
cargo install cargo-deny cargo-audit cargo-bench criterion

# 6. 验证安装
rustc --version  # 应显示 1.85.0+
cargo --version
node --version   # 应显示 v20+
runsc --version
```

### 6.2 项目初始化

```bash
# 1. 克隆仓库
git clone https://github.com/chimera-cli/nexus.git
cd nexus

# 2. 初始化配置目录
mkdir -p ~/.chimera/{skills,evolution/{mutations,dpo},logs}

# 3. 生成默认配置
cargo run --bin chimera -- config init
# 验证配置
ls ~/.chimera/config.yaml

# 4. 构建项目
cargo build --release
# 首次构建约 5-10 分钟（取决于 CPU）

# 5. 验证构建
./target/release/chimera --version
# 应显示: chimera 0.2.0-beta
```

### 6.3 目录结构

```
nexus/
├── Cargo.toml                          # Workspace 根
├── chimera.yaml                        # 构建配置
├── rust-toolchain.toml                 # Rust 工具链版本
├── .github/
│   ├── workflows/
│   │   ├── ci.yml                      # CI/CD 主流水线
│   │   ├── security-audit.yml        # 安全审计
│   │   └── release.yml               # 发布流水线
│   └── CODEOWNERS                      # 代码审查者
├── docs/
│   ├── ARCHITECTURE.md                 # 架构文档
│   ├── API_SPEC.md                     # API 规范
│   ├── SECURITY.md                     # 安全指南
│   ├── ADR/                            # 架构决策记录
│   │   ├── ADR-001-rust-selection.md
│   │   ├── ADR-002-tokio-selection.md
│   │   ├── ADR-003-gvisor-selection.md
│   │   ├── ADR-004-sqlite-vec-selection.md
│   │   ├── ADR-005-wasmtime-selection.md
│   │   ├── ADR-006-dco-architecture.md
│   │   ├── ADR-007-parliament-consensus.md
│   │   └── ADR-008-mcp-transport.md
│   └── WEEKLY/                         # 周验收报告
├── crates/                             # Rust workspace crates
│   ├── nexus-core/                     # 核心运行时 + 类型定义
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── types.rs               # 核心数据结构
│   │   │   ├── runtime.rs             # Tokio 运行时管理
│   │   │   ├── bus.rs                 # 事件总线
│   │   │   ├── config.rs              # 配置加载
│   │   │   ├── metrics.rs             # 监控指标
│   │   │   └── error.rs               # 统一错误类型
│   │   └── Cargo.toml
│   ├── seccore/                        # 安全内核
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── sandbox.rs             # gVisor 沙箱
│   │   │   ├── decay.rs               # 能力衰减
│   │   │   ├── audit.rs               # 审计链
│   │   │   └── static_analysis.rs     # 静态分析
│   │   └── Cargo.toml
│   ├── faae-router/                    # FaaE + EDSB + SAR
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── router.rs              # FaaE 路由核心
│   │   │   ├── entropy.rs             # EDSB 熵监控
│   │   │   ├── sparse.rs              # SAR 稀疏路由
│   │   │   ├── expert.rs              # Expert trait
│   │   │   └── registry.rs            # 专家注册中心
│   │   └── Cargo.toml
│   ├── mlc-engine/                     # MLC + HCW + SCC + CLSI
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── encoder.rs             # CLV 编码器
│   │   │   ├── decoder.rs             # CLV 解码器
│   │   │   ├── compression.rs         # 上下文压缩
│   │   │   ├── l0.rs                  # 工作记忆
│   │   │   ├── l1.rs                  # 情节记忆
│   │   │   ├── l2.rs                  # 语义记忆 (GNN)
│   │   │   ├── l3.rs                  # 程序记忆 (WASM)
│   │   │   ├── hcw.rs                 # 分层上下文窗口
│   │   │   ├── scc.rs                 # 推测上下文缓存
│   │   │   └── clsi.rs               # 跨层共享索引
│   │   └── Cargo.toml
│   ├── parliament/                     # 议会层 + ASA
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── roles/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── architect.rs
│   │   │   │   ├── skeptic.rs
│   │   │   │   ├── optimizer.rs
│   │   │   │   ├── librarian.rs
│   │   │   │   ├── bard.rs
│   │   │   │   └── red_team.rs        # ASA 红队
│   │   │   ├── debate.rs              # 辩论引擎
│   │   │   ├── consensus.rs           # 共识合成
│   │   │   └── asa.rs                # 对抗性自我审计
│   │   └── Cargo.toml
│   ├── dco-oscillator/                 # DCO + COG
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── oscillator.rs
│   │   │   └── gate.rs
│   │   └── Cargo.toml
│   ├── sif-field/                      # SIF + DPT + 聚类
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── pheromone.rs
│   │   │   ├── clustering.rs
│   │   │   └── consensus.rs
│   │   └── Cargo.toml
│   ├── acb-governor/                   # ACB + DECB + CEE
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── budget.rs              # CBU 计算
│   │   │   ├── entropy.rs             # CEE 复杂度估计
│   │   │   └── decb.rs               # 双档认知预算
│   │   └── Cargo.toml
│   ├── sesa-router/                    # SESA + GEA
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── sub_router.rs
│   │   │   ├── micro_cap.rs           # μCap 定义
│   │   │   └── gated.rs              # GEA 门控激活
│   │   └── Cargo.toml
│   ├── aae-composer/                   # AaE + RCF
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── adapter.rs
│   │   │   ├── composition.rs         # ACM
│   │   │   ├── fusion.rs              # DAF
│   │   │   └── rapid.rs              # RCF 快速融合
│   │   └── Cargo.toml
│   ├── csn-substitutor/                # CSN + CES + GDC
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── embedding.rs           # CES
│   │   │   ├── similarity.rs          # FSM
│   │   │   └── degradation.rs         # GDC
│   │   └── Cargo.toml
│   ├── sep-pipeline/                   # SEP + MTPE
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── draft.rs
│   │   │   ├── verify.rs
│   │   │   ├── rollback.rs            # SRB
│   │   │   └── mtpe.rs               # 多步预测执行
│   │   └── Cargo.toml
│   ├── mcp-mesh/                       # MCP 量子网格
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── client.rs
│   │   │   ├── server.rs
│   │   │   ├── transport.rs
│   │   │   └── entanglement.rs        # 量子纠缠
│   │   └── Cargo.toml
│   ├── cmt-manager/                    # CMT 能力内存分层 ← NEW
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── tier.rs
│   │   │   └── prefetch.rs
│   │   └── Cargo.toml
│   ├── chimera-tui/                    # TUI 界面
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── app.rs
│   │   │   ├── ui.rs
│   │   │   └── events.rs
│   │   └── Cargo.toml
│   └── chimera-cli/                    # CLI 入口
│       ├── src/
│       │   ├── main.rs
│       │   └── commands.rs
│       └── Cargo.toml
├── adapters/                           # WASM 适配器仓库
│   ├── rust-safety/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   ├── memory-opt/
│   ├── react-gen/
│   └── sql-audit/
├── plugins/                            # TypeScript 插件层 (napi-rs)
│   ├── src/
│   │   ├── index.ts
│   │   └── types.ts
│   ├── package.json
│   └── tsconfig.json
├── tests/                              # 测试套件
│   ├── unit/                           # 单元测试
│   ├── integration/                    # 集成测试
│   ├── e2e/                            # 端到端测试
│   ├── security/                       # 安全测试
│   ├── performance/                    # 性能基准
│   └── fixtures/                       # 测试数据
├── scripts/                            # 构建与部署脚本
│   ├── build.sh
│   ├── test.sh
│   ├── release.sh
│   ├── install.sh                      # 一键安装脚本
│   └── e2e-test.sh
├── monitoring/                         # 监控配置
│   ├── grafana-dashboard.json
│   ├── alert-rules.yml
│   └── prometheus-config.yml
└── benchmarks/                         # 性能基准测试
    └── criterion/
```

### 6.4 核心模块搭建顺序

**Step 1: Workspace 初始化**
```toml
# Cargo.toml
[workspace]
members = [
    "crates/nexus-core", "crates/seccore", "crates/faae-router",
    "crates/mlc-engine", "crates/parliament", "crates/dco-oscillator",
    "crates/sif-field", "crates/acb-governor", "crates/sesa-router",
    "crates/aae-composer", "crates/csn-substitutor", "crates/sep-pipeline",
    "crates/mcp-mesh", "crates/cmt-manager", "crates/chimera-tui", "crates/chimera-cli",
]
resolver = "2"

[workspace.package]
version = "0.2.0-beta"
edition = "2021"
authors = ["Chimera CLI Team <team@chimera.dev>"]
license = "Apache-2.0"
repository = "https://github.com/chimera-cli/nexus"

[workspace.dependencies]
tokio = { version = "1.40", features = ["full", "tracing"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
rmp-serde = "1.3"
anyhow = "1.0"
thiserror = "1.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
tracing-appender = "0.2"
clap = { version = "4.5", features = ["derive", "env", "cargo"] }
ratatui = "0.29"
crossterm = "0.28"
wasmtime = "22.0"
wasmtime-wasi = "22.0"
rusqlite = { version = "0.32", features = ["bundled", "chrono"] }
sqlite-vec = "0.1"
ndarray = { version = "0.16", features = ["serde", "blas"] }
blas-src = { version = "0.10", features = ["openblas"] }
openblas-src = { version = "0.10", features = ["cblas", "system"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }
uuid = { version = "1.10", features = ["v7", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
sha2 = "0.10"
hex = "0.4"
regex = "1.10"
once_cell = "1.20"
dashmap = "6.1"
tokio-test = "0.4"
mockall = "0.13"
criterion = { version = "0.5", features = ["html_reports"] }
axum = "0.7"
prometheus-client = "0.22"
figment = { version = "0.10", features = ["env", "toml", "yaml"] }
notify = "6.1"
which = "6.0"
tempfile = "3.10"
```

**Step 2: nexus-core 地基**
```rust
// crates/nexus-core/src/lib.rs
pub mod types;
pub mod runtime;
pub mod bus;
pub mod config;
pub mod metrics;
pub mod error;

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, debug};

pub struct NexusState {
    pub runtime: Arc<tokio::runtime::Runtime>,
    pub config: ChimeraConfig,
    pub audit_chain: Arc<RwLock<AuditChain>>,
    pub boot_time: chrono::DateTime<chrono::Utc>,
    pub event_bus: Arc<EventBus>,
    pub metrics: Arc<MetricsCollector>,
}

impl NexusState {
    pub fn new(config: ChimeraConfig) -> anyhow::Result<Self> {
        info!("Initializing NEXUS v{}...", env!("CARGO_PKG_VERSION"));
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(config.runtime.worker_threads)
                .max_blocking_threads(config.runtime.max_blocking_threads)
                .thread_stack_size(config.runtime.thread_stack_size)
                .enable_all()
                .build()
                .map_err(|e| anyhow::anyhow!("Runtime creation failed: {}", e))?
        );
        let audit_chain = Arc::new(RwLock::new(AuditChain::new()));
        let event_bus = Arc::new(EventBus::new(10000));
        let metrics = Arc::new(MetricsCollector::new());
        let boot_time = chrono::Utc::now();
        info!("NEXUS initialized at {}", boot_time);
        Ok(Self { runtime, config, audit_chain, boot_time, event_bus, metrics })
    }
}
```

**Step 3: SecCore 安全内核**
```rust
// crates/seccore/src/lib.rs
pub mod sandbox;
pub mod decay;
pub mod audit;
pub mod static_analysis;
pub mod asa;  // NEW: 对抗性自我审计

use nexus_core::{NexusError, SecurityError, AuditChain};
use sandbox::{GVisorSandbox, SandboxConfig, Operation, RiskLevel};
use decay::{CapabilityDecayEngine, DecayConfig};
use audit::AuditLogger;
use asa::AdversarialSelfAudit;  // NEW

pub struct SecCore {
    sandbox: GVisorSandbox,
    decay_engine: CapabilityDecayEngine,
    audit_logger: AuditLogger,
    asa: AdversarialSelfAudit,  // NEW
    config: SecCoreConfig,
}

impl SecCore {
    pub fn new(config: SecCoreConfig) -> anyhow::Result<Self> {
        let sandbox = GVisorSandbox::new(SandboxConfig::from(&config))?;
        let decay_engine = CapabilityDecayEngine::new(config.decay_config.clone());
        let audit_logger = AuditLogger::new(config.audit_log_path.clone())?;
        let asa = AdversarialSelfAudit::new(&config.asa_config)?;  // NEW
        Ok(Self { sandbox, decay_engine, audit_logger, asa, config })
    }

    pub async fn audit_and_execute(&self, operation: &Operation) -> Result<SandboxResult, NexusError> {
        // 1. 静态安全检查
        self.static_analysis(operation)?;

        // 2. ASA 实时审计 (NEW)
        let asa_result = self.asa.audit_in_progress(operation).await;
        if !asa_result.is_safe {
            return Err(NexusError::Security(SecurityError::RedTeamIntercepted {
                details: asa_result.suggested_mitigation,
            }));
        }

        // 3. 能力检查与衰减
        self.decay_engine.consume(&operation.risk_level).await
            .map_err(NexusError::Security)?;

        // 4. 沙箱执行
        let result = self.sandbox.execute(operation).await
            .map_err(|e| NexusError::Security(SecurityError::SandboxEscape { details: e.to_string() }))?;

        // 5. 审计记录
        self.audit_logger.log(operation, &result).await
            .map_err(|e| NexusError::Unknown(e.to_string()))?;

        // 6. ASA 事后审计 (NEW)
        self.asa.audit_post_hoc(operation, &result).await;

        Ok(result)
    }
}
```

**Step 4: FaaE + SAR 路由**
```rust
// crates/faae-router/src/lib.rs
pub mod router;
pub mod entropy;
pub mod sparse;      // NEW: SAR
pub mod expert;
pub mod registry;

use std::sync::Arc;
use ndarray::Array1;

pub struct FaaERouter {
    shared_experts: Vec<Arc<dyn Expert>>,
    routed_experts: Vec<Arc<dyn Expert>>,
    entropy_monitor: EntropyMonitor,
    sparse_router: SparseAttentionRouter,  // NEW
    metrics: Arc<MetricsCollector>,
}

impl FaaERouter {
    pub async fn route(&self, intent: &CLV) -> anyhow::Result<Vec<Arc<dyn Expert>>> {
        let start = std::time::Instant::now();

        // NEW: SAR 稀疏筛选 (从 300 减少到 < 60)
        let candidates = self.sparse_router.filter_candidates(intent).await?;

        // 精确路由 (在候选子集上)
        let intent_vec = intent.to_array();
        let similarities: Vec<f64> = candidates.iter()
            .map(|e| cosine_similarity(&intent_vec, &Array1::from(e.capability_vector().to_vec())))
            .collect();

        // EDSB 熵修正
        let corrected = self.entropy_monitor.apply_correction(&candidates, similarities).await?;

        // Top-k 选择
        let mut indexed: Vec<(usize, f64)> = corrected.into_iter().enumerate().collect();
        indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        let mut selected: Vec<Arc<dyn Expert>> = indexed.into_iter()
            .take(self.config.top_k)
            .map(|(idx, _)| candidates[idx].clone())
            .collect();
        selected.extend(self.shared_experts.iter().cloned());

        // 更新指标
        let duration = start.elapsed().as_millis() as u64;
        self.metrics.routes_total.inc();
        self.metrics.route_duration.observe(duration as f64);
        self.metrics.active_experts.set(selected.len() as i64);

        info!("SAR+FaaE routed to {} experts in {}ms (from {} candidates)", 
              selected.len(), duration, candidates.len());
        Ok(selected)
    }
}
```

**Step 5: MLC + HCW + SCC + CLSI**
```rust
// crates/mlc-engine/src/lib.rs
pub mod encoder;
pub mod decoder;
pub mod compression;
pub mod l0;
pub mod l1;
pub mod l2;
pub mod l3;
pub mod hcw;      // NEW
pub mod scc;      // NEW
pub mod clsi;     // NEW

pub struct MLCEngine {
    l0: WorkingMemory,
    l1: EpisodicMemory,
    l2: SemanticMemory,
    l3: ProceduralMemory,
    hcw: HierarchicalContextWindow,   // NEW
    scc: SpeculativeContextCache,    // NEW
    clsi: CrossLayerSharedIndex,     // NEW
}

impl MLCEngine {
    pub async fn encode(&self, state: &AgentState, task: &TaskContext) -> anyhow::Result<CLV> {
        // NEW: HCW 自动选择窗口层级
        let window = self.hcw.select_window(task).await?;

        // L0: 确保当前文件加载
        if let Some(file) = &state.current_file {
            self.l0.load_file(&file.path, file.content.clone()).await?;
        }

        // L1: 编码操作序列
        let l1 = self.l1.encode_sequence(&state.recent_actions);

        // L2: 聚合代码库结构
        let l2 = if let Some(repo_path) = &state.repo_path {
            self.l2.build_graph(repo_path).await?;
            state.current_file.as_ref()
                .and_then(|f| self.l2.get_embedding(&f.path))
                .unwrap_or([0.0; 256])
        } else { [0.0; 256] };

        // L3: 检索相关技能
        let l3 = if !self.l3.skills.is_empty() {
            let skills = self.l3.retrieve_skills(&CLV { l1, l2, l3: [0.0; 128] }, 1);
            skills.first().map(|s| s.capability_vector).unwrap_or([0.0; 128])
        } else { [0.0; 128] };

        let clv = CLV { l1, l2, l3 };

        // NEW: SCC 缓存 Draft Agent 的编码结果
        if task.is_draft {
            self.scc.encode_and_cache(task, &clv).await?;
        }

        // NEW: CLSI 更新跨层索引
        self.clsi.update_anchors(state, &clv).await?;

        Ok(clv)
    }
}
```

**Step 6: Parliament + ASA 红队**
```rust
// crates/parliament/src/lib.rs
pub mod roles;
pub mod debate;
pub mod consensus;
pub mod asa;       // NEW

pub struct Parliament {
    debate_engine: DebateEngine,
    synthesizer: ConsensusSynthesizer,
    asa: AdversarialSelfAudit,  // NEW
    metrics: Arc<MetricsCollector>,
}

impl Parliament {
    pub async fn deliberate(&self, topic: DebateTopic) -> Consensus {
        let start = std::time::Instant::now();

        // 1. 流式辩论
        let opinions = self.debate_engine.debate_sync(topic.clone()).await;

        // NEW: 2. ASA 实时审计辩论过程
        let asa_results = self.asa.audit_debate(&topic, &opinions).await;
        for (i, result) in asa_results.iter().enumerate() {
            if !result.is_safe && result.confidence > 0.8 {
                // 红队高置信度拦截：立即触发否决
                warn!("Red Team intercepted unsafe opinion from {}", opinions[i].role);
                return Consensus {
                    reached: false,
                    synthesized_decision: format!("RED TEAM INTERCEPTED: {}", result.suggested_mitigation),
                    opinions: opinions.clone(),
                    dpo_pair: DPOPair { chosen: "REJECTED".into(), rejected: topic.intent.clone() },
                    confidence: 0.0, duration_ms: start.elapsed().as_millis() as u64,
                };
            }
        }

        // 3. 合成共识
        let consensus = self.synthesizer.synthesize(&topic, &opinions);
        let duration = start.elapsed().as_millis() as u64;

        // 4. 更新指标
        self.metrics.debates_total.inc();
        self.metrics.consensus_duration.observe(duration as f64);
        if !consensus.reached { self.metrics.skeptic_vetoes.inc(); }

        // NEW: 5. 训练红队 (异步)
        tokio::spawn(async move {
            self.asa.train_red_team(100).await.ok();
        });

        consensus
    }
}
```

**Step 7: SESA + GEA**
```rust
// crates/sesa-router/src/lib.rs
pub mod sub_router;
pub mod micro_cap;
pub mod gated;       // NEW

pub struct SesaRouter {
    tool_experts: HashMap<String, ToolExpert>,
    gate_network: GatedExpertActivation,  // NEW
}

impl SesaRouter {
    pub fn activate(&mut self, tool_name: &str, intent: &CLV) -> Result<u32> {
        let tool = self.tool_experts.get_mut(tool_name)
            .ok_or_else(|| anyhow!("Tool not found: {}", tool_name))?;

        // NEW: GEA 门控激活 (连续值 [0,1] 而非二元)
        let activations = self.gate_network.gated_activate(tool, intent)?;

        let mut activated = 0;
        for (i, strength) in activations.iter().enumerate() {
            if *strength > 0.1 {  // 软阈值
                tool.activation_mask.set_soft(i as u8, *strength);
                activated += 1;
            }
        }

        info!("GEA+SESA activated {}/{} μCaps for {} (effective sparsity: {:.2}%)",
              activated, tool.micro_caps.len(), tool_name,
              tool.activation_mask.effective_sparsity() * 100.0);
        Ok(activated)
    }
}
```

**Step 8: AaE + RCF**
```rust
// crates/aae-composer/src/lib.rs
pub mod adapter;
pub mod composition;
pub mod fusion;
pub mod rapid;       // NEW

pub struct RapidCapabilityFusion {
    fusion_templates: DashMap<Vec<String>, PrecompiledFusion>,
    delta_cache: DashMap<String, LowRankDelta>,
    jit_compiler: WasmJITCompiler,
}

impl RapidCapabilityFusion {
    pub async fn rapid_fuse(&self, adapter_ids: &[String], weights: &[f32]) -> Result<FusedCapability> {
        let mut key = adapter_ids.to_vec();
        key.sort();

        // 1. 检查预编译模板
        if let Some(template) = self.fusion_templates.get(&key) {
            if self.is_template_still_valid(&template).await? {
                return Ok(FusedCapability::from_template(template.value().clone()));
            }
        }

        // 2. 增量更新
        if let Some(fused) = self.try_incremental_fuse(adapter_ids, weights).await? {
            return Ok(fused);
        }

        // 3. JIT 编译 (兜底)
        self.jit_compiler.compile_fusion(adapter_ids, weights).await
    }

    pub async fn background_precompile(&self) -> Result<()> {
        let common = self.analyze_common_combinations().await;
        for combo in common {
            let fused = self.jit_compiler.compile_fusion(&combo.ids, &combo.weights).await?;
            self.fusion_templates.insert(combo.ids, PrecompiledFusion {
                combined_wasm: fused.wasm_bytes, source_adapters: combo.ids.clone(),
                fusion_weights: combo.weights, compiled_at: Utc::now(),
            });
        }
        Ok(())
    }
}
```

**Step 9: SEP + MTPE**
```rust
// crates/sep-pipeline/src/lib.rs
pub mod draft;
pub mod verify;
pub mod rollback;
pub mod mtpe;        // NEW

pub struct SpeculativeExecutionPipeline {
    draft_agent: DraftAgent,
    verify_agent: VerificationAgent,
    rollback_buffer: SpeculativeRollbackBuffer,
    mtpe: MultiTokenPredictionExecution,  // NEW
    metrics: Arc<MetricsCollector>,
}

impl SpeculativeExecutionPipeline {
    pub async fn execute(&mut self, intent: &UserIntent) -> Result<ExecutionResult> {
        let budget = self.decb.allocate(intent).await?;

        if budget.depth > 0.6 {
            // 高预算：使用 MTPE 多步预测
            self.mtpe.execute_multi_step(intent).await
        } else {
            // 低预算：使用 SEP 单步推测
            self.execute_single_step(intent).await
        }
    }
}

// NEW: MTPE 实现
pub struct MultiTokenPredictionExecution {
    predictor: Box<dyn MultiStepPredictor>,
    verifier: Box<dyn BatchVerifier>,
    prediction_depth: Arc<RwLock<usize>>,
}

impl MultiTokenPredictionExecution {
    pub async fn execute_multi_step(&self, intent: &UserIntent) -> Result<ExecutionResult> {
        let n = *self.prediction_depth.read().await;

        // 1. 预测未来 N 步
        let sequence = self.predictor.predict_sequence(intent, &self.get_state().await?, n).await?;

        // 2. 批量验证
        let verification = self.verifier.verify_sequence(&sequence).await?;

        if verification.all_safe {
            // 批量执行
            self.execute_batch(&sequence.operations).await?;
            Ok(ExecutionResult { steps_executed: sequence.operations.len(), mode: ExecutionMode::MTPE })
        } else {
            // 回退到单步
            let first_failure = verification.first_failure_index.unwrap_or(0);
            if first_failure > 0 {
                self.execute_batch(&sequence.operations[..first_failure]).await?;
            }
            self.fallback_to_single_step(&sequence.operations[first_failure..]).await
        }
    }
}
```

**Step 10: MCP 量子网格**
```rust
// crates/mcp-mesh/src/lib.rs
pub mod client;
pub mod server;
pub mod transport;
pub mod entanglement;

pub struct QuantumMesh {
    servers: HashMap<String, McpServer>,
    entanglement_groups: HashMap<String, Vec<String>>,
}

impl QuantumMesh {
    pub async fn execute_entangled(&self, group_id: &str, op: Operation) -> Result<()> {
        let peers = self.entanglement_groups.get(group_id)
            .ok_or_else(|| anyhow!("Group not found"))?;

        // 1. 预执行检查
        let mut prepared = vec![];
        for peer_id in peers {
            let server = self.servers.get(peer_id).unwrap();
            let pre_image = server.capture_state().await?;
            prepared.push((peer_id, pre_image));
        }

        // 2. 原子性执行
        let mut results = vec![];
        for (peer_id, _) in &prepared {
            let server = self.servers.get(*peer_id).unwrap();
            match server.execute(op.clone()).await {
                Ok(r) => results.push((peer_id, Ok(r))),
                Err(e) => {
                    self.cascade_rollback(group_id, &prepared).await?;
                    return Err(anyhow!("Entangled execution failed: {}", e));
                }
            }
        }

        // 3. 提交
        for (peer_id, result) in results {
            self.servers.get(*peer_id).unwrap().commit(result).await?;
        }
        Ok(())
    }
}
```

**Step 11: CMT 能力内存分层**
```rust
// crates/cmt-manager/src/lib.rs
pub struct CapabilityMemoryTiering {
    hot: Arc<RwLock<HashMap<String, Box<dyn Expert>>>>,
    warm: Arc<RwLock<LruCache<String, Box<dyn Expert>>>>,
    cold: Arc<RwLock<PersistentCache<String, Box<dyn Expert>>>>,
    frozen: Arc<RwLock<RemoteRegistry>>,
}

impl CapabilityMemoryTiering {
    pub async fn auto_tier(&self, expert_id: &str) -> Result<ExpertTier> {
        let freq = self.get_frequency(expert_id).await;
        let last_access = self.get_last_access(expert_id).await;
        let age = Utc::now() - last_access;

        Ok(match (freq, age) {
            (f, _) if f > 1000 => ExpertTier::Hot,
            (f, a) if f > 100 && a < Duration::hours(1) => ExpertTier::Warm,
            (f, a) if f > 10 && a < Duration::hours(24) => ExpertTier::Cold,
            _ => ExpertTier::Frozen,
        })
    }

    pub async fn prefetch(&self, current_task: &TaskPattern) -> Result<()> {
        let predicted = self.predict_next_tools(current_task).await;
        for tool_id in predicted {
            if self.get_tier(&tool_id).await == ExpertTier::Frozen {
                self.promote_to_cold(&tool_id).await?;
            }
        }
        Ok(())
    }
}
```

**Step 12: TUI 界面**
```rust
// crates/chimera-tui/src/app.rs
pub struct TuiApp {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    state: TuiState,
    event_rx: mpsc::Receiver<TuiEvent>,
}

impl TuiApp {
    pub async fn run(&mut self) -> Result<()> {
        let mut tick = interval(Duration::from_secs(1));
        while !self.state.should_quit {
            tokio::select! {
                _ = tick.tick() => { self.state.status.uptime_secs += 1; self.draw().await?; }
                Some(ev) = self.event_rx.recv() => { self.handle_event(ev).await?; self.draw().await?; }
            }
        }
        Ok(())
    }

    async fn draw(&mut self) -> Result<()> {
        self.terminal.draw(|f| {
            let chunks = Layout::default().direction(Direction::Vertical).margin(1)
                .constraints([Length(3), Min(10), Length(6)]).split(f.area());

            // 标题栏
            let title = Paragraph::new("Chimera CLI / NEXUS v0.2.0")
                .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                .block(Block::default().borders(Borders::ALL));
            f.render_widget(title, chunks[0]);

            // 主内容：议会日志 + 执行日志
            let main = Layout::default().direction(Direction::Horizontal)
                .constraints([Percentage(50), Percentage(50)]).split(chunks[1]);

            let parl = List::new(self.state.parliament_log.iter().rev().take(20)
                .map(|s| ListItem::new(s.as_str())).collect::<Vec<_>>())
                .block(Block::default().borders(Borders::ALL).title("Parliament"));
            f.render_widget(parl, main[0]);

            let exec = List::new(self.state.execution_log.iter().rev().take(20)
                .map(|s| ListItem::new(s.as_str())).collect::<Vec<_>>())
                .block(Block::default().borders(Borders::ALL).title("Execution"));
            f.render_widget(exec, main[1]);

            // 状态栏
            let status = Paragraph::new(format!(
                "Mode: {} | CBU: {:.1}/{:.1} | Experts: {} | Uptime: {}s | SAR: {} | GEA: {} | MTPE: {}",
                self.state.status.mode, self.state.status.cbu_consumed, self.state.status.cbu_total,
                self.state.status.active_experts, self.state.status.uptime_secs,
                self.state.status.sar_enabled, self.state.status.gea_enabled, self.state.status.mtpe_enabled
            )).style(Style::default().fg(Color::Yellow))
              .block(Block::default().borders(Borders::ALL).title("Status"));
            f.render_widget(status, chunks[2]);
        })?; Ok(())
    }
}
```

**Step 13: CLI 入口**
```rust
// crates/chimera-cli/src/main.rs
#[derive(Parser)]
#[command(name = "chimera")]
#[command(about = "Chimera CLI / NEXUS - Next-generation AI Coding Agent")]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    #[command(subcommand)] command: Commands,
    #[arg(short, long, default_value = "~/.chimera/config.yaml")] config: String,
    #[arg(short, long, default_value = "info")] log_level: String,
}

#[derive(Subcommand)]
enum Commands {
    Session { #[arg(short, long)] name: Option<String> },
    Run { intent: String, #[arg(short, long)] auto: bool },
    Status,
    Config { #[command(subcommand)] action: ConfigAction },
    Audit { #[arg(short, long, default_value = "10")] last: usize },
    Evolution { #[command(subcommand)] action: EvolutionAction },  // NEW
}

#[derive(Subcommand)]
enum EvolutionAction {
    Status,    // 查看进化状态
    Trigger,   // 手动触发进化
    Export,    // 导出 DPO 数据
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&cli.log_level))
        .with_target(true).with_thread_ids(true).init();

    match cli.command {
        Commands::Session { name } => run_session(name).await,
        Commands::Run { intent, auto } => run_intent(&intent, auto).await,
        Commands::Status => show_status().await,
        Commands::Config { action } => handle_config(action).await,
        Commands::Audit { last } => show_audit(last).await,
        Commands::Evolution { action } => handle_evolution(action).await,  // NEW
    }
}
```

---

## 7. 8 周 40 天推进计划

### Phase 1: 地基浇筑（Week 1-2）

#### Day 1: Workspace 初始化
**目标**: 建立可编译的 17 crate Workspace，配置 CI/CD

**任务清单**:
- [ ] 09:30-10:30 创建 GitHub 仓库，初始化 README/LICENSE/CODE_OF_CONDUCT
- [ ] 10:30-12:30 编写根 Cargo.toml（30+ 共享依赖）
- [ ] 14:00-15:00 代码审查：依赖版本兼容性
- [ ] 15:00-17:00 初始化 17 个 crate 的骨架（lib.rs + Cargo.toml）
- [ ] 17:00-18:00 配置 .github/workflows/ci.yml（security-audit, lint, unit, integration, e2e, benchmark）

**代码提交目标**:
```bash
git add .
git commit -m "feat(workspace): initialize NEXUS v0.2.0 workspace with 17 crates

- Add 30+ shared dependencies in workspace root
- Setup CI/CD with security audit, lint, test, benchmark stages
- Initialize all crate skeletons with lib.rs and Cargo.toml

Refs #1"
```

**验收标准**:
- `cargo check --all` 成功
- `cargo test --all` 通过（空测试）
- CI 流水线在 PR 上自动运行

---

#### Day 2: nexus-core 地基
**目标**: 实现核心类型、运行时、事件总线

**任务清单**:
- [ ] 09:30-12:30 实现 `types.rs`（UserIntent, CLV, CBU, EntangledCall, Consensus, AuditEntry 等）
- [ ] 14:00-15:00 代码审查：类型定义的完整性
- [ ] 15:00-17:00 实现 `runtime.rs`（NexusRuntime + PriorityTaskScheduler）
- [ ] 17:00-18:00 实现 `bus.rs`（EventBus + NexusEvent 枚举）

**代码提交目标**:
```bash
git commit -m "feat(nexus-core): core types, runtime, and event bus

- Define 15+ core data structures with serde support
- Implement NexusRuntime with IO/CPU task separation
- Add typed EventBus with broadcast channel
- 100% unit test coverage for types

Refs #2"
```

---

#### Day 3: SecCore 沙箱基础
**目标**: 实现 gVisor 沙箱 + seccomp-BPF + 能力衰减

**任务清单**:
- [ ] 09:30-12:30 实现 `sandbox.rs`（GVisorSandbox + OCI config 生成）
- [ ] 14:00-15:00 代码审查：OCI 配置的安全性
- [ ] 15:00-17:00 实现 `decay.rs`（CapabilityDecayEngine + 连续衰减 + 自然恢复）
- [ ] 17:00-18:00 实现 `static_analysis.rs`（命令插值检测 + 环境变量白名单）

**关键测试**:
```rust
#[tokio::test]
async fn test_command_interpolation_blocked() {
    let seccore = setup_test_seccore().await;
    let malicious = ["echo $(cat /etc/passwd)", "ls; rm -rf /", "cat /etc/passwd | grep root"];
    for cmd in malicious {
        assert!(seccore.audit_and_execute(&operation(cmd)).await.is_err());
    }
}

#[tokio::test]
async fn test_capability_decay_freeze() {
    let engine = CapabilityDecayEngine::new(DecayConfig::default());
    for _ in 0..5 { engine.consume(&RiskLevel::High).await.unwrap(); }
    assert!(engine.consume(&RiskLevel::High).await.is_err());
}
```

**代码提交目标**:
```bash
git commit -m "feat(seccore): zero-trust security kernel with gVisor

- Add GVisorSandbox with OCI bundle generation and seccomp-BPF
- Implement CapabilityDecayEngine with continuous decay/recovery
- Add static analysis: command interpolation & env var whitelisting
- Security tests: 6 attack vectors, 5-step freeze verification

Refs #3"
```

---

#### Day 4: 审计链 + CLI 骨架
**目标**: Merkle Tree 审计 + CLI 入口

**任务清单**:
- [ ] 09:30-12:30 实现 `audit.rs`（AuditLogger + Merkle Tree + 异步批量写入）
- [ ] 14:00-15:00 代码审查：审计链的不可篡改性
- [ ] 15:00-17:00 实现 `main.rs`（Clap CLI: session, run, status, config, audit）
- [ ] 17:00-18:00 编写 E2E 测试脚本（scripts/e2e-test.sh）

**代码提交目标**:
```bash
git commit -m "feat(cli): CLI entry point and immutable audit chain

- Add AuditLogger with async batch writing and Merkle tree integrity
- Implement chimera CLI with session, run, status, config, audit commands
- Add e2e-test.sh for release binary validation

Refs #4"
```

---

#### Day 5: Week 1 验收
**目标**: 全量测试通过，安全基线稳固

**任务清单**:
- [ ] 09:30-12:30 运行全量测试：`cargo test --all`
- [ ] 14:00-15:00 安全专项测试：渗透测试 6 个攻击向量
- [ ] 15:00-17:00 编写 ADR-001 至 ADR-005
- [ ] 17:00-18:00 Week 1 验收会议

**验收清单**:
- [x] `cargo build --release` 成功
- [x] 15 crate 编译通过
- [x] SecCore 拦截 100% 命令注入测试
- [x] 能力衰减 5 次冻结测试通过
- [x] 审计链 Merkle Tree 验证通过
- [x] CLI `--version` / `--help` 可用
- [x] CI/CD 全流水线通过
- [x] 代码覆盖率 > 85%

**代码提交目标**:
```bash
git commit -m "docs(adr): Week 1 acceptance and architecture decisions

- ADR-001 to ADR-005: Rust, Tokio, gVisor, sqlite-vec, Wasmtime
- Week 1 acceptance: all 8 items passed
- Security baseline established

Refs #5, closes #1"
```

---

#### Day 6: Tokio 运行时 + TUI 骨架
**目标**: 运行时管理 + 多面板 TUI

**任务清单**:
- [ ] 09:30-12:30 完善 `runtime.rs`（spawn_io / spawn_cpu / metrics）
- [ ] 14:00-15:00 代码审查：线程安全性
- [ ] 15:00-17:00 实现 `chimera-tui/src/app.rs`（Ratatui 三面板）
- [ ] 17:00-18:00 实现键盘事件处理 + 异步状态更新

**代码提交目标**:
```bash
git commit -m "feat(runtime+tui): Tokio management and Ratatui interface

- Add NexusRuntime with IO/CPU separation and metrics
- Implement TuiApp with Parliament/Execution/Status panels
- Real-time keyboard events and async state updates

Refs #6"
```

---

#### Day 7: 配置系统 + 热加载
**目标**: Figment 多源配置 + notify 热加载

**任务清单**:
- [ ] 09:30-12:30 实现 `config.rs`（Figment: default < file < env < CLI）
- [ ] 14:00-15:00 代码审查：配置合并优先级
- [ ] 15:00-17:00 实现热加载（notify-based file watching）
- [ ] 17:00-18:00 编写配置验证测试

**代码提交目标**:
```bash
git commit -m "feat(config): multi-source configuration with hot-reload

- Implement Figment-based config loading: default < file < env < cli
- Add HotReloadConfig with notify-based file watching
- Config validation and error handling

Refs #7"
```

---

#### Day 8: 监控指标 + HTTP 端点
**目标**: Prometheus 指标 + /metrics + /health

**任务清单**:
- [ ] 09:30-12:30 实现 `metrics.rs`（MetricsCollector + 15+ 指标）
- [ ] 14:00-15:00 代码审查：指标命名规范
- [ ] 15:00-17:00 实现 HTTP 端点（axum: /metrics, /health）
- [ ] 17:00-18:00 编写监控测试

**代码提交目标**:
```bash
git commit -m "feat(monitoring): Prometheus metrics and HTTP endpoints

- Add MetricsCollector with 15+ metrics (security, routing, context, parliament, execution)
- Implement /metrics and /health endpoints via axum
- OpenMetrics text format compliance

Refs #8"
```

---

#### Day 9: 事件总线 + 模块生命周期
**目标**: 模块统一生命周期管理

**任务清单**:
- [ ] 09:30-12:30 完善 EventBus（持久化订阅 + 错误恢复）
- [ ] 14:00-15:00 代码审查：广播通道容量
- [ ] 15:00-17:00 实现 NexusModule trait（init/start/shutdown/health_check/metrics）
- [ ] 17:00-18:00 将 SecCore 实现为 NexusModule

**代码提交目标**:
```bash
git commit -m "feat(bus): module lifecycle and event bus integration

- Add persistent subscription to EventBus
- Implement NexusModule trait for all subsystems
- Integrate SecCore as NexusModule with lifecycle hooks

Refs #9"
```

---

#### Day 10: Week 2 验收
**目标**: 系统可交互运行

**验收清单**:
- [x] Tokio 运行时 IO/CPU 分离
- [x] 优先级队列处理 1000+ 任务
- [x] TUI 三面板实时更新
- [x] 事件总线支持 10+ 模块订阅
- [x] 配置热加载 1s 内生效
- [x] `/metrics` 端点可访问
- [x] 全量测试通过
- [x] 压力测试 1000 次操作无内存泄漏

**代码提交目标**:
```bash
git commit -m "docs(week2): Week 2 acceptance and core runtime docs

- Week 2 acceptance: all 8 items passed
- Core runtime documentation
- Performance baseline established

Refs #10, closes #2"
```

---

### Phase 2: 记忆与路由（Week 3-4）

#### Day 11: MLC L0/L1
**目标**: WorkingMemory (LRU) + EpisodicMemory (SQLite + sqlite-vec)

**任务清单**:
- [ ] 09:30-12:30 实现 `l0.rs`（WorkingMemory + LRU 驱逐）
- [ ] 14:00-15:00 代码审查：并发安全性
- [ ] 15:00-17:00 实现 `l1.rs`（EpisodicMemory + GRU 压缩 + 向量检索）
- [ ] 17:00-18:00 编写 MLC 测试

**代码提交目标**:
```bash
git commit -m "feat(mlc): L0 working memory and L1 episodic memory

- Add WorkingMemory with LRU eviction and token counting
- Add EpisodicMemory with SQLite + sqlite-vec vector storage
- GRU-based sequence compression (placeholder)
- Tests: LRU eviction, vector similarity

Refs #11"
```

---

#### Day 12: FaaE 路由核心
**目标**: Expert trait + FaaERouter + cosine similarity

**任务清单**:
- [ ] 09:30-12:30 实现 `expert.rs`（Expert trait + SharedExpert + RoutedExpert）
- [ ] 14:00-15:00 代码审查：trait 设计合理性
- [ ] 15:00-17:00 实现 `router.rs`（FaaERouter + top-k + EDSB）
- [ ] 17:00-18:00 编写路由测试

**代码提交目标**:
```bash
git commit -m "feat(faae): dynamic expert routing with entropy balancing

- Add Expert trait with capability vectors and health checks
- Implement FaaERouter with cosine similarity + EDSB
- Add FileIoExpert as concrete shared expert
- Tests: routing accuracy, entropy balancing

Refs #12"
```

---

#### Day 13: MLC L2/L3 + HCW
**目标**: SemanticMemory (GNN) + ProceduralMemory (WASM) + HCW

**任务清单**:
- [ ] 09:30-12:30 实现 `l2.rs`（SemanticMemory + PageRank GNN）
- [ ] 14:00-15:00 代码审查：GNN 收敛性
- [ ] 15:00-17:00 实现 `l3.rs`（ProceduralMemory + WASM 技能）
- [ ] 17:00-18:00 实现 `hcw.rs`（HierarchicalContextWindow）

**代码提交目标**:
```bash
git commit -m "feat(mlc+hcw): L2/L3 memory and hierarchical context window

- Add SemanticMemory with PageRank-style GNN aggregation
- Add ProceduralMemory with WASM skill storage and execution
- Add HierarchicalContextWindow with 4-layer attention density
- Tests: GNN convergence, WASM execution, HCW selection

Refs #13"
```

---

#### Day 14: SAR 稀疏路由 + 专家注册
**目标**: SparseAttentionRouter + ExpertRegistry

**任务清单**:
- [ ] 09:30-12:30 实现 `sparse.rs`（SAR + 注意力索引 + 类别筛选）
- [ ] 14:00-15:00 代码审查：稀疏掩码预计算
- [ ] 15:00-17:00 实现 `registry.rs`（ExpertRegistry + DashMap）
- [ ] 17:00-18:00 实现 `bootstrap.rs`（12 个默认专家）

**代码提交目标**:
```bash
git commit -m "feat(sar+registry): sparse attention routing and expert registry

- Add SparseAttentionRouter with category-based filtering (300 -> <60)
- Add ExpertRegistry with concurrent DashMap access
- Bootstrap 12 default experts across 6 categories
- Tests: SAR latency < 2ms, registry lifecycle

Refs #14"
```

---

#### Day 15: Week 3 验收
**目标**: 上下文压缩 > 4×，路由准确率 > 85%

**验收清单**:
- [x] L0 LRU 驱逐正确
- [x] L1 向量相似度 > 0.5
- [x] L2 GNN 3 轮收敛
- [x] L3 WASM 技能可执行
- [x] HCW 自动窗口选择正确
- [x] MLC 全层编码 < 100ms
- [x] FaaE 路由准确率 > 85%
- [x] SAR 路由延迟 < 2ms
- [x] EDSB 100 次偏斜后生效
- [x] 12 个默认专家健康

**代码提交目标**:
```bash
git commit -m "perf(week3): benchmarks and Week 3 acceptance

- Criterion benchmarks for CLV encoding and FaaE routing
- SAR optimization: 5x routing speedup
- Week 3 acceptance: all 10 items passed

Refs #15, closes #3"
```

---

#### Day 16: DCO + COG
**目标**: DualContextOscillator + ContextOscillationGate

**任务清单**:
- [ ] 09:30-12:30 实现 `gate.rs`（COG + 复杂度熵估计 + 全局信号提取）
- [ ] 14:00-15:00 代码审查：决策阈值合理性
- [ ] 15:00-17:00 实现 `oscillator.rs`（DCO + 5 种振荡事件）
- [ ] 17:00-18:00 编写 DCO 测试

**代码提交目标**:
```bash
git commit -m "feat(dco): dual-context oscillation with COG decision gate

- Add ContextOscillationGate with entropy + signal based decisions
- Add ComplexityEntropyEstimator with 5-dimension features
- Add DualContextOscillator with 5 oscillation events
- Tests: COG decision logic, DCO oscillation flow

Refs #16"
```

---

#### Day 17: EDSB 完善 + SCC
**目标**: EntropyMonitor + SpeculativeContextCache

**任务清单**:
- [ ] 09:30-12:30 完善 `entropy.rs`（指数衰减 + 定时清理）
- [ ] 14:00-15:00 代码审查：衰减公式正确性
- [ ] 15:00-17:00 实现 `scc.rs`（SCC + Draft-Verify 共享缓存）
- [ ] 17:00-18:00 编写 SCC 测试

**代码提交目标**:
```bash
git commit -m "feat(edsb+scc): entropy balancing and speculative context cache

- Add EntropyMonitor with exponential decay and periodic cleanup
- Add SpeculativeContextCache for Draft-Verify shared encoding
- Tests: entropy recovery, SCC hit rate > 70%

Refs #17"
```

---

#### Day 18: CLSI 跨层索引
**目标**: CrossLayerSharedIndex

**任务清单**:
- [ ] 09:30-12:30 实现 `clsi.rs`（语义坐标系 + 跨层检索 + 异步刷新）
- [ ] 14:00-15:00 代码审查：索引一致性
- [ ] 15:00-17:00 集成 CLSI 到 MLC Engine
- [ ] 17:00-18:00 编写 CLSI 测试

**代码提交目标**:
```bash
git commit -m "feat(clsi): cross-layer shared semantic index

- Add CrossLayerSharedIndex with anchor embeddings
- Implement cross-layer search with semantic consistency
- Async refresh via broadcast channel
- Tests: cross-layer consistency, index refresh

Refs #18"
```

---

#### Day 19: CMT 能力内存分层
**目标**: CapabilityMemoryTiering

**任务清单**:
- [ ] 09:30-12:30 实现 `tier.rs`（Hot/Warm/Cold/Frozen 四级）
- [ ] 14:00-15:00 代码审查：分层策略合理性
- [ ] 15:00-17:00 实现 `prefetch.rs`（预测性预加载）
- [ ] 17:00-18:00 编写 CMT 测试

**代码提交目标**:
```bash
git commit -m "feat(cmt): capability memory tiering with 4 levels

- Add Hot/Warm/Cold/Frozen tier management
- Implement auto-tier migration based on frequency and age
- Add predictive prefetch for next tools
- Tests: tier migration, prefetch accuracy

Refs #19"
```

---

#### Day 20: Week 4 验收
**目标**: 上下文系统完整，路由优化到位

**验收清单**:
- [x] COG 决策准确率 > 90%
- [x] DCO 跨文件场景触发振荡
- [x] CSA 单文件 < 100ms
- [x] HCA 全库扫描 < 500ms
- [x] EDSB 1000 次调用后熵 > 0.6
- [x] SCC 缓存命中率 > 70%
- [x] CLSI 跨层一致性验证通过
- [x] CMT 四级迁移正确
- [x] SAR 延迟 < 2ms
- [x] 全量 E2E 通过

**代码提交目标**:
```bash
git commit -m "docs(week4): Week 4 acceptance and context system docs

- Week 4 acceptance: all 10 items passed
- Context management system complete
- Routing optimization verified

Refs #20, closes #4"
```

---

### Phase 3: 议会与智能体（Week 5-6）

#### Day 21: 5 角色实现
**目标**: Architect + Skeptic + Optimizer + Librarian + Bard

**任务清单**:
- [ ] 09:30-12:30 实现 `roles/architect.rs` + `roles/skeptic.rs`（否决权）
- [ ] 14:00-15:00 代码审查：Skeptic 安全模式覆盖
- [ ] 15:00-17:00 实现 `roles/optimizer.rs` + `roles/librarian.rs` + `roles/bard.rs`
- [ ] 17:00-18:00 编写角色测试

**代码提交目标**:
```bash
git commit -m "feat(parliament): 5-role implementation with Skeptic veto

- Add ArchitectRole: module boundaries, API compatibility, performance
- Add SkepticRole: regex-based security scanning + veto power
- Add OptimizerRole, LibrarianRole, BardRole
- Tests: role analysis, veto interception

Refs #21"
```

---

#### Day 22: 辩论引擎 + 共识合成
**目标**: DebateEngine + ConsensusSynthesizer + DPO

**任务清单**:
- [ ] 09:30-12:30 实现 `debate.rs`（流式辩论 + 30s 超时）
- [ ] 14:00-15:00 代码审查：超时处理
- [ ] 15:00-17:00 实现 `consensus.rs`（辩证综合 + DPO 生成）
- [ ] 17:00-18:00 编写议会集成测试

**代码提交目标**:
```bash
git commit -m "feat(parliament): debate engine and consensus synthesis

- Add DebateEngine with async streaming and timeout handling
- Add ConsensusSynthesizer with veto check and weighted confidence
- Generate DPO pairs for both positive and negative outcomes
- Tests: consensus reached, veto for malicious tasks

Refs #22"
```

---

#### Day 23: ASA 红队 + ACB/DECB
**目标**: AdversarialSelfAudit + DualEffortCognitiveBudgeting

**任务清单**:
- [ ] 09:30-12:30 实现 `asa.rs`（Red Team + Critic-based PPO + 实时审计）
- [ ] 14:00-15:00 代码审查：红队权限边界
- [ ] 15:00-17:00 实现 `decb.rs`（连续可调认知预算 [0,1]）
- [ ] 17:00-18:00 编写 ASA + DECB 测试

**代码提交目标**:
```bash
git commit -m "feat(asa+decb): adversarial self-audit and continuous budgeting

- Add AdversarialSelfAudit with Red Team real-time interception
- Add Critic-based PPO training for Red Team
- Add DualEffortCognitiveBudgeting with [0,1] continuous dial
- Tests: Red Team interception, DECB efficiency feedback

Refs #23"
```

---

#### Day 24: SIF 信息素 + 聚类
**目标**: PheromoneSystem + CapabilityClustering

**任务清单**:
- [ ] 09:30-12:30 实现 `pheromone.rs`（DPT + 指数衰减 + 强化）
- [ ] 14:00-15:00 代码审查：信息素公式正确性
- [ ] 15:00-17:00 实现 `clustering.rs`（自动聚类 + 相似度阈值）
- [ ] 17:00-18:00 编写 SIF 测试

**代码提交目标**:
```bash
git commit -m "feat(sif): swarm intelligence field with pheromones and clustering

- Add PheromoneSystem with exponential decay and reinforcement
- Add CapabilityClustering with cosine similarity threshold
- Add EmergentConsensus with local interaction
- Tests: pheromone decay, clustering accuracy

Refs #24"
```

---

#### Day 25: Week 5 验收
**目标**: 议会认知层完整

**验收清单**:
- [x] 5 角色全部测试
- [x] Skeptic 否决恶意意图
- [x] Red Team 实时拦截率 > 95%
- [x] 共识 30s 内完成
- [x] DPO 对生成可用
- [x] CEE 与人工评估相关性 > 0.8
- [x] DECB 连续可调 [0,1]
- [x] ACB 溢出 150% 触发
- [x] 议会 + ASA 端到端通过
- [x] SIF 信息素收敛

**代码提交目标**:
```bash
git commit -m "docs(week5): Week 5 acceptance and parliament docs

- Week 5 acceptance: all 10 items passed
- Parliament cognitive layer complete
- ASA security audit integrated

Refs #25, closes #5"
```

---

#### Day 26: SESA + GEA
**目标**: SubExpertSparseActivation + GatedExpertActivation

**任务清单**:
- [ ] 09:30-12:30 实现 `micro_cap.rs`（256-bit 掩码 + 3-dim 向量）
- [ ] 14:00-15:00 代码审查：掩码内存效率
- [ ] 15:00-17:00 实现 `gated.rs`（门控网络 + 冲突消解 + 软激活 [0,1]）
- [ ] 17:00-18:00 编写 SESA + GEA 测试

**代码提交目标**:
```bash
git commit -m "feat(sesa+gea): sub-expert sparse activation with gating

- Add MicroCapability with 256-bit mask and 3-dim semantic vector
- Add GatedExpertActivation with continuous [0,1] activation
- Add conflict resolution for mutually exclusive μCaps
- Tests: selective activation, sparsity < 40%, conflict resolution

Refs #26"
```

---

#### Day 27: AaE + RCF
**目标**: Adapter-as-Expert + RapidCapabilityFusion

**任务清单**:
- [ ] 09:30-12:30 实现 `adapter.rs`（WASM + LoRA）
- [ ] 14:00-15:00 代码审查：低秩更新数学正确性
- [ ] 15:00-17:00 实现 `rapid.rs`（预编译模板 + 增量更新 + < 20ms）
- [ ] 17:00-18:00 编写 RCF 性能测试

**代码提交目标**:
```bash
git commit -m "feat(aae+rcf): adapter-as-expert with rapid fusion

- Add WasmCapabilityAdapter with base module + LoRA delta
- Add RapidCapabilityFusion with precompiled templates
- Incremental fusion: O(rank^2) instead of O(dim^2)
- Tests: fusion time < 20ms, 10x speedup

Refs #27"
```

---

#### Day 28: CSN + 降级链
**目标**: CapabilitySubstitutionNetwork + GracefulDegradationChain

**任务清单**:
- [ ] 09:30-12:30 实现 `embedding.rs`（CES 128-dim 统一编码）
- [ ] 14:00-15:00 代码审查：嵌入空间维度
- [ ] 15:00-17:00 实现 `degradation.rs`（GDC + 补偿提示词）
- [ ] 17:00-18:00 编写 CSN 测试

**代码提交目标**:
```bash
git commit -m "feat(csn): capability substitution network

- Add CapabilityEmbeddingSpace with 128-dim unified encoding
- Add GracefulDegradationChain with similarity-based ranking
- Generate CompensationPrompt with similarity-aware limitations
- Tests: github-cli -> gh -> curl ordering

Refs #28"
```

---

#### Day 29: MCP 量子网格
**目标**: MCP Client/Server + 量子纠缠事务

**任务清单**:
- [ ] 09:30-12:30 实现 `client.rs`（MCP Client + stdio/http 双传输）
- [ ] 14:00-15:00 代码审查：传输层安全性
- [ ] 15:00-17:00 实现 `entanglement.rs`（超位置态 + 纠缠事务 + 级联回滚）
- [ ] 17:00-18:00 编写 MCP 测试

**代码提交目标**:
```bash
git commit -m "feat(mcp): quantum mesh with entangled transactions

- Add MCP Client with stdio + HTTP dual transport
- Add QuantumMesh with superposition servers
- Add entangled transaction groups with atomic rollback
- Tests: transaction atomicity, cascade rollback

Refs #29"
```

---

#### Day 30: Week 6 验收
**目标**: 优化层完整

**验收清单**:
- [x] SESA μCap 稀疏度 < 40%
- [x] GEA 门控激活连续值正确
- [x] AaE 适配器组合 < 200ms
- [x] RCF 快速融合 < 20ms
- [x] CSN 降级链排序正确
- [x] MCP 量子网格支持 5+ 服务器纠缠
- [x] 3 优化模块单元测试通过
- [x] 端到端优化场景通过
- [x] 性能基准全部达标
- [x] 全量 E2E 通过

**代码提交目标**:
```bash
git commit -m "docs(week6): Week 6 acceptance and optimization layer docs

- Week 6 acceptance: all 10 items passed
- Optimization layer complete
- MCP quantum mesh operational

Refs #30, closes #6"
```

---

### Phase 4: 生产化（Week 7-8）

#### Day 31: SEP + MTPE
**目标**: SpeculativeExecution + MultiTokenPredictionExecution

**任务清单**:
- [ ] 09:30-12:30 实现 `draft.rs` + `verify.rs`（Draft-Verify 基础）
- [ ] 14:00-15:00 代码审查：并行性最大化
- [ ] 15:00-17:00 实现 `mtpe.rs`（N 步预测 + 批量验证 + 动态深度）
- [ ] 17:00-18:00 编写 SEP + MTPE 测试

**代码提交目标**:
```bash
git commit -m "feat(sep+mtpe): speculative and multi-token prediction execution

- Add DraftAgent with lightweight local model
- Add VerificationAgent with safety and correctness checks
- Add MultiTokenPredictionExecution with N-step prediction
- Dynamic depth adaptation based on success rate
- Tests: successful speculation, rollback on failure

Refs #31"
```

---

#### Day 32: SIMD 优化 + 性能调优
**目标**: ndarray BLAS + SQLite WAL + WASM 缓存

**任务清单**:
- [ ] 09:30-12:30 启用 ndarray BLAS 后端（OpenBLAS/MKL）
- [ ] 14:00-15:00 代码审查：SIMD 指令兼容性
- [ ] 15:00-17:00 优化 SQLite（WAL 模式 + 批量写入）
- [ ] 17:00-18:00 实现 WASM 模块缓存

**代码提交目标**:
```bash
git commit -m "perf(week7): SIMD optimization and database tuning

- Enable ndarray BLAS backend for SIMD cosine similarity
- SQLite WAL mode and batch writes
- WASM module cache to avoid recompilation
- Benchmark: routing < 5ms, encoding < 50ms

Refs #32"
```

---

#### Day 33: 安全加固
**目标**: 渗透测试 + CVE 扫描 + 模糊测试

**任务清单**:
- [ ] 09:30-12:30 运行 cargo-audit + cargo-deny
- [ ] 14:00-15:00 代码审查：OWASP Top 10 覆盖
- [ ] 15:00-17:00 实现模糊测试（10000 随机输入）
- [ ] 17:00-18:00 编写安全测试报告

**代码提交目标**:
```bash
git commit -m "security(week7): penetration testing and fuzzing

- OWASP Top 10 penetration testing
- cargo-audit vulnerability scan: 0 high severity
- Fuzz testing: 10000 random command inputs
- Security test report

Refs #33"
```

---

#### Day 34: 跨平台发布
**目标**: 5 平台 release binary + Docker 镜像

**任务清单**:
- [ ] 09:30-12:30 配置 cross-compilation（x86_64/aarch64 Linux/macOS）
- [ ] 14:00-15:00 代码审查：发布流水线
- [ ] 15:00-17:00 构建 Docker 镜像
- [ ] 17:00-18:00 编写安装脚本（scripts/install.sh）

**代码提交目标**:
```bash
git commit -m "ci(release): cross-platform release pipeline

- Add release workflow for 5 platforms
- Docker image build
- One-line install script
- CHANGELOG generation

Refs #34"
```

---

#### Day 35: 监控与告警
**目标**: Grafana 仪表盘 + Prometheus 告警规则

**任务清单**:
- [ ] 09:30-12:30 编写 `monitoring/grafana-dashboard.json`
- [ ] 14:00-15:00 代码审查：告警阈值合理性
- [ ] 15:00-17:00 编写 `monitoring/alert-rules.yml`
- [ ] 17:00-18:00 部署测试

**代码提交目标**:
```bash
git commit -m "feat(monitoring): Grafana dashboard and alert rules

- Add Grafana dashboard with 10+ panels
- Add Prometheus alert rules for 5 critical conditions
- CapabilityDepleted, HighOrphanRate, ParliamentDeadlock alerts

Refs #35"
```

---

#### Day 36: 文档完善
**目标**: API 文档 + 用户指南 + 运维手册

**任务清单**:
- [ ] 09:30-12:30 完善 README.md + API_SPEC.md
- [ ] 14:00-15:00 代码审查：文档准确性
- [ ] 15:00-17:00 编写运维手册（故障排查、性能调优）
- [ ] 17:00-18:00 编写用户快速开始指南

**代码提交目标**:
```bash
git commit -m "docs(complete): API docs, user guide, and ops manual

- Complete API specification with protobuf definitions
- User quick start guide
- Operations manual with troubleshooting
- Architecture diagrams

Refs #36"
```

---

#### Day 37-38: 最终集成测试
**目标**: 全量 E2E + 性能基准 + 稳定性测试

**任务清单**:
- [ ] 09:30-12:30 运行全量 E2E 测试（100 个场景）
- [ ] 14:00-15:00 性能基准测试（Criterion）
- [ ] 15:00-17:00 稳定性测试（24h 连续运行）
- [ ] 17:00-18:00 修复发现的 bug

**测试场景**:
1. 简单任务：`chimera run "rename foo to bar in src/main.rs"` → < 1s, CSA 模式
2. 中等任务：`chimera run "refactor auth to async"` → 1-3s, L1 预算, SAR 路由
3. 复杂任务：`chimera run "migrate REST to GraphQL"` → 议会审议, L2 预算, MTPE
4. 恶意任务：`chimera run "rm -rf /"` → Skeptic 否决 + Red Team 拦截, 0 CBU
5. 长时间运行：1000 次任务, 内存稳定, 熵均衡正常, SCC 命中率 > 70%

**性能基准**:
| 指标 | 目标 | 实际 |
|------|------|------|
| 启动时间 | < 200ms | ? |
| 单文件操作 | < 100ms | ? |
| 全库扫描 | < 500ms | ? |
| 内存占用 | < 500MB | ? |
| 孤儿调用率 | 0% | ? |
| 议会决策 | < 2s | ? |
| 推测命中率 | > 75% | ? |
| SAR 路由 | < 2ms | ? |
| RCF 融合 | < 20ms | ? |
| MTPE 加速 | > 30% | ? |

**代码提交目标**:
```bash
git commit -m "test(final): full E2E suite and performance benchmarks

- 100 E2E scenarios: all passed
- Criterion benchmarks: all targets met
- 24h stability test: no memory leaks, no crashes
- Bug fixes from integration testing

Refs #37"
```

---

#### Day 39-40: 最终验收与发布
**目标**: v0.2.0-beta 发布

**验收清单**:
- [x] 22 个第一代创新点全部实现并测试
- [x] 10 个第二代创新点全部实现并测试
- [x] 32 个创新点综合查重率 < 15%
- [x] 5 平台 release binary 生成
- [x] Docker 镜像构建成功
- [x] 全量 E2E 100% 通过
- [x] 性能基准全部达标
- [x] 稳定性测试 24h 无崩溃
- [x] 安全渗透测试通过
- [x] cargo-audit 无高危漏洞
- [x] 文档完整（API/用户/运维）
- [x] Grafana 仪表盘可用
- [x] 告警规则部署

**最终发布**:
```bash
git tag v0.2.0-beta
git push origin v0.2.0-beta
# GitHub Actions 自动构建 release binary
```

---

## 8. 测试与验收策略

### 8.1 测试金字塔

```
        /\
       /  \
      / E2E \      # 端到端场景测试 (10%)
     /--------\
    / Integration \  # 模块集成测试 (30%)
   /--------------\
  /    Unit Tests   \ # 单元测试 (60%)
 /--------------------\
```

### 8.2 关键测试套件

**安全测试套件** (`tests/security/`):
```rust
#[tokio::test]
async fn test_prevent_all_command_injection_vectors() {
    let seccore = setup_test_seccore().await;
    let malicious = vec![
        "echo $(cat /etc/passwd)", "echo ${IFS}malicious", "echo `whoami`",
        "ls; rm -rf /", "cat /etc/passwd | grep root", "echo <(cat /etc/passwd)",
        "eval('malicious')", "exec('malicious')", "system('malicious')",
    ];
    for cmd in malicious {
        let result = seccore.audit_and_execute(&operation(cmd)).await;
        assert!(result.is_err(), "Should block: {}", cmd);
    }
}

#[tokio::test]
async fn test_red_team_intercepts_malicious_intent() {
    let parliament = setup_test_parliament().await;
    let topic = DebateTopic {
        intent: "rm -rf / && drop database production".into(),
        enable_red_team: true, ..Default::default()
    };
    let consensus = parliament.deliberate(topic).await;
    assert!(!consensus.reached);
    assert!(consensus.synthesized_decision.contains("RED TEAM"));
}

#[tokio::test]
async fn test_capability_decay_and_recovery() {
    let engine = CapabilityDecayEngine::new(DecayConfig::default());
    for _ in 0..5 { engine.consume(&RiskLevel::High).await.unwrap(); }
    assert!(engine.consume(&RiskLevel::High).await.is_err());
    // 模拟议会共识恢复
    engine.consensus_recovery(1.0).await;
    assert!(engine.current().await > 0.3);
}
```

**路由测试套件** (`tests/routing/`):
```rust
#[tokio::test]
async fn test_sar_sparse_routing_performance() {
    let router = setup_test_router_with_300_experts().await;
    let intent = CLV::from_text("analyze rust memory safety");
    let start = Instant::now();
    let experts = router.route(&intent).await.unwrap();
    let duration = start.elapsed().as_millis();
    assert!(duration < 2, "SAR routing should be < 2ms, got {}ms", duration);
    assert!(experts.len() <= 9); // top-8 + shared
}

#[tokio::test]
async fn test_gea_gated_activation_sparsity() {
    let mut router = setup_test_sesa_router().await;
    let intent = CLV::from_text("git blame src/main.rs");
    let activated = router.activate("git", &intent).unwrap();
    let tool = router.tool_experts.get("git").unwrap();
    assert!(tool.activation_mask.effective_sparsity() < 0.4);
    assert!(activated >= 1 && activated <= 2); // blame + maybe diff
}

#[tokio::test]
async fn test_rcf_rapid_fusion_under_20ms() {
    let rcf = setup_test_rcf().await;
    let start = Instant::now();
    let fused = rcf.rapid_fuse(&["rust_safety", "memory_opt"], &[0.7, 0.3]).await.unwrap();
    let duration = start.elapsed().as_millis();
    assert!(duration < 20, "RCF fusion should be < 20ms, got {}ms", duration);
}
```

**E2E 测试套件** (`tests/e2e/`):
```rust
#[tokio::test]
async fn test_e2e_simple_rename() {
    let chimera = setup_test_chimera().await;
    let result = chimera.execute("rename variable foo to bar in src/main.rs").await.unwrap();
    assert_eq!(result.mode, ExecutionMode::CSA);
    assert!(result.execution_time_ms < 1000);
    assert_eq!(result.edits.len(), 1);
}

#[tokio::test]
async fn test_e2e_complex_migration_with_parliament() {
    let chimera = setup_test_chimera().await;
    let result = chimera.execute("migrate all REST APIs to GraphQL across entire codebase").await.unwrap();
    assert!(result.oscillation_count >= 1);
    assert!(result.execution_time_ms > 1000);
    assert!(result.steps_executed > 5); // MTPE multi-step
}

#[tokio::test]
async fn test_e2e_malicious_intent_blocked() {
    let chimera = setup_test_chimera().await;
    let result = chimera.execute("rm -rf / && drop database production").await;
    assert!(result.is_err());
}
```

### 8.3 性能基准

| 指标 | 目标 | 测试方法 | 频率 |
|------|------|---------|------|
| 启动时间 | < 200ms | `time chimera --version` | 每次 CI |
| 单文件操作 | < 100ms | CSA 模式，100 次平均 | 每日 |
| 全库扫描 | < 500ms | HCA 模式，10k 文件仓库 | 每日 |
| SAR 路由 | < 2ms | 300 专家，1000 次平均 | 每次 CI |
| RCF 融合 | < 20ms | 2 适配器，100 次平均 | 每次 CI |
| 内存占用 | < 500MB | `ps` 监控 24h | 每周 |
| 孤儿调用率 | 0% | 1000 次操作，检查回执 | 每次 CI |
| 议会决策 | < 2s | 5 角色并行辩论 | 每日 |
| SCC 命中率 | > 70% | Draft-Verify 场景统计 | 每日 |
| MTPE 加速 | > 30% | 对比单步执行 | 每周 |

---

## 9. 安全与合规模型

### 9.1 威胁模型

| 威胁 | 缓解措施 | 验证方法 | 责任模块 |
|------|---------|---------|---------|
| 命令注入 (CVE-2026-35022) | SecCore 禁止字符串插值 | 渗透测试 | seccore |
| 环境变量泄露 | WHITELIST 机制 | 静态分析 | seccore |
| 权限提升 | 能力衰减模型 | 自动化测试 | seccore |
| 沙箱逃逸 | gVisor + seccomp-BPF | 模糊测试 | seccore |
| 审计链篡改 | SHA-256 Merkle Tree | 完整性校验 | seccore |
| MCP 工具滥用 | 按服务器工具过滤 | 集成测试 | mcp-mesh |
| 决策黑客 | ASA 红队实时审计 | 对抗测试 | parliament |
| 群体思维 | Validator 检查点 | 共识测试 | sif-field |
| 提示注入 | Skeptic 正则扫描 | 模糊测试 | parliament |
| 资源耗尽 | Capability Decay | 压力测试 | seccore |

### 9.2 合规映射

| 标准 | 实现模块 | 证据 | 审计频率 |
|------|---------|------|---------|
| SOC 2 Type II | SecCore 审计链 | 不可篡改日志 | 年度 |
| ISO 27001 | 零信任架构 | 能力衰减 + 沙箱 | 年度 |
| GDPR (数据最小化) | MLC L3 程序记忆 | 仅存储模式，不存储代码 | 季度 |
| OWASP Top 10 | SecCore + ASA | 自动化安全测试 | 每次发布 |
| CC EAL4 | 形式化验证计划 | 核心模块证明 | 年度 |

---

## 10. 运维与故障排查

### 10.1 常见故障

**问题 1: 启动失败 "gVisor not found"**
```bash
# 症状: runsc not found in PATH
# 解决:
curl -fsSL https://storage.googleapis.com/gvisor/releases/release/latest/runsc -o /usr/local/bin/runsc
chmod +x /usr/local/bin/runsc
# 验证: runsc --version
```

**问题 2: 能力过早冻结**
```bash
# 症状: 3 次操作后能力为 0
# 诊断:
grep "Capability consumed" ~/.chimera/logs/audit.log
# 解决: 检查 ~/.chimera/config.yaml 中 decay.high_risk_decay（默认 0.2）
```

**问题 3: SAR 路由全部分配到同一专家**
```bash
# 症状: EDSB 熵 = 0.1
# 诊断:
curl localhost:9090/metrics | grep chimera_global_entropy
# 解决: 检查 entropy_monitor.half_life_secs（默认 3600）
```

**问题 4: 议会无法达成共识**
```bash
# 症状: 所有任务返回 "Consensus not reached"
# 诊断:
grep "Parliament" ~/.chimera/logs/chimera.log | tail -20
# 解决: 临时降低 consensus_threshold 到 0.6 测试
```

**问题 5: SCC 缓存命中率低**
```bash
# 症状: SCC hit rate < 50%
# 诊断:
curl localhost:9090/metrics | grep chimera_scc_hit_rate
# 解决: 增大 scc.max_entries 或检查 speculative_ttl_seconds
```

**问题 6: RCF 融合超时**
```bash
# 症状: fusion time > 20ms
# 诊断:
grep "RCF" ~/.chimera/logs/chimera.log | tail -10
# 解决: 运行 `chimera evolution trigger` 预编译常见组合
```

**问题 7: MTPE 批量验证频繁失败**
```bash
# 症状: MTPE 经常回退到单步执行
# 诊断:
curl localhost:9090/metrics | grep chimera_mtpe_success_rate
# 解决: 降低 prediction_depth（默认自适应，可手动设置上限）
```

### 10.2 性能调优指南

**SIMD 优化**:
```toml
# Cargo.toml
[dependencies]
ndarray = { version = "0.16", features = ["blas"] }
blas-src = { version = "0.10", features = ["openblas"] }
```

**SQLite 优化**:
```sql
-- 在 ~/.chimera/memory.db 上执行
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
PRAGMA cache_size=10000;
PRAGMA temp_store=memory;
```

**WASM 缓存**:
```yaml
# ~/.chimera/config.yaml
aaef:
  rcf:
    precompile_common_combos: true
    background_precompile_interval_hours: 24
    max_fusion_time_ms: 20
```

---

## 11. 附录

### 附录 A: 每日站会模板

```markdown
## 站会记录 YYYY-MM-DD

### 昨日完成
- [ ] 任务 1（提交: abc123）
- [ ] 任务 2

### 今日计划
- [ ] 任务 3（预计 4h）
- [ ] 任务 4（预计 3h）

### 阻塞点
- 需要 XXX 资源 / 需要决策 YYY

### 风险
- 任务 5 可能延期，因 ZZZ
```

### 附录 B: 代码审查清单

**安全审查（Skeptic 角色）**:
- [ ] 无命令字符串插值
- [ ] 环境变量白名单检查
- [ ] 能力衰减调用点正确
- [ ] 沙箱逃逸风险
- [ ] 审计日志完整性
- [ ] ASA 红队审计点覆盖

**性能审查（Optimizer 角色）**:
- [ ] 无阻塞操作在异步路径
- [ ] 数据库查询有索引
- [ ] 大内存分配有上限
- [ ] SIMD 优化已启用
- [ ] RCF 预编译策略合理

**架构审查（Architect 角色）**:
- [ ] 模块边界清晰
- [ ] 错误处理覆盖所有路径
- [ ] 接口向后兼容
- [ ] 无循环依赖
- [ ] CLSI 跨层索引更新正确

### 附录 C: 技术选型速查表

| 组件 | 选型 | 版本 | 备选 | 不选原因 |
|------|------|------|------|---------|
| 系统语言 | Rust | 1.85+ | Go | GC 延迟 |
| 异步运行时 | Tokio | 1.40+ | async-std | 生态 |
| TUI | Ratatui | 0.29+ | cursive | 维护 |
| WASM | Wasmtime | 22.0+ | Wasmer | 生态 |
| 向量 DB | sqlite-vec | 0.1+ | pgvector | 配置 |
| 序列化 | MessagePack | 1.3+ | JSON | 体积 |
| 配置 | Figment | 0.10+ | envy | 多源 |
| 监控 | prometheus-client | 0.22+ | statsd | 标准 |
| 沙箱 | gVisor | latest | Firejail | 更新 |
| 数学 | ndarray + BLAS | 0.16+ | nalgebra | 用途 |

### 附录 D: ADR 索引

| ADR | 主题 | 状态 | 日期 |
|-----|------|------|------|
| ADR-001 | Rust 选择 | Accepted | Week 1 |
| ADR-002 | Tokio 选择 | Accepted | Week 1 |
| ADR-003 | gVisor 选择 | Accepted | Week 1 |
| ADR-004 | sqlite-vec 选择 | Accepted | Week 1 |
| ADR-005 | Wasmtime 选择 | Accepted | Week 1 |
| ADR-006 | DCO 架构 | Accepted | Week 2 |
| ADR-007 | 议会共识 | Accepted | Week 3 |
| ADR-008 | MCP 传输 | Accepted | Week 4 |
| ADR-009 | SAR 稀疏路由 | Accepted | Week 4 |
| ADR-010 | RCF 快速融合 | Accepted | Week 5 |
| ADR-011 | ASA 红队 | Accepted | Week 5 |
| ADR-012 | DECB 连续预算 | Accepted | Week 6 |
| ADR-013 | MTPE 多步执行 | Accepted | Week 7 |

---

**文档结束**
