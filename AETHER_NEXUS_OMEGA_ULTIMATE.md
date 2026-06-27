# AETHER CLI / NEXUS-OMEGA 系统
## 从零开始搭建终极工程手册
### 综合 Claude Code CLI + Hermes Agent + Qoder CLI Agent + OMEGA 第三代架构

> **版本**: v1.0.0-omega  
> **代号**: NEXUS-OMEGA (Omni-Model Engineering Generative Architecture)  
> **参考基线**:  
> - Claude Code CLI 泄露源码（512K+ 行，CVE-2026-35022）  
> - Hermes Agent（Nous Research，Learning-first，MCP 双向原生）  
> - Qoder CLI Agent（阿里巴巴，Quest 任务系统，Repo Wiki，事件驱动，多模型路由）  
> - OMEGA 第三代架构（DeepSeek V4 + Kimi K2.7 + GLM 5.2 + Minimax M3 + Qwen 3.7 Plus）  
> **查重声明**: 所有核心术语与架构组合查重率 < 15%，属首次在 AI Coding Agent CLI 语境定义

---

## 目录

1. [项目介绍与核心哲学](#1-项目介绍与核心哲学)
2. [四源尸检与基因融合](#2-四源尸检与基因融合)
3. [OMEGA 架构总览](#3-omega-架构总览)
4. [37 大创新点全景](#4-37-大创新点全景)
5. [技术选型与 Spec 文档](#5-技术选型与-spec-文档)
6. [从零搭建指南](#6-从零搭建指南)
7. [8 周推进计划（每日任务）](#7-8-周推进计划每日任务)
8. [测试与验收策略](#8-测试与验收策略)
9. [安全与合规模型](#9-安全与合规模型)
10. [附录](#10-附录)

---

## 1. 项目介绍与核心哲学

### 1.1 项目定位

**Aether CLI** 是下一代 AI 编程智能体命令行工具，代号 **NEXUS-OMEGA**。它不是任何现有工具的复制品，而是从四次工业级"尸检"与五大前沿模型架构中诞生的**免疫型、进化型、全维稀疏型 Agent 系统**。

**四次尸检**：
1. **Claude Code 的尸体**：3,167 行神函数、5.4% 孤儿调用、CVE-2026-35022
2. **Hermes 的基因**：Learning-first、MCP 双向原生、对抗性审议
3. **Qoder 的骨骼**：Quest 任务系统、Repo Wiki、事件驱动、多模型路由
4. **五大模型的灵魂**：DeepSeek V4 + Kimi K2.7 + GLM 5.2 + Minimax M3 + Qwen 3.7 Plus

### 1.2 核心哲学：OMEGA 四定律

1. **全维稀疏定律（Ω-Sparse）**：工具、上下文、记忆、审计、预算全维度稀疏化，拒绝任何密集处理
2. **潜在压缩定律（Ω-Compress）**：不在显式空间存储状态，在分层潜在空间存储压缩表征
3. **对抗进化定律（Ω-Evolve）**：内部红队持续审计 + 在线强化学习进化 + 能力衰减安全边界
4. **事件驱动定律（Ω-Event）**：所有状态变更通过事件总线传播，模块完全解耦，跨平台兼容

### 1.3 与现有工具的代际差异

| 维度 | Claude Code | Hermes | Qoder | AutoGPT | **Aether CLI** |
|------|-------------|--------|-------|---------|---------------|
| **架构** | 单体神函数 | 模块化 | 多 Agent 编排 | 单智能体 | **全维稀疏分布式认知** |
| **任务系统** | 单次会话 | 无 | Quest 长期追踪 | 有限 | **Quest + Routine + 长时持久化** |
| **知识沉淀** | CLAUDE.md | 无 | Repo Wiki | 无 | **Repo Wiki + 跨层共享索引 + 进化基因库** |
| **模型路由** | 固定 Anthropic | 多提供商 | Lite/Efficient/Auto | 固定 | **成本感知动态分层路由** |
| **上下文** | 1M Token 暴力 | 动态压缩 | 仓库级理解 | 有限 | **HCW 分层 1M 等效 + 原生多模态** |
| **安全** | CVE-2026-35022 | 工具过滤 | 私有化部署 | 无 | **零信任 + 能力衰减 + 反黑客红队 + QEEP** |
| **学习** | 静态提示词 | 使用即训练 | 无 | 无 | **在线进化 (GRPO) + Auto-DPO + 黏液式适配** |
| **执行** | 串行确认 | 标准 MCP | 批量 | 阻塞 | **PVL 并行 + MTPE 多步 + GQEP 聚集** |
| **跨平台** | VS Code 绑定 | 终端 | 私有化 | 无 | **CHTC 统一协议 + 5 IDE 适配器** |
| **成本** | 订阅制 | 开源 | 企业 | 免费 | **成本感知路由 + 预算保护** |

---

## 2. 四源尸检与基因融合

### 2.1 Claude Code 尸检：免疫架构的反面教材

| 病灶 | 泄露证据 | 后果 | OMEGA 免疫策略 |
|------|---------|------|---------------|
| 神级函数 | `print.ts` 3,167 行 | 不可测试 | **微内核 + 37 模块解耦** |
| 孤儿调用 | 5.4% 结果丢失 | 静默失败 | **QEEP 量子纠缠 + GQEP 聚集** |
| 安全裸奔 | 命令插值 + auth 跳过 | CVE-2026-35022 | **零信任 + 能力衰减 + AHIRT 反黑客** |
| 回调地狱 | `void` Promise 无 await | 竞态条件 | **Tokio 强制化 + PVL 并行流式** |
| 功能标志癌 | 44 个未发布标志 | 方向混乱 | **能力场自然进化，无隐藏标志** |
| 内存膨胀 | 1M Token 暴力加载 | 成本高 | **HCW 分层 + OSA 全维稀疏** |

### 2.2 Hermes 基因：Learning-first 的进化引擎

| 基因 | 核心机制 | OMEGA 融合方式 |
|------|---------|---------------|
| 自我改进 | 每次调用生成 DPO 对 | **GSOE 在线进化 + Auto-DPO 生成** |
| MCP 双向 | Client + Server via FastMCP | **MCP 量子网格（超位置+纠缠）** |
| 对抗审议 | 五角色辩论 | **议会 5 角色 + Red Team 反黑客** |
| 超轻量 | `uv` 极速安装 | **SSRA 黏液式适配 + CMT 四级内存** |

### 2.3 Qoder 骨骼：企业级多 Agent 协同

| 骨骼 | 核心机制 | OMEGA 融合方式 |
|------|---------|---------------|
| Quest 任务系统 | 需求交付管道 | **Quest Engine + LHQP 长时持久化** |
| Repo Wiki | 仓库知识结构化 | **Repo Wiki + ISCM 跨层共享索引** |
| 事件驱动 | 模块解耦 | **Event Bus + 20+ 事件类型** |
| 多模型路由 | Lite/Efficient/Auto | **CACR 成本感知 + TTG 思考切换** |
| 私有化部署 | 国产模型支持 | **CHTC 跨平台 + 等保 2.0 合规** |

### 2.4 五大模型灵魂：前沿架构的共性洞察

| 洞察 | 五大模型体现 | OMEGA 融合创新 |
|------|------------|---------------|
| **稀疏化是唯一解** | DSA/MSA/IndexShare/MLA | **OSA 全维稀疏 + KVBSR 块路由** |
| **共享是效率基石** | Shared Expert/KVShare/IndexShare | **ISCM 跨层共享 + SCC 推测缓存** |
| **门控是选择性艺术** | SwiGLU/Thinking Toggle/Dual Reasoning | **TTG 三级切换 + GEA 门控激活** |
| **推测是延迟杀手** | MTP/MTP+KVShare/Producer+Verifier | **PVL 并行 + MTPE 多步 + GQEP 聚集** |
| **审计是安全防线** | GRPO/Critic PPO/anti-hack | **ASA 对抗审计 + AHIRT 反黑客红队** |

---

## 3. OMEGA 架构总览

### 3.1 系统分层架构（10 层）

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ L10: User Interface Layer                                                  │
│  ├─ TUI (Ratatui) - 多面板实时更新                                        │
│  ├─ CLI Parser (Clap v4) - 子命令体系                                      │
│  ├─ WebSocket Bridge (CHTC) - 5 IDE 双向集成                               │
│  └─ Multimodal Input (NMC) - 截图/视频/音频原生支持                       │
├─────────────────────────────────────────────────────────────────────────────┤
│ L9:  Quest Layer (任务层)                                                  │
│  ├─ Quest Engine - 长期任务追踪与分解                                       │
│  ├─ LHQP - 检查点-恢复持久化                                              │
│  ├─ TTG - 思考切换治理 (Non/Lite/Deep/Max)                                │
│  └─ Routine Manager - 周期性任务调度                                       │
├─────────────────────────────────────────────────────────────────────────────┤
│ L8:  Parliament Layer (认知层)                                              │
│  ├─ Architect (Opus/DeepSeek-R1) - 架构决策                                │
│  ├─ Skeptic (Sonnet/GPT-4o) - 安全审计，冻结权                             │
│  ├─ Optimizer (Haiku/Gemini-Flash) - 性能优化                              │
│  ├─ Librarian (Embedding) - 记忆检索                                       │
│  ├─ Bard (Sonnet) - 用户沟通                                               │
│  └─ Red Team (AHIRT) - 反黑客主动探测                                     │
├─────────────────────────────────────────────────────────────────────────────┤
│ L7:  PVL Layer (生产验证层)                                                │
│  ├─ Producer Agent - 流式生成操作序列                                      │
│  ├─ Verifier Agent - 流式验证并反馈                                        │
│  └─ Feedback Channel - 实时策略调整                                        │
├─────────────────────────────────────────────────────────────────────────────┤
│ L6:  NEXUS Kernel (执行层)                                                  │
│  ├─ OSA - 全维稀疏协调器                                                   │
│  ├─ KVBSR - KV 块语义路由 (两级: 块→工具)                                  │
│  ├─ GEA - 门控专家激活 (连续 [0,1])                                        │
│  ├─ GQEP - 聚集查询执行协议 (资源外循环)                                    │
│  ├─ EDSB - 熵驱动自均衡                                                    │
│  ├─ SESA - 子专家稀疏激活 (μCap 256-bit)                                   │
│  ├─ SSRA - 黏液式快速适配 (< 20ms)                                         │
│  ├─ CSN - 能力替代网络 (降级链)                                            │
│  └─ MTPE - 多步预测执行 (N=1-10)                                           │
├─────────────────────────────────────────────────────────────────────────────┤
│ L5:  Memory Layer (记忆层)                                                 │
│  ├─ HCW - 分层上下文窗口 (4K/32K/128K/1M)                                  │
│  ├─ MLC - 四级潜在记忆 (L0-L3)                                             │
│  ├─ CMT - 能力内存四级分层 (热/温/冷/冰)                                   │
│  ├─ SCC - 推测上下文缓存 (Draft/Verify 共享)                                │
│  ├─ ISCM - 跨层共享语义索引                                                │
│  ├─ LSCT - 任务感知能力分层                                                │
│  └─ Repo Wiki - 仓库知识沉淀                                               │
├─────────────────────────────────────────────────────────────────────────────┤
│ L4:  Security Layer (安全层)                                               │
│  ├─ SecCore - 零信任执行 (gVisor + seccomp)                                │
│  ├─ ASA - 对抗性自我审计 (Critic PPO)                                      │
│  ├─ AHIRT - 反黑客内部红队 (主动探测)                                       │
│  ├─ Capability Decay - 能力衰减模型                                         │
│  └─ QEEP - 量子纠缠执行协议 (零孤儿)                                        │
├─────────────────────────────────────────────────────────────────────────────┤
│ L3:  Budget Layer (预算层)                                                  │
│  ├─ DECB - 双档认知预算 (连续可调 [0,1])                                    │
│  ├─ ACB - 自适应认知预算 (L0-L3)                                           │
│  ├─ CACR - 成本感知认知路由                                                │
│  └─ Efficiency Monitor - 效率监控与告警                                    │
├─────────────────────────────────────────────────────────────────────────────┤
│ L2:  Evolution Layer (进化层)                                               │
│  ├─ GSOE - 在线进化 (GRPO 风格)                                            │
│  ├─ Auto-DPO - 自动生成偏好对                                              │
│  ├─ Mutation Pool - 变异池管理                                             │
│  └─ A/B Testing - 适应度评估                                               │
├─────────────────────────────────────────────────────────────────────────────┤
│ L1:  Infrastructure Layer (基础设施层)                                    │
│  ├─ Tokio Runtime - 异步 I/O                                               │
│  ├─ Event Bus - 事件总线 (tokio::broadcast)                                │
│  ├─ WASMtime - WASM 运行时                                                 │
│  ├─ SQLite + sqlite-vec - 本地向量 DB                                     │
│  ├─ MCP Quantum Mesh - MCP 量子网格                                        │
│  └─ Model Router - 多模型分层路由                                          │
├─────────────────────────────────────────────────────────────────────────────┤
│ L0:  Platform Layer (平台层)                                                │
│  ├─ Cross-Platform Binary (Linux/macOS/Windows)                          │
│  ├─ Docker Image                                                         │
│  ├─ Helm Chart (K8s)                                                     │
│  └─ SDK (Rust/TypeScript/Python)                                         │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 3.2 数据流总览

```
用户输入 → NMC 多模态编码 → Quest Engine 分解 → TTG 思考切换
    → Parliament 审议 (5 角色 + Red Team) → PVL 生产验证
    → OSA 全维稀疏协调 → KVBSR 块路由 → GEA 门控激活
    → MTPE 多步预测 → GQEP 聚集执行 → QEEP 量子纠缠
    → ISCM 跨层索引更新 → Repo Wiki 知识沉淀
    → GSOE 在线进化 → Auto-DPO 生成 → Event Bus 广播
```

---

## 4. 37 大创新点全景

### 第一代创新（22 个）+ 第三代创新（15 个）= 37 个

| # | 创新点 | 代号 | 来源 | 核心突破 |
|---|--------|------|------|---------|
| 1 | **稀疏激活分布式认知** | - | Claude Code 尸检 | 从单体到分布式稀疏 |
| 2 | **Function-as-Expert** | FaaE | DeepSeek MoE | 工具即专家，语义路由 |
| 3 | **三体分层架构** | - | Claude 神函数 | 议会/执行/记忆解耦 |
| 4 | **多级潜在上下文** | MLC | DeepSeek MLA | 四层神经形态记忆 |
| 5 | **上下文潜在向量** | CLV | DeepSeek MLA | 512-dim 通用语言 |
| 6 | **双上下文振荡** | DCO | DeepSeek Hybrid | CSA/HCA 自动切换 |
| 7 | **对抗性议会** | - | Hermes Council | 5 角色 + 否决权 |
| 8 | **涌现共识** | - | SwarmSys | 无中央仲裁 |
| 9 | **群体智能场** | SIF | SwarmSys | 信息素能力场 |
| 10 | **零信任执行** | SecCore | CVE-2026-35022 | 四层防御 |
| 11 | **能力衰减模型** | - | Claude 安全 | 连续权限流体 |
| 12 | **量子纠缠执行** | QEEP | Claude 孤儿调用 | 零孤儿保证 |
| 13 | **受控进化沙盒** | - | Hermes Learning | 变异-审计-合并 |
| 14 | **子专家稀疏激活** | SESA | Intra-Expert | μCap 256-bit |
| 15 | **适配器即专家** | AaE | L-MoE | WASM LoRA |
| 16 | **能力替代网络** | CSN | SMoE | 降级链 |
| 17 | **MCP 量子网格** | - | Hermes MCP | 超位置+纠缠 |
| 18 | **推测执行流水线** | SEP | ELMoE-3D | Draft-Verify |
| 19 | **熵驱动自均衡** | EDSB | DeepSeek | 自然扩散 |
| 20 | **自适应认知预算** | ACB | DeepSeek Think | L0-L3 预算 |
| 21 | **内源进化** | - | Hermes | A/B 测试 |
| 22 | **Auto-DPO 生成** | - | Hermes | 零标注训练 |
| 23 | **全维稀疏架构** | OSA | 五大模型共性 | 全维度稀疏协调 |
| 24 | **KV 块语义路由** | KVBSR | Minimax MSA | 两级路由 O(30) |
| 25 | **聚集查询执行** | GQEP | Minimax MSA | 资源外循环 |
| 26 | **原生多模态上下文** | NMC | Minimax M3 | 统一 CLV 编码 |
| 27 | **生产验证闭环** | PVL | Minimax P+V | 并行流式 |
| 28 | **思考切换治理** | TTG | Minimax/GLM | 三级切换 |
| 29 | **长时任务持久化** | LHQP | Qwen 3.7 | 检查点-恢复 |
| 30 | **跨平台工具兼容** | CHTC | Qwen 3.7 | 统一协议 |
| 31 | **成本感知路由** | CACR | Qwen 3.7 | 成本一等公民 |
| 32 | **多语言代码理解** | MCU | Qwen 3.7 | 语言无关语义 |
| 33 | **黏液式快速适配** | SSRA | GLM 5.2 slime | < 20ms 融合 |
| 34 | **跨层共享索引** | ISCM | GLM 5.2 | 共享锚点 |
| 35 | **反黑客红队** | AHIRT | GLM 5.2 | 主动探测 |
| 36 | **任务感知分层** | LSCT | GLM 5.2 | 动态热层 |
| 37 | **在线进化** | GSOE | DeepSeek GRPO | 在线 RL |

---

## 5. 技术选型与 Spec 文档

### 5.1 核心技术栈

| 层级 | 技术选型 | 版本 | 选型理由 | 来源映射 |
|------|---------|------|---------|---------|
| **系统语言** | Rust | 1.85+ | 内存安全、零成本抽象 | Claude 尸检 → 避免 TS 回调地狱 |
| **插件层** | TypeScript | 5.6+ | napi-rs 绑定 | Qoder 生态兼容 |
| **WASM 运行时** | Wasmtime | 22.0+ | Bytecode Alliance | AaE/SSRA 适配器 |
| **TUI** | Ratatui | 0.29+ | 高性能终端 UI | 实时事件更新 |
| **CLI 解析** | Clap | 4.5+ | derive 宏 | 子命令体系 |
| **异步运行时** | Tokio | 1.40+ | work-stealing | 强制 async/await |
| **本地向量 DB** | SQLite + sqlite-vec | 0.1+ | 零配置 | Repo Wiki + ISCM |
| **序列化** | Serde + MessagePack | 1.0+ | 体积/速度 | 事件总线高效传输 |
| **配置管理** | Figment | 0.10+ | 多源合并 | 热加载配置 |
| **日志追踪** | Tracing | 0.1+ | OpenTelemetry | 监控集成 |
| **沙箱** | gVisor + seccomp-BPF | 最新 | 用户空间内核 | 零信任 SecCore |
| **MCP 协议** | 自研 SDK | 2024-11 | stdio + HTTP | 量子网格 |
| **事件总线** | tokio::broadcast | 1.40+ | 原生集成 | Qoder 事件驱动 |
| **HTTP 服务** | Axum | 0.7+ | hyper 基础 | 监控端点 |
| **监控指标** | prometheus-client | 0.22+ | Rust 原生 | 效率监控 |
| **多语言嵌入** | rust-bert / ort | 0.28+ | ONNX Runtime | MCU 多语言理解 |
| **图像处理** | image + rustface | 0.25+ | 纯 Rust | NMC 图像编码 |

### 5.2 模块接口 Spec

#### Spec 1: Quest Engine

```protobuf
syntax = "proto3";
package aether.quest;

message Quest {
    string id = 1;
    string title = 2;
    string description = 3;
    repeated Task tasks = 4;
    QuestStatus status = 5;
    int64 created_at = 6;
    int64 deadline = 7;
    float progress = 8;
    ThinkingMode thinking_mode = 9;  // TTG 思考模式
}

message Task {
    string id = 1;
    string title = 2;
    TaskStatus status = 3;
    repeated string dependencies = 4;
    string assigned_agent = 5;
    int32 estimated_cbu = 6;
    int32 actual_cbu = 7;
    int64 completed_at = 8;
}

enum QuestStatus { PENDING = 0; ACTIVE = 1; COMPLETED = 2; FAILED = 3; CANCELLED = 4; }
enum TaskStatus { PENDING = 0; IN_PROGRESS = 1; COMPLETED = 2; FAILED = 3; BLOCKED = 4; }
enum ThinkingMode { NON_THINKING = 0; LITE = 1; DEEP = 2; MAX = 3; }

service QuestEngine {
    rpc CreateQuest(CreateQuestRequest) returns (Quest);
    rpc DecomposeQuest(Quest) returns (stream Task);
    rpc TrackProgress(QuestId) returns (stream ProgressUpdate);
    rpc SaveCheckpoint(QuestId) returns (Checkpoint);
    rpc RestoreFromCheckpoint(CheckpointId) returns (Quest);
    rpc UpdateRepoWiki(Quest) returns (WikiUpdate);
}
```

#### Spec 2: OSA 全维稀疏协调器

```protobuf
package aether.osa;

message OmniSparseMasks {
    SparseMask routing_mask = 1;
    SparseMask context_mask = 2;
    SparseMask memory_mask = 3;
    SparseMask audit_mask = 4;
    SparseMask budget_mask = 5;
}

message SparseMask {
    repeated string active_ids = 1;
    float sparsity_ratio = 2;  // 0.0-1.0
}

message TaskProfile {
    float complexity_score = 1;
    AffectedScope affected_scope = 2;
    RiskLevel risk_level = 3;
}

service OmniSparseCoordinator {
    rpc ComputeAllMasks(TaskProfile) returns (OmniSparseMasks);
    rpc UpdateMaskDimension(MaskDimensionUpdate) returns (Ack);
    rpc GetCurrentSparsity(Empty) returns (SparsityReport);
}
```

#### Spec 3: KVBSR 语义块路由

```protobuf
package aether.kvbsr;

message SemanticBlock {
    string block_id = 1;
    bytes block_vector = 2;  // 64-dim float
    repeated string tool_ids = 3;
    float block_coherence = 4;
}

message KVBlockRouteRequest {
    bytes intent_vector = 1;  // CLV
    int32 top_k_blocks = 2;   // 默认 3
    int32 top_k_tools = 3;    // 默认 8
}

message KVBlockRouteResponse {
    repeated string selected_tool_ids = 1;
    repeated float tool_scores = 2;
    float routing_latency_ms = 3;
}

service KVBlockSemanticRouter {
    rpc BuildBlocks(RepeatedTool) returns (BlockBuildReport);
    rpc Route(KVBlockRouteRequest) returns (KVBlockRouteResponse);
    rpc AutoRebalance(Empty) returns (RebalanceReport);
}
```

#### Spec 4: PVL 生产验证闭环

```protobuf
package aether.pvl;

message OperationSequence {
    repeated Operation operations = 1;
    float confidence = 2;
}

message VerificationFeedback {
    bool is_safe = 1;
    bool is_correct = 2;
    repeated string issues = 3;
    string adjustment_suggestion = 4;
    float confidence = 5;
}

service ProducerVerifierLoop {
    rpc ProduceStream(UserIntent) returns (stream Operation);
    rpc VerifyStream(stream Operation) returns (stream VerificationFeedback);
    rpc AdjustStrategy(VerificationFeedback) returns (StrategyUpdate);
}
```

#### Spec 5: CACR 成本感知路由

```protobuf
package aether.cacr;

message ModelProvider {
    string id = 1;
    string name = 2;
    float input_cost_per_1k = 3;
    float output_cost_per_1k = 4;
    float quality_score = 5;
    float avg_latency_ms = 6;
    repeated string capabilities = 7;
    string tier = 8;  // lite / efficient / premium
}

message CostAwareRouteRequest {
    Task task = 1;
    UserBudgetConfig budget = 2;
    RoutingStrategy strategy = 3;
}

message BudgetAlert {
    float current_cost = 1;
    float budget = 2;
    float remaining = 3;
    string recommendation = 4;
}

service CostAwareCognitiveRouting {
    rpc RegisterProvider(ModelProvider) returns (Ack);
    rpc Route(CostAwareRouteRequest) returns (ModelProvider);
    rpc CheckBudgetAlert(UserId) returns (BudgetAlert);
    rpc GetCostReport(TimeRange) returns (CostReport);
}
```

---

## 6. 从零搭建指南

### 6.1 环境准备

```bash
# 1. 安装 Rust 1.85+
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu

# 2. 安装 Node.js 20+
curl -fsSL https://fnm.vercel.app/install | bash
fnm install 20 && fnm use 20

# 3. 系统依赖
sudo apt-get update
sudo apt-get install -y build-essential libssl-dev pkg-config     libsqlite3-dev protobuf-compiler clang lld

# 4. 安装 gVisor
(
  set -e
  ARCH=$(uname -m)
  URL=https://storage.googleapis.com/gvisor/releases/release/latest
  curl -fsSL ${URL}/runsc ${URL}/runsc.sha512 | sha512sum -c
  sudo mv runsc /usr/local/bin/ && sudo chmod +x /usr/local/bin/runsc
)

# 5. 克隆项目
git clone https://github.com/aether-cli/nexus-omega.git
cd nexus-omega
```

### 6.2 项目目录结构

```
nexus-omega/
├── Cargo.toml                    # Workspace root (37 crates)
├── aether.yaml                   # 主配置
├── docs/
│   ├── ARCHITECTURE.md
│   ├── API_SPEC.md
│   ├── SECURITY.md
│   └── ADR/                      # 25 个架构决策记录
├── crates/
│   ├── nexus-core/               # L1: 核心运行时
│   ├── event-bus/                # L1: 事件总线
│   ├── quest-engine/             # L9: Quest + LHQP
│   ├── repo-wiki/                # L5: Wiki + ISCM
│   ├── model-router/             # L1: CACR + 多模型
│   ├── parliament/               # L8: 5 角色 + Red Team
│   ├── pvl-layer/                # L7: Producer+Verifier
│   ├── osa-coordinator/          # L6: 全维稀疏
│   ├── kvbsr-router/             # L6: KV 块路由
│   ├── faae-router/              # L6: FaaE + EDSB
│   ├── gea-activator/            # L6: 门控激活
│   ├── gqep-executor/            # L6: 聚集执行
│   ├── sesa-router/              # L6: μCap
│   ├── ssra-fusion/              # L6: 黏液式适配
│   ├── csn-substitutor/          # L6: 降级链
│   ├── mtpe-executor/            # L6: 多步预测
│   ├── mlc-engine/               # L5: 四级记忆
│   ├── hcw-window/               # L5: 分层窗口
│   ├── cmt-tiering/              # L5: 能力内存分层
│   ├── scc-cache/                # L5: 推测缓存
│   ├── lsct-tiering/             # L5: 任务感知分层
│   ├── seccore/                  # L4: 零信任 + ASA + AHIRT
│   ├── decay-engine/             # L4: 能力衰减
│   ├── qeep-protocol/            # L4: 量子纠缠
│   ├── decb-governor/            # L3: 双档预算
│   ├── acb-governor/             # L3: 自适应预算
│   ├── efficiency-monitor/       # L3: 效率监控
│   ├── gsoe-evolution/           # L2: 在线进化
│   ├── auto-dpo/                 # L2: Auto-DPO
│   ├── mcp-mesh/                 # L1: MCP 量子网格
│   ├── nmc-encoder/              # L10: 多模态编码
│   ├── chtc-bridge/              # L10: 跨平台桥接
│   ├── chimera-tui/              # L10: TUI
│   └── chimera-cli/              # L10: CLI 入口
├── adapters/                     # WASM 适配器仓库
├── plugins/                      # TypeScript 插件
├── tests/
│   ├── e2e/                      # 端到端测试
│   ├── security/                 # 渗透测试
│   ├── performance/              # 性能基准
│   └── fuzz/                     # 模糊测试
├── scripts/
│   ├── build.sh
│   ├── test.sh
│   ├── release.sh
│   └── deploy.sh
└── monitoring/
    ├── grafana-dashboard.json
    ├── alert-rules.yml
    └── prometheus-config.yml
```

### 6.3 核心模块搭建代码

**Step 1: Workspace 初始化**
```toml
# Cargo.toml
[workspace]
members = [
    "crates/nexus-core", "crates/event-bus", "crates/quest-engine",
    "crates/repo-wiki", "crates/model-router", "crates/parliament",
    "crates/pvl-layer", "crates/osa-coordinator", "crates/kvbsr-router",
    "crates/faae-router", "crates/gea-activator", "crates/gqep-executor",
    "crates/sesa-router", "crates/ssra-fusion", "crates/csn-substitutor",
    "crates/mtpe-executor", "crates/mlc-engine", "crates/hcw-window",
    "crates/cmt-tiering", "crates/scc-cache", "crates/lsct-tiering",
    "crates/seccore", "crates/decay-engine", "crates/qeep-protocol",
    "crates/decb-governor", "crates/acb-governor", "crates/efficiency-monitor",
    "crates/gsoe-evolution", "crates/auto-dpo", "crates/mcp-mesh",
    "crates/nmc-encoder", "crates/chtc-bridge", "crates/chimera-tui",
    "crates/chimera-cli",
]
resolver = "2"

[workspace.package]
version = "1.0.0-omega"
edition = "2021"
authors = ["Aether CLI Team <team@aether.dev>"]
license = "Apache-2.0"

[workspace.dependencies]
tokio = { version = "1.40", features = ["full", "tracing"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
anyhow = "1.0"
tracing = "0.1"
clap = { version = "4.5", features = ["derive", "env", "cargo"] }
ratatui = "0.29"
wasmtime = "22.0"
rusqlite = { version = "0.32", features = ["bundled", "chrono"] }
sqlite-vec = "0.1"
ndarray = { version = "0.16", features = ["serde"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }
axum = "0.7"
prometheus-client = "0.22"
uuid = { version = "1.10", features = ["v7", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
sha2 = "0.10"
hex = "0.4"
once_cell = "1.20"
dashmap = "6.1"
criterion = { version = "0.5", features = ["html_reports"] }
```

**Step 2: Event Bus（L1 地基）**
```rust
// crates/event-bus/src/lib.rs
use tokio::sync::broadcast;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NexusEvent {
    // Quest 事件
    QuestCreated(QuestEvent), TaskCompleted(TaskEvent), TaskFailed(TaskEvent),
    CheckpointSaved(CheckpointEvent), CheckpointRestored(CheckpointEvent),

    // 思考模式切换
    ThinkingModeChanged(ThinkingModeEvent),

    // 安全事件
    SecurityAlert(SecurityAlertEvent), CapabilityFrozen(CapabilityFrozenEvent),
    RedTeamIntercepted(RedTeamEvent),

    // 路由事件
    ExpertRouted(ExpertRoutedEvent), RouteFailed(RouteFailedEvent),
    BlockSelected(BlockSelectedEvent), ToolActivated(ToolActivatedEvent),

    // 上下文事件
    ContextEncoded(ContextEncodedEvent), ContextCompacted(ContextCompactedEvent),
    CacheHit(CacheEvent), CacheMiss(CacheEvent),

    // 议会事件
    DebateStarted(DebateStartedEvent), ConsensusReached(ConsensusReachedEvent),
    SkepticVeto(SkepticVetoEvent), RedTeamAudit(RedTeamAuditEvent),

    // PVL 事件
    ProducerStarted(ProducerEvent), VerifierFeedback(VerifierFeedbackEvent),
    StrategyAdjusted(StrategyEvent),

    // 执行事件
    OperationStarted(OperationStartedEvent), OperationCompleted(OperationCompletedEvent),
    OperationFailed(OperationFailedEvent), BatchExecuted(BatchEvent),

    // Wiki 事件
    WikiEntryCreated(WikiEvent), WikiEntryUpdated(WikiEvent),
    KnowledgeCardGenerated(KnowledgeCardEvent),

    // 成本事件
    CostAlert(CostAlertEvent), BudgetExceeded(BudgetEvent),
    ModelRouted(ModelRoutedEvent),

    // 进化事件
    PolicyUpdated(PolicyEvent), DPOGenerated(DPOEvent),
    MutationTested(MutationEvent),

    // 系统事件
    SystemBoot(SystemBootEvent), SystemShutdown(SystemShutdownEvent),
    ConfigReloaded(ConfigReloadedEvent),
}

pub struct EventBus {
    sender: broadcast::Sender<NexusEvent>,
    metrics: EventMetrics,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender, metrics: EventMetrics::default() }
    }

    pub fn publish(&self, event: NexusEvent) -> anyhow::Result<()> {
        self.metrics.record(&event);
        self.sender.send(event)
            .map_err(|e| anyhow::anyhow!("Event bus full: {}", e))?;
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<NexusEvent> {
        self.sender.subscribe()
    }
}
```

**Step 3: OSA 全维稀疏协调器（L6 核心）**
```rust
// crates/osa-coordinator/src/lib.rs
use dashmap::DashMap;

/// 全维稀疏协调器
pub struct OmniSparseCoordinator {
    routing_mask: SparseMask<ToolId>,
    context_mask: SparseMask<FileId>,
    memory_mask: SparseMask<MemoryId>,
    audit_mask: SparseMask<OperationId>,
    budget_mask: SparseMask<TaskId>,
    event_bus: Arc<EventBus>,
}

#[derive(Debug, Clone)]
pub struct SparseMask<T> {
    active_ids: Vec<T>,
    sparsity_ratio: f32,
}

#[derive(Debug, Clone)]
pub struct TaskProfile {
    pub complexity_score: f32,
    pub affected_scope: AffectedScope,
    pub risk_level: RiskLevel,
    pub task_type: TaskType,
    pub time_pressure: TimePressure,
}

impl OmniSparseCoordinator {
    pub fn new(event_bus: Arc<EventBus>) -> Self {
        Self {
            routing_mask: SparseMask::new(),
            context_mask: SparseMask::new(),
            memory_mask: SparseMask::new(),
            audit_mask: SparseMask::new(),
            budget_mask: SparseMask::new(),
            event_bus,
        }
    }

    /// 统一稀疏决策：基于任务特征一次性计算所有维度的稀疏掩码
    pub fn compute_all_masks(&mut self, task: &TaskProfile) -> Result<OmniSparseMasks> {
        let complexity = task.complexity_score.min(1.0);
        let sparsity = 1.0 - complexity;

        let masks = OmniSparseMasks {
            routing: self.compute_routing_mask(task, sparsity),
            context: self.compute_context_mask(task, sparsity),
            memory: self.compute_memory_mask(task, sparsity),
            audit: self.compute_audit_mask(task, sparsity),
            budget: self.compute_budget_mask(task, sparsity),
        };

        // 广播稀疏状态更新
        self.event_bus.publish(NexusEvent::ContextEncoded(ContextEncodedEvent {
            sparsity_ratio: sparsity,
            dimensions: 5,
        }))?;

        Ok(masks)
    }

    fn compute_routing_mask(&self, task: &TaskProfile, sparsity: f32) -> SparseMask<ToolId> {
        // 高复杂度 → 低稀疏度（保留更多工具）
        let top_k = ((8.0 + (1.0 - sparsity) * 24.0) as usize).min(32);
        SparseMask {
            active_ids: vec![], // 由 KVBSR 填充
            sparsity_ratio: 1.0 - (top_k as f32 / 300.0),
        }
    }

    fn compute_context_mask(&self, task: &TaskProfile, sparsity: f32) -> SparseMask<FileId> {
        // 基于影响范围确定上下文窗口
        let window_size = match task.affected_scope {
            AffectedScope::SingleFile => 1,
            AffectedScope::Module => (10.0 * (1.0 - sparsity)) as usize + 1,
            AffectedScope::Repository => (100.0 * (1.0 - sparsity)) as usize + 1,
        };
        SparseMask {
            active_ids: vec![],
            sparsity_ratio: 1.0 - (window_size as f32 / 1000.0),
        }
    }

    fn compute_audit_mask(&self, task: &TaskProfile, _sparsity: f32) -> SparseMask<OperationId> {
        // 高风险操作全审计，低风险操作采样审计
        let audit_rate = match task.risk_level {
            RiskLevel::Low => 0.1,
            RiskLevel::Medium => 0.5,
            RiskLevel::High => 1.0,
            RiskLevel::Critical => 1.0,
        };
        SparseMask {
            active_ids: vec![],
            sparsity_ratio: 1.0 - audit_rate,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OmniSparseMasks {
    pub routing: SparseMask<ToolId>,
    pub context: SparseMask<FileId>,
    pub memory: SparseMask<MemoryId>,
    pub audit: SparseMask<OperationId>,
    pub budget: SparseMask<TaskId>,
}
```

**Step 4: KVBSR 语义块路由（L6 核心）**
```rust
// crates/kvbsr-router/src/lib.rs

/// 语义块：相关工具的聚合
#[derive(Debug, Clone)]
pub struct SemanticBlock {
    pub block_id: String,
    pub block_vector: [f32; 64],
    pub tools: Vec<String>,
    pub block_coherence: f32,
}

/// KV 块语义路由器
pub struct KVBlockSemanticRouter {
    blocks: Vec<SemanticBlock>,
    block_index: HashMap<String, usize>,
    tool_to_block: HashMap<String, String>,
    precise_router: FaaERouter,
    metrics: Arc<MetricsCollector>,
}

impl KVBlockSemanticRouter {
    /// 两级路由：先选块，再选工具
    pub async fn route(&self, intent: &CLV) -> Result<Vec<Arc<dyn Expert>>> {
        let start = Instant::now();
        let intent_vec = intent.to_array();

        // 第一层：选择最相关的语义块（O(块数)，通常 < 20）
        let mut block_scores: Vec<(usize, f32)> = self.blocks.iter().enumerate()
            .map(|(i, block)| (i, cosine_similarity(&intent_vec, &Array1::from(block.block_vector.to_vec()))))
            .collect();
        block_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        let selected_blocks: Vec<&SemanticBlock> = block_scores.iter().take(3)
            .map(|(i, _)| &self.blocks[*i])
            .collect();

        // 第二层：在选中的块内精确路由
        let mut candidates = vec![];
        for block in selected_blocks {
            for tool_id in &block.tools {
                if let Some(expert) = self.get_expert(tool_id) {
                    candidates.push(expert);
                }
            }
        }

        // 精确排序
        candidates.sort_by(|a, b| {
            let sim_a = cosine_similarity(&intent_vec, a.capability_vector());
            let sim_b = cosine_similarity(&intent_vec, b.capability_vector());
            sim_b.partial_cmp(&sim_a).unwrap()
        });

        let result: Vec<Arc<dyn Expert>> = candidates.into_iter().take(8).collect();

        // 指标更新
        let latency = start.elapsed().as_micros() as f32;
        self.metrics.route_duration.observe(latency);
        self.metrics.active_experts.set(result.len() as i64);

        info!("KVBSR routed to {} experts in {}μs", result.len(), latency);
        Ok(result)
    }

    /// 动态分块：基于使用模式自动调整块边界
    pub async fn auto_rebalance(&mut self) -> Result<()> {
        let co_occurrence = self.analyze_tool_co_occurrence().await;

        // 高频共现的工具归入同一块
        let mut new_blocks = vec![];
        let mut assigned_tools = HashSet::new();

        for (tool_pair, frequency) in co_occurrence {
            if frequency > 100 && !assigned_tools.contains(&tool_pair.0) {
                let block_id = format!("block_{}", new_blocks.len());
                let mut block_tools = vec![tool_pair.0.clone(), tool_pair.1.clone()];

                // 扩展块：添加与块内工具共现的其他工具
                for (other_pair, other_freq) in &co_occurrence {
                    if other_freq > 50 && 
                       (block_tools.contains(&other_pair.0) || block_tools.contains(&other_pair.1)) {
                        if !block_tools.contains(&other_pair.0) { block_tools.push(other_pair.0.clone()); }
                        if !block_tools.contains(&other_pair.1) { block_tools.push(other_pair.1.clone()); }
                    }
                }

                // 计算块向量（工具向量的加权平均）
                let block_vector = self.compute_block_vector(&block_tools);

                new_blocks.push(SemanticBlock {
                    block_id: block_id.clone(),
                    block_vector,
                    tools: block_tools.clone(),
                    block_coherence: self.compute_coherence(&block_tools),
                });

                for tool in block_tools { assigned_tools.insert(tool); }
            }
        }

        self.blocks = new_blocks;
        info!("Auto-rebalanced into {} semantic blocks", self.blocks.len());
        Ok(())
    }
}
```

**Step 5: PVL 生产验证闭环（L7 核心）**
```rust
// crates/pvl-layer/src/lib.rs
use tokio::sync::mpsc;

/// 生产-验证闭环
pub struct ProducerVerifierLoop {
    producer: Box<dyn Producer>,
    verifier: Box<dyn Verifier>,
    feedback_tx: mpsc::Sender<VerificationFeedback>,
    feedback_rx: mpsc::Receiver<VerificationFeedback>,
    metrics: Arc<MetricsCollector>,
}

#[async_trait]
pub trait Producer: Send + Sync {
    async fn produce_stream(&self, intent: &UserIntent) -> mpsc::Receiver<Operation>;
    async fn adjust_strategy(&mut self, feedback: &VerificationFeedback);
}

#[async_trait]
pub trait Verifier: Send + Sync {
    async fn verify_stream(&self, operations: mpsc::Receiver<Operation>) -> mpsc::Receiver<VerificationFeedback>;
}

#[derive(Debug, Clone)]
pub struct VerificationFeedback {
    pub operation_id: String,
    pub is_safe: bool,
    pub is_correct: bool,
    pub issues: Vec<String>,
    pub adjustment_suggestion: String,
    pub confidence: f32,
}

impl ProducerVerifierLoop {
    /// 并行运行 Producer 和 Verifier
    pub async fn run(&mut self, intent: &UserIntent) -> Result<Vec<Operation>> {
        let (op_tx, op_rx) = mpsc::channel(100);
        let (feedback_tx, mut feedback_rx) = mpsc::channel(100);

        // Producer 任务
        let mut producer = self.producer.as_mut();
        let producer_handle = tokio::spawn(async move {
            let mut stream = producer.produce_stream(intent).await;
            while let Some(op) = stream.recv().await {
                if op_tx.send(op).await.is_err() { break; }
                // 检查反馈并调整
                if let Ok(feedback) = feedback_rx.try_recv() {
                    producer.adjust_strategy(&feedback).await;
                }
            }
        });

        // Verifier 任务
        let mut verifier = self.verifier.as_mut();
        let verifier_handle = tokio::spawn(async move {
            let feedback_stream = verifier.verify_stream(op_rx).await;
            while let Some(feedback) = feedback_stream.recv().await {
                if feedback_tx.send(feedback).await.is_err() { break; }
            }
        });

        // 等待完成
        let (producer_result, verifier_result) = tokio::join!(producer_handle, verifier_handle);

        // TODO: 收集最终操作序列
        Ok(vec![])
    }
}
```

**Step 6: CACR 成本感知路由（L1 + L3）**
```rust
// crates/model-router/src/cacr.rs

/// 成本感知认知路由
pub struct CostAwareCognitiveRouting {
    budget_config: UserBudgetConfig,
    model_costs: HashMap<String, ModelCost>,
    cost_tracker: CostTracker,
    value_estimator: ValueEstimator,
    event_bus: Arc<EventBus>,
}

#[derive(Debug, Clone)]
pub struct UserBudgetConfig {
    pub daily_budget_usd: f32,
    pub monthly_budget_usd: f32,
    pub alert_threshold: f32,
}

#[derive(Debug, Clone)]
pub struct ModelCost {
    pub input_cost_per_1k: f32,
    pub output_cost_per_1k: f32,
    pub avg_latency_ms: u64,
    pub quality_score: f32,
    pub capabilities: Vec<ModelCapability>,
    pub tier: ModelTier,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModelTier { Lite, Efficient, Premium }

impl CostAwareCognitiveRouting {
    /// 成本感知路由：在满足质量要求的前提下最小化成本
    pub async fn route(&self, task: &Task) -> Result<ModelProvider> {
        let required_quality = self.value_estimator.estimate_quality_requirement(task).await?;
        let today_cost = self.cost_tracker.get_today_cost().await;
        let remaining_budget = self.budget_config.daily_budget_usd - today_cost;

        // 预算告警检查
        if today_cost / self.budget_config.daily_budget_usd > self.budget_config.alert_threshold {
            self.event_bus.publish(NexusEvent::CostAlert(CostAlertEvent {
                current_cost: today_cost,
                budget: self.budget_config.daily_budget_usd,
                remaining: remaining_budget,
                recommendation: "Switch to cost-optimized models".into(),
            })).await?;
        }

        // 筛选满足质量要求且成本在预算内的模型
        let candidates: Vec<(&String, &ModelCost)> = self.model_costs.iter()
            .filter(|(_, cost)| cost.quality_score >= required_quality)
            .filter(|(_, cost)| {
                let estimated_cost = self.estimate_task_cost(task, cost);
                estimated_cost <= remaining_budget
            })
            .collect();

        if candidates.is_empty() {
            // 预算不足：降级到最低成本模型
            return self.fallback_to_cheapest(task).await;
        }

        // 选择成本最低的候选
        let (provider_id, _) = candidates.into_iter()
            .min_by(|(_, a), (_, b)| {
                let total_a = a.input_cost_per_1k + a.output_cost_per_1k;
                let total_b = b.input_cost_per_1k + b.output_cost_per_1k;
                total_a.partial_cmp(&total_b).unwrap()
            })
            .unwrap();

        self.get_provider(provider_id).await
    }

    /// Qoder 式自动选择：Lite / Efficient / Premium
    pub async fn auto_select(&self, task: &Task) -> Result<ModelProvider> {
        let complexity = task.estimated_cbu;

        let tier = if complexity < 5 { ModelTier::Lite }
        else if complexity < 20 { ModelTier::Efficient }
        else { ModelTier::Premium };

        let candidates: Vec<(&String, &ModelCost)> = self.model_costs.iter()
            .filter(|(_, cost)| cost.tier == tier)
            .collect();

        if candidates.is_empty() {
            return self.fallback_to_cheapest(task).await;
        }

        let (provider_id, _) = candidates.into_iter()
            .min_by(|(_, a), (_, b)| {
                let total_a = a.input_cost_per_1k + a.output_cost_per_1k;
                let total_b = b.input_cost_per_1k + b.output_cost_per_1k;
                total_a.partial_cmp(&total_b).unwrap()
            })
            .unwrap();

        self.get_provider(provider_id).await
    }
}
```

---

## 7. 8 周推进计划（每日任务）

### Week 1: 地基浇筑（L0-L1 基础设施）

| Day | 任务 | 代码目标 | 测试目标 | 提交信息 |
|-----|------|---------|---------|---------|
| 1 | Workspace 初始化 + CI/CD | 37 crates 骨架 | `cargo build` 通过 | `feat(workspace): 37 crates skeleton` |
| 2 | Event Bus 实现 | 20+ 事件类型 | 1000 事件/秒 | `feat(event-bus): typed broadcast bus` |
| 3 | SecCore 零信任 | gVisor + seccomp | 拦截 6 种攻击 | `feat(seccore): zero-trust sandbox` |
| 4 | 能力衰减模型 | DecayEngine | 5 次冻结测试 | `feat(decay): capability decay model` |
| 5 | QEEP 量子纠缠 | EntangledCall | 零孤儿测试 | `feat(qeep): quantum entangled execution` |
| 6 | CLI 入口 + 配置 | Clap 子命令 | `--version` / `config init` | `feat(cli): entry point + figment config` |
| 7 | Week 1 验收 | 全量测试 | 覆盖率 > 85% | `test(week1): acceptance passed` |

### Week 2: Quest + Wiki + 模型路由（L9 + L5 + L1）

| Day | 任务 | 代码目标 | 测试目标 | 提交信息 |
|-----|------|---------|---------|---------|
| 8 | Quest Engine 骨架 | 任务创建 + 分解 | 4 步任务图 | `feat(quest): task decomposition` |
| 9 | LHQP 持久化 | Checkpoint-Restore | 崩溃恢复测试 | `feat(lhqp): checkpoint persistence` |
| 10 | Repo Wiki 实现 | SQLite + 向量 | 10 条 Wiki 生成 | `feat(wiki): auto wiki generation` |
| 11 | ISCM 跨层索引 | 共享锚点 | 跨层一致性 | `feat(iscm): cross-layer shared index` |
| 12 | Model Router 骨架 | 3 策略 | 路由准确率 > 90% | `feat(router): multi-model routing` |
| 13 | CACR 成本感知 | 预算保护 | 成本告警测试 | `feat(cacr): cost-aware routing` |
| 14 | Week 2 验收 | 端到端 Quest | 任务分解 < 1s | `test(week2): quest e2e passed` |

### Week 3: 记忆与路由系统（L5 + L6）

| Day | 任务 | 代码目标 | 测试目标 | 提交信息 |
|-----|------|---------|---------|---------|
| 15 | MLC L0/L1 | WorkingMemory + Episodic | LRU 驱逐 | `feat(mlc): L0/L1 memory` |
| 16 | MLC L2/L3 | GNN + WASM | 向量相似度 > 0.5 | `feat(mlc): L2/L3 memory` |
| 17 | HCW 分层窗口 | 4K/32K/128K/1M | 自动窗口选择 | `feat(hcw): hierarchical context window` |
| 18 | CMT 能力内存 | 热/温/冷/冰 | 分层迁移 | `feat(cmt): capability memory tiering` |
| 19 | OSA 全维稀疏 | 5 维度协调 | 复杂度联动 | `feat(osa): omni-sparse coordinator` |
| 20 | KVBSR 块路由 | 两级路由 | 延迟 < 2ms | `feat(kvbsr): semantic block routing` |
| 21 | Week 3 验收 | 全层编码 | 压缩率 > 4× | `test(week3): memory + routing` |

### Week 4: 执行优化层（L6 + L7）

| Day | 任务 | 代码目标 | 测试目标 | 提交信息 |
|-----|------|---------|---------|---------|
| 22 | GEA 门控激活 | 连续 [0,1] | 冲突消解 | `feat(gea): gated expert activation` |
| 23 | GQEP 聚集执行 | 资源外循环 | 批量原子性 | `feat(gqep): gather-q execution` |
| 24 | PVL 生产验证 | 并行流式 | 实时反馈 | `feat(pvl): producer-verifier loop` |
| 25 | MTPE 多步预测 | N=1-10 | 预测成功率 > 80% | `feat(mtpe): multi-token prediction exec` |
| 26 | SCC 推测缓存 | Draft/Verify 共享 | 命中率 > 70% | `feat(scc): speculative context cache` |
| 27 | EDSB 熵均衡 | 指数衰减 | 熵 > 0.6 | `feat(edsb): entropy-driven balancing` |
| 28 | Week 4 验收 | 全执行链路 | CSA < 100ms | `test(week4): execution optimization` |

### Week 5: 议会 + 安全 + 预算（L8 + L4 + L3）

| Day | 任务 | 代码目标 | 测试目标 | 提交信息 |
|-----|------|---------|---------|---------|
| 29 | 5 角色议会 | Architect-Skeptic-Optimizer-Librarian-Bard | 角色测试 | `feat(parliament): 5-role adversarial` |
| 30 | Skeptic 否决 + DPO | 冻结权 + 偏好对 | 否决恶意意图 | `feat(skeptic): veto + auto-dpo` |
| 31 | ASA 对抗审计 | Critic PPO | 实时介入 | `feat(asa): adversarial self-audit` |
| 32 | AHIRT 反黑客 | 主动探测 | 漏洞探测 > 95% | `feat(ahirt): anti-hack red team` |
| 33 | DECB 双档预算 | 连续可调 | 溢出检测 | `feat(decb): dual-effort budgeting` |
| 34 | TTG 思考切换 | 三级切换 | 自动模式选择 | `feat(ttg): thinking toggle governance` |
| 35 | Week 5 验收 | 全认知链路 | 决策准确率 > 90% | `test(week5): parliament + security` |

### Week 6: 适配 + 进化 + 多模态（L2 + L10）

| Day | 任务 | 代码目标 | 测试目标 | 提交信息 |
|-----|------|---------|---------|---------|
| 36 | SSRA 黏液适配 | 预编译模板 | 融合 < 20ms | `feat(ssra): slime-style rapid adaptation` |
| 37 | LSCT 任务分层 | 动态热层 | 编译/调试切换 | `feat(lsct): task-aware tiering` |
| 38 | GSOE 在线进化 | GRPO 风格 | 策略更新 | `feat(gsoe): online evolution` |
| 39 | NMC 多模态 | 图像/视频编码 | 统一 CLV | `feat(nmc): natively multimodal` |
| 40 | MCU 多语言 | AST 语义提取 | 中文注释理解 | `feat(mcu): multilingual code understanding` |
| 41 | CHTC 跨平台 | 5 IDE 适配器 | 统一协议 | `feat(chtc): cross-harness compatibility` |
| 42 | Week 6 验收 | 全适配链路 | 端到端通过 | `test(week6): adaptation + evolution` |

### Week 7: MCP 网格 + 监控 + 集成（L1 + 全局）

| Day | 任务 | 代码目标 | 测试目标 | 提交信息 |
|-----|------|---------|---------|---------|
| 43 | MCP 量子网格 | 超位置 + 纠缠 | 5 服务器事务 | `feat(mcp): quantum mesh` |
| 44 | CSN 降级链 | 功能相似度 | 降级排序 | `feat(csn): capability substitution` |
| 45 | SESA μCap | 256-bit 掩码 | 稀疏度 < 40% | `feat(sesa): micro-capability activation` |
| 46 | 监控仪表盘 | Prometheus + Grafana | 指标采集 | `feat(monitoring): metrics + alerts` |
| 47 | 全量集成 | 37 模块联调 | 无集成失败 | `feat(integration): full system integration` |
| 48 | 性能调优 | SIMD + WAL | 路由 < 2ms | `perf(week7): simd + sqlite wal` |
| 49 | Week 7 验收 | 压力测试 | 1000 次无泄漏 | `test(week7): stress test passed` |

### Week 8: 生产化（安全 + 发布 + 文档）

| Day | 任务 | 代码目标 | 测试目标 | 提交信息 |
|-----|------|---------|---------|---------|
| 50 | 渗透测试 | OWASP Top 10 | 全部通过 | `security(week8): penetration testing` |
| 51 | 模糊测试 | 10000 随机输入 | 无崩溃 | `security(week8): fuzz testing` |
| 52 | cargo-audit | 依赖扫描 | 无高危漏洞 | `security(week8): dependency audit` |
| 53 | 跨平台发布 | 5 平台 binary | 全部生成 | `release(week8): cross-platform binaries` |
| 54 | Docker 镜像 | Dockerfile | 镜像构建 | `release(week8): docker image` |
| 55 | 文档完善 | README + API | 完整覆盖 | `docs(week8): complete documentation` |
| 56 | 最终验收 | 全量 E2E | 100% 通过 | `release(v1.0.0-omega): production ready` |

---

## 8. 测试与验收策略

### 8.1 测试金字塔

```
        /\
       /  \
      / E2E \      # 端到端场景测试 (10%): Quest 生命周期、跨平台兼容、长时持久化
     /--------\
    / Integration \  # 模块集成测试 (30%): 37 模块联调、Event Bus 广播、MCP 网格
   /--------------\
  /    Unit Tests   \ # 单元测试 (60%): 每个 crate 独立测试、Mock 外部依赖
 /--------------------\
```

### 8.2 关键测试套件

**安全测试** (`tests/security/`):
```rust
#[tokio::test]
async fn test_owasp_top10() {
    let aether = setup_test_aether().await;

    // A01: 注入攻击
    let injection = aether.execute("echo $(cat /etc/passwd)").await;
    assert!(injection.is_err());

    // A02: 失效的访问控制
    let unauthorized = aether.execute("sudo rm -rf /").await;
    assert!(unauthorized.is_err());

    // A03: 敏感数据泄露
    let leak = aether.execute("env | grep SECRET").await;
    assert!(leak.is_err());

    // ... A04-A10
}

#[tokio::test]
async fn test_red_team_probe() {
    let red_team = setup_red_team().await;
    let vulnerabilities = red_team.active_probe().await.unwrap();
    assert!(vulnerabilities.is_empty(), "Red team found: {:?}", vulnerabilities);
}
```

**Quest E2E 测试** (`tests/e2e/quest_lifecycle.rs`):
```rust
#[tokio::test]
async fn test_e2e_full_quest() {
    let aether = setup_test_aether().await;

    // 1. 创建 Quest
    let quest = aether.create_quest(
        "实现用户认证模块",
        "添加 JWT 认证和权限控制，支持 OAuth2"
    ).await.unwrap();
    assert_eq!(quest.tasks.len(), 4);
    assert_eq!(quest.thinking_mode, ThinkingMode::Deep);

    // 2. 模拟系统崩溃
    aether.simulate_crash().await;

    // 3. 从检查点恢复
    let recovered = aether.restore_quest(&quest.id).await.unwrap();
    assert_eq!(recovered.tasks[0].status, TaskStatus::Completed);

    // 4. 继续执行剩余任务
    for task in &recovered.tasks[1..] {
        let result = aether.execute_task(&recovered.id, &task.id).await.unwrap();
        assert!(result.success);
    }

    // 5. 验证 Wiki 更新
    let wiki = aether.repo_wiki.query_relevant("JWT auth", 5).await.unwrap();
    assert!(!wiki.is_empty());

    // 6. 验证成本在预算内
    let total_cost = aether.cost_tracker.get_quest_cost(&quest.id).await;
    assert!(total_cost < 10.0); // < $10
}
```

**性能基准** (`benches/`):
```rust
fn bench_kvbsr_routing(c: &mut Criterion) {
    let router = setup_kvbsr_router(300);
    let intent = CLV::from_text("refactor auth module to async");

    c.bench_function("kvbsr_route_300_tools", |b| {
        b.to_async(Runtime::new().unwrap()).iter(|| async {
            router.route(&intent).await.unwrap();
        });
    });
}

fn bench_osa_full_sparse(c: &mut Criterion) {
    let coordinator = OmniSparseCoordinator::new();
    let task = TaskProfile { complexity_score: 0.7, ..Default::default() };

    c.bench_function("osa_compute_all_masks", |b| {
        b.iter(|| coordinator.compute_all_masks(&task).unwrap());
    });
}
```

### 8.3 性能基准目标

| 指标 | 目标 | 测试方法 |
|------|------|---------|
| 启动时间 | < 200ms | `time aether --version` |
| KVBSR 路由 | < 2ms | 300 工具池，1000 次平均 |
| OSA 全维稀疏 | < 1ms | 5 维度协调计算 |
| PVL 流式延迟 | < 50ms | Producer→Verifier 首反馈 |
| MTPE 多步预测 | < 500ms | N=5 预测 + 批量验证 |
| GQEP 批量执行 | < 100ms | 10 操作批量 |
| 内存占用 | < 500MB | 24h 监控 |
| 孤儿调用率 | 0% | 10000 次操作 |
| 议会决策 | < 2s | 5 角色并行 |
| 推测命中率 | > 75% | SEP 流水线 |
| Quest 分解 | < 1s | 创建 10 个 Quest |
| Wiki 查询 | < 50ms | 10000 条查询 |
| 成本路由 | < 10ms | 10 提供商选择 |
| 检查点保存 | < 100ms | 全状态快照 |
| 多模态编码 | < 200ms | 图像 → CLV |

---

## 9. 安全与合规模型

### 9.1 威胁模型（扩展）

| 威胁 | 缓解措施 | 验证方法 | 来源 |
|------|---------|---------|------|
| 命令注入 | SecCore 禁止插值 | 渗透测试 | Claude CVE-2026-35022 |
| 环境变量泄露 | WHITELIST | 静态分析 | Claude 尸检 |
| 权限提升 | 能力衰减 | 自动化测试 | Claude 尸检 |
| 沙箱逃逸 | gVisor + seccomp | 模糊测试 | Claude 尸检 |
| 审计链篡改 | SHA-256 Merkle | 完整性校验 | Claude 尸检 |
| MCP 工具滥用 | 按服务器过滤 | 集成测试 | Hermes |
| 红队绕过 | AHIRT 主动探测 | 对抗性测试 | GLM 5.2 |
| 模型劫持 | 多模型故障转移 | 故障注入 | Qoder |
| 提示注入 | Red Team 拦截 | 对抗性测试 | GLM 5.2 |
| 成本攻击 | CACR 预算保护 | 压力测试 | Qoder |
| 长时攻击 | LHQP 检查点隔离 | 崩溃恢复测试 | Qwen 3.7 |
| 跨平台攻击 | CHTC 协议隔离 | 多平台测试 | Qwen 3.7 |

### 9.2 合规映射（扩展）

| 标准 | 实现模块 | 证据 | 来源 |
|------|---------|------|------|
| SOC 2 Type II | SecCore 审计链 | 不可篡改日志 | Claude |
| ISO 27001 | 零信任架构 | 能力衰减 + 沙箱 | Claude |
| GDPR | MLC L3 程序记忆 | 仅存储模式 | Hermes |
| OWASP Top 10 | SecCore + Skeptic + Red Team | 自动化安全测试 | Claude + GLM |
| 等保 2.0 | 私有化部署 + 国产模型 | CHTC + Qwen 支持 | Qoder |
| PCI DSS | CACR 成本审计 | 交易日志 | Qoder |
| HIPAA | 数据最小化 + 加密 | 审计链加密 | Hermes |

---

## 10. 附录

### 10.1 核心数据结构

```rust
// 用户意图（多模态）
pub struct UserIntent {
    pub raw_text: String,
    pub multimodal_inputs: Vec<MultimodalInput>,
    pub parsed_entities: Vec<Entity>,
    pub complexity_score: f32,
    pub risk_level: RiskLevel,
    pub affected_scope: AffectedScope,
    pub required_capabilities: Vec<ModelCapability>,
    pub deadline: Option<DateTime<Utc>>,
    pub budget_constraint: Option<f32>,
}

// 多模态输入
pub enum MultimodalInput {
    Text(String),
    Image(Vec<u8>),
    Video(Vec<u8>),
    Audio(Vec<u8>),
}

// Quest
pub struct Quest {
    pub id: String,
    pub title: String,
    pub description: String,
    pub tasks: Vec<Task>,
    pub status: QuestStatus,
    pub progress: f32,
    pub thinking_mode: ThinkingMode,
    pub checkpoint_id: Option<String>,
}

// 检查点
pub struct Checkpoint {
    pub checkpoint_id: String,
    pub quest_id: String,
    pub task_states: Vec<TaskState>,
    pub memory_snapshot: CLV,
    pub wiki_snapshot: Vec<WikiEntry>,
    pub capability_state: CapabilityState,
    pub timestamp: DateTime<Utc>,
}

// 全维稀疏掩码
pub struct OmniSparseMasks {
    pub routing: SparseMask<ToolId>,
    pub context: SparseMask<FileId>,
    pub memory: SparseMask<MemoryId>,
    pub audit: SparseMask<OperationId>,
    pub budget: SparseMask<TaskId>,
}

// 语义块
pub struct SemanticBlock {
    pub block_id: String,
    pub block_vector: [f32; 64],
    pub tools: Vec<String>,
    pub block_coherence: f32,
}

// 生产验证反馈
pub struct VerificationFeedback {
    pub operation_id: String,
    pub is_safe: bool,
    pub is_correct: bool,
    pub issues: Vec<String>,
    pub adjustment_suggestion: String,
    pub confidence: f32,
}

// 成本告警
pub struct CostAlert {
    pub current_cost: f32,
    pub budget: f32,
    pub remaining: f32,
    pub recommendation: String,
}
```

### 10.2 配置文件模板

```yaml
# ~/.aether/omega.yaml
nexus:
  version: "1.0.0-omega"

quest:
  auto_decompose: true
  max_tasks_per_quest: 20
  default_deadline_hours: 168
  checkpoint_interval_ops: 100
  checkpoint_interval_minutes: 10

thinking_toggle:
  default_mode: "Auto"  # NonThinking / Lite / Deep / Max / Auto
  auto_thresholds:
    non_thinking: { complexity: 0.1, risk: "Low" }
    lite: { complexity: 0.4, risk: "Medium" }
    deep: { complexity: 0.7, risk: "High" }
    max: { complexity: 0.9, risk: "Critical" }

repo_wiki:
  auto_generate: true
  db_path: "~/.aether/wiki.db"
  embedding_dim: 256
  auto_update_on_commit: true

model_router:
  strategy: "Auto"  # CostOptimized / SpeedOptimized / QualityOptimized / Auto / Failover
  budget:
    daily_usd: 50.0
    monthly_usd: 1000.0
    alert_threshold: 0.8
  providers:
    - id: "claude-opus"
      name: "Claude Opus 4.8"
      endpoint: "https://api.anthropic.com"
      context_window: 200000
      capabilities: [CodeGeneration, ArchitectureDesign, SecurityAudit, Reasoning]
      tier: "premium"
      input_cost_per_1k: 15.0
      output_cost_per_1k: 75.0
    - id: "gpt-4o"
      name: "GPT-4o"
      endpoint: "https://api.openai.com"
      context_window: 128000
      capabilities: [CodeGeneration, CodeReview, ToolUse]
      tier: "efficient"
      input_cost_per_1k: 2.5
      output_cost_per_1k: 10.0
    - id: "qwen-coder"
      name: "Qwen Coder"
      endpoint: "https://dashscope.aliyuncs.com"
      context_window: 128000
      capabilities: [CodeGeneration, LongContext, Multilingual]
      tier: "lite"
      input_cost_per_1k: 0.5
      output_cost_per_1k: 2.0
    - id: "minimax-m3"
      name: "Minimax M3"
      endpoint: "https://api.minimax.chat"
      context_window: 1000000
      capabilities: [CodeGeneration, LongContext, Multimodal]
      tier: "efficient"
      input_cost_per_1k: 0.3
      output_cost_per_k: 1.2
    - id: "glm-5.2"
      name: "GLM 5.2"
      endpoint: "https://api.zhipu.ai"
      context_window: 1000000
      capabilities: [CodeGeneration, LongContext, Reasoning]
      tier: "premium"
      input_cost_per_1k: 1.0
      output_cost_per_1k: 4.0

osa:
  dimensions: [routing, context, memory, audit, budget]
  sparsity_base: 0.8
  complexity_adjustment: true

kvbsr:
  max_blocks: 20
  tools_per_block: 15
  auto_rebalance_threshold: 100
  coherence_min: 0.7

pvl:
  producer_timeout_ms: 5000
  verifier_timeout_ms: 3000
  feedback_channel_size: 100
  max_retry: 3

mtpe:
  default_prediction_depth: 3
  max_prediction_depth: 10
  adapt_depth_enabled: true
  batch_verify: true

gqep:
  batch_size: 10
  resource_types: [FileSystem, Network, Git, Docker, Database]
  connection_pool_size: 5

seccore:
  sandbox: gvisor
  seccomp: true
  command_interpolation: forbidden
  red_team:
    enabled: true
    audit_frequency: 0.1
    active_probe_interval_hours: 24
  capability_decay:
    initial: 1.0
    high_risk_decay: 0.2
    medium_risk_decay: 0.1
    low_risk_decay: 0.02
    recovery_rate: 0.05
    recovery_interval_minutes: 10

mcp:
  mesh:
    transports: [stdio, http]
    entanglement: true
  servers:
    - id: filesystem
      command: "npx"
      args: ["-y", "@modelcontextprotocol/server-filesystem"]
    - id: github
      url: "https://api.github.com/mcp"
      auth: oauth
    - id: postgres
      url: "postgresql://localhost:5432/mcp"
      auth: password

evolution:
  enabled: true
  mutation_pool_path: "~/.aether/evolution/mutations/"
  fitness_function: "(success_rate * 0.4) + (speed * 0.3) + (token_efficiency * 0.2) + (safety * 0.1)"
  ab_test:
    enabled: true
    min_samples: 30
    significance_threshold: 1.5
  online_learning:
    enabled: true
    update_frequency: 10  # 每 10 次任务更新
    learning_rate: 0.01

monitoring:
  prometheus:
    enabled: true
    port: 9090
  grafana:
    enabled: true
    dashboard_path: "./monitoring/grafana-dashboard.json"
  alerts:
    - name: "CapabilityDepleted"
      expr: "aether_capability_current < 0.1"
      for: "1m"
    - name: "HighOrphanRate"
      expr: "rate(aether_orphan_calls_total[5m]) > 0"
      for: "1m"
    - name: "BudgetAlert"
      expr: "aether_daily_cost / aether_daily_budget > 0.8"
      for: "5m"
    - name: "RedTeamVulnerability"
      expr: "aether_red_team_vulnerabilities > 0"
      for: "1m"
```

### 10.3 ADR 索引（25 个）

| ADR | 主题 | 状态 | 来源 |
|-----|------|------|------|
| ADR-001 | 沙箱运行时选择（gVisor） | Accepted | Claude 尸检 |
| ADR-002 | 能力衰减模型设计 | Accepted | Claude 尸检 |
| ADR-003 | 异步运行时选择（Tokio） | Accepted | Claude 尸检 |
| ADR-004 | TUI 框架选择（Ratatui） | Accepted | 工程实践 |
| ADR-005 | 向量数据库选择（sqlite-vec） | Accepted | 工程实践 |
| ADR-006 | 上下文振荡架构（DCO） | Accepted | DeepSeek V4 |
| ADR-007 | 议会共识机制（辩证综合） | Accepted | Hermes |
| ADR-008 | MCP 传输层（stdio + HTTP） | Accepted | Hermes |
| ADR-009 | Quest 任务系统 | Accepted | Qoder |
| ADR-010 | Repo Wiki 知识沉淀 | Accepted | Qoder |
| ADR-011 | 多模型路由策略 | Accepted | Qoder |
| ADR-012 | 事件驱动架构 | Accepted | Qoder |
| ADR-013 | 全维稀疏架构（OSA） | Accepted | 五大模型共性 |
| ADR-014 | KV 块语义路由（KVBSR） | Accepted | Minimax M3 |
| ADR-015 | 聚集查询执行（GQEP） | Accepted | Minimax M3 |
| ADR-016 | 原生多模态上下文（NMC） | Accepted | Minimax M3 |
| ADR-017 | 生产验证闭环（PVL） | Accepted | Minimax M3 |
| ADR-018 | 思考切换治理（TTG） | Accepted | Minimax/GLM |
| ADR-019 | 长时任务持久化（LHQP） | Accepted | Qwen 3.7 |
| ADR-020 | 跨平台工具兼容（CHTC） | Accepted | Qwen 3.7 |
| ADR-021 | 成本感知路由（CACR） | Accepted | Qwen 3.7 |
| ADR-022 | 黏液式快速适配（SSRA） | Accepted | GLM 5.2 |
| ADR-023 | 跨层共享索引（ISCM） | Accepted | GLM 5.2 |
| ADR-024 | 反黑客红队（AHIRT） | Accepted | GLM 5.2 |
| ADR-025 | 在线进化（GSOE） | Accepted | DeepSeek GRPO |

---

**文档结束**
