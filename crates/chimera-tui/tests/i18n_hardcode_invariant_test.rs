//! i18n 硬编码防退化不变量测试 — Concord W4 T4.4(M4 门禁证据)
//!
//! 对应架构层:L10 Interface
//!
//! # 守护语义
//! panels/ 与 actions/ 的**生产代码段**(`#[cfg(test)]` 之前)不得出现
//! CJK 字面量——用户可见文案一律走 i18n 键表(crate::t!),保证 Ctrl+L
//! 语言切换在面板层真实生效(方案 P6 承诺兑现)。
//!
//! # 口径说明
//! - 行尾 `//` 注释先剥离(注释是开发文档媒介,中文为项目约定);
//! - `#[cfg(test)]` 之后的测试模块豁免(断言消息非渲染面);
//! - 新增面板/动作时若确需豁免,经评审后加入 EXEMPT 清单并注明理由。

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
const EXEMPT_PREFIXES: &[&str] = &[];

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

/// 扫描单个文件的生产代码段,返回含 CJK 的 (行号, 行内容) 列表
fn scan_file(path: &Path) -> Vec<(usize, String)> {
    let content = std::fs::read_to_string(path).expect("test source readable");
    let mut hits = Vec::new();
    let mut in_test_module = false;
    for (i, raw) in content.lines().enumerate() {
        if raw.contains("#[cfg(test)]") {
            in_test_module = true;
        }
        if in_test_module {
            continue;
        }
        let code = strip_line_comment(raw);
        if code.chars().any(is_cjk) {
            hits.push((i + 1, raw.trim().to_string()));
        }
    }
    hits
}

#[test]
fn panels_and_actions_production_code_has_no_hardcoded_cjk() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut violations: Vec<String> = Vec::new();
    for sub in ["src/panels", "src/actions"] {
        let dir = manifest.join(sub);
        for file in collect_rs_files(&dir) {
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
    "src/panels/budget.rs",              // Budget 面板正文/告警文案待迁移
    "src/panels/chat.rs",                // Chat 面板文案待迁移
    "src/panels/chtc.rs",                // CHTC 适配器面板正文待迁移
    "src/panels/clv_vector.rs",          // CLV 向量面板正文待迁移
    "src/panels/dag_viz.rs",             // DAG 可视化面板正文待迁移
    "src/panels/decay.rs",               // Decay 面板残留文案待迁移
    "src/panels/experience_card_viz.rs", // 经验卡片面板正文待迁移
    "src/panels/health.rs",              // Health 面板残留文案待迁移
    "src/panels/help.rs",                // 帮助面板搜索提示待迁移
    "src/panels/memory.rs",              // Memory 面板残留文案待迁移
    "src/panels/metrics_dashboard.rs",   // 指标仪表盘残留文案待迁移
    "src/panels/overwindow.rs",          // 超窗面板残留文案待迁移
    "src/panels/parliament.rs",          // 议会面板残留文案待迁移
    "src/panels/resource_monitor.rs",    // 资源监控残留文案待迁移
    "src/panels/router.rs",              // 路由面板残留文案待迁移
    "src/panels/security.rs",            // 安全面板残留文案待迁移
    "src/panels/sysinfo.rs",             // 系统信息残留文案待迁移
    "src/actions",                       // 动作域残留文案待迁移(与面板同批)
];

/// 英文扫描的行级豁免片段(子串命中即豁免该行;逐条登记理由)
///
/// WHY 行级而非文件级:仅 MCP 节点行的紧凑技术格式需要豁免,文件级豁免
/// 会放过该文件其余文案的增量退化。
const EXEMPT_ENGLISH_SNIPPETS: &[&str] = &[
    // msg/s + last_seen:MCP 节点行的单位符号与字段名标识符(紧凑技术格式,
    // 译为中文会破坏列对齐;与 timeline 面板 ev/s:/bud: 紧凑列同口径)
    "msg/s",
    "last_seen",
    // PgUp/PgDn:键盘键位标识符(语言中立,与 ↑/↓、Enter 同类,不译)
    "PgUp/PgDn",
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
        if raw.contains("#[cfg(test)]") {
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
fn panels_and_actions_production_code_has_no_hardcoded_english() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut violations: Vec<String> = Vec::new();
    for sub in ["src/panels", "src/actions"] {
        let dir = manifest.join(sub);
        for file in collect_rs_files(&dir) {
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
        if raw.contains("#[cfg(test)]") {
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
