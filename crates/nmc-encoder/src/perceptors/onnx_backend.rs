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
/// - `Text` → all-MiniLM-L6-v2 (文本 → 语义嵌入)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelType {
    /// 图像编码模型 (CLIP ViT-B/32)
    Image,
    /// 视频编码模型 (VideoMAE)
    Video,
    /// 音频编码模型 (Whisper encoder)
    Audio,
    /// 文本编码模型 (all-MiniLM-L6-v2)
    Text,
}

impl ModelType {
    /// 返回模态名称字符串 (用于错误消息与日志)
    fn modality_name(&self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Text => "Text",
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
            Self::Text => config
                .text_model
                .as_deref()
                .unwrap_or("all-MiniLM-L6-v2.onnx"),
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
/// pub(crate): 供 TextPerceptor 在 onnx 推理路径中复用
pub(crate) type TractPlan =
    SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

/// ONNX 推理后端 — 封装 tract-onnx 模型加载、预处理、推理、后处理流水线
///
/// # 生命周期
/// 1. `load()` — 加载 ONNX 模型并编译为 tract 执行计划
/// 2. `preprocess_*()` — 将原始字节转换为归一化输入张量
/// 3. `run()` — 执行 ONNX 推理
/// 4. `postprocess()` — 将输出张量归一化为 512 维 f32 向量
///
/// # 示例 (概念性)
/// ```text
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
    /// 流程: 检查 FFmpeg 可用性 → 帧提取 → 逐帧图像预处理 → 批处理 (N, 3, 224, 224)
    ///
    /// 如果 FFmpeg 不可用，保留占位实现并输出 tracing 警告。
    ///
    /// # 参数
    /// - `bytes`: 原始视频字节
    ///
    /// # 返回
    /// - `ndarray::ArrayD<f32>` 形状 (N, 3, 224, 224), N ≤ 16
    ///
    /// # 错误
    /// - `PreprocessError`: 空输入或解码失败
    pub fn preprocess_video(bytes: &[u8]) -> Result<ndarray::ArrayD<f32>, NmcError> {
        if bytes.is_empty() {
            return Err(NmcError::PreprocessError {
                modality: "Video".to_string(),
                reason: "输入为空".to_string(),
            });
        }

        // 检查 FFmpeg 是否可用
        if !is_ffmpeg_available() {
            // FFmpeg 不可用时，保留占位实现
            tracing::warn!("FFmpeg 不可用，视频预处理使用占位实现（单帧模式）");
            let frame = Self::preprocess_image(bytes)?;
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
            return Ok(batch);
        }

        // FFmpeg 可用: 使用 ffmpeg 提取帧
        // 写入临时文件，调用 ffmpeg 提取帧，读取帧数据
        let tmp_dir = std::env::temp_dir().join(format!("chimera_video_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp_dir);

        // 写入临时视频文件
        let video_path = tmp_dir.join("input_video");
        if let Err(e) = std::fs::write(&video_path, bytes) {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(NmcError::PreprocessError {
                modality: "Video".to_string(),
                reason: format!("写入临时视频文件失败: {}", e),
            });
        }

        // 最多提取 16 帧
        let max_frames = 16usize;
        let output_pattern = tmp_dir.join("frame_%04d.png").to_string_lossy().to_string();

        let output = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-i",
                &video_path.to_string_lossy(),
                "-vf",
                &format!("fps={}/1", max_frames as f64),
                "-frames:v",
                &max_frames.to_string(),
                "-q:v",
                "2",
                &output_pattern,
            ])
            .output();

        match output {
            Ok(out) => {
                if !out.status.success() {
                    // FFmpeg 执行失败，清理临时文件并回退到占位实现
                    let _ = std::fs::remove_dir_all(&tmp_dir);
                    tracing::warn!("FFmpeg 帧提取失败, 使用占位实现");
                    let frame = Self::preprocess_image(bytes)?;
                    let shape = frame.shape().to_vec();
                    let flat: Vec<f32> = frame.iter().copied().collect();
                    let mut batch_shape = vec![1usize];
                    batch_shape.extend_from_slice(&shape);
                    let batch =
                        ndarray::ArrayD::from_shape_vec(batch_shape, flat).map_err(|e| {
                            NmcError::PreprocessError {
                                modality: "Video".to_string(),
                                reason: format!("批处理形状构造失败: {}", e),
                            }
                        })?;
                    return Ok(batch);
                }

                // 读取提取的帧
                let mut frames: Vec<ndarray::ArrayD<f32>> = Vec::new();
                for i in 1..=max_frames {
                    let frame_path = tmp_dir.join(format!("frame_{:04}.png", i));
                    if frame_path.exists() {
                        let frame_bytes = match std::fs::read(&frame_path) {
                            Ok(b) => b,
                            Err(e) => {
                                tracing::warn!("读取帧文件失败: {}", e);
                                continue;
                            }
                        };
                        match Self::preprocess_image(&frame_bytes) {
                            Ok(tensor) => frames.push(tensor),
                            Err(e) => tracing::warn!("帧预处理失败: {}", e),
                        }
                    }
                }

                // 清理临时文件
                let _ = std::fs::remove_dir_all(&tmp_dir);

                if frames.is_empty() {
                    // 没有提取到任何帧，回退到占位实现
                    tracing::warn!("FFmpeg 未提取到任何帧，使用占位实现");
                    let frame = Self::preprocess_image(bytes)?;
                    let shape = frame.shape().to_vec();
                    let flat: Vec<f32> = frame.iter().copied().collect();
                    let mut batch_shape = vec![1usize];
                    batch_shape.extend_from_slice(&shape);
                    let batch =
                        ndarray::ArrayD::from_shape_vec(batch_shape, flat).map_err(|e| {
                            NmcError::PreprocessError {
                                modality: "Video".to_string(),
                                reason: format!("批处理形状构造失败: {}", e),
                            }
                        })?;
                    return Ok(batch);
                }

                // 合并为 batch tensor
                let n_frames = frames.len();
                let flat: Vec<f32> = frames.iter().flat_map(|f| f.iter().copied()).collect();
                let batch_shape = vec![n_frames, 3, 224, 224];
                let batch = ndarray::ArrayD::from_shape_vec(batch_shape, flat).map_err(|e| {
                    NmcError::PreprocessError {
                        modality: "Video".to_string(),
                        reason: format!("批处理形状构造失败: {}", e),
                    }
                })?;
                Ok(batch)
            }
            Err(e) => {
                // FFmpeg 命令执行失败
                let _ = std::fs::remove_dir_all(&tmp_dir);
                tracing::warn!("FFmpeg 命令执行失败: {}, 使用占位实现", e);
                let frame = Self::preprocess_image(bytes)?;
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
        }
    }

    /// 音频预处理: 原始 WAV 字节 → Mel 频谱张量
    ///
    /// 流程: WAV 解码 (hound) → 混音为单声道 → 重采样 16kHz → Mel 频谱 (mel_spec)
    ///
    /// # 参数
    /// - `bytes`: 原始 WAV 音频字节
    ///
    /// # 返回
    /// - `ndarray::ArrayD<f32>` 形状 (1, 80, 3000): batch=1, Mel 频带=80, 时间步=3000
    ///
    /// # 错误
    /// - `PreprocessError`: 空输入、WAV 解码失败、频谱计算失败
    pub fn preprocess_audio(bytes: &[u8]) -> Result<ndarray::ArrayD<f32>, NmcError> {
        if bytes.is_empty() {
            return Err(NmcError::PreprocessError {
                modality: "Audio".to_string(),
                reason: "输入为空".to_string(),
            });
        }

        // 1. 使用 hound 解码 WAV
        let mut cursor = std::io::Cursor::new(bytes);
        let reader = hound::WavReader::new(&mut cursor).map_err(|e| NmcError::PreprocessError {
            modality: "Audio".to_string(),
            reason: format!("WAV 解码失败: {}", e),
        })?;

        let spec = reader.spec();
        let sample_rate = spec.sample_rate;
        let channels = spec.channels as usize;
        let bits_per_sample = spec.bits_per_sample;

        // 2. 读取 PCM 样本并转换为 f32
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => {
                // 32-bit float PCM
                reader
                    .into_samples::<f32>()
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| NmcError::PreprocessError {
                        modality: "Audio".to_string(),
                        reason: format!("读取 float PCM 样本失败: {}", e),
                    })?
            }
            hound::SampleFormat::Int => {
                // 整数 PCM, 根据位深转换为 f32
                match bits_per_sample {
                    16 => {
                        let max_val = i16::MAX as f32;
                        reader
                            .into_samples::<i16>()
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|e| NmcError::PreprocessError {
                                modality: "Audio".to_string(),
                                reason: format!("读取 i16 PCM 样本失败: {}", e),
                            })?
                            .into_iter()
                            .map(|s| s as f32 / max_val)
                            .collect()
                    }
                    24 => {
                        // 24-bit 样本: hound 通过 i32 读取，左对齐
                        let max_val = i32::MAX as f32;
                        reader
                            .into_samples::<i32>()
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|e| NmcError::PreprocessError {
                                modality: "Audio".to_string(),
                                reason: format!("读取 i24 PCM 样本失败: {}", e),
                            })?
                            .into_iter()
                            .map(|s| (s >> 8) as f32 / max_val)
                            .collect()
                    }
                    32 => {
                        let max_val = i32::MAX as f32;
                        reader
                            .into_samples::<i32>()
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|e| NmcError::PreprocessError {
                                modality: "Audio".to_string(),
                                reason: format!("读取 i32 PCM 样本失败: {}", e),
                            })?
                            .into_iter()
                            .map(|s| s as f32 / max_val)
                            .collect()
                    }
                    _ => {
                        return Err(NmcError::PreprocessError {
                            modality: "Audio".to_string(),
                            reason: format!("不支持的位深: {} bit", bits_per_sample),
                        });
                    }
                }
            }
        };

        // 3. 混音为单声道 (多声道取平均)
        let mono: Vec<f32> = if channels > 1 {
            samples
                .chunks(channels)
                .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
                .collect()
        } else {
            samples
        };

        // 4. 重采样到 16kHz (若原始采样率不同)
        let resampled = if sample_rate != 16000 {
            resample_audio(&mono, sample_rate, 16000)
        } else {
            mono
        };

        // 5. 计算 Mel 频谱
        // 使用 mel_spec 库的 compute_mel_spectrogram_cpu 直接计算 Whisper 兼容的 log-mel 频谱
        // 参数: fft_size=400, hop_size=160, n_mels=80, sampling_rate=16000.0
        let mel_frames = mel_spec::stft::Spectrogram::compute_mel_spectrogram_cpu(
            &resampled, 400,     // fft_size
            160,     // hop_size
            80,      // n_mels
            16000.0, // sampling_rate
        );

        // mel_spec 输出为 Vec<Vec<f32>>, 形状为 (n_mels, n_frames)
        // 需要重塑为 (1, 80, 3000) — 添加 batch 维度
        let n_mels = 80usize;
        let target_time = 3000usize;

        // 找出实际帧数
        let actual_frames = if mel_frames.is_empty() {
            0
        } else {
            mel_frames[0].len()
        };

        let mut mel_data = Vec::with_capacity(n_mels * target_time);
        for m in 0..n_mels {
            if m < mel_frames.len() {
                let row = &mel_frames[m];
                for t in 0..target_time {
                    if t < actual_frames && t < row.len() {
                        mel_data.push(row[t]);
                    } else {
                        mel_data.push(0.0);
                    }
                }
            } else {
                mel_data.extend(std::iter::repeat_n(0.0, target_time));
            }
        }

        // 最终输出形状 (1, 80, 3000)
        let shape = vec![1usize, n_mels, target_time];
        let array = ndarray::ArrayD::from_shape_vec(shape, mel_data).map_err(|e| {
            NmcError::PreprocessError {
                modality: "Audio".to_string(),
                reason: format!("Mel 频谱形状构造失败: {}", e),
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
// 辅助函数
// ============================================================================

/// 检查 FFmpeg 系统命令是否可用
///
/// 执行 `ffmpeg -version` 检测 FFmpeg 是否已安装。
/// 不阻塞，使用 `std::process::Command` 快速检查。
fn is_ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 音频重采样 — 线性插值重采样到目标采样率
///
/// # 参数
/// - `samples`: 输入 PCM f32 样本
/// - `from_rate`: 原始采样率 (Hz)
/// - `to_rate`: 目标采样率 (Hz, 通常 16000)
///
/// # 返回
/// - 重采样后的 f32 样本向量
fn resample_audio(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }

    let ratio = to_rate as f64 / from_rate as f64;
    let new_len = (samples.len() as f64 * ratio).ceil() as usize;
    let mut resampled = Vec::with_capacity(new_len);

    for i in 0..new_len {
        let src_pos = i as f64 / ratio;
        let src_idx = src_pos.floor() as usize;
        let frac = src_pos - src_idx as f64;

        if src_idx + 1 < samples.len() {
            // 线性插值
            let val = samples[src_idx] as f64 * (1.0 - frac) + samples[src_idx + 1] as f64 * frac;
            resampled.push(val as f32);
        } else {
            resampled.push(samples[samples.len() - 1]);
        }
    }

    resampled
}

/// 生成合成正弦波 WAV 文件字节（用于测试）
///
/// # 参数
/// - `sample_rate`: 采样率 (Hz)
/// - `duration_secs`: 持续时间 (秒)
/// - `frequency`: 正弦波频率 (Hz)
/// - `amplitude`: 振幅 (0.0-1.0)
///
/// # 返回
/// - WAV 文件字节 (16-bit PCM, 单声道)
#[cfg(test)]
fn generate_sine_wav(
    sample_rate: u32,
    duration_secs: f32,
    frequency: f32,
    amplitude: f32,
) -> Vec<u8> {
    let num_samples = (sample_rate as f32 * duration_secs) as usize;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut buf = Vec::new();
    {
        let mut writer = hound::WavWriter::new(std::io::Cursor::new(&mut buf), spec).unwrap();
        for i in 0..num_samples {
            let t = i as f32 / sample_rate as f32;
            let sample_value = (amplitude
                * (2.0 * std::f32::consts::PI * frequency * t).sin()
                * i16::MAX as f32) as i16;
            writer.write_sample(sample_value).unwrap();
        }
        writer.finalize().unwrap();
    }
    buf
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

    /// 验证合成 WAV 文件预处理后输出形状正确且值非零
    #[test]
    fn test_preprocess_audio_synthetic_wav() {
        // 生成 1 秒 440Hz 正弦波 WAV, 16kHz 采样率
        let wav_bytes = generate_sine_wav(16000, 1.0, 440.0, 0.5);
        let result = OnnxBackend::preprocess_audio(&wav_bytes);
        assert!(result.is_ok(), "合成 WAV 预处理应成功: {:?}", result.err());
        let tensor = result.unwrap();
        // 形状: (1, 80, 3000)
        assert_eq!(tensor.shape(), &[1, 80, 3000]);
        // 值应为非零 (Mel 频谱)
        let sum: f32 = tensor.iter().map(|v| v.abs()).sum();
        assert!(sum > 0.0, "Mel 频谱应非零");
    }

    /// 验证损坏的 WAV 文件返回 PreprocessError
    #[test]
    fn test_preprocess_audio_corrupt() {
        // 损坏的字节序列 (不是有效的 WAV)
        let corrupt = vec![0x00, 0xFF, 0xDE, 0xAD, 0xBE, 0xEF];
        let result = OnnxBackend::preprocess_audio(&corrupt);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            NmcError::PreprocessError { modality, reason } => {
                assert_eq!(modality, "Audio");
                assert!(reason.contains("解码失败") || reason.contains("WAV"));
            }
            _ => panic!("期望 PreprocessError"),
        }
    }

    /// 验证非 WAV 格式返回 PreprocessError
    #[test]
    fn test_preprocess_audio_non_wav() {
        // 带有有效 RIFF 头部但无效内容的字节
        let fake_wav = {
            let mut bytes = Vec::with_capacity(44);
            bytes.extend_from_slice(b"RIFF");
            bytes.extend_from_slice(&[0u8; 4]); // 文件大小
            bytes.extend_from_slice(b"WAVE");
            bytes.extend_from_slice(b"fmt ");
            bytes.extend_from_slice(&[16u8; 4]); // 块大小
            bytes.extend_from_slice(&[1u8; 2]); // PCM 格式
            bytes.extend_from_slice(&[1u8; 2]); // 单声道
            bytes.extend_from_slice(&[0x80, 0x3E, 0x00, 0x00]); // 16000 Hz
            bytes.extend_from_slice(&[0x00, 0x7D, 0x00, 0x00]); // 字节率
            bytes.extend_from_slice(&[2u8; 2]); // 块对齐
            bytes.extend_from_slice(&[16u8; 2]); // 位深
            bytes.extend_from_slice(b"data");
            bytes.extend_from_slice(&[0u8; 4]); // 数据大小
            bytes
        };
        let result = OnnxBackend::preprocess_audio(&fake_wav);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            NmcError::PreprocessError { modality, .. } => {
                assert_eq!(modality, "Audio");
            }
            _ => panic!("期望 PreprocessError"),
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
        // 默认 text_model=None, 应返回 fallback 名称
        assert_eq!(
            ModelType::Text.get_model_filename(&config),
            "all-MiniLM-L6-v2.onnx"
        );
    }

    #[test]
    fn test_model_type_custom_filename() {
        let config = NmcConfig::default()
            .with_image_model("custom-clip.onnx")
            .with_video_model("custom-video.onnx")
            .with_audio_model("custom-audio.onnx")
            .with_text_model("custom-text.onnx");
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
        assert_eq!(
            ModelType::Text.get_model_filename(&config),
            "custom-text.onnx"
        );
    }

    #[test]
    fn test_model_type_modality_name() {
        assert_eq!(ModelType::Image.modality_name(), "Image");
        assert_eq!(ModelType::Video.modality_name(), "Video");
        assert_eq!(ModelType::Audio.modality_name(), "Audio");
        assert_eq!(ModelType::Text.modality_name(), "Text");
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

    /// Golden 测试: 验证音频预处理输出张量形状为 (1, 80, 3000) 且值非零。
    ///
    /// 关键断言:
    /// - 形状: (1, 80, 3000) — batch=1, Mel 频带=80, 时间步=3000
    /// - 值非零: 合成正弦波 WAV 应产生非零 Mel 频谱
    #[test]
    fn test_preprocess_audio_golden() {
        // 生成 1 秒 440Hz 正弦波 WAV, 16kHz 采样率
        let wav_bytes = generate_sine_wav(16000, 1.0, 440.0, 0.5);
        let tensor = OnnxBackend::preprocess_audio(&wav_bytes).expect("合成 WAV 预处理应成功");

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

        // 值非零: 合成正弦波应产生非零 Mel 频谱
        let sum: f32 = tensor.iter().map(|v| v.abs()).sum();
        assert!(
            sum > 0.0,
            "Golden: 合成 WAV 的 Mel 频谱应非零，实际和 {}",
            sum
        );
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

    /// 验证损坏的视频数据返回 PreprocessError
    #[test]
    fn test_preprocess_video_corrupt() {
        // 损坏的字节序列 (不是有效的图像/视频格式)
        let corrupt = vec![0x00, 0xFF, 0xDE, 0xAD, 0xBE, 0xEF];
        let result = OnnxBackend::preprocess_video(&corrupt);
        assert!(result.is_err());
        // 由于 FFmpeg 不可用时回退到 preprocess_image，modality 可能为 "Image" 或 "Video"
        // 只要返回 PreprocessError 即可
        match result.unwrap_err() {
            NmcError::PreprocessError { .. } => {} // OK
            other => panic!("期望 PreprocessError, 实际: {:?}", other),
        }
    }

    /// 验证 FFmpeg 不可用时使用占位实现
    /// 由于测试环境一般没有 FFmpeg，此测试验证降级行为
    #[test]
    fn test_preprocess_video_ffmpeg_unavailable() {
        // 使用有效的 PNG 图像字节作为视频输入
        // 当 FFmpeg 不可用时，应回退到 preprocess_image 并输出 (1, 3, 224, 224)
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([200, 100, 50]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("编码 PNG 应成功");

        let tensor = OnnxBackend::preprocess_video(&buf).expect("降级到占位实现应成功");
        assert_eq!(tensor.shape(), &[1, 3, 224, 224]);
        // 值范围应在 [0, 1] 内
        for &v in tensor.iter() {
            assert!((0.0..=1.0).contains(&v), "像素值 {} 超出范围 [0,1]", v);
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
