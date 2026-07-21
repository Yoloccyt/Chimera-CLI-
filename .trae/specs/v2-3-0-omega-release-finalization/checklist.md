# Checklist: v2.3.0-omega 发布就绪

> **change-id**: v2-3-0-omega-release-finalization
> **版本**: v2.3.0-omega

---

## 版本号与文档

- [x] `Cargo.toml` workspace.package.version = "2.3.0-omega"
- [x] `CHANGELOG.md` 包含 `## v2.3.0-omega (2026-07-20)` 条目
- [x] CHANGELOG 条目汇总架构审计、TUI 收尾、治理规范化

---

## 代码质量

- [x] `cargo test --workspace` 全部通过（2877 tests, 0 failed）
- [x] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 零警告
- [x] `cargo fmt --all -- --check` 格式一致
- [x] `cargo check --workspace` 类型检查通过

---

## 压力测试与 Fuzz

- [x] `cargo test --workspace --release -- --ignored --nocapture` 压力测试通过（5 stress tests）
- [x] `cargo check --manifest-path fuzz/Cargo.toml` fuzz 配置正确

---

## 构建与安全

- [x] `cargo build --workspace --release` binary 体积 3.44MB < 50MB
- [x] `scripts/verify_docker_locally.ps1` Docker 降级验证通过（6 项静态检查）
- [x] `cargo audit --deny warnings` 安全审计通过（1166 advisories, 297 deps, 零漏洞）

---

## 发布

- [ ] `git tag v2.3.0-omega` 已创建
- [ ] `git push origin v2.3.0-omega` 已推送