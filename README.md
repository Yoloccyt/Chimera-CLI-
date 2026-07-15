<p align="center">
  <img src="https://trae-api-cn.mchost.guru/api/ide/v1/text_to_image?prompt=aether+climbing+a+spiral+staircase+toward+a+sparse%2C+evolving+cosmic+nexus+in+dark+teal+and+amber%2C+sleek+minimalist+style%2C+tech+startup+aesthetic%2C+no+text%2C+high+contrast%2C+wide+format+landscape&image_size=landscape_16_9" alt="NEXUS-OMEGA" width="100%">
</p>

<h1 align="center">Chimera CLI — Aether (NEXUS-OMEGA)</h1>

<p align="center">
  <b>下一代 AI 编程智能体命令行工具</b><br>
  从四次工业级"尸检"与五大前沿模型架构中诞生的<br>
  免疫型 · 进化型 · 全维稀疏型 Agent 系统
</p>

<p align="center">
  <a href="#-一键安装"><img src="https://img.shields.io/badge/Windows-install.ps1-0078D4?logo=windows&logoColor=white" alt="Windows"></a>
  <a href="#-一键安装"><img src="https://img.shields.io/badge/Linux%2FmacOS-install.sh-FCC624?logo=linux&logoColor=black" alt="Linux/macOS"></a>
  <a href="#-docker"><img src="https://img.shields.io/badge/Docker-ghcr.io-2496ED?logo=docker&logoColor=white" alt="Docker"></a>
  <a href="#-源码构建"><img src="https://img.shields.io/badge/Rust-1.82%2B-dea584?logo=rust&logoColor=white" alt="Rust"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="License"></a>
</p>

---

## 项目简介

**Chimera CLI** — 代号 **NEXUS-OMEGA** (Omni-Model Engineering Generative Architecture)。基于 Rust 2021 + Tokio 异步运行时构建，遵循 **OMEGA 四定律**：

| 定律 | 工程实现 | 说明 |
|------|---------|------|
| **Ω-Sparse** | 全维稀疏掩码(工具/上下文/记忆/审计/预算) | 按需激活，零浪费 |
| **Ω-Compress** | 四级窗口 + 神经形态记忆 | 4K / 32K / 128K / 1M 分层上下文 |
| **Ω-Evolve** | GRPO 风格进化 + DPO 偏好优化 | 运行时自适应 |
| **Ω-Event** | 事件驱动架构 | Tokio broadcast 跨层解耦 |

核心组件：Event Bus 跨层通信 · SecCore 零信任沙箱 · 三层语义路由 · 14 面板终端仪表盘 · Merkle 审计链 · 能力衰减引擎 · 35 crates · 10 层架构。

---

## 📚 文档索引

新成员请优先阅读以下文档：

| 文档 | 路径 | 用途 |
|------|------|------|
| 架构文档索引 | [docs/architecture/INDEX.md](docs/architecture/INDEX.md) | 架构手册、设计文档、深研报告的统一入口 |
| 文件命名规范 | [docs/CONVENTIONS.md](docs/CONVENTIONS.md) | 文件命名与存放规则，新成员必读 |
| 项目规则 | [.trae/rules/nuxus规则.md](.trae/rules/nuxus规则.md) | 全局规则、架构硬约束、依赖铁律、async/SQLite/安全红线 |
| 项目命令 | [.claude/CLAUDE.md](.claude/CLAUDE.md) | 环境设置、常用命令、CI/CD 与发布、本地限制 |
| 版本演进史 | [CHANGELOG.md](CHANGELOG.md) | Week 1-8 验收记录 + v1.0.0-omega GA + 后续版本 |

### 缺失文档警示

- `CODE_WIKI.md` 与 `AETHER_NEXUS_OMEGA_ULTIMATE.md` 当前缺失，详见 [docs/architecture/INDEX.md](docs/architecture/INDEX.md) 缺失文档恢复建议。

---

## 一键安装

### Windows (PowerShell 5.1+)

```powershell
# 一行命令，适用于 PS 5.1 / PS 7+
$f="$env:TEMP\chimela-install.ps1";irm https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.ps1 -OutFile $f;& $f;ri $f -Force
```

### Linux / macOS (Shell)

```bash
# 一行命令
curl -fsSL https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.sh | sh

# 指定版本
curl -fsSL https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.sh | sh -s -- --version v1.7.0-omega
```

### Docker

```bash
docker pull ghcr.io/yoloccyt/chimera-cli:v1.7.0-omega
docker run --rm ghcr.io/yoloccyt/chimera-cli:v1.7.0-omega --version
```

### 验证安装

```bash
chimera --version   # 期望输出: chimera 1.7.0-omega
chimela --version   # 期望输出: chimera 1.7.0-omega (兼容别名)
aether --version    # 期望输出: chimera 1.7.0-omega (内部编码名)
```

---

## 基本使用

```bash
# 查看帮助
chimera --help

# 知识库检索
chimera wiki "查询关键词"

# 生成代码
chimera generate "任务描述"

# 启动 TUI 仪表盘 (14 面板实时监控)
chimera tui
# 或直接输入 chimera (无参数默认启动 TUI)
chimera

# 配置管理
chimera config init     # 初始化配置 ~/.chimera/omega.yaml
chimera config show     # 查看当前配置
```

---

## 平台支持

| 平台 | 架构 | 安装方式 |
|------|------|---------|
| Windows 10/11 | x86_64 | install.ps1 / Scoop |
| Linux | x86_64 / aarch64 | install.sh |
| macOS | x86_64 / Apple Silicon | install.sh / Homebrew |
| Docker | 多架构 (distroless) | `docker pull ghcr.io/...` |

---

## 源码构建

```bash
# 前置条件: Rust 1.82+、MinGW-w64 GCC (Windows)
git clone https://github.com/Yoloccyt/Chimera-CLI-.git
cd Chimera-CLI-

# Release 构建
cargo build --release -p chimera-cli

# 运行
./target/release/aether --version
```

---

## 开发状态

| 维度 | 状态 |
|------|------|
| 架构 | 10 层 · 35 crates · 129k+ 行 Rust |
| 测试 | 1142 单元测试 + 44 proptest + 26 benckmarks |
| 安全 | OWASP A01-A10 全覆盖 · `#![forbid(unsafe_code)]` |
| CI/CD | 8 条流水线 · 5 平台自动发布 · Docker distroless |
| 许可证 | MIT |

---

## 版本号规则

| 标签 | 语义 |
|------|------|
| `v1.7.0-omega` | 正式发布 |
| `v1.7.1-omega` | Bug 修复 |
| `v2.0.0-omega` | Breaking Change |

---

<p align="center">
  <b>NEXUS-OMEGA</b> — Ω-Sparse · Ω-Compress · Ω-Evolve · Ω-Event
</p>
