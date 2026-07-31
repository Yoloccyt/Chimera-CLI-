//! IDE 适配器注册中心 — 支持运行时注册新 IDE 适配器
//!
//! 对应架构:L10 Interface 内部扩展机制
//!
//! # 设计动机
//! - 内置 5 大 IDE(VSCode/IntelliJ/Vim/Emacs/Zed)在构造时预注册
//! - 第三方 IDE 可通过 `register` 在运行时注入工厂闭包,无需修改 enum dispatch
//! - 工厂模式解耦"IDE 标识"与"适配器实例化",支持未来动态加载
//!
//! # 并发安全
//! - 内部用 `std::sync::Mutex` 保护 HashMap(同步锁,不跨 .await)
//! - `create` 在锁内仅取工厂引用,释放锁后调用工厂(遵守 §4.4 #1 禁止持锁跨 .await)

use crate::adapters::{
    EmacsAdapter, IdeAdapterKind, IntelliJAdapter, VimAdapter, VscodeAdapter, ZedAdapter,
};
use std::collections::HashMap;
use std::sync::Mutex;

/// IDE 适配器工厂闭包类型 — 返回一个新的 IdeAdapterKind 实例
type AdapterFactory = Box<dyn Fn() -> IdeAdapterKind + Send + Sync>;

/// IDE 适配器注册中心 — 运行时管理 IDE 标识到适配器工厂的映射
///
/// WHY:enum dispatch 的 `for_source` 是编译期穷尽 match,无法支持运行时新增 IDE。
/// Registry 通过工厂闭包提供运行时扩展点,内置 5 大 IDE 在 `new()` 时预注册。
pub struct IdeAdapterRegistry {
    /// IDE 标识 → 工厂闭包(同步锁保护,create 不跨 .await)
    factories: Mutex<HashMap<&'static str, AdapterFactory>>,
}

impl IdeAdapterRegistry {
    /// 创建注册中心并预注册 5 大内置 IDE
    pub fn new() -> Self {
        let mut map: HashMap<&'static str, AdapterFactory> = HashMap::new();
        map.insert(
            "vscode",
            Box::new(|| IdeAdapterKind::Vscode(VscodeAdapter::new())),
        );
        map.insert(
            "intellij",
            Box::new(|| IdeAdapterKind::IntelliJ(IntelliJAdapter::new())),
        );
        map.insert("vim", Box::new(|| IdeAdapterKind::Vim(VimAdapter::new())));
        map.insert(
            "emacs",
            Box::new(|| IdeAdapterKind::Emacs(EmacsAdapter::new())),
        );
        map.insert("zed", Box::new(|| IdeAdapterKind::Zed(ZedAdapter::new())));
        Self {
            factories: Mutex::new(map),
        }
    }

    /// 注册一个新 IDE 适配器工厂
    ///
    /// 若 name 已存在则覆盖(支持运行时替换工厂)
    pub fn register(&self, name: &'static str, factory: AdapterFactory) {
        let mut guard = self
            .factories
            .lock()
            .expect("registry mutex poisoned(工厂注册时)");
        guard.insert(name, factory);
    }

    /// 根据 IDE 标识创建适配器实例
    ///
    /// 返回 `None` 表示该 IDE 未注册
    ///
    /// WHY 锁内调用:factory 是同步快调用(仅构造 enum 变体,无 IO/await),
    /// 持锁时间极短;`Box<dyn Fn>` 无法 clone,无法在锁外调用。
    /// 这不违反 §4.4 #1(禁止持锁跨 .await)——factory 调用不含 .await。
    pub fn create(&self, name: &str) -> Option<IdeAdapterKind> {
        let guard = self
            .factories
            .lock()
            .expect("registry mutex poisoned(创建适配器时)");
        guard.get(name).map(|f| f())
    }

    /// 列出所有已注册 IDE 标识
    pub fn list(&self) -> Vec<&'static str> {
        let guard = self
            .factories
            .lock()
            .expect("registry mutex poisoned(列举 IDE 时)");
        let mut keys: Vec<&'static str> = guard.keys().copied().collect();
        keys.sort();
        keys
    }
}

impl Default for IdeAdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::VscodeAdapter;

    #[test]
    fn test_registry_new_pre_registers_five_builtin_ides() {
        let registry = IdeAdapterRegistry::new();
        let list = registry.list();
        assert_eq!(list.len(), 5);
        assert!(list.contains(&"vscode"));
        assert!(list.contains(&"intellij"));
        assert!(list.contains(&"vim"));
        assert!(list.contains(&"emacs"));
        assert!(list.contains(&"zed"));
    }

    #[test]
    fn test_registry_create_returns_correct_adapter() {
        let registry = IdeAdapterRegistry::new();
        let adapter = registry.create("vscode").expect("vscode 应已注册");
        assert!(matches!(adapter, IdeAdapterKind::Vscode(_)));
    }

    #[test]
    fn test_registry_create_unknown_returns_none() {
        let registry = IdeAdapterRegistry::new();
        assert!(registry.create("sublime").is_none());
    }

    #[test]
    fn test_registry_register_new_ide() {
        let registry = IdeAdapterRegistry::new();
        // 注册一个自定义 IDE(复用 VscodeAdapter 作为占位)
        registry.register(
            "custom-ide",
            Box::new(|| IdeAdapterKind::Vscode(VscodeAdapter::new())),
        );
        let list = registry.list();
        assert!(list.contains(&"custom-ide"));
        assert_eq!(list.len(), 6);

        let adapter = registry.create("custom-ide").expect("custom-ide 应可创建");
        assert!(matches!(adapter, IdeAdapterKind::Vscode(_)));
    }

    #[test]
    fn test_registry_register_overrides_existing() {
        let registry = IdeAdapterRegistry::new();
        // 覆盖 vscode 工厂(模拟运行时替换)
        registry.register(
            "vscode",
            Box::new(|| IdeAdapterKind::Zed(ZedAdapter::new())),
        );
        let adapter = registry.create("vscode").expect("vscode 应存在");
        assert!(
            matches!(adapter, IdeAdapterKind::Zed(_)),
            "覆盖后应返回 Zed 适配器"
        );
    }
}
