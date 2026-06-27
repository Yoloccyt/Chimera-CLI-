# Tasks — Week 3 第三轮深度复审(架构完整性与长期演进)

> 本任务列表基于 5 位资深专家(架构/并发/性能/测试/代码规范)的分布式深度分析结果制定。
> 按 P0→P1→P2→P3 优先级排序,共 6 个 Task(Task 17-22),29 个 SubTask。
> 每个 SubTask 完成后立即勾选对应 checklist 项。

---

## Task 17:P0 架构完整性修复(事件驱动链路闭环)

闭合 OSA→HCW 稀疏化链路,补全事件 payload,实现关键事件无订阅者告警。

- [x] SubTask 17.1:闭合 OSA→HCW 事件驱动稀疏化链路
  - 在 `HcwState` 新增 `pending_context_mask: Option<Vec<String>>` 字段
  - 修改 `spawn_mask_listener`:收到 `OmniSparseMasksComputed` 事件后,将 `context_mask` 存入 `pending_context_mask`(而非忽略)
  - 修改 `insert` 与 `select_window`:调用前检查 `pending_context_mask`,若存在则自动调用 `apply_sparse_mask` 并清除标记
  - 注意锁重入:listener 内先释放写锁,再调用 apply_sparse_mask 获取写锁
  - 文件:`crates/hcw-window/src/window.rs`、`crates/hcw-window/src/types.rs`
  - 新增集成测试:OSA 发布事件 → HCW listener 自动应用稀疏化 → 验证条目数减少

- [x] SubTask 17.2:关键事件无订阅者告警
  - 修改 `EventBus::publish`:当 `subscriber_count() == 0` 且事件为 Critical 级时,记录 `warn` 日志
  - Normal 级事件保持静默丢弃(避免日志噪声)
  - 文件:`crates/event-bus/src/bus.rs`
  - 新增单元测试:发布 Critical 事件无订阅者时,验证 warn 日志(用 `tracing_test` 或手动捕获)

- [x] SubTask 17.3:补全 ToolsRouted 事件 payload
  - 在 `NexusEvent::ToolsRouted` 新增 `routed_tools: Vec<String>` 字段(默认 Top-8 工具 ID)
  - 更新 KVBSR 发布逻辑:填充完整工具列表
  - 更新 `metadata()`/`severity()`/`type_name()` match 分支
  - 向后兼容:旧消费者忽略新字段
  - 文件:`crates/event-bus/src/types.rs`、`crates/kvbsr-router/src/router.rs`
  - 更新现有事件测试:验证 `routed_tools` 字段正确

- [x] SubTask 17.4:补全 MemoryTiered 事件 payload
  - 在 `NexusEvent::MemoryTiered` 新增 `memory_id: Option<String>` 字段(Option 兼容批量迁移)
  - 更新 MLC 发布逻辑:单条迁移时填充 memory_id,批量迁移时为 None
  - 更新 `metadata()`/`severity()`/`type_name()` match 分支
  - 文件:`crates/event-bus/src/types.rs`、`crates/mlc-engine/src/engine.rs`
  - 更新现有事件测试:验证 `memory_id` 字段正确

---

## Task 18:P1 并发安全加固(跨层操作原子性)

消除 MLC migrate 与 CMT get 的 TOCTOU 窗口,保障 L0 insert 原子性。

- [x] SubTask 18.1:MLC migrate 引入条目级迁移锁
  - 在 `MlcEngine` 新增 `migration_locks: DashMap<MemoryId, ()>` 字段
  - `migrate` 方法开始时获取条目级锁(`entry().or_insert(())`),离开作用域自动释放
  - 确保 fetch_from_tier → insert → remove_from_tier 在锁保护下原子执行
  - 文件:`crates/mlc-engine/src/engine.rs`
  - 新增并发测试:10 线程同时 migrate 同一 MemoryId,断言目标层条目数 ≤ 1

- [x] SubTask 18.2:CMT promote_to_hot_internal 幂等化
  - 修改 `promote_to_hot_internal`:delete 失败时,若错误为 `EntryNotFound`,视为已被其他线程删除,继续完成提升
  - 记录 `debug` 日志:"Cold 层条目已被其他线程删除,继续提升"
  - 文件:`crates/cmt-tiering/src/coordinator.rs`
  - 新增并发测试:1 线程 get + 1 线程 delete 同一 CapabilityId,断言 get 不返回 MigrationFailed

- [x] SubTask 18.3:CMT run_decay_cycle 迁移前双重检查
  - 在迁移执行前,再次 `peek` 确认条目仍在源层
  - 若 `peek` 返回 None(已被提升/删除),跳过该条目迁移
  - 文件:`crates/cmt-tiering/src/coordinator.rs`
  - 新增并发测试:1 线程 run_decay_cycle + 10 线程 get,断言无 MigrationFailed 错误

- [x] SubTask 18.4:L0 WorkingMemory insert 使用 DashMap::entry() 原子操作
  - 修改 `insert`:使用 `DashMap::entry()` API 原子性检查并插入
  - `Entry::Occupied`:更新条目,无需 LRU 驱逐
  - `Entry::Vacant`:检查容量并可能驱逐,再插入
  - 消除 `contains_key` 与 `insert` 之间的 TOCTOU 窗口
  - 文件:`crates/mlc-engine/src/l0_working.rs`
  - 新增并发测试:10 线程各插入 10 条目(含相同 id),断言 L0 容量 ≤ 64 且无 panic

---

## Task 19:P1 性能优化(热点路径零冗余分配)

优化 Cold get 查询、衰减周期内存、L2 召回分配、HCW 锁内 clone、HCW get 索引、KVBSR blocks clone。

- [x] SubTask 19.1:Cold 层 get 改为单 SELECT + 内存构造 + 单 UPDATE
  - 参照 Warm 层优化模式:单次 SELECT 读取所有字段 → 内存构造返回条目(access_count+1, last_accessed_at=now)→ 单次 UPDATE
  - 消除原 SELECT → UPDATE → SELECT 三次查询模式
  - 文件:`crates/cmt-tiering/src/cold.rs`
  - 验证:Cold get 延迟降低约 33%

- [x] SubTask 19.2:run_decay_cycle 流式处理 + 仅查 metadata
  - 新增 `list_idle_metadata()` 方法:只返回 ID + 时间戳 + 计数(不含 content)
  - 衰减判断仅需 access_count + last_accessed_at,降级时再按需读取 content
  - 分批处理:65536 条目分批(每批 1024),批间释放内存
  - 文件:`crates/cmt-tiering/src/coordinator.rs`、`crates/cmt-tiering/src/warm.rs`、`crates/cmt-tiering/src/cold.rs`
  - 验证:衰减周期内存峰值降低 80%+

- [x] SubTask 19.3:L2 recall_by_clv 用索引替代 MemoryId clone
  - 修改 `scored: Vec<(MemoryId, f32)>` 为 `Vec<(usize, f32)>`(存储 vectors 索引)
  - select_nth_unstable_by 后,从 `inner.vectors[idx].1` 取 MemoryId
  - 消除 4096 次 String 堆分配/释放
  - 文件:`crates/mlc-engine/src/l2_semantic.rs`
  - 验证:4096 条目召回延迟降低 10-20%

- [x] SubTask 19.4:HCW compress 接受 &[ContextEntry] 避免 clone
  - 修改 `ContextCompressor::compress` 签名:接受 `&[ContextEntry]` 而非 `Vec<ContextEntry>`
  - 修改 `select_window` 与 `compress_to_capacity`:直接传 `&state.entries`,消除 `state.entries.clone()`
  - 文件:`crates/hcw-window/src/compressor.rs`、`crates/hcw-window/src/window.rs`
  - 验证:select_window 写锁持有时间减少 50-100μs(1000 条目规模)

- [x] SubTask 19.5:HCW get 用 HashMap 索引替代 O(n) 扫描
  - 在 `HcwState` 新增 `entries_index: HashMap<String, usize>` 字段(id → entries 索引)
  - `get`/`remove` 用 HashMap O(1) 查找,`insert` 同步更新索引
  - 文件:`crates/hcw-window/src/types.rs`、`crates/hcw-window/src/window.rs`
  - 验证:1000 条目 get 从 ~15μs 降到 ~0.1μs

- [x] SubTask 19.6:KVBSR route_impl 避免全量 blocks clone + 去重候选收集
  - 锁内只需 clone top-3 块的 tools 列表(而非全部 50 块)
  - 将 route_impl 已收集的 candidate_tool_ids 直接传给 select_top_tools,避免重复收集
  - 文件:`crates/kvbsr-router/src/router.rs`
  - 验证:路由延迟减少 5-10μs

---

## Task 20:P1 测试稳定性修复(CI 友好)

消除 flaky 测试,分离性能断言,补充 proptest 与错误路径测试。

- [x] SubTask 20.1:性能断言测试迁移至 benches/ 或标记 #[ignore]
  - 识别 30+ 性能断言测试(P50/P99 阈值、加速比、延迟断言)
  - 标记 `#[ignore = "perf: run with --ignored"]`(18 处,覆盖 12 个测试文件)
  - 保留 benches/ 中的 criterion 基准测试(不迁移,保持独立)
  - 文件:5 个 crate 的 tests/ 目录
  - 验证:`cargo test --workspace` 反馈循环 < 60s ✓

- [x] SubTask 20.2:替换 thread::sleep 为逻辑时钟
  - 识别 16 处 `thread::sleep(Duration::from_millis(2))`(已全部替换为 0 处)
  - 引入 `AtomicU64` 逻辑计数器作为 `last_accessed_at` 的替代(测试模式)
  - 文件:`crates/mlc-engine/src/l0_working.rs`、`crates/mlc-engine/tests/working_memory.rs`、`crates/cmt-tiering/src/hot.rs`、`crates/cmt-tiering/tests/hot.rs`
  - 验证:LRU 顺序测试在 Windows(15ms 定时器精度)下稳定通过 ✓

- [x] SubTask 20.3:补充 hcw-window proptest
  - 新增 5 个属性测试:
    - 压缩率不变量(compression_ratio ≥ 1.0)
    - 窗口选择单调性(complexity ↑ → tier ↑)
    - 压缩后条目数 ≤ target_size
    - 窗口容量边界不变量
    - 条目插入幂等性
  - 文件:`crates/hcw-window/tests/proptest.rs`(新增)
  - 验证:64 cases 全部通过 ✓

- [x] SubTask 20.4:补充 kvbsr-router proptest
  - 新增 5 个属性测试:
    - 路由结果数 ≤ top_k(test_route_result_le_top_k)
    - 路由结果数 ≤ 总工具数(test_route_result_le_total_tools)
    - 重平衡后块数量 ≤ 工具数(test_rebalance_block_count_le_tools)
    - 分数范围 ∈ [-1.0, 1.0](test_route_scores_in_unit_range,余弦相似度范围)
    - 工具存在时路由结果非空(test_route_non_empty_when_tools_exist)
  - 文件:`crates/kvbsr-router/tests/proptest.rs`(新增)
  - 验证:64 cases 全部通过 ✓

- [x] SubTask 20.5:补充错误路径测试
  - 每个 crate 补充 5 个错误路径测试(共 25 个):
    - mlc-engine:父目录不存在、CLV 缺失插入、空 db_path、L0 容量超限、serde_json 错误
    - cmt-tiering:Cold 父目录不存在、配置校验空路径、IO 错误转换、serde_json 错误转换
    - hcw-window:l0_capacity=0、非单调容量、零压缩阈值、过高压缩阈值、EventBus 错误转换
    - osa-coordinator:complexity>1.0、complexity<0、配置边界反转、预算阈值无效、serde_json 错误
    - kvbsr-router:未 build_blocks 路由、空工具 build_blocks、维度不匹配、零 block_vector_dim、零 top_tools
  - 文件:5 个 crate 的 tests/error_paths.rs(新增)
  - 验证:错误路径测试覆盖关键故障场景 ✓

---

## Task 21:P2 代码重复治理(提取共享工具到 nexus-core)

将 5 个重复工具函数提取到 nexus-core(L1),消除约 275 行重复代码。

- [x] SubTask 21.1:提取 id_newtype! 宏到 nexus-core
  - 在 `nexus-core` 新增 `pub mod newtype` 模块,导出 `id_newtype!` 宏
  - mlc-engine/types.rs 和 osa-coordinator/types.rs 改为 `use nexus_core::id_newtype`
  - kvbsr-router/types.rs 的 ToolId 改用宏(消除约 50 行手动实现)
  - 文件:`crates/nexus-core/src/newtype.rs`(新增)、`crates/nexus-core/src/lib.rs`、3 个 crate 的 types.rs
  - 验证:`cargo check --workspace` 通过,消除约 110 行重复

- [x] SubTask 21.2:提取 apply_performance_pragmas 到 nexus-core
  - 在 `nexus-core` 新增 `pub mod sqlite_pragma` 模块,导出 `apply_performance_pragmas(conn: &Connection) -> Result<(), NexusError>`
  - 3 处调用改为 `nexus_core::sqlite_pragma::apply_performance_pragmas(&conn).map_err(...)?`
  - 文件:`crates/nexus-core/src/sqlite_pragma.rs`(新增)、`crates/nexus-core/src/lib.rs`、3 个 crate 的源文件
  - 验证:消除约 60 行重复

- [x] SubTask 21.3:提取 expand_tilde 到 nexus-core
  - 在 `nexus-core` 新增 `pub mod path_util` 模块,导出 `expand_tilde(path: &Path) -> PathBuf`
  - mlc-engine/config.rs 和 cmt-tiering/config.rs 改为 `use nexus_core::path_util::expand_tilde`
  - 文件:`crates/nexus-core/src/path_util.rs`(新增)、`crates/nexus-core/src/lib.rs`、2 个 crate 的 config.rs
  - 验证:消除约 25 行重复

- [x] SubTask 21.4:统一 cosine_similarity 到 nexus-core
  - 在 `nexus-core` 新增 `pub fn cosine_similarity_slices(a: &[f32], b: &[f32]) -> f32` 自由函数
  - 统一零向量处理策略:返回 0.0(非 NaN)
  - mlc-engine/types.rs、kvbsr-router/blocks.rs、repo-wiki/vector.rs 改为 `use nexus_core::cosine_similarity_slices`
  - 文件:`crates/nexus-core/src/clv.rs`、3 个 crate 的源文件
  - 验证:消除约 80 行重复

---

## Task 22:P3 文档与清理

清理冗余声明,更新文档,全量验证。

- [x] SubTask 22.1:清理 OSA Cargo.toml 冗余声明
  - 移除 `crates/osa-coordinator/Cargo.toml` 中 `[dev-dependencies]` 的 `nexus-core` 行(保留 `[dependencies]` 中的)
  - 文件:`crates/osa-coordinator/Cargo.toml`

- [x] SubTask 22.2:删除 test_write.txt 残留文件
  - 删除 `crates/cmt-tiering/tests/test_write.txt`(调试残留)
  - 文件:`crates/cmt-tiering/tests/test_write.txt`

- [x] SubTask 22.3:更新 CHANGELOG.md 第三轮复审记录
  - 新增 "## Week 3 第三轮深度复审(2026-06-24)" 章节
  - 列出 Task 17-22 的修复内容与影响范围
  - 文件:`CHANGELOG.md`

- [x] SubTask 22.4:更新 project_memory.md 经验教训
  - 记录事件驱动链路闭环模式(生产者发布 + 消费者自动应用)
  - 记录条目级迁移锁模式(DashMap entry 锁)
  - 记录索引化召回模式(usize 索引替代 String clone)
  - 记录逻辑时钟替代墙钟时间(测试稳定性)
  - 文件:`c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md`

- [x] SubTask 22.5:更新 CODE_WIKI.md
  - 更新 HCW 事件订阅说明(自动应用 context_mask)
  - 更新事件 payload 说明(ToolsRouted.routed_tools、MemoryTiered.memory_id)
  - 更新 nexus-core 共享模块说明(newtype/pragma/path/cosine)
  - 文件:`CODE_WIKI.md`

- [x] SubTask 22.6:运行 cargo check/clippy/test/build --workspace --jobs 1 全部通过
  - 验证第三轮复审后全量构建无回归
  - 验证 `cargo clippy --workspace -- -D warnings` 零警告
  - 验证 `cargo test --workspace` 全通过(含新增测试)
  - 验证 `cargo build --workspace --release` 通过

---

## Task Dependencies

- Task 17(P0 架构)→ 无依赖,优先执行
- Task 18(P1 并发)→ 无依赖,可与 Task 17 并行
- Task 19(P1 性能)→ 无依赖,可与 Task 17/18 并行
- Task 20(P1 测试)→ 依赖 Task 17/18/19(测试新行为)
- Task 21(P2 重复治理)→ 无依赖,可与 Task 17-20 并行
- Task 22(P3 文档)→ 依赖 Task 17-21(文档记录已完成的工作)

## 优先级执行顺序

1. **第一批(并行)**:Task 17(P0 架构)+ Task 18(P1 并发)+ Task 19(P1 性能)+ Task 21(P2 重复治理)
2. **第二批**:Task 20(P1 测试,验证第一批的修改)
3. **第三批**:Task 22(P3 文档,记录全部完成的工作)
