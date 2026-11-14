# ADR-295: R11 三域收敛批——上下文冗余 · 检索延迟 · 跨会话一致性 + 并行调度解封

- 状态: Accepted
- 日期: 2026-11-14
- 批次: R11(W0~W6,单批六波次,用户裁决)
- 设计蓝本: 《HCW_上下文架构重构蓝图_v3.0_1M时代能力感知分层.md》(仓库根,225 行)
- 关联: ADR-288(hcw 版本化锁外压缩)/ ADR-289(R8 热路径削减,登记不实施表尾款清欠)/ ADR-290(跨会话一致性)/ ADR-291(HCW v3 P0)/ ADR-292(P3 桥)/ ADR-293(governor+w4)/ ADR-294(threads 404)/ ADR-192(mlc L2 分层)/ ADR-224(pvl 接线登记)/ ADR-281(混层写屏障分段——W1 判定链的既有语义基座)

## 背景

R1~R10 已将蓝图 P0/P1/P3-桥落地。R11 为蓝图收尾批 + 三域纵深批,根因均以代码证据定位:

1. **上下文冗余**:全仓唯一全局去重 = `(role,source,content)` 逐字三元组(`hcw-window/src/assemble.rs:114-166`);语义级折叠零实现;压缩器非语义 Top-N(`compressor.rs:36`)且 None-CLV 条目 relevance 恒 0 分系统性吃亏(`compressor.rs:103`)。
2. **检索延迟**:mlc 召回链 rerank 0 接线(`recall_fusion.rs:21`);`recall_hybrid` 四路并行编排显式延后 Wave 4(`engine.rs:963-964`);hnsw 后台重建 async 接缝缺(ADR-192:105-106);ADR-289 登记不实施表仍开放 5 项。
3. **跨会话一致性**:session-store 段-索引双写非原子、索引无自愈 API(`writer.rs:266-271`);cmt-tiering 生产构造点为零(`lib.rs:28-38`)、无自动升温;**MCP 并行封顶=0 真因** = `declared_side_effects` 配置表在装饰器链被丢弃(`tool_runtime/runtime.rs:280-293` 只转发 capability_of/declared_tools),R10 WP-T1 的配置供给到不了 `effective_side_effect` 查表点(`toolplan_runner.rs:537-568`)。

## 用户裁决(2026-11-14,AskUserQuestion 四问)

| # | 裁决点 | 结论 |
|---|--------|------|
| UD-1 | MCP 并行解封默认安全姿态 | **纯配置驱动 fail-closed**:修复装饰器转发链;`declared_side_effects` 空集默认=恒串行,omega.yaml 显式声明只读才并行 |
| UD-2 | 语义级/近重复折叠算法 | **CLV 余弦阈值折叠**:复用 SharedCLV 池 + L0 `cosine_similarity_slices`,阈值 0.92 对齐 scc semantic_cache 先验;三前置(双 CLV 存在/≥0.92/保留高分);零新依赖 |
| UD-3 | 批次组织 | **单批 R11 六波次**(W0~W6),每波独立提交+组合回归,批末一次全量回归 |
| UD-4 | cmt-tiering 生产启用姿态 | **分级启用**:decay 降级闭环默认开(机证充分),LSCT 自动升降级独立开关默认关、影子诊断先行 |

## 决策

### D1 — declared_side_effects 装饰器链穿透(W1,机械转发)

`ToolExecutor` trait 契约**零变化**;所有装饰器/包装器显式透传 `declared_side_effects()`:
`ToolRuntime`(runtime.rs)+ `CompositeToolExecutor`(builtin 与 mcp 双 delegate 声明表**合并**,各自声明各自负责)+ `exec_plan.rs` 三装饰器(RuntimeToolExecutor/OpTimeoutExecutor/CountingExecutor)+ `ExecPolicyToolExecutor`(若在链上)。
默认空集语义不变 ⇒ 未声明工具恒串行(fail-closed 不变)。判定优先级沿用 ADR-281 D1 既有 `effective_side_effect` 链。
新门:静态扫描所有 `impl ToolExecutor` 是否覆写 `declared_side_effects`(防未来装饰器再丢),注册 gate_manifest。

### D2 — recall_hybrid 四路并行编排(W2)

语义/情节/认识/图四路 `FuturesUnordered`;锁纪律遵 §4.4 反模式 1(读锁内快照/Arc clone→释放锁→await);env 逃生 `CHIMERA_RECALL_SEQUENTIAL=1`。并行化不改融合语义(proptest 锁 RRF 输出逐位一致)。

### D3 — hnsw 后台重建 async 接缝(W2)

`rebuild_and_recall` 建图窗口改 spawn_blocking 后台构建→就绪原子换装;构建期 recall 走精确层回退(现行为);`CHIMERA_NO_HNSW_MLC` 逃生不动(ADR-192 D5)。

### D4 — session-store 索引自愈 + schema_version(W3)

- 暴露 `TreeIndex::rebuild_from_segments` 生产 API + 启动完整性探针(seq 连续性/行数核对)→ 缺口时自动 rebuild + `IndexDegraded` 事件 + warn。**权威源语义不动**(纯段文件扫描,ADR-290 D2)——rebuild 是加速面优化不是正确性依赖。
- 树索引 meta 加 schema_version u16(对齐 ADR-213 Checkpoint F3 模式:尾字段+serde default,0=历史形态)。

### D5 — cmt-tiering 分级接线 + auto-promotion(W3,UD-4)

组合根接线 decay 降级闭环(默认开);LSCT 自动升降级独立开关(默认关,影子先行);get 跨层命中自动回热(可配置),INV-8 单调校验延伸升级面,级联防护对齐降级路径。

### D6 — scc WAL 正式减法裁决(W3)

ADR-289「scc SqliteWal 批量化接线」正式裁决为**不做**:SccCache 是可再生推测缓存,内容丢失可接受;WAL 预写给 insert 热路径加 fsync 成本,收益/成本倒挂(Ω₈-Assess)。SqliteWal 模块保留为独立可用库(零删除)。

### D7 — 上下文冗余治理(W4,UD-2)

- 4a 规范化归一去重:assemble 精确三元组升级二档(空白折叠+大小写规范化键),诊断计数分档。
- 4b CLV 余弦阈值语义折叠:落 assemble 装载面(不动存储权威源);三前置全满足才折叠;`CHIMERA_DEDUP_SEMANTIC=off` 逃生。
- 4c 压缩器语义化:Top-N 评分接入 CLV 语义相关性;**修复 None-CLV 恒 0 分**→中性先验(不奖不罚)。
- 4d recall_hybrid 跨路重复折叠策略化(默认保守=现状;跨路重叠是有意保留的 RRF 信号)。
- 4e w4 开/关对照实验:`CHIMERA_W4_DISABLED`(ADR-293 D3 已落的安全阀)双跑,双指标(质量:检索命中率/Top-K 重叠;成本:token 装载量/延迟)——蓝图 P2 验收条件自此机证闭环。

### D8 — delegation 数据面扇出上限(W5)

`delegation.rs` 粗粒度批次加 Semaphore;**语义澄清**:数据面执行扇出 ≠ mas-sched 控制面认领配额(`max_concurrent=4`),不强行贯通;默认 8(对齐 PlanRunner DEFAULT_CONCURRENCY/BatchLimits 家族),config 化。属行为变化(现无上限),由本条登记。

### D9 — pvl-layer VerifyConcurrency 接线或退役(诚实二选一,W5)

实施时核实真实消费者:有→按 ADR-224 接线;无→退役减法。两个结论都合法,不许静默搁置。

### D10 — M3 值漂移门(W5)

新脚本 `scripts/check_perf_thresholds_sync.py`:perf_thresholds.toml 键 ↔ bench 注册组双向对拍(audit_cmd_sync.py/G-137 双向对拍模式);注册 gate_manifest(编号现取顺延)。

## 减法(明确不做)

- threads 真实厂商端到端 / 探针真实标定 / L2 订阅链 / previous_response_id 增量续传:**维持 GATED**(外部条件:公测端点/flash 真通道/消费场景,本批不具备)。
- scc LRU O(n)→O(1):数据驱动,criterion 实测 256 条目 ≈1μs 不超标则维持现状(lru.rs:19-20 自辩),超标才改。
- coverage floor 表填数:无真实逐 crate 产物则维持(禁止编造)。
- 语义折叠不做跨 crate 共享原语大重构:落点以最小接线为准(Ω₆ 复用优先,防过度工程化)。

## 验收与回滚

- 每波:check → 组合回归(--no-fail-fast)→ criterion 红线零回退 → 门注册 → 逐文件 add + HEREDOC 提交。
- 行为变化全带 env/config 逃生:`CHIMERA_RECALL_SEQUENTIAL`/`CHIMERA_DEDUP_SEMANTIC`/`CHIMERA_W4_DISABLED`(已在盘)/`CHIMERA_NO_HNSW_MLC`(已在盘)/cmt 配置节点/delegation 上限配置。
- 波次=提交组,WP=原子 commit,单 WP revert 不影响他波;新事件变体(若有)走六锁同步,ordinal 尾号续增(ADR-197 惯例)。
- 全量回归前置:target/debug 体积复检(R10 104GB 教训;W0 实测 13G/92G 可用,健康)。
