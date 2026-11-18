//! NexusEvent 变体分组 · 进化治理 / 协调度量
//!
//! 本文件承载 `NexusEvent` 的连续一段变体（声明序 `R1ShadowRegressionDetected` … `ParliamentStrategyCapChanged`），
//! 经 `macro_rules!` 链式装配进 [`NexusEvent`](crate::types::NexusEvent)。
//!
//! 域: R1 影子模式与 R2 冻结治理(含回滚失败)、Spec 注册、协调成本度量、
//! RuntimeAuditor 审计、审议与委托闭环、推理悖论策略封顶。
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

macro_rules! variants_evolution {
    ($($acc:tt)*) => {
        crate::types::runtime::variants_runtime! {
            $($acc)*

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
        }
    };
}
pub(crate) use variants_evolution;
