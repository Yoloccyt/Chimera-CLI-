# Chimera CLI (NEXUS-OMEGA)

面向开发者的命令行 AI 编程智能体，代号 **NEXUS-OMEGA**。Rust workspace（多 crate），架构为 L0 契约层 + L1–L10 十层，依赖方向受铁律约束。

> 架构与计数的权威源不在本文件：架构看 `docs/architecture/CODE_WIKI.md`，版本演进看 `CHANGELOG.md`，crate 清单看 `Cargo.toml`（`workspace.members`），ADR 看 `docs/architecture/adr_index.md`。本文件不登记任何计数，避免漂移。

## 前置条件

- Rust 工具链（本仓库在 Windows 上使用 **GNU** 三元组 `stable-x86_64-pc-windows-gnu`，链接器为 msys2 MinGW gcc）。
- **本仓库刻意不提供 `rust-toolchain.toml`**（单值配置会破坏跨平台 CI）。Windows 上请跑一次环境注入脚本，而不是手工设变量：

```powershell
# PowerShell：注入 CARGO_HOME/RUSTUP_HOME/PATH 并设置 GNU channel（执行后需重启终端）
.\install.ps1 -SetupEnv
```

手动设置环境变量的等价写法（以及为什么不能提交 `rust-toolchain.toml`）见 `.claude/CLAUDE.md` §1。

## 构建与运行

```bash
cargo check --workspace            # 快速类型检查
cargo build --workspace --release  # 产物为 target/release/chimera(.exe)

# 二进制名是 chimera（不是 chimera-cli）：见 crates/chimera-cli/Cargo.toml 的 [[bin]] name
./target/release/chimera --version
cargo run -p chimera-cli -- --help
```

全部子命令的清单以代码为权威源：`crates/chimera-cli/src/cli.rs` 的 `Commands` enum。

## 安装

预构建产物由安装脚本负责（SHA256 校验 + PATH 注入 + `--version` 验证）：

```powershell
.\install.ps1        # Windows
```
```bash
./install.sh         # Linux / macOS
```

> ❗ `cargo install chimera-cli` **不可用**——原因是本仓库从未发布到 crates.io，**不是** `publish = false` 拦的：该键只写在根 `[package]`（`Cargo.toml:11`，包名 `chimera-e2e-tests`），既没进 `[workspace.package]`，`crates/chimera-cli/Cargo.toml` 也没有它 ⇒ 它**不覆盖** `chimera-cli`。要真正阻止误发布需把 `publish = false` 上提到 `[workspace.package]` 让全部成员继承（属仓库工件改动，登记待裁决，见 `docs/reports/doc-audit-ledger-P0-2026-09-20.md` 最新波次）。

## 测试与 Lint

```bash
cargo test --workspace --jobs 4                    # 16 核宿主用默认并发会 OOM，须限流
cargo test --workspace --release -- --ignored --test-threads=1   # 压测/性能红线必须 release 跑
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings   # --jobs 2 规避 Windows OOM
```

发布前的完整门禁清单（含 `cargo audit` 的 ignore 清单——条数以 `.github/workflows/audit.yml:65` 现取，不在本文件登记；fuzz 委托 Linux CI、Docker 三级降级验证）以 `.claude/CLAUDE.md` §2 与 §「发布前检查清单」为准；红线与 async 反模式清单见 `agents.md` §4.4 / §6.2。

## 已知待决事项

- **许可证口径不一致（需治理裁决）**：仓库根 `LICENSE` 文件正文是 **Apache-2.0**，而 `Cargo.toml` 的 `workspace.package.license = "MIT"`。两处矛盾，本文件不擅自择一；在统一之前，任何对外发布/合规声明都应先修正此冲突。

## 贡献

提交前至少跑：`cargo check --workspace --all-targets` + `cargo clippy … --jobs 2 -- -D warnings` + `cargo fmt --all -- --check`（Windows 本机若报 `os error 206`，按 `.claude/CLAUDE.md` 的逐包聚合法替代）。commit message 用 HEREDOC 传入并强制 UTF-8，避免中文乱码（教训见 `agents.md` §10.3a）。
