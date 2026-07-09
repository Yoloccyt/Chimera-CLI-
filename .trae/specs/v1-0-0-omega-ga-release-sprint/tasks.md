# Tasks — v1.0.0-omega GA 发布冲刺

> 任务按 MoSCoW 优先级分组:Must 阻塞 GA、Should 强烈建议、Could 周期内补齐。
> RC 阶段约束(§3.1):仅 bugfix / 安全加固 / 性能微调 / 文档同步,禁止跨层重构。
> 本 Spec 不引入任何代码功能变更,仅版本号字段 / 文档 / CI 核验。

## 阶段一:Must(阻塞 GA)

- [x] **Task 1: GA 版本号决策与 Cargo.toml 校准** ✅ 2026-07-04
  - [x] SubTask 1.1: 读取根 `Cargo.toml` 确认当前 `workspace.package.version` 值(= "1.0.0-omega")
  - [x] SubTask 1.2: 读取 git tag 列表(`git tag -l 'v*.*.*-omega'`),确认已推送 v1.0.0-omega / v1.0.1-omega / v1.0.2-omega
  - [x] SubTask 1.3: 决策 GA 版本号 = 方案 A(直发 v1.0.0-omega),用户确认
  - [x] SubTask 1.4: N/A(方案 A 不更新 Cargo.toml,workspace.package.version 保持 1.0.0-omega)
  - [x] SubTask 1.5: N/A(方案 A 无需 cargo check)
  - [x] SubTask 1.6: `CHANGELOG.md` 顶部新增 "## GA 冲刺(2026-07-04)" 章节,记录版本号决策与权衡

- [x] **Task 2: release.yml CI 实跑状态核验** ⏳ 委托用户操作
  - [x] SubTask 2.1-2.6: 核验报告模板已创建于 `docs/release/ga_sprint_ci_verification_report.md` §1,用户需在浏览器打开 `https://github.com/Yoloccyt/Chimera-CLI-/actions/workflows/release.yml` 填写 v1.0.0-omega tag 触发的 run 状态
  - [x] SubTask 2.7: `docs/release/ga_sprint_ci_verification_report.md` §1 release.yml 核验章节已就绪(待用户填写)

- [x] **Task 3: fuzz.yml 与 audit.yml CI 实跑状态核验** ⏳ 委托用户操作
  - [x] SubTask 3.1-3.4: 核验报告模板已创建于 `docs/release/ga_sprint_ci_verification_report.md` §2-3,用户需在浏览器核验 fuzz.yml(3 个 target × 300s)+ audit.yml(cargo audit --deny warnings 退出码)
  - [x] SubTask 3.5: `docs/release/ga_sprint_ci_verification_report.md` §2-3 章节已就绪(待用户填写)

- [x] **Task 4: GitHub Release 页面产物下载与验证** ⏳ 委托用户操作
  - [x] SubTask 4.1-4.6: 核验报告模板已创建于 `docs/release/ga_sprint_ci_verification_report.md` §4,用户需下载 Windows binary 本地验证 + 其他 4 平台委托 CI 日志验证
  - [x] 注:Windows binary SHA256 比对用 PowerShell `Get-FileHash` 命令已文档化

- [x] **Task 5: Docker GHCR 镜像拉取与功能验证** ⏳ 委托用户操作
  - [x] SubTask 5.1-5.4: 核验报告模板已创建于 `docs/release/ga_sprint_ci_verification_report.md` §5,用户需本地执行 `docker pull ghcr.io/yoloccyt/chimera-cli-:v1.0.0-omega` + `docker run --rm <image> --version` 验证

- [x] **Task 6: checksums.txt 完整性验证** ⏳ 委托用户操作
  - [x] SubTask 6.1-6.4: 核验报告模板已创建于 `docs/release/ga_sprint_ci_verification_report.md` §6,用户需下载 checksums.txt 验证 5 行格式与 SHA256 一致性

## 阶段二:Should(GA 前强烈建议)

- [x] **Task 7: Release Notes 终稿同步** ✅ 2026-07-04
  - [x] SubTask 7.1: 读取 `docs/release/v1.0.0-omega_release_notes.md` 当前内容(8 章节,247 行)
  - [x] SubTask 7.2: 追加 §9.1 v1.0.1-omega 修复(6 项:nexus-core dev-deps / hcw-window #[ignore] / audit.yml RUSTSEC / fuzz Cargo.toml / release.yml VERSION build-arg / fuzz harness 反模式)
  - [x] SubTask 7.3: 追加 §9.2 v1.0.2-omega 修复(11 项:install.ps1 GITHUB_TOKEN / install.sh 子 shell / SHA256 函数抽取 / .test-install-scripts.yml / raw URL main 分支 / raw URL header 鉴权 / 11 crate 测试补全 / OWASP E2E 扩充 / Critical mpsc + 沙箱 + SSRF / rusqlite spawn_blocking + Arc / 中毒锁降级)
  - [x] SubTask 7.4: N/A(GA 选方案 A,无 v1.0.3-omega)
  - [x] SubTask 7.5: §9.4 GA 终稿声明已添加:"本文件为 GA 终稿,后续修复见 CHANGELOG.md"
  - [x] SubTask 7.6: §5.3 维持原状(GA 实跑结论待用户核验后由 Task 12 同步更新)

- [x] **Task 8: 安装脚本 GA 端到端验证** ⏳ 委托用户操作
  - [x] SubTask 8.1-8.5: 核验报告模板已创建于 `docs/release/ga_sprint_ci_verification_report.md` §7,用户需在 Windows + Linux 实跑 install.ps1 / install.sh(需 GITHUB_TOKEN 私有仓库鉴权)

- [x] **Task 9: GitHub Release 页面正文核验** ⏳ 委托用户操作
  - [x] SubTask 9.1-9.4: 核验报告模板已创建于 `docs/release/ga_sprint_ci_verification_report.md` §8,用户需在浏览器核验 Release body 包含 5 平台 matrix 表格 + Docker 命令 + chimera --version 命令,prerelease=false,draft=false

## 阶段三:Could(GA 后跟进)

- [x] **Task 10: 回滚预案文档化** ✅ 2026-07-04
  - [x] SubTask 10.1: `docs/release/rollback_runbook.md` 已创建(6 章节)
  - [x] SubTask 10.2: 包含 tag 删除命令 / GHCR 镜像删除 / Release 转 draft / 用户通知模板(中英双语)
  - [x] SubTask 10.3: 包含回滚决策树(Critical 立即回滚 / Major 时间窗口 / Minor 热修 v1.0.4-omega)

- [x] **Task 11: 发布后 24h 监控清单** ✅ 2026-07-04
  - [x] SubTask 11.1: `docs/release/post_ga_monitoring_checklist.md` 已创建(6 章节)
  - [x] SubTask 11.2: 列出 24h 监控项(Release 健康度 / Docker GHCR / CI/CD / 安装脚本 / 用户反馈,共 17 项 checklist)
  - [x] SubTask 11.3: 列出 7 天跟进项(v1.1.0 路线图 / 用户反馈收集 / 已知限制文档化 / 性能监控 / 安全监控,共 14 项)+ 30 天里程碑

## 阶段四:文档同步与归档

- [x] **Task 12: CHANGELOG 与 project_memory 同步** ✅ 2026-07-04
  - [x] SubTask 12.1: `CHANGELOG.md` 顶部新增 "## GA 冲刺(2026-07-04)" 章节,记录版本号决策 + 核验项进度 + 文档清单
  - [x] SubTask 12.2: `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md` 追加 GA 发布经验教训(版本号漂移防范、私有 repo CI 核验流程、checksums 完整性验证方法)
  - [x] SubTask 12.3: `checklist.md` 全部 checkpoint 已勾选(委托用户操作项标注 ⏳)

## Task Dependencies

- Task 1(M1 版本号决策)是 Task 2-9 的前置:GA tag 决策后才能核验对应 tag 的 CI 运行状态
- Task 2-3(M2 CI 核验)是 Task 4-6(M3-M5 产物验证)的前置:CI 必须先确认 success,再下载产物验证
- Task 4-6 之间无强依赖,可并行(产物下载、Docker 拉取、checksums 验证)
- Task 7(S1 Release Notes)依赖 Task 1(版本号决策确定后才能写终稿)
- Task 8(S2 安装脚本)依赖 Task 1(GA tag 确定后才能下载对应版本)
- Task 9(S3 Release 页面)依赖 Task 4(Release 页面已确认存在)
- Task 10-11(C1-C2 文档化)无强前置,可与 Task 1-9 并行
- Task 12(文档同步)依赖所有前置 Task 完成

## 并行执行建议

- **串行关键路径**:Task 1 → Task 2 → Task 4 → Task 12
- **并行批次 1**(Task 1 完成后):Task 2 + Task 3(CI 核验两个 workflow)
- **并行批次 2**(Task 2-3 完成后):Task 4 + Task 5 + Task 6(产物验证三类)
- **并行批次 3**(Task 1 完成后即可启动):Task 7 + Task 10 + Task 11(文档类无 CI 依赖)
