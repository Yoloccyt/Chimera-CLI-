//! cost — 成本预估/回算纯函数(与 cost_guard 共同构成成本子域)
//!
//! 对应架构层: L10 Interface(mca-gateway)
//!
//! # 职责边界
//! - 本模块: 纯计算(预估/回算/峰谷系数), 无状态, 无副作用
//! - `cost_guard`: 有状态熔断(累计成本 + 超限拒绝), 原子无锁
//!
//! # 设计决策
//! 从 `adapters.rs` 提取为独立模块(Wave 2 可维护性优化):
//! 成本计算是跨请求复用的纯函数, 与请求编排(invoke 流水线)职责正交;
//! 物理分离后 adapters.rs 聚焦编排逻辑, 本模块可独立测试与基准。

use chrono::Timelike;
use nexus_contracts::affinity::{AffinityRequest, CostEstimate, PricingSpec, UsageReport};

/// 当前小时(0-23,厂商计费时区按本地时钟近似)
pub(crate) fn current_hour() -> u8 {
    chrono::Local::now().hour() as u8
}

/// 峰谷系数查表(小时桶 O(1),命中首条规则;无规则 = 100%)
pub(crate) fn peak_factor(pricing: &PricingSpec, hour: u8) -> u16 {
    for p in &pricing.peak_periods {
        // 常规区间 [start, end);跨零点规则由配置拆两条(spec_loader 文档约定)
        if p.start_hour <= hour && hour < p.end_hour {
            return p.factor_percent;
        }
    }
    100
}

/// 请求前成本预估 — 粗粒度字符启发式(P6:路由决策必须附带预估)
///
/// WHY 字符/4 启发式: 请求前无真实 token 数,中英文混排下 1 token ≈ 3-4
/// 字符是业界通用近似;预估只用于路由权重与预算预检,实际成本以
/// usage 回算为准(actual_cost),偏差由 acb-governor EWMA 自校正。
pub(crate) fn estimate_cost(
    pricing: &PricingSpec,
    request: &AffinityRequest,
    hour: u8,
) -> CostEstimate {
    let chars: usize = request
        .messages
        .iter()
        .flat_map(|m| &m.blocks)
        .map(|b| match b {
            nexus_contracts::affinity::ContentBlock::Text { text } => text.len(),
            nexus_contracts::affinity::ContentBlock::Thinking { thinking, .. } => thinking.len(),
            nexus_contracts::affinity::ContentBlock::ToolUse { input_json, .. } => input_json.len(),
            nexus_contracts::affinity::ContentBlock::ToolResult { content, .. } => content.len(),
        })
        .sum();
    let est_input_tokens = (chars / 4) as u64;
    let factor = peak_factor(pricing, hour);
    // 整数微元:tokens × 价(微元/百万) / 1M × 峰谷百分比 / 100
    let total =
        est_input_tokens * pricing.input_micro_per_mtok / 1_000_000 * u64::from(factor) / 100;
    CostEstimate {
        total_micro: total,
        peak_factor_percent: factor,
        cache_discount_micro: 0,
    }
}

/// 真实成本回算 — usage 三元组 × 定价(缓存命中按折扣价计)
pub(crate) fn actual_cost(pricing: &PricingSpec, usage: &UsageReport, hour: u8) -> CostEstimate {
    let factor = u64::from(peak_factor(pricing, hour));
    // 缓存命中部分按 cache_hit 价,未命中输入按全价(DeepSeek/豆包计费口径)
    let cached = usage.cache_hit_tokens.min(usage.input_tokens);
    let uncached = usage.input_tokens - cached;
    let input_cost = uncached * pricing.input_micro_per_mtok / 1_000_000;
    let cache_cost = cached * pricing.cache_hit_micro_per_mtok / 1_000_000;
    let output_cost = usage.output_tokens * pricing.output_micro_per_mtok / 1_000_000;
    let full_price_would_be = usage.input_tokens * pricing.input_micro_per_mtok / 1_000_000;
    CostEstimate {
        total_micro: (input_cost + cache_cost + output_cost) * factor / 100,
        peak_factor_percent: factor as u16,
        cache_discount_micro: full_price_would_be.saturating_sub(input_cost + cache_cost) * factor
            / 100,
    }
}
