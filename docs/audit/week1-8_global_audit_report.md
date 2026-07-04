# Week 1-8 全局深度审计报告

> **审计日期**: 2026-06-28
> **审计范围**: 34 个 crates,~2,940 个测试,十层架构
> **审计团队**: 7 维度专家 + 1 独立审计员(8 路并行)
> **审计方法**: 证据驱动,所有结论引用具体代码位置
> **审计性质**: 只读分析,未修改任何代码

---

## 1. 执行摘要

### 1.1 总体评价

Chimera CLI(NEXUS-OMEGA)项目在 Week 1-8 八周开发周期结束时整体表现 **良好(B+)**:

- **架构基线优秀**:十层架构映射与 34 个 crate 100% 一致,依赖方向 100% 合规,OMEGA 四定律全部落地,3 个关键 ADR(003/004/005)按决策执行,nexus-core 保持最小依赖,跨层通信统一走 EventBus。
- **安全基线扎实**:`#![forbid(unsafe_code)]` 100% 覆盖(34 lib.rs + 1 main.rs),OWASP Top 10 测试 20/20 通过,Decay 能力衰减模型完整,SecCore 四层防御已实现。
- **代码质量优良**:生产代码 0 个 `unwrap/expect/panic`,单函数 ≤200 行铁律 100% 遵守,WHY 注释覆盖优秀(482 处 / 100 文件),workspace 级依赖声明 100% 合规。
- **测试体系完善**:~2,940 个测试(较 Week 1-4 基线增长 83.8%),32/34 crate ≥ 20 测试,1000 次压测 p95=4ms 远低于 2000ms 阈值。

但存在 **11 个 Critical + 19 个 Major + 44 个 Minor** 共 74 个问题需在 v1.0.1 修复。Critical 问题集中在三处:**faae-router 持锁跨 await 系统性违规(4 项)**、**SQLite 操作缺 spawn_blocking(2 项)**、**文档-代码漂移(4 项)**,以及 **BudgetExceeded severity 违反 Hard Constraint(1 项)**。这些问题不影响编译与功能,但会误导新开发者、阻塞生产部署、在 高并发/慢消费者 场景下导致死锁或事件丢失。

### 1.2 基线指标

| 指标 | 当前值 | 行业基准 | 状态 |
|------|--------|---------|------|
| crate 数量 | 34 | - | ✅ |
| 测试总数 | ~2,940(crate 2,817 + E2E 123) | - | ✅ |
| `#![forbid(unsafe_code)]` 覆盖率 | 100%(34 lib.rs + 1 main.rs) | 100% | ✅ |
| 依赖方向合规性 | 100%(0 个 L(N)→L(N+1) 违规) | 100% | ✅ |
| 单函数 ≤200 行 | 100%(生产代码 0 个超长函数) | 100% | ✅ |
| 生产代码 unwrap/expect/panic | 0 | 0 | ✅ |
| TODO/FIXME | 27(全部有明确 Week 计划) | <10/1K行 | ✅ |
| 文档注释覆盖率 | 高(482 WHY 注释 / 100 文件) | 中 | ✅ |
| OWASP Top 10 测试 | 20/20 通过 | 20/20 | ✅ |
| workspace 级依赖声明 | 100% | 100% | ✅ |
| 错误处理一致性 | 33 thiserror + 1 anyhow | - | ✅ |
| broadcast subscribe 时机 | 100% 合规(spawn 前同步) | 100% | ✅ |
| Top-K 选择算法 | 全部 `select_nth_unstable_by` | - | ✅ |
| `FuturesUnordered` vs `join_all` | 0 处 `join_all` | 0 | ✅ |
| `with_capacity` 预分配 | 70+ 处 | - | ✅ |

### 1.3 问题分布

| 严重程度 | 数量 | 维度分布 |
|---------|------|---------|
| Critical | 11 | B(4), C(2), F(1), G(4) |
| Major | 19 | B(1), C(5), D(3), E(1), F(5), G(4) |
| Minor | 44 | A(5), B(3), C(6), D(6), E(6), F(7), G(11) |
| **总计** | **74** | — |

> 注:维度 B 报告摘要声称为 "Critical 4 / Major 2 / Minor 3",但其问题清单表格仅含 1 个 Major(B-Maj-1),本汇总以问题清单表格为准。维度 G 报告摘要声称为 "Major 6 / Minor 9",但问题清单表格含 4 Major(G-05~G-08)+ 11 Minor(G-09~G-19),本汇总以表格为准。

### 1.4 关键发现

1. **faae-router 存在系统性并发违规**:4 个 Critical 问题(B-Crit-1 ~ B-Crit-4)同源于一个设计反模式——"持锁跨 await 访问嵌套 RwLock"。`router.rs:196-213` 与 `edsb.rs:284-326` 在 `expert_registry` 读锁内调用 `edsb.balance().await`、`decay_usage_counts().await` 等含 await 点的函数,且 `spawn_decay_loop` 后台任务持外层读锁跨多个 await,高并发下将阻塞 register/unregister/decay 写路径并互锁路由读路径。修复需一次性重构 `route`/`register_expert`/`unregister_expert`/`decay_usage_counts`/`spawn_decay_loop`,统一采用"快照 → 释放锁 → await"模式。

2. **SQLite 访问层 spawn_blocking 覆盖不一致**:cmt-tiering、mlc-engine 已用 `spawn_blocking` 包装 SQLite,但 `repo-wiki/store.rs` 与 `scc-cache/wal.rs` 完全缺失此模式,所有方法为同步签名直接调用 `rusqlite::Connection::execute`,在 async 上下文调用将阻塞 Tokio 工作线程。建议在 nexus-core 或新建 `sqlite-util` crate 中提供统一 `AsyncSqlitePool` 抽象。

3. **文档-代码漂移严重**:AETHER §6.2 中 **15 个 crate 层级标注错误**(decb-governor/acb-governor 标 L3 实为 L8;mlc-engine/hcw-window 标 L5 实为 L2;mcp-mesh 标 L1 实为 L10 等),标题"37 crates"与实际 34 个不符;CHANGELOG Week 8 Task 2 中 acb-governor 与 auto-dpo 发布的事件名(`AcbCapabilityAdjusted` / `DpoSampleCollected`)在代码中不存在(实际为 `BudgetAdjusted` / `DpoPairGenerated`);`event-bus/src/lib.rs:8` 注释"32 个事件"严重过时(实际 66 个变体)。

4. **BudgetExceeded severity 违反 Hard Constraint**:`project_memory.md` 第 10 条 Hard Constraint 明确要求"BudgetExceeded event must be marked as Critical in `NexusEvent::severity()`",且第 43 行标记为 `✅ FIXED`,但 `crates/event-bus/src/types.rs:1142-1152` 中 `BudgetExceeded` 不在 Critical 列表,通过 `_ => Normal` 通配符返回 `Normal`。这是 Hard Constraint 违反 + FIXED 标记不实双重问题。

5. **历史问题修复率 73.7%**:Week 1-8 历史 review 共 19 项核验中 14 项已修复、1 项部分修复、2 项未修复(MTPE 伪预测按计划保持、BudgetExceeded severity 标记不实)、2 项委托用户验证 CI 产物。qeep-protocol 测试已从 8 个补充至 50 个(Major-2 解除);AHIRT 配置化 P2 已解决;但 MTPE 伪预测(Major-1)已超出 Week 7 计划仍为占位实现。

---

## 2. 维度审计汇总

### 2.1 维度 A:架构一致性

- **评级**: ✅ 合规(优秀)
- **核心结论**: 十层架构映射与 workspace members 完全一致,34 个 crate 生产依赖方向 100% 合规,OMEGA 四定律全部落地,3 个关键 ADR(003/004/005)按决策执行,nexus-core 保持最小依赖,跨层通信统一走 EventBus,V1/V2/V3/V4 历史违规均已通过事件机制修正。
- **问题数**: Critical 0 / Major 0 / Minor 5
- **关键亮点**: 7 类命名模式共 24 个类型全部符合规范;dev-dependencies 跨层引用均带明确注释说明"测试专用"。
- **详细报告**: [dimension_a_architecture.md](./dimension_a_architecture.md)

### 2.2 维度 B:并发安全

- **评级**: ⚠️ 需改进
- **核心结论**: 项目铁律"锁 guard 不跨 await 点"在绝大多数 crate 中被严格遵守(hcw-window/efficiency-monitor/mlc-engine/cmt-tiering 等均有优秀样例),但 **faae-router 存在 4 处系统性违规**(router.rs + edsb.rs),csn-substitutor 存在一处 DashMap TOCTOU 竞态,多处 fire-and-forget `tokio::spawn` 未管理 JoinHandle。
- **问题数**: Critical 4 / Major 1 / Minor 3
- **关键亮点**: broadcast subscribe 时机 100% 合规(全部在 spawn 之前同步调用);mpsc channel 配对规范;async fn Send + 'static 约束文档明确;decb-governor 主动用 `{}` 块规避 `MutexGuard` 非 Send 问题。
- **详细报告**: [dimension_b_concurrency.md](./dimension_b_concurrency.md)

### 2.3 维度 C:性能瓶颈

- **评级**: ⚠️ 良好(需改进)
- **核心结论**: 核心热路径已广泛采用 `select_nth_unstable_by`(10+ crate)、`FuturesUnordered`(0 处 `join_all`)、`with_capacity`(70+ 处)等最佳实践,但存在 2 处 Critical 级 SQLite 阻塞异步运行时问题(repo-wiki、scc-cache/wal 缺 spawn_blocking),以及 HCW/MLC clone 优化未闭环的 Major 问题。
- **问题数**: Critical 2 / Major 5 / Minor 6
- **关键亮点**: KVBSR 两级块路由达到 ~9× 加速(符合 10× 设计目标);HCW 压缩器 O(n + K log K)复杂度;MLC L2 SharedCLV 池化(4096 条目内存从 8MB 降至 k×2KB);WAL 1000 次崩溃恢复零丢失 p95=4ms;三层路由 p95=78.79µs(64× 余量)。
- **详细报告**: [dimension_c_performance.md](./dimension_c_performance.md)

### 2.4 维度 D:代码质量

- **评级**: ✅ 良好
- **核心结论**: `#![forbid(unsafe_code)]` 34/34 全覆盖,单函数 ≤200 行铁律 100% 遵守,workspace 级依赖声明 100% 合规,错误处理一致性优秀(33 thiserror + 1 anyhow),WHY 注释覆盖优秀(482 处)。主要技术债为 3 个伪实现(MTPE/FaaE/RepoWiki),均有明确替换计划和 WHY 注释说明,属于"有计划的占位实现"而非"失控的技术债"。
- **问题数**: Critical 0 / Major 3 / Minor 6
- **关键亮点**: 27 处 TODO 100% 有明确 Week 计划;16 处 `unwrap_or_default()` 均为合理降级;无错误吞没(`let _ = ...` 均有合理上下文)。
- **详细报告**: [dimension_d_quality.md](./dimension_d_quality.md)

### 2.5 维度 E:测试覆盖

- **评级**: ✅ 良好
- **核心结论**: 测试基础设施完善,覆盖广度优秀,~2,940 个测试(较 Week 1-4 基线 ~1,599 增长 83.8%),32/34 crate ≥ 20 测试。qeep-protocol 已从 8 个补充至 50 个(Major-2 解除),1000 次压测 p95=4ms 500× 余量。存在 1 个 Major 遗留(decay-engine 9 个测试,目标 ≥15)与若干 Minor 改进点。
- **问题数**: Critical 0 / Major 1 / Minor 6
- **关键亮点**: qeep-protocol 超时边界覆盖优秀(1ms/100ms/1s/10s + 刚好超时/未超时);并发测试多规模(10/50/100/1000 线程);跨周集成测试 3/4 已覆盖;proptest 19/34 crate 使用且语法符合规范;测试隔离性优秀(无全局可变状态,TempDir 隔离)。
- **详细报告**: [dimension_e_testing.md](./dimension_e_testing.md)

### 2.6 维度 F:安全

- **评级**: ⚠️ 部分合格(需加固)
- **核心结论**: 安全基线整体合格(OWASP Top 10 全覆盖、forbid(unsafe_code) 100%、AHIRT 配置化 P2 已解决),但存在 1 个 Critical(BudgetExceeded severity 违反 Hard Constraint)与 5 个 Major 问题(沙箱缺超时/资源限制、Linux gVisor 未实际启用、mcp-mesh 缺 SSRF 校验、Critical 事件未走 mpsc 点对点通道)需在生产部署前修复。
- **问题数**: Critical 1 / Major 5 / Minor 7
- **关键亮点**: Decay 能力衰减实现完整(连续流体模型 + 双驱动衰减 + 自动冻结 + 配置化);SecCore 四层防御已实现(静态分析 6 类攻击拦截 + 环境过滤 + 沙箱执行 + 审计 Merkle 链 + ASA 三档干预);13 个关键依赖无 High/Critical 漏洞;3 个 fuzz target 已委托 CI。
- **详细报告**: [dimension_f_security.md](./dimension_f_security.md)

### 2.7 维度 G:文档同步

- **评级**: ⚠️ 部分合规(核心文档体系完整,但存在严重"代码-文档"漂移)
- **核心结论**: 文档骨架完整(34 crate 全列出、8 周记录齐全、CI/Docker 基础配置正确),但存在四类严重漂移:(1) AETHER §6.2 目录结构对 15 个 crate 标注错误层级;(2) AETHER §10.1 核心数据结构字段与代码严重不一致;(3) CHANGELOG Week 8 Task 2 事件名称与代码不符;(4) `event-bus/src/lib.rs` 的"32 个事件"注释严重过时。此外 Dockerfile 缺 USER 非 root 与 HEALTHCHECK 配置,不符合生产安全基线。
- **问题数**: Critical 4 / Major 4 / Minor 11
- **关键亮点**: CODE_WIKI 34 crate 全列出且十层架构映射一致;CHANGELOG Week 1-8 章节齐全;CI 配置 5 平台 matrix + Docker job + fuzz 3 target 全部正确;docs/ 下五类文档(性能/安全/发布/grafana/architecture)齐全且与代码一致。
- **详细报告**: [dimension_g_documentation.md](./dimension_g_documentation.md)

### 2.8 历史问题追踪

- **评级**: 修复率 73.7%(14/19 已修复)
- **核心结论**: Week 1-4 cross-review 12 项中 11 项已修复(qeep 测试 50 个、HCW get_arc、MLC 条目级锁等),唯一未修复项 MTPE 伪预测按计划保持(依赖 Week 9 NMC ONNX);Week 5 deep-review 5 项中 3 项已修复、1 项标记不实(BudgetExceeded severity C2)、1 项部分修复(BudgetAdjusted 层级注释 M6);Week 8 limitations 3 项限制文件已全部创建,2 项委托用户验证 CI 产物。project_memory.md 6 个 ✅ FIXED 标记中 4 个一致、1 个不实(C2)、1 个部分一致(M6),1 个 P2 过时标记未更新。
- **详细报告**: [historical_issues_tracking.md](./historical_issues_tracking.md)

---

## 3. 历史问题追踪汇总

### 3.1 Week 1-4 cross-review 12 项

| ID | 问题 | 原状态 | 当前状态 | 核验证据 |
|----|------|--------|---------|---------|
| Major-1 | MTPE 伪预测 | Major | ❌ 未修复(按计划保持,依赖 Week 9 NMC ONNX) | `crates/mtpe-executor/src/predictor.rs:29-31, 115-119` 仍使用 `SIMULATED_INFERENCE_DELAY` + `generate_pseudo_predictions` |
| Major-2 | qeep-protocol 测试薄弱(8 个) | Major | ✅ 已修复(50 个测试,远超 ≥20 目标) | `crates/qeep-protocol/tests/qeep.rs`(40) + `tests/proptest.rs`(10) |
| Minor-1 | FaaE EDSB 伪随机 | Minor | ✅ 已修复(替换为香农熵驱动) | `crates/faae-router/src/edsb.rs:343` |
| Minor-2 | RepoWiki 占位嵌入 | Minor | ⚠️ 仍为伪实现(按计划 Week 6 替换,实际未替换) | `crates/repo-wiki/src/generator.rs:66` |
| Minor-3 | HCW/GEA 硬编码 | Minor | ✅ 已修复(已配置化) | `compressor.rs:249` + `activator.rs:142` 从 config 读取 |
| Minor-4 | HCW get() 返回 clone | Minor | ⚠️ 部分修复(get_arc() 内部仍 clone) | `crates/hcw-window/src/window.rs:176-183` 新增 get_arc() 但 `Arc::new(entry.clone())` |
| Minor-5 | 跨周集成测试覆盖 | Minor | ✅ 已修复(3/4 覆盖,仅 SSRA+MLC 缺失) | `crates/hcw-window/tests/integration_scc.rs` 等 |
| Minor-6 | changelog 一致性 | Minor | ✅ 已修复(Week 1-4 章节完整) | `CHANGELOG.md` |
| Minor-7 | 文档注释一致性 | Minor | ✅ 已修复(各 crate lib.rs `//!` 一致) | 各 crate lib.rs |
| Minor-8 | benchmark 覆盖 | Minor | ✅ 已修复(关键 crate 有 benches) | scc/sesa/ssra/kvbsr 等 |
| Minor-9 | error 处理一致性 | Minor | ✅ 已修复(33 thiserror + 1 anyhow) | 各 crate error.rs |
| Minor-10 | WHY 注释覆盖 | Minor | ✅ 已修复(482 处 WHY 注释) | 100 个 .rs 文件 |

### 3.2 Week 5-8 遗留项

| ID | 问题 | 原状态 | 当前状态 | 核验证据 |
|----|------|--------|---------|---------|
| C1 | Week 5 新增 8 事件 EventBus 集成 | Critical | ✅ 已修复 | `crates/event-bus/src/types.rs:720-875` 8 事件全部定义 |
| C2 | BudgetExceeded severity 标记 Critical | Critical | ❌ 未修复(标记不实) | `crates/event-bus/src/types.rs:1142-1152` 仍返回 Normal |
| M4 | ttg.rs 7 处 expect() 替换 | Major | ✅ 已修复 | `crates/quest-engine/src/ttg.rs:357-537` 全部替换为 `unwrap_or_else(\|p\| p.into_inner())` |
| M6 | BudgetAdjusted 层级注释修正 | Minor | ⚠️ 部分修复 | `types.rs:861` BudgetStatsReported 已修正,`types.rs:781` BudgetAdjusted 仍标 L3 |
| P2 | AHIRT 配置化 | Major | ✅ 已修复(标记过时) | `crates/parliament/src/config.rs:141-148` AhirtConfig 已引入 |
| 限制 1 | cargo-fuzz CI | Should | 🔄 委托验证 | `.github/workflows/fuzz.yml` 已创建,产物验证委托用户 |
| 限制 5 | clippy 根因分析 | Must | ✅ 已修复 | `docs/dev/clippy_root_cause_analysis.md` 结论为 OOM 触发 `__fastfail(7)` |
| 限制 2+3 | CI + Docker job | Should | 🔄 委托验证 | `.github/workflows/release.yml:149-225` Docker job 已补充 |

---

## 4. 问题清单(按优先级排序)

> 修复成本定义:XS(<30min) / S(30min-2h) / M(2-8h) / L(>8h)
> 排序规则:Critical → Major → Minor,同级按 L1→L10,文档/跨crate问题列于末尾

### 4.1 Critical 问题(阻塞生产,共 11 项)

| ID | 维度 | 层级 | 问题 | 代码位置 | 修复建议 | 修复成本 |
|----|------|------|------|---------|---------|---------|
| F-001 | F 安全 | L1 | BudgetExceeded severity() 返回 Normal,违反 Hard Constraint"必须 Critical" | `crates/event-bus/src/types.rs:1142-1151` | 在 severity() match 中将 `Self::BudgetExceeded { .. }` 显式列入 Critical 分支 | XS |
| C-02 | C 性能 | L3 | scc-cache SQLite WAL 操作未用 spawn_blocking,阻塞 async 运行时 | `crates/scc-cache/src/wal.rs:262,314-359` | 实现 WalTrait 的 async 包装层,用 spawn_blocking 委托同步 SqliteWal | S |
| C-01 | C 性能 | L5 | repo-wiki SQLite 操作未用 spawn_blocking,阻塞 async 运行时 | `crates/repo-wiki/src/store.rs:42,138-186,247,269` | 改为 async fn + spawn_blocking 包装,或用 r2d2 连接池 | S |
| B-Crit-1 | B 并发 | L6 | faae-router 持读锁跨 `edsb.balance().await`,阻塞 register/unregister/decay 写路径 | `crates/faae-router/src/router.rs:196-200` | 克隆 registry 快照后释放读锁,再调用 balance | M |
| B-Crit-2 | B 并发 | L6 | faae-router 三重嵌套锁 + 持锁跨 `last_used_at.write().await`,死锁风险 | `crates/faae-router/src/router.rs:207-213` | 缩小锁粒度:registry 仅查 Arc 后释放;profile 字段改原子操作 | M |
| B-Crit-3 | B 并发 | L6 | faae-router `decay_usage_counts` 嵌套读锁跨 `last_used_at.read().await` | `crates/faae-router/src/edsb.rs:284-304` | 去掉外层 profile.read(),仅对 last_used_at 加锁;计数用原子操作 | M |
| B-Crit-4 | B 并发 | L6 | faae-router `spawn_decay_loop` 持外层读锁跨 `decay_usage_counts().await`(内部多 await) | `crates/faae-router/src/edsb.rs:319-326` | 克隆 registry 快照为 Vec<Arc<...>> 后释放读锁,再 decay | M |
| G-01 | G 文档 | 文档 | AETHER §6.2 中 15 个 crate 层级标注错误(decb-governor/acb-governor 标 L3 实为 L8;mlc-engine/hcw-window 标 L5 实为 L2;mcp-mesh 标 L1 实为 L10 等) | `AETHER_NEXUS_OMEGA_ULTIMATE.md:485-528` | 按 CODE_WIKI §2.1 与各 crate lib.rs 层级标注同步修正,文档头部添加"以 CODE_WIKI 为准"声明 | S |
| G-02 | G 文档 | 文档 | AETHER §6.2 标题"37 crates"与 Cargo.toml 34 个 members 不一致 | `AETHER_NEXUS_OMEGA_ULTIMATE.md:487` | 修正为"34 crates",同步删除 §7 Week 1 Day 1 的"37 crates 骨架" | XS |
| G-03 | G 文档 | 文档 | CHANGELOG 称 acb-governor 发布 `AcbCapabilityAdjusted` 事件,实际为 `BudgetAdjusted` + `BudgetExceeded` | `CHANGELOG.md:21` | 修正为"发布 `BudgetAdjusted` / `BudgetExceeded` 事件" | XS |
| G-04 | G 文档 | 文档 | CHANGELOG 称 auto-dpo 发布 `DpoSampleCollected` 事件,实际为 `DpoPairGenerated` | `CHANGELOG.md:22` | 修正为"发布 `DpoPairGenerated` 事件" | XS |

### 4.2 Major 问题(影响可靠性,共 19 项)

| ID | 维度 | 层级 | 问题 | 代码位置 | 修复建议 | 修复成本 |
|----|------|------|------|---------|---------|---------|
| F-006 | F 安全 | L1 | EventBus 使用 broadcast 而非 mpsc,Critical 事件无点对点投递保障 | `crates/event-bus/src/bus.rs:34`、`backpressure.rs:9-13` | 实现 broadcast + mpsc 双通道:Critical 事件走独立 mpsc 通道 | L |
| G-07 | G 文档 | L1 | event-bus lib.rs:8 注释"32 个跨层事件"严重过时(实际 66 个变体) | `crates/event-bus/src/lib.rs:8` + `CODE_WIKI.md:447` | 更新为"定义 66 个跨层事件类型(Week 1-8 累计)" | XS |
| M-01 | C 性能 | L2 | HCW `get_arc()` 内部仍 `entry.clone()`,Minor-4 未完全闭环 | `crates/hcw-window/src/window.rs:180` | entries 改为 `Vec<Arc<ContextEntry>>`,get_arc 返回 `Arc::clone` | M |
| M-02 | C 性能 | L2 | HCW `get()` 深拷贝 ContextEntry(含 content String) | `crates/hcw-window/src/window.rs:162` | 同 M-01,或提供 `get_ref` 返回 `Arc<ContextEntry>` | M |
| M-03 | C 性能 | L2 | MLC 三个 tier `list_all()` 全量 clone,4096 条目 ~8MB 分配 | `crates/mlc-engine/src/{l0_working,l1_episodic,l2_semantic}.rs` | 提供 `list_all_arc()` 变体,或存储为 `Arc<MemoryEntry>` | M |
| M-05 | C 性能 | L2 | mlc-engine L2 benchmark 仅 100 条目,缺 4096 规模验证 | `crates/mlc-engine/benches/l2_recall.rs:21` | 增加 `bench_l2_recall_4096_entries` 验证 < 200ms 目标 | S |
| E-MAJOR-1 | E 测试 | L4 | decay-engine 测试严重不足(9 个,目标 ≥15),Week 1-4 遗留未修复 | `crates/decay-engine/tests/decay.rs` | 补充 6+ 测试:并发衰减、错误路径、边界值、restore 上限 | S |
| F-002 | F 安全 | L4 | 沙箱无超时机制,子进程可能永久阻塞(如 `sleep infinity`) | `crates/seccore/src/sandbox.rs:118-166` | Sandbox 增加 `timeout: Duration` 字段,用 `tokio::time::timeout` 包裹 | S |
| F-003 | F 安全 | L4 | 沙箱无资源限制(CPU/内存/FD),DoS 风险 | `crates/seccore/src/sandbox.rs:118-166` | Linux 用 `setrlimit`,Windows 用 Job Object | M |
| F-005 | F 安全 | L4 | 沙箱 Linux gVisor 未实际启用,与 ADR-001 不一致 | `crates/seccore/src/sandbox.rs:108-117` | 实现 `#[cfg(target_os="linux")]` 分支,通过 `runsc` 启动子进程 | L |
| Q-03 | D 质量 | L5 | RepoWiki 占位嵌入(伪实现,按计划 Week 6 替换,实际未替换) | `crates/repo-wiki/src/generator.rs:67` | Week 6 NMC 实现后替换为真实 CLV 嵌入(依赖 Week 9 NMC ONNX) | M |
| Q-02 | D 质量 | L6 | FaaE 伪随机概率实现(SystemTime 纳秒) | `crates/faae-router/src/edsb.rs:341` | Week 8 评估引入 rand crate 替换 | S |
| Q-01 | D 质量 | L7 | MTPE 伪预测占位实现(已超出 Week 7 计划) | `crates/mtpe-executor/src/predictor.rs:31,115,119,249` | Week 9 接入真实模型推理(依赖 NMC ONNX) | L |
| B-Maj-1 | B 并发 | L10 | csn-substitutor `register` TOCTOU 竞态(contains_key+insert 与 len()+insert 无锁保护) | `crates/csn-substitutor/src/substitutor.rs:89-118` | 改用 `DashMap::entry().or_insert()` 原子化 check-then-act | S |
| F-004 | F 安全 | L10 | mcp-mesh ServerRegistry 未校验 endpoint 格式,SSRF 风险 | `crates/mcp-mesh/src/server_registry.rs:88-95` | 校验 endpoint:拒绝 169.254.x.x/127.0.0.0/8/10.0.0.0/8 等内网地址 | S |
| M-04 | C 性能 | 跨crate | 10 个核心 crate 缺失 benchmark(nexus-core/event-bus/model-router/repo-wiki/qeep-protocol/decay-engine 等) | 见 C 报告 §7.2 | 优先为 nexus-core、event-bus、repo-wiki 补充基准 | L |
| G-05 | G 文档 | 文档 | project_memory.md L43 标记 BudgetExceeded 已 Critical,实际未修复 | `project_memory.md:43` + `event-bus/src/types.rs:1142-1152` | 与 F-001 一并处理:修复代码或更新标记 | XS |
| G-06 | G 文档 | 文档 | CODE_WIKI"Week 8: 286+"与 CHANGELOG"162 个"测试统计不一致 | `CODE_WIKI.md:34` + `CHANGELOG.md:93-99` | 统一测试统计口径 | XS |
| G-08 | G 文档 | 配置 | Dockerfile 缺 USER 非 root + HEALTHCHECK + LABELS,不符合生产安全基线 | `Dockerfile:37-43` | 添加 `USER nonroot:nonroot` + `HEALTHCHECK NONE` + OCI LABELS | S |

### 4.3 Minor 问题(改进项,共 44 项,列前 24 项最重要的)

| ID | 维度 | 层级 | 问题 | 代码位置 | 修复成本 |
|----|------|------|------|---------|---------|
| A-01 | A 架构 | L9 | quest-engine Cargo.toml:33 注释将 decb-governor 标为"L3 预算治理",实际为 L8 | `crates/quest-engine/Cargo.toml:33` | XS |
| A-02 | A 架构 | L10 | chimera-cli 骨架状态,无下层 crate path 依赖,未装配 EventBus | `crates/chimera-cli/Cargo.toml:6-20` | L(Week 7/8 集成) |
| A-03 | A 架构 | L5 | repo-wiki sqlite-vec 降级为内存向量检索,与 ADR-005 略有偏差 | `crates/repo-wiki/Cargo.toml:40-45` | M(长期) |
| A-04 | A 架构 | L8 | parliament dev-dependencies 引用 quest-engine(L9,L8→L9 向上) | `crates/parliament/Cargo.toml:44-46` | S |
| A-05 | A 架构 | L6 | gqep-executor 归属 L6/L7 不一致(根 Cargo.toml:203 vs 架构映射表) | `Cargo.toml:203` vs 架构映射表 | XS |
| B-Min-1 | B 并发 | L6 | kvbsr-router 自动重平衡 spawn 未保存 JoinHandle,fire-and-forget | `crates/kvbsr-router/src/router.rs:379-383` | S |
| B-Min-2 | B 并发 | L9 | efficiency-monitor `start_event_subscriber` 吞掉 JoinHandle | `crates/efficiency-monitor/src/lib.rs:226` | S |
| B-Min-3 | B 并发 | L6 | faae-router `spawn_decay_loop` 未返回 JoinHandle,fire-and-forget | `crates/faae-router/src/edsb.rs:315-327` | S |
| m-01 | C 性能 | L6 | KVBSR 候选工具向量 `tv.clone()` 每次路由 20-50 次深拷贝 | `crates/kvbsr-router/src/router.rs:336` | S |
| m-02 | C 性能 | L7 | PVL verifier `to_lowercase()` 每次验证分配新 String | `crates/pvl-layer/src/verifier.rs:95` | S |
| m-03 | C 性能 | L1 | nexus-core `snapshot_hash` 每次排序 + 逐条 JSON 序列化 | `crates/nexus-core/src/state.rs:88-95` | S |
| m-04 | C 性能 | L1 | model-router `list_by_cost/latency` 全量 clone + 排序 | `crates/model-router/src/registry.rs:81-96` | S |
| m-05 | C 性能 | L6 | OSA masks `active_set` 二次 clone Top-K ids | `crates/osa-coordinator/src/masks.rs:169-171` | XS |
| m-06 | C 性能 | L10 | csn-substitutor `target_vector.clone()` + `r.key().clone()` 每候选 | `crates/csn-substitutor/src/substitutor.rs:154,165` | S |
| Q-04 | D 质量 | L3 | LSCT 升降级阈值硬编码(PROMOTION/DEMOTION_THRESHOLD) | `crates/lsct-tiering/src/tiering/profile.rs:17,19` | S |
| Q-05 | D 质量 | L8 | DECB 溢出比例硬编码(OVERFLOW_WARN/DEGRADE/CRITICAL_RATIO) | `crates/decb-governor/src/overflow.rs:19,21,23` | S |
| Q-06 | D 质量 | L6 | FaaE 衰减间隔硬编码(DECAY_INTERVAL_SECS=300) | `crates/faae-router/src/edsb.rs:37` | XS |
| Q-07 | D 质量 | L2 | nmc-encoder 5 个 perceptor 全占位(Week 7/8 接入 ort ONNX) | `crates/nmc-encoder/src/perceptors/*.rs` | L(Week 9) |
| Q-08 | D 质量 | L5 | GSOE 4 处 TODO 待真实模型(Week 7 接入 MCP Mesh) | `crates/gsoe-evolution/src/policy/{mutation,grpo,fitness}.rs` | M(Week 9) |
| Q-09 | D 质量 | L8 | event-bus 集成 TODO 未完成(parliament/decb-governor) | `crates/parliament/src/ahirt.rs:496,570` + `decb-governor/src/error.rs:21` | S |
| E-MINOR-1 | E 测试 | L10 | chimera-cli 测试数(15)接近阈值,缺乏 proptest | `crates/chimera-cli/tests/cli.rs` | S |
| E-MINOR-2 | E 测试 | L7 | 跨周集成测试 SSRA + MLC 协作缺失 | `crates/ssra-fusion/tests/`、`mlc-engine/tests/` | S |
| E-MINOR-3 | E 测试 | L6 | test_scale_speedup_vs_full_scan 阈值降至 2.0×(理论 9×) | `crates/kvbsr-router/tests/scale.rs:166` | S(长期提升) |
| F-007 | F 安全 | L1 | AsaIntervention severity() 与 is_critical_alert_event 不一致 | `crates/event-bus/src/types.rs:1142-1151` | S |

> 其余 20 个 Minor 问题(E-MINOR-4~6、F-008~F-013、G-09~G-19)详见各维度报告问题清单。

---

## 5. 修复优先级与成本估算

| 优先级 | 问题数 | 预计修复成本 | 建议时机 | 修复目标 |
|--------|--------|------------|---------|---------|
| Critical | 11 | XS×5 + S×2 + M×4 = ~20-40 工时 | 立即(v1.0.1) | 100% 修复 |
| Major | 19 | XS×4 + S×8 + M×4 + L×3 = ~80-120 工时 | 本次(v1.0.1-v1.1.0) | ≥90% 修复 |
| Minor | 44 | XS×8 + S×22 + M×6 + L×4 = ~80-120 工时 | 本次/长期(v1.0.1-v1.2.0) | ≥70% 修复 |

### 5.1 修复批次建议

**批次 1 — 立即修复(v1.0.1,本周内)**:
- F-001 + G-05:BudgetExceeded severity 修复(1 行代码 + 1 行标记更新,XS)
- G-02 + G-03 + G-04 + G-12:文档数量与事件名修正(XS×4)
- G-07:event-bus lib.rs 事件数注释更新(XS)
- G-01:AETHER §6.2 层级标注修正(S)

**批次 2 — Critical 并发修复(v1.0.1,1-2 周)**:
- B-Crit-1 ~ B-Crit-4:faae-router 全链路重构(统一"快照 → 释放锁 → await"模式,M)
- B-Maj-1:csn-substitutor register 原子化(S)

**批次 3 — Critical 性能修复(v1.0.1,1-2 周)**:
- C-01:repo-wiki SQLite spawn_blocking 包装(S)
- C-02:scc-cache WAL spawn_blocking 包装(S)

**批次 4 — Major 安全加固(v1.0.1-v1.1.0,2-4 周)**:
- F-002:沙箱超时(S)
- F-004:mcp-mesh SSRF 校验(S)
- F-003 + F-005:沙箱资源限制 + gVisor 启用(M + L,Linux 优先)
- F-006:EventBus broadcast + mpsc 双通道(L)

**批次 5 — Major 测试补充(v1.0.1)**:
- E-MAJOR-1:decay-engine 测试补充至 ≥15 个(S)

**批次 6 — Major 性能优化(v1.1.0)**:
- M-01 + M-02:HCW entries 改为 `Vec<Arc<ContextEntry>>`(M)
- M-03:MLC list_all_arc() 变体(M)
- M-05:MLC L2 benchmark 4096 规模(S)
- M-04:10 个核心 crate benchmark 补充(L)

**批次 7 — Major 伪实现替换(v1.1.0+,依赖 Week 9 NMC ONNX)**:
- Q-01:MTPE 伪预测 → 真实模型推理(L,依赖 NMC ONNX)
- Q-03:RepoWiki 占位嵌入 → 真实 CLV 嵌入(M,依赖 NMC ONNX)
- Q-02:FaaE 伪随机 → rand crate(S)

**批次 8 — Minor 批量修复(v1.0.1-v1.2.0)**:
- 配置化(Q-04/Q-05/Q-06)、文档一致性(G-09~G-19)、测试补充(E-MINOR-1~6)、输入校验(F-008~F-010)等

---

## 6. 长期主义建议

### 6.1 治本:自动化校验机制(防止问题再生)

1. **依赖方向 CI lint**:建立 `cargo xtask check-deps` 子命令,基于 `layers.toml` 配置自动校验所有 crate 的 path 依赖方向,发现 L(N)→L(N+1) 立即拒绝合并。

2. **持锁跨 await CI lint**:在 workspace 级启用 `clippy::await_holding_lock = "deny"`,将持锁跨 await 检测前置到编译期(Rust 1.56+ 内置)。

3. **文档同步 CI job**:新增 `.github/workflows/docs-check.yml`,校验 crate 数量、事件名称、层级标注、ADR 完整性,防止"代码-文档"漂移。

4. **cargo-audit CI 集成**:在 CI 中加 cargo-audit step,每日定时运行 + PR 触发,自动创建 issue 跟踪新发现的 CVE。

5. **性能回归 CI 守护**:将关键 bench 指标(WAL p95、三层路由 p95、SSRA 5.64µs)写入 `perf-baseline.json`,CI 中回归 > 10% 告警。

### 6.2 架构演进:消除根因

6. **faae-router 锁结构重新设计**:当前 `Arc<RwLock<HashMap<ToolId, Arc<RwLock<ExpertProfile>>>>` 双层 RwLock + 内层 `last_used_at: RwLock<Instant>` 三层锁结构过于复杂。建议:外层改 `DashMap<ToolId, Arc<ExpertProfile>>`(分片锁,无锁读),`ExpertProfile` 内部计数用 `AtomicU64`,`last_used_at` 改 `AtomicU64`(Instant 序列化为 u64 纳秒)。此重构可彻底消除 B-Crit-1 ~ B-Crit-4 的根因。

7. **SQLite 访问层统一化**:在 nexus-core 或新建 `sqlite-util` crate 中提供统一 `AsyncSqlitePool` 抽象,封装 `r2d2` 连接池 + `spawn_blocking`,所有 crate 通过统一 async API 访问 SQLite,消除 spawn_blocking 遗漏风险。

8. **Arc 共享存储改造**:逐步将读多写少的大对象存储从值类型改为 `Arc<T>`(HCW: `Vec<Arc<ContextEntry>>`、MLC: `HashMap<MemoryId, Arc<MemoryEntry>>`、KVBSR: `DashMap<ToolId, Arc<ToolVector>>`),彻底消除热路径 clone。

9. **EventBus 双通道演进**:实现 broadcast + mpsc 双通道:Critical 事件走独立 mpsc 通道(点对点投递,不丢失),Normal 事件走 broadcast(多订阅者广播)。参考 `backpressure.rs:34-42` 的 `CriticalMpsc` 策略变体设计。

### 6.3 测试体系深化

10. **proptest 下沉**:为 L1-L4 下层 crate(nexus-core、event-bus、model-router、decay-engine)引入 proptest,验证核心不变量(CLV 维度守恒、EventBus 消息顺序、衰减单调性)。

11. **变异测试**:引入 `cargo-mutants` 验证测试有效性,确保测试能捕获代码变异(防止"绿色但无效"的测试)。

12. **loom 竞态测试**:在 Week 9+ 引入 `loom` 竞态测试框架,对 faae-router 与 csn-substitutor 进行模型化并发测试。

13. **测试覆盖率工具**:引入 `cargo-tarpaulin` 或 `cargo-llvm-cov` 量化行覆盖率,目标 ≥ 80%。

### 6.4 文档治理

14. **文档权威性层级**:明确 L0(代码本身)> L1(CODE_WIKI + docs/architecture)> L2(AETHER 历史快照)。在 AETHER §1 添加"⚠️ 本文档为初始设计文档,当前真相以 CODE_WIKI.md 与代码为准"声明。

15. **project_memory 清理节奏**:每月 1 日清理 project_memory.md,将已 ✅ FIXED 超过 4 周的条目移至 `project_memory_archive.md`,保持当前文件 ≤ 50 行。每个 ✅ FIXED 标记必须附带代码文件位置。

16. **事件命名公约**:所有事件变体采用 `<CrateName><Action>` 格式(如 `BudgetAdjusted` / `DpoPairGenerated`),避免缩写前缀混淆。新增事件时必须同步更新 event-bus/types.rs 四处 + 发布者 lib.rs + CHANGELOG + CODE_WIKI §5.2。

### 6.5 安全深化

17. **零信任深化**:将 `path_util` 等共享工具增加自我保护(不依赖 SecCore 的纵深防御),每个系统边界组件独立校验输入。

18. **fuzzing 扩展**:`fuzz/` 目录已有 3 个 fuzz target,建议扩展到 SecCore 沙箱、Decay Engine、Event Bus 等核心组件,持续运行以发现未知漏洞。

19. **供应链安全**:引入 `cargo-vet` 或 Sigstore 签名验证依赖完整性,防止依赖被篡改。移除未使用的 workspace 依赖(wasmtime/reqwest/axum/sqlite-vec)以减少供应链攻击面。

20. **威胁建模**:建立 STRIDE 威胁模型,定期复盘新功能的安全风险,形成"威胁建模 → 安全设计 → 渗透测试 → 复盘"闭环。

---

## 7. 附录:审计团队与分工

| 角色 | 维度 | 报告路径 | 问题数(C/M/m) |
|------|------|---------|---------------|
| 首席架构师 | A 架构一致性 | `docs/audit/dimension_a_architecture.md` | 0/0/5 |
| 并发安全专家 | B 并发安全 | `docs/audit/dimension_b_concurrency.md` | 4/1/3 |
| 性能优化专家 | C 性能瓶颈 | `docs/audit/dimension_c_performance.md` | 2/5/6 |
| 代码质量专家 | D 代码质量 | `docs/audit/dimension_d_quality.md` | 0/3/6 |
| 测试覆盖专家 | E 测试覆盖 | `docs/audit/dimension_e_testing.md` | 0/1/6 |
| 安全审计专家 | F 安全 | `docs/audit/dimension_f_security.md` | 1/5/7 |
| 文档同步专家 | G 文档同步 | `docs/audit/dimension_g_documentation.md` | 4/4/11 |
| 独立审计员 | H 历史问题追踪 | `docs/audit/historical_issues_tracking.md` | 19 项核验 |
| **首席审计汇总专家** | **全局汇总** | **`docs/audit/week1-8_global_audit_report.md`** | **11/19/44** |

### 审计完整性声明

- 本报告所有结论均引用具体代码位置(文件:行号),可由读者独立复现验证。
- 审计过程未修改任何代码,仅做只读分析。
- 问题数量以各维度报告的"问题清单"表格为准(部分维度报告摘要统计与表格存在轻微不一致,已在 §1.3 注明)。
- 修复成本为基于问题复杂度的估算,实际成本可能因上下文而异。
- 建议结合 `cargo check --workspace` + `cargo clippy --workspace -- -D warnings` + `cargo test --workspace` 验证修复效果。

---

> **报告生成**:2026-06-28
> **审计性质**:只读分析,证据驱动
> **下一步**:按 §5 修复批次建议,从批次 1(立即修复 XS 级文档/标记问题)开始推进
