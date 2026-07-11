# v1.5.0-omega — 算法优化与架构完善 - Product Requirement Document

## Overview
- **Summary**: 基于 v1.4.0-omega 已完成基础（P0监控+P1历史持久化+所有Critical/P0 bugfix），系统性地对34个crate进行剩余安全加固、架构一致性完善、热路径性能微优化、以及文档对齐。遵循OMEGA四定律（Ω-Sparse/Ω-Compress/Ω-Evolve/Ω-Event）与第二阶段开发原则（YAGNI、TDD守恒、长期主义），所有修改保持向后兼容且不破坏现有测试。
- **Purpose**: DEEP_RESEARCH报告中的Critical/P0问题已在v1.1.0-v1.4.0期间全部修复，但仍有若干High/P1级正确性问题、架构不一致性、热路径优化机会需要系统性处理，以进一步提升代码质量、安全性、性能基准，为GA后持续演进建立更稳固的工程基础。
- **Target Users**: Chimera CLI开发者、运维人员、下游集成方

## Goals
- 修复剩余High级安全问题（ASA空关键字绕过、AuditChain审计完整性）
- 修复架构一致性问题（ACB/DECB双治理器仲裁、GQEP全局超时、TTG EventBus集成）
- 优化热路径性能点（NexusState Arc共享、GEA Hash trait、cosine_similarity SIMD等）
- 对齐文档矛盾，消除两套ADR/层级编号冲突
- 保持100%向后兼容，现有3400+测试全部通过
- 沉淀新的project_memory原则

## Non-Goals (Out of Scope)
- M1向量索引升级（触发条件未满足：entries < 100, KNN p95 < 1ms）
- M2 RL路由策略（历史数据不足，需生产环境积累）
- M3配置热重载（无daemon模式，无用户请求）
- MTPE/PVL/NMC Image/Video/Audio真实模型接入（依赖外部模型服务）
- MCP 2PC分布式真实化（工作量大，非本阶段范围）
- gVisor跨平台沙箱（平台限制，ADR-001已降级记录）
- 大规模架构重构（RC/GA后演进阶段不允许跨层重构）

## Background & Context
- **当前版本**: v1.4.0-omega
- **测试基线**: 3416+ passed / 0 failed / 56 ignored
- **已完成修复**: DEEP_RESEARCH报告中N1(Critical cmd.exe绕过)、N2(P0 SSRA主导策略)、N3(P0 QEEP Ack)、A1(P0 checkpoint spawn_blocking)、B1(VectorIndex RwLock)、N7(ACB滞后)、N8(Skeptic覆议)、N10(马尔可夫链容量)、C1(EventTopic过滤)、F1(配置统一)、E1(懒加载)、N15(FTS5 trigram)均已修复
- **参考文档**: OMEGA_大模型架构魔改创新v3.0.0、从零搭建完全指南v2.0.0、CODE_WIKI.md、DEEP_RESEARCH_34CRATE_OPTIMIZATION.md
- **project_memory已有16条原则**（原则1-16），本阶段将继续沉淀跨场景通用模式

## Functional Requirements
- **FR-1**: ASA审计在风险关键字为空列表时不返回Low风险，必须升级为Unknown并触发额外审计
- **FR-2**: AuditChain改为pre-execution append（执行前记录意图），命令完成后更新状态而非事后追加
- **FR-3**: TTG(思考切换治理)订阅ACB BudgetAdjusted事件，引入仲裁层融合ACB(Token预算)与DECB(认知预算)
- **FR-4**: GQEP增加gather_deadline_ms全局超时，防止N个操作各耗90%超时导致总耗时累积
- **FR-5**: TTG模式切换事件从tracing::info迁移到EventBus，发布ThinkingModeChanged事件供订阅者响应
- **FR-6**: NexusState的get_quest()返回Arc<Quest>共享引用而非深拷贝，减少热路径堆分配
- **FR-7**: gea-activator的TaskProfile实现Hash trait替代serde_json序列化哈希计算
- **FR-8**: faae-router的EDSB次优选择从top-2扩展为"非最热候选中相似度最高的"，提升均衡效果
- **FR-9**: nmc-encoder的多模态perceive()支持并行化（tokio::join!处理独立Perceptor）
- **FR-10**: csn-substitutor的降级链耗尽时发布ChainExhausted事件而非仅warn日志，消除监控盲区
- **FR-11**: cosine_similarity_slices()在保持纯Rust无unsafe的前提下优化热路径（循环展开+target_feature dispatch）
- **FR-12**: auto-dpo的generate()单次遍历找max/min，替代两次遍历
- **FR-13**: gsoe-evolution的计算密集部分（evaluate_population等）使用spawn_blocking包装
- **FR-14**: 文档对齐——OMEGA_ULTIMATE.md层级编号统一为CODE_WIKI的L1-L10，ADR编号冲突加注释说明以CODE_WIKI为权威源
- **FR-15**: CACR预算计算在u64>2^24时保持精度（整数运算或f64中间值），避免f32阈值判定误差

## Non-Functional Requirements
- **NFR-1**: 所有现有测试必须继续通过，零回归
- **NFR-2**: clippy零警告，fmt零diff
- **NFR-3**: 向后兼容——所有公共API签名不变，默认行为不变
- **NFR-4**: 热路径优化必须有bench数据支撑，禁止无证据的"优化"
- **NFR-5**: 每个bugfix必须先写失败测试（TDD RED-GREEN）
- **NFR-6**: 新代码必须包含WHY注释解释设计决策（参照已有代码风格）
- **NFR-7**: unsafe_code禁令保持——所有crate继续`#![forbid(unsafe_code)]`
- **NFR-8**: 依赖铁律遵守——L(N)→L(N-1)允许，向上依赖禁止

## Constraints
- **Technical**: Rust 2021 edition, Tokio async, workspace 34 crates, #![forbid(unsafe_code)]
- **Business**: GA后演进阶段，禁止跨层重构、禁止新crate（除非ADR审批）、核心领域类型变更需ADR
- **Dependencies**: 只能使用workspace已有依赖，禁止新增第三方crate（除非绝对必要且经审批）
- **Timeline**: 分阶段实施（安全→正确性→性能→文档），每个阶段独立验证

## Assumptions
- v1.4.0测试基线全部通过（cargo test --workspace exit 0）
- 所有P0/Critical问题已修复，剩余问题均为P1/P2级
- M1/M2/M3条件触发任务在本阶段不实施，仅做评估确认
- 向后兼容是硬约束——任何API break必须有re-export层保持兼容

## Acceptance Criteria

### AC-1: ASA空关键字安全加固
- **Given**: ASA审计器配置了空风险关键字列表
- **When**: 审计一个命令
- **Then**: 返回RiskLevel::Unknown而非Low，触发额外审计日志记录
- **Verification**: `programmatic`
- **Notes**: 对应DEEP_RESEARCH N4

### AC-2: AuditChain前置审计记录
- **Given**: SecCore准备执行一个命令
- **When**: 调用audit_chain.append()
- **Then**: 在命令执行前记录意图（pending状态），执行后更新为success/failure
- **Verification**: `programmatic`
- **Notes**: 对应DEEP_RESEARCH N5；append失败时阻止命令执行

### AC-3: ACB/DECB双治理器仲裁
- **Given**: ACB和DECB同时发布BudgetAdjusted事件
- **When**: TTG接收事件
- **Then**: 仲裁层融合两个预算维度，选择更保守的（更高的思考级别）
- **Verification**: `programmatic`
- **Notes**: 对应DEEP_RESEARCH N6；不破坏现有DECB订阅逻辑

### AC-4: GQEP全局超时
- **Given**: GQEP gather配置了gather_deadline_ms
- **When**: 整体聚集时间超过deadline
- **Then**: 返回GatherTimeout错误，已完成的操作结果保留
- **Verification**: `programmatic`
- **Notes**: 对应DEEP_RESEARCH N14

### AC-5: TTG EventBus集成
- **Given**: TTG切换思考模式（Fast↔Standard↔Deep）
- **When**: 模式变更发生
- **Then**: 发布ThinkingModeChanged事件到EventBus，同时保留tracing日志
- **Verification**: `programmatic`
- **Notes**: 对应DEEP_RESEARCH N18

### AC-6: NexusState Arc共享
- **Given**: NexusState包含多个Quest
- **When**: 调用get_quest()
- **Then**: 返回Arc<Quest>而非深拷贝Quest，调用方获得共享引用
- **Verification**: `programmatic`
- **Notes**: 对应DEEP_RESEARCH N12；bench验证减少克隆开销

### AC-7: TaskProfile Hash trait
- **Given**: gea-activator需要哈希TaskProfile
- **When**: 计算hash_task_profile
- **Then**: 使用派生的Hash trait而非serde_json序列化，bench验证性能提升
- **Verification**: `programmatic`
- **Notes**: 对应DEEP_RESEARCH N17

### AC-8: 所有现有测试通过
- **Given**: 本阶段所有代码修改完成
- **When**: 运行cargo test --workspace
- **Then**: 全部passed / 0 failed，与v1.4.0基线一致或更好
- **Verification**: `programmatic`

### AC-9: clippy零警告
- **Given**: 所有代码修改完成
- **When**: 运行cargo clippy --workspace --all-targets --jobs 2 -- -D warnings
- **Then**: 退出码0，零警告
- **Verification**: `programmatic`

### AC-10: 文档一致性
- **Given**: 文档对齐完成
- **When**: 交叉比对OMEGA_ULTIMATE.md与CODE_WIKI.md
- **Then**: 层级编号统一为L1-L10，ADR编号冲突处标注以CODE_WIKI为权威
- **Verification**: `human-judgment`

### AC-11: 向后兼容验证
- **Given**: 所有FR实施完成
- **When**: 检查公共API签名与v1.4.0对比
- **Then**: 无break change，所有pub use路径保持可用
- **Verification**: `programmatic` + `human-judgment`

## Open Questions
- [ ] cosine_similarity是否需要引入target_feature dispatch（运行时CPU检测AVX2/SSE），还是保持纯标量实现保证可移植性？
- [ ] ACB/DECB仲裁层的具体融合策略：简单取max（更保守）还是加权融合？
- [ ] N13 NMC Perceptor并行化是否会引入不必要的复杂度（当前仅Text/Desktop为真实实现，其余为占位）？
- [ ] 文档对齐是否需要修改OMEGA_ULTIMATE.md原文，还是仅添加注释说明以CODE_WIKI为准？
