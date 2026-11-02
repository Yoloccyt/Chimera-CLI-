# ADR-071: 召回基准与生产哨兵

> **状态**: Proposed (2026-08-03 落档,v2.20.0-omega)
> **作者**: 8 位虚拟专家分布式深度分析 + superpowers-main + staff-engineer-mode
> **关联计划**: `docs/planning/HCW-Sparse_PROBE实施计划_v1.0.md` §2.2 (P0 召回评测基线"尺子")
> **关联进度**: `docs/planning/PROBE_实施进度报告.md` (P0.5 双基线对照表)
> **依赖**: ADR-070 (PROBE HCW-Sparse 深度优化总纲) / ADR-043 (R1 影子模式) / ADR-034 (灰度=能力场)

---

## 1. 背景与问题

HCW 召回路径(v2.4.0-omega 之前)无量化评测尺子,**召回质量(needle 维度)零度量**。PROBE P-1 阶段 P-1.1 修订 PROBE 设计文档时确认:
- 仓库 grep `needle` 零命中(needle/多针/位置扫描/多跳四类评测尺子全部缺失)
- 块填充召回率基准(RECALL_RATE_THRESHOLD=0.95)与 5 个 p95 红线测试已存在(性能维度,非质量维度)
- 静态"取前 k 块"路径多针召回仅 0.25、位置偏置 0(H4 病理实锤)、多跳全失败(H3 缺口量化)

**核心矛盾**: 优化措施能否提升召回,缺乏量化裁决依据。

---

## 2. 决策

7 项工程决策:

### 决策 1: 4 类评测尺子(needle/position/chain/recall@tier)
- **问题**: 召回质量需要可量化的多维度指标
- **决定**: 引入 4 类评测尺子:
  - **needle@8**: 单针/多针召回率(目标 ≥0.90 探针路径 vs 0.25 静态)
  - **position_bias**: 位置偏置(理想 1.0,衡量摘要/重排对抗 lost-in-the-middle 的效果)
  - **chain_success_rate**: 多跳链式成功率(目标 ≥0.80 探针路径)
  - **recall@tier**: 各档位基础召回率(L0-4K / L1-32K / L2-128K / L3-1M)
- **影响**: 100% 替代"4944+ lib tests"矛盾口径;PROBE P0.2 实现 1016 行代码(`src/recall/eval/{mod,needle,position,multihop,report}.rs`)

### 决策 2: 四档窗口双基线对照
- **问题**: 不同窗口档位召回能力不同
- **决定**: L0-4K / L1-32K / L2-128K / L3-1M 四档 × 静态/探针 两种路径 = 8 组对照,固定种子可复现
- **影响**: 探针优势 +0.75 点 needle@8 在四档一致(实测 1.0 vs 0.25),P1 接线的正当性获量化证据

### 决策 3: HcwRecallReported/Degraded 事件归一化
- **问题**: 召回质量需要可观测、可自动升档
- **决定**: event-bus 追加 2 个事件(126→128):
  - `HcwRecallReported` (Normal 级,周期报告)
  - `HcwRecallDegraded` (Normal 级,降级告警)
- **归一化**: `membrane categorize=ReadMetric` + `topic=System` + `severity=Normal`(不扩容 Critical 清单);`severity()` 通配分支返回 Normal(红线验证点)
- **影响**: TUI `OsaSparse` 面板五维掩码段可读数;`efficiency-monitor::RecallCollector` 订阅消费

### 决策 4: RecallSentinel `warn_only` 模式
- **问题**: 自动升档可能引入误报
- **决定**: `RecallSentinel` 默认 `warn_only=true`,命中只发布 `HcwRecallDegraded` + 日志,**不升档不 update_policy**(升档由 ADR-043 影子期裁决)
- **判定语义**: 复用 `below_baseline_80(needle < baseline × 0.8)`;**连续 2 次**低于基线 → DegradedWarn;健康复位
- **影响**: 误报风险最低;影子模式观察期(14 天)结束后由人工/规则触发 promotion

### 决策 5: feature `hcw-recall` 编译期门控
- **问题**: 评测 harness 不应污染生产二进制
- **决定**: `feature hcw-recall` 默认 on(ADR-034 编译期门控);`forbid(unsafe_code)` 继承;f32 全程红线
- **影响**: 单元测试 + doctest 全绿(`hcw-window` lib 245→245+,全量 23 doctest 通过)

### 决策 6: RecallCollector EWMA α=0.3
- **问题**: 召回统计需要抗噪
- **决定**: `efficiency-monitor/src/recall_collector.rs` `RecallCollector/RecallStats` 使用 EWMA α=0.3(`subscribe` 先于 `spawn` 红线)
- **影响**: 端到端测试可演示事件→collector→TUI 链路;TUI lib 641 全绿(含 3 个召回 sync 测试 + 面板 recall_line 渲染,serde(default) 向后兼容)

### 决策 7: A/B 回归闸(任一召回项下降不合并)
- **问题**: 优化措施可能引入回归
- **决定**: `tests/probe_ab_test.rs` 128K 档 A/B 对照:needle@8 ≥ 90% / bias ≥ 0.85 / chain ≥ 80% 三项全达标;任一下降不合并
- **影响**: 实测 static 0.25/0/0 vs probe 1.0/1.0/1.0;**全指标达标 + 无召回下降**

---

## 3. 双基线对照表(PROBE P0.5 冻结,固定种子可复现)

| 路径 | recall@tier | needle@8 | bias | chain | 解读 |
|------|-------------|----------|------|-------|------|
| L0-4K static | 1.000 | **0.250** | **0.000** | **0.000** | 只命中头部 2 针;位置偏置/多跳全灭 |
| L0-4K probe | 1.000 | **1.000** | **1.000** | **1.000** | CLV 探针全中 |
| L1-32K static | 1.000 | 0.250 | 0.000 | 0.000 | 同上(四档一致) |
| L1-32K probe | 1.000 | 1.000 | 1.000 | 1.000 | |
| L2-128K static | 1.000 | 0.250 | 0.000 | 0.000 | |
| L2-128K probe | 1.000 | 1.000 | 1.000 | 1.000 | |
| L3-1M static | 1.000 | 0.250 | 0.000 | 0.000 | |
| L3-1M probe | 1.000 | 1.000 | 1.000 | 1.000 | |

**对照结论**: 静态路径多针召回仅 0.25、位置偏置 0(H4 病理实锤)、多跳全失败(H3 缺口量化);CLV 探针路径四指标全 1.0。**探针优势 +0.75 点 needle@8**,P1 换刀头的优化空间与方向获量化证据。

---

## 4. 实施阶段(全部验收通过)

| 阶段 | 任务 | 关键交付物 | 状态 |
|------|------|-----------|------|
| P0.1 | CLV 判别力预检 | `tests/probe_clv_separation_test.rs` 4 测试(分离度 0.155) | ✅ |
| P0.2 | recall/eval 四类评测 harness | `src/recall/eval/{mod,needle,position,multihop,report}.rs` 1016 行 + feature `hcw-recall` | ✅ |
| P0.3 | event-bus 追加 2 事件 | `HcwRecallReported` / `HcwRecallDegraded`(126→128) | ✅ |
| P0.4 | efficiency-monitor collector + TUI 读数 | `recall_collector.rs` + TUI OsaSync/TuiState/DataSnapshot/pipeline/OsaSparsePanel | ✅ |
| P0.5 | 双基线对照表 | `tests/recall_baseline_test.rs`(`#[ignore]` release) | ✅ |

---

## 5. 验证基线

- 召回评测 harness 单元测试 + doctest 全绿(`hcw-window` lib 245→245+,全量 23 doctest)
- `efficiency-monitor` 152+16+3+1 全绿
- TUI lib 641 全绿(含 3 个召回 sync 测试 + 面板 recall_line 渲染)
- 双基线对照断言(探针 needle@8 ≥ 静态、静态 bias < 0.6、探针 ≥ 0.75)全绿
- `select_nth_unstable_by` Top-K 红线通过;f32 全程
- `severity()` 通配分支返回 Normal(红线验证点)

---

## 6. 关联 ADR

- **前置**: ADR-070 (PROBE 总纲) / ADR-043 (R1 影子模式) / ADR-034 (灰度=能力场)
- **平行**: ADR-069 (Token 效率优化,同样引入消费者观测事件)
- **后续**: ADR-054 (Phase 9 三环重组,召回评测可作为三环重组的健康度指标)

---

> **签署**: 待 Phase 9 启动时由治理委员会晋升 Accepted
> **最后更新**: 2026-08-03
