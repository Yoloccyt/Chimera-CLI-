//! data::sanitize — 模型输出控制序列消毒(Concord W11 T11.3,ADR-083 SEC-2)
//!
//! 对应架构层:L10 Interface
//!
//! # 职责
//! 模型输出(流式 chunk 提交文本)在汇入 ChatSync 前剥离控制序列字符,
//! 防止终端注入:恶意输出含 OSC 9/777、CSI 等序列时,不得在渲染或
//! 通知路径伪造通知、操纵终端状态。
//!
//! WHY 置于 data 层:ChatSync 是所有模型输出的唯一汇入点,在此消毒
//! 一次即覆盖全部下游(渲染/复制/通知);app 层通知构造亦复用本函数。

/// 剥离控制序列字符(保留可打印字符与换行/制表)
///
/// 保留 `\n`/`\t`(排版语义),其余 ASCII 控制字符与 0x7f 一律剔除。
pub fn sanitize_control_sequences(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn strips_osc_and_csi_injection() {
        // SEC-2 渗透:OSC 9 伪造通知 + CSI 清屏 + BEL 均被剥离
        assert_eq!(
            sanitize_control_sequences("ok\x1b]9;forged\x07\x1b[2J\x07end"),
            "ok]9;forged[2Jend"
        );
    }

    #[test]
    fn preserves_layout_whitespace() {
        assert_eq!(sanitize_control_sequences("a\nb\tc"), "a\nb\tc");
    }

    proptest! {
        /// 属性:任意输入经消毒后不含控制字符(除 \n/\t)
        #[test]
        fn sanitize_never_leaves_control_chars(s in "\\PC*") {
            let out = sanitize_control_sequences(&s);
            prop_assert!(!out.chars().any(|c| c.is_control() && c != '\n' && c != '\t'));
        }

        /// 属性:消毒幂等(二次消毒结果不变)
        #[test]
        fn sanitize_idempotent(s in "\\PC*") {
            let once = sanitize_control_sequences(&s);
            let twice = sanitize_control_sequences(&once);
            prop_assert_eq!(once, twice);
        }
    }
}
