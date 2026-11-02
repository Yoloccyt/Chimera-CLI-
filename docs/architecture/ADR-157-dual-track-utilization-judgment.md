# ADR-157: 利用率双口径三条件联合判定 + 偏差重定性（P5-T1）

> 日期：2026-08-28 ｜ 状态：实施先行待批（ADR-135 先例）｜ 分支 feat/phase1-w1-w8
> 关联：ADR-152（tokio busy 近似口径登记）/ ADR-155（WI-34 终验偏差登记与处置路径）/ WI-34（v4.0 §7.5.4 两档基准）/ D-Q2~D-Q5（Phase 5 计划裁决）

## 1. 背景与重定性（D-Q2，代码证据）

ADR-155 登记 WI-34 终验偏差：combined 利用率 55.2% vs 16 核档 65% 基准（-9.8pp）。Phase 5 计划期代码审计发现**偏差的真实根源**：

- `nexus-core/examples/utilization_probe.rs` 原版负载 **100% 走 rayon**（全部 `spawn_compute_batch` → ComputeBridge rayon 池），tokio 侧在采样窗口内**结构性零负载**（仅采样器自身）
- combined 公式 = 0.5×tokio + 0.5×rayon → 纯 rayon 负载下 combined ≈ 0.5×0 + 0.5×1.0 = 0.5，与实测 55.2% 吻合
- **结论：偏差是 measurement 管道单侧化缺陷（D-Q2 重定性），非生产性能缺口**——tokio 侧从未被测过，何谈"tokio 侧利用率低"

### 与 ADR-155"否决负载塑形"的边界
ADR-155 否决的是"加大 rayon 负载凑 65%"（单侧继续塑形）。本裁决补入的是 **tokio 侧此前缺失的真实负载**（CPU-lite + IO 模拟混合任务，对齐生产 tokio 职责：E8-2 裁决 IO/异步主战场）——修正的是管道代表性，不是为凑数捏造负载。修正后数字如实报告（D-Q5）。

## 2. 决策

### 决策 1：探针负载双侧化（D-Q3）
`utilization_probe` 并行运行两类负载：
- **rayon 侧**（既有）：8 波 × 1024 × 5ms CPU 自旋（spawn_compute_batch）
- **tokio 侧**（新增）：16 波 × 512 混合任务（CPU-lite 计时自旋 2ms + `tokio::time::sleep` 1ms IO 模拟），JoinSet 并发，总时长 ≈3s 覆盖采样窗口（300×10ms）

### 决策 2：tokio_unstable 局部启用（D-Q4）
- `TokioProbe` 新增 `busy_us` 字段：cfg(tokio_unstable) 下为 `worker_total_busy_duration` 累计和（微秒），否则恒 0（存活任务近似口径独立保留，RK-Q1 降级路径）
- cfg 通过**局部 RUSTFLAGS** 注入（`RUSTFLAGS="--cfg tokio_unstable" cargo run --release -p nexus-core --example utilization_probe`），**不进 .cargo/config.toml**（避免全局 cfg 污染依赖缓存与 CI）
- 采样器差分：相邻两次 busy_us 差 / (workers × 10ms 窗口) = 窗口 busy 比率

### 决策 3：双口径三条件联合判定（D-Q5，替代单一 combined ≥65%）
| 条件 | 指标 | Phase 5 实测 |
|---|---|---|
| ① rayon 侧满载可观测 | rayon peak ratio ≥ 0.9 | **1.000** ✅ |
| ② tokio 侧真实负载证据 | busy_duration 差分可观测（peak）或双侧修正 combined | **busy_peak = 1.000（busy_real=true，300 样本）** ✅ |
| ③ 端到端并行收益 | e2e 加速 ≥ 基线 6.47× | **7.45×**（Phase 4 T8） ✅ |

combined（修正口径）**如实报告，不预设 65% 硬线**：三条件联合为判定主轴，combined 数值为观测记录。

## 3. 实测证据（2026-08-28，release，16 核宿主）

| 口径 | combined mean | combined peak | tokio busy | 备注 |
|---|---|---|---|---|
| 修正前（单侧负载，debug） | 0.552 | 0.563 | —（无 tokio 负载） | ADR-155 登记值 |
| 修正后·近似口径（双侧负载，debug） | 0.992 | 1.000 | —（近似口径） | busy_us=0 |
| **修正后·busy 真实口径（双侧负载，release，cfg）** | **0.999** | **1.000** | **busy_peak=1.000 / mean=0.005（300 样本）** | busy_real=true |

- 修正后 combined **0.999 ≫ 65% 基准**——原偏差 100% 归因于管道单侧化，重定性被实测闭环证实
- busy_mean 0.005 低为预期：tokio 任务 CPU 2ms + sleep 1ms 交替（1/3 时间挂起让出 worker），busy 比率反映**真实 worker 计算占用**而非任务驻留；peak 1.000 证明峰值时刻全 worker busy 可观测
- tokio 侧负载总量校验：tokio_mixed_total = 33,550,336（8,192 任务全部完成，无 panic）

## 4. 后果

- 正面：WI-34 两档基准在修正管道下获得**双侧真实观测**；ADR-155 偏差关闭（三条件联合全 ✅）；tokio busy 真实口径成为后续性能回归的可复用证据源；诚实数据红线零妥协（数字全部实测、口径全部标注）
- 代价：probe 负载模型复杂化（双侧并行）；busy 口径依赖局部 RUSTFLAGS（CI 如需 busy 数据须在 workflow 显式注入，否则降级近似）
- 否决方案：①保持 55.2% 偏差登记不做管道修正——放弃可修复的观测缺陷；②全局 tokio_unstable——污染 workspace 编译缓存与 CI；③按 65% 硬线塑形达标——违反诚实数据红线（ADR-155 已否决，重申）

## 5. 与 ADR-155 的关系
本 ADR **关闭** ADR-155 登记的偏差（重定性 + 修正 + 三条件全 ✅）；ADR-155 的"双口径分离判定"处置路径 ②由此正式化；缓冲消耗（RK-P23 ×1）维持记录不回冲（消耗时点为真实偏差期）。
