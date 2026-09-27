//! Empirical guard for the lock shape used by `handle_quota_exhausted`.
//!
//! WHY this file exists: `src/lib.rs:115` takes `chains.get_mut(route_key)` inside
//! `if let Some(mut chain) = …`, so the `RefMut` stays alive for the whole arm —
//! and the `ChainExhausted` arm at `:119` then calls `chains.remove(route_key)` on
//! the SAME key. `cargo check` cannot see this (it is not a type error) and
//! `check_await_across_guard` cannot see it either (no `.await` sits between them):
//! the lock is a per-shard write lock, and taking it twice on one thread is a
//! self-deadlock, not a borrow-checker violation.
//!
//! So the only way to settle "is this a real defect or am I over-reading DashMap"
//! is to run the shape. Measured against dashmap 6.2.1 (`Cargo.lock:1387`).
//!
//! WHY these tests cannot hang CI: the nested attempt runs on its own thread and is
//! observed through a channel with a bounded `recv_timeout`. A parked thread does
//! not block process exit, and the thread is deliberately never joined.
//!
//! Test 2 is the positive control: without it, "the nested shape timed out" would be
//! indistinguishable from "this test is broken" — the same vacuity rule the repo's
//! gates apply to themselves.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
use dashmap::DashMap;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Long enough for an in-memory map op, short enough to keep the suite fast.
const BUDGET: Duration = Duration::from_millis(500);

fn map_with_entry() -> DashMap<String, u32> {
    let m = DashMap::new();
    m.insert("k".to_string(), 1u32);
    m
}

#[test]
fn nested_remove_while_refmut_held_does_not_return() {
    let m = map_with_entry();
    let (tx, rx) = mpsc::channel();
    // Move the map into the thread so a parked lock cannot poison any other test.
    let _t = thread::spawn(move || {
        if let Some(mut chain) = m.get_mut("k") {
            *chain.value_mut() += 1;
            m.remove("k");
        }
        drop(m);
        let _ = tx.send("done");
    });
    // The send is inside the thread; a deadlock means nothing ever arrives.
    let verdict = rx.recv_timeout(BUDGET);
    assert!(
        verdict.is_err(),
        "nested remove returned with {verdict:?}: the RefMut-then-remove shape does NOT \
         self-deadlock on this dashmap version -- retract the P1 finding at \
         docs/architecture/CHIMERA_架构减法与职责重组设计方案_v2.30.md §8.1"
    );
    // `_t` is deliberately never joined: joining would block the test binary on the
    // parked thread, which is the failure mode this file is designed to avoid.
}

#[test]
fn positive_control_block_scoped_guard_lets_the_same_remove_proceed() {
    let m = map_with_entry();
    let (tx, rx) = mpsc::channel();
    let t = thread::spawn(move || {
        // The shape `advance_degradation` / the event loop at :556-567 already use:
        // read through the guard inside a block, drop it, then mutate the map.
        let exhausted = {
            match m.get_mut("k") {
                Some(mut chain) => {
                    *chain.value_mut() += 1;
                    true
                }
                None => false,
            }
        };
        if exhausted {
            m.remove("k");
        }
        drop(m);
        let _ = tx.send("done");
    });
    assert_eq!(
        rx.recv_timeout(BUDGET)
            .expect("block-scoped pattern must complete"),
        "done",
        "the recommended pattern timed out too, so this file proves nothing about \
         ordering -- the harness itself is broken"
    );
    t.join().expect("positive-control thread panicked");
}

#[test]
fn positive_control_removes_the_entry_so_the_fix_is_not_a_no_op() {
    // Guards the thing a timeout test cannot see: that the block-scoped shape really
    // drops the chain, which is the behaviour the exhausted arm is supposed to have.
    let m = map_with_entry();
    let level = {
        let mut chain = m.get_mut("k").expect("entry present");
        *chain.value_mut() += 41;
        *chain
    };
    assert_eq!(level, 42, "guard read should observe the mutation");
    m.remove("k");
    assert!(
        m.get("k").is_none(),
        "entry must be gone after scoped removal"
    );
    assert_eq!(m.len(), 0);
}
