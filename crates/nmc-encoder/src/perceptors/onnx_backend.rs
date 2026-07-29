//! ONNX 推理后端 — 封装 tract-onnx 模型加载、预处理、推理、后处理流水线
//!
//! 对应架构层: L2 Memory
//! 对应创新点: NMC (Native Multimodal Context, 原生多模态上下文编码)
//!
//! # 设计决策 (WHY)
//! - **tract-onnx 而非 ort**: ort 仅支持 MSVC 工具链 (需要 ONNX Runtime C 库),
//!   tract-onnx 为纯 Rust 实现，与 `#![forbid(unsafe_code)]` 哲学一致，跨平台兼容
//! - **同步而非 async**: ONNX 推理是 CPU 密集型操作，无需 async；tract 的
//!   `SimplePlan::run()` 是同步方法，await 无意义且增加复杂度
//! - **ModelType 枚举**: 区分图像/视频/音频模型，因为不同模态的预处理逻辑不同，
//!   但推理接口统一
//! - **channel-first (CHW) 格式**: 图像预处理输出 CHW 而非 HWC，因为 ONNX 模型
//!   (CLIP ViT-B/32, VideoMAE) 的输入规范为 NCHW，直接构造 CHW 避免后续转置开销

use crate::config::NmcConfig;
use crate::error::NmcError;
use tract_onnx::prelude::*;

// ============================================================================
// ModelType 枚举
// ============================================================================

/// 模型类型 — 区分不同模态的 ONNX 模型
///
/// 每种类型对应一个预训练模型:
/// - `Image` → CLIP ViT-B/32 (图像 → 语义嵌入)
/// - `Video` → VideoMAE (视频帧 → 时空嵌入)
/// - `Audio` → Whisper encoder (音频 → Mel 频谱 → 嵌入)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelType {
    /// 图像编码模型 (CLIP ViT-B/32)
    Image,
    /// 视频编码模型 (VideoMAE)
    Video,
    /// 音频编码模型 (Whisper encoder)
    Audio,
}

impl ModelType {
    /// 返回模态名称字符串 (用于错误消息与日志)
    fn modality_name(&self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
        }
    }

    /// 从 NmcConfig 中获取对应模型的文件名
    ///
    /// WHY: 模型文件名由用户配置 (默认值在 NmcConfig 中定义)，
    /// 不硬编码在 ModelType 中，保持配置灵活性
    fn get_model_filename<'a>(&self, config: &'a NmcConfig) -> &'a str {
        match self {
            Self::Image => &config.image_model,
            Self::Video => &config.video_model,
            Self::Audio => &config.audio_model,
        }
    }
}

// ============================================================================
// OnnxBackend 结构体
// ============================================================================

/// tract ONNX 推理计划类型别名
///
/// WHY 类型别名: 完整泛型签名 `SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<...>>`
/// 过于冗长，触发 clippy::type_complexity。提取为类型别名提升可读性。
type TractPlan = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

/// ONNX 推理后端 — 封装 tract-onnx 模型加载、预处理、推理、后处理流水线
///
/// # 生命周期
/// 1. `load()` — 加载 ONNX 模型并编译为 tract 执行计划
/// 2. `preprocess_*()` — 将原始字节转换为归一化输入张量
/// 3. `run()` — 执行 ONNX 推理
/// 4. `postprocess()` — 将输出张量归一化为 512 维 f32 向量
///
/// # 示例 (概念性)
/// ```ignore
/// let config = NmcConfig::default().with_model_dir("/opt/models");
/// let backend = OnnxBackend::load(&config, ModelType::Image)?;
/// let image_bytes = std::fs::read("cat.jpg")?;
/// let input = OnnxBackend::preprocess_image(&image_bytes)?;
/// let embedding = backend.run(input)?;
/// assert_eq!(embedding.len(), 512);
/// ```
pub struct OnnxBackend {
    /// 模型类型 — 决定预处理流水线 (图像/视频/音频)
    model_type: ModelType,
    /// tract 推理计划 — 编译后的 ONNX 模型执行计划
    ///
    /// WHY 存储完整类型而非 type-erased: SimplePlan 的 run() 方法需要具体类型
    /// 参数；使用完整泛型签名避免 `Box<dyn Runnable>` 的虚函数调用开销
    plan: TractPlan,
    /// 模型配置 — 用于获取 model_dir 拼接完整路径
    /// WHY 存储 config: load 完成后 config 仍被需要 (如日志输出模型路径)，
    /// 克隆一份避免外部所有权问题。当前主要用于 load() 阶段，后续 run()
    /// 可输出模型路径到 tracing 日志。
    #[allow(dead_code)]
    config: NmcConfig,
}

// ============================================================================
// 核心方法实现
// ============================================================================

impl OnnxBackend {
    /// 加载 ONNX 模型并编译为 tract 执行计划
    ///
    /// # 参数
    /// - `config`: NmcConfig，用于获取 model_dir 与对应模型文件名
    /// - `model_type`: 模型类型 (决定加载哪个模型文件)
    ///
    /// # 错误
    /// - `ModelLoadError`: 模型文件不存在、格式不支持或编译失败
    ///
    /// # 实现细节
    /// 1. 从 config 获取模型文件名 (image_model / video_model / audio_model)
    /// 2. 通过 `config.model_path()` 拼接完整路径
    /// 3. 使用 `tract_onnx::onnx()` 加载 ONNX 模型
    /// 4. 调用 `.into_runnable()` 编译为执行计划
    pub fn load(config: &NmcConfig, model_type: ModelType) -> Result<Self, NmcError> {
        let model_name = model_type.get_model_filename(config);
        let path = config.model_path(model_name);
        let modality = model_type.modality_name();

        let plan = tract_onnx::onnx()
            .model_for_path(&path)
            .map_err(|e| NmcError::ModelLoadError {
                model_name: format!("{}({})", modality, model_name),
                reason: format!("模型加载失败: {}", e),
            })?
            // into_typed(): InferenceModel → TypedModel (类型化模型),
            // 之后 into_runnable() 返回 SimplePlan<TypedFact, ...> 而非 InferenceFact
            .into_typed()
            .map_err(|e| NmcError::ModelLoadError {
                model_name: format!("{}({})", modality, model_name),
                reason: format!("模型类型化失败: {}", e),
            })?
            .into_runnable()
            .map_err(|e| NmcError::ModelLoadError {
                model_name: format!("{}({})", modality, model_name),
                reason: format!("模型编译失败: {}", e),
            })?;

        Ok(Self {
            model_type,
            plan,
            config: config.clone(),
        })
    }

    /// 执行推理 — 输入张量 → ONNX 推理 → 输出 embedding
    ///
    /// # 参数
    /// - `input`: 预处理后的输入张量 (ndarray::ArrayD&lt;f32&gt;)
    ///
    /// # 返回
    /// - 512 维归一化 f32 向量 (L2 归一化，平方和 = 1.0)
    ///
    /// # 实现细节
    /// 1. ndarray → tract Tensor 转换
    /// 2. 执行 plan.run()
    /// 3. 提取第一个输出张量
    /// 4. 调用 postprocess() 归一化
    pub fn run(&self, input: ndarray::ArrayD<f32>) -> Result<Vec<f32>, NmcError> {
        let modality = self.model_type.modality_name();

        // ndarray::ArrayD<f32> → tract Tensor
        let input_tensor: Tensor = input.into();

        // 执行推理
        // Tensor → TValue: plan.run() 接受 TValue (TypedValue) 而非原始 Tensor
        let outputs =
            self.plan
                .run(tvec!(input_tensor.into()))
                .map_err(|e| NmcError::InferenceError {
                    modality: modality.to_string(),
                    reason: format!("推理执行失败: {}", e),
                })?;

        // 提取第一个输出张量
        let output = outputs
            .into_iter()
            .next()
            .ok_or_else(|| NmcError::InferenceError {
                modality: modality.to_string(),
                reason: "模型输出为空".to_string(),
            })?;

        // tract Tensor → ndarray ArrayViewD → owned ArrayD
        // to_array_view 返回 TractResult (而非 Option), 需 map_err 转换
        let array = output
            .to_array_view::<f32>()
            .map_err(|e| NmcError::InferenceError {
                modality: modality.to_string(),
                reason: format!("输出张量类型转换失败: {}", e),
            })?;

        Ok(Self::postprocess(array.to_owned()))
    }

    // ========================================================================
    // 预处理流水线
    // ========================================================================

    /// 图像预处理: 原始字节 → 归一化张量
    ///
    /// 流程: 解码 (PNG/JPEG/WebP) → 缩放 224×224 → 归一化 [0,1] → CHW 格式
    ///
    /// # 参数
    /// - `bytes`: 原始图像字节 (PNG/JPEG/WebP 格式)
    ///
    /// # 返回
    /// - `ndarray::ArrayD<f32>` 形状 (3, 224, 224): CHW 格式
    ///
    /// # 错误
    /// - `PreprocessError`: 空输入或解码失败
    ///
    /// # 实现细节
    /// - 使用 `image` crate 解码，支持 PNG/JPEG/WebP
    /// - 缩放使用 `FilterType::Triangle` (双线性插值的折中方案，质量与速度平衡)
    /// - CHW 格式: 先遍历通道 (R/G/B)，再遍历空间 (H/W)，符合 ONNX NCHW 输入规范
    pub fn preprocess_image(bytes: &[u8]) -> Result<ndarray::ArrayD<f32>, NmcError> {
        if bytes.is_empty() {
            return Err(NmcError::PreprocessError {
                modality: "Image".to_string(),
                reason: "输入为空".to_string(),
            });
        }

        // 解码: PNG/JPEG/WebP
        let img = image::load_from_memory(bytes).map_err(|e| NmcError::PreprocessError {
            modality: "Image".to_string(),
            reason: format!("图像解码失败: {}", e),
        })?;

        // 先转换为 RGB8 再缩放: resize 是泛型函数, 直接在 DynamicImage 上调用
        // 时返回类型为 ImageBuffer<Rgba<u8>, ...> 而非 DynamicImage, 导致后续
        // to_rgb8() 不可用。先显式转换确保类型一致。
        let rgb_img = img.to_rgb8();

        // 缩放至 224×224 (CLIP ViT-B/32 标准输入尺寸)
        let resized =
            image::imageops::resize(&rgb_img, 224, 224, image::imageops::FilterType::Triangle);

        // CHW 格式: 通道优先 (C=3, H=224, W=224)
        let mut data = Vec::with_capacity(3 * 224 * 224);
        for c in 0..3usize {
            for y in 0..224u32 {
                for x in 0..224u32 {
                    let pixel = resized.get_pixel(x, y);
                    // 归一化: [0, 255] → [0, 1]
                    data.push(pixel[c] as f32 / 255.0);
                }
            }
        }

        let array = ndarray::Array3::from_shape_vec((3, 224, 224), data).map_err(|e| {
            NmcError::PreprocessError {
                modality: "Image".to_string(),
                reason: format!("CHW数组构造失败: {}", e),
            }
        })?;

        Ok(array.into_dyn())
    }

    /// 视频预处理: 原始字节 → 批处理帧张量
    ///
    /// 流程: 关键帧采样 (最多 16 帧) → 逐帧图像预处理 → 批处理 (N, 3, 224, 224)
    ///
    /// # 注意
    /// 视频解码较复杂 (需要 ffmpeg/gstreamer 或纯 Rust 解码器)，
    /// **当前占位实现**: 将原始字节作为单帧图像处理，输出形状为 (1, 3, 224, 224)。
    /// 后续接入 `ffmpeg-next` 或 `video-rs` 后实现真正的关键帧采样。
    ///
    /// # 参数
    /// - `bytes`: 原始视频字节 (占位: 视作单帧图像)
    ///
    /// # 错误
    /// - `PreprocessError`: 空输入
    pub fn preprocess_video(bytes: &[u8]) -> Result<ndarray::ArrayD<f32>, NmcError> {
        if bytes.is_empty() {
            return Err(NmcError::PreprocessError {
                modality: "Video".to_string(),
                reason: "输入为空".to_string(),
            });
        }

        // 占位: 将原始字节作为单帧图像处理
        let frame = Self::preprocess_image(bytes)?;

        // 添加 batch 维度: (3, 224, 224) → (1, 3, 224, 224)
        let shape = frame.shape().to_vec();
        let flat: Vec<f32> = frame.iter().copied().collect();

        let mut batch_shape = vec![1usize];
        batch_shape.extend_from_slice(&shape);

        let batch = ndarray::ArrayD::from_shape_vec(batch_shape, flat).map_err(|e| {
            NmcError::PreprocessError {
                modality: "Video".to_string(),
                reason: format!("批处理形状构造失败: {}", e),
            }
        })?;

        Ok(batch)
    }

    /// 音频预处理: 原始字节 → Mel 频谱张量
    ///
    /// 流程: WAV 解码 → 重采样 16kHz 单声道 → 短时傅里叶变换 → Mel 滤波器组 → 对数压缩
    ///
    /// # 注意
    /// 音频预处理较复杂 (需要 FFT、Mel 滤波器组、重采样等信号处理操作)，
    /// **当前占位实现**: 生成零填充张量，维度符合 Whisper encoder 输入要求。
    /// 后续接入 `rustfft` + `hound` 后实现真正的 Mel 频谱提取。
    ///
    /// # 参数
    /// - `bytes`: 原始音频字节 (WAV 格式优先)
    ///
    /// # 返回
    /// - `ndarray::ArrayD<f32>` 形状 (1, 80, 3000): 批大小=1, Mel 频带=80, 时间步=3000
    ///
    /// # 错误
    /// - `PreprocessError`: 空输入
    pub fn preprocess_audio(bytes: &[u8]) -> Result<ndarray::ArrayD<f32>, NmcError> {
        if bytes.is_empty() {
            return Err(NmcError::PreprocessError {
                modality: "Audio".to_string(),
                reason: "输入为空".to_string(),
            });
        }

        // 尝试解析 WAV 头部获取采样率与声道数，用于调整时间步
        let (n_mels, time_steps) = if bytes.len() >= 44 && &bytes[0..4] == b"RIFF" {
            // WAV 头部偏移: 声道数@22 (u16 LE), 采样率@24 (u32 LE)
            let sample_rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
            let channels = u16::from_le_bytes([bytes[22], bytes[23]]);
            // 按采样率比例调整时间步 (基准: 16kHz → 3000 步)
            let time_steps = if sample_rate > 0 {
                ((3000.0 * sample_rate as f32 / 16000.0) as usize).clamp(1, 3000)
            } else {
                3000
            };
            // 声道数 > 1 时增加时间步以容纳多声道信息
            let time_steps = time_steps * channels.max(1) as usize;
            (80, time_steps.min(3000))
        } else {
            // 非 WAV 格式: 使用默认维度
            (80, 3000)
        };

        // 零填充占位张量
        let shape = vec![1usize, n_mels, time_steps];
        let total = shape.iter().product::<usize>();
        let data = vec![0.0f32; total];

        let array = ndarray::ArrayD::from_shape_vec(shape, data).map_err(|e| {
            NmcError::PreprocessError {
                modality: "Audio".to_string(),
                reason: format!("Mel频谱形状构造失败: {}", e),
            }
        })?;

        Ok(array)
    }

    // ========================================================================
    // 后处理
    // ========================================================================

    /// 后处理: 将 ONNX 输出张量归一化为 512 维 f32 向量
    ///
    /// 处理流程:
    /// 1. 展平任意形状张量 → 1D 向量
    /// 2. 截断或零填充至 512 维
    /// 3. L2 归一化 (所有值平方和 = 1.0)
    ///
    /// # 参数
    /// - `output`: ONNX 推理的原始输出张量 (任意形状)
    ///
    /// # 返回
    /// - 512 维 L2 归一化向量
    ///
    /// # 边界情况
    /// - 全零输入 → 返回全零向量 (避免除以零)
    /// - 空张量 → 返回全零 512 维向量
    pub fn postprocess(output: ndarray::ArrayD<f32>) -> Vec<f32> {
        // 展平为 1D
        let flat: Vec<f32> = output.iter().copied().collect();

        // 截断/零填充至 512 维
        let mut result = vec![0.0f32; 512];
        let copy_len = flat.len().min(512);
        result[..copy_len].copy_from_slice(&flat[..copy_len]);

        // L2 归一化
        let norm_sq: f32 = result.iter().map(|v| v * v).sum();
        let norm = norm_sq.sqrt();
        // 避免除以零: 范数极小 (浮点精度) 时视为零向量
        if norm > 1e-10 {
            for v in &mut result {
                *v /= norm;
            }
        }

        result
    }

    /// 返回模型类型
    pub fn model_type(&self) -> ModelType {
        self.model_type
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // preprocess_image 测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_preprocess_image_empty() {
        let result = OnnxBackend::preprocess_image(&[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            NmcError::PreprocessError { modality, reason } => {
                assert_eq!(modality, "Image");
                assert!(reason.contains("为空"));
            }
            _ => panic!("期望 PreprocessError"),
        }
    }

    #[test]
    fn test_preprocess_image_invalid() {
        // 损坏的字节序列 (不是有效的 PNG/JPEG/WebP)
        let corrupt = vec![0x00, 0xFF, 0xDE, 0xAD, 0xBE, 0xEF];
        let result = OnnxBackend::preprocess_image(&corrupt);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            NmcError::PreprocessError { modality, reason } => {
                assert_eq!(modality, "Image");
                assert!(reason.contains("解码失败"));
            }
            _ => panic!("期望 PreprocessError"),
        }
    }

    /// 验证有效 PNG 图像预处理后形状正确
    #[test]
    fn test_preprocess_image_valid_png() {
        // 创建一个最小的 1x1 RGB PNG 图像
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([128, 64, 32]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("编码 PNG 应成功");

        let result = OnnxBackend::preprocess_image(&buf);
        assert!(result.is_ok(), "有效 PNG 应成功预处理: {:?}", result.err());
        let tensor = result.unwrap();
        // CHW: (3, 224, 224) — resize 将 1x1 放大到 224x224
        assert_eq!(tensor.shape(), &[3, 224, 224]);
        // 所有像素值应在 [0, 1] 范围内
        for &v in tensor.iter() {
            assert!((0.0..=1.0).contains(&v), "像素值 {} 超出 [0,1]", v);
        }
    }

    // -----------------------------------------------------------------------
    // preprocess_video 测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_preprocess_video_empty() {
        let result = OnnxBackend::preprocess_video(&[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            NmcError::PreprocessError { modality, reason } => {
                assert_eq!(modality, "Video");
                assert!(reason.contains("为空"));
            }
            _ => panic!("期望 PreprocessError"),
        }
    }

    // -----------------------------------------------------------------------
    // preprocess_audio 测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_preprocess_audio_empty() {
        let result = OnnxBackend::preprocess_audio(&[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            NmcError::PreprocessError { modality, reason } => {
                assert_eq!(modality, "Audio");
                assert!(reason.contains("为空"));
            }
            _ => panic!("期望 PreprocessError"),
        }
    }

    /// 验证非空音频输入生成正确的零填充张量形状
    #[test]
    fn test_preprocess_audio_non_empty() {
        // 非 WAV 格式的任意字节
        let result = OnnxBackend::preprocess_audio(&[0x42; 100]);
        assert!(result.is_ok());
        let tensor = result.unwrap();
        // 默认形状: (1, 80, 3000)
        assert_eq!(tensor.shape(), &[1, 80, 3000]);
        // 所有值应为零
        for &v in tensor.iter() {
            assert!((v - 0.0).abs() < 1e-10, "期望零填充, 实际 {}", v);
        }
    }

    // -----------------------------------------------------------------------
    // postprocess 测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_postprocess_dimension() {
        // 输入任意形状，输出始终为 512 维
        let input = ndarray::ArrayD::from_shape_vec(vec![3, 4, 5], vec![0.1f32; 60]).unwrap();
        let result = OnnxBackend::postprocess(input);
        assert_eq!(result.len(), 512);
    }

    #[test]
    fn test_postprocess_normalize() {
        // 非零向量 → L2 归一化后平方和 ≈ 1.0
        let input = ndarray::ArrayD::from_shape_vec(vec![100], vec![0.5f32; 100]).unwrap();
        let result = OnnxBackend::postprocess(input);
        let norm_sq: f32 = result.iter().map(|v| v * v).sum();
        // 浮点误差容忍 (f32 累加 512 项平方和允许 1e-5 级误差)
        assert!(
            (norm_sq - 1.0).abs() < 1e-5,
            "L2归一化后平方和应为1.0, 实际 {}",
            norm_sq
        );
    }

    #[test]
    fn test_postprocess_padding() {
        // 输入 < 512 维 → 自动零填充到 512 维
        // 使用非零值避免归一化后首元素为零 (1.0 起始而非 0.0)
        let values: Vec<f32> = (1..101).map(|i| i as f32).collect();
        let input = ndarray::ArrayD::from_shape_vec(vec![100], values).unwrap();
        let result = OnnxBackend::postprocess(input);
        assert_eq!(result.len(), 512);
        // 前 100 个值应非零 (归一化后比例不变)
        assert!(result[0] > 0.0, "前100个值应非零 (归一化后)");
        // 后 412 个值应为零
        let tail: f32 = result[100..].iter().map(|v| v.abs()).sum();
        assert!(tail < 1e-10, "填充部分应全零, 实际尾部和 = {}", tail);
    }

    #[test]
    fn test_postprocess_truncation() {
        // 输入 > 512 维 → 自动截断到 512 维
        let input = ndarray::ArrayD::from_shape_vec(vec![1000], vec![0.1f32; 1000]).unwrap();
        let result = OnnxBackend::postprocess(input);
        assert_eq!(result.len(), 512);
        // 截断后归一化，平方和应为 1.0 (允许 f32 累加误差)
        let norm_sq: f32 = result.iter().map(|v| v * v).sum();
        assert!(
            (norm_sq - 1.0).abs() < 1e-5,
            "截断后L2归一化平方和应为1.0, 实际 {}",
            norm_sq
        );
    }

    #[test]
    fn test_postprocess_zero_input() {
        // 全零输入 → 全零输出 (避免除以零)
        let input = ndarray::ArrayD::zeros(vec![10]);
        let result = OnnxBackend::postprocess(input);
        assert_eq!(result.len(), 512);
        let sum: f32 = result.iter().map(|v| v.abs()).sum();
        assert!(sum < 1e-10, "全零输入应返回全零输出, 实际和 = {}", sum);
    }

    #[test]
    fn test_postprocess_empty_tensor() {
        // 空张量 → 全零 512 维输出
        let input = ndarray::ArrayD::from_shape_vec(vec![0], vec![]).unwrap();
        let result = OnnxBackend::postprocess(input);
        assert_eq!(result.len(), 512);
        let sum: f32 = result.iter().map(|v| v.abs()).sum();
        assert!(sum < 1e-10, "空张量应返回全零输出, 实际和 = {}", sum);
    }

    // -----------------------------------------------------------------------
    // ModelType 测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_model_type_get_filename() {
        let config = NmcConfig::default();
        assert_eq!(
            ModelType::Image.get_model_filename(&config),
            "clip-vit-b32.onnx"
        );
        assert_eq!(
            ModelType::Video.get_model_filename(&config),
            "videomae-base.onnx"
        );
        assert_eq!(
            ModelType::Audio.get_model_filename(&config),
            "whisper-encoder.onnx"
        );
    }

    #[test]
    fn test_model_type_custom_filename() {
        let config = NmcConfig::default()
            .with_image_model("custom-clip.onnx")
            .with_video_model("custom-video.onnx")
            .with_audio_model("custom-audio.onnx");
        assert_eq!(
            ModelType::Image.get_model_filename(&config),
            "custom-clip.onnx"
        );
        assert_eq!(
            ModelType::Video.get_model_filename(&config),
            "custom-video.onnx"
        );
        assert_eq!(
            ModelType::Audio.get_model_filename(&config),
            "custom-audio.onnx"
        );
    }

    #[test]
    fn test_model_type_modality_name() {
        assert_eq!(ModelType::Image.modality_name(), "Image");
        assert_eq!(ModelType::Video.modality_name(), "Video");
        assert_eq!(ModelType::Audio.modality_name(), "Audio");
    }

    // -----------------------------------------------------------------------
    // SubTask 7.1: Golden 预处理测试 — 确定性验证预处理管道输出形状与值范围
    // -----------------------------------------------------------------------

    /// Golden 测试: 使用已知像素值的 1x1 PNG 图像 (R=128, G=64, B=32)，
    /// 验证预处理输出为 (3, 224, 224) 且所有值在 [0, 1] 范围。
    ///
    /// 关键断言:
    /// - 形状: CHW 格式 (3, 224, 224) — 3 通道 × 224 高 × 224 宽
    /// - 值范围: 所有像素归一化到 [0, 1]，原始 (128/255, 64/255, 32/255)
    /// - 确定性: 相同输入始终产生相同输出
    #[test]
    fn test_preprocess_image_golden() {
        // 构造已知像素值的 1x1 PNG: R=128, G=64, B=32
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([128, 64, 32]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("编码 PNG 应成功");

        let tensor = OnnxBackend::preprocess_image(&buf).expect("已知 PNG 图像预处理应成功");

        // 形状断言: CHW (3, 224, 224)
        assert_eq!(
            tensor.shape(),
            &[3, 224, 224],
            "Golden: 图像预处理输出形状应为 (3, 224, 224)"
        );

        // 值范围断言: 所有值在 [0, 1] 内
        for &v in tensor.iter() {
            assert!(
                (0.0..=1.0).contains(&v),
                "Golden: 像素值 {} 超出预期范围 [0, 1]",
                v
            );
        }

        // 归一化一致性: resize 后所有像素应接近原始归一化值
        // 1x1 → 224x224 使用 Triangle 滤波器，所有像素插值到接近 (128/255, 64/255, 32/255)
        let expected_r = 128.0 / 255.0;
        let expected_g = 64.0 / 255.0;
        let expected_b = 32.0 / 255.0;
        // R 通道 (索引 0..50176)
        for &v in tensor.iter().take(224 * 224) {
            assert!(
                (v - expected_r).abs() < 0.01,
                "Golden: R 通道值应接近 {}，实际 {}",
                expected_r,
                v
            );
        }
        // G 通道 (跳过 R 通道 50176 个值)
        for &v in tensor.iter().skip(224 * 224).take(224 * 224) {
            assert!(
                (v - expected_g).abs() < 0.01,
                "Golden: G 通道值应接近 {}，实际 {}",
                expected_g,
                v
            );
        }
        // B 通道 (跳过 R+G 通道)
        for &v in tensor.iter().skip(2 * 224 * 224) {
            assert!(
                (v - expected_b).abs() < 0.01,
                "Golden: B 通道值应接近 {}，实际 {}",
                expected_b,
                v
            );
        }
    }

    /// Golden 测试: 验证音频预处理输出张量形状为 (1, 80, 3000)。
    ///
    /// 关键断言:
    /// - 形状: (1, 80, 3000) — batch=1, Mel 频带=80, 时间步=3000
    /// - 当前占位实现输出全零填充
    #[test]
    fn test_preprocess_audio_golden() {
        // 非 WAV 格式的任意字节，使用默认维度
        let audio_bytes = [0x42u8; 100];
        let tensor = OnnxBackend::preprocess_audio(&audio_bytes).expect("非空音频字节预处理应成功");

        // 形状断言: (1, 80, 3000)
        assert_eq!(
            tensor.shape(),
            &[1, 80, 3000],
            "Golden: 音频预处理输出形状应为 (1, 80, 3000)，实际 {:?}",
            tensor.shape()
        );

        // 元素总数断言
        let total = tensor.len();
        assert_eq!(
            total,
            80 * 3000,
            "Golden: 音频张量元素总数应为 {}，实际 {}",
            80 * 3000,
            total
        );

        // 占位实现: 当前所有值应为零
        for &v in tensor.iter() {
            assert!(v.abs() < 1e-10, "Golden: 音频占位张量应全零，实际值 {}", v);
        }
    }

    /// Golden 测试: 验证视频预处理输出张量形状为 (1, 3, 224, 224)，
    /// 额外添加了 batch 维度。
    ///
    /// 关键断言:
    /// - 形状: (1, 3, 224, 224) — batch=1, CHW=3×224×224
    /// - 值范围: [0, 1]（继承自图像预处理管道）
    #[test]
    fn test_preprocess_video_golden() {
        // 使用已知像素值的 1x1 PNG 作为视频帧
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([200, 100, 50]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("编码 PNG 应成功");

        let tensor = OnnxBackend::preprocess_video(&buf).expect("有效 PNG 视频帧预处理应成功");

        // 形状断言: (1, 3, 224, 224) — 添加了 batch 维度
        assert_eq!(
            tensor.shape(),
            &[1, 3, 224, 224],
            "Golden: 视频预处理输出形状应为 (1, 3, 224, 224)，实际 {:?}",
            tensor.shape()
        );

        // 总元素数: 1 × 3 × 224 × 224 = 150528
        let expected_total = 3 * 224 * 224;
        assert_eq!(
            tensor.len(),
            expected_total,
            "Golden: 视频张量元素总数应为 {}，实际 {}",
            expected_total,
            tensor.len()
        );

        // 值范围断言: 所有值在 [0, 1] 内
        for &v in tensor.iter() {
            assert!(
                (0.0..=1.0).contains(&v),
                "Golden: 视频帧像素值 {} 超出预期范围 [0, 1]",
                v
            );
        }
    }

    // -----------------------------------------------------------------------
    // SubTask 7.2: Golden 后处理测试 — 验证后处理管道的归一化、截断、填充行为
    // -----------------------------------------------------------------------

    /// Golden 测试: 输入恰好 512 维，验证 L2 归一化后向量范数为 1.0。
    #[test]
    fn test_postprocess_exact_512() {
        // 构造 512 维非零向量 (使用 1..513 的整数值)
        let values: Vec<f32> = (1..=512).map(|i| i as f32).collect();
        let input =
            ndarray::ArrayD::from_shape_vec(vec![512], values).expect("构造 512 维张量应成功");

        let result = OnnxBackend::postprocess(input);

        // 维度断言: 输入恰好 512 维，输出也为 512 维
        assert_eq!(
            result.len(),
            512,
            "Golden exact_512: 输出维度应为 512，实际 {}",
            result.len()
        );

        // L2 范数断言: 归一化后平方和应接近 1.0
        let norm_sq: f32 = result.iter().map(|v| v * v).sum();
        assert!(
            (norm_sq - 1.0).abs() < 1e-5,
            "Golden exact_512: L2 归一化后平方和应为 1.0，实际 {}",
            norm_sq
        );
    }

    /// Golden 测试: 输入 600 维，验证截断到 512 维。
    #[test]
    fn test_postprocess_truncate() {
        // 构造 600 维向量 (使用 1..=600)
        let values: Vec<f32> = (1..=600).map(|i| i as f32).collect();
        let input =
            ndarray::ArrayD::from_shape_vec(vec![600], values).expect("构造 600 维张量应成功");

        let result = OnnxBackend::postprocess(input);

        // 维度断言: 截断到 512
        assert_eq!(
            result.len(),
            512,
            "Golden truncate: 输出维度应为 512，实际 {}",
            result.len()
        );

        // 截断后归一化: 平方和应为 1.0
        let norm_sq: f32 = result.iter().map(|v| v * v).sum();
        assert!(
            (norm_sq - 1.0).abs() < 1e-5,
            "Golden truncate: 截断后 L2 归一化平方和应为 1.0，实际 {}",
            norm_sq
        );
    }

    /// Golden 测试: 输入 256 维，验证填充到 512 维（末尾补零）。
    #[test]
    fn test_postprocess_pad() {
        // 构造 256 维非零向量 (使用 1..=256)
        let values: Vec<f32> = (1..=256).map(|i| i as f32).collect();
        let input =
            ndarray::ArrayD::from_shape_vec(vec![256], values).expect("构造 256 维张量应成功");

        let result = OnnxBackend::postprocess(input);

        // 维度断言: 填充到 512
        assert_eq!(
            result.len(),
            512,
            "Golden pad: 输出维度应为 512，实际 {}",
            result.len()
        );

        // 前 256 个值应非零（归一化后）
        let front_sum: f32 = result[..256].iter().map(|v| v.abs()).sum();
        assert!(
            front_sum > 1e-10,
            "Golden pad: 前 256 个值归一化后应非零，实际绝对值和 {}",
            front_sum
        );

        // 后 256 个值应为零填充
        let tail_sum: f32 = result[256..].iter().map(|v| v.abs()).sum();
        assert!(
            tail_sum < 1e-10,
            "Golden pad: 末尾 256 个值应为零填充，实际绝对值和 {}",
            tail_sum
        );

        // 归一化: 平方和应为 1.0
        let norm_sq: f32 = result.iter().map(|v| v * v).sum();
        assert!(
            (norm_sq - 1.0).abs() < 1e-5,
            "Golden pad: 填充后 L2 归一化平方和应为 1.0，实际 {}",
            norm_sq
        );
    }

    /// Golden 测试: 全零向量，验证归一化不除零。
    /// 当范数 < 1e-10 时，向量保持不变（全零）。
    #[test]
    fn test_postprocess_zero_norm() {
        // 全零 10 维向量 → 应扩展为全零 512 维
        let input = ndarray::ArrayD::zeros(vec![10]);
        let result = OnnxBackend::postprocess(input);

        // 维度: 填充到 512
        assert_eq!(result.len(), 512, "Golden zero_norm: 输出维度应为 512");

        // 所有值应保持为零（不除零，不产生 NaN/inf）
        let total_abs: f32 = result.iter().map(|v| v.abs()).sum();
        assert!(
            total_abs < 1e-10,
            "Golden zero_norm: 全零输入应返回全零输出，不应除零，实际绝对值和 {}",
            total_abs
        );

        // 确保没有 NaN 或 inf
        for &v in &result {
            assert!(
                v.is_finite(),
                "Golden zero_norm: 输出不应包含 NaN 或 inf，实际 {}",
                v
            );
        }

        // 范数 < 1e-6 时保持不变 (即不执行归一化)
        // 使用小数构造一个范数 = 1e-15 的向量
        let tiny_input =
            ndarray::ArrayD::from_shape_vec(vec![5], vec![1e-16f32; 5]).expect("构造小向量应成功");
        let tiny_result = OnnxBackend::postprocess(tiny_input);
        // 末尾填充部分应为零
        let tail_sum: f32 = tiny_result[5..].iter().map(|v| v.abs()).sum();
        assert!(
            tail_sum < 1e-10,
            "Golden zero_norm: 小范数输入填充部分应为零"
        );
    }
}
