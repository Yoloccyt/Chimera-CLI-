//! SharedSemanticIndex 跨层共享语义索引 — GLM IndexShare 迁移(v4.0 WI-12)
//!
//! 对应任务:P2-T4(v4.0 WI-12,GLM-5.3 IndexShare 理念迁移)
//! 对应架构层:L2 Memory(hcw-window 内提供 + 供 mlc-engine 消费,P2-T6 RSB 联动
//! 预留接口;本任务只建索引,不接 RSB)
//!
//! # 核心职责
//! 三类跨层共享索引(符号 Symbol / 决策 Decision / 错误 Error),各域独立
//! DashMap 分片锁并发安全:
//! - `lookup(domain, key) -> Option<SemanticEntry>`:读索引(压缩 Collapse 级的
//!   语义聚类直接读索引,避免重复计算——IndexShare 语义)
//! - `insert(domain, entry)`:写索引(幂等:同域同 key 覆盖为最新值)
//! - `entries(domain)`:域内全量(供批量消费)
//!
//! # 设计决策(WHY)
//! - **三域独立 DashMap**:Symbol/Decision/Error 三类语义互不干扰(域隔离),
//!   DashMap 分片锁提供高并发读(多线程压缩/检索共享同一索引,零全局锁)。
//! - **值语义返回**:`lookup` 返回克隆的 `SemanticEntry`(值),避免跨线程引用
//!   泄漏(Ref 借用不能跨 await),消费方无需持有锁引用。
//! - **meta: u64 数值元数据**:Collapse 级把「合并后 token 数」存入 `meta`,
//!   载荷 `payload` 存合并内容 → 索引复用无需解析分隔符(避免拼接格式脆弱)。
//! - **索引即缓存**:索引内容是派生数据,可随时由调用方重算覆盖(无跨层
//!   一致性承诺;键建议按会话命名空间隔离,生产接线由 P2-T6 负责)。

use dashmap::DashMap;

/// 语义域 — 三类跨层共享索引
///
/// WHY 三域分立:符号(跨模块 API/类型)、决策(已做过的取舍)、错误(高频错误码)
/// 三类语义检索模式不同,合并在同一 Map 会互相污染。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemanticDomain {
    /// 符号域:符号/API/类型名等静态语义
    Symbol,
    /// 决策域:已做出的决策/取舍摘要
    Decision,
    /// 错误域:错误码/常见错误模式
    Error,
}

impl SemanticDomain {
    /// 域名字符串(日志与调试)
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Symbol => "symbol",
            Self::Decision => "decision",
            Self::Error => "error",
        }
    }
}

/// 索引条目 — 域内一个键对应的语义载荷
///
/// # 字段语义
/// - `key`:域内键(符号名 / 决策主题 / 错误码),即 DashMap 的键
/// - `domain`:所属域(与插入时指定域一致,防御性冗余)
/// - `payload`:语义载荷字符串(Collapse 合并内容 / 决策摘要 / 错误模式)
/// - `meta`:数值元数据(Collapse 合并后 token 数 / 决策版本 / 错误频次)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticEntry {
    /// 域内键
    pub key: String,
    /// 所属域
    pub domain: SemanticDomain,
    /// 语义载荷
    pub payload: String,
    /// 数值元数据
    pub meta: u64,
}

impl SemanticEntry {
    /// 创建索引条目
    #[must_use]
    pub fn new(
        key: impl Into<String>,
        domain: SemanticDomain,
        payload: impl Into<String>,
        meta: u64,
    ) -> Self {
        Self {
            key: key.into(),
            domain,
            payload: payload.into(),
            meta,
        }
    }
}

/// SharedSemanticIndex — 三域跨层共享语义索引
///
/// 线程安全(DashMap 分片锁),跨线程共享使用 `Arc<SharedSemanticIndex>`。
pub struct SharedSemanticIndex {
    symbols: DashMap<String, SemanticEntry>,
    decisions: DashMap<String, SemanticEntry>,
    errors: DashMap<String, SemanticEntry>,
}

impl SharedSemanticIndex {
    /// 创建空索引
    #[must_use]
    pub fn new() -> Self {
        Self {
            symbols: DashMap::new(),
            decisions: DashMap::new(),
            errors: DashMap::new(),
        }
    }

    /// 按域取 Map(内部路由,域隔离的单一入口)
    fn map(&self, domain: SemanticDomain) -> &DashMap<String, SemanticEntry> {
        match domain {
            SemanticDomain::Symbol => &self.symbols,
            SemanticDomain::Decision => &self.decisions,
            SemanticDomain::Error => &self.errors,
        }
    }

    /// 插入条目 — 幂等:同域同 key 覆盖为最新值,返回被覆盖的旧值(如有)
    ///
    /// WHY 幂等:跨层重复计算同一 key 时以最新写为准,不累积脏数据。
    pub fn insert(&self, domain: SemanticDomain, entry: SemanticEntry) -> Option<SemanticEntry> {
        self.map(domain).insert(entry.key.clone(), entry)
    }

    /// 查找条目 — 命中返回克隆值,未命中返回 None
    ///
    /// WHY 返回值而非引用:引用不能跨线程传递,克隆(短载荷)成本可忽略。
    #[must_use]
    pub fn lookup(&self, domain: SemanticDomain, key: &str) -> Option<SemanticEntry> {
        self.map(domain).get(key).map(|r| r.value().clone())
    }

    /// 删除条目 — 返回被删除的旧值(如有)
    pub fn remove(&self, domain: SemanticDomain, key: &str) -> Option<SemanticEntry> {
        self.map(domain).remove(key).map(|(_k, v)| v)
    }

    /// 判断键是否存在
    #[must_use]
    pub fn contains(&self, domain: SemanticDomain, key: &str) -> bool {
        self.map(domain).contains_key(key)
    }

    /// 指定域条目数
    #[must_use]
    pub fn len(&self, domain: SemanticDomain) -> usize {
        self.map(domain).len()
    }

    /// 指定域是否为空
    #[must_use]
    pub fn is_empty(&self, domain: SemanticDomain) -> bool {
        self.map(domain).is_empty()
    }

    /// 指定域内全部条目(顺序不保证,调用方不应依赖顺序)
    #[must_use]
    pub fn entries(&self, domain: SemanticDomain) -> Vec<SemanticEntry> {
        self.map(domain).iter().map(|r| r.value().clone()).collect()
    }

    /// 三域总条目数
    #[must_use]
    pub fn total_len(&self) -> usize {
        self.symbols.len() + self.decisions.len() + self.errors.len()
    }
}

impl Default for SharedSemanticIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn entry(key: &str, domain: SemanticDomain, payload: &str, meta: u64) -> SemanticEntry {
        SemanticEntry::new(key, domain, payload, meta)
    }

    #[test]
    fn test_insert_lookup_roundtrip() {
        // 写读往返:payload 与 meta 一致
        let idx = SharedSemanticIndex::new();
        idx.insert(
            SemanticDomain::Symbol,
            entry("File::parse", SemanticDomain::Symbol, "解析入口", 42),
        );
        let got = idx.lookup(SemanticDomain::Symbol, "File::parse");
        assert_eq!(
            got,
            Some(SemanticEntry::new(
                "File::parse",
                SemanticDomain::Symbol,
                "解析入口",
                42
            ))
        );
    }

    #[test]
    fn test_lookup_miss_returns_none() {
        // 未命中返回 None
        let idx = SharedSemanticIndex::new();
        assert_eq!(idx.lookup(SemanticDomain::Symbol, "missing"), None);
    }

    #[test]
    fn test_insert_idempotent_same_key_overwrites() {
        // 幂等:同域同 key 覆盖为最新值,条目数不增长
        let idx = SharedSemanticIndex::new();
        let old = idx.insert(
            SemanticDomain::Decision,
            entry("压缩级别", SemanticDomain::Decision, "v1", 1),
        );
        assert_eq!(old, None);
        let old = idx.insert(
            SemanticDomain::Decision,
            entry("压缩级别", SemanticDomain::Decision, "v2", 2),
        );
        assert_eq!(
            old,
            Some(SemanticEntry::new(
                "压缩级别",
                SemanticDomain::Decision,
                "v1",
                1
            ))
        );
        assert_eq!(
            idx.len(SemanticDomain::Decision),
            1,
            "同 key 覆盖后条目数不变"
        );
        assert_eq!(
            idx.lookup(SemanticDomain::Decision, "压缩级别")
                .map(|e| e.meta),
            Some(2),
            "应为最新值"
        );
    }

    #[test]
    fn test_domain_isolation() {
        // 域隔离:同 key 三个域互不干扰
        let idx = SharedSemanticIndex::new();
        idx.insert(
            SemanticDomain::Symbol,
            entry("k", SemanticDomain::Symbol, "sym", 1),
        );
        idx.insert(
            SemanticDomain::Decision,
            entry("k", SemanticDomain::Decision, "dec", 2),
        );
        idx.insert(
            SemanticDomain::Error,
            entry("k", SemanticDomain::Error, "err", 3),
        );
        assert_eq!(idx.len(SemanticDomain::Symbol), 1);
        assert_eq!(idx.len(SemanticDomain::Decision), 1);
        assert_eq!(idx.len(SemanticDomain::Error), 1);
        assert_eq!(idx.total_len(), 3);
        // 删除 Symbol 域不影响其他两域
        let removed = idx.remove(SemanticDomain::Symbol, "k");
        assert!(removed.is_some());
        assert_eq!(idx.lookup(SemanticDomain::Symbol, "k"), None);
        assert!(idx.lookup(SemanticDomain::Decision, "k").is_some());
        assert!(idx.lookup(SemanticDomain::Error, "k").is_some());
        assert_eq!(idx.total_len(), 2);
    }

    #[test]
    fn test_entries_scoped_to_domain() {
        // entries 只返回指定域
        let idx = SharedSemanticIndex::new();
        idx.insert(
            SemanticDomain::Symbol,
            entry("a", SemanticDomain::Symbol, "A", 1),
        );
        idx.insert(
            SemanticDomain::Symbol,
            entry("b", SemanticDomain::Symbol, "B", 2),
        );
        idx.insert(
            SemanticDomain::Error,
            entry("c", SemanticDomain::Error, "C", 3),
        );
        let symbols = idx.entries(SemanticDomain::Symbol);
        assert_eq!(symbols.len(), 2);
        for e in &symbols {
            assert_eq!(e.domain, SemanticDomain::Symbol);
        }
        assert_eq!(idx.entries(SemanticDomain::Error).len(), 1);
        assert_eq!(idx.entries(SemanticDomain::Decision).len(), 0);
    }

    #[test]
    fn test_contains_and_remove() {
        // contains 判定 + remove 返回旧值
        let idx = SharedSemanticIndex::new();
        idx.insert(
            SemanticDomain::Error,
            entry("E-1001", SemanticDomain::Error, "超时", 5),
        );
        assert!(idx.contains(SemanticDomain::Error, "E-1001"));
        assert!(!idx.contains(SemanticDomain::Symbol, "E-1001"));
        let old = idx.remove(SemanticDomain::Error, "E-1001");
        assert_eq!(old.map(|e| e.meta), Some(5));
        assert!(!idx.contains(SemanticDomain::Error, "E-1001"));
    }

    #[test]
    fn test_concurrent_access_safety() {
        // 并发安全:8 线程 × 500 次跨域插入,总数一致且无 panic(DashMap 分片锁)
        let idx = Arc::new(SharedSemanticIndex::new());
        let mut handles = Vec::new();
        for t in 0..8 {
            let idx = Arc::clone(&idx);
            handles.push(std::thread::spawn(move || {
                for i in 0..500 {
                    let domain = match t % 3 {
                        0 => SemanticDomain::Symbol,
                        1 => SemanticDomain::Decision,
                        _ => SemanticDomain::Error,
                    };
                    idx.insert(
                        domain,
                        entry(&format!("t{t}-i{i}"), domain, "payload", i as u64),
                    );
                    // 混合读(并发读路径)
                    let _ = idx.lookup(domain, &format!("t{t}-i{i}"));
                }
            }));
        }
        for h in handles {
            h.join().expect("线程 join 失败");
        }
        assert_eq!(idx.total_len(), 8 * 500, "并发插入总数必须一致(无丢失)");
        // 各域互不串扰
        assert!(idx.len(SemanticDomain::Symbol) > 0);
        assert!(idx.len(SemanticDomain::Decision) > 0);
        assert!(idx.len(SemanticDomain::Error) > 0);
    }

    #[test]
    fn test_domain_as_str() {
        assert_eq!(SemanticDomain::Symbol.as_str(), "symbol");
        assert_eq!(SemanticDomain::Decision.as_str(), "decision");
        assert_eq!(SemanticDomain::Error.as_str(), "error");
    }
}
