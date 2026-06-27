# Init Crates Workspace Spec

## Why
按 AETHER_NEXUS_OMEGA_ULTIMATE.md 6.2 节定义，初始化 NEXUS-OMEGA 项目的 Rust workspace 骨架：创建根 `Cargo.toml` 与 34 个 crate 子目录及各自的 `Cargo.toml`。

## What Changes
- 创建根 `Cargo.toml`（workspace 清单，含 34 个 member、`workspace.package` 元数据、`workspace.dependencies`）
- 创建 `crates/` 目录及其下 34 个子目录
- 每个子目录下创建最小 `Cargo.toml`（含 `[package]` 及对 workspace 依赖的引用）

## Impact
- Affected specs: 无（首次初始化）
- Affected code: 根目录、`crates/` 目录（全新创建）

## ADDED Requirements

### Requirement: Workspace Root Cargo.toml
系统 SHALL 在项目根目录创建 `Cargo.toml`，内容严格遵循 6.2 节 Step 1 的定义。

#### Scenario: 根 Cargo.toml 包含 34 个 workspace member
- **WHEN** 文件创建完成
- **THEN** `[workspace]` 的 `members` 数组包含全部 34 个 crate 路径
- **THEN** `resolver = "2"` 已设置

#### Scenario: 根 Cargo.toml 包含 workspace 级依赖
- **WHEN** 文件创建完成
- **THEN** `[workspace.dependencies]` 包含 tokio、serde、clap、ratatui、wasmtime、rusqlite、sqlite-vec、ndarray、reqwest、axum、prometheus-client、uuid、chrono、sha2、hex、once_cell、dashmap、criterion 等全部指定依赖

#### Scenario: 根 Cargo.toml 包含 workspace.package 元数据
- **WHEN** 文件创建完成
- **THEN** `version = "1.0.0-omega"`、`edition = "2021"`、`authors`、`license` 均已设置

### Requirement: 34 个 Crate 子目录及 Cargo.toml
系统 SHALL 在 `crates/` 下为每个 member 创建子目录及最小 `Cargo.toml`。

#### Scenario: 每个 crate 有独立子目录
- **WHEN** 初始化完成
- **THEN** `crates/` 下存在全部 34 个子目录

#### Scenario: 每个 crate 的 Cargo.toml 包含 package 声明
- **WHEN** 初始化完成
- **THEN** 每个 crate 的 `Cargo.toml` 包含 `[package]` 段，name 与目录名一致，version/edition 继承 workspace