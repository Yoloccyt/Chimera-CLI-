# v1.4.0-omega P2 实施路线图 Spec

> **来源**:`docs/optimization/v1.4.0/m1_vector_index_assessment.md` + `m2_rl_routing_assessment.md` + `m3_config_hot_reload_assessment.md`
> **前置 spec**:`v1-3-0-omega-post-optimization-roadmap`(P2 评估阶段已完成)
> **原则**:YAGNI(You Aren't Gonna Need It)+ 长期主义 + 优先级驱动 + TDD 守恒
> **协作模式**:精英专家级子代理团队,系统性分布式深度分析 + 多轮结构化验证

## Why

v1.3.0-omega P2 三项任务(M1/M2/M3)已完成评估阶段,触发条件均未满足。但评估报告揭示两项前置阻塞:

1. **监控缺口**:`WikiStore::count()` 存在但未接入 metrics 上报,无法观测 Wiki entries 增长趋势,使 M1 触发条件(> 1000 entries 且 KNN p95 > 10ms)的评估缺乏数据支撑
2. **历史数据持久化缺失**:M2 触发条件 1(> 10000 条历史路由数据)在当前 `InMemoryHistoryStore`(DashMap 内存实现)下物理不可达,进程重启即丢失全部历史,阻塞 RL 路由策略实施

本 spec 补齐这两项前置依赖,并在依赖就绪后按触发条件推进 M1/M2/M3 实施。同时,若用户明确请求配置热重载(M3),则立即启动实施。

## What Changes

### P0 — 前置必做:监控缺口补齐(独立小任务,~4h)

- **G1**:`repo-wiki` 暴露 `wiki_entries_total` gauge 指标(`prometheus-client`),实时反映 Wiki 条目总数
- **G2**:在 `WikiStore::insert` / `delete` 操作后更新 gauge,确保指标与实际条目数一致
- **G3**:设置预警阈值(entries 接近 800 时日志 WARN),为 M1 触发条件提供监控数据源

### P1 — 前置必做:M2 历史数据持久化(解除触发条件阻塞,~12h)

- **H1**:实现 `SqliteHistoryStore`(实现 `HistoryStore` trait),采用 SQLite + `spawn_blocking` 包装(遵守 §4.4 #2 rusqlite async 反模式)
- **H2**:`history` 表 schema:`model_id TEXT PK / success_count INTEGER / total_count INTEGER / latency_samples BLOB(MessagePack 序列化 VecDeque<f32>)`
- **H3**:迁移钩子:在 `route_auto_with_gate` 返回 `RoutingDecision` 后,调用方异步回调 `HistoryStore::record`(spawn_blocking 非阻塞主路径)
- **H4**:配置开关:`model_router.history.persistence = "sqlite" | "memory"`(默认 memory,向后兼容)

### P2 — 条件触发:M1 向量索引升级(~40h+,依赖 P0 监控数据)

- **M1**:当 P0 监控数据显示 entries > 1000 且 KNN p95 > 10ms 时启动实施
- **候选方案优先级**:qdrant > milvus > sqlite-vec(unsafe,不推荐)
- **实施路径**:L5 知识层新增 `VectorIndex` trait 抽象,内存 KNN 与 qdrant 实现可切换,API 不变
- **未触发时**:仅完成 P0 监控接入,每季度评估触发条件

### P3 — 条件触发:M2 RL 路由策略(~16h+,依赖 P1 持久化)

- **M2**:当 P1 持久化落地且历史路由数据 > 10000 条时启动实施
- **候选方案优先级**:Bandit > 在线梯度下降 > 离线训练
- **实施路径**:RL 权重作为"调整项"叠加在静态权重上,而非完全替代(向后兼容 v1.3.0 五维评分)
- **未触发时**:仅完成 P1 持久化,为 RL 路由奠基

### P4 — 条件触发:M3 配置热重载(~36h+,依赖用户明确请求)

- **M3**:当用户明确请求运行时配置变更能力时启动实施
- **候选方案**:`notify` crate(跨平台 + 无 unsafe,契合 `#![forbid(unsafe_code)]`)
- **实施路径**:`LazyConfig` 重构为 `RwLock<Arc<T>>` 或 `ArcSwap`,支持热替换;Event Bus 广播 `ConfigChanged` 事件
- **未触发时**:保持当前重启加载模式,OnceLock 性能已验证足够(p99 = 7.22μs)

### 明确排除(YAGNI)

- **sqlite-vec 向量索引**:违反 `#![forbid(unsafe_code)]` 铁律,明确排除(除非项目提供纯 Rust 安全 binding)
- **M1/M2/M3 无条件全量实施**:触发条件未满足前不启动,仅做前置依赖(P0/P1)与评估
- **长期 v2.0+ 三项**(FTS5 BM25 定制 / MoE 动态阈值 / 配置 schema 版本化)不在本 spec 范围

## Impact

- **Affected specs**:
  - `v1-3-0-omega-post-optimization-roadmap`(P2 评估阶段 → v1.4.0 实施阶段演进)
  - `v1-2-0-omega-deferred-optimization`(M1/M2/M3 是其后续优化建议的落地)
- **Affected code**:
  - **P0(G1-G3)**:`crates/repo-wiki/src/store.rs`(`WikiStore` 集成 metrics)+ `Cargo.toml`(dev-dep `prometheus-client`)+ `crates/repo-wiki/src/metrics.rs`(新增,指标定义)
  - **P1(H1-H4)**:`crates/model-router/src/history/sqlite.rs`(新增,`SqliteHistoryStore`)+ `crates/model-router/src/moe.rs`(trait 扩展)+ `crates/model-router/src/config.rs`(持久化配置)+ `crates/model-router/Cargo.toml`(dev-dep `rusqlite` 若未传递依赖)
  - **P2(M1)**:`crates/repo-wiki/src/vector.rs`(trait 抽象)+ 可能新增 `qdrant-bridge` crate(需 ADR)
  - **P3(M2)**:`crates/model-router/src/rl/`(新增,Bandit 算法)+ `crates/model-router/src/moe.rs`(权重叠加)
  - **P4(M3)**:`crates/chimera-cli/src/config.rs`(`LazyConfig` 重构)+ `crates/event-bus/src/types.rs`(`ConfigChanged` 事件)
- **Affected layers**:L1(model-router)/ L5(repo-wiki)/ L10(chimera-cli),均符合 §2.2 依赖铁律

## ADDED Requirements

### Requirement: P0 监控缺口补齐

系统 SHALL 在 `repo-wiki` crate 暴露 `wiki_entries_total` gauge 指标,实时反映 Wiki 条目总数,为 M1 触发条件提供数据支撑。

#### Scenario: 指标暴露
- **WHEN** `WikiStore::insert` 或 `delete` 操作完成后
- **THEN** `wiki_entries_total` gauge 更新为当前条目总数

#### Scenario: 预警阈值
- **WHEN** `wiki_entries_total` 接近 800(触发阈值的 80%)
- **THEN** 日志 WARN 提示"M1 触发条件接近,建议启动评估"

#### Scenario: Prometheus 采集
- **WHEN** Prometheus 抓取 `/metrics` 端点(若存在)
- **THEN** 返回 `wiki_entries_total` 指标,类型为 gauge

### Requirement: P1 历史数据持久化

系统 SHALL 提供 `SqliteHistoryStore` 实现 `HistoryStore` trait,采用 SQLite + `spawn_blocking` 包装,支持历史路由数据持久化,解除 M2 触发条件阻塞。

#### Scenario: 持久化存储
- **WHEN** 调用 `HistoryStore::record(model_id, latency_ms, success)` 后
- **THEN** 数据异步写入 SQLite `history` 表(spawn_blocking 非阻塞主路径)

#### Scenario: 重启恢复
- **WHEN** CLI 进程重启后
- **THEN** `SqliteHistoryStore` 从 SQLite 加载历史数据,`get(model_id)` 返回持久化记录

#### Scenario: 向后兼容
- **WHEN** 配置 `model_router.history.persistence = "memory"`(默认)
- **THEN** 使用 `InMemoryHistoryStore`(v1.3.0 行为),无持久化开销

#### Scenario: spawn_blocking 包装
- **WHEN** rusqlite 调用在 async 上下文
- **THEN** 通过 `tokio::task::spawn_blocking` 包装,避免阻塞 runtime(§4.4 #2)

## MODIFIED Requirements

### Requirement: HistoryStore trait(v1.3.0 → v1.4.0 演进)

v1.3.0 的 `HistoryStore` trait 保持对象安全(`&self` 方法 + owned 返回),v1.4.0 新增 `SqliteHistoryStore` 实现作为可选持久化后端。`MoeGate::gate()` 签名不变,`history: Option<&dyn HistoryStore>` 参数对内存/SQLite 实现透明。

### Requirement: MoeGate 评分(v1.3.0 → v1.4.0 演进)

v1.3.0 的五维静态评分(cost 0.3 / latency 0.3 / quality 0.2 / success_rate 0.1 / variance 0.1)在 v1.4.0 保留为基线。若 P3 RL 路由触发,RL 权重作为"调整项"叠加在静态权重上(如 `static_score * (1 + rl_adjustment)`),而非完全替代,确保向后兼容。

## REMOVED Requirements

无。本 spec 不移除任何现有需求,所有变更向后兼容。

---

## 设计原则

1. **YAGNI**:P2/P3/P4 设明确触发条件,未触发前仅做 P0/P1 前置依赖,避免过度工程化
2. **长期主义**:优先级 P0 → P1 → P2/P3/P4 严格递进,不跳级,不并行跨档任务
3. **TDD 守恒**:每项先写失败测试再实现,不允许删除已有测试
4. **依赖方向不可逆**:L1/L5/L10 修改遵守 §2.2,跨层通信只走 Event Bus
5. **`#![forbid(unsafe_code)]` 守恒**:sqlite-vec 明确排除,notify crate 无 unsafe,SqliteHistoryStore 通过 rusqlite bundled(unsafe 不传播)
6. **向后兼容**:P1 持久化为可选配置(默认 memory),P2/P3/P4 保留 v1.3.0 降级路径
7. **spawn_blocking 约束**:所有 rusqlite 调用必须 `spawn_blocking` 包装(§4.4 #2)

## 优先级与触发条件矩阵

| 任务 | 优先级 | 触发条件 | 当前状态 | 预估工时 |
|------|--------|---------|---------|---------|
| P0 监控缺口补齐 | 必做 | 无(独立小任务) | 待实施 | 4h |
| P1 历史数据持久化 | 必做 | 无(M2 前置依赖) | 待实施 | 12h |
| P2 M1 向量索引升级 | 条件触发 | entries > 1000 且 KNN p95 > 10ms | 依赖 P0 监控数据评估 | 40h+ |
| P3 M2 RL 路由策略 | 条件触发 | 历史 > 10000 条且次优率 > 5% | 依赖 P1 持久化 + 数据积累 | 16h+ |
| P4 M3 配置热重载 | 条件触发 | 用户明确请求 | 待用户反馈 | 36h+ |

## 关联文档

- **来源**:
  - `docs/optimization/v1.4.0/m1_vector_index_assessment.md`(M1 评估报告)
  - `docs/optimization/v1.4.0/m2_rl_routing_assessment.md`(M2 评估报告)
  - `docs/optimization/v1.4.0/m3_config_hot_reload_assessment.md`(M3 评估报告)
- **前置 spec**:`v1-3-0-omega-post-optimization-roadmap`(P2 评估阶段已完成)
- **规则**:`.trae/rules/nuxus规则.md` §3.3 第二阶段开发原则 + §4.4 async 反模式清单
- **关联 ADR**:ADR-005 持久化存储选型(SQLite + 向量,sqlite-vec 降级)
