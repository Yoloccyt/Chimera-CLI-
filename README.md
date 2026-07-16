<p align="center">
  <img src="https://trae-api-cn.mchost.guru/api/ide/v1/text_to_image?prompt=aether+climbing+a+spiral+staircase+toward+a+sparse%2C+evolving+cosmic+nexus+in+dark+teal+and+amber%2C+sleek+minimalist+style" alt="Chimera CLI Banner" width="760" />
</p>

<h1 align="center">Chimera CLI — NEXUS-OMEGA</h1>

<p align="center">
  <a href="#-安装"><img src="https://img.shields.io/badge/Windows-install.ps1-0078D4?logo=windows&logoColor=white" alt="Windows"></a>
  <a href="#-安装"><img src="https://img.shields.io/badge/Linux%2FmacOS-install.sh-FCC624?logo=linux&logoColor=black" alt="Linux/macOS"></a>
  <a href="#-安装"><img src="https://img.shields.io/badge/Docker-ghcr.io-2496ED?logo=docker&logoColor=white" alt="Docker"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="License"></a>
</p>

---

## 项目概述

**Chimera CLI**（代号 **NEXUS-OMEGA**）是一个面向 AI 编程的 Rust 命令行智能体工具，基于进化型与稀疏化 Agent 架构。

### OMEGA 四定律

| 定律 | 价值 |
|------|------|
| **Ω-Sparse** | 全维稀疏掩码，按需激活工具/上下文/记忆/审计/预算 |
| **Ω-Compress** | 四级分层窗口（4K/32K/128K/1M），智能记忆管理 |
| **Ω-Evolve** | GRPO 风格运行时进化，DPO 偏好学习 |
| **Ω-Event** | Tokio broadcast + mpsc 双通道事件驱动 |

---

## 核心功能

- **全维稀疏激活**：零资源浪费的按需能力调度
- **分层上下文窗口**：动态适配任务复杂度的上下文管理
- **运行时自适应进化**：无需重新训练的在线优化
- **事件驱动架构**：跨层解耦的通信系统
- **安全沙箱**：零信任执行环境，能力衰减机制
- **TUI 仪表盘**：实时系统监控与交互界面


## 🚀 安装

### Windows (PowerShell)

```powershell
& ([scriptblock]::Create((Invoke-RestMethod https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.ps1)))
```

### Linux / macOS

```bash
curl -fsSL https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.sh | sh
```

### Docker

```bash
docker pull ghcr.io/yoloccyt/chimera-cli:v1.7.0-omega
docker run --rm ghcr.io/yoloccyt/chimera-cli:v1.7.0-omega --version
```

### 源码构建

```bash
git clone https://github.com/Yoloccyt/Chimera-CLI-.git
cd Chimera-CLI-
cargo build --release -p chimera-cli
./target/release/chimera --version
```

> **环境要求与配置**：详见 [CODE_WIKI.md](docs/architecture/CODE_WIKI.md) §10

---

## ⚡ 快速开始

> **命令别名**：`chimela`、`chimera`、`aether` 三者功能相同，可互换使用。

```bash
chimera --help          # 帮助
chimera                 # 启动 TUI 界面
chimera wiki "关键词"    # 知识库查询
chimera generate "任务"  # 代码生成
chimera config init     # 初始化配置
```

---

## 开发与构建

```bash
cargo check --workspace              # 类型检查
cargo build --release -p chimera-cli # 构建
cargo test --workspace               # 测试
cargo clippy --all-targets -- -D warnings  # Lint
cargo fmt --all                      # 格式化
```


---

## 📄 许可证

MIT License — [LICENSE](LICENSE)

---

<p align="center">
  <b>NEXUS-OMEGA</b> — Ω-Sparse · Ω-Compress · Ω-Evolve · Ω-Event
</p>