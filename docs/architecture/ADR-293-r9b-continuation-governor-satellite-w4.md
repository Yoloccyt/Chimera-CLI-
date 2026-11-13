# ADR-293: R9b 接续批次——WindowGovernor 动态定容、checkpoint 卫星态接线、w4 来源维度

- 状态: Accepted
- 日期: 2026-11-13/14
- 批次: R9b（吸收验收 + 接续落地）
- 设计蓝本: 《HCW_上下文架构重构蓝图_v3.0_1M时代能力感知分层.md》§5.4 P1 / §5.4 P2(w4)
- 关联: ADR-290（跨会话记忆一致性，D4 由 ADR-292 修订）、ADR-291（R9 无冲突子集）、ADR-292（P3 桥）、ADR-288（版本化锁外压缩——WP-C 的硬兼容点）

## D1 — WindowGovernor 动态定容（WP-C，提交 6fcc15fb）

蓝图 P1 的核心交付：常驻层按**有效窗口**（非标称）动态定容。

- **governor.rs 新模块**：`WindowPlan{resident_cap, page_budget, tier_allocation, plan_source}` + `TierAlloc`（R/S/E/M 四层配额，DEFAULT 0.6/0.2/0.1/0.1，is_valid 和≈1.0）+ `WindowGovernor` trait（组合根可注入 mock）+ `AdaptiveGovernor`（定容公式：`resident = min(effective_window × s, 预算收缩项)`，s=0.8 缺省 / Deep 0.9 放宽；utilization>0.9 时 ×max(0.5, 1-util) 收缩；floor 4K 下界保护，**上界优先于 floor**——病态小模型 resident 不得超过有效窗口）+ `StaticGovernor`（静态回退显式形态，resident_cap=u32::MAX 不收缩）
- **依赖倒置**：`PlanContext` 纯数据字段（effective_window/budget_utilization/thinking_mode）——hcw-window(L2) 不感知 decb(L8)/TTG(L9)/mca-gateway(L10)，装配面采集传入（蓝图 §5.2 依赖铁律）
- **HcwWindow 集成**：`governor: Option` 字段（None=现状路径逐位不变，蓝图 §八「默认档=静态回退」）+ `with_governor` 构造器（既有构造点零破坏）+ `select_window` 重构为 `select_window_inner(complexity, cap_override)` 薄壳（**ADR-288 三阶段协议不变**）+ `select_window_with_context` 新入口（governor 定容**只收缩不放大** min 合并）+ `WindowPlanApplied` 事件发布（双跑对照与快照回放锚点）
- **R/S/E/M 重参数化的最小形态**：`capacity()/effective_capacity()` 已是 config 驱动（types.rs:102-157）——枚举 L0-L3 与 as_str/parse/upgrade/downgrade **零改动**，旧 4K/32K/128K/1M 数字降级为低能力机型缺省档（蓝图 §5.1）
- 教训：f32 安全系数禁 `as u64` 整数化（0.8 as u64 == 0 令 resident 全归零落 floor——首轮实现 bug 由测试当场抓获）

## D2 — checkpoint 卫星态接线恢复（WP-H，提交 be0e92ba）

跨会话一致性缺口闭合：kill -9 后搜索树/长任务图不再清零。

- **sidecar 形态**（裁决）：`Checkpoint` 宿主是 L0 nexus-contracts-core 契约，加字段须走 ADR-213 F3 schema 演进（跨层变更）——卫星态以旁路文件落盘（与主检查点同 checkpoint_id 配对、同生命周期），`.sat` 扩展名不被 list/load/prune 的 bin 过滤误枚举（测试锁）
- **from_bytes 降级缺陷修复**：`from_bytes_with(bytes, storage, hook)` 注入形态（旧 `from_bytes` 委托默认值并文档标注降级语义）——恢复后 record_step 写入注入 storage、触发注入 hook，不再降级 InMemory/Noop
- **引擎统一携带配对**：QuestEngine `satellites` 注册表；桥只更新内存态，`save_checkpoint` 统一读取同刻落盘——Quest 快照与卫星态**原子配对**（消除「桥自行落盘 vs 引擎无卫星态保存」异步错位）；restore None→remove 防陈旧残留
- **quest_loop 恢复通路三条**：CheckpointLoaded 确定性重建（主通路）+ QuestProgressUpdated 惰性自愈（兜底启动时序）+ QuestCreated 原路径；卫星态缺失降级等价重建（可用性优先）
- **诚实局限（登记）**：step_rewards 运行态不还原；LongTaskMap detail 内容（ExternalStorage trait 对象）不可序列化（from_bytes_with 已提供重注机制）；map_id 会话级重生成

## D3 — w4 来源维度预埋+默认开启（WP-D，提交 6310890c）

用户裁决偏离蓝图「默认 off」：直接默认开启（sw=0.10），安全阀替代对照期。

- **L0 SelectorWeights 三维契约不动**（关键裁决）：omega-learner S4 学习器的 arm 权重空间是三维 SelectorWeights 的冻结面（R2）——加第四维波及 arm 空间；w4 权重落 `HcwConfig.source_weight`（hcw-local，#[serde(default)]），**爆炸半径归零**
- **公式**：`score = (w1×(1-sw))×rec + (w2×(1-sw))×freq + (w3×(1-sw))×rel + sw×src_factor`——三项 (1-sw) 归一保总权重恒 1.0
- **单一真值源归一**：assemble.rs 提取 `pub fn source_value_factor`（六来源系数表：System/User 1.0、Memory 0.8、Retrieval 0.7、Assistant 0.5、Tool 0.3——沿用吸收成果标定），density() 与压缩评分共用一表
- **退化语义**：None 条目 src_factor=0.5 中性（对齐 DEFAULT_RELEVANCE 无 CLV 先例）——来源未知不奖不罚，存量条目均匀偏移、None 集内相对排序不变
- **回退开关**：env `CHIMERA_W4_DISABLED=1`（启动期 OnceLock，CHIMERA_NO_PARALLEL_HCW 先例同款；`effective_source_weight` 单点归一防调用方漏读 env）→ sw 视为 0 → **逐位回三维旧公式**（测试锁定位模式等价）
- **ContextEntry.source**：`Option<ContextSource>`（#[serde(default)] 旧 checkpoint 兼容；with_source 链式标注；new 缺省 None 零破坏）
- **策略路径隔离**：SelectorLearnerHolder 委托传 0.0 维持三维——w4 只挂 compress 主路径，S4 接缝不引入第四维

## 验收汇总

| WP | 提交 | 测试 | clippy |
|----|------|------|--------|
| WP-C | 6fcc15fb | hcw 全 501/0（+9 governor 测试）+ window_select bench ns 级零回退 | 干净 |
| WP-D | 6310890c | hcw 全 504/0（+3 w4 行为锁）| 干净 |
| WP-H | be0e92ba | quest-engine 422/0（基线 414+8）+ chimera-cli check + quest_loop lib 3/0 | 干净 |
| WP-E | 992515a6 | session-store + nexus-app-server 135/0 + 404 桥 5/0 + ledger_parity_locks 5/0 | 干净 |
| 吸收 | 601e44e1 | 六 crate 986/0 + TUI lib 1015/0 + cli 触及面 70/0 | — |
| 锁补齐 | 5696b634 | panorama 5/0（154→156 同步补齐，R9 WP-0 漏点） | — |

组合态回归（quest_lifecycle E2E + chimera-cli cli + 四 crate 组合）见执行报告 §5。

## 回滚

吸收提交独立可 revert；每 WP 独立 commit；w4 有 env 开关；governor 静态回退默认；P3 桥 append-only；sidecar 可整目录删除降级；锁 5 翻转随 WP-E revert 即恢复。
