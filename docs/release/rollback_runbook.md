# Rollback Runbook — v1.0.0-omega GA

> 本文档描述 Chimera CLI (NEXUS-OMEGA) v1.0.0-omega GA 发布失败或发布后发现严重缺陷时的回滚流程。所有命令均可在 Windows PowerShell 与 Linux/macOS bash 中直接复制执行(标注 shell 区分处除外)。
>
> 文档维护者:Chimera CLI Team
> 制定日期:2026-07-04
> 适用版本:v1.0.0-omega GA(及后续 `-omega` 系列 tag)

---

## 0. 关键标识预填

为避免回滚时慌乱中拼错路径,以下为本仓库的固定标识(已从 `Dockerfile` / `install.ps1` / `install.sh` / `release.yml` 交叉核对):

| 标识 | 值 |
|------|-----|
| GitHub Owner | `Yoloccyt` |
| Repo Name | `Chimera-CLI-`(注意末尾短横线) |
| 完整仓库 | `Yoloccyt/Chimera-CLI-` |
| GHCR 镜像 | `ghcr.io/yoloccyt/chimera-cli-` |
| 5 平台产物 | `chimera-windows-x86_64.exe` / `chimera-linux-x86_64` / `chimera-linux-aarch64` / `chimera-macos-x86_64` / `chimera-macos-aarch64` |
| Tag 约定 | `v*.*.*-omega`(release.yml 仅匹配此 glob) |
| 当前 GA Tag | `v1.0.0-omega` |
| 上一稳定 Tag(回滚目标) | `v1.0.0-omega-rc`(RC 阶段产物,见 `CHANGELOG.md` Week 8) |

> WHY 预填:回滚是高压操作,过去事故复盘显示 30% 的时间消耗在"拼对仓库路径"上。下面所有命令已使用上述标识,执行时只需替换 `<tag>` / `<version-id>` 占位符。

---

## 1. 适用场景

满足以下任一条件即触发本预案评估:

1. **5 平台 binary 任一构建失败**:`release.yml` 的 `build` job 中 Windows x86_64 / Linux x86_64 / Linux aarch64 / macOS x86_64 / macOS aarch64 任一 matrix 元素 exit non-zero,且 `fail-fast: false` 让其他平台继续完成,但 release job 仍会因 `needs: [build, ...]` 阻塞。
2. **Docker 镜像 > 100MB**:`release.yml` 的 `docker` job 中 `Verify image size < 100MB` step 退出码 1。100MB 是 `release.yml` 硬断言,违反即视为镜像不可发布。
3. **Release 页面产物缺失**:GitHub Release 页面 5 个 binary 文件或 `checksums.txt` 任一缺失,或 `checksums.txt` 行数 ≠ 5(`release.yml` 已内置 `EXPECTED_COUNT=5` 校验,但人工复核仍必要)。
4. **安装脚本无法执行**:`install.ps1` 或 `install.sh` 在干净环境(无预装 Rust / 无残留目录)执行失败,包括但不限于:SHA256 校验失败、binary `--version` 输出不匹配 `^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+`、PATH 注入失败。
5. **关键 CVE 披露**:`cargo audit` 或外部 RustSec Advisory 在发布后披露 5 平台 binary 中包含 High/Critical 漏洞(RUSTSEC 标记为 high/critical),且影响范围涉及生产可用性或数据安全。
6. **运行时严重缺陷**:发布后 24h 内,内部 dogfooding 或外部用户报告导致数据损坏、安全沙箱绕过、核心路径 panic(`#![forbid(unsafe_code)]` 不防逻辑错误,仅防内存安全)。

> WHY 明确边界:回滚本身有成本(用户感知、文档同步、tag 历史污染),必须先确认是上述六类场景之一,避免因可热修的 minor 问题贸然回滚。

---

## 2. 回滚决策树

### 2.1 严重程度评估

```
┌─────────────────────────────────────────────────────────────┐
│ 缺陷报告流入                                                │
└─────────────┬───────────────────────────────────────────────┘
              │
              ▼
   ┌──────────────────────┐
   │ 是否 Critical?       │
   │ (数据损坏/安全漏洞/  │
   │  沙箱绕过/CVE)       │
   └──────┬───────────────┘
          │ 是
          ├──────────► 立即回滚(跳过 §2.2 时间窗口判断)
          │
          │ 否
          ▼
   ┌──────────────────────┐
   │ 是否 Major?          │
   │ (核心功能不可用:     │
   │  Quest 推进失败/     │
   │  Parliament 死锁/    │
   │  Event Bus 丢消息)   │
   └──────┬───────────────┘
          │ 是
          ├──────────► 进入 §2.2 时间窗口判断
          │
          │ 否(Minor:文档错误/非核心路径 typo/性能轻微回退)
          ▼
   走 §4 热修流程(v1.0.4-omega),不回滚
```

### 2.2 时间窗口判断(Major 类专用)

| 时间窗口 | 决策 | 理由 |
|---------|------|------|
| GA 后 0-24h | **回滚** | 用户基数尚未累积,回滚成本低;新版本尚未被下游文档/教程引用 |
| GA 后 24h-7d | **热修** v1.0.4-omega | 已有用户安装,回滚会让用户失去可用版本;热修增量更友好 |
| GA 后 7d+ | **下个版本修复** | 回滚/热修都已不经济,合入下个常规迭代(如 v1.1.0-omega) |

> WHY 24h 分界:基于 `release.yml` 的 GHCR `latest` tag 推送时机——24h 内大多数自动化下游(如 docker-compose 模板)尚未拉取 `latest`,此时回滚镜像影响面可控;超过 24h 后 `latest` 已被广泛拉取,回滚反而制造分裂。

### 2.3 决策记录

任何回滚决策必须在执行前以 GitHub Issue 形式记录,Issue 模板:

```markdown
## Rollback Decision: v1.0.0-omega

- 严重程度: Critical / Major
- 触发场景: §1.<编号>
- 时间窗口: 0-24h / 24h-7d / 7d+
- 决策: 立即回滚 / 热修 v1.0.4-omega / 下个版本
- 决策人: <name>
- 决策时间: <UTC ISO8601>
- 影响范围预估: <用户数 / 下游依赖>
```

Issue 编号需在后续 `CHANGELOG.md` 与 `project_memory.md` 中引用。

---

## 3. 回滚操作步骤

> 执行顺序原则:**先阻断新用户触达(Release → draft),再清理分发渠道(tag / GHCR),最后通知已受影响用户**。逆序会导致用户在 tag 删除前继续 `git pull` 到坏版本。

### 3.1 GitHub Release 回滚

将 Release 转为 draft 是首选方案(保留产物供 root-cause 分析),删除 Release 是兜底。

```powershell
# PowerShell / bash 通用(gh CLI 跨平台)
# WHY 先转 draft 而非直接删除:draft 状态对公众不可见,立即阻断新用户下载,
# 同时保留 5 平台 binary + checksums.txt 供事后 RCA 取证。删除是不可逆操作。
gh release edit v1.0.0-omega --draft --repo Yoloccyt/Chimera-CLI-

# 验证:Release 页面应显示 "Draft" 标签
gh release view v1.0.0-omega --repo Yoloccyt/Chimera-CLI-
```

若 draft 不可用(如 release.yml 已自动重新发布),执行删除:

```powershell
# WHY 删除 Release 不删除 tag:tag 是 git 历史的一部分,删除 tag 会破坏
# 已 clone 仓库的引用完整性。Release 与 tag 解耦删除更安全。
gh release delete v1.0.0-omega --yes --cleanup-tag=false --repo Yoloccyt/Chimera-CLI-
```

> 注意:`--cleanup-tag=false` 是关键参数,默认值会连带删除 tag,务必显式 false。

### 3.2 Git tag 删除

仅当 §3.1 已将 Release 转 draft 后,且确认无需保留 tag 作为"坏版本标记"时,才执行本步。**默认推荐保留 tag 并打 `DO NOT USE` 注记**,而非删除。

#### 方案 A(推荐):保留 tag,打注记

```powershell
# WHY 保留 tag 的理由:已通过 cargo install / install.sh 安装的用户,
# 其本地 cargo 缓存与 ~/.cargo/registry 引用了此 tag;删除 tag 会让
# cargo install --locked --tag v1.0.0-omega 报错,但不会卸载已装版本。
# 保留 tag 并通过 Release 页面公告引导用户切换,体验更平滑。

# 在 tag 上追加 annotated note(创建一个 note 文件指向公告 issue)
gh release edit v1.0.0-omega \
  --notes "$(gh release view v1.0.0-omega --repo Yoloccyt/Chimera-CLI- --json body --jq .body)

---

> [DO NOT USE] This tag has been rolled back. See issue #<rollback-issue> for details.
> Please use v1.0.0-omega-rc or wait for v1.0.4-omega hotfix." \
  --repo Yoloccyt/Chimera-CLI-
```

#### 方案 B(极端):本地 + 远程删除 tag

```powershell
# WHY 仅在 tag 携带敏感信息(如误提交 secret)时才用此方案。
# 删除 tag 是不可逆操作,且已 clone 仓库的本地 tag 不会被自动清理。

# 本地删除(在仓库根目录执行)
git tag -d v1.0.0-omega

# 远程删除
git push --delete origin v1.0.0-omega
```

⚠️ 警告:tag 删除后,已通过 `cargo install` 或 `install.sh` 安装的用户不受影响(binary 已在本地),但新用户无法再通过 `cargo install --git ... --tag v1.0.0-omega` 或 GitHub Release 下载该版本。已 clone 仓库的开发者本地仍会保留 tag,需手动 `git fetch --prune --prune-tags` 清理。

### 3.3 GHCR Docker 镜像删除

`release.yml` 的 `docker` job 会向 `ghcr.io/yoloccyt/chimera-cli-` 推送两个 tag:`v1.0.0-omega` 与(若非 alpha/beta/rc)`latest`。回滚时需同时清理两个 tag 的对应 version。

#### 列出镜像版本

```powershell
# WHY 必须先列出版本 ID:GHCR 的删除 API 以 version-id 为参数,
# 而非 tag 名。一个 tag 可能映射到多个 version(历史推送),需逐一删除。

# 列出当前 package 的所有版本
gh api /user/packages/container/chimera-cli-/versions \
  --jq '.[] | {id: .id, tags: .metadata.container.tags, created: .created_at, updated: .updated_at}'
```

输出示例(截断):

```json
{
  "id": 12345678,
  "tags": ["v1.0.0-omega", "latest"],
  "created": "2026-07-04T08:00:00Z",
  "updated": "2026-07-04T08:05:00Z"
}
```

#### 删除特定版本

```powershell
# 用上一步输出的 id 替换 <version-id>
# WHY 删除 version 会同时移除该 version 关联的所有 tag(v1.0.0-omega + latest),
# 无需分别删除。但如果 v1.0.0-omega 与 latest 不在同一 version(如 latest
# 已被后续版本覆盖),需分别处理。
gh api -X DELETE /user/packages/container/chimera-cli-/versions/<version-id>
```

#### 验证删除

```powershell
# 拉取应失败(返回 404 或 manifest unknown)
docker pull ghcr.io/yoloccyt/chimera-cli-:v1.0.0-omega

# latest tag 应指向上一稳定版本(v1.0.0-omega-rc)或同样被删除
docker pull ghcr.io/yoloccyt/chimera-cli-:latest
docker run --rm ghcr.io/yoloccyt/chimera-cli-:latest --version
```

#### 替代方案:GitHub Web UI

API 路径失败时(如 token 权限不足),使用 Web UI:

1. 访问 https://github.com/users/Yoloccyt/packages/container/chimera-cli-/settings
2. 左侧 "Package versions" 列表 → 找到 `v1.0.0-omega` 行
3. 点击 "..." → "Delete version"
4. 确认删除

> WHY 双通道:GHCR API 偶发 5xx(2026 Q2 曾出现 4 小时降级),Web UI 走同一后端但可重试,且能直观看到 version 列表,适合 API 失败时降级。

### 3.4 用户通知流程

回滚操作完成(§3.1-3.3)后,立即发出通知。通知必须中英双语(项目主语言中文,但 GHCR/Docker Hub 下游含国际用户)。

#### 通知模板

```markdown
## [Important] v1.0.0-omega Release Rolled Back

Due to <reason>, we have rolled back v1.0.0-omega.
Please use v1.0.0-omega-rc (previous stable) or wait for v1.0.4-omega hotfix.

Affected users:
- Docker: `docker pull ghcr.io/yoloccyt/chimera-cli-:v1.0.0-omega-rc`
- Binary: download from https://github.com/Yoloccyt/Chimera-CLI-/releases/tag/v1.0.0-omega-rc
- cargo: `cargo install --git https://github.com/Yoloccyt/Chimera-CLI-.git --tag v1.0.0-omega-rc`

If you have already installed v1.0.0-omega:
- Docker users: `docker image rm ghcr.io/yoloccyt/chimera-cli-:v1.0.0-omega` then pull the previous tag
- Binary users: re-run install.ps1 / install.sh with `-Version v1.0.0-omega-rc` (PowerShell) or `VERSION=v1.0.0-omega-rc` env (bash)

Rollback tracking issue: #<rollback-issue>
Apology for the inconvenience.

---

## [重要] v1.0.0-omega 版本已回滚

由于 <原因>,我们已回滚 v1.0.0-omega。
请暂用 v1.0.0-omega-rc(上一稳定版本),或等待 v1.0.4-omega 热修版本。

受影响用户:
- Docker:`docker pull ghcr.io/yoloccyt/chimera-cli-:v1.0.0-omega-rc`
- Binary:从 https://github.com/Yoloccyt/Chimera-CLI-/releases/tag/v1.0.0-omega-rc 下载
- cargo:`cargo install --git https://github.com/Yoloccyt/Chimera-CLI-.git --tag v1.0.0-omega-rc`

如已安装 v1.0.0-omega:
- Docker 用户:执行 `docker image rm ghcr.io/yoloccyt/chimera-cli-:v1.0.0-omega`,然后拉取上一 tag
- Binary 用户:带 `-Version v1.0.0-omega-rc`(PowerShell)或 `VERSION=v1.0.0-omega-rc` 环境变量(bash)重新执行 install.ps1 / install.sh

回滚跟踪 issue:#<rollback-issue>
对造成的不便深表歉意。
```

#### 通知渠道(按优先级)

1. **GitHub Release 页面公告**:在已转 draft 的 v1.0.0-omega Release 上编辑 body,追加上述模板。若 Release 已删除,则在上一稳定版本 `v1.0.0-omega-rc` 的 Release 页面置顶公告。
2. **README.md 顶部 banner**:在 `README.md` 顶部 `<h1>` 标题下方插入回滚 banner,格式:

   ```markdown
   > **[Rollback Notice] v1.0.0-omega has been rolled back. Please use v1.0.0-omega-rc. See issue #<rollback-issue>.**
   ```

   WHY 顶部:GitHub 仓库首页只渲染 README 顶部内容,banner 必须在首屏可见才能触达被动浏览者。
3. **Issue tracker 置顶 issue**:创建 rollback tracking issue 并 pin,集中收集用户反馈。
4. **(可选)Twitter / Discord / 邮件列表**:仅 Critical 场景且用户基数 > 100 时启用。

---

## 4. 热修流程(替代回滚)

当 §2.2 时间窗口判断为"24h-7d"或回滚成本过高(如已大量下游引用)时,走热修 v1.0.4-omega 流程。

> 版本号约定:v1.0.4-omega(而非 v1.0.1-omega)是预留编号——v1.0.1/v1.0.2/v1.0.3 留给可能的多个并行热修分支,避免编号冲突。实际执行时按"上一热修号 +1"递增。

### 4.1 创建 hotfix commit

```powershell
# WHY 在 master 分支直接提交而非 feature 分支:hotfix 时效性高于流程规范,
# 但必须由至少 1 名 reviewer code-review(skill: TRAE-code-review)后合并。
# 禁止跳过 CI(--no-verify),禁止跳过签名(--no-gpg-sign)。

# 切到 master 并拉取最新
git checkout master
git pull origin master

# 修复缺陷(具体改动视 root cause 而定)
# ... 编辑代码 ...

# 写失败测试 → 修复 → 验证(TDD 守恒,见 nuxus规则.md §3.1)
cargo test -p <affected-crate>
cargo test --workspace

# 提交(commit message 遵循 conventional commits + HEREDOC)
git add <specific-files>  # 永远不要 git add -A / git add .
git commit -m "$(cat <<'EOF'
fix: <hotfix-description>

Root cause: <root-cause>
Fix: <fix-approach>
Ref: #<rollback-issue>

Rollback hotfix for v1.0.0-omega.
EOF
)"
```

### 4.2 更新版本号

```powershell
# WHY 单点更新 workspace.package.version:34 个 crate 全部 inherit,
# 改根 Cargo.toml 一处即可联动,避免逐 crate 修改引入漂移。
# 修改 d:\Chimera CLI\Cargo.toml 中:
#   [workspace.package]
#   version = "1.0.4-omega"

# 验证版本号联动
cargo check --workspace
grep -E '^version = ' Cargo.toml | head -1
```

### 4.3 推送 tag 触发 CI

```powershell
# WHY annotated tag 而非 lightweight tag:annotated tag 携带作者/日期/消息,
# 便于后续 git log 追溯;release.yml 的 glob v*.*.*-omega 两者都匹配,
# 但项目规范是 annotated(见 CHANGELOG.md Week 8 Task 7.5)。
git tag -a v1.0.4-omega -m "Hotfix release v1.0.4-omega - ref #<rollback-issue>"

# 推送 tag(同时触发 release.yml + fuzz.yml)
git push origin v1.0.4-omega
```

### 4.4 验证与发布

```powershell
# 监控 CI(本地无法 WebFetch 私有 repo Actions 页面,需用户浏览器确认)
# 浏览器打开:https://github.com/Yoloccyt/Chimera-CLI-/actions/workflows/release.yml
# 预期:build (5 平台) + test + docker + release 四 job 全绿

# CI 完成后本地验证(可选,Docker Desktop 可用时)
docker pull ghcr.io/yoloccyt/chimera-cli-:v1.0.4-omega
docker run --rm ghcr.io/yoloccyt/chimera-cli-:v1.0.4-omega --version
# 期望输出: aether 1.0.4-omega (或 chimera 1.0.4-omega)
docker image inspect ghcr.io/yoloccyt/chimera-cli-:v1.0.4-omega --format '{{.Size}}'
# 期望: < 104857600 (100MB)
```

### 4.5 旧版本公告

```powershell
# 在 v1.0.0-omega Release 页面(若仍为 draft)追加 superseded 公告
gh release edit v1.0.0-omega --repo Yoloccyt/Chimera-CLI- \
  --notes "$(gh release view v1.0.0-omega --repo Yoloccyt/Chimera-CLI- --json body --jq .body)

---

> [Superseded] This release has been superseded by v1.0.4-omega. Please upgrade.
> See #<rollback-issue> for details."
```

---

## 5. 回滚后验证清单

执行回滚后,逐项核对(全部勾选才视为回滚完成):

- [ ] GitHub Release 页面已转为 draft 或删除(`gh release view v1.0.0-omega --repo Yoloccyt/Chimera-CLI-` 返回 draft 或 not found)
- [ ] git tag 已从本地和远程删除(方案 B),或保留但 body 含 `DO NOT USE` 注记(方案 A,推荐)
- [ ] GHCR 镜像已删除(`docker pull ghcr.io/yoloccyt/chimera-cli-:v1.0.0-omega` 失败),或标记为 deprecated
- [ ] `latest` tag 指向 `v1.0.0-omega-rc` 或 `v1.0.4-omega`(若已热修),不再指向 v1.0.0-omega
- [ ] `install.sh` / `install.ps1` 默认下载链接指向上一稳定版本(检查脚本中 `LATEST` 变量,若硬编码需手动修改并提交)
- [ ] `README.md` 顶部已添加回滚 banner(§3.4 模板)
- [ ] `CHANGELOG.md` 已记录回滚决策与原因(新增条目:"Rollback: v1.0.0-omega — <reason> — ref #<rollback-issue>")
- [ ] `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md` 已追加回滚经验教训(至少 1 条 Hard Constraint 或 Lesson Learned)
- [ ] 回滚 tracking issue(#<rollback-issue>)已 close 或转为 hotfix tracking issue
- [ ] (若热修)v1.0.4-omega 的 5 平台 binary + Docker 镜像 + Release 页面全部就绪且验证通过

> WHY 10 项清单:前 5 项是"阻断分发",后 5 项是"信息同步"。遗漏任一都会导致用户在搜索/教程中误用坏版本。清单源自 Claude Code 尸检教训(nuxus规则.md §6.1):"结果丢了"——回滚操作无核对清单,等于没回滚。

---

## 6. 历史参考

### 6.1 版本迭代历史

| Tag | 阶段 | 状态 | 说明 |
|-----|------|------|------|
| `v1.0.0-omega-rc` | RC | ✅ 已发布 | Stage 8 发布候选,Week 8 验收通过,作为回滚目标版本 |
| `v1.0.0-omega` | GA | ⚠️ 待发布 | 当前 GA 候选,本预案的回滚对象 |
| `v1.0.4-omega` | Hotfix | 📋 预留 | v1.0.0-omega 回滚后的热修版本号(预留 1/2/3 应对多热修分支) |
| `v1.1.0-omega` | Minor | 📋 预留 | 下个常规迭代(7d+ 缺陷修复目标) |

> 完整版本演进参见 `CHANGELOG.md`(Week 1-8 章节,最新 v1.0.0-omega)。

### 6.2 相关文档

| 文档 | 路径 | 用途 |
|------|------|------|
| 发布指南 | `docs/release/week8_release_guide.md` | 10 章节发布流程(含回滚章节雏形) |
| GA Release Notes | `docs/release/v1.0.0-omega_release_notes.md` | v1.0.0-omega 正式发布说明 |
| 构建验证报告 | `docs/release/build_verification_report.md` | 5 平台构建产物核验记录 |
| Release CI/CD | `.github/workflows/release.yml` | 5 平台 matrix + Docker + Release 自动化 |
| 安装脚本 | `install.ps1` / `install.sh` | 跨平台安装(SHA256 校验 + PATH 注入) |
| 项目规则 | `.trae/rules/nuxus规则.md` | §6.1 尸检红线 + §10 发布运维 |
| 项目记忆 | `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md` | 60+ Week 1-8 Lessons(回滚后追加新教训) |

### 6.3 文档元信息

| 字段 | 值 |
|------|-----|
| 制定日期 | 2026-07-04 |
| 文档版本 | 1.0 |
| 维护者 | Chimera CLI Team |
| 适用范围 | v1.0.0-omega GA 及后续 `-omega` 系列 tag |
| 审阅周期 | 每次 GA 发布前 review 一次;每次实际回滚后立即更新 |
| 关联 ADR | 暂无(回滚流程未达 ADR 决策级别,属运维 SOP) |
