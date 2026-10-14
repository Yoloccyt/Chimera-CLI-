//! i18n 硬编码防退化不变量测试 — Concord W4 T4.4(M4 门禁证据)
//!
//! 对应架构层:L10 Interface
//!
//! # 守护语义
//! panels/、actions/ 与 app/ 的**生产代码段**(`#[cfg(test)]` 之前)不得出现
//! CJK 字面量——用户可见文案一律走 i18n 键表(crate::t!),保证 Ctrl+L
//! 语言切换在面板层真实生效(方案 P6 承诺兑现)。B4-I(F-04)起扫描域
//! 扩展到 src/app(状态栏/弹窗文案迁入键表后的防退化);B5-U(F-17)起
//! 扩展到顶层文件 src/render.rs(FOOTER_TEXT 迁键后的防退化)。
//!
//! # 口径说明
//! - 行尾 `//` 注释先剥离(注释是开发文档媒介,中文为项目约定);
//! - `#[cfg(test)]` / `#[cfg(all(test, ...))]` 之后的测试模块豁免(断言消息
//!   非渲染面);src/app 的 tests.rs / v3_render_tests.rs 整文件由 mod.rs 的
//!   模块级 cfg 门控,非 test 编译不参与构建,经文件级豁免登记(见下);
//! - 新增面板/动作时若确需豁免,经评审后加入 EXEMPT 清单并注明理由。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 串行化涉及全局 locale 的测试(与既有集成测试同范式)
static LOCALE_LOCK: Mutex<()> = Mutex::new(());

fn locale_guard() -> std::sync::MutexGuard<'static, ()> {
    LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// 豁免清单(相对 crate 根的路径前缀;新增需注明理由)
///
/// 2026-09-08 移除 task_manager.rs:批次-A 第 4 步 CJK 豁免棘轮——
/// "数据源未接入"提示已迁入 `panel.task.no_provider` 键表,生产段零 CJK
/// (棘轮只减不增)。
///
/// B4-I(F-04/F-65)src/app 域登记:以下两文件**整文件均为测试模块内容**,
/// 模块级 cfg 标记在 app/mod.rs(`#[cfg(test)] mod tests;` /
/// `#[cfg(all(test, feature = "v3-engine"))] mod v3_render_tests;`),
/// 逐行扫描器看不到 mod.rs 的标记;且两文件在非 test 编译下不参与构建,
/// 全局文件级豁免即既有"测试段豁免"语义的文件级延伸,无生产段放水风险。
const EXEMPT_PREFIXES: &[&str] = &["src/app/tests.rs", "src/app/v3_render_tests.rs"];

/// CJK 行级豁免片段(子串命中即豁免该行;逐条登记理由,只减不增)
///
/// WHY 行级而非文件级:app 域残留 CJK 均为诊断面字符串(错误/断言消息,
/// 非用户可见文案),按消息片段精确豁免可保留该文件其余行的新增检测能力。
const EXEMPT_CJK_SNIPPETS: &[&str] = &[
    // TuiError::Render 内部接线漂移诊断(开发者面,非渲染文案;
    // B4-I 2026-10-02 登记,随诊断消息迁英/删除后应移除)
    "v3_term 未初始化",
    // debug_assert_eq! 消息(仅 debug 构建断言失败时输出,纯调试面)
    "全量帧宽度应与终端尺寸一致",
];

/// CJK 统一表意文字区间判定
fn is_cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

/// 剥离行尾注释(简化口径:首个 `//` 起截断)
///
/// WHY 简化可接受:panels/actions 域无字符串内嵌 `//` 的场景;
/// 若未来出现,改为状态机解析并在此注明。
fn strip_line_comment(line: &str) -> &str {
    match line.find("//") {
        Some(idx) => &line[..idx],
        None => line,
    }
}

/// 枚举目录下全部 .rs 文件(递归)
fn collect_rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// 测试模块开始行判定 — `#[cfg(test)]` 与 `#[cfg(all(test, ...))]` 均视为
/// 测试段起点(后者用于 feature 门控的测试模块,如 render.rs 的
/// v3_engine_tests;生产 feature 门控写作 `#[cfg(feature = ...)]` 不匹配)。
fn marks_test_module(raw: &str) -> bool {
    raw.contains("#[cfg(test)]") || raw.contains("#[cfg(all(test")
}

/// 扫描单个文件的生产代码段,返回含 CJK 的 (行号, 行内容) 列表
fn scan_file(path: &Path) -> Vec<(usize, String)> {
    let content = std::fs::read_to_string(path).expect("test source readable");
    let mut hits = Vec::new();
    let mut in_test_module = false;
    for (i, raw) in content.lines().enumerate() {
        if marks_test_module(raw) {
            in_test_module = true;
        }
        if in_test_module {
            continue;
        }
        let code = strip_line_comment(raw);
        // CJK 行级片段豁免(诊断面字符串,逐条登记于 EXEMPT_CJK_SNIPPETS)
        if EXEMPT_CJK_SNIPPETS.iter().any(|s| code.contains(s)) {
            continue;
        }
        if code.chars().any(is_cjk) {
            hits.push((i + 1, raw.trim().to_string()));
        }
    }
    hits
}

#[test]
fn panels_actions_and_app_production_code_has_no_hardcoded_cjk() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut violations: Vec<String> = Vec::new();
    // WHY 混合目录与顶层文件:扫描域除子目录外含顶层 src/render.rs(B5-U
    // F-17 迁键后纳入)——路径是目录则递归收集,否则按单文件扫描
    for sub in ["src/panels", "src/actions", "src/app", "src/render.rs"] {
        let path = manifest.join(sub);
        let files = if path.is_dir() {
            collect_rs_files(&path)
        } else {
            vec![path]
        };
        for file in files {
            let rel = file
                .strip_prefix(&manifest)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| file.display().to_string());
            if EXEMPT_PREFIXES.iter().any(|x| rel.starts_with(x)) {
                continue;
            }
            for (lineno, line) in scan_file(&file) {
                violations.push(format!("{rel}:{lineno}: {line}"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "生产代码段发现硬编码 CJK(i18n 收口防退化,Concord W4 T4.4):\n{}",
        violations.join("\n")
    );
}

// ============================================================================
// 英文反向扫描(v3 复评批次-A US-02 门禁)
// ============================================================================

/// 英文扫描的文件级豁免前缀(仅作用于英文扫描;`EXEMPT_PREFIXES` 仍同时作用于
/// 全部三类扫描)。
///
/// 逐条登记理由:v3 复评 Top-1(US-02)/Top-2(US-01) 批次-A 仅覆盖 10 个文件,
/// 下列文件的存量英文属批次-B/后续批次迁移范围;本清单先冻结存量、禁止增量,
/// 后续批次迁移完成后应从此处移除对应前缀(只减不增棘轮)。
const EXEMPT_ENGLISH_PREFIXES: &[&str] = &[
    // US-01 专属文件(仅修 Debug 泄漏);全文件已被 EXEMPT_PREFIXES 豁免(CJK
    // 2026-08-17 登记"待用户 i18n 化"),英文存量同批处理,此处冗余登记防混淆
    // — 注意 EXEMPT_PREFIXES 已覆盖,本条仅为可读性注释位。
    // === 以下为批次-B/后续批次迁移范围(逐文件登记) ===
    //
    // B5-U(F-46)棘轮出清记录:overwindow.rs(代码已全走 t!)、help.rs
    // (滚动提示/命令语法/品牌行迁 help.scroll_hint / help.commands.line1-3 /
    // help.brand)、dag_viz.rs(F-30 三处迁 panel.dag.title/awaiting/more 后
    // 生产段英文清零)三条已移除——只减不增。
    //
    // B6-M(2026-10-04)棘轮出清记录:六面板(chtc/clv_vector/sysinfo/
    // experience_card_viz/resource_monitor/metrics_dashboard)正文英文全量迁移
    // (键族 panel.chtc.* / panel.clv.* / panel.sysinfo.* / panel.exp_card.* /
    // panel.resources.* / panel.metrics.*,zh/en 同步),六条豁免随迁移除——
    // 只减不增。语言无关技术标识(B{i} 块标号、C{i}: 每核标签、b{i} 类目、
    // dim=/value= 前缀、GB/MB/B 单位、UTC 时区后缀)经英文 token 口径
    // (≥2 个双字母以上词)天然放行,无需豁免登记。
    "src/panels/budget.rs",     // Budget 面板正文/告警文案待迁移
    "src/panels/chat.rs",       // Chat 面板文案待迁移
    "src/panels/decay.rs",      // Decay 面板残留文案待迁移
    "src/panels/health.rs",     // Health 面板残留文案待迁移
    "src/panels/memory.rs",     // Memory 面板残留文案待迁移
    "src/panels/parliament.rs", // 议会面板残留文案待迁移
    "src/panels/router.rs",     // 路由面板残留文案待迁移
    "src/panels/security.rs",   // 安全面板残留文案待迁移
    "src/actions",              // 动作域残留文案待迁移(与面板同批)
                                // (src/app 的 tests.rs / v3_render_tests.rs 已在 EXEMPT_PREFIXES 全局登记,
                                //  本清单不重复登记——英文扫描的豁免判定同时检查两份清单)
];

/// 英文扫描的行级豁免片段(子串命中即豁免该行;逐条登记理由)
///
/// WHY 行级而非文件级:仅 MCP 节点行的紧凑技术格式需要豁免,文件级豁免
/// 会放过该文件其余文案的增量退化。
const EXEMPT_ENGLISH_SNIPPETS: &[&str] = &[
    // msg/s + last_seen:MCP 节点行的单位符号与字段名标识符(紧凑技术格式,
    // 译为中文会破坏列对齐;与 timeline 面板 ev/s:/bud: 紧凑列同口径)。
    // B6-M(2026-10-04)复核:仍有实证依赖(mcp_nodes.rs 节点列表行格式串),
    // 保留豁免。
    "msg/s",
    "last_seen",
    // PgUp/PgDn:键盘键位标识符(语言中立,与 ↑/↓、Enter 同类,不译)
    "PgUp/PgDn",
    // Shift+Tab / Ctrl+Up / Ctrl+Down:help.rs global_shortcuts 键帽列
    // (键盘键位标识符,PgUp/PgDn 同类语言中立不译;B5-U help.rs 出清文件级
    // 豁免时登记——键帽非文案,随键帽行迁键应保持,非放水口)。
    // B6-M 复核:键帽行已迁键(help.rs 键帽列仍为字面量),键帽本身语言无关,
    // 登记为语言无关永久豁免。
    "Shift+Tab",
    "Ctrl+Up",
    "Ctrl+Down",
    // ={} tok:overwindow 语料/窗口行的字节单位格式(紧凑技术格式,与 msg/s
    // 同口径;数字插值居中,拆单键无收益。B5-U overwindow.rs 出清文件级豁免时
    // 登记"整行迁键后应移除"。B6-M 复核:该行标签部分(语料/窗口)已走键表
    // panel.overwindow.corpus/window,剩余 "{}={} tok" 为纯格式模板(tok 为
    // token 字节单位,语言无关),裁决为语言无关永久豁免)
    "={} tok",
    // === B4-I(F-04)src/app 域登记(逐条理由;随文案迁移应移除,只减不增)===
    // 事件源标识符(如 EventMetadata::new("chimera-tui")),机器协议面非文案
    "EventMetadata::new",
    // 确认弹窗内部协议命令(apply_confirm_command 的 strip_prefix 分发键)
    "export:csv",
    // 事件来源标识(publish_terminate 的 source 字段)
    "operator:terminate",
    // TuiHello 协议能力清单标识(与 chimera-cli handshake 约定,非文案)
    "slash-commands",
    "pane-mode",
    // tracing::warn! 日志消息(日志面英文惯例,非 UI 文案)
    "Failed to save TuiConfig",
    "Failed to save TuiState",
    // TuiError 错误诊断消息(错误面,非 UI 文案)
    "terminal size:",
    "v3 backend init:",
    "v3_term",
    // 环境变量名(语言中立标识符)
    "CHIMERA_NO_V3_ENGINE",
    // 终端能力白名单匹配串(term_program_supports_osc9 的技术比较,非文案)
    "windows terminal",
    // /status 与 /doctor 的键值诊断输出(quests=/budget= 等字段名协议化,
    // locale 切换无意义;若未来迁入键表应移除本条)
    "quests={} budget=",
    "tick={}ms persist=",
];

/// 判定字符串字面量是否为 i18n key 形状(如 `panel.log.keyword`)
///
/// WHY:t!()/tr() 调用点的 key 字面量本身是点分小写标识符,含多个"英文词",
/// 但它不是用户可见文案(渲染时经键表解析),必须从英文扫描中排除。
fn looks_like_i18n_key(literal: &str) -> bool {
    literal.contains('.')
        && !literal.is_empty()
        && literal
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
}

/// 提取一行内的字符串字面量内容(简化状态机:处理 `\` 转义)
///
/// WHY 简化可接受:panels/actions 域无 raw string(`r"..."`)与多行字符串字面量
/// 的用户文案场景;若未来出现,扩展为完整词法解析并在此注明。
fn extract_string_literals(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut escaped = false;
    for c in line.chars() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
                out.push(std::mem::take(&mut cur));
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_str = true;
            cur.clear();
        }
    }
    out
}

/// 统计字符串内容中的英文词 token 数
///
/// 启发式口径(US-02 门禁):
/// - 先剥除 `{...}` 格式占位符(占位符是插值点,非英文文案);
/// - token = 连续 `[A-Za-z_]` 段(下划线并入,`last_seen` 这类 snake_case
///   标识符算一个词),仅统计字母数 ≥ 2 的 token(`N/A`/`msg/s` 中的单字母
///   段不计,单位符号与单字母缩写不构成英文短语);
/// - token 数 ≥ 2 判定为英文短语(如 "Event Stream"、"showing of events")。
fn english_token_count(s: &str) -> usize {
    // 剥除 {...} 占位符(深度计数容错,panels 域无嵌套花括号字符串)
    let mut cleaned = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => cleaned.push(c),
            _ => {}
        }
    }
    cleaned
        .split(|c: char| !(c.is_ascii_alphabetic() || c == '_'))
        .filter(|t| t.chars().filter(|ch| ch.is_ascii_alphabetic()).count() >= 2)
        .count()
}

/// 扫描单个文件生产代码段中的英文短语(返回 (行号, 行内容))
fn scan_english_file(path: &Path) -> Vec<(usize, String)> {
    let content = std::fs::read_to_string(path).expect("test source readable");
    let mut hits = Vec::new();
    let mut in_test_module = false;
    for (i, raw) in content.lines().enumerate() {
        if marks_test_module(raw) {
            in_test_module = true;
        }
        if in_test_module {
            continue;
        }
        let code = strip_line_comment(raw);
        // 行级片段豁免(技术标识符/单位,逐条登记于 EXEMPT_ENGLISH_SNIPPETS)
        if EXEMPT_ENGLISH_SNIPPETS.iter().any(|s| code.contains(s)) {
            continue;
        }
        for literal in extract_string_literals(code) {
            if looks_like_i18n_key(&literal) {
                continue;
            }
            if english_token_count(&literal) >= 2 {
                hits.push((i + 1, raw.trim().to_string()));
                break;
            }
        }
    }
    hits
}

#[test]
fn panels_actions_and_app_production_code_has_no_hardcoded_english() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut violations: Vec<String> = Vec::new();
    // WHY 混合目录与顶层文件:同 CJK 扫描口径(B5-U F-17 起纳入 src/render.rs)
    for sub in ["src/panels", "src/actions", "src/app", "src/render.rs"] {
        let path = manifest.join(sub);
        let files = if path.is_dir() {
            collect_rs_files(&path)
        } else {
            vec![path]
        };
        for file in files {
            let rel = file
                .strip_prefix(&manifest)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| file.display().to_string());
            if EXEMPT_PREFIXES.iter().any(|x| rel.starts_with(x))
                || EXEMPT_ENGLISH_PREFIXES.iter().any(|x| rel.starts_with(x))
            {
                continue;
            }
            for (lineno, line) in scan_english_file(&file) {
                violations.push(format!("{rel}:{lineno}: {line}"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "生产代码段发现硬编码英文短语(US-02 i18n 收口防退化):\n{}",
        violations.join("\n")
    );
}

// ============================================================================
// {:?} Debug 泄漏扫描(v3 复评批次-A US-01 门禁)
// ============================================================================

/// {:?} 扫描的文件级豁免前缀(当前为空:US-01 清零后无豁免)
///
/// WHY 不沿用 EXEMPT_PREFIXES:task_manager.rs 因 CJK 存量被豁免,但其
/// `Mode: {:?}` 是真实用户可见 Debug 泄漏(US-01 修复点),不能连带豁免。
/// 未来若出现确属合理的生产 Debug 格式化(如纯调试日志),逐条登记并注明理由。
const EXEMPT_DEBUG_LEAK_PREFIXES: &[&str] = &[];

/// 扫描单个文件生产代码段中的 `{:?}` 格式化(返回 (行号, 行内容))
///
/// WHY 按行匹配而非限定 format!/push/Line/Text 关键字:多行 format! 调用的
/// 格式串常独占一行(如 quest.rs 元信息行),按行限定调用关键字会漏报格式串行;
/// panels 生产段当前无合法 `{:?}` 用法,逐行匹配是零漏报口径。
fn scan_debug_leak_file(path: &Path) -> Vec<(usize, String)> {
    let content = std::fs::read_to_string(path).expect("test source readable");
    let mut hits = Vec::new();
    let mut in_test_module = false;
    for (i, raw) in content.lines().enumerate() {
        if marks_test_module(raw) {
            in_test_module = true;
        }
        if in_test_module {
            continue;
        }
        let code = strip_line_comment(raw);
        if code.contains("{:?}") {
            hits.push((i + 1, raw.trim().to_string()));
        }
    }
    hits
}

#[test]
fn panels_production_code_has_no_debug_format_leak() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("src/panels");
    let mut violations: Vec<String> = Vec::new();
    for file in collect_rs_files(&dir) {
        let rel = file
            .strip_prefix(&manifest)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| file.display().to_string());
        if EXEMPT_DEBUG_LEAK_PREFIXES
            .iter()
            .any(|x| rel.starts_with(x))
        {
            continue;
        }
        for (lineno, line) in scan_debug_leak_file(&file) {
            violations.push(format!("{rel}:{lineno}: {line}"));
        }
    }
    assert!(
        violations.is_empty(),
        "生产代码段发现 {{:?}} Debug 格式泄漏(US-01 用户可见文案禁用 Debug 串):\n{}",
        violations.join("\n")
    );
}

#[test]
fn ctrl_l_panel_copy_switches_language_en() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    // Ctrl+L 切换实证(方案 P6 承诺):En locale 下面板 shortcuts 文案为英文
    let _guard = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    use chimera_tui::panels::Panel;
    let panel = chimera_tui::panels::log::LogPanel::new();
    let descs: Vec<&str> = panel.shortcuts().iter().map(|(_, d)| *d).collect();
    assert!(
        descs.contains(&"Navigate"),
        "En locale 下面板快捷键文案应为英文: {descs:?}"
    );
    chimera_tui::set_locale(chimera_tui::Locale::Zh);
    let descs_zh: Vec<&str> = panel.shortcuts().iter().map(|(_, d)| *d).collect();
    assert!(
        descs_zh.contains(&"导航"),
        "Zh locale 下面板快捷键文案应为中文: {descs_zh:?}"
    );
}
