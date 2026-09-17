//! 事件类型定义 — NEXUS-OMEGA 全维事件枚举
//!
//! 对应架构:十层架构 L1-L10 的跨层通信契约
//! 设计依据:Part A 依赖方向分析,通过预定义事件类型修正 4 处违规
//!
//! # 关键违规修正映射
//! - V1(OSA→HCW 向上依赖):`OmniSparseMasksComputed` 事件
//! - V2(MLC→efficiency-monitor 跨层):`MemoryMetricsReported` 事件
//! - V3/V4(Parliament→GSOE/AutoDPO 向上依赖):`ConsensusReached` 事件

// 辅助载荷类型从 `payloads` 模块导入,保持向后兼容
pub use crate::payloads::*;
use chrono::{DateTime, Utc};
// ADR-054 决策 6(P9-T7 Task 4):Quest 引用改从 L0 nexus-contracts 导入,
// 消除 event-bus 对 nexus-core 的 Quest 依赖(边解除的一部分)
use nexus_contracts::domain::Quest;
use serde::{Deserialize, Serialize};

/// NEXUS-OMEGA 核心事件枚举 — 跨层通信的唯一契约
///
/// 设计原则:
/// 1. 每个变体对应一条架构层间的数据流(见 §5.2 数据流参考)
/// 2. 变体命名采用"动作完成时态"(PastTense),表达"已发生"事实
/// 3. payload 仅携带消费者必需字段,大对象用 hash 引用
/// 4. 关键事件在文档中标注 `[Critical]`,背压策略据此保护
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "data")]
pub enum NexusEvent {
    // ============================================================
    // L10 Interface → L9 Quest:用户意图编码完成
    // ============================================================
    /// NMC 编码用户意图完成,Quest Engine 据此分解任务
    UserIntentEncoded {
        /// 事件元数据
        metadata: EventMetadata,
        /// 意图 ID
        intent_id: String,
        /// 用户输入原始文本
        raw_text: String,
        /// 风险等级(0-100),影响后续沙箱策略
        risk_level: u8,
    },

    // ============================================================
    // L1 Core → L2 Memory:全局状态变更
    // ============================================================
    /// NexusState 发生变更,MLC 需同步记忆快照
    NexusStateChanged {
        /// 事件元数据
        metadata: EventMetadata,
        /// 新状态哈希(sha256 hex)
        state_hash: String,
        /// 前一状态哈希,用于链式校验
        prev_hash: String,
    },

    // ============================================================
    // L1 Core → L9 Quest:模型路由选定
    // ============================================================
    /// Model Router 选定执行模型,Quest 据此调度
    ModelRouteSelected {
        /// 事件元数据
        metadata: EventMetadata,
        /// Quest ID
        quest_id: String,
        /// 模型 ID
        model_id: String,
        /// 路由原因
        route_reason: String,
    },

    // ============================================================
    // L9 Quest → L8 Parliament:任务生命周期
    // ============================================================
    /// 新 Quest 创建完成,Parliament 开始审议
    QuestCreated {
        /// 事件元数据
        metadata: EventMetadata,
        /// Quest ID
        quest_id: String,
        /// Quest 标题
        title: String,
        /// 任务数量
        task_count: u32,
    },

    /// Quest 进度更新,Parliament 据此评估是否需要干预
    QuestProgressUpdated {
        /// 事件元数据
        metadata: EventMetadata,
        /// Quest ID
        quest_id: String,
        /// 已完成任务数
        completed: u32,
        /// 总任务数
        total: u32,
    },

    /// Quest 完整列表更新 — L9 Quest → L10 Interface(P1.2 实时数据驱动面板)
    ///
    /// WHY:quest-engine 周期性发布完整列表,供 TUI 冷启动或 lag 后快速对齐,
    /// 避免依赖多次增量事件才能拼出完整状态。Normal 级别,丢失可由下次周期补偿。
    /// 注:此变体属于 P1.2 实时数据面板契约,非 M4 双向控制新增。
    QuestListUpdated {
        /// 事件元数据
        metadata: EventMetadata,
        /// 当前活动 Quest 完整列表
        quests: Vec<Quest>,
        /// 列表来源标识(如 "quest-engine")
        source: String,
    },

    /// Quest 已完成 — L9 Quest → L10 Interface(P1.2 实时数据驱动面板)
    ///
    /// WHY:标记 Quest 结束,TUI 据此从活动列表移除。携带 status 以区分
    /// 成功/失败/取消,便于面板展示不同视觉状态。
    /// 注:此变体属于 P1.2 实时数据面板契约,非 M4 双向控制新增。
    QuestCompleted {
        /// 事件元数据
        metadata: EventMetadata,
        /// Quest ID
        quest_id: String,
        /// 完成状态
        status: QuestStatus,
    },

    /// TTG 切换思考模式(快速/标准/深度),Parliament 据此调整预算
    ///
    /// # Week 5 扩展(SubTask 37.1)
    /// 新增 `reason` 字段携带切换原因,供订阅者(如 Parliament)记录
    /// 决策依据。复用现有变体(而非新增 `ThinkingModeChanged`)以保持
    /// 向后兼容:字段名保持 `from_mode`/`to_mode` 不变,避免破坏
    /// 已序列化数据与下游 match 模式。
    ThinkingModeSwitched {
        /// 事件元数据
        metadata: EventMetadata,
        /// Quest ID
        quest_id: String,
        /// 源思考模式
        from_mode: String,
        /// 目标思考模式
        to_mode: String,
        /// 切换原因(如 "complexity threshold exceeded")
        ///
        /// 向后兼容:`#[serde(default)]` 确保旧格式数据(无此字段)
        /// 反序列化为空字符串,旧消费者忽略此字段,新消费者检查
        /// `is_empty()` 判断是否为旧格式。
        #[serde(default)]
        reason: String,
    },

    // ============================================================
    // L9 Quest → L10 Interface:检查点持久化 [Critical]
    // ============================================================
    /// 检查点已保存 `[Critical]` — 丢失将导致 Quest 无法恢复
    ///
    /// 背压策略:标注 Critical,建议走 mpsc 点对点通道确保投递
    CheckpointSaved {
        /// 事件元数据
        metadata: EventMetadata,
        /// Quest ID
        quest_id: String,
        /// 检查点 ID
        checkpoint_id: String,
        /// 记忆快照哈希,恢复时校验完整性
        memory_snapshot_hash: String,
    },

    /// 检查点已加载,Quest 从断点恢复
    CheckpointLoaded {
        /// 事件元数据
        metadata: EventMetadata,
        /// Quest ID
        quest_id: String,
        /// 检查点 ID
        checkpoint_id: String,
    },

    // ============================================================
    // L8 Parliament → L7 Execution / L5 Knowledge:共识达成
    // ============================================================
    /// 议会达成共识 `[Critical]` — 修正 V3/V4 违规
    ///
    /// WHY:原架构 Parliament 直接 import GSOE/AutoDPO(向上依赖),
    /// 改为发布此事件,GSOE/AutoDPO 订阅消费,符合 §2.2 依赖铁律
    ConsensusReached {
        /// 事件元数据
        metadata: EventMetadata,
        /// Quest ID
        quest_id: String,
        /// 决议内容哈希
        decision_hash: String,
        /// 若共识产生 DPO 训练对,携带 pair_id 供 AutoDPO 消费
        dpo_pair_id: Option<String>,
    },

    /// 议员投票,用于议会内部计票(同层通信)
    VoteCast {
        /// 事件元数据
        metadata: EventMetadata,
        /// 提案 ID
        proposal_id: String,
        /// 投票者标识
        voter: String,
        /// true=赞成,false=反对
        vote: bool,
    },

    // ============================================================
    // L4 Security → L8 Parliament:能力冻结
    // ============================================================
    /// 能力被 Decay Engine 冻结,Parliament 据此撤销对应权限
    CapabilityFrozen {
        /// 事件元数据
        metadata: EventMetadata,
        /// 能力 ID
        capability_id: String,
        /// 冻结原因
        reason: String,
    },

    /// 影子模式熔断器跳闸 — L4 Security fail-closed 状态变更(L4 深度优化 P1-1)
    ///
    /// WHY:ShadowModeCircuitBreaker 检测到 FormalVerifier 属性违规永久跳闸时
    /// 发布(不可逆直至人工复位),供 TUI DecayPanel 等订阅方从事件流派生
    /// 熔断状态显示(替代原 shadow_breaker_status() 全局函数占位)。
    ShadowBreakerTripped {
        /// 事件元数据
        metadata: EventMetadata,
        /// 跳闸原因(形式化属性违反反例描述)
        reason: String,
    },

    // ============================================================
    // L3 Storage → L8 Parliament:预算超限
    // ============================================================
    /// 预算超限,Parliament 据此触发降级或终止
    BudgetExceeded {
        /// 事件元数据
        metadata: EventMetadata,
        /// 预算类型
        budget_type: String,
        /// 当前消耗值
        current: u64,
        /// 预算上限
        limit: u64,
    },

    // ============================================================
    // L4 Security → L9 Quest:沙箱违规
    // ============================================================
    /// 沙箱检测到违规,Quest 据此中止或告警
    SandboxViolation {
        /// 事件元数据
        metadata: EventMetadata,
        /// 违规类型
        violation_type: String,
        /// 违规详情
        detail: String,
    },

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
    // 只能走 Event Bus)。4 个变体均为 Normal 级别,追加在枚举末尾
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
    GatherTimedOut {
        /// 事件元数据
        metadata: EventMetadata,
        /// 全局 deadline 阈值(毫秒),即 `GqepConfig::gather_deadline_ms`
        deadline_ms: u64,
        /// 触发超时时实际已运行时间(毫秒)
        elapsed_ms: u64,
        /// 本次 gather 的总操作数
        total: u32,
        /// 被放弃(未完成)的操作数
        abandoned: u32,
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

    // ============================================================
    // Week 5 扩展(SubTask 37.1):Parliament/Security/Budget 跨层通信事件
    //
    // WHY:Week 5 新增 Parliament(L8)、ASA(L4)、AHIRT(L8)、DECB(L3)等
    // 组件,它们通过 EventBus 发布状态变更,符合 §2.2 依赖铁律(跨层通信
    // 只能走 Event Bus)。8 个新变体中,SkepticVeto 与 RedTeamAudit 为
    // Critical(安全/否决必须保证投递),其余 6 个为 Normal,追加在枚举
    // 末尾以保持向后兼容(不修改现有变体的字段或顺序)。
    // ============================================================
    /// 议会辩论开始 — L8 Parliament 内部状态变更
    ///
    /// WHY:Parliament 就提案发起辩论,发布此事件通知内部议员角色
    /// 准备投票。同层通信,Normal 级别(辩论开始本身不致命,丢失仅
    /// 导致本次辩论跳过,可由超时机制兜底)。
    DebateStarted {
        /// 事件元数据
        metadata: EventMetadata,
        /// Quest ID
        quest_id: String,
        /// 提案 ID
        proposal_id: String,
        /// 参与辩论的议员数量
        participant_count: u8,
    },

    /// Skeptic 行使否决权 `[Critical]` — L8 Parliament → L4 Security
    ///
    /// WHY:Skeptic 议员检测到高风险操作时行使否决权,必须保证投递到
    /// SecCore 以冻结对应能力。若丢失,Skeptic 否决形同虚设,高风险
    /// 操作将继续执行,违反架构红线"所有外部调用经 SecCore 沙箱"。
    SkepticVeto {
        /// 事件元数据
        metadata: EventMetadata,
        /// Quest ID
        quest_id: String,
        /// 否决原因(如 "unsafe shell injection detected")
        veto_reason: String,
        /// 被冻结的能力 ID 列表
        frozen_capabilities: Vec<String>,
    },

    /// Skeptic 否决权被人工覆盖 `[Critical]` — L8 Parliament → L4 Security/审计
    ///
    /// WHY Critical:Skeptic 否决是红队安全防线,覆盖否决是高风险操作,
    /// 必须保证投递到 SecCore 与审计系统。丢失将导致覆盖行为无审计记录,
    /// 违反"所有安全相关操作可追溯"原则。此事件与 SkepticVeto 互补:
    /// SkepticVeto 记录否决,VetoOverridden 记录覆盖,两者均不可丢弃。
    ///
    /// # 触发条件
    /// 由 `Parliament::deliberate_with_override()` 发布:
    /// 当 Skeptic 检测到恶意意图但操作方提供了 `VetoOverrideTicket` 时,
    /// 系统仍发布 SkepticVeto 事件(保留完整否决记录),随后发布此事件
    /// 标记覆盖行为,提案继续进入正常辩论流程。
    VetoOverridden {
        /// 事件元数据
        metadata: EventMetadata,
        /// Quest ID
        quest_id: String,
        /// 被覆盖否决的提案 ID
        proposal_id: String,
        /// 原始否决原因(Skeptic 检测到的恶意意图描述)
        veto_reason: String,
        /// 覆盖原因(操作方提供的覆盖理由)
        override_reason: String,
        /// 授权操作方标识(如 "admin:alice" 或 "system:auto-review")
        override_by: String,
    },

    /// AHIRT 红队审计结果 `[Critical]` — L8 Parliament → L4 Security
    ///
    /// WHY:AHIRT 红队探测发现安全漏洞时必须保证投递到 SecCore 进行
    /// 补救。若丢失,已知漏洞将被忽略,违反架构红线"所有外部调用经
    /// SecCore 沙箱 + Decay 衰减"。detection_rate > 0 即代表存在
    /// 可利用漏洞,消费者必须处理。
    RedTeamAudit {
        /// 事件元数据
        metadata: EventMetadata,
        /// 漏洞类型(如 "prompt_injection"/"tool_abuse")
        vulnerability_type: String,
        /// 失败的探测数(触发漏洞的探测)
        failed_probes: u32,
        /// 总探测数
        total_probes: u32,
        /// 检测率 [0.0, 1.0],failed_probes / total_probes
        detection_rate: f32,
        /// 补救建议(如 "add input sanitization")
        remediation_suggestion: String,
    },

    /// DECB 预算档位调整 — L3 Storage → L8 Parliament/L9 Quest
    ///
    /// WHY:DECB 根据消耗动态切换预算档位(如 High/Medium/Low),
    /// 发布此事件通知 Parliament 与 Quest 调整执行策略。与
    /// `BudgetExceeded` 不同:这是档位切换通知(预防性),不是
    /// 超限告警(惩罚性)。Normal 级别,丢失仅导致本次策略未及时
    /// 调整,可由下次周期补偿。
    BudgetAdjusted {
        /// 事件元数据
        metadata: EventMetadata,
        /// Quest ID
        quest_id: String,
        /// 旧档位(如 "High")
        old_tier: String,
        /// 新档位(如 "Medium")
        new_tier: String,
        /// 新档位预算系数 [0.0, +∞),1.0 为基准
        coefficient: f32,
        /// 调整原因(如 "consumption rate > 0.8")
        reason: String,
    },

    /// ASA 安全干预动作 — L4 Security → L7 Execution
    ///
    /// WHY:ASA 对操作进行安全评分并执行干预(Allow/Warn/Block),
    /// 发布此事件通知 Execution 层采取对应动作。
    ///
    /// P1-W2.1.4 修复(2026-07-23):severity() 统一返回 Critical,
    /// 对齐 spec.md L186 红线(AsaIntervention 是 6 个 Critical 事件之一)
    /// 与 §6.2 红线(Critical 安全事件用 mpsc 确保送达)。
    /// 历史设计曾返回 Normal(认为 severity() 不应依赖运行时值 action),
    /// 但 W1.2 TDD 测试暴露 spec/code 偏差,故统一提升为 Critical。
    /// 保守策略:所有 ASA 干预(含 Allow/Warn)走 Critical 通道,
    /// Allow/Warn 低频不会产生大量 Critical 事件。
    AsaIntervention {
        /// 事件元数据
        metadata: EventMetadata,
        /// 被干预的操作 ID
        operation_id: String,
        /// 干预动作(Allow/Warn/Block)
        action: String,
        /// 安全评分 [0.0, 1.0],越高越安全
        safety_score: f32,
        /// Block 时的阻断原因(仅 action="Block" 时填充)
        block_reason: Option<String>,
        /// 替代操作建议(可选,如 "use sandboxed tool X")
        alternative_suggestion: Option<String>,
    },

    /// AHIRT 探测批次完成 — L8 Parliament 内部指标
    ///
    /// WHY:AHIRT 完成一个批次的红队探测后发布统计,供 Parliament
    /// 评估当前安全态势。Normal 级别,丢失仅导致本次统计缺失,
    /// 可由下次批次补偿。
    AhirtProbeCompleted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 探测类型(如 "prompt_injection"/"tool_abuse")
        probe_type: String,
        /// 总探测数
        total: u32,
        /// 通过(未触发漏洞)的探测数
        passed: u32,
        /// 失败(触发漏洞)的探测数
        failed: u32,
        /// 检测率 [0.0, 1.0],failed / total
        detection_rate: f32,
    },

    /// 议会角色注册 — L8 Parliament 内部状态变更
    ///
    /// WHY:Parliament 启动时注册议员角色(如 Visionary/Skeptic/
    /// Pragmatist),发布此事件通知内部组件建立投票权重表。
    /// Normal 级别,丢失仅导致本次注册未记录,可由重试补偿。
    RoleRegistered {
        /// 事件元数据
        metadata: EventMetadata,
        /// 角色 ID(如 "visionary-01")
        role_id: String,
        /// 角色名称(如 "Visionary")
        role_name: String,
        /// 投票权重 [0.0, 1.0],所有角色权重之和应为 1.0
        voting_weight: f32,
    },

    /// 预算消耗统计上报 — L8 Parliament(同层内部统计,无跨层消费)
    ///
    /// WHY:DECB 周期性上报预算消耗统计,供 Parliament 评估是否
    /// 需要触发档位调整或终止 Quest。Normal 级别,丢失仅导致本次
    /// 统计缺失,可由下次周期补偿。
    BudgetStatsReported {
        /// 事件元数据
        metadata: EventMetadata,
        /// 总消耗量(单位由预算类型决定,如 token/字节)
        total_consumption: f64,
        /// 剩余预算
        remaining_budget: f64,
        /// 利用率 [0.0, 1.0],total_consumption / (total_consumption + remaining_budget)
        utilization_rate: f32,
    },

    /// 预算指标更新 — L9 Quest(efficiency-monitor)→ L10 Interface(P1.2 实时数据驱动面板)
    ///
    /// WHY:结构化预算指标,供 TUI Budget 面板直接消费,避免面板侧
    /// 从 BudgetStatsReported / BudgetAdjusted / BudgetExceeded 等多个
    /// 事件拼合。Normal 级别,丢失可由下次周期补偿。
    /// 注:此变体属于 P1.2 实时数据面板契约,非 M4 双向控制新增。
    BudgetMetricsUpdated {
        /// 事件元数据
        metadata: EventMetadata,
        /// 结构化预算指标
        metrics: BudgetMetricsPayload,
    },

    // ============================================================
    // Week 6 扩展:NMC 多模态编码完成事件
    //
    // WHY:nmc-encoder(L2 Memory)完成多模态感知编码后,通过 EventBus
    // 通知 L9 Quest Engine 据此分解任务、SSRA 据此调整融合模板。
    // 符合 §2.2 依赖铁律(跨层通信只能走 Event Bus)。Normal 级别,
    // 丢失仅导致本次编码未通知下游,可由下一次编码补偿。
    // ============================================================
    /// NMC 多模态编码完成 — L2 Memory → L9 Quest
    ///
    /// WHY:Quest Engine 据此分解任务;SSRA 据此调整融合模板。
    /// 携带 modality 与 content_hash 供下游定位编码结果,
    /// clv_dimension 始终为 512(CLV::DIMENSION),消费者可据此校验。
    NmcEncoded {
        /// 事件元数据
        metadata: EventMetadata,
        /// 编码模态(Modality::as_str(),如 "Text"/"Image")
        modality: String,
        /// 内容哈希(SHA256 hex),下游据此去重或检索
        content_hash: String,
        /// CLV 维度(始终为 512,与 CLV::DIMENSION 对齐)
        clv_dimension: usize,
    },

    /// CHTC 接收到 IDE 工具调用 — L10 Interface → L6 Router/L7 Execution
    ///
    /// WHY:下层路由组件订阅此事件做实际工具调用;
    /// CHTC 不直接调用下层,通过 EventBus 解耦(架构铁律 §2.2)
    ChtcToolCallReceived {
        /// 事件元数据
        metadata: EventMetadata,
        /// 调用唯一标识(UUIDv7,与 UnifiedToolCall.call_id 一致)
        call_id: String,
        /// 工具标识(如 VSCode 的 command)
        tool_id: String,
        /// IDE 来源标识(IdeSource::as_str())
        ide_source: String,
        /// 参数 SHA256 哈希,消费者据此去重或拉取具体参数
        parameters_hash: String,
    },

    // ============================================================
    // Week 6 扩展:SSRA 融合完成事件
    //
    // WHY:SSRA(L7 Execution)完成黏液式快速适配融合后,需通知
    // GSOE(L5 Knowledge)作为进化信号、Parliament(L8)评估适配效果。
    // 符合 §2.2 依赖铁律(跨层通信只能走 Event Bus)。Normal 级别,
    // 丢失仅导致本次进化信号缺失,可由下次融合补偿。
    // ============================================================
    /// SSRA 融合完成 — L7 Execution → L5 Knowledge / L8 Parliament
    ///
    /// WHY:GSOE 订阅此事件作为进化信号;Parliament 据此评估适配效果。
    /// 携带融合延迟与置信度,供订阅者决定是否触发能力调整。
    SsraFusionCompleted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 关联的 Quest ID
        quest_id: String,
        /// 融合产出的模板 ID(UUIDv7)
        fused_template_id: String,
        /// 融合延迟(毫秒)
        latency_ms: u64,
        /// 融合置信度 [0.0, 1.0]
        confidence: f32,
    },

    /// GSOE 策略进化完成 — L5 Knowledge → L8 Parliament/L7 Execution
    ///
    /// WHY:Parliament 据此调整审议权重;SSRA 据此更新融合模板。
    /// 携带新策略参数与改进幅度,供订阅者决定是否调整自身行为。
    /// Normal 级别,丢失仅导致本次进化未通知下游,可由下次进化补偿。
    GsoePolicyUpdated {
        /// 事件元数据
        metadata: EventMetadata,
        /// 进化世代数
        generation: u64,
        /// 相对上一代的改进幅度(新平均适应度 - 旧平均适应度)
        improvement: f32,
        /// 新策略变异率
        new_mutation_rate: f32,
        /// 新策略选择压力
        new_selection_pressure: f32,
    },

    // ============================================================
    // Week 6 扩展:LSCT 层级切换事件
    //
    // WHY:LSCT(L3 Storage)完成任务负载画像计算与升降温决策后,
    // 发布此事件通知 CMT(同层 L3)执行实际数据迁移。LSCT 是策略层,
    // 不直接操作 CMT 存储,仅发布事件让 CMT 订阅执行(§2.2 依赖铁律:
    // 同层互引 + 跨层走 EventBus)。Normal 级别,丢失仅导致本次迁移未执行,
    // 可由下次 tick 补偿。
    // ============================================================
    /// LSCT 层级切换 — L3 Storage(LSCT)→ L3 Storage(CMT)
    ///
    /// WHY:CMT 订阅此事件执行实际数据迁移;Parliament 可据此追踪能力层级变化。
    /// 携带 capability_id 与 from/to 层级,供订阅者精确定位迁移目标。
    LsctTierSwitched {
        /// 事件元数据
        metadata: EventMetadata,
        /// 被切换层级的能力 ID
        capability_id: String,
        /// 源层级(Tier::as_str(),如 "Warm")
        from_tier: String,
        /// 目标层级(Tier::as_str(),如 "Hot")
        to_tier: String,
        /// 切换原因(如 "compile task high intensity → promote")
        reason: String,
    },

    /// MCP Mesh 事务完成 — L10 Interface(mcp-mesh)→ 任意订阅者
    ///
    /// WHY:MCP 量子网格事务完成后广播,CSN 据此判断能力是否不可达;
    /// efficiency-monitor 据此统计事务成功率;Lead Architect 据此追踪分布式事务健康度。
    McpMeshTransactionCompleted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 事务 ID
        transaction_id: String,
        /// 参与服务器数量
        participant_count: u32,
        /// 事务耗时(毫秒)
        latency_ms: u64,
        /// 是否成功
        success: bool,
        /// 关联的能力 ID(可选)— 用于 csn-substitutor 精准推进降级链
        ///
        /// WHY Option:Task 0.7 v2.9.0-omega 引入。旧调用方(mcp-mesh 主流程)
        /// 不一定知道触发事务的能力 ID,默认 None;Task 0.5 csn-substitutor
        /// 重设计后将填充此字段,使降级链只推进相关条目而非全部(避免误伤)。
        capability_id: Option<String>,
    },

    /// CSN 替代触发 — L10 Interface(csn-substitutor)→ 任意订阅者
    ///
    /// WHY:能力不可达时 CSN 自动触发替代,降级链进入下一级;
    /// efficiency-monitor 据此统计替代触发率;GSOE 据此作为进化信号。
    CsnSubstitutionTriggered {
        /// 事件元数据
        metadata: EventMetadata,
        /// 原能力 ID
        original_capability_id: String,
        /// 替代候选 ID
        substitute_id: String,
        /// 余弦相似度得分([-1.0, 1.0])
        similarity_score: f32,
        /// 当前降级层级(从 0 开始)
        degradation_level: u32,
    },

    /// SESA 激活完成 — L6 Router(sesa-router)→ 任意订阅者
    ///
    /// WHY:子专家稀疏激活完成后广播;KVBSR/FaaE 据此协调路由;
    /// efficiency-monitor 据此监控稀疏度是否 < 40%。
    SesaActivationCompleted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 总专家数
        total_experts: u32,
        /// 激活专家数
        active_experts: u32,
        /// 实测稀疏度(active_experts / total_experts,[0.0, 1.0])
        sparsity_ratio: f32,
        /// 激活耗时(微秒)
        latency_us: u64,
    },

    /// 效率告警触发 — L9 Quest(efficiency-monitor)→ 任意订阅者
    ///
    /// WHY:监控告警触发后广播;Lead Architect 据此响应 Critical 事件;
    /// Parliament 据此决策是否启动 ASA 干预;AHIRT 据此调整红队探测频率。
    EfficiencyAlertTriggered {
        /// 事件元数据
        metadata: EventMetadata,
        /// 触发告警的规则 ID
        rule_id: String,
        /// 告警指标名
        metric_name: String,
        /// 触发值
        triggered_value: f64,
        /// 阈值
        threshold: f64,
    },

    // ============================================================
    // M4 扩展:TUI 双向控制请求事件
    //
    // WHY:chimera-tui(L10 Interface)作为控制面板,需通过 EventBus
    // 向下游发布控制请求,而非直接修改上游状态。所有变体均为请求语义,
    // 对应上游消费后产生状态变更事件。字段加 #[serde(default)] 保证
    // 未来字段扩展或旧数据反序列化兼容。
    // ============================================================
    /// Quest 暂停请求 — L10 Interface → L9 Quest
    QuestPauseRequested {
        /// 事件元数据
        metadata: EventMetadata,
        /// 目标 Quest ID
        #[serde(default)]
        quest_id: String,
        /// 请求者标识
        #[serde(default)]
        requested_by: String,
    },

    /// Quest 恢复请求 — L10 Interface → L9 Quest
    QuestResumeRequested {
        /// 事件元数据
        metadata: EventMetadata,
        /// 目标 Quest ID
        #[serde(default)]
        quest_id: String,
        /// 请求者标识
        #[serde(default)]
        requested_by: String,
    },

    /// 投票请求 — L10 Interface → L8 Parliament
    VoteCastRequested {
        /// 事件元数据
        metadata: EventMetadata,
        /// 目标提案 ID
        #[serde(default)]
        proposal_id: String,
        /// 投票者标识
        #[serde(default)]
        voter: String,
        /// 投票值
        vote: VoteValue,
    },

    /// 状态刷新请求 — L10 Interface → 任意订阅者
    RefreshStateRequested {
        /// 事件元数据
        metadata: EventMetadata,
        /// 请求者标识
        #[serde(default)]
        requested_by: String,
    },

    /// Quest 已暂停 — L9 Quest → L10 Interface
    ///
    /// WHY:quest-engine 消费 QuestPauseRequested 后发布状态变更事件,
    /// 供 TUI 数据管道感知并反馈给操作员,完成双向控制闭环。
    QuestPaused {
        /// 事件元数据
        metadata: EventMetadata,
        /// 已暂停的 Quest ID
        quest_id: String,
        /// 请求者标识
        requested_by: String,
    },

    /// Quest 已恢复 — L9 Quest → L10 Interface
    QuestResumed {
        /// 事件元数据
        metadata: EventMetadata,
        /// 已恢复的 Quest ID
        quest_id: String,
        /// 请求者标识
        requested_by: String,
    },

    // ============================================================
    // M4 扩展(续):Quest 取消与优先级控制双向事件
    //
    // WHY 新增(Task 1):补齐 TUI 双向控制闭环 — 除暂停/恢复外,操作员
    // 还需取消 Quest 与调整优先级。沿用 M4 既有模式:请求语义变体
    // (L10→L9)与状态变更反馈变体(L9→L10)成对出现,字段加
    // #[serde(default)] 保证未来扩展或旧数据反序列化兼容。
    // severity 统一为 Info:控制事件不阻断系统,不触发 mpsc 旁路投递。
    // ============================================================
    /// Quest 取消请求 — L10 Interface → L9 Quest
    QuestCancelRequested {
        /// 事件元数据
        metadata: EventMetadata,
        /// 目标 Quest ID
        #[serde(default)]
        quest_id: String,
        /// 请求者标识
        #[serde(default)]
        requested_by: String,
    },

    /// Quest 已取消 — L9 Quest → L10 Interface
    ///
    /// WHY:quest-engine 消费 QuestCancelRequested 后发布状态变更事件,
    /// 供 TUI 数据管道感知并反馈给操作员,完成取消控制闭环。
    QuestCancelled {
        /// 事件元数据
        metadata: EventMetadata,
        /// 已取消的 Quest ID
        #[serde(default)]
        quest_id: String,
        /// 请求者标识
        #[serde(default)]
        requested_by: String,
    },

    /// Quest 优先级变更请求 — L10 Interface → L9 Quest
    QuestPriorityChanged {
        /// 事件元数据
        metadata: EventMetadata,
        /// 目标 Quest ID
        #[serde(default)]
        quest_id: String,
        /// 新优先级(0-255,数值越大优先级越高)
        #[serde(default)]
        new_priority: u8,
        /// 请求者标识
        #[serde(default)]
        requested_by: String,
    },

    /// Quest 优先级已调整 — L9 Quest → L10 Interface
    ///
    /// WHY:quest-engine 消费 QuestPriorityChanged 后发布状态变更事件,
    /// 供 TUI 数据管道刷新 Quest 列表排序,完成优先级控制闭环。
    QuestPriorityAdjusted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 已调整的 Quest ID
        #[serde(default)]
        quest_id: String,
        /// 生效后的新优先级(0-255)
        #[serde(default)]
        new_priority: u8,
        /// 请求者标识
        #[serde(default)]
        requested_by: String,
    },

    /// 衰减指标报告 — L4 decay-engine 发布,L10 TUI Decay 面板消费
    ///
    /// WHY 新增(P2.1 TUI v1.7-omega):TUI 无法直接依赖 L4 decay-engine,
    /// 通过 event-bus 传递衰减系数与最近事件,供 Decay 面板绘制 sparkline。
    ///
    /// # P2-11 扩展(2026-07-28)
    ///
    /// 新增 `fallback_count_delta` 字段,携带本周期内 `DecayLearnerHolder`
    /// 触发 fallback 的次数(异常回退层 + 熔断入口层)。用于监控 learner
    /// 健康度:delta 持续 > 0 表明 learner 不稳定,需排查 omega-learner
    /// 或 PoisonError 根因。`#[serde(default)]` 保持向后兼容(旧消费者
    /// 反序列化时默认为 0)。
    DecayMetricsReported {
        /// 事件元数据
        metadata: EventMetadata,
        /// 当前衰减系数 [0.0, 1.0],1.0 表示无衰减
        coefficient: f32,
        /// 本周期内触发衰减的最近事件摘要(最多 N 条,由发布者截断)
        recent_events: Vec<String>,
        /// 本衰减周期开始时间
        cycle_start: DateTime<Utc>,
        /// P2-11: 本周期 fallback 触发次数(异常回退层 + 熔断入口层)
        ///
        /// 由发布者通过 `DecayLearnerHolder::take_fallback_count()` 获取。
        /// 向后兼容:`#[serde(default)]` 确保旧格式数据反序列化为 0。
        #[serde(default)]
        fallback_count_delta: u64,
    },

    /// 路由器统计报告 — L9 efficiency-monitor 聚合发布,L10 TUI Router 面板消费
    ///
    /// WHY 新增(P2.3 TUI v1.7-omega):三路由器(KVBSR/SESA/FaaE)的命中率
    /// 与延迟分位数统一通过此事件传递,避免 TUI 分别订阅三个路由器事件。
    RouterStatsReported {
        /// 事件元数据
        metadata: EventMetadata,
        /// KVBSR 路由器统计
        kvbsr_stats: RouterStatsPayload,
        /// SESA 路由器统计
        sesa_stats: RouterStatsPayload,
        /// FaaE 路由器统计
        faae_stats: RouterStatsPayload,
    },

    /// MCP 节点心跳 — L10 mcp-mesh 发布,L10 TUI McpNodes 面板消费
    ///
    /// WHY 新增(P2.4 TUI v1.7-omega):MCP Mesh 节点状态通过事件流推送到 TUI,
    /// 供操作员实时观察节点健康与吞吐量。
    McpNodeHeartbeat {
        /// 事件元数据
        metadata: EventMetadata,
        /// 节点 ID
        node_id: String,
        /// 节点状态字符串(如 "online"/"degraded"/"offline")
        status: String,
        /// 节点吞吐量(每秒事务数)
        throughput: u64,
        /// 最近一次心跳时间
        last_seen: DateTime<Utc>,
    },

    /// CHTC 适配器状态 — L10 chtc-bridge 发布,L10 TUI Chtc 面板消费
    ///
    /// WHY 新增(P2.5 TUI v1.7-omega):5 IDE 适配器的兼容性评分与请求计数
    /// 通过事件流推送到 TUI,供操作员观察跨平台工具兼容性。
    ChtcAdapterStatus {
        /// 事件元数据
        metadata: EventMetadata,
        /// 适配器 ID
        adapter_id: String,
        /// 适配器类型(如 "vscode"/"jetbrains"/"vim"/"emacs"/"cli")
        adapter_type: String,
        /// 兼容性评分 [0, 100]
        compatibility_score: u8,
        /// 最近请求(请求标识, 次数)列表
        recent_requests: Vec<(String, u32)>,
        /// 是否在线
        is_online: bool,
    },

    /// CLV 快照报告 — L2 Memory → L10 Interface
    ///
    /// WHY 新增(TUI v1.8-omega):chimera-tui 的 ClvVector 面板需要展示
    /// CLV 512 维向量的运行时摘要,但不能携带完整向量(性能负担)。
    /// NMC 编码器在完成编码后发布此事件,携带 ClvSummary 摘要
    /// (8 分块均值 + L2 范数 + Top-8 维度索引)。
    /// Normal 级别:丢失仅导致本次摘要未展示,可由下次编码补偿。
    ClvSnapshotReported {
        /// 事件元数据
        metadata: EventMetadata,
        /// 编码模态(与 NmcEncoded 一致,如 "Text"/"Image")
        modality: String,
        /// 内容哈希(与 NmcEncoded 一致,供去重或检索)
        content_hash: String,
        /// CLV 摘要(8 分块均值 + L2 范数 + Top-8 维度索引)
        clv_summary: ClvSummary,
    },

    // ============================================================
    // CHIMERA-MAS Agent 协作事件(ADR-026,Task 4)
    //
    // WHY:7 个新变体覆盖 Agent 间协作的全部通信场景:任务委派/完成/失败、
    // 咨询请求/回复、心跳、上下文溢出。所有变体均携带 metadata 字段
    // (与既有变体保持一致),使 metadata() 方法能统一返回 &EventMetadata。
    // severity 分配:仅 AgentTaskFailed 为 Critical(任务失败可能影响
    // Quest 完整性),其余 6 个为 Normal(severity() 显式列出,M1 起无通配符)。
    // ============================================================
    /// Agent 任务委派 — L9 chimera-mas 内部通信
    ///
    /// WHY:RootOrchestrator 将子任务委派给子 Agent 时发布此事件。
    /// 携带 deadline 与 priority 供调度器排序。Normal 级别,丢失仅
    /// 导致本次委派未记录,可由 AgentTaskCompleted/Failed 补偿。
    AgentTaskDelegated {
        /// 事件元数据
        metadata: EventMetadata,
        /// 委派方 Agent ID
        from: String,
        /// 被委派方 Agent ID
        to: String,
        /// 任务 ID
        task_id: String,
        /// 截止时间
        deadline: DateTime<Utc>,
        /// 任务优先级
        priority: TaskPriority,
    },

    /// Agent 任务完成 — L9 chimera-mas 内部通信
    ///
    /// WHY:子 Agent 完成任务后发布此事件,RootOrchestrator 据此
    /// 聚集结果并推进 Quest。Normal 级别,丢失仅导致本次完成未记录,
    /// 可由 AgentHeartbeat 补偿。
    AgentTaskCompleted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 完成方 Agent ID
        from: String,
        /// 委托方 Agent ID
        to: String,
        /// 任务 ID
        task_id: String,
        /// 结果摘要
        result_summary: String,
    },

    /// Agent 任务失败 `[Critical]` — L9 chimera-mas 内部通信
    ///
    /// WHY Critical:任务失败可能影响 Quest 完整性,必须保证投递到
    /// SecCore 与 Parliament 进行补救决策。若标为 Normal,在背压场景下
    /// 可能被丢弃,导致失败无人响应、Quest 持续等待已死 Agent 的结果。
    AgentTaskFailed {
        /// 事件元数据
        metadata: EventMetadata,
        /// 失败方 Agent ID
        from: String,
        /// 委托方 Agent ID
        to: String,
        /// 任务 ID
        task_id: String,
        /// 错误信息
        error: String,
        /// 已重试次数
        retry_count: u32,
    },

    /// Agent 咨询请求 — L9 chimera-mas 内部通信
    ///
    /// WHY:Agent 遇到不确定问题时向其他 Agent 发起咨询。Normal 级别,
    /// 丢失仅导致本次咨询未送达,可由超时重试补偿。
    AgentConsultRequested {
        /// 事件元数据
        metadata: EventMetadata,
        /// 咨询方 Agent ID
        from: String,
        /// 被咨询方 Agent ID
        to: String,
        /// 咨询问题
        question: String,
        /// 咨询上下文
        context: String,
        /// 紧急度
        urgency: ConsultUrgency,
    },

    /// Agent 咨询回复 — L9 chimera-mas 内部通信
    ///
    /// WHY:被咨询 Agent 返回答案。Normal 级别,丢失仅导致本次回复
    /// 未送达,可由超时重试补偿。
    AgentConsultResponded {
        /// 事件元数据
        metadata: EventMetadata,
        /// 回复方 Agent ID
        from: String,
        /// 咨询方 Agent ID
        to: String,
        /// 回答内容
        answer: String,
        /// 参考资料链接列表
        references: Vec<String>,
    },

    /// Agent 心跳 — L9 chimera-mas 内部通信
    ///
    /// WHY:Agent 定期发布心跳报告状态与资源占用。Normal 级别,
    /// 丢失仅导致本次心跳未记录,可由下次心跳补偿。
    AgentHeartbeat {
        /// 事件元数据
        metadata: EventMetadata,
        /// Agent ID
        from: String,
        /// Agent 运行时状态
        status: AgentStatus,
        /// 当前任务 ID(空闲时为 None)
        current_task: Option<String>,
        /// Token 使用量
        token_usage: u64,
        /// 内存使用量(MB)
        memory_usage_mb: u64,
    },

    /// Agent 上下文溢出 — L9 chimera-mas 内部通信
    ///
    /// WHY:Agent 的上下文 token 数达到上限。severity() 返回 Normal
    /// (同步函数不依赖运行时值),但语义上是告警,发布者应通过
    /// Critical 通道发送以确保投递(类似 AsaIntervention Block 场景)。
    AgentContextOverflow {
        /// 事件元数据
        metadata: EventMetadata,
        /// Agent ID
        agent_id: String,
        /// 当前 token 数
        current_tokens: usize,
        /// 最大 token 数
        max_tokens: usize,
    },

    // ============================================================
    // TUI 交互式动作协议(ADR-029,v3.1)
    //
    // WHY:统一 Action 协议覆盖 TUI 内全部可交互功能,三入口(Chat 斜杠命令/
    // 命令面板/面板上下文动作)共享同一契约。TUI(L10)只发起请求、接收反馈,
    // Agent/域编排在 chimera-cli(bin,可依赖下层),经 L1 EventBus 双向通信,
    // 不违反 L10 依赖铁律。所有变体携带 metadata,severity 均为 Info/Normal
    // (非 Critical:不占用 mpsc 旁路,该旁路仅留给稀有安全告警事件)。
    // ============================================================
    /// TUI 动作请求 — 三入口统一派发点(TUI → 编排层)
    ///
    /// WHY payload 为 JSON 字符串:event-bus(L1)不感知具体 Action 语义,
    /// 各 Action 的结构化参数由 chimera-tui 的 ActionDescriptor 定义 schema
    /// 并序列化,保持 L1 与 TUI 动作语义解耦。
    TuiActionRequested {
        /// 事件元数据
        metadata: EventMetadata,
        /// 请求唯一标识 — 由发起方(TUI)生成,终态回执原样回传
        ///
        /// WHY 独立字段而非复用 `action_id`:同 `action_id` 可并发/连续发起多次
        /// 请求(如连点两次 `quest.cancel`),仅凭 `action_id` 无法把
        /// `TuiActionCompleted/Failed` 归属到具体那一次请求,导致发起方的超时
        /// 计时被错误清除(表现为"失败却显示成功"或"超时永不提示")。
        /// 本字段是请求-回执配对的主键,`TuiActionCompleted`/`TuiActionFailed`
        /// 必须原样回传同一值。
        ///
        /// 约定:格式由发起方自定(当前 TUI 用 `tui-{单调序号}`);空串表示
        /// 未知/未启用关联,消费方须按"无关联"处理而非视为同一请求。
        ///
        /// WHY `#[serde(default)]`:事件存在 JSON/MessagePack 持久化与回放路径
        /// (fuzz_targets/event_serialize),旧格式不含本字段;缺省为空串恰好
        /// 落入"无关联"语义,旧数据反序列化不失败。
        #[serde(default)]
        request_id: String,
        /// 动作标识(如 "quest.pause"/"export.run"/"agent.chat")
        action_id: String,
        /// 动作参数(JSON 编码,schema 由 ActionDescriptor 定义)
        payload: String,
        /// 触发入口(Chat/Palette/Panel),用于审计与 UI 反馈定位
        source: ActionSource,
    },

    /// TUI 动作进度 — 流式反馈(编排层 → TUI)
    ///
    /// WHY Normal 级别:进度增量为高频事件,走 broadcast 通道;
    /// `TuiChatResponseChunk` 是本变体面向 token 流的高频特化。
    ///
    /// WHY 本变体**不携带** `request_id`(与 Requested/Completed/Failed 不对称):
    /// 进度是幂等的流式展示,不构成回执,当前无任何消费方按请求归属进度;
    /// 为其引入主键只会扩大协议变更面而无实际收益。若未来需要按请求聚合进度,
    /// 再以独立 ADR 补充。
    TuiActionProgressed {
        /// 事件元数据
        metadata: EventMetadata,
        /// 关联的动作标识
        action_id: String,
        /// 增量内容(语义由 action_id 决定,如进度文本/百分比 JSON)
        delta: String,
    },

    /// TUI 动作完成 — 终态反馈(编排层 → TUI)
    TuiActionCompleted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 回执归属的请求标识 — 原样回传 `TuiActionRequested.request_id`
        ///
        /// WHY `#[serde(default)]`:同 `TuiActionRequested`,旧格式回放兼容;
        /// 空串按"无归属"处理,消费方不清除任何超时计时。
        #[serde(default)]
        request_id: String,
        /// 关联的动作标识
        action_id: String,
        /// 结果摘要(JSON 编码或纯文本)
        result: String,
    },

    /// TUI 动作失败 — 错误反馈(编排层 → TUI)
    ///
    /// WHY Info 而非 Critical:动作失败是操作员可感知的交互结果,
    /// 由 UI 呈现给用户重试,不属于必须旁路投递的系统安全事件。
    TuiActionFailed {
        /// 事件元数据
        metadata: EventMetadata,
        /// 回执归属的请求标识 — 原样回传 `TuiActionRequested.request_id`
        ///
        /// WHY `#[serde(default)]`:同 `TuiActionCompleted`,旧格式回放兼容。
        #[serde(default)]
        request_id: String,
        /// 关联的动作标识
        action_id: String,
        /// 错误信息(面向用户的可读描述)
        error: String,
    },

    /// TUI 对话提交 — `agent.chat` 动作的语义特化(TUI → 编排层)
    ///
    /// WHY 保留独立变体而非全走 TuiActionRequested:对话是最高频交互,
    /// 独立变体让编排器可零成本模式匹配路由到 QueryLoop,且携带 session_id
    /// 支持多会话。语义等价于 `TuiActionRequested{ action_id:"agent.chat" }`。
    TuiChatSubmitted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 会话标识(支持多轮对话上下文关联)
        session_id: String,
        /// 用户查询原文
        query: String,
        /// 若为斜杠命令,携带命令名(如 "plan"/"clear");纯对话为 None
        slash_command: Option<String>,
    },

    /// TUI 对话流式分块 — token 增量(编排层 → TUI)
    ///
    /// WHY Normal 级别且禁止 Critical:token 流为高频事件,若走 mpsc 旁路
    /// 会冲垮仅为稀有安全告警保留的点对点通道。走 broadcast + 低延迟 drain,
    /// TUI 侧只标记光标行 dirty 实现增量渲染。
    TuiChatResponseChunk {
        /// 事件元数据
        metadata: EventMetadata,
        /// 会话标识
        session_id: String,
        /// 本次 token 增量文本
        delta: String,
        /// 光标行提示(供 TUI 定位增量渲染的脏行,减少全量重绘)
        cursor_hint: u32,
    },

    /// TUI 对话完成 — 本轮回答终态(编排层 → TUI)
    TuiChatCompleted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 会话标识
        session_id: String,
        /// 若本轮触发工具调用,携带工具调用摘要(JSON);否则 None
        tool_use: Option<String>,
    },

    /// TUI 对话状态变更 — 会话状态机(编排层 → TUI)
    ///
    /// WHY Normal 级别:状态指示器更新非关键,丢失可由下次状态事件纠正。
    TuiChatStatusChanged {
        /// 事件元数据
        metadata: EventMetadata,
        /// 会话标识
        session_id: String,
        /// 新状态(Thinking/ToolExecuting/Idle)
        status: ChatStatus,
    },

    /// TUI 会话历史整体替换 — `/compact` 策展回写(FC-2,ADR-081)
    ///
    /// WHY 独立变体:策展压缩(curator 五段分类 + 0-1 背包 + 抽取式摘要)在
    /// 编排器侧完成后,压缩后的会话历史必须回写到唯一所有者 ChatSync(M3b
    /// 单一所有权设计)——本事件是唯一合法的"整史替换"控制信道;逐条
    /// Submitted/Chunk 重放既有双计数歧义又无法表达"删除"。消息以
    /// [`super::payloads::TuiChatMessagePayload`] 字符串角色承载(L1 不感知
    /// L10 枚举),由 ChatSync 负责转换。Normal 级(走 broadcast 即可,历史
    /// 替换非高频且允许 Lagged 时下次 compact 重做)。
    TuiChatHistoryReplaced {
        /// 事件元数据
        metadata: EventMetadata,
        /// 会话标识(与 TuiChatSubmitted 同域,预留多会话)
        session_id: String,
        /// 压缩后的完整会话历史(原序)
        messages: Vec<super::payloads::TuiChatMessagePayload>,
    },

    /// TUI → 编排器协议握手请求(Concord W10 T10.1,ADR-082)
    ///
    /// WHY 独立变体:防 Codex #37536 式版本偏移静默故障——陈旧后端
    /// 存活时新 TUI 静默跑旧能力。TUI 启动(M4 总线注入后)发布本事件,
    /// 编排器应答 `TuiHelloAck`;SEC-4:仅信道建立初期接受一次,
    /// 运行期到达的握手帧丢弃并审计。
    TuiHello {
        /// 事件元数据
        metadata: EventMetadata,
        /// 协议版本(semver 字符串,如 "1.0.0")
        proto: String,
        /// TUI 端版本(workspace 版本)
        tui_version: String,
        /// TUI 声明的能力集标识(如 "orchestrated-commands"/"agent-tree")
        caps: Vec<String>,
    },

    /// 编排器 → TUI 协议握手应答(Concord W10 T10.1,ADR-082)
    ///
    /// WHY Info 级别:握手是一次性信道建立事件,丢失可由 TUI 超时降级
    /// 兜底(未收到 Ack → 按未知兼容处理),无需 Critical 旁路。
    TuiHelloAck {
        /// 事件元数据
        metadata: EventMetadata,
        /// 服务端支持的协议版本
        proto: String,
        /// 兼容级别(Full/Degraded 携降级项/Refused)
        compat: CompatLevel,
        /// 服务端版本
        server_version: String,
    },

    /// R1 影子模式退化检测 — 连续显著退化触发预警（P4-W16.2.2 步骤 5）
    ///
    /// WHY Normal 级别:退化检测是诊断信号,非阻断性事件。丢失仅导致本次
    /// 退化未被记录,可由下一日对比报告补偿。编排器根据 `regression_streak`
    /// 自行决定是否触发回滚（连续 3 天才回滚，ADR-043 决策 4）。
    R1ShadowRegressionDetected {
        /// 事件元数据
        metadata: EventMetadata,
        /// 报告日期（UTC，每日一份对比报告）
        report_date: DateTime<Utc>,
        /// 连续显著退化天数（达到 3 触发回滚）
        regression_streak: u32,
    },

    /// R1 影子模式解冻就绪 — 4 项解冻条件全部满足（P4-W16.2.2 步骤 5）
    ///
    /// WHY Normal 级别:解冻就绪是状态通知,非紧急事件。丢失仅导致本次
    /// 解冻信号未送达,可由下一日报告补偿（解冻需三方评审，非自动生效）。
    R1ShadowPromotionReady {
        /// 事件元数据
        metadata: EventMetadata,
        /// 报告日期（UTC）
        report_date: DateTime<Utc>,
        /// 14 天观察期内的胜率（R1 优于 L3 的天数比例）
        win_rate: f64,
        /// 当前 EWMA 成功率（≥ 0.7 解冻条件 1）
        ewma_level: f32,
    },

    /// R1 影子模式回滚失败 — 回滚操作执行失败（P4-W16.2.2 步骤 5）
    ///
    /// WHY Critical 级别:回滚失败意味着 R1 策略可能仍在生效但已退化,
    /// 必须保证投递到 SecCore 与 Parliament 进行紧急干预。若标为 Normal,
    /// 在背压场景下可能被丢弃,导致退化策略持续生效、Quest 质量下降。
    /// 对齐 §6.2 红线 5（Critical 安全事件用 mpsc 旁路通道）。
    ///
    /// # P2-13 结构化理由记录
    ///
    /// 旧版仅有 `reason: String`(自由文本),P2-13 扩展为结构化记录:
    /// - `trigger_type`:机器可读的触发条件枚举(4 种 + Unknown)
    /// - `triggered_at`:触发时间戳(UTC)
    /// - `details`:底层错误详情(如 CapabilityTokenRegistry 内部错误消息)
    /// - `diagnostic`:诊断上下文快照(EWMA 水平、观察期天数等)
    ///
    /// `reason` 字段保留为人类可读描述,向后兼容。所有新字段带 `#[serde(default)]`,
    /// 确保旧版本序列化的事件能被反序列化(SemVer minor 兼容)。
    R1ShadowRollbackFailed {
        /// 事件元数据
        metadata: EventMetadata,
        /// 回滚失败原因(人类可读描述,向后兼容保留)
        ///
        /// 旧版字段,保留用于日志展示与向后兼容。新增 `trigger_type` 字段
        /// 提供机器可读的结构化分类标签,避免字符串模糊匹配的歧义。
        reason: String,
        /// P2-13: 结构化触发条件类型(ADR-043 决策 4)
        ///
        /// 对应 4 种回滚触发条件 + Unknown 兜底。`#[serde(default)]`
        /// 确保旧版本序列化的事件(无此字段)能被反序列化为 Unknown。
        #[serde(default)]
        trigger_type: RollbackTriggerType,
        /// P2-13: 触发时间戳(UTC)
        ///
        /// 记录回滚失败发生的精确时间,用于审计时间线重建。
        /// None 表示时间戳未知(旧版本事件兼容)。
        #[serde(default)]
        triggered_at: Option<DateTime<Utc>>,
        /// P2-13: 详细错误消息
        ///
        /// 承载回滚操作失败的底层错误详情,如 CapabilityTokenRegistry
        /// 内部错误的具体消息。空字符串表示无详细错误信息。
        #[serde(default)]
        details: String,
        /// P2-13: 诊断上下文(EWMA 水平、观察期天数等)
        ///
        /// 承载回滚失败时的诊断快照,便于专家团队复盘根因。
        /// 默认为全 None 的空上下文(旧版本事件兼容)。
        #[serde(default)]
        diagnostic: RollbackDiagnosticContext,
    },

    /// P5.2.3: Spec 版本注册完成 — L5 Knowledge(gsoe-evolution)→ 任意订阅者
    ///
    /// 通道 B 否决通过后,候选 spec 通过 SpecRegistry::register 纳入谱系,
    /// 同时发布此事件通知下游(parliament / efficiency-monitor / repo-wiki):
    /// - Parliament 据此更新 spec 版本快照
    /// - efficiency-monitor 据此追踪 RHI-CG 进化指标
    /// - repo-wiki 据此记录 spec 版本历史
    ///
    /// WHY Normal 级别:spec 注册是常规进化操作,非阻断性事件。丢失仅导致
    /// 本次注册未通知下游,可由下次注册或主动查询补偿。Critical 路径
    /// (如不可进化面违反)通过 SpecRegistryError 返回值传播,不走事件。
    ///
    /// WHY 不携带完整 spec:HarnessSpec 是 nexus-contracts 类型,event-bus
    /// (L1)不能依赖 nexus-contracts(会破坏分层),且完整 spec 体积较大。
    /// 仅携带 (name, version, parent_version) 标识字段,完整 spec 通过
    /// SpecRegistry::get(name, version) 查询。
    SpecRegistered {
        /// 事件元数据
        metadata: EventMetadata,
        /// spec 名称(如 "quest-parse")
        spec_name: String,
        /// spec 版本号
        spec_version: u32,
        /// 父版本号(None 表示初始版本)
        parent_version: Option<u32>,
        /// 注册来源(如 "rhi-cg-channel-b" / "manual" / "ab-test")
        source: String,
    },

    /// R2 冻结违反 — 冻结期内检测到 R2(GSOE×AutoDPO 约束 RL)路径激活(ADR-042 决策 4)
    ///
    /// WHY Critical 级别:R2 违反等同于安全事件——奖励黑客风险可能立即生效,
    /// 进化策略学会绕过 L3 验证器而非真正改进代码质量(§3.4.5 进化悖论红线)。
    /// 必须保证投递到 SecCore 与 Parliament 进行紧急干预(自动回滚 + 告警广播)。
    /// 对齐 §6.2 红线 5(Critical 安全事件用 mpsc 旁路通道)。
    ///
    /// # 触发场景
    /// - CI 检测:扫描 gsoe-evolution / auto-dpo 源码发现 R2 路径实现
    /// - 运行时检测:`evolve_once()` 入口 `debug_assert!(!cfg!(feature = "r2_path"))` panic
    /// - 审计检测:AsaAuditor 周期性扫描进化路径发现 R2 激活痕迹
    R2FreezeViolation {
        /// 事件元数据
        metadata: EventMetadata,
        /// 违反类型(CiDetection / RuntimeAssertion / AuditScan)
        violation_type: String,
        /// 违反证据(如匹配的源码片段 / panic 信息 / 审计日志)
        evidence: String,
    },

    /// R2 冻结回滚失败 — 自动回滚操作执行失败(ADR-042 决策 4 步骤 1)
    ///
    /// WHY Critical 级别:回滚失败意味着 R2 路径代码可能仍在生效,必须保证
    /// 投递到 SecCore 与 Parliament 进行升级干预(从自动回滚升级为人工介入)。
    /// 若标为 Normal,在背压场景下可能被丢弃,导致 R2 违反持续生效。
    /// 对齐 §6.2 红线 5(Critical 安全事件用 mpsc 旁路通道)。
    R2FreezeRollbackFailed {
        /// 事件元数据
        metadata: EventMetadata,
        /// 回滚失败原因(如 "git revert 冲突" / "cargo build 失败")
        reason: String,
    },

    /// P2-1: 协调成本/推理增益比值报告 — L9 Quest(quest-engine)→ 任意订阅者
    ///
    /// 由 `CoordinationMetricsCollector::record_and_compute` 在 Quest 完成(或周期性
    /// 评估)时发布,携带当前 EWMA 比值快照。订阅者据此:
    /// - **efficiency-monitor**:订阅后若 `is_paradox_risk == true` 则触发
    ///   `EfficiencyAlertTriggered` 告警(推理悖论红线)
    /// - **Parliament**:据此调整 TTG 策略(高比值时降低协调开销,如跳过议会审议)
    /// - **TUI**:实时展示协调成本/推理增益比值趋势
    ///
    /// WHY Normal 级别:这是周期性指标报告,非阻断性事件。推理悖论风险告警
    /// 由 efficiency-monitor 订阅后通过 `EfficiencyAlertTriggered` 事件二次发布,
    /// 不需要走 mpsc 旁路通道(告警语义在订阅者处理,非事件本身)。这遵循
    /// "事件本身是事实陈述,告警是订阅者的解释"的设计原则。
    ///
    /// WHY 携带完整比值字段:虽然事件总线不应承载大量数据,但比值快照仅 7 个
    /// 标量字段(约 80 字节),远小于事件总线的消息上限。完整字段便于订阅者
    /// 直接消费,无需反向查询 quest-engine,降低耦合。
    ///
    /// 对应架构红线:§3.4.5 三重悖论推理悖论红线——"当协调成本超过推理增益时,
    /// 多 Agent 反而不如单 Agent"。此事件是该红线的可观测指标载体。
    CoordinationRatioReported {
        /// 事件元数据
        metadata: EventMetadata,
        /// EWMA 协调成本(毫秒)
        ///
        /// 经过 EWMA 平滑后的协调成本,包含 Event Bus 延迟、TTG 切换延迟、
        /// 议会审议延迟、多 Agent 委托开销的加权和。
        coordination_cost_ms: f64,
        /// EWMA 推理增益 [0.0, 1.0]
        ///
        /// 经过 EWMA 平滑后的推理增益,加权融合任务成功率、PVL 质量分数、
        /// 议会共识质量。
        inference_gain: f32,
        /// 归一化成本指数 [0.0, 1.0]
        ///
        /// `cost_index = min(coordination_cost_ms / cost_baseline_ms, 1.0)`,
        /// `cost_baseline_ms` 默认 1000ms。
        cost_index: f64,
        /// 归一化增益指数 [0.0, 1.0]
        ///
        /// 等于 `inference_gain`(增益本身已是 [0,1] 归一化分数)。
        gain_index: f64,
        /// 协调成本/推理增益比值
        ///
        /// `ratio = cost_index / gain_index`。`gain_index = 0` 时为 `f64::INFINITY`,
        /// 表示推理增益为零但协调成本非零,必然触发推理悖论风险。
        ratio: f64,
        /// 是否触发推理悖论风险(`ratio > threshold`)
        ///
        /// `true` 表示协调成本超过推理增益,多 Agent 协同的收益为负,
        /// 应考虑降级为单 Agent 模式或减少协调开销。
        is_paradox_risk: bool,
        /// 推理悖论告警阈值
        ///
        /// 默认 1.0(成本指数 = 增益指数为临界点)。可由 `CoordinationMetricsConfig`
        /// 自定义,降低阈值更敏感,升高阈值更宽松。
        threshold: f64,
        /// 已采集样本数
        ///
        /// 从收集器创建或上次 `reset()` 起累积的样本数,反映 EWMA 的置信度。
        /// 样本数 < 10 时比值波动较大,应谨慎用于决策。
        sample_count: u64,
    },

    /// polish-v2.7 P1-2: 运行时审计发现 — L9 efficiency-monitor(RuntimeAuditor)→ 任意订阅者
    ///
    /// 由 `RuntimeAuditor` 在审计能力/配置时发布,携带单条审计发现。订阅者据此:
    /// - **chimera-tui**:自评仪表盘展示待处理 Finding 列表
    /// - **repo-wiki**:沉淀高频 Finding 模式为知识条目
    ///
    /// WHY Normal 级别:审计发现是观察性事实陈述,非阻断事件。告警语义由订阅者
    /// 解释(同 `CoordinationRatioReported` 的设计原则"事件是事实,告警是解释")。
    ///
    /// WHY 字符串标签而非枚举:遵循 `R2FreezeViolation.violation_type: String` 先例,
    /// 避免在 L1 event-bus 引入 L9 专属枚举造成反向语义耦合。
    /// 合法取值见 `efficiency-monitor/src/auditor.rs` 的 `FindingSeverity`/`FindingCategory`。
    AuditFindingRaised {
        /// 事件元数据
        metadata: EventMetadata,
        /// 发现严重度标签("info" / "low" / "medium" / "high")
        finding_severity: String,
        /// 发现类别标签("unused_capability" / "verified_capability" / "evidence_gap")
        category: String,
        /// 人类可读描述(如 "Capability 'x' configured but never used")
        message: String,
        /// 证据种类("static_only" = 仅静态配置 / "runtime_events" = 有运行时事件证据)
        ///
        /// 对应 Qoder 证据纪律:静态发现 ≠ 已执行验证,只有 runtime_events
        /// 才计入五维度评分的"已验证"正证据。
        evidence_kind: String,
        /// 修复建议(无需动作时为描述性文本)
        fix_hint: String,
    },

    /// polish-v2.7 P1-2: 五维度 Harness 报告生成 — L9 efficiency-monitor(RuntimeAuditor)→ 任意订阅者
    ///
    /// 由 `RuntimeAuditor::generate_report` 周期性(或按需)发布,携带 Qoder Better
    /// Harness 五个维度的实时评分快照。订阅者据此:
    /// - **chimera-tui**:五维度 Gauge 仪表盘实时刷新
    /// - **gsoe-evolution(AEGIS)**:低分维度作为 Digester 的适应方向输入
    ///
    /// WHY 携带完整五维字段:仅 5 个 f32 + 1 个 u32(约 24 字节),远小于消息上限,
    /// 完整字段便于订阅者直接消费无需反向查询(同 CoordinationRatioReported 先例)。
    HarnessReportGenerated {
        /// 事件元数据
        metadata: EventMetadata,
        /// 任务理解维度评分 [0.0, 1.0]
        task_comprehension: f32,
        /// 可控执行维度评分 [0.0, 1.0]
        controllable_execution: f32,
        /// 变更验证维度评分 [0.0, 1.0]
        change_verification: f32,
        /// 可靠交付维度评分 [0.0, 1.0]
        reliable_delivery: f32,
        /// 经验沉淀维度评分 [0.0, 1.0]
        experience_accumulation: f32,
        /// 本次报告携带的审计发现数
        findings_count: u32,
    },

    /// L8 议会审议完成 — Parliament → L9 quest-engine / 任意订阅者
    ///
    /// 由 `Parliament::deliberate_with_policy` 在每次审议结束时发布
    /// (Reached / Rejected / Vetoed 全路径),携带审议端到端 wall-clock 延迟
    /// 与投票质量指标,供 quest-engine 填充 `CoordinationCostSample` 的
    /// `parliament_debate_latency_ms` 与 `InferenceGainSample` 的
    /// `consensus_quality`(协调度量接线闭环)。
    ///
    /// WHY 新增 Normal 事件而非扩容 Critical 级 `ConsensusReached`:
    /// ConsensusReached 走 mpsc 旁路且被 GSOE/AutoDPO/SecCore 多方消费,
    /// 扩字段回归面大;观测数据与治理决策事件分离更干净
    /// (同 `CoordinationRatioReported` 的设计原则"事件是事实,告警是解释")。
    DebateCompleted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 关联的 Quest ID(quest-engine 按此键合并待采样指标)
        quest_id: String,
        /// 提案 ID(审计追溯)
        proposal_id: String,
        /// 审议端到端延迟(毫秒)
        ///
        /// 口径:`deliberate_with_policy` 入口到共识返回的 wall-clock,
        /// 含 Skeptic 检测 + Opinion 收集 + 投票 + 事件发布串行 await 开销。
        debate_latency_ms: f64,
        /// 激活策略标签("fast-path" / "simplified" / "full",
        /// 取自 `ActivationStrategy::short_name()`)
        strategy: String,
        /// 加权赞成率 [0.0, 1.0](可选)
        ///
        /// 作为议会共识质量的 proxy(共识置信度),取自 `VoteResult`。
        /// `None` 表示该路径无投票(FastPath 直通 / Skeptic 前置否决)。
        /// 注意:这是置信度代理而非决策正确率 ground truth,
        /// 真实"决策正确率复盘"留待未来 GSOE 反馈闭环。
        weighted_approval_rate: Option<f32>,
        /// 参与率 [0.0, 1.0](可选,已投票角色数 / 总角色数)
        ///
        /// `None` 语义同 `weighted_approval_rate`。
        participation_rate: Option<f32>,
        /// 意见分歧度 [0.0, 1.0](可选,M2-T2.1 多维共识质量)
        ///
        /// 加权 position 方差归一化:全体一致=0,半赞成半反对=1。
        /// `#[serde(default)]` 保证旧序列化数据(无此字段)反序列化兼容。
        /// `None` 同 `weighted_approval_rate`(无投票路径)。
        #[serde(default)]
        divergence: Option<f32>,
        /// 弃权率 [0.0, 1.0](可选,弃权权重和 / 全部投票权重和)
        #[serde(default)]
        abstention_rate: Option<f32>,
        /// 共识裕度 [-1.0, 1.0](可选,approval_rate − consensus_threshold)
        #[serde(default)]
        consensus_margin: Option<f32>,
        /// 审议结果标签("Reached" / "Rejected" / "Vetoed")
        outcome: String,
    },

    /// 多 Agent 委托批次完成 — L9 chimera-mas(DelegationExecutor)→ 任意订阅者
    ///
    /// 由 `DelegationExecutor::execute_delegation` / `execute_batch_delegation`
    /// 在整批子任务汇聚完成后发布,携带批次 wall-clock 总开销,供 quest-engine
    /// 填充 `CoordinationCostSample.delegation_overhead_ms`(协调度量接线闭环)。
    ///
    /// WHY 批次 wall-clock 而非各子任务 duration 求和:子任务并行执行,
    /// 求和会重复计费;wall-clock 才是委托对 Quest 生命周期的真实时间开销。
    DelegationCompleted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 委托方 Agent ID
        parent_id: String,
        /// 关联的 Quest ID(可选)
        ///
        /// 取自子任务的 `AgentTask.quest_id` 关联字段;调用方未设置时为
        /// `None`,quest-engine 无法归因,仅记 debug 日志跳过合并。
        quest_id: Option<String>,
        /// 批次总开销(毫秒,派发到全部结果汇聚的 wall-clock)
        total_overhead_ms: f64,
        /// 子任务总数
        sub_task_count: u32,
        /// 成功子任务数
        success_count: u32,
    },

    /// L8 议会策略封顶变更 — Parliament(StrategyCapGuard)→ 任意订阅者
    ///
    /// 由 `StrategyCapGuard` 在协调成本/推理增益比值(ratio)连续越阈/回落
    /// 触发封顶升降时发布,供 TUI/efficiency-monitor 展示推理悖论风控动作。
    ///
    /// WHY Normal 级:封顶只影响审议深度上限(Full→Simplified→FastPath),
    /// Skeptic 否决检查在任何封顶档位照常执行(红队防线不变量),
    /// 事件丢失仅影响观测展示,不影响安全决策。
    ParliamentStrategyCapChanged {
        /// 事件元数据
        metadata: EventMetadata,
        /// 变更前封顶("fast-path" / "simplified" / "full")
        old_cap: String,
        /// 变更后封顶(同上取值)
        new_cap: String,
        /// 触发变更的协调成本/推理增益比值
        ratio: f64,
        /// 推理悖论告警阈值(来自 CoordinationRatioReported)
        threshold: f64,
    },

    // ============================================================
    // MCA M0(ADR-065):L10 mca-gateway 会话级/治理级事件(6 个新变体)
    //
    // WHY 只有会话级事件入 event-bus:流式数据面(per-token delta)走
    // 专用 bounded mpsc 直连调用方(ADR-065 决策 4),broadcast 1024 容量
    // 承载不了 per-token 流,Lagged 丢弃会破坏 TUI 体验。
    // ============================================================
    /// 路由决策留痕 — mca-gateway → model-router/omega-learner
    ///
    /// 每次通道选择发布,携带预估成本(P6 成本先行):路由历史与
    /// 学习臂(M3 s9 接缝)的数据源。
    ModelAffinitySelected {
        /// 事件元数据
        metadata: EventMetadata,
        /// 关联的用户意图标识(全链路追踪)
        intent_id: String,
        /// 路由键 `provider/model`(ProviderId::as_str 稳定形态)
        route_key: String,
        /// 实际使用的协议方言("open_ai_chat"/"anthropic_messages"/"open_ai_responses")
        dialect: String,
        /// 预估成本(微元,整数化禁浮点中间态)
        cost_estimate_micro: u64,
        /// 生效的峰谷系数百分比(100 = 1×,DeepSeek 高峰 200)
        peak_factor_percent: u16,
    },

    /// 跨厂商辩论通道选择 — parliament → mca-gateway/efficiency-monitor
    ///
    /// MCA P2-1 跨厂商辩论的通道选择留痕，记录每个角色在辩论中使用的
    /// 厂商通道，用于审计跨厂商去相关合规性(P7)与体验对等验收(E1-E5)。
    ///
    /// WHY Normal 级(非 Critical):跨厂商通道选择是辩论的准备阶段，
    /// 通道选择失败不会导致会话中断，降级为同厂商后仍可继续辩论。
    /// 丢失此事件不影响核心辩论流程，仅影响审计与体验分析。
    CrossVendorNegotiation {
        /// 事件元数据
        metadata: EventMetadata,
        /// 辩论会话 ID(与 DebateStarted 的 session_id 一致)
        session_id: String,
        /// 关联的 Quest ID
        quest_id: String,
        /// 生产者使用的厂商(ProviderId::as_str)
        producer_provider: String,
        /// 验证者使用的厂商
        verifier_provider: String,
        /// 怀疑者使用的厂商
        skeptic_provider: String,
        /// 是否强制了跨厂商去相关
        cross_vendor_enforced: bool,
        /// 去相关状态("enforced"/"fallback_same"/"fallback_skip")
        decorrelation_status: String,
    },

    /// 通道健康恶化 — mca-gateway → csn-substitutor/model-router
    ///
    /// 健康探针(TTFT/成功率 EWMA)跨过阈值或熔断器开闸时发布,
    /// 触发降级链评估与路由权重下调。
    ProviderDegraded {
        /// 事件元数据
        metadata: EventMetadata,
        /// 受影响的路由键 `provider/model`
        route_key: String,
        /// 恶化原因(如 "circuit_open: 5 consecutive 5xx")
        reason: String,
        /// 当前健康分(0-100,EWMA 折算)
        health_score: u8,
    },

    /// 能力协商结果 — mca-gateway → efficiency-monitor
    ///
    /// 三态降级协议(ADR-065/设计文档 §7 Round 3)的留痕:降级必须
    /// 明确告知(E4 不变量),特性启用率 = 实际启用/声明特性的分母数据源。
    AffinityCapabilityNegotiated {
        /// 事件元数据
        metadata: EventMetadata,
        /// 路由键 `provider/model`
        route_key: String,
        /// 协商保真度("full_fidelity"/"degraded_notified"/"channel_rejected")
        fidelity: String,
        /// 被降级的能力名清单(空 = 全保真)
        degraded_capabilities: Vec<String>,
    },

    /// [Critical] 厂商额度耗尽 — mca-gateway → decb-governor/csn-substitutor
    ///
    /// WHY Critical:额度耗尽意味着该通道即刻不可用,必须立即切换
    /// 通道才能保障会话连续性(E5 不变量)。丢失导致降级链无人触发、
    /// 请求持续打向死通道,语义对齐 BudgetExceeded Critical 红线。
    /// 必须同时列入 severity() 与 bus.rs is_critical_mpsc_event() 双清单。
    AffinityQuotaExhausted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 耗尽的路由键 `provider/model`
        route_key: String,
        /// 厂商限流/额度错误原文(申诉与排查用)
        reason: String,
    },

    /// 未知字段/事件留痕 — mca-gateway → repo-wiki/efficiency-monitor
    ///
    /// P3 双向容错的可观测面:响应中不认识的字段/事件类型吞掉不报错,
    /// 但必须留痕驱动 affinity.d spec 更新(厂商 API 演进信号源)。
    AffinityUnknownField {
        /// 事件元数据
        metadata: EventMetadata,
        /// 来源路由键 `provider/model`
        route_key: String,
        /// 协议方言(同 ModelAffinitySelected.dialect 取值)
        dialect: String,
        /// 未知内容摘录(截断后的原文,避免大 payload 进 broadcast)
        raw_excerpt: String,
    },

    /// 流式会话闭环 — mca-gateway → decb-governor/auto-dpo
    /// (历史上的下游 acb-governor 已按 ADR-182 退役删除 2026-09-16)
    ///
    /// 会话结束时发布真实计量:成本回写(EWMA α=0.1)、缓存命中率
    /// 回读、DPO 偏好对轨迹的数据源;TTFT 喂入健康探针与 E1 验收。
    StreamSessionCompleted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 关联的用户意图标识
        intent_id: String,
        /// 服务本会话的路由键 `provider/model`
        route_key: String,
        /// 输入 token 数
        input_tokens: u64,
        /// 输出 token 数
        output_tokens: u64,
        /// 缓存命中 token 数(隐式/显式缓存族统一口径)
        cache_hit_tokens: u64,
        /// 实际成本(微元,基于 usage 回算)
        cost_actual_micro: u64,
        /// 首 token 延迟(毫秒,E1 体验不变量度量)
        ttft_ms: u64,
        /// 是否为语义缓存命中(false=厂商调用路径,true=语义缓存热路径)
        semantic_cache_hit: bool,
        /// 上下文裁剪前估算 token(None = 未触发裁剪,观测闭环 ADR-070)
        ///
        /// 与 `trimmed_after_tokens` 成对出现:差值 = 裁剪节省量,
        /// 供 efficiency-monitor 验证 R4 裁剪收益与 SMART 等效输入成本目标。
        trimmed_before_tokens: Option<u64>,
        /// 上下文裁剪后估算 token(None = 未触发裁剪)
        trimmed_after_tokens: Option<u64>,
        /// 历史消息压缩率(实际压缩量/原始量;None = 未压缩,sidecar 降级原文也记 None)
        compressed_ratio: Option<f32>,
        /// early stop 原因(自然结束 = None;BudgetExceeded/SemanticComplete = 原因名)
        ///
        /// 字符串而非枚举:事件是跨层观测面,避免 L10 枚举泄漏到 L1 语义层;
        /// 消费方按需解析(参考事件字段区分决策,不新增事件变体)。
        early_stop_reason: Option<String>,
        /// 是否 in-flight 请求合并命中(共享一次厂商调用)
        coalesced: bool,
    },

    /// 窗口亲和折减结果 — mca-gateway hcw_integration → hcw-window
    ///
    /// MCA P5 承诺不超发:模型实际上限折减后,网关发布此事件告知 HCW
    /// 实际允许的窗口档位。`hcw-window` 消费后调整 `HcwWindow.current_tier`,
    /// 确保 1M 等效承诺不超出模型上限。
    ///
    /// # 跨层通信(C6)
    /// L10(mca-gateway) → L2(hcw-window),经 event-bus 解耦。
    /// 本事件为 Normal 级别(观测面),不触发 mpsc 旁路。
    WindowAffinityApplied {
        /// 事件元数据
        metadata: EventMetadata,
        /// 路由键 `provider/model`(与 ModelAffinitySelected 一致)
        route_key: String,
        /// 是否发生了折减(请求 L3 但模型上限不足)
        folded: bool,
        /// 是否需要任务分块(折减到 L2 封顶的中等窗口)
        needs_chunking: bool,
        /// 折减后的实际档位("L0"/"L1"/"L2"/"L3")
        tier: String,
    },

    /// 缓存亲和策略应用结果 — mca-gateway codec → scc-cache
    ///
    /// MCA A3 缓存亲和:记录当前请求使用的缓存策略(显式/隐式/无)及
    /// cache_control 断点位置。`scc-cache` 消费后调整缓存预取策略。
    ///
    /// # 跨层通信(C6)
    /// L10(mca-gateway) → L3(scc-cache),经 event-bus 解耦。
    /// 本事件为 Normal 级别(观测面),不触发 mpsc 旁路。
    CacheAffinityApplied {
        /// 事件元数据
        metadata: EventMetadata,
        /// 路由键 `provider/model`(与 ModelAffinitySelected 一致)
        route_key: String,
        /// 缓存策略: "none" / "implicit" / "explicit_control"
        strategy: String,
        /// 是否注入了 cache_control 断点(仅 ExplicitControl 族)
        cache_control_injected: bool,
        /// 断点数量(ExplicitControl 族,否则 0)
        breakpoint_count: u32,
    },

    // ============================================================
    // ADR-069 Token 效率优化事件
    // ============================================================
    /// 上下文预算分配 — OSA budget_mask 联动结果通知
    ///
    /// L6(osa-coordinator) → L2(hcw-window) / L10(mca-gateway)，
    /// 通知各层当前 token 预算分配。Normal 级别，丢失可由下次周期补偿。
    ///
    /// **[RESERVED]** §16.4 审计结论:变体已落地但生产发布端缺失
    /// (osa-coordinator 零生产构造)。标注 reserved 保留序列化兼容,
    /// 待后续装配时激活(Phase 10 Wave 4 孤儿治理)。
    ContextBudgetAllocated {
        /// 事件元数据
        metadata: EventMetadata,
        /// 分配的 token 预算
        budget_tokens: u32,
        /// 窗口层级描述
        tier: String,
        /// 当前稀疏度
        sparsity: f32,
    },
    /// 语义缓存命中 — 度量用（语义缓存命中率监控）
    ///
    /// L3(scc-cache) → 任意订阅者，Normal 级别。
    SemanticCacheHit {
        /// 事件元数据
        metadata: EventMetadata,
        /// 命中的命名空间
        namespace: String,
        /// 匹配相似度
        similarity: f32,
    },

    // ============================================================
    // P2-8 MemCon:幽灵记忆检测与策略自适应调整
    // ============================================================
    /// 幽灵记忆检测事件 — 当 MemCon 控制器检测到幽灵记忆模式时发布
    ///
    /// P2-8 MemCon 自适应控制器:当滑动窗口内幽灵记忆检测率超过阈值时,
    /// GhostMemoryDetector 发布此事件,通知订阅者(如 efficiency-monitor)
    /// 当前记忆系统中存在幽灵记忆现象。
    ///
    /// # 跨层通信(C7)
    /// L2(mlc-engine) → 任意订阅者,经 event-bus 解耦。
    /// 本事件为 Normal 级别(观测面),不触发 mpsc 旁路。
    ///
    /// # 使用场景
    /// - efficiency-monitor 订阅后触发告警
    /// - StrategyAdapter 订阅后触发策略衰减
    /// - TUI 事件面板显示幽灵记忆状态
    GhostMemoryDetected {
        /// 事件元数据
        metadata: EventMetadata,
        /// 幽灵记忆检测率(最近窗口内,范围 [0.0, 1.0])
        ghost_rate: f32,
        /// 窗口内检测到的幽灵记忆计数
        ghost_count: u32,
        /// 窗口总召回数
        total_recalls: u32,
        /// 当前活跃记忆策略(如 "StandardTopK" / "AggressivePruning")
        current_strategy: String,
    },

    /// MemCon 策略调整事件 — 当 MemCon 控制器自适应调整记忆策略时发布
    ///
    /// P2-8 MemCon 自适应控制器:StrategyAdapter 根据幽灵记忆检测结果
    /// 动态调整记忆策略时发布此事件,通知订阅者策略变更。
    ///
    /// # 跨层通信(C7)
    /// L2(mlc-engine) → 任意订阅者,经 event-bus 解耦。
    /// 本事件为 Normal 级别(观测面),不触发 mpsc 旁路。
    ///
    /// # 使用场景
    /// - efficiency-monitor 订阅后记录策略变更
    /// - TUI 事件面板显示策略调整历史
    /// - 全局记忆策略快照更新触发
    MemConStrategyAdjusted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 调整前策略(如 "StandardTopK")
        from_strategy: String,
        /// 调整后策略(如 "AggressivePruning")
        to_strategy: String,
        /// 调整原因(如 "ghost_memory_detected" / "stable_recovery" / "circuit_breaker")
        reason: String,
        /// 触发调整的幽灵记忆检测率(仅 ghost 相关原因时有值)
        ghost_rate: Option<f32>,
    },

    /// 基准指标采集完成（仅基准模式发布，Normal 级别）
    ///
    /// **[RESERVED]** §16.4 审计结论:变体已落地但生产发布端缺失
    /// (efficiency-monitor 基准模式零生产构造)。标注 reserved 保留
    /// 序列化兼容,待后续装配时激活(Phase 10 Wave 4 孤儿治理)。
    BenchmarkMetricsCollected {
        /// 事件元数据
        metadata: EventMetadata,
        /// 等效输入成本（微元，含缓存写入溢价摊销）
        equivalent_input_cost_micro: u64,
        /// 厂商缓存命中率（百分数，各厂商归一，0-100）
        vendor_cache_hit_rate_percent: u8,
        /// 语义缓存命中率（百分数，0-100）
        semantic_cache_hit_rate_percent: u8,
        /// TTFT P95（毫秒）
        ttft_p95_ms: u64,
        /// 输出 token 总量
        total_output_tokens: u64,
        /// 任务成功率（百分数，0-100）
        task_success_rate_percent: u8,
        /// 分厂商指标快照 JSON（provider → { hit_rate, cost, output_tokens }）
        per_vendor_snapshot_json: String,
    },

    // ============================================================
    // PROBE P0:HCW 召回评测事件(观测面,均为 Normal 级,走通配分支)
    // ============================================================
    /// HCW 召回评测报告事件 — PROBE P0 评测尺子产出（Normal 级观测面）
    ///
    /// # 跨层通信
    /// L2(hcw-window) → 任意订阅者(如 efficiency-monitor),经 event-bus 解耦。
    /// 本事件为 Normal 级别(观测面),不触发 mpsc 旁路——与 Critical 清单正交,
    /// `severity()` 走通配分支返回 Normal,显式 match 零修改(红线验证点)。
    ///
    /// # 使用场景
    /// - efficiency-monitor 召回 collector 订阅后做 EWMA 漂移跟踪(P0.4)
    /// - TUI OsaSparse 面板显示召回读数(P0.4)
    /// - P0.5 双基线对照表的持续观测通道
    HcwRecallReported {
        /// 事件元数据
        metadata: EventMetadata,
        /// 窗口档标识（L0/L1/L2/L3 或路径对照）
        tier: String,
        /// 多针召回率 needle_recall@8 ∈ [0,1]
        needle_recall_at_8: f32,
        /// 位置偏置比 ∈ [0,1]
        position_bias: f32,
        /// 链路成功率 ∈ [0,1]
        chain_success_rate: f32,
        /// 选中块数
        selected_count: u32,
    },

    /// HCW 召回退化事件 — PROBE 降级必告知（C6）时发布（Normal 级观测面）
    ///
    /// # 触发
    /// 召回哨兵连续 2 次低于基线 80%（P2 阶段,计划 §4.6 降级链）；
    /// 本事件通知订阅方自动升档窗口 + TUI 可见,禁止静默降召回。
    ///
    /// # 使用场景
    /// - efficiency-monitor 订阅后触发窗口升档建议
    /// - TUI OsaSparse 面板显示退化状态
    HcwRecallDegraded {
        /// 事件元数据
        metadata: EventMetadata,
        /// 当前档位（如 "L2"）
        tier: String,
        /// 当前召回率
        recall_rate: f32,
        /// 基线召回率（P0 冻结的对照值）
        baseline_recall: f32,
        /// 退化原因（如 "sentinel_2x_below_baseline"）
        reason: String,
    },
    /// PROBE P3.2: 超窗兜底触发（语料 > 有效窗口 → 两级检索链）
    OverWindowFallbackTriggered {
        /// 事件元数据
        metadata: EventMetadata,
        /// 语料规模（token）
        corpus_tokens: u64,
        /// 有效窗口（折减后，token）
        effective_window: u64,
        /// 候选集规模（kvbsr 块路由产出）
        candidate_count: u32,
        /// 精排后装窗数
        loaded_count: u32,
    },
    // ============================================================
    // L9 Ambient Mode → L9 Quest:资源恢复（Milestone B-2,append-only）
    // ============================================================
    /// 资源恢复 `[Normal]` — Ambient Mode 资源看门狗据此恢复被挂起的 Quest
    ///
    /// 与 `BudgetExceeded`（Critical）成对：预算超限挂起 → 资源恢复解除挂起。
    /// 由外部资源治理（调度器/管理员/CLI）在资源水位回落时发布。
    ResourceRecovered {
        /// 事件元数据
        metadata: EventMetadata,
        /// 资源类型（与 BudgetExceeded.budget_type 对应，如 "memory"）
        resource_type: String,
    },
    // ============================================================
    // L8 Parliament:行为契约强制层（Milestone B-3c,append-only）
    // ============================================================
    /// 行为契约违反 `[Critical]` — 强制层检出契约断言未覆盖，供 Parliament 审议
    ///
    /// 九层防御 L0 补齐（方案 §7.2）：BehaviorContract 违反 → 发布本事件 +
    /// Parliament 审议入口（违反即否决：候选不得进入后续阶段）。
    /// P1-5 升级 Critical：契约违反是安全语义（L0 "行为契约不可违反"），
    /// 丢失导致违反无人审议、候选继续进入后续阶段，因此走 mpsc 旁路确保投递。
    FormalViolation {
        /// 事件元数据
        metadata: EventMetadata,
        /// 契约 ID（如 "bc-test-1"）
        contract_id: String,
        /// 目标类型完整路径（如 "event_bus::EventBus"）
        target_type: String,
        /// 未被观测覆盖的断言列表
        violations: Vec<String>,
        /// 契约适用场景（Runtime/Test/Evolution）
        context: nexus_contracts::behavior_contract::ContractContext,
    },
    // ============================================================
    // L0 RewardSpec 奖励信号流（Milestone C-1,append-only）
    // ============================================================
    /// 奖励信号 `[Normal]` — 统一奖励框架的 EventBus 信号流载荷
    ///
    /// R1 数据面先接入（观测/回放池分层采样）；R2 训练面解冻后由训练服务消费。
    /// L4 安全观测信号（is_security_observation=true）仅观测不参与训练。
    RewardSignalReported {
        /// 事件元数据
        metadata: EventMetadata,
        /// 奖励信号（RewardSpec 加权后载荷）
        signal: nexus_contracts::reward::RewardSignal,
    },

    // ============================================================
    // §16.4 跨层事件协议补齐(Phase 10 审计修复 Wave 4)
    // ============================================================
    /// 停止策略裁决发布 — L8 Parliament → L9 Quest(规范 §16.4,Critical)
    ///
    /// ThreeFactorAdjudicator.adjudicate_stop 的裁决结果事件化(此前 StopRuling
    /// 仅本地枚举死代码);Critical 保障停止裁决不丢失(防 Quest 无界运行)。
    StopRulingIssued {
        /// 事件元数据
        metadata: EventMetadata,
        /// 所属 Quest ID
        quest_id: String,
        /// 裁决理由(停滞/达限/收益衰减等)
        reason: String,
        /// 是否保留历史最佳(Ω₉-Preserve)
        preserve_best: bool,
    },
    /// 变体审议通过 — L8 Parliament → L5/L6(规范 §16.4,Normal)
    ///
    /// ThreeFactorAdjudicator.adjudicate_variant 批准变体后发布,
    /// 供 L5 知识沉淀与 L6 算子路由消费。
    VariantApproved {
        /// 事件元数据
        metadata: EventMetadata,
        /// 变体标识(spec_name@spec_version)
        variant_id: String,
        /// 变体评分
        score: f32,
    },
    /// 三因子父本选择结果 — L5 Knowledge → L6/L9(规范 §16.4,Normal)
    ///
    /// select_parent 选择结果事件化,供 L6 算子路由与 L9 搜索树扩展消费。
    ParentSelected {
        /// 事件元数据
        metadata: EventMetadata,
        /// 所属任务 ID
        task_id: String,
        /// 选中父本节点 ID
        parent_node_id: String,
        /// 三因子归一化评分
        quality: f32,
        /// 进度因子
        progress: f32,
        /// 新颖性因子
        novelty: f32,
    },
    /// 错误签名匹配成功 — L4 Security → L2/L5(规范 §16.4,Critical)
    ///
    /// 铁律7 错误签名哈希去重聚类命中时发布(Debug 算子“相同错误签名
    /// 兄弟”检索的事件化通道);Critical 保障错误修复路径不丢失。
    ErrorSignatureMatched {
        /// 事件元数据
        metadata: EventMetadata,
        /// 错误签名哈希
        error_hash: String,
        /// 命中的卡片 ID 列表
        matched_card_ids: Vec<String>,
    },
    /// Token 证据记录 — L1 Core → L3 Storage(规范 §16.4,Normal)
    ///
    /// TokenLedger 追加条目后发布,通知 L3 持久化通道(铁律8 Token 证据全链路)。
    TokenLedgerRecorded {
        /// 事件元数据
        metadata: EventMetadata,
        /// 证据条目 ID
        evidence_id: String,
        /// Token 用量
        token_usage: u64,
    },
    /// 自我评估更新 — L10 Interface → L9(规范 §16.4,规范标 Low;
    /// EventSeverity 无 Low 变体,映射 Normal——丢失可由下周期报告补偿)
    ///
    /// RuntimeAuditor 周期报告的摘要事件(五维总分),供 L9 消费调整任务策略。
    AssessmentUpdated {
        /// 事件元数据
        metadata: EventMetadata,
        /// 五维加权总分
        overall_score: f32,
        /// 各维度评分(维度名 → 分值)
        dimensions: Vec<(String, f32)>,
    },
    // ============================================================
    // §16.5 跨层奖励传播 — L1 吞吐量观测(Phase 10 Wave 6,append-only)
    // ============================================================
    /// Event Bus 吞吐量报告 — L1 周期观测面事件(规范 §16.5,Normal)
    ///
    /// 审计发现规范要求"Event Bus 吞吐量"无实现;组合根周期拉取
    /// [`EventBus::published_total`] 计算速率后发布本事件(真实采集,
    /// 非伪造指标)。丢失可由下周期报告补偿。
    BusThroughputReported {
        /// 事件元数据
        metadata: EventMetadata,
        /// 窗口内发布事件总数
        published_total: u64,
        /// 窗口内事件/秒(上一周期差分速率)
        events_per_sec: f64,
        /// 观测窗口时长(秒)
        window_secs: u64,
    },
    /// L4 沙箱拦截率报告 — 安全层周期观测面事件(规范 §16.5,Normal)
    ///
    /// seccore 真实采集零信任沙箱的请求/拦截计数后发布本事件。
    /// 误拦截率需人工真值标注(哪些拦截是"错误"的),标注 v4.0 预留,
    /// 不实施假采集——对齐 §16.5 审计的诚实数据原则。
    SecurityInterceptionReported {
        /// 事件元数据
        metadata: EventMetadata,
        /// 累计请求总数(审计并执行入口)
        total_requests: u64,
        /// 累计被拦截数(任一防御层)
        blocked_requests: u64,
        /// 拦截率 blocked / total(无请求时为 0.0)
        interception_rate: f64,
    },
}
