//! 多语言CLI支持 — 中文分词与多语言token计数
//!
//! 对应架构层:L10 Interface
//! 对应创新点:P2-8 Qwen多语言CLI优化
//!
//! # 核心机制
//! - 中文文本分词(基于字符+简单规则,作为jieba-rs的降级)
//! - 多语言token计数(中文≈1字1token,英文≈4字3token)
//! - 支持中文/英文/日文/韩文混合文本

/// 多语言文本处理器
#[derive(Debug, Clone, Default)]
pub struct MultilingualProcessor;

impl MultilingualProcessor {
    /// 创建处理器
    pub fn new() -> Self {
        Self
    }

    /// 分词 — 支持中英文混合
    ///
    /// 中文:按字符分词(每个汉字独立)
    /// 英文:按空格和标点分词
    /// 日文/韩文:按字符分词
    pub fn tokenize(&self, text: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut current_word = String::new();

        for ch in text.chars() {
            if ch.is_ascii_whitespace() {
                if !current_word.is_empty() {
                    tokens.push(current_word.clone());
                    current_word.clear();
                }
                continue;
            }

            if ch.is_ascii_alphanumeric() || ch == '_' {
                // ASCII字母数字,累积为单词
                current_word.push(ch);
            } else if is_cjk_char(ch) {
                // CJK字符(中文/日文/韩文),每个字符独立成词
                if !current_word.is_empty() {
                    tokens.push(current_word.clone());
                    current_word.clear();
                }
                tokens.push(ch.to_string());
            } else if is_punctuation(ch) {
                // 标点符号,作为独立token
                if !current_word.is_empty() {
                    tokens.push(current_word.clone());
                    current_word.clear();
                }
                tokens.push(ch.to_string());
            } else {
                // 其他字符(如emoji),作为独立token
                if !current_word.is_empty() {
                    tokens.push(current_word.clone());
                    current_word.clear();
                }
                tokens.push(ch.to_string());
            }
        }

        // 处理末尾未提交的单词
        if !current_word.is_empty() {
            tokens.push(current_word);
        }

        tokens
    }

    /// 估算token数
    ///
    /// 中文:≈1字1token
    /// 英文:≈4字3token(按单词平均长度估算)
    /// 日文/韩文:≈1字1token
    pub fn estimate_tokens(&self, text: &str) -> usize {
        let tokens = self.tokenize(text);
        let mut count = 0usize;

        for token in &tokens {
            if token.chars().any(is_cjk_char) {
                // CJK字符,每个算1 token
                count += token.chars().filter(|&c| is_cjk_char(c)).count();
            } else if token.is_ascii() {
                // 纯ASCII,按单词长度估算
                count += (token.len() as f32 * 0.75).ceil() as usize;
            } else {
                // 其他,每个字符算1 token
                count += token.chars().count();
            }
        }

        count.max(1)
    }

    /// 检测主要语言
    pub fn detect_language(&self, text: &str) -> Language {
        let cjk_count = text.chars().filter(|&c| is_cjk_char(c)).count();
        let ascii_count = text.chars().filter(|c| c.is_ascii_alphabetic()).count();
        let total = text.chars().filter(|c| !c.is_ascii_whitespace()).count();

        if total == 0 {
            return Language::Unknown;
        }

        let cjk_ratio = cjk_count as f32 / total as f32;
        let ascii_ratio = ascii_count as f32 / total as f32;

        if cjk_ratio > 0.5 {
            // 进一步区分中文/日文/韩文
            if text.chars().any(|c| is_hiragana(c) || is_katakana(c)) {
                Language::Japanese
            } else if text.chars().any(is_hangul) {
                Language::Korean
            } else {
                Language::Chinese
            }
        } else if ascii_ratio > 0.5 {
            Language::English
        } else {
            Language::Mixed
        }
    }

    /// 截断文本到指定token数
    pub fn truncate_to_tokens(&self, text: &str, max_tokens: usize) -> String {
        let tokens = self.tokenize(text);
        if tokens.len() <= max_tokens {
            return text.to_string();
        }

        let mut result = String::new();

        for (count, token) in tokens.into_iter().enumerate() {
            if count >= max_tokens {
                break;
            }
            result.push_str(&token);
        }

        result
    }
}

/// 语言类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// 中文
    Chinese,
    /// 英文
    English,
    /// 日文
    Japanese,
    /// 韩文
    Korean,
    /// 混合语言
    Mixed,
    /// 未知
    Unknown,
}

impl Language {
    /// 语言名称
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chinese => "Chinese",
            Self::English => "English",
            Self::Japanese => "Japanese",
            Self::Korean => "Korean",
            Self::Mixed => "Mixed",
            Self::Unknown => "Unknown",
        }
    }
}

/// 判断是否为CJK字符
fn is_cjk_char(ch: char) -> bool {
    // CJK Unified Ideographs: U+4E00 - U+9FFF
    // CJK Unified Ideographs Extension A: U+3400 - U+4DBF
    // CJK Compatibility Ideographs: U+F900 - U+FAFF
    matches!(ch,
        '\u{4E00}'..='\u{9FFF}' |
        '\u{3400}'..='\u{4DBF}' |
        '\u{F900}'..='\u{FAFF}'
    )
}

/// 判断是否为平假名
fn is_hiragana(ch: char) -> bool {
    matches!(ch, '\u{3040}'..='\u{309F}')
}

/// 判断是否为片假名
fn is_katakana(ch: char) -> bool {
    matches!(ch, '\u{30A0}'..='\u{30FF}')
}

/// 判断是否为韩文
fn is_hangul(ch: char) -> bool {
    matches!(ch,
        '\u{AC00}'..='\u{D7AF}' | // Hangul Syllables
        '\u{1100}'..='\u{11FF}' | // Hangul Jamo
        '\u{3130}'..='\u{318F}'   // Hangul Compatibility Jamo
    )
}

/// 判断是否为标点符号
fn is_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation()
        || matches!(ch,
            '\u{3000}'..='\u{303F}' | // CJK Symbols and Punctuation
            '\u{FF00}'..='\u{FFEF}'    // Halfwidth and Fullwidth Forms
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_chinese() {
        let processor = MultilingualProcessor::new();
        let tokens = processor.tokenize("你好世界");
        assert_eq!(tokens, vec!["你", "好", "世", "界"]);
    }

    #[test]
    fn test_tokenize_english() {
        let processor = MultilingualProcessor::new();
        let tokens = processor.tokenize("hello world");
        assert_eq!(tokens, vec!["hello", "world"]);
    }

    #[test]
    fn test_tokenize_mixed() {
        let processor = MultilingualProcessor::new();
        let tokens = processor.tokenize("hello世界");
        assert_eq!(tokens, vec!["hello", "世", "界"]);
    }

    #[test]
    fn test_estimate_tokens_chinese() {
        let processor = MultilingualProcessor::new();
        let count = processor.estimate_tokens("你好世界");
        assert_eq!(count, 4); // 4个汉字 ≈ 4 tokens
    }

    #[test]
    fn test_estimate_tokens_english() {
        let processor = MultilingualProcessor::new();
        let count = processor.estimate_tokens("hello world");
        // 当前实现按 ASCII 单词字符长度 × 0.75 向上取整估算:
        // hello(5) → ceil(3.75)=4, world(5) → 4, 合计 8 tokens。
        // 该估算偏保守(真实 BPE 对常见短词约 1 token/词),但符合函数文档
        // "英文:≈4字3token" 的声明;此处仅验证结果在算法预期范围内。
        assert!(
            (6..=10).contains(&count),
            "英文2单词按当前算法应估算为6-10 tokens,实际 {count}"
        );
    }

    #[test]
    fn test_detect_language_chinese() {
        let processor = MultilingualProcessor::new();
        assert_eq!(processor.detect_language("你好世界"), Language::Chinese);
    }

    #[test]
    fn test_detect_language_english() {
        let processor = MultilingualProcessor::new();
        assert_eq!(processor.detect_language("Hello World"), Language::English);
    }

    #[test]
    fn test_truncate_to_tokens() {
        let processor = MultilingualProcessor::new();
        let truncated = processor.truncate_to_tokens("你好世界", 2);
        assert_eq!(truncated, "你好");
    }

    #[test]
    fn test_is_cjk_char() {
        assert!(is_cjk_char('中'));
        assert!(is_cjk_char('文'));
        assert!(!is_cjk_char('a'));
        assert!(!is_cjk_char('1'));
    }
}
