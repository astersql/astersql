// Copyright 2026 AsterSQL.

//! Runtime signals supplied by the Domain's resource-group controller.
//! `None` means no usable local state (including before the first token response).

pub trait ResourceGroupRuntimeStateProvider: Send + Sync {
    fn has_limited_burst(&self, resource_group_name: &str) -> Option<bool>;
}
