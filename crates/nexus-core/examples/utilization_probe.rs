//! utilization_probe — WI-34 CPU 利用率测量管道示例（T1 建立,T14 复用,P5-T1 双侧化修正）
//!
//! 运行（近似口径）:`cargo run -p nexus-core --example utilization_probe`
//! 运行（busy 真实口径,D-Q4 局部 cfg）:
//!   `RUSTFLAGS="--cfg tokio_unstable" cargo run -p nexus-core --example utilization_probe`
//! 输出:JSON 快照（tokio 双口径探针 + rayon 池活跃探针 + 合成评分 + busy 证据）
//!
//! # P5-T1 双侧化修正（D-Q2/D-Q3,ADR-157）
//! 原探针负载 100% 走 rayon（spawn_compute_batch 单侧化）,tokio 侧采样窗口
//! 结构性零负载 → combined ≈ 0.5×0 + 0.5×1.0 ≈ 0.5,系 **measurement 管道
//! 缺陷**（非生产性能缺口）。本版补 **tokio 侧真实混合负载**（CPU-lite 自旋
//! 加 sleep IO 模拟,对齐生产 tokio 职责:E8-2 裁决 IO/异步主战场）,与 rayon
//! 批量负载并行运行,使 combined 口径两侧均有真实负载。
//!
//! # 诚实数据纪律（D-Q5）
//! 修正口径数字如实报告,**不预设 65% 达标线**;判定以 ADR-157 双口径三条件
//! 联合为准（rayon ≥0.9 + tokio busy 证据 + e2e ≥ 基线）。

use std::sync::Arc;
use std::time::Duration;

use nexus_core::compute::{bridge, TaskKind, UtilizationSampler, UtilizationSnapshot};

/// tokio 侧混合负载任务 — CPU-lite 自旋 + sleep IO 模拟（对齐生产 tokio 职责）
///
/// E8-2 裁决:IO/异步类走 tokio（禁 rayon）;本任务模拟「解析/校验（CPU-lite）
/// → IO 等待（sleep 模拟 LLM/磁盘）」生产典型形态。
async fn tokio_mixed_task(id: usize) -> usize {
    // CPU-lite 段:~2ms 计时自旋（解析/校验类轻计算;无算术 panic 面）
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(2) {
        std::hint::spin_loop();
    }
    // IO 模拟段:~1ms 挂起（让出 worker,模拟 LLM 调用/磁盘等待）
    tokio::time::sleep(Duration::from_millis(1)).await;
    id
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() {
    let sampler = UtilizationSampler::new();
    let b = bridge();
    // 诊断:活跃峰值跟踪（验证采样是否覆盖负载满载窗口）
    let peak_active = Arc::new(std::sync::atomic::AtomicU64::new(0));

    // 采样器后台任务:每 10ms 采样一次,持续到主批处理结束;
    // busy 差分:相邻两次 busy_us 差 / (workers × interval) = 窗口 busy 比率
    let peak_tracker = Arc::clone(&peak_active);
    let sampler_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        let mut last_busy_us = 0u64;
        let mut busy_ratio_sum = 0.0f64;
        let mut busy_ratio_n = 0usize;
        let mut busy_peak_ratio = 0.0f64;
        for _ in 0..300 {
            interval.tick().await;
            let snap = UtilizationSnapshot {
                sampled_at: std::time::Instant::now(),
                tokio: nexus_core::compute::TokioProbe::sample(),
                rayon: b.rayon_probe(),
            };
            // 峰值跟踪（诊断）
            let a = snap.rayon.active_tasks as u64;
            peak_tracker.fetch_max(a, std::sync::atomic::Ordering::Relaxed);
            // busy 差分（cfg 下 busy_us>0 才有意义;近似口径全 0 跳过）
            if let Some(t) = &snap.tokio {
                if t.busy_us > 0 && t.busy_us >= last_busy_us && t.num_workers > 0 {
                    let d_busy_us = t.busy_us - last_busy_us;
                    let window_us = t.num_workers as u64 * 10_000; // 10ms × workers
                    let ratio = (d_busy_us as f64 / window_us as f64).min(1.0);
                    busy_ratio_sum += ratio;
                    busy_ratio_n += 1;
                    busy_peak_ratio = busy_peak_ratio.max(ratio);
                }
                last_busy_us = t.busy_us;
            }
            sampler.record(&snap);
        }
        (
            sampler,
            if busy_ratio_n > 0 {
                busy_ratio_sum / busy_ratio_n as f64
            } else {
                0.0
            },
            busy_peak_ratio,
            busy_ratio_n,
        )
    });

    // tokio 侧负载:16 波 × 512 混合任务（CPU 2ms + IO 1ms）,与 rayon 波并行;
    // 总时长 ≈ 16×512×3ms / 8 workers ≈ 3s,覆盖采样窗口（300×10ms = 3s）
    let tokio_load = tokio::spawn(async move {
        let mut set = tokio::task::JoinSet::new();
        for wave in 0..16u32 {
            for i in 0..512u32 {
                let id = (wave as usize) * 512 + i as usize;
                set.spawn(tokio_mixed_task(id));
            }
        }
        let mut total = 0usize;
        while let Some(r) = set.join_next().await {
            total += r.unwrap_or(0);
        }
        total
    });

    // rayon 侧负载（既有）:8 波 × 1024 任务,每任务 ~5ms CPU 自旋
    for wave in 0..8u32 {
        let items: Vec<_> = (0..1024)
            .map(|i| {
                move || {
                    let start = std::time::Instant::now();
                    let mut acc = 0u64;
                    while start.elapsed() < Duration::from_millis(5) {
                        acc = acc.wrapping_add(i as u64);
                        std::hint::spin_loop();
                    }
                    acc
                }
            })
            .collect();
        let out = b.spawn_compute_batch(TaskKind::Generic, items);
        assert_eq!(out.len(), 1024, "批次结果数必须等于输入数");
        println!("wave {wave}: rayon batch 1024 done");
    }

    let tokio_total = tokio_load.await.expect("tokio 负载任务应正常完成");
    let (sampler, busy_mean, busy_peak, busy_n) = sampler_handle.await.expect("采样任务应正常完成");
    let samples = sampler.count();

    // 诊断:池线程数与负载期峰值活跃任务（验证探针正确性）
    println!(
        "probe: pool_threads={} peak_active_sampled={} peak_active_tracked={} rayon_side_peak={:.3} tokio_mixed_total={}",
        b.pool_threads(),
        b.pool_active_tasks(),
        peak_active.load(std::sync::atomic::Ordering::Relaxed),
        peak_active.load(std::sync::atomic::Ordering::Relaxed) as f64 / b.pool_threads() as f64,
        tokio_total,
    );

    // 输出测量报告（诚实数据:全部为实测值;busy 口径注明 cfg 状态）
    let final_snap = UtilizationSnapshot {
        sampled_at: std::time::Instant::now(),
        tokio: nexus_core::compute::TokioProbe::sample(),
        rayon: b.rayon_probe(),
    };
    let busy_real = busy_n > 0;
    println!(
        "{{\"machine\":\"16-core-host\",\"tokio_workers\":{},\"rayon_pool_threads\":{},\
\"samples\":{},\"mean_utilization\":{:.3},\"peak_utilization\":{:.3},\
\"tokio_busy_mean\":{:.3},\"tokio_busy_peak\":{:.3},\"busy_samples\":{},\"busy_real\":{},\
\"dual_sided_load\":true,\
\"final\":{{\"tokio_alive_tasks\":{},\"tokio_queue_depth\":{},\"rayon_active\":{}}},\
\"approx_note\":\"busy_real=true 时 tokio busy 为 tokio_unstable 真实口径（D-Q4 局部 cfg）;\
false 时存活任务近似（RK-Q1 降级）;判定以 ADR-157 双口径三条件联合为准（D-Q5）\"}}",
        final_snap.tokio.map_or(0, |t| t.num_workers),
        b.pool_threads(),
        samples,
        sampler.mean(),
        sampler.peak(),
        busy_mean,
        busy_peak,
        busy_n,
        busy_real,
        final_snap.tokio.map_or(0, |t| t.num_alive_tasks),
        final_snap.tokio.map_or(0, |t| t.global_queue_depth),
        final_snap.rayon.active_tasks,
    );
}
