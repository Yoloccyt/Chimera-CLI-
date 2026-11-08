# ADR-259: U 项三态裁决汇总（P4 / 全景收官裁决波）

- **状态**：Accepted（2026-11-08，Tranche 2 工单 G；deepdive §9 照单执行，含 D1~D9 决策）
- **上游**：deepdive §9（三态裁决表 + §9.3 事件预算六锁 + §9.4 TDD）
- **关联**：ADR-256~258（P1~P3 同批）/ ADR-224（U-09 议程面）/ ADR-225 §5 T-19（接线路径）

## 1. 三态裁决表（每项 = 现状锚点 + 三态 + 理由；详表见 deepdive §9.2 原文）

| 项 | 三态 | 关键理由 |
| --- | --- | --- |
| U-05 VoteCastRequested | **维持预留** | publish_vote 刻意不发（event_loop.rs 注释自陈不变量=U-10 纪律正确形态）;解锁=L8 提案模型落地 |
| U-06 slash→ActionRegistry 映射 | **移交 TUI 批次** | action_id: None 计 56 条（深审实测,上游 54 漂移+2）;跨域回归面违反波次隔离 |
| U-07 TuiChatSubmitted.slash_command | **维持预留**（与 U-06 同批解锁） | types.rs 契约收窄注释写死（B4-S F-57）,单方面恢复填充制造半途契约 |
| U-08 session-store | **写面已接线（Z-3 订正）/读面维持预留/注记修复（本波完成）** | 写面=composition.rs CbmrWriter::open + with_session_store（N-4 降级分支实证）;读面无真实消费场景,为绿接线=虚构消费;lib.rs 三订正段落档（含文本锁锚） |
| U-09 pvl-layer 反馈闭环 | **维持预留**（挂 ADR-224 议程） | 登记在先的议程面（重复裁决双源）;上游三处行号引的是测试段丢弃（订正） |
| U-11 RewardSignalReported | **维持预留**（RL 闸门期） | 消费者=Python 训练服务——RL 闸门期禁止实施（治理决策） |
| U-20 BusLogger 导出 | **改判「已接线（env 可选文件 dump）」** | experience_loop.rs CHIMERA_METRICS_DUMP_PATH → 5s 周期 render_metrics → tokio::fs::write（回归锁实证）;HTTP /metrics 端点**否决**（零端口暴露） |
| F-09 TuiActionProgressed | **接线候选**（分支集合挂 OQ-27,独立波次） | 消费面已存在（chat.rs:305-320）、生产发布点 0=刻意决策（types.rs 注释锁）——补点待 OQ-27 分支集合裁决;request_id 维持不加 |
| F-12 已知 TODO 三处 | **全维持** | per-task 调度=Phase 3 独立 ADR 量级;unreachable 登记（N-8） |

## 2. 事件预算纪律（六锁同批口径，deepdive §9.3 照录）

P1~P4 **零新增 NexusEvent 变体**（P1 零新增零发布点 / P2 复用 AgentTask* /
P3 走轨二 DynamicEvent）。后续若需新增：六锁同批（VARIANT_COUNT_LOCK=154 /
doc_count_freeze / registry 定级行 / ownership 台账 / bus.rs 双常量 / TUI
穷尽 match）+ 要么带生产构造者要么带证据登记行（U-10 禁令）。

## 3. 回归锁（panorama_adjudication_locks 5/5，2026-11-08）

1. `u20_metrics_dump_env_path`：设 env → 组合根总线（BusLogger 经 W3 set_logger
   装配——裸 EventBus::new 的 logger 恒 None,首跑教训）→ experience_loop →
   6.5s 后 dump 文件含 `nexus_event` 指标族（渲染无 `_total` 后缀——deepdive
   §9.4 原引 nexus_event_total 为预写形态,已按实测锚修正）。
2. `u08_session_store_note_parity`：三订正段文本锁（composition.rs +
   with_session_store + PRODUCTION-WIRED 三锚）。
3. `f09_no_producer_is_deliberate`：types.rs 刻意决策注释段 + 解锁路径注释在场。
4. `u05_vote_remains_unpublished`：诚实降级注释块在场 + publish_vote 函数体
   无 publish 调用（函数体切片扫描）。
5. `variant_count_lock_unchanged`：VARIANT_COUNT_LOCK=154 文本锁。

跨 crate 路径：一律 CARGO_MANIFEST_DIR 起算（§9.4 missing-11 纪律,先例
tool_runtime_wiring.rs:223）。

## 4. 回滚

纯裁决 + 回归锁测试 + U-08 注记订正——revert 单 commit 即回裁决前;
六锁零触碰。
