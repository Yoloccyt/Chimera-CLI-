# Changelog

## v1.8.0-omega (2026-07-16)

**版本代号**: NEXUS-OMEGA (Enterprise TUI Monitoring · Task · Visualization)
**Spec**: [enterprise-tui-monitoring-task-viz](.trae/specs/enterprise-tui-monitoring-task-viz/spec.md)
**架构基线**: v1.7.0-omega → v1.8.0-omega(第三阶段深度演进)

### 主要变化

#### 🖥️ TUI 企业级监控套件(v1.8 P0-P3)

- **P0 设计手册** — `docs/architecture/NEXUS_OMEGA_TUI_DESIGN_BIBLE.md` 8 章编写完成
  - §1 设计哲学(5 原则) · §2 主题系统 · §3 交互范式 · §4 布局系统
  - §5 面板 API · §6 可视化组件 · §7 性能预算 · §8 扩展指南
  - 9 个核心类型导出(TuiBibleVersion/LayoutTemplate/ColorRole/KeyBinding/VizChartKind/PerfBudget/PanelApi/ExtensionHook/PanelRegistry)
- **P0 趋势图 + 阈值告警** — `ResourceMonitorPanel` 增强
  - CPU/内存/网络/磁盘 IO 四维度 5 分钟滑动窗口(300 样本)
  - 中位数滤波(5 样本窗口)去抖动
  - 阈值告警(70%/90%) + RGB 平滑颜色渐变(P4.1 优化)
  - 网络/磁盘 IO 子区域新增
- **P0 任务管理面板** — `TaskManagerPanel` 新增
  - Quest CRUD:P 暂停 / R 恢复 / T 终止 / +/- 优先级(0-10 强校验) / Enter 详情
  - 双向控制通过 `TuiCommand::QuestControl { id, action: QuestAction }`
  - 优先级桥接 0-10 → 0-255(`priority_255 = level * 25`)
- **P1 可视化组件库** — `viz/` 5 组件新增
  - `line_chart` / `heatmap` / `bar_chart` / `gauge` / `histogram`
  - 全部基于 ratatui Canvas/Chart,纯文本渲染,无外部图像依赖
  - 统一 `VizWidget` trait + `VizChartKind` 枚举
- **P1 指标仪表盘** — `MetricsDashboardPanel` 5×2 网格
  - 10 个 cell 可绑定任意 `TuiDataSource::snapshot()` 数据源
  - `bind(source, kind, position)` + `unbind(position)` 动态管理
  - `PanelId::MetricsDashboard` 18 变体全循环自洽
- **P1 历史持久化** — `MetricsHistory` SQLite 落地
  - 路径:`~/.chimera/metrics_history.sqlite`,表 `(unix_ts, metric, value)` 复合主键
  - `INSERT ... ON CONFLICT REPLACE` 幂等,启动时 `cleanup(retention_days)` 自动清理
  - 所有 rusqlite 调用 `tokio::task::spawn_blocking` 包裹(Week 7 教训)
  - WAL 模式 + `synchronous=NORMAL` 平衡一致性/性能
- **P2 系统信息** — `SysinfoPanel` 主机与进程信息
  - 主机信息:OS 内核 · CPU 型号/核心数 · 总内存 · 启动时间
  - 进程信息:Chimera PID · RSS · 线程数
  - 5s 周期刷新(可由 `sysinfo_refresh_interval_ms` 配置覆盖)
  - 跨平台 Windows/Linux/macOS(sysinfo 0.32 统一 API)
- **P2 配置加载器** — `TuiBible` Figment 4 源合并
  - 路径:`~/.chimera/tui_bible.yaml`(与既有 tui.yaml 区分)
  - 环境变量前缀 `CHIMERA_BIBLE_*`,嵌套字段用 `__` 分隔
  - 配置文件不存在 → 静默回退默认;YAML 损坏 → `TuiError::ConfigError`
  - `examples/config/tui_bible.sample.yaml` 含主题/颜色/键位/阈值/布局 5 段示例
- **P3 颜色渐变** — `gradient_color` RGB 三段线性插值
  - Green(#2ECC40) → Yellow(#FFDC00) → OrangeRed(#FF851B) → Red(#FF4136)
  - `is_finite()` 守卫 NaN 输入,`clamp(0, 100)` 边界外钳制
- **P3 三模式排序** — `TaskManagerPanel` 排序模式
  - `SortMode::Priority` 默认 · `Status` · `CreatedAt`
  - 'S' 键循环切换,面板标题动态显示当前模式
  - `created_at_index` 侧表自治追踪首次观察时间(不修改 L1 域类型)

#### 🔧 TuiConfig 5 字段扩展(向后兼容)

- `enable_trend_charts: bool`(默认 false,需显式开启)
- `metrics_sample_interval_ms: u64`(默认 1000,范围 [100, 60000])
- `metrics_history_retention_days: u32`(默认 7,范围 ≥ 1)
- `task_manager_default_sort: SortMode`(默认 Priority)
- `sysinfo_refresh_interval_ms: u64`(默认 5000,范围 ≥ 100)
- 全部 `#[serde(default)]`,旧配置零迁移加载

### 测试矩阵

| 测试套件 | 通过率 | 备注 |
|---------|--------|------|
| `color_gradient_test` | ✅ 11/11 | P4.1 新增 |
| `task_manager_test` | ✅ 10/10 | 5 原有 + 4 排序 + 1 公共 |
| `sysinfo_panel_test` | ✅ 4/4 | P3.1 |
| `tui_bible_config_test` | ✅ 3/3 | P3.2 |
| `metrics_history_persistence_test` | ✅ 3/3 | P1.3 |
| `viz_components_test` | ✅ 5/5 | P1.1 |
| `metrics_dashboard_test` | ✅ 3/3 | P1.2 |
| `trend_charts_test` | ⚠️ 8/9 | 1 视觉测试预存在失败,与本任务无关 |
| `resource_monitor_panel_test` | ✅ 4/4 | 零破坏 |
| `config_persistence_integration` | ✅ 3/3 | 零破坏 |
| lib 单元测试 | ✅ 426/426 | 零破坏 |
| **合计新增** | **+30+ 测试** | P0/P1/P2/P3 全覆盖 |

### 架构约束保持

- ✅ 全部新文件保持 `#![forbid(unsafe_code)]`
- ✅ L10 → L1 event-bus 依赖方向保持,无向上依赖
- ✅ Panel trait 7 方法签名零修改(`id`/`title`/`render`/`handle_key`/`handle_mouse`/`focus`/`scroll_to_*`)
- ✅ 核心领域类型未变更(UserIntent/Quest/Checkpoint/OmniSparseMasks/CLV/NexusState)
- ✅ nexus-core 仍保持最小依赖
- ✅ 所有 rusqlite 调用 `spawn_blocking`(无同步调用,§4.4 #2 红线)
- ✅ broadcast subscribe 模式正确(无 Week 6 事件丢失)
- ✅ Top-K 选择遵循 `select_nth_unstable` 约定
- ✅ sysinfo 0.32 已存在于既有依赖,无新增 crate-level 依赖

### 文档同步

- ✅ `docs/architecture/NEXUS_OMEGA_TUI_DESIGN_BIBLE.md` 新增(8 章)
- ✅ `docs/architecture/tui-suite-architecture.md` 新增(架构图)
- ✅ `docs/architecture/tui-api-impact-matrix.md` 新增(API 影响矩阵)
- ✅ `docs/architecture/tui-suite-tech-stack.md` 新增(技术栈评估)
- ✅ `examples/config/tui_bible.sample.yaml` 新增(配置样例)
- ✅ `Cargo.toml` workspace.package.version = 1.8.0-omega
- ✅ spec docs/architecture/INDEX.md 同步登记手册

### 升级路径(从 v1.7.0-omega)

1. **自动兼容**: `TuiConfig` 新增字段全部 `#[serde(default)]`,旧 `tui.yaml` 无需修改
2. **可选启用**: 趋势图默认关闭,`enable_trend_charts: true` 显式开启
3. **新配置文件**: `~/.chimera/tui_bible.yaml` 不存在时静默 fallback,与 `tui.yaml` 独立
4. **新面板**: `TaskManagerPanel` / `SysinfoPanel` / `MetricsDashboardPanel` 通过 `PanelId` 自动注册,无破坏既有焦点循环
5. **数据库**: `~/.chimera/metrics_history.sqlite` 自动创建,无需手工初始化

---

## v1.7.0-omega (2026-07-14)

**版本代号**: NEXUS-OMEGA (Evolved Interface)

### 主要变化

#### 🖥️ TUI 完整重构 (v1.7-omega Milestone M0-M6)

- **M0** — 接入 EventBus 实时数据流 (443a49c)
- **M1** — 重构为 Panel 架构，分离面板责任 (9b9c97f)
- **M2** — 5 系统监控面板交付 (70ed23d)
- **M3** — 增强交互：命令执行、搜索过滤、弹窗、鼠标与可调整布局 (e04e602)
- **M4** — 双向控制：TUI 通过 EventBus 发布控制请求 (1dfaf95)
- **M5** — TUI P1 验证与打磨 (9346d4a)
- **P2** — 5 监控面板完整交付 (49c22ef)
- **P3** — 交互能力升级 (2ca37ec)
- **P4** — 性能优化 (0b4a356)
- **P5** — 跨面板联动 (a267a6d)
- **P6** — 主题运行时切换与布局模板 (93bc535)

#### 🔒 P0 安全修复

- 清理 main/master 分叉 (af00fda)
- 移除 sqlite-vec 违规 unsafe 依赖 (af00fda)
- 升级 Dockerfile 基础镜像 rust:1.82 → 1.85 (af00fda)
- 添加 .gitignore .worktrees/ 排除 (af00fda)
- 添加 P0 验证脚本 `scripts/verify-p0-cleanup.ps1` (72fc20b)

#### 📦 安装与分发

- 新增 README.md 项目首页 (9cd6be8)
- 新增 Scoop 包管理器 manifest `bucket/chimela.json` (9cd6be8)
- 新增 Homebrew 包管理器 formula `Formula/chimela.rb` (9cd6be8)
- 新增 GPG 签名配置脚本 `scripts/setup-gpg-signing.ps1` (9cd6be8)
- 修复 PS 7.x 兼容性：改用 `[scriptblock]::Create()` (9cd6be8, 1da194d, 317e7ab)
- 统一品牌名为 chimela，消除 Release 下载 404 (9e3301b)

#### 📚 文档

- 更新发布指南至 v1.7.0-omega 精简版 (b6957a9)
- 强化 §9 代码修改前置思考与冗余代码杜绝规则 (eceafc6)
- 远程仓库文档清理 + 版本统一 (189c87b)
- TUI P3 验证归档 + v1.7-omega TUI 深化演进 spec 立项 (0f1d1a0)

#### ✅ 测试

| 类型 | 通过率 |
|------|--------|
| 单元测试 | ✅ 100% |
| 集成测试 | ✅ 100% |
| OWASP Top 10 | ✅ 20/20 |
| 压力测试 (1000 次) | ✅ 零失败 |

---

## v1.5.8-omega (2026-07-13)

- Cargo.lock 版本同步 + workspace 稳定性增强
- 发布物包含 Windows/Linux/macOS × x86_64/aarch64 五平台二进制

## v1.5.7-omega (2026-07-12)

- 首个含 GitHub Release artifacts 的版本
- 初始 5 平台 matrix 构建流水线

## v1.5.6-omega (2026-07-11)

- 持续集成与依赖更新

## v1.5.5-omega — v1.5.0-omega (2026-07-09 ~ 2026-07-11)

- MCP Mesh 量子网格迭代
- CSN 降级链完善
- 监控系统深化
- 集成测试体系建立

## v1.4.0-omega (2026-07-09)

- **架构跳跃版本**: L1-L10 全部 34 crate 功能完整
- SSRA Fusion、LSCT Tiering、GSOE Evolution、NMC Encoder、CHTC Bridge 五大 L2+L10 crate 接入
- MCP 量子网格原型
- E2E 测试体系建立

## v1.0.2-omega — v1.0.0-omega (2026-06-27 ~ 2026-06-28)

- **首周启动**: L0-L1 基础设施、Event Bus、SecCore、Decay、QEEP、CLI 入口
- L9+L5+L1: Quest Engine、Repo Wiki、Model Router
- L6: MLC、HCW、CMT、OSA、KVBSR
- L7: GEA、GQEP、PVL、MTPE、SCC
- L8+L4+L3: Parliament、ASA、AHIRT、TTG、DECB
- **v1.0.0-omega 初始发布** (2026-06-28): 34 crate 全覆盖，3000+ 测试全绿
