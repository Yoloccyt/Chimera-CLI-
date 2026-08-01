//! 通道亲和降级 — CapabilitySet 加权距离 + 配额耗尽降级选择(MCA §5.5,ADR-068)
//!
//! 对应架构层:L10 Interface(csn-substitutor)
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.5 降级链
//!
//! # 能力相似度降级(P7 去相关)
//! `AffinityQuotaExhausted`(Critical mpsc)触发后,csn 按**能力相似度**找
//! 替代通道:能力相似度 = `CapabilitySet` 加权距离。相似度越高的通道越优先
//! 接管,保证体验退化最小(E1-E5 对等)。
//!
//! # 加权距离设计
//! 核心能力(streaming/tool_calling)权重最高(缺失即不可替代);次要维度
//! (思考支持度/上下文窗口/缓存/状态守恒)按重要性加权。距离越小越相似。
//!
//! # 依赖方向(§2.2 铁律)
//! 本模块依赖 L0 `nexus_contracts::affinity::CapabilitySet`(L10 → L0 合规)。

use nexus_contracts::affinity::{
    CacheSupport, CapabilitySet, StatePreservationPolicy, ThinkingSupport,
};

/// 能力维度权重(距离计算)——核心能力权重最高
struct CapabilityWeights;

impl CapabilityWeights {
    /// 流式(核心:缺失即通道不可用,权重最高)
    const STREAMING: f32 = 3.0;
    /// 工具调用(核心)
    const TOOL_CALLING: f32 = 3.0;
    /// 思考支持度(次要)
    const THINKING: f32 = 1.5;
    /// 上下文窗口档位(次要)
    const WINDOW: f32 = 1.0;
    /// 缓存支持度(次要)
    const CACHE: f32 = 0.8;
    /// 状态守恒策略(次要,MiniMax 迁移相关)
    const STATE: f32 = 1.0;
}

/// 思考支持度序数(距离计算:None < OnOff < EffortLevels)
fn thinking_ordinal(t: &ThinkingSupport) -> f32 {
    match t {
        ThinkingSupport::None => 0.0,
        ThinkingSupport::OnOff => 1.0,
        ThinkingSupport::EffortLevels(_) => 2.0,
    }
}

/// 缓存支持度序数(None < Implicit < ExplicitControl)
fn cache_ordinal(c: CacheSupport) -> f32 {
    match c {
        CacheSupport::None => 0.0,
        CacheSupport::Implicit => 1.0,
        CacheSupport::ExplicitControl => 2.0,
    }
}

/// 状态守恒序数(None < BlockPreservation < VerbatimThinking)
fn state_ordinal(s: StatePreservationPolicy) -> f32 {
    match s {
        StatePreservationPolicy::None => 0.0,
        StatePreservationPolicy::BlockPreservation => 1.0,
        StatePreservationPolicy::VerbatimThinking => 2.0,
    }
}

/// 窗口档位序数(按 128K/512K 阈值分 4 档,与 hcw-window 折减对齐)
fn window_ordinal(context_window: u32) -> f32 {
    if context_window >= 512_000 {
        3.0
    } else if context_window >= 128_000 {
        2.0
    } else if context_window >= 32_000 {
        1.0
    } else {
        0.0
    }
}

/// 能力加权距离 — 两个 CapabilitySet 的加权欧氏距离(越小越相似)
///
/// # 核心能力硬约束
/// 若候选通道缺失被替代通道具备的核心能力(streaming/tool_calling),
/// 距离加上大惩罚(核心能力不可替代);其余维度按加权序数差累加。
pub fn capability_distance(from: &CapabilitySet, to: &CapabilitySet) -> f32 {
    let mut dist = 0.0f32;

    // 核心能力:候选缺失原通道具备的能力 → 大惩罚(不可替代)
    if from.streaming && !to.streaming {
        dist += CapabilityWeights::STREAMING;
    }
    if from.tool_calling && !to.tool_calling {
        dist += CapabilityWeights::TOOL_CALLING;
    }

    // 次要维度:加权序数差(对称距离)
    dist += CapabilityWeights::THINKING
        * (thinking_ordinal(&from.thinking) - thinking_ordinal(&to.thinking)).abs();
    dist += CapabilityWeights::WINDOW
        * (window_ordinal(from.context_window) - window_ordinal(to.context_window)).abs();
    dist += CapabilityWeights::CACHE
        * (cache_ordinal(from.prompt_caching) - cache_ordinal(to.prompt_caching)).abs();
    dist += CapabilityWeights::STATE
        * (state_ordinal(from.state_preservation) - state_ordinal(to.state_preservation)).abs();

    dist
}

/// 从候选通道中选择能力最相似的替代通道(配额耗尽降级)
///
/// # 参数
/// - `exhausted`: 耗尽通道的能力集
/// - `candidates`: 候选通道 `(route_key, CapabilitySet)` 列表(应已排除耗尽通道)
///
/// 返回能力距离最小的候选 route_key(None = 无候选)。
/// 使用 `min_by`(O(n) 单趟)选最相似,不排序全表(对齐 Top-K O(n) 红线精神)。
pub fn select_substitute<'a>(
    exhausted: &CapabilitySet,
    candidates: &'a [(String, CapabilitySet)],
) -> Option<&'a str> {
    candidates
        .iter()
        .map(|(key, caps)| (key, capability_distance(exhausted, caps)))
        // 核心能力缺失(距离含 ≥3.0 惩罚)的候选可被更相似者击败,但若全部
        // 缺核心也仍返回最小者(降级优于无替代;调用方可再经能力协商拒绝)
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(key, _)| key.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(
        streaming: bool,
        tool_calling: bool,
        thinking: ThinkingSupport,
        window: u32,
    ) -> CapabilitySet {
        let mut c = CapabilitySet::minimal_text(window, 8192);
        c.streaming = streaming;
        c.tool_calling = tool_calling;
        c.thinking = thinking;
        c
    }

    #[test]
    fn identical_capabilities_zero_distance() {
        let a = caps(true, true, ThinkingSupport::OnOff, 1_000_000);
        let b = caps(true, true, ThinkingSupport::OnOff, 1_000_000);
        assert!((capability_distance(&a, &b)).abs() < 1e-6);
    }

    #[test]
    fn missing_core_capability_large_distance() {
        // 原通道支持工具,候选不支持 → 距离含大惩罚
        let from = caps(true, true, ThinkingSupport::OnOff, 1_000_000);
        let to = caps(true, false, ThinkingSupport::OnOff, 1_000_000);
        assert!(
            capability_distance(&from, &to) >= CapabilityWeights::TOOL_CALLING,
            "缺核心能力必含大惩罚"
        );
    }

    #[test]
    fn select_substitute_prefers_most_similar() {
        // 耗尽通道:1M 窗口 + 工具 + EffortLevels 思考
        let exhausted = caps(
            true,
            true,
            ThinkingSupport::EffortLevels(vec!["low".into(), "high".into()]),
            1_000_000,
        );
        let candidates = vec![
            // 候选 A:能力接近(1M + 工具 + EffortLevels)
            (
                "zhipu/glm-5.2".to_string(),
                caps(
                    true,
                    true,
                    ThinkingSupport::EffortLevels(vec!["low".into(), "high".into()]),
                    1_000_000,
                ),
            ),
            // 候选 B:缺工具 + 小窗口(距离大)
            (
                "step_fun/step-3.5-flash-2603".to_string(),
                caps(true, false, ThinkingSupport::OnOff, 262_144),
            ),
        ];
        assert_eq!(
            select_substitute(&exhausted, &candidates),
            Some("zhipu/glm-5.2")
        );
    }

    #[test]
    fn select_substitute_empty_returns_none() {
        let exhausted = caps(true, true, ThinkingSupport::OnOff, 1_000_000);
        assert!(select_substitute(&exhausted, &[]).is_none());
    }

    #[test]
    fn window_downgrade_contributes_distance() {
        // 1M → 256K 窗口降级贡献距离(P5 窗口亲和相关)
        let from = caps(true, true, ThinkingSupport::OnOff, 1_000_000);
        let to = caps(true, true, ThinkingSupport::OnOff, 262_144);
        assert!(capability_distance(&from, &to) > 0.0);
    }
}
