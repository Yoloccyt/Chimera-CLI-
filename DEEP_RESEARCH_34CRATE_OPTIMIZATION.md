# Deep Research: NEXUS-OMEGA 34 Crate 全量分布式深度分析与架构优化

> **Generated**: 2026-07-08 | **Depth**: deep | **Sources**: 34 crate 源码 + 7 份架构文档 + 2 份已有研究报告
> **子代理**: 6 路并行分析（A:性能存储 / B:算法执行 / C1:安全层 / C2:治理层 / D:接口横切 / 文档交叉比对）
> **代码覆盖**: ~109,458 LOC / 254 .rs 文件 / 34 crate 全量审计

---

## TL;DR

对 Chimera CLI/NEXUS-OMEGA 全部 34 个 crate（~109K LOC）进行了源码级分布式深度分析，审计了 26 项已有优化方案的实施状态，并发现了 19 项报告未覆盖的新问题。核心结论：**项目代码质量基线极高**（生产代码零 unwrap、零 unsafe、依赖铁律零违规），但存在 1 个 Critical 安全漏洞（seccore cmd.exe 绕过）、3 个 P0 级正确性 bug（SSRA 主导策略选取错误、checkpoint 同步 I/O 阻塞 async runtime、QEEP 三元组协议仅实现二元组）、以及 5 个 P0 级性能反模式。建议按"安全修复→正确性修复→性能优化→架构补债"四阶段推进，首阶段可在 2 天内完成且不破坏现有 3002+ 测试。

---

## Executive Summary

NEXUS-OMEGA 是一个 34-crate、10 层架构的 Rust AI Agent 框架，当前处于 v1.0.0-omega GA 后的 v1.1.0 开发阶段。本项目在架构纪律方面堪称 Rust 工程典范：`#![forbid(unsafe_code)]` 全覆盖、依赖铁律（L(N)→L(N+1) 禁止）零违规、事件驱动完全解耦。3002+ 测试覆盖了 OWASP Top 10 渗透、E2E 全链路、1000 次压测等场景。

通过 6 路并行子代理的源码级深度分析，本报告产出以下核心发现：

**安全层面**（1 Critical / 2 High）：seccore 的命令验证策略存在 cmd.exe 绕过路径——攻击者可通过 `cmd /c "危险命令"` 绕过四层安全防御；ASA 审计评分依赖调用者提供风险关键字，空列表即绕过检测；AuditChain 在命令执行后才追加记录，append 失败则无审计痕迹。

**正确性层面**（3 个 bug）：ssra-fusion 的 `select_top_k_desc` 使用 `select_nth_unstable_by` 后取 `[0]` 作为主导策略，但该函数不保证 `[0]` 是最大值；qeep-protocol 的三元组协议（Request→Ack→Receipt）中 Ack 从未被创建，实际仅实现二元组；quest-engine 的 DAG 分解器产出线性链而非真 DAG，任务并行度为零。

**性能层面**（5 个 P0 反模式）：checkpoint.rs 的同步 `fs::write`/`fs::read` 直接阻塞 Tokio worker 线程；VectorIndex 的 `Mutex<HashMap>` 阻止并发 KNN 搜索；ModelRegistry 的 DashMap 在 ≤10 模型场景下分片锁开销大于 RwLock；WikiStore 单 Mutex 连接否定 WAL 并发读优势；事件总线 65 变体全量广播无主题过滤。

**26 项已有方案审计结果**：3 项已实施（E2 Release Profile、A2 UUIDv7、Skeptic 否决权），2 项部分实施（C2 双格式序列化、F3 依赖铁律 CI），21 项未实施。

---

## 1. 全量模块分析 [Confidence: High]

### 1.1 L1 Core — nexus-core / event-bus / model-router (6,152 LOC)

**nexus-core** (1,432 LOC) 提供核心类型 CLV(512-dim)、NexusState(`Arc<RwLock>`)、Quest/Task/Checkpoint 等。`cosine_similarity_slices` 为纯标量 O(n) 实现，512-dim 单次 ~2μs，但被 repo-wiki/mlc-engine/kvbsr-router 多处调用，热路径累计可达 ms 级。`snapshot_hash` 每次序列化全部 Quest 后 SHA-256，含不必要的堆分配。NexusState 的 `get_quest()` 返回深拷贝而非 Arc 共享引用。代码质量 A。

**event-bus** (3,173 LOC) 实现了 broadcast + mpsc 双通道架构，Critical 事件（SkepticVeto/RedTeamAudit 等 4 类）走 mpsc 旁路确保投递。但 `types.rs` 1750 行定义了 65 个 NexusEvent 变体，是全局最大的单文件。`EventMetadata::new()` 每次调用生成 UUID + 时间戳（两次系统调用），高频场景下可优化为批量时间戳。事件主题过滤层（C1 方案）未实施——每个订阅者必须处理全部 65 种事件。代码质量 A-。

**model-router** (1,547 LOC) 实现三策略路由（Lite/Efficient/Auto）+ CACR 成本守卫。ModelRegistry 使用 `DashMap<String, ModelInfo>`（B3 方案指出 ≤10 模型时 RwLock 更优）。`registry.list()` 每次路由克隆全部 ModelInfo。CACR 使用 `f32` 乘法处理预算阈值，大预算值（u64 > 2^24）会因精度丢失产生误差。`register()` 使用 `contains_key` + `insert` 两步操作存在竞态窗口（应改用 `entry()` API）。代码质量 A-。

### 1.2 L2 Memory — nmc-encoder / mlc-engine / hcw-window (8,963 LOC)

**nmc-encoder** (1,784 LOC) 实现了 5 种 Perceptor + 3 种融合策略，纯计算模块无锁无 I/O。但 ImagePerceptor / VideoPerceptor / AudioPerceptor 仍为占位实现（12 处 TODO/FIXME）。5 种 Perceptor 的 `perceive()` 当前串行处理，可并行化（`tokio::join!`）。代码质量 A。

**mlc-engine** (4,540 LOC) 是 L2 最大 crate，实现四级记忆（L0 DashMap+LRU / L1 BTreeMap / L2 RwLock+KNN / L3 SQLite）。L2 SemanticMemory 的线性 KNN 扫描 O(n×d) 在 4096 条目×512 维时约 200ms——急需 ANN 索引。L0 的手动双向链表 LRU 实现 450+ 行，可考虑 `lru` crate 替代。L3 ProceduralMemory 使用 `Mutex<Connection>` 无 spawn_blocking，可能在 async 上下文中阻塞 runtime。clone 密集（63 处），部分函数接近 200 行上限。代码质量 B+。

**hcw-window** (2,639 LOC) 实现四级窗口（L0=4K/L1=32K/L2=128K/L3=1M）+ 重要性压缩 + OSA mask 稀疏化。已实施 M-01/M-02 Arc CoW 优化，工程深度优秀。`entries_index` HashMap 维护 ID→索引映射，`push_entry`/`remove` 需同步更新，建议增加一致性断言。代码质量 A。

### 1.3 L3 Storage — cmt-tiering / scc-cache / lsct-tiering (9,196 LOC)

**cmt-tiering** (4,844 LOC) 是全项目最大 crate，实现四级存储（Hot DashMap / Warm SQLite WAL / Cold SQLite / Ice 文件系统）+ 衰减降级 + 跨层提升。`coordinator.rs` 跨层查找时顺序遍历 Hot→Warm→Cold→Ice，每次提升涉及 2 个不同锁/存储的删除+插入，可能产生延迟尖刺。warm.rs 和 cold.rs 各自维护独立 `Arc<Mutex<Connection>>`，无连接池，并发 SQLite 操作被串行化。Ice 层使用文件系统直接 I/O 无缓冲。代码质量 B+。

**scc-cache** (2,558 LOC) 实现 DashMap 缓存 + LRU 逻辑时钟 + 马尔可夫链预取 + WAL。`insert_lock: Arc<Mutex<()>>` 粗粒度锁在高并发插入时成为瓶颈。马尔可夫链 `patterns: RwLock<HashMap<...>>` **无容量上限**——长时间运行后转移矩阵无限增长，存在内存泄漏风险。`access_order` 和 `entries` 是两个独立 DashMap，驱逐时需同步删除两处，存在一致性风险。代码质量 A-。

**lsct-tiering** (1,794 LOC) 是策略层（非存储层），按任务负载画像计算目标层级。纯计算+事件发布，无 I/O 无锁。`TierAssignment` 映射无持久化，进程重启后丢失。代码质量 A。

### 1.4 L4 Security — seccore / qeep-protocol / decay-engine (3,026 LOC)

**seccore** (2,044 LOC) 实现四层安全防御。**发现 1 个 Critical 漏洞**：命令验证策略中 `cmd` 在白名单中，攻击者可通过 `cmd /c "危险命令"` 绕过全部四层防御。ASA 审计评分依赖调用者提供风险关键字，空列表即绕过检测（High）。AuditChain 在命令执行后才追加记录，append 失败则无审计痕迹（High）。沙箱从 gVisor 降级为进程隔离，安全性缺口显著。I9（主动安全不变量）和 G1（AuditChain 并发化）均未实施。

**qeep-protocol** (561 LOC) 的 OrphanGuard Drop 机制设计精巧，但三元组协议只实现了二元组——Ack 从未被创建。Request→Receipt 直接跳转，Ack 阶段的确认语义缺失。OrphanDetector 的 Drop 语义在 tokio task cancel 时仍能生效。

**decay-engine** (421 LOC) 是全项目最小 crate，连续 [0,1] 流体衰减，DashMap 并发安全。数值边界处理规范，仅 `register_capability` 存在 TOCTOU 竞态。代码质量最高。

### 1.5 L5 Knowledge — repo-wiki / gsoe-evolution / auto-dpo (4,633 LOC)

**repo-wiki** (2,025 LOC) 实现 WikiStore(SQLite WAL + spawn_blocking) + VectorIndex(内存 KNN + Mutex) + ISCM 锚点。VectorIndex 的 `Mutex<HashMap>` 阻止并发搜索（B1 方案：改为 RwLock，修复复杂度低）。WikiStore 单 Mutex 连接否定 WAL 并发读优势（A3/D1 方案）。`search_fulltext` 使用 `LIKE '%query%'` 全表扫描无 FTS5 索引。`list_by_tag` 使用 `LIKE '%"tag"%'` 无法利用 B-tree 索引。代码质量 B+。

**gsoe-evolution** (1,669 LOC) 实现 GRPO 风格进化引擎。`evolve_once` 标记为 async 但内部无真正 await 点（除事件发布），计算密集部分应使用 spawn_blocking。`evaluate_population` 串行评估种群，可并行化。代码质量 B+。

**auto-dpo** (939 LOC) 偏好对生成器，纯计算模块，AtomicU64 计数器，无锁。`generate()` O(n) 两次遍历找最大/最小值，可用 `iter().max_by_key()`/`min_by_key()` 单次遍历。代码质量 A。

### 1.6 L6 Router — osa-coordinator / kvbsr-router / faae-router / gea-activator / sesa-router (9,351 LOC)

五层路由链路通过 EventBus 事件解耦，顺序靠约定驱动而非代码强制——这是最大的架构风险。

**osa-coordinator** (1,704 LOC)：五维稀疏掩码计算正确，`select_nth_unstable_by` O(n) Top-K + 二次排序保证确定性。NaN 处理 `unwrap_or(Equal)` 安全但分区位置不确定。`heuristic_scores()` 为占位符（评分完全由注册顺序决定），是五维路由质量的上限瓶颈。反序列化缺 `sparsity_ratio` 校验。代码质量 8/10。

**kvbsr-router** (2,218 LOC)：Union-Find 聚类（路径半分裂 + union-by-rank）实现标准，近似 O(α(n)) 均摊。两级余弦路由（块级 Top-3 → 块内 Top-8）对 300 工具规模 < 2ms。但块向量在工具向量更新后不会自动刷新，需等 rebalance 触发——建议增加 dirty flag。代码质量 9/10。

**faae-router** (1,799 LOC)：EDSB 熵均衡公式自洽，`p = 1 - entropy` 作为重分配概率逻辑正确。但次优选择仅考虑 top-2，当候选 > 2 时均衡效果打折——建议改为"非最热候选中相似度最高的"。`expert_registry` 双层锁（外层保护 registry，内层保护 profile）复杂度高。代码质量 8/10。

**gea-activator** (1,596 LOC)：sigmoid 门控数值稳定性可接受（输入通常 ±5，不会溢出）。冲突消解全排序+贪心检测 O(n log n + k²)。`hash_task_profile` 每次做 serde_json 序列化，128-capacity 缓存下是热路径开销——应为 TaskProfile 实现 Hash trait。代码质量 7/10。

**sesa-router** (2,034 LOC)：256-bit 位掩码操作精确，off-by-one 边界已逐 case 验证。`enforce_sparsity` 严格维护 < 40% 不变量。`AtomicU32` index 分配的 rollback 非原子（fetch_sub 与并发 fetch_add 交叉），极端并发下可能跳号但不破坏不变量。256 专家上限是编译期硬约束。代码质量 8/10。

### 1.7 L7 Execution — pvl-layer / gqep-executor / mtpe-executor / ssra-fusion (5,643 LOC)

**pvl-layer** (1,670 LOC)：Producer-Verifier 通过 mpsc 通道（capacity 128）杜绝竞态。三阶段验证（syntax→security→dependency）短路返回。滞后反馈（threshold/2 带）防策略振荡。但 Producer 的 `confidence` 是 SHA-256 前 4 字节→u32→[0,1]，纯伪随机。Verifier 安全检查是关键词匹配，无法覆盖管道组合攻击。代码质量 8/10。

**gqep-executor** (1,497 LOC)：FuturesUnordered 流式聚集 + QEEP 孤儿检测 + 超时治理。错误双向映射（GqepError ↔ QeepError）完整但丢失 operation_id（映射时用 `String::new()`）。无全局 gather 超时——仅有单操作超时，50 个操作各耗 timeout 的 90% 时总耗时仍为 50×0.9×timeout。代码质量 8/10。

**mtpe-executor** (1,063 LOC)：框架已搭建但核心预测为 FNV-1a 伪随机 placeholder。回退机制仅回退到 N=1，无更细粒度（前 5 步正确后 5 步错误时，N=1 回退浪费前 4 步正确预测）。代码质量 6/10。

**ssra-fusion** (1,413 LOC)：三策略融合（WeightedAverage/TopK/MeanField）数学正确。**发现 P0 bug**：`select_top_k_desc` 使用 `select_nth_unstable_by` 后取 `selected[0]` 作为主导策略权重，但 `select_nth_unstable_by` 不保证 `[0]` 是最大值——仅保证 `[k-1]` 是第 k 大且 `[0..k]` 都 ≥ pivot。主导策略可能选取了非最大权重元素。`deadline_ms=0` 语义歧义（无超时 vs 立即超时），建议用 `Option<u64>` 区分。代码质量 7/10。

### 1.8 L8 Parliament — parliament / decb-governor / acb-governor (9,572 LOC)

**parliament** (5,870 LOC)：5 角色加权投票（0.25+0.30+0.20+0.15+0.10=1.0），数学保证 ∈ [0,1]。AHIRT 红队 100 个载荷（4 类×25），覆盖度良好。**Skeptic 否决权无覆议机制**——Skeptic 权重最高(0.30)与否决优先于赞成率判定，25 条规则匹配的误判可能导致合法提案被大量否决。`voting.rs` 中 `frozen_capabilities` 为空向量，与 `debate.rs` 的 `exercise_veto` 路径不一致。投票机制为加权赞成率标量平均（非成对比较），不存在 Condorcet 悖论但也无法表达偏好排序。代码质量 8.5/10。

**decb-governor** (2,430 LOC)：高低双档 + 滞后机制 + 溢出检测三级阈值。预算系数 `base × complexity × urgency × remaining` 乘法耦合——任一因子趋零则系数趋零（正确的安全语义）。但 `tier_switch_lag_ms` 由配置注入，配置为 0 则防抖失效，代码无最小值校验。`check_overflow` 为无状态纯函数，不记忆上次检测结果。代码质量 8.0/10。

**acb-governor** (1,272 LOC)：四级预算 L0-L3 自动升降。**无时间维度滞后机制**（与 DECB 不同），仅有阈值稳定带。若利用率在 `degrade_threshold` 附近波动会导致 L2→L1→L2→L1 振荡。**与 DECB 矛盾指令风险**：ACB（Token 预算）和 DECB（认知预算）各自发布 `BudgetAdjusted` 事件，TTG 仅订阅 DECB 事件，ACB 事件无消费者联动——无仲裁层。代码质量 7.5/10。

### 1.9 L9 Quest — quest-engine / efficiency-monitor (5,066 LOC)

**quest-engine** (2,884 LOC)：**P0 级同步 I/O 阻塞**——`checkpoint.rs` 的 `save()`/`load()`/`load_latest()`/`prune_old()` 全程使用同步 `fs::write`/`fs::read`，在 async fn 中直接调用会阻塞 tokio worker 线程。`load_latest` 最坏需读 5 个检查点文件 + MessagePack 反序列化 + SHA-256 校验，阻塞数十毫秒。DAG 分解器产出线性链（task_i 依赖 task_{i-1}），`validate_dag` 和 `topological_order` 支持真 DAG 但分解器不产出分支结构。TTG 实现 3 级（Fast/Standard/Deep）而非文档声称的 4 级。TTG 模式切换事件仍走 `tracing::info!` 而非 EventBus（注释标注"待 Task 37"但 Task 37 在其他 crate 已完成）。I3（Speculative DAG）和 A1（spawn_blocking）均未实施，A2（UUIDv7）已实施。代码质量 7.0/10。

**efficiency-monitor** (2,182 LOC)：订阅全部 NexusEvent + 告警规则引擎 + cooldown 防抖。`record_event` 对每个事件做 DashMap 写入，高吞吐下可能成背压源头。Critical 立即告警路径绕过规则引擎——批量 SkepticVeto 可产生告警风暴。`is_in_cooldown` 处理时钟回拨（elapsed < 0 返回 false）——鲁棒性好。代码质量 8.0/10。

### 1.10 L10 Interface — chimera-cli / chimera-tui / chtc-bridge / mcp-mesh / csn-substitutor (8,504 LOC)

**chimera-cli** (1,521 LOC)：唯一 bin crate。`config.rs` 1061 行全量 eager 加载 14 个配置 section，即使 `--version` 也解析完整配置（实际 `--version` 由 Clap 内部处理不进入 config::load）。5 个子命令（run/tui/quest/wiki/parliament）均为 println 骨架。nexus-core 无 config.rs，30+ 配置 struct 全部内联定义，与下层 crate（如 MeshConfig/CsnConfig）形成平行类型——F1（配置类型统一）是 v1.1 接线的前置条件。

**chimera-tui** (1,268 LOC)：Ratatui 5 面板布局，无全屏重绘问题（ratatui diff 机制）。但面板内容全部为硬编码静态字符串，未接入 EventBus。事件循环使用 sync crossterm poll（100ms timeout）——这是正确模式。TUI 命令在 `#[tokio::main]` 下空跑多线程运行时，建议用 `current_thread`。

**chtc-bridge** (1,546 LOC)：5 个 IDE 适配器（vscode/intellij/vim/emacs/zed），ProtocolConverter 无状态设计 + round-trip 一致性验证。但仅 VSCode 的 `execute()` 完整实现，其余返回 NotImplemented。参数为 `serde_json::Value` 动态类型，无 schema 验证。

**mcp-mesh** (2,155 LOC)：Superposition（JoinSet fanout + timeout 部分结果返回）是有意义的并发抽象。但 Entanglement 以命名包装为主，Transaction 2PC 为 in-process mock（sleep 模拟网络往返），无持久化 WAL、无参与者故障检测。ServerRegistry DashMap 并发安全，SSRF 校验覆盖 IPv4/IPv6 全保留段 + 云元数据域名黑名单，但缺少 DNS rebinding 防御。

**csn-substitutor** (2,014 LOC)：余弦相似度 Top-K 替代 + 多级降级链。替代候选无自动发现机制，全部手动注册。降级链耗尽仅 warn 日志无告警事件——监控盲区。

### 1.11 横切关注点

**unwrap() 审计**：全项目 2,965 处 `.unwrap()`，其中 97.6% 在测试代码、1.4% 在基准测试、0.7% 在文档注释、**0 处在生产代码**。项目的 unwrap 治理水平极高，与 `#![forbid(unsafe_code)]` 铁律一致。

**clone 密集度**：mlc-engine(63 处)、cmt-tiering(45 处)、scc-cache(42 处) clone 最密集。许多是 String/Vec 深拷贝，在热路径上产生不必要的堆分配。hcw-window 已做 M-01/M-02 Arc 优化，但其他 crate 未跟进。

**文档矛盾**：层级编号冲突（OMEGA_ULTIMATE 的 L0-L10 vs CODE_WIKI 的 L1-L10）、ADR 编号两套体系冲突、crate 数量（37 规划 vs 34 实际，CODE_WIKI 已自我修正）。

**async/sync 边界模糊**：gsoe-evolution 的 `evolve_once`、auto-dpo 的 `generate` 标记为 async 但内部无真正 await 点，可能误导调用方。

---

## 2. 26 项已有方案审计 [Confidence: High]

| # | 方案 | 优先级 | 状态 | 证据 |
|---|------|--------|------|------|
| I1 | MoE-Inspired Sparse Model Routing | P0 | **未实施** | strategies.rs 仅 3 种简单策略，无稀疏门控 |
| I2 | MSA-Inspired Two-Stage Vector Search | P0 | **未实施** | vector.rs 仅 O(n) 暴力 KNN |
| I3 | Speculative DAG Execution | P0 | **未实施** | engine.rs 线性链分解，无投机执行 |
| I4 | Priority Residual Event Stream | P0 | **未实施** | 仅二级 Normal/Critical 分类，无优先队列 |
| I5 | Hierarchical Latent Context Compression | P1 | **未实施** | clv.rs 仅固定 512-dim，无分层压缩 |
| I6 | GRPO-Inspired Adaptive Task Scoring | P1 | **未实施** | 无组内相对比较评分 |
| I7 | OS-Memory Wiki with Meta-Forgetting | P1 | **未实施** | 无元遗忘机制 |
| I8 | CISPO-Inspired Asymmetric Budget Control | P1 | **未实施** | cacr.rs 仅双阈值线性判定 |
| I9 | Proactive Security Invariants (QK-Clip) | P2 | **未实施** | 标注 Week 6 TODO |
| I10 | Adaptive Thinking Budget Router | P2 | **基础就绪** | OSA budget 维度已实现，缺上层算法 |
| A1 | spawn_blocking 封装 | P0 | **未实施** | checkpoint.rs 全程同步 fs 操作 |
| A2 | UUIDv7 时间戳零反序列化 | P0 | **已实施** | checkpoint.rs L73 `Uuid::now_v7()` |
| A3 | WikiStore 异步读写分离 | P0 | **未实施** | 单 Mutex<Connection> 共享 |
| B1 | VectorIndex Mutex→RwLock | P0 | **未实施** | vector.rs L34 仍为 Mutex |
| B2 | HNSW 混合 ANN 索引 | P2 | **未实施** | 仅内存暴力 KNN |
| B3 | ModelRegistry DashMap→RwLock | P0 | **未实施** | registry.rs 仍为 DashMap |
| C1 | 事件主题过滤层 | P1 | **未实施** | subscribe() 返回全量广播 |
| C2 | 双格式序列化策略 | P1 | **部分实施** | msgpack/json 函数存在，无自动选择 |
| D1 | SQLite 读写分离连接池 | P1 | **未实施** | 单连接 Mutex |
| E1 | 懒配置加载 | P2 | **未实施** | config.rs 全量 eager 加载 |
| E2 | Release Profile 调优 | P2 | **已实施** | Cargo.toml:228-233 完全匹配 |
| F1 | 配置类型统一 | P1 | **未实施** | nexus-core 无 config.rs |
| F2 | 骨架 crate trait 前置 | P1 | **未实施** | 骨架 crate 无性能前置约束 |
| F3 | 依赖铁律 CI 守护 | P1 | **部分实施** | 仅 cargo-audit，无新增依赖审批 |
| G1 | AuditChain 并发化 | P2 | **未实施** | 标注 Week 6 TODO |
| G2 | Prometheus 指标导出 | P2 | **未实施** | BusLogger 为 tracing 层，无 Prometheus |

**审计汇总**：已实施 3/26（12%）、部分实施 2/26（8%）、未实施 21/26（81%）。P0 级方案 8 项中仅 1 项已实施（A2），7 项待修复。

---

## 3. 19 项新发现 [Confidence: High]

以下为两份已有报告未覆盖的新发现，由本次 6 路子代理分析识别：

| # | 名称 | 严重度 | 目标 | 描述 |
|---|------|--------|------|------|
| N1 | seccore cmd.exe 绕过 | **Critical** | seccore/policy.rs | cmd 在白名单中，`cmd /c "危险命令"` 绕过四层防御 |
| N2 | SSRA 主导策略选取 bug | **P0** | ssra-fusion | select_nth_unstable_by 后取 [0] 不保证是最大值 |
| N3 | QEEP Ack 未创建 | **P0** | qeep-protocol | 三元组协议仅实现二元组，Ack 阶段缺失 |
| N4 | ASA 空关键字绕过 | **High** | seccore/asa.rs | 评分依赖调用者提供风险关键字，空列表即绕过 |
| N5 | AuditChain 后置记录 | **High** | seccore/audit.rs | 命令执行后才追加，append 失败则无审计痕迹 |
| N6 | ACB/DECB 矛盾指令 | **P1** | acb+decb governor | 双治理器无仲裁层，TTG 仅订阅 DECB |
| N7 | ACB 振荡风险 | **P1** | acb-governor | 无时间维度滞后，利用率波动导致频繁切换 |
| N8 | Skeptic 否决无覆议 | **P1** | parliament/voting.rs | Skeptic 权重最高+否决优先，可能导致决策僵局 |
| N9 | L6 链路无顺序保证 | **P1** | 5 个 router crate | 五层路由通过事件解耦，顺序靠约定无代码强制 |
| N10 | 马尔可夫链无容量上限 | **P1** | scc-cache/prefetch.rs | 转移矩阵无限增长，长时间运行内存泄漏 |
| N11 | CACR f32 精度丢失 | **P2** | model-router/cacr.rs | 大预算 u64>2^24 时 f32 阈值判定误差 |
| N12 | NexusState 深拷贝 | **P2** | nexus-core/state.rs | get_quest() 返回深拷贝而非 Arc 共享 |
| N13 | NMC Perceptor 串行 | **P2** | nmc-encoder | 5 模态 perceive() 串行处理 |
| N14 | GQEP 缺全局超时 | **P2** | gqep-executor | 仅有单操作超时，大规模 gather 超时累积 |
| N15 | FTS5 全文索引缺失 | **P2** | repo-wiki | LIKE '%query%' 全表扫描 |
| N16 | CSN 降级链耗尽无告警 | **P2** | csn-substitutor | ChainExhausted 仅 warn 日志 |
| N17 | GEA 缓存哈希低效 | **P2** | gea-activator | 每次做 serde_json 序列化 |
| N18 | TTG 事件未集成 EventBus | **P2** | quest-engine/ttg.rs | 模式切换走 tracing::info 而非事件 |
| N19 | EDSB 次优选择局限 | **P2** | faae-router | 仅考虑 top-2，建议扩展为非最热中最高相似度 |

---

## 4. Action Plan

### Phase I: 安全紧急修复（2 天，不破坏现有测试）

- [ ] **N1 [Critical]**: seccore/policy.rs — 将 `cmd` 从命令白名单移除，或增加 `cmd /c` 参数的二次校验。修复后运行 `cargo test -p seccore` 验证。
- [ ] **N4 [High]**: seccore/asa.rs — 当风险关键字列表为空时返回 `RiskLevel::Unknown` 而非 `Low`，触发额外的审计检查。
- [ ] **N5 [High]**: seccore/audit.rs — 将审计记录追加改为 pre-execution append（执行前记录意图），命令完成后更新状态。

### Phase II: 正确性修复（3 天）

- [ ] **N2 [P0]**: ssra-fusion — 将 `selected[0]` 替换为 `selected.iter().max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Equal))` 确保主导策略为最大权重。
- [ ] **N3 [P0]**: qeep-protocol — 在 Request→Receipt 链路中增加 Ack 创建点，实现完整三元组。
- [ ] **A1 [P0]**: quest-engine/checkpoint.rs — 将 `save()`/`load()`/`load_latest()`/`prune_old()` 的 `fs::write`/`fs::read` 包装到 `tokio::task::spawn_blocking`。

### Phase III: P0 性能优化（1 周）

- [ ] **B1**: repo-wiki/vector.rs — `Mutex<HashMap>` → `RwLock<HashMap>`，`lock()` → `read()`/`write()`。预估 1h。
- [ ] **B3**: model-router/registry.rs — `DashMap<String, ModelInfo>` → `RwLock<HashMap<String, ModelInfo>>`。预估 2h。
- [ ] **N10**: scc-cache/prefetch.rs — 为马尔可夫链转移矩阵增加 LRU 淘汰（容量上限 10000）。预估 2h。
- [ ] **A3**: repo-wiki/store.rs — 写操作通过 mpsc channel 序列化到专用写入线程，读操作通过 spawn_blocking。预估 4h。
- [ ] **N11**: model-router/cacr.rs — `f32` 预算计算改为 `u64` 整数运算。预估 30min。

### Phase IV: P1 架构补债（2 周）

- [ ] **C1**: event-bus — 增加 `EventTopic` 枚举（7 类）+ `FilteredSubscriber`，每个订阅者仅接收相关事件。
- [ ] **N6/N7**: acb-governor — 增加时间维度滞后机制（参照 DECB 的 `tier_switch_lag_ms`），并在 TTG 中增加 ACB/DECB 仲裁层。
- [ ] **N8**: parliament — 增加 Skeptic 否决覆议机制（其他 4 角色以 2/3 超级多数可推翻否决）。
- [ ] **N9**: sesa-router（链路末端）— 增加前置事件校验（验证是否收到 OSA + KVBSR + FaaE 事件）。
- [ ] **F1**: 将 chimera-cli/config.rs 的 14 个 section 类型迁移到 nexus-core/config.rs，各 crate 通过共享引用消除重复。
- [ ] **D1**: repo-wiki — 引入 r2d2 连接池，单写入连接 + 只读连接池利用 WAL 并发读。

### Phase V: P2 渐进优化（4 周）

- [ ] **I4**: event-bus — Critical 事件走 mpsc 保证投递 + Normal 事件走 broadcast + 注意力过滤。
- [ ] **I1**: model-router — MoE 稀疏门控路由，50+ 模型时 O(n)→O(k)。
- [ ] **N14**: gqep-executor — 增加 `gather_deadline_ms` 全局超时。
- [ ] **N17**: gea-activator — 为 TaskProfile 实现 `Hash` trait 替代 serde_json 序列化。
- [ ] **N18**: quest-engine/ttg.rs — 将模式切换事件从 tracing::info 迁移到 EventBus。
- [ ] **N15**: repo-wiki — 启用 SQLite FTS5 扩展替代 LIKE 全表扫描。
- [ ] **E1**: chimera-cli — OnceCell 懒初始化配置各 section。
- [ ] **G2**: event-bus/BusLogger — 注册 Prometheus 指标。

---

## 5. Open Questions & Caveats

1. **Placeholder 真实化时间表不确定**：MTPE（FNV-1a 伪预测）、PVL（SHA-256 伪置信度）、OSA（index 降序评分）、NMC（Image/Video/Audio 占位感知器）的核心功能为 placeholder，真实化依赖外部模型接入（ort ONNX runtime、LLM speculative decoding），时间线取决于模型服务可用性。

2. **MCP 2PC 的分布式真实化**：当前 mcp-mesh 的事务为 in-process mock（sleep 模拟网络往返），生产化需要跨进程通信 + 持久化 WAL + 参与者故障检测，工作量预估 1-2 周。

3. **gVisor 跨平台缺口**：seccore 的沙箱从 gVisor 降级为进程隔离，在 Windows/macOS 上的安全等级低于 Linux。无替代方案的明确规划。

4. **双治理器（ACB/DECB）的设计意图**：两个独立治理器是有意为之（不同治理维度）还是架构遗留？当前 TTG 仅订阅 DECB 事件，ACB 事件无消费者——如果 ACB 是有意设计，需要补充联动机制；如果是遗留，应考虑合并。

5. **256 专家硬上限**：SESA 的 256-bit 位掩码是编译期常量，未来专家数 > 256 时需要多 mask 分片。建议文档化为架构约束或提前设计分片方案。

6. **文档矛盾未消除**：OMEGA_ULTIMATE.md 中的 "37 crates" 与 Cargo.toml 的 34 crate 矛盾、两套 ADR 编号体系冲突、层级编号不一致（L0-L10 vs L1-L10）——建议以 CODE_WIKI.md + Cargo.toml 为单一权威源进行文档对齐。

---

## 6. 模块质量评分总览

| 层级 | Crate | LOC | 评分 | 关键问题 |
|------|-------|-----|------|---------|
| L1 | nexus-core | 1,432 | A | 余弦相似度无 SIMD |
| L1 | event-bus | 3,173 | A- | types.rs 1750 行过大 |
| L1 | model-router | 1,547 | A- | DashMap 选型 / f32 精度 |
| L2 | nmc-encoder | 1,784 | A | 3 个感知器占位 |
| L2 | mlc-engine | 4,540 | B+ | 线性 KNN / clone 密集 |
| L2 | hcw-window | 2,639 | A | Arc CoW 优化优秀 |
| L3 | cmt-tiering | 4,844 | B+ | SQLite 单连接 Mutex |
| L3 | scc-cache | 2,558 | A- | 马尔可夫链无上限 |
| L3 | lsct-tiering | 1,794 | A | 无持久化 |
| L4 | seccore | 2,044 | B | **cmd.exe Critical 漏洞** |
| L4 | qeep-protocol | 561 | B+ | Ack 未创建 |
| L4 | decay-engine | 421 | A | 全项目最小最精 |
| L5 | repo-wiki | 2,025 | B+ | Mutex KNN / 单连接 |
| L5 | gsoe-evolution | 1,669 | B+ | async/sync 边界模糊 |
| L5 | auto-dpo | 939 | A | 简洁正确 |
| L6 | osa-coordinator | 1,704 | 8/10 | 评分 placeholder |
| L6 | kvbsr-router | 2,218 | 9/10 | 块向量更新延迟 |
| L6 | faae-router | 1,799 | 8/10 | EDSB 次优局限 |
| L6 | gea-activator | 1,596 | 7/10 | 缓存哈希低效 |
| L6 | sesa-router | 2,034 | 8/10 | 256 上限硬编码 |
| L7 | pvl-layer | 1,670 | 8/10 | confidence 伪随机 |
| L7 | gqep-executor | 1,497 | 8/10 | operation_id 丢失 |
| L7 | mtpe-executor | 1,063 | 6/10 | 核心 placeholder |
| L7 | ssra-fusion | 1,413 | 7/10 | **主导策略 bug** |
| L8 | parliament | 5,870 | 8.5/10 | Skeptic 无覆议 |
| L8 | decb-governor | 2,430 | 8.0/10 | 滞后参数无下限 |
| L8 | acb-governor | 1,272 | 7.5/10 | 振荡 + DECB 矛盾 |
| L9 | quest-engine | 2,884 | 7.0/10 | **sync I/O P0** |
| L9 | efficiency-monitor | 2,182 | 8.0/10 | Critical 无速率限制 |
| L10 | chimera-cli | 1,521 | 7/10 | 5 个子命令骨架 |
| L10 | chimera-tui | 1,268 | 7.5/10 | 面板静态文本 |
| L10 | chtc-bridge | 1,546 | 8/10 | 4 个 IDE 未实现 |
| L10 | mcp-mesh | 2,155 | 7/10 | 2PC mock |
| L10 | csn-substitutor | 2,014 | 8/10 | 无自动发现 |

---

## 7. Methodology

**研究深度**：Deep（6 路子代理并行分析 + 7 份架构文档交叉比对 + 2 份已有研究报告审计）

**子代理分工**：
- Agent A（性能与存储）：L1 Core + L2 Memory + L3 Storage + L5 Knowledge = 14 crate
- Agent B（算法与执行）：L6 Router + L7 Execution = 9 crate
- Agent C1（安全层）：L4 Security = 3 crate
- Agent C2（治理层）：L8 Parliament + L9 Quest = 5 crate
- Agent D（接口与横切）：L10 Interface = 5 crate + 全项目 unwrap/配置/文档/构建/测试
- 文档交叉比对：7 份架构文档（FULL_DOCUMENTATION / GEN2_INNOVATIONS / GEN3_OMEGA / OMEGA_ULTIMATE / CODE_WIKI / BUILD_GUIDE / CHANGELOG）

**分析维度**：算法正确性验证、性能瓶颈量化、安全漏洞扫描、架构合规审计、代码质量评分、已有方案实施状态核查

**三角验证**：每个 P0 发现由至少 2 个独立分析路径确认（如 checkpoint sync I/O 由 Agent A 和 Agent C2 独立发现；SSRA bug 由 Agent B 从代码逻辑和数学证明双路径确认）

**局限性**：Agent C（安全与治理）首次返回空结果，经替代子代理重试后成功，安全层分析的深度可能略低于其他领域。cmd.exe 绕过漏洞的发现来自代码模式分析而非实际渗透测试，建议后续通过 OWASP 渗透验证。

---

## 8. 内部参考来源

[1] `DEEP_RESEARCH_LLM_ARCHITECTURE_MAPPING.md` — LLM 架构映射创新报告（10 项创新，43 来源）
[2] `DEEP_RESEARCH_OPTIMIZATION_ALGORITHM.md` — 全链路优化算法报告（16 项优化，27 来源）
[3] `AETHER_NEXUS_OMEGA_从零搭建完全指南.md` — 12 周 84 天实施路线图，十层架构设计
[4] `OMEGA_大模型架构魔改创新_AI_Agent项目套用设计.md` — 12 大架构改造方案
[5] `AETHER_NEXUS_FULL_DOCUMENTATION.md` — Gen1 完整项目文档
[6] `CODE_WIKI.md` — 权威代码 Wiki（层级/crate 数量/ADR 的最终裁定源）
[7] `CHANGELOG.md` — Week 1-8 变更日志，3002+ 测试累计
[8] `CHIMERA_NEXUS_GEN2_INNOVATIONS.md` — Gen2 10 项创新
[9] `AETHER_NEXUS_GEN3_OMEGA.md` — Gen3 10 项创新
[10] `AETHER_NEXUS_OMEGA_ULTIMATE.md` — OMEGA 终极版文档
[11] `CHIMERA_NEXUS_COMPLETE_BUILD_GUIDE.md` — 完整构建指南
[12] `Cargo.toml` — 34 crate workspace 定义（权威 crate 列表）
