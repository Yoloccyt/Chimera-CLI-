# ADR-258: nexus-hook 生命周期接线（P3 / U-03）

- **状态**：Accepted（2026-11-08，Tranche 2 工单 F；deepdive §8 规格照单执行）
- **上游**：deepdive §8（五元组 + §8.3 接口 + §8.4 TDD + §8.8 决策点）
- **关联**：ADR-244（组合根装配纪律）/ ADR-253（轨二不占变体数）/ ADR-160（GATED 轨道约定）/ ADR-224 D3（契约派生器）

## 1. 背景

`HookEventBridge::register_all`（nexus-hook/src/event_bridge.rs:93-104）函数体
为生产码但调用面全在 cfg(test)——生产调用 0；`DynamicEventRegistry`
（event-bus/src/dynamic_registry.rs:58）EventBus 不持有（「挂 L1」需动结构
——否决）、lib.rs:39 存在「零引用」陈旧注记（deepdive §8.2 指认与现势矛盾）。
P3 = 注册面闭合 + 契约头诚实化。

## 2. 决策

- **D1 · DynamicEventRegistry 生产构造点 = 组合根**（上游方案遗漏项）：
  composition.rs build_with_sharding 内构造 `Arc<DynamicEventRegistry>` +
  `HookEventBridge::register_all(&registry)`（幂等,同 ID 覆盖为 Replaced,
  14 ≤ 配额 64）；AppContext 增 `pub hook_registry` 字段。
- **D2 · 消费面第一波仅「注册 + 触发实例可查」**：`get`/serialize 拉取消费方
  留独立增量——**不为绿造消费者**（U-10 教训）。
- **D3 · register_all 表述订正**：函数体生产码、调用面全在 cfg(test)——
  「去门」实际动作 = 组合根新增生产调用，**非改 event_bridge.rs**（零改动
  证明：nexus-hook event_bridge 既有 4 测试原样通过）。
- **D4 · Hook 失败姿态** = warn + 计数不反噬主流程（N-4 降级姿态先例,
  会话存储批次债项编号）；实测：超时熔断/沙箱拒绝形态审计条目 roundtrip
  仍闭合（重要性 ≥0.9 如实记录）。
- **D5 · 契约头**：`PRODUCERS: 0` **诚实保留**（第一波只闭合注册面,触发
  事件生产待真实 hook 执行批次——区分「注册面闭合」与「触发事件生产」
  两个状态）；`CONSUMERS: 1 chimera-cli`。
- **实施偏差登记（feature 门控）**：deepdive §8.3 为无条件接线——现势
  nexus-hook 为 chimera-cli **optional 依赖**（r2_island_repayment 捆绑,
  GATED 轨道,ADR-160 约定）。接线以 `#[cfg(feature = "r2_island_repayment")]`
  门控（AppContext 字段 + 装配块 + 测试文件）：默认构建零符号/零边,
  **可达性棘轮 38 不变**；无条件提升为默认可达 = 可达性棘轮变更,属独立
  裁决（不在本 ADR）。验收命令相应为
  `cargo test -p chimera-cli --features r2_island_repayment --test hook_wiring`。
- **附带订正**：event-bus lib.rs:39「零引用」陈旧注记修正（nexus-hook 自
  P4-T3 起已引用,注记漂移——deepdive §8.2 指认）。

## 3. 验收（deepdive §8.7 命令套件全绿，2026-11-08）

- chimera-cli hook_wiring **5/5**（组合根注册 14 项 / 幂等 / roundtrip 含
  sandbox_denied+command 字段 / 失败形态不反噬 / 无 spawn 纪律反向断言）
- nexus-hook event_bridge 既有 5 测试**零改动通过**（D3 零改动证明）
- check_event_ownership [OK]（154 不变,hook.* 走轨二）
- check_composition_root [OK]（DynamicEventRegistry::new 不在
  ASSEMBLY_CALL 固定类型清单,组合根 7 点不变）
- clippy（3 crate + feature 面）0 告警 / fmt 收敛 / 双态 check
  （含 feature 与默认）绿

## 4. 回滚

单 commit revert——AppContext 字段 + 组合根接线块 + 契约头还原 0/0；
event_bridge.rs/lifecycle.rs 零改动；回退后 nexus-hook 回「机制已写、
生产零装配」诚实态。
