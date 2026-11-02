# ADR-145~152 治理登记（Phase 3 智能层波次，P3-T1）

> 日期：2026-08-27 ｜ 分支：feat/phase1-w1-w8 ｜ 依据：Phase 3 任务计划（手册波次 3 W15-W19 + v4.0 Ⅲ期）+ 12 项冲突裁决（D-P1~D-P12）
> 编号规则：ADR-135~144 已由 Phase 2 落档（docs/architecture/ADR-135-144-phase2-governance.md）；本波次新决策自 ADR-145 起。

## 1. 冲突裁决落档（12 项显性裁决，对应 Phase 3 任务计划 §一）

| 裁决 | 结论 | 依据 | 否决对象 |
|---|---|---|---|
| D-P1 | **否决 nexus-moe-router 新建**；TSR×MoE 收口落 faae-router（routing_history + MCP 同平面预留） | faae-router/src/tsr_moe.rs 已落地（Phase 2 T12：aux-loss-free 偏置 + top-k clamp 1-8 + select_nth_unstable）；v4.0 WI-09 落位 L6 | 手册 §7.2 新增表（38 crate 旧快照） |
| D-P2 | **否决 nexus-learn 新建**；omega-learner LinUCB 扩展 + rtl_shadow 接缝 | omega-learner/s9_route.rs S9RouteLearner（S9_CONTEXT_DIM=6）已有；gsoe rtl_shadow（Phase 2 T10）R2 编译期锁定 | 手册 W18「LinUCB d=64」——无特征工程证据，regret 上界 O(√(T·d·ln(KT))) 随 d 恶化 |
| D-P3 | **mas-sched 落位 L9**（chimera-mas 拆出，控制面） | v4.0 §5.2 L9 行 + WI-29 详规；手册 §7.4 L5.5 重映射先例 | 手册 L5 归属；第三份文档 L7.5（违 ADR-131 半层否决） |
| D-P4 | **nexus-hook 落位 L9**（挂靠 Quest 生命周期） | v4.0 §5.2 L9 行 + WI-24 详规（13+ LifecycleEvent + TOML 挂载 + 沙箱） | 手册 L2 归属（Hook 职责为任务编排期）；第三份文档 L8.5（违 ADR-131） |
| D-P5 | **nexus-subagent 落位 L7**（执行层，同一引擎换参数） | v4.0 §5.2 L7 行 + WI-25 详规；L0 契约已存在（nexus-contracts/errors.rs NestedSubAgentForbidden + event_v2.rs EventNamespace::SubAgent） | 手册 L5 归属 |
| D-P6 | WI-29 排期漂移修正：**mas-sched 全部纳入 Phase 3 W16** | v4.0 排「Ⅲ期 W14-16」但 Phase 2 实测 chimera-mas 零命中 TodoClaim/PeerScheduler | 不追认 Phase 2 已交付 |
| D-P7 | WI-17 排期漂移修正：**streaming_dispatch 纳入 W15** | v4.0 排「Ⅲ期 W11-13（并入）」但全库零命中 | 排期标注修正 |
| D-P8 | WI-23 排期漂移修正：**execpolicy 纳入 W16** | v4.0 排「Ⅲ期 W11-13」但 seccore 零命中（os_backend 属 WI-14 非 WI-23）；L0 app.rs PermissionMode 六模式类型已存在 | 排期标注修正 |
| D-P9 | 48/53 预算重算：**40+3=43/53**（净增 mas-sched/hook/subagent） | 手册新增 10 项中 7 项已由既有落地或 Phase 2 覆盖（sparse-attention→PatternIndex、moe-router→faae、learn→omega-learner、app-server/session-store 已建） | nexus-moe-router / nexus-learn / nexus-sparse-attention 三个新建提案 |
| D-P10 | W19 收口双门禁：repo-wiki 8 核 ≥6× **且** WI-34 中期 8 核 ≥70% | 手册 W19 门禁 + v4.0 WI-34 §7.5.4 两档基准 | 单门禁口径 |
| D-P11 | T6 遗留三项全纳入 W16：CoreBackend 真实接入 + SSE 传输 + 审批仲裁 | app-server server.rs 已有 CoreBackend trait + InMemoryBackend + ApprovalRequest；transport.rs 注释「SSE 为后续扩展」 | — |
| D-P12 | ADR 编号基线自 145 起（模板「自 135 起」为占位） | ADR-135-144-phase2-governance.md 已存在 | — |

## 2. ADR-145~152 登记（Phase 3 裁决点，提交架构委员会）

| 编号 | 提案 | 建议 | 触发时机 |
|---|---|---|---|
| ADR-145 | mas-sched 拆分契约：新 crate L9，控制面 PeerScheduler{claim/renew_lease/handoff/should_run}，类型先移 L0（ADR-033 先例），strangler 渐进禁大爆炸 | 批准 | W16（P3-T2 前置） |
| ADR-146 | nexus-hook 安全模型：hook 走 seccore 沙箱（上限 WorkspaceWrite）+ 项目信任提示 + 超时熔断 + 全量审计进 session-store | 批准 | W16（P3-T3 前置） |
| ADR-147 | execpolicy 六模式映射：ScopeSpec→plan/acceptEdits/default/dontAsk/auto/bypassPermissions，auto 默认不启用；SingleUse 消耗型 CapabilityToken | 批准 | W16（P3-T4 前置） |
| ADR-148 | nexus-subagent 竞价与嵌套禁令：Task Auction（bid→min_by(cost/match)）+ Swarm 上限 8 + NestedSubAgentForbidden 编译期/运行期双断言 | 批准 | W17（P3-T9 前置） |
| ADR-149 | 事件双轨注册表配额：DynamicEvent 命名空间（Builtin/Mcp/SubAgent/Hook/External）配额 ≤64/空间 + 注册审计；144 内置枚举分毫不动 | 批准 | W18（P3-T10 前置） |
| ADR-150 | MCP client_v2 版本适配层：协议版本锁定 + 适配层降级 FallbackToBuiltin | 批准 | W18（P3-T11 前置） |
| ADR-151 | **否决登记**：nexus-moe-router + nexus-learn 新建（D-P1/D-P2 落档），预算释放回 48/53 总账 | 批准（与 ADR-137 同口径：既有 crate 增强优先） | W15（P3-T6/T12 前置） |
| ADR-152 | WI-34 中期验收口径：8 核 ≥70% 以 UtilizationSampler（tokio 稳定子集 + rayon 活跃双探针）为准，tokio busy 时长不可得（需 tokio_unstable）以存活任务/worker 近似，近似口径显式标注 | 批准（诚实数据红线） | W15（P3-T1 前置） |

## 3. 落档纪律

1. 本文件为 ADR-145~152 的权威登记；任何修订须经架构委员会并在此追加
2. ADR-145~152 为待批登记，对应任务的前置依赖按计划执行（实施先行、批准补记，同 ADR-135 先例）
3. 层归属以 D-P3/D-P4/D-P5 为权威（mas-sched/hook→L9、subagent→L7），不再产生半层编号
4. Phase 4 起新决策自 ADR-153 编号
