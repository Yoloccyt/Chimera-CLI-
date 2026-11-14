# R10 收口批执行报告：capability_of 语义化 + threads 404 安全网 + 探针骨架（2026-11-14）

> 分支 `feat/toolchain-parallel-optimization`，承接 R9（无冲突子集）+ R9b（吸收验收 + 接续）。
> 本报告为 R10 批次单一事实源。

## 0. 用户裁决

1. **收口批全量**：capability_of 语义化 + threads 传输面（404 模拟验收，真实厂商端到端 GATED 如实声明）+ 探针接线骨架（L2 订阅整链延后）。
2. **push R9b 五提交**（授权解除不 push 红线——步骤 0 已执行：`5908aa8b..910bbc45` 入远程备份链）。

## 1. 现场核查判词（Explore 代理，Phase 1）

- HEAD=910bbc45 无外来提交；工作树仅 Cargo.lock autocrlf 幻影 M；stash@{0} 幻觉版本继续隔离
- R9b 五提交未推送（远程备份链停在 e9f522e2）——步骤 0 补齐
- capability_of 语义化全就绪（批量声明类型已落但 from_config 零供给）；`layer_requires_serial` 为幽灵命名（6 处注释引用不存在的 fn）
- threads 传输面三缺口：404 无特判（:566 通用分支）、codec 无 previous_response_id、SessionStore 无句柄承载
- 探针：EffectiveWindowProbe/HealthRegistry 已导出但 build_mca_gateway assembly-only（spec 空）、无 bus、无通道——跨 L10/L2 整链（probe→事件→L2 订阅→governor）select_window_with_context 与 WindowPlanApplied **均无生产调用方**（U-10 纪律：不造）

## 2. 交付（四提交）

| WP | 提交 | 交付 | 验收 |
|----|------|------|------|
| 步骤 0 | （push） | R9b 五提交入远程备份链 | `5908aa8b..910bbc45` |
| WP-T1 | d473fa42 | capability_of 语义化：McpConfig.declared_side_effects 配置段（#[serde(default)]）+ from_config 装配 + 6 处幽灵命名修正（layer_requires_serial → effective_side_effect/node_is_read_safe） | chimera-cli lib 10/0（+2 装配面测试）+ nexus-core 4/0 + 三 crate clippy 净 |
| WP-T2 | a637869e | threads 404 安全网（ADR-294）：transport_invoke 404 特判同 body 重发一次（SDK helper 语义）+ SessionStore.thread_handles 句柄底座 + TcpListener 本地 mock 测试（零新依赖） | mca-gateway lib 404 测试 2/0 + 句柄 1/0 + clippy 净 |
| WP-T3 | 25ea8a97 | 探针接线骨架：McaProbeStack{gateway,probe,health} + build_mca_probe_stack（spec 目录装载/降级 + Observe 默认）——observe 手动档不烧 token | probe_stack 2/0 + mca feature clippy 净 |
| WP-T4 | d4388d2f + docs | wiring 适配 + ADR-294 已随 WP-T2 + 本报告 | 四 crate 1247/0 + E2E 3/0 |

## 3. 关键裁决

1. **404 安全网的形态收敛（研读后的重大简化）**：蓝图 §二原文「continue 404 则须重新 create（SDK helper 自动重发全量）」——当前每轮全量重发模式下 **404 时同 body 重发一次即等价重新 create**；原方案的回放 hook 注入经研读判**过度设计**（AppServer 层进程重启恢复已由 R9b 覆盖，与传输层正交）。previous_response_id 增量续传（TTFT 优化）GATED 留灰度批——句柄底座（thread_handles 表）先行落库。
2. **测试策略**：Transport 真实 reqwest 无注入抽象，但 `check_allowlist` 只校验 host 一致性——**TcpListener 本地 mock（零新依赖）** + spec.base_url 指向 mock 使 404 分支可测（SSRF 拒 loopback 是 mcp-mesh 注册面，非 VendorAdapter 传输面）。
3. **探针骨架与 build_mca_gateway 分离**：后者是 ADR-177「装配态自检」assembly-only 语义（debug_assert spec_count==0），探针栈允许装载 spec——语义不同构，独立装配函数互不污染。
4. **幽灵命名**：6 处注释引用不存在的 fn `layer_requires_serial`（计划层真实机制=effective_side_effect/node_is_read_safe）——注释-代码漂移的清理项（Mimosa 拒 sed 后逐点 Edit）。

## 4. 组合回归

### 4.1 磁盘满事件与处置（教训再现）

首跑组合回归链接失败 `No space left on device`——**D 盘 100% 满（剩 341MB），根因 target/debug 膨胀至 104GB**（R1 教训「可达 109GB」再现；R2 时 39GB → 现状 104GB，膨胀速率约 65GB/月）。处置：`rm -rf target/debug`（保留 release 的 bench 工件）→ 磁盘 100%→63%（105GB 空闲）→ debug 全量重编译（~40min）后重跑。教训：**target/debug 膨胀至 ~100GB 量级时必须主动清理**，登记进 AGENTS.md §10.5 短板表候选（膨胀速率监测）。

### 4.2 结果（全绿）

- 四 crate 组合（chimera-cli/nexus-core/mca-gateway/gqep-executor，72 test targets，--no-fail-fast）：**1247 passed / 0 failed**
- quest_lifecycle E2E：**3 passed / 0 failed**
- 附带修复：`mcp_tool_executor_wiring` 集成测试字面构造 McpConfig 缺新字段（d4388d2f）——教训：L0 配置结构加字段须 grep 全部字面构造点

### 4.3 R10 提交链（六提交）

`d473fa42`（WP-T1 capability_of 语义化）→ `a637869e`（WP-T2 threads 404 安全网 + ADR-294）→ `25ea8a97`（WP-T3 探针骨架）→ `d4388d2f`（wiring 适配）→ docs（本报告）→ push。

## 5. 验收边界诚实声明（ADR-294 D3）

- threads **真实厂商端到端 GATED**：无 Anthropic 公测端点（beta 头 `message-threads-2026-08-12`），TTFT 对照（蓝图 P3 线 ≥-25%）留灰度批
- 探针**真实标定 GATED**：需「最便宜 flash 真通道」（蓝图 §九）；本批骨架=结构就位/数据流可走/默认 Observe 不烧 token
- L2 订阅链（ContextCapabilityUpdated → with_governor）延后：select_window_with_context 与 WindowPlanApplied 现无生产调用方——无消费场景验证不造（U-10）

## 6. 回滚

每 WP 独立 commit；McpConfig/SessionStore 均 serde/CREATE TABLE 兼容迁移；404 特判独立 if 分支 revert 即恢复；探针 Observe 默认无副作用。
