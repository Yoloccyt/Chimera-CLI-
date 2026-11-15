//! 生产装配压缩率基准 — R12 D0-1 / 本 Spec A5 的取数面
//!
//! 语料设计（固定、确定性，可复现）：
//! - 80 条唯一上下文：60 条对话/系统条目 + 20 条工具响应（长文本，模拟工具输出）；
//! - 60 条精确重复：与唯一集前 60 条 `(role, source, content)` 逐字相同；
//! - 60 条规范化重复：与后 60 条仅空白/ASCII 大小写不同（`CHIMERA_DEDUP_NORMALIZE=on`）。
//!
//! 合计 200 条，去重后 80 条 ⇒ 压缩比 `original/used = 2.5×`，即
//! `1 - compressed/original = 60%`。这是本 Spec §0.3 的压缩率口径，
//! 不是"估算"也不是"抽测"：断言在 bench 启动时先自证，不达即 panic。
//!
//! WHY 固定语料而非随机：压缩率必须可跨轮对照；随机语料会让"提升"变成
//! 语料分布变化。ADR-296 D4 的取数纪律同样适用——跨会话 criterion 值不作证据，
//! 只认同轮 A/B 与语料指纹。

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use hcw_window::assemble::{assemble_context, ContextItem, ContextRole, ContextSource};
use hcw_window::estimate_context_tokens;
use nexus_contracts::spec::harness_dimensions::ContextAssemblyContract;

/// 工具响应文本（长文本，提升工具输出在压缩率中的权重）
const TOOL_BODY: &str = "tool-result: build output line; warning and diagnostic payload; \
repeated machine text that models routinely see more than once in long sessions. ";

fn item(seq: u64, role: ContextRole, source: ContextSource, content: String) -> ContextItem {
    ContextItem {
        role,
        source,
        content,
        pinned: false,
        seq,
    }
}

/// 构造固定 200 条语料
fn make_items() -> Vec<ContextItem> {
    let mut items = Vec::with_capacity(200);
    let mut seq = 0u64;

    // 80 条唯一：60 条对话 + 20 条工具响应
    for i in 0..60 {
        let role = match i % 4 {
            0 => ContextRole::System,
            1 => ContextRole::User,
            2 => ContextRole::Assistant,
            _ => ContextRole::Tool,
        };
        let source = match role {
            ContextRole::System => ContextSource::System,
            ContextRole::User => ContextSource::User,
            ContextRole::Assistant => ContextSource::Assistant,
            ContextRole::Tool => ContextSource::Tool,
        };
        items.push(item(
            seq,
            role,
            source,
            format!("unique-dialogue-{i:02}: deterministic context payload for assembly."),
        ));
        seq += 1;
    }
    for i in 0..20 {
        items.push(item(
            seq,
            ContextRole::Tool,
            ContextSource::Tool,
            format!("unique-tool-{i:02}: {TOOL_BODY}"),
        ));
        seq += 1;
    }

    // 60 条精确重复：覆盖 40 条对话 + 20 条工具响应（长文本进入重复面，
    // 避免"重复只落在短条目"导致压缩率被唯一长条目稀释）
    for i in 20..80usize {
        items.push(item(
            seq,
            items[i].role,
            items[i].source,
            items[i].content.clone(),
        ));
        seq += 1;
    }

    // 60 条规范化重复：仅空白/ASCII 大小写不同（覆盖 20..79，含 20 条工具响应，
    // 使长文本获得第三份副本，压缩率不被唯一长条目稀释）
    for i in 20..80usize {
        let base = &items[i];
        let normalized = format!("  {}  ", base.content.to_ascii_uppercase());
        items.push(item(seq, base.role, base.source, normalized));
        seq += 1;
    }

    items
}

fn bench_assembly_dedup(c: &mut Criterion) {
    let items = make_items();
    let contract = ContextAssemblyContract::default_contract();

    // 语料指纹 + 压缩率自证：不达 2.5× 直接 panic，禁止"静默降标"
    let original: usize = items
        .iter()
        .map(|i| estimate_context_tokens(&i.content))
        .sum();
    let assembled = assemble_context(items.clone(), &contract, 0);
    let used = assembled.diagnostics.used_tokens.max(1);
    let ratio = original as f32 / used as f32;
    let reduction = 1.0 - (used as f32 / original as f32);
    println!(
        "assembly_dedup corpus: items={} original_tokens={} used_tokens={} ratio={ratio:.3} reduction={reduction:.3} deduped={} normalized={} evicted={} kept={}",
        items.len(),
        original,
        used,
        assembled.diagnostics.deduped,
        assembled.diagnostics.deduped_normalized,
        assembled.diagnostics.evicted,
        assembled.items.len()
    );
    assert!(
        ratio >= 2.5,
        "压缩率未达 60%：original={original} used={used} ratio={ratio:.3} deduped={} normalized={} evicted={} kept={}",
        assembled.diagnostics.deduped,
        assembled.diagnostics.deduped_normalized,
        assembled.diagnostics.evicted,
        assembled.items.len()
    );

    c.bench_function("assemble_200_items_60pct_redundancy", |b| {
        b.iter(|| {
            let out = assemble_context(black_box(items.clone()), black_box(&contract), 0);
            black_box(out)
        })
    });
}

criterion_group!(benches, bench_assembly_dedup);
criterion_main!(benches);
