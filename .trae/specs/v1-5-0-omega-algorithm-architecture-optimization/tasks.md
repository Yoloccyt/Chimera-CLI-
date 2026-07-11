# v1.5.0-omega — 算法优化与架构完善 - The Implementation Plan

## Phase I: 安全加固（High优先级，不破坏现有测试）

---

### [x] Task 1: ASA空关键字安全加固（N4）
- **Priority**: high
- **Depends On**: None
- **Description**: 
  - 修改 `crates/seccore/src/asa.rs`，当风险关键字列表为空时返回 `RiskLevel::Unknown` 而非 `Low`
  - 增加TDD测试验证空关键字列表的行为
  - 对应 FR-1, AC-1
- **Acceptance Criteria Addressed**: AC-1
- **Test Requirements**:
  - `programmatic` TR-1.1: 空风险关键字列表返回 RiskLevel::Unknown（而非 Low）
  - `programmatic` TR-1.2: 非空关键字列表保持原有行为（回归测试）
  - `programmatic` TR-1.3: Unknown 风险级别触发 warn! 日志记录
  - `human-judgement` TR-1.4: 代码包含 WHY 注释解释空列表≠安全
- **Notes**: 保持向后兼容——Unknown是新增级别，原有Allow/Warn/Block判定逻辑不变，仅空列表的默认值改变

---

### [x] Task 2: AuditChain前置审计记录（N5）
- **Priority**: high
- **Depends On**: None
- **Description**: 
  - 修改 `crates/seccore/src/audit.rs`，将审计记录改为两阶段：pre-execution append（pending状态）→ post-execution update（success/failure）
  - append失败时阻止命令执行（返回错误而非继续）
  - 增加TDD测试验证前置记录
  - 对应 FR-2, AC-2
- **Acceptance Criteria Addressed**: AC-2
- **Test Requirements**:
  - `programmatic` TR-2.1: 命令执行前审计链中存在pending状态记录
  - `programmatic` TR-2.2: append失败时命令不被执行（返回错误）
  - `programmatic` TR-2.3: 命令成功后记录更新为success
  - `programmatic` TR-2.4: 命令失败后记录更新为failure（含错误信息）
  - `human-judgement` TR-2.5: 两阶段状态机清晰，有WHY注释
- **Notes**: AuditEntry新增status字段（Pending/Success/Failure），现有append API保持兼容但语义改变

---

## Phase II: 架构一致性修复（P0/P1优先级）

---

### [x] Task 3: TTG EventBus集成（N18）
- **Priority**: high
- **Depends On**: None
- **Description**: 
  - 在 `crates/event-bus/src/types.rs` 新增 `ThinkingModeChanged` 事件变体（携带old_mode/new_mode）
  - 修改 `crates/quest-engine/src/ttg.rs`，模式切换时发布EventBus事件，保留tracing日志
  - TTG订阅者（若有）响应新事件
  - 增加TDD测试验证事件发布
  - 对应 FR-5, AC-5
- **Acceptance Criteria Addressed**: AC-5
- **Test Requirements**:
  - `programmatic` TR-3.1: TTG切换模式时发布ThinkingModeChanged事件
  - `programmatic` TR-3.2: 事件payload包含old_mode和new_mode
  - `programmatic` TR-3.3: 原有tracing::info!日志仍保留（向后兼容）
  - `programmatic` TR-3.4: 订阅者能正确接收事件
  - `human-judgement` TR-3.5: EventSeverity判定为Normal（非Critical）
- **Notes**: 新增事件变体不影响现有订阅者（broadcast模式忽略未知变体）

---

### [x] Task 4: GQEP全局超时（N14）
- **Priority**: high
- **Depends On**: None
- **Description**: 
  - 修改 `crates/gqep-executor/src/config.rs`，新增 `gather_deadline_ms: Option<u64>` 配置字段
  - 修改 `crates/gqep-executor/src/gatherer.rs`，增加全局超时检测（tokio::time::timeout）
  - 超时时返回GatherTimeout错误，已完成的操作结果保留
  - 增加TDD测试验证全局超时
  - 对应 FR-4, AC-4
- **Acceptance Criteria Addressed**: AC-4
- **Test Requirements**:
  - `programmatic` TR-4.1: gather_deadline_ms=None时无全局超时（原有行为）
  - `programmatic` TR-4.2: 超过deadline时返回GatherTimeout错误
  - `programmatic` TR-4.3: 超时前完成的操作结果在返回值中保留
  - `programmatic` TR-4.4: 单操作超时仍独立工作（不被全局超时覆盖）
  - `human-judgement` TR-4.5: 默认值为None（向后兼容）
- **Notes**: 默认None保持原有行为；全局超时与单操作超时是独立的两层防护

---

### [x] Task 5: ACB/DECB双治理器仲裁（N6）
- **Priority**: high
- **Depends On**: Task 3（TTG EventBus集成）
- **Description**: 
  - 修改 `crates/quest-engine/src/ttg.rs`，订阅ACB BudgetAdjusted事件（当前仅订阅DECB）
  - 新增仲裁逻辑：融合ACB(Token预算级别)和DECB(认知预算级别)，选择更保守的（更高级别=更慢更安全）
  - 仲裁策略：取两者的max级别（max保守原则）
  - 增加TDD测试验证双事件融合
  - 对应 FR-3, AC-3
- **Acceptance Criteria Addressed**: AC-3
- **Test Requirements**:
  - `programmatic` TR-5.1: TTG订阅ACB BudgetAdjusted事件
  - `programmatic` TR-5.2: ACB级别>DECB时使用ACB级别
  - `programmatic` TR-5.3: DECB级别>ACB时使用DECB级别
  - `programmatic` TR-5.4: 仅DECB事件时保持原有行为（向后兼容）
  - `programmatic` TR-5.5: 仅ACB事件时ACB级别生效
  - `human-judgement` TR-5.6: 仲裁逻辑有WHY注释解释max保守原则
- **Notes**: 采用简单max策略（更保守=更高思考级别），不做加权融合避免过度复杂；这是Open Question中推荐的方案

---

### [x] Task 6: CACR f32精度修复（N11）
- **Priority**: medium
- **Depends On**: None
- **Description**: 
  - 修改 `crates/model-router/src/cacr.rs`，大预算值(u64>2^24)的比较使用u64整数运算或f64中间值
  - 避免f32乘法导致的精度丢失
  - 增加proptest验证边界情况
  - 对应 FR-15
- **Acceptance Criteria Addressed**: AC-11（向后兼容）
- **Test Requirements**:
  - `programmatic` TR-6.1: u64>2^24的预算阈值判定精确（无f32精度误差）
  - `programmatic` TR-6.2: 小预算值(u64<2^24)行为与之前一致
  - `programmatic` TR-6.3: proptest覆盖u64全范围的预算判定
  - `human-judgement` TR-6.4: 选择f64中间值（成本低、精度足够）而非大整数运算
- **Notes**: 选f64中间值而非u128——f64有53位精度，足够表示u64预算值的精确比较

---

## Phase III: 热路径性能微优化（P1/P2优先级，需bench支撑）

---

### [~] Task 7: NexusState Arc共享（N12）— 跳过（YAGNI，需bench数据支撑）
- **Priority**: medium
- **Depends On**: None
- **Description**: 
  - 修改 `crates/nexus-core/src/state.rs`，quests存储从 `HashMap<QuestId, Quest>` 改为 `HashMap<QuestId, Arc<Quest>>`
  - `get_quest()` 返回 `Option<Arc<Quest>>` 而非 `Option<Quest>`（深拷贝→共享引用）
  - 所有调用点适配（clone Arc而非clone Quest）
  - 增加bench验证克隆开销减少
  - 对应 FR-6, AC-6
- **Acceptance Criteria Addressed**: AC-6, AC-11
- **Test Requirements**:
  - `programmatic` TR-7.1: get_quest()返回Arc<Quest>，引用计数正确
  - `programmatic` TR-7.2: 所有现有测试通过（调用点正确适配）
  - `programmatic` TR-7.3: bench显示get_quest()延迟显著降低（Arc clone ~5ns vs Quest深拷贝~500ns+）
  - `human-judgement` TR-7.4: 可变路径仍正确（Interior mutability或写时复制）
- **Notes**: Quest本身是相对大的结构（含Vec<Task>/Checkpoint等），Arc共享可显著减少热路径堆分配；写操作需考虑Arc::make_mut或保留写锁路径

---

### [~] Task 8: TaskProfile Hash trait（N17）— 跳过（YAGNI，需bench数据支撑）
- **Priority**: medium
- **Depends On**: None
- **Description**: 
  - 为 `crates/gea-activator/src/types.rs` 的TaskProfile派生Hash trait（或手动实现）
  - 修改hash_task_profile()直接使用DefaultHasher而非serde_json::to_string
  - 增加bench验证哈希计算性能提升
  - 增加proptest验证哈希一致性
  - 对应 FR-7, AC-7
- **Acceptance Criteria Addressed**: AC-7
- **Test Requirements**:
  - `programmatic` TR-8.1: 相同TaskProfile哈希值相同
  - `programmatic` TR-8.2: 不同TaskProfile哈希值高概率不同（基本无冲突）
  - `programmatic` TR-8.3: bench显示哈希计算速度提升10x+（serde_json ~1µs vs Hash ~100ns）
  - `human-judgement` TR-8.4: 所有字段都参与Hash计算（不遗漏）
- **Notes**: 128-capacity缓存场景下，serde_json序列化是热路径开销；Hash trait是零成本抽象

---

### [~] Task 9: faae-router EDSB次优选择改进（N19）— 跳过（策略变更，非bugfix，延后GA后）
- **Priority**: medium
- **Depends On**: None
- **Description**: 
  - 修改 `crates/faae-router/src/edsb.rs`，次优选择从"top-2中选第二"改为"非最热候选中相似度最高的"
  - 提升EDSB熵均衡效果
  - 增加TDD测试验证新策略
  - 对应 FR-8
- **Acceptance Criteria Addressed**: AC-11
- **Test Requirements**:
  - `programmatic` TR-9.1: 候选>2时选择非最高相似度中的最优者
  - `programmatic` TR-9.2: 候选=2时行为与之前一致（回归）
  - `programmatic` TR-9.3: 候选=1时直接返回（无次优选择）
  - `human-judgement` TR-9.4: WHY注释解释新策略的数学依据
- **Notes**: 原策略仅看top-2，当候选>2时均衡效果打折；新策略更通用但仍O(n)

---

### [x] Task 10: auto-dpo单次遍历优化 — 已是最优实现，额外修复AtomicF32→RwLock预存在问题
- **Priority**: low
- **Depends On**: None
- **Description**: 
  - 修改 `crates/auto-dpo/src/generator.rs`，generate()中找max/min用单次遍历（iter().max_by()/min_by()或手动fold）
  - 替代两次遍历的实现
  - 对应 FR-12
- **Acceptance Criteria Addressed**: AC-11
- **Test Requirements**:
  - `programmatic` TR-10.1: 生成结果与之前完全一致（无行为变化）
  - `programmatic` TR-10.2: 所有现有测试通过
- **Notes**: 微优化但代码更简洁；O(n)单次遍历vs两次遍历，常数因子改进

---

### [x] Task 11: csn-substitutor ChainExhausted事件（N16）
- **Priority**: medium
- **Depends On**: Task 3（若需要新事件类型）
- **Description**: 
  - 在event-bus新增ChainExhausted事件（或复用已有事件类型）
  - 修改 `crates/csn-substitutor/src/degradation_chain.rs`，降级链耗尽时发布事件而非仅warn!
  - 增加TDD测试验证事件发布
  - 对应 FR-10
- **Acceptance Criteria Addressed**: AC-11
- **Test Requirements**:
  - `programmatic` TR-11.1: 降级链耗尽时发布事件
  - `programmatic` TR-11.2: warn!日志仍保留（向后兼容）
  - `programmatic` TR-11.3: 事件包含chain_id和last_error信息
  - `human-judgement` TR-11.4: EventSeverity判定为Warning级别
- **Notes**: 消除监控盲区——当前降级链耗尽只有日志，监控系统无法感知

---

## Phase IV: 可选项（经Open Question确认后实施）

---

### [~] Task 12: cosine_similarity热路径优化（N12关联）— 跳过（默认不实施，需bench证明瓶颈）
- **Priority**: low
- **Depends On**: Task 7完成后bench数据支撑
- **Description**: 
  - 评估 `crates/nexus-core/src/clv.rs` 的cosine_similarity_slices()热路径
  - 若bench显示是瓶颈（512-dim被mlc-engine/kvbsr-router/repo-wiki高频调用），在保持#![forbid(unsafe_code)]前提下优化：
    - 循环展开（4x unroll）
    - f32 SIMD通过aligned chunks + std::simd（稳定后）或纯Rust迭代器优化
  - **Open Question待确认**：是否引入target_feature dispatch？默认保持纯标量实现保证可移植性
  - 对应 FR-11
- **Acceptance Criteria Addressed**: AC-11
- **Test Requirements**:
  - `programmatic` TR-12.1: 计算结果与原实现bit-exact一致（proptest验证）
  - `programmatic` TR-12.2: bench显示性能提升>20%才合并，否则保留原实现
  - `programmatic` TR-12.3: 无unsafe代码
  - `human-judgement` TR-12.4: 可移植性优先——不引入target_feature除非bench证明显著收益
- **Notes**: 默认不实施——纯标量实现512-dim约2µs，需bench证明确实是瓶颈才优化

---

### [~] Task 13: NMC Perceptor并行化（N13）— 跳过（占位阶段无收益，延后真实perceptor接入时）
- **Priority**: low
- **Depends On**: None
- **Description**: 
  - 修改 `crates/nmc-encoder/src/fusion.rs`，独立Perceptor的perceive()调用使用tokio::join!并行
  - **Open Question待确认**：当前仅Text/Desktop为真实实现，Image/Video/Audio为占位（~100行stub），并行化收益有限
  - 若占位perceive()是立即返回（无I/O无计算），并行化反而引入调度开销
  - 对应 FR-9
- **Acceptance Criteria Addressed**: AC-11
- **Test Requirements**:
  - `programmatic` TR-13.1: 并行化后融合结果与串行完全一致
  - `programmatic` TR-13.2: bench显示在多模态真实实现场景下有收益
  - `human-judgement` TR-13.3: 若当前占位实现无收益，本任务延后到真实perceptor接入时再做
- **Notes**: 默认不实施——占位阶段并行化无收益，延后到NMC真实化时一并处理

---

### [~] Task 14: gsoe-evolution spawn_blocking（异步边界）— 跳过（YAGNI，种群规模小无需spawn_blocking）
- **Priority**: low
- **Depends On**: None
- **Description**: 
  - 修改 `crates/gsoe-evolution/src/engine.rs`，evaluate_population()等计算密集部分用spawn_blocking包装
  - 对应 FR-13
- **Acceptance Criteria Addressed**: AC-11
- **Test Requirements**:
  - `programmatic` TR-14.1: 所有现有测试通过
  - `programmatic` TR-14.2: 计算密集任务不在async worker线程上执行
  - `human-judgement` TR-14.3: 确认evolve_once()的计算量是否真的足够大到需要spawn_blocking——GRPO种群规模<100时计算量在亚毫秒级，spawn_blocking调度开销(~5µs)可能得不偿失
- **Notes**: 默认不实施——YAGNI原则，种群规模小时spawn_blocking overhead大于收益

---

## Phase V: 文档对齐与经验沉淀

---

### [ ] Task 15: 文档矛盾对齐（FR-14）
- **Priority**: medium
- **Depends On**: None
- **Description**: 
  - 在 `AETHER_NEXUS_OMEGA_ULTIMATE.md` 文件头部添加权威源说明注释：
    > **注意**：本文档为早期设计文档，层级编号（L0-L10）、ADR编号、crate数量（37）与当前实现存在差异。
    > **权威源**：以 `CODE_WIKI.md` 为架构权威源（L1-L10十层、34个crate、ADR-001~010）。
  - 不修改原文内容（保持历史文档完整性），仅添加头部注释
  - 对应 FR-14, AC-10
- **Acceptance Criteria Addressed**: AC-10
- **Test Requirements**:
  - `human-judgement` TR-15.1: ULTIMATE.md头部有清晰的权威源说明
  - `human-judgement` TR-15.2: 原文内容不被修改（历史保留）
  - `human-judgement` TR-15.3: CODE_WIKI.md保持不变（已是权威源）
- **Notes**: 采用注释说明而非修改原文——保留早期设计文档的历史价值，同时消除歧义

---

### [ ] Task 16: project_memory经验沉淀
- **Priority**: medium
- **Depends On**: Task 1-15全部完成
- **Description**: 
  - 将本阶段的关键设计决策和通用模式提炼为project_memory新原则（原则17+）
  - 记录内容：
    - 两阶段审计模式（pre-execution append + post-execution update）的通用适用性
    - 多治理器仲裁的max保守原则
    - f64中间值解决f32精度问题的通用模式
    - Arc共享减少深拷贝的适用场景
    - Hash trait vs serde_json哈希的性能权衡
  - 更新CHANGELOG.md添加v1.5.0-omega汇总章节
- **Acceptance Criteria Addressed**: AC-11
- **Test Requirements**:
  - `human-judgement` TR-16.1: 每条原则是跨场景通用模式，非本项目特定hack
  - `human-judgement` TR-16.2: CHANGELOG汇总章节准确反映本阶段变更
  - `programmatic` TR-16.3: 原则编号连续（17,18,...），不重复不遗漏
- **Notes**: 遵循原则13（checklist以文件实际内容为准，信任子代理报告但必须Read验证）

---

## Phase VI: 条件触发任务状态确认（不实施，仅评估）

---

### [ ] Task 17: M1/M2/M3条件触发再评估
- **Priority**: medium
- **Depends On**: Task 1-16完成
- **Description**: 
  - 重新评估三个条件触发任务的状态：
    - M1向量索引升级：确认当前wiki entries数量（P0已接入wiki_entries_total gauge）
    - M2 RL路由：确认history持久化数据积累情况（P1已实现SqliteHistoryStore）
    - M3配置热重载：确认是否有用户请求或daemon模式计划
  - 更新评估报告，记录下次评估时间点
- **Acceptance Criteria Addressed**: （非功能交付，状态确认）
- **Test Requirements**:
  - `human-judgement` TR-17.1: 评估报告有数据支撑（gauge数值、history条目数）
  - `human-judgement` TR-17.2: 触发条件明确列出，未满足则继续延后
- **Notes**: v1.4.0评估结论：M1 entries<100, M2历史不足, M3无需求；预计v1.5.0仍不满足

---

### [ ] Task 18: 全量验证与交付
- **Priority**: high
- **Depends On**: Task 1-17完成
- **Description**: 
  - 运行完整验证套件：
    - `cargo check --workspace`
    - `cargo test --workspace --jobs 1`（Windows OOM缓解）
    - `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings`
    - `cargo fmt --all -- --check`
  - 生成v1.5.0-omega综合优化报告
  - 确认所有验收标准通过
- **Acceptance Criteria Addressed**: AC-8, AC-9, AC-11
- **Test Requirements**:
  - `programmatic` TR-18.1: cargo check 退出码0
  - `programmatic` TR-18.2: cargo test 全部passed / 0 failed
  - `programmatic` TR-18.3: cargo clippy 零警告
  - `programmatic` TR-18.4: cargo fmt 零diff
  - `human-judgement` TR-18.5: 综合报告包含每个Task的验证结果与bench数据
- **Notes**: 最终交付前的全量门禁
