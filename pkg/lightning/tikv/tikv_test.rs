// Copyright 2019 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// TikV/PD 交互逻辑单元测试。
//
// 覆盖 `ForAllStores` 状态过滤、从指标解析导入模式、以及 PD/TiKV 版本区间校验。

use crate::*;
use semver::Version;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// 测试用 PD 替身：预置 Store 列表与版本。
struct MockPd {
    stores: Vec<Store>,
    version: String,
}

impl MockPd {
    /// 返回克隆的 Store 列表。
    fn GetStores(&self) -> Result<Vec<Store>, TikvError> {
        Ok(self.stores.clone())
    }
    /// 返回预置 PD 版本。
    fn GetPDVersion(&self) -> Result<String, TikvError> {
        Ok(self.version.clone())
    }
}

impl PdClient for MockPd {
    fn GetStores(&self) -> Result<Vec<Store>, TikvError> {
        MockPd::GetStores(self)
    }
    fn GetPDVersion(&self) -> Result<String, TikvError> {
        MockPd::GetPDVersion(self)
    }
}

/// 构造 `Store` 描述。
fn store_info(id: u64, address: &str, version: &str, state: StoreState) -> Store {
    Store {
        id,
        address: address.into(),
        version: version.into(),
        state,
    }
}

/// `store_info` 别名，对应 Go 侧 meta store 辅助函数。
fn meta_store(id: u64, address: &str, version: &str, state: StoreState) -> Store {
    store_info(id, address, version, state)
}

/// 仅遍历 state ≤ Offline 的 Store，跳过 Tombstone。
#[test]
fn TestForAllStores() {
    let pd = MockPd {
        stores: vec![
            store_info(1, "tikv-1", "v8.5.0", StoreState::Up),
            store_info(2, "tikv-2", "v8.5.0", StoreState::Offline),
            store_info(3, "tikv-3", "v8.5.0", StoreState::Tombstone),
        ],
        version: "v8.5.0".into(),
    };
    let visited = Arc::new(Mutex::new(Vec::new()));
    ForAllStores(&pd, StoreState::Offline, {
        let visited = Arc::clone(&visited);
        move |store| {
            visited.lock().unwrap().push(store.id);
            Ok(())
        }
    })
    .unwrap();
    let mut ids = visited.lock().unwrap().clone();
    ids.sort();
    assert_eq!(ids, vec![1, 2]);
    assert_eq!(meta_store(1, "a", "v1.0.0", StoreState::Up).id, 1);
}

/// 单个回调失败会返回错误，但已经选中的 Store 仍都被并行调度。
#[test]
fn ForAllStoresPropagatesActionError() {
    let pd = MockPd {
        stores: vec![
            store_info(1, "tikv-1", "v8.5.0", StoreState::Up),
            store_info(2, "tikv-2", "v8.5.0", StoreState::Offline),
        ],
        version: "v8.5.0".into(),
    };
    let calls = AtomicUsize::new(0);
    let error = ForAllStores(&pd, StoreState::Offline, |store| {
        calls.fetch_add(1, Ordering::SeqCst);
        if store.id == 1 {
            Err(TikvError::Remote("boom".into()))
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert!(error.to_string().contains("boom"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// 指标中 hard_pending_compaction_bytes_limit 为 0 表示 Import。
#[test]
fn TestFetchModeFromMetrics() {
    let prefix = "tikv_config_rocksdb{cf=\"default\",name=\"hard_pending_compaction_bytes_limit\"}";
    assert_eq!(
        FetchModeFromMetrics(&format!("{prefix} 0\n")).unwrap(),
        SwitchMode::Import
    );
    assert_eq!(
        FetchModeFromMetrics(&format!("other 1\n{prefix} 274877906944\n")).unwrap(),
        SwitchMode::Normal
    );
    assert_eq!(
        FetchModeFromMetrics(&format!("{prefix} 0.0\n")).unwrap(),
        SwitchMode::Normal
    );
    assert_eq!(
        FetchModeFromMetrics(&format!("{prefix} not-a-number\n")).unwrap(),
        SwitchMode::Normal
    );
    assert!(FetchModeFromMetrics("other_metric 1").is_err());
}

/// PD 版本落在 [min, max) 内通过，过旧或非法 semver 失败。
#[test]
fn TestCheckPDVersion() {
    let min = Version::new(8, 0, 0);
    let max = Version::new(9, 0, 0);
    let compatible = MockPd {
        stores: vec![],
        version: "v8.5.1".into(),
    };
    CheckPDVersion(&compatible, &min, &max).unwrap();
    let old = MockPd {
        stores: vec![],
        version: "v7.5.0".into(),
    };
    assert!(CheckPDVersion(&old, &min, &max).is_err());
    let invalid = MockPd {
        stores: vec![],
        version: "not-semver".into(),
    };
    assert!(CheckPDVersion(&invalid, &min, &max).is_err());
    let repeated_prefix = MockPd {
        stores: vec![],
        version: "vv8.5.1".into(),
    };
    assert!(CheckPDVersion(&repeated_prefix, &min, &max).is_err());
    let max_beta = MockPd {
        stores: vec![],
        version: "v9.0.0-beta".into(),
    };
    let error = CheckPDVersion(&max_beta, &min, &max).unwrap_err();
    assert!(error.to_string().contains("PD version too new"));
}

/// TiKV 版本检查跳过 Tombstone；过旧节点应报错并含地址。
#[test]
fn TestCheckTiKVVersion() {
    let min = Version::new(8, 0, 0);
    let max = Version::new(9, 0, 0);
    let pd = MockPd {
        stores: vec![
            store_info(1, "tikv-1", "v8.4.0", StoreState::Up),
            store_info(2, "tikv-2", "8.5.0", StoreState::Offline),
            store_info(3, "removed", "v1.0.0", StoreState::Tombstone),
        ],
        version: "v8.5.0".into(),
    };
    CheckTiKVVersion(&pd, &min, &max).unwrap();

    let bad = MockPd {
        stores: vec![store_info(1, "tikv-old", "v7.1.0", StoreState::Up)],
        version: "v8.5.0".into(),
    };
    let error = CheckTiKVVersion(&bad, &min, &max).unwrap_err();
    assert!(error.to_string().contains("tikv-old"));

    let max_beta = MockPd {
        stores: vec![store_info(1, "tikv-beta", "v9.0.0-beta", StoreState::Up)],
        version: "v8.5.0".into(),
    };
    let error = CheckTiKVVersion(&max_beta, &min, &max).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("TiKV (at tikv-beta) version too new")
    );
}
