//! i18n — TUI 国际化资源与运行时中英切换(ADR-029,v3.1)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **默认中文**:项目定位中文优先,`LOCALE` 初值为 `Locale::Zh`。
//! - **运行时切换**:全局 `AtomicU8` 存储当前 locale,`Ctrl+L` 触发 `toggle`,
//!   渲染时读取,无需重启。`Relaxed` 序足够——UI 文案读取无跨线程一致性要求,
//!   且 TUI 主循环单线程,不存在竞态。
//! - **编译期静态表 + match**:文案量 < 500 条,用 `match` 静态映射而非引入
//!   fluent/gettext 外部依赖(避免二进制体积增长与 cargo audit 面扩大)。
//! - **key 命名 `模块.组件.用途`**:如 `panel.quest.title`,便于分层管理与查漏。
//! - **缺失回退**:未命中 key 时返回 key 本身(而非 panic),保证渐进接入期
//!   即使漏译也不崩溃,key 本身即可读的占位提示,便于开发期发现缺失文案。
//!
//! # 使用
//! ```
//! use chimera_tui::t;
//! let title = t!("panel.quest.title"); // 按当前 locale 返回译文
//! ```

// WHY allow(clippy 1.97 误报): 模块内 thread_local 初始化已用 Rust 1.88+ 推荐
// const 块写法,但 missing_const_for_thread_local 对 thread_local! 宏展开内部仍报
// "can be made const",且调用点 allow 无法抑制宏内部 lint(实测 unused attribute)。
#![allow(clippy::missing_const_for_thread_local)]

#[cfg(test)]
use std::cell::Cell;
use std::sync::atomic::{AtomicU8, Ordering};
#[cfg(test)]
use std::sync::{Mutex, MutexGuard};

pub mod en;
pub mod zh;

/// 界面语言 — 中文(默认)/ 英文
///
/// WHY `u8` 可表示:配合全局 `AtomicU8` 做无锁运行时切换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Locale {
    /// 简体中文(默认)
    Zh,
    /// 英文
    En,
}

impl Locale {
    /// 转为原子存储用的 `u8` 判别值
    fn as_u8(self) -> u8 {
        match self {
            Locale::Zh => 0,
            Locale::En => 1,
        }
    }

    /// 从原子存储的 `u8` 还原;非法值回退到默认中文(防御边界)
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Locale::En,
            // 0 与任何非法值都回退中文默认,保证 UI 永不因坏值 panic
            _ => Locale::Zh,
        }
    }

    /// 返回切换后的另一种语言(中 ↔ 英),用于 `Ctrl+L` 循环
    pub fn toggled(self) -> Self {
        match self {
            Locale::Zh => Locale::En,
            Locale::En => Locale::Zh,
        }
    }

    /// 返回该语言的短标识(状态栏展示用,如 "中" / "EN")
    pub fn short_label(self) -> &'static str {
        match self {
            Locale::Zh => "中",
            Locale::En => "EN",
        }
    }
}

/// 全局当前 locale — 单一事实源,默认中文(`Locale::Zh` == 0)
///
/// WHY 全局静态:UI 文案在渲染各层高频读取,通过参数层层传递 locale 会污染
/// 大量函数签名;全局原子读取零成本且线程安全,契合 TUI 单线程渲染模型。
static LOCALE: AtomicU8 = AtomicU8::new(0);

/// 读取当前界面语言
pub fn current_locale() -> Locale {
    Locale::from_u8(LOCALE.load(Ordering::Relaxed))
}

/// 设置界面语言(运行时切换)
///
/// 2026-08-07 测试互斥修复:并行测试直接写全局 `LOCALE` 会污染依赖中文文案的
/// 断言(overwindow 空态 flaky 根因 —— 既有 guard 未覆盖非 guard 测试的写路径)。
/// cfg(test) 下:非 guard 路径经 `LOCALE_TEST_LOCK` 串行化;guard 持有者(同线程)
/// 经 `LOCALE_LOCK_HELD` 重入检测直接写,避免自锁。生产构建零开销(块被剥离)。
pub fn set_locale(locale: Locale) {
    #[cfg(test)]
    {
        if !LOCALE_LOCK_HELD.with(|h| h.get()) {
            let _g = LOCALE_TEST_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            LOCALE.store(locale.as_u8(), Ordering::Relaxed);
            return;
        }
    }
    LOCALE.store(locale.as_u8(), Ordering::Relaxed);
}

/// 切换中英并返回切换后的语言(供 `Ctrl+L` 快捷键调用)
pub fn toggle_locale() -> Locale {
    let next = current_locale().toggled();
    set_locale(next);
    next
}

/// 按当前 locale 翻译 key;未命中返回 key 本身作为占位
///
/// WHY 返回 `&'static str`:所有译文均为编译期字面量,零分配;
/// 未命中时返回传入的 `key`(其生命周期由调用方保证,通常也是字面量)。
pub fn tr(key: &'static str) -> &'static str {
    let found = match current_locale() {
        Locale::Zh => zh::lookup(key),
        Locale::En => en::lookup(key),
    };
    found.unwrap_or(key)
}

/// 翻译宏 — `t!("panel.quest.title")` 展开为 `tr(...)`
///
/// WHY 宏而非直接调用:调用点更简洁,且未来可扩展参数插值(如 `t!("x", n)`)
/// 而不改动调用点签名。
#[macro_export]
macro_rules! t {
    ($key:expr) => {
        $crate::i18n::tr($key)
    };
}

/// 测试专用 locale 互斥锁与持有标记 —— 消除并行测试对全局 `LOCALE` 的竞争
///
/// WHY:生产为单线程(仅主线程读写 locale,无竞态);但 `cargo test` 默认多线程并行,
/// 多个测试同时 En-pin(set En → 捕获 → reset Zh)会互相污染窗口。`set_locale` 的
/// cfg(test) 分支与 `locale_test_guard` 共用同一把锁,覆盖**全部**写路径:
/// 非 guard 测试的 set_locale 自动加锁;guard 测试经重入标记直接写。
#[cfg(test)]
static LOCALE_TEST_LOCK: Mutex<()> = Mutex::new(());

// 当前线程是否持有 locale 测试锁(重入检测,防 guard 内 set_locale 自锁)
// 注:普通注释而非 /// —— rustdoc 不为宏调用生成文档,/// 会触发 clippy unused_doc_comments。
// WHY allow: 初始化已是 Rust 1.88+ 推荐 const 块写法,clippy 1.97 的
// missing_const_for_thread_local 对该宏语法仍误报(can be made const);
// allow 必须位于 cfg(test) 之前,否则宏展开后 lint 抑制失效。
#[cfg(test)]
thread_local! {
    static LOCALE_LOCK_HELD: Cell<bool> = const { Cell::new(false) };
}

/// 测试专用 locale 序列化 guard — 绑定到局部变量(如 `let _g = ...`)以在整个
/// 测试作用域内持锁;drop 时复位线程持有标记并释放锁。锁中毒时降级取用内部值
/// (测试已 panic 失败,不因中毒再连锁 panic 掩盖真实失败)。
#[cfg(test)]
pub(crate) struct LocaleTestGuard {
    _guard: MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for LocaleTestGuard {
    fn drop(&mut self) {
        LOCALE_LOCK_HELD.with(|h| h.set(false));
    }
}

#[cfg(test)]
pub(crate) fn locale_test_guard() -> LocaleTestGuard {
    LOCALE_LOCK_HELD.with(|h| h.set(true));
    LocaleTestGuard {
        _guard: LOCALE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个测试后复位 locale,避免全局状态污染其他测试(TUI 单线程,串行足够)
    fn reset() {
        set_locale(Locale::Zh);
    }

    #[test]
    fn default_locale_is_chinese() {
        let _locale_guard = locale_test_guard();
        reset();
        assert_eq!(current_locale(), Locale::Zh);
        assert_eq!(tr("panel.quest.title"), "任务");
    }

    #[test]
    fn toggle_switches_between_zh_and_en() {
        let _locale_guard = locale_test_guard();
        reset();
        assert_eq!(toggle_locale(), Locale::En);
        assert_eq!(tr("panel.quest.title"), "Quests");
        assert_eq!(toggle_locale(), Locale::Zh);
        assert_eq!(tr("panel.quest.title"), "任务");
    }

    #[test]
    fn missing_key_falls_back_to_key_itself() {
        let _locale_guard = locale_test_guard();
        reset();
        // 未定义的 key 返回自身,不 panic
        assert_eq!(tr("nonexistent.key.xyz"), "nonexistent.key.xyz");
    }

    #[test]
    fn zh_and_en_tables_cover_same_seed_keys() {
        // 种子集必须中英对齐:任一表命中的 key,另一表也应命中,避免半译
        for key in zh::SEED_KEYS {
            assert!(zh::lookup(key).is_some(), "zh 表缺失种子 key: {key}");
            assert!(en::lookup(key).is_some(), "en 表缺失种子 key: {key}");
        }
    }
}
