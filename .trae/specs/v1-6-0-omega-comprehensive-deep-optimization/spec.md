# v1.6.0-omega — 全量分布式深度优化与创新演进 Spec

## Why

v1.5.0-omega 阶段完成了 8 项核心修复（安全加固 2 + 架构一致性 4 + 精度修复 1 + 监控盲区 1），但 `cargo check --workspace` 仍有 5 个 crate 存在预存在编译错误（mlc-engine/nmc-encoder/repo-wiki/hcw-window/parliament），且 DEEP_RESEARCH 报告中 26 项优化方案仍有 15+ 项未实施，6 项 v1.5.0 YAGNI 跳过的优化需基于 bench 数据重新评估。本阶段以"先修复编译基线 → 再完成 P0/P1 剩余项 → 再深化创新点 → 最后性能微调"的长期主义路线推进，为 GA 后持续演进清除最后的技术债障碍。

## What Changes

### Phase I: 编译基线修复（P0 阻塞项）
- 修复 mlc-engine 预存在编译错误（L2 四级记忆引擎）
- 修复 nmc-encoder 预存在编译错误（L2 多模态编码器）
- 修复 repo-wiki 预存在编译错误（L5 知识管理）
- 修复 hcw-window 预存在编译错误（L2 分层上下文窗口）
- 修复 parliament 预存在编译错误（L8 治理决策）
- 确保 `cargo check --workspace` 退出码 0

### Phase II: DEEP_RESEARCH 剩余 P0/P1 修复
- **A3 WikiStore 异步读写分离**：单 Mutex<Connection> → 读写分离（r2d2 连接池或 RwLock 双连接）
- **B3 ModelRegistry DashMap→RwLock**：≤10 模型场景下 RwLock 分片锁开销更低
- **D1 SQLite 读写分离连接池**：cmt-tiering/scc-cache 的单 Mutex<Connection> → 连接池
- **I4 优先级残差事件流**：Normal/Critical 二级 → Normal/Warning/Critical/Priority 四级优先队列
- **N9 L6 路由链路顺序保证**：五层路由通过事件解耦，增加代码级顺序保证机制
- **G1 AuditChain 并发化**：Mutex 串行 → DashMap 并发审计记录

### Phase III: v1.5.0 YAGNI 重新评估（需 bench 数据支撑）
- **Task 7 NexusState Arc 共享**：bench 验证深拷贝开销，若 > 100ns 则实施 Arc<Quest>
- **Task 8 TaskProfile Hash trait**：bench 验证 serde_json 序列化是否为瓶颈
- **Task 9 EDSB 次优选择**：实施"非最热候选中相似度最高的"策略
- **Task 12 cosine_similarity 优化**：bench 验证是否为热路径瓶颈，若 > 20% 提升则实施
- **Task 13 NMC Perceptor 并行化**：评估占位 perceptor 并行化收益
- **Task 14 gsoe spawn_blocking**：评估种群规模是否足够大

### Phase IV: OMEGA 魔改创新深化
- **I1 MoE 稀疏门控深化**：model-router MoeGate 从二维评分扩展到五维（已部分完成 v1.3.0 S2）
- **I3 Speculative DAG 执行**：quest-engine 线性链 → 真 DAG 分解 + 投机执行
- **I5 分层潜在上下文压缩**：CLV 从固定 512-dim → 分层压缩（L0=512 / L1=256 / L2=128）
- **I6 GRPO 自适应任务评分**：gsoe-evolution 从规则式 → 组内相对比较评分
- **I7 OS-Memory Wiki 元遗忘**：repo-wiki 增加遗忘曲线 + 重要性衰减
- **I8 CISPO 非对称预算控制**：model-router CACR 从双阈值线性 → 非对称预算控制
- **I9 主动安全不变量**：seccore 增加 QK-Clip 主动安全不变量检查

### Phase V: 性能微优化（需 bench 支撑）
- **clone 减少**：mlc-engine(63 处)、cmt-tiering(45 处)、scc-cache(42 处) 热路径深拷贝优化
- **B1 VectorIndex Mutex→RwLock**：若未修复则修复（允许并发 KNN 搜索）
- **C2 双格式序列化完成**：msgpack/json 自动选择策略
- **G2 Prometheus 指标扩展**：repo-wiki 已接入，扩展到 event-bus/efficiency-monitor
- **heuristic_scores() 实现**：osa-coordinator 占位符 → 真实评分函数

### Phase VI: 文档对齐与经验沉淀
- 更新 CODE_WIKI.md 反映 v1.6.0 变更
- 更新 CHANGELOG.md 添加 v1.6.0-omega 汇总章节
- 沉淀 project_memory 新原则（原则 23+）

### Phase VII: 全量验证与交付
- `cargo check --workspace` 退出码 0
- `cargo test --workspace` 全部 passed / 0 failed
- `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 零警告
- `cargo fmt --all -- --check` 零 diff
- `cargo audit --deny warnings` 通过

## Impact
- **Affected specs**: v1.5.0-omega（前置基线）、v1.4.0-omega（P1 历史持久化）、v1.3.0-omega（S1/S2/S3 优化）
- **Affected code**: 全部 35 crate（编译基线修复涉及 5 crate，P0/P1 修复涉及 10+ crate，创新深化涉及 8+ crate）
- **Affected docs**: CODE_WIKI.md、CHANGELOG.md、project_memory.md、ULTIMATE.md
- **Risk**: 编译基线修复可能暴露更多隐藏问题；创新深化涉及核心算法变更需 ADR 记录

## ADDED Requirements

### Requirement: 编译基线零错误
系统 SHALL 确保 `cargo check --workspace` 退出码为 0，无任何编译错误。

#### Scenario: 全量编译检查
- **WHEN** 执行 `cargo check --workspace`
- **THEN** 退出码 0，无 E0428/E0252/E0277/E0599 等错误

#### Scenario: 单 crate 编译检查
- **WHEN** 对任意 crate 执行 `cargo check -p <name>`
- **THEN** 退出码 0，可独立编译

### Requirement: WikiStore 异步读写分离
系统 SHALL 将 WikiStore 的单 Mutex<Connection> 改为读写分离架构，支持并发读操作。

#### Scenario: 并发读场景
- **GIVEN** 多个并发读请求
- **WHEN** 同时查询 WikiStore
- **THEN** 读操作不相互阻塞，吞吐量提升

### Requirement: ModelRegistry 并发优化
系统 SHALL 在模型数量 ≤ 阈值时使用 RwLock 替代 DashMap，减少分片锁开销。

#### Scenario: 少量模型场景
- **GIVEN** 注册模型数 ≤ 10
- **WHEN** 并发路由查询
- **THEN** 使用 RwLock 读锁并发，无分片锁开销

### Requirement: L6 路由链路顺序保证
系统 SHALL 通过代码级机制（而非约定）保证五层路由的执行顺序。

#### Scenario: 路由顺序验证
- **GIVEN** 一个用户请求进入 L6 路由链路
- **WHEN** OSA → KVBSR → FaaE → SESA → GEA 依次执行
- **THEN** 顺序由代码强制保证，不依赖事件到达顺序

### Requirement: Speculative DAG 执行
系统 SHALL 将 Quest 任务分解为真 DAG 结构（含并行分支），支持投机执行。

#### Scenario: 并行任务分解
- **GIVEN** 一个可分解为并行子任务的 Quest
- **WHEN** DAG 分解器处理
- **THEN** 产出含分支的 DAG 而非线性链

### Requirement: 优先级残差事件流
系统 SHALL 支持四级事件优先级（Normal/Warning/Critical/Priority），Priority 级事件优先投递。

#### Scenario: Priority 事件优先投递
- **GIVEN** 一个 Priority 级事件（如 SkepticVeto）
- **WHEN** 事件总线调度
- **THEN** 优先于 Normal/Warning 级事件投递

## MODIFIED Requirements

### Requirement: CLV 分层压缩
CLV 从固定 512-dim 扩展为分层压缩：L0=512-dim（原始）/ L1=256-dim（压缩）/ L2=128-dim（高压缩），按上下文窗口层级选择。

### Requirement: GRPO 进化评分
gsoe-evolution 从规则式评分扩展为 GRPO 风格组内相对比较评分，支持自适应任务评分。

### Requirement: CACR 非对称预算控制
CACR 从双阈值线性判定扩展为非对称预算控制（CISPO 启发），支持不同维度的独立预算阈值。

### Requirement: AuditChain 并发化
AuditChain 从 Mutex 串行改为 DashMap 并发审计记录，支持高并发审计场景。

## REMOVED Requirements
（本阶段无移除需求）

## Open Questions
- [ ] WikiStore 读写分离使用 r2d2 连接池还是 RwLock 双连接？r2d2 引入新依赖，RwLock 双连接无新依赖但需管理一致性
- [ ] Speculative DAG 的投机执行策略：激进投机（全部并行）还是保守投机（仅独立分支并行）？
- [ ] CLV 分层压缩是否破坏现有 cosine_similarity_slices 接口？需 ADR 记录类型变更
- [ ] GRPO 评分的历史数据来源：使用 model-router HistoryStore 还是 gsoe 内部积累？
- [ ] 5 个 crate 的预存在编译错误根因是什么？是 v1.5.0 引入的回归还是历史遗留？
