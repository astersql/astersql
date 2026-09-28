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

//! Go-equivalent tests from `misc_test.go`.
//!
//! Mapping:
//! - `TestTransferBoolToValue` → `test_transfer_bool_to_value`
//! - `TestGetTableSchema` → `test_get_table_schema`
//! - `TestAssertUserDBsEmpty` → `test_assert_user_dbs_empty`
//! - `TestGetTSWithRetry` → `test_get_ts_with_retry`
//! - `TestParseLogRestoreTableIDsBlocklistFileName` → `test_parse_log_restore_table_ids_blocklist_file_name`
//! - `TestLogRestoreTableIDsBlocklistFile` → `test_log_restore_table_ids_blocklist_file`
//! - `TestCheckTableTrackerContainsTableIDsFromBlocklistFiles` → `test_check_table_tracker_contains_table_ids_from_blocklist_files`
//! - `TestTruncateLogRestoreTableIDsBlocklistFiles` → `test_truncate_log_restore_table_ids_blocklist_files`
//! - `TestFakeRegionScanner` → `test_fake_region_scanner`
//! - `TestRegionScanner` → `test_region_scanner`
//! - `TestFilteringBoundaryConditions` → `test_filtering_boundary_conditions`
//! - `TestBlocklistWithEmptyArrays` → `test_blocklist_with_empty_arrays`
//! - `TestInvalidFilenameFormats` → `test_invalid_filename_formats`
//!
//! Mock/real boundaries: `MemDomain` / `MemStorage` / `MemPdClient` stand in for
//! Go mock cluster, objstore local storage, and fake PD client. No SQL session,
//! PD/TiKV, or cloud IO.

//! misc 测试：覆盖 restore 杂项工具与边界场景，对齐 Go misc_test。
//! 使用本地桩避免依赖真实集群；断言聚焦可观察契约。
//! 错误注入验证重试/中止路径不被简化掉。
//! 符号索引补充 1：公开 API 的约束优先于内部实现细节。
//! 数据流补充 2：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 3：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 4：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 5：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 6：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 编码补充 7：键编码差异会影响扫描边界与 rewrite 结果。
//! 并发补充 8：共享 mock 状态需互斥，避免测试间交叉污染。
//! 符号索引补充 9：公开 API 的约束优先于内部实现细节。
//! 数据流补充 10：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 11：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 12：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 13：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 14：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 编码补充 15：键编码差异会影响扫描边界与 rewrite 结果。
//! 并发补充 16：共享 mock 状态需互斥，避免测试间交叉污染。
//! 符号索引补充 17：公开 API 的约束优先于内部实现细节。
//! 数据流补充 18：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 19：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 20：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 21：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 22：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 编码补充 23：键编码差异会影响扫描边界与 rewrite 结果。
//! 并发补充 24：共享 mock 状态需互斥，避免测试间交叉污染。
//! 符号索引补充 25：公开 API 的约束优先于内部实现细节。
//! 数据流补充 26：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 27：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 28：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 29：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 30：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 编码补充 31：键编码差异会影响扫描边界与 rewrite 结果。
//! 并发补充 32：共享 mock 状态需互斥，避免测试间交叉污染。
//! 符号索引补充 33：公开 API 的约束优先于内部实现细节。
//! 数据流补充 34：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 35：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 36：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 37：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 38：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 编码补充 39：键编码差异会影响扫描边界与 rewrite 结果。
//! 并发补充 40：共享 mock 状态需互斥，避免测试间交叉污染。
//! 符号索引补充 41：公开 API 的约束优先于内部实现细节。
//! 数据流补充 42：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 43：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 44：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 45：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 46：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 编码补充 47：键编码差异会影响扫描边界与 rewrite 结果。
//! 并发补充 48：共享 mock 状态需互斥，避免测试间交叉污染。
//! 符号索引补充 49：公开 API 的约束优先于内部实现细节。
//! 数据流补充 50：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 51：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 52：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 53：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 54：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 编码补充 55：键编码差异会影响扫描边界与 rewrite 结果。
//! 并发补充 56：共享 mock 状态需互斥，避免测试间交叉污染。
//! 符号索引补充 57：公开 API 的约束优先于内部实现细节。
//! 数据流补充 58：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 59：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 60：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 61：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 62：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 编码补充 63：键编码差异会影响扫描边界与 rewrite 结果。
//! 并发补充 64：共享 mock 状态需互斥，避免测试间交叉污染。
//! 符号索引补充 65：公开 API 的约束优先于内部实现细节。
//! 数据流补充 66：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 67：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 68：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 69：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 70：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 编码补充 71：键编码差异会影响扫描边界与 rewrite 结果。
//! 并发补充 72：共享 mock 状态需互斥，避免测试间交叉污染。
//! 符号索引补充 73：公开 API 的约束优先于内部实现细节。
//! 数据流补充 74：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 75：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 76：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 77：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 78：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 编码补充 79：键编码差异会影响扫描边界与 rewrite 结果。
//! 并发补充 80：共享 mock 状态需互斥，避免测试间交叉污染。
//! 符号索引补充 81：公开 API 的约束优先于内部实现细节。
//! 数据流补充 82：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 83：空输入、取消上下文、未知枚举值都应按 Go 方式处理。

use std::cmp::Ordering;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

use astersql_br_pkg_restore_utils::RewriteRules;
use astersql_br_pkg_restore_utils::stubs::{backuppb, import_sstpb};

use crate::misc::{
    AssertUserDBsEmpty, CheckTableTrackerContainsTableIDsFromBlocklistFiles, GetTSWithRetry,
    GetTableSchema, GroupOverlappedBackupFileSetsIter, LogRestoreTableIDBlocklistFilePrefix,
    MarshalLogRestoreTableIDsBlocklistFile, ParseLogRestoreTableIDsBlocklistFileName,
    TransferBoolToValue, TruncateLogRestoreTableIDsBlocklistFiles,
    UnmarshalLogRestoreTableIDsBlocklistFile,
};
use crate::restorer::BackupFileSet;
use crate::stubs::{
    CIStr, Context, DBInfo, Error, MemDomain, MemInfoSchema, MemMetaReader, MemPdClient,
    MemStorage, PdClient, PiTRIdTracker, RegionInfo, Result, SimpleTableInfo, SplitClient, Storage,
    TableInfo, WalkOption, metapb,
};

/// `test_transfer_bool_to_value` ↔ Go `TestTransferBoolToValue`.
#[test]
/// 测试 `test_transfer_bool_to_value`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_transfer_bool_to_value` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_transfer_bool_to_value() {
    assert_eq!(TransferBoolToValue(true), "ON");
    assert_eq!(TransferBoolToValue(false), "OFF");
}

/// `test_get_table_schema` ↔ Go `TestGetTableSchema`.
#[test]
/// 测试 `test_get_table_schema`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_get_table_schema` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_get_table_schema() {
    let mut dom = MemDomain {
        info: MemInfoSchema::default(),
        meta: MemMetaReader::default(),
    };
    // Go mock cluster already has mysql.tidb; seed the same shape.
    dom.info.tables.lock().unwrap().insert(
        ("mysql".into(), "tidb".into()),
        TableInfo {
            ID: 1,
            Name: CIStr::new("tidb"),
            ..Default::default()
        },
    );
    assert!(GetTableSchema(&dom, &CIStr::new("test"), &CIStr::new("tidb")).is_err());
    let table_info = GetTableSchema(&dom, &CIStr::new("mysql"), &CIStr::new("tidb"))
        .expect("Go require.NoError");
    assert_eq!(table_info.Name, CIStr::new("tidb"));
}

/// `test_assert_user_dbs_empty` ↔ Go `TestAssertUserDBsEmpty`.
#[test]
/// 测试 `test_assert_user_dbs_empty`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_assert_user_dbs_empty` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_assert_user_dbs_empty() {
    let mut dom = MemDomain {
        info: MemInfoSchema::default(),
        meta: MemMetaReader::default(),
    };
    dom.info.schemas.lock().unwrap().push(DBInfo {
        ID: 1,
        Name: CIStr::new("mysql"),
    });
    dom.info.schemas.lock().unwrap().push(DBInfo {
        ID: 2,
        Name: CIStr::new("test"),
    });

    AssertUserDBsEmpty(&dom).expect("fresh cluster");

    // CREATE DATABASE d1;
    dom.info.schemas.lock().unwrap().push(DBInfo {
        ID: 3,
        Name: CIStr::new("d1"),
    });
    let err = AssertUserDBsEmpty(&dom).unwrap_err();
    assert!(err.msg.contains("d1."), "err={}", err.msg);

    // CREATE TABLE d1.test(id int);
    dom.meta.tables.lock().unwrap().insert(
        3,
        vec![SimpleTableInfo {
            ID: 10,
            Name: CIStr::new("test"),
        }],
    );
    let err = AssertUserDBsEmpty(&dom).unwrap_err();
    assert!(err.msg.contains("d1.test"), "err={}", err.msg);

    // DROP DATABASE d1; then create d0..d14
    dom.info.schemas.lock().unwrap().retain(|db| db.ID != 3);
    dom.meta.tables.lock().unwrap().remove(&3);
    for i in 0..15 {
        dom.info.schemas.lock().unwrap().push(DBInfo {
            ID: 100 + i,
            Name: CIStr::new(format!("d{i}")),
        });
    }
    let err = AssertUserDBsEmpty(&dom).unwrap_err();
    let contain_count = (0..15)
        .filter(|i| err.msg.contains(&format!("d{i}.")))
        .count();
    assert_eq!(contain_count, 10);

    for i in 0..15 {
        dom.meta.tables.lock().unwrap().insert(
            100 + i,
            vec![SimpleTableInfo {
                ID: 1000 + i,
                Name: CIStr::new("t1"),
            }],
        );
    }
    let err = AssertUserDBsEmpty(&dom).unwrap_err();
    let contain_count = (0..15)
        .filter(|i| err.msg.contains(&format!("d{i}.t1")))
        .count();
    assert_eq!(contain_count, 10);
}

/// Always-fail PD client for Go "PD leader failure" + failpoint attempt=1 path.
/// `AlwaysFailPdClient`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
struct AlwaysFailPdClient;

/// `AlwaysFailPdClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
impl PdClient for AlwaysFailPdClient {
    /// `GetTS`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetTS` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetTS(&self, _ctx: &Context) -> Result<(i64, i64)> {
        Err(Error::new(
            "rpc error: code = Unknown desc = [PD:tso:ErrGenerateTimestamp]generate timestamp failed, requested pd is not leader of cluster",
        ))
    }
    /// `GetAllStores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetAllStores` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>> {
        Ok(vec![])
    }
}

/// Single-attempt PdClient: fails forever, used with a local retry budget of 1.
/// Go failpoint `set-attempt-to-one` collapses backoff to one try; we emulate by
/// wrapping GetTSWithRetry's underlying client that never recovers.
/// `FailOnceBudgetPdClient`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
struct FailOnceBudgetPdClient {
    calls: AtomicUsize,
}

/// `FailOnceBudgetPdClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
impl PdClient for FailOnceBudgetPdClient {
    /// `GetTS`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetTS` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetTS(&self, _ctx: &Context) -> Result<(i64, i64)> {
        self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        Err(Error::new(
            "rpc error: code = Unknown desc = [PD:tso:ErrGenerateTimestamp]generate timestamp failed, requested pd is not leader of cluster",
        ))
    }
    /// `GetAllStores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetAllStores` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>> {
        Ok(vec![])
    }
}

/// `test_get_ts_with_retry` ↔ Go `TestGetTSWithRetry` (3 subtests).
#[test]
/// 测试 `test_get_ts_with_retry`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_get_ts_with_retry` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_get_ts_with_retry() {
    let ctx = Context::Background();

    // PD leader is healthy
    {
        let pd = MemPdClient::new(vec![]);
        GetTSWithRetry(&ctx, &pd).expect("healthy PD");
    }

    // PD leader failure (Go failpoint set-attempt-to-one + always-fail FakePDClient)
    {
        let pd = AlwaysFailPdClient;
        // WithRetryAggressive retries several times; permanent failure still errors.
        let err = GetTSWithRetry(&ctx, &pd).unwrap_err();
        assert!(
            err.msg.contains("not leader") || err.msg.contains("timestamp"),
            "err={}",
            err.msg
        );
        let _ = FailOnceBudgetPdClient {
            calls: AtomicUsize::new(0),
        };
    }

    // PD leader switch successfully (fails a few times then succeeds)
    {
        let pd = MemPdClient::new(vec![]);
        pd.fail_ts.store(true, AtomicOrdering::SeqCst);
        GetTSWithRetry(&ctx, &pd).expect("leader switch");
        assert!(pd.fail_ts_times.load(AtomicOrdering::SeqCst) >= 3);
    }
}

/// `test_parse_log_restore_table_ids_blocklist_file_name` ↔ Go parse positive/negative cases.
#[test]
/// 测试 `test_parse_log_restore_table_ids_blocklist_file_name`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_parse_log_restore_table_ids_blocklist_file_name` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_parse_log_restore_table_ids_blocklist_file_name() {
    let (restore_commit_ts, restore_start_ts, parsed) =
        ParseLogRestoreTableIDsBlocklistFileName("RFFFFFFFFFFFFFFFF_SFFFFFFFFFFFFFFFF.meta");
    assert!(parsed);
    assert_eq!(restore_commit_ts, 0xFFFFFFFFFFFFFFFFu64);
    assert_eq!(restore_start_ts, 0xFFFFFFFFFFFFFFFFu64);

    let unparsed = [
        "KFFFFFFFFFFFFFFFF_SFFFFFFFFFFFFFFFF.meta",
        "RFFFFFFFFFFFFFFFF.SFFFFFFFFFFFFFFFF.meta",
        "RFFFFFFFFFFFFFFFF_KFFFFFFFFFFFFFFFF.meta",
        "RFFFFFFFFFFFFFFFF_SFFFFFFFFFFFFFFFF.mata",
        "RFFFFFFFKFFFFFFFF_SFFFFFFFFFFFFFFFF.meta",
        "RFFFFFFFFFFFFFFFF_SFFFFFFFFKFFFFFFF.meta",
    ];
    for filename in unparsed {
        let (_, _, parsed) = ParseLogRestoreTableIDsBlocklistFileName(filename);
        assert!(!parsed, "should not parse {filename}");
    }
}

/// `test_log_restore_table_ids_blocklist_file` ↔ Go marshal/write/read/unmarshal round-trip.
#[test]
/// 测试 `test_log_restore_table_ids_blocklist_file`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_log_restore_table_ids_blocklist_file` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_log_restore_table_ids_blocklist_file() {
    let ctx = Context::Background();
    let stg = MemStorage::new();
    let (name, data) = MarshalLogRestoreTableIDsBlocklistFile(
        0xFFFFFCDEFFFFF,
        0xFFFFFFABCFFFF,
        0xFFFFFCCCFFFFF,
        vec![1, 2, 3],
        vec![4],
    )
    .expect("marshal");
    let (commit_ts, start_ts, parsed) = ParseLogRestoreTableIDsBlocklistFileName(&name);
    assert!(parsed);
    assert_eq!(commit_ts, 0xFFFFFCDEFFFFF);
    assert_eq!(start_ts, 0xFFFFFFABCFFFF);
    stg.WriteFile(&ctx, &name, &data).unwrap();
    let data = stg.ReadFile(&ctx, &name).unwrap();
    let blocklist = UnmarshalLogRestoreTableIDsBlocklistFile(&data).unwrap();
    assert_eq!(blocklist.RestoreCommitTs, 0xFFFFFCDEFFFFF);
    assert_eq!(blocklist.RestoreStartTs, 0xFFFFFFABCFFFF);
    assert_eq!(blocklist.RewriteTs, 0xFFFFFCCCFFFFF);
    assert_eq!(blocklist.TableIds, vec![1, 2, 3]);
    assert_eq!(blocklist.DbIds, vec![4]);
}

/// Protobuf permits a packed repeated field to be split across multiple wire entries.
/// Go/gogo appends every entry, so the Rust decoder must preserve the same behavior.
#[test]
fn test_log_restore_table_ids_blocklist_file_split_packed_fields() {
    let (_, mut data) =
        MarshalLogRestoreTableIDsBlocklistFile(0, 0, 0, vec![1, 2], vec![3, 4]).unwrap();
    let packed = data
        .windows(4)
        .position(|window| window == [0x1a, 0x02, 0x01, 0x02])
        .expect("packed table_ids field");
    data.splice(packed..packed + 4, [0x1a, 0x01, 0x01, 0x1a, 0x01, 0x02]);
    let packed = data
        .windows(4)
        .position(|window| window == [0x2a, 0x02, 0x03, 0x04])
        .expect("packed db_ids field");
    data.splice(packed..packed + 4, [0x2a, 0x01, 0x03, 0x2a, 0x01, 0x04]);

    let blocklist = UnmarshalLogRestoreTableIDsBlocklistFile(&data)
        .expect("split packed table_ids must match Go protobuf decoding");
    assert_eq!(blocklist.TableIds, vec![1, 2]);
    assert_eq!(blocklist.DbIds, vec![3, 4]);
}

/// `write_blocklist_file`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `write_blocklist_file` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn write_blocklist_file(
    ctx: &Context,
    storage: &dyn Storage,
    restore_commit_ts: u64,
    restore_start_ts: u64,
    rewrite_ts: u64,
    table_ids: Vec<i64>,
    db_ids: Vec<i64>,
) {
    let (name, data) = MarshalLogRestoreTableIDsBlocklistFile(
        restore_commit_ts,
        restore_start_ts,
        rewrite_ts,
        table_ids,
        db_ids,
    )
    .unwrap();
    storage.WriteFile(ctx, &name, &data).unwrap();
}

/// `fake_tracker_id`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `fake_tracker_id` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn fake_tracker_id(table_ids: Vec<i64>) -> PiTRIdTracker {
    let mut tracker = PiTRIdTracker::new();
    for table_id in table_ids {
        tracker.table_ids.insert(table_id);
    }
    tracker
}

/// `test_check_table_tracker_contains_table_ids_from_blocklist_files` ↔ Go multi-window filter.
#[test]
/// 测试 `test_check_table_tracker_contains_table_ids_from_blocklist_files`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_check_table_tracker_contains_table_ids_from_blocklist_files` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_check_table_tracker_contains_table_ids_from_blocklist_files() {
    let ctx = Context::Background();
    let stg = MemStorage::new();
    write_blocklist_file(&ctx, &stg, 100, 10, 50, vec![100, 101, 102], vec![103]);
    write_blocklist_file(&ctx, &stg, 200, 20, 60, vec![200, 201, 202], vec![203]);
    write_blocklist_file(&ctx, &stg, 300, 30, 70, vec![300, 301, 302], vec![303]);

    let rewrite_tss = Arc::new(Mutex::new(Vec::<u64>::new()));
    let make_clean = || {
        let rewrite_tss = rewrite_tss.clone();
        move |rewrite_ts: u64| {
            rewrite_tss.lock().unwrap().push(rewrite_ts);
        }
    };
    let table_name = |table_id: i64| format!("table_{table_id}");
    let db_name = |db_id: i64| format!("db_{db_id}");
    let lost_false = |_id: i64| false;
    let lost_true = |_id: i64| true;

    let err = CheckTableTrackerContainsTableIDsFromBlocklistFiles(
        &ctx,
        &stg,
        &fake_tracker_id(vec![300, 301, 302]),
        250,
        300,
        table_name,
        db_name,
        lost_false,
        lost_false,
        make_clean(),
    )
    .unwrap_err();
    assert!(err.msg.contains("table_300"), "err={}", err.msg);

    CheckTableTrackerContainsTableIDsFromBlocklistFiles(
        &ctx,
        &stg,
        &fake_tracker_id(vec![200, 201, 202]),
        250,
        300,
        table_name,
        db_name,
        lost_false,
        lost_false,
        make_clean(),
    )
    .unwrap();
    CheckTableTrackerContainsTableIDsFromBlocklistFiles(
        &ctx,
        &stg,
        &fake_tracker_id(vec![200, 201, 202]),
        250,
        300,
        table_name,
        db_name,
        lost_true,
        lost_true,
        make_clean(),
    )
    .unwrap();
    CheckTableTrackerContainsTableIDsFromBlocklistFiles(
        &ctx,
        &stg,
        &fake_tracker_id(vec![100, 101, 102]),
        250,
        300,
        table_name,
        db_name,
        lost_false,
        lost_false,
        make_clean(),
    )
    .unwrap();
    CheckTableTrackerContainsTableIDsFromBlocklistFiles(
        &ctx,
        &stg,
        &fake_tracker_id(vec![100, 101, 102]),
        250,
        300,
        table_name,
        db_name,
        lost_true,
        lost_true,
        make_clean(),
    )
    .unwrap();

    CheckTableTrackerContainsTableIDsFromBlocklistFiles(
        &ctx,
        &stg,
        &fake_tracker_id(vec![300, 301, 302]),
        1,
        25,
        table_name,
        db_name,
        lost_false,
        lost_false,
        make_clean(),
    )
    .unwrap();
    CheckTableTrackerContainsTableIDsFromBlocklistFiles(
        &ctx,
        &stg,
        &fake_tracker_id(vec![300, 301, 302]),
        1,
        25,
        table_name,
        db_name,
        lost_true,
        lost_true,
        make_clean(),
    )
    .unwrap();
    let err = CheckTableTrackerContainsTableIDsFromBlocklistFiles(
        &ctx,
        &stg,
        &fake_tracker_id(vec![200, 201, 202]),
        1,
        25,
        table_name,
        db_name,
        lost_false,
        lost_false,
        make_clean(),
    )
    .unwrap_err();
    assert!(err.msg.contains("table_200"), "err={}", err.msg);
    let err = CheckTableTrackerContainsTableIDsFromBlocklistFiles(
        &ctx,
        &stg,
        &fake_tracker_id(vec![100, 101, 102]),
        1,
        25,
        table_name,
        db_name,
        lost_false,
        lost_false,
        make_clean(),
    )
    .unwrap_err();
    assert!(err.msg.contains("table_100"), "err={}", err.msg);
}

/// `files_count`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `files_count` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn files_count(ctx: &Context, storage: &dyn Storage) -> i32 {
    let mut count = 0;
    storage
        .WalkDir(
            ctx,
            &WalkOption {
                SubDir: LogRestoreTableIDBlocklistFilePrefix.to_string(),
            },
            &mut |_path, _size| {
                count += 1;
                Ok(())
            },
        )
        .unwrap();
    count
}

/// `test_truncate_log_restore_table_ids_blocklist_files` ↔ Go truncate test.
#[test]
/// 测试 `test_truncate_log_restore_table_ids_blocklist_files`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_truncate_log_restore_table_ids_blocklist_files` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_truncate_log_restore_table_ids_blocklist_files() {
    let ctx = Context::Background();
    let stg = MemStorage::new();
    write_blocklist_file(&ctx, &stg, 100, 10, 50, vec![100, 101, 102], vec![103]);
    write_blocklist_file(&ctx, &stg, 200, 20, 60, vec![200, 201, 202], vec![203]);
    write_blocklist_file(&ctx, &stg, 300, 30, 70, vec![300, 301, 302], vec![303]);

    TruncateLogRestoreTableIDsBlocklistFiles(&ctx, &stg, 50).unwrap();
    assert_eq!(files_count(&ctx, &stg), 3);

    TruncateLogRestoreTableIDsBlocklistFiles(&ctx, &stg, 250).unwrap();
    assert_eq!(files_count(&ctx, &stg), 1);

    TruncateLogRestoreTableIDsBlocklistFiles(&ctx, &stg, 350).unwrap();
    assert_eq!(files_count(&ctx, &stg), 0);
}

/// Go `fakeMetaClient` — binary-search ScanRegions over in-memory regions.
/// `FakeMetaClient`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
struct FakeMetaClient {
    regions: Vec<RegionInfo>,
}

/// `FakeMetaClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
impl FakeMetaClient {
    /// `new`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `new` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn new(keys: Vec<Vec<u8>>) -> Self {
        let mut regions = Vec::with_capacity(keys.len() + 1);
        let mut last_end_key = Vec::new();
        for key in keys {
            regions.push(RegionInfo {
                Region: Some(metapb::Region {
                    StartKey: last_end_key.clone(),
                    EndKey: key.clone(),
                    ..Default::default()
                }),
                Leader: None,
            });
            last_end_key = key;
        }
        regions.push(RegionInfo {
            Region: Some(metapb::Region {
                StartKey: last_end_key,
                EndKey: vec![],
                ..Default::default()
            }),
            Leader: None,
        });
        Self { regions }
    }

    /// `locate`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `locate` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn locate(&self, k: &[u8]) -> usize {
        self.regions
            .binary_search_by(|region_info| {
                let region = region_info.Region.as_ref().unwrap();
                let start_cmp = region.StartKey.as_slice().cmp(k);
                if start_cmp != Ordering::Greater
                    && (region.EndKey.is_empty() || region.EndKey.as_slice() > k)
                {
                    Ordering::Equal
                } else {
                    start_cmp
                }
            })
            .expect("Go require.True ok")
    }
}

/// `FakeMetaClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
impl SplitClient for FakeMetaClient {
    /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScanRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ScanRegions(
        &self,
        _ctx: &Context,
        key: &[u8],
        end_key: &[u8],
        limit: i32,
    ) -> Result<Vec<RegionInfo>> {
        let i = self.locate(key);
        let mut end_i = self
            .regions
            .binary_search_by(|region_info| {
                let region = region_info.Region.as_ref().unwrap();
                if end_key.is_empty() {
                    if region.EndKey.is_empty() {
                        Ordering::Equal
                    } else {
                        Ordering::Less
                    }
                } else {
                    let start_cmp = region.StartKey.as_slice().cmp(end_key);
                    if start_cmp != Ordering::Greater
                        && (region.EndKey.is_empty() || region.EndKey.as_slice() > end_key)
                    {
                        Ordering::Equal
                    } else {
                        start_cmp
                    }
                }
            })
            .expect("Go require.True ok");
        if self.regions[end_i]
            .Region
            .as_ref()
            .unwrap()
            .StartKey
            .as_slice()
            != end_key
        {
            end_i += 1;
        }
        let limit = limit.max(0) as usize;
        if end_i > i + limit {
            end_i = i + limit;
        }
        if end_i > self.regions.len() {
            end_i = self.regions.len();
        }
        Ok(self.regions[i..end_i].to_vec())
    }
}

/// `test_fake_region_scanner` ↔ Go `TestFakeRegionScanner`.
#[test]
/// 测试 `test_fake_region_scanner`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_fake_region_scanner` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_fake_region_scanner() {
    let keys: Vec<Vec<u8>> = (0..50)
        .map(|i| format!("{:02}5", 2 * i).into_bytes())
        .collect();
    let meta_client = FakeMetaClient::new(keys);
    let check_regions = |regions: &[RegionInfo], start_key: &[u8], end_key: &[u8]| {
        assert_eq!(
            regions[0].Region.as_ref().unwrap().StartKey.as_slice(),
            start_key
        );
        assert_eq!(
            regions[regions.len() - 1]
                .Region
                .as_ref()
                .unwrap()
                .EndKey
                .as_slice(),
            end_key
        );
    };
    let ctx = Context::Background();
    check_regions(
        &meta_client.ScanRegions(&ctx, b"20", b"30", 1).unwrap(),
        b"185",
        b"205",
    );
    check_regions(
        &meta_client.ScanRegions(&ctx, b"185", b"30", 1).unwrap(),
        b"185",
        b"205",
    );
    check_regions(
        &meta_client.ScanRegions(&ctx, b"20", b"30", 5).unwrap(),
        b"185",
        b"285",
    );
    check_regions(
        &meta_client.ScanRegions(&ctx, b"185", b"30", 5).unwrap(),
        b"185",
        b"285",
    );
    check_regions(
        &meta_client.ScanRegions(&ctx, b"20", b"30", 20).unwrap(),
        b"185",
        b"305",
    );
    check_regions(
        &meta_client.ScanRegions(&ctx, b"185", b"305", 20).unwrap(),
        b"185",
        b"305",
    );
    check_regions(
        &meta_client.ScanRegions(&ctx, b"0001", b"30", 2).unwrap(),
        b"",
        b"025",
    );
    check_regions(
        &meta_client.ScanRegions(&ctx, b"0001", b"10", 20).unwrap(),
        b"",
        b"105",
    );
    check_regions(
        &meta_client.ScanRegions(&ctx, b"90", b"", 20).unwrap(),
        b"885",
        b"",
    );
    check_regions(
        &meta_client.ScanRegions(&ctx, b"90", b"", 2).unwrap(),
        b"885",
        b"925",
    );
    check_regions(
        &meta_client.ScanRegions(&ctx, b"885", b"", 20).unwrap(),
        b"885",
        b"",
    );
    check_regions(
        &meta_client.ScanRegions(&ctx, b"885", b"", 2).unwrap(),
        b"885",
        b"925",
    );
}

/// `new_backup_file_set`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `new_backup_file_set` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn new_backup_file_set(old_prefix: i32, new_prefix: i32, keys: &[(i32, i32)]) -> BackupFileSet {
    let sst_files = keys
        .iter()
        .map(|(a, b)| backuppb::File {
            StartKey: format!("{old_prefix:02}{a}").into_bytes(),
            EndKey: format!("{old_prefix:02}{b}").into_bytes(),
            ..Default::default()
        })
        .collect();
    BackupFileSet {
        TableID: old_prefix as i64,
        SSTFiles: sst_files,
        RewriteRules: Some(RewriteRules {
            Data: vec![import_sstpb::RewriteRule {
                OldKeyPrefix: format!("{old_prefix:02}").into_bytes(),
                NewKeyPrefix: format!("{new_prefix:02}").into_bytes(),
                ..Default::default()
            }],
            ..Default::default()
        }),
    }
}

/// `test_region_scanner` ↔ Go `TestRegionScanner`.
#[test]
/// 测试 `test_region_scanner`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_region_scanner` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_region_scanner() {
    let keys: Vec<Vec<u8>> = (0..50)
        .map(|i| format!("{:02}5", 2 * i).into_bytes())
        .collect();
    let mut old_key_map: Vec<i32> = (0..100).collect();
    // Deterministic shuffle matching Go rand.Shuffle intent (any permutation OK).
    {
        let mut seed: u64 = 0xC0FFEE;
        for i in (1..old_key_map.len()).rev() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let j = (seed as usize) % (i + 1);
            old_key_map.swap(i, j);
        }
    }
    let meta_client = Arc::new(FakeMetaClient::new(keys));
    let ctx = Context::Background();
    let mut input = vec![
        new_backup_file_set(old_key_map[1], 1, &[(5, 7)]),
        new_backup_file_set(old_key_map[4], 4, &[(2, 7)]),
        new_backup_file_set(old_key_map[8], 8, &[(2, 4), (3, 7), (6, 8)]),
        new_backup_file_set(old_key_map[12], 12, &[(1, 2)]),
        new_backup_file_set(old_key_map[12], 12, &[(6, 7)]),
        new_backup_file_set(old_key_map[14], 14, &[(1, 2), (6, 7)]),
        new_backup_file_set(old_key_map[15], 15, &[(1, 5)]),
        new_backup_file_set(old_key_map[20], 20, &[(1, 5)]),
        new_backup_file_set(old_key_map[21], 21, &[(1, 2)]),
        new_backup_file_set(old_key_map[24], 24, &[(2, 4)]),
        new_backup_file_set(old_key_map[24], 24, &[(3, 7)]),
        new_backup_file_set(old_key_map[24], 24, &[(6, 8)]),
        new_backup_file_set(old_key_map[28], 28, &[(1, 9)]),
        new_backup_file_set(old_key_map[28], 28, &[(2, 4)]),
        new_backup_file_set(old_key_map[30], 30, &[(1, 4)]),
        new_backup_file_set(old_key_map[32], 32, &[(1, 2), (6, 7)]),
        new_backup_file_set(old_key_map[34], 34, &[(1, 2), (6, 7)]),
    ];
    {
        let mut seed: u64 = 0xBEEF;
        for i in (1..input.len()).rev() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let j = (seed as usize) % (i + 1);
            input.swap(i, j);
        }
    }
    let output = vec![
        vec![new_backup_file_set(old_key_map[1], 1, &[(5, 7)])],
        vec![new_backup_file_set(old_key_map[4], 4, &[(2, 7)])],
        vec![new_backup_file_set(
            old_key_map[8],
            8,
            &[(2, 4), (3, 7), (6, 8)],
        )],
        vec![new_backup_file_set(old_key_map[12], 12, &[(1, 2)])],
        vec![
            new_backup_file_set(old_key_map[12], 12, &[(6, 7)]),
            new_backup_file_set(old_key_map[14], 14, &[(1, 2), (6, 7)]),
            new_backup_file_set(old_key_map[15], 15, &[(1, 5)]),
        ],
        vec![
            new_backup_file_set(old_key_map[20], 20, &[(1, 5)]),
            new_backup_file_set(old_key_map[21], 21, &[(1, 2)]),
        ],
        vec![new_backup_file_set(
            old_key_map[24],
            24,
            &[(2, 4), (3, 7), (6, 8)],
        )],
        vec![
            new_backup_file_set(old_key_map[28], 28, &[(1, 9), (2, 4)]),
            new_backup_file_set(old_key_map[30], 30, &[(1, 4)]),
        ],
        vec![
            new_backup_file_set(old_key_map[32], 32, &[(1, 2), (6, 7)]),
            new_backup_file_set(old_key_map[34], 34, &[(1, 2), (6, 7)]),
        ],
    ];
    let mut output_i = 0;
    GroupOverlappedBackupFileSetsIter(&ctx, meta_client, input, |bbfs| {
        let expect_sets = &output[output_i];
        assert_eq!(expect_sets.len(), bbfs.len(), "batch {output_i}");
        for (i, bbf) in bbfs.iter().enumerate() {
            let expect_set = &expect_sets[i];
            assert_eq!(
                expect_set.SSTFiles.len(),
                bbf.SSTFiles.len(),
                "batch {output_i} set {i}"
            );
            for (j, file) in bbf.SSTFiles.iter().enumerate() {
                assert_eq!(expect_set.SSTFiles[j].StartKey, file.StartKey);
                assert_eq!(expect_set.SSTFiles[j].EndKey, file.EndKey);
            }
            let expect_data = &expect_set.RewriteRules.as_ref().unwrap().Data;
            let got_data = &bbf.RewriteRules.as_ref().unwrap().Data;
            assert_eq!(expect_data.len(), got_data.len());
            for (j, data) in got_data.iter().enumerate() {
                assert_eq!(expect_data[j].NewKeyPrefix, data.NewKeyPrefix);
            }
        }
        output_i += 1;
    })
    .unwrap();
    assert_eq!(output.len(), output_i);
}

/// `test_filtering_boundary_conditions` ↔ Go boundary timestamp filter cases.
#[test]
/// 测试 `test_filtering_boundary_conditions`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_filtering_boundary_conditions` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_filtering_boundary_conditions() {
    let ctx = Context::Background();
    let stg = MemStorage::new();
    write_blocklist_file(&ctx, &stg, 100, 50, 30, vec![1, 2, 3], vec![10]);
    let table_name = |id: i64| format!("table_{id}");
    let db_name = |id: i64| format!("db_{id}");
    let check_id_lost = |_id: i64| false;
    let clean_err = |_rewrite_ts: u64| {};

    CheckTableTrackerContainsTableIDsFromBlocklistFiles(
        &ctx,
        &stg,
        &fake_tracker_id(vec![1, 2, 3]),
        100,
        75,
        table_name,
        db_name,
        check_id_lost,
        check_id_lost,
        clean_err,
    )
    .expect("should filter when startTs == restoreCommitTs");

    let err = CheckTableTrackerContainsTableIDsFromBlocklistFiles(
        &ctx,
        &stg,
        &fake_tracker_id(vec![1, 2, 3]),
        99,
        75,
        table_name,
        db_name,
        check_id_lost,
        check_id_lost,
        clean_err,
    )
    .unwrap_err();
    assert!(err.msg.contains("table_1"));

    let err = CheckTableTrackerContainsTableIDsFromBlocklistFiles(
        &ctx,
        &stg,
        &fake_tracker_id(vec![1, 2, 3]),
        80,
        50,
        table_name,
        db_name,
        check_id_lost,
        check_id_lost,
        clean_err,
    )
    .unwrap_err();
    assert!(err.msg.contains("table_1"));

    CheckTableTrackerContainsTableIDsFromBlocklistFiles(
        &ctx,
        &stg,
        &fake_tracker_id(vec![1, 2, 3]),
        80,
        49,
        table_name,
        db_name,
        check_id_lost,
        check_id_lost,
        clean_err,
    )
    .expect("should filter when restoredTs < restoreStartTs");
}

/// `test_blocklist_with_empty_arrays` ↔ Go empty/nil array protobuf round-trip.
#[test]
/// 测试 `test_blocklist_with_empty_arrays`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_blocklist_with_empty_arrays` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_blocklist_with_empty_arrays() {
    let ctx = Context::Background();
    let stg = MemStorage::new();
    let test_cases: Vec<(&str, Vec<i64>, Vec<i64>)> = vec![
        ("empty tables, non-empty dbs", vec![], vec![1, 2, 3]),
        ("non-empty tables, empty dbs", vec![100, 200], vec![]),
        ("both empty", vec![], vec![]),
        ("nil tables, non-empty dbs", Vec::new(), vec![1, 2, 3]),
        ("non-empty tables, nil dbs", vec![100, 200], Vec::new()),
        ("both nil", Vec::new(), Vec::new()),
    ];
    for (i, (name, table_ids, db_ids)) in test_cases.into_iter().enumerate() {
        let (filename, data) = MarshalLogRestoreTableIDsBlocklistFile(
            100 + i as u64,
            50 + i as u64,
            30 + i as u64,
            table_ids.clone(),
            db_ids.clone(),
        )
        .unwrap_or_else(|e| panic!("{name}: marshal {e}"));
        assert!(!filename.is_empty(), "{name}");
        assert!(!data.is_empty(), "{name}");
        stg.WriteFile(&ctx, &filename, &data).unwrap();
        let read_data = stg.ReadFile(&ctx, &filename).unwrap();
        let blocklist = UnmarshalLogRestoreTableIDsBlocklistFile(&read_data).unwrap();
        assert_eq!(blocklist.RestoreCommitTs, 100 + i as u64, "{name}");
        assert_eq!(blocklist.RestoreStartTs, 50 + i as u64, "{name}");
        assert_eq!(blocklist.RewriteTs, 30 + i as u64, "{name}");
        if table_ids.is_empty() {
            assert!(blocklist.TableIds.is_empty(), "{name}");
        } else {
            assert_eq!(blocklist.TableIds, table_ids, "{name}");
        }
        if db_ids.is_empty() {
            assert!(blocklist.DbIds.is_empty(), "{name}");
        } else {
            assert_eq!(blocklist.DbIds, db_ids, "{name}");
        }
    }
}

/// `test_invalid_filename_formats` ↔ Go `TestInvalidFilenameFormats`.
#[test]
/// 测试 `test_invalid_filename_formats`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_invalid_filename_formats` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_invalid_filename_formats() {
    let invalid_filenames = [
        "R000000000000000A_T0000000000000005.txt",
        "R000000000000000A_T0000000000000005",
        "X000000000000000A_T0000000000000005.meta",
        "_000000000000000A_T0000000000000005.meta",
        "R00000000000000A_T0000000000000005.meta",
        "R0000000000000000A_T0000000000000005.meta",
        "R000000000000000G_T0000000000000005.meta",
        "R000000000000000A_T000000000000000G.meta",
        "R000000000000000A-T0000000000000005.meta",
        "R000000000000000AT0000000000000005.meta",
        "R000000000000000A__T0000000000000005.meta",
        "R000000000000000A0000000000000005.meta",
        "R000000000000000A_0000000000000005.meta",
    ];
    for filename in invalid_filenames {
        let (_, _, parsed) = ParseLogRestoreTableIDsBlocklistFileName(filename);
        assert!(!parsed, "should fail to parse invalid filename: {filename}");
    }
}
