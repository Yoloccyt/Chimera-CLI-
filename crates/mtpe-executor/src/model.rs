//! MTPE 模型封装 — 基于 NMC 编码器的 ONNX 推理
//!
//! 对应架构层:L7 Execution
//!
//! 包含 MtpeModel 结构体,封装 NMC 编码器的 ONNX 推理后端。
//! 模型加载失败时自动回退到伪预测(生成器模式)。

use std::path::PathBuf;

use nmc_encoder::config::NmcConfig;
use nmc_encoder::perceptors::{ModelType, OnnxBackend};
use tracing::warn;

/// MTPE 模型 — 包装 NMC 编码器提供真实预测
///
/// 封装 NMC 编码器的 OnnxBackend,为 MTPE 提供 ONNX 模型推理能力。
/// 模型文件不存在或加载失败时,`backend` 字段为 None,
/// 调用方回退到伪预测(即现有的 `generate_pseudo_predictions`)。
///
/// # 设计决策
/// 使用 Option<OnnxBackend> 而非 Result:
/// 模型加载失败是预期行为(无模型文件),不应阻断调用方流程。
pub struct MtpeModel {
    /// NMC ONNX 推理后端（可选，模型加载失败时降级）
    backend: Option<OnnxBackend>,
    /// 模型路径
    model_path: PathBuf,
    /// 是否已加载
    loaded: bool,
}

impl MtpeModel {
    /// 创建 MtpeModel 实例 — 尝试加载 ONNX 模型
    ///
    /// # 参数
    /// - `model_path`: ONNX 模型文件路径
    ///
    /// # 行为
    /// 1. 检查模型文件是否存在
    /// 2. 文件存在时使用 NMC 编码器尝试加载
    /// 3. 加载失败时记录警告,保持 `backend` 为 None
    /// 4. 文件不存在时记录信息,保持 `backend` 为 None
    pub fn new(model_path: PathBuf) -> Self {
        // 检查模型文件是否存在
        if !model_path.exists() {
            warn!(
                model_path = %model_path.display(),
                "MTPE ONNX 模型文件不存在,将使用伪预测回退"
            );
            return Self {
                model_path,
                backend: None,
                loaded: false,
            };
        }

        // 使用 NMC 编码器配置加载 ONNX 模型
        // 构造 NmcConfig,设置模型目录为模型文件所在目录
        let model_dir = model_path
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .to_string();
        let model_name = model_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let config = NmcConfig::default()
            .with_model_dir(&model_dir)
            .with_text_model(&model_name);

        // 加载 Text 类型的 ONNX 模型(文本编码器用于 MTPE 预测)
        match OnnxBackend::load(&config, ModelType::Text) {
            Ok(backend) => {
                tracing::info!(
                    model_path = %model_path.display(),
                    "MTPE ONNX 模型加载成功"
                );
                Self {
                    model_path,
                    backend: Some(backend),
                    loaded: true,
                }
            }
            Err(e) => {
                warn!(
                    model_path = %model_path.display(),
                    error = %e,
                    "MTPE ONNX 模型加载失败,将使用伪预测回退"
                );
                Self {
                    model_path,
                    backend: None,
                    loaded: false,
                }
            }
        }
    }

    /// 检查模型是否已加载
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// 获取模型路径
    pub fn model_path(&self) -> &PathBuf {
        &self.model_path
    }

    /// 执行推理 — 有模型时调用 NMC 编码器推理,无模型时返回 None
    ///
    /// # 参数
    /// - `input_tensor`: 输入 tensor 数据(一维 f32 数组)
    ///
    /// # 返回
    /// - `Some(Vec<f32>)`: 推理结果(当模型已加载时)
    /// - `None`: 模型未加载,调用方应回退到伪预测
    pub fn predict(&self, input_tensor: Vec<f32>) -> Option<Vec<f32>> {
        let backend = self.backend.as_ref()?;

        // 将 Vec<f32> 转换为 ndarray::ArrayD<f32>
        let shape = vec![1usize, input_tensor.len()];
        let array = ndarray::ArrayD::from_shape_vec(shape, input_tensor).ok()?;

        match backend.run(array) {
            Ok(output) => Some(output),
            Err(e) => {
                warn!(
                    error = %e,
                    "MTPE ONNX 推理失败"
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mtpe_model_not_loaded_when_no_file() {
        // 模型文件不存在时,backend 应为 None
        let model = MtpeModel::new(PathBuf::from("/nonexistent/model.onnx"));
        assert!(
            !model.is_loaded(),
            "模型文件不存在时 is_loaded 应返回 false"
        );
        assert!(
            model.predict(vec![0.0; 512]).is_none(),
            "模型未加载时 predict 应返回 None"
        );
    }

    #[test]
    fn test_mtpe_model_path_getter() {
        let path = PathBuf::from("/tmp/test_model.onnx");
        let model = MtpeModel::new(path.clone());
        assert_eq!(model.model_path(), &path);
    }

    #[test]
    fn test_mtpe_model_default_not_loaded() {
        // 默认情况下(无模型文件)不应加载
        let model = MtpeModel::new(PathBuf::from("/tmp/nonexistent.onnx"));
        assert!(!model.is_loaded());
    }

    #[test]
    fn test_mtpe_model_predict_not_loaded() {
        // 模型未加载时 predict 返回 None
        let model = MtpeModel::new(PathBuf::from("/nonexistent/model.onnx"));
        assert!(model.predict(vec![0.1; 512]).is_none());
    }
}
