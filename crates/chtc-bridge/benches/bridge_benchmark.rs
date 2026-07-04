//! CHTC 桥接器性能基准 — 工具调用转发延迟
//!
//! 验收标准:p95 ≤ 10ms(ProtocolConverter 转换 + VSCode execute)
//!
//! 运行:`cargo bench -p chtc-bridge`

use chtc_bridge::{ChtcBridge, ChtcConfig, IdeSource, ProtocolConverter};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use serde_json::json;

/// 测试完整链路:receive(协议转换) + execute(VSCode 模拟执行)
fn bench_receive_execute(c: &mut Criterion) {
    let bridge = ChtcBridge::new(ChtcConfig::default());
    let raw = json!({ "command": "editor.open", "args": { "file": "/x.rs" } });
    c.bench_function("receive_execute_vscode", |b| {
        b.iter(|| {
            let call = bridge
                .receive(black_box(raw.clone()), IdeSource::vscode())
                .expect("receive 失败");
            let _ = bridge.execute(black_box(&call)).expect("execute 失败");
        });
    });
}

/// 测试纯协议转换延迟(不含 execute)
fn bench_protocol_convert(c: &mut Criterion) {
    let raw = json!({ "command": "c", "args": { "k": "v" } });
    c.bench_function("protocol_convert_vscode", |b| {
        b.iter(|| {
            let _ = ProtocolConverter::from_vscode_format(black_box(raw.clone()))
                .expect("convert 失败");
        });
    });
}

/// 测试反向转换延迟(to_native_format)
fn bench_to_native(c: &mut Criterion) {
    let raw = json!({ "command": "c", "args": { "k": "v" } });
    let call = ProtocolConverter::from_vscode_format(raw).expect("convert 失败");
    c.bench_function("to_native_format_vscode", |b| {
        b.iter(|| {
            let _ = ProtocolConverter::to_native_format(black_box(&call));
        });
    });
}

criterion_group!(
    benches,
    bench_receive_execute,
    bench_protocol_convert,
    bench_to_native
);
criterion_main!(benches);
