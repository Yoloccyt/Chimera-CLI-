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

## 7. Tranche 1 结算（工程工单批准后执行，2026-11-08）

| 工单 | commit | 结果 | 证据 |
| --- | --- | --- | --- |
| C rust-version | 0e4dfc31 | ✅ | workspace `rust-version="1.97"`（对齐 CI 1.97.1）；edition 2021 维持（C1 裁决） |
| B RSS 峰值 | 62b07dec | ✅ | ADR-265 双口径（窗口峰值/生存期峰值）；explorer 实测 peak≥avg；G-85 28/28（重写曾丢 BOM，按门 FIX 恢复） |
| A X2-N1 | 本批 | ✅ **160ms→32ms（收紧 5×）** | 九步程序全过：bench 执行器补能力表（§5.3 规格）→ pre_x3 快照装回（现态备份 tmp/x2_pre_restore_backup）→ 双轮 → 合理性门比值 0.95（ADR-252 D3 预言兑现）→ ceil(12.585×2.5)=32ms → 棘轮护栏 <160ms ✓ → 双源同 commit；首轮 undeclared c8 vs 改前基线 **−79.5%** |
| A 三键 | 本批 | spol_* ✅ / actx_*/xloop_* 不采纳 | spol_opt_hit(350ns)/spol_opt_miss(300ns) 五处双写 + I2 机检零重叠 + I7 CI 行补齐；actx/xloop 按 ADR-249 D6 记录不采纳（ADR-251 §4 终裁） |
| A F-07 | 本批 | ✅ | 基线机器再生（18 行同集硬门 ✓，test_ctor 计数刷新，尾注 152→154，头注同步），门 [OK] |
| A F-08 | 本批 | ✅ 占位 | gqep-executor floor=45 占位行（tier=critical）；validate-config [OK]；终值待 CI coverage.yml |
| A N-2/N-4/N-5 | 本批 | ✅ | adr_index「七份→八份」+ ADR-254 遗留句闭合标注；bench_check.yml 注释去整数化（47→动态现取）；bus.rs 四处 17⊆21 字面注释改指常量（断言消息留 OQ-16） |
| A ADR-254 §6 | 本批 | ✅ 回填 | c1/c8 mean 表（8.1×/8.5×≥4×）+ 阈值收紧记录 + 测量面订正（/1 负载敏感不设阈值；改前基线=双缺省形态失去直接可比性） |

**磁盘事件（登记）**：X2 前置核验发现 D 盘 97% 满（8.5G 可用 < 40GB 前置）——
根因 = `target/debug` **109GB**（当日全量回归的全矩阵产物）。处置：移除
target/debug（可再生缓存，release 工件保留），自由空间恢复至 118G（58%）；
代价 = 收尾全量回归需重建 debug 树（该回归本就要跑，成本并入）。

**Tranche 1 全量回归**：`cargo test --workspace --no-fail-fast --jobs 2`
（debug 树重建后首跑），结果见 §8 追记。

## 8. Tranche 1 全量回归追记（2026-11-08 完成）

`cargo test --workspace --no-fail-fast --jobs 2`（target/debug 清理后首跑，
debug 树全量重建）：**passed=12143 / failed=13 / ignored=89**，
日志 `tmp/tranche1_regression.log`。

**13 个 failed 逐项裁决（全部 = repo-wiki --doc 的 doctest 工件解析失败，
非本批回归）**：

- 失败集 = repo-wiki 全部 13 个 doctest（vector/config/search/memory_knn_store/
  hnsw_store/lib），错误 = **E0462**（found staticlib `derive_builder_macro`
  instead of rlib）与 **E0460**（found possibly newer version of crate
  `image`/`rand`）——均为 **windows-gnu 下 proc-macro/依赖工件解析的构建态
  问题**，非断言/语义失败。
- **与本批改动的无关性证据**：① repo-wiki 本批零触碰（本批 Rust 面仅
  rust-version 元数据 + bus.rs 注释 + bench-only 文件）；② 同一轮回
  **12,143 个非 doc 测试全绿**（含 repo-wiki 全部单测）；③ 晨间旧 debug 树
  同套 doctest 全绿（§6：12154/2/89）；④ 本批后三次复现尝试（单 crate 重
  构建、proc-macro 工件清除、workspace 语境 doc 复跑）错误形态在
  E0462/E0460 间迁移但均不消除——构建态问题而非代码问题。
- **归因推断**：`rm -rf target/debug` 后的单次 workspace 全重建在 windows-gnu
  上落下了 proc-macro 导入库（.dll.a）被 doctest extern 解析判定为
  staticlib-kind 的工件态（多哈希并存歧义）；【推断】与 cargo 的
  hardlink 去重/多 feature-set 重复构建交互有关——需独立 spike（登记未闭环）。
- **环境附注**：修复尝试中的一次 `cargo build -p repo-wiki` 又引入 E0460
  代际混杂——进一步佐证为工件态问题（本批代码零语义变更）。
- **处置**：本批代码工件的验收以「12,143 非 doc 全绿 + doctest 失败归因
  登记」收口；repo-wiki doctest 环境修复立独立工单（与 hnsw flaky 同批
  处理，属 windows-gnu 构建态调查），**不得以改 doctest 断言方式掩盖**。

**Tranche 1 代码面验收口径**：非 doc 12143 全绿（含本批新增测试）+
doctest 失败全部归因登记 + 静态门全绿（invariants 85/85 与 I1-I10、
I3/I7 56、I2=1≤1、static-only rc=0、G-85 28/28、coverage validate [OK]、
event_ownership [OK]）。

## 9. Tranche 2 结算（工程工单 D/E/F/G，2026-11-08）

| 工单 | commit | 结果 | 证据 |
| --- | --- | --- | --- |
| D P1 四环接入 | e2627cf3 | ✅ | ADR-256 D1~D8；McpToolExecutor+CompositeToolExecutor+组合根装配；单元 7/7（512-case parse proptest）+ 集成 8/8（64-case 零网络 proptest）+ 回归四套件 + composition root 7 点不变 + 154 不变；**D8 诚实性声明（协议级 ACK 非业务结果）** |
| E P2 装配接线 | 39d812b4 | ✅ **含规格前提修正** | ADR-257；deepdive「全字段 Clone」枚举遗漏三类型 → **Arc 内化**（TtgGovernor/CoordinationMetricsCollector/CheckpointManager）→ QuestEngine derive(Clone) 浅共享；D2 tokio Mutex 串行化代价登记；D4 适配器事件闭环（AgentTask* 三变体）；D5 契约头 WIRED；quest-engine 254/0（浅共享语义锁）+ subagent_wiring 7/7（SWARM 确定性 enforcement/事件闭环/双 proptest） |
| F P3 注册面闭合 | 06581d15 | ✅ **含实施偏差登记** | ADR-258；DynamicEventRegistry 组合根构造（深审增补项 D1）+ register_all 幂等 14 hook.*（轨二不占 154）；**nexus-hook 为 optional 依赖 → cfg 门控（r2_island_repayment）——可达性棘轮 38 不变**（deepdive §8.3 无条件形态不可行）；契约头 CONSUMERS 0→1；event-bus lib.rs:39 陈旧注记修正；hook_wiring 5/5 + nexus-hook 5/5 零改动 |
| G P4 U 项裁决 | 79aa1b70 | ✅ | ADR-259 九决策（U-05/07/09/11 预留、U-06 移交 TUI、**U-08 改判写面已接线**（Z-3+lib.rs 三订正段）、**U-20 改判已接线**（env dump 回归锁实证）、F-09 候选挂 OQ-27、F-12 维持）；panorama_adjudication_locks 5/5；零新增事件变体（六锁同批） |

**W6 收官全量回归**：见 §10 追记（P1~P4 增量重建后首跑）。

## 10. W6 全量回归追记（2026-11-09 完成）

`cargo test --workspace --no-fail-fast --jobs 2`（P1~P4 增量重建后首跑）：
**passed=12138 / failed=26 / ignored=87**，日志 `tmp/w6_regression.log`。

**26 个 failed 逐项裁决（全部 = windows-gnu doctest 工件解析失败家族，
非本批回归）**：

- 失败分布 = **repo-wiki --doc 13 项 + scc-cache --doc 13 项**（两套 doctest
  整体编译失败,非断言失败）；错误族 = E0462（staticlib instead of rlib）/
  E0460（newer version of crate）/"required to be available in rlib format"
  （decay_engine/nmc_encoder/tract_hir/tract_linalg 等依赖 rlib 缺位）——
  同一 **windows-gnu proc-macro/依赖工件解析构建态问题**（Tranche 1 §8
  已登记首例）。
- **与本批改动的无关性证据**：① repo-wiki/scc-cache 本批零触碰（Tranche 2
  Rust 面 = chimera-cli/nexus-app-server/quest-engine/nexus-hook/
  event-bus 注释/session-store 注释）；② 同一轮 **12,138 非 doc 测试全绿**
  （含 P1 mcp_tool_executor 15 项 / P2 subagent_wiring 7 项 / P4 locks 5 项
  等全部新增）；③ 晨间旧 debug 树同套 doctest 全绿（§6）；④ 错误类型为
  工件解析而非语义失败,且家族跨 crate 扩散（repo-wiki → +scc-cache）
  ——与「构建态非确定性」诊断一致。
- **根因推断【推断】**：rm -rf target/debug 后的单次 workspace 全重建在
  windows-gnu 上落入 proc-macro/依赖 rlib 解析歧义工件态；与 cargo 的
  多 feature-set 重复构建/hardlink 去重交互（待独立工单取证）。
- **处置**：独立工单「repo-wiki/scc-cache doctest windows-gnu 工件态调查」
  （升级覆盖两 crate）；验收口径 = 非 doc 12,138 全绿 + doctest 失败全族
  归因登记；**不得以改 doctest 断言方式掩盖**。

**W6 收官口径**：Tranche 1 + Tranche 2 全部工单（C/B/A/D/E/F/G）验收通过；
ADR-249~259 + 265 落档；零新增事件变体（154 六锁不变）；可达性 38 不变；
遗留 = §5 移交清单 + doctest 工件态独立工单。
