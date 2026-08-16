//! components::prefix_hint — 前缀键瞬显提示状态机(Concord W11 T11.4,ADR-083)
//!
//! 对应架构层:L10 Interface
//!
//! # 职责
//! g / Ctrl+W 前缀按下后若 300ms 内无后续键,提示可用的后缀键位
//! (which-key 范式),解决前缀键可发现性短板(方案 §5.4/§6.4);
//! 任意后续键到达即取消提示。纯状态机(时钟注入),事件循环负责
//! 定时检查与弹窗呈现。

use std::time::{Duration, Instant};

/// 瞬显触发延迟(方案 §5.4:300ms 无后续键)
pub const PREFIX_HINT_DELAY: Duration = Duration::from_millis(300);

/// 前缀键类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixKind {
    /// `g` 前缀(滚动/跳转族)
    G,
    /// `Ctrl+W` 前缀(窗格导航族)
    W,
}

/// 前缀瞬显状态机
#[derive(Debug, Default)]
pub struct PrefixHintState {
    /// 已武装的前缀与武装时刻(None = 未武装)
    pending: Option<(PrefixKind, Instant)>,
}

impl PrefixHintState {
    /// 创建空状态机
    pub fn new() -> Self {
        Self::default()
    }

    /// 前缀键按下:武装计时器
    pub fn arm(&mut self, kind: PrefixKind, now: Instant) {
        self.pending = Some((kind, now));
    }

    /// 后续键到达或模式退出:解除武装
    pub fn disarm(&mut self) {
        self.pending = None;
    }

    /// 是否到期(≥300ms 无后续键);到期返回前缀类别(调用方呈现后应 disarm)
    pub fn due(&self, now: Instant) -> Option<PrefixKind> {
        self.pending
            .filter(|(_, armed_at)| now.duration_since(*armed_at) >= PREFIX_HINT_DELAY)
            .map(|(kind, _)| kind)
    }

    /// 当前武装状态(测试/诊断)
    pub fn pending(&self) -> Option<PrefixKind> {
        self.pending.map(|(k, _)| k)
    }
}

/// 前缀键提示条目(键名 → i18n 键,呈现时经 tr() 解析;纯函数)
///
/// 条目与 InputRouter 路由表同源一致(route_gprefix/route_wprefix);
/// 说明文案走 i18n 收口(i18n_hardcode_invariant 防退化)。
pub fn prefix_hint_entries(kind: PrefixKind) -> Vec<(&'static str, &'static str)> {
    match kind {
        PrefixKind::G => vec![("g", "prefix.hint.g_top"), ("1-6", "prefix.hint.g_jump")],
        PrefixKind::W => vec![
            ("h/l", "prefix.hint.w_hl"),
            ("j/k", "prefix.hint.w_jk"),
            ("w", "prefix.hint.w_cycle"),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_and_due_after_delay() {
        let mut s = PrefixHintState::new();
        let t0 = Instant::now();
        assert_eq!(s.due(t0), None, "未武装不触发");
        s.arm(PrefixKind::G, t0);
        assert_eq!(
            s.due(t0 + Duration::from_millis(299)),
            None,
            "未到 300ms 不触发"
        );
        assert_eq!(
            s.due(t0 + Duration::from_millis(300)),
            Some(PrefixKind::G),
            "恰 300ms 触发"
        );
    }

    #[test]
    fn disarm_cancels_hint() {
        let mut s = PrefixHintState::new();
        let t0 = Instant::now();
        s.arm(PrefixKind::W, t0);
        s.disarm();
        assert_eq!(s.due(t0 + Duration::from_secs(1)), None, "解除后不触发");
        assert_eq!(s.pending(), None);
    }

    #[test]
    fn rearm_replaces_kind_and_time() {
        let mut s = PrefixHintState::new();
        let t0 = Instant::now();
        s.arm(PrefixKind::G, t0);
        s.arm(PrefixKind::W, t0 + Duration::from_millis(200));
        assert_eq!(s.pending(), Some(PrefixKind::W), "重武装覆盖");
        assert_eq!(
            s.due(t0 + Duration::from_millis(400)),
            None,
            "从最后一次武装起算未到 300ms"
        );
        assert_eq!(s.due(t0 + Duration::from_millis(500)), Some(PrefixKind::W));
    }

    #[test]
    fn hint_entries_cover_both_prefixes() {
        let _guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        assert!(!prefix_hint_entries(PrefixKind::G).is_empty());
        assert!(!prefix_hint_entries(PrefixKind::W).is_empty());
        // 说明键均能在 zh 表解析出非键本身的文案(i18n 收口验证)
        for (_, key) in prefix_hint_entries(PrefixKind::G) {
            let desc = crate::i18n::tr(key);
            assert_ne!(desc, key, "说明键应已翻译: {key}");
        }
    }
}
