//! P4.3 可调 tick 暴露集成测试
//!
//! 验证 TuiConfig.tick_interval_ms 正确桥接到 DataSourceConfig

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
use chimera_tui::config::TuiConfig;
use chimera_tui::data::DataSourceConfig;

/// 验证 TuiConfig.tick_interval_ms 默认值
#[test]
fn tui_config_default_tick_interval() {
    let config = TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    };
    assert_eq!(config.tick_interval_ms, 250);
}

/// 验证 TuiConfig 校验 tick_interval_ms 范围
#[test]
fn tui_config_validate_tick_interval_range() {
    // 正常值通过
    for ms in [100u16, 200, 500, 1000] {
        let config = TuiConfig {
            tick_interval_ms: ms,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    // 过低
    let config = TuiConfig {
        tick_interval_ms: 50,
        ..Default::default()
    };
    assert!(config.validate().is_err());

    // 过高
    let config = TuiConfig {
        tick_interval_ms: 2000,
        ..Default::default()
    };
    assert!(config.validate().is_err());
}

/// 验证 DataSourceConfig::from_tui_config 桥接
#[test]
fn data_source_config_from_tui_config() {
    let tui_config = TuiConfig {
        tick_interval_ms: 500,
        ..Default::default()
    };
    let ds_config = DataSourceConfig::from_tui_config(&tui_config);
    assert_eq!(ds_config.tick_interval_ms, 500);
}

/// 验证不同 tick_interval_ms 值的桥接
#[test]
fn data_source_config_from_tui_config_various() {
    for ms in [100u16, 200, 500, 1000] {
        let tui_config = TuiConfig {
            tick_interval_ms: ms,
            ..Default::default()
        };
        assert!(tui_config.validate().is_ok());
        let ds_config = DataSourceConfig::from_tui_config(&tui_config);
        assert_eq!(ds_config.tick_interval_ms, ms as u64);
    }
}

/// 验证 TuiApp 构造时 tick_interval_ms 从 TuiConfig 传入
#[test]
fn tui_app_uses_configured_tick_interval() {
    let config = TuiConfig {
        tick_interval_ms: 500,
        ..Default::default()
    };
    let app = chimera_tui::TuiApp::new(config).unwrap();
    assert_eq!(app.config().tick_interval_ms, 500);
}

// ---------------------------------------------------------------------------
// Eco 切换规则的确定性夹具（ADR-222 R8-7）
//
// WHY 直接驱动纯函数而不是跑周期：切换规则此前零测试覆盖，而任何“起 pipeline
// 再 sleep”的写法都会重新变成竞速测试（同型教训：data_test 的 1000 事件/秒）。
// `eco_transition` 是抽取出的唯一真源，循环只负责把结果映射到 tick 时长。
// ---------------------------------------------------------------------------
use chimera_tui::data::pipeline::eco_transition;
use chimera_tui::types::TickMode;

const THRESHOLD: usize = 100;

/// Normal 态的边界：恰好等于阈值即进入 Eco（判据是 `>=`，不是 `>`）。
#[test]
fn eco_enters_on_threshold_boundary() {
    assert_eq!(
        eco_transition(TickMode::Normal, 0, THRESHOLD - 1, THRESHOLD),
        (TickMode::Normal, 0),
        "阈值下方一个事件不得切 Eco"
    );
    assert_eq!(
        eco_transition(TickMode::Normal, 0, THRESHOLD, THRESHOLD),
        (TickMode::Eco, 5),
        "恰好达到阈值必须切 Eco，并重置 5 次倒计时"
    );
    assert_eq!(
        eco_transition(TickMode::Normal, 0, 10_000, THRESHOLD),
        (TickMode::Eco, 5),
        "远高于阈值同样切 Eco"
    );
}

/// Eco 态：连续 5 次低 backlog 才回 Normal，第 4 次仍在 Eco（防“早一次”回归）。
#[test]
fn eco_leaves_only_after_five_consecutive_low_ticks() {
    let mut state = (TickMode::Eco, 5u32);
    let expected_countdown = [4u32, 3, 2, 1];
    for expected in expected_countdown {
        state = eco_transition(state.0, state.1, 0, THRESHOLD);
        assert_eq!(
            state,
            (TickMode::Eco, expected),
            "第 {} 次低 backlog 仍应处于 Eco 且倒计时递减",
            5 - expected
        );
    }
    assert_eq!(
        eco_transition(state.0, state.1, 0, THRESHOLD),
        (TickMode::Normal, 0),
        "第 5 次低 backlog 才回到 Normal"
    );
}

/// 打断降级：Eco 中一旦重新越阈，倒计时重置为 5（不退出、不累计）。
#[test]
fn eco_high_backlog_resets_countdown() {
    let (mode, countdown) = eco_transition(TickMode::Eco, 2, THRESHOLD, THRESHOLD);
    assert_eq!((mode, countdown), (TickMode::Eco, 5));

    // 重置后仍需要完整 5 次低 backlog 才能退出（倒计时不继承旧值）。
    let mut state = (mode, countdown);
    for _ in 0..4 {
        state = eco_transition(state.0, state.1, 0, THRESHOLD);
        assert_eq!(state.0, TickMode::Eco);
    }
    assert_eq!(
        eco_transition(state.0, state.1, 0, THRESHOLD).0,
        TickMode::Normal
    );
}

/// Normal 态的低 backlog 不得动倒计时（它只在 Eco 期间有意义）。
#[test]
fn normal_low_backlog_keeps_countdown_untouched() {
    assert_eq!(
        eco_transition(TickMode::Normal, 3, 0, THRESHOLD),
        (TickMode::Normal, 3)
    );
}

/// 阈值为 0 的边界：`>= 0` 恒成立 ⇒ 永远进入 Eco（把 `>=` 语义钉住）。
#[test]
fn zero_threshold_always_enters_eco() {
    assert_eq!(
        eco_transition(TickMode::Normal, 0, 0, 0),
        (TickMode::Eco, 5)
    );
}

/// 纯函数：同输入同输出，与调用次数无关（夹具可重复叠加而无需重置全局状态）。
#[test]
fn transition_is_pure_and_repeatable() {
    let first = eco_transition(TickMode::Eco, 5, 7, THRESHOLD);
    for _ in 0..100 {
        assert_eq!(eco_transition(TickMode::Eco, 5, 7, THRESHOLD), first);
    }
}
