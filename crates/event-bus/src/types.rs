//! 事件类型定义 — NEXUS-OMEGA 全维事件枚举
//!
//! 对应架构:十层架构 L1-L10 的跨层通信契约
//! 设计依据:Part A 依赖方向分析,通过预定义事件类型修正 4 处违规
//!
//! # 关键违规修正映射
//! - V1(OSA→HCW 向上依赖):`OmniSparseMasksComputed` 事件
//! - V2(MLC→efficiency-monitor 跨层):`MemoryMetricsReported` 事件
//! - V3/V4(Parliament→GSOE/AutoDPO 向上依赖):`ConsensusReached` 事件

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 事件元数据 — 每个事件携带,用于追踪、审计与因果排序
///
/// WHY 字段说明:
/// - `event_id`:UUIDv7(时间有序),便于跨进程因果追踪与去重
/// - `timestamp`:单调时钟来源,审计日志按此排序
/// - `source`:发布者 crate 名(如 "osa-coordinator"),用于依赖方向校验
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventMetadata {
    /// 事件唯一标识(UUIDv7,时间有序)
    pub event_id: Uuid,
    /// 事件产生时刻(UTC)
    pub timestamp: DateTime<Utc>,
    /// 发布者 crate 名,用于依赖方向校验与审计
    pub source: String,
}

impl EventMetadata {
    /// 以指定 source 创建元数据,event_id 与 timestamp 自动生成
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            event_id: Uuid::now_v7(),
            timestamp: Utc::now(),
            source: source.into(),
        }
    }
}

/// 事件严重级别 — 用于背压策略决定是否优先投递
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventSeverity {
    /// 普通事件:可被背压策略丢弃
    Normal,
    /// 警告事件:降级链耗尽、非致命异常等,建议优先投递但允许背压丢弃
    ///
    /// WHY:ChainExhausted 等降级链耗尽事件属于监控告警级别,
    /// 不如 Critical 致命(不会导致数据不一致或安全漏洞),但比
    /// Normal 重要(运维需监控降级链耗尽率以评估系统健康度)。
    /// 介于二者之间,新增 Warning 级别明确语义。
    Warning,
    /// 关键事件:检查点、共识、安全告警等,不可丢弃
    ///
    /// WHY:CheckpointSaved 等事件丢失会导致 Quest 无法恢复,
    /// 必须标注 Critical 以触发 mpsc 点对点通道或保留优先级
    Critical,
}

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
    /// 发布此事件通知 Execution 层采取对应动作。severity() 统一
    /// 返回 Normal(因为 severity() 是同步函数且不依赖运行时值);
    /// 但 Block 级别干预在语义上等价于 Critical,发布者应通过
    /// Critical 通道发送 Block 事件以确保投递。
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
    // Task 11 (N16): CSN 降级链耗尽事件
    //
    // WHY:原实现仅用 warn! 日志记录降级链耗尽,存在监控盲区——
    // 运维无法通过 EventBus 订阅统计降级链耗尽率、定位哪些能力
    // 频繁耗尽。新增此事件后,efficiency-monitor 可订阅统计指标,
    // Lead Architect 可据此告警。warn! 日志仍保留(向后兼容)。
    // Warning 级别:不如 Critical 致命(不会导致数据不一致),
    // 但比 Normal 重要(需监控系统健康度)。
    // ============================================================
    /// CSN 降级链已耗尽 — L10 Interface(csn-substitutor)→ 任意订阅者
    ///
    /// WHY:降级链耗尽意味着所有替代方案均不可达,能力完全失效。
    /// 原实现仅 warn! 日志,存在监控盲区;新增事件供
    /// efficiency-monitor 统计耗尽率、Lead Architect 触发告警。
    /// warn! 日志仍保留(向后兼容),事件是额外补充。
    ChainExhausted {
        /// 事件元数据
        metadata: EventMetadata,
        /// 耗尽的降级链 ID(通常等于 capability_id)
        chain_id: String,
        /// 最后一个错误的描述信息(来自 CsnError::ChainExhausted 的 Display)
        last_error: String,
    },
}

impl NexusEvent {
    /// 获取事件元数据引用
    pub fn metadata(&self) -> &EventMetadata {
        match self {
            Self::UserIntentEncoded { metadata, .. }
            | Self::NexusStateChanged { metadata, .. }
            | Self::ModelRouteSelected { metadata, .. }
            | Self::QuestCreated { metadata, .. }
            | Self::QuestProgressUpdated { metadata, .. }
            | Self::ThinkingModeSwitched { metadata, .. }
            | Self::CheckpointSaved { metadata, .. }
            | Self::CheckpointLoaded { metadata, .. }
            | Self::ConsensusReached { metadata, .. }
            | Self::VoteCast { metadata, .. }
            | Self::CapabilityFrozen { metadata, .. }
            | Self::BudgetExceeded { metadata, .. }
            | Self::SandboxViolation { metadata, .. }
            | Self::OperationProduced { metadata, .. }
            | Self::PredictionVerified { metadata, .. }
            | Self::OmniSparseMasksComputed { metadata, .. }
            | Self::ToolsRouted { metadata, .. }
            | Self::ExecutionCompleted { metadata, .. }
            | Self::MemoryMetricsReported { metadata, .. }
            | Self::MemoryTiered { metadata, .. }
            | Self::CacheHit { metadata, .. }
            | Self::CacheMiss { metadata, .. }
            | Self::WikiUpdated { metadata, .. }
            | Self::EvolutionTriggered { metadata, .. }
            | Self::DpoPairGenerated { metadata, .. }
            | Self::AuditLogged { metadata, .. }
            | Self::McpMessageReceived { metadata, .. }
            | Self::SlowConsumerDropped { metadata, .. }
            | Self::ContextWindowSwitched { metadata, .. }
            | Self::ContextCompressed { metadata, .. }
            | Self::CapabilityTiered { metadata, .. }
            | Self::BlocksRebalanced { metadata, .. }
            | Self::ExpertActivated { metadata, .. }
            | Self::ActivationThresholdAdjusted { metadata, .. }
            | Self::ActivationCacheStats { metadata, .. }
            | Self::GatherCompleted { metadata, .. }
            | Self::OperationTimedOut { metadata, .. }
            | Self::GatherTimedOut { metadata, .. }
            | Self::OrphanCallDetected { metadata, .. }
            | Self::ProducerStrategyAdjusted { metadata, .. }
            | Self::PredictionMade { metadata, .. }
            | Self::PredictionStatsReported { metadata, .. }
            | Self::PredictionRolledBack { metadata, .. }
            | Self::CachePrefetched { metadata, .. }
            | Self::CacheStatsReported { metadata, .. }
            | Self::ExpertRouted { metadata, .. }
            | Self::EntropyBalanced { metadata, .. }
            | Self::ExpertRegistered { metadata, .. }
            | Self::ExpertUnregistered { metadata, .. }
            | Self::DebateStarted { metadata, .. }
            | Self::SkepticVeto { metadata, .. }
            | Self::VetoOverridden { metadata, .. }
            | Self::RedTeamAudit { metadata, .. }
            | Self::BudgetAdjusted { metadata, .. }
            | Self::AsaIntervention { metadata, .. }
            | Self::AhirtProbeCompleted { metadata, .. }
            | Self::RoleRegistered { metadata, .. }
            | Self::BudgetStatsReported { metadata, .. }
            | Self::NmcEncoded { metadata, .. }
            | Self::ChtcToolCallReceived { metadata, .. }
            | Self::SsraFusionCompleted { metadata, .. }
            | Self::GsoePolicyUpdated { metadata, .. }
            | Self::LsctTierSwitched { metadata, .. }
            | Self::McpMeshTransactionCompleted { metadata, .. }
            | Self::CsnSubstitutionTriggered { metadata, .. }
            | Self::SesaActivationCompleted { metadata, .. }
            | Self::EfficiencyAlertTriggered { metadata, .. }
            | Self::ChainExhausted { metadata, .. } => metadata,
        }
    }

    /// 判断事件是否为关键事件(Critical)
    ///
    /// 关键事件:CheckpointSaved、ConsensusReached、SlowConsumerDropped、
    /// OrphanCallDetected(Week 4 新增)、SkepticVeto/RedTeamAudit(Week 5 新增)、
    /// VetoOverridden(P1-3 新增:否决覆盖审计)、
    /// BudgetExceeded(F-001 修复:Hard Constraint 第 10 条要求)
    /// 这些事件丢失会导致系统状态不一致或告警遗漏
    ///
    /// WHY BudgetExceeded 标记为 Critical:预算耗尽是系统红线,意味着资源
    /// 已达上限,必须立即触发背压保护(走 mpsc 点对点通道确保投递)并通知
    /// Parliament 触发降级或终止。若标为 Normal,在背压场景下可能被丢弃,
    /// 导致预算超限无人响应、Quest 持续消耗资源直至 OOM,违反架构红线
    /// "1M Token 暴力加载"的预防机制。此为 Hard Constraint 第 10 条的
    /// 强制要求(F-001 修复)。
    ///
    /// WHY:Week 3 新增的 4 个变体(ContextWindowSwitched/ContextCompressed/
    /// CapabilityTiered/BlocksRebalanced)均为 Normal 级别,由通配符分支
    /// 自动覆盖。Week 4 新增的 16 个变体中,仅 OrphanCallDetected 为 Critical
    /// (对应 Claude Code 尸检 5.4% 孤儿调用教训),其余 15 个为 Normal,
    /// 由通配符分支自动覆盖。Week 5 新增的 8 个变体中,SkepticVeto(否决权
    /// 行使)与 RedTeamAudit(红队漏洞审计)为 Critical(丢失导致安全机制
    /// 失效),其余 6 个为 Normal,由通配符分支自动覆盖。P1-3 新增
    /// VetoOverridden 为 Critical(否决覆盖审计,丢失导致覆盖行为不可追溯)。
    /// 若未来新增 Critical 事件,必须在此显式列出,避免被通配符误判为 Normal。
    pub fn severity(&self) -> EventSeverity {
        match self {
            Self::CheckpointSaved { .. }
            | Self::ConsensusReached { .. }
            | Self::SlowConsumerDropped { .. }
            | Self::OrphanCallDetected { .. }
            | Self::SkepticVeto { .. }
            | Self::VetoOverridden { .. }
            | Self::RedTeamAudit { .. }
            | Self::BudgetExceeded { .. } => EventSeverity::Critical,
            // Task 11 (N16): ChainExhausted 为 Warning 级别
            // WHY:降级链耗尽意味着所有替代方案不可达,能力完全失效,
            // 但不会导致数据不一致或安全漏洞(不如 Critical 致命),
            // 运维需监控此指标评估系统健康度(比 Normal 重要)
            Self::ChainExhausted { .. } => EventSeverity::Warning,
            _ => EventSeverity::Normal,
        }
    }

    /// 事件类型名(用于序列化 tag 与日志)
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::UserIntentEncoded { .. } => "UserIntentEncoded",
            Self::NexusStateChanged { .. } => "NexusStateChanged",
            Self::ModelRouteSelected { .. } => "ModelRouteSelected",
            Self::QuestCreated { .. } => "QuestCreated",
            Self::QuestProgressUpdated { .. } => "QuestProgressUpdated",
            Self::ThinkingModeSwitched { .. } => "ThinkingModeSwitched",
            Self::CheckpointSaved { .. } => "CheckpointSaved",
            Self::CheckpointLoaded { .. } => "CheckpointLoaded",
            Self::ConsensusReached { .. } => "ConsensusReached",
            Self::VoteCast { .. } => "VoteCast",
            Self::CapabilityFrozen { .. } => "CapabilityFrozen",
            Self::BudgetExceeded { .. } => "BudgetExceeded",
            Self::SandboxViolation { .. } => "SandboxViolation",
            Self::OperationProduced { .. } => "OperationProduced",
            Self::PredictionVerified { .. } => "PredictionVerified",
            Self::OmniSparseMasksComputed { .. } => "OmniSparseMasksComputed",
            Self::ToolsRouted { .. } => "ToolsRouted",
            Self::ExecutionCompleted { .. } => "ExecutionCompleted",
            Self::MemoryMetricsReported { .. } => "MemoryMetricsReported",
            Self::MemoryTiered { .. } => "MemoryTiered",
            Self::CacheHit { .. } => "CacheHit",
            Self::CacheMiss { .. } => "CacheMiss",
            Self::WikiUpdated { .. } => "WikiUpdated",
            Self::EvolutionTriggered { .. } => "EvolutionTriggered",
            Self::DpoPairGenerated { .. } => "DpoPairGenerated",
            Self::AuditLogged { .. } => "AuditLogged",
            Self::McpMessageReceived { .. } => "McpMessageReceived",
            Self::SlowConsumerDropped { .. } => "SlowConsumerDropped",
            Self::ContextWindowSwitched { .. } => "ContextWindowSwitched",
            Self::ContextCompressed { .. } => "ContextCompressed",
            Self::CapabilityTiered { .. } => "CapabilityTiered",
            Self::BlocksRebalanced { .. } => "BlocksRebalanced",
            Self::ExpertActivated { .. } => "ExpertActivated",
            Self::ActivationThresholdAdjusted { .. } => "ActivationThresholdAdjusted",
            Self::ActivationCacheStats { .. } => "ActivationCacheStats",
            Self::GatherCompleted { .. } => "GatherCompleted",
            Self::OperationTimedOut { .. } => "OperationTimedOut",
            Self::GatherTimedOut { .. } => "GatherTimedOut",
            Self::OrphanCallDetected { .. } => "OrphanCallDetected",
            Self::ProducerStrategyAdjusted { .. } => "ProducerStrategyAdjusted",
            Self::PredictionMade { .. } => "PredictionMade",
            Self::PredictionStatsReported { .. } => "PredictionStatsReported",
            Self::PredictionRolledBack { .. } => "PredictionRolledBack",
            Self::CachePrefetched { .. } => "CachePrefetched",
            Self::CacheStatsReported { .. } => "CacheStatsReported",
            Self::ExpertRouted { .. } => "ExpertRouted",
            Self::EntropyBalanced { .. } => "EntropyBalanced",
            Self::ExpertRegistered { .. } => "ExpertRegistered",
            Self::ExpertUnregistered { .. } => "ExpertUnregistered",
            Self::DebateStarted { .. } => "DebateStarted",
            Self::SkepticVeto { .. } => "SkepticVeto",
            Self::VetoOverridden { .. } => "VetoOverridden",
            Self::RedTeamAudit { .. } => "RedTeamAudit",
            Self::BudgetAdjusted { .. } => "BudgetAdjusted",
            Self::AsaIntervention { .. } => "AsaIntervention",
            Self::AhirtProbeCompleted { .. } => "AhirtProbeCompleted",
            Self::RoleRegistered { .. } => "RoleRegistered",
            Self::BudgetStatsReported { .. } => "BudgetStatsReported",
            Self::NmcEncoded { .. } => "NmcEncoded",
            Self::ChtcToolCallReceived { .. } => "ChtcToolCallReceived",
            Self::SsraFusionCompleted { .. } => "SsraFusionCompleted",
            Self::GsoePolicyUpdated { .. } => "GsoePolicyUpdated",
            Self::LsctTierSwitched { .. } => "LsctTierSwitched",
            Self::McpMeshTransactionCompleted { .. } => "McpMeshTransactionCompleted",
            Self::CsnSubstitutionTriggered { .. } => "CsnSubstitutionTriggered",
            Self::SesaActivationCompleted { .. } => "SesaActivationCompleted",
            Self::EfficiencyAlertTriggered { .. } => "EfficiencyAlertTriggered",
            Self::ChainExhausted { .. } => "ChainExhausted",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_creation() {
        let meta = EventMetadata::new("osa-coordinator");
        assert_eq!(meta.source, "osa-coordinator");
        assert!(!meta.event_id.to_string().is_empty());
    }

    #[test]
    fn test_severity_classification() {
        let critical = NexusEvent::CheckpointSaved {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q1".into(),
            checkpoint_id: "c1".into(),
            memory_snapshot_hash: "abc".into(),
        };
        assert_eq!(critical.severity(), EventSeverity::Critical);

        let normal = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        };
        assert_eq!(normal.severity(), EventSeverity::Normal);
    }

    #[test]
    fn test_type_name_stable() {
        let e = NexusEvent::VoteCast {
            metadata: EventMetadata::new("parliament"),
            proposal_id: "p1".into(),
            voter: "v1".into(),
            vote: true,
        };
        assert_eq!(e.type_name(), "VoteCast");
    }

    // ============================================================
    // Week 4 扩展测试:验证新增 16 个事件变体的行为
    // ============================================================

    #[test]
    fn test_week4_event_orphan_call_critical() {
        let e = NexusEvent::OrphanCallDetected {
            metadata: EventMetadata::new("gqep-executor"),
            operation_id: "op-1".into(),
            spawn_location: "gatherer.rs:42".into(),
        };
        assert_eq!(e.severity(), EventSeverity::Critical);
        assert_eq!(e.type_name(), "OrphanCallDetected");
    }

    #[test]
    fn test_week4_event_expert_activated_normal() {
        let e = NexusEvent::ExpertActivated {
            metadata: EventMetadata::new("gea-activator"),
            activated_experts: vec!["e1".into(), "e2".into()],
            suppressed_experts: vec!["e3".into()],
            top_gate_value: 0.85,
        };
        assert_eq!(e.severity(), EventSeverity::Normal);
        assert_eq!(e.type_name(), "ExpertActivated");
        assert_eq!(e.metadata().source, "gea-activator");
    }

    #[test]
    fn test_week4_event_gather_completed() {
        let e = NexusEvent::GatherCompleted {
            metadata: EventMetadata::new("gqep-executor"),
            total: 10,
            succeeded: 8,
            failed: 2,
            latency_ms: 50.0,
        };
        assert_eq!(e.type_name(), "GatherCompleted");
        assert_eq!(e.severity(), EventSeverity::Normal);
    }

    #[test]
    fn test_week4_event_serialization() {
        let e = NexusEvent::CachePrefetched {
            metadata: EventMetadata::new("scc-cache"),
            prefetched_ids: vec!["ctx-1".into(), "ctx-2".into()],
        };
        let json = serde_json::to_string(&e).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, restored);
    }

    // ============================================================
    // Week 5 扩展测试(SubTask 37.1):验证新增 8 个事件变体 +
    // ThinkingModeSwitched 扩展字段的行为
    // ============================================================

    // --- severity() 正确性测试 ---

    #[test]
    fn test_week5_event_critical_severity() {
        // SkepticVeto 行使否决权,Critical
        let skeptic_veto = NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q-1".into(),
            veto_reason: "unsafe shell injection".into(),
            frozen_capabilities: vec!["shell_exec".into()],
        };
        assert_eq!(skeptic_veto.severity(), EventSeverity::Critical);

        // RedTeamAudit 红队审计发现漏洞,Critical
        let red_team = NexusEvent::RedTeamAudit {
            metadata: EventMetadata::new("parliament"),
            vulnerability_type: "prompt_injection".into(),
            failed_probes: 5,
            total_probes: 20,
            detection_rate: 0.25,
            remediation_suggestion: "add input sanitization".into(),
        };
        assert_eq!(red_team.severity(), EventSeverity::Critical);
    }

    #[test]
    fn test_week5_event_normal_severity() {
        let meta = EventMetadata::new("test-source");
        let debate = NexusEvent::DebateStarted {
            metadata: meta.clone(),
            quest_id: "q-1".into(),
            proposal_id: "p-1".into(),
            participant_count: 5,
        };
        assert_eq!(debate.severity(), EventSeverity::Normal);

        let budget_adj = NexusEvent::BudgetAdjusted {
            metadata: meta.clone(),
            quest_id: "q-1".into(),
            old_tier: "High".into(),
            new_tier: "Medium".into(),
            coefficient: 0.5,
            reason: "consumption > 0.8".into(),
        };
        assert_eq!(budget_adj.severity(), EventSeverity::Normal);

        let asa = NexusEvent::AsaIntervention {
            metadata: meta.clone(),
            operation_id: "op-1".into(),
            action: "Block".into(),
            safety_score: 0.2,
            block_reason: Some("unsafe".into()),
            alternative_suggestion: None,
        };
        // WHY:AsaIntervention 即使 action=Block 也返回 Normal,
        // 因为 severity() 是同步函数不依赖运行时值。
        // Block 级别应通过 Critical 通道发送(由发布者负责)。
        assert_eq!(asa.severity(), EventSeverity::Normal);

        let ahirt = NexusEvent::AhirtProbeCompleted {
            metadata: meta.clone(),
            probe_type: "prompt_injection".into(),
            total: 20,
            passed: 15,
            failed: 5,
            detection_rate: 0.25,
        };
        assert_eq!(ahirt.severity(), EventSeverity::Normal);

        let role = NexusEvent::RoleRegistered {
            metadata: meta.clone(),
            role_id: "visionary-01".into(),
            role_name: "Visionary".into(),
            voting_weight: 0.4,
        };
        assert_eq!(role.severity(), EventSeverity::Normal);

        let stats = NexusEvent::BudgetStatsReported {
            metadata: meta,
            total_consumption: 5000.0,
            remaining_budget: 5000.0,
            utilization_rate: 0.5,
        };
        assert_eq!(stats.severity(), EventSeverity::Normal);
    }

    // --- type_name() 正确性测试 ---

    #[test]
    fn test_week5_event_type_names() {
        let meta = EventMetadata::new("test");
        assert_eq!(
            NexusEvent::DebateStarted {
                metadata: meta.clone(),
                quest_id: "q".into(),
                proposal_id: "p".into(),
                participant_count: 1,
            }
            .type_name(),
            "DebateStarted"
        );
        assert_eq!(
            NexusEvent::SkepticVeto {
                metadata: meta.clone(),
                quest_id: "q".into(),
                veto_reason: "r".into(),
                frozen_capabilities: vec![],
            }
            .type_name(),
            "SkepticVeto"
        );
        assert_eq!(
            NexusEvent::RedTeamAudit {
                metadata: meta.clone(),
                vulnerability_type: "t".into(),
                failed_probes: 0,
                total_probes: 0,
                detection_rate: 0.0,
                remediation_suggestion: "s".into(),
            }
            .type_name(),
            "RedTeamAudit"
        );
        assert_eq!(
            NexusEvent::BudgetAdjusted {
                metadata: meta.clone(),
                quest_id: "q".into(),
                old_tier: "H".into(),
                new_tier: "M".into(),
                coefficient: 1.0,
                reason: "r".into(),
            }
            .type_name(),
            "BudgetAdjusted"
        );
        assert_eq!(
            NexusEvent::AsaIntervention {
                metadata: meta.clone(),
                operation_id: "o".into(),
                action: "Allow".into(),
                safety_score: 1.0,
                block_reason: None,
                alternative_suggestion: None,
            }
            .type_name(),
            "AsaIntervention"
        );
        assert_eq!(
            NexusEvent::AhirtProbeCompleted {
                metadata: meta.clone(),
                probe_type: "t".into(),
                total: 0,
                passed: 0,
                failed: 0,
                detection_rate: 0.0,
            }
            .type_name(),
            "AhirtProbeCompleted"
        );
        assert_eq!(
            NexusEvent::RoleRegistered {
                metadata: meta.clone(),
                role_id: "r".into(),
                role_name: "n".into(),
                voting_weight: 1.0,
            }
            .type_name(),
            "RoleRegistered"
        );
        assert_eq!(
            NexusEvent::BudgetStatsReported {
                metadata: meta,
                total_consumption: 0.0,
                remaining_budget: 0.0,
                utilization_rate: 0.0,
            }
            .type_name(),
            "BudgetStatsReported"
        );
    }

    // --- 序列化 round-trip 测试(每个新变体) ---

    #[test]
    fn test_week5_event_debate_started_serialization() {
        let e = NexusEvent::DebateStarted {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q-1".into(),
            proposal_id: "p-1".into(),
            participant_count: 5,
        };
        let json = serde_json::to_string(&e).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, restored);
    }

    #[test]
    fn test_week5_event_skeptic_veto_serialization() {
        let e = NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q-1".into(),
            veto_reason: "unsafe shell injection".into(),
            frozen_capabilities: vec!["shell_exec".into(), "fs_write".into()],
        };
        let json = serde_json::to_string(&e).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, restored);
    }

    #[test]
    fn test_week5_event_red_team_audit_serialization() {
        let e = NexusEvent::RedTeamAudit {
            metadata: EventMetadata::new("parliament"),
            vulnerability_type: "prompt_injection".into(),
            failed_probes: 5,
            total_probes: 20,
            detection_rate: 0.25,
            remediation_suggestion: "add input sanitization".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, restored);
    }

    #[test]
    fn test_week5_event_budget_adjusted_serialization() {
        let e = NexusEvent::BudgetAdjusted {
            metadata: EventMetadata::new("decb-governor"),
            quest_id: "q-1".into(),
            old_tier: "High".into(),
            new_tier: "Medium".into(),
            coefficient: 0.5,
            reason: "consumption > 0.8".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, restored);
    }

    #[test]
    fn test_week5_event_asa_intervention_serialization() {
        // 测试 Block 场景(带 block_reason 和 alternative_suggestion)
        let e_block = NexusEvent::AsaIntervention {
            metadata: EventMetadata::new("seccore"),
            operation_id: "op-1".into(),
            action: "Block".into(),
            safety_score: 0.2,
            block_reason: Some("unsafe operation".into()),
            alternative_suggestion: Some("use sandboxed tool".into()),
        };
        let json = serde_json::to_string(&e_block).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e_block, restored);

        // 测试 Allow 场景(block_reason 和 alternative_suggestion 为 None)
        let e_allow = NexusEvent::AsaIntervention {
            metadata: EventMetadata::new("seccore"),
            operation_id: "op-2".into(),
            action: "Allow".into(),
            safety_score: 0.95,
            block_reason: None,
            alternative_suggestion: None,
        };
        let json = serde_json::to_string(&e_allow).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e_allow, restored);
    }

    #[test]
    fn test_week5_event_ahirt_probe_completed_serialization() {
        let e = NexusEvent::AhirtProbeCompleted {
            metadata: EventMetadata::new("parliament"),
            probe_type: "tool_abuse".into(),
            total: 100,
            passed: 95,
            failed: 5,
            detection_rate: 0.05,
        };
        let json = serde_json::to_string(&e).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, restored);
    }

    #[test]
    fn test_week5_event_role_registered_serialization() {
        let e = NexusEvent::RoleRegistered {
            metadata: EventMetadata::new("parliament"),
            role_id: "skeptic-01".into(),
            role_name: "Skeptic".into(),
            voting_weight: 0.3,
        };
        let json = serde_json::to_string(&e).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, restored);
    }

    #[test]
    fn test_week5_event_budget_stats_reported_serialization() {
        let e = NexusEvent::BudgetStatsReported {
            metadata: EventMetadata::new("decb-governor"),
            total_consumption: 7500.0,
            remaining_budget: 2500.0,
            utilization_rate: 0.75,
        };
        let json = serde_json::to_string(&e).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, restored);
    }

    // --- ThinkingModeSwitched 扩展字段测试 ---

    #[test]
    fn test_week5_thinking_mode_switched_with_reason() {
        let e = NexusEvent::ThinkingModeSwitched {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-1".into(),
            from_mode: "fast".into(),
            to_mode: "deep".into(),
            reason: "complexity threshold exceeded".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, restored);
        assert_eq!(e.type_name(), "ThinkingModeSwitched");
        assert_eq!(e.severity(), EventSeverity::Normal);
    }

    #[test]
    fn test_week5_thinking_mode_switched_backward_compat() {
        // WHY:旧格式数据(无 reason 字段)必须能反序列化为新结构,
        // reason 字段通过 #[serde(default)] 填充为空字符串。
        // 这确保 Week 1/2 已序列化的 ThinkingModeSwitched 数据
        // 仍能被 Week 5 的新消费者正确读取。
        let old_json = r#"{"type":"ThinkingModeSwitched","data":{"metadata":{"event_id":"01901234-5678-7abc-def0-123456789abc","timestamp":"2025-01-01T00:00:00Z","source":"quest-engine"},"quest_id":"q-1","from_mode":"fast","to_mode":"deep"}}"#;
        let restored: NexusEvent = serde_json::from_str(old_json).unwrap();
        match restored {
            NexusEvent::ThinkingModeSwitched {
                quest_id,
                from_mode,
                to_mode,
                reason,
                ..
            } => {
                assert_eq!(quest_id, "q-1");
                assert_eq!(from_mode, "fast");
                assert_eq!(to_mode, "deep");
                // 旧格式数据无 reason 字段,反序列化为空字符串
                assert_eq!(reason, "");
            }
            _ => panic!("expected ThinkingModeSwitched variant"),
        }
    }

    // ============================================================
    // Week 6 扩展测试:验证 NmcEncoded 事件变体的行为
    // ============================================================

    #[test]
    fn test_week6_event_nmc_encoded_normal_severity() {
        let e = NexusEvent::NmcEncoded {
            metadata: EventMetadata::new("nmc-encoder"),
            modality: "Text".into(),
            content_hash: "abc123".into(),
            clv_dimension: 512,
        };
        assert_eq!(e.severity(), EventSeverity::Normal);
        assert_eq!(e.type_name(), "NmcEncoded");
        assert_eq!(e.metadata().source, "nmc-encoder");
    }

    #[test]
    fn test_week6_event_nmc_encoded_serialization() {
        let e = NexusEvent::NmcEncoded {
            metadata: EventMetadata::new("nmc-encoder"),
            modality: "Desktop".into(),
            content_hash: "deadbeef".into(),
            clv_dimension: 512,
        };
        let json = serde_json::to_string(&e).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, restored);
    }

    #[test]
    fn test_week6_event_nmc_encoded_msgpack_roundtrip() {
        let e = NexusEvent::NmcEncoded {
            metadata: EventMetadata::new("nmc-encoder"),
            modality: "Image".into(),
            content_hash: "cafebabe".into(),
            clv_dimension: 512,
        };
        let bytes = crate::bus::serialize_msgpack(&e).unwrap();
        let decoded = crate::bus::deserialize_msgpack(&bytes).unwrap();
        assert_eq!(e, decoded);
    }

    // ============================================================
    // F-001 回归测试:验证 BudgetExceeded severity == Critical
    // Hard Constraint 第 10 条:BudgetExceeded 必须标记为 Critical
    // WHY:预算耗尽是系统红线,若被通配符误判为 Normal,在背压场景下
    // 可能被丢弃,导致预算超限无人响应、Quest 持续消耗资源直至 OOM。
    // 此测试守护 severity() 显式分支,防止未来重构时意外回退。
    // ============================================================

    #[test]
    fn test_budget_exceeded_severity_is_critical() {
        let e = NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("decb-governor"),
            budget_type: "token".into(),
            current: 10_000,
            limit: 8_000,
        };
        assert_eq!(
            e.severity(),
            EventSeverity::Critical,
            "BudgetExceeded 必须为 Critical (Hard Constraint 第 10 条)"
        );
        assert_eq!(e.type_name(), "BudgetExceeded");
    }

    // ============================================================
    // P1-3 扩展测试:验证 VetoOverridden 事件变体
    // ============================================================

    #[test]
    fn test_veto_overridden_severity_is_critical() {
        let e = NexusEvent::VetoOverridden {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q-1".into(),
            proposal_id: "p-1".into(),
            veto_reason: "command_injection detected".into(),
            override_reason: "false positive: legitimate shell script".into(),
            override_by: "admin:alice".into(),
        };
        assert_eq!(
            e.severity(),
            EventSeverity::Critical,
            "VetoOverridden 必须为 Critical(否决覆盖审计)"
        );
        assert_eq!(e.type_name(), "VetoOverridden");
        assert_eq!(e.metadata().source, "parliament");
    }

    #[test]
    fn test_veto_overridden_serialization_roundtrip() {
        let e = NexusEvent::VetoOverridden {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q-1".into(),
            proposal_id: "p-1".into(),
            veto_reason: "Skeptic 否决:DataExfiltration 'curl'".into(),
            override_reason: "legitimate API call to github.com".into(),
            override_by: "system:auto-review".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, restored);
    }

    #[test]
    fn test_veto_overridden_msgpack_roundtrip() {
        let e = NexusEvent::VetoOverridden {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q-2".into(),
            proposal_id: "p-2".into(),
            veto_reason: "sandbox_escape /proc/".into(),
            override_reason: "monitoring use case".into(),
            override_by: "admin:bob".into(),
        };
        let bytes = crate::bus::serialize_msgpack(&e).unwrap();
        let decoded = crate::bus::deserialize_msgpack(&bytes).unwrap();
        assert_eq!(e, decoded);
    }

    // ============================================================
    // Task 11 (N16): ChainExhausted 事件测试
    // ============================================================

    #[test]
    fn test_chain_exhausted_has_warning_severity() {
        let e = NexusEvent::ChainExhausted {
            metadata: EventMetadata::new("csn-substitutor"),
            chain_id: "cap-shell-exec".into(),
            last_error: "降级链已耗尽(共 3 级)".into(),
        };
        assert_eq!(
            e.severity(),
            EventSeverity::Warning,
            "ChainExhausted 应为 Warning 级别(既不是 Critical 也不是 Normal)"
        );
        assert_eq!(e.type_name(), "ChainExhausted");
        assert_eq!(e.metadata().source, "csn-substitutor");
    }

    #[test]
    fn test_chain_exhausted_serialization_roundtrip() {
        let e = NexusEvent::ChainExhausted {
            metadata: EventMetadata::new("csn-substitutor"),
            chain_id: "fs-write".into(),
            last_error: "降级链已耗尽: fs-write(共 4 级)".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, restored);
    }

    #[test]
    fn test_chain_exhausted_msgpack_roundtrip() {
        let e = NexusEvent::ChainExhausted {
            metadata: EventMetadata::new("csn-substitutor"),
            chain_id: "shell-exec".into(),
            last_error: "降级链已耗尽: shell-exec(共 3 级)".into(),
        };
        let bytes = crate::bus::serialize_msgpack(&e).unwrap();
        let decoded = crate::bus::deserialize_msgpack(&bytes).unwrap();
        assert_eq!(e, decoded);
    }
}
