# Week 8 限制深度攻坚 — 验收检查清单

> **参照 spec**:`spec.md`
> **参照 tasks**:`tasks.md`
> **验收原则**:每项检查点必须 [x] 已完成,平台限制项加注说明

---

## Task 1:clippy procdump 根因分析(E1,Must)

### SubTask 1.1:安装 procdump
- [ ] 1.1 procdump 已下载并验证可执行
- [ ] 1.2 安装路径已记录

### SubTask 1.2:捕获 clippy-driver 崩溃 dump
- [ ] 1.3 procdump 监控命令已执行
- [ ] 1.4 clippy 默认 jobs 触发崩溃
- [ ] 1.5 `.dmp` 文件已生成(或降级方案已记录)

### SubTask 1.3:分析 dump 调用栈
- [ ] 1.6 dump 文件已加载到 WinDbg/cdb
- [ ] 1.7 `!analyze -v` 已执行
- [ ] 1.8 调用栈已提取
- [ ] 1.9 根因已区分(/GS / __fastfail / 堆损坏 / 其他)

### SubTask 1.4:根因分析报告
- [ ] 1.10 `docs/dev/clippy_root_cause_analysis.md` 已创建
- [ ] 1.11 报告包含 7 章节(环境/现象/捕获/分析/调用栈/结论/建议)
- [ ] 1.12 报告包含 3 组对比实验数据
- [ ] 1.13 报告包含 RUST_MIN_STACK 无效证据

### SubTask 1.5:上游 issue 草稿
- [ ] 1.14 `docs/dev/upstream_clippy_issue_draft.md` 已创建
- [ ] 1.15 草稿包含 8 章节(Title/Environment/Reproduction/Expected/Actual/Evidence/Workaround/RootCause)
- [ ] 1.16 草稿符合 rust-lang/rust-clippy issue 模板

### SubTask 1.6:更新 release_notes
- [ ] 1.17 `docs/release/v1.0.0-omega_release_notes.md` §6.1 已更新
- [ ] 1.18 clippy 状态升级为"根因分析完成 + 上游 issue 草稿就绪"

---

## Task 2:编写 fuzz.yml + release.yml Docker job(E3,Should)

### SubTask 2.1:创建 fuzz.yml
- [ ] 2.1 `.github/workflows/fuzz.yml` 已创建
- [ ] 2.2 触发条件正确(workflow_dispatch + push tags)
- [ ] 2.3 ubuntu-latest runner
- [ ] 2.4 nightly + llvm-tools-preview 安装步骤
- [ ] 2.5 cargo-fuzz 安装步骤
- [ ] 2.6 3 个 target 各 300s 运行步骤
- [ ] 2.7 失败处理(panic 则 job 失败)
- [ ] 2.8 artifact 上传(fuzz 日志 + crash 输入)

### SubTask 2.2:补充 release.yml Docker job
- [ ] 2.9 release.yml 已新增 `docker` job
- [ ] 2.10 docker job `needs: build`
- [ ] 2.11 docker/setup-buildx-action 步骤
- [ ] 2.12 docker/login-action(GHCR)步骤
- [ ] 2.13 docker/build-push-action(build + push)步骤
- [ ] 2.14 镜像 tag 正确(`ghcr.io/${{ github.repository }}:${{ github.ref_name }}` + latest)
- [ ] 2.15 `permissions: packages: write` 已添加

### SubTask 2.3:验证 workflow 语法
- [ ] 2.16 fuzz.yml YAML 语法正确
- [ ] 2.17 release.yml 修改后语法正确
- [ ] 2.18 workflow 间无触发冲突

### SubTask 2.4:更新发布指南
- [ ] 2.19 `docs/release/release_guide.md` 已更新
- [ ] 2.20 fuzz workflow 说明已补充
- [ ] 2.21 Docker job 说明已补充

---

## Task 3:git push + 触发 CI + 验证产物(E3+E2,Should)

### SubTask 3.1:提交 workflow 变更
- [ ] 3.1 git add 所有新增/修改文件
- [ ] 3.2 git commit 成功
- [ ] 3.3 commit 消息符合规范

### SubTask 3.2:配置远程仓库 + 推送
- [ ] 3.4 git remote 已配置(或用户已提供 URL)
- [ ] 3.5 git push origin main 成功
- [ ] 3.6 annotated tag v1.0.1-omega 已创建
- [ ] 3.7 git push origin v1.0.1-omega 成功

### SubTask 3.3:监控 release.yml CI
- [ ] 3.8 release workflow 已触发
- [ ] 3.9 5 平台 build job 全部成功
- [ ] 3.10 docker job 构建成功
- [ ] 3.11 release job 创建 GitHub Release 成功
- [ ] 3.12 CI 运行日志已记录

### SubTask 3.4:监控 fuzz.yml CI
- [ ] 3.13 fuzz workflow 已触发(或手动触发)
- [ ] 3.14 3 个 target 编译成功
- [ ] 3.15 3 个 target 各运行 300s
- [ ] 3.16 panic 信息已记录(如有)
- [ ] 3.17 fuzz 日志已归档

### SubTask 3.5:验证产物(E2)
- [ ] 3.18 GitHub Release v1.0.1-omega 包含 5 平台 binary
- [ ] 3.19 Docker 镜像在 GHCR 可用(或至少构建成功)
- [ ] 3.20 fuzz workflow 3 target 无 panic(或 panic 已分析)
- [ ] 3.21 `docs/acceptance/week8_deep_remediation_ci_report.md` 已创建

### SubTask 3.6:更新安全报告
- [ ] 3.22 `docs/security/week8_security_report.md` §3.5 已更新
- [ ] 3.23 Linux CI 实际运行结果已补充
- [ ] 3.24 限制 1 状态升级为"完全解除"

---

## Task 4:文档同步 + checklist 更新(E5,收尾)

### SubTask 4.1:验收报告
- [ ] 4.1 `docs/acceptance/week8_limitations_deep_remediation_report.md` 已创建
- [ ] 4.2 报告包含 7 章节
- [ ] 4.3 3 个限制最终状态已明确

### SubTask 4.2:CHANGELOG
- [ ] 4.4 `CHANGELOG.md` 已追加"深度攻坚"子章节
- [ ] 4.5 包含 clippy 根因 + fuzz CI + Docker job + CI 触发

### SubTask 4.3:project_memory
- [ ] 4.6 `project_memory.md` 已追加 3 条经验教训
- [ ] 4.7 经验教训涵盖 procdump/libFuzzer CI/Docker GHCR

### SubTask 4.4:release_notes
- [ ] 4.8 release_notes §6 已知限制表格已更新
- [ ] 4.9 限制 1 升级为"完全解除"
- [ ] 4.10 限制 5 升级为"根因分析完成 + 上游 issue 草稿"
- [ ] 4.11 限制 2+3 升级为"CI 实际触发通过"

### SubTask 4.5:checklist
- [ ] 4.12 本 checklist 所有检查点已 [x]
- [ ] 4.13 全局门槛 G1-G6 已 [x]

---

## 全局门槛(G1-G6)

- [ ] **G1**:clippy 根因分析报告包含 dump 调用栈 + 根因结论
- [ ] **G2**:上游 issue 草稿完整(8 章节 + 证据)
- [ ] **G3**:fuzz.yml 在 Linux CI 运行 3 target 各 300s 无 panic
- [ ] **G4**:release.yml Docker job 构建镜像 < 100MB 并推送 GHCR
- [ ] **G5**:CI 实际触发,5 平台 binary + Docker 镜像验证通过
- [ ] **G6**:5 份文档同步(验收报告/CHANGELOG/project_memory/release_notes/checklist)

---

## 验收结论

- Must 项(Task 1 clippy 根因):[ ] 通过
- Should 项(Task 2+3 CI 集成与触发):[ ] 通过
- 文档同步(Task 4):[ ] 通过
- **最终验收**:[ ] 通过
