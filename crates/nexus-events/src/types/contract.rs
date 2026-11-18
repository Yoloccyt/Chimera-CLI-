//! NexusEvent 变体分组 · 契约与遥测观测面
//!
//! 本文件承载 `NexusEvent` 的连续一段变体（声明序 `UserIntentEncoded` … `TelemetryReporterDegraded`），
//! 经 `macro_rules!` 链式装配进 [`NexusEvent`](crate::types::NexusEvent)。
//!
//! 域: 意图编码入口、裁决权分离(Claim/Certify/Declaim)、FailureRegistry、黑板条目,
//! 以及 L1 计算面饱和与 L0-B 遥测治理两条观测面。
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

macro_rules! variants_contract {
    ($($acc:tt)*) => {
        crate::types::lifecycle::variants_lifecycle! {
            $($acc)*
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
            // 裁决权分离契约(v2.3 (b.1))：Claim/Certify/Declaim 语义分离
            //   —— agent 只能 Claim(宣布)，Certify(确认)由验证器独占签发。
            //   payload 仅携带消费者必需字段，完整结构化定义见 L0
            //   nexus_contracts::adjudication(ClaimPayload/CertifyPayload/...)。
            // ============================================================
            /// 裁决权分离·Claim — agent 宣布某命题成立(广播级,agent 可写;不含验证语义)
            ClaimIssued {
                /// 事件元数据
                metadata: EventMetadata,
                /// 声明全局唯一 ID(供 Certify/Declaim 引用)
                claim_id: String,
                /// 被宣布成立的命题主体(人类可读摘要;大对象引用化)
                subject: String,
                /// 声明发起者标识(agent/expert ID)
                claimant: String,
            },
            /// 裁决权分离·Certify — 验证器确认某 Claim 成立(Critical 旁路,验证器语义独占)
            ///
            /// WHY Critical:成功渲染必须消费 Certify;丢失=已验证事实无法投递到 UI/审计,
            /// 等价于「生成者自评」漏洞重新成立(Anthropic 低方差红线)。
            CertifyIssued {
                /// 事件元数据
                metadata: EventMetadata,
                /// 被认证/否决的 Claim ID
                claim_ref: String,
                /// 命题主体(冗余携带,便于审计与 UI 渲染)
                subject: String,
                /// 签发验证器种类(白名单治理门据此授权;取值见 nexus_contracts::VerifierKind)
                verifier: String,
            },
            /// 裁决权分离·Declaim — agent 撤回此前提出的 Claim(广播级,审计完整性)
            ClaimRetracted {
                /// 事件元数据
                metadata: EventMetadata,
                /// 被撤回的 Claim ID
                claim_ref: String,
                /// 撤回原因(人类可读)
                reason: String,
            },
            /// FailureRegistry — 运行时踩坑的结构化登记(Critical 旁路,必达)
            ///
            /// WHY Critical:丢失一条 = 全 swarm 集体重复踩坑(Anthropic「独有信息淹没」
            /// 是结构性的)。完整四字段(适用条件/实验依据/失效边界/修复状态)落
            /// nexus_contracts::FailureRecord 与 WAL;事件面携带签名+归属+状态。
            FailureRegistered {
                /// 事件元数据
                metadata: EventMetadata,
                /// 失败签名(去重与定向检索用)
                signature: String,
                /// 受影响组件(crate 名或模块路径)
                affected_component: String,
                /// 修复状态(取值见 nexus_contracts::FailureFixStatus)
                fix_status: String,
            },

            /// 黑板条目 (v2.3 §(b.2)) — MLC 协作态投影的一条共享上下文(广播级 Normal)。
            ///
            /// 黑板 = MLC 的协作态投影(非事件流副产品);条目带 TTL + 认识类型标签;
            /// provenance 一期可选(不被 ADR-132 因果归因未接线阻塞);持久化转正属 Wave 3。
            BlackboardEntry {
                /// 事件元数据
                metadata: EventMetadata,
                /// 黑板条目键(检索/去重用)
                key: String,
                /// 条目摘要(消费者必需,大对象引用化)
                summary: String,
                /// TTL 秒(条目在黑板的存活时长)
                ttl_secs: u64,
                /// 认识类型标签(取值见 nexus_contracts::EpistemicType)
                epistemic: String,
                /// 因果来源引用(ADR-132 接线后启用;一期可选)
                provenance: Option<String>,
            },

            /// 计算池饱和观测事件 (ADR-197) — L1 计算桥 rayon 池占用超阈时发布(广播级 Normal)。
            ///
            /// # 触发
            /// `ComputeBridge` 的 L-a 池活跃占用 `saturation_ratio_bp` 达
            /// `CHIMERA_COMPUTE_SATURATION_BP`(默认 900 = 90%)时，由**装配层周期上报**
            /// (先例:`BusThroughputReported` 亦由 chimera-cli experience_loop 周期发布)。
            /// 同一次超阈期内去抖 —— 仅在"未饱和 → 饱和"状态迁移或计数递增后发布，避免刷屏。
            ///
            /// # 为何是 Normal 而非 Critical
            /// 本事件**只报告观测量，不改变对外结果语义**:饱和时降级为 Inline 执行
            /// (方向 = "变慢不变错"，与 ADR-153 "事件永不丢弃 / 降级不变错" 同族)，
            /// 因此不进 `is_critical_mpsc_event` 与 `LANE_FORBIDDEN_SHARD` 两张清单。
            ///
            /// # 使用场景
            /// - efficiency-monitor 订阅后计入 RuntimeAuditor 资源压力维度(Ω₈ 证据纪律)
            /// - 数值均为整型/千分比,避开浮点相等与 `unwrap_or` 静默失真类陷阱
            ComputePoolSaturated {
                /// 事件元数据
                metadata: EventMetadata,
                /// 活跃任务数(发射瞬间快照)
                active_tasks: usize,
                /// 池线程数(num_cpus−2,下限 2,构造期快照)
                pool_threads: usize,
                /// 实际占用千分比 = active_tasks / pool_threads × 1000
                ///
                /// 这是**单口径**（仅 rayon 侧）信号，专用于“池要不要降级”判定；
                /// 评估“整个运行时忙不忙”必须看 `combined_utilization_bp`。
                saturation_ratio_bp: u32,
                /// 双口径合成利用率千分比（ADR-157 口径:tokio 与 rayon 各 0.5 权重；
                /// 无 runtime 时退化为 rayon 单侧并在日志标注降级）
                ///
                /// WHY 与上面并存：ADR-155 的“combined 55.2% 偏差”正是只看了 IO 侧留下的教训，
                /// 因此事件必须同时给出“判定用单口径”与“归因用双口径”，不得用一个冒充另一个。
                combined_utilization_bp: u32,
                /// 触发判定阈值千分比(env 快照,供事后归因)
                threshold_bp: u32,
                /// 累计超阈观测次数（单调;`Observe` 下也增长，因此它是“上报去抖”的基线）
                saturation_count: u64,
                /// 累计因饱和而降级为 Inline 的卸载次数（单调递增；仅 `Degrade` 策略下增长）
                ///
                /// WHY 与上面分开：默认策略是 Observe（只观测不改分支），若两者合用一个字段，
                /// 事件会把“观测到的超阈”误报成“发生了行为变更”。
                degraded_count: u64,
                /// 占用归因:计数最高的任务类型名(`TaskKind` 文本;无采样时为 "Unknown")
                hottest_kind: String,
            },

            /// 遥测校准写表完成 (ADR-223 D2) — SPRT 校准在 apply 模式写入 HTS 阈值表时发布(广播级 Normal)。
            ///
            /// # 触发
            /// 周期上报的一次 tick 中 `SaturationSource::apply_calibration()` 返回的写入行数 > 0。
            /// 非 apply 模式恒为 0 ⇒ 不发布(不为零变化刷屏)。
            ///
            /// # 为何是 Normal 而非 Critical
            /// 本事件只报告治理动作结果,不改变请求语义、不承载必须送达的否决/预算类信号;
            /// 因此不进 `is_critical_mpsc_event` 与 `LANE_FORBIDDEN_SHARD` 两张清单。
            ///
            /// # 使用场景
            /// - efficiency-monitor 订阅后把“阈值被改写”计入 RuntimeAuditor 治理证据
            /// - 与 `tracing::info!` 同点发出:日志可读、总线可订阅,两条证据面不互斥
            TelemetryCalibrationApplied {
                /// 事件元数据
                metadata: EventMetadata,
                /// 本轮写入 HTS 阈值表的行数(>0)
                applied_rows: usize,
            },

            /// 遥测上报器启动期配置降级 (ADR-223 D2) — `period_secs == 0` 被钳到 1s 时发布一次。
            ///
            /// # 为何单独成事件
            /// `clamped_period` 是本 crate 唯一一条“输入被静默改写”的规则;若只在 tracing 里写,
            /// 运维无法通过事件面发现“配置写了 0、系统按 1s 跑”。本事件只在启动时发一次,
            /// 不随 tick 刷屏。
            ///
            /// # 为何是 Normal
            /// 配置降级是启动期可观测性信号,不改变对外结果语义,不进 mpsc 旁路与分片禁区。
            TelemetryReporterDegraded {
                /// 事件元数据
                metadata: EventMetadata,
                /// 装配层传入的原始周期(秒)
                requested_period_secs: u64,
                /// 实际生效周期(秒)
                effective_period_secs: u64,
            },
        }
    };
}
pub(crate) use variants_contract;
