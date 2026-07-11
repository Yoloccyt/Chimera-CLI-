---
alwaysApply: true
---
# 全局指令 + Chimera CLI 项目专属规则

> 本文件包含通用协作约定与 Chimera CLI (NEXUS-OMEGA) 项目专属硬约束。项目特定的环境设置、常用命令、CI/CD 操作与发布流程见 `.claude/CLAUDE.md`;代码 Wiki 与 ADR 权威源见 `CODE_WIKI.md`。两份文件应交叉引用,避免重复或冲突。

## 协作偏好

- **语言**:全部用中文回复(代码标识符、命令、错误信息保持原文)
- **代码风格**:简洁、实用,避免过度工程化与过早抽象，要确保清晰的代码逻辑性和高代码可读性，必要时添加清晰的代码注释，以便于其他开发者理解代码的功能和逻辑。修改代码时避免短视、综合对整套项目系统的理解和影响，保持长期主义，不能竭泽而渔，尤其是要保证修改的是高质量代码（清晰的代码逻辑性和代码的高可读性），给更改的代码添加清晰的代码注释。
- **解释强度**:写代码前后给出 `★ Insight` 教育性见解
- **决策点**:涉及业务逻辑、错误处理、算法选型时，**邀请我参与**写关键 5-10 行

## 通用编码约束

- 不主动写注释——只在 WHY 不明显的地方加(隐藏约束、变通方案、反直觉行为)
- 不引入未被任务要求的特性、抽象、向后兼容垫片
- 不为不可能发生的场景写防御性代码;只在系统边界(用户输入、外部 API)做校验
- UI 改动必须真正在浏览器里跑过再说"完成";仅靠 type-check + 测试不算验证 UX
- 不因为遇到障碍就用破坏性命令绕过;先找根因

## Git 协作

- 永远**不要** `git add -A` / `git add .`,逐文件 add 以防误纳敏感文件
- 永远**不要**在未明确请求时:force-push、删除分支、amend、修改 git config
- 永远**不要** `--no-verify`、`--no-gpg-sign` 跳过 hooks/签名(除非用户明确要求)
- 提交信息用 HEREDOC 传入,避免格式损坏
- tag 命名遵循 `v*.*.*-omega` 约定(触发 release.yml + fuzz.yml CI)

## 工作目录与平台

- **平台**:Windows 11 + bash(Git Bash);用正斜杠 `/` 替代反斜杠;空设备用 `/dev/null`(不是 `NUL`)
- **路径**:含中文/特殊字符的路径必须用双引号包裹
- **Rust 工具链**:使用 `cargo` 命令编译/测试/检查;优先用 `cargo check` 做快速类型验证,`cargo build` 做完整构建,`cargo test` 运行测试
- **工具链位置**:Rust 工具链已迁移到 D 盘(`D:\Chimera CLI\.toolchain\`),默认使用 GNU 工具链(`stable-x86_64-pc-windows-gnu`),链接器使用 `D:\msys64\mingw64\bin\gcc.exe`。

> 环境变量设置、D 盘重定向方案与工具链一致性说明见 `.claude/CLAUDE.md` §1。`.cargo/config.toml` 与 `rust-toolchain.toml` 已入库(2026-06-29 修复),但 `CARGO_HOME`/`RUSTUP_HOME`/`PATH`/`TMP`/`TEMP` 仍需按 `.claude/CLAUDE.md` §1 设置。

## 工具使用偏好

- 文件搜索:**Glob**(不是 `find`)
- 内容搜索:**Grep**(不是 `grep` / `rg`)
- 文件读取:**Read**(不是 `cat`)
- 编辑:**Edit / Write**(不是 `sed` / `awk` / `echo >`)
- 长调研:用 Agent + Explore 子代理;短查找直接用 Glob/Grep
- **MCP 工具**:调用前先读取 tool schema 确认参数,所有参数通过 `args` 字段传入

## 记忆系统

- 路径:`c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\`
- 会话记忆按日期存放:`<date>/session_memory_<session_id>.jsonl`
- 主题索引:`<date>/topics.md`
- 项目级持久规则: `project_memory.md`
- 在引用记忆前必须验证(grep/读文件),记忆**会陈旧**
- 项目状态变化大时(数量/规则/路径),先更新记忆再继续
- **引用机制**(§10.4):本规则只摘录 Hard Constraints 摘要与核心新红线,60+ 条 Week 1-8 实战 Lessons 保留在 `project_memory.md`,通过引用指针访问,避免规则膨胀

---

# 🧬 Chimera CLI (NEXUS-OMEGA) 项目专属规则

> 以下规则基于 `AETHER_NEXUS_OMEGA_ULTIMATE.md` 定义的架构,所有决策必须与 OMEGA 四定律一致。ADR 编号以 `CODE_WIKI.md §2.3` 为权威源(见 §2.3 调和说明)。

---

## 1. 项目全景

### 1.1 身份标识

| 字段 | 值 |
|------|-----|
| 项目名 | Chimera CLI |
| 代号 | NEXUS-OMEGA (Omni-Model Engineering Generative Architecture) |
| 根目录 | `D:\Chimera CLI` |
| 技术栈 | Rust 2021 edition · Tokio async · Workspace × 35 crates |
| 核心哲学 | OMEGA 四定律: Ω-Sparse · Ω-Compress · Ω-Evolve · Ω-Event |
| 设计来源 | Claude Code 尸检 + Hermes 基因 + Qoder 骨骼 + 五大模型灵魂 |
| 创新总数 | 37 个(22 个第一代 + 15 个第三代) |
| 当前版本 | `workspace.package.version = 1.4.0-omega`; CHANGELOG 最新汇总 `v1.5.0-omega` |
| 测试规模 | 单元/集成测试 + proptest + benches + OWASP A01-A10 + E2E/压测 target(持续增加) |

### 1.2 当前开发阶段

- **阶段**:GA 后演进阶段 — 以 `CHANGELOG.md` 最新汇总章节(v1.5.0-omega)为当前基线,持续迭代优化
- **实现状态**:35/35 crate 已实现;零 `todo!()`/`unimplemented!()`;依赖铁律零违规;error 体系分层(库层 thiserror / 应用层 anyhow)
- **下一步**:按 CHANGELOG 规划继续 v1.x 演进(性能/架构/安全/监控)
- **参照**:`CHANGELOG.md` + `CODE_WIKI.md` + 本规则 §3.3 第二阶段原则

### 1.3 核心术语速查

| 缩写 | 全称 | 对应 crate |
|------|------|-----------|
| Ω-Sparse | 全维稀疏(工具/上下文/记忆/审计/预算) | `osa-coordinator` |
| CLV | Context Latent Vector (512-dim 潜在语言) | `nexus-core` |
| MLC | Multi-Level Context (四级神经形态记忆) | `mlc-engine` |
| HCW | Hierarchical Context Window (4K/32K/128K/1M) | `hcw-window` |
| CMT | Capability Memory Tiering (热/温/冷/冰) | `cmt-tiering` |
| OSA | Omni-Sparse Architecture (全维稀疏协调器) | `osa-coordinator` |
| KVBSR | KV-Block Semantic Router (两级块路由) | `kvbsr-router` |
| FaaE | Function-as-Expert (工具即专家,语义路由) | `faae-router` |
| PVL | Producer-Verifier Loop (并行流式生成验证) | `pvl-layer` |
| MTPE | Multi-Token Prediction Execution (多步预测执行) | `mtpe-executor` |
| GQEP | Gather-Query Execution Protocol (聚集执行) | `gqep-executor` |
| QEEP | Quantum-Entangled Execution Protocol (量子纠缠) | `qeep-protocol` |
| TTG | Thinking Toggle Governance (三级思考切换) | `quest-engine` |
| SSRA | Slime-Style Rapid Adaptation (黏液式适配) | `ssra-fusion` |
| ISCM | Inter-Shared Cross Module (跨层共享索引) | `repo-wiki` |
| SCC | Speculative Context Cache (推测缓存) | `scc-cache` |
| LHQP | Long-Horizon Quest Persistence (检查点持久化) | `quest-engine` |
| GSOE | Guided Self-Organizing Evolution (在线进化) | `gsoe-evolution` |
| AHIRT | Anti-Hack Intelligent Red Team (反黑客红队) | `parliament` |
| CHTC | Cross-Harness Tool Compatibility (跨平台适配) | `chtc-bridge` |
| OL | Online Learning (在线参数学习) | `online-learning` |

---

## 2. 十层架构与依赖规则

### 2.1 分层映射(L1→L10)

> 权威源:`CODE_WIKI.md §2.1`。注意 `AETHER_NEXUS_OMEGA_ULTIMATE.md §3.1` 描述的 L0-L10(11 层)为早期设计,已废弃,以本表 10 层为准。

```
L10  Interface ── chimera-cli · chimera-tui · chtc-bridge · mcp-mesh · csn-substitutor
L9   Quest ───── quest-engine · gea-activator · efficiency-monitor
L8   Parliament ─ parliament · acb-governor · decb-governor
L7   Execution ── pvl-layer · gqep-executor · mtpe-executor · ssra-fusion
L6   Router ───── osa-coordinator · kvbsr-router · faae-router · sesa-router
L5   Knowledge ── repo-wiki · gsoe-evolution · auto-dpo
L4   Security ─── seccore · qeep-protocol · decay-engine
L3   Storage ──── scc-cache · lsct-tiering · cmt-tiering
L2   Memory ───── nmc-encoder · hcw-window · mlc-engine
L1   Core ─────── nexus-core · event-bus · model-router · online-learning
```

### 2.2 依赖铁律

```
L(N) → L(N)   ✓ 同层互引允许
L(N) → L(N-1) ✓ 向下依赖允许
L(N) → L(N+1) ✗ 向上依赖禁止
L(N) ──event-bus── L(M)  ✓ 跨层通信只能走 Event Bus
L(N) ──mcp-mesh─── L(M)  ✓ 跨进程通信只能走 MCP Mesh
```

- `nexus-core` 必须保持最小依赖,不能直接 import 上层任何 crate
- `event-bus` 是唯一的模块间通信通道,所有状态变更必须通过事件类型广播
- 任何违反依赖方向规则的 import 必须被拒绝,除非有 ADR 记录特批
- **dev-dependencies 可绕过生产依赖方向**(测试代码非生产代码),但仅限 `tests/` 目录
- 所有 crate 必须 `#![forbid(unsafe_code)]`(crate 级,不传播到依赖,见 §4.1)

### 2.3 ADR 决策参考

> ⚠️ **ADR 编号调和**:以 `CODE_WIKI.md §2.3` 为权威源。`AETHER_NEXUS_OMEGA_ULTIMATE.md §10.3` 的 ADR 编号为早期草案,已与 CODE_WIKI 冲突(ADR-003/004/005 定义不同),后续将在 ULTIMATE.md 加历史注释说明。

| ADR | 主题 | 启示 | 落地状态 |
|-----|------|------|---------|
| ADR-001 | 沙箱运行时选择(gVisor) | 执行沙箱优先 | ⚠️ 降级(seccore `sandbox.rs:127` 注释"当前实现为降级版本") |
| ADR-002 | 能力衰减模型设计 | 连续权限流体 | ✅ decay-engine 落地 |
| ADR-003 | Event Bus 实现选型 | Tokio broadcast | ✅ event-bus 落地 |
| ADR-004 | 消息序列化协议 | MessagePack | ✅ rmp-serde 18 文件使用 |
| ADR-005 | 持久化存储选型 | SQLite + 向量 | ⚠️ 部分降级(sqlite-vec 0.1.9 违反 forbid(unsafe_code),改内存 KNN) |
| ADR-006 | rusqlite 依赖从 nexus-core 下沉 | L1 trait abstraction(`PragmaCapable` trait) | ✅ 已完成(2026-07-08) |
| ADR-007 | EventTopic 9 类分类 + FilteredSubscriber | 架构纯净度优先 | ✅ 已完成(2026-07-09) |
| ADR-008 | ACB tier 切换滞后机制 | `tier_switch_lag_ms` 防止振荡 | ✅ 已完成(2026-07-09) |
| ADR-009 | Skeptic 否决覆议机制 | 2/3 超级多数 | ✅ 已完成(2026-07-09) |
| ADR-010 | 配置类型迁移到 L1 nexus-core | 消除平行类型漂移风险 | ✅ 已完成(2026-07-09) |

> 完整 ADR 权威源见 `CODE_WIKI.md §2.3`。

## 3. 当前发布阶段感知

### 3.1 GA 后演进阶段规则

> 项目第一阶段(Stage 0-8 / v1.0.0-omega GA)已完成。当前处于 **GA 后演进阶段**,以 `CHANGELOG.md` 最新汇总章节(v1.5.0-omega)为当前基线,`Cargo.toml` `workspace.package.version = 1.4.0-omega` 为发布候选版本号。所有决策继续遵循 OMEGA 四定律与 §2.2 依赖铁律。

此阶段开发原则:

1. **OMEGA 四定律守恒** — Ω-Sparse / Ω-Compress / Ω-Evolve / Ω-Event 不可变更,任何演进必须与之对齐
2. **依赖方向不可逆** — 遵循 §2.2 依赖铁律,跨层通信只走 Event Bus / MCP Mesh,向上依赖禁止
3. **TDD 守恒** — 新特性/bugfix 必须先写失败测试再实现;不允许删除已有测试
4. **领域类型稳定性** — 核心领域类型(`UserIntent`/`Quest`/`Checkpoint`/`OmniSparseMasks`/`CLV`/`NexusState`)变更需 ADR 记录
5. **向后兼容** — API 变更须遵循 SemVer,破坏性变更需 major 版本升级
6. **新 crate 准入** — 允许新建 crate,但必须完成 §3.3.2 新 crate 准入 checklist
7. **发布前检查清单** — tag 推送前必跑 §7.2 检查清单全部通过

### 3.2 8 周推进历史

8 周推进计划已全部完成,作为历史归档移到 **附录 §A.1**。当前不再作为决策依据,仅作回顾参考。

### 3.3 第二阶段开发(GA 后演进)

> **阶段定义**:v1.0.0-omega GA 发布后的持续演进阶段(v1.1.0-omega 及之后,当前已演进至 v1.5.0-omega)。项目从"功能完成 + 稳定发布"转向"创新深化 + 生态扩展"。第一阶段已交付 35/35 crate 全覆盖,第二阶段聚焦创新点深化、性能极致优化、跨场景套用。

#### 3.3.1 开发原则

1. **OMEGA 四定律守恒** — Ω-Sparse / Ω-Compress / Ω-Evolve / Ω-Event 不可变更,任何演进必须与之对齐
2. **依赖方向不可逆** — 遵循 §2.2 依赖铁律,跨层通信只走 Event Bus / MCP Mesh,向上依赖禁止
3. **TDD 守恒** — 新特性/bugfix 必须先写失败测试再实现;不允许删除已有测试
4. **领域类型稳定性** — 核心领域类型(`UserIntent`/`Quest`/`Checkpoint`/`OmniSparseMasks`/`CLV`/`NexusState`)变更需 ADR 记录
5. **向后兼容** — GA 后 API 变更须遵循 SemVer,破坏性变更需 major 版本升级
6. **新 crate 准入** — 见 §3.3.2 准入 checklist

#### 3.3.2 新 crate 准入 checklist

新增 crate 必须完成以下步骤,否则视为规则违反:

1. 写入 `Cargo.toml [workspace].members`。
2. 在 `CODE_WIKI.md §3.1` 与 `nuxus规则.md §2.1` 中登记层级归属。
3. 创建 `crates/<name>/tests/` 与 `crates/<name>/benches/`;至少包含 `tests/proptest.rs`。
4. `Cargo.toml` 使用 `version.workspace = true`、`edition.workspace = true`,依赖优先 `workspace = true`。
5. 若使用 `chrono`/`rand`/`serde` 等 workspace 已有依赖,必须显式写入 `Cargo.toml`,禁止隐式使用。
6. 顶层声明 `#![forbid(unsafe_code)]` 与 `#![warn(missing_docs, clippy::all)]`。
7. 遵循 §4.2 模块组织模式(`lib.rs`/`types.rs`/`config.rs`/`error.rs`)。
8. 运行并至少通过:
   ```powershell
   cargo check -p <name>
   cargo test -p <name>
   cargo clippy -p <name> --all-targets -- -D warnings
   cargo fmt --all -- --check
   ```
9. 更新 `CHANGELOG.md` 与 project_memory。

#### 3.3.3 主要参考资料(互补分工)

第二阶段开发以下列两份文档为主要参考,覆盖"如何搭"与"如何进化"两个维度:

| 文档 | 角色 | 版本 | 适用场景 |
|------|------|------|---------|
| `AETHER_NEXUS_OMEGA_从零搭建完全指南.md` | **工程实施主参考**(如何搭) | v2.0.0-omega | 新 crate 搭建、模块从零实现、架构全貌理解、搭建步骤参考 |
| `OMEGA_大模型架构魔改创新_AI_Agent项目套用设计.md` | **创新演进主参考**(如何进化) | v3.0.0-omega | 创新点演进、五大模型理念融合、魔改架构深化、学术支撑引用 |

#### 3.3.4 引用说明与实施指导

**从零搭建完全指南(v2.0.0-omega)** — 工程实施主参考:

- ⚠️ **已知错误**:文档中"37 crates 骨架"数量错误,实际为 35 crate(以 `Cargo.toml` workspace.members 为权威)
- ⚠️ **版本漂移**:文档基于早期设计,部分 crate 名称/层级可能已演进,以 `CODE_WIKI.md §3.1` 35 crate 索引为权威
- **适用章节**:
  - §5 OMEGA 十层架构详解 → 理解分层设计原理
  - §7 核心模块从零实现 → 新 crate 实现模式参考
  - §8 12 周推进计划 → 历史归档,不作决策依据(以本规则 §A.1 为准)
- **实施指导**:搭建新 crate 时,先查 `CODE_WIKI.md §3.1` 与 `nuxus规则.md §2.1` 确认层级归属与依赖方向,再参考本指南的模块实现模式;遇到"37 crates"描述一律以 35 为准

**OMEGA架构魔改创新(v3.0.0-omega)** — 创新演进主参考:

- **学术支撑**:20+ 篇 2025-2026 顶会论文(NeurIPS/ICLR/arXiv),创新点有理论根基
- **适用章节**:
  - §3 十二大魔改创新架构 → 创新点深化与演进方向
  - §4 项目实践中的具体套用 → 架构魔改落地参考
  - §6 附录:架构决策记录 → ADR 补充参考(以 `CODE_WIKI.md §2.3` 为权威源)
- **实施指导**:规划新创新特性时,先核对本文档的十二大魔改创新是否已覆盖,避免重复设计;演进现有创新点时参考五大模型(DeepSeek V4 / Kimi K2.7 / GLM 5.2 / Minimax M3 / Qwen 3.7 Plus)理念映射

#### 3.3.5 第二阶段开发检查清单

- [ ] 新特性是否与 OMEGA 四定律(Ω-Sparse / Ω-Compress / Ω-Evolve / Ω-Event)对齐?
- [ ] 是否查阅了从零搭建指南的对应模块实现(§7 核心模块从零实现)?
- [ ] 是否查阅了 OMEGA架构魔改的对应创新点(§3 十二大魔改创新架构)?
- [ ] 依赖方向是否遵守 §2.2 铁律(L(N)→L(N-1) 允许,L(N)→L(N+1) 禁止)?
- [ ] 是否先写失败测试再实现(TDD 守恒)?
- [ ] 核心 API 变更是否有 ADR 记录?
- [ ] 新 crate 是否已完成 §3.3.2 准入 checklist?

---

## 4. Rust 编码规范(项目定制)

### 4.1 通用约定

```rust
// ✓ 正确:workspace 级版本
[package]
name = "my-crate"
version.workspace = true
edition.workspace = true

// ✓ 正确:workspace 级依赖
[dependencies]
tokio = { workspace = true }
serde = { workspace = true }

// ✗ 错误:独立声明版本(除非 workspace 未收录)
tokio = { version = "1.40", features = [...] }
```

- 所有 async fn 必须满足 `Send + 'static` 约束,避免 spawn 失败
- 应用层错误用 `anyhow::Result<T>`,库层用自定义 `thiserror` enum(33 个 error.rs 全部 thiserror)
- 避免 `unwrap()`/`expect()` — 所有可能失败的边界必须用 `?` 或 `match` 处理(ttg.rs 7 处 expect 已修复为 `unwrap_or_else`)
- 避免 `Box<dyn Trait>` — 优先使用 `impl Trait` 或 `enum dispatch`(chtc-bridge 5 IDE 适配器用 enum dispatch)
- **所有 crate 必须 `#![forbid(unsafe_code)]`** — crate 级属性,只约束当前 crate 源码,不传播到依赖(rusqlite bundled / prometheus-client 内部 unsafe 不影响当前 crate)
- **Top-K 选择必须用 `select_nth_unstable` (O(n))** — 禁止 `sort_by` (O(n log n)) 做 Top-K
- **proptest 1.11+ 用 block-named 语法** — `fn test_name(x in 0..100u32) { ... }`,closure 形式某些 pattern 解析失败
- **并发收集用 `FuturesUnordered`** — 优于 `join_all`,减少内存占用,支持流式结果
- **风险规则列表为空时须返回 `RiskLevel::Unknown`** — 禁止默认视为安全;同时记录 `warn!` 日志(seccore ASA 空关键字教训)
- **SQLite `PRAGMA journal_mode=WAL` 必须用 `pragma_update` API** — `execute("PRAGMA journal_mode=WAL;")` 不生效;`conn` 须声明为 `mut`
- **u64 预算/大整数参与百分比计算须用 `f64` 中间值** — `u64 > 2^24` 时直接 `f32` 乘法会精度丢失(model-router CACR 教训)
- **stable Rust 不存在 `AtomicF32`** — 并发浮点计数改用 `AtomicU32` + `f32::to_bits`/`from_bits`,或 `RwLock<f32>`
- **降级/耗尽路径必须发布对应 `NexusEvent`** — 如 CSN 降级链耗尽必须发 `ChainExhausted`,禁止只用 `warn!` 日志
- **TTG 事件禁止新增 `ThinkingModeChanged`** — 统一复用现有 `ThinkingModeSwitched`,保持向后兼容
- **多治理器协同必须经 `ArbitrationLayer` 取保守值** — 如 ACB/DECB 同时影响 TTG 时,所有路径统一通过 `effective_tier()` 取 max 降级
- **GQEP 必须实现 `gather_deadline_ms` 全局超时** — 在单操作超时之上再加全局截止时间,超时保留已完成结果

### 4.2 模块组织模式

每个 crate 的标准布局:

```
my-crate/
├── Cargo.toml
├── src/
│   ├── lib.rs           # 公开 API 导出:pub mod ... + prelude + #![forbid(unsafe_code)]
│   ├── types.rs         # 核心类型定义
│   ├── config.rs        # 配置解析(Figment 多源)
│   ├── error.rs         # 错误类型(thiserror enum)
│   └── ...              # 功能子模块
│   └── tests/           # 集成测试
│       └── integration.rs
```

### 4.3 此项目特有的命名模式

| 模式 | 示例 | 说明 |
|------|------|------|
| `*Coordinator` | `OmniSparseCoordinator` | 协调器模式,管理多个子组件 |
| `*Engine` | `DecayEngine` | 引擎模式,有独立生命周期 |
| `*Router` | `KVBlockSemanticRouter` | 路由模式,输入→匹配→输出 |
| `*Protocol` | `QuantumEntangledProtocol` | 协议模式,定义通信契约 |
| `*Governor` | `ACBGovernor` | 治理模式,速率/预算控制 |
| `*Mask<T>` | `OmniSparseMasks` | 掩码模式,稀疏化选择 |
| `*Block` | `SemanticBlock` | 块模式,结构化数据单元 |

### 4.4 async 反模式清单(Week 1-8 实战教训)

> 以下反模式来自 `project_memory.md` Lessons Learned,违反即触发 CI 失败或运行时死锁。

1. **禁止持锁跨 `.await`** — DashMap/Mutex 写锁必须在 `.await` 前释放(faae-router `tests/lock_holding.rs` 检测)。正确模式:锁内取快照→释放锁→await 快照
2. **rusqlite 调用必须 `spawn_blocking`** — rusqlite 非 async,直接在 async 上下文调用阻塞 runtime(repo-wiki/scc-cache 79 处已包装)
3. **`tokio::broadcast` 不缓存历史消息** — `bus.subscribe()` 必须在 `tokio::spawn()` **之前同步调用**,否则事件静默丢失(Week 6 SSRA 教训,Week 7 4 crate 遵循)
4. **`with_event_bus(config, bus)` 会 move bus** — 若构造器 consume bus by value,subscribe 必须在 `with_event_bus` 之前,或让构造器内部 subscribe(efficiency-monitor 教训)
5. **`Arc::new(self.chains.clone())` 创建独立副本** — async 任务需共享 mutate 状态必须用 `Arc::clone(&self.chains)`,不是 clone(csn-substitutor 教训)
6. **f32 禁止隐式转 f64 比较** — `0.4f32 as f64` 精度膨胀变为 > 0.4,导致稀疏度 < 40% 误判为 ≥ 40%(sesa-router 教训),全程保持 f32
7. **`tokio::spawn` fire-and-forget 评估框架** — 幂等操作(重平衡/事件订阅)失败仅记日志可接受;关键路径(衰减循环)必须管理 JoinHandle;panic 影响数据一致性必须 spawn_blocking
8. **`publish_blocking()` 是 sync 方法的正确发布模式** — `tokio::spawn` 在 `#[test]` 无 runtime 会 panic;sync 方法(audit/verify_security/switch_tier)用 `publish_blocking`,async 方法用 `publish().await` 配合作用域 MutexGuard

---

## 5. 核心领域类型与数据流

### 5.1 关键类型参照

> 权威源:`nexus-core/src/types.rs` + `nexus-core/src/clv.rs` + `nexus-core/src/state.rs`。`OmniSparseMasks` 位于 `osa-coordinator/src/coordinator.rs`,`SemanticBlock` 位于 `kvbsr-router/src/types.rs`(层内所有权,非 L1 共享)。

- `UserIntent` — 多模态用户意图(含 intent_id/raw_text/multimodal_inputs/risk_level)
- `Quest` — 长期任务(含 id/tasks/thinking_mode/checkpoint_id)
- `Checkpoint` — 检查点(含 quest_id/serialized_state:Vec<u8> MessagePack/memory_snapshot_hash/created_at)
- `OmniSparseMasks` — 全维稀疏掩码(routing/context/memory/audit/budget 五维度)
- `SemanticBlock` — 语义块(含 block_id/block_vector/capability_id)
- `CLV` — 上下文潜在向量(512-dim f32 + `cosine_similarity_slices`)
- `NexusState` — 全局运行时状态(独立 state.rs 模块)
- `MultimodalInput::Text` — Image/Video/Audio 为 Week 6 扩展

### 5.2 数据流参考

```
用户输入 → NMC 编码 → Quest 分解 → TTG 切换
    → Parliament 审议 → PVL 生产验证
    → OSA 协调 → KVBSR 路由 → GEA 激活
    → MTPE 多步预测 → GQEP 聚集 → QEEP 纠缠
    → ISCM 更新 → Wiki 沉淀
    → GSOE 进化 → Auto-DPO → Event Bus 广播
```

### 5.3 事件总线事件类型

event-bus 定义 65 个 `NexusEvent` 变体,关键 Critical 级事件(必须用 mpsc channel 确保送达):
- `SkepticVeto` / `RedTeamAudit` / `AsaIntervention` / `BudgetExceeded`

> 完整事件清单见 `crates/event-bus/src/types.rs`。`BudgetExceeded` 的 `severity()` 必须 = `EventSeverity::Critical`(C2 修复,2026-06-25;代码权威源 `types.rs:1158`)。

---

## 6. 架构红线

### 6.1 原始六条尸检红线

每次做架构/实现决策时,对照以下"尸检教训":

| 问题 | Claude Code 教训 | 本项目红线 |
|------|-----------------|-----------|
| 函数太大? | `print.ts` 3167 行神函数 | **单函数 ≤200 行,超过必须拆模块** |
| 结果丢了? | 5.4% 孤儿调用 | **所有异步操作必须有 GQEP 聚集/超时处理** |
| 裸奔? | 命令插值 + auth 跳过 | **所有外部调用经 SecCore 沙箱 + Decay 衰减** |
| 竞态? | void Promise 无 await | **所有 async 必须 await 或 spawn 管理** |
| 功能乱? | 44 个未发布标志 | **禁止功能标志,用能力场自然进化替代** |
| 内存爆炸? | 1M Token 暴力加载 | **必须经 HCW 分层 + OSA 稀疏化后再加载**(1M = 128K 实际 + 8× 稀疏压缩) |

### 6.2 Week 1-8 实战新红线

> 以下红线来自 `project_memory.md` Hard Constraints + Lessons Learned,违反即阻塞发布。

| 红线 | 教训来源 | 说明 |
|------|---------|------|
| **禁止持锁 .await** | faae-router 4 Critical | DashMap/Mutex 写锁跨 await 导致死锁,必须快照→释放→await |
| **rusqlite 必须 spawn_blocking** | repo-wiki/scc-cache 79 处 | rusqlite 非 async,直接调用阻塞 runtime |
| **broadcast 先 subscribe 再 spawn** | Week 6 SSRA + Week 7 4 crate | `bus.subscribe()` 必须在 `tokio::spawn()` 之前同步调用,否则事件静默丢失 |
| **BudgetExceeded severity = Critical** | C2 修复 | 禁止降级,`NexusEvent::severity()` 必须返回 `EventSeverity::Critical`(`types.rs:1158`) |
| **Critical 安全事件用 mpsc** | efficiency-monitor | SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded 必须用 mpsc channel 确保送达 |
| **禁止 cargo add 不更新 Cargo.lock** | audit.yml | `cargo audit --deny warnings` 每日扫描,依赖漂移阻塞 CI |
| **sqlite-vec 禁用(违反 forbid unsafe)** | ADR-005 降级 | sqlite-vec 0.1.9 binding 需 unsafe,改内存 KNN(10-1000 entry scale) |
| **Top-K 用 select_nth_unstable** | Engineering Convention | O(n) 替代 O(n log n) sort_by |
| **ASA 空关键字须返回 Unknown** | v1.5.0 seccore | 风险关键字列表为空禁止默认 Low,必须 `RiskLevel::Unknown` + `warn!` |
| **WAL  pragma 必须用 `pragma_update`** | v1.5.0 seccore | `execute("PRAGMA journal_mode=WAL;")` 不生效,`conn` 须 `mut` |
| **TTG 事件禁止新增变体** | v1.5.0 quest-engine | 禁止新增 `ThinkingModeChanged`,统一复用 `ThinkingModeSwitched` |
| **GQEP 必须有全局超时** | v1.5.0 gqep-executor | 必须配置 `gather_deadline_ms`,全局超时 + 单操作超时双层防护 |
| **多治理器必须经 ArbitrationLayer** | v1.5.0 quest-engine | ACB/DECB 等同时影响决策时,所有路径通过 `effective_tier()` 取保守值 |
| **CSN 降级链耗尽必须发事件** | v1.5.0 csn-substitutor | 降级链耗尽必须发布 `ChainExhausted`,禁止仅用 `warn!` |
| **CACR 大预算用 f64 中间值** | v1.5.0 model-router | `u64 > 2^24` 时百分比/阈值计算用 `f64`,禁止直接 `f32` 乘法 |
| **禁止 AtomicF32** | v1.5.0 auto-dpo | stable Rust 无 `AtomicF32`,并发浮点用 `AtomicU32`+bits 或 `RwLock<f32>` |

> 完整 60+ 条 Week 1-8+ Lessons 见 `project_memory.md`(引用机制 §10.4);v1.5.0 新增规则已同步到 §4.1。

---

## 7. 开发工作流(项目定制)

### 7.1 日常命令

完整命令速查(环境设置、构建、测试、lint、Docker、fuzz 本地静态检查)见 `.claude/CLAUDE.md` §2。

本节只保留与**规则相关**的说明:

- **clippy OOM 根因**:Windows `STATUS_STACK_BUFFER_OVERRUN (0xC0000409)` 实际是 `__fastfail` 的 `FAST_FAIL_FATAL_APP_EXIT`(P9=7),objdump 定位到 `std::alloc::rust_oom`,是 OOM 非栈溢出。`--jobs 2` 是最优缓解(44% 快于 `--jobs 1`)。
- **单 crate 修改最小验证**:修改特定 crate 时,优先 `cargo check -p <name>` / `cargo test -p <name>`,通过后再跑全量。
- **新 crate 必须隔离验证**:见 §3.3.2 checklist 第 8 条。

### 7.2 发布前检查清单(替代周验收)

完整命令版见 `.claude/CLAUDE.md` §5。本清单强调**规则级要求**:

1. `cargo check --workspace` 通过(无 `E0428` 重复定义、`E0252` 重复导入等合并后遗症)。
2. `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 通过。
3. `cargo fmt --all -- --check` 通过。
4. `cargo test --workspace` 通过;`cargo test -- --ignored --nocapture` 压力测试通过。
5. 安全审计命令必须与 `.github/workflows/audit.yml` 一致:
   ```powershell
   cargo audit --deny warnings `
     --ignore RUSTSEC-2026-0190 `
     --ignore RUSTSEC-2026-0002 `
     --ignore RUSTSEC-2024-0436
   ```
6. `cargo check --manifest-path fuzz/Cargo.toml` 通过(实际 fuzz 委托 Linux CI)。
7. Docker 镜像体积 < 100MB;`docker run --rm <image> --version` 输出匹配 `^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+`。
8. `cargo build --workspace --release` 通过;binary 体积 < 50MB。
9. **版本同步**:发布 `vX.Y.Z-omega` 前,`Cargo.toml [workspace.package].version` 必须改为 `X.Y.Z`,`CHANGELOG.md` 必须存在 `vX.Y.Z-omega` 汇总章节。
10. `git tag v<x.y.z>-omega && git push origin v<x.y.z>-omega` 触发 `release.yml` + `fuzz.yml`。

### 7.3 新建 crate 模板

GA 后演进阶段允许新建 crate。模板见附录 §A.2;新增 crate 必须完成 §3.3.2 准入 checklist。

---

## 8. 关键文件索引

### 8.1 核心文档

| 文件 | 内容 | 重要性 |
|------|------|--------|
| `CODE_WIKI.md` | 代码 Wiki(架构概览、模块职责、核心类型速查、ADR 权威源) | ⭐⭐⭐ |
| `AETHER_NEXUS_OMEGA_ULTIMATE.md` | 架构手册(10 章,25 ADR,8 周计划) — ⚠️ §3.1 层级映射已废弃,以 CODE_WIKI 为准 | ⭐⭐ |
| `AETHER_NEXUS_GEN3_OMEGA.md` | 第三代 10 大魔改创新详解 | ⭐⭐⭐ |
| `AETHER_NEXUS_FULL_DOCUMENTATION.md` | 完整文档汇编 | ⭐⭐ |
| `CHIMERA_NEXUS_COMPLETE_BUILD_GUIDE.md` | 构建指南 | ⭐⭐ |
| `AETHER_NEXUS_OMEGA_从零搭建完全指南.md` | 从零搭建指南 — ⚠️ "37 crates 骨架"数量错误(实际 35)。**第二阶段工程实施主参考**(见 §3.3.2,如何搭) | ⭐⭐⭐⭐ |
| `CHIMERA_NEXUS_GEN2_INNOVATIONS.md` | 第二代创新详解 | ⭐⭐ |
| `OMEGA_大模型架构魔改创新_AI_Agent项目套用设计.md` | OMEGA 架构魔改设计(五大模型融合,十二大魔改创新)。**第二阶段创新演进主参考**(见 §3.3.2,如何进化) | ⭐⭐⭐⭐ |
| `DEEP_RESEARCH_OPTIMIZATION_ALGORITHM.md` | 深度研究:优化算法 — ⚠️ 基于 Week 2 快照,部分 crate 已演进 | ⭐ |
| `DEEP_RESEARCH_LLM_ARCHITECTURE_MAPPING.md` | 深度研究:LLM 架构映射 — ⚠️ 基于 Week 2 快照 | ⭐ |
| `CHANGELOG.md` | 版本演进史(Week 1-8 + GA 后演进,最新 v1.5.0-omega) | ⭐⭐⭐ |
| `README.md` | 项目入口(开发状态表准确) | ⭐⭐⭐ |
| `Cargo.toml` | Workspace 根配置(35 members × 20+ 共享依赖,根 package `chimera-e2e-tests`) | ⭐⭐⭐ |

### 8.2 工程基建

| 文件 | 内容 | 重要性 |
|------|------|--------|
| `.github/workflows/audit.yml` | 每日 cargo audit + PR 触发(改 Cargo.lock) | ⭐⭐⭐ |
| `.github/workflows/release.yml` | tag 触发:5 平台 matrix build + test + docker(GHCR + 100MB + --version grep) + release | ⭐⭐⭐ |
| `.github/workflows/fuzz.yml` | tag/手动触发:nightly + cargo-fuzz 6 target × 300s(委托 Linux CI) | ⭐⭐⭐ |
| `Dockerfile` | 多阶段:rust:1.82-slim builder + distroless/cc-debian12 runtime + nonroot + HEALTHCHECK | ⭐⭐⭐ |
| `install.ps1` / `install.sh` | 跨平台安装脚本(SHA256 校验 + PATH 注入 + --version 验证) | ⭐⭐ |
| `test_version_verification.ps1` | 本地模拟 CI --version grep 校验(24 测试用例) | ⭐⭐ |
| `fuzz/Cargo.toml` | 独立 fuzz package(隔离 workspace,cargo-fuzz metadata) | ⭐⭐ |
| `.gitignore` | 覆盖 target/ + target_clippy*/ + .toolchain/ + tmp/ + .env* + *.pem | ⭐⭐⭐ |

### 8.3 测试与审计

| 文件 | 内容 | 重要性 |
|------|------|--------|
| `tests/e2e/*.rs` | 9 个 E2E 测试(week5-8 主流程 + 安全 + 集成 + 压测 + 验收) | ⭐⭐⭐ |
| `tests/security/owasp_top10.rs` | OWASP A01-A10 渗透测试(零信任白名单 + Merkle 审计链) | ⭐⭐⭐ |
| `tests/stress/week7_stress.rs` | 1000 次压测(Arc 探针 + 延迟稳定性) | ⭐⭐ |
| `fuzz/fuzz_targets/*.rs` | 6 个 fuzz target(quest_parse/seccore_sandbox/event_serialize/cacr_budget_parse/checkpoint_deserialize/config_section_parse) | ⭐⭐ |
| `docs/audit/dimension_f_security.md` | 安全审计维度文档 | ⭐⭐ |

### 8.4 规则与命令

| 文件 | 内容 | 重要性 |
|------|------|--------|
| `.trae/rules/nuxus规则.md` | 本文件(全局指令 + 项目专属规则) | ⭐⭐⭐ |
| `.claude/CLAUDE.md` | 项目特定命令(CI 触发 / Docker / fuzz 委托 / 发布 checklist) | ⭐⭐⭐ |

---

## 9. 工作时的要求

组建一个由多名拥有 10 年以上行业经验的精英专家级子代理构成的协作团队，以任务优先级为核心指导原则，对各项任务进度实施系统性的分布式深度分析。团队需通过多轮结构化思考、充分探讨及严谨的验证流程，确保对任务的理解全面且准确。在执行阶段，严格按照既定的任务优先级顺序推进实施工作，同时始终秉持长期主义的工作理念，杜绝短期行为和资源过度消耗。特别强调在代码修改过程中，必须保证产出高质量的代码成果，具体标准包括：清晰的代码逻辑结构、高度的代码可读性、完善的注释说明以及符合行业最佳实践的编码规范。在整个任务执行周期内，授权团队调用所有符合任务要求且系统允许的工具资源，包括但不限于 mcp、skills 等相关工具，以保障任务的高效完成和卓越质量。

---

## 10. 发布与运维

### 10.1 CI/CD 准入门槛

| Workflow | 触发 | 关键 job | 准入门槛 |
|----------|------|---------|---------|
| `audit.yml` | 每日 UTC 02:00 + PR 改 Cargo.lock | cargo audit | `--deny warnings` 0 退出 |
| `release.yml` | tag `v*.*.*-omega` | build(5 平台) + test + docker + release | build/test/docker 全 pass 才能 release |
| `fuzz.yml` | tag + workflow_dispatch | fuzz(6 target × 300s) | crash 上传(90 天留存),非阻塞 |

**5 平台 matrix**:Win x86_64 / Linux x86_64+aarch64 / macOS x86_64+aarch64,`fail-fast: false`。

### 10.2 Docker 镜像约束

- **基础镜像**:`gcr.io/distroless/cc-debian12`(无 shell,内置 nonroot UID 65532)
- **USER**:`nonroot:nonroot`(契合 `#![forbid(unsafe_code)]` 哲学)
- **HEALTHCHECK**:`CMD ["chimera","--version"]` exec form
- **ENTRYPOINT**:`["chimera"]`
- **体积**:< 100MB(release.yml 断言)
- **--version 验证**:`docker pull` + `docker run --rm --version`,grep `^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+`(case-sensitive,PowerShell 用 `-cmatch`)
- **品牌一致性**:内部 codename `aether`(`crates/chimera-cli/Cargo.toml [[bin]]`),Dockerfile/CI 重命名 `chimera` 保持外部品牌

### 10.3 fuzz 与 cargo-audit 委托模式

> **平台限制**:libFuzzer 的 `FuzzerExtFunctionsWindows.cpp` 仅适配 MSVC(`__declspec(dllimport)`),MinGW g++ 无法解析。Windows GNU-only 环境无法跑 cargo-fuzz。

**委托模式**(本地静态验证 + CI 实际执行):
- 本地:`fuzz/Cargo.toml` 静态核验(独立 workspace 隔离 + `[package.metadata] cargo-fuzz = true`)
- CI:`fuzz.yml` ubuntu-latest + nightly + matrix 6 target × 300s
- cargo-audit:本地网络超时时手动检查 Cargo.lock 13 个关键依赖版本

### 10.4 project_memory 引用机制

本规则只摘录 **Hard Constraints 摘要**(§6.2 8 条核心新红线)与 **async 反模式清单**(§4.4 8 条)。完整 60+ 条 Week 1-8 实战 Lessons 保留在:

```
c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md
```

**引用规则**:
- 遇到 async 死锁 / broadcast 丢事件 / SQLite 阻塞 / fuzz 失败等问题,先查 `project_memory.md` 是否有历史教训
- 引用记忆前必须验证(grep/读文件),记忆**会陈旧**
- 新教训产生时,先更新 `project_memory.md`,再评估是否提炼进本规则 §6.2

### 10.5 已知基建短板(待修复)

| 短板 | 影响 | 优先级 | 状态 |
|------|------|--------|------|
| `.cargo/config.toml` 已入库(linker配置) | ✅ 2026-06-29 已修复,linker已配置 | P0 | ✅ 已完成 |
| `rust-toolchain.toml` 已入库 | ✅ 2026-06-29 已修复,指定stable-x86_64-pc-windows-gnu | P0 | ✅ 已完成 |
| `target_clippy*/` 残留 | ✅ 2026-06-29 已清理(核验无残留) | P0 | ✅ 已完成 |
| release 镜像未设 `RUST_BACKTRACE=1` | ✅ 2026-06-29 已修复(Dockerfile 加 ENV RUST_BACKTRACE=1) | P1 | ✅ 已完成 |
| figment 三源已声明但无 `*.yaml` 配置样例 | ✅ 2026-06-29 已补齐(examples/config.sample.{yaml,toml}) | P2 | ✅ 已完成 |
| 环境变量(CARGO_HOME/PATH)仍需手动设置 | ✅ 2026-06-29 已改进(install.ps1 --setup-env) | P1 | ✅ 已完成 |
| release.yml Windows job 未安装 MinGW | `windows-latest` 默认无 `D:/msys64/...`,GNU target 可能链接失败 | P1 | ⚠️ 待验证/修复 |
| 合并/大改后 `cargo check` 重复定义 | 当前 `check_errors*.txt` 显示 `E0428`/`E0252` 等,需在提交前清零 | P0 | ⚠️ 需立即修复 |
| D盘空间管理(回收站黑洞/应用商店缓存) | 后台下载+未清空回收站可导致磁盘满 | P1 | ⚠️ 需定期清理 |

---

## 附录 §A

### §A.1 8 周推进计划速查(历史归档,已完成)

> 8 周推进计划已于 2026-06-28 全部验收通过。本附录仅作历史回顾,不再作为决策依据。

```
Week 1: L0-L1 基础设施 ─── Event Bus · SecCore · Decay · QEEP · CLI 入口
Week 2: L9+L5+L1 ──────── Quest Engine · Repo Wiki · Model Router · CACR
Week 3: L5+L6 ─────────── MLC · HCW · CMT · OSA · KVBSR
Week 4: L6+L7 ─────────── GEA · GQEP · PVL · MTPE · SCC · EDSB
Week 5: L8+L4+L3 ──────── Parliament · ASA · AHIRT · TTG · DECB
Week 6: L2+L10 ────────── SSRA · LSCT · GSOE · NMC · CHTC
Week 7: MCP Mesh ──────── MCP 量子网格 · CSN 降级链 · 监控 · 集成
Week 8: 打磨 ──────────── 性能 · 安全 · 文档 · 发布
```

### §A.2 新建 crate 模板(历史归档,RC 阶段不再新建)

```toml
[package]
name = "<crate-name>"
version.workspace = true
edition.workspace = true

[dependencies]
# 从 workspace 共享依赖中选取
tokio = { workspace = true }
serde = { workspace = true, features = ["derive"] }
anyhow = { workspace = true }
tracing = { workspace = true }
```

```rust
// src/lib.rs
#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

//! <crate 简述>
//!
//! 架构层归属: L?
//! 核心职责: <一句话>

pub mod config;
pub mod error;
pub mod types;
// pub mod <功能子模块>;

pub use error::{Error, Result};
pub use types::*;

pub mod prelude {
    pub use crate::{config::*, error::*, types::*};
}
```
