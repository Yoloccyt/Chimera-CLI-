# ADR-286 -- 自动检查点后台化：`update_task_status` 关键路径摘除 ~4ms 串行税

- 状态：Accepted（2026-11-12）
- 决策者：R7 批次（WP-C2）
- 关联：ADR-213（Checkpoint schema_version 尾字段）/ 工单集 §0.3 检查点归因（save(100 任务) ≈4.0ms，瓶颈为磁盘固定开销 ~3.7ms）/ §4.4 #7（fire-and-forget 评估框架）/ ADR-257 D1 案 β（QuestEngine Clone = 浅共享内核，spawn 依赖此语义）
- 影响面：`crates/quest-engine/src/{engine.rs,config.rs}`
- 行为影响：`checkpoint_async: bool` 默认 `true`——自动检查点从 `update_task_status` 内联 await 改为后台 spawn 保存；**durability 语义变化**（返回 ≠ 已落盘）；`false` 回滚为内联 await（历史行为）。同步 `save_checkpoint`/`load_*`/prune 路径零变化。

## 1. 背景

| # | 事实 | 证据 |
|---|---|---|
| 1 | 自动检查点在 `update_task_status` 内联 `save_checkpoint().await`，把 ~4ms 磁盘固定开销串进任务状态更新关键路径 | `engine.rs:538-553`（改前）；`checkpoint.rs` bench 登记 save(100)≈4.0ms |
| 2 | save 内部已 `spawn_blocking`（磁盘 IO 不占 async worker），但调用方**同步等待**其完成 | `checkpoint.rs:97-104` |
| 3 | interval 语义本就不要求逐步持久化（默认每 3 个任务完成才存一次），单次丢失窗口与 interval 粒度同量级 | `QuestConfig::checkpoint_interval` 语义 |
| 4 | QuestEngine Clone 为浅共享（Arc 内核），任务内 clone 可安全快照同一 Quest 状态 | `engine.rs:51-57`；ADR-257 D1 案 β |

## 2. 决策

### D1 — 后台化 + 终态同步兜底（默认 `checkpoint_async = true`）

- 非终态触发：`tokio::spawn` 后台保存，fire-and-forget（幂等操作失败仅日志，§4.4 #7 允许；无 JoinHandle 管理——保存失败仅损失一次检查点，不损一致性）。
- **终态转换（`all_terminal`）恒同步**：`complete_quest` 发布 `ExecutionCompleted` 后上层可能立即消费/销毁 Quest，后台任务来不及快照就丢终态——durability 关键点不异步。
- `checkpoint_async = false`：回滚为内联 await（历史行为，`update_task_status` 返回前已落盘）。

### D2 — 不做去抖（显式否决）

批准方案原含"同 quest 去抖合并"。实施裁决砍掉：interval 触发频率低（每 N 个完成 1 次，N 默认 3），后台化已消除关键路径税；去抖需引入 per-quest 待存状态与竞态面，复杂度与收益不成比例。若未来出现高频触发场景（interval=1 且任务完成速率 >> 4ms），再评估（重开条件：后台保存任务堆积可观测）。

### D3 — 诚实登记：durability 窗口变化

默认开启后，`update_task_status` 返回 ≠ 检查点已落盘；崩溃窗口扩大至"后台保存完成前"（~4ms 级）。兜底：① 终态同步（D1）；② interval 粒度本就容忍单步丢失。测试锁定：async 落盘最终性（5s 轮询窗口）、同步模式返回前落盘、终态同步模式返回即落盘（3 测试）。

## 3. 验证

- quest-engine lib **274 passed / 0 failed**（新增 3 测试全绿）；clippy `-D warnings` 零告警；fmt 干净。

## 4. 回滚

配置面开关 `checkpoint_async=false` 即回历史行为（零代码回滚）；代码级单 commit revert。`QuestConfig` 新字段有 `#[serde(default)]`，旧配置文件零兼容性问题。
