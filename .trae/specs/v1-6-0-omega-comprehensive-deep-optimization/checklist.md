# v1.6.0-omega — 全量分布式深度优化与创新演进 - Verification Checklist

## Phase I: 编译基线修复验证

- [ ] Checkpoint 1: `cargo check -p mlc-engine` 退出码 0，无编译错误
- [ ] Checkpoint 2: `cargo test -p mlc-engine` 全部 passed / 0 failed
- [ ] Checkpoint 3: `cargo clippy -p mlc-engine -- -D warnings` 零警告
- [ ] Checkpoint 4: `cargo check -p nmc-encoder` 退出码 0
- [ ] Checkpoint 5: `cargo test -p nmc-encoder` 全部 passed
- [ ] Checkpoint 6: `cargo clippy -p nmc-encoder -- -D warnings` 零警告
- [ ] Checkpoint 7: `cargo check -p repo-wiki` 退出码 0
- [ ] Checkpoint 8: `cargo test -p repo-wiki` 全部 passed
- [ ] Checkpoint 9: `cargo clippy -p repo-wiki -- -D warnings` 零警告
- [ ] Checkpoint 10: `cargo check -p hcw-window` 退出码 0
- [ ] Checkpoint 11: `cargo test -p hcw-window` 全部 passed
- [ ] Checkpoint 12: `cargo clippy -p hcw-window -- -D warnings` 零警告
- [ ] Checkpoint 13: `cargo check -p parliament` 退出码 0
- [ ] Checkpoint 14: `cargo test -p parliament` 全部 passed
- [ ] Checkpoint 15: `cargo clippy -p parliament -- -D warnings` 零警告
- [ ] Checkpoint 16: `cargo check --workspace` 退出码 0（全部 35 crate 编译通过）
- [ ] Checkpoint 17: `cargo test --workspace --jobs 1` 全部 passed / 0 failed
- [ ] Checkpoint 18: 测试数量 >= v1.5.0 基线（3400+）
- [ ] Checkpoint 19: 5 个 crate 的编译错误根因有文档记录

## Phase II: DEEP_RESEARCH 剩余 P0/P1 修复验证

- [ ] Checkpoint 20: WikiStore 并发读操作不相互阻塞（A3）
- [ ] Checkpoint 21: WikiStore 写操作仍正确串行化
- [ ] Checkpoint 22: WikiStore bench 显示并发读吞吐量提升 ≥ 2x
- [ ] Checkpoint 23: ModelRegistry 使用 RwLock 替代 DashMap（B3）
- [ ] Checkpoint 24: ModelRegistry register() 无 TOCTOU 竞态（使用 entry() API）
- [ ] Checkpoint 25: ModelRegistry bench 显示 ≤10 模型场景性能提升
- [ ] Checkpoint 26: cmt-tiering SQLite 并发读不阻塞（D1）
- [ ] Checkpoint 27: scc-cache SQLite 并发读不阻塞
- [ ] Checkpoint 28: cmt-tiering/scc-cache 写操作仍正确串行化
- [ ] Checkpoint 29: event-bus 支持 Priority 级事件优先投递（I4）
- [ ] Checkpoint 30: SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded 升级为 Priority 级
- [ ] Checkpoint 31: Priority 级事件优先于 Normal/Warning 投递
- [ ] Checkpoint 32: 向后兼容——现有 Normal 级事件行为不变
- [ ] Checkpoint 33: ADR-011 记录事件优先级设计决策
- [ ] Checkpoint 34: L6 路由顺序由代码强制保证（N9）
- [ ] Checkpoint 35: 路由顺序为 OSA → KVBSR → FaaE → SESA → GEA
- [ ] Checkpoint 36: 路由顺序不依赖事件到达顺序
- [ ] Checkpoint 37: AuditChain 并发 append 不相互阻塞（G1）
- [ ] Checkpoint 38: AuditChain 审计链完整性不变

## Phase III: v1.5.0 YAGNI 重新评估验证

- [ ] Checkpoint 39: NexusState 深拷贝延迟有 bench 数据（Task 13）
- [ ] Checkpoint 40: NexusState 决策有 bench 数据支撑（go/no-go）
- [ ] Checkpoint 41: 若实施 Arc 共享，所有调用点正确适配
- [ ] Checkpoint 42: TaskProfile serde_json 哈希延迟有 bench 数据（Task 14）
- [ ] Checkpoint 43: TaskProfile 决策有 bench 数据支撑（go/no-go）
- [ ] Checkpoint 44: 若实施 Hash trait，hash 一致性 proptest 验证
- [ ] Checkpoint 45: EDSB 次优选择候选 > 2 时选择非最高相似度中的最优者（Task 15）
- [ ] Checkpoint 46: EDSB 候选 = 2 时行为与之前一致（回归测试）
- [ ] Checkpoint 47: EDSB 候选 = 1 时直接返回
- [ ] Checkpoint 48: cosine_similarity 512-dim 延迟有 bench 数据（Task 16）
- [ ] Checkpoint 49: cosine_similarity 调用频率有测量数据
- [ ] Checkpoint 50: 若实施优化，proptest 验证 bit-exact 一致
- [ ] Checkpoint 51: 若实施优化，bench 显示 > 20% 性能提升
- [ ] Checkpoint 52: 若实施优化，无 unsafe 代码
- [ ] Checkpoint 53: NMC Perceptor 各模态延迟有评估数据（Task 17）
- [ ] Checkpoint 54: NMC Perceptor 决策有数据支撑（go/no-go）
- [ ] Checkpoint 55: gsoe evaluate_population 延迟有 bench 数据（Task 18）
- [ ] Checkpoint 56: gsoe spawn_blocking 决策有数据支撑（go/no-go）

## Phase IV: OMEGA 魔改创新深化验证

- [ ] Checkpoint 57: DAG 分解器产出含分支结构（I3）
- [ ] Checkpoint 58: 独立分支可并行执行
- [ ] Checkpoint 59: 依赖分支串行执行
- [ ] Checkpoint 60: ADR-012 记录 Speculative DAG 设计决策
- [ ] Checkpoint 61: CLV 支持从 512-dim 压缩到 256/128-dim（I5）
- [ ] Checkpoint 62: CLV 压缩后相似度计算仍有效
- [ ] Checkpoint 63: CLV 向后兼容——512-dim 接口不变
- [ ] Checkpoint 64: ADR-013 记录 CLV 类型变更
- [ ] Checkpoint 65: GRPO 评分产出组内相对比较对（I6）
- [ ] Checkpoint 66: GRPO 优势对/劣势对区分正确
- [ ] Checkpoint 67: gsoe-evolution 所有现有测试通过
- [ ] Checkpoint 68: Wiki 遗忘曲线计算正确（I7）
- [ ] Checkpoint 69: 低重要性条目长时间未访问后降级到冷存储
- [ ] Checkpoint 70: 高重要性条目不降级
- [ ] Checkpoint 71: CACR 升阈值宽松、降阈值严格（I8）
- [ ] Checkpoint 72: CACR 多维度独立预算阈值
- [ ] Checkpoint 73: CACR 向后兼容——双阈值场景行为不变
- [ ] Checkpoint 74: seccore 安全不变量违反时阻止执行（I9）
- [ ] Checkpoint 75: SecurityInvariantViolated 事件正确发布
- [ ] Checkpoint 76: 正常操作不受安全不变量检查影响

## Phase V: 性能微优化验证

- [ ] Checkpoint 77: mlc-engine/cmt-tiering/scc-cache 热路径 clone 减少 ≥ 30%
- [ ] Checkpoint 78: 热路径 clone 减少 bench 显示性能提升
- [ ] Checkpoint 79: VectorIndex 使用 RwLock 允许并发 KNN 搜索（B1）
- [ ] Checkpoint 80: VectorIndex 写操作仍正确串行化
- [ ] Checkpoint 81: VectorIndex bench 显示并发搜索性能提升
- [ ] Checkpoint 82: event-bus 双格式序列化自动选择（C2）
- [ ] Checkpoint 83: 小 payload (< 1KB) 使用 JSON
- [ ] Checkpoint 84: 大 payload (≥ 1KB) 使用 MessagePack
- [ ] Checkpoint 85: event-bus 暴露 events_published_total 指标（G2）
- [ ] Checkpoint 86: efficiency-monitor 暴露 alerts_triggered_total 指标
- [ ] Checkpoint 87: 指标值正确反映操作
- [ ] Checkpoint 88: osa-coordinator heuristic_scores() 产出真实评分
- [ ] Checkpoint 89: heuristic_scores() 评分基于 CLV 余弦相似度
- [ ] Checkpoint 90: heuristic_scores() 评分基于能力标签匹配

## Phase VI: 文档对齐与经验沉淀验证

- [ ] Checkpoint 91: CODE_WIKI.md §3.1 crate 索引反映 v1.6.0 变更
- [ ] Checkpoint 92: CODE_WIKI.md §2.3 新增 ADR-011/012/013
- [ ] Checkpoint 93: ADR 编号连续无冲突
- [ ] Checkpoint 94: 新 ADR 有完整记录（背景/决策/后果）
- [ ] Checkpoint 95: CHANGELOG.md 有 v1.6.0-omega 汇总章节
- [ ] Checkpoint 96: CHANGELOG 准确描述每个 Phase 的变更内容
- [ ] Checkpoint 97: 跳过的任务有原因说明
- [ ] Checkpoint 98: project_memory.md 新增原则 23+
- [ ] Checkpoint 99: 新原则为跨场景通用模式而非项目特定 hack
- [ ] Checkpoint 100: 原则编号连续（23, 24, ...），不重复不遗漏
- [ ] Checkpoint 101: 验证 project_memory 文件实际内容（遵循原则 13，不信任子代理报告）

## Phase VII: 全量验证与交付验证

- [ ] Checkpoint 102: `cargo check --workspace` 退出码 0
- [ ] Checkpoint 103: `cargo test --workspace --jobs 1` 全部 passed / 0 failed
- [ ] Checkpoint 104: 测试数量 >= v1.5.0 基线（3400+）
- [ ] Checkpoint 105: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 零警告
- [ ] Checkpoint 106: `cargo fmt --all -- --check` 零 diff
- [ ] Checkpoint 107: `cargo audit --deny warnings --ignore RUSTSEC-2026-0190 --ignore RUSTSEC-2026-0002 --ignore RUSTSEC-2024-0436` 通过
- [ ] Checkpoint 108: 所有修改的 crate 保持 `#![forbid(unsafe_code)]`
- [ ] Checkpoint 109: 依赖铁律遵守——无向上依赖（L(N)→L(N+1) 禁止）
- [ ] Checkpoint 110: 公共 API 向后兼容——新增功能不破坏现有接口
- [ ] Checkpoint 111: 所有新增代码有 WHY 注释解释设计决策
- [ ] Checkpoint 112: v1.6.0-omega 综合优化报告完成
- [ ] Checkpoint 113: 综合报告包含每个 Task 的验证结果与 bench 数据
- [ ] Checkpoint 114: `Cargo.toml [workspace.package].version` 同步为 `1.6.0-omega`
