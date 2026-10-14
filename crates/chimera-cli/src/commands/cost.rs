//! `chimera cost` — 统一价格向量一期可见性（只读）
//!
//! 对应架构层：**L10 Interface**（消费 L0 `nexus_contracts::price_vector::PriceVector`
//! 与 L1 `nexus_core::config::ProviderConfig`，L10→L0/L1 向下依赖，§2.2 合规）
//!
//! 对应任务：v3.0 收敛 spec §(b.6)「价格向量一期=可见性」。
//!
//! # 为什么存在
//! L0 `PriceVector` 契约已定义但**生产零消费者**（v2.3 §3 E2）。本命令把
//! `model_router.providers` 的每模型单价（配置单位 USD/1k tokens）归一为
//! `PriceVector`（统一单位 micro USD/1k），并只读展示——把"成本"从散落各处的
//! 单价变为一处可见，为二期成本感知路由（Wave 4）预备消费面。
//!
//! # 边界（一期不做）
//! - **不做成本感知路由**（二期/Wave 4）：本命令纯展示，不改变任何路由决策。
//! - **不做精确计费**：本处整数换算仅供可见性估算；精确 token 对账归各 crate 的
//!   `token_ledger`（`estimate_cost_micro` 末位向下取整）。
//!
//! # 降级语义
//! `model_router.providers` 为空（或去重后无条目）→ 向量 `is_empty()`，命令优雅
//! 输出"成本不可见"并返回 `Ok`，不报错（对齐 `price_vector.rs` 的空向量约定）。

use anyhow::Result;

use crate::config::{ChimeraConfig, ProviderConfig};
use crate::output;
use nexus_contracts::price_vector::PriceVector;

/// 配置单价单位「USD / 1k tokens」→ `PriceVector` 单位「micro USD / 1k」。
///
/// `1 USD = 1e6 micro`。四舍五入取整（`.round()`），并防御非有限/负值：
/// - `NaN` / `±inf` → `0`（视为未配置单价）
/// - 负值 → `0`（单价不可能为负，防御误配）
///
/// `f64 as u64` 在 Rust ≥1.45 对越界饱和（不 panic），故超大值安全截顶。
fn usd_per_1k_to_micro(usd: f64) -> u64 {
    if !usd.is_finite() || usd <= 0.0 {
        return 0;
    }
    (usd * 1_000_000.0).round() as u64
}

/// 把 provider 列表归一为统一价格向量（一期可见性的核心聚合）。
///
/// 模型标识取 `provider.id`（与单价表/路由键对齐）。空输入产出空向量。
pub(crate) fn price_vector_from_providers(providers: &[ProviderConfig]) -> PriceVector {
    PriceVector::from_parts(providers.iter().map(|p| {
        (
            p.id.clone(),
            usd_per_1k_to_micro(p.input_cost_per_1k),
            usd_per_1k_to_micro(p.output_cost_per_1k),
        )
    }))
}

/// 构造 `--json` envelope 的 payload（纯函数，便于测试与人类可读路径共用口径）。
///
/// `sample_estimate_micro` = 最便宜模型「1000 in + 1000 out」的整数成本估算，
/// 仅作量级示例（非真实调用）。向量空时 `available=false` 且样本为 `None`。
fn build_payload(pv: &PriceVector) -> serde_json::Value {
    let cheapest = pv.cheapest_model();
    let sample = cheapest.and_then(|m| pv.estimate_cost_micro(&m.model, 1000, 1000));
    serde_json::json!({
        "available": !pv.is_empty(),
        "unit": "micro_usd_per_1k",
        "providers_count": pv.entries.len(),
        "entries": pv.entries,
        "cheapest_model": cheapest.map(|m| m.model.clone()),
        "sample_estimate_cheapest_1000in_1000out_micro": sample,
    })
}

/// 执行 `chimera cost` 子命令 — 只读展示统一价格向量。
///
/// `json=true` 走 [`output::print_json`] envelope；否则人类可读表格 + 最便宜行。
/// 无任何写操作/事件发布。
pub async fn execute(cfg: &ChimeraConfig, json: bool) -> Result<()> {
    let pv = price_vector_from_providers(&cfg.model_router.providers);

    if pv.is_empty() {
        if json {
            output::print_json(&build_payload(&pv))?;
        } else {
            output::print_info(
                "成本不可见：未配置任何 provider 单价（model_router.providers 为空）",
            );
        }
        return Ok(());
    }

    if json {
        output::print_json(&build_payload(&pv))?;
        return Ok(());
    }

    output::print_info(
        "统一价格向量（单位：micro USD / 1k tokens，来源 model_router.providers）：",
    );
    let rows: Vec<Vec<String>> = pv
        .entries
        .iter()
        .map(|e| {
            vec![
                e.model.clone(),
                e.input_micro_per_1k.to_string(),
                e.output_micro_per_1k.to_string(),
            ]
        })
        .collect();
    output::print_table(&["model", "input_micro/1k", "output_micro/1k"], &rows);
    if let Some(cheap) = pv.cheapest_model() {
        output::print_success(&format!(
            "最便宜模型（in+out 单价和最低）：{}（{} + {} micro/1k）",
            cheap.model, cheap.input_micro_per_1k, cheap.output_micro_per_1k
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;

    fn provider(id: &str, input: f64, output: f64) -> ProviderConfig {
        ProviderConfig {
            id: id.to_string(),
            input_cost_per_1k: input,
            output_cost_per_1k: output,
            ..Default::default()
        }
    }

    #[test]
    fn converts_usd_per_1k_to_micro() {
        // 配置 15.0 USD/1k → 15_000_000 micro/1k；2.5 → 2_500_000
        let pv = price_vector_from_providers(&[
            provider("claude-opus", 15.0, 75.0),
            provider("gpt-4o", 2.5, 10.0),
        ]);
        assert_eq!(pv.input_price("claude-opus"), Some(15_000_000));
        assert_eq!(
            pv.estimate_cost_micro("gpt-4o", 1000, 1000),
            Some(2_500_000 + 10_000_000)
        );
    }

    #[test]
    fn cheapest_by_average_sum() {
        let pv = price_vector_from_providers(&[
            provider("exp", 30.0, 60.0),
            provider("cheap", 0.1, 0.2),
            provider("mid", 5.0, 10.0),
        ]);
        assert_eq!(pv.cheapest_model().map(|m| m.model.as_str()), Some("cheap"));
    }

    #[test]
    fn defensive_nonfinite_and_negative_become_zero() {
        assert_eq!(usd_per_1k_to_micro(f64::NAN), 0);
        assert_eq!(usd_per_1k_to_micro(f64::INFINITY), 0);
        assert_eq!(usd_per_1k_to_micro(-3.0), 0);
        assert_eq!(usd_per_1k_to_micro(0.0), 0);
        // 0.0000004 USD → 0.4 micro → round → 0
        assert_eq!(usd_per_1k_to_micro(0.0000004), 0);
        // 0.000001 USD → 1.0 micro
        assert_eq!(usd_per_1k_to_micro(0.000001), 1);
    }

    #[test]
    fn empty_providers_yield_empty_vector() {
        let pv = price_vector_from_providers(&[]);
        assert!(pv.is_empty());
        assert!(pv.cheapest_model().is_none());
    }

    #[test]
    fn payload_shape_available_and_flags() {
        let pv = price_vector_from_providers(&[provider("m1", 1.0, 2.0)]);
        let v = build_payload(&pv);
        assert_eq!(v["available"], serde_json::json!(true));
        assert_eq!(v["providers_count"], serde_json::json!(1));
        assert_eq!(v["unit"], serde_json::json!("micro_usd_per_1k"));
        assert_eq!(v["cheapest_model"], serde_json::json!("m1"));
        // 空向量：available=false，样本 null
        let empty = build_payload(&PriceVector::default());
        assert_eq!(empty["available"], serde_json::json!(false));
        assert!(empty["sample_estimate_cheapest_1000in_1000out_micro"].is_null());
    }

    mod proptest_price {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// 聚合保真：任意非负 (in,out) USD → estimate(1000,1000) 恰等于 in+out micro；
            /// 单价越大，估算成本不减（单调）。
            #[test]
            fn estimate_matches_sum_and_monotonic(
                in_usd in 0f64..10_000.0,
                out_usd in 0f64..10_000.0,
            ) {
                let pv = price_vector_from_providers(&[provider("m", in_usd, out_usd)]);
                let est = pv.estimate_cost_micro("m", 1000, 1000).unwrap();
                prop_assert_eq!(est, usd_per_1k_to_micro(in_usd) + usd_per_1k_to_micro(out_usd));
            }

            /// 换算非负且对 (in+out) 单调不减。
            #[test]
            fn conversion_nonneg_monotonic(a in 0f64..1_000_000.0, b in 0f64..1_000_000.0) {
                let (x, y) = (a.max(b), a.min(b));
                prop_assert!(usd_per_1k_to_micro(x) >= usd_per_1k_to_micro(y));
            }
        }
    }
}
