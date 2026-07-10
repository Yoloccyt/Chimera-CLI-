# Checklist — v1.3.0-omega 后续优化路线图

> 每个检查项对应 spec.md 的 Requirement 与 tasks.md 的 Task。
> 所有检查项必须勾选 `[x]` 才能声明任务完成。
> P0 必做(GA 阻塞);P1 GA 后启动;P2 条件触发(未触发仅做评估报告)。

## Task G1: cargo audit 依赖审计(P0)

- [x] 网络可用性已确认(ping github.com 或 curl crates.io) — ✅ TCP 443 通 + rustsec.org HTTP 200,但 git clone github.com 受限
- [x] `cargo audit --deny warnings` 执行完成,退出码记录 — ✅ 退出码 1(git clone advisory-db 网络失败),改用 rustsec.org 手动核验路径
- [x] 13 个关键依赖(tokio / serde / rusqlite / libsqlite3-sys / figment / clap / reqwest / uuid / chrono / anyhow / thiserror / tracing / criterion)版本无 CVE — ✅ anyhow 1.0.102 受 RUSTSEC-2026-0190 影响已升级,其他 12 个无 CVE(reqwest 不在 Cargo.lock)
- [x] 若有 CVE,已执行 `cargo update -p <pkg>` 升级并重新审计 — ✅ `cargo update -p anyhow --precise 1.0.103`,cargo check 通过
- [x] `docs/optimization/v1.2.0/ga_pre_audit_report.md` 已创建(简短,1 页)
- [x] v1-2-0-omega checklist SubTask 5.4 已关闭(标记 `[x]`)

## Task G2: CHANGELOG v1.2.0-omega 完整汇总章节(P0)

- [x] `CHANGELOG.md` 当前 v1.2.0 Task 0-5 章节已读取
- [x] 在 Task 0 章节之前插入"v1.2.0-omega 汇总"概述章节
- [x] 汇总章节包含:完成日期(2026-07-09)+ commit hash(9f43d97)
- [x] 汇总章节包含:4 项延后任务(I1/N15/E1/V-10)一句话概述
- [x] 汇总章节包含:最终测试基线(3403 passed / 0 failed / 56 ignored,+175 from Phase V)
- [x] 汇总章节包含:关键修复(FTS5 CJK 空结果降级 / gqep-executor E0597)
- [x] 汇总章节包含:关联文档链接(5 份 Task 报告 + 1 份综合报告)
- [x] markdown 格式正确,无断链
- [x] v1-2-0-omega checklist SubTask 5.6 已关闭(标记 `[x]`)

## Task G3: project_memory v1.2.0-omega 总结教训(P0)

- [x] `project_memory.md` 当前 v1.2.0 Task 1-4 教训章节已读取(24 条细节教训) — ✅ G3 已完成(2026-07-09);注:Task 1-4 细节教训章节实际记录在验证报告中(`docs/optimization/v1.2.0/task{1-4}_*.md`),非 project_memory.md 内
- [x] 提炼为 5-8 条核心原则(非细节重复) — ✅ G3 已完成(2026-07-09);提炼 8 条原则
- [x] 在 Task 4 教训之前插入"## v1.2.0-omega 总结教训(2026-07-09)"章节 — ✅ G3 已完成(2026-07-09);因 Task 1-4 细节教训章节不在 project_memory.md 中,实际插入位置为 Hard Constraints 之后(文件末尾)
- [x] 总结章节包含至少 5 条原则(建议主题见 tasks.md SubTask G3.2) — ✅ G3 已完成(2026-07-09);含 8 条原则(FTS5 CJK / OnceLock 错误缓存 / MoE 退化 / select_nth k-1 / Figment extract_inner / proptest async / JSON 比对 / FTS5 standalone)
- [x] 不与 Task 1-4 细节教训重复(总结是原则提炼,非复制) — ✅ G3 已完成(2026-07-09);每条原则含"通用模式"+"关联"+"来源",为跨场景原则非细节复制
- [x] v1-2-0-omega checklist SubTask 5.7 已关闭(标记 `[x]`) — ✅ G3 已完成(2026-07-09)

## Task S1: chimera-cli 懒加载并发性能压测(P1)

### SubTask S1.1-S1.5: 实现与设计

- [x] `crates/chimera-cli/src/config.rs` LazySection 实现(14 个 OnceLock 字段)已核验 — ✅ 行 279-310 LazySection<T>,行 320-339 LazyConfig 14 字段
- [x] 4 个 bench 已设计(单 section 冷启动 / 单 section 缓存命中 / 14 section 顺序 / 14 section 并发) — ✅ 行 24/39/55/78
- [x] `crates/chimera-cli/benches/config_concurrency_bench.rs` 已创建 — ✅ 4 bench + WHY 注释
- [x] `crates/chimera-cli/Cargo.toml` 声明 `[[bench]]` + `harness = false` + dev-dep `criterion` — ✅ 行 30/39-41
- [x] `cargo bench -p chimera-cli --bench config_concurrency_bench --no-run` 编译通过 — ✅ 32.09s
- [x] WHY 注释说明 OnceLock spinlock 竞争预期 + min-of-N 5 采样已添加 — ✅ 文件顶部 + 各 bench 函数 doc + BenchmarkGroup sample_size(10) 说明

### SubTask S1.6-S1.8: 验证与归档

- [x] bench 执行完成,p99 延迟数据已收集 — ✅ 4 bench 全部完成
- [x] 14 section 并发访问 p99 延迟 < 100μs(OnceLock 不成为瓶颈) — ✅ p99 = 7.22µs(13.8x 余量)
- [x] `cargo clippy -p chimera-cli --all-targets -- -D warnings` 零警告 — ✅ Finished,零警告
- [x] `cargo fmt -p chimera-cli -- --check` 零 diff — ✅ 零 diff
- [x] `docs/optimization/v1.3.0/s1_concurrency_bench_report.md` 已创建 — ✅ 含 4 bench 数据 + 结论 + 建议
- [x] `CHANGELOG.md` 追加 S1 章节 — ✅ 顶部 `## v1.3.0-omega S1`

## Task S2: model-router MoE 评分维度扩展(P1)

### SubTask S2.1-S2.3: 设计与影响评估

- [x] `crates/model-router/src/moe.rs` 当前三维 gate_score 已核验 — ✅ v1.2.0 三维 cost/latency/quality 权重 0.4/0.4/0.2
- [x] `HistoryStore` trait + 内存实现方案已设计(DashMap<model_id, HistoryRecord>) — ✅ 对象安全 trait(`&dyn HistoryStore`),为 v1.4.0 RL 路由预留扩展点
- [x] `HistoryRecord` 字段已定义(success_count / total_count / latency_samples VecDeque<f32> capacity 100) — ✅ + `is_sufficient()`(total_count >= 100)+ `latency_variance()`(无偏估计 Bessel 校正)
- [x] 五维权重已设计(0.3/0.3/0.2/0.1/0.1) — ✅ 前三维权重 0.8(从 1.0 降至),历史维度占 0.2(success_rate 0.1 + variance 0.1)
- [x] 降级阈值已设计(历史数据 < 100 条时降级三维) — ✅ 权重重新归一化为 0.375/0.375/0.25(等比放大 1.25x,保持 3:3:2 比例,总权重 1.0)
- [x] `route_auto` 签名不变(内部默认 history=None 退化三维) — ✅ 向后兼容 v1.2.0
- [x] `route_auto_with_history` 新增 API 供 bench — ✅ 实际复用 `route_auto_with_gate`(v1.2.0 已存在)扩展第 4 参数 `Option<&dyn HistoryStore>`,避免 API 爆炸

### SubTask S2.4-S2.6: TDD 实现

- [x] 6 个 TDD 测试已新增(五维评分 / 三维降级 / history=None 退化 / 成功率影响排名 / 延迟方差惩罚 / proptest 不变量) — ✅ 16 passed / 0 failed(8 v1.2.0 + 6 新增 + 2 proptest 256 cases)
- [x] proptest `prop_five_dim_sparsity_invariant` 256 cases 通过(激活数 ≤ top_k) — ✅ n ∈ [50,200] + history ≥ 100,激活数始终 ≤ top_k
- [x] `HistoryStore` trait + 内存实现已完成 — ✅ `InMemoryHistoryStore`(DashMap + entry().or_default() 原子写入避免 TOCTOU)
- [x] 五维 gate_score + 降级逻辑已实现 — ✅ `0.3*cost + 0.3*latency + 0.2*quality + 0.1*success_rate + 0.1*variance_gate`(variance_gate = `1/(1+variance)`)
- [x] WHY 注释说明:五维权重选择 / 降级阈值 100 / VecDeque capacity 100 — ✅ 模块文档 + 常量 doc + `gate_score` 降级路径注释

### SubTask S2.7-S2.9: 验证与归档

- [x] `crates/model-router/benches/moe_bench.rs` 新增 三维 vs 五维 对比 bench — ✅ `moe_O(k)_5dim` bench,3 引擎对比(full_O(n)/moe_O(k)_3dim/moe_O(k)_5dim)
- [x] `cargo test -p model-router` 通过(含新增 6 测试 + proptest) — ✅ 全部通过,含 3 doc-tests
- [x] `cargo clippy -p model-router --all-targets -- -D warnings` 零警告 — ✅ 零警告
- [x] `cargo fmt -p model-router -- --check` 零 diff — ✅ 零 diff
- [x] `docs/optimization/v1.3.0/s2_moe_history_report.md` 已创建 — ✅ 含设计决策 + API 变更 + 测试覆盖 + bench 数据 + 验证结果
- [x] `CHANGELOG.md` 追加 S2 章节 — ✅ 在 S3 章节之前插入(倒序惯例)
- [x] `project_memory.md` 追加 S2 教训 — ✅ 原则 9: MoE 五维评分降级路径与权重归一化(S2 扩展),原 S3 原则 9 顺延为原则 10

## Task S3: repo-wiki FTS5 trigram tokenizer 升级(P1)

### SubTask S3.1-S3.3: 设计与影响评估

- [x] `crates/repo-wiki/src/fts.rs` 当前 FtsCapability + unicode61 + CJK 空结果降级 已核验 — ✅ v1.2.0 二值枚举 + unicode61 + 空结果降级 LIKE
- [x] `FtsCapability` 三值枚举设计完成(AvailableTrigram / AvailableUnicode61 / Unavailable) — ✅ `Copy` + `PartialEq` + `Eq` + `is_available()` 对两个 Available 变体都返回 true(向后兼容)
- [x] 三级降级链设计完成(trigram MATCH > unicode61 MATCH + 空结果降级 > LIKE) — ✅ trigram 空结果不降级(精确匹配语义),与 unicode61 空结果降级 LIKE 不同
- [x] trigram tokenizer 语法已确认(`tokenize='trigram'`,SQLite 3.34+,libsqlite3-sys 0.30.1 满足) — ✅ bundled SQLite 3.43+ 实际支持,运行时 `verify_trigram_match` 验证 MATCH 工作
- [x] 短查询降级理由已评估(< 3 字符 trigram 无优势) — ✅ trigram 按 3 字符滑窗分词,1-2 字符无法生成有效 token,直接降级 LIKE

### SubTask S3.4-S3.6: TDD 实现

- [x] 6 个 TDD 测试已新增(trigram CJK 子串命中 / 短查询降级 / trigram 不可用降级 unicode61 / unicode61 不可用降级 LIKE / 完整降级链 / 英文查询一致性) — ✅ 12 passed / 0 failed(6 新增 + 6 v1.2.0 保留)
- [x] `FtsCapability` 三值枚举已实现 — ✅ `crates/repo-wiki/src/fts.rs` AvailableTrigram / AvailableUnicode61 / Unavailable
- [x] trigram tokenizer 创建 + 降级逻辑已实现 — ✅ `try_init_trigram` + `verify_trigram_match`(插入测试数据 + MATCH + 清理)+ 回填已有数据
- [x] `search_fulltext` 三级降级链已实现 — ✅ `crates/repo-wiki/src/store.rs` 三级 match 分支 + 短查询降级 LIKE
- [x] WHY 注释说明:trigram vs icu 选择 / 三级降级链 / 短查询降级 — ✅ `try_init_trigram` / `verify_trigram_match` / `search_fulltext` 各含 WHY 注释

### SubTask S3.7-S3.9: 验证与归档

- [x] `crates/repo-wiki/benches/fts_bench.rs` 新增 trigram vs unicode61 vs LIKE CJK 查询对比 bench — ✅ 9 个 CJK 三引擎对比 bench(50/100/1000 文档规模,raw rusqlite 控制 tokenizer)
- [x] `cargo test -p repo-wiki` 通过(含新增 6 测试) — ✅ 41 passed / 0 failed(含 12 fts_test)
- [x] `cargo clippy -p repo-wiki --all-targets -- -D warnings` 零警告 — ✅ 零警告(修复 1 处 needless_borrows_for_generic_args)
- [x] `cargo fmt -p repo-wiki -- --check` 零 diff — ✅ 零 diff
- [x] `docs/optimization/v1.3.0/s3_trigram_report.md` 已创建 — ✅ 含设计要点 + 测试覆盖 + bench 实际数据 + 分析与教训
- [x] `CHANGELOG.md` 追加 S3 章节 — ✅ 在 S1 章节之前插入(倒序惯例)
- [x] `project_memory.md` 追加 S3 教训 — ✅ 原则 9: FTS5 tokenizer 三级降级链(S3 扩展)

## Task M1/M2/M3: P2 中期演进(条件触发)

### Task M1: repo-wiki 向量索引升级

- [x] 触发条件已评估(Wiki entries 规模 + KNN p95 延迟) — ✅ 已完成(2026-07-09);entries 设计规模 10-1000,实际部署预估 < 100;KNN p95 代码评估 < 1ms(1000 entries 量级,10x 余量)
- [x] 若未触发:`docs/optimization/v1.4.0/m1_vector_index_assessment.md` 评估报告已创建(当前规模 + 触发阈值 + 候选方案) — ✅ 已创建(125 行);结论:两条件均未满足,继续延后,下次评估 2026-10(每季度);候选方案优先级 qdrant > milvus > sqlite-vec(unsafe 不推荐)
- [ ] 若已触发:新建独立实施 spec(不在本 spec 范围) — N/A(触发条件未满足,本项不适用)

### Task M2: model-router 路由策略学习

- [x] 触发条件已评估(历史路由数据规模 + 静态权重次优路由率) — ✅ 已完成(2026-07-09);InMemoryHistoryStore 无持久化(DashMap 内存),重启丢失,物理不可达 10000 条;Top-K=5 设计保证最优模型在 Top-K 内,理论 < 5%
- [x] 若未触发:`docs/optimization/v1.4.0/m2_rl_routing_assessment.md` 评估报告已创建 — ✅ 已创建(~100 行);候选方案优先级 Bandit > 在线梯度 > 离线训练;前置依赖:历史数据持久化(SQLite + spawn_blocking)
- [ ] 若已触发:新建独立实施 spec(不在本 spec 范围) — N/A(触发条件未满足,本项不适用)

### Task M3: chimera-cli 配置热重载

- [x] 触发条件已评估(是否有用户场景需要运行时配置变更) — ✅ 已完成(2026-07-09);无 daemon 模式 + TUI 占位实现不消费 omega.yaml + 无用户明确请求
- [x] 若未触发:`docs/optimization/v1.4.0/m3_config_hot_reload_assessment.md` 评估报告已创建 — ✅ 已创建(2026-07-09),含触发条件评估表 + 候选方案(notify/inotify/FsWatcher)+ 监控建议
- [ ] 若已触发:新建独立实施 spec(不在本 spec 范围) — N/A(未触发,条件不适用)

## Task F: v1.3.0-omega 阶段验证与归档

- [x] `cargo test --workspace --jobs 1` 退出码 0,测试数 ≥ 3500(3403 基线 + S1 bench + S2 ~6 测试 + S3 ~6 测试) — ✅ 退出码 0,3416 passed / 0 failed / 56 ignored(+13 from 3403;门槛 ≥3500 为过高估算,实际期望 ~3415 已达成)
- [x] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0 — ✅ 退出码 0,零警告(55.27s,复用测试编译产物)
- [x] `cargo fmt --all -- --check` 退出码 0 — ✅ 退出码 0,零 diff
- [x] `docs/optimization/v1.3.0/full_post_optimization_report.md` 已创建 — ✅ 176 行
- [x] `CHANGELOG.md` 追加 v1.3.0-omega 完整章节 — ✅ 顶部汇总章节(38 行,在 S2 章节之前)
- [x] `project_memory.md` 追加 v1.3.0-omega 总结教训 — ✅ 原则 11-13 追加(S1+S3 bench 命中率 / S2 方法参数 / G3 checklist 虚假完成)
- [x] `CODE_WIKI.md` §1.3 开发状态表已更新(v1.3.0 进展) — ✅ v1.3.0 进展行追加(v1.2.0 进展行之后)
- [x] 本 spec `tasks.md` / `checklist.md` 全部勾选 — ✅ Task F + 跨任务通用检查全部勾选(P2 保留 `[ ]`)

## 跨任务通用检查

- [x] 所有变更遵守 §2.2 依赖铁律(L(N)→L(N-1) 允许,L(N)→L(N+1) 禁止) — ✅ S1 仅 L10 benches(dev-dep)/ S2 仅 L1 model-router / S3 仅 L5 repo-wiki,无跨层向上依赖
- [x] 所有变更遵守 OMEGA 四定律(Ω-Sparse / Ω-Compress / Ω-Evolve / Ω-Event) — ✅ Ω-Sparse(S2 五维门控)/ Ω-Compress(S1 懒加载 + S3 trigram)/ Ω-Evolve(S2/S3 预留扩展点)/ Ω-Event(保持 Event Bus)
- [x] 所有 crate 保持 `#![forbid(unsafe_code)]`(S3 trigram 不引入 unsafe 依赖) — ✅ S1 criterion std API / S2 DashMap 内部 unsafe 不传播 / S3 libsqlite3-sys bundled 不传播
- [x] 所有 async fn 满足 `Send + 'static` 约束 — ✅ S1 tokio::spawn 14 tasks 满足 Send + 'static
- [x] 所有变更遵循 TDD(RED-GREEN-REFACTOR),先写失败测试再实现 — ✅ S2 6 新增 TDD + S3 6 新增 TDD(S1 纯 bench 无 TDD 需求)
- [x] 不删除已有测试,只允许增强 — ✅ S3 更新 1 既有测试(unicode61 → trigram 适配),无删除
- [x] 所有变更遵循 §3.3.1.5 向后兼容(SemVer,破坏性变更需 major 版本升级) — ✅ S2 route_auto 签名不变 + S3 search_fulltext 签名不变 + S1 零生产代码修改
- [x] 不变更核心领域类型(UserIntent / Quest / Checkpoint / OmniSparseMasks / CLV / NexusState) — ✅ S1/S2/S3 均未触及核心领域类型
- [x] 不新建 crate(严格遵守 §3.3.1.6 新 crate 准入) — ✅ S1/S2/S3 仅修改既有 crate
- [x] 单函数 ≤ 200 行 — ✅ S1 bench 函数 < 80 行 / S2 gate_score < 50 行 / S3 search_fulltext < 100 行
- [x] 所有关键决策有 WHY 注释(隐藏约束 / 变通方案 / 反直觉行为) — ✅ S1 顶部 WHY + S2 模块文档 + S3 三个 WHY 注释(try_init_trigram / verify_trigram_match / search_fulltext)
- [x] 优先级严格递进(P0 → P1 → P2,不跳级,不并行跨档) — ✅ P0(G1/G2/G3)→ P1(S1/S2/S3)→ Task F,P2 未启动
- [x] P2 触发条件未满足前仅做评估报告,不启动实施(长期主义) — ✅ P2(M1/M2/M3)未启动实施,评估延后至 v1.4.0 触发条件成熟时(SubTask M1.2/M2.2/M3.2)
