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
    tools: HashMap<String, (Arc<dyn ToolProvider>, ToolDescriptor)>,
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
            self.tools
                .insert(descriptor.name.clone(), (Arc::clone(&provider), descriptor));
        }
        Ok(())
    }

    /// Resolve a tool name to its provider + descriptor.
    #[must_use]
    pub fn resolve(&self, tool: &str) -> Option<(Arc<dyn ToolProvider>, ToolDescriptor)> {
        self.tools.get(tool).cloned()
    }

    /// All descriptors, sorted by name for deterministic listing/tests.
    #[must_use]
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        let mut out: Vec<ToolDescriptor> = self
            .tools
            .values()
            .map(|(_, descriptor)| descriptor.clone())
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
