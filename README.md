# Chimera CLI / Aether CLI

[![Version](https://img.shields.io/badge/version-1.0.0--omega-blue.svg)](./CHANGELOG.md)
[![License](https://img.shields.io/badge/license-Apache--2.0-green.svg)](./LICENSE)
[![Workspace](https://img.shields.io/badge/rust_workspace-34_crates-orange.svg)](./Cargo.toml)

> 中文优先的仓库入口页。  
> Chimera CLI（内部二进制名 `aether`）是一个基于 Rust 的多模型 AI 工程化命令行系统，强调安全、可观测、可进化。

---

## 项目定位

Chimera CLI / Aether CLI 面向「多模型 AI 应用的工程落地」，核心目标是：

- 用 **10 层架构（L1-L10）** 管理复杂度
- 用 **事件驱动 + 审计链** 保证可追踪与可治理
- 用 **Rust workspace（34 crates）** 支撑模块化演进

如果你只读一个页面，请先看：
- [docs/README.md](./docs/README.md)（文档中心）
- [docs/architecture/README.md](./docs/architecture/README.md)（架构入口）

---

## 价值主张

- **工程安全**：`#![forbid(unsafe_code)]`、安全审计与策略治理
- **性能可控**：面向低延迟路由、缓存与执行链路优化
- **演进友好**：分层 crate 结构 + ADR 决策记录 + 发布/回滚文档
- **运维可观测**：指标监控、审计链、发布验证与验收报告齐备

---

## 快速开始

### 1) 环境要求

- Rust stable（仓库当前 `rust-toolchain.toml` 指向 `stable-x86_64-pc-windows-gnu`）
- 支持平台：Windows / Linux / macOS

### 2) 安装（概览）

```bash
# Linux / macOS
curl -fsSL https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.sh | sh
```

```powershell
# Windows
iex (irm https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.ps1)
```

更多安装与构建细节：
- [CHIMERA_NEXUS_COMPLETE_BUILD_GUIDE.md](./CHIMERA_NEXUS_COMPLETE_BUILD_GUIDE.md)
- [docs/release/release_guide.md](./docs/release/release_guide.md)

### 3) 本地验证

```bash
cargo +stable check --workspace
cargo +stable test --workspace
```

---

## 架构总览（简版）

```text
L10 Interface  → CLI/TUI/Bridge/MCP
L9  Quest      → 任务编排与策略切换
L8  Parliament → 治理与决策
L7  Execution  → 执行与融合
L6  Router     → 稀疏路由与协调
L5  Knowledge  → Wiki/进化/偏好优化
L4  Security   → 沙箱/协议/衰减治理
L3  Storage    → 缓存与分层存储
L2  Memory     → 上下文窗口与记忆引擎
L1  Core       → 核心类型/事件总线/模型路由
```

深入阅读：
- [docs/architecture/ten_layers.md](./docs/architecture/ten_layers.md)
- [docs/architecture/data_flow.md](./docs/architecture/data_flow.md)
- [docs/architecture/adr_index.md](./docs/architecture/adr_index.md)

---

## 文档导航（精选）

| 类别 | 入口 |
|---|---|
| 文档中心 | [docs/README.md](./docs/README.md) |
| 全量索引 | [docs/INDEX.md](./docs/INDEX.md) |
| 架构 | [docs/architecture/](./docs/architecture/) |
| 发布 | [docs/release/](./docs/release/) |
| 安全 | [docs/security/](./docs/security/) |
| 性能 | [docs/performance/](./docs/performance/) |
| 监控 | [docs/grafana/](./docs/grafana/) |
| 审计与验收 | [docs/audit/](./docs/audit/), [docs/acceptance/](./docs/acceptance/) |
| 开发与优化记录 | [docs/dev/](./docs/dev/), [docs/optimization/](./docs/optimization/) |

---

## 维护与版本

- 当前对外版本：`v1.0.0-omega`（详见 [CHANGELOG.md](./CHANGELOG.md)）
- 工作区结构：Rust workspace（见 [Cargo.toml](./Cargo.toml)）
- 许可证：Apache-2.0（见 [LICENSE](./LICENSE)）

如需从仓库入口继续深入，请从 [docs/README.md](./docs/README.md) 开始。
