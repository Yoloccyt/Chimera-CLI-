//! 写入敏感信息扫描引擎 — 记忆/上下文/会话持久化前的敏感载荷拦截（v2.3 §(b.5)）
//!
//! 对应架构层: **L0 Contracts**（纯类型 + 无 IO 无状态纯函数，ADR-033 先例：
//! 与 `archive_monotonicity` / `mcsm` / `util::xts_top_k` 同级的纯函数工具）
//!
//! # WHY（本模块存在的理由）
//! v2.3 核验发现：记忆面写入路径**无任何敏感扫描**（`mlc`/`hcw`/`cmt` 写路径 0 命中，
//! `repo-wiki` 仅查询注入防护，`session-store` 仅视图面剔除、敏感载荷仍写盘）。
//! 敏感信息（密钥/令牌/私钥）一旦落入 SQLite/JSONL/WAL 持久层即长期驻留。
//!
//! # 落点决策（依赖铁律）
//! 写入点跨 L2(mlc) / L3(scc/session/cmt) / L5(repo-wiki) / L9(checkpoint) / L10(mcp-mesh)。
//! 一个跨层复用的扫描原语**只能放在它们都能向下依赖的层**（L(N)→L(0) 恒允许；
//! 反之 L2/L3 不能依赖 L4 `seccore`——向上违规）。又因 nexus-contracts 铁律「唯一外部
//! 依赖 = serde，禁 `regex`」，本扫描器**刻意不用正则**：绝大多数密钥/凭证由**固定前缀
//! /字面标记**即可识别（PEM `-----BEGIN`、云厂商 `AKIA`、GitHub `ghp_`、Slack `xoxb-`、
//! OpenAI `sk-` 等），配合小写化 `contains` + 键名匹配即可高置信命中，无需正则回溯。
//!
//! # 动作分级（可配置）
//! 纯函数只做**识别**（返回命中清单 + 建议处置），不改变数据、不做 IO；调用方（各写入口）
//! 依据返回的 [`ScanVerdict`] 决定「打码入库 / 拦截不入库 / 原样放行」。默认策略见
//! [`ScanPolicy`]。

use serde::{Deserialize, Serialize};

/// 扫描命中的敏感类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SensitiveKind {
    /// 私钥/证书 PEM 块（`-----BEGIN ... PRIVATE KEY-----`）
    PrivateKeyPem,
    /// 云厂商访问凭证前缀（AWS `AKIA`/`ASIA`、GCP `AIza`、Azure 等）
    CloudCredential,
    /// 第三方服务令牌（GitHub `ghp_`/`gho_`/`github_pat_`、Slack `xoxb-`/`xoxp-`、
    /// OpenAI/Anthropic `sk-` 等）
    ServiceToken,
    /// 显式键名赋值的秘密（`password=` / `api_key:` / `secret=` / `bearer ` 等）
    SecretKeyValue,
    /// 疑似高熵字符串（长随机 token 启发式，见 [`looks_high_entropy`]）
    HighEntropy,
}

/// 建议处置动作分级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SensitiveDisposition {
    /// 放行（无命中）
    Pass,
    /// 打码后入库（脱敏命中内容，保留可审计残迹）
    Redact,
    /// 拦截不入库（拒绝写入该载荷）
    Block,
}

/// 单次命中记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanHit {
    /// 命中的类别
    pub kind: SensitiveKind,
    /// 命中的标记（固定字面量或小写化后的键名；不含原始敏感值，避免二次泄露）
    pub marker: String,
    /// 命中在输入文本中的字节起始偏移（best-effort，供上层定位脱敏区间）
    pub byte_offset: usize,
}

/// 扫描结论：命中清单 + 按策略聚合出的最终处置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanVerdict {
    /// 最终处置（无命中时为 [`SensitiveDisposition::Pass`]）
    pub disposition: SensitiveDisposition,
    /// 全部命中（按出现顺序）
    pub hits: Vec<ScanHit>,
}

impl ScanVerdict {
    /// 是否应阻断写入（`Block`）。
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        self.disposition == SensitiveDisposition::Block
    }

    /// 是否干净（无命中）。
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.hits.is_empty()
    }
}

/// 扫描策略：为每个类别配置处置级别，决定命中后的整体动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanPolicy {
    /// 私钥 PEM 的处置（默认 [`SensitiveDisposition::Block`]：绝不落盘）
    pub on_private_key: SensitiveDisposition,
    /// 云凭证的处置（默认 `Block`）
    pub on_cloud_credential: SensitiveDisposition,
    /// 服务令牌的处置（默认 `Block`）
    pub on_service_token: SensitiveDisposition,
    /// 键名秘密的处置（默认 [`SensitiveDisposition::Redact`]：打码入库）
    pub on_secret_kv: SensitiveDisposition,
    /// 高熵串的处置（默认 `Redact`）
    pub on_high_entropy: SensitiveDisposition,
}

impl Default for ScanPolicy {
    fn default() -> Self {
        Self {
            on_private_key: SensitiveDisposition::Block,
            on_cloud_credential: SensitiveDisposition::Block,
            on_service_token: SensitiveDisposition::Block,
            on_secret_kv: SensitiveDisposition::Redact,
            on_high_entropy: SensitiveDisposition::Redact,
        }
    }
}

/// 固定前缀/字面标记表（小写；不含正则）。私钥、云凭证、服务令牌各自的判别串。
const PRIVATE_KEY_MARKERS: &[&str] = &["-----begin", "private key-----"];
const CLOUD_CRED_MARKERS: &[&str] = &[
    "akia",
    "asia",
    "aient",
    "aws_secret_access_key",
    "aws-access-key-id",
];
const SERVICE_TOKEN_MARKERS: &[&str] = &[
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "aiza",
    "sk-ant-",
];
/// 键名秘密（`key=value`/`key: value` 中的 key）。
const SECRET_KEY_NAMES: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "api_key",
    "apikey",
    "access_token",
    "auth_token",
    "refresh_token",
    "private_key",
    "client_secret",
    "bearer",
];

/// 处置优先级（用于把多个命中聚合为一个整体处置）：`Block > Redact > Pass`。
const fn disposition_rank(d: SensitiveDisposition) -> u8 {
    match d {
        SensitiveDisposition::Pass => 0,
        SensitiveDisposition::Redact => 1,
        SensitiveDisposition::Block => 2,
    }
}

/// 对输入文本做敏感扫描（纯函数，无 IO、无状态、无正则、大小写不敏感匹配标记）。
///
/// 返回 [`ScanVerdict`]：命中清单 + 按 [`ScanPolicy`] 聚合出的整体处置。
/// 上层写入口据此选择打码 / 拦截 / 放行。
#[must_use = "扫描结论决定写入处置，忽略返回值等于未扫描"]
pub fn scan(text: &str, policy: &ScanPolicy) -> ScanVerdict {
    let lower = text.to_lowercase();
    let mut hits: Vec<ScanHit> = Vec::new();
    let mut worst = SensitiveDisposition::Pass;

    let mut bump = |kind: SensitiveKind, marker: &str, offset: usize, disp| {
        if disposition_rank(disp) > disposition_rank(worst) {
            worst = disp;
        }
        hits.push(ScanHit {
            kind,
            marker: marker.to_string(),
            byte_offset: offset,
        });
    };

    for m in PRIVATE_KEY_MARKERS {
        if let Some(i) = lower.find(m) {
            bump(SensitiveKind::PrivateKeyPem, m, i, policy.on_private_key);
        }
    }
    for m in CLOUD_CRED_MARKERS {
        if let Some(i) = lower.find(m) {
            bump(
                SensitiveKind::CloudCredential,
                m,
                i,
                policy.on_cloud_credential,
            );
        }
    }
    for m in SERVICE_TOKEN_MARKERS {
        if let Some(i) = lower.find(m) {
            bump(SensitiveKind::ServiceToken, m, i, policy.on_service_token);
        }
    }
    for k in SECRET_KEY_NAMES {
        if let Some(i) = lower.find(k) {
            bump(SensitiveKind::SecretKeyValue, k, i, policy.on_secret_kv);
        }
    }
    // 高熵启发式：对空白切分出的长 token 抽样判断。
    for (tok, start) in token_spans(text) {
        if looks_high_entropy(tok) {
            bump(
                SensitiveKind::HighEntropy,
                "<token>",
                start,
                policy.on_high_entropy,
            );
        }
    }

    ScanVerdict {
        disposition: worst,
        hits,
    }
}

/// 高熵判定（无外部依赖的启发式）。
///
/// 规则：长度 ≥ 20，且字符类别（小写/大写/数字/符号）≥ 2 种混合、字母数字占比高，
/// 视为可疑随机 token。阈值刻意保守以降低误报（误报由上层 `Redact` 消化，不阻断）。
#[must_use]
pub fn looks_high_entropy(tok: &str) -> bool {
    let bytes = tok.as_bytes();
    if bytes.len() < 20 {
        return false;
    }
    let (mut lower, mut upper, mut digit, mut symbol, mut other) = (0, 0, 0, 0, 0);
    for &b in bytes {
        match b {
            b'a'..=b'z' => lower += 1,
            b'A'..=b'Z' => upper += 1,
            b'0'..=b'9' => digit += 1,
            b'-' | b'_' | b'=' | b'+' | b'/' => symbol += 1,
            _ => other += 1,
        }
    }
    let classes = [lower > 0, upper > 0, digit > 0, symbol > 0]
        .iter()
        .filter(|x| **x)
        .count();
    // 至少 2 类混合，且字母数字+符号占比 ≥ 0.9（排除普通长句），且无空格类 other。
    let alnum = lower + upper + digit + symbol;
    classes >= 2 && other == 0 && (alnum as f64) >= (bytes.len() as f64) * 0.9
}

/// 迭代文本中以空白分隔的 token 及其字节起始偏移（供高熵抽样）。
fn token_spans(text: &str) -> Vec<(&str, usize)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // 跳过空白
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if start < i {
            if let Some(s) = text.get(start..i) {
                out.push((s, start));
            }
        }
    }
    out
}

/// 依据 [`ScanPolicy`] 对文本做处置，返回（处理后文本，扫描结论）。
///
/// - [`SensitiveDisposition::Pass`] → 原文返回（无命中）；
/// - [`SensitiveDisposition::Block`] → 返回空串（调用方据 `verdict.is_blocked()` **拒绝入库**）；
/// - [`SensitiveDisposition::Redact`] → 按命中类别脱敏后返回：高熵 token 与键名秘密的**值**
///   均以 `__REDACTED__` 掩码，保留可审计残迹（键名 / 良性词），空白按单空格归一。
///
/// 纯函数、无 IO；上层写入口据此决定落盘内容。
#[must_use = "脱敏结果决定写入内容，忽略返回值等于未脱敏"]
pub fn redact(text: &str, policy: &ScanPolicy) -> (String, ScanVerdict) {
    let verdict = scan(text, policy);
    match verdict.disposition {
        SensitiveDisposition::Pass => (text.to_string(), verdict),
        SensitiveDisposition::Block => (String::new(), verdict),
        SensitiveDisposition::Redact => (mask_sensitive_tokens(text), verdict),
    }
}

/// Redact 处置的掩码实现（纯函数）：逐 token 处理，掩掉两类敏感载荷——
/// ① 高熵 token；② 键名秘密的**值**（`key=value` / `key:value` 形态掩右值；
/// 裸键名 token 则掩其后的值 token）。键名本身与良性词保留（可审计残迹）。
///
/// WHY（回归 B-P1-1）：此前 Redact 只掩高熵 token，低熵口令（`password=hunter2`、
/// `password hunter2short`）命中 `SecretKeyValue` 却原样透传 → 敏感值泄露。
fn mask_sensitive_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut mask_next = false;
    for (i, (tok, _)) in token_spans(text).into_iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        // 上一 token 为裸键名（如 `password` / `api_key:`）→ 当前为其值，掩掉。
        if mask_next {
            out.push_str("__REDACTED__");
            mask_next = false;
            continue;
        }
        // ① 高熵 token 整体掩码。
        if looks_high_entropy(tok) {
            out.push_str("__REDACTED__");
            continue;
        }
        // ② `key=value` / `key:value`：键名（大小写不敏感含秘密词）命中则掩右值。
        if let Some(pos) = tok.find(['=', ':']) {
            let key_lower = tok[..pos].to_lowercase();
            if SECRET_KEY_NAMES.iter().any(|k| key_lower.contains(k)) {
                out.push_str(&tok[..pos]); // 保留键名（原文大小写）
                out.push_str(&tok[pos..pos + 1]); // 保留分隔符 = 或 :
                if pos + 1 < tok.len() {
                    out.push_str("__REDACTED__"); // 值在本 token 内
                } else {
                    mask_next = true; // 值在下一个 token（`api_key: value`）
                }
                continue;
            }
        }
        // ③ 裸键名 token（整词命中秘密表）：保留键名，掩其后的值 token。
        if SECRET_KEY_NAMES.contains(&tok.to_lowercase().as_str()) {
            out.push_str(tok);
            mask_next = true;
            continue;
        }
        out.push_str(tok);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> ScanPolicy {
        ScanPolicy::default()
    }

    #[test]
    fn clean_text_passes() {
        let v = scan("just a normal memory note about caching", &policy());
        assert!(v.is_clean(), "hits={:?}", v.hits);
        assert_eq!(v.disposition, SensitiveDisposition::Pass);
    }

    #[test]
    fn private_key_blocked() {
        let v = scan(
            "here is a key:\n-----BEGIN RSA PRIVATE KEY-----\nabc",
            &policy(),
        );
        assert_eq!(v.disposition, SensitiveDisposition::Block);
        assert!(v.is_blocked());
        assert!(v
            .hits
            .iter()
            .any(|h| h.kind == SensitiveKind::PrivateKeyPem));
    }

    #[test]
    fn cloud_and_service_token_blocked() {
        let v = scan("AWS key AKIAIOSFODNN7EXAMPLE", &policy());
        assert!(v.is_blocked());
        let v2 = scan("github token ghp_abcdefghijklmnop", &policy());
        assert!(v2.is_blocked());
    }

    #[test]
    fn secret_kv_redacts_not_blocks() {
        let v = scan("config: password hunter2short", &policy());
        assert_eq!(v.disposition, SensitiveDisposition::Redact);
        assert!(v
            .hits
            .iter()
            .any(|h| h.kind == SensitiveKind::SecretKeyValue));
    }

    #[test]
    fn high_entropy_detects_random_token() {
        let tok = "aZ3xK9mQ2wEr7TyU1iOp4sDf6gHj8kL";
        assert!(
            looks_high_entropy(tok),
            "long mixed token should be high entropy"
        );
        assert!(
            !looks_high_entropy("the quick brown fox jumps over"),
            "prose is not"
        );
        let v = scan(&format!("cache value {tok}"), &policy());
        assert!(v.hits.iter().any(|h| h.kind == SensitiveKind::HighEntropy));
    }

    #[test]
    fn case_insensitive_marker_match() {
        // 大写标记亦须命中（内部小写化）
        let v = scan("BEARER sometokenhere", &policy());
        assert!(v
            .hits
            .iter()
            .any(|h| h.kind == SensitiveKind::SecretKeyValue));
    }

    #[test]
    fn disposition_priority_is_block_over_redact() {
        let v = scan("password=xxx and -----BEGIN PRIVATE KEY-----", &policy());
        assert_eq!(v.disposition, SensitiveDisposition::Block, "worst-of wins");
    }

    #[test]
    fn redact_benign_unchanged() {
        let (out, v) = redact("hello normal note", &policy());
        assert!(v.is_clean());
        assert_eq!(out, "hello normal note");
    }

    #[test]
    fn redact_masks_high_entropy() {
        let tok = "aZ3xK9mQ2wEr7TyU1iOp4sDf6gHj8kL";
        let (out, v) = redact(&format!("cache value {tok}"), &policy());
        assert_eq!(v.disposition, SensitiveDisposition::Redact);
        assert!(out.contains("__REDACTED__"), "out={out}");
        assert!(!out.contains(tok), "secret token must be masked");
        assert!(out.contains("cache value"), "benign words preserved");
    }

    #[test]
    fn redact_block_returns_empty() {
        let (out, v) = redact("-----BEGIN PRIVATE KEY----- xyz", &policy());
        assert!(v.is_blocked());
        assert!(out.is_empty(), "blocked payload must not be stored");
    }

    #[test]
    fn redact_masks_secret_kv_value_not_leaked() {
        // 回归 B-P1-1：Redact 此前只掩高熵 token，低熵口令原样透传 → 泄露。
        // key=value 形态：掩右值、保留键名。
        let (out, v) = redact("login password=hunter2 account u1", &policy());
        assert_eq!(v.disposition, SensitiveDisposition::Redact);
        assert!(
            !out.contains("hunter2"),
            "secret value must be masked, out={out}"
        );
        assert!(out.contains("password="), "审计残迹：键名保留");
        // 裸键名 + 值形态：掩值 token。
        let (out2, v2) = redact("config: password hunter2short", &policy());
        assert_eq!(v2.disposition, SensitiveDisposition::Redact);
        assert!(
            !out2.contains("hunter2short"),
            "bare-name value must be masked, out={out2}"
        );
        assert!(out2.contains("password"), "键名保留");
    }

    /// §(b.5) 误报率基线采样：写点采用的 Block-only 策略（高熵/键名设 Pass）
    /// 对真实良性技术载荷（SHA256/提交号/base64/校验和/配置路径）应**零误报**（不阻断）。
    #[test]
    fn write_point_block_only_policy_has_zero_false_positives() {
        let wp = ScanPolicy {
            on_high_entropy: SensitiveDisposition::Pass,
            on_secret_kv: SensitiveDisposition::Pass,
            ..Default::default()
        };
        let benign = [
            "commit 1ead632ab19f refactored router-traits retirement",
            "sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "content hash 3f7869c7 a9d1b2c3 d4e5f6a7 (blake2 checksum line)",
            "image data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB",
            "cache key scc:warm:8f14e45fceea167a5a36dedd4bea2543 entries=42",
            "path D:/Chimera CLI/.toolchain/cargo/bin/cargo.exe loaded",
            "model=qwen-max temp=0.2 prompt_tokens=1500 completion=900",
        ];
        for s in benign {
            let v = scan(s, &wp);
            assert!(
                !v.is_blocked(),
                "benign payload must NOT be blocked (false positive): {s}"
            );
        }
    }
}
