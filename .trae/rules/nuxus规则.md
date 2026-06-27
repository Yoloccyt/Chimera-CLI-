---
alwaysApply: true
---
# 全局指令

> 本文件是用户级偏好,适用于所有项目。项目特定的命令、架构约束放在 `<repo>/.claude/CLAUDE.md`,不放这里。

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

## 工作目录与平台

- **平台**:Windows 11 + PowerShell;用正斜杠 `/` 替代反斜杠;空设备用 `/dev/null`(不是 `NUL`)
- **路径**:含中文/特殊字符的路径必须用双引号包裹
- **Rust 工具链**:使用 `cargo` 命令编译/测试/检查;优先用 `cargo check` 做快速类型验证,`cargo build` 做完整构建,`cargo test` 运行测试
- **工具链位置**:Rust 工具链已迁移到 D 盘(`D:\Chimera CLI\.toolchain\`),默认使用 GNU 工具链(`stable-x86_64-pc-windows-gnu`),链接器使用 `D:\msys64\mingw64\bin\gcc.exe`。需在 PowerShell 中设置环境变量:
  ```powershell
  $env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
  $env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
  $env:TMP = 'D:\Chimera CLI\tmp'
  $env:TEMP = 'D:\Chimera CLI\tmp'
  $env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"
  ```

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

---

# 🧬 Chimera CLI (NEXUS-OMEGA) 项目专属规则

> 以下规则基于 `AETHER_NEXUS_OMEGA_ULTIMATE.md` 定义的架构与 8 周推进计划，所有决策必须与 OMEGA 四定律一致。

---

## 1. 项目全景

### 1.1 身份标识

| 字段 | 值 |
|------|-----|
| 项目名 | Chimera CLI |
| 代号 | NEXUS-OMEGA (Omni-Model Engineering Generative Architecture) |
| 根目录 | `D:\Chimera CLI` |
| 技术栈 | Rust 2021 edition · Tokio async · Workspace × 34 crates |
| 核心哲学 | OMEGA 四定律: Ω-Sparse · Ω-Compress · Ω-Evolve · Ω-Event |
| 设计来源 | Claude Code 尸检 + Hermes 基因 + Qoder 骨骼 + 五大模型灵魂 |
| 创新总数 | 37 个(22 个第一代 + 15 个第三代) |

### 1.2 当前开发阶段

- **阶段**:Stage 0 — Workspace 脚手架(仅 Cargo.toml 骨架,无 `.rs` 实现)
- **下一步**:按 8 周推进计划,从 **Week 1(L0-L1 基础设施)** 开始浇筑
- **参照路线图**:`AETHER_NEXUS_OMEGA_ULTIMATE.md` 第 7 章(8 周每日推进计划)

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

---

## 2. 十层架构与依赖规则

### 2.1 分层映射(L1→L10)

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
L1   Core ─────── nexus-core · event-bus · model-router
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

### 2.3 ADR 决策参考

25 个架构决策记录参见 `AETHER_NEXUS_OMEGA_ULTIMATE.md`和 `AETHER CLI / NEXUS-OMEGA 系统 —— 从零开始搭建终极工程手册.md` 第 10.3 节。关键 ADR:

| ADR | 主题 | 启示 |
|-----|------|------|
| ADR-001 | 沙箱运行时选择(gVisor) | 执行沙箱优先 |
| ADR-002 | 能力衰减模型设计 | 连续权限流体 |
| ADR-003 | Event Bus 实现选型 | Tokio broadcast |
| ADR-004 | 消息序列化协议 | MessagePack |
| ADR-005 | 持久化存储选型 | SQLite + 向量 |

---

## 3. 开发阶段感知规则

### 3.1 当前阶段:Stage 0 → Stage 1 过渡

> 项目当前处于**零代码阶段**(仅有 Cargo.toml 骨架)。所有 `src/` 目录为空,`lib.rs` 和 `main.rs` 尚未创建。

此阶段的开发原则:

1. **先搭骨架,再填血肉** — 每个 crate 先建好 `lib.rs` 模块结构(公开 API 签名 + `pub mod`),再逐个实现函数体
2. **TDD-first** — 核心领域类型(如 `NexusState`、`UserIntent`、`Quest`)先写类型定义和基础测试,再写业务逻辑
3. **从底层往上** — 按 L1→L2→...→L10 顺序推进,下层未稳定前上层只做接口设计
4. **每周末验收** — 严格按照 8 周推进计划的每周验收标准,通过后才进入下一周

### 3.2 8 周推进速查

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

- 所有 async fn 必须满足 `Send + 'static + 'async` 约束,避免 spawn 失败
- 应用层错误用 `anyhow::Result<T>`,库层用自定义 `thiserror` enum
- 避免 `unwrap()`/`expect()` — 所有可能失败的边界必须用 `?` 或 `match` 处理
- 避免 `Box<dyn Trait>` — 优先使用 `impl Trait` 或 `enum dispatch`

### 4.2 模块组织模式

每个 crate 的标准布局:

```
my-crate/
├── Cargo.toml
├── src/
│   ├── lib.rs           # 公开 API 导出:pub mod ...
│   ├── types.rs         # 核心类型定义
│   ├── config.rs        # 配置解析
│   ├── error.rs         # 错误类型(库层)
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

---

## 5. 核心领域类型与数据流

### 5.1 关键类型参照

项目中的核心领域类型定义在 `AETHER_NEXUS_OMEGA_ULTIMATE.md` §10.1:

- `UserIntent` — 多模态用户意图(含 raw_text、multimodal_inputs、risk_level 等)
- `Quest` — 长期任务(含 id、tasks、thinking_mode、checkpoint_id)
- `Checkpoint` — 检查点(含 quest_id、memory_snapshot、wiki_snapshot)
- `OmniSparseMasks` — 全维稀疏掩码(routing/context/memory/audit/budget 五维度)
- `SemanticBlock` — 语义块(含 block_id、block_vector、capability_id)
- `CLV` — 上下文潜在向量(512-dim f32 数组)
- `NexusState` — 全局运行时状态

### 5.2 数据流参考

```
用户输入 → NMC 编码 → Quest 分解 → TTG 切换
    → Parliament 审议 → PVL 生产验证
    → OSA 协调 → KVBSR 路由 → GEA 激活
    → MTPE 多步预测 → GQEP 聚集 → QEEP 纠缠
    → ISCM 更新 → Wiki 沉淀
    → GSOE 进化 → Auto-DPO → Event Bus 广播
```

---

## 6. 架构红线:从四次尸检看向决策

每次做架构/实现决策时,对照以下"尸检教训":

| 问题 | Claude Code 教训 | 本项目红线 |
|------|-----------------|-----------|
| 函数太大? | `print.ts` 3167 行神函数 | **单函数 ≤200 行,超过必须拆模块** |
| 结果丢了? | 5.4% 孤儿调用 | **所有异步操作必须有 GQEP 聚集/超时处理** |
| 裸奔? | 命令插值 + auth 跳过 | **所有外部调用经 SecCore 沙箱 + Decay 衰减** |
| 竞态? | void Promise 无 await | **所有 async 必须 await 或 spawn 管理** |
| 功能乱? | 44 个未发布标志 | **禁止功能标志,用能力场自然进化替代** |
| 内存爆炸? | 1M Token 暴力加载 | **必须经 HCW 分层 + OSA 稀疏化后再加载** |

---

## 7. 开发工作流(项目定制)

### 7.1 日常命令

```powershell
# 快速类型检查(推荐日常使用)
cargo check --workspace

# 只检查单个 crate(修改特定 crate 时)
cargo check -p <crate-name>

# 完整构建
cargo build --workspace

# 运行所有测试
cargo test --workspace

# 单 crate 测试
cargo test -p <crate-name>

# lint + format
cargo clippy --workspace
cargo fmt --all
```

### 7.2 周验收流程

每周结束时,运行:

```powershell
cargo check --workspace  && `
cargo clippy --workspace -- -D warnings && `
cargo test --workspace && `
cargo build --workspace --release
```

全部通过后,参考 8 周推进计划的"验收"条目确认覆盖率等指标。

### 7.3 新建 crate 模板

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

---

## 8. 关键文件索引

| 文件 | 内容 | 重要性 |
|------|------|--------|
| `AETHER_NEXUS_OMEGA_ULTIMATE.md` | 主架构手册(10 章,含 8 周每日计划、25 ADR、核心类型) | ⭐⭐⭐ |
| `AETHER_NEXUS_GEN3_OMEGA.md` | 第三代 10 大魔改创新详解 | ⭐⭐⭐ |
| `AETHER_NEXUS_FULL_DOCUMENTATION.md` | 完整文档汇编 | ⭐⭐ |
| `CODE_WIKI.md` | 代码 Wiki(架构概览、模块职责、核心类型速查) | ⭐⭐⭐ |
| `CHIMERA_NEXUS_COMPLETE_BUILD_GUIDE.md` | 构建指南 | ⭐⭐ |
| `Cargo.toml` | Workspace 根配置(34 members × 20+ 共享依赖) | ⭐⭐⭐ |
| `.trae/specs/*/` | Spec 规范文档 | ⭐⭐ |

---

## 9. 工作时的要求

组建一个由多名拥有 10 年以上行业经验的精英专家级子代理构成的协作团队，以任务优先级为核心指导原则，对各项任务进度实施系统性的分布式深度分析。团队需通过多轮结构化思考、充分探讨及严谨的验证流程，确保对任务的理解全面且准确。在执行阶段，严格按照既定的任务优先级顺序推进实施工作，同时始终秉持长期主义的工作理念，杜绝短期行为和资源过度消耗。特别强调在代码修改过程中，必须保证产出高质量的代码成果，具体标准包括：清晰的代码逻辑结构、高度的代码可读性、完善的注释说明以及符合行业最佳实践的编码规范。在整个任务执行周期内，授权团队调用所有符合任务要求且系统允许的工具资源，包括但不限于 mcp、skills 等相关工具，以保障任务的高效完成和卓越质量。
