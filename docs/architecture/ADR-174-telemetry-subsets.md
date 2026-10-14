# ADR-174: 观测面输出决策——维持可移植遥测子集,冻结 OTLP 导出评估

- **状态**: Accepted（2026-09-03，方向 B 观测面唯一出口盘点终裁）
- **决策者**: 架构批次 E01 提案；用户批准批次 spec 后终裁
- **关联**:
  - ADR-143（否决 nexus-telemetry 新建,event-bus 增强裁决）
  - `crates/event-bus/src/telemetry.rs`（OTel 风格可移植子集:TurnSpan + LatencyHistogram,头注"OTLP 留 Phase 3"）
  - `crates/efficiency-monitor`（业务效能;与 telemetry 分工已写明）
  - `docs/reports/arch-direction-B-telemetry-2026-09-03.md`（盘点证据）

> **编号说明**: 承接 ADR-173 顺延为 **ADR-174**。

## 背景

盘点确证(arch-direction-B 报告):
- **埋点覆盖 = 0**:telemetry.rs 唯一接线是 `event-bus/src/lib.rs:118 pub mod telemetry`;全库无任何生产调用(TurnSpan/LatencyHistogram/start_span 均零命中;仅 model-router/trajectory.rs:67 一条 Jaeger/OTel 提及注释)。4 个单元测试孤悬。
- **Span 数据纯内存、不可检索、不可导出**:`end_span` 后属性即丢弃;OTLP/JSON/Protobuf 导出器**确认不存在**(仅头注"留 Phase 3")。
- 分工成立且未重叠:efficiency-monitor(业务告警,`EfficiencyAlertTriggered` + publish_blocking)vs event-bus telemetry(基础设施 Span);t哪里暂无三路平行。

## 决策

1. **维持"可移植子集"姿态,冻结 Phase 3 导出评估**:不引入 OTLP/opentelemetry 依赖、不新增 JSON/Protobuf 导出器;telemetry.rs 头注的"留 Phase 3 评估"表述**改造为决策记录**(ADR-174),避免"悬空 Phase 3"再被误读为未决策。
2. **导出门槛前置声明**:任何未来导出器必须:① 另立 ADR;② 满足 `<5% CPU 开销门禁`(与 telemetry 原子计数无锁设计一致);③ 提供导出-解析 round-trip 测试。
3. **埋点优先级记录**:当前 0 生产埋点属已知缺口,记入 arch-direction-B 报告;埋点是"待接线能力"而非本轮必做工(避免为埋点而埋点的过度工程)。
4. 观测唯一出口现状 = `EfficiencyAlertTriggered`(业务)+ 未来 telemetry(基础设施),二者名义分工保持;新增观测出口需本 ADR 流程。

## 否决项

- **否决落地 JSON 导出器**:当前 0 埋点,导出器无消费对象,属过度工程。
- **否决引入 OTLP 依赖**:依赖面/二进制体积风险(50MB binary 红线)现值不合算。

## 后果

- **正向**: 悬空 Phase 3 表述终结;导出决策门槛化;未来实现有明确验收。
- **无负向**: 零代码行为改动(仅头注);现有 4 单测保持绿。

## 验证

- [x] `arch-direction-B-telemetry-2026-09-03.md` 已落地(B1 证据表/B4 缺口结论)
- [x] telemetry.rs 头注决策化(实施代理验证)
- [x] `cargo test -p event-bus` 0 failed(实施代理验证)