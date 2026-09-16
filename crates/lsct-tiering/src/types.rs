//! LSCT 核心领域类型 — 任务感知能力分层的统一数据模型
//!
//! 对应架构层:L3 Storage
//! 对应创新点:LSCT(Load-aware Semantic Capability Tiering)
//!
//! # 类型职责
//! - `TaskType`:任务类型(编译/调试/测试/运行),不同类型对存储层级有不同偏好
//! - `TaskLoadProfile`:任务负载画像,描述任务对存储层级的负载特征
//! - `TierAssignment`:层级分配,记录能力的当前层级与目标层级
//! - `TierSwitchDecision`:层级切换决策,enum dispatch 携带完整迁移信息
//!
//! # 设计决策(WHY)
//! - **LSCT 自有 `Tier` 枚举**(M10, ADR-160 孤岛偿还批次):原实现复用
//!   `cmt_tiering::Tier`(类型重用)。cmt 反向接入 LSCT 策略层
//!   (`lsct_policy` 装配)后,若继续复用会形成 cmt↔lsct 生产依赖环
//!   (cargo 禁止)。两类型语义 1:1 对应,事件 payload 字符串契约
//!   (`as_str`/`parse_tier`)保持逐字一致,由 cmt 侧转换函数守护。
//! - **TierSwitchDecision 携带完整信息**:Promote/Demote/Keep 各带 capability_id、
//!   层级与 reason,apply_decision 通过模式匹配一次性获取参数,避免额外查表

use serde::{Deserialize, Serialize};

/// 能力存储层级 — 热/温/冷/冰四级
///
/// 语义与 `cmt_tiering::Tier` 1:1 对应(LSCT 是 CMT 之上的任务感知策略层,
/// 不操作 CMT 存储,仅计算策略并通过 `LsctTierSwitched` 事件下发)。
/// WHY 自有类型而非复用:M10 偿还批次中 cmt → lsct 生产边的引入
/// 会使 cmt↔lsct 互依赖成环(cargo 禁止生产依赖环),故 LSCT 自持层级类型,
/// 转换一致性由 `cmt_tiering::lsct_policy` 的双向转换函数保证。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tier {
    /// 热层:内存索引,延迟 < 1μs(访问最频繁的能力)
    Hot,
    /// 温层:SQLite WAL,延迟 < 5ms
    Warm,
    /// 冷层:SQLite 附加数据库,延迟 < 50ms
    Cold,
    /// 冰层:归档只读文件,延迟 < 500ms(近乎不再访问)
    Ice,
}

impl Tier {
    /// 返回层级名称(用于 `LsctTierSwitched` 事件 payload 与日志)
    ///
    /// 字符串契约:必须与 `cmt_tiering::Tier::as_str` 逐字一致
    /// (CMT 订阅侧按此字符串解析层级)。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hot => "Hot",
            Self::Warm => "Warm",
            Self::Cold => "Cold",
            Self::Ice => "Ice",
        }
    }

    /// 从事件 payload / 配置字符串解析层级(大小写敏感,与 CMT 契约一致)
    pub fn parse_tier(s: &str) -> Option<Self> {
        match s {
            "Hot" => Some(Self::Hot),
            "Warm" => Some(Self::Warm),
            "Cold" => Some(Self::Cold),
            "Ice" => Some(Self::Ice),
            _ => None,
        }
    }
}

/// 任务类型 — 不同任务对存储层级有不同偏好
///
/// 设计依据:编译任务需要快速访问编译产物 → 升温到 Hot;
/// 调试任务访问频率低 → 降温到 Warm/Cold。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskType {
    /// 编译任务:高强度时升温到 Hot(快速访问编译产物)
    Compile,
    /// 调试任务:低强度时降温到 Warm/Cold(访问频率低)
    Debug,
    /// 测试任务:中强度时保持 Warm(平衡访问与存储)
    Test,
    /// 运行任务:始终升温到 Hot(需要快速响应)
    Run,
}

impl TaskType {
    /// 返回任务类型名称(用于日志与事件 payload)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Compile => "Compile",
            Self::Debug => "Debug",
            Self::Test => "Test",
            Self::Run => "Run",
        }
    }
}

/// 任务负载画像 — 描述任务对存储层级的负载特征
///
/// `intensity` 与 `frequency` 共同决定能力应处的层级:
/// - 高 intensity 编译/运行任务 → Hot
/// - 低 intensity 调试任务 → Warm/Cold
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskLoadProfile {
    /// 任务类型
    pub task_type: TaskType,
    /// 任务强度 [0.0, 1.0],越高越需要快速访问
    pub intensity: f32,
    /// 任务频率(单位时间内的执行次数)
    pub frequency: u32,
}

/// 层级分配 — 记录能力的当前层级与目标层级
///
/// `current_tier` 与 `target_tier` 的差异驱动升降温决策:
/// - current < target(rank 更大)→ 需要 Demote
/// - current > target(rank 更小)→ 需要 Promote
/// - current == target → Keep
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TierAssignment {
    /// 能力 ID
    pub capability_id: String,
    /// 当前所在层级
    pub current_tier: Tier,
    /// 目标层级(由任务负载画像决定)
    pub target_tier: Tier,
    /// 决策原因(如 "compile task high intensity")
    pub reason: String,
}

/// 层级切换决策 — enum dispatch,携带完整迁移信息
///
/// WHY 携带完整信息:apply_decision 通过模式匹配一次性获取 capability_id、
/// from/to 层级与 reason,无需回查 assignments,降低锁竞争与一致性风险。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TierSwitchDecision {
    /// 升温:将能力提升到更热的层级(逐级,rank - 1)
    Promote {
        /// 能力 ID
        capability_id: String,
        /// 源层级
        from: Tier,
        /// 目标层级
        to: Tier,
        /// 决策原因
        reason: String,
    },
    /// 降温:将能力降级到更冷的层级(逐级,rank + 1)
    Demote {
        /// 能力 ID
        capability_id: String,
        /// 源层级
        from: Tier,
        /// 目标层级
        to: Tier,
        /// 决策原因
        reason: String,
    },
    /// 保持:层级不变
    Keep {
        /// 能力 ID
        capability_id: String,
        /// 当前层级
        tier: Tier,
        /// 决策原因
        reason: String,
    },
}

impl TierSwitchDecision {
    /// 获取能力 ID(三种决策共用)
    pub fn capability_id(&self) -> &str {
        match self {
            Self::Promote { capability_id, .. }
            | Self::Demote { capability_id, .. }
            | Self::Keep { capability_id, .. } => capability_id,
        }
    }

    /// 判断是否为升温决策
    pub fn is_promote(&self) -> bool {
        matches!(self, Self::Promote { .. })
    }

    /// 判断是否为降温决策
    pub fn is_demote(&self) -> bool {
        matches!(self, Self::Demote { .. })
    }
}

/// 层级温度排序 — Hot 最热(0),Ice 最冷(3)
///
/// WHY:用于升降温的逐级校验与单调性不变量验证。
/// 数字越小表示越热,升温时 `tier_rank(to) == tier_rank(from) - 1`。
pub fn tier_rank(tier: Tier) -> u8 {
    match tier {
        Tier::Hot => 0,
        Tier::Warm => 1,
        Tier::Cold => 2,
        Tier::Ice => 3,
    }
}

/// 获取更热的相邻层级(逐级升温)
///
/// 返回 None 表示已是最热层(Hot),无法继续升温。
/// WHY 逐级迁移:防止跨级跳跃(如 Ice→Hot)造成存储层负载突变,
/// 每次只迁移到相邻层级,多个 tick 周期逐步达到目标层级。
pub fn next_warmer(tier: Tier) -> Option<Tier> {
    match tier {
        Tier::Hot => None,
        Tier::Warm => Some(Tier::Hot),
        Tier::Cold => Some(Tier::Warm),
        Tier::Ice => Some(Tier::Cold),
    }
}

/// 获取更冷的相邻层级(逐级降温)
///
/// 返回 None 表示已是最冷层(Ice),无法继续降温。
/// WHY 逐级迁移:与 `next_warmer` 对称,防止跨级跳跃。
pub fn next_colder(tier: Tier) -> Option<Tier> {
    match tier {
        Tier::Hot => Some(Tier::Warm),
        Tier::Warm => Some(Tier::Cold),
        Tier::Cold => Some(Tier::Ice),
        Tier::Ice => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_type_as_str() {
        assert_eq!(TaskType::Compile.as_str(), "Compile");
        assert_eq!(TaskType::Debug.as_str(), "Debug");
        assert_eq!(TaskType::Test.as_str(), "Test");
        assert_eq!(TaskType::Run.as_str(), "Run");
    }

    #[test]
    fn test_tier_rank_ordering() {
        // Hot 最热(0),Ice 最冷(3),单调递增
        assert_eq!(tier_rank(Tier::Hot), 0);
        assert_eq!(tier_rank(Tier::Warm), 1);
        assert_eq!(tier_rank(Tier::Cold), 2);
        assert_eq!(tier_rank(Tier::Ice), 3);
        assert!(tier_rank(Tier::Hot) < tier_rank(Tier::Warm));
        assert!(tier_rank(Tier::Warm) < tier_rank(Tier::Cold));
        assert!(tier_rank(Tier::Cold) < tier_rank(Tier::Ice));
    }

    #[test]
    fn test_next_warmer_adjacent() {
        // 逐级升温:每层只升一级,Hot 已是最热返回 None
        assert_eq!(next_warmer(Tier::Hot), None);
        assert_eq!(next_warmer(Tier::Warm), Some(Tier::Hot));
        assert_eq!(next_warmer(Tier::Cold), Some(Tier::Warm));
        assert_eq!(next_warmer(Tier::Ice), Some(Tier::Cold));
    }

    #[test]
    fn test_next_colder_adjacent() {
        // 逐级降温:每层只降一级,Ice 已是最冷返回 None
        assert_eq!(next_colder(Tier::Hot), Some(Tier::Warm));
        assert_eq!(next_colder(Tier::Warm), Some(Tier::Cold));
        assert_eq!(next_colder(Tier::Cold), Some(Tier::Ice));
        assert_eq!(next_colder(Tier::Ice), None);
    }

    #[test]
    fn test_tier_switch_decision_capability_id() {
        let promote = TierSwitchDecision::Promote {
            capability_id: "cap-1".into(),
            from: Tier::Warm,
            to: Tier::Hot,
            reason: "compile".into(),
        };
        assert_eq!(promote.capability_id(), "cap-1");
        assert!(promote.is_promote());
        assert!(!promote.is_demote());

        let demote = TierSwitchDecision::Demote {
            capability_id: "cap-2".into(),
            from: Tier::Hot,
            to: Tier::Warm,
            reason: "debug".into(),
        };
        assert!(demote.is_demote());
        assert!(!demote.is_promote());

        let keep = TierSwitchDecision::Keep {
            capability_id: "cap-3".into(),
            tier: Tier::Warm,
            reason: "stable".into(),
        };
        assert!(!keep.is_promote());
        assert!(!keep.is_demote());
        assert_eq!(keep.capability_id(), "cap-3");
    }

    #[test]
    fn test_tier_assignment_fields() {
        let assignment = TierAssignment {
            capability_id: "cap-1".into(),
            current_tier: Tier::Warm,
            target_tier: Tier::Hot,
            reason: "compile task".into(),
        };
        assert_eq!(assignment.capability_id, "cap-1");
        assert_eq!(assignment.current_tier, Tier::Warm);
        assert_eq!(assignment.target_tier, Tier::Hot);
    }
}
