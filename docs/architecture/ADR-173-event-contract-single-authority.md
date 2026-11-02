# ADR-173: 事件契约明文分工——单表权威 + 轨二冻结标注

- **状态**: Accepted（2026-09-03，方向 C 事件契约双轨盘点终裁）
- **决策者**: 架构批次 E01 提案；用户批准批次 spec 后终裁
- **关联**:
  - ADR-160（event_types.rs 分层镜像退役先例;分类真值源收敛单表）
  - `crates/event-bus/src/types.rs`（144 NexusEvent 单表）
  - `crates/nexus-contracts/src/event_v2.rs`（EventMetadataV2 / DynamicEvent 轨二）
  - `crates/nexus-contracts/src/event_metadata.rs`（payload_version）
  - `docs/reports/arch-direction-C-event-contract-2026-09-03.md`（盘点证据）

> **编号说明**: 承接 ADR-172 顺延为 **ADR-173**。

## 背景

盘点确证(arch-direction-C 报告):
- **`impl DynamicEvent` 实现者 = 3**(生产 2:nexus-hook::HookTriggerEvent `event_bridge.rs:63`、model-router::ProviderDriftEvent `provider_drift.rs:230`;测试 1:event-bus dynamic_registry.rs)。
- **`EventMetadataV2` 生产引用仅上述实现者字段**;`payload_version` 生产代码恒为 1 无分支。
- 单表 144 变体全部使用 **v1 EventMetadata**(`types.rs:2589` metadata() 权威源),不使用 v2。
- 注册表面:`DynamicEventRegistry::new()`/`HookEventBridge::register_all` 全库仅 `#[cfg(test)]` 出现;`DynamicEventRouter`(dynamic_registry.rs:13 注释)为**从未实现的悬空符号**。轨二"已实现、未装配、未激活"。

## 决策

1. **明文分工**:事件契约以 **`event-bus/src/types.rs` NexusEvent 单表为唯一权威演进位置**;轨二(EventMetadataV2/DynamicEvent)定位为**外源动态事件注册扩展契约**,当前**冻结未激活**(不做生产装配、不新增实现者)。
2. **禁止演化歧路**:除现有 2 个生产实现者外,禁止新增 `impl DynamicEvent`;任何解冻必须另立 ADR。
3. **消解悬空符号**:dynamic_registry.rs 中 `DynamicEventRouter` 注释改为"设计意图符号、从未实现、ADR-173 裁定不实现"(实施由批次代码任务完成)。
4. **标注冻结**:event_v2.rs 文件头追加"冻结未激活 + 单表权威"说明(实施由批次代码任务完成)。
5. 不删除轨二代码(4 个受影响测试文件保留):其测试覆盖为既有外源场景回归面。

## 否决项

- **否决单轨化删除(方案 A)**:会推倒 HookTriggerEvent/ProviderDriftEvent/注册表/4 测试文件,破坏外源事件扩展面,收益仅是行数。
- **否决完全平行双轨**:若允许轨二生产装配会形成重复演化路径(重蹈 ADR-160 镜像退役覆辙)。

## 后果

- **正向**: 事件面单一演进权威落地;悬空符号消除;新增事件与新增 DynamicEvent 的边界书面化。
- **无负向**: 行为代码零改动(仅注释/标注);测试全绿。

## 验证

- [x] `arch-direction-C-event-contract-2026-09-03.md` 已落地(C1/C2 计数表)
- [x] 代码任务完成:event_v2.rs 头注 + dynamic_registry.rs 悬空符号注释(见实施验证)
- [x] `cargo test -p event-bus -p nexus-contracts` 0 failed(实施代理验证)