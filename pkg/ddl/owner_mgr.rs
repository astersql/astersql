// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// DDL Owner 管理器（按 keyspace 维度）。
//
// DDL owner 是集群中唯一负责调度/执行 DDL 作业的角色；通过 etcd（由 PD
// Placement Driver 管理）竞选产生。本模块维护「keyspace → OwnerManager」
// 的全局映射：仅在使用 TiKV 存储且 etcd 可用时真正启动竞选相关状态，
// UniStore 等本地存储路径直接跳过。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// 全局 OwnerManager 表：键为空字符串表示默认 keyspace。
static OWNER_MANAGERS: OnceLock<Mutex<BTreeMap<String, OwnerManager>>> = OnceLock::new();
/// 生成唯一 owner ID 的单调计数器。
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// 惰性初始化并返回全局管理器表。
fn managers() -> &'static Mutex<BTreeMap<String, OwnerManager>> {
    OWNER_MANAGERS
        .get_or_init(|| Mutex::new(BTreeMap::from([(String::new(), OwnerManager::default())])))
}

/// 单个 keyspace 上的 DDL owner 状态。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OwnerManager {
    id: String,
    started: bool,
    is_owner: bool,
}
impl OwnerManager {
    /// 启动 owner：非 TiKV 或已启动则空操作；缺 etcd 时报错；成功则生成本地 owner ID。
    pub fn start(&mut self, tikv_store: bool, etcd_available: bool) -> Result<(), String> {
        if self.started || !tikv_store {
            return Ok(());
        }
        if !etcd_available {
            return Err("etcd client is nil, maybe the server is not started with PD".into());
        }
        self.id = format!("ddl-owner-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
        self.started = true;
        self.is_owner = true;
        Ok(())
    }
    /// 关闭并清除 owner 身份。
    pub fn close(&mut self) {
        self.started = false;
        self.is_owner = false;
    }
    /// 返回当前 owner 标识字符串。
    pub fn id(&self) -> &str {
        &self.id
    }
    /// 是否处于「已启动且自认为是 owner」状态。
    pub fn is_owner(&self) -> bool {
        self.started && self.is_owner
    }
}
/// 按 keyspace 启动（或获取已有）OwnerManager，返回其 ID。
pub fn start_owner_manager(
    keyspace: &str,
    tikv_store: bool,
    etcd_available: bool,
) -> Result<String, String> {
    let mut all = managers().lock().unwrap();
    let manager = all.entry(keyspace.to_owned()).or_default();
    manager.start(tikv_store, etcd_available)?;
    Ok(manager.id.clone())
}
/// 关闭指定 keyspace 的 OwnerManager。
pub fn close_owner_manager(keyspace: &str) {
    if let Some(manager) = managers().lock().unwrap().get_mut(keyspace) {
        manager.close();
    }
}
/// 查询指定 keyspace 的 OwnerManager 快照。
pub fn owner_manager(keyspace: &str) -> Option<OwnerManager> {
    managers().lock().unwrap().get(keyspace).cloned()
}
