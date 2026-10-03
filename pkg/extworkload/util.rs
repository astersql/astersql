// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Predicates for manager availability and external-workload roles.
//
// 外部工作负载 Manager 可用性与角色判断谓词。
//
// 对应 Go 包级辅助函数：在调用后台作业前先确认 Manager 非空，
// 且当前 TiDB 角色与目标作业匹配（master / gcv2 / ttl / auto-analyze）。

use crate::{Manager, config};

/// 报告调用方是否持有外部工作负载 Manager（对应 Go `m != nil`）。
///
/// Rust 用 `Option<&dyn Manager>` 表达可空接口，避免 Go 的 nil interface 陷阱被隐式吞掉。
// IsEnabled 对应 Go 的 m != nil，报告调用方是否持有外部工作负载 Manager。
// Rust 用 Option<&dyn Manager> 表达可空接口，避免 Go 的 nil interface 陷阱被隐式吞掉。
#[allow(non_snake_case)]
pub fn IsEnabled(manager: Option<&dyn Manager>) -> bool {
    manager.is_some()
}

/// 报告当前 TiDB 是否承担普通主角色（非专职后台 worker）。
// IsMaster 报告当前 TiDB 是否承担普通 TiDB 主角色。
#[allow(non_snake_case)]
pub fn IsMaster(manager: Option<&dyn Manager>) -> bool {
    roleIs(manager, config::RoleMaster.to_owned())
}

/// 报告当前 TiDB 是否为专用 keyspace 级 GC（垃圾回收）worker。
// IsGCV2Worker 报告当前 TiDB 是否为专用 keyspace 级 GC worker。
#[allow(non_snake_case)]
pub fn IsGCV2Worker(manager: Option<&dyn Manager>) -> bool {
    roleIs(manager, config::RoleGCV2Worker.to_owned())
}

/// 报告当前 TiDB 是否应运行 TTL（按存活时间删行）作业。
// IsTTLTaskWorker 报告当前 TiDB 是否应运行 TTL 作业。
#[allow(non_snake_case)]
pub fn IsTTLTaskWorker(manager: Option<&dyn Manager>) -> bool {
    roleIs(manager, config::RoleTTLTaskWorker.to_owned())
}

/// 报告当前 TiDB 是否应运行自动分析（收集统计信息）作业。
// IsAutoAnalyzeWorker 报告当前 TiDB 是否应运行自动分析作业。
#[allow(non_snake_case)]
pub fn IsAutoAnalyzeWorker(manager: Option<&dyn Manager>) -> bool {
    roleIs(manager, config::RoleAutoAnalyzeWorker.to_owned())
}

/// 公共内部判断：仅当 Manager 存在且角色相等时返回 true。
///
/// `if let` 保持 Go `IsEnabled(m) && m.Role() == role` 的短路顺序，不会对空 Manager 调用 Role。
// roleIs 对应 Go 的公共内部判断：仅当 Manager 存在且角色相等时返回 true。
// if let 保持 Go `IsEnabled(m) && m.Role() == role` 的短路顺序，不会对空 Manager 调用 Role。
#[allow(non_snake_case)]
fn roleIs(manager: Option<&dyn Manager>, role: config::ExternalWorkloadRole) -> bool {
    if let Some(manager) = manager {
        manager.Role() == role
    } else {
        false
    }
}

/// Abort outstanding work only for a dedicated GCV2 worker before upgrade.
#[allow(non_snake_case)]
pub fn AbortGCV2ForUpgrade(
    context: &crate::context::Context,
    manager: Option<&mut dyn Manager>,
) -> Result<bool, crate::ManagerError> {
    let Some(manager) = manager else {
        return Ok(false);
    };
    if manager.Role() != config::RoleGCV2Worker {
        return Ok(false);
    }
    manager.AbortGCV2(context)?;
    Ok(true)
}

/// Match PD's GC management setting; absent metadata uses unified GC.
#[allow(non_snake_case)]
pub fn IsKeyspaceUsingKeyspaceLevelGC(meta: Option<&crate::keyspacepb::KeyspaceMeta>) -> bool {
    meta.and_then(|m| m.config.get("gc_management_type"))
        .is_some_and(|v| v == "keyspace_level")
}

/// Shared controller ownership for all consumers of one canonical storage runtime.
pub type SharedManager = std::sync::Arc<std::sync::Mutex<Box<dyn Manager>>>;

/// Storage-owner boundary implemented by the canonical Domain.
/// This reuses its existing store lifetime and avoids a process-wide manager registry.
pub trait ManagerStore {
    fn replace_external_workload_manager(
        &self,
        manager: Option<SharedManager>,
    ) -> Option<SharedManager>;
    fn get_external_workload_manager(&self) -> Option<SharedManager>;
}

#[allow(non_snake_case)]
pub fn SetManagerForStore(
    store: Option<&dyn ManagerStore>,
    manager: Option<SharedManager>,
) -> Option<SharedManager> {
    store.and_then(|store| store.replace_external_workload_manager(manager))
}

#[allow(non_snake_case)]
pub fn GetManagerFromStore(store: Option<&dyn ManagerStore>) -> Option<SharedManager> {
    store.and_then(ManagerStore::get_external_workload_manager)
}
