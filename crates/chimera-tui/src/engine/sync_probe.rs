//! engine::sync_probe — CSI 2026 同步输出能力探测(Concord W7 T7.1,ADR-079)
//!
//! 对应架构层:L10 Interface
//!
//! # 职责
//! 判定当前终端是否支持 DEC 2026 同步输出(CSI ? 2026 h/l),供
//! [`super::atomic_frame::AtomicFrameWriter`] 决定是否用同步序列包裹帧。
//!
//! # 探测回退链(WHY 三级,方案 §5.1)
//! 1. **环境变量逃生门**:`CHIMERA_NO_SYNC_OUTPUT=1` → `ForcedOff`。
//!    对齐 pi Agent 的 `PI_NO_SYNC_OUTPUT` 修法:残缺实现的终端(VTE 系)
//!    由用户显式关闭,不依赖探测结果。
//! 2. **tmux 特例**:`$TMUX` 存在时,tmux 的 DECRQM 应答来自 tmux 自身状态
//!    而非真实客户端(tmux#5470 实证),直接探测会误判;改为解析
//!    `tmux -V` 版本,≥3.3a 才视为支持。
//! 3. **DECRQM 探测**:写 `CSI ? 2026 $ p` 查询,100ms 超时读应答;
//!    应答参数 Ps ∈ {1,2,3,4}(模式被识别)→ `Probed`,其余/超时 → `Disabled`。
//!
//! # 设计决策(WHY)
//! - **纯函数 + 薄 IO 分层**:`decide_sync_mode` / `tmux_supports_sync` /
//!   `parse_decrqm_answer` 全部纯函数可单测;IO(读 env、跑 tmux、poll 终端)
//!   收敛在 `probe_sync_output` 一个入口,CI 无 TTY 时探测超时自然降级
//!   `Disabled`,不阻断启动(SEC-3 缓解)。
//! - **三态而非布尔**:`ForcedOff`(用户显式关)与 `Disabled`(能力缺失)
//!   分开,供遥测区分"不愿用"与"不能用"(方案 §9.2 启用率指标)。
//!   能力探测不是运行时 feature flag,不违反 ADR-034。

use std::io::Write;
use std::time::{Duration, Instant};

/// DECRQM 查询序列:请求终端报告私有模式 2026(同步输出)的支持状态
const DECRQM_QUERY: &[u8] = b"\x1b[?2026$p";
/// 同步输出开启序列(CSI ? 2026 h)
pub const SYNC_BEGIN: &[u8] = b"\x1b[?2026h";
/// 同步输出关闭序列(CSI ? 2026 l)
pub const SYNC_END: &[u8] = b"\x1b[?2026l";
/// 探测超时:100ms(方案 §5.1;超时即降级,不阻塞启动)
const PROBE_TIMEOUT: Duration = Duration::from_millis(100);
/// 环境变量逃生门(对齐 pi Agent PI_NO_SYNC_OUTPUT 语义)
pub const NO_SYNC_ENV: &str = "CHIMERA_NO_SYNC_OUTPUT";
/// tmux 支持同步输出的最低版本(3.3a)
const TMUX_MIN: (u32, u32, u8) = (3, 3, b'a');

/// 同步输出模式 — 探测回退链的终态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// 能力探测通过:帧输出用 CSI ? 2026 h/l 包裹
    Probed,
    /// 能力缺失或探测失败:帧输出不包裹(静默降级,不阻断)
    Disabled,
    /// 用户经环境变量显式关闭(逃生门)
    ForcedOff,
}

impl SyncMode {
    /// 是否启用同步序列包裹(仅 `Probed` 启用)
    pub fn wraps_frames(self) -> bool {
        matches!(self, SyncMode::Probed)
    }
}

/// 纯函数决策:一级/二级回退裁决(不含 IO,全分支可单测)
///
/// 返回 `Some(mode)` 表示已可终判;返回 `None` 表示需继续三级 DECRQM 探测。
///
/// # 参数
/// - `no_sync_env`:`CHIMERA_NO_SYNC_OUTPUT` 的值(None = 未设置)
/// - `tmux_env`:`$TMUX` 的值(None = 非 tmux 会话)
/// - `tmux_supported`:tmux 版本白名单判定结果(仅 tmux 分支使用)
pub fn decide_sync_mode(
    no_sync_env: Option<&str>,
    tmux_env: Option<&str>,
    tmux_supported: bool,
) -> Option<SyncMode> {
    // 一级:逃生门("1" 为显式关闭;其余值视为未设置,避免误拼写静默生效)
    if no_sync_env == Some("1") {
        return Some(SyncMode::ForcedOff);
    }
    // 二级:tmux 特例(应答不可信,按版本白名单)
    if tmux_env.is_some() {
        return Some(if tmux_supported {
            SyncMode::Probed
        } else {
            SyncMode::Disabled
        });
    }
    // 三级交由 DECRQM 探测(调用方根据 parse_decrqm_answer 结果回填)
    None
}

/// 解析 `tmux -V` 输出版本是否 ≥ 3.3a
///
/// 输入形如 `tmux 3.3a` / `tmux 3.4` / `tmux next-3.4`;解析失败一律 false
/// (保守降级)。版本序:3.3 < 3.3a < 3.3b < 3.4(无后缀 release 早于
/// 同主次版本的字母补丁版);字母后缀按字节序比较。
pub fn tmux_supports_sync(version_output: &str) -> bool {
    // 取最后一个"含数字"的空白分隔词,并从首个数字处截取,
    // 兼容 "tmux next-3.4" 形态(版本词不以数字开头)
    let token = version_output
        .split_whitespace()
        .rev()
        .find(|w| w.chars().any(|c| c.is_ascii_digit()));
    let Some(token) = token else {
        return false;
    };
    let ver = token
        .find(|c: char| c.is_ascii_digit())
        .map(|i| &token[i..])
        .unwrap_or(token);
    let mut nums = String::new();
    let mut suffix: Option<u8> = None;
    for c in ver.chars() {
        if c.is_ascii_digit() || c == '.' {
            nums.push(c);
        } else if c.is_ascii_alphabetic() && suffix.is_none() {
            suffix = Some(c as u8);
            break;
        } else {
            break;
        }
    }
    let mut parts = nums.trim_end_matches('.').split('.');
    let major: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let (min_major, min_minor, min_suffix) = TMUX_MIN;
    match (major, minor).cmp(&(min_major, min_minor)) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        // 主.次相同:版本序上 3.3 < 3.3a(无后缀 release 早于字母补丁版),
        // 故无后缀 = 不支持;有后缀按字节比较(a 及以上支持)
        std::cmp::Ordering::Equal => match suffix {
            None => false,
            Some(s) => s >= min_suffix,
        },
    }
}

/// 解析 DECRQM 应答字节流,返回是否识别模式 2026
///
/// 应答格式:`CSI ? 2026 ; Ps $ y`,Ps ∈ {1,2,3,4} 表示模式被识别
/// (1=已置位 / 2=已复位 / 3=永久置位 / 4=永久复位),Ps = 0 或无应答 = 不支持。
/// 纯函数:任意字节输入不 panic(proptest 守护)。
pub fn parse_decrqm_answer(bytes: &[u8]) -> Option<bool> {
    // 定位 "?2026;" 后读取 Ps 数字,再校验以 "$y" 收尾
    let marker = b"?2026;";
    let start = bytes
        .windows(marker.len())
        .position(|w| w == marker)
        .map(|p| p + marker.len())?;
    let rest = &bytes[start..];
    let mut ps: u32 = 0;
    let mut digits = 0usize;
    for (i, &b) in rest.iter().enumerate() {
        match b {
            b'0'..=b'9' => {
                ps = ps.saturating_mul(10).saturating_add((b - b'0') as u32);
                digits += 1;
            }
            b'$' => {
                // 需紧跟 'y' 才是合法 DECRPM 收尾
                if digits > 0 && rest.get(i + 1) == Some(&b'y') {
                    // Ps ∈ 1..=4:模式被识别;0 = 无效模式(不支持)
                    return Some((1..=4).contains(&ps));
                }
                return None;
            }
            _ => return None,
        }
    }
    None
}

/// 生产入口:执行完整探测回退链(含 IO)
///
/// 在 `enable_raw_mode()` 之后、EnterAlternateScreen 之前调用一次。
/// 任何 IO 失败/超时均降级 `Disabled`,绝不阻断启动。
pub fn probe_sync_output() -> SyncMode {
    let no_sync = std::env::var(NO_SYNC_ENV).ok();
    let tmux = std::env::var("TMUX").ok().filter(|v| !v.is_empty());

    // tmux 分支:版本白名单替代不可信的 DECRQM 应答(tmux#5470)
    let tmux_supported = tmux
        .as_ref()
        .map(|_| query_tmux_version().is_some_and(|v| tmux_supports_sync(&v)))
        .unwrap_or(false);

    if let Some(mode) = decide_sync_mode(no_sync.as_deref(), tmux.as_deref(), tmux_supported) {
        return mode;
    }
    probe_decrqm()
}

/// 运行 `tmux -V` 获取版本字符串;失败返回 None(保守降级)
fn query_tmux_version() -> Option<String> {
    let out = std::process::Command::new("tmux").arg("-V").output().ok()?;
    if out.status.success() {
        String::from_utf8(out.stdout).ok()
    } else {
        None
    }
}

/// DECRQM 实探测:写查询 → 100ms 内 poll 终端应答 → 解析
fn probe_decrqm() -> SyncMode {
    let mut stdout = std::io::stdout();
    if stdout.write_all(DECRQM_QUERY).is_err() || stdout.flush().is_err() {
        return SyncMode::Disabled;
    }
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut buf: Vec<u8> = Vec::with_capacity(64);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return SyncMode::Disabled;
        }
        match crossterm::event::poll(remaining) {
            Ok(true) => match crossterm::event::read() {
                // 终端应答以可见字符形式到达 raw mode 输入流
                Ok(crossterm::event::Event::Key(key)) => {
                    buf.extend(key.code.to_string().as_bytes());
                }
                Ok(_) => {}
                Err(_) => return SyncMode::Disabled,
            },
            // 超时(false)或错误:无应答 → 降级
            Ok(false) | Err(_) => return SyncMode::Disabled,
        }
        if let Some(supported) = parse_decrqm_answer(&buf) {
            return if supported {
                SyncMode::Probed
            } else {
                SyncMode::Disabled
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 三级回退链(decide/early_decide)─────────────────────

    #[test]
    fn env_escape_hatch_wins_over_everything() {
        assert_eq!(
            decide_sync_mode(Some("1"), Some("/tmp/tmux"), true),
            Some(SyncMode::ForcedOff),
            "逃生门优先级最高,即使 tmux 支持也强制关闭"
        );
    }

    #[test]
    fn env_other_values_do_not_force_off() {
        // 误拼写(如 "true"/"0")不触发逃生门,避免静默生效
        assert_eq!(
            decide_sync_mode(Some("true"), None, false),
            None,
            "非 '1' 值视为未设置,继续后续探测"
        );
        assert_eq!(decide_sync_mode(Some("0"), None, false), None);
    }

    #[test]
    fn tmux_branch_uses_version_whitelist() {
        assert_eq!(
            decide_sync_mode(None, Some("/tmp/tmux-1000/default"), true),
            Some(SyncMode::Probed)
        );
        assert_eq!(
            decide_sync_mode(None, Some("/tmp/tmux-1000/default"), false),
            Some(SyncMode::Disabled),
            "tmux < 3.3a 或版本未知 → 降级"
        );
    }

    #[test]
    fn non_tmux_falls_through_to_decrqm() {
        assert_eq!(decide_sync_mode(None, None, false), None);
    }

    // ── tmux 版本解析 ─────────────────────────────────────

    #[test]
    fn tmux_version_thresholds() {
        assert!(tmux_supports_sync("tmux 3.3a"));
        assert!(tmux_supports_sync("tmux 3.3b"));
        assert!(tmux_supports_sync("tmux 3.4"));
        assert!(tmux_supports_sync("tmux 3.5"));
        assert!(tmux_supports_sync("tmux next-3.4"));
        assert!(!tmux_supports_sync("tmux 3.2a"));
        assert!(
            !tmux_supports_sync("tmux 3.3"),
            "版本序 3.3 早于 3.3a,不支持"
        );
    }

    #[test]
    fn tmux_version_garbage_degrades() {
        assert!(!tmux_supports_sync(""));
        assert!(!tmux_supports_sync("tmux unknown"));
        assert!(!tmux_supports_sync("no digits here"));
    }

    // ── DECRQM 应答解析 ───────────────────────────────────

    #[test]
    fn decrqm_recognized_answers() {
        for ps in ["1", "2", "3", "4"] {
            let ans = format!("\x1b[?2026;{ps}$y");
            assert_eq!(parse_decrqm_answer(ans.as_bytes()), Some(true), "Ps={ps}");
        }
    }

    #[test]
    fn decrqm_invalid_mode_rejected() {
        assert_eq!(parse_decrqm_answer(b"\x1b[?2026;0$y"), Some(false));
        assert_eq!(parse_decrqm_answer(b"\x1b[?2026;9$y"), Some(false));
    }

    #[test]
    fn decrqm_malformed_returns_none() {
        assert_eq!(parse_decrqm_answer(b""), None);
        assert_eq!(parse_decrqm_answer(b"\x1b[?2027;1$y"), None, "其它模式号");
        assert_eq!(parse_decrqm_answer(b"\x1b[?2026;1$x"), None, "非 $y 收尾");
        assert_eq!(parse_decrqm_answer(b"\x1b[?2026;$y"), None, "无 Ps 数字");
        assert_eq!(parse_decrqm_answer(b"\x1b[?2026;1"), None, "未闭合");
    }

    #[test]
    fn decrqm_tolerates_prefix_noise() {
        // 真实终端可能在应答前夹带其它序列
        assert_eq!(
            parse_decrqm_answer(b"\x1b[0m\x1b[?2026;2$y\x1b["),
            Some(true)
        );
    }

    // ── SyncMode 语义 ─────────────────────────────────────

    #[test]
    fn only_probed_wraps_frames() {
        assert!(SyncMode::Probed.wraps_frames());
        assert!(!SyncMode::Disabled.wraps_frames());
        assert!(!SyncMode::ForcedOff.wraps_frames());
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// 不变量①:任意字节序列喂给 DECRQM 解析器不 panic,
        /// 且返回值只能是 None/Some(bool)(SEC-3:恶意应答不崩溃)
        #[test]
        fn parse_decrqm_never_panics_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            let _ = parse_decrqm_answer(&bytes);
        }

        /// 不变量②:tmux 版本解析对任意字符串不 panic
        #[test]
        fn tmux_version_parse_never_panics(s in ".*") {
            let _ = tmux_supports_sync(&s);
        }

        /// 不变量③:决策函数全域无 panic 且逃生门恒为最高优先级
        #[test]
        fn env_escape_hatch_always_wins(
            tmux in proptest::option::of("[a-z/]{0,10}"),
            supported in any::<bool>(),
        ) {
            let mode = decide_sync_mode(Some("1"), tmux.as_deref(), supported);
            prop_assert_eq!(mode, Some(SyncMode::ForcedOff));
        }
    }
}
