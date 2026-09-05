# nuxus规则.md (项目专属规则 · v2.28.0-omega 在途基线)

> **核心位置**:本文件是 Trae/Qoder 多 agent 工具**自动加载**的 `.trae/rules/nuxus规则.md` 路径的**权威规则载体**。原 `AGENTS.md`(项目根目录)为详细全文载体;本文件聚焦"项目代发"所需的**速查 + 决策基线 + 硬约束**,便于智能体快速消费。
>
> **历史溯源**:早期为重定向占位(2026-08-11 之前的 nuxus规则.md 由 AGENTS.md 完全托管);自 2026-08-11 v2.26.0-omega 同步后,本文件升级为**速查 + 基线 + 硬约束**三重定位,与 `AGENTS.md`(全量规则)+ `project_memory.md`(持久记忆)形成"快速 / 详细 / 历史"三层规则体系。
>
> **最后更新**:2026-08-30(v2.28.0-omega **在途**基线同步:Phase 1-5 Ch12 W1-W26 全部收尾 + 5 新 crate(38→43)+ ADR-095~160 治理 + ADR-160 可达性棘轮(28 可达/15 孤岛)+ event_types.rs 镜像退役 + 三轮冗余收敛;最新已发 tag = v2.27.0-omega(v2.27.1-omega 为 CHANGELOG-only 补丁,本地与 origin 均无 tag),v2.28 打 tag 待用户指示;**RL 开发闸门决策持续有效**:Rust-First,Python 侧仅规划)
> **生成方式**:trae-remote-official:staff-engineer-mode + superpowers-main + praxis + brooks-lint + product-lifecycle-workbench 多 agent 工具分布式深度分析

---

## 0. 速查决策表

| 决策点 | 默认值 | 例外条件 | 验证方式 |
|--------|--------|---------|----------|
| 新模块落点 | 既有 crate 子模块 | 经 E01+E04+E05 3 人评审 + ADR 记录 | `scripts/check_doc_consistency.ps1` |
| 跨层依赖方向 | L(N)→L(N-1) 允许 | 向上依赖绝对禁止,跨层仅 Event Bus / MCP Mesh | `scripts/check_dependency_rules.{ps1,sh}` |
| 新事件变体 | append-only Normal 级 | Critical 需 mpsc 旁路 + 治理评审 | types.rs enum 解析 + 事件源审查 |
| 错误处理 | 库层 thiserror / 应用层 anyhow | 无 | `cargo clippy --workspace -D warnings` |
| 性能证据 | criterion benchmark | 任何性能声明必附 bench 数据 | `cargo bench` + `scripts/check_perf_redlines.ps1` |
| unsafe code | `#![forbid(unsafe_code)]` | 绝对禁止(包含 crate 顶层) | grep 全文 `unsafe` |
| 单函数长度 | ≤200 行 | 超必须拆模块 | `scripts/audit_fnlen.py` |
| test timeout | `scaled_timeout!` 宏 | `CHIMERA_TEST_TIMEOUT_SCALE=0.1` 默认 | `cargo test --workspace` |
| Python RL 训练服务 | **仅规划,禁止实施** | Rust 系统彻底成熟稳定运行后开启(RL 开发闸门) | 规则文档 + AGENTS.md + 全部规划文档标注 |

---

## 1. v2.28.0-omega 基线声明(2026-08-30 同步,在途未打 tag)

### 1.1 项目身份

| 字段 | 值 |
|------|-----|
| 项目名 | Chimera CLI |
| 代号 | NEXUS-OMEGA (Omni-Model Engineering Generative Architecture) |
| 根目录 | `D:\Chimera CLI` |
| 技术栈 | Rust 2021 edition · Tokio async · Workspace × **43 crates**(38 基线 + v2.28 新增 L10 `nexus-app-server` + L3 `session-store` + L9 `mas-sched`/`nexus-hook` + L7 `nexus-subagent`) |
| 核心哲学 | **OMEGA 十一定律**: Ω₁-Sparse · Ω₂-Compress · Ω₃-Evolve · Ω₄-Event · Ω₅-Credit · Ω₆-Reuse · Ω₇-Locate · Ω₈-Assess · Ω₉-Preserve · Ω₁₀-Card · Ω₁₁-Synthesize |
| 当前版本 | `v2.28.0-omega`(workspace.package.version,**在途开发,工作区 `feat/phase1-w1-w8`,尚未打 tag**;最新已发 tag v2.27.0-omega(v2.27.1-omega 为 CHANGELOG-only 补丁,本地与 origin 均无 tag);[2.28.1] 在途补丁登记未升 version) |
| 测试规模 | **11564 passed / 0 failed**(2026-08-31 当前工作树全量重测,485 test target;静态 `#[test]` 计数 11433,差值为 doctest+宏展开) |
| crates | **43/43**(零 Stub / 零 `todo!()` 真代码 / 零 `unimplemented!()`;ADR-160 裁定 28 生产可达 + 14 冻结孤岛 + 1 GATED(mca-gateway,ADR-177),**"零 Stub" ≠ "已装配"**) |
| NexusEvent 变体 | **144 个**(types.rs 单表;`event_types.rs` 分层子枚举镜像已按 ADR-160 决策 5 退役删除,分类真值源收敛一处) |
| ADR 数量 | **主编号至 ADR-160**(ADR-001~006 + ADR-026~037 + ADR-042~094 + ADR-095~160 Phase 1-5 治理;Phase 1-5 以四份合并档 095-134/135-144/145-152/153-156 + 单档 132/157/159/160 落档,ADR-158 登记于 phase5 收官报告;权威映射见 `adr_index.md`) |

### 1.1a OMEGA 十一定律(Ω₁~Ω₉ 基座 + Ω₁₀/Ω₁₁ 扩展;权威定义源:`Chimera CLI 十层架构深度打磨与优化方案 最新版.md` §3 + `Chimera CLI 十层架构与算法深度打磨优化方案.md` §2.1;Ω₁₀/Ω₁₁ 见 `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §3.1,收录于 ADR-170)

> ★ Insight: 四定律→九定律→十一定律演进(2026-08-11 全库核验 Ω₁~Ω₉,2026-09-02 ADR-170 收录 Ω₁₀/Ω₁₁):Ω₁~Ω₄ 为架构基座,Ω₅~Ω₉ 为 v2.x 学习/进化体系补齐,Ω₁₀/Ω₁₁ 为经验卡片与按需记忆合成扩展。全部十一定律已有代码落地与 E2E 验证。

| 定律 | 符号 | 含义(设计定义) | 落地 crate / 文件 | 落地状态 |
|------|------|--------------|------------------|---------|
| **Ω-Sparse** | Ω₁ | 策略稀疏性:全维稀疏掩码 + 按需激活 | `osa-coordinator`(五维度掩码) + `sesa-router`(稀疏激活) | ✅ |
| **Ω-Compress** | Ω₂ | 经验压缩:四级窗口 + Mem-π 生成式记忆 | `hcw-window`(4K/32K/128K/1M) + `mlc-engine`(L0-L3 四级记忆) | ✅ |
| **Ω-Evolve** | Ω₃ | 在线策略梯度:AEGIS 四阶段引擎 + 变体隔离 | `gsoe-evolution/aegis` + `chimera-mas`(VariantPool 变体隔离) | ✅ |
| **Ω-Event** | Ω₄ | 经验回放基础设施:Event Bus = 异步 PER 双通道 | `event-bus`(144 事件,broadcast + Critical mpsc) | ✅ |
| **Ω-Credit** | Ω₅ | 信用分配:SHARP Shapley 值精确归因 | `parliament/src/sharp.rs` + `mappo.rs`(三元分解奖励) | ✅ |
| **Ω-Reuse** | Ω₆ | 复用率优先:奖励函数优化技能复用率 | `repo-wiki/skill_graph.rs`(reuse_count) + `csn-substitutor` | ✅ |
| **Ω-Locate** | Ω₇ | 行为定位:L1→L2→L3 自动导航代码修改点 | `parliament/src/critical_path.rs`(关键路径动态识别) | ✅ |
| **Ω-Assess** | Ω₈ | 自我评估:Runtime Auditor 五维度证据纪律 | `efficiency-monitor/src/auditor.rs`(EvidenceGap) | ✅ |
| **Ω-Preserve** | Ω₉ | 保留历史最佳:变体隔离 + 停止策略 | `chimera-mas/src/variant_pool.rs` + `gsoe-evolution` checkpoint | ✅ |
| **Ω-Card** | Ω₁₀ | 经验卡片数据结构:不可变 + 版本化 + append-only 事件流 | `event-bus/src/experience_card_bus.rs` + `nexus-contracts/src/experience_card.rs` + `mlc-engine/src/experience_card_system.rs` + `cmt-tiering/src/experience_card_storage.rs` | ✅ |
| **Ω-Synthesize** | Ω₁₁ | 按需记忆合成算法:懒加载合成 + 不阻塞主流程 + Debug→同错误签名兄弟定向检索 | `mlc-engine/src/on_demand_synthesizer.rs` + `event-bus`(错误签名级检索) | ✅ |

**守恒铁律**:OMEGA 十一定律(Ω₁~Ω₉ 基座 + Ω₁₀/Ω₁₁ 扩展)的 Ω₁~Ω₉ 不可变更,任何演进必须与至少一条定律对齐;新增定律需 ADR 记录(ADR-049 polish-v2.7 + ADR-065~068 治理已覆盖 Ω₅~Ω₉ 落地,ADR-170 收录 Ω₁₀/Ω₁₁)。

### 1.2 三方一致性(权威源:Cargo.toml)

- `Cargo.toml` workspace.package.version = `2.28.0-omega`(在途) ⇔
- `CHANGELOG.md` 最新条目 = `[2.28.1-omega] 2026-08-28 在途补丁登记(未升 version)` ⇔
- `CODE_WIKI.md` / `AGENTS.md` / `.claude/CLAUDE.md` / 本文件 = **43 crates(28 可达/15 孤岛)· 144 NexusEvent(types.rs 单表)· 11564 tests · ADR 主编号至 170**

### 1.3 关键里程碑(v2.20+ 演进链)

| 版本 | 交付 |
|------|------|
| v2.20.0-omega | PROBE HCW-Sparse 深度优化完整闭环(P-1~P3,38 crates,126 NexusEvent,ADR-070/071) |
| v2.21.0-omega | CLI LLM 统一入口(`chimera llm` + `/llm` slash) |
| v2.22.0-omega | MCA token 效率深度优化(coalescing + token_estimate + 亲和缓存) |
| v2.24.0-omega | Phase 9 三环循环元架构重组收尾(P9-T12)+ RUSTSEC-2026-0217/0222/0223 修复 |
| v2.25.0-omega | Milestone B 全部交付 B-1~B-6 + Milestone C R2 解冻前置 + Milestone D RL 全栈三位一体闭环 |
| **v2.26.0-omega** | **Concord TUI 重构 W0~W11 全部收尾**(SlashCommandRegistry 53 命令 + `/` 一级整合 + Chat/Quest 双轨 + ApprovalMode 动态 Shift+Tab + NewlineGate 闸门 + i18n 中英门户 + 10 份 ADR-074~083 落档) |
| **v2.27.0-omega** | **Phase 10 §16 跨层协同闭环审计修复正式发布**(W1-W7 全波次闭环,144 NexusEvent,10836 tests) |
| **v2.27.1-omega** | **GPG 签名补发 + MCA E2E 超时加固**(无功能性变更) |
| **v2.28.0-omega(在途)** | **Phase 1-5 Ch12 W1-W26 全部收尾**(38→43 crates 五新成员 nexus-app-server/session-store/mas-sched/nexus-hook/nexus-subagent;ComputeBridge 双运行时 + 分片总线双跑零 diff ADR-153 Go 全量 B 级 + CausalGraph ADR-132 + 供应商漂移 ADR-154 + 利用率双口径 ADR-157 + payload 双跑 ADR-158;ADR-095~160 治理;ADR-160 可达性棘轮 28/15 + event_types 镜像退役;485 test target 11564 tests;**尚未打 tag**) |
| **v2.28.1-omega(工作区在途)** | **审计遗留修复(fix-audit-followup,2026-08-28)**:B1 freeze_guard 双向接线 R2FreezeRollbackFailed;C1 xts_top_k 收敛红线 #8;E1 bench 三态门禁;14 幽灵事件接线 13+预留 1;FormalVerificationFailed 定稿;ADR-159 登记(未改 workspace.package.version) |

### 1.4 当前焦点(2026-08-30)

- **v2.28.0-omega 在途收口**:Phase 1-5 Ch12 W1-W26 已全部收尾(43 crates · 11564 tests · 144 事件),工作区 `feat/phase1-w1-w8` 尚有未提交改动;**打 tag 待用户指示**,收口前跑全量回归 + clippy + fmt + 依赖铁律双源门禁
- **ADR-160 15 冻结孤岛偿还**:按三条路径(组合根接线 / `optional`+cargo feature / ADR 记录理由)逐步去孤岛;新增不可达 crate 若未登记会让 `check_crate_reachability.sh` 非零退出
- **冗余审计后续**:R1 依赖层 / R2 契约层 / R3 微观逻辑三轮(2026-08-30)已收敛,下一轮 R4 排查跨层语义重复
- **R2 解冻影子期**(≥14 天,ADR-053 rev4 + ADR-054 治理签署,五要素 fail-closed 门禁)
- **ADR-065~068 MCA 体系双轨验证**(`--features mca` 旁路 CI job;mca-gateway 当前为 feature 门控孤岛)
- **★ RL 开发闸门(Rust-First,2026-08-15 治理决策,持续有效)**:现阶段**只做 Rust 侧**;Python 侧(RL 版)训练服务**仅保留规划**(C-4 协议契约 / rl-client-protocol.md 不动,Python 服务实体**禁止实施**);待整个 Rust 系统彻底成熟并稳定运行后(R2 解冻 + 稳定性观察期通过)再开启 RL。所有规划文档已同步标注此状态。
- **历史收编(已完成,留存索引)**:v2.27.0 Phase 10 §16 跨层协同闭环(W1-W7)、v2.27.1 GPG 补发 + MCA E2E 超时加固;v2.28 Phase 1-5 详见 `docs/reports/phase{1..5}-wave*-closure.md` 五份波次收官报告。

---

## 2. 依赖铁律速查(§2.2 完整版见 AGENTS.md)

```
L(N) → L(N)   ✓ 同层互引允许
L(N) → L(N-1) ✓ 向下依赖允许
L(N) → L(N+1) ✗ 向上依赖绝对禁止
L(N) ──event-bus── L(M)  ✓ 跨层通信只能走 Event Bus
L(N) ──mcp-mesh─── L(M)  ✓ 跨进程通信只能走 MCP Mesh
L(N) → L(0)    ✓ L0 Contracts 恒允许(ADR-033)
```

**校验命令**:`pwsh scripts/check_dependency_rules.ps1`(期望 EXIT=0)

---

## 3. 38 Crate 分层映射(L0 Contracts + L1-L10,完整 11 层语义)

```
L0   Contracts ── nexus-contracts                            (纯类型零依赖契约层,ADR-033)
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

**v2.x 关键变更**:
- v2.0.0 L9 新增 `chimera-mas`(ADR-026,35→37 crate)
- v2.4.0 L0 新增 `nexus-contracts`(ADR-033) + L6 新增 `omega-learner`(ADR-031,Bandit 学习)
- v2.22.0 L10 新增 `mca-gateway`(ADR-065,多通道亲和网关,38 crate)
- v2.28.0 38→43 crate:L10 `nexus-app-server`(WI-01)、L3 `session-store`(ADR-141)、L9 `mas-sched`(ADR-145)/`nexus-hook`(ADR-146)、L7 `nexus-subagent`(ADR-148);ADR-160 可达性棘轮 28 生产可达 + 15 冻结孤岛

---

## 4. async 反模式清单(完整 8 条 + Week 1-8 教训,见 AGENTS.md §4.4 / project_memory.md)

1. **禁止持锁跨 `.await`** — DashMap/Mutex 写锁必须在 `.await` 前释放
2. **rusqlite 必须 `spawn_blocking`** — 79 处已包装(repo-wiki / scc-cache)
3. **`tokio::broadcast` 先 subscribe 再 spawn** — 否则事件静默丢失
4. **`with_event_bus(config, bus)` 会 move bus** — subscribe 必须在 with_event_bus 之前
5. **`Arc::new(self.chains.clone())` 创建独立副本** — 必须 `Arc::clone(&self.chains)`
6. **f32 禁止隐式转 f64 比较** — 0.4f32 as f64 精度膨胀,全程保持 f32
7. **`tokio::spawn` fire-and-forget** — 关键路径(衰减循环)必须管理 JoinHandle
8. **`publish_blocking()` 是 sync 方法的正确发布模式** — `tokio::spawn` 在 `#[test]` 无 runtime 会 panic

---

## 5. 架构红线速查(完整 14 条,见 AGENTS.md §6.2 + chimera-auditor)

**P0 阻断级**(违反即阻塞发布):

1. `#![forbid(unsafe_code)]` 缺失(每个 crate 顶层)
2. 单函数 >200 行
3. 持锁跨 `.await`
4. rusqlite 未 `spawn_blocking`
5. `tokio::broadcast` subscribe 在 spawn 之后
6. Critical 事件未走 mpsc 旁路
7. `BudgetExceeded.severity() != EventSeverity::Critical`
8. Top-K 用 `sort_by` 而非 `select_nth_unstable`
9. INV-7 上下文预算界(总 ≤ 117MB)违反
10. INV-8 归档单调性(Hot→Warm→Cold→Ice 禁止回升)违反
11. INV-9 委托图无环(任务委托深度 ≤ 5)违反
12. 跨层依赖方向违规(向上 import)
13. 孤儿调用(异步无 GQEP 聚集/超时)
14. R2 冻结扫描关键词命中(constrained_rl / r2_policy / train_r2 / GsoeAutoDpoRL / evolve_with_constrained_rl)

**Critical 事件清单**(必须 mpsc 旁路;**权威源 `event-bus/src/bus.rs::is_critical_mpsc_event()`,13 个真实 NexusEvent**):
`SkepticVeto` / `RedTeamAudit` / `BudgetExceeded` / `AgentTaskFailed` / `AsaIntervention` / `AffinityQuotaExhausted` / `R2FreezeViolation` / `R2FreezeRollbackFailed` / `FormalViolation` / `VetoOverridden` / `R1ShadowRollbackFailed` / `StopRulingIssued` / `ErrorSignatureMatched`
> 注(2026-08-28 ADR-159 定稿,源冗余审计维度2):`FormalVerificationFailed` 实为 `GsoeError` 的错误变体(gsoe-evolution/src/error.rs:98),**非 NexusEvent**,已从此清单剔除;`VetoOverridden`/`R1ShadowRollbackFailed` 为 Phase 10 Wave 5 双清单对齐补齐,此前文档清单漏列。清单以代码 `is_critical_mpsc_event()` 为唯一事实源,文档改动须同步更新。

---

## 6. UNLEARNABLE_SECURITY_RULES(seccore 6 条不可学习红线)

1. **seccomp 不可降** — 沙箱配置只能更严,不能更松
2. **审计链不可篡改** — Merkle 链只追加,不可修改历史节点
3. **零孤儿不可绕过** — 任何异步操作必须有 GQEP 聚集/超时处理
4. **最小权限底线** — CapabilityToken 默认拒绝,显式 allow
5. **forbid(unsafe) 不可移除** — `#![forbid(unsafe_code)]` 是 crate 顶层强制
6. **BudgetExceeded=Critical 不可降级** — `NexusEvent::severity()` 必须返回 `EventSeverity::Critical`

---

## 7. 9 条不可压缩的工程铁律(Week 1-8 + v2.x 实战)

1. **禁止持锁 .await** — 锁内取快照→释放→await
2. **rusqlite 必须 spawn_blocking** — 79 处已包装
3. **broadcast 先 subscribe 再 spawn** — `bus.subscribe()` 同步调用
4. **BudgetExceeded severity = Critical** — types.rs:1158 权威源
5. **Critical 安全事件用 mpsc** — `Vec<UnboundedSender>` 旁路
6. **禁止 cargo add 不更新 Cargo.lock** — cargo audit 每日扫描
7. **sqlite-vec 禁用** — 违反 forbid(unsafe),改内存 KNN
8. **Top-K 用 select_nth_unstable** — O(n) 替代 O(n log n)
9. **sub-agent 修改代码后必须 `cargo fmt --all` + `--check`** — 与提交前必跑

---

## 8. 关键文件路径速查

| 类别 | 路径 | 用途 |
|------|------|------|
| 规则总览(本文件) | `.trae/rules/nuxus规则.md` | 速查 + 基线 + 硬约束 |
| 详细规则 | `AGENTS.md`(项目根) | 全量规则(10 章 + 附录) |
| 项目特定命令 | `.claude/CLAUDE.md` | 环境/CI/Docker/发布 checklist |
| 持久记忆 | `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI--p2-35a24f2af7eb9ad9ddea\project_memory.md` | Hard Constraints + Lessons Learned |
| 架构权威源 | `docs/architecture/CODE_WIKI.md` | 43 crate 完整索引(§3.11 冻结孤岛清单)+ 144 NexusEvent + ADR-001~160 + 8 专家深度分析 |
| 版本演进权威源 | `CHANGELOG.md` | v1.0.0→[2.28.1-omega] 在途完整历史 |
| 当前基线行数报告 | `docs/reports/project_line_count_report_v2.26.0-omega.md` | 623,344 LOC / 1,870 文件(2026-08-15 实测;competition 3 份已归档至 tmp) |
| 文档一致性巡检 | `scripts/check_doc_consistency.ps1` | 6 类 14 项 EXIT=0 |
| 依赖铁律校验 | `scripts/check_dependency_rules.{ps1,sh}` | L(N)→L(N-1) 验证 |
| 性能红线校验 | `scripts/check_perf_redlines.ps1` | 6 项 SLO + 80% PASS 阈值 |
| 覆盖率门(唯一裁决) | `scripts/coverage_gate.py` + `scripts/coverage_baseline.toml` + `scripts/coverage_per_crate_floor.toml` | 聚合 min/逐 crate floor SSOT;coverage.yml nightly `--check` 消费(方案③趋势门,RK-P45) |
| 可达性棘轮 | `scripts/check_crate_reachability.{py,sh}` + `scripts/crate_reachability_freeze.txt` | 28 可达/14 冻结孤岛+1 GATED(mca-gateway,ADR-177),只减不增 |
| Top-K 例外登记门 | `scripts/check_topk_registry.py` + `scripts/topk_sortby_freeze.txt` | sort_by 例外只减不增(格式/重复/失效/裁决/棘轮),红线 #8 |
| 收口判据清单 | `scripts/gate_manifest.toml` + `scripts/run_gate_manifest.py` | 收口门禁 SSOT(G-01~G-23;light 默认,heavy 磁盘门控) |
| 6 份专家 agent 规则 | `.qoder/agents/chimera-{architect,auditor,perf-engineer,rl-engineer,rust-engineer,security-engineer}.md` | 多 agent 工具自动委派 |
| 2 份 qoder 规则 | `.qoder/rules/{Chimera,Chimera2}.md` | Qoder 工具自动加载 |

---

## 9. Concord TUI 重构关键引用(v2.26.0-omega 新增)

- **SlashCommandRegistry**:`crates/chimera-tui/src/actions/slash_registry.rs`(53 命令注册)
- **`/` 一级整合**:`InputMode::Slash` + SlashCommandSurface(ADR-075)
- **Chat/Quest 双轨会话模式**:`ChatMode`(ADR-076)
- **ApprovalMode 动态 Shift+Tab**:`approval_mode.rs`(ADR-074)
- **NewlineGate 闸门**:`chimera-tui/...`(ADR-078,ttfb 实测 135ns)
- **i18n 中英门户**:`crates/chimera-tui/src/i18n/{en,zh}.rs`
- **Composer 历史三角化**:Composer 上下/删除/持久化(去掉/重做/持久化)
- **10 份 ADR-074~083**:`docs/architecture/ADR-074~083-*.md` 或 `adr_index.md`

**27 面板 FocusManager 同步铁律**(v2.26.0-omega 立规时 22 面板 → Phase10 §15.2b/§15.3 后当前 27,以 `types.rs` REGISTERED_FOCUS_ORDER = PanelId enum 变体数为准):
- 插入新面板必须同步更新 `crates/chimera-tui/src/types.rs` + `app/mod.rs` + `app/tests.rs` + `tests/integration.rs` 四处断言
- `PanelId::next/prev` 循环测试断言需同步维护

---

## 10. 引用机制(三层规则体系)

```
┌──────────────────────────────────────────────────────────────┐
│ .trae/rules/nuxus规则.md (本文件) ─── 速查 + 基线 + 硬约束   │
│ ↓ 详情见 ↓                                                    │
├──────────────────────────────────────────────────────────────┤
│ AGENTS.md(项目根) ─── 全量规则(10 章 + 附录)                  │
│ ↓ 实战教训见 ↓                                                │
├──────────────────────────────────────────────────────────────┤
│ project_memory.md ─── Hard Constraints + Lessons Learned 持久 │
└──────────────────────────────────────────────────────────────┘
```

**引用规则**:
- 速查决策先查本文件(§0 决策表 + §1 基线 + §2 依赖铁律)
- 详细章节查 `AGENTS.md` §X.Y
- 实战教训查 `project_memory.md` Lessons
- **引用前必须验证**(grep/读文件),记忆**会陈旧**(v1.x 的代码行号在 v2.x 后已失效)

---

## 附录 §A · 协作偏好(继承自 AGENTS.md §0)

- **语言**:中文回复(代码标识符、命令、错误信息保持原文)
- **代码风格**:高效、实用,避免过度工程化;清晰逻辑 + 高可读性 + 完善注释
- **解释强度**:写代码前后给出 `★ Insight` 教育性见解
- **决策点**:业务逻辑/错误处理/算法选型时,**邀请用户参与**写关键 5-10 行
- **TDD 守恒**:先写失败测试(或 benchmark)再实现;不允许删除已有测试
- **解释强度**:`★ Insight` 教育性见解

---

## 附录 §B · 工具链速查

```powershell
# 工具链 env 设置
$env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
$env:TMP = 'D:\Chimera CLI\tmp'
$env:TEMP = 'D:\Chimera CLI\tmp'
$env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"

# 快速类型检查
cargo check --workspace

# 全量测试(11564 passed / 0 failed,2026-08-31 当前工作树全量重测,485 test target)
cargo test --workspace

# clippy(Windows OOM 缓解:--jobs 2)
$env:RUST_MIN_STACK = '33554432'; $env:CARGO_INCREMENTAL = '0'
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings

# format
cargo fmt --all -- --check

# 文档一致性巡检(本任务已验证 EXIT=0)
pwsh scripts/check_doc_consistency.ps1

# 依赖铁律校验
pwsh scripts/check_dependency_rules.ps1

# 性能红线校验
pwsh scripts/check_perf_redlines.ps1
```

---

*本速查规则由 trae-remote-official:staff-engineer-mode + superpowers-main + praxis + brooks-lint + product-lifecycle-workbench 多 agent 工具分布式深度分析生成,2026-08-20 同步(基于 v2.27.1-omega 代码库 + 全部 MD 文档 + project_memory 持久记忆)。详细规则请参阅 [AGENTS.md](file:///d:/Chimera%20CLI/AGENTS.md) 与 [project_memory.md](file:///c:/Users/30324/.trae-cn/memory/projects/-d-Chimera-CLI--p2-35a24f2af7eb9ad9ddea/project_memory.md)。*
