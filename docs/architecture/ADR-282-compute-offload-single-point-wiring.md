# ADR-282 -- 计算面卸载点位：无生产调用方 / 低于阈值 → 全部不接线（维持 G-87）

- 状态：Accepted（2026-11-10）
- 决策者：W4 波次
- 关联：ADR-195（GPU 三重关闭；「不预投不可达点位」）/ ADR-196 C1（TaskKind 拆位）/ ADR-199（`spawn_compute` 删除）/ ADR-262（双池定界）/ G-87（卸载可达性冻结）/ G-88（同步卸载让出）
- 影响面：**零生产代码、零 freeze 台账变更**；仅本 ADR 与索引
- 行为影响：无

## 1. 背景

`scripts/compute_offload_reachability_freeze.txt` 的 9 个 `spawn_compute_batch` 锚点：

| 锚点 | G-87 裁决 | 调用方（freeze 原文） |
|---|---|---|
| `chimera-cli/overwindow_bridge.rs::dense_scores_pooled` | **WIRED_COMPOSITION** | `build_provider` / `compute_provider_parts` |
| `decay-engine/parallel.rs::decay_batch_parallel` | **DEAD** | — |
| `faae-router/parallel.rs::score_parallel` | **DEAD** | — |
| `faae-router/parallel.rs::score_scoring_parallel` | CRATE_API_LIVE | `faae-router/src/router.rs::route` |
| `gsoe-evolution/fitness_parallel.rs::score_batch` | **DEAD** | — |
| `hcw-window/parallel.rs::score_parallel` | PUB_API_DIAGNOSTIC_ONLY | `compressor.rs::compress` / `window.rs::select_window` |
| `nmc-encoder/parallel.rs::perceive_parallel` | **DEAD** | — |
| `osa-coordinator/parallel.rs::compute_five_masks_parallel` | **DEAD** | — |
| `repo-wiki/vector/parallel.rs::knn_batch_parallel` | CRATE_API_LIVE | `repo-wiki/src/vector.rs::search_batch` |

即：**5 DEAD + 2 CRATE_API_LIVE + 1 诊断面 + 1 WIRED**。每个 `parallel.rs` 头部都写有
「接线三要件」（错误姿态 / G-88 让出 / 逐位等价），说明接线是**已设计未实施**，不是遗漏。

## 2. 方法：接线必须同时满足三前置

1. **有生产调用方**（组合根可达，非测试/bench/crate API 自证）；
2. **真实 N ≥ 该 `TaskKind` 阈值**（否则 `ComputeBridge::route` 直接 `Inline`，收益为 0）；
3. **端到端 ≥5%**（ADR-239 既有门槛；criterion 同轮 A/B）。

三条缺一即不接线。只看「并行函数存在」或「方向为正」不足以接线（ADR-249/263 的反面教训）。

## 3. 逐点裁决

阈值权威源：`crates/nexus-core/src/compute/dispatch.rs:38-65`（文档投影）+
`crates/nexus-core/src/compute/hts.rs:120-160`（`default_entry` 唯一值定义点）。

| 卸载点 | TaskKind | 阈值 | 生产调用方 | N 来源 / 量级 | 裁决 |
|---|---|---|---|---|---|
| `decay_batch_parallel` | `DecayBatch` | **10,000** | **无**（DEAD） | 能力注册表条数（tens） | **不接线**：无调用方 + N≪阈值 |
| `faae::score_parallel` | `ExpertScore` | **10,000** | **无**（DEAD） | 专家数（tens） | **不接线** |
| `nmc::perceive_parallel` | `NmcEncode` | **1,000** | **无**（DEAD） | 输入批大小（个位数） | **不接线** |
| `osa::compute_five_masks_parallel` | `OsaMask` | **100** | **无**（DEAD） | profile 批（约 60） | **不接线**（最接近阈值；若未来出现 ≥100 profile 的调用方再开） |
| `gsoe::score_batch` | `GsoeEvaluate` | **500** | **无**（DEAD；`select_elite` 未接线） | rollout 种群（调用方定） | **不接线** |
| `faae::score_scoring_parallel` | `ExpertScore` | **10,000** | CRATE_API_LIVE（`route`，非组合根） | 专家数（tens） | **不接线**：N≪阈值 |
| `repo-wiki::knn_batch_parallel` | `KnnSearch` | **5,000** | CRATE_API_LIVE（`search_batch` 仅 bench 调用） | 查询批（tens） | **不接线**：无生产调用方 |
| `hcw::score_parallel` | `HcwEntryScore` | **200** | PUB_API_DIAGNOSTIC_ONLY（G-88 GUARDED） | 条目数（调用方定） | 维持诊断面，不新增接线 |
| `overwindow::dense_scores_pooled` | `ClvSimilarity` | **1,000** | **WIRED_COMPOSITION** | chunk 批 | **保持**（唯一生产卸载） |

**结论：本波不接线任何点位。** 计算面并没有「空转」——
`ComputeBridge` 的三态设计本来就是「小批量 Inline、大批量 Rayon」；把低于阈值的点强行
接上只会让它路由到 `Inline`（零收益）或绕过阈值直上 rayon（增加调度开销）。
把无调用方的点接上则是「预投不可达点位」（ADR-195 D2 明确排除）。

## 4. 为什么这不是「不作为」

- **唯一 WIRED 点（overwindow）已在生产**，且其 `join_chunked` 失败整批回落 inline
  （`chimera-cli/src/overwindow_bridge.rs:396-402`）—— 卸载面的「生产可达」不是 0。
- **N 不是「永远小」**：`OsaMask` 阈值 100 是十类里最低的，profile 批约 60 已接近；
  一旦有调用方批量 ≥100，本 ADR 的重开条件即满足。
- **G-87 是机器门**：接线会改变 exact-set 锚点，必须同批更新 freeze；在无调用方时更新
  锚点等于给死代码发「已接线」证书，比不接线更坏。

## 5. 逐点重开条件（出现即按三前置重新裁决）

| 点 | 重开条件 |
|---|---|
| `decay_batch_parallel` | 出现组合根可达的批量衰减调用方，且单次 N ≥ 10,000 |
| `faae::score_parallel` / `score_scoring_parallel` | 专家数 ≥ 10,000，或经单独 ADR 下调 `ExpertScore` 阈值并附实测 |
| `nmc::perceive_parallel` | 批量编码调用方且单批 ≥ 1,000 |
| `osa::compute_five_masks_parallel` | 出现 ≥100 profile 的批量调用方（最接近） |
| `gsoe::score_batch` | `select_elite` 进入生产路径且种群 ≥ 500（R2 约束仍不动） |
| `repo-wiki::knn_batch_parallel` | 出现生产批量检索调用方且查询批 ≥ 5,000 |
| `hcw::score_parallel` | 从诊断面升格为生产调用方；同时满足 G-88 |

## 6. 验收与回滚

- 验收：本 ADR 落盘 + `scripts/compute_offload_reachability_freeze.txt` **逐字未变**
  （`git diff --stat` 不含该文件）；G-87/G-88 维持原判。
- 回滚：ADR-only 单 commit revert（零代码，无行为回滚面）。

## 7. 证据边界

- **无生产 N 遥测**：本 ADR 的决定性证据是「G-87 无调用方」+「阈值比可观测 N 量级高
  1~2 个数量级」，不是线上计数。若未来出现调用方，必须用真实 N 的 A/B 重开，不能引用本文。
- 不声称「CPU 计算面已极致利用」：只声称「当前 9 个已登记点位中没有可接线的高收益点」。
  新增卸载点（新算法/新批量路径）仍应走三前置 + 三要件。
