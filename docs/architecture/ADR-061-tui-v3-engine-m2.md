# ADR-061: TUI v3-engine M2 默认启用 — 自研渲染路径切换决策

## 状态

已接受 (2026-07-30)

## 背景

- ADR-029(v3.1 M0,2026-07-21)确立 `v3-engine` feature 双轨渐进迁移策略:M0-M1 引擎骨架默认编译供 CI 类型检查,后续里程碑逐步切换实际渲染路径。M0 已落地 `engine/` 五模块(rect/style/buffer/diff/writer)+ `v3-engine` feature + 2 benches(diff_engine / streaming,RED-first)。
- v2.9.0-omega Task 0.4 要求完成 M2 切换准备:feature flag 逐面板迁移。截至本 ADR 起草时,M0-M1 已通过 `cargo test -p chimera-tui` 全绿(507 lib + 集成测试 + doctest),`diff_engine_bench` < 100µs 阈值已达成,引擎骨架具备接入实际渲染路径的条件。
- 当前 `chimera-tui` 的 `render.rs` 使用 ratatui `Layout::split` + `frame.render_widget` 作为唯一渲染路径,自研引擎虽已存在但未实际接入,处于"骨架可用、未启用"状态。若长期保持此状态,引擎代码将沦为死代码,违背 ADR-029 决策 8 的"TDD 守恒 + CI 全绿 + 随时可回滚"渐进意图。
- 本 ADR 记录 M2 默认启用 v3-engine 的决策、风险评估与回退机制,作为后续 M3(完整 widget 替换)/ M4(组件系统)/ M5(移除 ratatui)的权威依据。

> **ADR 编号确认**: 现有最大编号为 ADR-060(chimera-cli ExitCode 矩阵)。本 ADR 编号确认为 **ADR-061**。

## 决策

### 决策 1: `v3-engine` feature 默认启用 — `default = ["v3-engine"]`

将 `crates/chimera-tui/Cargo.toml` 的 `[features]` 从 `v3-engine = []`(默认关闭)改为:

```toml
[features]
default = ["v3-engine"]
v3-engine = []
```

- **理由**: `default` feature 在 `cargo build` / `cargo test` / `cargo clippy` 时自动启用,确保 CI 与本地开发默认走 v3 路径;`--no-default-features` 仍可关闭用于 ratatui 路径回归测试。
- **影响范围**: 所有依赖 `chimera-tui` 的 crate(`chimera-cli` 经 workspace 依赖)默认获得 v3-engine feature,无需显式声明。

### 决策 2: M2 minimal 切换策略 — 布局引擎替换,widget 渲染保留 ratatui

M2 阶段**只切换布局计算后端**,widget 渲染仍委托 ratatui `frame.render_widget`:

- `render.rs` 新增 `v3_render(frame)` 与 `v3_layout(area)` 函数,使用 `engine::layout::flex::split` 替换 ratatui `Layout::split`。
- 面板 widget(QuestPanel / ParliamentPanel / ... 共 19 个)仍通过 `frame.render_widget` 渲染到 ratatui `Buffer`,**不**经 `DoubleBuffer` / `DiffEngine` / `TerminalWriter` 输出。
- `v3_render_widget(widget, area)` 函数(M0 骨架已声明)在 M2 阶段为 thin wrapper,内部仍调用 `frame.render_widget`,留待 M3 替换为真实双缓冲路径。

**理由**:
1. **TDD 守恒**: 现有 51 集成测试基于 ratatui `TestBackend` + `Buffer` 断言,若同时切换 widget 输出后端,会破坏全部渲染快照测试。M2 minimal 策略保证测试全绿,符合 ADR-029 决策 8。
2. **增量可证伪**: 布局引擎是自研引擎的核心创新点(Flexbox 约束求解),先切换布局可独立验证其正确性,与 widget 渲染解耦。
3. **M3 铺路**: M3 起 `v3_render_widget` 逐面板替换为 `DoubleBuffer` + `DiffEngine` 输出,M2 的 minimal 切换是必要前置。

### 决策 3: 双重回退机制 — env var + CLI flag

提供两层回退通道,确保生产环境可快速回滚:

1. **环境变量 `CHIMERA_NO_V3_ENGINE=1`**: 运行时回退,无需重新编译。`TuiApp::render` 通过 `v3_engine_disabled_by_env()` 检测此变量,若为 `1` 或 `true`(大小写不敏感)则走 `legacy_ratatui_render` 路径。适用于 CI/运维场景。
2. **CLI flag `--no-v3-engine`**: `chimera tui --no-v3-engine` 显式回退。`chimera-cli` 的 `commands/tui.rs` 在 TUI 启动前将 flag 转译为 env var 设置,适用于开发者手动验证或排查渲染问题。

**理由**:
- feature flag 是编译期决策,无法在已编译二进制中关闭;env var + CLI flag 提供运行时逃生舱。
- 双层设计覆盖不同用户群体:env var 面向运维(不修改命令),CLI flag 面向开发者(显式语义清晰)。
- 兼容窗口计划至 v2.11.0-omega 移除(2 个版本周期),与 ADR-060 `--legacy-exit-code` 的兼容策略一致。

### 决策 4: 51 集成测试不修改 — 双路径语义对齐

`v3_layout` 的布局语义与原 `layout()` 完全对齐:

| LayoutMode | 原 ratatui 路径 | v3 路径 | 语义一致性 |
|------------|----------------|---------|-----------|
| SinglePane | `[Rect::default(), area, Rect::default()]` | 同左 | ✅ 完全一致 |
| DualPane / VimSplit | tabs(3) + main(ratio%) + bottom(min 4) | 同左,经 `flex::split` 求解 | ✅ 完全一致 |
| TriplePane | tabs(3) + main(ratio×0.7) + bottom(min 4) | 同左,经 `flex::split` 求解 | ✅ 完全一致 |

**理由**: 布局求解结果在 ratatui `Rect` 层面完全一致,widget 渲染输入不变,因此 51 集成测试无需任何修改即可在 v3 路径下通过。这是 M2 minimal 策略的核心保证。

## 理由

- **调和"默认启用"与"TDD 守恒"**: 默认启用 v3-engine 是 ADR-029 渐进迁移的必然下一步,否则引擎代码将永远处于"骨架可用、未启用"的僵死状态;M2 minimal 策略(只换布局,不换 widget)确保切换风险最小化,51 测试全绿。
- **运行时回退能力**: 双重回退机制(env var + CLI flag)确保生产环境可快速回滚,无需重新编译发布,符合 §3.4.1 第 5 条"向后兼容"原则。
- **为 M3-M5 铺路**: M2 验证自研布局引擎在生产路径下的正确性后,M3 可逐步替换 widget 渲染为 `DoubleBuffer` + `DiffEngine`,M5 移除 ratatui 与兼容层,完成 ADR-029 决策 1 的终态目标。

## 影响

### 代码影响

- `crates/chimera-tui/Cargo.toml`: `[features] default = ["v3-engine"]`,注释更新引用 ADR-061。
- `crates/chimera-tui/src/app/render.rs`: 新增 `v3_render` / `v3_layout` / `v3_engine_disabled_by_env` 三个 `#[cfg(feature = "v3-engine")]` 函数;`render()` 方法增加 v3/legacy 分发逻辑;原 ratatui 路径重命名为 `legacy_ratatui_render`(语义不变,仅改名)。
- `crates/chimera-cli/src/cli.rs`: `Commands::Tui` 从单元变体改为结构体变体 `Tui { no_v3_engine: bool }`,新增 `--no-v3-engine` flag。
- `crates/chimera-cli/src/commands/mod.rs`: `dispatch` 传递 `no_v3_engine` 到 `tui::execute`。
- `crates/chimera-cli/src/commands/tui.rs`: `execute` 签名增加 `no_v3_engine: bool` 参数,启动前设置 `CHIMERA_NO_V3_ENGINE` env var。

### 测试影响

- **现有 51 集成测试**: 无需修改,默认走 v3 路径全绿(布局语义对齐,widget 渲染不变)。
- **新增单元测试**: 覆盖 `v3_engine_disabled_by_env` 的 env var 解析(`1`/`true`/`TRUE`/其他值)与 `v3_layout` 三种 LayoutMode 的布局正确性。
- **回归测试**: `cargo test --no-default-features -p chimera-tui` 验证 feature 关闭时走 ratatui 路径仍全绿。

### CI/发布影响

- `audit.yml` / `release.yml` / `fuzz.yml`: 无需修改,`default` feature 在 `cargo build` / `cargo test` 时自动启用。
- 5 平台 matrix build: 无影响,`v3-engine` feature 不引入新依赖,仅激活已有代码路径。
- Docker 镜像: 无影响,binary 体积变化 < 1KB(少量条件编译代码)。

## 风险与缓解

| 风险 | 严重度 | 缓解措施 |
|------|--------|---------|
| 自研布局引擎在边界场景(极小终端/超大终端)与 ratatui 行为不一致 | 中 | `v3_layout` 语义与 `layout()` 完全对齐(决策 4 表格);新增单元测试覆盖 1×1 / 80×24 / 200×50 三种尺寸;env var + CLI flag 可快速回退 |
| 双路径维护成本(2 个版本周期) | 低 | M2 minimal 策略下 `legacy_ratatui_render` 仅保留不动,无新增维护负担;M5 移除后双路径收敛为单路径 |
| 用户依赖 ratatui 特定行为(如 `Layout::split` 的边缘 case) | 低 | 布局语义对齐(决策 4)保证用户无感知;`--no-v3-engine` 提供显式回退 |
| `std::env::set_var` 在多线程下不安全(Rust 2024 edition) | 低 | 项目使用 Rust 2021 edition,`set_var` 为 safe;且 `execute` 在 TUI 启动前单线程调用,无并发风险 |

## 替代方案(被否决)

1. **一次性移除 ratatui,直接走完整 v3 路径**: 否决。违反 ADR-029 决策 8 的"TDD 守恒 + CI 全绿",51 集成测试会因 widget 渲染后端变更而全部失败,回滚困难。
2. **永不默认启用 v3-engine,保持 feature 关闭**: 否决。引擎代码将沦为死代码,违背 ADR-029 渐进迁移意图;且无法在 CI 默认路径下验证 v3 引擎的正确性,风险后置。
3. **仅 env var 回退,无 CLI flag**: 否决。用户体验差,需用户记忆环境变量名;CLI flag 提供更友好的显式语义,且 `--help` 可自动文档化。
4. **仅 CLI flag 回退,无 env var**: 否决。运维场景下修改命令行可能不便利(如 systemd unit / Docker entrypoint 已固化),env var 更适合 CI/运维。
5. **M2 同时切换布局与 widget 渲染**: 否决。风险过高,若 `DoubleBuffer` + `DiffEngine` 在生产路径下有性能或正确性问题,回退边界模糊;minimal 策略确保 M2 只验证布局引擎,M3 独立验证 widget 渲染。

## M2 落地状态(2026-07-30)

- ✅ `crates/chimera-tui/Cargo.toml`: `default = ["v3-engine"]` 已落地,注释引用 ADR-061。
- ✅ `crates/chimera-tui/src/app/render.rs`: `v3_render` / `v3_layout` / `v3_engine_disabled_by_env` 三函数实现,`render()` 双路径分发,`legacy_ratatui_render` 保留。
- ✅ `crates/chimera-cli/src/cli.rs`: `Commands::Tui { no_v3_engine: bool }` 结构体变体 + `--no-v3-engine` flag。
- ✅ `crates/chimera-cli/src/commands/mod.rs`: `dispatch` 传递 `no_v3_engine`。
- ✅ `crates/chimera-cli/src/commands/tui.rs`: `execute(cfg, no_v3_engine)` 签名 + env var 设置逻辑。
- ✅ 新增单元测试覆盖 env var 解析与 v3_layout 布局正确性。
- ✅ 验证:`cargo check -p chimera-tui` / `cargo test -p chimera-tui` / `cargo clippy -p chimera-tui --all-targets -- -D warnings` 全绿。

## 后续里程碑

- **M3**: `v3_render_widget` 逐面板替换为 `DoubleBuffer` + `DiffEngine` + `TerminalWriter` 输出,移除 `frame.render_widget` 调用。
- **M4**: 组件系统落地(类 React 组件树 + 生命周期 + diff 算法)。
- **M5**: 移除 ratatui 依赖与 `legacy_ratatui_render` 路径,移除 `--no-v3-engine` flag 与 `CHIMERA_NO_V3_ENGINE` env var,引擎成为唯一渲染路径。

---

> **维护者**:NEXUS-OMEGA 团队
> **文档版本**:ADR-061 起草(2026-07-30,v2.9.0-omega Task 0.4)
> **前置 ADR**:[ADR-029](ADR-029-tui-v3-interactive-refactor.md) v3.1 交互式重构
