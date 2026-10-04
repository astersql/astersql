// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Starter-only lookup fallback options, corresponding to the Go controller options.
use std::time::Duration;
use tikv_client::{proto::resource_manager as rm, resource_group_lookup::CreateOption};

pub const DEFAULT_DEGRADED_RU_FILL_RATE: u64 = 2_000_000;
pub const DEFAULT_DEGRADED_RU_BURST_LIMIT: i64 = 50_000_000_000;
pub const DEFAULT_DEGRADED_MODE_WAIT_TIMEOUT: Duration = Duration::from_millis(1500);
pub const TOKEN_WAIT_RETRY_INTERVAL: Duration = Duration::from_millis(100);
pub const TOKEN_WAIT_RETRY_TIMES: u32 = 20;

pub fn new_default_degraded_ru_settings() -> rm::GroupRequestUnitSettings {
    rm::GroupRequestUnitSettings {
        r_u: Some(rm::TokenBucket {
            settings: Some(rm::TokenLimitSettings {
                fill_rate: DEFAULT_DEGRADED_RU_FILL_RATE,
                burst_limit: DEFAULT_DEGRADED_RU_BURST_LIMIT,
                ..Default::default()
            }),
            ..Default::default()
        }),
    }
}
/// The entry adapter passes its process deploy-mode decision explicitly.
pub fn new_resource_groups_controller_options(
    is_starter: bool,
    enable_fallback: bool,
) -> Vec<CreateOption> {
    let mut options = vec![CreateOption::MaxWaitDuration(Duration::from_millis(
        astersql_resourcegroup_runaway::manager::MaxWaitDurationMillis,
    ))];
    if is_starter && enable_fallback {
        options.extend([
            CreateOption::DegradedRuSettings(new_default_degraded_ru_settings()),
            CreateOption::DegradedModeWaitDuration(DEFAULT_DEGRADED_MODE_WAIT_TIMEOUT),
            CreateOption::WaitRetryInterval(TOKEN_WAIT_RETRY_INTERVAL),
            CreateOption::WaitRetryTimes(TOKEN_WAIT_RETRY_TIMES),
        ]);
    }
    options
}
