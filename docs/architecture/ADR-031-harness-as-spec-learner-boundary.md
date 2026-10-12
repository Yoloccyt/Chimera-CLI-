# ADR-031: Harness-as-Spec + omega-learner 边界(含 C2 嫁接点命名映射表与 §5.2 九项裁决对账附录)

> **🔄 v2.4.0-omega 基线增补**(2026-07-26):本文档基于 v2.3.1-omega **35 crate** 快照撰写(含 35 × 624 .rs 文件零命中证据);**v2.4.0-omega 后 workspace 演进为 37 crate**(新增 L0 `nexus-contracts` + L6 `omega-learner`,后者为本 ADR 决策 1 直接落地物)——**L0 决策 1 实际已落地**(ADR-033 nexus-contracts L0),35 → 37 crate 演进增量为 v5.0 P2/P4 本 ADR 决策的工程实现。决策核心(C2 嫁接点 + 命名映射表)对 37 crate 全部适用;**附录 A 命名映射表** 建议扩展至 37 crate × 760+ .rs 文件重新跑零命中验证(可由 E01 在 v2.5.0-omega 复审时执行)。

## 状态

已批准 (2026-07-23)

## 背景

- 项目当前处于 v2.3.1-omega(第三阶段模块级系统性优化,35 crate 全覆盖,2877 测试全绿),`chimera-mas` 子系统 Stage A 骨架与 Stage B Part II 闭环能力均已落地(ADR-026/027/028),INV-7/INV-8 不变量经 proptest 验证。
- v5.0 设计文档提出 **Harness-as-Spec DSL** 范式:将验证器(verifier)自身从硬编码提升为可版本化、可进化、可回滚的规格(spec)资源,使"什么算正确"与"如何执行"分离,支持 A/B 对照与一键回滚。
- v5.0 设计文档 **C2 决策**提出新建 `omega-learner` crate(L6 Router 层),承载 Bandit 学习(LinUCB 算法),与既有 `gsoe-evolution`(GRPO 在线进化) / `auto-dpo`(偏好对生成)嫁接,而非独立新建学习栈。
- v5.0 设计文档 **§5.2** 提出九项收敛裁决(MAX_AGENT_DEPTH / WSJF / ExpertConsultant / INV-8 / StabilityGuard / AgentStatus TypeState / NamespaceQuota / INV-9 / AgentMessage 11 类),需与 v2.3.1-omega 真实代码逐项对账,识别"已对账一致"与"需补裁决"两类处置。
- **学习层隔离原则**:学习(learner)是进化反馈的消费者与策略的生产者,但绝不能出现在推理关键路径上。推理路径禁止同步调用 learner;策略更新经事件总线(Event Bus)异步广播,调用方本地缓存策略快照、本地 fallback。

> **ADR 编号确认**: `docs/architecture/adr_index.md` 现有最大编号为 ADR-030(含 ADR-029 TUI v3.1 引擎)。本 ADR 编号确认为 **ADR-031**,与 `CODE_WIKI.md §2.3 ADR 决策参考` 保持一致(总数 30 → 31)。

> **命名漂移声明**: v5.0 设计文档使用的部分类型/方法名(`GrpoPolicy` / `EvolutionRecord` / `TrajectorySample` / `SpecRegistry` / `generate_pair()`)在 v2.3.1-omega 真实代码中**完全不存在**(35 crate × 624 .rs 文件零命中,`rg -c` 退出码 1 确认)。本 ADR 附录 A 记录完整命名映射表,附录 B 记录九项裁决对账,确保设计文档与代码基线的偏差被显式文档化而非静默漂移。

## 决策

经 E01 首席架构师 + E04 路由算法专家 + E06 认知科学专家分布式深度分析与多轮交叉验证,对 Harness-as-Spec DSL + omega-learner 边界作出以下 6 项架构决策:

### 决策 1: Harness-as-Spec DSL v0 — `HarnessSpec` TOML 反序列化 + Merkle 完整性

在新建 crate `nexus-contracts`(L0 合约层,详见决策 3 依赖说明)的 `src/harness.rs` 中定义 `HarnessSpec` 类型,以 TOML 反序列化承载验证器规格:

```toml
# HarnessSpec TOML 结构(v0)
[meta]
spec_id = "quest_completion_v3"
version = "3.1.0"
author = "E01"
created_at = "2026-07-23T10:00:00Z"

[[contracts]]
name = "all_tasks_completed"
assertion = "quest.tasks.iter().all(|t| t.status == Completed)"

[[hops]]
action = "delegate"
max_depth = 5

[retry]
max_attempts = 3
backoff_ms = 500

[auxiliary]
timeout_ms = 30000
```

- **反序列化层**: `HarnessSpec` 使用 `serde` + `toml` crate 反序列化,字段划分为 `meta`(规格元数据)/ `contracts`(断言契约)/ `hops`(执行跳点)/ `retry`(重试策略)/ `auxiliary`(辅助参数)五大段。
- **完整性校验**: spec 文件与代码同级 **Merkle 完整性校验**,复用 `seccore` 的 `audit.rs` Merkle 审计链机制(ADR-002 能力衰减模型已落地)。spec 的 SHA-256 哈希注入审计链,任何篡改可检测。
- **无写路径原则**: 任务输入(payload)经 `HarnessSpec` 求值时**无写路径** — spec 只读不写,防止恶意任务输入通过 spec 注入修改验证器行为(§6.1 架构红线"裸奔"防护)。
- **不可进化面硬编码**: 以下内容**不进入** `HarnessSpec`(不可被 spec 修改):13 条架构红线(§6.1/§6.2)、Critical 事件清单、INV-7/INV-8/INV-9 不变量、沙箱配置(SecCore)、QEEP 量子纠缠协议、验证器自身的核心逻辑。这些硬编码在代码中,spec 只能引用不能覆盖。

### 决策 2: SpecRegistry 扩展 — 复用 `EvolutionResult` 谱系 + 版本化 + A/B + 回滚

在 `crates/gsoe-evolution/src/` 新建 `SpecRegistry` 模块,复用既有 [EvolutionResult](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L156) 的谱系(lineage)机制作为 spec 版本存储:

- **谱系复用**: [EvolutionResult](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L156) 已含 `new_policy` / `improvement` / `generation` 三字段,天然适合表达 spec 版本演进。`SpecRegistry` 将 `HarnessSpec` 的版本快照以 `EvolutionResult` 谱系链存储,每代 spec 对应一个 `generation`。
- **版本化**: 每个 `HarnessSpec` 携带 `meta.version`(SemVer),`SpecRegistry` 维护 `spec_id → Vec<HarnessSpec>` 的版本历史。新版本通过 `SpecRegistry::register(spec)` 追加,旧版本不删除(append-only,与 ADR-028 决策 1 一致)。
- **A/B 对照**: `SpecRegistry::activate_ab(spec_id_a, spec_id_b, traffic_split)` 支持按流量比例激活两个 spec 版本,验证器执行时按 `traffic_split` 随机选择版本,结果分别统计。
- **一键回滚**: `SpecRegistry::rollback(spec_id, target_version)` 将活跃 spec 回滚到指定历史版本。回滚操作发布 `NexusEvent::SpecRolledBack` 事件(Normal 级 broadcast),所有验证器订阅后切换本地缓存。
- **不可进化面守护**: `SpecRegistry` 在 `register()` 时校验新 spec 是否触碰不可进化面(决策 1 硬编码清单)。若新 spec 试图修改 INV-7 阈值或 Critical 事件清单,返回 `GsoeError::ImmutableSurfaceViolated`,拒绝注册。

### 决策 3: omega-learner 边界 — L6 crate + 异步下发 + 本地 fallback

新建 `omega-learner` crate(L6 Router 层),承载 Bandit 学习(LinUCB 算法),与既有 `gsoe-evolution`(GRPO) / `auto-dpo`(DPO 偏好对)嫁接:

- **层级归属**: L6 Router 层,与 `osa-coordinator` / `kvbsr-router` / `faae-router` / `sesa-router` 同层。
- **依赖清单**(§2.2 依赖铁律合规):
  - `nexus-contracts`(L0)— 向下依赖 ✅(消费 `HarnessSpec` + `SelectorPolicy` 类型)
  - `event-bus`(L1)— 向下依赖 ✅(发布 `PolicyUpdated` 事件)
  - `ndarray`(外部 crate)— 线性代数运算,无层级约束
  - `gsoe-evolution`(L5)— 向下依赖 ✅(复用 [EvolutionPolicy](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L23) 作为策略载体)
  - `auto-dpo`(L5)— 向下依赖 ✅(消费 [PreferencePair](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L18) 作为奖励信号)
- **异步下发原则**: `omega-learner` **只异步下发策略**,不参与同步推理。learner 通过 `PolicyUpdated` 事件经 Event Bus 广播新策略快照,调用方(路由器/执行器)订阅事件后更新本地缓存的策略快照。推理路径读取本地快照,不等待 learner。
- **本地 fallback**: 调用方在读取本地策略快照时,若快照未就绪(首次启动)/ 过期(超 TTL)/ learner panic,则降级到 `SelectorPolicy::Static`(静态默认策略),不阻塞推理。fallback 路径零分配、零 IO,保证最坏情况延迟可预测。
- **LinUCB 算法**: 采用 Li et al. 2010 的 LinUCB(Contextual Bandit)算法,以 `HarnessSpec` 版本为 arm、任务特征向量为 context、验证通过率为 reward,在线学习最优 spec 版本分配策略。算法实现在 `omega-learner::linucb` 模块,矩阵运算用 `ndarray`,无 unsafe 依赖。

### 决策 4: 学习永不在关键路径 — 推理路径禁止同步调用 learner

- **关键路径定义**: 推理关键路径 = 用户输入 → NMC 编码 → Quest 分解 → TTG 切换 → Parliament 审议 → PVL 生产验证 → OSA 协调 → 路由 → 执行 → GQEP 聚集 → 输出。此路径上的任何同步调用 learner 都会引入不可预测延迟(learner 需矩阵求逆、梯度更新)。
- **禁止同步调用**: 推理路径上的所有 crate(L1-L10)禁止以 `learner.select_arm(ctx).await` 形式同步调用 `omega-learner`。唯一的交互方式是读取本地缓存的 `SelectorPolicy` 快照(内存读,纳秒级)。
- **异步广播**: `PolicyUpdated` 事件经 Event Bus 异步广播(D3 修复路径同步落地)。learner 在后台任务中更新策略,完成后 `publish(PolicyUpdated)` 广播。调用方订阅 `PolicyUpdated` 后异步刷新本地快照,刷新失败不影响推理(继续用旧快照或 fallback)。
- **panic 隔离**: learner 后台任务若 panic,由 `tokio::spawn` 的 JoinHandle 捕获(§4.4 async 反模式第 7 条),记日志后重启 learner。推理路径完全无感知,继续用本地快照。

### 决策 5: C2 嫁接点命名映射表(附录 A)

v5.0 设计文档 C2 决策使用的类型/方法名与 v2.3.1-omega 真实代码存在命名漂移。本决策记录完整映射表(详见附录 A),处置原则为"命名映射 + 缺失新建"二分法:

- **命名映射**(3 项):v5.0 文档命名与代码命名语义等价但名称不同,推荐在后续代码中沿用代码既有命名,文档侧标注映射关系。
- **缺失新建**(2 项):v5.0 文档命名在代码中完全不存在,按 P4-W15.2 优先级新建对应模块。

### 决策 6: §5.2 九项收敛裁决对账(附录 B)

v5.0 设计文档 §5.2 提出的九项收敛裁决,经与 v2.3.1-omega 真实代码逐项对账(详见附录 B),处置为三类:

- **✅ 对账一致**(4 项):代码实现与文档裁决完全一致(MAX_AGENT_DEPTH / WSJF / ExpertConsultant / INV-8),其中 INV-8 存在 proptest 用例数差异(文档称 1000 次,实际默认 256 次),需显式配置对齐。
- **⚠️ 补裁决待实施**(5 项):文档裁决项在代码中部分缺失或未达目标形态(StabilityGuard facade 合并 / AgentStatus TypeState / NamespaceQuota L0 上提 / INV-9 委托图无环 / AgentMessage 11 类扩展),按 P2-P5 优先级分阶段实施。

## 理由

### 决策 1 理由(Harness-as-Spec DSL v0)

- **§6.1 架构红线(功能乱)**: Claude Code 尸检教训"44 个未发布标志",本项目红线"禁止功能标志,用能力场自然进化替代"。`HarnessSpec` 将验证器从硬编码标志提升为可版本化 spec,是"能力场自然进化"的规格化表达 — spec 版本演进即能力进化,无需功能标志开关。
- **§3.4.1 第 8 条(专家团队评审)**: spec 变更需经 8 专家(E01-E08)评审,`HarnessSpec` 的 TOML 格式使 spec 变更可 diff、可 review、可回滚,比硬编码验证器更易审计。
- **防注入(无写路径)**: spec 只读不写,任务输入经 spec 求值时不修改 spec 自身,防止恶意输入通过 spec 注入修改验证器行为(§6.1 "裸奔"防护)。
- **Merkle 完整性**: 复用 seccore `audit.rs` 已落地的 Merkle 审计链(ADR-002),spec 哈希注入审计链后任何篡改可检测,无需新建完整性机制。

### 决策 2 理由(SpecRegistry 扩展)

- **§3.3.1 第 1 条(OMEGA 四定律守恒)**: Ω-Evolve 由 `gsoe-evolution` 单一实现。`SpecRegistry` 建在 `gsoe-evolution` 内而非新建 crate,复用 [EvolutionResult](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L156) 谱系机制,是 Ω-Evolve 单一实现的延伸而非重复。
- **§3.4.1 第 6 条(性能可证伪)**: A/B 对照需有可证伪证据。`SpecRegistry::activate_ab()` 的流量分割 + 结果统计为 spec 版本对比提供客观证据。
- **不可进化面守护**: 13 条红线 + Critical 清单 + INV-7/8/9 + 沙箱/QEEP + 验证器本身硬编码,是"进化悖论红线"(§3.4.5)的落地 — 进化不能修改进化自身的验证器,否则形成自指悖论。

### 决策 3 理由(omega-learner 边界)

- **C2 决策(嫁接而非新建)**: `omega-learner` 嫁接 `gsoe-evolution`(GRPO 策略)和 `auto-dpo`(DPO 偏好对),复用 [EvolutionPolicy](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L23) 作为策略载体、[PreferencePair](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L18) 作为奖励信号,不新建独立的策略/奖励类型栈。这与 ADR-026 决策 2"80% 能力复用"原则一致。
- **§3.4.1 第 7 条(学术支撑落地)**: LinUCB 算法(Li et al. 2010)是 Contextual Bandit 的经典算法,有严格 regret 上界证明(O(log T)),适合 spec 版本选择的在线学习场景。
- **§2.2 依赖铁律**: `omega-learner` 在 L6,依赖 L0/L1/L5 全部向下,不向上依赖,合规。
- **本地 fallback**: `SelectorPolicy::Static` 降级保证最坏情况零延迟,即使 learner 完全不可用,系统仍可运行(降级运行是 §6.1 "结果丢了"红线的补偿机制)。

### 决策 4 理由(学习永不在关键路径)

- **§6.1 架构红线(竞态)**: Claude Code 尸检教训"void Promise 无 await",本项目红线"所有 async 必须 await 或 spawn 管理"。learner 后台任务用 `tokio::spawn` 管理 JoinHandle,推理路径不 await learner,二者解耦。
- **§3.4.5 推理悖论红线**: "10 层架构的跨层协调成本存在推理悖论阈值 — 当协调成本超过推理增益时,多 Agent 反而不如单 Agent"。learner 若在关键路径同步调用,其矩阵运算延迟(毫秒级)远超路由决策增益(微秒级),直接触发推理悖论。异步广播 + 本地快照将协调成本降至纳秒级(内存读),远低于增益。
- **§4.4 async 反模式第 7 条**: "tokio::spawn fire-and-forget 评估框架 — 关键路径(衰减循环)必须管理 JoinHandle"。learner 后台任务管理 JoinHandle,panic 可捕获重启,符合该框架。
- **D3 修复路径**: `PolicyUpdated` 事件经膜(Event Bus)异步广播,是 D3 修复路径(学习层与推理层解耦)的同步落地。

### 决策 5 理由(命名映射表)

- **命名漂移显式化**: v5.0 设计文档作为前瞻性设计蓝图,其命名(`GrpoPolicy` / `EvolutionRecord` / `TrajectorySample`)与 v2.3.1 代码([EvolutionPolicy](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L23) / [EvolutionResult](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L156) / [ModelOutput](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L18))存在漂移。若不显式记录映射,后续开发者可能误以为代码缺失而重复新建,违反 DRY。
- **零命中证据**: `rg -c "GrpoPolicy|EvolutionRecord|TrajectorySample|SpecRegistry|NamespaceQuota"` 退出码 1(无匹配),确认 35 crate × 624 .rs 文件零命中,映射表基于实证而非假设。

### 决策 6 理由(九项裁决对账)

- **设计文档与代码对账**: v5.0 设计文档 §5.2 的九项裁决是 MAS 子系统的收敛基线。对账确保设计意图与代码实现一致,偏差被显式记录而非静默漂移。
- **补裁决优先级分配**: 5 项"需补裁决"按依赖关系与影响面分配 P2-P5 优先级:NamespaceQuota(P2-W5.1,L0 合约层先建)→ INV-9(P3-W11.3,依赖 invariants.rs 扩展)→ StabilityGuard facade(P5-W19.1)/ AgentStatus TypeState(P5-W19.2)/ AgentMessage 11 类(P5-W19.2,MAS 深化阶段统一实施)。
- **proptest 用例数对齐**: INV-8 文档称"proptest 1000 次",实际 [proptest.rs](file:///D:/Chimera CLI/crates/chimera-mas/tests/proptest.rs#L19) 默认 256 次(注释"每个属性测试默认 256 cases")。补裁决要求显式 `ProptestConfig::with_cases(1000)` 对齐 INV-7/INV-8 规格,与 ADR-028 决策 3"1000 次属性测试"一致。

## 影响

### 新增内容

- **新增 crate(2 个)**:
  - `nexus-contracts`(L0 合约层)— 定义 `HarnessSpec` / `SelectorPolicy` / `NamespaceQuota`(决策 7 补裁决)等跨层共享契约类型。crate 数量 35 → 36。
  - `omega-learner`(L6 Router 层)— LinUCB Bandit 学习,嫁接 gsoe-evolution / auto-dpo。crate 数量 36 → 37。
- **新增模块**:
  - `crates/nexus-contracts/src/harness.rs` — `HarnessSpec` TOML 反序列化 + Merkle 完整性校验(决策 1)
    > ⚠ **落点现状注记（2026-09-22 按代码复核；决策本身不改写）**：`nexus-contracts` 内**不存在** `harness.rs`。
    > 决策 1 的能力已落地，但分成两个模块：`crates/nexus-contracts/src/harness_spec.rs`（`HarnessSpec:412`、`HarnessSpecError:270`）
    > 与 `crates/nexus-contracts/src/harness_dimensions.rs`。⇒ 检索/改动请去这两个文件，勿按本文路径打开 `harness.rs`。
  - `crates/gsoe-evolution/src/spec_registry.rs` — `SpecRegistry` 版本化 + A/B + 回滚(决策 2)
  - `crates/omega-learner/src/linucb.rs` — LinUCB 算法实现(决策 3)
  - `crates/omega-learner/src/policy_broadcast.rs` — `PolicyUpdated` 异步广播(决策 4)
- **新增 NexusEvent 变体**:
  - `SpecRegistered`(Normal)— spec 版本注册成功
  - `SpecRolledBack`(Normal)— spec 版本回滚
  - `PolicyUpdated`(Normal)— learner 策略更新广播
  - `ImmutableSurfaceViolated`(Critical)— spec 试图触碰不可进化面(走 mpsc 通道)
- **新增测试**: `HarnessSpec` TOML 往返测试 + Merkle 完整性测试 + `SpecRegistry` A/B 回滚测试 + LinUCB regret 收敛测试 + learner panic fallback 测试
- **新增 criterion 基准**:
  - `harness_spec_deserialize` — TOML 反序列化性能(< 1ms)
  - `linucb_select_arm` — LinUCB 选臂性能(< 100μs)
  - `policy_snapshot_read` — 本地策略快照读取性能(< 1μs,证明不在关键路径)
- **新增 ADR**: 本文档(ADR-031)

### 修改内容

- **`Cargo.toml`**: `workspace.members` 新增 `crates/nexus-contracts` + `crates/omega-learner`(35 → 37)
- **`crates/gsoe-evolution/src/lib.rs`**: 新增 `spec_registry` 模块声明 + 重导出
- **`crates/event-bus/src/types.rs`**: 新增 4 个事件变体(74 → 78)
- **`crates/seccore/src/audit.rs`**: 暴露 Merkle 校验公共接口供 `nexus-contracts` 复用(若当前为私有则改为 `pub`)
- **`CHANGELOG.md`**: 新增 ADR-031 章节
- **`docs/architecture/CODE_WIKI.md`**: 新增 nexus-contracts(L0)+ omega-learner(L6)条目
- **`docs/architecture/adr_index.md`**: 新增 ADR-031 条目(总数 30 → 31)

### 资源影响评估

| 维度 | 评估 |
|------|------|
| crate 数量 | 35 → 37(新增 nexus-contracts + omega-learner) |
| 依赖变更 | 新增 `ndarray`(omega-learner)+ `toml`(nexus-contracts),均为纯 Rust 无 unsafe |
| Docker/binary 体积 | 预估增加 < 2MB(ndarray + toml 纯 Rust,无 C binding) |
| NexusEvent 变体数 | 74 → 78(新增 4 个,其中 1 个 Critical) |
| 测试覆盖 | 新增 ~40 个测试(HarnessSpec + SpecRegistry + LinUCB + fallback) |
| criterion 基准 | 42+ → 45+(新增 3 项) |
| 版本号 | 待附录 C 裁决(P0-W0.4 版本号协调策略) |

### 复杂度预算抵消清单

新增 2 crate 带来复杂度增量,需以下抵消措施:

| 抵消项 | 复杂度削减 | 状态 |
|--------|-----------|------|
| `SpecRegistry` 复用 `EvolutionResult` 谱系(非新建 lineage) | -1 模块 | ✅ 决策 2 |
| `omega-learner` 嫁接 gsoe-evolution/auto-dpo(非新建策略栈) | -2 模块 | ✅ 决策 3 |
| Merkle 校验复用 seccore audit.rs(非新建审计机制) | -1 模块 | ✅ 决策 1 |
| `nexus-contracts` 承载 NamespaceQuota 等待上提类型(集中化) | -3 散落定义 | ⚠️ P2-W5.1 |
| learner 本地 fallback 复用 `SelectorPolicy::Static`(非新建降级链) | -1 模块 | ✅ 决策 3 |

## 考虑的方案

### 方案 A: Harness-as-Spec DSL + SpecRegistry + omega-learner 边界(采纳)

- **内容**: 如决策 1-6 所述,新建 `nexus-contracts`(L0)+ `omega-learner`(L6),spec 可版本化/可回滚,learner 异步隔离。
- **采纳理由**: 满足 v5.0 设计文档 Harness-as-Spec 范式与 C2 嫁接决策,学习层隔离符合 D3 关键路径不学习原则,命名漂移显式文档化。

### 方案 B: 硬编码 spec 在 parliament 内(否决)

- **内容**: 将验证器规格以硬编码 `match` 分支实现在 `parliament` crate(L8)内,不新建 `nexus-contracts` / `HarnessSpec`。
- **否决理由**:
  1. 违反 v5.0 §11 深模块候选原则 — `contracts` / `hops` 类型应替换为 Harness-as-Spec DSL,而非硬编码在 Parliament 审议器内。Parliament 的职责是"审议",不是"定义什么是正确"。
  2. 硬编码 spec 无法版本化、无法 A/B 对照、无法回滚,失去 Harness-as-Spec 的核心价值。
  3. spec 与审议逻辑耦合,违反单一职责原则,parliament 已承载 RoleRegistry / SkepticVeto / AHIRT 等职责,进一步膨胀违反 §6.1 "函数太大"(单 crate 膨胀同理)。

### 方案 C: learner 同步调用(否决)

- **内容**: 推理路径直接 `learner.select_arm(ctx).await` 同步调用 `omega-learner`,获取实时策略。
- **否决理由**:
  1. 违反 D3 关键路径不学习原则 — learner 的 LinUCB 矩阵求逆是毫秒级操作,注入推理路径后单次推理延迟从微秒级退化为毫秒级。
  2. 违反 §3.4.5 推理悖论红线 — 协调成本(毫秒级)超过推理增益(微秒级),直接触发推理悖论阈值。
  3. learner panic 会阻塞推理路径(同步 await 无法 fallback),违反 §6.1 "竞态"红线(未管理的 async 阻塞)。
  4. learner 不可用时推理路径直接失败,无降级路径,违反 §6.1 "结果丢了"红线。

## 合规性

- **§2.1 分层映射**: 符合。`nexus-contracts` 为 L0(新建合约层,位于 L1 Core 之下),`omega-learner` 为 L6 Router 层,不改现有分层结构。
- **§2.2 依赖铁律**: 符合。`omega-learner`(L6)依赖 `nexus-contracts`(L0)+ `event-bus`(L1)+ `gsoe-evolution`(L5)+ `auto-dpo`(L5),全部向下依赖;`ndarray` 为外部 crate 无层级约束;跨层通信走 Event Bus(`PolicyUpdated` 广播)。
- **§3.3.1 第 1 条(OMEGA 四定律守恒)**: 符合。Ω-Evolve 由 `gsoe-evolution` 单一实现,`SpecRegistry` / `omega-learner` 是其延伸而非重复;Ω-Event 由 `event-bus` 单一实现,`PolicyUpdated` 复用现有总线。
- **§3.4.1 第 6 条(性能可证伪)**: 符合。3 项 criterion 基准(`harness_spec_deserialize` / `linucb_select_arm` / `policy_snapshot_read`)提供性能证据,其中 `policy_snapshot_read < 1μs` 证明学习不在关键路径。
- **§3.4.1 第 7 条(学术支撑落地)**: 符合。LinUCB 算法(Li et al. 2010)有 regret 上界证明;Harness-as-Spec 范式有 v5.0 §11 深模块理论支撑。
- **§3.4.1 第 8 条(专家团队评审)**: 符合。本 ADR 经 E01 首席架构师 + E04 路由算法专家 + E06 认知科学专家分布式评审。
- **§3.4.5 推理悖论红线**: 符合。决策 4 明确禁止推理路径同步调用 learner,异步广播 + 本地快照将协调成本降至纳秒级。
- **§4.1 编码规范**: 符合。`#![forbid(unsafe_code)]` 保持(ndarray / toml 均为纯 Rust 无 unsafe);库层 thiserror;无生产路径 unwrap/expect;单函数 ≤ 200 行;Top-K 用 `select_nth_unstable`;并发收集用 `FuturesUnordered`。
- **§4.4 async 反模式**: 符合。learner 后台任务管理 JoinHandle(第 7 条);`PolicyUpdated` 广播用 `publish().await` 配合 async 方法;不持锁跨 `.await`。
- **§6.1 架构红线**: 符合。spec 无写路径防注入("裸奔"防护);learner 异步隔离不阻塞推理("竞态"防护);不可进化面硬编码("功能乱"防护 — 进化不修改进化验证器)。
- **§6.2 Week 1-8 实战新红线**: 符合。`ImmutableSurfaceViolated` 为 Critical 事件走 mpsc 通道;不引入 unsafe 依赖;`PolicyUpdated` 广播先 subscribe 再 spawn(第 3 条)。
- **§10.2 Docker 镜像约束**: 符合。新增 ndarlay + toml 为纯 Rust,镜像体积增量 < 2MB,总量仍 < 100MB。
- **ADR-026 决策 2(80% 复用)**: 符合。`omega-learner` 嫁接 gsoe-evolution / auto-dpo,复用 [EvolutionPolicy](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L23) / [PreferencePair](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L18),非新建策略栈。
- **ADR-028 决策 1(append-only)**: 符合。`SpecRegistry` 版本历史 append-only,旧版本不删除;INV-7/INV-8 不变量不变。
- **C2 决策(嫁接而非新建)**: 符合。omega-learner 嫁接而非新建学习栈,附录 A 记录完整命名映射。

## 相关文档

- **设计文档**: v5.0 设计文档 §3(设计总纲)/ §7(学习系统)/ §5.2(九项收敛裁决)/ §11(深模块候选)/ C2 决策(omega-learner 嫁接)
- **规则**: `.trae/rules/nuxus规则.md` §2.1(分层映射)/ §2.2(依赖铁律)/ §3.3.1(第二阶段开发原则)/ §3.4.1(第三阶段开发原则)/ §3.4.5(前瞻性架构参考 — 三重悖论红线)/ §4.1(编码规范)/ §4.4(async 反模式)/ §6.1(架构红线)/ §6.2(Week 1-8 新红线)/ §10.2(Docker 镜像约束)
- **CODE_WIKI.md**: `docs/architecture/CODE_WIKI.md` §3.1(crate 索引)/ §2.3(ADR 表)
- **ADR 索引**: `docs/architecture/adr_index.md`(本 ADR 同步更新,总数 30 → 31)
- **关联 ADR**:
  - ADR-026(MAS 子系统 Stage A,80% 能力复用先例)
  - ADR-028(Part II 闭环,INV-7/INV-8 + append-only 策略)
  - ADR-002(能力衰减模型,Merkle 审计链复用源)
  - ADR-003(Event Bus 实现选型,`PolicyUpdated` 广播基础)
  - ADR-005(持久化存储选型,sqlite-vec 禁用先例 — ndarray 纯 Rust 无 unsafe)
  - ADR-029(TUI v3.1 引擎,feature 开关先例)
- **代码基线**:
  - [gsoe-evolution/types.rs](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs) — `EvolutionPolicy` / `GrpoRollout` / `EvolutionResult` 实际类型
  - [auto-dpo/generator.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs) — `PreferencePairGenerator::generate()` / `ModelOutput` / `PreferencePair` 实际方法与类型
- **学术参考**: Li, L., Chu, W., Langford, J., & Schapire, R. E. (2010). A Contextual-Bandit Approach to Personalized News Article Recommendation. *WWW '10*.

---

## 附录 A: C2 嫁接点命名映射表

> v5.0 设计文档 C2 决策使用的类型/方法名与 v2.3.1-omega 真实代码的命名漂移映射。
> 零命中验证: `rg -c "GrpoPolicy|EvolutionRecord|TrajectorySample|SpecRegistry|NamespaceQuota" "D:/Chimera CLI/crates" --type rust` 退出码 1(无匹配),确认 35 crate × 624 .rs 文件零命中。

| # | v5.0 文档命名 | v2.3.1 实际命名 | 代码位置 | 处置 | 优先级 |
|---|---------------|----------------|---------|------|--------|
| A1 | `GrpoPolicy` | [`EvolutionPolicy`](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L23) | [gsoe-evolution/types.rs:23](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L23) | 命名映射(推荐沿用代码命名) | — |
| A2 | `EvolutionRecord` | 完全缺失 | — | P4-W15.2 新建 `SpecRegistry`(决策 2)承载谱系机制,复用 [`EvolutionResult`](file:///D:/Chimera CLI/crates/gsoe-evolution/src/types.rs#L156) 谱系 | P4 |
| A3 | `generate_pair()` | [`generate()`](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L94) | [auto-dpo/generator.rs:94](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L94) | 方法名映射(推荐沿用代码命名) | — |
| A4 | `TrajectorySample` | [`ModelOutput`](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L18) | [auto-dpo/generator.rs:18](file:///D:/Chimera CLI/crates/auto-dpo/src/generator.rs#L18)(类型定义在 `types.rs`) | 类型映射(推荐沿用代码命名) | — |
| A5 | `SpecRegistry` | 完全缺失 | — | P4-W15.2 新建(决策 2),建在 `crates/gsoe-evolution/src/spec_registry.rs` | P4 |

**映射原则**:

1. **命名映射(A1/A3/A4)**: v5.0 文档命名与代码命名语义等价,推荐后续工作沿用代码既有命名,文档侧标注映射关系,不重命名既有代码(append-only 原则)。
2. **缺失新建(A2/A5)**: v5.0 文档命名在代码中完全不存在,按 P4-W15.2 优先级新建。`SpecRegistry`(A5)作为模块建在 `gsoe-evolution` 内而非新建 crate(决策 2);`EvolutionRecord`(A2)的谱系功能由 `SpecRegistry` 复用 `EvolutionResult` 承载,不单独新建类型。

---

## 附录 B: §5.2 九项收敛裁决对账

> v5.0 设计文档 §5.2 九项收敛裁决与 v2.3.1-omega 真实代码的逐项对账。
> 对账方法: 代码行号实证 + `rg` 搜索验证。行号基于 v2.3.1-omega 快照(2026-07-23 核实)。

| # | 裁决项 | 对账状态 | 行号证据 / 补裁决目标阶段 |
|---|--------|---------|-------------------------|
| 1 | MAX_AGENT_DEPTH=5 | ✅ 对账一致 | [orchestrator.rs:49](file:///D:/Chimera CLI/crates/chimera-mas/src/orchestrator.rs#L49) `pub const MAX_AGENT_DEPTH: usize = 5;` + `MasError::MaxDepthExceeded` 错误类型(注释引用 ADR-026 决策 1) |
| 2 | WSJF 优先级调度 | ✅ 对账一致 | [scheduler.rs:125](file:///D:/Chimera CLI/crates/chimera-mas/src/scheduler.rs#L125) `wsjf_score()` + [scheduler.rs:218](file:///D:/Chimera CLI/crates/chimera-mas/src/scheduler.rs#L218) `struct PriorityScheduler` + [scheduler.rs:53](file:///D:/Chimera CLI/crates/chimera-mas/src/scheduler.rs#L53) `struct WsjfWeights`(完整公式实现,Critical 抢占 Low + 饥饿线性提权) |
| 3 | ExpertConsultant | ✅ 对账一致 | [expert_consult.rs:86](file:///D:/Chimera CLI/crates/chimera-mas/src/knowledge/expert_consult.rs#L86) `struct ExpertConsultant`(含 `ConsultSla` / `new()` / `available_permits()`)。knowledge/ 子模块另含 `mutual_inquiry.rs`(`MutualInquirer`)+ `wiki_retrieval.rs`(`WikiRetriever`),三者构成 §18 知识协同完整闭环 |
| 4 | archive INV-8 单调性 | ✅ 对账一致(proptest 用例数差异) | [invariants.rs:238](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs#L238) `check_inv8_archive_monotonicity()` + `ArchiveTier` 枚举(Hot/Warm/Cold/Ice 四级,`level()` 方法)。**补裁决**: v5.0 文档称"proptest 1000 次",实际 [proptest.rs:19](file:///D:/Chimera CLI/crates/chimera-mas/tests/proptest.rs#L19) 默认 256 次(注释"每个属性测试默认 256 cases",未显式 `ProptestConfig`)。需显式配置 `ProptestConfig::with_cases(1000)` 对齐 INV-7/INV-8 规格(与 ADR-028 决策 3 一致) |
| 5 | StabilityGuard 合并进 ImmuneSystem facade | ⚠️ 补裁决: P5-W19.1 实施 | [stability.rs:138](file:///D:/Chimera CLI/crates/chimera-mas/src/stability.rs#L138) `struct StabilityGuard` + [stability.rs:91](file:///D:/Chimera CLI/crates/chimera-mas/src/stability.rs#L91) `struct CircuitBreaker` + [stability.rs:221](file:///D:/Chimera CLI/crates/chimera-mas/src/stability.rs#L221) `struct DegradationChain` 既有保留。**补裁决**: P5-W19.1 新建 `ImmuneSystem` facade 聚合上述三者(非替换,C1 收敛原则 — facade 只聚合公共接口,不删除既有类型),提供统一的 `immune_check()` / `immune_degrade()` 入口 |
| 6 | AgentStatus TypeState 强化 | ⚠️ 补裁决: P5-W19.2 实施 | [agent/meta.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/agent/meta.rs) 当前为普通 `enum AgentStatus`(六态:Idle / Running / Paused / Completed / Failed / Crashed),未用 TypeState 模式。**补裁决**: P5-W19.2 引入 `AgentStatus<S>` 类型参数(如 `AgentStatus<Idle>` / `AgentStatus<Running>`),编译期拒绝非法状态转换(如 `Completed → Running`),消除运行时 `InvalidTransition` 错误 |
| 7 | NamespaceQuota 上提 L0 | ⚠️ 补裁决: P2-W5.1 实施 | 35 crate × 624 .rs 文件零命中(`rg -c` 退出码 1 确认)。**补裁决**: P2-W5.1 在 `nexus-contracts`(L0,本 ADR 决策 1 新建)中定义 `NamespaceQuota` 类型,作为跨层共享的命名空间配额契约,供 L6-L9 各层向下引用 |
| 8 | INV-9 委托图无环 | ⚠️ 补裁决: P3-W11.3 实施 | [invariants.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs) 仅含 INV-7(上下文预算界)+ INV-8(归档单调性),无 INV-9。**补裁决**: P3-W11.3 新增 `check_inv9_delegation_acyclic()` + proptest 1000 次(显式 `ProptestConfig::with_cases(1000)`,对齐 INV-7/INV-8 规格),验证 Agent 委托图(`RootOrchestrator → MainAgent → SubAgent → GrandAgent`)无环 |
| 9 | AgentMessage 11 类消息协议并入事件系统 | ⚠️ 补裁决: P5-W19.2 实施 | 当前 [types.rs:1693-1820](file:///D:/Chimera CLI/crates/event-bus/src/types.rs#L1693-L1820) 含 7 个 Agent 变体(`AgentTaskDelegated` / `AgentTaskCompleted` / `AgentTaskFailed` / `AgentConsultRequested` / `AgentConsultResponded` / `AgentHeartbeat` / `AgentContextOverflow`),与 CHANGELOG 67→74 一致。**补裁决**: P5-W19.2 扩展至 11 个(新增 4 个变体,覆盖 v5.0 文档 §5.2 裁决 9 的 11 类消息协议完整语义) |

**对账汇总**:

| 对账状态 | 数量 | 裁决项编号 |
|---------|------|-----------|
| ✅ 对账一致 | 4 项 | #1, #2, #3, #4(含 proptest 用例数补裁决) |
| ⚠️ 补裁决待实施 | 5 项 | #5(P5-W19.1), #6(P5-W19.2), #7(P2-W5.1), #8(P3-W11.3), #9(P5-W19.2) |

**补裁决优先级排序**(按依赖关系与影响面):

1. **P2-W5.1** — NamespaceQuota 上提 L0(决策 7):nexus-contracts 先建,为后续补裁决提供 L0 合约层基础
2. **P3-W11.3** — INV-9 委托图无环(决策 8):依赖 invariants.rs 扩展,与 INV-7/INV-8 同构,proptest 1000 次对齐
3. **P5-W19.1** — ImmuneSystem facade(决策 5):MAS 深化阶段,聚合既有 StabilityGuard / CircuitBreaker / DegradationChain
4. **P5-W19.2** — AgentStatus TypeState + AgentMessage 11 类(决策 6 + 9):MAS 深化阶段统一实施,TypeState 强化 + 事件协议扩展

---

## 附录 C: 版本号协调策略(✅ 已裁决 — 2026-07-23 用户裁决方案 A)

> **状态**: 已裁决(P0-W0.4 完成于 2026-07-23,用户批准方案 A)
>
> **裁决结果**: ✅ **方案 A(已采纳)** — v5.0 计划目标改为 **v3.2.0-omega**(minor,承接 v3.1.0-omega append-only minor 链)。先完成 v3.1.0-omega WIP(TUI v3.1 M1-M5)合并,再启动 v5.0 P1 工作,最终以 v3.2.0-omega 发布。
>
> **裁决理由**:
> 1. v3.1.0-omega 已是 v2.3.1-omega 的 append-only minor 升级(无 BREAKING,CHANGELOG 已记录)
> 2. v5.0 新增 2 crate(nexus-contracts L0 + omega-learner L6)为 append-only 增量,不破坏既有公共签名(C1 收敛原则)
> 3. 故 v3.1.0 → v3.2.0 为 minor 升级,符合 SemVer 规范与项目既有惯例(v2.1.0/v2.2.0 均为 minor append-only)
> 4. 方案 B(v4.0.0-omega)跳过 v3.x 过于激进;方案 C(回退 v3.1.0 WIP)破坏已发布 WIP,均不采纳
>
> **议题裁决**:
> 1. 本增量归为 SemVer **minor**(类比 ADR-028 决策 6 append-only minor 策略)— ✅ 已裁决
> 2. 新增 crate 不触发 major 版本升级(append-only 增量,不破坏既有公共签名)— ✅ 已裁决
> 3. `nexus-contracts` 作为 L0 新层级不单独架构版本号(与 crate 版本号统一,workspace.package.version 同步)— ✅ 已裁决
> 4. `PolicyUpdated` / `ImmutableSurfaceViolated` 等 Critical 事件新增不影响 Event Bus SemVer(append-only 事件变体,下游 match 走通配分支)— ✅ 已裁决
>
> **执行**: `workspace.package.version` 在 P1 启动时从 `2.3.1-omega` 升至 `3.2.0-omega`(承接 v3.1.0-omega WIP 合并后);新增 crate 的版本号与 workspace 统一(非 `0.1.0` pre-GA)。
>
> **关联文档**: `.trae/specs/nexus-omega-v5-implementation-plan/spec.md` §版本号协调策略 / `tasks.md` P0-W0.4 / `CHANGELOG.md` v3.2.0-omega 章节

---

> **维护者**: NEXUS-OMEGA 团队
> **创建日期**: 2026-07-23
> **基线版本**: v2.3.1-omega(创建时)
> **决策者**: E01 首席架构师 + E04 路由算法专家 + E06 认知科学专家(分布式评审)
> **分析团队**: 3 专家视角分布式深度分析(首席架构 + 路由算法 + 认知科学)
