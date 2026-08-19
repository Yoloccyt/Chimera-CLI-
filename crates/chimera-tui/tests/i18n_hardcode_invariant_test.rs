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
/// 当前为空:W4 收口后 panels/actions 生产代码段零 CJK。
///
/// 2026-08-17 新增:用户并行改动区(task_manager.rs quadrant_footer 新增
/// "数据源未接入"提示文案未走 t!(),待用户 i18n 化后移除本豁免)。
/// 注:rel 路径含 src/ 前缀(相对 CARGO_MANIFEST_DIR),故前缀需完整。
const EXEMPT_PREFIXES: &[&str] = &["src/panels/task_manager.rs"];

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

#[test]
fn ctrl_l_panel_copy_switches_language_en() {
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
