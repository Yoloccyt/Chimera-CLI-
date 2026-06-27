//! CHTC 属性测试 — 协议转换不变量验证
//!
//! 验证两个核心不变量:
//! 1. 任何 ide_source 经转换后字段完整(tool_id/parameters/ide_source/call_id 非空)
//! 2. to_native_format(from_*_format(raw)) 保持 tool_id 一致

#![forbid(unsafe_code)]

use chtc_bridge::{IdeSource, ProtocolConverter};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量:5 种 IDE 协议转换字段完整 + round-trip tool_id 一致 + deadline 一致
    #[test]
    fn test_protocol_invariants(x in 0..100u32) {
        // === 不变量1:VSCode 转换后字段完整 ===
        let raw = serde_json::json!({ "command": format!("cmd-{}", x), "args": { "n": x } });
        let call = ProtocolConverter::from_vscode_format(raw).expect("vscode convert");
        prop_assert!(!call.tool_id.is_empty(), "tool_id 非空");
        prop_assert!(!call.call_id.is_empty(), "call_id 非空");
        prop_assert_eq!(call.ide_source.clone(), IdeSource::vscode());

        // === 不变量2:to_native_format(from_*) 保持 tool_id 一致 ===
        let native = ProtocolConverter::to_native_format(&call);
        prop_assert_eq!(native["command"].as_str(), Some(call.tool_id.as_str()));

        // === IntelliJ:字段完整 + round-trip ===
        let ij_raw = serde_json::json!({ "action": format!("a-{}", x), "params": { "p": x } });
        let ij = ProtocolConverter::from_intellij_format(ij_raw).expect("intellij");
        prop_assert!(!ij.tool_id.is_empty());
        prop_assert!(!ij.call_id.is_empty());
        let ij_native = ProtocolConverter::to_native_format(&ij);
        prop_assert_eq!(ij_native["action"].as_str(), Some(ij.tool_id.as_str()));

        // === Vim:字段完整 + round-trip ===
        let vm_raw = serde_json::json!({ "cmd": format!("c-{}", x), "args": [x] });
        let vm = ProtocolConverter::from_vim_format(vm_raw).expect("vim");
        prop_assert!(!vm.tool_id.is_empty());
        prop_assert!(!vm.call_id.is_empty());
        let vm_native = ProtocolConverter::to_native_format(&vm);
        prop_assert_eq!(vm_native["cmd"].as_str(), Some(vm.tool_id.as_str()));

        // === Emacs:字段完整 + round-trip ===
        let em_raw = serde_json::json!({ "sexp": format!("(s {})", x), "buffer": "buf" });
        let em = ProtocolConverter::from_emacs_format(em_raw).expect("emacs");
        prop_assert!(!em.tool_id.is_empty());
        prop_assert!(!em.call_id.is_empty());
        let em_native = ProtocolConverter::to_native_format(&em);
        prop_assert_eq!(em_native["sexp"].as_str(), Some(em.tool_id.as_str()));

        // === Zed:字段完整 + round-trip ===
        let zd_raw = serde_json::json!({ "action": format!("z-{}", x), "data": { "d": x } });
        let zd = ProtocolConverter::from_zed_format(zd_raw).expect("zed");
        prop_assert!(!zd.tool_id.is_empty());
        prop_assert!(!zd.call_id.is_empty());
        let zd_native = ProtocolConverter::to_native_format(&zd);
        prop_assert_eq!(zd_native["action"].as_str(), Some(zd.tool_id.as_str()));

        // === deadline_ms 与默认值一致 ===
        prop_assert_eq!(call.deadline_ms, chtc_bridge::protocol::DEFAULT_DEADLINE_MS);
    }
}
