# Week 8 已知限制修复 Spec(week8-limitations-remediation)

## Why

Week 8 验收通过(v1.0.0-omega tag 已创建),但验收报告 §9 列出 5 项已知限制。这些限制虽非阻塞发布,但影响生产就绪度与质量信誉。本 spec 将这 5 项限制正式化为可执行任务,组建精英专家子代理团队按 MoSCoW 优先级修复,确保 NEXUS-OMEGA 项目达到真正的生产级标准。

## 5 项已知限制与可修复性分析

### 限制 1:cargo-fuzz 3 target 未实际运行(需 nightly 工具链)
- **现状**:3 个 fuzz target 已创建(quest_parse / seccore_sandbox / event_serialize),fuzz crate 配置完整,但当前环境仅有 stable 工具链,无法运行 `cargo +nightly fuzz run`
- **可修复性**:中-高
- **方案**:安装 nightly-x86_64-pc-windows-gnu 工具链 + llvm-tools-preview,运行 3 个 target 各 60s 或 10000 输入
- **风险**:nightly 下载约 200MB(可能耗时);fuzz 可能发现 panic(需修复或记录)
- **红线兼容**:fuzz crate 独立于主 workspace,libfuzzer-sys 的 unsafe 不污染 `#![forbid(unsafe_code)]` 40/40 覆盖

### 限制 2:5 平台 binary 交叉编译未实际验证(需 CI 环境)
- **现状**:本机仅验证 Windows x86_64(aether.exe 6.96MB),其他 4 平台(Linux x86_64/aarch64 / macOS x86_64/aarch64)需 GitHub Actions CI 触发
- **可修复性**:低(本机 Docker 未装 / zig 未装 / macOS target 下载大)
- **方案**:静态验证 `.github/workflows/release.yml` 语法与逻辑完整性;确认 5 平台 matrix 配置正确;本机尝试 1-2 个可交叉编译的 target(如 x86_64-unknown-linux-gnu via cargo-zigbuild,需先装 zig)
- **替代**:标注为"需 CI 环境验证",在 release guide 中明确说明

### 限制 3:Docker 镜像未实际构建(本机未装 Docker)
- **现状**:Dockerfile 配置完整(多阶段 builder → distroless/cc-debian12),但本机未安装 Docker Desktop
- **可修复性**:低(需安装 Docker Desktop,约 500MB+)
- **方案**:静态验证 Dockerfile 语法与多阶段构建逻辑;确认 base image / COPY / ENTRYPOINT 正确;验证镜像体积估算(< 100MB)
- **替代**:标注为"需 Docker 环境验证",在 release guide 中说明

### 限制 4:stress_test 1000 次压测未实际运行(标记 `#[ignore]`)
- **现状**:`tests/e2e/stress_test.rs` 的 `test_stress_1000_iterations` 标记 `#[ignore]`,编译通过但未实际运行
- **可修复性**:高
- **方案**:运行 `cargo test --test stress_test -- --ignored`,验证 1000 次全链路迭代无 panic / 无内存泄漏 / 延迟退化 < 50%
- **风险**:压测可能发现内存泄漏或性能退化(这正是测试价值,需修复或记录)
- **预期成果**:1000 次压测通过,输出 p50/p95/p99 延迟统计

### 限制 5:clippy 在 Windows 需 `--jobs 1` + `CARGO_INCREMENTAL=0`(clippy-driver 栈溢出)
- **现状**:Windows 下 clippy-driver.exe 并行编译时 STATUS_STACK_BUFFER_OVERRUN,需 `--jobs 1` + `CARGO_INCREMENTAL=0` workaround
- **可修复性**:中(根本修复困难,workaround 已是合理方案)
- **方案**:尝试 `RUST_MIN_STACK=33554432`(32MB)增大栈大小;对比 `--jobs 1` vs `--jobs 2` vs 默认;分析根因(可能是 clippy-driver 递归深度)
- **替代**:确认 workaround 是最佳方案,在文档中记录为 Windows 环境已知限制

## What Changes

### 限制 1 修复(cargo-fuzz 运行)
- 安装 `nightly-x86_64-pc-windows-gnu` 工具链 + `llvm-tools-preview` 组件
- 运行 `cargo +nightly fuzz run quest_parse -- -max_total_time=60`(3 个 target 各 60s)
- 若发现 panic:修复被测代码 或 记录为已知 bug
- 更新 `docs/security/week8_security_report.md` 补充 fuzz 实际运行结果

### 限制 2 修复(CI 验证 + 条件性交叉编译)
- 静态验证 `.github/workflows/release.yml`:5 平台 matrix / cross / strip / upload / release 逻辑
- 尝试安装 zig(若可下载),运行 `cargo zigbuild --release --target x86_64-unknown-linux-gnu -p chimera-cli`
- 若 zig 不可用:标注为"需 CI 环境验证",更新 `docs/release/week8_release_guide.md`

### 限制 3 修复(Dockerfile 验证)
- 静态验证 Dockerfile:多阶段构建 / base image / COPY / ENTRYPOINT / 体积估算
- 验证 `# syntax=docker/dockerfile:1.7` 指令正确
- 确认 distroless/cc-debian12 镜像约 20MB + binary 7MB = 27MB < 100MB
- 更新 `docs/release/week8_release_guide.md` 补充 Docker 验证状态

### 限制 4 修复(stress_test 运行)
- 运行 `cargo test --test stress_test -- --ignored --nocapture`
- 验证 6 项断言:1000 次成功 / Wiki 3000 条 / WikiStore 持久化 / 延迟退化 < 50% / 最大单次 < 2s / p95 统计
- 若失败:分析根因,修复内存泄漏或调整阈值(需 CCB 审批)
- 记录压测结果至 `docs/performance/week8_stress_test_report.md`

### 限制 5 修复(clippy 根因分析)
- 尝试 `RUST_MIN_STACK=33554432 cargo clippy --workspace --all-targets -- -D warnings`(32MB 栈)
- 对比 `--jobs 1`(当前 workaround)vs `--jobs 2` vs 默认并行
- 若 `RUST_MIN_STACK` 解决:更新文档移除 `--jobs 1` 要求
- 若未解决:确认 workaround 是最佳方案,记录为 Windows 环境已知限制

## Impact

- **Affected specs**:`week8-production-release-hardening`(已知限制清单清零)
- **Affected code**:
  - `tests/e2e/stress_test.rs`(限制 4,可能调整阈值)
  - `fuzz/`(限制 1,仅运行不修改)
  - `docs/security/week8_security_report.md`(限制 1,补充运行结果)
  - `docs/release/week8_release_guide.md`(限制 2/3,补充验证状态)
  - `docs/performance/week8_stress_test_report.md`(限制 4,新建)
  - `docs/acceptance/week8_final_acceptance_report.md`(限制 5,更新 clippy 章节)

## 团队组建与职责分配(RACI)

### 团队规模:5 名精英专家子代理(10+ 年经验)

| 角色 | 代号 | 职责 | 限制负责 |
|------|------|------|---------|
| 首席架构师 / 总协调 | E1 | 任务优先级裁定 + 总协调 + 压测验证 | 限制 4 |
| 资深测试工程师 | E2 | cargo-fuzz 运行 + panic 分析 | 限制 1 |
| DevOps 工程师 | E3 | CI workflow 验证 + Dockerfile 验证 + zig 尝试 | 限制 2/3 |
| 性能工程师 | E4 | clippy 栈溢出根因分析 + RUST_MIN_STACK 实验 | 限制 5 |
| 文档工程师 | E5 | 所有文档同步 + checklist 更新 | 全部 |

### RACI 责任矩阵

| 任务 | 负责人(R) | 参与者(A) | 咨询(C) | 知情(I) |
|------|-----------|-----------|---------|---------|
| 限制 4 压测运行 | E1 | E4 | E2 | E5 |
| 限制 1 cargo-fuzz | E2 | E1 | E4 | E5 |
| 限制 2 CI 验证 | E3 | E1 | - | E5 |
| 限制 3 Docker 验证 | E3 | E1 | - | E5 |
| 限制 5 clippy 根因 | E4 | E1 | E2 | E5 |
| 文档同步 | E5 | E1 | 全员 | - |

## 任务优先级(MoSCoW)

| 优先级 | 限制 | 理由 |
|--------|------|------|
| **Must** | 限制 4(stress_test) | 直接可做,验证压测稳定性,价值最高 |
| **Should** | 限制 1(cargo-fuzz) | 需安装 nightly,验证模糊测试,价值高 |
| **Should** | 限制 5(clippy 根因) | 分析栈溢出根因,可能找到更优方案 |
| **Could** | 限制 2(CI 验证) | 环境限制,改为静态验证 |
| **Could** | 限制 3(Docker 验证) | 环境限制,改为静态验证 |

## 风险评估

| 风险 | 可能性 | 影响 | 应对措施 |
|------|--------|------|---------|
| nightly 安装失败(网络) | 中 | 中 | 重试 3 次;若失败则限制 1 标注为"需 nightly 环境" |
| fuzz 发现 panic | 中 | 高 | 分析根因;若是被测代码 bug 则修复;若是 fuzz target 误报则调整 |
| 压测发现内存泄漏 | 低 | 高 | 分析 leak_probe strong_count;修复 Arc 泄漏;或记录为已知限制 |
| zig 安装失败 | 高 | 低 | 标注为"需 CI 环境";不影响本机验证 |
| clippy 根因无法修复 | 中 | 低 | 确认 `--jobs 1` workaround 是最佳方案;记录为 Windows 已知限制 |

## 质量验收基准

- **功能测试通过率**:≥ 95%(现有 3002+ 测试 + 压测通过)
- **代码质量评分**:≥ 85 分(clippy 零警告 + forbid(unsafe_code) 40/40)
- **性能指标**:压测 p95 < 2s / 延迟退化 < 50%
- **安全指标**:fuzz 3 target 无 panic(或已记录)
- **文档同步**:所有报告更新 + checklist 全勾

## ADDED Requirements

### Requirement: stress_test 1000 次压测实际运行

系统 SHALL 实际运行 `tests/e2e/stress_test.rs::test_stress_1000_iterations`,验证 1000 次全链路迭代无 panic、无内存泄漏、延迟退化 < 50%。

#### Scenario: 压测通过
- **WHEN** 运行 `cargo test --test stress_test -- --ignored --nocapture`
- **THEN** 1000 次迭代全部成功,Wiki 累积 3000 条,延迟退化 < 50%,p95 < 2s

#### Scenario: 压测失败
- **WHEN** 压测发现内存泄漏或性能退化
- **THEN** 分析根因,修复或记录为已知限制(需 CCB 审批)

### Requirement: cargo-fuzz 3 target 实际运行

系统 SHALL 在 nightly 工具链下运行 3 个 fuzz target 各 60s,验证无 panic。

#### Scenario: fuzz 通过
- **WHEN** 运行 `cargo +nightly fuzz run <target> -- -max_total_time=60`
- **THEN** 3 个 target 各运行 60s 无 panic

#### Scenario: fuzz 发现 panic
- **WHEN** fuzz target 发现 panic
- **THEN** 分析根因,修复被测代码或调整 fuzz target,重新运行验证

### Requirement: clippy 栈溢出根因分析

系统 SHALL 分析 clippy-driver.exe 栈溢出根因,尝试 `RUST_MIN_STACK=33554432` 增大栈大小,若解决则更新文档移除 `--jobs 1` 要求。

#### Scenario: RUST_MIN_STACK 解决
- **WHEN** 运行 `RUST_MIN_STACK=33554432 cargo clippy --workspace --all-targets -- -D warnings`
- **THEN** clippy 正常完成无栈溢出,更新文档移除 `--jobs 1` workaround

#### Scenario: RUST_MIN_STACK 未解决
- **WHEN** RUST_MIN_STACK 仍栈溢出
- **THEN** 确认 `--jobs 1` + `CARGO_INCREMENTAL=0` 是最佳方案,记录为 Windows 已知限制

### Requirement: CI workflow 与 Dockerfile 静态验证

系统 SHALL 静态验证 `.github/workflows/release.yml` 与 `Dockerfile` 的语法与逻辑完整性,确认 5 平台 matrix 配置正确、多阶段构建逻辑正确。

#### Scenario: CI 验证通过
- **WHEN** 审查 release.yml
- **THEN** 5 平台 matrix / cross / strip / upload / release 逻辑完整,语法正确

#### Scenario: Dockerfile 验证通过
- **WHEN** 审查 Dockerfile
- **THEN** 多阶段构建 / base image / COPY / ENTRYPOINT 正确,体积估算 < 100MB

## MODIFIED Requirements

### Requirement: Week 8 已知限制清单清零

原 Week 8 验收报告 §9 列出 5 项已知限制。本周通过实际运行 + 静态验证 + 根因分析,将可修复的限制清零,不可修复的限制(环境依赖)记录为"需 CI/Docker 环境验证"。

## REMOVED Requirements

无。

## 执行原则

1. **长期主义**:修复方案考虑可维护性,杜绝短期行为
2. **资源监控**:每个 Task 控制在 4 小时内,避免资源过度消耗
3. **变更控制**:压测阈值调整需 CCB(首席架构师)审批
4. **TDD-first**:若修复被测代码,先写失败测试再修复
5. **证据驱动**:所有修复需提供命令输出 / 日志作为证据
6. **并行化**:限制 1/4/5 可并行(独立子代理),限制 2/3 串行(同一 DevOps)

## 资源授权

授权团队调用以下工具资源:
- **RunCommand**:执行 cargo / rustup / git 命令
- **Read / Edit / Write**:读取 / 修改源码与文档
- **Grep / Glob**:代码搜索
- **Task**(子代理):并行调度专家子代理
- **MCP 工具**:如需(kubernetes / context7 等)
- **Skill**:superpowers-main(深度思考)

## 参考文献

- `AETHER_NEXUS_OMEGA_ULTIMATE.md` §7(8 周推进计划)
- `docs/acceptance/week8_final_acceptance_report.md` §9(已知限制)
- `docs/security/week8_security_report.md`(安全测试报告)
- `docs/release/week8_release_guide.md`(发布指南)
- `.github/workflows/release.yml`(CI/CD 配置)
- `Dockerfile`(多阶段构建)
- `tests/e2e/stress_test.rs`(压测代码)
- `fuzz/Cargo.toml`(fuzz 配置)
