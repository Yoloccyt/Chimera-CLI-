# GPU 三门重开就绪度评估（R3-4，2026-11-08）

- **性质**：纯评估报告（零代码、零依赖变更）；回应阶段四方案对「GPU 资源利用率」维度的处置裁决——**维持 C-15 三重关闭，CPU 计算面极致优化替代**（用户 2026-11-07 裁决）
- **权威源**：v3.2 文档 C-15（`docs/architecture/_archive/2026-Q4/root/Chimera架构重构与优化方案_v3.2-L0L3深度打磨版.md` L97/L180/L416）+ ADR-195 + G-86 机器门（`scripts/gate_manifest.toml:960`，`scripts/check_accel_exclusion.py`）

## 1. 封印现状复核（三重关闭逐项现取）

| 关闭层 | 内容 | 2026-11-08 现取 | 状态 |
| --- | --- | --- | --- |
| ① ADR-195 | GPU 异构栈否决决策 | adr_index 在册（G-86 spec_ref=ADR-195），未被任何后续 ADR 撤销 | ✅ 仍生效 |
| ② G-86 机器门 | Cargo.lock 中 wgpu/wgpu-core/wgpu-hal/naga/ash/d3d12/candle/burn/ort 族 0 命中 | `check_accel_exclusion.py` 为 gate_manifest 在册门（M0 后 mismatch=0 批次成员）；本批 R1~R3 未引入任何新依赖（mimalloc 评测属 R4、尚未实施） | ✅ 仍绿 |
| ③ 三门证伪 | G-A/G-B/G-C 重开门均不满足 | 见 §2 逐门对照 | ✅ 均不满足 |

## 2. 三门逐门对照（重开条件 vs 现势）

### G-A · 性能门（CPU 计算面出现 SLO 红线）

- **重开条件**：现有 CPU 计算面 SLO 不再全绿（v3.0 §4 证伪条件一的反面）。
- **现势**：`perf_redlines.toml` 11 条硬红线 + 6 条 SLO 警戒线全绿
  （`check_perf_redlines.py --static-only` rc=0，2026-11-08 R2 波次实测）；
  R1 判决体优化后派发链热路径**大幅变快**（ADR-260：hit 93.5%/miss
  82~99.7%），计算面压力方向是下降而非逼近瓶颈。
- **传感器**：`slo.yml` 日跑 + `perf_thresholds.toml` 计算族键
  （`nmc_parallel_encoding`/`faae_*`/`hnsw_tiered`/`memory_graph_recall_*`/
  `osa_sequential_baseline` 等）。
- **判定：不满足**（远未满足——热路径刚被优化 30~50%）。

### G-B · 载荷门（生产常驻相似度/压缩载荷自然增长）

- **重开条件**：v3.0 §6 与 v3.2 :338 指认的唯一诚实松动路径——
  **功能接线（dedup/MemoryGraph/写路径接生产）使 GPU 类批量相似负载成为
  生产常驻**，且 CPU 面实测出现拐点数据。
- **现势**：MemoryGraph 挂桥（WI-G9，`recall_with_graph` 全库唯一未挂桥
  生产相似点）尚未接线；dedup/写路径无新增生产常驻批量相似负载。
  R1~R3 波次未新增任何常驻计算载荷。
- **判定：不满足**（无新载荷、无拐点数据）。

### G-C · 架构门（结构条件放开）

- **重开条件**：v3.0 §4 的 D2 四面随重启放开 / 可达性差距（2.6×~4 数量级）
  消除；工程前置 = forbid(unsafe_code) × bytemuck::Pod（wgpu 路径）、
  ort 仅 MSVC 破 windows-gnu 钉定（G-86 note 原文）两大未决。
- **现势**：`#![forbid(unsafe_code)]` 全仓红线未变；windows-gnu 工具链钉定
  未变；DispatchPlan 仍为 `{Inline, Rayon, Async}` 三态
  (`crates/nexus-core/src/compute/dispatch.rs:23-30`，无 Gpu 分支)。
- **判定：不满足**（两大工程前置均未解决）。

## 3. 本轮 CPU 计算面极致优化的替代成果（R1~R3）

| 项 | 成果 | 证据 |
| --- | --- | --- |
| R1 判决体 | 索引 O(1) + 零分配 glob + 无锁统计：miss 82~99.7% / hit 93.5% | ADR-260 §4 |
| R3-1/R3-2b | `CHIMERA_RT_WORKERS` / `CHIMERA_COMPUTE_THREADS` 双旋钮（tokio 池 × 计算池核数显式划分） | ADR-262 |
| R3-2 双池 spike | 见 ADR-262 §3（hnsw_rs 全局池并存的量化与三选一裁决） | ADR-262 |

计算面优化后 GPU 的边际收益区间进一步收窄（GPU 引入的动机本来是计算瓶颈，
而瓶颈刚被消掉一个大头）。

## 4. 监测仪表（何时该重开本评估）

以下任一信号触发时应重跑本评估并考虑启动三门程序：

1. `slo.yml`/`bench_check.yml` 任一计算族键连续 3 次越阈值（CPU 面退化且
   无法用池旋钮救回）；
2. MemoryGraph/dedup/写路径功能接线落地后，`memory_graph_recall_*`/
   `dedup_semantic` 键的实测载荷量级较基线上移 ≥10×；
3. 全仓 `forbid(unsafe_code)` 红线或 windows-gnu 钉定被正式裁决放开
  （G-C 工程前置消除）。

## 5. 结论

**维持 C-15 三重关闭**。三门全部不满足且方向性证据（R1 优化）在远离门槛。
GPU 维度在本阶段的正确形态 = CPU 计算面极致优化（已交付）+ 本报告的监测
仪表（防盲飞）。不引入 wgpu/candle/burn，不建第三运行时——与 v3.2 C-15
原文逐字一致。
