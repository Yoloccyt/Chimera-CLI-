# ADR-251: W5 并行化深化与阈值双源裁决（5.1~5.6 六点定界）

- **状态**：Accepted（2026-11-08，`feat/toolchain-parallel-optimization` W5 波次）
- **上游**：终稿 `docs/reports/toolchain-w7r2-panorama-deepdive-2026-11-07.md` §4（W5.0 + 5.1~5.6）
- **关联**：ADR-239（先实测后登记）/ ADR-249（借用脊柱否决，W4 收口）/ ADR-246~248（同支前波）

## 1. 六点裁决总表

| 点 | 裁决 | 落地形态 |
| --- | --- | --- |
| W5.0 | ✅ 已收口（f15ef9a） | I10 双源对拍（PATH_KEYWORDS heredoc 键集机检），hnsw_tiered 词条补接 |
| 5.1 | ✅ **采纳**（本 ADR D-W5-A） | action_orchestrator 有界并发，缓解形态 |
| 5.2 | ✅ **采纳**（本 ADR D-W5-B） | experience_loop 装配分区 join! |
| 5.3 | ✅ 采纳旋钮，**施工移交 R3-1**（ADR-262） | `CHIMERA_RT_WORKERS`（OQ-09 关闭） |
| 5.4 | ❌ **维持否决**（OQ-10） | 无第二生产源，bus 内部零改动 |
| 5.5 | ✅ **核对确认，零施工** | ShardingPolicy 边界已实现（spec.rs :72-98 实测） |
| 5.6 | ✅ **已由 f70eb45 提前闭合，零施工** | G-27/G-35 stale re-key（:556→:566）双表实测在册 |

## 2. D-W5-A：action_orchestrator 有界并发（采纳，缓解形态）

**改造**：`spawn_action_orchestrator` 的 per-event 无界 `tokio::spawn`（原 :375）
改为有界并发。并发骨架抽为独立 `run_action_loop`（与动作处理解耦，可直接测试）：

- `try_acquire` 优先：拿到许可 → spawn（许可随任务体结束释放）；
- 许可耗尽 → **在 recv 循环内同步执行**：背压降级为串行而非停止 recv。
  WHY 缓解形态：阻塞式 `acquire_owned().await` 会让 recv 停转，broadcast
  缓冲（容量 1024）涨满触发 `Lagged` 丢**任意类型**事件——影响面大于本点；
  串行降级不丢事件、不无界堆积。代价：在飞峰值至多 bound+1（spawn ≤ bound
  + 环内至多 1 个串行执行）。
- `CHIMERA_ACTION_CONCURRENCY` 表驱动（`resolve_action_concurrency` 纯函数）：
  未设=8 / `unlimited`=usize::MAX（≈改造前语义）/ `0`、负数、非数字=8+warn /
  正整数 N=N。
- **附带修复（生产隐患）**：`usize::MAX` 超出 tokio `Semaphore::MAX_PERMITS`
  会 panic——`action_semaphore` 统一钳制到 `MAX_PERMITS`（unlimited 路径在
  改造前若直接以 usize::MAX 建信号量同样会炸，本批以助手函数收口）。

**测试**（`action_orchestrator.rs` tests，221 lib 全绿）：
`resolve_action_concurrency_table`（表驱动 8 案例，env 变更集中单测防竞态）/
`bounded_loop_caps_inflight_and_delivers_exactly_once`（10 请求恰一次 + 在飞
峰值 ≤ bound+1）/ `unlimited_loop_preserves_per_event_spawn`（Barrier(8) 全员
并发过障——串行化即死锁超时）。

**对终稿 TDD 矩阵的显式偏差**：①「每 request_id 回执恰一次」以确定性 10-id
断言替代 proptest 形态——该不变量是「事件恰被循环消费一次」的确定性行为
（spawn 或串行二选一，无第三路径），随机输入空间无增益；②「峰值 ≤ bound」
按缓解形态如实放宽为 ≤ bound+1（终稿 §4.8 字面 ≤ bound 与其 §4.4 缓解设计
互斥，本 ADR 裁定以缓解设计为准）。

## 3. D-W5-B：experience_loop 装配分区（采纳）

**改造**：`spawn_experience_loop` 装配中唯一的 ms 级重活（SQLite 打开）抽为
`open_storage_or_fallback` 惰性 future，与「仅依赖 nexus_bus/engine 的订阅器
群」（原 §5~§A1 各点，**块内顺序逐字保留**）经 `tokio::join!` 并行；card_bus
依赖链（card_bus→mlc；router←mlc；feedback←router）保持 join 前串行主体；
卡片生成触发点（依赖 card_bus）留 join 后串行。

**订阅序偏差登记**：card_bus 的两个订阅者原序「持久化 → mlc」变为
「mlc（join 前）→ 持久化（join 后）」——两者皆订阅者且 join 前无任何 card
生产者（触发点在最后启动），订阅序互换无语义影响；终稿 §4.8 的
「卡片先持久化后反馈」不变量由既有测试
`spawn_experience_loop_wires_full_chain` / `prediction_verified_drives_card_generation`
锁定（6/6 绿）。

## 4. 阈值登记纪律（actx_* / xloop_assemble 移交 X2）

5.1/5.2 的对照键（`actx_*` / `xloop_assemble`，终稿 §4.9）**不在本波登记**：
「先实测后登记」（ADR-239）要求先取同轮读数，而全支唯一 bench 期在 X2
（W4/W5/R1~R5 改码全部完成后校准最终代码态）——本波提前登记会让阈值锚在
中途基线上。两键随 X2 期实测后按五处双写 + I10 对拍入册（若届时收益不显著
则按 ADR-249 D6 先例记录不采纳）。5.2 收益画像本身较小（一次 SQLite open
重叠），不构成阻塞项。

## 5. 回滚

- 5.1：`CHIMERA_ACTION_CONCURRENCY=unlimited` 恢复无界语义（L1，秒级）；
  `resolve_action_concurrency` 纯函数与全部测试保留（W2 回退先例）。
- 5.2：L2 revert（静态分区无运行时开关面）。
- 波次级：5.1/5.2 各自独立 commit，逆序撤销。
