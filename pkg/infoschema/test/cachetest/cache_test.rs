// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// InfoSchema 版本缓存（InfoCache）行为测试。
//
// 对应 Go `pkg/infoschema/test/cachetest/cache_test.go`。覆盖按 schema 版本 /
// 快照时间戳（snapshot timestamp，MVCC 读所用的时间点）插入、查找、淘汰、
// 扩缩容，以及空 schema version 与版本缺口等边界。语义与分支与 Go 一一对应。

// 对应 pkg/infoschema/test/cachetest/cache_test.go，语义与分支与 Go 版本一一对应。
//
// Go 侧 `infoschema.NewCache(store, capacity)` 的第一个参数是 `kv.Storage`；
// 本测试文件里所有调用都传 `nil`，即从不触发 store 相关的后台 GC 路径
// （见 pkg/infoschema/cache.rs 里 `Insert` 对应的 GC 调度分支，这里同样只
// 触碰版本/时间戳排序缓存，不connect真实存储）。Rust 生产实现的 `NewCache`
// 没有 store 形参（`pkg/infoschema/cache.rs::NewCache(capacity)`），因此这里
// 直接调用 `infoschema::NewCache(capacity)`，等价于 Go 的 `NewCache(nil, capacity)`。
//
// Go 用 `require.Equal(t, is, ic.GetByVersion(v))` 比较的是同一个 InfoSchema
// 接口对象（testify 对指针做深度比较，但生产代码从不复制/克隆 schema 对象，
// 因此实际比较的就是同一个底层指针）。Rust 侧用 `Arc::ptr_eq` 表达这个“同一个
// 对象实例”的身份相等语义，比 `SchemaMetaVersion()` 数值比较更强、更贴近 Go 原意。

use std::sync::Arc;

use astersql_infoschema::{self as infoschema, SchemaRef};

/// 对应 Go 里反复出现的 `infoschema.MockInfoSchemaWithSchemaVer(nil, ver)`。
fn mock(schema_ver: i64) -> SchemaRef {
    infoschema::MockInfoSchemaWithSchemaVer(Vec::new(), schema_ver)
}

/// 断言 `actual` 是 `Some`，且与 `expected` 是同一个 schema 对象实例。
fn assert_same(expected: &SchemaRef, actual: &Option<SchemaRef>, msg: &str) {
    match actual {
        Some(actual) => {
            assert!(
                Arc::ptr_eq(expected, actual),
                "{msg}: expected the same schema instance"
            )
        }
        None => panic!("{msg}: expected Some(schema), got None"),
    }
}

/// 断言 `actual` 是 `None`，对应 Go 的 `require.Nil`。
fn assert_absent(actual: &Option<SchemaRef>, msg: &str) {
    assert!(actual.is_none(), "{msg}: expected None, got Some(schema)");
}

// test_new_cache 对应 Go 的 TestNewCache。
#[test]
fn test_new_cache() {
    let ic = infoschema::NewCache(16);
    // Go: require.NotNil(t, ic)。Rust 的 NewCache 返回值类型本身非 Option/指针，
    // 构造成功即隐含非 nil；改为验证新缓存的初始状态确实可用。
    assert_eq!(0, ic.Len());
}

// test_insert 对应 Go 的 TestInsert：按版本插入、替换和淘汰 schema cache。
#[test]
fn test_insert() {
    let ic = infoschema::NewCache(3);

    let is2 = mock(2);
    ic.Insert(is2.clone(), 2);
    assert_same(&is2, &ic.GetByVersion(2), "GetByVersion(2)");
    assert_same(&is2, &ic.GetBySnapshotTS(2), "GetBySnapshotTS(2)");
    assert_same(&is2, &ic.GetBySnapshotTS(10), "GetBySnapshotTS(10)");
    assert_absent(&ic.GetBySnapshotTS(0), "GetBySnapshotTS(0)");

    // newer
    let is5 = mock(5);
    ic.Insert(is5.clone(), 5);
    assert_same(&is5, &ic.GetByVersion(5), "GetByVersion(5)");
    assert_same(
        &is2,
        &ic.GetByVersion(2),
        "GetByVersion(2) after inserting 5",
    );
    // there is a gap in schema cache, so don't use this version
    assert_absent(
        &ic.GetBySnapshotTS(2),
        "GetBySnapshotTS(2) after inserting 5",
    );
    assert_same(
        &is5,
        &ic.GetBySnapshotTS(10),
        "GetBySnapshotTS(10) after inserting 5",
    );

    // older
    let is0 = mock(0);
    ic.Insert(is0.clone(), 0);
    assert_same(
        &is5,
        &ic.GetByVersion(5),
        "GetByVersion(5) after inserting 0",
    );
    assert_same(
        &is2,
        &ic.GetByVersion(2),
        "GetByVersion(2) after inserting 0",
    );
    assert_same(
        &is0,
        &ic.GetByVersion(0),
        "GetByVersion(0) after inserting 0",
    );

    // replace 5, drop 0
    let is6 = mock(6);
    ic.Insert(is6.clone(), 6);
    assert_same(&is6, &ic.GetByVersion(6), "GetByVersion(6)");
    assert_same(
        &is5,
        &ic.GetByVersion(5),
        "GetByVersion(5) after inserting 6",
    );
    assert_same(
        &is2,
        &ic.GetByVersion(2),
        "GetByVersion(2) after inserting 6",
    );
    assert_absent(&ic.GetByVersion(0), "GetByVersion(0) after inserting 6");
    // there is a gap in schema cache, so don't use this version
    assert_absent(
        &ic.GetBySnapshotTS(2),
        "GetBySnapshotTS(2) after inserting 6",
    );
    assert_same(
        &is5,
        &ic.GetBySnapshotTS(5),
        "GetBySnapshotTS(5) after inserting 6",
    );
    assert_same(
        &is6,
        &ic.GetBySnapshotTS(10),
        "GetBySnapshotTS(10) after inserting 6",
    );

    // replace 2, drop 2
    let is3 = mock(3);
    ic.Insert(is3.clone(), 3);
    assert_same(
        &is6,
        &ic.GetByVersion(6),
        "GetByVersion(6) after inserting 3",
    );
    assert_same(
        &is5,
        &ic.GetByVersion(5),
        "GetByVersion(5) after inserting 3",
    );
    assert_same(
        &is3,
        &ic.GetByVersion(3),
        "GetByVersion(3) after inserting 3",
    );
    assert_absent(&ic.GetByVersion(2), "GetByVersion(2) after inserting 3");
    assert_absent(&ic.GetByVersion(0), "GetByVersion(0) after inserting 3");
    assert_absent(
        &ic.GetBySnapshotTS(2),
        "GetBySnapshotTS(2) after inserting 3",
    );
    assert_same(
        &is6,
        &ic.GetBySnapshotTS(10),
        "GetBySnapshotTS(10) after inserting 3",
    );

    // insert 2, but failed silently
    ic.Insert(is2.clone(), 2);
    assert_same(
        &is6,
        &ic.GetByVersion(6),
        "GetByVersion(6) after re-inserting 2",
    );
    assert_same(
        &is5,
        &ic.GetByVersion(5),
        "GetByVersion(5) after re-inserting 2",
    );
    assert_same(
        &is3,
        &ic.GetByVersion(3),
        "GetByVersion(3) after re-inserting 2",
    );
    assert_absent(&ic.GetByVersion(2), "GetByVersion(2) after re-inserting 2");
    assert_absent(&ic.GetByVersion(0), "GetByVersion(0) after re-inserting 2");
    assert_absent(
        &ic.GetBySnapshotTS(2),
        "GetBySnapshotTS(2) after re-inserting 2",
    );
    assert_same(
        &is6,
        &ic.GetBySnapshotTS(10),
        "GetBySnapshotTS(10) after re-inserting 2",
    );

    // insert 5, but it is already in
    ic.Insert(is5.clone(), 5);
    assert_same(
        &is6,
        &ic.GetByVersion(6),
        "GetByVersion(6) after re-inserting 5",
    );
    assert_same(
        &is5,
        &ic.GetByVersion(5),
        "GetByVersion(5) after re-inserting 5",
    );
    assert_same(
        &is3,
        &ic.GetByVersion(3),
        "GetByVersion(3) after re-inserting 5",
    );
    assert_absent(&ic.GetByVersion(2), "GetByVersion(2) after re-inserting 5");
    assert_absent(&ic.GetByVersion(0), "GetByVersion(0) after re-inserting 5");
    assert_absent(
        &ic.GetBySnapshotTS(2),
        "GetBySnapshotTS(2) after re-inserting 5",
    );
    assert_same(
        &is5,
        &ic.GetBySnapshotTS(5),
        "GetBySnapshotTS(5) after re-inserting 5",
    );
    assert_same(
        &is6,
        &ic.GetBySnapshotTS(10),
        "GetBySnapshotTS(10) after re-inserting 5",
    );
}

// test_get_by_version 对应 Go 的 TestGetByVersion：精确版本查找和窗口内回退。
#[test]
fn test_get_by_version() {
    let ic = infoschema::NewCache(2);
    let is1 = mock(1);
    ic.Insert(is1.clone(), 1);
    let is3 = mock(3);
    ic.Insert(is3.clone(), 3);

    assert_same(&is1, &ic.GetByVersion(1), "GetByVersion(1)");
    assert_same(&is3, &ic.GetByVersion(3), "GetByVersion(3)");
    assert_absent(&ic.GetByVersion(0), "index == 0, but not found");
    assert_eq!(
        1_i64,
        ic.GetByVersion(2)
            .expect("GetByVersion(2) should fall back to version 1")
            .SchemaMetaVersion()
    );
    assert_absent(&ic.GetByVersion(4), "index == length, but not found");
}

// test_get_latest 对应 Go 的 TestGetLatest：最新 schema 只随更高版本推进。
#[test]
fn test_get_latest() {
    let ic = infoschema::NewCache(16);
    assert_absent(&ic.GetLatest(), "GetLatest() on empty cache");

    let is1 = mock(1);
    ic.Insert(is1.clone(), 1);
    assert_same(&is1, &ic.GetLatest(), "GetLatest() after inserting 1");

    // newer change the newest
    let is2 = mock(2);
    ic.Insert(is2.clone(), 2);
    assert_same(&is2, &ic.GetLatest(), "GetLatest() after inserting 2");

    // older schema doesn't change the newest
    let is0 = mock(0);
    ic.Insert(is0, 0);
    assert_same(&is2, &ic.GetLatest(), "GetLatest() after inserting 0");
}

// test_get_by_timestamp 对应 Go 的 TestGetByTimestamp：按 snapshot ts 查找 schema，并覆盖错误 ts 修正路径。
#[test]
fn test_get_by_timestamp() {
    let ic = infoschema::NewCache(16);
    assert_absent(&ic.GetLatest(), "GetLatest() on empty cache");
    assert_eq!(0, ic.Len());

    let is1 = mock(1);
    ic.Insert(is1.clone(), 1);
    assert_absent(&ic.GetBySnapshotTS(0), "GetBySnapshotTS(0)");
    assert_same(&is1, &ic.GetBySnapshotTS(1), "GetBySnapshotTS(1)");
    assert_same(&is1, &ic.GetBySnapshotTS(2), "GetBySnapshotTS(2)");
    assert_eq!(1, ic.Len());

    let is3 = mock(3);
    ic.Insert(is3.clone(), 3);
    assert_same(&is3, &ic.GetLatest(), "GetLatest() after inserting 3");
    assert_absent(
        &ic.GetBySnapshotTS(0),
        "GetBySnapshotTS(0) after inserting 3",
    );
    // there is a gap, no schema returned for ts 2
    assert_absent(
        &ic.GetBySnapshotTS(2),
        "GetBySnapshotTS(2) after inserting 3",
    );
    assert_same(&is3, &ic.GetBySnapshotTS(3), "GetBySnapshotTS(3)");
    assert_same(&is3, &ic.GetBySnapshotTS(4), "GetBySnapshotTS(4)");
    assert_eq!(2, ic.Len());

    let is2 = mock(2);
    // schema version 2 doesn't have timestamp set
    // thus all schema before ver 2 cannot be searched by timestamp anymore
    // because the ts of ver 2 is not accurate
    ic.Insert(is2.clone(), 0);
    assert_same(
        &is3,
        &ic.GetLatest(),
        "GetLatest() after inserting 2 with ts=0",
    );
    assert_absent(
        &ic.GetBySnapshotTS(0),
        "GetBySnapshotTS(0) after inserting 2 with ts=0",
    );
    assert_absent(
        &ic.GetBySnapshotTS(1),
        "GetBySnapshotTS(1) after inserting 2 with ts=0",
    );
    assert_absent(
        &ic.GetBySnapshotTS(2),
        "GetBySnapshotTS(2) after inserting 2 with ts=0",
    );
    assert_same(
        &is3,
        &ic.GetBySnapshotTS(3),
        "GetBySnapshotTS(3) after inserting 2 with ts=0",
    );
    assert_same(
        &is3,
        &ic.GetBySnapshotTS(4),
        "GetBySnapshotTS(4) after inserting 2 with ts=0",
    );
    assert_eq!(3, ic.Len());

    // insert is2 again with correct timestamp, to correct previous wrong timestamp
    ic.Insert(is2.clone(), 2);
    assert_same(
        &is3,
        &ic.GetLatest(),
        "GetLatest() after correcting ts for 2",
    );
    assert_same(
        &is1,
        &ic.GetBySnapshotTS(1),
        "GetBySnapshotTS(1) after correcting ts for 2",
    );
    assert_same(
        &is2,
        &ic.GetBySnapshotTS(2),
        "GetBySnapshotTS(2) after correcting ts for 2",
    );
    assert_same(
        &is3,
        &ic.GetBySnapshotTS(3),
        "GetBySnapshotTS(3) after correcting ts for 2",
    );
    assert_eq!(3, ic.Len());
}

// test_re_size 对应 Go 的 TestReSize：扩容保留数据，缩容只保留最新窗口。
#[test]
fn test_re_size() {
    let ic = infoschema::NewCache(2);
    let is1 = mock(1);
    ic.Insert(is1.clone(), 1);
    let is2 = mock(2);
    ic.Insert(is2.clone(), 2);

    ic.ReSize(3);
    assert_eq!(2, ic.Size());
    assert_same(&is1, &ic.GetByVersion(1), "GetByVersion(1) after ReSize(3)");
    assert_same(&is2, &ic.GetByVersion(2), "GetByVersion(2) after ReSize(3)");
    let is3 = mock(3);
    assert!(ic.Insert(is3.clone(), 3));
    assert_same(
        &is1,
        &ic.GetByVersion(1),
        "GetByVersion(1) after inserting 3",
    );
    assert_same(
        &is2,
        &ic.GetByVersion(2),
        "GetByVersion(2) after inserting 3",
    );
    assert_same(
        &is3,
        &ic.GetByVersion(3),
        "GetByVersion(3) after inserting 3",
    );

    ic.ReSize(1);
    assert_eq!(1, ic.Size());
    assert_absent(&ic.GetByVersion(1), "GetByVersion(1) after ReSize(1)");
    assert_absent(&ic.GetByVersion(2), "GetByVersion(2) after ReSize(1)");
    assert_same(&is3, &ic.GetByVersion(3), "GetByVersion(3) after ReSize(1)");
    assert!(!ic.Insert(is2, 2));
    assert_eq!(1, ic.Size());
    let is4 = mock(4);
    assert!(ic.Insert(is4.clone(), 4));
    assert_eq!(1, ic.Size());
    assert_absent(&ic.GetByVersion(1), "GetByVersion(1) after inserting 4");
    assert_absent(&ic.GetByVersion(2), "GetByVersion(2) after inserting 4");
    assert_absent(&ic.GetByVersion(3), "GetByVersion(3) after inserting 4");
    assert_same(
        &is4,
        &ic.GetByVersion(4),
        "GetByVersion(4) after inserting 4",
    );
}

// test_cache_with_schema_ts_zero 对应 Go 的 TestCacheWithSchemaTsZero：覆盖 schema
// timestamp 为 0、版本缺口和 empty schema version。
#[test]
fn test_cache_with_schema_ts_zero() {
    let mut ic = infoschema::NewCache(16);

    for i in 1_i64..=8 {
        ic.Insert(mock(i), i as u64);
    }

    let check_fn = |ic: &infoschema::InfoCache, start: i64, end: i64, exist: bool| {
        assert!(start <= end);
        let latest_schema_version = ic
            .GetLatest()
            .expect("GetLatest() should be Some in this test")
            .SchemaMetaVersion();
        for ts in start..=end {
            match ic.GetBySnapshotTS(ts as u64) {
                Some(is) => {
                    assert!(exist, "ts {ts}");
                    if ts > latest_schema_version {
                        assert_eq!(latest_schema_version, is.SchemaMetaVersion(), "ts {ts}");
                    } else {
                        assert_eq!(ts, is.SchemaMetaVersion(), "ts {ts}");
                    }
                }
                None => {
                    assert!(!exist, "ts {ts}");
                }
            }
        }
    };
    check_fn(&ic, 1, 8, true);
    check_fn(&ic, 8, 10, true);

    // mock for meet error There is no Write MVCC info for the schema version
    ic.Insert(mock(9), 0);
    check_fn(&ic, 1, 7, true);
    check_fn(&ic, 8, 9, false);
    check_fn(&ic, 9, 10, false);

    for i in 10_i64..=16 {
        ic.Insert(mock(i), i as u64);
        check_fn(&ic, 1, 7, true);
        check_fn(&ic, 8, 9, false);
        check_fn(&ic, 10, 16, true);
    }
    assert_eq!(16, ic.Size());

    // refill the cache
    ic.Insert(mock(9), 9);
    check_fn(&ic, 1, 16, true);
    assert_eq!(16, ic.Size());

    // Test more than capacity
    ic.Insert(mock(17), 17);
    check_fn(&ic, 1, 1, false);
    check_fn(&ic, 2, 17, true);
    check_fn(&ic, 2, 20, true);
    assert_eq!(16, ic.Size());

    // Test for there is a hole in the middle.
    ic = infoschema::NewCache(16);

    // mock for restart with full load the latest version schema.
    ic.Insert(mock(100), 100);
    check_fn(&ic, 1, 99, false);
    check_fn(&ic, 100, 100, true);

    for i in 1_i64..=16 {
        ic.Insert(mock(i), i as u64);
    }
    check_fn(&ic, 1, 1, false);
    check_fn(&ic, 2, 15, true);
    check_fn(&ic, 16, 16, false);
    check_fn(&ic, 100, 100, true);
    assert_eq!(16, ic.Size());

    for i in 85_i64..100 {
        ic.Insert(mock(i), i as u64);
    }
    check_fn(&ic, 1, 84, false);
    check_fn(&ic, 85, 100, true);
    assert_eq!(16, ic.Size());

    // Test cache with schema version hole, which is cause by schema version doesn't has related schema-diff.
    ic = infoschema::NewCache(16);
    for i in 1_i64..=8 {
        ic.Insert(mock(i), i as u64);
    }
    check_fn(&ic, 1, 10, true);
    // mock for schema version hole, schema-version 9 is missing.
    ic.Insert(mock(10), 10);
    check_fn(&ic, 1, 7, true);
    // without empty schema version map, get snapshot by ts 8, 9 will both failed.
    check_fn(&ic, 8, 9, false);
    check_fn(&ic, 10, 10, true);
    // add empty schema version 9.
    ic.InsertEmptySchemaVersion(9);
    // after set empty schema version, get snapshot by ts 8, 9 will both success.
    check_fn(&ic, 1, 8, true);
    check_fn(&ic, 10, 10, true);
    let is = ic.GetBySnapshotTS(9);
    assert!(is.is_some());
    // since schema version 9 is empty, so get by ts 9 will get schema which version is 8.
    assert_eq!(8_i64, is.unwrap().SchemaMetaVersion());
}

// test_cache_empty_schema_version 对应 Go 的 TestCacheEmptySchemaVersion：空 schema version 集合也遵循容量淘汰。
#[test]
fn test_cache_empty_schema_version() {
    let ic = infoschema::NewCache(16);
    assert_eq!(0, ic.GetEmptySchemaVersions().len());
    for i in 0_i64..16 {
        ic.InsertEmptySchemaVersion(i);
    }
    let empty_versions = ic.GetEmptySchemaVersions();
    assert_eq!(16, empty_versions.len());
    for i in 0_i64..16 {
        assert!(empty_versions.contains(&i));
    }

    for i in 16_i64..20 {
        ic.InsertEmptySchemaVersion(i);
    }
    let empty_versions = ic.GetEmptySchemaVersions();
    assert_eq!(16, empty_versions.len());
    for i in 4_i64..20 {
        assert!(empty_versions.contains(&i));
    }
}
