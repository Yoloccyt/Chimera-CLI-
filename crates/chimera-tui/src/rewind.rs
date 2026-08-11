//! Esc Esc 双击回退检测器(Concord W4 T4.5,Claude Code 对齐)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! 方案 §7.4:Esc Esc 为会话回退键。纯函数检测器只回答"两次 Esc 是否构成
//! 双击"(时间窗口判定),回退动作语义(弹 overlay/诚实提示)归执行层。
//!
//! Chat 视图单 Esc 不退出(失焦/回退链);Dashboard 保留 Esc 退出肌肉记忆。

/// 双击窗口默认值(毫秒)
pub const DOUBLE_ESC_WINDOW_MS: u64 = 500;

/// 判定两次 Esc 是否构成双击
///
/// # 参数
/// - `prev_esc_ms`:上一次 Esc 的时间戳(毫秒;None 表示首次)
/// - `now_ms`:本次 Esc 的时间戳(毫秒)
/// - `window_ms`:双击窗口长度(毫秒)
///
/// # 返回
/// true = 构成双击(应触发回退);false = 首次/超窗(应仅记录)
///
/// # 边界语义
/// - 时间戳回退(now < prev,时钟调整)视为非双击,防误触发;
/// - 恰好等于窗口边界视为双击(闭区间,与主流编辑器口径一致)。
pub fn is_double_esc(prev_esc_ms: Option<u64>, now_ms: u64, window_ms: u64) -> bool {
    prev_esc_ms.is_some_and(|prev| now_ms >= prev && now_ms.saturating_sub(prev) <= window_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn first_esc_is_never_double() {
        assert!(!is_double_esc(None, 1000, DOUBLE_ESC_WINDOW_MS));
    }

    #[test]
    fn within_window_is_double() {
        assert!(is_double_esc(Some(1000), 1499, DOUBLE_ESC_WINDOW_MS));
    }

    #[test]
    fn boundary_equal_window_is_double() {
        // 闭区间边界:恰好 500ms 仍算双击
        assert!(is_double_esc(Some(1000), 1500, DOUBLE_ESC_WINDOW_MS));
    }

    #[test]
    fn beyond_window_is_not_double() {
        assert!(!is_double_esc(Some(1000), 1501, DOUBLE_ESC_WINDOW_MS));
    }

    #[test]
    fn clock_backwards_is_not_double() {
        // 时钟回拨防御:now < prev 不触发
        assert!(!is_double_esc(Some(2000), 1000, DOUBLE_ESC_WINDOW_MS));
    }

    proptest! {
        /// 窗口内任意间隔恒为双击;窗外恒非(单调时间戳前提下)
        #[test]
        fn window_boundary_property(base in 0u64..1_000_000, gap in 0u64..2000) {
            let now = base + gap;
            let expect = gap <= DOUBLE_ESC_WINDOW_MS;
            prop_assert_eq!(
                is_double_esc(Some(base), now, DOUBLE_ESC_WINDOW_MS),
                expect
            );
        }
    }
}
