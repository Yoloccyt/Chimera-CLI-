//! 纯领域类型契约 — 用户意图/长期任务/思考模式上提(ADR-054 决策 6,P9-T7)
//!
//! 对应架构层: **L0 Contracts**(从 L1 `nexus-core` 上提,缓解 D1 上帝 crate 病理)
//! 对应 ADR: **ADR-054 决策 6**(5 个高价值纯领域类型下沉 L0,nexus-core 改 re-export)
//!
//! # 核心职责
//!
//! 承载跨层共享的**纯领域类型**(ThinkingMode / MultimodalInput / UserIntent / Quest / Task)。
//! 原定义于 `nexus-core/src/types.rs`(nexus-core 为 L1 超级节点,被 30 依赖方引用),
//! 下沉到 L0 共享契约层后:
//! - 依赖方可直接依赖 L0(`L(N) → L(0)` 恒允许),不再依赖 L1 `nexus-core`
//! - L1 `nexus-core` 保留 re-export,对外 API 零破坏
//!
//! # 设计约束(ADR-033)
//!
//! - **纯类型 + 零逻辑**: 仅类型定义 + 基础构造函数(Quest::default / default_priority),
//!   不含业务逻辑
//! - **零 crate 依赖**(serde derive 例外): 与 L0 其余模块一致,仅依赖 serde derive,
//!   严禁引入 nexus-core / 任何 workspace crate
//! - **Task.status 引用**: 直接使用 L0 同层 `crate::task::TaskStatus`(Task 3.10 已下沉)
//!
//! # 语义对齐(WHY)
//!
//! 5 类型定义(derive / 字段 / impl 块)与 nexus-core 原实现**逐字一致**,仅移动定义位置。
//! `#[serde(default = "default_priority")]` 的旧数据兼容语义(无 priority 字段反序列化
//! 取默认 128)为冻结契约,消费方(quest-engine 等)依赖该行为,禁止改动。

use serde::{Deserialize, Serialize};

/// 用户意图 — NMC 编码后的多模态用户输入
///
/// `risk_level` 范围 0-100,影响后续沙箱策略:
/// - 0-30:低风险,只读操作
/// - 31-70:中风险,有副作用但可控
/// - 71-100:高风险,需 Parliament 审议
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserIntent {
    /// 意图唯一标识(UUIDv7,时间有序)
    pub intent_id: String,
    /// 用户输入原始文本
    pub raw_text: String,
    /// 多模态输入列表(Week 2 仅 Text 变体,Week 6 扩展 Image/Video/Audio)
    pub multimodal_inputs: Vec<MultimodalInput>,
    /// 风险等级(0-100),影响沙箱策略与议会审议门槛
    pub risk_level: u8,
}

/// 多模态输入枚举 — 支持文本、图像、视频、音频
///
/// WHY:Week 2 阶段仅实现 Text 变体,但提前定义完整枚举
/// 以避免后续扩展时破坏序列化兼容性(serde tag 已固定)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MultimodalInput {
    /// 文本输入(Week 2 唯一实现的变体)
    Text(String),
    // Week 6 扩展:
    // Image(Vec<u8>),
    // Video(Vec<u8>),
    // Audio(Vec<u8>),
}

/// 思考模式 — TTG(Thinking Toggle Governance)三级切换
///
/// Parliament 根据 Quest 复杂度与预算动态切换:
/// - `Fast`:简单任务,快速响应(如查询、格式化)
/// - `Standard`:常规任务,平衡速度与深度
/// - `Deep`:复杂任务,深度推理(如架构设计、调试)
///
/// # 镜像关系(ADR-065)
/// L0 `nexus_contracts::affinity::ThinkingPreference` 是本类型的镜像
/// (L0 零 crate 依赖铁律禁止 L0 → L1 引用),两处三档语义必须保持同步。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThinkingMode {
    /// 快速模式:低延迟,适合简单任务
    Fast,
    /// 标准模式:平衡,适合常规任务
    Standard,
    /// 深度模式:高延迟高深度,适合复杂任务
    Deep,
}

/// 任务节点 — Quest 中的单个执行单元
///
/// `dependencies` 存储前置 Task ID 列表,支持 DAG 依赖图。
/// GQEP 执行器据此拓扑排序,确保依赖先完成。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Task {
    /// 任务唯一标识
    pub task_id: String,
    /// 任务描述(自然语言)
    pub description: String,
    /// 当前状态(Task 3.10 已下沉 L0,同层直接引用)
    pub status: crate::task::TaskStatus,
    /// 前置 Task ID 列表(空表示无依赖,可立即执行)
    pub dependencies: Vec<String>,
}

/// 长期任务 — 用户意图分解后的多步骤执行计划
///
/// 由 Quest Engine 从 `UserIntent` 分解而来,经 Parliament 审议后执行。
/// `checkpoint_id` 指向最近一次检查点,支持断点恢复(LHQP)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Quest {
    /// Quest 唯一标识
    pub quest_id: String,
    /// Quest 标题(人类可读)
    pub title: String,
    /// 任务列表(DAG 结构,通过 Task.dependencies 表达)
    pub tasks: Vec<Task>,
    /// 思考模式(TTG),影响执行深度与延迟
    pub thinking_mode: ThinkingMode,
    /// 最近检查点 ID(无检查点时为 None)
    pub checkpoint_id: Option<String>,
    /// Quest 优先级 (0=最低, 255=最高, 默认 128)
    /// WHY u8: 足够 256 级优先级,内存占用最小
    /// WHY #[serde(default)]: 保证旧数据(无 priority 字段)反序列化时取默认值 128
    #[serde(default = "default_priority")]
    pub priority: u8,
}

/// Quest.priority 的 serde 默认值函数
/// WHY 独立函数:`#[serde(default = "...")]` 需要具名函数路径,不能用闭包或常量
fn default_priority() -> u8 {
    128
}

impl Default for Quest {
    fn default() -> Self {
        Self {
            quest_id: String::new(),
            title: String::new(),
            tasks: vec![],
            // ThinkingMode 无 Default impl,Standard 是语义上的默认(平衡速度与深度)
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: default_priority(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskStatus;

    /// 全部思考模式变体清单 — 供遍历式测试复用,避免遗漏新增变体
    const ALL_THINKING_MODES: [ThinkingMode; 3] = [
        ThinkingMode::Fast,
        ThinkingMode::Standard,
        ThinkingMode::Deep,
    ];

    /// proptest 策略: 全变体空间任意 `ThinkingMode`
    ///
    /// WHY 用 `prop::sample::select` 显式覆盖三档,而非为纯枚举实现 `Arbitrary`:
    /// 保持 L0 零逻辑约束,测试专用策略不进入生产 API(ADR-033)。
    fn any_thinking_mode() -> impl proptest::strategy::Strategy<Value = ThinkingMode> {
        proptest::sample::select(vec![
            ThinkingMode::Fast,
            ThinkingMode::Standard,
            ThinkingMode::Deep,
        ])
    }

    /// 构造一个完整的多模态用户意图(含 Text 输入)
    fn make_user_intent() -> UserIntent {
        UserIntent {
            intent_id: "intent-1".into(),
            raw_text: "帮我写一份架构设计".into(),
            multimodal_inputs: vec![MultimodalInput::Text("hello".into())],
            risk_level: 42,
        }
    }

    /// 构造一个完整的任务节点(含 6 变体状态)
    fn make_task() -> Task {
        Task {
            task_id: "t-1".into(),
            description: "第一步".into(),
            status: TaskStatus::Running,
            dependencies: vec!["t-0".into()],
        }
    }

    /// 构造一个完整的长期任务(含任务列表 + 思考模式 + 优先级)
    fn make_quest() -> Quest {
        Quest {
            quest_id: "q-1".into(),
            title: "测试 Quest".into(),
            tasks: vec![make_task()],
            thinking_mode: ThinkingMode::Deep,
            checkpoint_id: Some("cp-1".into()),
            priority: 200,
        }
    }

    /// UserIntent serde_json 往返: 序列化 → 反序列化后与原值相等
    #[test]
    fn test_user_intent_serde_roundtrip() {
        let intent = make_user_intent();
        let json = serde_json::to_string(&intent).unwrap();
        let restored: UserIntent = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, intent, "UserIntent 序列化往返失败");
    }

    /// MultimodalInput serde_json 往返: Text 变体序列化 → 反序列化后与原值相等
    #[test]
    fn test_multimodal_input_serde_roundtrip() {
        let input = MultimodalInput::Text("hello".into());
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("Text"), "Text 变体应保留 tag: {json}");
        assert!(json.contains("hello"), "负载内容应保留: {json}");
        let restored: MultimodalInput = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, input, "MultimodalInput 序列化往返失败");
    }

    /// Task serde_json 往返: 含状态枚举与依赖列表的结构体
    #[test]
    fn test_task_serde_roundtrip() {
        let task = make_task();
        let json = serde_json::to_string(&task).unwrap();
        let restored: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, task, "Task 序列化往返失败");
    }

    /// Quest serde_json 往返: 含任务列表 + 思考模式 + 检查点 + 优先级的完整结构
    #[test]
    fn test_quest_serde_roundtrip() {
        let quest = make_quest();
        let json = serde_json::to_string(&quest).unwrap();
        let restored: Quest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, quest, "Quest 序列化往返失败");
    }

    /// ThinkingMode serde_json 往返: 全部 3 个变体逐一验证
    #[test]
    fn test_thinking_mode_serde_roundtrip_all_variants() {
        for mode in ALL_THINKING_MODES {
            let json = serde_json::to_string(&mode).unwrap();
            let restored: ThinkingMode = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, mode, "变体 {mode:?} 序列化往返失败");
        }
    }

    /// Quest::default() 基础构造: 优先级默认 128,思考模式默认 Standard
    #[test]
    fn quest_default_priority_is_128() {
        let quest = Quest::default();
        assert_eq!(quest.priority, 128, "默认优先级应为 128");
        assert_eq!(
            quest.thinking_mode,
            ThinkingMode::Standard,
            "默认思考模式应为 Standard"
        );
        assert!(quest.checkpoint_id.is_none(), "默认无检查点");
        assert!(quest.tasks.is_empty(), "默认任务列表为空");
    }

    /// 旧数据兼容: 无 priority 字段的 JSON 反序列化后取默认优先级 128
    #[test]
    fn quest_old_data_without_priority_deserializes_to_default() {
        let old_json = r#"{"quest_id":"q1","title":"Old","tasks":[],"thinking_mode":"Standard","checkpoint_id":null}"#;
        let decoded: Quest = serde_json::from_str(old_json).unwrap();
        assert_eq!(decoded.priority, 128, "旧数据应取默认优先级 128");
    }

    // proptest 属性: 全变体空间任意 ThinkingMode serde_json 往返后与原值相等
    //
    // WHY 用普通注释而非 doc comment:proptest! 宏会为 #[test] fn 生成包装,
    // 宏外部的 doc comment 无法附着到生成项,会触发 unused_doc_comments 警告。
    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

        /// 全变体空间不变量: 任意思考模式序列化往返后与原值相等
        #[test]
        fn prop_thinking_mode_serde_roundtrip(mode in any_thinking_mode()) {
            let json = serde_json::to_string(&mode).unwrap();
            let restored: ThinkingMode = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, mode, "变体 {mode:?} 序列化往返失败");
        }
    }
}
