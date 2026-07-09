# Checklist — v1.2.0-omega 延后优化任务

> 每个检查项对应 spec.md 的 Requirement 与 tasks.md 的 Task。
> 所有检查项必须勾选 `[x]` 才能声明任务完成。

## Task 0:脱敏化处理与安全提交

- [ ] 26 个修改文件全部经过敏感模式 grep 扫描(api_key / secret / password / token / private_key / credential / bearer / auth_token / access_key / AWS_ / GITHUB_TOKEN)
- [ ] 所有 grep 命中项经人工核验,真实敏感信息已脱敏(替换为占位符或环境变量读取)
- [ ] 个人路径信息(C:\Users\ / /home/ / /Users/)扫描完成,无个人路径泄露到源码
- [ ] `.gitignore` 确认覆盖 `.env*` / `*.pem` / `credentials*` / `.toolchain/` / `target/`
- [ ] `git status` 确认无 `.env` / 凭据文件被暂存
- [ ] 26 个文件逐个 `git add`(未使用 `git add -A` / `git add .`)
- [ ] git identity env vars 已设置(GIT_AUTHOR_NAME / GIT_AUTHOR_EMAIL / GIT_COMMITTER_NAME / GIT_COMMITTER_EMAIL)
- [ ] commit message 使用 PowerShell here-string `@'...'@` 传入,含脱敏化处理说明
- [ ] `git commit` 成功(commit hash 记录)
- [ ] `git push origin master` 已执行(若网络不可达,状态已记录等待用户手动重试)
- [ ] `docs/optimization/v1.2.0/task0_desensitization_report.md` 已创建

## Task 1:测试覆盖补齐(V-10)

### SubTask 1.1:benches 补齐

- [ ] 5 个缺 benches 的 crate 已识别(grep `crates/*/Cargo.toml` 缺 `[[bench]]`)
- [ ] 每个 crate 新增 `benches/<name>_bench.rs`,含至少 2 个 criterion bench(延迟 + 吞吐量)
- [ ] 每个 crate 的 `Cargo.toml` 声明 `[[bench]]` + `harness = false` + dev-dep `criterion`
- [ ] `cargo bench -p <crate> --bench <name> --no-run` 5 crate 全部编译通过
- [ ] bench 设计遵循 min-of-N 5 次采样减少调度噪声

### SubTask 1.2:proptest 补齐

- [ ] 5 个缺 proptest 的 crate 已识别
- [ ] 每个 crate 新增 `tests/proptest.rs`,含至少 1 个 proptest 验证核心不变量
- [ ] proptest 使用 1.11+ block-named 语法 `fn test_name(x in 0..100u32) { ... }`
- [ ] `cargo test -p <crate> --test proptest` 5 crate 全部通过
- [ ] proptest 不变量选择有 WHY 注释说明理由

### SubTask 1.3:doctest 补齐

- [ ] 23 个 crate 的公开 API(pub fn / pub struct / pub enum)已补 `///` 文档注释
- [ ] doctest 含 `# Examples` 可运行示例
- [ ] `cargo test --doc --workspace` 23 crate 全部通过
- [ ] doctest 简洁有效,无过度冗长

### SubTask 1.4:fuzz 扩展

- [ ] 3 个新增 fuzz target 已创建(`fuzz/fuzz_targets/*.rs`)
- [ ] `fuzz/Cargo.toml` 含 6 个 `[[bin]]` 声明
- [ ] `[package.metadata] cargo-fuzz = true` 存在
- [ ] 空 `[workspace]` 表存在(fuzz 独立 workspace)
- [ ] `cargo check --manifest-path fuzz/Cargo.toml` 静态验证通过
- [ ] fuzz target 选择有 WHY 注释说明理由

### Task 1 整体验证

- [ ] `cargo test --workspace --jobs 1` 退出码 0,测试增量 ≥ 20
- [ ] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
- [ ] `cargo fmt --all -- --check` 退出码 0
- [ ] `docs/optimization/v1.2.0/task1_test_coverage_report.md` 已创建
- [ ] `CHANGELOG.md` 追加 Task 1 章节
- [ ] `project_memory.md` 追加 Task 1 教训

## Task 2:repo-wiki FTS5 全文索引(N15)

- [ ] `crates/repo-wiki/src/store.rs` 当前 LIKE 全表扫描实现已核验
- [ ] FTS5 虚拟表 schema 已设计(`CREATE VIRTUAL TABLE docs_fts USING fts5(...)`)
- [ ] `index_document` 同步写入 FTS5 索引已实现
- [ ] `search` 优先走 FTS5 `MATCH` 查询已实现
- [ ] FTS5 不可用时降级到 `LIKE` 已实现(运行时检测)
- [ ] 降级时记录 warning 日志提示"FTS5 不可用,回退到 LIKE"
- [ ] Windows GNU 编译配置已解决(`rusqlite` `bundled` feature + `SQLITE_ENABLE_FTS5`)
- [ ] `cargo build -p repo-wiki` Windows GNU 编译通过
- [ ] 3 个 TDD 测试已新增(test_fts5_search_returns_relevant_docs / test_fts5_fallback_to_like_when_unavailable / test_fts5_index_document_synced)
- [ ] `FtsCapability` 枚举(Available / Unavailable)已提取
- [ ] WHY 注释说明降级策略已添加
- [ ] `crates/repo-wiki/benches/fts_bench.rs` 对比 FTS5 MATCH vs LIKE 在 1000+ 文档规模的性能
- [ ] `cargo test -p repo-wiki` 通过
- [ ] `cargo bench -p repo-wiki --bench fts_bench --no-run` 编译通过
- [ ] `cargo test --workspace --jobs 1` 退出码 0
- [ ] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
- [ ] `cargo fmt --all -- --check` 退出码 0
- [ ] `docs/optimization/v1.2.0/task2_fts5_verification_report.md` 已创建
- [ ] `CHANGELOG.md` 追加 Task 2 章节
- [ ] `project_memory.md` 追加 Task 2 教训(FTS5 编译配置 / 降级策略)

## Task 3:model-router MoE 稀疏门控(I1)

- [ ] `crates/model-router/src/strategies.rs` + `registry.rs` 当前 O(n) 全量评估已核验
- [ ] MoE 稀疏门控方案已设计(轻量级门控函数 + Top-K 选取 + 完整评估 + 阈值退化)
- [ ] 门控函数基于 CLV cosine similarity 粗筛(轻量级,不计算完整 CACR)
- [ ] Top-K 选取使用 `select_nth_unstable_by`(O(n) 复杂度)
- [ ] 阈值退化:模型数 < 50 时自动退化为全量评估
- [ ] 50+ 模型规模测试夹具已搭建(mock ModelInfo 批量生成器)
- [ ] 4 个 TDD 测试已新增(test_moe_gate_activates_top_k_only / test_moe_gate_degrades_when_below_threshold / test_moe_gate_latency_improvement / prop_moe_gate_sparsity_invariant)
- [ ] proptest 256 cases 验证稀疏性不变量通过
- [ ] `MoeGate` 类型已实现(`crates/model-router/src/moe.rs`)
- [ ] `route` 已集成稀疏门控
- [ ] 配置项 `moe_threshold`(默认 50)+ `moe_top_k`(默认 5)已添加
- [ ] WHY 注释说明 O(n)→O(k) 优化 + 小规模退化 + 门控轻量级设计已添加
- [ ] `crates/model-router/benches/moe_bench.rs` 对比 O(n) vs O(k) 在 50/100/200 模型规模的延迟
- [ ] bench 验证 p95 延迟降低 ≥ 40%
- [ ] `cargo test -p model-router` 通过
- [ ] `cargo bench -p model-router --bench moe_bench --no-run` 编译通过
- [ ] `cargo test --workspace --jobs 1` 退出码 0
- [ ] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
- [ ] `cargo fmt --all -- --check` 退出码 0
- [ ] `docs/optimization/v1.2.0/task3_moe_verification_report.md` 已创建
- [ ] `CHANGELOG.md` 追加 Task 3 章节
- [ ] `project_memory.md` 追加 Task 3 教训(MoE 门控设计 / 50+ 规模验证)

## Task 4:chimera-cli OnceCell 懒加载(E1)

- [ ] `crates/chimera-cli/src/config.rs` 当前 14 个 section eager 加载已核验
- [ ] OnceCell 懒加载架构已设计(`OnceCell<SectionType>` + `get_or_init`)
- [ ] 14 section 的依赖关系已评估
- [ ] `get()` API 兼容性已评估(签名不变)
- [ ] 4 个核心 TDD 测试已新增(test_config_lazy_load_only_when_accessed / test_config_repeated_access_returns_cached / test_config_backward_compatible_api + 14 section 各 1 个)
- [ ] 14 个 section 字段已改为 `OnceCell<SectionType>`
- [ ] `get::<T>()` 已改为通过 `OnceCell::get_or_init` 懒加载
- [ ] `Config::new()` 仅解析必需 section(core / logging)
- [ ] `LazySection<T>` 辅助类型已提取
- [ ] WHY 注释说明懒加载性能收益 + OnceCell 线程安全 + 14 section 独立提交策略已添加
- [ ] 14 section 串行重构完成,每步 `cargo test -p chimera-cli` 验证
- [ ] `cargo test -p chimera-cli` 通过
- [ ] `cargo test --workspace --jobs 1` 退出码 0
- [ ] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
- [ ] `cargo fmt --all -- --check` 退出码 0
- [ ] `docs/optimization/v1.2.0/task4_oncecell_verification_report.md` 已创建
- [ ] `CHANGELOG.md` 追加 Task 4 章节
- [ ] `project_memory.md` 追加 Task 4 教训(OnceCell 懒加载 / 14 section 重构策略)

## 最终交付(Task 5)

- [ ] `cargo test --workspace --jobs 1` 退出码 0,最终测试数 ≥ 3248
- [ ] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
- [ ] `cargo fmt --all -- --check` 退出码 0
- [ ] `cargo audit --deny warnings` 退出码 0(网络可用时)
- [ ] `docs/optimization/v1.2.0/full_deferred_optimization_report.md` 已创建
- [ ] `CHANGELOG.md` 追加 v1.2.0-omega 完整章节
- [ ] `project_memory.md` 追加 v1.2.0-omega 总结教训
- [ ] `v1-1-0-systematic-optimization-deep-analysis/tasks.md` 中 V-2/V-6/V-7/V-10 标记为"已在 v1.2.0-omega 完成"
- [ ] `CODE_WIKI.md` §1.3 开发状态表已更新

## 跨任务通用检查

- [ ] 所有变更遵守 §2.2 依赖铁律(L(N)→L(N-1) 允许,L(N)→L(N+1) 禁止)
- [ ] 所有变更遵守 OMEGA 四定律(Ω-Sparse / Ω-Compress / Ω-Evolve / Ω-Event)
- [ ] 所有 crate 保持 `#![forbid(unsafe_code)]`
- [ ] 所有 async fn 满足 `Send + 'static` 约束
- [ ] 所有变更遵循 TDD(RED-GREEN-REFACTOR),先写失败测试再实现
- [ ] 不删除已有测试,只允许增强
- [ ] 所有变更遵循 §3.3.1.5 向后兼容(SemVer,破坏性变更需 major 版本升级)
- [ ] 不变更核心领域类型(UserIntent / Quest / Checkpoint / OmniSparseMasks / CLV / NexusState)
- [ ] 不新建 crate(严格遵守 §3.3.1.6 新 crate 准入)
- [ ] 单函数 ≤ 200 行
- [ ] 所有关键决策有 WHY 注释(隐藏约束 / 变通方案 / 反直觉行为)
