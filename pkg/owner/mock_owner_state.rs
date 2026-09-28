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

// 进程级模拟 Owner 登记表。
//
// 用 `(store_id, owner_type)` 作为键保存当前 Owner ID，对应 Go 包内全局状态，
// 供本地存储与 Owner 相关单测在无 etcd 时模拟竞选结果。
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// 进程级模拟 Owner 注册表，对应 Go 包内全局变量。
/// Process-wide mock owner registry, matching the Go package global.
#[allow(non_upper_case_globals)]
pub static MockGlobalStateEntry: LazyLock<MockGlobalState> =
    LazyLock::new(MockGlobalState::default);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// Owner 条目键：存储 UUID 与 Owner 类型（路径）组合。
struct OwnerKey {
    store_id: String,
    owner_type: String,
}

/// 本地存储与 Owner 测试共享的全局状态。
/// Shared state used by local storage and owner tests.
#[derive(Debug, Default)]
pub struct MockGlobalState {
    current_owner: Mutex<HashMap<OwnerKey, String>>,
}

impl MockGlobalState {
    /// 选择 `(store_id, owner_type)` 上独立竞选的 Owner 条目。
    /// Selects the independently elected owner for `(store_id, owner_type)`.
    pub fn OwnerKey<'a>(
        &'a self,
        store_id: impl Into<String>,
        owner_type: impl Into<String>,
    ) -> MockGlobalStateSelector<'a> {
        MockGlobalStateSelector {
            state: self,
            key: OwnerKey {
                store_id: store_id.into(),
                owner_type: owner_type.into(),
            },
        }
    }
}

/// 针对单个模拟 Owner 条目的原子选择器（在锁内完成读改）。
/// Atomic selector over a single mock owner entry.
pub struct MockGlobalStateSelector<'a> {
    state: &'a MockGlobalState,
    key: OwnerKey,
}

impl MockGlobalStateSelector<'_> {
    /// 返回 Owner ID；无条目时返回空串（对应 Go 字符串零值）。
    /// Returns the owner ID, or Go's string zero value when no entry exists.
    pub fn GetOwner(&self) -> String {
        self.state
            .current_owner
            .lock()
            .expect("mock owner mutex poisoned")
            .get(&self.key)
            .cloned()
            .unwrap_or_default()
    }

    /// 仅当当前值为空时设置 Owner，成功返回 true（模拟抢占空位）。
    /// Sets `owner` only when the current value is empty.
    pub fn SetOwner(&self, owner: impl Into<String>) -> bool {
        let mut owners = self
            .state
            .current_owner
            .lock()
            .expect("mock owner mutex poisoned");
        let current = owners.entry(self.key.clone()).or_default();
        if current.is_empty() {
            *current = owner.into();
            true
        } else {
            false
        }
    }

    /// 仅当 `owner` 仍是当前 Owner 时清空条目，成功返回 true。
    /// Clears the entry only when `owner` is still the current owner.
    pub fn UnsetOwner(&self, owner: &str) -> bool {
        let mut owners = self
            .state
            .current_owner
            .lock()
            .expect("mock owner mutex poisoned");
        let current = owners.entry(self.key.clone()).or_default();
        if current == owner {
            current.clear();
            true
        } else {
            false
        }
    }

    /// 在与更新相同的锁下比较是否为当前 Owner。
    /// Compares against the current value under the same lock as updates.
    pub fn IsOwner(&self, owner: &str) -> bool {
        self.state
            .current_owner
            .lock()
            .expect("mock owner mutex poisoned")
            .get(&self.key)
            .map(String::as_str)
            .unwrap_or_default()
            == owner
    }
}
