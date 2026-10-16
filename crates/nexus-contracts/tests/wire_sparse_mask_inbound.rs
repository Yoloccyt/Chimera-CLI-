//! `SparseMask<T>` 入站形状测试 —— 判的是 L0 **唯一**那份手写 `Deserialize`
//! （`src/sparse/masks.rs` 的 `Helper` 路径），不是 round-trip 恒等。
//!
//! WHY 单独一份文件，而不是并进 `wire_roundtrip.rs`
//! ------------------------------------------------
//! `wire_roundtrip.rs` 的判据形态是「编出去再解回来必须相等」，它对入站非法形状
//! 天然失明：喂进去的每一个值都是自己编码出来的，`active_set` 必然与 `active_ids`
//! 一致、`sparsity_ratio` 必然在值域内。手写 `Deserialize` 存在的**唯一理由**恰好是
//! 处理别人写的数据（`#[serde(skip)]` 的 `active_set` 若不重建，`is_active` 会恒
//! false 而往返恒等式依然成立）。所以那套断言覆盖不到这里要问的问题——
//! 「一份**不是我写的**、形状不完全合意的 payload 进来，会得到什么」。
//! 分工先例即 `wire_format_compat.rs`（专判位置式 vs 命名式，round-trip 文件不重复）。
//!
//! 缺口登记（F-12，2026-10-16 实测）：本类型此前只被 `src/sparse/masks.rs` 的
//! `#[cfg(test)]` 尾内一份 `serde_json` 往返测过；`grep -rn SparseMask crates/nexus-contracts/tests/`
//! = **0** ⇒ ADR-004 的跨层协议（MessagePack）与入站非法形状两个面从未执行过。
//!
//! 断言口径：**每条都钉「现状」而不是「应该」**。值域不校验、重复 id 不去重这两件事
//! 是否要改成硬校验，属公共 API 行为变更（会拒掉今天能读的存量数据），本文件不替
//! 那个决定作保——它保证的是：一旦有人改了实现，这里立刻红，而不是静默换语义。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test-unit idiom; E-5 targets production code

use nexus_contracts::masks::SparseMask;
use nexus_contracts::ToolId;
use serde::Serialize;

/// 与线上形状同构的两键结构（`active_set` 是 `#[serde(skip)]`，不进 wire）。
#[derive(Serialize)]
struct WireNamed {
    active_ids: Vec<ToolId>,
    sparsity_ratio: f32,
}

/// 少一个键：判「入站缺字段」是否被拒（Helper 无 `#[serde(default)]`）。
#[derive(Serialize)]
struct WireNoRatio {
    active_ids: Vec<ToolId>,
}

/// 多两个键：判未知字段是否被忽略，且 `active_set` 是否**无视线上同名键**、
/// 坚持由 `active_ids` 重建（若哪天有人把 `#[serde(skip)]` 改成 `skip_deserializing`
/// 之外的形态并让线上 `active_set` 生效，这里会红）。
#[derive(Serialize)]
struct WireExtraKeys {
    active_ids: Vec<ToolId>,
    sparsity_ratio: f32,
    active_set: Vec<ToolId>,
    unknown: u8,
}

fn t(ids: &[&str]) -> Vec<ToolId> {
    ids.iter().map(|s| ToolId::new(*s)).collect()
}

fn json_of(v: &serde_json::Value) -> SparseMask<ToolId> {
    serde_json::from_str::<SparseMask<ToolId>>(&v.to_string())
        .unwrap_or_else(|e| panic!("合法形状入站解码失败: {e} / payload={v}"))
}

// ============================================================
// 1. 双协议正形：同一条数据，两种编码解出同一个值
// ============================================================

#[test]
fn same_payload_decodes_identically_under_both_protocols() {
    let wire = WireNamed {
        active_ids: t(&["tool-a", "tool-b", "tool-c"]),
        sparsity_ratio: 0.4,
    };
    let json: SparseMask<ToolId> =
        serde_json::from_str(&serde_json::to_string(&wire).unwrap()).unwrap();
    let mp: SparseMask<ToolId> =
        rmp_serde::from_slice(&rmp_serde::to_vec_named(&wire).unwrap()).unwrap();
    assert_eq!(json, mp, "JSON 与 MessagePack 命名 map 必须解出同一个值");
    // 手写 impl 的核心不变量：重建出的集合与 vec 同源。
    assert_eq!(json.active_set.len(), 3);
    for id in json.active_ids.clone() {
        assert!(
            json.is_active(&id),
            "{id:?} 在 active_ids 里却 is_active=false"
        );
    }
}

#[test]
fn positional_array_still_decodes_for_this_type() {
    // 本仓的命名 map 约定由 ADR-213 R8.1 定（ADR-004 只裁「MessagePack 而非 JSON」、
    // ADR-184 只裁 NexusError 无 source 字段，两者都不裁编码形态），而「位置式 array 是否仍读得动」
    // 已由 `wire_format_compat.rs` 对整个 crate 实测过（结论：读得动，F1 双读 shim
    // 因此取消）。本条只做一件事：确认 SparseMask 这份**手写** impl 转发的
    // `Helper`（derive）没有丢掉 visit_seq 路径——它是 2 字段、无可选，故位置式安全。
    let wire = WireNamed {
        active_ids: t(&["tool-a"]),
        sparsity_ratio: 0.0,
    };
    let bytes = rmp_serde::to_vec(&wire).unwrap();
    let back: SparseMask<ToolId> = rmp_serde::from_slice(&bytes)
        .expect("位置式 array 必须可读：本类型无 skip_serializing_if 字段，长度恒为 2");
    assert_eq!(back.active_ids, t(&["tool-a"]));
    assert!(back.is_active(&ToolId::new("tool-a")));
}

// ============================================================
// 2. 入站缺字段：两份协议都不得静默补默认值
// ============================================================

#[test]
fn missing_sparsity_ratio_is_rejected_by_both_protocols() {
    let json_err = serde_json::from_str::<SparseMask<ToolId>>(r#"{"active_ids":["a"]}"#);
    assert!(
        json_err.is_err(),
        "缺 sparsity_ratio 却被收下: {json_err:?} —— derive 的缺字段报错被吞掉了？"
    );
    let wire = WireNoRatio {
        active_ids: t(&["a"]),
    };
    let mp_err =
        rmp_serde::from_slice::<SparseMask<ToolId>>(&rmp_serde::to_vec_named(&wire).unwrap());
    assert!(
        mp_err.is_err(),
        "MessagePack 命名 map 缺键被收下: {mp_err:?}"
    );
}

#[test]
fn missing_active_ids_is_rejected() {
    let err = serde_json::from_str::<SparseMask<ToolId>>(r#"{"sparsity_ratio":0.5}"#);
    assert!(err.is_err(), "缺 active_ids 却被收下: {err:?}");
}

#[test]
fn wrong_field_type_is_rejected() {
    // active_ids 必须是数组；把标量喂进 Vec 解码路径应当报错而不是解出空集。
    let err =
        serde_json::from_str::<SparseMask<ToolId>>(r#"{"active_ids":5,"sparsity_ratio":0.5}"#);
    assert!(err.is_err(), "active_ids=5（标量）被收下: {err:?}");
}

// ============================================================
// 3. 入站多余键：忽略，且不覆盖重建结果
// ============================================================

#[test]
fn extra_keys_are_ignored_and_set_is_rebuilt_from_ids() {
    let wire = WireExtraKeys {
        active_ids: t(&["a", "b"]),
        sparsity_ratio: 0.5,
        // 线上放一个与 active_ids 矛盾的 active_set：它**不该**被采用。
        active_set: t(&["zzz"]),
        unknown: 7,
    };
    let mp: SparseMask<ToolId> =
        rmp_serde::from_slice(&rmp_serde::to_vec_named(&wire).unwrap()).unwrap();
    assert_eq!(mp.active_ids, t(&["a", "b"]), "多余键不得改变 active_ids");
    assert_eq!(mp.sparsity_ratio, 0.5);
    assert!(mp.is_active(&ToolId::new("a")) && mp.is_active(&ToolId::new("b")));
    assert!(
        !mp.is_active(&ToolId::new("zzz")),
        "线上 active_set 键生效了——集合不再由 active_ids 重建"
    );
    // JSON 侧同理：未声明字段被 derive 忽略（Helper 无 deny_unknown_fields）。
    let json = json_of(&serde_json::json!({
        "active_ids": ["a", "b"], "sparsity_ratio": 0.5, "unknown": 7
    }));
    assert_eq!(json, mp, "同一逻辑负载两种协议必须等价");
}

// ============================================================
// 4. 重复 id：Vec 计数 vs 集合基数（现状，非不变量）
// ============================================================

#[test]
fn duplicate_active_ids_count_as_vec_but_dedup_in_set() {
    // 钉的是**现状**：`active_count()` 取 `active_ids.len()`（不去重），
    // 而 `is_active` 走 HashSet（天然去重）。二者对同一份数据的口径不同。
    // 这不是「实现正确」的断言——将来若给入站路径加去重/校验，本条会红，
    // 而那正是要有人签字的时刻：它是公共类型在存量数据上的行为变更。
    let m = json_of(&serde_json::json!({
        "active_ids": ["dup", "dup", "solo"], "sparsity_ratio": 0.0
    }));
    assert_eq!(
        m.active_count(),
        3,
        "active_count 现状=Vec 长（不是去重基数）"
    );
    assert_eq!(m.active_set.len(), 2, "集合现状=去重基数");
    assert!(m.is_active(&ToolId::new("dup")));
    assert_eq!(
        m.active_ids.iter().filter(|i| i.as_str() == "dup").count(),
        2,
        "重复元素不得在解码时被丢弃"
    );
}

// ============================================================
// 5. sparsity_ratio：透传，不重算，也不校验值域
// ============================================================

#[test]
fn sparsity_ratio_is_passed_through_not_recomputed() {
    // 生产构造器保证 ratio == 1 - k/n；入站路径**不重算**（Helper 原样取字段）。
    let m = json_of(&serde_json::json!({
        "active_ids": ["a", "b", "c"], "sparsity_ratio": 0.9
    }));
    assert_eq!(m.sparsity(), 0.9, "入站 ratio 被重算或改写了");
    assert_eq!(
        m.active_count(),
        3,
        "ratio 与 ids 长度不成比例不得影响集合重建"
    );
}

#[test]
fn out_of_domain_ratio_is_accepted_currently() {
    // 现状：类型文档声明 ratio ∈ [0,1]，但入站路径没有任何校验。
    // 本条不是「这样对」的主张，而是把「今天越界值读得进来」钉住：
    // 加校验是一个会拒掉存量数据的决定，必须先有人改这条再改实现。
    for bad in [1.5f32, -0.5, 100.0] {
        let m = json_of(&serde_json::json!({
            "active_ids": ["a"], "sparsity_ratio": bad
        }));
        assert_eq!(
            m.sparsity(),
            bad,
            "越界 ratio {bad} 竟然被改写/拒绝——值域校验被谁加上了？"
        );
    }
}

// ============================================================
// 6. 空集边界
// ============================================================

#[test]
fn empty_active_ids_decodes_to_empty_set_not_missing() {
    // 关键区分：`[]`（空集）与「键缺失」（报错）是两件事。若 derive 的 Option
    // 化处理被引入，空集会静默变成 None 而不是空 Vec。
    let m = json_of(&serde_json::json!({"active_ids": [], "sparsity_ratio": 1.0}));
    assert_eq!(m.active_count(), 0);
    assert!(m.active_set.is_empty());
    assert!(!m.is_active(&ToolId::new("anything")));
    assert_eq!(m, SparseMask::empty(), "空集入站必须等于 empty() 构造结果");
}

// ============================================================
// 7. 断言牙口反事实（self-check：证明上面的集合断言不是恒真）
// ============================================================

#[test]
fn set_consistency_checks_would_fire_on_the_derive_shape() {
    // 本文件最核心的主张是「手写 impl 若退化成 derive，`is_active` 会恒 false」。
    // 光看测试全绿证明不了这条会红——`active_set` 是 pub，所以不必动生产码就能
    // 手工造出 derive 的产物形状，把断言面按在缺陷本身上验一次。
    // 与本仓「惰性子门反事实测负覆盖」同法：改坏输入，看判决是否跟着变。
    let mut m = json_of(&serde_json::json!({
        "active_ids": ["a", "b"], "sparsity_ratio": 0.0
    }));
    let healthy = m.clone();
    m.active_set = std::collections::HashSet::new(); // == derive 会留下的空集
    assert_eq!(
        healthy.active_set.len(),
        2,
        "前置不成立：正常解码后集合就是空的，那本文件所有集合断言都在测空气"
    );
    assert!(!m.is_active(&ToolId::new("a")), "空 set 却判活跃");
    assert_eq!(m.active_ids.len(), 2, "vec 侧应当完好（缺陷只在 set 侧）");
    assert_ne!(
        m.active_set.len(),
        m.active_ids.len(),
        "集合数与 vec 数区分不开 ⇒ same_payload/… 里的 active_set.len() 断言是恒真"
    );
    // 且恒等式掩盖不了它：退化形状与正确形状**不相等**，所以 round-trip 的
    // PartialEq 断言同样有牙（不是只有集合断言在守）。
    assert_ne!(m, healthy, "PartialEq 必须把 desync 视为不等");
}
