//! Golden Embedding 回归测试 — 跨模态一致性与后处理确定性验证
//!
//! 对应 SubTask 7.3: 验证 Image/Video/Audio 三种模态的 embedding 维度一致性
//! 与 L2 归一化正确性，即使在没有 ONNX 模型文件的情况下也能通过。
//!
//! # 测试策略
//! - ONNX 模型不可用时，感知器返回 EncodingFailed，但预处理和后处理管道
//!   仍可独立验证：预处理输出形状正确、后处理输出 512 维 L2 归一化向量
//! - 三个感知器（Image/Video/Audio）共享相同的 OnnxBackend 后处理管道，
//!   通过测试后处理的一致性来保证跨模态 embedding 维度统一

use nmc_encoder::{
    AudioPerceptor, ImagePerceptor, Modality, NmcConfig, OnnxBackend, PerceptionInput, Perceptor,
    VideoPerceptor,
};

// ============================================================================
// 跨模态一致性测试
// ============================================================================

/// 验证三种模态 (Image/Video/Audio) 的 preprocessing 管道输出形状一致，
/// 且 postprocessing 管道始终输出 512 维向量。
///
/// 当 ONNX 模型不可用时，感知器返回 EncodingFailed，但预处理和后处理
/// 管道仍可独立验证：
/// - Image: 预处理输出形状 (3, 224, 224)
/// - Video: 预处理输出形状 (1, 3, 224, 224)
/// - Audio: 预处理输出形状 (1, 80, 3000)
/// - 后处理: 任意形状输入 → 512 维 L2 归一化向量
///
/// 此测试验证后处理管道的维度一致性（所有模态共享同一 postprocess 函数），
/// 确保无论输入形状如何，embedding 维度始终为 512。
#[test]
fn test_cross_modal_embedding_dimension() {
    // --- 图像预处理 ---
    // 构造已知像素值的 1x1 PNG
    let img = image::RgbImage::from_pixel(1, 1, image::Rgb([100, 150, 200]));
    let mut img_buf = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut img_buf),
        image::ImageFormat::Png,
    )
    .expect("编码 PNG 应成功");

    let img_tensor = OnnxBackend::preprocess_image(&img_buf).expect("图像预处理应成功");
    assert_eq!(
        img_tensor.shape(),
        &[3, 224, 224],
        "Image: 预处理输出形状应为 (3, 224, 224)"
    );

    let img_embedding = OnnxBackend::postprocess(img_tensor);
    assert_eq!(
        img_embedding.len(),
        512,
        "Image: 后处理输出维度应为 512，实际 {}",
        img_embedding.len()
    );

    // --- 视频预处理 ---
    let img2 = image::RgbImage::from_pixel(1, 1, image::Rgb([50, 100, 150]));
    let mut vid_buf = Vec::new();
    img2.write_to(
        &mut std::io::Cursor::new(&mut vid_buf),
        image::ImageFormat::Png,
    )
    .expect("编码 PNG 应成功");

    let vid_tensor = OnnxBackend::preprocess_video(&vid_buf).expect("视频预处理应成功");
    assert_eq!(
        vid_tensor.shape(),
        &[1, 3, 224, 224],
        "Video: 预处理输出形状应为 (1, 3, 224, 224)"
    );

    let vid_embedding = OnnxBackend::postprocess(vid_tensor);
    assert_eq!(
        vid_embedding.len(),
        512,
        "Video: 后处理输出维度应为 512，实际 {}",
        vid_embedding.len()
    );

    // --- 音频预处理 ---
    let audio_bytes = [0x00u8; 100];
    let aud_tensor = OnnxBackend::preprocess_audio(&audio_bytes).expect("音频预处理应成功");
    assert_eq!(
        aud_tensor.shape(),
        &[1, 80, 3000],
        "Audio: 预处理输出形状应为 (1, 80, 3000)"
    );

    let aud_embedding = OnnxBackend::postprocess(aud_tensor);
    assert_eq!(
        aud_embedding.len(),
        512,
        "Audio: 后处理输出维度应为 512，实际 {}",
        aud_embedding.len()
    );

    // --- 跨模态维度一致性 ---
    // 所有三种模态的后处理输出维度均为 512
    assert_eq!(img_embedding.len(), 512);
    assert_eq!(vid_embedding.len(), 512);
    assert_eq!(aud_embedding.len(), 512);
}

/// 验证后处理输出的 L2 范数 ≈ 1.0（归一化正确性）。
///
/// 测试多种输入形状和数值分布，确保归一化在各种场景下都正确：
/// - 恰好 512 维非零向量
/// - 小于 512 维（填充后归一化）
/// - 大于 512 维（截断后归一化）
/// - 全零向量（保持零，不除零）
#[test]
fn test_embedding_l2_normalized() {
    // 场景 1: 恰好 512 维非零向量 → L2 范数 ≈ 1.0
    {
        let values: Vec<f32> = (1..=512).map(|i| i as f32).collect();
        let input =
            ndarray::ArrayD::from_shape_vec(vec![512], values).expect("构造 512 维张量应成功");
        let result = OnnxBackend::postprocess(input);
        let norm_sq: f32 = result.iter().map(|v| v * v).sum();
        assert!(
            (norm_sq - 1.0).abs() < 1e-5,
            "场景1 (恰好512维): L2 归一化平方和应为 1.0，实际 {}",
            norm_sq
        );
    }

    // 场景 2: 100 维向量 → 填充到 512 后归一化
    {
        let values: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        let input =
            ndarray::ArrayD::from_shape_vec(vec![100], values).expect("构造 100 维张量应成功");
        let result = OnnxBackend::postprocess(input);
        let norm_sq: f32 = result.iter().map(|v| v * v).sum();
        assert!(
            (norm_sq - 1.0).abs() < 1e-5,
            "场景2 (100维填充): L2 归一化平方和应为 1.0，实际 {}",
            norm_sq
        );
    }

    // 场景 3: 1000 维向量 → 截断到 512 后归一化
    {
        let values: Vec<f32> = (1..=1000).map(|i| i as f32).collect();
        let input =
            ndarray::ArrayD::from_shape_vec(vec![1000], values).expect("构造 1000 维张量应成功");
        let result = OnnxBackend::postprocess(input);
        let norm_sq: f32 = result.iter().map(|v| v * v).sum();
        assert!(
            (norm_sq - 1.0).abs() < 1e-5,
            "场景3 (1000维截断): L2 归一化平方和应为 1.0，实际 {}",
            norm_sq
        );
    }

    // 场景 4: 3D 图像张量 (3, 224, 224) → 展平后归一化
    {
        let total = 3 * 224 * 224;
        let values: Vec<f32> = (0..total).map(|i| (i as f32 % 255.0) / 255.0).collect();
        let input = ndarray::ArrayD::from_shape_vec(vec![3, 224, 224], values)
            .expect("构造 3D 图像张量应成功");
        let result = OnnxBackend::postprocess(input);
        let norm_sq: f32 = result.iter().map(|v| v * v).sum();
        assert!(
            (norm_sq - 1.0).abs() < 1e-5,
            "场景4 (3D图像张量): L2 归一化平方和应为 1.0，实际 {}",
            norm_sq
        );
    }

    // 场景 5: 全零向量 → 保持零，不除零，不产生 NaN
    {
        let input = ndarray::ArrayD::zeros(vec![50]);
        let result = OnnxBackend::postprocess(input);
        let total_abs: f32 = result.iter().map(|v| v.abs()).sum();
        assert!(
            total_abs < 1e-10,
            "场景5 (全零向量): 应保持全零，实际绝对值和 {}",
            total_abs
        );
        for &v in &result {
            assert!(v.is_finite(), "全零输入不应产生 NaN 或 inf");
        }
    }
}

// ============================================================================
// 感知器 Fallback 一致性测试
// ============================================================================

/// 验证三个感知器 (Image/Video/Audio) 在 ONNX 不可用时返回一致的
/// EncodingFailed 错误，且错误信息包含正确的模态名称。
#[test]
fn test_perceptors_fallback_consistent() {
    let config = NmcConfig::default();

    // ImagePerceptor: ONNX 不可用 → EncodingFailed
    let img_perceptor = ImagePerceptor::new(config.clone());
    let img_result = img_perceptor.perceive(&PerceptionInput::Image(vec![0xFF; 1024]));
    assert!(
        img_result.is_err(),
        "ImagePerceptor 在 ONNX 不可用时应返回错误"
    );
    let img_err = img_result.unwrap_err().to_string();
    assert!(
        img_err.contains("Image"),
        "ImagePerceptor 错误信息应包含 'Image'，实际: {img_err}"
    );
    assert!(
        img_err.contains("ONNX"),
        "ImagePerceptor 错误信息应包含 'ONNX'，实际: {img_err}"
    );

    // VideoPerceptor: ONNX 不可用 → EncodingFailed
    let vid_perceptor = VideoPerceptor::new(config.clone());
    let vid_result = vid_perceptor.perceive(&PerceptionInput::Video(vec![0xFF; 1024]));
    assert!(
        vid_result.is_err(),
        "VideoPerceptor 在 ONNX 不可用时应返回错误"
    );
    let vid_err = vid_result.unwrap_err().to_string();
    assert!(
        vid_err.contains("Video"),
        "VideoPerceptor 错误信息应包含 'Video'，实际: {vid_err}"
    );
    assert!(
        vid_err.contains("ONNX"),
        "VideoPerceptor 错误信息应包含 'ONNX'，实际: {vid_err}"
    );

    // AudioPerceptor: ONNX 不可用 → EncodingFailed
    let aud_perceptor = AudioPerceptor::new(config.clone());
    let aud_result = aud_perceptor.perceive(&PerceptionInput::Audio(vec![0xFF; 1024]));
    assert!(
        aud_result.is_err(),
        "AudioPerceptor 在 ONNX 不可用时应返回错误"
    );
    let aud_err = aud_result.unwrap_err().to_string();
    assert!(
        aud_err.contains("Audio"),
        "AudioPerceptor 错误信息应包含 'Audio'，实际: {aud_err}"
    );
    assert!(
        aud_err.contains("ONNX"),
        "AudioPerceptor 错误信息应包含 'ONNX'，实际: {aud_err}"
    );
}

/// 验证三个感知器返回正确的模态标识。
#[test]
fn test_perceptors_modality_identity() {
    let config = NmcConfig::default();

    assert_eq!(
        ImagePerceptor::new(config.clone()).modality(),
        Modality::Image
    );
    assert_eq!(
        VideoPerceptor::new(config.clone()).modality(),
        Modality::Video
    );
    assert_eq!(
        AudioPerceptor::new(config.clone()).modality(),
        Modality::Audio
    );
}

// ============================================================================
// 后处理确定性验证
// ============================================================================

/// 验证后处理是确定性的：相同输入始终产生相同输出。
#[test]
fn test_postprocess_deterministic() {
    let values: Vec<f32> = vec![0.1, 0.2, 0.3, 0.4, 0.5];
    let input = ndarray::ArrayD::from_shape_vec(vec![5], values).expect("构造张量应成功");

    let result1 = OnnxBackend::postprocess(input.clone());
    let result2 = OnnxBackend::postprocess(input);

    assert_eq!(result1, result2, "相同输入应产生相同的后处理输出");
}

/// 验证后处理输出的值全部为有限值（无 NaN 或 inf）。
#[test]
fn test_postprocess_output_is_finite() {
    // 测试多种输入分布
    let test_cases: Vec<Vec<f32>> = vec![
        vec![1.0f32; 100],     // 全 1.0
        vec![0.5f32; 200],     // 全 0.5
        vec![-1.0f32, 1.0f32], // 包含负值
        vec![0.0f32; 10],      // 全零
        vec![1e10f32; 50],     // 大值
        vec![1e-10f32; 50],    // 小值
    ];

    for (i, values) in test_cases.iter().enumerate() {
        let input = ndarray::ArrayD::from_shape_vec(vec![values.len()], values.clone())
            .expect("构造张量应成功");

        let result = OnnxBackend::postprocess(input);
        let all_finite = result.iter().all(|v| v.is_finite());
        assert!(all_finite, "测试用例 {}: 后处理输出应全部为有限值", i);
    }
}
