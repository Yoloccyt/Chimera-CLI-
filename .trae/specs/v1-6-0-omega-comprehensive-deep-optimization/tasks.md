# v1.6.0-omega — 全量分布式深度优化与创新演进 - The Implementation Plan

## Phase I: 编译基线修复（P0 阻塞项，最高优先级）

---

### [x] Task 1: 诊断并修复 mlc-engine 预存在编译错误
- **Priority**: P0
- **Depends On**: None
- **Description**: 
  - 运行 `cargo check -p mlc-engine` 诊断编译错误根因
  - 修复所有编译错误（可能是类型不匹配、缺失导入、API 变更回归等）
  - 运行 `cargo test -p mlc-engine` 确保测试通过
  - 运行 `cargo clippy -p mlc-engine --all-targets -- -D warnings` 确保零警告
- **Acceptance Criteria**: `cargo check -p mlc-engine` 退出码 0
- **Test Requirements**:
  - `programmatic` TR-1.1: `cargo check -p mlc-engine` 退出码 0
  - `programmatic` TR-1.2: `cargo test -p mlc-engine` 全部 passed / 0 failed
  - `programmatic` TR-1.3: `cargo clippy -p mlc-engine -- -D warnings` 零警告
- **Notes**: mlc-engine 是 L2 四级记忆引擎（4540 LOC），预存在错误可能源自 v1.5.0 API 变更回归

---

### [x] Task 2: 诊断并修复 nmc-encoder 预存在编译错误
- **Priority**: P0
- **Depends On**: None
- **Description**: 
  - 运行 `cargo check -p nmc-encoder` 诊断编译错误根因
  - 修复所有编译错误
  - 运行 `cargo test -p nmc-encoder` 确保测试通过
- **Acceptance Criteria**: `cargo check -p nmc-encoder` 退出码 0
- **Test Requirements**:
  - `programmatic` TR-2.1: `cargo check -p nmc-encoder` 退出码 0
  - `programmatic` TR-2.2: `cargo test -p nmc-encoder` 全部 passed
  - `programmatic` TR-2.3: clippy 零警告
- **Notes**: nmc-encoder 是 L2 多模态编码器（1784 LOC），含 Image/Video/Audio 占位 perceptor

---

### [x] Task 3: 诊断并修复 repo-wiki 预存在编译错误（已完成：6 处 clippy redundant_closure 修复 + vector.rs Mutex→RwLock 调整）
- **Priority**: P0
- **Depends On**: None
- **Description**: 
  - 运行 `cargo check -p repo-wiki` 诊断编译错误根因
  - 修复所有编译错误
  - 运行 `cargo test -p repo-wiki` 确保测试通过
- **Acceptance Criteria**: `cargo check -p repo-wiki` 退出码 0
- **Test Requirements**:
  - `programmatic` TR-3.1: `cargo check -p repo-wiki` 退出码 0
  - `programmatic` TR-3.2: `cargo test -p repo-wiki` 全部 passed
  - `programmatic` TR-3.3: clippy 零警告
- **Notes**: repo-wiki 是 L5 知识管理（2025 LOC），含 FTS5 三级降级链（v1.3.0 S3）

---

### [x] Task 4: 诊断并修复 hcw-window 预存在编译错误
- **Priority**: P0
- **Depends On**: None
- **Description**: 
  - 运行 `cargo check -p hcw-window` 诊断编译错误根因
  - 修复所有编译错误
  - 运行 `cargo test -p hcw-window` 确保测试通过
- **Acceptance Criteria**: `cargo check -p hcw-window` 退出码 0
- **Test Requirements**:
  - `programmatic` TR-4.1: `cargo check -p hcw-window` 退出码 0
  - `programmatic` TR-4.2: `cargo test -p hcw-window` 全部 passed
  - `programmatic` TR-4.3: clippy 零警告
- **Notes**: hcw-window 是 L2 分层上下文窗口（2639 LOC），四级窗口 4K/32K/128K/1M

---

### [x] Task 5: 诊断并修复 parliament 预存在编译错误
- **Priority**: P0
- **Depends On**: None
- **Description**: 
  - 运行 `cargo check -p parliament` 诊断编译错误根因
  - 修复所有编译错误
  - 运行 `cargo test -p parliament` 确保测试通过
- **Acceptance Criteria**: `cargo check -p parliament` 退出码 0
- **Test Requirements**:
  - `programmatic` TR-5.1: `cargo check -p parliament` 退出码 0
  - `programmatic` TR-5.2: `cargo test -p parliament` 全部 passed
  - `programmatic` TR-5.3: clippy 零警告
- **Notes**: parliament 是 L8 治理决策（5870 LOC），5 角色加权投票 + AHIRT 红队

---

### [ ] Task 6: 全量编译基线验证
- **Priority**: P0
- **Depends On**: Task 1, 2, 3, 4, 5
- **Description**: 
  - 运行 `cargo check --workspace` 确认全部 35 crate 编译通过
  - 运行 `cargo test --workspace --jobs 1` 确认测试基线（Windows OOM 缓解）
  - 记录基线测试数量（应为 3400+ passed）
- **Acceptance Criteria**: `cargo check --workspace` 退出码 0
- **Test Requirements**:
  - `programmatic` TR-6.1: `cargo check --workspace` 退出码 0
  - `programmatic` TR-6.2: `cargo test --workspace --jobs 1` 全部 passed / 0 failed
  - `programmatic` TR-6.3: 测试数量 >= v1.5.0 基线（3400+）

---

## Phase II: DEEP_RESEARCH 剩余 P0/P1 修复

---

### [x] Task 7: WikiStore 异步读写分离（A3）— 已实现（mpsc 写线程 + 读连接池）
- **Priority**: P0
- **Depends On**: Task 3（repo-wiki 编译基线修复）
- **Description**: 
  - 修改 `crates/repo-wiki/src/store.rs`，将单 `Mutex<Connection>` 改为读写分离架构
  - 方案选择（需 Open Question 确认）：RwLock 双连接（无新依赖）或 r2d2 连接池
  - 写操作仍走 Mutex，读操作走 RwLock 读锁并发
  - 增加 bench 验证并发读吞吐量提升
  - 增加 TDD 测试验证并发读不阻塞
- **Acceptance Criteria**: 并发读吞吐量提升 ≥ 2x
- **Test Requirements**:
  - `programmatic` TR-7.1: 多个并发读操作不相互阻塞
  - `programmatic` TR-7.2: 写操作仍正确串行化
  - `programmatic` TR-7.3: bench 显示并发读吞吐量提升
  - `programmatic` TR-7.4: 所有现有 repo-wiki 测试通过

---

### [x] Task 8: ModelRegistry DashMap→RwLock（B3）— 已实现（RwLock<HashMap> + entry() API）
- **Priority**: P1
- **Depends On**: Task 6（全量编译基线验证）
- **Description**: 
  - 修改 `crates/model-router/src/registry.rs`，将 `DashMap<String, ModelInfo>` 改为 `RwLock<HashMap<String, ModelInfo>>`
  - 读操作（list/get/route）走 RwLock 读锁并发
  - 写操作（register/unregister）走 RwLock 写锁
  - 修复 `register()` 的 TOCTOU 竞态（`contains_key` + `insert` → `entry()` API）
  - 增加 bench 验证 ≤10 模型场景性能提升
- **Acceptance Criteria**: ≤10 模型场景下路由查询延迟降低
- **Test Requirements**:
  - `programmatic` TR-8.1: 并发路由查询走读锁不阻塞
  - `programmatic` TR-8.2: register 无 TOCTOU 竞态
  - `programmatic` TR-8.3: bench 显示 ≤10 模型场景性能提升
  - `programmatic` TR-8.4: 所有现有 model-router 测试通过

---

### [ ] Task 9: SQLite 连接池（D1）
- **Priority**: P1
- **Depends On**: Task 6
- **Description**: 
  - 为 cmt-tiering 和 scc-cache 的 SQLite 连接实现连接池（或 RwLock 双连接）
  - cmt-tiering 的 warm.rs 和 cold.rs 各自维护独立 `Arc<Mutex<Connection>>` → 共享连接池
  - scc-cache 的 WAL 操作从单 Mutex → 读写分离
  - 增加 bench 验证并发性能提升
- **Acceptance Criteria**: 并发 SQLite 操作吞吐量提升
- **Test Requirements**:
  - `programmatic` TR-9.1: 并发 SQLite 读操作不阻塞
  - `programmatic` TR-9.2: 写操作仍正确串行化
  - `programmatic` TR-9.3: 所有现有 cmt-tiering/scc-cache 测试通过

---

### [ ] Task 10: 优先级残差事件流（I4）
- **Priority**: P1
- **Depends On**: Task 6
- **Description**: 
  - 修改 `crates/event-bus/src/types.rs`，新增 `EventPriority::Priority` 级别
  - 修改 event-bus 的 mpsc 通道为优先级队列（Priority > Critical > Warning > Normal）
  - SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded 升级为 Priority 级
  - 增加 TDD 测试验证优先级投递
  - 需 ADR-011 记录事件优先级设计决策
- **Acceptance Criteria**: Priority 级事件优先于 Normal/Warning 投递
- **Test Requirements**:
  - `programmatic` TR-10.1: Priority 级事件优先投递
  - `programmatic` TR-10.2: Critical 级事件仍确保投递
  - `programmatic` TR-10.3: 向后兼容——现有 Normal 级事件行为不变
  - `human-judgement` TR-10.4: ADR-011 记录设计决策

---

### [ ] Task 11: L6 路由链路顺序保证（N9）
- **Priority**: P1
- **Depends On**: Task 6
- **Description**: 
  - 在 OSA/KVBSR/FaaE/SESA/GEA 五层路由间建立代码级顺序保证机制
  - 方案：引入 `RoutingPipeline` 结构，按顺序调用各路由层，而非依赖事件到达顺序
  - 增加 TDD 测试验证路由顺序
- **Acceptance Criteria**: 五层路由顺序由代码强制保证
- **Test Requirements**:
  - `programmatic` TR-11.1: 路由顺序为 OSA → KVBSR → FaaE → SESA → GEA
  - `programmatic` TR-11.2: 顺序不依赖事件到达顺序
  - `programmatic` TR-11.3: 所有现有路由测试通过

---

### [ ] Task 12: AuditChain 并发化（G1）
- **Priority**: P2
- **Depends On**: Task 6
- **Description**: 
  - 修改 `crates/seccore/src/audit.rs`，将 `Mutex<Vec<AuditEntry>>` 改为 `DashMap<u64, AuditEntry>` 或 `RwLock<Vec<AuditEntry>>`
  - 并发 append 不阻塞读操作
  - 增加 TDD 测试验证并发审计
- **Acceptance Criteria**: 并发审计吞吐量提升
- **Test Requirements**:
  - `programmatic` TR-12.1: 并发 append 不相互阻塞
  - `programmatic` TR-12.2: 审计链完整性不变
  - `programmatic` TR-12.3: 所有现有 seccore 测试通过

---

## Phase III: v1.5.0 YAGNI 重新评估（需 bench 数据支撑）

---

### [ ] Task 13: NexusState Arc 共享重新评估（v1.5.0 Task 7）
- **Priority**: P2
- **Depends On**: Task 6
- **Description**: 
  - 编写 bench 测量 `get_quest()` 深拷贝开销（Quest 含 Vec<Task>/Checkpoint 等大结构）
  - 若 bench 显示 > 100ns 且被热路径高频调用，则实施 `HashMap<QuestId, Arc<Quest>>` 改造
  - 若 bench 显示 < 100ns 或调用频率低，记录评估结论继续延后
- **Acceptance Criteria**: 有 bench 数据支撑的 go/no-go 决策
- **Test Requirements**:
  - `programmatic` TR-13.1: bench 报告含深拷贝延迟数据
  - `programmatic` TR-13.2: 若实施，所有调用点正确适配 Arc
  - `human-judgement` TR-13.3: 决策有 bench 数据支撑

---

### [ ] Task 14: TaskProfile Hash trait 重新评估（v1.5.0 Task 8）
- **Priority**: P2
- **Depends On**: Task 6
- **Description**: 
  - 编写 bench 测量 `hash_task_profile()` 的 serde_json 序列化开销
  - 若 bench 显示是瓶颈（128-capacity 缓存场景下 > 1µs），则为 TaskProfile 派生 Hash trait
  - 若 bench 显示 < 1µs 或非瓶颈，记录评估结论继续延后
- **Acceptance Criteria**: 有 bench 数据支撑的 go/no-go 决策
- **Test Requirements**:
  - `programmatic` TR-14.1: bench 报告含 serde_json 哈希延迟数据
  - `programmatic` TR-14.2: 若实施，hash 一致性 proptest 验证
  - `human-judgement` TR-14.3: 决策有 bench 数据支撑

---

### [ ] Task 15: EDSB 次优选择策略改进（v1.5.0 Task 9）
- **Priority**: P2
- **Depends On**: Task 6
- **Description**: 
  - 修改 `crates/faae-router/src/edsb.rs`，次优选择从"top-2 中选第二"改为"非最热候选中相似度最高的"
  - 增加 TDD 测试验证新策略
  - 增加 WHY 注释解释数学依据
- **Acceptance Criteria**: 候选 > 2 时选择非最高相似度中的最优者
- **Test Requirements**:
  - `programmatic` TR-15.1: 候选 > 2 时选择非最高相似度中的最优者
  - `programmatic` TR-15.2: 候选 = 2 时行为与之前一致（回归）
  - `programmatic` TR-15.3: 候选 = 1 时直接返回

---

### [ ] Task 16: cosine_similarity 优化重新评估（v1.5.0 Task 12）
- **Priority**: P2
- **Depends On**: Task 6
- **Description**: 
  - 编写 bench 测量 `cosine_similarity_slices()` 在 512-dim 下的延迟
  - 测量被 repo-wiki/mlc-engine/kvbsr-router 调用的累积频率
  - 若 bench 显示热路径累计 > ms 级，在保持 `#![forbid(unsafe_code)]` 前提下优化
  - 优化方案：循环展开（4x unroll）或 aligned chunks
- **Acceptance Criteria**: 有 bench 数据支撑的 go/no-go 决策
- **Test Requirements**:
  - `programmatic` TR-16.1: bench 报告含 512-dim 延迟与调用频率数据
  - `programmatic` TR-16.2: 若实施，proptest 验证 bit-exact 一致
  - `programmatic` TR-16.3: 若实施，bench 显示 > 20% 性能提升
  - `human-judgement` TR-16.4: 无 unsafe 代码

---

### [ ] Task 17: NMC Perceptor 并行化重新评估（v1.5.0 Task 13）
- **Priority**: P3
- **Depends On**: Task 2（nmc-encoder 编译基线修复）
- **Description**: 
  - 评估当前 Image/Video/Audio 占位 perceptor 的 perceive() 返回延迟
  - 若占位实现是立即返回（无 I/O 无计算），记录评估结论继续延后
  - 若有真实计算（如 Text/Desktop perceptor），实施 `tokio::join!` 并行化
- **Acceptance Criteria**: 有评估结论的 go/no-go 决策
- **Test Requirements**:
  - `programmatic` TR-17.1: 评估报告含各 perceptor 延迟数据
  - `programmatic` TR-17.2: 若实施，并行化结果与串行完全一致
  - `human-judgement` TR-17.3: 决策有数据支撑

---

### [ ] Task 18: gsoe spawn_blocking 重新评估（v1.5.0 Task 14）
- **Priority**: P3
- **Depends On**: Task 6
- **Description**: 
  - 评估 gsoe-evolution 的 `evaluate_population()` 计算量
  - 若种群规模 < 100 且计算量在亚毫秒级，记录评估结论继续延后
  - 若计算量 > 5µs，实施 spawn_blocking 包装
- **Acceptance Criteria**: 有评估结论的 go/no-go 决策
- **Test Requirements**:
  - `programmatic` TR-18.1: bench 报告含 evaluate_population 延迟数据
  - `human-judgement` TR-18.2: 决策有数据支撑

---

## Phase IV: OMEGA 魔改创新深化

---

### [ ] Task 19: Speculative DAG 执行（I3）
- **Priority**: P1
- **Depends On**: Task 6
- **Description**: 
  - 修改 `crates/quest-engine/src/semantic_dag.rs`，将线性链分解器改为真 DAG 分解
  - 识别可并行子任务，产出含分支的 DAG 结构
  - 实施保守投机执行策略（仅独立分支并行）
  - 增加 TDD 测试验证 DAG 分解正确性
  - 需 ADR-012 记录 Speculative DAG 设计决策
- **Acceptance Criteria**: DAG 分解器产出含分支的 DAG
- **Test Requirements**:
  - `programmatic` TR-19.1: DAG 分解器产出含分支结构
  - `programmatic` TR-19.2: 独立分支可并行执行
  - `programmatic` TR-19.3: 依赖分支串行执行
  - `human-judgement` TR-19.4: ADR-012 记录设计决策

---

### [ ] Task 20: CLV 分层压缩（I5）
- **Priority**: P2
- **Depends On**: Task 6
- **Description**: 
  - 修改 `crates/nexus-core/src/clv.rs`，CLV 从固定 512-dim 扩展为分层压缩
  - 新增 `CLVLayer` 枚举（L0=512 / L1=256 / L2=128）
  - `compress()` 方法从 L0 降维到 L1/L2
  - `cosine_similarity_slices()` 支持不同维度比较（通过 L0 投影）
  - 需 ADR-013 记录 CLV 类型变更
  - 增加 TDD 测试验证压缩正确性
- **Acceptance Criteria**: CLV 支持分层压缩
- **Test Requirements**:
  - `programmatic` TR-20.1: CLV 可从 512-dim 压缩到 256/128-dim
  - `programmatic` TR-20.2: 压缩后相似度计算仍有效
  - `programmatic` TR-20.3: 向后兼容——512-dim 接口不变
  - `human-judgement` TR-20.4: ADR-013 记录类型变更

---

### [ ] Task 21: GRPO 自适应任务评分（I6）
- **Priority**: P2
- **Depends On**: Task 6
- **Description**: 
  - 修改 `crates/gsoe-evolution/src/engine.rs`，从规则式评分扩展为 GRPO 风格组内相对比较
  - `evaluate_population()` 对种群分组，组内相对比较产出优势/劣势对
  - 历史数据来源：优先使用 model-router HistoryStore（若可用），否则 gsoe 内部积累
  - 增加 TDD 测试验证 GRPO 评分正确性
- **Acceptance Criteria**: GRPO 评分产出组内相对比较对
- **Test Requirements**:
  - `programmatic` TR-21.1: 评分产出组内相对比较对
  - `programmatic` TR-21.2: 优势对/劣势对区分正确
  - `programmatic` TR-21.3: 所有现有 gsoe-evolution 测试通过

---

### [ ] Task 22: OS-Memory Wiki 元遗忘（I7）
- **Priority**: P2
- **Depends On**: Task 3（repo-wiki 编译基线修复）
- **Description**: 
  - 修改 `crates/repo-wiki/src/store.rs`，增加遗忘曲线 + 重要性衰减机制
  - 基于 Ebbinghaus 遗忘曲线计算条目保留概率
  - 重要性低的条目在长时间未访问时自动降级到冷存储
  - 增加 TDD 测试验证遗忘机制
- **Acceptance Criteria**: 低重要性条目长时间未访问后降级
- **Test Requirements**:
  - `programmatic` TR-22.1: 遗忘曲线计算正确
  - `programmatic` TR-22.2: 低重要性条目降级到冷存储
  - `programmatic` TR-22.3: 高重要性条目不降级
  - `programmatic` TR-22.4: 所有现有 repo-wiki 测试通过

---

### [ ] Task 23: CACR 非对称预算控制（I8）
- **Priority**: P2
- **Depends On**: Task 6
- **Description**: 
  - 修改 `crates/model-router/src/cacr.rs`，从双阈值线性判定扩展为非对称预算控制
  - 支持不同维度（Token/Cost/Latency）的独立预算阈值
  - 非对称：升阈值可宽松，降阈值严格（防振荡）
  - 增加 TDD 测试验证非对称控制
- **Acceptance Criteria**: CACR 支持非对称预算控制
- **Test Requirements**:
  - `programmatic` TR-23.1: 升阈值宽松、降阈值严格
  - `programmatic` TR-23.2: 多维度独立预算阈值
  - `programmatic` TR-23.3: 向后兼容——双阈值场景行为不变

---

### [ ] Task 24: 主动安全不变量检查（I9）
- **Priority**: P3
- **Depends On**: Task 6
- **Description**: 
  - 修改 `crates/seccore/src/lib.rs`，增加 QK-Clip 主动安全不变量检查
  - 在命令执行前验证安全不变量（如"不执行未审计命令"、"不超出预算限制"）
  - 不变量违反时阻止执行并发布 SecurityInvariantViolated 事件
  - 增加 TDD 测试验证不变量检查
- **Acceptance Criteria**: 安全不变量违反时阻止执行
- **Test Requirements**:
  - `programmatic` TR-24.1: 不变量违反时阻止执行
  - `programmatic` TR-24.2: SecurityInvariantViolated 事件正确发布
  - `programmatic` TR-24.3: 正常操作不受影响

---

## Phase V: 性能微优化（需 bench 支撑）

---

### [ ] Task 25: 热路径 clone 减少
- **Priority**: P2
- **Depends On**: Task 6
- **Description**: 
  - 分析 mlc-engine(63 处)、cmt-tiering(45 处)、scc-cache(42 处) 的 clone 密集度
  - 识别热路径上的不必要深拷贝（String/Vec）
  - 替换为 Arc 共享或引用传递
  - 增加 bench 验证性能提升
- **Acceptance Criteria**: 热路径 clone 减少 ≥ 30%
- **Test Requirements**:
  - `programmatic` TR-25.1: 热路径 clone 数量减少
  - `programmatic` TR-25.2: bench 显示性能提升
  - `programmatic` TR-25.3: 所有现有测试通过

---

### [x] Task 26: VectorIndex Mutex→RwLock（B1）— 已实现（RwLock<HashMap>）
- **Priority**: P1
- **Depends On**: Task 3（repo-wiki 编译基线修复）
- **Description**: 
  - 修改 `crates/repo-wiki/src/vector.rs`，将 `Mutex<HashMap>` 改为 `RwLock<HashMap>`
  - 允许并发 KNN 搜索
  - 增加 bench 验证并发搜索性能提升
- **Acceptance Criteria**: 并发 KNN 搜索吞吐量提升
- **Test Requirements**:
  - `programmatic` TR-26.1: 并发搜索走读锁不阻塞
  - `programmatic` TR-26.2: 写操作仍正确串行化
  - `programmatic` TR-26.3: 所有现有 repo-wiki 测试通过

---

### [ ] Task 27: 双格式序列化完成（C2）
- **Priority**: P3
- **Depends On**: Task 6
- **Description**: 
  - 完善 event-bus 的 msgpack/json 双格式序列化自动选择策略
  - 小 payload（< 1KB）使用 JSON（可读性）
  - 大 payload（≥ 1KB）使用 MessagePack（紧凑性）
  - 增加 TDD 测试验证自动选择
- **Acceptance Criteria**: 序列化格式自动选择
- **Test Requirements**:
  - `programmatic` TR-27.1: 小 payload 使用 JSON
  - `programmatic` TR-27.2: 大 payload 使用 MessagePack
  - `programmatic` TR-27.3: 所有现有 event-bus 测试通过

---

### [ ] Task 28: Prometheus 指标扩展（G2）
- **Priority**: P3
- **Depends On**: Task 6
- **Description**: 
  - 将 repo-wiki 已接入的 prometheus-client 模式扩展到 event-bus 和 efficiency-monitor
  - event-bus: 暴露 `events_published_total` Counter（按 EventKind 分维度）
  - efficiency-monitor: 暴露 `alerts_triggered_total` Counter（按 AlertLevel 分维度）
  - 增加 TDD 测试验证指标正确性
- **Acceptance Criteria**: event-bus 和 efficiency-monitor 暴露 Prometheus 指标
- **Test Requirements**:
  - `programmatic` TR-28.1: event-bus 暴露 events_published_total
  - `programmatic` TR-28.2: efficiency-monitor 暴露 alerts_triggered_total
  - `programmatic` TR-28.3: 指标值正确反映操作

---

### [ ] Task 29: heuristic_scores() 真实实现
- **Priority**: P2
- **Depends On**: Task 6
- **Description**: 
  - 修改 `crates/osa-coordinator/src/coordinator.rs`，将 `heuristic_scores()` 占位符替换为真实评分函数
  - 评分基于工具能力匹配度（CLV 余弦相似度 + 能力标签匹配）
  - 增加 TDD 测试验证评分正确性
- **Acceptance Criteria**: heuristic_scores() 产出真实评分
- **Test Requirements**:
  - `programmatic` TR-29.1: 评分基于 CLV 余弦相似度
  - `programmatic` TR-29.2: 评分基于能力标签匹配
  - `programmatic` TR-29.3: 所有现有 osa-coordinator 测试通过

---

## Phase VI: 文档对齐与经验沉淀

---

### [ ] Task 30: 更新 CODE_WIKI.md
- **Priority**: P2
- **Depends On**: Task 1-29 完成
- **Description**: 
  - 更新 §3.1 crate 索引反映 v1.6.0 变更
  - 新增 ADR-011/012/013（事件优先级/Speculative DAG/CLV 分层压缩）
  - 更新 §2.3 ADR 权威源
- **Acceptance Criteria**: CODE_WIKI.md 反映 v1.6.0 变更
- **Test Requirements**:
  - `human-judgement` TR-30.1: crate 索引准确
  - `human-judgement` TR-30.2: ADR 编号连续
  - `human-judgement` TR-30.3: 新 ADR 有完整记录

---

### [ ] Task 31: 更新 CHANGELOG.md
- **Priority**: P2
- **Depends On**: Task 1-29 完成
- **Description**: 
  - 添加 v1.6.0-omega 汇总章节
  - 记录每个 Phase 的变更内容
  - 记录跳过的任务及原因
- **Acceptance Criteria**: CHANGELOG.md 有 v1.6.0-omega 章节
- **Test Requirements**:
  - `human-judgement` TR-31.1: 章节准确反映变更
  - `human-judgement` TR-31.2: 跳过任务有原因说明

---

### [ ] Task 32: project_memory 经验沉淀
- **Priority**: P2
- **Depends On**: Task 1-29 完成
- **Description**: 
  - 将本阶段关键设计决策提炼为 project_memory 新原则（原则 23+）
  - 记录内容：
    - 编译基线修复的根因分析模式
    - 读写分离架构的通用模式
    - DAG 分解与投机执行的适用场景
    - CLV 分层压缩的设计权衡
    - 非对称预算控制的防振荡原理
- **Acceptance Criteria**: project_memory 新增原则 23+
- **Test Requirements**:
  - `human-judgement` TR-32.1: 每条原则是跨场景通用模式
  - `programmatic` TR-32.2: 原则编号连续
  - `human-judgement` TR-32.3: 验证文件实际内容（遵循原则 13）

---

## Phase VII: 全量验证与交付

---

### [ ] Task 33: 全量验证与交付
- **Priority**: P0
- **Depends On**: Task 1-32 完成
- **Description**: 
  - 运行完整验证套件：
    - `cargo check --workspace`
    - `cargo test --workspace --jobs 1`（Windows OOM 缓解）
    - `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings`
    - `cargo fmt --all -- --check`
    - `cargo audit --deny warnings --ignore RUSTSEC-2026-0190 --ignore RUSTSEC-2026-0002 --ignore RUSTSEC-2024-0436`
  - 确认所有验收标准通过
  - 生成 v1.6.0-omega 综合优化报告
- **Acceptance Criteria**: 所有验证通过
- **Test Requirements**:
  - `programmatic` TR-33.1: cargo check 退出码 0
  - `programmatic` TR-33.2: cargo test 全部 passed / 0 failed
  - `programmatic` TR-33.3: cargo clippy 零警告
  - `programmatic` TR-33.4: cargo fmt 零 diff
  - `programmatic` TR-33.5: cargo audit 通过
  - `human-judgement` TR-33.6: 综合报告包含每个 Task 的验证结果

---

# Task Dependencies

- Task 1-5: 无依赖，可并行执行
- Task 6: 依赖 Task 1-5 全部完成
- Task 7: 依赖 Task 3（repo-wiki 编译修复）
- Task 8-12: 依赖 Task 6（全量编译基线验证）
- Task 13-18: 依赖 Task 6，可并行执行（YAGNI 重新评估）
- Task 19-24: 依赖 Task 6，可并行执行（创新深化）
- Task 25-29: 依赖 Task 6，可并行执行（性能微优化）
- Task 30-32: 依赖 Task 1-29 完成
- Task 33: 依赖 Task 1-32 全部完成

## 并行执行策略

**Wave 1**（编译基线修复）：Task 1, 2, 3, 4, 5 并行
**Wave 2**（全量基线验证）：Task 6（串行，依赖 Wave 1）
**Wave 3**（P0/P1 修复 + YAGNI 评估 + 创新深化 + 性能优化）：Task 7-29 并行（按依赖关系分组）
**Wave 4**（文档沉淀）：Task 30, 31, 32 并行
**Wave 5**（全量验证）：Task 33（串行，依赖全部）
