# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> **版本控制说明**:本文件受 `.gitignore:55 .claude/` 政策管辖、不入 git 仓库,**文件内变更记录是唯一可追溯机制**。最后更新:2026-10-11(仓库治理批次:补版本控制说明;权威源指针未变)。历史要点:2026-10-11 与 `nuxus规则.md` v2.29.0-omega 纠漂批次同步;更早历史见 `CHANGELOG.md`。

## Chimera CLI (NEXUS-OMEGA) 项目指引

> 本文件面向未来操作本仓库的 Claude Code 实例。
>
> - **全局协作偏好**（语言、代码风格、Git 约束、工具使用偏好）见用户级 `C:\Users\30324\.claude\CLAUDE.md`。
> - **架构硬约束、依赖铁律、async/SQLite/安全红线**见 `.trae/rules/nuxus规则.md` §2、§4、§6。
> - **版本演进权威源**见 `CHANGELOG.md`;**架构权威源**见 `docs/architecture/CODE_WIKI.md`(crate 完整索引(§3.11 冻结孤岛清单;crate 数以 `Cargo.toml` `workspace.members` 为准,可达性由 `scripts/check_crate_reachability.sh` 现测)/核心类型/事件系统/依赖铁律,已与 `Cargo.toml` 对齐;MCA 体系 L10 `mca-gateway`,ADR-065~068;Concord TUI 重构 W0~W11 收尾,ADR-074~083;Phase 10 §16 v2.27.0;Phase 1-5 Ch12 W1-W26 v2.28.0 发布提交 af62e44 已落)。`CHANGELOG.md` 含 v1.0.0→[2.29.0-omega] 完整汇总 + ADR 摘要(ADR 索引权威源 = CODE_WIKI §11 与 `docs/architecture/adr_index.md`;物理文件最大主编号不登记,2026-10-11 现取已至 ADR-211,需要时 `ls docs/architecture/ADR-*.md` 现取)。
> - **协作流程规则**(分支模型 / 提交约定 / PR 合并 / 标签发布)见根目录 [CONTRIBUTING.md](../CONTRIBUTING.md)(2026-10-11 立,唯一权威源)。
> - **架构文档索引**见 `docs/architecture/README.md` 与 `docs/architecture/INDEX.md`(分类索引,条目与分类数以该文件自身为准,链接指向现存的 CODE_WIKI / ADR 等文档)。
>
> **当前基线(2026-09-27 核验)**:`Cargo.toml` `workspace.package.version = 2.29.0-omega`;**v2.29.0-omega 于 2026-09-27 本地发布(提交 + 本地 tag,未 push,故 release.yml/fuzz.yml 尚未触发)**——收口四维深审运行期质量批次 + 架构减法 A/B/P0 治理 + mlc L2 分层检索(ADR-192);上一发布 **v2.28.2-omega 于 2026-09-08 发布**(收口 09-05 治理批次 + TUI 四维评估三轮修复,高严重度问题 5→0;tag 推送触发 release.yml 5 平台矩阵 + fuzz.yml;完整回归由 release.yml test job clean-runner 承担,本地证据 = cargo check EXIT=0 + chimera-tui 定向 lib 868/868 + 24 目标 291/291);⚠ **双支线历史**:tag v2.28.2-omega → `a1b4616` 不在 main 祖先链上(merge-base `23a5a30`,2026-08-20),待治理,见报告 §5.51;main 祖先链上最后一个发布 tag = `v2.27.0-omega`(`bcf9d4d`,2026-08-19)（v2.28.0-omega → `94499b4` 同样不在 main 链上）;`CHANGELOG.md` 最新为 `[2.29.0-omega]` 正式发布条目;**crate 数以 `Cargo.toml` `workspace.members` 为准**(可达性/冻结孤岛由 `scripts/check_crate_reachability.sh` 现测,冻结清单 `scripts/crate_reachability_freeze.txt`;`mca-gateway` 经 ADR-177 重分类并已由 chimera-cli `optional` 边接线为 GATED);`NexusEvent` 变体数以 `crates/event-bus/src/types.rs` 为唯一权威(2026-10-11 实测 152,基线锁 `scripts/doc_count_freeze.txt`,勿在文档抄数);测试 passed 数不登记"当前值"——最近登记时点为 **11794 passed / 0 failed**(2026-09-17 M13 提交态,回写 `d3840ba`),更早时点 11587(2026-09-02,485 test target,出处 `docs/reports/redundancy-R9-disposition_2026-09-02.md`),架构减法与 v2.29 批次后的现势值以 `docs/reports/` 最新回归报告为准;**覆盖率体系**:测量引擎已由 tarpaulin 改为 **cargo-llvm-cov**(2026-09-05),`scripts/coverage_gate.py` 单点裁决,基线 `scripts/coverage_baseline.toml` min_line=85,CI 首跑实测 **92.97%**,coverage.yml 每晚阻塞门;CI 4 个 Rust job 已钉定工具链 **1.97.1**(2026-09-05,adb3517);项目当前焦点:**release.yml/fuzz.yml 发布流水线监控 + ADR-160 孤岛偿还(逐时点计数由门禁现测) + R2 解冻影子期(ADR-053 rev4 + ADR-054)+ MCA 双轨验证(`--features mca`)+ RL 开发闸门(Rust-First,Python 侧仅规划)**;26 周双轨计划 W1-W26 全部收尾(Phase 1-5,分片灰度全量 B 级,ADR-153 Go + ADR-157 双口径判定落档)。**⚠ 磁盘教训(2026-09-06)**:D 盘 275GB 上全量 debug 测试产物 20-30GB 级,本地全量回归客观不可行(曾爆盘至 100%),发布验证以 CI clean-runner 为准 + 本地 `cargo check --all-targets` 与核心层抽样。

## 1. 环境设置

仓库使用项目内 Rust 工具链。**首次配置的唯一权威方式**是运行一次安装脚本（执行后需重启终端）：

```powershell
.\install.ps1 -SetupEnv
```

它一步完成两件事，二者缺一不可：

1. 将 `CARGO_HOME` / `RUSTUP_HOME` / `PATH` / `TMP` / `TEMP` 写入用户级环境变量并同步当前会话；
2. 在项目本地 `RUSTUP_HOME` 内执行 `rustup default stable-x86_64-pc-windows-gnu`，选定 GNU 工具链 channel（详见 install.ps1 的 `Set-Environment` 函数）。

> **本仓库刻意不提供 `rust-toolchain.toml`**（原因见下方“工具链一致性”），因此 channel 只能由 `install.ps1 -SetupEnv` 保证；这是新克隆者选定正确 channel 的唯一入口。

若需了解 `-SetupEnv` 具体注入了什么，下面是等价的手动设置——但**手动块只设环境变量、不设工具链 channel**，仍须自行补一步 `rustup default stable-x86_64-pc-windows-gnu`，否则会落到本机默认宿主（通常 MSVC）导致 `linker=gcc` 失配：

```powershell
$env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
$env:TMP = 'D:\Chimera CLI\tmp'
$env:TEMP = 'D:\Chimera CLI\tmp'
$env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"
rustup default stable-x86_64-pc-windows-gnu   # 手动块必须补这一步（-SetupEnv 已内置）
```

> **工具链一致性**：`.cargo/config.toml` 已固化以下关键配置，因此日常开发**无需**再手动设置 `CARGO_INCREMENTAL` 或 linker：
>
> - `target.x86_64-pc-windows-gnu` 的 `linker = "gcc"`（从 PATH 解析 MinGW，避免硬编码路径在 CI 失效）
> - `[build] incremental = false`（根治 rustc 1.96.0 + GNU 的 ICE）
> - `[env] SQLITE_ENABLE_FTS5 = "1"`（启用 repo-wiki 的 FTS5 全文索引）
>
> **工具链 channel（GNU）由 `install.ps1 -SetupEnv` 保证**：它在项目本地 `RUSTUP_HOME` 内执行 `rustup default stable-x86_64-pc-windows-gnu`。**不要新增 `rust-toolchain.toml`**——它是全局单值：写 `stable` 会按本机“默认宿主”（通常 MSVC）展开成 MSVC 目标，令 `linker=gcc` 失效；写死 `stable-x86_64-pc-windows-gnu` 又会让 `release.yml` 的 Linux/macOS runner 无法安装该 Windows 宿主工具链而 CI 失败。

若 D 盘空间不足，可重定向构建临时文件：

```powershell
$env:CARGO_TARGET_DIR = 'E:\chimera-target'
$env:TMP = 'E:\chimera-tmp'
$env:TEMP = 'E:\chimera-tmp'
```

## 2. 常用命令

### 构建与运行

```powershell
# 快速类型检查
cargo check --workspace

# 只检查单个 crate
cargo check -p <crate-name>

# Release 构建（产物为 target/release/chimera.exe / chimera,与 CLI 命令入口一致）
cargo build --workspace --release

# 直接运行 CLI（二进制名是 chimera；子命令权威源 = crates/chimera-cli/src/cli.rs 的 Commands enum）
cargo run -p chimera-cli -- --version
cargo run -p chimera-cli -- help                 # 含 EXAMPLES 段
cargo run -p chimera-cli -- wiki "查询内容"
cargo run -p chimera-cli -- run "一次性代码生成/问答提示词"   # 注意：无 generate 子命令（旧文档误载，已实测确认）
cargo run -p chimera-cli -- --config ~/.chimera/omega.yaml wiki "查询"
```

### 测试

> **回归分级(2026-08-05 引入)**:快速回归档(PR 日常,nextest 并行 + 排除 E2E/doc-tests,~3-4min)与完整验收档(release/手动,`--ignored` 全量)。CI 已按此拆分(见 `ci.yml` check/e2e job)。

```powershell
# 快速回归(推荐日常使用):nextest 并行执行所有测试 binary,不含 doc-tests
# 与 E2E/压测 target;背压测试降为 5s(CHIMERA_BACKPRESSURE_SECS)
cargo nextest run --workspace --profile ci-fast --exclude chimera-e2e-tests -E 'not binary(/stress/)' --no-fail-fast
#   ↑ 与 PR 门禁**逐字同口径**（权威源 `.github/workflows/ci.yml` 的步骤 `Test (workspace, 快速回归)`）。三点必须知道：① `--profile ci-fast` 定义在 `.config/nextest.toml:44`，同文件 `:31` 记其目标为「总耗时较 baseline-full 减少 ≥30%」；② 快轨的 `CHIMERA_TEST_TIMEOUT_SCALE=0.1` **不是 profile 设的**，而是 CI 端注入（`.config/nextest.toml:37` 自述「依赖：CI 端设 CHIMERA_TEST_TIMEOUT_SCALE=0.1 收敛 2s+ 硬编码等待」）⇒ **本地不注入该变量即按 1.0 跑**，耗时与 CI 不可直接对比；③ stress 类靠 `-E 'not binary(/stress/)'` **按 binary 名**排除，**不是** test-group（该仓 `test-groups` 声明数为 0，`.config/nextest.toml:7-11` 已记该「描述了一个不存在的机制」的订正）。不带这两个参数即完整档，与 PR 快轨不可比。

# 完整验收(发布前 / 手动):含 doc-tests 与 60s 背压 spec 验收
cargo test --workspace --no-fail-fast   # 必须 --no-fail-fast：默认 fail-fast 会在首个红 test binary 中止后续 target(R2 M56)

# 单 crate
cargo test -p <crate-name>

# 单测试用例
cargo test -p <crate-name> <test-name>

# 显示测试输出
cargo test --workspace --no-fail-fast -- --nocapture

# 压力测试 / 性能红线（#[ignore] 标记,含 week7_stress 1000 次压测、
# stress_test 压测、hcw-window 性能红线、backpressure 60s spec 验收等）
cargo test -- --ignored --nocapture

# 单独跑根目录注册的 E2E / 安全 / 压测 / 控制闭环 target（下列为常用示例；
# 全部 target 名以根 `Cargo.toml` 的 [[test]] 注册为唯一权威源，本文件不登记清单与计数）
cargo test --test week5_event_flow
cargo test --test week6_setup
cargo test --test week6_main_flow
cargo test --test week6_security
cargo test --test week7_setup
cargo test --test week7_main_flow
cargo test --test week7_security
cargo test --test week7_stress
cargo test --test quest_lifecycle
cargo test --test full_integration
cargo test --test stress_test
cargo test --test week8_final_acceptance
cargo test --test owasp_top10
cargo test --test tui_control_loop
```

### Lint、格式与文档

```powershell
# clippy（Windows 本机建议限制并行 jobs 并关闭增量，避免 OOM/ICE；CI 侧 2026-09-05 起
# 4 个 Rust job 已钉定工具链 1.97.1，不再随 stable 浮动——新 clippy lint drift 由钉定消除）
$env:RUST_MIN_STACK = '33554432'
$env:CARGO_INCREMENTAL = '0'
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings

# 格式化
cargo fmt --all -- --check   # 检查
cargo fmt --all              # 应用

# 文档
cargo doc --workspace --no-deps
```

### 基准与安全审计

```powershell
# 单个 crate 的 criterion 基准
cargo bench -p <crate-name>

# 安全审计（必须与 .github/workflows/audit.yml 的 `Run security audit` 步骤逐字一致，6 个 ignore 为经评估确认不影响项目的间接依赖；
# 2026-08-07 升级修复后清理：RUSTSEC-2026-0217(tract-onnx 0.22.3)/RUSTSEC-2026-0222+0223(wasmtime 47.0.3；现锁 47.0.4) 已修复移除)
# 2026-08-28 P5-T5：--deny warnings 改为 --deny unmaintained --deny unsound —— 前者会被
# RUSTSEC-2024-0436(未维护但仅影响文档生成工具链)一类 warning 噪声直接卡死,反而诱使放宽整体门禁)
# 2026-09-02 新增 RUSTSEC-2026-0253(lru pop panic-safety 评估登记,ratatui→lru 键不 panic,无升级路径)
cargo audit --deny unmaintained --deny unsound `
  --ignore RUSTSEC-2026-0190 `
  --ignore RUSTSEC-2026-0002 `
  --ignore RUSTSEC-2026-0253 `
  --ignore RUSTSEC-2024-0436 `
  --ignore RUSTSEC-2025-0141 `
  --ignore RUSTSEC-2025-0119

# fuzz 配置静态检查（Windows GNU 无法实际运行，fuzz 委托 Linux CI）
cargo check --manifest-path fuzz/Cargo.toml
```

### Docker

```powershell
docker build -t chimera-cli:local .
docker run --rm chimera-cli:local --version
```

## 3. 架构总览

项目是基于 Rust 2021 + Tokio 的 workspace。`Cargo.toml` 的 `workspace.members` 为 **crate 数唯一权威源**(2026-09-28 现取 **39** crates;逐时点值不另立登记,演进史:v2.0.0-omega 新增 `chimera-mas` @ L9,v2.4.0-omega 新增 `nexus-contracts` @ L0 + `omega-learner` @ L6,v2.22.0-omega 新增 `mca-gateway` @ L10,v2.28.0-omega 新增 L10 `nexus-app-server`·L3 `session-store`·L9 `mas-sched`·`nexus-hook`·L7 `nexus-subagent`,批次 A 退役 `auto-dpo`/`model-router`;ADR-193 架构减法批次一退役 `router-traits`(41→40)、批次二将 `lsct-tiering` 整并入 `cmt-tiering`(40→39),台账已同步 `scripts/doc_count_freeze.txt` = 39)。**可达性口径由 `scripts/check_crate_reachability.sh` 现测**(冻结清单 `scripts/crate_reachability_freeze.txt`;dev-dep/feature 边不计入生产图;`mca-gateway` 经 ADR-177 重分类为 GATED)。

### 10 层架构

> 🆕 **TUI v3.1 交互式重构(ADR-029,已默认启用)**: `chimera-tui` 自研渲染引擎 + Action 统一协议 5 模块已实装(`actions/` / `engine/` / `components/` / `i18n/` / `input/`);**`v3-engine` feature 已默认启用**(`crates/chimera-tui/Cargo.toml` `default = ["v3-engine"]`,Concord 重构起为实际渲染路径)。`event-bus` append-only +8 事件变体(`TuiAction{Requested,Progressed,Completed,Failed}` + `TuiChat{Submitted,ResponseChunk,Completed,StatusChanged}`)+ 2 枚举(`ActionSource` / `ChatStatus`);commit `bf9aa75`(2026-07-21)。详见 [docs/architecture/ADR-029-tui-v3-interactive-refactor.md](docs/architecture/ADR-029-tui-v3-interactive-refactor.md) 与 [CHANGELOG.md](CHANGELOG.md) `v3.1.0-omega` 章节。

> 架构硬约束、ADR 权威源与依赖铁律的详细规则见 `.trae/rules/nuxus规则.md` §2。本节仅作快速导航。

```text
L0   Contracts ─ nexus-contracts                       (纯类型零依赖契约层,ADR-033)
L10  Interface ── chimera-cli · chimera-tui · chtc-bridge · mcp-mesh · csn-substitutor · mca-gateway · nexus-app-server
L9   Quest ───── quest-engine · gea-activator · efficiency-monitor · chimera-mas · mas-sched · nexus-hook
L8   Parliament ─ parliament · decb-governor
L7   Execution ── pvl-layer · gqep-executor · mtpe-executor · ssra-fusion · nexus-subagent
L6   Router ───── osa-coordinator · kvbsr-router · faae-router · sesa-router · omega-learner
L5   Knowledge ── repo-wiki · gsoe-evolution
L4   Security ─── seccore · qeep-protocol · decay-engine
L3   Storage ──── scc-cache · cmt-tiering · session-store
L2   Memory ───── nmc-encoder · hcw-window · mlc-engine
L1   Core ─────── nexus-core · event-bus
```

### OMEGA 十一定律

| 定律 | 工程实现 | 关键 crate |
| ---------- | ----------------------- | --------------------------- |
| Ω₁-Sparse | 全维稀疏掩码（工具/上下文/记忆/审计/预算） | `osa-coordinator`、`sesa-router` |
| Ω₂-Compress | 四级窗口 + 神经形态记忆 | `hcw-window`、`mlc-engine` |
| Ω₃-Evolve | GRPO 风格进化 + DPO 偏好优化 | `gsoe-evolution`、`chimera-mas`(原 `auto-dpo` crate 已物理删除,见 CODE_WIKI §3.12) |
| Ω₄-Event | 事件驱动架构(broadcast + Critical mpsc 双通道;变体数以 types.rs/doc_count_freeze 现取,2026-10-11 = 152) | `event-bus` |
| Ω₅-Credit | SHARP Shapley 值精确信用归因 | `parliament/sharp.rs`、`mappo.rs` |
| Ω₆-Reuse | 复用率优先,技能复用奖励 | `repo-wiki`、`csn-substitutor` |
| Ω₇-Locate | L1→L2→L3 行为定位,自动导航修改点 | `parliament/critical_path.rs` |
| Ω₈-Assess | Runtime Auditor 五维证据纪律 | `efficiency-monitor/auditor.rs` |
| Ω₉-Preserve | 变体隔离 + 停止策略,保留历史最佳 | `parliament/variant_pool.rs`（变体隔离，ADR-051）、`gsoe-evolution`（停止策略）、`chimera-mas`（INV-8/INV-9） |

### 依赖铁律

```text
L(N) → L(N)     ✓ 同层互引允许
L(N) → L(N-1)   ✓ 向下依赖允许
L(N) → L(N+1)   ✗ 向上依赖禁止
L(N) ──event-bus── L(M)  ✓ 跨层通信只能走 Event Bus
L(N) ──mcp-mesh─── L(M)  ✓ 跨进程通信只能走 MCP Mesh
```

- `nexus-core` 必须保持最小依赖，不能 import 上层任何 crate。
- 所有 crate 顶层均声明 `#![forbid(unsafe_code)]`；依赖库中的 unsafe 不破坏本项目 crate 的编译期保证。
- `dev-dependencies` 中的测试代码可绕过生产依赖方向，但仅限 `tests/` 目录。
- 任何违反依赖方向的 import 必须被拒绝，除非有 ADR 记录特批。

### 核心类型

- `CLV`（`nexus-core`）：512 维 f32 上下文潜在向量，NMC 编码器输出，被路由/记忆/激活组件消费。
- `NexusEvent`（`event-bus`）：跨层事件枚举，Critical 级事件无订阅者时记录 warn。
- `UserIntent` / `Quest` / `NexusState` / `OmniSparseMasks`：核心领域类型，修改需 ADR 记录。

### 端到端数据流示例

```text
用户输入 → NMC 编码(L2)
  → Quest 分解 / TTG 切换(L9)
  → Parliament 审议(L8) → Skeptic 否决检查
  → PVL 生产验证(L7)
  → OSA 协调(L6) → KVBSR/SESA 路由(L6)
  → GEA 激活(L9)
  → MTPE 多步预测 / GQEP 聚集执行(L7)
  → QEEP 纠缠(L4)
  → Wiki 沉淀 / GSOE 进化 / Auto-DPO(L5)
  → Event Bus 广播(L1)
```

> ⚠ 上图 `Auto-DPO(L5)` 的原 crate `auto-dpo` 已于批次 A（`38c72e6`，2026-09-20）物理删除（唯一登记表：`docs/architecture/CODE_WIKI.md §3.12`）；`DpoPairGenerated` 事件契约仍在(`crates/event-bus/src/registry.rs` 注册表 + `types.rs` 枚举,行号勿引),故事件面未断，但不要再按 L5 crate 去找它。其余三份规则/派生文档已含同一条注记，此处补齐。

### 前瞻性架构参考(第三阶段演进方向)

> 以下 5 份设计文档(1 份在仓库根,4 份在 `docs/architecture/_blueprints/`,落点权威源 `docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md` §4.3)为第三阶段模块级优化的**前瞻性参考**,完整规则见 `.trae/rules/nuxus规则.md` §3.4.5。

| 文档 | 核心价值 | 落地状态(2026-08-30 核验) |
|------|---------|---------|
| `CHIMERA_NEXUS_OMEGA_完整对话摘要.md` | 项目演进全景记录(6 轮 × 427,000+ 字) | ✅ 元文档(第七轮 Phase1-5 演进待补) |
| `docs/architecture/_blueprints/mas-quadrant/CHIMERA_MULTI_AGENT_四象限协同工作系统_系统性设计文档.md` | MAS-Q 设计蓝图(四象限 + 三层委托 + PDCA) | ✅ `chimera-mas` 全栈已实现(ADR-026~028,INV-7/8/9);v2.28 调度面拆出 `mas-sched`(ADR-145) |
| `docs/architecture/_blueprints/caf-router/CHIMERA_全球大模型亲和系统_终极设计文档.md` | CAF 渠道亲和(4 渠道 × 60+ 模型) | 🟡 MCA 部分落地(`mca-gateway` ADR-065~068;ADR-160 曾标注其 feature 门控孤岛,**2026-09-03 起 ADR-177 重分类为 GATED 并经 chimera-cli `optional` 边接线**);设计 v6 vs 实现增量仍在补齐 |
| `docs/architecture/_blueprints/triple-paradox/AI_Agent_三重悖论_x_Chimera_深度映射分析.md` | 三重悖论免疫(记忆/推理/进化) | 🟡 形式化验证器 M0/M1/M2 已落地(v2.13~v2.20,7 个 proptest 统计性验证器,聚合门 FormalVerifierGate,C2 口径订正);进化悖论 L4 门已接,R2 解冻影子期中 |
| `docs/architecture/_blueprints/three-ring-reorg/三环循环_十层接口_元架构重组深度分析.md` | 内环(9 crate) + 外环重组方案 | ✅ Phase 9 已交付(v2.24.0,P9-T1~T12,ADR-054);星型耦合解耦、2 条违规边消除 |

**三环重组核心约束(前瞻)**:
- 内环 9 crate(记忆+推理+进化)使用共享内存+直接调用,延迟 <1ms
- 外环 crate 保持事件驱动,只能通过 `CoreLoopEvent` 与内环通信
  > ⚠ **`CoreLoopEvent` 是设计名,当前代码零载体**(2026-09-22 实测:`crates/**/*.rs` 中 `CoreLoopEvent` 与
  > `inner_ring`/`InnerRing` 均 **0 命中**)。曾作为内环边界唯一强制点的治理脚本名单
  > `is_inner_ring()` / `is_inner_base()`(Check A)**已于 2026-09-24 随 ADR-187 从
  > `check_dependency_rules.{sh,ps1}` 双侧删除**——环分区现为 NOT-ENFORCED 设计意图,无任何机器强制;
  > 外环→内环方向改由 `scripts/check_outer_inner_ratchet.py`(G-75/G-76)只防增长。
  > 引入该类型或复活 Check A 属**待实现**,须另立 ADR(ADR-187 三前置);"内环 9 / 外环 25"为 v2.20 设计时点数。
- **关键约束**:内环 crate 不能依赖外环 crate;外环 crate 只能通过 event-bus 依赖内环公开接口

## 4. 关键工程约定

- **库层 vs 应用层错误**：库层使用 `thiserror` 自定义 enum(计数不登记:2026-10-11 实测 37 个 `crates/*/src/error.rs` / 39 成员(ADR-222 后 46 crates,+L0-B `nexus-telemetry` 与 6 个 L0-A sibling 契约 crate),缺员清单不抄写——router-traits 退役后旧"缺 4 个"枚举已过期);CLI/应用层使用 `anyhow`。`chimera-mas` 的 `MasError` 变体数以其 enum 现取。
- **配置加载**：`chimera-cli` 通过 Figment 合并**三个有效源**，优先级由低到高（权威源 `crates/chimera-cli/src/config.rs:219-221`）：
  1. 内置默认值（`Serialized::defaults(ChimeraConfig::default())`）
  2. YAML 配置文件（`Yaml::file(path)`；默认 `~/.chimera/omega.yaml`，`--config` 只改变该路径，见 `config.rs:43`）
  3. 环境变量 `CHIMERA_*`（嵌套字段用 `__`,例如 `CHIMERA_MODEL_ROUTER__STRATEGY=CostOptimized`；`Env::prefixed("CHIMERA_").split("__")`，`config.rs:221`）
     > ⚠ 段名必须取自真实 14 段（`nexus/quest/thinking_toggle/repo_wiki/model_router/osa/kvbsr/pvl/mtpe/gqep/seccore/mcp/evolution/monitoring`，
     > 模板见 `config.rs:253-418`）：顶层并无 `model` 段，旧示例 `CHIMERA_MODEL__PRIMARY` 不对应任何字段，照抄无效。

  > ⚠ **CLI 参数不进 Figment**（本条 2026-09-22 按代码更正）：`config.rs:216-218` 注释自陈"CLI 参数目前仅影响
  > config_path，未直接进入 Figment；后续可扩展 CLI override provider" ⇒ 此处旧版列的"第 4 源 = CLI 参数（最高优先级）"
  > 与实现不符，已删。若要按参数覆盖，现状只有 `CHIMERA_*` 环境变量一条路。
  > ⚠ 加载器**只注册 `Yaml` provider**（`config.rs:28`）⇒ `examples/config.sample.toml` 头部"复制为 `~/.aether/omega.toml`"
  > 两处不可达（目录名与格式都不是代码读取的口径），该样例文件待仓库工件修正裁决；`examples/config.sample.yaml` 现仅 9 字节（只有标题行），
  > 真正可用的样例是 `config.rs:253` 起的内嵌默认模板，落盘由 `chimera config init`（`commands/config.rs:32`）
  > 或 `chimera doctor`（`commands/doctor.rs:248`）触发——**不会在首次运行时自动写盘**（`init_config_file` 无其他调用点）。
  > **端到端实跑验证（2026-09-22 P115；在沙箱 HOME 下跑 `target/debug/chimera.exe`，跑完即删沙箱）**：
  > ① `chimera config init` rc=0，落 `.chimera/omega.yaml` **4,312 B 真配置**（非 examples 里那个 9 字节壳）；
  > ② `chimera config show` 读回该文件（`auto_decompose`、`max_tasks_per_quest` 等按文件值打印）；把 `max_tasks_per_quest: 20` 改成 `7` 后 show 即变 7 ⇒ **文件确实被消费，不存在「改了静默不生效」**；
  > ③ 把 YAML 写坏后 show **rc=1**，报 `ConfigError: 加载配置失败:<绝对路径>`（**点明是哪个文件**），与本文「退出码矩阵 1 = ConfigError」一致；Windows 下该路径带 `\?\` verbatim 前缀，属显示层而非缺陷，但**照抄去 grep 时要按字面匹配**。
  > ④ 生成模板首行的 `nexus.version` 值为 `1.0.0-omega`，与工作区版本脱节 ⇒ 见待决项 #29；该行**不得**当版本权威源。
- **二进制名称**：`crates/chimera-cli` 的产物名为 `chimera`(与 CLI 命令入口一致);本地直接运行请使用 `cargo run -p chimera-cli` 或 `target/release/chimera`。
- **占位实现**：
  - **v1.0.0-omega 起**:MTPE 伪预测 / SecCore ASA 规则式评分 / GSOE 规则式进化策略 / RepoWiki placeholder embedding / SCC InMemoryWal
  - **v1.x 完善**:NMC image/video/audio 感知器已实现
  - **v2.0.0-omega**:chimera-mas 3 子模块(`orchestrator` / `agent` / `context`)+ AgentTask wrapper(包装 `nexus_core::Task` 不修改核心类型)
  - **v2.1.0-omega**:chimera-mas 四象限 `Quadrant` 模型(Q1-Q4)+ WSJF `PriorityScheduler` + `ExpertRegistry`(E01-E08 内置)
  - **v2.2.0-omega**:chimera-mas Part II 7 子模块(§15-§21,INV-7 上下文预算界 / INV-8 归档单调性)+ 5 项 criterion bench
  - 替换为生产实现时不得破坏既有接口契约。

## 5. CI/CD 与发布

- **Tag 触发**:推送 `v*.*.*-omega` 标签自动触发 `.github/workflows/release.yml`(5 平台构建 + GitHub Release)和 `fuzz.yml`(**8 target × 300s**,已与 `fuzz/Cargo.toml` 同步)。
- **Security Audit**:`.github/workflows/audit.yml` 在改动 `Cargo.lock`/`Cargo.toml` 的 PR 上触发,并每日 UTC 02:00 运行。本地使用与 CI 一致的命令(见 §2"基准与安全审计")。
- **完整门禁面(2026-09-22 实测 11 份 workflow)**:除上述三份,还有 `ci.yml`(PR/push main:check+e2e+mca-feature+benchmark,含 typos 与 taplo fmt 门)、`deny.yml`(每日 UTC 05:00 + 改锁 PR:`cargo deny check licenses bans`)、`zizmor.yml`(每日 UTC 06:00 + 改 workflow PR:`--min-severity=high`)、`coverage.yml`(每晚 UTC 04:00:`scripts/coverage_gate.py --check`,阈值只在 `scripts/coverage_baseline.toml`)、`slo.yml`(tag + 每日 04:00)、`stress.yml`(每日 03:00)、`bench_check.yml`(改 bench/CLI 启动路径的 PR + tag)、`test-install-scripts.yml`(每日 04:00)⇒ 触发矩阵与准入逐份见 `agents.md` §10.1,本文件不复制表格。
- **版本号同步**:发布 `vX.Y.Z-omega` 前,`Cargo.toml` 的 `workspace.package.version` 必须同步为 `X.Y.Z`,且 `CHANGELOG.md` 必须存在对应的 `vX.Y.Z-omega` 汇总章节。**v2.3.0 → v2.3.1-omega 教训**:commit message 编码损坏时(见 nuxus规则.md §10.3a)不要 `--amend` / `push --force`,新 tag 递增 patch 即可。

### 本地辅助脚本(`scripts/`)

| 脚本 | 用途 |
|------|------|
| `check_doc_consistency.{ps1,sh}` | 架构文档一致性巡检(Cargo.toml members vs 磁盘 vs 索引文档) |
| `verify-p0-cleanup.ps1` | 分支清理 + 密钥扫描(7 类:AWS Key / GitHub Token / Private Key / Slack / npm / JWT / 硬编码凭据) |
| `setup-gpg-signing.ps1` | GPG 签名一次性密钥生成(RSA 4096 + 公钥/私钥导出 + 验证;解决 commit message 编码损坏,见 nuxus规则.md §10.3a) |
| `check_fuzz_config.{ps1,sh}` | fuzz 静态核验(8 项检查) |
| `verify_docker_locally.{ps1,sh}` | Docker 三级降级验证 |
| `cleanup_disk_space.ps1` | 磁盘清理(Diagnose/SafeClean/ProjectClean 三模式) |

### 发布前检查清单

> 先跑文档一致性巡检(以 `Cargo.toml` 为准):`pwsh -File scripts/check_doc_consistency.ps1`(Windows)或 `bash scripts/check_doc_consistency.sh`(Linux/macOS/CI),确保架构文档 crate 数与代码对齐。
>
> ⚠ **巡检执行面真相(2026-08-31 实测)**:`.gitignore:54-56` 排除 `*.md`/`.claude/`/`.trae/`,仓内 tracked 的 md 只有 1 个——即 `ci.yml` 里那道巡检在 CI 的 checkout 下**看不到 CHANGELOG/CODE_WIKI/ADR 本体**,B1/B2/C1/D2/E1/E2 全部走 warn 分支不计失败,真正生效的只有 A1(members vs 磁盘)与 A2(version 字段存在)。因此**文档一致性以本地 `.ps1` 为权威执行体**(本清单第 2.5 项),不得把“CI 绿”当作“文档已核验”。口径重划见 ADR-166。

1. 确认 `Cargo.toml` 的 `workspace.package.version` 与待发布 tag 一致。
2. 确认 `CHANGELOG.md` 已存在对应的 `vX.Y.Z-omega` 汇总章节。
2.5. `pwsh -File scripts/check_doc_consistency.ps1` 本地全 PASS(**检查数由脚本末尾自报**，形如 `6 categories / N check ids, self-reported`；勿在本清单写死数字——2026-08-31 实测该数字曾被复制到 5 处文档，每加一条检查就同时使 5 处陈旧)。本清单唯一能真正看到文档本体的环节。
2.6. `python scripts/audit_cmd_sync.py` 退出 0（CI 与两份规则文档的 `cargo audit` 口径逐字一致；文件缺失时退 2 判不可判定，不得视为通过。见 ADR-167）
3. `cargo check --workspace --all-targets`
4. `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings`（本地 Windows GNU 缓解口径;CI 已钉定工具链 1.97.1,2026-09-05 起）
5. `cargo fmt --all -- --check`(⚠ Windows 本机因长路径前缀会报 `os error 206`;此时**逐包聚合**等价验:`cargo fmt -p <pkg> -- --check` 对全部 crate(枚举见 `Cargo.toml` `workspace.members`)+ 根 package 逐一跑,任一非 0 即未通过)
6. `cargo test --workspace --no-fail-fast --jobs 2`(⚠ 16 核宿主不限流会编译面 OOM,报 `memory allocation failed` + 派生 `E0786 invalid metadata`,易被误判为代码回归)
7. ``cargo test --workspace --release --no-fail-fast -- --ignored --test-threads=1`（**release 模式必需**：`#[ignore]` 基准测试含性能阈值，debug 模式开销 + 并行竞争会污染延迟测量导致 flaky 误报；逐项归属见 `scripts/ignored_test_inventory_freeze.txt`，规则级说明见 `AGENTS.md` §7.2）
8. `cargo audit --deny unmaintained --deny unsound --ignore RUSTSEC-2026-0190 --ignore RUSTSEC-2026-0002 --ignore RUSTSEC-2026-0253 --ignore RUSTSEC-2024-0436 --ignore RUSTSEC-2025-0141 --ignore RUSTSEC-2025-0119`（与 `.github/workflows/audit.yml` 的 `Run security audit` 步骤逐字一致；P5-T5 2026-08-28 起已不用 `--deny warnings`；2026-09-02 新增 RUSTSEC-2026-0253 lru pop panic-safety 评估登记）
8.5. `bash scripts/check_perf_redlines.sh --selftest && bash scripts/check_perf_redlines.sh --static-only`（红线存在性 + bench 三态清单 + 数据表自证 + `#[ignore]` 归属门;**条目数以门自报为准,不在此抄数**——2026-09-21 曾测 bench 文件 116、ignored total 70;CI 已在 `check` job 阻塞执行）
9. `cargo check --manifest-path fuzz/Cargo.toml`（Windows-GNU 下通过 `fuzz/src/lib.rs` stub 宏方案验证 fuzz 逻辑语法；**✅ 2026-09-23 实测 EXIT=0（`Finished dev profile in 1m 04s`，0 error）**——原阻断项已消化：`fuzz/Cargo.toml` 悬空边已删，`fuzz/fuzz_targets/cacr_budget_parse.rs` 三个模糊目标整体迁到活体后继面 `mca-gateway`（`spec_loader::parse_profile_toml`/`parse_spec_toml` + `cost_guard::CostGuard`），`[[bin]]` 数仍为 8 故 `fuzz.yml` matrix 未动；变更说明见 `docs/reports/SAB-B0-2-impl-maturity-gate-2026-09-23.md §14`，`agents.md` §10.3 同条目已于 2026-09-24 同步）：`#[cfg(windows)] use chimera_fuzz::fuzz_target` + `#[cfg(not(windows))] use libfuzzer_sys::fuzz_target`）；可选运行 `scripts/check_fuzz_config.ps1`（Windows）或 `scripts/check_fuzz_config.sh`（Linux/macOS）静态验证配置完整性
10. `docker build -t chimera-cli:rc .`
11. `docker run --rm chimera-cli:rc --version` 输出匹配 `chimera [0-9]+\.[0-9]+\.[0-9]+`
12. `docker image inspect chimera-cli:rc --format '{{.Size}}'` 确认镜像体积 < 100 MB
    > **本地无 Docker 时的降级验证**：可运行 `scripts/verify_docker_locally.ps1`（Windows）或 `scripts/verify_docker_locally.sh`（Linux/macOS）。脚本自动检测 Docker → Podman → 降级验证（Dockerfile 静态检查 + release binary 体积 < 50MB 代理指标 + CI 状态查询引导）。完整镜像验证由 `release.yml` docker job 在 tag 推送时自动执行，详见 §6。
13. `cargo build --workspace --release` 确认 binary 体积 < 50 MB
14. `git tag v<x.y.z>-omega && git push origin v<x.y.z>-omega`

> 规则级发布要求与详细说明见 `AGENTS.md` §7.2(“发布前检查清单(替代周验收)”)。
>
> ⚠ 旧稿此处指向 `.trae/rules/nuxus规则.md §7.2`,但该文件 §7 是 1-9 扁平清单、**无 §7.2 子节**(2026-08-31 实测),属挂空锚点。同一病根(引用存在但目标不存在)亦见于本文§5 对 CI 文档巡检覆盖度的描述、以及 `check_perf_redlines` “已在 CI 装配”的旧声明。

## 6. 本地无法运行的限制

- **cargo-fuzz**：libFuzzer 的 Windows 适配依赖 MSVC，本项目 Windows 环境使用 MinGW，因此 fuzz 只能委托 Linux CI。`fuzz/Cargo.toml` 声明了 **8 个 fuzz target**（quest_parse/seccore_sandbox/event_serialize/cacr_budget_parse/checkpoint_deserialize/config_section_parse + harness_spec_parse/sse_parse），`fuzz.yml` CI 已同步至 8 target。Windows-GNU 下通过 `fuzz/src/lib.rs` stub 宏方案（`chimera_fuzz::fuzz_target` 替代 `libfuzzer_sys::fuzz_target`）使 `cargo check --manifest-path fuzz/Cargo.toml` 能通过语法验证。可选运行 `scripts/check_fuzz_config.{ps1,sh}` 静态验证配置完整性。实际 fuzz 运行委托 Linux CI（`.github/workflows/fuzz.yml`）。
- **Docker 镜像构建**：本地未安装 Docker Desktop 时，发布前检查清单第 10-12 项的镜像验证可降级为 `scripts/verify_docker_locally.ps1`（Windows）/ `scripts/verify_docker_locally.sh`（Linux/macOS）。脚本三级降级：Docker → Podman → Dockerfile 静态检查 + binary 体积代理 + CI 引导。完整镜像验证由 `release.yml` docker job 在 tag 推送时自动执行。
- **CI Windows GNU 构建**：`release.yml` 的 Windows job 使用 `windows-latest` + GNU target，但 runner 默认没有 `D:/msys64/mingw64/bin/gcc.exe`；若 Windows release build 链接失败，需要检查是否已安装 MinGW 或改用与本地一致的环境验证。

## 7. 合并/大改后的自检与 `check_errors*.txt`

仓库根目录可能出现 `check_errors*.txt`（如 `check_errors.txt`、`check_errors2.txt`、`check_errors3.txt`）。这些文件是历次 `cargo check --workspace` 的错误日志，没有元信息说明对应提交。处理原则：

- **提交前**：必须确保 `cargo check --workspace` 通过，不得遗留未归档的 `check_errors*.txt`。
- **出现** **`E0428`（重复定义）或** **`E0252`（重复导入）时**：优先检查是否把同一段代码粘贴了两次（常见于合并/大段移动）。
- **生成规范**：如需本地保存日志，使用带时间戳的文件名，例如 `check_errors_$(date +%Y%m%d_%H%M%S).txt`。

## 8. 必须遵守的编码红线(摘要)

以下规则的权威版与详细出处见 `.trae/rules/nuxus规则.md` §4、§6:

**async / 并发(Week 1-8 实战)**:
- 禁止持锁跨 `.await`;`rusqlite` 调用必须 `spawn_blocking`;`tokio::broadcast` 必须先 `subscribe` 再 `spawn`
- Top-K 必须 `select_nth_unstable` (O(n)),禁止 `sort_by` (O(n log n))
- 并发收集用 `FuturesUnordered` 而非 `join_all`

**内存与依赖**:
- 所有 crate 顶层 `#![forbid(unsafe_code)]`;依赖库 unsafe 不破坏当前 crate 保证
- 风险规则列表为空时返回 `RiskLevel::Unknown`;`PRAGMA journal_mode=WAL` 必须用 `pragma_update`
- u64 大数百分比计算用 `f64` 中间值;stable Rust 无 `AtomicF32`

**多治理器 / 多 Agent(v2.x 新增)**:
- 多治理器协同必须经 `ArbitrationLayer` 取保守值
- **Critical 事件**(SkepticVeto / RedTeamAudit / AsaIntervention / BudgetExceeded / **`AgentTaskFailed`** 等;规模不抄写——2026-10-11 实测 mpsc 旁路 **16** 个、severity()=Critical 总 **20** 个,完整权威清单见 `.trae/rules/nuxus规则.md` §5,代码真值源 `event-bus/src/bus.rs::is_critical_mpsc_event()` + `CRITICAL_MPSC_VARIANTS`/`CRITICAL_TOTAL` 常量)必须走 mpsc 旁路通道确保送达
- **INV-7 上下文预算界**:任意时刻 `m_total ≤ MEMORY_BUDGET_MB × MEMORY_BUDGET_UTILIZATION`(130MB × 0.9 = 117MB)
- **INV-8 归档单调性**:归档沿 Hot→Warm→Cold→Ice 单向降级,禁止逆向升级
- `chimera-mas` 任务委托深度 ≤ 5(`MAX_AGENT_DEPTH`)

**降级路径**:
- 降级/耗尽路径必须经**已存在的活体事件**发布可观测信号：规则/风险告警走 `EfficiencyAlertTriggered`（`efficiency-monitor` `monitor.rs` 的 `publish_rule_alert`/`publish_paradox_risk_alert`/`publish_critical_alert` 真实发布），预算耗尽走 `BudgetExceeded`（Critical+mpsc）。`MemoryNearBudget`/`ExpertOverload`/`ArchiveIoContention` 是 `chimera-mas/src/stability.rs:157` 的**内部 `enum PressureSource`**（降级链输入分类，非 `NexusEvent` 变体；`event-bus` 无这三名）。**红线经 ADR-210 Option C 与现网对齐、自 2026-10-11 起可满足（由上列活体事件承载）。未来若要把压力源升为一等总线事件，前置=先给 `DegradationChain::apply()` 接生产调用方（现无）**，再按七点位同批（含 `variant_count_test` 计数锁与 `doc_count_freeze.txt`）新增变体——不得先加零生产者装饰变体。**

**三重悖论免疫(第三阶段架构决策,见 nuxus规则.md §3.4.5)**:
- **记忆悖论**:OSA 静态稀疏掩码不能替代自适应记忆控制;固定 top-k 召回在任务阶段切换时会产生"幽灵记忆";记忆策略必须随任务阶段自适应(MinimalRecall→StandardTopK→QueryReformulation→AggressivePruning)
- **推理悖论**:10 层协调成本存在"推理悖论阈值"——当协调成本超过推理增益时,多 Agent 不如单 Agent;SkepticVeto 可能被策略性利用;需定期测量协调成本/推理增益比值
- **进化悖论**:GSOE/AutoDPO 验证器层级为 L3(执行反馈),存在"奖励黑客"风险;需向 L4(形式化验证)跃迁

**三环重组约束(前瞻,见 nuxus规则.md §3.4.5)**:
- 内环 9 crate 不依赖外环;外环仅通过 event-bus 依赖内环公开接口(crate 落点与计数以 `Cargo.toml` members + `CODE_WIKI.md` crate 索引为权威,含 L0 `nexus-contracts`、L6 `omega-learner`、L10 `mca-gateway`/`nexus-app-server`、L3 `session-store`、L9 `mas-sched`/`nexus-hook`、L7 `nexus-subagent`)
- (v2.20 诊断时点 38 crate 的)4 类病理——星型耦合(L6 5 节点)/跨层渗透(L7→L4)/隐式循环依赖(L5↔L9)/L1 上帝 crate——**v2.24.0 Phase 9(P9-T1~T12,ADR-054)已系统处置**:星型耦合经 L0 共享类型解耦、2 条生产违规边消除、D3/D4 消解;ImmuneSystem facade(ADR-046)以事件订阅镜像避免新增跨层依赖;ADR-187 退役内环 Check A 后依赖铁律双源脚本重新等价为绿,残余跨层渗透仅余 ADR-048/185 已收编为合法向下边的 gqep→qeep 一条

**9.工作时必须遵守**：组建一个由多名拥有10年以上行业经验的精英专家级子代理构成的专业化协作团队，该团队需以任务优先级为核心指导原则，对各项任务进度实施系统性的分布式深度分析与监控。团队应通过多轮结构化思考、充分探讨及严谨的验证流程，确保对任务需求、目标及约束条件的理解全面且准确。在执行阶段，必须严格按照既定的任务优先级顺序推进实施工作，同时始终秉持长期主义的工作理念，杜绝任何短期行为和资源过度消耗的情况。特别强调在代码修改过程中，必须先进行多方面、系统性的思考与规划后再实施代码修改，确保产出高质量的代码成果，具体质量标准包括：清晰且模块化的代码逻辑结构、高度的代码可读性、杜绝冗余代码和技术债务、完善的注释说明（包括功能描述、参数说明及复杂逻辑解释）以及符合行业最佳实践的编码规范。在整个任务执行周期内，授权团队调用所有符合任务要求且系统允许的工具资源，包括但不限于mcp、skills等相关工具，以保障任务的高效完成和卓越质量。团队还应建立定期进度汇报与质量审查机制，确保任务执行过程的透明度和成果的可追溯性。

claude.md文档和nuxus规则.md文档，进行融会贯通式相互参考。
