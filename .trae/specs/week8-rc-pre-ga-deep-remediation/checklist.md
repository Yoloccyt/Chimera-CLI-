# Checklist — Week 8 RC GA 前深度审计与修复

## 阶段一:P0 阻塞项验收

### Task 1: event-bus Critical 事件双通道化
- [x] `crates/event-bus/src/bus.rs` 新增 `subscribe_critical_events()` API
- [x] `crates/event-bus/src/bus.rs` 新增 `publish_critical()` 方法,broadcast + mpsc 双推
- [x] `crates/event-bus/src/bus.rs` `publish().await` 对 Critical 事件自动双推
- [x] `crates/event-bus/src/backpressure.rs` `CriticalMpsc` 策略注释更新为"已实现"
- [x] `crates/event-bus/tests/integration_critical_channel.rs` 新增,模拟 broadcast Lagged 场景,断言 mpsc 接收 4 类 Critical 事件
- [x] `crates/efficiency-monitor/src/lib.rs` `start_event_subscriber()` 增加 Critical mpsc 订阅
- [x] `cargo test -p event-bus` 全绿
- [x] `cargo test -p efficiency-monitor` 全绿

### Task 2: decb-governor BudgetExceeded 事件发布
- [x] `crates/decb-governor/src/{lib.rs, governor.rs}` 预算超限分支调用 `bus.publish_blocking(NexusEvent::BudgetExceeded { ... })`
- [x] `crates/decb-governor/src/error.rs:21` TODO 注释删除
- [x] `crates/decb-governor/tests/integration_budget_exceeded.rs` 新增,断言事件 severity = Critical
- [x] `cargo test -p decb-governor` 全绿

### Task 3: tests/stress/week7_stress.rs 加 #[ignore]
- [x] `tests/stress/week7_stress.rs` 5 个 `#[test]` 全部添加 `#[ignore = "perf: run with --ignored"]`
- [x] `cargo test --workspace` 默认跳过这 5 个测试
- [x] `cargo test -- --ignored` 能显式触发

### Task 4: chimera-cli 注释校准
- [x] `crates/chimera-cli/src/commands/mod.rs:3` "Stage 0" 改为 "Stage 8 RC"
- [x] `crates/chimera-cli/src/commands/{run, tui, quest, config, wiki, parliament}.rs` 过时 Stage 注释同步
- [x] 注释明确标注"L10 编排接线延后到 v1.1"

### Task 5: Week 8 新 crate tests/ 目录
- [x] `crates/acb-governor/tests/integration.rs` 新增,≥ 3 个集成测试
- [x] `crates/auto-dpo/tests/integration.rs` 新增,≥ 3 个集成测试
- [x] `crates/chimera-tui/tests/integration.rs` 新增,≥ 3 个集成测试
- [x] `cargo test -p acb-governor -p auto-dpo -p chimera-tui` 全绿

## 阶段二:P1 高优项验收

### Task 6: Dockerfile RUST_BACKTRACE
- [x] `Dockerfile` runtime stage 包含 `ENV RUST_BACKTRACE=1`
- [x] 可选:`ENV RUST_LOG=info` 已添加
- [x] Dockerfile 多阶段构建静态核验通过

### Task 7: repo-wiki Top-K 优化
- [x] `crates/repo-wiki/src/vector.rs:102` 使用 `select_nth_unstable_by(top_k, ...)`
- [x] 仅对 `scored[..k]` 做 K-log-K 排序(若需有序输出)
- [x] `cargo test -p repo-wiki` 向量检索结果一致

### Task 8: gsoe-evolution Top-K 优化
- [x] `crates/gsoe-evolution/src/engine.rs:255` 使用 `select_nth_unstable_by(elite_count, ...)`
- [x] `cargo test -p gsoe-evolution` 精英选择结果一致

### Task 9: parliament ahirt.rs f32 精度
- [x] `crates/parliament/src/ahirt.rs:440` 全程 f32 比较
- [x] 同文件其他 f32→f64 转换已修复
- [x] `cargo test -p parliament` AHIRT 阈值判断正确

### Task 10: faae-router Arc 共享 mutate
- [x] `crates/faae-router/src/router.rs` `edsb` 字段改为 `Arc<EdsbBalancer>`(或 `Arc<Mutex<EdsbBalancer>>`)
- [x] `crates/faae-router/src/router.rs:316` 使用 `Arc::clone(&self.edsb)`
- [x] `cargo test -p faae-router` EDSB 衰减循环反映到原 router

### Task 11: faae-router 引入 rand
- [x] 根 `Cargo.toml` `[workspace.dependencies]` 新增 `rand = "0.8"`
- [x] `crates/faae-router/Cargo.toml` 添加 `rand = { workspace = true }`
- [x] `crates/faae-router/src/edsb.rs:364` `pseudo_random_probability` 使用 `rand::random::<f64>()`
- [x] `cargo test -p faae-router` EDSB 概率均衡

### Task 12: CI workflows timeout + RUST_BACKTRACE
- [x] `.github/workflows/audit.yml` audit job `timeout-minutes: 30`
- [x] `.github/workflows/release.yml` build/test/docker/release jobs 各 `timeout-minutes: 30`
- [x] `.github/workflows/release.yml` build job `env:` 块含 `RUST_BACKTRACE: 1`
- [x] `.github/workflows/fuzz.yml` fuzz job `timeout-minutes: 20`
- [x] 三个 workflow YAML 语法静态核验通过

### Task 13: OWASP A06 + A03/A10 加固
- [x] `tests/security/owasp_top10.rs` A06 新增 `test_a06_dependency_version_assertions`
- [x] A06 测试解析 `Cargo.lock`,断言 rusqlite ≥ 0.31 / tokio ≥ 1.40 / serde ≥ 1.0
- [x] A03 新增 `#[cfg(windows)] test_a03_windows_path_traversal`
- [x] A10 新增 `#[cfg(windows)] test_a10_windows_ssrf_powershell`
- [x] `cargo test --test owasp_top10` 全绿

## 阶段三:P2 工程基建与文档验收

### Task 14: CI workflow 优化
- [x] `.github/workflows/audit.yml` 加 `Swatinem/rust-cache@v2` 或 `actions/cache@v4`
- [x] `.github/workflows/fuzz.yml` `cargo install cargo-fuzz --locked`
- [x] `.github/workflows/release.yml` 删除冗余 `'v1.0.0-omega'` tag 模式
- [x] `.github/workflows/release.yml` test job 加 `if: startsWith(github.ref, 'refs/tags/v')`

### Task 15: figment 配置样例
- [x] `examples/config.sample.yaml` 新增,包含 figment 三源样例
- [x] `examples/config.sample.toml` 新增
- [x] `crates/chimera-cli/src/config.rs` 文档注释引用样例路径

### Task 16: install.ps1 自动化 env
- [x] `install.ps1` 新增 `Set-Environment` 函数
- [x] `install.ps1` 添加 `--setup-env` 开关参数
- [x] README 文档化"克隆即用"流程

### Task 17: 占位实现文档化
- [x] `README.md` "已知限制"章节列出 6 类占位实现
- [x] `docs/release/v1.0.0-omega_release_notes.md` 同步更新
- [x] 每项占位实现注明 v1.1 计划

### Task 18: nexus-core proptest + benches
- [x] `crates/nexus-core/tests/proptest.rs` 新增,CLV / UserIntent / Quest 不变量
- [x] `crates/nexus-core/benches/clv_bench.rs` 新增
- [x] `crates/nexus-core/Cargo.toml` 添加 `[dev-dependencies] proptest` 与 `[[bench]]` 配置
- [x] `cargo test -p nexus-core` 全绿
- [x] `cargo bench -p nexus-core` 编译通过

### Task 19: nuxus规则.md §10.5 同步
- [x] `.trae/rules/nuxus规则.md` §10.5 `target_clippy*/` 残留条目改为"已清理"
- [x] Dockerfile RUST_BACKTRACE 短板状态更新为"已修复"
- [x] env 自动化短板状态更新为"已改进(install.ps1 --setup-env)"
- [x] figment 样例短板状态更新为"已补齐"

## 阶段四:全量验收

### Task 20: 全量验收 + 文档同步
- [x] `cargo check --workspace` exit 0
- [x] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` exit 0(用 §9 缓解)
- [x] `cargo test --workspace` 全绿(默认不含 #[ignore])
- [x] `cargo test -- --ignored --nocapture` 压测通过
- [x] `cargo fmt --all -- --check` exit 0(零 diff)
- [x] `CHANGELOG.md` 追加 Week 8 RC 修复章节
- [x] `project_memory.md` 追加经验教训
- [x] `checklist.md` 全部 checkpoint 勾选

## 综合验收

- [x] 5 个 P0 阻塞项全部修复
- [x] 8 个 P1 高优项全部修复(Task 6-13)
- [x] 6 个 P2 中优项全部完成(Task 14-19)
- [x] RC 阶段约束(§3.1)未违反:无跨层重构、无新 crate、无核心领域类型变更
- [x] 综合健康度从 80.4 提升到 ≥ 90
- [x] v1.0.0-omega GA 发布就绪
