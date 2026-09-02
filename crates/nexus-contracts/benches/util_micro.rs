//! util_micro 基准 — L0 共享纯函数收敛后的性能证据链(第三轮冗余审计 批 B/C)
//!
//! 覆盖 `nexus_contracts::util::{sigmoid, percentile_sorted}` 两个新收敛的共享工具,
//! 目标是把"收敛未引入回退"从主观判断变成可证伪数据:
//!
//! 1. **响应时间 / CPU 时间**:criterion 中位数 ns 级测量。
//! 2. **内存**:`CountingAlloc` 包装 `System` 分配器,统计单次调用的堆分配次数与字节数。
//!    两者都应恒为 0 —— `sigmoid` 是标量运算,`percentile_sorted` 只索引已排序切片。
//!    这条断言取代了"看起来不会分配内存"的口头保证:一旦后续改动引入隐式 `to_vec`
//!    或插值克隆,分配计数立即非零。
//!
//! WHY 不 bench 排序本身:`percentile_sorted` 契约要求调用方传入已排序切片(排序是
//! O(n log n),属调用方策略而非取分位这一步)。这里只测取分位的 O(1) 成本。

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use nexus_contracts::util::{percentile_sorted, sigmoid};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// 包装 `System` 并累计分配次数/字节数的计数分配器
struct CountingAlloc;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

/// 取 `(分配次数, 字节数)` 快照
fn alloc_snapshot() -> (u64, u64) {
    (
        ALLOCS.load(Ordering::Relaxed),
        BYTES.load(Ordering::Relaxed),
    )
}

/// 断言 `f` 执行期间零堆分配,返回耗时用于对照
///
/// WHY 在 bench 内做一次硬断言而非只报数:非零分配意味着收敛后的共享实现
/// 悄悄引入了堆操作,那会让所有调用点(6 个 p95 红线测试)一起承担额外开销。
fn assert_zero_alloc<T>(label: &str, mut f: impl FnMut() -> T) -> T {
    let (a0, b0) = alloc_snapshot();
    let out = f();
    let (a1, b1) = alloc_snapshot();
    assert_eq!(
        a1 - a0,
        0,
        "{label}: 期望零堆分配,实际 {} 次 / {} 字节",
        a1 - a0,
        b1 - b0
    );
    out
}

fn sorted_durations(n: usize) -> Vec<Duration> {
    // 确定性伪随机(LCG)后排序,使 bench 可复现且不把排序成本混进被测步
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut v: Vec<Duration> = (0..n)
        .map(|_| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            Duration::from_nanos(seed % 5_000_000)
        })
        .collect();
    v.sort_unstable();
    v
}

fn bench_sigmoid(c: &mut Criterion) {
    assert_zero_alloc("sigmoid(单次)", || sigmoid(black_box(0.75_f32)));

    // 单次调用:置信度映射的最小粒度
    c.bench_function("sigmoid scalar", |b| {
        b.iter(|| black_box(sigmoid(black_box(0.75_f32))))
    });

    // 1024 次批量:GEA 门控 / MAPPO 策略头这类逐元素映射的真实使用形态
    let xs: Vec<f32> = (0..1024).map(|i| i as f32 * 0.01 - 5.0).collect();
    c.bench_function("sigmoid map 1024", |b| {
        b.iter(|| {
            let sum: f32 = xs.iter().map(|&x| sigmoid(black_box(x))).sum();
            black_box(sum)
        })
    });
}

fn bench_percentile_sorted(c: &mut Criterion) {
    for n in [100usize, 10_000] {
        let data = sorted_durations(n);

        assert_zero_alloc("percentile_sorted", || {
            percentile_sorted(&data, black_box(0.95))
        });

        // 取分位本身是 O(1):与 n 无关,bench 两组 n 正是为了证明这条复杂度结论
        c.bench_function(&format!("percentile_sorted p95 n={n}"), |b| {
            b.iter(|| black_box(percentile_sorted(&data, black_box(0.95))))
        });
    }
}

criterion_group!(benches, bench_sigmoid, bench_percentile_sorted);
criterion_main!(benches);
