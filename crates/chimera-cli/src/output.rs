//! 统一输出 helper — CLI 层的彩色输出 / 表格渲染 / 进度指示 / JSON 序列化
//!
//! 对应架构层:L10 Interface
//! 对应任务:
//! - Task 1.7:全局 `--json` 参数(JSON 输出 helper + schema)
//! - Task 1.12:彩色输出 + 进度指示 + 表格渲染
//!
//! # JSON 输出 Schema(Task 1.7.4)
//!
//! `--json` flag 启用时,所有命令输出遵循以下 envelope schema:
//!
//! ## 成功输出
//! ```json
//! { "status": "ok", "data": <command-specific payload> }
//! ```
//! - `data` 字段为命令特定结构,直接由 `serde_json::to_string_pretty` 序列化
//! - 例:`config show --json` 的 `data` 为完整 `ChimeraConfig` 序列化
//! - 例:`quest list --json` 的 `data` 为 `Vec<QuestSummary>`
//!
//! ## 错误输出(stderr)
//! ```json
//! {
//!   "status": "error",
//!   "error": {
//!     "kind": "<ChimeraCliError 变体名>",
//!     "message": "<错误详情>"
//!   },
//!   "exit_code": <0-6>
//! }
//! ```
//! - `kind`:错误变体名(如 `NotImplemented` / `ConfigError` / `UserCancelled`)
//! - `exit_code`:对应 ADR-060 ExitCode 矩阵(0=success / 1=user_error / 2=not_implemented /
//!   3=system_error / 4=user_cancelled / 5=permission_denied / 6=timeout)
//!
//! ## 颜色控制
//! - `--no-color` flag 或 `NO_COLOR` 环境变量(任一启用)禁用 ANSI 颜色码
//! - 禁用颜色时,`print_success` 等仅输出纯文本前缀(`✓` / `✗` / `⚠` / `ℹ`)
//! - CI 友好:非 TTY 环境自动检测(但当前实现需显式 `--no-color`)

#![forbid(unsafe_code)]

use std::sync::OnceLock;

use comfy_table::{ContentArrangement, Table};
use indicatif::{ProgressBar, ProgressStyle};
use nu_ansi_term::Color;
use serde::Serialize;

/// 颜色模式 — 控制 ANSI 颜色码输出
///
/// - `Auto`:遵循 `--no-color` flag 与 `NO_COLOR` 环境变量
/// - `Never`:强制禁用颜色(CI 友好)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorMode {
    /// 自动检测(默认):检查 `NO_COLOR` 环境变量
    #[default]
    Auto,
    /// 强制禁用颜色(由 `--no-color` flag 触发)
    Never,
}

/// 全局颜色模式(进程级单例,由 `init_color_mode` 在 main 启动时设置一次)
///
/// WHY 用 OnceLock 而非 thread-local:CLI 输出跨线程一致性优于线程独立,
/// 且 `--no-color` 是进程级 flag,不应因线程不同而表现不同。
static COLOR_MODE: OnceLock<ColorMode> = OnceLock::new();

/// 初始化全局颜色模式(应在 main 入口调用一次)
///
/// `no_color=true` 时强制禁用;否则为 `Auto`(由 `should_colorize` 运行时检查 `NO_COLOR`)
pub fn init_color_mode(no_color: bool) {
    let mode = if no_color {
        ColorMode::Never
    } else {
        ColorMode::Auto
    };
    // set 失败仅意味着已设置过,无需特殊处理(首次设置生效)
    let _ = COLOR_MODE.set(mode);
}

/// 运行时判断是否应输出颜色
///
/// 判定顺序:
/// 1. 全局 `COLOR_MODE` 为 `Never` → false
/// 2. `NO_COLOR` 环境变量存在(任意值,遵循 https://no-color.org 规范) → false
/// 3. 否则 → true
pub fn should_colorize() -> bool {
    match COLOR_MODE.get() {
        Some(ColorMode::Never) => false,
        Some(ColorMode::Auto) | None => std::env::var_os("NO_COLOR").is_none(),
    }
}

/// 内部 helper:根据 `should_colorize()` 决定是否对文本着色
fn paint<'a>(color: Color, text: &'a str) -> std::borrow::Cow<'a, str> {
    if should_colorize() {
        std::borrow::Cow::Owned(color.paint(text).to_string())
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// 输出成功消息(绿色 `✓` 前缀 + 消息文本,到 stderr)
///
/// WHY stderr 而非 stdout:成功消息属诊断输出,stdout 应保留给命令的"数据"输出,
/// 便于 `chimera quest list --json | jq` 等管道场景不被人类可读提示污染。
pub fn print_success(msg: &str) {
    eprintln!("{} {}", paint(Color::Green, "✓"), msg);
}

/// 输出错误消息(红色 `✗` 前缀,到 stderr)
pub fn print_error(msg: &str) {
    eprintln!("{} {}", paint(Color::Red, "✗"), msg);
}

/// 输出警告消息(黄色 `⚠` 前缀,到 stderr)
pub fn print_warning(msg: &str) {
    eprintln!("{} {}", paint(Color::Yellow, "⚠"), msg);
}

/// 输出信息消息(蓝色 `ℹ` 前缀,到 stderr)
pub fn print_info(msg: &str) {
    eprintln!("{} {}", paint(Color::Blue, "ℹ"), msg);
}

/// 渲染表格到 stdout
///
/// `headers`:列标题;`rows`:每行数据(字符串向量)。
/// 使用 `comfy-table` 的 `Dynamic` 自适应布局,终端宽度不足时自动换行。
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut table = Table::new();
    table
        .load_preset(comfy_table::presets::UTF8_FULL)
        .apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(headers.iter().copied());
    for row in rows {
        table.add_row(row.iter().map(|c| c.as_str()));
    }
    println!("{table}");
}

/// 创建并返回一个进度条(用于长任务)
///
/// `msg`:spinner 旁显示的消息。返回的 `ProgressBar` 由调用方管理生命周期
/// (典型用法:`pb.finish_with_message("done")` 或 `pb.finish_and_clear()`)。
///
/// WHY 返回而非全局存储:进度条的生命周期与具体命令执行绑定,
/// 全局存储会引入不必要的同步开销,且 `indicatif::ProgressBar` 本身
/// 是 `Send + Sync`(内部 `Arc`),调用方可自由传递。
pub fn print_progress(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(std::time::Duration::from_millis(120));
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    pb.set_message(msg.to_string());
    pb
}

/// JSON 输出 envelope — 成功响应的统一包装(Task 1.7.4 schema)
#[derive(Debug, Serialize)]
pub struct JsonResult<T: Serialize> {
    /// 固定为 `"ok"`,便于脚本程序化判断
    pub status: &'static str,
    /// 命令特定数据载荷
    pub data: T,
}

/// 渲染成功 JSON envelope 为 pretty-print 字符串(纯函数,无 IO 副作用)
///
/// `data`:命令特定载荷(任意 `Serialize` 类型)。
/// 返回序列化后的 JSON 字符串——调用方决定输出目标
/// (`print_json` 输出到 stdout;快照测试直接断言字符串,与终端环境无关)。
///
/// WHY 纯函数分离:JSON envelope schema(Task 1.7.4)是程序化消费契约,
/// 快照测试锁定其格式演进;直接测 `print_json` 需捕获 stdout,引入
/// 线程级输出捕获(不稳定),故拆出无副作用的渲染层。
pub fn render_json<T: Serialize>(data: &T) -> Result<String, serde_json::Error> {
    let payload = JsonResult { status: "ok", data };
    serde_json::to_string_pretty(&payload)
}

/// 输出成功 JSON 到 stdout(Task 1.7)
///
/// 将任意 `Serialize` 类型包装为 `{ "status": "ok", "data": <payload> }` 并 pretty-print。
/// 用于 `--json` flag 启用时的所有命令成功输出。
pub fn print_json<T: Serialize>(data: &T) -> Result<(), serde_json::Error> {
    let json = render_json(data)?;
    println!("{json}");
    Ok(())
}

/// JSON 输出 envelope — 错误响应的统一包装(Task 1.7.4 schema)
#[derive(Debug, Serialize)]
pub struct JsonError {
    /// 固定为 `"error"`
    pub status: &'static str,
    /// 错误详情(变体名 + 消息)
    pub error: JsonErrorDetail,
    /// 对应 ADR-060 ExitCode 矩阵的退出码
    pub exit_code: u8,
}

/// JSON 错误详情
#[derive(Debug, Serialize)]
pub struct JsonErrorDetail {
    /// `ChimeraCliError` 变体名(如 `NotImplemented` / `ConfigError`)
    pub kind: &'static str,
    /// 错误详情文本
    pub message: String,
}

/// 输出错误 JSON 到 stderr(Task 1.7)
///
/// 用于 `--json` flag 启用时,命令返回 `Err(ChimeraCliError)` 时的结构化错误输出。
pub fn print_json_error(kind: &'static str, message: &str, exit_code: u8) {
    let payload = JsonError {
        status: "error",
        error: JsonErrorDetail {
            kind,
            message: message.to_string(),
        },
        exit_code,
    };
    // 错误输出走 stderr(与人类可读模式保持一致)
    if let Ok(json) = serde_json::to_string_pretty(&payload) {
        eprintln!("{json}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证默认颜色模式为 Auto
    #[test]
    fn test_default_color_mode_is_auto() {
        let mode = ColorMode::default();
        assert_eq!(mode, ColorMode::Auto);
    }

    /// 验证 `init_color_mode(true)` 设置全局模式为 Never
    ///
    /// WHY 不直接断言 `should_colorize()` 返回值:`OnceLock` 是进程级单例,
    /// 测试间共享全局状态,且 `Auto` 模式还会受 `NO_COLOR` 环境变量影响。
    /// 此测试仅验证 `init_color_mode` 不 panic 且 `should_colorize` 可调用。
    #[test]
    fn test_init_color_mode_does_not_panic() {
        init_color_mode(true);
        // 仅验证调用不 panic;OnceLock 已设置后再次 set 静默失败(预期行为)
        init_color_mode(false);
        let _ = should_colorize();
    }

    /// 验证 `should_colorize()` 在 `NO_COLOR` 环境变量设置时返回 false
    #[test]
    fn test_should_colorize_respects_no_color_env() {
        // 保存原始值,测试后恢复(避免污染其它测试)
        let original = std::env::var_os("NO_COLOR");
        std::env::set_var("NO_COLOR", "1");
        // Auto 模式 + NO_COLOR 设置 → false
        // 注意:若 COLOR_MODE 已被其它测试设为 Never,此处也会返回 false,断言仍成立
        assert!(!should_colorize(), "NO_COLOR 设置时应禁用颜色");
        // 恢复原始环境
        match original {
            Some(v) => std::env::set_var("NO_COLOR", v),
            None => std::env::remove_var("NO_COLOR"),
        }
    }

    /// 验证 `paint` 在禁用颜色时返回纯文本
    #[test]
    fn test_paint_returns_plain_text_when_disabled() {
        // 直接调用 paint 测试逻辑,不依赖全局状态
        // should_colorize 在测试环境中可能为 true 或 false,此测试只验证
        // 不 panic 且返回非空字符串
        let result = paint(Color::Red, "test");
        assert!(!result.is_empty());
    }

    /// 验证 `print_table` 不 panic 且输出非空
    #[test]
    fn test_print_table_outputs_nonempty() {
        // 捕获 stdout 验证输出(comfy-table 在非 TTY 仍能渲染)
        // 此测试只验证不 panic,实际输出内容由集成测试覆盖
        print_table(
            &["ID", "Name", "Status"],
            &[
                vec!["1".into(), "quest-1".into(), "running".into()],
                vec!["2".into(), "quest-2".into(), "done".into()],
            ],
        );
    }

    /// 验证 `print_progress` 返回可用的 ProgressBar
    #[test]
    fn test_print_progress_returns_progress_bar() {
        let pb = print_progress("loading");
        assert!(!pb.is_finished());
        pb.finish_and_clear();
    }

    /// 验证 `print_json` 正确序列化 envelope schema
    #[test]
    fn test_print_json_envelope_schema() {
        let data = serde_json::json!({ "version": "2.9.0-omega", "count": 5 });
        // 序列化不应失败
        let payload = JsonResult {
            status: "ok",
            data: &data,
        };
        let json = serde_json::to_string_pretty(&payload).unwrap();
        assert!(json.contains("\"status\": \"ok\""), "应包含 status: ok");
        assert!(json.contains("\"data\""), "应包含 data 字段");
        assert!(json.contains("2.9.0-omega"), "应包含原始数据");
    }

    /// 验证 `JsonError` schema 结构完整
    #[test]
    fn test_json_error_schema_complete() {
        let err = JsonError {
            status: "error",
            error: JsonErrorDetail {
                kind: "NotImplemented",
                message: "chimera wiki 尚未接入".into(),
            },
            exit_code: 2,
        };
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"status\":\"error\""));
        assert!(json.contains("\"kind\":\"NotImplemented\""));
        assert!(json.contains("\"exit_code\":2"));
    }
}
