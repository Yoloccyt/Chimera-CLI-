//! 记忆金字塔契约 — MSCE + TencentDB 融合（设计文档 §5.4）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §5.4
//! 对应论文: MSCE（三层记忆 L1 Trace/L2 Policy/L3 Env）+ TencentDB Agent Memory（四层金字塔）
//!
//! # 核心职责
//!
//! 承载记忆金字塔的跨层契约类型，使 L2 mlc-engine / L3 cmt-tiering /
//! L5 gsoe-evolution 按统一契约组织记忆：
//!
//! | 类型 | 金字塔层级 | 职责 | 消费层 |
//! |------|-----------|------|--------|
//! | [`MemoryPyramidLevel`] | L0-L3 | 记忆抽象层级（Raw/Atomic/Scene/Persona） | L2 mlc-engine / L3 cmt-tiering |
//! | [`AtomicMemoryCard`] | L1 | 原子记忆卡片（TencentDB L1 + MSCE L1 Trace 融合） | L2 mlc-engine |
//! | [`SceneBlock`] | L2 | 场景档案（TencentDB L2 + MSCE L2 Policy 融合） | L2 mlc-engine / L9 quest-engine |
//! | [`PersonaSummary`] | L3 | 人格摘要（TencentDB L3） | L2 mlc-engine / L10 TUI |
//!
//! # 设计约束（ADR-033）
//!
//! - **纯类型 + 纯函数**: 仅类型定义与层级映射纯函数（无 IO 无状态变更）
//! - **零 crate 依赖**: 仅 `serde` derive + 同层 `rl_hooks` 类型引用
//! - **f32 字段仅 `PartialEq`**: value/gain_gamma 为浮点字段
//! - **`Box<str>` / `Box<[T]>` 优化**: 不可变文本与集合字段采用堆紧凑形态
//! - **层级映射**: `MemoryPyramidLevel` → `ArchiveTier` 为静态映射纯函数
//!   （金字塔层级决定存储温度基线，不构成迁移，不违反 INV-8）

use serde::{Deserialize, Serialize};

// 复用 rl_hooks 的 RL 向量类型（L0 同层引用，记忆卡片承载 RL 状态快照）
use crate::rl_hooks::{RLActionVector, RLStateVector};
// 复用 archive_monotonicity 的 ArchiveTier（L0 同层引用，层级映射目标）
use crate::archive_monotonicity::ArchiveTier;

// ============================================================
// 金字塔层级
// ============================================================

/// 记忆金字塔层级 — MSCE L1/L2/L3 × TencentDB L0/L1/L2/L3 融合
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryPyramidLevel {
    /// 全量原始对话（TencentDB L0）— 未结构化，体积最大
    L0RawLog,
    /// 结构化卡片（TencentDB L1 / MSCE L1 Trace）
    L1AtomicMemory,
    /// 场景档案（TencentDB L2 / MSCE L2 Policy）
    L2SceneBlock,
    /// 人格摘要（TencentDB L3 / MSCE L3 Env Cognition）
    L3Persona,
}

impl MemoryPyramidLevel {
    /// 层级数值（0-3，金字塔自底向上）
    pub fn level_value(&self) -> u8 {
        match self {
            MemoryPyramidLevel::L0RawLog => 0,
            MemoryPyramidLevel::L1AtomicMemory => 1,
            MemoryPyramidLevel::L2SceneBlock => 2,
            MemoryPyramidLevel::L3Persona => 3,
        }
    }
}

/// 金字塔层级 → 存储温度基线映射（静态映射纯函数）
///
/// 语义: 越向上越精炼、访问越频繁 → 存储温度越热。
/// - L0 Raw 日志: 体积大、低频访问 → Cold
/// - L1 Atomic / L2 Scene: 中频访问 → Warm
/// - L3 Persona: 高频注入（每次对话注入人格摘要）→ Hot
///
/// 注: 本映射仅定义层级与温度基线的静态对应，不构成层级迁移，
/// 与 INV-8 归档单调性（Hot→Warm→Cold→Ice 单向降级）正交。
impl From<MemoryPyramidLevel> for ArchiveTier {
    fn from(level: MemoryPyramidLevel) -> Self {
        match level {
            MemoryPyramidLevel::L0RawLog => ArchiveTier::Cold,
            MemoryPyramidLevel::L1AtomicMemory => ArchiveTier::Warm,
            MemoryPyramidLevel::L2SceneBlock => ArchiveTier::Warm,
            MemoryPyramidLevel::L3Persona => ArchiveTier::Hot,
        }
    }
}

// ============================================================
// 原子卡片类型
// ============================================================

/// 原子记忆卡片类型 — TencentDB 三类 × MSCE 三类融合
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtomicCardType {
    /// 用户偏好（TencentDB）
    Preference,
    /// 事件记录（TencentDB）
    Event,
    /// 规则约束（TencentDB）
    Rule,
    /// 执行轨迹（MSCE L1 Trace）
    Trace,
    /// 策略经验（MSCE L2 Policy）
    Policy,
    /// 环境认知（MSCE L3 Env Cognition）
    EnvCognition,
}

// ============================================================
// L1: 原子记忆卡片
// ============================================================

/// 原子记忆卡片 — L1 结构化记忆单元（TencentDB + MSCE 融合）
///
/// 承载单条可检索的结构化记忆；RL 相关卡片可携带状态/动作快照
/// （`state_snapshot` / `action_record`）供训练数据面回溯。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AtomicMemoryCard {
    /// 卡片 ID（约定 UUIDv7）
    pub card_id: Box<str>,
    /// 卡片类型
    pub card_type: AtomicCardType,
    /// 优先级（0-255，越高越优先保留）
    pub priority: u8,
    /// 所属场景标识
    pub scene: Box<str>,
    /// 卡片内容（结构化文本）
    pub content: Box<str>,
    /// 来源轨迹 ID（None = 手工/外部注入）
    pub source_traj_id: Option<Box<str>>,
    /// RL 状态快照（None = 非 RL 卡片）
    pub state_snapshot: Option<RLStateVector>,
    /// RL 动作记录（None = 非 RL 卡片）
    pub action_record: Option<RLActionVector>,
    /// 观察（生成上下文）
    pub observation: Option<Box<str>>,
    /// 反思（自我评估文本）
    pub reflection: Option<Box<str>>,
    /// 价值信号（None = 未评估）
    pub value: Option<f32>,
    /// 创建时间（Unix 毫秒）
    pub created_at: u64,
    /// 更新时间（Unix 毫秒）
    pub updated_at: u64,
}

impl AtomicMemoryCard {
    /// 创建原子记忆卡片
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        card_id: &str,
        card_type: AtomicCardType,
        priority: u8,
        scene: &str,
        content: &str,
        source_traj_id: Option<&str>,
        observation: Option<&str>,
        reflection: Option<&str>,
        value: Option<f32>,
        created_at: u64,
    ) -> Self {
        Self {
            card_id: Box::from(card_id),
            card_type,
            priority,
            scene: Box::from(scene),
            content: Box::from(content),
            source_traj_id: source_traj_id.map(Box::from),
            state_snapshot: None,
            action_record: None,
            observation: observation.map(Box::from),
            reflection: reflection.map(Box::from),
            value,
            created_at,
            updated_at: created_at,
        }
    }

    /// 关联 RL 状态/动作快照（RL 卡片专用装配）
    pub fn with_rl_snapshot(mut self, state: RLStateVector, action: RLActionVector) -> Self {
        self.state_snapshot = Some(state);
        self.action_record = Some(action);
        self
    }

    /// 是否为 RL 卡片（携带状态/动作快照）
    pub fn is_rl_card(&self) -> bool {
        self.state_snapshot.is_some() && self.action_record.is_some()
    }
}

// ============================================================
// L2: 场景档案
// ============================================================

/// 场景档案 — L2 场景级记忆聚合（TencentDB + MSCE 融合）
///
/// 聚合同场景的原子卡片 + MSCE 四元组技能要素
/// （触发器 φ / 执行过程 π / 验证条件 κ / 边界条件 β / 增益 γ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneBlock {
    /// 场景块 ID
    pub block_id: Box<str>,
    /// 场景名称（检索键）
    pub scene_name: Box<str>,
    /// 聚合的原子卡片 ID 列表
    pub cards: Vec<Box<str>>,
    /// 场景摘要
    pub summary: Box<str>,
    /// 热度值（访问频率指数，越高越热）
    pub heat_value: u32,
    /// MSCE 触发器（何时使用该技能）
    pub trigger_phi: Option<Box<str>>,
    /// MSCE 执行过程（如何执行）
    pub procedure_pi: Option<Box<str>>,
    /// MSCE 验证条件（如何验证成功）
    pub verification_kappa: Option<Box<str>>,
    /// MSCE 边界条件（何时不适用）
    pub boundary_beta: Option<Box<str>>,
    /// MSCE 增益（历史收益估计）
    pub gain_gamma: Option<f32>,
}

impl SceneBlock {
    /// 创建场景档案
    pub fn new(block_id: &str, scene_name: &str, cards: Vec<Box<str>>, summary: &str) -> Self {
        Self {
            block_id: Box::from(block_id),
            scene_name: Box::from(scene_name),
            cards,
            summary: Box::from(summary),
            heat_value: 0,
            trigger_phi: None,
            procedure_pi: None,
            verification_kappa: None,
            boundary_beta: None,
            gain_gamma: None,
        }
    }

    /// 关联 MSCE 四元组技能要素
    #[allow(clippy::too_many_arguments)]
    pub fn with_msce_elements(
        mut self,
        trigger_phi: &str,
        procedure_pi: &str,
        verification_kappa: &str,
        boundary_beta: &str,
        gain_gamma: f32,
    ) -> Self {
        self.trigger_phi = Some(Box::from(trigger_phi));
        self.procedure_pi = Some(Box::from(procedure_pi));
        self.verification_kappa = Some(Box::from(verification_kappa));
        self.boundary_beta = Some(Box::from(boundary_beta));
        self.gain_gamma = Some(gain_gamma);
        self
    }
}

// ============================================================
// L3: 人格摘要
// ============================================================

/// 人格摘要 — L3 用户级长期画像（TencentDB L3）
///
/// 每次对话注入的高频记忆（TencentDB 注入策略：系统提示末尾），
/// 由偏好与规则聚合提炼而成。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonaSummary {
    /// 人格 ID
    pub persona_id: Box<str>,
    /// 用户 ID
    pub user_id: Box<str>,
    /// 人格摘要文本
    pub summary: Box<str>,
    /// 偏好列表（结构化）
    pub preferences: Vec<Box<str>>,
    /// 规则列表（结构化）
    pub rules: Vec<Box<str>>,
    /// 创建时间（Unix 毫秒）
    pub created_at: u64,
    /// 更新时间（Unix 毫秒）
    pub updated_at: u64,
}

impl PersonaSummary {
    /// 创建人格摘要
    pub fn new(
        persona_id: &str,
        user_id: &str,
        summary: &str,
        preferences: Vec<Box<str>>,
        rules: Vec<Box<str>>,
        created_at: u64,
    ) -> Self {
        Self {
            persona_id: Box::from(persona_id),
            user_id: Box::from(user_id),
            summary: Box::from(summary),
            preferences,
            rules,
            created_at,
            updated_at: created_at,
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 层级与映射 ----------

    #[test]
    fn pyramid_level_closed_enum() {
        let all = [
            MemoryPyramidLevel::L0RawLog,
            MemoryPyramidLevel::L1AtomicMemory,
            MemoryPyramidLevel::L2SceneBlock,
            MemoryPyramidLevel::L3Persona,
        ];
        assert_eq!(all.len(), 4);
        // 层级数值单调递增（金字塔自底向上）
        let values: Vec<u8> = all.iter().map(|l| l.level_value()).collect();
        assert_eq!(values, vec![0, 1, 2, 3]);
    }

    #[test]
    fn pyramid_level_archive_mapping() {
        // 静态映射: 层级 → 存储温度基线
        assert_eq!(
            ArchiveTier::from(MemoryPyramidLevel::L0RawLog),
            ArchiveTier::Cold
        );
        assert_eq!(
            ArchiveTier::from(MemoryPyramidLevel::L1AtomicMemory),
            ArchiveTier::Warm
        );
        assert_eq!(
            ArchiveTier::from(MemoryPyramidLevel::L2SceneBlock),
            ArchiveTier::Warm
        );
        assert_eq!(
            ArchiveTier::from(MemoryPyramidLevel::L3Persona),
            ArchiveTier::Hot
        );
    }

    #[test]
    fn pyramid_level_wire_format_frozen() {
        let json =
            serde_json::to_string(&MemoryPyramidLevel::L1AtomicMemory).expect("JSON 序列化失败");
        assert_eq!(json, "\"l1_atomic_memory\"");
    }

    // ---------- 原子记忆卡片 ----------

    #[test]
    fn atomic_card_json_roundtrip() {
        let card = AtomicMemoryCard::new(
            "card-1",
            AtomicCardType::Preference,
            200,
            "code-review",
            "用户偏好简洁注释",
            Some("traj-1"),
            Some("观察到长注释"),
            Some("应精简"),
            Some(0.8),
            1_700_000_000_000,
        );
        let json = serde_json::to_string(&card).expect("JSON 序列化失败");
        let decoded: AtomicMemoryCard = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, card);
    }

    #[test]
    fn atomic_card_msgpack_roundtrip() {
        let card = AtomicMemoryCard::new(
            "card-2",
            AtomicCardType::Trace,
            100,
            "debug",
            "双指针定位法",
            None,
            None,
            None,
            None,
            1_700_000_000_000,
        );
        let bytes = rmp_serde::to_vec(&card).expect("MsgPack 序列化失败");
        let decoded: AtomicMemoryCard =
            rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化失败");
        assert_eq!(decoded, card);
    }

    #[test]
    fn atomic_card_rl_snapshot_assembly() {
        // RL 卡片装配: 状态/动作快照携带（训练数据面回溯）
        let state = RLStateVector::zeros();
        let action = RLActionVector::new("S2", 0, vec![0.3]);
        let card = AtomicMemoryCard::new(
            "card-3",
            AtomicCardType::Policy,
            150,
            "routing",
            "S2 记忆策略",
            None,
            None,
            None,
            Some(0.6),
            1_700_000_000_000,
        )
        .with_rl_snapshot(state, action);
        assert!(card.is_rl_card());
        // 非 RL 卡片
        let plain = AtomicMemoryCard::new(
            "card-4",
            AtomicCardType::Event,
            10,
            "event",
            "事件记录",
            None,
            None,
            None,
            None,
            1_700_000_000_000,
        );
        assert!(!plain.is_rl_card());
    }

    #[test]
    fn atomic_card_type_exhaustive() {
        // 六类卡片类型闭集（TencentDB 3 + MSCE 3）
        let all = [
            AtomicCardType::Preference,
            AtomicCardType::Event,
            AtomicCardType::Rule,
            AtomicCardType::Trace,
            AtomicCardType::Policy,
            AtomicCardType::EnvCognition,
        ];
        assert_eq!(all.len(), 6);
    }

    #[test]
    fn atomic_card_wire_format_frozen() {
        let card = AtomicMemoryCard::new(
            "card-1",
            AtomicCardType::Policy,
            150,
            "routing",
            "内容",
            None,
            None,
            None,
            None,
            1_700_000_000_000,
        );
        let json = serde_json::to_string(&card).expect("JSON 序列化失败");
        assert!(json.contains("\"card_type\":\"policy\""));
        assert!(json.contains("\"priority\":150"));
    }

    // ---------- 场景档案 ----------

    #[test]
    fn scene_block_roundtrip() {
        let block = SceneBlock::new(
            "scene-1",
            "code-review",
            vec![Box::from("card-1"), Box::from("card-2")],
            "代码评审场景",
        )
        .with_msce_elements(
            "当提交代码时",
            "运行评审流程",
            "评审通过",
            "非生产代码",
            0.9,
        );
        let json = serde_json::to_string(&block).expect("JSON 序列化失败");
        let decoded: SceneBlock = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, block);
        assert_eq!(decoded.cards.len(), 2);
        assert_eq!(decoded.trigger_phi.as_deref(), Some("当提交代码时"));
        assert_eq!(decoded.gain_gamma, Some(0.9));
    }

    #[test]
    fn scene_block_msce_elements_optional() {
        // MSCE 四元组可缺席（纯 TencentDB 场景）
        let plain = SceneBlock::new("scene-2", "report", vec![], "报告场景");
        assert!(plain.trigger_phi.is_none());
        assert!(plain.gain_gamma.is_none());
    }

    // ---------- 人格摘要 ----------

    #[test]
    fn persona_summary_roundtrip() {
        let persona = PersonaSummary::new(
            "persona-1",
            "user-1",
            "偏好简洁高效的代码风格",
            vec![Box::from("简洁注释"), Box::from("模块化")],
            vec![Box::from("不引入未要求的抽象")],
            1_700_000_000_000,
        );
        let json = serde_json::to_string(&persona).expect("JSON 序列化失败");
        let decoded: PersonaSummary = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, persona);
        assert_eq!(decoded.preferences.len(), 2);
        assert_eq!(decoded.updated_at, decoded.created_at);
    }
}
