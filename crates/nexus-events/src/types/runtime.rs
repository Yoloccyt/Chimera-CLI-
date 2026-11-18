//! NexusEvent 变体分组 · 模型通道 / 上下文 / 记忆策略 / 观测面
//!
//! 本文件承载 `NexusEvent` 的连续一段变体（声明序 `ModelAffinitySelected` … `SecurityInterceptionReported`），
//! 经 `macro_rules!` 链式装配进 [`NexusEvent`](crate::types::NexusEvent)。
//!
//! 域: MCA 通道选择与额度、窗口与缓存亲和、上下文预算、语义缓存、幽灵记忆、
//! 基准与 HCW 召回、形式化裁决、奖励信号、吞吐与拦截率观测。
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

macro_rules! variants_runtime {
    ($($acc:tt)*) => {
        crate::types::define_nexus_event! {
            $($acc)*

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

            /// 模型上下文能力档案更新 — HCW v3 蓝图 P0(ADR-290)
            ///
            /// mca-gateway(L10) 的有效窗口探针(observe 档)完成一次标定后广播:
            /// effective_window 为探针拟合结果(80% 检索准确率截点)或公开基准先验,
            /// hcw-window(L2) 经 L0 `ContextCapabilityProfile` 只读消费本事件刷新
            /// WindowGovernor 定容输入。WHY 经事件而非直调:依赖铁律 L2 不得感知 L10,
            /// 能力数据一律经 L0 中转 + 事件广播(蓝图 §5.2)。
            ///
            /// # 跨层通信(C6)
            /// L10(mca-gateway) → L2(hcw-window),经 event-bus 解耦。
            /// 本事件为 Normal 级别(观测面),不触发 mpsc 旁路;丢失可由下次探针周期补偿。
            ContextCapabilityUpdated {
                /// 事件元数据
                metadata: EventMetadata,
                /// 路由键 `provider/model`(与 ModelAffinitySelected 一致)
                route_key: String,
                /// 标称窗口(官方,token)
                advertised_window: u32,
                /// 有效窗口(探针标定或先验,token)——常驻层定容的真实依据
                effective_window: u32,
                /// 标定置信度: "external_prior" / "own_probe" / "inferred"
                confidence: String,
            },

            /// 窗口计划应用留痕 — HCW v3 蓝图 P1 WindowGovernor(ADR-290)
            ///
            /// hcw-window(L2) 的 WindowGovernor 每次生成 WindowPlan 后发布本事件,
            /// 作为「双跑对照回滚」与「同任务 WindowPlan 快照可回放」的证据锚点
            /// (蓝图 §七 P1 验收线 + §八 回滚设计)。纯观测,不影响窗口执行路径。
            ///
            /// # 跨层通信(C6)
            /// L2(hcw-window) 广播,观测面;Normal 级别,不触发 mpsc 旁路。
            WindowPlanApplied {
                /// 事件元数据
                metadata: EventMetadata,
                /// 本请求常驻上限(token,= min(有效窗口×安全系数,预算上限,延迟 SLO))
                resident_cap: u32,
                /// 页预算(沿用 4000 码元页预算传统)
                page_budget: u32,
                /// R/S/E/M 四层配额比例快照(JSON 序列化的 TierAlloc,供回放)
                tier_allocation_json: String,
                /// 计划来源: "static_fallback" / "governor"(双跑对照的分流标记)
                plan_source: String,
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
    };
}
pub(crate) use variants_runtime;
