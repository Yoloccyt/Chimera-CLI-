//! NexusEvent 变体分组 · 状态 / Quest 生命周期 / 安全 Critical
//!
//! 本文件承载 `NexusEvent` 的连续一段变体（声明序 `NexusStateChanged` … `SandboxViolation`），
//! 经 `macro_rules!` 链式装配进 [`NexusEvent`](crate::types::NexusEvent)。
//!
//! 域: 全局状态变更、模型路由选定、Quest 生命周期与检查点、议会共识,以及
//! 熔断/形式化/预算/沙箱四类 Critical 安全事件。
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

macro_rules! variants_lifecycle {
    ($($acc:tt)*) => {
        crate::types::execution::variants_execution! {
            $($acc)*

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

            /// 影子模式熔断器跳闸 — L4 Security fail-closed 状态变更 (L4 深度优化 P1-1)
            ///
            /// WHY:ShadowModeCircuitBreaker 检测到 FormalVerifier 属性违规永久跳闸时
            /// 发布 (不可逆直至人工复位),供 TUI DecayPanel 等订阅方从事件流派生
            /// 熔断状态显示 (替代原 shadow_breaker_status() 全局函数占位)。
            ShadowBreakerTripped {
                /// 事件元数据
                metadata: EventMetadata,
                /// 跳闸原因 (形式化属性违反反例描述)
                reason: String,
            },

            /// 形式化验证失败 — L4 Security Critical 事件 (mpsc 旁路通道)
            ///
            /// WHY Critical:进化否决影响重大，TUI DecayPanel 需立即高亮告警；
            /// 走 mpsc 而非 broadcast 确保送达（broadcast 可能丢包）。
            ///
            /// 触发时机：FormalVerifierGate::evaluate 返回 failed 或
            /// ShadowModeCircuitBreaker 永久跳闸时发布。
            FormalVerificationFailed {
                /// 事件元数据
                metadata: EventMetadata,
                /// 哪个属性被违反
                property: String,
                /// 反例描述
                counterexample: String,
                /// 当前进化世代
                generation: u64,
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
        }
    };
}
pub(crate) use variants_lifecycle;
