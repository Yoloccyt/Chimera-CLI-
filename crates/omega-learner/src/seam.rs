//! 七接缝标识 — v5.0 §7.3 omega-learner 的七个学习接缝
//!
//! 对应任务: **P4-W13.1.2**（模块骨架）+ **P4-W16.2.2**（S7 R1 接缝扩展）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 + §7.5
//! 对应 ADR: **ADR-031**（omega-learner 边界）+ **ADR-043**（R1 影子模式）
//!
//! # 七接缝概览
//!
//! | # | 接缝 | 真实代码锚点 | 臂 | 奖励 | 算法 |
//! |---|------|------------|-----|------|------|
//! | S1 | DDR/HCW 密度档位 | hcw-window selector | ρ∈{0.5,2,5,10} | 成功率 − 延迟惩罚 | LinUCB |
//! | S2 | 记忆策略选择 | mlc-engine recall | 最小检索/标准TopK/查询重构/激进剪枝/时间聚焦 | 阶段目标达成率 | LinUCB |
//! | S3 | SCC 预取 | scc-cache prefetch | 预取候选集 | 命中率 | LinUCB |
//! | S4 | selector 权重系数 | hcw-window selector.rs w1/w2/w3 | 权重向量 | 后悔率 | LinUCB |
//! | S5 | Parliament 激活 | parliament Fast Path | 跳过/精简/完整辩论 | 推翻率 × 辩论成本 | LinUCB |
//! | S6 | 衰减参数 | decay-engine DecayProfile | profile 参数 | 误拦率 vs 漏拦率加权 | LinUCB |
//! | S7 | 召回配额（R1） | omega-learner r1_recall_quota | k∈{5,10,20,50,100} | 召回率 − 误杀 − 延迟 | CQL/IQL 离线 RL |
//!
//! # S7 接缝特殊性（ADR-043）
//!
//! - **算法不同**: S1-S6 用在线 LinUCB bandit；S7 用离线 RL（CQL/IQL）
//! - **数据源不同**: S1-S6 实时观察奖励；S7 从 `ReplayPool<RecallQuotaTransition>` 采样训练
//! - **解冻条件更严**: S7 需影子模式 2 周观察 + EWMA≥0.7 + 胜率≥71.4% + 无 ASA + 三方评审
//! - **R2 冻结关联**: ADR-042 将 R2（GSOE×AutoDPO 约束 RL）冻结，S7 仅承载 R1
//!
//! # 设计原则
//!
//! - **本期仅定义类型**: W13.1 只搭骨架,W13.2/W13.3 才接入 S1/S4 接缝
//! - **枚举而非 trait**: 七接缝有限且固定,枚举更轻量(避免 trait object 开销)
//! - **每个接缝独立上下文维度**: 由调用方在接入时定义(本期不强制)

use serde::{Deserialize, Serialize};

/// 七接缝标识枚举
///
/// WHY 用枚举而非字符串:
/// - 编译期穷尽性检查(match 必须覆盖所有变体)
/// - 避免 typo 错误
/// - IDE 自动补全
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum SeamId {
    /// S1: DDR/HCW 密度档位(hcw-window selector)
    ///
    /// 臂: ρ∈{0.5, 2, 5, 10} 四档密度档位
    /// 上下文: 任务类型/DAG 深度/内存压力
    /// 奖励: 成功率 − 延迟惩罚
    S1Density = 1,

    /// S2: 记忆策略选择(mlc-engine recall 路径)
    ///
    /// 臂: 最小检索/标准TopK/查询重构/激进剪枝/时间聚焦
    /// 上下文: 任务阶段(初期/卡壳/长跑)
    /// 奖励: 阶段目标达成率
    S2MemoryStrategy = 2,

    /// S3: SCC 预取(scc-cache prefetch.rs)
    ///
    /// 臂: 预取候选集
    /// 上下文: 编辑历史/调用图
    /// 奖励: 命中率(Hawkeye 式模仿学习标签)
    S3Prefetch = 3,

    /// S4: selector 权重系数(hcw-window selector.rs w1/w2/w3)
    ///
    /// 臂: 权重向量(SelectorWeights 空间的离散采样)
    /// 上下文: 块类型/访问时序/错误关联
    /// 奖励: 后悔率(被驱逐块后被需要)
    S4SelectorWeights = 4,

    /// S5: Parliament 激活(parliament Fast Path)
    ///
    /// 臂: 跳过/精简/完整辩论
    /// 上下文: risk_level/只读性/历史模式
    /// 奖励: 推翻率 × 辩论成本
    S5ParliamentActivation = 5,

    /// S6: 衰减参数(decay-engine DecayProfile)
    ///
    /// 臂: DecayProfile 参数
    /// 上下文: 操作类型/风险信号密度
    /// 奖励: 误拦率 vs 漏拦率加权
    S6DecayProfile = 6,

    /// S7: 召回配额（omega-learner r1_recall_quota，P4-W16.2.2）
    ///
    /// R1 离线 RL 接缝：CQL/IQL 算法学习召回配额 k ∈ {5,10,20,50,100}。
    /// 与 S1-S6 在线 bandit 不同，S7 从 `ReplayPool<RecallQuotaTransition>`
    /// 采样训练，需满足影子模式 2 周前置（ADR-043）才能解冻为 Authorized。
    ///
    /// 臂: k∈{5,10,20,50,100}（5 档配额）
    /// 上下文: 任务阶段 + 任务复杂度 + 内存压力（6 维）
    /// 奖励: 召回率 − 0.5×误杀率 − 0.3×延迟惩罚
    S7RecallQuota = 7,
}

impl SeamId {
    /// 返回接缝编号(1-7)
    pub const fn number(self) -> u8 {
        self as u8
    }

    /// 返回接缝简称(用于日志/调试)
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::S1Density => "S1-density",
            Self::S2MemoryStrategy => "S2-memory",
            Self::S3Prefetch => "S3-prefetch",
            Self::S4SelectorWeights => "S4-selector",
            Self::S5ParliamentActivation => "S5-parliament",
            Self::S6DecayProfile => "S6-decay",
            Self::S7RecallQuota => "S7-recall-quota",
        }
    }

    /// 返回接缝全称(用于文档/UI)
    pub const fn full_name(self) -> &'static str {
        match self {
            Self::S1Density => "DDR/HCW density tier",
            Self::S2MemoryStrategy => "memory strategy selection",
            Self::S3Prefetch => "SCC prefetch",
            Self::S4SelectorWeights => "selector weight coefficients",
            Self::S5ParliamentActivation => "Parliament activation",
            Self::S6DecayProfile => "decay profile parameters",
            Self::S7RecallQuota => "recall quota (R1 offline RL)",
        }
    }

    /// 返回接缝的代码锚点(用于诊断与跨 crate 引用)
    pub const fn code_anchor(self) -> &'static str {
        match self {
            Self::S1Density => "crates/hcw-window/src/selector.rs",
            Self::S2MemoryStrategy => "crates/mlc-engine/src/",
            Self::S3Prefetch => "crates/scc-cache/src/prefetch.rs",
            Self::S4SelectorWeights => "crates/hcw-window/src/selector.rs (w1/w2/w3)",
            Self::S5ParliamentActivation => "crates/parliament/src/",
            Self::S6DecayProfile => "crates/decay-engine/src/",
            Self::S7RecallQuota => "crates/omega-learner/src/r1_recall_quota.rs",
        }
    }

    /// 返回该接缝对应的任务编号(便于回溯 tasks.md)
    pub const fn task_id(self) -> &'static str {
        match self {
            Self::S1Density => "P4-W13.2",
            Self::S2MemoryStrategy => "P4-W14.1",
            Self::S3Prefetch => "P4-W14.2",
            Self::S4SelectorWeights => "P4-W13.3",
            Self::S5ParliamentActivation => "P4-W14.3",
            Self::S6DecayProfile => "P4-W14.4",
            Self::S7RecallQuota => "P4-W16.2.2",
        }
    }

    /// 返回所有七接缝(用于遍历初始化)
    ///
    /// WHY 7 而非 6: S7 为 P4-W16.2.2 新增 R1 离线 RL 接缝
    pub const fn all() -> [SeamId; 7] {
        [
            Self::S1Density,
            Self::S2MemoryStrategy,
            Self::S3Prefetch,
            Self::S4SelectorWeights,
            Self::S5ParliamentActivation,
            Self::S6DecayProfile,
            Self::S7RecallQuota,
        ]
    }
}

impl std::fmt::Display for SeamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seam_id_number() {
        assert_eq!(SeamId::S1Density.number(), 1);
        assert_eq!(SeamId::S2MemoryStrategy.number(), 2);
        assert_eq!(SeamId::S3Prefetch.number(), 3);
        assert_eq!(SeamId::S4SelectorWeights.number(), 4);
        assert_eq!(SeamId::S5ParliamentActivation.number(), 5);
        assert_eq!(SeamId::S6DecayProfile.number(), 6);
        // P4-W16.2.2: S7 召回配额（R1 离线 RL 接缝）
        assert_eq!(SeamId::S7RecallQuota.number(), 7);
    }

    #[test]
    fn test_seam_id_short_name() {
        assert_eq!(SeamId::S1Density.short_name(), "S1-density");
        assert_eq!(SeamId::S6DecayProfile.short_name(), "S6-decay");
        // P4-W16.2.2: S7 简称
        assert_eq!(SeamId::S7RecallQuota.short_name(), "S7-recall-quota");
    }

    #[test]
    fn test_seam_id_full_name() {
        assert_eq!(SeamId::S1Density.full_name(), "DDR/HCW density tier");
        assert_eq!(
            SeamId::S5ParliamentActivation.full_name(),
            "Parliament activation"
        );
        // P4-W16.2.2: S7 全称
        assert_eq!(
            SeamId::S7RecallQuota.full_name(),
            "recall quota (R1 offline RL)"
        );
    }

    #[test]
    fn test_seam_id_code_anchor() {
        assert_eq!(
            SeamId::S1Density.code_anchor(),
            "crates/hcw-window/src/selector.rs"
        );
        assert_eq!(
            SeamId::S3Prefetch.code_anchor(),
            "crates/scc-cache/src/prefetch.rs"
        );
        // P4-W16.2.2: S7 代码锚点
        assert_eq!(
            SeamId::S7RecallQuota.code_anchor(),
            "crates/omega-learner/src/r1_recall_quota.rs"
        );
    }

    #[test]
    fn test_seam_id_task_id() {
        assert_eq!(SeamId::S1Density.task_id(), "P4-W13.2");
        assert_eq!(SeamId::S4SelectorWeights.task_id(), "P4-W13.3");
        assert_eq!(SeamId::S2MemoryStrategy.task_id(), "P4-W14.1");
        // P4-W16.2.2: S7 任务编号
        assert_eq!(SeamId::S7RecallQuota.task_id(), "P4-W16.2.2");
    }

    #[test]
    fn test_seam_id_all_returns_seven() {
        let all = SeamId::all();
        assert_eq!(all.len(), 7);
        assert!(all.contains(&SeamId::S1Density));
        assert!(all.contains(&SeamId::S6DecayProfile));
        // P4-W16.2.2: S7 必须在 all() 中
        assert!(all.contains(&SeamId::S7RecallQuota));
    }

    #[test]
    fn test_seam_id_all_unique() {
        let all = SeamId::all();
        let mut seen = std::collections::HashSet::new();
        for seam in all.iter() {
            assert!(seen.insert(seam.number()), "duplicate seam number");
        }
    }

    #[test]
    fn test_seam_id_display() {
        assert_eq!(format!("{}", SeamId::S1Density), "S1-density");
        assert_eq!(format!("{}", SeamId::S4SelectorWeights), "S4-selector");
    }

    #[test]
    fn test_seam_id_equality() {
        assert_eq!(SeamId::S1Density, SeamId::S1Density);
        assert_ne!(SeamId::S1Density, SeamId::S2MemoryStrategy);
    }

    #[test]
    fn test_seam_id_copy() {
        let seam = SeamId::S1Density;
        let copied = seam; // Copy
        assert_eq!(seam, copied);
    }

    #[test]
    fn test_seam_id_serialize_json() {
        let seam = SeamId::S4SelectorWeights;
        let json = serde_json::to_string(&seam).unwrap();
        let deserialized: SeamId = serde_json::from_str(&json).unwrap();
        assert_eq!(seam, deserialized);
    }

    #[test]
    fn test_seam_id_repr_u8() {
        // 验证 #[repr(u8)] 表示: 内存中占 1 字节
        assert_eq!(std::mem::size_of::<SeamId>(), 1);
    }
}
