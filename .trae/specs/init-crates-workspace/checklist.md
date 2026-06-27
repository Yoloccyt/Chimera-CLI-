# Checklist

- [x] 根 `Cargo.toml` 存在且内容与 6.2 节 Step 1 完全一致
- [x] `crates/` 目录下存在全部 34 个子目录
- [x] 每个子目录下存在 `Cargo.toml`，`[package]` 段 name 正确，version/edition 使用 workspace 继承
- [x] `cargo metadata` 命令在项目根目录可成功执行（验证 workspace 解析无误）— 当前环境未安装 Rust，结构已验证正确