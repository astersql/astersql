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

//! canonical Domain 的 InfoSchema-v2 生产装配测试。
//!
//! 通过 mock store 执行真实 DDL，验证新旧快照共享版本历史与 GC 状态，
//! 同时确保回收旧版本不会破坏当前快照中的表元数据。

use crate::NewTestKit;
use crate::mockstore::CreateMockStoreAndDomainV2;

#[test]
/// 验证 Domain 经 DDL 更新后，旧 V2 快照也能观察并执行共享历史的版本回收。
fn canonical_domain_ddl_populates_shared_v2_history() {
    let (store, domain) = CreateMockStoreAndDomainV2(1024 * 1024);
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table v2_history (id int key, b int)", Vec::new());

    // 保留 DDL 变更前的快照句柄，用它验证历史缓存并非每个快照各自持有。
    let old_is = domain.info_schema();
    assert!(old_is.IsV2());
    let old_version = old_is.SchemaMetaVersion();
    let table_id = domain
        .table_by_name("test", "v2_history")
        .expect("old V2 snapshot must resolve the DDL-created table")
        .ID;

    // 反复增删索引推进 schema 版本，并产生足够多的历史项供后续 GC 回收。
    for _ in 0..4 {
        tk.MustExec(
            "alter table v2_history add index v2_history_b(b)",
            Vec::new(),
        );
        tk.MustExec("alter table v2_history drop index v2_history_b", Vec::new());
    }

    let now_is = domain.info_schema();
    assert!(now_is.SchemaMetaVersion() > old_version);
    // 从旧快照触发共享 GC：旧句柄应失去已回收表，而当前快照仍能按 ID 找到它。
    let (deleted, _) = old_is
        .GCOldVersion(now_is.SchemaMetaVersion() - 2)
        .expect("V2 snapshots must share production GC state");
    assert!(deleted > 0);
    assert!(old_is.TableByID(table_id).is_none());
    assert!(now_is.TableByID(table_id).is_some());
}
