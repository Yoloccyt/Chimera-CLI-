# v1.2.0-omega 延后优化任务实施 Spec

> **Spec ID**: `v1-2-0-omega-deferred-optimization`
> **阶段**: v1.2.0-omega 第二阶段开发(GA 后持续演进)
> **前置 Spec**: `v1-1-0-systematic-optimization-deep-analysis`(Phase I-V 已完成,本 Spec 承接其中 4 项延后任务)
> **创建时间**: 2026-07-09
> **基线状态**: v1.1.0-omega Phase V 收尾完成,commit 7024b03 本地落地,3228 测试通过 / 0 失败 / 55 ignored,clippy 零警告,fmt 零 diff

---

## Why

v1.1.0-omega 系统性深度优化(Phase I-V)已闭环 21 项优化点中的 17 项,剩余 4 项因"规模验证门槛 / 编译配置复杂度 / 重构风险 / 测试工程量"评估后延后到 v1.2.0-omega 周期。这 4 项任务并非可选项 — 它们是 v1.2.0-omega 的核心交付物,关闭后项目才能宣称"深度优化全量完成"。

本 Spec 的目标是:在严格遵守第二阶段开发规则(nuxus规则 §3.3:OMEGA 四定律守恒 / 依赖方向不可逆 / TDD 守恒 / 领域类型稳定性 / 向后兼容 / 新 crate 准入)的前提下,以前置脱敏化提交为安全门槛,按"测试基础设施 → 独立功能增强 → 算法规模验证 → 高风险重构"的优先级顺序推进 4 项延后任务,确保每一项都有充分的回归保护与可量化收益。

## What Changes

### 前置任务(P0,阻塞所有后续)

- **[Task 0]** 对 Phase V commit 7024b03 涉及的 26 个修改文件执行严格的脱敏化扫描 — grep 敏感模式(api_key / secret / password / token / private_key / 硬编码凭据 / 个人路径信息),人工核验命中项,必要时脱敏;确认无 `.env` / 凭据文件被暂存;逐文件 `git add`(严禁 `git add -A`);设置 git identity env vars 后 `git commit` + `git push origin master`

### 主体任务(按优先级顺序)

- **[P1 Task 1 / V-10]** **测试覆盖补齐**(基础设施,12h)— 为 5 个缺 benches 的 crate 补齐 criterion 基准、为 5 个缺 proptest 的 crate 补齐属性测试、为 23 个 crate 补齐 doctest、将 fuzz target 从 3 个扩展到 6 个。这是后续 I1/E1 任务的回归安全网,必须先完成
- **[P2 Task 2 / N15]** **repo-wiki FTS5 全文索引**(独立功能增强,8h)— 启用 SQLite FTS5 扩展替代 `LIKE '%query%'` 全表扫描,需解决 Windows GNU 工具链下 `sqlite3` 编译配置复杂问题(可能需 `bundled` feature 或 `SQLITE_ENABLE_FTS5` 编译开关)
- **[P3 Task 3 / I1]** **model-router MoE 稀疏门控**(算法规模验证,20h)— 实现稀疏门控路由,50+ 模型规模下将 O(n) 全量评估降为 O(k) Top-K 激活;需搭建 50+ 模型规模的测试夹具验证收益(当前 3 模型无收益)
- **[P4 Task 4 / E1]** **chimera-cli OnceCell 懒加载**(高风险重构,8h)— 将 14 个配置 section 从 eager 全量加载改为 `OnceCell` 懒初始化,消除启动期不必要的解析开销;14 section 重构风险高,需独立设计与完整测试覆盖,放最后执行

### **BREAKING** 变更声明

- **无** — 所有变更严格遵守 §3.3.1.5 向后兼容。具体保证:
  - N15 FTS5:仅 `repo-wiki` 内部查询实现变更,公开 API(`search` / `index_document`)签名不变;FTS5 不可用时回退到 LIKE(降级策略)
  - I1 MoE 门控:`ModelRegistry::route` 内部增加稀疏门控逻辑,公开 API 不变;模型数 < 阈值时自动退化为全量评估(向后兼容)
  - E1 OnceCell:配置 section 访问 API 从同步立即解析改为懒加载,调用方无感知(`get()` 语义不变)
  - V-10 测试补齐:纯测试代码增量,不修改生产代码

## Impact

### 受影响的 Spec(已有)

- `v1-1-0-systematic-optimization-deep-analysis`(Phase V 已完成)— 本 Spec 承接其 4 项延后任务(V-2/V-6/V-7/V-10),不修改 v1.1.0 已交付产物
- `v1-0-0-omega-ga-release-sprint`(GA 已发布)— 本 Spec 不影响已发布 GA 产物

### 受影响的代码

**L1 Core**:
- `crates/model-router/src/strategies.rs` + `registry.rs`(I1 MoE 稀疏门控)

**L5 Knowledge**:
- `crates/repo-wiki/src/store.rs` + 新增 `fts.rs` 模块(N15 FTS5 全文索引)

**L10 Interface**:
- `crates/chimera-cli/src/config.rs` + 14 个 section 模块(E1 OnceCell 懒加载)

**测试基础设施(跨多 crate)**:
- 5 crate 新增 `benches/*.rs`(V-10)
- 5 crate 新增 `tests/proptest.rs`(V-10)
- 23 crate `src/lib.rs` + 公开 API 补 `///` doctest(V-10)
- `fuzz/fuzz_targets/` 新增 3 个 target(V-10)
- `fuzz/Cargo.toml` 新增 3 个 `[[bin]]` 声明(V-10)

### 受影响的依赖

- N15 可能新增 `rusqlite` 的 `bundled` feature(若系统 sqlite3 不支持 FTS5)— 通过 workspace.dependencies 调整
- 无新增 crate(严格遵守 §3.3.1.6 新 crate 准入)
- 不变更核心领域类型(`UserIntent`/`Quest`/`Checkpoint`/`OmniSparseMasks`/`CLV`/`NexusState` — §3.3.1.4)

## ADDED Requirements

### Requirement: 前置脱敏化与安全提交

系统 SHALL 在任何 v1.2.0-omega 代码变更前,对 Phase V commit 7024b03 的 26 个修改文件执行严格的脱敏化扫描,确保无敏感信息(API key / secret / password / token / private key / 个人路径)泄露到远程仓库。

#### Scenario: 脱敏化扫描通过

- **WHEN** 对 26 个修改文件执行 grep 敏感模式扫描
- **THEN** 所有命中项经人工核验确认为非敏感(如测试用例的占位字符串 `test_key`)或已脱敏
- **AND** 确认 `.gitignore` 覆盖 `.env*` / `*.pem` / `credentials*`
- **AND** `git status` 显示无 `.env` / 凭据文件被暂存

#### Scenario: 脱敏化发现问题

- **WHEN** 扫描发现真实敏感信息(如硬编码 API key)
- **THEN** 该文件脱敏处理(替换为占位符或从环境变量读取)后重新 stage
- **AND** 在 commit message 中记录脱敏化处理说明

#### Scenario: 安全提交流程

- **WHEN** 脱敏化扫描通过
- **THEN** 逐文件 `git add`(严禁 `git add -A` / `git add .`)
- **AND** 设置 git identity env vars(`GIT_AUTHOR_NAME` / `GIT_AUTHOR_EMAIL` / `GIT_COMMITTER_NAME` / `GIT_COMMITTER_EMAIL`)
- **AND** 使用 PowerShell here-string `@'...'@` 传入 commit message
- **AND** `git push origin master`(若网络不可达,记录状态等待用户手动重试)

### Requirement: 测试覆盖补齐(V-10)

系统 SHALL 为测试基础设施存在缺口的 crate 补齐 criterion benches、proptest 属性测试、doctest、fuzz target,确保 v1.2.0-omega 的算法验证(I1)与高风险重构(E1)有充分的回归保护。

#### Scenario: benches 补齐

- **WHEN** 5 个缺 benches 的 crate 被识别
- **THEN** 每个 crate 新增 `benches/<name>_bench.rs`,包含至少 2 个 criterion bench(延迟 + 吞吐量)
- **AND** `Cargo.toml` 声明 `[[bench]]` + `harness = false` + dev-dependencies `criterion`
- **AND** `cargo bench -p <crate> --no-run` 编译通过

#### Scenario: proptest 补齐

- **WHEN** 5 个缺 proptest 的 crate 被识别
- **THEN** 每个 crate 新增 `tests/proptest.rs`,包含至少 1 个 proptest 验证核心不变量
- **AND** proptest 1.11+ block-named 语法 `fn test_name(x in 0..100u32) { ... }`
- **AND** `cargo test -p <crate> --test proptest` 通过

#### Scenario: doctest 补齐

- **WHEN** 23 个 crate 的公开 API 缺 doctest
- **THEN** 每个 crate 的 `src/lib.rs` 公开 API 补 `///` 文档注释含可运行示例
- **AND** `cargo test --doc -p <crate>` 通过

#### Scenario: fuzz 扩展

- **WHEN** 当前 3 个 fuzz target 需扩展到 6 个
- **THEN** 新增 3 个 target 覆盖关键 parser / serializer / sandbox 路径
- **AND** `fuzz/Cargo.toml` 声明对应 `[[bin]]` + `[package.metadata] cargo-fuzz = true`
- **AND** `cargo check --manifest-path fuzz/Cargo.toml` 通过(Windows 静态验证)
- **AND** 实际执行委托 Linux CI `fuzz.yml`

### Requirement: repo-wiki FTS5 全文索引(N15)

系统 SHALL 为 `repo-wiki` 启用 SQLite FTS5 扩展,替代 `LIKE '%query%'` 全表扫描,在大规模文档库(1000+ 文档)场景下实现 O(log n) 全文检索。

#### Scenario: FTS5 可用

- **WHEN** sqlite3 编译时启用 FTS5 扩展(`SQLITE_ENABLE_FTS5` 编译开关或 `rusqlite` `bundled` feature)
- **THEN** `WikiStore` 初始化时创建 FTS5 虚拟表 `CREATE VIRTUAL TABLE docs_fts USING fts5(...)``
- **AND** `index_document` 时同步写入 FTS5 索引
- **AND** `search` 时优先走 FTS5 `MATCH` 查询,LIKE 作为回退

#### Scenario: FTS5 不可用降级

- **WHEN** sqlite3 不支持 FTS5(运行时检测失败)
- **THEN** 回退到 `LIKE '%query%'` 全表扫描
- **AND** 记录 warning 日志提示"FTS5 不可用,回退到 LIKE,大规模场景性能受限"

#### Scenario: Windows GNU 编译配置

- **WHEN** Windows GNU 工具链下 `rusqlite` `bundled` feature 编译 FTS5
- **THEN** 通过 `Cargo.toml` features 配置启用 `bundled` + `SQLITE_ENABLE_FTS5` 编译开关
- **AND** 验证 `cargo build -p repo-wiki` 在 Windows GNU 编译通过
- **AND** 验证 Linux CI 同样编译通过(fuzz.yml / release.yml)

### Requirement: model-router MoE 稀疏门控(I1)

系统 SHALL 为 `model-router` 实现 MoE(Mixture of Experts)稀疏门控,在 50+ 模型规模下将路由决策从 O(n) 全量评估降为 O(k) Top-K 激活(k ≤ 5),仅对 Top-K 候选模型计算完整成本/延迟。

#### Scenario: 大规模路由加速

- **WHEN** 注册模型数 ≥ 50
- **THEN** `route()` 先用轻量级门控函数(如基于 CLV cosine similarity 的粗筛)选取 Top-K 候选
- **AND** 仅对 Top-K 候选计算完整 CACR 成本/延迟评估
- **AND** 路由延迟从 O(n) 降为 O(k)(k ≤ 5),p95 延迟降低 ≥ 40%

#### Scenario: 小规模自动退化

- **WHEN** 注册模型数 < 阈值(默认 50)
- **THEN** 自动退化为全量评估(O(n)),保持与当前行为一致
- **AND** 记录 debug 日志"模型数 < 阈值,跳过稀疏门控"

#### Scenario: 50+ 模型规模验证

- **WHEN** I1 实施时
- **THEN** 必须搭建 50+ 模型规模的测试夹具(可用 mock ModelInfo 批量生成)
- **AND** bench 对比 O(n) vs O(k) 路由延迟,验证 p95 改善 ≥ 40%
- **AND** proptest 验证门控函数的稀疏性不变量(每次仅激活 k 个)

### Requirement: chimera-cli OnceCell 懒加载(E1)

系统 SHALL 将 `chimera-cli/config.rs` 的 14 个配置 section 从 eager 全量加载改为 `OnceCell` 懒初始化,仅在首次访问时解析对应 section,消除启动期不必要的解析开销。

#### Scenario: 懒加载语义

- **WHEN** 配置文件包含 14 个 section
- **THEN** 启动时仅解析必需 section(如 `core` / `logging`)
- **AND** 其他 section 在首次 `get::<Section>()` 时通过 `OnceCell::get_or_init` 懒加载
- **AND** 重复访问返回已缓存值,无重复解析

#### Scenario: 向后兼容

- **WHEN** 下游代码调用 `config.get::<MemorySection>()` 
- **THEN** API 签名与返回类型不变
- **AND** 首次调用可能略慢(解析延迟),后续调用返回缓存(O(1))
- **AND** 配置文件格式不变,既有配置文件零修改可用

#### Scenario: 14 section 重构安全

- **WHEN** 重构 14 个 section 的加载逻辑
- **THEN** 每个 section 的重构独立提交,便于回滚
- **AND** 每次重构后运行 `cargo test -p chimera-cli` 验证
- **AND** 全部完成后运行 `cargo test --workspace` 验证无回归

## MODIFIED Requirements

### Requirement: repo-wiki 全文检索(N15 修改后)

`repo-wiki::WikiStore::search` 在 N15 实施后 SHALL 优先使用 FTS5 `MATCH` 查询,FTS5 不可用时回退到 `LIKE`。

### Requirement: model-router 路由决策(I1 修改后)

`model-router::ModelRegistry::route` 在 I1 实施后 SHALL 在模型数 ≥ 阈值时启用稀疏门控,模型数 < 阈值时退化为全量评估。

### Requirement: chimera-cli 配置加载(E1 修改后)

`chimera-cli::Config::get::<T>()` 在 E1 实施后 SHALL 通过 `OnceCell` 懒加载对应 section,而非启动期全量 eager 解析。

## REMOVED Requirements

### Requirement: repo-wiki LIKE 全表扫描作为唯一检索方式

**Reason**: N15 优化 — 大规模文档库(1000+)场景下 LIKE O(n) 全表扫描性能不足
**Migration**: FTS5 作为主检索方式,LIKE 作为 FTS5 不可用时的降级回退

### Requirement: model-router 全量评估作为唯一路由策略

**Reason**: I1 优化 — 50+ 模型规模下 O(n) 全量评估延迟过高
**Migration**: 稀疏门控作为主路由策略(≥ 阈值),全量评估作为小规模退化策略(< 阈值)

### Requirement: chimera-cli eager 全量配置加载

**Reason**: E1 优化 — 启动期解析 14 个 section 中未使用的部分造成不必要开销
**Migration**: OnceCell 懒加载,仅首次访问时解析

---

## 附:执行策略说明

### 优先级排序依据

| 优先级 | 任务 | 工时 | 排序依据 |
|--------|------|------|---------|
| P0 | Task 0 脱敏化 + 提交 | 1h | 阻塞所有后续,安全门槛 |
| P1 | Task 1 / V-10 测试覆盖补齐 | 12h | 基础设施,为 I1/E1 提供回归安全网 |
| P2 | Task 2 / N15 FTS5 | 8h | 独立功能增强,低风险,快速完成 |
| P3 | Task 3 / I1 MoE 门控 | 20h | 算法规模验证,中等复杂度,需测试夹具 |
| P4 | Task 4 / E1 OnceCell | 8h | 高风险重构,需最完整测试覆盖,放最后 |

### 子代理协作团队配置(§9 要求)

| 子代理 | 职责 | 任务映射 |
|--------|------|---------|
| **安全脱敏 agent** | Task 0 脱敏化扫描 + 安全提交 | Task 0 |
| **测试基础设施 agent** | benches / proptest / doctest / fuzz 补齐 | Task 1 (V-10) |
| **存储优化 agent** | FTS5 全文索引 + 降级策略 | Task 2 (N15) |
| **算法优化 agent** | MoE 稀疏门控 + 50+ 模型验证 | Task 3 (I1) |
| **重构专家 agent** | OnceCell 懒加载 + 14 section 重构 | Task 4 (E1) |
| **质量验证 agent** | cargo test / clippy / fmt / bench 全量回归 | 每任务末尾 |
| **文档同步 agent** | CODE_WIKI / CHANGELOG / project_memory 同步 | 每任务末尾 |

### 多轮结构化思考与验证流程

每个任务执行前 MUST 经过以下三轮结构化思考:

1. **Round 1(现状核验)**:Read 目标代码当前状态,验证延后理由是否仍然成立
2. **Round 2(方案设计)**:基于核验结果设计具体修改方案,明确文件路径与代码片段
3. **Round 3(影响评估)**:评估变更对其他 crate 的影响,识别潜在 break 点

每个任务执行后 MUST 经过严谨验证流程:

1. **V1(编译验证)**:`cargo check --workspace` 退出码 0
2. **V2(测试验证)**:`cargo test --workspace --jobs 1` 退出码 0,测试数量 ≥ 基线(3228+ v1.2.0 起点)
3. **V3(lint 验证)**:`cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
4. **V4(格式验证)**:`cargo fmt --all -- --check` 退出码 0
5. **V5(文档验证)**:相关 CODE_WIKI / CHANGELOG / project_memory 章节已同步

### 长期主义工作理念

- 不为短期性能牺牲架构纪律(如不引入 unsafe / 不违反依赖铁律)
- 每个优化点必须有测试覆盖,不允许"裸奔"优化
- 每个任务必须有验证报告归档,作为后续演进的历史参考
- 高风险任务(E1)放最后,且需前置任务(V-10)提供完整回归保护
- 不竭泽而渔:若某任务发现超出预期的工作量,记录并评估是否再次延后,而非强行推进导致质量问题

### 资源授权

团队可调用所有符合任务要求且系统允许的工具资源,包括但不限于:
- **MCP**:Sequential Thinking(多轮思考)、Memory(经验记录)、DesktopCommander(文件操作)
- **Skills**:test-driven-development、systematic-debugging、requesting-code-review、verification-before-completion
- **Sub-agents**:Explore(代码搜索)、general-purpose(多步骤任务)、rust-architecture-expert(Rust 架构专家)、algorithm-optimization-team(算法优化团队)

### 交付成果

每个任务交付:
1. **实现代码**:遵循 TDD(RED-GREEN-REFACTOR),含 WHY 注释
2. **测试覆盖**:单元测试 + 集成测试 + (适用时)proptest / bench
3. **验证报告**:`docs/optimization/v1.2.0/<task>_verification_report.md`
4. **文档同步**:CODE_WIKI / CHANGELOG / project_memory 章节更新

最终交付:
- `docs/optimization/v1.2.0/full_deferred_optimization_report.md` — 全量延后优化报告
- `CHANGELOG.md` v1.2.0-omega 章节 — 完整变更记录
- `project_memory.md` 新增 4-8 条 Lessons Learned
- v1.1.0 spec `tasks.md` 中 V-2/V-6/V-7/V-10 标记为"已在 v1.2.0-omega 完成"
