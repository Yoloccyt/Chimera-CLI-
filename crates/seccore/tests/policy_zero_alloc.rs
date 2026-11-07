//! R1(ADR-260)分配预算锁:判决热路径零堆分配断言。
//!
//! 独立集成测试二进制:全局分配器替换只影响本文件,单测试函数串行执行
//! 断言(并行测试会污染全局计数,故本文件刻意只含一个 #[test])。
//!
//! 覆盖面(与 ADR-260 §2 的零分配主张一致):
//! - 未登记程序(零信任默认 Deny):索引 miss,O(R)→O(1) 且零分配;
//! - `*` 白名单规则命中(声明式配置的主力形态):归一快路径 + `*` 短路,零分配;
//! - 非 `*` 通配规则仍含 join_args 分配(明示不纳入零分配主张,见 ADR-260)。
//!
//! CountingAlloc 模式复用 `crates/nexus-contracts/benches/util_micro.rs` 先例。

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

use seccore::execpolicy::{ExecPolicy, ExecPolicyRule, PolicyAction, RulePattern};

/// 断言 `f` 执行期间零堆分配(先热身一次,消除首次执行的惰性初始化噪声)
fn assert_zero_alloc(label: &str, mut f: impl FnMut() -> PolicyAction) -> PolicyAction {
    let _ = f(); // 热身:HashMap 已建、分支预测就绪
    let before = ALLOCS.load(Relaxed);
    let out = f();
    let after = ALLOCS.load(Relaxed);
    assert_eq!(
        after - before,
        0,
        "{label}: 期望零堆分配,实际 {} 次",
        after - before
    );
    out
}

#[test]
fn evaluate_ref_hot_paths_are_zero_alloc() {
    // 零信任白名单:echo 系列放行,其余默认 Deny(生产零信任姿态的代表形态)
    let policy = ExecPolicy::new()
        .add_rule(ExecPolicyRule {
            pattern: RulePattern::new("bench.echo", "*"),
            action: PolicyAction::Allow,
        })
        .add_rule(ExecPolicyRule {
            pattern: RulePattern::new("fs.read", "cat *"),
            action: PolicyAction::Allow,
        });

    // ① 未登记程序 → 索引 miss → 默认 Deny:O(1) 且零分配
    // (零信任下所有未声明工具都走此路径——安全裁决的 dominant path)
    let out = assert_zero_alloc("deny-miss(未登记程序)", || {
        policy.evaluate_ref("unknown.tool", &["--flag", "value"])
    });
    assert_eq!(out, PolicyAction::Deny);

    // ② `*` 白名单命中:归一快路径 + `*` 短路,带参也零分配
    let out = assert_zero_alloc("allow-hit(* 规则带参)", || {
        policy.evaluate_ref("bench.echo", &["--flag", "value"])
    });
    assert_eq!(out, PolicyAction::Allow);

    // ③ 无参调用同路径(规范小写输入)
    let out = assert_zero_alloc("allow-hit(* 规则无参)", || {
        policy.evaluate_ref("bench.echo", &[])
    });
    assert_eq!(out, PolicyAction::Allow);

    // ④ 非规范输入(大写)的归一成本上界 = 恰 1 次 to_lowercase 分配
    // (归一单次化:整次评估仅此一处分配,不随规则数放大——旧实现是 2k 次)
    let before = ALLOCS.load(Relaxed);
    let out = policy.evaluate_ref("BENCH.ECHO", &["--flag"]);
    let after = ALLOCS.load(Relaxed);
    assert_eq!(out, PolicyAction::Allow);
    assert_eq!(
        after - before,
        1,
        "大写输入应恰 1 次归一分配,实际 {} 次",
        after - before
    );
}
