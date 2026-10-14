# ADR-180: 批次2 收口——telemetry 埋点决策 与 批次外方向清单

- **状态**: Accepted（2026-09-03，架构重构批次2 收口决策）
- **决策者**: 架构批次 E01 提案；用户批准批次 spec 后终裁
- **关联**:
  - ADR-174（观测面维持可移植遥测子集,导出器冻结,`<5% CPU` 门禁）
  - ADR-176 G/H（TUI 双引擎 / 配置 schema 立项）
  - `crates/event-bus/src/telemetry.rs`（Telemetry::start_span/end_span + AtomicU64 直方图）
  - `docs/reports/arch-direction-GH-diagnostics-2026-09-03.md` / `arch-batch2-new-directions-2026-09-03.md`〔⚠ 2026-09-22 实测**此名不在盘**；同批次现存件为 `docs/reports/arch-refactor-batch2-2026-09-03.md`，是否同一文档待治理确认〕

> **编号说明**: 承接 ADR-179 顺延为 **ADR-180**。

## 节 1:telemetry 最小埋点决策(维持"待接线")

批次2 Task 5 评估(勘察证据):
- `Telemetry::start_span/end_span` 生产引用 = **0**(仅 telemetry.rs 自身与测试)。
- TurnSpan 规格为"每 Agent Turn 一 Span";而上批(ADR-172)已确证**生产无真实 LLM/Agent Turn 触发点**(模型调用面=0);Quest 处理为业务事件已由 CoordinationRatioReported 覆盖观测,不构成独立埋点收益。
- 埋点产物(延迟直方图)当前**无任何消费方**(观测面 0 消费,ADR-174 记录)。

**决策**:
1. **维持"待接线"**:本批不落地生产埋点(为埋点而埋点=过度工程;无消费方、无触发点)。
2. **解冻触发条件(登记)**:同时满足——① 真实模型调用面落地(mca-gateway M4 完整激活 + transport 连线,经 ADR-065/177 路径)提供 Agent Turn 语义;② 观测消费方建立(可观测前端/报告/导出器经 ADR-174 门槛);③ `<5% CPU` 门禁有 criterion bench 验证。
3. **优先级注记**:观测埋点属"待接线能力",排于 mca transport 激活之后。

## 节 2:批次外方向清单(登记,不实施)

经批次1(arch-direction-GH)与批次2(arch-batch2-new-directions)探索确认、但**本批不实施**的方向(防过度工程/大工程独立批次):

| # | 方向 | 证据 | 建议批次 |
|---|------|------|---------|
| O1 | 组合根模块化(装配层组件化) | `main.rs:48-160` + `commands/mod.rs` 装配发散,43 crate 手动 new/spawn | 批次3 候选(先量化) |
| O2 | 事件订阅聚合中心 | Critical mpsc 注册(bus.rs:139-178/948-1058)+ 各 crate 独立 spawn 订阅无聚合 | 批次3 候选 |
| O3 | 外部依赖去重评估 | 根 Cargo.toml:83-158 集中声明;cargo audit 6 ignore 沿用 | 周期审计项 |
| O4 | test-utils dev-dep 深化 | scaled_timeout!/locale_test_guard/断言 helper 跨 crate 复用现状(部分已收敛 R10/R12/R14) | 增量收敛 |
| O5 | 配置 schema L0 收敛 | 同名 Config 跨 crate 漂移(QuestConfig/RepoWikiConfig/ModelRouterConfig 等;arch-direction-GH) | 批次3 候选(大,需用户批准立项) |
| O6 | TUI 旧渲染路径清理 | v3-engine 为主,旧 ratatui 直写 41 文件并存(ADR-176 G) | 独立 UI 批次 |
| O7 | CI/发布架构优化 | release.yml 门禁步骤、cargo audit 口径(ADR-166/167) | 按需 |

上述方向以本 ADR + 各证据报告为锚,任何启动须另行 spec/批准(用户终裁)。

## 否决项

- **否决本批强制落地 telemetry 埋点**:无触发点/无消费方/门禁验证成本,收益为负(理由见节 1)。
- **否决本批启动 O5/O6 大工程**:配置 schema 收敛与 TUI 清理均跨多 crate 数十文件,独立批次更利于隔离回归。

## 后果

- **正向**: 观测埋点从"悬空能力"变为"带触发条件的能力";O1-O7 方向清单化可追踪;批次范围收束。
- **无负向**: 零代码改动(本 ADR 为决策+登记)。

## 验证

- [x] `Telemetry` 生产引用=0 复核;候选路径评估完成(worklog Task 5)
- [x] 批次外方向清单(上表)与证据报告对照成立
- [ ] 批次2 终验全绿(见批次 worklog 收口)