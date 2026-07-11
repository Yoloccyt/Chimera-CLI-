# v1.5.0-omega Release Readiness Gap Closure Spec

> **评估日期**:2026-07-11
> **当前基线**:`Cargo.toml` workspace.package.version = `1.4.0-omega`;`CHANGELOG.md` 最新汇总 `v1.5.0-omega`(2026-07-10)
> **目标**:闭合正式发布 v1.5.0-omega 前的全部 P0 阻塞项与关键 P1 风险项,使项目达到可推 tag `v1.5.0-omega` 的发布就绪状态

---

## Why

项目已完成 v1.5.0-omega 阶段的 8 项核心改进(2 安全加固 + 4 架构一致性 + 1 精度修复 + 1 监控盲区),CHANGELOG 已汇总,但四维专家评估(架构/测试/CI-CD/性能安全)发现 **5 项 P0 阻塞项** 与 **9 项 P1 风险项** 阻止正式 tag 发布。P0 项涵盖:TDD 守恒硬伤(`online-learning` 零测试)、版本同步漂移(Cargo.toml 1.4.0 ≠ CHANGELOG 1.5.0)、文档过时(README/CODE_WIKI 仍写 34 crate)、CI 残留日志(check_errors*.txt × 5)。本 spec 定义闭合这些差距的精确要求与验收标准。

## What Changes

### P0 阻塞项(必须完成方可推 tag)

- **补齐 `online-learning` crate 测试基建**:新增 `tests/proptest.rs` + `tests/integration.rs` + `benches/registry_bench.rs`,覆盖 `ParameterRegistry` / `OnlineLearner` / `GradientDescent` 公开 API
- **清理 5 个 `check_errors*.txt` 残留文件**:删除根目录 `check_errors.txt` / `check_errors2.txt` / `check_errors3.txt` / `check_errors_current.txt` / `check_errors_current2.txt`
- **版本同步**:将 `Cargo.toml:31` `workspace.package.version` 从 `1.4.0-omega` 更新为 `1.5.0-omega`
- **README.md 全量更新**:version badge(1.0.0→1.5.0)、crates badge(34→35)、forbid badge(34/34→35/35)、正文 4 处 "34 crate"→"35 crate"
- **CODE_WIKI.md 更新**:7 处 "34 crates"/"34/34"→"35 crates"/"35/35"

### P1 风险项(强烈建议发布前完成)

- **release.yml Windows job 安装 MinGW**:`.github/workflows/release.yml:43-47` 缺少 MinGW 安装步骤,Windows GNU target 必然链接失败(§10.5 P1 短板)
- **src/ unwrap/expect 审计**:重点 crate `repo-wiki/store.rs`(72 unwrap)、`mlc-engine/l2_semantic.rs`(72)、`cmt-tiering/{coordinator,cold,warm}.rs`(153),非 `#[cfg(test)]` 模块的 unwrap 改为 `?` 或 `unwrap_or_else`
- **7 个 crate 补 proptest**:`chimera-tui` / `csn-substitutor` / `mcp-mesh` / `auto-dpo` / `efficiency-monitor` / `decay-engine` / `gsoe-evolution`(§3.3.2 第 3 条要求)
- **CHANGELOG 补齐 v1.0.0/v1.1.0 汇总章节**:当前仅 v1.2.0~v1.5.0 有汇总
- **release.yml 增加 binary < 50MB CI 断言**:§7.2 第 8 条要求,当前无显式 CI 检查
- **release.yml test job 加入 PR 触发**:当前仅 tag 触发,PR 无 test 守护(回归风险)
- **Dockerfile ARG VERSION 默认值更新**:`1.0.0-omega`→`1.5.0-omega`(虽 CI 覆盖,本地构建会漂移)
- **fuzz target 文档同步**:规则文档声称 3 个 target,实际 6 个(`fuzz/Cargo.toml` 已声明)
- **MoE 五维 variance 缓存优化**:n=200 时 89.93µs(1.1× 余量),缓存 variance 计算可降 ~40% 延迟

### P2 GA 后跟进(不阻塞本次发布,记录为 tracking)

- Linux gVisor 沙箱启用(ADR-001 完整落地)
- Linux setrlimit 资源限制(F-003)
- 监控指标扩展(35 crate 大多数无 Prometheus 指标)
- fuzz target 扩展(decay-engine / osa-coordinator / kvbsr-router)
- `Box<dyn>` 33 处评估改 enum dispatch
- `as f32` 252 处精度审计(CACR 教训)

## Impact

- **Affected specs**:无既有 spec 受影响(本 spec 为发布就绪闭合)
- **Affected code**:
  - `Cargo.toml`(版本号)
  - `README.md` / `CODE_WIKI.md`(文档同步)
  - `crates/online-learning/`(新增 tests/ + benches/)
  - `crates/repo-wiki/src/store.rs` / `crates/mlc-engine/src/l2_semantic.rs` / `crates/cmt-tiering/src/*.rs`(unwrap 审计)
  - 7 个 crate 的 `tests/proptest.rs`(新增)
  - `.github/workflows/release.yml`(Windows MinGW + binary 断言 + PR 触发)
  - `Dockerfile`(ARG VERSION)
  - `CHANGELOG.md`(补齐 v1.0.0/v1.1.0)
  - `.trae/rules/nuxus规则.md` / `.claude/CLAUDE.md`(fuzz target 数量同步)
  - `crates/model-router/src/history/memory.rs`(variance 缓存)
  - 根目录 5 个 `check_errors*.txt`(删除)

---

## ADDED Requirements

### Requirement: online-learning crate 测试基建

`online-learning` crate 必须补齐测试基建,满足 §3.3.2 新 crate 准入 checklist 第 3/8 条。

#### Scenario: proptest 覆盖 ParameterRegistry 并发安全
- **WHEN** 多线程并发调用 `ParameterRegistry::register` / `unregister` / `get`
- **THEN** 注册表状态保持一致,无竞态条件,proptest 256 cases 全部通过

#### Scenario: 集成测试覆盖 GradientDescent 反馈驱动更新
- **WHEN** 调用 `GradientDescent::apply_gradient` 更新参数
- **THEN** 参数值按梯度方向更新,学习率衰减正确,收敛性验证通过

#### Scenario: bench 覆盖 registry 注册/查询延迟
- **WHEN** 运行 `cargo bench -p online-learning`
- **THEN** bench 编译通过,registry 注册/查询延迟在微秒级

### Requirement: release.yml Windows MinGW 安装

`release.yml` 的 Windows job 必须安装 MinGW 工具链,确保 GNU target 链接成功。

#### Scenario: Windows GNU target 链接成功
- **WHEN** 推送 `v1.5.0-omega` tag 触发 release.yml
- **THEN** Windows job 成功安装 MinGW,`cargo build --release --target x86_64-pc-windows-gnu` 链接成功

### Requirement: release.yml binary 体积 CI 断言

`release.yml` 必须在 build job 后断言 binary 体积 < 50MB。

#### Scenario: binary 体积超限检测
- **WHEN** release build 产生 binary
- **THEN** CI 步骤检查 binary 体积 < 50MB,超限则 fail

### Requirement: MoE 五维 variance 缓存

`model-router` 的 `InMemoryHistoryStore` 必须缓存 `latency_variance()` 计算结果,避免每次 `gate()` 重复计算。

#### Scenario: variance 缓存命中
- **WHEN** 多次调用 `gate()` 传入同一 `HistoryRecord`
- **THEN** variance 仅在 `record()` 时计算一次,后续 `gate()` 直接读缓存,n=200 延迟降 ~40%

---

## MODIFIED Requirements

### Requirement: 版本同步

`Cargo.toml` 的 `workspace.package.version` 必须与待发布 tag 版本一致,`CHANGELOG.md` 必须存在对应汇总章节。

#### Scenario: v1.5.0-omega 版本同步
- **WHEN** 准备发布 v1.5.0-omega
- **THEN** `Cargo.toml:31` = `1.5.0-omega`,`CHANGELOG.md` 存在 `v1.5.0-omega 汇总` 章节

### Requirement: README.md 文档同步

`README.md` 的 badge 与正文必须反映当前项目状态(35 crate / v1.5.0-omega / 35/35 forbid unsafe)。

#### Scenario: README badge 准确
- **WHEN** 读取 README.md
- **THEN** version badge = `1.5.0--omega`,crates badge = `35`,forbid badge = `35/35-success`,正文无 "34 crate" 残留

### Requirement: CODE_WIKI.md 文档同步

`CODE_WIKI.md` 的 crate 计数必须反映实际 35 crate(含 `online-learning`)。

#### Scenario: CODE_WIKI crate 数准确
- **WHEN** 读取 CODE_WIKI.md
- **THEN** 7 处 "34 crates"/"34/34" 全部更新为 "35 crates"/"35/35"

### Requirement: release.yml PR 测试守护

`release.yml` 的 test job 应在 PR 触发时运行,防止回归。

#### Scenario: PR 触发测试守护
- **WHEN** 提交改动 Cargo.lock 的 PR
- **THEN** test job 自动运行,失败则阻止合并

### Requirement: fuzz target 文档同步

规则文档(`nuxus规则.md §10.3` / `CLAUDE.md §6`)的 fuzz target 数量必须与 `fuzz/Cargo.toml` 实际声明一致。

#### Scenario: fuzz target 数量一致
- **WHEN** 读取规则文档与 fuzz/Cargo.toml
- **THEN** 文档声称的 fuzz target 数量 = `fuzz/Cargo.toml` 声明的数量(当前 6 个)

---

## REMOVED Requirements

### Requirement: check_errors*.txt 残留文件

**Reason**:违反 `CLAUDE.md §7` "提交前不得遗留未归档的 check_errors*.txt";当前 `cargo check --workspace` 已通过(EXIT_CODE=0),残留文件为历史日志,造成误导。

**Migration**:直接删除 5 个文件;如需保留日志,归档为带时间戳文件名(如 `check_errors_20260711_120000.txt.gz`)并移至 `docs/archive/`。

### Requirement: src/ unwrap/expect 滥用

**Reason**:违反 `nuxus规则.md §4.1` "避免 unwrap()/expect() — 所有可能失败的边界必须用 ? 或 match 处理";src/ 中 1222 unwrap + 326 expect 存在边界 panic 风险。

**Migration**:重点 crate(`repo-wiki/store.rs` / `mlc-engine/l2_semantic.rs` / `cmt-tiering/*.rs`)的非 `#[cfg(test)]` 模块 unwrap 改为 `?` 或 `unwrap_or_else`,带 WHY 注释说明为何安全。`#[cfg(test)]` 模块的 unwrap 保留(测试代码可接受 panic)。

---

## 验收标准汇总

| 编号 | 验收点 | 优先级 |
|------|--------|--------|
| AC-01 | `online-learning` crate 有 tests/proptest.rs + tests/integration.rs + benches/registry_bench.rs | P0 |
| AC-02 | 根目录 0 个 check_errors*.txt 残留 | P0 |
| AC-03 | `Cargo.toml:31` = `1.5.0-omega` | P0 |
| AC-04 | README.md badge 与正文全部更新为 35 crate / 1.5.0-omega | P0 |
| AC-05 | CODE_WIKI.md 7 处 34→35 全部更新 | P0 |
| AC-06 | release.yml Windows job 有 MinGW 安装步骤 | P1 |
| AC-07 | src/ 重点 crate unwrap/expect 审计完成 | P1 |
| AC-08 | 7 个 crate 补 proptest.rs | P1 |
| AC-09 | CHANGELOG 补齐 v1.0.0/v1.1.0 汇总 | P1 |
| AC-10 | release.yml 有 binary < 50MB CI 断言 | P1 |
| AC-11 | release.yml test job 加入 PR 触发 | P1 |
| AC-12 | Dockerfile ARG VERSION = 1.5.0-omega | P1 |
| AC-13 | 规则文档 fuzz target 数量 = 6 | P1 |
| AC-14 | MoE variance 缓存实现,bench 延迟降 ~40% | P1 |
| AC-15 | `cargo check --workspace` 退出码 0 | P0 |
| AC-16 | `cargo test --workspace` 退出码 0 | P0 |
| AC-17 | `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0 | P0 |
| AC-18 | `cargo fmt --all -- --check` 退出码 0 | P0 |
