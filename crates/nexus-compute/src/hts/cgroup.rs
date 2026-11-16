//! HTS-CPU cgroup 核数校正（第三重阈值来源）
//!
//! cgroup 核数校正 — ADR-103 三重来源③（容器内 `available_parallelism` 不可信的修正）
//!
//! # 背景
//! 容器内 `std::thread::available_parallelism()` 可能返回宿主机核数而非 cgroup 配额,
//! 导致线程数/并行度配置虚高（手册 §8.4 / ADR-103）。本模块读取 cgroup v2 的
//! `cpu.max`（格式 `"quota period"`,如 `"80000 100000"` → 0.8 核）,
//! 与物理核数取小得有效核数。
//!
//! # 边界
//! - 解析失败 / 非 Linux → `None`（无 cgroup 校正信息,调用方回退物理核数）;
//! - `"max 100000"`（未限制）→ `None`;
//! - 测试经 [`CgroupProbe`] 注入临时文件路径（Ω₇ 可测试性）。
//!
// WHY 独立成文件（ADR-314 W7 / BC-04）：cgroup 探针是平台校正，与统计判定
// 正交；`#[cfg(target_os = "linux")]` 分支留在本文件内（平台差异就地可读）。
// `hts::cgroup::*` 的公开路径逐字不变（子模块仍为 public）。

use std::path::{Path, PathBuf};

/// cgroup 探测点 — 路径可配置（测试注入临时文件,生产默认 `/sys/fs/cgroup/cpu.max`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgroupProbe {
    /// `cpu.max` 文件路径
    pub cpu_max_path: PathBuf,
}

impl CgroupProbe {
    /// 以显式路径构造 — 测试注入临时文件用
    #[must_use]
    pub fn new(cpu_max_path: PathBuf) -> Self {
        Self { cpu_max_path }
    }

    /// 探测 cgroup 配额核数 — 读文件 + 解析
    ///
    /// 读取失败（文件缺失/无权限）与解析失败均返回 `None`;
    /// 文件内容格式:第一行 `"quota period"`。
    #[must_use]
    pub fn probe(&self) -> Option<f64> {
        let content = std::fs::read_to_string(&self.cpu_max_path).ok()?;
        parse_cpu_max(&content)
    }
}

impl Default for CgroupProbe {
    /// 生产默认路径 — cgroup v2 配额文件
    fn default() -> Self {
        Self {
            cpu_max_path: PathBuf::from("/sys/fs/cgroup/cpu.max"),
        }
    }
}

/// 解析 `cpu.max` 内容 — 纯函数,平台无关,可单测
///
/// 格式:`"<quota> <period>"`,如 `"80000 100000"` → `Some(0.8)`。
/// - `quota = "max"`（未限制）→ `None`;
/// - 字段缺失 / 多余字段 / 非数值 / 非正数 / 非有限 → `None`。
#[must_use]
pub fn parse_cpu_max(content: &str) -> Option<f64> {
    let mut parts = content.split_whitespace();
    let quota = parts.next()?;
    let period = parts.next()?;
    // 多余字段视为格式非法（严格解析,防误读截断）
    if parts.next().is_some() {
        return None;
    }
    // cgroup v2 语义:`max` 表示未限制配额,无 cgroup 校正信息
    if quota == "max" {
        return None;
    }
    let q: f64 = quota.parse().ok()?;
    let p: f64 = period.parse().ok()?;
    if !q.is_finite() || !p.is_finite() || q <= 0.0 || p <= 0.0 {
        return None;
    }
    Some(q / p)
}

/// 有效核数 — cgroup 配额核数 vs 物理核数取小（ADR-103 三重来源③）
///
/// 无 cgroup 限制（`probe` 返回 `None`）或无法探测物理核数时返回 `None`,
/// 由调用方回退 `available_parallelism`（本函数只在有 cgroup 校正信息时给出修正值）。
#[cfg(target_os = "linux")]
#[must_use]
pub fn effective_cores() -> Option<f64> {
    let quota = CgroupProbe::default().probe()?;
    let phys = std::thread::available_parallelism().ok()?.get() as f64;
    if phys <= 0.0 {
        return None;
    }
    Some(quota.min(phys))
}

/// 非 Linux 平台占位 — 无 cgroup 校正,恒 `None`
#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn effective_cores() -> Option<f64> {
    None
}

/// 路径借用辅助 — 供 `probe` 的文档示例引用（占位无实际用途）
#[allow(dead_code)]
fn _path_ref(_p: &Path) {}
