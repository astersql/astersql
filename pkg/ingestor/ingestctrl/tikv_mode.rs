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

// TiKV Store 导入/正常模式切换。
//
// 物理导入（Lightning）前将 Store 切到 Import 模式以降低写冲突与压缩干扰；
// 导入结束后切回 Normal。Store 是 TiKV 集群中的一个存储节点。

use std::sync::{Arc, Mutex};

use crate::{CancellationToken, KeyRange, Result};

/// Store 工作模式：Import 面向大批量 SST 导入，Normal 面向常规读写。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwitchMode {
    Import,
    Normal,
}

/// Store 生命周期状态；不可用节点在模式切换时会被跳过。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreState {
    Up,
    Offline,
    Tombstone,
    Disconnected,
}

/// TiKV Store 元信息：节点 ID、地址与当前状态。
#[derive(Clone, Debug)]
pub struct Store {
    pub id: u64,
    pub address: String,
    pub state: StoreState,
}

/// 从 PD/元数据侧枚举当前可见的 Store 列表。
pub trait StoreCatalog: Send + Sync {
    /// 在可取消令牌下拉取 Store 快照。
    fn Stores(&self, token: &CancellationToken) -> Result<Vec<Store>>;
}
/// 向单个 Store 下发 Import/Normal 模式切换请求。
pub trait ModeClient: Send + Sync {
    /// 对指定 Store 的 key 范围应用目标模式。
    fn SwitchMode(
        &self,
        token: &CancellationToken,
        store: &Store,
        mode: SwitchMode,
        ranges: &[KeyRange],
    ) -> Result<()>;
}

/// 批量将集群内 Store 切换到 Import 或 Normal 模式的门面。
pub trait TiKVModeSwitcher: Send + Sync {
    /// 切入 Import 模式，便于物理写入 SST。
    fn ToImportMode(&self, token: &CancellationToken, ranges: &[KeyRange]);
    /// 切回 Normal 模式，恢复常规服务。
    fn ToNormalMode(&self, token: &CancellationToken, ranges: &[KeyRange]);
}

/// 默认实现：枚举 Store、跳过不可用节点，并记录切换失败。
pub struct switcher {
    catalog: Arc<dyn StoreCatalog>,
    client: Arc<dyn ModeClient>,
    failures: Mutex<Vec<(u64, String)>>,
}

/// 构造带失败记录缓冲的模式切换器。
pub fn NewTiKVModeSwitcher(
    catalog: Arc<dyn StoreCatalog>,
    client: Arc<dyn ModeClient>,
) -> Arc<dyn TiKVModeSwitcher> {
    Arc::new(switcher {
        catalog,
        client,
        failures: Mutex::new(Vec::new()),
    })
}

impl switcher {
    /// 对所有 Up 状态 Store 执行模式切换；失败只记录不中断整批。
    fn switchTiKVMode(&self, token: &CancellationToken, mode: SwitchMode, ranges: &[KeyRange]) {
        let Ok(stores) = self.catalog.Stores(token) else {
            return;
        };
        std::thread::scope(|scope| {
            for store in stores {
                // Go ForAllStores uses Offline as an inclusive maximum state.
                if !matches!(store.state, StoreState::Up | StoreState::Offline) {
                    continue;
                }
                scope.spawn(move || {
                    if let Err(error) = self.client.SwitchMode(token, &store, mode, ranges) {
                        // 单节点失败不影响其它节点；汇总供上层诊断
                        if let Ok(mut failures) = self.failures.lock() {
                            failures.push((store.id, error.to_string()));
                        }
                    }
                });
            }
        });
    }

    /// 返回已记录的 (store_id, 错误信息) 列表。
    pub fn failures(&self) -> Vec<(u64, String)> {
        self.failures
            .lock()
            .map(|failures| failures.clone())
            .unwrap_or_default()
    }
}

impl TiKVModeSwitcher for switcher {
    fn ToImportMode(&self, token: &CancellationToken, ranges: &[KeyRange]) {
        self.switchTiKVMode(token, SwitchMode::Import, ranges);
    }
    fn ToNormalMode(&self, token: &CancellationToken, ranges: &[KeyRange]) {
        self.switchTiKVMode(token, SwitchMode::Normal, ranges);
    }
}
