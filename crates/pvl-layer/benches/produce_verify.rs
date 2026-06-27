//! PVL 生产验证性能基准 — criterion 基准测试
//!
//! 对应 SubTask 25.6:测量 10/50/100 操作流式生产验证延迟
//!
//! 运行方式:
//! ```powershell
//! cargo bench -p pvl-layer
//! ```

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use pvl_layer::{FeedbackChannel, Operation, Producer, PvlConfig, Verifier};
use tokio::sync::mpsc;

/// 基准测试:不同规模操作流式生产验证的延迟
///
/// 测量 10/50/100 个操作经 PVL 流式生产→验证→反馈的完整耗时,
/// 包含 mpsc 通道传输 + 事件发布开销。
///
/// WHY 完整流程:基准应覆盖 Producer→Verifier→Feedback 全链路,
/// 而非仅测 Producer 或 Verifier 单端,以反映真实负载
fn bench_produce_verify(c: &mut Criterion) {
    // 创建 tokio runtime 供异步基准使用
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("produce_verify");
    group.sample_size(30); // 降低样本数以加速基准(默认 100)

    for size in [10usize, 50, 100] {
        group.bench_with_input(format!("{size}_ops"), &size, |b, &size| {
            b.to_async(&rt).iter(|| async {
                let bus = EventBus::new();
                let config = PvlConfig::default();

                let producer = Producer::new(config.clone(), bus.clone());
                let verifier = Verifier::new(config.clone(), bus.clone());
                let feedback = FeedbackChannel::new(config, bus);

                let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
                let (fb_tx, mut fb_rx) = mpsc::channel::<pvl_layer::FeedbackMessage>(128);

                // 启动验证者(后台任务)
                let verifier_handle = tokio::spawn(async move {
                    let mut rx = op_rx;
                    verifier.run(&mut rx, &fb_tx).await
                });

                // 生产操作
                producer.produce("bench-quest", size, &op_tx).await.unwrap();
                drop(op_tx);

                // 处理反馈
                while let Some(fb) = fb_rx.recv().await {
                    feedback.process_feedback(fb);
                }

                // 等待验证者完成
                verifier_handle.await.unwrap().unwrap();
            });
        });
    }

    group.finish();
}

/// 基准测试:仅 Producer 生产延迟(隔离 Producer 开销)
///
/// 测量 Producer 生成 N 个操作并通过通道发送的耗时,
/// 不含 Verifier 验证,用于隔离 Producer 性能瓶颈
fn bench_produce_only(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("produce_only");
    group.sample_size(30);

    for size in [10usize, 50, 100] {
        group.bench_with_input(format!("{size}_ops"), &size, |b, &size| {
            b.to_async(&rt).iter(|| async {
                let producer = Producer::new(PvlConfig::default(), EventBus::new());
                let (tx, mut rx) = mpsc::channel::<Operation>(128);

                producer.produce("bench-produce", size, &tx).await.unwrap();
                drop(tx);

                // 接收所有操作以避免通道积压影响测量
                while rx.recv().await.is_some() {}
            });
        });
    }

    group.finish();
}

/// 基准测试:仅 Verifier 验证延迟(隔离 Verifier 开销)
///
/// 测量 Verifier 验证 N 个操作的耗时,
/// 不含 Producer 生产,用于隔离 Verifier 性能瓶颈
fn bench_verify_only(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("verify_only");
    group.sample_size(30);

    for size in [10usize, 50, 100] {
        group.bench_with_input(format!("{size}_ops"), &size, |b, &size| {
            b.to_async(&rt).iter(|| async {
                let verifier = Verifier::new(PvlConfig::default(), EventBus::new());
                let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
                let (fb_tx, mut fb_rx) = mpsc::channel::<pvl_layer::FeedbackMessage>(128);

                // 预填充操作
                use pvl_layer::OperationId;
                for i in 0..size {
                    let mut op = Operation::new(
                        OperationId::new(format!("op-{i}")),
                        "bench-verify",
                        format!("content-{i}"),
                    );
                    op.mark_produced(0.8);
                    op_tx.send(op).await.unwrap();
                }
                drop(op_tx);

                // 启动验证者
                let handle = tokio::spawn(async move {
                    let mut rx = op_rx;
                    verifier.run(&mut rx, &fb_tx).await
                });

                // 接收所有反馈
                while fb_rx.recv().await.is_some() {}

                handle.await.unwrap().unwrap();
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_produce_verify,
    bench_produce_only,
    bench_verify_only
);
criterion_main!(benches);
