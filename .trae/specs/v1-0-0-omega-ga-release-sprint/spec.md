# v1.0.0-omega GA 发布冲刺 Spec

## Why

v1.0.0-omega 源码与 CI 配置已就绪(34/34 crate、3002+ 测试、5 平台 matrix、Docker GHCR 推送、checksums.txt 生成、GitHub Release 自动创建均已配置),但实际 GA 发布状态存在 **3 类未闭合卡点**:

1. **版本号漂移**:`workspace.package.version = 1.0.0-omega`,但 git 已推送 `v1.0.1-omega`(2026-06-29 pre-release hardening)与 `v1.0.2-omega`(2026-07-04 文档同步 + 安装脚本加固)两个 tag,导致"GA 候选版本"语义模糊。
2. **CI 实跑状态未核验**:仓库为私有(`Yoloccyt/Chimera-CLI-.git`),`WebFetch` 无法读取 Actions 页面,5 平台 binary 产物 / Docker GHCR 镜像 / GitHub Release 页面是否真正发布成功,目前依赖用户手动浏览器确认,缺乏结构化核验流程。
3. **Release Notes 与 tag 漂移**:`docs/release/v1.0.0-omega_release_notes.md` 定稿于 2026-06-28,未覆盖后续 v1.0.1-omega / v1.0.2-omega 两轮修复(raw URL 鉴权、install 脚本边界 case、CI workflow 加固)。

本 Spec 聚焦"最后一公里" GA 冲刺,**不引入任何代码功能变更**,仅做版本号校准、CI 状态核验、文档同步与发布页面确认。

## What Changes

### Must(阻塞 GA)

- **M1:版本号校准与 GA tag 决策**:明确 GA 版本号(`v1.0.0-omega` 直发 vs `v1.0.3-omega` 新 tag),更新 `Cargo.toml` `workspace.package.version` 与 tag 保持一致,避免"tag 与 Cargo.toml 版本号不一致"的发布反模式
- **M2:CI 实跑状态结构化核验**:用户在浏览器或本地 `gh` CLI 核对 `release.yml` / `fuzz.yml` / `audit.yml` 三个 workflow 的运行状态,形成《CI 实跑核验报告》(每 workflow 列出:run id、触发 tag、5 平台 build job 状态、test job 状态、docker job 状态、release job 状态、产物 URL)
- **M3:5 平台 binary 产物下载验证**:从 GitHub Release 页面下载 5 个 binary,逐一执行 `--version`,断言输出匹配 `^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+`
- **M4:Docker GHCR 镜像拉取验证**:执行 `docker pull ghcr.io/yoloccyt/chimera-cli-:<ga-tag>` + `docker run --rm <image> --version`,断言镜像体积 < 100MB 且 `--version` 输出正确
- **M5:checksums.txt 完整性验证**:下载 Release 附件 `checksums.txt`,断言包含 5 行(5 平台产物),每行 SHA256 与实际下载文件 `sha256sum` 一致

### Should(GA 前强烈建议)

- **S1:Release Notes 终稿同步**:在 `docs/release/v1.0.0-omega_release_notes.md` 追加 §9 "Post-RC 修复章节",列出 v1.0.1-omega / v1.0.2-omega / v1.0.3-omega(若存在)三轮修复内容(raw URL 鉴权、install 脚本加固、CI workflow timeout 等),并标注"本文件为 GA 终稿"
- **S2:安装脚本 GA 端到端验证**:在 Linux x86_64 + Windows x86_64 两个平台实跑 `curl -fsSL https://raw.githubusercontent.com/.../install.sh | sh` 与 `iwr -UseBasicParsing https://raw.githubusercontent.com/.../install.ps1 | iex`,断言安装成功 + `chimera --version` 可执行
- **S3:GitHub Release 页面正文核验**:确认 Release 页面 `body` 包含 5 平台 matrix 表格 + Docker 拉取命令 + `chimera --version` 验证命令;`prerelease` 标志为 `false`(GA 不应是 prerelease)

### Could(GA 后跟进)

- **C1:回滚预案文档化**:在 `docs/release/` 新增 `rollback_runbook.md`,描述 GA 失败时如何回滚 Release(tag 删除、GHCR 镜像删除、Release 转为 draft、用户通知流程)
- **C2:发布后 24h 监控清单**:列出 GA 后 24 小时需监控的指标(GitHub Release 下载量、Docker pull 量、issue 报告、audit.yml 次日运行结果)

### 不在本 Spec 范围

- 任何源码功能变更或 crate 重构(RC 阶段约束 §3.1)
- v1.1.0 路线规划(独立 spec 处理)
- Week 1-8 历史回顾(已完成,见 `CHANGELOG.md` Week 8 章节)
- 上游 rust-lang/rust-clippy issue 提交(独立任务)

## Impact

### 受影响 specs

- `week8-rc-pre-ga-deep-remediation`(RC 阶段修复已全部完成,本 Spec 是其 GA 发布接续)
- `week1-8-global-deep-audit-and-remediation`(全局深度审计已完成,本 Spec 不涉及审计维度)
- `week8-production-release-hardening`(原始发布硬化,本 Spec 是其"最后一公里"冲刺)

### 受影响代码

**Must 修复(仅版本号字段,无逻辑变更)**:
- `Cargo.toml`(根 `workspace.package.version` 字段,若决定 GA 版本为 v1.0.3-omega 则更新)
- `crates/chimera-cli/Cargo.toml`(`version.workspace = true` 继承,无需直接修改)

**Should 文档同步**:
- `docs/release/v1.0.0-omega_release_notes.md`(追加 §9 Post-RC 修复章节)
- `CHANGELOG.md`(追加 GA 冲刺章节,记录版本号决策与 CI 核验结论)
- `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md`(追加 GA 发布经验教训)

**Could 文档新增**:
- `docs/release/rollback_runbook.md`(回滚预案,新建)
- `docs/release/post_ga_monitoring_checklist.md`(发布后监控清单,新建)

### 受影响产物(外部)

- GitHub Release 页面(`https://github.com/Yoloccyt/Chimera-CLI-/releases/tag/<ga-tag>`)
- GHCR Docker 镜像(`ghcr.io/yoloccyt/chimera-cli-:<ga-tag>`)
- GitHub Actions workflow 运行记录(`release.yml` / `fuzz.yml` / `audit.yml`)

## ADDED Requirements

### Requirement: GA 版本号一致性

`Cargo.toml` `workspace.package.version` SHALL 与 GA git tag 严格一致,不允许"tag 推送 v1.0.2-omega 但 Cargo.toml 仍为 1.0.0-omega"的漂移。

#### Scenario: GA tag 决策

- **WHEN** 团队评审 GA 候选版本
- **THEN** MUST 在以下两个方案中二选一:
  - **方案 A(直发 v1.0.0-omega)**:确认 v1.0.0-omega tag 的 CI 产物可用,workspace.package.version 保持 1.0.0-omega
  - **方案 B(新发 v1.0.3-omega)**:更新 workspace.package.version = "1.0.3-omega",推送新 tag v1.0.3-omega 触发 CI
- **AND** 决策结果 MUST 记录在 `CHANGELOG.md` GA 冲刺章节

#### Scenario: Cargo.toml 与 tag 一致性校验

- **WHEN** GA tag 推送后
- **THEN** `grep '^version' Cargo.toml | head -1` 输出 MUST 匹配 tag 版本号(去除 `v` 前缀)
- **AND** `cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'` 输出 MUST 匹配 tag 版本号

### Requirement: CI 实跑状态结构化核验

GA 发布前 SHALL 完成 `release.yml` / `fuzz.yml` / `audit.yml` 三个 workflow 的实跑状态核验,形成《CI 实跑核验报告》。

#### Scenario: release.yml 核验

- **WHEN** 核验 release.yml 运行状态
- **THEN** MUST 列出:run id、触发 tag、5 平台 build job 状态(每平台一项)、test job 状态、docker job 状态、release job 状态
- **AND** 所有 job MUST 为 `success`
- **AND** Release 页面 MUST 包含 5 个 binary + checksums.txt 共 6 个附件

#### Scenario: fuzz.yml 核验

- **WHEN** 核验 fuzz.yml 运行状态
- **THEN** MUST 列出:run id、触发 tag、3 个 fuzz target(seccore_sandbox / quest_parse / event_serialize)job 状态
- **AND** 所有 job MUST 为 `success`(或无 crash 上传)

#### Scenario: audit.yml 核验

- **WHEN** 核验 audit.yml 最近一次运行(每日 UTC 02:00 或 PR 触发)
- **THEN** MUST 列出:run id、触发方式、cargo audit 退出码
- **AND** 退出码 MUST 为 0(`--deny warnings` 通过)

### Requirement: 5 平台 binary 产物下载验证

GA 发布前 SHALL 从 GitHub Release 页面下载 5 个 binary 产物,逐一验证 `--version` 输出。

#### Scenario: 5 平台 binary 验证

- **WHEN** 下载 5 个 binary(chimera-windows-x86_64.exe / chimera-linux-x86_64 / chimera-linux-aarch64 / chimera-macos-x86_64 / chimera-macos-aarch64)
- **THEN** 每个 binary MUST 能在对应平台执行 `--version`
- **AND** 输出 MUST 匹配正则 `^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+`(case-sensitive)
- **AND** Windows binary 体积 MUST < 50MB,其他平台 binary 体积 MUST < 50MB

#### Scenario: 跨平台验证委托

- **WHEN** 本地只有 Windows x86_64 环境
- **THEN** Windows binary MUST 本地实跑验证
- **AND** Linux/macOS binary MUST 委托 CI 环境或用户手动在对应平台验证(在核验报告中标注"委托验证")

### Requirement: Docker GHCR 镜像拉取验证

GA 发布前 SHALL 执行 Docker 镜像拉取与功能验证。

#### Scenario: Docker 镜像拉取与运行

- **WHEN** 执行 `docker pull ghcr.io/yoloccyt/chimera-cli-:<ga-tag>`
- **THEN** 拉取 MUST 成功
- **AND** `docker image inspect` 输出 `.Size` MUST < 104857600(100MB)
- **AND** `docker run --rm <image> --version` 输出 MUST 匹配 `^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+`

### Requirement: checksums.txt 完整性验证

GA 发布前 SHALL 验证 GitHub Release 附件 `checksums.txt` 的完整性与正确性。

#### Scenario: checksums.txt 格式与内容

- **WHEN** 下载 Release 附件 checksums.txt
- **THEN** 文件 MUST 包含 5 行(每平台一行)
- **AND** 每行格式 MUST 为 `<64-hex-sha256>  <filename>`(双空格分隔)
- **AND** 5 个 filename MUST 与 Release 页面 5 个 binary 附件名一致

#### Scenario: SHA256 实际校验

- **WHEN** 下载 5 个 binary 并计算 `sha256sum`
- **THEN** 每个 binary 的实际 SHA256 MUST 与 checksums.txt 中对应行的 hash 一致

## MODIFIED Requirements

### Requirement: Release Notes 终稿

`docs/release/v1.0.0-omega_release_notes.md` SHALL 追加 §9 "Post-RC 修复章节",覆盖 v1.0.1-omega / v1.0.2-omega(及 v1.0.3-omega 若存在)的修复内容,并标注"GA 终稿"。

#### Scenario: Post-RC 修复章节内容

- **WHEN** 阅读 release notes §9
- **THEN** MUST 看到 v1.0.1-omega 修复列表(raw URL 鉴权、CI workflow timeout、RUST_BACKTRACE 等)
- **AND** MUST 看到 v1.0.2-omega 修复列表(install 脚本加固、文档同步等)
- **AND** MUST 标注"本文件为 GA 终稿,后续修复见 CHANGELOG.md"

## REMOVED Requirements

无。本 Spec 不删除任何已有需求,仅新增 GA 冲刺核验项。
