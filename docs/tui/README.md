# Chimera TUI 用户手册

> 版本:v1.4.0-omega
> 范围:`chimera-tui` + `crates/chimera-cli/src/commands/tui.rs`

本手册说明如何启动和使用 Chimera 终端用户界面(TUI)。

## 启动 TUI

```bash
aether tui
```

CLI 会创建本地 `EventBus`,通过 `EventSubscriber` 订阅 `NexusEvent` 流,
并启动 `DataPipeline` 将事件聚合为面板可消费的统一数据快照。
退出后(TUI 内部会恢复终端状态),CLI 会清理数据管道后台任务。

## 面板概览

TUI 采用标签页+主面板布局,共 8 个面板:

| 编号 | 面板 | 说明 |
|------|------|------|
| 1 | Quest | 当前活动 Quest 列表、任务进度、思考模式 |
| 2 | Parliament | 议会投票、共识、Skeptic 否决等治理事件 |
| 3 | Budget | 预算档位、消耗、剩余、利用率与历史曲线 |
| 4 | Memory | 缓存命中率、上下文窗口、压缩率与历史曲线 |
| 5 | Security | Skeptic 否决、红队审计、ASA 干预、被冻结能力 |
| 6 | Health | 事件速率、慢消费者计数、MCP Mesh 平均延迟、健康评分 |
| 7 | Log | 最近 NexusEvent 流,支持关键字/主题/级别过滤 |
| 8 | Help | 快捷键速查 |

每个面板均通过 `Panel` trait 实现,新增面板无需修改主事件循环。

## 键盘快捷键

### 全局导航

| 按键 | 动作 |
|------|------|
| `Tab` | 下一个面板 |
| `Shift+Tab` | 上一个面板 |
| `1` ~ `8` | 直接跳转到对应编号面板 |
| `F1` | Quest |
| `F2` | Parliament |
| `F3` | Budget |
| `F6` | Memory |
| `F7` | Security |
| `F8` | Health |
| `?` | 显示 Help 面板 |

### 输入模式

| 按键 | 动作 |
|------|------|
| `:` | 进入命令模式(底部显示命令输入栏) |
| `/` | 进入搜索模式(设置全局关键字过滤器) |
| `Enter` | 提交当前命令/搜索 |
| `Esc` | 取消输入并返回普通模式;搜索模式下同时清除过滤器 |

### 布局与退出

| 按键 | 动作 |
|------|------|
| `Ctrl+Up` | 增大主面板占比 |
| `Ctrl+Down` | 减小主面板占比 |
| `q` / `Esc` | 退出应用(普通模式下) |

### 弹窗操作

| 按键 | 动作 |
|------|------|
| `Esc` / `q` | 关闭当前弹窗 |
| `Enter` | 确认当前弹窗(选中 Yes 时执行关联命令) |
| `Left` / `Right` | 在确认弹窗中切换 Yes/No |
| `Up` / `Down` | 滚动详情弹窗内容 |

## 鼠标支持

TUI 默认启用鼠标(可通过配置关闭):

- **点击标签栏**:切换到对应面板。
- **点击底部状态栏**:进入命令模式。
- **在主面板区域滚动**:当前焦点面板处理滚动(如 Log 面板)。
- **在弹窗上滚动**:滚动弹窗内容。

## 命令参考

在命令模式下(按 `:` 后)输入以下命令:

### 面板切换

| 命令 | 动作 |
|------|------|
| `quest` / `parliament` / `budget` / `memory` / `security` / `health` / `log` / `help` | 切换到对应面板 |

### 过滤器

| 命令 | 动作 |
|------|------|
| `find <keyword>` | 设置全局关键字过滤器(影响 Log / Quest 面板) |
| `filter <topic>` | 按主题过滤 Log 面板;合法主题:`quest`/`security`/`memory`/`health`/`parliament`/`budget`/`system` |
| `level <severity>` | 按严重级别过滤 Log 面板;合法级别:`info`/`warn`/`error`/`critical` |

### 控制命令

| 命令 | 动作 |
|------|------|
| `pause <quest-id>` | 请求暂停指定 Quest(弹出确认框) |
| `resume <quest-id>` | 请求恢复指定 Quest(弹出确认框) |
| `vote <yes|no|abstain> <proposal-id>` | 请求对指定提案投票(弹出确认框) |
| `refresh` | 请求上游刷新状态(非破坏性,直接发布事件) |
| `quit` | 退出 TUI |

## 控制命令与确认弹窗

破坏性控制命令(`pause` / `resume` / `vote`)在发布到 EventBus 前会弹出确认框,
避免误操作:

1. 输入命令,例如 `:pause q-123`。
2. TUI 显示确认弹窗,默认选中 **Yes**。
3. 按 `Enter` 执行,或按 `Right` 切换到 **No** 后按 `Enter` 取消。
4. 确认后,TUI 通过 `EventBus` 发布对应请求事件:
   - `QuestPauseRequested`
   - `QuestResumeRequested`
   - `VoteCastRequested`
5. 上游订阅者(如 `quest-engine`)消费请求事件并更新系统状态,
   状态变更再通过 EventBus 反馈到 TUI 面板,形成双向控制闭环。

`refresh` 为非破坏性命令,不会弹出确认框,直接发布 `RefreshStateRequested`。

## 架构速览

```text
EventBus ─publish─> EventSubscriber ─try_recv─> DataPipeline ─snapshot─> TuiApp.update()
                                                                        ↓
                                                            TuiState { quest_list, budget, memory_metrics,
                                                                      security_state, health_metrics,
                                                                      latest_events, ... }
                                                                        ↓
                                                            render() → Quest / Parliament / Budget / Memory /
                                                                       Security / Health / Log / Help
```

- `EventSubscriber`:1024 容量环形缓冲区,先 `subscribe` 再 `spawn`,溢出丢弃最旧事件。
- `DataPipeline`:250ms tick 批量消费,同 tick 内对 `QuestListUpdated`/`BudgetMetricsUpdated` 去重。
- `TuiApp.update()`:每帧从数据源拉取快照,失败时保留旧状态并在状态栏显示警告。

## 依赖与安全约束

- `chimera-tui` 位于 L10 Interface,仅依赖 L1 的 `event-bus` 与 `nexus-core`,不直接依赖 L9 业务 crate。
- 所有 crate 顶层声明 `#![forbid(unsafe_code)]`。
