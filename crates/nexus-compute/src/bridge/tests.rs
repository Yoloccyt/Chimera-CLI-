use super::*;
use std::sync::atomic::Ordering;

use crate::dispatch::TaskKind;
use crate::hts::ThresholdSource;
use crate::telemetry::SaturationMode;
use crate::utilization::ActiveCounter;

/// R3-2b(ADR-262):`CHIMERA_COMPUTE_THREADS` 覆盖表驱动（env 变更集中单测,
/// 结束时恢复;非法值回退既有口径 = 未设时的 compute_threads() 读数）
#[test]
fn compute_threads_env_override_matrix() {
    let saved = std::env::var("CHIMERA_COMPUTE_THREADS").ok();
    std::env::remove_var("CHIMERA_COMPUTE_THREADS");
    let default_threads = compute_threads();
    assert!(default_threads >= MIN_THREADS);
    let cases: &[(&str, Option<usize>)] = &[
        ("4", Some(4)),
        (" 6 ", Some(6)),
        ("1", None), // < MIN_THREADS → 非法回退
        ("0", None),
        ("-1", None),
        ("abc", None),
    ];
    for (raw, want) in cases {
        std::env::set_var("CHIMERA_COMPUTE_THREADS", raw);
        assert_eq!(
            compute_threads(),
            want.unwrap_or(default_threads),
            "input={raw:?}"
        );
    }
    match saved {
        Some(v) => std::env::set_var("CHIMERA_COMPUTE_THREADS", v),
        None => std::env::remove_var("CHIMERA_COMPUTE_THREADS"),
    }
}

/// bridge() 单例幂等 — 两次调用返回同一指针（OnceLock 语义）
#[test]
fn bridge_is_singleton() {
    let a = bridge();
    let b = bridge();
    assert!(std::ptr::eq(a, b));
}

/// 计算池线程数 — 恒 >= 下限 2
#[test]
fn compute_threads_at_least_two() {
    assert!(compute_threads() >= MIN_THREADS);
}

/// decide 纯函数 — 三态全分支（含阈值边界）
#[test]
fn decide_three_way() {
    let b = bridge();
    // Inline: n < 阈值
    assert_eq!(b.route(TaskKind::Generic, 9_999), DispatchPlan::Inline);
    // Rayon: n == 阈值(边界)与 n > 阈值
    assert_eq!(b.route(TaskKind::Generic, 10_000), DispatchPlan::Rayon);
    assert_eq!(b.route(TaskKind::Generic, 10_001), DispatchPlan::Rayon);
    // Async: io_bound 优先(与 n 无关)
    assert_eq!(
        decide(TaskKind::Generic, true, 10_000, 0),
        DispatchPlan::Async
    );
    assert_eq!(
        decide(TaskKind::Generic, true, 10_000, usize::MAX),
        DispatchPlan::Async
    );
}

/// Cancelled 映射 — oneshot 接收端在发送端失效时以 Err 收场
#[tokio::test]
async fn cancelled_mapping() {
    let (tx, rx) = tokio::sync::oneshot::channel::<u64>();
    drop(tx);
    let mapped: Result<u64, ComputeError> = rx.await.map_err(|_| ComputeError::Cancelled);
    assert_eq!(mapped, Err(ComputeError::Cancelled));
}

/// spawn_compute_batch — 结果数与输入一致,顺序对应,无 panic
#[tokio::test]
async fn spawn_compute_batch_count_and_order() {
    let n = 32;
    let items: Vec<_> = (0..n).map(|i| move || i * 2).collect();
    let out = bridge().spawn_compute_batch(TaskKind::Generic, items);
    assert_eq!(out.len(), n, "结果数必须等于输入数");
    for (idx, r) in out.iter().enumerate() {
        assert_eq!(
            r.as_ref().expect("批量任务应成功"),
            &(idx * 2),
            "顺序必须对应输入"
        );
    }
}

/// spawn_compute_batch — 空输入返回空结果
#[test]
fn spawn_compute_batch_empty() {
    // F = 函数指针,显式标注类型以避免空迭代器无法推断 T
    let out: Vec<Result<u64, ComputeError>> =
        bridge().spawn_compute_batch(TaskKind::Generic, std::iter::empty::<fn() -> u64>());
    assert!(out.is_empty());
}

/// spawn_compute_batch — 单任务 panic 不影响同批其他任务
#[test]
fn spawn_compute_batch_panic_isolated() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let items: Vec<Box<dyn FnOnce() -> u64 + Send>> =
        vec![Box::new(|| 1), Box::new(|| panic!("boom")), Box::new(|| 3)];
    let out = bridge().spawn_compute_batch(TaskKind::Generic, items);
    std::panic::set_hook(prev);
    assert_eq!(out.len(), 3);
    assert!(matches!(&out[0], Ok(1)));
    assert_eq!(out[1], Err(ComputeError::Panicked));
    assert!(matches!(&out[2], Ok(3)));
}

/// 并发 route — 8 线程 × 1000 次无 panic 无数据竞争
///
/// WHY 而非 loom L-01：原注称「loom 在 Windows GNU 不适用/Windows 支持受限」，
/// **该断言已被 R2 C-102 实测推翻**：loom 0.7.2 在 `stable-x86_64-pc-windows-gnu`
/// 上编过（59.9s）且 `loom::model` 能枚举出丢更新交错（证据：`tmp/loom_probe`）。
/// 不用它的真实理由是**工具-属性匹配**：`DispatchPlan` 是 Copy 纯枚举 + 静态阈值表，
/// 并发读天然无竞争——loom 的排列枚举在这里没东西可枚举，
/// 本测试只需锁「并发调用 route 行为一致」。
#[test]
fn route_concurrent_reads() {
    let b = bridge();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(move || {
                for i in 0..1_000usize {
                    for kind in TaskKind::ALL {
                        let plan = b.route(kind, i);
                        let t = kind.threshold();
                        assert_eq!(plan, decide(kind, kind.is_io_bound(), t, i));
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("并发 route 线程应正常退出");
    }
}

/// update_threshold 联动 — route() 读动态表:阈值 1000→10 后 20 项 Inline→Rayon
///
/// WHY 独立实例而非全局 `bridge()`:全局单例表被并行测试共享,直接修改会与
/// 其他测试（如本文件 `route_concurrent_reads` 的静态阈值断言）竞争;
/// 独立实例在私有字段可访问处构造（同模块测试）,零全局副作用。
#[test]
fn update_threshold_changes_route() {
    let b = ComputeBridge::test_bridge(2);
    let kind = TaskKind::ClvSimilarity;
    // §8.4 初值:阈值 1000 → 20 项走 Inline
    assert_eq!(b.route(kind, 20), DispatchPlan::Inline);
    // T1/T14 校准:阈值降至 10 → 20 项转 Rayon,9 项仍 Inline
    b.update_threshold(kind, 10, 64, ThresholdSource::ConservativeDefault);
    assert_eq!(
        b.route(kind, 20),
        DispatchPlan::Rayon,
        "阈值降低后 route 必须跟随动态表"
    );
    assert_eq!(b.route(kind, 9), DispatchPlan::Inline);
    // 未更新的类别不受影响
    assert_eq!(b.route(TaskKind::KnnSearch, 5_000), DispatchPlan::Rayon);
}

/// 并发 update_threshold 无丢更新 — 锁住旧实现 load→clone→store 的非原子竞态
///
/// WHY:旧实现裸 `store` 整表替换,多线程并发写**各不同的 kind**时会互相覆盖
/// (后写者基于自己早先 load 的快照,抹掉先写者对其他 kind 的更新)。distinct-kind
/// 设计不依赖各 kind 默认值、断言稳健:每个 kind 的哨兵写入都必须存活。CAS 环
/// (`rc_swap`)失败即基于最新快照重放,保证全部写入存活。本测在旧实现有概率红、
/// 新实现必绿(跨 kind 丢失更新被 CAS 消除)。用 `test_bridge` 独立实例避全局单例竞争。
#[test]
fn update_threshold_concurrent_no_lost_update() {
    let b = Arc::new(ComputeBridge::test_bridge(4));
    const SENTINEL: usize = 12_345;
    let handles: Vec<_> = TaskKind::ALL
        .iter()
        .copied()
        .map(|kind| {
            let b = Arc::clone(&b);
            std::thread::spawn(move || {
                b.update_threshold(kind, SENTINEL, 64, ThresholdSource::ConservativeDefault);
            })
        })
        .collect();
    for h in handles {
        h.join().expect("并发 update_threshold 线程应正常退出");
    }
    for kind in TaskKind::ALL {
        assert_eq!(
            b.threshold_of(kind),
            SENTINEL,
            "{kind:?} 的阈值更新被他线程覆盖(lost update)"
        );
    }
}

/// 池探针语义 — 批量任务执行期间活跃数 > 0,完成后归零
///
/// WHY 独立实例而非全局 `bridge()`:全局单例池被并行测试共享（如
/// `pool_active_probe_concurrent` 的在途任务会污染 active 计数断言）;
/// 独立实例构造私有字段（同模块测试可见）,零全局副作用。
#[test]
fn pool_active_probe_lifecycle() {
    let b = ComputeBridge::test_bridge(2);
    assert_eq!(b.pool_active_tasks(), 0, "空闲池活跃数必须为 0");
    assert_eq!(b.pool_threads(), 2);
    let n = 16;
    let items: Vec<_> = (0..n)
        .map(|i| {
            move || {
                // 模拟 CPU 任务:自旋一小段时间保证探针采样窗口可见
                let start = std::time::Instant::now();
                while start.elapsed() < std::time::Duration::from_millis(2) {
                    std::hint::spin_loop();
                }
                i
            }
        })
        .collect();
    let probe_before = b.rayon_probe();
    let out = b.spawn_compute_batch(TaskKind::Generic, items);
    let probe_after = b.rayon_probe();
    assert_eq!(out.len(), n);
    assert_eq!(probe_before.active_tasks, 0);
    assert_eq!(probe_after.active_tasks, 0, "批量完成后活跃数必须归零");
    assert_eq!(b.pool_active_tasks(), 0);
}

/// 池探针并发可见性 — 8 并发 batch 交叉执行时峰值计数可达 >0
///
/// WHY 用 `Arc<test_bridge>` 而不拿全局 `bridge()`:同文件
/// [`pool_active_probe_lifecycle`] 的注释早点名“全局单例被并行测试共享会污染
/// active 计数断言”——该污染是真实的（2026-10-02 全量跑中出现
/// “活跃数应为 0、实际为 1”的偶发失败）。独立实例 + Arc 既让 8 线程共享同一个桥,
/// 又与其他测试零串扰。
///
/// 同时补上本测试名义上宣称却从未断言的语义：并发期间“峰值 > 0”。
#[test]
fn pool_active_probe_concurrent() {
    let b = Arc::new(ComputeBridge::test_bridge(4));
    let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let b = Arc::clone(&b);
            let peak = Arc::clone(&peak);
            std::thread::spawn(move || {
                let items: Vec<_> = (0..8)
                    .map(|i| {
                        // 每个任务闭包各自持一份桥与峰值计数器的句柄（'static 要求）
                        let b = Arc::clone(&b);
                        let peak = Arc::clone(&peak);
                        move || {
                            // 先记录在途峰值（本测试要断言的并发可见性）,再自旋一小段
                            peak.fetch_max(b.pool_active_tasks(), Ordering::Relaxed);
                            let start = std::time::Instant::now();
                            while start.elapsed() < std::time::Duration::from_millis(1) {
                                std::hint::spin_loop();
                            }
                            i
                        }
                    })
                    .collect();
                let out = b.spawn_compute_batch(TaskKind::Generic, items);
                assert_eq!(out.len(), 8);
            })
        })
        .collect();
    for h in handles {
        h.join().expect("并发线程应正常退出");
    }
    assert!(
        peak.load(Ordering::Relaxed) > 0,
        "并发执行期间活跃峰值应 > 0（实测 {}）",
        peak.load(Ordering::Relaxed)
    );
    assert_eq!(b.pool_active_tasks(), 0, "全部并发任务完成后活跃数归零");
}

/// ComputeError Display — 三变体文案符合 §11.1 契约
#[test]
fn compute_error_display() {
    assert_eq!(
        ComputeError::Panicked.to_string(),
        "task panicked in compute pool"
    );
    assert_eq!(
        ComputeError::Cancelled.to_string(),
        "cancelled before completion"
    );
    assert_eq!(
        ComputeError::Saturated.to_string(),
        "pool saturated, retryable"
    );
}

/// reduce 契约 — §11.1:bridge 委托 reduce 模块,双模式结果逐位一致
#[test]
fn bridge_reduce_delegates() {
    let vals = [1.0, -2.0, 0.5, 1e-3, -7.0e-9];
    for mode in [
        super::super::reduce::ReduceMode::Deterministic,
        super::super::reduce::ReduceMode::Audit,
    ] {
        let via_bridge = bridge().reduce(&vals, mode);
        let direct = super::super::reduce::reduce(&vals, mode);
        assert_eq!(
            via_bridge.to_bits(),
            direct.to_bits(),
            "bridge reduce({mode:?}) 必须与 reduce 模块直调逐位一致"
        );
    }
}

// ============================================================
// T16 Compute 契约不变式(手册 §11.1)
// ============================================================

/// 契约不变式(§11.1):route 是纯函数 — 同一 (kind,n_items) 多次调用结果
/// 确定性一致(无随机/时间/副作用依赖),纳秒级查表三态不抖动。
#[test]
fn compute_invariant_route_deterministic_pure() {
    let b = bridge();
    // 全任务类型 + 边界规模:同一输入重复调用必须返回同一 DispatchPlan
    for kind in TaskKind::ALL {
        for n in [0usize, 1, 999, 10_000, usize::MAX] {
            let a = b.route(kind, n);
            assert_eq!(
                b.route(kind, n),
                a,
                "route({kind:?},{n}) 必须确定性一致(纯查表,pure)"
            );
        }
    }
    // 阈值边缘稳定:Inline 判定不因合并/并发读而抖到 Rayon
    assert_eq!(b.route(TaskKind::Generic, 0), DispatchPlan::Inline);
}

/// 契约不变式(§11.1):reduce(Deterministic) 同一构建内多次调用逐位一致
/// (跨构建容差 ≤ 1e-6 的编译内锚点 —— 确定性归约不可累积或乱序)。
#[test]
fn compute_invariant_reduce_deterministic_bitwise() {
    let vals = [1.0, -2.0, 0.5, 1e-3, -7.0e-9, 1e8, 1.337, 0.0];
    for mode in [
        super::super::reduce::ReduceMode::Deterministic,
        super::super::reduce::ReduceMode::Audit,
    ] {
        let first = bridge().reduce(&vals, mode).to_bits();
        for _ in 0..16 {
            assert_eq!(
                bridge().reduce(&vals, mode).to_bits(),
                first,
                "reduce({mode:?}) 必须逐位确定(不变式:同输入同模式同输出)"
            );
        }
    }
}

// ============================================================
// A1/ADR-197:饱和降级与严格模式行为
//
// WHY 用实例级注入缝（force_saturation + 人工占池）而不改 env:
// 阈值与严格模式的 env 缓存是进程级 OnceLock,并行测试下会彼此干扰;
// 实例字段只在当前测试拥有的桥内生效,结果确定。
// ============================================================

/// 构造一个可被测试占满的桥:返回（桥, 活跃计数器副本）
///
/// WHY 分开返回而不返回 guard:`enter()` 返回的守卫借用计数器,
/// 无法与按值返回的桥同存（自引用）;调用方在本地持有 `Arc` 副本再 `enter()`,
/// 既占住活跃数又不产生跨所有权 borrow。
fn saturated_bridge(mode: SaturationMode) -> (ComputeBridge, Arc<ActiveCounter>) {
    let b = ComputeBridge::test_bridge(2);
    b.force_saturation(1_000, mode);
    let counter = b.active_handle();
    (b, counter)
}

/// A1 风险缓解的核心用例：默认 Observe 下只记观测，**分支不变**（任务仍走池）
#[test]
fn observe_mode_records_saturation_without_changing_branch() {
    let (b, counter) = saturated_bridge(SaturationMode::Observe);
    let _guards = (counter.enter(), counter.enter());
    let results = b.spawn_compute_batch(TaskKind::KnnSearch, (0..4u32).map(|i| move || i + 1));
    let vals: Vec<u32> = results.into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(vals, vec![1, 2, 3, 4], "Observe 下结果必须与平时一致");
    assert_eq!(b.kind_counters().saturations(), 1, "超阈必须被观测到");
    assert_eq!(
        b.kind_counters().degradations(),
        0,
        "Observe 必须零行为变更:不能把观测误记为降级"
    );
    assert_eq!(b.kind_counters().rejections(), 0);
}

/// spawn_compute_batch Degrade — 饱和时整批 Inline 执行,结果逐元素不变,降级计数+1
///（与 spawn_compute 的 async Degrade 测同构,补 batch 入口覆盖，为 S-2-B 前置）。
#[test]
fn saturated_spawn_compute_batch_degrades_to_inline_with_same_results() {
    let (b, counter) = saturated_bridge(SaturationMode::Degrade);
    let _guards = (counter.enter(), counter.enter());
    assert!(b.is_saturated(), "占池后必须判为饱和");
    let results = b.spawn_compute_batch(TaskKind::KnnSearch, (0..4u32).map(|i| move || i * 2));
    let vals: Vec<u32> = results
        .into_iter()
        .map(|r| r.expect("降级方向=变慢不变错"))
        .collect();
    assert_eq!(vals, vec![0, 2, 4, 6], "降级结果必须与走池逐元素一致");
    assert_eq!(b.kind_counters().degradations(), 1, "整批降级计一次");
    assert_eq!(b.kind_counters().saturations(), 1, "超阈观测与降级分开计数");
    assert_eq!(b.kind_counters().rejections(), 0, "Degrade 非拒绝");
}

/// spawn_compute_batch Strict — 饱和时逐任务 Err(Saturated),任何闭包不执行,拒绝计数+1
#[test]
fn strict_pool_batch_rejects_all_without_running() {
    let (b, counter) = saturated_bridge(SaturationMode::Strict);
    let _guards = (counter.enter(), counter.enter());
    let executed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let flag = Arc::clone(&executed);
    let results = b.spawn_compute_batch(
        TaskKind::OsaMask,
        (0..3u32).map(move |_| {
            let flag = Arc::clone(&flag);
            move || {
                flag.fetch_add(1, Ordering::SeqCst);
                1u32
            }
        }),
    );
    assert_eq!(
        results,
        vec![Err(ComputeError::Saturated); 3],
        "严格模式整批逐任务报饱和"
    );
    assert_eq!(
        executed.load(Ordering::SeqCst),
        0,
        "严格模式语义=不执行任何闭包"
    );
    assert_eq!(b.kind_counters().rejections(), 1, "一次批量调用计一次拒绝");
    assert_eq!(b.kind_counters().degradations(), 0, "拒绝与降级分开计数");
}

/// spawn_compute_batch panic 隔离 — 单任务 panic 转 Err(Panicked),同批其它任务不受影响
#[test]
fn spawn_compute_batch_panic_isolated_per_item() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {})); // 静音预期 panic,避免污染测试输出
    let results = bridge().spawn_compute_batch(
        TaskKind::Generic,
        (0..3u64).map(|i| move || if i == 1 { panic!("boom") } else { i }),
    );
    std::panic::set_hook(prev);
    assert_eq!(results.len(), 3, "结果数恒等于输入数");
    assert_eq!(results[0], Ok(0), "panic 前项正常");
    assert_eq!(
        results[1],
        Err(ComputeError::Panicked),
        "panic 隔离为错误值,不跨线程传播"
    );
    assert_eq!(results[2], Ok(2), "panic 后项不受影响");
}

#[test]
fn saturated_batch_keeps_input_order_and_values() {
    let (b, counter) = saturated_bridge(SaturationMode::Degrade);
    let _guards = (counter.enter(), counter.enter());
    let results = b.spawn_compute_batch(TaskKind::Generic, (0..8u32).map(|i| move || i * 2));
    let vals: Vec<u32> = results.into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(
        vals,
        vec![0, 2, 4, 6, 8, 10, 12, 14],
        "结果序必须等于输入序"
    );
    assert_eq!(
        b.kind_counters().degradations(),
        1,
        "一批只记一次降级(批粒度)"
    );
}

/// 第三轮 T2.2:降级项数与批数**双口径**独立计数（项粒度 vs 批粒度）；
/// Observe/Strict 不得误记项数（只 Degrade 分支才 +n）。
#[test]
fn degraded_items_counts_items_not_batches() {
    // Degrade 一批 6 项:批粒度 +1、项粒度 +6
    let (b, counter) = saturated_bridge(SaturationMode::Degrade);
    let _guards = (counter.enter(), counter.enter());
    let results = b.spawn_compute_batch(TaskKind::KnnSearch, (0..6u32).map(|i| move || i));
    assert_eq!(results.len(), 6, "结果数不变(降级仅换执行路径)");
    assert_eq!(b.saturation_degradations(), 1, "批粒度仍计一批");
    assert_eq!(
        b.saturation_degraded_items(),
        6,
        "项粒度计六项(T2.2 独立计数)"
    );
    // 二批再降级 3 项:批 +1(共 2)、项 +3(共 9)——两口径互不可互推
    let _ = b.spawn_compute_batch(TaskKind::KnnSearch, (0..3u32).map(|i| move || i));
    assert_eq!(b.saturation_degradations(), 2, "两批→批粒度 2");
    assert_eq!(b.saturation_degraded_items(), 9, "6+3→项粒度 9");
}

/// T2.2 反侧:Observe/Strict 不降级，故 degraded_items 恒为 0
#[test]
fn observe_and_strict_do_not_count_degraded_items() {
    let (o, oc) = saturated_bridge(SaturationMode::Observe);
    let _og = (oc.enter(), oc.enter());
    let _ = o.spawn_compute_batch(TaskKind::KnnSearch, (0..4u32).map(|i| move || i));
    assert_eq!(
        o.saturation_degraded_items(),
        0,
        "Observe 不降级→项计数不动"
    );
    let (s, sc) = saturated_bridge(SaturationMode::Strict);
    let _sg = (sc.enter(), sc.enter());
    let _ = s.spawn_compute_batch(TaskKind::KnnSearch, (0..4u32).map(|i| move || i));
    assert_eq!(
        s.saturation_degraded_items(),
        0,
        "Strict 拒绝不执行→项计数不动"
    );
}

#[test]
fn strict_batch_reports_saturated_per_item() {
    let (b, counter) = saturated_bridge(SaturationMode::Strict);
    let _guards = (counter.enter(), counter.enter());
    let results = b.spawn_compute_batch(TaskKind::KnnSearch, (0..5u32).map(|i| move || i));
    assert_eq!(results.len(), 5);
    assert!(
        results.iter().all(|r| *r == Err(ComputeError::Saturated)),
        "逐项报错而不缩减结果数(调用方索引对齐契约)"
    );
}

#[test]
fn unsaturated_bridge_does_not_degrade() {
    let b = ComputeBridge::test_bridge(2);
    assert!(!b.is_saturated(), "空闲桥不得判为饱和");
    let results = b.spawn_compute_batch(TaskKind::GsoeEvaluate, (0..4u32).map(|i| move || i + 1));
    let vals: Vec<u32> = results.into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(vals, vec![1, 2, 3, 4]);
    assert_eq!(b.kind_counters().degradations(), 0);
    assert_eq!(b.kind_counters().saturations(), 0, "未超阈不得记观测");
}

#[test]
fn inline_panic_is_counted_as_failure_not_zero_value() {
    let (b, counter) = saturated_bridge(SaturationMode::Degrade);
    let _guards = (counter.enter(), counter.enter());
    let got = b.spawn_compute_batch(
        TaskKind::CscCollapseScore,
        vec![
            Box::new(|| -> u32 { 1 }) as Box<dyn FnOnce() -> u32 + Send>,
            Box::new(|| -> u32 { panic!("注入池内 panic") }) as Box<dyn FnOnce() -> u32 + Send>,
        ],
    );
    assert_eq!(got[0], Ok(1), "同批其他任务不受影响");
    assert_eq!(
        got[1],
        Err(ComputeError::Panicked),
        "失败必须以 Err 暴露,不得被折算成合法值(审计 S-3)"
    );
    assert_eq!(b.kind_counters().failures_of(TaskKind::CscCollapseScore), 1);
}
