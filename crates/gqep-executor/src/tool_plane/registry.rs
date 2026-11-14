//! Tool registry — name resolution and duplicate rejection.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::types::{ToolDescriptor, ToolError, ToolProvider};

/// Immutable-ish registry populated at composition time.
///
/// WHY no `Debug`: `Arc<dyn ToolProvider>` is not Debug; printing the registry
/// would require a provider-specific dump that adds no diagnostic value.
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, (Arc<dyn ToolProvider>, Arc<ToolDescriptor>)>,
}

impl ToolRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register all tools of one provider atomically.
    ///
    /// Registration is all-or-nothing: any duplicate, provider-id mismatch or
    /// empty descriptor list rejects the whole provider and leaves the registry
    /// unchanged. This prevents half-registered providers that fail at call time.
    pub fn register(&mut self, provider: Arc<dyn ToolProvider>) -> Result<(), ToolError> {
        let provider_id = provider.provider_id();
        if provider_id.is_empty() {
            return Err(ToolError::InvalidArgs("provider id is empty".into()));
        }
        let descriptors = provider.descriptors();
        if descriptors.is_empty() {
            return Err(ToolError::InvalidArgs(format!(
                "provider {provider_id} exposes no tools"
            )));
        }

        let mut batch_names = HashSet::with_capacity(descriptors.len());
        for descriptor in &descriptors {
            if descriptor.name.is_empty() {
                return Err(ToolError::InvalidArgs(format!(
                    "provider {provider_id} has an empty tool name"
                )));
            }
            if descriptor.provider != provider_id {
                return Err(ToolError::InvalidArgs(format!(
                    "tool {} declares provider {} but provider id is {provider_id}",
                    descriptor.name, descriptor.provider
                )));
            }
            if self.tools.contains_key(&descriptor.name) || !batch_names.insert(&descriptor.name) {
                return Err(ToolError::InvalidArgs(format!(
                    "duplicate tool name: {}",
                    descriptor.name
                )));
            }
        }

        for descriptor in descriptors {
            let name = descriptor.name.clone();
            self.tools
                .insert(name, (Arc::clone(&provider), Arc::new(descriptor)));
        }
        Ok(())
    }

    /// Resolve a tool name to its provider + descriptor.
    ///
    /// WHY the descriptor is shared behind an `Arc` rather than returned by
    /// value: `resolve` sits on every tool call's hot path, and `ToolDescriptor`
    /// owns four `String`s — a by-value return allocated all four again per
    /// call (measured `llm_diag_resolve_hit` 134.29 ns vs a `miss` at 14.72 ns,
    /// i.e. ~120 ns of pure allocator work per call). Registration is composition
    /// time, so moving the copy there costs nothing on the hot path.
    /// `resolve` 返回的 `Arc` 一律指向 `register` 时那一个描述符，永不重建。
    #[must_use]
    pub fn resolve(&self, tool: &str) -> Option<(Arc<dyn ToolProvider>, Arc<ToolDescriptor>)> {
        self.tools.get(tool).cloned()
    }

    /// All descriptors, sorted by name for deterministic listing/tests.
    ///
    /// Deliberately still owned: this is the cold listing path (`chimera tool
    /// list`, tests, benches), and handing out `Arc` here would push the
    /// sharing decision onto every caller that only wants to print a table.
    #[must_use]
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        let mut out: Vec<ToolDescriptor> = self
            .tools
            .values()
            .map(|(_, descriptor)| descriptor.as_ref().clone())
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Number of registered tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}
