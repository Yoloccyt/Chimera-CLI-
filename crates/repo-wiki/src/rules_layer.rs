//! 分层项目规则收集 — "rules as code" 就近优先(rules_layer)
//!
//! 对应架构层:L5 Knowledge(repo-wiki 子模块)
//! 对应 v4.0 WI-33:分层项目规则 CHIMERA.md/AGENTS.md 逐级收集
//!
//! # 动机
//!
//! 在大型仓库中,规则散落于各层目录(仓库根 → 模块 → 示例)。本模块提供
//! 一种简单的"就近优先覆盖"规则收集:从给定目录逐级向上到文件系统根,
//! 收集每一级存在的 `CHIMERA.md`(若该级不存在则回退 `AGENTS.md`),
//! 并以"根 → 近"顺序返回,近者(depth 更大)覆盖远者(depth 更小)。
//!
//! # 与 LPA 组织层对接(说明性 hook,不进静态层)
//!
//! 本模块只提供**纯函数**:把收集到的规则文本按就近顺序合并成一段可注入
//! 的上下文(见 [`merge_rules_text`])。调用方(处于提示词"组织层/context
//! 层")将合并文本拼入提示词即可。repo-wiki 为 L5 知识层,**不得向上依赖
//! L6 model-router**:这里不产生任何 LLM 调用,不注入静态层,仅负责
//! "读文件 → 排序 → 组文本"的确定性工作。
//!
//! # 红线遵守
//!
//! - 无 unsafe(crate 顶层 `#![forbid(unsafe_code)]` 已覆盖,本模块不重复)
//! - 单函数 ≤ 200 行
//! - 收集容错:单级不可读仅跳过并产出警告,`collect_rules` 不因此失败中断
//! - 错误统一走 `crate::error::WikiError`,优先复用,不新增独立错误类型

use std::path::{Path, PathBuf};

use crate::error::WikiError;

/// `CHIMERA.md` 规则文件名(优先于 `AGENTS.md`)
pub const CHIMERA_MD: &str = "CHIMERA.md";
/// `AGENTS.md` 规则文件名(CHIMERA 缺失时的回退)
pub const AGENTS_MD: &str = "AGENTS.md";

/// 单条收集到的规则文件
#[derive(Debug, Clone)]
pub struct RuleFile {
    /// 规则文件所在目录(绝对/相对路径,与传入 `from_dir` 层级一致)
    pub dir: PathBuf,
    /// 规则文件来源名("CHIMERA.md" 或 "AGENTS.md")
    pub source: &'static str,
    /// 规则文件原文内容(整文件读取)
    pub content: String,
    /// 目录层级深度:0 = 最远的根,越接近当前目录 depth 越大(近者覆盖远者)
    pub depth: usize,
}

/// 单级收集跳过时的警告记录
#[derive(Debug, Clone)]
pub struct CollectWarning {
    /// 被跳过的目录
    pub dir: PathBuf,
    /// 跳过原因(IO 错误描述等)
    pub reason: String,
}

/// 从 `from_dir` 逐级向上到 `root`(含)收集每一级的规则文件。
///
/// 返回按「根 → 近」顺序排序的列表(即索引 0 = 最远的根级,末位 = 最近级)。
/// 每级优先读 `CHIMERA.md`;不存在则回退 `AGENTS.md`。单级读取失败仅跳过
/// 该级(细节见 [`collect_rules_with_warnings`]),不中断整个收集。
///
/// # 边界 `root`
/// 收集**向上止于 `root` 目录本身**(含),不会越过 `root` 上升——避免拾取
/// 仓库根之外的无关规则(如用户目录/文件系统根的 `AGENTS.md`)。调用方传
/// 仓库根;`None` 时退化为截至文件系统根的收集(仅用于特殊场景)。
///
/// # 容错
/// 收集不因单级失败中断:不可读目录/文件级会被跳过,仍返回其余成功项。
pub fn collect_rules(from_dir: &Path, root: Option<&Path>) -> Result<Vec<RuleFile>, WikiError> {
    let (rules, _warnings) = collect_rules_with_warnings(from_dir, root)?;
    Ok(rules)
}

/// 同 [`collect_rules`],但额外返回收集过程中的跳过警告。
///
/// 返回 `(规则列表, 警告列表)`:规则已按「根 → 近」排序;警告仅作诊断,
/// 不影响规则收集结果。
pub fn collect_rules_with_warnings(
    from_dir: &Path,
    root: Option<&Path>,
) -> Result<(Vec<RuleFile>, Vec<CollectWarning>), WikiError> {
    let mut level = from_dir.to_path_buf();
    // 近 → 根 方向暂存(为便于 depth 赋值,下面再反转)
    let mut collected: Vec<(PathBuf, &'static str, String)> = Vec::new();
    let mut warnings: Vec<CollectWarning> = Vec::new();

    loop {
        // 记录本轮处理的是否为 root 级;root 级处理后必须终止,不得再上升
        let mut is_root = false;
        if let Some(root) = root {
            if level == root {
                is_root = true;
            }
        }
        // 该级优先读 CHIMERA.md;NotFound 则回退 AGENTS.md;
        // 读取出现其他错误(如目录伪装成文件)=> 跳过整个 level 并记警告(不中断)。
        for src in [CHIMERA_MD, AGENTS_MD] {
            let candidate = level.join(src);
            match std::fs::read_to_string(&candidate) {
                Ok(content) => {
                    collected.push((level.clone(), src, content));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // 该文件名不存在,继续尝试下一个(AGENTS.md)
                    continue;
                }
                Err(e) => {
                    warnings.push(CollectWarning {
                        dir: level.clone(),
                        reason: format!("read {} failed: {e}", candidate.display()),
                    });
                    break; // 视为已处理该级,跳过
                }
            }
        }
        // 到了显式 root 级:处理完即止,绝不上升(避免拾取仓库根之外规则)
        if is_root {
            break;
        }
        // 上升一级;返回 false 表示已到文件系统根,终止
        if !level.pop() {
            break;
        }
    }

    // collected 当前为 近 → 根;反转成 根 → 近,并按序赋 depth(远=0,近=末位最大)
    collected.reverse();
    let rules: Vec<RuleFile> = collected
        .into_iter()
        .enumerate()
        .map(|(depth, (dir, source, content))| RuleFile {
            dir,
            source,
            content,
            depth,
        })
        .collect();

    Ok((rules, warnings))
}

/// 就近优先:返回优先级最高的规则文件(即 `depth` 最大者 / 最近级)。
///
/// 列表为空时返回 `None`;
/// 非空时恒定返回 `max_by_key(depth)` 命中的最近级规则,语义上近者覆盖远者。
pub fn nearest_wins(rules: &[RuleFile]) -> Option<&RuleFile> {
    rules.iter().max_by_key(|r| r.depth)
}

/// 就近优先:取最近级规则的内容;无任何规则时返回空字符串。
pub fn nearest_rule_content(rules: &[RuleFile]) -> String {
    nearest_wins(rules)
        .map(|r| r.content.clone())
        .unwrap_or_default()
}

/// 将「根 → 近」的规则列表合并为一段可注入提示词的文本。
///
/// 释放顺序遵循就近优先:更近的规则追加在末尾、后出现,自然覆盖更远规则。
/// 这是与 LPA「提示词组织层」对接的纯函数入口,不产生 LLM 调用、不注入静态层。
pub fn merge_rules_text(rules: &[RuleFile]) -> String {
    let mut out = String::new();
    for r in rules {
        out.push_str(&format!(
            "\n==== {} ({}, depth {}) ====\n",
            r.dir.display(),
            r.source,
            r.depth
        ));
        out.push_str(&r.content);
        if !r.content.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造唯一临时目录并返回其路径;每次测试独立
    fn unique_temp_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "repo_wiki_rules_layer_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&base).expect("create temp dir");
        base
    }

    /// 确保规则文件的规则:若 no_file 非空则跳过 direct(产生失败级)
    fn write_rule(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).expect("write rule file");
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn multi_level_collection_order_and_nearness() {
        let base = unique_temp_dir("order");
        // 树:base/a/b/c;其中 a 有 AGENTS.md,b 有 CHIMERA.md,c 无规则
        let a = base.join("a");
        let b = a.join("b");
        let c = b.join("c");
        std::fs::create_dir_all(&c).unwrap();
        write_rule(&a, AGENTS_MD, "rule-a");
        write_rule(&b, CHIMERA_MD, "rule-b-mid");

        let rules = collect_rules(&c, Some(&base)).expect("collect should succeed");
        // 根→近顺序:[a(depth 0), b(depth 1)],止于 base 不再上溯
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].dir, a);
        assert_eq!(rules[0].depth, 0);
        assert_eq!(rules[0].content, "rule-a");
        assert_eq!(rules[1].dir, b);
        assert_eq!(rules[1].depth, 1);
        assert_eq!(rules[1].content, "rule-b-mid");

        // 边界:即便 base 之上(user 目录/文件系统根)存在无关规则,也不越界收集
        let rules_bounded = collect_rules(&c, Some(&b)).expect("bounded collect");
        assert_eq!(rules_bounded.len(), 1, "止于 b,仅 b 一级");
        assert_eq!(rules_bounded[0].dir, b);

        // 就近覆盖:最近级 = b
        let nearest = nearest_wins(&rules).expect("has nearest");
        assert_eq!(nearest.dir, b);
        assert_eq!(nearest_rule_content(&rules), "rule-b-mid");

        cleanup(&base);
    }

    #[test]
    fn chimera_takes_priority_over_agents() {
        let base = unique_temp_dir("priority");
        // 单级同时存在 CHIMERA.md 与 AGENTS.md,应选 CHIMERA
        write_rule(&base, CHIMERA_MD, "chimera-content");
        write_rule(&base, AGENTS_MD, "agents-content");

        let rules = collect_rules(&base, Some(&base)).expect("collect should succeed");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].source, CHIMERA_MD);
        assert_eq!(rules[0].content, "chimera-content");
        // 父级(root 方向)即使有 AGENTS,也不会覆盖更近的 CHIMERA
        assert_eq!(nearest_wins(&rules).expect("has one").source, CHIMERA_MD);

        cleanup(&base);
    }

    #[test]
    fn single_level_failure_is_skipped_with_warning() {
        let base = unique_temp_dir("skip");
        // near/CHIMERA.md 做成目录 → read_to_string 失败(IsADirectory);
        // 收集应跳过该级并继续向上收集到更远的 base 级,验证"不中断"。
        let near = base.join("near");
        std::fs::create_dir_all(&near).unwrap();
        // 把 CHIMERA.md 建成目录以触发读取失败
        std::fs::create_dir(near.join(CHIMERA_MD)).unwrap();
        // 更远一级(根方向)成功
        write_rule(&base, AGENTS_MD, "far-rule");

        // from 目录=near;该级 CHIMERA.md 是目录 → 读取失败 → 跳过并警告
        let (rules, warnings) =
            collect_rules_with_warnings(&near, Some(&base)).expect("collect should not error");
        // 即便 near 级失败,仍能收到更远的 base 级规则
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].dir, base);
        assert_eq!(rules[0].content, "far-rule");
        // 且产生了一条警告,未导致 Err
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].dir, near);

        cleanup(&base);
    }

    #[test]
    fn merge_text_keeps_near_last() {
        let base = unique_temp_dir("merge");
        let near = base.join("sub");
        std::fs::create_dir_all(&near).unwrap();
        write_rule(&base, AGENTS_MD, "far");
        write_rule(&near, CHIMERA_MD, "near-rule");

        let rules = collect_rules(&near, Some(&base)).unwrap();
        let text = merge_rules_text(&rules);
        // 根→近:far 在前,near-rule 在后(就近覆盖语义)
        let far_pos = text.find("far").expect("far present");
        let near_pos = text.find("near-rule").expect("near present");
        assert!(far_pos < near_pos);

        cleanup(&base);
    }
}
