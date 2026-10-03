//! 快捷键声明-可达性对账 — B5-K(R5 F-24/F-27/F-41 死承诺防复发)
//!
//! 对应架构层:L10 Interface
//!
//! # 背景(R5 §死键汇总 + 批次四 §6.1 #6/§7 反向锚定缺口)
//! 批次四(B4-H)移除 8 条死声明——F-41 六面板 "R=刷新"、F-27 timeline
//! "←/→"、F-24 task_manager "Esc 清选"——均为删除型修复且无测试锁定,
//! 未来重添死声明不会红。本文件补对账型守护:`shortcuts()` 经 help_context
//! 广播到 Help overlay 的每个键帽,必须真实可达(或登记豁免)。
//!
//! # 与 direct_key_coverage_test 的分工
//! 该文件管"面板必有直达键"(路由表覆盖焦点环);本文件管"shortcuts()
//! 声明的键真实可达"(声明-路由一致性,方向相反)。先例:
//! budget_panel_test.rs `shortcuts_declare_refresh_only`(单面板版对账)。
//!
//! # 判定口径(仅对单字符键帽机械验证)
//! 键帽可达 = 以下任一证据在**生产段**(`#[cfg(test)]` 之前)存在:
//! 1. 面板自身源码有对应按键臂 — `Char('x')` 字面同时覆盖 handle_key
//!    match 臂(budget.rs:172)与 task_manager 式 KeyRule 表条目
//!    (MAIN_KEY_TABLE 的 `KeyPattern::Code(KeyCode::Char('R'))`);
//! 2. `src/input/router.rs` 全局路由有直达(Char('G') => ScrollBottom、
//!    Char('/') => EnterSlash、Char('i') => EnterMode(Insert) 等);
//! 3. `src/actions/domains/*.rs` codegen 键位表绑定(default_key/alias_keys,
//!    如 system.show_help 的 `Some("?")`)。
//!
//! WHY 源码文本扫描而非行为实调:handle_key 返回 None 不代表未消费
//! (sysinfo 刷新臂返回 None),Esc/q 走全局路由不进面板,行为判定假阴率高;
//! 文本扫描与 direct_key_coverage_test 的 router_panel_jump_targets() 同款。
//!
//! 组合键/方向键/多键序列("g g"/"↑/↓"/"Ctrl+A")不可机械验证,逐条
//! 登记于 `NON_MECHANICAL_KEYCAPS`;豁免**双向对账只减不增**——孤儿豁免与
//! 未登记的新复合键帽都会红,强制同步清理(同 direct_key_coverage_test
//! EXEMPTED_PANELS 的防腐烂设计)。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom
#![forbid(unsafe_code)]

use std::path::PathBuf;

use chimera_tui::panels::{
    BenchPanel, BlackboardPanel, BudgetPanel, ChatPanel, ChtcPanel, ClvVectorPanel, DagVizPanel,
    DecayPanel, EventStreamPanel, ExperienceCardVizPanel, HealthPanel, HelpPanel,
    InjectionStrategyPanel, LogPanel, McpNodesPanel, MemoryPanel, MetricsDashboardPanel,
    OsaSparsePanel, OverWindowPanel, Panel, ParliamentPanel, PvlScorePanel, QuestPanel,
    ResourceMonitorPanel, ReviewQueuePanel, RouterPanel, SecurityPanel, SelfAssessmentPanel,
    SysinfoPanel, TaskManagerPanel, TimelinePanel,
};
use chimera_tui::PanelId;

/// 复合键帽豁免清单:(面板 Debug 名, 键帽, 不可机械验证的理由)
///
/// 豁免 ≠ 判定不可达,仅表示"该键帽形态无法用 `Char('x')` 文本口径机械
/// 对账",其可达性由行为级测试(direct_key_coverage/global_keys 等)承担。
/// 只减不增:键帽删除后条目成孤儿(`exemption_list_has_no_orphans` 红);
/// 新增复合键帽必须在此逐条登记(`declared_keycaps_are_reachable_or_exempted` 红)。
const NON_MECHANICAL_KEYCAPS: &[(&str, &str, &str)] = &[
    // 方向符号键帽(↑/↓/←/→ 非单字符;导航臂为 KeyCode::Up/Down/Left/Right)
    ("Quest", "↑/↓", "方向键导航(list_state 导航臂)"),
    ("Parliament", "↑/↓", "方向键导航"),
    ("Security", "↑/↓", "方向键导航"),
    ("Log", "↑/↓", "方向键导航"),
    ("EventStream", "↑/↓", "方向键导航"),
    ("McpNodes", "↑/↓", "方向键导航"),
    ("Chtc", "↑/↓", "方向键导航"),
    ("Timeline", "↑/↓", "方向键导航"),
    ("MetricsDashboard", "↑/↓/←/→", "方向符号复合键帽(含四向)"),
    ("PvlScore", "↑/↓", "方向键导航(维度选择)"),
    ("TaskManager", "↑/↓", "方向键导航"),
    ("Chat", "↑/↓ j/k", "方向与 vim 键混合键帽"),
    // 键位对
    ("Quest", "PgUp/PgDn", "翻页键位对"),
    ("Log", "PgUp/PgDn", "翻页键位对"),
    ("EventStream", "PgUp/PgDn", "翻页键位对"),
    // 多字符功能键
    ("Quest", "Enter", "功能键(跳转事件流,路由表 Enter 分支)"),
    ("Parliament", "Enter", "功能键(事件详情弹窗)"),
    ("Security", "Enter", "功能键(事件详情)"),
    ("EventStream", "Enter", "功能键(事件详情)"),
    ("Chat", "Enter", "功能键(发送消息)"),
    ("Chat", "Esc", "功能键(退出输入模式)"),
    ("TaskManager", "Enter", "功能键(打开详情)"),
    ("TaskManager", "Space", "功能键(多选切换)"),
    (
        "TaskManager",
        "Ctrl+A",
        "组合键(全选,KeyPattern::Ctrl 形态)",
    ),
    ("SelfAssessment", "Tab", "功能键(面板切换,全局路由)"),
    ("DagViz", "Tab", "功能键(面板切换,全局路由)"),
    ("OverWindow", "Tab", "功能键(面板切换,全局路由)"),
    ("ExperienceCardViz", "Tab", "功能键(面板切换,全局路由)"),
    ("Bench", "Tab", "功能键(面板切换,全局路由)"),
    ("ReviewQueue", "Tab", "功能键(面板切换,全局路由)"),
    ("Blackboard", "Tab", "功能键(面板切换,全局路由)"),
    // 两键序列(前缀 g 进 GPrefix 模式后接第二键,任务书点名豁免形态)
    ("Quest", "g g", "两键序列(g 前缀 + g)"),
    ("Log", "g g", "两键序列(g 前缀 + g)"),
    ("EventStream", "g g", "两键序列(g 前缀 + g)"),
    ("Chat", "g g / G", "两键序列与方向键混合键帽"),
    // 斜杠分隔 vim 键复合键帽(单键臂存在但键帽形态非单字符)
    (
        "MetricsDashboard",
        "h/j/k",
        "斜杠复合键帽(h/j/k 臂在面板内)",
    ),
    ("PvlScore", "j/k", "斜杠复合键帽(j/k 臂在面板内)"),
    // 符号对
    (
        "TaskManager",
        "+/-",
        "符号对(优先级增减,Char('+')/Char('-') 臂)",
    ),
];

/// 批次四(B4-H)移除的死声明 — 重添即红
///
/// (面板 Debug 名, 键帽):六面板 "R" 无 Char('r') 臂亦无键位表绑定(R5
/// F-41;budget/decay/health/memory/router/sysinfo/task_manager 的 R 为
/// 真实实现,不在本清单);timeline "←/→" 无 Left/Right 分支(F-27);
/// task_manager "Esc" 生产路径被全局 quit 截获(F-24,'g+Esc' 隐藏路径
/// 保留但不声明)。重添前须先恢复可达性并同步更新本清单。
const REMOVED_DEAD_DECLARATIONS: &[(&str, &str)] = &[
    ("McpNodes", "R"),
    ("Chtc", "R"),
    ("OsaSparse", "R"),
    ("ClvVector", "R"),
    ("ResourceMonitor", "R"),
    ("MetricsDashboard", "R"),
    ("Timeline", "←/→"),
    ("TaskManager", "Esc"),
];

/// 构造面板实例并返回其源文件相对路径(判定域 1 的扫描对象)
///
/// WHY 测试内复刻 app::make_panel 分支而非导出生产工厂:工厂为 app 模块
/// 私有,导出属 API 面扩大;match 穷尽全部 PanelId 变体(含不在焦点环的
/// InjectionStrategy),新增变体时本文件编译失败即强制同步(与
/// make_panel 的 INV-F 守护同构)。
fn panel_with_source(id: PanelId) -> (Box<dyn Panel>, &'static str) {
    match id {
        PanelId::Quest => (Box::new(QuestPanel::new()), "src/panels/quest.rs"),
        PanelId::Parliament => (Box::new(ParliamentPanel::new()), "src/panels/parliament.rs"),
        PanelId::Budget => (Box::new(BudgetPanel::new()), "src/panels/budget.rs"),
        PanelId::Memory => (Box::new(MemoryPanel::new()), "src/panels/memory.rs"),
        PanelId::Security => (Box::new(SecurityPanel::new()), "src/panels/security.rs"),
        PanelId::Health => (Box::new(HealthPanel::new()), "src/panels/health.rs"),
        PanelId::Log => (Box::new(LogPanel::new()), "src/panels/log.rs"),
        PanelId::Help => (Box::new(HelpPanel::new()), "src/panels/help.rs"),
        PanelId::Decay => (Box::new(DecayPanel::new()), "src/panels/decay.rs"),
        PanelId::EventStream => (
            Box::new(EventStreamPanel::new()),
            "src/panels/event_stream.rs",
        ),
        PanelId::Router => (Box::new(RouterPanel::new()), "src/panels/router.rs"),
        PanelId::McpNodes => (Box::new(McpNodesPanel::new()), "src/panels/mcp_nodes.rs"),
        PanelId::Chtc => (Box::new(ChtcPanel::new()), "src/panels/chtc.rs"),
        PanelId::Timeline => (Box::new(TimelinePanel::new()), "src/panels/timeline.rs"),
        PanelId::OsaSparse => (Box::new(OsaSparsePanel::new()), "src/panels/osa_sparse.rs"),
        PanelId::ClvVector => (Box::new(ClvVectorPanel::new()), "src/panels/clv_vector.rs"),
        PanelId::ResourceMonitor => (
            Box::new(ResourceMonitorPanel::new()),
            "src/panels/resource_monitor.rs",
        ),
        PanelId::MetricsDashboard => (
            Box::new(MetricsDashboardPanel::new()),
            "src/panels/metrics_dashboard.rs",
        ),
        PanelId::Sysinfo => (Box::new(SysinfoPanel::new()), "src/panels/sysinfo.rs"),
        PanelId::Chat => (Box::new(ChatPanel::new()), "src/panels/chat.rs"),
        PanelId::SelfAssessment => (
            Box::new(SelfAssessmentPanel::new()),
            "src/panels/self_assessment.rs",
        ),
        PanelId::DagViz => (Box::new(DagVizPanel::new()), "src/panels/dag_viz.rs"),
        PanelId::PvlScore => (Box::new(PvlScorePanel::new()), "src/panels/pvl_score.rs"),
        PanelId::TaskManager => (
            Box::new(TaskManagerPanel::new()),
            "src/panels/task_manager.rs",
        ),
        PanelId::OverWindow => (Box::new(OverWindowPanel::new()), "src/panels/overwindow.rs"),
        PanelId::ExperienceCardViz => (
            Box::new(ExperienceCardVizPanel::new()),
            "src/panels/experience_card_viz.rs",
        ),
        PanelId::InjectionStrategy => (
            Box::new(InjectionStrategyPanel::new()),
            "src/panels/injection_strategy.rs",
        ),
        PanelId::Bench => (Box::new(BenchPanel::new()), "src/panels/bench.rs"),
        PanelId::ReviewQueue => (
            Box::new(ReviewQueuePanel::new()),
            "src/panels/review_queue.rs",
        ),
        PanelId::Blackboard => (Box::new(BlackboardPanel::new()), "src/panels/blackboard.rs"),
    }
}

/// 测试模块起点判定(与 i18n_hardcode_invariant_test::marks_test_module 同款)
fn marks_test_module(raw: &str) -> bool {
    raw.contains("#[cfg(test)]") || raw.contains("#[cfg(all(test")
}

/// 剥行尾/行首注释(首个 `//` 起截断;`///` 文档注释整行归空)
fn strip_line_comment(line: &str) -> &str {
    match line.find("//") {
        Some(idx) => &line[..idx],
        None => line,
    }
}

/// 读取文件生产段(`#[cfg(test)]` 起截断)
///
/// WHY 剥测试段:内嵌测试断言中的 Char('x') 引用(KeyEvent::new 构造)不代表
/// 生产可达,剥离后对账只看生产代码。
///
/// WHY 判定前先剥注释:含 "#[cfg(test)]" 字样的**文档注释行**(如
/// pvl_score.rs:57 "/// 现阶段仅被 `#[cfg(test)]` 使用…")不是测试段起点,
/// 不剥会从该行起整文件误判为测试段。
fn production_source(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("源文件应可读 {rel}: {e}"));
    let mut prod = String::new();
    for raw in content.lines() {
        if marks_test_module(strip_line_comment(raw)) {
            break;
        }
        prod.push_str(raw);
        prod.push('\n');
    }
    prod
}

/// 单字符键帽可达性判定(三判定域,见模块 doc)
///
/// 大小写敏感按设计:键帽 "R" 只认 `Char('R')`(F-41 的键帽错位教训——
/// sysinfo 修复前只认小写 'r' 而键帽大写 R,敏感口径可防错位复发)。
fn single_char_keycap_reachable(panel_src: &str, keycap: char) -> bool {
    let arm = format!("Char('{keycap}')");
    // 域 1:面板自身(handle_key 臂 / KeyRule 表条目)
    if panel_src.contains(&arm) {
        return true;
    }
    // 域 2:全局路由表直达
    if production_source("src/input/router.rs").contains(&arm) {
        return true;
    }
    // 域 3:actions 键位表(default_key/alias_keys 的键帽字符串字面)
    let actions_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/actions/domains");
    let needle = format!("\"{keycap}\"");
    let entries = match std::fs::read_dir(&actions_dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            // domains 文件为键位注册表,单字符字符串字面仅键位使用,
            // 粗粒度全文匹配即可(与域 1/2 的生产段口径差异已注释)
            if text.contains(&needle) {
                return true;
            }
        }
    }
    false
}

fn is_exempted(panel: &str, keycap: &str) -> bool {
    NON_MECHANICAL_KEYCAPS
        .iter()
        .any(|(p, k, _)| *p == panel && *k == keycap)
}

/// 主对账:焦点环全部面板的 shortcuts() 键帽必须可达(单字符)或已登记豁免
#[test]
fn declared_keycaps_are_reachable_or_exempted() {
    let mut violations: Vec<String> = Vec::new();
    for id in PanelId::REGISTERED_FOCUS_ORDER {
        let (panel, src_rel) = panel_with_source(*id);
        let panel_src = production_source(src_rel);
        let name = format!("{id:?}");
        for (keycap, _) in panel.shortcuts() {
            let mut chars = keycap.chars();
            let is_single_char = chars.next().is_some() && chars.next().is_none();
            if is_single_char {
                let c = keycap.chars().next().unwrap();
                if !single_char_keycap_reachable(&panel_src, c) {
                    violations.push(format!(
                        "{name}: shortcuts 声明键帽 {keycap:?} 无可达证据 \
                         (面板按键臂/路由表/actions 键位表三域均未命中)—— \
                         死承诺,先恢复可达性再声明(R5 F-24/F-27/F-41 口径)"
                    ));
                }
            } else if !is_exempted(&name, keycap) {
                violations.push(format!(
                    "{name}: 复合键帽 {keycap:?} 未登记 NON_MECHANICAL_KEYCAPS \
                     (豁免清单只减不增,新增复合键帽须逐条登记理由)"
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "shortcuts 声明-可达性对账失败:\n{}",
        violations.join("\n")
    );
}

/// 豁免清单孤儿对账:每条豁免必须对应焦点环面板当前实际声明的键帽
///
/// WHY 反向断言:键帽删除/改名后豁免条目若不清理即成"永久放水口"——
/// 本测试强制豁免清单与声明面同步收缩(只减不增的另一半)。
#[test]
fn exemption_list_has_no_orphans() {
    let mut declared: Vec<(String, String)> = Vec::new();
    for id in PanelId::REGISTERED_FOCUS_ORDER {
        let (panel, _) = panel_with_source(*id);
        let name = format!("{id:?}");
        for (keycap, _) in panel.shortcuts() {
            declared.push((name.clone(), keycap.to_string()));
        }
    }
    let mut orphans: Vec<String> = NON_MECHANICAL_KEYCAPS
        .iter()
        .filter(|(p, k, _)| !declared.iter().any(|(dp, dk)| dp == p && dk == k))
        .map(|(p, k, _)| {
            format!("{p}: 豁免键帽 {k:?} 已不在该面板 shortcuts() 声明中(孤儿豁免,请删除条目)")
        })
        .collect();
    orphans.sort();
    assert!(
        orphans.is_empty(),
        "豁免清单存在孤儿条目(只减不增棘轮):\n{}",
        orphans.join("\n")
    );
}

/// 死声明禁复活:批次四(B4-H)移除的 8 条死声明重添即红
#[test]
fn removed_dead_declarations_do_not_revive() {
    let mut revived: Vec<String> = Vec::new();
    for id in PanelId::REGISTERED_FOCUS_ORDER {
        let (panel, _) = panel_with_source(*id);
        let name = format!("{id:?}");
        for (keycap, _) in panel.shortcuts() {
            for (dead_panel, dead_keycap) in REMOVED_DEAD_DECLARATIONS {
                if &name == dead_panel && keycap == *dead_keycap {
                    revived.push(format!(
                        "{name}: 死声明 {keycap:?} 复活(B4-H 已移除;重添前须先 \
                         恢复可达性并按 declared_keycaps_are_reachable_or_exempted 对账)"
                    ));
                }
            }
        }
    }
    assert!(
        revived.is_empty(),
        "已移除的死声明复活:\n{}",
        revived.join("\n")
    );
}
