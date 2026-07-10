# v1.3.0-omega 后续优化路线图 Spec

> **来源**:`docs/optimization/v1.2.0/full_deferred_optimization_report.md` §6 后续优化建议 + §7.1 GA 前待办
> **原则**:YAGNI(You Aren't Gonna Need It)+ 长期主义 + 优先级驱动
> **协作模式**:精英专家级子代理团队,系统性分布式深度分析 + 多轮结构化验证

## Why

v1.2.0-omega 四项延后任务(I1/N15/E1/V-10)已完成并推送(commit `9f43d97`,3403 passed)。综合报告 §6 列出 9 项后续优化建议,分短期 / 中期 / 长期三档;§7.1 列出 3 项 GA 前待办。本 spec 将这 12 项建议整合为统一路线图,按优先级驱动推进,确保 GA 发布前完成必要收尾,并在 GA 后按需启动短期增强,避免一次性过度投入资源。

## What Changes

### P0 — GA 前收尾(3 项,必做,阻塞 GA 发布)

- **G1**:执行 `cargo audit --deny warnings`,核验 `Cargo.lock` 13 个关键依赖无 CVE(网络可用时)
- **G2**:在 `CHANGELOG.md` 追加 v1.2.0-omega 完整汇总章节(整合 Task 0-5 章节为统一概述)
- **G3**:在 `project_memory.md` 追加 v1.2.0-omega 总结教训章节(提炼 Task 1-4 教训为 5-8 条核心原则)

### P1 — 短期增强 v1.3.0-omega(3 项,GA 后启动,按风险升序)

- **S1**:chimera-cli 懒加载并发性能压测 — 14 section 并发访问 `OnceLock` 竞争基准(最低风险,纯测试新增)
- **S2**:model-router MoE 评分维度扩展 — 加入 `historical_success_rate` / `avg_latency_variance` 运行时统计维度(中等风险,需历史数据采集)
- **S3**:repo-wiki FTS5 trigram tokenizer 升级 — `unicode61` → `trigram`(适合短查询,无 libicu 编译依赖,改善 CJK 子串检索)

### P2 — 中期演进 v1.4.0-omega+(3 项,条件触发,非阻塞)

- **M1**:repo-wiki 向量索引升级 — 内存 KNN → sqlite-vec(需解决 unsafe 约束)或外部向量数据库
  - **触发条件**:Wiki entries > 1000 且 KNN 搜索 p95 > 10ms
- **M2**:model-router 路由策略学习 — `gate_score` 权重从静态 0.4/0.4/0.2 演进为学习参数
  - **触发条件**:历史路由数据累积 > 10000 条且静态权重导致 > 5% 次优路由
- **M3**:chimera-cli 配置热重载 — `LazyConfig` 扩展 `notify` + `watch` + 热重载
  - **触发条件**:用户明确请求运行时配置变更能力

### 明确排除(YAGNI)

- **长期 v2.0+ 三项**(FTS5 BM25 定制 / MoE 动态阈值 / 配置 schema 版本化)不在本 spec 范围,待 v1.4.0+ 评估后再立 spec
- **fuzz 6 target 实际执行**已委托 Linux CI(`fuzz.yml`),非本 spec 范围
- **不引入新 crate**(遵守 §3.3.1.6 新 crate 准入)

## Impact

- **Affected specs**:
  - `v1-2-0-omega-deferred-optimization`(G1/G2/G3 关闭其 GA 前待办)
  - `v1-1-0-systematic-optimization-deep-analysis`(S1/S2/S3 是其 V-2/V-6/V-7 的后续演进)
- **Affected code**:
  - **G1-G3**:仅文档/审计,零代码修改
  - **S1**:`crates/chimera-cli/benches/config_concurrency_bench.rs`(新增,纯 bench)
  - **S2**:`crates/model-router/src/moe.rs` + `strategies.rs` + `registry.rs`(扩展评分维度 + 历史数据采集)
  - **S3**:`crates/repo-wiki/src/fts.rs` + `store.rs` + `Cargo.toml`(tokenizer 切换 + 降级策略调整)
  - **M1-M3**:较大范围修改,触发条件未满足前不启动
- **Affected layers**:L1(model-router)/ L5(repo-wiki)/ L10(chimera-cli),均符合 §2.2 依赖铁律

## ADDED Requirements

### Requirement: P0 GA 前收尾

系统 SHALL 在 GA 发布前完成 3 项收尾:依赖审计无 CVE、CHANGELOG 完整汇总、project_memory 总结教训。

#### Scenario: cargo audit 通过
- **WHEN** 执行 `cargo audit --deny warnings`
- **THEN** 退出码 0,`Cargo.lock` 13 个关键依赖无已知 CVE

#### Scenario: CHANGELOG 汇总章节
- **WHEN** 阅读 `CHANGELOG.md` v1.2.0-omega 章节
- **THEN** 包含 Task 0-5 统一概述 + 关键数据(3403 passed / +175 增量 / commit hash)

#### Scenario: project_memory 总结教训
- **WHEN** 阅读 `project_memory.md` v1.2.0-omega 总结章节
- **THEN** 包含 5-8 条提炼自 Task 1-4 的核心原则(非 24 条细节教训的重复)

### Requirement: P1 短期增强(S1 懒加载并发压测)

系统 SHALL 提供 chimera-cli `LazyConfig` 14 section 并发访问的性能基准,验证 `OnceLock::get_or_init` 在高并发场景下的竞争情况。

#### Scenario: 并发基准通过
- **WHEN** 执行 `cargo bench -p chimera-cli --bench config_concurrency_bench`
- **THEN** 14 section 并发访问的 p99 延迟 < 100μs(OnceLock spinlock 不成为瓶颈)

### Requirement: P1 短期增强(S2 MoE 评分维度扩展)

系统 SHALL 扩展 `MoeGate` 评分函数,加入 `historical_success_rate` 与 `avg_latency_variance` 两个运行时统计维度,权重通过配置可调。

#### Scenario: 五维评分生效
- **WHEN** 注册表模型数 ≥ 50 且历史数据可用
- **THEN** `gate_score` 使用五维评分(cost / latency / quality / historical_success_rate / avg_latency_variance),权重默认 0.3/0.3/0.2/0.1/0.1

#### Scenario: 历史数据不足时降级
- **WHEN** 模型历史路由数据 < 100 条
- **THEN** 自动降级为三维评分(向后兼容 v1.2.0 行为)

### Requirement: P1 短期增强(S3 FTS5 trigram tokenizer)

系统 SHALL 将 FTS5 tokenizer 从 `unicode61` 升级为 `trigram`,改善 CJK 子串检索召回率,无需 libicu 编译依赖。

#### Scenario: CJK 子串检索命中
- **WHEN** 索引 "性能分析报告" 后搜索 "分析"
- **THEN** FTS5 MATCH 直接命中(无需降级 LIKE)

#### Scenario: trigram 不可用时降级
- **WHEN** FTS5 trigram 创建失败(老版本 SQLite 不支持)
- **THEN** 降级为 unicode61 + CJK 空结果降级 LIKE(v1.2.0 行为)

## MODIFIED Requirements

### Requirement: FTS5 全文索引(v1.2.0 → v1.3.0 演进)

v1.2.0 的 `FtsCapability` 枚举从二值(`Available` / `Unavailable`)扩展为三值:
- `AvailableTrigram` — trigram tokenizer 可用(首选)
- `AvailableUnicode61` — 仅 unicode61 可用(降级,配合 CJK 空结果降级 LIKE)
- `Unavailable` — FTS5 完全不可用(降级 LIKE)

`search_fulltext` 查询路径优先级:trigram MATCH > unicode61 MATCH + 空结果降级 > LIKE。

### Requirement: MoE 门控评分(v1.2.0 → v1.3.0 演进)

v1.2.0 的 `gate_score` 三维倒数评分扩展为五维(条件性):
- 历史数据充足(≥ 100 条):五维评分
- 历史数据不足:三维评分(向后兼容)

`MoeGate` 新增 `history_store: Option<&HistoryStore>` 字段,`None` 时退化三维。

## REMOVED Requirements

无。本 spec 不移除任何现有需求,所有变更向后兼容。

---

## 设计原则

1. **YAGNI**:P2 中期三项设明确触发条件,未触发前不启动,避免过度工程化
2. **长期主义**:优先级 P0 → P1 → P2 严格递进,不跳级,不并行跨档任务
3. **TDD 守恒**:S1/S2/S3 每项先写失败测试再实现,不允许删除已有测试
4. **依赖方向不可逆**:L1/L5/L10 修改遵守 §2.2,跨层通信只走 Event Bus
5. **`#![forbid(unsafe_code)]` 守恒**:S3 trigram 不引入 unsafe 依赖,M1 sqlite-vec 需评估 unsafe 约束
6. **向后兼容**:S2/S3 保留 v1.2.0 降级路径,不破坏既有 API

## 关联文档

- **来源**:`docs/optimization/v1.2.0/full_deferred_optimization_report.md` §6 + §7.1
- **前置 spec**:`v1-2-0-omega-deferred-optimization`(已完成)
- **关联 spec**:`v1-1-0-systematic-optimization-deep-analysis`(V-2/V-6/V-7 后续演进)
- **规则**:`.trae/rules/nuxus规则.md` §3.3 第二阶段开发原则
