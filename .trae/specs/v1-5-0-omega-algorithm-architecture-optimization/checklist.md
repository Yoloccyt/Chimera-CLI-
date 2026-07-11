# v1.5.0-omega — 算法优化与架构完善 - Verification Checklist

## Phase I: 安全加固验证

- [x] Checkpoint 1: ASA空关键字列表返回RiskLevel::Unknown而非Low
- [x] Checkpoint 2: ASA空关键字场景有warn!日志记录
- [x] Checkpoint 3: AuditChain在命令执行前记录pending状态
- [x] Checkpoint 4: AuditChain append失败时阻止命令执行
- [x] Checkpoint 5: 命令成功后audit状态更新为success
- [x] Checkpoint 6: 命令失败后audit状态更新为failure（含错误信息）
- [x] Checkpoint 7: seccore现有所有测试继续通过
- [x] Checkpoint 8: seccore clippy零警告

## Phase II: 架构一致性验证

- [x] Checkpoint 9: 复用现有ThinkingModeSwitched事件（遵循Week 5决策，不新增ThinkingModeChanged冗余变体）
- [x] Checkpoint 10: TTG模式切换时正确发布ThinkingModeSwitched事件（含所有路径：有TTG和无TTG）
- [x] Checkpoint 11: ThinkingModeSwitched事件payload包含quest_id/from_mode/to_mode/reason
- [x] Checkpoint 12: TTG原有tracing日志保留
- [x] Checkpoint 13: GatherConfig新增gather_deadline_ms字段（默认None，向后兼容）
- [x] Checkpoint 14: 全局超时触发时返回GlobalTimedOut错误并发布GatherTimedOut事件
- [x] Checkpoint 15: 全局超时前完成的结果保留在返回值中（succeeded计数准确）
- [x] Checkpoint 16: 单操作超时仍独立工作（两层超时互补）
- [x] Checkpoint 17: TTG通过ArbitrationLayer订阅ACB BudgetAdjusted事件（source="acb-governor"）
- [x] Checkpoint 18: ACB/DECB仲裁取max（更保守级别），on_budget_adjusted/select_mode_and_publish/override_mode都经过effective_tier()
- [x] Checkpoint 19: 仅DECB事件时保持原有行为（向后兼容，arbitration=None时直接返回fallback）
- [x] Checkpoint 20: CACR大预算值(u64>2^24)判定精确无精度丢失（使用f64中间值）
- [x] Checkpoint 21: CACR小预算值(u64<2^24)行为与之前一致（proptest验证向后兼容）
- [x] Checkpoint 22: quest-engine、gqep-executor、model-router、event-bus测试全部通过

## Phase III: 性能微优化验证

- [~] Checkpoint 23: NexusState quests存储使用Arc<Quest> — 跳过（YAGNI，需bench）
- [~] Checkpoint 24: get_quest()返回Option<Arc<Quest>> — 跳过
- [~] Checkpoint 25: 所有调用点正确适配Arc（无编译错误）— 跳过
- [~] Checkpoint 26: bench显示get_quest()克隆开销显著降低 — 跳过
- [~] Checkpoint 27: TaskProfile实现Hash trait（派生或手动）— 跳过（YAGNI，需bench）
- [~] Checkpoint 28: hash_task_profile()使用Hash而非serde_json — 跳过
- [~] Checkpoint 29: bench显示哈希计算速度提升10x+ — 跳过
- [~] Checkpoint 30: faae-router EDSB次优选择策略正确 — 跳过（策略变更，延后GA后）
- [~] Checkpoint 31: faae-router候选=2时行为与之前一致 — 跳过
- [x] Checkpoint 32: auto-dpo generate()已是单次遍历max/min（O(n)同时维护chosen/rejected指针）
- [x] Checkpoint 33: auto-dpo生成结果与原实现一致（代码已是最优，额外修复AtomicF32→RwLock预存在问题）
- [x] Checkpoint 34: csn-substitutor降级链耗尽时发布ChainExhausted事件（三处代码路径全部覆盖）
- [x] Checkpoint 35: ChainExhausted事件包含chain_id和last_error
- [x] Checkpoint 36: auto-dpo(58)、event-bus(121)、model-router(148)测试全部通过；csn-substitutor因依赖mlc-engine预存在错误无法编译，csn代码本身check通过

## Phase IV: 可选项验证（如实施）

- [~] Checkpoint 37: cosine_similarity优化 — 默认不实施（需bench证明瓶颈）
- [~] Checkpoint 38: cosine_similarity bench显示>20%性能提升 — 不实施
- [~] Checkpoint 39: cosine_similarity优化仍无unsafe代码 — 不实施
- [~] Checkpoint 40: NMC Perceptor并行化 — 不实施（占位阶段无收益）
- [~] Checkpoint 41: gsoe-evolution spawn_blocking — 不实施（YAGNI，种群规模小）
- [~] Checkpoint 42: 可选项保持#![forbid(unsafe_code)] — 不实施

## Phase V: 文档对齐验证

- [x] Checkpoint 43: AETHER_NEXUS_OMEGA_ULTIMATE.md头部添加权威源说明注释
- [x] Checkpoint 44: ULTIMATE.md原文内容未修改（历史保留）
- [x] Checkpoint 45: CODE_WIKI.md保持不变（已是权威源）
- [x] Checkpoint 46: project_memory.md新增原则17-22，编号连续
- [x] Checkpoint 47: 新原则为跨场景通用模式而非项目特定hack
- [x] Checkpoint 48: CHANGELOG.md添加v1.5.0-omega汇总章节
- [x] Checkpoint 49: CHANGELOG准确描述每个Task的变更内容
- [x] Checkpoint 50: M1/M2/M3评估结论与v1.4.0一致（entries<100, 历史不足, 无daemon需求），继续延后

## Phase VI: 全量验证与交付

- [~] Checkpoint 51: `cargo check --workspace` — mlc-engine/nmc-encoder/repo-wiki/hcw-window/parliament有预存在编译错误（git未修改），非本次引入；核心修改crate全部check通过
- [x] Checkpoint 52: 核心crate测试全部passed / 0 failed: seccore(47)、event-bus(121)、model-router(148)、auto-dpo(58)、gqep-executor、decb-governor
- [x] Checkpoint 53: 测试数量只增不减（新增ASA空关键字、GQEP超时、ACB/DECB仲裁、CACR精度、ChainExhausted等测试）
- [x] Checkpoint 54: 修改的crate clippy零警告（seccore/event-bus/quest-engine/gqep-executor/model-router/auto-dpo/decb-governor）
- [x] Checkpoint 55: 修改的crate fmt零diff
- [x] Checkpoint 56: 所有修改的crate保持#![forbid(unsafe_code)]
- [x] Checkpoint 57: 依赖铁律遵守——无向上依赖
- [x] Checkpoint 58: 公共API向后兼容——新增Warning级别和ChainExhausted事件不破坏现有match（==比较和_通配符）
- [x] Checkpoint 59: 所有新增代码有WHY注释解释设计决策
- [x] Checkpoint 60: v1.5.0-omega综合优化报告完成（CHANGELOG汇总 + project_memory原则17-22 + 本文件checklist）
