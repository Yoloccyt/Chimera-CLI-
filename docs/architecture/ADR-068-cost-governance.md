# ADR-068: 成本治理与峰谷模型

> **⚠️ RL 开发闸门(2026-08-15 治理决策)**: 现阶段**只做 Rust 侧**;Python 侧(RL 版)训练服务**仅保留规划**(本文档所述协议/方案/规划内容保留为设计资产),Python 服务实体**禁止实施**;待整个 Rust 系统彻底成熟并稳定运行后(R2 解冻 + 稳定性观察期通过)再开启 RL。权威源: `.trae/rules/nuxus规则.md` §1.4 / `AGENTS.md` §1.2 当前焦点。

> **状态**: Proposed (2026-07-31)
> **决策日期**: 2026-07-31
> **关联 ADR**: ADR-065(MCA 总纲)、ADR-066(能力协商)、ADR-037(EWMA α=0.1)、
> ADR-031(omega-learner 边界,s9 路由臂)
> **设计源**: `Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.4/§5.5
> **权威源**: 本 ADR 记录 MCA M3 路由与成本治理的落地方式

---

## 1. 背景

M0-M2 建成契约、网关、协议、能力协商、缓存亲和后,M3 解决"如何路由、
如何控成本"。设计文档病理 D2(model-router 有路由无通道)与 D6(成本治理
无对象)要求把已建成的路由/预算机器接上真实通道与真实 token 计费。

## 2. 决策

### 决策 1:model-router 通道化(RouteTarget 三元组 + 新表)

路由决策单元从"模型名"升级为 `RouteTarget = (ProviderId, model,
ThinkingProfile)`(§5.4)。**最小变更策略**(R3/R8 风险缓解):

- **新增** `RouteTarget` 类型(model-router/route_target.rs),**不动**
  既有 `RoutingDecision`/`ModelInfo`(避免构造点雪崩);
- **新建** `route_history` 表(provider/cost_actual/cache_hit_tokens/
  ttft_ms_ewma 列),与既有 `history` 表(PK=model_id)并存,旧 MoE
  路由策略零改动,双表并存至 M4 验收后再评估归并;
- `RouteTarget::arm_id()` = `provider/model/mode`,是 omega-learner s9
  学习臂与 route_history 存储的统一键。

### 决策 2:omega-learner s9 路由学习臂(影子模式,不触 R2 冻结)

新建 **独立** `s9_route.rs` 接缝(对既有 s1-s8 零改动):

- LinUCB 臂空间 = provider × model × thinking_mode(~40 臂);
- 上下文 6 维(任务复杂度/预算水位/延迟敏感度/缓存命中历史/风险等级/bias);
- 奖励 = success × 质量 − λ₁·成本 − λ₂·延迟(λ₁=0.3,λ₂=0.2,加法惩罚
  符合 LinUCB 线性假设);
- **影子模式合规**(ADR-043):只产出 `SelectorPolicy::Learned` 建议,
  本地 fallback `SelectorPolicy::Static`,胜率 < 71.4% 回滚;
- **不触 R2 冻结面**(ADR-042):s9 不 import gsoe-evolution/auto-dpo,
  无 evolve/RL 训练路径,只做在线 bandit 选择;
- 40 臂决策延迟 p99 < 50μs(`linucb_40arm` benchmark 验证,arm.rs L23
  假设 ~10 臂,40 臂需性能证据)。

### 决策 3:acb-governor 成本模型(EWMA + 否决 + service_tier 授权)

`cost_model.rs` 落地成本治理三职责(P6 成本先行):

1. **成本 EWMA**:每通道预估/实际成本 EWMA(α=0.1,对齐 ADR-037);
   `ModelAffinitySelected.cost_estimate_micro` → record_estimate,
   `StreamSessionCompleted.cost_actual_micro` → record_actual;
2. **预算否决**:acb-governor 有权否决路由——预估成本推预算过 120% 阈值
   即 `CostVerdict::Veto`(调用方回落廉价档级联/换通道);日成本 > 预算
   120% → `BudgetExceeded`(既有 Critical,零新增);
3. **service_tier 授权**:priority 加价档(MiniMax 1.5×)必须 BudgetMask
   显式授权,禁止默认开启(`authorize_service_tier`)。

**微元整数口径**:成本单位微元(1e-6),EWMA 内部 f64(平滑),累计总额
整数(避免浮点累加误差);峰谷/缓存折扣在 mca-gateway 上游(adapters.rs
`actual_cost`)按小时桶查表 + 整数运算计入,acb 只做聚合治理。

### 决策 4:跨厂商级联降级(FrugalGPT 落点,csn-substitutor)

`AffinityQuotaExhausted`(Critical mpsc)→ csn 按能力相似度找替代通道
(CapabilitySet 加权距离)→ 会话状态迁移(session::migrate_history 按新
通道守恒策略转译/安全丢弃思考块并留痕)。升级判据复用 Parliament
`ConsensusQualityMetrics`(零新增度量成本)。

## 3. 后果

**正面**:路由从纯数据结构演练升级为真实通道选择;成本可控可否决;
service_tier 加价不失控;学习路由影子合规上线。

**负面与缓解**:
- route_history 双表并存增加一次归并决策 → M4 验收后评估;
- 40 臂决策延迟风险 → benchmark 门禁,超 50μs 才引入 θ 缓存(证据驱动);
- s9 影子期学习劣化 → ADR-043 胜率 < 71.4% 自动回滚 Static。

## 4. 落地状态(M3)

- ✅ `RouteTarget` 三元组 + `RouteHistoryStore`(route_history 新表,4 新列)
- ✅ `s9_route.rs` 路由学习臂(~40 臂,6 维上下文,影子模式 Learned 建议)
- ✅ `AffinityCostModel`(成本 EWMA + CostVerdict 否决 + service_tier 授权)
- ⏳ csn-substitutor 能力相似度降级链 + 配额切换 E2E(T3.4)
