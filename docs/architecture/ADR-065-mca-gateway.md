# ADR-065: MCA 总纲与 L10 mca-gateway 多通道亲和网关

> **状态**: Proposed (2026-07-31)
> **决策日期**: 2026-07-31
> **关联 ADR**: ADR-033(L0 契约层,affinity 类型落点)、ADR-034(禁止运行时功能标志)、
> ADR-043(影子模式,M3 学习路由前置)、ADR-004(MessagePack 序列化)
> **设计源**: `Chimera_全模型亲和适配体系设计文档_v1.0.md`(PANTHEON v1.0,597 行)
> **权威源**: 本 ADR 记录 MCA(Model-Channel Affinity)体系立项的 6 项架构裁决,
> 后续 ADR-066(能力协商与三态降级)/ADR-067(跨厂商级联与去相关议会)/
> ADR-068(成本治理与峰谷模型)按里程碑节奏跟进

---

## 1. 背景

v2.11.0-omega 基线的病理诊断(设计文档 §2)确认:37 crate 中没有任何 HTTP 客户端
(workspace 声明的 reqwest 0.12 零消费者),十层架构的路由/议会/进化/预算/降级链
全部就绪,唯独缺"模型通道"最后一公里。竞品尸检最大教训(Claude Code):按模型名
字符串匹配做能力门控 + 单厂商 Beta Header 深度耦合,导致第三方模型接入即"降级残血"。

MCA 体系的核心命题:**不为任何单一模型做亲和,而为"协议—能力—上下文—成本—体验"
五个维度建立通用亲和基座**。国内七厂商 API 已收敛为三协议格局
(OpenAI Chat / Anthropic Messages / OpenAI Responses),渠道亲和的本质 =
三个协议码器 + 每厂商一张能力描述符,不是 7 套接入代码。

## 2. 决策

### 决策 1:网关落位 L10 独立 crate `mca-gateway`(第 38 个 crate)

**裁决**:厂商通道是"外部接口",与 chtc-bridge(IDE 适配)、mcp-mesh(MCP 适配)
同级同构。依赖面仅 L0(nexus-contracts)+ L1(event-bus/nexus-core/model-router),
跨层信息全部走 event-bus(C6 铁律)。

**否决方案**:
- L1 model-router 内置通道:L1 被 33/37 crate 依赖,HTTP/SSE/重试等易变逻辑
  放进 L1 污染全系统变更半径;
- 独立 sidecar 进程(类 LiteLLM Proxy):部署复杂度 + QEEP 零孤儿/Critical 事件
  红线需进程内闭环;但 AffinityRequest/Response 保持序列化友好纯契约,
  保留未来抽出为 sidecar 的边界。

### 决策 2:L0 契约扩展(affinity 模块,ADR-033 合规)

`nexus-contracts/src/affinity.rs` 新增纯类型:ProviderId(闭集枚举 + Custom 开放
变体)/ProtocolDialect/CapabilitySet/ModelAffinitySpec/AffinityRequest/
AffinityResponse/ContentBlock 统一块模型。关键约束:

- **零 crate 依赖守恒**:工具入参/出参用原始 JSON 字符串(`Box<str>`)而非
  `serde_json::Value`,解析责任在 L10 Codec(系统边界校验原则);
- **QuirkRule 闭集枚举**:厂商怪癖数据化(TOML 条目),拼错在反序列化即失败,
  新增怪癖变体需 ADR 记录;
- **ThinkingPreference 镜像**:L0 不能依赖 L1 的 `ThinkingMode`,三档语义镜像
  定义,两处互指注释强制同步;
- **价格整数化**:微元/百万 token(u64)+ 峰谷系数整数百分比(u16),
  禁止浮点中间态(sesa-router f32 精度事故红线)。

### 决策 3:VendorAdapter 为 struct 而非 enum(设计文档矛盾裁决)

设计文档 §4.2 目录树写"VendorAdapter enum 分发",代码块却是
`pub struct VendorAdapter`。裁决为 **struct**:spec 驱动零厂商硬编码意味着
只有一个 generic 实现,单变体 enum 是冗余抽象。`Codec` 三方言与 `StreamEvent`
仍为 enum 分发(C4 红线,无 `Box<dyn>`)。

### 决策 4:流式数据面不走 event-bus(TTFT 红线第一防线)

broadcast 容量 1024(bus.rs `DEFAULT_CAPACITY`),per-token delta 流会瞬间打爆
并触发 Lagged 丢弃。裁决:

- **数据面**:`VendorAdapter::stream()` 返回专用 bounded `mpsc::Receiver<StreamEvent>`
  (容量 256)直连调用方(TUI/quest-engine);
- **控制面**:event-bus 只承载 6 个会话级/治理级新事件(ModelAffinitySelected/
  ProviderDegraded/AffinityCapabilityNegotiated/AffinityQuotaExhausted/
  AffinityUnknownField/StreamSessionCompleted);
- `AffinityQuotaExhausted` 为 Critical:**必须同时**列入 `NexusEvent::severity()`
  显式 match 与 `bus.rs::is_critical_mpsc_event()` 两张清单(双清单陷阱,
  只改前者会"标 Critical 但 Lagged 丢失")。

### 决策 5:CircuitBreaker 本地复制而非依赖 chimera-mas

通道级熔断(连续 5 次 5xx → 30s 半开)复用 `chimera-mas/src/stability.rs`
的 AtomicU8 三态 CAS 范式。L10 → L9 依赖虽合法(向下依赖),但为 ~150 行
熔断器拉入整个 L9 crate 破坏 mca-gateway"仅依赖 L0/L1"的 chtc-bridge 同构
约束。裁决:**本地复制 + 两处代码互注镜像关系**,长期可评估上提 nexus-core。

### 决策 6:编译期 feature `mca` 双轨隔离(ADR-034 合规)

- mca-gateway 保持 workspace member 恒编译(`cargo check --workspace` 覆盖);
- 下游 crate 的 mca 集成代码全部 `#[cfg(feature = "mca")]`,默认 off;
- CI 追加 `--features mca` 旁路 job 消除门控盲区;
- feature 转默认 on 为 M4 独立决策 PR(可单独回滚);
- 运行时灰度走 CapabilityToken 四态能力场(ADR-037 既有机制),
  无任何运行时布尔旗。

## 3. 后果

**正面**:
- 新厂商接入零代码(P8 元数据外置,affinity.d/*.toml);
- 三态降级协议(FullFidelity/DegradedNotified/ChannelRejected)使"体验不削弱"
  可验收(E1-E5 不变量);
- 主干恒绿:feature off 时既有 4944 lib 测试零变化。

**负面与缓解**:
- CircuitBreaker 双实现漂移风险 → 镜像注释 + 评审 checklist;
- L0 类型追加触发全 workspace 重编译 → 集中一次合入后冻结 L0 追加;
- SSE 手写状态机维护成本 → fuzz target `sse_parse` + 录播 fixture 回归。

## 4. 实施路线(20 周,里程碑关门制)

| 阶段 | 周 | 交付 | 验收关门 |
|------|----|------|---------|
| M0 契约与骨架 | W1-W4 | L0 类型/6 事件/网关骨架/双 Codec/GLM+DeepSeek 双通道 | 双通道问答+工具调用;38 crate 全绿 |
| M1 七厂商全接 | W5-W8 | 七张 spec 卡/SSE 归一器/session 守恒/Responses Codec | 协议矩阵 7 厂商全绿;E5 哨兵 100% |
| M2 思考/上下文/缓存 | W9-W12 | TTG 映射/HCW 折减/缓存策略/TUI 面板 | E2/E4 达标;缓存命中率有基线 |
| M3 路由与成本治理 | W13-W16 | 路由三元组/s9 学习臂(影子)/成本模型/降级链 | 成本 ≤ 基线 ×0.8;配额切换 E2E |
| M4 议会与体验验收 | W17-W20 | 跨厂商议会/集中度探针/50 任务验收/v2.12.0-omega | E1-E5 全表达标;CODE_WIKI v2.12 |
