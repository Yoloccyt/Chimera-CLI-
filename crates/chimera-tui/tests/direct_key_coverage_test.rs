//! PS-2 I-6:面板直达键覆盖不变量
//!
//! # 背景(评估报告 I-6)
//! 26 个注册面板中曾有 11 个无直达键:用户只能 Tab 循环(最坏 25 次)或
//! 背下 `/panel <name>` 命令名。PS-3 补 `g7-g0`(4 个业务面板),
//! PS-2 本批补 `F4/F5/F9-F12`(6 个),至此**全部注册面板皆有直达键**
//! (Chat 例外,经 `\` 互切视图模式可达)。
//!
//! # 本文件守护两条契约
//! 1. **静态覆盖**:`router.rs` 中所有 `PanelJump(PanelId::X)` 目标 ∪ 显式豁免
//!    必须**恰好等于** `PanelId::REGISTERED_FOCUS_ORDER` —— 新增面板若不分配
//!    直达键、也不登记豁免,本测试即红;
//! 2. **运行期绑定**:逐个按键实调 `InputRouter::route`,断言路由结果确为
//!    预期面板的 `PanelJump` —— 证明绑定真实存在且指向正确(而非"源码里有字符串")。

#![forbid(unsafe_code)]

use chimera_tui::input::router::{InputRouter, RouteTarget, RouterMode};
use chimera_tui::PanelId;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 无直达键但**已登记豁免**的面板:(面板名, 可达方式说明)
///
/// 豁免必须给出"另一种真实可达路径",不得仅为让测试通过而登记。
/// 若某面板的豁免被撤销(即它后来分配了直达键),静态覆盖检查会因
/// "豁免项已出现在路由表中"而失败 —— 强制同步清理,避免豁免腐烂。
const EXEMPTED_PANELS: &[(&str, &str)] =
    &[("Chat", "经 `\\` 互切视图模式可达(ToggleViewMode 路由)")];

/// 读取 `src/input/router.rs` 的全部 `PanelJump(PanelId::X)` 目标名
fn router_panel_jump_targets() -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("input")
        .join("router.rs");
    let src = std::fs::read_to_string(&path).expect("router.rs should be readable");
    let mut names = Vec::new();
    let needle = "PanelJump(PanelId::";
    let mut rest = src.as_str();
    while let Some(i) = rest.find(needle) {
        let after = &rest[i + needle.len()..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        names.push(after[..end].to_string());
        rest = &after[end..];
    }
    names.sort();
    names.dedup();
    names
}

/// 面板名(PanelId 变体的 Debug 名)全集
fn registered_panel_names() -> Vec<String> {
    let mut names: Vec<String> = PanelId::REGISTERED_FOCUS_ORDER
        .iter()
        .map(|p| format!("{p:?}"))
        .collect();
    names.sort();
    names.dedup();
    names
}

#[test]
fn every_registered_panel_has_a_direct_key_or_documented_exemption() {
    let routed = router_panel_jump_targets();
    let registered = registered_panel_names();
    let exempted: Vec<&str> = EXEMPTED_PANELS.iter().map(|(n, _)| *n).collect();

    // 1) 每个注册面板都必须"有直达键 或 已登记豁免"
    let uncovered: Vec<&String> = registered
        .iter()
        .filter(|n| !routed.contains(n) && !exempted.contains(&n.as_str()))
        .collect();
    assert!(
        uncovered.is_empty(),
        "以下注册面板既无直达键、也未登记豁免(用户只能靠 Tab 循环 25 次或背命令名):\n  {uncovered:?}\n\
         修法二选一:① 在 src/input/router.rs 为其分配直达键;② 在 EXEMPTED_PANELS 登记\
         并写明另一种真实可达路径。"
    );

    // 2) 路由表不得指向未注册面板(防拼写错误/僵尸绑定)
    let phantom: Vec<&String> = routed.iter().filter(|n| !registered.contains(n)).collect();
    assert!(
        phantom.is_empty(),
        "路由表指向了未注册的面板(拼写错误或面板已下线?): {phantom:?}"
    );

    // 3) 豁免不得腐烂:已登记豁免的面板不得同时出现在路由表中
    let stale: Vec<&&str> = exempted
        .iter()
        .filter(|n| routed.contains(&n.to_string()))
        .collect();
    assert!(
        stale.is_empty(),
        "以下面板已登记豁免,却已存在直达键 —— 请从 EXEMPTED_PANELS 移除: {stale:?}"
    );
}

/// 逐键实调路由,断言绑定真实且指向正确
fn assert_routes_to(key: KeyEvent, mode: RouterMode, expected: PanelId) {
    let target = InputRouter::route(mode, key);
    assert_eq!(
        target,
        RouteTarget::PanelJump(expected),
        "按键 {key:?}(模式 {mode:?})应直达 {expected:?},实际 {target:?}"
    );
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn digit_keys_jump_to_first_nine_panels() {
    let cases = [
        ('1', PanelId::Quest),
        ('2', PanelId::Parliament),
        ('3', PanelId::Budget),
        ('4', PanelId::Memory),
        ('5', PanelId::Security),
        ('6', PanelId::Health),
        ('7', PanelId::Log),
        ('8', PanelId::Help),
        ('9', PanelId::Decay),
    ];
    for (ch, panel) in cases {
        assert_routes_to(key(KeyCode::Char(ch)), RouterMode::Normal, panel);
    }
}

#[test]
fn f_keys_cover_twelve_panels() {
    // PS-2 I-6 补齐:F4/F5/F9-F12 此前为空闲(回退焦点面板)
    let cases = [
        (1u8, PanelId::Quest),
        (2, PanelId::Parliament),
        (3, PanelId::Budget),
        (4, PanelId::OsaSparse),
        (5, PanelId::ClvVector),
        (6, PanelId::Memory),
        (7, PanelId::Security),
        (8, PanelId::Health),
        (9, PanelId::MetricsDashboard),
        (10, PanelId::Sysinfo),
        (11, PanelId::OverWindow),
        (12, PanelId::ExperienceCardViz),
    ];
    for (n, panel) in cases {
        assert_routes_to(key(KeyCode::F(n)), RouterMode::Normal, panel);
    }
}

#[test]
fn g_prefix_keys_cover_ten_panels() {
    let cases = [
        ('1', PanelId::EventStream),
        ('2', PanelId::Router),
        ('3', PanelId::McpNodes),
        ('4', PanelId::Chtc),
        ('5', PanelId::Timeline),
        ('6', PanelId::ResourceMonitor),
        ('7', PanelId::SelfAssessment),
        ('8', PanelId::DagViz),
        ('9', PanelId::PvlScore),
        ('0', PanelId::TaskManager),
    ];
    for (ch, panel) in cases {
        assert_routes_to(key(KeyCode::Char(ch)), RouterMode::GPrefix, panel);
    }
}

#[test]
fn chat_exemption_is_a_real_reachability_path() {
    // 豁免必须"真实可达":`\`(反斜杠)触发视图互切 → Chat 视图。
    // 它不是 PanelJump 路由(故计入豁免而非直达键),但必须真实存在 ——
    // 否则豁免就是空头承诺。
    let target = InputRouter::route(RouterMode::Normal, key(KeyCode::Char('\\')));
    assert_eq!(
        target,
        RouteTarget::ToggleViewMode,
        "Chat 的豁免依据是 `\\` 视图互切,该绑定必须真实存在"
    );
}
