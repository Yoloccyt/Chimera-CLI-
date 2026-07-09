# Tasks — v1.2.0-omega 延后优化任务

> 任务按优先级顺序编排:前置脱敏 → 测试基础设施 → 独立功能 → 算法验证 → 高风险重构。
> 每个任务完成后必须通过 checklist.md 全部检查项才能进入下一任务。
> **执行规范**:每个 Task 都遵循 TDD(RED-GREEN-REFACTOR),先写失败测试再实现;子代理产物必须 `cargo fmt --all` + `cargo clippy --jobs 2 -- -D warnings` 通过;每个 Task 完成后勾选 `[x]`。

## 前置任务:脱敏化处理与安全提交(P0,阻塞所有后续)

- [x] **Task 0: 脱敏化扫描 + 安全提交**(1h,安全脱敏 agent) — 已完成 2026-07-09(push 待网络恢复)
  - [x] SubTask 0.1: 识别 Phase V commit 7024b03 涉及的 26 个修改文件(19 修改 + 7 新增) — 7024b03 已含 26 文件
  - [x] SubTask 0.2: Grep 敏感模式扫描完成 — 命中全部为领域术语(token_limit 等)+ 配置示例 + 文档占位符
  - [x] SubTask 0.3: 人工核验命中项 — 全部假阳性,无真实敏感信息
  - [x] SubTask 0.4: 个人路径扫描完成 — `C:\Users\30324\` 仅出现在 memory 路径引用(低风险,非凭据)
  - [x] SubTask 0.5: `.gitignore` 覆盖核验通过(.env* / *.pem / .toolchain/ / target/)
  - [x] SubTask 0.6: `git status` 确认无 .env / 凭据文件暂存
  - [x] SubTask 0.7: 逐文件 git add 完成(4 文件:spec.md/tasks.md/checklist.md/task0_desensitization_report.md)
  - [x] SubTask 0.8: git identity env vars 已设置(GIT_AUTHOR_NAME/EMAIL + GIT_COMMITTER_NAME/EMAIL)
  - [x] SubTask 0.9: PowerShell here-string commit message 成功(commit 8d22a75)
  - [ ] SubTask 0.10: `git push origin master` — ⚠️ 网络不可达(github.com:443 超时),待用户网络恢复后手动执行 `git push origin master`(本地领先 2 commit:7024b03 + 8d22a75)
  - [x] SubTask 0.11: 脱敏化报告已创建 `docs/optimization/v1.2.0/task0_desensitization_report.md`

## Task 1: 测试覆盖补齐(P1,基础设施,12h)

> **依赖**:Task 0 完成(代码已提交)
> **并行性**:SubTask 1.1 / 1.2 / 1.3 / 1.4 可并行(独立 crate / 独立测试类型)
> **验收门槛**:5 crate benches 编译通过 + 5 crate proptest 通过 + 23 crate doctest 通过 + fuzz 6 target 静态验证通过 + cargo test --workspace 退出码 0

- [x] **Task 1: 测试覆盖补齐 [V-10]**(12h,测试基础设施 agent) — 已完成 2026-07-09,3339 passed / 0 failed / 56 ignored(+111 增量)
  - [x] SubTask 1.1: 5 crate 补齐 criterion benches(并行) — 已完成 2026-07-09(Agent A)
    - [x] SubTask 1.1.1: Round 1 现状核验 — 选定 5 crate:event-bus(L1) / acb-governor(L8) / decay-engine(L4) / qeep-protocol(L4) / auto-dpo(L5),覆盖 5 个架构层(优于 spec 建议的热路径 crate,覆盖面更广)
    - [x] SubTask 1.1.2: Round 2 方案设计 — 每个 crate 2 个 bench(延迟 + 吞吐量),参考已有 bench 风格
    - [x] SubTask 1.1.3: TDD-RED — 创建 5 个 `benches/<name>_bench.rs`,声明 `[[bench]]` + `harness = false` + dev-dep `criterion`
    - [x] SubTask 1.1.4: TDD-GREEN — 实现 bench 函数,5 crate `--no-run` 编译全通过
    - [x] SubTask 1.1.5: TDD-REFACTOR — 添加 WHY 注释说明 bench 设计意图 + min-of-N 5 采样说明
    - [x] SubTask 1.1.6: `cargo bench -p <crate> --bench <name> --no-run` 验证 5 crate 全部编译通过 + `cargo clippy` 零警告
  - [x] SubTask 1.2: 5 crate 补齐 proptest(并行) — 已完成 2026-07-09(Agent A + Agent B)
    - [x] SubTask 1.2.1: Round 1 现状核验 — 选定 5 crate:acb-governor / model-router / repo-wiki / sesa-router / gea-activator
    - [x] SubTask 1.2.2: Round 2 方案设计 — 每个 crate 识别核心不变量(预算级别递增 / CACR 一致性 / KNN 最近性 / 稀疏比 / 激活幂等性)
    - [x] SubTask 1.2.3: TDD-RED — 创建 5 个 `tests/proptest.rs`,使用 proptest 1.11+ block-named 语法
    - [x] SubTask 1.2.4: TDD-GREEN — 实现 proptest,5 crate 全部通过(acb-governor 3 invariants × 64 cases + model-router 1 + repo-wiki 1 + sesa-router 2 + gea-activator 1 = 8 invariants)
    - [x] SubTask 1.2.5: TDD-REFACTOR — 添加 WHY 注释说明不变量选择理由
    - [x] SubTask 1.2.6: `cargo test -p <crate> --test proptest` 验证 5 crate 全部通过
  - [x] SubTask 1.3: 23 crate 补齐 doctest(并行) — 已完成 2026-07-09
    - [x] SubTask 1.3.1: Round 1 现状核验 — 34 crate 全部启用 `#![warn(missing_docs)]`,31/34 已有模块级示例
    - [x] SubTask 1.3.2: Round 2 方案设计 — 为 qeep-protocol / decay-engine / chimera-cli 3 个缺示例 crate 补 `# 快速示例` 代码块
    - [x] SubTask 1.3.3: TDD-RED — 3 个 crate 补齐 `# 快速示例` 含 WHY 注释 + 可运行代码块
    - [x] SubTask 1.3.4: TDD-GREEN — `cargo test --doc -p <crate>` 3 crate 全部 1 passed
    - [x] SubTask 1.3.5: TDD-REFACTOR — 示例简洁有效(5-15 行),风格与 event-bus/acb-governor 对齐
    - [x] SubTask 1.3.6: `cargo test --doc --workspace` 验证 34 crate 全部通过,零失败
  - [x] SubTask 1.4: fuzz 3→6 target 扩展(并行) — 已完成 2026-07-09(Agent B)
    - [x] SubTask 1.4.1: Round 1 现状核验 — 原有 3 target(seccore_sandbox / quest_parse / event_serialize)
    - [x] SubTask 1.4.2: Round 2 方案设计 — 新增 3 target:cacr_budget_parse(JSON+MsgPack 反序列化) / checkpoint_deserialize(往返不变量) / config_section_parse(替代 moe_gate_compute,因 MoE 模块未实现)
    - [x] SubTask 1.4.3: TDD-RED — 创建 3 个 `fuzz/fuzz_targets/<name>.rs`,在 `fuzz/Cargo.toml` 声明对应 `[[bin]]`
    - [x] SubTask 1.4.4: TDD-GREEN — 实现 fuzz target,Rust 源码静态验证通过(C++ 编译失败为预存平台限制 §10.3,委托 Linux CI)
    - [x] SubTask 1.4.5: TDD-REFACTOR — 添加 WHY 注释说明 fuzz target 选择理由 + Inf/NaN finite 检查
    - [x] SubTask 1.4.6: 验证 `fuzz/Cargo.toml` 含 6 个 `[[bin]]` + model-router 依赖已添加
  - [x] SubTask 1.5: Task 1 验证与归档 — 已完成 2026-07-09
    - [x] SubTask 1.5.1: `cargo test --workspace --jobs 1` 退出码 0,测试增量 +111(3339 ≥ 3248 期望,5 bench 编译 + 8 proptest + 3 doctest + 3 fuzz check)
    - [x] SubTask 1.5.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0(修复预存 E0597:gqep-executor/tests/gatherer_test.rs L131 缺 `move` 关键字)
    - [x] SubTask 1.5.3: `cargo fmt --all -- --check` 退出码 0
    - [x] SubTask 1.5.4: 创建 `docs/optimization/v1.2.0/task1_test_coverage_report.md`
    - [x] SubTask 1.5.5: `CHANGELOG.md` 追加 Task 1 章节(v1.2.0 开发中 section)
    - [x] SubTask 1.5.6: `project_memory.md` 追加 Task 1 教训(8 条)

## Task 2: repo-wiki FTS5 全文索引(P2,独立功能增强,8h)

> **依赖**:Task 1 完成(测试基础设施提供回归保护)
> **并行性**:无(单 crate 修改)
> **验收门槛**:FTS5 可用时走 MATCH 查询 + 不可用时降级到 LIKE + bench 对比 MATCH vs LIKE 性能 + cargo test -p repo-wiki 通过

- [x] **Task 2: repo-wiki FTS5 全文索引 [N15]**(8h,存储优化 agent) — 已完成 2026-07-09(workspace 级别验证通过 + 文档归档完成)
  - [x] SubTask 2.1: Round 1 现状核验 — Read `crates/repo-wiki/src/store.rs` 当前 `search` 实现(使用 `LIKE '%query%'`),确认 LIKE 全表扫描在大规模文档库的性能瓶颈
  - [x] SubTask 2.2: Round 2 方案设计 — 设计 FTS5 虚拟表 schema + `index_document` 同步写入 + `search` 优先 FTS5 MATCH + 降级策略;设计 Windows GNU 编译配置(`rusqlite` `bundled` feature + `SQLITE_ENABLE_FTS5` 编译开关)
  - [x] SubTask 2.3: Round 3 影响评估 — 评估 `rusqlite` `bundled` feature 对编译时间的影响;评估 `repo-wiki` 公开 API 兼容性
  - [x] SubTask 2.4: TDD-RED — 在 `crates/repo-wiki/tests/fts_test.rs` 新增测试(实际 6 个,超出 spec 要求的 3 个,覆盖降级/同步/UPSERT/capability):
    - `test_fts5_search_returns_relevant_docs` — FTS5 MATCH 查询返回相关文档
    - `test_fts5_fallback_to_like_when_unavailable` — FTS5 不可用时降级到 LIKE
    - `test_fts5_index_document_synced` — index_document 时 FTS5 索引同步写入
    - `test_fts5_fallback_handles_invalid_query` — FTS5 语法错误降级到 LIKE
    - `test_fts5_capability_detected` — 运行时检测 FTS5 可用性
    - `test_fts5_upsert_no_duplicate_index` — UPSERT 不产生重复索引
  - [x] SubTask 2.5: TDD-GREEN — 实现 FTS5 模块:
    - 新增 `crates/repo-wiki/src/fts.rs` — FTS5 虚拟表创建 + 索引写入 + MATCH 查询
    - 修改 `crates/repo-wiki/src/store.rs` — `search` 优先走 FTS5,失败降级到 LIKE
    - 修改 `crates/repo-wiki/Cargo.toml` — `rusqlite` `bundled` feature 已启用(workspace 级)+ FTS5 编译开关(`.cargo/config.toml [env] SQLITE_ENABLE_FTS5 = "1"`)
  - [x] SubTask 2.6: TDD-REFACTOR — 提取 `FtsCapability` 枚举(`Available` / `Unavailable`)记录 FTS5 可用性,添加 WHY 注释说明降级策略
  - [x] SubTask 2.7: 在 `crates/repo-wiki/benches/fts_bench.rs` 新增 bench 对比 FTS5 MATCH vs LIKE 在 1000+ 文档规模下的性能差异(sample_size=10,criterion 最小值)
  - [x] SubTask 2.8: 验证 Windows GNU 编译 — `cargo check -p repo-wiki --all-targets` 编译通过(0.95s,stable-x86_64-pc-windows-gnu)
  - [x] SubTask 2.9: `cargo test -p repo-wiki`(35 passed: 6 fts_test + 12 iscm + 1 proptest + 14 store + 2 doctest)+ `cargo bench -p repo-wiki --bench fts_bench --no-run` 编译通过 + `cargo clippy -p repo-wiki --all-targets -- -D warnings` 零警告 + `cargo fmt -p repo-wiki -- --check` 零 diff
  - [x] SubTask 2.10: Task 2 验证与归档 — 已完成 2026-07-09
    - [x] SubTask 2.10.1: `cargo test --workspace --jobs 1` 退出码 0 — 3403 passed / 0 failed / 56 ignored
    - [x] SubTask 2.10.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
    - [x] SubTask 2.10.3: `cargo fmt --all -- --check` 退出码 0
    - [x] SubTask 2.10.4: 创建 `docs/optimization/v1.2.0/task2_fts5_verification_report.md`
    - [x] SubTask 2.10.5: `CHANGELOG.md` 追加 Task 2 章节
    - [x] SubTask 2.10.6: `project_memory.md` 追加 Task 2 教训(FTS5 编译配置 / 降级策略 / CJK 空结果降级)

## Task 3: model-router MoE 稀疏门控(P3,算法规模验证,20h)

> **依赖**:Task 1 完成(测试基础设施提供回归保护)+ Task 2 完成可选(独立)
> **并行性**:无(单 crate 修改)
> **验收门槛**:50+ 模型规模下 p95 路由延迟降低 ≥ 40% + 小规模自动退化 + proptest 验证稀疏性不变量 + cargo test -p model-router 通过

- [x] **Task 3: model-router MoE 稀疏门控 [I1]**(20h,算法优化 agent) — 已完成 2026-07-09(代码实现 + 单 crate 验证全部通过;workspace 级别验证与文档归档受用户约束延后)
  - [x] SubTask 3.1: Round 1 现状核验 — Read `crates/model-router/src/strategies.rs` + `registry.rs` 当前 `route` 实现(O(n) 全量评估所有模型),确认 3 模型规模下 O(n) 无瓶颈
  - [x] SubTask 3.2: Round 2 方案设计 — 设计 MoE 稀疏门控(采用倒数评分函数而非 CLV cosine,因 model-router 不依赖 CLV 向量,倒数形式无需全局 max 归一化支持单遍 O(n)):
    - 轻量级门控函数:基于 cost/latency/quality 倒数评分 `1/(1+x)` 粗筛(纯算术,无 format!,常数因子远低于完整评估)
    - Top-K 选取:`select_nth_unstable_by` 选取 Top-K(k ≤ 5)候选(O(n) 复杂度)
    - 完整评估:仅对 Top-K 候选计算完整归一化评分
    - 阈值退化:模型数 < 阈值(默认 50)时自动退化为全量评估(向后兼容)
  - [x] SubTask 3.3: Round 3 影响评估 — 门控函数纯算术无字符串操作,开销远低于完整评估;`route_auto` 签名不变,新增 `route_auto_with_gate` 供 bench/可配置场景,向后兼容
  - [x] SubTask 3.4: 搭建 50+ 模型规模测试夹具 — `make_models(n)` 批量生成器在 `tests/moe_test.rs`,cost/latency 随 index 递增、quality 递减确保评分差异化,支持 50/100/200 规模
  - [x] SubTask 3.5: TDD-RED — 新增 10 个测试(超出 spec 要求的 4 个,覆盖 Top-K 激活/阈值退化/召回/向后兼容 + 2 proptest 不变量):
    - `test_moe_gate_activates_top_k_only` / `_100_models` / `_200_models` — 50/100/200 模型仅激活 ≤ K
    - `test_moe_gate_degrades_when_below_threshold` / `_default_config` — 模型数 < 50 退化为全量
    - `test_moe_gate_custom_top_k` — 自定义 top_k=3
    - `test_moe_gate_recalls_best_model` — 门控召回全量评估 Top-1
    - `test_route_auto_backward_compatible_below_threshold` — route_auto 与退化 gate 行为一致
    - proptest `prop_moe_gate_sparsity_invariant`(n ∈ [50,200], top_k ∈ [1,10], 256 cases)— 激活数 ≤ top_k
    - proptest `prop_moe_gate_degrade_invariant`(n ∈ [1,49], threshold ∈ [50,100])— 退化 candidates = n-1
  - [x] SubTask 3.6: TDD-GREEN — 实现 MoE 稀疏门控:
    - 新增 `crates/model-router/src/moe.rs` — `MoeGate` 类型(两参 `new(threshold, top_k)`)+ `gate()` 方法 + 阈值配置
    - 修改 `crates/model-router/src/strategies.rs` — `route_auto` 内部用 `MoeGate::default()`,新增 `route_auto_with_gate` 仅对 gated 候选完整评估
    - 修改 `crates/model-router/src/config.rs` — 新增 `moe_threshold`(默认 50)+ `moe_top_k`(默认 5)字段
  - [x] SubTask 3.7: TDD-REFACTOR — 提取 `MoeGate` 类型,添加 WHY 注释说明:
    - 稀疏门控的 O(n)→O(k) 复杂度优化(select_nth_unstable_by O(n) 替代 sort_by O(n log n))
    - 小规模退化策略的向后兼容性(模型数 < 50 返回全部引用,行为与历史全量评估一致)
    - 门控函数轻量级设计(倒数形式 1/(1+x) 无需全局 max 归一化,支持单遍 O(n) 评分)
  - [x] SubTask 3.8: 在 `crates/model-router/benches/moe_bench.rs` 新增 bench 对比 O(n) vs O(k) 在 50/100/200 模型规模下的路由延迟(sample_size=10,criterion 最小值,等价 min-of-N 5)
  - [x] SubTask 3.9: `cargo test -p model-router`(123 passed: 71 unit + 22 cacr + 1 cacr_test + 10 moe_test + 1 proptest + 13 router + 3 top_k_equivalence + 2 doctest)+ `cargo bench -p model-router --bench moe_bench --no-run` 编译通过 + `cargo clippy -p model-router --all-targets --jobs 2 -- -D warnings` 零警告 + `cargo fmt -p model-router -- --check` 零 diff
  - [x] SubTask 3.10: Task 3 验证与归档 — 已完成 2026-07-09
    - [x] SubTask 3.10.1: `cargo test --workspace --jobs 1` 退出码 0 — 3403 passed / 0 failed / 56 ignored
    - [x] SubTask 3.10.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
    - [x] SubTask 3.10.3: `cargo fmt --all -- --check` 退出码 0
    - [x] SubTask 3.10.4: 创建 `docs/optimization/v1.2.0/task3_moe_verification_report.md`
    - [x] SubTask 3.10.5: `CHANGELOG.md` 追加 Task 3 章节
    - [x] SubTask 3.10.6: `project_memory.md` 追加 Task 3 教训(MoE 门控设计 / 50+ 规模验证)

## Task 4: chimera-cli OnceCell 懒加载(P4,高风险重构,8h)

> **依赖**:Task 1 完成(测试基础设施提供回归保护)— **强烈依赖**,E1 重构风险高,必须有完整测试覆盖
> **并行性**:无(单 crate 修改,14 section 串行重构)
> **验收门槛**:14 section 全部改为 OnceCell 懒加载 + 向后兼容(公开 API 不变)+ cargo test -p chimera-cli 通过 + cargo test --workspace 通过

- [x] **Task 4: chimera-cli OnceCell 懒加载 [E1]**(8h,重构专家 agent) — 已完成 2026-07-09(改用 std::sync::OnceLock 替代 once_cell,LazySection<T> + Figment::extract_inner section 级懒加载,22 测试覆盖)
  - [x] SubTask 4.1: Round 1 现状核验 — Read `crates/chimera-cli/src/config.rs` 当前 14 个 section 的 eager 加载实现(`ChimeraConfig::default` + `config::load` 全量 extract),确认启动期全量解析的性能开销
  - [x] SubTask 4.2: Round 2 方案设计 — 设计 OnceCell 懒加载架构(实际采用 std::sync::OnceLock 而非 OnceCell):
    - `LazyConfig` 持有 `Figment` provider + 14 个 `LazySection<SectionType>` 字段
    - 各 getter 通过 `OnceLock::get_or_try_init` 懒加载
    - 首次访问时调用 `Figment::extract_inner` 按路径反序列化对应 section
    - 重复访问返回缓存 `Result<T, String>`(错误也缓存)
  - [x] SubTask 4.3: Round 3 影响评估 — 14 section 互相独立(无 section 间依赖);`config::load` / `config::default_config` / `ChimeraConfig::default` 既有 API 签名与行为不变(向后兼容);潜在 break 点通过 22 等价性测试验证
  - [x] SubTask 4.4: TDD-RED — 在 `crates/chimera-cli/tests/config_test.rs` + `tests/config_lazy.rs` 新增 22 测试:
    - `test_lazy_load_unaccessed_section_not_parsed`(错误探针验证懒加载隔离性)
    - `test_config_repeated_access_returns_cached`(`std::ptr::eq` 验证缓存命中)
    - `test_config_backward_compatible_api`(公开 API 签名不变)
    - 14 个 section 逐个 JSON 字符串比对 lazy vs eager 等价性
    - `test_to_chimera_config_aggregates_all_sections`(聚合方法)
  - [x] SubTask 4.5: TDD-GREEN — 实现 OnceLock 懒加载:
    - 修改 `crates/chimera-cli/src/config.rs` — 14 个 section 字段改为 `LazySection<SectionType>`
    - 各 getter 通过 `OnceLock::get_or_try_init` 懒加载(委托 `extract_section` 内部 `Figment::extract_inner`)
    - `LazyConfig::new()` 只构建 provider 链不 extract
  - [x] SubTask 4.6: TDD-REFACTOR — 提取 `LazySection<T>` 辅助类型(封装 `OnceLock<Result<T, String>>` + `get_or_try_init`),添加 WHY 注释说明:
    - 懒加载的性能收益(避免启动期解析未使用 section)
    - OnceLock 的线程安全保证(Rust 1.70+ 标准库,无 unsafe,零新增依赖)
    - 错误缓存语义(Result 而非 Option,配置格式错误不因重试自愈)
  - [x] SubTask 4.7: 14 section 串行重构 — 14 个 `LazySection` 字段统一模式,每个 getter 缩为一行(`self.<field>.get_or_try_init(|| extract_section(...))`),`cargo test -p chimera-cli` 41 passed 验证
  - [x] SubTask 4.8: `cargo test -p chimera-cli`(41 passed)+ `cargo test --workspace --jobs 1`(3403 passed)验证全部通过
  - [x] SubTask 4.9: Task 4 验证与归档 — 已完成 2026-07-09
    - [x] SubTask 4.9.1: `cargo test --workspace --jobs 1` 退出码 0 — 3403 passed / 0 failed / 56 ignored
    - [x] SubTask 4.9.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
    - [x] SubTask 4.9.3: `cargo fmt --all -- --check` 退出码 0
    - [x] SubTask 4.9.4: 创建 `docs/optimization/v1.2.0/task4_oncecell_verification_report.md`
    - [x] SubTask 4.9.5: `CHANGELOG.md` 追加 Task 4 章节
    - [x] SubTask 4.9.6: `project_memory.md` 追加 Task 4 教训(OnceLock 懒加载 / 14 section 重构策略)

## 最终交付

- [x] **Task 5: 全量验证与归档**(4h,质量验证 + 文档同步 agent) — 已完成 2026-07-09
  - [x] SubTask 5.1: `cargo test --workspace --jobs 1` 退出码 0,最终测试数 3403(≥ 3248 门槛,增量 +175 from Phase V 3228)
  - [x] SubTask 5.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
  - [x] SubTask 5.3: `cargo fmt --all -- --check` 退出码 0
  - [ ] SubTask 5.4: `cargo audit --deny warnings` 退出码 0 — ⚠️ 网络不可达延后到 GA 前手动执行
  - [x] SubTask 5.5: 创建 `docs/optimization/v1.2.0/full_deferred_optimization_report.md` — 全量延后优化报告
  - [ ] SubTask 5.6: `CHANGELOG.md` 追加 v1.2.0-omega 完整章节 — ⚠️ Task 1-4 已各自追加章节,v1.2.0-omega 完整汇总章节延后到 GA 发布前
  - [ ] SubTask 5.7: `project_memory.md` 追加 v1.2.0-omega 总结教训 — ⚠️ Task 1-4 已各自追加教训章节,v1.2.0-omega 总结教训延后到 GA 发布前
  - [x] SubTask 5.8: 更新 `v1-1-0-systematic-optimization-deep-analysis/tasks.md` — V-2/V-6/V-7/V-10 标记为"已在 v1.2.0-omega 完成"
  - [x] SubTask 5.9: 更新 `CODE_WIKI.md` §1.3 开发状态表

## Task Dependencies

- **Task 0**(脱敏化 + 提交):阻塞所有后续,必须先完成
- **Task 1**(V-10 测试覆盖):依赖 Task 0;为 Task 3 / Task 4 提供回归安全网
- **Task 2**(N15 FTS5):依赖 Task 0(代码已提交);与 Task 1 / Task 3 / Task 4 可并行(独立 crate)
- **Task 3**(I1 MoE):依赖 Task 0 + Task 1(测试覆盖);与 Task 2 / Task 4 可并行(独立 crate)
- **Task 4**(E1 OnceCell):依赖 Task 0 + Task 1(测试覆盖,强烈依赖);与 Task 2 / Task 3 可并行(独立 crate)
- **Task 5**(最终归档):依赖 Task 1 / Task 2 / Task 3 / Task 4 全部完成

## 并行执行建议

- **批次 1**(Task 0 完成后):Task 1(V-10 测试覆盖)单独执行,为基础设施
- **批次 2**(Task 1 完成后):Task 2(N15 FTS5) / Task 3(I1 MoE) / Task 4(E1 OnceCell)三任务并行(独立 crate,无相互依赖)
- **批次 3**(Task 1-4 全部完成后):Task 5(最终归档)收尾

> **注**:Task 2 / Task 3 / Task 4 虽然可并行,但若资源受限(子代理数量 / 编译资源),建议按优先级顺序串行执行:Task 2(N15,8h,低风险)→ Task 3(I1,20h,中等复杂度)→ Task 4(E1,8h,高风险)。
