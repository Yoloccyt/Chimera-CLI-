//! ExitCode 矩阵分发性能基准(Task 0.2 SubTask 0.2.5)
//!
//! 验证 `ChimeraCliError::exit_code()` 7 变体分发的延迟 < 10µs。
//!
//! WHY 此基准:ExitCode 矩阵是 main 退出路径的关键分支,每次 CLI 错误退出
//! 都会调用。虽然 match 分发本身是 O(1) 纳秒级,但基准化可:
//! 1. 证伪"引入 ExitCode 矩阵会拖慢退出路径"的主观判断(性能可证伪原则)
//! 2. 防止未来重构(如增加变体或改用 HashMap 分发)意外引入性能回归
//! 3. 提供 criterion 统计置信区间,排除测量噪声

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use chimera_cli::ChimeraCliError;

/// 基准:7 变体 exit_code() 分发的吞吐量与延迟
fn bench_exit_code_dispatch(c: &mut Criterion) {
    // 构造 7 变体错误样本(覆盖完整矩阵)
    let errors: Vec<ChimeraCliError> = vec![
        ChimeraCliError::NotImplemented("bench not_implemented".into()),
        ChimeraCliError::ConfigError("bench config_error".into()),
        ChimeraCliError::EngineError("bench engine_error".into()),
        ChimeraCliError::UserCancelled,
        ChimeraCliError::PermissionDenied("bench permission_denied".into()),
        ChimeraCliError::Timeout("bench timeout".into()),
        ChimeraCliError::IoError(std::io::Error::other("bench io_error")),
    ];

    c.bench_function("exit_code_dispatch", |b| {
        b.iter(|| {
            // black_box 防止编译器消除"无用"分发调用
            let codes: Vec<_> = black_box(&errors).iter().map(|e| e.exit_code()).collect();
            black_box(codes);
        })
    });
}

criterion_group!(benches, bench_exit_code_dispatch);
criterion_main!(benches);
