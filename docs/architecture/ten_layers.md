# 10 层架构详解 — NEXUS-OMEGA

> Chimera CLI (NEXUS-OMEGA) 采用 10 层分层架构,每层职责明确,依赖方向严格向下。
> 本文与 [CODE_WIKI.md §2](../../CODE_WIKI.md) 同步,提供更详细的每层说明。

---

## 架构总览

```text
┌─────────────────────────────────────────────────────────────────┐
│ L10  Interface ─ chimera-cli · chimera-tui · chtc-bridge        │
│                   mcp-mesh · csn-substitutor                    │
├─────────────────────────────────────────────────────────────────┤
│ L9   Quest ────── quest-engine · gea-activator · efficiency-monitor │
├─────────────────────────────────────────────────────────────────┤
│ L8   Parliament ─ parliament · acb-governor · decb-governor     │
├─────────────────────────────────────────────────────────────────┤
│ L7   Execution ── pvl-layer · gqep-executor · mtpe-executor     │
│                   ssra-fusion                                   │
├─────────────────────────────────────────────────────────────────┤
│ L6   Router ───── osa-coordinator · kvbsr-router · faae-router  │
│                   sesa-router                                   │
├─────────────────────────────────────────────────────────────────┤
│ L5   Knowledge ── repo-wiki · gsoe-evolution · auto-dpo         │
├─────────────────────────────────────────────────────────────────┤
│ L4   Security ─── seccore · qeep-protocol · decay-engine        │
├─────────────────────────────────────────────────────────────────┤
│ L3   Storage ──── scc-cache · lsct-tiering · cmt-tiering        │
├─────────────────────────────────────────────────────────────────┤
│ L2   Memory ───── nmc-encoder · hcw-window · mlc-engine         │
├─────────────────────────────────────────────────────────────────┤
│ L1   Core ─────── nexus-core · event-bus · model-router         │
└─────────────────────────────────────────────────────────────────┘
```

---

## 依赖方向规则(铁律)

```
L(N) → L(N)     ✓ 同层互引允许
L(N) → L(N-1)   ✓ 向下依赖允许
L(N) → L(N+1)   ✗ 向上依赖禁止
L(N) ──event-bus── L(M)  ✓ 跨层通信只能走 Event Bus
L(N) ──mcp-mesh─── L(M)  ✓ 跨进程通信只能走 MCP Mesh
```

### 关键约束

- `nexus-core` 必须保持最小依赖,不能直接 import 上层任何 crate
- `event-bus` 是唯一的模块间通信通道,所有状态变更必须通过事件类型广播
- 任何违反依赖方向规则的 import 必须被拒绝,除非有 ADR 记录特批
- `decb-governor` 归位 L8 Parliament(非 L3 Storage)

### 依赖方向验证

| 验证项 | 结果 | 说明 |
|--------|------|------|
| 跨层向上依赖 | ✓ 0 个 | 所有 crate 遵守 L(N)→L(N-1) 铁律 |
| EventBus 解耦 | ✓ | 4 处原违规(V1-V4)已通过事件修正 |
| dev-dependencies | ✓ | 测试代码可绕过生产依赖方向(测试非生产) |
| nexus-core 最小化 | ✓ | 无上层 import |

---

## 各层详解

### L1 Core — 核心层

| Crate | 职责 | 关键类型 |
|-------|------|---------|
| `nexus-core` | 全局核心类型 + 共享工具 | `CLV`(512-dim)、`NexusState`、`UserIntent`、`FileId` |
| `event-bus` | 跨层通信唯一通道 | `NexusEvent`(40+ 变体)、`EventMetadata`、`EventSeverity` |
| `model-router` | CACR 成本感知模型路由 | `ModelRoute`、`RouteDecision` |

**设计约束**:
- `nexus-core` 仅依赖 Rust 标准库 + workspace 共享依赖,保持最小化
- `event-bus` 基于 `tokio::broadcast`,Critical 级事件无订阅者时记录 `warn`
- 所有 async fn 必须满足 `Send + 'static + 'async` 约束

### L2 Memory — 记忆层

| Crate | 职责 | 关键机制 |
|-------|------|---------|
| `nmc-encoder` | 神经多模态上下文编码(5 感知器 → CLV) | TextPerceptor + Image/Video/Audio/Desktop |
| `hcw-window` | 分层上下文窗口(4K/32K/128K/1M) | 1M = 128K 实际 + 8× 稀疏化压缩 |
| `mlc-engine` | 四级神经形态记忆 | L0 工作 / L1 情景 / L2 语义 / L3 程序 |

**事件流**:
- `NmcEncoded`(L2→L9)、`ContextWindowSwitched`、`ContextCompressed`、`MemoryMetricsReported`、`MemoryTiered`

### L3 Storage — 存储层

| Crate | 职责 | 关键机制 |
|-------|------|---------|
| `scc-cache` | 推测上下文缓存 | 马尔可夫链预取(>0.6)、LRU 驱逐、WAL 崩溃恢复 |
| `lsct-tiering` | 任务感知能力分层 | 编译/调试/测试/运行负载画像 → 目标层级 |
| `cmt-tiering` | 能力记忆分层(热/温/冷/冰) | SQLite WAL + PRAGMA、cascade 降级防级联 |

**事件流**:
- `LsctTierSwitched`(LSCT→CMT)、`CapabilityTiered`

### L4 Security — 安全层

| Crate | 职责 | 关键机制 |
|-------|------|---------|
| `seccore` | 安全核心 + ASA 对抗审计 | Merkle 审计链、白名单 + 静态分析、Allow/Warn/Block |
| `qeep-protocol` | 量子纠缠执行(孤儿检测) | Sender drop 检测未完成 Future、超时治理 |
| `decay-engine` | 能力衰减引擎 | 连续权限流体(ADR-002) |

**事件流**:
- `SandboxViolation`、`AsaIntervention` [Critical]、`CapabilityFrozen`、`OrphanCallDetected` [Critical]

### L5 Knowledge — 知识层

| Crate | 职责 | 关键机制 |
|-------|------|---------|
| `repo-wiki` | 代码 Wiki + ISCM 跨层索引 | SQLite 持久化、全文检索、向量 KNN |
| `gsoe-evolution` | 引导式自组织在线进化 | GRPO 风格(DeepSeek V4)、订阅 ConsensusReached/RedTeamAudit |
| `auto-dpo` | DPO 偏好优化 | 偏好数据生成与优化(Week 8 补齐) |

**事件流**:
- `GsoePolicyUpdated`、Wiki 检索通过 `wiki "查询"` CLI 子命令

### L6 Router — 路由层

| Crate | 职责 | 关键机制 |
|-------|------|---------|
| `osa-coordinator` | 全维稀疏协调器 | 五维度掩码(routing/context/memory/audit/budget) |
| `kvbsr-router` | KV-Block 语义路由 | 两级块路由(块级 Top-N + 块内 Top-K)、Union-Find 聚类 |
| `faae-router` | Function-as-Expert + EDSB | Top-K 精筛、香农熵均衡、指数衰减负载统计 |
| `sesa-router` | 子专家稀疏激活 | 256-bit 位向量掩码、O(n) Top-K、稀疏度 < 40% |

**事件流**:
- `OmniSparseMasksComputed`(L6→L2)、`ExpertRouted`、`EntropyBalanced`、`SesaActivationCompleted`

### L7 Execution — 执行层

| Crate | 职责 | 关键机制 |
|-------|------|---------|
| `pvl-layer` | 生产验证闭环 | Producer-Verifier mpsc 通道、拒绝率 > 30% 策略调整 |
| `gqep-executor` | 聚集查询执行协议 | FuturesUnordered 流式聚集、超时治理、QEEP 孤儿检测 |
| `mtpe-executor` | 多步预测执行 | N=1-10 伪预测、成功率分组、失败回退单步 |
| `ssra-fusion` | 黏液式快速适配 | 预编译模板 + 运行时融合(p95 ≤ 20ms,实测 5.64µs) |

**事件流**:
- `OperationProduced`、`PredictionVerified`、`GatherCompleted`、`OperationTimedOut`、`OrphanCallDetected` [Critical]、`SsraFusionCompleted`

### L8 Parliament — 议会层

| Crate | 职责 | 关键机制 |
|-------|------|---------|
| `parliament` | 5 角色对抗性议会 | Architect/Skeptic/Optimizer/Librarian/Bard、FuturesUnordered 并发 |
| `acb-governor` | ACB 能力预算治理器 | 能力预算控制(Week 8 补齐) |
| `decb-governor` | 双档认知预算治理 | 连续可调 [0,1]、HighTier/LowTier/Degraded 三档 |

**事件流**:
- `DebateStarted`、`SkepticVeto` [Critical]、`RedTeamAudit` [Critical]、`ConsensusReached`、`VoteCast`、`BudgetAdjusted`、`BudgetExceeded` [Critical]

**权重**:Architect=0.25 / Skeptic=0.30 / Optimizer=0.20 / Librarian=0.15 / Bard=0.10

### L9 Quest — 任务层

| Crate | 职责 | 关键机制 |
|-------|------|---------|
| `quest-engine` | Quest 引擎 + TTG + LHQP | 任务分解、DAG 校验、检查点持久化(MessagePack + SHA-256) |
| `gea-activator` | 门控专家激活器 | Sigmoid 连续 [0,1] 门控、Top-K + CLV 重叠检测 |
| `efficiency-monitor` | 效率监控与告警 | Prometheus /metrics、4 个 Critical 事件立即告警 |

**事件流**:
- `QuestCreated`、`ThinkingModeSwitched`、`CheckpointSaved` [Critical]、`ExpertActivated`、`EfficiencyAlertTriggered`

**TTG 选择规则**:Degraded→Fast、简单+非HighTier→Fast、中等或LowTier→Standard、复杂或HighTier→Deep

### L10 Interface — 接口层

| Crate | 职责 | 关键机制 |
|-------|------|---------|
| `chimera-cli` | CLI 入口 | Clap + Figment 多源配置(默认 < File < Env < CLI) |
| `chimera-tui` | TUI 界面 | ratatui + crossterm(Week 8 补齐) |
| `chtc-bridge` | 跨平台 IDE 桥 | 5 大 IDE 适配器(VSCode/IntelliJ/Vim/Emacs/Zed)、enum dispatch |
| `mcp-mesh` | MCP 量子网格 | 2PC 量子事务、超位置查询、纠缠链接(跨进程通信唯一通道) |
| `csn-substitutor` | 能力替代网络 | 余弦相似度 Top-K 替代、多级降级链(≥ 3 级) |

**事件流**:
- `ChtcToolCallReceived`(L10→下层)、`McpMeshTransactionCompleted`、`CsnSubstitutionTriggered`

---

## 架构红线(来自四次尸检)

| 问题 | Claude Code 教训 | 本项目红线 |
|------|-----------------|-----------|
| 函数太大? | `print.ts` 3167 行神函数 | **单函数 ≤200 行,超过必须拆模块** |
| 结果丢了? | 5.4% 孤儿调用 | **所有异步操作必须有 GQEP 聚集/超时处理** |
| 裸奔? | 命令插值 + auth 跳过 | **所有外部调用经 SecCore 沙箱 + Decay 衰减** |
| 竞态? | void Promise 无 await | **所有 async 必须 await 或 spawn 管理** |
| 功能乱? | 44 个未发布标志 | **禁止功能标志,用能力场自然进化替代** |
| 内存爆炸? | 1M Token 暴力加载 | **必须经 HCW 分层 + OSA 稀疏化后再加载** |

---

> **维护者**:NEXUS-OMEGA 团队
> **文档版本**:Week 8 同步(2026-06-27)
