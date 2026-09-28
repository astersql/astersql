// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Ported from pkg/infoschema/issyncer/syncer_test.go.

// Syncer 单元测试：覆盖 `skipMDLCheck` 在普通 / 跨 keyspace 下的分支。
//
// MDL（Metadata Lock，元数据锁）检查：跨 KS Syncer 仅关心系统（保留 ID）表，
// 表集合不含保留 ID 时可跳过；普通 Syncer 永不跳过。

use crate::{JobMDL, New, NewCrossKSSyncer, SchemaStore, getFlashbackStartTSFromErrorMsg};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// testStoreWithKS mirrors Go's `testStoreWithKS`, a store stub that only
/// needs to answer `GetKeyspace`; every other `SchemaStore` method keeps the
/// trait's empty default since `skipMDLCheck` never touches the store.
///
/// 仅实现 `GetKeyspace` 的 SchemaStore stub，供跨 KS Syncer 构造。
#[derive(Default)]
struct TestStoreWithKS;
impl SchemaStore for TestStoreWithKS {
    fn GetKeyspace(&self) -> String {
        "test_ks".to_string()
    }
}

/// 将 ID 切片转为 HashSet，便于传给 `skipMDLCheck`。
fn idSet(ids: &[i64]) -> HashSet<i64> {
    ids.iter().copied().collect()
}

/// 对照 Go：普通 Syncer 永不跳过；跨 KS 在含保留表 ID 时不跳过，否则跳过。
#[test]
fn test_syncer_skip_mdl_check() {
    // 普通 Syncer：任意表集合都不跳过 MDL。
    let syncer = New(None, None, 0, None, None, None);
    assert!(!syncer.skipMDLCheck(&idSet(&[])));
    assert!(!syncer.skipMDLCheck(&idSet(&[123])));
    assert!(!syncer.skipMDLCheck(&idSet(&[123, 456])));
    assert!(!syncer.skipMDLCheck(&idSet(&[metadef::ReservedGlobalIDUpperBound])));
    assert!(!syncer.skipMDLCheck(&idSet(&[123, metadef::ReservedGlobalIDUpperBound])));

    // 跨 KS：无保留 ID 则跳过；含保留 ID 则不跳过。
    let syncer = NewCrossKSSyncer(
        Some(Arc::new(TestStoreWithKS) as Arc<dyn SchemaStore>),
        None,
        0,
        None,
        None,
        "ks1",
    );
    assert!(syncer.skipMDLCheck(&idSet(&[])));
    assert!(syncer.skipMDLCheck(&idSet(&[123])));
    assert!(syncer.skipMDLCheck(&idSet(&[123, 456])));
    assert!(!syncer.skipMDLCheck(&idSet(&[metadef::ReservedGlobalIDUpperBound])));
    assert!(!syncer.skipMDLCheck(&idSet(&[123, metadef::ReservedGlobalIDUpperBound])));

    let mut jobs = HashMap::new();
    jobs.insert(
        1,
        JobMDL {
            Ver: 10,
            TableIDs: idSet(&[123]),
        },
    );
    jobs.insert(
        2,
        JobMDL {
            Ver: 11,
            TableIDs: idSet(&[metadef::ReservedGlobalIDUpperBound]),
        },
    );
    syncer.refreshMDLCheckTableInfoWithJobs(jobs, 11);
    let (version, jobs) = syncer.mdlCheckSnapshot();
    assert_eq!(version, 11);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs.get(&2).unwrap().Ver, 11);
    assert!(syncer.mdlCheckContains(metadef::ReservedGlobalIDUpperBound));
}

/// Go's `strconv.ParseUint` rejects leading/trailing whitespace and the helper
/// only accepts the exact flashback error suffix.
#[test]
fn flashback_start_ts_parser_matches_go() {
    assert_eq!(
        getFlashbackStartTSFromErrorMsg(
            "schema is in flashback progress, FlashbackStartTS is 18446744073709551615"
        ),
        u64::MAX
    );
    assert_eq!(
        getFlashbackStartTSFromErrorMsg(
            "schema is in flashback progress, FlashbackStartTS is 123 "
        ),
        0
    );
    assert_eq!(
        getFlashbackStartTSFromErrorMsg(
            "schema is in flashback progress, FlashbackStartTS is  123"
        ),
        0
    );
    assert_eq!(getFlashbackStartTSFromErrorMsg("unrelated error"), 0);
}
