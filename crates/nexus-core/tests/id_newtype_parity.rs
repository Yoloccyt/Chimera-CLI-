//! `id_newtype!` 镜像契约的**行为**对拍 —— L1 `nexus-core` ↔ L0 `nexus-contracts`。
//!
//! 对应 ADR: **ADR-218 决策 3**（承 ADR-033 / ADR-217 P3-7 的登记项）。
//!
//! # 为什么需要本测
//!
//! 两侧各有一份同名宏：`crates/nexus-core/src/newtype.rs:61` 与
//! `crates/nexus-contracts/src/sparse/ids.rs:38`。L0 禁止依赖任何 workspace crate
//! （ADR-033 / G-99），因此**无法**复用 L1 的宏，只能另写一份并在文档里声称
//! 「生成的实现完全一致，保证序列化兼容性」。在本测之前，那句承诺**零机器判据**——
//! 改一侧而另一侧不动，没有任何东西会红。
//!
//! # 为什么用行为对拍而不是宏体文本比对
//!
//! ADR-218 决策 3 明确否决「跨 crate 宏体文本比对门」：那需要第二份宏体解析器，
//! 而双份解析器必漂是本仓既有病灶。本测改为断言**可观察语义**（序列化字节、
//! Display、`&str` 互操作、哈希键互通），即文档承诺的实质内容。
//!
//! 实测两侧宏体的差异**只有路径限定写法**（`::std::string::String` vs `String` 等
//! 7 处），语义等价 ⇒ 下面的断言今日应当全部成立；若将来任一侧改掉
//! `#[serde(transparent)]`、`Borrow<str>` 或 `Hash` 派生，本测当场定位。
//!
//! # 为什么放在 L1 侧
//!
//! 方向合规：L1 dev/生产依赖 L0 恒允许（`L(N) → L(0)`）。
//! **刻意不在 L0 侧做此对拍**——那需要 `nexus-contracts` 反向 dev-dep `nexus-core`，
//! 会重新引入 2026-09-11 刚清零的 L0→L1 dev 依赖环（见
//! `crates/nexus-contracts/Cargo.toml` 的解环注释），与 ADR-212 决策 3 直接冲突。
//!
//! # 覆盖的重名对
//!
//! 两侧同名类型（即同一物理 ID 可能被任一侧构造、进而跨层传输）恰有两个：
//! `TaskId`、`OperationId`。其余名字各侧独有，不参与互通，故不在对拍集内。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test-unit idiom; E-5 targets production code

use std::collections::HashMap;

use nexus_contracts::{OperationId as L0OperationId, TaskId as L0TaskId};
use nexus_core::{OperationId, TaskId};

/// 逐维度对拍：任一侧的改动若偏离另一侧，必有一项断言红。
///
/// 只断言**可观察行为**，不断言类型相同——它们是两种类型，且这正确
/// （newtype 的意义就是禁止 `ToolId` 误传给 `FileId`；跨层靠字符串 wire 而非类型）。
macro_rules! parity_pair {
    ($l1:ty, $l0:ty, $payload:expr) => {{
        let payload: &str = $payload;
        let a: $l1 = <$l1>::new(payload);
        let b: $l0 = <$l0>::new(payload);

        // 1) Display 一致（日志/错误文本与 ID 拼接的可达面）
        assert_eq!(a.to_string(), b.to_string(), "Display 不一致");
        assert_eq!(a.to_string(), payload);

        // 2) `&str` 三通道一致：Deref / AsRef / Borrow
        let da: &str = &a;
        let db: &str = &b;
        assert_eq!(da, db, "Deref 结果不一致");
        assert_eq!(a.as_ref(), db, "AsRef 结果不一致");
        assert_eq!(da, payload);

        // 3) 构造入口一致：new / From<&str> / From<String> 三者应产出相等值
        assert_eq!(a, <$l1>::from(payload));
        assert_eq!(a, <$l1>::from(payload.to_string()));
        assert_eq!(b, <$l0>::from(payload));
        assert_eq!(b, <$l0>::from(payload.to_string()));

        // 4) JSON 字节一致 + 与**内部字符串的 JSON** 一致（`#[serde(transparent)]` 的全部
        //    意义：已落盘的 SQLite 数据与 EventBus payload 无需迁移）。
        //    期望值必须由同一序列化器对裸 `&str` 推出，不能手写 `"\"{payload}\""`——
        //    后者对含引号/反斜杠的 payload 少一层转义，本测首跑正是这样误判的
        //    （代码是对的，断言是错的；与「静默吞参数的解析器要 round-trip 自证」同族）。
        let ja = serde_json::to_string(&a).unwrap();
        let jb = serde_json::to_string(&b).unwrap();
        let jbare = serde_json::to_string(payload).unwrap();
        assert_eq!(ja, jb, "JSON 字节不一致");
        assert_eq!(ja, jbare, "JSON 不等于内部字符串的 JSON（transparent 失效）");

        // 5) MessagePack 字节一致（ADR-004：事件序列化协议为 MessagePack）
        let ma = rmp_serde::to_vec(&a).unwrap();
        let mb = rmp_serde::to_vec(&b).unwrap();
        assert_eq!(ma, mb, "MessagePack 字节不一致");

        // 6) 双向跨类型往返：一侧序列化的 wire 必须能被另一侧反序列化
        //    （这才是"序列化兼容性"承诺的可执行形式）
        let a_from_b_json: $l1 = serde_json::from_str(&jb).unwrap();
        let b_from_a_json: $l0 = serde_json::from_str(&ja).unwrap();
        assert_eq!(a, a_from_b_json, "L0 wire → L1 反序列化不等");
        assert_eq!(b, b_from_a_json, "L1 wire → L0 反序列化不等");
        let a_from_b_msg: $l1 = rmp_serde::from_slice(&mb).unwrap();
        assert_eq!(a, a_from_b_msg, "L0 msgpack → L1 反序列化不等");

        // 7) 哈希键互通：两侧都以 `Borrow<str>` 参与 HashMap 查找，且派生 Hash 等价于
        //    内部 String 的 Hash ⇒ 同一 payload 在两侧、以及用 &str 查，必须同桶。
        //    这条最容易被"顺手加个字段/换成包装"破坏，而破坏后是**运行期查不到**而非编译错。
        use std::hash::{Hash, Hasher};
        let mut hb1 = std::collections::hash_map::DefaultHasher::new();
        let mut hb2 = std::collections::hash_map::DefaultHasher::new();
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        a.hash(&mut hb1);
        b.hash(&mut hb2);
        payload.hash(&mut hs);
        assert_eq!(hb1.finish(), hb2.finish(), "两侧 Hash 不等");
        assert_eq!(hb1.finish(), hs.finish(), "newtype Hash 与 &str Hash 不等（Borrow 互通破裂）");

        let mut map: HashMap<$l1, u8> = HashMap::new();
        map.insert(a.clone(), 7);
        assert_eq!(map.get(payload), Some(&7), "&str 查不到（L1 侧 Borrow<str> 失效）");
        let mut map0: HashMap<$l0, u8> = HashMap::new();
        map0.insert(b.clone(), 7);
        assert_eq!(map0.get(payload), Some(&7), "&str 查不到（L0 侧 Borrow<str> 失效）");
    }};
}

#[test]
fn task_id_wire_and_behavior_parity_across_l0_l1() {
    parity_pair!(TaskId, L0TaskId, "task-42");
    // 真实形态也拍一次：UUIDv7 风格的长 ID
    parity_pair!(TaskId, L0TaskId, "018f6d2a-7c3b-7a12-9b0e-4f2d1c8e9a77");
}

#[test]
fn operation_id_wire_and_behavior_parity_across_l0_l1() {
    parity_pair!(OperationId, L0OperationId, "op-audit-0001");
    parity_pair!(
        OperationId,
        L0OperationId,
        "018f6d2a-7c3b-7a12-9b0e-4f2d1c8e9a78"
    );
}

/// 空串与含引号/反斜杠的 payload：wire 兼容承诺在这些边界上最容易破
/// （transparent 序列化必须仍走 serde 的字符串转义，而不是手写拼接）。
#[test]
fn id_parity_covers_awkward_payloads() {
    parity_pair!(TaskId, L0TaskId, "");
    parity_pair!(TaskId, L0TaskId, "he said \"hi\" \\ back");
    parity_pair!(OperationId, L0OperationId, "中文与 emoji 🚀 id");
}

/// 反向证据：两个 `TaskId` 是**不同类型**，不能直接比较或互相赋值。
///
/// 这是刻意的（newtype 的类型安全是设计目标，见 `sparse/ids.rs` 的「类型安全」节）。
/// 若哪天有人把 L0 类型 `pub use` 进 nexus-core 冒充"消除重复"，本测会因
/// `Deserialize` 实现歧义或 `parity_pair!` 的类型标注退化而暴露——故此处显式
/// 记录"不合并类型"这一决定，而不是留给下一个人重新发现。
#[test]
fn l0_and_l1_task_id_are_distinct_types() {
    let core_id = TaskId::new("t-1");
    let l0_id = L0TaskId::new("t-1");
    // 字符串层面相等……
    assert_eq!(core_id.as_str(), l0_id.as_str());
    // ……类型层面不同（type_name 不等即证明不是同一类型的别名）
    assert_ne!(
        std::any::type_name::<TaskId>(),
        std::any::type_name::<L0TaskId>(),
        "两侧 TaskId 变成了同一类型——newtype 分层的类型安全边界被改动，须先立 ADR"
    );
}

/// **本测自身有牙的植入证据**（不靠"跑过了"就当"测到了"）。
///
/// 上面的 `parity_pair!` 全部断言都是相等式；若某条因为类型退化/序列化改动而恒成立，
/// 整个文件就是空转。故此处用**故意错配**的 payload 走同一批断言，要求它们必须炸。
/// 两个方向各一条：Display 与 JSON —— 正是「镜像契约」最可能被静默破坏的两面。
#[test]
#[should_panic(expected = "Display 不一致")]
fn display_assertion_actually_fires_on_mismatch() {
    let a = TaskId::new("left");
    let b = L0TaskId::new("right");
    assert_eq!(a.to_string(), b.to_string(), "Display 不一致");
}

#[test]
#[should_panic(expected = "JSON 字节不一致")]
fn json_assertion_actually_fires_on_mismatch() {
    let a = TaskId::new("left");
    let b = L0TaskId::new("right");
    let ja = serde_json::to_string(&a).unwrap();
    let jb = serde_json::to_string(&b).unwrap();
    assert_eq!(ja, jb, "JSON 字节不一致");
}
