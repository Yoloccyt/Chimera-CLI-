//! 流式输出批量化等价性（Phase 6A / T6A.1）
//!
//! 锁两件事：
//! 1. **字节等价**：默认 `batch=1` 与任何 `batch>1` 写出的字节序列完全一致
//!    （含末尾换行），批量只是减少 write/flush 次数，不改内容；
//! 2. **节奏不变**：每批 sleep = `delay × 该批字符数`，故总墙钟与逐字符路径相同
//!    —— 这是"按批摊销"的算术证明，不靠计时断言（共享宿主上计时会假红）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::time::Duration;

use chimera_cli::commands::run::{batch_delay, stream_to, DEFAULT_STREAM_BATCH_CHARS};
use chimera_cli::orchestrator::plan_chunks;

/// 计数 sink：记录 write/flush 次数与落盘字节
#[derive(Default)]
struct CountingSink {
    bytes: Vec<u8>,
    writes: usize,
    flushes: usize,
}

impl Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writes += 1;
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

/// 默认批量必须是 1（不改现状行为），且逐字符与批路径字节一致
#[tokio::test]
async fn stream_batch_default_is_per_char() {
    assert_eq!(
        DEFAULT_STREAM_BATCH_CHARS, 1,
        "默认批量必须为逐字符,否则本批改动的默认行为就变了"
    );
    let reply = "第一行。\nsecond line with ascii and 日本語。";
    let chars: Vec<String> = plan_chunks(reply);

    let mut per_char = CountingSink::default();
    stream_to(reply, Duration::ZERO, 1, &mut per_char)
        .await
        .expect("write ok");
    let mut via_plan = CountingSink::default();
    for delta in &chars {
        write!(via_plan, "{delta}").expect("write ok");
    }
    writeln!(via_plan).expect("write ok");

    assert_eq!(
        per_char.bytes, via_plan.bytes,
        "默认路径必须与旧逐字符写入逐字节相同"
    );
    // 逐字符：每字符一次 write + 一次 flush，末尾换行只 write（与旧实现一致）
    let n = reply.chars().count();
    assert_eq!(per_char.writes, n + 1, "每字符一次 write + 末尾换行");
    assert_eq!(per_char.flushes, n, "每字符一次 flush（旧行为）");
}

/// batch=8：字节不变，write/flush 次数按批数下降
#[tokio::test]
async fn stream_batch_amortizes_writes_and_keeps_bytes() {
    let reply = "abcdefghijklmnopqrstuvwxyz0123456789";
    let n = reply.chars().count();

    let mut single = CountingSink::default();
    stream_to(reply, Duration::ZERO, 1, &mut single)
        .await
        .expect("write ok");
    let mut batched = CountingSink::default();
    stream_to(reply, Duration::ZERO, 8, &mut batched)
        .await
        .expect("write ok");

    assert_eq!(single.bytes, batched.bytes, "批量不得改变输出字节");
    let batches = n.div_ceil(8);
    assert_eq!(single.writes, n + 1, "逐字符写次数 = 字符数 + 末尾换行");
    assert_eq!(single.flushes, n, "逐字符 flush 次数 = 字符数");
    assert_eq!(
        batched.writes,
        batches + 1,
        "write 次数 = ceil(n/8) + 末尾换行"
    );
    assert_eq!(batched.flushes, batches, "flush 次数 = ceil(n/8)");
    assert!(
        batched.flushes * 7 <= single.flushes,
        "8 字符一批至少把 flush 压到 1/7（single={} batched={}）",
        single.flushes,
        batched.flushes
    );
}

/// 延迟摊销的算术证明：总延迟不变，只是摊到更少次 sleep
#[test]
fn batch_delay_preserves_total_pacing() {
    let per_char = Duration::from_millis(20);
    assert_eq!(
        batch_delay(per_char, 1),
        per_char,
        "batch=1 必须与旧逐字符延迟相同"
    );
    assert_eq!(
        batch_delay(per_char, 8),
        per_char * 8,
        "一批 8 字符 = 8 次单字符延迟"
    );
    // n=8、batch=8 ⇒ 恰 1 批：总时长与逐字符 8 次完全相同（节奏未变）
    assert_eq!(
        batch_delay(per_char, 8) * 1,
        per_char * 8,
        "总墙钟必须等于逐字符路径"
    );
    // 零延迟必须仍为零（测试/CI 加速路径）
    assert!(batch_delay(Duration::ZERO, 8).is_zero());
}

/// 多字节安全：批切分按 char 而非 byte，不得把 UTF-8 序列切碎
#[tokio::test]
async fn stream_batch_is_char_boundary_safe() {
    let reply = "🚀中文混合abc";
    let mut sink = CountingSink::default();
    stream_to(reply, Duration::ZERO, 3, &mut sink)
        .await
        .expect("write ok");
    let text = String::from_utf8(sink.bytes.clone()).expect("必须是合法 UTF-8（未切碎多字节）");
    assert_eq!(text, format!("{reply}\n"), "内容逐字符保序");
}

/// 空回复不得产出任何字符写入（只留末尾换行），且不 panic
#[tokio::test]
async fn stream_batch_empty_reply_writes_only_newline() {
    let mut sink = CountingSink::default();
    stream_to("", Duration::ZERO, 4, &mut sink)
        .await
        .expect("write ok");
    assert_eq!(sink.bytes, b"\n", "空回复只写换行");
}
