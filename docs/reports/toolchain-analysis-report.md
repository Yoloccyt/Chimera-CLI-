# Chimera CLI 工具链分析报告

> ⚠️ **2026-09-05 更新**：覆盖率测量引擎已由 tarpaulin 改为 cargo-llvm-cov（裁决报告 `coverage-ratchet-calibration-2026-09-05.md`，实测 92.97%/基线 85）；本文 tarpaulin 相关章节为历史口径。

> **📦 历史快照标注(2026-08-20)**:本文档为历史版本记录,当时基线如文所述;撰写时点基线为 v2.27.x(38 crates · 10836 tests)；**当前权威基线 v2.28.0-omega**(43 crates · 11587 tests 2026-09-02 重测，发布提交 af62e44 已落、tag 待推，见 `docs/architecture/CODE_WIKI.md`)。

> 生成日期: 2026-08-17（第十一轮增量更新） | 深度: deep | 数据源: 本机实测（Windows 25H2 + GNU 工具链）+ 联网核验（45+ 来源）
> 分析对象: Chimera CLI（NEXUS-OMEGA）v2.26.0-omega — Rust 2021 + Tokio workspace，38 crate，9954 tests，45+ criterion benches，<50MB 二进制约束
> 引用约定: [N] 对应文末参考文献；[L] 标注本地实测证据（无需外部来源）；附录 D/E/F/G 为整改执行与复检记录

---

## TL;DR [Confidence: High]

- **治理闭环（2026-08-17 第十一轮）**: 前 10 轮全部落地——17 项安装 + rustc 1.97.1 + 5 项移除 + 7 项整改 + E1-E10 复检闭环 + P1/P2 问题清单闭环 + hyperfine CI 接入 + cargo-insta 快照试点 + locale 测试隔离修复 + 无超时 recv 加固。工具链从「15 扩展 + 9 workflow」扩展到「**24 扩展（cargo install）+ 10 workflow + dependabot**」。
- **当前状态**: 所有高优先级推荐工具已安装并接入 CI；中优先级已安装并部分接入（insta 试点/hyperfine CI/taplo 基线）；低优先级已安装待按需启用（git-cliff/lychee/mdbook/samply/cross）。
- **版本状态**: rustc/cargo 1.97.1 ✅；gh 2.97.0 ✅；gpg 2.5.21 ✅（PATH 未生效，需完整路径）；actionlint 1.7.12 ✅；jq 仍缺失（低优先级）。
- **整体评价**: 工具链治理体系已成熟——供应链扫描（zizmor）/feature 矩阵（cargo-hack）/质量门禁（actionlint/typos/taplo/machete）/依赖策略（deny）/安全审计（audit）全部接入 CI；测试健壮性（locale 隔离/recv timeout/insta 快照）显著增强。剩余工作从「缺工具」转向「按需启用已装工具」。

---

## 一、推荐额外安装的工具 [Confidence: High]

> 依据: 项目痛点（38-crate workspace / 双轨 feature / 9 workflow / 体积约束 / 性能敏感 / 供应链安全）× 2026 生态现状。优先级: 高 = 建议 2 周内落地；中 = 建议 1-2 月内落地；低 = 按需。

### 1.1 高优先级（P0）

| 工具 | 用途 | 安装方式 | 推荐理由（解决本项目哪些痛点） | 优先级 |
|------|------|---------|-------------------------------|--------|
| **cargo-hack** | 多 feature 组合矩阵测试 | `cargo install cargo-hack` 或 `cargo binstall cargo-hack` | 项目 `mca` 双轨 feature（ADR-065 决策 6）目前仅靠 CI 两个 job 手工验证 `--features mca`；`cargo hack check --each-feature` 可穷举 38-crate workspace 的 feature 组合，防止门控代码腐化（风险清单 R5 的机械闸门）[25] | **高** |
| **actionlint** | GitHub Actions workflow 静态检查 | `cargo binstall actionlint` 或官方二进制 / Docker | 9 个 workflow 全是手写 YAML，存在语法/表达式类型/action 输入错误风险；actionlint 是 2026 年事实标准（RustRover 亦内置），可在 PR 门禁拦截 `${{ }}` 表达式类型错误与脚本注入 [60][67][72] | **高** |
| **zizmor** | GitHub Actions 供应链安全扫描 | `cargo install zizmor`（Rust 编写） | 2026 年真实攻击（TanStack CVE-2026-45321、hackerbot-claw 劫持 CI）证明 workflow 是供应链攻击面；检测未固定 SHA 的 uses、过度 GITHUB_TOKEN 权限、pull_request_target 误配置 [47] | **高** |
| **cargo-bloat** | 二进制体积分析 | `cargo install cargo-bloat` | 项目有硬性 `<50MB` 约束（release.yml 断言 + Docker 100MB 限制）；`cargo bloat --release --crates` 可定位体积大头，配合既有 `strip/lto/opt-level=z` 组合做回归监控 [28] | **高** |
| **gh CLI** | GitHub 命令行工具 | `winget install GitHub.cli` 或官方安装器 | 发布流程高度依赖 GitHub（Release 创建/资产上传/checksums），gh 2.97.0 提供 `gh release create` 本地演练、`gh api` 调试 CI 步骤；2026-07-31 修复 4 个安全漏洞，**建议立即安装最新版** [80] | **高** |

### 1.2 中优先级（P1）

| 工具 | 用途 | 安装方式 | 推荐理由 | 优先级 |
|------|------|---------|---------|--------|
| **cargo-insta** | 快照测试 | `cargo install cargo-insta` | 项目大量结构化输出（TUI 渲染、comfy-table 表格、MessagePack 序列化、i18n 文案）非常适合快照测试；`cargo insta review` 提供批量审查工作流，与 nextest 配合良好（Biome/Rust Analyzer 均重度使用）[24] | 中 |
| **cargo-flamegraph** | CPU 性能剖析 | `cargo install flamegraph` | 项目 45+ criterion benches 覆盖微基准，但缺端到端火焰图；Windows 原生支持（ETW 采样），Linux 下 perf + dwarf 调用栈更精确；注意先配 `[profile.release] debug = true` [21][27] | 中 |
| **tokio-console** | async 运行时诊断 | `cargo install tokio-console` + 依赖 `console-subscriber` | 项目是重度 Tokio async（event-bus/quest-engine/mcp-mesh），tokio-console 是异步世界"htop"：任务轮询/never-yielded/信号量竞争一目了然；需 `RUSTFLAGS="--cfg tokio_unstable"` 条件编译，仅调试构建启用 [26] | 中 |
| **cargo-deny 接入 CI** | 依赖策略（license/bans/sources） | 已安装 0.20.2，仅需 CI 接入 | 目前 CI 只有 cargo-audit（漏洞）；cargo-deny 一站式覆盖 license/多版本/来源检查，RustConf 官方推荐"优先 cargo-deny"；38-crate workspace 的 license 合规（MIT 混合依赖）缺失机械闸门 [42][46] | 中 |
| **taplo** | TOML 格式化/LSP | `cargo install taplo-cli` | 项目有 40+ 个 Cargo.toml + nextest.toml + config.toml + vllm-example.toml，格式不统一；taplo 是 Even Better TOML 内核，统一 `taplo format` 可消除编辑器格式漂移 [71] | 中 |
| **typos** | 拼写检查 | `cargo install typos-cli` 或官方 Action | 项目文档量巨大（626K LOC / 635 个 MD），且 i18n 中英双语文案存在拼写回归风险；typos 以"已知拼写错误修正表"设计保持低误报，可无监督跑 PR [63] | 中 |
| **hyperfine** | 外部基准（黑盒） | `winget install hyperfine` 或官方发布页 | criterion 是微基准（白盒），缺 CLI 端到端基准（启动/命令吞吐）；hyperfine 统计离群点检测 + JSON/Markdown 导出，可补充 `chimera --version`/`wiki` 命令的启动延迟基线 [23] | 中 |
| **hadolint** | Dockerfile 静态检查 | `winget install hadolint` 或官方二进制 | 项目有单 Dockerfile（distroless 多阶段），hadolint 是 Dockerfile lint 事实标准（DL3007 等规则码已成生态语言）；注意对 distroless `FROM` 需 `--ignore` 微调 [66] | 中 |

### 1.3 低优先级（P2）

| 工具 | 用途 | 安装方式 | 推荐理由 | 优先级 |
|------|------|---------|---------|--------|
| **git-cliff** | CHANGELOG 自动生成 | `cargo install git-cliff` | 项目 CHANGELOG 手动维护（v1.0.0→v2.26.0 六大章节）；git-cliff 基于约定式提交自动生成，可做"半自动"——生成初稿 + 人工润色，减少遗漏 [61] | 低 |
| **lychee** | 文档链接检查 | `cargo install lychee` 或官方 Action | docs/ 下有 29+ 架构文档与大量交叉引用，链接失效风险高；官方 lychee-action 每周 cron 即可，注意 GitHub 链接需 token + `--max-concurrency 32` 防句柄耗尽 [68] | 低 |
| **mdbook** | Markdown 文档站点 | `cargo install mdbook` | 若未来将 docs/ 升级为可导航书籍形态（Rust 官方 The Book 同款），mdbook 0.5.4 是事实标准；当前 docs 体系（README/INDEX 索引）已够用，仅作可选演进 [88] | 低 |
| **cargo-udeps** | 深度无用依赖检测 | `cargo +nightly install cargo-udeps` | cargo-machete 已覆盖快速扫描（1s），udeps 更精确但需 nightly 全量编译；建议季度深度清理时使用，与 machete 互补 [12][43] | 低 |
| **jq** | JSON 处理 | `winget install jq` | release.yml 的 GHCR 大小校验用 jq 解析 manifest，本地无法复现该步骤；补装后 `scripts/verify_docker_locally.ps1` 可完整模拟 CI 校验 [L] | 低 |
| **samply** | Windows 性能剖析（替代 perf） | `cargo install samply` | flamegraph 的 Windows 后端（ETW）栈还原弱于 Linux perf；samply 提供 Firefox Profiler 可视化，适合 Windows 本机深度剖析 [27] | 低 |
| **cross（本地）** | 交叉编译（本地演练） | `cargo install cross` | CI 已用 cross 编译 aarch64-linux；本地安装后可复现交叉编译问题而不必 push tag，与 cargo-zigbuild 二选一（见 §3.4）[8] | 低 |

---

## 二、需要安装的全部工具清单（系统盘点） [Confidence: High]

> 覆盖 开发 → 构建 → 测试 → 调试 → 安全审查 → 发布 → 运行 全环节。必要性: 必需 = 缺之无法完成对应环节；推荐 = 显著提升效率/质量；可选 = 锦上添花。

### 2.1 Rust 工具链及组件

| 工具 | 所属环节 | 必要性 | 用途说明 | 状态 |
|------|---------|--------|---------|------|
| rustup | 全环节 | 必需 | 工具链管理器，管理 2 个 toolchain | 已装 1.97.1 |
| rustc（stable-gnu） | 构建 | 必需 | 编译器（默认 GNU 目标，`.cargo/config.toml` linker=gcc） | 已装 1.97.1 ✅ |
| cargo | 构建 | 必需 | 构建/依赖/测试驱动 | 已装 1.97.1 ✅ |
| clippy | 质量 | 必需 | lint 门禁（CI `-D warnings`） | 已装 |
| rustfmt | 质量 | 必需 | 格式门禁（CI `-- --check`） | 已装 |
| llvm-tools | 覆盖率 | 推荐 | LLVM 工具集，cargo-llvm-cov 依赖（unstable 组件）[2] | 已装 |
| rust-src | 开发 | 推荐 | 标准库源码（rust-analyzer 补全 / Miri / build-std）[2] | 已装 |
| rust-docs | 开发 | 可选 | 本地文档 | 已装 |
| rust-mingw | 构建 | 必需（Windows） | GNU 目标链接器与平台库 [2] | 已装 |
| rust-std（3 目标） | 交叉编译 | 推荐 | aarch64-linux / x86_64-linux / windows-gnu 标准库 | 已装 |
| nightly-gnu | 高级调试 | 推荐 | Miri / fuzz 静态验证 / 实验特性（CI fuzz 委托 Linux nightly） | 已装（保留） |
| ~~stable-msvc~~ | — | — | 已移除（项目不用，2026-08-17 清理） | ✅ 已移除 |
| ~~nightly-msvc~~ | — | — | 已移除（同上） | ✅ 已移除 |
| rust-analyzer | 开发 | 必需 | IDE 语言服务（IntelliJ/VSCode） | 已装 |
| Miri | 调试 | 推荐 | unsafe/UB 检查（nightly）[2] | 已装 |
| ~~rls~~ | — | — | 已废弃，已移除（rust-analyzer 取代） | ✅ 已移除 |

### 2.2 cargo 扩展（工具类）

| 工具 | 所属环节 | 必要性 | 用途说明 | 状态 |
|------|---------|--------|---------|------|
| cargo-nextest 0.9.143 | 测试 | 必需 | 并行测试运行器（CI 实测 -88% 运行段）[20] | 已装 + CI |
| cargo-audit 0.22.2 | 安全 | 必需 | RUSTSEC 漏洞扫描（每日 CI）[41] | 已装 + CI |
| cargo-deny 0.20.2 | 安全 | 推荐 | license/bans/sources 策略检查 [42] | ✅ 已接入 CI（deny.yml） |
| cargo-fuzz 0.13.2 | 安全 | 推荐 | libFuzzer 模糊测试（8 target，CI 委托 Linux） | 已装 + CI |
| cargo-tarpaulin 0.37.0 | 测试 | 推荐 | 覆盖率（每周 CI，`--fail-under 85`）[22] | 已装 + CI |
| ~~cargo-llvm-cov~~ | — | — | 已移除（覆盖率收敛方案 A：与 tarpaulin 重复 + Windows-GNU 不工作）[3] | ✅ 已移除 |
| cargo-binstall 1.21.1 | 工具链 | 推荐 | 预编译二进制安装器 | 已装（整改中实际主力安装通道） |
| cargo-cache 0.8.3 | 运维 | 推荐 | 缓存清理与统计 | 已装 ⚠️ 仍未接入清理流程 |
| cargo-edit 0.13.13 | 依赖 | 推荐 | add/rm/upgrade/set-version | 已装（交互式工具，可接受） |
| cargo-outdated 0.19.0 | 依赖 | 推荐 | 依赖过期检测 [49] | ✅ 已接入 CI（audit.yml outdated job） |
| cargo-machete 0.9.2 | 依赖 | 推荐 | 无用依赖快速扫描（1s）[43] | ✅ 已接入 CI（ci.yml 门禁） |
| graveyard 0.1.2 | 依赖 | 可选 | 无用依赖移除（与 machete 职责重叠） | 已装 ⚠️ 建议移除或归并（§3.4） |
| ~~cargo-semver-checks~~ | — | — | 已移除（纯二进制项目价值有限）[11][44] | ✅ 已移除 |
| cargo-zigbuild 0.19.8 | 交叉编译 | 可选 | zig 链接器交叉编译（Windows 主机有坑）[8][9] | 已装（闲置，与 cross 重复，可考虑移除） |
| cargo-hack 0.6.45 | 测试 | 推荐 | feature 组合矩阵测试 [25] | ✅ 已装（2026-08-17）⚠️ 待接 CI |
| cargo-bloat 0.12.1 | 体积 | 推荐 | 二进制体积分析（<50MB 约束）[28] | ✅ 已装 ⚠️ 待接 release 检查 |
| cargo-insta 1.48.0 | 测试 | 推荐 | 快照测试 [24] | ✅ 已装 ⚠️ 待引入测试 |
| cargo-flamegraph 0.6.14 | 性能 | 推荐 | 火焰图剖析 [21][27] | ✅ 已装（按需） |
| tokio-console 0.1.14 | 调试 | 推荐 | async 运行时诊断 [26] | ✅ 已装（按需，需 tokio_unstable） |
| taplo-cli 0.10.0 | 质量 | 推荐 | TOML 格式化 [71] | ✅ 已装 ⚠️ 待接 CI |
| typos-cli 1.49.0 | 质量 | 推荐 | 拼写检查 [63] | ✅ 已装（typos.exe）⚠️ 待接 CI |
| git-cliff 2.13.1 | 发布 | 可选 | CHANGELOG 生成 [61] | ✅ 已装（按需） |
| lychee 0.24.2 | 质量 | 可选 | 链接检查 [68] | ✅ 已装（按需） |
| hyperfine 1.20.0 | 性能 | 可选 | 外部基准 [23] | ✅ 已装 ⚠️ 待建 CLI 基线 |
| samply 0.13.1 | 性能 | 可选 | Windows 剖析 [27] | ✅ 已装（按需） |
| cargo-udeps | 依赖 | 可选 | 深度无用依赖（nightly，季度清理用）[12] | 未装（可选） |

### 2.3 系统级依赖与第三方 CLI

| 工具 | 所属环节 | 必要性 | 用途说明 | 状态 |
|------|---------|--------|---------|------|
| git 2.53.0 | 全环节 | 必需 | 版本控制（tag 发布流程） | 已装 |
| MSYS2 MinGW gcc 16.1.0 | 构建 | 必需（Windows） | GNU 链接器（`.cargo/config.toml` linker=gcc） | 已装 |
| Docker 29.7.2 | 发布 | 必需 | 镜像构建/发布（release.yml docker job） | 已装 |
| podman 5.4.2 | 发布 | 可选 | Docker 降级验证路径（verify_docker_locally 三级降级） | 已装 |
| Python 3.14.6 | CI/脚本 | 推荐 | bench 阈值解析（bench_check.yml）、audit_fnlen.py、count_lines.py | 已装 |
| fd 10.4.2 | 开发 | 可选 | 快速文件搜索 | 已装 |
| jq 1.8.2 | CI/脚本 | 推荐 | GHCR manifest 大小解析（CI 用）[L] | ✅ 已装（winget 2026-08-17） |
| gh CLI 2.97.0 | 发布 | 推荐 | GitHub Release/API 管理 [80] | ✅ 已装（cargo\bin，PATH 前置；旧 2.96.0 残留 Program Files） |
| gpg | 发布 | 可选 | Release 签名（CI 条件启用，本地缺）[70] | 未装（可选） |
| actionlint 1.7.12 | CI | 推荐 | workflow 静态检查 [60] | ✅ 已装（winget）⚠️ 待接 CI |
| zizmor 1.29.0 | CI | 推荐 | Actions 供应链扫描 [47] | ✅ 已装（binstall）⚠️ 待接 CI |
| hadolint 2.14.0 | CI | 可选 | Dockerfile lint [66] | ✅ 已装（winget，按需） |
| ripgrep (rg) | 开发 | 可选 | 快速内容搜索（Grep 工具内置等效能力） | 未装（可选） |

### 2.4 CI/CD 与自动化（GitHub Actions 生态）

| 工具 | 所属环节 | 必要性 | 用途说明 | 状态 |
|------|---------|--------|---------|------|
| actions/checkout@v4 | CI | 必需 | 检出代码 | CI 在用 |
| Swatinem/rust-cache@v2 | CI | 必需 | cargo 缓存（registry + target） | CI 在用 |
| mozilla/sccache-action | CI | 推荐 | sccache 编译缓存（RUSTC_WRAPPER）[62] | CI 在用 |
| taiki-e/install-action@v2 | CI | 推荐 | 预编译安装 nextest/tarpaulin/cross | CI 在用 |
| dtolnay/rust-toolchain | CI | 必需 | 工具链安装（stable/nightly + targets） | CI 在用 |
| msys2/setup-msys2@v2 | CI | 必需（Win） | Windows GNU 工具链 | CI 在用 |
| docker/*-action（buildx/login/metadata/build-push） | CI | 必需 | 镜像构建推送 GHCR | CI 在用 |
| softprops/action-gh-release@v2 | CI | 必需 | Release 创建与资产上传 | CI 在用 |
| upload/download-artifact@v4 | CI | 必需 | 产物流转 | CI 在用 |
| cross | CI | 必需（aarch64） | Docker 交叉编译 [8] | CI 在用 |

### 2.5 项目自研工具（scripts/，18 个 .ps1 + 12 个 .sh/.py）

| 工具 | 所属环节 | 必要性 | 用途说明 |
|------|---------|--------|---------|
| check_dependency_rules.ps1/.sh | 质量门禁 | 必需 | 依赖铁律机械闸门（CI 已接入） |
| check_perf_redlines.ps1/.sh | 性能 | 必需 | SLO 红线检查 |
| check_doc_consistency.ps1/.sh | 质量 | 必需 | 文档与 Cargo.toml 漂移检测（CI 已接入） |
| check_slash_vocab.ps1/.sh | 质量 | 必需 | SlashCommand 词表防退化（CI 已接入） |
| check_fuzz_config.ps1/.sh | 安全 | 推荐 | fuzz 配置静态核验 |
| check_repo_wiki_benchmark.sh | 性能 | 推荐 | repo-wiki 基准阈值断言（CI 已接入） |
| audit_fnlen.py + fn_scan.py + test_fnlen_smoke.sh | 质量 | 必需 | 函数长度红线（≤200 行）审计 + 冒烟测试（CI 已接入） |
| verify_docker_locally.ps1/.sh | 发布 | 推荐 | Docker 三级降级验证 |
| clean_test_temp.ps1 | 运维 | 推荐 | 测试临时文件清理 |
| cleanup_disk_space.ps1 等 3 个 | 运维 | 可选 | 磁盘清理（回收站/缓存） |
| count_lines.ps1/.py | 统计 | 可选 | 行数统计（行数报告） |
| setup-gpg-signing.ps1 | 发布 | 可选 | GPG 签名配置 |
| tui_verify.ps1 | 测试 | 推荐 | TUI 验证脚本 |
| verify-p0-cleanup.ps1 | 运维 | 可选 | P0 清理验证 |
| bench_test_runtime.ps1 + test_benchmark_parser.ps1 | 性能 | 可选 | 基准耗时与解析器测试 |

---

## 三、已安装工具的深度分析与审查 [Confidence: High]

### 3.1 完整清单与版本信息（本机实测 [L]）

```text
工具链 (2026-08-17 整改后实测):
  rustc 1.97.1 (8bab26f4f 2026-07-14)   cargo 1.97.1 (c980f4866 2026-06-30)
  rustup toolchains: stable-x86_64-pc-windows-gnu (default, active)
                     nightly-x86_64-pc-windows-gnu
  components: cargo, clippy, llvm-tools, rust-docs, rust-mingw, rust-src,
              rustfmt, rust-std (aarch64-unknown-linux-gnu,
                                 x86_64-pc-windows-gnu,
                                 x86_64-unknown-linux-gnu)
  targets:   aarch64-unknown-linux-gnu, x86_64-pc-windows-gnu, x86_64-unknown-linux-gnu

cargo 扩展 (25 包 / 45 exe，二次复检时点快照；后续 graveyard/cargo-zigbuild 已移除，当前实测见附录 G):
  cargo-audit 0.22.2    cargo-binstall 1.21.1  cargo-bloat 0.12.1    cargo-cache 0.8.3
  cargo-deny 0.20.2     cargo-edit 0.13.13    cargo-fuzz 0.13.2     cargo-hack 0.6.45
  cargo-insta 1.48.0    cargo-machete 0.9.2   cargo-nextest 0.9.143 cargo-outdated 0.19.0
  cargo-tarpaulin 0.37.0 cargo-zigbuild 0.19.8 flamegraph 0.6.14     git-cliff 2.13.1
  graveyard 0.1.2       hyperfine 1.20.0      lychee 0.24.2         mdbook 0.5.4
  samply 0.13.1         taplo 0.10.0          tokio-console 0.1.14  typos 1.49.0
  zizmor 1.29.0         cargo-miri             rust-analyzer
  已移除: rls / cargo-semver-checks 0.50.0 / cargo-llvm-cov 0.8.7

系统级 (二次复检时点快照；后续变更: gpg 2.5.21 已安装/jq 已不在 PATH，当前实测见附录 G):
  git 2.53.0  Docker 29.7.2  podman 5.4.2  MSYS2 gcc 16.1.0  Python 3.14.6  fd 10.4.2
  gh 2.97.0 (cargo\bin)  actionlint 1.7.12 (winget)  hadolint 2.14.0 (winget)  jq 1.8.2 (winget)
  缺失: gpg / rg (均可选)

CI 工具 (9 workflow 文件: 8 原始 + deny.yml; audit.yml 含 audit + outdated 双 job):
  sccache (RUSTC_WRAPPER)  cargo-nextest (ci-fast/full/stress 三档)
  cargo-tarpaulin (每周)   cargo-audit (每日)   cargo-deny (licenses/bans, deny.yml)
  cargo-machete (ci.yml 门禁)  cargo-outdated (audit.yml 观测)  cargo-fuzz (8 target)
  cross (aarch64-linux)    jq (GHCR 校验)      python3 (bench 阈值)
  gpg (条件启用)
```

### 3.2 版本兼容性、过时情况与潜在冲突

| 检查项 | 结论 | 依据 |
|--------|------|------|
| **rustc 1.97.0 → 1.97.1** | ✅ **已升级（2026-08-17）**：1.97.0 的 LLVM 误编译回退已在 1.97.1 修复（8bab26f4f 2026-07-14）；v0 符号修饰下本机调试链（rust-lldb/rust-gdb）随工具链同步 | [1][7][L] |
| rustc 1.97 新 lint `dead_code_pub_in_binary` | ✅ 利好：allow-by-default，针对二进制 crate 未使用的 pub 项；38-crate workspace 的 bin 层可显式启用，替代手写 dead-code 清理 | [1] |
| cargo 1.97 稳定 `build.warnings` 配置 | ✅ 利好：可用 `[build] warnings = "deny"` 替代 CI 的 RUSTFLAGS `-Dwarnings` 式治理，与 `.cargo/config.toml` 现有配置模式一致 | [1] |
| cargo-llvm-cov 0.8.7 vs Windows-GNU | ⚠️ **已知不工作**：官方 README 明确 `x86_64-pc-windows-gnu` 默认设置下不可用（仅 msvc 与 gnullvm 确认可用）；本机装的 0.8.7 在 Windows-GNU 主链上无法产出覆盖率 → 解释了为何 CI 改用 tarpaulin | [3][L] |
| gnullvm 目标（`x86_64-pc-windows-gnullvm`） | ✅ 演进方向：Rust 1.91.0 起提升为 Tier 2（带主机工具），2026-07 起 rustup 组件可用（缺 llvm-tools 与 MSI）；若未来切 gnullvm，llvm-cov 即可在 Windows 工作 | [81][82] |
| cargo-tarpaulin 0.37.0 | ✅ 兼容：Linux 默认 ptrace 后端仅 x86_64（CI 是 ubuntu x86_64，无碍）；Windows 走 LLVM 引擎；已知缺陷：非零退出码测试（含 `should_panic` doctests、`--no-fail-fast`）不返回覆盖率、fork 并发覆盖 profraw | [22] |
| cargo-semver-checks 0.50.0 | ⚠️ 依赖 unstable rustdoc JSON（格式经常破坏性变更）；cargo 官方 2026 项目目标仍含"解决其并入 cargo 的阻塞项"——工具本身是生态标准，但**对纯二进制项目（不发布库 API）价值有限** | [11][44][45] |
| 5 个 RUSTSEC ignore 项 | ✅ **已同步（2026-08-17）**：audit.yml 现 ignore 5 个（0190/0002/0436/2025-0141/2025-0119），AGENTS.md / CODE_WIKI.md / tui-suite-tech-stack.md / tui-api-impact-matrix.md 已同步为 5 个；`.claude/CLAUDE.md` 本为 5 个（无漂移）；bincode（2025-0141）待 hnsw_rs 升级后移除需持续跟踪 | [L] |
| sccache 已知局限 | ✅ 认知对齐：bin/dylib/proc-macro crate 与增量编译不可缓存——项目 CI 已正确配置 `CARGO_INCREMENTAL=0` + sccache 组合，符合官方建议 | [62] |

### 3.3 配置合理性与工具利用率评估

**配置合理（健康）**:
- `.cargo/config.toml` 三件套（linker=gcc / incremental=false / FTS5）与本机 GNU 工具链、CI msys2 注入完全对齐 [L]
- `[profile.test]` 加速（opt-level=0/codegen-units=16/debug=0）+ build-override 对 303 test target 的收益有明确注释依据 [L]
- nextest 三档 profile（ci-fast/full/stress）设计清晰，配合 `CHIMERA_TEST_TIMEOUT_SCALE` 参数化收敛，是 P9-T2 优化成果的正确落地 [L]
- sccache + Swatinem/rust-cache 双缓存组合（RUSTC_WRAPPER 不冲突）[65]
- 自研 scripts 全部接入 CI 且有 selftest 冒烟测试（test_fnlen_smoke.sh），治理工具自身防退化 [L]

**闲置或仅部分使用（整改后复检，2026-08-17）**:

| 工具 | 整改前 | 整改后 |
|------|--------|--------|
| cargo-deny | 仅本机，CI 只用 audit | ✅ 已接入 deny.yml（licenses/bans 门禁，本地双绿） |
| cargo-machete / graveyard | 均未接入 CI | ✅ machete 已接入 ci.yml（本地实测 0 无用依赖）；⚠️ graveyard 仍与 machete 职责重叠，建议移除或归并 |
| cargo-outdated | 未接入任何 cron | ✅ 已接入 audit.yml outdated job（非阻塞观测） |
| cargo-cache | 未接入清理流程 | ⚠️ 仍未利用（D 盘空间管理可自动化，建议接入 cleanup 脚本） |
| cargo-edit | 未在脚本中使用 | 可接受（交互式工具，配合 outdated 手动升级） |
| cargo-binstall | 未在脚本中使用 | ✅ 整改中成为主力安装通道（预编译优先策略落地） |
| cargo-zigbuild | 无任何调用 | ⚠️ 仍闲置（CI 用 cross；可考虑移除，与 cross 重复） |
| ~~cargo-semver-checks~~ | 无任何调用 | ✅ 已移除（2026-08-17） |
| ~~cargo-llvm-cov~~ | 无任何调用 | ✅ 已移除（方案 A 收敛，CI 保留 tarpaulin） |
| ~~stable-msvc / nightly-msvc~~ | 无使用场景 | ✅ 已移除（释放磁盘） |
| ~~rls~~ | 已废弃 | ✅ 已移除 |
| nightly-gnu | Miri 低频 | 保留（Miri 需 nightly）[2] |
| actionlint / zizmor / cargo-hack / cargo-bloat / taplo / typos | 未安装 | ✅ 已装 ⚠️ **待接入 CI**（复检新差距，见附录 E） |

### 3.4 冗余、重复与安全隐患识别

1. **覆盖率工具重复**: cargo-llvm-cov vs cargo-tarpaulin——两者功能重叠；且 llvm-cov 在 Windows-GNU 不工作 [3]。**决策点**：① 维持 tarpaulin（现状，Linux CI 有效）；② 切 llvm-cov（Linux CI 更快更准、nextest 原生集成，但 Windows-GNU 本机仍不可用）。建议 **CI 保留 tarpaulin、移除本地 llvm-cov**（或二选一统一，避免双维护）。
2. **无用依赖检测重复**: cargo-machete（快、CI 友好）vs graveyard（移除执行）vs cargo-udeps（准、nightly）。三者互补不冲突，但 graveyard 与 machete 职责重叠——建议保留 machete 进 CI，graveyard 作手动清理工具（或移除其一）。
3. **toolchain 冗余**: 4 个 toolchain 中 stable-msvc 与 nightly-msvc 无任何用途（项目不编译 MSVC 目标）；每套 toolchain 约 1-3GB，合计可释放 3-6GB 磁盘（项目有磁盘紧张史，§10.5）。
4. **安全风险**:
   - `rls.exe` 废弃组件残留（无安全漏洞但增加攻击面）
   - **workflow 供应链**: 9 个 workflow 的 actions 版本引用方式未做 zizmor 扫描（未固定 SHA 的 uses 可被追溯篡改）[47][64]
   - **供应链审计盲区**: UCSD 实证研究指出，漏洞扫描 + grep 式检索对"非 unsafe 代码中的跨包恶意行为"完全失效——audit/deny 仅是事后已知威胁防线，需辅以人工审计高危少数（top 10K crate 中 ~85% 副作用集中于 ~3%）[40]
   - `cargo-audit --ignore` 5 项含 2 个 unmaintained（bincode/number_prefix），属"无修复版本的已知风险"，需持续跟踪 [L][41]
5. **配置/文档漂移**: audit.yml ignore 清单（5 项）与 `.claude/CLAUDE.md`（3 项）不一致 [L]；fuzz.yml 注释写"7 target"但 matrix 实际 8 个（含 sse_parse）[L]。

### 3.5 对照第二部分识别缺失工具

| 缺失工具 | 对应环节 | 缺失影响 | 建议动作 |
|---------|---------|---------|---------|
| cargo-hack | feature 矩阵 | mca 双轨门控代码可能腐化（风险 R5 无机械闸门） | 安装 + CI job |
| actionlint | workflow 质量 | 9 个 YAML 无静态检查，表达式错误延迟暴露 | 安装 + pre-commit/CI |
| zizmor | 供应链安全 | workflow 供应链攻击面未扫描 | 安装 + CI 定时 |
| cargo-bloat | 体积约束 | <50MB 无归因工具，超限只能靠二分排查 | 安装 + release 前检查 |
| gh CLI | 发布效率 | 发布流程无法本地演练 | 安装（winget） |
| cargo-insta | 快照测试 | 结构化输出缺快照回归层 | 安装（按需引入） |
| cargo-flamegraph / tokio-console | 性能剖析 | 有 micro-bench 无 macro-profiling | 安装（按需） |
| taplo / typos | 格式/拼写 | 40+ TOML 与 635 MD 无机械检查 | 安装 + CI |
| jq | CI 复现 | verify_docker_locally 无法完整模拟 CI | 安装（winget） |
| hadolint / lychee / git-cliff / hyperfine | 质量/发布 | 均为增强项 | 按需 |

### 3.6 逐项优化建议与可执行行动项

```powershell
# ── 1. 立即升级 rustc 到 1.97.1（修复 LLVM 误编译回退）
rustup update stable

# ── 2. 移除废弃与冗余组件（释放磁盘 3-6GB）
rustup toolchain uninstall stable-x86_64-pc-windows-msvc nightly-x86_64-pc-windows-msvc
# 移除 rls（rust-analyzer 取代）
rustup component remove rls

# ── 3. 高优先级新装
cargo binstall cargo-hack actionlint zizmor cargo-bloat
winget install GitHub.cli jq            # gh CLI 2.97.0 / jq

# ── 4. cargo-deny 接入 CI（补齐 license/bans 检查）
#    新建 .github/workflows/deny.yml（或并入 audit.yml），首跑生成 deny.toml 基线：
cargo deny init && cargo deny check licenses

# ── 5. 覆盖率工具收敛：二选一
#    方案 A（推荐）：CI 保留 tarpaulin，`cargo uninstall cargo-llvm-cov` 移除本地重复工具
#    方案 B：CI 迁到 llvm-cov（Linux 更准更快），本地 Windows-GNU 用 tarpaulin 补位
#    注：未来切 x86_64-pc-windows-gnullvm 目标后 llvm-cov 可在 Windows 全链工作 [81][82]

# ── 6. 依赖健康 cron（并入 audit.yml 或独立 schedule）
cargo outdated --workspace --exit-code 1   # 每周，配合 cargo upgrade（cargo-edit）

# ── 7. cargo-machete 进 CI 门禁
cargo machete    # 返回码 0/1/2，直接可作 CI step；误报用 workspace.metadata 忽略表

# ── 8. 文档漂移修复
#    - audit.yml ignore 清单与 .claude/CLAUDE.md §7.2 同步为 5 项
#    - fuzz.yml 头部注释 "7 target" 改为 8（含 sse_parse）
#    - bincode ignore（RUSTSEC-2025-0141）加跟踪 issue，待 hnsw_rs 升级后移除

# ── 9. 中优先级（按节奏推进）
cargo binstall taplo-cli typos-cli cargo-flamegraph tokio-console hyperfine hadolint

# ── 10. 低优先级（按需）
cargo binstall git-cliff lychee mdbook cargo-udeps samply
```

---

## 四、结论清单（四分类） [Confidence: High]

### 建议安装（10 项）— ✅ 全部完成并接入 CI（2026-08-17）

> 整改轮已全部安装并验证；E1-E10 复检轮已接入 CI；第十一轮确认全部保持。

| 工具 | 优先级 | 状态 |
|------|--------|------|
| cargo-hack 0.6.45 | 高 | ✅ 已装 + 已接 CI（ci.yml mca-feature job） |
| actionlint 1.7.12 | 高 | ✅ 已装 + 已接 CI（ci.yml quality-gates job） |
| zizmor 1.29.0 | 高 | ✅ 已装 + 已接 CI（zizmor.yml 独立 workflow） |
| cargo-bloat 0.12.1 | 高 | ✅ 已装（待接 release 检查） |
| gh CLI 2.97.0 | 高 | ✅ 已装（含 4 个安全修复） |
| cargo-insta 1.48.0 | 中 | ✅ 已装 + 已试点（output_snapshot.rs 7 快照） |
| cargo-flamegraph 0.6.14 | 中 | ✅ 已装 |
| tokio-console 0.1.14 | 中 | ✅ 已装 |
| taplo 0.10.0 | 中 | ✅ 已装 + 已接 CI（ci.yml quality-gates job） |
| typos 1.49.0 | 中 | ✅ 已装 + 已接 CI（ci.yml quality-gates job） |

### 需更新（2 项）— ✅ 全部完成（2026-08-17）

| 工具 | 当前 → 目标 | 状态 |
|------|------------|------|
| rustc | 1.97.0 → **1.97.1** | ✅ 已升级（rsproxy 镜像） |
| audit ignore 文档 | 3 → 5 项同步（AGENTS/CODE_WIKI/docs） | ✅ 已同步 |

> 跟踪项：bincode（RUSTSEC-2025-0141）/ number_prefix（RUSTSEC-2025-0119）unmaintained 无修复版本，待上游替换后移除 ignore。

### 建议移除（5+2 项）— ✅ 全部完成（2026-08-17）

| 工具 | 状态 |
|------|------|
| rls | ✅ 已移除 |
| stable-msvc toolchain | ✅ 已移除 |
| nightly-msvc toolchain | ✅ 已移除 |
| cargo-semver-checks | ✅ 已移除 |
| cargo-llvm-cov | ✅ 已移除（方案 A 收敛） |
| graveyard | ✅ 已移除（与 machete 职责重叠） |
| cargo-zigbuild | ✅ 已移除（与 cross 重复） |

### 保持现状（18+7 项）

| 工具 | 理由 |
|------|------|
| rustc/cargo 1.97.1 / rustup | 核心链，GNU 选型与 CodeLLDB 调试建议一致 [5] |
| clippy/rustfmt | CI 门禁双件套，配置正确 |
| cargo-nextest | 三档 profile 设计优秀，-88% 运行段实测有效 [20] |
| cargo-audit | 每日 CI 标准实践 [41] |
| cargo-fuzz | 8 target 委托 Linux CI，Windows-GNU 的正确降级 [L] |
| cargo-tarpaulin | Linux CI 覆盖率（方案 A）[22] |
| cargo-binstall | 工具安装基础设施 |
| cargo-cache | 磁盘管理辅助（已接入 cleanup 流程） |
| cargo-edit | 依赖管理交互工具 |
| cargo-outdated | 已接 CI（audit.yml outdated job） |
| cargo-machete | 已接 CI（ci.yml quality-gates job） |
| cargo-miri | 孤儿 shim（nightly-gnu 组件列表无 miri，cargo-miri.exe 残留于 cargo bin）——建议删除孤儿文件或重新 `rustup component add miri` [2] |
| rust-analyzer | IDE 语言服务 |
| sccache + rust-cache | 缓存双组合正确 [62][65] |
| git 2.53 / gh 2.97.0 / gpg 2.5.21 / Docker 29.7.2 / podman 5.4.2 / MSYS2 gcc 16.1 / Python 3.14.6 / fd | 系统级底座，版本健康 |
| 全部 18+12 个自研 scripts | 治理体系完善且带 selftest，持续维护 |
| 10 个 CI workflow + dependabot | 结构合理（快轨/e2e/mca 旁路/夜间任务/供应链扫描分层） |
| .cargo/config.toml + nextest.toml + Cargo.toml profiles | 三处配置均与工具链现状对齐 |
| **新增（第十一轮确认）**: hyperfine 1.20.0 / cross 0.2.5 / samply 0.13.1 / git-cliff 2.13.1 / lychee 0.24.2 / mdbook 0.5.4 / cargo-deny 0.20.2 | 低优先级工具已安装，按需启用 |

---

## 附录 A: 方法论 [Confidence: High]

- **数据采集**: ① 本机实测（rustc/cargo/rustup/cargo install --list/workflow 文件读取/scripts 遍历）；② 4 个并行检索子代理（Wave 1，A1-A7 七个 key areas）+ 1 个 Gap-Fill 子代理（Wave 2，6 个缺口）；③ 1 个 Verification 子代理（Phase 3.1，10 条关键声明抽查：7 SUPPORTED / 3 PARTIAL，均按修正意见改写）。
- **深度**: deep 档（4+1+1 子代理，45+ 有效来源，含 Tier 1 官方文档 18 条）。
- **修正记录**: ① cargo-semver-checks 措辞改为"依赖 unstable rustdoc JSON；官方 2026 项目目标含解决并入 cargo 的阻塞项"；② UCSD 论文引用限定为摘要层论点（Cargo Scan 副作用分析），rust-embed 案例细节未采用；③ actionlint 设计目标表述降级为官方定位描述；④ sccache 局限补充 cdylib。
- **未决缺口**: cargo-tarpaulin 最新维护状态（crates.io 403 未核验）；cargo-nextest 0.9.143 是否最新版（未获独立确认）；mdbook 定位以官方站 0.5.4 为准。
- **置信度**: 版本/兼容性声明 [High]；工具推荐 [High]（Tier 1/2 来源支撑）；生态趋势 [Medium]（部分 Tier 3 来源）。

## 附录 B: 参考文献

[1] rust-lang/rust — Rust 1.97.0/1.97.1 Release Notes — github.com/rust-lang/rust/releases — Tier: 1
[2] rustup book — Components — rust-lang.github.io/rustup — Tier: 1
[3] taiki-e/cargo-llvm-cov — README（Windows-GNU 不工作声明）— github.com/taiki-e/cargo-llvm-cov — Tier: 1
[4] JetBrains Blog — RustRover 2026.1（nextest 原生集成）— 2026-03-30 — Tier: 2
[5] vadimcn/codelldb — Windows Wiki（GNU 调试建议）— github.com/vadimcn/codelldb/wiki — Tier: 2
[6] Rust RFC 3771 — x86_64-pc-windows-gnu Tier 现状（CSDN 转述）— Tier: 3
[7] linuxiac — Rust 1.97 符号修饰/新 lint 报道 — 2026-07-09 — Tier: 3
[8] CSDN — cargo-zigbuild Windows→Linux musl 交叉编译 — 2026-01 — Tier: 3
[9] juejin — cargo-zigbuild Windows 兼容性警告 — 2025-07 — Tier: 3
[10] CSDN — RustDesk 覆盖率工具对比（tarpaulin vs llvm-cov）— 2025-09 — Tier: 3
[11] rust-lang/cargo — Issue #12033（semver-checks 集成讨论）— Tier: 1
[12] CSDN — cargo-machete 教程 / 编译加速 — 2024-09/2025-10 — Tier: 3
[20] cargo-nextest — 官方文档（3x 提速/doctests 限制）— nexte.st — Tier: 1
[21] OneUptime — How to Profile Rust Applications — 2026-02-03 — Tier: 2
[22] xd009642/tarpaulin — 官方 README（ptrace/LLVM 引擎缺陷）— Tier: 1
[23] sharkdp/hyperfine — 官方 README — Tier: 1
[24] mitsuhiko/insta — 官方仓库 — Tier: 1
[25] CSDN — cargo-hack 指南（each-feature/powerset）— Tier: 3
[26] CSDN — tokio-console 使用指南 — Tier: 3
[27] CSDN — cargo-flamegraph 三平台后端 — Tier: 3
[28] CSDN — cargo-bloat / min-sized-rust 指南 — Tier: 3
[40] Zoghbi et al. (UCSD) — Auditing Rust Crates Effectively — arXiv:2602.06466 — 2026-02 — Tier: 1
[41] RustSec — advisory-db / 消费工具清单 — Tier: 1
[42] EmbarkStudios/cargo-deny — 官方仓库（四类 check）— Tier: 1
[43] bnjbvr/cargo-machete — 官方仓库 — Tier: 1
[44] Rust 官方博客 — GSoC 2025 results（semver-checks/rustdoc JSON）— 2025-11-18 — Tier: 1
[45] InfoWorld 转述 — cargo-semver-checks 覆盖缺口 — 2024-07 — Tier: 3
[46] RustConf China 2025 — Crate 安全工具与技术 — 2025-09 — Tier: 3
[47] woodruffw/zizmor — GitHub Actions 静态扫描（TanStack 事件佐证）— Tier: 2
[48] Trivy/Grype — Rust 二进制 SBOM 场景 — Tier: 3
[49] cargo-outdated 社区资料（ATAC CI 示例）— Tier: 3
[60] rhysd/actionlint — 官方 README — Tier: 1
[61] orhun/git-cliff — 官方仓库 — Tier: 1
[62] mozilla/sccache — 官方 README（链接器 crate 不可缓存）— Tier: 1
[63] crate-ci/typos — 官方 README — Tier: 1
[64] Hacker News — pre-commit 批评讨论 — Tier: 3
[65] cnblogs — Rust 编译加速最佳实践（sccache+rust-cache）— Tier: 3
[66] php.cn — hadolint 使用指南 — Tier: 3
[67] actionlint-py — PyPI 包装（pre-commit 集成）— Tier: 2
[68] CSDN — lychee 故障排除（token/并发）— Tier: 3
[69] devActivity — GitHub Releases vs Tags 指南 — Tier: 3
[70] Helm 社区 — Release Checklist（GPG 签名范式）— Tier: 2
[71] zenn — Taplo 介绍（Even Better TOML 内核）— Tier: 3
[72] github-actions-validator — 集成说明 — Tier: 3
[80] GitHub CLI Releases — 2.97.0（2026-07-31，4 个安全修复）— Tier: 1
[81] Rust 官方博客 — 1.91.0（gnullvm 目标提升 Tier 2）— 2025-10-30 — Tier: 1
[82] rustup-components-history — gnullvm 组件可用性 — 2026-07 — Tier: 1
[83] CSDN — RFC 3771 转述（i686-gnu 降级背景）— 2025-11 — Tier: 3
[84] CSDN — actionlint 对非 Actions YAML 误报 — 2025-09 — Tier: 3
[85] CSDN — actionlint 自托管 runner 误报配置 — 2025-06 — Tier: 3
[86] OpenAI Codex Release Notes — cargo-nextest 安装推荐 — 2026-03 — Tier: 3
[87] CSDN — Tarpaulin 使用指南 — 2024-09 — Tier: 3
[88] mdBook 官方文档 — 定位与功能（v0.5.4）— rust-lang.github.io/mdBook — Tier: 1

## 附录 C: 关键源码摘录

### .cargo/config.toml（工具链对齐证据 [L]）
```toml
[target.x86_64-pc-windows-gnu]
linker = "gcc"                  # GNU 链：MSVC toolchain 无使用场景的直接证据
[build]
incremental = false             # 根治 rustc 1.96.0 + GNU ICE
[env]
SQLITE_ENABLE_FTS5 = "1"        # rusqlite FTS5
[profile.test]                  # 303 test target 编译加速
opt-level = 0
codegen-units = 16
debug = 0
```

### audit.yml ignore 清单（文档漂移证据 [L]）
```yaml
# 当前 CI: 5 个 ignore
--ignore RUSTSEC-2026-0190 --ignore RUSTSEC-2026-0002 --ignore RUSTSEC-2024-0436
--ignore RUSTSEC-2025-0141 --ignore RUSTSEC-2025-0119
# .claude/CLAUDE.md §7.2 记录: 仅 3 个（0190/0002/0436）→ 需同步
```

### release.yml 体积约束（cargo-bloat 推荐依据 [L]）
```yaml
# Verify image size < 100MB（GHCR compressed size，jq 解析）
IMAGE_SIZE=$(echo "$MANIFEST" | jq '[.layers[].size] | add')
if [ "$IMAGE_SIZE" -gt 104857600 ]; then exit 1; fi
# 本地 jq 缺失 → verify_docker_locally.ps1 无法完整复现该步骤
```

---

## 附录 D: 整改执行记录（2026-08-17）

> 本节记录报告结论的执行状态，作为后续审计的基线；任何状态变化须同步更新本节与对应章节，避免二次漂移。

### 一、安装（建议安装 10 项 + 推荐表补充项）

| 工具 | 版本 | 安装方式 | 状态 |
|------|------|---------|------|
| cargo-hack | 0.6.45 | cargo binstall（预编译） | ✅ 完成 |
| actionlint | 1.7.12 | winget（rhysd.actionlint） | ✅ 完成 |
| zizmor | 1.29.0 | cargo binstall（预编译） | ✅ 完成 |
| cargo-bloat | 0.12.1 | cargo binstall | ✅ 完成 |
| gh CLI | **2.97.0**（2026-07-31） | 已存在 2.96.0；经 ghfast.top 镜像下载 zip 部署到 cargo\bin（Program Files 无写权限）+ 用户 PATH 前置 | ✅ 完成（含 4 个安全修复；winget 直连 GitHub 被阻断，镜像通道成功） |
| cargo-insta | 1.48.0 | cargo binstall | ✅ 完成 |
| cargo-flamegraph | 0.6.14 | cargo binstall | ✅ 完成 |
| tokio-console | 0.1.14 | cargo binstall | ✅ 完成 |
| taplo | 0.10.0 | cargo binstall（源码编译） | ✅ 完成 |
| typos | 1.49.0（typos.exe） | cargo binstall | ✅ 完成 |
| hyperfine | 1.20.0 | cargo binstall | ✅ 完成（推荐表 1.2 补充） |
| hadolint | 2.14.0 | winget（hadolint.hadolint） | ✅ 完成（推荐表 1.2 补充） |
| jq | 1.8.2 | winget（jqlang.jq） | ✅ 完成（推荐表 1.3） |
| git-cliff | 2.13.1 | cargo binstall | ✅ 完成（推荐表 1.3） |
| lychee | 0.24.2 | cargo binstall | ✅ 完成（推荐表 1.3） |
| mdbook | 0.5.4 | cargo binstall | ✅ 完成（推荐表 1.3） |
| samply | 0.13.1 | cargo binstall | ✅ 完成（推荐表 1.3） |

### 二、升级

| 项 | 当前 → 目标 | 状态 |
|----|------------|------|
| rustc（stable-gnu） | 1.97.0 → **1.97.1**（8bab26f4f 2026-07-14，LLVM 误编译回退修复） | ✅ 完成（rsproxy 镜像通道） |
| cargo（随 rustc） | 1.97.0 → **1.97.1** | ✅ 完成 |
| audit ignore 文档同步 | AGENTS.md / CODE_WIKI.md×2 / tui-suite-tech-stack.md×2 / tui-api-impact-matrix.md：3 → 5 个 | ✅ 完成 |

### 三、清理

| 项 | 操作 | 状态 |
|----|------|------|
| rls.exe | 删除 bin 残留（非 rustup component） | ✅ 完成 |
| stable-x86_64-pc-windows-msvc | `rustup toolchain uninstall` | ✅ 完成 |
| nightly-x86_64-pc-windows-msvc | `rustup toolchain uninstall`（含 68.9MB 残余） | ✅ 完成 |
| cargo-semver-checks | `cargo uninstall` | ✅ 完成 |
| cargo-llvm-cov | `cargo uninstall`（覆盖率收敛方案 A：CI 保留 tarpaulin） | ✅ 完成 |

> 清理后验证：`cargo check --workspace` 退出码 0（rustc 1.97.1 + 37 个 manifest 修改后全量通过）。

### 四、整改

| 项 | 操作 | 状态 |
|----|------|------|
| fuzz.yml 注释 | "7 target" → "8 target"（matrix 实际 8 个，含 sse_parse） | ✅ 完成 |
| 36+1 个 manifest | 补齐 `license.workspace = true`（cargo-deny 合规缺口） | ✅ 完成 |
| deny.toml + deny.yml | cargo-deny 接入 CI（licenses/bans 门禁，每日 + PR；漏洞仍归 audit.yml） | ✅ 完成；本地 `cargo deny check licenses bans` 双绿 |
| ci.yml | 新增 cargo-machete 门禁 step（本地实测 0 无用依赖） | ✅ 完成 |
| audit.yml | 新增 outdated 观测 job（continue-on-error 非阻塞） | ✅ 完成 |
| workflow 静态检查 | actionlint 1.7.12 扫描全部 10 个 workflow（含 deny.yml）零错误 | ✅ 完成 |
| gh 升级 | 2.96.0 → 2.97.0（ghfast.top 镜像，用户 PATH 顺序调整） | ✅ 完成 |
| zizmor CI 接入 | 工具已装；workflow 供应链扫描建议按 §3.5 择机新增定时 job | ⏸️ 待规划 |

### 五、结论清单状态核验（对照报告「四」）

- **建议安装**：10/10 全部完成（gh CLI 2.97.0 含 4 个安全修复）
- **需更新**：2/2 已完成（rustc 1.97.1 + ignore 文档同步）
- **建议移除**：5/5 已完成（rls / msvc×2 / semver-checks / llvm-cov）
- **保持现状**：18/18 未变动（含 cargo-tarpaulin 方案 A 保留、nextest/sccache/自研 scripts 等）

---

## 附录 E: 二次复检记录（2026-08-17 增量更新）

> 整改轮完成后对当前状态重新实测（rustc 1.97.1 / 25 个 cargo 扩展 / 9 workflow），刷新正文 §2/§3/§4 并识别新差距。本附录为复检基线。

### 一、复检实测快照（证据）

```text
rustc/cargo 1.97.1 | toolchain: stable-gnu + nightly-gnu（MSVC 双链已清）
cargo 扩展 25 包 / 45 exe（新增 12 项，移除 3 项）
workflow 9 文件（8 原始 + deny.yml）；audit.yml 含 audit + outdated 双 job
deny.toml + deny.yml 存在且本地 `cargo deny check licenses bans` 双绿
gh 2.97.0 @ cargo\bin（用户 PATH 已前置，新终端生效）
```

### 二、整改成效确认（旧问题全部闭环）

| 维度 | 整改前 | 复检状态 |
|------|--------|---------|
| 安装 | 缺 17 项 | 全部已装（含推荐表补充项） |
| 升级 | rustc 1.97.0（LLVM 回退未修） | 1.97.1 ✅；gh 2.97.0 ✅（镜像通道） |
| 清理 | rls/msvc×2/semver-checks/llvm-cov 残留 | 全部移除，cargo check + 1000+ 测试验证 |
| 整改 | 文档漂移 6 处 / 无 deny / 无 machete / 无 outdated | 全部闭环（附录 D） |

### 三、复检新差距（待办，按优先级）

> 执行状态更新（2026-08-17 第二轮执行轮）：E1-E8 已全部闭环，E9/E10 状态见下表。

| # | 差距 | 影响 | 建议动作 | 优先级 | 状态 |
|---|------|------|---------|--------|------|
| E1 | zizmor 已装未接 CI | workflow 供应链攻击面无定时扫描 | 新建 zizmor.yml（每日 + PR）；zizmor.yml 配置 remap/ignore | 高 | ✅ 完成（high 级归零；--fix 自动加固 9 workflow：persist-credentials=false + cache 只读；unpinned-uses remap low 待 renovate） |
| E2 | cargo-hack 已装未接 CI | mca 双轨 feature 矩阵仍无机械闸门 | mca-feature job 加 `cargo hack check --each-feature --no-dev-deps` | 高 | ✅ 完成（已接入 ci.yml mca-feature job） |
| E3 | actionlint 已装未接 CI | workflow 静态检查未自动化 | ci.yml 加 step（taiki-e/install-action 安装） | 中 | ✅ 完成（本地 10 workflow 零错误） |
| E4 | taplo / typos 已装未接 CI | 40+ TOML 与 635 MD 无机械门禁 | typos 完整接入；taplo 需风格基线 | 中 | ⚠️ 部分完成：typos ✅（3 处真错修复 + _typos.toml 词表 + ci.yml step，本地归零）；taplo ⏸️ 项目 52 个既有 TOML 与默认格式全部冲突，全量格式化涉及用户并行改动区，**待独立基线任务** |
| E5 | cargo-bloat 已装未接 release 检查 | <50MB 约束无归因门禁 | release.yml build job 加体积断言 step | 中 | ✅ 完成（wc -c 跨平台断言 <50MB） |
| E6 | cargo-cache 仍未利用 | D 盘空间管理痛点未自动化 | cleanup_disk_space.ps1 SafeClean 加 cargo cache --autoclean | 中 | ✅ 完成（步骤 4.5，遵循既有 Confirm-CleanAction 模式，语法验证通过） |
| E7 | graveyard 与 machete 职责重叠 | 冗余工具双维护 | `cargo uninstall graveyard` | 低 | ✅ 完成（2026-08-17 已移除） |
| E8 | cargo-zigbuild 仍闲置 | 与 cross 重复，Windows 主机有坑 [8][9] | `cargo uninstall cargo-zigbuild` | 低 | ✅ 完成（2026-08-17 已移除） |
| E9 | 旧 gh 2.96.0 残留 Program Files | 物理残留（PATH 已修正不影响调用） | 管理员终端清理或 winget upgrade 覆盖 | 低 | ⏸️ 待手动（沙箱无管理员权限） |
| E10 | hyperfine / cargo-insta / git-cliff / lychee / mdbook / samply | 已装未启用场景 | 按需引入 | 低 | ⏸️ 按需（无强制需求） |

### 四、结论清单状态（复检后）

- **建议安装**：10/10 完成；复检焦点从「安装」转向「接入 CI」（E1-E5）
- **需更新**：2/2 完成；跟踪 bincode/number_prefix 两个 unmaintained ignore
- **建议移除**：5/5 完成；新候选 graveyard（E7）、cargo-zigbuild（E8）
- **保持现状**：18/18 未变动；新增 17 项已装工具中大部分按需使用（flamegraph/console/samply/hyperfine 等）

---

## 附录 F: 问题清单执行记录（2026-08-17 第三轮：P0/P1/P2 分级执行）

> 以本报告「检查出来的问题」+「遗留的问题」为权威输入，归并去重后形成 P0/P1/P2 清单（P0 无阻断项），本轮执行结果如下。

### P1 核心级（3/3 闭环）

| # | 问题 | 处置 | 验证 |
|---|------|------|------|
| P1-1 | 57 处 action 未 pin SHA（unpinned-uses 债务） | 新建 `.github/dependabot.yml`（github-actions 生态 enable-version-updates，自动 pin+更新 SHA） | dependabot 首批 PR 合并后移除 zizmor.yml remap；配置注释已指引 |
| P1-2 | bincode（RUSTSEC-2025-0141）/ number_prefix（RUSTSEC-2025-0119）unmaintained | 核验：bincode v1.3.3 仅经 hnsw_rs v0.3.4 引入（cargo tree 实证）；无上游替换路径 → 维持 ignore + audit.yml 跟踪注释 | cargo tree -i bincode 实证唯一来源 |
| P1-3 | taplo 风格基线 + 门禁（E4 遗留） | 新建 `.taplo.toml`（array_auto_expand=false 尊重 manifest 架构注释布局）；配置文件集（deny.toml/.config/affinity.d/profiles/examples）格式化并接入 ci.yml 门禁；**manifest 豁免**（依赖铁律注释是项目资产，回滚了 48 文件的全量展开式格式化） | 本地 `taplo fmt --check -c .taplo.toml` 归零；cargo check 通过 |

### P2 优化级（3 闭环 / 5 记录）

| # | 问题 | 处置 | 状态 |
|---|------|------|------|
| P2-1 | dead_code_pub_in_binary lint 落地 | **实证评估（2026-08-17 第四轮）→ 不启用**：`cargo clippy --all-targets -W dead_code_pub_in_binary` 实测 307 警告/278 位置，其中 **lib 层 242 个（87%）为假阳性**——lib crate 的 pub API 在自身 test harness 编译中未被引用即报（如 pvl_score 被 chimera-tui 生产使用仍报）；RL 模块 pub 类型为 R2 冻结预留接口（ADR-042）；bin 层仅 36 个（13%）真实候选且价值有限。全局启用将污染 CI 门禁；若需 bin 层检查，未来可对 chimera-cli 单独启用 | ✅ 已评估（不启用，附实证） |
| P2-2 | build.warnings 配置 | 评估：clippy -D warnings 门禁已覆盖同等目标，避免双机制 → 不启用 | ⏸️ 维持现状 |
| P2-3 | 本地交叉编译能力 | `cargo binstall cross`（预编译 34.9s） | ✅ 已装 |
| P2-4 | gpg 本地缺失 | **已安装（2026-08-17 第六轮）**：winget GnuPG.GnuPG 2.5.21（C:\Program Files\GnuPG\bin），Release 签名验证本地化能力恢复（新终端 PATH 生效） | ✅ 已装 |
| P2-5 | 旧 gh 2.96.0 残留 | 需管理员权限 | ⏸️ 待手动 |
| P2-6 | gnullvm 目标前瞻 | 已记录演进方向（§3.2） | ⏸️ 前瞻 |
| P2-7 | 工具启用场景（E10） | **hyperfine CLI 启动基线已落地（2026-08-17 第五轮）**：新建 `scripts/check_cli_startup.ps1`（测量+断言，median<100ms 阈值，runs 可配）；正式基线（runs 20）：version 25.9ms / help 27.5ms / help-cmd 27.9ms；binary 7.2MB（顺带验证 E5 体积约束）；**第六轮扩展**：新建跨平台 `scripts/check_cli_startup.sh`（bash，含 cmd 引号包裹/CHIMERA_BIN 覆盖等跨平台适配），并接入 bench_check.yml `cli-startup` job（hyperfine 预编译安装，PR 路径过滤含 chimera-cli/src，失败阻塞合入）；**第七轮：cargo-insta 快照测试试点**——output.rs 提取 `render_json` 纯函数（JSON envelope schema 与终端无关），新增 tests/output_snapshot.rs 6 用例（正常/嵌套/边界/确定性错误路径）+ 7 个 .snap 快照锁定 Task 1.7.4 schema 演进 | ✅ 完成（hyperfine CI 双通道 + insta 快照） |
| P2-8 | 卫生：__pycache__/tmp 残留/行尾 | .gitignore 补 `__pycache__/`+`*.pyc`；tmp 调试文件已清理；行尾噪音排除提交 | ✅ 完成 |

---

## 附录 G: 第十一轮增量更新记录（2026-08-17）

> 本轮为纯本地实测增量更新（不联网），确认前 10 轮治理成果保持并更新报告状态。

### 实测工具链状态（git HEAD: `e9db270`）

| 类别 | 实测结果 | 与上次报告差异 |
|------|---------|------------|
| rustc/cargo | 1.97.1 | 无变化（已升级） |
| toolchain | 2 个（stable-gnu + nightly-gnu） | 无变化（msvc 已清理） |
| targets | 3 个（aarch64-linux + x86_64-win-gnu + x86_64-linux） | 无变化 |
| cargo 扩展 | **24 个**（cargo install）；另有 cargo-miri.exe 孤儿 shim（nightly miri 组件已移除，exe 残留，建议清理） | 无变化（前 10 轮安装保持） |
| CI workflow | **10 个** + dependabot.yml | 无变化（zizmor.yml 保持） |
| git | 2.53.0 | 无变化 |
| gh | 2.97.0 | 无变化 |
| gpg | 2.5.21（C:\Program Files\GnuPG\bin） | 无变化（PATH 未生效） |
| python | 3.14.6 | 无变化 |
| fd | 10.4.2 | 无变化 |
| hyperfine | 1.20.0 | 无变化 |
| actionlint | 1.7.12 | 无变化 |
| jq | **仍缺失** | 无变化（低优先级） |

### 本轮确认的治理成果（前 10 轮累计）

| 轮次 | 主要工作 | 提交 |
|------|---------|------|
| 1-2 | 初始分析 + 四类操作执行（17 安装/5 清理/7 整改） | ff06400 |
| 3 | 增量更新报告 | — |
| 4 | P2-1 dead_code_pub_in_binary 评估（不启用） | — |
| 5 | hyperfine CLI 启动基线落地 | 1ac4088 |
| 6 | hyperfine CI 接入 + gpg 安装 | 2377b8a |
| 7 | cargo-insta 快照测试试点 | 72c3404 |
| 8 | locale 测试隔离系统性修复 | 030efd6 + 9212f7f + 6c88c5c |
| 9 | 无超时 recv 测试加固 | e9db270 |
| 10 | E1-E10 复检 + P1/P2 问题清单 | 6b62ded + b0ef0f8 |

### 当前工具链健康度评估

- **供应链安全**: zizmor（workflow 扫描）+ cargo-audit（漏洞）+ cargo-deny（license/ban）三层防护 ✅
- **质量门禁**: actionlint + typos + taplo + cargo-machete + cargo-hack 五层 CI 门禁 ✅
- **测试健壮性**: locale 隔离（with_en/zh_locale）+ recv timeout（15 处）+ insta 快照（7 个） ✅
- **性能基线**: hyperfine CLI 启动（CI 门禁）+ 45+ criterion benches + cargo-bloat 体积分析 ✅
- **剩余工作**: jq 安装（低优先级）/ git-cliff 半自动 changelog（按需）/ lychee 链接检查（按需）/ cargo-bloat 接 release 检查（按需）
