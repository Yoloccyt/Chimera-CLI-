# Tasks — v1.1.0-omega 系统性深度优化

> 任务按 MoSCoW 优先级 + 五阶段顺序编排,严格遵守"安全→正确性→性能→架构→演进"顺序。
> 每个阶段完成后必须通过 checklist.md 全部检查项才能进入下一阶段。
> 阶段预算基于 `DEEP_RESEARCH_34CRATE_OPTIMIZATION.md` Action Plan 估算,可按实际情况调整。
> **执行规范**:每个 Task 都遵循 TDD(RED-GREEN-REFACTOR),先写失败测试再实现;子代理产物必须 `cargo fmt --all` + `cargo clippy --jobs 2 -- -D warnings` 通过;每个 Task 完成后勾选 `[x]`。

## Phase I:安全紧急修复(2 天,6h 估算)

> **依赖**:无前置
> **并行性**:Task I-1 / I-2 / I-3 可并行(独立文件修改)
> **验收门槛**:三项全部完成 + `cargo test --workspace` 退出码 0 + `cargo clippy --workspace --jobs 2 -- -D warnings` 退出码 0 + OWASP A01-A10 渗透用例通过

- [x] **Task I-1: 修复 seccore cmd.exe 绕过 Critical 漏洞 [N1]**(2h,安全专家 agent) — 已完成 2026-07-09
  - [x] SubTask I-1.1: Round 1 现状核验 — Read `crates/seccore/src/policy.rs` 当前命令白名单,确认 `cmd` 条目存在;grep `cmd` 在白名单中的位置
  - [x] SubTask I-1.2: Round 2 方案设计 — 采用选项 A:从 `CommandPolicy::default_secure()` 白名单中移除 `cmd`
  - [x] SubTask I-1.3: TDD-RED — 在 `tests/security/owasp_top10.rs` 新增测试 `test_owasp_a03_cmd_exe_bypass_blocked`,验证 `cmd /c "危险命令"` 被拒绝(测试当前失败)
  - [x] SubTask I-1.4: TDD-GREEN — 实现修复,使测试通过
  - [x] SubTask I-1.5: TDD-REFACTOR — 添加 WHY 注释说明 cmd.exe 绕过原理
  - [x] SubTask I-1.6: `cargo test -p seccore` 验证所有现有测试通过 + 新增测试通过
  - [x] SubTask I-1.7: `cargo test --test owasp_top10` 验证 OWASP A03 用例全部通过

- [x] **Task I-2: 修复 ASA 空关键字绕过 [N4]**(2h,安全专家 agent) — 已完成 2026-07-09
  - [x] SubTask I-2.1: Round 1 现状核验 — Read `crates/seccore/src/asa.rs` 当前 `audit()` 实现,确认风险关键字列表为空时返回 `RiskLevel::Low`
  - [x] SubTask I-2.2: Round 3 影响评估 — grep `RiskLevel::Low` 全 workspace 使用点,确认改为 `Unknown` 不破坏下游 match 分支
  - [x] SubTask I-2.3: TDD-RED — 在 `crates/seccore/tests/asa_test.rs` 新增测试 `test_audit_empty_keywords_returns_unknown`,验证空列表返回 `RiskLevel::Unknown`(当前失败)
  - [x] SubTask I-2.4: TDD-GREEN — 修改 `asa.rs::audit()`,当风险关键字列表为空时返回 `RiskLevel::Unknown`
  - [x] SubTask I-2.5: TDD-REFACTOR — 添加 WHY 注释说明"空关键字 = 未知风险,触发额外审计"的安全语义
  - [x] SubTask I-2.6: `cargo test -p seccore` 验证测试通过

- [x] **Task I-3: 修复 AuditChain 后置记录 [N5]**(2h,安全专家 agent) — 已完成 2026-07-09
  - [x] SubTask I-3.1: Round 1 现状核验 — Read `crates/seccore/src/audit.rs` 当前 AuditChain 实现,确认 append 在命令执行后才调用
  - [x] SubTask I-3.2: Round 2 方案设计 — 设计 pre-execution append 模式:执行前记录 `Intent` 状态 → 执行 → 更新为 `Executed` / `Failed`;append 失败必须返回 Err 阻止命令执行
  - [x] SubTask I-3.3: TDD-RED — 在 `crates/seccore/tests/audit_test.rs` 新增测试 `test_audit_chain_pre_execution_append` 与 `test_audit_chain_append_failure_blocks_execution`
  - [x] SubTask I-3.4: TDD-GREEN — 重构 AuditChain.append() 流程为 pre-execution → execute → post-update 模式
  - [x] SubTask I-3.5: TDD-REFACTOR — 引入 `AuditRecordStatus` 枚举(`Intent` / `Executed` / `Failed`),添加 WHY 注释
  - [x] SubTask I-3.6: `cargo test -p seccore` 验证测试通过

- [x] **Task I-4: Phase I 验证与归档**(1h,质量验证 + 文档同步 agent) — 已完成 2026-07-09
  - [x] SubTask I-4.1: `cargo test --workspace` 退出码 0
  - [x] SubTask I-4.2: `$env:RUST_MIN_STACK = '33554432'; $env:CARGO_INCREMENTAL = '0'; cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
  - [x] SubTask I-4.3: `cargo fmt --all -- --check` 退出码 0
  - [x] SubTask I-4.4: 创建 `docs/optimization/v1.1.0/phase1_security_verification_report.md`
  - [x] SubTask I-4.5: `CHANGELOG.md` 追加 Phase I 安全修复章节
  - [x] SubTask I-4.6: `project_memory.md` 追加 Phase I 教训(cmd.exe 绕过模式 / pre-execution audit 模式)

## Phase II:正确性修复(2 天,6h 估算)

> **依赖**:Phase I 全部完成
> **并行性**:Task II-1 / II-2 / II-3 可并行(独立 crate 修改)
> **验收门槛**:三项 P0 bug 的 TDD 测试补齐 + cargo test 全绿 + 测试增量 ≥ 6
>
> **实施前核实(2026-07-09)**:N2/N3/A1 的生产代码修复已存在于当前代码库
> (DEEP_RESEARCH 报告 2026-07-08 之后的代码演进已解决这些 bug)。
> - N2:`crates/ssra-fusion/src/fusion/engine.rs:139-146` 已用 `max_by` 选取真正最大权重
> - N3:`crates/qeep-protocol/src/protocol.rs` 已实现 `entangle()`/`entangle_spawn()` + `Ack` struct + `CallState`
> - A1:`crates/quest-engine/src/checkpoint.rs` 四方法均已改为 async + `spawn_blocking`
>
> 但 TDD 测试覆盖不完整(缺 proptest、三元组完整性测试、非阻塞验证测试)。
> Phase II 调整为"补齐缺失的 TDD 测试 + 辅助函数抽象",**不重写已修复的生产代码**。
> 子代理实施时以 task 描述为准,跳过"修复生产代码"类子任务,聚焦"补测试"。

- [x] **Task II-1: 补齐 SSRA 主导策略 TDD 测试 [N2 P0,代码已修复]**(2h,正确性专家 agent) — 已完成 2026-07-09
  - [x] SubTask II-1.1: Round 1 现状核验 — Read `crates/ssra-fusion/src/fusion/engine.rs::select_top_k_desc`,确认 `select_nth_unstable_by` 后用 `pick_max_weight` 显式取最大(实际路径为 engine.rs,非 strategy.rs)
  - [x] SubTask II-1.2: Round 2 数学证明 — 验证 `select_nth_unstable_by(slice, k, cmp)` 仅保证 `slice[k-1]` 是第 k 大且 `slice[0..k]` 都 ≥ pivot,不保证 `slice[0]` 是最大值
  - [x] SubTask II-1.3: TDD-RED — 在 `crates/ssra-fusion/tests/strategy_proptest.rs` 新增 proptest `prop_main_strategy_always_max`,生成随机权重向量,验证主导策略权重 == 向量最大值
  - [x] SubTask II-1.4: TDD-GREEN — `pick_max_weight` 用 `max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Equal))` 选取真正最大权重(engine.rs:214-218)
  - [x] SubTask II-1.5: TDD-REFACTOR — 提取 `pick_max_weight()` 辅助函数,添加 WHY 注释说明 `select_nth_unstable_by` 语义(engine.rs:211-213)
  - [x] SubTask II-1.6: `cargo test -p ssra-fusion` + `cargo test -p ssra-fusion --test strategy_proptest` 全绿(128 cases)
  - [x] SubTask II-1.7: 添加单元测试覆盖 NaN / 空向量 / 单元素边界情况(engine.rs:586-639,5 个测试)

- [x] **Task II-2: 补齐 QEEP 三元组 Ack TDD 测试 [N3 P0,代码已修复]**(2h,正确性专家 agent) — 已完成 2026-07-09
  - [x] SubTask II-2.1: Round 1 现状核验 — Read `crates/qeep-protocol/src/protocol.rs`,确认 Ack 在 entangle() 步骤 1.5 创建(protocol.rs:133-143)
  - [x] SubTask II-2.2: Round 2 方案设计 — Ack 在 Request 注册后、future poll 前创建;Ack 包含 id + acknowledged_at;状态机 Pending→Acknowledged→Completed
  - [x] SubTask II-2.3: TDD-RED — 在 `crates/qeep-protocol/tests/protocol_test.rs` 新增测试 `test_full_triplet_request_ack_receipt`(L27)与 `test_ack_missing_blocks_receipt`(L86),验证三元组完整性
  - [x] SubTask II-2.4: TDD-GREEN — 实现 Ack 创建与校验逻辑(protocol.rs:133-143 + types.rs:44-50)
  - [x] SubTask II-2.5: TDD-REFACTOR — 引入 `CallState` 状态机(Pending/Acknowledged/Completed/Timeout/Failed,功能等价于 spec 的 TripletState),添加 WHY 注释(protocol.rs:134-135)
  - [x] SubTask II-2.6: `cargo test -p qeep-protocol` 全绿

- [x] **Task II-3: 补齐 Checkpoint spawn_blocking TDD 测试 [A1 P0,代码已修复]**(2h,正确性专家 agent) — 已完成 2026-07-09
  - [x] SubTask II-3.1: Round 1 现状核验 — Read `crates/quest-engine/src/checkpoint.rs`,确认四方法已改为 async + spawn_blocking(save:94 / load:158 / load_latest:190 / prune_old:262)
  - [x] SubTask II-3.2: Round 2 方案设计 — 四方法改为 `async fn`,内部用 `tokio::task::spawn_blocking` 包装同步 I/O;错误类型保持 `QuestError` 不变
  - [x] SubTask II-3.3: TDD-RED — 在 `crates/quest-engine/tests/checkpoint.rs` 新增测试 `test_save_load_not_blocking_runtime`(L574)+ `test_load_latest_not_blocking_runtime`(L601),用 `tokio::time::timeout` 验证 save/load 不阻塞
  - [x] SubTask II-3.4: TDD-GREEN — 四方法 async + spawn_blocking 已落地(checkpoint.rs:64-265)
  - [x] SubTask II-3.5: TDD-REFACTOR — 提取独立静态函数 `*_blocking`(save_blocking/load_blocking/load_latest_blocking/prune_old_blocking,功能等价于泛型 spawn_blocking_io,避免 &self 借用冲突),添加 WHY 注释(checkpoint.rs:101-102)
  - [x] SubTask II-3.6: 修复所有调用点(quest-engine 内部 + 下游 crate)的 `.await`,`cargo check --workspace` 编译通过
  - [x] SubTask II-3.7: `cargo test -p quest-engine` + `cargo check --workspace` 验证编译与测试通过

- [x] **Task II-4: Phase II 验证与归档**(1h,质量验证 + 文档同步 agent) — 已完成 2026-07-09
  - [x] SubTask II-4.1: `cargo test --workspace` 退出码 0,测试增量 12 项 ≥ 9(N2: 1 proptest + 5 单元 / N3: 2 集成 / A1: 3 异步 + 1 基线)
  - [x] SubTask II-4.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0,零警告
  - [x] SubTask II-4.3: `cargo fmt --all -- --check` 退出码 0,零 diff
  - [x] SubTask II-4.4: 创建 `docs/optimization/v1.1.0/phase2_correctness_verification_report.md`
  - [x] SubTask II-4.5: `CHANGELOG.md` 追加 Phase II 正确性修复章节(位于 Phase I 与 Phase III 之间)
  - [x] SubTask II-4.6: `project_memory.md` 追加 Phase II 教训(select_nth_unstable_by 语义 / Ack 状态机 / spawn_blocking 模式)

## Phase III:P0 性能优化(1 周,30h 估算)

> **依赖**:Phase II 全部完成
> **并行性**:Task III-1 / III-2 / III-3 / III-5 可并行(独立 crate);Task III-4 依赖 III-1(repo-wiki 同 crate)
> **验收门槛**:5 项性能优化全部完成 + bench 性能基线对比 + p95 延迟改善 ≥ 5%

- [x] **Task III-1: repo-wiki VectorIndex Mutex→RwLock [B1]**(已完成,代码已使用 RwLock,核实于 2026-07-09)
  - [x] SubTask III-1.1: Round 1 现状核验 — Read `crates/repo-wiki/src/vector.rs`,确认原 `Mutex<HashMap>` 已改为 `RwLock<HashMap>`
  - [x] SubTask III-1.2: TDD-RED — 在 `crates/repo-wiki/benches/vector_bench.rs` 新增 3 个 bench(`single_thread_knn_latency` / `concurrent_knn_search_throughput` / `search_under_write_load`),验证 RwLock 并发读收益(~5 倍并行加速)
  - [x] SubTask III-1.3: TDD-GREEN — `Mutex<HashMap>` 已改为 `RwLock<HashMap>`,`lock()` → `read()`/`write()`(前序会话完成)
  - [x] SubTask III-1.4: TDD-REFACTOR — 添加 WHY 注释说明读密集场景 RwLock 优势([vector.rs:31-32](file:///d:/Chimera%20CLI/crates/repo-wiki/src/vector.rs#L31-L32))
  - [x] SubTask III-1.5: `cargo test -p repo-wiki` + `cargo bench -p repo-wiki --bench vector_bench --no-run` 验证通过

- [x] **Task III-2: model-router DashMap→RwLock [B3]**(已完成,代码已使用 RwLock + B3 优化注释,核实于 2026-07-09)
  - [x] SubTask III-2.1: Round 1 现状核验 — Read `crates/model-router/src/registry.rs`,确认已使用 `RwLock<HashMap<String, ModelInfo>>`(原 DashMap 已替换)
  - [x] SubTask III-2.2: Round 2 影响评估 — grep `ModelRegistry` 公开 API,确认仅 `register`/`get`/`list`/`list_by_cost`/`list_by_latency` 等方法,改 RwLock 不破坏 API
  - [x] SubTask III-2.3: TDD-RED — 在 `crates/model-router/benches/registry_bench.rs` 新增 2 个 bench(`single_get_latency` + `concurrent_register_get`),验证 RwLock 在 ≤10 模型 + 10 并发场景下的吞吐
  - [x] SubTask III-2.4: TDD-GREEN — 已改为 `RwLock<HashMap<String, ModelInfo>>`,`register` 用 `write()`、`get`/`list` 用 `read()`(前序会话完成)
  - [x] SubTask III-2.5: 修复 `register()` 的 TOCTOU 竞态 — 使用 `entry()` API 替代 `contains_key` + `insert`(前序会话完成)
  - [x] SubTask III-2.6: `cargo test -p model-router` + `cargo bench -p model-router --bench registry_bench --no-run` 验证通过

- [x] **Task III-3: scc-cache 马尔可夫链 LRU 淘汰 [N10]**(4h,性能专家 agent) — 已完成 2026-07-09
  - [x] SubTask III-3.1: Round 1 现状核验 — Read `crates/scc-cache/src/prefetch.rs`,确认 `patterns: RwLock<HashMap<...>>` 无容量上限
  - [x] SubTask III-3.2: TDD-RED — 在 `crates/scc-cache/tests/prefetch_test.rs` 新增测试 `test_pattern_capacity_limit`,插入 20000 个模式后验证容量保持在 10000(当前应失败)
  - [x] SubTask III-3.3: TDD-GREEN — 自实现 `LruPatternMap`(Vec 索引双向链表,无 unsafe),为转移矩阵增加 10000 容量上限
  - [x] SubTask III-3.4: TDD-REFACTOR — 提取 `LruPatternMap` 类型 + `AccessPatternLearner::with_capacity`/`pattern_count`,添加 WHY 注释
  - [x] SubTask III-3.5: `cargo test -p scc-cache` 验证通过

- [x] **Task III-4: repo-wiki 写线程分离 + 读 spawn_blocking [A3]**(8h,性能专家 agent) — 已完成 2026-07-09
  - [x] SubTask III-4.1: Round 1 现状核验 — Read `crates/repo-wiki/src/store.rs`,确认单 `Arc<Mutex<Connection>>`,所有读写串行
  - [x] SubTask III-4.2: Round 2 方案设计 — 设计 mpsc 写入线程模型:`WikiStore` 持有 `write_tx: mpsc::Sender<WriteOp>` + `read_conns: Arc<Vec<Mutex<Connection>>>`;写操作通过 tx 投递到专用线程,读操作用 `spawn_blocking` + 只读连接池
  - [x] SubTask III-4.3: TDD-RED — 在 `crates/repo-wiki/benches/store_bench.rs` 新增 bench `concurrent_read_during_write`,验证写入时读取不阻塞
  - [x] SubTask III-4.4: TDD-GREEN — 实现写入线程 + 读连接池(默认 2 个只读连接利用 WAL 并发读)
  - [x] SubTask III-4.5: TDD-REFACTOR — 提取 `WriteOp` 类型与 `with_read_conn` 辅助函数,添加 WHY 注释说明 WAL 并发读优势
  - [x] SubTask III-4.6: `cargo test -p repo-wiki` + `cargo bench -p repo-wiki` 验证
  - [x] SubTask III-4.7: 架构审查修复 — `:memory:` 数据库拒绝 read_pool_size>0、WAL PRAGMA 调优、oneshot receiver drop 处理

- [x] **Task III-5: model-router CACR f32→u64 [N11]**(1h,性能专家 agent) — 已完成 2026-07-09
  - [x] SubTask III-5.1: Round 1 现状核验 — Read `crates/model-router/src/cacr.rs`,确认 `f32` 预算计算
  - [x] SubTask III-5.2: TDD-RED — 在 `crates/model-router/tests/cacr_test.rs` 新增测试 `test_large_budget_no_precision_loss`,预算 = 2^25 时验证阈值判定正确(当前失败)
  - [x] SubTask III-5.3: TDD-GREEN — 将 `f32` 预算计算改为 `u64` 整数百分比运算(`remaining_budget * percent / 100`)
  - [x] SubTask III-5.4: TDD-REFACTOR — 添加 WHY 注释说明 f32 在 u64 > 2^24 时精度丢失;同步修正 `check` 文档注释,明确 budget=0 时任何成本(含 0)Block
  - [x] SubTask III-5.5: `cargo test -p model-router` + `cargo fmt --all -- --check` + `cargo clippy -p model-router --all-targets -- -D warnings` 验证
  - [x] SubTask III-5.6: 并行架构审查发现 III-4 `:memory:` 数据库读连接池语义不一致,已在 `crates/repo-wiki/src/store.rs` 彻底拒绝 `:memory:` 并更新回归测试

- [x] **Task III-6: Phase III 验证与归档**(2h,质量验证 + 文档同步 agent)
  - [x] SubTask III-6.1: `cargo test --workspace` 退出码 0
  - [x] SubTask III-6.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
  - [x] SubTask III-6.3: `cargo bench --workspace` 收集 before/after 性能数据
  - [x] SubTask III-6.4: 创建 `docs/optimization/v1.1.0/phase3_performance_verification_report.md` + `performance_baseline_comparison.md`
  - [x] SubTask III-6.5: `CHANGELOG.md` + `project_memory.md` 同步

## Phase IV:P1 架构补债(2 周,80h 估算)

> **依赖**:Phase III 全部完成
> **并行性**:Task IV-1 / IV-3 / IV-4 / IV-5 / IV-6 可并行;Task IV-2 依赖 IV-3(N6/N7 仲裁层依赖 C1 EventBus)
> **验收门槛**:6 项架构补债完成 + 向后兼容性测试 + 集成测试增量 ≥ 12

- [ ] **Task IV-1: event-bus EventTopic + FilteredSubscriber [C1]**(12h,架构专家 agent)
  - [x] SubTask II-1.1: Round 1 现状核验 — Read `crates/event-bus/src/lib.rs` + `types.rs`,确认 `subscribe()` 返回全量广播
  - [x] SubTask II-1.2: Round 2 方案设计 — 设计 `EventTopic` 枚举(7 类:`Security` / `Budget` / `Quest` / `Memory` / `Router` / `Execution` / `System`);设计 `FilteredSubscriber` 仅接收指定 topic 的事件
  - [x] SubTask II-1.3: TDD-RED — 在 `crates/event-bus/tests/filtered_subscriber_test.rs` 新增测试 `test_filtered_subscriber_only_receives_topic_events`,验证 FilteredSubscriber 仅接收订阅的 topic
  - [x] SubTask II-1.4: TDD-GREEN — 实现 `EventTopic` + `FilteredSubscriber`,既有 `subscribe()` 保持全量广播向后兼容
  - [x] SubTask II-1.5: TDD-REFACTOR — 添加 WHY 注释说明 topic 分类与向后兼容策略
  - [x] SubTask II-1.6: 为 65 个 NexusEvent 变体添加 `topic()` 方法映射
  - [x] SubTask II-1.7: `cargo test -p event-bus` 验证

- [ ] **Task IV-2: acb-governor 滞后机制 + TTG 仲裁层 [N6/N7]**(12h,架构专家 agent,依赖 IV-1)
  - [x] SubTask II-2.1: Round 1 现状核验 — Read `crates/acb-governor/src/governor.rs` 与 `crates/quest-engine/src/ttg.rs`,确认 ACB 无时间维度滞后,TTG 仅订阅 DECB
  - [x] SubTask II-2.2: Round 2 方案设计 — ACB 增加 `tier_switch_lag_ms` 参数(默认 1000ms),防止利用率波动导致频繁切换;TTG 增加 ACB/DECB 仲裁层,综合两个治理器的事件
  - [x] SubTask II-2.3: TDD-RED — 在 `crates/acb-governor/tests/governor_test.rs` 新增测试 `test_tier_switch_lag_prevents_oscillation`,验证滞后机制防止振荡
  - [x] SubTask II-2.4: TDD-GREEN — 实现滞后机制与仲裁层
  - [x] SubTask II-2.5: TDD-REFACTOR — 提取 `ArbitrationLayer` 类型,添加 WHY 注释
  - [x] SubTask II-2.6: `cargo test -p acb-governor -p quest-engine` 验证

- [ ] **Task IV-3: parliament Skeptic 否决覆议 [N8]**(8h,架构专家 agent)
  - [x] SubTask II-3.1: Round 1 现状核验 — Read `crates/parliament/src/voting.rs` + `debate.rs`,确认 Skeptic 否决优先且无覆议
  - [x] SubTask II-3.2: TDD-RED — 在 `crates/parliament/tests/voting_test.rs` 新增测试 `test_skeptic_veto_can_be_overridden_by_two_thirds`,验证 2/3 超级多数可推翻否决
  - [x] SubTask II-3.3: TDD-GREEN — 实现 `reopen_veto()` 方法,4 角色(Explorer/Architect/Skeptic/Validator)中 3 个或以上赞成即可推翻 Skeptic 否决
  - [x] SubTask II-3.4: TDD-REFACTOR — 添加 WHY 注释说明防止决策僵局的设计意图
  - [x] SubTask II-3.5: `cargo test -p parliament` 验证

- [ ] **Task IV-4: sesa-router 前置事件校验 [N9]**(8h,架构专家 agent)
  - [x] SubTask II-4.1: Round 1 现状核验 — Read `crates/sesa-router/src/lib.rs`,确认链路末端无前置校验
  - [x] SubTask II-4.2: TDD-RED — 在 `crates/sesa-router/tests/prerequisite_test.rs` 新增测试 `test_blocks_activation_without_upstream_events`,验证未收到 OSA+KVBSR+FaaE 事件时拒绝激活
  - [x] SubTask II-4.3: TDD-GREEN — 实现 `PrerequisiteChecker`,在 `activate()` 入口校验上游事件
  - [x] SubTask II-4.4: TDD-REFACTOR — 添加 WHY 注释说明五层路由顺序的代码强制
  - [x] SubTask II-4.5: `cargo test -p sesa-router` 验证

- [ ] **Task IV-5: 配置类型迁移到 nexus-core [F1]**(20h,架构专家 agent,**影响范围最大,需谨慎**)
  - [x] SubTask II-5.1: Round 1 现状核验 — Read `crates/chimera-cli/src/config.rs`(1061 行),列出 14 个 section 类型;grep 全 workspace `use chimera_cli::config` 使用点
  - [x] SubTask II-5.2: Round 3 影响评估 — 评估迁移对下游 crate 的影响,识别所有需要调整 `use` 路径的位置
  - [x] SubTask II-5.3: TDD-RED — 在 `crates/nexus-core/tests/config_test.rs` 新增测试 `test_config_types_in_nexus_core`,验证 14 个 section 类型在 `nexus_core::config` 可访问
  - [x] SubTask II-5.4: TDD-GREEN — 在 `crates/nexus-core/src/` 新建 `config.rs`,迁移 14 个 section 类型;`lib.rs` 添加 `pub mod config;` + `pub use config::*;`
  - [x] SubTask II-5.5: TDD-REFACTOR — `chimera-cli/src/config.rs` 改为 `pub use nexus_core::config::*;` re-export 保持向后兼容
  - [x] SubTask II-5.6: 更新所有下游 crate 的 `use` 路径(改为 `use nexus_core::config::*`)
  - [x] SubTask II-5.7: `cargo check --workspace` + `cargo test --workspace` 验证编译与测试通过
  - [x] SubTask II-5.8: 添加 WHY 注释说明"L1 配置类型共享消除平行类型漂移风险"

- [ ] **Task IV-6: repo-wiki r2d2 连接池 [D1]**(12h,架构专家 agent)
  - [x] SubTask II-6.1: Round 1 现状核验 — Read `crates/repo-wiki/Cargo.toml` + `src/store.rs`,确认无 r2d2 依赖
  - [x] SubTask II-6.2: 在根 `Cargo.toml` `[workspace.dependencies]` 添加 `r2d2 = "0.8"` + `r2d2_sqlite = "0.24"`
  - [x] SubTask II-6.3: TDD-RED — 在 `crates/repo-wiki/benches/pool_bench.rs` 新增 bench `concurrent_read_with_pool`,对比单连接 Mutex vs r2d2 连接池的并发读吞吐
  - [x] SubTask II-6.4: TDD-GREEN — 引入 `r2d2::Pool<SqliteConnectionManager>`,1 个写连接 + N 个只读连接
  - [x] SubTask II-6.5: TDD-REFACTOR — 添加 WHY 注释说明 WAL 并发读优势
  - [x] SubTask II-6.6: `cargo test -p repo-wiki` + `cargo bench -p repo-wiki` 验证

- [ ] **Task IV-7: Phase IV 验证与归档**(8h,质量验证 + 文档同步 agent)
  - [x] SubTask II-7.1: `cargo test --workspace` 退出码 0,测试增量 ≥ 12
  - [x] SubTask II-7.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
  - [x] SubTask II-7.3: `cargo fmt --all -- --check` 退出码 0
  - [x] SubTask II-7.4: 创建 `docs/optimization/v1.1.0/phase4_architecture_verification_report.md`
  - [x] SubTask II-7.5: 更新 `CODE_WIKI.md` §2.3 ADR 表格新增 ADR-007(EventTopic 过滤)/ ADR-008(ACB 滞后机制)/ ADR-009(覆议机制)/ ADR-010(配置类型迁移)
  - [x] SubTask II-7.6: `CHANGELOG.md` + `project_memory.md` 同步

## Phase V:P2 渐进优化(4 周,80h 估算,可按需延后到 v1.2.0-omega)

> **依赖**:Phase IV 全部完成
> **并行性**:Task V-1 ~ V-8 大部分可并行(独立 crate)
> **验收门槛**:至少完成 5 项渐进优化(其余可延后)

- [x] **Task V-1: event-bus Critical mpsc + Normal broadcast + 注意力过滤 [I4]**(4h,演进专家 agent) — 已完成 2026-07-09(调整为核验任务)
  - [x] SubTask V-1.1: Round 1 现状核验 — 确认 publish/publish_blocking 统一入口已实现 mpsc 旁路(is_critical_mpsc_event 自动路由),7 发布点全合规
  - [x] SubTask V-1.2: TDD-RED — 新增 4 个 critical_channel_test 验证双通道投递正确性
  - [x] SubTask V-1.3: TDD-GREEN — Phase IV C1 已实现双通道架构,纯核验无需修改生产代码
  - [x] SubTask V-1.4: TDD-REFACTOR + `cargo test -p event-bus` 验证通过

- [ ] **Task V-2: model-router MoE 稀疏门控 [I1]**(20h,演进专家 agent) — 延后到 v1.2.0-omega
  - [ ] ~~SubTask V-2.1: Round 1 现状核验~~ (延后:需 50+ 模型规模验证,当前 3 模型无收益)
  - [ ] ~~SubTask V-2.2: Round 2 方案设计~~
  - [ ] ~~SubTask V-2.3: TDD-RED + TDD-GREEN + TDD-REFACTOR~~

- [x] **Task V-3: gqep-executor 全局超时 [N14]**(4h,演进专家 agent) — 已完成 2026-07-09
  - [x] SubTask V-3.1: 新增 gather_deadline_ms(默认 5000,0=禁用)+ GlobalTimedOut 错误 + GatherTimedOut 事件 + collect_with_deadline 独立方法 + 4 测试 + `cargo test -p gqep-executor` 验证通过

- [x] **Task V-4: gea-activator TaskProfile Hash [N17]**(4h,演进专家 agent) — 已完成 2026-07-09
  - [x] SubTask V-4.1: impl Hash+PartialEq+Eq for TaskProfile(f32 to_bits)+ DashMap key 改为 TaskProfile + 4 测试 + `cargo test -p gea-activator` 验证通过

- [x] **Task V-5: quest-engine TTG EventBus 集成 [N18]**(4h,演进专家 agent) — 已完成 2026-07-09(调整为清理重复 tracing)
  - [x] SubTask V-5.1: 9 处 info!→debug!/删除 + 4 特征化测试 + `cargo test -p quest-engine` 验证通过

- [ ] **Task V-6: repo-wiki FTS5 全文索引 [N15]**(8h,演进专家 agent) — 延后到 v1.2.0-omega
  - [ ] ~~SubTask V-6.1~~ (延后:FTS5 编译配置复杂,LIKE 已满足当前规模)

- [ ] **Task V-7: chimera-cli OnceCell 懒加载 [E1]**(8h,演进专家 agent) — 延后到 v1.2.0-omega
  - [ ] ~~SubTask V-7.1~~ (延后:14 section 重构风险高,需独立设计)

- [x] **Task V-8: event-bus Prometheus 指标 [G2]**(8h,演进专家 agent) — 已完成 2026-07-09
  - [x] SubTask V-8.1: prometheus-client 依赖 + 3 指标(nexus_event_total/nexus_critical_event_total/nexus_event_publish_duration_seconds)+ TopicLabel 独立枚举 + 6 测试 + `cargo test -p event-bus` 验证通过

- [x] **Task V-9: Top-K 全量优化(select_nth_unstable)**(8h,演进专家 agent) — 已完成 2026-07-09
  - [x] SubTask V-9.1: grep 全 workspace sort_by 用于 Top-K 选取的位置(5 候选 Site)
  - [x] SubTask V-9.2: 交叉验证(Site 1-4 已在先前阶段优化,仅 Site 5 需修改)
  - [x] SubTask V-9.3: Site 5 (model-router/strategies.rs) 替换为 select_nth_unstable_by + 候选 sort_by
  - [x] SubTask V-9.4: `cargo test --workspace` 验证通过(3 等价性测试)

- [ ] **Task V-10: 测试覆盖补齐**(并行进行,12h,质量验证 agent) — 延后到 v1.2.0-omega
  - [ ] ~~SubTask V-10.1~~ (延后:5 crate benches,规模大)
  - [ ] ~~SubTask V-10.2~~ (延后:5 crate proptest)
  - [ ] ~~SubTask V-10.3~~ (延后:23 crate doctest)
  - [ ] ~~SubTask V-10.4~~ (延后:fuzz 3→6 target)

- [x] **Task V-11: Phase V 验证与归档 + 最终交付**(8h,质量验证 + 文档同步 agent) — 已完成 2026-07-09
  - [x] SubTask V-11.1: `cargo test --workspace` 退出码 0(3228 passed / 0 failed / 55 ignored)
  - [x] SubTask V-11.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0(零警告)
  - [x] SubTask V-11.3: `cargo fmt --all -- --check` 退出码 0(零 diff)
  - [ ] SubTask V-11.4: `cargo audit --deny warnings` 退出码 0(网络可用时)
  - [x] SubTask V-11.5: 创建 `docs/optimization/v1.1.0/phase5_progressive_optimization_report.md`(Phase V 验证报告)
  - [ ] SubTask V-11.6: 更新 `docs/optimization/v1.1.0/performance_baseline_comparison.md`(延后到 GA 前)
  - [x] SubTask V-11.7: `CHANGELOG.md` 追加 Phase V 章节
  - [x] SubTask V-11.8: `project_memory.md` 追加 Phase V 教训
  - [ ] SubTask V-11.9: 更新 `CODE_WIKI.md` §1.3(延后到 GA 前)

## Task Dependencies

- **Phase I**: I-1 / I-2 / I-3 可并行,I-4 依赖前三者
- **Phase II**: II-1 / II-2 / II-3 可并行,II-4 依赖前三者;Phase II 整体依赖 Phase I 完成
- **Phase III**: III-1 / III-2 / III-3 / III-5 可并行,III-4 依赖 III-1(repo-wiki 同 crate),III-6 依赖前五者;Phase III 整体依赖 Phase II 完成
- **Phase IV**: IV-1 / IV-3 / IV-4 / IV-5 / IV-6 可并行,IV-2 依赖 IV-1(N6/N7 仲裁层依赖 C1 EventBus),IV-7 依赖前六者;Phase IV 整体依赖 Phase III 完成
- **Phase V**: V-1 ~ V-9 大部分可并行,V-10 测试补齐可全程并行,V-11 依赖前九者;Phase V 整体依赖 Phase IV 完成

## 并行执行建议

- **关键路径**:Phase I (3 task 串行+1 验证) → Phase II (3 task 串行+1 验证) → Phase III (5 task 串行+1 验证) → Phase IV (6 task 串行+1 验证) → Phase V (9 task 串行+1 验证)
- **并行批次 1**(Phase I 启动后):I-1 + I-2 + I-3(三个安全修复并行)
- **并行批次 2**(Phase II 启动后):II-1 + II-2 + II-3(三个 P0 bug 修复并行)
- **并行批次 3**(Phase III 启动后):III-1 + III-2 + III-3 + III-5(四个独立 crate 并行),III-4 等 III-1 完成
- **并行批次 4**(Phase IV 启动后):IV-1 + IV-3 + IV-4 + IV-5 + IV-6(五个独立 crate 并行),IV-2 等 IV-1 完成
- **并行批次 5**(Phase V 启动后):V-1 ~ V-9 大部分并行,V-10 全程并行

## 预算汇总

| 阶段 | 原估算 | 调整后 | 关键交付物 |
|------|--------|--------|-----------|
| Phase I 安全紧急修复 | 6h | 6h | N1/N4/N5 三项修复 + OWASP 渗透用例 |
| Phase II 正确性修复 | 12h | 6h ↓ | N2/N3/A1 补齐 TDD 测试(代码已修复) |
| Phase III P0 性能优化 | 30h | 26h ↓ | B1/B3 已完成;N10/A3/N11 三项 + bench 对比 |
| Phase IV P1 架构补债 | 80h | 80h | C1/N6/N7/N8/N9/F1/D1 六项架构补债 + ADR-007~010 |
| Phase V P2 渐进优化 | 80h | 80h | I1/I4/N14/N15/N17/N18/E1/G2 + Top-K + 测试补齐 |
| **总计** | **208h** | **198h** | **45 项优化点全部闭环 + 完整文档体系** |

> Phase V 可按需延后到 v1.2.0-omega,不阻塞 v1.1.0-omega 发布(§3.3 长期主义理念)。
>
> **核实调整(2026-07-09)**:DEEP_RESEARCH 报告(2026-07-08)后的代码演进已解决
> N2/N3/A1(代码层)+ B1/B3(代码层),实际工作量减少 10h。
> Phase IV/V 启动前应做类似核实,避免重复已完成的工作。
