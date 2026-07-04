# Week 8 RC GA 前深度审计与修复 Spec

## Why

v1.0.0-omega 已进入 Stage 8 RC 阶段,34/34 crate 已实现、3002+ 测试全绿,但在 GA 发布前需进行一轮系统性的分布式深度审计,识别并修复影响发布质量的功能性缺陷、性能瓶颈、安全风险与工程基建短板。

本轮审计由 5 个精英专家子代理并行执行,覆盖 5 个正交维度(实现深度 / 测试体系 / 架构合规 / 性能与安全 / 工程基建),综合健康度 **80.4 / 100**,识别出 **5 个 P0 阻塞项**、**14 个 P1 高优项**、**11 个 P2 中优项**。本 Spec 仅纳入 RC 阶段允许的 bugfix / 安全加固 / 性能微调 / 文档同步范围内的修复项,不涉及核心领域类型变更或跨层重构。

## What Changes

### P0 阻塞项修复(GA 前必须完成)

- **event-bus Critical 事件双通道化**:为 `SkepticVeto` / `RedTeamAudit` / `AsaIntervention` / `BudgetExceeded` 4 类 Critical 事件增设 `tokio::sync::mpsc` 旁路通道,确保背压下不丢消息(违反 §6.2 红线)
- **decb-governor 集成 BudgetExceeded 事件发布**:补齐 `crates/decb-governor/src/error.rs:21` 标记的 TODO,使预算超限能通过 EventBus 广播
- **tests/stress/week7_stress.rs 5 测试加 `#[ignore]`**:避免日常 `cargo test` 跑 5×1000 次重测试
- **chimera-cli/src/commands/mod.rs:3 注释校准**:删除 "Stage 0" 过时注释,改为 "Stage 8 RC"
- **acb-governor / auto-dpo / chimera-tui 补 tests/ 目录**:满足"每 crate 必有 tests/"规则

### P1 高优项修复(GA 前强烈建议)

- **Dockerfile 加 `ENV RUST_BACKTRACE=1`**:panic=abort profile 下恢复线上 panic 诊断能力
- **repo-wiki/src/vector.rs:102 Top-K 改 select_nth_unstable**:违反 §4.1 硬约束,改 O(n) 算法
- **gsoe-evolution/src/engine.rs:255 Top-K 改 select_nth_unstable**:同上
- **parliament/src/ahirt.rs:440 f32→f64 修复**:全程 f32 比较,避免阈值边界误判
- **faae-router/router.rs:316 Arc 共享 mutate 修复**:改 `Arc::clone(&self.edsb)` 共享引用
- **faae-router/src/edsb.rs:364 引入 rand crate**:替换 `SystemTime` 纳秒为密码学安全随机源
- **CI workflows 加 timeout-minutes**:audit/release/fuzz 三个 workflow 防止 runner 挂起
- **release.yml build job 加 RUST_BACKTRACE env**:CI 失败可定位栈
- **OWASP A06 测试加固**:补 `Cargo.lock` 关键依赖版本断言
- **OWASP A03/A10 跨平台测试**:加 `#[cfg(windows)]` 分支

### P2 中优项(发布周期内补齐)

- **CI workflow 优化**:audit.yml 加 cargo 缓存 / fuzz.yml cargo-fuzz 锁版本 / release.yml tag 模式去冗余 / test job 加 if 守卫
- **figment 配置样例**:在 `examples/` 加 `config.sample.yaml` + `config.sample.toml`
- **install.ps1 自动化 env 设置**:解决新克隆者体验痛点
- **占位实现文档化**:mtpe-executor pseudo_predictions / seccore asa.rs / gsoe-evolution policy/* / nmc 多模态感知器 / repo-wiki placeholder_embedding / scc-cache InMemoryWal 在 README/release notes 显式声明为已知限制
- **nexus-core 补 proptest + benches**:CLV/UserIntent 核心不变量

### 不在本 Spec 范围(延后到 v1.1)

- chimera-cli L10 真实编排接线(C-1,需大量跨层调用代码,违反 RC 约束)
- nexus-core rusqlite 依赖下沉到 L3(架构 P3,需重构 L1 边界)
- seccore 沙箱 Windows/macOS 内核级隔离(ADR-001 完整实现)
- GSOE 真实强化学习策略接入
- NMC 多模态感知器 ONNX 接入
- MCP Mesh 2PC 真实跨进程通信

## Impact

### 受影响 specs

- `week1-8-global-deep-audit-and-remediation`(本轮深度审计接续其审计维度)
- `week8-production-release-hardening`(GA 发布前最后修复轮次)
- `week8-limitations-remediation`(部分占位实现状态需同步)

### 受影响代码

**Critical 修复(P0)**:
- `crates/event-bus/src/{bus.rs, backpressure.rs, types.rs}`(双通道化)
- `crates/decb-governor/src/{lib.rs, error.rs}`(BudgetExceeded 发布)
- `tests/stress/week7_stress.rs`(5 处 `#[ignore]`)
- `crates/chimera-cli/src/commands/mod.rs`(注释校准)
- `crates/{acb-governor, auto-dpo, chimera-tui}/tests/integration.rs`(新建 3 个)

**High 修复(P1)**:
- `Dockerfile`(RUST_BACKTRACE)
- `crates/repo-wiki/src/vector.rs`(Top-K)
- `crates/gsoe-evolution/src/engine.rs`(Top-K)
- `crates/parliament/src/ahirt.rs`(f32 精度)
- `crates/faae-router/src/{router.rs, edsb.rs, Cargo.toml}`(Arc 共享 + rand)
- `.github/workflows/{audit, release, fuzz}.yml`(timeout + RUST_BACKTRACE + cache + 锁版本)
- `tests/security/owasp_top10.rs`(A06 + A03/A10 跨平台)
- `Cargo.toml`(rand workspace 依赖)

**Medium 修复(P2)**:
- `examples/config.sample.{yaml, toml}`(新建)
- `install.ps1`(env 自动化)
- `README.md` + `docs/release/v1.0.0-omega_release_notes.md`(占位声明)
- `crates/nexus-core/tests/{proptest.rs, benches.rs}`(新建)

### 受影响文档

- `CHANGELOG.md`(追加 Week 8 RC 修复章节)
- `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md`(追加经验教训)
- `.trae/rules/nuxus规则.md` §10.5(`target_clippy*/` 残留条目已陈旧,需更新)

## ADDED Requirements

### Requirement: Critical 安全事件双通道投递保证

系统 SHALL 为 `SkepticVeto` / `RedTeamAudit` / `AsaIntervention` / `BudgetExceeded` 4 类 Critical 安全事件提供 mpsc 旁路通道,确保在 broadcast 背压(Lagged)场景下消息仍能送达关键订阅者(efficiency-monitor / parliament / decb-governor)。

#### Scenario: broadcast 背压下 Critical 事件不丢

- **WHEN** EventBus 的 broadcast channel 因慢消费者积压超过容量
- **AND** 发布者发布 `BudgetExceeded` 事件
- **THEN** broadcast 侧可能返回 `Lagged` 错误
- **AND** mpsc 旁路通道 MUST 成功投递该事件给至少一个 Critical 事件订阅者
- **AND** 订阅者能通过 `subscribe_critical_events()` 获取专用 mpsc Receiver

#### Scenario: 现有 broadcast API 向后兼容

- **WHEN** 现有调用方使用 `bus.subscribe()` 与 `bus.publish().await`
- **THEN** 行为保持不变
- **AND** 仅新增 `bus.subscribe_critical_events()` 与 `bus.publish_critical()` API

### Requirement: BudgetExceeded 事件链路完整性

`decb-governor` SHALL 在预算超限检测路径上通过 EventBus 发布 `BudgetExceeded` 事件,且事件 severity MUST 为 `EventSeverity::Critical`。

#### Scenario: 预算超限触发事件

- **WHEN** DECB 检测到当前预算消耗超过限额
- **THEN** 调用 `bus.publish(NexusEvent::BudgetExceeded { ... }).await` 或 `bus.publish_blocking(...)`
- **AND** 事件被 Critical mpsc 旁路通道接收

### Requirement: 测试体系合规化

每个 crate SHALL 拥有 `tests/` 目录(集成测试),`#[ignore]` 标记 SHALL 仅用于重测试(stress / bench / 长时运行)。

#### Scenario: stress 测试默认不执行

- **WHEN** 开发者运行 `cargo test --workspace`
- **THEN** `tests/stress/week7_stress.rs` 的 5 个 1000 次迭代测试 MUST 被跳过
- **AND** 仅在显式 `--ignored` 时执行

#### Scenario: Week 8 新 crate 集成测试齐备

- **WHEN** 检查 `crates/{acb-governor, auto-dpo, chimera-tui}/tests/`
- **THEN** 每个 crate 至少有 1 个 `integration.rs` 文件
- **AND** 包含至少 3 个跨模块集成测试

### Requirement: Top-K 选择算法合规

向量检索与精英选择场景 SHALL 使用 `select_nth_unstable_by`(O(n))而非 `sort_by` + `truncate`(O(n log n))。

#### Scenario: repo-wiki 向量检索 Top-K

- **WHEN** `repo_wiki::vector::VectorStore::search(query, top_k)` 被调用
- **THEN** 内部 MUST 调用 `select_nth_unstable_by(top_k, ...)` 取前 K
- **AND** 仅对 `scored[..k]` 做 K-log-K 排序(若需有序输出)

#### Scenario: GSOE 精英选择 Top-K

- **WHEN** `gsoe_evolution::engine::select_elites(reports, elite_ratio)` 被调用
- **THEN** 内部 MUST 使用 `select_nth_unstable_by(elite_count, ...)`

### Requirement: Arc 共享 mutate 状态正确性

异步任务若需 mutate 共享状态,SHALL 使用 `Arc::clone(&self.field)` 共享引用,而非 `Arc::new(self.field.clone())` 创建独立副本。

#### Scenario: faae-router decay_loop 共享 edsb

- **WHEN** `faae_router::router::FaaeRouter::spawn_decay_loop()` 启动后台任务
- **THEN** 任务持有的 `edsb` 引用 MUST 通过 `Arc::clone(&self.edsb)` 获取
- **AND** 后台对 edsb 的修改 MUST 反映到原 router 实例

### Requirement: 工程基建可观测性

发布产物 SHALL 在 panic 时提供 backtrace,CI workflows SHALL 设置超时上限。

#### Scenario: Docker 镜像 panic 可诊断

- **WHEN** 容器内 chimera 二进制 panic
- **THEN** stderr 输出 MUST 包含 backtrace(因 `ENV RUST_BACKTRACE=1`)

#### Scenario: CI workflow 超时保护

- **WHEN** audit.yml / release.yml / fuzz.yml 任意 job 挂起
- **THEN** 在 `timeout-minutes` 上限后 MUST 自动取消
- **AND** build/test job timeout = 30 分钟,fuzz job timeout = 20 分钟

### Requirement: OWASP 测试加固

OWASP A06 SHALL 包含依赖版本断言,A03/A10 SHALL 覆盖 Windows 平台攻击向量。

#### Scenario: A06 依赖版本断言

- **WHEN** 运行 `test_a06_vulnerable_components_*`
- **THEN** 测试 MUST 解析 `Cargo.lock` 并断言关键依赖版本(rusqlite ≥ 0.31, tokio ≥ 1.40, etc.)

#### Scenario: A03 Windows 路径遍历

- **WHEN** 在 Windows 平台运行 A03 测试
- **THEN** 测试 MUST 包含 `C:\Windows\System32\config\SAM` 等路径,验证 SecCore 拦截

### Requirement: 占位实现显式声明

所有已知占位实现(pseudo_predictions / asa rule-based / gsoe policy rule-based / nmc multimodal perceptors / repo-wiki placeholder_embedding / scc-cache InMemoryWal)SHALL 在 README 与 release notes 中显式声明为已知限制。

#### Scenario: README 已知限制章节

- **WHEN** 用户阅读 README.md
- **THEN** MUST 能找到"已知限制"章节
- **AND** 章节内列出全部 6 类占位实现及其 v1.1 计划

## MODIFIED Requirements

### Requirement: §10.5 已知基建短板清单

`nuxus规则.md` §10.5 中的 `target_clippy*/` 残留条目 SHALL 更新为"已清理"(经实际核查 `target/` 目录外无残留),其他短板状态保持同步。

## REMOVED Requirements

无。本 Spec 不删除任何已有需求,仅新增与修复。
