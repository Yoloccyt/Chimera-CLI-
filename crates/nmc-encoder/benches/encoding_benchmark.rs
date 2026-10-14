//! 文本编码延迟基准测试
//!
//! 对应 SubTask 4.5:验证文本感知器编码延迟
//! 验收标准:p95 ≤ 30ms
//!
//! # 基准场景
//! - 短文本(100 字符)编码延迟
//! - 中等文本(1KB)编码延迟
//! - 长文本(10KB)编码延迟
//!
//! # image / video / audio 三档（W5-1 R-3 / ADR-196 D4 第 4 条）
//! 新增三组**真实模型推理**骨架。它们只在 ONNX 资产已就位时跑；
//! 无资产时**诚实 SKIP 并打印原因**，不用合成输入冒充真实推理。
//! 因此三组的真实 p50/p95 仍为【待验证】，且**不得入 SLO**（未进
//! `scripts/bench_check.yml`）——资产来源与许可属 owner 决策域，不在本批。
//!
//! 资产约定目录 = `assets/nmc/models`，与 `scripts/fetch_nmc_assets.{ps1,sh}`
//! 的默认落点逐字一致（chimera-cli 侧的同一约定由 doctor 第 11 探针的
//! `model_asset_paths_match_fetch_scripts` 测钉住）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nmc_encoder::{NmcConfig, NmcEncoder, PerceptionInput};

/// 三档真实推理基准的资产约定目录（= fetch 脚本默认落点）。
const ASSET_MODEL_DIR: &str = "assets/nmc/models";

/// 构造指向约定资产目录的配置（不改 `NmcConfig::default()` 的“默认空 =
/// 未启用”语义，那是生产默认，仅本 bench 指向测试目录）。
fn asset_config() -> NmcConfig {
    NmcConfig::default().with_model_dir(ASSET_MODEL_DIR)
}

/// 判定某档真实推理路径是否可测：目录已配且模型文件存在。
///
/// WHY 这是跳过的唯一合法判据：无模型时感知器按设计降级并返回
/// `Err(EncodingFailed)`（`perceptors/image.rs:76-81` 等），此时 bench 量到的是
/// **占位降级路径**而不是 ONNX 推理；把它写进基准表就是造数。
fn model_ready(config: &NmcConfig, model_name: &str) -> bool {
    std::path::Path::new(&config.model_path(model_name)).exists()
}

/// 打印诚实 SKIP 原因（包括如何获取资产与“禁入 SLO”的限定）。
fn skip(modality: &str, model_name: &str) {
    println!(
        "SKIP {modality}_encoding: 未找到 ONNX 资产 {ASSET_MODEL_DIR}/{model_name};\
         真实 p50/p95 【待验证】且禁入 SLO。获取:scripts/fetch_nmc_assets.{{ps1,sh}}\
         （需先由 owner 确认资产来源与许可，见 manifest.toml 的 PENDING 行）"
    );
}

/// 定长合成输入字节（**仅用于决定张量形状**，不代表真实媒体语义）。
///
/// 取递增而非全零：避开全零输入可能命中部分算法的 shortcut 分支，
/// 使计时对真实推理更具代表性。
fn deterministic_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

/// 基准:不同长度文本的编码延迟
fn bench_text_encoding(c: &mut Criterion) {
    let encoder = NmcEncoder::new(NmcConfig::default()).expect("编码器构造应成功");

    let mut group = c.benchmark_group("text_encoding");

    for (name, size) in [
        ("short_100b", 100),
        ("medium_1kb", 1_000),
        ("long_10kb", 10_000),
    ] {
        let text = "a".repeat(size);
        group.bench_with_input(BenchmarkId::from_parameter(name), &text, |b, text| {
            b.iter(|| {
                encoder
                    .perceive(PerceptionInput::Text(text.clone()))
                    .expect("编码应成功");
            });
        });
    }

    group.finish();
}

/// 基准:Desktop 模态编码延迟(对比文本)
fn bench_desktop_encoding(c: &mut Criterion) {
    let encoder = NmcEncoder::new(NmcConfig::default()).expect("编码器构造应成功");

    c.bench_function("desktop_encoding", |b| {
        b.iter(|| {
            encoder
                .perceive(PerceptionInput::Desktop(nmc_encoder::DesktopCapture::new(
                    1920,
                    1080,
                    "code editor with syntax highlighting",
                )))
                .expect("编码应成功");
        });
    });
}

/// 基准:Image 模态真实 ONNX 推理（默认模型 clip-vit-b32.onnx）。
///
/// 输入为 224×224×3 定长合成字节（仅决定张量形状，预处理仍走
/// `OnnxBackend::preprocess_image` 真实流水线）。
fn bench_image_encoding(c: &mut Criterion) {
    let config = asset_config();
    if !model_ready(&config, &config.image_model) {
        skip("image", &config.image_model);
        return;
    }
    let encoder = NmcEncoder::new(config).expect("资产就位后编码器构造应成功");
    let input = deterministic_bytes(224 * 224 * 3);

    let mut group = c.benchmark_group("image_encoding");
    group.bench_with_input(
        BenchmarkId::from_parameter("raw_224x224x3"),
        &input,
        |b, input| {
            b.iter(|| {
                encoder
                    .perceive(PerceptionInput::Image(input.clone()))
                    .expect("真实图像推理应成功");
            });
        },
    );
    group.finish();
}

/// 基准:Video 模态真实 ONNX 推理（默认模型 videomae-base.onnx）。
///
/// 输入为 8 帧 ×112×112×3 定长合成字节；真实帧数/分辨率由模型要求决定，
/// 资产就位后需按 `preprocess_video` 的实际入口形状调参（属后续批次）。
fn bench_video_encoding(c: &mut Criterion) {
    let config = asset_config();
    if !model_ready(&config, &config.video_model) {
        skip("video", &config.video_model);
        return;
    }
    let encoder = NmcEncoder::new(config).expect("资产就位后编码器构造应成功");
    let input = deterministic_bytes(8 * 112 * 112 * 3);

    let mut group = c.benchmark_group("video_encoding");
    group.bench_with_input(
        BenchmarkId::from_parameter("raw_8f_112x112"),
        &input,
        |b, input| {
            b.iter(|| {
                encoder
                    .perceive(PerceptionInput::Video(input.clone()))
                    .expect("真实视频推理应成功");
            });
        },
    );
    group.finish();
}

/// 基准:Audio 模态真实 ONNX 推理（默认模型 whisper-encoder.onnx）。
///
/// 输入为 1s 单声道 PCM16（16000 样本 ×2 字节）定长合成字节。
fn bench_audio_encoding(c: &mut Criterion) {
    let config = asset_config();
    if !model_ready(&config, &config.audio_model) {
        skip("audio", &config.audio_model);
        return;
    }
    let encoder = NmcEncoder::new(config).expect("资产就位后编码器构造应成功");
    let input = deterministic_bytes(16_000 * 2);

    let mut group = c.benchmark_group("audio_encoding");
    group.bench_with_input(
        BenchmarkId::from_parameter("raw_pcm16_mono_1s"),
        &input,
        |b, input| {
            b.iter(|| {
                encoder
                    .perceive(PerceptionInput::Audio(input.clone()))
                    .expect("真实音频推理应成功");
            });
        },
    );
    group.finish();
}

criterion_group!(
    benches,
    bench_text_encoding,
    bench_desktop_encoding,
    bench_image_encoding,
    bench_video_encoding,
    bench_audio_encoding
);
criterion_main!(benches);
