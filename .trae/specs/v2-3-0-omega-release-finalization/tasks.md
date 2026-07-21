# Tasks: v2.3.0-omega 发布就绪

> **change-id**: v2-3-0-omega-release-finalization
> **版本**: v2.3.0-omega

---

## Task 1: 版本号与 CHANGELOG 更新 [P1] ✅

- [x] SubTask 1.1: 更新 `Cargo.toml` workspace.package.version 为 "2.3.0-omega"
  - 文件: `d:\Chimera CLI\Cargo.toml`
  - 将 `version = "2.2.0-omega"` 改为 `version = "2.3.0-omega"`
- [x] SubTask 1.2: 在 `CHANGELOG.md` 添加 v2.3.0-omega 条目
  - 文件: `d:\Chimera CLI\CHANGELOG.md`
  - 在文件顶部（`## v2.2.0-omega` 之前）插入 `## v2.3.0-omega (2026-07-20)` 章节
  - 汇总内容：架构审计报告、TUI 收尾（动态 tick + TaskManagerPanel 测试）、治理规范化（专家团队框架 + 任务优先级体系）

## Task 2: 全量测试回归 [P0] ✅

- [x] SubTask 2.1: 运行 `cargo test --workspace` 确保全量测试通过
  - 验证: 2877 tests passed, 0 failed, exit code 0
- [x] SubTask 2.2: 运行 `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 确保零警告
  - 验证: 零 clippy 警告，退出码为 0
- [x] SubTask 2.3: 运行 `cargo fmt --all -- --check` 确保格式一致
  - 验证: 所有文件格式一致，退出码为 0
- [x] SubTask 2.4: 运行 `cargo check --workspace` 确保类型检查通过
  - 验证: 零编译错误，退出码为 0

## Task 3: 压力测试与 Fuzz 验证 [P1] ✅

- [x] SubTask 3.1: 运行 `cargo test --workspace --release -- --ignored --nocapture` 确保压力测试通过
  - 验证: 5 stress tests passed, 无性能退化
- [x] SubTask 3.2: 运行 `cargo check --manifest-path fuzz/Cargo.toml` 确保 fuzz 配置正确
  - 验证: fuzz target 编译通过，退出码为 0

## Task 4: Release Binary 构建与 Docker 验证 [P1] ✅

- [x] SubTask 4.1: 运行 `cargo build --workspace --release` 构建 release binary
  - 验证: 构建成功，`target/release/chimera.exe` 体积 3.44MB < 50MB
- [x] SubTask 4.2: 运行 `scripts/verify_docker_locally.ps1` 验证 Docker 镜像
  - 验证: 三级降级验证通过（Dockerfile 6 项静态检查 + binary 体积代理指标）

## Task 5: 安全检查 [P1] ✅

- [x] SubTask 5.1: 运行 `cargo audit --deny warnings --ignore RUSTSEC-2026-0190 --ignore RUSTSEC-2026-0002 --ignore RUSTSEC-2024-0436`
  - 验证: 1166 security advisories loaded, 297 crate dependencies scanned, 零漏洞, exit code 0

# Task Dependencies

- Task 2 依赖 Task 1（版本号更新后运行测试以验证一致性）
- Task 3 依赖 Task 2（测试回归通过后再跑压力测试）
- Task 4 依赖 Task 3（所有检查通过后再构建 release）
- Task 5 可与 Task 1-4 并行