# Week 8 生产化 + 安全 + 发布 + 文档 开发方案 Spec

> **本周目标**:完成 NEXUS-OMEGA 项目最终生产化阶段——补齐剩余 3/34 crate 至 100% 覆盖、闭合 Week 7 进行中的 Task 5/6/9/10、执行 OWASP Top 10 渗透测试与 10000 输入模糊测试、生成 5 平台 binary 与 Docker 镜像、完善全量文档并跑通全量 E2E,于 Day 56 发布 `v1.0.0-omega` 生产就绪版本,8 周推进计划收尾。

---

## 1. 现状分析与基线

### 1.1 项目当前进度(截至 2026-06-27,Week 7 验收通过)

| 维度 | 当前状态 | 数据来源 |
|------|---------|---------|
| 已完成周次 | Week 1-7 全部验收通过 | Week 7 验收报告(CHANGELOG.md L8-125) |
| 全量测试通过 | 2716 passed / 0 failed(Week 1-6: 2378 + Week 7: 338) | CHANGELOG.md L97 |
| clippy 警告 | 0 warnings(`--all-targets -- -D warnings`) | CHANGELOG.md L103 |
| `#![forbid(unsafe_code)]` 覆盖 | 40/40 crate(100%) | Week 6 验收基线 |
| 安全免疫率 | 100%(120 载荷:100 旧 + 20 Week6 新) | Week 6 验收基线 |
| crate 覆盖率 | 31/34(91.2%) | CHANGELOG.md L10 |
| 全量构建 | `cargo build --workspace --release` 通过 | CHANGELOG.md L105 |
| 待收尾 Task | Week 7 Task 5(性能基准报告)/ Task 6(37 模块集成 + 压测)/ Task 9(性能调优)/ Task 10(端到端验收) | CHANGELOG.md L115 |

### 1.2 Week 1-7 验收标准对比分析报告

| 周次 | 主题 | 核心验收指标 | 关键交付 | 滚动到 Week 8 的约束 |
|------|------|-------------|---------|--------------------|
| Week 1 | L0-L1 基础设施 | Event Bus · SecCore · Decay · QEEP · CLI 入口 | 5 crate 完整实现 | EventBus API 稳定基线不可破坏 |
| Week 2 | L9+L5+L1 | Quest · Wiki · Model Router · CACR | Quest 分解 + Wiki 沉淀 | Quest 生命周期作为 Week 8 E2E 主线 |
| Week 3 | L5+L6 | MLC · HCW · CMT · OSA · KVBSR | 四级记忆 + 全维稀疏 | CMT/OSA 接口冻结 |
| Week 4 | L6+L7 | GEA · GQEP · PVL · MTPE · SCC · EDSB | 执行链路完整 | 执行链路事件流作为 Week 8 集成测试输入 |
| Week 5 | L8+L4+L3 | Parliament · ASA · AHIRT · TTG · DECB | 7700 行 + 2023 测试 | Critical 事件被 Week 8 安全测试验证 |
| Week 6 | L2+L10 | SSRA · LSCT · GSOE · NMC · CHTC | 406 测试 + SSRA 5.64μs | 5 个 Week 6 事件须在 Week 8 全量 E2E 中验证链路完整性 |
| Week 7 | L10+L6+L9 | MCP · CSN · SESA · Efficiency Monitor | 338 测试 + 4 性能基准达标 | Task 5/6/9/10 进行中,Week 8 须闭合 |

**对比分析结论**:
- Week 1-7 累计完成 31 crate,Week 8 首要任务是补齐剩余 3 crate 至 100% 覆盖率,并闭合 Week 7 进行中任务
- Week 8 是 8 周推进计划中唯一"零新功能 + 全量收尾 + 对外发布"的周次,**集成与发布双重压力最高**
- Week 5-7 引入的 Critical 事件链(SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded)在 Week 8 须通过 OWASP Top 10 渗透测试验证端到端防御能力
- 首次引入 cargo-audit / cargo-fuzz / Docker / 跨平台 binary 构建工具链,需评估对 `#![forbid(unsafe_code)]` 的影响
- Week 7 已建立 4 个性能基准基线(MCP p95 ≤ 100ms / CSN p95 ≤ 30ms / SESA p95 ≤ 5ms 稀疏度 < 40% / Monitor ≤ 1ms),Week 8 须在此基线上完成 SIMD + WAL 性能调优,目标路由延迟 ≤ 2ms

### 1.3 反馈跟踪矩阵(结构化)

| 反馈来源 | 内容描述 | 优先级 | 处理状态 | 对应 Task |
|---------|---------|--------|---------|----------|
| Week 7 验收报告 | Task 5/6/9/10 进行中,须 Week 8 闭合 | Must | 待处理 | Task 1 性能调优收尾 |
| Week 7 验收报告(架构师) | MCP Mesh 2PC 为占位实现,Week 8 后考虑接入真实跨进程通信 | Could | 待处理 | Task 6 已知问题归档 |
| Week 7 验收报告(集成专家) | CSN 替代候选注册需手动注册,Week 8 后考虑自动发现 | Could | 待处理 | Task 6 已知问题归档 |
| Week 7 验收报告(性能专家) | SESA 路由延迟须 SIMD 优化达到 ≤ 2ms | Must | 待处理 | Task 1 SIMD 优化 |
| Week 7 验收报告(SRE) | WAL 真实持久化已实现(SqliteWal),须压测验证崩溃恢复 | Must | 待处理 | Task 2 WAL 压测 |
| 产品建议 | 跨平台发布须覆盖 Windows/Linux/macOS × x86_64/aarch64 5 平台 | Must | 待处理 | Task 4 跨平台构建 |
| SRE 反馈 | Docker 镜像须 < 100MB,基于 distroless | Must | 待处理 | Task 4 Docker 镜像 |
| 安全反馈 | OWASP Top 10 渗透测试须 100% 通过 | Must | 待处理 | Task 3 渗透测试 |
| 安全反馈 | 模糊测试 10000 随机输入无崩溃 | Must | 待处理 | Task 3 模糊测试 |
| 安全反馈 | cargo-audit 依赖扫描须无高危漏洞 | Must | 待处理 | Task 3 依赖审计 |
| 客户反馈 | README + API 文档须完整覆盖 37 模块 | Must | 待处理 | Task 5 文档完善 |
| 测试反馈 | 全量 E2E 须 100% 通过(含 Quest 生命周期 + 崩溃恢复) | Must | 待处理 | Task 6 最终验收 |
| 维护反馈 | cargo doc 零 warnings(Week 7 已修复,Week 8 须保持) | Should | 待处理 | Task 5 文档构建 |
| DevOps 反馈 | CI/CD 流水线须自动化构建 + 测试 + 发布 | Should | 待处理 | Task 4 CI/CD |

### 1.4 文档摘要与关键约束清单

**关键约束**:
1. **技术栈**:Rust 2021 edition · Tokio async · Workspace × 34 crates(原规划 37,实际 34)
2. **架构规范**:10 层架构 L1→L10,依赖铁律(L(N) → L(N-1) 允许,L(N) → L(N+1) 禁止,跨层走 EventBus)
3. **版本**:1.0.0-omega
4. **`#![forbid(unsafe_code)]`**:40/40 crate 强制,Week 8 引入的 cargo-audit/cargo-fuzz/Docker 工具链须验证不破坏此约束
5. **性能指标**:路由延迟 ≤ 2ms(SIMD + WAL)、Quest E2E p95 ≤ 500ms、并发无死锁(1000 次压测)
6. **安全指标**:OWASP Top 10 100% 通过、模糊测试 10000 输入无崩溃、cargo-audit 无高危
7. **发布指标**:5 平台 binary + Docker 镜像(< 100MB)
8. **文档指标**:README + API 文档完整覆盖,cargo doc 零 warnings
9. **Windows 11 + PowerShell 环境**:Rust 工具链在 `D:\Chimera CLI\.toolchain\`,GNU 工具链
10. **磁盘空间约束**:C: 盘空间紧张,构建时设置 `$env:CARGO_TARGET_DIR = 'D:\Chimera CLI\target'`

### 1.5 项目现状分析报告

**当前进度**:
- 完成 91.2% crate 实现(31/34),Week 8 须补齐剩余 3 crate 至 100%
- 完成 7/8 周推进计划,Week 8 是收尾周
- 2716 测试全绿,clippy 零警告,forbid(unsafe_code) 全覆盖

**已完成功能**:
- L1 Core:nexus-core / event-bus / model-router
- L2 Memory:nmc-encoder / hcw-window / mlc-engine
- L3 Storage:scc-cache(含 WAL)/ lsct-tiering / cmt-tiering
- L4 Security:seccore / qeep-protocol / decay-engine
- L5 Knowledge:repo-wiki / gsoe-evolution / auto-dpo
- L6 Router:osa-coordinator / kvbsr-router / faae-router / sesa-router
- L7 Execution:pvl-layer / gqep-executor / mtpe-executor / ssra-fusion
- L8 Parliament:parliament / acb-governor / decb-governor
- L9 Quest:quest-engine / gea-activator / efficiency-monitor
- L10 Interface:mcp-mesh / csn-substitutor / chtc-bridge / chimera-tui / chimera-cli

**存在问题**:
1. Week 7 Task 5/6/9/10 进行中,须 Week 8 闭合
2. 性能调优(SIMD + WAL)未完成,路由延迟须验证 ≤ 2ms
3. 全量集成测试 + 1000 次压测未完成
4. 端到端验收未完成

**潜在风险**:
1. **跨平台编译风险**:Windows 环境下交叉编译到 Linux/macOS/aarch64 可能遇到链接器缺失
2. **Docker 镜像体积风险**:Rust binary + 依赖可能导致镜像 > 100MB
3. **cargo-fuzz 与 forbid(unsafe_code) 兼容性**:cargo-fuzz 内部使用 unsafe,需验证仅在 fuzzing target 中启用
4. **OWASP Top 10 测试覆盖风险**:SecCore 沙箱覆盖范围若不足,可能暴露注入/越权漏洞
5. **性能调优与稳定性冲突**:SIMD 优化若引入 unsafe,可能破坏 forbid(unsafe_code) 约束
6. **磁盘空间**:C: 盘紧张,大规模构建可能失败

---

## 2. 团队组建与职责分配

### 2.1 团队规模与核心能力要求

组建 **6 人精英专家级子智能体协同开发团队**(每个子代理具备 10+ 年行业经验),按 10 层架构与 Week 8 收尾任务分配:

| 编号 | 角色 | 核心能力 | 经验证明 |
|------|------|---------|---------|
| E1 | 首席架构师(Lead Architect) | 10 层架构治理 · 依赖方向性审计 · ADR 决策 · 跨层集成 | 累计 7 周 ADR 决策记录 · Week 5 L8 rehoming · Week 6 LSCT 策略层设计 |
| E2 | 性能优化专家(Performance Engineer) | SIMD 优化 · criterion 基准 · 内存分析 · Top-K 算法调优 | Week 6 SSRA 5.64μs(3500× 余量)· Week 7 SESA 256-bit 掩码 |
| E3 | 安全工程师(Security Engineer) | OWASP Top 10 · 模糊测试 · cargo-audit · 沙箱渗透 | Week 4 SecCore 沙箱 · Week 5 AHIRT 红队 · 100% 免疫率 |
| E4 | DevOps 工程师(Release Engineer) | cross-rs · Docker · distroless · CI/CD · GitHub Actions | Week 7 Grafana 仪表盘 · Prometheus /metrics 集成 |
| E5 | 测试工程师(QA Engineer) | E2E 测试 · 压力测试 · 集成测试矩阵 · 燃尽图 | Week 7 338 测试全绿 · 1000 次压测设计 |
| E6 | 技术文档工程师(Tech Writer) | README · API 文档 · cargo doc · CODE_WIKI · CHANGELOG | Week 1-7 全量文档同步 · CODE_WIKI 5 次重建 |

### 2.2 RACI 责任矩阵

| Task \ 角色 | E1 架构师 | E2 性能 | E3 安全 | E4 DevOps | E5 测试 | E6 文档 |
|------------|----------|---------|---------|-----------|---------|---------|
| Task 1 性能调优收尾(SIMD + WAL) | C | **R/A** | I | C | C | I |
| Task 2 3 crate 补齐(若适用) | **R/A** | C | C | I | C | I |
| Task 3 OWASP + 模糊 + cargo-audit | C | I | **R/A** | C | C | I |
| Task 4 跨平台发布 + Docker + CI/CD | C | I | C | **R/A** | C | I |
| Task 5 文档完善 + cargo doc | C | I | I | I | I | **R/A** |
| Task 6 全量 E2E + 最终验收 | C | C | C | C | **R/A** | C |
| Task 7 文档同步 + v1.0.0-omega 发布 | C | I | I | C | I | **R/A** |

**RACI 图例**:**R** = Responsible(执行) · **A** = Accountable(负责) · **C** = Consulted(咨询) · **I** = Informed(知情)

### 2.3 团队协作机制与沟通流程

| 机制 | 频率 | 时长 | 目的 |
|------|------|------|------|
| 每日站会 | 每日 09:00 | 15 分钟 | 同步昨日进度 + 今日计划 + 阻塞问题 |
| 周例会 | 周一 14:00 | 60 分钟 | 回顾上周 + 规划本周 + 风险评估 |
| 紧急响应 | 触发时 | 2 小时响应 / 24 小时方案 | Critical 缺陷 / 编译阻塞 / 安全漏洞 |
| 代码评审 | PR 提交时 | 4 小时内 | peer review,通过率 100% |
| 架构评审 | ADR 提案时 | 1 工作日内 | Lead Architect 决策 |
| 燃尽图同步 | 每日 18:00 | 自动化 | TodoWrite 状态同步 + 偏差分析 |

---

## 3. 第八周开发范围与目标

### 3.1 任务 MoSCoW 优先级划分

| 优先级 | Task | 判定依据 |
|--------|------|---------|
| **Must** | Task 1 性能调优收尾(SIMD + WAL + 路由 ≤ 2ms) | Week 7 进行中,验收硬指标 |
| **Must** | Task 3 安全三件套(OWASP + 模糊 + cargo-audit) | Day 50-52 硬指标,生产化必须 |
| **Must** | Task 4 跨平台发布(5 平台 binary + Docker) | Day 53-54 硬指标,发布必须 |
| **Must** | Task 6 全量 E2E + 最终验收 | Day 56 硬指标,8 周计划收尾 |
| **Must** | Task 7 文档同步 + v1.0.0-omega 发布 | 发布必须,版本号 1.0.0-omega |
| **Should** | Task 5 文档完善(README + API + cargo doc) | Day 55,客户反馈 Must |
| **Could** | Task 2 3 crate 补齐(若 31/34 状态仍存在) | 视实际审计结果,可能已闭合 |
| **Won't**(本周不做) | MCP Mesh 2PC 真实跨进程实现 | Week 7 已知问题,Week 8 后做 |
| **Won't**(本周不做) | CSN 替代候选自动发现 | Week 7 已知问题,Week 8 后做 |
| **Won't**(本周不做) | GSOE 接入真实强化学习模型 | 长期演进项,不属于生产化范畴 |

### 3.2 任务交付物、验收标准与负责人

#### Task 1:性能调优收尾(SIMD + WAL,Week 7 Task 9 闭合)

**交付物**:
- SIMD 加速的路由查询实现(`crates/sesa-router` 或 `kvbsr-router`,若适用)
- WAL 真实持久化压测报告(`crates/scc-cache/src/wal.rs` 已实现 SqliteWal,须崩溃恢复压测)
- 三层路由(KVBSR+SESA+FaaE)联调 p95 报告(Week 7 已建立 three_layer_routing 基准)
- 性能调优报告(对比 Week 7 基线)

**验收标准**:
- 路由延迟 p95 ≤ 2ms(SIMD + WAL 启用后)
- WAL 崩溃恢复压测 1000 次无数据丢失
- 三层路由基准 `cargo bench -p sesa-router --bench three_layer_routing -- --ignored` 输出 p95 ≤ 2ms
- `cargo test --workspace` 全绿,无性能回归
- `#![forbid(unsafe_code)]` 保持 40/40 覆盖(SIMD 若需 unsafe,改用 std 内建 SIMD 或 autovectorization)

**负责人**:E2 性能专家(R/A)· E1 架构师(C)· E5 测试(C)

#### Task 2:3 crate 补齐审计(条件性)

**交付物**:
- crate 覆盖率审计报告(确认 31/34 还是 34/34)
- 若仍有 3 crate 未实现:补齐 lib.rs + 模块骨架 + 基础测试
- 若已 34/34:本任务标记为已完成,跳过

**验收标准**:
- `cargo check --workspace` 通过
- crate 覆盖率达 100%(34/34)
- `#![forbid(unsafe_code)]` 全覆盖

**负责人**:E1 架构师(R/A)· E5 测试(C)

#### Task 3:安全三件套(OWASP + 模糊 + cargo-audit)

**交付物**:
- `tests/security/owasp_top10.rs` — OWASP Top 10 渗透测试套件(A01-A10)
- `tests/fuzz/` — cargo-fuzz 模糊测试 target(10000 输入)
- `cargo audit` 报告 — 依赖漏洞扫描
- 安全测试报告(渗透 + 模糊 + 审计三维度)

**验收标准**:
- OWASP Top 10 10 项 100% 通过(SecCore 沙箱拦截注入/越权/泄露/CSRF/SSRF 等)
- `cargo fuzz run <target>` 10000 次输入无崩溃(可执行 60s 快速验证)
- `cargo audit` 无 High/Critical 漏洞(若存在,升级依赖并复测)
- 模糊测试 target 与 `#![forbid(unsafe_code)]` 兼容(fuzzing target 单独 crate,不破坏主 crate)
- 安全测试报告归档至 `docs/security/week8_security_report.md`

**负责人**:E3 安全工程师(R/A)· E1 架构师(C)· E5 测试(C)

#### Task 4:跨平台发布 + Docker + CI/CD

**交付物**:
- 5 平台 binary:Windows x86_64 / Linux x86_64 / Linux aarch64 / macOS x86_64 / macOS aarch64
- `Dockerfile` — 基于 distroless,镜像 < 100MB
- `.github/workflows/release.yml` — GitHub Actions CI/CD 流水线(自动化构建 + 测试 + 发布)
- `docs/release/week8_release_guide.md` — 发布指南

**验收标准**:
- 5 平台 binary 全部生成,文件大小合理(< 50MB each)
- Docker 镜像构建成功,`docker run` 可执行 `chimera --version`
- Docker 镜像 < 100MB(distroless 基础镜像)
- CI/CD 流水线在 push tag `v1.0.0-omega` 时自动触发构建 + 测试 + 发布
- `#![forbid(unsafe_code)]` 在所有平台保持 40/40 覆盖
- cross-rs 或 cargo-zigbuild 工具链验证(Windows 主机交叉编译)

**负责人**:E4 DevOps(R/A)· E1 架构师(C)· E3 安全(C)

#### Task 5:文档完善(README + API + cargo doc)

**交付物**:
- `README.md` — 项目总览、快速开始、架构图、37 模块索引、性能指标、安全特性
- `docs/api/` — API 参考文档(自动从 cargo doc 生成)
- `cargo doc --workspace --no-deps` 零 warnings(Week 7 已修复,Week 8 保持)
- `CODE_WIKI.md` — Week 8 章节新增(发布 + 安全 + 性能调优)
- `CHANGELOG.md` — Week 8 章节新增
- `docs/architecture/` — 架构文档整理(10 层架构图、数据流图、ADR 索引)

**验收标准**:
- README.md 覆盖项目全景,新用户可在 10 分钟内启动
- cargo doc 零 warnings,HTML 文档可生成
- CODE_WIKI.md 与实现 100% 同步(34 crate 全覆盖)
- CHANGELOG.md Week 8 章节完整
- 文档评审通过率 100%(E6 自审 + E1 复核)

**负责人**:E6 文档工程师(R/A)· E1 架构师(C)

#### Task 6:全量 E2E + 最终验收(Day 56)

**交付物**:
- `tests/e2e/quest_lifecycle.rs` — Quest 全生命周期 E2E(创建 → 分解 → 执行 → 崩溃 → 恢复 → 完成)
- `tests/e2e/full_integration.rs` — 37 模块全量集成测试(Week 7 Task 6 闭合)
- `tests/e2e/stress_test.rs` — 1000 次压测无内存泄漏(Week 7 Task 6 闭合)
- `tests/e2e/week8_final_acceptance.rs` — Week 8 最终验收测试
- 最终验收报告(8 周推进计划全部验收项核对)

**验收标准**:
- 全量 E2E 100% 通过(Quest 生命周期 + 37 模块集成 + 崩溃恢复)
- 1000 次压测无内存泄漏(用 `valgrind` 或 `jemalloc` 统计,Windows 用 `heaptrack` 替代或仅验证无 panic)
- `cargo test --workspace` 全绿(预计 2716 + Week 8 新增 ≥ 100,总计 ≥ 2800)
- `cargo check --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo build --workspace --release` 三连通过
- 8 周推进计划全部 Day 1-56 验收项核对通过
- 最终验收报告归档至 `docs/acceptance/week8_final_acceptance_report.md`

**负责人**:E5 测试工程师(R/A)· E1 架构师(C)· E2 性能(C)· E3 安全(C)

#### Task 7:文档同步 + v1.0.0-omega 发布

**交付物**:
- `CHANGELOG.md` Week 8 章节
- `project_memory.md` Week 8 经验教训
- `.trae/specs/week8-production-release-hardening/checklist.md` 全部 ✅
- Git tag `v1.0.0-omega`
- Release notes(`docs/release/v1.0.0-omega_release_notes.md`)

**验收标准**:
- 所有文档同步完成(CODE_WIKI / CHANGELOG / project_memory / spec checklist)
- Git tag `v1.0.0-omega` 创建并推送
- Release notes 完整(8 周总结 + 性能指标 + 安全特性 + 已知限制)
- 8 周推进计划正式收尾

**负责人**:E6 文档工程师(R/A)· E1 架构师(C)

---

## 4. 风险评估与缓释策略

### 4.1 风险登记表

| 风险编号 | 风险描述 | 可能性 | 影响程度 | 优先级 | 应对措施 |
|---------|---------|--------|---------|--------|---------|
| R1 | 跨平台交叉编译失败(Windows 主机编译 Linux/macOS binary 链接器缺失) | 高 | 高 | Must | 使用 cargo-zigbuild 或 cross-rs,Docker 容器内编译;CI/CD 在 GitHub Actions 多平台 runner |
| R2 | Docker 镜像 > 100MB(Rust binary 体积大) | 中 | 中 | Should | distroless 基础镜像 + `strip` binary + `panic = "abort"` + `opt-level = "z"` |
| R3 | cargo-fuzz 与 forbid(unsafe_code) 冲突 | 中 | 高 | Must | fuzzing target 单独 crate(`fuzz/`),不污染主 crate;cargo-fuzz 内部 unsafe 不传播 |
| R4 | OWASP Top 10 渗透测试发现 SecCore 沙箱漏洞 | 中 | 高 | Must | Week 4-7 已 100% 免疫 120 载荷,Week 8 扩展载荷;发现漏洞立即修复并补测试 |
| R5 | SIMD 优化需 unsafe,破坏 forbid(unsafe_code) | 中 | 高 | Must | 优先使用 std 内建 SIMD(`std::simd` stable)或 autovectorization;若必须 unsafe,记录 ADR 评估 |
| R6 | C: 盘磁盘空间不足导致构建失败 | 高 | 中 | Should | 强制 `$env:CARGO_TARGET_DIR = 'D:\Chimera CLI\target'`;CI/CD 在 GitHub Actions runner 不受此限制 |
| R7 | 1000 次压测内存泄漏(Rust 理论上无,但 DashMap/EventBus 可能累积) | 低 | 高 | Must | 压测中监控 RSS;若泄漏用 `cargo instruments` 或 `heaptrack` 定位 |
| R8 | 31/34 crate 覆盖率审计发现实际仍缺 3 crate | 中 | 中 | Should | Task 2 条件性补齐;若 3 crate 为规划外,记录 ADR 决策 |
| R9 | Week 7 Task 5/6/9/10 进行中项无法在 Week 8 闭合 | 低 | 高 | Must | Task 1 优先处理,若 Task 6 压测阻塞,降级为 100 次压测 + 标注限制 |
| R10 | cargo audit 发现 High/Critical 漏洞依赖 | 中 | 高 | Must | 立即升级依赖;若升级引入 breaking change,记录 ADR;无法升级则记录已知限制 |
| R11 | CI/CD GitHub Actions 多平台 runner 配额不足 | 低 | 中 | Should | 使用 self-hosted runner 备选;或仅在 release tag 时触发跨平台构建 |
| R12 | cargo doc 在 Week 8 新增代码后出现 warnings | 中 | 低 | Should | E6 在 Task 5 同步修复;CI 中加入 `cargo doc --no-deps` 检查 |

### 4.2 关键路径法(CPM)分析

**关键路径**(最长依赖链):
```
Task 1 性能调优 → Task 6 全量 E2E(含压测) → Task 7 文档同步 + 发布
                    ↑
Task 3 安全测试 ────┘
                    ↑
Task 4 跨平台发布 ──┘(发布依赖测试通过)
```

**关键路径总长**:Task 1(2 天) + Task 3(2 天,可并行) + Task 6(1.5 天) + Task 7(0.5 天) = **6 天**(Day 50-55,Day 56 缓冲 + 最终验收)

**潜在瓶颈**:
1. Task 1 SIMD 优化若引入 unsafe,需 ADR 评审,可能阻塞 1-2 天
2. Task 3 cargo-fuzz 10000 输入若发现崩溃,定位 + 修复可能阻塞 1 天
3. Task 4 跨平台编译若链接器缺失,需切换工具链,可能阻塞 1 天

**并行化机会**:
- Task 1 性能 / Task 3 安全 / Task 4 发布 / Task 5 文档 可 4 路并行(Day 50-53)
- Task 2 条件性补齐 可与 Task 1 并行
- Task 6 最终验收 必须在 Task 1/3/4 完成后串行(Day 54-55)

---

## 5. 甘特图与时间计划

### 5.1 甘特图(Day 50-56,7 天)

```
Day 50 | Day 51 | Day 52 | Day 53 | Day 54 | Day 55 | Day 56
=======+========+========+========+========+========+========
[Task 1 性能调优 ████████████████████]
[Task 2 crate 补齐 ██] (条件性)
[Task 3 安全测试     ████████████████████]
[Task 4 跨平台发布           ████████████████████]
[Task 5 文档完善                   ████████████████]
[Task 6 全量 E2E                          ████████████]
[Task 7 发布同步                                       ██]
                                                       ↑
                                                  v1.0.0-omega
```

### 5.2 精确时间节点

| Task | 开始时间 | 结束时间 | 工时(人日) | 所需资源 |
|------|---------|---------|------------|---------|
| Task 1 性能调优 | Day 50 09:00 | Day 51 18:00 | 2.0 | E2 + E1 评审 |
| Task 2 crate 补齐 | Day 50 09:00 | Day 50 18:00 | 0.5(条件) | E1 |
| Task 3 安全测试 | Day 50 09:00 | Day 52 18:00 | 2.0 | E3 + E5 |
| Task 4 跨平台发布 | Day 52 09:00 | Day 54 18:00 | 2.0 | E4 + GitHub Actions |
| Task 5 文档完善 | Day 53 09:00 | Day 55 18:00 | 1.5 | E6 |
| Task 6 全量 E2E | Day 54 09:00 | Day 55 18:00 | 1.5 | E5 + E1/E2/E3 协同 |
| Task 7 发布同步 | Day 56 09:00 | Day 56 18:00 | 0.5 | E6 + E1 |
| **合计** | | | **10.0 人日** | 6 人协同 |

---

## 6. 质量验收基准

### 6.1 功能测试通过率

| 指标 | 目标 | 测量方法 |
|------|------|---------|
| 单元测试通过率 | 100% | `cargo test --workspace` |
| 集成测试通过率 | 100% | `cargo test --workspace --test '*'` |
| E2E 测试通过率 | 100% | `cargo test --test e2e_*` |
| 总测试数 | ≥ 2800 | Week 7 基线 2716 + Week 8 新增 ≥ 100 |
| 压测通过率 | 1000 次无 panic/泄漏 | `tests/e2e/stress_test.rs` |

### 6.2 代码质量评分

| 指标 | 目标 | 测量方法 |
|------|------|---------|
| clippy 警告 | 0 | `cargo clippy --workspace --all-targets -- -D warnings` |
| `#![forbid(unsafe_code)]` 覆盖 | 40/40(100%) | 全 crate 顶部声明扫描 |
| 单函数行数 | ≤ 200 行 | 架构红线 §6 |
| 代码复用率 | ≥ 60% | 主观评估 + DRY 原则 |
| 注释覆盖率 | ≥ 80%(关键 WHY 注释) | 关键模块抽样 |
| cargo doc warnings | 0 | `cargo doc --workspace --no-deps` |

### 6.3 性能指标

| 指标 | 目标 | 基线(Week 7) | 测量方法 |
|------|------|-------------|---------|
| 路由延迟 p95 | ≤ 2ms | 待测(SIMD + WAL 后) | `cargo bench -p sesa-router --bench three_layer_routing` |
| MCP Mesh 5 服务器事务 p95 | ≤ 100ms | ≤ 100ms ✓ | `cargo bench -p mcp-mesh` |
| CSN 单次替代查询 p95 | ≤ 30ms | ≤ 30ms ✓ | `cargo bench -p csn-substitutor` |
| SESA 256 专家激活 p95 | ≤ 5ms | ≤ 5ms ✓ | `cargo bench -p sesa-router` |
| SESA 稀疏度 | < 40% | 0.3984375 ✓ | 单元测试断言 |
| efficiency-monitor 采集 | ≤ 1ms/样本 | ≤ 1ms ✓ | `cargo bench -p efficiency-monitor` |
| Quest E2E p95 | ≤ 500ms | 待测 | `tests/e2e/quest_lifecycle.rs` |
| 1000 次压测 | 0 死锁/泄漏 | 待测 | `tests/e2e/stress_test.rs` |

### 6.4 安全指标

| 指标 | 目标 | 测量方法 |
|------|------|---------|
| OWASP Top 10 | 100% 通过 | `tests/security/owasp_top10.rs` |
| 模糊测试 | 10000 输入无崩溃 | `cargo fuzz run <target>` |
| cargo audit | 无 High/Critical | `cargo audit` |
| 安全免疫率 | 100%(扩展载荷) | Week 6 基线 120 载荷 + Week 8 扩展 |

### 6.5 发布指标

| 指标 | 目标 | 测量方法 |
|------|------|---------|
| 跨平台 binary | 5 平台 | 文件存在性检查 |
| Docker 镜像 | < 100MB | `docker images` 检查 |
| CI/CD 流水线 | 自动化 | GitHub Actions 触发 |
| Git tag | v1.0.0-omega | `git tag` 验证 |

---

## 7. 执行原则与要求

### 7.1 任务依赖关系图

```
Task 2(条件性)─┐
               ├─→ Task 6(全量 E2E)─→ Task 7(发布)
Task 1(性能)──┤                          ↑
               │                          │
Task 3(安全)──┼──────────────────────────┤
               │                          │
Task 4(发布)──┼──────────────────────────┘
               │
Task 5(文档)──┘(并行,不阻塞)
```

### 7.2 长期主义原则

1. **SOLID 原则**:Week 8 新增代码遵循单一职责 / 开闭原则 / 里氏替换 / 接口隔离 / 依赖倒置
2. **DRY 原则**:跨 crate 共享逻辑通过 workspace 依赖,不重复实现
3. **YAGNI 原则**:不引入 Week 8 范围外的功能(MCP 2PC 真实跨进程 / CSN 自动发现 / GSOE 强化学习均归 Won't)
4. **可维护性**:所有新增代码须有 WHY 注释(架构决策、变通方案、反直觉行为)
5. **可扩展性**:发布流程脚本化、可复用,为后续版本发布奠定基础

### 7.3 资源使用监控

| 资源 | 监控指标 | 阈值 | 应对 |
|------|---------|------|------|
| 磁盘空间 | C: 盘剩余空间 | < 5GB | 强制 CARGO_TARGET_DIR 到 D: 盘 |
| 内存 | 构建时 RSS | > 8GB | `--jobs 1` 限制并行编译 |
| CPU | 编译负载 | > 90% 持续 | 降级为分 crate 编译 |
| 工时 | 每日人日 | > 1.5 人日/人 | 重新评估优先级,降级 Could 项 |

### 7.4 进度复盘机制

| 机制 | 频率 | 输出 |
|------|------|------|
| 燃尽图 | 每日 18:00 | TodoWrite 状态同步 + 剩余任务曲线 |
| 偏差分析 | 每日 18:30 | 偏差 > 10% 须根因分析 + 纠正措施 |
| 周复盘 | Day 56 17:00 | 8 周总结 + 经验教训归档 |

### 7.5 变更控制流程(CCB)

所有超出 Week 8 范围的变更须经 CCB(Lead Architect + 任务负责人)审批:
1. 提交变更请求(描述 + 影响 + 紧急程度)
2. CCB 1 工作日内决策(Approve / Reject / Defer)
3. Approve 后更新 spec.md + tasks.md + checklist.md
4. Won't 项默认 Defer 到 v1.1.0

---

## 8. 代码质量规范(项目定制)

### 8.1 Rust 编码规范

- 所有 async fn 满足 `Send + 'static + 'async`
- 应用层错误 `anyhow::Result<T>`,库层 `thiserror` enum
- 避免 `unwrap()`/`expect()`(测试代码除外)
- 避免 `Box<dyn Trait>`,优先 `impl Trait` 或 `enum dispatch`
- 单函数 ≤ 200 行,超过必须拆模块
- Top-K 选择用 `select_nth_unstable`(O(n))
- 关键代码决策必须含 WHY 注释

### 8.2 注释覆盖率

- 类/结构体注释:100%(doc comment)
- 公开方法注释:100%(含 `# Arguments` / `# Returns` / `# Errors`)
- 关键逻辑注释:≥ 80%(WHY 注释,非 WHAT 注释)
- 私有方法注释:推荐(非强制)

### 8.3 工具链强制

| 工具 | 用途 | 强制级别 |
|------|------|---------|
| `cargo fmt --all` | 格式化 | Must(Week 7 遗留 2 文件,Week 8 修复) |
| `cargo clippy --workspace --all-targets -- -D warnings` | lint | Must |
| `cargo test --workspace` | 测试 | Must |
| `cargo audit` | 依赖审计 | Must |
| `cargo doc --workspace --no-deps` | 文档 | Must(零 warnings) |
| `cargo fuzz` | 模糊测试 | Must(fuzz crate 单独) |

---

## 9. 资源授权与保障

### 9.1 工具资源清单

| 工具 | 用途 | 权限级别 | 使用范围 |
|------|------|---------|---------|
| cargo / rustc | 编译 / 测试 / 检查 | 读/写 | 全 workspace |
| cargo-audit | 依赖漏洞扫描 | 读 | 全 workspace |
| cargo-fuzz | 模糊测试 | 读/写 | `fuzz/` crate(新建) |
| cargo-zigbuild / cross-rs | 跨平台编译 | 读/写 | 5 平台 binary |
| Docker | 镜像构建 | 读/写 | `Dockerfile` |
| GitHub Actions | CI/CD | 读/写 | `.github/workflows/` |
| criterion | 性能基准 | 读/写 | 各 crate `benches/` |
| MCP 工具 | 子代理协作 | 读/写 | 全项目(按需) |
| Skills(superpowers-main) | 工作流编排 | 读/写 | 全项目 |
| TodoWrite | 任务跟踪 | 读/写 | 全项目 |
| Task(Sub-Agent) | 并行子代理 | 读/写 | 全项目 |

### 9.2 工具使用培训与支持

| 工具 | 培训内容 | 支持联系人 |
|------|---------|-----------|
| cargo-zigbuild | 跨平台编译配置 | E4 DevOps |
| cargo-fuzz | 模糊测试 target 编写 | E3 安全 |
| Docker distroless | 镜像优化 | E4 DevOps |
| GitHub Actions | 多平台 runner 配置 | E4 DevOps |
| criterion | 基准报告解读 | E2 性能 |

### 9.3 环境保障

```powershell
# 工具链环境(Windows 11 + PowerShell)
$env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
$env:TMP = 'D:\Chimera CLI\tmp'
$env:TEMP = 'D:\Chimera CLI\tmp'
$env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"

# 磁盘空间缓解
$env:CARGO_TARGET_DIR = 'D:\Chimera CLI\target'
$env:CARGO_INCREMENTAL = '0'  # clippy 兼容
```

---

## What Changes

- **闭合 Week 7 进行中任务**:Task 5(性能基准报告)/ Task 6(37 模块集成 + 1000 次压测)/ Task 9(SIMD + WAL 性能调优)/ Task 10(端到端验收)
- **新增安全测试套件**:OWASP Top 10 渗透测试 + cargo-fuzz 模糊测试 + cargo-audit 依赖审计
- **新增跨平台发布**:5 平台 binary(Windows/Linux x86_64 + Linux/macOS aarch64 + macOS x86_64)+ Docker distroless 镜像 + GitHub Actions CI/CD
- **新增全量文档**:README + API + cargo doc 零 warnings + CODE_WIKI Week 8 章节 + CHANGELOG Week 8 章节
- **新增全量 E2E**:Quest 生命周期 + 37 模块集成 + 1000 次压测 + Week 8 最终验收
- **发布 v1.0.0-omega**:Git tag + Release notes + 8 周推进计划收尾
- **BREAKING**:无(Week 1-7 API 稳定,Week 8 仅收尾与发布)

## Impact

- Affected specs:
  - `week7-mesh-monitoring-integration`(Task 5/6/9/10 闭合)
  - `fix-week8-carryover`(已完成的 6 项结转在此基线上推进)
- Affected code:
  - `crates/sesa-router/`(SIMD 优化,若适用)
  - `crates/scc-cache/src/wal.rs`(WAL 压测)
  - `crates/scc-cache/benches/`(WAL 基准)
  - `tests/security/owasp_top10.rs`(新增)
  - `tests/fuzz/`(新增 cargo-fuzz crate)
  - `Dockerfile`(新增)
  - `.github/workflows/release.yml`(新增)
  - `README.md`(重写/完善)
  - `CODE_WIKI.md`(Week 8 章节)
  - `CHANGELOG.md`(Week 8 章节)
  - `tests/e2e/quest_lifecycle.rs`(新增/完善)
  - `tests/e2e/full_integration.rs`(新增)
  - `tests/e2e/stress_test.rs`(新增)
  - `tests/e2e/week8_final_acceptance.rs`(新增)
  - `docs/security/week8_security_report.md`(新增)
  - `docs/release/week8_release_guide.md`(新增)
  - `docs/acceptance/week8_final_acceptance_report.md`(新增)
  - `docs/release/v1.0.0-omega_release_notes.md`(新增)

## ADDED Requirements

### Requirement: OWASP Top 10 渗透测试

系统 SHALL 通过 OWASP Top 10(A01-A10)渗透测试,SecCore 沙箱须拦截注入、越权、敏感数据泄露、CSRF、SSRF 等攻击。

#### Scenario: 渗透测试通过

- **WHEN** 运行 `cargo test --test owasp_top10`
- **THEN** 10 项测试 100% 通过,SecCore 沙箱拦截所有攻击载荷

### Requirement: 模糊测试无崩溃

系统 SHALL 通过 10000 次随机输入模糊测试,无 panic / 无 undefined behavior。

#### Scenario: 模糊测试运行

- **WHEN** 运行 `cargo fuzz run <target>`(60s 快速验证或 10000 输入)
- **THEN** 无崩溃,若有崩溃则修复并复测

### Requirement: 依赖漏洞审计

系统 SHALL 通过 `cargo audit` 依赖扫描,无 High/Critical 漏洞。

#### Scenario: 依赖审计

- **WHEN** 运行 `cargo audit`
- **THEN** 无 High/Critical 漏洞,若有则升级依赖并复测

### Requirement: 跨平台 binary 发布

系统 SHALL 生成 5 平台 binary:Windows x86_64 / Linux x86_64 / Linux aarch64 / macOS x86_64 / macOS aarch64。

#### Scenario: 跨平台构建

- **WHEN** 触发 CI/CD(push tag `v1.0.0-omega`)
- **THEN** 5 平台 binary 全部生成,文件大小 < 50MB each

### Requirement: Docker 镜像发布

系统 SHALL 提供 Docker 镜像,基于 distroless,镜像 < 100MB,可执行 `chimera --version`。

#### Scenario: Docker 镜像构建

- **WHEN** 运行 `docker build -t chimera:v1.0.0-omega .`
- **THEN** 镜像构建成功,`docker run chimera:v1.0.0-omega --version` 输出版本号

### Requirement: 全量 E2E 通过

系统 SHALL 通过全量 E2E 测试(Quest 生命周期 + 37 模块集成 + 1000 次压测 + Week 8 最终验收)。

#### Scenario: 最终验收

- **WHEN** 运行 `cargo test --test e2e_*`
- **THEN** 100% 通过,1000 次压测无 panic/泄漏

### Requirement: v1.0.0-omega 发布

系统 SHALL 发布 `v1.0.0-omega` 版本,包含 Git tag + Release notes + 全量文档。

#### Scenario: 版本发布

- **WHEN** Day 56 验收全部通过
- **THEN** 创建 Git tag `v1.0.0-omega`,发布 Release notes,8 周推进计划收尾

## MODIFIED Requirements

### Requirement: 性能调优闭合(Week 7 Task 9)

原 Week 7 Task 9 进行中,Week 8 闭合:路由延迟 p95 ≤ 2ms(SIMD + WAL 启用后),WAL 崩溃恢复压测 1000 次无数据丢失。

### Requirement: 全量集成测试闭合(Week 7 Task 6)

原 Week 7 Task 6 进行中,Week 8 闭合:37 模块全量集成测试 + 1000 次压测无内存泄漏。

## REMOVED Requirements

无。

---

## 10. 范围约束

- 本 spec 仅处理 Week 8 生产化 + 安全 + 发布 + 文档,不涉及新功能开发
- 每项 Task 工作量 ≤ 2 人日(16 工时)
- Won't 项(MCP 2PC 真实跨进程 / CSN 自动发现 / GSOE 强化学习)归档至 v1.1.0 路线图
- 所有变更须经 CCB(Lead Architect + 任务负责人)审批
- 修复完成后更新 `week7-mesh-monitoring-integration/checklist.md` 对应进行中项为 ✅

---

## 11. 参考文献

1. `AETHER_NEXUS_OMEGA_ULTIMATE.md` §7 Week 8 推进计划(Day 50-56)
2. `AETHER_NEXUS_OMEGA_ULTIMATE.md` §8 测试与验收策略
3. `AETHER_NEXUS_OMEGA_ULTIMATE.md` §10.3 ADR 决策记录
4. `CHANGELOG.md` Week 7 章节(基线)
5. `CODE_WIKI.md` 34 crate 模块说明
6. `.trae/specs/week7-mesh-monitoring-integration/spec.md`(Week 7 范围)
7. `.trae/specs/fix-week8-carryover/spec.md`(6 项结转已闭合)
8. `project_memory.md` Week 1-7 经验教训
9. OWASP Top 10 官方文档(https://owasp.org/Top10/)
10. cargo-fuzz 官方文档(https://rust-fuzz.github.io/book/cargo-fuzz.html)
11. cargo-audit 官方文档(https://docs.rs/cargo-audit)
12. cross-rs 官方文档(https://github.com/cross-rs/cross)
13. Docker distroless 官方文档(https://github.com/GoogleContainerTools/distroless)
