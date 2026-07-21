//! 流式 token 增量渲染基准(ADR-029,v3.1 M0 benchmark-first)
//!
//! 对应架构层:L10 Interface(`chimera-tui::engine`)
//!
//! # 基准项与目标(RED-first)
//! - `single_token`:单个 token 追加到光标行后的 diff 延迟,目标 P95 < 500µs·token。
//! - `sustained_tokens`:连续追加多个 token 的稳态吞吐(tokens/sec)。
//!
//! # 设计理由(WHY)
//! - **只动光标行**:流式对话每次只在末行追加 token,理想情况下 diff 仅产出
//!   该行变化的少量单元格。本基准验证"增量渲染代价 = O(1 行)"而非 O(整帧),
//!   为 M3 Chat 面板流式渲染与 M4 低带宽优化提供可证伪基线。
//! - **纯内存路径**:不含 EventBus/终端 IO,聚焦 append + diff 的 CPU 成本。

#![forbid(unsafe_code)]

use chimera_tui::engine::{Buffer, DiffEngine, Rect, Style};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

/// 在 `base` 的最后一行、`col` 起始处写入 `token`,返回追加后的新缓冲
///
/// WHY 克隆 base:模拟"front 已呈现,back 追加一个 token"的双缓冲流程,
/// diff(base, next) 即该 token 引入的增量变化。
fn append_token(base: &Buffer, col: u16, token: &str) -> Buffer {
    let mut next = base.clone();
    let last_row = base.area.height.saturating_sub(1);
    next.set_string(col, last_row, token, Style::new());
    next
}

/// 单 token 增量:append + diff 的端到端延迟(目标 P95 < 500µs·token)
fn single_token(c: &mut Criterion) {
    let sizes = [(80u16, 24u16), (120, 40)];
    let token = "hello ";
    let mut group = c.benchmark_group("streaming_single_token");
    for (w, h) in sizes {
        let front = Buffer::empty(Rect::new(0, 0, w, h));
        group.bench_with_input(BenchmarkId::new("size", format!("{w}x{h}")), &(), |b, _| {
            b.iter(|| {
                let back = append_token(black_box(&front), 0, black_box(token));
                let changes = DiffEngine::compute(&front, &back);
                black_box(changes);
            });
        });
    }
    group.finish();
}

/// 稳态吞吐:连续追加 N 个 token,逐 token diff,报告 tokens/sec
fn sustained_tokens(c: &mut Criterion) {
    const N: u64 = 50;
    let token = "tok ";
    let mut group = c.benchmark_group("streaming_sustained");
    group.throughput(Throughput::Elements(N));
    group.bench_function("50_tokens_80x24", |b| {
        b.iter(|| {
            let mut front = Buffer::empty(Rect::new(0, 0, 80, 24));
            let mut col = 0u16;
            for _ in 0..N {
                let back = append_token(&front, col, token);
                let changes = DiffEngine::compute(&front, &back);
                black_box(&changes);
                // 推进光标(超出宽度则回绕到行首,模拟自动换行前的近似)
                col = (col + token.len() as u16) % 80;
                front = back;
            }
        });
    });
    group.finish();
}

criterion_group!(benches, single_token, sustained_tokens);
criterion_main!(benches);
