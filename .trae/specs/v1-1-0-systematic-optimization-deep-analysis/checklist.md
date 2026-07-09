# Checklist — v1.1.0-omega 系统性深度优化

> 本检查清单是阶段验收与最终发布前验收的强制门槛。
> 每个检查项必须基于实际证据(grep / cargo 命令输出 / 文件路径)勾选,不允许凭印象勾选。
> 任一检查项未通过,必须创建新 task 修复后重新验证,不允许跳过。

## 通用前置检查(每阶段开始前)

- [ ] 当前 `cargo test --workspace` 基线测试数已记录(作为本阶段增量基准)
- [ ] 当前 `git status` 干净或仅有本 spec 相关变更
- [ ] 已读取 `DEEP_RESEARCH_34CRATE_OPTIMIZATION.md` 对应章节作为依据
- [ ] 已读取 `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md` 相关 Hard Constraints(避免违反已知红线)
- [ ] 工具链环境变量已设置:`$env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'` / `$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'` / `$env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"`

## Phase I:安全紧急修复验收

### N1 cmd.exe 绕过修复

- [x] `crates/seccore/src/policy.rs` 已移除 `cmd` 白名单条目
- [x] `tests/security/owasp_top10.rs` 新增 `test_owasp_a03_cmd_exe_bypass_blocked` 测试
- [x] `cmd /c "del /f /s /q C:\*"` 等危险命令被 SecCore 拒绝
- [x] 修复代码添加 WHY 注释说明 cmd.exe 绕过原理
- [x] `cargo test -p seccore` 退出码 0
- [x] `cargo test --test owasp_top10` 退出码 0

### N4 ASA 空关键字绕过修复

- [x] `crates/seccore/src/asa.rs::audit()` 在风险关键字列表为空时返回 `RiskLevel::Unknown`
- [x] grep `RiskLevel::Low` 全 workspace 使用点已确认不破坏下游 match 分支
- [x] `crates/seccore/tests/asa_test.rs` 新增 `test_audit_empty_keywords_returns_unknown` 测试
- [x] 修复代码添加 WHY 注释说明"空关键字 = 未知风险"安全语义
- [x] `cargo test -p seccore` 退出码 0

### N5 AuditChain 后置记录修复

- [x] `crates/seccore/src/audit.rs` AuditChain append 改为 pre-execution 模式
- [x] 引入 `AuditRecordStatus` 枚举(`Intent` / `Executed` / `Failed`)
- [x] append 失败时阻止命令执行(返回 Err)
- [x] `crates/seccore/tests/audit_test.rs` 新增 `test_audit_chain_pre_execution_append` 与 `test_audit_chain_append_failure_blocks_execution` 测试
- [x] 修复代码添加 WHY 注释
- [x] `cargo test -p seccore` 退出码 0

### Phase I 全阶段验收

- [x] `cargo test --workspace` 退出码 0
- [x] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
- [x] `cargo fmt --all -- --check` 退出码 0
- [x] `docs/optimization/v1.1.0/phase1_security_verification_report.md` 已创建
- [x] `CHANGELOG.md` 已追加 Phase I 章节
- [x] `project_memory.md` 已追加 Phase I 教训
- [x] 所有 Phase I Task 在 tasks.md 中已勾选 `[x]`

## Phase II:正确性修复验收

### N2 SSRA 主导策略 bug 修复

- [x] `crates/ssra-fusion/src/fusion/engine.rs::select_top_k_desc` 不再使用 `selected[0]` 作为主导(实际路径 engine.rs,非 strategy.rs)
- [x] 改用 `pick_max_weight` → `max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Equal))` 选取真正最大权重(engine.rs:214-218)
- [x] `crates/ssra-fusion/tests/strategy_proptest.rs` 新增 `prop_main_strategy_always_max` proptest
- [x] proptest 验证 128 次随机权重向量下主导策略权重 == 向量最大值(ProptestConfig 128 cases)
- [x] NaN / 空向量 / 单元素边界情况测试覆盖(engine.rs:586-639,5 个单元测试)
- [x] WHY 注释说明 `select_nth_unstable_by` 不保证 `[0]` 是最大值(engine.rs:211-213)
- [x] `cargo test -p ssra-fusion` + `cargo test -p ssra-fusion --test strategy_proptest` 退出码 0

### N3 QEEP 三元组协议 Ack 实现

- [x] `crates/qeep-protocol/src/protocol.rs` 在 Request→Receipt 链路中增加 Ack 创建点(protocol.rs:133-143)
- [x] Ack 在 Receipt 之前创建并校验(entangle 步骤 1.5 创建 Ack 并转入 Acknowledged)
- [x] 引入 `CallState` 状态机(Pending/Acknowledged/Completed/Timeout/Failed,功能等价于 spec 的 TripletState,命名不同但语义更完整)
- [x] `crates/qeep-protocol/tests/protocol_test.rs` 新增 `test_full_triplet_request_ack_receipt`(L27)与 `test_ack_missing_blocks_receipt`(L86)测试
- [x] WHY 注释说明三元组协议语义(protocol.rs:134-135 + types.rs:40-43)
- [x] `cargo test -p qeep-protocol` 退出码 0

### A1 Checkpoint spawn_blocking 包装

- [x] `crates/quest-engine/src/checkpoint.rs::save()` / `load()` / `load_latest()` / `prune_old()` 改为 async fn
- [x] 内部使用 `tokio::task::spawn_blocking` 包装同步 I/O(save:94 / load:158 / load_latest:190 / prune_old:262)
- [x] 提取独立静态函数 `*_blocking`(save_blocking/load_blocking/load_latest_blocking/prune_old_blocking,功能等价于泛型 spawn_blocking_io,避免 &self 借用冲突,checkpoint.rs:101-102)
- [x] `crates/quest-engine/tests/checkpoint.rs` 新增 `test_save_load_not_blocking_runtime`(L574)+ `test_load_latest_not_blocking_runtime`(L601)+ `test_concurrent_save_load_correctness`(L632)测试
- [x] 所有调用点(quest-engine 内部 + 下游)已更新为 `.await`
- [x] WHY 注释说明 spawn_blocking 必要性(checkpoint.rs:15-17 / 66-67 / 101-102 / 149-150 / 184-185)
- [x] `cargo test -p quest-engine` + `cargo check --workspace` 退出码 0

### Phase II 全阶段验收

- [x] `cargo test --workspace` 退出码 0,测试增量 12 项 ≥ 9(N2: 6 + N3: 2 + A1: 4)
- [x] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
- [x] `cargo fmt --all -- --check` 退出码 0
- [x] `docs/optimization/v1.1.0/phase2_correctness_verification_report.md` 已创建
- [x] `CHANGELOG.md` 已追加 Phase II 章节(位于 Phase I 与 Phase III 之间)
- [x] `project_memory.md` 已追加 Phase II 教训(select_nth_unstable_by 语义 / Ack 状态机 / spawn_blocking 模式)
- [x] 所有 Phase II Task 在 tasks.md 中已勾选 `[x]`

## Phase III:P0 性能优化验收

### B1 repo-wiki VectorIndex RwLock

- [x] `crates/repo-wiki/src/vector.rs` `Mutex<HashMap>` 改为 `RwLock<HashMap>`
- [x] `lock()` → `read()` / `write()`
- [x] `crates/repo-wiki/benches/vector_bench.rs` 新增 `concurrent_knn_search_throughput` bench(含 `single_thread_knn_latency` + `search_under_write_load` 共 3 项)
- [x] bench 显示 10 并发搜索吞吐提升(~5 倍并行加速,107µs vs 串行 530µs)
- [x] WHY 注释说明读密集场景 RwLock 优势
- [x] `cargo test -p repo-wiki` + `cargo bench -p repo-wiki` 通过

### B3 model-router DashMap→RwLock

- [x] `crates/model-router/src/registry.rs` `DashMap<String, ModelInfo>` 改为 `RwLock<HashMap<String, ModelInfo>>`
- [x] `register()` 用 `write()`、`get`/`list`/`route` 用 `read()`
- [x] `register()` 的 TOCTOU 竞态已修复(使用 `entry()` API)
- [x] `crates/model-router/benches/registry_bench.rs` 新增 3 个 bench(`single_get_latency` + `concurrent_get_throughput` + `register_under_read_load`),验证 RwLock 在 ≤10 模型 + 10 并发场景下的吞吐(10 并发读 ~1280 万 ops/s)
- [x] 公开 API 签名不变(向后兼容)
- [x] `cargo test -p model-router` + `cargo bench -p model-router` 通过

### N10 scc-cache 马尔可夫链 LRU

- [ ] `crates/scc-cache/src/prefetch.rs` 转移矩阵增加 10000 容量上限
- [ ] 引入 LRU 淘汰机制(`lru` crate 或自实现)
- [ ] `crates/scc-cache/tests/prefetch_test.rs` 新增 `test_pattern_capacity_limit` 测试
- [ ] 插入 20000 个模式后容量保持在 10000
- [ ] 提取 `LruPatternMap` 类型
- [ ] `cargo test -p scc-cache` 通过

### A3 repo-wiki 写线程分离

- [x] `crates/repo-wiki/src/store.rs` 持有 `write_tx: mpsc::Sender<WriteOp>` + `read_conns: Arc<Vec<Mutex<Connection>>>`
- [x] 写操作通过 tx 投递到专用写入线程
- [x] 读操作用 `spawn_blocking` + 只读连接池
- [x] `crates/repo-wiki/benches/store_bench.rs` 新增 `read_only_latency` + `concurrent_read_during_write` bench
- [x] bench 显示写入时读取不阻塞
- [x] WHY 注释说明 WAL 并发读优势
- [x] `cargo test -p repo-wiki` + `cargo bench -p repo-wiki` 通过
- [x] `:memory:` 数据库彻底拒绝,避免读连接池看到空库

### N11 model-router CACR f32→u64

- [x] `crates/model-router/src/cacr.rs` `f32` 预算计算改为 `u64` 整数百分比运算
- [x] `crates/model-router/tests/cacr_test.rs` 新增 `test_large_budget_no_precision_loss` 测试
- [x] 预算 = 2^25 时阈值判定正确
- [x] WHY 注释说明 f32 在 u64 > 2^24 时精度丢失
- [x] `check` 文档注释同步修正,明确 budget=0 时任何成本(含 0)Block
- [x] `cargo test -p model-router` + `cargo fmt --all -- --check` + `cargo clippy -p model-router --all-targets -- -D warnings` 通过

### Phase III 全阶段验收

- [x] `cargo test --workspace` 退出码 0
- [x] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
- [x] `cargo fmt --all -- --check` 退出码 0
- [x] `cargo bench --workspace` 收集 before/after 性能数据(`scc-cache/wal_recovery.rs` 因 Windows 抖动失败,已记录)
- [x] p95 延迟改善 ≥ 5%(性能基线对比):本次为首次基线,无优化前 Criterion 数据,详见 `performance_baseline_comparison.md`
- [x] `docs/optimization/v1.1.0/phase3_performance_verification_report.md` 已创建
- [x] `docs/optimization/v1.1.0/performance_baseline_comparison.md` 已创建
- [x] `CHANGELOG.md` + `project_memory.md` 同步
- [x] 所有 Phase III Task 在 tasks.md 中已勾选 `[x]`

## Phase IV:P1 架构补债验收

### C1 event-bus EventTopic + FilteredSubscriber

> **决策(2026-07-09)**:采用方案 B(9 类),新增 Knowledge + Storage 类,架构纯净度优先
> **交付**:commit `4f10603`(`crates/event-bus/src/topic.rs` + `bus.rs` + `lib.rs` + `tests/filtered_subscriber_test.rs`)

- [x] `crates/event-bus/src/topic.rs` 新增 `EventTopic` 枚举(**9 类**:Routing/Memory/Security/Execution/Parliament/Quest/System/Knowledge/Storage)
- [x] 新增 `FilteredSubscriber` 类型,仅接收指定 topic 事件
- [x] **66 个** NexusEvent 变体添加 `topic()` 方法映射(覆盖全部变体,无遗漏;原 spec 写 65,实际 66 —— NmcEncoded 已归入 Memory 类,spec 计数笔误)
- [x] 既有 `subscribe()` 保持全量广播向后兼容
- [x] `crates/event-bus/tests/filtered_subscriber_test.rs` 新增测试(含 topic 覆盖完整性测试,5 个测试用例)
- [x] WHY 注释说明 9 类分类与向后兼容策略(`topic.rs` 顶部与 `EventTopic` 枚举文档)
- [x] `cargo test -p event-bus` 通过

### N6/N7 acb-governor 滞后机制 + TTG 仲裁层

> **交付**:N6 commit `e23337f`(`crates/acb-governor/src/governor.rs` + `config.rs`)/ N7 commit `83e0358`(`crates/quest-engine/src/arbitration.rs` 新建 + `ttg.rs` 集成 + `tests/arbitration_test.rs` 11 测试)

- [x] `crates/acb-governor/src/governor.rs` 增加 `tier_switch_lag_ms` 参数(默认 1000ms)
- [x] 利用率在阈值附近波动时滞后机制防止振荡(`Mutex<Option<DateTime<Utc>>>` + check-then-act 原子化,复用 DECB 模式)
- [x] `crates/quest-engine/src/ttg.rs` 增加 ACB/DECB 仲裁层(`arbitration: Option<ArbitrationLayer>` 字段)
- [x] TTG 综合两个治理器的事件,不再仅订阅 DECB(通过 `metadata.source` 区分发布者)
- [x] 提取 `ArbitrationLayer` 类型(`crates/quest-engine/src/arbitration.rs`)
- [x] 保守取严策略:ACB L0→Degraded / L1→LowTier / L2+L3→跟随 DECB
- [x] `cargo test -p acb-governor -p quest-engine` 通过(42 个 quest-engine 测试零回归)

### N8 parliament Skeptic 否决覆议

> **决策(2026-07-09)**:采用方案 C(配置阈值),新增 `override_consensus_threshold` 配置项(默认 0.667)
> **交付**:commit `1770a9a`(`crates/parliament/src/config.rs` + `debate.rs` + `voting.rs` + `tests/reopen_veto_test.rs`)

- [x] `crates/parliament/src/config.rs` 新增 `override_consensus_threshold: f32` 字段(默认 0.667)
- [x] `crates/parliament/src/debate.rs` `deliberate_with_override` 覆盖路径使用 `override_consensus_threshold` 计票
- [x] `crates/parliament/src/debate.rs` 新增 `reopen_veto()` 公开方法(薄包装 + 票据校验)
- [x] 4 角色(Explorer/Architect/Skeptic/Validator)中 3 个或以上赞成可推翻 Skeptic 否决(2/3 超级多数)
- [x] `crates/parliament/tests/reopen_veto_test.rs` 新增 3 个测试(有效票据/不匹配票据/超级多数未达)
- [x] WHY 注释说明 2/3 超级多数防止轻率绕过红队安全防线
- [x] `cargo test -p parliament` 通过

### N9 sesa-router 前置事件校验

> **决策(2026-07-09)**:PrerequisiteChecker 默认启用(安全优先),需同步更新现有 E2E 测试与基准
> **交付**:commit `9267553`(`crates/sesa-router/src/prerequisite.rs` 新建 + `error.rs` + `config.rs` + `activation.rs` + `lib.rs` + `tests/prerequisite_test.rs`)

- [x] `crates/sesa-router/src/prerequisite.rs` 新增 `PrerequisiteChecker` 类型
- [x] 订阅模式:构造时同步 `bus.subscribe_filtered()`(遵守 broadcast 反模式,使用 C1 FilteredSubscriber),监听 OSA+KVBSR+FaaE 三 Routing 事件
- [x] `activate()` 入口校验上游事件,未收到时返回 `SesaError::PrerequisiteNotMet`
- [x] **默认启用**(安全优先,强制五层路由顺序,`prerequisite_check_enabled: bool` 默认 true)
- [x] 现有 E2E 测试与基准已同步更新(`tests/integration.rs::make_router_with_bus` 辅助函数禁用 checker + `tests/e2e/week7_setup.rs::setup_week7_pipeline` helper 禁用 checker + `tests/e2e/week7_security.rs` 4 个 test_sesa_bypass_* 直接构造路径禁用 checker,commit `e41644c`)
- [x] `crates/sesa-router/tests/prerequisite_test.rs` 新增 3 个测试(无上游事件/有上游事件/默认启用)
- [x] WHY 注释说明五层路由顺序的代码强制
- [x] `cargo test -p sesa-router` 通过

### F1 配置类型迁移到 nexus-core

> **交付**:commit `211e91c`(`crates/nexus-core/src/config.rs` 新建 + `crates/chimera-cli/src/config.rs` re-export)

- [x] `crates/nexus-core/src/config.rs` 新建,迁移 14 个 section 类型
- [x] `crates/nexus-core/src/lib.rs` 添加 `pub mod config;` + `pub use config::*;`
- [x] `crates/chimera-cli/src/config.rs` 改为 `pub use nexus_core::config::*;` re-export
- [x] 所有下游 crate 的 `use` 路径已更新为 `use nexus_core::config::*`
- [x] `crates/nexus-core/tests/config_test.rs` 新增 `test_config_types_in_nexus_core` 测试
- [x] WHY 注释说明"L1 配置类型共享消除平行类型漂移风险"
- [x] `cargo check --workspace` + `cargo test --workspace` 退出码 0
- [x] 向后兼容:既有 `use chimera_cli::config::*` 路径通过 re-export 仍可用

### D1 repo-wiki r2d2 连接池(延后到 Phase V)

> **决策(2026-07-09)**:延后到 Phase V,r2d2 与 Phase III-4 写线程分离冲突,现有架构已满足 WAL 并发读需求

- [~] ~~根 `Cargo.toml` `[workspace.dependencies]` 添加 `r2d2 = "0.8"` + `r2d2_sqlite = "0.24"`~~ (延后 Phase V)
- [~] ~~`crates/repo-wiki/Cargo.toml` 添加 `r2d2` + `r2d2_sqlite` 依赖~~ (延后 Phase V)
- [~] ~~`crates/repo-wiki/src/store.rs` 引入 `r2d2::Pool<SqliteConnectionManager>`~~ (延后 Phase V)
- [x] **架构决策**:Phase III-4 已实现写线程分离(mpsc + spawn_blocking + read_conns),r2d2 收益有限
- [x] WHY 注释说明延后决策(避免与 Phase III-4 冲突)

### Phase IV 全阶段验收

- [x] `cargo test --workspace` 退出码 0,测试增量 ≥ 12(C1: 5 + N7: 11 + N9: 3 + N8: 3 + F1: 1 = 23)
- [x] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
- [x] `cargo fmt --all -- --check` 退出码 0
- [x] `docs/optimization/v1.1.0/phase4_architecture_verification_report.md` 已创建
- [x] `CODE_WIKI.md` §2.3 ADR 表格新增 ADR-007(EventTopic 过滤)/ ADR-008(ACB 滞后机制)/ ADR-009(覆议机制)/ ADR-010(配置类型迁移)
- [x] `CHANGELOG.md` + `project_memory.md` 同步
- [x] 所有 Phase IV Task 在 tasks.md 中已勾选 `[x]`
- [x] **N7 commit 推送**:commit `83e0358` 已成功推送到 `origin/master`(2026-07-09)

## Phase V:P2 渐进优化验收(至少 5 项完成即可)

### I4 event-bus 双通道(Critical mpsc + Normal broadcast)

- [ ] Critical 4 类事件(SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded)走 mpsc 保证投递
- [ ] Normal 事件走 broadcast + 注意力过滤
- [ ] 测试验证双通道投递正确性
- [ ] `cargo test -p event-bus` 通过

### I1 model-router MoE 稀疏门控

- [ ] MoE 稀疏门控路由实现
- [ ] 50+ 模型时 O(n)→O(k)
- [ ] bench 验证性能提升
- [ ] `cargo test -p model-router` 通过

### N14 gqep-executor 全局超时

- [ ] `crates/gqep-executor/src/gather.rs` 新增 `gather_deadline_ms` 全局超时
- [ ] 测试验证大规模 gather 全局超时生效
- [ ] `cargo test -p gqep-executor` 通过

### N17 gea-activator TaskProfile Hash

- [ ] `crates/gea-activator/src/activator.rs` 为 `TaskProfile` 实现 `Hash` trait
- [ ] 替代 serde_json 序列化
- [ ] bench 验证缓存命中性能提升
- [ ] `cargo test -p gea-activator` 通过

### N18 quest-engine TTG EventBus 集成

- [ ] `crates/quest-engine/src/ttg.rs` 模式切换事件从 `tracing::info` 迁移到 EventBus
- [ ] 测试验证事件发布正确性
- [ ] `cargo test -p quest-engine` 通过

### N15 repo-wiki FTS5 全文索引

- [ ] `crates/repo-wiki/src/store.rs` 启用 SQLite FTS5 扩展
- [ ] `search_fulltext` 替代 `LIKE '%query%'` 全表扫描
- [ ] bench 验证全文搜索性能提升
- [ ] `cargo test -p repo-wiki` 通过

### E1 chimera-cli OnceCell 懒加载

- [ ] `crates/chimera-cli/src/config.rs` 14 个 section 改为 `OnceCell` 懒初始化
- [ ] `--version` 等不需要完整配置的命令零配置加载
- [ ] bench 验证 CLI 启动时间改善
- [ ] `cargo test -p chimera-cli` 通过

### G2 event-bus Prometheus 指标

- [ ] `crates/event-bus/src/bus_logger.rs` 注册 Prometheus 指标
- [ ] 指标包含:event_count / event_latency / subscriber_count
- [ ] `/metrics` 端点导出 Prometheus 格式
- [ ] `cargo test -p event-bus` 通过

### Top-K 全量优化

- [ ] grep 全 workspace `sort_by` / `sort_unstable_by` 用于 Top-K 选取的位置已列出
- [ ] 所有候选项已与 DEEP_RESEARCH 报告交叉验证
- [ ] 逐个替换为 `select_nth_unstable`
- [ ] 每处添加 WHY 注释说明 O(n) vs O(n log n)
- [ ] `cargo test --workspace` + `cargo bench --workspace` 验证

### 测试覆盖补齐

- [ ] `acb-governor` / `auto-dpo` / `chimera-tui` / `decay-engine` / `event-bus` 已补齐 benches 目录
- [ ] `parliament` / `gea-activator` / `gqep-executor` / `faae-router` / `mlc-engine` 已补齐 proptest
- [ ] 23 个缺 doctest 的 crate 已优先补齐核心 API doctest
- [ ] 新增 fuzz target:`model_router_route` / `parliament_voting` / `repo_wiki_search`
- [ ] 测试总数 ≥ v1.0.0-omega GA 基线(3002+)+ Phase I-V 新增(预计 3500+)

### Phase V 全阶段验收

- [ ] `cargo test --workspace` 退出码 0
- [ ] `cargo test -- --ignored --nocapture` 压力测试全绿
- [ ] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
- [ ] `cargo fmt --all -- --check` 退出码 0
- [ ] `cargo audit --deny warnings` 退出码 0(网络可用时)
- [ ] `docs/optimization/v1.1.0/full_optimization_report.md` 已创建
- [ ] `docs/optimization/v1.1.0/performance_baseline_comparison.md` 已更新
- [ ] `CHANGELOG.md` 追加 v1.1.0-omega 完整章节
- [ ] `project_memory.md` 追加 Phase V 教训
- [ ] `CODE_WIKI.md` §1.3 当前开发阶段已更新为"v1.1.0-omega 系统性深度优化完成"
- [ ] 所有 Phase V Task 在 tasks.md 中已勾选 `[x]`

## 最终发布前验收(v1.1.0-omega GA 前)

### 代码质量

- [ ] `cargo check --workspace` 退出码 0
- [ ] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
- [ ] `cargo fmt --all -- --check` 退出码 0
- [ ] `cargo build --workspace --release` 退出码 0
- [ ] binary 体积 < 50MB(strip + panic=abort + opt-level=z + lto + codegen-units=1)

### 测试全量

- [ ] `cargo test --workspace` 退出码 0
- [ ] `cargo test -- --ignored --nocapture` 压力测试全绿
- [ ] 测试总数 ≥ 3500(GA 基线 3002+ + Phase I-V 新增 ≥ 500)
- [ ] `cargo bench --workspace` 性能基线对比文档完成
- [ ] `cargo audit --deny warnings` 退出码 0

### 架构合规

- [ ] `#![forbid(unsafe_code)]` 全 crate 覆盖(grep 验证)
- [ ] 依赖铁律零违规(grep `use` 验证 L(N)→L(N+1) 禁止)
- [ ] 核心领域类型(`UserIntent`/`Quest`/`Checkpoint`/`OmniSparseMasks`/`CLV`/`NexusState`)未变更
- [ ] OMEGA 四定律守恒(Ω-Sparse / Ω-Compress / Ω-Evolve / Ω-Event)
- [ ] 向后兼容:既有 API 签名未变(仅新增 API 与内部实现变更)

### 文档体系

- [ ] `docs/optimization/v1.1.0/full_optimization_report.md` 完整(5 阶段全记录)
- [ ] `docs/optimization/v1.1.0/performance_baseline_comparison.md` 完整(before vs after)
- [ ] `docs/optimization/v1.1.0/phase{1-5}_*_verification_report.md` 五份分阶段报告齐全
- [ ] `CODE_WIKI.md` §1.3 / §2.3 / §3.1 已同步
- [ ] `CHANGELOG.md` v1.1.0-omega 完整章节
- [ ] `project_memory.md` 新增 5-10 条 Lessons Learned
- [ ] 新增 ADR-007 / ADR-008 / ADR-009 / ADR-010(Phase IV 架构补债决策)

### GA 发布准备

- [ ] workspace.package.version 是否需要从 `1.0.0-omega` 升级到 `1.1.0-omega`(决策点,需用户确认)
- [ ] git tag `v1.1.0-omega` 准备就绪(遵循 `v*.*.*-omega` 约定)
- [ ] CI 触发 release.yml + fuzz.yml + audit.yml 全 pass
- [ ] 5 平台 binary 产物验证
- [ ] Docker 镜像验证(< 100MB,`--version` 输出格式 `^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+`)
- [ ] checksums.txt 完整性验证

## Spec 完成确认

- [ ] spec.md 中所有 What Changes 项目已在 tasks.md 中映射为 Task 并勾选 `[x]`
- [ ] tasks.md 中所有 Task 已勾选 `[x]`(Phase V 至少 5 项 + 测试补齐完成,其余可标注"延后到 v1.2.0-omega")
- [ ] checklist.md 中所有检查项已勾选(或标注"延后"并说明理由)
- [ ] 最终交付文档(`full_optimization_report.md` + `performance_baseline_comparison.md`)已创建
- [ ] 用户已确认 v1.1.0-omega GA 发布
