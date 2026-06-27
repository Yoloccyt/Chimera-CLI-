# Tasks

- [x] Task 1: 创建根 Cargo.toml（workspace 清单）
  - 严格按 6.2 节 Step 1 的完整内容写入 `Cargo.toml`
  - 包含 `[workspace]`、`[workspace.package]`、`[workspace.dependencies]`

- [x] Task 2: 创建 crates/ 目录及 34 个子目录，每个子目录包含最小 Cargo.toml
  - 创建 `crates/` 目录
  - 为 34 个 member 分别创建子目录和 `Cargo.toml`
  - 每个 crate 的 `Cargo.toml` 声明 `[package]`（name = 目录名, version.workspace = true, edition.workspace = true）

# Task Dependencies
- Task 2 可与 Task 1 并行执行（纯文件创建，无依赖）