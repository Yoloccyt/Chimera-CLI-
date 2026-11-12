# ADR-289: R8 记忆/存储层热路径削减汇总决策

- 状态: Accepted
- 日期: 2026-11-12
- 批次: R8 工具链优化（上下文/记忆层 + 调度面）
- 关联: ADR-288（hcw 版本化锁外压缩，独立成档）、R8 执行报告、ADR-284/286/287（R7 前序批次）

## 范围与证据

四路并行深审（mlc-engine / hcw-window+nmc-encoder / L3 存储三 crate / 调度面+hnsw）产出证据级发现清单（行号锚定 HEAD=8d86b5c3），本 ADR 记录采纳的实施决策与**显式不采纳/登记**项。全部改动以「不削减功能、不改对外契约、测试全绿」为准入。

## 采纳决策（7 项）

### D1 — mlc-engine L2 深拷贝消除（WP-01）

- `SemanticMemory` 新增只读 `get_arc`（仿 `list_all_arc` 先例 idiom），替换 `engine.rs` 三处「整条 MemoryEntry 深拷贝（CLV 2KB+content）仅为读元数据/算 token_estimate」。
- `insert` 的 `entry.clv.clone()` 改 `as_ref()` 借用 intern。**关键修正**：深审原建议 `take()` 会掏空存入条目的 CLV，破坏 `test_insert_and_get` 的 `clv.is_some()` 取回契约——借用方案同样消除拷贝且语义真零变化。
- 验收：461 passed / 0 failed，分数逐位一致契约（to_bits）与 loom 锁协议全绿。

### D2 — mlc-engine 锁内算法与分配热点（WP-02）

- `ExperienceCardSystem::add_card` success_rate 从 O(n)/卡全表扫描（锁内）改为 `MethodStatistics.success_count` 增量计数（同式除法，f32 位模式一致；锁协议不动——loom 建模的短临界区得以保持）。
- `memory_graph::build_semantic_edges_exact` 打分阶段 `Vec<(String, f32)>` → `Vec<(usize, f32)>`（1K 节点 ≈100 万次 String 分配消除），Top-K 选定后才物化 id；`select_nth_unstable_by` 比较器不变 → 选序逐位等价。bench 目标档 471ms→431ms（-10%，与消除项的 Amdahl 估算 7-8% 吻合；本机噪声底 ±4~7%）。

### D3 — cmt-tiering decay 快照翻转 + 统计风暴节流（WP-03）

- `run_decay_cycle` 的 Hot 层从「`list_all()` 全量克隆含 content → 事后筛选」翻转为 `list_matching(predicate)` 就地筛选（谓词在 DashMap 分片读锁内对借用值判定），仅候选条目物化；判定等价（同一 now / 同一 should_demote），`CapabilityTiered` 逐条发布语义逐位不变。
- `report_tier_stats` 四调用点（insert/delete/decay/LSCT 迁移后）的风暴治理：`tier_stats` 重算加 2s TTL 缓存（`STATS_RECOMPUTE_TTL`），事件仍每次发布；锁纪律三段式（锁内查缓存→放锁重算→锁内回写），不持锁跨 await。
- 显式不采纳：Ice 层增量计数（需处理外部变更漂移，复杂度/收益比不干净）——TTL 节流已达成目标。

### D4 — session-store flush 单事务合并 + 性能 pragma（WP-04）

- 新增 `TreeIndex::insert_flush_batch(segments, rows)`：单 `BEGIN IMMEDIATE` 完成一次 flush 的全部段 upsert + 事件插入（原 N+1 次写事务/flush → 1 次）；既有 `insert_segment`/`insert_events` 保留（fork/rebuild 兼容），私有 `upsert_segment_row`/`insert_events_rows` 消除 SQL 重复。
- `TreeIndex::init` 追加 `synchronous=NORMAL` + `busy_timeout=5000`。**依据**：本 crate 崩溃恢复语义以段文件为权威源（lib.rs），SQLite 仅索引面且可 rebuild；WAL 模式 NORMAL 下断电不损坏库文件、最多丢最后未 checkpoint 事务（可接受），而 FULL 档每次 COMMIT 的 fsync 在 Windows FlushFileBuffers 上有 R7 实测 ~2.9ms 固有成本。
- 验收：67 passed / 0 failed；`syscall_reduction_pct = 98.44%`（门禁 ≥80%）。

### D5 — quest-engine 检查点序列化收编（WP-05）

- `CheckpointManager::save(&Quest)` → `save(Arc<Quest>)`：Quest 编码挪入既有 spawn_blocking（ADR-286 checkpoint_async 化后仅存的 async worker 序列化点消除）；恢复路径解码 + 双副本克隆合入单阻塞闭包，async 线程 payload CPU 降为 0。
- **数学边界声明**：恢复路径双所有权消费方（DashMap 值语义注册表 + 公共 API 返回 `Quest`）决定克隆只能重排不能消除；彻底消除需注册表改存 `Arc<Quest>`（核心领域类型变更，超出本 WP，登记为后续候选）。
- 验收：414 passed / 0 failed，clippy/fmt 干净；终态恒同步 durability 决策（engine.rs 注释锁）未动。

### D6 — nmc-encoder G-88 红线闭环（WP-08，异步 facade）

- **现场修正（深审偏差订正）**：wiki_retrieval 的 encoder 实为 `Option<Arc<TextPerceptor>>`（经 `Perceptor` trait 分派），async facade 挂 `TextPerceptor` 固有 impl——挂 `NmcEncoder` 无法修复该违规点且会制造新 DEAD API（G-87 负收益红线）。
- 新增 `TextPerceptor::perceive_async(self: Arc<Self>, input: PerceptionInput)`：有 runtime 走 `spawn_blocking` 卸载，无 runtime 经 `Handle::try_current()` 降级同步（session-store `run_blocking` 先例形态）；Arc receiver 满足 'static 且与调用方既有所有权形态零成本匹配。
- chimera-mas `wiki_retrieval::search` 裸调切换至该 facade；`perceive_batch` 维持 G-87 DEAD 登记状态（头注更新）。
- 验收：nmc-encoder 157/0（+3 新测试，含手写最小 executor 测降级分支）、chimera-mas lib 317/0 + knowledge_test 25/0；双侧 clippy 干净；nmc-encoder 新增 tokio 为外部 workspace 依赖，crate 间边零变化。

### D7 — repo-wiki hnsw flaky 根因修复（WP-07）

- 深审判词：flaky = **OS 熵随机种子（Hnsw::new 无 seed 注入口）× 退化等距数据集（5 个共线向量含零向量）× 严格断言**；并行度只是暴露频率放大器（旧注释「tie-break 受并行负载影响」系误读——单独 3 连跑均绿正是低抽样次数下低概率事件不显现）。
- 修复=测试数据去退化（两两非共线确定性向量 + 查询精确等于末次 upsert 向量 → 最近邻唯一有梯度），**断言不变**；200 次循环 stress 200/200 全绿。生产 `upsert` 的 check-then-act TOCTOU 登记为正确性观察项（锁协议变更需独立裁决，本批不动）。

## 登记不实施（证据留存，后续批次裁决）

| 项 | 证据 | 不实施理由 |
|---|------|-----------|
| rerank `density_order` 全排序 → partial select | rerank.rs:794-802 | 贪心填充消费头部，收益中等；WP-06 落地后按余量评估 |
| 粗召回三信号 data-parallel | coarse.rs:128-131 | 代码自认可并行；需 ComputeBridge 桥接纪律评估，独立工作包 |
| scc-cache LRU O(n) 扫描 / 命中路径 publish_blocking | lru.rs:39-75、cache.rs:118-125 | 256 条目自辩可接受；事件采样化改变消费方可观测性，需裁决 |
| scc-cache SqliteWal 批量化 | wal.rs:444-495 | WAL 面每条 2 次阻塞池跳转；收益明确但属接口新增，排下一批 |
| quest-engine `update_task_status` 增量计数 | engine.rs:491-517 | 任务上限 16，O(3n) 收益窄 |
| mas-sched ShadowLog 无界 Vec | shadow.rs:72-111 | 长跑内存风险；需容量策略裁决（界/环形/分段），非纯性能 |
| nmc 模型加载进程级缓存 + ffmpeg 探测缓存 | fusion.rs:177-207 | 涉及 encoder 生命周期归属（组合根裁决），非 crate 内可定 |
| hnsw `insert_batch` parallel_insert 接线 | hnsw_store.rs:609-640 | 刻意串行有注释自辩；确定性纪律需先建基准 |

## 效果汇总

| 路径 | 改动前 | 改动后 | 证据 |
|------|--------|--------|------|
| mlc recall 结果打包（每结果） | 整条 MemoryEntry 深拷贝 ~2KB+ | Arc 引用计数 | 461 测试位模式不变 |
| mlc 建边 1K 节点 | 471ms（n² String 分配） | 431ms（-10%） | criterion quick |
| cmt decay Hot 快照 | 全量克隆含 content | 仅候选物化 | 380 测试等价验证 |
| cmt tier_stats 重算 | 每次变更全扫 | ≤0.5 次/秒 | TTL 节流 |
| session-store flush SQLite | N+1 写事务 | 1 写事务 | syscall_reduction 98.44% |
| quest 恢复 async 线程 | 同步 msgpack 解码+整份 clone | 阻塞池内完成 | 414 测试不变 |
| hcw 压缩期间读者 | 全程阻塞 | 不阻塞 | 并发安全网全绿 |
| wiki_retrieval 推理调用 | async 裸调（G-88 违规） | spawn_blocking facade | 红线闭环 |
