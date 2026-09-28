// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Affinity 包级对外接口。
//
// 维护进程内单例 `PackageState`（当前 Manager 与可选 PD Client），
// 对外提供创建 / 删除 / 查询 Affinity Group 的便捷函数。
// Affinity Group：将一组 key range 声明为亲和集合，便于 PD（Placement Driver）调度。

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::thread;
use std::time::Duration;

use crate::manager::{
    AffinityError, AffinityGroupKeyRange, AffinityGroupState, Context, Manager, PdClient,
    new_mock_manager, new_pd_manager,
};

/// 删除 Affinity Group 时的最大重试次数（不含首次尝试）。
pub const MAX_RETRY_TIMES: usize = 3;
/// 两次删除重试之间的等待间隔。
pub const RETRY_INTERVAL: Duration = Duration::from_millis(200);

/// 包级共享状态：当前 Manager 实现与可选的 PD Client。
struct PackageState {
    manager: Arc<dyn Manager>,
    pd_client: Option<Arc<dyn PdClient>>,
}

/// 惰性初始化并返回包级 `PackageState` 的读写锁。
fn package_state() -> &'static RwLock<PackageState> {
    static STATE: OnceLock<RwLock<PackageState>> = OnceLock::new();
    STATE.get_or_init(|| {
        RwLock::new(PackageState {
            manager: new_mock_manager(),
            pd_client: None,
        })
    })
}

/// 初始化包级 Manager：有 PD Client 时用 PdManager，否则用 MockManager。
pub fn init_manager(pd_client: Option<Arc<dyn PdClient>>) {
    let manager = pd_client
        .clone()
        .map(new_pd_manager)
        .unwrap_or_else(new_mock_manager);
    *package_state()
        .write()
        .expect("affinity package state lock poisoned") = PackageState { manager, pd_client };
}

/// 若指定 Affinity Group 尚不存在则创建；空输入直接成功。
pub fn create_groups_if_not_exists(
    ctx: &dyn Context,
    groups: &HashMap<String, Vec<AffinityGroupKeyRange>>,
) -> Result<(), AffinityError> {
    if groups.is_empty() {
        return Ok(());
    }
    package_state()
        .read()
        .expect("affinity package state lock poisoned")
        .manager
        .create_affinity_groups_if_not_exists(ctx, groups)
}

/// 批量删除 Affinity Group；空输入直接成功。
pub fn delete_groups(ctx: &dyn Context, ids: &[String]) -> Result<(), AffinityError> {
    if ids.is_empty() {
        return Ok(());
    }
    package_state()
        .read()
        .expect("affinity package state lock poisoned")
        .manager
        .delete_affinity_groups(ctx, ids)
}

/// 带重试的批量删除：最多重试 `MAX_RETRY_TIMES` 次，间隔 `RETRY_INTERVAL`。
pub fn delete_groups_with_retry(ctx: &dyn Context, ids: &[String]) -> Result<(), AffinityError> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut last_error = None;
    // 含首次共 MAX_RETRY_TIMES+1 次尝试；中间失败则 sleep 后重试。
    for attempt in 0..=MAX_RETRY_TIMES {
        match delete_groups(ctx, ids) {
            Ok(()) => return Ok(()),
            Err(err) => {
                if attempt == MAX_RETRY_TIMES {
                    log::error!(
                        target: "astersql_domain_affinity",
                        "Failed to delete affinity groups after retries; error={err}; groupIDs={ids:?}"
                    );
                }
                last_error = Some(err);
            }
        }
        if attempt != MAX_RETRY_TIMES {
            thread::sleep(RETRY_INTERVAL);
        }
    }
    Err(last_error.expect("at least one delete attempt must have failed"))
}

/// 按 id 列表查询 Affinity Group 状态；空列表返回空 Map。
pub fn get_groups(
    ctx: &dyn Context,
    ids: &[String],
) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    package_state()
        .read()
        .expect("affinity package state lock poisoned")
        .manager
        .get_affinity_groups(ctx, ids)
}

/// 拉取全部 Affinity Group 状态；未配置 PD Client 时返回空 Map。
pub fn get_all_group_states(
    ctx: &dyn Context,
) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
    let client = package_state()
        .read()
        .expect("affinity package state lock poisoned")
        .pd_client
        .clone();
    match client {
        Some(client) => client.get_all_affinity_groups(ctx),
        None => Ok(HashMap::new()),
    }
}

/// 测试辅助：临时替换包级 PD Client，返回恢复闭包。
pub fn set_pd_client_for_test(client: Option<Arc<dyn PdClient>>) -> impl FnOnce() {
    let original = {
        let mut state = package_state()
            .write()
            .expect("affinity package state lock poisoned");
        std::mem::replace(&mut state.pd_client, client)
    };
    move || {
        package_state()
            .write()
            .expect("affinity package state lock poisoned")
            .pd_client = original;
    }
}
