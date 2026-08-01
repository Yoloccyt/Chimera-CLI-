//! 窗口亲和折减 — HCW 分层窗口按模型实际上限折减(MCA P5,ADR-065/066)
//!
//! 对应架构层:L2 Memory(hcw-window)
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.2 窗口亲和映射
//!
//! # 承诺不超发(P5)
//! HCW 四级窗口(L0=4K/L1=32K/L2=128K/L3=1M 等效)加载前必须先经模型
//! 实际上限折减:小窗口模型接入时,1M 等效上下文承诺会破灭。折减规则:
//!
//! | 模型实际上限 | HCW 允许最高档 | 依据 |
//! |-------------|---------------|------|
//! | ≥ 512K | L3(1M/512K 等效) | GLM/DeepSeek/MiniMax 1M;MiniMax 保底 512K |
//! | ≥ 128K | L2 封顶 | Step 256K → 禁 1M 等效,超 128K 触发任务分块 |
//! | ≥ 32K | L1 封顶 | — |
//! | < 32K | L0 封顶 | — |
//!
//! # WHY 纯函数 + 无 mca-gateway 依赖
//! 折减只需模型 `context_window`(u32),不引入 L10 依赖(依赖铁律)。
//! 网关经 `ModelAffinitySelected` 事件下发 context_window,HCW 消费后折减。
//! 折减是 O(1) 查表,不进任何热路径分配。

use crate::types::WindowTier;

/// L1 窗口所需的最小模型上限(32K)
const L1_MIN_WINDOW: u32 = 32_000;
/// L2 窗口所需的最小模型上限(128K)
const L2_MIN_WINDOW: u32 = 128_000;
/// L3 窗口所需的最小模型上限(512K;低于此 L3 的 1M 等效承诺不诚实)
///
/// WHY 512K 而非 1M: MiniMax API 保底 512K 即允许 L3(等效 512K = 128K +
/// 4× 稀疏);1M 模型是 128K + 8× 稀疏。两者都用 L3 档,等效上限由 window 界定。
const L3_MIN_WINDOW: u32 = 512_000;

/// 折减结果 — 折后档位 + 是否发生折减(留痕/事件用)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldResult {
    /// 折减后的实际允许档位
    pub tier: WindowTier,
    /// 是否发生了折减(请求档位 > 模型允许上限)
    pub folded: bool,
    /// 是否需要任务分块(请求档位对应 token 超模型上限,如 Step 256K 的超 128K 部分)
    pub needs_chunking: bool,
}

/// 窗口亲和折减器 — 按模型实际上限钳制 HCW 档位(P5 承诺不超发)
pub struct WindowAffinity;

impl WindowAffinity {
    /// 模型实际上限允许的最高 HCW 档位
    ///
    /// O(1) 阈值查表(从高到低,命中即返回)。
    pub fn max_tier_for_window(context_window: u32) -> WindowTier {
        if context_window >= L3_MIN_WINDOW {
            WindowTier::L3
        } else if context_window >= L2_MIN_WINDOW {
            WindowTier::L2
        } else if context_window >= L1_MIN_WINDOW {
            WindowTier::L1
        } else {
            WindowTier::L0
        }
    }

    /// 折减:请求档位与模型允许上限取小(承诺不超发,P5)
    ///
    /// # 分块判定
    /// `needs_chunking` = 请求档位被折减且折减到 L2 封顶(Step 256K 样本):
    /// 128K 以上的上下文无法整体加载,必须触发任务分块(chimera-mas chunker)。
    pub fn fold(requested: WindowTier, context_window: u32) -> FoldResult {
        let max_allowed = Self::max_tier_for_window(context_window);
        // WindowTier 派生 Ord(L0 < L1 < L2 < L3),取小即折减
        let tier = requested.min(max_allowed);
        let folded = tier != requested;
        // 分块:折减发生 且 模型上限落在 L2 封顶(禁 1M 等效的中等窗口)
        let needs_chunking = folded && max_allowed == WindowTier::L2;
        FoldResult {
            tier,
            folded,
            needs_chunking,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]
        #[test]
        fn fold_reduces_to_max_allowed_tier(
            requested in 0u8..4,  // 0=L0, 1=L1, 2=L2, 3=L3
            context_window in 1u32..=2_000_000,
        ) {
            let tiers = [WindowTier::L0, WindowTier::L1, WindowTier::L2, WindowTier::L3];
            let req = tiers[requested as usize];
            let result = WindowAffinity::fold(req, context_window);
            let max_allowed = WindowAffinity::max_tier_for_window(context_window);
            // 折减后档位不能超过模型允许的最高档
            prop_assert!(result.tier <= max_allowed, "tier must not exceed max_allowed");
            // 如果没有折减,档位应等于请求档位
            if !result.folded {
                prop_assert_eq!(result.tier, req, "not folded => tier == requested");
            }
            // 分块标记仅当折减到 L2 时成立
            if result.needs_chunking {
                prop_assert_eq!(result.tier, WindowTier::L2, "chunking only when capped at L2");
            }
        }

        #[test]
        fn fold_is_deterministic(
            requested in 0u8..4,
            context_window in 1u32..=2_000_000,
        ) {
            let tiers = [WindowTier::L0, WindowTier::L1, WindowTier::L2, WindowTier::L3];
            let req = tiers[requested as usize];
            let a = WindowAffinity::fold(req, context_window);
            let b = WindowAffinity::fold(req, context_window);
            prop_assert_eq!(a, b, "折减必须幂等");
        }
    }

    #[test]
    fn one_million_window_allows_all_tiers() {
        // GLM/DeepSeek/MiniMax 1M:请求任意档均不折减
        for requested in [
            WindowTier::L0,
            WindowTier::L1,
            WindowTier::L2,
            WindowTier::L3,
        ] {
            let r = WindowAffinity::fold(requested, 1_000_000);
            assert_eq!(r.tier, requested);
            assert!(!r.folded);
            assert!(!r.needs_chunking);
        }
    }

    #[test]
    fn step_256k_caps_at_l2_and_triggers_chunking() {
        // Step 256K:请求 L3(1M 等效)被折减到 L2 封顶,触发分块(P5 最严样本)
        let r = WindowAffinity::fold(WindowTier::L3, 262_144);
        assert_eq!(r.tier, WindowTier::L2, "256K 必须封顶 L2,禁 1M 等效");
        assert!(r.folded);
        assert!(r.needs_chunking, "超 128K 部分需任务分块");
    }

    #[test]
    fn step_256k_low_tier_request_not_folded() {
        // 256K 模型请求 L1:在允许范围内,不折减不分块
        let r = WindowAffinity::fold(WindowTier::L1, 262_144);
        assert_eq!(r.tier, WindowTier::L1);
        assert!(!r.folded);
        assert!(!r.needs_chunking);
    }

    #[test]
    fn minimax_512k_floor_allows_l3() {
        // MiniMax API 保底 512K:L3 允许(等效 512K = 128K + 4× 稀疏)
        let r = WindowAffinity::fold(WindowTier::L3, 524_288);
        assert_eq!(r.tier, WindowTier::L3);
        assert!(!r.folded);
    }

    #[test]
    fn max_tier_thresholds() {
        assert_eq!(WindowAffinity::max_tier_for_window(4_000), WindowTier::L0);
        assert_eq!(WindowAffinity::max_tier_for_window(32_000), WindowTier::L1);
        assert_eq!(WindowAffinity::max_tier_for_window(128_000), WindowTier::L2);
        assert_eq!(WindowAffinity::max_tier_for_window(512_000), WindowTier::L3);
        assert_eq!(
            WindowAffinity::max_tier_for_window(1_048_576),
            WindowTier::L3
        );
    }

    #[test]
    fn tiny_window_caps_at_l0() {
        let r = WindowAffinity::fold(WindowTier::L3, 4_096);
        assert_eq!(r.tier, WindowTier::L0);
        assert!(r.folded);
        // L0 封顶不标记分块(分块仅针对 L2 封顶的中等窗口)
        assert!(!r.needs_chunking);
    }
}
