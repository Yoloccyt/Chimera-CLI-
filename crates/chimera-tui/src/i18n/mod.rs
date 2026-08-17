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

// WHY 非 cfg(test):LOCALE_LOCK_HELD(thread_local)在非 test 编译下
// (集成测试调用 lib)也存在,Cell 需全编译模式可见。
use std::cell::Cell;
use std::sync::atomic::{AtomicU8, Ordering};
// WHY 非 cfg(test):LOCALE_TEST_LOCK 与 LocaleTestGuard 已对集成测试公开
// (lib 以 non-test 模式编译),Mutex/MutexGuard 需在非 test 编译下可见。
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
/// 2026-08-17 统一加锁:原 cfg(test) 分支导致集成测试(tests/,lib 以 non-test
/// 模式编译)的 set_locale 走无锁路径,integration 测试之间互相覆盖 locale
/// (layout 面板标题断言并行必败实证)。现统一:非持有者经 `LOCALE_TEST_LOCK`
/// 串行化;持有者(guard 同线程)经 `LOCALE_LOCK_HELD` 重入检测直接写。
/// 生产路径仅 main 启动调用一次,锁开销可忽略。
pub fn set_locale(locale: Locale) {
    // 非持有者加锁串行化写;持有者(guard 测试)重入直接写
    if !LOCALE_LOCK_HELD.with(|h| h.get()) {
        let _g = LOCALE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
///
/// WHY 非 cfg(test)(2026-08-17):集成测试(tests/)编译时 lib 为 non-test 模式,
/// cfg(test) 项不可见;而集成测试与单测共享同一全局 locale,必须用同一把锁
/// 才能互斥。锁在生产路径仅存在未使用(无调用,零开销)。
#[doc(hidden)]
pub(crate) static LOCALE_TEST_LOCK: Mutex<()> = Mutex::new(());

// 当前线程是否持有 locale 测试锁(重入检测,防 guard 内 set_locale 自锁)
// 注:普通注释而非 /// —— rustdoc 不为宏调用生成文档,/// 会触发 clippy unused_doc_comments。
// WHY allow: 初始化已是 Rust 1.88+ 推荐 const 块写法,clippy 1.97 的
// missing_const_for_thread_local 对该宏语法仍误报(can be made const);
// allow 必须位于 cfg(test) 之前,否则宏展开后 lint 抑制失效。
// WHY 非 cfg(test)(2026-08-17):set_locale 统一加锁后,非 test 编译
// (集成测试调用 lib)也需重入检测,thread_local 需全编译模式可见。
thread_local! {
    static LOCALE_LOCK_HELD: Cell<bool> = const { Cell::new(false) };
}

/// 测试专用 locale 序列化 guard — 绑定到局部变量(如 `let _g = ...`)以在整个
/// 测试作用域内持锁;drop 时复位线程持有标记并释放锁。锁中毒时降级取用内部值
/// (测试已 panic 失败,不因中毒再连锁 panic 掩盖真实失败)。
///
/// WHY pub + 非 cfg(test)(2026-08-17):集成测试(tests/)与单元测试共享同一
/// 全局 locale,且 set_locale 的锁在函数返回即释放,En-pin 窗口(set→渲染→
/// 恢复)不防其他测试插入写;guard 全程持锁是窗口安全的唯一途径。集成测试
/// 编译时 lib 为 non-test 模式,故 guard 不能 cfg(test) 门控。
#[doc(hidden)]
pub struct LocaleTestGuard {
    _guard: MutexGuard<'static, ()>,
    // WHY 记录进入时 locale:drop 时恢复,防止 guard 测试修改后残留污染
    // 后续依赖默认状态的测试(2026-08-17 overwindow 空态 flaky 甄别)。
    previous: Locale,
}

// WHY 非 cfg(test)(2026-08-17):集成测试(lib non-test 编译)的 guard 也需
// 设置/重置重入标记,否则 guard 内 set_locale 会自锁(HELD=false → 抢同一把锁)。
impl Drop for LocaleTestGuard {
    fn drop(&mut self) {
        LOCALE_LOCK_HELD.with(|h| h.set(false));
        // 恢复进入时 locale(RAII 完整语义),防残留污染后续测试
        LOCALE.store(self.previous.as_u8(), Ordering::Relaxed);
    }
}

#[doc(hidden)]
pub fn locale_test_guard() -> LocaleTestGuard {
    LOCALE_LOCK_HELD.with(|h| h.set(true));
    let previous = current_locale();
    LocaleTestGuard {
        _guard: LOCALE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
        previous,
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
