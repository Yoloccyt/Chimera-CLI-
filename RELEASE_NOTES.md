# chimela CLI v1.5.7-omega Release Notes

**发布日期**: 2026-07-13
**版本**: v1.5.7-omega
**代号**: NEXUS-OMEGA (Omni-Model Engineering Generative Architecture)
**状态**: GA 后演进 patch 版本

---

## 1. 发布概要

chimela CLI v1.5.7-omega 是 v1.5.6-omega 的 GA 后演进版本。本次迭代聚焦**品牌统一**与**发布工程一致性修复**,无功能性代码变更。所有用户侧入口、安装脚本、CI/CD 产物、packaging 模板与文档均已对齐到新的品牌名 `chimela`,并保留 `chimera`/`aether` 兼容别名,确保现有脚本与用户习惯无缝迁移。

---

## 2. Highlights

- **统一品牌名**: 用户侧命令与产物名全面切换为 `chimela`;`install.ps1`、`install.sh`、`.github/workflows/release.yml`、`packaging/*` 同步更新。
- **兼容别名保留**: `chimera` 与 `aether` 仍作为入口别名存在,现有脚本无需修改即可继续工作。
- **版本引用一致性修复**: 修正 README、Dockerfile、安装脚本、packaging 与 CI 中残留的 `1.5.3`–`1.5.6` 版本引用,统一为 `1.5.7-omega`。
- **安装体验加固**: `install.ps1` 新增 TLS 1.2/1.3 强制、代理支持与企业网络重试;`install.sh` 增强私有仓库鉴权与下载容错。
- **QA 加固落地**: 引入 `serial_test` 串行化标注,隔离 `repo-wiki` 并行 flaky test;发布前压力测试命令明确在 release 模式下运行。

---

## 3. 安装

推荐使用对应平台的一键安装脚本:

```bash
# Linux / macOS
curl -fsSL https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.sh | sh

# Windows PowerShell
iex (irm https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.ps1)
```

> 本仓库为**私有仓库**,从 `raw.githubusercontent.com` 或 GitHub Release 下载时必须携带 `GITHUB_TOKEN`。详见完整安装指南。

完整安装说明(含手动下载、`.deb`、Docker、源码构建)请参见:
[docs/release/v1.5.7-omega_installation_guide.md](./docs/release/v1.5.7-omega_installation_guide.md)

---

## 4. 新变更 / 行为变更

- **用户侧命令更名**: 新品牌名为 `chimela`,文档与示例全部使用 `chimela` 或 `chimela.exe`。
- **安装产物**:
  - Windows: `install.ps1` 默认安装到 `%LOCALAPPDATA%\Programs\chimela`,同时生成 `chimela.exe`、`chimera.exe`、`aether.exe`。
  - Linux / macOS: `install.sh` 默认安装到 `~/.local/bin`,创建 `chimela` 主入口与 `chimera` 软链接别名。
- **Docker 入口**: 镜像内 binary 入口为 `chimela`,可直接运行 `docker run --rm ghcr.io/yoloccyt/chimera-cli:v1.5.7-omega --version`。
- **Release 产物命名**: 5 平台 binary 与 `.deb` 包均以 `chimela` 为前缀,例如 `chimela-windows-x86_64.exe`、`chimela-cli_1.5.7-omega_amd64.deb`。

本次变更**无破坏性**,配置格式、`omega.yaml` 结构、子命令行为与 API 契约均保持不变。

---

## 5. Bug 修复 / 一致性修复

- 修正 README 中 badge、安装示例、Docker 示例的版本号,统一为 `1.5.7-omega`。
- 修正 `install.ps1` 与 `install.sh` 的默认版本提示与示例,统一为 `v1.5.7-omega`。
- 修正 `packaging/homebrew/chimela-cli.rb`、`packaging/scoop/chimela-cli.json`、`packaging/apt/chimela-cli.control` 中的版本引用。
- 修正 `.github/workflows/release.yml` Release 正文中的 Homebrew/Scoop/APT 命令与安装指南链接,指向 `v1.5.7-omega_installation_guide.md`。
- 为 `install.ps1` 与 `scripts/check_fuzz_config.ps1` 添加 UTF-8 BOM,符合 `.editorconfig` 约定。

---

## 6. 已知问题

| 问题 | 影响 | 临时方案 |
|------|------|---------|
| Homebrew tap、Scoop bucket、APT 仓库**尚未初始化** | 无法通过 `brew install` / `scoop install` / `apt install chimela-cli` 官方源安装 | 使用一键脚本、Release 二进制直接下载、`.deb` 本地安装或 Docker 镜像 |
| 仓库为私有仓库 | `raw.githubusercontent.com` 与 GitHub Release 下载需要鉴权 | 在环境变量中设置 `GITHUB_TOKEN`,并显式传入 HTTP header |
| Windows GNU 本地 clippy 高并行可能触发 OOM | 仅影响本地 Windows 开发,不影响产物功能 | 使用 `--jobs 2` + `CARGO_INCREMENTAL=0` + `RUST_MIN_STACK=33554432` |

Homebrew / Scoop / APT 的 packaging 文件当前为模板,版本号已更新为 `1.5.7-omega`;外部仓库初始化后将自动具备分发能力。

---

## 7. 迁移说明

### 从 `chimera` 迁移

- 原有 `chimera` 命令继续可用;安装后 `chimera` 是 `chimela` 的兼容别名。
- 更新文档或脚本时,建议逐步替换为 `chimela`,但**不要求强制替换**。

### 从 `aether` 迁移

- `aether` 是 Cargo 内部 binary 名,源码构建产物仍为 `aether` / `aether.exe`。
- 通过 install 脚本或 `.deb` 安装后,`aether` 作为别名可用。
- 配置文件路径 `~/.aether/omega.yaml` 保持不变。

### 通用注意事项

- 无配置迁移成本:`omega.yaml` 结构、环境变量前缀 `AETHER_`、子命令签名均不变。
- 若使用 `.deb` 包安装,旧版本 `chimera-cli` 包名已被替换为 `chimela-cli`;建议先卸载旧包再安装,避免 `/usr/bin` 下文件冲突。

---

## 8. 发布资产

本次 Release 包含以下资产(可从 [GitHub Releases](https://github.com/Yoloccyt/Chimera-CLI-/releases) 下载):

| 平台 | 文件名 |
|------|--------|
| Windows x86_64 | `chimela-windows-x86_64.exe` |
| Linux x86_64 | `chimela-linux-x86_64` |
| Linux aarch64 | `chimela-linux-aarch64` |
| macOS x86_64 (Intel) | `chimela-macos-x86_64` |
| macOS aarch64 (Apple Silicon) | `chimela-macos-aarch64` |
| Debian / Ubuntu (amd64) | `chimela-cli_1.5.7-omega_amd64.deb` |
| 校验文件 | `checksums.txt` |

Docker 镜像:

```bash
# GHCR 镜像(tag 与 git tag 一致,镜像名已按 Docker reference 规范归一化)
docker pull ghcr.io/yoloccyt/chimera-cli:v1.5.7-omega
```

> 注意: `ghcr.io/yoloccyt/chimera-cli` 由 `Yoloccyt/Chimera-CLI-` 仓库名小写并移除末尾连字符后得到。

---

## 9. 验证

```bash
# 查看版本
chimela --version

# 查看帮助
chimela help

# 测试 Wiki 查询(需要已初始化配置或默认配置)
chimela wiki "OMEGA 四定律"
```

---

## 10. 致谢

感谢 NEXUS-OMEGA 团队对发布工程、QA 加固与文档一致性的持续投入。v1.5.7-omega 为后续外部包管理器仓库初始化与更广泛的用户分发奠定了统一的命名与版本基线。
