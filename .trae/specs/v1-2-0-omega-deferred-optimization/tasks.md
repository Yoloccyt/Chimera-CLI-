# Tasks — v1.2.0-omega 延后优化任务

> 任务按优先级顺序编排:前置脱敏 → 测试基础设施 → 独立功能 → 算法验证 → 高风险重构。
> 每个任务完成后必须通过 checklist.md 全部检查项才能进入下一任务。
> **执行规范**:每个 Task 都遵循 TDD(RED-GREEN-REFACTOR),先写失败测试再实现;子代理产物必须 `cargo fmt --all` + `cargo clippy --jobs 2 -- -D warnings` 通过;每个 Task 完成后勾选 `[x]`。

## 前置任务:脱敏化处理与安全提交(P0,阻塞所有后续)

- [ ] **Task 0: 脱敏化扫描 + 安全提交**(1h,安全脱敏 agent)
  - [ ] SubTask 0.1: 识别 Phase V commit 7024b03 涉及的 26 个修改文件(19 修改 + 7 新增)
  - [ ] SubTask 0.2: Grep 敏感模式扫描 — 对 26 个文件执行 grep:`api_key|secret|password|token|private_key|credential|bearer|auth_token|access_key|AWS_|GITHUB_TOKEN`(case-insensitive)
  - [ ] SubTask 0.3: 人工核验命中项 — 区分真实敏感信息与测试占位符(如 `test_key` / `dummy_secret`);真实敏感信息需脱敏(替换为占位符或环境变量读取)
  - [ ] SubTask 0.4: 扫描个人路径信息 — grep `C:\\Users\\|/home/|/Users/` 等用户路径,确认无个人路径泄露到源码
  - [ ] SubTask 0.5: 确认 `.gitignore` 覆盖 `.env*` / `*.pem` / `credentials*` / `.toolchain/` / `target/`
  - [ ] SubTask 0.6: 确认 `git status` 无 `.env` / 凭据文件被暂存
  - [ ] SubTask 0.7: 逐文件 `git add`(严禁 `git add -A` / `git add .`),26 个文件逐个 add
  - [ ] SubTask 0.8: 设置 git identity env vars — `$env:GIT_AUTHOR_NAME='Aether CLI Team'` + `$env:GIT_AUTHOR_EMAIL='team@aether.dev'` + `$env:GIT_COMMITTER_NAME='Aether CLI Team'` + `$env:GIT_COMMITTER_EMAIL='team@aether.dev'`(匹配之前提交身份,规则禁止修改 git config)
  - [ ] SubTask 0.9: 使用 PowerShell here-string `@'...'@` 传入 commit message(`git commit -m @'...'@`),message 描述 Phase V 归档 + 脱敏化处理说明
  - [ ] SubTask 0.10: `git push origin master`(若网络不可达,记录状态等待用户手动重试,不阻塞后续本地任务)
  - [ ] SubTask 0.11: 记录脱敏化处理结果到 `docs/optimization/v1.2.0/task0_desensitization_report.md`

## Task 1: 测试覆盖补齐(P1,基础设施,12h)

> **依赖**:Task 0 完成(代码已提交)
> **并行性**:SubTask 1.1 / 1.2 / 1.3 / 1.4 可并行(独立 crate / 独立测试类型)
> **验收门槛**:5 crate benches 编译通过 + 5 crate proptest 通过 + 23 crate doctest 通过 + fuzz 6 target 静态验证通过 + cargo test --workspace 退出码 0

- [ ] **Task 1: 测试覆盖补齐 [V-10]**(12h,测试基础设施 agent)
  - [ ] SubTask 1.1: 5 crate 补齐 criterion benches(并行)
    - [ ] SubTask 1.1.1: Round 1 现状核验 — Grep `crates/*/Cargo.toml` 缺 `[[bench]]` 声明的 crate,识别 5 个最需要 bench 的 crate(优先选择热路径 crate:osa-coordinator / kvbsr-router / faae-router / sesa-router / scc-cache)
    - [ ] SubTask 1.1.2: Round 2 方案设计 — 每个 crate 设计 2 个 bench(延迟 bench + 吞吐量 bench),参考已有 bench 风格(`repo-wiki/benches/vector_bench.rs` / `model-router/benches/registry_bench.rs`)
    - [ ] SubTask 1.1.3: TDD-RED — 创建 `crates/<crate>/benches/<name>_bench.rs`,声明 `[[bench]]` + `harness = false` + dev-dep `criterion`
    - [ ] SubTask 1.1.4: TDD-GREEN — 实现 bench 函数,`cargo bench -p <crate> --bench <name> --no-run` 编译通过
    - [ ] SubTask 1.1.5: TDD-REFACTOR — 添加 WHY 注释说明 bench 设计意图(min-of-N 5 次采样减少调度噪声)
    - [ ] SubTask 1.1.6: `cargo bench -p <crate> --bench <name> --no-run` 验证 5 crate 全部编译通过
  - [ ] SubTask 1.2: 5 crate 补齐 proptest(并行)
    - [ ] SubTask 1.2.1: Round 1 现状核验 — Grep `crates/*/tests/proptest.rs` 缺失的 crate,识别 5 个最需要 proptest 的 crate(优先选择有核心不变量的 crate:osa-coordinator / kvbsr-router / gea-activator / cmt-tiering / lsct-tiering)
    - [ ] SubTask 1.2.2: Round 2 方案设计 — 每个 crate 识别 1-2 个核心不变量(如稀疏度不变量 / Top-K 不变量 / 状态机不变量)
    - [ ] SubTask 1.2.3: TDD-RED — 创建 `crates/<crate>/tests/proptest.rs`,使用 proptest 1.11+ block-named 语法 `fn test_name(x in 0..100u32) { ... }`
    - [ ] SubTask 1.2.4: TDD-GREEN — 实现 proptest,`cargo test -p <crate> --test proptest` 通过
    - [ ] SubTask 1.2.5: TDD-REFACTOR — 添加 WHY 注释说明不变量选择的理由
    - [ ] SubTask 1.2.6: `cargo test -p <crate> --test proptest` 验证 5 crate 全部通过
  - [ ] SubTask 1.3: 23 crate 补齐 doctest(并行)
    - [ ] SubTask 1.3.1: Round 1 现状核验 — 对每个 crate 运行 `cargo test --doc -p <crate> --no-run`,识别 doctest 编译失败或无 doctest 的 crate
    - [ ] SubTask 1.3.2: Round 2 方案设计 — 为每个 crate 的 `src/lib.rs` 公开 API(pub fn / pub struct / pub enum)补 `///` 文档注释,含 `# Examples` 可运行示例
    - [ ] SubTask 1.3.3: TDD-RED — 补齐 doctest,`cargo test --doc -p <crate>` 失败的应转为通过
    - [ ] SubTask 1.3.4: TDD-GREEN — 确保 doctest 可运行且通过
    - [ ] SubTask 1.3.5: TDD-REFACTOR — doctest 示例应简洁有效,避免过度冗长
    - [ ] SubTask 1.3.6: `cargo test --doc --workspace` 验证 23 crate 全部通过
  - [ ] SubTask 1.4: fuzz 3→6 target 扩展(并行)
    - [ ] SubTask 1.4.1: Round 1 现状核验 — Read `fuzz/fuzz_targets/` 当前 3 个 target(seccore_sandbox / quest_parse / event_serialize),Read `fuzz/Cargo.toml` 当前 `[[bin]]` 声明
    - [ ] SubTask 1.4.2: Round 2 方案设计 — 识别 3 个新增 fuzz target 候选(优先选择 parser / serializer / 边界输入路径,如 cacr_budget_parse / checkpoint_deserialize / moe_gate_compute)
    - [ ] SubTask 1.4.3: TDD-RED — 创建 `fuzz/fuzz_targets/<name>.rs`,在 `fuzz/Cargo.toml` 声明对应 `[[bin]]`
    - [ ] SubTask 1.4.4: TDD-GREEN — 实现 fuzz target,`cargo check --manifest-path fuzz/Cargo.toml` 静态验证通过(Windows 无法实际执行,委托 Linux CI)
    - [ ] SubTask 1.4.5: TDD-REFACTOR — 添加 WHY 注释说明 fuzz target 选择的理由
    - [ ] SubTask 1.4.6: 验证 `fuzz/Cargo.toml` 含 6 个 `[[bin]]` + `[package.metadata] cargo-fuzz = true` + 空 `[workspace]` 表
  - [ ] SubTask 1.5: Task 1 验证与归档
    - [ ] SubTask 1.5.1: `cargo test --workspace --jobs 1` 退出码 0,测试增量 ≥ 20(5 bench 编译 + 5 proptest + 23 doctest + 3 fuzz check)
    - [ ] SubTask 1.5.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
    - [ ] SubTask 1.5.3: `cargo fmt --all -- --check` 退出码 0
    - [ ] SubTask 1.5.4: 创建 `docs/optimization/v1.2.0/task1_test_coverage_report.md`
    - [ ] SubTask 1.5.5: `CHANGELOG.md` 追加 Task 1 章节
    - [ ] SubTask 1.5.6: `project_memory.md` 追加 Task 1 教训

## Task 2: repo-wiki FTS5 全文索引(P2,独立功能增强,8h)

> **依赖**:Task 1 完成(测试基础设施提供回归保护)
> **并行性**:无(单 crate 修改)
> **验收门槛**:FTS5 可用时走 MATCH 查询 + 不可用时降级到 LIKE + bench 对比 MATCH vs LIKE 性能 + cargo test -p repo-wiki 通过

- [ ] **Task 2: repo-wiki FTS5 全文索引 [N15]**(8h,存储优化 agent)
  - [ ] SubTask 2.1: Round 1 现状核验 — Read `crates/repo-wiki/src/store.rs` 当前 `search` 实现(使用 `LIKE '%query%'`),确认 LIKE 全表扫描在大规模文档库的性能瓶颈
  - [ ] SubTask 2.2: Round 2 方案设计 — 设计 FTS5 虚拟表 schema + `index_document` 同步写入 + `search` 优先 FTS5 MATCH + 降级策略;设计 Windows GNU 编译配置(`rusqlite` `bundled` feature + `SQLITE_ENABLE_FTS5` 编译开关)
  - [ ] SubTask 2.3: Round 3 影响评估 — 评估 `rusqlite` `bundled` feature 对编译时间的影响;评估 `repo-wiki` 公开 API 兼容性
  - [ ] SubTask 2.4: TDD-RED — 在 `crates/repo-wiki/tests/fts_test.rs` 新增测试:
    - `test_fts5_search_returns_relevant_docs` — FTS5 MATCH 查询返回相关文档
    - `test_fts5_fallback_to_like_when_unavailable` — FTS5 不可用时降级到 LIKE
    - `test_fts5_index_document_synced` — index_document 时 FTS5 索引同步写入
  - [ ] SubTask 2.5: TDD-GREEN — 实现 FTS5 模块:
    - 新增 `crates/repo-wiki/src/fts.rs` — FTS5 虚拟表创建 + 索引写入 + MATCH 查询
    - 修改 `crates/repo-wiki/src/store.rs` — `search` 优先走 FTS5,失败降级到 LIKE
    - 修改 `crates/repo-wiki/Cargo.toml` — 启用 `rusqlite` `bundled` feature + FTS5 编译开关
  - [ ] SubTask 2.6: TDD-REFACTOR — 提取 `FtsCapability` 枚举(`Available` / `Unavailable`)记录 FTS5 可用性,添加 WHY 注释说明降级策略
  - [ ] SubTask 2.7: 在 `crates/repo-wiki/benches/fts_bench.rs` 新增 bench 对比 FTS5 MATCH vs LIKE 在 1000+ 文档规模下的性能差异(min-of-N 5 次采样)
  - [ ] SubTask 2.8: 验证 Windows GNU 编译 — `cargo build -p repo-wiki` 在 Windows GNU 工具链编译通过
  - [ ] SubTask 2.9: `cargo test -p repo-wiki` + `cargo bench -p repo-wiki --bench fts_bench --no-run` 验证通过
  - [ ] SubTask 2.10: Task 2 验证与归档
    - [ ] SubTask 2.10.1: `cargo test --workspace --jobs 1` 退出码 0
    - [ ] SubTask 2.10.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
    - [ ] SubTask 2.10.3: `cargo fmt --all -- --check` 退出码 0
    - [ ] SubTask 2.10.4: 创建 `docs/optimization/v1.2.0/task2_fts5_verification_report.md`
    - [ ] SubTask 2.10.5: `CHANGELOG.md` 追加 Task 2 章节
    - [ ] SubTask 2.10.6: `project_memory.md` 追加 Task 2 教训(FTS5 编译配置 / 降级策略)

## Task 3: model-router MoE 稀疏门控(P3,算法规模验证,20h)

> **依赖**:Task 1 完成(测试基础设施提供回归保护)+ Task 2 完成可选(独立)
> **并行性**:无(单 crate 修改)
> **验收门槛**:50+ 模型规模下 p95 路由延迟降低 ≥ 40% + 小规模自动退化 + proptest 验证稀疏性不变量 + cargo test -p model-router 通过

- [ ] **Task 3: model-router MoE 稀疏门控 [I1]**(20h,算法优化 agent)
  - [ ] SubTask 3.1: Round 1 现状核验 — Read `crates/model-router/src/strategies.rs` + `registry.rs` 当前 `route` 实现(O(n) 全量评估所有模型),确认 3 模型规模下 O(n) 无瓶颈
  - [ ] SubTask 3.2: Round 2 方案设计 — 设计 MoE 稀疏门控:
    - 轻量级门控函数:基于 CLV cosine similarity 的粗筛(仅计算向量相似度,不计算完整 CACR 成本)
    - Top-K 选取:`select_nth_unstable_by` 选取 Top-K(k ≤ 5)候选
    - 完整评估:仅对 Top-K 候选计算完整 CACR 成本/延迟评估
    - 阈值退化:模型数 < 阈值(默认 50)时自动退化为全量评估
  - [ ] SubTask 3.3: Round 3 影响评估 — 评估门控函数的计算开销(确保轻量级门控本身不成为瓶颈);评估 `route` 公开 API 兼容性
  - [ ] SubTask 3.4: 搭建 50+ 模型规模测试夹具 — 在 `crates/model-router/tests/moe_test.rs` 创建 mock ModelInfo 批量生成器,生成 50 / 100 / 200 模型规模的测试数据
  - [ ] SubTask 3.5: TDD-RED — 新增测试:
    - `test_moe_gate_activates_top_k_only` — 50+ 模型时仅激活 k 个(K ≤ 5)
    - `test_moe_gate_degrades_when_below_threshold` — 模型数 < 50 时退化为全量评估
    - `test_moe_gate_latency_improvement` — 50+ 模型时 p95 延迟降低 ≥ 40%(bench 验证)
    - proptest `prop_moe_gate_sparsity_invariant` — 门控函数始终仅激活 k 个(proptest 256 cases)
  - [ ] SubTask 3.6: TDD-GREEN — 实现 MoE 稀疏门控:
    - 新增 `crates/model-router/src/moe.rs` — `MoeGate` 类型 + `gate()` 方法 + 阈值配置
    - 修改 `crates/model-router/src/strategies.rs` — `route` 集成稀疏门控
    - 修改 `crates/model-router/src/config.rs` — 新增 `moe_threshold`(默认 50)+ `moe_top_k`(默认 5)配置
  - [ ] SubTask 3.7: TDD-REFACTOR — 提取 `MoeGate` 类型,添加 WHY 注释说明:
    - 稀疏门控的 O(n)→O(k) 复杂度优化
    - 小规模退化策略的向后兼容性
    - 门控函数轻量级设计(避免门控本身成为瓶颈)
  - [ ] SubTask 3.8: 在 `crates/model-router/benches/moe_bench.rs` 新增 bench 对比 O(n) vs O(k) 在 50/100/200 模型规模下的路由延迟(min-of-N 5 次采样)
  - [ ] SubTask 3.9: `cargo test -p model-router` + `cargo bench -p model-router --bench moe_bench --no-run` 验证通过
  - [ ] SubTask 3.10: Task 3 验证与归档
    - [ ] SubTask 3.10.1: `cargo test --workspace --jobs 1` 退出码 0
    - [ ] SubTask 3.10.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
    - [ ] SubTask 3.10.3: `cargo fmt --all -- --check` 退出码 0
    - [ ] SubTask 3.10.4: 创建 `docs/optimization/v1.2.0/task3_moe_verification_report.md`
    - [ ] SubTask 3.10.5: `CHANGELOG.md` 追加 Task 3 章节
    - [ ] SubTask 3.10.6: `project_memory.md` 追加 Task 3 教训(MoE 门控设计 / 50+ 规模验证)

## Task 4: chimera-cli OnceCell 懒加载(P4,高风险重构,8h)

> **依赖**:Task 1 完成(测试基础设施提供回归保护)— **强烈依赖**,E1 重构风险高,必须有完整测试覆盖
> **并行性**:无(单 crate 修改,14 section 串行重构)
> **验收门槛**:14 section 全部改为 OnceCell 懒加载 + 向后兼容(公开 API 不变)+ cargo test -p chimera-cli 通过 + cargo test --workspace 通过

- [ ] **Task 4: chimera-cli OnceCell 懒加载 [E1]**(8h,重构专家 agent)
  - [ ] SubTask 4.1: Round 1 现状核验 — Read `crates/chimera-cli/src/config.rs` 当前 14 个 section 的 eager 加载实现,确认启动期全量解析的性能开销
  - [ ] SubTask 4.2: Round 2 方案设计 — 设计 OnceCell 懒加载架构:
    - `Config` 持有 14 个 `OnceCell<SectionType>` 字段
    - `get::<T>()` 通过 `OnceCell::get_or_init` 懒加载
    - 首次访问时调用对应 section 的解析函数
    - 重复访问返回缓存值
  - [ ] SubTask 4.3: Round 3 影响评估 — 评估 14 section 的依赖关系(某些 section 可能依赖其他 section);评估 `get()` API 兼容性(签名不变,行为从同步立即解析改为懒加载);识别潜在 break 点
  - [ ] SubTask 4.4: TDD-RED — 在 `crates/chimera-cli/tests/config_test.rs` 新增测试:
    - `test_config_lazy_load_only_when_accessed` — 未访问的 section 不被解析(通过 side effect 检测)
    - `test_config_repeated_access_returns_cached` — 重复访问返回缓存值
    - `test_config_backward_compatible_api` — 公开 API 签名不变,既有调用方零修改
    - 14 个 section 各 1 个懒加载测试
  - [ ] SubTask 4.5: TDD-GREEN — 实现 OnceCell 懒加载:
    - 修改 `crates/chimera-cli/src/config.rs` — 14 个 section 字段改为 `OnceCell<SectionType>`
    - 修改 `get::<T>()` — 通过 `OnceCell::get_or_init` 懒加载
    - 保持 `Config::new()` 只解析必需 section(如 `core` / `logging`)
  - [ ] SubTask 4.6: TDD-REFACTOR — 提取 `LazySection<T>` 辅助类型(封装 OnceCell + 解析函数),添加 WHY 注释说明:
    - 懒加载的性能收益(避免启动期解析未使用 section)
    - OnceCell 的线程安全保证(无 unsafe)
    - 14 section 独立提交的安全策略
  - [ ] SubTask 4.7: 14 section 串行重构 — 每个 section 独立修改 + `cargo test -p chimera-cli` 验证,便于回滚
  - [ ] SubTask 4.8: `cargo test -p chimera-cli` + `cargo test --workspace --jobs 1` 验证全部通过
  - [ ] SubTask 4.9: Task 4 验证与归档
    - [ ] SubTask 4.9.1: `cargo test --workspace --jobs 1` 退出码 0
    - [ ] SubTask 4.9.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
    - [ ] SubTask 4.9.3: `cargo fmt --all -- --check` 退出码 0
    - [ ] SubTask 4.9.4: 创建 `docs/optimization/v1.2.0/task4_oncecell_verification_report.md`
    - [ ] SubTask 4.9.5: `CHANGELOG.md` 追加 Task 4 章节
    - [ ] SubTask 4.9.6: `project_memory.md` 追加 Task 4 教训(OnceCell 懒加载 / 14 section 重构策略)

## 最终交付

- [ ] **Task 5: 全量验证与归档**(4h,质量验证 + 文档同步 agent)
  - [ ] SubTask 5.1: `cargo test --workspace --jobs 1` 退出码 0,最终测试数 ≥ 3248(3228 基线 + 20+ 新增)
  - [ ] SubTask 5.2: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
  - [ ] SubTask 5.3: `cargo fmt --all -- --check` 退出码 0
  - [ ] SubTask 5.4: `cargo audit --deny warnings` 退出码 0(网络可用时)
  - [ ] SubTask 5.5: 创建 `docs/optimization/v1.2.0/full_deferred_optimization_report.md` — 全量延后优化报告
  - [ ] SubTask 5.6: `CHANGELOG.md` 追加 v1.2.0-omega 完整章节
  - [ ] SubTask 5.7: `project_memory.md` 追加 v1.2.0-omega 总结教训
  - [ ] SubTask 5.8: 更新 `v1-1-0-systematic-optimization-deep-analysis/tasks.md` — V-2/V-6/V-7/V-10 标记为"已在 v1.2.0-omega 完成"
  - [ ] SubTask 5.9: 更新 `CODE_WIKI.md` §1.3 开发状态表

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
