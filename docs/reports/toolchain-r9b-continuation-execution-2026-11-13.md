# R9b 接续批次执行报告：吸收验收 + HCW v3 P1/P3 桥/卫星态/w4（2026-11-13）

> 分支 `feat/toolchain-parallel-optimization`，承接 R9 无冲突子集（0ddda7f4..e9f522e2）。
> 本报告为 R9b 单一事实源。

## 0. 用户裁决（AskUserQuestion ×2）

1. **吸收接续**：另一会话 611 行在途成果（自 R9 收官后双方零活动，疑似已结束）经验收后入库，在其基础上接续 WP-C/E/H/D。
2. **修订 ADR-290 D4**：读面 replay_since 以「threads 404 安全网」为真实消费场景接生产（蓝图 §二/§5.5），ledger_parity_locks 锁 5 同批翻转。已落 `ADR-292-p3-threads-404-replay-bridge.md`。

## 1. 阶段 0：吸收验收（已完成）

### 1.1 逐 crate diff 审查结论（26 文件 +611/-81 全部通过）

| crate/文件 | 改动 | 性质判定 |
|-----------|------|---------|
| hcw-window assemble.rs（新 304 行）+ lib.rs 挂载 + tests/assembly_contract.rs | 统一上下文装配内核：FNV-1a 分桶+逐字复核去重、预算账本 min(max_tokens, tier_capacity)、三级保护组（可驱逐/近期/钉选）价值密度+seq 驱逐、输出保输入序；纯函数 | 痛点①上下文冗余的直接解；质量过关（契约测试在位） |
| hcw coarse.rs（+80） | 三信号 thread::scope 并行，panic 回退串行，逐位相等测试 | R8 深审项由另一会话落地 |
| hcw rerank.rs（±5） | density_order sort_by→sort_unstable_by（全序 tiebreaker，注释含不做 select_nth 投机截断的理由） | R8 深审项的正确处置 |
| hcw window.rs（±32） | **纯 rustfmt 折行**（对 R8 ADR-288 代码零语义变化） | 与我方改动叠加兼容 |
| session-store tree.rs（±12） | **纯 rustfmt 折行**（对 R8 WP-04 代码） | 兼容 |
| scc-cache wal.rs（+133） | `write_ahead_log_batch` 单事务批量化（单次 spawn_blocking、敏感拦截整批拒绝、空批次合法） | R8/ADR-289 登记项落地 |
| gqep toolplan_runner.rs（+76）+ chimera-cli mcp_tool_executor.rs（+25） | `declared_side_effects` 批量声明面 + `with_declared_side_effects` 注入（优先级链：节点显式>capability_of>批量声明>未知按写；默认空集既有行为逐字不变） | **mcp 并行封顶点（capability_of 恒 None）的解法** |
| chimera-cli run.rs（+90） | `CHIMERA_TOOL_BATCH_CONCURRENCY` 批量派发并发度配置（默认 8 逐字不变，纯函数解析） | 调度面配置化 |
| chimera-tui curator.rs（+104） | 精确去重（role+content 逐字同保留最先；原始下标空间实现保 Recent 边界契约；零差异保证注释） | 痛点①的 TUI 面 |
| ADR-290 + audit_memory_consistency.py + ledger_parity_locks 锁 5 + scripts 基线七件 + workflows + 契约头订正 + tests/session_isolation.rs（新） | 跨会话一致性契约 D1-D6 + 只读审计器 + 反向锁 + 基线重生成 | D4 将由 ADR-292 修订（裁决预告已注） |

### 1.2 测试验收

- 六 crate 组合（hcw-window/session-store/scc-cache/gqep-executor/nexus-hook/nexus-app-server）：**986 passed / 0 failed**
- chimera-tui lib：**1015 passed / 0 failed**
- chimera-cli 触及面（cli + ledger_parity_locks + panorama）：初跑 **1 failed** → 根因=**我方 R9 WP-0 漏同步**（panorama_adjudication_locks 的 154 变体锁未随 ADR-291 升 156——另一会话在途 diff 为纯 fmt 与此无关）→ 修复提交 `5696b634` → 复跑 **70 passed / 0 failed**
- 全仓 `grep "usize = 154"` 零残留

### 1.3 吸收提交

- `5696b634` fix(locks)：panorama 变体计数锁 154→156 同步补齐（我方 WP-0 漏同步点）
- `601e44e1` feat：吸收并行会话在途成果（30 文件 +1639/-68，含 ADR-290 入库 + adr_index ADR-290/291 双登记）

**阶段 0 结论：阻塞解除，阶段 1 开工。**

## 2. 阶段 1：四 WP 实施（进行中）

| WP | crate 所有权 | 内容 | 执行 |
|----|-------------|------|------|
| WP-C | hcw-window | WindowPlan/WindowGovernor/TierAlloc + R/S/E/M 配置注入（枚举不动）+ ADR-288 协议兼容 + assemble 整合 + WindowPlanApplied 发布 + StaticGovernor 静态回退 | 代理 |
| WP-E | session-store + nexus-app-server + ledger_parity_locks | 事件 schema 扩展（thread.started/item.append）+ thread_replay.rs 重构器/游标映射 + AppServer 线程级回放入口 + 锁 5 翻转 + 404 注入测试（依据 ADR-292） | 代理 |
| WP-H | quest-engine + quest_loop | 卫星态嵌入/还原 + from_bytes 降级修复（参数注入）+ QuestBridgeState 恢复重建 | 代理 |
| WP-D | hcw-window types/compressor + nexus-contracts-seams | w4 预埋+默认开启：ContextEntry.source（EntrySource 复用 assemble ContextSource 六变体单一真值源归一）+ SelectorWeights 四维 + compute_importance_score 第四项 + 退化安全阀 + CHIMERA_W4_DISABLED env | 主会话（WP-C 后串行） |

## 3. 验收标准

1. 逐 crate 定向测试全绿 + clippy -D warnings
2. hcw criterion 全组基准零回退（蓝图 P1 验收线）
3. w4 行为锁：source=None 打分逐位不变（默认开启=存量零影响）
4. P3 桥 404 注入零丢失；ledger_parity_locks 翻转后绿
5. WindowPlanApplied 快照可回放（governor proptest 预算边界）
6. 组合态：chimera-cli check + quest_lifecycle + cli 测试
7. 事件预算零新增（WindowPlanApplied 已在 R9 落，本批变体数不动）

## 4. 回滚

吸收提交 `601e44e1` 独立可 revert（恢复在途态）；每 WP 独立 commit；w4 env 开关；governor 静态回退默认；P3 桥 append-only；锁 5 翻转随 WP-E revert 即恢复预留锁。

## 5. 执行记录

### 5.1 提交链（R9b 七提交）

`5696b634`（锁补齐 fix）→ `601e44e1`（吸收 30 文件 +1639/-68）→ `6fcc15fb`（WP-C governor）→ `6310890c`（WP-D w4）→ `be0e92ba`（WP-H 卫星态）→ `992515a6`（WP-E P3 桥 + ADR-292）→ docs（ADR-293 + 本报告 + .gitignore 豁免）。

### 5.2 逐 WP 验收

| WP | 提交 | 测试 | 关键裁决 |
|----|------|------|---------|
| 阶段 0 吸收 | 5696b634 + 601e44e1 | 六 crate 986/0 + TUI 1015/0 + cli 70/0（修复 154 锁漏点后） | 26 文件全部通过审查（含 Wal 批量化/declared_side_effects/TUI 去重/coarse 并行/R9 残句订正） |
| WP-C governor | 6fcc15fb | hcw 全 501/0 + bench ns 级零回退 | PlanContext 依赖倒置；select_window_inner 重构保 ADR-288 协议；**f32 as u64 整数化 bug**（0.8→0）测试抓获 |
| WP-D w4 | 6310890c | hcw 全 504/0 + 3 行为锁 | **L0 三维不动**（S4 arm 冻结面）改 hcw-local；source_value_factor 单一真值源归一；env 回退逐位保证 |
| WP-H 卫星态 | be0e92ba | quest-engine 422/0 + quest_loop 3/0 | sidecar 形态（L0 不动规避 ADR-213 F3）；引擎统一携带原子配对；三条恢复通路；四条诚实局限登记 |
| WP-E P3 桥 | 992515a6 | session+nexus 135/0 + 404 桥 5/0 + 锁 5/0 | ADR-292 修订 D4；schema append-only；WI-G11 断言适配为 seq 单调性（schema 无关） |

### 5.3 外部介入事件（第三幕，已处置）

实施期间外部会话再次活动：seccore wasmtime 47→50.0.2「治理升级」（RUSTSEC-2026-0315~0424）——**版本号幻觉**（cargo update --precise 实证 crates.io 无 wasmtime 50.0.2，max_stable=50.0.1），其半成品 lock 重解析失败阻塞全部 cargo 命令。处置：Cargo.lock checkout 恢复 + seccore M 经 `git stash push`（带可检索 message）隔离——后经 WP-H 代理独立佐证「外部会话已自行回退 47.0.2」，工作树 seccore M 消失自愈。stash@{0} 保留其 50.0.2 尝试备查。

### 5.4 组合态回归（全绿）

- quest_lifecycle E2E：**3 passed / 0 failed**
- chimera-cli cli：**61 passed / 0 failed**
- 四 crate 组合（hcw-window/session-store/nexus-app-server/quest-engine，--no-fail-fast）：**1061 passed / 0 failed**
- 事件预算零新增（R9 已 +2，本批变体数不动——panorama 锁 156 验证绿）
