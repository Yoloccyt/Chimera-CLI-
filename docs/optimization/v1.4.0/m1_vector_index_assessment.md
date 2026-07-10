# M1 向量索引升级触发条件评估报告

> **评估日期**:2026-07-09
> **任务**:M1(P2 中期演进,条件触发评估)
> **关联 spec**:`.trae/specs/v1-3-0-omega-post-optimization-roadmap/`
> **基线版本**:v1.3.0-omega(repo-wiki 内存 KNN + FTS5 trigram)
> **评估 agent**:向量数据库评估精英子代理

## 1. 触发条件评估

| 触发条件 | 阈值 | 当前状态 | 是否触发 |
|---------|------|---------|---------|
| Wiki entries 规模 | > 1000 | 设计规模 10-1000;实际部署规模无法直接测量,基于配置默认值推测远低于 1000 | ❌ 否 |
| KNN p95 延迟 | > 10ms | 1000 entries 量级 < 1ms(代码复杂度评估 + vector.rs 注释明示);无 bench 实测数据 | ❌ 否 |

**结论**:触发条件**任一未满足** → 继续延后实施,仅做评估报告。建议下次评估时间:2026-10(每季度)。

## 2. 当前状态分析

### 2.1 当前向量索引实现

- **位置**:`crates/repo-wiki/src/vector.rs`
- **实现**:内存 KNN,基于 `RwLock<HashMap<String, Vec<f32>>>`
- **算法**:O(n) 全量扫描 + `select_nth_unstable_by` Top-K 选择(O(n))+ K log K 局部排序
- **相似度**:`nexus_core::cosine_similarity_slices`(512-dim 余弦相似度)
- **设计规模**:10-1000 entries(vector.rs 注释行 13-15 明示)
- **并发模型**:RwLock 多读并发(B1 优化),写锁互斥
- **API**:`search(query, top_k) -> Vec<(entry_id, f32)>`,纯同步(非 async)

### 2.2 当前规模评估

- **默认配置规模上限**:`WikiConfig::default()`(`crates/repo-wiki/src/types.rs:106-116`)
  无 `max_entries` 字段,HashMap 无硬上限;`vector_dim=512` / `read_pool_size=2` / `fts_enabled=true`
- **实际部署规模**:**无法直接测量**(无运行实例可观测);基于代码注释与设计目标推测,
  当前 Chimera CLI 尚处于 v1.0.0-omega RC 阶段,Wiki 主要由 `WikiGenerator` 从 Quest 结果
  自动沉淀,实际 entries 规模预计远低于 1000(典型部署 < 100 量级)
- **容量预警机制**:**缺失**(无监控指标暴露 entries 数量;`WikiStore::count()` 存在但未接入
  metrics 上报)。建议 v1.4.0 监控层补齐 entries 计数指标,作为触发条件监控的数据源

### 2.3 KNN p95 延迟评估

- **bench 设计**:`crates/repo-wiki/benches/vector_bench.rs` 设计了 3 个基准:
  - `single_thread_knn_latency`:100/1000 entries 单线程延迟基线
  - `concurrent_knn_search_throughput`:10 并发 search 吞吐
  - `search_under_write_load`:写负载下 search 延迟
- **实测数据**:**未执行**(`target/criterion/` 无 JSON 结果数据;RC 阶段未跑过 bench)
- **代码复杂度评估**:
  - 1000 entries × 512-dim 余弦相似度:O(n) × 512 FLOPS ≈ 51.2 万 FLOPS
  - 现代 CPU 单核 ~10 GFLOPS,理论计算时间 ~50μs
  - 加上 HashMap 迭代 + Top-K 选择 + RwLock 读锁开销,p95 预估 **< 1ms**
  - 与 vector.rs 注释「1000 条目规模:KNN 检索 < 10ms(可接受)」一致,且实际留有 10x 余量
- **结论**:p95 < 10ms 阈值,**远未触发**(预估 < 1ms,余量 10x+)

## 3. 候选方案

### 3.1 sqlite-vec(若未来触发)

- **优势**:与现有 rusqlite 集成,无外部进程依赖,单文件部署
- **劣势**:`sqlite-vec 0.1.9` Rust binding 仅暴露 C 入口 `sqlite3_vec_init`,
  注册扩展需 `rusqlite::ffi::sqlite3_auto_extension` + `unsafe` 块
- **评估**:**不推荐**(违反项目铁律 `#![forbid(unsafe_code)]`,ADR-005 降级原因)
- **解除条件**:sqlite-vec 项目提供纯 Rust 安全 binding(目前未出现)

### 3.2 qdrant(外部向量数据库)

- **优势**:Rust 实现,高性能 HNSW 索引,支持 100 万+ vectors,过滤查询
- **劣势**:外部进程依赖,部署复杂度上升;需新增 mcp-mesh 跨进程通信
- **评估**:适合大规模场景(> 10000 entries),触发后首选
- **集成路径**:L5 知识层新增 `qdrant-bridge` crate(向下依赖 L1 event-bus,
  向 L5 repo-wiki 暴露 trait),保持 `VectorIndex` API 不变(trait 抽象)

### 3.3 milvus(外部向量数据库)

- **优势**:分布式,支持亿级向量,适合多节点部署
- **劣势**:Go 实现,重量级,部署复杂;无 Rust 原生 binding(gRPC 调用)
- **评估**:适合超大规模场景(> 100 万 entries),非当前优先
- **集成路径**:同 qdrant,但 gRPC 依赖更重

### 3.4 保留内存 KNN + 优化(未触发,当前选择)

- **优势**:零外部依赖,符合 `#![forbid(unsafe_code)]`,API 稳定
- **劣势**:规模上限 1000-10000(O(n) 线性扫描);10000+ 延迟显著上升
- **评估**:**当前规模足够,无需升级**
- **未来优化预留**:vector.rs 注释「Week 6 NMC 编码器实现后,本层可替换为
  基于 `nexus_core::CLV` 的专用向量索引(如 HNSW)」,保持 API 不变

## 4. 建议与后续行动

### 4.1 当前建议

**触发条件未满足,继续使用内存 KNN**,延后评估至下次规模翻倍时:

- 当前预估 entries < 100,p95 < 1ms(10x 余量)
- 触发阈值 entries > 1000 且 p95 > 10ms,与当前状态差距 10x+
- 无需在 v1.4.0 启动实施 spec,保留 P2 条件触发性质

### 4.2 触发条件监控

**当前监控缺口**:`WikiStore::count()` 存在但未接入 metrics 上报,无法观测 entries 增长趋势。

建议 v1.4.0 监控层补齐(非 M1 范围,独立小任务):

- `repo-wiki` 暴露 `wiki_entries_total` gauge 指标(`prometheus-client`)
- 当 entries 接近 800 时启动预警(日志 WARN)
- 当 entries > 1000 且 KNN p95 > 10ms 时触发 M1 实施 spec
- 定期(每季度)评估 entries 规模与 KNN 延迟

### 4.3 触发后的实施 spec 路径

若未来触发条件满足,新建独立 spec:`.trae/specs/v1-4-0-omega-vector-index-upgrade/`

- 候选方案优先级:**qdrant > milvus > sqlite-vec(unsafe)**
- 预估工时:40h+(trait 抽象 + 集成 + bench + 迁移工具)
- 风险评估:外部依赖引入破坏「零外部进程」部署模型;需 ADR 记录权衡

## 5. 关联文档

- ADR-005 持久化存储选型降级说明:`CODE_WIKI.md §2.3`(sqlite-vec unsafe 降级原因)
- vector.rs 内存 KNN 实现:`crates/repo-wiki/src/vector.rs`
- vector_bench.rs 延迟基准:`crates/repo-wiki/benches/vector_bench.rs`
- v1.2.0 Task 2 FTS5 报告:`docs/optimization/v1.2.0/task2_fts5_verification_report.md`
- v1.3.0 S3 trigram 报告:`docs/optimization/v1.3.0/s3_trigram_report.md`
- v1.3.0 综合报告:`docs/optimization/v1.3.0/full_post_optimization_report.md`
- spec 路径:`.trae/specs/v1-3-0-omega-post-optimization-roadmap/`
