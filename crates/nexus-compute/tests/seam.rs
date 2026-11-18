//! 缝合点测试替身的行为契约 — Clock / Rng / Fs（手册 §10.8,Ω₇ 可测试性根基）
//!
//! 只覆盖**内存替身**（`FixedClock` / `SeedRng42` / `ChaCha8Rng` / `MemFs`）:
//! 它们是不依赖真实时间/真实磁盘的确定性注入面,集成测试必须依赖其契约而非实现。
//! 生产后端（`SystemClock` / `OsFs`）的磁盘行为由 `src/seam.rs` 内联单测锁定,
//! 此处刻意不碰真实 IO（测试确定性纪律）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code

use std::path::{Path, PathBuf};

use nexus_compute::seam::{ChaCha8Rng, Clock, FixedClock, Fs, MemFs, Rng, SeamError, SeedRng42};

#[test]
fn fixed_clock_returns_the_same_instant_on_every_read() {
    // 契约:注入时钟不随真实时间推进 —— 两次读取必须完全相同
    let clock = FixedClock::default();
    let t0 = clock.now();
    std::thread::sleep(std::time::Duration::from_millis(2));
    assert_eq!(clock.now(), t0, "FixedClock 不得随墙钟推进");
}

#[test]
fn fixed_clock_instances_capture_independent_snapshots() {
    // 契约:每个实例在构造时刻快照 —— 同实例恒等,不宣称跨实例排序
    let a = FixedClock::new();
    let b = FixedClock::new();
    assert_eq!(a.now(), a.now());
    assert_eq!(b.now(), b.now());
    assert!(b.now() >= a.now(), "后构造实例快照不早于先构造实例");
}

#[test]
fn seed_rng42_replays_identical_sequence_per_instance() {
    // 契约（Ω₂）:同种子必同序列,跨实例可复放
    let a = SeedRng42::new();
    let b = SeedRng42::new();
    let seq_a: Vec<u64> = (0..16).map(|_| a.next_u64()).collect();
    let seq_b: Vec<u64> = (0..16).map(|_| b.next_u64()).collect();
    assert_eq!(seq_a, seq_b, "SeedRng42 同种子必须逐项同序列");
    assert!(seq_a.windows(2).any(|w| w[0] != w[1]), "序列不得退化为常量");
}

#[test]
fn chacha8_rng_differs_by_seed_and_replays_by_seed() {
    // 契约:同种子可复放,异种子序列不同（确定性注入的前提）
    let s7a = ChaCha8Rng::new(7);
    let s7b = ChaCha8Rng::new(7);
    let s8 = ChaCha8Rng::new(8);
    let seq = |r: &ChaCha8Rng| -> Vec<u64> { (0..8).map(|_| r.next_u64()).collect() };
    assert_eq!(seq(&s7a), seq(&s7b), "同种子必须同序列");
    assert_ne!(seq(&s7b), seq(&s8), "异种子必须异序列");
}

#[test]
fn mem_fs_write_then_read_roundtrips_exact_bytes() {
    // 契约:覆盖写语义 + 逐字节回读（含空内容）
    let fs = MemFs::new();
    let p = Path::new("/mem/a.bin");
    fs.write(p, b"hello").expect("首次写入应成功");
    assert_eq!(fs.read(p).expect("读取应成功"), b"hello".to_vec());
    fs.write(p, b"hi").expect("覆盖写应成功");
    assert_eq!(
        fs.read(p).expect("读取应成功"),
        b"hi".to_vec(),
        "写为覆盖语义"
    );

    let empty = Path::new("/mem/empty.bin");
    fs.write(empty, b"").expect("空内容写入应成功");
    assert_eq!(fs.read(empty).expect("读取应成功"), Vec::<u8>::new());
}

#[test]
fn mem_fs_read_of_unknown_path_is_a_fail_closed_error() {
    // 契约:缺失路径必须报错,绝不返回空字节伪装成功
    let fs = MemFs::new();
    let err = fs
        .read(Path::new("/mem/absent.bin"))
        .expect_err("缺失路径必须失败");
    assert_eq!(err, SeamError::Unsupported("MemFs:path-not-found"));
}

#[test]
fn mem_fs_list_filters_by_prefix_and_ignores_other_dirs() {
    // 契约:list 只回本前缀下的条目（非递归语义由 starts_with 组件匹配承担）
    let fs = MemFs::new();
    fs.write(Path::new("/mem/x/a.txt"), b"a").unwrap();
    fs.write(Path::new("/mem/x/b.txt"), b"b").unwrap();
    fs.write(Path::new("/mem/y/c.txt"), b"c").unwrap();
    let mut listed: Vec<PathBuf> = fs.list(Path::new("/mem/x")).expect("list 应成功");
    listed.sort();
    assert_eq!(
        listed,
        vec![PathBuf::from("/mem/x/a.txt"), PathBuf::from("/mem/x/b.txt")],
        "不得混入其他目录条目"
    );
}
