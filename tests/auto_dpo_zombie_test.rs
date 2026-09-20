//! auto-dpo 僵尸 crate 检测测试
//!
//! **目的**:验证 auto-dpo 是全库零消费 (除自身 lib.rs + 注释 + dev-dep)
//! **证据链**:grep "use auto_dpo" in src/ excluding auto-dpo itself
//! **置信度**:High(四源一致：零 src 引用 + zero test + zero bench)

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    #[test]
    fn test_auto_dpo_is_zombie() {
        // 递归扫描 crates/目录下的所有.rs 文件
        let crates_dir = Path::new("crates");
        let mut found_imports = Vec::new();

        scan_rs_files(crates_dir, &mut found_imports, "auto_dpo");

        // 过滤掉 auto-dpo 自身的引用
        let real_consumers: Vec<&String> = found_imports
            .iter()
            .filter(|line| !line.contains("crates/auto-dpo/"))
            .filter(|line| {
                // 跳过注释行
                !line.trim().starts_with("//") &&
                !line.trim().starts_with("/*")
            })
            .collect();

        if !real_consumers.is_empty() {
            let consumed_lines: Vec<String> = real_consumers.iter().map(|s| s.to_string()).collect();
            panic!(
                "auto-dpo 被发现被其他 crate 导入:\n{}",
                consumed_lines.join("\n")
            );
        }

        println!("✅ auto-dpo 确认为僵尸 crate(全库零生产消费)");
    }

    #[test]
    fn test_auto_dpo_removed() {
        let lib_rs_path = Path::new("crates/auto-dpo/src/lib.rs");
        assert!(!lib_rs_path.exists(), "auto-dpo 应已被删除");
        println!("✅ auto-dpo crate 已成功删除");
    }

    // 递归扫描.rs 文件并查找包含 pattern 的行
    fn scan_rs_files(dir: &Path, results: &mut Vec<String>, pattern: &str) {
        if dir.is_dir() {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        scan_rs_files(&path, results, pattern);
                    } else if path.extension().map_or(false, |ext| ext == "rs") {
                        if let Ok(content) = fs::read_to_string(&path) {
                            for (line_num, line) in content.lines().enumerate() {
                                if line.contains(pattern) {
                                    let full_path = path
                                        .to_string_lossy()
                                        .replace("\\", "/");
                                    results.push(format!(
                                        "{}:{}:{}",
                                        full_path,
                                        line_num + 1,
                                        line.trim()
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
