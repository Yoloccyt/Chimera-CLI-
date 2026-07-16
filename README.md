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

---

## 🖥️ TUI 企业级监控套件 (v1.8.0-omega)

v1.8.0-omega 在既有 TUI 仪表盘之上引入企业级监控能力,覆盖趋势可视化、任务管理、指标仪表盘、历史持久化、系统信息与可配置主题六大维度。

### 趋势图与阈值告警 (ResourceMonitorPanel)

- **四维度监控**：CPU / 内存 / 网络 / 磁盘 IO,5 分钟滑动窗口(300 样本)
- **中位数滤波**：5 样本窗口去抖动,平滑短期抖动
- **阈值告警**：70% 警告 / 90% 严重,RGB 平滑颜色渐变(Green → Yellow → OrangeRed → Red)
- **启用方式**:`enable_trend_charts: true` 显式开启(默认关闭,向后兼容)

### 任务管理面板 (TaskManagerPanel)

- **Quest CRUD**：`P` 暂停 / `R` 恢复 / `T` 终止 / `+`/`-` 优先级调整(0-10 强校验) / `Enter` 查看详情
- **双向控制**：通过 `TuiCommand::QuestControl { id, action }` 桥接 L9 Quest Engine
- **三模式排序**：`Priority`(默认) / `Status` / `CreatedAt`,按 `S` 键循环切换,面板标题动态显示当前模式
- **优先级映射**：0-10 → 0-255 内部表达(`priority_255 = level * 25`)

### 5×2 指标仪表盘 (MetricsDashboardPanel)

- **10 个 cell 网格**：可绑定任意 `TuiDataSource::snapshot()` 数据源
- **动态管理**：`bind(source, kind, position)` 绑定 / `unbind(position)` 解绑
- **PanelId 全循环**：18 变体自洽,无破坏既有焦点循环

### SQLite 历史持久化 (MetricsHistory)

- **存储路径**:`~/.chimera/metrics_history.sqlite`,表 `(unix_ts, metric, value)` 复合主键
- **幂等写入**:`INSERT ... ON CONFLICT REPLACE`,启动时 `cleanup(retention_days)` 自动清理过期数据
- **WAL 模式**:`journal_mode=WAL` + `synchronous=NORMAL` 平衡一致性与性能
- **异步安全**:所有 rusqlite 调用经 `tokio::task::spawn_blocking` 包裹(遵循 §4.4 #2 红线)

### 系统信息面板 (SysinfoPanel)

- **主机信息**:OS 内核 · CPU 型号/核心数 · 总内存 · 启动时间
- **进程信息**:Chimera PID · RSS · 线程数
- **刷新周期**:5s 默认,可通过 `sysinfo_refresh_interval_ms` 配置覆盖
- **跨平台**:Windows / Linux / macOS 统一 API(sysinfo 0.32)

### TuiBible Figment 4 源配置

配置通过 Figment 合并四源(优先级由低到高):

1. 内置默认值
2. `~/.chimera/tui_bible.yaml`(与既有 `tui.yaml` 独立)
3. 环境变量 `CHIMERA_BIBLE_*`(嵌套字段用 `__` 分隔,例如 `CHIMERA_BIBLE_THEME__COLOR_ROLE=Dracula`)
4. CLI 参数

> 配置文件不存在时静默回退默认;YAML 损坏时返回 `TuiError::ConfigError`。

---

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
docker pull ghcr.io/yoloccyt/chimera-cli:v1.8.0-omega
docker run --rm ghcr.io/yoloccyt/chimera-cli:v1.8.0-omega --version
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

## ⚙️ TUI 配置示例 (v1.8.0-omega)

TUI 企业级监控套件通过 `~/.chimera/tui_bible.yaml` 配置(可选,不存在时静默回退默认值)。简单示例:

```yaml
# ~/.chimera/tui_bible.yaml (可选,不存在时静默回退默认)
theme:
  color_role: Dracula
thresholds:
  cpu_warning: 70
  cpu_critical: 90
```

完整配置项(主题 / 颜色 / 键位 / 阈值 / 布局)参见 `examples/config/tui_bible.sample.yaml`。环境变量前缀 `CHIMERA_BIBLE_*`,嵌套字段用 `__` 分隔(例如 `CHIMERA_BIBLE_THEME__COLOR_ROLE=Dracula`)。

`TuiConfig` 新增 5 字段全部 `#[serde(default)]`,旧 `tui.yaml` 无需修改即可加载:

| 字段 | 默认值 | 范围 / 说明 |
|------|--------|------------|
| `enable_trend_charts` | `false` | 显式开启趋势图 |
| `metrics_sample_interval_ms` | `1000` | `[100, 60000]` |
| `metrics_history_retention_days` | `7` | `≥ 1` |
| `task_manager_default_sort` | `Priority` | `Priority` / `Status` / `CreatedAt` |
| `sysinfo_refresh_interval_ms` | `5000` | `≥ 100` |

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