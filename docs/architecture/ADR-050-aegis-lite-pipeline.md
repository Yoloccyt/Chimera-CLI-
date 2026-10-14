# ADR-050: AEGIS-lite 四阶段进化流水线降级设计

> **状态**: Accepted (2026-07-28)
> **决策日期**: 2026-07-28
> **关联 ADR**: ADR-049(方案裁剪总决策)/ ADR-042(R2 冻结)/ ADR-044(RHI-CG 双通道)/ ADR-046(ImmuneSystem)
> **关联文档**: `chimera_ultimate_polish_v2.7.md` §9(小米 AEGIS 四阶段引擎)
> **权威源**: 本 ADR 记录 AEGIS 在 R2 冻结约束下的降级落地方式

---

## 1. 背景

方案 §9.1 提出 AEGIS(Digester→Planner→Evolver→Critic)四阶段进化引擎,原设计依赖
MetaModel(LLM 根因分析)与 CodeGenerator(LLM 代码生成),且隐含 RL 参数更新路径,
与 ADR-042 R2 冻结直接冲突。既有 gsoe-evolution 已具备 80% 同构能力:
CiGate(烟雾/回归门)、SignificanceDetector(p<0.05 防误杀)、SpecRegistry(谱系+回滚)、
SpecLoader(ImmutableSurface 守护)。

## 2. 决策

### 决策 1:落点与命名

- 落点:`gsoe-evolution/src/aegis/`(mod/digester/planner/evolver/critic 五文件),
  不新建 crate(ADR-049 决策 1)
- 顶层类型命名 `AegisPipeline`(遵循 `*Pipeline`/`*Engine` 模式),
  **全部标识符规避 5 个 R2 扫描关键词**(`constrained_rl`/`r2_policy`/`train_r2`/
  `GsoeAutoDpoRL`/`evolve_with_constrained_rl`)

### 决策 2:四阶段降级映射(无 RL 参数更新)

| 阶段 | 方案原设计 | AEGIS-lite 降级实现 |
|---|---|---|
| Digester | LLM 轨迹摘要 + 聚类 | 纯规则统计聚类:按 `error_kind + error_location` 分桶,频次排序 |
| Planner | MetaModel 根因分析 | 规则表驱动:失败模式 → 适应方向的静态映射(超时→放宽重试;验证失败→收紧验证) |
| Evolver | CodeGenerator 生成代码 | **仅生成 HarnessSpec TOML 变体**(修改 RetryPolicy 等可进化面),不生成代码;`spec.validate()` + `meta.immutable` 守护 |
| Critic | 神经评估 + 回归测试 | 复用 `CiGate::execute`(烟雾/回归)+ `SignificanceDetector`(误杀防护)+ 奖励欺骗启发式(变异幅度上限) |

### 决策 3:输入类型本地定义

`TrajectoryOutcome`(轨迹结局摘要)在 aegis 模块本地定义,不依赖 L9 quest-engine
类型(依赖铁律:L5 不得向上依赖)。上层(chimera-mas/efficiency-monitor)负责将
Quest 轨迹转换为 `TrajectoryOutcome` 投喂。

### 决策 4:奖励欺骗启发式(Critic)

变体候选相对基线的变异幅度硬上限:`max_attempts ≤ 3× 基线且 ≤ 20`、
`backoff_ms ∈ [100, 60_000]`。超限即拒绝——防止进化器通过极端重试参数
"游戏化"成功率指标(对应方案 §15.2 奖励欺骗检测的规则化替身)。

### 决策 5:谱系与回滚

被 Critic 接受的变体经 `SpecRegistry::register` 登记(parent 链自动维护),
候选灰度走 `set_candidate` → `promote_candidate`,回滚走既有 `rollback()`
一键回滚——AEGIS 不新增任何回滚机制。

## 3. R2 冻结合规声明

- AEGIS-lite 全程**无梯度更新、无策略网络、无约束 RL**;
  所有"学习"均为规则表与统计计数
- gsoe-evolution 源码继续通过 `r2_freeze_violation_e2e` CI 关键词扫描
- 未来 R2 解冻(ADR-047 FormalVerifier 落地)后,Planner 可升级为
  学习驱动,须新 ADR 评审

## 4. 否决方案

| 方案 | 否决理由 |
|---|---|
| 照抄方案 §9.1(MetaModel/CodeGenerator) | 依赖 LLM 在线调用与 RL 更新,违反 ADR-042;CodeGenerator 生成代码的变体属自修改代码,风险不可控 |
| aegis-engine 独立 crate | ADR-049 决策 1 已否决(crate 爆炸);且与 gsoe-evolution 共享 CiGate/SpecRegistry,拆分徒增依赖边 |
| Evolver 直接修改 HopSpec 拓扑 | 流程拓扑变异的回归面远大于重试参数,首期仅开放 RetryPolicy 可进化面,拓扑变异推迟 |

## 5. 验收标准

1. 四阶段流水线单测全绿,聚类/规则表/变异边界/否决路径全覆盖
2. Critic 对超限变体 100% 拒绝(奖励欺骗启发式生效)
3. `r2_freeze_violation_e2e` 持续全绿
4. 接受的变体可经 SpecRegistry 登记且 lineage 正确
