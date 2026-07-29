//! AutoBuilder 骨架 — 双智能体环境构建(polish-v2.7 closure Stage B-7)
//!
//! 对应架构层: L7 Execution(pvl-layer producer 扩展)
//! 对应 ADR: ADR-049 决策 1(auto-builder 降级档:pvl-layer producer 扩展,
//! VerifyAgent 骨架复用沙箱能力)
//! 对应设计源: `chimera_ultimate_polish_v2.7.md` §8.2(快手 KAT "83.5% 仓库无法运行")
//!
//! # 降级映射(ADR-049)
//!
//! | 方案原设计 | 骨架降级实现 |
//! |---|---|
//! | BuildAgent(LLM 解析仓库生成脚本) | 规则式脚本生成(按 manifest 类型映射构建命令) |
//! | VerifyAgent(沙箱运行测试) | `SandboxExec` trait 抽象——**不直接依赖 seccore**,由调用方注入沙箱实现 |
//! | 迭代修复循环 | 保留(build → verify → fix,`max_iterations` 上限) |
//! | 真实执行检查 | 复用 `process_score::check_real_execution`(>10ms + coverage>0) |
//!
//! # WHY trait 抽象而非直接依赖 seccore
//!
//! pvl-layer(L7)→ seccore(L4)向下依赖虽合法,但骨架期默认不接线
//! (closure 计划 Stage B-7 "仅导出类型 + 单测"),trait 注入使沙箱实现
//! 延迟到真正接线时由上层编排器提供(seccore::Sandbox 适配层),
//! 骨架自身零新增依赖边、可独立测试(MockSandboxExec)。
//!
//! # 使用示例
//!
//! ```
//! use pvl_layer::auto_builder::{
//!     AutoBuilder, BuildScript, ExecReport, ManifestKind, RepoLayout, SandboxExec,
//! };
//!
//! // 调用方注入的沙箱执行器(真实场景为 seccore Sandbox 适配层)
//! struct AlwaysPass;
//! impl SandboxExec for AlwaysPass {
//!     fn execute(&self, _script: &BuildScript) -> ExecReport {
//!         ExecReport { pass_rate: 1.0, execution_time_ms: 120, coverage: 0.8, errors: vec![] }
//!     }
//! }
//!
//! let builder = AutoBuilder::new(AlwaysPass, 3);
//! let layout = RepoLayout { manifest: ManifestKind::CargoToml, has_lockfile: true };
//! let result = builder.build(&layout);
//! assert!(result.success);
//! assert_eq!(result.iterations, 0); // 首次即通过,无修复迭代
//! ```

use crate::process_score::check_real_execution;
use serde::{Deserialize, Serialize};

// ============================================================
// 常量定义
// ============================================================

/// 验证通过的最低测试通过率(方案 §8.2 success_rate >= 0.9)
pub const VERIFY_PASS_RATE_THRESHOLD: f32 = 0.9;

/// 默认最大修复迭代次数
///
/// WHY 3: 规则式修复的收益边际递减极快(每轮只能追加环境铺垫步骤),
/// 3 轮未通过即返回失败报告交由上层(人工/Hint-Recovery)处置。
pub const DEFAULT_MAX_ITERATIONS: u32 = 3;

// ============================================================
// 仓库布局与构建脚本类型
// ============================================================

/// 仓库 manifest 类型 — BuildAgent 规则式脚本生成的输入
///
/// WHY 枚举而非探测逻辑: 骨架期不做文件系统扫描,manifest 类型由
/// 调用方探测后传入(职责分离,骨架保持纯函数可测)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManifestKind {
    /// Rust workspace / crate(Cargo.toml)
    CargoToml,
    /// Node.js 项目(package.json)
    PackageJson,
    /// Python 项目(pyproject.toml / requirements.txt)
    PyProject,
    /// 未识别 manifest — 生成保守的探测性脚本
    Unknown,
}

/// 仓库布局摘要 — BuildAgent 的输入
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoLayout {
    /// manifest 类型
    pub manifest: ManifestKind,
    /// 是否存在依赖锁文件(Cargo.lock / package-lock.json 等)
    ///
    /// 有锁文件时脚本使用锁定安装(可复现构建),无锁文件时降级为普通安装
    pub has_lockfile: bool,
}

/// 构建脚本 — 有序命令步骤序列
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildScript {
    /// 有序构建步骤(每步一条 shell 命令)
    pub steps: Vec<String>,
    /// 修复迭代中追加的环境铺垫提示(用于诊断与审计)
    pub fix_notes: Vec<String>,
}

/// 构建失败记录 — VerifyAgent 产出,BuildAgent 修复的输入
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildFailure {
    /// 失败步骤索引(指向 `BuildScript::steps`)
    pub step_index: usize,
    /// 失败信息摘要
    pub message: String,
}

// ============================================================
// BuildAgent — 规则式脚本生成与修复
// ============================================================

/// BuildAgent — 解析仓库布局生成构建脚本(方案 §8.2 的规则式降级)
///
/// 无状态纯函数集合:manifest 类型 → 构建命令的静态映射,
/// 修复 = 在失败步骤前插入环境铺垫步骤(规则表驱动)。
#[derive(Debug, Default, Clone, Copy)]
pub struct BuildAgent;

impl BuildAgent {
    /// 创建 BuildAgent
    pub fn new() -> Self {
        Self
    }

    /// 按仓库布局生成初始构建脚本(规则式映射)
    pub fn initial_script(&self, layout: &RepoLayout) -> BuildScript {
        let steps = match layout.manifest {
            ManifestKind::CargoToml => vec![
                "cargo fetch".to_string(),
                "cargo build".to_string(),
                "cargo test --no-fail-fast".to_string(),
            ],
            ManifestKind::PackageJson => {
                // 有锁文件用 ci(可复现),无锁文件降级 install
                let install = if layout.has_lockfile {
                    "npm ci"
                } else {
                    "npm install"
                };
                vec![install.to_string(), "npm test".to_string()]
            }
            ManifestKind::PyProject => vec!["pip install -e .".to_string(), "pytest".to_string()],
            ManifestKind::Unknown => vec![
                // 未识别 manifest:保守探测,只列目录不执行构建
                "ls".to_string(),
            ],
        };
        BuildScript {
            steps,
            fix_notes: Vec::new(),
        }
    }

    /// 基于失败记录修复脚本(规则式:失败步骤前插入环境铺垫)
    ///
    /// 骨架期修复规则表(按失败信息关键词匹配):
    /// - 依赖缺失类("not found" / "missing")→ 插入依赖刷新步骤
    /// - 其余 → 记录 fix note 供上层诊断,脚本不变
    pub fn fix(&self, script: &BuildScript, failures: &[BuildFailure]) -> BuildScript {
        let mut fixed = script.clone();
        for failure in failures {
            let msg_lower = failure.message.to_lowercase();
            if msg_lower.contains("not found") || msg_lower.contains("missing") {
                // 依赖缺失:在失败步骤前插入依赖刷新(骨架期通用铺垫)
                let insert_at = failure.step_index.min(fixed.steps.len());
                fixed
                    .steps
                    .insert(insert_at, "cargo fetch --locked".to_string());
                fixed.fix_notes.push(format!(
                    "step {}: dependency missing, inserted fetch before it",
                    failure.step_index
                ));
            } else {
                fixed.fix_notes.push(format!(
                    "step {}: unrecognized failure '{}', manual review suggested",
                    failure.step_index, failure.message
                ));
            }
        }
        fixed
    }
}

// ============================================================
// SandboxExec trait — 沙箱执行器抽象(调用方注入)
// ============================================================

/// 沙箱执行报告 — VerifyAgent 验证判定的输入
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecReport {
    /// 测试通过率 ∈ [0, 1]
    pub pass_rate: f32,
    /// 执行耗时(毫秒)— 真实执行检查用(>10ms)
    pub execution_time_ms: u64,
    /// 代码覆盖率 ∈ [0, 1] — 真实执行检查用(>0)
    pub coverage: f32,
    /// 执行失败记录
    pub errors: Vec<BuildFailure>,
}

/// 沙箱执行器抽象 — 由调用方注入具体实现
///
/// WHY trait: 骨架期不依赖 seccore,真实接线时上层编排器提供
/// seccore::Sandbox 适配层实现;测试用 mock 实现(见模块测试)。
///
/// WHY 同步签名: 骨架为纯逻辑验证,异步执行由适配层内部处理
/// (适配层可 spawn_blocking 包装,接口保持简单)。
pub trait SandboxExec {
    /// 在沙箱中执行构建脚本并返回执行报告
    fn execute(&self, script: &BuildScript) -> ExecReport;
}

// ============================================================
// VerifyAgent — 沙箱验证与真实执行检查
// ============================================================

/// 验证结论 — VerifyAgent 对单次执行的判定
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verification {
    /// 是否通过(pass_rate ≥ 0.9 且真实执行检查通过)
    pub passed: bool,
    /// 测试通过率
    pub pass_rate: f32,
    /// 真实执行检查结果(快手 Process-Score 关键检查:防"硬编码通过")
    pub real_execution: bool,
    /// 失败记录(供 BuildAgent 修复)
    pub errors: Vec<BuildFailure>,
}

/// VerifyAgent — 沙箱验证智能体(方案 §8.2 的骨架降级)
///
/// 持有注入的沙箱执行器,验证 = 执行 + 通过率判定 + 真实执行检查。
/// 方案的多次运行一致性检查(consistency_checker)推迟到接线期
/// (骨架期单次执行,一致性检查需真实沙箱多次运行才有意义)。
#[derive(Debug)]
pub struct VerifyAgent<E: SandboxExec> {
    executor: E,
}

impl<E: SandboxExec> VerifyAgent<E> {
    /// 创建 VerifyAgent(注入沙箱执行器)
    pub fn new(executor: E) -> Self {
        Self { executor }
    }

    /// 验证构建脚本:沙箱执行 → 通过率判定 + 真实执行检查
    pub fn verify(&self, script: &BuildScript) -> Verification {
        let report = self.executor.execute(script);
        // 真实执行检查(复用 process_score,快手洞察:>10ms 且 coverage>0)
        let real_execution = check_real_execution(report.execution_time_ms, report.coverage);
        let passed = report.pass_rate >= VERIFY_PASS_RATE_THRESHOLD && real_execution;
        Verification {
            passed,
            pass_rate: report.pass_rate,
            real_execution,
            errors: report.errors,
        }
    }
}

// ============================================================
// AutoBuilder — 双智能体迭代循环
// ============================================================

/// 构建结果 — AutoBuilder 迭代循环的最终产出
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildResult {
    /// 是否构建成功(最终验证通过)
    pub success: bool,
    /// 最终脚本(含修复迭代产出)
    pub script: BuildScript,
    /// 最终测试通过率
    pub pass_rate: f32,
    /// 实际修复迭代次数(0 = 首次即通过)
    pub iterations: u32,
}

/// AutoBuilder — 双智能体协同构建可运行环境(方案 §8.2 骨架)
///
/// 迭代循环: BuildAgent 生成脚本 → VerifyAgent 沙箱验证 →
/// 未通过则 BuildAgent 修复 → 重验,直至通过或达 `max_iterations`。
#[derive(Debug)]
pub struct AutoBuilder<E: SandboxExec> {
    build_agent: BuildAgent,
    verify_agent: VerifyAgent<E>,
    max_iterations: u32,
}

impl<E: SandboxExec> AutoBuilder<E> {
    /// 创建 AutoBuilder(注入沙箱执行器与迭代上限)
    pub fn new(executor: E, max_iterations: u32) -> Self {
        Self {
            build_agent: BuildAgent::new(),
            verify_agent: VerifyAgent::new(executor),
            max_iterations,
        }
    }

    /// 自动分析仓库布局并迭代构建环境
    ///
    /// # 终止条件
    /// - 验证通过(success = true,iterations = 已用修复轮数)
    /// - 达到 `max_iterations`(success = false,返回最后一轮脚本与通过率)
    pub fn build(&self, layout: &RepoLayout) -> BuildResult {
        let mut script = self.build_agent.initial_script(layout);
        let mut iteration = 0u32;

        loop {
            let verification = self.verify_agent.verify(&script);
            if verification.passed {
                return BuildResult {
                    success: true,
                    script,
                    pass_rate: verification.pass_rate,
                    iterations: iteration,
                };
            }
            if iteration >= self.max_iterations {
                return BuildResult {
                    success: false,
                    script,
                    pass_rate: verification.pass_rate,
                    iterations: iteration,
                };
            }
            // 未通过且有余量:BuildAgent 规则式修复后重验
            script = self.build_agent.fix(&script, &verification.errors);
            iteration += 1;
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// mock 执行器:恒定通过
    struct AlwaysPass;
    impl SandboxExec for AlwaysPass {
        fn execute(&self, _script: &BuildScript) -> ExecReport {
            ExecReport {
                pass_rate: 1.0,
                execution_time_ms: 100,
                coverage: 0.9,
                errors: vec![],
            }
        }
    }

    /// mock 执行器:恒定失败(依赖缺失)
    struct AlwaysFail;
    impl SandboxExec for AlwaysFail {
        fn execute(&self, _script: &BuildScript) -> ExecReport {
            ExecReport {
                pass_rate: 0.2,
                execution_time_ms: 100,
                coverage: 0.5,
                errors: vec![BuildFailure {
                    step_index: 1,
                    message: "crate not found".to_string(),
                }],
            }
        }
    }

    /// mock 执行器:第 N 次调用起通过(验证修复迭代路径)
    struct PassAfter {
        calls: Cell<u32>,
        pass_at: u32,
    }
    impl SandboxExec for PassAfter {
        fn execute(&self, _script: &BuildScript) -> ExecReport {
            let n = self.calls.get() + 1;
            self.calls.set(n);
            if n >= self.pass_at {
                ExecReport {
                    pass_rate: 0.95,
                    execution_time_ms: 200,
                    coverage: 0.7,
                    errors: vec![],
                }
            } else {
                ExecReport {
                    pass_rate: 0.5,
                    execution_time_ms: 200,
                    coverage: 0.7,
                    errors: vec![BuildFailure {
                        step_index: 0,
                        message: "dependency missing".to_string(),
                    }],
                }
            }
        }
    }

    /// mock 执行器:"硬编码通过"(通过率 1.0 但执行 0ms 零覆盖)
    struct FakePass;
    impl SandboxExec for FakePass {
        fn execute(&self, _script: &BuildScript) -> ExecReport {
            ExecReport {
                pass_rate: 1.0,
                execution_time_ms: 1, // <10ms:疑似空跑
                coverage: 0.0,        // 零覆盖:疑似硬编码
                errors: vec![],
            }
        }
    }

    // ============================================================
    // BuildAgent 测试
    // ============================================================

    #[test]
    fn test_initial_script_cargo() {
        let agent = BuildAgent::new();
        let script = agent.initial_script(&RepoLayout {
            manifest: ManifestKind::CargoToml,
            has_lockfile: true,
        });
        assert_eq!(script.steps.len(), 3);
        assert!(script.steps[1].contains("cargo build"));
    }

    #[test]
    fn test_initial_script_npm_lockfile_uses_ci() {
        let agent = BuildAgent::new();
        let with_lock = agent.initial_script(&RepoLayout {
            manifest: ManifestKind::PackageJson,
            has_lockfile: true,
        });
        assert_eq!(with_lock.steps[0], "npm ci");
        let without_lock = agent.initial_script(&RepoLayout {
            manifest: ManifestKind::PackageJson,
            has_lockfile: false,
        });
        assert_eq!(without_lock.steps[0], "npm install");
    }

    #[test]
    fn test_initial_script_unknown_is_conservative() {
        let agent = BuildAgent::new();
        let script = agent.initial_script(&RepoLayout {
            manifest: ManifestKind::Unknown,
            has_lockfile: false,
        });
        // 未识别 manifest 只探测不构建
        assert_eq!(script.steps.len(), 1);
    }

    #[test]
    fn test_fix_inserts_fetch_on_missing_dependency() {
        let agent = BuildAgent::new();
        let script = agent.initial_script(&RepoLayout {
            manifest: ManifestKind::CargoToml,
            has_lockfile: true,
        });
        let fixed = agent.fix(
            &script,
            &[BuildFailure {
                step_index: 1,
                message: "crate foo not found".to_string(),
            }],
        );
        assert_eq!(fixed.steps.len(), script.steps.len() + 1);
        assert!(fixed.steps[1].contains("fetch"));
        assert_eq!(fixed.fix_notes.len(), 1);
    }

    #[test]
    fn test_fix_unrecognized_failure_only_notes() {
        let agent = BuildAgent::new();
        let script = agent.initial_script(&RepoLayout {
            manifest: ManifestKind::CargoToml,
            has_lockfile: true,
        });
        let fixed = agent.fix(
            &script,
            &[BuildFailure {
                step_index: 2,
                message: "segfault".to_string(),
            }],
        );
        // 未识别失败:脚本不变,只记 note
        assert_eq!(fixed.steps.len(), script.steps.len());
        assert_eq!(fixed.fix_notes.len(), 1);
    }

    // ============================================================
    // VerifyAgent 测试
    // ============================================================

    #[test]
    fn test_verify_pass() {
        let agent = VerifyAgent::new(AlwaysPass);
        let script = BuildAgent::new().initial_script(&RepoLayout {
            manifest: ManifestKind::CargoToml,
            has_lockfile: true,
        });
        let v = agent.verify(&script);
        assert!(v.passed);
        assert!(v.real_execution);
    }

    #[test]
    fn test_verify_rejects_fake_pass() {
        // 快手 Process-Score 关键检查:通过率 1.0 但空跑/零覆盖 = 疑似硬编码,拒绝
        let agent = VerifyAgent::new(FakePass);
        let script = BuildAgent::new().initial_script(&RepoLayout {
            manifest: ManifestKind::CargoToml,
            has_lockfile: true,
        });
        let v = agent.verify(&script);
        assert!(!v.passed);
        assert!(!v.real_execution);
    }

    #[test]
    fn test_verify_fail_carries_errors() {
        let agent = VerifyAgent::new(AlwaysFail);
        let script = BuildAgent::new().initial_script(&RepoLayout {
            manifest: ManifestKind::CargoToml,
            has_lockfile: true,
        });
        let v = agent.verify(&script);
        assert!(!v.passed);
        assert_eq!(v.errors.len(), 1);
    }

    // ============================================================
    // AutoBuilder 迭代循环测试
    // ============================================================

    #[test]
    fn test_build_first_pass_zero_iterations() {
        let builder = AutoBuilder::new(AlwaysPass, DEFAULT_MAX_ITERATIONS);
        let result = builder.build(&RepoLayout {
            manifest: ManifestKind::CargoToml,
            has_lockfile: true,
        });
        assert!(result.success);
        assert_eq!(result.iterations, 0);
    }

    #[test]
    fn test_build_recovers_after_fix() {
        // 第 2 次执行通过 → 1 轮修复迭代后成功
        let builder = AutoBuilder::new(
            PassAfter {
                calls: Cell::new(0),
                pass_at: 2,
            },
            DEFAULT_MAX_ITERATIONS,
        );
        let result = builder.build(&RepoLayout {
            manifest: ManifestKind::CargoToml,
            has_lockfile: true,
        });
        assert!(result.success);
        assert_eq!(result.iterations, 1);
        // 修复迭代应在脚本中留下痕迹
        assert!(!result.script.fix_notes.is_empty());
    }

    #[test]
    fn test_build_gives_up_at_max_iterations() {
        let builder = AutoBuilder::new(AlwaysFail, 2);
        let result = builder.build(&RepoLayout {
            manifest: ManifestKind::CargoToml,
            has_lockfile: true,
        });
        assert!(!result.success);
        assert_eq!(result.iterations, 2);
        assert!(result.pass_rate < VERIFY_PASS_RATE_THRESHOLD);
    }
}
