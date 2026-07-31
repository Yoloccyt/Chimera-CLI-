//! PolicyOscillationDetector 属性测试 — L9 优化 3.5(强制 proptest 规范)
//!
//! 覆盖抖振检测器的核心不变量:
//! 1. **窗口计数不变量**:记录 N 个 TTG 切换后,窗口内计数 ≤ N(惰性清理只减不增)
//! 2. **严重度有界**:severity ∈ [0.0, 1.0] 恒成立(复合指标 min 封顶)
//! 3. **告警单调性**:severity 超过阈值 ⟺ should_alert(判定与阈值一致)
//! 4. **震荡对计数一致**:oscillation_pairs ≤ oscillation_patterns.len()

use efficiency_monitor::{OscillationConfig, PolicyOscillationDetector};
use event_bus::{EventMetadata, NexusEvent};
use proptest::prelude::*;

/// 构造 TTG 切换事件(from/to 取自小字母表,制造可重复的震荡对)
fn ttg_event(from: &str, to: &str) -> NexusEvent {
    NexusEvent::ThinkingModeSwitched {
        metadata: EventMetadata::new("proptest"),
        quest_id: "q-prop".into(),
        from_mode: from.into(),
        to_mode: to.into(),
        reason: "proptest".into(),
    }
}

proptest! {
    /// 不变量 1+2:窗口计数 ≤ 记录数,severity 恒 ∈ [0.0, 1.0]
    #[test]
    fn prop_window_count_bounded_and_severity_in_range(
        // 用 0..3 的模式索引序列驱动 from/to,长度 0..50
        switches in proptest::collection::vec(0u8..3, 0..50),
    ) {
        let detector = PolicyOscillationDetector::new();
        let modes = ["Fast", "Standard", "Deep"];
        for &s in &switches {
            let from = modes[s as usize];
            let to = modes[(s as usize + 1) % 3];
            detector.record_event(&ttg_event(from, to));
        }
        let report = detector.detect();

        // 窗口内计数不超过记录总数(default window 足够长,无过期清理)
        prop_assert!(report.ttg_switches_in_window <= switches.len());
        // 严重度复合指标恒有界
        prop_assert!((0.0..=1.0).contains(&report.severity),
            "severity {} 越界", report.severity);
    }

    /// 不变量 3:should_alert ⟺ severity > alert_threshold(判定与阈值一致)
    #[test]
    fn prop_should_alert_matches_threshold(
        switches in proptest::collection::vec(0u8..3, 0..60),
    ) {
        let config = OscillationConfig::default();
        let threshold = config.severity_alert_threshold;
        let detector = PolicyOscillationDetector::with_config(config);
        let modes = ["Fast", "Standard", "Deep"];
        for &s in &switches {
            detector.record_event(&ttg_event(
                modes[s as usize],
                modes[(s as usize + 1) % 3],
            ));
        }
        let report = detector.detect();
        // should_alert 与 severity>threshold 判定必须一致
        prop_assert_eq!(report.should_alert, report.severity > threshold);
    }

    /// 不变量 4:oscillation_pairs 与 patterns 列表长度一致
    #[test]
    fn prop_oscillation_pairs_consistent_with_patterns(
        switches in proptest::collection::vec(0u8..3, 0..60),
    ) {
        let detector = PolicyOscillationDetector::new();
        let modes = ["Fast", "Standard", "Deep"];
        for &s in &switches {
            detector.record_event(&ttg_event(
                modes[s as usize],
                modes[(s as usize + 1) % 3],
            ));
        }
        let report = detector.detect();
        // oscillation_pairs 是达阈值的震荡对数,等于 patterns 列表长度
        prop_assert_eq!(report.oscillation_pairs, report.oscillation_patterns.len());
    }
}
