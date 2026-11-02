# ADR-051: Variant Pool 变体隔离池 + 规则式任务路由

> **状态**: Accepted (2026-07-28)
> **决策日期**: 2026-07-28
> **关联 ADR**: ADR-049(方案裁剪总决策)/ ADR-050(AEGIS-lite)/ ADR-017(Skeptic 否决权)
> **关联文档**: `chimera_ultimate_polish_v2.7.md` §12.3(小米变体隔离与审议)
> **权威源**: 本 ADR 记录变体隔离池与三角色审议的落地方式

---

## 1. 背景

小米 HarnessX 实测:全局单 Harness 在多任务类型下退化 -24.3%(灾难性遗忘),
变体隔离节省 25% Token。方案 §12.3 提出 Variant Parliament(三角色审议 +
Security 一票否决 + 2/3 多数),§12.1 的 MAPPO+SHARP 神经博弈已被 ADR-049 推迟。

## 2. 决策

### 决策 1:落点与类型分工

- 类型:`nexus-contracts/src/variant.rs`(VariantId / VariantContract,L0 纯类型)
- 逻辑:`parliament/src/variant_pool.rs`(池 + 规则路由)+
  `variant_review.rs`(三角色审议),L8 子模块不新建 crate
- VariantId 复用 SpecRegistry 的 `(spec_name, spec_version)` 主键,
  避免双主键漂移;变体本体仍存 SpecRegistry(单一事实源)

### 决策 2:规则式任务路由(非 RL)

`VariantPool::route(task_type)` 匹配顺序:
1. 精确匹配 `task_types` 包含该类型的变体(多个时取 `expected_performance` 最高)
2. 兜底:`task_types` 为空的通用变体
3. 无匹配:返回 None(调用方沿用当前活跃 spec)

R2 冻结约束:路由为确定性规则,无 bandit/RL;未来升级须走 omega-learner 接缝 + 新 ADR。

### 决策 3:三角色审议(复用既有机制)

`VariantReview::review(contract)` 三角色顺序:
1. **Security**(一票否决):候选触碰 `UNLEARNABLE_SECURITY_RULES` 语义
   (由调用方预检传入 `security_concerns`)→ 立即 Reject
2. **Skeptic**:`expected_performance` 低于池内同任务类型现役变体 → 反对票
3. **Execution**:`max_regression` > 0.2(回归容忍上限)→ 反对票

裁决:Security 通过后,Skeptic + Execution 中 ≥1 赞成(与 Security 合计 2/3 多数)即 Approve。

### 决策 4:池容量与淘汰

每任务类型最多 4 个变体(小米变体池经验值);超限时淘汰
`expected_performance` 最低者。全池上限 64,防无界增长。

## 3. 否决方案

| 方案 | 否决理由 |
|---|---|
| MAPPO + SHARP 神经博弈审议(方案 §12.1) | 神经网络训练违反 ADR-042;Shapley 归因随 v3.x MAPPO 一并推迟 |
| 变体池独立 crate | ADR-049 决策 1 已否决;审议与否决机制在 parliament,拆分徒增跨 crate 调用 |
| 池内存储完整 HarnessSpec | 与 SpecRegistry 双份存储,漂移风险;池只存 VariantContract 引用 |

## 4. 验收标准

1. 路由三级匹配(精确/兜底/None)单测全覆盖
2. Security 一票否决路径 100% 拒绝
3. 池容量淘汰确定性(性能最低者出局)
4. E2E:审议否决路径 + 通过路径
