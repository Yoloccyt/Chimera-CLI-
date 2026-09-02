---
alwaysApply: true
---
# 全局指令

> 本文件是用户级偏好,适用于所有项目。项目特定的命令、架构约束放在 `<repo>/.claude/CLAUDE.md`,不放这里。

## 协作偏好

- **语言**:全部用中文回复(代码标识符、命令、错误信息保持原文)
- **代码风格**:高效、实用,避免过度工程化与过早抽象，要确保清晰的代码逻辑性和高代码可读性，必要时添加清晰的代码注释，以便于其他开发者理解代码的功能和逻辑。修改代码时避免短视、综合对整套项目系统的理解和影响，保持长期主义，不能竭泽而渔，尤其是要保证修改的是高质量代码（清晰的代码逻辑性和代码的高可读性），给更改的代码添加清晰的代码注释。
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
  > ℹ️ **工具链固化现状(2026-07-22 核实)**:`.cargo/config.toml` **已入库**(固化 `linker=gcc` / `incremental=false` / `SQLITE_ENABLE_FTS5`);而 **`rust-toolchain.toml` 刻意不入库**——它是全局单值配置,在 MSVC 宿主上写 `channel="stable"` 会展开成 MSVC 令 `linker=gcc` 失效,写死 `stable-x86_64-pc-windows-gnu` 又会让 `release.yml` 的 Linux/macOS runner 无法安装该 Windows 宿主工具链而 CI 失败。工具链 channel(GNU)统一由 `install.ps1 -SetupEnv`(项目本地 `rustup default stable-x86_64-pc-windows-gnu`)保证:新克隆者执行一次即可;上述手动 env 块只设变量、**不**设 channel(须另跑 `rustup default`),详见 §10.5 与 `.claude/CLAUDE.md` §1。

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

> ⚠️ **MCA 体系默认未启用**: 需 `cargo build --features mca` 启用 L10 mca-gateway 双轨验证(ADR-065 决策 6,装配期注入,非运行时 flag)。详见 `docs/architecture/adr_index.md` ADR-065/066/067/068。

> 以下规则基于 `AETHER_NEXUS_OMEGA_ULTIMATE.md` 定义的架构,所有决策必须与 **OMEGA 十一定律**(Ω₁-Sparse / Ω₂-Compress / Ω₃-Evolve / Ω₄-Event / Ω₅-Credit / Ω₆-Reuse / Ω₇-Locate / Ω₈-Assess / Ω₉-Preserve / Ω₁₀-Card / Ω₁₁-Synthesize)一致。ADR 编号以 `CODE_WIKI.md §2.3` 为权威源(见 §2.3 调和说明)。Ω₁~Ω₉ 权威定义见 `Chimera CLI 十层架构深度打磨与优化方案 最新版.md` §3(Ω₁~Ω₉ 全部有代码落地,2026-08-11 核验);Ω₁₀/Ω₁₁ 权威定义见 `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §3.1,正式收录于 ADR-170。

---

## 1. 项目全景

### 1.1 身份标识

| 字段 | 值 |
|------|-----|
| 项目名 | Chimera CLI |
| 代号 | NEXUS-OMEGA (Omni-Model Engineering Generative Architecture) |
| 根目录 | `D:\Chimera CLI` |
| 技术栈 | Rust 2021 edition · Tokio async · Workspace × **43 crates**(L0 `nexus-contracts` + L6 `omega-learner` + L10 `mca-gateway` / `nexus-app-server` + L3 `session-store` + L9 `mas-sched` / `nexus-hook` + L7 `nexus-subagent`) <!-- v2.28.0-omega 同步: 2026-08-29,权威源 Cargo.toml workspace.members;计数以 manifest 为准,不以本文档为准 --> |
| 核心哲学 | **OMEGA 十一定律**: Ω₁-Sparse · Ω₂-Compress · Ω₃-Evolve · Ω₄-Event · Ω₅-Credit · Ω₆-Reuse · Ω₇-Locate · Ω₈-Assess · Ω₉-Preserve · Ω₁₀-Card · Ω₁₁-Synthesize |
| 设计来源 | Claude Code 尸检 + Hermes 基因 + Qoder 骨骼 + 五大模型灵魂 |
| 创新总数 | 40+ 个(22 个第一代 + 15 个第三代 + PROBE/MCA/PROBE-Sparse/Token-Eff 等增量) |
| **当前版本** | `2.28.0-omega`(workspace.package.version,权威源 `Cargo.toml`) <!-- v2.28.0-omega 同步: 2026-08-29;CHANGELOG [2.28.1-omega] 为工作区在途补丁登记,未升 version --> |
| **测试规模** | **11564 passed / 0 failed**(debug 全量回归,**2026-08-31 对当前工作树重测**,`--jobs 4` 限流,485 test target / 485 段全 `ok` / 0 panicked;上一登记值 11522 为 2026-08-29 时点,已被 WS-C/D/E 新增测试净增 34;演进 v2.26.0 9954 → v2.27.0 10836 → v2.28.0 11522 → 11556 → 11564;含 Concord TUI **27 面板**(`PanelId` enum 实测 27 变体,旧称 22 为 W0~W6 时点)+ 53 slash commands + 双轨会话 + Phase 10 跨层闭环 + Phase 12 Ch12 波次 1-5 + 5 新 crate 完整链路) <!-- v2.28.0-omega 同步: 2026-08-31 R1-T1 绿证重建 --> |

### 1.2 当前开发阶段

- **阶段**:**第三阶段 — 模块级系统性优化**(v2.0.0-omega 起) <!-- v2.27.1-omega 同步: 2026-08-20 -->
  - **第一阶段**(v1.0.0-omega GA): 8 周推进 + RC,完成 34/34 crate 全覆盖
  - **第二阶段**(v1.1.0–v1.8.0-omega): GA 后演进,创新深化与生态扩展,新增 v1.8.0-omega TUI 企业级套件
  - **第三阶段**(v2.0.0-omega 起): 模块级系统性优化,引入 `chimera-mas` 多 Agent 协同子系统(ADR-026/027/028);Phase 9 三环重组收尾(v2.24.0-omega) + Milestone A v3.0.0 收尾 / Milestone B 差距关闭(v2.25.0-omega) + Milestone C R2 解冻前置 + Milestone D RL 全栈全部就绪;**v2.26.0-omega Concord TUI 重构 W0~W11 全部收尾**(SlashCommandRegistry 53 命令注册 + `/` 一级整合 + Chat/Quest 双轨会话模式 + ApprovalMode 动态 Shift+Tab + i18n 中英门户 + Composer 历史三角化 + 10 份 ADR-074~083 落档);**v2.27.0-omega Phase 10 §16 跨层协同闭环正式发布**(W1-W7 全波次闭环,144 NexusEvent,10836 tests);**v2.27.1-omega GPG 签名补发 + MCA E2E 超时加固**;**v2.28.0-omega(在途,未打 tag)Phase 1-5 Ch12 波次 W1-W26 全部收尾**——ComputeBridge 双运行时 + ShardedBus 分片总线双跑 + CausalGraph 因果归因(ADR-132)+ 供应商漂移守卫 + 利用率双口径(ADR-157)+ payload 级双跑(ADR-158),新增 5 crate(39 `nexus-app-server`/40 `session-store`/41 `mas-sched`/42 `nexus-hook`/43 `nexus-subagent`),ADR-095~160 治理落档,ADR-160 可达性棘轮 + `event_types.rs` 镜像退役;当前 43/43 crate(其中 **28 个生产可达**,15 个为 ADR-160 冻结登记的不可达孤岛;`crates/*/src` 物理行口径,孤岛 08-29 时点 50,604 行=15.2%,经第三轮微观冗余收敛后现值略降,**LOC 为时点点位快照、以 `check_crate_reachability.sh` 输出为准,勿逐值追逐**)
- **实现状态**:43/43 crate 已实现,**零 Stub**;零 `todo!()`/`unimplemented!()`;依赖铁律零违规(`check_dependency_rules.ps1` 与 CI 实际调用的 `.sh` **双源均 EXIT=0**——2026-08-29 审计发现 `.sh` 层图曾滞留 38 导致该 job 为红,已追平并补 Check D);装配面由 ADR-160 可达性棘轮守护(`scripts/check_crate_reachability.sh`,dev-dep 不计入装配面,**"零 Stub" ≠ "已装配"**);error 体系完整分层(库层 thiserror / 应用层 anyhow);RUSTSEC-2026-0217/0222/0223 修复(`tract-onnx 0.22.3` + `wasmtime 47.0.4`;2026-08-31 R1-T7 又按 RUSTSEC-2026-0268/0269 升至 47.0.4)
- **当前焦点**:**v2.28.0-omega 在途(Phase 1-5 W1-W26 已收尾,打 tag 待用户指示;★ tag 事实订正 2026-08-31:`v2.27.1-omega` 本地与 origin 均无 tag,实际最新已发 tag = `v2.27.0-omega`(bcf9d4d),v2.27.1 为 CHANGELOG-only 补丁)** — ① 工作区 `feat/phase1-w1-w8` 未提交改动收口 + 全量回归门禁后决定 v2.28.0 tag;② ADR-160 15 冻结孤岛按三条偿还路径(组合根接线 / optional+feature / ADR 记录)逐步去孤岛;③ 冗余审计 R1/R2/R3(2026-08-30)与 R4(2026-08-31)的后续接线——性能红线静态门与双跑比对已入 CI(R1-T5/T6),`ignored` 归属门已登记 76 条,`unignore-target` 7 条待解除;④ R2 解冻影子期(≥14 天,ADR-053 rev4 + ADR-054 治理签署)+ ADR-065/066/067/068 MCA 体系双轨验证(`--features mca`);⑤ **★ RL 开发闸门(Rust-First,2026-08-15 治理决策,持续有效):现阶段只做 Rust 侧;Python 侧(RL 版)训练服务仅保留规划(C-4 协议契约不动,Python 服务实体禁止实施),待 Rust 系统彻底成熟并稳定运行后(R2 解冻 + 稳定性观察期通过)再开启 RL**
- **参照**:`CHANGELOG.md` v1.0.0→[2.28.1-omega] 在途完整汇总(含 v2.20.0 PROBE / v2.21.0 CLI LLM / v2.22.0 MCA token / v2.24.0 Phase 9 / v2.25.0 Milestone B / v2.26.0 Concord TUI / v2.27.0 Phase 10 / v2.28.0 Phase 1-5 Ch12 治理 八大版本章节) + §3.4 第三阶段开发规则 + `docs/reports/milestone-{A,B,C,D}-execution-report.md` 四份阶段执行报告 + `docs/reports/phase{1,2,3,4,5}-wave*-closure.md` 五份 Ch12 波次收官报告 + `docs/tui/tui-refactor-implementation-plan.md` Concord TUI 重构实施计划 + `docs/reports/phase10-cross-layer-closure-report.md` Phase 10 收编报告 + `docs/reports/redundancy-R{1,2,3}_2026-08-30.md` 三轮冗余审计

### 1.3 核心术语速查

| 缩写 | 全称 | 对应 crate |
|------|------|-----------|
| Ω-Sparse | 全维稀疏(工具/上下文/记忆/审计/预算) | `osa-coordinator` |
| Ω-Compress | 经验压缩(四级窗口 + Mem-π 生成式记忆) | `hcw-window` + `mlc-engine` |
| Ω-Evolve | 在线进化(AEGIS 四阶段引擎 + 变体隔离) | `gsoe-evolution` |
| Ω-Event | 事件驱动(Event Bus = 异步 PER 双通道) | `event-bus` |
| Ω-Credit | 信用分配(SHARP Shapley 值精确归因) | `parliament` |
| Ω-Reuse | 复用率优先(奖励函数优化技能复用率) | `repo-wiki` + `csn-substitutor` |
| Ω-Locate | 行为定位(L1→L2→L3 自动导航修改点) | `parliament` |
| Ω-Assess | 自我评估(Runtime Auditor 五维度证据纪律) | `efficiency-monitor` |
| Ω-Preserve | 保留历史最佳(变体隔离 + 停止策略) | `chimera-mas` + `gsoe-evolution` |
| Ω-Card | 经验卡片数据结构(不可变 + 版本化 + append-only 事件流) | `event-bus` + `nexus-contracts` + `mlc-engine` + `cmt-tiering` |
| Ω-Synthesize | 按需记忆合成算法(懒加载 + 非阻塞 + 错误签名定向检索) | `mlc-engine` + `event-bus` |
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
| CAF | Channel Affinity Framework (渠道亲和,不对单一模型适配) | `model-router` |
| MAS-Q | Multi-Agent System Quadrant (四象限稳定分工) | `chimera-mas` |
| PDCA | Plan-Do-Check-Act (持续改进闭环) | `efficiency-monitor` |

---

## 2. 十层架构与依赖规则

### 2.1 分层映射(L0→L10)

> 权威源:`CODE_WIKI.md §2.1`。注意 `AETHER_NEXUS_OMEGA_ULTIMATE.md §3.1` 描述的 L0-L10(11 层)为早期设计,已废弃,以本表 10 层为准(加 L0 Contracts 实际为 11 层语义但 10 层架构不变,ADR-033 决议)。

```
L0   Contracts ─ nexus-contracts                       (纯类型零依赖契约层,ADR-033)
L10  Interface ── chimera-cli · chimera-tui · chtc-bridge · mcp-mesh · csn-substitutor · mca-gateway · nexus-app-server
L9   Quest ───── quest-engine · gea-activator · efficiency-monitor · chimera-mas · mas-sched · nexus-hook
L8   Parliament ─ parliament · acb-governor · decb-governor
L7   Execution ── pvl-layer · gqep-executor · mtpe-executor · ssra-fusion · nexus-subagent
L6   Router ───── osa-coordinator · kvbsr-router · faae-router · sesa-router · omega-learner
L5   Knowledge ── repo-wiki · gsoe-evolution · auto-dpo
L4   Security ─── seccore · qeep-protocol · decay-engine
L3   Storage ──── scc-cache · lsct-tiering · cmt-tiering · session-store
L2   Memory ───── nmc-encoder · hcw-window · mlc-engine
L1   Core ─────── nexus-core · event-bus · model-router
```

> **v2.0.0-omega 变更**:L9 新增 `chimera-mas`(多 Agent 协同子系统,ADR-026),第 35 个 crate,与 10 个现有 crate 复用(80% 能力复用,Part II append-only 闭环见 ADR-028)。
> **v2.4.0-omega 变更**:L0 新增 `nexus-contracts`(纯类型零依赖契约层,ADR-033);L6 新增 `omega-learner`(LinUCB Bandit 在线学习路由器,ADR-031+ADR-043,R2 冻结)——第 37 个 crate,六接缝异步下发 SelectorPolicy::Learned。
> **v2.22.0-omega 变更**:L10 新增 `mca-gateway`(多通道亲和网关,ADR-065)——第 38 个 crate,流式数据面走 bounded mpsc 不进 event-bus(ADR-065 决策 4)。
> **v2.28.0-omega 变更**:38 → 43 crate,新增 L10 `nexus-app-server`(第 39,WI-01 宿主协议门面)、L3 `session-store`(第 40,ADR-141 append-only 会话事件流)、L9 `mas-sched`(第 41,ADR-145 调度控制面)、L9 `nexus-hook`(第 42,ADR-146 生命周期 Hook)、L7 `nexus-subagent`(第 43,ADR-148 类型化子代理 + Task Auction);可达性 ADR-160 棘轮:28 生产可达 + 15 冻结孤岛。

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

---

## 3. 当前发布阶段感知

### 3.1 RC 阶段规则(历史归档)

> ⚠️ **本章为 v1.0.0-omega GA 发布前的临时规则**,已于 2026-06-28 完成使命。第一阶段(8 周推进 + RC)已交付 34/34 crate 全覆盖,自 v1.0.0-omega 起进入 GA 后演进。**当前阶段请参考 [§3.4 第三阶段开发](#34-第三阶段开发模块级系统性优化)**。

### 3.2 8 周推进历史

8 周推进计划已全部完成,作为历史归档移到 **附录 §A.1**。当前不再作为决策依据,仅作回顾参考。

### 3.3 第二阶段开发(GA 后演进 — v1.1.0~v1.8.0)

> **阶段定义**:v1.0.0-omega GA 发布后至 v1.8.0-omega 的持续演进阶段。项目从"功能完成 + 稳定发布"转向"创新深化 + 生态扩展"。**本阶段已于 v1.8.0-omega 收尾**(TUI 企业级套件上线),自 v2.0.0-omega 起进入第三阶段(§3.4)。本节规则仍然适用,但**当前开发主参考是 [§3.4](#34-第三阶段开发模块级系统性优化)**。

#### 3.3.1 开发原则

1. **OMEGA 十一定律守恒** — Ω₁-Sparse / Ω₂-Compress / Ω₃-Evolve / Ω₄-Event / Ω₅-Credit / Ω₆-Reuse / Ω₇-Locate / Ω₈-Assess / Ω₉-Preserve / Ω₁₀-Card / Ω₁₁-Synthesize 不可变更,任何演进必须与之对齐
2. **依赖方向不可逆** — 遵循 §2.2 依赖铁律,跨层通信只走 Event Bus / MCP Mesh,向上依赖禁止
3. **TDD 守恒** — 新特性必须先写失败测试再实现;不允许删除已有测试
4. **领域类型稳定性** — 核心领域类型(`UserIntent`/`Quest`/`Checkpoint`/`OmniSparseMasks`/`CLV`/`NexusState`)变更需 ADR 记录
5. **向后兼容** — GA 后 API 变更须遵循 SemVer,破坏性变更需 major 版本升级
6. **新 crate 准入** — 第二阶段允许新建 crate,但必须先更新 `CODE_WIKI.md §3.1` 索引并经 ADR 审批

#### 3.3.2 主要参考资料(互补分工)

第二阶段开发以下列两份文档为主要参考,覆盖"如何搭"与"如何进化"两个维度:

| 文档 | 角色 | 版本 | 适用场景 |
|------|------|------|---------|
| `AETHER_NEXUS_OMEGA_从零搭建完全指南.md` | **工程实施主参考**(如何搭) | v2.0.0-omega | 新 crate 搭建、模块从零实现、架构全貌理解、搭建步骤参考 |
| `OMEGA_大模型架构魔改创新_AI_Agent项目套用设计.md` | **创新演进主参考**(如何进化) | v3.0.0-omega | 创新点演进、五大模型理念融合、魔改架构深化、学术支撑引用 |

#### 3.3.3 引用说明与实施指导

**从零搭建完全指南(v2.0.0-omega)** — 第二阶段工程实施主参考:

- ⚠️ **历史偏差**:文档中"37 crates 骨架"数量为早期估计,实际为 35 crate(第二阶段核对时;v2.26.0-omega 当前为 **38**,以 `Cargo.toml` workspace.members 为权威)
- ⚠️ **版本漂移**:文档基于 v1.x 设计,部分 crate 名称/层级已演进(v2.0.0-omega 新增 `chimera-mas` 至 L9),以 `CODE_WIKI.md §3` crate 索引为权威(当前为 43 crate:28 生产可达 + 15 ADR-160 冻结孤岛)
- **适用章节**(作为历史理解参考,新工作以 v3 终极文档 + 模块优化分析报告为准):
  - §5 OMEGA 十层架构详解 → 理解分层设计原理
  - §7 核心模块从零实现 → 模块实现模式参考
  - §8 12 周推进计划 → 历史归档,不作决策依据(以本规则 §A.1 为准)
- **实施指导**:阅读此文档理解早期设计脉络;新工作请参考 [§3.4](#34-第三阶段开发模块级系统性优化) 的 v3/v4 文档

**OMEGA架构魔改创新(v3.0.0-omega)** — 创新演进主参考:

- **学术支撑**:20+ 篇 2025-2026 顶会论文(NeurIPS/ICLR/arXiv),创新点有理论根基
- **适用章节**:
  - §3 十二大魔改创新架构 → 创新点深化与演进方向
  - §4 项目实践中的具体套用 → 架构魔改落地参考
  - §6 附录:架构决策记录 → ADR 补充参考(以 `CODE_WIKI.md §2.3` 为权威源)
- **实施指导**:规划新创新特性时,先核对本文档的十二大魔改创新是否已覆盖,避免重复设计;演进现有创新点时参考五大模型(DeepSeek V4 / Kimi K2.7 / GLM 5.2 / Minimax M3 / Qwen 3.7 Plus)理念映射

#### 3.3.4 第二阶段开发检查清单

- [ ] 新特性是否与 OMEGA 十一定律(Ω₁-Sparse / Ω₂-Compress / Ω₃-Evolve / Ω₄-Event / Ω₅-Credit / Ω₆-Reuse / Ω₇-Locate / Ω₈-Assess / Ω₉-Preserve / Ω₁₀-Card / Ω₁₁-Synthesize)对齐?
- [ ] 是否查阅了从零搭建指南的对应模块实现(§7 核心模块从零实现)?
- [ ] 是否查阅了 OMEGA架构魔改的对应创新点(§3 十二大魔改创新架构)?
- [ ] 依赖方向是否遵守 §2.2 铁律(L(N)→L(N-1) 允许,L(N)→L(N+1) 禁止)?
- [ ] 是否先写失败测试再实现(TDD 守恒)?
- [ ] 核心 API 变更是否有 ADR 记录?
- [ ] 新 crate 是否已更新 `CODE_WIKI.md §3.1` 索引?

### 3.4 第三阶段开发(模块级系统性优化 — v2.0.0 起,**当前阶段**)

> **阶段定义**:v2.0.0-omega 及之后的深度优化与模块级系统化阶段。在第二阶段(GA 后演进)完成创新深化与生态扩展的基础上,第三阶段聚焦"模块级系统性优化 + 性能极致调优 + 算法演进 + 学术支撑落地"。8 位虚拟领域专家(E01-E08)分布式深度分析 L1-L10 各层,输出 P0-P4 优先级评估与实施路线图。<!-- v2.26.0-omega 同步: 2026-08-11 -->
>
> **关键里程碑**:
> - v2.0.0-omega:`chimera-mas` 多 Agent 协同子系统(ADR-026,35 crate 全覆盖)
> - v2.1.0-omega:四象限稳定分工 + WSJF 调度(ADR-027)
> - v2.2.0-omega:Part II 七项闭环能力补齐(ADR-028,INV-7/INV-8 不变量)
> - v2.3.0-omega:Phase A 架构审计 + Phase B TUI 收尾 + Phase C 治理规范化
> - v2.4.0-omega:P5 进化闭环(NEXUS-OMEGA v5.0,37 crate,109 NexusEvent,KPI-01 100% / KPI-02 0% / KPI-03 708ns / KPI-04 44.38µs 全部达标)
> - v2.5.0–v2.13.0-omega:9 版本迭代,补齐 MCA PANTHEON / 9 层防御 / 协调度量 / 三角色审议 / 形式化验证器 M0/M1/M2
> - v2.14.0–v2.19.0-omega:P2 Sprint 14 项任务全部完成(测试计数算术修正/workspace deps/MTPE 真实预测/entangle 基准/感知器升级/MCA/efficiency-monitor/CMT decay/lru eviction 全部交付)
> - v2.20.0-omega:PROBE HCW-Sparse 深度优化完整闭环(P-1~P3 全部阶段验收,38 crate,126 NexusEvent,ADR-070/071 落档)
> - v2.21.0-omega:CLI --help 规整化 + LLM 统一入口(`chimera llm` 一级子命令 + `chimera help` EXAMPLES + 6 维 doctor + `/llm` slash command)
> - v2.22.0-omega:MCA token 效率深度优化(coalescing 合并 + token_estimate + 亲和缓存 + `vllm-example.toml` 部署样例,tag 已于 2026-08-07 发布,章节补录)
> - v2.24.0-omega:Phase 9 三环循环元架构重组收尾发布(P9-T12)+ RUSTSEC-2026-0217/0222/0223 修复(`tract-onnx 0.22.3` + `wasmtime 47.0.4`;2026-08-31 R1-T7 又按 RUSTSEC-2026-0268/0269 升至 47.0.4)+ 12 处 ignored doctest 修复 + `check_perf_redlines.ps1` µs 解析 bug 修复 + Test (ubuntu-latest) job 解决
> - **v2.25.0-omega:Milestone B 全部交付 B-1~B-6** — RL 共享类型补齐(ADR-049 漂移关闭) + Ambient Mode(后台常驻守护循环,新增 `ResourceRecovered`) + 九层防御三补齐(SkillGraph 安全约束 + 回放池完整性审计 + 行为契约强制层 `FormalViolation`) + PlatformGroundingSpec(平台接地 5 类要求 + RuntimeAuditor 第 0 维) + Agent Grep CLI(`chimera grep <pattern>` 双通道检索) + 关键路径动态识别(六风险因子规则综合,`parliament/critical_path.rs`)
> - **Milestone C**:R2 解冻前置全就绪(C-1 RewardSpec 统一奖励框架 + C-2 GSOE Week-7 TODO 闭合 + C-3 rl-client 通道 + C-4 Python 训练服务契约 + C-5 R2UnfreezeReadiness 六要素 fail-closed 门;**9699 passed / 0 failed**)
> - **Milestone D**:RL 全栈 + 三位一体闭环 Rust 侧全部落地(D-2a DQN 记忆迁移 + D-2b RLSecurityPolicy + D-2c/d GTPO + RLVR + D-2e 八维度奖励接入 + D-3 评估-进化-协同闭环 E2E;**9744 passed / 0 failed**)
> - **v2.26.0-omega**:Concord TUI 重构 W0~W11 全部收尾 —— SlashCommandRegistry 53 命令注册(ADR-075) + `/` 一级整合 + Chat/Quest 双轨会话模式(ADR-076) + ApprovalMode 动态 Shift+Tab(ADR-074) + NewlineGate 闸门(ADR-078) + i18n 中英门户 + Composer 历史三角化 + 10 份 ADR-074~083 落档;**9954 passed / 0 failed**(chimera-tui 1387/0 + workspace 9659 + doctest 295)
> - **v2.27.0-omega**:Phase 10 §16 跨层协同闭环正式发布(W1-W7:经验卡片组合根 + Quest 生命周期桥 + 卡片生成触发点 + 事件协议补齐 + mpsc 双清单对齐 + 合成闭环 + 奖励缺口);NexusEvent 136→144;**10836 passed / 0 failed**;ADR-085 双态收编
> - **v2.27.1-omega**:GPG 签名补发 + MCA E2E 超时加固(无功能性变更)
> - **v2.28.0-omega(在途,未打 tag)**:Phase 1-5 Ch12 波次 W1-W26 全部收尾 —— ComputeBridge 双运行时 + ShardedBus 分片总线双跑(零 diff 后 Go 全量 B 级)+ CausalGraph 因果归因(ADR-132)+ 供应商漂移守卫(ADR-154)+ 利用率双口径三条件判定(ADR-157,combined 0.552→0.999)+ payload 级双跑(ADR-158,13/13);新增 5 crate(39~43,38→43);ADR-095~160 治理;ADR-160 生产可达性棘轮(28 可达/15 孤岛)+ event_types.rs 镜像退役;三轮冗余收敛(R1 依赖层/R2 契约层/R3 微观逻辑);**11522 passed / 0 failed**(485 test target);[2.28.1-omega] 为在途补丁登记未升 version

#### 3.4.1 开发原则

1. **OMEGA 十一定律守恒** — Ω₁-Sparse / Ω₂-Compress / Ω₃-Evolve / Ω₄-Event / Ω₅-Credit / Ω₆-Reuse / Ω₇-Locate / Ω₈-Assess / Ω₉-Preserve / Ω₁₀-Card / Ω₁₁-Synthesize 不可变更,任何优化必须与之对齐
2. **依赖方向不可逆** — 遵循 §2.2 依赖铁律,跨层通信只走 Event Bus / MCP Mesh,向上依赖禁止
3. **TDD 守恒** — 优化必须先写失败测试(benchmark)再实现;不允许删除已有测试
4. **领域类型稳定性** — 核心领域类型(`UserIntent`/`Quest`/`Checkpoint`/`OmniSparseMasks`/`CLV`/`NexusState`)变更需 ADR 记录
5. **向后兼容** — API 变更须遵循 SemVer,破坏性变更需 major 版本升级
6. **性能可证伪** — 任何性能优化必须有 `criterion` benchmark 证据,不接受主观判断
7. **学术支撑落地** — 优化建议必须有学术论文(NeurIPS/ICLR/arXiv)或工业尸检证据
8. **专家团队评审** — 重大优化需经 8 位专家(E01-E08)分布式评审,优先级评估 P0-P4

#### 3.4.2 主要参考资料(互补分工)

第三阶段开发以下列两份文档为主要参考,覆盖"工程实施升级"与"模块级优化"两个维度:

| 文档 | 角色 | 版本 | 适用场景 |
|------|------|------|---------|
| `docs/architecture/AETHER_NEXUS_OMEGA_从零搭建终极文档_v3.md` | **工程实施升级参考**(如何搭 v3) | v3.0.0-omega | 6 源尸检 + OMEGA 十层架构 + 核心模块从零实现 + 测试策略 + 安全模型 |
| `docs/architecture/AETHER_NEXUS_OMEGA_模块级系统性优化分析报告.md` | **模块优化主参考**(如何优化) | v4.0.0-omega | L1-L10 各层算法优化 + 跨层协同 + 实施路线图 + 8 专家评审 |

> ✅ **文档位置与状态**(2026-08-11 复核):两份第三阶段参考文档**均已存在于** `docs/architecture/`——`AETHER_NEXUS_OMEGA_从零搭建终极文档_v3.md` 与 `AETHER_NEXUS_OMEGA_模块级系统性优化分析报告.md` 齐备;同目录 `CODE_WIKI.md`(架构权威源,38 crate)/ `INDEX.md` / `ADR-026~028` / `ARCHITECTURE_HEALTH_AUDIT_v2.2.0-omega.md` / `DEEP_RESEARCH_*.md` 亦在,`docs/CONVENTIONS.md` 与 `docs/audit/dimension_f_security.md` 同样存在。唯一仍缺失的是 `AETHER_NEXUS_OMEGA_ULTIMATE.md`(已被 `CODE_WIKI.md` + ADR-026~028 取代,无需恢复)。版本演进仍以 `CHANGELOG.md`(v1.0.0→v2.26.0-omega)为权威源。

#### 3.4.3 引用说明与实施指导

**从零搭建终极文档(v3.0.0-omega)** — 工程实施升级参考:

- **与 v2.0.0-omega 关系**:v3.0.0-omega 是 v2.0.0-omega"从零搭建完全指南"的升级版,综合了 6 个工业级系统尸检(Claude Code/Hermes/Qoder/OpenCode/PI/Codex) + 第三代 OMEGA 架构 + 大模型魔改创新 + 50+ 学术论文
- **查重声明**:所有核心术语与架构组合查重率 < 15%,属首次在 AI Coding Agent CLI 语境定义
- **适用章节**:
  - §2 六源尸检与基因融合 → 理解设计来源与基因组合
  - §4 OMEGA 十层架构详解 → 分层设计原理(升级版,比 v2.0.0-omega 更全面)
  - §5 核心模块从零实现 → 新 crate 实现模式参考(升级版)
  - §7 测试策略与验收标准 → 测试覆盖率与验收标准
  - §8 安全模型与合规映射 → 安全模型设计参考
- **实施指导**:搭建新 crate 或重构现有 crate 时,先查 `CODE_WIKI.md §3.1` 确认层级归属,再参考本手册 §5 核心模块从零实现;理解架构设计原理时参考 §4(比 v2.0.0-omega 更全面);遇到与 v2.0.0-omega 描述冲突时以 v3.0.0-omega 为准

**模块级系统性优化分析报告(v4.0.0-omega)** — 模块优化主参考:

- **分析日期**:2026-07-09(基于 v3.0.0-omega 快照)
- **专家团队**:8 位虚拟领域专家(E01 首席架构师 / E02 安全架构师 / E03 记忆系统专家 / E04 路由算法专家 / E05 生产系统专家 / E06 认知科学专家 / E07 任务调度专家 / E08 前端与交互专家,各 10+ 年经验)
- **优先级评估**:P0 阻断级 / P1 核心级 / P2 优化级 / P3 增强级 / P4 维护级
- **学术支撑**:Token Budgets (Khan, 2026), SpecSA (arXiv:2605.19893), PiKV (arXiv:2508.06526), Zero-trust LLM Agents (Kushnerov, 2026), GraphBit (arXiv:2605.13848), Self-Organizing MAS (Lyu, 2026) 等 60+ 篇论文
- **适用章节**:
  - §1 专家团队组建与优先级评估体系 → 优化优先级判定方法
  - §2-§10 L1-L10 各层优化 → 对应层优化参考(逐层深入)
  - §11 跨层协同优化 → 跨层依赖与协同优化
  - §12 实施路线图与验证报告 → 优化实施路线图与验证方法
- **实施指导**:优化特定层时参考对应章节(如优化 L4 安全层参考 §3 L4 安全层优化);规划优化路线图时参考 §12;优先级判定参考 §1.2 P0-P4 评估体系;涉及跨层协同时参考 §11

#### 3.4.4 第三阶段开发检查清单

- [ ] 优化是否与 OMEGA 十一定律(Ω₁-Sparse / Ω₂-Compress / Ω₃-Evolve / Ω₄-Event / Ω₅-Credit / Ω₆-Reuse / Ω₇-Locate / Ω₈-Assess / Ω₉-Preserve / Ω₁₀-Card / Ω₁₁-Synthesize)对齐?
- [ ] 是否查阅了终极工程手册(v3.0.0-omega)的对应模块实现(§5 核心模块从零实现)?
- [ ] 是否查阅了模块级优化分析报告(v4.0.0-omega)的对应层优化章节?
- [ ] 优化是否有 `criterion` benchmark 证据(性能可证伪)?
- [ ] 优化建议是否有学术论文或工业尸检证据(学术支撑落地)?
- [ ] 重大优化是否经 8 位专家(E01-E08)分布式评审?
- [ ] 依赖方向是否遵守 §2.2 铁律(L(N)→L(N-1) 允许,L(N)→L(N+1) 禁止)?
- [ ] 是否先写失败测试(benchmark)再实现(TDD 守恒)?
- [ ] 核心 API 变更是否有 ADR 记录?

#### 3.4.5 前瞻性架构参考(第三阶段演进方向)

> 以下 5 份根目录设计文档为第三阶段模块级优化的**前瞻性参考**,涵盖多 Agent 四象限协同、全球大模型亲和、三重悖论免疫、元架构重组四大演进方向。这些文档为**设计蓝图**而非已实施代码,引用时需区分"设计目标"与"当前实现"。

| 文档 | 角色 | 核心价值 | 落地状态(2026-08-09 更新) |
|------|------|---------|---------|
| `CHIMERA_MULTI_AGENT_四象限协同工作系统_系统性设计文档.md` | **MAS-Q 设计蓝图**(v1.0) | 三层递归委托 + 四象限稳定分工 + 5 治理 + PDCA + 6 维质量模型;Part II 7 项闭环能力(上下文隔离/复杂度分块/记忆归档/知识协同) | ✅ **`chimera-mas` 全栈已实现**(v2.0.0-omega ADR-026 ~ v2.2.0-omega ADR-028 闭环);四象限、INV-7 上下文预算界、INV-8 归档单调性、任务委托深度 ≤ 5、INV-9 委托图无环 已生效;`docs/reports/milestone-C-execution-report.md` 章节 1 验证 |
| `CHIMERA_NEXUS_OMEGA_完整对话摘要.md` | **项目演进全景记录** | 6 轮深度迭代(架构理论→工程实现→优化分析→TUI 设计→多 Agent→模型亲和),427,000+ 字,是理解项目设计脉络的**首要入口** | ✅ 元文档,记录已发生过程;v2.26.0-omega 阶段建议追加第五/六轮迭代(Milestone A/B/C/D + Concord TUI 全链路) |
| `CHIMERA_全球大模型亲和系统_终极设计文档.md` | **CAF 渠道亲和设计**(v6.0.0-omega) | 4 大渠道(Quality/Balanced/Cost/Speed) × 15+ 厂商 × 60+ 模型 × 地域路由;核心理念:不对单一模型适配,对所有模型做渠道亲和 | ✅ **MCA 体系已落地**(v2.21.0-omega `chimera llm` 入口 + v2.22.0-omega token 效率深度优化 + v2.25.0-omega `chimera grep` 双通道 + 43 crate 中 `mca-gateway` 8 个 affinity profile);MCA 实现已推进至 v2.28.0-omega(ADR-065~068 + ADR-160 标注其为 feature 门控孤岛,默认 binary 不含,经 `--features mca` 装配),设计 v6.0.0-omega 增量仍按 ADR-065 决策 6 装配期注入路径补齐 |
| `AI_Agent_三重悖论_x_Chimera_深度映射分析.md` | **三重悖论免疫分析** | 记忆悖论(静态稀疏掩码→幽灵记忆)、推理悖论(10 层协调成本→分布式单体)、进化悖论(验证器层级 3→需跃迁至层级 4 形式化验证);**13+3 条新工程铁律** | ✅ **形式化验证器 M0/M1/M2 全部落地**(v2.13.0~v2.20.0-omega,7+1+1=9 属性),L4 验证器跃迁完成;R2 解冻前置就绪,影子期等待治理签署(ADR-053 rev4) |
| `三环循环_十层接口_元架构重组深度分析.md` | **元架构重组方案** | 内环(9 crate:记忆+推理+进化,共享内存<1ms) + 外环(25 crate,事件驱动向后兼容);诊断 4 类病理(星型耦合/跨层渗透/循环依赖/L1 上帝 crate) | ✅ **Phase 9 全部交付**(v2.24.0-omega P9-T1~T12),7 份报告归档于 `docs/architecture/_blueprints/three-ring-reorg/`;ADR-054 三环重组决策 3/6 全部落档;依赖铁律实况 0 违规 |

**三重悖论三条核心红线(第三阶段架构决策必查)**:

1. **记忆悖论红线**:OSA 静态稀疏掩码不能替代 MemCon 式自适应记忆控制;固定 top-k 相似度召回在任务阶段切换时会产生"幽灵记忆"(新旧事实共存无法区分时间有效性);记忆策略必须随任务阶段自适应(MinimalRecall→StandardTopK→QueryReformulation→AggressivePruning)
2. **推理悖论红线**:10 层架构的跨层协调成本存在"推理悖论阈值"——当协调成本超过推理增益时,多 Agent 反而不如单 Agent;Parliament 的 SkepticVeto 机制可能被策略性利用(推理越强,绕过安全护栏的能力越强);需定期测量"协调成本/推理增益"比值
3. **进化悖论红线**:当前 GSOE/AutoDPO 使用执行反馈(测试通过/失败)作为验证信号,属于验证器层级 L3(执行反馈),存在被"奖励黑客"游戏化风险;第三阶段需向 L4(形式化验证)或 L5(人类研究判断)跃迁

**三环重组核心约束(前瞻)**:

- 内环 9 crate(mlc-engine/hcw-window/nmc-encoder/quest-engine/parliament/gea-activator/gsoe-evolution/auto-dpo/repo-wiki)使用共享内存+直接调用,延迟 <1ms
- 外环 25 crate 保持事件驱动,只能通过 `CoreLoopEvent` 与内环通信
- **关键约束**:内环 crate 不能依赖外环 crate;外环 crate 只能通过 event-bus 依赖内环公开接口
- (v2.20 诊断时点,38 crate)架构曾存在 4 类病理:星型耦合(L6 Router 5 crate 都依赖 osa-coordinator)、跨层渗透(L7 gqep-executor 依赖 L4 qeep-protocol)、隐式循环依赖(L5 GSOE ↔ L9 Quest 通过 Event Bus 逻辑循环)、L1 上帝 crate(nexus-core/event-bus 被所有 crate 依赖)。**处置状态**:v2.24.0-omega Phase 9(P9-T1~T12)已完成三环重组收尾——星型耦合经 L0 `nexus-contracts` 共享类型解耦、2 条生产违规边(quest→decb 上提、parliament→seccore 事件化)消除、D3/D4 病理消解;至 v2.28(43 crate)依赖铁律 `.ps1/.sh` 双源 EXIT=0,残余跨层渗透仅余 ADR-048 显式豁免的 gqep→qeep 一条

---

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
- 应用层错误用 `anyhow::Result<T>`,库层用自定义 `thiserror` enum(39 个 error.rs 全部 thiserror,含 `chimera-mas` 的 `MasError` 33 变体 + 2 象限约束变体;`nexus-contracts` 为纯类型契约层,错误类型内嵌无独立 error.rs;v2.28 新增 `session-store`/`mas-sched` 各带 error.rs)
- 避免 `unwrap()`/`expect()` — 所有可能失败的边界必须用 `?` 或 `match` 处理(ttg.rs 7 处 expect 已修复为 `unwrap_or_else`)
- 避免 `Box<dyn Trait>` — 优先使用 `impl Trait` 或 `enum dispatch`(chtc-bridge 5 IDE 适配器用 enum dispatch)
- **所有 crate 必须 `#![forbid(unsafe_code)]`** — crate 级属性,只约束当前 crate 源码,不传播到依赖(rusqlite bundled / prometheus-client 内部 unsafe 不影响当前 crate)
- **Top-K 选择必须用 `select_nth_unstable` (O(n))** — 禁止 `sort_by` (O(n log n)) 做 Top-K
- **proptest 1.11+ 用 block-named 语法** — `fn test_name(x in 0..100u32) { ... }`,closure 形式某些 pattern 解析失败
- **并发收集用 `FuturesUnordered`** — 优于 `join_all`,减少内存占用,支持流式结果

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

event-bus 定义 144 个 `NexusEvent` 变体(types.rs 单表,metadata() 负责分类;event_types.rs 镜像已按 ADR-160 决策 5 退役),关键 Critical 级事件(必须用 mpsc channel 确保送达):
- `SkepticVeto` / `RedTeamAudit` / `AsaIntervention` / `BudgetExceeded` / `AgentTaskFailed`

> 完整事件清单见 `crates/event-bus/src/types.rs`。`BudgetExceeded` 的 `severity()` 必须 = `EventSeverity::Critical`(C2 修复,2026-06-25;代码权威源 `classification.rs:46`——NexusEvent::severity() 综合 match)。`AgentTaskFailed` 自 v2.0.0-omega 起亦为 Critical(§6.2 红线,走 mpsc 旁路通道)。

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
| **BudgetExceeded severity = Critical** | C2 修复 | 禁止降级,`NexusEvent::severity()` 必须返回 `EventSeverity::Critical`(`classification.rs:46`) |
| **Critical 安全事件用 mpsc** | efficiency-monitor | SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded 必须用 mpsc channel 确保送达 |
| **禁止 cargo add 不更新 Cargo.lock** | audit.yml | `cargo audit --deny unmaintained --deny unsound` 每日扫描,依赖漂移阻塞 CI |
| **sqlite-vec 禁用(违反 forbid unsafe)** | ADR-005 降级 | sqlite-vec 0.1.9 binding 需 unsafe,改内存 KNN(10-1000 entry scale) |
| **Top-K 用 select_nth_unstable** | Engineering Convention | O(n) 替代 O(n log n) sort_by |

> 完整 60+ 条 Week 1-8 Lessons 见 `project_memory.md`(引用机制 §10.4)。

---

## 7. 开发工作流(项目定制)

### 7.1 日常命令

```powershell
# 工具链 env 设置见 §工作目录与平台(全局指令)或 .claude/CLAUDE.md §1
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

# lint(clippy OOM 已知问题,用 --jobs 2 缓解)
$env:RUST_MIN_STACK = '33554432'; $env:CARGO_INCREMENTAL = '0'
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings

# format
cargo fmt --all

# 压力测试(#[ignore] 标记的重测试)
cargo test -- --ignored --test-threads=1 --nocapture
```

> ⚠️ **clippy OOM 根因**:Windows `STATUS_STACK_BUFFER_OVERRUN (0xC0000409)` 实际是 `__fastfail` 的 `FAST_FAIL_FATAL_APP_EXIT`(P9=7),objdump 定位到 `std::alloc::rust_oom`,是 OOM 非栈溢出。`--jobs 2` 是最优缓解(44% 快于 `--jobs 1`)。

### 7.2 发布前检查清单(替代周验收)

```powershell
# 1. 类型 + lint + format
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings
cargo fmt --all -- --check
#    ⚠ Windows 本机 `cargo fmt --all` 会因长路径前缀报 `os error 206`(2026-08-31 实测),
#      此时逐包聚合同等验:对 43 个 crate + 根 package 依次 `cargo fmt -p <pkg> -- --check`,
#      任一非 0 即未通过。不可因跑不动而跳过本项(HEAD 实测曾漂到 14/44 包不干净)。

# 2. 全量测试
cargo test --workspace --jobs 4
#    ⚠ 16 核宿主上默认并行度会 OOM(实测 `memory allocation ... failed` + 派生的
#      E0786 invalid metadata,2026-08-31),看起来像代码回归实为编译面爆内存在;
#      故必须限流 `--jobs 4`(仍 OOM 递减至 2),并把 OOM 与回归分开判定。
cargo test --workspace --release -- --ignored --test-threads=1 --nocapture   # 压测/性能红线(SLO 阈值在 debug 下会假失败)

# 3. 安全审计(audit.yml 每日跑,发布前手动确认)
#    5 个 ignore 为经评估确认不影响项目的间接依赖(详见 audit.yml 注释)
#    ⚠ 必须与 audit.yml:61 逐字一致:自 P5-T5(2026-08-28)起用
#      `--deny unmaintained --deny unsound` 替代 `--deny warnings`(后者会被
#      RUSTSEC-2024-0436 一类 warning 噪声直接卡死,反而诱使放宽整体门禁)。
cargo audit --deny unmaintained --deny unsound `
  --ignore RUSTSEC-2026-0190 `
  --ignore RUSTSEC-2026-0002 `
  --ignore RUSTSEC-2024-0436 `
  --ignore RUSTSEC-2025-0141 `
  --ignore RUSTSEC-2025-0119


# 3a. audit 口径一致性自证（ADR-167 决策 4：CI 与本清单、CLAUDE.md 三处必须逐字等价）
#     目标文件缺失时退 2（不可判定），绝不静默放过——.md 不入库，CI 里跑它没有意义
python scripts/audit_cmd_sync.py
# 3b. 性能红线与 bench 清单静态门(R1-T5 起为 CI check job 阻塞门,本地同口径)
bash scripts/check_perf_redlines.sh --selftest
bash scripts/check_perf_redlines.sh --static-only

# 4. fuzz(本地 Windows GNU 无法跑,委托 Linux CI)
#    见 .github/workflows/fuzz.yml,tag 推送后自动触发

# 5. Docker 镜像验证(release.yml docker job)
docker pull ghcr.io/<owner>/chimera-cli:<tag>
docker run --rm ghcr.io/<owner>/chimera-cli:<tag> --version
#    期望输出: ^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+

# 6. 镜像体积 < 100MB
docker image inspect <image> --format '{{.Size}}' | awk '{print $1/1024/1024 " MB"}'

# 7. release 构建
cargo build --workspace --release
#    binary 体积 < 50MB(strip + panic=abort + opt-level=z + lto + codegen-units=1)

# 8. tag 推送(触发 release.yml + fuzz.yml)
git tag v<x.y.z>-omega
git push origin v<x.y.z>-omega
```

### 7.3 新建 crate 模板

> RC 阶段不再新建 crate。历史模板见附录 §A.2。

---

## 8. 关键文件索引

### 8.1 核心文档

> ✅ **2026-07-21 复核**:`docs/architecture/` 已含 **29 份 `.md` 文档**——`CODE_WIKI.md`(架构权威源) / `INDEX.md` / `ADR-026~028` / `ARCHITECTURE_HEALTH_AUDIT_v2.2.0-omega.md` / `DEEP_RESEARCH_*.md` / `从零搭建终极文档_v3.md` / `模块级系统性优化分析报告.md` 均在;`docs/CONVENTIONS.md` 与 `docs/audit/dimension_f_security.md` 亦在。**唯一缺失**的是 `AETHER_NEXUS_OMEGA_ULTIMATE.md`(已被 `CODE_WIKI.md` + ADR-026~028 取代,无需恢复)。`CODE_WIKI.md` 为架构权威源,`CHANGELOG.md` 为版本演进权威源。

| 文件 | 内容 | 状态 | 重要性 |
|------|------|------|--------|
| `CHANGELOG.md` | **版本演进权威源** — v1.0.0→v2.26.0-omega 完整历史(含 Week 1-8 推进 + GA 后多阶段 + Concord TUI 重构 + ADR 摘要) | ✅ 存在 | ⭐⭐⭐ |
| `docs/architecture/README.md` | 架构文档权威索引(7 分类,子文档均存在;已同步至 43 crate) | ✅ 存在 | ⭐⭐ |
| `docs/tui/README.md` | Chimera TUI 用户手册(实际版本 v2.26.0-omega,Concord TUI 重构 W0~W11 收尾) | ✅ 已同步 | ⭐⭐ |
| `docs/grafana/README.md` + `dashboard.json` | Grafana 监控面板导出 | ✅ 存在 | ⭐ |
| `docs/architecture/CODE_WIKI.md` | **架构权威源** — 代码 Wiki(架构概览/模块职责/核心类型/43 crate 索引(§3.11 冻结孤岛清单)/ADR-001~160) | ✅ 存在 | ⭐⭐⭐ |
| `docs/architecture/INDEX.md` | 7 分类文档索引 | ✅ 存在 | ⭐⭐ |
| `docs/architecture/AETHER_NEXUS_OMEGA_ULTIMATE.md` | 原架构手册(10 章 + 25 ADR + 8 周计划);§3.1 层级映射已废弃,内容已被 CODE_WIKI + ADR-026~028 覆盖 | ❌ 不存在(已被取代,无需恢复) | — |
| `docs/architecture/AETHER_NEXUS_OMEGA_从零搭建终极文档_v3.md` | v3.0.0-omega 工程手册(6 源尸检 + 50+ 论文) | ✅ 存在 | ⭐⭐ |
| `docs/architecture/AETHER_NEXUS_OMEGA_模块级系统性优化分析报告.md` | v4.0.0-omega 模块优化报告(8 专家 P0-P4) | ✅ 存在 | ⭐⭐ |
| `docs/architecture/ADR-026-chimera-mas-subsystem.md` | MAS 子系统决策(8 项决策;CHANGELOG v2.0.0 亦复述) | ✅ 存在 | ⭐⭐ |
| `docs/architecture/ADR-027-chimera-mas-quadrant.md` | 四象限决策(6 项决策;CHANGELOG v2.1.0 亦复述) | ✅ 存在 | ⭐⭐ |
| `docs/architecture/ADR-028-chimera-mas-part2-closure.md` | Part II 闭环决策(8 项决策;CHANGELOG v2.2.0 亦复述) | ✅ 存在 | ⭐⭐ |
| `docs/architecture/ARCHITECTURE_HEALTH_AUDIT_v2.2.0-omega.md` | v2.2.0-omega 架构健康度审计(35 crate / 10 层) | ✅ 存在 | ⭐ |
| `docs/architecture/DEEP_RESEARCH_*.md` | 优化算法 / LLM 架构映射深研报告(基于 Week 2 快照,部分已演进) | ✅ 存在 | ⭐ |
| `docs/CONVENTIONS.md` | 根目录白名单规范 | ✅ 存在 | ⭐ |
| `docs/audit/dimension_f_security.md` | 安全审计维度文档 | ✅ 存在 | ⭐ |
| `Cargo.toml` | Workspace 根配置(**43 members**,含 L0 `nexus-contracts`、L6 `omega-learner`、L10 `mca-gateway`/`nexus-app-server`、L3 `session-store`、L9 `mas-sched`/`nexus-hook`、L7 `nexus-subagent`;根 package `chimera-e2e-tests` 承载 34 个 E2E/安全/压测/控制闭环 test target) | ✅ v2.28.0-omega | ⭐⭐⭐ |
| `README.md` | 项目入口 | ✅ 存在 | ⭐⭐ |
| `.trae/rules/nuxus规则.md` | 本文件(全局指令 + 项目专属规则) | ✅ v2.26.0-omega 同步 | ⭐⭐⭐ |
| `.claude/CLAUDE.md` | 项目特定命令(环境/CI/Docker/发布 checklist) | ✅ v2.26.0-omega 同步 | ⭐⭐⭐ |
| **🆕 根目录前瞻设计文档(5 份)** | | | |
| `CHIMERA_MULTI_AGENT_四象限协同工作系统_系统性设计文档.md` | MAS-Q 设计蓝图(四象限 + 三层委托 + 治理 PDCA) | ✅ 存在 | ⭐⭐⭐ |
| `CHIMERA_NEXUS_OMEGA_完整对话摘要.md` | 项目 6 轮演进全景记录(427,000+ 字) | ✅ 存在 | ⭐⭐⭐ |
| `CHIMERA_全球大模型亲和系统_终极设计文档.md` | CAF 渠道亲和框架(4 渠道 × 60+ 模型) | ✅ 存在 | ⭐⭐⭐ |
| `AI_Agent_三重悖论_x_Chimera_深度映射分析.md` | 三重悖论免疫分析(13+3 新铁律) | ✅ 存在 | ⭐⭐⭐ |
| `三环循环_十层接口_元架构重组深度分析.md` | 内环/外环元架构重组方案 | ✅ 存在 | ⭐⭐⭐ |

> **现状**:上述架构文档(CODE_WIKI / INDEX / ADR-026~028 / ARCHITECTURE_HEALTH_AUDIT / DEEP_RESEARCH / v3 工程手册 / 优化报告)均已存在于 `docs/architecture/`;`CHANGELOG.md` v2.0.0~v2.3.1 章节仍是 ADR 决策的完整复述,可与 ADR 文件互为对照。唯 `AETHER_NEXUS_OMEGA_ULTIMATE.md` 已被 CODE_WIKI + ADR-026~028 取代,不再恢复。

<!-- v2.26.0-omega 同步: 2026-08-11(Concord TUI 重构 W0~W11 收尾) — 当前版本 v2.26.0-omega;38 crate(含 L0 nexus-contracts @ L0 / omega-learner @ L6 / mca-gateway @ L10);CHANGELOG 含 v2.20.0 PROBE / v2.21.0 CLI LLM / v2.22.0 MCA token / v2.24.0 Phase 9 / v2.25.0 Milestone B / v2.26.0 Concord TUI 六大版本章节;9954 passed / 0 failed(workspace 全量);`docs/architecture/` 子文档经复核**均已存在**,仅 ULTIMATE.md 缺失且已被取代 -->
<!-- v2.27.1-omega 同步: 2026-08-20(Phase 10 §16 跨层协同闭环发布收编 + GPG 补发) — 当前版本 v2.27.1-omega;38 crate;CHANGELOG 含 v2.27.0 Phase 10 / v2.27.1 GPG 七版本章节;144 NexusEvent · 10836 passed / 0 failed(2026-08-19 workspace 全量实测) -->
<!-- 历史同步: v1.7.0-omega 同步 2026-07-15;2026-07-21 复核订正 — 早期快照曾误记 docs 缺失,经复核 `docs/architecture/` 29 份文档齐备(仅 ULTIMATE.md 缺失,已被取代),CODE_WIKI.md 为架构权威源 -->

### 8.2 工程基建

| 文件 | 内容 | 重要性 |
|------|------|--------|
| `.github/workflows/audit.yml` | 每日 cargo audit + PR 触发(改 Cargo.lock) | ⭐⭐⭐ |
| `.github/workflows/release.yml` | tag 触发:5 平台 matrix build + test + docker(GHCR + 100MB + --version grep) + release | ⭐⭐⭐ |
| `.github/workflows/fuzz.yml` | tag/手动触发:nightly + cargo-fuzz 6 target × 300s(委托 Linux CI) | ⭐⭐⭐ |
| `Dockerfile` | 多阶段:rust:1-slim-bookworm builder + distroless/cc-debian12 runtime + nonroot + HEALTHCHECK + RUST_BACKTRACE=1 | ⭐⭐⭐ |
| `install.ps1` / `install.sh` | 跨平台安装脚本(SHA256 校验 + PATH 注入 + --version 验证) | ⭐⭐ |
| `test_version_verification.ps1` | 本地模拟 CI --version grep 校验(24 测试用例) | ⭐⭐ |
| `fuzz/Cargo.toml` | 独立 fuzz package(隔离 workspace,cargo-fuzz metadata) | ⭐⭐ |
| `.gitignore` | 覆盖 target/ + target_clippy*/ + .toolchain/ + tmp/ + .env* + *.pem | ⭐⭐⭐ |

### 8.3 测试与审计

| 文件 | 内容 | 重要性 |
|------|------|--------|
| `tests/e2e/*.rs` | 32+ 个 E2E 测试 + 1 安全 + 2 压测 = **35+ 个 test target**(week5-8 主流程 + 安全 + 集成 + 压测 + 验收 + quest_lifecycle + full_integration + stress_test + tui_control_loop + formal_verifier + trinity_loop + tui_handshake + rhi_cg + r2_freeze + runtime_auditor + variant_governance + token_efficiency + mca_quota_switch 等,Cargo.toml [[test]] 注册) | ⭐⭐⭐ |
| `tests/security/owasp_top10.rs` | OWASP A01-A10 渗透测试(零信任白名单 + Merkle 审计链) | ⭐⭐⭐ |
| `tests/stress/week7_stress.rs` | 1000 次压测(Arc 探针 + 延迟稳定性) | ⭐⭐ |
| `fuzz/fuzz_targets/*.rs` | 6 个 fuzz target(quest_parse/seccore_sandbox/event_serialize/cacr_budget_parse/checkpoint_deserialize/config_section_parse) | ⭐⭐ |
| `crates/*/benches/*.rs` | 42+ criterion benches(v1.x 26 + v2.x 新增 mlc_l2_knn/wiki_knn@1000/wiki_knn@10/50agent_mem_peak 等) | ⭐⭐ |
| `docs/audit/dimension_f_security.md` | 安全审计维度文档 | ⭐⭐ |

### 8.4 规则与命令

| 文件 | 内容 | 重要性 |
|------|------|--------|
| `.trae/rules/nuxus规则.md` | 本文件(全局指令 + 项目专属规则) | ⭐⭐⭐ |
| `.claude/CLAUDE.md` | 项目特定命令(CI 触发 / Docker / fuzz 委托 / 发布 checklist) | ⭐⭐⭐ |

---

## 9. 工作时的要求

组建一个由多名拥有 10 年以上行业经验的精英专家级子代理构成的协作团队，以任务优先级为核心指导原则，对各项任务进度实施系统性的分布式深度分析。团队需通过多轮结构化思考、充分探讨及严谨的验证流程，确保对任务的理解全面且准确。在执行阶段，严格按照既定的任务优先级顺序推进实施工作，同时始终秉持长期主义的工作理念，杜绝短期行为和资源过度消耗。特别强调在代码修改过程中，必须先多方面思考清楚后再修改代码，必须保证产出高质量的代码成果，具体标准包括：清晰的代码逻辑结构、高度的代码可读性、杜绝冗余的代码、完善的注释说明以及符合行业最佳实践的编码规范。在整个任务执行周期内，授权团队调用所有符合任务要求且系统允许的工具资源，包括但不限于 mcp、skills 等相关工具，以保障任务的高效完成和卓越质量。

---

## 10. 发布与运维

### 10.1 CI/CD 准入门槛

| Workflow | 触发 | 关键 job | 准入门槛 |
|----------|------|---------|---------|
| `audit.yml` | 每日 UTC 02:00 + PR 改 Cargo.lock | cargo audit | `--deny unmaintained --deny unsound` 0 退出 |
| `release.yml` | tag `v*.*.*-omega` | build(5 平台) + test + docker + release | build/test/docker 全 pass 才能 release |
| `fuzz.yml` | tag + workflow_dispatch | fuzz(ubuntu-latest + nightly,6 target × 300s,已与 fuzz/Cargo.toml 同步) | crash 上传(90 天留存),非阻塞 |

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
- 本地:`fuzz/Cargo.toml` 静态核验(独立 workspace 隔离 + `[package.metadata] cargo-fuzz = true` + 6 个 [[bin]] 声明),通过 `cargo check --manifest-path fuzz/Cargo.toml` 验证语法
- CI:`fuzz.yml` ubuntu-latest + nightly + matrix 6 target × 300s(quest_parse / seccore_sandbox / event_serialize / cacr_budget_parse / checkpoint_deserialize / config_section_parse)
- cargo-audit:本地网络超时时手动检查 Cargo.lock 13 个关键依赖版本
- 静态核验脚本:`scripts/check_fuzz_config.{ps1,sh}`(8 项检查) + `scripts/verify_docker_locally.{ps1,sh}`(Docker 三级降级)

### 10.3a Commit Message UTF-8 教训(v2.3.0-omega → v2.3.1-omega 关键经验)

> **根因**:PowerShell 沙箱在 Windows 默认 GBK 编码下传输含中文的 commit message 到 GitHub,导致仓库记录乱码。v2.3.0-omega 的 `chore(release): v2.3.0-omega ????` 永久保留。
>
> **避坑**:
> 1. `git commit -m` 用 HEREDOC 传入(避免命令行解析)
> 2. 中文 commit message 前用 `$env:GIT_AUTHOR_ENCODING = 'utf-8'` 强制编码
> 3. tag 推送后如发现 commit message 损坏,不要 `--amend`(历史已锁定),**新 tag 递增 patch 版本**触发新 workflow
> 4. 参考 v2.3.1-omega 处理:同一 commit 内容 + `version` 递增 → 新 tag → 正常触发 release.yml
>
> **CI 检测**:`release.yml` workflow 应在 job 内 echo commit message 用于人工核验,异常时手动修复。

### 10.4 project_memory 引用机制

本规则只摘录 **Hard Constraints 摘要**(§6.2 8 条核心新红线)与 **async 反模式清单**(§4.4 8 条)。**完整 Lessons 集合**保留在:

```
c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md
```

**Lessons 范围演进**:
- Week 1-8(2026-06):第一阶段 8 周推进 + RC 实战,60+ 条核心 Lessons
- v1.x(2026-06~07):GA 后演进补充(8 条 async 反模式 / Top-K select_nth_unstable / sqlite-vec 禁用等)
- v2.x(2026-07~至今):第三阶段补充(4 象限不变量 / Part II 闭环 / Commit Message UTF-8 / `MasError` 变体演进等)

**引用规则**:
- 遇到 async 死锁 / broadcast 丢事件 / SQLite 阻塞 / fuzz 失败 / 多 Agent 协同不变量等问题,先查 `project_memory.md` 是否有历史教训
- 引用记忆前必须验证(grep/读文件),记忆**会陈旧**(v1.x 的代码行号在 v2.x 后已失效)
- 新教训产生时,先更新 `project_memory.md`,再评估是否提炼进本规则 §6.2 / §4.4

### 10.5 已知基建短板(待修复)

| 短板 | 影响 | 优先级 | 状态 |
|------|------|--------|------|
| `.cargo/config.toml` 已入库(linker配置) | ✅ 2026-06-29 已修复,linker已配置 | P0 | ✅ 已完成 <!-- verified: 2026-07-21 --> |
| ~~`rust-toolchain.toml` 已入库~~ **刻意不入库**(2026-07-22 订正) | 早期误记为"已入库",实际全仓库无此文件。全局单值配置会破坏跨平台 CI(MSVC 宿主展开为 MSVC 令 `linker=gcc` 失效;写死 GNU 三元组令 `release.yml` 的 Linux/macOS runner 失败)。channel 改由 `install.ps1 -SetupEnv` 的项目本地 `rustup default stable-x86_64-pc-windows-gnu` 保证(权威源:install.ps1 的 `Set-Environment` + `.claude/CLAUDE.md` §1) | P0 | ✅ 已解决(方案=install.ps1 -SetupEnv,非入库文件) <!-- corrected: 2026-07-22 --> |
| `target_clippy*/` 残留 | ✅ 2026-06-29 已清理(核验无残留) | P0 | ✅ 已完成 <!-- verified: 2026-07-21 --> |
| release 镜像未设 `RUST_BACKTRACE=1` | ✅ 2026-06-29 已修复(Dockerfile 加 ENV RUST_BACKTRACE=1) | P1 | ✅ 已完成 <!-- verified: 2026-07-21 --> |
| figment 三源已声明但无 `*.yaml` 配置样例 | ✅ 2026-06-29 已补齐(examples/config.sample.{yaml,toml}) | P2 | ✅ 已完成 <!-- verified: 2026-07-21 --> |
| 环境变量(CARGO_HOME/PATH)仍需手动设置 | ✅ 2026-06-29 已改进:`install.ps1 -SetupEnv` 一步注入 env + 设 GNU channel(PowerShell 参数为单破折号 `-SetupEnv`) | P1 | ✅ 已完成 <!-- verified: 2026-07-22 --> |
| D盘空间管理(回收站黑洞/应用商店缓存) | 后台下载+未清空回收站可导致磁盘满;2026-07-15 实测 D 盘满(274.71GB/0.01GB 剩余),回收站 127.93 GB(46.6%)+联想系缓存 68.76 GB;清理脚本 scripts/cleanup_disk_space.ps1 已创建(Diagnose/SafeClean/ProjectClean 三模式);沙箱限制需在系统 PowerShell 手动清理 | P1 | ⚠️ 需定期清理(回收站清空+联想系目录删除需用户手动执行) <!-- verified: 2026-07-21 -->
| C盘空间管理(AppData膨胀/休眠文件/重复工具链) | AppData 62.86 GB + hiberfil.sys 6.08 GB + C:\Users\30324\.rustup 4.27 GB(历史残留,D 盘工具链已验证完整);`powercfg /hibernate off`(管理员)可释放 6 GB;删除 C:\chimera-test-target(2.44 GB)+C:\chimera-target(0.29 GB)需手动执行 | P1 | ⚠️ 需定期清理 <!-- verified: 2026-07-21 --> |
| `fuzz.yml` CI matrix 滞后 | fuzz/Cargo.toml 声明 6 个 target,fuzz.yml 已同步至 6 target | P2 | ✅ 已完成 <!-- verified: 2026-07-21 --> |
| `fuzz/src/lib.rs` stub 宏缺失 | Windows-GNU stub 宏方案已实现(fuzz/src/lib.rs + 条件编译 import + [lib] 声明),`cargo check --manifest-path fuzz/Cargo.toml` 通过 | P3 | ✅ 已完成 <!-- verified: 2026-07-21 --> |
| `scripts/check_fuzz_config.{ps1,sh}` 缺失 | 已实现,8 项静态检查(metadata/lib/target-dep/文件数/[[bin]]/stub 宏/条件编译) | P3 | ✅ 已完成 <!-- verified: 2026-07-21 --> |
| `scripts/verify_docker_locally.{ps1,sh}` 缺失 | 已实现,三级降级验证(Docker→Podman→静态检查+binary体积代理+CI引导) | P3 | ✅ 已完成 <!-- verified: 2026-07-21 --> |
| **`docs/architecture/` 子文档**(2026-07-21 复核订正) | 早期快照曾误记为缺失;经复核 `CODE_WIKI.md` / `INDEX.md` / `ADR-026~028` / `ARCHITECTURE_HEALTH_AUDIT` / `DEEP_RESEARCH_*` / `从零搭建终极文档_v3` / `模块级优化报告` / `docs/CONVENTIONS.md` / `docs/audit/dimension_f_security.md` **均已存在**(共 29 份 .md);唯 `AETHER_NEXUS_OMEGA_ULTIMATE.md` 缺失,已被 CODE_WIKI + ADR-026~028 取代 | P4 | ✅ 已复核存在 <!-- corrected: 2026-07-21 --> |
| **🆕 v2.3.0-omega commit message 编码损坏**(PowerShell 沙箱 GBK,2026-07-21 发现) | `e19e280` commit message 永久保留乱码(`chore(release): v2.3.0-omega ????`);**修复路径见 §10.3a**:新 tag 递增 patch(v2.3.0 → v2.3.1)触发新 release workflow,**禁止 `--amend` / `push --force`** | P1 | ⚠️ v2.3.1-omega 已补救 <!-- found: 2026-07-21 --> |

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
