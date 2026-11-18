//! NexusEvent 变体分组 · 议会治理 / 预算 / 控制面
//!
//! 本文件承载 `NexusEvent` 的连续一段变体（声明序 `DebateStarted` … `QuestPriorityAdjusted`），
//! 经 `macro_rules!` 链式装配进 [`NexusEvent`](crate::types::NexusEvent)。
//!
//! 域: L8 议会审议与否决、预算治理、Week 6 融合与适配器状态、TUI 双向控制请求,
//! 以及衰减指标与三路由器统计聚合。
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

macro_rules! variants_governance {
    ($($acc:tt)*) => {
        crate::types::interface::variants_interface! {
            $($acc)*

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
        }
    };
}
pub(crate) use variants_governance;
