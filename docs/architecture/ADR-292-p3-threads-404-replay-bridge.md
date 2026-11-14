# ADR-292: P3 桥落地——修订 ADR-290 D4，replay_since 读面以「threads 404 安全网」接生产

- 状态: Accepted
- 日期: 2026-11-13
- 批次: R9b 接续（吸收验收后）
- 设计蓝本: 《HCW_上下文架构重构蓝图_v3.0_1M时代能力感知分层.md》§二 / §5.5
- 关联: ADR-290 D4（本 ADR 修订其读面裁决）、ADR-259 D3 / U-08（读面预留的原始裁决）、ADR-141（append-only 会话流）、ADR-291（R9 无冲突子集）

## 背景与对撞

ADR-290 D4 裁定「`replay_since` 读面维持预留，以『生产段零调用 + composition 零引用』反向锁死（ledger_parity_locks 锁 5）」。该裁决在当时成立：读面无真实消费场景，为绿接线=虚构消费（U-08/U-10 纪律）。

但 HCW v3 蓝图 §二/§5.5 赋予读面一个**真实且必要的消费场景**：Anthropic Message threads 的 `continue` 返回 404（服务端线程 best-effort，5 分钟空闲过期）时，harness 必须重新 `create`——即按 Thread 段**完整回放重建对话**。蓝图原文：「Chimera 的 append-only 会话流（ADR-141）天然持有重新 create 所需的完整对话——接入 threads 时 Chimera 是唯一不怕 best-effort 丢失的 harness（自己的持久化兜底，服务端线程只是加速器）」。session-store 的 k-way 归并回放（`replay()`，fork 前缀链自动合并）正是为此设计的读面。

**结论：读面从「无场景预留」变为「有场景待接线」，ADR-290 D4 的预留前提消失。本 ADR 修订 D4：读面接生产，以 threads 404 回放桥为首个真实消费场景（P3 桥最小闭环）。**

## 决策

### D1 — 事件 schema append-only 扩展（不动旧 turn.submit）

session-store 现仅持久化 `turn.submit`（payload 仅 turn_id JSON）——Item 文本/工具调用未落盘，回放无法重建 `Vec<Item>`。新增两类持久化事件类型（append-only，旧数据回放兼容）：

- `thread.started`：线程元数据（thread_id, created_at）
- `item.append`：Item 级内容追加（item_id, item_kind, payload JSON）

`MODEL_VISIBLE_MAPPING` 白名单投影（ADR-201）**不动**——本批新事件类型不进模型可见投影（回放桥是 harness 内部重建面，非模型上下文投影面）。

### D2 — event→Item 重构器（纯函数，可单测）

新 `session-store/src/thread_replay.rs`：`rebuild_items(events) -> Vec<Item>`——从事件流重构 Item 序列。纯函数（输入事件切片、输出 Item 向量），与 I/O 解耦。

### D3 — Offset↔ItemId 游标映射

AppServer 内存游标是 ItemId 字符串、store 游标是 Offset（u64）——`thread_replay.rs` 提供双向映射结构（事件流内 item.append 同时携带两元，重构器输出 (Offset, ItemId) 对账表）。

### D4 — AppServer 线程级回放入口

`server.rs` 新增线程级回放路径：读 session-store（磁盘权威源）而非内存 DashMap——`replay_since` 的生产消费面就此激活。语义：404 回退时按 Thread 段回放重建，事件流为权威源（ADR-290 D2 语义沿用）。

### D5 — ledger_parity_locks 锁 5 翻转

锁 5 从「读面预留反向锁」（出现调用即失败）改为「读面已接生产行为锁」（组合根/生产段**必须**存在回放桥调用 + ADR-292 引用锁）——锁的强制力方向反转，防静默回退到预留态。

### D6 — 与 ADR-259 D3 / U-08 的关系

U-08 当初裁定「读面 replay_since 预留——现无断线重连 CLI 面，为绿接线=虚构消费」。本 ADR 不推翻该纪律，而是声明**场景已出现**（蓝图 P3 threads 404 回退），接线从虚构变为必要。ADR-290 D4 的「独立 ADR 裁决」要求由本 ADR 满足。

## 范围边界（减法）

- **不做 threads 传输面**（mca-gateway create/continue 协议）——组合根 mca 装配为 GATED assembly-only，无灰度环境；本 ADR 只落「404 发生后的重建能力」，传输面后续独立裁决
- 不做 CLI 命令面（404 重建是 harness 内部路径，非用户命令）
- 不动 `MODEL_VISIBLE_MAPPING` 投影白名单

## 验收

- 404 注入测试：构造线程事件流 → 模拟线程丢失 → 回放重建 → 断言重建的 `Vec<Item>` 与原始逐条一致（零丢失）
- 旧数据兼容：仅含 `turn.submit` 的历史会话回放不 panic（重构器对新事件类型缺省容忍）
- ledger_parity_locks 翻转后全绿
- `cargo test -p session-store -p nexus-app-server` + clippy 干净

## 回滚

事件 schema append-only（新类型不写即回退）；AppServer 入口与重构器单 commit 可 revert；锁 5 翻转随本批 revert 即恢复预留锁。
