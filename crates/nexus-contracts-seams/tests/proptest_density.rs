//! 属性测试（W2b）— DensityTier::from_rho 最近档搜索不变量
//!
//! WHY: from_rho 是 seams 的受控纯逻辑（G-119 登记），其「取绝对值最近的档」
//! 语义此前只有单元测试点例；属性测试把「任意输入 → 返回档 = argmin 距离」
//! 钉死，防止将来改档表/改比较方向时静默漂移。
#![forbid(unsafe_code)]

use nexus_contracts_seams::seams::density::DensityTier;
use proptest::prelude::*;

fn nearest(rho: f64) -> DensityTier {
    DensityTier::ALL
        .iter()
        .copied()
        .min_by(|a, b| {
            (a.rho() - rho)
                .abs()
                .partial_cmp(&(b.rho() - rho).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap()
}

proptest! {
    /// 任意档位的自身 ρ 值必须被 from_rho 无失真映射回自身
    /// （4 档 ρ 值互异 ⇒ 最近档 = 自身；将来新增重叠档位时此属性立即变红）。
    #[test]
    fn from_rho_roundtrip_identity(tier in prop::sample::select(DensityTier::ALL.to_vec())) {
        prop_assert_eq!(DensityTier::from_rho(tier.rho()), tier);
    }

    /// 任意 ρ ∈ [0, 10] 的返回档必须等于参考实现的 argmin（min_by 语义钉死）。
    #[test]
    fn from_rho_returns_nearest(rho in 0.0f64..10.0f64) {
        prop_assert_eq!(DensityTier::from_rho(rho), nearest(rho));
    }
}
