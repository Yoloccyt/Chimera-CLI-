# ADR-321: 投递通道权威统一决策（R5 批：`delivery_lane()` 纯函数化）

- 状态: Accepted（决策批准；施工前置 = R4 已落地 + 在途 L1-B 线落地，见 D5）
- 日期: 2026-11-16
- 批次: L1 层深度重构 R5（第一卷 §5.2 R5 + 第二卷 §5.2 细化）
- 设计源: 第一卷取证（`is_critical_mpsc_event` 手写 18 臂与 registry 宏生成 severity 的「双清单同步」= 设计出来的永久负债，历史两次漂移实例：P1-W2.1.4 / P3-14）
- 关联: ADR-194 D5（I8b 计数覆盖先例）、ADR-314（bus/ 拆分后宏结构现状）、ADR-197（变体序数纪律）

---

## D1 — 病灶与决策

投递通道（broadcast vs Critical mpsc 旁路）本质应是 severity 的纯函数，现状却是第二份人工裁量清单（`crates/event-bus/src/bus/mod.rs::is_critical_mpsc_event`，18 臂手写），与 `crates/event-bus/src/registry.rs::define_event_registry!` 宏生成的 severity 靠互锁测试防漂移（registry.rs:29-35 设计说明自承）。**决策**：通道判定收编进注册表宏，生成唯一权威 `NexusEvent::delivery_lane()`；手写清单进入退役窗。

## D2 — 设计

1. `define_event_registry!` 登记行扩展可选第三列 `!mpsc`（默认=按 severity 推导：Critical→MpscBypass）；宏新增生成
   `pub fn delivery_lane(&self) -> DeliveryLane`（`enum DeliveryLane { Broadcast, MpscBypass }`，置于 `types.rs`）。
2. **宏内编译断言**（fail-closed，I-3 不变量保持）：severity==Critical ∧ 未标 `!mpsc` ⇒ `compile_error!`；标 `!mpsc` ∧ severity!=Critical ⇒ `compile_error!`——差集必须显式。
3. **差集-4 常量登记**：`CheckpointSaved`/`ConsensusReached`/`SlowConsumerDropped`/`OrphanCallDetected`（历史广播级 Critical，I8c 现状注释载明）登记为
   `registry.rs::BROADCAST_CRITICAL: &[&str]`——「投递通道≠severity」的历史事实从注释升格为机器断言；互锁测试反向断言（mpsc 清单 ⊆ severity Critical 且差集恰为该表）。
4. `is_critical_mpsc_event` 改 `matches!(self.delivery_lane(), MpscBypass)` 薄封装 + `#[deprecated(since="2.30.0-omega", note="use delivery_lane()")]`；workspace 调用点 grep 归零证据进 commit message；物理删除留 2.31 窗口。
5. `CRITICAL_MPSC_VARIANTS`/`CRITICAL_TOTAL` 常量保留（I8/I8c 不变式锚点不变）。

## D3 — 验证设计

- 新增 compile_fail 测试：Critical 变体漏标 lane ⇒ 编译错（红→绿 TDD 先行）。
- 现有守卫全绿：`variant_count_test.rs` 三层锁（变体数不变）、`publish_path_equivalence_test.rs`、`I8/I8b/I8c` 门、`bus_bench`/`cross_layer` 阈值门。
- 负例：构造「severity 升 Critical 但漏接旁路」的回归场景（历史上恰为此类漂移），宏内断言在编译期拦截。

## D4 — 破坏面与兼容

新增 pub API `delivery_lane()`（纯新增，无破坏）；`is_critical_mpsc_event` 软破坏（deprecated 一个版本窗）；registry 宏第三列为可选（既有行零改动）。

## D5 — 前置与回滚

- 前置：R4 批（`82fdafb6` 前 `e76b4d44`/`0a43ae78`，bus/ 结构已稳）已落地 ✓；在途 L1-B 线未落地（其脏 `registry.rs`/`bus/mod.rs` 相交——现核：`registry.rs` 不在其脏清单内，仅 `lib.rs`/`attribution.rs` 脏；施工前复核一次）。
- 回滚：单 commit revert；互锁测试保反向断言，revert 后双清单现状自动恢复。
