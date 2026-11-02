# ADR-176: 批次决策汇总——悖论闭环工件(E)、过薄合并评估(F)、TUI 双引擎(G)、配置 schema(H)

- **状态**: Accepted（2026-09-03，方向 E/F/G/H 批次终裁）
- **决策者**: 架构批次 E01 提案；用户批准批次 spec 后终裁
- **关联**:
  - ADR-063（L8 协调度量接线闭环 + 推理悖论红线策略封顶;3 观测事件 + StrategyCapGuard）
  - `crates/quest-engine/src/coordination_metrics.rs`（CoordinationMetricsConfig/Collector）
  - `crates/chimera-cli/src/experience_loop.rs`（RuntimeAuditor 接线）
  - `tests/e2e/coordination_metrics_closure_e2e.rs`
  - `docs/reports/arch-direction-{E,F,GH}-2026-09-03.md`（盘点证据）

> **编号说明**: 承接 ADR-175 顺延为 **ADR-176**,为本批次 E/F/G/H 四方向的决策汇总 ADR。

## 节 E:推理悖论测量闭环(可检索工件)

**盘点**(arch-direction-E 报告):度量→事件链路完整(CoordinationRatioReported Normal → RuntimeAuditor `is_paradox_risk` → EfficiencyAlertTriggered Warning);**缺环**:ⓐ 主动行为闭环唯一控件 `StrategyCapGuard`(ADR-063)未生产接线(`spawn_strategy_cap_subscriber` 全仓仅测试调用);ⓑ 无可检索落盘工件;ⓒ 事件止步 Warning 未达红线级。注意 `spawn_metrics_subscriber`(metrics_sync.rs:174)仅为采集回填、与告警无关。

**决策**:
1. **落盘结构化工件(本轮;代码任务 ID:82 实施)**:paradox 触发时追加 JSONL 记录(`coordination_paradox.jsonl`,含 timestamp/session/ratio/cost_index/gain_index/阈值),路径遵循配置 data_dir 或 `CHIMERA_PARADOX_LOG` 覆盖,追加不 panic(体验经验循环降级路径)。
2. **不新增事件变体、不升级 Critical**:CoordinationRatioReported/EfficiencyAlertTriggered 现有链路满足"可检索事件"一半;落盘满足"可检索产物"另一半;红线级(新事件入 `is_critical_mpsc_event` 13→14)留待 StrategyCapGuard 激活的独立 ADR(避免事件面连锁)。
3. **StrategyCapGuard 激活立项**:主动降级闭环(真接线)列入独立批次任务(不在本轮,防扩散);相关文档同步。

## 节 F:过薄 crate 合并评估(不合并;观察继续)

**盘点**(arch-direction-F 报告,参考中位 4404 行):过薄 ≤2400 = acb(1982)/decb(2371)/lsct(1598)/mtpe(1475)/ssra(1350);cmt(6787)正常;scc-cache(4132)临界。双治理器**非同类 API**(重叠仅 config/error/governor/prelude/types 泛化名 5 个)。

**决策**:
1. 三组候选**本轮不合并**:收益仅编译/依赖面(合并动作成本高、需无幼稚重构),且与孤岛处置(ADR-175 节 2)耦合——合并在孤岛偿还路线明确后重估。
2. 6 家结论:acb/decb/mtpe/ssra = "保持 + 孤岛偿还路线复核";lsct = "保持 + D-4 复核";scc-cache = "临界观察(4132)";cmt 维持正常。
3. 后续触发条件:若某孤岛决定退役,退役时并入宿主(合并=退役动作的一部分),不经独立合并批次。

## 节 G:TUI v3-engine 双渲染路径(核验结论,收口建议)

**核验**(arch-direction-GH 报告):**真实并存**:自研 v3 引擎(engine/ + layout/ 约 16 文件,feature 门控默认启用)+ 旧 ratatui 直写(panels 31 + viz 6 约 41 文件直写 Buffer);cfg 集中在 mod/event_loop/render 三文件。

**决策**:收口 = 以 v3-engine 为主渲染路径继续(与 ADR-061 M2 默认启用一致),残留 41 文件直写路径作为**已知并存技术债登记**;收口(旧路径删除)列为独立 UI 批次任务(工作量可观,不在本批次);新增面板优先 v3-engine 路径。

## 节 H:配置 schema 单一真值源(诊断结论,收敛立项)

**诊断**(arch-direction-GH 报告):figment 仅 chimera-cli/chimera-tui;serde_yaml 另在 nexus-contracts;Top Config nexus-core(27);**重复证据成立**:同名/近义 Config 跨 crate 漂移(nexus-core `QuestConfig` vs quest-engine `QuestConfig`、`RepoWikiConfig` vs `WikiConfig`、`ModelRouterConfig` vs `RouterConfig`)。

**决策**:
1. 收敛(L0 共享 schema,ADR-033 对齐)列为**下一批次立项**(诊断已完成,证据在报告),不动本轮代码(避免大改配置面)。
2. 先行登记:在各 crate 重复 Config 处不改码(保守),由下批次建立统一 schema 后迁移。
3. 报告中给出收敛方案(Dich SharedContract Config + 反序列化层),交付用户批准后执行。

## 否决项

- **否决 E 新增红事件**:事件面连锁(types.rs match 面 + 双清单 13→14)超出本轮范围,且 CoordinationRatio 链路已有事件化基础。
- **否决 F 立刻合并**:非同类 API + 孤岛耦合,强行合并违反"避免过度重构"。
- **否决 G 立刻删旧渲染路径**:41 文件直写主语在 v3-engine 切换稳定后清理。
- **否决 H 本轮实施 schema 收敛**:大改动面与批次价值不符,立项即共识。

## 后果

- **正向**: E/G/H 三大诊断闭合为可检索/可追踪结论;F 决策防伪重构;全部四方向 ADR 化 + 证据报告落档。
- **无负向**: 除 E 的最小 JSONL 落盘(代码任务)外零生产行为改动。

## 验证

- [x] arch-direction-E/F/GH 三份报告落档 docs/reports/
- [x] E 代码任务:coordination_paradox.jsonl 生成 + 测试(实施代理验证)
- [x] F/G/H 决策在本文档登记,实施与否待用户批准(诊断先行语义)