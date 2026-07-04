//! 策略变异 — 基于 MutationType 生成扰动候选并应用到策略参数
//!
//! 对应架构层:L5 Knowledge
//!
//! # 变异类型
//! - `Gaussian`:幅度 = rate * N(0,1) 近似,精细调优
//! - `Uniform`:幅度 = rate * U(-1,1),大范围探索
//! - `Elite`:幅度 = 0,精英直接传承不变异

use crate::error::GsoeError;
use crate::policy::grpo;
use crate::types::{EvolutionPolicy, MutationCandidate, MutationType};

/// 基于 mutation_type 生成变异候选
///
/// 本周占位:用线性同余 PRNG 生成伪随机数(不引入 rand crate)
/// // TODO(Week 7): 接入 MCP Mesh 真实模型,用 logits 引导变异方向
pub fn mutate(policy: &EvolutionPolicy, rate: f32) -> Result<MutationCandidate, GsoeError> {
    if !(0.0..=1.0).contains(&rate) {
        return Err(GsoeError::MutationFailed {
            reason: format!("rate={rate} 超出 [0.0, 1.0]"),
        });
    }

    // 基于 policy 字段哈希生成 seed,保证可复现
    let seed = hash_seed(&policy.mutation_rate.to_le_bytes())
        .wrapping_add(hash_seed(&policy.selection_pressure.to_le_bytes()));

    let magnitude = match select_mutation_type(policy) {
        MutationType::Gaussian => {
            // Box-Muller 近似:N(0,1) ≈ (u1 + u2 + u3 - 1.5) * sqrt(2)
            // 用 3 个均匀分布的和近似正态分布(CLT 中心极限定理)
            let mut rng = grpo::Lcg::new(seed);
            let u1 = rng.next_f32();
            let u2 = rng.next_f32();
            let u3 = rng.next_f32();
            let gaussian = (u1 + u2 + u3 - 1.5) * 0.8165; // sqrt(2/3)
            rate * gaussian
        }
        MutationType::Uniform => {
            let mut rng = grpo::Lcg::new(seed);
            rate * rng.next_f32()
        }
        MutationType::Elite => 0.0,
    };

    Ok(MutationCandidate {
        policy_id: format!("policy-{}", seed % 10000),
        mutation_type: select_mutation_type(policy),
        magnitude,
    })
}

/// 原地应用变异候选到策略参数
///
/// 变异作用于 `mutation_rate` 与 `selection_pressure` 两个字段:
/// - `mutation_rate` += magnitude * 0.1(小幅扰动)
/// - `selection_pressure` += magnitude * 0.5(中幅扰动)
///
/// 变异后自动 clamp 到合法范围,防止参数越界。
pub fn apply_mutation(policy: &mut EvolutionPolicy, candidate: &MutationCandidate) {
    // Elite 不变异
    if candidate.mutation_type == MutationType::Elite {
        return;
    }

    // mutation_rate 扰动系数 0.1(小幅)
    let mr_delta = candidate.magnitude * 0.1;
    policy.mutation_rate = (policy.mutation_rate + mr_delta).clamp(0.001, 1.0);

    // selection_pressure 扰动系数 0.5(中幅)
    let sp_delta = candidate.magnitude * 0.5;
    policy.selection_pressure = (policy.selection_pressure + sp_delta).max(0.0);
}

/// 根据 policy 当前状态选择变异类型
///
/// 规则:elite_ratio > 0.5 时倾向 Elite(高精英比例时减少探索)
fn select_mutation_type(policy: &EvolutionPolicy) -> MutationType {
    if policy.elite_ratio > 0.5 {
        MutationType::Elite
    } else if policy.mutation_rate > 0.15 {
        MutationType::Gaussian
    } else {
        MutationType::Uniform
    }
}

/// 简单哈希函数 — 将字节序列混合为 u64 seed
fn hash_seed(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0x517cc1b727220a95;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_policy() -> EvolutionPolicy {
        EvolutionPolicy::new(0.1, 1.5, 0.2, 8).unwrap()
    }

    #[test]
    fn test_mutate_returns_valid_candidate() {
        let policy = make_policy();
        let candidate = mutate(&policy, 0.1).unwrap();
        assert!(!candidate.policy_id.is_empty());
    }

    #[test]
    fn test_mutate_invalid_rate_negative() {
        let policy = make_policy();
        let err = mutate(&policy, -0.1).unwrap_err();
        assert!(matches!(err, GsoeError::MutationFailed { .. }));
    }

    #[test]
    fn test_mutate_invalid_rate_overflow() {
        let policy = make_policy();
        let err = mutate(&policy, 1.5).unwrap_err();
        assert!(matches!(err, GsoeError::MutationFailed { .. }));
    }

    #[test]
    fn test_mutate_elite_type_zero_magnitude() {
        // elite_ratio > 0.5 → Elite 类型 → magnitude = 0
        let policy = EvolutionPolicy::new(0.1, 1.5, 0.6, 8).unwrap();
        let candidate = mutate(&policy, 0.1).unwrap();
        assert_eq!(candidate.mutation_type, MutationType::Elite);
        assert_eq!(candidate.magnitude, 0.0);
    }

    #[test]
    fn test_mutate_gaussian_type() {
        // mutation_rate > 0.15 且 elite_ratio <= 0.5 → Gaussian
        let policy = EvolutionPolicy::new(0.2, 1.5, 0.2, 8).unwrap();
        let candidate = mutate(&policy, 0.1).unwrap();
        assert_eq!(candidate.mutation_type, MutationType::Gaussian);
        // Gaussian magnitude 在 [-0.1*3*0.8165, 0.1*3*0.8165] 范围内
        let bound = 0.1 * 3.0 * 0.8165;
        assert!(
            candidate.magnitude.abs() <= bound + 1e-6,
            "Gaussian magnitude 超出预期范围: {}",
            candidate.magnitude
        );
    }

    #[test]
    fn test_mutate_uniform_type() {
        // mutation_rate <= 0.15 且 elite_ratio <= 0.5 → Uniform
        let policy = EvolutionPolicy::new(0.1, 1.5, 0.2, 8).unwrap();
        let candidate = mutate(&policy, 0.1).unwrap();
        assert_eq!(candidate.mutation_type, MutationType::Uniform);
        // Uniform magnitude 在 [-0.1, 0.1] 范围内
        assert!(
            candidate.magnitude.abs() <= 0.1 + 1e-6,
            "Uniform magnitude 超出预期范围: {}",
            candidate.magnitude
        );
    }

    #[test]
    fn test_apply_mutation_elite_no_change() {
        let mut policy = make_policy();
        let original_mr = policy.mutation_rate;
        let original_sp = policy.selection_pressure;
        let candidate = MutationCandidate {
            policy_id: "p-1".into(),
            mutation_type: MutationType::Elite,
            magnitude: 0.5,
        };
        apply_mutation(&mut policy, &candidate);
        assert_eq!(policy.mutation_rate, original_mr);
        assert_eq!(policy.selection_pressure, original_sp);
    }

    #[test]
    fn test_apply_mutation_clamps_mutation_rate() {
        let mut policy = make_policy();
        // 大幅度正向变异 → mutation_rate 应 clamp 到 1.0
        let candidate = MutationCandidate {
            policy_id: "p-1".into(),
            mutation_type: MutationType::Uniform,
            magnitude: 100.0,
        };
        apply_mutation(&mut policy, &candidate);
        assert!(policy.mutation_rate <= 1.0);
        assert!(policy.mutation_rate > 0.0);
    }

    #[test]
    fn test_apply_mutation_clamps_mutation_rate_lower_bound() {
        let mut policy = make_policy();
        // 大幅度负向变异 → mutation_rate 应 clamp 到 0.001
        let candidate = MutationCandidate {
            policy_id: "p-1".into(),
            mutation_type: MutationType::Uniform,
            magnitude: -100.0,
        };
        apply_mutation(&mut policy, &candidate);
        assert!(policy.mutation_rate >= 0.001);
    }

    #[test]
    fn test_apply_mutation_selection_pressure_non_negative() {
        let mut policy = make_policy();
        let candidate = MutationCandidate {
            policy_id: "p-1".into(),
            mutation_type: MutationType::Uniform,
            magnitude: -100.0,
        };
        apply_mutation(&mut policy, &candidate);
        assert!(policy.selection_pressure >= 0.0);
    }

    #[test]
    fn test_apply_mutation_zero_magnitude_no_change() {
        let mut policy = make_policy();
        let original_mr = policy.mutation_rate;
        let original_sp = policy.selection_pressure;
        let candidate = MutationCandidate {
            policy_id: "p-1".into(),
            mutation_type: MutationType::Uniform,
            magnitude: 0.0,
        };
        apply_mutation(&mut policy, &candidate);
        assert_eq!(policy.mutation_rate, original_mr);
        assert_eq!(policy.selection_pressure, original_sp);
    }
}
