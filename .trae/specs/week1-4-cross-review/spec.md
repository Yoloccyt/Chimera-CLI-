# Week 1-4 横向深度复审 Spec

> **复审目标**:对 Week 1-4 已实现的 21 个 crate 进行跨周横向深度审计,识别架构漂移、跨层集成缺陷、隐式技术债、并发安全隐患、性能瓶颈、测试盲区与文档失同步问题,为 Week 5 及后续阶段的健康推进奠定基础。

---

## 1. 复审范围

### 1.1 已实现 crate 清单(21 个,按层级分组)

| 层级 | Crate | 源码行数 | 实现周次 |
|------|-------|---------|---------|
| L1 | nexus-core | 1143 | Week 1 |
| L1 | event-bus | 2076 | Week 1 |
| L1 | model-router | 1358 | Week 2 |
| L2 | mlc-engine | 3797 | Week 3 |
| L2 | hcw-window | 2207 | Week 3 |
| L3 | cmt-tiering | 4146 | Week 3 |
| L3 | scc-cache | 1288 | Week 4 |
| L4 | seccore | 1009 | Week 1 |
| L4 | decay-engine | 374 | Week 1 |
| L4 | qeep-protocol | 493 | Week 1 |
| L5 | repo-wiki | 1607 | Week 2 |
| L6 | osa-coordinator | 1538 | Week 3 |
| L6 | kvbsr-router | 2018 | Week 3 |
| L6 | faae-router | 1515 | Week 4 |
| L6 | gqep-executor | 1325 | Week 4 |
| L7 | pvl-layer | 1477 | Week 4 |
| L7 | mtpe-executor | 935 | Week 4 |
| L9 | quest-engine | 1550 | Week 2 |
| L9 | gea-activator | 1428 | Week 4 |
| L10 | chimera-cli | 1349 | Week 1-2 |

**合计**:~33,000 行源码

### 1.2 骨架 crate(13 个,不在复审范围)

acb-governor, auto-dpo, chimera-tui, chtc-bridge, csn-substitutor, decb-governor, efficiency-monitor, gsoe-evolution, lsct-tiering, mcp-mesh, nmc-encoder, parliament, sesa-router, ssra-fusion

### 1.3 基线指标(已验证)

| 指标 | 当前值 | 状态 |
|------|--------|------|
| TODO/FIXME/HACK 计数 | 0 | ✅ 优秀 |
| unsafe 代码块 | 0 | ✅ 优秀 |
| `#![forbid(unsafe_code)]` 覆盖率 | 35/35 crate | ✅ 完整 |
| 生产代码 unwrap/expect/panic | 0(全在 `#[cfg(test)]`) | ✅ 合规 |
| 跨 crate 向上依赖违规 | 0 | ✅ 合规 |

---

## 2. 复审维度与验收标准

### 2.1 维度 A:架构一致性审计

**目标**:验证实际实现与 `AETHER_NEXUS_OMEGA_ULTIMATE.md` 设计文档的一致性。

**检查项**:
- A1: 核心领域类型(UserIntent/Quest/Checkpoint/OmniSparseMasks/SemanticBlock/CLV/NexusState)的字段与设计文档 §10.1 一致性
- A2: 十层架构依赖方向规则(L(N)→L(N-1) 允许,L(N)→L(N+1) 禁止)的实际合规性
- A3: OMEGA 四定律(Ω-Sparse/Ω-Compress/Ω-Evolve/Ω-Event)在代码中的体现
- A4: 25 个 ADR 决策的执行情况(特别是 ADR-003 Event Bus 实现、ADR-004 MessagePack 序列化、ADR-005 SQLite+向量)
- A5: 命名模式(*Coordinator/*Engine/*Router/*Protocol/*Governor/*Mask/*Block)的规范遵循

**验收标准**:列出所有架构漂移点,按严重程度分级(Critical/Major/Minor)。

### 2.2 维度 B:跨层集成审计

**目标**:验证 EventBus 事件契约的完整性与跨层通信的正确性。

**检查项**:
- B1: NexusEvent 枚举变体与 §5.2 数据流参考的覆盖完整性
- B2: 每个事件变体的 source 字段与发布者 crate 的对应关系
- B3: Critical 事件(CheckpointSaved/ConsensusReached)的背压保护策略
- B4: 事件发布与订阅的配对完整性(无孤儿事件、无幽灵订阅)
- B5: EventBus API 使用模式一致性(publish/subscribe/publish_blocking)
- B6: 跨层 API 边界的类型安全(无 String 弱类型传递关键 ID)

**验收标准**:事件流图完整,无孤儿事件,无类型逃逸。

### 2.3 维度 C:技术债与代码质量审计

**目标**:识别隐式技术债(非 TODO 标记的)和代码质量问题。

**检查项**:
- C1: 伪实现/桩函数(如 MTPE 的 pseudo-prediction、SCC 的模拟预取)
- C2: 硬编码常量(应配置化的阈值、超时、容量)
- C3: 错误处理一致性(anyhow vs thiserror 的使用边界、错误传播链)
- C4: 注释完整性(WHY 注释覆盖隐藏约束、变通方案、反直觉行为)
- C5: 函数长度合规性(≤200 行规则)
- C6: 模块组织规范性(lib.rs → types.rs → config.rs → error.rs → 功能模块)

**验收标准**:技术债清单按修复成本(S/M/L)分级,代码质量评分≥4/5。

### 2.4 维度 D:并发与性能审计

**目标**:识别并发安全隐患和性能瓶颈。

**检查项**:
- D1: 锁竞争热点(DashMap/RwLock/Mutex 的持有时间与粒度)
- D2: 死锁风险(锁顺序、async 持锁、跨锁引用)
- D3: async 正确性(所有 async fn 满足 Send + 'static、无遗忘 await)
- D4: 分配热点(不必要的 Vec/HashMap 分配、热路径上的 clone)
- D5: 算法复杂度(Top-K 用 select_nth_unstable、避免 O(n²)、缓存命中率)
- D6: 并发原语选择(FuturesUnordered vs join_all、mpsc vs broadcast)

**验收标准**:无死锁风险,无 async 正确性缺陷,性能热点清单可量化。

### 2.5 维度 E:测试覆盖盲区审计

**目标**:识别测试覆盖的盲区与薄弱环节。

**检查项**:
- E1: 错误路径测试覆盖率(每个 Result 返回点的失败场景)
- E2: 边界条件测试(空输入、最大容量、并发竞争)
- E3: 集成测试缺口(跨 crate 协作场景、E2E 流程)
- E4: proptest 不变量验证的充分性
- E5: 测试隔离性(无共享状态、无顺序依赖)
- E6: 测试代码质量(无过度 mock、无脆弱断言)

**验收标准**:盲区清单按风险分级,关键路径 100% 覆盖。

### 2.6 维度 F:文档同步审计

**目标**:验证文档与代码的一致性。

**检查项**:
- F1: CODE_WIKI.md 模块职责描述与实际实现一致
- F2: CHANGELOG.md 记录与实际功能对应
- F3: crate 级 lib.rs 文档注释与模块功能一致
- F4: spec 文档(spec.md/tasks.md/checklist.md)与实现状态一致
- F5: project_memory.md 教训记录的时效性

**验收标准**:文档失同步点清单,关键差异≤3 个。

---

## 3. 团队组建与职责分配

### 3.1 专家团队(6 名子代理)

| 角色 | 职责 | 负责维度 | 验收产出 |
|------|------|---------|---------|
| 架构师 | 架构一致性与依赖合规 | 维度 A | 架构漂移清单 |
| 集成专家 | 跨层事件契约与 API 边界 | 维度 B | 事件流图与缺陷清单 |
| 代码质量专家 | 技术债识别与代码规范 | 维度 C | 技术债分级清单 |
| 并发专家 | 并发安全与性能瓶颈 | 维度 D | 并发风险与性能热点清单 |
| 测试专家 | 测试覆盖与质量 | 维度 E | 盲区清单与补测建议 |
| 文档专家 | 文档同步与一致性 | 维度 F | 文档失同步清单 |

### 3.2 协作机制

- **并行执行**:维度 A/B/C/D 可并行(独立检查路径),E/F 依赖前四者输出
- **结构化思考**:每个维度采用"假设→验证→结论"流程
- **充分探讨**:发现跨维度问题时,触发跨专家会审
- **严谨验证**:所有结论必须有代码证据(文件:行号)

---

## 4. 任务分解

### Task 1:架构一致性审计(维度 A)
- 1.1 核心领域类型字段一致性检查
- 1.2 十层依赖方向合规性验证
- 1.3 OMEGA 四定律代码体现审计
- 1.4 ADR 决策执行情况核查
- 1.5 命名模式规范遵循检查

### Task 2:跨层集成审计(维度 B)
- 2.1 NexusEvent 变体覆盖完整性
- 2.2 事件 source 字段对应关系验证
- 2.3 Critical 事件背压保护审计
- 2.4 事件发布/订阅配对完整性
- 2.5 EventBus API 使用模式一致性
- 2.6 跨层 API 类型安全审计

### Task 3:技术债与代码质量审计(维度 C)
- 3.1 伪实现/桩函数识别
- 3.2 硬编码常量盘点
- 3.3 错误处理一致性审计
- 3.4 注释完整性评估
- 3.5 函数长度合规性检查
- 3.6 模块组织规范性检查

### Task 4:并发与性能审计(维度 D)
- 4.1 锁竞争热点分析
- 4.2 死锁风险评估
- 4.3 async 正确性验证
- 4.4 分配热点识别
- 4.5 算法复杂度审计
- 4.6 并发原语选择评估

### Task 5:测试覆盖盲区审计(维度 E)
- 5.1 错误路径覆盖率分析
- 5.2 边界条件测试检查
- 5.3 集成测试缺口识别
- 5.4 proptest 充分性评估
- 5.5 测试隔离性验证
- 5.6 测试代码质量评估

### Task 6:文档同步审计(维度 F)
- 6.1 CODE_WIKI.md 一致性检查
- 6.2 CHANGELOG.md 对应性检查
- 6.3 lib.rs 文档注释一致性
- 6.4 spec 文档状态一致性
- 6.5 project_memory.md 时效性

### Task 7:汇总报告与修复建议
- 7.1 跨维度问题汇总
- 7.2 优先级排序(Critical/Major/Minor)
- 7.3 修复成本评估(S/M/L)
- 7.4 Week 5 前置条件确认

---

## 5. 执行原则

1. **证据驱动**:所有结论必须引用具体代码位置(文件:行号)
2. **长期主义**:关注架构健康度,而非短期修复
3. **分布式分析**:各专家独立检查,定期同步发现
4. **多轮验证**:初步发现→交叉验证→最终确认
5. **质量优先**:宁可多花时间,不放过潜在风险

---

## 6. 验收标准

### 6.1 复审完成条件

- [ ] 6 个维度全部完成检查
- [ ] 每个维度产出结构化清单
- [ ] 跨维度问题完成会审
- [ ] 汇总报告生成
- [ ] 优先级修复建议明确

### 6.2 质量要求

- 所有发现可追溯到代码证据
- 无误报(False Positive)
- Critical 级问题零遗漏
- 修复建议可操作(具体到文件和行号)
