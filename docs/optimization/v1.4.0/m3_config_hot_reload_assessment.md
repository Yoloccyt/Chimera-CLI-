# M3 配置热重载触发条件评估报告

> **评估日期**:2026-07-09
> **任务**:M3(P2 中期演进,条件触发评估)
> **关联 spec**:`.trae/specs/v1-3-0-omega-post-optimization-roadmap/`
> **基线版本**:v1.3.0-omega
> **评估对象**:`crates/chimera-cli/src/config.rs` LazyConfig + `crates/chimera-cli/src/cli.rs` CLI 命令树 + `crates/chimera-tui/`

## 1. 触发条件评估

| 触发条件 | 阈值 | 当前状态 | 是否触发 |
|---------|------|---------|---------|
| 用户明确请求运行时配置变更 | 是 | 无明确用户请求(本次为 spec 内置 P2 评估任务) | 否 |
| 长期运行服务消费配置 | 存在 daemon 模式 | **无 daemon 模式**(`cli.rs` Commands 枚举仅 Run/Tui/Quest/Config/Wiki/Parliament) | 否 |
| 交互式 TUI 消费 omega.yaml | TUI 读取主配置 | **TUI 不读取 omega.yaml**(`TuiApp` 仅消费 `TuiConfig`,与 `ChimeraConfig` 解耦) | 否 |
| 用户反馈热重载需求 | 有反馈 | 无反馈(无 GitHub issue / 无 project_memory 记录) | 否 |

**结论**:触发条件**未满足** → 继续延后,仅做评估,不启动实施

## 2. 当前状态分析

### 2.1 当前配置加载实现
- 实现:LazyConfig(`OnceLock` + `Figment::extract_inner` section 级懒加载)
- 加载时机:首次访问对应 getter 时按路径反序列化对应 section,未访问 section 零解析开销
- 配置变更:需重启 CLI 进程(`main.rs:43` `config::load(...)` 在进程启动一次性加载)
- 并发性能:p99 = 7.22μs(S1 验证,OnceLock 不成为瓶颈,13.8x 余量)
- 14 个顶层 section:nexus / quest / thinking_toggle / repo_wiki / model_router / osa / kvbsr / pvl / mtpe / gqep / seccore / mcp / evolution / monitoring

### 2.2 CLI 使用场景评估(基于 `cli.rs` Commands 枚举)
- `Run <prompt>`:单次任务,**一次性命令** → 不需要热重载
- `Tui`:启动交互式 TUI(长期运行,事件循环直到 q/Esc)→ 理论可能受益,但当前 TUI 是占位实现
- `Quest <list|show|cancel|checkpoint>`:**一次性命令** → 不需要热重载
- `Config <init|list|show|path>`:**一次性命令**(配置管理本身) → 不需要热重载
- `Wiki <query>`:**一次性命令** → 不需要热重载
- `Parliament <proposal>`:**一次性命令** → 不需要热重载

### 2.3 项目场景评估
- **daemon 模式**:**不存在**(`cli.rs` 无 `Daemon` 子命令,无后台服务模式)
- **TUI 模式**:存在(`chimera-tui` crate),但:
  - `TuiApp::panel_content()`(`app.rs:237-262`)为**硬编码占位字符串**,非真实业务数据
  - `TuiConfig`(`config.rs`)与 `ChimeraConfig` **完全解耦**,TUI 不消费 omega.yaml
  - 当前 TUI 不存在"运行中修改 omega.yaml 后 TUI 立即生效"的场景
- **用户反馈**:**无**明确热重载需求记录(project_memory.md / GitHub issues 均无)

## 3. 候选技术方案

### 3.1 notify crate(推荐,若未来触发)
- **优势**:跨平台(Linux/macOS/Windows),Rust 生态主流,无 `unsafe`,契合 `#![forbid(unsafe_code)]` 铁律
- **劣势**:需处理 debounce(避免频繁重载),需监听线程生命周期管理
- **评估**:适合跨平台热重载,符合项目跨平台需求(Windows/macOS/Linux)

### 3.2 inotify(Linux 专用)
- **优势**:Linux 原生,高性能
- **劣势**:仅 Linux,不符合项目跨平台需求(release.yml 5 平台 matrix)
- **评估**:不推荐

### 3.3 FsWatcher(macOS 专用)
- **优势**:macOS 原生
- **劣势**:仅 macOS
- **评估**:不推荐

### 3.4 保留重启加载(当前建议,未触发)
- **优势**:零复杂度,OnceLock 已验证性能足够(p99 = 7.22μs)
- **劣势**:配置变更需重启 CLI 进程
- **评估**:当前无热重载需求,保留重启加载即可

## 4. 建议与后续行动

### 4.1 当前建议(未触发)
- **继续使用 LazyConfig + 重启加载**,延迟评估到用户明确请求时
- OnceLock 已验证性能瓶颈不在此(13.8x 余量),无优化压力
- 项目当前阶段(v1.3.0 → v1.4.0+)无 daemon/TUI 实时配置消费场景

### 4.2 触发条件监控
- 关注用户反馈是否有热重载需求(GitHub issues / project_memory 记录)
- 若项目新增 daemon 模式(如 `aether daemon` 子命令),重新评估
- 若 TUI 从占位实现演进为真实业务消费 `ChimeraConfig`,重新评估
- 若用户明确请求,立即触发实施

### 4.3 实施前置条件(若未来触发)
- 新建独立 spec:`.trae/specs/v1-4-0-omega-config-hot-reload/`
- 候选方案:`notify` crate(跨平台 + 无 unsafe)
- 预估工时:40h+(spec 设计 4h + 实施 36h+)
- 技术要点:
  - `notify` crate 集成(dev-dependency)
  - debounce 逻辑(避免频繁重载,建议 500ms)
  - 配置变更通知机制(Event Bus 广播 `ConfigChanged` 事件,扩展 `NexusEvent` 枚举)
  - 线程管理(监听线程生命周期,与 CLI 主进程协调)
  - `LazyConfig` 重构:`OnceLock` → `RwLock<Arc<T>>` 或 `ArcSwap`(支持热替换)
  - 风险:`OnceLock` 不可变语义被破坏,需重新评估所有 getter 调用方对引用稳定性的假设

## 5. 关联文档

- v1.2.0 Task 4 OnceCell 报告:`docs/optimization/v1.2.0/task4_oncecell_verification_report.md`
- v1.3.0 S1 并发 bench 报告:`docs/optimization/v1.3.0/s1_concurrency_bench_report.md`
- v1.3.0 综合报告:`docs/optimization/v1.3.0/full_post_optimization_report.md`
- spec 路径:`.trae/specs/v1-3-0-omega-post-optimization-roadmap/tasks.md`(Task M3 行 138-142)
