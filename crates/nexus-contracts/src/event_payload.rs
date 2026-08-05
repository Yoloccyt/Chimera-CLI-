//! 事件载荷契约 — L0 共享的纯载荷枚举(ADR-054 决策 6,P9-T7 Task 2)
//!
//! 对应架构层: **L0 Contracts**(从 L1 `event-bus` 下沉,缓解 L1 上帝 crate)
//! 对应 ADR: **ADR-054 决策 6(D1 治理)** — event-bus 是 L1 超级节点(34 依赖方),
//! 3 个高价值纯载荷类型(`EventSeverity` / `TaskPriority` / `AgentStatus`)下沉 L0
//! `nexus-contracts`,供 L1-L10 所有上层 crate 直接导入,消除跨层对 event-bus 的
//! 非必要依赖。
//!
//! # 核心职责
//!
//! 承载事件总线 3 个轻量级分类枚举:
//! - [`EventSeverity`]: 事件严重级别(Normal/Info/Critical),驱动背压与 mpsc 旁路决策
//! - [`TaskPriority`]: Agent 任务委派调度优先级(Low/Medium/High/Critical)
//! - [`AgentStatus`]: Agent 生命周期状态(Idle/Running/Paused/Completed/Failed/Crashed)
//!
//! # 设计约束(ADR-033 + ADR-054 决策 6)
//!
//! - **纯类型 + 零逻辑**: 仅枚举定义(变体/derive 与 event-bus 原定义**逐字一致**),
//!   不含业务逻辑方法
//! - **零 crate 依赖**(serde derive 例外): 与 L0 其余模块一致,仅依赖 serde derive
//! - **severity() 判定逻辑不迁移(架构红线)**: Critical 事件的 mpsc 点对点保障是
//!   系统可靠性红线,`NexusEvent::severity()` / `EventClassification::severity()`
//!   判定逻辑(哪些事件变体判为 Critical/Info/Normal)必须留在 L1 `event-bus`,
//!   本模块仅下沉枚举类型本身,避免判定逻辑漂移导致 Critical 事件降级。
//!
//! # 语义对齐(WHY)
//!
//! 3 枚举的变体名/顺序/derive 与 event-bus `payloads.rs` 原定义逐字一致,
//! serde 无 `rename_all` 等属性(单元变体按变体名字符串序列化),
//! 序列化线格式已冻结,任何改动都会破坏跨进程传输兼容(MCP Mesh)。
//! 参考同系迁移先例: `budget_tier.rs`(P9-T3)/ `event_metadata.rs`(Task 3.10)。

use serde::{Deserialize, Serialize};

/// 事件严重级别 — 用于背压策略决定是否优先投递
///
/// WHY 下沉 L0(ADR-054 决策 6): 本枚举是纯分类标签,被 L1-L10 多个 crate
/// (backpressure/bus/membrane/logging 及各类消费方)共享,下沉后消费方可直接
/// 依赖 L0(`L(N) → L(0)` 恒允许),不再经由 L1 超级节点。
/// **注意**: 判定"某事件是否为 Critical"的 `severity()` 逻辑留在 L1 event-bus,
/// 此处仅承载枚举本身(架构红线:Critical 事件 mpsc 保障)。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventSeverity {
    /// 普通事件:可被背压策略丢弃
    Normal,
    /// 信息级事件:控制请求/反馈等,不阻断系统但优先级高于 Normal
    ///
    /// WHY 独立变体: TUI 双向控制事件(Quest 取消/优先级调整)属于操作员意图传达,
    /// 重要性高于普通遥测事件,但非安全关键(不会触发 mpsc 旁路投递)。
    /// 现有 `== Critical` 判定自动将其视为非关键,与"不阻断系统"语义一致。
    Info,
    /// 关键事件:检查点、共识、安全告警等,不可丢弃
    ///
    /// WHY: CheckpointSaved 等事件丢失会导致 Quest 无法恢复,
    /// 必须标注 Critical 以触发 mpsc 点对点通道或保留优先级。
    Critical,
}

/// 任务优先级 — Agent 任务委派(AgentTaskDelegated)的调度优先级
///
/// WHY 下沉 L0(ADR-054 决策 6): 原定义于 event-bus(L1)仅供 chimera-mas(L9)
/// 发布 `AgentTaskDelegated` 事件时作为 payload 字段;下沉后 L9 可直接依赖 L0,
/// 无需经 L1 超级节点中转,消除 L1 上帝 crate 的 34 依赖方负担。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TaskPriority {
    /// 低优先级,空闲时调度
    Low,
    /// 中等优先级,正常调度队列
    Medium,
    /// 高优先级,优先调度
    High,
    /// 最高优先级,立即调度(可能抢占低优先级任务)
    Critical,
}

/// Agent 生命周期状态 — AgentHeartbeat 事件携带的 Agent 运行时状态
///
/// WHY 下沉 L0(ADR-054 决策 6): 同 TaskPriority,为纯分类标签。
/// 变体语义与 chimera-mas::AgentStatus 保持一致(Idle/Running/Paused/
/// Completed/Failed/Crashed),L9 发布心跳事件时经
/// `From<chimera_mas::AgentStatus>` 转换映射到本类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum AgentStatus {
    /// 空闲状态,等待任务分配
    Idle,
    /// 运行中,正在执行任务
    Running,
    /// 已暂停,可恢复
    Paused,
    /// 任务已完成
    Completed,
    /// 任务执行失败
    Failed,
    /// Agent 崩溃,不可恢复
    Crashed,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全部 EventSeverity 变体清单 — 供遍历式测试复用,避免遗漏新增变体
    const ALL_SEVERITIES: [EventSeverity; 3] = [
        EventSeverity::Normal,
        EventSeverity::Info,
        EventSeverity::Critical,
    ];

    /// 全部 TaskPriority 变体清单
    const ALL_PRIORITIES: [TaskPriority; 4] = [
        TaskPriority::Low,
        TaskPriority::Medium,
        TaskPriority::High,
        TaskPriority::Critical,
    ];

    /// 全部 AgentStatus 变体清单
    const ALL_AGENT_STATUSES: [AgentStatus; 6] = [
        AgentStatus::Idle,
        AgentStatus::Running,
        AgentStatus::Paused,
        AgentStatus::Completed,
        AgentStatus::Failed,
        AgentStatus::Crashed,
    ];

    /// proptest 策略: 全变体空间任意 `EventSeverity`
    ///
    /// WHY 用 `prop::sample::select` 显式覆盖全变体,而非为纯枚举实现 `Arbitrary`:
    /// 保持 L0 零逻辑约束,测试专用策略不进入生产 API(ADR-033)。
    fn any_severity() -> impl proptest::strategy::Strategy<Value = EventSeverity> {
        proptest::sample::select(vec![
            EventSeverity::Normal,
            EventSeverity::Info,
            EventSeverity::Critical,
        ])
    }

    /// proptest 策略: 全变体空间任意 `TaskPriority`
    fn any_priority() -> impl proptest::strategy::Strategy<Value = TaskPriority> {
        proptest::sample::select(vec![
            TaskPriority::Low,
            TaskPriority::Medium,
            TaskPriority::High,
            TaskPriority::Critical,
        ])
    }

    /// proptest 策略: 全变体空间任意 `AgentStatus`
    fn any_agent_status() -> impl proptest::strategy::Strategy<Value = AgentStatus> {
        proptest::sample::select(vec![
            AgentStatus::Idle,
            AgentStatus::Running,
            AgentStatus::Paused,
            AgentStatus::Completed,
            AgentStatus::Failed,
            AgentStatus::Crashed,
        ])
    }

    /// 序列化往返: 每个 EventSeverity 变体 serde_json 序列化 → 反序列化后与原值相等
    #[test]
    fn test_event_severity_serde_json_roundtrip_all_variants() {
        for sev in ALL_SEVERITIES {
            let json = serde_json::to_string(&sev).unwrap();
            let restored: EventSeverity = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, sev, "变体 {sev:?} 序列化往返失败");
        }
    }

    /// 序列化往返: 每个 TaskPriority 变体 serde_json 序列化 → 反序列化后与原值相等
    #[test]
    fn test_task_priority_serde_json_roundtrip_all_variants() {
        for pri in ALL_PRIORITIES {
            let json = serde_json::to_string(&pri).unwrap();
            let restored: TaskPriority = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, pri, "变体 {pri:?} 序列化往返失败");
        }
    }

    /// 序列化往返: 每个 AgentStatus 变体 serde_json 序列化 → 反序列化后与原值相等
    #[test]
    fn test_agent_status_serde_json_roundtrip_all_variants() {
        for st in ALL_AGENT_STATUSES {
            let json = serde_json::to_string(&st).unwrap();
            let restored: AgentStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, st, "变体 {st:?} 序列化往返失败");
        }
    }

    /// 变体完整性: 穷举 match 断言 EventSeverity 全集与数量
    ///
    /// WHY 穷举 match: 新增变体时编译器强制补充分支(编译期完整性检查),
    /// 与 ALL_SEVERITIES 数组形成双重保护。
    #[test]
    fn test_event_severity_variant_completeness() {
        let names: Vec<&str> = ALL_SEVERITIES
            .iter()
            .map(|s| match s {
                EventSeverity::Normal => "Normal",
                EventSeverity::Info => "Info",
                EventSeverity::Critical => "Critical",
            })
            .collect();
        assert_eq!(names.len(), 3);
        assert_eq!(names, vec!["Normal", "Info", "Critical"]);
    }

    /// 变体完整性: 穷举 match 断言 TaskPriority 全集与数量
    #[test]
    fn test_task_priority_variant_completeness() {
        let names: Vec<&str> = ALL_PRIORITIES
            .iter()
            .map(|p| match p {
                TaskPriority::Low => "Low",
                TaskPriority::Medium => "Medium",
                TaskPriority::High => "High",
                TaskPriority::Critical => "Critical",
            })
            .collect();
        assert_eq!(names.len(), 4);
        assert_eq!(names, vec!["Low", "Medium", "High", "Critical"]);
    }

    /// 变体完整性: 穷举 match 断言 AgentStatus 全集与数量
    #[test]
    fn test_agent_status_variant_completeness() {
        let names: Vec<&str> = ALL_AGENT_STATUSES
            .iter()
            .map(|s| match s {
                AgentStatus::Idle => "Idle",
                AgentStatus::Running => "Running",
                AgentStatus::Paused => "Paused",
                AgentStatus::Completed => "Completed",
                AgentStatus::Failed => "Failed",
                AgentStatus::Crashed => "Crashed",
            })
            .collect();
        assert_eq!(names.len(), 6);
        assert_eq!(
            names,
            vec![
                "Idle",
                "Running",
                "Paused",
                "Completed",
                "Failed",
                "Crashed"
            ]
        );
    }

    /// 线格式冻结: serde 单元变体序列化为变体名字符串(无 rename_all 属性)
    ///
    /// WHY 显式断言 JSON 字符串: 枚举为跨进程契约(MCP Mesh / 事件持久化),
    /// 线格式与 event-bus 原定义逐字一致,任何改动都会破坏向后兼容。
    #[test]
    fn test_event_severity_json_wire_format_frozen() {
        assert_eq!(
            serde_json::to_string(&EventSeverity::Normal).unwrap(),
            "\"Normal\""
        );
        assert_eq!(
            serde_json::to_string(&EventSeverity::Info).unwrap(),
            "\"Info\""
        );
        assert_eq!(
            serde_json::to_string(&EventSeverity::Critical).unwrap(),
            "\"Critical\""
        );
        assert_eq!(
            serde_json::to_string(&TaskPriority::Critical).unwrap(),
            "\"Critical\""
        );
        assert_eq!(
            serde_json::to_string(&AgentStatus::Crashed).unwrap(),
            "\"Crashed\""
        );
    }

    /// 枚举闭集: 未知变体字符串必须反序列化失败
    ///
    /// WHY 拒绝未知变体: 契约类型闭集,非法值应快速失败而非静默降级,
    /// 防止跨进程混入未知严重级别绕过背压/mpsc 判定。
    #[test]
    fn test_event_severity_rejects_unknown_variant() {
        let err = serde_json::from_str::<EventSeverity>("\"Unknown\"").unwrap_err();
        assert!(!err.to_string().is_empty(), "未知变体应返回具体错误信息");
    }

    // proptest 属性: 全变体空间 serde_json 序列化往返保真(Eq 不变式)
    //
    // WHY 用普通注释而非 doc comment: proptest! 宏会为 #[test] fn 生成包装,
    // 宏外部的 doc comment 无法附着到生成项,会触发 unused_doc_comments 警告。
    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

        /// 全变体空间不变量: EventSeverity 任意值序列化往返后与原值相等
        #[test]
        fn prop_event_severity_roundtrip_invariant(sev in any_severity()) {
            let json = serde_json::to_string(&sev).unwrap();
            let restored: EventSeverity = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, sev, "变体 {sev:?} 往返不一致");
        }

        /// 全变体空间不变量: TaskPriority 任意值序列化往返后与原值相等
        #[test]
        fn prop_task_priority_roundtrip_invariant(pri in any_priority()) {
            let json = serde_json::to_string(&pri).unwrap();
            let restored: TaskPriority = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, pri, "变体 {pri:?} 往返不一致");
        }

        /// 全变体空间不变量: AgentStatus 任意值序列化往返后与原值相等
        #[test]
        fn prop_agent_status_roundtrip_invariant(st in any_agent_status()) {
            let json = serde_json::to_string(&st).unwrap();
            let restored: AgentStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, st, "变体 {st:?} 往返不一致");
        }
    }
}
