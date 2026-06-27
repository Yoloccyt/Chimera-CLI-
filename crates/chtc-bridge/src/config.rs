//! CHTC 配置 — 支持的 IDE 列表、超时、并发上限

use crate::types::IdeSource;

/// CHTC 桥接器配置
#[derive(Debug, Clone)]
pub struct ChtcConfig {
    /// 受支持的 IDE 列表,默认包含全部 5 种
    pub supported_ides: Vec<IdeSource>,
    /// 单次工具调用超时(毫秒),默认 5000
    pub call_timeout_ms: u64,
    /// 最大并发调用数,默认 32
    pub max_concurrent_calls: usize,
}

impl Default for ChtcConfig {
    fn default() -> Self {
        Self {
            supported_ides: vec![
                IdeSource::vscode(),
                IdeSource::intellij(),
                IdeSource::vim(),
                IdeSource::emacs(),
                IdeSource::zed(),
            ],
            call_timeout_ms: 5000,
            max_concurrent_calls: 32,
        }
    }
}

impl ChtcConfig {
    /// 判断指定 IDE 是否受支持
    pub fn is_supported(&self, source: &IdeSource) -> bool {
        self.supported_ides
            .iter()
            .any(|s| s.as_str() == source.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = ChtcConfig::default();
        assert_eq!(cfg.supported_ides.len(), 5);
        assert_eq!(cfg.call_timeout_ms, 5000);
        assert_eq!(cfg.max_concurrent_calls, 32);
    }

    #[test]
    fn test_is_supported() {
        let cfg = ChtcConfig::default();
        assert!(cfg.is_supported(&IdeSource::vscode()));
        assert!(cfg.is_supported(&IdeSource::zed()));
    }
}
