# Week 8 限制深度攻坚 — 任务分解

> **参照 spec**:`spec.md`
> **团队**:E1 首席架构师 / E3 DevOps 工程师 / E2 测试工程师 / E5 文档工程师
> **总工时**:4.0 人日
> **执行策略**:阶段 1 并行(E1 + E3),阶段 2 串行(E3 触发 CI + E2 验证),阶段 3 串行(E5 收尾)

---

## Task 1:clippy procdump 根因分析(E1,Must)

**目标**:捕获 clippy-driver.exe 崩溃 dump,定位根因,生成分析报告 + 上游 issue 草稿
**Owner**:E1 首席架构师
**优先级**:Must
**工时**:1.5 人日

### SubTask 1.1:安装 procdump
- [ ] 下载 procdump(Sysinternals 官方,~2MB)
- [ ] 验证 `procdump.exe -?` 可执行
- [ ] 记录安装路径到 `docs/dev/clippy_root_cause_analysis.md` 环境章节

### SubTask 1.2:捕获 clippy-driver 崩溃 dump
- [ ] 设置环境变量(RUST_MIN_STACK=33554432 + CARGO_INCREMENTAL=0)
- [ ] 启动 procdump 监控 clippy-driver.exe:`procdump -ma -e 1 -f "" clippy-driver.exe D:\Chimera CLI\tmp\clippy_dumps\`
- [ ] 运行 `cargo clippy --workspace --all-targets`(默认 jobs,触发崩溃)
- [ ] 验证 `.dmp` 文件生成
- [ ] 如果 procdutt 无法捕获(权限/时序),降级为 Windows 错误报告 + 事件查看器日志

### SubTask 1.3:分析 dump 调用栈
- [ ] 使用 WinDbg(或 cdb.exe)加载 dump 文件
- [ ] 执行 `!analyze -v` 自动分析
- [ ] 提取调用栈、崩溃模块、异常代码
- [ ] 区分根因:`/GS` 检查 / `__fastfail` / 堆损坏 / 其他
- [ ] 记录分析结果

### SubTask 1.4:生成根因分析报告
- [ ] 创建 `docs/dev/clippy_root_cause_analysis.md`
- [ ] 章节:1.环境 2.现象 3.捕获过程 4.dump 分析 5.调用栈 6.根因结论 7.建议
- [ ] 包含 3 组对比实验数据(--jobs 1/2/默认)
- [ ] 包含 RUST_MIN_STACK 无效证据

### SubTask 1.5:撰写上游 issue 草稿
- [ ] 创建 `docs/dev/upstream_clippy_issue_draft.md`
- [ ] 章节:1.Title 2.Environment 3.Reproduction 4.Expected 5.Actual 6.Evidence(dump+实验) 7.Workaround 8.Root Cause Analysis
- [ ] 格式符合 rust-lang/rust-clippy issue 模板
- [ ] 附 dump 文件路径(供上游附加上传)

### SubTask 1.6:更新 release_notes
- [ ] 更新 `docs/release/v1.0.0-omega_release_notes.md` §6.1 clippy 章节
- [ ] 状态从"workaround 改进"升级为"根因分析完成 + 上游 issue 草稿就绪"
- [ ] 补充 dump 分析结论

---

## Task 2:编写 fuzz.yml + release.yml Docker job(E3,Should)

**目标**:新增 fuzz CI workflow + 补充 release.yml Docker job(不触发 CI)
**Owner**:E3 DevOps 工程师
**优先级**:Should
**工时**:1.0 人日

### SubTask 2.1:创建 fuzz.yml
- [ ] 创建 `.github/workflows/fuzz.yml`
- [ ] 触发条件:`workflow_dispatch` + `push tags: v*.*.*-omega`(与 release.yml 一致)
- [ ] ubuntu-latest runner
- [ ] 步骤:checkout → 安装 nightly + llvm-tools-preview → 安装 cargo-fuzz → cd fuzz/ → 运行 3 个 target 各 300s
- [ ] 失败处理:任一 target panic 则 job 失败
- [ ] artifact 上传:fuzz 日志 + 任何 crash 输入

### SubTask 2.2:补充 release.yml Docker job
- [ ] 读取现有 `.github/workflows/release.yml`
- [ ] 新增 `docker` job(needs: build)
- [ ] 步骤:checkout → docker/setup-buildx-action → docker/login-action(GHCR) → docker/build-push-action(build + push)
- [ ] 镜像 tag:`ghcr.io/${{ github.repository }}:${{ github.ref_name }}` + `latest`
- [ ] 验证 Dockerfile 路径正确(根目录)
- [ ] 确保 `permissions: packages: write`(GHCR 推送权限)

### SubTask 2.3:验证 workflow 语法
- [ ] 使用 `yamllint`(或在线 YAML 验证器)检查 fuzz.yml 语法
- [ ] 检查 release.yml 修改后语法正确
- [ ] 验证 workflow 间无冲突(fuzz.yml 与 release.yml 触发条件可区分)

### SubTask 2.4:更新发布指南
- [ ] 更新 `docs/release/release_guide.md`
- [ ] 补充 fuzz workflow 说明(触发方式 + 运行时间 + 结果查看)
- [ ] 补充 Docker job 说明(GHCR 镜像地址 + pull 命令)

---

## Task 3:git push + 触发 CI + 验证产物(E3+E2,Should)

**目标**:推送 tag 触发 CI,验证 5 平台 binary + Docker 镜像 + fuzz 结果
**Owner**:E3 DevOps 工程师(触发)+ E2 测试工程师(验证)
**优先级**:Should
**工时**:1.0 人日

### SubTask 3.1:提交 workflow 变更
- [ ] git add `.github/workflows/fuzz.yml`
- [ ] git add `.github/workflows/release.yml`(修改后)
- [ ] git add `docs/dev/clippy_root_cause_analysis.md`(Task 1 产出)
- [ ] git add `docs/dev/upstream_clippy_issue_draft.md`(Task 1 产出)
- [ ] git add `docs/release/release_guide.md`(Task 2 修改)
- [ ] git commit(消息:"ci: add fuzz workflow + docker job + clippy root cause analysis")
- [ ] 不推送 v1.0.0-omega tag(已存在),改用 v1.0.1-omega

### SubTask 3.2:配置远程仓库 + 推送
- [ ] 检查 `git remote -v` 是否有远程仓库
- [ ] 如果无远程,询问用户 GitHub 仓库 URL 并添加
- [ ] git push origin main(或 master)
- [ ] 创建并推送 annotated tag `v1.0.1-omega`:`git tag -a v1.0.1-omega -m "..."` + `git push origin v1.0.1-omega`

### SubTask 3.3:监控 release.yml CI 运行
- [ ] 在 GitHub Actions 页面监控 release workflow
- [ ] 等待 5 平台 build job 完成(预计 10-20min)
- [ ] 等待 docker job 完成(预计 5-10min)
- [ ] 等待 release job 完成(创建 GitHub Release)
- [ ] 记录每个 job 的状态/耗时/日志关键信息

### SubTask 3.4:监控 fuzz.yml CI 运行
- [ ] 在 GitHub Actions 页面监控 fuzz workflow
- [ ] 等待 3 个 target 运行完成(各 300s + 编译,预计 15-20min)
- [ ] 记录每个 target 的执行次数/覆盖率/panic 信息
- [ ] 如果 panic,下载 crash 输入并分析

### SubTask 3.5:验证产物(E2)
- [ ] 验证 GitHub Release v1.0.1-omega 包含 5 平台 binary
- [ ] 验证 Docker 镜像可 `docker pull ghcr.io/<org>/chimera:v1.0.1-omega`(或至少 GHCR 上存在)
- [ ] 验证 fuzz workflow 3 个 target 无 panic(或 panic 已分析)
- [ ] 生成 `docs/acceptance/week8_deep_remediation_ci_report.md`

### SubTask 3.6:更新安全报告
- [ ] 更新 `docs/security/week8_security_report.md` §3.5
- [ ] 补充 Linux CI 实际运行结果(执行次数/覆盖率/panic)
- [ ] 状态从"部分解除"升级为"完全解除"

---

## Task 4:文档同步 + checklist 更新(E5,收尾)

**目标**:同步所有文档,更新 checklist,生成验收报告
**Owner**:E5 文档工程师
**优先级**:Should
**工时**:0.5 人日

### SubTask 4.1:生成验收报告
- [ ] 创建 `docs/acceptance/week8_limitations_deep_remediation_report.md`
- [ ] 章节:1.执行摘要 2.限制 5 根因分析 3.限制 1 CI 集成 4.限制 2+3 CI 触发 5.质量验收 6.遗留问题 7.结论

### SubTask 4.2:更新 CHANGELOG
- [ ] 在 `CHANGELOG.md` Week 8 章节追加"深度攻坚"子章节
- [ ] 包含:clippy 根因分析 + 上游 issue 草稿 + fuzz CI + Docker job + CI 实际触发

### SubTask 4.3:更新 project_memory
- [ ] 在 `project_memory.md` Lessons Learned 追加 3 条经验:
  - procdump 捕获 clippy-driver 崩溃 dump 方法论
  - libFuzzer Linux CI 集成模式
  - GitHub Actions Docker job + GHCR 推送配置

### SubTask 4.4:更新 release_notes
- [ ] 更新 `docs/release/v1.0.0-omega_release_notes.md` §6 已知限制表格
- [ ] 限制 1:部分解除 → 完全解除(CI 运行通过)
- [ ] 限制 5:workaround 改进 → 根因分析完成 + 上游 issue 草稿
- [ ] 限制 2+3:委托 CI → CI 实际触发通过

### SubTask 4.5:更新 checklist
- [ ] 更新 `.trae/specs/week8-limitations-deep-remediation/checklist.md`
- [ ] 所有检查点标记 [x]
- [ ] 全局门槛 G1-G6 标记 [x]

---

## 甘特图

```
Day 1:
  09:00-12:00  E1: Task 1.1-1.3(procdump 安装 + 捕获 dump + 分析)  ─┐
  09:00-12:00  E3: Task 2.1-2.3(fuzz.yml + Docker job + 语法验证) ─┤ 并行
  13:00-15:00  E1: Task 1.4-1.6(报告 + issue 草稿 + release_notes) ─┘
  15:00-17:00  E3: Task 3.1-3.2(提交 + push + 触发 CI)

Day 2:
  09:00-12:00  E3: Task 3.3-3.4(监控 CI 运行)  ─┐
  09:00-12:00  E2: 等待 CI 完成                  ─┤ 串行依赖
  13:00-15:00  E2: Task 3.5-3.6(验证产物 + 更新安全报告) ─┘
  15:00-17:00  E5: Task 4.1-4.5(文档同步 + checklist)
```

## 资源清单

| 专家 | 任务 | 工时 | 关键产出 |
|------|------|------|---------|
| E1 首席架构师 | Task 1 | 1.5 人日 | clippy_root_cause_analysis.md + upstream_clippy_issue_draft.md |
| E3 DevOps 工程师 | Task 2 + Task 3.1-3.4 | 1.5 人日 | fuzz.yml + release.yml Docker job + CI 触发 |
| E2 测试工程师 | Task 3.5-3.6 | 0.5 人日 | CI 验证报告 + 安全报告更新 |
| E5 文档工程师 | Task 4 | 0.5 人日 | 5 份文档同步 |
| **合计** | — | **4.0 人日** | — |

## 并行化机会

- **阶段 1(并行)**:Task 1(E1 clippy 根因)+ Task 2(E3 workflow 编写)
- **阶段 2(串行)**:Task 3(E3 触发 CI → E2 验证)
- **阶段 3(串行)**:Task 4(E5 文档同步)

## 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| procdump 捕获失败(权限/时序) | 中 | 中 | 降级为事件查看器日志 + 已有实验数据 |
| GitHub 远程仓库未配置 | 低 | 高 | 提前询问用户 URL |
| CI 运行超时(>30min) | 中 | 中 | 分 job 并行,retention 30 天 |
| fuzz target 在 Linux 触发 panic | 低 | 高 | 修复或记录为已知 bug |
| Docker job 推送 GHCR 失败(权限) | 中 | 中 | 检查 `packages: write` 权限 |
