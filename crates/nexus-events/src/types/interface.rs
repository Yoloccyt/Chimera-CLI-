//! NexusEvent 变体分组 · 观测上报 / Agent 协作 / TUI 交互
//!
//! 本文件承载 `NexusEvent` 的连续一段变体（声明序 `DecayMetricsReported` … `TuiHelloAck`），
//! 经 `macro_rules!` 链式装配进 [`NexusEvent`](crate::types::NexusEvent)。
//!
//! 域: 衰减、路由统计、MCP 心跳、CLV 快照等观测上报;CHIMERA-MAS Agent 协作事件;
//! TUI v3 交互式动作协议与握手。
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

macro_rules! variants_interface {
    ($($acc:tt)*) => {
        crate::types::evolution::variants_evolution! {
            $($acc)*

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
                /// 斜杠命令名(预留字段):当前 TUI 侧不填充(恒为 None),chat 内以
                /// `/` 开头的输入按普通对话处理(B4-S F-57 契约收窄;生产代码无读取
                /// 点,恢复填充需同步编排器消费与两端测试,消除只写不读的半途契约)
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
        }
    };
}
pub(crate) use variants_interface;
