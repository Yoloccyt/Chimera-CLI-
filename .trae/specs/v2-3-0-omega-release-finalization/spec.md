# v2.3.0-omega 发布就绪与最终验证 Spec

> **change-id**: v2-3-0-omega-release-finalization
> **版本**: v2.3.0-omega
> **日期**: 2026-07-20
> **状态**: Active

## Why

项目已完成 v2.2.0-omega 的架构审计（Phase A）、TUI 收尾（Phase B）、治理规范化（Phase C），现需执行 Phase D 发布前检查清单，确保 v2.3.0-omega 满足所有质量门禁后推送 tag 触发 CI/CD 发布流程。

## What Changes

- 执行全量测试回归（cargo test/clippy/fmt/check）
- 运行压力测试（release + ignored）
- 验证 fuzz 配置正确性
- 构建 release binary 并验证体积
- Docker 镜像验证（降级）
- 更新 CHANGELOG.md 添加 v2.3.0-omega 条目
- 更新 Cargo.toml workspace.package.version = "2.3.0"
- 推送 v2.3.0-omega tag

## Impact

- Affected specs: 无（纯发布流程）
- Affected code: `Cargo.toml`（版本号）、`CHANGELOG.md`（新增条目）

## ADDED Requirements

### Requirement: 全量测试回归
系统 SHALL 通过 `cargo test --workspace` 全部测试，零失败。

#### Scenario: 全量测试通过
- **WHEN** 执行 `cargo test --workspace`
- **THEN** 所有测试通过，退出码为 0

### Requirement: 压力测试通过
系统 SHALL 通过 `cargo test --workspace --release -- --ignored --nocapture` 全部压力测试。

#### Scenario: 压力测试通过
- **WHEN** 执行 release 模式下的 ignored 测试
- **THEN** 所有压力测试通过，无性能退化

### Requirement: Clippy 零警告
系统 SHALL 通过 `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 零警告。

#### Scenario: Clippy 通过
- **WHEN** 执行 clippy 检查
- **THEN** 零警告，退出码为 0

### Requirement: 格式一致性
系统 SHALL 通过 `cargo fmt --all -- --check` 格式检查。

#### Scenario: 格式检查通过
- **WHEN** 执行格式检查
- **THEN** 所有文件格式一致，退出码为 0

### Requirement: 类型检查通过
系统 SHALL 通过 `cargo check --workspace` 类型检查。

#### Scenario: 类型检查通过
- **WHEN** 执行 cargo check
- **THEN** 零编译错误，退出码为 0

### Requirement: Fuzz 配置正确
系统 SHALL 通过 `cargo check --manifest-path fuzz/Cargo.toml` fuzz 配置验证。

#### Scenario: Fuzz 配置验证通过
- **WHEN** 执行 fuzz 配置检查
- **THEN** fuzz target 编译通过，退出码为 0

### Requirement: Release Binary 体积合规
Release binary SHALL 体积 < 50MB。

#### Scenario: Binary 体积检查
- **WHEN** 执行 `cargo build --workspace --release`
- **THEN** `target/release/chimera.exe` 体积 < 50MB

### Requirement: Docker 镜像验证
Docker 镜像 SHALL 通过 `scripts/verify_docker_locally.ps1` 降级验证。

#### Scenario: Docker 降级验证通过
- **WHEN** 执行 `scripts/verify_docker_locally.ps1`
- **THEN** 三级降级验证（Docker → Podman → 静态检查）通过

### Requirement: CHANGELOG 更新
CHANGELOG.md SHALL 包含 v2.3.0-omega 条目，汇总架构审计、TUI 收尾、治理规范化。

#### Scenario: CHANGELOG 条目存在
- **WHEN** 查看 CHANGELOG.md
- **THEN** 包含 `## v2.3.0-omega (2026-07-20)` 章节

### Requirement: 版本号同步
Cargo.toml workspace.package.version SHALL 为 "2.3.0"。

#### Scenario: 版本号正确
- **WHEN** 查看 Cargo.toml
- **THEN** `workspace.package.version = "2.3.0-omega"`