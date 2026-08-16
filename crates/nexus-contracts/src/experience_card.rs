//! 经验卡片契约 — OpenMLE 核心数据结构（设计文档 §5.2）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §5.2
//! 对应论文: 清华 Frontis-MA1 OpenMLE（经验卡片 + 三因子评估）+ Dressage（Token 级证据）
//!
//! # 核心职责
//!
//! 承载 OpenMLE 经验卡片体系的跨层契约类型，使 L1-L7 全链路能按统一契约
//! 生成/索引/消费结构化经验：
//!
//! | 类型 | 职责 | 消费层 |
//! |------|------|--------|
//! | [`ExperienceCard`] | 单次执行节点的结构化经验卡片 | L1 event-bus / L7 PVL |
//! | [`AtomicOperator`] | 四套原子算子（Draft/Improve/Debug/Crossover） | L5 four-operators / L7 atomic-operators |
//! | [`ThreeFactorScore`] | 三因子评分（Quality + Progress + Novelty） | L5 three-factor-selector / L8 裁决 |
//! | [`ErrorSignature`] | 结构化错误签名（支持哈希去重与聚类，铁律7） | L4 error-signature-collector |
//! | [`ExecutionStatus`] | 六类执行状态反馈（铁律8 全链路追踪） | L7 PVL / L4 六类反馈集成 |
//! | [`CardMetadata`] | 卡片执行元数据（耗时/Token/技能/环境） | L10 经验卡片可视化 |
//!
//! # 设计约束（ADR-033）
//!
//! - **纯类型 + 纯函数**: 仅类型定义与无副作用判定函数（输入确定则输出确定，铁律4）
//! - **零 crate 依赖**: 仅 `serde` derive + 同层 `omni_message::TokenUsage` 复用
//!   （遵循 RetryPolicy 复用先例，避免重复定义）
//! - **不可变契约**: 经验卡片写入后不可变（铁律3）——本模块不提供任何
//!   `&mut` 变更方法；版本化更新 = 基于旧卡片构造新卡片（`with_*` 模式）
//! - **f32 字段仅 `PartialEq`**: score/delta/quality/progress/novelty 为浮点字段，
//!   禁止 derive `Eq`/`Hash`（浮点比较红线）
//! - **`Box<str>` 优化**: 不可变文本字段用 `Box<str>`（省去容量字段 8 bytes/字符串，
//!   遵循 omni_message.rs JSON 字段先例），高频卡片场景显著降低内存占用
//!
//! # ADR-033 例外声明（第 4 个明确例外）
//!
//! 本模块承载三因子评分纯函数（`selection_utility` / `normalize`）与状态判定
//! 纯函数（`is_retryable` / `generates_meaningful_card` 等），与
//! `archive_monotonicity`（例外 2）同类——均为无 IO 无状态变更的纯函数，
//! 不违反"纯类型零逻辑"约束边界。
//!
//! # 与 ErrorSignature 哈希的关系
//!
//! 规范原型要求 `ErrorSignature::compute_hash`（SHA-256），但 L0 零 crate 依赖
//! 铁律禁止引入 `sha2`。**决策**: `error_hash` 仅作为承载字段，哈希计算由
//! 消费方（L4 `error-signature-collector` / L5 `gsoe-evolution`）完成，
//! L0 仅保证字段标准化（前 16 位十六进制字符串约定）。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// 复用 omni_message 的 TokenUsage（L0 同层引用，避免重复定义）
pub use crate::omni_message::TokenUsage;

// ============================================================
// 原子算子
// ============================================================

/// 四套原子算子 — OpenMLE 算子化抽象
///
/// 所有代码生成/修改/修复/融合操作必须映射到这四个算子之一
/// （架构设计第一性原理 2：算子化抽象）。
///
/// WHY 闭集枚举: 算子集合是封闭的，枚举提供编译期穷尽检查。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtomicOperator {
    /// 从零起草 — 生成全新代码
    Draft,
    /// 迭代改进 — 在现有代码上优化
    Improve,
    /// 错误修复 — 定位并修复缺陷
    Debug,
    /// 代码融合 — 交叉合并多个方案
    Crossover,
}

impl AtomicOperator {
    /// 是否为生成型算子（产出全新代码）
    pub fn is_generative(&self) -> bool {
        matches!(self, AtomicOperator::Draft | AtomicOperator::Crossover)
    }

    /// 是否为修改型算子（在既有代码上操作）
    pub fn is_modifying(&self) -> bool {
        matches!(self, AtomicOperator::Improve | AtomicOperator::Debug)
    }

    /// 默认 Token 消耗估算 — 供 L6 预算控制与 L7 执行规划使用
    ///
    /// 经验值来自 OpenMLE 实证：Draft 最长、Debug 最短。
    pub fn default_token_estimate(&self) -> usize {
        match self {
            AtomicOperator::Draft => 5000,
            AtomicOperator::Improve => 3000,
            AtomicOperator::Debug => 2000,
            AtomicOperator::Crossover => 4000,
        }
    }
}

// ============================================================
// 三因子评分
// ============================================================

/// 三因子评分 — OpenMLE 核心评估体系
///
/// 三因子（Quality + Progress + Novelty）共同决定父本选择，避免单一分数
/// 采样丢失潜力分支。**铁律4**: 所有方法为纯函数，无副作用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreeFactorScore {
    /// 绝对质量 — 如验证通过率（0.0-1.0）
    pub quality: f32,
    /// 相对父本的改进幅度（可为负，表示退化）
    pub progress: f32,
    /// 方法新颖性 — 避免重复相同路径（0.0-1.0）
    pub novelty: f32,
}

impl ThreeFactorScore {
    /// 选择效用综合分 — 三因子加权求和
    ///
    /// 用于父本选择的快速排序键；无需归一化即可比较同任务内卡片。
    pub fn selection_utility(&self) -> f32 {
        self.quality + self.progress + self.novelty
    }

    /// 归一化三因子 — 按任务内最大值缩放至 [0,1] 区间
    ///
    /// 除零保护: `max(1e-8)` 避免 max 值为 0 时产生 NaN/Inf。
    pub fn normalize(&self, max_q: f32, max_p: f32, max_n: f32) -> NormalizedThreeFactor {
        NormalizedThreeFactor {
            quality: self.quality / max_q.max(1e-8),
            progress: self.progress / max_p.max(1e-8),
            novelty: self.novelty / max_n.max(1e-8),
        }
    }

    /// 根节点默认三因子 — 搜索树根无父本可比较
    ///
    /// Quality/Progress 为 0（无参照），Novelty 为 1（新路径必然新颖）。
    pub fn default_root() -> Self {
        Self {
            quality: 0.0,
            progress: 0.0,
            novelty: 1.0,
        }
    }
}

/// 归一化三因子 — 三因子评分归一化后的传输形态
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedThreeFactor {
    /// 归一化质量分（[0,1]）
    pub quality: f32,
    /// 归一化改进分（[0,1]）
    pub progress: f32,
    /// 归一化新颖分（[0,1]）
    pub novelty: f32,
}

// ============================================================
// 错误签名
// ============================================================

/// 结构化错误签名 — 支持哈希去重与聚类（铁律7）
///
/// 承载错误分类与定位信息；`error_hash` 为消费方计算的哈希
/// （约定 SHA-256 前 16 位十六进制），L0 仅标准化承载。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ErrorSignature {
    /// 错误类型 — 如 "compile_error" / "test_failure" / "timeout"
    pub error_type: Box<str>,
    /// 错误位置 — 如 "src/foo.rs:42" 或模块路径
    pub error_location: Box<str>,
    /// 错误摘要 — 人类可读的错误概述
    pub error_summary: Box<str>,
    /// 错误哈希 — SHA-256 前 16 位（由消费方计算，L0 零依赖铁律）
    pub error_hash: Box<str>,
}

// ============================================================
// 执行状态
// ============================================================

/// 六类执行状态反馈 — 全链路追踪（铁律8）
///
/// OpenMLE 六类状态必须从生成到评分全链路追踪，是 L4 AutoBuilder
/// 六类状态反馈与 L7 经验卡片生成器的共同契约。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    /// 成功 — 执行并评分通过
    Success,
    /// 执行错误 — 运行期失败
    Error,
    /// 未生成代码 — 模型未产出可执行代码
    MissingCode,
    /// 未提交 — 未提交评分
    NoSubmit,
    /// 评分失败 — 验证环境无法评分
    ScoreFailed,
    /// 超时 — 超出执行时限
    Timeout,
}

impl ExecutionStatus {
    /// 是否可重试 — Error/Timeout/ScoreFailed 为可恢复状态
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ExecutionStatus::Error | ExecutionStatus::Timeout | ExecutionStatus::ScoreFailed
        )
    }

    /// 是否生成有意义的卡片 — Success/Error/Timeout 产生可复用经验
    ///
    /// MissingCode/NoSubmit 无实质执行信息，ScoreFailed 无有效评分，
    /// 这三类生成的卡片不应进入经验库（避免噪声稀释）。
    pub fn generates_meaningful_card(&self) -> bool {
        matches!(
            self,
            ExecutionStatus::Success | ExecutionStatus::Error | ExecutionStatus::Timeout
        )
    }
}

// ============================================================
// 卡片元数据
// ============================================================

/// 卡片元数据 — 执行耗时/Token 消耗/技能/环境
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardMetadata {
    /// 执行耗时（毫秒）
    pub execution_time_ms: u64,
    /// Token 消耗（复用 omni_message::TokenUsage，u64 防溢出）
    pub token_usage: TokenUsage,
    /// 变更行数（负值表示净删除）
    pub lines_changed: i32,
    /// 使用的技能 ID 列表
    pub skills_used: Vec<Box<str>>,
    /// 执行环境信息
    pub environment: EnvironmentInfo,
}

/// 环境信息 — 卡片生成时的运行环境快照
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    /// Rust 版本（如 "1.85.0"）
    pub rust_version: Box<str>,
    /// 操作系统（如 "windows" / "linux"）
    pub os: Box<str>,
    /// CPU 架构（如 "x86_64" / "aarch64"）
    pub cpu_arch: Box<str>,
    /// Chimera 版本（如 "2.26.0-omega"）
    pub chimera_version: Box<str>,
}

impl Default for EnvironmentInfo {
    fn default() -> Self {
        Self {
            rust_version: Box::from(""),
            os: Box::from(""),
            cpu_arch: Box::from(""),
            chimera_version: Box::from(""),
        }
    }
}

impl Default for CardMetadata {
    fn default() -> Self {
        Self {
            execution_time_ms: 0,
            token_usage: TokenUsage::new(0, 0),
            lines_changed: 0,
            skills_used: Vec::new(),
            environment: EnvironmentInfo::default(),
        }
    }
}

// ============================================================
// 经验卡片
// ============================================================

/// 经验卡片 — OpenMLE 核心数据结构，Chimera Event Bus 一级公民
///
/// 每个执行节点生成一张卡片，记录结构化经验。融合 Dressage Token 级证据：
/// `token_evidence_ids` 关联 TokenLedgerEntry，`segment_id` 关联轨迹分段，
/// 形成"经验 - 证据"完整闭环。
///
/// # 不可变契约（铁律3）
///
/// 经验卡片**写入后不可变**——本类型不提供 `&mut` 变更方法；
/// 版本化更新通过构造新卡片实现（保留旧版本用于审计与回滚）。
/// 集成测试 `test_experience_card_immutability_contract` 守护该约定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperienceCard {
    /// 卡片 ID（约定 UUIDv7，由消费方生成）
    pub card_id: Box<str>,
    /// 所属任务 ID
    pub task_id: Box<str>,
    /// 执行节点 ID（搜索树节点唯一标识）
    pub node_id: Box<str>,
    /// 父节点卡片 ID（None = 根节点）
    pub parent_id: Option<Box<str>>,
    /// 创建时间（UTC）
    pub created_at: DateTime<Utc>,
    /// 执行的原子算子
    pub operator: AtomicOperator,
    /// 绝对评分（0.0-1.0）
    pub score: f32,
    /// 相对父节点的改进幅度（可为负）
    pub delta_vs_parent: f32,
    /// 方法家族（如 "draft_pipeline" / "two_pass_debug"）
    pub method_family: Box<str>,
    /// 错误签名（执行失败时 Some）
    pub error_signature: Option<ErrorSignature>,
    /// 三因子评分
    pub three_factor: ThreeFactorScore,
    /// 六类执行状态
    pub execution_status: ExecutionStatus,
    /// 关联的 TokenLedgerEntry IDs（Dressage 证据链）
    pub token_evidence_ids: Vec<Box<str>>,
    /// 关联的 Segment ID（Dressage 轨迹分段）
    pub segment_id: Option<Box<str>>,
    /// 卡片元数据
    pub metadata: CardMetadata,
}

impl ExperienceCard {
    /// 基于当前卡片创建新版本（不可变契约的版本化更新入口）
    ///
    /// 保留 `self` 原样，返回以 `new_card_id` 标识的新卡片；调用方负责
    /// 归档旧版本。仅允许更新 `score` / `three_factor` / `execution_status`
    /// 三个评估维度字段——其余字段为一次写入的固有属性。
    #[allow(clippy::too_many_arguments)]
    pub fn updated_assessment(
        &self,
        new_card_id: &str,
        score: f32,
        three_factor: ThreeFactorScore,
        execution_status: ExecutionStatus,
        error_signature: Option<ErrorSignature>,
        token_evidence_ids: Vec<Box<str>>,
        segment_id: Option<Box<str>>,
    ) -> Self {
        Self {
            card_id: Box::from(new_card_id),
            task_id: self.task_id.clone(),
            node_id: self.node_id.clone(),
            parent_id: Some(self.card_id.clone()),
            created_at: self.created_at,
            operator: self.operator,
            score,
            delta_vs_parent: score - self.score,
            method_family: self.method_family.clone(),
            error_signature,
            three_factor,
            execution_status,
            token_evidence_ids,
            segment_id,
            metadata: self.metadata.clone(),
        }
    }

    /// 是否为根卡片（无父节点）
    pub fn is_root(&self) -> bool {
        self.parent_id.is_none()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- AtomicOperator ----------

    #[test]
    fn atomic_operator_generative_classification() {
        assert!(AtomicOperator::Draft.is_generative());
        assert!(AtomicOperator::Crossover.is_generative());
        assert!(!AtomicOperator::Improve.is_generative());
        assert!(!AtomicOperator::Debug.is_generative());
    }

    #[test]
    fn atomic_operator_modifying_classification() {
        assert!(AtomicOperator::Improve.is_modifying());
        assert!(AtomicOperator::Debug.is_modifying());
        assert!(!AtomicOperator::Draft.is_modifying());
        assert!(!AtomicOperator::Crossover.is_modifying());
    }

    #[test]
    fn atomic_operator_token_estimate_ordering() {
        // 经验排序: Draft 最长 > Crossover > Improve > Debug 最短
        assert!(
            AtomicOperator::Draft.default_token_estimate()
                > AtomicOperator::Crossover.default_token_estimate()
        );
        assert!(
            AtomicOperator::Crossover.default_token_estimate()
                > AtomicOperator::Improve.default_token_estimate()
        );
        assert!(
            AtomicOperator::Improve.default_token_estimate()
                > AtomicOperator::Debug.default_token_estimate()
        );
    }

    #[test]
    fn atomic_operator_exhaustive_closure() {
        // 闭集枚举穷尽性: 全变体必须可遍历（编译期 + 运行期双验证）
        let all = [
            AtomicOperator::Draft,
            AtomicOperator::Improve,
            AtomicOperator::Debug,
            AtomicOperator::Crossover,
        ];
        assert_eq!(all.len(), 4);
        assert!(all.iter().all(|op| op.default_token_estimate() > 0));
    }

    // ---------- ThreeFactorScore（铁律4: 纯函数） ----------

    #[test]
    fn three_factor_selection_utility_is_sum() {
        let s = ThreeFactorScore {
            quality: 0.6,
            progress: 0.2,
            novelty: 0.5,
        };
        assert!((s.selection_utility() - 1.3).abs() < f32::EPSILON);
    }

    #[test]
    fn three_factor_normalize_scales_to_unit() {
        let s = ThreeFactorScore {
            quality: 0.5,
            progress: 0.25,
            novelty: 0.75,
        };
        let n = s.normalize(1.0, 0.5, 1.5);
        assert!((n.quality - 0.5).abs() < 1e-6);
        assert!((n.progress - 0.5).abs() < 1e-6);
        assert!((n.novelty - 0.5).abs() < 1e-6);
    }

    #[test]
    fn three_factor_normalize_zero_max_guard() {
        // 除零保护: max 为 0 时输出 0（而非 NaN/Inf）
        let s = ThreeFactorScore {
            quality: 0.0,
            progress: 0.0,
            novelty: 0.0,
        };
        let n = s.normalize(0.0, 0.0, 0.0);
        assert!(n.quality.is_finite() && n.quality == 0.0);
        assert!(n.progress.is_finite() && n.progress == 0.0);
        assert!(n.novelty.is_finite() && n.novelty == 0.0);
    }

    #[test]
    fn three_factor_root_default() {
        let root = ThreeFactorScore::default_root();
        assert_eq!(root.quality, 0.0);
        assert_eq!(root.progress, 0.0);
        assert_eq!(root.novelty, 1.0);
    }

    #[test]
    fn three_factor_is_pure_function() {
        // 铁律4: 输入确定则输出确定（同一输入重复调用结果一致）
        let s = ThreeFactorScore {
            quality: 0.7,
            progress: 0.1,
            novelty: 0.4,
        };
        assert_eq!(s.selection_utility(), s.selection_utility());
        let n1 = s.normalize(1.0, 1.0, 1.0);
        let n2 = s.normalize(1.0, 1.0, 1.0);
        assert_eq!(n1, n2);
    }

    // ---------- ExecutionStatus（铁律8: 六类状态） ----------

    #[test]
    fn execution_status_six_variants_closed() {
        let all = [
            ExecutionStatus::Success,
            ExecutionStatus::Error,
            ExecutionStatus::MissingCode,
            ExecutionStatus::NoSubmit,
            ExecutionStatus::ScoreFailed,
            ExecutionStatus::Timeout,
        ];
        assert_eq!(all.len(), 6);
    }

    #[test]
    fn execution_status_retryable_classification() {
        assert!(ExecutionStatus::Error.is_retryable());
        assert!(ExecutionStatus::Timeout.is_retryable());
        assert!(ExecutionStatus::ScoreFailed.is_retryable());
        assert!(!ExecutionStatus::Success.is_retryable());
        assert!(!ExecutionStatus::MissingCode.is_retryable());
        assert!(!ExecutionStatus::NoSubmit.is_retryable());
    }

    #[test]
    fn execution_status_meaningful_card_classification() {
        assert!(ExecutionStatus::Success.generates_meaningful_card());
        assert!(ExecutionStatus::Error.generates_meaningful_card());
        assert!(ExecutionStatus::Timeout.generates_meaningful_card());
        assert!(!ExecutionStatus::MissingCode.generates_meaningful_card());
        assert!(!ExecutionStatus::NoSubmit.generates_meaningful_card());
        assert!(!ExecutionStatus::ScoreFailed.generates_meaningful_card());
    }

    // ---------- 序列化 roundtrip ----------

    fn sample_card() -> ExperienceCard {
        ExperienceCard {
            card_id: Box::from("card-001"),
            task_id: Box::from("task-1"),
            node_id: Box::from("node-1"),
            parent_id: None,
            created_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("合法时间戳"),
            operator: AtomicOperator::Draft,
            score: 0.85,
            delta_vs_parent: 0.0,
            method_family: Box::from("draft_pipeline"),
            error_signature: None,
            three_factor: ThreeFactorScore {
                quality: 0.85,
                progress: 0.0,
                novelty: 1.0,
            },
            execution_status: ExecutionStatus::Success,
            token_evidence_ids: vec![Box::from("ledger-1")],
            segment_id: Some(Box::from("seg-1")),
            metadata: CardMetadata::default(),
        }
    }

    #[test]
    fn experience_card_json_roundtrip() {
        let card = sample_card();
        let json = serde_json::to_string(&card).expect("JSON 序列化失败");
        let decoded: ExperienceCard = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, card);
    }

    #[test]
    fn experience_card_msgpack_roundtrip() {
        let card = sample_card();
        let bytes = rmp_serde::to_vec(&card).expect("MsgPack 序列化失败");
        let decoded: ExperienceCard = rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化失败");
        assert_eq!(decoded, card);
    }

    #[test]
    fn experience_card_wire_format_frozen() {
        // 线格式冻结: 关键字段的 JSON 形态不可漂移（破坏即破坏跨进程兼容）
        let card = sample_card();
        let json = serde_json::to_string(&card).expect("JSON 序列化失败");
        assert!(json.contains("\"card_id\":\"card-001\""));
        assert!(json.contains("\"operator\":\"draft\""));
        assert!(json.contains("\"execution_status\":\"success\""));
        assert!(json.contains("\"token_evidence_ids\":[\"ledger-1\"]"));
    }

    // ---------- 不可变契约（铁律3） ----------

    #[test]
    fn experience_card_immutability_contract() {
        // 版本化更新: 旧卡片保持不变，新卡片携带父指针 + 正确 delta
        let original = sample_card();
        let updated = original.updated_assessment(
            "card-002",
            0.92,
            ThreeFactorScore {
                quality: 0.92,
                progress: 0.07,
                novelty: 0.6,
            },
            ExecutionStatus::Success,
            None,
            vec![Box::from("ledger-1"), Box::from("ledger-2")],
            Some(Box::from("seg-2")),
        );
        // 原卡片不可变（字段原样）
        assert_eq!(original.card_id.as_ref(), "card-001");
        assert_eq!(original.score, 0.85);
        // 新卡片: 新 ID + 父指针 + 正确 delta（0.92 - 0.85 = 0.07）
        assert_eq!(updated.card_id.as_ref(), "card-002");
        assert_eq!(updated.parent_id.as_deref(), Some("card-001"));
        assert!((updated.delta_vs_parent - 0.07).abs() < f32::EPSILON);
        assert_eq!(updated.token_evidence_ids.len(), 2);
    }

    #[test]
    fn experience_card_root_detection() {
        let root = sample_card();
        assert!(root.is_root());
        let child = root.updated_assessment(
            "card-002",
            0.9,
            ThreeFactorScore {
                quality: 0.9,
                progress: 0.05,
                novelty: 0.5,
            },
            ExecutionStatus::Success,
            None,
            Vec::new(),
            None,
        );
        assert!(!child.is_root());
    }

    #[test]
    fn error_signature_hash_carrier() {
        // error_hash 为承载字段（消费方计算），L0 仅保证标准化
        let sig = ErrorSignature {
            error_type: Box::from("compile_error"),
            error_location: Box::from("src/lib.rs:42"),
            error_summary: Box::from("未定义标识符 foo"),
            error_hash: Box::from("0123456789abcdef"),
        };
        assert_eq!(sig.error_hash.len(), 16);
        // 哈希参与 Eq/Hash 派生 → 可作去重键（铁律7）
        let mut seen = std::collections::HashSet::new();
        seen.insert(sig.clone());
        assert!(seen.contains(&sig));
    }
}
