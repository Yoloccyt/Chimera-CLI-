//! ADR-226 D1 / R2 C-94：`mlc-engine::card_system` 锁段协议的 permutation 模型检验。
//!
//! # 为什么是这个范围（先读再改）
//! `MlcEngine` 里 `card_system: Arc<Mutex<ExperienceCardSystem>>`（`engine.rs`）是**唯一**
//! “纯同步锁 + 短临界区、锁内不 await”的可建模面：写者只调 `add_card`，读者只读
//! `card_count()` / `cards()` / `get_card_by_node()`。hcw-window 的主状态锁是
//! `tokio::sync::RwLock`（async 锁），loom 不建模 tokio task，**不在本测试范围内**
//! （见 ADR-226 §2 ② 的工具-属性错配订正）。
//!
//! # 为什么用 `#![cfg(loom)]`
//! 本文件只在 `RUSTFLAGS="--cfg loom"` 时编译；常规 `cargo test` 下它是空 target，
//! 不改变既有测试面、不改变本地耗时。CI 里由独立 job 用 `--cfg loom` 真跑
//! （ADR-226 D2 的 E1-a/E1-c：载体必须是**真实执行**的步骤，不是观测位）。
//!
//! # 它证的是什么（不要夸大）
//! 证的是「锁 + 被保护类型的状态迁移」在**任意交错**下保持计数守恒、且读者永不看到
//! 半写状态。它**不**证具体读写路径的业务正确性（那属于常规测试），也**不**建模
//! `ExperienceCardSystem` 内部的 HashMap 访问（那些在锁内串行，loom 无需展开）。

#![cfg(loom)]
// test/bench 惯用法；E-5 的 unwrap/expect 目标面是生产代码（同仓其他 tests 同口径）。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use loom::sync::{Arc, Mutex};
use loom::thread;

use mlc_engine::ExperienceCardSystem;
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, ExecutionStatus, ExperienceCard,
};
use nexus_contracts::ThreeFactorScore;

/// 构造样例卡片（与 `experience_card_system.rs` 内部测试夹具同构，仅 node 不同）。
fn card(node: &str) -> ExperienceCard {
    ExperienceCard {
        card_id: Box::from(format!("card-{node}")),
        task_id: Box::from("t1"),
        node_id: Box::from(node),
        parent_id: None,
        created_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0)
            .expect("合法时间戳"),
        operator: AtomicOperator::Draft,
        score: 0.8,
        delta_vs_parent: 0.0,
        method_family: Box::from("draft_pipeline"),
        error_signature: None,
        three_factor: ThreeFactorScore {
            quality: 0.8,
            progress: 0.1,
            novelty: 0.5,
        },
        execution_status: ExecutionStatus::Success,
        token_evidence_ids: Vec::new(),
        segment_id: None,
        metadata: CardMetadata::default(),
    }
}

/// 两写者并发 `add_card` + 一读者并发读：卡片数必须恰好等于写入次数，
/// 且读者读到的值永远落在 `[0, 2]`（不得看到半个状态、不得 panic 于锁中毒）。
#[test]
fn card_system_lock_section_conserves_count() {
    loom::model(|| {
        let sys = Arc::new(Mutex::new(ExperienceCardSystem::new(1.414, 0.1)));

        let w1 = {
            let sys = sys.clone();
            thread::spawn(move || {
                let mut g = sys.lock().unwrap();
                g.add_card(card("n1"));
                g.card_count()
            })
        };
        let w2 = {
            let sys = sys.clone();
            thread::spawn(move || {
                let mut g = sys.lock().unwrap();
                g.add_card(card("n2"));
                g.card_count()
            })
        };
        let r = {
            let sys = sys.clone();
            thread::spawn(move || {
                let g = sys.lock().unwrap();
                g.card_count()
            })
        };

        // 写者在本次锁内看到的计数必须是 1 或 2（自己刚写的那张一定可见）
        let c1 = w1.join().unwrap();
        let c2 = w2.join().unwrap();
        let cr = r.join().unwrap();
        assert!((1..=2).contains(&c1), "写者锁内计数越界: {c1}");
        assert!((1..=2).contains(&c2), "写者锁内计数越界: {c2}");
        assert!(cr <= 2, "读者不得看到超过总写入数的计数: {cr}");

        // 终态必须精确等于写入次数 —— 丢更新/重复计数在这里会响
        let fin = sys.lock().unwrap();
        assert_eq!(fin.card_count(), 2, "终态卡片数必须等于写入次数");
    });
}
