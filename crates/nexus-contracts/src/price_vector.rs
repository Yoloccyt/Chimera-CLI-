//! 统一价格向量契约 — 聚合分散的模型单价为可路由消费的向量（v2.3 §(b.6)/(c)）
//!
//! 对应架构层: **L0 Contracts**（纯类型 + 无 IO 纯函数，ADR-033 先例）
//!
//! # WHY
//! v2.3 核验（§3 E2）发现：**价格数据本身已存在**（`chimera-cli` config 五模型单价、
//! `mca-gateway` `capability.TOKEN_PRICE_MICRO`、`cost.rs` 预估/回算、`token_ledger`），
//! 缺的不是数据而是一个**统一价格向量 + 路由消费面**（现状 `hcw_integration` 只读
//! `context_window`，`CONSUMERS:0`）。本模块定义这个聚合契约：
//! - **一期（本契约）**：可见性 —— 把各模型单价归一为一个 [`PriceVector`]，可被 `/bench`
//!   展示与后续成本感知路由消费；
//! - **二期**：成本感知路由（Wave 4 条件触发），复用 `ModelAffinitySelected.cost_estimate_micro` 留痕。
//!
//! 单位沿用 mca 口径：`micro_per_1k` = 每 1000 tokens 的**微美元**（1e-6 USD），整数运算避免
//! 浮点不确定性。纯类型零逻辑（除整数估算纯函数外），不引入任何外部 crate。

use serde::{Deserialize, Serialize};

/// 单个模型的 token 单价（微美元 / 1k tokens）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelPrice {
    /// 模型标识（与 `config` 单价表 / `ProviderId` 模型名对齐）
    pub model: String,
    /// 输入单价：每 1000 input tokens 的微美元
    pub input_micro_per_1k: u64,
    /// 输出单价：每 1000 output tokens 的微美元
    pub output_micro_per_1k: u64,
}

/// 统一价格向量 —— 多模型单价的聚合体，提供成本估算与最便宜模型查询。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PriceVector {
    /// 各模型单价条目
    pub entries: Vec<ModelPrice>,
}

impl PriceVector {
    /// 由条目构造。
    #[must_use]
    pub fn new(entries: Vec<ModelPrice>) -> Self {
        Self { entries }
    }

    /// 便捷构造：`(model, input_micro_per_1k, output_micro_per_1k)` 三元组序列。
    #[must_use]
    pub fn from_parts<I, S>(it: I) -> Self
    where
        I: IntoIterator<Item = (S, u64, u64)>,
        S: Into<String>,
    {
        Self {
            entries: it
                .into_iter()
                .map(|(m, i, o)| ModelPrice {
                    model: m.into(),
                    input_micro_per_1k: i,
                    output_micro_per_1k: o,
                })
                .collect(),
        }
    }

    /// 查某模型的输入单价（微美元/1k）。
    #[must_use]
    pub fn input_price(&self, model: &str) -> Option<u64> {
        self.entries
            .iter()
            .find(|e| e.model == model)
            .map(|e| e.input_micro_per_1k)
    }

    /// 估算一次调用的总成本（微美元）：`in_tok/1000*in_price + out_tok/1000*out_price`。
    ///
    /// 整数运算，末位向下取整（对成本可见性足够；精确对账由各 crate 的 `token_ledger` 负责）。
    /// 模型不在向量内返回 `None`。
    #[must_use]
    pub fn estimate_cost_micro(&self, model: &str, in_tok: u64, out_tok: u64) -> Option<u64> {
        let e = self.entries.iter().find(|x| x.model == model)?;
        let in_cost = in_tok.saturating_mul(e.input_micro_per_1k) / 1000;
        let out_cost = out_tok.saturating_mul(e.output_micro_per_1k) / 1000;
        Some(in_cost + out_cost)
    }

    /// 平均单价（in+out）最低的模型，供成本感知路由一期"可见性"排序参考。
    #[must_use]
    pub fn cheapest_model(&self) -> Option<&ModelPrice> {
        self.entries
            .iter()
            .min_by_key(|e| e.input_micro_per_1k + e.output_micro_per_1k)
    }

    /// 向量是否为空（无价格数据时上层应降级为"成本不可见"而非报错）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec() -> PriceVector {
        PriceVector::from_parts([
            ("cheap-model", 100_u64, 200_u64),
            ("mid-model", 500_u64, 1000_u64),
            ("expensive-model", 3000_u64, 6000_u64),
        ])
    }

    #[test]
    fn estimate_cost_integer_math() {
        let pv = vec();
        // 2000 in *100/1k + 1000 out *200/1k = 200 + 200 = 400 微美元
        assert_eq!(pv.estimate_cost_micro("cheap-model", 2000, 1000), Some(400));
    }

    #[test]
    fn unknown_model_none() {
        assert_eq!(vec().estimate_cost_micro("ghost", 10, 10), None);
        assert_eq!(vec().input_price("ghost"), None);
        assert_eq!(vec().input_price("mid-model"), Some(500));
    }

    #[test]
    fn cheapest_by_average() {
        let pv = vec();
        assert_eq!(
            pv.cheapest_model().map(|m| m.model.as_str()),
            Some("cheap-model")
        );
    }

    #[test]
    fn empty_vector_is_empty_and_no_cheapest() {
        let pv = PriceVector::default();
        assert!(pv.is_empty());
        assert!(pv.cheapest_model().is_none());
    }
}
