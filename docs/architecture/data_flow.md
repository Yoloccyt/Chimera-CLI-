# 数据流图 — NEXUS-OMEGA

> 本文描述 Chimera CLI (NEXUS-OMEGA) 的端到端数据流,从用户输入到 Wiki 沉淀的完整链路。
> 与 [CODE_WIKI.md §4](../../CODE_WIKI.md) 同步,提供更详细的流程说明。

---

## 1. 端到端认知治理流程

```text
用户输入
    │
    ▼
┌─────────────────────────────────────────────────────────────┐
│ L10 Interface                                               │
│  chimera-cli 接收 → chimera-tui/chtc-bridge/mcp-mesh        │
│  发布 ChtcToolCallReceived 事件                             │
└───────────────────────────┬─────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│ L2 Memory                                                   │
│  nmc-encoder 多模态编码 → CLV(512-dim)                     │
│  发布 NmcEncoded 事件                                       │
└───────────────────────────┬─────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│ L9 Quest                                                    │
│  quest-engine 分解 → Quest + Tasks(DAG 校验)               │
│  ttg 切换思考模式(Fast/Standard/Deep)                      │
│  发布 QuestCreated / ThinkingModeSwitched 事件              │
└───────────────────────────┬─────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│ L8 Parliament                                               │
│  parliament 5 角色辩论(Architect/Skeptic/Optimizer/...)    │
│  Skeptic 否决检查(25 条规则,5 类攻击)                     │
│  AHIRT 红队探测(100 载荷)                                  │
│  发布 ConsensusReached / SkepticVeto / RedTeamAudit 事件    │
└───────────────────────────┬─────────────────────────────────┘
                            │(共识达成)
                            ▼
┌─────────────────────────────────────────────────────────────┐
│ L7 Execution                                                │
│  pvl-layer 生产验证闭环(Producer-Verifier)                 │
│  mtpe-executor 多步预测(N=1-10)                            │
│  gqep-executor 聚集查询(FuturesUnordered + 超时治理)       │
│  ssra-fusion 黏液式适配(模板融合)                          │
│  发布 OperationProduced / PredictionVerified 事件           │
└───────────────────────────┬─────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│ L6 Router                                                   │
│  osa-coordinator 全维稀疏(五维度掩码)                      │
│  kvbsr-router 语义块路由(两级:块级 Top-N + 块内 Top-K)    │
│  faae-router Function-as-Expert(熵驱动自均衡)              │
│  sesa-router 子专家稀疏激活(256-bit 掩码,< 40%)           │
│  发布 OmniSparseMasksComputed / ExpertRouted 事件           │
└───────────────────────────┬─────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│ L9 Quest(激活)                                            │
│  gea-activator 门控专家激活(Sigmoid 连续门控)              │
│  发布 ExpertActivated 事件                                  │
└───────────────────────────┬─────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│ L4 Security                                                 │
│  qeep-protocol 量子纠缠(孤儿调用检测)                      │
│  seccore SecCore 沙箱(零信任 + ASA 审计)                   │
│  decay-engine 能力衰减(连续权限流体)                       │
│  发布 OrphanCallDetected / SandboxViolation 事件            │
└───────────────────────────┬─────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│ L5 Knowledge                                                │
│  repo-wiki ISCM 更新(跨层共享索引)                         │
│  gsoe-evolution 在线进化(GRPO 风格)                        │
│  auto-dpo 偏好优化                                          │
│  Wiki 沉淀(SQLite 持久化 + 向量检索)                       │
│  发布 GsoePolicyUpdated 事件                                │
└───────────────────────────┬─────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│ L1 Core                                                     │
│  event-bus 广播所有事件(Tokio broadcast)                   │
│  所有层订阅感兴趣的事件                                     │
└─────────────────────────────────────────────────────────────┘
```

---

## 2. 事件流详解

### 2.1 关键事件链路

| 事件 | 发布者 | 订阅者 | 级别 |
|------|--------|--------|------|
| `ChtcToolCallReceived` | chtc-bridge (L10) | mcp-mesh (L10) | Normal |
| `NmcEncoded` | nmc-encoder (L2) | quest-engine (L9) | Normal |
| `QuestCreated` | quest-engine (L9) | parliament (L8) | Normal |
| `ThinkingModeSwitched` | quest-engine (L9) | efficiency-monitor (L9) | Normal |
| `ConsensusReached` | parliament (L8) | gsoe-evolution (L5) / ssra-fusion (L7) / sesa-router (L6) | Normal |
| `SkepticVeto` | parliament (L8) | decay-engine (L4) / efficiency-monitor (L9) | Critical |
| `RedTeamAudit` | parliament (L8) | gsoe-evolution (L5) / ssra-fusion (L7) / efficiency-monitor (L9) | Critical |
| `BudgetExceeded` | decb-governor (L8) | quest-engine (L9) / efficiency-monitor (L9) | Critical |
| `OmniSparseMasksComputed` | osa-coordinator (L6) | hcw-window (L2) | Normal |
| `LsctTierSwitched` | lsct-tiering (L3) | cmt-tiering (L3) | Normal |
| `CheckpointSaved` | quest-engine (L9) | — | Critical |
| `SandboxViolation` | seccore (L4) | parliament (L8) / decay-engine (L4) | Critical |
| `OrphanCallDetected` | gqep-executor (L7) | qeep-protocol (L4) | Critical |
| `McpMeshTransactionCompleted` | mcp-mesh (L10) | csn-substitutor (L10) / efficiency-monitor (L9) | Normal |
| `CsnSubstitutionTriggered` | csn-substitutor (L10) | efficiency-monitor (L9) / gsoe-evolution (L5) | Normal |
| `EfficiencyAlertTriggered` | efficiency-monitor (L9) | parliament (L8) | Normal |

### 2.2 事件级别说明

- **Normal**:常规事件,无订阅者时静默丢弃(但记录日志)
- **Critical**:关键事件,无订阅者时记录 `warn` 日志。4 个 Critical 事件立即触发 efficiency-monitor 告警:
  - `SkepticVeto` — Skeptic 否决恶意意图
  - `RedTeamAudit` — 红队探测率 < 95%
  - `AsaIntervention` — ASA 安全干预
  - `BudgetExceeded` — 预算超限

---

## 3. Week 6-7 数据流集成

### 3.1 Week 6 数据流

```text
NMC(L2): 用户多模态输入 → CLV 512-dim → NmcEncoded 事件 → Quest Engine
LSCT(L3): 任务负载画像 → 目标层级计算 → LsctTierSwitched 事件 → CMT 执行迁移
GSOE(L5): 订阅 ConsensusReached + RedTeamAudit → 策略变异 → GsoePolicyUpdated 事件
SSRA(L7): 订阅 ConsensusReached + RedTeamAudit → 模板融合 → SsraFusionCompleted 事件
CHTC(L10): IDE 工具调用 → 统一协议归一化 → ChtcToolCallReceived 事件 → 下层处理
```

### 3.2 Week 7 数据流

```text
MCP Mesh(L10): 订阅 ChtcToolCallReceived → 跨服务器量子事务 → McpMeshTransactionCompleted 事件
CSN(L10): 订阅 McpMeshTransactionCompleted(事务失败时)→ 余弦相似度 Top-K 替代 → CsnSubstitutionTriggered 事件
SESA(L6): 订阅 ConsensusReached → 256-bit 掩码稀疏激活 → SesaActivationCompleted 事件
efficiency-monitor(L9): 订阅全部 NexusEvent → 指标采集 + 4 个 Critical 事件立即告警 → EfficiencyAlertTriggered 事件
```

---

## 4. 关键路径性能

| 路径 | 目标 | 实测 | 状态 |
|------|------|------|------|
| 三层路由(SESA → KVBSR → FaaE) | ≤ 2ms | 78.79µs | ✅ 25× 余量 |
| SSRA 模板融合 | ≤ 20ms | 5.64µs | ✅ 3500× 余量 |
| SESA 256 专家激活 | ≤ 5ms | ≤ 5ms | ✅ 达标 |
| MCP Mesh 5 服务器事务 | ≤ 100ms | ≤ 100ms | ✅ 达标 |
| CSN 单次替代查询 | ≤ 30ms | ≤ 30ms | ✅ 达标 |
| WAL 崩溃恢复(1000 次) | 零丢失 | 1000/1000 | ✅ 达标 |

---

## 5. 配置加载流程

```text
内置默认值(ChimeraConfig::default())
    │ 优先级 1(最低)
    ▼
配置文件(~/.aether/omega.yaml)
    │ 优先级 2,可 --config 覆盖
    ▼
环境变量(前缀 AETHER_,嵌套用 __)
    │ 优先级 3,如 AETHER_MODEL__PRIMARY=premium
    ▼
CLI 参数(--model, --config 等)
    │ 优先级 4(最高),Clap 解析
    ▼
Figment 多源合并 → 最终配置
```

---

> **维护者**:NEXUS-OMEGA 团队
> **文档版本**:Week 8 同步(2026-06-27)
