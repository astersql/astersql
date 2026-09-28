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

// OwnerManager 行为单测，对齐 Go TestOwnerManager 并直接覆盖生产实现。

use crate::owner_mgr::{close_owner_manager, owner_manager, start_owner_manager};

/// 分别验证 UniStore / TiKV 在空与命名 keyspace 下的启动、关闭与重复启动语义。
#[test]
fn test_owner_manager() {
    for keyspace in ["", "ks_test"] {
        let id = start_owner_manager(keyspace, false, false).expect("start UniStore owner manager");
        assert!(id.is_empty());
        assert!(!owner_manager(keyspace).expect("manager exists").is_owner());
        close_owner_manager(keyspace);

        let error = start_owner_manager(keyspace, true, false)
            .expect_err("TiKV owner manager requires etcd");
        assert_eq!(
            error,
            "etcd client is nil, maybe the server is not started with PD"
        );

        let id = start_owner_manager(keyspace, true, true).expect("start TiKV owner manager");
        assert!(!id.is_empty());
        let manager = owner_manager(keyspace).expect("manager exists");
        assert_eq!(manager.id(), id);
        assert!(manager.is_owner());

        // Go's Start returns nil without recreating an already-started manager.
        assert_eq!(
            start_owner_manager(keyspace, true, false).expect("repeat start is idempotent"),
            id
        );

        close_owner_manager(keyspace);
        let manager = owner_manager(keyspace).expect("close retains the map entry");
        assert_eq!(manager.id(), id);
        assert!(!manager.is_owner());
    }
}
