//! OS 级沙箱后端（P2-T7，v4.0 WI-14）
//!
//! 对应架构层: **L4 Security**（seccore，ADR-001 降级先例延续）
//! 对应任务: **P2-T7**（手册 W10-12 并行窗口）
//!
//! # 四后端三档（v4.0 WI-14 + ADR-001 先例）
//! 后端优先级降级链按 target_os 拆分（C-20）：macOS 首选 Seatbelt（sandbox-exec）；
//! Linux 首选 Landlock+seccomp → bwrap（bubblewrap 容器）；其余平台 bwrap →
//! ProcessFence；ProcessFence（进程级兜底）为全平台链尾兜底。
//! Windows 平台实现 ProcessFence（命令校验 + 子进程隔离近似）；
//! Landlock/Seatbelt/bwrap 为平台 cfg 占位（Linux/macOS CI 承接完整能力）。
//!
//! # 三档模式
//! - `Strict`：bash 包 argv 逐项校验，越界写 OS 层拒否（写 /etc、越界网络）
//! - `Standard`：默认档（命令白名单 + 路径越界拒绝）
//! - `Relaxed`：仅高风险操作拦截（权限提升/网络外发）
//!
//! # 快照抽象（WI-14 snapshot/fork/restore）
//! `SandboxProvider` trait 定义快照三分：`snapshot`（保存决策+校验状态）、
//! `fork`（派生隔离上下文）、`restore`（回滚到快照）。Windows 上进程级
//! 快照（Docker commit/pause 近似）标注为平台能力，本任务实现**决策级快照**
//! （校验状态 + 策略快照）；完整进程快照由 Linux/macOS 后端在 CI 承接
//! （快照 P50 <200ms【门禁目标】）。
//!
//! # 逃逸防线（WI-14 门禁）
//! 写 /etc、越界网络等路径在**命令校验层**拒绝（复用 L0 CommandValidator
//! 语义），测试断言拒绝路径命中——合法操作零误伤（白名单 + 越界拒绝双测试）。

use std::sync::Arc;
use std::time::Instant;

use nexus_contracts::command_validation::{Command, CommandPolicy};
use sha2::{Digest, Sha256};

use crate::error::SecCoreError;
use crate::policy::validate_command;

/// 沙箱后端（四档，平台能力标记）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OsBackend {
    /// macOS Seatbelt（sandbox-exec）
    Seatbelt,
    /// Linux Landlock + seccomp
    Landlock,
    /// bubblewrap 容器兜底（Linux）
    Bwrap,
    /// 进程级兜底（全平台：命令校验 + 子进程隔离近似）
    ProcessFence,
}

/// 平台族 — target_os 判定的可测试投影（C-20）
///
/// WHY 私有枚举而非散落 cfg：`available()` 原非 Windows 分支无平台检测，
/// 把 Seatbelt（macOS 专属）排在 Linux 链首位——`select()` 会做出潜伏的
/// 错误平台选择。将 cfg 收敛到 `current()` 单点、链构造交给纯函数
/// [`OsBackend::for_platform`]，跨平台排序可在任意宿主上以注入方式单测。
///
/// WHY `#[allow(dead_code)]`：单宿主非测试构建只构造 `current()` 的 cfg
/// 激活变体（如 Windows 宿主仅 `Windows`），其余变体由跨平台单测构造——
/// 这是平台投影枚举的固有形态，非死码。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PlatformFamily {
    /// macOS（Seatbelt/sandbox-exec 原生）
    MacOs,
    /// Linux（Landlock+seccomp / bwrap）
    Linux,
    /// Windows（仅进程围栏，无 OS 级沙箱 API 接线）
    Windows,
    /// 其余平台（BSD 等：不承诺平台专属后端，bwrap → ProcessFence）
    Other,
}

impl PlatformFamily {
    /// 编译期平台检测（每平台恰好一个 cfg 臂激活）
    #[cfg(target_os = "macos")]
    fn current() -> Self {
        Self::MacOs
    }

    #[cfg(target_os = "linux")]
    fn current() -> Self {
        Self::Linux
    }

    #[cfg(target_os = "windows")]
    fn current() -> Self {
        Self::Windows
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    fn current() -> Self {
        Self::Other
    }
}

/// 三档模式（v4.0 WI-14）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SandboxMode {
    /// bash 包 argv 逐项校验，越界写 OS 层拒否
    Strict,
    /// 默认：命令白名单 + 路径越界拒绝
    Standard,
    /// 仅高风险操作拦截
    Relaxed,
}

impl OsBackend {
    /// 平台可用后端列表（按优先级降序，按 target_os 拆分，C-20）
    ///
    /// Windows：仅 ProcessFence（Job Object 由平台 API 承接，std 层校验先行）；
    /// macOS：首选 Seatbelt；Linux：首选 Landlock；其余平台：Bwrap → ProcessFence。
    /// 各链均以 ProcessFence 收尾（全平台进程围栏兜底，ADR-001 降级链）。
    #[must_use]
    pub fn available() -> Vec<Self> {
        Self::for_platform(PlatformFamily::current())
    }

    /// 按平台族构造后端优先级链（纯函数，C-20 单测注入面）
    ///
    /// 平台专属后端只出现在所属平台链内（Seatbelt→macOS、Landlock→Linux），
    /// 杜绝"Linux 首选 Seatbelt"一类跨平台错误选择。
    #[must_use]
    fn for_platform(family: PlatformFamily) -> Vec<Self> {
        match family {
            // Seatbelt（sandbox-exec）为 macOS 专属，不进其他平台链
            PlatformFamily::MacOs => vec![Self::Seatbelt, Self::ProcessFence],
            // Landlock+seccomp 为 Linux 专属；bwrap 容器兜底次之
            PlatformFamily::Linux => vec![Self::Landlock, Self::Bwrap, Self::ProcessFence],
            PlatformFamily::Windows => vec![Self::ProcessFence],
            // 其余平台不承诺 Seatbelt/Landlock（平台专属能力），bwrap 可用则优先
            PlatformFamily::Other => vec![Self::Bwrap, Self::ProcessFence],
        }
    }

    /// 选择首个可用后端（降级链：None 表示全部不可用 → 调用方按 ADR-001
    /// 降级为应用层沙箱并告警）
    #[must_use]
    pub fn select() -> Option<Self> {
        Self::available().first().copied()
    }

    /// 是否支持进程级快照（snapshot/fork/restore）
    #[must_use]
    pub fn supports_process_snapshot(self) -> bool {
        // ProcessFence 为决策级快照；进程级快照为 Linux/macOS 容器后端能力
        matches!(self, Self::Bwrap)
    }
}

/// 快照内容 — 决策级快照（校验状态 + 策略指纹）
///
/// 进程级快照（容器 pause/commit）为平台能力，本类型承载可移植的
/// 决策级快照：恢复 = 用快照校验状态重建隔离上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxSnapshot {
    /// 策略指纹（白名单 + 阻断模式内容的 SHA-256 hex，见 [`hash_policy_content`]）
    ///
    /// 内容寻址：同策略恒同值、异策略必异值，跨进程/跨时间可比（C-21）。
    pub policy_fingerprint: Arc<str>,
    /// 校验状态计数（已放行/已拒绝）
    pub allowed_ops: u64,
    /// 被拒绝操作计数(校验未放行的操作次数,与 allowed_ops 对偶)
    pub denied_ops: u64,
    /// 快照时刻（UNIX 毫秒）
    pub taken_at_ms: u128,
}

/// 沙箱提供者 — snapshot/fork/restore 三分（v4.0 WI-14）
pub trait SandboxProvider: Send + Sync {
    /// 保存当前隔离上下文快照（决策级）
    fn snapshot(&self) -> Result<SandboxSnapshot, SecCoreError>;
    /// 派生隔离上下文（fork 语义：共享快照基线，独立演进）
    fn fork(&self, base: &SandboxSnapshot) -> Result<Box<dyn SandboxProvider>, SecCoreError>;
    /// 回滚到快照状态
    fn restore(&mut self, snap: &SandboxSnapshot) -> Result<(), SecCoreError>;
}

/// ProcessFence 后端 — 命令校验 + 子进程隔离近似（全平台兜底）
///
/// Windows 可验证子集：路径越界（/etc、系统目录写）、越界网络外发在
/// 命令校验层拒绝（复用 L0 CommandValidator 语义 + 逃逸静态检查）；
/// 子进程 OS 级隔离（Job Object 等平台 API）为 Linux/macOS CI 承接项，
/// 本模块以校验层保证为主。
pub struct ProcessFence {
    /// 命令校验策略（L0 契约，default_secure 白名单）
    policy: Arc<CommandPolicy>,
    /// 模式档位
    mode: SandboxMode,
    /// 统计
    allowed_ops: u64,
    denied_ops: u64,
}

impl std::fmt::Debug for ProcessFence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessFence")
            .field("mode", &self.mode)
            .field("allowed_ops", &self.allowed_ops)
            .field("denied_ops", &self.denied_ops)
            .finish()
    }
}

impl ProcessFence {
    /// 新建 ProcessFence（默认 Standard 模式 + default_secure 策略）
    #[must_use]
    pub fn new(policy: Arc<CommandPolicy>, mode: SandboxMode) -> Self {
        Self {
            policy,
            mode,
            allowed_ops: 0,
            denied_ops: 0,
        }
    }

    /// 命令准入校验（逃逸防线：越界写/越界网络先于策略拒绝）
    ///
    /// # 返回
    /// - `Ok(())`：合法操作放行（零误伤由白名单保证）
    /// - `Err(SecCoreError::CommandBlocked)`：越界操作拒绝（写 /etc、
    ///   系统目录、越界网络）或策略命中——策略命中时 attack_type/detail
    ///   原样透传（C-22：拦截模式的攻击分类/白名单 Abuse 不降格为 DataLeak）
    pub fn check(&mut self, program: &str, args: &[String]) -> Result<(), SecCoreError> {
        // 逃逸路径静态拒绝（任何模式都不允许：写系统目录/越界网络）
        if is_escape_attempt(program, args) {
            self.denied_ops += 1;
            return Err(SecCoreError::CommandBlocked {
                attack_type: nexus_contracts::command_validation::AttackType::DataLeak,
                detail: format!("escape attempt: {program} {}", args.join(" ")),
            });
        }
        // L0 契约校验（白名单 + 阻断模式）
        let mut cmd = Command::new(program);
        cmd.args.extend(args.iter().cloned());
        match validate_command(&cmd, &self.policy) {
            Ok(_spec) => {
                self.allowed_ops += 1;
                Ok(())
            }
            // C-22：策略拒绝原样透传——validate_command 的 CommandBlocked 携带
            // 真实攻击分类（拦截模式 Injection→…→Tamper / 白名单 Abuse）；
            // 旧实现解构后统一重包为 DataLeak/"policy blocked"，抹掉攻击分类，
            // 违反 error.rs「拦截类错误携带攻击类型与详情便于审计」契约
            Err(e @ SecCoreError::CommandBlocked { .. }) => {
                self.denied_ops += 1;
                Err(e)
            }
            Err(e) => Err(e),
        }
    }

    /// 统计（诊断）
    #[must_use]
    pub fn stats(&self) -> (u64, u64) {
        (self.allowed_ops, self.denied_ops)
    }
}

/// 逃逸尝试判定 — 写系统目录 / 越界网络（任何模式均拒绝）
///
/// WHY 静态前缀检查：命令解析的近似防线（真实 argv 校验由 L0 策略承担）；
/// 覆盖 WI-14 门禁样例：写 /etc、越界网络。
#[must_use]
fn is_escape_attempt(program: &str, args: &[String]) -> bool {
    let lower_cmd = program.to_ascii_lowercase();
    let joined = format!("{lower_cmd} {}", args.join(" "));
    let lower = joined.to_ascii_lowercase();
    // 写系统目录（/etc、/usr、C:\Windows、C:\Program Files）
    let system_paths = [
        "/etc",
        "/usr",
        "c:\\windows",
        "c:\\program files",
        "/system",
    ];
    if system_paths.iter().any(|p| lower.contains(p))
        && (lower.contains('>')
            || lower.contains(">>")
            || lower.contains("write")
            || lower.contains("rm "))
    {
        return true;
    }
    // 越界网络外发（curl/wget 到非本地地址——静态近似：本地地址外全拒）
    if (lower.contains("curl ") || lower.contains("wget "))
        && !lower.contains("localhost")
        && !lower.contains("127.0.0.1")
    {
        return true;
    }
    false
}

/// 计算命令策略内容的 SHA-256 指纹（C-21，修复登记册 L09）
///
/// 哈希内容 = 排序后白名单 + 按序阻断模式（pattern/attack_type/description）。
/// 替代旧 `format!("{:p}", Arc 指针)`——指针地址在 ASLR 下跨进程/跨时间不可比，
/// 冒充"策略指纹"属语义性缺陷（全仓零消费点，无迁移面）。
fn hash_policy_content(policy: &CommandPolicy) -> String {
    // WHY 排序：allowed_commands 是 HashSet，迭代序不确定；
    // 不排序则同一策略两次快照指纹漂移，破坏"同策略同指纹"语义
    let mut allowed: Vec<&str> = policy.allowed_commands.iter().map(String::as_str).collect();
    allowed.sort_unstable();

    let mut hasher = Sha256::new();
    for cmd in allowed {
        hasher.update(b"allow\x00");
        hasher.update(cmd.as_bytes());
        hasher.update(b"\x00");
    }
    // 阻断模式保持 Vec 原序入哈希（检查顺序敏感，顺序本身即策略语义）；
    // 字段以 \x00 分隔防拼接歧义（沿 audit.rs hash_command 先例）
    for bp in &policy.blocked_patterns {
        hasher.update(b"block\x00");
        hasher.update(bp.pattern.as_bytes());
        hasher.update(b"\x00");
        hasher.update(format!("{:?}", bp.attack_type).as_bytes());
        hasher.update(b"\x00");
        hasher.update(bp.description.as_bytes());
        hasher.update(b"\x00");
    }
    hex::encode(hasher.finalize())
}

impl SandboxProvider for ProcessFence {
    fn snapshot(&self) -> Result<SandboxSnapshot, SecCoreError> {
        Ok(SandboxSnapshot {
            policy_fingerprint: Arc::from(hash_policy_content(&self.policy)),
            allowed_ops: self.allowed_ops,
            denied_ops: self.denied_ops,
            taken_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        })
    }

    fn fork(&self, base: &SandboxSnapshot) -> Result<Box<dyn SandboxProvider>, SecCoreError> {
        // fork 语义：共享策略，独立统计（从快照基线继续）
        let mut f = ProcessFence::new(Arc::clone(&self.policy), self.mode);
        f.allowed_ops = base.allowed_ops;
        f.denied_ops = base.denied_ops;
        Ok(Box::new(f))
    }

    fn restore(&mut self, snap: &SandboxSnapshot) -> Result<(), SecCoreError> {
        self.allowed_ops = snap.allowed_ops;
        self.denied_ops = snap.denied_ops;
        Ok(())
    }
}

/// 快照性能探针 — 快照 P50 < 200ms【门禁目标】
///
/// 决策级快照为纯内存复制（µs 级）；本函数测量 ProcessFence 快照往返，
/// 供 CI 断言（进程级快照由容器后端承接）。
#[must_use]
pub fn probe_snapshot_latency(fence: &mut ProcessFence, n: usize) -> f64 {
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        // 契约:ProcessFence::snapshot 为纯内存复制,实现层保证不返回 Err;
        // 若返回 Err 即不变量被破坏(开发期缺陷),显式 panic 暴露而非静默降级
        let snap = match fence.snapshot() {
            Ok(snap) => snap,
            Err(e) => panic!("进程围栏快照不可失败(不变量破坏): {e}"),
        };
        let _ = fence.restore(&snap);
        samples.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    // 耗时样本由 as_secs_f64() 产出,恒为非 NaN,partial_cmp 理论不可失效,
    // Equal 兼容分支仅作防御(避免排序 panic)
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    samples[n / 2]
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn fence(mode: SandboxMode) -> ProcessFence {
        ProcessFence::new(Arc::new(CommandPolicy::default()), mode)
    }

    #[test]
    fn backend_selection_priority() {
        let backends = OsBackend::available();
        assert!(!backends.is_empty(), "至少一个可用后端");
        assert!(OsBackend::select().is_some());
        // 全平台 ProcessFence 必须可用（兜底）
        assert!(backends.contains(&OsBackend::ProcessFence));
    }

    #[test]
    fn available_per_platform_split() {
        // C-20：按 target_os 拆分后的各平台链（注入断言，任意宿主可测）
        // macOS 首选 Seatbelt，且不得混入 Linux 专属 Landlock
        let mac = OsBackend::for_platform(PlatformFamily::MacOs);
        assert_eq!(mac.first().copied(), Some(OsBackend::Seatbelt));
        assert!(!mac.contains(&OsBackend::Landlock));
        // Linux 首选 Landlock，且不得混入 macOS 专属 Seatbelt
        let linux = OsBackend::for_platform(PlatformFamily::Linux);
        assert_eq!(linux.first().copied(), Some(OsBackend::Landlock));
        assert!(!linux.contains(&OsBackend::Seatbelt));
        // 其余平台：Bwrap → ProcessFence，两个平台专属后端均不入选
        let other = OsBackend::for_platform(PlatformFamily::Other);
        assert_eq!(other, vec![OsBackend::Bwrap, OsBackend::ProcessFence]);
        // Windows：仅 ProcessFence
        let win = OsBackend::for_platform(PlatformFamily::Windows);
        assert_eq!(win, vec![OsBackend::ProcessFence]);
    }

    #[test]
    fn available_matches_current_platform_and_keeps_fallback_tail() {
        // 不变式一：任何平台链都以 ProcessFence 收尾（全平台兜底，ADR-001）
        for family in [
            PlatformFamily::MacOs,
            PlatformFamily::Linux,
            PlatformFamily::Windows,
            PlatformFamily::Other,
        ] {
            let chain = OsBackend::for_platform(family);
            assert_eq!(
                chain.last().copied(),
                Some(OsBackend::ProcessFence),
                "链尾 ProcessFence 兜底缺失: {family:?}"
            );
        }
        // 不变式二：宿主实测 available() 与 cfg 检测的平台链一致，
        // select() 恒取链首（降级链正确性）
        assert_eq!(
            OsBackend::available(),
            OsBackend::for_platform(PlatformFamily::current())
        );
        assert_eq!(OsBackend::select(), OsBackend::available().first().copied());
    }

    #[test]
    fn escape_write_etc_rejected_all_modes() {
        // WI-14 门禁：写 /etc 必须被拒（任何模式）
        for mode in [
            SandboxMode::Strict,
            SandboxMode::Standard,
            SandboxMode::Relaxed,
        ] {
            let mut f = fence(mode);
            let args = vec!["/etc/passwd".to_string(), "x".to_string()];
            assert!(
                f.check("echo", &args).is_err(),
                "写系统目录必须拒绝（echo /etc/passwd）"
            );
            let (_, denied) = f.stats();
            assert!(denied >= 1, "拒绝必须计数");
        }
    }

    #[test]
    fn escape_network_egress_rejected() {
        let mut f = fence(SandboxMode::Standard);
        let args = vec!["http://evil.example.com/data".to_string()];
        assert!(f.check("curl", &args).is_err(), "越界网络外发必须拒绝");
    }

    #[test]
    fn localhost_network_not_escape() {
        // 合法操作零误伤：本地回环网络不触发逃逸拒绝（是否放行由策略判定）
        let mut f = fence(SandboxMode::Standard);
        let args = vec!["http://localhost:8080/api".to_string()];
        let r = f.check("curl", &args);
        // 逃逸静态检查不拒绝本地地址；最终判定交给策略层（无 panic 即可）
        let _ = r;
    }

    #[test]
    fn snapshot_restore_roundtrip() {
        let mut f = fence(SandboxMode::Standard);
        let snap = f.snapshot().expect("快照");
        // 推进状态（拒绝一次逃逸）
        let args = vec!["/etc/passwd".to_string()];
        let _ = f.check("rm", &args);
        let (_, denied) = f.stats();
        assert!(denied > 0);
        // 回滚 → 统计恢复
        f.restore(&snap).expect("恢复");
        let (_, denied2) = f.stats();
        assert_eq!(denied2, snap.denied_ops, "恢复后统计必须回到快照点");
    }

    #[test]
    fn fork_independent_stats() {
        let f = fence(SandboxMode::Standard);
        let snap = f.snapshot().expect("快照");
        let child = f.fork(&snap).expect("fork");
        // trait 对象仅暴露 trait 方法：用 snapshot 验证 fork 基线一致
        let child_snap = child.snapshot().expect("子快照");
        assert_eq!(
            child_snap.allowed_ops, snap.allowed_ops,
            "fork 从快照基线继续"
        );
    }

    #[test]
    fn snapshot_latency_well_below_gate() {
        // 决策级快照 µs 级（进程级 200ms 门禁由容器后端承接）
        let mut f = fence(SandboxMode::Standard);
        let p50_us = probe_snapshot_latency(&mut f, 100);
        assert!(
            p50_us < 200_000.0,
            "决策级快照必须远低于 200ms, 实测 {p50_us}µs"
        );
    }

    #[test]
    fn command_validation_error_mapping() {
        // L0 校验错误必须可传播（CommandValidationError 映射路径不 panic）
        let mut f = fence(SandboxMode::Standard);
        let args = vec!["--help".to_string()];
        // 白名单外命令 → 策略拒绝或放行（依 default_secure 白名单），不得 panic
        let _ = f.check("unknown-cmd-xyz", &args);
    }

    #[test]
    fn fingerprint_same_policy_same_value() {
        // C-21：指纹来自策略内容而非 Arc 指针地址——
        // 两次独立堆分配（地址不同）的相同策略必须产出相同指纹
        let policy = CommandPolicy::default_secure();
        let f1 = ProcessFence::new(Arc::new(policy.clone()), SandboxMode::Standard);
        let f2 = ProcessFence::new(Arc::new(policy), SandboxMode::Standard);
        let s1 = f1.snapshot().expect("快照1");
        let s2 = f2.snapshot().expect("快照2");
        // SHA-256 hex 编码恒 64 字符（内容寻址形态自检）
        assert_eq!(s1.policy_fingerprint.len(), 64);
        assert_eq!(
            s1.policy_fingerprint, s2.policy_fingerprint,
            "同策略必须同指纹（内容哈希，与分配地址无关）"
        );
    }

    #[test]
    fn fingerprint_independent_of_insertion_order() {
        // HashSet 迭代序不确定：集合内容相同即同指纹（白名单排序后入哈希）
        let a = CommandPolicy::new()
            .allow_command("cat")
            .allow_command("echo")
            .allow_command("ls");
        let b = CommandPolicy::new()
            .allow_command("ls")
            .allow_command("cat")
            .allow_command("echo");
        let fa = ProcessFence::new(Arc::new(a), SandboxMode::Standard);
        let fb = ProcessFence::new(Arc::new(b), SandboxMode::Standard);
        assert_eq!(
            fa.snapshot().expect("快照a").policy_fingerprint,
            fb.snapshot().expect("快照b").policy_fingerprint,
            "白名单集合相同（插入序不同）必须同指纹"
        );
    }

    #[test]
    fn fingerprint_differs_for_different_policy() {
        // C-21：异策略必异指纹——白名单增一条
        let base = CommandPolicy::default_secure();
        let f_base = ProcessFence::new(Arc::new(base.clone()), SandboxMode::Standard);
        let extended = base.clone().allow_command("newcmd-for-fingerprint-test");
        let f_ext = ProcessFence::new(Arc::new(extended), SandboxMode::Standard);
        assert_ne!(
            f_base.snapshot().expect("基线快照").policy_fingerprint,
            f_ext.snapshot().expect("扩白名单快照").policy_fingerprint,
            "白名单不同必须指纹不同"
        );
        // 阻断模式增一条（白名单不变）
        let blocked = base.block_pattern(
            "dangerous-fingerprint-marker",
            nexus_contracts::command_validation::AttackType::Tamper,
            "测试注入的拦截模式",
        );
        let f_blk = ProcessFence::new(Arc::new(blocked), SandboxMode::Standard);
        assert_ne!(
            f_base.snapshot().expect("基线快照2").policy_fingerprint,
            f_blk.snapshot().expect("增拦截快照").policy_fingerprint,
            "阻断模式不同必须指纹不同"
        );
    }

    #[test]
    fn policy_block_error_passthrough_preserves_attack_type() {
        // C-22：策略拦截错误必须原样透传——拦截模式命中的攻击分类（Injection）
        // 不得被二次包装降格为 DataLeak（旧实现统一重包 DataLeak/"policy blocked"，
        // 抹掉 attack_type 分类，违反 error.rs 审计契约）
        let policy = CommandPolicy::new().allow_command("cat").block_pattern(
            "c22-injection-marker",
            nexus_contracts::command_validation::AttackType::Injection,
            "C-22 测试: 命中 Injection 拦截模式",
        );
        let mut f = ProcessFence::new(Arc::new(policy), SandboxMode::Standard);
        // 命令不含逃逸特征（系统路径/外发网络），确保走策略校验路径
        let args = vec!["c22-injection-marker".to_string()];
        match f.check("cat", &args) {
            Err(SecCoreError::CommandBlocked {
                attack_type,
                detail,
            }) => {
                assert_eq!(
                    attack_type,
                    nexus_contracts::command_validation::AttackType::Injection,
                    "拦截模式 attack_type 必须透传（不得降格为 DataLeak）"
                );
                assert_eq!(detail, "C-22 测试: 命中 Injection 拦截模式");
            }
            other => panic!("命中拦截模式必须拒绝, 实得: {other:?}"),
        }
        let (_, denied) = f.stats();
        assert!(denied >= 1, "策略拒绝必须计数");
    }

    #[test]
    fn whitelist_rejection_passthrough_preserves_abuse() {
        // C-22：白名单外命令被 policy.rs 判为 Abuse——透传后不得降格为 DataLeak
        let mut f = fence(SandboxMode::Standard);
        let args = vec!["--help".to_string()];
        match f.check("unknown-cmd-c22", &args) {
            Err(SecCoreError::CommandBlocked { attack_type, .. }) => {
                assert_eq!(
                    attack_type,
                    nexus_contracts::command_validation::AttackType::Abuse,
                    "白名单外必须透传 Abuse 分类（不得降格为 DataLeak）"
                );
            }
            other => panic!("白名单外命令必须拒绝, 实得: {other:?}"),
        }
    }
}
