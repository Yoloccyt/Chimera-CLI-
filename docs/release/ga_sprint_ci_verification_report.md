# GA 冲刺 CI 核验报告 — v1.0.0-omega

> 本报告汇总 v1.0.0-omega GA 发布冲刺期间的 CI 实跑状态核验结果。
> 由于仓库为私有,WebFetch 无法访问 Actions 页面,以下核验需用户在浏览器或本地 gh CLI 手动完成。
- 报告日期:2026-07-04
- GA tag:v1.0.0-omega
- 核验人:[待用户填写]

## 1. release.yml 核验

**核验方式**:用户在浏览器打开 `https://github.com/Yoloccyt/Chimera-CLI-/actions/workflows/release.yml`,定位 v1.0.0-omega tag 触发的 run。

### 1.1 Run 基本信息
- run id:[待填写]
- 触发 tag:v1.0.0-omega
- 运行时间:[待填写,如 2026-06-28 12:34:56 UTC]
- 总耗时:[待填写,如 15m 23s]
- 总状态:[ ] success [ ] failure [ ] cancelled

### 1.2 5 平台 build job 状态

| 平台 | Target | Runner | 状态 | 耗时 | 备注 |
|------|--------|--------|------|------|------|
| Windows x86_64 | x86_64-pc-windows-gnu | windows-latest | [ ] success [ ] failure | [填写] | |
| Linux x86_64 | x86_64-unknown-linux-gnu | ubuntu-latest | [ ] success [ ] failure | [填写] | |
| Linux aarch64 | aarch64-unknown-linux-gnu | ubuntu-latest (cross) | [ ] success [ ] failure | [填写] | |
| macOS x86_64 | x86_64-apple-darwin | macos-latest | [ ] success [ ] failure | [填写] | |
| macOS aarch64 | aarch64-apple-darwin | macos-latest | [ ] success [ ] failure | [填写] | |

### 1.3 test job 状态
- 状态:[ ] success [ ] failure [ ] skipped
- 耗时:[填写]
- 测试总数:[填写,预期 3002+]
- 失败数:[填写,预期 0]

### 1.4 docker job 状态
- 状态:[ ] success [ ] failure [ ] skipped
- 耗时:[填写]
- GHCR 镜像 URL:ghcr.io/yoloccyt/chimera-cli-:v1.0.0-omega
- 镜像体积:[填写,如 95.2 MB,< 100MB 限制]
- --version 输出:[填写,如 "chimera 1.0.0-omega"]
- 体积验证:[ ] pass [ ] fail
- 功能验证:[ ] pass [ ] fail

### 1.5 release job 状态
- 状态:[ ] success [ ] failure [ ] skipped
- 耗时:[填写]
- GitHub Release URL:https://github.com/Yoloccyt/Chimera-CLI-/releases/tag/v1.0.0-omega
- Release draft 标志:[ ] false [ ] true(必须 false)
- Release prerelease 标志:[ ] false [ ] true(必须 false,GA 不应是 prerelease)
- 附件数量:[填写,预期 6 = 5 binary + checksums.txt]
- 附件清单:
  - [ ] chimera-windows-x86_64.exe
  - [ ] chimera-linux-x86_64
  - [ ] chimera-linux-aarch64
  - [ ] chimera-macos-x86_64
  - [ ] chimera-macos-aarch64
  - [ ] checksums.txt

## 2. fuzz.yml 核验

**核验方式**:用户在浏览器打开 `https://github.com/Yoloccyt/Chimera-CLI-/actions/workflows/fuzz.yml`,定位 v1.0.0-omega tag 触发的 run。

### 2.1 Run 基本信息
- run id:[待填写]
- 触发 tag:v1.0.0-omega
- 运行时间:[待填写]
- 总耗时:[待填写,预期 ~15min = 3 target × 300s + overhead]
- 总状态:[ ] success [ ] failure [ ] cancelled

### 2.2 3 个 fuzz target job 状态

| Target | 运行时长 | 状态 | crash 数 | 上传 artifact | 备注 |
|--------|---------|------|---------|--------------|------|
| seccore_sandbox | [填写,预期 300s] | [ ] success [ ] failure | [填写,预期 0] | [ ] yes [ ] no | |
| quest_parse | [填写,预期 300s] | [ ] success [ ] failure | [填写,预期 0] | [ ] yes [ ] no | |
| event_serialize | [填写,预期 300s] | [ ] success [ ] failure | [填写,预期 0] | [ ] yes [ ] no | |

**注意**:fuzz.yml 的 crash 上传为非阻塞(90 天留存),即使有 crash 不阻塞 GA,但需评估 crash 严重程度。

## 3. audit.yml 核验

**核验方式**:用户在浏览器打开 `https://github.com/Yoloccyt/Chimera-CLI-/actions/workflows/audit.yml`,定位最近一次运行。

### 3.1 Run 基本信息
- run id:[待填写]
- 触发方式:[ ] schedule(每日 UTC 02:00) [ ] pull_request [ ] workflow_dispatch
- 运行时间:[待填写]
- 总状态:[ ] success [ ] failure [ ] cancelled

### 3.2 cargo audit job 状态
- 退出码:[填写,预期 0]
- --deny warnings:[ ] pass [ ] fail
- High/Critical 漏洞数:[填写,预期 0]
- 已忽略的 RUSTSEC 告警:[填写,如 RUSTSEC-2024-xxxx ignored]

## 4. 5 平台 binary 产物验证

### 4.1 Windows x86_64 binary(本地实跑验证)
- 下载来源:https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v1.0.0-omega/chimera-windows-x86_64.exe
- 文件大小:[填写,如 6.96 MB,< 50MB 限制]
- SHA256(Get-FileHash):[填写]
- --version 输出:[填写,如 "chimera 1.0.0-omega"]
- 输出匹配正则 `^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+`:[ ] pass [ ] fail
- 验证人:[填写]
- 验证日期:2026-07-04

### 4.2 其他 4 平台 binary(委托验证)

| 平台 | 验证方式 | 验证结果 | 备注 |
|------|---------|---------|------|
| Linux x86_64 | [ ] CI 日志 [ ] WSL2 [ ] 委托用户 | [ ] pass [ ] fail [ ] pending | |
| Linux aarch64 | [ ] CI 日志 [ ] 委托用户 | [ ] pass [ ] fail [ ] pending | |
| macOS x86_64 | [ ] CI 日志 [ ] 委托用户 | [ ] pass [ ] fail [ ] pending | |
| macOS aarch64 | [ ] CI 日志 [ ] 委托用户 | [ ] pass [ ] fail [ ] pending | |

**委托验证说明**:本地只有 Windows x86_64 环境,其他 4 平台依赖 CI run 日志中的 "Verify binary runs" step 输出,或委托用户在对应平台手动验证。

## 5. Docker GHCR 镜像验证

- 拉取命令:`docker pull ghcr.io/yoloccyt/chimera-cli-:v1.0.0-omega`
- 拉取状态:[ ] success [ ] failure [ ] pending(需 Docker Desktop + GitHub 登录)
- 镜像体积(docker image inspect --format '{{.Size}}'):[填写,如 99800000 bytes = 95.27 MB]
- 体积 < 100MB(104857600 bytes):[ ] pass [ ] fail
- --version 输出(docker run --rm <image> --version):[填写]
- 输出匹配正则:[ ] pass [ ] fail
- 镜像 USER:[填写,预期 nonroot:nonroot]
- 镜像 ENV RUST_BACKTRACE:[填写,预期 1]
- 验证人:[填写]
- 验证日期:2026-07-04

## 6. checksums.txt 完整性验证

- 下载来源:https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v1.0.0-omega/checksums.txt
- 文件行数:[填写,预期 5]
- 格式校验(每行 `<64-hex-sha256>  <filename>` 双空格分隔):[ ] pass [ ] fail
- 5 个 filename 与 Release 附件一致:[ ] pass [ ] fail
- Windows binary SHA256 实际校验:
  - checksums.txt 中记录的 hash:[填写]
  - 本地 Get-FileHash 计算的 hash:[填写]
  - 一致性:[ ] match [ ] mismatch
- 其他 4 平台 SHA256 委托验证:[ ] pending [ ] done

## 7. 安装脚本端到端验证

### 7.1 Windows x86_64(install.ps1)
- 执行命令:`iwr -UseBasicParsing https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.ps1 -Headers @{Authorization="token $env:GITHUB_TOKEN"} | iex`
- 执行状态:[ ] success [ ] failure [ ] pending
- 安装后 `chimera --version` 输出:[填写]
- SHA256 校验逻辑生效:[ ] yes [ ] no(空转则 fail)

### 7.2 Linux x86_64(install.sh)
- 执行环境:[ ] WSL2 [ ] CI runner [ ] 委托用户
- 执行命令:`curl -fsSL -H "Authorization: token $GITHUB_TOKEN" https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.sh | sh`
- 执行状态:[ ] success [ ] failure [ ] pending
- 安装后 `chimera --version` 输出:[填写]
- SHA256 校验逻辑生效:[ ] yes [ ] no

## 8. GitHub Release 页面正文核验

- Release URL:https://github.com/Yoloccyt/Chimera-CLI-/releases/tag/v1.0.0-omega
- body 包含 5 平台 matrix 表格:[ ] yes [ ] no
- body 包含 Docker 拉取命令示例:[ ] yes [ ] no
- body 包含 `chimera --version` 验证命令:[ ] yes [ ] no
- prerelease 标志为 false:[ ] yes [ ] no(GA 必须非 prerelease)
- draft 标志为 false:[ ] yes [ ] no(GA 必须已发布)

## 9. 核验结论

- [ ] 全部 Must 项通过(M1-M5),可宣布 GA 发布
- [ ] 部分 Must 项 pending(委托用户核验),待用户填写后再下结论
- [ ] 部分 Must 项 fail,需启动回滚流程(参见 rollback_runbook.md)

**核验人签字**:___________________ **日期**:___________________

---

**核验说明**:
- 本报告中所有 "[待填写]" 与 "[ ] pending" 项需用户在浏览器或本地环境完成核验后填写
- 私有仓库 raw URL 鉴权:install.ps1 / install.sh 需携带 GITHUB_TOKEN header
- 委托验证项可附 CI run 截图或日志链接作为证据
- 全部 Must 项通过后,可在 README.md 顶部添加 "v1.0.0-omega GA 已发布" 声明
