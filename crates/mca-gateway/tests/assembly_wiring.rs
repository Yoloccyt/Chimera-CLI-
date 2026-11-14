//! R12 W1-1 装配内核接线集成测试 — 默认关闭 / 逐字节等价 / fail-open
//!
//! 对应计划:R12 W1-1「把上下文装配内核接进生产路径(默认关闭,逐字节等价旧路径)」
//!
//! 覆盖:
//! 1. [`wire_off_equals_legacy`]      关态输出与改动前纯裁剪**逐字节**相等(含 JSON 字节)
//! 2. [`wire_off_keeps_duplicates`]   关态**不去重**(证明"零行为变更",不是"顺手改了")
//! 3. [`wire_on_dedups_duplicates`]   开态把 2 条完全相同的消息收敛为 1 条,且输出为输入子序列
//! 4. [`wire_on_never_touches_pinned`] 旧路径恒保留档(工具返回/系统提示)不被装配移除
//! 5. [`wire_on_preserves_block_structure`] 去重不重建消息 ⇒ Thinking/ToolUse 结构逐字节保留(P4)
//! 6. [`invalid_env_falls_back_to_off`] 非法开关值 ⇒ 等价 off
//! 7. proptest:任意消息序列 ⇒ 输出是输入子序列、不含重复项、长度不增
//!
//! WHY 不用 `set_var` 驱动环境变量:进程环境是全局可变状态,`cargo test`
//! 在同一进程内多线程并行执行,`set_var` 会与其它测试互相污染;而生产侧
//! `assembly_wiring()` 用 `OnceLock` 缓存首读(热路径红线),第二次读恒返回
//! 首值,使"设了再读"的测试本质上不可靠。故本文件分两层断言:
//! - 环境解析 = 纯函数 [`parse_assembly_wiring`](`Option<&str>` → 开关);
//! - 分派行为 = 纯函数 [`trim_to_budget_with`](显式传入开关)。
//!
//! 二者组合覆盖了"未设 ⇒ off"与"非法值 ⇒ off",且不触碰进程环境。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use mca_gateway::conversation_trim::{
    parse_assembly_wiring, trim_to_budget, trim_to_budget_legacy, trim_to_budget_with,
    AssemblyWiring, ASSEMBLY_WIRED_ENV,
};
use nexus_contracts::affinity::{AffinityMessage, ContentBlock, MessageRole};

// ============================================================
// 共享夹具
// ============================================================

/// 构造单文本块消息
fn msg(role: MessageRole, text: &str) -> AffinityMessage {
    AffinityMessage {
        role,
        blocks: vec![ContentBlock::Text { text: text.into() }],
    }
}

/// 构造超预算用的长文本消息(200 字符 ≈ 50 token,足以压出裁剪)
fn long_msg(role: MessageRole, prefix: &str) -> AffinityMessage {
    msg(role, &format!("{prefix} {}", "x".repeat(200)))
}

/// 与 conversation_trim 单测 `trim_preserves_original_relative_order` **同形**的
/// 100 字符消息 —— 用于复用该单测已锁定的期望结果作独立金标准
fn unit_msg(role: MessageRole, prefix: &str) -> AffinityMessage {
    msg(role, &format!("{prefix} {}", "x".repeat(100)))
}

/// 贪心匹配:`out` 每条消息在 `input` 中自上次匹配位置之后的首个相等项处匹配,
/// 返回命中下标序列;任一条无法匹配 ⇒ `None`(即 `out` 不是 `input` 的子序列)。
///
/// WHY 贪心即可:子序列判定只需"存在一组严格递增的命中下标",贪心取最左命中
/// 不会比任何其它选择更差(标准结论)。
fn matched_indices(out: &[AffinityMessage], input: &[AffinityMessage]) -> Option<Vec<usize>> {
    let mut indices = Vec::with_capacity(out.len());
    let mut cursor = 0usize;
    for message in out {
        let offset = input[cursor..]
            .iter()
            .position(|candidate| candidate == message)?;
        let index = cursor + offset;
        indices.push(index);
        cursor = index + 1;
    }
    Some(indices)
}

/// 是否为子序列(逐条内容相等)
fn is_subsequence(out: &[AffinityMessage], input: &[AffinityMessage]) -> bool {
    matched_indices(out, input).is_some()
}

/// 输出中是否有重复消息(同角色 + 同块序列计为重复)
fn has_duplicates(messages: &[AffinityMessage]) -> bool {
    messages
        .iter()
        .enumerate()
        .any(|(i, a)| messages[i + 1..].iter().any(|b| a == b))
}

/// 输出中**指定角色**是否有重复消息（R12 W1-1 守卫用）
///
/// WHY 需要按角色分：去重只对 `Tool` 生效（ADR-298 D2），因此"开态无重复"这条性质
/// 只能对 Tool 断言；对全部角色断言会把"对话轮次必须保留重复"这条正确行为判成失败
/// （首轮实测正是如此：3 条用例因全角色断言而假红）。
fn has_duplicates_for_role(messages: &[AffinityMessage], role: MessageRole) -> bool {
    let of_role: Vec<&AffinityMessage> = messages.iter().filter(|m| m.role == role).collect();
    of_role
        .iter()
        .enumerate()
        .any(|(i, a)| of_role[i + 1..].iter().any(|b| a == b))
}

/// 序列化后逐字节比较用 — "逐字节等价"的可执行判据
fn json_bytes(messages: &[AffinityMessage]) -> String {
    serde_json::to_string(messages).expect("AffinityMessage 必须可序列化")
}

/// 单条消息的序列化字节(块结构守恒断言用)
fn json_one(message: &AffinityMessage) -> String {
    serde_json::to_string(message).expect("AffinityMessage 必须可序列化")
}

// ============================================================
// 1. 关态逐字节等价旧路径
// ============================================================

#[test]
fn wire_off_equals_legacy() {
    // 与 conversation_trim 单测 trim_preserves_original_relative_order 同输入同预算,
    // 该用例的期望结果([Tool, User, Assistant])已被既有单测锁定 ⇒ 可作独立金标准。
    let input = vec![
        unit_msg(MessageRole::User, "first user"),
        unit_msg(MessageRole::Assistant, "assistant turn"),
        msg(MessageRole::Tool, "tool result"),
        unit_msg(MessageRole::User, "second user"),
        msg(MessageRole::Assistant, "latest assistant"),
    ];
    let budget = 50;

    let off = trim_to_budget_with(input.clone(), budget, AssemblyWiring::Off);
    let legacy = trim_to_budget_legacy(input.clone(), budget);

    // ① 与"改动前的纯裁剪函数"逐字节相等
    assert_eq!(off, legacy, "关态必须与旧路径逐值相等");
    assert_eq!(
        json_bytes(&off),
        json_bytes(&legacy),
        "关态必须与旧路径逐字节相等(序列化字节)"
    );

    // ② 独立金标准:既有单测锁定的行为(Tool 恒保留 + 最新消息兜底 + 低权重桶丢弃)
    let roles: Vec<MessageRole> = off.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![MessageRole::Tool, MessageRole::User, MessageRole::Assistant],
        "关态行为必须仍是 R4 旧语义, roles = {roles:?}"
    );

    // ③ "未设环境变量 ⇒ Off"的纯函数面(未设 = None)
    assert_eq!(
        parse_assembly_wiring(None),
        AssemblyWiring::Off,
        "未设 CHIMERA_ASSEMBLY_WIRED 必须解析为 Off"
    );
    // 进程级读:仅当环境确实未设时断言(测试不应要求开发者 shell 干净)
    if std::env::var_os(ASSEMBLY_WIRED_ENV).is_none() {
        let via_env = trim_to_budget(input.clone(), budget);
        assert_eq!(
            json_bytes(&via_env),
            json_bytes(&legacy),
            "环境未设时对外入口 trim_to_budget 必须走旧路径(逐字节)"
        );
    }
}

#[test]
fn wire_off_keeps_duplicates() {
    // 关态**不去重**是"零行为变更"的核心证据:旧路径完全没有去重能力,
    // 若关态也去重,则本波次就不是"默认关闭的接线"而是"默认开启的行为变更"。
    let duplicate = msg(MessageRole::User, "repeat me");
    let input = vec![
        msg(MessageRole::User, "first"),
        duplicate.clone(),
        msg(MessageRole::Assistant, "mid"),
        duplicate,
        msg(MessageRole::Assistant, "last"),
    ];

    let off = trim_to_budget_with(input.clone(), 1_000_000, AssemblyWiring::Off);
    assert_eq!(off.len(), 5, "关态必须原样保留全部 5 条(不做任何去重)");
    assert_eq!(
        json_bytes(&off),
        json_bytes(&input),
        "关态在预算充足时必须与输入逐字节相同"
    );
    assert!(
        has_duplicates(&off),
        "关态必须保留重复消息(证明去重能力只在开态生效)"
    );
}

// ============================================================
// 2. 开态去重
// ============================================================

#[test]
fn wire_on_dedups_duplicates() {
    // R12 W1-1 订正（ADR-298 D2）：**只有 Tool 消息参与去重**。本用例因此把重复对
    // 从 User 改成 Tool —— 对话轮次的重复是不同事件，不能被折叠（见下方守卫用例）。
    let duplicate = msg(MessageRole::Tool, "repeat me");
    let input = vec![
        msg(MessageRole::User, "first"),
        duplicate.clone(),
        msg(MessageRole::Assistant, "mid"),
        duplicate,
        msg(MessageRole::Assistant, "last"),
    ];

    let on = trim_to_budget_with(input.clone(), 1_000_000, AssemblyWiring::On);

    // ① 条数恰好减少 1(2 条完全相同的工具输出 ⇒ 收敛为 1 条)
    assert_eq!(
        on.len(),
        input.len() - 1,
        "2 条完全相同的工具消息必须恰好收敛掉 1 条, got {} 条",
        on.len()
    );

    // ② 剩余消息顺序是输入的子序列(逐条比对下标)
    let indices = matched_indices(&on, &input).expect("开态输出必须是输入的子序列");
    assert!(
        indices.windows(2).all(|w| w[0] < w[1]),
        "子序列下标必须严格递增, indices = {indices:?}"
    );
    assert_eq!(
        indices,
        vec![0, 1, 2, 4],
        "保留的必须是首次出现者(1)与被保留的其余消息, indices = {indices:?}"
    );

    // ③ 剩余工具消息无重复项
    assert!(
        !has_duplicates_for_role(&on, MessageRole::Tool),
        "开态输出的工具消息不得重复"
    );
}

/// **守卫用例**：对话轮次的重复是不同事件，必须原样保留
///
/// 语义依据（ADR-298 D2）：用户连说两次「继续」是两次发言；`CHIMERA_DEDUP_NORMALIZE`
/// 默认开启，若对对话轮次去重，连"仅空白不同"的两条不同轮次也会被折成一条。
/// 这条用例是该裁决的哨兵：任何人把 `is_dedup_participant` 改成 `true`（全角色参与），
/// 它会立刻红。
#[test]
fn wire_on_never_dedups_conversation_turns() {
    let same_user = msg(MessageRole::User, "继续");
    let same_assistant = msg(MessageRole::Assistant, "好的");
    let input = vec![
        same_user.clone(),
        same_user.clone(), // 两次「继续」= 两次发言
        same_assistant.clone(),
        same_assistant.clone(),
        msg(MessageRole::System, "sys"),
        msg(MessageRole::System, "sys"),
    ];

    let on = trim_to_budget_with(input.clone(), 1_000_000, AssemblyWiring::On);

    assert_eq!(
        on.len(),
        input.len(),
        "对话轮次（User/Assistant/System）不得被折叠，got {} 条（输入 {} 条）",
        on.len(),
        input.len()
    );
    assert_eq!(
        json_bytes(&on),
        json_bytes(&input),
        "全为对话轮次时，开态必须与输入逐字节相同"
    );
}

#[test]
fn wire_on_never_touches_pinned() {
    // 旧路径的"恒保留"集合 = 工具返回消息 + 最后一条消息;装配阶段不得把它们整体移除。
    let input = vec![
        msg(MessageRole::System, "sys prompt"),
        msg(MessageRole::Tool, "tool result ok"),
        long_msg(MessageRole::Assistant, "history"),
        msg(MessageRole::Tool, "tool result ok"), // 与上一条相同 ⇒ 只应收敛掉这一份
        msg(MessageRole::User, "latest"),
    ];

    let on = trim_to_budget_with(input.clone(), 1_000_000, AssemblyWiring::On);

    // ① 每条不同的工具/系统消息至少保留一份(钉选档不被整体移除)
    for expected in ["sys prompt", "tool result ok"] {
        assert!(
            on.iter().any(|m| m.blocks
                == vec![ContentBlock::Text {
                    text: expected.into()
                }]),
            "开态不得移除 {expected:?}, got {on:?}"
        );
    }
    // ② 重复的工具返回只收敛掉一份(去重 ≠ 清空恒保留档)
    let tool_copies = on
        .iter()
        .filter(|m| m.role == MessageRole::Tool)
        .filter(|m| {
            m.blocks
                == vec![ContentBlock::Text {
                    text: "tool result ok".into(),
                }]
        })
        .count();
    assert_eq!(tool_copies, 1, "重复工具返回应恰好收敛为 1 份");
    // ③ 装配不去重非重复内容
    assert_eq!(
        on.len(),
        input.len() - 1,
        "只应失去那一份重复消息, got {} 条",
        on.len()
    );
}

#[test]
fn wire_on_keeps_tool_and_last_under_budget_pressure() {
    // 开态 + 超预算:装配去重后仍由旧路径裁决预算,旧路径的两条不变量不得被破坏。
    let input = vec![
        long_msg(MessageRole::User, "history 0"),
        long_msg(MessageRole::User, "history 1"),
        msg(MessageRole::Tool, "tool result ok"),
        long_msg(MessageRole::Assistant, "old assistant"),
        msg(MessageRole::User, "latest user"),
    ];

    let on = trim_to_budget_with(input.clone(), 120, AssemblyWiring::On);
    assert!(
        on.iter().any(|m| m.role == MessageRole::Tool),
        "工具返回消息必须恒保留(旧路径不变量), got {on:?}"
    );
    assert_eq!(
        on.last(),
        input.last(),
        "最后一条消息必须保留(时近性保尾段)"
    );
    assert!(is_subsequence(&on, &input), "输出必须是输入的子序列");
    assert!(!has_duplicates(&on), "输出不得含重复消息");
}

#[test]
fn wire_on_preserves_block_structure() {
    // P4 状态守恒:去重只决定"保留哪几条",绝不重建消息内容。
    // Thinking 的 signature 与 ToolUse 的 id/name/input_json 必须逐字节存活。
    let assistant_with_state = AffinityMessage {
        role: MessageRole::Assistant,
        blocks: vec![
            ContentBlock::Thinking {
                thinking: "plan: call search".into(),
                signature: Some("sig-abc-123".into()),
            },
            ContentBlock::ToolUse {
                id: "call-1".into(),
                name: "search".into(),
                input_json: "{\"q\":\"rust\"}".into(),
            },
            ContentBlock::Text {
                text: "calling search".into(),
            },
        ],
    };
    let tool_result = AffinityMessage {
        role: MessageRole::Tool,
        blocks: vec![ContentBlock::ToolResult {
            tool_use_id: "call-1".into(),
            content: "42 results".into(),
            is_error: false,
        }],
    };
    let input = vec![
        assistant_with_state.clone(),
        tool_result.clone(),
        assistant_with_state.clone(), // 对话轮次重复 ⇒ **必须原样保留**（ADR-298 D2 守卫）
        tool_result.clone(),          // 工具输出重复 ⇒ 收敛掉这一份
    ];

    let on = trim_to_budget_with(input.clone(), 1_000_000, AssemblyWiring::On);
    assert_eq!(
        on.len(),
        3,
        "重复的 assistant 轮次必须保留、重复的工具输出应收敛（期望 3 条）, got {on:?}"
    );
    assert_eq!(
        json_one(&on[0]),
        json_one(&assistant_with_state),
        "Thinking/ToolUse 结构必须逐字节保留(签名不可丢)"
    );
    assert_eq!(
        json_one(&on[1]),
        json_one(&tool_result),
        "ToolResult 结构必须逐字节保留"
    );
    assert_eq!(
        json_one(&on[2]),
        json_one(&assistant_with_state),
        "第二条相同的 assistant 轮次必须原样存活（不是被改写、不是被折叠）"
    );
    assert!(is_subsequence(&on, &input), "输出必须是输入的子序列");
}

// ============================================================
// 3. 环境解析:非法值 fail-safe 回退 off
// ============================================================

#[test]
fn invalid_env_falls_back_to_off() {
    // 表驱动断言:与 parse_assembly_wiring 文档表格逐行对齐
    for raw in ["garbage", "2", "on!", "-1", "enabled", "tru"] {
        assert_eq!(
            parse_assembly_wiring(Some(raw)),
            AssemblyWiring::Off,
            "非法值 {raw:?} 必须回退 Off(warn 后 fail-safe)"
        );
    }
    // 合法值(ASCII 大小写不敏感 + 首尾空白容忍)
    for raw in ["on", "ON", " On ", "1", "true", "TRUE", "yes"] {
        assert_eq!(
            parse_assembly_wiring(Some(raw)),
            AssemblyWiring::On,
            "{raw:?} 必须解析为 On"
        );
    }
    for raw in ["off", "OFF", "0", "false", "no"] {
        assert_eq!(
            parse_assembly_wiring(Some(raw)),
            AssemblyWiring::Off,
            "{raw:?} 必须解析为 Off"
        );
    }
    // 空串 / 纯空白 / 未设 ⇒ Off(空串视同未设)
    for raw in [Some(""), Some("   "), None] {
        assert_eq!(
            parse_assembly_wiring(raw),
            AssemblyWiring::Off,
            "{raw:?} 必须解析为 Off"
        );
    }

    // 行为面:非法值解析出的 Off 与显式 Off 输出逐字节一致
    let input = vec![
        msg(MessageRole::User, "first"),
        msg(MessageRole::User, "repeat me"),
        msg(MessageRole::User, "repeat me"),
        msg(MessageRole::Assistant, "last"),
    ];
    let from_invalid = parse_assembly_wiring(Some("garbage"));
    assert_eq!(
        json_bytes(&trim_to_budget_with(input.clone(), 50, from_invalid)),
        json_bytes(&trim_to_budget_with(input.clone(), 50, AssemblyWiring::Off)),
        "非法值必须与显式 off 完全等价"
    );
    assert_eq!(
        json_bytes(&trim_to_budget_with(input, 50, from_invalid)),
        json_bytes(&trim_to_budget_legacy(
            vec![
                msg(MessageRole::User, "first"),
                msg(MessageRole::User, "repeat me"),
                msg(MessageRole::User, "repeat me"),
                msg(MessageRole::Assistant, "last"),
            ],
            50
        )),
        "非法值必须与旧路径完全等价"
    );
}

// ============================================================
// 4. 属性测试:子序列 + 无重复
// ============================================================

mod property {
    use super::*;
    use proptest::prelude::*;

    const ROLES: [MessageRole; 4] = [
        MessageRole::System,
        MessageRole::User,
        MessageRole::Assistant,
        MessageRole::Tool,
    ];

    /// 生成消息:角色 4 选 1,文本取自小字母表(让重复与规范化近重复都能出现)
    fn arb_message() -> impl Strategy<Value = AffinityMessage> {
        (0usize..4, "[a-c ]{0,6}").prop_map(|(role_index, text)| msg(ROLES[role_index], &text))
    }

    proptest::proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// 任意消息序列、**充裕预算**下,开态输出:
        /// ① 是输入的子序列(下标严格递增)② 工具消息无重复 ③ 对话轮次逐条保留 ④ 长度不增
        ///
        /// WHY 预算取充裕值:本用例要验证的是**去重语义**,而预算裁剪（旧路径）会合法
        /// 丢掉消息 —— 两者混在一处会让"轮次被裁剪"被误判成"轮次被去重"
        /// （首版实测正是如此）。预算压力下的行为由下一条属性单独覆盖。
        #[test]
        fn wire_on_output_is_subsequence_and_deduped(
            input in prop::collection::vec(arb_message(), 0..24),
            budget in 200_000u32..400_000,
        ) {
            let out = trim_to_budget_with(input.clone(), budget, AssemblyWiring::On);

            let indices = matched_indices(&out, &input);
            prop_assert!(
                indices.is_some(),
                "开态输出必须是输入的子序列: out={:?} input={:?}",
                out,
                input
            );
            if let Some(indices) = indices {
                prop_assert!(
                    indices.windows(2).all(|w| w[0] < w[1]),
                    "子序列下标必须严格递增: {:?}",
                    indices
                );
            }
            prop_assert!(
                !has_duplicates_for_role(&out, MessageRole::Tool),
                "开态输出的**工具**消息不得重复: {:?}",
                out
            );
            // 对话轮次必须逐条存活：去重只对 Tool 生效（ADR-298 D2）
            let turns_in = input.iter().filter(|m| m.role != MessageRole::Tool).count();
            let turns_out = out.iter().filter(|m| m.role != MessageRole::Tool).count();
            prop_assert_eq!(
                turns_out,
                turns_in,
                "对话轮次（非 Tool）必须逐条保留: out={:?} input={:?}",
                out,
                input
            );
            prop_assert!(
                out.len() <= input.len(),
                "输出长度不得增加: {} > {}",
                out.len(),
                input.len()
            );
        }

        /// 关态恒等于旧路径(逐字节),且长度不增 —— 接线不得引入任何计划外行为
        #[test]
        fn wire_off_always_equals_legacy(
            input in prop::collection::vec(arb_message(), 0..24),
            budget in 0u32..400,
        ) {
            let off = trim_to_budget_with(input.clone(), budget, AssemblyWiring::Off);
            let legacy = trim_to_budget_legacy(input, budget);
            prop_assert_eq!(json_bytes(&off), json_bytes(&legacy));
        }

        /// 任意预算（含极紧）下,开态输出仍是输入的子序列且长度不增
        ///
        /// 这条只管**形状**：预算裁剪会合法丢消息，故不断言"轮次全在"；
        /// 它保证的是"开态不会凭空造出输入里没有的消息、也不会把顺序打乱"。
        #[test]
        fn wire_on_under_budget_pressure_keeps_shape(
            input in prop::collection::vec(arb_message(), 0..24),
            budget in 0u32..400,
        ) {
            let out = trim_to_budget_with(input.clone(), budget, AssemblyWiring::On);
            prop_assert!(
                is_subsequence(&out, &input),
                "开态输出必须是输入的子序列: out={:?} input={:?}",
                out,
                input
            );
            prop_assert!(
                out.len() <= input.len(),
                "输出长度不得增加: {} > {}",
                out.len(),
                input.len()
            );
        }
    }
}
