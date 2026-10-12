# ADR-159: 审计遗留项系统性修复治理登记(fix-audit-followup)

- **状态**: Accepted(2026-08-28,spec `fix-audit-followup` 批准执行 + WS-5 收口)
- **决策者**: 项目治理(automation spec approval)+ 安全红线纪律
- **关联**: ADR-042(R2 冻结违反处置)、ADR-065~068(MCA 三件)、红线 #8(Top-K 用 select_nth_unstable)、红线 14(R2 冻结关键词)

## 背景

上一轮 `audit-redundancy-deadcode` 审计遗留四项:B1(R2FreezeRollbackFailed 0 生产发布方)、C1(Top-K sort_by 8+ 处违反红线 #8)、E1(~86 孤儿 bench 无门禁)、14 个发布侧幽灵事件;并发现在途规格漂移(FormalVerificationFailed 实为 GsoeError)。

## 决策

1. **B1 冻结违反处置链接线(而非删除)**:Critical 旁路成员禁止删除。实现 `auto-dpo::freeze_guard::attempt_rollback_with_guard`——尽力回滚 pending 更新,回滚失败经 mpsc 旁路真实发布 `R2FreezeRollbackFailed`;回滚成功静默。全链路不触碰 R2 冻结关键词(constrained_rl / r2_policy / train_r2 / GsoeAutoDpoRL / evolve_with_constrained_rl)。事件语义真实(无伪造),AD-042 决策 4 的"自动回滚+Critical 告警"从此有运行时兑现路径。
2. **C1 红线 #8 合规**:新增 `nexus-contracts::util::{xts_top_k, xts_top_k_by}`(L0 零依赖,select_nth_unstable_by 分区 + 尾段 sort,O(n+k log k));替换 7 站 8 处 sort_by 调用点;序列等价性由 proptest 三性质 + sort_by 等价测试守护。
3. **E1 bench 门禁三态**:每个 criterion bench 必须落入 gated / registered / dev-only 三态之一(`check_perf_redlines.ps1` Part 3 完整性门,unknown → FAIL)。bench_check.yml 注册主要 KPI 指涉 bench(阈值待首跑校准);无 spec KPI 者显式 dev-only 登记。**禁止删除既有 bench**(38 crate 零 TODO 蓝线)。
4. **幽灵事件处置**:13 个建立真实生产者(仅在有真实语义的运行路径发布,禁止伪造采集,对齐 §5.6 诚实数据原则);`BenchmarkMetricsCollected` 无自然发布流 → 正式登记**预留(禁删)**。
5. **FormalVerificationFailed 定稿**:确认为 `GsoeError` 变体(非 NexusEvent);从 Critical 事件文档清单剔除并加注;不新增虚假事件面;若未来需事件化须另行立项并注册旁路。
6. **144 变体守恒**:全部接线不动枚举;类型/severity/type_name 一致性由 event-bus 分类与既有断言守护。

## 后果

- 旁路权威口径:Critical mpsc 成员清单 13 个不变(bus.rs is_critical_mpsc_event)。
- 性能:xts_top_k O(n) 取代 O(n log n);bench 门禁三态全量登记。
- 文档:CODE_WIKI §5.7/§5.8 接线与定稿登记;CHANGELOG v2.28.1-omega(在途)条目;本 ADR。
- 风险:bench_check 阈值为首跑校准值(宽松),CI 首跑后需按实测收紧(另立任务)。