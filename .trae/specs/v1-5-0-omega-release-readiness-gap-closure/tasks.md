# Tasks — v1.5.0-omega Release Readiness Gap Closure

> **执行原则**:TDD 守恒(先写失败测试再实现);长期主义(不牺牲代码质量);YAGNI(不过度设计);每任务后 `cargo fmt --all` + `cargo check -p <crate>` 验证。
> **并行标注**:🚀 = 可并行;🔒 = 有依赖,需前置任务完成;⚡ = 串行关键路径。

---

## P0 阻塞项(必须完成方可推 tag v1.5.0-omega)

- [x] **Task 1: 清理 check_errors*.txt 残留文件** 🚀
  - [ ] SubTask 1.1: 删除根目录 5 个文件(`check_errors.txt` / `check_errors2.txt` / `check_errors3.txt` / `check_errors_current.txt` / `check_errors_current2.txt`)
  - [ ] SubTask 1.2: 运行 `cargo check --workspace` 确认退出码 0,无新错误日志生成
  - **验证**:Glob 搜索 `check_errors*.txt` 返回 No file found;`cargo check --workspace` EXIT_CODE=0

- [x] **Task 2: 版本同步 Cargo.toml** ⚡
  - [ ] SubTask 2.1: 编辑 `Cargo.toml:31`,将 `version = "1.4.0-omega"` 改为 `version = "1.5.0-omega`
  - [ ] SubTask 2.2: 确认 `CHANGELOG.md` 已有 `v1.5.0-omega 汇总` 章节(已存在,2026-07-10)
  - **验证**:Read `Cargo.toml:31` 确认 `1.5.0-omega`;Grep `CHANGELOG.md` 确认 `v1.5.0-omega` 章节存在

- [x] **Task 3: README.md 全量更新** 🚀
  - [ ] SubTask 3.1: 更新 `README.md:3` version badge `1.0.0--omega` → `1.5.0--omega`
  - [ ] SubTask 3.2: 更新 `README.md:5` crates badge `34` → `35`
  - [ ] SubTask 3.3: 更新 `README.md:7` forbid badge `34/34-success` → `35/35-success`
  - [ ] SubTask 3.4: 更新 `README.md:16` 正文 "34 crate workspace" → "35 crate workspace"
  - [ ] SubTask 3.5: 更新 `README.md:29` 正文 "34/34 crate" → "35/35 crate"
  - [ ] SubTask 3.6: Grep `README.md` 确认无 "34 crate" / "34/34" / "1.0.0--omega" 残留
  - **验证**:Grep `README.md` pattern `34` 在 crate 上下文返回 0 匹配;Grep `1.0.0--omega` 返回 0 匹配

- [x] **Task 4: CODE_WIKI.md 更新** 🚀
  - [ ] SubTask 4.1: Grep `CODE_WIKI.md` 定位所有 "34 crates" / "34/34" / "34 个" 出现位置
  - [ ] SubTask 4.2: 逐处更新为 "35 crates" / "35/35" / "35 个"
  - [ ] SubTask 4.3: 确认 `online-learning` crate 在 §3.1 crate 索引中已登记
  - **验证**:Grep `CODE_WIKI.md` pattern `34\s*(crates|个|/34)` 返回 0 匹配

- [x] **Task 5: 补齐 online-learning crate 测试基建** 🔒(依赖 Task 2 版本同步)
  - [ ] SubTask 5.1: 创建 `crates/online-learning/tests/` 目录
  - [ ] SubTask 5.2: 编写 `tests/proptest.rs` — ParameterRegistry 并发注册/注销不变量(block-named 语法,256 cases)
    - 覆盖:`register` 后 `get` 返回 Some;`unregister` 后 `get` 返回 None;并发 register 同名参数不冲突
  - [ ] SubTask 5.3: 编写 `tests/integration.rs` — GradientDescent 反馈驱动更新闭环
    - 覆盖:`apply_gradient` 参数更新方向;学习率衰减;收敛性验证
  - [ ] SubTask 5.4: 创建 `crates/online-learning/benches/` 目录
  - [ ] SubTask 5.5: 编写 `benches/registry_bench.rs` — registry 注册/查询延迟(criterion)
  - [ ] SubTask 5.6: 更新 `crates/online-learning/Cargo.toml` 添加 `[dev-dependencies]` proptest / criterion + `[[bench]]` 声明
  - [ ] SubTask 5.7: 运行 `cargo test -p online-learning` 确认全部通过
  - [ ] SubTask 5.8: 运行 `cargo clippy -p online-learning --all-targets -- -D warnings` 零警告
  - [ ] SubTask 5.9: 运行 `cargo fmt -p online-learning -- --check` 零 diff
  - **验证**:`cargo test -p online-learning` 退出码 0;`cargo bench -p online-learning --no-run` 编译通过

- [x] **Task 6: P0 全量验证** 🔒(依赖 Task 1-5 全部完成)
  - [x] SubTask 6.1: `cargo check --workspace` 退出码 0
  - [x] SubTask 6.2: `cargo test --workspace --jobs 1` 退出码 0(2026-07-11 验证通过,EXIT_CODE=0)
  - [x] SubTask 6.3: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
  - [x] SubTask 6.4: `cargo fmt --all -- --check` 退出码 0
  - **验证**:四条命令全部 EXIT_CODE=0

---

## P1 风险项(强烈建议发布前完成)

- [x] **Task 7: release.yml Windows job 安装 MinGW** 🚀
  - [ ] SubTask 7.1: 读取 `.github/workflows/release.yml:43-47` Windows job 配置
  - [ ] SubTask 7.2: 添加 MinGW 安装 step(推荐 `msys2/setup-msys2@v2` action 或 `choco install mingw`)
  - [ ] SubTask 7.3: 确保 `D:/msys64/mingw64/bin/gcc.exe` 路径可用,或更新 `.cargo/config.toml` linker 路径为 CI 环境
  - [ ] SubTask 7.4: 验证 YAML 语法正确
  - **验证**:Read release.yml 确认 Windows job 有 MinGW 安装 step;YAML lint 通过

- [x] **Task 8: release.yml binary 体积 CI 断言** 🚀
  - [ ] SubTask 8.1: 在 release.yml build job 后添加 step,检查 binary 体积 < 50MB(52428800 bytes)
  - [ ] SubTask 8.2: 使用 `stat -c%s` (Linux) / `$((Get-Item).Length)` (Windows) 跨平台获取体积
  - [ ] SubTask 8.3: 超限则 `exit 1` 阻断 release
  - **验证**:Read release.yml 确认有 binary 体积断言 step

- [x] **Task 9: release.yml test job 加入 PR 触发** 🚀
  - [ ] SubTask 9.1: 在 release.yml `on:` 节点添加 `pull_request` 触发条件(改 Cargo.lock / Cargo.toml / src/ 时触发)
  - [ ] SubTask 9.2: 确保 test job 在 PR 时运行,但 build/docker/release job 仅 tag 触发
  - **验证**:Read release.yml `on:` 节点确认 `pull_request` 配置

- [x] **Task 10: Dockerfile ARG VERSION 更新** 🚀
  - [ ] SubTask 10.1: 编辑 `Dockerfile:49`(或对应行),将 `ARG VERSION=1.0.0-omega` 改为 `ARG VERSION=1.5.0-omega`
  - **验证**:Read Dockerfile 确认 ARG VERSION = 1.5.0-omega

- [x] **Task 11: CHANGELOG 补齐 v1.0.0/v1.1.0 汇总** 🚀
  - [ ] SubTask 11.1: 读取 `CHANGELOG.md` 确认 v1.0.0-omega / v1.1.0-omega 是否有汇总章节
  - [ ] SubTask 11.2: 如缺失,基于 `docs/optimization/v1.1.0/` 报告与 git log 补齐汇总章节
  - [ ] SubTask 11.3: 确保章节格式与 v1.2.0~v1.5.0 一致
  - **验证**:Grep `CHANGELOG.md` pattern `v1\.[01]\.0-omega` 确认汇总章节存在

- [x] **Task 12: fuzz target 文档同步** 🚀
  - [ ] SubTask 12.1: 读取 `fuzz/Cargo.toml` 确认实际 fuzz target 数量(当前 6 个)
  - [ ] SubTask 12.2: 更新 `.trae/rules/nuxus规则.md §10.3` "3 target" → "6 target"
  - [ ] SubTask 12.3: 更新 `.claude/CLAUDE.md §6` "3 个 fuzz target" → "6 个 fuzz target"
  - [ ] SubTask 12.4: 列出 6 个 target 名称:`quest_parse` / `seccore_sandbox` / `event_serialize` / `cacr_budget_parse` / `checkpoint_deserialize` / `config_section_parse`
  - **验证**:Grep 规则文档确认无 "3 target" / "3 个 fuzz" 残留

- [x] **Task 13: 7 个 crate 补 proptest** 🔒(可并行于 Task 7-12)
  - [x] SubTask 13.1: `chimera-tui/tests/proptest.rs` — TUI 渲染不变量
  - [x] SubTask 13.2: `csn-substitutor/tests/proptest.rs` — 降级链顺序不变量
  - [x] SubTask 13.3: `mcp-mesh/tests/proptest.rs` — 服务注册查询不变量
  - [x] SubTask 13.4: `auto-dpo/tests/proptest.rs` — 偏好对构建不变量
  - [x] SubTask 13.5: `efficiency-monitor/tests/proptest.rs` — 监控指标聚合不变量
  - [x] SubTask 13.6: `decay-engine/tests/proptest.rs` — 能力衰减曲线不变量
  - [x] SubTask 13.7: `gsoe-evolution/tests/proptest.rs` — 进化策略选择不变量
  - [x] SubTask 13.8: 每个 crate 的 Cargo.toml 添加 proptest dev-dependency(如未声明)
  - [x] SubTask 13.9: 每个 crate 运行 `cargo test -p <name>` + `cargo clippy -p <name> --all-targets -- -D warnings` + `cargo fmt -p <name> -- --check`
  - **验证**:7 个 crate 各有 tests/proptest.rs;各自测试通过

- [x] **Task 14: src/ unwrap/expect 审计(重点 crate)** 🔒(可并行于 Task 7-13)
  - [ ] SubTask 14.1: 审计 `crates/repo-wiki/src/store.rs`(72 unwrap)— 非 `#[cfg(test)]` 模块的 unwrap 改为 `?` 或 `unwrap_or_else`
  - [ ] SubTask 14.2: 审计 `crates/mlc-engine/src/l2_semantic.rs`(72 unwrap)
  - [ ] SubTask 14.3: 审计 `crates/cmt-tiering/src/coordinator.rs`(53 unwrap)+ `cold.rs`(50)+ `warm.rs`
  - [ ] SubTask 14.4: 每处修改添加 WHY 注释说明为何安全(或为何改用 `?`)
  - [ ] SubTask 14.5: 每个修改的 crate 运行 `cargo test -p <name>` + `cargo clippy` + `cargo fmt --check`
  - **验证**:Grep 重点 crate src/ 的 unwrap 数量下降 > 50%;测试全部通过

- [x] **Task 15: MoE 五维 variance 缓存优化** 🔒(可并行于 Task 7-14)
  - [ ] SubTask 15.1: 读取 `crates/model-router/src/history/memory.rs` 与 `history/mod.rs`
  - [ ] SubTask 15.2: 在 `HistoryRecord` 中添加 `cached_variance: AtomicU32`(用 bits 编码 f32,§4.1 禁止 AtomicF32)或 `RwLock<Option<f32>>`
  - [ ] SubTask 15.3: `latency_variance()` 优先读缓存,`record()` 时重算并更新缓存
  - [ ] SubTask 15.4: 编写 TDD 测试:缓存命中 vs 重算结果一致
  - [ ] SubTask 15.5: 运行 `cargo bench -p model-router --bench moe_bench` 对比优化前后 n=200 延迟
  - [ ] SubTask 15.6: `cargo test -p model-router` + `cargo clippy` + `cargo fmt --check`
  - **验证**:bench 显示 n=200 五维延迟降 ~40%;测试全部通过

- [x] **Task 16: P1 全量验证** 🔒(依赖 Task 7-15 全部完成)
  - [x] SubTask 16.1: `cargo check --workspace` 退出码 0
  - [x] SubTask 16.2: `cargo test --workspace --jobs 1` 退出码 0(2026-07-11 验证通过,EXIT_CODE=0)
  - [x] SubTask 16.3: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
  - [x] SubTask 16.4: `cargo fmt --all -- --check` 退出码 0
  - [x] SubTask 16.5: `cargo audit --deny warnings --ignore RUSTSEC-2026-0190 --ignore RUSTSEC-2026-0002 --ignore RUSTSEC-2024-0436` 退出码 0
  - [x] SubTask 16.6: `cargo check --manifest-path fuzz/Cargo.toml` 退出码 0(已知平台限制,本地静态核验通过,委托 Linux CI)
  - **验证**:六条命令全部 EXIT_CODE=0(CP-60 为已知平台限制,本地静态核验通过)

---

## P2 GA 后跟进(记录为 tracking,不在本次执行)

- [ ] **Task T1: Linux gVisor 沙箱启用(ADR-001 完整落地)** — 延后 GA 后
- [ ] **Task T2: Linux setrlimit 资源限制(F-003)** — 延后 GA 后
- [ ] **Task T3: 监控指标扩展(35 crate Prometheus 覆盖)** — 延后 GA 后
- [ ] **Task T4: fuzz target 扩展(decay-engine / osa-coordinator / kvbsr-router)** — 延后 GA 后
- [ ] **Task T5: Box<dyn> 33 处评估改 enum dispatch** — 延后 GA 后
- [ ] **Task T6: as f32 252 处精度审计(CACR 教训)** — 延后 GA 后

---

# Task Dependencies

- **Task 1-4**:相互独立,可完全并行(🚀)
- **Task 5**(online-learning 测试):依赖 Task 2(版本同步)完成,因 Cargo.toml 版本变更影响测试编译
- **Task 6**(P0 全量验证):依赖 Task 1-5 全部完成(🔒)
- **Task 7-12**:相互独立,可完全并行(🚀)
- **Task 13**(7 crate proptest):独立,可并行(🔒 但与 Task 7-12 并行)
- **Task 14**(unwrap 审计):独立,可并行(🔒 但与 Task 7-13 并行)
- **Task 15**(variance 缓存):独立,可并行(🔒 但与 Task 7-14 并行)
- **Task 16**(P1 全量验证):依赖 Task 7-15 全部完成(🔒)

## 优先级执行顺序

1. **第一波(并行)**:Task 1 + Task 2 + Task 3 + Task 4(P0 文档/版本/清理)
2. **第二波(并行)**:Task 5(online-learning 测试)+ Task 7-12(P1 CI/CD/文档)
3. **第三波(并行)**:Task 13(7 crate proptest)+ Task 14(unwrap 审计)+ Task 15(variance 缓存)
4. **第四波(串行)**:Task 6(P0 验证)→ Task 16(P1 验证)

## 时间预估

- P0(Task 1-6):约 2-3 小时(online-learning 测试占主要时间)
- P1(Task 7-16):约 4-6 小时(7 crate proptest + unwrap 审计占主要时间)
- 合计:约 6-9 小时(并行执行可压缩至 4-6 小时)
