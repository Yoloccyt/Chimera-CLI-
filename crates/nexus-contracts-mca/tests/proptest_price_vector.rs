//! 属性测试（W2b）— PriceVector 成本算术一致性
//!
//! WHY: price_vector 是 MCA 成本感知路由的 L0 契约面；cheapest_model 的
//! 排序键（in+out 合计）与 estimate_cost_micro 的整除取整语义由属性测试
//! 对参考实现钉死，防止排序键或取整方向被静默改动。
//! ModelPrice 为 #[non_exhaustive]，经 `PriceVector::from_parts` 构造。
#![forbid(unsafe_code)]

use nexus_contracts_mca::mca::price_vector::PriceVector;
use proptest::prelude::*;

fn parts_strategy() -> impl Strategy<Value = (String, u64, u64)> {
    ("m-[a-z]{1,6}", 1u64..=1_000_000u64, 1u64..=1_000_000u64)
        .prop_map(|(model, input, output)| (model, input, output))
}

proptest! {
    /// cheapest_model 必须等于 (input+output) 的 argmin；空向量时返回 None。
    /// 并列最小取首个（min_by 稳定性），两侧同语义故按 model 名比对。
    #[test]
    fn cheapest_is_min_total(parts in prop::collection::vec(parts_strategy(), 0..8)) {
        let vector = PriceVector::from_parts(parts);
        let got = vector.cheapest_model().map(|e| e.model.as_str());
        let want = vector
            .entries
            .iter()
            .min_by_key(|e| e.input_micro_per_1k + e.output_micro_per_1k)
            .map(|e| e.model.as_str());
        prop_assert_eq!(got, want);
    }

    /// estimate_cost_micro 与手工整除计算一致；未知模型返回 None。
    #[test]
    fn estimate_matches_manual(in_tok in 0u64..1_000_000u64, out_tok in 0u64..1_000_000u64) {
        let vector = PriceVector::from_parts(vec![("m", 300, 1500)]);
        let expected = in_tok * 300 / 1000 + out_tok * 1500 / 1000;
        prop_assert_eq!(vector.estimate_cost_micro("m", in_tok, out_tok), Some(expected));
        prop_assert_eq!(vector.estimate_cost_micro("absent", in_tok, out_tok), None);
    }
}
