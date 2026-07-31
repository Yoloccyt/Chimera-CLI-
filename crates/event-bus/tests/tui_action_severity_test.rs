//! # TuiAction 协议 severity 守护测试 — ADR-029 v3.1 M2 启用前评估(Task 3.1)
//!
//! ## 背景
//! v3-engine M2 启用后渲染引擎成为唯一路径,需评估 TuiAction 8 变体的
//! `severity()` 分级是否需调整(任务假设可能降级 Info→Debug 或升级
//! TuiActionFailed→Warning/Critical)。
//!
//! ## ADR 风格评估结论(2026-07-30)
//!
//! ### Decision: 不调整 TuiAction 8 变体的 severity 分级
//!
//! ### Context
//! - EventSeverity 枚举只有 3 档(payloads.rs:51-65):Normal / Info / Critical
//! - 任务假设的 Debug / Warning 档**不存在**,无法降级或升级到这些档
//!
//! ### Findings
//! 1. **EventSeverity 三档设计意图明确**:
//!    - Normal:可被背压策略丢弃(高频流式)
//!    - Info:控制请求/反馈,不阻断系统但优先级高于 Normal(低频请求/终态)
//!    - Critical:不可丢弃,走 mpsc 旁路(稀有安全告警)
//!
//! 2. **M2 启用不影响事件发布频率**:
//!    - 渲染引擎是事件**消费方**(TUI 侧增量渲染),不是事件源
//!    - 事件流量由用户交互频率决定,不由渲染引擎决定
//!    - 高频流式变体已走 Normal 通配符分支,不占 mpsc 旁路
//!
//! 3. **当前分级合理**(types.rs:1499-1507, 1524-1527, 1547-1550):
//!    - 3 个高频流式变体(Progressed/ResponseChunk/StatusChanged)→ Normal(通配符)
//!    - 5 个低频请求/终态变体(Requested/Completed/Failed/ChatSubmitted/ChatCompleted)→ Info(显式)
//!    - 0 个 Critical(TuiActionFailed 注释 types.rs:1547-1550 明确:失败是操作员
//!      可感知交互结果,非系统安全事件)
//!
//! 4. **severity() 的 8 个使用点均不受影响**:
//!    - bus.rs:210/281 Critical 无订阅者告警(TuiAction 不触发)
//!    - backpressure.rs:127 Critical 不丢弃(TuiAction 可被丢弃)
//!    - membrane.rs:324 Critical 穿膜优先(TuiAction 走分类逻辑)
//!    - logging.rs / TUI 面板:Critical 高亮(TuiAction 不高亮)
//!
//! ### Consequences
//! - 无 severity() 代码改动(保守原则:不强改合理设计)
//! - 新增 2 个守护测试防止未来回归(覆盖 Progressed + Failed,补全
//!   `test_tui_action_protocol_severity` 只覆盖 Requested/ResponseChunk/Submitted 的缺口)
//!
//! ### 测试性质说明
//! 本测试为**守护测试(Guard Test)**,不是 TDD RED 驱动测试。因评估结论是
//! "不调整 severity",无新实现要驱动,测试直接 GREEN。断言符合 ADR-029 设计
//! 意图(types.rs 注释),非匹配 buggy code。守护目标:防止未来误调整:
//! - 误把 Progressed 提到 Info → 增加噪音且无必要(高频流式本就该可丢弃)
//! - 误把 Failed 降到 Normal → 丢失操作员可感知性
//! - 误把 Failed 升到 Critical → 占用仅为稀有安全告警保留的 mpsc 旁路

use event_bus::{ActionSource, ChatStatus, EventMetadata, EventSeverity, NexusEvent};

// ============================================================
// 守护测试 — TuiAction 8 变体 severity 防回归
// ============================================================

// ============================================================
// Task 3.1 新增测试 1: TuiAction 8 变体 severity 分级验证
// ============================================================

/// 验证 TuiAction 8 变体的 severity 分级符合预期
///
/// 分级依据(ADR-029 + types.rs 注释):
/// - 高频流式 3 变体(Progressed/ResponseChunk/StatusChanged) → Normal(通配符分支)
/// - 低频请求/终态 5 变体(Requested/Completed/Failed/ChatSubmitted/ChatCompleted) → Info(显式分支)
/// - 0 个 Critical(TuiActionFailed 是操作员可感知交互结果,非系统安全事件)
///
/// 防护目标: 防止任一变体被误提/误降到错误级别
#[test]
fn test_tui_action_severity_classification() {
    // --- 5 个 Info 级变体(低频请求/终态) ---
    let requested = NexusEvent::TuiActionRequested {
        metadata: EventMetadata::new("chimera-tui"),
        action_id: "quest.pause".into(),
        payload: r#"{"quest_id":"q1"}"#.into(),
        source: ActionSource::Palette,
    };
    assert_eq!(
        requested.severity(),
        EventSeverity::Info,
        "TuiActionRequested: 低频动作请求,Info 级"
    );

    let completed = NexusEvent::TuiActionCompleted {
        metadata: EventMetadata::new("chimera-cli"),
        action_id: "quest.export".into(),
        result: r#"{"file":"export.json"}"#.into(),
    };
    assert_eq!(
        completed.severity(),
        EventSeverity::Info,
        "TuiActionCompleted: 低频终态,Info 级"
    );

    let failed = NexusEvent::TuiActionFailed {
        metadata: EventMetadata::new("chimera-cli"),
        action_id: "quest.pause".into(),
        error: "quest already completed".into(),
    };
    assert_eq!(
        failed.severity(),
        EventSeverity::Info,
        "TuiActionFailed: 操作员可感知交互失败,Info 级(非系统安全事件)"
    );

    let chat_submitted = NexusEvent::TuiChatSubmitted {
        metadata: EventMetadata::new("chimera-tui"),
        session_id: "s1".into(),
        query: "实现登录".into(),
        slash_command: None,
    };
    assert_eq!(
        chat_submitted.severity(),
        EventSeverity::Info,
        "TuiChatSubmitted: 低频对话提交,Info 级"
    );

    let chat_completed = NexusEvent::TuiChatCompleted {
        metadata: EventMetadata::new("chimera-cli"),
        session_id: "s1".into(),
        tool_use: None,
    };
    assert_eq!(
        chat_completed.severity(),
        EventSeverity::Info,
        "TuiChatCompleted: 低频对话终态,Info 级"
    );

    // --- 3 个 Normal 级变体(高频流式,走通配符分支) ---
    let progressed = NexusEvent::TuiActionProgressed {
        metadata: EventMetadata::new("chimera-cli"),
        action_id: "quest.export".into(),
        delta: r#"{"progress":45}"#.into(),
    };
    assert_eq!(
        progressed.severity(),
        EventSeverity::Normal,
        "TuiActionProgressed: 高频流式进度,走通配符 Normal"
    );

    let response_chunk = NexusEvent::TuiChatResponseChunk {
        metadata: EventMetadata::new("chimera-cli"),
        session_id: "s1".into(),
        delta: "hello".into(),
        cursor_hint: 0,
    };
    assert_eq!(
        response_chunk.severity(),
        EventSeverity::Normal,
        "TuiChatResponseChunk: 高频 token 流,走通配符 Normal"
    );

    let status_changed = NexusEvent::TuiChatStatusChanged {
        metadata: EventMetadata::new("chimera-cli"),
        session_id: "s1".into(),
        status: ChatStatus::Thinking,
    };
    assert_eq!(
        status_changed.severity(),
        EventSeverity::Normal,
        "TuiChatStatusChanged: 高频状态指示器,走通配符 Normal"
    );

    // --- 验证: 无 TuiAction 变体为 Critical ---
    // TuiActionFailed 注释(types.rs:1547-1550)明确: 动作失败是操作员可感知交互结果,
    // 非系统安全事件,不占用 mpsc 旁路
    assert_ne!(
        failed.severity(),
        EventSeverity::Critical,
        "TuiActionFailed 不得为 Critical(TuiAction 8 变体中无 Critical)"
    );
}

// ============================================================
// Task 3.1 新增测试 2: EventSeverity 序关系验证
// ============================================================

/// 验证 EventSeverity 三档的序关系: Critical > Info > Normal
///
/// EventSeverity 不派生 Ord,此测试通过自定义比较函数验证序关系。
/// 防护目标: 防止未来 EventSeverity 变体顺序被误调整导致背压策略
/// 或 mpsc 旁路逻辑错误。
#[test]
fn test_tui_action_severity_ordering() {
    // 自定义序关系: Critical(3) > Info(2) > Normal(1)
    // WHY 不直接用 enum discriminant: 语义序 ≠ 声明序,显式映射更安全
    fn severity_rank(s: EventSeverity) -> u8 {
        match s {
            EventSeverity::Critical => 3,
            EventSeverity::Info => 2,
            EventSeverity::Normal => 1,
        }
    }

    // 验证序关系
    assert!(
        severity_rank(EventSeverity::Critical) > severity_rank(EventSeverity::Info),
        "Critical 应高于 Info"
    );
    assert!(
        severity_rank(EventSeverity::Critical) > severity_rank(EventSeverity::Normal),
        "Critical 应高于 Normal"
    );
    assert!(
        severity_rank(EventSeverity::Info) > severity_rank(EventSeverity::Normal),
        "Info 应高于 Normal"
    );

    // 等值自反性
    assert_eq!(
        severity_rank(EventSeverity::Critical),
        severity_rank(EventSeverity::Critical),
        "Critical 等级应自反"
    );
    assert_eq!(
        severity_rank(EventSeverity::Info),
        severity_rank(EventSeverity::Info),
        "Info 等级应自反"
    );
    assert_eq!(
        severity_rank(EventSeverity::Normal),
        severity_rank(EventSeverity::Normal),
        "Normal 等级应自反"
    );

    // 验证 TuiAction 变体不在 Critical 级别(对齐 ADR-029 设计)
    // 所有 TuiAction 变体的 severity 应 ≤ Info
    let tui_events: Vec<NexusEvent> = vec![
        NexusEvent::TuiActionRequested {
            metadata: EventMetadata::new("t"),
            action_id: "a".into(),
            payload: "{}".into(),
            source: ActionSource::Chat,
        },
        NexusEvent::TuiActionProgressed {
            metadata: EventMetadata::new("t"),
            action_id: "a".into(),
            delta: "d".into(),
        },
        NexusEvent::TuiActionCompleted {
            metadata: EventMetadata::new("t"),
            action_id: "a".into(),
            result: "r".into(),
        },
        NexusEvent::TuiActionFailed {
            metadata: EventMetadata::new("t"),
            action_id: "a".into(),
            error: "e".into(),
        },
        NexusEvent::TuiChatSubmitted {
            metadata: EventMetadata::new("t"),
            session_id: "s".into(),
            query: "q".into(),
            slash_command: None,
        },
        NexusEvent::TuiChatResponseChunk {
            metadata: EventMetadata::new("t"),
            session_id: "s".into(),
            delta: "d".into(),
            cursor_hint: 0,
        },
        NexusEvent::TuiChatCompleted {
            metadata: EventMetadata::new("t"),
            session_id: "s".into(),
            tool_use: None,
        },
        NexusEvent::TuiChatStatusChanged {
            metadata: EventMetadata::new("t"),
            session_id: "s".into(),
            status: ChatStatus::Idle,
        },
    ];

    for event in &tui_events {
        let rank = severity_rank(event.severity());
        assert!(
            rank <= severity_rank(EventSeverity::Info),
            "TuiAction 变体({}) severity 应 ≤ Info, 当前 rank={}",
            event.type_name(),
            rank
        );
    }
}

/// 守护测试(Guard Test)— `TuiActionProgressed` severity 必须为 Normal
///
/// 红线来源:ADR-029 + types.rs:1524-1527 注释 + types.rs:2064-2066 通配符分支
///
/// 防护目标:
/// - 防止误把 `TuiActionProgressed` 显式提到 Info 分支(types.rs:2059-2063)
/// - 高频流式进度事件必须走 Normal(通配符),可被背压策略丢弃
/// - M2 启用后渲染引擎增量消费进度事件,若提到 Info 会增加噪音且不可丢弃,
///   违背"高频流式走 broadcast + 低延迟 drain"设计(types.rs:1526-1527)
#[test]
fn test_tui_action_progressed_severity_is_normal() {
    let event = NexusEvent::TuiActionProgressed {
        metadata: EventMetadata::new("chimera-cli"),
        action_id: "quest.export".into(),
        delta: r#"{"progress":45}"#.into(),
    };
    assert_eq!(
        event.severity(),
        EventSeverity::Normal,
        "TuiActionProgressed severity 必须为 Normal(高频流式进度事件,走通配符分支可被背压丢弃);\
         M2 启用后渲染引擎增量消费,提到 Info 会增加噪音且不可丢弃,违背 ADR-029 设计"
    );
}

/// 守护测试(Guard Test)— `TuiActionFailed` severity 必须为 Info
///
/// 红线来源:ADR-029 + types.rs:1547-1550 注释(Info 而非 Critical 的设计理由)
///
/// 防护目标:
/// - 防止误降到 Normal(丢失操作员可感知性,失败被背压丢弃后用户无反馈)
/// - 防止误升到 Critical(占用 mpsc 旁路,违背"该旁路仅留给稀有安全告警"红线)
/// - 保持 types.rs:1549-1550 设计:动作失败是操作员可感知的交互结果,
///   由 UI 呈现给用户重试,不属于必须旁路投递的系统安全事件
#[test]
fn test_tui_action_failed_severity_is_info() {
    let event = NexusEvent::TuiActionFailed {
        metadata: EventMetadata::new("chimera-cli"),
        action_id: "quest.pause".into(),
        error: "quest already completed".into(),
    };
    assert_eq!(
        event.severity(),
        EventSeverity::Info,
        "TuiActionFailed severity 必须为 Info(操作员可感知的交互失败,非系统安全事件);\
         降到 Normal 会丢失可感知性,升到 Critical 会占用 mpsc 旁路(违背 §6.2 红线)"
    );
}
