# ADR-057: OSA Memory 维度集成 omega-learner S2 自适应记忆策略

## 状态

Proposed — 2026-07-30

## 上下文

三重悖论"记忆悖论"红线（§3.4.5 红线 1）指出：OSA 静态稀疏掩码不能替代 MemCon 式自适应记忆控制。固定 top-k 相似度召回在任务阶段切换时会产生"幽灵记忆"（新旧事实共存无法区分时间有效性）。

### 当前病理

OSA `compute_memory_mask` 直接复用 routing 的 Top-K 策略（8/16/24/32），未实现自适应记忆策略。omega-learner S2 接缝已实现 5 种记忆策略（MinimalRecall / StandardTopK / QueryReformulation / AggressivePruning / TimeFocused），但 OSA 尚未集成。

### 约束

- 依赖铁律 §2.2：osa-coordinator (L6) 不能直接依赖 omega-learner (L6) 的具体实现
- 向后兼容：未配置 S2 时必须 fallback 到 StandardTopK（当前行为）
- `#![forbid(unsafe_code)]` 合规

## 决策

在 `nexus-contracts` (L0) 中定义 `MemoryStrategyProvider` trait 和 `MemoryStrategy` 枚举，OSA 通过 `Option<Arc<dyn MemoryStrategyProvider>>` 注入策略提供者。

### 关键设计

1. **L0 trait 解耦**：`MemoryStrategyProvider` 定义在 L0，omega-learner 在 L6 实现此 trait，OSA 在 L6 通过 `Arc<dyn>` 调用。与 `VectorStore` trait 同模式（ADR-033）。

2. **5 种策略 × k_multiplier**：
   - MinimalRecall → 0.5× K（快速响应，减少噪声）
   - StandardTopK → 1.0× K（当前行为）
   - QueryReformulation → 1.5× K（多角度查询，扩大召回）
   - AggressivePruning → 0.25× K（仅保留最高相关）
   - TimeFocused → 1.0× K（按时间有效性过滤）

3. **C4 能力场灰度**：未注入 provider 时 fallback 到 `StandardTopK`（编译期 const，非运行时旗标）。

## 影响

- 新增 `crates/nexus-contracts/src/memory_strategy.rs`（L0 纯类型）
- 修改 `crates/osa-coordinator/src/coordinator.rs`（新增字段 + builder + compute_memory_mask）
- 修改 `crates/osa-coordinator/src/types.rs`（TaskProfile 新增 task_phase 字段）
- 修改 `crates/omega-learner/src/s2_memory.rs`（实现 MemoryStrategyProvider trait）

## 验证

- `cargo test -p osa-coordinator -p nexus-contracts -p omega-learner` 全部通过
- `cargo clippy --workspace --all-targets -- -D warnings` 零警告
- S2 不可用时 fallback 到 StandardTopK（向后兼容测试通过）
