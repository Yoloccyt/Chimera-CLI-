//! NMC 编码器集成测试 — 验证编码 → 事件发布链路与多模态错误处理
//!
//! 对应 SubTask 4.5:集成测试 ≥ 5 个
//!
//! # 测试场景
//! 1. 编码文本 → 发布 NmcEncoded 事件(端到端链路)
//! 2. 编码桌面 → 发布 NmcEncoded 事件
//! 3. 不同模态的事件 modality 字段正确
//! 4. v2.0 感知器(Image/Video/Audio)对非空数据成功编码,空数据返回 EncodingFailed
//! 5. 无事件总线时编码仍正常工作
//! 6. 事件 clv_dimension 始终为 512

use event_bus::{EventBus, NexusEvent};
use nmc_encoder::{
    DesktopCapture, FusionStrategy, Modality, NmcConfig, NmcEncoder, PerceptionInput,
};

/// 辅助:创建带事件总线的编码器,返回 (编码器, 接收者)
fn make_encoder_with_bus() -> (NmcEncoder, event_bus::EventReceiver) {
    let bus = EventBus::new();
    let rx = bus.subscribe();
    let encoder = NmcEncoder::with_event_bus(NmcConfig::default(), bus).expect("编码器构造应成功");
    (encoder, rx)
}

#[test]
fn test_encode_text_publishes_event() {
    let (encoder, mut rx) = make_encoder_with_bus();
    let output = encoder
        .perceive(PerceptionInput::Text("hello world".into()))
        .expect("文本编码应成功");

    assert_eq!(output.dimension(), 512);

    let event = rx.try_recv().expect("接收不应出错").expect("应有事件");
    match event {
        NexusEvent::NmcEncoded {
            modality,
            content_hash,
            clv_dimension,
            ..
        } => {
            assert_eq!(modality, "Text");
            assert!(!content_hash.is_empty());
            assert_eq!(clv_dimension, 512);
        }
        other => panic!("期望 NmcEncoded 事件,收到 {other:?}"),
    }
}

#[test]
fn test_encode_desktop_publishes_event() {
    let (encoder, mut rx) = make_encoder_with_bus();
    let input = PerceptionInput::Desktop(DesktopCapture::new(1920, 1080, "code editor"));
    let output = encoder.perceive(input).expect("桌面编码应成功");

    assert_eq!(output.dimension(), 512);

    let event = rx.try_recv().expect("接收不应出错").expect("应有事件");
    match event {
        NexusEvent::NmcEncoded { modality, .. } => {
            assert_eq!(modality, "Desktop");
        }
        other => panic!("期望 NmcEncoded 事件,收到 {other:?}"),
    }
}

#[test]
fn test_different_modalities_produce_different_events() {
    let (encoder, mut rx) = make_encoder_with_bus();

    // 编码文本
    encoder
        .perceive(PerceptionInput::Text("text input".into()))
        .expect("文本编码应成功");
    let event1 = rx.try_recv().expect("接收不应出错").expect("应有事件");

    // 编码桌面
    encoder
        .perceive(PerceptionInput::Desktop(DesktopCapture::new(
            800, 600, "desktop",
        )))
        .expect("桌面编码应成功");
    let event2 = rx.try_recv().expect("接收不应出错").expect("应有事件");

    match (event1, event2) {
        (
            NexusEvent::NmcEncoded { modality: m1, .. },
            NexusEvent::NmcEncoded { modality: m2, .. },
        ) => {
            assert_eq!(m1, "Text");
            assert_eq!(m2, "Desktop");
            assert_ne!(m1, m2);
        }
        _ => panic!("两个事件都应为 NmcEncoded"),
    }
}

#[test]
fn test_image_perceptor_returns_encoding_failed() {
    // WHY v2.0 迁移:ImagePerceptor 已升级为像素统计嵌入,
    // 非空数据成功编码为 512-dim CLV,仅空数据返回 EncodingFailed。
    let encoder = NmcEncoder::new(NmcConfig::default()).expect("编码器构造应成功");

    // 空图像 → EncodingFailed
    let result = encoder.perceive(PerceptionInput::Image(vec![]));
    assert!(result.is_err(), "空图像应返回 EncodingFailed");

    // 非空图像 → 成功编码为 512-dim CLV
    let output = encoder
        .perceive(PerceptionInput::Image(vec![0xFF; 2048]))
        .expect("非空图像应成功编码");
    assert_eq!(output.dimension(), 512);
}

#[test]
fn test_video_and_audio_perceptors_return_errors() {
    // WHY v2.0 迁移:Video/AudioPerceptor 已升级为统计特征嵌入,
    // 非空数据成功编码,仅空数据返回 EncodingFailed。
    let encoder = NmcEncoder::new(NmcConfig::default()).expect("编码器构造应成功");

    // 空视频 → EncodingFailed
    let video_result = encoder.perceive(PerceptionInput::Video(vec![]));
    assert!(video_result.is_err(), "空视频应返回 EncodingFailed");

    // 非空视频 → 成功编码
    let video_output = encoder
        .perceive(PerceptionInput::Video(vec![0; 1024]))
        .expect("非空视频应成功编码");
    assert_eq!(video_output.dimension(), 512);

    // 空音频 → EncodingFailed
    let audio_result = encoder.perceive(PerceptionInput::Audio(vec![]));
    assert!(audio_result.is_err(), "空音频应返回 EncodingFailed");

    // 非空音频 → 成功编码
    let audio_output = encoder
        .perceive(PerceptionInput::Audio(vec![0; 512]))
        .expect("非空音频应成功编码");
    assert_eq!(audio_output.dimension(), 512);
}

#[test]
fn test_encode_without_event_bus_works() {
    let encoder = NmcEncoder::new(NmcConfig::default()).expect("编码器构造应成功");
    assert!(encoder.event_bus().is_none());

    // 无总线时编码仍正常工作,不发布事件
    let output = encoder
        .perceive(PerceptionInput::Text("no bus test".into()))
        .expect("编码应成功");
    assert_eq!(output.dimension(), 512);
}

#[test]
fn test_event_clv_dimension_always_512() {
    // 测试不同融合策略,clv_dimension 始终为 512
    for strategy in [
        FusionStrategy::Concat,
        FusionStrategy::Mean,
        FusionStrategy::Weighted,
    ] {
        let config = NmcConfig::default().with_fusion_strategy(strategy);
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let encoder = NmcEncoder::with_event_bus(config, bus).expect("编码器构造应成功");
        encoder
            .perceive(PerceptionInput::Text(format!("test {strategy:?}")))
            .expect("编码应成功");
        let event = rx.try_recv().expect("接收不应出错").expect("应有事件");
        if let NexusEvent::NmcEncoded { clv_dimension, .. } = event {
            assert_eq!(
                clv_dimension, 512,
                "策略 {strategy:?} 的 clv_dimension 应为 512"
            );
        }
    }
}

#[test]
fn test_deterministic_encoding_same_input_same_output() {
    let encoder = NmcEncoder::new(NmcConfig::default()).expect("编码器构造应成功");
    let input = "deterministic integration test";

    let output1 = encoder
        .perceive(PerceptionInput::Text(input.into()))
        .expect("编码应成功");
    let output2 = encoder
        .perceive(PerceptionInput::Text(input.into()))
        .expect("编码应成功");

    assert_eq!(output1, output2, "相同输入应产生相同 CLV 输出");
}

#[test]
fn test_config_validation_rejects_invalid_clv_dim() {
    let bad_config = NmcConfig::default().with_clv_dim(256);
    let result = NmcEncoder::new(bad_config);
    assert!(result.is_err());
    let err_msg = result.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(
        err_msg.contains("clv_dim"),
        "错误信息应包含 clv_dim,实际为: {err_msg}"
    );
}

#[test]
fn test_modality_as_str_matches_event() {
    assert_eq!(Modality::Text.as_str(), "Text");
    assert_eq!(Modality::Desktop.as_str(), "Desktop");
    assert_eq!(Modality::Image.as_str(), "Image");
}
