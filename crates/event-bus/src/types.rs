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
    // (与现有 85 个变体保持一致),使 metadata() 方法能统一返回 &EventMetadata。
    // severity 分配:仅 AgentTaskFailed 为 Critical(任务失败可能影响
    // Quest 完整性),其余 6 个为 Normal(由通配符覆盖)。
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

    /// 流式会话闭环 — mca-gateway → acb-governor/auto-dpo
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
}

impl NexusEvent {
    /// 获取事件元数据引用
    ///
    /// 委托给子枚举的 `EventClassification::metadata()` 实现。
    pub fn metadata(&self) -> &EventMetadata {
        match self {
            Self::UserIntentEncoded { metadata, .. } => metadata,
            Self::NexusStateChanged { metadata, .. } => metadata,
            Self::ModelRouteSelected { metadata, .. } => metadata,
            Self::SlowConsumerDropped { metadata, .. } => metadata,
            Self::AuditLogged { metadata, .. } => metadata,
            Self::MemoryMetricsReported { metadata, .. } => metadata,
            Self::MemoryTiered { metadata, .. } => metadata,
            Self::ContextWindowSwitched { metadata, .. } => metadata,
            Self::ContextCompressed { metadata, .. } => metadata,
            Self::NmcEncoded { metadata, .. } => metadata,
            Self::ClvSnapshotReported { metadata, .. } => metadata,
            Self::CacheHit { metadata, .. } => metadata,
            Self::CacheMiss { metadata, .. } => metadata,
            Self::CapabilityTiered { metadata, .. } => metadata,
            Self::CachePrefetched { metadata, .. } => metadata,
            Self::CacheStatsReported { metadata, .. } => metadata,
            Self::LsctTierSwitched { metadata, .. } => metadata,
            Self::SandboxViolation { metadata, .. } => metadata,
            Self::CapabilityFrozen { metadata, .. } => metadata,
            Self::BudgetExceeded { metadata, .. } => metadata,
            Self::BudgetAdjusted { metadata, .. } => metadata,
            Self::AsaIntervention { metadata, .. } => metadata,
            Self::BudgetStatsReported { metadata, .. } => metadata,
            Self::BudgetMetricsUpdated { metadata, .. } => metadata,
            Self::OmniSparseMasksComputed { metadata, .. } => metadata,
            Self::ToolsRouted { metadata, .. } => metadata,
            Self::ExpertActivated { metadata, .. } => metadata,
            Self::ActivationThresholdAdjusted { metadata, .. } => metadata,
            Self::ActivationCacheStats { metadata, .. } => metadata,
            Self::ExpertRouted { metadata, .. } => metadata,
            Self::EntropyBalanced { metadata, .. } => metadata,
            Self::ExpertRegistered { metadata, .. } => metadata,
            Self::ExpertUnregistered { metadata, .. } => metadata,
            Self::BlocksRebalanced { metadata, .. } => metadata,
            Self::SesaActivationCompleted { metadata, .. } => metadata,
            Self::OperationProduced { metadata, .. } => metadata,
            Self::PredictionVerified { metadata, .. } => metadata,
            Self::ExecutionCompleted { metadata, .. } => metadata,
            Self::GatherCompleted { metadata, .. } => metadata,
            Self::OperationTimedOut { metadata, .. } => metadata,
            Self::GatherTimedOut { metadata, .. } => metadata,
            Self::OrphanCallDetected { metadata, .. } => metadata,
            Self::ProducerStrategyAdjusted { metadata, .. } => metadata,
            Self::PredictionMade { metadata, .. } => metadata,
            Self::PredictionStatsReported { metadata, .. } => metadata,
            Self::PredictionRolledBack { metadata, .. } => metadata,
            Self::QuestCreated { metadata, .. } => metadata,
            Self::QuestProgressUpdated { metadata, .. } => metadata,
            Self::QuestListUpdated { metadata, .. } => metadata,
            Self::QuestCompleted { metadata, .. } => metadata,
            Self::ThinkingModeSwitched { metadata, .. } => metadata,
            Self::CheckpointSaved { metadata, .. } => metadata,
            Self::CheckpointLoaded { metadata, .. } => metadata,
            Self::ConsensusReached { metadata, .. } => metadata,
            Self::VoteCast { metadata, .. } => metadata,
            Self::DebateStarted { metadata, .. } => metadata,
            Self::SkepticVeto { metadata, .. } => metadata,
            Self::VetoOverridden { metadata, .. } => metadata,
            Self::RedTeamAudit { metadata, .. } => metadata,
            Self::AhirtProbeCompleted { metadata, .. } => metadata,
            Self::RoleRegistered { metadata, .. } => metadata,
            Self::QuestPauseRequested { metadata, .. } => metadata,
            Self::QuestResumeRequested { metadata, .. } => metadata,
            Self::VoteCastRequested { metadata, .. } => metadata,
            Self::QuestPaused { metadata, .. } => metadata,
            Self::QuestResumed { metadata, .. } => metadata,
            Self::QuestCancelRequested { metadata, .. } => metadata,
            Self::QuestCancelled { metadata, .. } => metadata,
            Self::QuestPriorityChanged { metadata, .. } => metadata,
            Self::QuestPriorityAdjusted { metadata, .. } => metadata,
            Self::R1ShadowRegressionDetected { metadata, .. } => metadata,
            Self::R1ShadowPromotionReady { metadata, .. } => metadata,
            Self::R1ShadowRollbackFailed { metadata, .. } => metadata,
            Self::SsraFusionCompleted { metadata, .. } => metadata,
            Self::GsoePolicyUpdated { metadata, .. } => metadata,
            Self::McpMessageReceived { metadata, .. } => metadata,
            Self::ChtcToolCallReceived { metadata, .. } => metadata,
            Self::McpMeshTransactionCompleted { metadata, .. } => metadata,
            Self::CsnSubstitutionTriggered { metadata, .. } => metadata,
            Self::EfficiencyAlertTriggered { metadata, .. } => metadata,
            Self::DecayMetricsReported { metadata, .. } => metadata,
            Self::RouterStatsReported { metadata, .. } => metadata,
            Self::McpNodeHeartbeat { metadata, .. } => metadata,
            Self::ChtcAdapterStatus { metadata, .. } => metadata,
            Self::AgentTaskDelegated { metadata, .. } => metadata,
            Self::AgentTaskCompleted { metadata, .. } => metadata,
            Self::AgentTaskFailed { metadata, .. } => metadata,
            Self::AgentConsultRequested { metadata, .. } => metadata,
            Self::AgentConsultResponded { metadata, .. } => metadata,
            Self::AgentHeartbeat { metadata, .. } => metadata,
            Self::AgentContextOverflow { metadata, .. } => metadata,
            Self::TuiActionRequested { metadata, .. } => metadata,
            Self::TuiActionProgressed { metadata, .. } => metadata,
            Self::TuiActionCompleted { metadata, .. } => metadata,
            Self::TuiActionFailed { metadata, .. } => metadata,
            Self::TuiChatSubmitted { metadata, .. } => metadata,
            Self::TuiChatResponseChunk { metadata, .. } => metadata,
            Self::TuiChatCompleted { metadata, .. } => metadata,
            Self::TuiChatStatusChanged { metadata, .. } => metadata,
            Self::RefreshStateRequested { metadata, .. } => metadata,
            Self::SpecRegistered { metadata, .. } => metadata,
            Self::R2FreezeViolation { metadata, .. } => metadata,
            Self::R2FreezeRollbackFailed { metadata, .. } => metadata,
            // P2-1: 协调成本/推理增益比值报告
            Self::CoordinationRatioReported { metadata, .. } => metadata,
            // polish-v2.7 P1-2: RuntimeAuditor 审计事件(2 个新变体)
            Self::AuditFindingRaised { metadata, .. } => metadata,
            Self::HarnessReportGenerated { metadata, .. } => metadata,
            // L8 协调度量接线闭环:议会审议完成 + 委托批次完成(2 个新变体)
            Self::DebateCompleted { metadata, .. } => metadata,
            Self::DelegationCompleted { metadata, .. } => metadata,
            // L8 推理悖论风控:策略封顶变更
            Self::ParliamentStrategyCapChanged { metadata, .. } => metadata,
            Self::WikiUpdated { metadata, .. } => metadata,
            Self::EvolutionTriggered { metadata, .. } => metadata,
            Self::DpoPairGenerated { metadata, .. } => metadata,
            // MCA M0(ADR-065):mca-gateway 会话级/治理级事件(6 个新变体)
            Self::ModelAffinitySelected { metadata, .. } => metadata,
            // MCA P2-1:跨厂商辩论通道选择
            Self::CrossVendorNegotiation { metadata, .. } => metadata,
            Self::ProviderDegraded { metadata, .. } => metadata,
            Self::AffinityCapabilityNegotiated { metadata, .. } => metadata,
            Self::AffinityQuotaExhausted { metadata, .. } => metadata,
            Self::AffinityUnknownField { metadata, .. } => metadata,
            Self::StreamSessionCompleted { metadata, .. } => metadata,
            // MCA P5:窗口亲和折减结果(观测面事件,不影响系统状态投递)
            Self::WindowAffinityApplied { metadata, .. } => metadata,
            // MCA A3:缓存亲和策略应用结果(观测面事件,不影响系统状态投递)
            Self::CacheAffinityApplied { metadata, .. } => metadata,
            // ADR-069: Token 效率优化事件(观测面)
            Self::ContextBudgetAllocated { metadata, .. } => metadata,
            Self::SemanticCacheHit { metadata, .. } => metadata,
            // P2-8 MemCon:幽灵记忆检测事件(观测面事件,不影响系统状态投递)
            Self::GhostMemoryDetected { metadata, .. } => metadata,
            // P2-8 MemCon:策略调整事件(观测面事件,不影响系统状态投递)
            Self::MemConStrategyAdjusted { metadata, .. } => metadata,
            Self::BenchmarkMetricsCollected { metadata, .. } => metadata,
            // PROBE P0:HCW 召回评测事件(观测面,metadata 一致)
            Self::HcwRecallReported { metadata, .. } => metadata,
            Self::HcwRecallDegraded { metadata, .. } => metadata,
            Self::OverWindowFallbackTriggered { metadata, .. } => metadata,
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
            | Self::BudgetExceeded { .. }
            // CHIMERA-MAS:AgentTaskFailed 为 Critical(Task 4,ADR-026)
            // WHY:任务失败可能影响 Quest 完整性,必须保证投递到 SecCore 与
            // Parliament 进行补救决策。丢失会导致失败无人响应、Quest 持续等待已死 Agent 结果。
            | Self::AgentTaskFailed { .. }
            // P1-W2.1.4:AsaIntervention 统一标记为 Critical(对齐 spec.md L186 红线)
            // WHY 历史:原设计认为 severity() 是同步函数不应依赖运行时值(action
            // 字段),故走通配符返回 Normal。但 spec.md L186 与 §6.2 红线均将
            // AsaIntervention 列为 6 个 Critical 事件之一(W1.2 TDD 暴露偏差)。
            // 修复策略:统一返回 Critical,无论 action 是 Allow/Warn/Block。
            // 保守策略确保所有 ASA 安全干预走 Critical 通道,Allow/Warn 为低频
            // 事件(每个安全操作最多一个干预),不会产生大量 Critical 事件。
            // Block 级别更需 Critical 投递保证(丢失导致高风险操作继续执行)。
            | Self::AsaIntervention { .. }
            // P4-W16.2.2 步骤 5:R1 影子模式回滚失败为 Critical
            // WHY:回滚失败意味着退化策略可能仍在生效,必须保证投递到 SecCore
            // 与 Parliament 进行紧急干预。丢失导致 Quest 持续受退化策略影响。
            | Self::R1ShadowRollbackFailed { .. }
            // ADR-042 决策 4:R2 冻结违反 + 回滚失败为 Critical
            // WHY:R2 违反等同于安全事件(奖励黑客风险立即生效),回滚失败意味着
            // R2 路径代码可能仍在生效。必须走 mpsc 旁路通道确保投递,对齐 §6.2 红线 5。
            | Self::R2FreezeViolation { .. }
            | Self::R2FreezeRollbackFailed { .. }
            // MCA M0(ADR-065):厂商额度耗尽为 Critical
            // WHY:额度耗尽 = 通道即刻不可用,丢失导致降级链(csn-substitutor)
            // 无人触发、请求持续打向死通道。语义对齐 BudgetExceeded(资源红线
            // 必须确保投递);bus.rs is_critical_mpsc_event() 已同步列入(双清单)。
            | Self::AffinityQuotaExhausted { .. } => EventSeverity::Critical,
            // 控制事件(请求/反馈):不阻断系统,不触发 mpsc 旁路投递
            Self::QuestCancelRequested { .. }
            | Self::QuestCancelled { .. }
            | Self::QuestPriorityChanged { .. }
            | Self::QuestPriorityAdjusted { .. }
            // TUI 交互式动作协议(ADR-029):请求/终态为 Info,高频流式为 Normal
            | Self::TuiActionRequested { .. }
            | Self::TuiActionCompleted { .. }
            | Self::TuiActionFailed { .. }
            | Self::TuiChatSubmitted { .. }
            | Self::TuiChatCompleted { .. } => EventSeverity::Info,
            // TuiActionProgressed / TuiChatResponseChunk / TuiChatStatusChanged
            // 为高频流式事件,走 Normal(broadcast),由通配符分支覆盖
            // MCA P5:窗口亲和折减结果 + MCA A3:缓存亲和策略应用结果 + MCA M0:跨厂商协商(均为观测面事件)
            // WHY Normal:CrossVendorNegotiation 记录 PVL 辩论中的跨厂商去相关决策,
            // 同 WindowAffinityApplied 等观测面事件,仅用于审计与监控留痕,
            // 不阻塞系统关键路径,无需 mpsc 旁路投递。
            Self::WindowAffinityApplied { .. }
            | Self::CacheAffinityApplied { .. }
            | Self::CrossVendorNegotiation { .. }
            // P2-8 MemCon:幽灵记忆检测与策略调整(均为观测面事件,不阻断系统)
            | Self::GhostMemoryDetected { .. }
            | Self::MemConStrategyAdjusted { .. }
            | Self::BenchmarkMetricsCollected { .. } => EventSeverity::Normal,
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
            Self::QuestListUpdated { .. } => "QuestListUpdated",
            Self::QuestCompleted { .. } => "QuestCompleted",
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
            Self::BudgetMetricsUpdated { .. } => "BudgetMetricsUpdated",
            Self::NmcEncoded { .. } => "NmcEncoded",
            Self::ChtcToolCallReceived { .. } => "ChtcToolCallReceived",
            Self::SsraFusionCompleted { .. } => "SsraFusionCompleted",
            Self::GsoePolicyUpdated { .. } => "GsoePolicyUpdated",
            Self::LsctTierSwitched { .. } => "LsctTierSwitched",
            Self::McpMeshTransactionCompleted { .. } => "McpMeshTransactionCompleted",
            Self::CsnSubstitutionTriggered { .. } => "CsnSubstitutionTriggered",
            Self::SesaActivationCompleted { .. } => "SesaActivationCompleted",
            Self::EfficiencyAlertTriggered { .. } => "EfficiencyAlertTriggered",
            Self::QuestPauseRequested { .. } => "QuestPauseRequested",
            Self::QuestResumeRequested { .. } => "QuestResumeRequested",
            Self::VoteCastRequested { .. } => "VoteCastRequested",
            Self::RefreshStateRequested { .. } => "RefreshStateRequested",
            Self::QuestPaused { .. } => "QuestPaused",
            Self::QuestResumed { .. } => "QuestResumed",
            Self::DecayMetricsReported { .. } => "DecayMetricsReported",
            Self::RouterStatsReported { .. } => "RouterStatsReported",
            Self::McpNodeHeartbeat { .. } => "McpNodeHeartbeat",
            Self::ChtcAdapterStatus { .. } => "ChtcAdapterStatus",
            Self::ClvSnapshotReported { .. } => "ClvSnapshotReported",
            Self::QuestCancelRequested { .. } => "QuestCancelRequested",
            Self::QuestCancelled { .. } => "QuestCancelled",
            Self::QuestPriorityChanged { .. } => "QuestPriorityChanged",
            Self::QuestPriorityAdjusted { .. } => "QuestPriorityAdjusted",
            // CHIMERA-MAS Agent 事件(Task 4,ADR-026)
            Self::AgentTaskDelegated { .. } => "AgentTaskDelegated",
            Self::AgentTaskCompleted { .. } => "AgentTaskCompleted",
            Self::AgentTaskFailed { .. } => "AgentTaskFailed",
            Self::AgentConsultRequested { .. } => "AgentConsultRequested",
            Self::AgentConsultResponded { .. } => "AgentConsultResponded",
            Self::AgentHeartbeat { .. } => "AgentHeartbeat",
            Self::AgentContextOverflow { .. } => "AgentContextOverflow",
            // TUI 交互式动作协议(ADR-029)
            Self::TuiActionRequested { .. } => "TuiActionRequested",
            Self::TuiActionProgressed { .. } => "TuiActionProgressed",
            Self::TuiActionCompleted { .. } => "TuiActionCompleted",
            Self::TuiActionFailed { .. } => "TuiActionFailed",
            Self::TuiChatSubmitted { .. } => "TuiChatSubmitted",
            Self::TuiChatResponseChunk { .. } => "TuiChatResponseChunk",
            Self::TuiChatCompleted { .. } => "TuiChatCompleted",
            Self::TuiChatStatusChanged { .. } => "TuiChatStatusChanged",
            // P4-W16.2.2 步骤 5:R1 影子模式事件（3 个新变体）
            Self::R1ShadowRegressionDetected { .. } => "R1ShadowRegressionDetected",
            Self::R1ShadowPromotionReady { .. } => "R1ShadowPromotionReady",
            Self::R1ShadowRollbackFailed { .. } => "R1ShadowRollbackFailed",
            // P5.2.3: SpecRegistered 事件
            Self::SpecRegistered { .. } => "SpecRegistered",
            // ADR-042 决策 4:R2 冻结违反处置事件(2 个新变体)
            Self::R2FreezeViolation { .. } => "R2FreezeViolation",
            Self::R2FreezeRollbackFailed { .. } => "R2FreezeRollbackFailed",
            // P2-1: 协调成本/推理增益比值报告(三重悖论推理悖论红线度量)
            Self::CoordinationRatioReported { .. } => "CoordinationRatioReported",
            // polish-v2.7 P1-2: RuntimeAuditor 审计事件(2 个新变体)
            Self::AuditFindingRaised { .. } => "AuditFindingRaised",
            Self::HarnessReportGenerated { .. } => "HarnessReportGenerated",
            // L8 协调度量接线闭环:观测事件(2 个新变体,Normal 级走通配符)
            Self::DebateCompleted { .. } => "DebateCompleted",
            Self::DelegationCompleted { .. } => "DelegationCompleted",
            // L8 推理悖论风控:策略封顶变更(Normal 级走通配符)
            Self::ParliamentStrategyCapChanged { .. } => "ParliamentStrategyCapChanged",
            // MCA M0(ADR-065):mca-gateway 事件(6 个新变体,仅 AffinityQuotaExhausted 为 Critical)
            Self::ModelAffinitySelected { .. } => "ModelAffinitySelected",
            // MCA P2-1:跨厂商辩论通道选择
            Self::CrossVendorNegotiation { .. } => "CrossVendorNegotiation",
            Self::ProviderDegraded { .. } => "ProviderDegraded",
            Self::AffinityCapabilityNegotiated { .. } => "AffinityCapabilityNegotiated",
            Self::AffinityQuotaExhausted { .. } => "AffinityQuotaExhausted",
            Self::AffinityUnknownField { .. } => "AffinityUnknownField",
            Self::StreamSessionCompleted { .. } => "StreamSessionCompleted",
            // MCA P5:窗口亲和折减结果
            Self::WindowAffinityApplied { .. } => "WindowAffinityApplied",
            // MCA A3:缓存亲和策略应用结果
            Self::CacheAffinityApplied { .. } => "CacheAffinityApplied",
            // ADR-069: Token 效率优化事件
            Self::ContextBudgetAllocated { .. } => "ContextBudgetAllocated",
            Self::SemanticCacheHit { .. } => "SemanticCacheHit",
            // P2-8 MemCon:幽灵记忆检测
            Self::GhostMemoryDetected { .. } => "GhostMemoryDetected",
            // P2-8 MemCon:策略调整
            Self::MemConStrategyAdjusted { .. } => "MemConStrategyAdjusted",
            Self::BenchmarkMetricsCollected { .. } => "BenchmarkMetricsCollected",
            // PROBE P0:HCW 召回评测事件
            Self::HcwRecallReported { .. } => "HcwRecallReported",
            Self::HcwRecallDegraded { .. } => "HcwRecallDegraded",
            Self::OverWindowFallbackTriggered { .. } => "OverWindowFallbackTriggered",
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

    /// ADR-029:TUI 交互式动作协议事件的 severity 分级验证
    ///
    /// 请求/终态(Requested/Completed/Failed/ChatSubmitted/ChatCompleted)为 Info;
    /// 高频流式(Progressed/ResponseChunk/StatusChanged)为 Normal——
    /// 确保高频事件不占用仅为稀有安全告警保留的 mpsc 旁路通道。
    #[test]
    fn test_tui_action_protocol_severity() {
        let requested = NexusEvent::TuiActionRequested {
            metadata: EventMetadata::new("chimera-tui"),
            action_id: "quest.pause".into(),
            payload: "{\"quest_id\":\"q1\"}".into(),
            source: ActionSource::Palette,
        };
        assert_eq!(requested.severity(), EventSeverity::Info);

        let chunk = NexusEvent::TuiChatResponseChunk {
            metadata: EventMetadata::new("chimera-cli"),
            session_id: "s1".into(),
            delta: "hello".into(),
            cursor_hint: 0,
        };
        assert_eq!(
            chunk.severity(),
            EventSeverity::Normal,
            "高频 token 流必须为 Normal,避免冲垮 mpsc 旁路"
        );

        let submitted = NexusEvent::TuiChatSubmitted {
            metadata: EventMetadata::new("chimera-tui"),
            session_id: "s1".into(),
            query: "实现登录".into(),
            slash_command: None,
        };
        assert_eq!(submitted.severity(), EventSeverity::Info);
    }

    /// ADR-029:新增事件的 type_name 稳定性与 metadata 可取性验证
    #[test]
    fn test_tui_action_protocol_type_name_and_metadata() {
        let events = [
            NexusEvent::TuiActionRequested {
                metadata: EventMetadata::new("chimera-tui"),
                action_id: "a".into(),
                payload: "{}".into(),
                source: ActionSource::Chat,
            },
            NexusEvent::TuiActionProgressed {
                metadata: EventMetadata::new("chimera-cli"),
                action_id: "a".into(),
                delta: "d".into(),
            },
            NexusEvent::TuiChatStatusChanged {
                metadata: EventMetadata::new("chimera-cli"),
                session_id: "s".into(),
                status: ChatStatus::Thinking,
            },
        ];
        // metadata() 对所有新变体可取,source 非空;type_name 以 "Tui" 前缀一致
        for e in &events {
            assert!(!e.metadata().source.is_empty());
            assert!(e.type_name().starts_with("Tui"));
        }
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
        // P1-W2.1.4 修复:AsaIntervention 统一返回 Critical(对齐 spec.md L186 红线)。
        // 历史设计曾返回 Normal,W1.2 TDD 测试暴露 spec/code 偏差后修复。
        // 详见 severity() 方法中 AsaIntervention 分支注释。
        assert_eq!(asa.severity(), EventSeverity::Critical);

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
    // M4 扩展测试:验证 TUI 双向控制事件
    // ============================================================

    #[test]
    fn test_m4_control_events_normal_severity() {
        let meta = EventMetadata::new("chimera-tui");
        let pause = NexusEvent::QuestPauseRequested {
            metadata: meta.clone(),
            quest_id: "q-1".into(),
            requested_by: "operator".into(),
        };
        let resume = NexusEvent::QuestResumeRequested {
            metadata: meta.clone(),
            quest_id: "q-1".into(),
            requested_by: "operator".into(),
        };
        let vote = NexusEvent::VoteCastRequested {
            metadata: meta.clone(),
            proposal_id: "p-1".into(),
            voter: "operator".into(),
            vote: VoteValue::Abstain,
        };
        let refresh = NexusEvent::RefreshStateRequested {
            metadata: meta,
            requested_by: "operator".into(),
        };

        for e in [pause, resume, vote, refresh] {
            assert_eq!(e.severity(), EventSeverity::Normal);
        }
    }

    #[test]
    fn test_m4_control_events_type_names() {
        let meta = EventMetadata::new("chimera-tui");
        assert_eq!(
            NexusEvent::QuestPauseRequested {
                metadata: meta.clone(),
                quest_id: "q-1".into(),
                requested_by: "operator".into(),
            }
            .type_name(),
            "QuestPauseRequested"
        );
        assert_eq!(
            NexusEvent::QuestResumeRequested {
                metadata: meta.clone(),
                quest_id: "q-1".into(),
                requested_by: "operator".into(),
            }
            .type_name(),
            "QuestResumeRequested"
        );
        assert_eq!(
            NexusEvent::VoteCastRequested {
                metadata: meta.clone(),
                proposal_id: "p-1".into(),
                voter: "operator".into(),
                vote: VoteValue::Yes,
            }
            .type_name(),
            "VoteCastRequested"
        );
        assert_eq!(
            NexusEvent::RefreshStateRequested {
                metadata: meta.clone(),
                requested_by: "operator".into(),
            }
            .type_name(),
            "RefreshStateRequested"
        );
        assert_eq!(
            NexusEvent::QuestPaused {
                metadata: meta.clone(),
                quest_id: "q-1".into(),
                requested_by: "operator".into(),
            }
            .type_name(),
            "QuestPaused"
        );
        assert_eq!(
            NexusEvent::QuestResumed {
                metadata: meta,
                quest_id: "q-1".into(),
                requested_by: "operator".into(),
            }
            .type_name(),
            "QuestResumed"
        );
    }

    #[test]
    fn test_m4_control_events_serialization_roundtrip() {
        let cases = vec![
            NexusEvent::QuestPauseRequested {
                metadata: EventMetadata::new("chimera-tui"),
                quest_id: "q-1".into(),
                requested_by: "operator".into(),
            },
            NexusEvent::QuestResumeRequested {
                metadata: EventMetadata::new("chimera-tui"),
                quest_id: "q-2".into(),
                requested_by: "operator".into(),
            },
            NexusEvent::VoteCastRequested {
                metadata: EventMetadata::new("chimera-tui"),
                proposal_id: "p-1".into(),
                voter: "operator".into(),
                vote: VoteValue::No,
            },
            NexusEvent::RefreshStateRequested {
                metadata: EventMetadata::new("chimera-tui"),
                requested_by: "operator".into(),
            },
            NexusEvent::QuestPaused {
                metadata: EventMetadata::new("quest-engine"),
                quest_id: "q-1".into(),
                requested_by: "operator".into(),
            },
            NexusEvent::QuestResumed {
                metadata: EventMetadata::new("quest-engine"),
                quest_id: "q-1".into(),
                requested_by: "operator".into(),
            },
        ];

        for e in cases {
            let json = serde_json::to_string(&e).unwrap();
            let restored: NexusEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(e, restored);
        }
    }

    #[test]
    fn test_m4_vote_value_serialization() {
        for value in [VoteValue::Yes, VoteValue::No, VoteValue::Abstain] {
            let json = serde_json::to_string(&value).unwrap();
            let restored: VoteValue = serde_json::from_str(&json).unwrap();
            assert_eq!(value, restored);
        }
    }

    // ============================================================
    // TUI v1.8-omega 扩展测试:验证 ClvSnapshotReported 事件变体
    // ============================================================

    #[test]
    fn test_clv_snapshot_reported_normal_severity() {
        let summary = ClvSummary {
            block_means: vec![0.1; 8],
            l2_norm: 2.5,
            top_dims: vec![(0, 0.8), (64, 0.6)],
        };
        let e = NexusEvent::ClvSnapshotReported {
            metadata: EventMetadata::new("nmc-encoder"),
            modality: "Text".into(),
            content_hash: "abc123".into(),
            clv_summary: summary,
        };
        assert_eq!(e.severity(), EventSeverity::Normal);
        assert_eq!(e.type_name(), "ClvSnapshotReported");
        assert_eq!(e.metadata().source, "nmc-encoder");
    }

    #[test]
    fn test_clv_snapshot_reported_serialization_json() {
        let summary = ClvSummary {
            block_means: vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7],
            l2_norm: 1.234,
            top_dims: vec![(0, 0.9), (128, 0.7), (256, 0.5)],
        };
        let e = NexusEvent::ClvSnapshotReported {
            metadata: EventMetadata::new("nmc-encoder"),
            modality: "Image".into(),
            content_hash: "deadbeef".into(),
            clv_summary: summary,
        };
        let json = serde_json::to_string(&e).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, restored);
    }

    #[test]
    fn test_clv_snapshot_reported_msgpack_roundtrip() {
        let summary = ClvSummary {
            block_means: vec![-0.5; 8],
            l2_norm: 0.0,
            top_dims: vec![],
        };
        let e = NexusEvent::ClvSnapshotReported {
            metadata: EventMetadata::new("nmc-encoder"),
            modality: "Audio".into(),
            content_hash: "cafebabe".into(),
            clv_summary: summary,
        };
        let bytes = crate::bus::serialize_msgpack(&e).unwrap();
        let decoded = crate::bus::deserialize_msgpack(&bytes).unwrap();
        assert_eq!(e, decoded);
    }

    #[test]
    fn test_clv_summary_partial_eq() {
        let s1 = ClvSummary {
            block_means: vec![0.1; 8],
            l2_norm: 1.0,
            top_dims: vec![(1, 0.5)],
        };
        let s2 = ClvSummary {
            block_means: vec![0.1; 8],
            l2_norm: 1.0,
            top_dims: vec![(1, 0.5)],
        };
        assert_eq!(s1, s2);

        let s3 = ClvSummary {
            block_means: vec![0.2; 8],
            l2_norm: 1.0,
            top_dims: vec![(1, 0.5)],
        };
        assert_ne!(s1, s3);
    }

    #[test]
    fn test_clv_snapshot_reported_metadata_extraction() {
        let summary = ClvSummary {
            block_means: vec![0.0; 8],
            l2_norm: 0.0,
            top_dims: vec![],
        };
        let metadata = EventMetadata::new("test-source");
        let expected_id = metadata.event_id;
        let e = NexusEvent::ClvSnapshotReported {
            metadata,
            modality: "Text".into(),
            content_hash: "test".into(),
            clv_summary: summary,
        };
        assert_eq!(e.metadata().event_id, expected_id);
        assert_eq!(e.metadata().source, "test-source");
    }

    // ============================================================
    // TUI v1.8-omega: ClvSummary::from_clv 计算方法测试
    // ============================================================

    #[test]
    fn test_clv_summary_from_clv_zero_vector() {
        // 零向量:l2_norm = 0.0, block_means 全 0, top_dims 空
        let clv = nexus_core::clv::CLV::zero();
        let summary = ClvSummary::from_clv(&clv);
        assert_eq!(summary.block_means.len(), 8);
        assert!(summary.block_means.iter().all(|&v| v == 0.0));
        assert_eq!(summary.l2_norm, 0.0);
        assert!(summary.top_dims.is_empty());
    }

    #[test]
    fn test_clv_summary_from_clv_uniform_vector() {
        // 均匀向量(全 1.0):所有分块均值 = 1.0, l2_norm = sqrt(512) ≈ 22.63
        let v = vec![1.0_f32; 512];
        let clv = nexus_core::clv::CLV::from_vec(v).unwrap();
        let summary = ClvSummary::from_clv(&clv);
        assert_eq!(summary.block_means.len(), 8);
        assert!(summary.block_means.iter().all(|&m| (m - 1.0).abs() < 1e-5));
        let expected_norm = (512.0_f32).sqrt();
        assert!((summary.l2_norm - expected_norm).abs() < 1e-3);
        // Top-8: 所有维度值相同,取前 8 个(索引 0-7)
        assert_eq!(summary.top_dims.len(), 8);
        // 所有 |值| = 1.0,排序后前 8 个任意,但值都应为 1.0
        assert!(summary
            .top_dims
            .iter()
            .all(|&(_, v)| (v - 1.0).abs() < 1e-5));
    }

    #[test]
    fn test_clv_summary_from_clv_known_vector() {
        // 已知向量:前 64 维 = 2.0,其余 = 0.0
        // block_means[0] = 2.0, block_means[1..8] = 0.0
        // l2_norm = sqrt(64 * 4) = sqrt(256) = 16.0
        // top_dims: 前 8 个应是维度 0-7(值 2.0)
        let mut v = vec![0.0_f32; 512];
        for val in v.iter_mut().take(64) {
            *val = 2.0;
        }
        let clv = nexus_core::clv::CLV::from_vec(v).unwrap();
        let summary = ClvSummary::from_clv(&clv);
        assert!((summary.block_means[0] - 2.0).abs() < 1e-5);
        for i in 1..8 {
            assert!((summary.block_means[i] - 0.0).abs() < 1e-5);
        }
        assert!((summary.l2_norm - 16.0).abs() < 1e-3);
        assert_eq!(summary.top_dims.len(), 8);
        // 前 8 个应是维度 0-7(值 2.0)
        assert!(summary
            .top_dims
            .iter()
            .all(|&(_, v)| (v - 2.0).abs() < 1e-5));
    }

    #[test]
    fn test_clv_summary_from_clv_block_means_length() {
        // 验证 block_means 长度始终为 8
        let clv = nexus_core::clv::CLV::zero();
        let summary = ClvSummary::from_clv(&clv);
        assert_eq!(summary.block_means.len(), 8);
    }

    #[test]
    fn test_clv_summary_from_clv_top_dims_sorted_desc() {
        // 验证 top_dims 按 |值| 降序排列
        let mut v = vec![0.0_f32; 512];
        v[0] = 5.0; // |5.0|
        v[1] = 3.0; // |3.0|
        v[2] = -4.0; // |4.0|
        v[3] = 1.0; // |1.0|
        v[4] = -2.0; // |2.0|
        let clv = nexus_core::clv::CLV::from_vec(v).unwrap();
        let summary = ClvSummary::from_clv(&clv);
        assert!(!summary.top_dims.is_empty());
        // 验证降序:|v[0]| >= |v[1]| >= ...
        for i in 1..summary.top_dims.len() {
            let prev_abs = summary.top_dims[i - 1].1.abs();
            let curr_abs = summary.top_dims[i].1.abs();
            assert!(
                prev_abs >= curr_abs || (prev_abs - curr_abs).abs() < 1e-5,
                "top_dims not sorted desc: |{}| < |{}|",
                prev_abs,
                curr_abs
            );
        }
        // 第一个应是维度 0(值 5.0)
        assert_eq!(summary.top_dims[0].0, 0);
    }

    #[test]
    fn test_clv_summary_from_clv_negative_values() {
        // 负值向量:验证 |值| 正确排序
        let mut v = vec![0.0_f32; 512];
        v[0] = -5.0; // |-5.0| = 5.0
        v[1] = 3.0; // |3.0| = 3.0
        let clv = nexus_core::clv::CLV::from_vec(v).unwrap();
        let summary = ClvSummary::from_clv(&clv);
        // 第一个应是维度 0(值 -5.0,|值|最大)
        assert_eq!(summary.top_dims[0].0, 0);
        assert!((summary.top_dims[0].1 - (-5.0)).abs() < 1e-5);
        // 第二个应是维度 1(值 3.0)
        assert_eq!(summary.top_dims[1].0, 1);
    }

    // ============================================================
    // P2-13: R1ShadowRollbackFailed 结构化理由记录测试
    // ============================================================

    /// 验证 `RollbackTriggerType` 默认值为 Unknown
    ///
    /// WHY 默认 Unknown:确保未显式设置 trigger_type 的旧版本事件反序列化后
    /// 不会误归类为某一具体触发条件。
    #[test]
    fn test_p2_13_rollback_trigger_type_default_is_unknown() {
        let default_trigger: RollbackTriggerType = Default::default();
        assert_eq!(default_trigger, RollbackTriggerType::Unknown);
    }

    /// 验证 `RollbackTriggerType::description()` 返回非空人类可读描述
    ///
    /// 每个变体应有唯一的描述字符串,用于日志与 TUI 展示。
    #[test]
    fn test_p2_13_rollback_trigger_type_description() {
        let cases = [
            (
                RollbackTriggerType::ConsecutiveRegression,
                "R1 significantly worse than L3 for 3 consecutive days",
            ),
            (
                RollbackTriggerType::AsaIntervention,
                "ASA intervention triggered on R1 seam",
            ),
            (
                RollbackTriggerType::EwmaCollapse,
                "EWMA collapsed by >=0.3 within 24h",
            ),
            (
                RollbackTriggerType::RecallRateDrop,
                "Recall rate dropped >=5% vs L3 baseline",
            ),
            (RollbackTriggerType::Unknown, "Unknown rollback trigger"),
        ];
        for (trigger, expected_desc) in cases {
            assert_eq!(
                trigger.description(),
                expected_desc,
                "RollbackTriggerType::{:?} description mismatch",
                trigger
            );
        }
    }

    /// 验证 `RollbackTriggerType` 序列化为 snake_case(ADR-043 决策 4 对齐)
    ///
    /// WHY snake_case:对齐 Rust serde 惯例与 JSON 字段命名规范,
    /// 便于审计日志解析与 efficiency-monitor 告警规则匹配。
    #[test]
    fn test_p2_13_rollback_trigger_type_serialization_snake_case() {
        let cases = [
            (
                RollbackTriggerType::ConsecutiveRegression,
                "consecutive_regression",
            ),
            (RollbackTriggerType::AsaIntervention, "asa_intervention"),
            (RollbackTriggerType::EwmaCollapse, "ewma_collapse"),
            (RollbackTriggerType::RecallRateDrop, "recall_rate_drop"),
            (RollbackTriggerType::Unknown, "unknown"),
        ];
        for (trigger, expected_json) in cases {
            let json = serde_json::to_string(&trigger).unwrap();
            assert_eq!(json, format!("\"{}\"", expected_json));
            let restored: RollbackTriggerType = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, trigger);
        }
    }

    /// 验证 `RollbackDiagnosticContext::default()` 所有字段为 None
    #[test]
    fn test_p2_13_rollback_diagnostic_context_default_all_none() {
        let ctx = RollbackDiagnosticContext::default();
        assert_eq!(ctx.ewma_level, None);
        assert_eq!(ctx.observation_days, None);
        assert_eq!(ctx.regression_streak, None);
        assert_eq!(ctx.recall_rate_drop, None);
        assert_eq!(ctx.rollback_target_version, None);
    }

    /// 验证 `RollbackDiagnosticContext` builder 模式正确设置字段
    ///
    /// builder 模式用于 R1 回滚失败时构造诊断快照,便于专家团队复盘根因。
    #[test]
    fn test_p2_13_rollback_diagnostic_context_builder() {
        let ctx = RollbackDiagnosticContext::empty()
            .with_ewma_level(0.35)
            .with_observation_days(7)
            .with_regression_streak(3)
            .with_recall_rate_drop(0.08)
            .with_rollback_target_version(42);
        assert_eq!(ctx.ewma_level, Some(0.35));
        assert_eq!(ctx.observation_days, Some(7));
        assert_eq!(ctx.regression_streak, Some(3));
        assert_eq!(ctx.recall_rate_drop, Some(0.08));
        assert_eq!(ctx.rollback_target_version, Some(42));
    }

    /// 验证 `RollbackDiagnosticContext` 序列化/反序列化往返一致
    #[test]
    fn test_p2_13_rollback_diagnostic_context_serialization() {
        let ctx = RollbackDiagnosticContext::empty()
            .with_ewma_level(0.42)
            .with_observation_days(14)
            .with_regression_streak(5);
        let json = serde_json::to_string(&ctx).unwrap();
        let restored: RollbackDiagnosticContext = serde_json::from_str(&json).unwrap();
        assert_eq!(ctx, restored);
    }

    /// 验证 `R1ShadowRollbackFailed` 事件可构造且新字段正确(P2-13 结构化字段)
    ///
    /// 覆盖完整字段构造,模拟真实回滚失败场景:
    /// - trigger_type = EwmaCollapse(EWMA 24h 内下降 ≥ 0.3)
    /// - triggered_at = 精确时间戳
    /// - details = CapabilityTokenRegistry 内部错误消息
    /// - diagnostic = EWMA 水平 0.35 + 观察期 7 天
    #[test]
    fn test_p2_13_r1_shadow_rollback_failed_with_structured_fields() {
        let triggered_at = chrono::Utc::now();
        let diagnostic = RollbackDiagnosticContext::empty()
            .with_ewma_level(0.35)
            .with_observation_days(7);
        let event = NexusEvent::R1ShadowRollbackFailed {
            metadata: EventMetadata::new("omega-learner"),
            reason: "EWMA collapsed from 0.7 to 0.35 within 24h".to_string(),
            trigger_type: RollbackTriggerType::EwmaCollapse,
            triggered_at: Some(triggered_at),
            details: "CapabilityTokenRegistry::trigger_asa_intervention failed: internal error"
                .to_string(),
            diagnostic,
        };
        assert_eq!(event.severity(), EventSeverity::Critical);
        // 使用模式匹配解构枚举变体字段
        match &event {
            NexusEvent::R1ShadowRollbackFailed {
                trigger_type,
                triggered_at: ta,
                details,
                diagnostic,
                ..
            } => {
                assert_eq!(*trigger_type, RollbackTriggerType::EwmaCollapse);
                assert_eq!(*ta, Some(triggered_at));
                assert!(details.contains("CapabilityTokenRegistry"));
                assert_eq!(diagnostic.ewma_level, Some(0.35));
                assert_eq!(diagnostic.observation_days, Some(7));
            }
            other => panic!(
                "Expected R1ShadowRollbackFailed, got {:?}",
                other.type_name()
            ),
        }
    }

    /// 验证 `R1ShadowRollbackFailed` 事件序列化/反序列化往返一致
    ///
    /// 确保结构化字段(trigger_type / triggered_at / details / diagnostic)
    /// 在 JSON 序列化后能完整恢复。
    #[test]
    fn test_p2_13_r1_shadow_rollback_failed_serialization() {
        let triggered_at = chrono::Utc::now();
        let diagnostic = RollbackDiagnosticContext::empty()
            .with_regression_streak(3)
            .with_rollback_target_version(10);
        let event = NexusEvent::R1ShadowRollbackFailed {
            metadata: EventMetadata::new("omega-learner"),
            reason: "ConsecutiveRegression detected".to_string(),
            trigger_type: RollbackTriggerType::ConsecutiveRegression,
            triggered_at: Some(triggered_at),
            details: "R1 worse than L3 for 3 consecutive days".to_string(),
            diagnostic,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, restored);
    }

    /// 验证向后兼容性:旧格式 JSON(仅有 reason 字段)能被反序列化
    ///
    /// P2-13 之前的事件只有 `metadata` + `reason` 字段。新增的 4 个字段
    /// (trigger_type / triggered_at / details / diagnostic)都有 `#[serde(default)]`,
    /// 确保旧格式 JSON 能被反序列化为默认值。
    ///
    /// 这是 SemVer minor 兼容性的关键验证。
    ///
    /// NOTE: `NexusEvent` 使用 `#[serde(tag = "type", content = "data")]` internally
    /// tagged 表示,JSON 格式为 `{"type": "VariantName", "data": {fields}}`。
    #[test]
    fn test_p2_13_r1_shadow_rollback_failed_backward_compatibility() {
        // 模拟旧格式 JSON(无 trigger_type / triggered_at / details / diagnostic 字段)
        let old_json = r#"{
            "type": "R1ShadowRollbackFailed",
            "data": {
                "metadata": {
                    "event_id": "550e8400-e29b-41d4-a716-446655440000",
                    "source": "omega-learner",
                    "timestamp": "2026-07-25T10:00:00Z"
                },
                "reason": "ConsecutiveRegression"
            }
        }"#;
        let restored: NexusEvent = serde_json::from_str(old_json).unwrap();
        match restored {
            NexusEvent::R1ShadowRollbackFailed {
                reason,
                trigger_type,
                triggered_at,
                details,
                diagnostic,
                ..
            } => {
                assert_eq!(reason, "ConsecutiveRegression");
                // 新字段应有默认值
                assert_eq!(trigger_type, RollbackTriggerType::Unknown);
                assert_eq!(triggered_at, None);
                assert_eq!(details, "");
                assert_eq!(diagnostic, RollbackDiagnosticContext::default());
            }
            other => panic!(
                "Expected R1ShadowRollbackFailed, got {:?}",
                other.type_name()
            ),
        }
    }

    /// 验证 `R1ShadowRollbackFailed` 的 type_name 稳定性(序列化兼容性)
    ///
    /// type_name 必须保持 "R1ShadowRollbackFailed",不允许因 P2-13 扩展而变更,
    /// 否则会破坏 efficiency-monitor 的告警规则匹配与 TUI 事件分类。
    #[test]
    fn test_p2_13_r1_shadow_rollback_failed_type_name_stable() {
        let event = NexusEvent::R1ShadowRollbackFailed {
            metadata: EventMetadata::new("test"),
            reason: "test".to_string(),
            trigger_type: RollbackTriggerType::Unknown,
            triggered_at: None,
            details: String::new(),
            diagnostic: RollbackDiagnosticContext::default(),
        };
        assert_eq!(event.type_name(), "R1ShadowRollbackFailed");
    }

    // ============================================================
    // P2-1 后续增强:CoordinationRatioReported 事件测试
    // ============================================================

    /// 验证 `CoordinationRatioReported` 的 type_name 稳定性
    ///
    /// type_name 必须保持 "CoordinationRatioReported",不允许变更,
    /// 否则会破坏 efficiency-monitor 的告警规则匹配与 TUI 事件分类。
    #[test]
    fn test_p2_1_coordination_ratio_reported_type_name_stable() {
        let event = NexusEvent::CoordinationRatioReported {
            metadata: EventMetadata::new("quest-engine"),
            coordination_cost_ms: 500.0,
            inference_gain: 0.8,
            cost_index: 0.5,
            gain_index: 0.8,
            ratio: 0.625,
            is_paradox_risk: false,
            threshold: 1.0,
            sample_count: 10,
        };
        assert_eq!(event.type_name(), "CoordinationRatioReported");
    }

    /// 验证 `CoordinationRatioReported` 的 metadata 可取性
    ///
    /// metadata.source 必须与构造时传入的 "quest-engine" 一致,
    /// 确保事件溯源信息不丢失。
    #[test]
    fn test_p2_1_coordination_ratio_reported_metadata_accessible() {
        let event = NexusEvent::CoordinationRatioReported {
            metadata: EventMetadata::new("quest-engine"),
            coordination_cost_ms: 500.0,
            inference_gain: 0.8,
            cost_index: 0.5,
            gain_index: 0.8,
            ratio: 0.625,
            is_paradox_risk: false,
            threshold: 1.0,
            sample_count: 10,
        };
        assert_eq!(event.metadata().source, "quest-engine");
    }

    /// 验证 `CoordinationRatioReported` 为 Normal 严重级别
    ///
    /// WHY Normal:这是周期性指标报告,非阻断性事件。推理悖论风险告警
    /// 由 efficiency-monitor 订阅后通过 EfficiencyAlertTriggered 二次发布,
    /// 不走 mpsc 旁路通道(§6.2 红线 5 仅适用于 Critical 安全事件)。
    #[test]
    fn test_p2_1_coordination_ratio_reported_severity_normal() {
        let event = NexusEvent::CoordinationRatioReported {
            metadata: EventMetadata::new("quest-engine"),
            coordination_cost_ms: 1000.0,
            inference_gain: 0.1,
            cost_index: 1.0,
            gain_index: 0.1,
            ratio: 10.0,
            is_paradox_risk: true, // 即使触发推理悖论风险,事件本身仍为 Normal
            threshold: 1.0,
            sample_count: 5,
        };
        assert_eq!(
            event.severity(),
            crate::EventSeverity::Normal,
            "CoordinationRatioReported 必须为 Normal,告警由订阅者处理"
        );
    }

    /// 验证 `CoordinationRatioReported` 归入 Quest 主题
    ///
    /// 该事件由 L9 quest-engine 发布,归入 Quest 主题组,
    /// 与 ThinkingModeSwitched / QuestCompleted 等同级。
    #[test]
    fn test_p2_1_coordination_ratio_reported_topic_quest() {
        let event = NexusEvent::CoordinationRatioReported {
            metadata: EventMetadata::new("quest-engine"),
            coordination_cost_ms: 300.0,
            inference_gain: 0.9,
            cost_index: 0.3,
            gain_index: 0.9,
            ratio: 0.333,
            is_paradox_risk: false,
            threshold: 1.0,
            sample_count: 1,
        };
        assert_eq!(
            event.topic(),
            crate::topic::EventTopic::Quest,
            "CoordinationRatioReported 应归入 Quest 主题"
        );
    }

    /// 验证 `CoordinationRatioReported` 的序列化/反序列化往返
    ///
    /// 确保事件的 serde tag="type" content="data" 格式正确,
    /// 且所有字段(包括 f64 的 ratio / INFINITY 边界)都能正确往返。
    #[test]
    fn test_p2_1_coordination_ratio_reported_serialization_roundtrip() {
        let event = NexusEvent::CoordinationRatioReported {
            metadata: EventMetadata::new("quest-engine"),
            coordination_cost_ms: 750.0,
            inference_gain: 0.65,
            cost_index: 0.75,
            gain_index: 0.65,
            ratio: 1.153846,
            is_paradox_risk: true,
            threshold: 1.0,
            sample_count: 42,
        };
        let json = serde_json::to_string(&event).expect("序列化失败");
        assert!(
            json.contains("CoordinationRatioReported"),
            "JSON 应包含 type tag: {json}"
        );
        let decoded: NexusEvent = serde_json::from_str(&json).expect("反序列化失败");
        match decoded {
            NexusEvent::CoordinationRatioReported {
                coordination_cost_ms,
                inference_gain,
                cost_index,
                gain_index,
                ratio,
                is_paradox_risk,
                threshold,
                sample_count,
                ..
            } => {
                assert!((coordination_cost_ms - 750.0).abs() < 1e-6);
                assert!((inference_gain - 0.65).abs() < 1e-6);
                assert!((cost_index - 0.75).abs() < 1e-6);
                assert!((gain_index - 0.65).abs() < 1e-6);
                assert!((ratio - 1.153846).abs() < 1e-6);
                assert!(is_paradox_risk);
                assert!((threshold - 1.0).abs() < 1e-6);
                assert_eq!(sample_count, 42);
            }
            other => panic!(
                "Expected CoordinationRatioReported, got {:?}",
                other.type_name()
            ),
        }
    }

    /// 验证 `CoordinationRatioReported` 能承载 INFINITY ratio(增益为零的边界)
    ///
    /// 当 gain_index = 0.0 时 ratio = f64::INFINITY,必须能正确构造与访问。
    /// 这是推理悖论的极端场景:有协调成本但无推理增益。
    ///
    /// WHY 不测试 JSON 序列化往返:serde_json 将 f64::INFINITY 序列化为 null,
    /// 反序列化时 null 无法还原为 f64::INFINITY(JSON 规范不支持 Infinity)。
    /// 生产环境使用 MessagePack(rmp-serde,ADR-004)序列化,支持 INFINITY。
    /// 此处仅验证事件构造与字段访问,序列化兼容性由 MessagePack 保证。
    #[test]
    fn test_p2_1_coordination_ratio_reported_infinity_ratio() {
        let event = NexusEvent::CoordinationRatioReported {
            metadata: EventMetadata::new("quest-engine"),
            coordination_cost_ms: 500.0,
            inference_gain: 0.0,
            cost_index: 0.5,
            gain_index: 0.0,
            ratio: f64::INFINITY,
            is_paradox_risk: true,
            threshold: 1.0,
            sample_count: 3,
        };
        // 验证事件可正确构造与字段访问
        match event {
            NexusEvent::CoordinationRatioReported {
                ratio,
                is_paradox_risk,
                gain_index,
                ..
            } => {
                assert!(ratio.is_infinite(), "ratio 应为 INFINITY");
                assert!(ratio.is_sign_positive(), "ratio 应为正无穷");
                assert!(is_paradox_risk, "增益为零时必然触发推理悖论风险");
                assert_eq!(gain_index, 0.0);
            }
            _ => panic!("Expected CoordinationRatioReported"),
        }
    }

    // ============================================================
    // L8 协调度量接线闭环:DebateCompleted / DelegationCompleted 事件测试
    // ============================================================

    /// 构造测试用 DebateCompleted 事件(Full 策略共识达成场景)
    fn make_debate_completed() -> NexusEvent {
        NexusEvent::DebateCompleted {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q-1".into(),
            proposal_id: "p-1".into(),
            debate_latency_ms: 42.5,
            strategy: "full".into(),
            weighted_approval_rate: Some(0.85),
            participation_rate: Some(1.0),
            divergence: Some(0.2),
            abstention_rate: Some(0.1),
            consensus_margin: Some(0.25),
            outcome: "Reached".into(),
        }
    }

    /// 构造测试用 DelegationCompleted 事件(4 子任务 3 成功场景)
    fn make_delegation_completed() -> NexusEvent {
        NexusEvent::DelegationCompleted {
            metadata: EventMetadata::new("chimera-mas:DelegationExecutor"),
            parent_id: "root-1".into(),
            quest_id: Some("q-1".into()),
            total_overhead_ms: 120.0,
            sub_task_count: 4,
            success_count: 3,
        }
    }

    /// 验证两个新观测事件的 type_name 稳定性与 metadata 可取性
    ///
    /// type_name 不允许变更,否则会破坏 quest-engine 订阅器的事件匹配
    /// 与 TUI 事件分类(同 CoordinationRatioReported 稳定性要求)。
    #[test]
    fn test_debate_delegation_completed_type_name_and_metadata() {
        let debate = make_debate_completed();
        assert_eq!(debate.type_name(), "DebateCompleted");
        assert_eq!(debate.metadata().source, "parliament");

        let delegation = make_delegation_completed();
        assert_eq!(delegation.type_name(), "DelegationCompleted");
        assert_eq!(
            delegation.metadata().source,
            "chimera-mas:DelegationExecutor"
        );
    }

    /// 验证两个新观测事件为 Normal 严重级别
    ///
    /// WHY Normal:它们是只读延迟/质量观测事件,丢失仅影响单次度量样本
    /// (Option 字段保持 None,EWMA 不阻塞),不影响共识/安全决策,
    /// 不得占用仅为稀有安全告警保留的 mpsc 旁路通道(§6.2 红线 5)。
    #[test]
    fn test_debate_delegation_completed_severity_normal() {
        assert_eq!(make_debate_completed().severity(), EventSeverity::Normal);
        assert_eq!(
            make_delegation_completed().severity(),
            EventSeverity::Normal
        );
    }

    /// 验证两个新观测事件的主题归类
    ///
    /// DebateCompleted 归 Parliament(与 DebateStarted/ConsensusReached 同组),
    /// DelegationCompleted 归 Agent(与 AgentTaskCompleted 同组),
    /// 使订阅者按主题过滤即可获取完整生命周期事件。
    #[test]
    fn test_debate_delegation_completed_topic() {
        assert_eq!(
            make_debate_completed().topic(),
            crate::topic::EventTopic::Parliament
        );
        assert_eq!(
            make_delegation_completed().topic(),
            crate::topic::EventTopic::Agent
        );
    }

    /// 验证 DebateCompleted 的序列化/反序列化往返(含 Option 字段两态)
    #[test]
    fn test_debate_completed_serialization_roundtrip() {
        // 态 1:有投票数据(Simplified/Full 路径)
        let json = serde_json::to_string(&make_debate_completed()).expect("序列化失败");
        assert!(
            json.contains("DebateCompleted"),
            "JSON 应含 type tag: {json}"
        );
        let decoded: NexusEvent = serde_json::from_str(&json).expect("反序列化失败");
        match decoded {
            NexusEvent::DebateCompleted {
                quest_id,
                debate_latency_ms,
                strategy,
                weighted_approval_rate,
                participation_rate,
                divergence,
                abstention_rate,
                consensus_margin,
                outcome,
                ..
            } => {
                assert_eq!(quest_id, "q-1");
                assert!((debate_latency_ms - 42.5).abs() < 1e-6);
                assert_eq!(strategy, "full");
                assert!((weighted_approval_rate.expect("应有赞成率") - 0.85).abs() < 1e-6);
                assert!((participation_rate.expect("应有参与率") - 1.0).abs() < 1e-6);
                // M2-T2.2:多维质量字段往返
                assert!((divergence.expect("应有分歧度") - 0.2).abs() < 1e-6);
                assert!((abstention_rate.expect("应有弃权率") - 0.1).abs() < 1e-6);
                assert!((consensus_margin.expect("应有共识裕度") - 0.25).abs() < 1e-6);
                assert_eq!(outcome, "Reached");
            }
            other => panic!("Expected DebateCompleted, got {:?}", other.type_name()),
        }

        // 态 2:无投票数据(FastPath/Vetoed 路径,Option 字段为 None)
        let vetoed = NexusEvent::DebateCompleted {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q-2".into(),
            proposal_id: "p-2".into(),
            debate_latency_ms: 3.2,
            strategy: "fast-path".into(),
            weighted_approval_rate: None,
            participation_rate: None,
            divergence: None,
            abstention_rate: None,
            consensus_margin: None,
            outcome: "Vetoed".into(),
        };
        let json = serde_json::to_string(&vetoed).expect("序列化失败");
        let decoded: NexusEvent = serde_json::from_str(&json).expect("反序列化失败");
        match decoded {
            NexusEvent::DebateCompleted {
                weighted_approval_rate,
                participation_rate,
                divergence,
                outcome,
                ..
            } => {
                assert!(weighted_approval_rate.is_none(), "否决路径无投票数据");
                assert!(participation_rate.is_none());
                assert!(divergence.is_none(), "无投票路径多维质量也为 None");
                assert_eq!(outcome, "Vetoed");
            }
            _ => panic!("Expected DebateCompleted"),
        }
    }

    /// 验证 DelegationCompleted 的序列化/反序列化往返
    #[test]
    fn test_delegation_completed_serialization_roundtrip() {
        let json = serde_json::to_string(&make_delegation_completed()).expect("序列化失败");
        assert!(
            json.contains("DelegationCompleted"),
            "JSON 应含 type tag: {json}"
        );
        let decoded: NexusEvent = serde_json::from_str(&json).expect("反序列化失败");
        match decoded {
            NexusEvent::DelegationCompleted {
                parent_id,
                quest_id,
                total_overhead_ms,
                sub_task_count,
                success_count,
                ..
            } => {
                assert_eq!(parent_id, "root-1");
                assert_eq!(quest_id.as_deref(), Some("q-1"));
                assert!((total_overhead_ms - 120.0).abs() < 1e-6);
                assert_eq!(sub_task_count, 4);
                assert_eq!(success_count, 3);
            }
            other => panic!("Expected DelegationCompleted, got {:?}", other.type_name()),
        }
    }

    /// 验证 BenchmarkMetricsCollected 为 Normal 级别（基准模式观测面事件）
    #[test]
    fn test_benchmark_metrics_collected_severity_normal() {
        let event = NexusEvent::BenchmarkMetricsCollected {
            metadata: EventMetadata::new("efficiency-monitor"),
            equivalent_input_cost_micro: 1250,
            vendor_cache_hit_rate_percent: 66,
            semantic_cache_hit_rate_percent: 40,
            ttft_p95_ms: 320,
            total_output_tokens: 5000,
            task_success_rate_percent: 95,
            per_vendor_snapshot_json: "{}".into(),
        };
        assert_eq!(
            event.severity(),
            EventSeverity::Normal,
            "基准指标采集必须为 Normal（观测面事件，不阻断系统）"
        );
        assert_eq!(event.type_name(), "BenchmarkMetricsCollected");
        assert!(!event.metadata().source.is_empty());
    }
}
