//! NexusEvent 变体分组 · 执行 / 路由 / 记忆 / 知识 / 存储
//!
//! 本文件承载 `NexusEvent` 的连续一段变体（声明序 `OperationProduced` … `ExpertUnregistered`），
//! 经 `macro_rules!` 链式装配进 [`NexusEvent`](crate::types::NexusEvent)。
//!
//! 域: L7 执行产出与验证、L6 路由与激活、L2 记忆分层、L5 知识沉淀、L3 缓存,
//! 以及 Week 5 前引入的审计与 MCP 通道事件。
//!
//! # WHY 宏装配而非子枚举
//!
//! `NexusEvent` 必须是**单一枚举**:serde 线格式以 `tag = "type"` 区分变体、
//! 155 变体计数锁(`crates/event-bus/tests/variant_count_test.rs`)与
//! `severity()/topic()/type_name()/metadata()` 的无通配符穷尽 match 都依赖它。
//! 而 Rust 既不允许跨文件拆分 enum 定义,也不允许宏在枚举变体位展开
//! (`macros cannot expand to enum variants`),所以按"令牌累加链"装配:
//! 本段宏把自身变体追加到累加器 `$($acc)*` 后调用下一段宏,最后由
//! `types/mod.rs` 的 `define_nexus_event!` 一次性落成枚举。
//!
//! # 维护规约
//!
//! 新增/删除变体:在本文件按声明序插入,并同步 `registry.rs` 的定级行;
//! `types/mod.rs` 的装配链无需改动(段边界按语义域固定)。声明顺序即
//! `variant_ordinals()` 的序数,不得重排既有变体。

macro_rules! variants_execution {
    ($($acc:tt)*) => {
        crate::types::governance::variants_governance! {
            $($acc)*

            // ============================================================
            // L7 Execution → L6 Router:操作产出
            // ============================================================
            /// PVL 生产验证完成一个操作,Router 据此路由
            OperationProduced {
                /// 事件元数据
                metadata: EventMetadata,
                /// 操作 ID
                op_id: String,
                /// 产出内容哈希
                content_hash: String,
            },

            /// PVL 验证评分,用于内部质量门控(同层通信)
            PredictionVerified {
                /// 事件元数据
                metadata: EventMetadata,
                /// 操作 ID
                op_id: String,
                /// 验证分数 [0.0, 1.0]
                score: f32,
            },

            // ============================================================
            // L6 Router → L5 Knowledge / L2 Memory:稀疏掩码计算
            // ============================================================
            /// OSA 计算完全维稀疏掩码 — 修正 V1 违规
            ///
            /// WHY:原架构 OSA 直接 import HCW(向上依赖 L6→L2),
            /// 改为发布此事件,HCW 订阅消费,符合 §2.2 依赖铁律
            ///
            /// # SubTask 14.3 改进
            /// 事件携带 `context_mask`(FileId 的字符串形式),HCW 订阅后直接使用,
            /// 无需再通过共享存储拉取。WHY 用 `Vec<String>` 而非 `Vec<FileId>`:
            /// event-bus 在 L1,不能依赖 OSA(L6)的 FileId newtype(向上依赖违规)
            OmniSparseMasksComputed {
                /// 事件元数据
                metadata: EventMetadata,
                /// 掩码哈希,消费者据此拉取具体掩码数据
                mask_hash: String,
                /// 稀疏度 [0.0, 1.0],1.0 表示全稀疏
                sparsity: f32,
                /// context 维度活跃文件 ID 列表(FileId 的字符串形式)
                ///
                /// WHY:event-bus 在 L1,不能依赖 OSA(L6)的 FileId newtype,
                /// 用 `Vec<String>` 传递。OSA 的 FileId 实现了 Display trait,
                /// 发布时通过 `f.to_string()` 转换;HCW 订阅后直接使用
                context_mask: Vec<String>,
            },

            /// FaaE 工具路由完成,Knowledge 层据此更新工具索引
            ToolsRouted {
                /// 事件元数据
                metadata: EventMetadata,
                /// 已路由工具数
                routed_count: u32,
                /// 最匹配工具 ID
                top_tool: String,
                /// SubTask 17.3:已路由工具 ID 列表(默认 Top-8 工具 ID 的字符串形式)
                ///
                /// WHY:原事件仅携带 `top_tool`(单个工具),消费者无法获知完整路由结果。
                /// 新增 `routed_tools` 字段携带完整 Top-K 工具列表,供订阅者(如 GEA
                /// 激活器)进行后续工具调度决策。
                ///
                /// 向后兼容:`#[serde(default)]` 确保旧格式数据(无此字段)反序列化为空 Vec,
                /// 旧消费者忽略此字段,新消费者检查 `is_empty()` 判断是否为旧格式。
                #[serde(default)]
                routed_tools: Vec<String>,
            },

            // ============================================================
            // L6 Router → L9 Quest:执行完成
            // ============================================================
            /// 执行流程完成,Quest 据此推进或收尾
            ExecutionCompleted {
                /// 事件元数据
                metadata: EventMetadata,
                /// Quest ID
                quest_id: String,
                /// 结果哈希
                result_hash: String,
            },

            // ============================================================
            // L2 Memory → L9 Quest:记忆指标上报 — 修正 V2 违规
            // ============================================================
            /// MLC 上报记忆指标 — 修正 V2 违规
            ///
            /// WHY:原架构 MLC 直接 import efficiency-monitor(跨层违规),
            /// 改为发布此事件,efficiency-monitor 订阅消费
            MemoryMetricsReported {
                /// 事件元数据
                metadata: EventMetadata,
                /// 缓存命中率 [0.0, 1.0]
                hit_rate: f32,
                /// 周期内驱逐数
                evictions: u64,
            },

            /// 记忆分层完成,CMT/LSCT 据此迁移数据
            MemoryTiered {
                /// 事件元数据
                metadata: EventMetadata,
                /// 目标分层(Hot/Warm/Cold/Ice)
                tier: String,
                /// 该层条目数
                item_count: u32,
                /// SubTask 17.4:被迁移的记忆条目 ID(单条迁移时填充,批量迁移时为 None)
                ///
                /// WHY:原事件仅携带 `tier` 与 `item_count`,消费者无法定位具体被迁移的条目。
                /// 新增 `memory_id` 字段,单条 promote/demote 迁移时填充条目 ID,
                /// 供订阅者(如 efficiency-monitor)更新条目位置索引。
                /// 批量迁移场景(如衰减周期批量降级)为 None,消费者据此区分单条/批量。
                ///
                /// 向后兼容:Option 类型 + `#[serde(default)]` 确保旧格式数据(无此字段)
                /// 反序列化为 None,不影响现有消费者逻辑。
                #[serde(default)]
                memory_id: Option<String>,
            },

            // ============================================================
            // L3 Storage → L6 Router:缓存命中/未命中
            // ============================================================
            /// SCC 缓存命中,Router 跳过重复计算
            CacheHit {
                /// 事件元数据
                metadata: EventMetadata,
                /// 缓存键
                cache_key: String,
            },

            /// SCC 缓存未命中,Router 触发计算
            CacheMiss {
                /// 事件元数据
                metadata: EventMetadata,
                /// 缓存键
                cache_key: String,
            },

            // ============================================================
            // L5 Knowledge → L9 Quest:知识沉淀
            // ============================================================
            /// Repo Wiki 更新完成,Quest 据此刷新上下文
            WikiUpdated {
                /// 事件元数据
                metadata: EventMetadata,
                /// Wiki 内容哈希
                wiki_hash: String,
                /// 增量条目数
                delta: u32,
            },

            /// GSOE 触发在线进化(同层通信)
            ///
            /// **[RESERVED]** §16.4 审计结论:变体已落地但生产发布端缺失
            /// (gsoe-evolution 零生产构造)。标注 reserved 保留序列化兼容,
            /// 待后续装配时激活(Phase 10 Wave 4 孤儿治理)。
            EvolutionTriggered {
                /// 事件元数据
                metadata: EventMetadata,
                /// 进化世代数
                generation: u64,
                /// 当前适应度
                fitness: f32,
            },

            /// AutoDPO 生成训练对(同层通信)
            DpoPairGenerated {
                /// 事件元数据
                metadata: EventMetadata,
                /// 训练对 ID
                pair_id: String,
                /// 被选中的输出
                chosen: String,
                /// 被拒绝的输出
                rejected: String,
            },

            // ============================================================
            // L6 Router → L4 Security:审计日志
            // ============================================================
            /// 审计日志已记录,SecCore 据此做合规检查
            AuditLogged {
                /// 事件元数据
                metadata: EventMetadata,
                /// 审计记录哈希
                audit_hash: String,
                /// 严重级别
                severity: String,
            },

            // ============================================================
            // L10 Interface:MCP 网格消息
            // ============================================================
            /// MCP 网格收到远端消息(同层通信)
            McpMessageReceived {
                /// 事件元数据
                metadata: EventMetadata,
                /// 源节点标识
                source_node: String,
                /// 消息类型
                msg_type: String,
            },

            // ============================================================
            // 系统级:背压告警 [Critical]
            // ============================================================
            /// 慢消费者被丢弃 `[Critical]` — 系统健康告警
            ///
            /// WHY:此事件本身标注 Critical,确保运维层必定收到告警
            SlowConsumerDropped {
                /// 事件元数据
                metadata: EventMetadata,
                /// 被丢弃的订阅者标识
                subscriber_id: String,
                /// 滞后事件数
                lag: u64,
                /// 被丢弃事件总数
                dropped_count: u64,
            },

            // ============================================================
            // Week 3 扩展:HCW/CMT/KVBSR 跨层通信事件
            //
            // WHY:Week 3 新增三个 crate(hcw-window/cmt-tiering/kvbsr-router),
            // 它们通过 EventBus 发布状态变更,符合 §2.2 依赖铁律(跨层通信
            // 只能走 Event Bus)。本分节变体均为 Normal 级别,追加在枚举末尾
            // 以保持向后兼容(不修改现有变体的字段或顺序)。
            // ============================================================
            /// HCW 窗口层级切换 — L2 Memory 内部状态变更
            ///
            /// WHY:HCW 在 L0/L1/L2/L3 四级窗口间自动切换,发布此事件通知
            /// 订阅者(如 efficiency-monitor)更新监控指标
            ContextWindowSwitched {
                /// 事件元数据
                metadata: EventMetadata,
                /// 源窗口层级(如 "L0"/"L1"/"L2"/"L3")
                from_tier: String,
                /// 目标窗口层级
                to_tier: String,
                /// 切换原因(如 "L0 capacity exceeded")
                reason: String,
            },

            /// HCW 上下文压缩完成 — L2 Memory 内部状态变更
            ///
            /// WHY:HCW 在窗口溢出时按重要性评分压缩上下文,发布此事件通知
            /// 订阅者记录压缩率指标,用于后续优化压缩策略
            ContextCompressed {
                /// 事件元数据
                metadata: EventMetadata,
                /// 压缩前大小(字节)
                original_size: u64,
                /// 压缩后大小(字节)
                compressed_size: u64,
                /// 压缩率 [0.0, 1.0],compressed_size / original_size
                ratio: f32,
            },

            /// CMT 能力分层迁移 — L3 Storage 内部状态变更
            ///
            /// WHY:CMT 在 Hot/Warm/Cold/Ice 四级间自动迁移能力,发布此事件
            /// 通知订阅者(如 efficiency-monitor)更新能力位置索引
            CapabilityTiered {
                /// 事件元数据
                metadata: EventMetadata,
                /// 能力 ID
                capability_id: String,
                /// 源分层(如 "Hot"/"Warm"/"Cold"/"Ice")
                from_tier: String,
                /// 目标分层
                to_tier: String,
                /// 迁移原因(如 "decay priority below threshold")
                reason: String,
            },

            /// CMT 四层存储统计上报 — L3 Storage 分布快照(L3 深度优化 P1-1)
            ///
            /// WHY:CMT 在 insert/migrate 变更后发布四层条目计数快照,
            /// 供 TUI MemoryPanel 等订阅方从事件流派生存储分布显示
            /// (替代原 tier_distribution() 全局函数占位,事件驱动化)。
            CapabilityTierStatsReported {
                /// 事件元数据
                metadata: EventMetadata,
                /// Hot 层条目数
                hot: u64,
                /// Warm 层条目数
                warm: u64,
                /// Cold 层条目数
                cold: u64,
                /// Ice 层条目数
                ice: u64,
            },

            /// KVBSR 块重平衡完成 — L6 Router 内部状态变更
            ///
            /// WHY:KVBSR 定期分析工具共现频率重建语义块,发布此事件通知
            /// 订阅者刷新块索引缓存,避免使用过期的块路由表
            BlocksRebalanced {
                /// 事件元数据
                metadata: EventMetadata,
                /// 重平衡前的块数量
                old_block_count: u32,
                /// 重平衡后的块数量
                new_block_count: u32,
            },

            // ============================================================
            // Week 4 扩展:执行优化层(L6 + L7)跨层通信事件
            //
            // WHY:Week 4 新增六个 crate(gea-activator/gqep-executor/pvl-layer/
            // mtpe-executor/scc-cache/faae-router),它们通过 EventBus 发布状态
            // 变更,符合 §2.2 依赖铁律(跨层通信只能走 Event Bus)。
            // ============================================================
            /// GEA 专家激活完成 — L6 Router 状态变更
            ///
            /// WHY:GEA 计算门控值并冲突消解后,发布此事件通知订阅者(如 PVL)
            /// 已激活的专家列表,供后续生产验证使用
            ExpertActivated {
                /// 事件元数据
                metadata: EventMetadata,
                /// 已激活专家 ID 列表(Top-K)
                activated_experts: Vec<String>,
                /// 被抑制专家 ID 列表
                suppressed_experts: Vec<String>,
                /// 综合评分最高的专家门控值 [0.0, 1.0]
                top_gate_value: f32,
            },

            /// GEA 激活阈值动态调整 — L6 Router 状态变更
            ActivationThresholdAdjusted {
                /// 事件元数据
                metadata: EventMetadata,
                /// 旧阈值
                old_threshold: f32,
                /// 新阈值
                new_threshold: f32,
                /// 负载因子 [0.0, 1.0]
                load_factor: f32,
            },

            /// GEA 激活缓存统计 — L6 Router 内部指标
            ActivationCacheStats {
                /// 事件元数据
                metadata: EventMetadata,
                /// 缓存命中率 [0.0, 1.0]
                hit_rate: f32,
                /// 缓存条目数
                entry_count: u32,
            },

            /// GQEP 聚集执行完成 — L6 Router 状态变更
            GatherCompleted {
                /// 事件元数据
                metadata: EventMetadata,
                /// 总操作数
                total: u32,
                /// 成功操作数
                succeeded: u32,
                /// 失败操作数
                failed: u32,
                /// 聚集延迟(毫秒)
                latency_ms: f32,
            },

            /// GQEP 操作超时 — L6 Router 状态变更
            OperationTimedOut {
                /// 事件元数据
                metadata: EventMetadata,
                /// 超时操作 ID
                operation_id: String,
                /// 超时阈值(毫秒)
                timeout_ms: u64,
            },

            /// GQEP 全局 gather 超时 — L6 Router 状态变更(Phase V Task V-3 [N14])
            ///
            /// 整个 gather 流程触达全局 deadline,剩余未完成的 future 被放弃。
            /// 与 `OperationTimedOut`(单操作超时)互补,二者构成双层超时防护:
            /// 单操作超时保护单个 future,全局超时保护整个 gather 流程不因单操作
            /// 超时累积而失控。供 efficiency-monitor 等订阅者记录全局超时指标。
            ///
            /// ADR-202 I5 扩展:并发上界(分批入流)引入 `not_started` —— 它标识
            /// **从未入流、因而从未被 poll** 的操作,这类操作**不会触发 QEEP
            /// `OrphanGuard`**(注册发生在 async 体首次执行),故若无本字段则只能静默消失。
            /// `abandoned` 与 `not_started` 语义不可合并:前者已执行过(重试可能重复副作用),
            /// 后者根本没跑(可安全重试)。
            GatherTimedOut {
                /// 事件元数据
                metadata: EventMetadata,
                /// 全局 deadline 阈值(毫秒),即 `GqepConfig::gather_deadline_ms`
                deadline_ms: u64,
                /// 触发超时时实际已运行时间(毫秒)
                elapsed_ms: u64,
                /// 本次 gather 的总操作数
                total: u32,
                /// 已入流被 poll、但因全局 deadline 放弃而未完成的操作数
                abandoned: u32,
                /// 从未入流、因而从未被 poll 的操作数(并发上界下的 backlog 尾部)
                ///
                /// WHY `#[serde(default)]`:本字段是后继追加的,旧版本持久化/投递的
                /// payload 里没有它;不标则反序列化直接失败。沿用本枚举内已有惯例。
                #[serde(default)]
                not_started: u32,
            },

            /// GQEP 检测到孤儿调用 `[Critical]` — 系统健康告警
            ///
            /// WHY:对应 Claude Code 尸检 5.4% 孤儿调用教训,孤儿调用必须
            /// 标注 Critical 确保运维层必定收到告警
            OrphanCallDetected {
                /// 事件元数据
                metadata: EventMetadata,
                /// 孤儿操作 ID
                operation_id: String,
                /// spawn 位置(文件:行号)
                spawn_location: String,
            },

            /// PVL Producer 策略调整 — L7 Execution 状态变更
            ProducerStrategyAdjusted {
                /// 事件元数据
                metadata: EventMetadata,
                /// 调整原因
                adjustment_reason: String,
                /// 新策略名称
                new_strategy: String,
            },

            /// MTPE 多步预测完成 — L7 Execution 状态变更
            PredictionMade {
                /// 事件元数据
                metadata: EventMetadata,
                /// Quest ID
                quest_id: String,
                /// 预测步数 N
                n: usize,
                /// 平均置信度 [0.0, 1.0]
                avg_confidence: f32,
            },

            /// MTPE 预测成功率统计 — L7 Execution 内部指标
            PredictionStatsReported {
                /// 事件元数据
                metadata: EventMetadata,
                /// 按 N 值分组的成功率(N=1 → 0.95, N=5 → 0.85, ...)
                success_rate_by_n: std::collections::HashMap<usize, f32>,
            },

            /// MTPE 预测失败回退 — L7 Execution 状态变更
            PredictionRolledBack {
                /// 事件元数据
                metadata: EventMetadata,
                /// 失败步序号
                failed_step: usize,
                /// 回退到的步数(通常为 1)
                rollback_to: usize,
            },

            /// SCC 推测性预取完成 — L3 Storage 状态变更
            CachePrefetched {
                /// 事件元数据
                metadata: EventMetadata,
                /// 预取的上下文 ID 列表
                prefetched_ids: Vec<String>,
            },

            /// SCC 缓存统计 — L3 Storage 内部指标
            CacheStatsReported {
                /// 事件元数据
                metadata: EventMetadata,
                /// 缓存命中率 [0.0, 1.0]
                hit_rate: f32,
                /// 驱逐数
                eviction_count: u64,
            },

            /// FaaE 专家路由完成 — L6 Router 状态变更
            ExpertRouted {
                /// 事件元数据
                metadata: EventMetadata,
                /// 路由到的工具 ID
                routed_tool: String,
                /// 路由置信度 [0.0, 1.0]
                confidence: f32,
            },

            /// EDSB 熵均衡完成 — L6 Router 状态变更
            EntropyBalanced {
                /// 事件元数据
                metadata: EventMetadata,
                /// 均衡前熵值
                old_entropy: f32,
                /// 均衡后熵值
                new_entropy: f32,
                /// 重分配的请求数
                redistributed_count: u32,
            },

            /// FaaE 工具专家注册 — L6 Router 状态变更
            ExpertRegistered {
                /// 事件元数据
                metadata: EventMetadata,
                /// 注册的工具 ID
                tool_id: String,
            },

            /// FaaE 工具专家注销 — L6 Router 状态变更
            ExpertUnregistered {
                /// 事件元数据
                metadata: EventMetadata,
                /// 注销的工具 ID
                tool_id: String,
            },
        }
    };
}
pub(crate) use variants_execution;
