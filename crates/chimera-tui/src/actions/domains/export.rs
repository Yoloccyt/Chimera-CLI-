//! 数据导出域动作(ADR-029,v3.1)
//!
//! 对应架构层:L10 Interface

use crate::actions::descriptor::{ActionDescriptor, ActionDomain};

/// 返回 Export 域的全部动作描述
///
/// WHY 单一 export.run:导出格式(Csv/Json/Markdown)与目标(File/Clipboard/Pipe)
/// 作为 payload 参数由导出弹窗采集,而非拆成 9 个 Action——避免 Registry 膨胀,
/// 保持"一个功能一个 Action"的粒度纪律(§4.2 熔断预防)。
pub fn descriptors() -> Vec<ActionDescriptor> {
    vec![ActionDescriptor {
        is_core: true,
        // Ctrl+E 唤起导出弹窗(§4.6 统一交互语法)
        default_key: Some("Ctrl+E"),
        // Concord T1.3:'E' 为历史兼容别名(路由表曾硬编码 Shift+E 同达本动作),
        // 收进声明后由 codegen 统一派生,INV-K-B 不变量据此认可别名路由。
        alias_keys: &["E"],
        ..ActionDescriptor::new(
            "export.run",
            ActionDomain::Export,
            "action.export.run",
            Some("export"),
        )
    }]
}
