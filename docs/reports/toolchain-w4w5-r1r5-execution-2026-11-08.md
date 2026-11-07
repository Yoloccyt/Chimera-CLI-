# 工具链深度优化执行报告 — W5.0/W4 收口/W5/R1~R5（2026-11-08）

- **分支**：`feat/toolchain-parallel-optimization`（基线 33b20d9 → 本报告 4c616484，7 commits）
- **方案源**：批准方案「工具链系统性深度优化重构方案（承接 W7-R2 终稿 + 阶段四 R1~R5）」+ 终稿 `toolchain-w7r2-panorama-deepdive-2026-11-07.md`
- **用户四裁决**（2026-11-07）：GPU 维持封印·CPU 计算面极致优化 / 能耗=代理指标体系 / mimalloc 评测式引入 / 承接终稿+新增四维波次

## 1. 波次结算总表

| 波次 | commit | 裁决 | 核心证据 |
| --- | --- | --- | --- |
| W5.0 I10 双源对拍 | f15ef9a | ✅ 收口 | I10 83/83 现形；selftest-22a~f 有牙；hnsw_tiered 幽灵门变真门 |
| W4 收口 | f1c56cd | ❌ **借用脊柱否决** | 硬门三对全不满足（0.971/0.998/0.998）；ADR-249；脊柱 8 文件回退；门独立收益保留 |
| W5 | 6468f94 | ✅ 5.1/5.2 采纳 | ADR-251 六点定界；有界并发（MAX_PERMITS panic 修复）+ 装配分区 join! |
| R1 seccore | 3dbcf608 | ✅ **采纳** | **硬门六对全过且 CI 数量级分离：miss 82~99.7% / hit 93.5%**（miss_R256 26ns vs 8.66µs） |
| R2 批量 | 52f5c89 | ✅ R2-1 采纳 | 在飞装箱峰值 N→bound；tbatch 四键阈值内；R2-2/3/4 定界暂缓 |
| R3 计算面 | 8275a0de | ✅ 双旋钮+零盲改 | CHIMERA_RT_WORKERS / CHIMERA_COMPUTE_THREADS；双池选项②维持；轮询取证证伪（top 候选全为测试代码）；GPU 三门报告（维持封印） |
| R4 内存 | 6b30b9c8 | ✅ 棘轮+审计；mimalloc 未过门 | 分配预算 7/68 只降不升；零无界增长点审计；A/B 仅 1/6 过 5% → 保持非默认 |
| R5 能效 | 4c616484 | ✅ 工具面采纳 | 生产轮询基线 32（测试污染 99 剥离）；energy_profile.ps1；R5-3 设计冻结待真机验证 |

## 2. 量化收益汇总（四维对照）

| 维度 | 改造前 | 改造后 | 证据 |
| --- | --- | --- | --- |
| 判决热路径（策略评估） | ~516ns/次 × 2 次/派发（2k 分配） | 33ns hit / 26ns miss，全零分配（规范输入） | ADR-260 §4 |
| 派发链端到端 | ≈1-3µs（策略评估占 30-60%） | 预期降 ~30-50%（X2 期现键实测收敛） | ADR-260 §4 推断 |
| 批量在飞装箱峰值 | N（64 请求=64 箱） | bound（8 箱，8×↓）；deadline 放弃零作废 | ADR-261 |
| 分配预算 | 无守卫 | 单发 7/批量 68 棘轮（只降不升）+ 判决零分配锁 | ADR-263 |
| CPU 调度面 | 无旋钮 | RT_WORKERS + COMPUTE_THREADS 双旋钮（默认零变更） | ADR-262 |
| 空闲唤醒面 | 无测量 | 生产 32 点基线 + 审计工具；TUI Eco 设计冻结 | ADR-264 |
| GPU | 封印无评估 | 三重关闭复核 + 三门对照 + 监测仪表 | gpu-unseal-readiness-2026-11.md |

## 3. 裁决波次的否决记录（同等重要的负结论）

1. **ADR-249 借用脊柱否决**：静态 10→7 分配下降实测仅 0.2~2.9% 墙钟差——
   「分配数下降 ≠ 墙钟收益」教训入档；bench 首版 drive_n 死锁
   （FuturesUnordered::push 不 poll）修复并沉淀为模式。
2. **ADR-261 R2-2/R2-3/R2-4 暂缓**：借用载体前置失效 / owned 脊柱下净差 0 /
   L0 契约类型变更收益不足。
3. **ADR-262 R3-3 零改造**：轮询治理前提被取证证伪（top 候选全为测试代码）；
   双池 spike 三选一维持选项②。
4. **ADR-263 mimalloc 未过门**：六点全显著但仅 1/6 过 5%——feature 留作
   评测基建不设默认。
5. **ADR-264 R5-3 暂缓**：TUI 空闲 Eco 设计冻结，真机验证前置（用户红线）。

## 4. 全量回归（本报告验收门）

`cargo test --workspace --no-fail-fast --jobs 2`，基线 12136/0/89 只增不减；
结果见本报告文末追记（回归完成后回填）。

## 5. 移交下一批次（未闭合项）

| 项 | 状态 | 触发/前置 |
| --- | --- | --- |
| X2 治理债 + N-1 收紧 | 未开工 | 唯一 bench 期 = 全部改码后（actx_*/xloop_*/spol_* 键实测登记一并做） |
| P1~P4（终稿阶段三） | 未开工 | 按 deepdive §6~§9 原样执行（ADR-256~259） |
| W6+ 扩展收官 | 部分素材就绪 | 收官报告 + 回滚演练（本报告为输入） |
| R5-3 TUI 空闲 Eco | 设计冻结 | 真机验证（三场景）后实施并默认 0→5 |
| mimalloc 重评估 | 证据留档 | 分配占比重新主导或大分配载荷上线 |
| ADR-249 借用脊柱重评估 | 否决留档 | 高频借用调用方出现或分配占比上移跨过 5% 门 |

## 6. 全量回归追记（2026-11-08 完成）

`cargo test --workspace --no-fail-fast --jobs 2`：**passed=12154 / failed=2 / ignored=89**（550 test target，日志 `tmp/full_regression_r1r5.log`；对照基线 12136/0/89 → **passed +18** = 本批新增测试）。

2 个 failed 的逐项裁决：

1. `repo-wiki hnsw_store::test_repeated_upsert_no_duplicate_in_search` —
   **既有 flaky**（测试文件 :1180-1186 自带历史：2026-08-03 已判定，根因 =
   hnsw_rs 等距向量 tie-break 受并行负载影响；本轮 5 连跑 4 绿 1 红，
   失败后单独复跑即绿）。与本批改动无关（repo-wiki/hnsw_rs/依赖版本零触碰）。
   **处置：不改断言**（代码注释已给出修复方向：检查 upsert 后 HNSW 图内
   残留旧链接的去重路径）——移交下一批次作为独立修复项。
2. `seccore execpolicy::with_default_action` doctest — **本批引入、已被本门
   抓获并修复**（commit 见 git log）：W4 泛型化 `evaluate<S: AsRef<str>>`
   后空切片字面量推断歧义（chimera-cli 侧同款当时已修，doctest 漏网）；
   修复 = `&[] as &[String]` 消歧，复跑 seccore doctest 7/0 绿。

修后等效口径：**12155+ / 0 / 89**（doctest 修复 +1，flaky 复跑绿）。
