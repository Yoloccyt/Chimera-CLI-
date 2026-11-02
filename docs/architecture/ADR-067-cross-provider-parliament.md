# ADR-067: 跨厂商级联与去相关议会

> **状态**: Proposed (2026-07-31)
> **决策日期**: 2026-07-31
> **关联 ADR**: ADR-065(MCA 总纲)、ADR-066(能力协商)、ADR-068(成本治理,本 ADR 承接其决策 4)
> **设计源**: `Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.5 跨厂商议会 / §2 病理 D3
> **权威源**: 本 ADR 记录 MCA M4 跨厂商去相关议会与厂商集中度免疫的落地方式

---

## 1. 背景

病理 D3(设计文档 §2):L8 Parliament 的 Skeptic/Security/Execution 角色当前
只能是同一模型的自问自答,辩论存在"同源相关失败"——同一模型的盲区在所有
角色间相关,AHIRT 红队形同虚设。MCA M4 必须把"多角色辩论"升级为"跨模型
对抗",并对"供应商锁定"建立系统性免疫。

## 2. 决策

### 决策 1:跨厂商去相关(provider 侧表方案,R8 规避)

**不动** `RoleProfile` 与 5 角色默认配置(避免构造点雪崩)。provider 绑定走
独立侧表 `ProviderAffinityRegistry { RwLock<HashMap<RoleId, ProviderBinding>> }`
(parliament/provider_affinity.rs),与 `RoleRegistry` 并存(RwLock 读多写少,
bind 低频)。

`ProviderBinding { producer, verifier, skeptic }` 三方厂商,去相关硬约束(P7):

- **Producer(PVL 生产者)与 Verifier(PVL 验证者)默认异厂商**——第二意见
  若与生产者同源,验证形同自我确认;
- **Skeptic 角色强制与提案生产者异厂商**——红队若与生产者同模型,盲区相关,
  AHIRT 形同虚设;
- `validate_cross_provider(binding)`:producer==verifier 或 producer==skeptic
  时返回 `ConfigError`(去相关硬约束);verifier==skeptic 允许(只要都与
  producer 异厂商);
- 未绑定角色向后兼容(`is_decorrelated` 返回 true,未启用跨厂商议会的场景
  不强制)。

### 决策 2:厂商集中度免疫探针(N7,ImmuneSystem 第四探针)

单厂商流量占比 EWMA(α=0.1)> 70% → 发布 `ProviderConcentrationWarning`
(Normal 级,不扩容 Critical 旁路)→ acb-governor 自动拉低该厂商路由权重。
这是对"供应商绑定风险"的系统性免疫:把"单一首选厂商默认配置"定义为
系统级病理并自动纠偏(设计文档 §8.3 否决清单第 6 条的落地)。

数据流:消费 `StreamSessionCompleted.route_key` 累计厂商流量,EWMA 平滑;
均衡分布(7 厂商各 ~14%)不触发。

### 决策 3:跨厂商级联升级判据复用(零新增度量成本)

FrugalGPT 三级级联(廉价提议 → 主力 → 跨厂商确认)的"不自信"升级判据
**复用** Parliament `ConsensusQualityMetrics`(v2.11.0 已有的分歧度/弃权率/
共识裕度五指标)——级联的"廉价档不自信"与议会的"未共识"同源度量,
零新增度量成本(设计文档 §5.4 N5 创新点)。

## 3. 后果

**正面**:多角色辩论从"单模型自问自答"升级为"跨模型对抗",AHIRT 红队
恢复意义;供应商锁定被系统性免疫;级联升级判据零新增度量成本。

**负面与缓解**:
- 侧表与 RoleRegistry 并发一致性 → RwLock 读多写少,bind 低频,validate 高频;
- 跨厂商调用增加一次网络往返(第二意见跨厂商)→ 由级联的"廉价档优先"
  抵消(多数请求在廉价档自洽即采纳,不升级)。

## 4. 落地状态(M4)

- ✅ `ProviderBinding` / `ProviderAffinityRegistry` / `validate_cross_provider`
  (侧表方案,RoleProfile 零改动,6 单测)
- ⏳ ImmuneSystem 第四探针(厂商集中度 EWMA >70% 告警)
- ⏳ 50 任务体验对等验收(E1-E5 矩阵)
