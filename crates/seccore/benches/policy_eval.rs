//! R1(ADR-260)判决体优化同轮对照基准:索引化零分配 vs 改造前线性判决。
//!
//! # 设计（ADR-260 §4,ADR-249 形态的 L4 硬门）
//!
//! - **同一条规则集**：`{4, 32, 256}` 规则（program 轮转 + glob 轮转）,
//!   优化臂（`ExecPolicy`）与参考臂（`ReferencePolicy` = 改造前判决体的
//!   逐位复刻:线性扫描 + 每规则 `to_lowercase` + join_args + DP glob）
//!   同轮交替插入同一 `BenchmarkGroup`,禁跨轮对比。
//! - **场景**：`hit` = 首条规则 `*` 白名单命中（零信任放行的 dominant
//!   形态）;`miss` = 未登记程序回默认 Deny（零信任下所有未声明工具,
//!   R1 收益的主战场:O(R)→O(1) 且消 2k 次归一分配）。
//! - **采纳判据（ADR-260 硬门）**：R=32/256 的 miss 对 `opt ≤ 0.70 × ref`;
//!   hit 对 informational（两侧均 O(1),允许 parity,劣化 >5% 才触发排查）。
//!
//! 运行：`cargo bench -p seccore --bench policy_eval`
#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use seccore::execpolicy::{ExecPolicy, ExecPolicyRule, PolicyAction, RulePattern};

/// 参考（改造前）判决体 —— 线性扫描 + 每规则 to_lowercase + join_args + DP glob
///
/// WHY 在 bench 内复刻而非引用历史代码:ADR-249 的同轮对照纪律要求两臂在同一
/// 二进制同一轮内可执行;参考体即 `execpolicy.rs` W4 后/R1 前的 `evaluate` +
/// `RulePattern::matches` + `glob_match` 的逐行平移。
struct ReferencePolicy {
    rules: Vec<(String, String, PolicyAction)>,
    default_action: PolicyAction,
}

impl ReferencePolicy {
    fn evaluate(&self, program: &str, args: &[&str]) -> PolicyAction {
        for (prog, glob, action) in &self.rules {
            if program.to_lowercase() != *prog {
                continue;
            }
            let joined = join_args(args);
            if joined.is_empty() && glob.ends_with(" *") {
                return *action;
            }
            if glob_match_dp(glob, &joined) {
                return *action;
            }
        }
        self.default_action
    }
}

/// 与 `&[String]::join(" ")` 语义一致的单次分配拼接（execpolicy.rs 既有助手平移）
fn join_args(args: &[&str]) -> String {
    let mut joined = String::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            joined.push(' ');
        }
        joined.push_str(arg);
    }
    joined
}

/// 改造前 glob:迭代 DP（Vec\<char\>×2 + n·m bool 表,三重堆分配）
fn glob_match_dp(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (n, m) = (p.len(), t.len());
    let mut dp = vec![vec![false; m + 1]; n + 1];
    dp[0][0] = true;
    for i in 1..=n {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=n {
        for j in 1..=m {
            if p[i - 1] == '*' {
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if p[i - 1] == t[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            }
        }
    }
    dp[n][m]
}

/// 规则集构造:program 轮转(git/npm/sh 前后夹查询名) + glob 轮转 + action 轮转;
/// 首条规则恒为 ("bench.echo", "*", Allow) 供 hit 场景立即命中。
fn build_rules(n: usize) -> Vec<ExecPolicyRule> {
    let programs = ["bench.echo", "git", "npm", "sh"];
    let globs = ["*", "push*", "install *", "status"];
    let actions = [PolicyAction::Allow, PolicyAction::Ask, PolicyAction::Deny];
    (0..n)
        .map(|i| ExecPolicyRule {
            pattern: RulePattern::new(programs[i % programs.len()], globs[i % globs.len()]),
            action: actions[i % actions.len()],
        })
        .collect()
}

const QUERY_HIT: (&str, [&str; 2]) = ("bench.echo", ["--flag", "value"]);
const QUERY_MISS: (&str, [&str; 2]) = ("unknown.tool", ["--flag", "value"]);

fn bench_policy_eval(c: &mut Criterion) {
    let mut group = c.benchmark_group("spol_eval");
    group.sample_size(30);

    for rules_n in [4usize, 32, 256] {
        let rules = build_rules(rules_n);
        let optimized: ExecPolicy = rules
            .iter()
            .cloned()
            .fold(ExecPolicy::new(), |p, r| p.add_rule(r));
        let reference = ReferencePolicy {
            rules: rules
                .iter()
                .map(|r| {
                    (
                        r.pattern.program.clone(),
                        r.pattern.args_glob.clone(),
                        r.action,
                    )
                })
                .collect(),
            default_action: PolicyAction::Deny,
        };

        for (scene, (program, args)) in [("hit", QUERY_HIT), ("miss", QUERY_MISS)] {
            group.bench_function(
                BenchmarkId::new("opt", format!("{scene}_R{rules_n}")),
                |b| b.iter(|| optimized.evaluate_ref(program, &args)),
            );
            group.bench_function(
                BenchmarkId::new("ref", format!("{scene}_R{rules_n}")),
                |b| b.iter(|| reference.evaluate(program, &args)),
            );
        }
    }
    group.finish();
}

criterion_group!(benches, bench_policy_eval);
criterion_main!(benches);
