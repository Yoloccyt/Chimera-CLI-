# ADR-294: threads 404 安全网——同 body 重发一次的传输面收口

- 状态: Accepted
- 日期: 2026-11-14
- 批次: R10 收口批
- 设计蓝本: 《HCW_上下文架构重构蓝图_v3.0_1M时代能力感知分层.md》§二 / §5.5
- 关联: ADR-292（P3 桥——session-store 读面接生产）、ADR-290（跨会话一致性）、ADR-065/066（MCA 通道）

## 背景

R9b 已落 session-store 回放桥（ADR-292：`thread.started`/`item.append` schema + `rebuild_items` 重构器 + AppServer 线程级回放入口），传输面（mca-gateway）仍缺三件：404 特判、create/continue 分叉、厂商句柄持久化。真实厂商端到端 GATED（无 Anthropic 公测端点）。

## 决策

### D1 — 404 安全网 = 同 body 重发一次（SDK helper 语义）

蓝图 §二原文：「continue 返回 404 则须重新 create（SDK helper 自动重发全量）」。当前 Chimera 每轮全量重发（body.input 已含完整对话），**404 时同 body 重发一次即等价「重新 create」**——无需重建 hook（原方案在 transport_invoke 注入回放 hook 属过度设计：AppServer 层的进程重启恢复路径已由 R9b 覆盖，与传输层正交）。

实现（adapters.rs `transport_invoke`）：`status == 404` 时同 body 重发一次；重发成功即返回；仍失败 → fail-closed 归 Protocol 错误（错误文案保留 404 语义）。单次重发防风暴；其余 4xx/5xx 维持既有语义零变化。

### D2 — previous_response_id 增量续传与厂商句柄映射 GATED 留灰度批

`continue` 增量（只发 delta + previous_response_id）是 TTFT 优化（蓝图 §1.3：threads 只降 TTFT 不降账单），需真实公测端点验证协议形态。本批落的底座：

- `session.rs SessionStore` 新表 `thread_handles(session_id PK, provider, provider_thread_id, updated_at)`（CREATE TABLE IF NOT EXISTS 零破坏迁移）+ `set_thread_handle`/`get_thread_handle`（upsert 语义，续轮刷新句柄）——「厂商句柄↔本地线程」映射的持久化底座，灰度批接 build_request 时直接消费
- L0 `ThreadSupport` 枚举（ADR-291）已就位，灰度批经 `is_supported()` gating 分叉

### D3 — 测试边界诚实声明

Transport 用真实 reqwest::Client（无注入抽象），但 `check_allowlist` 只校验 host 一致性——测试以 **TcpListener 本地 mock（零新依赖）** + spec.base_url 指向 mock 使 404 分支可测：

- `threads_404_retries_full_resend_once`：404 → 重发 → 200，恰 2 次请求
- `threads_404_twice_fails_closed`：双 404 → Protocol 错误，恰 2 次请求
- `thread_handle_upsert_roundtrip`：set/get 往返 + upsert 刷新 + 会话隔离

**验收边界**：真实 Anthropic threads 端点（beta 头 `message-threads-2026-08-12`）的端到端与 TTFT 对照（蓝图 P3 验收线 ≥-25%）留灰度批——本批验收=404 模拟 + 协议形态单测。

### D4 — 红线沿用（蓝图 §二）

不依赖线程做存储（段文件权威源，ADR-290 D2）；ZDR 类隐私配置位不牺牲（本批未引入线程持久化的敏感面——句柄是 opaque id）；计费不变意味着 404 重发不省钱（安全网不是成本机制）。

## 回滚

404 特判为独立 if 分支（revert 该 diff 即恢复通用 4xx 错误）；句柄表 CREATE TABLE IF NOT EXISTS 零破坏（表可留库不消费）；测试随 WP revert。
