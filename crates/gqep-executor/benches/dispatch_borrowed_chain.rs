//! 链级「拥有型 vs 借用型」同轮对照基准（W4 / F-04 / ADR-249）
//!
//! # 设计（deepdive §3.6）
//!
//! - **同一条生产装配链**：`ToolRuntime::new(Arc::new(EchoExecutor), allow_policy)`
//!   （双参形态，runtime.rs 既有签名）；owned 与 borrowed 臂共用同一实例，
//!   交替插入同一 `BenchmarkGroup`（同机同进程同轮，禁跨轮对比）。
//! - 6 测点 = `{owned, borrowed} × {c1, c4, c8}`：c1 = 串行基线；c4/c8 用
//!   `Semaphore` 有界并发 harness；**入参形态是唯一变量**。
//! - borrowed 臂的 `DispatchRequestRef` 构造在计时区**外**（借自循环外拥有
//!   缓冲）——测的是派发，不是视图构造。
//! - 首轮冷启动丢弃（ADR-234 冷启动噪声 29-33%）。
//!
//! # 采纳判据（ADR-249 硬门）
//!
//! 每对 `(owned_cX, borrowed_cX)`：`borrowed_median ≤ 0.95 × owned_median`
//! **且** 两组 median CI 不相交；三对全部满足才采纳，否则按 ADR-241 D5
//! 否决并保留原 owned 升级实现（否决亦为正式结论）。
//!
//! 预期（deepdive §3.4 静态清单）：借用臂 10→7 分配/派发（-30% 分配数）；
//! owned 臂因 `exec_tool_executor.rs:68` 共用切换预期 **-1 分配（轻微改善
//! 方向，非零变化护栏）**。
//!
//! 运行：`cargo bench -p gqep-executor --bench dispatch_borrowed_chain`
#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::Arc;

use async_trait::async_trait;
use criterion::{criterion_group, criterion_main, Criterion};
use futures::stream::{FuturesUnordered, StreamExt};
use gqep_executor::tool_runtime::ToolRuntime;
use gqep_executor::ToolExecutor;
use seccore::execpolicy::{ExecPolicy, ExecPolicyRule, PolicyAction, RulePattern};
use tokio::sync::Semaphore;

/// 回显执行器 — 零 I/O，隔离链本身开销
struct EchoExecutor;

#[async_trait]
impl ToolExecutor for EchoExecutor {
    async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
        Ok(args_json.to_string())
    }
}

fn allow_policy() -> Arc<ExecPolicy> {
    Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
        pattern: RulePattern::new("bench.echo", "*"),
        action: PolicyAction::Allow,
    }))
}

/// 固定短名 + 定长 args_json（≈120B，落在等价测试 `.{0,120}` 域内）
fn request() -> gqep_executor::tool_runtime::DispatchRequest {
    gqep_executor::tool_runtime::DispatchRequest {
        tool_name: "bench.echo".to_string(),
        args_json: "x".repeat(112) + "\"}",
    }
}

/// 有界并发派发 N 次的 harness（c1 = 串行语义：并发上界 1）
///
/// WHY 「填满即 poll、完成即补」的单循环而非 for 循环内逐个 acquire：
/// `FuturesUnordered::push` 不会 poll 新推入的 future（首次 poll 发生在
/// `set.next().await`），for 循环会在第 conc+1 次 `acquire_owned().await`
/// 上死锁——前 conc 个许可被未启动的 future 永久持有（本 bench 首版实证，
/// 表现为 warm-up 卡死 CPU=0）。与 batch.rs:241-257 的已验证装载模式同构：
/// 在飞数 ≤ bound 由「set.len() < bound 才 push」的循环结构保证。
async fn drive_n<F, Fut>(n: usize, conc: usize, mut make: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<
        Output = Result<
            gqep_executor::tool_runtime::DispatchOutcome,
            gqep_executor::tool_runtime::DispatchError,
        >,
    >,
{
    let bound = conc.max(1);
    let sem = Arc::new(Semaphore::new(bound));
    let mut set = FuturesUnordered::new();
    let mut pushed = 0usize;
    loop {
        if pushed < n && set.len() < bound {
            let permit = sem
                .clone()
                .acquire_owned()
                .await
                .expect("semaphore 不应关闭");
            let fut = make();
            set.push(async move {
                let r = fut.await;
                drop(permit);
                r
            });
            pushed += 1;
            continue;
        }
        match set.next().await {
            Some(r) => {
                let _ = r;
            }
            None => break,
        }
    }
}

const N: usize = 64;

fn bench_dispatch_borrowed_chain(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio multi-thread runtime build failed");
    let runtime =
        ToolRuntime::new(Arc::new(EchoExecutor), allow_policy()).expect("生产装配链应构造成功");
    let owned_req = request();
    // 借用视图构造在计时区外（借自循环外拥有缓冲）
    let borrowed_req = gqep_executor::tool_runtime::DispatchRequestRef::from(&owned_req);

    let mut group = c.benchmark_group("dispatch_borrowed_chain");
    group.sample_size(30);

    for conc in [1usize, 4, 8] {
        group.bench_function(format!("owned_c{conc}"), |b| {
            b.to_async(&rt)
                .iter(|| drive_n(N, conc, || runtime.dispatch(&owned_req)));
        });
        group.bench_function(format!("borrowed_c{conc}"), |b| {
            b.to_async(&rt)
                .iter(|| drive_n(N, conc, || runtime.dispatch_borrowed(&borrowed_req)));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_dispatch_borrowed_chain);
criterion_main!(benches);
