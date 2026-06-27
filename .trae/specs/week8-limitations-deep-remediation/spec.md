# Week 8 限制深度攻坚 Spec(Deep Remediation)

## Why

前一轮 `week8-limitations-remediation` 已处理 5 项已知限制,其中 3 项未完全解除:

| 限制 | 前一轮状态 | 本轮目标 |
|------|-----------|---------|
| 限制 1(cargo-fuzz 3 target) | ⚠️ 部分解解(nightly + cargo-fuzz 已装,平台限制未运行) | ✅ 完全解除(CI 实际运行) |
| 限制 5(clippy 并行栈溢出) | ⚠️ workaround 改进(--jobs 2) | ✅ 根因分析 + 上游 issue |
| 限制 2+3(CI + Docker) | ℹ️ 委托 CI(静态验证 10/10) | ✅ 实际触发 CI + 补充 Docker job |

用户已确认 3 个关键决策点:
1. GitHub 远程仓库可用,授权 git push + 推送 tag 触发 CI
2. 授权安装 procdump(~2MB,Sysinternals 官方工具)
3. 接受 CI 验证作为"完全解除"标准

## What Changes

### 限制 5(clippy 根因分析)— Must

* **procdump 捕获**:安装 procdump,在 clippy 默认并行 jobs 崩溃时捕获 `clippy-driver.exe` 的 crash dump
* **WinDbg/调用栈分析**:分析 dump 文件,定位崩溃的具体函数/模块(区分 `/GS` 检查、`__fastfail`、堆损坏)
* **根因分析报告**:生成 `docs/dev/clippy_root_cause_analysis.md`,包含 dump 分析、调用栈、根因结论
* **上游 issue 草稿**:基于 dump 证据 + 3 组对比实验,撰写 rust-lang/rust-clippy 上游 issue 草稿 `docs/dev/upstream_clippy_issue_draft.md`

### 限制 1(cargo-fuzz CI 集成)— Should

* **新增 fuzz workflow**:创建 `.github/workflows/fuzz.yml`,ubuntu-latest runner,nightly 工具链,cargo-fuzz install,3 个 target 各 300s
* **CI 触发验证**:推送 tag 或手动触发,验证 3 个 target 在 Linux 下实际运行无 panic
* **结果归档**:fuzz 运行结果(执行次数、覆盖率、panic 信息)归档到安全报告

### 限制 2+3(CI 触发 + Docker job)— Should

* **补充 release.yml Docker job**:新增 `docker` job,使用 `docker/build-push-action@v5` 构建镜像并推送 GHCR
* **CI 触发验证**:推送 `v1.0.1-omega`(或类似)tag 触发 release.yml,验证 5 平台 binary + Docker 镜像构建
* **产物验证**:5 平台 binary 上传 artifact + Docker 镜像 pull 测试

### 文档同步(收尾)

* 验收报告、CHANGELOG、project_memory、checklist、release_notes 同步更新

## Impact

* Affected specs: `week8-limitations-remediation`(3 项未完全解除项清零)
* Affected code:
  * `.github/workflows/fuzz.yml`(新建)
  * `.github/workflows/release.yml`(补充 Docker job)
  * `docs/dev/clippy_root_cause_analysis.md`(新建)
  * `docs/dev/upstream_clippy_issue_draft.md`(新建)
  * `docs/security/week8_security_report.md`(更新 fuzz 实际运行结果)
  * `docs/release/v1.0.0-omega_release_notes.md`(更新已知限制状态)
  * `CHANGELOG.md`(追加深度攻坚章节)
  * `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md`(追加经验教训)

## ADDED Requirements

### Requirement: clippy 崩溃 dump 捕获与根因分析

系统 SHALL 通过 procdump 捕获 `clippy-driver.exe` 在默认并行 jobs 下的崩溃 dump,并通过调用栈分析定位根因。

#### Scenario: procdump 捕获

* **WHEN** 运行 `cargo clippy --workspace --all-targets`(默认 jobs)并同时用 procdump 监控
* **THEN** clippy-driver.exe 崩溃时,procdump 自动生成 `.dmp` 文件
* **AND** dump 文件包含完整的调用栈信息

#### Scenario: 根因分析

* **WHEN** 分析 dump 文件
* **THEN** 输出根因分析报告,明确区分以下可能性:
  * `/GS` 缓冲区安全检查触发
  * `__fastfail` 主动快速失败
  * 堆损坏检测
  * 栈空间耗尽(已排除)
* **AND** 报告包含调用栈、崩溃模块、根因结论

### Requirement: clippy 上游 issue 草稿

系统 SHALL 生成 rust-lang/rust-clippy 上游 issue 草稿,附 dump 证据 + 3 组对比实验数据。

#### Scenario: issue 草稿

* **WHEN** 根因分析完成
* **THEN** 生成 `docs/dev/upstream_clippy_issue_draft.md`,包含:
  * 现象描述(exit code 0xC0000409)
  * 复现步骤(Windows GNU + 默认 jobs)
  * 3 组对比实验(--jobs 1/2/默认)
  * RUST_MIN_STACK 无效证据
  * dump 分析结论
  * 环境信息(rustc/clippy/Windows 版本)

### Requirement: cargo-fuzz CI workflow

系统 SHALL 提供 `.github/workflows/fuzz.yml`,在 ubuntu-latest runner 上运行 3 个 fuzz target 各 300s。

#### Scenario: fuzz CI 运行

* **WHEN** 推送 tag 或手动触发 fuzz workflow
* **THEN** ubuntu-latest runner 安装 nightly + cargo-fuzz
* **AND** 运行 `cargo +nightly fuzz run quest_parse -- -max_total_time=300`
* **AND** 运行 `cargo +nightly fuzz run seccore_sandbox -- -max_total_time=300`
* **AND** 运行 `cargo +nightly fuzz run event_serialize -- -max_total_time=300`
* **AND** 3 个 target 均无 panic 则通过

### Requirement: release.yml Docker job

系统 SHALL 在 `release.yml` 中新增 `docker` job,构建 distroless 镜像并推送 GHCR。

#### Scenario: Docker 镜像构建

* **WHEN** 推送 `v*.*.*-omega` tag 触发 release workflow
* **THEN** `docker` job(needs: build)使用 `docker/build-push-action@v5` 构建镜像
* **AND** 镜像推送到 `ghcr.io/${{ github.repository }}:${{ github.ref_name }}`
* **AND** 镜像体积 < 100MB

### Requirement: CI 实际触发与产物验证

系统 SHALL 通过推送 tag 实际触发 CI,并验证 5 平台 binary + Docker 镜像 + fuzz 结果。

#### Scenario: CI 触发

* **WHEN** 推送 `v1.0.1-omega` tag(或类似)到远程仓库
* **THEN** release.yml(fuzz.yml 可单独触发)开始运行
* **AND** 5 平台 matrix build job 全部成功
* **AND** docker job 构建并推送镜像成功
* **AND** GitHub Release 创建成功,附 5 平台 binary

#### Scenario: 产物验证

* **WHEN** CI 完成
* **THEN** 验证 5 平台 binary artifact 已上传(retention 30 天)
* **AND** 验证 Docker 镜像可 `docker pull`(或至少 GHCR 上存在)
* **AND** 验证 fuzz workflow 3 个 target 无 panic

## MODIFIED Requirements

### Requirement: 限制 1(cargo-fuzz)状态升级

前一轮 `week8-limitations-remediation` 标注为"部分解除"。本轮升级为"完全解除"(CI 实际运行通过)。

### Requirement: 限制 5(clippy)状态升级

前一轮标注为"workaround 改进"。本轮升级为"根因分析完成 + 上游 issue 已提交"(本地 workaround 保持 --jobs 2)。

### Requirement: 限制 2+3(CI+Docker)状态升级

前一轮标注为"委托 CI"。本轮升级为"CI 实际触发通过 + Docker job 已补充"。

## REMOVED Requirements

无。

## 范围约束

* 本 spec 仅处理 3 项未完全解除限制,不涉及新功能
* 不修改任何 crate 代码 / Cargo.toml / 测试代码(已发布 v1.0.0-omega)
* 仅修改 CI workflow(fuzz.yml 新建 + release.yml 补充 Docker job)
* procdump 仅用于分析,不修改 clippy 配置
* 上游 issue 草稿不实际提交到 rust-lang/rust-clippy 仓库(仅生成草稿)

## 质量验收基准

* clippy 根因分析报告包含 dump 调用栈 + 根因结论
* 上游 issue 草稿完整(现象/复现/实验/证据/环境)
* fuzz.yml 在 Linux CI 运行 3 target 各 300s 无 panic
* release.yml Docker job 构建镜像 < 100MB 并推送 GHCR
* CI 实际触发,5 平台 binary + Docker 镜像验证通过
* 5 份文档同步(checklist/CHANGELOG/project_memory/release_notes/验收报告)
