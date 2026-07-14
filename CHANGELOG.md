# Changelog

## v1.7.0-omega (2026-07-14)

**版本代号**: NEXUS-OMEGA (Evolved Interface)

### 主要变化

#### 🖥️ TUI 完整重构 (v1.7-omega Milestone M0-M6)

- **M0** — 接入 EventBus 实时数据流 (443a49c)
- **M1** — 重构为 Panel 架构，分离面板责任 (9b9c97f)
- **M2** — 5 系统监控面板交付 (70ed23d)
- **M3** — 增强交互：命令执行、搜索过滤、弹窗、鼠标与可调整布局 (e04e602)
- **M4** — 双向控制：TUI 通过 EventBus 发布控制请求 (1dfaf95)
- **M5** — TUI P1 验证与打磨 (9346d4a)
- **P2** — 5 监控面板完整交付 (49c22ef)
- **P3** — 交互能力升级 (2ca37ec)
- **P4** — 性能优化 (0b4a356)
- **P5** — 跨面板联动 (a267a6d)
- **P6** — 主题运行时切换与布局模板 (93bc535)

#### 🔒 P0 安全修复

- 清理 main/master 分叉 (af00fda)
- 移除 sqlite-vec 违规 unsafe 依赖 (af00fda)
- 升级 Dockerfile 基础镜像 rust:1.82 → 1.85 (af00fda)
- 添加 .gitignore .worktrees/ 排除 (af00fda)
- 添加 P0 验证脚本 `scripts/verify-p0-cleanup.ps1` (72fc20b)

#### 📦 安装与分发

- 新增 README.md 项目首页 (9cd6be8)
- 新增 Scoop 包管理器 manifest `bucket/chimela.json` (9cd6be8)
- 新增 Homebrew 包管理器 formula `Formula/chimela.rb` (9cd6be8)
- 新增 GPG 签名配置脚本 `scripts/setup-gpg-signing.ps1` (9cd6be8)
- 修复 PS 7.x 兼容性：改用 `[scriptblock]::Create()` (9cd6be8, 1da194d, 317e7ab)
- 统一品牌名为 chimela，消除 Release 下载 404 (9e3301b)

#### 📚 文档

- 更新发布指南至 v1.7.0-omega 精简版 (b6957a9)
- 强化 §9 代码修改前置思考与冗余代码杜绝规则 (eceafc6)
- 远程仓库文档清理 + 版本统一 (189c87b)
- TUI P3 验证归档 + v1.7-omega TUI 深化演进 spec 立项 (0f1d1a0)

#### ✅ 测试

| 类型 | 通过率 |
|------|--------|
| 单元测试 | ✅ 100% |
| 集成测试 | ✅ 100% |
| OWASP Top 10 | ✅ 20/20 |
| 压力测试 (1000 次) | ✅ 零失败 |

---

## v1.5.8-omega (2026-07-13)

- Cargo.lock 版本同步 + workspace 稳定性增强
- 发布物包含 Windows/Linux/macOS × x86_64/aarch64 五平台二进制

## v1.5.7-omega (2026-07-12)

- 首个含 GitHub Release artifacts 的版本
- 初始 5 平台 matrix 构建流水线

## v1.5.6-omega (2026-07-11)

- 持续集成与依赖更新

## v1.5.5-omega — v1.5.0-omega (2026-07-09 ~ 2026-07-11)

- MCP Mesh 量子网格迭代
- CSN 降级链完善
- 监控系统深化
- 集成测试体系建立

## v1.4.0-omega (2026-07-09)

- **架构跳跃版本**: L1-L10 全部 34 crate 功能完整
- SSRA Fusion、LSCT Tiering、GSOE Evolution、NMC Encoder、CHTC Bridge 五大 L2+L10 crate 接入
- MCP 量子网格原型
- E2E 测试体系建立

## v1.0.2-omega — v1.0.0-omega (2026-06-27 ~ 2026-06-28)

- **首周启动**: L0-L1 基础设施、Event Bus、SecCore、Decay、QEEP、CLI 入口
- L9+L5+L1: Quest Engine、Repo Wiki、Model Router
- L6: MLC、HCW、CMT、OSA、KVBSR
- L7: GEA、GQEP、PVL、MTPE、SCC
- L8+L4+L3: Parliament、ASA、AHIRT、TTG、DECB
- **v1.0.0-omega 初始发布** (2026-06-28): 34 crate 全覆盖，3000+ 测试全绿
