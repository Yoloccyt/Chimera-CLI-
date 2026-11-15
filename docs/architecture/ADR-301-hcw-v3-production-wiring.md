# ADR-301: HCW v3 生产接线——能力快照、Governor 预算、来源标注与分层配额

- 状态: Accepted
- 日期: 2026-11-14
- 批次: 本 Spec A 波（HCW v3 P1/P2 生产接线）
- 设计源: `HCW_上下文架构重构蓝图_v3.0_1M时代能力感知分层.md` §5.2~§5.4 / §六
- 关联: ADR-291（P0 能力档案）、ADR-293（Governor + w4）、ADR-298（R12 装配接线）

## D1 — 能力档案快照（A1）

- 新增 `crates/hcw-window/src/capability_snapshot.rs`：`CapabilitySnapshot`（`Arc<RwLock<SnapshotInner>>`）+ `CapabilityWindow`。
- `apply_event` 消费 `NexusEvent::ContextCapabilityUpdated`，校验 `0 < effective_window ≤ advertised_window`；非法载荷保留旧值。
- `spawn_refresh` 先 `bus.subscribe()` 再 `tokio::spawn`（避免广播静默丢失），返回 `JoinHandle`；Lagged 只 warn 不中断。
- 依赖方向：L10 发布事件 → L0 契约 → L2 订阅；hcw 不 import mca-gateway。
- 证据：`cargo test -p hcw-window --lib` 345 passed（含 5 条 capability_snapshot 用例）。

## D2 — mca 会话预算接 Governor（A2）

- `AdapterOptions` 新增 `window_governor: Option<Arc<dyn WindowGovernor>>` 与 `plan_context: Option<PlanContext>`，`VendorAdapter` 持有同名字段。
- 新增 `conversation_budget_with_plan`：governor 产出 `PlanSource::Governor` 时预算 = `plan.resident_cap`，否则回退 legacy ratio；`CHIMERA_WINDOW_GOVERNOR=off` 强制回退（OnceLock 启动期读取）。
- `prepare_effective_request` 在 governor 产 plan 时发布 `WindowPlanApplied`（双跑对照锚点）；静态回退不发布，事件序列与 HEAD 一致。
- 证据：`cargo test -p mca-gateway --lib` 260 passed（含 `governor_plan_overrides_legacy_budget` / `static_fallback_keeps_legacy_budget`）；`cargo test -p mca-gateway --test assembly_wiring` 11 passed（关态逐字节等价仍绿）。

## D3 — 生产 ContextEntry 来源标注（A4）

- `chimera-mas::ContextBlock` 新增 `source: Option<hcw_window::ContextSource>`（`#[serde(default)]`）+ `with_source` 链式构造。
- `build_prompt_legacy` 将 `Some(source)` 透传到 `ContextEntry::with_source`，激活 `HcwConfig.source_weight`（w4）；`None` 保持中性 0.5。
- 兼容：旧 checkpoint / 序列化数据无此字段时反序列化为 `None`，零破坏。
- 证据：`cargo test -p chimera-mas --lib` 320 passed；`--test agent_context_test` 15 passed；hcw 既有 `test_w4_tool_source_downweighted` 绿。

## D4 — 分层配额与页预算成为运行行为（A3）

- 新增 `TierBudget` + `WindowPlan::tier_budget()`：R/S/E/M = `resident_cap × tier_allocation`，余量由 semantic 吸收，四项之和恒等于 `resident_cap`；`assembly_capacity() = S+E+M`。
- 新增装配内核入口 `assemble_context_with_tier(items, contract, &TierBudget, estimate)`（`crates/hcw-window/src/assemble.rs`）：
  - **页预算**：单条 token 估算超过 `page_budget` 即**拒绝装载**，计入新诊断字段 `AssemblyDiagnostics::page_rejected`（`#[serde(default)]`，旧载荷兼容）；`0` = 关闭。WHY 拒绝而非切分：`seq` 是调用方回取原消息的唯一句柄，切分会破坏「输出是输入子序列」的结构性保证。
  - **分层配额**：来源映射 R=`System|User|Assistant`、S=`Tool`、E=`Memory`、M=`Retrieval`；某层累计超配额时在该层内按 `eviction_priority`（保护组 → 价值密度升序 → `seq` 降序）驱逐，计入 `evicted`。分层与全局两道驱逐共用同一排序函数，避免优先级分叉。
  - **全局预算** = `min(contract.max_tokens, tier_budget.total())`；四项全 0 = 显式零容量（驱逐全部），「未配置」的形态是不提供 `TierBudget`（走旧入口）。
  - **关态等价**：旧入口 `assemble_context_with` 委托同一内核且传 `None`，新增执行段整段跳过；「各项无限」预算与旧入口逐字段相等的测试锁定该等价性。
- mca 侧维持既有 `trim_to_budget_with_tier`（只传 `tier_capacity = S+E+M`，不传页预算）：该路径的 `content` 是去重键扁平化文本（表示层缺口），且工具消息「恒保留」是不变量，页预算在该表示上会误删合法大工具输出。该边界登记为**生产接线待办**，不属于内核能力缺口。
- 证据：`cargo test -p hcw-window --test assembly_tier_budget` 8 passed（页拒绝边界、页关闭、S/E/M 分层驱逐、零容量、旧入口逐字段等价、plan 派生预算、proptest 四不变量）；`cargo test -p hcw-window` 全绿；`cargo test -p mca-gateway` 全绿；`cargo clippy -p hcw-window -p mca-gateway --all-targets -- -D warnings` EXIT=0。

## 回滚

- A1：删除新模块与 lib.rs 声明；无行为变更（默认未订阅）。
- A2：`AdapterOptions` 新字段默认 `None`，不注入即回到 legacy；env `CHIMERA_WINDOW_GOVERNOR=off` 为第二道逃生。
- A3：不提供 `TierBudget` 即回退旧内核路径（结构性等价）；`WindowPlan::tier_budget` 为纯函数。
- A4：`source = None` 即 w4 中性；`CHIMERA_W4_DISABLED=1` 逐位回退三维公式。

## 未完成 / 后续

- Governor 尚未在组合根生产装配中注入（`build_mca_gateway` 仍是 assembly-only）；本批交付的是**接线面 + 单测证据**，生产注入需随 mca 灰度环境单独开启。
- `page_budget` 已在装配内核执行并有测试锁定，但 mca 生产裁剪尚未消费它：该路径的去重键文本表示与「工具消息恒保留」不变量使页预算在那里会误删合法大输出；需先关闭 R12 W1-2 B1' 表示层缺口，再单独灰度。
- `>1M` 标杆语料、threads 真实端点均保持 GATED/条件项。
