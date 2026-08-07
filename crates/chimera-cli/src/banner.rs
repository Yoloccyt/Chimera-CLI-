//! 启动 banner 输出
//!
//! 提供 NEXUS-OMEGA CLI 启动时打印的 ASCII art 品牌彩条。
//! 默认走 stderr(避免污染 stdout 数据流,便于 `chimera --json ...` 等
//! 程序化消费场景)。通过 `--no-banner` 全局 flag 可关闭。
//!
//! WHY 独立模块:banner 是用户首次接触的视觉锚点,集中维护便于后续替换
//! ASCII art 设计(比如换 logo / 换标语),不必在 `main.rs` 散落 eprintln。

#![forbid(unsafe_code)]

/// 品牌 ASCII art(单行 ANSI 彩条 + 标题)
const BANNER: &str = r#"
╔══════════════════════════════════════════════════════════════╗
║  NEXUS-OMEGA AI Coding Agent  ·  全维稀疏架构的下一代编码代理  ║
╚══════════════════════════════════════════════════════════════╝
"#;

/// 输出启动 banner(到 stderr,避免污染 stdout 数据流)
///
/// 调用方应在 `--no-banner` flag 关闭时跳过本函数,避免不必要的 IO。
/// 本函数为纯副作用函数,无可观察返回值。
pub fn print() {
    eprintln!("{BANNER}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 断言 BANNER 常量非空,保证 `banner::print()` 至少有一行可输出。
    ///
    /// WHY 仅做存在性断言:本测试聚焦"常量在编译期仍包含有效字符串",
    /// 不锁定具体内容,允许后续品牌文案自由迭代。
    #[test]
    fn banner_constant_is_non_empty() {
        assert!(!BANNER.is_empty(), "BANNER 常量不应为空字符串");
        // 同时验证 BANNER 包含 NEXUS-OMEGA 关键字,防止误改为空字符串外的占位符
        assert!(
            BANNER.contains("NEXUS-OMEGA"),
            "BANNER 应包含品牌关键字 NEXUS-OMEGA"
        );
    }
}
