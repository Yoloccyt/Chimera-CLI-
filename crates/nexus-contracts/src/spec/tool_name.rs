//! 工具名统一解析契约 — 双 MCP 命名格式的单一解析入口（L0 纯契约）。
//!
//! # 背景（W1 / L2 工具层重构方案 §6.1，ADR-32X 待落档）
//!
//! 工具层存在两套 MCP 命名格式：
//! - 斜杠格式 `mcp:<server>/<tool>` — tool_plane（LLM 发布面，`tool_plane/mcp.rs`）；
//! - 点号格式 `mcp.<server>.<tool>` — 四环链（计划面，ADR-256 D1）。
//!
//! 本模块把「解析」收敛为单一纯函数：**双格式等价接受、canonical 单格式输出**
//! （重构方案裁决）。解析收敛 ≠ 格式重发布 —— 两种历史输入继续合法，新代码
//! 一律消费 [`canonical_name`] 的斜杠格式。
//!
//! # 层级纪律
//!
//! L0 不认识任何具体 provider 名单（echo/fs/mcp 的存在是 L10 装配事实），因此
//! 解析只做**结构分类**：MCP 前缀形态 → server/tool 分离；其余整体名原样透传
//! 为 [`ParsedToolName::Plain`]（provider 段的语义切分归 L10 消费方）。
//!
//! # 语义对齐（与既有实现逐字核对）
//!
//! server_id 白名单与 `chimera-cli/src/mcp_tool_executor.rs` 的 ADR-256 D1 规则
//! 一致：长度 1..=64、仅 `[a-zA-Z0-9_-]`；点号/斜杠均不在白名单内 —— 分别保证
//! 点号格式「首点切分」与斜杠格式「首斜杠切分」的解析无歧义。连续分隔符
//! （`mcp.a..t` / `mcp:a//t`）破坏消歧前提，统一归 [`ToolNameError::IllegalServerId`]
//! （点号格式的既有归类；斜杠格式为同款加固，合法输入不受影响）。

use std::fmt;

/// MCP 前缀 — 斜杠格式（canonical 输出格式；tool_plane/LLM 发布面既有惯例）
pub const MCP_COLON_PREFIX: &str = "mcp:";

/// MCP 前缀 — 点号格式（计划面历史格式；ADR-256 D1）
pub const MCP_DOT_PREFIX: &str = "mcp.";

/// server_id 白名单长度上界（ADR-256 D1：配置层 id 无校验，解析入口收紧）
pub const MAX_SERVER_ID_LEN: usize = 64;

/// 解析后的工具名 — 结构分类，不含 provider 语义（层级纪律见模块头）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedToolName {
    /// MCP 远程工具：server_id + tool 两段（两种输入格式归一到此变体）。
    Mcp {
        /// 服务器标识（白名单见 [`is_legal_server_id`]）。
        server_id: String,
        /// 工具名（点号/斜杠格式均取**首个分隔符之后全部内容**，允许内含分隔符，
        /// 如 `mcp.a.b.c` → server="a", tool="b.c"）。
        tool: String,
    },
    /// 非 MCP 工具：整体名原样保留（`text.echo` / `echo.echo` / `fs.read` 等；
    /// provider 段语义归 L10，本层不切分）。
    Plain {
        /// 原始工具名（未经裁剪）。
        name: String,
    },
}

impl fmt::Display for ParsedToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 与 canonical_name 同构：Display 是 canonical 的借用版（诊断日志面）。
        match self {
            Self::Mcp { server_id, tool } => write!(f, "mcp:{server_id}/{tool}"),
            Self::Plain { name } => f.write_str(name),
        }
    }
}

/// 工具名解析错误 — 仅 MCP 分支可失败（[`ParsedToolName::Plain`] 恒成功）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolNameError {
    /// `mcp:` / `mcp.` 前缀后为空（无 server 段）。
    #[error("empty server segment after mcp prefix: `{0}`")]
    EmptyServer(String),
    /// server_id 白名单外（含空段与**连续分隔符**——server 段边界歧义）。
    #[error("illegal server id in `{0}` (whitelist [a-zA-Z0-9_-], 1..={MAX_SERVER_ID_LEN})")]
    IllegalServerId(String),
    /// tool 段缺失（无分隔符）。
    #[error("missing tool segment: `{0}`")]
    MissingTool(String),
}

/// server_id 白名单校验：长度 1..=[`MAX_SERVER_ID_LEN`]，仅 `[a-zA-Z0-9_-]`。
///
/// 点号/斜杠都不在白名单内 —— 这是「首分隔符切分」无歧义的语义依据（ADR-256 D1
/// 既有裁决；斜杠格式沿用同一白名单，两类 server_id 合法集恒等）。
#[must_use]
pub fn is_legal_server_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SERVER_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 统一解析入口（纯函数；仅 MCP 分支可失败，其余原样透传）。
///
/// 两种 MCP 前缀（[`MCP_COLON_PREFIX`] / [`MCP_DOT_PREFIX`]）解析到同一
/// [`ParsedToolName::Mcp`] 变体 —— 调用方无需感知输入来自哪个面。
#[must_use]
pub fn parse_tool_name(raw: &str) -> Result<ParsedToolName, ToolNameError> {
    if let Some(rest) = raw.strip_prefix(MCP_COLON_PREFIX) {
        parse_mcp_body(rest, '/', raw)
    } else if let Some(rest) = raw.strip_prefix(MCP_DOT_PREFIX) {
        parse_mcp_body(rest, '.', raw)
    } else {
        Ok(ParsedToolName::Plain {
            name: raw.to_string(),
        })
    }
}

/// canonical 输出：MCP → `mcp:<server>/<tool>`（斜杠格式）；Plain → 原样。
///
/// WHY 斜杠格式为 canonical：LLM tools 表由 tool_plane 发布（`mcp:x/y` 已是
/// 既有 wire 形态），计划面在统一装配后经适配器归一到该格式（W2）。
#[must_use]
pub fn canonical_name(parsed: &ParsedToolName) -> String {
    match parsed {
        ParsedToolName::Mcp { server_id, tool } => format!("mcp:{server_id}/{tool}"),
        ParsedToolName::Plain { name } => name.clone(),
    }
}

/// MCP 前缀体解析（前缀已剥除）：按分隔符切 server/tool 并做白名单校验。
///
/// 语义（与 mcp_tool_executor 点号格式既有规则逐字对齐，斜杠格式同款加固）：
/// - 体为空 → [`ToolNameError::EmptyServer`]；
/// - 无分隔符 → [`ToolNameError::MissingTool`]；
/// - server 段白名单失败（含空段）→ [`ToolNameError::IllegalServerId`]；
/// - tool 段以分隔符开头（连续分隔符，边界歧义）→ [`ToolNameError::IllegalServerId`]；
/// - tool 段为空 → [`ToolNameError::MissingTool`]。
fn parse_mcp_body(rest: &str, sep: char, raw: &str) -> Result<ParsedToolName, ToolNameError> {
    if rest.is_empty() {
        return Err(ToolNameError::EmptyServer(raw.to_string()));
    }
    let Some((server, tool)) = rest.split_once(sep) else {
        return Err(ToolNameError::MissingTool(raw.to_string()));
    };
    if !is_legal_server_id(server) {
        return Err(ToolNameError::IllegalServerId(raw.to_string()));
    }
    if tool.is_empty() || tool.starts_with(sep) {
        // 空段与连续分隔符分开报：空段是「没写」，连续段是「歧义」——
        // 但两者都归 MissingTool 之外的既有语义位（连续段归 IllegalServerId，
        // 与点号格式 ADR-256 D1 的既有归类一致）。
        if tool.is_empty() {
            return Err(ToolNameError::MissingTool(raw.to_string()));
        }
        return Err(ToolNameError::IllegalServerId(raw.to_string()));
    }
    Ok(ParsedToolName::Mcp {
        server_id: server.to_string(),
        tool: tool.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // proptest 断言/假设宏(prop_assert_eq/prop_assume)经 prelude 导入 ——
    // proptest! 宏展开体在调用方作用域解析宏,模块内测试须显式引入。
    use proptest::prelude::*;

    // === 结构分类 ===

    #[test]
    fn plain_names_pass_through_unchanged() {
        // 四环链 builtin 与 tool_plane provider 命名形态全谱（含空串：
        // 空名是结构事实，分类为 Plain，语义拒绝归消费方）。
        for name in ["text.echo", "echo.echo", "fs.read", "proc.exec", ""] {
            assert_eq!(
                parse_tool_name(name),
                Ok(ParsedToolName::Plain {
                    name: name.to_string()
                }
                .clone()),
                "Plain 透传失败: {name}"
            );
        }
    }

    // === 双格式等价 ===

    #[test]
    fn both_mcp_formats_parse_to_same_variant() {
        let dotted = parse_tool_name("mcp.cai.list_issues").unwrap();
        let slashed = parse_tool_name("mcp:cai/list_issues").unwrap();
        assert_eq!(dotted, slashed, "双格式必须归一到同一变体");
        assert_eq!(
            dotted,
            ParsedToolName::Mcp {
                server_id: "cai".to_string(),
                tool: "list_issues".to_string()
            }
        );
    }

    #[test]
    fn canonical_output_is_slash_format() {
        let parsed = parse_tool_name("mcp.cai.list_issues").unwrap();
        assert_eq!(canonical_name(&parsed), "mcp:cai/list_issues");
        // Display 与 canonical 同构
        assert_eq!(parsed.to_string(), "mcp:cai/list_issues");
    }

    #[test]
    fn tool_segment_may_contain_separators() {
        // 首分隔符切分：tool 段允许内含点号（ADR-256 D1 既有语义）与斜杠
        let dotted = parse_tool_name("mcp.a.b.c").unwrap();
        assert_eq!(
            dotted,
            ParsedToolName::Mcp {
                server_id: "a".to_string(),
                tool: "b.c".to_string()
            }
        );
        let slashed = parse_tool_name("mcp:a/b/c").unwrap();
        assert_eq!(
            slashed,
            ParsedToolName::Mcp {
                server_id: "a".to_string(),
                tool: "b/c".to_string()
            }
        );
    }

    // === 白名单边界 ===

    #[test]
    fn server_id_length_boundary() {
        let ok = "a".repeat(MAX_SERVER_ID_LEN);
        assert!(is_legal_server_id(&ok));
        let too_long = "a".repeat(MAX_SERVER_ID_LEN + 1);
        assert!(!is_legal_server_id(&too_long));
        // 经完整解析同样拒绝
        let raw = format!("mcp.{too_long}.t");
        assert!(matches!(
            parse_tool_name(&raw),
            Err(ToolNameError::IllegalServerId(_))
        ));
    }

    #[test]
    fn server_id_character_whitelist() {
        for bad in ["a.b", "a/b", "a b", "服务", "a!", ""] {
            assert!(!is_legal_server_id(bad), "白名单外字符应拒绝: {bad}");
        }
        for good in ["a", "A", "0", "_", "-", "srv-1_2"] {
            assert!(is_legal_server_id(good), "白名单内应通过: {good}");
        }
    }

    // === 错误归类（与 ADR-256 D1 既有归类逐字对齐） ===

    #[test]
    fn empty_server_forms() {
        // 前缀体全空
        assert!(matches!(
            parse_tool_name("mcp."),
            Err(ToolNameError::EmptyServer(_))
        ));
        assert!(matches!(
            parse_tool_name("mcp:"),
            Err(ToolNameError::EmptyServer(_))
        ));
    }

    #[test]
    fn missing_tool_forms() {
        // 无分隔符（只有 server 段）与空 tool 段
        assert!(matches!(
            parse_tool_name("mcp.a"),
            Err(ToolNameError::MissingTool(_))
        ));
        assert!(matches!(
            parse_tool_name("mcp:a"),
            Err(ToolNameError::MissingTool(_))
        ));
        assert!(matches!(
            parse_tool_name("mcp.a."),
            Err(ToolNameError::MissingTool(_))
        ));
    }

    #[test]
    fn consecutive_separator_is_illegal_server_id() {
        // 连续点号（边界歧义）—— ADR-256 D1 既有归类
        assert!(matches!(
            parse_tool_name("mcp.a..t"),
            Err(ToolNameError::IllegalServerId(_))
        ));
        // 连续斜杠 —— 斜杠格式同款加固
        assert!(matches!(
            parse_tool_name("mcp:a//t"),
            Err(ToolNameError::IllegalServerId(_))
        ));
        // 空 server 段（首分隔符前为空）
        assert!(matches!(
            parse_tool_name("mcp..x"),
            Err(ToolNameError::IllegalServerId(_))
        ));
        assert!(matches!(
            parse_tool_name("mcp:/x"),
            Err(ToolNameError::IllegalServerId(_))
        ));
    }

    // === proptest 属性（block-named 语法,proptest 1.11+ 纪律） ===
    //
    // 坑位记录（承 proptest 四连坑）：proptest! 宏按顶层逗号分割「策略对」，
    // 策略表达式内的闭包（prop_filter 的参数列表）会被当成分界 → 编译错。
    // 规避：不用 prop_filter，改用测试体内的 `prop_assume!` 语句级过滤。

    proptest::proptest! {
        /// 双格式等价 + canonical 回环：任意合法 (server, tool) 经两种格式解析
        /// 必须得到同一变体,且 canonical 输出恰为斜杠格式。
        #[test]
        fn dual_format_equivalence_roundtrip(
            server in "[a-zA-Z0-9_-]{1,32}",
            tool in "[a-zA-Z0-9_-]{1,32}",
        ) {
            let dotted = format!("mcp.{server}.{tool}");
            let slashed = format!("mcp:{server}/{tool}");
            let a = parse_tool_name(&dotted).unwrap_or_else(|e| panic!("点号格式应合法: {e}"));
            let b = parse_tool_name(&slashed).unwrap_or_else(|e| panic!("斜杠格式应合法: {e}"));
            // 比较用引用:prop_assert_eq! 展开会把左值 move 进格式化,先比后用。
            prop_assert_eq!(&a, &b);
            prop_assert_eq!(canonical_name(&a), slashed);
        }

        /// Plain 透传不变量：不含 MCP 前缀的任意串原样返回（解析不改变非 MCP 名）。
        #[test]
        fn plain_passthrough_invariant(name in proptest::prelude::any::<String>()) {
            prop_assume!(!name.starts_with("mcp:") && !name.starts_with("mcp."));
            prop_assert_eq!(
                parse_tool_name(&name).unwrap_or_else(|e| panic!("Plain 分支恒成功: {e}")),
                ParsedToolName::Plain { name }
            );
        }
    }
}
