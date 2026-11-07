//! X2 派发链热路径改造的等价性守卫（ADR-256）。
//!
//! 本批改的是**判决的构造方式**（去分配、去丢弃的拥有型出口），不是判决本身。
//! 因此唯一可接受的验收是：新判决入口与旧判决入口对**同一输入集**给出
//! 逐字相同的结论（含错误类型与文案）。
//!
//! 判据取材纪律：`default_secure` 的拦截模式清单**不手抄**，而是遍历
//! `CommandPolicy::default_secure().blocked_patterns` 现场派生 —— 手抄夹具
//! 会在生产清单变动时继续「测过」，等于没有测。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code

use seccore::policy::validate_command_verdict;
use seccore::{validate_command, AttackType, Command, CommandPolicy};

/// 把两条判决入口的结论归一为可比较的字符串。
///
/// 旧入口返回 `Result<CommandSpec, _>`（其 Ok 负载在派发链被**整个丢弃**，
/// 见 sandbox_gate.rs 的 `check`），故归一到「判决形状」而非「判决 + 负载」。
fn owned_verdict(program: &str, args: &[String], policy: &CommandPolicy) -> String {
    let cmd = Command {
        program: program.to_string(),
        args: args.to_vec(),
        env: std::collections::HashMap::new(),
    };
    match validate_command(&cmd, policy) {
        Ok(_) => "OK".to_string(),
        Err(e) => format!("ERR {:?}", e),
    }
}

fn verdict(program: &str, args: &[&str], policy: &CommandPolicy) -> String {
    match validate_command_verdict(program, args, policy) {
        Ok(()) => "OK".to_string(),
        Err(e) => format!("ERR {:?}", e),
    }
}

/// 逐条覆盖 `default_secure` 真实拦截模式：把每条 pattern 嵌进一条命令，
/// 两入口必须给出逐字相同的结论。
#[test]
fn verdict_path_matches_owned_path_for_every_default_secure_pattern() {
    let policy = CommandPolicy::default_secure();
    let mut checked = 0usize;

    for pattern in &policy.blocked_patterns {
        // 前后垫字符，避免依赖 pattern 恰好落在串首/串尾。
        let program = format!("ab{}cd", pattern.pattern);
        let owned = owned_verdict(&program, &[], &policy);
        let fast = verdict(&program, &[], &policy);
        assert_eq!(
            owned, fast,
            "pattern {:?} 注入命令 {program:?} 时两入口判决分叉",
            pattern.pattern
        );
        assert!(
            fast.starts_with("ERR"),
            "pattern {:?} 本应拦截，但判决为 {fast:?}",
            pattern.pattern
        );
        // 归因到**该条模式自己的 attack_type**，而非白名单 Abuse ——
        // 否则「ERR」可能来自命令压根不在白名单，模式是否命中从未被检验。
        assert!(
            fast.contains(&format!("{:?}", pattern.attack_type)),
            "pattern {:?} 的拦截归因不是它自身的 attack_type（{:?}）:{fast:?}",
            pattern.pattern,
            pattern.attack_type
        );
        checked += 1;
    }

    // 规模闸门：遍历必须真的覆盖了清单，否则本测试是空转。
    assert!(
        checked >= 10,
        "default_secure 拦截模式数异常偏低（{checked}），等价性覆盖不成立"
    );
}

/// 参数参与拼接的形态（`build_full_command_string` 用空格连接 program+args）：
/// 注入可能落在 args 里而非 program 里，两入口同样必须一致。
#[test]
fn verdict_path_matches_owned_path_when_injection_is_in_args() {
    let policy = CommandPolicy::default_secure();
    for args in [
        vec!["$(id)".to_string()],
        vec!["safe".to_string(), ";rm".to_string()],
        vec!["a|b".to_string()],
        vec!["--x".to_string(), "*".to_string()],
        vec!["out>file".to_string()],
        vec!["echo".to_string(), "plain".to_string()],
    ] {
        let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
        let owned = owned_verdict("echo", &args, &policy);
        let fast = verdict("echo", &args_ref, &policy);
        assert_eq!(owned, fast, "args {args:?} 两入口判决分叉");
    }
}

/// 放行路径：白名单内且不含任何拦截模式的命令，两入口都必须 OK。
#[test]
fn both_paths_allow_clean_whitelisted_command() {
    let policy = CommandPolicy::default_secure();
    let owned = owned_verdict("echo", &["hello".to_string()], &policy);
    let fast = verdict("echo", &["hello"], &policy);
    assert_eq!(owned, "OK", "旧入口应放行，实得 {owned:?}");
    assert_eq!(owned, fast);
}

/// 非 ASCII 拦截模式必须仍走 Unicode 折叠路径（快路径只接管两侧皆 ASCII 的情形）。
///
/// 中文放 **args** 位而非 program 位：白名单比的是 program 整串，
/// 若把待测文本塞进 program，命令会先以 Abuse 被拦 —— 那样两入口照样「一致」，
/// 但一致的原因是**非 ASCII 模式根本没被试过**，测试失去牙齿（本条首跑即栽在此）。
#[test]
fn non_ascii_pattern_still_matches_through_verdict_path() {
    let policy = CommandPolicy::new().allow_command("echo").block_pattern(
        "删除",
        AttackType::Tamper,
        "自定义非 ASCII 拦截模式（测试夹具）",
    );

    // 命中：haystack 非 ASCII ⇒ 必须回落到 to_lowercase 路径，且判决逐字一致。
    let args = vec!["请删除文件".to_string()];
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    let owned = owned_verdict("echo", &args, &policy);
    let fast = verdict("echo", &args_ref, &policy);
    assert!(
        owned.starts_with("ERR"),
        "非 ASCII 模式应拦截，实得 {owned:?}"
    );
    assert!(
        owned.contains("Tamper"),
        "应归因到非 ASCII 拦截模式，而不是白名单 Abuse（那样本条就没试过该模式）:{owned:?}"
    );
    assert_eq!(owned, fast);

    // 不命中：既必须一致，也必须真的放行。
    let ok_args = vec!["保留文件".to_string()];
    let ok_ref: Vec<&str> = ok_args.iter().map(String::as_str).collect();
    let owned_ok = owned_verdict("echo", &ok_args, &policy);
    assert_eq!(owned_ok, verdict("echo", &ok_ref, &policy));
    assert_eq!(owned_ok, "OK", "与模式无关的命令应放行，实得 {owned_ok:?}");
}

/// Unicode 大小写折叠的**跨域陷阱**正控：U+212A KELVIN SIGN 在 `to_lowercase()`
/// 下折叠成 ASCII 'k'，但「逐字符 ASCII 小写比较」看不见它。
/// 若快路径错误地按「needle 是 ASCII」而非「haystack 是 ASCII」来选路，
/// 这条命令就会被放行 —— 本测试即为该缺陷的示证靶。
///
/// 夹具用 builder 组合而非 `BlockedPattern` 字面量：后者是 `#[non_exhaustive]`，
/// 跨 crate 无法用结构体字面量构造（编译器 E0639），且刻意如此 ——
/// 新增字段时不破坏下游。
#[test]
fn unicode_case_fold_edge_case_agrees_between_paths() {
    let policy = CommandPolicy::new().allow_command("echo").block_pattern(
        "k",
        AttackType::Tamper,
        "ASCII k 模式（Unicode 折叠正控夹具）",
    );

    // haystack 含 U+212A ⇒ to_lowercase 后为 "akb"，应命中 "k"。
    // 文本放 args 位：program 若含该码位，白名单会先以 Abuse 拦截，
    // 于是「两入口都是 ERR」会因**错误的原因**通过 —— 故既比一致，
    // 也断言归因必须是 Tamper（证明模式真的被试过）。
    let args = vec!["a\u{212a}b".to_string()];
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    let owned = owned_verdict("echo", &args, &policy);
    let fast = verdict("echo", &args_ref, &policy);
    assert!(
        owned.starts_with("ERR"),
        "前提不成立：旧入口未把 U+212A 折叠为 k，本正控失去意义（实得 {owned:?}）"
    );
    assert!(
        owned.contains("Tamper"),
        "必须归因到夹具的 k 模式（Tamper），而不是白名单 Abuse:{owned:?}"
    );
    assert_eq!(
        owned, fast,
        "快路径漏掉了 Unicode 折叠命中 ⇒ 安全判决被优化掉（fast={fast:?}）"
    );

    // 反向对照：纯 ASCII 且不含 k 的命令两入口都应放行。
    let clean = vec!["ab".to_string()];
    let clean_ref: Vec<&str> = clean.iter().map(String::as_str).collect();
    let owned_clean = owned_verdict("echo", &clean, &policy);
    assert_eq!(owned_clean, verdict("echo", &clean_ref, &policy));
    assert_eq!(owned_clean, "OK", "无关命令应放行，实得 {owned_clean:?}");
}

/// 风险通道只影响被丢弃的 `CommandSpec`，不影响判决：
/// 含 `>` / `*` 的命令若未触拦截模式，两入口结论必须相同。
#[test]
fn risk_scoring_path_does_not_change_the_verdict() {
    let policy = CommandPolicy::new().allow_command("grep");
    // `>` 在 default_secure 里是拦截模式，但本夹具未挂 ⇒ 仅进入风险评分。
    for args in [vec!["a>b"], vec!["*.log"], vec!["plain"]] {
        let owned = owned_verdict(
            "grep",
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &policy,
        );
        let fast = verdict("grep", &args, &policy);
        assert_eq!(owned, fast, "风险通道不应改变判决：{args:?}");
    }
}

/// 白名单缺失（Abuse 通道）两入口一致，且 program 大小写不敏感语义保持。
#[test]
fn abuse_verdict_and_case_insensitive_program_agree() {
    let policy = CommandPolicy::default_secure();
    for program in ["nosuchprogram", "ECHO", "Echo", "echo"] {
        let owned = owned_verdict(program, &[], &policy);
        let fast = verdict(program, &[], &policy);
        assert_eq!(owned, fast, "program {program:?} 判决分叉");
    }
}
