//! i18n 完整性检测 — 缺失文案/无效 key/硬编码 CJK 检测机制(P2-6)
//!
//! 对应架构层:L10 Interface
//!
//! # 检测内容(P2-6)
//! 1. **无效 key 检测**: 所有 `t!("key")` / `tr("key")` 调用的 key 必须在
//!    zh 和 en 两个翻译表中同时存在。拼写错误或遗漏的 key 会导致 UI
//!    回退显示 key 本身(如 `panel.quest.titl`),影响用户体验。
//!
//! 2. **硬编码 CJK 检测**: panel 渲染代码中的字符串字面量若包含 CJK
//!    字符(中日韩统一表意文字),应通过 `t!()` 国际化,而非硬编码。
//!    本检测扫描 `src/panels/` 目录下的字符串字面量,报告含 CJK 的
//!    硬编码文案,确保中英切换覆盖所有 UI 文本。
//!
//! 3. **SEED_KEYS 对齐检测**: 已有 `i18n::tests::zh_and_en_tables_cover_same_seed_keys`
//!    覆盖(本文件不重复)。
//!
//! # 设计决策(WHY)
//! - **无外部依赖**: 不引入 `regex` / `walkdir` dev-dependency,使用 `std::fs`
//!   递归扫描 + 手写字符串解析,保持 `cargo audit` 面最小。
//! - **注释感知**: CJK 检测跳过 `//` 行注释和 `/* */` 块注释,避免
//!   将注释中的中文误报为硬编码文案。
//! - **白名单机制**: 已知合理的 CJK 字面量(如 SQL 表名、format! 模板中的
//!   固定文案)可通过 `ALLOWED_CJK_LITERALS` 白名单豁免。
//!
//! # 运行
//! ```bash
//! cargo test -p chimera-tui --test i18n_completeness_test
//! ```

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

// ============================================================================
// 工具函数: 文件遍历与字符串解析
// ============================================================================

/// 递归收集目录下所有 `.rs` 文件
fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

/// 从源码文本中提取所有 `t!("...")` 和 `tr("...")` 调用的 key
///
/// 搜索模式:
/// - `t!("key")` — 宏调用
/// - `t!(r"key")` — 宏调用(raw string)
/// - `tr("key")` — 直接函数调用
///
/// 返回 `(key, line_number)` 元组列表,便于定位错误
fn extract_i18n_keys(source: &str) -> Vec<(String, usize)> {
    let mut keys = Vec::new();
    for (line_num, line) in source.lines().enumerate() {
        // 在每一行中搜索 t!(" 或 tr(" 模式
        // WHY 词边界检查: `t!(` 是 `format!(` 的子串(t 在 format 末尾),
        // `tr(` 是 `ptr(` / `str(` 的子串。必须检查匹配位置前一个字符
        // 不是标识符字符(alphanumeric / _),避免误匹配。
        for pat in &["t!(", "tr("] {
            let mut search_from = 0;
            while let Some(pos) = line[search_from..].find(pat) {
                let abs_start = search_from + pos;

                // 词边界检查: 确保匹配位置前一个字符不是标识符字符
                // 这防止 `format!(` 中的 `t!(` 被误匹配,以及 `ptr(` 中的 `tr(` 被误匹配
                if abs_start > 0 {
                    let prev_char = line[..abs_start].chars().next_back().unwrap();
                    if prev_char.is_alphanumeric() || prev_char == '_' {
                        search_from = abs_start + 1;
                        continue;
                    }
                }

                let abs_pos = abs_start + pat.len();
                if abs_pos >= line.len() {
                    break;
                }
                // 跳过空白字符
                let rest = line[abs_pos..].trim_start();
                let skip = line[abs_pos..].len() - rest.len();
                // 检查是否是字符串字面量 "..." 或 r"..."
                // WHY strip_prefix 替代 starts_with + 手动切片:clippy::manual_strip 建议,
                // 避免冗余边界检查且语义更清晰
                let key = if let Some(stripped) = rest.strip_prefix("\"") {
                    // 普通字符串: 提取引号内的内容
                    extract_string_literal(stripped)
                } else if let Some(stripped) = rest.strip_prefix("r\"") {
                    // raw 字符串: 提取引号内的内容
                    extract_string_literal(stripped)
                } else {
                    // 非字符串字面量(可能是变量或 concat!),跳过
                    None
                };
                if let Some(k) = key {
                    keys.push((k, line_num + 1));
                }
                // 移动搜索位置,避免重复匹配
                search_from = abs_pos + skip + 1;
            }
        }
    }
    keys
}

/// 从字符串开头提取到下一个未转义引号为止的内容
fn extract_string_literal(s: &str) -> Option<String> {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // 转义字符,取下一个字符
                if let Some(next) = chars.next() {
                    result.push(next);
                }
            }
            '"' => {
                return Some(result);
            }
            _ => {
                result.push(c);
            }
        }
    }
    None // 未找到闭合引号
}

/// 从源码中移除注释,保留字符串字面量中的内容
///
/// WHY: CJK 检测需要区分注释中的中文(合法)和字符串字面量中的中文(需 i18n)。
/// 本函数移除 `//` 行注释和 `/* */` 块注释,但保留字符串字面量。
fn strip_comments(source: &str) -> String {
    let mut result = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                // 字符串字面量: 保留完整内容(含转义)
                result.push(c);
                while let Some(&sc) = chars.peek() {
                    result.push(chars.next().unwrap());
                    if sc == '\\' {
                        // 转义,取下一个字符
                        if let Some(nc) = chars.next() {
                            result.push(nc);
                        }
                    } else if sc == '"' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                // 行注释: 跳过到行尾
                chars.next();
                for c in chars.by_ref() {
                    if c == '\n' {
                        result.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                // 块注释: 跳过到 */
                chars.next();
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
            }
            _ => {
                result.push(c);
            }
        }
    }
    result
}

/// 从源码中移除 `mod tests { ... }` 块(含 `#[cfg(test)]` 标注的测试模块)
///
/// WHY: panel 文件的 `#[cfg(test)] mod tests { ... }` 块中含大量中文
/// 断言字符串(如 `assert_eq!(..., "空列表")`),这些是测试代码而非
/// UI 文案,不应要求 i18n。本函数用花括号深度计数定位 `mod tests` 块
/// 的结束位置,将整个块替换为空行,保留行号对齐。
fn strip_test_modules(source: &str) -> String {
    let mut result = String::with_capacity(source.len());
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        // 检测 `mod tests` 块的开始(可能前面有 #[cfg(test)] 标注)
        if line.contains("mod tests") && line.contains('{') {
            // 找到 mod tests 块,用花括号深度计数跳到匹配的 }
            let mut depth: i32 = 0;
            for ch in line.chars() {
                if ch == '{' {
                    depth += 1;
                } else if ch == '}' {
                    depth -= 1;
                }
            }
            // 逐行消费直到 depth 回到 0
            result.push('\n'); // 保留行号对齐
            while depth > 0 {
                match lines.next() {
                    Some(l) => {
                        for ch in l.chars() {
                            if ch == '{' {
                                depth += 1;
                            } else if ch == '}' {
                                depth -= 1;
                            }
                        }
                        result.push('\n'); // 保留行号对齐
                    }
                    None => break,
                }
            }
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }
    result
}

/// 检查字符串是否包含 CJK 统一表意文字(U+4E00..U+9FFF)
fn contains_cjk(s: &str) -> bool {
    s.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c))
}

/// 从去注释源码中提取包含 CJK 的字符串字面量
fn extract_cjk_string_literals(source: &str) -> Vec<(String, usize)> {
    let mut results = Vec::new();
    for (line_num, line) in source.lines().enumerate() {
        let mut in_string = false;
        let mut current = String::new();
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if in_string {
                match c {
                    '\\' => {
                        current.push(c);
                        if let Some(&nc) = chars.peek() {
                            current.push(chars.next().unwrap());
                            if nc == '"' {
                                // 转义引号,继续在字符串内
                            }
                        }
                    }
                    '"' => {
                        // 字符串结束
                        if contains_cjk(&current) {
                            results.push((current.clone(), line_num + 1));
                        }
                        current.clear();
                        in_string = false;
                    }
                    _ => {
                        current.push(c);
                    }
                }
            } else if c == '"' {
                in_string = true;
                current.clear();
            }
        }
    }
    results
}

// ============================================================================
// 测试 1: 所有 t!() / tr() 调用的 key 必须在 zh 和 en 表中同时存在
// ============================================================================

/// 验证所有 `t!()` / `tr()` 调用的 key 在中英翻译表中都存在
///
/// WHY: 拼写错误或遗漏的 key 会导致 `tr()` 回退返回 key 本身
/// (如 `panel.quest.titl`),用户看到的是技术性 key 而非人类可读文案。
/// 本测试在编译/测试期捕获此类问题,防止漏译进入生产。
#[test]
fn all_t_macro_keys_exist_in_both_tables() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let src_dir = Path::new(manifest_dir).join("src");
    let mut files = Vec::new();
    collect_rust_files(&src_dir, &mut files);

    let mut all_keys: Vec<(String, String, usize)> = Vec::new(); // (key, file, line)
    for file in &files {
        // 排除 i18n 模块本身: 其内部的测试和文档示例使用故意不存在的 key
        // (如 `nonexistent.key.xyz` 测试回退行为),不是真正的无效 key
        let file_str = file.to_string_lossy().replace('\\', "/");
        if file_str.contains("/i18n/") {
            continue;
        }
        let Ok(source) = fs::read_to_string(file) else {
            continue;
        };
        for (key, line) in extract_i18n_keys(&source) {
            let file_name = file
                .strip_prefix(manifest_dir)
                .unwrap_or(file)
                .to_string_lossy()
                .replace('\\', "/");
            all_keys.push((key, file_name, line));
        }
    }

    assert!(!all_keys.is_empty(), "应至少找到一个 t!() 调用");

    let mut errors = Vec::new();
    for (key, file, line) in &all_keys {
        if chimera_tui::i18n::zh::lookup(key).is_none() {
            errors.push(format!(
                "  - key '{key}' 不在 zh 翻译表中 (调用位置: {file}:{line})"
            ));
        }
        if chimera_tui::i18n::en::lookup(key).is_none() {
            errors.push(format!(
                "  - key '{key}' 不在 en 翻译表中 (调用位置: {file}:{line})"
            ));
        }
    }

    assert!(
        errors.is_empty(),
        "发现 {} 个无效 i18n key(不在翻译表中):\n{}",
        errors.len(),
        errors.join("\n")
    );
}

// ============================================================================
// 测试 2: panel 渲染代码中无硬编码 CJK 字符串
// ============================================================================

/// 已知合理的 CJK 字面量白名单 — 这些字符串虽含 CJK 但不应通过 t!() 国际化
///
/// WHY: 某些 CJK 字面量是技术性标识符(如数据库表名、format! 模板中的
/// 固定分隔符),不属于用户可见的 UI 文案,无需 i18n。白名单机制避免
/// 误报,同时保持对新硬编码文案的检测能力。
///
/// 格式: (文件名后缀, CJK 字面量)
const ALLOWED_CJK_LITERALS: &[(&str, &str)] = &[
    // 示例(按需添加):
    // ("panels/quest.rs", "任务表"),
];

/// 已知 i18n 违规基线 — 现存硬编码 CJK UI 文案,待后续任务渐进修复
///
/// WHY(P2-6 基线锁定策略):
/// P2-6 的目标是建立 i18n 缺失文案**检测机制**,而非一次性修复所有
/// 历史遗留的硬编码文案。采用"基线锁定"策略:
/// 1. 现存违规记入此基线列表,测试通过(不阻塞当前 CI)
/// 2. 新增违规不在基线中,测试失败(阻塞 CI,防止问题扩大)
/// 3. 后续任务逐步将违规文案 i18n 化,从基线列表移除
/// 4. 基线列表清空时,所有 UI 文案完成 i18n
///
/// 修复方式: 为每个违规文案在 `SEED_KEYS` 中新增 key,在 zh/en 表中
/// 添加翻译,然后将代码中的硬编码字符串替换为 `t!("new.key")`。
///
/// # 系统性问题备注(2026-07-28)
/// 大多数违规位于各 panel 的 `shortcuts()` 方法返回的 `Vec<(&'static str,
/// &'static str)>` 中。由于 `shortcuts()` 返回 `&'static str` 引用,而
/// `t!()` 返回的 `&'static str` 依赖运行时 locale,将 `shortcuts()` 迁移
/// 到 i18n 需要重构 trait 签名(返回 `String` 或 `Cow<'static, str>`),
/// 影响所有 17 个 panel。这应作为独立的 P3 i18n 重构任务处理,
/// 而非在 P2-6 检测机制任务中一并完成。
const KNOWN_VIOLATIONS: &[(&str, &str)] = &[
    // panels/budget.rs
    ("panels/budget.rs", "刷新"),
    // panels/chat.rs
    ("panels/chat.rs", "输入消息"),
    ("panels/chat.rs", "发送"),
    ("panels/chat.rs", "退出输入"),
    ("panels/chat.rs", "滚动"),
    ("panels/chat.rs", "跳顶/贴底"),
    // panels/chtc.rs
    ("panels/chtc.rs", "导航"),
    ("panels/chtc.rs", "刷新"),
    // panels/clv_vector.rs
    ("panels/clv_vector.rs", "刷新"),
    ("panels/clv_vector.rs", "导航"),
    // panels/decay.rs
    ("panels/decay.rs", "导航"),
    ("panels/decay.rs", "刷新"),
    // panels/event_stream.rs
    (
        "panels/event_stream.rs",
        "[ALERT] CRITICAL 事件丢弃: {} (旁路通道容量满)",
    ),
    ("panels/event_stream.rs", "[新事件 {} 条] 按 G 跟随"),
    ("panels/event_stream.rs", "导航"),
    ("panels/event_stream.rs", "刷新"),
    ("panels/event_stream.rs", "时间跳转"),
    ("panels/event_stream.rs", "翻页"),
    ("panels/event_stream.rs", "跳顶"),
    ("panels/event_stream.rs", "跳底"),
    ("panels/event_stream.rs", "详情"),
    ("panels/event_stream.rs", "过滤"),
    // panels/health.rs
    ("panels/health.rs", "刷新"),
    // panels/help.rs
    ("panels/help.rs", "关闭"),
    ("panels/help.rs", "显示帮助"),
    // panels/log.rs
    ("panels/log.rs", "导航"),
    ("panels/log.rs", "刷新"),
    ("panels/log.rs", "翻页"),
    ("panels/log.rs", "跳顶"),
    ("panels/log.rs", "跳底"),
    ("panels/log.rs", "过滤"),
    // panels/mcp_nodes.rs
    ("panels/mcp_nodes.rs", "导航"),
    ("panels/mcp_nodes.rs", "刷新"),
    // panels/memory.rs
    ("panels/memory.rs", "导航"),
    ("panels/memory.rs", "刷新"),
    // panels/metrics_dashboard.rs
    ("panels/metrics_dashboard.rs", "导航"),
    ("panels/metrics_dashboard.rs", "刷新"),
    // panels/osa_sparse.rs
    ("panels/osa_sparse.rs", "导航"),
    ("panels/osa_sparse.rs", "刷新"),
    // panels/osa_sparse.rs(2026-08-02 补充:五维掩码标题 + omega-learner 六接缝状态文案)
    ("panels/osa_sparse.rs", "五维掩码状态"),
    ("panels/osa_sparse.rs", "omega-learner 六接缝状态: 暂未实现"),
    // panels/parliament.rs
    ("panels/parliament.rs", "导航"),
    ("panels/parliament.rs", "投票"),
    ("panels/parliament.rs", "是/否/弃权"),
    // P3(I-9):移除虚假 V/Y/N/A 投票提示后,Enter=详情文案加入白名单
    ("panels/parliament.rs", "详情"),
    // panels/pvl_score.rs(2026-08-02 补充:PVL 过程评分九维度标签 + 快捷键文案)
    ("panels/pvl_score.rs", "真实执行"),
    ("panels/pvl_score.rs", "覆盖率"),
    ("panels/pvl_score.rs", "验证通过"),
    ("panels/pvl_score.rs", "置信度"),
    ("panels/pvl_score.rs", "效率"),
    ("panels/pvl_score.rs", "重试纪律"),
    ("panels/pvl_score.rs", "产出实质性"),
    ("panels/pvl_score.rs", "零孤儿"),
    ("panels/pvl_score.rs", "沙箱清洁"),
    ("panels/pvl_score.rs", "选择维度"),
    ("panels/pvl_score.rs", "导航"),
    // panels/quest.rs
    ("panels/quest.rs", "导航"),
    ("panels/quest.rs", "详情"),
    // P3(I-1 修复):Quest 详情改绑 `v` 后新增的快捷键文案,
    // 与既有 shortcuts 白名单同批(面板快捷键整体 i18n 收口时一并迁移)
    ("panels/quest.rs", "翻页"),
    ("panels/quest.rs", "跳转事件流"),
    ("panels/quest.rs", "跳顶"),
    ("panels/quest.rs", "跳底"),
    // panels/resource_monitor.rs
    ("panels/resource_monitor.rs", "刷新"),
    // panels/router.rs
    ("panels/router.rs", "导航"),
    ("panels/router.rs", "刷新"),
    // panels/security.rs
    ("panels/security.rs", "导航"),
    ("panels/security.rs", "详情"),
    // panels/self_assessment.rs
    ("panels/self_assessment.rs", "切换面板"),
    // panels/dag_viz.rs(closure Stage B-10,同 self_assessment 的展示型快捷键文案)
    ("panels/dag_viz.rs", "切换面板"),
    // panels/overwindow.rs(P1,ADR-072:展示型面板快捷键沿用既有 "切换面板" 惯例,
    // 面板级 i18n 收口时随自评/DAG 面板一并迁移)
    ("panels/overwindow.rs", "切换面板"),
    // panels/sysinfo.rs
    ("panels/sysinfo.rs", "刷新"),
    // panels/task_manager.rs
    ("panels/task_manager.rs", "导航"),
    ("panels/task_manager.rs", "过滤搜索"),
    ("panels/task_manager.rs", "暂停"),
    ("panels/task_manager.rs", "批量暂停"),
    ("panels/task_manager.rs", "恢复"),
    ("panels/task_manager.rs", "终止"),
    ("panels/task_manager.rs", "优先级"),
    ("panels/task_manager.rs", "排序"),
    ("panels/task_manager.rs", "导出"),
    ("panels/task_manager.rs", "详情"),
    ("panels/task_manager.rs", "清除选择"),
    ("panels/task_manager.rs", "多选"),
    ("panels/task_manager.rs", "全选"),
    // panels/timeline.rs
    ("panels/timeline.rs", "导航"),
    ("panels/timeline.rs", "时间跳转"),
];

/// 检测 `src/panels/` 目录下的硬编码 CJK 字符串字面量
///
/// WHY: panel 渲染代码中的 CJK 字符串是用户可见的 UI 文案,应通过
/// `t!()` 国际化。硬编码 CJK 会导致英文模式下仍显示中文,破坏
/// 中英切换体验(ADR-029 v3.1 i18n 设计目标)。
///
/// 检测策略:
/// 1. 移除注释(避免将注释中的中文误报)
/// 2. 提取字符串字面量中的 CJK 内容
/// 3. 跳过白名单中的已知合理 CJK 字面量
/// 4. 报告所有未白名单化的硬编码 CJK 字面量
#[test]
fn no_hardcoded_cjk_in_panels() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let panels_dir = Path::new(manifest_dir).join("src").join("panels");
    let mut files = Vec::new();
    collect_rust_files(&panels_dir, &mut files);

    let mut violations: Vec<String> = Vec::new();

    for file in &files {
        let Ok(source) = fs::read_to_string(file) else {
            continue;
        };
        // 移除 `mod tests { ... }` 块(测试代码中的中文断言不需要 i18n)
        let no_tests = strip_test_modules(&source);
        // 移除注释,保留字符串字面量
        let stripped = strip_comments(&no_tests);
        // 提取含 CJK 的字符串字面量
        let cjk_literals = extract_cjk_string_literals(&stripped);

        let file_name = file
            .strip_prefix(manifest_dir)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");

        for (literal, line) in cjk_literals {
            // 检查白名单(永久允许的合理 CJK 字面量)
            let is_allowed = ALLOWED_CJK_LITERALS
                .iter()
                .any(|(suffix, allowed)| file_name.ends_with(suffix) && literal == *allowed);

            // 检查已知违规基线(现存历史遗留,待渐进修复)
            let is_known_violation = KNOWN_VIOLATIONS
                .iter()
                .any(|(suffix, known)| file_name.ends_with(suffix) && literal == *known);

            if !is_allowed && !is_known_violation {
                violations.push(format!("  - {file_name}:{line} — \"{literal}\""));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "发现 {} 处硬编码 CJK 字符串(应通过 t!() 国际化):\n{}\n\
         提示: 若为合理硬编码(如表名/分隔符),请添加到 ALLOWED_CJK_LITERALS 白名单",
        violations.len(),
        violations.join("\n")
    );
}

// ============================================================================
// 测试 3: SEED_KEYS 中的 key 在代码中被实际使用(无死 key)
// ============================================================================

/// 验证 `SEED_KEYS` 中声明的 key 在代码中被实际调用
///
/// WHY: `SEED_KEYS` 是 i18n 完整性的"种子清单",声明的 key 应在代码中
/// 通过 `t!()` 使用。未使用的 seed key 是死代码,可能是删除功能后
/// 遗留的孤儿 key,应清理以保持翻译表精简。
///
/// 注意: 本测试为 advisory(建议性),某些 key 可能通过动态构造调用
/// (如 `format!("panel.{}.title", name)`),这些 key 不会被静态扫描
/// 捕获。若动态调用模式导致误报,请将对应 key 添加到豁免列表。
#[test]
fn seed_keys_are_used_in_code() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let src_dir = Path::new(manifest_dir).join("src");
    let mut files = Vec::new();
    collect_rust_files(&src_dir, &mut files);

    // 收集代码中所有 t!()/tr() 调用的 key
    let mut used_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    for file in &files {
        let Ok(source) = fs::read_to_string(file) else {
            continue;
        };
        for (key, _) in extract_i18n_keys(&source) {
            used_keys.insert(key);
        }
    }

    // 已知通过动态构造调用的 key(静态扫描无法捕获),手动豁免
    //
    // WHY: 以下 key 通过 `t!(format!("panel.{}.title", name))` 等动态构造模式
    // 调用,静态扫描无法捕获。它们在 SEED_KEYS 中声明且在 zh/en 表中翻译,
    // 但在源码中没有直接的 `t!("key")` 字面量调用。若未来改为直接调用,
    // 可从豁免列表移除。
    let dynamically_constructed_keys: &[&str] = &[
        "app.name",
        "panel.task.title",
        "panel.chat.title",
        "panel.monitor.title",
        "panel.log.title",
        "panel.help.title",
        "status.mode",
        "status.view",
        "status.chat_pending",
        "mode.normal",
        "mode.insert",
        "mode.command",
        "action.agent.chat",
        "action.quest.pause",
        "action.overwindow.run",
        "action.export.run",
        "action.view.switch_layout",
        "action.system.toggle_locale",
        "action.monitor.pause_sampling",
        "action.monitor.time_window",
        "action.viz.switch_dimension",
        "action.view.apply_saved",
        "action.config.edit",
        "action.quest.jump",
        "hint.palette",
        "hint.help",
    ];

    let mut unused: Vec<&str> = Vec::new();
    for &seed_key in chimera_tui::i18n::zh::SEED_KEYS {
        if !used_keys.contains(seed_key) && !dynamically_constructed_keys.contains(&seed_key) {
            unused.push(seed_key);
        }
    }

    assert!(
        unused.is_empty(),
        "SEED_KEYS 中有 {} 个 key 未在代码中通过 t!()/tr() 使用(可能为死 key):\n  - {}\n\
         提示: 若为动态构造调用,请添加到 dynamically_constructed_keys 豁免列表",
        unused.len(),
        unused.join("\n  - ")
    );
}
