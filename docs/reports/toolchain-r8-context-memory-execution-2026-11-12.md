# R8 批次执行报告：上下文/记忆层 + 调度面性能优化（2026-11-12）

> 分支 `feat/toolchain-parallel-optimization`，承接 R7（cadfc312..0b168149，ADR-284/286/287）。
> 本报告为 R8 批次单一事实源：方案、证据、验收、回滚。证据行号以深审时点工作树（HEAD=8d86b5c3）为准。

## 0. 关键假设（信息不足处的显式声明）

1. 「对上下文、记忆层进行优化」按项目分层语义解释为 **L2 Memory（nmc-encoder / hcw-window / mlc-engine）+ L3 Storage（scc-cache / cmt-tiering / session-store）+ L5 repo-wiki 检索面**；「工具链调度」指 mas-sched / quest-engine / nexus-subagent 调度面（R7 已覆盖大半，本轮补残查结论）。
2. 「不削减」= 全部现有测试（含 doctest 口径按 W6 教训单列）与行为语义保持；性能改动不得改变对外 API 契约。
3. scc-cache + repo-wiki 的 26 项 doctest windows-gnu 工件态失败为既有独立工单，不计入本批回归口径（非 doc 全绿为基线）。

## 1. 深审事实底座（4 路并行子代理 + 主会话抽查复核）

### 1.1 mlc-engine（L2 记忆引擎）
| # | 发现 | 证据 |
|---|------|------|
| M-a | `engine.rs:627-629/729-731/977-980` 三处 `l2.get(id)` 整条 MemoryEntry 深拷贝（CLV 2KB+content）仅为读元数据/算 token_estimate | engine.rs 对应行 |
| M-b | `recall_approx` 墓碑补口对全部 n 条 `(id.clone(), sim)` + O(n×k) `any` 过滤 | l2_semantic.rs:666-679 |
| M-c | `SemanticMemory::insert` 每次深拷贝 `entry.clv`（2KB）后才 intern | l2_semantic.rs:421 |
| M-d | `ExperienceCardSystem::add_card` success_rate 全表扫描 O(n)/卡（锁内），全程 O(n²) | experience_card_system.rs:170-183 |
| M-e | `memory_graph.rs:175` 精确建边 n² String clone（1K 节点≈100 万次分配，bench 464ms） | memory_graph.rs:168-177 |
| 锁面 | 全 crate 无持锁跨 await（G-27 合规有锚）；loom 锁协议不可动，仅可优化锁内算法 | tests/loom_card_system.rs |
| 不可破坏 | 分数逐位一致契约（l2_semantic.rs:1294/1389、tests/hnsw_tiered.rs:324）、4096 精确层 <200ms | 同左 |

### 1.2 hcw-window（上下文窗口）
| # | 发现 | 证据 |
|---|------|------|
| H-a | **降级/L3 溢出压缩全程持写锁**，锁内做全量评分+Top-K+rayon 批调度，阻塞所有读者 | window.rs:329-431、:657-704 |
| H-b | 历史警示：SubTask 12.7 曾将「锁外压缩」判为 P0 竞态（覆盖并发 insert 丢数据）回退为持锁 | window.rs:315-320 注释 |
| H-c | rerank `density_order` 全排序 O(N log N) 但只消费头部；两处二次排序可复用 | rerank.rs:794-802、:868-873 |
| H-d | 粗召回三信号（dep BFS/sem cosine/co 查询）串行，代码自注认可并行 | coarse.rs:128-131 |
| 红线合规 | 1M 等效走 128K 实际+8× 稀疏双路径强制；Top-K 全部 select_nth_unstable | window.rs:471-477、compressor.rs:238-242 |

### 1.3 L3 存储层（scc-cache / cmt-tiering / session-store）
| # | 发现 | 证据 |
|---|------|------|
| S-a | cmt `run_decay_cycle` Hot 层全量 `list_all()` 克隆含 content；Warm/Cold 批内逐 id 串行 await peek（每条一次 SQLite 往返） | coordinator.rs:474、:594-599、:643-648 |
| S-b | cmt `report_tier_stats` 在每次 insert/delete/decay/LSCT 迁移后调用，每次 Ice 全目录遍历 + 2 次 SQLite COUNT | coordinator.rs:248/419/523/799 → :697、ice.rs:145-169 |
| S-c | session-store 每 flush：N 段各自 autocommit 事务（N 次 fsync）后才单事务写事件；TreeIndex 未应用 `apply_performance_pragmas` | writer.rs:379-382、tree.rs:247-270、:206-211 |
| S-d | scc-cache SqliteWal 每条 entry 一次 spawn_blocking + autocommit INSERT + commit 再一次 UPDATE | wal.rs:444-463、:487-495 |
| S-e | scc-cache LRU `select_victim` O(n) 全表扫描×每候选 2 次分片锁；命中路径每必 publish_blocking+clone+to_string | lru.rs:39-75、cache.rs:118-125 |
| 红线合规 | 三 crate rusqlite spawn_blocking 0 违规 | wal.rs/tree.rs 全量 grep |

### 1.4 调度面（mas-sched / quest-engine / repo-wiki hnsw / nmc-encoder）
| # | 发现 | 证据 |
|---|------|------|
| Q-a | quest-engine 恢复路径 Quest msgpack 解码在 async 线程同步跑 + `quest.clone()` 整份拷贝；`CheckpointManager::save` 编码同理（checkpoint_async 后已离开关键路径但仍在 async worker） | engine.rs:301、:305、checkpoint.rs:85-86 |
| Q-b | `update_task_status` 持 DashMap 写锁做 3 次 O(n) 线性扫描；任务上限 16 影响有限 | engine.rs:491-517 |
| Q-c | mas-sched 为同步租约表 claim 模型，无 tick、无持锁跨 await；`ShadowLog` 无界 Vec + snapshot 全量 clone（长跑内存风险，登记不实施） | scheduler.rs:44-46、shadow.rs:72-111 |
| R-a | **hnsw flaky 根因判定：随机种子（Hnsw::new 每次从 OS 熵播种，无注入口）× 退化等距数据集（5 个共线向量含零向量）× 严格断言 len==1**；并行度只是暴露频率放大器。修法=测试数据去退化/种子可控，禁放宽断言 | hnsw_store.rs:752-754、:855-856、:1186-1200 |
| R-b | hnsw `upsert` check-then-act TOCTOU（并发同 id 可产生两个活 DataId）——登记为正确性观察项，本轮不动（改动涉及锁协议，需独立裁决） | hnsw_store.rs:421-441 |
| N-a | **红线违规：chimera-mas `wiki_retrieval.rs:112` async fn 内裸调同步 `encoder.perceive`**（TextPerceptor 挂 ONNX 即同步推理占死 worker）；nmc-encoder `perceive_batch` G-87 DEAD（零生产调用点） | wiki_retrieval.rs:107-121、parallel.rs:7-10 |
| N-b | nmc-encoder 模型加载无进程级缓存：每次 `NmcEncoder::new` 重复 4 次 ONNX 解析编译；`is_ffmpeg_available` 每次视频预处理 spawn 子进程 | fusion.rs:177-207、onnx_backend.rs:305/:669-675 |

## 2. R8 工作包（可落地：模块/文件/接口/步骤）

| WP | crate（文件所有权） | 内容 | 接口变化 | 风险 | 执行 |
|----|--------------------|------|---------|------|------|
| WP-01 | mlc-engine | M1 新增 `get_arc`/元数据轻读替换 3 处深拷贝；M2 补口 usize 索引化；M3 insert CLV `take` 零拷贝 | L2 新增只读方法（纯增量） | 低 | 子代理 |
| WP-02 | mlc-engine | M4 card success_rate 增量计数（锁内算法，锁协议不动）；M5 建边 usize 索引 | 无 | 低 | 同代理串行 |
| WP-03 | cmt-tiering | S1 decay Hot 就地过滤免 content 克隆 + 批内 peek 按 id 集合 SQL 化；S2 report_tier_stats 增量计数 + 短 TTL 缓存 | 内部实现，事件载荷不变 | 中 | 子代理 |
| WP-04 | session-store | S3 段元数据并入单事务 + `apply_performance_pragmas`（WAL 下 synchronous=NORMAL 语义） | TreeIndex 内部 | 中 | 子代理 |
| WP-05 | quest-engine | Q-a 恢复/保存解码编收入既有 spawn_blocking + 消 `quest.clone()` | 无 | 低 | 子代理 |
| WP-06 | hcw-window | H-a **版本化锁外压缩**：读锁快照(Arc refcount)+mutations 计数 → 锁外 compress → 写锁重校验 + retained∩current ∪ 新增 差集合并 → 超容拒绝降级（语义同失败分支）。保 SubTask 12.7 不丢数据 | HcwState 增 `mutations: u64`（serde skip） | 高 | 主会话 |
| WP-07 | repo-wiki | R-a flaky 测试数据去退化（seed 固定生成非共线向量），断言不变；200 次循环验证 | 仅测试 | 低 | 主会话 |
| WP-08 | nmc-encoder + chimera-mas | N-a 补齐 async 调用面：nmc-encoder 提供 `perceive_async`（spawn_blocking facade），wiki_retrieval 换用 | 新增 async 方法（纯增量） | 低 | 子代理 |

**登记不实施（证据留存，后续批次裁决）**：H-c/H-d（rerank partial select、三信号并行——WP-06 落地后按余量决定）、S-d/S-e（scc Wal 批量化与 LRU 结构升级）、Q-b（增量计数，任务上限 16 收益窄）、Q-c ShadowLog 界、R-b upsert TOCTOU、N-b 模型加载缓存（涉及生命周期归属，需组合根裁决）。

## 3. 验收标准（可验证）

1. **逐 crate 定向回归**：`cargo test -p <crate> --jobs 2` 全绿（mlc-engine 约 176 测试含 loom cfg 门控、hcw-window、cmt-tiering、session-store、quest-engine、repo-wiki、chimera-mas 触及面）。
2. **行为锁**：mlc 分数逐位一致契约、hcw 并发 insert 不丢失（12.7 既有测试 + 新增版本化合并测试）、cmt 事件载荷不变（CapabilityTiered 逐条发布）、session-store flush 语义（wal_replay 100% 一致性门）。
3. **bench 对照**：memory_graph_edges（M5）、l2_recall/mlc_l2_knn（M1-M3）、lsct_tiering_benchmark（S2）、session_store_bench（S3）、compress/parallel_compress + window_select（WP-06）。目标：热路径组改善或持平，无 >5% 回归。
4. **全量口径**：非 doc 全量回归（--jobs 2）≥ R7 基线（1249 定向 / 全量 12138+ 新增），26 项 doctest 工件态单列。

## 4. 回滚方式（可回溯）

- 每 WP 独立 commit（Conventional Commits），回滚 = git revert 单提交。
- WP-06 涉及 HcwState 序列化字段新增（serde skip+default）：旧 checkpoint 数据反序列化兼容（mutations 从 0 起算，单调性语义不受起点影响），无数据迁移。
- ADR：本批决策落 ADR-288（hcw 版本化锁外压缩——含 SubTask 12.7 竞态的正式解法）与 ADR-289（记忆层热路径削减汇总）；flaky 修复与调度面残查并入收官报告。

## 5. 执行记录

### 5.1 执行拓扑

主会话（WP-06 hcw / WP-07 hnsw / WP-04 session-store）+ 并行子代理（WP-01/02 mlc、WP-03 cmt、WP-05 quest、WP-08 nmc+mas）。过程事件：首批 5 代理中 3 个被会话并发限制击落（mlc/session-store/nmc），工作树零残留（尚在读码阶段），mlc 重派成功、其余由主会话串行接管。

### 5.2 逐 WP 结果

| WP | 改动 | 测试 | clippy | bench/门禁 |
|----|------|------|--------|-----------|
| WP-01/02 mlc-engine | l2_semantic.rs(+20)/engine.rs(+33/-13)/experience_card_system.rs/memory_graph.rs | **461/0**（分数逐位契约+loom 绿） | 0 警告 | memory_graph_edges/1000: 471→431ms（-10%，噪声底 ±4~7%）；l2_knn 全档 ≤400µs（200ms 红线余量 >10⁴） |
| WP-03 cmt-tiering | hot.rs(+15 list_matching)/coordinator.rs(+64/-14 decay 翻转+TTL) | **380/0**（proptest 绿） | 0 警告 | lsct bench 不经被改路径（诚实声明）；收益=候选外零拷贝+重算 ≤0.5 次/s |
| WP-04 session-store | tree.rs（PRAGMA+insert_flush_batch+私有共用函数）/writer.rs（切换调用） | **67/0** | 0 警告 | **syscall_reduction_pct=98.44%**（门禁 ≥80%）；wal_replay_cycle 19.9ms |
| WP-05 quest-engine | checkpoint.rs（save→Arc<Quest>+load_latest_quest）/engine.rs（恢复收编） | **414/0** | 0 警告 + fmt 净 | checkpoint_save 四组 criterion 判定均 No change（噪声地板 ±30% 由零改动对照组 checkpoint_list 证实） |
| WP-06 hcw-window | types.rs（mutations+replace_entries）/window.rs（SelectPlan 三阶段+merge_compress_with_snapshot+compress_to_capacity 同构） | 全 target **0 failed**（含 12.7 并发安全网）+ 5 新单测 | 0 警告 | compress_100k_to_32k 8.26µs 无回归；window_select ns 级 |
| WP-07 repo-wiki | hnsw_store.rs 测试数据去退化（断言不变） | 修复测试 200/200 stress 全绿 | — | 根因=随机种子×退化等距数据集（判词见 ADR-289 D7） |
| WP-08 nmc+mas | text.rs(+40 perceive_async)/wiki_retrieval.rs(调用切换)/Cargo.toml(tokio)/parallel.rs(状态注记) | nmc **157/0**（+3）+ mas lib **317/0** + knowledge_test **25/0** | 双侧 0 警告 | 现场修正：违规点宿主=TextPerceptor（深审记 NmcEncoder 有偏差）；crate 间边零变化 |

### 5.3 回滚锚点

每 WP 独立 commit；WP-06 的 `mutations` 字段 serde skip 不阻碍回滚；详细决策与不采纳清单见 ADR-288/ADR-289。
