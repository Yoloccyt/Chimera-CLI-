# Tasks — Week 8 已知限制修复(week8-limitations-remediation)

> 5 个 Task,5 名专家子代理,3.0 人日总工时。Task 1/2/3 可并行(独立子代理),Task 4 串行(同一 DevOps),Task 5 收尾。

## Task 依赖关系

- Task 1(stress_test)→ Task 5(文档同步)
- Task 2(cargo-fuzz)→ Task 5(文档同步)
- Task 3(clippy 根因)→ Task 5(文档同步)
- Task 4(CI + Docker 验证)→ Task 5(文档同步)
- Task 1/2/3 可并行(独立限制,无依赖)
- Task 4 串行(CI + Docker 同一 DevOps)

---

## Task 1:stress_test 1000 次压测运行验证(Must,限制 4)

**负责人**:E1 首席架构师 / **参与者**:E4 性能工程师 / **预计工时**:1.0 人日

- [ ] SubTask 1.1:运行 stress_test — 执行 `cargo test --test stress_test -- --ignored --nocapture`,记录完整输出
  - 验证:命令 exit 0,1000 次迭代全部成功
- [ ] SubTask 1.2:验证 6 项断言 — 1000 次成功 / Wiki 3000 条 / WikiStore 持久化 / 延迟退化 < 50% / 最大单次 < 2s / p95 统计
  - 验证:6 项断言全部通过
- [ ] SubTask 1.3:若压测失败,分析根因 — 检查 leak_probe strong_count / 延迟退化 / WikiStore 计数
  - 验证:根因分析报告(若失败)
- [ ] SubTask 1.4:记录压测结果至 `docs/performance/week8_stress_test_report.md`(新建)
  - 验证:报告含 p50/p95/p99 延迟 + 首次/末次对比 + 1000 次成功统计

## Task 2:cargo-fuzz 3 target 实际运行(Should,限制 1)

**负责人**:E2 资深测试工程师 / **参与者**:E1 首席架构师 / **预计工时**:1.0 人日

- [ ] SubTask 2.1:安装 nightly 工具链 — `rustup install nightly-x86_64-pc-windows-gnu` + `rustup component add llvm-tools-preview --toolchain nightly`
  - 验证:`rustup toolchain list` 包含 nightly-x86_64-pc-windows-gnu
- [ ] SubTask 2.2:安装 cargo-fuzz — `cargo +nightly install cargo-fuzz`
  - 验证:`cargo +nightly fuzz --help` 输出 usage
- [ ] SubTask 2.3:运行 quest_parse target — `cargo +nightly fuzz run quest_parse -- -max_total_time=60`
  - 验证:60s 运行无 panic(或发现 panic 并记录)
- [ ] SubTask 2.4:运行 seccore_sandbox target — `cargo +nightly fuzz run seccore_sandbox -- -max_total_time=60`
  - 验证:60s 运行无 panic(或发现 panic 并记录)
- [ ] SubTask 2.5:运行 event_serialize target — `cargo +nightly fuzz run event_serialize -- -max_total_time=60`
  - 验证:60s 运行无 panic(或发现 panic 并记录)
- [ ] SubTask 2.6:若发现 panic,分析根因 — 检查被测代码 / fuzz target / 输入语料
  - 验证:根因分析报告 + 修复(若适用)
- [ ] SubTask 2.7:更新 `docs/security/week8_security_report.md` 补充 fuzz 实际运行结果
  - 验证:报告含 3 target 运行时间 + 覆盖输入数 + panic 状态

## Task 3:clippy 栈溢出根因分析(Should,限制 5)

**负责人**:E4 性能工程师 / **参与者**:E1 首席架构师 / **预计工时**:0.5 人日

- [ ] SubTask 3.1:RUST_MIN_STACK 实验 — 运行 `$env:RUST_MIN_STACK=33554432; cargo clippy --workspace --all-targets -- -D warnings`(32MB 栈,默认并行)
  - 验证:clippy 正常完成无栈溢出(或仍栈溢出)
- [ ] SubTask 3.2:对比 `--jobs 1`(workaround)vs `--jobs 2` vs 默认并行
  - 验证:三种配置的 clippy 行为对比表
- [ ] SubTask 3.3:若 RUST_MIN_STACK 解决,验证 clippy 零警告 — `RUST_MIN_STACK=33554432 cargo clippy --workspace --all-targets -- -D warnings` exit 0
  - 验证:exit 0 且零警告
- [ ] SubTask 3.4:更新文档记录根因分析结论
  - 验证:`docs/acceptance/week8_final_acceptance_report.md` §9.1 clippy 章节更新

## Task 4:CI workflow + Dockerfile 静态验证(Could,限制 2/3)

**负责人**:E3 DevOps 工程师 / **参与者**:E1 首席架构师 / **预计工时**:0.5 人日

- [ ] SubTask 4.1:静态验证 `.github/workflows/release.yml` — 检查 5 平台 matrix / cross / strip / upload / release 逻辑
  - 验证:YAML 语法正确,5 平台配置完整,逻辑无遗漏
- [ ] SubTask 4.2:静态验证 `Dockerfile` — 检查多阶段构建 / base image / COPY / ENTRYPOINT / 体积估算
  - 验证:Dockerfile 语法正确,多阶段构建逻辑正确,体积估算 < 100MB
- [ ] SubTask 4.3:尝试安装 zig(条件性)— 若可下载,运行 `cargo zigbuild --release --target x86_64-unknown-linux-gnu -p chimera-cli`
  - 验证:若 zig 可用,Linux x86_64 binary 生成;若不可用,标注为"需 CI 环境"
- [ ] SubTask 4.4:更新 `docs/release/week8_release_guide.md` 补充 CI/Docker 验证状态
  - 验证:指南含 CI workflow 验证结论 + Dockerfile 验证结论 + zig 状态

## Task 5:文档同步 + checklist 更新(收尾)

**负责人**:E5 文档工程师 / **参与者**:E1 首席架构师 / **预计工时**:0.5 人日

- [ ] SubTask 5.1:更新 `docs/acceptance/week8_final_acceptance_report.md` §9 已知限制清单 — 标注每项限制的修复状态
  - 验证:5 项限制状态更新(已修复 / 需 CI 环境 / 需 Docker 环境)
- [ ] SubTask 5.2:更新 `CHANGELOG.md` 新增"Week 8 已知限制修复"小节
  - 验证:小节含 5 项限制修复结论
- [ ] SubTask 5.3:更新 `project_memory.md` 新增修复经验教训
  - 验证:经验教训条目(如 nightly 安装 / fuzz panic 分析 / clippy 栈溢出根因)
- [ ] SubTask 5.4:更新 `.trae/specs/week8-limitations-remediation/checklist.md` 全部勾选
  - 验证:checklist 所有项 ✅
- [ ] SubTask 5.5:更新 `docs/release/v1.0.0-omega_release_notes.md` §7 已知限制章节
  - 验证:已知限制章节反映最新修复状态

---

# Task Dependencies

- [Task 1] → [Task 5](压测结果需记录到文档)
- [Task 2] → [Task 5](fuzz 结果需记录到文档)
- [Task 3] → [Task 5](clippy 结论需记录到文档)
- [Task 4] → [Task 5](CI/Docker 验证结论需记录到文档)
- [Task 1/2/3] 可并行(独立限制,无依赖)
- [Task 4] 串行(CI + Docker 同一 DevOps,顺序执行)

# 并行化机会

- 阶段 1(并行):Task 1(stress_test)+ Task 2(cargo-fuzz)+ Task 3(clippy 根因)三路并行
- 阶段 2(串行):Task 4(CI + Docker 验证)
- 阶段 3(串行):Task 5(文档同步,依赖前 4 个 Task)

# 时间计划(甘特图)

```
Day 1 上午:Task 1(stress_test)  | Task 2(cargo-fuzz) | Task 3(clippy 根因)
Day 1 下午:Task 1 收尾            | Task 2 收尾         | Task 3 收尾
Day 2 上午:Task 4(CI + Docker 验证)
Day 2 下午:Task 5(文档同步 + checklist 更新)
```

# 资源清单

| 工具 | 用途 | 权限 |
|------|------|------|
| RunCommand | 执行 cargo / rustup / git 命令 | 读/写 |
| Read / Edit / Write | 读取 / 修改源码与文档 | 读/写 |
| Grep / Glob | 代码搜索 | 读 |
| Task(子代理) | 并行调度专家子代理 | 管理 |
| MCP 工具 | 如需(kubernetes / context7) | 读 |
| Skill: superpowers-main | 深度思考 | 读 |
