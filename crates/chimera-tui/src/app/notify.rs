//! app::notify — NotifyBridge 桌面通知桥(Concord W11 T11.3,ADR-083)
//!
//! 对应架构层:L10 Interface
//!
//! # 职责
//! - **OSC 9 opt-in 通知**(对齐 Codex TUI 通知语义):终端失焦/长任务完成时
//!   经 OSC 9 序列推送桌面通知;默认关闭,`/notify on|off` 切换并持久化;
//! - **能力探测**:`TERM_PROGRAM` 白名单判定终端是否支持 OSC 9
//!   (Warp/iTerm2 系),不支持则静默降级(不写序列);
//! - **SEC-2 消毒**:所有外发通知体与模型输出经 [`sanitize_control_sequences`]
//!   剥离控制序列——恶意模型输出含 OSC 序列时不得伪造通知或操纵终端。
//!
//! # 触发源白名单(方案 §6.5)
//! quest 完成/失败快照跃迁;Critical 旁路事件的只读观察延伸(不拦截 mpsc)。
//! 本模块只产出"应发什么"(纯函数),发送决策与写出在事件循环收口。

// SEC-2 消毒函数单一事实源在 data 层(ChatSync 汇入点复用)
pub use crate::data::sanitize::sanitize_control_sequences;

/// OSC 9 通知体构造:BEL 终止的 OSC 9 序列(纯函数)
///
/// # 安全
/// body/title 先经 [`sanitize_control_sequences`] 消毒——即使调用方传入
/// 含 ESC 的文本,也无法注入第二条控制序列(SEC-2)。
pub fn build_osc9_notification(title: &str, body: &str) -> String {
    let clean_title = sanitize_control_sequences(title);
    let clean_body = sanitize_control_sequences(body);
    let payload = if clean_title.is_empty() {
        clean_body
    } else if clean_body.is_empty() {
        clean_title
    } else {
        format!("{clean_title}: {clean_body}")
    };
    format!("\x1b]9;{payload}\x07")
}

/// TERM_PROGRAM 是否支持 OSC 9(纯函数,参数注入便于测试)
///
/// 白名单:Warp / iTerm / WezTerm / mintty / ConEmu;未知终端返回 false
/// (静默降级,不写序列——避免不支持的终端打印乱码)。
pub fn term_program_supports_osc9(term_program: Option<&str>) -> bool {
    let Some(program) = term_program else {
        return false;
    };
    let lower = program.to_ascii_lowercase();
    [
        "warp",
        "iterm",
        "wezterm",
        "mintty",
        "conemu",
        "windows terminal",
        "windowsterminal",
    ]
    .iter()
    .any(|supported| lower.contains(supported))
}

/// 从进程环境探测 OSC 9 能力(生产入口)
pub fn detect_osc9_capability() -> bool {
    term_program_supports_osc9(std::env::var("TERM_PROGRAM").ok().as_deref())
}

/// 按开关与能力发送桌面通知(触发源白名单的出口,事件循环/状态同步调用)
///
/// 返回 `true` = 实际写出。开关关闭或终端不支持时静默降级(opt-in 语义)。
pub fn maybe_notify(enabled: bool, capable: bool, title: &str, body: &str) -> bool {
    if !(enabled && capable) {
        return false;
    }
    let seq = build_osc9_notification(title, body);
    use std::io::Write;
    std::io::stdout()
        .write_all(seq.as_bytes())
        .and_then(|_| std::io::stdout().flush())
        .is_ok()
}

/// Base64 编码(标准字母表,含填充;OSC 52 剪贴板序列载荷用)
///
/// WHY 内联实现:仅 OSC 52 一处使用,不值得引入 base64 依赖;纯函数可测。
pub fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn osc9_wraps_payload_with_bel_terminator() {
        let seq = build_osc9_notification("Quest 完成", "任务结束");
        assert!(seq.starts_with("\x1b]9;"), "OSC 9 起始序列");
        assert!(seq.ends_with('\x07'), "BEL 终止");
        assert!(seq.contains("Quest 完成: 任务结束"));
    }

    #[test]
    fn osc9_sanitizes_injection_attempts() {
        // SEC-2 渗透:模型输出含 OSC 序列不得产生第二条控制序列
        let malicious = "done\x1b]9;forged\x07\x1b[2J";
        let seq = build_osc9_notification("t", malicious);
        let inner = &seq[4..seq.len() - 1]; // 去掉 \x1b]9; 与 \x07
        assert!(!inner.contains('\x1b'), "消毒后不得残留 ESC");
        assert!(!inner.contains('\x07'), "消毒后不得残留 BEL");
        assert!(inner.contains("forged"), "正文内容保留(仅剥离控制字符)");
    }

    #[test]
    fn sanitize_preserves_newlines_and_tabs() {
        assert_eq!(
            sanitize_control_sequences("line1\nline2\tcol\x1b[31mred"),
            "line1\nline2\tcol[31mred"
        );
    }

    #[test]
    fn term_program_whitelist() {
        assert!(term_program_supports_osc9(Some("WezTerm")));
        assert!(term_program_supports_osc9(Some("iTerm.app")));
        assert!(term_program_supports_osc9(Some("WarpTerminal")));
        assert!(term_program_supports_osc9(Some("Windows Terminal")));
        assert!(!term_program_supports_osc9(Some("xterm-256color")));
        assert!(!term_program_supports_osc9(Some("")));
        assert!(!term_program_supports_osc9(None));
    }

    #[test]
    fn maybe_notify_respects_gate() {
        // 开关关闭或能力缺失 → 不写出(返回 false,静默降级)
        assert!(!maybe_notify(false, true, "t", "b"), "开关关闭不发送");
        assert!(!maybe_notify(true, false, "t", "b"), "终端不支持不发送");
        assert!(!maybe_notify(false, false, "t", "b"));
        // 双条件满足时实际写出(stdout 可写,返回 true)
        assert!(maybe_notify(true, true, "t", "b"), "双条件满足应写出");
    }

    #[test]
    fn base64_rfc4648_vectors() {
        // RFC 4648 标准测试向量 + 填充边界
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    proptest! {
        /// 属性:OSC 9 构造恒以 \x1b]9; 开头、\x07 结尾,且内部无残留控制符
        #[test]
        fn osc9_shape_invariant(title in "\\PC{0,20}", body in "\\PC{0,40}") {
            let seq = build_osc9_notification(&title, &body);
            prop_assert!(seq.starts_with("\x1b]9;"));
            prop_assert!(seq.ends_with('\x07'));
            let inner = &seq[4..seq.len() - 1];
            prop_assert!(!inner.chars().any(|c| c.is_control()));
        }
    }
}
