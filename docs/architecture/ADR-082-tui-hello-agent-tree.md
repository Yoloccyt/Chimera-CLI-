# ADR-082:TuiHello 协议握手与 AgentTree 谱系面板(Concord W10 波8·编排)

- **状态**:Accepted(W10 波次交付,2026-08-12 门槛全绿)

> ⚠ **实施状态校正（2026-09-22 现测）**：本 ADR 两半中 **TuiHello 握手 ✅ 已落地**
> （`app/event_loop.rs`、`data/snapshot.rs`、`data/sync.rs` 均有实现与调用）；
> 而 **AgentTree 面板全库 0 命中**（`AgentTree`/`agent_tree`/`DelegationTree` 皆无，`PanelId` 27 变体无此项）
> ⇒ 本波次实为**半交付**，委托树可视化 = **待实现**。
- **日期**:2026-08-12(对应实施波次 2026-08-12 W10)
- **决策发起方**:Concord 重构方案 v2.0(《Chimera_CLI_TUI重构方案_v2.0.md》§7)
- **关联**:ADR-026(chimera-mas Agent 事件族)、ADR-072(会话桥会话级总线先例)、
  方案 §12.3 N9/N10 组合创新
- **影响架构层**:L1 Core(event-bus +2 变体)+ L10 Interface(chimera-tui/chimera-cli)

## 背景(Context)

Codex #37536(2026-08-08)真实事故:陈旧 app-server 存活时新 TUI 静默跑旧后端,
multi_agent_v2 功能静默缺失。Chimera TUI 与编排器经 EventBus 解耦,远程化后
同构风险成立——M4 通道无版本概念,版本偏移只能靠用户发现功能缺失。
多智能体侧:TUI 只有 Agent 列表类视图,无编排谱系(puppeteer 编排语义,
NeurIPS 2025),委托层级关系不可观测。

## 决策(Decision)

### 一、TuiHello 协议握手(方案 §7.1)

1. **事件载体**:NexusEvent 新增 `TuiHello{proto,tui_version,caps}` /
   `TuiHelloAck{proto,compat,server_version}`(132→134,权威口径当时 136,现 144),复用既有 EventBus
   通道(否决进程外 RPC,方案 §12.4);`CompatLevel{Full, Degraded(Vec),
   Refused}` 入 payloads.rs,serde 支持 MessagePack 往返(to_vec_named 形态,
   相邻标签枚举约束)。
2. **协商规则**(`chimera-cli::handshake::negotiate` 纯函数):任一侧版本
   不可解析 → Refused;主版本不同 → Refused;客户端次版本高于服务端 →
   Degraded(携降级项);其余 → Full。协议版本两端各自声明
   (TUI_PROTO_VERSION = "1.0.0")。
3. **SEC-4 一次性握手**:应答器 AtomicBool 记录已应答,首个 TuiHello 后到达
   的握手帧仅留 warn 审计日志后丢弃;TUI 侧 HandshakeSync 同样仅接受首个
   Ack(防伪造握手帧篡改已建立兼容状态)。
4. **门控与横幅(T10.3)**:Ack 经管道 HandshakeSync 入快照 `handshake`
   字段;app 按状态变更刷横幅(Full=Info / Degraded=Warning 列降级项 /
   Refused=Warning);Refused 时 `DispatchPlan::LegacyFallback` 路径只读化
   (诚实反馈"编排命令不可用"),instant/agent 层不受影响;未握手(None)
   乐观放行,兼容无总线测试/桩场景。

### 二、AgentTree 谱系面板(方案 §7.2)

5. **第 26 注册面板**:`PanelId::AgentTree` 接入 REGISTERED_FOCUS_ORDER
   单一事实源(INV-F 自动覆盖)+ make_panel 工厂分支。
6. **AgentTreeSync**(`data/sync.rs`):消费 AgentTaskDelegated/Completed/
   Failed 维护委托边(去重保序)+ 节点统计(委派出/完成/失败/最近任务);
   谱系快照入 DataSnapshot.agent_tree。
7. **渲染与钻取**:DFS 自根展开(无入边即根=编排者;纯环节点二次遍历
   诚实展开,visited 防环);Enter 钻取节点详情(PopupKind::Detail:
   父边/子边/统计/最近任务),对齐 panel.drill_down 既有规则。
8. **线程级运行时标注**:节点标签携带 completed/failed/delegated 统计
   (Codex multi_agent_v2 "runtime choice per thread" 的诚实降维——
   当前事件族无运行时字段,不伪造)。

### 备选与否决

| 备选 | 否决理由 |
|---|---|
| 进程外 RPC 握手协议 | 过度工程;两个事件变体足够(方案 §12.4) |
| 握手走 Critical mpsc 旁路 | 一次性信道建立事件,丢失可由未知兼容乐观降级兜底,Info 即可 |
| Refused 时全命令禁用 | 客户端能力(instant/agent)与服务端编排能力正交,只读化仅限编排 |
| AgentTree 新增 PopupKind | Detail 弹窗足够,不膨胀 PopupKind 枚举 |
| 面板直接消费 event-bus | 违反快照驱动范式;AgentTreeSync 入管道与其余 25 面板同构 |

## 结果(Consequences)

- **正确性**:event-bus 握手测试 7 测(双表 type_name/severity 同步 +
  MessagePack 往返三态 + topic System + proptest 字段守恒);
  chimera-cli 应答器 6 测(协商矩阵 + SEC-4 一次性);E2E 5 测
  (Full/Degraded/Refused 三态回环 + 伪造握手防篡改 + Refused 门控);
  AgentTreeSync 4 测 + 面板 5 测(DFS/防环/钻取内容)。
- **零静默验收**(方案 §7.1):偏移场景(客户端 1.3.0 vs 服务端 1.0.0)
  显式 Degraded 携降级项;主版本断裂显式 Refused + 横幅 + 只读门控。
- **回归**:NexusEvent 132→134 全 workspace 编译/测试通过(权威口径当时 136,现 144,types.rs 枚举 + metadata() 双重验证);焦点环
  INV-F/INV-K 派生自单一事实源自动覆盖第 26 面板(环边断言同步更新)。

## 风险与回滚

- **R15 握手误伤**(正常版本组合被判 Refused):握手降级为"仅警告不阻断"
  (门控条件集中于单一 matches! 分支,移除即回退),修复后恢复三态。
- **信道满/应答丢失**:TUI 未收 Ack 按未知兼容乐观处理,功能不阻断;
  横幅仅在状态确立时出现,不产生噪声。
- **Agent 事件族演进**:AgentTreeSync 仅消费既有三变体,新增 Agent 事件
  变体时需同步扩展 apply_event(match 分支显式,编译器不强制——列入
  事件族扩展检查清单)。
