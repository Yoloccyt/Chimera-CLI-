//! 同步 DB 打开必须离开 runtime 工作线程（Phase 6A / T6A.2，§4.4 红线 2）
//!
//! 两条断言配成一组，缺一条就是空转测试：
//! - 正向：走生产 seam `db_open::on_blocking_thread` ⇒ 工作发生在**别的线程**，
//!   且阻塞期间 runtime 仍能驱动其他任务（ticker 有 tick）；
//! - 反事实：同一段同步睡眠直接写在 async 上下文 ⇒ ticker 被饿死。
//!   没有这条负对照，"ticker > 0" 可能只是调度噪声，测不出红线是否真的生效。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use chimera_cli::db_open::on_blocking_thread;

/// 起一个"心跳"任务：旗标为真时每 5ms 计一次 tick
fn spawn_ticker(keep_going: &Arc<AtomicBool>, ticks: &Arc<AtomicUsize>) {
    let (going, ticks) = (Arc::clone(keep_going), Arc::clone(ticks));
    tokio::spawn(async move {
        while going.load(Ordering::SeqCst) {
            ticks.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
}

/// 正向：慢同步工作经生产 seam 跑在 blocking 池，runtime 不被占住
#[tokio::test(flavor = "current_thread")]
async fn blocking_work_runs_off_runtime_thread() {
    let runtime_thread = thread::current().id();
    let keep_going = Arc::new(AtomicBool::new(true));
    let ticks = Arc::new(AtomicUsize::new(0));
    spawn_ticker(&keep_going, &ticks);

    let observed = on_blocking_thread(|| {
        let id = thread::current().id();
        // 模拟冷启动下的 Connection::open + WAL pragma + FTS5 建表
        thread::sleep(Duration::from_millis(100));
        id
    })
    .await
    .expect("blocking 任务应成功");

    keep_going.store(false, Ordering::SeqCst);

    assert_ne!(
        observed, runtime_thread,
        "同步 DB 工作必须离开 runtime 工作线程（否则 §4.4 红线 2 失守）"
    );
    assert!(
        ticks.load(Ordering::SeqCst) > 0,
        "阻塞 100ms 期间 runtime 必须仍能驱动其他任务;got={}",
        ticks.load(Ordering::SeqCst)
    );
}

/// 负对照：同样的同步睡眠直接写在 async 上下文里,ticker 得不到持续驱动
#[tokio::test(flavor = "current_thread")]
async fn inline_blocking_would_starve_the_runtime() {
    let keep_going = Arc::new(AtomicBool::new(true));
    let ticks = Arc::new(AtomicUsize::new(0));
    spawn_ticker(&keep_going, &ticks);

    // 故意不经 seam：这就是改造前 `WikiStore::open` 在 async 里的形态
    thread::sleep(Duration::from_millis(100));
    keep_going.store(false, Ordering::SeqCst);

    let got = ticks.load(Ordering::SeqCst);
    assert!(
        got <= 1,
        "inline 阻塞必须饿死 ticker(若它也 >1,说明正向断言没有判别力);got={got}"
    );
}

/// seam 必须把 blocking 任务的 panic 转成错误,而不是让调用方 panic
#[tokio::test(flavor = "current_thread")]
async fn blocking_task_failure_becomes_error_not_panic() {
    let result = on_blocking_thread(|| -> u8 {
        panic!("模拟 rusqlite 侧 panic");
    })
    .await;
    assert!(
        result.is_err(),
        "blocking 任务 panic 必须降级为 EngineError,不得冒泡到 CLI 退出码判定"
    );
}
