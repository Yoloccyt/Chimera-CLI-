# Tasks — Week 8 RC GA 前深度审计与修复

> 任务按优先级分组,P0 阻塞 GA、P1 强烈建议、P2 周期内补齐。
> RC 阶段约束(§3.1):仅 bugfix / 安全加固 / 性能微调 / 文档同步,禁止跨层重构。

## 阶段一:P0 阻塞项修复(GA 前必须)

- [x] **Task 1: event-bus Critical 事件双通道化**
  - [x] SubTask 1.1: 在 `crates/event-bus/src/bus.rs` 新增 `critical_tx: Mutex<HashMap<TypeId, mpsc::UnboundedSender<NexusEvent>>>` 字段,提供 `subscribe_critical_events() -> mpsc::UnboundedReceiver<NexusEvent>` API
  - [x] SubTask 1.2: 新增 `publish_critical(event: NexusEvent)` 方法,broadcast + mpsc 双推;`publish().await` 内部对 Critical 事件自动调用双推
  - [x] SubTask 1.3: 在 `crates/event-bus/src/backpressure.rs` 更新 `CriticalMpsc` 策略注释,标注"已实现双通道"
  - [x] SubTask 1.4: 新增测试 `tests/integration_critical_channel.rs`:模拟 broadcast Lagged 场景,断言 mpsc 旁路仍能接收 4 类 Critical 事件
  - [x] SubTask 1.5: 更新 `crates/efficiency-monitor/src/lib.rs` 的 `start_event_subscriber()`,增加 `bus.subscribe_critical_events()` 订阅 Critical mpsc 流

- [x] **Task 2: decb-governor 集成 BudgetExceeded 事件发布**
  - [x] SubTask 2.1: 读取 `crates/decb-governor/src/{lib.rs, error.rs, governor.rs}` 确认当前预算超限检测路径
  - [x] SubTask 2.2: 在预算超限分支调用 `self.bus.publish_blocking(NexusEvent::BudgetExceeded { ... })`(sync 方法用 publish_blocking)
  - [x] SubTask 2.3: 删除 `error.rs:21` 的 `TODO(Week 5 Task 37)` 注释
  - [x] SubTask 2.4: 新增测试 `tests/integration_budget_exceeded.rs`:模拟预算超限,断言 EventBus 收到 `BudgetExceeded` 事件且 `severity() == Critical`

- [x] **Task 3: tests/stress/week7_stress.rs 5 测试加 #[ignore]**
  - [x] SubTask 3.1: 读取 `tests/stress/week7_stress.rs`,定位 5 个 `#[test]` 函数
  - [x] SubTask 3.2: 为每个 `#[test]` 添加 `#[ignore = "perf: run with --ignored"]` 属性
  - [x] SubTask 3.3: 运行 `cargo test --workspace` 验证 5 测试默认跳过;`cargo test -- --ignored` 验证可显式触发

- [x] **Task 4: chimera-cli/src/commands/mod.rs:3 注释校准**
  - [x] SubTask 4.1: 读取 `crates/chimera-cli/src/commands/mod.rs` 头部 30 行
  - [x] SubTask 4.2: 将 "当前 Stage 0 阶段" 改为 "当前 Stage 8 RC 阶段(L10 编排接线延后到 v1.1)"
  - [x] SubTask 4.3: 同步检查 `commands/{run, tui, quest, config, wiki, parliament}.rs` 中的过时 Stage 注释

- [x] **Task 5: acb-governor / auto-dpo / chimera-tui 补 tests/ 目录**
  - [x] SubTask 5.1: 创建 `crates/acb-governor/tests/integration.rs`,至少 3 个集成测试(预算调整事件发布 / 流体控制边界 / with_event_bus 集成)
  - [x] SubTask 5.2: 创建 `crates/auto-dpo/tests/integration.rs`,至少 3 个集成测试(DpoPairGenerated 事件 / 偏好对收集 / with_event_bus 集成)
  - [x] SubTask 5.3: 创建 `crates/chimera-tui/tests/integration.rs`,至少 3 个集成测试(布局渲染 / 输入模式切换 / 键盘事件处理)
  - [x] SubTask 5.4: 运行 `cargo test -p acb-governor -p auto-dpo -p chimera-tui` 验证全绿

## 阶段二:P1 高优项修复(GA 前强烈建议)

- [x] **Task 6: Dockerfile 加 ENV RUST_BACKTRACE=1**
  - [x] SubTask 6.1: 读取 `Dockerfile` 全文,定位 runtime stage
  - [x] SubTask 6.2: 在 `FROM gcr.io/distroless/cc-debian12` 之后添加 `ENV RUST_BACKTRACE=1`
  - [x] SubTask 6.3: 可选追加 `ENV RUST_LOG=info` 用于生产排查
  - [x] SubTask 6.4: 静态核验 Dockerfile 多阶段构建完整性

- [x] **Task 7: repo-wiki/src/vector.rs:102 Top-K 改 select_nth_unstable**
  - [x] SubTask 7.1: 读取 `crates/repo-wiki/src/vector.rs` 第 80-130 行
  - [x] SubTask 7.2: 将 `scored.sort_by(...); scored.truncate(top_k)` 改为 `scored.select_nth_unstable_by(top_k, ...); let top = &scored[..top_k];`
  - [x] SubTask 7.3: 若需有序输出,对 `top` 做 K-log-K `sort_by`
  - [x] SubTask 7.4: 运行 `cargo test -p repo-wiki` 验证向量检索结果一致

- [x] **Task 8: gsoe-evolution/src/engine.rs:255 Top-K 改 select_nth_unstable**
  - [x] SubTask 8.1: 读取 `crates/gsoe-evolution/src/engine.rs` 第 240-280 行
  - [x] SubTask 8.2: 将精英选择 `sorted.sort_by(...)` 改为 `select_nth_unstable_by(elite_count, ...)`
  - [x] SubTask 8.3: 运行 `cargo test -p gsoe-evolution` 验证精英选择结果一致

- [x] **Task 9: parliament/src/ahirt.rs:440 f32→f64 修复**
  - [x] SubTask 9.1: 读取 `crates/parliament/src/ahirt.rs` 第 430-460 行
  - [x] SubTask 9.2: 将 `(stats.detection_rate as f64) < threshold` 改为全程 f32 比较:`stats.detection_rate < threshold as f32`(或 threshold 本身声明为 f32)
  - [x] SubTask 9.3: 检查同文件其他 f32→f64 转换并修复
  - [x] SubTask 9.4: 运行 `cargo test -p parliament` 验证 AHIRT 阈值判断

- [x] **Task 10: faae-router/router.rs:316 Arc 共享 mutate 修复**
  - [x] SubTask 10.1: 读取 `crates/faae-router/src/router.rs` 第 60-100 行(字段定义)与 300-330 行(spawn_decay_loop)
  - [x] SubTask 10.2: 将 `edsb: EdsbBalancer` 字段改为 `edsb: Arc<EdsbBalancer>`(若需 mutate,内部加 Mutex)
  - [x] SubTask 10.3: line 316 改为 `let edsb = Arc::clone(&self.edsb);`
  - [x] SubTask 10.4: 运行 `cargo test -p faae-router` 验证 EDSB 衰减循环反映到原 router

- [x] **Task 11: faae-router/src/edsb.rs:364 引入 rand crate**
  - [x] SubTask 11.1: 在根 `Cargo.toml` `[workspace.dependencies]` 新增 `rand = "0.8"`
  - [x] SubTask 11.2: 在 `crates/faae-router/Cargo.toml` 添加 `rand = { workspace = true }`
  - [x] SubTask 11.3: 读取 `crates/faae-router/src/edsb.rs` 第 350-380 行
  - [x] SubTask 11.4: 将 `pseudo_random_probability` 用 `rand::random::<f64>()` 替换 `SystemTime` 纳秒
  - [x] SubTask 11.5: 运行 `cargo test -p faae-router` 验证 EDSB 概率均衡

- [x] **Task 12: CI workflows 加 timeout-minutes + RUST_BACKTRACE**
  - [x] SubTask 12.1: `.github/workflows/audit.yml` 加 `timeout-minutes: 30` 到 audit job
  - [x] SubTask 12.2: `.github/workflows/release.yml` build/test/docker/release jobs 各加 `timeout-minutes: 30`,build job `env:` 块加 `RUST_BACKTRACE: 1`
  - [x] SubTask 12.3: `.github/workflows/fuzz.yml` fuzz job 加 `timeout-minutes: 20`
  - [x] SubTask 12.4: 静态核验三个 workflow YAML 语法

- [x] **Task 13: OWASP A06 + A03/A10 测试加固**
  - [x] SubTask 13.1: 读取 `tests/security/owasp_top10.rs` A06/A03/A10 测试函数
  - [x] SubTask 13.2: A06 新增 `test_a06_dependency_version_assertions`:解析 `Cargo.lock`,断言 rusqlite ≥ 0.31 / tokio ≥ 1.40 / serde ≥ 1.0
  - [x] SubTask 13.3: A03 新增 `#[cfg(windows)] test_a03_windows_path_traversal`:验证 SecCore 拦截 `C:\Windows\System32\config\SAM`
  - [x] SubTask 13.4: A10 新增 `#[cfg(windows)] test_a10_windows_ssrf_powershell`:验证 SecCore 拦截 `Invoke-WebRequest`
  - [x] SubTask 13.5: 运行 `cargo test --test owasp_top10` 验证全绿

## 阶段三:P2 工程基建与文档同步

- [x] **Task 14: CI workflow 优化**
  - [x] SubTask 14.1: `audit.yml` 加 `Swatinem/rust-cache@v2` 缓存
  - [x] SubTask 14.2: `fuzz.yml` `cargo install cargo-fuzz` 改 `--locked`
  - [x] SubTask 14.3: `release.yml` 删除冗余 `'v1.0.0-omega'` tag 模式
  - [x] SubTask 14.4: `release.yml` test job 加 `if: startsWith(github.ref, 'refs/tags/v')`

- [x] **Task 15: figment 配置样例**
  - [x] SubTask 15.1: 创建 `examples/config.sample.yaml`,包含 figment 三源(默认 > File > Env > CLI)样例
  - [x] SubTask 15.2: 创建 `examples/config.sample.toml`,作为 TOML 替代样例
  - [x] SubTask 15.3: 在 `crates/chimera-cli/src/config.rs` 文档注释中引用样例路径

- [x] **Task 16: install.ps1 自动化 env 设置**
  - [x] SubTask 16.1: 读取 `install.ps1` 全文
  - [x] SubTask 16.2: 新增 `Set-Environment` 函数:检测 `.toolchain/cargo/bin` 是否在 PATH,若无则写入用户环境变量
  - [x] SubTask 16.3: 添加 `--setup-env` 开关参数,显式触发 env 自动化
  - [x] SubTask 16.4: 文档化"克隆即用"流程到 README

- [x] **Task 17: 占位实现文档化**
  - [x] SubTask 17.1: 读取 `README.md`,定位"已知限制"或新增章节
  - [x] SubTask 17.2: 列出 6 类占位实现:mtpe-executor pseudo_predictions / seccore asa rule-based / gsoe-evolution policy rule-based / nmc-encoder multimodal perceptors / repo-wiki placeholder_embedding / scc-cache InMemoryWal
  - [x] SubTask 17.3: 每项注明 v1.1 计划
  - [x] SubTask 17.4: 同步更新 `docs/release/v1.0.0-omega_release_notes.md`

- [x] **Task 18: nexus-core 补 proptest + benches**
  - [x] SubTask 18.1: 创建 `crates/nexus-core/tests/proptest.rs`:CLV 512-dim 余弦相似性切片不变量 / UserIntent risk_level 边界 / Quest 序列化往返
  - [x] SubTask 18.2: 创建 `crates/nexus-core/benches/clv_bench.rs`:CLV 编码 + 余弦相似性基准
  - [x] SubTask 18.3: 在 `crates/nexus-core/Cargo.toml` 添加 `[dev-dependencies] proptest` 与 `[[bench]]` 配置
  - [x] SubTask 18.4: 运行 `cargo test -p nexus-core` 与 `cargo bench -p nexus-core` 验证

- [x] **Task 19: nuxus规则.md §10.5 状态同步**
  - [x] SubTask 19.1: 读取 `.trae/rules/nuxus规则.md` §10.5
  - [x] SubTask 19.2: 将 "4 个 target_clippy*/ 残留" 条目改为 "已清理(2026-06-29 核验)"
  - [x] SubTask 19.3: 同步其他短板状态(Dockerfile RUST_BACKTRACE / env 自动化 / figment 样例)

## 阶段四:验收与文档同步

- [x] **Task 20: 全量验收 + 文档同步**
  - [x] SubTask 20.1: 运行 `cargo check --workspace` 验证零错误
  - [x] SubTask 20.2: 运行 `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 验证零警告(用 §9 clippy OOM 缓解)
  - [x] SubTask 20.3: 运行 `cargo test --workspace` 验证全绿(默认不含 #[ignore])
  - [x] SubTask 20.4: 运行 `cargo test -- --ignored --nocapture` 验证压测通过
  - [x] SubTask 20.5: 运行 `cargo fmt --all -- --check` 验证零 diff
  - [x] SubTask 20.6: 追加 `CHANGELOG.md` Week 8 RC 修复章节
  - [x] SubTask 20.7: 追加 `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md` 经验教训
  - [x] SubTask 20.8: 勾选 `checklist.md` 全部 checkpoint

## Task Dependencies

- Task 2 依赖 Task 1(Critical mpsc 通道先就绪,BudgetExceeded 才能受益)
- Task 11 依赖 Task 10(faae-router 字段重构先完成,再引入 rand)
- Task 13 依赖 Task 12(OWASP 测试加固与 CI workflow 优化无强依赖,但建议 CI 先就绪)
- Task 17 依赖 Task 14-16(占位实现文档化与基建优化无强依赖,但建议先完成基建再文档化)
- Task 20 依赖所有前置 Task 完成
- 阶段一(Task 1-5)、阶段二(Task 6-13)、阶段三(Task 14-19)内部 Task 大多可并行
