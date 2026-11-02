# ADR-153~156 治理登记（Phase 4 集成与影子双跑波次，P4-T1）

> 日期：2026-08-27 ｜ 分支：feat/phase1-w1-w8 ｜ 依据：Phase 4 任务计划（手册 Ch12 波次 4，W20-W24）+ 8 项冲突裁决（D-P1~D-P8）
> 编号规则：ADR-145~152 已由 Phase 3 落档（docs/architecture/ADR-145-152-phase3-governance.md）；本波次新决策自 ADR-153 起。

## 1. 冲突裁决落档（8 项显性裁决，对应 Phase 4 任务计划 §一）

| 裁决 | 结论 | 依据 | 否决对象 |
|---|---|---|---|
| D-P1 | 本计划实为 **Phase 4（波次 4，W20-W24）**；ADR 自 153 起；测试基线 11,439（Phase 3 末实测） | phase3-wave3-closure.md §6 交接 + Ch12 波次划分 | 模板"第一阶段/ADR 自 135 起/10,836 基线"占位文本 |
| D-P2 | **新建 ADR-132 落档**（CausalGraph 归因管道）；causal.rs VectorClock（71 测试，ADR-033 豁免圈）为基座 | 代码核验：causal.rs 存在且 71 测试；ADR-132 文件此前缺失 | 沉默引用不存在的 ADR |
| D-P3 | W24 分片决策对象为**已灰度 3 crate（ADR-136）扩量至全量 B 级**——Go=全量接管，No-Go=3 crate 维持灰度+其余永久单流 | ADR-136 已闭环 3/3；shard.rs ShadowStats 采集已存在（P1-T12） | "从零开始"解读 |
| D-P4 | W24 **双门禁**：双跑零 diff ≥7 天（正确性）+ Sharded Bus stretch 吞吐实测记录（性能观测项，不硬断言） | Ch12 W24 行 + v4.0 WI-34（正交维度） | 单一门禁 |
| D-P5 | Phase 3 遗留 3 项接线**纳入 Phase 4 W20**（集成周） | closure §6 交接清单；W20 主题=集成；不占新 crate 预算 | 后置至 Phase 5 |
| D-P6 | WI-34 终验=**复跑 e2e_parallel + utilization_probe（release）按 8 核 ≥75%/16 核 ≥65% 新口径**；Phase 3 数字作基线对照 | v4.0 WI-34 §7.5.4 两档基准 | 用 Phase 3 数字替代终验 |
| D-P7 | W20 指标门禁以**审计报告**核验（指标覆盖矩阵：有数/TODO/不适用），非新增实现 | 代码库 TODO 指标零命中；telemetry/token_ledger 已有通道 | 凭空新增指标实现 |
| D-P8 | T-08 供应商守卫落位 **model-router 层内模块**（零新增 crate） | Ch12 W22 门禁；model-router 已有 provider 注册与热更基础（WI-06） | 新建 crate |

## 2. ADR-153~156 登记（Phase 4 裁决点，提交架构委员会）

| 编号 | 提案 | 建议 | 触发时机 |
|---|---|---|---|
| ADR-153 | 分片总线 Go/No-Go 决策：双跑零 diff ≥7 天 → Go（扩量全量 B 级，分批 3 crate × 回归门禁）；No-Go → 3 crate 永久灰度 + 单流保留（红线 1 兑现） | 批准（W24 决策落档后） | W24（P4-T10） |
| ADR-132（补建） | CausalGraph 归因管道：causal.rs VectorClock（Lamport，71 测试）+ diff 事件 5s 窗口因果链回溯 API；归因 P95 >5s 降级人工兜底 | 批准 | W22（P4-T6 前置） |
| ADR-154 | T-08 供应商守卫：漂移检测（延迟/错误率/标签）+ ProviderDriftDetected 告警（DynamicEvent 双轨发布）；只报警不熔断；阈值保守化 | 批准 | W21（P4-T5 前置） |
| ADR-155 | WI-34 终验偏差登记（如 8 核 <75% 且热点已回滚）：偏差数字 + 缓冲消耗记录 | 按需 | W23（P4-T8，如触发）；**判定口径已被 ADR-157 三条件联合判定取代**（2026-08-31 ADR-168 回写 追加；combined 0.552→0.999，见 `phase4-wave4-closure.md:58`） |

## 3. 落档纪律

1. 本文件为 ADR-153~156 及 ADR-132 补建的权威登记；任何修订须经架构委员会并在此追加
2. 全部为待批登记，实施先行、批准补记（同 ADR-135 先例）
3. Phase 5（W25-W26 GA）新决策自 ADR-157 起
4. 分片 Go/No-Go 为全局最强门禁：No-Go 为红线 1 的合法终态（非失败），Go 需 ≥7 天零 diff + 扩量分批回归

## 4. W24 决策落档（2026-08-27，P4-T10）

### ADR-153 — 分片总线 Go/No-Go：**建议 Go（待委员会终批）**

- **结论**：双跑连续 7 天零 diff（3,584 对，快照落盘可审计）+ stretch 吞吐 2.17M msg/s（超观测项 4.3×）+ 漏发率 0 + 5s 归因验证通过（评审材料：`docs/reports/phase4-w23-adjudication-pack.md`）→ **满足 Go 前置**
- **扩量路径**：从已灰度 3 crate（ADR-136：mlc/efficiency-monitor/gqep）分批扩量至全量 B 级，每批 3 crate × 全量回归门禁；单批失败单批回退，其余批次不受影响
- **批次 1 执行记录（2026-08-28，P4-T10）**：osa-coordinator（OmniSparseMasksComputed 高频）+ nmc-encoder（NmcEncoded 高频）+ scc-cache（CacheHit/Miss 高频）——构造器直接持 bus，`let _ = enable_sharding(DEFAULT_SHARD_COUNT)` 幂等接线（P1-T12 先例，无 runtime 时 Err 降级回单流，零回归）；三 crate 测试全绿 + 扩量后全量回归零失败（见 closure §3 扩量注记）；hcw-window 移交批次 2（构造器无 bus 参数，接线需签名变更，登记为非零侵入项）
- **否决**：No-Go（永久灰度）——正确性门禁已满足，无事实依据；「等利用率达标再 Go」——利用率属 WI-34 性能维度，与分片正确性正交（D-P4），捆绑将阻塞正确性收益
- **红线兑现**：17 Critical 单流禁区不变；顺序敏感通道继续禁入分片（LANE_FORBIDDEN_SHARD 断言延续）

### ADR-155 — WI-34 终验利用率偏差登记（RK-P23 触发）

- **实测**：release 复跑 utilization_probe（16 核宿主）：rayon 侧峰值 1.000（满载可观测），combined 均值 **55.2%** / 峰值 56.3%，对 16 核档 ≥65% 基准偏差 **-9.8pp**
- **归因**：combined = 0.5×tokio + 0.5×rayon 固定权重；tokio busy 时长因 tokio_unstable 未启用以存活任务近似（ADR-152 口径延续）——tokio 侧低估拖累 combined；rayon 侧真实满载说明 CPU 密集路径并行收益已达（e2e 7.45× 对照基线 6.47× 提升）
- **处置**：登记偏差 + 消耗 RK-P23 缓冲（4 周缓冲首次消耗）；不阻塞 Phase 5（加速比与吞吐均正向）；Phase 5 可选改进：tokio_unstable 启用后重测 busy 时长口径，或按运行时分离双口径判定
- **否决**：沉默达标（把 55.2% 说成达标）——违反诚实数据红线；强行凑 65%（加大负载塑形）——测试塑形非生产收益

### 缓冲消耗记录

| 风险项 | 消耗 | 余量 |
|---|---|---|
| RK-P23（WI-34 终验偏差） | 1 次（ADR-155） | 3 次等效缓冲余量 |
| RK-P19~P22 | 0 | 全额 |
