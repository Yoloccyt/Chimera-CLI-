//! PS-2 U-1/U-2:百分比格式化统一 —— 不变量 + 契约测试
//!
//! # 背景(评估报告 U-1/U-2)
//! - **U-1(基数歧义)**:同一面板内混用两种基数且格式串完全相同 ——
//!   `memory.rs` 的 `hit_rate_percent` 是 0-100 值、`compressed_ratio` 是 0-1 比值,
//!   两处都写 `{:.1}%`,读代码无法判断该不该乘 100。
//! - **U-2(精度散落)**:同类字段在不同视图各写 `{:.0}` / `{:.1}`,无统一约定。
//!
//! # 本文件守护两条契约
//! 1. **静态不变量**:`src/**` 中不得出现"带字面精度的百分号格式"
//!    (如 `{:.1}%` / `{:>3.0}%`)—— 基座与精度必须经由 `crate::render` 的
//!    统一辅助表达,使调用点自解释;
//! 2. **辅助契约**:`percent_*` 系列的行为(含 NaN、越界、f32/f64)与
//!    精度常量取值的确定性断言。

#![forbid(unsafe_code)]

use chimera_tui::render::{
    percent_detail, percent_from_ratio, percent_from_value, percent_summary, PercentValue,
    PERCENT_PRECISION_DETAIL, PERCENT_PRECISION_SUMMARY,
};

// ============================================================
// 1. 静态不变量:百分比格式不得内联字面精度
// ============================================================

/// 递归收集 `src/**` 的代码行(已剥离 `//` 行注释)
///
/// WHY 剥离注释:文档注释里常引用格式串(如本文件头部),
/// 那些不是真实格式化点,不计入检查。
fn collect_src_code_lines() -> Vec<(String, usize, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, usize, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (idx, line) in text.lines().enumerate() {
                let code = match line.find("//") {
                    Some(i) => &line[..i],
                    None => line,
                };
                out.push((path.display().to_string(), idx + 1, code.to_string()));
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    walk(&root, &mut out);
    out
}

/// 判断一行是否含"带字面精度的百分号格式"
///
/// 匹配形如 `{:...1}%` 的格式占位符(精度为字面数字)。
/// 不匹配 `{:.*}%`(精度由参数传入 —— 统一辅助内部即用此形式),
/// 也不匹配 `"{}%"`(无精度,如给已预格式化字符串补 `%` 的场景)。
fn has_literal_precision_percent(code: &str) -> bool {
    let bytes = code.as_bytes();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b':' {
            // 找到本占位符结束
            let Some(close_rel) = code[i..].find('}') else {
                return false;
            };
            let close = i + close_rel;
            let spec = &code[i + 2..close];
            // 形态:`.` 后紧跟数字(字面精度)
            let literal_precision = spec
                .find('.')
                .map(|dot| {
                    spec[dot + 1..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_digit())
                })
                .unwrap_or(false);
            // 占位符紧跟 `%` 才是百分比格式
            if literal_precision && code[close + 1..].starts_with('%') {
                return true;
            }
            i = close + 1;
        } else {
            i += 1;
        }
    }
    false
}

#[test]
fn no_inline_literal_precision_percent_formats_in_src() {
    let violations: Vec<String> = collect_src_code_lines()
        .into_iter()
        .filter(|(_, _, code)| has_literal_precision_percent(code))
        .map(|(path, line, code)| format!("{path}:{line} — {}", code.trim()))
        .collect();

    assert!(
        violations.is_empty(),
        "以下位置内联了带字面精度的百分号格式(基数与精度因此隐含在调用点,\n\
         正是 U-1/U-2 的成因)。请改用 crate::render 的统一辅助:\n\
         · 0-1 比值 → percent_from_ratio / percent_summary / percent_detail\n\
         · 0-100 数值 → percent_from_value(value, PERCENT_PRECISION_*)\n\
         违规清单:\n{}",
        violations.join("\n")
    );
}

// ============================================================
// 2. 辅助契约(含边界)
// ============================================================

#[test]
fn precision_constants_encode_the_convention() {
    // 约定:摘要=整数位,详情=一位小数(由常量承载,避免各视图自行选值)
    assert_eq!(PERCENT_PRECISION_SUMMARY, 0);
    assert_eq!(PERCENT_PRECISION_DETAIL, 1);
}

#[test]
fn ratio_and_value_helpers_are_not_interchangeable() {
    // U-1 的核心:两种基数必须产生不同结果,否则调用点无法自证正确
    assert_eq!(percent_from_ratio(0.72_f64, 1), "72.0%");
    assert_eq!(percent_from_value(0.72_f64, 1), "0.7%");
    // 同一位小数精度下,0-100 值直显
    assert_eq!(percent_from_value(87.5_f64, 1), "87.5%");
    // 0-1 比值乘 100
    assert_eq!(percent_from_ratio(0.875_f64, 1), "87.5%");
}

#[test]
fn summary_and_detail_wrappers_apply_the_expected_precision() {
    assert_eq!(percent_summary(0.874_f64), "87%");
    assert_eq!(percent_detail(0.874_f64), "87.4%");
}

#[test]
fn helpers_accept_both_f32_and_f64() {
    // 面板字段类型不一(memory 的 hit_rate_percent 是 f32、budget 的
    // utilization_rate 是 f64),trait 收敛保证两型同用一套 API
    assert_eq!(percent_detail(0.5_f32), "50.0%");
    assert_eq!(percent_detail(0.5_f64), "50.0%");
    assert_eq!(percent_from_value(100.0_f32, 0), "100%");
}

#[test]
fn percent_value_trait_widens_explicitly_and_losslessly() {
    // trait 的存在意义:把 f32→f64 的加宽收敛到一处**显式**实现,
    // 而非散落在各调用点的 `as f64`;对有限值为精确变换。
    assert_eq!(0.5_f32.as_f64(), 0.5_f64);
    assert_eq!(0.5_f64.as_f64(), 0.5_f64);
    // 可表示性的极端值:f32 的最大有限值加宽后仍是有限 f64
    assert!(f32::MAX.as_f64().is_finite());
}

#[test]
fn helpers_do_not_silently_mask_nan_or_clamp_out_of_range() {
    // 契约:**不**做 NaN 遮掩、**不**做钳位 —— 那是调用点的语义决策
    // (如 budget 面板显式把 NaN 渲染为 "N/A";osa_sparse 先 clamp 再格式化)。
    // 此处固化"辅助保持中立",防止有人日后悄悄加钳位而改变各面板行为。
    assert_eq!(percent_from_ratio(f64::NAN, 1), "NaN%");
    assert_eq!(percent_from_ratio(1.5_f64, 1), "150.0%");
    assert_eq!(percent_from_ratio(-0.25_f64, 1), "-25.0%");
}
