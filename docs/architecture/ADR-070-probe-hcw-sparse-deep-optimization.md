# ADR-070: PROBE HCW-Sparse 深度优化总纲

> **状态**: Proposed (2026-08-03 落档,v2.20.0-omega)
> **作者**: 8 位虚拟专家分布式深度分析 + superpowers-main + staff-engineer-mode
> **关联计划**: `docs/planning/HCW-Sparse_PROBE实施计划_v1.0.md` (PROBE 实施计划 v1.0-impl)
> **关联进度**: `docs/planning/PROBE_实施进度报告.md` (P-1/P0/P1/P2/P3 全部阶段验收通过)
> **依赖**: ADR-043 (R1 影子模式) / ADR-034 (灰度=能力场 + 编译期 feature) / ADR-033 (L0 nexus-contracts)

---

## 1. 背景与问题

HCW (Hierarchical Context Window) 是 L2 记忆层的核心子系统 (`crates/hcw-window`)。v2.4.0-omega 阶段 PROBE 设计文档提出"召回优先 + 延迟换召回"优化目标,但文档基线严重失真(声称 v2.3.0/37 crates/112 事件变体,与代码实况 v2.19.0-omega/38 crates/126 事件变体全部不符)。

**病理诊断**:
- **H1 召回质量零度量**: 仓库 0 命中 needle/多针/位置/多跳评测(实测全仓库 grep `needle` 零命中)
- **H2 路径分叉**: recall/ 流水线虽已实现(三级召回 coarse/fine/rerank/streaming),但 chimera-mas ContextManager 仍走老路径 `select_window + apply_sparse_mask`
- **H3 缺评测尺子**: 无 needle/position/chain 量化指标,无法判定优化方向
- **H4 路径阻塞**: 静态"取前 k 块"路径多针召回 0.25,位置偏置 0,多跳全失败(0/0/0)
- **H5 文档失真**: PROBE 设计文档引用的 ADR 编号与基线冲突
- **H6"建好未接线"**: `s4_selector.rs:66` 声称的 `HcwWindow::update_selector_policy()` 注入 API 实际不存在

**优化方向**:
- 探针打分(CLV-based 探针替代相关性维度,提供 0.75 点 needle@8 提升空间)
- 位置重排(对抗 lost-in-the-middle 现象,top-2 置头/次高置尾)
- 三区结构(zone_fill:sink 恒留 + 配额 + 摘要中段)
- 反馈闭环(RecallSentinel 哨兵 + 影子模式 2 周观察)

---

## 2. 决策

8 项工程决策:

### 决策 1: 文档基线勘误(PROBE 设计文档失真修订)
- **问题**: PROBE 设计文档声称"v2.3.0/37 crates/112 事件变体/64 ADR",全部与代码实况不符
- **决定**: 实施前必须先完成 P-1 基线勘误(硬前置),废弃旧基线声明,以代码实况(v2.20.0-omega/38 crates/129 事件变体/71 ADR 含 ADR-070/071)为权威
- **影响**: PROBE 计划文档 v1.0-impl 锁定新基线;`docs/planning/HCW-Sparse_PROBE实施计划_v1.0.md` §1.1 三方基线对照表为权威裁决

### 决策 2: Append-only 与零新 crate 原则
- **问题**: 是否新增独立 crate 承载 PROBE 能力
- **决定**: **38 → 38 不变**,能力增量全部落入既有 crate(主战场 `hcw-window`,辅战场 `omega-learner`/`event-bus`/`chimera-mas`/`chimera-cli`)
- **影响**: 维护成本与依赖铁律零增量;与 ADR-049 (polish v2.7 裁剪"0 新 crate 全子模块落地")一致

### 决策 3: R2 冻结零触碰
- **问题**: PROBE 涉及学习环路(omega-learner),与 R2 冻结(ADR-042)潜在冲突
- **决定**: 影子模式零改动复用 `crates/omega-learner/src/shadow_mode.rs`,四条件(14 天 + 71.4% + EWMA ≥0.7 + 无 ASA)与回滚预案(连续 3 天退化 / EWMA 崩塌 0.3 / 召回下降 5%)全部走 git diff 为空验证
- **影响**: CI `r2_freeze_violation_e2e` 持续全绿;PROBE 不触 gsoe/auto-dpo

### 决策 4: ADR-043 解冻阈值复用
- **问题**: 探针路径 promotion 需要门槛
- **决定**: 14 天观察期(L49) / 胜率 ≥71.4%(L59) / EWMA ≥0.7(L65) 全部复用常量;一次性注入 `Learned` 策略(`promotion_injected` 幂等护栏),影子期主路径恒 Static
- **影响**: 与 ADR-043 影子模式无缝衔接;真正的"召回提升"需经历 2 周观察期

### 决策 5: 探针打分 `probe.rs` 不入 nexus-contracts
- **问题**: 探针权重 `ProbeWeights` 类型归属
- **决定**: `ProbeWeights` 定义于 `hcw-window` 内部,**不入 nexus-contracts**(避免触碰 L0 零逻辑约束 ADR-033)
- **影响**: L0 契约零改动;类型下移空间被否决(理由:ADSR-033 L0 trait 解耦模式已就位,无必要下移)

### 决策 6: 三区结构(zone_fill)sink 恒留
- **问题**: 提示构建时摘要可能挤压系统提示(sink)
- **决定**: `ZoneFillConfig`/`ZoneFillInput`/`fill_zones()` 三区结构:head(top-2 探针) + body(temporal) + tail(次高探针) + sink 区(系统提示恒留不被挤出)
- **影响**: 6 测试全绿(sink 恒留/配额不变量/摘要中段/中段密度序/fill 零回归);`RerankFillConfig` ~8 处构造零改动

### 决策 7: A/B 回归闸
- **问题**: 优化措施可能引入回归
- **决定**: 静态 vs 探针路径 A/B 对照测试 `tests/probe_ab_test.rs`;**任一召回项下降不合并**(needle@8 ≥ 90% / bias ≥ 0.85 / chain ≥ 80% 三项全达标)
- **影响**: 实测 static 0.25/0/0 vs probe 1.0/1.0/1.0;**探针优势 +0.75 点 needle@8**

### 决策 8: `HcwWindow::update_selector_policy()` 注入 API
- **问题**: S4 学习的策略无法注入到选择器
- **决定**: `HcwWindow.selector_learner` 字段 + `with_learner` 构造器 + `update_selector_policy()`/`current_selector_policy()` 方法;sync 无跨 await;不失效 repr_clv 缓存(缓存与策略权重正交)
- **影响**: 20+ 构造点零破坏;两处 compress 调用点接线 holder 策略优先;`chimera-cli` 端通过 `SelectorOrchestrator::emit_policy` 链路闭环

---

## 3. 实施阶段(全部验收通过,详见 PROBE 实施进度报告)

| 阶段 | 任务 | 关键交付物 | 状态 |
|------|------|-----------|------|
| P-1 | 基线勘误(硬前置) | PROBE 设计文档 v1.1 + recall/ 资产映射表 + 测试基线 8455 + 巡检脚本 F 类 + R7 降级预案 | ✅ 5/5 |
| P0 | 召回评测尺子 | `recall/eval/{needle,position,multihop,report}.rs` 1016 行 + feature `hcw-recall` + HcwRecallReported/Degraded 事件(126→128) + RecallCollector + 双基线对照表 | ✅ 5/5 |
| P1 | 探针选择器刀头(W3-W6) | `probe.rs` 284 行 + 三区结构 + repr_clv 缓存 + 位置重排 + 生产接线 + 快照通路 + 双性能红线(p95<10ms 实测 566μs,17.7× 余量;吞吐 ≥10K 实测 509,966 块/秒,51× 余量) + A/B 验收 | ✅ 8/8 |
| P2 | 反馈闭环(W7-W9) | T1 注入 API + T2 编排器接线 + T3-A S4 臂 5→29(24 探针臂) + T3-B RecallSentinel + T4 影子模式评估链 | ✅ T1/T2/T3-A/T3-B/T4 |
| P3 | 结构兜底(W10) | P3.1 effective_fold(宣称×60%) + P3.2 OverWindowFallback(超窗兜底链) + P3.3 发布收尾 | ✅ P3.1/P3.2,P3.3 进行中 |

---

## 4. 性能红线

| 指标 | 红线 | 实测 | 余量 |
|------|------|------|------|
| 探针装窗 p95 | <10ms | **566μs** | 17.7× |
| 打分吞吐 | ≥10K 块/秒 | **509,966 块/秒** | 51× |
| 旧 window_select | <1ms(不放宽) | 既有守护 | — |
| S4 select 29 臂 | <50μs | **5.77μs** | 8.7× |

---

## 5. 验证基线

- 全量测试: **8455 passed / 1 failed / 71 ignored(303 个 test target)**
- clippy: 零警告(`--all-targets -D warnings`)
- fmt: 零差异
- L0 契约零改动
- 零新 crate(38→38 不变)
- 影子模式评估链 `git diff crates/omega-learner/src/shadow_mode.rs` 为空
- A/B 回归闸全指标达标(needle@8 = 1.0 vs 0.25, +0.75 点)

---

## 6. 后续路径

- **P3.3 发布收尾**: ADR-070/071 落档 + tag 推送 + CHANGELOG 校准
- **Phase 9 三环重组**: v3.0.0-omega 启动,ADR-054 激活;ADR-048 (gqep-executor L7→L4 跨层渗透) 根治路径
- **长期演进**: v3.2.0-omega Harness Engineering 终极形态

---

## 7. 关联 ADR

- **前置**: ADR-033 (L0 nexus-contracts) / ADR-034 (灰度=能力场) / ADR-042 (R2 冻结) / ADR-043 (R1 影子模式)
- **平行**: ADR-057 (OSA Memory S2) / ADR-069 (Token 效率优化)
- **后续**: ADR-054 (Phase 9 三环重组) / ADR-071 (召回基准与生产哨兵)

---

> **签署**: 待 Phase 9 启动时由治理委员会晋升 Accepted
> **最后更新**: 2026-08-03
