//! 缝合点 — Clock / Rng / Fs / Net 四个可注入接口（手册 §10.8,Ω₇ 可测试性的根基）
//!
//! 对应架构层:L1 Core
//!
//! # 设计动机（Ω₇ 测试性）
//! 业务逻辑**禁止直调** `SystemTime::now()` / `thread_rng()` / `std::fs` / 网络（§11.6 禁令）,
//! 否则单测无法注入确定性时钟与种子。经本模块 trait 注入后:
//! - 生产路径: `SystemClock` / `ChaCha8Rng` / `OsFs`(占位) / 网络后端(骨架不实现)
//! - 测试路径: `FixedClock` / `SeedableRng(42)` / `MemFs` / `MockNet`
//!
//! # 本任务边界（T8）
//! 只建 trait 与测试实现,**不要求任何生产代码改用它**;
//! T14(WI-34 并行化注入)时才接线本模块到 ComputeBridge 与各热点。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use rand::rngs::StdRng;
// WHY RngCore 而非 Rng: next_u64 的实际提供 trait 为 RngCore(rand 0.8 中
// Rng 仅委托),且本模块自有 trait 名恰为 Rng,避免同名导入冲突
use rand::{RngCore, SeedableRng};
use thiserror::Error;

/// 缝合点错误 — 最小抽象,占位后端与测试后端共用
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SeamError {
    /// 后端在骨架阶段未实现（如 `OsFs` 生产后端,T14 接线）
    #[error("seam backend not implemented: {0}")]
    Unsupported(&'static str),
}

/// 时间缝合点 — 生产 `SystemClock`,测试 `FixedClock`
pub trait Clock: Send + Sync {
    /// 当前时间 — 骨架阶段返回单调时钟（T14 后升级为可配置时间源）
    fn now(&self) -> Instant;
}

/// 随机数缝合点 — 生产 `ChaCha8Rng`,测试 `SeedableRng(42)`
///
/// WHY `&self` 签名:缝合点需 `Send + Sync`（跨线程注入）,
/// 实现侧用内部可变性（`Mutex<StdRng>`）承载状态。
pub trait Rng: Send + Sync {
    /// 下一 u64 — 与 rand 的 `Rng::next_u64` 同名不同 trait,注意导入区分
    fn next_u64(&self) -> u64;
}

/// 文件系统缝合点 — 生产 `OsFs`（骨架占位）,测试 `MemFs`
///
/// 最小抽象:read / write / list 三个操作,足够覆盖存储类热点的可测试需求;
/// 生产后端（真实磁盘 IO）在 T14 接线。
pub trait Fs: Send + Sync {
    /// 读取文件全部字节
    fn read(&self, path: &Path) -> Result<Vec<u8>, SeamError>;
    /// 写入文件全部字节（覆盖语义）
    fn write(&self, path: &Path, data: &[u8]) -> Result<(), SeamError>;
    /// 列目录
    fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, SeamError>;
}

/// 网络缝合点 — 生产后端骨架不实现（任务边界）,测试 `MockNet`
///
/// 最小抽象:HTTP GET / POST;生产后端（reqwest 等）在 T14 接线,
/// 且 **LLM 调用类任务禁止进 rayon 池**（红线,§7.5.3 纪律⑥）。
pub trait Net: Send + Sync {
    /// HTTP GET — 返回响应体字节
    fn get(&self, url: &str) -> Result<Vec<u8>, SeamError>;
    /// HTTP POST — 返回响应体字节
    fn post(&self, url: &str, body: &[u8]) -> Result<Vec<u8>, SeamError>;
}

/// 生产时钟 — `Instant::now()`（单调,不受系统时间回拨影响,§14.2 场景 9）
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// 生产随机源 — 命名对齐手册 §10.8（`ChaCha8Rng`）;内部为 `rand::rngs::StdRng`
///
/// WHY StdRng 而非 rand_chacha:`rand_chacha` 未在 workspace 声明（Ω₆ 最小依赖）,
/// 且 `rand` 0.8 自带 `StdRng`（ChaCha12 流密码,`SeedableRng` 确定性语义同 ChaCha8:
/// 同种子必同序列,满足 Ω₂）。如需精确 ChaCha8 算法,T9 引入 rand_chacha 后
/// 仅替换内部实现,接口不变。此缝合点用途:确定性随机注入（Ω₇ + Ω₂）。
pub struct ChaCha8Rng {
    /// 内部状态 — Mutex 承载 `&self` + Sync 语义（缝合点跨线程注入）
    inner: Mutex<StdRng>,
}

impl ChaCha8Rng {
    /// 以显式种子构造（Ω₂ 确定性:相同输入相同输出）
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            inner: Mutex::new(StdRng::seed_from_u64(seed)),
        }
    }
}

impl Rng for ChaCha8Rng {
    fn next_u64(&self) -> u64 {
        // 单线程注入场景锁竞争可忽略;unwrap_or_else 防御 poison(本模块无 panic 路径)
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .next_u64()
    }
}

/// 生产文件系统 — 骨架占位,返回 [`SeamError::Unsupported`]
///
/// WHY 占位:本任务只建 trait 与测试实现,真实磁盘 IO 后端在 T14 接线;
/// 占位语义显式化,防止误用未实现后端产生"看似成功"的假读写。
pub struct OsFs;

impl Fs for OsFs {
    fn read(&self, _path: &Path) -> Result<Vec<u8>, SeamError> {
        Err(SeamError::Unsupported("OsFs"))
    }

    fn write(&self, _path: &Path, _data: &[u8]) -> Result<(), SeamError> {
        Err(SeamError::Unsupported("OsFs"))
    }

    fn list(&self, _dir: &Path) -> Result<Vec<PathBuf>, SeamError> {
        Err(SeamError::Unsupported("OsFs"))
    }
}

/// 测试时钟 — 返回固定 `Instant`（构造时快照）,确定性测试注入
pub struct FixedClock {
    /// 固定时间点
    fixed: Instant,
}

impl FixedClock {
    /// 以当前时刻快照构造 — 单测内两次读取恒等
    #[must_use]
    pub fn new() -> Self {
        Self {
            fixed: Instant::now(),
        }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Instant {
        self.fixed
    }
}

impl Default for FixedClock {
    fn default() -> Self {
        Self::new()
    }
}

/// 测试随机源 — 固定种子 42（手册 §10.8）;同种子必同序列（Ω₂）
pub struct SeedRng42 {
    /// 内部 ChaCha8Rng 实现,种子 42
    inner: ChaCha8Rng,
}

impl SeedRng42 {
    /// 构造 — 种子固定 42,测试可复现
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: ChaCha8Rng::new(42),
        }
    }
}

impl Default for SeedRng42 {
    fn default() -> Self {
        Self::new()
    }
}

impl Rng for SeedRng42 {
    fn next_u64(&self) -> u64 {
        self.inner.next_u64()
    }
}

/// 测试文件系统 — 内存 HashMap 后端（确定性,无真实磁盘）
pub struct MemFs {
    /// 路径 → 内容;`Mutex` 承载 `&self` + Sync
    map: Mutex<std::collections::HashMap<PathBuf, Vec<u8>>>,
}

impl MemFs {
    /// 构造空内存文件系统
    #[must_use]
    pub fn new() -> Self {
        Self {
            map: Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for MemFs {
    fn default() -> Self {
        Self::new()
    }
}

impl Fs for MemFs {
    fn read(&self, path: &Path) -> Result<Vec<u8>, SeamError> {
        let map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        map.get(path)
            .cloned()
            .ok_or(SeamError::Unsupported("MemFs:path-not-found"))
    }

    fn write(&self, path: &Path, data: &[u8]) -> Result<(), SeamError> {
        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(path.to_path_buf(), data.to_vec());
        Ok(())
    }

    fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, SeamError> {
        let map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        Ok(map.keys().filter(|p| p.starts_with(dir)).cloned().collect())
    }
}

/// 测试网络 — 固定响应（URL → 响应体）;用于不变量测试的确定性注入
pub struct MockNet {
    /// URL → 响应体
    responses: Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

impl MockNet {
    /// 构造空模拟网络
    #[must_use]
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// 注册固定响应 — URL 命中时返回该字节序列
    pub fn stub_get(&self, url: &str, body: Vec<u8>) {
        self.responses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(url.to_owned(), body);
    }
}

impl Net for MockNet {
    fn get(&self, url: &str) -> Result<Vec<u8>, SeamError> {
        let responses = self.responses.lock().unwrap_or_else(|p| p.into_inner());
        responses
            .get(url)
            .cloned()
            .ok_or(SeamError::Unsupported("MockNet:no-stub"))
    }

    fn post(&self, _url: &str, _body: &[u8]) -> Result<Vec<u8>, SeamError> {
        // 骨架测试后端只模拟 GET;POST 语义 T14 接线
        Err(SeamError::Unsupported("MockNet:post"))
    }
}

impl Default for MockNet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FixedClock — 两次读取返回同一固定值
    #[test]
    fn fixed_clock_returns_constant() {
        let clock = FixedClock::new();
        let t1 = clock.now();
        let t2 = clock.now();
        assert_eq!(t1, t2, "FixedClock 必须返回固定时刻");
        // 与 SystemClock 的推进性对照:固定时钟不随真实时间推进
        let real = SystemClock.now();
        let real_later = SystemClock.now();
        assert!(real_later >= real, "SystemClock 单调不减");
    }

    /// SeedRng42 — 同种子同序列（Ω₂ 确定性）
    #[test]
    fn seed_rng_deterministic() {
        let a = SeedRng42::new();
        let b = SeedRng42::new();
        for _ in 0..8 {
            assert_eq!(a.next_u64(), b.next_u64(), "同种子必须同序列");
        }
    }

    /// ChaCha8Rng — 同种子同序列;异种子序列不同
    #[test]
    fn chacha8_rng_seeded_determinism() {
        let a = ChaCha8Rng::new(7);
        let b = ChaCha8Rng::new(7);
        let c = ChaCha8Rng::new(8);
        let (mut seq_a, mut seq_b, mut seq_c) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..8 {
            seq_a.push(a.next_u64());
            seq_b.push(b.next_u64());
            seq_c.push(c.next_u64());
        }
        assert_eq!(seq_a, seq_b, "同种子同序列");
        assert_ne!(seq_a, seq_c, "异种子序列应不同");
    }

    /// OsFs — 占位后端恒返回 Unsupported（防止假读写）
    #[test]
    fn os_fs_placeholder_unsupported() {
        let fs = OsFs;
        let path = Path::new("/tmp/x");
        assert_eq!(fs.read(path), Err(SeamError::Unsupported("OsFs")));
        assert_eq!(fs.write(path, b"x"), Err(SeamError::Unsupported("OsFs")));
        assert_eq!(
            fs.list(Path::new("/tmp")),
            Err(SeamError::Unsupported("OsFs"))
        );
    }

    /// MemFs — 写后读一致,list 前缀过滤
    #[test]
    fn mem_fs_roundtrip_and_list() {
        let fs = MemFs::new();
        fs.write(Path::new("/a/b.txt"), b"hello")
            .expect("MemFs 写入应成功");
        fs.write(Path::new("/a/c.txt"), b"world")
            .expect("MemFs 写入应成功");
        assert_eq!(
            fs.read(Path::new("/a/b.txt")).expect("读取应成功"),
            b"hello"
        );
        let listed = fs.list(Path::new("/a")).expect("list 应成功");
        assert_eq!(listed.len(), 2);
        assert!(listed.contains(&PathBuf::from("/a/b.txt")));
        assert!(listed.contains(&PathBuf::from("/a/c.txt")));
    }

    /// MockNet — stub 命中返回固定响应;未注册 URL 报错
    #[test]
    fn mock_net_stub_hit_and_miss() {
        let net = MockNet::new();
        net.stub_get("http://test.local/a", vec![1, 2, 3]);
        assert_eq!(
            net.get("http://test.local/a").expect("stub 应命中"),
            vec![1, 2, 3]
        );
        assert_eq!(
            net.get("http://test.local/miss"),
            Err(SeamError::Unsupported("MockNet:no-stub"))
        );
    }

    /// SeamError Display — 文案含后端名
    #[test]
    fn seam_error_display() {
        assert!(SeamError::Unsupported("OsFs").to_string().contains("OsFs"));
    }
}
