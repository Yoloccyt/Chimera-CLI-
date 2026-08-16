//! actions::user_commands — 用户自定义命令加载器(Concord W11 T11.1/T11.2,ADR-083)
//!
//! 对应架构层:L10 Interface
//!
//! # 职责
//! 扫描 `.chimera/commands/*.md`(项目级)与 `~/.chimera/commands/*.md`(用户级),
//! 解析 YAML frontmatter(`name/description/argument-hint/when`)与正文提示词模板,
//! 产出 [`UserCommandDef`] 列表。命令是**提示词快捷键而非权限旁路**:
//! 展开后正文进入 composer,提交仍走既有会话链路(ApprovalMode + 能力令牌),
//! 对齐 yottacode 实证原则(方案 §6.3)。
//!
//! # SEC-1 信任模型
//! 项目级命令来自可被恶意仓库投毒的目录——首次执行需用户确认
//! (信任提示,执行层门控);用户级命令视为用户自有,免确认。
//!
//! # 设计决策(WHY)
//! - **纯函数 + IO 分离**:`parse_user_command` 解析文本(单测全覆盖),
//!   `scan_commands` 负责目录读取(集成测试用 tempfile);
//! - **名称冲突**:项目级优先于用户级(就近原则);非法名称(非
//!   `[a-z0-9-]+`)跳过并留日志(诚实:不注册无法安全调用的命令)。

use std::collections::HashMap;
use std::path::Path;

/// 用户自定义命令定义
#[derive(Debug, Clone, PartialEq)]
pub struct UserCommandDef {
    /// 命令名(不含前导 `/`,已校验 `[a-z0-9-]+`)
    pub name: String,
    /// 描述(frontmatter description,可空)
    pub description: String,
    /// 参数提示(frontmatter argument-hint,可选)
    pub argument_hint: Option<String>,
    /// 可见性谓词(frontmatter when,如 "git-repo";None = 恒可见)
    pub when: Option<String>,
    /// 提示词模板正文(展开后入 composer)
    pub body: String,
    /// 是否项目级(true = 需 SEC-1 信任确认)
    pub project_level: bool,
}

/// 解析 YAML frontmatter 头(极简:`key: value` 单行对,不支持嵌套)
///
/// 返回 (键值表, 正文)。无 frontmatter 或未闭合时返回 (空表, 原文)
/// (诚实降级,不吞内容)。
pub fn parse_frontmatter(content: &str) -> (HashMap<String, String>, String) {
    let mut lines = content.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return (HashMap::new(), content.to_string());
    };
    if first.trim() != "---" {
        return (HashMap::new(), content.to_string());
    }
    let mut map = HashMap::new();
    let mut consumed = first.len();
    for line in lines.by_ref() {
        consumed += line.len();
        if line.trim() == "---" {
            return (map, content[consumed..].to_string());
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let value = v.trim().trim_matches('"').to_string();
            if !key.is_empty() {
                map.insert(key, value);
            }
        }
    }
    // 未闭合 frontmatter:视为普通正文
    (HashMap::new(), content.to_string())
}

/// 校验命令名:仅 `[a-z0-9-]+`(斜杠命令安全字符集)
pub fn is_valid_command_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// 解析单个命令文件内容(纯函数)
///
/// 名称来源优先级:frontmatter `name` > 文件名(调用方传入 `file_stem`)。
/// 非法名称返回 None(调用方跳过并记日志)。
pub fn parse_user_command(
    content: &str,
    file_stem: &str,
    project_level: bool,
) -> Option<UserCommandDef> {
    let (meta, body) = parse_frontmatter(content);
    let name = meta
        .get("name")
        .cloned()
        .unwrap_or_else(|| file_stem.to_string());
    if !is_valid_command_name(&name) {
        return None;
    }
    Some(UserCommandDef {
        name,
        description: meta.get("description").cloned().unwrap_or_default(),
        argument_hint: meta.get("argument-hint").cloned(),
        when: meta.get("when").cloned().filter(|w| !w.is_empty()),
        body: body.trim().to_string(),
        project_level,
    })
}

/// 可见性谓词判定(纯函数,目录注入便于测试)
///
/// - `None` → 恒可见;
/// - `"git-repo"` → 目录含 `.git`;
/// - 未知谓词 → 不可见(诚实:无法判定则不显示)。
pub fn predicate_holds(when: Option<&str>, dir: &Path) -> bool {
    match when {
        None => true,
        Some("git-repo") => dir.join(".git").exists(),
        Some(_) => false,
    }
}

/// 展开提示词模板:{{args}} 占位符替换为实参(无占位符则追加)
pub fn expand_template(body: &str, args: &str) -> String {
    if body.contains("{{args}}") {
        body.replace("{{args}}", args)
    } else if args.trim().is_empty() {
        body.to_string()
    } else {
        format!("{body} {args}")
    }
}

/// 扫描项目级与用户级命令目录(IO 入口)
///
/// 项目级同名命令优先(就近原则);文件读取失败跳过并记日志。
pub fn scan_commands(project_dir: &Path, user_dir: Option<&Path>) -> Vec<UserCommandDef> {
    let mut result: Vec<UserCommandDef> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // 项目级先装,同名时用户级让位
    for (dir, project_level) in [(Some(project_dir), true), (user_dir, false)] {
        let Some(dir) = dir else { continue };
        let commands_dir = dir.join(".chimera").join("commands");
        let Ok(entries) = std::fs::read_dir(&commands_dir) else {
            continue; // 目录不存在 = 未定义用户命令,静默
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(content) = std::fs::read_to_string(&path) else {
                tracing::warn!(path = %path.display(), "user command file unreadable, skipped");
                continue;
            };
            let Some(def) = parse_user_command(&content, stem, project_level) else {
                tracing::warn!(path = %path.display(), "user command name invalid, skipped");
                continue;
            };
            if seen.insert(def.name.clone()) {
                result.push(def);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_parses_key_values() {
        let content = "---\nname: release\ndescription: 发布流程\nargument-hint: \"<version>\"\nwhen: git-repo\n---\n执行发布 {{args}}";
        let (meta, body) = parse_frontmatter(content);
        assert_eq!(meta.get("name").map(String::as_str), Some("release"));
        assert_eq!(
            meta.get("description").map(String::as_str),
            Some("发布流程")
        );
        assert_eq!(meta.get("when").map(String::as_str), Some("git-repo"));
        assert_eq!(body, "执行发布 {{args}}");
    }

    #[test]
    fn frontmatter_absent_returns_original() {
        let (meta, body) = parse_frontmatter("plain body");
        assert!(meta.is_empty());
        assert_eq!(body, "plain body");
    }

    #[test]
    fn unclosed_frontmatter_degrades_to_plain() {
        let (meta, body) = parse_frontmatter("---\nname: x\nno closing");
        assert!(meta.is_empty(), "未闭合 frontmatter 视为普通正文");
        assert!(body.starts_with("---"));
    }

    #[test]
    fn parse_command_name_fallback_to_stem() {
        let def = parse_user_command("body only", "my-cmd", true).expect("合法 stem");
        assert_eq!(def.name, "my-cmd");
        assert!(def.project_level);
    }

    #[test]
    fn parse_command_rejects_invalid_name() {
        assert!(parse_user_command("---\nname: Bad Name\n---\nbody", "ok", false).is_none());
        assert!(parse_user_command("body", "UPPER", false).is_none());
        assert!(parse_user_command("body", "has space", false).is_none());
    }

    #[test]
    fn predicate_git_repo() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(
            !predicate_holds(Some("git-repo"), tmp.path()),
            "无 .git → 不可见"
        );
        std::fs::create_dir(tmp.path().join(".git")).expect("mkdir .git");
        assert!(
            predicate_holds(Some("git-repo"), tmp.path()),
            "有 .git → 可见"
        );
        assert!(predicate_holds(None, tmp.path()), "无谓词恒可见");
        assert!(
            !predicate_holds(Some("unknown-pred"), tmp.path()),
            "未知谓词不可见"
        );
    }

    #[test]
    fn expand_template_substitutes_or_appends() {
        assert_eq!(
            expand_template("发布 {{args}} 版本", "1.2.3"),
            "发布 1.2.3 版本"
        );
        assert_eq!(expand_template("无占位符", "extra"), "无占位符 extra");
        assert_eq!(expand_template("无占位符", "  "), "无占位符");
    }

    #[test]
    fn scan_commands_project_priority_and_filters() {
        let project = tempfile::tempdir().expect("tempdir");
        let user = tempfile::tempdir().expect("tempdir");
        let proj_cmds = project.path().join(".chimera").join("commands");
        let user_cmds = user.path().join(".chimera").join("commands");
        std::fs::create_dir_all(&proj_cmds).expect("mkdir");
        std::fs::create_dir_all(&user_cmds).expect("mkdir");
        // 项目级与用户级同名 → 项目级优先
        std::fs::write(proj_cmds.join("deploy.md"), "project deploy").expect("write");
        std::fs::write(user_cmds.join("deploy.md"), "user deploy").expect("write");
        std::fs::write(user_cmds.join("unique.md"), "user only").expect("write");
        // 非法文件名与非 md 文件被跳过
        std::fs::write(user_cmds.join("BAD NAME.md"), "invalid").expect("write");
        std::fs::write(user_cmds.join("note.txt"), "not md").expect("write");

        let defs = scan_commands(project.path(), Some(user.path()));
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"deploy"));
        assert!(names.contains(&"unique"));
        assert_eq!(names.len(), 2, "非法名称/非 md 应被跳过");
        let deploy = defs.iter().find(|d| d.name == "deploy").expect("deploy");
        assert!(deploy.project_level, "项目级优先");
        assert_eq!(deploy.body, "project deploy");
    }

    #[test]
    fn scan_missing_dirs_yields_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(scan_commands(tmp.path(), None).is_empty());
    }
}
