# ADR-049: chimera_ultimate_polish_v2.7 方案裁剪总决策(17 组件三档分级)

> **⚠️ RL 开发闸门(2026-08-15 治理决策)**: 现阶段**只做 Rust 侧**;Python 侧(RL 版)训练服务**仅保留规划**(本文档所述协议/方案/规划内容保留为设计资产),Python 服务实体**禁止实施**;待整个 Rust 系统彻底成熟并稳定运行后(R2 解冻 + 稳定性观察期通过)再开启 RL。权威源: `.trae/rules/nuxus规则.md` §1.4 / `AGENTS.md` §1.2 当前焦点。

> **状态**: Accepted (2026-07-28)
> **决策日期**: 2026-07-28
> **关联 ADR**: ADR-042(R2 冻结) / ADR-043(R1 影子模式) / ADR-033(L0 契约层) / ADR-034(灰度=能力场+编译期 feature) / ADR-047(FormalVerifier 路线图)
> **关联文档**: `chimera_ultimate_polish_v2.7.md`(方案本体) / `docs/architecture/IMPLEMENTATION_PLAN_polish_v2.7.md`(实施计划)
> **权威源**: 本 ADR 是 polish v2.7 方案 17 个新组件落地方式的唯一裁决记录

---

## 1. 背景

### 1.1 方案来源

`chimera_ultimate_polish_v2.7.md`(2598 行,自称 v3.1.0-omega AEGIS 融合版)基于九篇
Harness Engineering 论文(郝建业/CMU/RUC/北大/jcode/腾讯/快手/Qoder/小米)提出:

- OMEGA 四定律扩展为八定律(新增 Ω₅ Credit / Ω₆ Reuse / Ω₇ Locate / Ω₈ Assess)
- 17 个新增/强化组件(runtime-auditor / aegis-engine / variant-pool / process-score 等)
- 五阶段路线图(Phase 0 自我评估 → Phase 4 验证,Week 1-8)
- Rust-Python gRPC 隔离训练架构 + 八维度奖励函数体系

### 1.2 基线对账(2026-07-28 实测)

方案文档头部声称的基线与代码库实际状态存在多处失真,实施前完成对账:

| 项 | 方案声称 | 代码库实测 | 处置 |
|---|---|---|---|
| 版本 | v2.4.0-omega | git tag `v2.4.0-omega` 已发布,但 `Cargo.toml` 仍为 `2.3.1-omega`(发版漏 bump) | 对账至 `2.4.1-omega` 起点 |
| crate 数 | 37 | ✅ 37(nexus-contracts L0 → omega-learner L6) | 一致 |
| NexusEvent | 104 | **107**(脚本实测 `pub enum NexusEvent` 变体数) | 以 107 为准 |
| ADR 数 | 41 | 43(最大编号 ADR-048) | 新 ADR 从 ADR-049 起 |
| 测试规模 | 3690+ | 数千量级自洽(nmc-encoder 117 + seccore 175 等) | 不阻塞 |

### 1.3 核心冲突:方案 RL 全栈设计 vs ADR-042 R2 冻结

方案 §5-§13 大量出现 DQN/PPO/GRPO/MAPPO 在线训练、反向传播设计,与既有治理红线直接冲突:

- **ADR-042**:R2(GSOE×AutoDPO 约束 RL)在 FormalVerifier 落地前**无条件冻结**,
  有 `tests/e2e/r2_freeze_violation_e2e.rs` 守护测试 + CI 关键词扫描
  (`constrained_rl` / `r2_policy` / `train_r2` / `GsoeAutoDpoRL` / `evolve_with_constrained_rl`,大小写不敏感)
- **ADR-043**:新学习策略必须经 CapabilityToken::Provisional 影子模式 2 周验证
- **ADR-031**:学习永不在关键路径,omega-learner 接缝模式为唯一学习通道

## 2. 决策

### 决策 1:三档裁剪框架

17 组件按「本期落地 / 降级实现 / 推迟」三档分级,以 **0 个新 crate + 既有 crate 子模块**
方式落地(方案原设计"每组件一 crate"将使 workspace 暴涨至 54 成员,否决):

| 组件 | 裁决 | 落点 |
|---|---|---|
| runtime-auditor | ✅ 本期 | `efficiency-monitor/src/auditor.rs` |
| 五维评估面板 + dag-viz-panel | ✅ 本期 | `chimera-tui` 新 Panel |
| rl-types(RLState/VariantId/ProceduralBlueprint/BehaviorContract) | ⚠️ 部分落地(修订) | `nexus-contracts` 新模块；VariantId/VariantContract/BehaviorContract/ProceduralBlueprint 已落地(2026-07-28)，**RLState/RLAction/RLExperience 未落地**(全仓库 0 匹配,2026-08-07 复核)——修订裁决为 v3.0.x 补齐(见 §6 修订记录) |
| variant-pool + Task Router | ✅ 本期 | `parliament/src/{variant_pool,variant_review}.rs`(规则路由非 RL) |
| process-score + hint-recovery | ✅ 本期 | `pvl-layer/src/{process_score,hint_recovery}.rs` |
| aegis-engine | ⚠️ 降级 | `gsoe-evolution/src/aegis/`(规则/统计驱动,无 RL 参数更新) |
| PER 优先级回放 | ⚠️ 降级 | `omega-learner/src/per_buffer.rs`(SumTree,数据仅供 R1) |
| 分层回放池 | ⚠️ 降级 | `cmt-tiering/src/rl_replay_pool.rs` |
| mem-pi / memory-sideagent / memory-graph | ⚠️ 降级 | `mlc-engine` 子模块 + omega-learner S8 接缝 |
| skill-graph / procedural-blueprint / behavior-localization / agent-grep | ⚠️ 降级 | `repo-wiki` 子模块 |
| auto-builder | ⚠️ 降级 | pvl-layer producer 扩展(VerifyAgent 骨架复用 seccore Sandbox) |
| meta-agent-adapter | ⚠️ 降级 | gsoe-evolution spec_loader 扩展 |
| rl-client(gRPC)+ Cross-Harness GRPO + Python 训练服务 | ❌ 推迟 v3.x | 前置 = ADR-047 FormalVerifier + R2 解冻程序 |
| MAPPO/SHARP/RLSecurityPolicy/DQN 迁移/弱模型验证 | ❌ 推迟 v3.x | 神经网络训练 + 外部资源依赖 |

### 决策 2:OMEGA 八定律采纳声明

采纳方案的八定律扩展作为**设计哲学层面**的指导原则(Ω₁-Ω₈),但明确:

- Ω₃ Evolve 的"在线进化"在 R2 解冻前限定为规则/统计驱动(AEGIS-lite)
- Ω₅ Credit 的 Shapley 归因推迟至 v3.x(随 MAPPO)
- 八定律不改变四定律的既有落地(Ω₁-Ω₄ 对应 crate 不变)

### 决策 3:R2 合规策略(全程硬约束)

1. gsoe-evolution / auto-dpo 源码禁止出现 5 个 R2 扫描关键词
2. 所有学习能力走 omega-learner 接缝(新接缝从 S8 起)+ CapabilityToken 灰度 + ADR-043 影子模式
3. AEGIS Critic 仅使用 CiGate + SignificanceDetector + 启发式检测,禁止约束 RL
4. 每 Phase 合并前本地跑 `r2_freeze_violation_e2e` 验证

### 决策 4:PER 落点与实现约束

- **否决**方案 §5.1 将 PER 放入 event-bus(复发 ADR-033 治理的 L1 上帝 crate 病理,
  且与 omega-learner 既有 `replay_pool.rs` 重复建设)
- PER 落 L6 omega-learner,数据结构用 SumTree(O(log n) 采样/更新),
  **否决**方案参考实现的 Vec 线性扫描(O(n) 淘汰 + O(n·batch) 采样)
- 经验摄入 = event-bus 异步旁路订阅 + 有界 try_send(复刻 Critical 通道模式),
  严禁侵入 publish 热路径;验收门禁:bus_bench 吞吐回退 ≤2%

### 决策 5:版本节奏

**否决**方案直接跳版本至 v3.1.0-omega;走 SemVer minor 渐进:

- Phase 0(对账+计划)→ 2.4.1-omega 起点
- Phase 1(自我评估层)→ v2.5.0-omega
- Phase 2-3(AEGIS-lite + 变体隔离)→ v2.6.0-omega
- Phase 4-5(学习基建 + 知识层 + 收敛)→ v2.7.0-omega

每版本独立可发布可回滚(新模块 append-only,回滚即不导出)。

### 决策 6:性能可证伪门禁

- 每项优化先建 criterion 基线(Phase 0 记录 bus_bench / vector_bench / linucb_select 现值)
- 图谱建边用 hnsw_rs 近邻候选,**否决**方案 §6.2 的 O(n²) 全对比较
- release binary <50MB 红线(本期零新增重依赖,tonic/prost 推迟)

## 3. 否决方案

| 方案 | 否决理由 |
|---|---|
| 每组件一个新 crate(17 新 crate) | workspace 暴涨至 54 成员,编译时间与依赖图失控;既有 crate 已有 80% 承载能力 |
| PER 入 event-bus | L1 上帝 crate 病理复发 + 与 replay_pool 重复建设 |
| 本期引入 tonic/prost gRPC | 无 Python 训练服务实体,纯死代码 + 约 2-5MB 体积风险 + 受 R2 冻结 |
| DQN/PPO/MAPPO 在线训练照抄 | 直接违反 ADR-042 无条件冻结 |
| 直接跳版本 v3.1.0-omega | 违反 SemVer 渐进与"每阶段独立可回滚"原则 |
| 方案 Vec 线性 PER / O(n²) 建边 | 万级规模性能塌方 |

## 4. 修订记录(2026-08-07,基线 v2.22.0-omega 对账)

| 修订项 | 原裁决 | 修订后 | 依据 |
|---|---|---|---|
| rl-types 落地范围 | ✅ 本期(全部 4 类型) | ⚠️ 部分落地:VariantId/ProceduralBlueprint/BehaviorContract ✅ 已落地;RLState/RLAction/RLExperience ❌ 未落地,推迟至 v3.0.x 补齐(随方案 Milestone B-1) | 2026-08-07 全仓库 Grep 复核:RLState/RLAction/RLExperience 零匹配;RL 训练面仍受 ADR-042 R2 冻结,补齐仅限 L0 纯类型 + 接缝映射,不含训练逻辑 |
| 裁决依据文档 | 方案文档(chimera_ultimate_polish_v2.7.md) | 新增 `docs/architecture/CHIMERA_V3_专项优化方案_v2.21基线.md` 为落地裁决依据(基线已对账至 v2.22.0-omega) | 2026-08-07 基线对账 |
| **rl-types 落地状态** | ⚠️ 部分落地(RLState/RLAction/RLExperience 缺失) | **✅ 已补齐(2026-08-08)**: `nexus-contracts/src/rl_types.rs` 落地 RLState/RLAction/RLExperience/MemPiAction 纯类型(Serde + 接缝映射 `seam_id()`,零训练逻辑,ADR-033/042 合规);S1-S8 映射 SeamId,S9 Route/Custom 返回 None(SeamId 枚举 9 号位为 S9TokenEfficiency/ADR-069,诚实表达);测试: 4 集成 + 4 proptest 不变量(JSON/MsgPack 往返) + 内嵌单测 | 方案 Milestone B-1 交付;nexus-contracts 567 tests 全绿 |

> 本修订不改变其余 16 组件裁决。落地执行以 `CHIMERA_V3_专项优化方案_v2.21基线.md` + 本 ADR 为双重权威。

## 5. 影响范围

- **修改 crate**:efficiency-monitor / chimera-tui / nexus-contracts / seccore / gsoe-evolution /
  parliament / pvl-layer / omega-learner / cmt-tiering / mlc-engine / repo-wiki / event-bus(新事件)
- **新增 crate**:无
- **新增 ADR**:ADR-050(AEGIS 降级设计)/ ADR-051(Variant Pool + 规则路由)
- **新增文档**:`docs/architecture/IMPLEMENTATION_PLAN_polish_v2.7.md`

## 6. 验收标准

1. workspace 全量测试 0 失败,不删既有测试
2. `r2_freeze_violation_e2e` 持续全绿
3. PER:10 万规模采样 p99 <100µs,bus_bench 吞吐回退 ≤2%
4. AEGIS:100% 变体经 Critic + Parliament 审议,误杀率 <5%(二项检验)
5. release binary <50MB
6. 3 个 minor 版本各自独立可发布可回滚
