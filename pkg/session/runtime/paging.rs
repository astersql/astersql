// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Session paging context at the existing concrete cop-request boundary.

use super::*;
use astersql_store_driver::resource_group_runtime::ResourceGroupRuntimeStates;

struct ControllerRuntimeStates(Arc<ResourceGroupRuntimeStates>);

impl astersql_domain::resource_group_runtime::ResourceGroupRuntimeStateProvider
    for ControllerRuntimeStates
{
    fn has_limited_burst(&self, name: &str) -> Option<bool> {
        self.0
            .get_resource_group_runtime_state(name)
            .map(|state| state.has_limited_burst)
    }
}

/// Bind the state view updated from actual PD token responses to its Domain.
/// Passing None removes the controller, so subsequent statements use metadata.
#[allow(non_snake_case)]
pub fn SetResourceGroupRuntimeStates(
    domain: &Arc<Domain>,
    states: Option<Arc<ResourceGroupRuntimeStates>>,
) {
    domain.set_resource_group_runtime_states(states.map(|states| {
        Arc::new(ControllerRuntimeStates(states))
            as Arc<dyn astersql_domain::resource_group_runtime::ResourceGroupRuntimeStateProvider>
    }));
}

pub(super) fn resource_group_allows_paging_size_bytes(
    domain: Option<&Arc<Domain>>,
    name: &str,
) -> bool {
    let Some(domain) = domain.filter(|_| !name.is_empty()) else {
        return false;
    };
    if let Some(has_limited_burst) = domain.resource_group_has_limited_burst(name) {
        return has_limited_burst;
    }
    // This is the metadata owner used by this Rust runtime's real SQL CREATE,
    // ALTER, DROP and information_schema.resource_groups paths.
    RUNTIME_RESOURCE_GROUPS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&runtime_domain_id(domain))
        .and_then(|groups| groups.get(&name.to_lowercase()))
        .is_some_and(|group| {
            astersql_meta_model::group_3::ResourceGroupSettings {
                RURate: group.ru_per_sec,
                BurstLimit: group.burst_limit,
                ..Default::default()
            }
            .GetBurstLimitAdjusted()
                >= 0
        })
}

pub(super) fn effective_paging_size_bytes(
    domain: Option<&Arc<Domain>>,
    name: &str,
    budget: i64,
    enabled: bool,
) -> i64 {
    if budget > 0 && (!enabled || !resource_group_allows_paging_size_bytes(domain, name)) {
        0
    } else {
        budget
    }
}

#[derive(Clone, Copy)]
struct PagingByteBudget(u64);

impl ConcreteSession {
    /// Capture only this commit's byte budget in the existing statement cache.
    /// Existing requests retain their captured budget across global changes.
    pub(super) fn cop_paging_size_bytes(&self, name: &str) -> u64 {
        let cached = self.session_vars.StmtCtx.GetOrInitDistSQLFromCache(|| {
            let budget = astersql_sessionctx_vardef::PagingSizeBytes.Load();
            let enabled = self
                .domain
                .global_system_variable("tidb_enable_resource_control")
                .map_or_else(
                    || astersql_sessionctx_vardef::EnableResourceControl.Load(),
                    |value| variable_is_on(&value),
                );
            astersql_sessionctx_stmtctx::cache_value(PagingByteBudget(
                effective_paging_size_bytes(Some(&self.domain), name, budget, enabled).max(0)
                    as u64,
            ))
        });
        astersql_sessionctx_stmtctx::cache_downcast_ref::<PagingByteBudget>(&cached)
            .expect("session DistSQL paging byte budget cache type")
            .0
    }
}
