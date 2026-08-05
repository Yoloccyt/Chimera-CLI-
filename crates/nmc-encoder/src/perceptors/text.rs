//! 文本感知器 — 将文本输入编码为认知元素
//!
//! 对应架构层:L2 Memory
//!
//! # 实现说明
//! 当前实现支持两种路径:
//! 1. **ONNX 语义嵌入路径**: 当配置了 model_dir 且 text_model 文件存在时，
//!    使用 tract-onnx + tokenizers 加载 all-MiniLM-L6-v2 模型进行语义嵌入
//! 2. **字节频率降级路径**: 未配置 ONNX 模型时，使用 SHA256 + 字节频率统计
//!
//! ONNX 推理路径流程:
//! 1. tokenizers 分词 → input_ids + attention_mask
//! 2. 构造 ndarray 张量输入
//! 3. tract-onnx 推理 → last_hidden_state (1, seq_len, 384)
//! 4. mean-pooling (忽略 padding tokens)
//! 5. L2 归一化 → 384 维语义嵌入

use crate::config::NmcConfig;
use crate::error::NmcError;
use crate::perceptors::onnx_backend::TractPlan;
use crate::perceptors::{byte_frequency_embedding, sha256_hex, Perceptor};
use crate::types::{CognitiveElement, Modality, PerceptionInput};
use ndarray;
use tract_onnx::prelude::*;

/// all-MiniLM-L6-v2 最大序列长度
const MAX_SEQ_LEN: usize = 256;

/// all-MiniLM-L6-v2 输出维度
const TEXT_EMBEDDING_DIM: usize = 384;

/// 文本感知器 — 支持 ONNX 语义嵌入与字节频率降级
///
/// 当 ONNX 模型可用时,使用 all-MiniLM-L6-v2 生成 384 维语义嵌入;
/// 否则降级到字节频率统计嵌入。
pub struct TextPerceptor {
    /// 配置(含 text_dim 维度参数)
    config: NmcConfig,
    /// 可选的 tokenizers 分词器(None 表示降级)
    tokenizer: Option<tokenizers::Tokenizer>,
    /// 可选的 tract ONNX 推理计划(None 表示降级)
    plan: Option<TractPlan>,
}

impl TextPerceptor {
    /// 创建文本感知器
    ///
    /// 当 `config.model_dir` 非空且 `config.text_model` 为 Some 时,
    /// 尝试加载 ONNX 模型和 tokenizer。加载失败时记录 warning 并降级到字节频率。
    pub fn new(config: NmcConfig) -> Self {
        let (tokenizer, plan) = Self::try_load_onnx(&config);

        Self {
            config,
            tokenizer,
            plan,
        }
    }

    /// 尝试加载 ONNX 模型和 tokenizer
    ///
    /// 返回 (tokenizer, plan) 元组,两者都为 Some 时表示 ONNX 路径可用。
    /// 任一加载失败时记录 warning 并返回 (None, None) 以降级。
    fn try_load_onnx(config: &NmcConfig) -> (Option<tokenizers::Tokenizer>, Option<TractPlan>) {
        // 只有在 model_dir 非空且 text_model 已指定时才尝试加载
        if config.model_dir.is_empty() || config.text_model.is_none() {
            return (None, None);
        }

        // 加载 tokenizer.json
        let tokenizer_path = config.model_path("tokenizer.json");
        let tokenizer = match tokenizers::Tokenizer::from_file(&tokenizer_path) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(
                    "tokenizer.json 加载失败 ({}), 降级到字节频率嵌入: {}",
                    tokenizer_path,
                    e
                );
                None
            }
        };

        // 加载 ONNX 模型
        let model_name = config
            .text_model
            .as_deref()
            .unwrap_or("all-MiniLM-L6-v2.onnx");
        let model_path = config.model_path(model_name);
        let plan = match tract_onnx::onnx()
            .model_for_path(&model_path)
            .and_then(|m| m.into_typed())
            .and_then(|m| m.into_runnable())
        {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!(
                    "ONNX 文本模型加载失败 ({}), 降级到字节频率嵌入: {}",
                    model_path,
                    e
                );
                None
            }
        };

        // 只有 tokenizer 和 plan 都加载成功时才启用 ONNX 路径
        if tokenizer.is_some() && plan.is_some() {
            (tokenizer, plan)
        } else {
            (None, None)
        }
    }

    /// 返回配置引用
    pub fn config(&self) -> &NmcConfig {
        &self.config
    }

    /// ONNX 推理路径: 文本 → tokenizer → tract-onnx → mean-pooling → L2 归一化
    fn onnx_embedding(&self, text: &str) -> Result<Vec<f32>, NmcError> {
        let tokenizer = self
            .tokenizer
            .as_ref()
            .ok_or_else(|| NmcError::EncodingFailed {
                modality: "Text".to_string(),
                reason: "tokenizer 未初始化".to_string(),
            })?;
        let plan = self.plan.as_ref().ok_or_else(|| NmcError::EncodingFailed {
            modality: "Text".to_string(),
            reason: "ONNX 推理计划未初始化".to_string(),
        })?;

        // 1. 分词
        let encoding = tokenizer
            .encode(text, true)
            .map_err(|e| NmcError::PreprocessError {
                modality: "Text".to_string(),
                reason: format!("分词失败: {}", e),
            })?;

        let ids = encoding.get_ids();
        let attention_mask = encoding.get_attention_mask();

        // 截断到 MAX_SEQ_LEN
        let seq_len = ids.len().min(MAX_SEQ_LEN);
        let ids: Vec<i64> = ids[..seq_len].iter().map(|&v| v as i64).collect();
        let mask: Vec<i64> = attention_mask[..seq_len]
            .iter()
            .map(|&v| v as i64)
            .collect();

        // 2. 构造 ndarray 输入张量 (1, seq_len)
        let input_ids_array =
            ndarray::Array2::<i64>::from_shape_vec([1, seq_len], ids).map_err(|e| {
                NmcError::PreprocessError {
                    modality: "Text".to_string(),
                    reason: format!("input_ids 张量构造失败: {}", e),
                }
            })?;
        let attention_mask_array = ndarray::Array2::<i64>::from_shape_vec([1, seq_len], mask)
            .map_err(|e| NmcError::PreprocessError {
                modality: "Text".to_string(),
                reason: format!("attention_mask 张量构造失败: {}", e),
            })?;

        // 3. ndarray → tract Tensor (into_dyn() 转换为动态维度)
        // 注意: attention_mask_array 需要在 mean_pooling 中再次使用, 故 clone
        let input_ids_tensor: Tensor = input_ids_array.into_dyn().into();
        let attention_mask_tensor: Tensor = attention_mask_array.clone().into_dyn().into();

        // 4. 执行推理 (双输入: input_ids + attention_mask)
        let outputs = plan
            .run(tvec!(input_ids_tensor.into(), attention_mask_tensor.into()))
            .map_err(|e| NmcError::InferenceError {
                modality: "Text".to_string(),
                reason: format!("推理执行失败: {}", e),
            })?;

        // 5. 提取第一个输出张量 (last_hidden_state)
        let output = outputs
            .into_iter()
            .next()
            .ok_or_else(|| NmcError::InferenceError {
                modality: "Text".to_string(),
                reason: "模型输出为空".to_string(),
            })?;

        // tract Tensor → ndarray ArrayViewD
        let array = output
            .to_array_view::<f32>()
            .map_err(|e| NmcError::InferenceError {
                modality: "Text".to_string(),
                reason: format!("输出张量类型转换失败: {}", e),
            })?;

        // 6. mean-pooling + L2 归一化
        Ok(Self::mean_pooling(&array, &attention_mask_array))
    }

    /// mean-pooling + L2 归一化
    ///
    /// 对 last_hidden_state 进行 mean-pooling,忽略 attention_mask=0 的 padding 位置,
    /// 然后对结果进行 L2 归一化。
    ///
    /// # 参数
    /// - `hidden_state`: ONNX 模型输出的 last_hidden_state, 形状 (1, seq_len, 384)
    /// - `attention_mask`: 输入 attention_mask, 形状 (1, seq_len)
    ///
    /// # 返回
    /// - 384 维 L2 归一化向量
    fn mean_pooling(
        hidden_state: &ndarray::ArrayViewD<'_, f32>,
        attention_mask: &ndarray::Array2<i64>,
    ) -> Vec<f32> {
        let shape = hidden_state.shape();
        // 期望形状: (1, seq_len, 384), 安全处理任意维度
        let seq_len = if shape.len() >= 2 {
            shape[shape.len() - 2]
        } else {
            0
        };
        let dim = if shape.len() >= 3 {
            shape[shape.len() - 1]
        } else {
            TEXT_EMBEDDING_DIM
        };

        let mut pooled = vec![0.0f32; dim];
        let mut mask_sum = 0i64;

        // 逐 token 迭代,累加非 padding 位置的嵌入
        for s in 0..seq_len {
            let mask_val = attention_mask[[0, s]];
            if mask_val > 0 {
                for d in 0..dim {
                    pooled[d] += hidden_state[[0, s, d]];
                }
                mask_sum += mask_val;
            }
        }

        // Mean: 除以有效 token 数
        if mask_sum > 0 {
            let mask_sum_f = mask_sum as f32;
            for v in &mut pooled {
                *v /= mask_sum_f;
            }
        }

        // L2 归一化
        let norm_sq: f32 = pooled.iter().map(|v| v * v).sum();
        let norm = norm_sq.sqrt();
        if norm > 1e-10 {
            for v in &mut pooled {
                *v /= norm;
            }
        }

        pooled
    }
}

impl Perceptor for TextPerceptor {
    fn modality(&self) -> Modality {
        Modality::Text
    }

    fn perceive(&self, input: &PerceptionInput) -> Result<CognitiveElement, NmcError> {
        let text = match input {
            PerceptionInput::Text(t) => t.as_str(),
            other => {
                return Err(NmcError::InvalidModality {
                    reason: format!("TextPerceptor 仅接受 Text 输入,收到 {}", other.modality()),
                });
            }
        };

        // content_hash: SHA256 of UTF-8 bytes
        let content_hash = sha256_hex(text.as_bytes());

        // 选择嵌入路径: ONNX 语义嵌入 (可用时) 或字节频率降级
        let embedding = if self.tokenizer.is_some() && self.plan.is_some() {
            self.onnx_embedding(text)?
        } else {
            byte_frequency_embedding(text.as_bytes(), self.config.text_dim)
        };

        Ok(CognitiveElement::new(
            Modality::Text,
            content_hash,
            embedding,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NmcConfig;

    // ========================================================================
    // 字节频率降级路径测试 (向后兼容)
    // ========================================================================

    #[test]
    fn test_text_perceptor_empty_text() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem = p.perceive(&PerceptionInput::Text(String::new())).unwrap();
        assert_eq!(elem.modality, Modality::Text);
        assert_eq!(elem.embedding_dim(), 256);
        // 空文本:所有桶为 0.0
        assert!(elem.embedding.iter().all(|&v| v == 0.0));
        // 空文本仍有有效哈希
        assert!(!elem.content_hash.is_empty());
    }

    #[test]
    fn test_text_perceptor_long_text_10kb() {
        let p = TextPerceptor::new(NmcConfig::default());
        let long_text = "a".repeat(10_000);
        let elem = p
            .perceive(&PerceptionInput::Text(long_text.clone()))
            .unwrap();
        assert_eq!(elem.embedding_dim(), 256);
        // 全 'a' 文本:字节 0x61 对应桶 0x61 % 256 = 97,该桶值为 1.0
        assert!((elem.embedding[97] - 1.0).abs() < 1e-6);
        // 其余桶为 0.0
        for (i, &v) in elem.embedding.iter().enumerate() {
            if i != 97 {
                assert!(v.abs() < 1e-6, "桶 {i} 应为 0,实际为 {v}");
            }
        }
    }

    #[test]
    fn test_text_perceptor_chinese() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem = p
            .perceive(&PerceptionInput::Text("你好世界".into()))
            .unwrap();
        assert_eq!(elem.embedding_dim(), 256);
        // 中文 UTF-8 编码为多字节,频率应分布在多个桶
        let non_zero = elem.embedding.iter().filter(|&&v| v > 0.0).count();
        assert!(non_zero > 0, "中文文本应产生非零嵌入");
        // 频率之和应接近 1.0(归一化)
        let sum: f32 = elem.embedding.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "频率之和应为 1.0,实际为 {sum}");
    }

    #[test]
    fn test_text_perceptor_unicode_emoji() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem = p
            .perceive(&PerceptionInput::Text("Hello 🌍🚀".into()))
            .unwrap();
        assert_eq!(elem.embedding_dim(), 256);
        let sum: f32 = elem.embedding.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_text_perceptor_special_chars() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem = p
            .perceive(&PerceptionInput::Text("!@#$%^&*()".into()))
            .unwrap();
        assert_eq!(elem.embedding_dim(), 256);
        let sum: f32 = elem.embedding.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_text_perceptor_repeated_text() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem1 = p.perceive(&PerceptionInput::Text("abcabc".into())).unwrap();
        let elem2 = p.perceive(&PerceptionInput::Text("abcabc".into())).unwrap();
        // 相同文本产生相同哈希与嵌入
        assert_eq!(elem1.content_hash, elem2.content_hash);
        assert_eq!(elem1.embedding, elem2.embedding);
    }

    #[test]
    fn test_text_perceptor_content_hash_deterministic() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem1 = p.perceive(&PerceptionInput::Text("hello".into())).unwrap();
        let elem2 = p.perceive(&PerceptionInput::Text("hello".into())).unwrap();
        assert_eq!(elem1.content_hash, elem2.content_hash);
        // SHA256 of "hello" 应为固定值
        assert_eq!(
            elem1.content_hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn test_text_perceptor_different_text_different_hash() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem1 = p.perceive(&PerceptionInput::Text("hello".into())).unwrap();
        let elem2 = p.perceive(&PerceptionInput::Text("world".into())).unwrap();
        assert_ne!(elem1.content_hash, elem2.content_hash);
    }

    #[test]
    fn test_text_perceptor_wrong_modality() {
        let p = TextPerceptor::new(NmcConfig::default());
        let result = p.perceive(&PerceptionInput::Image(vec![1, 2, 3]));
        assert!(matches!(result, Err(NmcError::InvalidModality { .. })));
    }

    #[test]
    fn test_text_perceptor_custom_text_dim() {
        let config = NmcConfig::default().with_text_dim(128);
        let p = TextPerceptor::new(config);
        let elem = p.perceive(&PerceptionInput::Text("test".into())).unwrap();
        assert_eq!(elem.embedding_dim(), 128);
    }

    #[test]
    fn test_text_perceptor_embedding_normalized() {
        let p = TextPerceptor::new(NmcConfig::default());
        let elem = p
            .perceive(&PerceptionInput::Text("The quick brown fox".into()))
            .unwrap();
        let sum: f32 = elem.embedding.iter().sum();
        // 非空文本的频率之和应为 1.0
        assert!((sum - 1.0).abs() < 1e-5, "频率之和应为 1.0,实际为 {sum}");
    }

    // ========================================================================
    // 降级路径测试: 未配置 model_dir 时使用字节频率
    // ========================================================================

    #[test]
    fn test_text_perceptor_fallback_when_no_model_dir() {
        // model_dir 为空时,即使 text_model 有值也不加载 ONNX
        let config = NmcConfig::default().with_text_model("all-MiniLM-L6-v2.onnx");
        let p = TextPerceptor::new(config);
        let elem = p
            .perceive(&PerceptionInput::Text("fallback test".into()))
            .unwrap();
        // 应使用字节频率嵌入,维度为 256
        assert_eq!(elem.embedding_dim(), 256);
        // 验证确实是字节频率: 字节频率之和为 1.0
        let sum: f32 = elem.embedding.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_text_perceptor_fallback_when_model_dir_empty() {
        // model_dir 为空字符串,应降级
        let config = NmcConfig::new()
            .with_model_dir("")
            .with_text_model("all-MiniLM-L6-v2.onnx");
        let p = TextPerceptor::new(config);
        let elem = p.perceive(&PerceptionInput::Text("test".into())).unwrap();
        assert_eq!(elem.embedding_dim(), 256);
    }

    #[test]
    fn test_text_perceptor_fallback_when_text_model_none() {
        // text_model 为 None,即使 model_dir 非空也不加载 ONNX
        let config = NmcConfig::new().with_model_dir("/some/path");
        let p = TextPerceptor::new(config);
        let elem = p.perceive(&PerceptionInput::Text("test".into())).unwrap();
        assert_eq!(elem.embedding_dim(), 256);
    }

    // ========================================================================
    // mean-pooling 后处理函数测试
    // ========================================================================

    #[test]
    fn test_mean_pooling_normal_case() {
        // 构造一个简单的 last_hidden_state (1, 3, 4) 和 attention_mask (1, 3)
        // token 0: [1.0, 2.0, 3.0, 4.0], mask=1
        // token 1: [5.0, 6.0, 7.0, 8.0], mask=1
        // token 2: [9.0, 10.0, 11.0, 12.0], mask=0 (padding)
        let data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ];
        let hidden = ndarray::ArrayD::from_shape_vec(vec![1, 3, 4], data).unwrap();
        let mask = ndarray::Array2::<i64>::from_shape_vec([1, 3], vec![1, 1, 0]).unwrap();

        let result = TextPerceptor::mean_pooling(&hidden.view(), &mask);

        // 预期: 只有 token 0 和 token 1 参与平均
        // pooled = [(1+5)/2, (2+6)/2, (3+7)/2, (4+8)/2] = [3.0, 4.0, 5.0, 6.0]
        // L2 norm = sqrt(9+16+25+36) = sqrt(86) ≈ 9.2736
        // normalized = [3.0/9.2736, 4.0/9.2736, 5.0/9.2736, 6.0/9.2736]
        // WHY 数组而非 vec!: 只读遍历,无需堆分配(clippy useless_vec)
        let expected_pooled = [3.0, 4.0, 5.0, 6.0];
        let norm_sq: f32 = expected_pooled.iter().map(|v| v * v).sum();
        let norm = norm_sq.sqrt();
        let expected: Vec<f32> = expected_pooled.iter().map(|v| v / norm).collect();

        assert_eq!(result.len(), 4);
        for (r, e) in result.iter().zip(expected.iter()) {
            assert!((r - e).abs() < 1e-5, "期望 {} 实际 {}", e, r);
        }
    }

    #[test]
    fn test_mean_pooling_all_padding() {
        // 所有 token 都是 padding (mask=0), 应返回全零向量
        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let hidden = ndarray::ArrayD::from_shape_vec(vec![1, 2, 3], data).unwrap();
        let mask = ndarray::Array2::<i64>::from_shape_vec([1, 2], vec![0, 0]).unwrap();

        let result = TextPerceptor::mean_pooling(&hidden.view(), &mask);

        // 全零向量
        assert_eq!(result.len(), 3);
        for v in &result {
            assert!((*v).abs() < 1e-10, "期望 0.0, 实际 {}", v);
        }
    }

    #[test]
    fn test_mean_pooling_single_token() {
        // 只有一个有效 token
        let data = vec![2.0, 4.0, 6.0];
        let hidden = ndarray::ArrayD::from_shape_vec(vec![1, 1, 3], data).unwrap();
        let mask = ndarray::Array2::<i64>::from_shape_vec([1, 1], vec![1]).unwrap();

        let result = TextPerceptor::mean_pooling(&hidden.view(), &mask);

        // pooled = [2.0, 4.0, 6.0]
        // L2 norm = sqrt(4+16+36) = sqrt(56) ≈ 7.4833
        // normalized = [2/7.4833, 4/7.4833, 6/7.4833]
        let norm_sq: f32 = 4.0 + 16.0 + 36.0;
        let norm = norm_sq.sqrt();
        let expected: Vec<f32> = vec![2.0 / norm, 4.0 / norm, 6.0 / norm];

        assert_eq!(result.len(), 3);
        for (r, e) in result.iter().zip(expected.iter()) {
            assert!((r - e).abs() < 1e-5, "期望 {} 实际 {}", e, r);
        }
    }

    #[test]
    fn test_mean_pooling_l2_normalized() {
        // 验证 mean-pooling 后 L2 范数接近 1.0
        let data: Vec<f32> = (0..12).map(|i| (i + 1) as f32).collect();
        let hidden = ndarray::ArrayD::from_shape_vec(vec![1, 3, 4], data).unwrap();
        let mask = ndarray::Array2::<i64>::from_shape_vec([1, 3], vec![1, 1, 1]).unwrap();

        let result = TextPerceptor::mean_pooling(&hidden.view(), &mask);

        let norm_sq: f32 = result.iter().map(|v| v * v).sum();
        assert!(
            (norm_sq - 1.0).abs() < 1e-5,
            "L2 归一化后平方和应为 1.0, 实际 {}",
            norm_sq
        );
    }
}
