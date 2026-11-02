# ADR-066: 能力协商与三态降级协议

> **状态**: Proposed (2026-07-31)
> **决策日期**: 2026-07-31
> **关联 ADR**: ADR-065(MCA 总纲,本 ADR 承接其 M1 能力协商细化)、
> ADR-037(CapabilityToken 能力场灰度)、ADR-004(MessagePack 序列化)
> **设计源**: `Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.1/§5.3/§7 Round 3
> **权威源**: 本 ADR 记录能力协商算法、三态降级协议、会话状态守恒策略的落地方式

---

## 1. 背景

M0 建成契约与网关骨架、三协议 Codec 后,M1 需要解决"能力如何协商、
特性不支持时如何降级、思考状态如何守恒"三个核心问题。竞品尸检两难
(Claude Code 第三方代理"报错或静默"):按模型名门控导致特性缺失时
要么硬报错中断、要么静默降级暗损体验。MCA 必须给出第三条路。

## 2. 决策

### 决策 1:三态降级协议(设计文档 §7 Round 3 裁决)

能力协商产出三态,由 `NegotiationFidelity` 承载并经
`AffinityCapabilityNegotiated` 事件留痕:

- **FullFidelity**:描述符声明的特性全部启用;
- **DegradedNotified**:非核心特性(思考模式/缓存/结构化输出)缺失 →
  降级 + 事件留痕 + 会话内一次性明确告知(E4 不变量:降级必须告知);
- **ChannelRejected**:核心能力(流式/工具调用)缺失 → 通道标记不可用,
  不进路由池(`spec_loader::validate_spec` 拒绝非流式通道注册)。

**裁决理由**:降级是产品行为不是技术兜底。静默降级违反 E4,硬报错违反 P3
容错。三态协议让"能跑但残血"变为"明确告知的可控降级"。

### 决策 2:TTG × 七厂商思考模式映射(数据化,非代码)

TTG 三档(Fast/Standard/Deep)是全局语义,厂商思考参数完全异构
(`reasoning_effort` 七档 / `enable_thinking` 开关 / thinking budget /
low-high think mode)。映射表存于 spec 配置(`CapabilitySet::thinking`),
协商算法据 `ThinkingSupport` 三态映射:

- `None` → TTG 强制 Fast,发 `AffinityCapabilityNegotiated{degraded: thinking}`;
- `OnOff` → Fast=关,Standard/Deep=开(Deep 追加 max_output 上浮);
- `EffortLevels(levels)` → 按档位表精确映射;请求档位不在取值域内 →
  就近取档并留痕(GLM 各模型取值域不同)。

M1 交付映射的**数据契约与协商三态**;精确档位映射引擎属 M2(T2.1)。

### 决策 3:会话状态守恒策略(P4,MiniMax 断链专项)

`StatePreservationPolicy` 三态,由 `session::apply_preservation_policy`
纯函数落地:

- `None`:剥离 thinking 块省 token,保留 text/tool_use/tool_result;
- `BlockPreservation`:全部块原序保留(Kimi K3 要求回传完整 assistant 块);
- `VerbatimThinking`:同 BlockPreservation,**且承诺 thinking 逐字保真,
  禁止任何 strip/转换**(MiniMax interleaved thinking,C9 硬约束)。

**跨通道迁移**(`session::migrate_history`):切换通道时按新通道策略
转译/保真/安全丢弃思考块并计数留痕。降级到 `None` 通道时思考块被
安全丢弃(不臆造),留痕驱动 E4 告知。

E5 哨兵校验(200 例 proptest):VerbatimThinking 下任意思考内容
(含哨兵)必须逐字幸存,strip 即视为断链。

### 决策 4:健康探针软权重 + 熔断硬开关互补

- **熔断器**(transport.rs `CircuitBreaker`,ADR-065 决策 5 本地复制):
  硬开关,连续 5 次 5xx → 30s 半开,快速止血;
- **健康探针**(health.rs `ChannelHealth`):软权重,成功率/TTFT EWMA
  (α=0.1,对齐 ADR-037),平滑降权喂 model-router;
- 健康分 < 60 → 网关发 `ProviderDegraded`(Normal 级,触发降级链评估)。

TTFT EWMA 喂 E1 体验不变量验收(p50/p95 退化 ≤25%)。

## 3. 后果

**正面**:第三方模型接入不再"残血"——特性缺失明确降级告知,核心能力
缺失干净拒绝;MiniMax 思考链逐字守恒;健康分平滑路由。

**负面与缓解**:
- 三态判定分散风险 → 集中于 capability 协商引擎(M2 T2.1)与
  `spec_loader::validate_spec`,单一事实源;
- 会话 SQLite 持久化开销 → checkpoint 只在 ToolCallEnd/Done 边界落库,
  不做 per-delta 写(避免流式被阻塞)。

## 4. 落地状态(M1)

- ✅ `NegotiationFidelity` 三态(L0)+ `AffinityCapabilityNegotiated` 事件
- ✅ `session::apply_preservation_policy` / `migrate_history`(三态守恒 + 迁移留痕)
- ✅ `SessionStore`(SQLite spawn_blocking,会话历史持久化)
- ✅ `health::ChannelHealth` / `HealthRegistry`(TTFT/成功率 EWMA)
- ✅ E5 哨兵 proptest(200 例,VerbatimThinking 逐字幸存)
- ⏳ 精确档位映射引擎(M2 T2.1 `capability.rs`)
