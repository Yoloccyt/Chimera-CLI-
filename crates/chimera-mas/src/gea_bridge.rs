//! MAS → GEA 桥接 — E01-E08 静态编制到 gea 动态门控的接线层(M12 / ADR-185 D2)
//!
//! 架构层归属: L9 Quest(chimera-mas 内部子模块,与 feedback.rs 同层)
//! 核心职责: 把 MAS 侧静态专家编制(`experts::ExpertProfile`, &'static 权限模型)
//!           翻译为 GEA 侧动态专家画像(`gea_activator::ExpertProfile`, 64 维向量模型),
//!           并把 AgentTask 的任务特征翻译为 gea `TaskProfile` 供门控激活。
//!
//! ## 设计关系(兑现 feedback.rs:7-12 书面设计)
//!
//! - gea 侧管"激活倾向"(门控 confidence + 动态阈值 + 激活缓存);
//!   mas 侧管"调度/分配倾向"(WSJF 权重 + 四象限编制)。两者互补,不合并类型。
//! - 本桥接层只做**确定性向量翻译**,不引入 tract-onnx 推理
//!   (M11 裁决: 轻量 64 维桥接即可,无须模型推理)。
//!
//! ## 同名类型消歧纪律(领域类型稳定性红线)
//!
//! mas `experts::ExpertProfile`(静态编制, 全 `&'static`)与
//! gea `ExpertProfile`(动态画像, owned + 运行时反馈字段)是**两个不同领域类型**,
//! 本模块一律全限定路径书写, 禁止合并/别名遮蔽。
//!
//! ## 64 维桥接词表(D2 权威定义)
//!
//! | 维区间      | 语义 | 专家向量 | 任务 CLV |
//! |-------------|------|----------|----------|
//! | `dims[0..4]` | 象限 one-hot(Q1→0, Q2→1, Q3→2, Q4→3) | 主责象限 = 1.0 | 各激活象限 = 复杂度 |
//! | `dim[4]`    | 强度维 | 编制权重(Tier 映射) | 复杂度评分 |
//! | `dim[5]`    | 风险维 | —(0) | 风险等级 / 100 |
//! | `dim[6]`    | 保留 | 0 | 0 |
//! | `dims[7..64]` | 能力标签 FNV-1a 稳定哈希填充 | 各标签 → 1.0 | task_type 同哈希 → 复杂度 |
//!
//! WHY 任务侧同步哈希 task_type: 只填专家侧会引入无任务对应维的范数膨胀,
//! 稀释 relevance;双侧同表哈希使"共享词表"在 relevance 通道也有对应
//! (词表 = 小写象限名,与亲和通道 `compute_affinity` 对表)。
//! WHY 哈希而非 embedding: 桥接层职责是"确定性可测的语义翻译",
//! 语义相似度属 GSOE 进化侧职责(与 gea gating.rs `compute_affinity` 的
//! 诚实边界注释一致), 不臆造 tag embedding 基建。
//!
//! ## 门控配置(`mas_gea_config`)
//!
//! gea `GeaConfig::default()` 面向稠密 512 维 CLV 调参(bias 0.5、w4=0);
//! 桥接词表为稀疏 one-hot,relevance 上限更低,且接线目的本身要求
//! confidence 反馈闭环真正影响门控(w4>0)。故 MAS 侧消费统一经
//! [`mas_gea_config`] 重校准(权重和 = 1.0 通过 gea 校验)。
//! 高负载冷启动下激活趋保守(动态阈值随负载/未命中率抬升)——
//! 这是 Ω-Sparse 的稀疏激活语义,非缺陷。

use std::sync::Arc;

use event_bus::TaskPriority;

use crate::delegation::{AgentTask, TaskComplexity};
use crate::experts::{ExpertRegistry, PermissionTier};
use crate::quadrant::{Quadrant, QuadrantPlan};

/// 桥接向量维度 — 与 gea `ExpertProfile.expert_vector`(64 维压缩表示)对齐
pub const BRIDGE_VECTOR_DIMS: usize = 64;

/// 象限 one-hot 基维(Q1→0 / Q2→1 / Q3→2 / Q4→3)
const QUADRANT_ONEHOT_BASE: usize = 0;
/// 强度维(专家 = 编制权重;任务 = 复杂度评分)
const WEIGHT_DIM: usize = 4;
/// 风险维(仅任务侧填充,取值风险等级 / 100)
const RISK_DIM: usize = 5;
/// 能力标签哈希填充基维(哈希值映射到 [TAG_HASH_BASE, BRIDGE_VECTOR_DIMS))
const TAG_HASH_BASE: usize = 7;

/// PermissionTier → 编制权重 [0.0, 1.0](ADR-185 D2)
///
/// 三级权限模型(§11.2)的权重派生: 权限越高,编制权重越大——
/// 冲突消解时高权限专家(如需审批的 DevOps)在同分情况下优先。
/// 取值固定在 (0,1) 开区间内且互不相同,保证可逆判别。
pub fn tier_priority_weight(tier: PermissionTier) -> f32 {
    match tier {
        PermissionTier::ReadOnly => 0.3,
        PermissionTier::LimitedWrite => 0.6,
        PermissionTier::HighRiskApproval => 0.9,
    }
}

/// TaskComplexity → 复杂度评分 [0.0, 1.0](ADR-185 D2)
///
/// 取四档带宽中点: Simple=[0,0.5) → 0.25, Medium=[0.5,0.75) → 0.5,
/// Complex=[0.75,0.9) → 0.75, VeryComplex=[0.9,1.0] → 1.0。
/// WHY 中点而非档位序数/4: 与 gea 门控公式中 complexity 项的连续语义一致,
/// VeryComplex 须达满档才能越过最高动态阈值(负载抬高时仍可激活)。
pub fn complexity_weight(complexity: TaskComplexity) -> f32 {
    match complexity {
        TaskComplexity::Simple => 0.25,
        TaskComplexity::Medium => 0.5,
        TaskComplexity::Complex => 0.75,
        TaskComplexity::VeryComplex => 1.0,
    }
}

/// TaskPriority → 风险等级 0-100(ADR-185 D2)
///
/// MAS 委托任务无独立风险字段,以调度优先级作为风险代理:
/// 优先级越高,失败代价越大(抢占式任务失败影响面更广)。
pub fn priority_risk_level(priority: TaskPriority) -> u8 {
    match priority {
        TaskPriority::Low => 10,
        TaskPriority::Medium => 30,
        TaskPriority::High => 60,
        TaskPriority::Critical => 90,
    }
}

/// 象限 → one-hot 维下标(Q1 Implementation → 0,…,Q4 Hardening → 3)
///
/// 下标取 `Quadrant::ALL` 声明序,与桥接词表 dims[0..4] 一一对应。
pub fn quadrant_dim(quadrant: Quadrant) -> usize {
    Quadrant::ALL
        .iter()
        .position(|q| q == &quadrant)
        .expect("Quadrant::ALL 覆盖全部四象限,position 必命中")
}

/// 能力标签 → 哈希填充维(确定性 FNV-1a,跨进程/跨平台一致)
///
/// WHY FNV-1a 而非 DefaultHasher: DefaultHasher 的键随机化(SipHash)会使
/// 同标签跨进程落到不同维,破坏桥接确定性;FNV-1a 无状态,输出仅由输入决定。
/// 冲突语义: 不同标签落到同维自然合并(值恒 1.0,叠加不增),与 one-hot 填充一致。
fn tag_hash_dim(tag: &str) -> usize {
    const FNV_OFFSET: u32 = 0x811c_9dc5;
    const FNV_PRIME: u32 = 0x0100_0193;
    let mut hash = FNV_OFFSET;
    for byte in tag.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    TAG_HASH_BASE + (hash as usize % (BRIDGE_VECTOR_DIMS - TAG_HASH_BASE))
}

/// 主责象限 → 亲和标签(task_type 词表,小写象限名)
///
/// gea 亲和通道(`gating::compute_affinity`)对 task_type 与 capability_tags
/// 做归一化精确/前缀匹配;桥接用词表 = 小写象限名,同象限专家亲和度 1.0。
fn quadrant_affinity_tag(quadrant: Quadrant) -> String {
    quadrant.name().to_lowercase()
}

/// 标签集合 → 64 维哈希填充向量(dims[TAG_HASH_BASE..64] 置位,值 1.0)
fn tag_hash_vector(tags: &[String]) -> Vec<f32> {
    let mut vector = vec![0.0_f32; BRIDGE_VECTOR_DIMS];
    for tag in tags {
        vector[tag_hash_dim(tag)] = 1.0;
    }
    vector
}

/// mas 静态编制专家 → gea 动态专家画像(全字段映射,ADR-185 D2)
///
/// ## 映射规则
/// - `expert_id` ← 编制 `id`(E01..E08),两 crate 以专家 ID 为共享键
/// - `expert_vector` ← dims[0..4] 主责象限 one-hot + dim[4] 编制权重 +
///   dims[7..64] 能力标签哈希填充
/// - `priority` ← `tier_priority_weight(highest_tier())`(编制权重派生)
/// - `capability_tags` ← [小写象限名(亲和词表), `sub_agent_type`]
/// - 运行时反馈字段(success/total/latency)留零,由执行层 record_outcome 回填
///
/// ## 消歧
/// 参数为 mas `experts::ExpertProfile`,返回 gea `gea_activator::ExpertProfile`,
/// 全限定路径书写,禁止合并两个同名类型(领域类型稳定性红线)。
pub fn build_gea_expert_profile(
    mas_profile: &crate::experts::ExpertProfile,
) -> gea_activator::ExpertProfile {
    let weight = tier_priority_weight(mas_profile.highest_tier());
    let tags = vec![
        quadrant_affinity_tag(mas_profile.primary_quadrant),
        mas_profile.sub_agent_type.to_string(),
    ];
    let mut expert_vector = tag_hash_vector(&tags);
    expert_vector[QUADRANT_ONEHOT_BASE + quadrant_dim(mas_profile.primary_quadrant)] = 1.0;
    expert_vector[WEIGHT_DIM] = weight;
    gea_activator::ExpertProfile::new(mas_profile.id, expert_vector, weight, tags)
}

/// 把 E01-E08 静态编制注册进 gea 激活器(启动时一次性,幂等)
///
/// 幂等依据: `GeaActivator::register_expert` 对同 ID 覆盖旧画像,
/// 重复调用不产生重复条目。反馈字段由注册表既有条目保留(覆盖会清零——
/// WHY 可接受: 启动期一次性调用,运行期反馈尚未产生;运行期再注册属误用)。
pub fn register_mas_experts(gea: &gea_activator::GeaActivator) {
    for mas_profile in ExpertRegistry::new().all() {
        gea.register_expert(build_gea_expert_profile(mas_profile));
    }
}

/// 构造注册好 E01-E08 的共享激活器(组合根/测试的单一装配入口)
///
/// WHY 独立构造函数: 保证 `GeaActivator::new`(config 校验)+ 注册两步
/// 在任何调用方(chimera-cli 组合根 / mas 集成测试)行为一致,
/// 避免"注册了但 config 不同"的漂移面。
pub fn build_mas_activator(
    config: gea_activator::GeaConfig,
    event_bus: event_bus::EventBus,
) -> Result<Arc<gea_activator::GeaActivator>, gea_activator::GeaError> {
    let activator = gea_activator::GeaActivator::new(config, event_bus)?;
    register_mas_experts(&activator);
    Ok(Arc::new(activator))
}

/// AgentTask → gea TaskProfile(64 维轻量桥接,无须 tract-onnx 推理)
///
/// ## 映射规则
/// - `complexity_score` ← `complexity_weight(task.complexity)`
/// - `risk_level` ← `priority_risk_level(task.priority)`(优先级作风险代理)
/// - `task_type` ← 激活计划首象限的小写名(亲和词表,与专家 capability_tags 对表)
/// - `clv` ← dims[0..4] 各激活象限(`QuadrantPlan::from_complexity`) = 复杂度
///   + dim[4] 复杂度 + dim[5] 风险/100 + dims[7..64] task_type 哈希维 = 复杂度
///
/// WHY 复用 QuadrantPlan 而非重造映射: 象限激活矩阵(§3.4)是既有权威语义,
/// 桥接只翻译不新造;Simple→[Q1] 到 VeryComplex→[Q1..Q4] 与 delegate_quadrants 一致。
pub fn task_profile_from_agent_task(task: &AgentTask) -> gea_activator::TaskProfile {
    let complexity = complexity_weight(task.complexity);
    let risk_level = priority_risk_level(task.priority);
    let plan = QuadrantPlan::from_complexity(task.inner.task_id.clone(), task.complexity);

    let task_type = quadrant_affinity_tag(plan.quadrants()[0]);
    let mut clv = vec![0.0_f32; BRIDGE_VECTOR_DIMS];
    for quadrant in plan.quadrants() {
        clv[QUADRANT_ONEHOT_BASE + quadrant_dim(*quadrant)] = complexity;
    }
    clv[WEIGHT_DIM] = complexity;
    clv[RISK_DIM] = f32::from(risk_level) / 100.0;
    // task_type 同哈希维 = 复杂度(与专家侧标签哈希同表对位,relevance 可感知共享词表)
    clv[tag_hash_dim(&task_type)] = complexity;

    gea_activator::TaskProfile::new(complexity, task_type, risk_level, clv)
}

/// MAS 桥接推荐 gea 门控配置(ADR-185 D2)
///
/// ## 与 `GeaConfig::default()` 的差异及 WHY
/// - `w4_confidence = 0.25`:接线核心目的 = Ω-Evolve 反馈闭环影响门控,
///   default 的 w4=0 使 confidence 永不参与门控(闭环空转,违 ADR-179 判据)
/// - `bias = 0.35`:桥接稀疏 one-hot 的 relevance 上限低于稠密 CLV,
///   下调偏置补偿缩放,保证中-高复杂度委托在冷启动低负载下可激活
/// - `w1=0.3 / w2=0.2 / w3=0.25`:复杂度/相关性/亲和三通道再平衡,
///   四权重和 = 1.0 满足 gea `Config::validate`
///
/// 权重和校验委托 gea `GeaConfig::validate`(GeaActivator::new 时执行),
/// 本函数返回常量配置,恒通过校验。
pub fn mas_gea_config() -> gea_activator::GeaConfig {
    gea_activator::GeaConfig {
        w1: 0.3,
        w2: 0.2,
        w3: 0.25,
        w4_confidence: 0.25,
        bias: 0.35,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_priority_weight_distinct_and_in_range() {
        // 三档权重互异且在 (0,1) 开区间——可逆判别 + 门控权重合法域
        let w_ro = tier_priority_weight(PermissionTier::ReadOnly);
        let w_lw = tier_priority_weight(PermissionTier::LimitedWrite);
        let w_hr = tier_priority_weight(PermissionTier::HighRiskApproval);
        assert!(0.0 < w_ro && w_ro < w_lw && w_lw < w_hr && w_hr < 1.0);
    }

    #[test]
    fn test_complexity_weight_monotonic_midpoints() {
        // 单调递增 + 四档带宽中点(0.25/0.5/0.75/1.0)
        assert_eq!(complexity_weight(TaskComplexity::Simple), 0.25);
        assert_eq!(complexity_weight(TaskComplexity::Medium), 0.5);
        assert_eq!(complexity_weight(TaskComplexity::Complex), 0.75);
        assert_eq!(complexity_weight(TaskComplexity::VeryComplex), 1.0);
    }

    #[test]
    fn test_priority_risk_level_mapping() {
        assert_eq!(priority_risk_level(TaskPriority::Low), 10);
        assert_eq!(priority_risk_level(TaskPriority::Medium), 30);
        assert_eq!(priority_risk_level(TaskPriority::High), 60);
        assert_eq!(priority_risk_level(TaskPriority::Critical), 90);
    }

    #[test]
    fn test_quadrant_dim_matches_all_order() {
        assert_eq!(quadrant_dim(Quadrant::Implementation), 0);
        assert_eq!(quadrant_dim(Quadrant::Integration), 1);
        assert_eq!(quadrant_dim(Quadrant::Verification), 2);
        assert_eq!(quadrant_dim(Quadrant::Hardening), 3);
    }

    #[test]
    fn test_tag_hash_dim_deterministic_and_in_range() {
        // 确定性: 同标签两次哈希同维;值域: [TAG_HASH_BASE, BRIDGE_VECTOR_DIMS)
        let d1 = tag_hash_dim("code-gen");
        let d2 = tag_hash_dim("code-gen");
        assert_eq!(d1, d2, "FNV-1a 无状态,同标签必须同维");
        assert!((TAG_HASH_BASE..BRIDGE_VECTOR_DIMS).contains(&d1));
        for tag in ["a", "rust", "chimera-release-analyst", ""] {
            assert!((TAG_HASH_BASE..BRIDGE_VECTOR_DIMS).contains(&tag_hash_dim(tag)));
        }
    }

    #[test]
    fn test_build_gea_expert_profile_preserves_id_and_onehot() {
        // 往返: 编制 ID 保留 + 主责象限 one-hot 恰一维为 1.0
        let registry = ExpertRegistry::new();
        for mas_profile in registry.all() {
            let gea_profile = build_gea_expert_profile(mas_profile);
            assert_eq!(gea_profile.expert_id.as_str(), mas_profile.id);
            assert_eq!(gea_profile.expert_vector.len(), BRIDGE_VECTOR_DIMS);
            let onehot_sum: f32 = gea_profile.expert_vector[0..4].iter().sum();
            assert!(
                (onehot_sum - 1.0).abs() < 1e-6,
                "{} 象限 one-hot 恰一维, got {onehot_sum}",
                mas_profile.id
            );
            let hot_dim = QUADRANT_ONEHOT_BASE + quadrant_dim(mas_profile.primary_quadrant);
            assert_eq!(gea_profile.expert_vector[hot_dim], 1.0);
            // 编制权重派生 priority
            let expected = tier_priority_weight(mas_profile.highest_tier());
            assert!((gea_profile.priority - expected).abs() < 1e-6);
            // 亲和词表: 小写象限名必须入 tags
            let tag = quadrant_affinity_tag(mas_profile.primary_quadrant);
            assert!(gea_profile.capability_tags.contains(&tag));
        }
    }

    #[test]
    fn test_register_mas_experts_idempotent() {
        let gea = gea_activator::GeaActivator::new(
            gea_activator::GeaConfig::default(),
            event_bus::EventBus::new(),
        )
        .expect("默认 config 合法");
        register_mas_experts(&gea);
        assert_eq!(gea.expert_count(), 8, "E01-E08 全量注册");
        register_mas_experts(&gea);
        assert_eq!(gea.expert_count(), 8, "重复注册幂等(覆盖不产生重复)");
    }

    #[test]
    fn test_build_mas_activator_registers_eight() {
        let gea = build_mas_activator(
            gea_activator::GeaConfig::default(),
            event_bus::EventBus::new(),
        )
        .expect("装配应成功");
        assert_eq!(gea.expert_count(), 8);
    }

    #[test]
    fn test_task_profile_from_agent_task_layout() {
        let task = AgentTask::new(
            nexus_core::Task {
                task_id: "t-gea".into(),
                description: "桥接测试".into(),
                status: nexus_core::TaskStatus::Pending,
                dependencies: vec![],
            },
            TaskComplexity::Complex,
            1000,
            std::time::Duration::from_secs(60),
            crate::delegation::QualityLevel::Standard,
        )
        .with_priority(TaskPriority::High);
        let profile = task_profile_from_agent_task(&task);

        assert!((profile.complexity_score - 0.75).abs() < 1e-6);
        assert_eq!(profile.risk_level, 60);
        assert_eq!(profile.clv.len(), BRIDGE_VECTOR_DIMS);
        // Complex → 激活 [Q1, Q2, Q3]:dims[0..3] = 复杂度,dims[3] = 0
        for dim in 0..3 {
            assert!(
                (profile.clv[dim] - 0.75).abs() < 1e-6,
                "dim[{dim}] 应为复杂度"
            );
        }
        assert_eq!(profile.clv[3], 0.0, "Q4 未激活应为 0");
        assert!((profile.clv[WEIGHT_DIM] - 0.75).abs() < 1e-6);
        assert!((profile.clv[RISK_DIM] - 0.6).abs() < 1e-6);
        // task_type 哈希维同值填充(与专家侧标签哈希同表对位)
        let hash_dim = tag_hash_dim(&profile.task_type);
        assert!(
            (profile.clv[hash_dim] - 0.75).abs() < 1e-6,
            "task_type 哈希维[{hash_dim}] 应为复杂度"
        );
        // 亲和词表 = 首激活象限小写名(Complex → Q1 Implementation)
        assert_eq!(profile.task_type, "implementation");
    }

    #[test]
    fn test_mas_gea_config_validates() {
        // 推荐配置必须通过 gea 校验(权重和 = 1.0、阈值/容量合法域)
        let config = mas_gea_config();
        assert!(config.validate().is_ok(), "mas_gea_config 应通过 gea 校验");
        assert!(config.w4_confidence > 0.0, "w4 必须启用(闭环不空转)");
        // 与 default 的差异是刻意的:校准点记录,防止误改回 default
        let default = gea_activator::GeaConfig::default();
        assert_ne!(config.bias, default.bias);
        assert_ne!(config.w4_confidence, default.w4_confidence);
    }

    #[test]
    fn test_task_profile_verycomplex_covers_all_quadrants() {
        let task = AgentTask::new(
            nexus_core::Task {
                task_id: "t-vc".into(),
                description: "全象限".into(),
                status: nexus_core::TaskStatus::Pending,
                dependencies: vec![],
            },
            TaskComplexity::VeryComplex,
            1000,
            std::time::Duration::from_secs(60),
            crate::delegation::QualityLevel::Production,
        );
        let profile = task_profile_from_agent_task(&task);
        for dim in 0..4 {
            assert!(
                (profile.clv[dim] - 1.0).abs() < 1e-6,
                "VeryComplex dim[{dim}] 应为 1.0"
            );
        }
    }
}
