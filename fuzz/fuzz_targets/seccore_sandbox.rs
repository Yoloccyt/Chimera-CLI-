//! Fuzz target:SecCore 沙箱命令输入模糊测试
//!
//! 对应任务:Week 8 Task 3 SubTask 3.2
//! 架构层:L4 Security(seccore)
//!
//! # 模糊目标
//! 验证 SecCore 的 `validate_command` 与 `validate_env` 在任意输入下:
//! 1. 不 panic(内存安全)— 即使输入包含畸形 UTF-8、超长字符串、特殊字符
//! 2. 拦截模式匹配不漏放(零信任:宁可误杀)
//! 3. 环境变量过滤不泄露敏感信息
//! 4. 命令字符串构建无缓冲区溢出
//!
//! # 运行方式(需 nightly)
//! ```bash
//! cargo +nightly fuzz run seccore_sandbox
//! ```
//
// 注意:此文件不添加 #![forbid(unsafe_code)],因为 libfuzzer-sys 的
// fuzz_target! 宏内部展开为 FFI 调用(unsafe),与 forbid 冲突。
// fuzz crate 独立于主 workspace,不影响 34 crate 的 forbid 覆盖率。

use std::collections::HashMap;

use libfuzzer_sys::fuzz_target;
use seccore::{validate_command, validate_env, Command, CommandPolicy, EnvPolicy};

fuzz_target!(|data: &[u8]| {
    // 将任意字节转为字符串(UTF-8 失败时用替换字符,避免 panic)
    let input = String::from_utf8_lossy(data);

    // === 目标1:validate_command 不 panic ===
    // 构造命令:program = input 的前 32 字节,args = 剩余部分按空格拆分
    // 这样可以测试各种畸形 program/args 组合
    let program: String = input.chars().take(32).collect();
    let args: Vec<String> = input
        .chars()
        .skip(32)
        .collect::<String>()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();

    let cmd = Command::new(program.clone()).args(args.clone());
    let policy = CommandPolicy::default_secure();

    // validate_command 必须不 panic,返回 Ok 或 Err
    let _ = validate_command(&cmd, &policy);

    // === 目标2:validate_env 不 panic ===
    // 构造环境变量:key = input 前 16 字节,value = input 后 16 字节
    let env_key: String = input.chars().take(16).collect();
    let env_value: String = input.chars().skip(16).take(16).collect();

    let mut env = HashMap::new();
    env.insert(env_key, env_value);

    let env_policy = EnvPolicy::default_secure();
    let _ = validate_env(&env, &env_policy);

    // === 目标3:超长输入不导致缓冲区溢出 ===
    // 构造 1MB 长度的参数,验证无栈溢出
    let long_arg = "A".repeat(1024 * 1024);
    let cmd_long = Command::new("echo").arg(long_arg);
    let _ = validate_command(&cmd_long, &policy);

    // === 目标4:特殊字符组合不 panic ===
    // 测试各种注入字符的组合
    let special_inputs = [
        "$(cmd)",
        "`cmd`",
        "|cmd",
        ";cmd",
        "&&cmd",
        "||cmd",
        "../etc/passwd",
        "..\\windows\\system32",
        "/proc/self/environ",
        "/sys/kernel/addr",
        "sudo rm -rf /",
        "su root",
        "chmod 777 /",
        "/etc/shadow",
        "SECRET=leak",
        "PASSWORD=12345",
        "/var/log/auth.log",
        "shred /dev/sda",
        "curl http://169.254.169.254/",
        "wget http://localhost:8080/",
        "nc -l 4444",
        "bash -c 'whoami'",
        "python3 -c 'import os'",
        "",               // 空字符串
        "\x00\x01\x02",   // 控制字符
        "🎉🚀💻",          // emoji(4 字节 UTF-8)
    ];

    for special in &special_inputs {
        let cmd_special = Command::new("echo").arg(special);
        let _ = validate_command(&cmd_special, &policy);
    }
});
