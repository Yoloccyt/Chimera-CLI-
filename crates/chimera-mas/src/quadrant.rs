//! 孙代理四象限稳定分工模型 (§3 CHIMERA-MAS-Q / ADR-027 决策 1-3)
//!
//! 本模块把「无界扇出的同质孙代理」收敛为**固定四象限稳定分工**,回应
//! 三重悖论文档的「推理悖论」(协调成本失控 + 职责漂移)。理论根基是代码库
//! 自有的 PVL 生产-验证闭环(L7 pvl-layer): X 轴对应 Producer/Verifier 对立。
//!
//! ## 二维坐标系 (§3.2)
//!
//! - **X 轴 Produce↔Assure**: 生产侧产出制品, 保障侧验证与加固制品。
//! - **Y 轴 Core↔Cross-cutting**: 核心侧聚焦主业务逻辑, 横切侧聚焦跨模块关注点。
//!
//! ```text
//!                     Core(核心)
//!                         ▲
//!       Q1 实现象限        │        Q3 验证象限
//!       (Produce×Core)     │        (Assure×Core)
//!   Produce ──────────────┼──────────────► Assure
//!       Q2 集成象限        │        Q4 加固象限
//!       (Produce×Cross)    │        (Assure×Cross)
//!                         ▼
//!                 Cross-cutting(横切)
//! ```
//!
//! ## 稳定性三重保证 (§3.5)
//!
//! 1. **角色固定**: 四象限是恒定角色集合, 不随任务内容变化。
//! 2. **上界固定**: 孙代理扇出恒 `≤ 4`(INV-3/INV-4), 杜绝无界委托。
//! 3. **映射固定**: 象限 ↔ 六维质量 ↔ 三步验证 存在稳定映射(§3.6)。
//!
//! ## 与 AgentType 的关系 (ADR-027 决策 2)
//!
//! 象限**不修改** `AgentType` 五变体, 而是通过 `task_scope` 尾缀 `#Q1..#Q4`
//! 编码(如 `"refactor-parser#Q3"`), 与 AgentType 解耦, 保证向后兼容。

use crate::delegation::TaskComplexity;
use crate::error::{MasError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// 孙代理扇出上界 (INV-3) — 四象限, 恒为 4。
///
/// WHY 恒为 4 而非跟随 `sub_agent_count`: 四象限是 2×2 矩阵, 第 5 个象限无语义归属。
/// `VeryComplex` 的第 5 并行度由象限内子任务承载, 而非新增第 5 种象限(ADR-027 决策 3)。
pub const MAX_QUADRANT_FANOUT: usize = 4;

/// 生产/保障轴 (X 轴, §3.2) — 对应 PVL 的 Producer / Verifier 对立。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProduceAssure {
    /// 生产侧 — 产出制品(功能代码 / 接口适配)。
    Produce,
    /// 保障侧 — 验证与加固制品(测试 / 安全 / 性能)。
    Assure,
}

/// 核心/横切轴 (Y 轴, §3.2)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CoreCross {
    /// 核心侧 — 聚焦主业务逻辑。
    Core,
    /// 横切侧 — 聚焦跨模块关注点(接口 / 依赖 / 安全 / 性能 / 文档)。
    CrossCutting,
}

/// 六维质量维度 (§10) — 用于象限 → 质量的稳定映射 (§3.6)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QualityDimension {
    /// D1 清晰且模块化的逻辑结构(单一职责 / 开闭 / 依赖倒置)。
    D1ModularLogic,
    /// D2 高可读性与可维护性(命名规范 / 函数长度受控 / 架构清晰)。
    D2Readability,
    /// D3 杜绝冗余与技术债(无死代码 / 复用率 / 定期重构)。
    D3NoTechDebt,
    /// D4 完善的注释说明(函数头 / 参数 / 返回 / 算法 / 业务)。
    D4Documentation,
    /// D5 编码规范最佳实践(风格统一 / 安全编码 / 性能准则)。
    D5BestPractices,
    /// D6 错误处理与异常恢复(异常捕获 / 日志规范 / 故障自愈)。
    D6ErrorHandling,
}

/// 三步验证法步骤 (§9 / §3.6) — 象限在哪一步交付。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ValidationStep {
    /// Step1 多维度系统性思考与规划(影响范围 / 依赖梳理 / 回滚预案)。
    Step1PlanImpact,
    /// Step2 详细修改方案与风险评估(技术方案 / 风险矩阵 / 测试覆盖计划)。
    Step2RiskDesign,
    /// Step3 实施代码修改(原子化提交 / 单一职责)。
    Step3AtomicImpl,
}

/// 孙代理四象限 (depth 3 执行单元的固定角色, §3.3)。
///
/// 每个象限归属唯一坐标, 主责固定的六维质量维度与三步验证法步骤,
/// 使「谁对哪一维质量负责 / 谁在哪一步交付」始终可判定、可审计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Quadrant {
    /// Q1 实现象限 Implementation (Produce×Core) — 主业务逻辑与核心算法。
    Implementation,
    /// Q2 集成象限 Integration (Produce×Cross) — 接口对接 / 依赖管理 / 兼容性适配。
    Integration,
    /// Q3 验证象限 Verification (Assure×Core) — 单元/集成测试 / 正确性验证。
    Verification,
    /// Q4 加固象限 Hardening (Assure×Cross) — 安全 / 性能基准 / 文档 / 可观测性。
    Hardening,
}

impl Quadrant {
    /// 全部四象限, 固定顺序 Q1 → Q2 → Q3 → Q4。
    ///
    /// WHY 固定顺序: 激活矩阵与孙层编排均依赖稳定顺序, 保证象限分配可预测。
    pub const ALL: [Quadrant; 4] = [
        Quadrant::Implementation,
        Quadrant::Integration,
        Quadrant::Verification,
        Quadrant::Hardening,
    ];

    /// 象限二维坐标 (X 轴生产/保障, Y 轴核心/横切, §3.2)。
    pub fn axis(&self) -> (ProduceAssure, CoreCross) {
        match self {
            Quadrant::Implementation => (ProduceAssure::Produce, CoreCross::Core),
            Quadrant::Integration => (ProduceAssure::Produce, CoreCross::CrossCutting),
            Quadrant::Verification => (ProduceAssure::Assure, CoreCross::Core),
            Quadrant::Hardening => (ProduceAssure::Assure, CoreCross::CrossCutting),
        }
    }

    /// 象限编号 (1..=4), 对应 Q1..Q4。
    pub fn index(&self) -> u8 {
        match self {
            Quadrant::Implementation => 1,
            Quadrant::Integration => 2,
            Quadrant::Verification => 3,
            Quadrant::Hardening => 4,
        }
    }

    /// 象限短标签 `"#Q1".."#Q4"` — 用于 `task_scope` 尾缀编码 (ADR-027 决策 2)。
    pub fn tag(&self) -> &'static str {
        match self {
            Quadrant::Implementation => "#Q1",
            Quadrant::Integration => "#Q2",
            Quadrant::Verification => "#Q3",
            Quadrant::Hardening => "#Q4",
        }
    }

    /// 象限人类可读名称(英文, 用于日志 / 错误信息 / 审计)。
    pub fn name(&self) -> &'static str {
        match self {
            Quadrant::Implementation => "Implementation",
            Quadrant::Integration => "Integration",
            Quadrant::Verification => "Verification",
            Quadrant::Hardening => "Hardening",
        }
    }

    /// 将象限编码进 `task_scope` 尾缀: `encode_scope("refactor") == "refactor#Q1"`。
    ///
    /// WHY 尾缀编码: 象限元信息与 `AgentType` 解耦, 不修改核心类型(ADR-027 决策 2);
    /// 与 `from_task_scope` 互为逆运算。
    pub fn encode_scope(&self, base: &str) -> String {
        format!("{base}{}", self.tag())
    }

    /// 从 `task_scope` 尾缀解析象限; 无 `#Qn` 尾缀返回 `None`。
    ///
    /// 与 `encode_scope` 互逆: `Quadrant::from_task_scope(&q.encode_scope(b)) == Some(q)`。
    pub fn from_task_scope(scope: &str) -> Option<Quadrant> {
        // WHY 遍历 ALL 而非解析数字: 象限集合恒定为 4, 遍历 O(1) 且避免数字解析错误分支。
        Quadrant::ALL.into_iter().find(|q| scope.ends_with(q.tag()))
    }

    /// 主责六维质量维度 (§3.6 稳定映射)。
    ///
    /// - Q1 实现 → D1 模块化逻辑, D2 可读可维护
    /// - Q2 集成 → D3 杜绝冗余/技术债
    /// - Q3 验证 → D6 错误处理与异常恢复
    /// - Q4 加固 → D4 注释, D5 编码规范
    pub fn quality_dimensions(&self) -> &'static [QualityDimension] {
        match self {
            Quadrant::Implementation => &[
                QualityDimension::D1ModularLogic,
                QualityDimension::D2Readability,
            ],
            Quadrant::Integration => &[QualityDimension::D3NoTechDebt],
            Quadrant::Verification => &[QualityDimension::D6ErrorHandling],
            Quadrant::Hardening => &[
                QualityDimension::D4Documentation,
                QualityDimension::D5BestPractices,
            ],
        }
    }

    /// 三步验证法归属 (§3.6 稳定映射)。
    ///
    /// - Q1 实现 / Q2 集成 → Step3 原子实施(生产)
    /// - Q3 验证 → Step2 风险评估与验证执行
    /// - Q4 加固 → Step1 影响分析与收尾加固
    pub fn validation_step(&self) -> ValidationStep {
        match self {
            Quadrant::Implementation | Quadrant::Integration => ValidationStep::Step3AtomicImpl,
            Quadrant::Verification => ValidationStep::Step2RiskDesign,
            Quadrant::Hardening => ValidationStep::Step1PlanImpact,
        }
    }
}

/// 按任务复杂度返回激活的象限集合 (§3.4 象限激活矩阵)。
///
/// 四象限是「固定角色」, 但**是否激活**由复杂度决定, 从而控制协调成本
/// (回应推理悖论「禁止默认多智能体」):
///
/// | TaskComplexity | 激活象限 | 象限数 |
/// |----------------|---------|:------:|
/// | Simple | Q1 | 1 |
/// | Medium | Q1, Q3 | 2 |
/// | Complex | Q1, Q2, Q3 | 3 |
/// | VeryComplex | Q1, Q2, Q3, Q4 | 4 |
///
/// 返回顺序始终遵循 Q1→Q2→Q3→Q4 稳定序, 且长度恒 `≤ MAX_QUADRANT_FANOUT`(INV-3)。
pub fn activated_quadrants(complexity: TaskComplexity) -> Vec<Quadrant> {
    match complexity {
        TaskComplexity::Simple => vec![Quadrant::Implementation],
        TaskComplexity::Medium => vec![Quadrant::Implementation, Quadrant::Verification],
        TaskComplexity::Complex => vec![
            Quadrant::Implementation,
            Quadrant::Integration,
            Quadrant::Verification,
        ],
        TaskComplexity::VeryComplex => vec![
            Quadrant::Implementation,
            Quadrant::Integration,
            Quadrant::Verification,
            Quadrant::Hardening,
        ],
    }
}

/// 象限分工计划 — 子代理专家对孙层的稳定分工方案, 强制 INV-3/INV-4。
///
/// - **INV-3(孙层扇出界)**: 象限数 `≤ MAX_QUADRANT_FANOUT`(4)。
/// - **INV-4(象限唯一)**: 同一计划内每个象限至多出现一次。
///
/// 通过 `from_complexity`(自动满足两不变量)或 `from_quadrants`(显式校验)构造。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuadrantPlan {
    /// 基础任务范围(不含象限尾缀), 如 `"refactor-parser"`。
    base_scope: String,
    /// 激活的象限集合(已保证 ≤4 且唯一, 遵循稳定序)。
    quadrants: Vec<Quadrant>,
}

impl QuadrantPlan {
    /// 从任务复杂度构造激活计划 — 自动满足 INV-3/INV-4。
    ///
    /// ## 参数
    /// - `base_scope`: 基础任务范围(不含象限尾缀)
    /// - `complexity`: 任务复杂度, 决定激活哪些象限(§3.4)
    pub fn from_complexity(base_scope: impl Into<String>, complexity: TaskComplexity) -> Self {
        Self {
            base_scope: base_scope.into(),
            quadrants: activated_quadrants(complexity),
        }
    }

    /// 从显式象限列表构造 — 校验 INV-3(扇出≤4) 与 INV-4(象限唯一)。
    ///
    /// ## 错误
    /// - `MasError::QuadrantFanoutExceeded`: 象限数 > 4(违反 INV-3)
    /// - `MasError::QuadrantConflict`: 存在重复象限(违反 INV-4)
    pub fn from_quadrants(base_scope: impl Into<String>, quadrants: Vec<Quadrant>) -> Result<Self> {
        // INV-3: 扇出 ≤ 4
        if quadrants.len() > MAX_QUADRANT_FANOUT {
            return Err(MasError::QuadrantFanoutExceeded {
                requested: quadrants.len(),
                max: MAX_QUADRANT_FANOUT,
            });
        }
        // INV-4: 象限唯一(检测重复, HashSet::insert 返回 false 表示已存在)
        let mut seen = std::collections::HashSet::with_capacity(quadrants.len());
        for quadrant in &quadrants {
            if !seen.insert(*quadrant) {
                return Err(MasError::QuadrantConflict {
                    quadrant: quadrant.name().to_string(),
                });
            }
        }
        Ok(Self {
            base_scope: base_scope.into(),
            quadrants,
        })
    }

    /// 象限数(= 孙代理扇出), 恒 `≤ MAX_QUADRANT_FANOUT`。
    pub fn fanout(&self) -> usize {
        self.quadrants.len()
    }

    /// 激活的象限集合(只读切片)。
    pub fn quadrants(&self) -> &[Quadrant] {
        &self.quadrants
    }

    /// 基础任务范围(不含象限尾缀)。
    pub fn base_scope(&self) -> &str {
        &self.base_scope
    }

    /// 判断某象限是否在本计划中激活。
    pub fn is_active(&self, quadrant: Quadrant) -> bool {
        self.quadrants.contains(&quadrant)
    }

    /// 生成「象限 → 编码后 task_scope」映射对, 供孙层编排逐象限委托。
    ///
    /// 例: base=`"refactor"`, 激活 [Q1,Q3] → `[(Q1,"refactor#Q1"), (Q3,"refactor#Q3")]`。
    pub fn scoped_assignments(&self) -> Vec<(Quadrant, String)> {
        self.quadrants
            .iter()
            .map(|q| (*q, q.encode_scope(&self.base_scope)))
            .collect()
    }
}

// ============================================================
// QuadrantStatus — 四象限状态静态快照（Task 3.9:L10 → L9 向下依赖）
// ============================================================

/// 四象限状态静态快照（Task 3.9:L10 → L9 向下依赖）
///
/// 为 TUI TaskManagerPanel 提供无需异步上下文的静态快照，
/// 展示四象限稳定分工（ADR-027）的 Agent 数量、任务数量与 WSJF 分数。
///
/// 真实象限状态由 `RootOrchestrator` 运行时动态更新。
///
/// # W8 假遥测治理（诚实边界）
///
/// 原实现为无参全局函数返回**全零占位**——TUI 面板将"未实现"渲染为
/// "零 Agent 零任务"的真实数据外观（假遥测，与 `five_dimension_masks`
/// 先例同构）。修复:
/// - 引入 [`QuadrantStatusProvider`] 注入接口（装配层注册真实数据源，
///   如 RuntimeAuditor 或 orchestrator 快照——先例: osa memory_strategy_provider）
/// - 未注册 provider 时 [`quadrant_status`] 返回 [`QuadrantStatus::unavailable`]
///   （数值仍为零，但语义为"数据源未接入"），TUI 经
///   [`quadrant_status_available`] 显示诚实标记而非假装真实数据
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuadrantStatus {
    /// 各象限 Agent 数量（Q1/Q2/Q3/Q4）
    pub agent_counts: [usize; 4],
    /// 各象限任务数量（Q1/Q2/Q3/Q4）
    pub task_counts: [usize; 4],
    /// 各象限 WSJF 分数（Q1/Q2/Q3/Q4）
    pub wsjf_scores: [f32; 4],
}

impl QuadrantStatus {
    /// 数据源未接入时的语义化不可用状态（数值全零,语义明确）
    ///
    /// WHY 与裸全零字面量的区别: 调用方应经 [`quadrant_status_available`]
    /// 区分"未接入"与"真实零活动",避免把占位渲染成真实数据。
    pub const fn unavailable() -> Self {
        Self {
            agent_counts: [0; 4],
            task_counts: [0; 4],
            wsjf_scores: [0.0; 4],
        }
    }
}

/// 象限状态数据源 — 装配层注入的真实快照提供者（W8 假遥测治理）
///
/// 实现方可为: RuntimeAuditor 采集器、RootOrchestrator 运行时快照、
/// 或未来的事件累积器。全局唯一注册（`OnceLock` 语义,重复注册报错）。
pub trait QuadrantStatusProvider: Send + Sync + std::fmt::Debug {
    /// 返回当前象限统计快照
    fn snapshot(&self) -> QuadrantStatus;
}

/// 全局象限状态提供者注册点（OnceLock: 首次注册生效,重复注册报错）
static QUADRANT_STATUS_PROVIDER: std::sync::OnceLock<Arc<dyn QuadrantStatusProvider>> =
    std::sync::OnceLock::new();

/// 注册象限状态提供者（装配期调用一次;重复注册返回 `MasError`）
///
/// # 示例
/// ```no_run
/// use chimera_mas::quadrant::{set_quadrant_status_provider, QuadrantStatusProvider};
/// use chimera_mas::quadrant::QuadrantStatus;
///
/// #[derive(Debug)]
/// struct SnapshotSource;
/// impl QuadrantStatusProvider for SnapshotSource {
///     fn snapshot(&self) -> QuadrantStatus { QuadrantStatus::unavailable() }
/// }
///
/// let _ = set_quadrant_status_provider(std::sync::Arc::new(SnapshotSource));
/// ```
pub fn set_quadrant_status_provider(
    provider: Arc<dyn QuadrantStatusProvider>,
) -> Result<()> {
    QUADRANT_STATUS_PROVIDER
        .set(provider)
        .map_err(|_| MasError::ProviderAlreadyRegistered {
            name: "quadrant_status".to_string(),
        })
}

/// 返回四象限状态的静态快照（W8: provider 注册后返回真实数据）
///
/// 未注册 provider 时返回 [`QuadrantStatus::unavailable`]——数值全零但语义
/// 为"数据源未接入";消费方（TUI 面板）应配合 [`quadrant_status_available`]
/// 显示诚实标记,不得把占位渲染为真实零活动。
pub fn quadrant_status() -> QuadrantStatus {
    QUADRANT_STATUS_PROVIDER
        .get()
        .map(|provider| provider.snapshot())
        .unwrap_or_else(QuadrantStatus::unavailable)
}

/// 象限状态数据源是否已接入（TUI 诚实标记依据）
pub fn quadrant_status_available() -> bool {
    QUADRANT_STATUS_PROVIDER.get().is_some()
}

// ============================================================
// QuadrantSelector — 基于任务领域语义的动态象限选择 (§3.4 扩展)
// ============================================================

/// 象限选择器 — 根据任务领域/语义动态选择激活象限
///
/// 默认实现 `ConfigurableQuadrantSelector` 使用可配置的领域→象限映射表，
/// 可替换为基于 CLV 语义相似度的动态选择器。
///
/// # 向后兼容
///
/// 未配置选择器时，系统退化为固定复杂度驱动的激活矩阵（`activated_quadrants`），
/// 保证默认行为与修改前一致。
pub trait QuadrantSelector: Send + Sync + std::fmt::Debug {
    /// 根据领域和任务描述选择激活象限
    fn select_quadrants(&self, domain: &str, description: &str) -> Vec<Quadrant>;
}

/// 基于配置映射的象限选择器
///
/// 使用领域→象限映射表，支持运行时更新。
/// 当领域不在映射表中时，使用默认象限集。
///
/// # 示例
///
/// ```
/// use chimera_mas::quadrant::{
///     ConfigurableQuadrantSelector, Quadrant, QuadrantSelector,
/// };
/// use std::collections::HashMap;
///
/// let mut map = HashMap::new();
/// map.insert("测试发布域".into(), vec![Quadrant::Hardening, Quadrant::Verification]);
/// let selector = ConfigurableQuadrantSelector::new(map, vec![Quadrant::Implementation]);
/// let quadrants = selector.select_quadrants("测试发布域", "发布任务");
/// assert_eq!(quadrants, vec![Quadrant::Hardening, Quadrant::Verification]);
/// ```
#[derive(Debug, Clone)]
pub struct ConfigurableQuadrantSelector {
    /// 领域→象限映射表（线程安全，支持运行时更新）
    domain_map: Arc<RwLock<HashMap<String, Vec<Quadrant>>>>,
    /// 默认象限（未匹配领域时的降级）
    default_quadrants: Vec<Quadrant>,
}

impl ConfigurableQuadrantSelector {
    /// 创建新的配置化象限选择器
    ///
    /// ## 参数
    /// - `domain_map`: 领域→象限映射表
    /// - `default_quadrants`: 未匹配领域时使用的默认象限
    pub fn new(
        domain_map: HashMap<String, Vec<Quadrant>>,
        default_quadrants: Vec<Quadrant>,
    ) -> Self {
        Self {
            domain_map: Arc::new(RwLock::new(domain_map)),
            default_quadrants,
        }
    }

    /// 运行时更新领域→象限映射
    ///
    /// 支持动态调整映射关系，无需重新创建选择器。
    ///
    /// ## 错误
    /// - `MasError::QuadrantConflict`: 待映射的象限列表包含重复项
    /// - `MasError::Internal`: 映射表读写锁被毒化
    pub fn update_mapping(
        &self,
        domain: &str,
        quadrants: Vec<Quadrant>,
    ) -> std::result::Result<(), MasError> {
        // 校验待映射象限无重复
        let mut seen = std::collections::HashSet::new();
        for q in &quadrants {
            if !seen.insert(*q) {
                return Err(MasError::QuadrantConflict {
                    quadrant: q.name().to_string(),
                });
            }
        }
        match self.domain_map.write() {
            Ok(mut map) => {
                map.insert(domain.to_string(), quadrants);
                Ok(())
            }
            Err(_) => Err(MasError::Internal(
                "ConfigurableQuadrantSelector domain_map RwLock poisoned".into(),
            )),
        }
    }

    /// 使用当前硬编码领域→象限映射作为默认值创建选择器
    ///
    /// 基于 §6.1 静态专家编制表中的领域→主责象限关系：
    ///
    /// | 领域 | 映射象限 |
    /// |------|:--------:|
    /// | 测试发布域 | Hardening, Verification |
    /// | 架构设计域 | Integration, Hardening |
    /// | 代码实现域 | Implementation, Verification, Hardening |
    ///
    /// 默认降级为全四象限，与 `activated_quadrants(VeryComplex)` 行为一致。
    pub fn from_static_mapping() -> Self {
        let mut map = HashMap::new();
        map.insert(
            "测试发布域".to_string(),
            vec![Quadrant::Hardening, Quadrant::Verification],
        );
        map.insert(
            "架构设计域".to_string(),
            vec![Quadrant::Integration, Quadrant::Hardening],
        );
        map.insert(
            "代码实现域".to_string(),
            vec![
                Quadrant::Implementation,
                Quadrant::Verification,
                Quadrant::Hardening,
            ],
        );
        Self::new(map, Quadrant::ALL.to_vec())
    }
}

impl QuadrantSelector for ConfigurableQuadrantSelector {
    fn select_quadrants(&self, domain: &str, _description: &str) -> Vec<Quadrant> {
        match self.domain_map.read() {
            Ok(map) => match map.get(domain) {
                Some(quadrants) => quadrants.clone(),
                None => self.default_quadrants.clone(),
            },
            // 锁毒化时安全降级到默认象限
            Err(_) => self.default_quadrants.clone(),
        }
    }
}

// ============================================================
// 单元测试(QuadrantStatus + QuadrantSelector)
// ============================================================

// 注意:以下测试追加到现有 #[cfg(test)] mod tests 中（文件末尾）。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_has_four_quadrants_in_order() {
        assert_eq!(Quadrant::ALL.len(), 4);
        assert_eq!(Quadrant::ALL[0], Quadrant::Implementation);
        assert_eq!(Quadrant::ALL[3], Quadrant::Hardening);
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        for q in Quadrant::ALL {
            let scope = q.encode_scope("base-task");
            assert_eq!(Quadrant::from_task_scope(&scope), Some(q));
        }
    }

    #[test]
    fn test_from_task_scope_without_tag_returns_none() {
        assert_eq!(Quadrant::from_task_scope("no-tag-here"), None);
    }

    #[test]
    fn test_axis_mapping() {
        assert_eq!(
            Quadrant::Implementation.axis(),
            (ProduceAssure::Produce, CoreCross::Core)
        );
        assert_eq!(
            Quadrant::Hardening.axis(),
            (ProduceAssure::Assure, CoreCross::CrossCutting)
        );
    }

    #[test]
    fn test_activation_matrix_counts() {
        assert_eq!(activated_quadrants(TaskComplexity::Simple).len(), 1);
        assert_eq!(activated_quadrants(TaskComplexity::Medium).len(), 2);
        assert_eq!(activated_quadrants(TaskComplexity::Complex).len(), 3);
        assert_eq!(activated_quadrants(TaskComplexity::VeryComplex).len(), 4);
    }

    #[test]
    fn test_plan_from_quadrants_rejects_duplicate() {
        let err = QuadrantPlan::from_quadrants(
            "t",
            vec![Quadrant::Implementation, Quadrant::Implementation],
        );
        assert!(matches!(err, Err(MasError::QuadrantConflict { .. })));
    }

    #[test]
    fn test_plan_from_quadrants_rejects_over_fanout() {
        // 5 个象限(含重复也会先触发扇出或冲突, 此处构造 5 元素触发 INV-3)
        let five = vec![
            Quadrant::Implementation,
            Quadrant::Integration,
            Quadrant::Verification,
            Quadrant::Hardening,
            Quadrant::Implementation,
        ];
        let err = QuadrantPlan::from_quadrants("t", five);
        assert!(matches!(err, Err(MasError::QuadrantFanoutExceeded { .. })));
    }

    #[test]
    fn test_scoped_assignments_encode_tags() {
        let plan = QuadrantPlan::from_complexity("refactor", TaskComplexity::Medium);
        let pairs = plan.scoped_assignments();
        assert_eq!(pairs.len(), 2);
        assert_eq!(
            pairs[0],
            (Quadrant::Implementation, "refactor#Q1".to_string())
        );
        assert_eq!(
            pairs[1],
            (Quadrant::Verification, "refactor#Q3".to_string())
        );
    }

    // ============================================================
    // ConfigurableQuadrantSelector 测试
    // ============================================================

    #[test]
    fn test_configurable_selector_domain_mapping() {
        let mut map = HashMap::new();
        map.insert("test-domain".into(), vec![Quadrant::Implementation]);
        let selector = ConfigurableQuadrantSelector::new(map, Quadrant::ALL.to_vec());
        let result = selector.select_quadrants("test-domain", "");
        assert_eq!(result, vec![Quadrant::Implementation]);
    }

    #[test]
    fn test_configurable_selector_default_fallback() {
        let map = HashMap::new();
        let selector = ConfigurableQuadrantSelector::new(map, vec![Quadrant::Verification]);
        let result = selector.select_quadrants("unknown-domain", "");
        assert_eq!(result, vec![Quadrant::Verification]);
    }

    #[test]
    fn test_configurable_selector_update_mapping() {
        let mut map = HashMap::new();
        map.insert("domain-a".into(), vec![Quadrant::Implementation]);
        let selector = ConfigurableQuadrantSelector::new(map, Quadrant::ALL.to_vec());

        // 初始映射
        let result = selector.select_quadrants("domain-a", "");
        assert_eq!(result, vec![Quadrant::Implementation]);

        // 运行时更新映射
        selector
            .update_mapping("domain-a", vec![Quadrant::Hardening])
            .unwrap();
        let result = selector.select_quadrants("domain-a", "");
        assert_eq!(result, vec![Quadrant::Hardening]);
    }

    #[test]
    fn test_configurable_selector_update_rejects_duplicates() {
        let selector = ConfigurableQuadrantSelector::new(HashMap::new(), Quadrant::ALL.to_vec());
        let err = selector.update_mapping(
            "d",
            vec![Quadrant::Implementation, Quadrant::Implementation],
        );
        assert!(matches!(err, Err(MasError::QuadrantConflict { .. })));
    }

    #[test]
    fn test_configurable_selector_from_static_mapping() {
        let selector = ConfigurableQuadrantSelector::from_static_mapping();
        // 测试发布域 → Hardening, Verification
        let result = selector.select_quadrants("测试发布域", "");
        assert_eq!(result, vec![Quadrant::Hardening, Quadrant::Verification]);
        // 架构设计域 → Integration, Hardening
        let result = selector.select_quadrants("架构设计域", "");
        assert_eq!(result, vec![Quadrant::Integration, Quadrant::Hardening]);
        // 代码实现域 → Implementation, Verification, Hardening
        let result = selector.select_quadrants("代码实现域", "");
        assert_eq!(
            result,
            vec![
                Quadrant::Implementation,
                Quadrant::Verification,
                Quadrant::Hardening
            ]
        );
        // 未知领域 → 全四象限
        let result = selector.select_quadrants("未知领域", "");
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn test_configurable_selector_description_param_ignored_by_configurable() {
        // ConfigurableQuadrantSelector 当前忽略 description 参数，
        // 验证 description 不影响结果
        let mut map = HashMap::new();
        map.insert("d".into(), vec![Quadrant::Integration]);
        let selector = ConfigurableQuadrantSelector::new(map, Quadrant::ALL.to_vec());
        assert_eq!(
            selector.select_quadrants("d", "任意描述"),
            vec![Quadrant::Integration]
        );
        assert_eq!(
            selector.select_quadrants("d", "不同描述"),
            vec![Quadrant::Integration]
        );
    }

    #[test]
    fn test_quadrant_selector_trait_object_send_sync() {
        // 验证 QuadrantSelector 满足 Send + Sync 约束
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ConfigurableQuadrantSelector>();
    }

    // ============================================================
    // W8 假遥测治理测试（Provider 注入 + 诚实降级）
    // ============================================================

    /// 测试用提供者 — 返回固定快照
    #[derive(Debug)]
    struct TestStatusProvider {
        snapshot: QuadrantStatus,
    }
    impl QuadrantStatusProvider for TestStatusProvider {
        fn snapshot(&self) -> QuadrantStatus {
            self.snapshot
        }
    }

    fn test_snapshot() -> QuadrantStatus {
        QuadrantStatus {
            agent_counts: [2, 1, 3, 0],
            task_counts: [5, 4, 6, 2],
            wsjf_scores: [0.9, 0.7, 0.8, 0.2],
        }
    }

    #[test]
    fn quadrant_status_provider_lifecycle() {
        // W8 生命周期全链路（顺序自包含,规避并行测试对全局 OnceLock 的污染）:
        // 未注册 → 语义化不可用;注册 → 真实快照透传;重复注册 → 拒绝且首个权威
        let unavailable = quadrant_status();
        assert_eq!(unavailable, QuadrantStatus::unavailable());
        assert!(!quadrant_status_available(), "未注册时 available=false");

        let first = TestStatusProvider {
            snapshot: test_snapshot(),
        };
        set_quadrant_status_provider(std::sync::Arc::new(first)).expect("首次注册成功");
        assert!(quadrant_status_available(), "注册后 available=true");
        let status = quadrant_status();
        assert_eq!(status.agent_counts, [2, 1, 3, 0], "真实快照透传");
        assert_eq!(status.task_counts, [5, 4, 6, 2]);
        assert_eq!(status.wsjf_scores, [0.9, 0.7, 0.8, 0.2]);

        // OnceLock 语义: 重复注册报错,首个注册者保持权威
        let second = TestStatusProvider {
            snapshot: QuadrantStatus::unavailable(),
        };
        set_quadrant_status_provider(std::sync::Arc::new(second))
            .expect_err("第二次注册必须报 ProviderAlreadyRegistered");
        let still_first = quadrant_status();
        assert_eq!(still_first.agent_counts, [2, 1, 3, 0], "首个注册者权威不变");
    }
}
