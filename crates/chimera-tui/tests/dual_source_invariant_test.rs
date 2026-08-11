//! 双源一致性不变量测试 — Concord 重构 T1.2(R4 缓解,测试先行)
//!
//! 对应架构层:L10 Interface
//!
//! # 背景(P5 双源漂移)
//! 重构方案 §2 P5 诊断出两处"声明式注册只做了一半"的双源结构:
//! - **①焦点双源**:`PanelId::next/prev` 是手写静态表(types.rs),而生产
//!   FocusManager 顺序派生自 `TuiApp` 面板注册序(app/mod.rs)。新增/下线
//!   面板需两处同步,漏改即静默漂移。
//! - **②键位双源**:`ActionDescriptor.default_key` 声明在 domains/*.rs,
//!   而 InputRouter 的 Normal 路由表是硬编码 match(router.rs)。
//!
//! 本文件先于 codegen 化(T1.3/T1.4)落地两条不变量:初始运行允许红
//! (暴露漂移实锤,首测结果记入进度报告);T1.3/T1.4 完成后必须常绿,
//! 此后任何双源漂移都会被 CI 即时捕获。
//!
//! # 不变量定义
//! - **INV-F(焦点环闭合一致)**:从生产焦点环首个面板出发,沿 `PanelId::next()`
//!   静态表遍历一周,得到的序列必须与 FocusManager 注册序**逐项相等**。
//! - **INV-K-A(声明键必被路由)**:每个声明了 `default_key` 且 `global_route`
//!   的动作,InputRouter 在 Normal 模式下必须把该键路由到 `GlobalAction(id)`。
//! - **INV-K-B(路由键必有声明)**:InputRouter Normal 模式路由出的每个
//!   `GlobalAction(id)`,其 id 必须已注册,且其声明的 default_key 必须落在
//!   路由到该 id 的键集合内(未声明 default_key 即漂移)。

use chimera_tui::{
    ActionRegistry, InputRouter, PanelId, RouteTarget, RouterMode, TuiApp, TuiConfig,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

/// 解析 default_key 声明字符串为 KeyEvent(测试内私有,与 T1.3 codegen 解析器对齐)
///
/// 支持形式:`"Ctrl+X"`(Ctrl 组合,字符转小写与路由表一致)、单字符(含
/// `?` `]` `\\`)、命名键(`Enter`/`Tab`/`Esc`)。未识别形式 panic 暴露声明笔误。
fn parse_declared_key(s: &str) -> KeyEvent {
    if let Some(rest) = s.strip_prefix("Ctrl+") {
        let c = rest
            .chars()
            .next()
            .unwrap_or_else(|| panic!("Ctrl+ 后缺字符: {s}"))
            .to_ascii_lowercase();
        return KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    }
    let code = match s {
        "Enter" => KeyCode::Enter,
        "Tab" => KeyCode::Tab,
        "Esc" => KeyCode::Esc,
        one if one.chars().count() == 1 => KeyCode::Char(one.chars().next().unwrap()),
        other => panic!("无法解析 default_key 声明: {other}"),
    };
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// INV-F:焦点环闭合一致 — 静态表遍历序 == 生产 FocusManager 注册序
///
/// 漂移实锤(T1.2 首测预期红):静态环含未注册的 Timeline/Sysinfo,且
/// OsaSparse/ClvVector 相对顺序与注册序相反——新增面板时两处未同步所致。
#[test]
fn inv_f_focus_cycle_matches_production_focus_order() {
    let app = TuiApp::new(TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })
    .expect("TuiApp 构造应成功");
    let order: Vec<PanelId> = app.panel_focus_order().to_vec();
    assert!(!order.is_empty());

    // 沿静态表从注册序首面板遍历一周(上界 64 防死循环,远超面板总数)
    let mut static_cycle = vec![order[0]];
    let mut cur = order[0];
    for _ in 0..64 {
        cur = cur.next();
        if cur == order[0] {
            break;
        }
        static_cycle.push(cur);
    }

    assert_eq!(
        static_cycle, order,
        "INV-F 违反:PanelId::next 静态循环序与 FocusManager 注册序漂移\n\
         静态环:{static_cycle:?}\n注册序:{order:?}",
    );

    // prev 必须是 next 的逆映射(环闭合双向一致)
    for w in static_cycle.windows(2) {
        assert_eq!(
            w[1].prev(),
            w[0],
            "INV-F 违反:prev 与 next 不互逆({:?} -> {:?})",
            w[0],
            w[1]
        );
    }
    assert_eq!(static_cycle[0].prev(), *static_cycle.last().unwrap());
}

/// INV-K-A:声明键必被路由 — default_key(global_route=true)按 Normal 键路由到本动作
#[test]
fn inv_k_a_declared_default_keys_route_to_owning_action() {
    let reg = ActionRegistry::with_builtin_domains();
    for d in reg.all() {
        let Some(key_str) = d.default_key else {
            continue;
        };
        if !d.global_route {
            // 面板内消费的键(如 panel.drill_down 的 Enter)不经全局路由,豁免
            continue;
        }
        let target = InputRouter::route(RouterMode::Normal, parse_declared_key(key_str));
        assert_eq!(
            target,
            RouteTarget::GlobalAction(d.id),
            "INV-K-A 违反:动作 {} 声明 default_key={key_str},但 Normal 模式路由为 {target:?}",
            d.id,
        );
    }
}

/// INV-K-B:路由键必有声明 — Normal 模式路由出的 GlobalAction 必须可回溯到声明
///
/// 探测键空间:小写/大写字母 + Ctrl 字母 + 常用符号键,覆盖 route_normal
/// 全部分支的输入面(纯函数路由,穷举探测成本可忽略)。
#[test]
fn inv_k_b_routed_global_actions_have_declared_keys() {
    let reg = ActionRegistry::with_builtin_domains();

    let mut probe_keys: Vec<KeyEvent> = Vec::new();
    for c in 'a'..='z' {
        probe_keys.push(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        probe_keys.push(KeyEvent::new(
            KeyCode::Char(c.to_ascii_uppercase()),
            KeyModifiers::NONE,
        ));
        probe_keys.push(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }
    for c in ['?', ']', '\\', '/', ':', ';', '\'', ',', '.', '-', '=', '['] {
        probe_keys.push(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    // 归集:action_id -> 路由到它的键描述集合
    let mut routed: HashMap<&'static str, Vec<String>> = HashMap::new();
    for key in probe_keys {
        if let RouteTarget::GlobalAction(id) = InputRouter::route(RouterMode::Normal, key) {
            let desc = format!("{:?}", key.code);
            routed.entry(id).or_default().push(desc);
        }
    }

    for (id, keys) in &routed {
        let Some(d) = reg.get(id) else {
            panic!("INV-K-B 违反:路由器派发了未注册的动作 {id}(键:{keys:?})");
        };
        // 动作必须声明 default_key,且声明键解析后落在路由键集合内
        let Some(key_str) = d.default_key else {
            panic!(
                "INV-K-B 违反:动作 {id} 被键 {keys:?} 全局路由,但未声明 default_key(键位双源漂移)"
            );
        };
        let declared = format!("{:?}", parse_declared_key(key_str).code);
        assert!(
            keys.contains(&declared),
            "INV-K-B 违反:动作 {id} 声明键 {key_str}(解析为 {declared})不在实际路由键集合 {keys:?} 内",
        );
    }
}
